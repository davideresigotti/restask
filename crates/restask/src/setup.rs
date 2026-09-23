//! Setup wizard (§13.2): composes the vault scaffold, TODO.md adoption, machine-config
//! records, list bindings, and the first full reconcile. The pure/plan surfaces
//! ([`parse_radicale_compose`], [`adopt_todo_md`], [`run_setup`]) carry the tested
//! behavior; [`run_interactive`] is a thin TTY shell over them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::caldav::{CaldavClient, CaldavPort};
use crate::config::{CaldavConfig, ListBinding, MachineConfig, VaultConfig, VaultSection};
use crate::domain::{Clock, ListSlug};
use crate::markdown::{mutator, parse, MARKER};
use crate::sync::{Engine, ReconcileReport};
use crate::TaskresError;

/// Parses the repeatable `--collection list=collection` flag values (§13.2).
pub fn parse_collections(raw: &[String]) -> Result<Vec<(String, String)>, TaskresError> {
    raw.iter()
        .map(|item| match item.split_once('=') {
            Some((name, collection)) if !name.is_empty() && !collection.is_empty() => {
                Ok((name.to_string(), collection.to_string()))
            }
            _ => Err(TaskresError::Validation {
                field: "collection",
                reason: format!("expected list=collection, got `{item}`"),
            }),
        })
        .collect()
}

/// Compose discovery facts (§13.2 step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ComposeFacts {
    /// Host-side port of the Radicale mapping, when present.
    pub host_port: Option<u16>,
    /// Container-side port of the Radicale mapping, when present.
    pub container_port: Option<u16>,
}

/// Parses a docker-compose file and extracts the Radicale service's `5232` port mapping
/// (§13.2 step 3): the service whose `image` contains `radicale`; the short syntax
/// (`[bind:]host:container[/proto]`) and the long syntax (`{ target, published }`) both
/// work. A missing service or mapping yields `None` fields — the wizard prompts instead
/// of guessing (§13.2: ambiguity never resolves by default).
pub fn parse_radicale_compose(text: &str) -> ComposeFacts {
    let mut facts = ComposeFacts::default();
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return facts;
    };
    let Some(services) = value
        .get("services")
        .and_then(serde_yaml::Value::as_mapping)
    else {
        return facts;
    };
    for (_, service) in services {
        let image = service
            .get("image")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or_default();
        if !image.contains("radicale") {
            continue;
        }
        let Some(ports) = service
            .get("ports")
            .and_then(serde_yaml::Value::as_sequence)
        else {
            continue;
        };
        for port in ports {
            match port {
                serde_yaml::Value::String(mapping) => {
                    let parts: Vec<&str> = mapping.split(':').collect();
                    let (host, container) = match parts.as_slice() {
                        [host, container] => (*host, *container),
                        [_, host, container] => (*host, *container),
                        _ => continue,
                    };
                    let container = container.split('/').next().unwrap_or(container);
                    if container == "5232" {
                        facts.container_port = Some(5232);
                        facts.host_port = host.parse::<u16>().ok();
                    }
                }
                serde_yaml::Value::Mapping(long) => {
                    let target = long
                        .get(serde_yaml::Value::String("target".into()))
                        .and_then(serde_yaml::Value::as_u64);
                    let published = long
                        .get(serde_yaml::Value::String("published".into()))
                        .and_then(serde_yaml::Value::as_u64);
                    if target == Some(5232) {
                        facts.container_port = Some(5232);
                        facts.host_port = published.and_then(|port| u16::try_from(port).ok());
                    }
                }
                _ => {}
            }
        }
    }
    facts
}

/// The TODO.md adoption outcome (§13.2 step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionOutcome {
    /// Backup file name (`TODO.pre-restask-YYYYMMDD-HHMMSS.md`, device-local time); the
    /// caller writes the verbatim original there.
    pub backup: String,
    /// Texts of the checkbox lines carried into the fresh `## Inbox`; UIDs are minted by
    /// the first reconcile's registration pass.
    pub migrated: Vec<String>,
}

/// Pure core of the TODO.md adoption (§13.2 step 2): derives the backup file name for
/// `clock` and the texts of the checkbox lines to migrate. Fenced blocks (obsidian-tasks
/// ```tasks) are skipped — they survive only in the backup.
pub fn adopt_todo_md(existing: &str, clock: &dyn Clock) -> AdoptionOutcome {
    let stamp = clock
        .now_utc()
        .with_timezone(&clock.local_offset())
        .format("%Y%m%d-%H%M%S");
    let cfg = VaultConfig::default();
    let migrated = parse(existing, &cfg)
        .tasks
        .iter()
        .map(|task| task.draft.text.clone())
        .collect();
    AdoptionOutcome {
        backup: format!("TODO.pre-restask-{stamp}.md"),
        migrated,
    }
}

/// Non-interactive setup inputs (§13.2 flags). Exactly one of `password_env` /
/// `password_file` is recorded in the machine config — never the secret itself (§17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupArgs {
    /// Vault directory.
    pub vault: PathBuf,
    /// Machine config target (`config.toml`, chmod 600).
    pub config_path: PathBuf,
    /// CalDAV base URL.
    pub url: String,
    /// CalDAV user name.
    pub username: String,
    /// Env var name holding the password (non-interactive `--password-env`).
    pub password_env: Option<String>,
    /// Passwd file holding the password (interactive path).
    pub password_file: Option<PathBuf>,
    /// `list=collection` bindings from `--collection` flags (repeatable).
    pub collections: Vec<(String, String)>,
}

impl SetupArgs {
    /// Validates the §13.2 non-interactive flags (`--url`, `--username`, `--password-env`
    /// are all required when `--non-interactive` is set).
    pub fn from_flags(
        vault: PathBuf,
        config_path: PathBuf,
        url: Option<String>,
        username: Option<String>,
        password_env: Option<String>,
        collections: Vec<(String, String)>,
    ) -> Result<Self, TaskresError> {
        let missing = |flag: &str| TaskresError::Validation {
            field: "setup",
            reason: format!("--{flag} is required with --non-interactive"),
        };
        Ok(Self {
            vault,
            config_path,
            url: url.ok_or_else(|| missing("url"))?,
            username: username.ok_or_else(|| missing("username"))?,
            password_env: Some(password_env.ok_or_else(|| missing("password-env"))?),
            password_file: None,
            collections,
        })
    }
}

/// What one setup run did (§13.2 step 7 summary inputs).
#[derive(Debug, Clone)]
pub struct SetupSummary {
    /// Vault directory.
    pub vault: PathBuf,
    /// Machine config written.
    pub config_path: PathBuf,
    /// Backup file name, when an unmarked TODO.md was adopted.
    pub backup: Option<String>,
    /// Number of checkbox lines migrated into the fresh inbox.
    pub migrated: usize,
    /// Collections ensured (created or verified) before the first sync.
    pub collections: Vec<String>,
    /// The first full reconcile's report.
    pub report: ReconcileReport,
}

/// Executes the non-interactive setup plan (§13.2): scaffold the vault, adopt an unmarked
/// TODO.md, record the machine config and list bindings, ensure the bound collections
/// exist, and run the first full reconcile.
pub async fn run_setup<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<SetupSummary, TaskresError> {
    prepare_and_sync(args, caldav, clock).await
}

/// Interactive setup (§13.2): prompts for every step, verifies credentials with a
/// `PROPFIND` (401 re-prompts up to 3 tries), stores the password only in the
/// machine-local `radicale.passwd` (0600, §17), then shares the non-interactive plan.
pub async fn run_interactive(
    vault: PathBuf,
    server: Option<String>,
    docker_root: &str,
    config_path: PathBuf,
    clock: Arc<dyn Clock>,
) -> Result<(), TaskresError> {
    let known = vault.join("restask.toml").is_file() || vault.join(".taskres").is_dir();
    if !known && !crate::tui::confirm(&format!("Use {} as the vault?", vault.display()))? {
        return Err(TaskresError::Validation {
            field: "vault",
            reason: "setup cancelled".to_string(),
        });
    }

    let discovered = match &server {
        Some(alias) => discover_url_via_ssh(alias, docker_root)?,
        None => None,
    };
    let url = match discovered {
        Some(url) => url,
        None => crate::tui::prompt("CalDAV URL (e.g. http://radicale.local:5232):")?,
    };
    let username = crate::tui::prompt("CalDAV username:")?;
    let mut password = crate::tui::secret("CalDAV password:")?;
    let mut tries = 0;
    let client = loop {
        let candidate = CaldavClient::new(&url, username.clone(), Some(password.clone()))?;
        match candidate.list_collections().await {
            Ok(_) => break candidate,
            Err(
                error @ TaskresError::Caldav {
                    kind: crate::CaldavErrorKind::Auth,
                    ..
                },
            ) if tries < 2 => {
                tries += 1;
                tracing::warn!(%error, "auth_warning");
                password = crate::tui::secret("CalDAV password (retry):")?;
            }
            Err(error) => return Err(error),
        }
    };

    let dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir)?;
    let passwd = dir.join("radicale.passwd");
    write_secret(&passwd, &password)?;

    let mut collections = Vec::new();
    loop {
        let line = crate::tui::prompt("Bind list=collection (empty to finish):")?;
        if line.is_empty() {
            break;
        }
        match line.split_once('=') {
            Some((name, collection)) if !name.is_empty() && !collection.is_empty() => {
                collections.push((name.to_string(), collection.to_string()));
            }
            _ => println!("expected list=collection"),
        }
    }

    let args = SetupArgs {
        vault,
        config_path: config_path.clone(),
        url,
        username,
        password_env: None,
        password_file: Some(passwd),
        collections,
    };
    let summary = prepare_and_sync(args, client, clock).await?;
    print_summary(&summary);
    Ok(())
}

/// Shared setup body: vault scaffold, TODO.md adoption, machine-config records, bound
/// collection creation, first reconcile.
async fn prepare_and_sync<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<SetupSummary, TaskresError> {
    let vault = args.vault.clone();

    // Step 1 — vault: §14-default restask.toml when missing, plus `.taskres/`.
    let config_path = vault.join("restask.toml");
    let cfg = if config_path.is_file() {
        VaultConfig::load(&config_path).map_err(|error| config_error(&config_path, &error))?
    } else {
        let cfg = VaultConfig::default();
        cfg.save(&config_path)
            .map_err(|error| config_error(&config_path, &error))?;
        cfg
    };
    std::fs::create_dir_all(vault.join(".taskres"))?;

    // Step 2 — TODO.md adoption, only when the file exists and lacks the §7 marker.
    let inbox = vault.join(&cfg.inbox_file);
    let mut backup = None;
    let mut migrated = 0usize;
    if inbox.is_file() {
        let existing = std::fs::read_to_string(&inbox)?;
        if !existing.contains(MARKER) {
            let outcome = adopt_todo_md(&existing, clock.as_ref());
            mutator::write_atomic(&vault.join(&outcome.backup), &existing)?;
            let mut fresh = format!("# Tasks\n\n{MARKER}\n\n## Inbox\n");
            for text in &outcome.migrated {
                fresh.push_str(&format!("- [ ] {text}\n"));
            }
            migrated = outcome.migrated.len();
            mutator::write_atomic(&inbox, &fresh)?;
            backup = Some(outcome.backup);
        }
    } else {
        let fresh = format!("# Tasks\n\n{MARKER}\n\n## Inbox\n");
        mutator::write_atomic(&inbox, &fresh)?;
    }

    // Steps 3–5 — machine config (endpoint + secret reference, never the secret) and the
    // requested bindings; bound collections are created (MKCOL) before the first sync.
    let mut machine = MachineConfig {
        vault: VaultSection {
            path: Some(vault.clone()),
        },
        caldav: CaldavConfig {
            url: Some(args.url.clone()),
            username: Some(args.username.clone()),
            password_env: args.password_env.clone(),
            password_file: args.password_file.clone(),
            ..CaldavConfig::default()
        },
        lists: Vec::new(),
    };
    let mut collections = Vec::new();
    for (name, collection) in &args.collections {
        let slug = ListSlug::from_name(collection)?;
        caldav.ensure_collection(&slug, name).await?;
        collections.push(slug.as_str().to_string());
        machine.lists.push(ListBinding {
            name: name.clone(),
            collection: slug.as_str().to_string(),
        });
    }
    machine
        .save(&args.config_path)
        .map_err(|error| config_error(&args.config_path, &error))?;

    // Step 6 — first sync: registers the migrated inbox lines, pushes every task.
    let engine = Engine::new(&vault, cfg, machine, caldav, clock);
    let report = engine.reconcile().await?;

    Ok(SetupSummary {
        vault,
        config_path: args.config_path,
        backup,
        migrated,
        collections,
        report,
    })
}

/// Writes the password to `path` with mode 0600 on Unix (§17).
fn write_secret(path: &Path, password: &str) -> Result<(), TaskresError> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
            .write_all(password.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, password.as_bytes())?;
        Ok(())
    }
}

/// Read-only server discovery over SSH (§13.2 step 3): `ssh <alias> cat
/// <docker-root>/radicale/docker-compose.yml`, then `ssh -G <alias>` for the hostname.
/// `None` on any failure or ambiguity — the wizard prompts instead of guessing.
fn discover_url_via_ssh(alias: &str, docker_root: &str) -> Result<Option<String>, TaskresError> {
    let compose = Command::new("ssh")
        .args([
            alias,
            "cat",
            &format!("{docker_root}/radicale/docker-compose.yml"),
        ])
        .output()?;
    if !compose.status.success() {
        return Ok(None);
    }
    let facts = parse_radicale_compose(&String::from_utf8_lossy(&compose.stdout));
    let Some(host_port) = facts.host_port else {
        return Ok(None);
    };
    let general = Command::new("ssh").args(["-G", alias]).output()?;
    let host = String::from_utf8_lossy(&general.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(str::to_string)
        .ok_or_else(|| TaskresError::Validation {
            field: "server",
            reason: format!("`ssh -G {alias}` has no hostname line"),
        })?;
    Ok(Some(format!("http://{host}:{host_port}")))
}

/// Prints the §13.2 step-7 summary (backup, config, client wiring, next steps) to stdout.
pub fn print_summary(summary: &SetupSummary) {
    if let Some(backup) = &summary.backup {
        println!(
            "backed up TODO.md to {backup} ({} task(s) migrated)",
            summary.migrated
        );
    }
    println!("machine config: {}", summary.config_path.display());
    println!("collections: {}", summary.collections.join(", "));
    println!(
        "first sync: scanned {} registered {} pushed {}",
        summary.report.scanned_files, summary.report.registered, summary.report.pushes
    );
    println!(
        "client wiring: <url>/<user>/<slug>/ per bound collection; enable \
         contrib/restask.service and keep .taskres/ inside the Syncthing share"
    );
}

/// Maps a [`crate::config::ConfigError`] to the crate error for `path`.
fn config_error(path: &Path, error: impl std::fmt::Display) -> TaskresError {
    TaskresError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}
