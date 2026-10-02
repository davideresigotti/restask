//! Integration tests for configuration types and loaders (`docs/spec/config.md` §14, §17).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use pretty_assertions::assert_eq;
use restask::config::{
    expand_tilde, machine_config_path_from, ConfigError, MachineConfig, VaultConfig,
    ENV_CALDAV_PASSWORD, ENV_CALDAV_URL, ENV_CALDAV_USERNAME, ENV_CONFIG, ENV_VAULT,
};
use restask::daemon::DaemonConfig;
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
