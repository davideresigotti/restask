//! Configuration types and loaders (`docs/spec/config.md` §14, §17).
//!
//! Two configuration layers exist:
//!
//! * **Vault config** — `restask.toml` at the vault root (synced, safe to commit): scanning
//!   knobs such as `track`/`ignore` globs.
//! * **Machine config** — `$XDG_CONFIG_HOME/restask/config.toml` (chmod 600, never synced):
//!   CalDAV endpoint and secret sources on the sync node; on a machine that only edits,
//!   where the sync node is.
//!
//! Secrets never live in config files (§17): a literal `password` key anywhere in a config
//! document is rejected at parse time. Passwords resolve at call time from the environment or
//! a 0600 password file via [`MachineConfig::resolved_password`].

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;

/// Environment variable overriding the vault path (§14.3).
pub const ENV_VAULT: &str = "RESTASK_VAULT";
/// Environment variable overriding the machine-config file path (§14.3).
pub const ENV_CONFIG: &str = "RESTASK_CONFIG";
/// Environment variable overriding the CalDAV URL (§14.3).
pub const ENV_CALDAV_URL: &str = "RESTASK_CALDAV_URL";
/// Environment variable overriding the CalDAV username (§14.3).
pub const ENV_CALDAV_USERNAME: &str = "RESTASK_CALDAV_USERNAME";
/// Environment variable holding the CalDAV password; always wins over `password_file` (§14.3).
pub const ENV_CALDAV_PASSWORD: &str = "RESTASK_CALDAV_PASSWORD";

/// Errors produced while loading, validating or saving configuration files.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("config file {path}: {source}")]
    Io {
        /// File involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The file is not valid TOML for the expected schema.
    #[error("config parse error in {path}: {source}")]
    Parse {
        /// File being parsed.
        path: PathBuf,
        /// Underlying TOML error.
        source: toml::de::Error,
    },
    /// The config could not be serialized to TOML.
    #[error("config serialize error for {path}: {source}")]
    Serialize {
        /// File being written.
        path: PathBuf,
        /// Underlying TOML error.
        source: toml::ser::Error,
    },
    /// A literal `password` key was found (§17: secrets never live in config files).
    #[error("literal `password` key in {0}: secrets never live in config files (use password_file or password_env)")]
    LiteralPassword(PathBuf),
    /// A `track`/`ignore` pattern is not a valid glob.
    #[error("invalid glob pattern `{pattern}`: {source}")]
    Glob {
        /// The offending pattern.
        pattern: String,
        /// Underlying globset error.
        source: globset::Error,
    },
    /// No home directory could be determined (needed to expand `~`).
    #[error("cannot determine the home directory")]
    NoHome,
    /// The configured password file exists but could not be read.
    #[error("password file {path}: {source}")]
    PasswordFile {
        /// Password file path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

/// Reads and parses a TOML document of `T`, rejecting literal `password` keys (§17).
fn parse_toml<T: DeserializeOwned>(contents: &str, path: &Path) -> Result<T, ConfigError> {
    let value: toml::Value = toml::from_str(contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    reject_literal_password(&value, path)?;
    value.try_into().map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Rejects any `password` key anywhere in the document (exact key match; `password_file` and
/// `password_env` are unaffected).
fn reject_literal_password(value: &toml::Value, path: &Path) -> Result<(), ConfigError> {
    match value {
        toml::Value::Table(map) => {
            if map.contains_key("password") {
                return Err(ConfigError::LiteralPassword(path.to_path_buf()));
            }
            for child in map.values() {
                reject_literal_password(child, path)?;
            }
            Ok(())
        }
        toml::Value::Array(items) => {
            for item in items {
                reject_literal_password(item, path)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Expands a leading `~` or `~/` to `home`; other paths (including `~user/...`) pass through.
pub fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    match text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => path.to_path_buf(),
    }
}

/// Atomically writes a config file through [`crate::fsio`], mapping failures to
/// [`ConfigError::Io`].
fn write_atomic(path: &Path, contents: &str, unix_mode: Option<u32>) -> Result<(), ConfigError> {
    crate::fsio::write_atomic_mode(path, contents, unix_mode).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn default_done_heading() -> String {
    "Done".to_string()
}

fn default_inbox_file() -> String {
    "TODO.md".to_string()
}

fn default_inbox_list() -> String {
    "inbox".to_string()
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_track() -> Vec<String> {
    vec!["**/*.md".to_string()]
}

fn default_ignore() -> Vec<String> {
    [".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
        .iter()
        .map(|pattern| pattern.to_string())
        .collect()
}

/// Vault configuration — `restask.toml` in the vault root (§14.1: synced, safe to commit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// Heading text that starts the completed-records region of a note.
    pub done_heading: String,
    /// Engine-managed inbox and aggregation view (vault-relative path).
    pub inbox_file: String,
    /// The list the inbox file routes to (§5.2); its slug names the bound CalDAV
    /// collection. `restask setup` records the user-chosen calendar here.
    pub inbox_list: String,
    /// Further calendars whose tasks live in the inbox file (§7.5): a task created on the
    /// server in one of them, and that no note is the home of, gets its line there, marked
    /// with the calendar's name. Empty: the inbox file holds the tasks of `inbox_list` only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub todo_lists: Vec<String>,
    /// Whether a calendar that appears on the server later — made in another client —
    /// is added to `todo_lists` by the sync node (§7.5). On unless set to `false`.
    #[serde(skip_serializing_if = "is_true")]
    pub todo_new_lists: bool,
    /// The vault's name in Obsidian, for the links that open a task's linked notes from
    /// another client (§8.4). `restask setup` records the vault folder's name; absent, no
    /// link is written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obsidian_vault: Option<String>,
    /// Vault-relative globs of files the engine may parse; `ignore` wins over `track`.
    pub track: Vec<String>,
    /// Vault-relative globs excluded even when matched by `track`.
    pub ignore: Vec<String>,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            done_heading: default_done_heading(),
            inbox_file: default_inbox_file(),
            inbox_list: default_inbox_list(),
            todo_lists: Vec::new(),
            todo_new_lists: true,
            obsidian_vault: None,
            track: default_track(),
            ignore: default_ignore(),
        }
    }
}

impl VaultConfig {
    /// Reads and parses `restask.toml` from `path`. A missing file is an error; callers may use
    /// [`VaultConfig::default`] when the vault has no config file yet.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        parse_toml(&contents, path)
    }

    /// Serializes and atomically writes this config to `path` (tmp + fsync + rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let contents =
            toml::ser::to_string_pretty(self).map_err(|source| ConfigError::Serialize {
                path: path.to_path_buf(),
                source,
            })?;
        write_atomic(path, &contents, None)
    }

    /// Compiles the `track`/`ignore` glob sets for this config.
    pub fn matchers(&self) -> Result<VaultMatchers, ConfigError> {
        VaultMatchers::build(self)
    }
}

impl FromStr for VaultConfig {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_toml(s, Path::new("<inline>"))
    }
}

/// Precompiled `track`/`ignore` glob sets derived from a [`VaultConfig`].
#[derive(Debug, Clone)]
pub struct VaultMatchers {
    track: GlobSet,
    ignore: GlobSet,
}

impl VaultMatchers {
    /// Compiles both glob sets; fails on invalid user-supplied patterns.
    pub fn build(config: &VaultConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            track: compile_globset(&config.track)?,
            ignore: compile_globset(&config.ignore)?,
        })
    }

    /// Whether a vault-relative path (forward slashes) must be parsed and synced.
    ///
    /// `ignore` wins over `track`: a path matched by `ignore` is never tracked even when a
    /// `track` pattern matches it. Globs use gitignore-style component semantics: `*` does not
    /// cross `/` while a `**` component does.
    pub fn is_tracked(&self, vault_relative_path: &str) -> bool {
        if self.ignore.is_match(vault_relative_path) {
            return false;
        }
        self.track.is_match(vault_relative_path)
    }

    /// Whether a vault-relative path is excluded by an `ignore` pattern.
    pub fn is_ignored(&self, vault_relative_path: &str) -> bool {
        self.ignore.is_match(vault_relative_path)
    }
}

fn compile_globset(patterns: &[String]) -> Result<GlobSet, ConfigError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|source| ConfigError::Glob {
                pattern: pattern.clone(),
                source,
            })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| ConfigError::Glob {
        pattern: "<glob set>".to_string(),
        source,
    })
}

/// `[vault]` section of the machine config: optional pinned vault path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VaultSection {
    /// Absolute vault path, when pinned (usually discovered per §13.3 instead).
    pub path: Option<PathBuf>,
}

/// `[caldav]` section of the machine config: server endpoint and secret sources (§14.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaldavConfig {
    /// Base URL of the CalDAV server (e.g. `http://192.168.1.10:5232`).
    pub url: Option<String>,
    /// CalDAV user name.
    pub username: Option<String>,
    /// 0600 file holding the password; a leading `~` expands to the home directory.
    pub password_file: Option<PathBuf>,
    /// Environment variable holding the password (alternative to `password_file`).
    pub password_env: Option<String>,
    /// Daemon poll interval in seconds: a pass whether or not anything changed.
    pub poll_secs: u64,
    /// How often the daemon asks the server whether another client wrote there, in
    /// seconds; `0` leaves server-side changes to the poll.
    pub watch_secs: u64,
    /// Whether the engine may MKCOL missing collections on first push.
    pub allow_create_lists: bool,
}

impl Default for CaldavConfig {
    fn default() -> Self {
        Self {
            url: None,
            username: None,
            password_file: None,
            password_env: None,
            poll_secs: 300,
            watch_secs: 2,
            allow_create_lists: true,
        }
    }
}

/// `[node]` section of the machine config (§14.2): present on a machine that only edits
/// the vault and leaves the syncing to the vault's sync node (§1.1). Such a machine holds
/// no server credentials and never contacts the server; the section says where the
/// daemon is, for the commands that point there (`restask doctor`, `restask lists`,
/// `contrib/update.sh`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NodeSection {
    /// ssh host of the sync node, when setup installed the daemon there.
    pub host: Option<String>,
    /// Directory of the daemon's compose stack on that host.
    pub dir: Option<String>,
    /// The vault's folder on that host.
    pub vault: Option<String>,
    /// CalDAV base URL the node's daemon syncs with — what the other clients connect to.
    pub url: Option<String>,
    /// CalDAV user name on that server.
    pub username: Option<String>,
}

/// Machine configuration — `$XDG_CONFIG_HOME/restask/config.toml` (§14.2: chmod 600, never synced).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MachineConfig {
    /// Optional pinned vault path.
    pub vault: VaultSection,
    /// CalDAV endpoint and secret sources: the sync node's half.
    pub caldav: CaldavConfig,
    /// Where the vault's daemon runs, on a machine that is not the sync node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeSection>,
}

impl MachineConfig {
    /// Reads and parses the machine config from `path`. No environment overrides are applied;
    /// call [`MachineConfig::apply_env`] afterwards for the §14.3 behavior.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        parse_toml(&contents, path)
    }

    /// Applies the §14.3 environment overrides using the process environment: `RESTASK_CALDAV_URL`
    /// and `RESTASK_CALDAV_USERNAME` replace the file values when set (empty values count as
    /// unset). The password is never stored; it resolves at call time via
    /// [`MachineConfig::resolved_password`].
    pub fn apply_env(&mut self) {
        self.apply_env_with(|key| std::env::var(key).ok().filter(|value| !value.is_empty()));
    }

    /// [`MachineConfig::apply_env`] with an injectable environment lookup (hermetic tests).
    pub fn apply_env_with<E: Fn(&str) -> Option<String>>(&mut self, env: E) {
        if let Some(url) = env(ENV_CALDAV_URL).filter(|value| !value.is_empty()) {
            self.caldav.url = Some(url);
        }
        if let Some(username) = env(ENV_CALDAV_USERNAME).filter(|value| !value.is_empty()) {
            self.caldav.username = Some(username);
        }
    }

    /// Vault path after the `RESTASK_VAULT` override (env wins over the `[vault]` section).
    pub fn resolve_vault_path(&self) -> Option<PathBuf> {
        self.resolve_vault_path_with(|key| {
            std::env::var(key).ok().filter(|value| !value.is_empty())
        })
    }

    /// [`MachineConfig::resolve_vault_path`] with an injectable environment lookup.
    pub fn resolve_vault_path_with<E: Fn(&str) -> Option<String>>(
        &self,
        env: E,
    ) -> Option<PathBuf> {
        if let Some(path) = env(ENV_VAULT).filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(path));
        }
        self.vault.path.clone()
    }

    /// Resolves the CalDAV password (§14.3, §17): `RESTASK_CALDAV_PASSWORD` wins over
    /// `password_env`, which wins over `password_file` (trimmed, `~` expanded). Empty env values
    /// count as unset. Returns `None` when no source is configured.
    pub fn resolved_password(&self) -> Result<Option<String>, ConfigError> {
        let home = std::env::home_dir().ok_or(ConfigError::NoHome)?;
        self.resolved_password_with(&home, |key| {
            std::env::var(key).ok().filter(|value| !value.is_empty())
        })
    }

    /// [`MachineConfig::resolved_password`] with an injectable home directory and environment
    /// lookup (hermetic tests).
    pub fn resolved_password_with<E: Fn(&str) -> Option<String>>(
        &self,
        home: &Path,
        env: E,
    ) -> Result<Option<String>, ConfigError> {
        if let Some(password) = env(ENV_CALDAV_PASSWORD).filter(|value| !value.is_empty()) {
            return Ok(Some(password));
        }
        if let Some(var_name) = self.caldav.password_env.as_deref() {
            if let Some(password) = env(var_name).filter(|value| !value.is_empty()) {
                return Ok(Some(password));
            }
        }
        if let Some(file) = &self.caldav.password_file {
            let path = expand_tilde(file, home);
            let contents = fs::read_to_string(&path)
                .map_err(|source| ConfigError::PasswordFile { path, source })?;
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
        Ok(None)
    }

    /// Serializes and atomically writes this config to `path`, mode 0600 on Unix (§14.2).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let contents =
            toml::ser::to_string_pretty(self).map_err(|source| ConfigError::Serialize {
                path: path.to_path_buf(),
                source,
            })?;
        write_atomic(path, &contents, Some(0o600))
    }
}

impl FromStr for MachineConfig {
    type Err = ConfigError;

    /// Parses a machine config from TOML text. No environment overrides are applied; see
    /// [`MachineConfig::load`] and [`MachineConfig::apply_env`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_toml(s, Path::new("<inline>"))
    }
}

/// Default machine-config path: `$RESTASK_CONFIG`, else `$XDG_CONFIG_HOME/restask/config.toml`,
/// else `$HOME/.config/restask/config.toml` (§14.2, §14.3).
pub fn machine_config_path() -> PathBuf {
    machine_config_path_from(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

/// [`machine_config_path`] with an injectable environment lookup (hermetic tests).
pub fn machine_config_path_from<E: Fn(&str) -> Option<String>>(env: E) -> PathBuf {
    if let Some(path) = env(ENV_CONFIG).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    let base = env("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("restask").join("config.toml")
}

/// The machine config of `vault` (§14.2): a machine has one per vault it works on, so
/// that two vaults — each with its own server, its own sync node — never read each
/// other's. `$RESTASK_CONFIG` names it outright; else see [`machine_config_of`].
pub fn machine_config_path_for(vault: &Path) -> PathBuf {
    machine_config_path_for_from(vault, |key| {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    })
}

/// [`machine_config_path_for`] with an injectable environment lookup (hermetic tests).
pub fn machine_config_path_for_from<E: Fn(&str) -> Option<String>>(
    vault: &Path,
    env: E,
) -> PathBuf {
    if let Some(path) = env(ENV_CONFIG).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    machine_config_of(&machine_config_path_from(env), vault)
}

/// Which file holds the machine config of `vault`, given the machine's first one, `main`
/// (`…/restask/config.toml`). The file is known by the vault its `[vault] path` names:
///
/// 1. a further config, `…/restask/vaults/<name>/config.toml`, that names this vault;
/// 2. `main`, when it is not there yet, names this vault, names none, or names a folder
///    this machine does not have (a hand-written path, a vault that was moved): one
///    config serves a machine with one vault, as it always did;
/// 3. else — `main` is another vault's — a further config of this vault's own, in a
///    directory called after the vault's folder (`-2`, `-3` … when another vault of that
///    name has it). Whatever lies beside a config (the password file) is per vault too.
pub fn machine_config_of(main: &Path, vault: &Path) -> PathBuf {
    let further = main.parent().unwrap_or(Path::new(".")).join("vaults");
    let mut configs: Vec<PathBuf> = fs::read_dir(&further)
        .map(|entries| {
            entries
                .filter_map(|entry| Some(entry.ok()?.path().join("config.toml")))
                .filter(|config| config.is_file())
                .collect()
        })
        .unwrap_or_default();
    configs.sort();
    if let Some(own) = configs
        .into_iter()
        .find(|config| named_vault(config).is_some_and(|named| same_folder(&named, vault)))
    {
        return own;
    }
    match named_vault(main) {
        Some(named) if !same_folder(&named, vault) && named.is_dir() => {}
        _ => return main.to_path_buf(),
    }
    let name = folder_slug(vault);
    let mut dir = further.join(&name);
    let mut count = 1;
    while dir.exists() {
        count += 1;
        dir = further.join(format!("{name}-{count}"));
    }
    dir.join("config.toml")
}

/// The vault a machine config names in `[vault] path`; `None` when the file is not
/// there, does not parse or names none.
fn named_vault(config: &Path) -> Option<PathBuf> {
    let value: toml::Value = toml::from_str(&fs::read_to_string(config).ok()?).ok()?;
    Some(PathBuf::from(value.get("vault")?.get("path")?.as_str()?))
}

/// Whether two paths are one folder, links resolved where the folders exist.
fn same_folder(left: &Path, right: &Path) -> bool {
    let resolved = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolved(left) == resolved(right)
}

/// A vault folder's name as a directory name of this machine's configs: lower case,
/// anything but letters and digits a dash.
fn folder_slug(vault: &Path) -> String {
    let name = fs::canonicalize(vault)
        .unwrap_or_else(|_| vault.to_path_buf())
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "vault".to_string()
    } else {
        slug
    }
}
