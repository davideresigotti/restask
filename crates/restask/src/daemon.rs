//! Daemon (§13.1): a `notify` watcher plus a poll timer feed one debounced queue; a
//! single reconciler task owns every mutation. `SIGTERM`/`SIGINT` arrive as a
//! `tokio::sync::watch` signal; the in-flight reconcile finishes (state files are written
//! at its end), then the loop exits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use notify::Watcher as _;

use crate::caldav::client::CaldavClient;
use crate::caldav::CaldavPort;
use crate::config::{MachineConfig, VaultConfig};
use crate::domain::{Clock, SystemClock};
use crate::sync::engine::{Engine, ReconcileReport};
use crate::TaskresError;

/// Daemon knobs (§13.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// Events for one path coalesce for this long before a reconcile fires.
    pub debounce_ms: u64,
    /// Poll interval in seconds (backup for missed watcher events).
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
/// `restask sync` and tests).
pub async fn run_once(
    vault: &Path,
    machine: &MachineConfig,
    clock: Arc<dyn Clock>,
) -> Result<ReconcileReport, TaskresError> {
    let caldav = build_client(machine)?;
    run_once_with(vault, machine, clock, caldav).await
}

/// [`run_once`] with an injected port (hermetic tests, §3).
pub async fn run_once_with<C: CaldavPort>(
    vault: &Path,
    machine: &MachineConfig,
    clock: Arc<dyn Clock>,
    caldav: C,
) -> Result<ReconcileReport, TaskresError> {
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
) -> Result<(), TaskresError> {
    let caldav = build_client(&machine)?;
    run_with(vault, machine, dc, shutdown, caldav, Arc::new(SystemClock)).await
}

/// [`run`] with an injected port and clock (hermetic tests, §3).
pub async fn run_with<C: CaldavPort>(
    vault: PathBuf,
    machine: MachineConfig,
    dc: DaemonConfig,
    mut shutdown: watch::Receiver<bool>,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<(), TaskresError> {
    let cfg = load_vault_config(&vault)?;
    let engine = Arc::new(Engine::new(&vault, cfg, machine, caldav, clock));

    if dc.once {
        engine.reconcile().await?;
        return Ok(());
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let _watcher = spawn_watcher(&vault, tx)?;
    let mut poll = tokio::time::interval(Duration::from_secs(dc.poll_secs.max(1)));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut queue = DebounceQueue::default();

    loop {
        if *shutdown.borrow() {
            break;
        }
        for path in queue.due(Instant::now()) {
            tracing::debug!(path = %path.display(), "debounced fs event");
            // A failed reconcile (e.g. transient network) must not kill the daemon; the
            // next poll tick retries.
            if let Err(error) = engine.handle_fs_change(&path).await {
                tracing::error!(%error, "reconcile failed");
            }
        }
        let deadline = queue.next_deadline();
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(path) => {
                    queue.push(path, Instant::now() + Duration::from_millis(dc.debounce_ms));
                }
                None => break,
            },
            _ = poll.tick() => {
                queue.push(vault.clone(), Instant::now() + Duration::from_millis(dc.debounce_ms));
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {},
            _ = shutdown.changed() => {},
        }
    }
    tracing::info!("daemon stopped");
    Ok(())
}

/// Debounce registry (§13.1): one fire-at instant per path. A new event for a path
/// refreshes (postpones) it, so a burst of writes coalesces into a single reconcile.
#[derive(Debug, Default)]
pub struct DebounceQueue {
    pending: BTreeMap<PathBuf, Instant>,
}

impl DebounceQueue {
    /// Registers an event for `path` firing at `fire_at`, replacing any earlier deadline.
    pub fn push(&mut self, path: PathBuf, fire_at: Instant) {
        self.pending.insert(path, fire_at);
    }

    /// Drains every path whose deadline has passed, in path order.
    pub fn due(&mut self, now: Instant) -> Vec<PathBuf> {
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &due {
            self.pending.remove(path);
        }
        due
    }

    /// The earliest pending deadline, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().copied().min()
    }

    /// Whether nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Spawns the recursive `*.md` watcher for the vault (§13.1); the returned watcher must
/// stay alive for the daemon's lifetime.
fn spawn_watcher(
    vault: &Path,
    tx: mpsc::UnboundedSender<PathBuf>,
) -> Result<notify::RecommendedWatcher, TaskresError> {
    let mut watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            if let Ok(event) = event {
                for path in event.paths {
                    if path.extension().is_some_and(|ext| ext == "md") {
                        let _ = tx.send(path);
                    }
                }
            }
        })
        .map_err(|error| TaskresError::Validation {
            field: "watcher",
            reason: error.to_string(),
        })?;
    watcher
        .watch(vault, notify::RecursiveMode::Recursive)
        .map_err(|error| TaskresError::Validation {
            field: "watcher",
            reason: error.to_string(),
        })?;
    Ok(watcher)
}

/// Builds the real CalDAV client from the machine config (§10.3, §14.2).
pub(crate) fn build_client(machine: &MachineConfig) -> Result<CaldavClient, TaskresError> {
    let url = machine
        .caldav
        .url
        .clone()
        .ok_or_else(|| TaskresError::Config {
            path: crate::config::machine_config_path().display().to_string(),
            reason: "no CalDAV url configured (run `restask setup`)".to_string(),
        })?;
    let username = machine
        .caldav
        .username
        .clone()
        .ok_or_else(|| TaskresError::Config {
            path: crate::config::machine_config_path().display().to_string(),
            reason: "no CalDAV username configured (run `restask setup`)".to_string(),
        })?;
    let password = machine
        .resolved_password()
        .map_err(|error| TaskresError::Config {
            path: crate::config::machine_config_path().display().to_string(),
            reason: error.to_string(),
        })?;
    CaldavClient::new(&url, username, password)
}

/// Loads `<vault>/restask.toml`, mapping config failures to [`TaskresError::Config`].
fn load_vault_config(vault: &Path) -> Result<VaultConfig, TaskresError> {
    let path = vault.join("restask.toml");
    VaultConfig::load(&path).map_err(|error| TaskresError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}
