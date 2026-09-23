//! CLI surface (§13.3): clap definition, vault resolution, and command dispatch. A thin
//! adapter: every state mutation routes through [`crate::sync::Engine`], [`crate::daemon`],
//! or the store APIs; the read-only scan mirrors the engine's registration pass without
//! minting UIDs or writing files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::caldav::{CaldavClient, CaldavPort};
use crate::config::{
    machine_config_path, ConfigError, ListBinding, MachineConfig, VaultConfig, VaultMatchers,
};
use crate::daemon::{self, DaemonConfig};
use crate::domain::{
    Clock, ListSlug, Priority, SourceRef, Status, SystemClock, Task, TaskUid, When,
};
use crate::markdown::{parse, parser::link_parents, MARKER};
use crate::router::{scan_frontmatter, NoteMeta, NoteRouting, Router};
use crate::setup;
use crate::store::{cache_remove, cache_write, Index, IndexEntry};
use crate::sync::Engine;
use crate::{CaldavErrorKind, TaskresError};

/// `RESTASK_VAULT` (§14.3; ARCHITECTURE naming map).
const ENV_VAULT: &str = "RESTASK_VAULT";

/// The `restask` command line (§13.3).
#[derive(Debug, Parser)]
#[command(name = "restask", version)]
pub struct Cli {
    /// Vault directory. Defaults to `$RESTASK_VAULT`, then an upward search from the
    /// working directory for `restask.toml` or `.taskres/` (§13.3 resolution order).
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
        /// SSH alias for read-only server discovery (§13.2 step 3).
        #[arg(long)]
        server: Option<String>,
        /// Docker root used for server discovery.
        #[arg(long, default_value = "/opt/docker")]
        docker_root: String,
        /// CalDAV base URL (required with `--non-interactive`).
        #[arg(long)]
        url: Option<String>,
        /// CalDAV username (required with `--non-interactive`).
        #[arg(long)]
        username: Option<String>,
        /// Environment variable holding the password (required with `--non-interactive`).
        #[arg(long)]
        password_env: Option<String>,
        /// Bind a list to a collection as `list=collection` (repeatable).
        #[arg(long = "collection")]
        collections: Vec<String>,
        /// Fail instead of prompting.
        #[arg(long)]
        non_interactive: bool,
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
    /// Print vault and sync-state counts.
    Status {
        /// Emit machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Re-derive index and cache state from the vault; the server is never touched.
    Rebuild,
    /// Diagnose config, routing, vault, and server health (§13.3).
    Doctor {
        /// SSH alias for a read-only server-compose probe.
        #[arg(long)]
        server: Option<String>,
    },
    /// Manage list bindings (§13.2 step 5 records).
    List {
        /// The list action.
        #[command(subcommand)]
        action: ListAction,
    },
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

/// List-binding actions (§13.3 `restask list`).
#[derive(Debug, Subcommand)]
pub enum ListAction {
    /// Show the recorded bindings.
    Show,
    /// Create the collection for a list and record the binding.
    Create {
        /// List display name.
        name: String,
    },
    /// Record a binding to an existing collection.
    Bind {
        /// List display name.
        name: String,
        /// Radicale collection name.
        collection: String,
    },
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
    /// Operations parked in `.taskres/outbox.json`.
    pub outbox_backlog: usize,
    /// RFC 3339 instant of the most recent reconciliation known to the index.
    pub last_sync: Option<String>,
}

/// Environment entry point (§13.3): resolves the vault and machine config, builds the
/// CalDAV client for server-bound commands, and dispatches. Offline commands ([`Command::Status`],
/// [`Command::Rebuild`], `list show`/`list bind`) never construct a server client.
pub async fn execute(cli: Cli) -> Result<i32, TaskresError> {
    let Cli { vault, command } = cli;
    // `setup` carries its own vault fallback (§13.2 step 1): cwd, confirmed or
    // TODO.md-marked — a fresh vault has no markers for the strict search to find.
    let vault = match &command {
        Command::Setup {
            non_interactive, ..
        } => resolve_setup_vault(vault.as_deref(), *non_interactive)?,
        _ => resolve_vault(vault.as_deref())?,
    };
    let config_path = machine_config_path();
    let machine = load_machine(&config_path)?;
    match command {
        Command::Setup {
            non_interactive,
            server,
            docker_root,
            url,
            username,
            password_env,
            collections,
        } => {
            if !non_interactive {
                setup::run_interactive(
                    vault,
                    server,
                    &docker_root,
                    config_path,
                    Arc::new(SystemClock),
                )
                .await?;
                return Ok(0);
            }
            let args = setup::SetupArgs::from_flags(
                vault,
                config_path,
                url,
                username,
                password_env,
                setup::parse_collections(&collections)?,
            )?;
            let password_env = match &args.password_env {
                Some(name) => name.clone(),
                None => {
                    return Err(TaskresError::Validation {
                        field: "password-env",
                        reason: "--password-env is required with --non-interactive".to_string(),
                    })
                }
            };
            let password = std::env::var(&password_env)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| TaskresError::Validation {
                    field: "password-env",
                    reason: format!("${password_env} is not set"),
                })?;
            let caldav = CaldavClient::new(&args.url, args.username.clone(), Some(password))?;
            let summary = setup::run_setup(args, caldav, Arc::new(SystemClock)).await?;
            setup::print_summary(&summary);
            Ok(0)
        }
        Command::Doctor { server } => {
            // Diagnostics must run even when the machine config is unusable, so the
            // client is best-effort here instead of the catch-all below.
            let caldav = daemon::build_client(&machine).ok();
            let report = doctor(
                &vault,
                &machine,
                caldav,
                &config_path,
                server.as_deref(),
                Arc::new(SystemClock),
            )
            .await?;
            print_doctor(&report);
            Ok(report.exit_code)
        }
        Command::Status { json } => print_status(&vault, Arc::new(SystemClock), json),
        Command::Rebuild => run_rebuild(&vault, Arc::new(SystemClock)),
        Command::List {
            action: ListAction::Show,
        } => {
            list_show(&machine);
            Ok(0)
        }
        Command::List {
            action: ListAction::Bind { name, collection },
        } => {
            save_binding(&config_path, machine, ListBinding { name, collection })?;
            Ok(0)
        }
        server => {
            let caldav = daemon::build_client(&machine)?;
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

/// Dispatches one command with an injected CalDAV port and clock (hermetic tests, §3).
/// Server-bound commands use `caldav`; offline commands ignore it.
pub async fn run_with<C: CaldavPort>(
    command: Command,
    vault: PathBuf,
    machine: MachineConfig,
    config_path: PathBuf,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<i32, TaskresError> {
    match command {
        Command::Setup {
            non_interactive,
            server,
            docker_root,
            url,
            username,
            password_env,
            collections,
        } => {
            if !non_interactive {
                setup::run_interactive(vault, server, &docker_root, config_path, clock).await?;
                return Ok(0);
            }
            let args = setup::SetupArgs::from_flags(
                vault,
                config_path,
                url,
                username,
                password_env,
                setup::parse_collections(&collections)?,
            )?;
            let summary = setup::run_setup(args, caldav, clock).await?;
            setup::print_summary(&summary);
            Ok(0)
        }
        Command::Daemon { once } => {
            let dc = DaemonConfig {
                debounce_ms: 300,
                poll_secs: machine.caldav.poll_secs,
                once,
            };
            let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            daemon::run_with(vault, machine, dc, shutdown_rx, caldav, clock).await?;
            Ok(0)
        }
        Command::Sync => {
            let report = daemon::run_once_with(&vault, &machine, clock, caldav).await?;
            println!(
                "scanned {} registered {} pushed {} moved {} deleted {} mutations {} inserts {} adopted {} deferred {} parked {}",
                report.scanned_files,
                report.registered,
                report.pushes,
                report.moves,
                report.deletes,
                report.markdown_mutations,
                report.inserts,
                report.adoptions,
                report.deferred,
                report.parked
            );
            Ok(0)
        }
        Command::Add {
            text,
            priority,
            due,
        } => {
            let priority = parse_priority(priority)?;
            let due = parse_due(due)?;
            let cfg = load_vault_config(&vault)?;
            let engine = Engine::new(&vault, cfg, machine, caldav, clock);
            let task = engine.add(&text, priority, due).await?;
            println!("{}", task.uid);
            Ok(0)
        }
        Command::Done { selector } => {
            set_done(&vault, machine, caldav, clock, selector, true).await
        }
        Command::Undone { selector } => {
            set_done(&vault, machine, caldav, clock, selector, false).await
        }
        Command::Doctor { server } => {
            let report = doctor(
                &vault,
                &machine,
                Some(caldav),
                &config_path,
                server.as_deref(),
                clock,
            )
            .await?;
            print_doctor(&report);
            Ok(report.exit_code)
        }
        Command::Status { json } => print_status(&vault, clock, json),
        Command::Rebuild => run_rebuild(&vault, clock),
        Command::List { action } => match action {
            ListAction::Show => {
                list_show(&machine);
                Ok(0)
            }
            ListAction::Bind { name, collection } => {
                save_binding(&config_path, machine, ListBinding { name, collection })?;
                Ok(0)
            }
            ListAction::Create { name } => {
                let slug = ListSlug::from_name(&name)?;
                caldav.ensure_collection(&slug, &name).await?;
                save_binding(
                    &config_path,
                    machine,
                    ListBinding {
                        collection: slug.as_str().to_string(),
                        name,
                    },
                )?;
                Ok(0)
            }
        },
    }
}

/// Resolves the vault directory (§13.3): `flag` → `$RESTASK_VAULT` → upward search from
/// `start` for `restask.toml` or `.taskres/`. A miss is a config error (exit 4).
pub fn resolve_vault_with(
    flag: Option<&Path>,
    env: Option<&str>,
    start: &Path,
) -> Result<PathBuf, TaskresError> {
    if let Some(flag) = flag {
        return Ok(flag.to_path_buf());
    }
    if let Some(value) = env.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("restask.toml").is_file() || dir.join(".taskres").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(TaskresError::Config {
        path: "<vault>".to_string(),
        reason: "no vault found: pass --vault, set RESTASK_VAULT, or run inside a vault \
                 (restask.toml or .taskres/)"
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
) -> Result<PathBuf, TaskresError> {
    if let Ok(vault) = resolve_vault_with(flag, env, start) {
        return Ok(vault);
    }
    if non_interactive && !start.join("TODO.md").is_file() {
        return Err(TaskresError::Config {
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
) -> Result<PathBuf, TaskresError> {
    let env = std::env::var(ENV_VAULT).ok();
    let cwd = std::env::current_dir()?;
    resolve_setup_vault_with(flag, env.as_deref(), &cwd, non_interactive)
}

/// [`resolve_vault_with`] against the process environment and working directory.
pub fn resolve_vault(flag: Option<&Path>) -> Result<PathBuf, TaskresError> {
    let env = std::env::var(ENV_VAULT).ok();
    let cwd = std::env::current_dir()?;
    resolve_vault_with(flag, env.as_deref(), &cwd)
}

/// Process exit code for `error` (§12.1): 4 config invalid (incl. no vault found), 3
/// CalDAV unreachable, 1 any other runtime failure; clap reports usage errors as 2.
pub fn exit_code(error: &TaskresError) -> i32 {
    match error {
        TaskresError::Config { .. } => 4,
        TaskresError::Caldav {
            kind: CaldavErrorKind::Network | CaldavErrorKind::Tls,
            ..
        } => 3,
        _ => 1,
    }
}

/// Computes the [`StatusReport`] for `vault` (§13.3): a read-only vault scan plus the
/// index and outbox state under `.taskres/`.
pub fn status_report(vault: &Path, clock: &dyn Clock) -> Result<StatusReport, TaskresError> {
    let cfg = load_vault_config(vault)?;
    let tasks = scan_local(vault, &cfg, clock)?;
    let index = Index::load(&vault.join(".taskres"))?;
    let mut lists: BTreeMap<String, usize> = BTreeMap::new();
    let mut priorities: BTreeMap<String, usize> = BTreeMap::new();
    let mut done_today = 0usize;
    let today = clock.today_local();
    for task in tasks.values() {
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
        outbox_backlog: outbox_backlog(&vault.join(".taskres")),
        last_sync,
    })
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
/// endpoint is configured; the optional `--server SSH` alias adds a read-only compose
/// probe. Check failures land in the report with the derived exit code, never as `Err`.
pub async fn doctor<C: CaldavPort>(
    vault: &Path,
    machine: &MachineConfig,
    caldav: Option<C>,
    config_path: &Path,
    server: Option<&str>,
    clock: Arc<dyn Clock>,
) -> Result<DoctorReport, TaskresError> {
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
    match scan_local(vault, &cfg, clock.as_ref()) {
        Ok(tasks) => {
            checks.push(check("routing", DoctorStatus::Ok, "no list conflicts"));
            checks.push(check(
                "scan",
                DoctorStatus::Ok,
                format!("{} routed task(s)", tasks.len()),
            ));
        }
        Err(TaskresError::ListConflict { dir, a, b }) => {
            checks.push(check(
                "routing",
                DoctorStatus::Fail,
                format!("{dir}: {a} vs {b}"),
            ));
            exit_code = 1;
        }
        Err(error @ TaskresError::UidConflict { .. }) => {
            // Routing succeeded (no list conflict); the duplicate UID broke the scan.
            checks.push(check("routing", DoctorStatus::Ok, "no list conflicts"));
            checks.push(check("scan", DoctorStatus::Fail, error));
            exit_code = 1;
        }
        Err(TaskresError::Config { path, reason }) => {
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
        Ok(contents) if contents.contains(MARKER) => checks.push(check(
            "todo-marker",
            DoctorStatus::Ok,
            format!("{} carries the marker", cfg.inbox_file),
        )),
        Ok(_) => checks.push(check(
            "todo-marker",
            DoctorStatus::Warn,
            format!(
                "{} lacks the Taskres marker (restask setup writes it)",
                cfg.inbox_file
            ),
        )),
        Err(_) => checks.push(check(
            "todo-marker",
            DoctorStatus::Warn,
            format!("{} is missing", cfg.inbox_file),
        )),
    }

    // Syncthing conflict artifacts signal unresolved divergences.
    if let Ok(matchers) = cfg.matchers() {
        let mut files: Vec<(String, String)> = Vec::new();
        if walk(vault, "", &matchers, &mut files).is_ok() {
            let conflicts = files
                .iter()
                .filter(|(path, _)| path.contains("sync-conflict"))
                .count();
            if conflicts > 0 {
                checks.push(check(
                    "sync-conflict",
                    DoctorStatus::Warn,
                    format!("{conflicts} file(s) — resolve and delete them"),
                ));
            } else {
                checks.push(check("sync-conflict", DoctorStatus::Ok, "none"));
            }
        }
    }

    // Optional read-only SSH probe (§13.3 `--server`, default docker root of §13.2).
    if let Some(alias) = server {
        match std::process::Command::new("ssh")
            .args([alias, "cat", "/opt/docker/radicale/docker-compose.yml"])
            .output()
        {
            Ok(output) if output.status.success() => {
                let facts = setup::parse_radicale_compose(&String::from_utf8_lossy(&output.stdout));
                if facts.host_port.is_some() {
                    checks.push(check(
                        "server-ssh",
                        DoctorStatus::Ok,
                        format!("{alias}: radicale compose readable"),
                    ));
                } else {
                    checks.push(check(
                        "server-ssh",
                        DoctorStatus::Warn,
                        format!("{alias}: compose has no 5232 mapping — confirm the URL manually"),
                    ));
                }
            }
            _ => checks.push(check(
                "server-ssh",
                DoctorStatus::Warn,
                format!("{alias}: compose not readable via ssh"),
            )),
        }
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
                                        Err(TaskresError::Caldav {
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
            Err(TaskresError::Caldav {
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
        None => checks.push(check(
            "caldav",
            DoctorStatus::Warn,
            "not configured (restask setup)",
        )),
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

/// Completes/reopens via [`Engine::set_done`] after resolving the selector (§13.3).
async fn set_done<C: CaldavPort>(
    vault: &Path,
    machine: MachineConfig,
    caldav: C,
    clock: Arc<dyn Clock>,
    selector: Selector,
    done: bool,
) -> Result<i32, TaskresError> {
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
) -> Result<TaskUid, TaskresError> {
    if let Some(raw) = &selector.uid {
        return TaskUid::parse(raw).map_err(|error| TaskresError::Validation {
            field: "uid",
            reason: error.0,
        });
    }
    let (file, line) = match (&selector.file, selector.line) {
        (Some(file), Some(line)) => (file, line),
        _ => {
            return Err(TaskresError::Validation {
                field: "selector",
                reason: "use --uid or both --file and --line".to_string(),
            })
        }
    };
    let contents = std::fs::read_to_string(vault.join(file))?;
    parse(&contents, cfg)
        .tasks
        .into_iter()
        .find(|task| task.line_no == line)
        .and_then(|task| task.draft.uid)
        .ok_or_else(|| TaskresError::Validation {
            field: "line",
            reason: format!("no registered task at {file}:{line}"),
        })
}

/// Parses `--priority` (§13.3): one of the five CLI names.
fn parse_priority(raw: Option<String>) -> Result<Option<Priority>, TaskresError> {
    raw.map(|name| {
        Priority::from_cli_name(&name).ok_or_else(|| TaskresError::Validation {
            field: "priority",
            reason: format!(
                "unknown priority `{name}` (expected highest, high, medium, low, lowest)"
            ),
        })
    })
    .transpose()
}

/// Parses `--due` (§13.3): `YYYY-MM-DD[ HH:MM]` (§4: a date-only value is midnight UTC).
fn parse_due(raw: Option<String>) -> Result<Option<When>, TaskresError> {
    raw.map(|text| {
        When::parse_date_or_datetime(&text).map_err(|error| TaskresError::Validation {
            field: "due",
            reason: error.to_string(),
        })
    })
    .transpose()
}

/// Prints the [`StatusReport`] (human text or JSON) to stdout (§12.2 one-shot format).
fn print_status(vault: &Path, clock: Arc<dyn Clock>, json: bool) -> Result<i32, TaskresError> {
    let report = status_report(vault, clock.as_ref())?;
    if json {
        let rendered =
            serde_json::to_string(&report).map_err(|error| TaskresError::Validation {
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
        println!("outbox backlog: {}", report.outbox_backlog);
        println!(
            "last sync: {}",
            report.last_sync.as_deref().unwrap_or("never")
        );
    }
    Ok(0)
}

/// Re-derives state and prints the outcome (§13.3 `restask rebuild`).
fn run_rebuild(vault: &Path, clock: Arc<dyn Clock>) -> Result<i32, TaskresError> {
    let cfg = load_vault_config(vault)?;
    let count = rebuild_state(vault, &cfg, clock.as_ref())?;
    println!("re-derived {count} task(s)");
    Ok(0)
}

/// Re-derives `.taskres/` index + cache from the vault (§13.3 `restask rebuild`): entries
/// whose thumbprint is unchanged keep their etag, stale entries and caches are dropped,
/// and the server is never contacted.
fn rebuild_state(
    vault: &Path,
    cfg: &VaultConfig,
    clock: &dyn Clock,
) -> Result<usize, TaskresError> {
    let state_dir = vault.join(".taskres");
    let tasks = scan_local(vault, cfg, clock)?;
    let now = clock.now_utc();
    let old = Index::load(&state_dir)?;
    let mut index = Index::default();
    for (uid, task) in &tasks {
        let thumbprint = task.thumbprint();
        let caldav_etag = old
            .get(uid)
            .filter(|entry| entry.thumbprint == thumbprint)
            .and_then(|entry| entry.caldav_etag.clone());
        index.upsert(IndexEntry {
            uid: uid.clone(),
            list: task.list.clone(),
            source_path: task.source.path.clone(),
            thumbprint,
            caldav_etag,
            seen_at: now,
            defer_count: 0,
        });
        cache_write(&state_dir, task, now)?;
    }
    for uid in old.entries.keys() {
        if index.get(uid).is_none() {
            cache_remove(&state_dir, uid)?;
        }
    }
    index.save(&state_dir)?;
    Ok(index.entries.len())
}

/// Prints the recorded list bindings (§13.3 `restask list show`).
fn list_show(machine: &MachineConfig) {
    for binding in &machine.lists {
        println!("{}\t{}", binding.name, binding.collection);
    }
}

/// Upserts `binding` (by list name) into the machine config at `path` and saves it.
fn save_binding(
    path: &Path,
    mut machine: MachineConfig,
    binding: ListBinding,
) -> Result<(), TaskresError> {
    machine
        .lists
        .retain(|existing| existing.name != binding.name);
    machine.lists.push(binding);
    machine.save(path).map_err(|error| TaskresError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// Loads the machine config (§14.2) with §14.3 env overrides; a missing file yields the
/// default config (fresh machine).
fn load_machine(path: &Path) -> Result<MachineConfig, TaskresError> {
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
        Err(error) => Err(TaskresError::Config {
            path: path.display().to_string(),
            reason: error.to_string(),
        }),
    }
}

/// Loads `<vault>/restask.toml`, mapping config failures to [`TaskresError::Config`].
fn load_vault_config(vault: &Path) -> Result<VaultConfig, TaskresError> {
    let path = vault.join("restask.toml");
    VaultConfig::load(&path).map_err(|error| TaskresError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// Number of operations parked in `.taskres/outbox.json` (0 when absent or unreadable).
fn outbox_backlog(state_dir: &Path) -> usize {
    let contents = match std::fs::read_to_string(state_dir.join("outbox.json")) {
        Ok(contents) => contents,
        Err(_) => return 0,
    };
    match serde_json::from_str::<serde_json::Value>(&contents) {
        Ok(value) => value
            .get("queue")
            .and_then(serde_json::Value::as_array)
            .map_or(0, |queue| queue.len()),
        Err(_) => 0,
    }
}

/// Read-only vault scan: every routed, registered task (§13.3 `status`/`rebuild`). Mirrors
/// the engine's scan (routing, inbox mirror-line skip, UID conflict check) without
/// registering new lines or writing any file.
fn scan_local(
    vault: &Path,
    cfg: &VaultConfig,
    clock: &dyn Clock,
) -> Result<BTreeMap<TaskUid, Task>, TaskresError> {
    let matchers = cfg.matchers().map_err(|error| TaskresError::Config {
        path: vault.join("restask.toml").display().to_string(),
        reason: error.to_string(),
    })?;
    let mut files: Vec<(String, String)> = Vec::new();
    walk(vault, "", &matchers, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let metas: Vec<NoteMeta> = files
        .iter()
        .map(|(path, contents)| {
            let (file_list, folder_list) = scan_frontmatter(contents);
            NoteMeta {
                path: path.to_string(),
                file_list,
                folder_list,
            }
        })
        .collect();
    let router = Router::build(&metas)?;
    let inbox = inbox_slug()?;

    let mut local: BTreeMap<TaskUid, Task> = BTreeMap::new();
    let mut seen: BTreeMap<TaskUid, String> = BTreeMap::new();
    // Notes first, then the inbox file: mirror lines lose to their source note's line.
    let mut order: Vec<usize> = (0..files.len())
        .filter(|&i| files[i].0 != cfg.inbox_file)
        .collect();
    order.extend((0..files.len()).filter(|&i| files[i].0 == cfg.inbox_file));
    for i in order {
        let (path, contents) = &files[i];
        let meta = &metas[i];
        let routing = if path.as_str() == cfg.inbox_file {
            NoteRouting::List(inbox.clone())
        } else {
            router.resolve(path, meta.file_list.as_deref())
        };
        let NoteRouting::List(list) = routing else {
            continue;
        };
        let parsed = parse(contents, cfg);
        let parents = link_parents(&parsed.tasks);
        let mtime = file_mtime(&vault.join(path))?;
        for (task, parent) in parsed.tasks.iter().zip(parents) {
            // Mirror lines in the inbox file are rendered views, never sources.
            if path.as_str() == cfg.inbox_file
                && (task.raw.contains("[[")
                    || task
                        .draft
                        .uid
                        .as_ref()
                        .is_some_and(|uid| seen.contains_key(uid)))
            {
                continue;
            }
            let Some(uid) = task.draft.uid.clone() else {
                continue;
            };
            if let Some(first) = seen.insert(uid.clone(), path.clone()) {
                return Err(TaskresError::UidConflict {
                    uid,
                    a: first,
                    b: path.clone(),
                });
            }
            let status = if task.in_done_region || task.draft.checked {
                Status::Completed {
                    on: task
                        .draft
                        .completed_on
                        .unwrap_or_else(|| clock.today_local()),
                }
            } else {
                Status::Active
            };
            local.insert(
                uid.clone(),
                Task {
                    uid,
                    list: list.clone(),
                    text: task.draft.text.clone(),
                    status,
                    priority: task.draft.priority,
                    due: task.draft.due,
                    start: task.draft.start,
                    scheduled: task.draft.scheduled,
                    created: task.draft.created,
                    parent,
                    source: SourceRef {
                        path: path.clone(),
                        line: task.line_no,
                    },
                    source_heading: task.heading.clone(),
                    source_mtime: mtime,
                    last_modified: mtime,
                },
            );
        }
    }
    Ok(local)
}

/// The engine-managed inbox list (§5.2): TODO.md routes to the `inbox` collection.
fn inbox_slug() -> Result<ListSlug, TaskresError> {
    ListSlug::from_name("Inbox").map_err(|_| TaskresError::Validation {
        field: "inbox",
        reason: "cannot slugify the inbox list name".to_string(),
    })
}

/// File mtime as a UTC instant (the engine's scan semantics).
fn file_mtime(path: &Path) -> Result<DateTime<Utc>, TaskresError> {
    Ok(std::fs::metadata(path)?.modified()?.into())
}

/// Recursively collects tracked files as `(vault-relative path, contents)` pairs
/// (mirrors the engine's scan walk; `ignore` beats `track`).
fn walk(
    dir: &Path,
    relative: &str,
    matchers: &VaultMatchers,
    out: &mut Vec<(String, String)>,
) -> Result<(), TaskresError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &child, matchers, out)?;
        } else if matchers.is_tracked(&child) {
            match std::fs::read_to_string(entry.path()) {
                Ok(contents) => out.push((child, contents)),
                Err(error) => {
                    tracing::warn!(path = %child, %error, "unreadable vault file skipped");
                }
            }
        }
    }
    Ok(())
}
