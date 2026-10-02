//! Setup wizard tests (§13.2): the fresh TODO.md creation with rename-to-backup, the
//! typed inbox binding, the Obsidian plugin install, the non-interactive full setup
//! against the in-memory CalDAV mock, joining a vault that is already set up, and where
//! the vault's one daemon is put — this machine, or a node that is handed the
//! credentials.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tempfile::TempDir;

use common::{FixedClock, MockCaldav};
use restask::cli;
use restask::config::{MachineConfig, VaultConfig};
use restask::setup::{
    daemon_unit_content, install_obsidian_plugin, is_loopback, joins, match_collection,
    parse_collections, run_setup, DaemonFlags, DaemonHost, DaemonInstaller, NodeAccess, NodeTarget,
    SetupArgs,
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
    common::sealed(&format!(
        "---\nrestask-list: {inbox_list}\n---\n# TODO\n\n## Done\n"
    ))
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
        join: false,
        daemon: DaemonHost::Here,
        password: None,
    }
}

/// The always-on server of these tests.
fn node() -> NodeTarget {
    NodeTarget {
        host: "homeserver".to_string(),
        vault: "/srv/sync/vault".to_string(),
        dir: "restask".to_string(),
    }
}

/// [`args`] for a computer whose vault is synced by the daemon on [`node`]: the password
/// is in memory only, as the wizard has it after it was typed.
fn node_args(vault: &TempDir) -> SetupArgs {
    SetupArgs {
        daemon: DaemonHost::Node(node()),
        password: Some(restask::setup::Secret::new("s3cret")),
        password_env: None,
        ..args(vault)
    }
}

/// [`args`] for a second machine joining `vault`: its own machine config, no bindings.
fn join_args(vault: &TempDir, machine: &TempDir) -> SetupArgs {
    SetupArgs {
        config_path: machine.path().join("config.toml"),
        collections: Vec::new(),
        join: true,
        ..args(vault)
    }
}

/// Every file under `dir` except the engine's state (`.restask/`), with its contents.
fn vault_files(dir: &Path) -> std::collections::BTreeMap<PathBuf, String> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != ".restask" {
                    walk(root, &path, out);
                }
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read_to_string(&path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// A vault as the first machine leaves it — set up, with one synced inbox task and a
/// routed note — and the server it synced with. The machine config written by that setup
/// is removed: it never rides the vault.
async fn set_up_vault() -> (TempDir, MockCaldav) {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();
    let todo = vault.path().join("TODO.md");
    let text = std::fs::read_to_string(&todo).unwrap();
    std::fs::write(&todo, format!("{text}- [ ] keep me\n")).unwrap();
    std::fs::write(
        vault.path().join("Home.md"),
        "---\nrestask-list: Home\n---\n- [ ] fix the gutter\n",
    )
    .unwrap();
    restask::sync::Engine::new(
        vault.path(),
        VaultConfig::load(&vault.path().join("restask.toml")).unwrap(),
        MachineConfig::load(&vault.path().join("machine.toml")).unwrap(),
        mock.clone(),
        clock(),
    )
    .reconcile()
    .await
    .unwrap();
    std::fs::remove_file(vault.path().join("machine.toml")).unwrap();
    (vault, mock)
}

/// A [`DaemonInstaller`] that records `vault|exec` calls instead of touching the host
/// systemd session (§13.2 step 6 is a port — the suite must stay hermetic), and what a
/// node was asked instead of reaching for one.
#[derive(Clone, Default)]
struct RecordingInstaller {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    fail: bool,
    /// One line per node call, in order: `prepare <host> | <TODO.md at that moment>` and
    /// `install <host> <vault there> <dir> <url> <username> <password> | <TODO.md>`.
    node_calls: Arc<std::sync::Mutex<Vec<String>>>,
    /// The vault the node calls look at when they record its TODO.md.
    watched: Option<PathBuf>,
    fail_prepare: bool,
    fail_node: bool,
}

impl RecordingInstaller {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn watching(vault: &TempDir) -> Self {
        Self {
            watched: Some(vault.path().to_path_buf()),
            ..Self::default()
        }
    }

    fn node_calls(&self) -> Vec<String> {
        self.node_calls.lock().unwrap().clone()
    }

    fn todo(&self) -> String {
        self.watched
            .as_ref()
            .and_then(|vault| std::fs::read_to_string(vault.join("TODO.md")).ok())
            .unwrap_or_default()
    }

    fn injected() -> RestaskError {
        RestaskError::Validation {
            field: "node",
            reason: "injected failure".to_string(),
        }
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

    fn prepare_node(&self, node: &NodeTarget) -> Result<(), RestaskError> {
        if self.fail_prepare {
            return Err(Self::injected());
        }
        self.node_calls
            .lock()
            .unwrap()
            .push(format!("prepare {} | {}", node.host, self.todo()));
        Ok(())
    }

    fn install_node(
        &self,
        node: &NodeTarget,
        _vault: &Path,
        access: NodeAccess<'_>,
    ) -> Result<String, RestaskError> {
        if self.fail_node {
            return Err(Self::injected());
        }
        self.node_calls.lock().unwrap().push(format!(
            "install {} {} {} {} {} {} | {}",
            node.host,
            node.vault,
            node.dir,
            access.url,
            access.username,
            access.password.expose(),
            self.todo()
        ));
        Ok(format!("daemon: running on {}", node.host))
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

/// Setup writes nothing into the body of TODO.md but the view (§7.3): no comment line,
/// and a view from before that is replaced by one without them.
#[tokio::test]
async fn setup_writes_a_view_without_comment_lines() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();
    let todo = vault.path().join("TODO.md");
    let created = std::fs::read_to_string(&todo).unwrap();
    assert_eq!(created, fresh_todo("inbox"));
    assert!(!created.contains("<!--"));
    std::fs::write(
        &todo,
        "---\nrestask-list: inbox\n---\n<!-- AUTOGENERATED BY Restask -->\n<!-- restask-render: f76adea5b8d0980f -->\n\n# TODO\n\n## Done\n",
    )
    .unwrap();

    run_setup(args(&vault), mock.clone(), later_clock(), None)
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&todo).unwrap(), fresh_todo("inbox"));
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
        false,
    )
    .unwrap();
    assert_eq!(args.inbox_collection.as_deref(), Some("Tasks"));
    assert_eq!(
        args.collections,
        vec![("Home".to_string(), "home".to_string())]
    );
}

#[test]
fn from_flags_refuses_bindings_on_a_join() {
    let flags = |collections: Vec<(String, String)>| {
        SetupArgs::from_flags(
            PathBuf::from("/vault"),
            PathBuf::from("/machine.toml"),
            Some("http://radicale.local:5232".to_string()),
            Some("me".to_string()),
            Some("RESTASK_TEST_PASS".to_string()),
            collections,
            true,
        )
    };
    assert!(flags(Vec::new()).unwrap().join);
    assert!(matches!(
        flags(vec![("inbox".to_string(), "Tasks".to_string())]),
        Err(RestaskError::Validation {
            field: "collection",
            ..
        })
    ));
}

#[tokio::test]
async fn setup_joins_a_vault_that_is_set_up_when_the_machine_has_no_config() {
    let (vault, _mock) = set_up_vault().await;
    let machine = tempfile::tempdir().unwrap();
    let config = machine.path().join("config.toml");

    // The second machine: the vault arrived whole, the machine config did not.
    assert!(joins(vault.path(), &config, false).unwrap());
    assert!(joins(vault.path(), &config, true).unwrap());

    // The machine that set the vault up: a plain re-run is the full setup, as before;
    // `--join` still joins.
    std::fs::write(&config, "").unwrap();
    assert!(!joins(vault.path(), &config, false).unwrap());
    assert!(joins(vault.path(), &config, true).unwrap());
}

#[test]
fn join_is_refused_on_a_vault_that_is_not_set_up() {
    // A folder the file sync has not filled yet, a legacy vault, and a vault whose
    // restask.toml arrived before its TODO.md: a fresh setup there is the user's call,
    // never what `--join` silently turns into.
    let config = Path::new("/nowhere/config.toml");
    let empty = tempfile::tempdir().unwrap();
    let legacy = legacy_vault();
    let half = legacy_vault();
    std::fs::write(half.path().join("restask.toml"), "").unwrap();

    for vault in [empty.path(), legacy.path(), half.path()] {
        assert!(!joins(vault, config, false).unwrap());
        assert!(matches!(
            joins(vault, config, true),
            Err(RestaskError::Validation { field: "join", .. })
        ));
    }
}

#[tokio::test]
async fn joining_configures_the_machine_and_leaves_the_vault_as_it_is() {
    let (vault, mock) = set_up_vault().await;
    let machine = tempfile::tempdir().unwrap();
    let before = vault_files(vault.path());
    let on_server = (mock.resource_names("inbox"), mock.resource_names("home"));
    let (puts, deletes, _) = mock.counters();
    let installer = RecordingInstaller::default();

    let summary = run_setup(
        join_args(&vault, &machine),
        mock.clone(),
        later_clock(),
        Some(&installer),
    )
    .await
    .unwrap();

    // No fresh TODO.md, no backup, no plugin install, no file rewritten: what the other
    // devices hold is not touched by a machine joining.
    assert!(summary.joined);
    assert!(summary.backup.is_none());
    assert!(summary.plugin.is_none());
    assert!(summary.collections.is_empty());
    assert_eq!(vault_files(vault.path()), before);
    assert!(before
        .get(Path::new("TODO.md"))
        .unwrap()
        .contains("keep me"));

    // The server is as the first machine left it: the pass had nothing to send.
    assert_eq!(
        (mock.resource_names("inbox"), mock.resource_names("home")),
        on_server
    );
    let (puts_after, deletes_after, _) = mock.counters();
    assert_eq!(
        (puts_after, deletes_after),
        (puts, deletes),
        "no write request"
    );
    assert_eq!(summary.report.deletes, 0);

    // This machine: its config (endpoint, secret reference, this vault) and the unit.
    let config = MachineConfig::load(&machine.path().join("config.toml")).unwrap();
    assert_eq!(
        config.caldav.url.as_deref(),
        Some("http://radicale.local:5232")
    );
    assert_eq!(
        config.caldav.password_env.as_deref(),
        Some("RESTASK_TEST_PASS")
    );
    assert_eq!(config.vault.path.as_deref(), Some(vault.path()));
    assert_eq!(installer.calls().len(), 1);
    assert!(installer.calls()[0].starts_with(&vault.path().display().to_string()));
}

#[tokio::test]
async fn a_failed_join_records_nothing_so_the_next_run_joins_again() {
    let (vault, mock) = set_up_vault().await;
    let machine = tempfile::tempdir().unwrap();
    let config = machine.path().join("config.toml");
    let before = vault_files(vault.path());
    let installer = RecordingInstaller::default();

    mock.fail_next(restask::CaldavErrorKind::Auth);
    let failed = run_setup(
        join_args(&vault, &machine),
        mock.clone(),
        later_clock(),
        Some(&installer),
    )
    .await;

    // Wrong credentials: no machine config, no unit, vault untouched — and the run after
    // it is still a join, not a fresh setup that would replace TODO.md.
    assert!(matches!(
        failed,
        Err(RestaskError::Caldav {
            kind: restask::CaldavErrorKind::Auth,
            ..
        })
    ));
    assert!(!config.exists());
    assert!(installer.calls().is_empty());
    assert_eq!(vault_files(vault.path()), before);
    assert!(joins(vault.path(), &config, false).unwrap());
}

#[test]
fn match_collection_is_case_insensitive_and_canonical() {
    let collections = vec![
        restask::caldav::CollectionInfo {
            href: "/me/inbox/".to_string(),
            slug: "inbox".to_string(),
            display_name: Some("Inbox".to_string()),
            supports_vtodo: true,
            ctag: None,
        },
        restask::caldav::CollectionInfo {
            href: "/me/Tasks/".to_string(),
            slug: "Tasks".to_string(),
            display_name: None,
            supports_vtodo: true,
            ctag: None,
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
async fn a_machine_that_is_not_the_sync_node_gets_no_daemon_unit_and_keeps_no_credentials() {
    // `--no-daemon`: the daemon is installed by hand on another machine. Setup does
    // everything else — the first sync included — and leaves the installer alone; the
    // machine config says where the server is, not how to log in.
    let installer = RecordingInstaller::default();
    let vault = legacy_vault();
    std::fs::write(
        vault.path().join("radicale.passwd"),
        "from an earlier setup",
    )
    .unwrap();
    let no_daemon = SetupArgs {
        daemon: DaemonHost::Elsewhere,
        ..args(&vault)
    };
    let summary = run_setup(no_daemon, MockCaldav::new(), clock(), Some(&installer))
        .await
        .unwrap();
    assert!(summary.daemon.as_deref().unwrap().contains("--no-daemon"));
    assert!(summary.synced);
    assert_eq!(summary.report.scanned_files, 1);
    let machine = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(machine.caldav.url, None);
    assert_eq!(machine.caldav.password_env, None);
    assert_eq!(machine.caldav.password_file, None);
    let recorded = machine.node.unwrap();
    assert_eq!(recorded.host, None);
    assert_eq!(recorded.url.as_deref(), Some("http://radicale.local:5232"));
    // The password file of an earlier setup, next to the config, goes with the role.
    assert!(!vault.path().join("radicale.passwd").exists());

    // A join of such a machine makes no pass: that is the sync node's.
    let (vault, mock) = set_up_vault().await;
    let machine = TempDir::new().unwrap();
    let (puts, deletes, reports) = mock.counters();
    let no_daemon = SetupArgs {
        daemon: DaemonHost::Elsewhere,
        ..join_args(&vault, &machine)
    };
    let summary = run_setup(no_daemon, mock.clone(), later_clock(), Some(&installer))
        .await
        .unwrap();
    assert!(summary.joined);
    assert!(!summary.synced);
    assert!(summary.daemon.as_deref().unwrap().contains("--no-daemon"));
    assert_eq!(mock.counters(), (puts, deletes, reports));
    let config = MachineConfig::load(&machine.path().join("config.toml")).unwrap();
    assert_eq!(config.caldav.url, None);
    assert!(config.node.is_some());

    assert!(installer.calls().is_empty());
    assert!(installer.node_calls().is_empty());
}

#[tokio::test]
async fn setup_puts_the_daemon_on_the_node_with_the_credentials_typed_here() {
    // The owner's case: one run on the computer. The node is looked at before the vault
    // is touched, the first sync is made from here, and the node then gets the daemon
    // with the same URL, user and password — which this machine does not keep.
    let vault = legacy_vault();
    let legacy = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    std::fs::write(
        vault.path().join("radicale.passwd"),
        "from an earlier setup",
    )
    .unwrap();
    let mock = MockCaldav::new();
    let installer = RecordingInstaller::watching(&vault);

    let summary = run_setup(node_args(&vault), mock.clone(), clock(), Some(&installer))
        .await
        .unwrap();

    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert_eq!(todo, fresh_todo("inbox"));
    assert_eq!(
        installer.node_calls(),
        vec![
            format!("prepare homeserver | {legacy}"),
            format!(
                "install homeserver /srv/sync/vault restask http://radicale.local:5232 \
                 me s3cret | {todo}"
            ),
        ]
    );
    assert!(installer.calls().is_empty(), "no unit on this machine");
    assert!(summary.synced);
    assert_eq!(mock.collection_names(), vec!["home", "inbox"]);
    assert_eq!(
        summary.daemon.as_deref(),
        Some("daemon: running on homeserver")
    );

    // This machine only edits from now on: where the daemon is, and nothing to log in
    // with — not in the config, not in a file beside it.
    let raw = std::fs::read_to_string(vault.path().join("machine.toml")).unwrap();
    assert!(!raw.contains("s3cret"), "{raw}");
    let machine = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(machine.caldav.url, None);
    assert_eq!(machine.caldav.username, None);
    assert_eq!(machine.caldav.password_env, None);
    assert_eq!(machine.caldav.password_file, None);
    assert_eq!(
        machine.node,
        Some(restask::config::NodeSection {
            host: Some("homeserver".to_string()),
            dir: Some("restask".to_string()),
            vault: Some("/srv/sync/vault".to_string()),
            url: Some("http://radicale.local:5232".to_string()),
            username: Some("me".to_string()),
        })
    );
    assert_eq!(machine.vault.path.as_deref(), Some(vault.path()));
    assert!(!vault.path().join("radicale.passwd").exists());
}

#[tokio::test]
async fn a_node_that_cannot_run_the_daemon_leaves_the_vault_untouched() {
    // No Docker there, the vault not shared with it, ssh refused: the wizard stops
    // before it replaces TODO.md or contacts the task server.
    let vault = legacy_vault();
    let before = vault_files(vault.path());
    let mock = MockCaldav::new();
    let installer = RecordingInstaller {
        fail_prepare: true,
        ..RecordingInstaller::default()
    };

    let failed = run_setup(node_args(&vault), mock.clone(), clock(), Some(&installer)).await;

    assert!(failed.is_err());
    assert_eq!(vault_files(vault.path()), before);
    assert!(!vault.path().join(".restask").exists());
    assert!(!vault.path().join("machine.toml").exists());
    assert!(mock.collection_names().is_empty());

    // The node's daemon runs in a container: an address that means "this computer"
    // cannot be the one it is given.
    let local = SetupArgs {
        url: "http://localhost:5232".to_string(),
        ..node_args(&vault)
    };
    let refused = run_setup(local, mock.clone(), clock(), None).await;
    assert!(matches!(
        refused,
        Err(RestaskError::Validation { field: "url", .. })
    ));
    assert_eq!(vault_files(vault.path()), before);
}

#[tokio::test]
async fn a_node_install_that_fails_is_finished_by_a_join() {
    // The vault is set up and synced, the daemon is not running: the run fails, says how
    // to finish, and records no machine config — so nothing takes the setup for done.
    let vault = legacy_vault();
    let machine_dir = TempDir::new().unwrap();
    let config = machine_dir.path().join("config.toml");
    let node_args = |vault: &TempDir| SetupArgs {
        config_path: config.clone(),
        ..node_args(vault)
    };
    let mock = MockCaldav::new();
    let failing = RecordingInstaller {
        fail_node: true,
        ..RecordingInstaller::default()
    };
    let failed = run_setup(node_args(&vault), mock.clone(), clock(), Some(&failing)).await;
    let Err(RestaskError::Validation { field, reason }) = failed else {
        panic!("a failed node install must fail the run");
    };
    assert_eq!(field, "node");
    assert!(
        reason.contains("restask setup --join --node homeserver --node-vault \"/srv/sync/vault\""),
        "{reason}"
    );
    assert!(!config.exists());
    assert_eq!(
        std::fs::read_to_string(vault.path().join("TODO.md")).unwrap(),
        fresh_todo("inbox")
    );

    // The same command with --join (and a plain `restask setup`: the vault is set up and
    // the machine has no config) installs the daemon and changes nothing else: no file
    // of the vault, no request that writes — and no pass from this machine at all.
    assert!(joins(vault.path(), &config, false).unwrap());
    let before = vault_files(vault.path());
    let (puts, deletes, reports) = mock.counters();
    let installer = RecordingInstaller::default();
    let join = SetupArgs {
        join: true,
        collections: Vec::new(),
        ..node_args(&vault)
    };
    let summary = run_setup(join, mock.clone(), later_clock(), Some(&installer))
        .await
        .unwrap();
    assert!(summary.joined);
    assert!(!summary.synced);
    assert_eq!(vault_files(vault.path()), before);
    assert_eq!(mock.counters(), (puts, deletes, reports));
    assert_eq!(installer.node_calls().len(), 2);
    assert!(installer.node_calls()[1].starts_with("install homeserver /srv/sync/vault"));
    assert!(installer.calls().is_empty());
    let machine = MachineConfig::load(&config).unwrap();
    assert_eq!(machine.node.unwrap().host.as_deref(), Some("homeserver"));
}

#[test]
fn the_flags_say_where_the_daemon_goes() {
    assert_eq!(DaemonFlags::default().resolve().unwrap(), DaemonHost::Here);
    let no_daemon = DaemonFlags {
        no_daemon: true,
        ..DaemonFlags::default()
    };
    assert_eq!(no_daemon.resolve().unwrap(), DaemonHost::Elsewhere);

    // A node needs the vault's folder there; the stack directory has a default.
    let host_only = DaemonFlags {
        node: Some("homeserver".to_string()),
        ..DaemonFlags::default()
    };
    assert!(matches!(
        host_only.resolve(),
        Err(RestaskError::Validation {
            field: "node-vault",
            ..
        })
    ));
    let complete = DaemonFlags {
        node: Some("homeserver".to_string()),
        node_vault: Some("/srv/sync/vault".to_string()),
        ..DaemonFlags::default()
    };
    assert_eq!(complete.resolve().unwrap(), DaemonHost::Node(node()));
    let elsewhere = DaemonFlags {
        node: Some("homeserver".to_string()),
        node_vault: Some("/srv/sync/vault".to_string()),
        node_dir: Some("/opt/docker/restask".to_string()),
        ..DaemonFlags::default()
    };
    assert_eq!(
        elsewhere.resolve().unwrap(),
        DaemonHost::Node(NodeTarget {
            dir: "/opt/docker/restask".to_string(),
            ..node()
        })
    );
}

#[test]
fn an_address_of_this_machine_is_recognised() {
    for url in [
        "http://localhost:5232",
        "http://LOCALHOST",
        "https://127.0.0.1:5232/dav/",
        "http://user@127.0.1.1/",
        "http://[::1]:5232",
        "localhost:5232",
    ] {
        assert!(is_loopback(url), "{url}");
    }
    for url in [
        "http://192.168.1.10:5232",
        "https://dav.example.org/localhost/",
        "http://radicale.local:5232",
        "http://localhost.example.org",
    ] {
        assert!(!is_loopback(url), "{url}");
    }
}

#[test]
fn a_password_in_memory_is_never_printed() {
    let args = node_args(&legacy_vault());
    let printed = format!("{args:?}");
    assert!(!printed.contains("s3cret"), "{printed}");
}

#[tokio::test]
async fn daemon_install_failure_only_warns() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    let installer = RecordingInstaller {
        fail: true,
        ..RecordingInstaller::default()
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
