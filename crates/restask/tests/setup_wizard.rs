//! Setup wizard tests (§13.2): the fresh TODO.md creation with rename-to-backup, the
//! typed inbox binding, the Obsidian plugin install, and the non-interactive full setup
//! against the in-memory CalDAV mock.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tempfile::TempDir;

use common::{FixedClock, MockCaldav};
use restask::cli;
use restask::config::{MachineConfig, VaultConfig};
use restask::markdown::MARKER;
use restask::setup::{
    daemon_unit_content, install_obsidian_plugin, match_collection, parse_collections, run_setup,
    DaemonInstaller, SetupArgs,
};
use restask::store::Index;
use restask::RestaskError;

fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ))
}

/// One hour after [`clock`], so a second setup run stamps a distinct backup name.
fn later_clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 9, 22, 13, 0, 0).unwrap(),
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ))
}

/// The exact fresh TODO.md scaffold: byte-identical to the §7 render of an empty vault.
fn fresh_todo(inbox_list: &str) -> String {
    format!("---\nrestask-list: {inbox_list}\n---\n\n{MARKER}\n\n# TODO\n\n## Done\n")
}

/// A vault with a legacy TODO.md; no restask.toml, no `.restask/`.
fn legacy_vault() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("TODO.md"),
        "# My tasks\n\n- [ ] water the plants\n\nSome prose.\n\n```tasks\n- [ ] inside tasks block\n```\n",
    )
    .unwrap();
    dir
}

fn args(vault: &TempDir) -> SetupArgs {
    SetupArgs {
        vault: vault.path().to_path_buf(),
        config_path: vault.path().join("machine.toml"),
        url: "http://radicale.local:5232".to_string(),
        username: "me".to_string(),
        password_env: Some("RESTASK_TEST_PASS".to_string()),
        password_file: None,
        inbox_collection: None,
        collections: vec![("Home".to_string(), "home".to_string())],
    }
}

/// A [`DaemonInstaller`] that records `vault|exec` calls instead of touching the host
/// systemd session (§13.2 step 6 is a port — the suite must stay hermetic).
#[derive(Clone, Default)]
struct RecordingInstaller {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    fail: bool,
}

impl RecordingInstaller {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl DaemonInstaller for RecordingInstaller {
    fn install(&self, vault: &Path, exec: &Path) -> Result<Option<String>, RestaskError> {
        if self.fail {
            return Err(RestaskError::Validation {
                field: "daemon",
                reason: "injected failure".to_string(),
            });
        }
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}|{}", vault.display(), exec.display()));
        Ok(Some(format!("daemon: enabled for {}", vault.display())))
    }
}

#[test]
fn collection_flags_must_be_list_equals_collection() {
    assert_eq!(
        parse_collections(&["Home=home".to_string()]).unwrap(),
        vec![("Home".to_string(), "home".to_string())]
    );
    assert!(matches!(
        parse_collections(&["Home".to_string()]),
        Err(RestaskError::Validation { .. })
    ));
}

#[tokio::test]
async fn non_interactive_setup_end_to_end() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();

    let summary = run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();

    // Step 1: §14-default restask.toml written, `.restask/` created; the inbox list
    // stays the §14 default because no inbox binding was requested.
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.done_heading, "Done");
    assert_eq!(cfg.inbox_list, "inbox");
    assert!(vault.path().join(".restask").is_dir());
    assert!(summary.daemon.is_none());

    // Step 2: the original file was renamed verbatim to the backup; TODO.md is the exact
    // fresh scaffold with no tasks carried over.
    let backup_name = summary.backup.clone().unwrap();
    assert_eq!(backup_name, "TODO.pre-restask-20260922-140000.md");
    let backup = std::fs::read_to_string(vault.path().join(&backup_name)).unwrap();
    assert!(backup.contains("- [ ] water the plants"));
    assert!(backup.contains("```tasks"));
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert_eq!(todo, fresh_todo("inbox"));
    assert!(!todo.contains("water the plants"));

    // Steps 3–4: machine config records the endpoint, the env-var reference (never the
    // secret) and the vault path. Routing is not machine state: it lives in the notes.
    let machine = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(
        machine.caldav.url.as_deref(),
        Some("http://radicale.local:5232")
    );
    assert_eq!(machine.caldav.username.as_deref(), Some("me"));
    assert_eq!(
        machine.caldav.password_env.as_deref(),
        Some("RESTASK_TEST_PASS")
    );
    assert!(machine.caldav.password_file.is_none());
    assert_eq!(machine.vault.path.as_deref(), Some(vault.path()));
    let raw = std::fs::read_to_string(vault.path().join("machine.toml")).unwrap();
    assert!(!raw.contains("[[lists]]"), "{raw}");

    // MKCOL happened for the requested collection, and the first sync created the inbox
    // collection the fresh TODO.md routes to.
    assert_eq!(mock.collection_names(), vec!["home", "inbox"]);

    // Step 5: the first sync has nothing to register or push yet.
    let index = Index::load(&vault.path().join(".restask")).unwrap();
    assert!(index.entries.is_empty());
    assert!(mock.resource_names("inbox").is_empty());
}

#[tokio::test]
async fn setup_recreates_todo_md_on_rerun() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    let first = run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();

    let second = run_setup(args(&vault), mock.clone(), later_clock(), None)
        .await
        .unwrap();

    // Every run recreates TODO.md: the first fresh file was renamed to a second,
    // distinctly stamped backup; the original legacy content lives only in the first.
    assert_eq!(
        first.backup.as_deref(),
        Some("TODO.pre-restask-20260922-140000.md")
    );
    assert_eq!(
        second.backup.as_deref(),
        Some("TODO.pre-restask-20260922-150000.md")
    );
    let first_backup = std::fs::read_to_string(vault.path().join(first.backup.unwrap())).unwrap();
    assert!(first_backup.contains("- [ ] water the plants"));
    let second_backup = std::fs::read_to_string(vault.path().join(second.backup.unwrap())).unwrap();
    assert_eq!(second_backup, fresh_todo("inbox"));
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert_eq!(todo, fresh_todo("inbox"));
    let index = Index::load(&vault.path().join(".restask")).unwrap();
    assert!(index.entries.is_empty());
}

#[test]
fn setup_vault_falls_back_to_cwd_with_todo_md() {
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(vault.path().join("TODO.md"), "# Restask\n").unwrap();

    assert_eq!(
        cli::resolve_setup_vault_with(None, None, vault.path(), true).unwrap(),
        vault.path().to_path_buf()
    );

    let bare = tempfile::tempdir().unwrap();
    assert_eq!(
        cli::resolve_setup_vault_with(None, None, bare.path(), false).unwrap(),
        bare.path().to_path_buf()
    );
    assert!(matches!(
        cli::resolve_setup_vault_with(None, None, bare.path(), true),
        Err(RestaskError::Config { .. })
    ));
}

#[test]
fn setup_vault_prefers_flags_env_and_markers_over_the_fallback() {
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(vault.path().join("restask.toml"), "").unwrap();
    std::fs::write(vault.path().join("TODO.md"), "").unwrap();
    let nested = vault.path().join("sub");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        cli::resolve_setup_vault_with(None, Some("/env/vault"), Path::new("/nowhere"), true)
            .unwrap(),
        PathBuf::from("/env/vault")
    );
    assert_eq!(
        cli::resolve_setup_vault_with(None, None, &nested, true).unwrap(),
        vault.path().to_path_buf()
    );
    assert_eq!(
        cli::resolve_setup_vault_with(
            Some(Path::new("/srv/other")),
            Some("/env/vault"),
            &nested,
            true
        )
        .unwrap(),
        PathBuf::from("/srv/other")
    );
}

#[tokio::test]
async fn inbox_binding_retargets_todo_md_to_the_chosen_calendar() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    let mut setup_args = args(&vault);
    setup_args.inbox_collection = Some("Tasks".to_string());

    let summary = run_setup(setup_args, mock.clone(), clock(), None)
        .await
        .unwrap();

    // restask.toml records the chosen calendar as the inbox list; the fresh TODO.md
    // carries it in the frontmatter and its tasks sync to the `tasks` collection.
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.inbox_list, "tasks");
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert_eq!(todo, fresh_todo("tasks"));
    assert_eq!(
        summary.collections,
        vec!["tasks".to_string(), "home".to_string()]
    );
    assert!(mock.collection_names().iter().any(|slug| slug == "tasks"));
    assert!(mock.collection_names().iter().any(|slug| slug == "home"));

    let index = Index::load(&vault.path().join(".restask")).unwrap();
    assert!(index.entries.is_empty());
}

#[test]
fn from_flags_lifts_the_inbox_binding_out_of_collections() {
    let args = SetupArgs::from_flags(
        PathBuf::from("/vault"),
        PathBuf::from("/machine.toml"),
        Some("http://radicale.local:5232".to_string()),
        Some("me".to_string()),
        Some("RESTASK_TEST_PASS".to_string()),
        vec![
            ("Home".to_string(), "home".to_string()),
            ("inbox".to_string(), "Tasks".to_string()),
        ],
    )
    .unwrap();
    assert_eq!(args.inbox_collection.as_deref(), Some("Tasks"));
    assert_eq!(
        args.collections,
        vec![("Home".to_string(), "home".to_string())]
    );
}

#[test]
fn match_collection_is_case_insensitive_and_canonical() {
    let collections = vec![
        restask::caldav::CollectionInfo {
            href: "/me/inbox/".to_string(),
            slug: "inbox".to_string(),
            display_name: Some("Inbox".to_string()),
            supports_vtodo: true,
        },
        restask::caldav::CollectionInfo {
            href: "/me/Tasks/".to_string(),
            slug: "Tasks".to_string(),
            display_name: None,
            supports_vtodo: true,
        },
    ];

    assert_eq!(
        match_collection("tasks", &collections).map(|collection| collection.slug.as_str()),
        Some("Tasks")
    );
    assert_eq!(
        match_collection("  INBOX  ", &collections).map(|collection| collection.slug.as_str()),
        Some("inbox")
    );
    assert!(match_collection("", &collections).is_none());
    assert!(match_collection("family", &collections).is_none());
}

#[tokio::test]
async fn setup_installs_and_enables_the_daemon_unit() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    let installer = RecordingInstaller::default();

    let summary = run_setup(args(&vault), mock.clone(), clock(), Some(&installer))
        .await
        .unwrap();

    // The unit install happened exactly once, pointing at this vault and the running
    // binary; the summary surfaces the enablement note.
    let calls = installer.calls();
    assert_eq!(calls.len(), 1);
    let (recorded_vault, recorded_exec) = calls[0].split_once('|').unwrap();
    assert_eq!(Path::new(recorded_vault), vault.path());
    assert!(!recorded_exec.is_empty());
    assert!(summary
        .daemon
        .as_deref()
        .unwrap()
        .contains(&vault.path().display().to_string()));
}

#[tokio::test]
async fn daemon_install_failure_only_warns() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    let installer = RecordingInstaller {
        calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        fail: true,
    };

    // A daemon-unit failure must not fail the setup run — the sync already happened.
    let summary = run_setup(args(&vault), mock.clone(), clock(), Some(&installer))
        .await
        .unwrap();
    assert!(summary.daemon.is_none());
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.inbox_list, "inbox");
}

#[test]
fn daemon_unit_content_is_a_valid_user_unit() {
    let content = daemon_unit_content(
        Path::new("/srv/My Vault"),
        Path::new("/home/me/.cargo/bin/restask"),
    );
    assert!(content.contains(
        "ExecStart=\"/home/me/.cargo/bin/restask\" daemon --vault \"/srv/My Vault\""
    ));
    assert!(content.contains("Restart=on-failure"));
    assert!(content.contains("RestartSec=5"));
    assert!(content.contains("WantedBy=default.target"));
    assert!(content.contains("Description=restask sync daemon (vault <-> CalDAV)"));
}

#[tokio::test]
async fn rerunning_setup_never_deletes_inbox_tasks_from_the_server() {
    // Setup replaces TODO.md with a fresh file. The lines that left with the old file
    // must not be read as "the user deleted these": the server copies flow back in.
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();

    // Capture a task in the inbox and sync it.
    let todo = vault.path().join("TODO.md");
    let text = std::fs::read_to_string(&todo).unwrap();
    std::fs::write(&todo, format!("{text}- [ ] keep me\n")).unwrap();
    let engine = |clock: Arc<FixedClock>| {
        restask::sync::Engine::new(
            vault.path(),
            VaultConfig::load(&vault.path().join("restask.toml")).unwrap(),
            MachineConfig::load(&vault.path().join("machine.toml")).unwrap(),
            mock.clone(),
            clock,
        )
    };
    engine(clock()).reconcile().await.unwrap();
    assert_eq!(mock.resource_names("inbox").len(), 1);

    let summary = run_setup(args(&vault), mock.clone(), later_clock(), None)
        .await
        .unwrap();
    assert!(summary.backup.is_some());
    assert_eq!(
        mock.resource_names("inbox").len(),
        1,
        "the task is still on the server"
    );
    let todo = std::fs::read_to_string(&todo).unwrap();
    assert!(
        todo.contains("- [ ] keep me"),
        "and back in the fresh TODO.md:\n{todo}"
    );
    assert_eq!(summary.report.deletes, 0);
}

/// The plugin bundle the binary embeds (kept current by `npm run build`, App. C).
fn embedded_plugin_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/obsidian")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn plugin_dir(vault: &Path) -> PathBuf {
    vault.join(".obsidian/plugins/restask")
}

#[tokio::test]
async fn setup_installs_and_enables_the_obsidian_plugin() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();

    let summary = run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();

    // The three files Obsidian loads a plugin from, byte-identical to the built bundle.
    for name in ["main.js", "manifest.json", "styles.css"] {
        assert_eq!(
            std::fs::read_to_string(plugin_dir(vault.path()).join(name)).unwrap(),
            embedded_plugin_file(name),
            "{name}"
        );
    }
    assert!(embedded_plugin_file("manifest.json").contains("\"id\": \"restask\""));
    // Enabled; the default done heading needs no settings file.
    assert_eq!(
        std::fs::read_to_string(vault.path().join(".obsidian/community-plugins.json")).unwrap(),
        "[\n  \"restask\"\n]"
    );
    assert!(!plugin_dir(vault.path()).join("data.json").exists());
    assert!(summary.plugin.as_deref().unwrap().contains("enabled"));
}

#[test]
fn plugin_install_keeps_other_plugins_and_the_users_settings() {
    let vault = tempfile::tempdir().unwrap();
    let obsidian = vault.path().join(".obsidian");
    std::fs::create_dir_all(plugin_dir(vault.path())).unwrap();
    let enabled = obsidian.join("community-plugins.json");
    std::fs::write(&enabled, "[\"dataview\", \"obsidian-git\"]").unwrap();
    std::fs::write(
        plugin_dir(vault.path()).join("main.js"),
        "// an older build",
    )
    .unwrap();
    let settings = plugin_dir(vault.path()).join("data.json");
    std::fs::write(&settings, "{\"suggestWhileTyping\":false}").unwrap();
    let cfg = VaultConfig {
        done_heading: "Fatto".to_string(),
        ..VaultConfig::default()
    };

    install_obsidian_plugin(vault.path(), &cfg).unwrap();

    // restask joins the list, the others stay in order; the old build is replaced; the
    // existing settings are the user's, even though the vault's heading is not the default.
    assert_eq!(
        std::fs::read_to_string(&enabled).unwrap(),
        "[\n  \"dataview\",\n  \"obsidian-git\",\n  \"restask\"\n]"
    );
    assert_eq!(
        std::fs::read_to_string(plugin_dir(vault.path()).join("main.js")).unwrap(),
        embedded_plugin_file("main.js")
    );
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        "{\"suggestWhileTyping\":false}"
    );
}

#[test]
fn plugin_install_is_a_no_op_on_an_installed_vault() {
    let vault = tempfile::tempdir().unwrap();
    let cfg = VaultConfig::default();
    install_obsidian_plugin(vault.path(), &cfg).unwrap();
    // Obsidian (or the user) may keep the list in another layout: already listing the
    // plugin, it is not reformatted.
    let enabled = vault.path().join(".obsidian/community-plugins.json");
    std::fs::write(&enabled, "[\"restask\"]\n").unwrap();
    let tracked: Vec<PathBuf> = ["main.js", "manifest.json", "styles.css"]
        .iter()
        .map(|name| plugin_dir(vault.path()).join(name))
        .chain([enabled.clone()])
        .collect();
    let modified = |paths: &[PathBuf]| -> Vec<std::time::SystemTime> {
        paths
            .iter()
            .map(|path| std::fs::metadata(path).unwrap().modified().unwrap())
            .collect()
    };
    let before = modified(&tracked);

    let note = install_obsidian_plugin(vault.path(), &cfg).unwrap();

    assert!(note.contains("enabled"), "{note}");
    assert_eq!(
        modified(&tracked),
        before,
        "nothing rewritten (no Syncthing churn)"
    );
    assert_eq!(
        std::fs::read_to_string(&enabled).unwrap(),
        "[\"restask\"]\n"
    );
    let leftovers: Vec<_> = std::fs::read_dir(plugin_dir(vault.path()))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().ends_with("restask-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn plugin_install_never_overwrites_a_plugin_list_it_cannot_read() {
    // Unknown is not empty: a list restask cannot parse may still name the user's other
    // plugins, so it is left byte-for-byte and the user enables the plugin by hand.
    let vault = tempfile::tempdir().unwrap();
    let obsidian = vault.path().join(".obsidian");
    std::fs::create_dir_all(&obsidian).unwrap();
    let enabled = obsidian.join("community-plugins.json");
    for unreadable in ["[\"dataview\", ", "{\"dataview\": true}"] {
        std::fs::write(&enabled, unreadable).unwrap();

        let note = install_obsidian_plugin(vault.path(), &VaultConfig::default()).unwrap();

        assert_eq!(std::fs::read_to_string(&enabled).unwrap(), unreadable);
        assert!(note.contains("by hand"), "{note}");
        assert!(plugin_dir(vault.path()).join("main.js").is_file());
    }
}

#[test]
fn plugin_install_seeds_a_non_default_done_heading() {
    let vault = tempfile::tempdir().unwrap();
    let cfg = VaultConfig {
        done_heading: "Fatto".to_string(),
        ..VaultConfig::default()
    };

    install_obsidian_plugin(vault.path(), &cfg).unwrap();

    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugin_dir(vault.path()).join("data.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings, serde_json::json!({ "doneHeading": "Fatto" }));
}

#[cfg(unix)]
#[test]
fn plugin_install_leaves_a_symlinked_development_install_alone() {
    // A checkout linked into the vault (per file, or the whole folder) is managed by
    // hand: setup must not replace the links with copies, nor write through them.
    let checkout = tempfile::tempdir().unwrap();
    for name in ["main.js", "manifest.json", "styles.css"] {
        std::fs::write(checkout.path().join(name), "work in progress").unwrap();
    }
    let cfg = VaultConfig::default();

    let per_file = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(plugin_dir(per_file.path())).unwrap();
    for name in ["main.js", "manifest.json", "styles.css"] {
        std::os::unix::fs::symlink(
            checkout.path().join(name),
            plugin_dir(per_file.path()).join(name),
        )
        .unwrap();
    }
    install_obsidian_plugin(per_file.path(), &cfg).unwrap();

    let whole_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(whole_dir.path().join(".obsidian/plugins")).unwrap();
    std::os::unix::fs::symlink(checkout.path(), plugin_dir(whole_dir.path())).unwrap();
    install_obsidian_plugin(whole_dir.path(), &cfg).unwrap();

    for name in ["main.js", "manifest.json", "styles.css"] {
        let link = plugin_dir(per_file.path()).join(name);
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(checkout.path().join(name)).unwrap(),
            "work in progress"
        );
    }
    // Still enabled in both vaults.
    for vault in [per_file.path(), whole_dir.path()] {
        assert_eq!(
            std::fs::read_to_string(vault.join(".obsidian/community-plugins.json")).unwrap(),
            "[\n  \"restask\"\n]"
        );
    }
}
