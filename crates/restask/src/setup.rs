//! Setup wizard (§13.2): composes the vault scaffold, TODO.md adoption, machine-config
//! records, list bindings, and the first full reconcile. The pure/plan surfaces
//! ([`adopt_todo_md`], [`run_setup`]) carry the tested behavior; [`run_interactive`] is a
//! thin TTY shell over them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::caldav::{CaldavClient, CaldavPort, CollectionInfo};
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
    /// Calendar TODO.md (the inbox) is bound to, recorded as `vault.inbox_list` (§14.1).
    /// Interactive setup always sets it; non-interactive setup derives it from a
    /// `--collection inbox=<calendar>` flag.
    pub inbox_collection: Option<String>,
    /// `list=collection` bindings from `--collection` flags (repeatable).
    pub collections: Vec<(String, String)>,
}

impl SetupArgs {
    /// Validates the §13.2 non-interactive flags (`--url`, `--username`, `--password-env`
    /// are all required when `--non-interactive` is set). A binding named `inbox`
    /// (case-insensitive) is lifted into [`SetupArgs::inbox_collection`].
    pub fn from_flags(
        vault: PathBuf,
        config_path: PathBuf,
        url: Option<String>,
        username: Option<String>,
        password_env: Option<String>,
        mut collections: Vec<(String, String)>,
    ) -> Result<Self, TaskresError> {
        let missing = |flag: &str| TaskresError::Validation {
            field: "setup",
            reason: format!("--{flag} is required with --non-interactive"),
        };
        let inbox_index = collections
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case("inbox"));
        let inbox_collection = inbox_index.map(|index| collections.remove(index).1);
        Ok(Self {
            vault,
            config_path,
            url: url.ok_or_else(|| missing("url"))?,
            username: username.ok_or_else(|| missing("username"))?,
            password_env: Some(password_env.ok_or_else(|| missing("password-env"))?),
            password_file: None,
            inbox_collection,
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
    config_path: PathBuf,
    clock: Arc<dyn Clock>,
) -> Result<(), TaskresError> {
    let known = vault.join("restask.toml").is_file() || vault.join(".restask").is_dir();
    if !known && !crate::tui::confirm(&format!("Use {} as the vault?", vault.display()))? {
        return Err(TaskresError::Validation {
            field: "vault",
            reason: "setup cancelled".to_string(),
        });
    }

    let url = crate::tui::prompt("CalDAV URL (e.g. http://radicale.local:5232):")?;
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

    // Step 4 (§13.2): the one required binding — TODO.md (the inbox) is bound to a
    // calendar chosen by name from the server's list. Other lists are declared by hand
    // with `restask-list` frontmatter in the notes; no per-list wizard probing happens.
    let server_collections = client.list_collections().await?;
    if server_collections.is_empty() {
        return Err(TaskresError::Validation {
            field: "collections",
            reason: "no calendars found on the server; create one and re-run `restask setup` \
                     — binding TODO.md is required for sync"
                .to_string(),
        });
    }
    let names: Vec<&str> = server_collections
        .iter()
        .map(|collection| collection.slug.as_str())
        .collect();
    println!("Server calendars: {}", names.join(", "));
    let inbox = loop {
        let typed = crate::tui::prompt("Bind TODO.md to (insert one of the calendars above):")?;
        match match_collection(&typed, &server_collections) {
            Some(collection) => break collection.slug.clone(),
            None => println!("`{typed}` is not one of: {}", names.join(", ")),
        }
    };

    let args = SetupArgs {
        vault,
        config_path: config_path.clone(),
        url,
        username,
        password_env: None,
        password_file: Some(passwd),
        inbox_collection: Some(inbox),
        collections: Vec::new(),
    };
    let summary = prepare_and_sync(args, client, clock).await?;
    print_summary(&summary);
    Ok(())
}

/// Case-insensitively matches a typed calendar name against the server's collections
/// (§13.2 step 4), returning the canonical entry so the binding and the TODO.md
/// frontmatter always use the server's own slug. `None` means the wizard re-prompts.
pub fn match_collection<'a>(
    typed: &str,
    collections: &'a [CollectionInfo],
) -> Option<&'a CollectionInfo> {
    let typed = typed.trim();
    collections
        .iter()
        .find(|collection| collection.slug.eq_ignore_ascii_case(typed))
}

/// Shared setup body: vault scaffold, TODO.md adoption, machine-config records, bound
/// collection creation, first reconcile.
async fn prepare_and_sync<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
) -> Result<SetupSummary, TaskresError> {
    let vault = args.vault.clone();

    // Step 1 — vault: §14-default restask.toml when missing, plus `.restask/`.
    let config_path = vault.join("restask.toml");
    let mut cfg = if config_path.is_file() {
        VaultConfig::load(&config_path).map_err(|error| config_error(&config_path, &error))?
    } else {
        let cfg = VaultConfig::default();
        cfg.save(&config_path)
            .map_err(|error| config_error(&config_path, &error))?;
        cfg
    };
    // The user-chosen calendar becomes the inbox list (§5.2); restask.toml carries it so
    // every synced device renders TODO.md identically (§7 determinism).
    if let Some(inbox_collection) = &args.inbox_collection {
        let slug = ListSlug::from_name(inbox_collection)?;
        if cfg.inbox_list != slug.as_str() {
            cfg.inbox_list = slug.as_str().to_string();
            cfg.save(&config_path)
                .map_err(|error| config_error(&config_path, &error))?;
        }
    }
    std::fs::create_dir_all(vault.join(".restask"))?;

    // Step 2 — TODO.md adoption, only when the file exists and lacks the §7 marker.
    let inbox = vault.join(&cfg.inbox_file);
    let mut backup = None;
    let mut migrated = 0usize;
    let header = format!(
        "---\nrestask-list: {}\n---\n\n# Tasks\n\n{MARKER}\n\n## Inbox\n",
        cfg.inbox_list
    );
    if inbox.is_file() {
        let existing = std::fs::read_to_string(&inbox)?;
        if !existing.contains(MARKER) {
            let outcome = adopt_todo_md(&existing, clock.as_ref());
            mutator::write_atomic(&vault.join(&outcome.backup), &existing)?;
            let mut fresh = header.clone();
            for text in &outcome.migrated {
                fresh.push_str(&format!("- [ ] {text}\n"));
            }
            migrated = outcome.migrated.len();
            mutator::write_atomic(&inbox, &fresh)?;
            backup = Some(outcome.backup);
        }
    } else {
        mutator::write_atomic(&inbox, &header)?;
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
    if let Some(inbox_collection) = &args.inbox_collection {
        let slug = ListSlug::from_name(inbox_collection)?;
        caldav
            .ensure_collection(&slug, &slug.display_name())
            .await?;
        collections.push(slug.as_str().to_string());
        machine.lists.push(ListBinding {
            name: cfg.inbox_list.clone(),
            collection: slug.as_str().to_string(),
        });
    }
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
         contrib/restask.service and keep .restask/ inside the Syncthing share"
    );
}

/// Maps a [`crate::config::ConfigError`] to the crate error for `path`.
fn config_error(path: &Path, error: impl std::fmt::Display) -> TaskresError {
    TaskresError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}
