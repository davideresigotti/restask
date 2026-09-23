//! CLI integration tests (§13.3): add/done/undone/status/sync/rebuild against a tempdir
//! vault and the in-memory CalDAV mock.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tempfile::TempDir;

use common::{sample_task, temp_vault, FixedClock, MockCaldav};
use restask::cli::{self, Cli, Command, ListAction, Selector};
use restask::config::{ListBinding, MachineConfig};
use restask::domain::{ListSlug, Priority, TaskUid};
use restask::store::{cache_path, cache_write, Index, IndexEntry};
use restask::vtodo::to_vcalendar;
use restask::{CaldavErrorKind, TaskresError};

/// Fixed clock: 2026-09-22 12:00 UTC (in the past, so file mtimes are always newer than
/// cached stamps and the planner's R1 tie-window never defers), device-local +02:00.
fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ))
}

/// Machine config with the inbox pre-bound (§13.2 step 5 pattern).
fn machine() -> MachineConfig {
    MachineConfig {
        lists: vec![ListBinding {
            name: "Inbox".to_string(),
            collection: "inbox".to_string(),
        }],
        ..MachineConfig::default()
    }
}

/// Runs one command against the temp vault and mock server.
async fn run(command: Command, vault: &Path, mock: &MockCaldav) -> Result<i32, TaskresError> {
    cli::run_with(
        command,
        vault.to_path_buf(),
        machine(),
        vault.join("machine.toml"),
        mock.clone(),
        clock(),
    )
    .await
}

/// Adds a task through the CLI and returns its UID (parsed back from the rendered line).
async fn add(
    vault: &TempDir,
    mock: &MockCaldav,
    text: &str,
    priority: Option<&str>,
    due: Option<&str>,
) -> String {
    let code = run(
        Command::Add {
            text: text.to_string(),
            priority: priority.map(str::to_string),
            due: due.map(str::to_string),
        },
        vault.path(),
        mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    uid_of(vault, text)
}

/// Finds the rendered TODO.md line containing `text` and extracts its UID token.
fn uid_of(vault: &TempDir, text: &str) -> String {
    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    contents
        .lines()
        .find(|line| line.contains(text))
        .and_then(|line| line.split("🆔 ").nth(1))
        .map(str::trim)
        .unwrap()
        .to_string()
}

/// Seeds one managed remote task into the mock's inbox collection.
fn seed_remote(mock: &MockCaldav, text: &str) -> TaskUid {
    mock.seed_collection("inbox", "Inbox");
    let uid = TaskUid::generate();
    let remote = sample_task(uid.as_str(), "inbox", text);
    let body = to_vcalendar(
        &remote,
        Utc.with_ymd_and_hms(2026, 9, 22, 11, 0, 0).unwrap(),
    );
    mock.seed_resource("inbox", uid.as_str(), &body);
    uid
}

#[tokio::test]
async fn add_registers_pushes_and_renders() {
    let vault = temp_vault();
    let mock = MockCaldav::new();

    let uid = add(&vault, &mock, "buy milk", None, None).await;

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let line = contents.lines().find(|l| l.contains("buy milk")).unwrap();
    assert!(line.starts_with("- [ ] buy milk"));
    assert!(uid.starts_with("taskres-"));

    assert_eq!(mock.resource_names("inbox"), vec![uid.clone()]);
    let index = Index::load(&vault.path().join(".taskres")).unwrap();
    let entry = index.get(&TaskUid::parse(&uid).unwrap()).unwrap();
    assert_eq!(entry.list.as_str(), "inbox");
    assert!(entry.caldav_etag.is_some());
}

#[tokio::test]
async fn add_with_priority_and_due() {
    let vault = temp_vault();
    let mock = MockCaldav::new();

    let uid = add(
        &vault,
        &mock,
        "file taxes",
        Some("high"),
        Some("2026-09-30"),
    )
    .await;

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let line = contents.lines().find(|l| l.contains("file taxes")).unwrap();
    assert!(line.contains(Priority::High.emoji()));
    assert!(line.contains("2026-09-30"));

    let body = mock.resource("inbox", &uid).unwrap().body;
    assert!(body.contains(&format!("PRIORITY:{}", Priority::High.to_ical())));
    assert!(body.contains("DUE;VALUE=DATE:20260930"));
}

#[tokio::test]
async fn done_and_undone_by_uid() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let uid = add(&vault, &mock, "buy milk", None, None).await;

    let code = run(
        Command::Done {
            selector: Selector {
                uid: Some(uid.clone()),
                file: None,
                line: None,
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let line = contents.lines().find(|l| l.contains("buy milk")).unwrap();
    assert!(line.starts_with("- [x] buy milk"));
    assert!(contents.contains("## Done"));
    let body = mock.resource("inbox", &uid).unwrap().body;
    assert!(body.contains("STATUS:COMPLETED"));

    let code = run(
        Command::Undone {
            selector: Selector {
                uid: Some(uid.clone()),
                file: None,
                line: None,
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let line = contents.lines().find(|l| l.contains("buy milk")).unwrap();
    assert!(line.starts_with("- [ ] buy milk"));
    let body = mock.resource("inbox", &uid).unwrap().body;
    assert!(body.contains("STATUS:NEEDS-ACTION"));
}

#[tokio::test]
async fn done_by_file_and_line() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let uid = add(&vault, &mock, "buy milk", None, None).await;

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let line_no = contents
        .lines()
        .position(|l| l.contains("buy milk"))
        .unwrap()
        + 1;

    let code = run(
        Command::Done {
            selector: Selector {
                uid: None,
                file: Some("TODO.md".to_string()),
                line: Some(line_no),
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);

    let body = mock.resource("inbox", &uid).unwrap().body;
    assert!(body.contains("STATUS:COMPLETED"));
}

#[tokio::test]
async fn status_counts_active_done_and_backlog() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let first = add(&vault, &mock, "alpha", None, None).await;
    add(&vault, &mock, "beta", Some("high"), None).await;

    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    assert_eq!(report.lists.get("inbox"), Some(&2));
    assert_eq!(report.priorities.get("high"), Some(&1));
    assert_eq!(report.done_today, 0);
    assert_eq!(report.outbox_backlog, 0);
    assert!(report.last_sync.is_some());

    run(
        Command::Done {
            selector: Selector {
                uid: Some(first),
                file: None,
                line: None,
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();

    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    assert_eq!(report.lists.get("inbox"), Some(&1));
    assert_eq!(report.priorities.get("high"), Some(&1));
    assert_eq!(report.done_today, 1);
}

#[tokio::test]
async fn status_json_is_serializable() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    add(&vault, &mock, "alpha", None, None).await;

    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("\"lists\":{\"inbox\":1}"));
    assert!(json.contains("\"done_today\":0"));
    assert!(json.contains("\"outbox_backlog\":0"));
    assert!(json.contains("\"last_sync\":"));
}

#[tokio::test]
async fn rebuild_rederives_state_preserves_etags_and_prunes() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let alpha = add(&vault, &mock, "alpha", None, None).await;
    add(&vault, &mock, "beta", None, None).await;
    let state = vault.path().join(".taskres");
    let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();

    let alpha_uid = TaskUid::parse(&alpha).unwrap();
    let alpha_etag = Index::load(&state)
        .unwrap()
        .get(&alpha_uid)
        .unwrap()
        .caldav_etag
        .clone()
        .unwrap();

    // A stale entry + cache from a task no longer in the vault.
    let ghost = TaskUid::generate();
    let mut index = Index::load(&state).unwrap();
    index.upsert(IndexEntry {
        uid: ghost.clone(),
        list: ListSlug::from_name("Ghost").unwrap(),
        source_path: "gone.md".to_string(),
        thumbprint: 0,
        caldav_etag: Some("stale".to_string()),
        seen_at: now,
        defer_count: 0,
    });
    index.save(&state).unwrap();
    cache_write(&state, &sample_task(ghost.as_str(), "ghost", "ghost"), now).unwrap();
    assert!(cache_path(&state, &ghost).exists());

    let code = run(Command::Rebuild, vault.path(), &mock).await.unwrap();
    assert_eq!(code, 0);

    let after = Index::load(&state).unwrap();
    assert_eq!(after.entries.len(), 2);
    assert!(after.get(&ghost).is_none());
    assert!(!cache_path(&state, &ghost).exists());
    assert_eq!(
        after.get(&alpha_uid).unwrap().caldav_etag.as_deref(),
        Some(alpha_etag.as_str())
    );
    assert!(cache_path(&state, &alpha_uid).exists());
    // Remote untouched: only the two add-pushes ever happened.
    assert_eq!(mock.resource_names("inbox").len(), 2);
}

#[tokio::test]
async fn sync_pulls_a_remote_task_into_the_inbox() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let uid = seed_remote(&mock, "from server");

    let code = run(Command::Sync, vault.path(), &mock).await.unwrap();
    assert_eq!(code, 0);

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(contents.contains("from server"));
    assert!(contents.contains(uid.as_str()));
    let index = Index::load(&vault.path().join(".taskres")).unwrap();
    assert!(index.get(&uid).is_some());
}

#[tokio::test]
async fn daemon_once_reconciles() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    seed_remote(&mock, "from server");

    let code = run(Command::Daemon { once: true }, vault.path(), &mock)
        .await
        .unwrap();
    assert_eq!(code, 0);

    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(contents.contains("from server"));
}

#[tokio::test]
async fn list_bind_persists_and_create_mkcollections() {
    let vault = temp_vault();
    let mock = MockCaldav::new();

    let code = run(
        Command::List {
            action: ListAction::Bind {
                name: "Home".to_string(),
                collection: "home".to_string(),
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);

    let saved = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(saved.lists.len(), 2); // the pre-bound Inbox + Home
    assert!(saved
        .lists
        .iter()
        .any(|b| b.name == "Home" && b.collection == "home"));

    let code = run(
        Command::List {
            action: ListAction::Create {
                name: "Work".to_string(),
            },
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert!(mock.collection_names().iter().any(|slug| slug == "work"));

    let saved = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert!(saved
        .lists
        .iter()
        .any(|b| b.name == "Work" && b.collection == "work"));
}

#[test]
fn vault_resolution_flag_env_then_upward_search() {
    let vault = temp_vault();
    let nested = vault.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        cli::resolve_vault_with(
            Some(Path::new("/srv/vault")),
            Some("/env/vault"),
            Path::new("/nowhere")
        )
        .unwrap(),
        PathBuf::from("/srv/vault")
    );
    assert_eq!(
        cli::resolve_vault_with(None, Some("/env/vault"), Path::new("/nowhere")).unwrap(),
        PathBuf::from("/env/vault")
    );
    assert_eq!(
        cli::resolve_vault_with(None, None, &nested).unwrap(),
        vault.path().to_path_buf()
    );

    let empty = tempfile::tempdir().unwrap();
    assert!(matches!(
        cli::resolve_vault_with(None, None, empty.path()),
        Err(TaskresError::Config { .. })
    ));
}

#[test]
fn exit_code_follows_the_taxonomy() {
    assert_eq!(
        cli::exit_code(&TaskresError::Config {
            path: "p".into(),
            reason: "r".into()
        }),
        4
    );
    assert_eq!(
        cli::exit_code(&TaskresError::Caldav {
            kind: CaldavErrorKind::Network,
            status: None,
            detail: "d".into(),
        }),
        3
    );
    assert_eq!(
        cli::exit_code(&TaskresError::Validation {
            field: "f",
            reason: "r".into()
        }),
        1
    );
}

#[test]
fn selector_requires_uid_or_file_and_line() {
    use clap::Parser as _;

    assert!(Cli::try_parse_from(["restask", "done"]).is_err());
    assert!(Cli::try_parse_from(["restask", "done", "--file", "TODO.md"]).is_err());
    assert!(Cli::try_parse_from([
        "restask",
        "done",
        "--uid",
        "taskres-01jzetq1v2h3k4m5n6p7r8t9w0",
        "--file",
        "TODO.md",
        "--line",
        "1"
    ])
    .is_err());

    let ok = Cli::try_parse_from(["restask", "done", "--file", "TODO.md", "--line", "3"]).unwrap();
    match ok.command {
        Command::Done { selector } => {
            assert_eq!(selector.file.as_deref(), Some("TODO.md"));
            assert_eq!(selector.line, Some(3));
        }
        _ => panic!("expected the done command"),
    }
}
