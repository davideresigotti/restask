//! Setup wizard (§13.2): composes the vault scaffold (with the Obsidian plugin installed
//! and enabled in it), the fresh TODO.md creation (any pre-existing file is renamed to the
//! timestamped backup), machine-config records, the typed inbox binding, the first full
//! reconcile, and the install of the vault's one daemon ([`DaemonHost`]): as a systemd
//! user unit on this machine, or — over ssh, with the credentials typed here — on the
//! always-on server that holds a copy of the vault. A vault that another device already
//! set up is *joined* instead ([`joins`]): the machine gets its config, a first sync and
//! the unit, and the vault is left as it is. [`run_setup`] carries the tested behavior;
//! [`run_interactive`] is a thin TTY shell over it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::caldav::{CaldavClient, CaldavPort, CollectionInfo};
use crate::config::{CaldavConfig, MachineConfig, NodeSection, VaultConfig, VaultSection};
use crate::domain::{Clock, ListSlug};
use crate::fsio;
use crate::markdown::{is_view, render};
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

/// Stack directory on the sync node when none is given: `restask` in the home directory
/// of the ssh user.
pub const NODE_DIR: &str = "restask";

/// Where the daemon goes when it is installed on another machine (§13.2 step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTarget {
    /// ssh host of the always-on machine, as `ssh <host>` reaches it.
    pub host: String,
    /// The vault's folder on that machine, as the file sync keeps it there.
    pub vault: String,
    /// Directory of the daemon's compose stack there (sources, machine config).
    pub dir: String,
}

/// Which machine runs the vault's one daemon (§1.1) — the question setup settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonHost {
    /// This machine: it is the sync node, keeps the credentials and gets the unit.
    Here,
    /// An always-on server: setup installs the daemon there over ssh and hands it the
    /// credentials. This machine only edits and keeps none.
    Node(NodeTarget),
    /// Another machine, where the daemon is installed by hand (`--no-daemon`). This
    /// machine only edits and keeps no credentials.
    Elsewhere,
}

/// What the command line says about the daemon's place (`--node`, `--node-vault`,
/// `--node-dir`, `--no-daemon`); the interactive wizard asks for what is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonFlags {
    /// `--no-daemon`.
    pub no_daemon: bool,
    /// `--node <ssh-host>`.
    pub node: Option<String>,
    /// `--node-vault <path>`.
    pub node_vault: Option<String>,
    /// `--node-dir <dir>`.
    pub node_dir: Option<String>,
}

impl DaemonFlags {
    /// The place the flags name, without asking: this machine unless they say otherwise.
    /// `--node` needs `--node-vault` here, since nothing can be prompted for.
    pub fn resolve(self) -> Result<DaemonHost, RestaskError> {
        match (self.node, self.no_daemon) {
            (Some(host), _) => Ok(DaemonHost::Node(NodeTarget {
                host,
                vault: self.node_vault.ok_or_else(|| RestaskError::Validation {
                    field: "node-vault",
                    reason: "--node needs --node-vault: the vault's folder on that machine"
                        .to_string(),
                })?,
                dir: self.node_dir.unwrap_or_else(|| NODE_DIR.to_string()),
            })),
            (None, true) => Ok(DaemonHost::Elsewhere),
            (None, false) => Ok(DaemonHost::Here),
        }
    }

    /// [`DaemonFlags::resolve`] for the interactive wizard: with no flag it asks whether
    /// an always-on server holds the vault, and for the vault's folder there.
    fn ask(self) -> Result<DaemonHost, RestaskError> {
        if self.no_daemon && self.node.is_none() {
            return Ok(DaemonHost::Elsewhere);
        }
        let host = match self.node {
            Some(host) => host,
            None => {
                println!(
                    "\nOne machine that is always on keeps the vault and the server in sync. \
                     If a server holds a copy of this vault (through the file sync), restask \
                     is installed there now, over ssh, with the credentials above."
                );
                crate::tui::prompt("ssh host of that server (Enter: this computer does it):")?
            }
        };
        if host.is_empty() {
            return Ok(DaemonHost::Here);
        }
        let vault = match self.node_vault {
            Some(vault) => vault,
            None => loop {
                let typed = crate::tui::prompt(&format!("Vault folder on {host}:"))?;
                if !typed.is_empty() {
                    break typed;
                }
            },
        };
        Ok(DaemonHost::Node(NodeTarget {
            host,
            vault,
            dir: self.node_dir.unwrap_or_else(|| NODE_DIR.to_string()),
        }))
    }
}

/// A password in memory. It is never printed: `Debug` shows a placeholder (§17).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps `password`.
    pub fn new(password: impl Into<String>) -> Self {
        Self(password.into())
    }

    /// The password itself, for the one place that must send it on.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// What the daemon on a node is given to reach the server ([`DaemonInstaller::install_node`]).
#[derive(Debug, Clone, Copy)]
pub struct NodeAccess<'a> {
    /// CalDAV base URL, as the node reaches it.
    pub url: &'a str,
    /// CalDAV user name.
    pub username: &'a str,
    /// CalDAV password.
    pub password: &'a Secret,
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
    /// Join a vault that is already set up (§13.2 *Joining*, see [`joins`]): only this
    /// machine is configured, nothing in the vault is created or replaced.
    pub join: bool,
    /// Which machine runs the vault's daemon (§1.1, §13.2 step 6). Anything but
    /// [`DaemonHost::Here`] makes this an editing machine: no unit, and no credentials
    /// in its machine config.
    pub daemon: DaemonHost,
    /// The password itself, for [`DaemonHost::Node`] only: it is handed to the node's
    /// daemon and recorded nowhere on this machine.
    pub password: Option<Secret>,
}

impl SetupArgs {
    /// Validates the §13.2 non-interactive flags (`--url` and `--username` are required
    /// when `--non-interactive` is set; the password is the caller's to obtain, from
    /// `--password-env` or `--password-stdin`). A binding named `inbox`
    /// (case-insensitive) is lifted into [`SetupArgs::inbox_collection`]. A join takes no
    /// bindings: the vault it joins already names its lists. The daemon is placed on this
    /// machine; the caller sets [`SetupArgs::daemon`] for anything else.
    pub fn from_flags(
        vault: PathBuf,
        config_path: PathBuf,
        url: Option<String>,
        username: Option<String>,
        password_env: Option<String>,
        mut collections: Vec<(String, String)>,
        join: bool,
    ) -> Result<Self, RestaskError> {
        if join && !collections.is_empty() {
            return Err(RestaskError::Validation {
                field: "collection",
                reason: "--collection has no meaning when joining a vault that is already \
                         set up: its restask.toml and notes name the lists"
                    .to_string(),
            });
        }
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
            password_env,
            password_file: None,
            inbox_collection,
            collections,
            join,
            daemon: DaemonHost::Here,
            password: None,
        })
    }
}

/// Whether this setup run joins the vault instead of setting it up (§13.2 *Joining*):
/// asked for with `--join`, or decided by what is there — the vault is already set up and
/// this machine has no config yet, which is a second machine receiving the vault through
/// the file sync. Setting such a vault up again would replace its TODO.md on every device.
///
/// `--join` on a vault that is not set up is an error rather than a fresh setup: on a
/// machine the file sync has not reached yet, a fresh setup would write files that then
/// collide with the ones arriving.
pub fn joins(vault: &Path, config_path: &Path, requested: bool) -> Result<bool, RestaskError> {
    let set_up = vault_is_set_up(vault)?;
    if requested && !set_up {
        return Err(RestaskError::Validation {
            field: "join",
            reason: format!(
                "{} is not set up yet (no restask.toml, or its inbox file is not a restask \
                 view). If the file sync is still delivering the vault, wait for it; to set \
                 the vault up here, run `restask setup` without --join",
                vault.display()
            ),
        });
    }
    Ok(requested || (set_up && !config_path.is_file()))
}

/// Whether some restask already set `vault` up: it has a `restask.toml`, and the inbox
/// file that config names is a view restask rendered.
fn vault_is_set_up(vault: &Path) -> Result<bool, RestaskError> {
    let config_path = vault.join("restask.toml");
    if !config_path.is_file() {
        return Ok(false);
    }
    let cfg =
        VaultConfig::load(&config_path).map_err(|error| config_error(&config_path, &error))?;
    Ok(std::fs::read_to_string(vault.join(&cfg.inbox_file)).is_ok_and(|inbox| is_view(&inbox)))
}

/// What one setup run did (§13.2 step 7 summary inputs).
#[derive(Debug, Clone)]
pub struct SetupSummary {
    /// Vault directory.
    pub vault: PathBuf,
    /// Machine config written.
    pub config_path: PathBuf,
    /// `true` when the run joined a vault that was already set up (§13.2 *Joining*).
    pub joined: bool,
    /// Backup file name, when a pre-existing TODO.md was renamed aside (every run that
    /// finds one creates one).
    pub backup: Option<String>,
    /// Collections ensured (created or verified) before the first sync.
    pub collections: Vec<String>,
    /// Human note about the Obsidian plugin install (§13.2 step 1); `None` when it failed
    /// (a warning was logged).
    pub plugin: Option<String>,
    /// Human note about the daemon (§13.2 step 6): the unit that was enabled, the node
    /// it runs on, or that it is installed by hand elsewhere. `None` when the environment
    /// cannot host a unit or its install failed.
    pub daemon: Option<String>,
    /// Whether this run made a first sync from this machine. A machine that joins a vault
    /// whose daemon runs elsewhere does not: the pass is the sync node's.
    pub synced: bool,
    /// The first full reconcile's report (all zero when [`SetupSummary::synced`] is not).
    pub report: ReconcileReport,
}

/// Executes the non-interactive setup plan (§13.2): scaffold the vault and install the
/// Obsidian plugin in it, recreate TODO.md (renaming any existing file to the backup), record the machine config, ensure the
/// inbox collection exists, run the first full reconcile, and install the daemon where
/// [`SetupArgs::daemon`] says, via `installer` (pass `None` to skip — hermetic tests).
/// With [`SetupArgs::join`] the vault steps are skipped: machine config, first reconcile
/// and daemon only.
pub async fn run_setup<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
) -> Result<SetupSummary, RestaskError> {
    if args.join {
        join_and_sync(args, caldav, clock, installer).await
    } else {
        prepare_and_sync(args, caldav, clock, installer).await
    }
}

/// Interactive setup (§13.2): prompts for every step, verifies credentials with a
/// `PROPFIND` (401 re-prompts up to 3 tries), stores the password only in the
/// machine-local `radicale.passwd` (0600, §17), then shares the non-interactive plan.
/// With `join` (see [`joins`]) the vault already names the calendar TODO.md is bound to.
/// Last it asks which machine runs the daemon, unless `daemon` says so
/// ([`DaemonFlags`]): the password typed once goes to this machine's password file when
/// the daemon runs here, and to the node — and nowhere on this machine — when it runs
/// there.
pub async fn run_interactive(
    vault: PathBuf,
    config_path: PathBuf,
    join: bool,
    daemon: DaemonFlags,
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
    if join {
        println!(
            "{} is already set up for restask: connecting this machine to it. Nothing in \
             the vault is replaced.",
            vault.display()
        );
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

    if join {
        let daemon = daemon.ask()?;
        let args = SetupArgs {
            vault,
            password_file: keep_password(&config_path, &daemon, &password)?,
            config_path,
            url,
            username,
            password_env: None,
            inbox_collection: None,
            collections: Vec::new(),
            join: true,
            daemon,
            password: Some(Secret::new(password)),
        };
        let summary = join_and_sync(args, client, clock, installer).await?;
        print_summary(&summary);
        return Ok(());
    }

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

    let daemon = daemon.ask()?;
    let args = SetupArgs {
        vault,
        password_file: keep_password(&config_path, &daemon, &password)?,
        config_path,
        url,
        username,
        password_env: None,
        inbox_collection: Some(inbox),
        collections: Vec::new(),
        join: false,
        daemon,
        password: Some(Secret::new(password)),
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

    // A node is looked at before anything is written here: a setup that cannot place
    // its daemon leaves the vault as it found it. The daemon already running there is
    // stopped, so this machine's first sync is the only pass while the vault changes.
    prepare_node(installer, &args)?;

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
    let machine = machine_config(&args);
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
    if args.daemon == DaemonHost::Here {
        save_machine(&machine, &args.config_path)?;
    }

    // Step 5 — first sync: registers fresh lines, pulls tasks from the bound calendar.
    // It is made from here also when the daemon runs elsewhere: the vault leaves this
    // machine converged, and the node's daemon finds nothing to do in it.
    let engine = Engine::new(&vault, cfg, machine.clone(), caldav, clock);
    let report = engine.reconcile().await?;

    // Step 6 — daemon (§13.2), so the vault keeps syncing without manual steps.
    let daemon = place_daemon(installer, &machine, &args)?;

    Ok(SetupSummary {
        vault,
        config_path: args.config_path,
        joined: false,
        backup,
        collections,
        plugin,
        daemon,
        synced: true,
        report,
    })
}

/// Step 6 of both setup bodies: puts the daemon where [`SetupArgs::daemon`] says and, on
/// a machine that only edits, records that in the machine config.
///
/// On this machine a failed unit install is a warning — the sync already happened and
/// `restask doctor` diagnoses the environment. On a node a failure is the run's error:
/// without the daemon nothing syncs the vault. The machine config of an editing machine
/// is written only after the daemon is in place, so a run that failed is repeated, not
/// taken for done.
fn place_daemon(
    installer: Option<&dyn DaemonInstaller>,
    machine: &MachineConfig,
    args: &SetupArgs,
) -> Result<Option<String>, RestaskError> {
    let note = match &args.daemon {
        DaemonHost::Here => return Ok(install_daemon(installer, &args.vault)),
        DaemonHost::Node(node) => match installer {
            Some(installer) => {
                let access = NodeAccess {
                    url: &args.url,
                    username: &args.username,
                    password: args.password.as_ref().ok_or(RestaskError::Validation {
                        field: "node",
                        reason: "no password to hand to the node's daemon".to_string(),
                    })?,
                };
                Some(
                    installer
                        .install_node(node, &args.vault, access)
                        .map_err(|error| node_error(node, &error))?,
                )
            }
            None => None,
        },
        DaemonHost::Elsewhere => Some(
            "daemon: not installed here (--no-daemon). Install it on the always-on machine \
             with `restask setup --join` in its copy of the vault; a unit from an earlier \
             setup keeps running here until `systemctl --user disable --now restask`"
                .to_string(),
        ),
    };
    save_machine(machine, &args.config_path)?;
    forget_password(&args.config_path)?;
    Ok(note)
}

/// Runs [`DaemonInstaller::prepare_node`] when the daemon goes to a node. A URL that
/// names this machine itself is refused first: the node could not reach it.
fn prepare_node(
    installer: Option<&dyn DaemonInstaller>,
    args: &SetupArgs,
) -> Result<(), RestaskError> {
    let DaemonHost::Node(node) = &args.daemon else {
        return Ok(());
    };
    if is_loopback(&args.url) {
        return Err(RestaskError::Validation {
            field: "url",
            reason: format!(
                "{} is this machine itself; the daemon on {} needs the address the server \
                 has on the network (e.g. http://192.168.1.10:5232)",
                args.url, node.host
            ),
        });
    }
    match installer {
        Some(installer) => installer.prepare_node(node),
        None => Ok(()),
    }
}

/// Whether `url` names the local machine (`localhost`, `127.x.x.x`, `[::1]`).
pub fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.") || host == "::1"
}

/// The error of a node install that failed, with the way to finish it: the vault is set
/// up by then, so the same command with `--join` installs the daemon and nothing else.
fn node_error(node: &NodeTarget, error: &RestaskError) -> RestaskError {
    RestaskError::Validation {
        field: "node",
        reason: format!(
            "the daemon is not running on {host} yet: {error}. Nothing syncs the vault \
             until it does. When the cause is fixed, finish with `restask setup --join \
             --node {host} --node-vault \"{vault}\" --node-dir \"{dir}\"`",
            host = node.host,
            vault = node.vault,
            dir = node.dir,
        ),
    }
}

/// Writes the machine config (0600), creating its directory: on a machine restask was
/// never set up on there is none, and only the sync node's password file made one.
fn save_machine(machine: &MachineConfig, path: &Path) -> Result<(), RestaskError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| config_error(path, error))?;
    }
    machine
        .save(path)
        .map_err(|error| config_error(path, &error))
}

/// The machine config a setup run records. On the sync node: this vault, the endpoint,
/// and where the password is found — never the password (§17). On a machine that only
/// edits: this vault and where its daemon is, with no way to reach the server (§14.2).
fn machine_config(args: &SetupArgs) -> MachineConfig {
    let vault = VaultSection {
        path: Some(args.vault.clone()),
    };
    let node = |target: Option<&NodeTarget>| NodeSection {
        host: target.map(|node| node.host.clone()),
        dir: target.map(|node| node.dir.clone()),
        vault: target.map(|node| node.vault.clone()),
        url: Some(args.url.clone()),
        username: Some(args.username.clone()),
    };
    match &args.daemon {
        DaemonHost::Here => MachineConfig {
            vault,
            caldav: CaldavConfig {
                url: Some(args.url.clone()),
                username: Some(args.username.clone()),
                password_env: args.password_env.clone(),
                password_file: args.password_file.clone(),
                ..CaldavConfig::default()
            },
            node: None,
        },
        DaemonHost::Node(target) => MachineConfig {
            vault,
            caldav: CaldavConfig::default(),
            node: Some(node(Some(target))),
        },
        DaemonHost::Elsewhere => MachineConfig {
            vault,
            caldav: CaldavConfig::default(),
            node: Some(node(None)),
        },
    }
}

/// Setup body for a vault that is already set up (§13.2 *Joining*): the machine-local
/// half only. The vault is handed to the engine as it is — no scaffold, no fresh TODO.md,
/// no plugin install, no collection created by hand — so nothing here reaches the other
/// devices except what an ordinary pass does.
///
/// A machine that joins as the sync node makes the first sync. One that joins a vault
/// whose daemon runs elsewhere makes none — the pass is the node's, and a second machine
/// passing over its own copy would race the file sync (§1.1): it proves the credentials,
/// has the daemon installed on the node when asked to, and records where it is.
///
/// The machine config is written last. A join that fails (server unreachable, wrong
/// credentials, a vault the pass refuses) leaves the machine without one, so running
/// setup again joins again instead of setting the vault up afresh.
async fn join_and_sync<C: CaldavPort>(
    args: SetupArgs,
    caldav: C,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
) -> Result<SetupSummary, RestaskError> {
    let vault = args.vault.clone();
    let config_path = vault.join("restask.toml");
    let cfg =
        VaultConfig::load(&config_path).map_err(|error| config_error(&config_path, &error))?;
    let machine = machine_config(&args);
    prepare_node(installer, &args)?;

    // Endpoint and credentials are proven before anything is recorded.
    caldav.list_collections().await?;
    let synced = args.daemon == DaemonHost::Here;
    let report = if synced {
        let engine = Engine::new(&vault, cfg, machine.clone(), caldav, clock);
        let report = engine.reconcile().await?;
        save_machine(&machine, &args.config_path)?;
        report
    } else {
        ReconcileReport::default()
    };
    let daemon = place_daemon(installer, &machine, &args)?;

    Ok(SetupSummary {
        vault,
        config_path: args.config_path,
        joined: true,
        backup: None,
        collections: Vec::new(),
        plugin: None,
        daemon,
        synced,
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

/// Installs the vault's daemon (§13.2 step 6): the machine-local systemd user unit that
/// keeps `restask daemon` running on this vault, or the daemon on an always-on server.
/// A port so tests record instead of touching the host's systemd session or the network.
pub trait DaemonInstaller {
    /// Writes and enables the unit. `Ok(None)` means this environment cannot host a user
    /// unit (no systemd session) — setup continues without one.
    ///
    /// `vault` is the folder the unit points at; `exec` is the `restask` binary to run.
    fn install(&self, vault: &Path, exec: &Path) -> Result<Option<String>, RestaskError>;

    /// Checks that `node` can run the daemon — it is reachable, has what the install
    /// needs, holds the vault's folder — and stops a daemon already running there. Called
    /// before setup writes anything, so that no other pass runs while it does.
    fn prepare_node(&self, node: &NodeTarget) -> Result<(), RestaskError> {
        Err(no_node_support(node))
    }

    /// Installs and starts the daemon on `node`, configured with `access`, once the file
    /// sync has delivered `vault` (this machine's copy) there unchanged. Returns the
    /// summary note.
    fn install_node(
        &self,
        node: &NodeTarget,
        vault: &Path,
        access: NodeAccess<'_>,
    ) -> Result<String, RestaskError> {
        let _ = (vault, access);
        Err(no_node_support(node))
    }
}

/// The answer of an installer that reaches no other machine.
fn no_node_support(node: &NodeTarget) -> RestaskError {
    RestaskError::Validation {
        field: "node",
        reason: format!("this installer cannot install on {}", node.host),
    }
}

/// Real [`DaemonInstaller`]. On this machine: writes `restask.service` into
/// `$XDG_CONFIG_HOME/systemd/user/`, runs `systemctl --user daemon-reload`,
/// `systemctl --user enable --now restask.service`, and enables linger so the unit
/// survives logout; re-running setup overwrites the unit (idempotent refresh). On a
/// node: runs `contrib/node.sh` of the restask sources ([`node_script`]), which drives
/// the server over ssh (App. E).
pub struct SystemInstaller;

impl DaemonInstaller for SystemInstaller {
    fn prepare_node(&self, node: &NodeTarget) -> Result<(), RestaskError> {
        let script = node_script()?;
        println!("checking {} …", node.host);
        run_script(
            &script,
            &["check", &node.host, &node.vault, &node.dir],
            None,
        )
    }

    fn install_node(
        &self,
        node: &NodeTarget,
        vault: &Path,
        access: NodeAccess<'_>,
    ) -> Result<String, RestaskError> {
        let script = node_script()?;
        // The script works from the sources' directory: a vault given relative to this
        // process's working directory would name another folder there.
        let vault = std::path::absolute(vault)?.display().to_string();
        // The script reads the three values from its standard input, one per line: the
        // password is in no argument list and no file.
        let input = format!(
            "{}\n{}\n{}\n",
            access.url,
            access.username,
            access.password.expose()
        );
        run_script(
            &script,
            &["install", &node.host, &node.vault, &node.dir, &vault],
            Some(&input),
        )?;
        Ok(format!(
            "daemon: running on {} (stack {}, vault {}); this computer runs none and \
             keeps no password",
            node.host, node.dir, node.vault
        ))
    }

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

/// Best-effort daemon-unit install on this machine (§13.2 step 6): no installer, an
/// unresolvable binary or an installer error all degrade to a warning — never fail the
/// setup run.
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

/// `contrib/node.sh` of the restask sources: the script that installs the daemon on a
/// node, and the tree it builds the daemon from there (a node has its own architecture,
/// so this machine's binary is of no use to it). The sources are the ones named by
/// `RESTASK_SOURCE`, else the clone this binary was built from.
pub fn node_script() -> Result<PathBuf, RestaskError> {
    let source = std::env::var_os("RESTASK_SOURCE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let script = source.join("contrib").join("node.sh");
    if script.is_file() {
        Ok(script)
    } else {
        Err(RestaskError::Validation {
            field: "node",
            reason: format!(
                "the restask sources are needed to build the daemon on the server, and {} \
                 is not there: set RESTASK_SOURCE to a clone of the repository",
                script.display()
            ),
        })
    }
}

/// Runs `contrib/node.sh` with `args`, its output going to the terminal, `input` to its
/// standard input.
fn run_script(script: &Path, args: &[&str], input: Option<&str>) -> Result<(), RestaskError> {
    use std::io::Write as _;
    let mut child = Command::new("bash")
        .arg(script)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{} {} failed: {status}",
            script.display(),
            args.first().copied().unwrap_or_default()
        ))
        .into())
    }
}

/// The machine's password file: `radicale.passwd` beside the machine config.
fn password_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("radicale.passwd")
}

/// Stores `password` in the machine's password file (0600, §17) and returns its path:
/// what the sync node's daemon reads it from.
pub fn store_password(config_path: &Path, password: &str) -> Result<PathBuf, RestaskError> {
    let path = password_path(config_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_secret(&path, password)?;
    Ok(path)
}

/// [`store_password`] when this machine runs the daemon; a machine that only edits
/// stores nothing.
fn keep_password(
    config_path: &Path,
    daemon: &DaemonHost,
    password: &str,
) -> Result<Option<PathBuf>, RestaskError> {
    match daemon {
        DaemonHost::Here => store_password(config_path, password).map(Some),
        DaemonHost::Node(_) | DaemonHost::Elsewhere => Ok(None),
    }
}

/// Removes the password file an earlier setup left on a machine that now only edits: its
/// config no longer names it, and a secret nothing reads should not stay on disk (§17).
fn forget_password(config_path: &Path) -> Result<(), RestaskError> {
    match std::fs::remove_file(password_path(config_path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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
    if summary.joined {
        println!(
            "joined the vault at {} (already set up; nothing in it was replaced)",
            summary.vault.display()
        );
    }
    if let Some(backup) = &summary.backup {
        println!("renamed existing TODO.md to {backup}");
    }
    println!("machine config: {}", summary.config_path.display());
    if !summary.collections.is_empty() {
        println!("collections: {}", summary.collections.join(", "));
    }
    if summary.synced {
        println!(
            "first sync: scanned {} registered {} pushed {}",
            summary.report.scanned_files, summary.report.registered, summary.report.pushes
        );
    }
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
