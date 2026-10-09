//! Daemon (§13.1): a `notify` watcher, a look at the server's change tags and a poll
//! timer mark the vault dirty; a single reconciler loop owns every mutation. `SIGTERM`/`SIGINT` arrive as a
//! `tokio::sync::watch` signal; an in-flight reconcile finishes (its state writes are the
//! last thing it does), then the loop exits.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use notify::Watcher as _;

use crate::caldav::client::CaldavClient;
use crate::caldav::{CaldavPort, CollectionInfo};
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
    /// Poll interval in seconds: a pass whether or not anything was seen to change — the
    /// net under the watcher and the server watch.
    pub poll_secs: u64,
    /// How often the daemon asks the server whether another client wrote there, in
    /// milliseconds ([`server_tags`]); a changed answer starts a pass. `0` never asks:
    /// server-side changes then wait for the poll.
    pub watch_ms: u64,
    /// How long a note that still needs local work is left alone after it was last
    /// written, in milliseconds (§13.1): the device it is being edited on gets that
    /// long to do the work itself. `0` never waits.
    pub settle_ms: u64,
    /// Perform a single reconcile and exit (`restask daemon --once`).
    pub once: bool,
}

/// What woke the reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// A note of the vault was written, here or by the file sync.
    Vault,
    /// Another client wrote to the server.
    Server,
}

/// How many times [`DaemonConfig::settle_ms`] a pass is held back at most: a vault that
/// some program rewrites all the time is still synced.
const SETTLE_LIMIT: u32 = 6;

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 300,
            poll_secs: 300,
            watch_ms: 2_000,
            settle_ms: 10_000,
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
    let server = caldav.clone();
    let mut engine = Engine::new(&vault, cfg, machine, caldav, clock);

    if dc.once {
        engine.reconcile().await?;
        return Ok(());
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Wake>();
    let _watcher = spawn_watcher(&vault, matchers, tx.clone())?;
    let server_watch = if dc.watch_ms > 0 {
        // The first look is taken before the first pass: whatever another client writes
        // from here on differs from it, also while that pass is running.
        let seen = server
            .list_collections()
            .await
            .ok()
            .map(|collections| server_tags(&collections));
        Some(spawn_server_watch(
            server,
            Duration::from_millis(dc.watch_ms),
            seen,
            tx,
        ))
    } else {
        None
    };
    let mut poll = tokio::time::interval(Duration::from_secs(dc.poll_secs.max(1)));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let debounce = Duration::from_millis(dc.debounce_ms);
    let settle = Duration::from_millis(dc.settle_ms);
    let mut pending = Debounce::default();
    // When a note was last seen to change, and since when a pass is being held back.
    let mut changed: Option<Instant> = None;
    let mut held: Option<Instant> = None;

    loop {
        if *shutdown.borrow() {
            break;
        }
        if pending.fire(Instant::now()) {
            // Whatever happened to the vault since the last pass, one reconcile covers
            // it. A failed pass (e.g. the server is down) must not kill the daemon; the
            // next event or poll tick retries.
            refresh_config(&vault, &mut engine);
            // A note written a moment ago that still needs local work — a line without
            // its UID, a view that is not filed — is most likely being edited: the
            // device it is edited on does that work when the edit is finished (§1.1).
            // A pass now would write into the file under the editor, and the file sync
            // would meet two versions of it.
            if let Some(until) = settling(&engine, changed, settle, &mut held).await {
                pending.touch(until);
                continue;
            }
            follow_calendars(&mut engine).await;
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
                Some(wake) => {
                    let now = Instant::now();
                    if wake == Wake::Vault {
                        changed = Some(now);
                    }
                    pending.touch(now + debounce);
                }
                None => break,
            },
            // The first tick is immediate: the daemon reconciles on start.
            _ = poll.tick() => pending.touch(Instant::now()),
            _ = sleep_until(pending.deadline()) => {},
            _ = shutdown.changed() => {},
        }
    }
    if let Some(server_watch) = server_watch {
        server_watch.abort();
    }
    tracing::info!("daemon stopped");
    Ok(())
}

/// What the server says about its own state (§13.1): the change tag of every task
/// collection, by slug. Two equal answers mean that no client wrote to a task list in
/// between; a server that reports no tags always answers the same, and its changes are
/// left to the poll.
pub fn server_tags(collections: &[CollectionInfo]) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = collections
        .iter()
        .filter(|collection| collection.supports_vtodo)
        .filter_map(|collection| Some((collection.slug.clone(), collection.ctag.clone()?)))
        .collect();
    tags.sort();
    tags
}

/// Spawns the server watch (§13.1): every `every`, one `PROPFIND` for the collections'
/// change tags; an answer that differs from the last one (`seen`, `None` while no look
/// has succeeded) wakes the reconciler like a file event does. It only ever starts a
/// pass — what changed is the pass's to find out — and it keeps no state the pass
/// relies on. The daemon's own pushes change the tags too: the pass that follows them
/// finds nothing to do.
fn spawn_server_watch<C: CaldavPort>(
    server: C,
    every: Duration,
    mut seen: Option<Vec<(String, String)>>,
    tx: mpsc::UnboundedSender<Wake>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(Instant::now() + every, every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            // A look that fails says nothing about the server's tasks; a server that is
            // down is reported by the poll's pass.
            let Ok(collections) = server.list_collections().await else {
                continue;
            };
            let tags = server_tags(&collections);
            if seen.as_ref() == Some(&tags) {
                continue;
            }
            seen = Some(tags);
            tracing::info!("server_changed");
            if tx.send(Wake::Server).is_err() {
                return;
            }
        }
    })
}

/// Until when the pass that is due waits (§13.1), `None` when it runs now: a note
/// changed less than `settle` ago (`changed`) and the vault still has local work
/// waiting ([`Engine::unsettled`]). `held` is since when passes have been waiting; after
/// [`SETTLE_LIMIT`] times `settle` the pass runs whatever the vault looks like. A vault
/// that cannot be looked at is the pass's to report.
async fn settling<C: CaldavPort>(
    engine: &Engine<C>,
    changed: Option<Instant>,
    settle: Duration,
    held: &mut Option<Instant>,
) -> Option<Instant> {
    let now = Instant::now();
    let until = changed.map(|at| at + settle).filter(|until| *until > now);
    let limit = held.map(|since| since + settle * SETTLE_LIMIT);
    let wait = match until {
        Some(until) if limit.is_none_or(|limit| limit > now) => {
            engine.unsettled().await.unwrap_or(false).then_some(until)
        }
        _ => None,
    };
    match wait {
        Some(until) => {
            if held.is_none() {
                tracing::info!("vault_settling");
                *held = Some(now);
            }
            Some(limit.map_or(until, |limit| until.min(limit)))
        }
        None => {
            *held = None;
            None
        }
    }
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
    tx: mpsc::UnboundedSender<Wake>,
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
                let _ = tx.send(Wake::Vault);
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

/// Gives `engine` the vault config as it is on disk now, when it changed since the
/// engine got its own (§13.1): `restask.toml` rides the file sync, so the calendars
/// TODO.md shows can be changed on any device and reach the daemon as a file. A config
/// that cannot be read or parsed — half delivered, or mistyped — changes nothing: the
/// pass runs with the one the engine has. `track` and `ignore` still take a restart to
/// change which file events wake the daemon; the scan follows them at once.
fn refresh_config<C: CaldavPort>(vault: &Path, engine: &mut Engine<C>) {
    match load_vault_config(vault) {
        Ok(cfg) if cfg != *engine.config() => {
            tracing::info!("config_reloaded");
            engine.set_config(cfg);
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "restask.toml not usable; keeping the last one"),
    }
}

/// Lets `engine` add the calendars that appeared on the server to the ones TODO.md
/// shows (§7.5), before the pass that then brings their tasks in. A look that fails
/// adds nothing and says nothing: the pass reports a server that is down.
async fn follow_calendars<C: CaldavPort>(engine: &mut Engine<C>) {
    if let Err(error) = engine.follow_calendars().await {
        tracing::debug!(%error, "calendars not looked at");
    }
}

/// Loads `<vault>/restask.toml`, mapping config failures to [`RestaskError::Config`].
pub(crate) fn load_vault_config(vault: &Path) -> Result<VaultConfig, RestaskError> {
    let path = vault.join("restask.toml");
    VaultConfig::load(&path).map_err(|error| RestaskError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}
