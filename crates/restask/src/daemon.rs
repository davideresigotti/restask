//! Daemon (§13.1): a `notify` watcher plus a poll timer mark the vault dirty; a single
//! reconciler loop owns every mutation. `SIGTERM`/`SIGINT` arrive as a
//! `tokio::sync::watch` signal; an in-flight reconcile finishes (its state writes are the
//! last thing it does), then the loop exits.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use notify::Watcher as _;

use crate::caldav::client::CaldavClient;
use crate::caldav::CaldavPort;
use crate::config::{MachineConfig, VaultConfig, VaultMatchers};
use crate::domain::{Clock, SystemClock};
use crate::sync::engine::{Engine, ReconcileReport};
use crate::vault;
use crate::{CaldavErrorKind, RestaskError};

/// Daemon knobs (§13.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// A burst of file events coalesces for this long before one reconcile fires.
    pub debounce_ms: u64,
    /// Poll interval in seconds: picks up server-side changes (and missed file events).
    pub poll_secs: u64,
    /// Perform a single reconcile and exit (`restask daemon --once`).
    pub once: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 300,
            poll_secs: 300,
            once: false,
        }
    }
}

/// Performs a single full reconcile with the real CalDAV client (§13.1; used by
/// `restask sync`).
pub async fn run_once(
    vault: &Path,
    machine: &MachineConfig,
    clock: Arc<dyn Clock>,
) -> Result<ReconcileReport, RestaskError> {
    let caldav = build_client(machine)?;
    run_once_with(vault, machine, clock, caldav).await
}

/// [`run_once`] with an injected port (hermetic tests).
pub async fn run_once_with<C: CaldavPort>(
    vault: &Path,
    machine: &MachineConfig,
    clock: Arc<dyn Clock>,
    caldav: C,
) -> Result<ReconcileReport, RestaskError> {
    let cfg = load_vault_config(vault)?;
    let engine = Engine::new(vault, cfg, machine.clone(), caldav, clock);
    engine.reconcile().await
}

/// The daemon loop (§13.1). Returns when the shutdown signal fires (or the event channel
/// closes); the last reconcile's state writes are already on disk by then.
pub async fn run(
    vault: PathBuf,
    machine: MachineConfig,
    dc: DaemonConfig,
    shutdown: watch::Receiver<bool>,
) -> Result<(), RestaskError> {
    let caldav = build_client(&machine)?;
    run_with(vault, machine, dc, shutdown, caldav, Arc::new(SystemClock)).await
}

/// [`run`] with an injected port and clock (hermetic tests).
pub async fn run_with<C: CaldavPort>(
    vault: PathBuf,
    machine: MachineConfig,
    dc: DaemonConfig,
    mut shutdown: watch::Receiver<bool>,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<(), RestaskError> {
    let cfg = load_vault_config(&vault)?;
    let matchers = cfg.matchers().map_err(|error| RestaskError::Config {
        path: vault.join("restask.toml").display().to_string(),
        reason: error.to_string(),
    })?;
    let engine = Engine::new(&vault, cfg, machine, caldav, clock);

    if dc.once {
        engine.reconcile().await?;
        return Ok(());
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let _watcher = spawn_watcher(&vault, matchers, tx)?;
    let mut poll = tokio::time::interval(Duration::from_secs(dc.poll_secs.max(1)));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let debounce = Duration::from_millis(dc.debounce_ms);
    let mut pending = Debounce::default();

    loop {
        if *shutdown.borrow() {
            break;
        }
        if pending.fire(Instant::now()) {
            // Whatever happened to the vault since the last pass, one reconcile covers
            // it. A failed pass (e.g. the server is down) must not kill the daemon; the
            // next event or poll tick retries.
            match engine.reconcile().await {
                Ok(report) => tracing::debug!(?report, "reconciled"),
                Err(
                    error @ RestaskError::Caldav {
                        kind: CaldavErrorKind::Auth,
                        ..
                    },
                ) => tracing::error!(%error, "auth_warning"),
                Err(error) => tracing::error!(%error, "reconcile failed"),
            }
            continue;
        }
        tokio::select! {
            event = rx.recv() => match event {
                Some(()) => pending.touch(Instant::now() + debounce),
                None => break,
            },
            // The first tick is immediate: the daemon reconciles on start.
            _ = poll.tick() => pending.touch(Instant::now()),
            _ = sleep_until(pending.deadline()) => {},
            _ = shutdown.changed() => {},
        }
    }
    tracing::info!("daemon stopped");
    Ok(())
}

/// Sleeps until `deadline`, or forever when there is none.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Debounce state (§13.1): the vault is either clean or dirty with a fire-at instant.
/// Every new event postpones the instant, so a burst of writes — an editor saving, file
/// sync delivering a batch — coalesces into a single reconcile.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Debounce {
    deadline: Option<Instant>,
}

impl Debounce {
    /// Marks the vault dirty, to be reconciled at `fire_at` (replacing an earlier instant).
    pub fn touch(&mut self, fire_at: Instant) {
        self.deadline = Some(fire_at);
    }

    /// Returns `true` — and resets to clean — when the fire-at instant has passed.
    pub fn fire(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(deadline) if deadline <= now => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }

    /// The pending fire-at instant, if the vault is dirty.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Spawns the recursive vault watcher (§13.1); the returned watcher must stay alive for
/// the daemon's lifetime. Only events on notes the scan would read (or on directories)
/// wake the reconciler — the engine's own state writes under `.restask/` never do.
fn spawn_watcher(
    vault: &Path,
    matchers: VaultMatchers,
    tx: mpsc::UnboundedSender<()>,
) -> Result<notify::RecommendedWatcher, RestaskError> {
    // Event paths are absolute and canonical; compare against the canonical vault root.
    let root = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    let mut watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            let Ok(event) = event else {
                return;
            };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            let relevant = event.paths.iter().any(|path| {
                path.strip_prefix(&root)
                    .ok()
                    .and_then(Path::to_str)
                    .is_some_and(|relative| {
                        vault::is_relevant_event(&relative.replace('\\', "/"), &matchers)
                    })
            });
            if relevant {
                let _ = tx.send(());
            }
        })
        .map_err(|error| RestaskError::Validation {
            field: "watcher",
            reason: error.to_string(),
        })?;
    watcher
        .watch(vault, notify::RecursiveMode::Recursive)
        .map_err(|error| RestaskError::Validation {
            field: "watcher",
            reason: error.to_string(),
        })?;
    Ok(watcher)
}

/// Builds the real CalDAV client from the machine config (§10.3, §14.2).
pub(crate) fn build_client(machine: &MachineConfig) -> Result<CaldavClient, RestaskError> {
    let missing = |what: &str| RestaskError::Config {
        path: crate::config::machine_config_path().display().to_string(),
        reason: format!("no CalDAV {what} configured (run `restask setup`)"),
    };
    let url = machine.caldav.url.clone().ok_or_else(|| missing("url"))?;
    let username = machine
        .caldav
        .username
        .clone()
        .ok_or_else(|| missing("username"))?;
    let password = machine
        .resolved_password()
        .map_err(|error| RestaskError::Config {
            path: crate::config::machine_config_path().display().to_string(),
            reason: error.to_string(),
        })?;
    CaldavClient::new(&url, username, password)
}

/// Loads `<vault>/restask.toml`, mapping config failures to [`RestaskError::Config`].
pub(crate) fn load_vault_config(vault: &Path) -> Result<VaultConfig, RestaskError> {
    let path = vault.join("restask.toml");
    VaultConfig::load(&path).map_err(|error| RestaskError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}
