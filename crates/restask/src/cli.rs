//! CLI surface (§13.3): clap definition, vault resolution, and command dispatch. A thin
//! adapter: every vault or server mutation routes through [`crate::sync::Engine`]; the
//! read-only commands share the engine's scan ([`crate::vault::scan`]) in read-only mode.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::SecondsFormat;
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::caldav::{CaldavClient, CaldavPort, Offline};
use crate::config::{machine_config_path, ConfigError, MachineConfig, VaultConfig};
use crate::daemon::{self, DaemonConfig};
use crate::domain::{Clock, Priority, Recurrence, Status, SystemClock, TaskUid, When};
use crate::markdown::{is_view, parse};
use crate::setup;
use crate::store::{cache_read, Index};
use crate::sync::merge::fields_differ;
use crate::sync::Engine;
use crate::vault::{self, Scan, ScanMode, STATE_DIR};
use crate::{CaldavErrorKind, RestaskError};

/// `RESTASK_VAULT` (§14.3; ARCHITECTURE naming map).
const ENV_VAULT: &str = "RESTASK_VAULT";

/// The `restask` command line (§13.3).
#[derive(Debug, Parser)]
#[command(name = "restask", version)]
pub struct Cli {
    /// Vault directory. Defaults to `$RESTASK_VAULT`, then an upward search from the
    /// working directory for `restask.toml` or `.restask/` (§13.3 resolution order).
    #[arg(long, global = true)]
    pub vault: Option<PathBuf>,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands (§13.3 table).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// First-time setup wizard (§13.2).
    Setup {
        /// CalDAV base URL (required with `--non-interactive`).
        #[arg(long)]
        url: Option<String>,
        /// CalDAV username (required with `--non-interactive`).
        #[arg(long)]
        username: Option<String>,
        /// Environment variable holding the password (with `--non-interactive`: this or
        /// `--password-stdin`).
        #[arg(long)]
        password_env: Option<String>,
        /// With `--non-interactive`: read the password from standard input (one line)
        /// and store it in this machine's password file, where its daemon finds it.
        #[arg(long, conflicts_with = "password_env", requires = "non_interactive")]
        password_stdin: bool,
        /// Bind a list to a collection as `list=collection` (repeatable).
        #[arg(long = "collection")]
        collections: Vec<String>,
        /// Fail instead of prompting.
        #[arg(long)]
        non_interactive: bool,
        /// Connect this machine to a vault that is already set up (it arrived through the
        /// file sync): credentials, first sync and the daemon; the vault is left as it is.
        #[arg(long, conflicts_with = "collections")]
        join: bool,
        /// Install the daemon on this always-on server instead of this computer, over
        /// ssh (`ssh <NODE>` must work; the server needs Docker and a copy of the vault
        /// from the file sync). The interactive wizard asks for it when no flag says.
        #[arg(long, value_name = "SSH_HOST")]
        node: Option<String>,
        /// The vault's folder on the `--node` machine.
        #[arg(long, value_name = "PATH", requires = "node")]
        node_vault: Option<String>,
        /// Where the daemon's files go on the `--node` machine (default: `restask` in
        /// the ssh user's home directory).
        #[arg(long, value_name = "DIR", requires = "node")]
        node_dir: Option<String>,
        /// Do not install the daemon at all: it is installed by hand on another, always-on
        /// machine (one sync node per vault). This machine keeps no credentials.
        #[arg(long, conflicts_with = "node")]
        no_daemon: bool,
    },
    /// Run the single-writer reconciler until shutdown (§13.1).
    Daemon {
        /// Reconcile once and exit.
        #[arg(long)]
        once: bool,
    },
    /// Perform one full reconcile.
    Sync,
    /// Append a task to the TODO.md inbox, register it, and push it.
    Add {
        /// Task text.
        text: String,
        /// Priority name (`highest`, `high`, `medium`, `low`, `lowest`).
        #[arg(long)]
        priority: Option<String>,
        /// Due date or date-time (`YYYY-MM-DD[ HH:MM]`).
        #[arg(long)]
        due: Option<String>,
        /// Repeat rule in the vault spelling, e.g. `"every week on Monday"`.
        #[arg(long)]
        repeat: Option<String>,
    },
    /// Complete a task and move it to the done region.
    Done {
        /// How to address the task.
        #[command(flatten)]
        selector: Selector,
    },
    /// Reopen a completed task.
    Undone {
        /// How to address the task.
        #[command(flatten)]
        selector: Selector,
    },
    /// Do the local work for the vault as it is now — register, repair, file, refresh
    /// TODO.md — without contacting the server. Editor integrations run it on save.
    Settle {
        /// A note of the vault (absolute path): the vault is found from it.
        #[arg(long)]
        file: Option<String>,
    },
    /// Print vault and sync-state counts.
    Status {
        /// Emit machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Drop the sync state under `.restask/` (tombstones are kept); the next sync
    /// re-derives it from the vault and the server. Neither of them is touched.
    Rebuild,
    /// Diagnose config, routing, vault, and server health (§13.3).
    Doctor {},
    /// Show the lists the vault routes to, with their notes and collection URLs.
    Lists,
}

/// Task selector for `done`/`undone` (§13.3): `--uid`, or `--file` together with `--line`.
#[derive(Debug, clap::Args)]
pub struct Selector {
    /// Eternal task UID (exclusive with `--file`/`--line`).
    #[arg(
        long,
        required_unless_present_any = ["file", "line"],
        conflicts_with_all = ["file", "line"]
    )]
    pub uid: Option<String>,

    /// Vault-relative source file; requires `--line`.
    #[arg(long, requires = "line")]
    pub file: Option<String>,

    /// 1-based line number within `--file`; requires `--file`.
    #[arg(long, requires = "file")]
    pub line: Option<usize>,
}

/// Vault and sync-state summary (§13.3 `restask status`).
#[derive(Debug, Serialize)]
pub struct StatusReport {
    /// Active tasks per list slug.
    pub lists: BTreeMap<String, usize>,
    /// Active tasks per priority CLI name; tasks without a priority are omitted.
    pub priorities: BTreeMap<String, usize>,
    /// Tasks completed today (device-local date).
    pub done_today: usize,
    /// Tasks whose current vault content the server has not confirmed yet (new, edited,
    /// or waiting for a reachable server).
    pub pending: usize,
    /// RFC 3339 instant of the most recent change settled with the server.
    pub last_sync: Option<String>,
}

/// Environment entry point (§13.3): resolves the vault and machine config, builds the
/// CalDAV client for server-bound commands, and dispatches. Offline commands
/// ([`Command::Settle`], [`Command::Status`], [`Command::Rebuild`], [`Command::Lists`])
/// never construct a server client; `add`/`done`/`undone` work on a machine without one
/// (vault-side only).
pub async fn execute(cli: Cli) -> Result<i32, RestaskError> {
    let Cli { vault, command } = cli;
    // `setup` carries its own vault fallback (§13.2 step 1): cwd, confirmed or
    // TODO.md-marked — a fresh vault has no markers for the strict search to find.
    let vault = match &command {
        Command::Setup {
            non_interactive, ..
        } => resolve_setup_vault(vault.as_deref(), *non_interactive)?,
        // `done/undone --file <absolute path>` (editor integrations) locate the vault
        // from the file itself, wherever the editor's working directory is.
        Command::Done { selector } | Command::Undone { selector } => {
            resolve_vault_for_file(vault.as_deref(), selector.file.as_deref())?
        }
        Command::Settle { file } => resolve_vault_for_file(vault.as_deref(), file.as_deref())?,
        _ => resolve_vault(vault.as_deref())?,
    };
    let config_path = machine_config_path();
    let machine = load_machine(&config_path)?;
    match command {
        Command::Setup {
            non_interactive,
            join,
            no_daemon,
            node,
            node_vault,
            node_dir,
            url,
            username,
            password_env,
            password_stdin,
            collections,
        } => {
            let join = setup::joins(&vault, &config_path, join)?;
            let daemon = setup::DaemonFlags {
                no_daemon,
                node,
                node_vault,
                node_dir,
            };
            if !non_interactive {
                setup::run_interactive(
                    vault,
                    config_path,
                    join,
                    daemon,
                    Arc::new(SystemClock),
                    Some(&setup::SystemInstaller::default()),
                    Some(&setup::SystemObsidian),
                )
                .await?;
                return Ok(0);
            }
            let daemon = daemon.resolve()?;
            let password = if password_stdin {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                line.trim_end_matches(['\r', '\n']).to_string()
            } else {
                let name = password_env.as_deref().ok_or(RestaskError::Validation {
                    field: "password-env",
                    reason: "--password-env or --password-stdin is required with \
                             --non-interactive"
                        .to_string(),
                })?;
                std::env::var(name).unwrap_or_default()
            };
            if password.is_empty() {
                return Err(RestaskError::Validation {
                    field: "password",
                    reason: match &password_env {
                        Some(name) => format!("${name} is not set"),
                        None => "no password on standard input".to_string(),
                    },
                });
            }
            // The typed password is stored where this machine's daemon reads it; a
            // machine that only edits stores none.
            let password_file = if password_stdin && daemon == setup::DaemonHost::Here {
                Some(setup::store_password(&config_path, &password)?)
            } else {
                None
            };
            let args = setup::SetupArgs {
                daemon,
                password_file,
                password: Some(setup::Secret::new(password.clone())),
                ..setup::SetupArgs::from_flags(
                    vault,
                    config_path,
                    url,
                    username,
                    password_env,
                    setup::parse_collections(&collections)?,
                    join,
                )?
            };
            let caldav = CaldavClient::new(&args.url, args.username.clone(), Some(password))?;
            let mut summary = setup::run_setup(
                args,
                caldav,
                Arc::new(SystemClock),
                Some(&setup::SystemInstaller::default()),
            )
            .await?;
            // Unattended: Obsidian is told when it can be, and never restarted.
            summary.load_plugin(&setup::SystemObsidian, None);
            setup::print_summary(&summary);
            Ok(0)
        }
        Command::Doctor {} => {
            // Diagnostics must run even when the machine config is unusable, so the
            // client is best-effort here instead of the catch-all below.
            let caldav = server_client(&machine).ok();
            let report = doctor(
                &vault,
                &machine,
                caldav,
                &config_path,
                Arc::new(SystemClock),
            )
            .await?;
            print_doctor(&report);
            Ok(report.exit_code)
        }
        Command::Status { json } => print_status(&vault, Arc::new(SystemClock), json),
        Command::Rebuild => run_rebuild(&vault),
        Command::Lists => print_lists(&vault, &machine, Arc::new(SystemClock)),
        Command::Settle { .. } => settle(&vault, machine, Arc::new(SystemClock)).await,
        Command::Daemon { once } => {
            let caldav = server_client(&machine)?;
            let dc = DaemonConfig {
                poll_secs: machine.caldav.poll_secs,
                watch_ms: machine.caldav.watch_secs.saturating_mul(1_000),
                once,
                ..DaemonConfig::default()
            };
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            tokio::spawn(async move {
                wait_for_shutdown_signal().await;
                let _ = shutdown_tx.send(true);
            });
            daemon::run_with(
                vault,
                machine,
                dc,
                shutdown_rx,
                caldav,
                Arc::new(SystemClock),
            )
            .await?;
            Ok(0)
        }
        local @ (Command::Add { .. } | Command::Done { .. } | Command::Undone { .. }) => {
            // The vault part of these commands needs no server; without a configured
            // endpoint they still save locally and say so. A machine that leaves the
            // syncing to the sync node builds no client at all.
            match server_client(&machine) {
                Ok(caldav) => {
                    run_with(
                        local,
                        vault,
                        machine,
                        config_path,
                        caldav,
                        Arc::new(SystemClock),
                    )
                    .await
                }
                Err(_) => {
                    run_with(
                        local,
                        vault,
                        machine,
                        config_path,
                        Offline,
                        Arc::new(SystemClock),
                    )
                    .await
                }
            }
        }
        server => {
            let caldav = server_client(&machine)?;
            run_with(
                server,
                vault,
                machine,
                config_path,
                caldav,
                Arc::new(SystemClock),
            )
            .await
        }
    }
}

/// The CalDAV client of a command that is server work (`sync`, `daemon`). A machine that
/// leaves the syncing to the vault's sync node (`[node]`, §14.2) is refused one: a pass
/// from a second machine races the file sync (§1.1).
fn server_client(machine: &MachineConfig) -> Result<CaldavClient, RestaskError> {
    match &machine.node {
        Some(node) => Err(RestaskError::Config {
            path: machine_config_path().display().to_string(),
            reason: format!(
                "this machine only edits the vault: {} syncs it with the server. \
                 `restask settle` does the local work here",
                match &node.host {
                    Some(host) => format!("the daemon on {host}"),
                    None => "the daemon on the vault's sync node".to_string(),
                }
            ),
        }),
        None => daemon::build_client(machine),
    }
}

/// Resolves when the process is asked to stop: `SIGINT` (Ctrl-C) or, on Unix, `SIGTERM`
/// (what `systemctl stop` sends).
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Dispatches one command with an injected CalDAV port and clock (hermetic tests, §3).
/// Server-bound commands use `caldav`; offline commands ignore it.
pub async fn run_with<C: CaldavPort>(
    command: Command,
    vault: PathBuf,
    machine: MachineConfig,
    config_path: PathBuf,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<i32, RestaskError> {
    match command {
        Command::Setup {
            non_interactive,
            join,
            no_daemon,
            node,
            node_vault,
            node_dir,
            url,
            username,
            password_env,
            password_stdin: _,
            collections,
        } => {
            let join = setup::joins(&vault, &config_path, join)?;
            let daemon = setup::DaemonFlags {
                no_daemon,
                node,
                node_vault,
                node_dir,
            };
            if !non_interactive {
                // Hermetic dispatch: never touch the host's systemd session, a node or
                // the user's Obsidian.
                setup::run_interactive(vault, config_path, join, daemon, clock, None, None).await?;
                return Ok(0);
            }
            let args = setup::SetupArgs {
                daemon: daemon.resolve()?,
                ..setup::SetupArgs::from_flags(
                    vault,
                    config_path,
                    url,
                    username,
                    password_env,
                    setup::parse_collections(&collections)?,
                    join,
                )?
            };
            // Hermetic dispatch: never touch the host's systemd session or a node.
            let summary = setup::run_setup(args, caldav, clock, None).await?;
            setup::print_summary(&summary);
            Ok(0)
        }
        Command::Daemon { once } => {
            let dc = DaemonConfig {
                poll_secs: machine.caldav.poll_secs,
                watch_ms: machine.caldav.watch_secs.saturating_mul(1_000),
                once,
                ..DaemonConfig::default()
            };
            let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            daemon::run_with(vault, machine, dc, shutdown_rx, caldav, clock).await?;
            Ok(0)
        }
        Command::Sync => {
            let report = daemon::run_once_with(&vault, &machine, clock, caldav).await?;
            println!(
                "scanned {} registered {} normalized {} pushed {} moved {} deleted {} mutations {} inserts {} adopted {} deferred {} failed {}",
                report.scanned_files,
                report.registered,
                report.normalized,
                report.pushes,
                report.moves,
                report.deletes,
                report.markdown_mutations,
                report.inserts,
                report.adoptions,
                report.deferred,
                report.failed
            );
            Ok(0)
        }
        Command::Add {
            text,
            priority,
            due,
            repeat,
        } => {
            let priority = parse_priority(priority)?;
            let due = parse_due(due)?;
            let repeat = parse_repeat(repeat)?;
            let cfg = load_vault_config(&vault)?;
            let engine = Engine::new(&vault, cfg, machine, caldav, clock);
            let task = engine.add(&text, priority, due, repeat).await?;
            println!("{}", task.uid);
            Ok(0)
        }
        Command::Done { selector } => {
            set_done(&vault, machine, caldav, clock, selector, true).await
        }
        Command::Undone { selector } => {
            set_done(&vault, machine, caldav, clock, selector, false).await
        }
        Command::Doctor {} => {
            let report = doctor(&vault, &machine, Some(caldav), &config_path, clock).await?;
            print_doctor(&report);
            Ok(report.exit_code)
        }
        Command::Settle { .. } => settle(&vault, machine, clock).await,
        Command::Status { json } => print_status(&vault, clock, json),
        Command::Rebuild => run_rebuild(&vault),
        Command::Lists => print_lists(&vault, &machine, clock),
    }
}

/// Resolves the vault directory (§13.3): `flag` → `$RESTASK_VAULT` → upward search from
/// `start` for `restask.toml` or `.restask/`. A miss is a config error (exit 4).
pub fn resolve_vault_with(
    flag: Option<&Path>,
    env: Option<&str>,
    start: &Path,
) -> Result<PathBuf, RestaskError> {
    if let Some(flag) = flag {
        return Ok(flag.to_path_buf());
    }
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("restask.toml").is_file() || dir.join(".restask").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(RestaskError::Config {
        path: "<vault>".to_string(),
        reason: "no vault found: pass --vault, set RESTASK_VAULT, or run inside a vault \
                 (restask.toml or .restask/)"
            .to_string(),
    })
}

/// [`resolve_vault`] with the setup-only fallback (§13.2 step 1): when the strict chain
/// (flag → env → upward search) misses, the start directory is accepted — confirmed
/// interactively by [`crate::setup::run_interactive`]. `--non-interactive` cannot
/// confirm, so it additionally requires a `TODO.md` in the start directory.
pub fn resolve_setup_vault_with(
    flag: Option<&Path>,
    env: Option<&str>,
    start: &Path,
    non_interactive: bool,
) -> Result<PathBuf, RestaskError> {
    if let Ok(vault) = resolve_vault_with(flag, env, start) {
        return Ok(vault);
    }
    if non_interactive && !start.join("TODO.md").is_file() {
        return Err(RestaskError::Config {
            path: "<vault>".to_string(),
            reason: "no vault found: --non-interactive setup needs --vault, RESTASK_VAULT, \
                     an existing vault marker, or a directory containing TODO.md"
                .to_string(),
        });
    }
    Ok(start.to_path_buf())
}

/// [`resolve_setup_vault_with`] against the process environment and working directory.
pub fn resolve_setup_vault(
    flag: Option<&Path>,
    non_interactive: bool,
) -> Result<PathBuf, RestaskError> {
    let env = std::env::var(ENV_VAULT).ok();
    let cwd = std::env::current_dir()?;
    resolve_setup_vault_with(flag, env.as_deref(), &cwd, non_interactive)
}

/// [`resolve_vault_with`] against the process environment and working directory.
pub fn resolve_vault(flag: Option<&Path>) -> Result<PathBuf, RestaskError> {
    let env = std::env::var(ENV_VAULT).ok();
    let cwd = std::env::current_dir()?;
    resolve_vault_with(flag, env.as_deref(), &cwd)
}

/// [`resolve_vault`] for commands addressing a task by file: when `file` is absolute the
/// upward search starts at the file's own directory instead of the working directory.
pub fn resolve_vault_for_file(
    flag: Option<&Path>,
    file: Option<&str>,
) -> Result<PathBuf, RestaskError> {
    let env = std::env::var(ENV_VAULT).ok();
    match file.map(Path::new).filter(|file| file.is_absolute()) {
        Some(file) => {
            let start = file.parent().unwrap_or(file);
            resolve_vault_with(flag, env.as_deref(), start)
        }
        None => resolve_vault(flag),
    }
}

/// Process exit code for `error` (§12.1): 4 config invalid (incl. no vault found), 3
/// CalDAV unreachable, 1 any other runtime failure; clap reports usage errors as 2.
pub fn exit_code(error: &RestaskError) -> i32 {
    match error {
        RestaskError::Config { .. } => 4,
        RestaskError::Caldav {
            kind: CaldavErrorKind::Network | CaldavErrorKind::Tls,
            ..
        } => 3,
        _ => 1,
    }
}

/// Computes the [`StatusReport`] for `vault` (§13.3): a read-only vault scan plus the
/// index under `.restask/`.
pub fn status_report(vault: &Path, clock: &dyn Clock) -> Result<StatusReport, RestaskError> {
    let cfg = load_vault_config(vault)?;
    let index = Index::load(&vault.join(STATE_DIR))?;
    let tasks = scan_read_only(vault, &cfg, clock, &index)?.local;
    let mut lists: BTreeMap<String, usize> = BTreeMap::new();
    let mut priorities: BTreeMap<String, usize> = BTreeMap::new();
    let mut done_today = 0usize;
    let mut pending = 0usize;
    let today = clock.today_local();
    let state_dir = vault.join(STATE_DIR);
    for task in tasks.values() {
        // Settled = the server confirmed exactly what the vault line says now.
        let settled = index
            .get(&task.uid)
            .is_some_and(|entry| entry.caldav_etag.is_some())
            && cache_read(&state_dir, &task.uid, &chrono::Utc)
                .is_some_and(|base| !fields_differ(&base, task));
        if !settled {
            pending += 1;
        }
        match task.status {
            Status::Active => {
                *lists.entry(task.list.as_str().to_string()).or_default() += 1;
                if let Some(priority) = task.priority {
                    *priorities
                        .entry(priority.cli_name().to_string())
                        .or_default() += 1;
                }
            }
            Status::Completed { on } if on == today => done_today += 1,
            Status::Completed { .. } => {}
        }
    }
    let last_sync = index
        .entries
        .values()
        .map(|entry| entry.seen_at)
        .max()
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true));
    Ok(StatusReport {
        lists,
        priorities,
        done_today,
        pending,
        last_sync,
    })
}

/// The read-only vault scan shared by `status`, `doctor` and `lists`.
fn scan_read_only(
    vault: &Path,
    cfg: &VaultConfig,
    clock: &dyn Clock,
    index: &Index,
) -> Result<Scan, RestaskError> {
    vault::scan(vault, cfg, clock, index, ScanMode::ReadOnly)
}

/// Check outcome severity (§13.3 `restask doctor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorStatus {
    /// Healthy.
    Ok,
    /// Worth fixing, not fatal.
    Warn,
    /// Broken.
    Fail,
}

impl DoctorStatus {
    /// Bracket tag used in the one-shot output (§12.2).
    pub fn tag(self) -> &'static str {
        match self {
            DoctorStatus::Ok => "ok",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "fail",
        }
    }
}

/// One `restask doctor` check result (§13.3).
#[derive(Debug)]
pub struct DoctorCheck {
    /// Check identifier (e.g. `routing`, `caldav`).
    pub name: &'static str,
    /// Outcome.
    pub status: DoctorStatus,
    /// Human-readable detail.
    pub detail: String,
}

/// Doctor report (§13.3): every check plus the §12.1 exit code (4 config invalid, 3
/// CalDAV unreachable, 1 broken check or §17 no-auth hard warning, 0 healthy).
#[derive(Debug)]
pub struct DoctorReport {
    /// Checks in execution order.
    pub checks: Vec<DoctorCheck>,
    /// Process exit code derived from the checks.
    pub exit_code: i32,
}

/// Diagnoses vault and server health (§13.3): machine/vault config, routing (incl.
/// `ListConflict`), vault scan, TODO marker, sync-conflict files, CalDAV reachability,
/// and the §17 no-auth hard warning (config-layer when no password source is set, plus
/// an active wrong-password probe against the real endpoint). `caldav` is `None` when no
/// endpoint is configured. Check failures land in the report with the derived exit code,
/// never as `Err`.
pub async fn doctor<C: CaldavPort>(
    vault: &Path,
    machine: &MachineConfig,
    caldav: Option<C>,
    config_path: &Path,
    clock: Arc<dyn Clock>,
) -> Result<DoctorReport, RestaskError> {
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut exit_code = 0i32;

    if config_path.is_file() {
        checks.push(check(
            "machine-config",
            DoctorStatus::Ok,
            config_path.display(),
        ));
    } else {
        checks.push(check(
            "machine-config",
            DoctorStatus::Warn,
            "no machine config yet (restask setup)",
        ));
    }

    // Vault config: a failure dominates (exit 4, §12.1).
    let cfg = match load_vault_config(vault) {
        Ok(cfg) => {
            checks.push(check(
                "vault-config",
                DoctorStatus::Ok,
                format!("restask.toml loaded (inbox {})", cfg.inbox_file),
            ));
            cfg
        }
        Err(error) => {
            checks.push(check("vault-config", DoctorStatus::Fail, error));
            return Ok(DoctorReport {
                checks,
                exit_code: 4,
            });
        }
    };

    // Routing and vault scan over one read-only pass.
    let index = Index::load(&vault.join(STATE_DIR)).unwrap_or_default();
    match scan_read_only(vault, &cfg, clock.as_ref(), &index) {
        Ok(scan) => {
            checks.push(check("routing", DoctorStatus::Ok, "no list conflicts"));
            checks.push(check(
                "scan",
                DoctorStatus::Ok,
                format!(
                    "{} routed task(s) in {} file(s), {} list(s)",
                    scan.local.len(),
                    scan.files_scanned,
                    scan.homes.len()
                ),
            ));
            if !scan.duplicates.is_empty() {
                checks.push(check(
                    "duplicates",
                    DoctorStatus::Warn,
                    format!(
                        "{} copied task line(s) share a UID (first in {}) — the next sync \
                         gives each copy its own",
                        scan.duplicates.len(),
                        scan.duplicates[0].1
                    ),
                ));
            }
            if scan.conflict_files.is_empty() {
                checks.push(check("sync-conflict", DoctorStatus::Ok, "none"));
            } else {
                checks.push(check(
                    "sync-conflict",
                    DoctorStatus::Warn,
                    format!(
                        "{} file(s), e.g. {} — merge what you need and delete them",
                        scan.conflict_files.len(),
                        scan.conflict_files[0]
                    ),
                ));
            }
        }
        Err(RestaskError::ListConflict { dir, a, b }) => {
            checks.push(check(
                "routing",
                DoctorStatus::Fail,
                format!("{dir}: {a} vs {b}"),
            ));
            exit_code = 1;
        }
        Err(RestaskError::Config { path, reason }) => {
            checks.push(check(
                "vault-config",
                DoctorStatus::Fail,
                format!("{path}: {reason}"),
            ));
            return Ok(DoctorReport {
                checks,
                exit_code: 4,
            });
        }
        Err(error) => {
            checks.push(check("scan", DoctorStatus::Fail, error));
            exit_code = 1;
        }
    }

    let inbox = vault.join(&cfg.inbox_file);
    match std::fs::read_to_string(&inbox) {
        Ok(contents) if is_view(&contents) => checks.push(check(
            "todo-view",
            DoctorStatus::Ok,
            format!("{} is a restask view", cfg.inbox_file),
        )),
        Ok(_) => checks.push(check(
            "todo-view",
            DoctorStatus::Warn,
            format!(
                "{} was not rendered by restask (the next sync rewrites it)",
                cfg.inbox_file
            ),
        )),
        Err(_) => checks.push(check(
            "todo-view",
            DoctorStatus::Warn,
            format!("{} is missing", cfg.inbox_file),
        )),
    }

    // CalDAV reachability and the §17 no-auth hard warning.
    match caldav {
        Some(client) => match client.list_collections().await {
            Ok(collections) => {
                checks.push(check(
                    "caldav",
                    DoctorStatus::Ok,
                    format!("reachable ({} collection(s))", collections.len()),
                ));
                // The §17 auth layers only apply when an endpoint is configured.
                if machine.caldav.url.is_some() {
                    match machine.resolved_password() {
                        Ok(Some(_)) => {
                            if let Some(username) = &machine.caldav.username {
                                // §17 finding: a wrong password must be rejected; a
                                // success means auth is disabled server-side. One attempt.
                                let probe = CaldavClient::with_retry_delays(
                                    machine.caldav.url.as_deref().unwrap_or_default(),
                                    username.clone(),
                                    Some("restask-doctor-invalid-password".to_string()),
                                    Vec::new(),
                                );
                                match probe {
                                    Ok(probe) => match probe.list_collections().await {
                                        Ok(_) => {
                                            checks.push(check(
                                                "caldav-auth",
                                                DoctorStatus::Warn,
                                                "the server accepts any credentials — auth is \
                                                 disabled (§17)",
                                            ));
                                            if exit_code == 0 {
                                                exit_code = 1;
                                            }
                                        }
                                        Err(RestaskError::Caldav {
                                            kind: CaldavErrorKind::Auth,
                                            ..
                                        }) => checks.push(check(
                                            "caldav-auth",
                                            DoctorStatus::Ok,
                                            "server rejects wrong credentials",
                                        )),
                                        Err(error) => checks.push(check(
                                            "caldav-auth",
                                            DoctorStatus::Warn,
                                            format!("could not verify auth: {error}"),
                                        )),
                                    },
                                    Err(error) => checks.push(check(
                                        "caldav-auth",
                                        DoctorStatus::Warn,
                                        format!("could not build the auth probe: {error}"),
                                    )),
                                }
                            }
                        }
                        Ok(None) => {
                            checks.push(check(
                                "caldav-auth",
                                DoctorStatus::Warn,
                                "no password configured — requests are sent unauthenticated (§17)",
                            ));
                            if exit_code == 0 {
                                exit_code = 1;
                            }
                        }
                        Err(error) => checks.push(check(
                            "caldav-auth",
                            DoctorStatus::Warn,
                            format!("password source unreadable: {error}"),
                        )),
                    }
                }
            }
            Err(RestaskError::Caldav {
                kind: CaldavErrorKind::Network | CaldavErrorKind::Tls,
                detail,
                ..
            }) => {
                checks.push(check(
                    "caldav",
                    DoctorStatus::Fail,
                    format!("unreachable: {detail}"),
                ));
                exit_code = 3;
            }
            Err(error) => {
                checks.push(check("caldav", DoctorStatus::Fail, error));
                exit_code = 1;
            }
        },
        None => match &machine.node {
            Some(node) => checks.push(check(
                "caldav",
                DoctorStatus::Ok,
                match (&node.host, &node.dir) {
                    (Some(host), Some(dir)) => format!(
                        "left to the daemon on {host} (its log: ssh {host} 'cd {dir} && \
                         docker compose logs --tail 20')"
                    ),
                    _ => "left to the daemon on the vault's sync node".to_string(),
                },
            )),
            None => checks.push(check(
                "caldav",
                DoctorStatus::Warn,
                "not configured (restask setup)",
            )),
        },
    }

    Ok(DoctorReport { checks, exit_code })
}

/// Prints the doctor report to stdout (§12.2 one-shot format).
fn print_doctor(report: &DoctorReport) {
    for check in &report.checks {
        println!("[{}] {}: {}", check.status.tag(), check.name, check.detail);
    }
}

/// Builds a [`DoctorCheck`] from a displayable detail.
fn check(name: &'static str, status: DoctorStatus, detail: impl std::fmt::Display) -> DoctorCheck {
    DoctorCheck {
        name,
        status,
        detail: detail.to_string(),
    }
}

/// Runs [`Engine::settle`] (§13.3 `restask settle`). The engine is built over the
/// [`Offline`] port whatever the machine has configured: local work never reaches for
/// the server.
async fn settle(
    vault: &Path,
    machine: MachineConfig,
    clock: Arc<dyn Clock>,
) -> Result<i32, RestaskError> {
    let cfg = load_vault_config(vault)?;
    let report = Engine::new(vault, cfg, machine, Offline, clock)
        .settle()
        .await?;
    println!(
        "scanned {} registered {} normalized {}",
        report.scanned_files, report.registered, report.normalized
    );
    Ok(0)
}

/// Completes/reopens via [`Engine::set_done`] after resolving the selector (§13.3).
async fn set_done<C: CaldavPort>(
    vault: &Path,
    machine: MachineConfig,
    caldav: C,
    clock: Arc<dyn Clock>,
    selector: Selector,
    done: bool,
) -> Result<i32, RestaskError> {
    let cfg = load_vault_config(vault)?;
    let uid = resolve_selector(vault, &cfg, &selector)?;
    let engine = Engine::new(vault, cfg, machine, caldav, clock);
    engine.set_done(&uid, done).await?;
    println!("{uid}");
    Ok(0)
}

/// Resolves the task UID from a [`Selector`] (§13.3): `--uid` directly, else the file is
/// parsed and the registered task at the 1-based `--line` wins.
fn resolve_selector(
    vault: &Path,
    cfg: &VaultConfig,
    selector: &Selector,
) -> Result<TaskUid, RestaskError> {
    if let Some(raw) = &selector.uid {
        return TaskUid::parse(raw).map_err(|error| RestaskError::Validation {
            field: "uid",
            reason: error.0,
        });
    }
    let (file, line) = match (&selector.file, selector.line) {
        (Some(file), Some(line)) => (file, line),
        _ => {
            return Err(RestaskError::Validation {
                field: "selector",
                reason: "use --uid or both --file and --line".to_string(),
            })
        }
    };
    // `vault.join` keeps an absolute `--file` as-is (editor integrations pass one).
    let contents = std::fs::read_to_string(vault.join(file))?;
    parse(&contents, cfg)
        .tasks
        .into_iter()
        .find(|task| task.line_no == line)
        .and_then(|task| task.draft.uid)
        .ok_or_else(|| RestaskError::Validation {
            field: "line",
            reason: format!("no registered task at {file}:{line}"),
        })
}

/// Parses `--priority` (§13.3): one of the five CLI names.
fn parse_priority(raw: Option<String>) -> Result<Option<Priority>, RestaskError> {
    raw.map(|name| {
        Priority::from_cli_name(&name).ok_or_else(|| RestaskError::Validation {
            field: "priority",
            reason: format!(
                "unknown priority `{name}` (expected highest, high, medium, low, lowest)"
            ),
        })
    })
    .transpose()
}

/// Parses `--due` (§13.3): `YYYY-MM-DD[ HH:MM]` (§4: a date-only value is midnight UTC).
fn parse_due(raw: Option<String>) -> Result<Option<When>, RestaskError> {
    raw.map(|text| {
        When::parse_date_or_datetime(&text).map_err(|error| RestaskError::Validation {
            field: "due",
            reason: error.to_string(),
        })
    })
    .transpose()
}

/// Parses `--repeat` (§13.3): a whole rule in the vault spelling (§3.6).
fn parse_repeat(raw: Option<String>) -> Result<Option<Recurrence>, RestaskError> {
    raw.map(|text| {
        let text = text.trim();
        match Recurrence::from_text(text) {
            Some((rule, len)) if len == text.len() => Ok(rule),
            _ => Err(RestaskError::Validation {
                field: "repeat",
                reason: format!(
                    "cannot read `{text}` as a repeat rule (e.g. `every day`, `every 2 weeks on \
                     Monday, Thursday`, `every month on the 15th`, `every year`)"
                ),
            }),
        }
    })
    .transpose()
}

/// Prints the [`StatusReport`] (human text or JSON) to stdout (§12.2 one-shot format).
fn print_status(vault: &Path, clock: Arc<dyn Clock>, json: bool) -> Result<i32, RestaskError> {
    let report = status_report(vault, clock.as_ref())?;
    if json {
        let rendered =
            serde_json::to_string(&report).map_err(|error| RestaskError::Validation {
                field: "status",
                reason: error.to_string(),
            })?;
        println!("{rendered}");
    } else {
        println!("active by list:");
        for (list, count) in &report.lists {
            println!("  {list}: {count}");
        }
        println!("active by priority:");
        for (priority, count) in &report.priorities {
            println!("  {priority}: {count}");
        }
        println!("done today: {}", report.done_today);
        println!("pending sync: {}", report.pending);
        println!(
            "last sync: {}",
            report.last_sync.as_deref().unwrap_or("never")
        );
    }
    Ok(0)
}

/// Drops the sync state (§13.3 `restask rebuild`): the index, the base snapshots and the
/// remembered TODO.md render. Tombstones stay, so deleted tasks are not resurrected. The
/// next sync starts from "nothing agreed yet": equal content settles silently, differing
/// content is merged as a conflict (the vault wins unless the server copy is newer).
fn run_rebuild(vault: &Path) -> Result<i32, RestaskError> {
    let state_dir = vault.join(STATE_DIR);
    let known = Index::load(&state_dir)
        .map(|index| index.entries.len())
        .unwrap_or(0);
    for file in [
        "index.json",
        crate::sync::engine::RENDERED_FILE,
        "outbox.json",
    ] {
        match std::fs::remove_file(state_dir.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    match std::fs::remove_dir_all(state_dir.join("tasks")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    println!("dropped the sync state of {known} task(s); run `restask sync` to re-derive it");
    Ok(0)
}

/// Prints the routed lists (§13.3 `restask lists`): slug, active tasks, home note, and
/// the collection URL to point other clients at.
fn print_lists(
    vault: &Path,
    machine: &MachineConfig,
    clock: Arc<dyn Clock>,
) -> Result<i32, RestaskError> {
    let cfg = load_vault_config(vault)?;
    let index = Index::load(&vault.join(STATE_DIR))?;
    let scan = scan_read_only(vault, &cfg, clock.as_ref(), &index)?;
    let inbox = vault::inbox_list(&cfg)?;
    let mut homes = scan.homes.clone();
    homes.insert(inbox, cfg.inbox_file.clone());
    // On a machine that only edits, the endpoint is the one its sync node talks to.
    let node = machine.node.as_ref();
    let endpoint = (
        machine
            .caldav
            .url
            .as_ref()
            .or(node.and_then(|node| node.url.as_ref())),
        machine
            .caldav
            .username
            .as_ref()
            .or(node.and_then(|node| node.username.as_ref())),
    );
    let base = match endpoint {
        (Some(url), Some(username)) => Some(format!("{}/{}", url.trim_end_matches('/'), username)),
        _ => None,
    };
    for (list, home) in &homes {
        let active = scan
            .local
            .values()
            .filter(|task| task.list == *list && task.status == Status::Active)
            .count();
        let url = match &base {
            Some(base) => format!("{base}/{}/", list.as_str()),
            None => "(no CalDAV endpoint configured)".to_string(),
        };
        println!("{}\t{active} active\t{home}\t{url}", list.as_str());
    }
    Ok(0)
}

/// Loads the machine config (§14.2) with §14.3 env overrides; a missing file yields the
/// default config (fresh machine).
fn load_machine(path: &Path) -> Result<MachineConfig, RestaskError> {
    match MachineConfig::load(path) {
        Ok(mut machine) => {
            machine.apply_env();
            Ok(machine)
        }
        Err(ConfigError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            let mut machine = MachineConfig::default();
            machine.apply_env();
            Ok(machine)
        }
        Err(error) => Err(RestaskError::Config {
            path: path.display().to_string(),
            reason: error.to_string(),
        }),
    }
}

/// Loads `<vault>/restask.toml`, mapping config failures to [`RestaskError::Config`].
fn load_vault_config(vault: &Path) -> Result<VaultConfig, RestaskError> {
    let path = vault.join("restask.toml");
    VaultConfig::load(&path).map_err(|error| RestaskError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}
