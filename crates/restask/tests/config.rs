//! Integration tests for configuration types and loaders (`docs/spec/config.md` §14, §17).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use pretty_assertions::assert_eq;
use restask::config::{
    expand_tilde, machine_config_of, machine_config_path_for_from, machine_config_path_from,
    ConfigError, MachineConfig, VaultConfig, ENV_CALDAV_PASSWORD, ENV_CALDAV_URL,
    ENV_CALDAV_USERNAME, ENV_CONFIG, ENV_VAULT,
};
use restask::daemon::DaemonConfig;
use restask::store::{device_file, Device};
use tempfile::tempdir;

/// Builds an injectable environment lookup from key/value pairs (hermetic; no process env).
fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    move |key| map.get(key).cloned()
}

#[test]
fn vault_config_defaults() {
    let cfg = VaultConfig::default();
    assert_eq!(cfg.done_heading, "Done");
    assert_eq!(cfg.inbox_file, "TODO.md");
    assert_eq!(cfg.track, vec!["**/*.md"]);
    assert_eq!(
        cfg.ignore,
        vec![".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
    );

    let parsed = VaultConfig::from_str("").unwrap();
    assert_eq!(parsed, cfg);

    let partial = VaultConfig::from_str("done_heading = \"Archived\"\n").unwrap();
    assert_eq!(partial.done_heading, "Archived");
    assert_eq!(partial.inbox_file, "TODO.md");
    assert_eq!(partial.track, cfg.track);
    assert_eq!(partial.ignore, cfg.ignore);
}

#[test]
fn vault_config_round_trip() {
    let cfg = VaultConfig::from_str(
        "done_heading = \"Archived\"\ninbox_file = \"Tasks.md\"\ntrack = [\"notes/*.md\"]\nignore = [\".restask/**\"]\n",
    )
    .unwrap();
    let dir = tempdir().unwrap();
    let path = dir.path().join("restask.toml");
    cfg.save(&path).unwrap();
    assert_eq!(VaultConfig::load(&path).unwrap(), cfg);
    assert!(!dir.path().join("restask.toml.restask-tmp").exists());
}

#[test]
fn the_calendars_todo_md_shows_are_written_only_when_there_are_some() {
    // A vault set up before the key existed keeps its file as it is.
    let cfg = VaultConfig::default();
    assert!(cfg.todo_lists.is_empty());
    let dir = tempdir().unwrap();
    let path = dir.path().join("restask.toml");
    cfg.save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("todo_lists"));

    let cfg =
        VaultConfig::from_str("inbox_list = \"personal\"\ntodo_lists = [\"work\", \"Home Lab\"]\n")
            .unwrap();
    assert_eq!(cfg.todo_lists, vec!["work", "Home Lab"]);
    cfg.save(&path).unwrap();
    assert_eq!(VaultConfig::load(&path).unwrap(), cfg);
    // As slugs, without the calendar the file is bound to.
    let lists = restask::vault::todo_lists(&VaultConfig {
        todo_lists: vec!["Work".into(), "personal".into(), "Home Lab".into()],
        ..cfg
    })
    .unwrap();
    let names: Vec<&str> = lists.iter().map(|list| list.as_str()).collect();
    assert_eq!(names, vec!["home-lab", "work"]);
}

#[test]
fn the_obsidian_vault_is_written_only_when_it_is_known() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("restask.toml");
    VaultConfig::default().save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("obsidian_vault"));
    assert_eq!(VaultConfig::load(&path).unwrap().obsidian_vault, None);

    let cfg = VaultConfig::from_str("obsidian_vault = \"2nd brain\"\n").unwrap();
    assert_eq!(cfg.obsidian_vault.as_deref(), Some("2nd brain"));
    cfg.save(&path).unwrap();
    assert_eq!(VaultConfig::load(&path).unwrap(), cfg);
}

#[test]
fn vault_load_missing_file_is_error() {
    let dir = tempdir().unwrap();
    let err = VaultConfig::load(&dir.path().join("absent.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
}

#[test]
fn machine_config_defaults() {
    let cfg = MachineConfig::from_str("").unwrap();
    assert_eq!(cfg.caldav.poll_secs, 300);
    // A machine config that does not spell the watch out — every one setup wrote — asks
    // the server every two seconds.
    assert_eq!(cfg.caldav.watch_secs, 2);
    assert_eq!(
        DaemonConfig::default().watch_ms,
        cfg.caldav.watch_secs * 1_000
    );
    assert!(cfg.caldav.allow_create_lists);
    assert!(cfg.caldav.url.is_none());
    assert!(cfg.caldav.username.is_none());
    assert!(cfg.vault.path.is_none());

    let both =
        MachineConfig::from_str("[caldav]\npassword_file = \"p\"\npassword_env = \"E\"\n").unwrap();
    assert!(both.caldav.password_file.is_some());
    assert!(both.caldav.password_env.is_some());
}

#[test]
fn machine_config_round_trip() {
    let text = "[vault]\npath = \"/opt/docker/syncthing/data/obsidian\"\n\n[caldav]\nurl = \"http://192.168.1.10:5232\"\nusername = \"me\"\npassword_file = \"~/.config/restask/radicale.passwd\"\npoll_secs = 300\nallow_create_lists = true\n\n[[lists]]\nname = \"Inbox\"\ncollection = \"inbox\"\n\n[[lists]]\nname = \"University\"\ncollection = \"university\"\n";
    let cfg = MachineConfig::from_str(text).unwrap();
    assert_eq!(
        cfg.vault.path,
        Some(PathBuf::from("/opt/docker/syncthing/data/obsidian"))
    );
    assert_eq!(cfg.caldav.url.as_deref(), Some("http://192.168.1.10:5232"));
    assert_eq!(cfg.caldav.username.as_deref(), Some("me"));
    assert_eq!(
        cfg.caldav.password_file,
        Some(PathBuf::from("~/.config/restask/radicale.passwd"))
    );
    assert_eq!(cfg.caldav.poll_secs, 300);
    assert!(cfg.caldav.allow_create_lists);
    // The `[[lists]]` tables above are what older versions recorded: routing lives in the
    // notes' frontmatter, so they are accepted and ignored — an existing config keeps
    // loading.

    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    cfg.save(&path).unwrap();
    assert_eq!(MachineConfig::load(&path).unwrap(), cfg);
}

#[test]
fn an_editing_machine_records_its_sync_node_and_no_credentials() {
    // What `restask setup` writes on a computer whose vault is synced by a daemon on an
    // always-on server (§14.2): where that daemon is, and the endpoint it talks to — for
    // `restask lists` — but nothing this machine could log in with.
    let text = "[vault]\npath = \"/home/me/Vault\"\n\n[node]\nhost = \"homeserver\"\ndir = \"restask\"\nvault = \"/srv/sync/vault\"\nurl = \"http://192.168.1.10:5232\"\nusername = \"me\"\n";
    let cfg = MachineConfig::from_str(text).unwrap();
    let node = cfg.node.clone().unwrap();
    assert_eq!(node.host.as_deref(), Some("homeserver"));
    assert_eq!(node.dir.as_deref(), Some("restask"));
    assert_eq!(node.vault.as_deref(), Some("/srv/sync/vault"));
    assert_eq!(node.url.as_deref(), Some("http://192.168.1.10:5232"));
    assert_eq!(node.username.as_deref(), Some("me"));
    assert_eq!(cfg.caldav.url, None);
    assert_eq!(
        cfg.resolved_password_with(Path::new("/home/me"), |_| None)
            .unwrap(),
        None
    );

    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    cfg.save(&path).unwrap();
    assert_eq!(MachineConfig::load(&path).unwrap(), cfg);

    // The section is the mark of an editing machine: a sync node's config has none, also
    // after it was written back, and a bare `[node]` (the daemon was installed by hand)
    // is kept.
    assert_eq!(MachineConfig::default().node, None);
    MachineConfig::default().save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("[node]"));
    let bare = MachineConfig::from_str("[node]\n").unwrap();
    bare.save(&path).unwrap();
    assert!(MachineConfig::load(&path).unwrap().node.is_some());
}

#[test]
fn literal_password_rejected() {
    let err = MachineConfig::from_str("[caldav]\npassword = \"hunter2\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::LiteralPassword(_)));

    let err =
        MachineConfig::from_str("[[lists]]\nname = \"x\"\npassword = \"hunter2\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::LiteralPassword(_)));

    assert!(
        MachineConfig::from_str("[caldav]\npassword_file = \"p\"\npassword_env = \"E\"\n").is_ok()
    );
}

#[test]
fn vault_matchers_ignore_wins_over_track() {
    let matchers = VaultConfig::default().matchers().unwrap();
    assert!(matchers.is_tracked("TODO.md"));
    assert!(matchers.is_tracked("notes/idea.md"));
    assert!(matchers.is_tracked("projects/school/plan.md"));
    assert!(!matchers.is_tracked(".restask/index.json"));
    assert!(!matchers.is_tracked(".restask/tasks/restask-01abc.ics"));
    assert!(!matchers.is_tracked(".obsidian/app.json"));
    assert!(!matchers.is_tracked(".trash/old.md"));
    assert!(!matchers.is_tracked(".git/config"));
}

#[test]
fn vault_matchers_custom_patterns() {
    let cfg = VaultConfig::from_str(
        "track = [\"notes/*.md\", \"notes/**/*.md\"]\nignore = [\"notes/scratch.md\"]\n",
    )
    .unwrap();
    let matchers = cfg.matchers().unwrap();
    assert!(matchers.is_tracked("notes/a.md"));
    assert!(matchers.is_tracked("notes/sub/b.md"));
    assert!(!matchers.is_tracked("notes/scratch.md"));
    assert!(!matchers.is_tracked("other/c.md"));
}

#[test]
fn invalid_glob_rejected() {
    let cfg = VaultConfig::from_str("track = [\"[invalid\"]\n").unwrap();
    assert!(matches!(
        cfg.matchers().unwrap_err(),
        ConfigError::Glob { .. }
    ));
}

#[test]
fn env_overrides_caldav() {
    let mut cfg =
        MachineConfig::from_str("[caldav]\nurl = \"http://file:5232\"\nusername = \"file-user\"\n")
            .unwrap();
    cfg.apply_env_with(env_map(&[
        (ENV_CALDAV_URL, "http://env:5232"),
        (ENV_CALDAV_USERNAME, "env-user"),
    ]));
    assert_eq!(cfg.caldav.url.as_deref(), Some("http://env:5232"));
    assert_eq!(cfg.caldav.username.as_deref(), Some("env-user"));

    let mut cfg = MachineConfig::from_str("[caldav]\nurl = \"http://file:5232\"\n").unwrap();
    cfg.apply_env_with(env_map(&[(ENV_CALDAV_URL, "")]));
    assert_eq!(cfg.caldav.url.as_deref(), Some("http://file:5232"));
}

#[test]
fn env_overrides_vault_path() {
    let cfg = MachineConfig::from_str("[vault]\npath = \"/from/file\"\n").unwrap();
    assert_eq!(
        cfg.resolve_vault_path_with(env_map(&[])),
        Some(PathBuf::from("/from/file"))
    );
    assert_eq!(
        cfg.resolve_vault_path_with(env_map(&[(ENV_VAULT, "/from/env")])),
        Some(PathBuf::from("/from/env"))
    );
}

#[test]
fn password_resolution_order() {
    let home = tempdir().unwrap();
    let secrets = home.path().join("secrets");
    fs::create_dir(&secrets).unwrap();
    fs::write(secrets.join("radicale.passwd"), "file-pass\n").unwrap();

    let cfg = MachineConfig::from_str(
        "[caldav]\npassword_file = \"~/secrets/radicale.passwd\"\npassword_env = \"TASKS_PASS\"\n",
    )
    .unwrap();

    // 1. RESTASK_CALDAV_PASSWORD always wins.
    let env = env_map(&[
        (ENV_CALDAV_PASSWORD, "master-env-pass"),
        ("TASKS_PASS", "named-env-pass"),
    ]);
    assert_eq!(
        cfg.resolved_password_with(home.path(), &env)
            .unwrap()
            .as_deref(),
        Some("master-env-pass")
    );

    // Empty RESTASK_CALDAV_PASSWORD counts as unset; password_env is next.
    let env = env_map(&[(ENV_CALDAV_PASSWORD, ""), ("TASKS_PASS", "named-env-pass")]);
    assert_eq!(
        cfg.resolved_password_with(home.path(), &env)
            .unwrap()
            .as_deref(),
        Some("named-env-pass")
    );

    // 3. password_file last (trimmed, tilde-expanded).
    let env = env_map(&[]);
    assert_eq!(
        cfg.resolved_password_with(home.path(), &env)
            .unwrap()
            .as_deref(),
        Some("file-pass")
    );

    // 4. Nothing configured → None.
    let bare = MachineConfig::from_str("[caldav]\n").unwrap();
    assert_eq!(
        bare.resolved_password_with(home.path(), &env).unwrap(),
        None
    );

    // 5. Missing password file → error.
    let gone =
        MachineConfig::from_str("[caldav]\npassword_file = \"~/secrets/absent.passwd\"\n").unwrap();
    assert!(matches!(
        gone.resolved_password_with(home.path(), &env).unwrap_err(),
        ConfigError::PasswordFile { .. }
    ));
}

#[test]
fn tilde_expansion() {
    let home = Path::new("/home/me");
    assert_eq!(expand_tilde(Path::new("~"), home), home);
    assert_eq!(
        expand_tilde(Path::new("~/docs/t.md"), home),
        PathBuf::from("/home/me/docs/t.md")
    );
    assert_eq!(
        expand_tilde(Path::new("/abs/t.md"), home),
        PathBuf::from("/abs/t.md")
    );
    assert_eq!(
        expand_tilde(Path::new("~other/t.md"), home),
        PathBuf::from("~other/t.md")
    );
}

#[test]
fn machine_config_path_precedence() {
    assert_eq!(
        machine_config_path_from(env_map(&[
            (ENV_CONFIG, "/tmp/custom.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home")
        ])),
        PathBuf::from("/tmp/custom.toml")
    );
    assert_eq!(
        machine_config_path_from(env_map(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home")])),
        PathBuf::from("/xdg/restask/config.toml")
    );
    assert_eq!(
        machine_config_path_from(env_map(&[("HOME", "/home")])),
        PathBuf::from("/home/.config/restask/config.toml")
    );
}

/// A machine config at `path` that names `vault`.
fn config_naming(path: &Path, vault: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("[vault]\npath = \"{}\"\n", vault.display())).unwrap();
}

#[test]
fn every_vault_has_a_machine_config_of_its_own() {
    // One computer, two vaults, two task servers: a command run in one vault must never
    // find the other's server (or the other's sync node) in the config it loads.
    let home = tempdir().unwrap();
    let main = home.path().join("restask/config.toml");
    let further = home.path().join("restask/vaults");
    let folders = tempdir().unwrap();
    let first = folders.path().join("Obsidian");
    let second = folders.path().join("Work Notes");
    let third = folders.path().join("elsewhere/Work Notes");
    for vault in [&first, &second, &third] {
        fs::create_dir_all(vault).unwrap();
    }

    // A machine restask was never set up on: the first vault gets the one config.
    assert_eq!(machine_config_of(&main, &first), main);
    config_naming(&main, &first);
    assert_eq!(machine_config_of(&main, &first), main);

    // A second vault gets a config in a directory called after its folder — before it
    // is set up too: a command there finds no config, not the first vault's.
    let own = further.join("work-notes/config.toml");
    assert_eq!(machine_config_of(&main, &second), own);
    config_naming(&own, &second);
    assert_eq!(machine_config_of(&main, &second), own);
    assert_eq!(machine_config_of(&main, &first), main);

    // Another vault whose folder has the same name does not take that directory.
    assert_eq!(
        machine_config_of(&main, &third),
        further.join("work-notes-2/config.toml")
    );

    // The vault is one folder, however it is reached.
    #[cfg(unix)]
    {
        let link = folders.path().join("link");
        std::os::unix::fs::symlink(&second, &link).unwrap();
        assert_eq!(machine_config_of(&main, &link), own);
    }
}

/// Seen on the maintainer's machine (2026-10-09): a vault that is not the machine's
/// first and was never set up there got a new directory under `vaults/` — `-2`, `-3`,
/// … `-15` — at every `restask settle`, and with each a new device identity and a new
/// claim in the vault's `.restask/devices/`.
#[test]
fn a_vault_without_a_config_keeps_the_directory_its_identity_is_in() {
    let home = tempdir().unwrap();
    let main = home.path().join("restask/config.toml");
    let further = home.path().join("restask/vaults");
    let folders = tempdir().unwrap();
    let first = folders.path().join("Obsidian");
    let second = folders.path().join("Work Notes");
    let third = folders.path().join("elsewhere/Work Notes");
    for vault in [&first, &second, &third] {
        fs::create_dir_all(vault.join(".restask")).unwrap();
    }
    config_naming(&main, &first);

    // The first command in the second vault registers a task: the identity goes
    // beside the config the vault will have, the claim into the vault.
    let own = further.join("work-notes/config.toml");
    assert_eq!(machine_config_of(&main, &second), own);
    let taken = std::collections::BTreeSet::new();
    let state = second.join(".restask");
    let device = Device::open(&state, Some(&device_file(&own)), None, &taken).unwrap();

    // Every later command finds the same directory, and so the same identity.
    assert_eq!(machine_config_of(&main, &second), own);
    let again = Device::open(&state, Some(&device_file(&own)), None, &taken).unwrap();
    assert_eq!(again, device);
    assert_eq!(fs::read_dir(state.join("devices")).unwrap().count(), 1);

    // Another vault of the same folder name has no claim to it.
    assert_eq!(
        machine_config_of(&main, &third),
        further.join("work-notes-2/config.toml")
    );
}

#[test]
fn one_config_still_serves_a_machine_with_one_vault() {
    let home = tempdir().unwrap();
    let main = home.path().join("restask/config.toml");
    let vault = tempdir().unwrap();
    fs::create_dir_all(main.parent().unwrap()).unwrap();

    // A config that names no vault — written by hand — is the machine's.
    fs::write(&main, "[caldav]\nurl = \"http://192.168.1.10:5232\"\n").unwrap();
    assert_eq!(machine_config_of(&main, vault.path()), main);

    // So is one that names a folder this machine does not have: the example path of a
    // hand-written config in a container, a vault that was moved. Without this the
    // daemon of such a machine would lose its server with the update.
    config_naming(
        &main,
        Path::new("/opt/docker/syncthing/data/not-on-this-machine"),
    );
    assert_eq!(machine_config_of(&main, vault.path()), main);

    // A config that does not parse is still the one that is loaded, and reported.
    fs::write(&main, "[vault\n").unwrap();
    assert_eq!(machine_config_of(&main, vault.path()), main);

    // `RESTASK_CONFIG` names the config outright, whatever vault it is used for.
    config_naming(&main, tempdir().unwrap().path());
    assert_eq!(
        machine_config_path_for_from(
            vault.path(),
            env_map(&[(ENV_CONFIG, "/tmp/custom.toml"), ("HOME", "/home")])
        ),
        PathBuf::from("/tmp/custom.toml")
    );
}

#[test]
#[cfg(unix)]
fn machine_config_saved_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    MachineConfig::default().save(&path).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn new_calendars_are_followed_unless_the_vault_says_no() {
    let cfg = VaultConfig::from_str("inbox_list = \"personal\"\n").unwrap();
    assert!(cfg.todo_new_lists);
    // The default is not written: configs of vaults set up before do not change.
    assert!(!toml::to_string_pretty(&cfg)
        .unwrap()
        .contains("todo_new_lists"));
    let off = VaultConfig::from_str("todo_new_lists = false\n").unwrap();
    assert!(!off.todo_new_lists);
    assert!(toml::to_string_pretty(&off)
        .unwrap()
        .contains("todo_new_lists = false"));
}
