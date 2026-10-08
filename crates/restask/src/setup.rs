//! Setup wizard (§13.2): composes the vault scaffold (with the Obsidian plugin installed
//! and enabled in it — and loaded by an Obsidian that has the vault open, [`ObsidianApp`]),
//! the fresh TODO.md creation (any pre-existing file is renamed to the
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
use std::sync::{Arc, Mutex};

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

    /// [`DaemonFlags::resolve`] for the interactive wizard: with no flag it asks, through
    /// `prompt`, whether an always-on server holds the vault, and for the vault's folder
    /// there.
    ///
    /// The server is connected to as soon as it is named
    /// ([`DaemonInstaller::connect_node`]), so whatever ssh needs typed — a password, a
    /// key's passphrase — is asked right after the host, once, and before any other
    /// question. A typed host that cannot be reached is asked for again; one given by
    /// `--node` is the run's error.
    pub fn ask(
        self,
        installer: Option<&dyn DaemonInstaller>,
        prompt: &mut dyn FnMut(&str) -> Result<String, RestaskError>,
    ) -> Result<DaemonHost, RestaskError> {
        if self.no_daemon && self.node.is_none() {
            return Ok(DaemonHost::Elsewhere);
        }
        let connect = |host: &str| match installer {
            Some(installer) => installer.connect_node(host),
            None => Ok(()),
        };
        let host = match self.node {
            Some(host) => {
                connect(&host)?;
                host
            }
            None => {
                println!(
                    "\nOne machine that is always on keeps the vault and the server in sync. \
                     If a server holds a copy of this vault (through the file sync), restask \
                     is installed there now, over ssh, with the credentials above."
                );
                loop {
                    let host = prompt("ssh host of that server (Enter: this computer does it):")?;
                    if host.is_empty() {
                        return Ok(DaemonHost::Here);
                    }
                    match connect(&host) {
                        Ok(()) => break host,
                        Err(error) => println!("{error}"),
                    }
                }
            }
        };
        let vault = match self.node_vault {
            Some(vault) => vault,
            None => loop {
                let typed = prompt(&format!("Vault folder on {host}:"))?;
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
    /// Further calendars whose tasks TODO.md shows, recorded as `vault.todo_lists`
    /// (§7.5, §14.1). Interactive setup always sets it (to none, when one calendar was
    /// chosen); non-interactive setup takes it from `--todo-list` flags. `None` leaves
    /// what the vault's `restask.toml` says.
    pub todo_collections: Option<Vec<String>>,
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
            todo_collections: None,
            collections,
            join,
            daemon: DaemonHost::Here,
            password: None,
        })
    }

    /// Adds the calendars of the repeatable `--todo-list` flag (§13.2): the further
    /// calendars whose tasks TODO.md shows. None given leaves the vault's choice as it
    /// is. A join takes none: the vault it joins already says which.
    pub fn showing(mut self, calendars: Vec<String>) -> Result<Self, RestaskError> {
        if calendars.is_empty() {
            return Ok(self);
        }
        if self.join {
            return Err(RestaskError::Validation {
                field: "todo-list",
                reason: "--todo-list has no meaning when joining a vault that is already \
                         set up: its restask.toml names the calendars TODO.md shows"
                    .to_string(),
            });
        }
        for calendar in &calendars {
            ListSlug::from_name(calendar)?;
        }
        self.todo_collections = Some(calendars);
        Ok(self)
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

/// What one setup run did (§13.2 step 8 summary inputs).
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
    /// What the Obsidian plugin install left in the vault (§13.2 step 1); `None` when
    /// it failed (a warning was logged), and for a join, which installs none.
    pub plugin: Option<PluginInstall>,
    /// How the plugin got into an Obsidian that was running (§13.2 step 7); set by
    /// [`SetupSummary::load_plugin`].
    pub plugin_load: PluginLoad,
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

impl SetupSummary {
    /// Step 7 (§13.2), after everything else is in place: when this run put something
    /// new of the plugin into the vault and an Obsidian on this computer has the vault
    /// open, that Obsidian is made to load it ([`load_obsidian_plugin`]). `ask` is the
    /// wizard's yes/no question, for the restart; an unattended run passes `None`.
    pub fn load_plugin(&mut self, app: &dyn ObsidianApp, ask: Option<Confirm<'_>>) {
        if self
            .plugin
            .is_some_and(|install| install.listed && install.changed)
        {
            self.plugin_load = load_obsidian_plugin(app, &self.vault, ask);
        }
    }

    /// The summary line about the Obsidian plugin, when it was installed.
    pub fn plugin_note(&self) -> Option<String> {
        self.plugin
            .map(|install| plugin_note(install, self.plugin_load))
    }
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
/// ([`DaemonFlags::ask`]; a server is connected to as soon as it is named, so ssh asks
/// what it needs there): the password typed once goes to this machine's password file when
/// the daemon runs here, and to the node — and nowhere on this machine — when it runs
/// there. When all is in place, an Obsidian that has the vault open is made to load the
/// plugin, through `app` ([`SetupSummary::load_plugin`]; `None` leaves it alone).
pub async fn run_interactive(
    vault: PathBuf,
    config_path: PathBuf,
    join: bool,
    daemon: DaemonFlags,
    clock: Arc<dyn Clock>,
    installer: Option<&dyn DaemonInstaller>,
    app: Option<&dyn ObsidianApp>,
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
        let daemon = daemon.ask(installer, &mut |message| crate::tui::prompt(message))?;
        let args = SetupArgs {
            vault,
            password_file: keep_password(&config_path, &daemon, &password)?,
            config_path,
            url,
            username,
            password_env: None,
            inbox_collection: None,
            todo_collections: None,
            collections: Vec::new(),
            join: true,
            daemon,
            password: Some(Secret::new(password)),
        };
        let summary = join_and_sync(args, client, clock, installer).await?;
        print_summary(&summary);
        return Ok(());
    }

    // Step 4 (§13.2): the calendars TODO.md shows, chosen by name from the server's
    // list, and the one of them a task typed there without a calendar goes to. Lists
    // that live in notes are declared by hand with `restask-list` frontmatter; no
    // per-list wizard probing happens.
    let server_collections = client.list_collections().await?;
    if server_collections.is_empty() {
        return Err(RestaskError::Validation {
            field: "collections",
            reason: "no calendars found on the server; create one and re-run `restask setup` \
                     — binding TODO.md is required for sync"
                .to_string(),
        });
    }
    let offered = offered_collections(&server_collections);
    // At a terminal the calendars are a checklist, all of them ticked; the typed
    // questions remain for piped input and for a server with nothing to tick.
    let pick = crate::tui::interactive() && !offered.is_empty();
    let names: Vec<&str> = server_collections
        .iter()
        .map(|collection| collection.slug.as_str())
        .collect();
    let shown = if pick {
        let labels: Vec<String> = offered
            .iter()
            .map(|collection| collection_label(collection))
            .collect();
        let ticked = vec![true; offered.len()];
        loop {
            let picked = crate::tui::pick_many(
                "Calendars TODO.md shows (space: tick or untick, a: all, Enter: confirm)",
                &labels,
                &ticked,
            )?;
            if picked.is_empty() {
                println!("TODO.md needs at least one calendar.");
                continue;
            }
            break picked
                .into_iter()
                .filter_map(|at| offered.get(at).copied())
                .collect::<Vec<_>>();
        }
    } else {
        println!("Server calendars: {}", names.join(", "));
        loop {
            let typed = crate::tui::prompt(
                "Calendars TODO.md shows (comma-separated, Enter for all of them):",
            )?;
            match select_collections(&typed, &server_collections) {
                Ok(shown) => break shown,
                Err(miss) => println!("`{miss}` is not one of: {}", names.join(", ")),
            }
        }
    };
    let inbox = match shown.as_slice() {
        [only] => only.slug.clone(),
        _ if pick => {
            let labels: Vec<String> = shown
                .iter()
                .map(|collection| collection_label(collection))
                .collect();
            let at = crate::tui::pick_one("New tasks typed in TODO.md go to", &labels)?;
            match shown.get(at) {
                Some(collection) => collection.slug.clone(),
                None => {
                    return Err(RestaskError::Validation {
                        field: "collections",
                        reason: "no calendar chosen for new tasks".to_string(),
                    })
                }
            }
        }
        _ => {
            let shown_names: Vec<&str> = shown
                .iter()
                .map(|collection| collection.slug.as_str())
                .collect();
            loop {
                let typed = crate::tui::prompt(&format!(
                    "New tasks typed in TODO.md go to (one of: {}):",
                    shown_names.join(", ")
                ))?;
                match shown
                    .iter()
                    .find(|collection| collection.slug.eq_ignore_ascii_case(typed.trim()))
                {
                    Some(collection) => break collection.slug.clone(),
                    None => println!("`{typed}` is not one of: {}", shown_names.join(", ")),
                }
            }
        }
    };
    let others: Vec<String> = shown
        .iter()
        .filter(|collection| collection.slug != inbox)
        .map(|collection| collection.slug.clone())
        .collect();

    let daemon = daemon.ask(installer, &mut |message| crate::tui::prompt(message))?;
    let args = SetupArgs {
        vault,
        password_file: keep_password(&config_path, &daemon, &password)?,
        config_path,
        url,
        username,
        password_env: None,
        inbox_collection: Some(inbox),
        todo_collections: Some(others),
        collections: Vec::new(),
        join: false,
        daemon,
        password: Some(Secret::new(password)),
    };
    let mut summary = prepare_and_sync(args, client, clock, installer).await?;
    if let Some(app) = app {
        summary.load_plugin(app, Some(&mut |message| crate::tui::confirm_yes(message)));
    }
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

/// The calendars the wizard offers for TODO.md (§13.2 step 4): those of the server that
/// can hold tasks, in the server's order. All of them are ticked when the list opens.
pub fn offered_collections(collections: &[CollectionInfo]) -> Vec<&CollectionInfo> {
    collections
        .iter()
        .filter(|collection| collection.supports_vtodo)
        .collect()
}

/// How the wizard's list names a calendar: by its slug, the name `restask.toml` and a
/// `📁` token use — with the name other clients show in front when that is another one
/// (a calendar made in a CalDAV client has a path the user never saw).
pub fn collection_label(collection: &CollectionInfo) -> String {
    match &collection.display_name {
        Some(name) if !name.trim().eq_ignore_ascii_case(&collection.slug) => {
            format!("{} ({})", name.trim(), collection.slug)
        }
        _ => collection.slug.clone(),
    }
}

/// The calendars TODO.md shows, as typed in step 4 of the wizard (§13.2): names of the
/// server's calendars separated by commas, matched like [`match_collection`], each once
/// and in the order typed; nothing typed means all of them that can hold tasks. `Err`
/// carries the first name that is not a calendar of the server, and the wizard asks
/// again.
pub fn select_collections<'a>(
    typed: &str,
    collections: &'a [CollectionInfo],
) -> Result<Vec<&'a CollectionInfo>, String> {
    let mut chosen: Vec<&CollectionInfo> = Vec::new();
    for name in typed
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let found = match_collection(name, collections).ok_or_else(|| name.to_string())?;
        if !chosen.iter().any(|held| held.slug == found.slug) {
            chosen.push(found);
        }
    }
    if chosen.is_empty() {
        return Ok(offered_collections(collections));
    }
    Ok(chosen)
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
    // The further calendars TODO.md shows (§7.5) travel the same way, as slugs, each
    // once and without the one the file is bound to.
    if let Some(calendars) = &args.todo_collections {
        let mut shown: Vec<String> = Vec::new();
        for calendar in calendars {
            let slug = ListSlug::from_name(calendar)?.as_str().to_string();
            if slug != cfg.inbox_list && !shown.contains(&slug) {
                shown.push(slug);
            }
        }
        if cfg.todo_lists != shown {
            cfg.todo_lists = shown;
            cfg.save(&config_path)
                .map_err(|error| config_error(&config_path, &error))?;
        }
    }
    // Obsidian knows a vault by its folder's name; the links that open a task's notes
    // from another client need it (§8.4), and the sync node sees the folder under
    // another name. Recorded once, so a name the user corrected stays.
    if cfg.obsidian_vault.is_none() {
        let name = std::fs::canonicalize(&vault)?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if name.is_some() {
            cfg.obsidian_vault = name;
            cfg.save(&config_path)
                .map_err(|error| config_error(&config_path, &error))?;
        }
    }
    std::fs::create_dir_all(vault.join(".restask"))?;

    // The vault is set: put the Obsidian plugin in it. Best-effort like the daemon unit —
    // the plugin is a convenience (§15) and a vault nobody opens in Obsidian loses nothing.
    let plugin = match install_obsidian_plugin(&vault, &cfg) {
        Ok(install) => Some(install),
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
    if args.todo_collections.is_some() {
        for slug in crate::vault::todo_lists(&cfg)? {
            caldav
                .ensure_collection(&slug, &slug.display_name())
                .await?;
            collections.push(slug.as_str().to_string());
        }
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
        plugin_load: PluginLoad::default(),
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
        plugin_load: PluginLoad::default(),
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

/// What [`install_obsidian_plugin`] left in a vault: the input of the summary line, and
/// of the step that gets the plugin into an Obsidian that is already running
/// ([`SetupSummary::load_plugin`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginInstall {
    /// Whether the vault's list of enabled plugins names the plugin. `false`: the list
    /// is not one restask can read, and was left as it is.
    pub listed: bool,
    /// Whether this run wrote something Obsidian reads when it opens the vault — a file
    /// of the plugin, or its entry in the list. An Obsidian that has the vault open has
    /// not seen it.
    pub changed: bool,
}

/// Installs the Obsidian plugin into `<vault>/.obsidian/plugins/restask/` and lists it in
/// `.obsidian/community-plugins.json` (§13.2 step 1), so the vault opens in Obsidian with
/// the plugin on — on every device the vault is synced to.
///
/// Re-running refreshes the three plugin files and nothing else: the user's plugin
/// settings and every other enabled plugin are kept, and a file that already has the
/// right content is not rewritten. A plugin folder or file that is a symlink is managed
/// by hand (a development checkout) and left alone. A `community-plugins.json` that is
/// not a JSON list is never overwritten; the summary then asks to enable the plugin by
/// hand.
pub fn install_obsidian_plugin(
    vault: &Path,
    cfg: &VaultConfig,
) -> Result<PluginInstall, RestaskError> {
    let obsidian = vault.join(".obsidian");
    let plugin_dir = obsidian.join("plugins").join(OBSIDIAN_PLUGIN_ID);
    let mut changed = false;
    if !is_symlink(&plugin_dir) {
        std::fs::create_dir_all(&plugin_dir)?;
        for (name, contents) in OBSIDIAN_PLUGIN_FILES {
            let path = plugin_dir.join(name);
            if !is_symlink(&path) {
                changed |= fsio::write_if_changed(&path, contents)?;
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

    Ok(match enable_obsidian_plugin(&obsidian)? {
        None => PluginInstall {
            listed: false,
            changed,
        },
        Some(added) => PluginInstall {
            listed: true,
            changed: changed || added,
        },
    })
}

/// Adds the plugin id to `.obsidian/community-plugins.json`, keeping every other entry in
/// place. `Ok(None)` means the file holds something other than a JSON list and was left
/// untouched; `Ok(Some(added))` says whether the entry was written by this call — a file
/// that already lists the plugin is not rewritten.
fn enable_obsidian_plugin(obsidian: &Path) -> Result<Option<bool>, RestaskError> {
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
            Err(_) => return Ok(None),
        }
    };
    if enabled
        .iter()
        .any(|id| id.as_str() == Some(OBSIDIAN_PLUGIN_ID))
    {
        return Ok(Some(false));
    }
    enabled.push(serde_json::Value::from(OBSIDIAN_PLUGIN_ID));
    fsio::write_atomic(&path, &format!("{:#}", serde_json::Value::Array(enabled)))?;
    Ok(Some(true))
}

/// Whether `path` itself is a symbolic link (not followed).
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// A yes/no question put to the user, as the wizard asks it on the terminal.
pub type Confirm<'a> = &'a mut dyn FnMut(&str) -> Result<bool, RestaskError>;

/// The Obsidian app of this computer, as far as setup deals with it (§13.2 step 7): the
/// one that may have the vault open while setup installs the plugin. Obsidian reads a
/// vault's plugins when it opens the vault and never again, so a plugin installed under
/// a running Obsidian is not on until Obsidian is told, or started again. A port so
/// tests record instead of reaching for the user's app.
pub trait ObsidianApp {
    /// Whether an Obsidian that is running on this computer has `vault` open.
    fn has_open(&self, vault: &Path) -> bool;

    /// Sends one command of Obsidian's command line interface (`plugin:enable`,
    /// `reload`, …) to the window of `vault` and returns the answer. `Ok(None)`: the
    /// interface is switched off in that Obsidian (its Settings → General).
    fn command(&self, vault: &Path, args: &[&str]) -> Result<Option<String>, RestaskError>;

    /// Closes Obsidian and starts it again the way it was started; it then opens the
    /// vaults it had open by itself.
    fn restart(&self) -> Result<(), RestaskError>;
}

/// How the plugin a setup run installed got into an Obsidian that was already running
/// (§13.2 step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginLoad {
    /// Nothing was done, and as far as setup can tell nothing had to be: the run wrote
    /// nothing new, or no running Obsidian has the vault open — Obsidian loads the
    /// plugin when it opens the vault.
    #[default]
    NotNeeded,
    /// Obsidian loaded the plugin when asked, through its command line interface.
    Loaded,
    /// Obsidian reloaded the vault's window when asked, through its command line
    /// interface, and read the vault's plugins again.
    Reloaded,
    /// Obsidian was closed and started again, with the user's consent.
    Restarted,
    /// The vault is in restricted mode on this computer: Obsidian loads no community
    /// plugin there until the user turns it off. Setup does not decide that for them.
    Restricted,
    /// Obsidian has the vault open and still runs without the plugin: the restart was
    /// declined, could not be asked for, or failed.
    Pending,
}

/// The answer Obsidian gives to every command while its command line interface is off.
const OBSIDIAN_CLI_OFF: &str = "Command line interface is not enabled";

/// Gets the plugin setup just installed into an Obsidian that has `vault` open (§13.2
/// step 7). Through Obsidian's own command line interface when that is on — nothing
/// closes. Otherwise `ask` is asked whether Obsidian may be restarted; without an `ask`
/// (an unattended run) Obsidian is left running. Nothing here fails setup: what could
/// not be done is [`PluginLoad::Pending`].
pub fn load_obsidian_plugin(
    app: &dyn ObsidianApp,
    vault: &Path,
    ask: Option<Confirm<'_>>,
) -> PluginLoad {
    if !app.has_open(vault) {
        return PluginLoad::NotNeeded;
    }
    match app.command(vault, &["plugins:restrict"]) {
        Ok(Some(restricted)) => load_through_cli(app, vault, &restricted),
        Ok(None) => load_by_restart(app, ask),
        Err(error) => {
            tracing::warn!(%error, "obsidian did not answer; restart it to load the plugin");
            PluginLoad::Pending
        }
    }
}

/// [`load_obsidian_plugin`] with the command line interface on. `restricted` is the
/// answer to `plugins:restrict` (`on` / `off`). The gentlest command that works is the
/// one used: reload the plugin (it was on, its files are new), enable it (Obsidian knows
/// its folder, it was off), reload the window (the folder is new to this Obsidian).
fn load_through_cli(app: &dyn ObsidianApp, vault: &Path, restricted: &str) -> PluginLoad {
    if restricted.trim() == "on" {
        return PluginLoad::Restricted;
    }
    let id = format!("id={OBSIDIAN_PLUGIN_ID}");
    let says = |args: &[&str], done: &str| matches!(app.command(vault, args), Ok(Some(answer)) if answer.trim_start().starts_with(done));
    if says(&["plugin:reload", &id], "Reloaded")
        || says(&["plugin:enable", &id, "filter=community"], "Enabled")
    {
        PluginLoad::Loaded
    } else if says(&["reload"], "Reloading") {
        PluginLoad::Reloaded
    } else {
        PluginLoad::Pending
    }
}

/// [`load_obsidian_plugin`] with the command line interface off: Obsidian has to start
/// again, and that closes the user's windows — so only when `ask` says yes.
fn load_by_restart(app: &dyn ObsidianApp, ask: Option<Confirm<'_>>) -> PluginLoad {
    let Some(ask) = ask else {
        return PluginLoad::Pending;
    };
    let agreed = ask(
        "Obsidian has this vault open, and it loads a new plugin only when it starts. \
         Restart Obsidian now?",
    );
    if !matches!(agreed, Ok(true)) {
        return PluginLoad::Pending;
    }
    match app.restart() {
        Ok(()) => PluginLoad::Restarted,
        Err(error) => {
            println!("Obsidian was not restarted: {error}");
            PluginLoad::Pending
        }
    }
}

/// The summary line about the plugin: where it is, and whether an Obsidian that was
/// running has it by now.
fn plugin_note(install: PluginInstall, load: PluginLoad) -> String {
    let location = format!(".obsidian/plugins/{OBSIDIAN_PLUGIN_ID}");
    if !install.listed {
        return format!(
            "obsidian plugin: installed in {location}; .obsidian/community-plugins.json is \
             not a list, so enable \"{OBSIDIAN_PLUGIN_ID}\" under Community plugins by hand"
        );
    }
    let running = match load {
        PluginLoad::NotNeeded if install.changed => " (restart Obsidian if this vault is open)",
        PluginLoad::NotNeeded => "",
        PluginLoad::Loaded => "; the Obsidian that has this vault open loaded it",
        PluginLoad::Reloaded => "; Obsidian reloaded this vault's window and loaded it",
        PluginLoad::Restarted => "; Obsidian was restarted and loaded it",
        PluginLoad::Restricted => {
            "; Obsidian is in restricted mode for this vault and loads no community plugin: \
             turn that off under Settings → Community plugins"
        }
        PluginLoad::Pending => {
            "; Obsidian has this vault open and loads the plugin when it is restarted"
        }
    };
    format!("obsidian plugin: installed in {location} and enabled{running}")
}

/// Real [`ObsidianApp`]: the Obsidian of this user, found through the list of vaults it
/// keeps in its own configuration folder and the socket its command line interface
/// listens on while it runs. Restarting it is done on Linux only.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemObsidian;

impl ObsidianApp for SystemObsidian {
    fn has_open(&self, vault: &Path) -> bool {
        obsidian_window(vault).is_some()
    }

    fn command(&self, vault: &Path, args: &[&str]) -> Result<Option<String>, RestaskError> {
        let Some((id, socket)) = obsidian_window(vault) else {
            return Err(obsidian_error("no running Obsidian has this vault open"));
        };
        let answer = obsidian_cli_request(&socket, &id, vault, args)?;
        Ok(Some(answer).filter(|answer| !answer.starts_with(OBSIDIAN_CLI_OFF)))
    }

    fn restart(&self) -> Result<(), RestaskError> {
        #[cfg(target_os = "linux")]
        {
            // Obsidian is started again in this terminal's environment: one that is
            // not in the desktop session (ssh) could close it, but not show it again.
            let in_session = ["WAYLAND_DISPLAY", "DISPLAY"]
                .iter()
                .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
            if !in_session {
                return Err(obsidian_error(
                    "this terminal is not in the desktop session Obsidian runs in",
                ));
            }
            let pid = obsidian_pid()?;
            println!("restarting Obsidian …");
            restart_process(pid).map(|_| ())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(obsidian_error(
                "restarting Obsidian is not supported on this system",
            ))
        }
    }
}

/// An error of the dealings with the running Obsidian.
fn obsidian_error(reason: impl Into<String>) -> RestaskError {
    RestaskError::Validation {
        field: "obsidian",
        reason: reason.into(),
    }
}

/// The id under which the Obsidian whose list of vaults is `list` (the text of its
/// `obsidian.json`) has `vault` open in a window. `None`: it does not know the folder,
/// or has no window on it. (The mark also survives a quit, for the next start; whether
/// that Obsidian runs is the caller's question.)
pub fn open_vault_id(list: &str, vault: &Path) -> Option<String> {
    let list: serde_json::Value = serde_json::from_str(list).ok()?;
    let same = |path: &str| {
        let path = Path::new(path);
        path == vault || std::fs::canonicalize(path).is_ok_and(|path| path == vault)
    };
    list.get("vaults")?
        .as_object()?
        .iter()
        .find(|(_, entry)| {
            entry.get("open").and_then(serde_json::Value::as_bool) == Some(true)
                && entry
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(same)
        })
        .map(|(id, _)| id.clone())
}

/// Where an Obsidian install keeps its list of vaults (`obsidian.json` in the first
/// path) and where it listens for its command line interface (the second): the app
/// installed the ordinary way, and on Linux the Flatpak.
fn obsidian_homes() -> Vec<(PathBuf, PathBuf)> {
    let dir = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let Some(home) = dir("HOME") else {
        return Vec::new();
    };
    const SOCKET: &str = ".obsidian-cli.sock";
    if cfg!(target_os = "macos") {
        return vec![(
            home.join("Library/Application Support/obsidian"),
            home.join(SOCKET),
        )];
    }
    let runtime = dir("XDG_RUNTIME_DIR");
    let mut homes = vec![(
        xdg_config_home().join("obsidian"),
        runtime.clone().unwrap_or_else(|| home.clone()).join(SOCKET),
    )];
    if let Some(runtime) = runtime {
        const FLATPAK: &str = "md.obsidian.Obsidian";
        homes.push((
            home.join(".var/app").join(FLATPAK).join("config/obsidian"),
            runtime
                .join(".flatpak")
                .join(FLATPAK)
                .join("xdg-run")
                .join(SOCKET),
        ));
    }
    homes
}

/// The running Obsidian that has `vault` open: the id it knows the vault by and the
/// socket of its command line interface. It runs if the socket takes a connection.
fn obsidian_window(vault: &Path) -> Option<(String, PathBuf)> {
    #[cfg(unix)]
    {
        let vault = std::fs::canonicalize(vault).ok()?;
        obsidian_homes().into_iter().find_map(|(config, socket)| {
            let list = std::fs::read_to_string(config.join("obsidian.json")).ok()?;
            let id = open_vault_id(&list, &vault)?;
            std::os::unix::net::UnixStream::connect(&socket).ok()?;
            Some((id, socket))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = vault;
        None
    }
}

/// How long Obsidian is given to answer a command, in seconds.
const OBSIDIAN_ANSWER_SECS: u64 = 15;

/// Sends one command line to the Obsidian listening on `socket`, for its vault `id`, and
/// returns what it answers. This is what Obsidian's own `obsidian` command sends: one
/// JSON line (`argv`, `tty`, `cwd`), answered with text until the connection closes.
/// The vault is named in the first argument: without it Obsidian takes the window that
/// had the focus last, which may be another vault.
pub fn obsidian_cli_request(
    socket: &Path,
    id: &str,
    vault: &Path,
    args: &[&str],
) -> Result<String, RestaskError> {
    #[cfg(unix)]
    {
        use std::io::{Read as _, Write as _};
        let timeout = Some(std::time::Duration::from_secs(OBSIDIAN_ANSWER_SECS));
        let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
        let mut argv = vec![format!("vault={id}")];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        let request = serde_json::json!({ "argv": argv, "tty": false, "cwd": vault });
        stream.write_all(format!("{request}\n").as_bytes())?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        Ok(answer.trim().to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, id, vault, args);
        Err(obsidian_error(
            "Obsidian's command line interface is not reachable on this system",
        ))
    }
}

/// Whether a process is Obsidian itself — its executable is called `obsidian` — and not
/// one of the helper processes it starts from the same executable (`--type=renderer`,
/// `--type=gpu-process`, …).
pub fn is_obsidian_main(exe: &Path, argv: &[String]) -> bool {
    exe.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("obsidian"))
        && !argv.iter().any(|arg| arg.starts_with("--type="))
}

/// The one Obsidian this user runs. Other users' processes cannot be looked at and are
/// passed over; two Obsidians (an ordinary install and a sandboxed one) are an error —
/// setup does not guess which one to close.
#[cfg(target_os = "linux")]
fn obsidian_pid() -> Result<u32, RestaskError> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let Some(pid) = entry
            .ok()
            .and_then(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        else {
            continue;
        };
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let (Ok(exe), Ok(argv)) = (
            std::fs::read_link(proc.join("exe")),
            nul_separated(&proc.join("cmdline")),
        ) else {
            continue;
        };
        let argv: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        if is_obsidian_main(&exe, &argv) {
            found.push(pid);
        }
    }
    match found[..] {
        [pid] => Ok(pid),
        [] => Err(obsidian_error("no running Obsidian found")),
        _ => Err(obsidian_error("more than one Obsidian is running")),
    }
}

/// The entries of a `/proc/<pid>/cmdline` file.
#[cfg(target_os = "linux")]
fn nul_separated(path: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
    use std::os::unix::ffi::OsStringExt as _;
    Ok(std::fs::read(path)?
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| std::ffi::OsString::from_vec(entry.to_vec()))
        .collect())
}

/// How long a process is given to close after it was asked to, in seconds. It is never
/// killed: one that does not close keeps running, and is not started a second time.
#[cfg(target_os = "linux")]
const RESTART_GRACE_SECS: u64 = 15;

/// Whether process `pid` has ended (a process that ended and was not collected by its
/// parent yet counts).
#[cfg(target_os = "linux")]
fn has_ended(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // "<pid> (<name>) <state> …": the name may hold anything, the state follows
        // its last parenthesis.
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        Err(_) => true,
    }
}

/// Asks process `pid` to close (`SIGTERM`, the request a desktop sends at logout), waits
/// until it has, and starts it again as it was started: the same program, arguments and
/// working directory, detached from this terminal. Returns the new process id.
///
/// The environment is this process's own: the one the program was started with cannot
/// be read back from an app like Obsidian, which blanks it. Whether the program can be
/// started again is checked first — one whose program is not a file here (it runs in a
/// sandbox), or is a file that goes away with the process (an AppImage's mount), is
/// left running.
#[cfg(target_os = "linux")]
pub fn restart_process(pid: u32) -> Result<u32, RestaskError> {
    use std::os::unix::process::CommandExt as _;
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let argv = nul_separated(&proc.join("cmdline"))?;
    let started_as = |path: PathBuf| {
        Some(path).filter(|path| {
            path.is_absolute() && path.is_file() && !path.starts_with("/tmp/.mount_")
        })
    };
    let program = argv
        .first()
        .and_then(|program| started_as(PathBuf::from(program)))
        .or_else(|| started_as(std::fs::read_link(proc.join("exe")).ok()?))
        .ok_or_else(|| {
            obsidian_error(
                "its program is not a file this session can start again (an AppImage or a \
                 sandboxed install?); restart it by hand",
            )
        })?;
    let cwd = std::fs::read_link(proc.join("cwd")).ok();

    run("kill", &["-TERM", &pid.to_string()])?;
    let asked = std::time::Instant::now();
    while !has_ended(pid) {
        if asked.elapsed().as_secs() >= RESTART_GRACE_SECS {
            return Err(obsidian_error(format!(
                "it did not close within {RESTART_GRACE_SECS} s, and was left as it is"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let mut command = Command::new(&program);
    command
        .args(argv.iter().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group: a Ctrl-C in this terminal is not for it.
        .process_group(0);
    if let Some(cwd) = cwd.filter(|cwd| cwd.is_dir()) {
        command.current_dir(cwd);
    }
    let child = command.spawn().map_err(|error| {
        obsidian_error(format!(
            "it closed, but {} could not be started again ({error}); start it by hand",
            program.display()
        ))
    })?;
    Ok(child.id())
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

    /// Opens the connection to `host`, the ssh host of a node, letting ssh ask on the
    /// terminal whatever it needs to log in (a password, a key's passphrase, a new host's
    /// fingerprint). What follows on that host — [`DaemonInstaller::prepare_node`],
    /// [`DaemonInstaller::install_node`] — goes through the same connection and asks
    /// nothing again. The wizard calls it as soon as the host is typed; an installer
    /// that is not asked first connects when it first needs the host.
    fn connect_node(&self, host: &str) -> Result<(), RestaskError> {
        let _ = host;
        Ok(())
    }

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
/// the server over ssh (App. E) — through one connection ([`NodeLink`]) that is opened
/// once, kept for the run and closed when the installer is dropped.
#[derive(Default)]
pub struct SystemInstaller {
    link: Mutex<Option<NodeLink>>,
}

/// The ssh connection of one setup run to its node: a master connection (`ControlMaster`)
/// that ssh authenticated once, shared through the socket `control` by every ssh call of
/// the run.
struct NodeLink {
    /// The host the connection goes to.
    host: String,
    /// Private directory that holds the socket.
    dir: PathBuf,
    /// `ControlPath` of the connection.
    control: String,
}

impl Drop for NodeLink {
    fn drop(&mut self) {
        let _ = Command::new("ssh")
            .args(["-o", &format!("ControlPath={}", self.control)])
            .args(["-O", "exit", "--", &self.host])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How long the connection to a node stays open with nothing using it, in seconds: the
/// time the user may take over the wizard's next question before ssh asks again.
const NODE_LINK_IDLE_SECS: u32 = 900;

/// The arguments of the `ssh` call that opens the connection to `host` and leaves it in
/// the background behind the socket `control`: ssh asks what it needs on the terminal,
/// runs nothing on the host and returns.
pub fn node_link_args(control: &str, host: &str) -> Vec<String> {
    vec![
        "-o".to_string(),
        "ControlMaster=yes".to_string(),
        "-o".to_string(),
        format!("ControlPath={control}"),
        "-o".to_string(),
        format!("ControlPersist={NODE_LINK_IDLE_SECS}"),
        "--".to_string(),
        host.to_string(),
        "true".to_string(),
    ]
}

/// A new, private (0700) directory for a connection's socket: in `$XDG_RUNTIME_DIR`,
/// else the system's temporary directory.
fn node_link_dir() -> Result<PathBuf, RestaskError> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("restask-ssh-{}", std::process::id()));
    // What an interrupted run with this process id left behind.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
}

impl SystemInstaller {
    /// The `ControlPath` of the connection to `host`, opening it when this run has none
    /// yet ([`DaemonInstaller::connect_node`]).
    fn link(&self, host: &str) -> Result<String, RestaskError> {
        let mut link = self
            .link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(open) = link.as_ref().filter(|open| open.host == host) {
            return Ok(open.control.clone());
        }
        // A name that starts with a dash would be read by ssh as an option.
        if host.starts_with('-') || host.chars().any(char::is_whitespace) {
            return Err(RestaskError::Validation {
                field: "node",
                reason: format!("`{host}` is not an ssh host"),
            });
        }
        *link = None;
        println!("connecting to {host} …");
        let dir = node_link_dir()?;
        let control = dir.join("%C").display().to_string();
        // Created before the call, so that a connection left behind by a call that
        // failed half-way is closed with it.
        let opening = NodeLink {
            host: host.to_string(),
            dir,
            control: control.clone(),
        };
        let status = Command::new("ssh")
            .args(node_link_args(&control, host))
            .stdin(Stdio::null())
            .status()
            .map_err(|error| RestaskError::Validation {
                field: "node",
                reason: format!("cannot run ssh: {error}"),
            })?;
        if !status.success() {
            return Err(RestaskError::Validation {
                field: "node",
                reason: format!("cannot reach `{host}` over ssh (does `ssh {host}` work?)"),
            });
        }
        *link = Some(opening);
        Ok(control)
    }
}

impl DaemonInstaller for SystemInstaller {
    fn connect_node(&self, host: &str) -> Result<(), RestaskError> {
        self.link(host).map(|_| ())
    }

    fn prepare_node(&self, node: &NodeTarget) -> Result<(), RestaskError> {
        let script = node_script()?;
        let control = self.link(&node.host)?;
        println!("checking {} …", node.host);
        run_script(
            &script,
            &["check", &node.host, &node.vault, &node.dir],
            None,
            &control,
        )
    }

    fn install_node(
        &self,
        node: &NodeTarget,
        vault: &Path,
        access: NodeAccess<'_>,
    ) -> Result<String, RestaskError> {
        let script = node_script()?;
        let control = self.link(&node.host)?;
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
            &control,
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

/// Environment variable that hands `contrib/node.sh` the `ControlPath` of an ssh
/// connection that is already open: the script uses it and leaves it open.
pub const ENV_SSH_CONTROL: &str = "RESTASK_SSH_CONTROL";

/// Runs `contrib/node.sh` with `args`, its output going to the terminal, `input` to its
/// standard input, its ssh calls through the connection behind `control`.
fn run_script(
    script: &Path,
    args: &[&str],
    input: Option<&str>,
    control: &str,
) -> Result<(), RestaskError> {
    use std::io::Write as _;
    let mut child = Command::new("bash")
        .arg(script)
        .args(args)
        .env(ENV_SSH_CONTROL, control)
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

/// Prints the §13.2 step-8 summary (backup, config, client wiring, next steps) to stdout.
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
    if let Some(note) = summary.plugin_note() {
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
