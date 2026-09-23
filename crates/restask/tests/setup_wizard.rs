//! Setup wizard tests (§13.2): the fresh TODO.md creation with rename-to-backup, the
//! typed inbox binding, and the non-interactive full setup against the in-memory CalDAV
//! mock.

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
    daemon_unit_content, match_collection, parse_collections, run_setup, DaemonInstaller, SetupArgs,
};
use restask::store::Index;
use restask::TaskresError;

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
    fn install(&self, vault: &Path, exec: &Path) -> Result<Option<String>, TaskresError> {
        if self.fail {
            return Err(TaskresError::Validation {
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
        Err(TaskresError::Validation { .. })
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

    // Steps 3–5: machine config records the endpoint and the env-var reference (never
    // the secret), the vault path, and the Home→home binding.
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
    assert_eq!(machine.lists.len(), 1);
    assert_eq!(machine.lists[0].name, "Home");
    assert_eq!(machine.lists[0].collection, "home");

    // MKCOL happened for the flag binding; the inbox collection is only ensured once a
    // task needs it (the fresh TODO.md is empty and the mock server has no tasks).
    assert!(mock.collection_names().iter().any(|slug| slug == "home"));
    assert!(!mock.collection_names().iter().any(|slug| slug == "inbox"));

    // Step 6: the first sync has nothing to register or push yet.
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
    std::fs::write(vault.path().join("TODO.md"), "# Taskres\n").unwrap();

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
        Err(TaskresError::Config { .. })
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

    let machine = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(machine.lists.len(), 2);
    let inbox_binding = machine
        .lists
        .iter()
        .find(|binding| binding.collection == "tasks")
        .unwrap();
    assert_eq!(inbox_binding.name, "tasks");
    assert_eq!(inbox_binding.collection, "tasks");

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
    assert!(content.contains("Description=Taskres sync daemon (vault <-> Radicale)"));
}
