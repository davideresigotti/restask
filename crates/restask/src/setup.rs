//! Setup wizard (§13.2): composes the vault scaffold (with the Obsidian plugin installed
//! and enabled in it), the fresh TODO.md creation (any pre-existing file is renamed to the
//! timestamped backup), machine-config records, the typed inbox binding, the first full
//! reconcile, and the systemd user-unit install that keeps the daemon running on this
//! vault. [`run_setup`] carries the tested behavior;
//! [`run_interactive`] is a thin TTY shell over it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::caldav::{CaldavClient, CaldavPort, CollectionInfo};
use crate::config::{CaldavConfig, MachineConfig, VaultConfig, VaultSection};
use crate::domain::{Clock, ListSlug};
use crate::fsio;
use crate::markdown::render;
use crate::store::{cache_remove, Index};
use crate::sync::{Engine, ReconcileReport};
use crate::RestaskError;

/// Parses the repeatable `--collection list=collection` flag values (§13.2).
pub fn parse_collections(raw: &[String]) -> Result<Vec<(String, String)>, RestaskError> {
    raw.iter()
        .map(|item| match item.split_once('=') {
            Some((name, collection)) if !name.is_empty() && !collection.is_empty() => {
                Ok((name.to_string(), collection.to_string()))
            }
            _ => Err(RestaskError::Validation {
                field: "collection",
                reason: format!("expected list=collection, got `{item}`"),
            }),
        })
        .collect()
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
    /// Further `name=collection` pairs from `--collection` flags (repeatable): each
    /// collection is created up front with `name` as its display name. Routing itself
    /// is declared in the notes (§5), never here.
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
    ) -> Result<Self, RestaskError> {
        let missing = |flag: &str| RestaskError::Validation {
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
    /// Backup file name, when a pre-existing TODO.md was renamed aside (every run that
    /// finds one creates one).
    pub backup: Option<String>,
    /// Collections ensured (created or verified) before the first sync.
    pub collections: Vec<String>,
    /// Human note about the Obsidian plugin install (§13.2 step 1); `None` when it failed
    /// (a warning was logged).
    pub plugin: Option<String>,
    /// Human note about the daemon unit install (§13.2 step 6), when one was enabled.
    pub daemon: Option<String>,
    /// The first full reconcile's report.
    pub report: ReconcileReport,
}

/// Executes the non-interactive setup plan (§13.2): scaffold the vault and install the
/// Obsidian plugin in it, recreate TODO.md (renaming any existing file to the backup), record the machine config, ensure the
/// inbox collection exists, run the first full reconcile, and install the daemon unit
/// via `installer` (pass `None` to skip — hermetic tests).
pub async fn run_setup<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
) -> Result<SetupSummary, RestaskError> {
    prepare_and_sync(args, caldav, clock, installer).await
}

/// Interactive setup (§13.2): prompts for every step, verifies credentials with a
/// `PROPFIND` (401 re-prompts up to 3 tries), stores the password only in the
/// machine-local `radicale.passwd` (0600, §17), then shares the non-interactive plan.
pub async fn run_interactive(
    vault: PathBuf,
    config_path: PathBuf,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
) -> Result<(), RestaskError> {
    let known = vault.join("restask.toml").is_file() || vault.join(".restask").is_dir();
    if !known && !crate::tui::confirm(&format!("Use {} as the vault?", vault.display()))? {
        return Err(RestaskError::Validation {
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
                error @ RestaskError::Caldav {
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
        return Err(RestaskError::Validation {
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
    let summary = prepare_and_sync(args, client, clock, installer).await?;
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

/// Shared setup body: vault scaffold, Obsidian plugin, fresh TODO.md, machine config,
/// collection creation, first reconcile, daemon-unit install.
async fn prepare_and_sync<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
) -> Result<SetupSummary, RestaskError> {
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

    // The vault is set: put the Obsidian plugin in it. Best-effort like the daemon unit —
    // the plugin is a convenience (§15) and a vault nobody opens in Obsidian loses nothing.
    let plugin = match install_obsidian_plugin(&vault, &cfg) {
        Ok(note) => Some(note),
        Err(error) => {
            tracing::warn!(%error, "obsidian plugin install failed; see INSTALL.md to add it by hand");
            None
        }
    };

    // Step 2 — TODO.md: a fresh engine-owned file in every scenario. A pre-existing file
    // is renamed to the timestamped backup; its tasks are NOT migrated (0.1.0 — the user
    // reconciles the backup manually; tasks already on the bound calendar flow back in
    // through the first sync).
    let inbox = vault.join(&cfg.inbox_file);
    let mut backup = None;
    if inbox.is_file() {
        let stem = cfg
            .inbox_file
            .strip_suffix(".md")
            .unwrap_or(&cfg.inbox_file);
        let stamp = clock
            .now_utc()
            .with_timezone(&clock.local_offset())
            .format("%Y%m%d-%H%M%S");
        let name = format!("{stem}.pre-restask-{stamp}.md");
        std::fs::rename(&inbox, vault.join(&name))?;
        backup = Some(name);
        // The lines just left the vault with the file. Without this, the first sync
        // would read "known tasks whose lines are gone" as deletions and remove them
        // from the server; forgetting them makes the server copies flow back in instead.
        forget_inbox_tasks(&vault, &cfg)?;
    }
    let fresh = render(&BTreeMap::new(), &cfg);
    fsio::write_atomic(&inbox, &fresh)?;

    // Steps 3–4 — machine config (endpoint + secret reference, never the secret); the
    // inbox collection and any requested ones exist before the first sync.
    let machine = MachineConfig {
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
    };
    let mut collections = Vec::new();
    if let Some(inbox_collection) = &args.inbox_collection {
        let slug = ListSlug::from_name(inbox_collection)?;
        caldav
            .ensure_collection(&slug, &slug.display_name())
            .await?;
        collections.push(slug.as_str().to_string());
    }
    for (name, collection) in &args.collections {
        let slug = ListSlug::from_name(collection)?;
        caldav.ensure_collection(&slug, name).await?;
        collections.push(slug.as_str().to_string());
    }
    machine
        .save(&args.config_path)
        .map_err(|error| config_error(&args.config_path, &error))?;

    // Step 5 — first sync: registers fresh lines, pulls tasks from the bound calendar.
    let engine = Engine::new(&vault, cfg, machine, caldav, clock);
    let report = engine.reconcile().await?;

    // Step 6 — daemon (§13.2): install/refresh the systemd user unit so this vault keeps
    // syncing without manual steps. A failed install is a warning — the sync already
    // happened and `restask doctor` diagnoses the environment.
    let daemon = install_daemon(installer, &vault);

    Ok(SetupSummary {
        vault,
        config_path: args.config_path,
        backup,
        collections,
        plugin,
        daemon,
        report,
    })
}

/// Drops the sync state of every task that lived in the inbox file, plus the remembered
/// render: the next sync treats the server's inbox tasks as new arrivals.
fn forget_inbox_tasks(vault: &Path, cfg: &VaultConfig) -> Result<(), RestaskError> {
    let state_dir = vault.join(crate::vault::STATE_DIR);
    let mut index = Index::load(&state_dir)?;
    let inbox_tasks: Vec<_> = index
        .entries
        .values()
        .filter(|entry| entry.source_path == cfg.inbox_file)
        .map(|entry| entry.uid.clone())
        .collect();
    for uid in &inbox_tasks {
        cache_remove(&state_dir, uid)?;
        index.remove(uid);
    }
    index.save(&state_dir)?;
    match std::fs::remove_file(state_dir.join(crate::sync::engine::RENDERED_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Id of the Obsidian plugin: its folder under `.obsidian/plugins/` and its entry in
/// `community-plugins.json` (`manifest.json` `id`, App. C).
pub const OBSIDIAN_PLUGIN_ID: &str = "restask";

/// The plugin as `plugins/obsidian` builds it, compiled into the binary so setup needs
/// neither the repository nor Node (`npm run build` refreshes these copies, App. C).
const OBSIDIAN_PLUGIN_FILES: [(&str, &str); 3] = [
    ("main.js", include_str!("../assets/obsidian/main.js")),
    (
        "manifest.json",
        include_str!("../assets/obsidian/manifest.json"),
    ),
    ("styles.css", include_str!("../assets/obsidian/styles.css")),
];

/// Installs the Obsidian plugin into `<vault>/.obsidian/plugins/restask/` and lists it in
/// `.obsidian/community-plugins.json` (§13.2 step 1), so the vault opens in Obsidian with
/// the plugin ready — on every device the vault is synced to. Returns the summary note.
///
/// Re-running refreshes the three plugin files and nothing else: the user's plugin
/// settings and every other enabled plugin are kept, and a file that already has the
/// right content is not rewritten. A plugin folder or file that is a symlink is managed
/// by hand (a development checkout) and left alone. A `community-plugins.json` that is
/// not a JSON list is never overwritten; the note then asks to enable the plugin by hand.
pub fn install_obsidian_plugin(vault: &Path, cfg: &VaultConfig) -> Result<String, RestaskError> {
    let obsidian = vault.join(".obsidian");
    let plugin_dir = obsidian.join("plugins").join(OBSIDIAN_PLUGIN_ID);
    if !is_symlink(&plugin_dir) {
        std::fs::create_dir_all(&plugin_dir)?;
        for (name, contents) in OBSIDIAN_PLUGIN_FILES {
            let path = plugin_dir.join(name);
            if !is_symlink(&path) {
                fsio::write_if_changed(&path, contents)?;
            }
        }
        // The plugin moves completed lines under its own `doneHeading` setting, which
        // must name the vault's heading (§15.4). Seed it where no settings exist yet;
        // settings the user already has are theirs.
        let settings = plugin_dir.join("data.json");
        if cfg.done_heading != VaultConfig::default().done_heading
            && std::fs::symlink_metadata(&settings).is_err()
        {
            let seed = serde_json::json!({ "doneHeading": cfg.done_heading });
            fsio::write_atomic(&settings, &format!("{seed:#}"))?;
        }
    }

    let location = format!(".obsidian/plugins/{OBSIDIAN_PLUGIN_ID}");
    Ok(if enable_obsidian_plugin(&obsidian)? {
        format!(
            "obsidian plugin: installed in {location} and enabled (reload Obsidian if this \
             vault is open)"
        )
    } else {
        format!(
            "obsidian plugin: installed in {location}; .obsidian/community-plugins.json is \
             not a list, so enable \"{OBSIDIAN_PLUGIN_ID}\" under Community plugins by hand"
        )
    })
}

/// Adds the plugin id to `.obsidian/community-plugins.json`, keeping every other entry in
/// place. `Ok(false)` means the file holds something other than a JSON list and was left
/// untouched; a file that already lists the plugin is not rewritten.
fn enable_obsidian_plugin(obsidian: &Path) -> Result<bool, RestaskError> {
    let path = obsidian.join("community-plugins.json");
    let current = match std::fs::read_to_string(&path) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut enabled = if current.trim().is_empty() {
        Vec::new()
    } else {
        match serde_json::from_str::<Vec<serde_json::Value>>(&current) {
            Ok(enabled) => enabled,
            Err(_) => return Ok(false),
        }
    };
    if enabled
        .iter()
        .any(|id| id.as_str() == Some(OBSIDIAN_PLUGIN_ID))
    {
        return Ok(true);
    }
    enabled.push(serde_json::Value::from(OBSIDIAN_PLUGIN_ID));
    fsio::write_atomic(&path, &format!("{:#}", serde_json::Value::Array(enabled)))?;
    Ok(true)
}

/// Whether `path` itself is a symbolic link (not followed).
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Installs the machine-local systemd user unit (§13.2 step 6) that keeps
/// `restask daemon` running on this vault after setup. A port so tests record instead of
/// touching the host's systemd session.
pub trait DaemonInstaller {
    /// Writes and enables the unit. `Ok(None)` means this environment cannot host a user
    /// unit (no systemd session) — setup continues without one.
    ///
    /// `vault` is the folder the unit points at; `exec` is the `restask` binary to run.
    fn install(&self, vault: &Path, exec: &Path) -> Result<Option<String>, RestaskError>;
}

/// Real [`DaemonInstaller`]: writes `restask.service` into
/// `$XDG_CONFIG_HOME/systemd/user/`, runs `systemctl --user daemon-reload`,
/// `systemctl --user enable --now restask.service`, and enables linger so the unit
/// survives logout. Re-running setup overwrites the unit (idempotent refresh).
pub struct SystemdInstaller;

impl DaemonInstaller for SystemdInstaller {
    fn install(&self, vault: &Path, exec: &Path) -> Result<Option<String>, RestaskError> {
        if !systemd_user_available() {
            return Ok(None);
        }
        let unit_dir = xdg_config_home().join("systemd").join("user");
        std::fs::create_dir_all(&unit_dir)?;
        let unit_path = unit_dir.join("restask.service");
        std::fs::write(&unit_path, daemon_unit_content(vault, exec))?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run(
            "systemctl",
            &["--user", "enable", "--now", "restask.service"],
        )?;
        if let Err(error) = run("loginctl", &["enable-linger"]) {
            tracing::warn!(%error, "could not enable linger; the daemon stops at logout");
        }
        Ok(Some(format!(
            "daemon: enabled {} (starts now and at boot)",
            unit_path.display()
        )))
    }
}

/// Whether a systemd user session is reachable (`$XDG_RUNTIME_DIR/systemd/` exists).
pub fn systemd_user_available() -> bool {
    #[cfg(unix)]
    {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("systemd").is_dir())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The user-unit body: absolute `ExecStart` (current binary + `daemon --vault`), restart
/// on failure, start with the user session. Paths are double-quoted so vault folders
/// with spaces survive systemd's argv splitter.
pub fn daemon_unit_content(vault: &Path, exec: &Path) -> String {
    format!(
        "[Unit]\nDescription=restask sync daemon (vault <-> CalDAV)\n\n[Service]\n\
         ExecStart=\"{}\" daemon --vault \"{}\"\nRestart=on-failure\nRestartSec=5\n\n\
         [Install]\nWantedBy=default.target\n",
        exec.display(),
        vault.display()
    )
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config` (the §14.2 fallback order).
fn xdg_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"))
}

/// Best-effort daemon-unit install (§13.2 step 6): no installer, an unresolvable binary
/// or an installer error all degrade to a warning — never fail the setup run.
fn install_daemon(installer: Option<&dyn DaemonInstaller>, vault: &Path) -> Option<String> {
    let installer = installer?;
    let exec = match std::env::current_exe() {
        Ok(exec) => exec,
        Err(error) => {
            tracing::warn!(%error, "cannot resolve the restask binary; skipping daemon install");
            return None;
        }
    };
    match installer.install(vault, &exec) {
        Ok(note) => note,
        Err(error) => {
            tracing::warn!(%error, "daemon unit install failed; see `restask doctor`");
            None
        }
    }
}

/// Runs a helper command, failing when it cannot start or exits non-zero.
fn run(program: &str, args: &[&str]) -> Result<(), RestaskError> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{program} {} failed: {status}", args.join(" "))).into())
    }
}

/// Writes the password to `path` with mode 0600 on Unix (§17).
fn write_secret(path: &Path, password: &str) -> Result<(), RestaskError> {
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
        println!("renamed existing TODO.md to {backup}");
    }
    println!("machine config: {}", summary.config_path.display());
    println!("collections: {}", summary.collections.join(", "));
    println!(
        "first sync: scanned {} registered {} pushed {}",
        summary.report.scanned_files, summary.report.registered, summary.report.pushes
    );
    if let Some(note) = &summary.plugin {
        println!("{note}");
    }
    if let Some(note) = &summary.daemon {
        println!("{note}");
    }
    println!(
        "client wiring: <url>/<user>/<slug>/ per bound collection; keep .restask/ inside \
         the Syncthing share"
    );
}

/// Maps a [`crate::config::ConfigError`] to the crate error for `path`.
fn config_error(path: &Path, error: impl std::fmt::Display) -> RestaskError {
    RestaskError::Config {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}
