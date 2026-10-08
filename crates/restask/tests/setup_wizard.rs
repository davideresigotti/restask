//! Setup wizard tests (§13.2): the fresh TODO.md creation with rename-to-backup, the
//! typed inbox binding, the Obsidian plugin install and how a running Obsidian comes to
//! load it, the non-interactive full setup
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
    calendar_name, collection_label, daemon_unit_content, install_obsidian_plugin, is_loopback,
    is_obsidian_main, joins, match_collection, node_link_args, offered_collections, open_vault_id,
    parse_collections, run_setup, select_collections, DaemonFlags, DaemonHost, DaemonInstaller,
    NodeAccess, NodeTarget, ObsidianApp, PluginInstall, PluginLoad, SetupArgs,
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
        todo_collections: None,
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
    /// An ssh host that cannot be logged in to.
    unreachable: Option<String>,
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

    fn connect_node(&self, host: &str) -> Result<(), RestaskError> {
        self.node_calls
            .lock()
            .unwrap()
            .push(format!("connect {host}"));
        if self.unreachable.as_deref() == Some(host) {
            return Err(Self::injected());
        }
        Ok(())
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

/// §7.5, §13.2 step 4: the calendars TODO.md shows are recorded in `restask.toml`, and
/// the first sync brings their tasks into TODO.md, each line naming its calendar.
#[tokio::test]
async fn the_calendars_todo_md_shows_are_recorded_and_their_tasks_come_in() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    mock.seed_resource(
        "work",
        "from-phone",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:from-phone@tasks.org\r\n\
         CREATED:20260921T081233Z\r\nLAST-MODIFIED:20260922T101400Z\r\n\
         SUMMARY:Update restask README\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    );
    let mut setup_args = args(&vault);
    setup_args.inbox_collection = Some("Personal".to_string());
    // The bound calendar named again, a name twice, a display spelling: one entry each.
    setup_args.todo_collections = Some(vec![
        "Work".to_string(),
        "personal".to_string(),
        "Family Stuff".to_string(),
        "work".to_string(),
    ]);

    let summary = run_setup(setup_args, mock.clone(), clock(), None)
        .await
        .unwrap();

    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.inbox_list, "personal");
    assert_eq!(cfg.todo_lists, vec!["work", "family-stuff"]);
    assert_eq!(
        summary.collections,
        vec!["personal", "family-stuff", "work", "home"]
    );
    for calendar in ["personal", "family-stuff", "work"] {
        assert!(mock.collection_names().iter().any(|slug| slug == calendar));
    }
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(
        todo.contains("## No Priority\n- [ ] Update restask README 📁 work 🆔 restask-"),
        "{todo}"
    );
    assert_eq!(mock.resource_names("work"), vec!["from-phone"]);
    assert!(mock.resource_names("personal").is_empty());

    // A run that says nothing about them leaves the choice as it is; one that names
    // none clears it.
    let mut again = args(&vault);
    again.inbox_collection = Some("Personal".to_string());
    run_setup(again.clone(), mock.clone(), clock(), None)
        .await
        .unwrap();
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.todo_lists, vec!["work", "family-stuff"]);
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(todo.contains("Update restask README 📁 work"), "{todo}");
    again.todo_collections = Some(Vec::new());
    run_setup(again, mock.clone(), clock(), None).await.unwrap();
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert!(cfg.todo_lists.is_empty());
    assert_eq!(mock.resource_names("work"), vec!["from-phone"]);
}

#[tokio::test]
async fn setup_records_the_vaults_name_in_obsidian_once() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    run_setup(args(&vault), mock.clone(), clock(), None)
        .await
        .unwrap();
    let path = vault.path().join("restask.toml");
    let folder = std::fs::canonicalize(vault.path())
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        VaultConfig::load(&path).unwrap().obsidian_vault.as_deref(),
        Some(folder.as_str())
    );

    // A name the user corrected is kept by the next run.
    let mut cfg = VaultConfig::load(&path).unwrap();
    cfg.obsidian_vault = Some("2nd-brain".to_string());
    cfg.save(&path).unwrap();
    run_setup(args(&vault), mock, clock(), None).await.unwrap();
    assert_eq!(
        VaultConfig::load(&path).unwrap().obsidian_vault.as_deref(),
        Some("2nd-brain")
    );
}

#[test]
fn the_calendars_todo_md_shows_are_chosen_from_the_servers_by_name() {
    let server: Vec<restask::caldav::CollectionInfo> =
        ["personal", "work", "birthdays", "home-lab"]
            .iter()
            .map(|slug| restask::caldav::CollectionInfo {
                href: format!("/me/{slug}/"),
                slug: slug.to_string(),
                display_name: None,
                // `birthdays` holds events only.
                supports_vtodo: *slug != "birthdays",
                ctag: None,
            })
            .collect();
    let chosen = |typed: &str| {
        select_collections(typed, &server).map(|found| {
            found
                .iter()
                .map(|collection| collection.slug.clone())
                .collect::<Vec<_>>()
        })
    };
    // Enter: all that can hold tasks, in the server's order.
    assert_eq!(chosen("").unwrap(), vec!["personal", "work", "home-lab"]);
    assert_eq!(chosen(" , ").unwrap(), vec!["personal", "work", "home-lab"]);
    // In the order typed, any case, each once.
    assert_eq!(
        chosen("Work, personal,work ,").unwrap(),
        vec!["work", "personal"]
    );
    assert_eq!(chosen("home-lab").unwrap(), vec!["home-lab"]);
    // A name the server does not have is asked for again.
    assert_eq!(chosen("work, wrok").unwrap_err(), "wrok");
    // One that was typed is taken at the user's word.
    assert_eq!(chosen("birthdays").unwrap(), vec!["birthdays"]);
}

#[test]
fn the_wizards_checklist_offers_every_calendar_that_holds_tasks_under_a_name_the_user_knows() {
    let calendar =
        |slug: &str, display: Option<&str>, tasks: bool| restask::caldav::CollectionInfo {
            href: format!("/me/{slug}/"),
            slug: slug.to_string(),
            display_name: display.map(str::to_string),
            supports_vtodo: tasks,
            ctag: None,
        };
    let server = vec![
        calendar("personal", Some("Personal"), true),
        calendar("birthdays", None, false),
        calendar("work", None, true),
        // Made in another client: the path is not the name the user gave it.
        calendar(
            "0b1f6c1e-3a52-4c0e-9d58-0f3c2f6f1a77",
            Some("University"),
            true,
        ),
        calendar("home-lab", Some("Home Lab"), true),
    ];
    // All that can hold tasks, in the server's order — the list opens with each ticked.
    let offered = offered_collections(&server);
    assert_eq!(
        offered
            .iter()
            .map(|collection| collection_label(collection, &server))
            .collect::<Vec<_>>(),
        vec!["personal", "work", "university", "home-lab (Home Lab)"]
    );
    // What is written into `restask.toml` is the name, never the path nobody chose.
    assert_eq!(
        offered
            .iter()
            .map(|collection| calendar_name(collection, &server))
            .collect::<Vec<_>>(),
        vec!["personal", "work", "university", "home-lab"]
    );
    // Enter on the typed question is the same choice, and a name can be typed.
    assert_eq!(select_collections("", &server).unwrap(), offered);
    assert_eq!(
        select_collections("University, Home-Lab", &server).unwrap(),
        vec![offered[2], offered[3]]
    );
}

/// §5.4: setup creates a calendar only when the server has none of that name — one made
/// in another client is found, and gets no empty twin at the name's own path.
#[tokio::test]
async fn setup_takes_a_calendar_made_in_another_client_by_its_name() {
    let vault = tempfile::tempdir().unwrap();
    let mock = MockCaldav::new();
    let phone = "56de6126-33a4-46fd-a66e-3cc49ad32fe5";
    mock.seed_collection(phone, "Prova");
    mock.seed_resource(
        phone,
        "from-phone",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:from-phone@tasks.org\r\n\
         CREATED:20260921T081233Z\r\nLAST-MODIFIED:20260922T101400Z\r\n\
         SUMMARY:Try the new list\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    );
    let mut setup_args = args(&vault);
    setup_args.inbox_collection = Some("personal".to_string());
    setup_args.todo_collections = Some(vec!["prova".to_string()]);
    setup_args.collections = Vec::new();

    run_setup(setup_args, mock.clone(), clock(), None)
        .await
        .unwrap();

    assert_eq!(mock.collection_names(), vec![phone, "personal"]);
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(
        todo.contains("- [ ] Try the new list 📁 prova 🆔 restask-"),
        "{todo}"
    );
    assert_eq!(mock.resource_names(phone), vec!["from-phone"]);
}

#[test]
fn the_todo_list_flags_are_refused_on_a_join_and_checked() {
    let flags = |join: bool| {
        SetupArgs::from_flags(
            PathBuf::from("/vault"),
            PathBuf::from("/machine.toml"),
            Some("http://radicale.local:5232".to_string()),
            Some("me".to_string()),
            Some("RESTASK_TEST_PASS".to_string()),
            Vec::new(),
            join,
        )
        .unwrap()
    };
    assert_eq!(
        flags(false).showing(Vec::new()).unwrap().todo_collections,
        None
    );
    assert_eq!(
        flags(false)
            .showing(vec!["work".to_string()])
            .unwrap()
            .todo_collections,
        Some(vec!["work".to_string()])
    );
    assert!(flags(true).showing(Vec::new()).unwrap().join);
    assert!(matches!(
        flags(true).showing(vec!["work".to_string()]),
        Err(RestaskError::Validation {
            field: "todo-list",
            ..
        })
    ));
    assert!(flags(false).showing(vec!["***".to_string()]).is_err());
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
async fn the_machine_config_is_written_on_a_machine_that_has_no_config_directory_yet() {
    // A computer restask was never set up on has no `~/.config/restask/`. Only the sync
    // node's password file used to create it, so the wizard's run with the daemon on a
    // server installed the daemon there and then failed on its last write, here.
    let home = TempDir::new().unwrap();
    let config = home.path().join(".config/restask/config.toml");
    let hosts = [
        DaemonHost::Node(node()),
        DaemonHost::Elsewhere,
        DaemonHost::Here,
    ];
    for daemon in hosts {
        let vault = legacy_vault();
        let installer = RecordingInstaller::default();
        let editing = daemon != DaemonHost::Here;
        let fresh = SetupArgs {
            config_path: config.clone(),
            daemon,
            ..node_args(&vault)
        };
        run_setup(fresh, MockCaldav::new(), clock(), Some(&installer))
            .await
            .unwrap();
        let machine = MachineConfig::load(&config).unwrap();
        assert_eq!(machine.node.is_some(), editing);
        assert_eq!(machine.vault.path.as_deref(), Some(vault.path()));
        std::fs::remove_dir_all(home.path().join(".config")).unwrap();
    }

    // The same for a join: the run that finishes a setup whose last step failed.
    let (vault, mock) = set_up_vault().await;
    let installer = RecordingInstaller::default();
    let join = SetupArgs {
        config_path: config.clone(),
        join: true,
        collections: Vec::new(),
        ..node_args(&vault)
    };
    run_setup(join, mock, later_clock(), Some(&installer))
        .await
        .unwrap();
    let machine = MachineConfig::load(&config).unwrap();
    assert_eq!(machine.node.unwrap().host.as_deref(), Some("homeserver"));
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

/// What the wizard asked and what it did on the node, in order: `ask <question>` and the
/// installer's `connect <host>`. `answers` are typed in turn.
fn asked(
    flags: DaemonFlags,
    installer: &RecordingInstaller,
    answers: &[&str],
) -> (Result<DaemonHost, RestaskError>, Vec<String>) {
    let mut answers = answers.iter();
    let log = installer.node_calls.clone();
    let host = flags.ask(Some(installer), &mut |question| {
        log.lock().unwrap().push(format!("ask {question}"));
        Ok(answers
            .next()
            .expect("the wizard asked more than the test answers")
            .to_string())
    });
    (host, installer.node_calls())
}

#[test]
fn the_server_is_connected_to_as_soon_as_its_ssh_host_is_typed() {
    // ssh asks for what it needs to log in — a password, where no key is set up — when
    // the connection is opened. That is right after the host is typed and before the
    // next question, not minutes later in the middle of the install.
    const HOST: &str = "ask ssh host of that server (Enter: this computer does it):";
    let installer = RecordingInstaller::default();
    let (host, log) = asked(
        DaemonFlags::default(),
        &installer,
        &["homeserver", "", "/srv/sync/vault"],
    );
    assert_eq!(host.unwrap(), DaemonHost::Node(node()));
    assert_eq!(
        log,
        vec![
            HOST,
            "connect homeserver",
            "ask Vault folder on homeserver:",
            "ask Vault folder on homeserver:",
        ]
    );

    // A host that cannot be logged in to (wrong password, wrong name) is asked for
    // again, there and then; Enter gives up on the server.
    let installer = RecordingInstaller {
        unreachable: Some("homserver".to_string()),
        ..RecordingInstaller::default()
    };
    let (host, log) = asked(
        DaemonFlags::default(),
        &installer,
        &["homserver", "homeserver", "/srv/sync/vault"],
    );
    assert_eq!(host.unwrap(), DaemonHost::Node(node()));
    assert_eq!(
        log,
        vec![
            HOST,
            "connect homserver",
            HOST,
            "connect homeserver",
            "ask Vault folder on homeserver:",
        ]
    );
    let (host, log) = asked(DaemonFlags::default(), &installer, &["homserver", ""]);
    assert_eq!(host.unwrap(), DaemonHost::Here);
    assert_eq!(log.last().map(String::as_str), Some(HOST));

    // No server, no connection.
    let installer = RecordingInstaller::default();
    let (host, log) = asked(DaemonFlags::default(), &installer, &[""]);
    assert_eq!(host.unwrap(), DaemonHost::Here);
    assert_eq!(log, vec![HOST]);
    let no_daemon = DaemonFlags {
        no_daemon: true,
        ..DaemonFlags::default()
    };
    let (host, log) = asked(no_daemon, &installer, &[]);
    assert_eq!(host.unwrap(), DaemonHost::Elsewhere);
    assert_eq!(log, vec![HOST]);

    // A host named by `--node` is connected to before anything is asked, and is not
    // asked for again when it cannot be reached: the flag was the answer.
    let flags = |host: &str| DaemonFlags {
        node: Some(host.to_string()),
        node_vault: Some("/srv/sync/vault".to_string()),
        ..DaemonFlags::default()
    };
    let installer = RecordingInstaller {
        unreachable: Some("homserver".to_string()),
        ..RecordingInstaller::default()
    };
    let (host, log) = asked(flags("homeserver"), &installer, &[]);
    assert_eq!(host.unwrap(), DaemonHost::Node(node()));
    assert_eq!(log, vec!["connect homeserver"]);
    let (host, _) = asked(flags("homserver"), &installer, &[]);
    assert!(host.is_err());
}

#[test]
fn the_connection_to_the_server_is_opened_once_and_kept_for_the_run() {
    // One master connection behind a socket, left in the background: ssh asks once,
    // and the host can never be read as an option.
    assert_eq!(
        node_link_args("/run/user/1000/restask-ssh-7/%C", "me@homeserver"),
        vec![
            "-o",
            "ControlMaster=yes",
            "-o",
            "ControlPath=/run/user/1000/restask-ssh-7/%C",
            "-o",
            "ControlPersist=900",
            "--",
            "me@homeserver",
            "true",
        ]
    );
}

/// Runs `contrib/node.sh check` with an `ssh` that only records its arguments, one line
/// per call, and returns those lines. `control` is setup's open connection, if any.
#[cfg(unix)]
fn node_script_ssh_calls(control: Option<&str>) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = TempDir::new().unwrap();
    let log = bin.path().join("calls");
    let fake = bin.path().join("ssh");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >>'{}'\ncat >/dev/null\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut script = std::process::Command::new("bash");
    script
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrib/node.sh"))
        .args(["check", "homeserver", "/srv/sync/vault"])
        .env("PATH", path)
        .env_remove(restask::setup::ENV_SSH_CONTROL)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if let Some(control) = control {
        script.env(restask::setup::ENV_SSH_CONTROL, control);
    }
    assert!(script.status().unwrap().success());
    std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
#[test]
fn the_node_script_uses_the_connection_setup_opened_and_leaves_it_open() {
    // Setup logged in when the host was typed. The script's ssh calls go through that
    // connection — a second one would ask for the password again — and do not close
    // it: the install that follows needs it.
    let calls = node_script_ssh_calls(Some("/run/user/1000/restask-ssh-7/%C"));
    assert!(!calls.is_empty());
    for call in &calls {
        assert!(
            call.contains("ControlPath=/run/user/1000/restask-ssh-7/%C"),
            "{call}"
        );
        assert!(!call.contains("-O exit"), "{call}");
    }

    // Run by itself (`contrib/update.sh`) the script has a connection of its own for
    // the run, and closes it.
    let calls = node_script_ssh_calls(None);
    assert!(calls.len() > 1);
    assert!(calls.iter().all(|call| !call.contains("restask-ssh-7")));
    assert!(calls.last().unwrap().contains("-O exit"), "{calls:?}");
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
    assert_eq!(
        summary.plugin,
        Some(PluginInstall {
            listed: true,
            changed: true
        })
    );
    // No Obsidian was looked for: the line says what to do if one has the vault open.
    assert_eq!(
        summary.plugin_note().unwrap(),
        "obsidian plugin: installed in .obsidian/plugins/restask and enabled (restart \
         Obsidian if this vault is open)"
    );
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

    let install = install_obsidian_plugin(vault.path(), &cfg).unwrap();

    assert_eq!(
        install,
        PluginInstall {
            listed: true,
            changed: false
        }
    );
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

        let install = install_obsidian_plugin(vault.path(), &VaultConfig::default()).unwrap();

        assert_eq!(std::fs::read_to_string(&enabled).unwrap(), unreadable);
        assert!(!install.listed);
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

/// An [`ObsidianApp`] that records what it was asked instead of reaching for the user's
/// Obsidian: whether it has the vault open, what its command line interface answers
/// (`None`: switched off), whether a restart works.
#[derive(Default)]
struct RecordingObsidian {
    open: bool,
    /// Answers by the command's first word; a command not listed is refused the way
    /// Obsidian refuses it.
    cli: Option<Vec<(&'static str, &'static str)>>,
    fail_restart: bool,
    /// One line per call: `open?`, the command line, `restart`.
    calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingObsidian {
    fn with_cli(answers: &[(&'static str, &'static str)]) -> Self {
        Self {
            open: true,
            cli: Some(answers.to_vec()),
            ..Self::default()
        }
    }

    fn without_cli() -> Self {
        Self {
            open: true,
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl ObsidianApp for RecordingObsidian {
    fn has_open(&self, _vault: &Path) -> bool {
        self.calls.lock().unwrap().push("open?".to_string());
        self.open
    }

    fn command(&self, _vault: &Path, args: &[&str]) -> Result<Option<String>, RestaskError> {
        self.calls.lock().unwrap().push(args.join(" "));
        Ok(self.cli.as_ref().map(|answers| {
            answers
                .iter()
                .find(|(command, _)| *command == args[0])
                .map_or("Error: Plugin \"restask\" not found.", |(_, answer)| answer)
                .to_string()
        }))
    }

    fn restart(&self) -> Result<(), RestaskError> {
        self.calls.lock().unwrap().push("restart".to_string());
        if self.fail_restart {
            return Err(RestaskError::Validation {
                field: "obsidian",
                reason: "injected failure".to_string(),
            });
        }
        Ok(())
    }
}

/// A fresh setup of a vault, as far as its summary: the plugin is new in the vault.
async fn set_up_summary() -> (TempDir, restask::setup::SetupSummary) {
    let vault = legacy_vault();
    let summary = run_setup(args(&vault), MockCaldav::new(), clock(), None)
        .await
        .unwrap();
    (vault, summary)
}

/// What Obsidian's command line answers, what setup makes of it, the calls it took, and
/// a part of the summary line.
type Case = (
    &'static [(&'static str, &'static str)],
    PluginLoad,
    &'static [&'static str],
    &'static str,
);

#[tokio::test]
async fn an_obsidian_that_has_the_vault_open_loads_the_plugin_when_setup_ends() {
    // The owner's run: Obsidian was open on the vault, read its plugins hours before,
    // and setup's entry in the list changed nothing in it. With Obsidian's command line
    // interface on, setup asks Obsidian itself — the gentlest command that works.
    const RELOAD: &str = "plugin:reload id=restask";
    const ENABLE: &str = "plugin:enable id=restask filter=community";
    let cases: [Case; 4] = [
        // The plugin was on (an earlier setup): its new files are loaded.
        (
            &[
                ("plugins:restrict", "off"),
                ("plugin:reload", "Reloaded: restask"),
            ],
            PluginLoad::Loaded,
            &["open?", "plugins:restrict", RELOAD],
            "the Obsidian that has this vault open loaded it",
        ),
        // Obsidian knows the folder, the plugin was off: it is turned on.
        (
            &[
                ("plugins:restrict", "off"),
                ("plugin:reload", "Error: Plugin \"restask\" is not enabled."),
                ("plugin:enable", "Enabled: restask"),
            ],
            PluginLoad::Loaded,
            &["open?", "plugins:restrict", RELOAD, ENABLE],
            "the Obsidian that has this vault open loaded it",
        ),
        // The folder is new to this Obsidian: the vault's window is reloaded.
        (
            &[("plugins:restrict", "off"), ("reload", "Reloading...")],
            PluginLoad::Reloaded,
            &["open?", "plugins:restrict", RELOAD, ENABLE, "reload"],
            "Obsidian reloaded this vault's window and loaded it",
        ),
        // Restricted mode is the user's decision: nothing is loaded behind it.
        (
            &[
                ("plugins:restrict", "on"),
                ("plugin:enable", "Enabled: restask"),
            ],
            PluginLoad::Restricted,
            &["open?", "plugins:restrict"],
            "restricted mode",
        ),
    ];
    for (answers, expected, calls, note) in cases {
        let (_vault, mut summary) = set_up_summary().await;
        let app = RecordingObsidian::with_cli(answers);
        let mut asked = 0;

        summary.load_plugin(
            &app,
            Some(&mut |_| {
                asked += 1;
                Ok(true)
            }),
        );

        assert_eq!(summary.plugin_load, expected);
        assert_eq!(app.calls(), calls, "{expected:?}");
        assert_eq!(asked, 0, "nothing closes, so nothing is asked");
        let line = summary.plugin_note().unwrap();
        assert!(line.contains(note), "{line}");
    }
}

#[tokio::test]
async fn obsidian_is_restarted_only_when_the_wizard_is_told_to() {
    // Obsidian's command line interface is off (its default): the plugin is loaded by
    // starting Obsidian again, which closes the user's windows — their call.
    let (_vault, mut summary) = set_up_summary().await;
    let app = RecordingObsidian::without_cli();
    let mut questions = Vec::new();
    summary.load_plugin(
        &app,
        Some(&mut |question| {
            questions.push(question.to_string());
            Ok(true)
        }),
    );
    assert_eq!(summary.plugin_load, PluginLoad::Restarted);
    assert_eq!(app.calls(), ["open?", "plugins:restrict", "restart"]);
    assert_eq!(questions.len(), 1, "{questions:?}");
    assert!(
        questions[0].ends_with("Restart Obsidian now?"),
        "{questions:?}"
    );
    assert_eq!(
        summary.plugin_note().unwrap(),
        "obsidian plugin: installed in .obsidian/plugins/restask and enabled; Obsidian was \
         restarted and loaded it"
    );

    // Declined: Obsidian keeps running, and the summary says what is left to do.
    let (_vault, mut summary) = set_up_summary().await;
    let app = RecordingObsidian::without_cli();
    summary.load_plugin(&app, Some(&mut |_| Ok(false)));
    assert_eq!(summary.plugin_load, PluginLoad::Pending);
    assert_eq!(app.calls(), ["open?", "plugins:restrict"]);
    assert!(summary
        .plugin_note()
        .unwrap()
        .ends_with("loads the plugin when it is restarted"));

    // An unattended run has nobody to ask, and never restarts.
    let (_vault, mut summary) = set_up_summary().await;
    let app = RecordingObsidian::without_cli();
    summary.load_plugin(&app, None);
    assert_eq!(summary.plugin_load, PluginLoad::Pending);
    assert_eq!(app.calls(), ["open?", "plugins:restrict"]);

    // A restart that fails is not a failed setup.
    let (_vault, mut summary) = set_up_summary().await;
    let app = RecordingObsidian {
        fail_restart: true,
        ..RecordingObsidian::without_cli()
    };
    summary.load_plugin(&app, Some(&mut |_| Ok(true)));
    assert_eq!(summary.plugin_load, PluginLoad::Pending);
}

#[tokio::test]
async fn an_obsidian_with_nothing_new_to_load_is_left_alone() {
    // The vault is not open in any running Obsidian: it reads the list when it opens it.
    let (vault, mut summary) = set_up_summary().await;
    let closed = RecordingObsidian {
        cli: Some(Vec::new()),
        ..RecordingObsidian::default()
    };
    summary.load_plugin(&closed, Some(&mut |_| Ok(true)));
    assert_eq!(summary.plugin_load, PluginLoad::NotNeeded);
    assert_eq!(closed.calls(), ["open?"]);

    // A second run wrote nothing of the plugin: Obsidian is not even looked for, and
    // the line asks for no restart.
    let mut again = run_setup(args(&vault), MockCaldav::new(), later_clock(), None)
        .await
        .unwrap();
    let open = RecordingObsidian::without_cli();
    again.load_plugin(&open, Some(&mut |_| Ok(true)));
    assert_eq!(again.plugin_load, PluginLoad::NotNeeded);
    assert!(open.calls().is_empty(), "{:?}", open.calls());
    assert_eq!(
        again.plugin_note().unwrap(),
        "obsidian plugin: installed in .obsidian/plugins/restask and enabled"
    );

    // A join installs no plugin, so it has nothing to load either.
    let (vault, mock) = set_up_vault().await;
    let machine = tempfile::tempdir().unwrap();
    let mut joined = run_setup(join_args(&vault, &machine), mock, clock(), None)
        .await
        .unwrap();
    let open = RecordingObsidian::without_cli();
    joined.load_plugin(&open, Some(&mut |_| Ok(true)));
    assert!(open.calls().is_empty(), "{:?}", open.calls());
    assert_eq!(joined.plugin_note(), None);
}

#[test]
fn the_vault_is_found_in_obsidians_own_list_of_vaults() {
    // `obsidian.json` as Obsidian 1.13 writes it: `open` marks the vaults with a window.
    let vault = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let here = std::fs::canonicalize(vault.path()).unwrap();
    let list = |open: &str| {
        format!(
            "{{\"vaults\":{{\"414bc3a5585fc108\":{{\"path\":{:?},\"ts\":1790970393634{open}}},\
             \"ce52333f47bdb282\":{{\"path\":{:?},\"ts\":1790863924616,\"open\":true}}}}}}",
            here.display().to_string(),
            other.path().display().to_string(),
        )
    };

    assert_eq!(
        open_vault_id(&list(",\"open\":true"), &here).as_deref(),
        Some("414bc3a5585fc108")
    );
    // Known to Obsidian, but no window on it.
    assert_eq!(open_vault_id(&list(""), &here), None);
    // Not a vault of this Obsidian; a subfolder of one is not the vault.
    assert_eq!(
        open_vault_id(&list(",\"open\":true"), Path::new("/nowhere")),
        None
    );
    assert_eq!(
        open_vault_id(&list(",\"open\":true"), &here.join("sub")),
        None
    );
    assert_eq!(open_vault_id("not json", &here), None);
    assert_eq!(open_vault_id("{}", &here), None);
}

#[cfg(unix)]
#[test]
fn a_command_reaches_obsidian_the_way_its_own_command_line_sends_it() {
    use std::io::{BufRead as _, Write as _};
    // A listener that only records the request and answers, in Obsidian's place.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("cli.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let obsidian = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufReader::new(&stream)
            .read_line(&mut request)
            .unwrap();
        stream.write_all(b"Enabled: restask\n").unwrap();
        request
    });

    let answer = restask::setup::obsidian_cli_request(
        &socket,
        "ce52333f47bdb282",
        Path::new("/home/me/My Vault"),
        &["plugin:enable", "id=restask", "filter=community"],
    )
    .unwrap();

    assert_eq!(answer, "Enabled: restask");
    // One JSON line; the vault is named, so Obsidian does not fall back to the window
    // that had the focus; never a terminal session.
    assert_eq!(
        obsidian.join().unwrap(),
        "{\"argv\":[\"vault=ce52333f47bdb282\",\"plugin:enable\",\"id=restask\",\
         \"filter=community\"],\"cwd\":\"/home/me/My Vault\",\"tty\":false}\n"
    );
}

#[test]
fn obsidian_itself_is_told_from_its_helper_processes() {
    let exe = Path::new("/home/me/.local/lib/obsidian/obsidian");
    let argv = |args: &[&str]| -> Vec<String> { args.iter().map(|arg| arg.to_string()).collect() };
    assert!(is_obsidian_main(
        exe,
        &argv(&[
            "/home/me/.local/lib/obsidian/obsidian",
            "--ozone-platform=wayland"
        ])
    ));
    assert!(!is_obsidian_main(
        exe,
        &argv(&[
            "/home/me/.local/lib/obsidian/obsidian",
            "--type=gpu-process",
            "--ozone-platform=wayland"
        ])
    ));
    assert!(!is_obsidian_main(
        exe,
        &argv(&["/home/me/.local/lib/obsidian/obsidian", "--type=zygote"])
    ));
    assert!(!is_obsidian_main(
        Path::new("/home/me/.local/lib/obsidian/obsidian-cli"),
        &argv(&["obsidian-cli", "reload"])
    ));
    assert!(!is_obsidian_main(
        Path::new("/usr/bin/bash"),
        &argv(&["bash", "obsidian"])
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn a_restarted_program_is_closed_and_started_again_as_it_was() {
    // A stand-in program (a shell loop, not Obsidian): each start writes one line with
    // what it was started with, into the file its first argument names.
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("obsidian");
    let log = dir.path().join("starts");
    std::fs::write(
        &script,
        "log=$1\nshift\necho \"$(pwd -P)|$*\" >> \"$log\"\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    let starts = |count: usize| -> Vec<String> {
        for _ in 0..100 {
            let lines: Vec<String> = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect();
            if lines.len() >= count {
                return lines;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the program did not start {count} time(s)");
    };
    let mut first = std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg(&log)
        .args(["--flag", "two words"])
        .current_dir(dir.path())
        .spawn()
        .unwrap();
    starts(1);

    let second = restask::setup::restart_process(first.id()).unwrap();

    // The first one was asked to close, not left running beside the new one.
    assert!(!first.wait().unwrap().success());
    assert_ne!(second, first.id());
    let lines = starts(2);
    let _ = std::process::Command::new("kill")
        .arg(second.to_string())
        .status();
    // Same arguments and working directory.
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        lines,
        vec![format!("{}|--flag two words", cwd.display()); 2]
    );
}
