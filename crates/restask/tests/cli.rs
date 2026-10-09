//! CLI integration tests (§13.3): add/done/undone/status/sync/rebuild against a tempdir
//! vault and the in-memory CalDAV mock.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tempfile::TempDir;

use common::{fresh_uid, sample_task, temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::cli::{self, Cli, Command, DoctorStatus, Selector};
use restask::config::{CaldavConfig, MachineConfig};
use restask::domain::{Priority, TaskUid};
use restask::store::{cache_path, Index};
use restask::vtodo::to_vcalendar;
use restask::{CaldavErrorKind, RestaskError};

/// Fixed clock: 2026-09-22 12:00 UTC (in the past, so file mtimes are always newer than
/// cached stamps and the planner's R1 tie-window never defers), device-local +02:00.
fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ))
}

/// Machine config as setup leaves it (endpoint fields are irrelevant to the mock port).
fn machine() -> MachineConfig {
    MachineConfig::default()
}

/// Runs one command against the temp vault and mock server.
async fn run(command: Command, vault: &Path, mock: &MockCaldav) -> Result<i32, RestaskError> {
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
            repeat: None,
        },
        vault.path(),
        mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    uid_of(vault, text)
}

/// Finds the rendered TODO.md line containing `text` and returns the UID its token
/// spells, in full (`🆔 a42` → `restask-a42`): the name of the task's resource.
fn uid_of(vault: &TempDir, text: &str) -> String {
    let contents = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let token = contents
        .lines()
        .find(|line| line.contains(text))
        .and_then(|line| line.split("🆔 ").nth(1))
        .map(str::trim)
        .unwrap()
        .to_string();
    TaskUid::from_token(&token).unwrap().as_str().to_string()
}

/// Seeds one managed remote task into the mock's inbox collection.
fn seed_remote(mock: &MockCaldav, text: &str) -> TaskUid {
    mock.seed_collection("inbox", "Inbox");
    let uid = fresh_uid();
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
    // A counted UID of this device: its tag and the first number.
    let (_, number) = TaskUid::parse(&uid)
        .unwrap()
        .minted_by()
        .map(|(tag, n)| (tag.to_string(), n))
        .unwrap();
    assert_eq!(number, 1);

    assert_eq!(mock.resource_names("inbox"), vec![uid.clone()]);
    let index = Index::load(&vault.path().join(".restask")).unwrap();
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
async fn status_counts_active_done_and_pending() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let first = add(&vault, &mock, "alpha", None, None).await;
    add(&vault, &mock, "beta", Some("high"), None).await;

    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    assert_eq!(report.lists.get("inbox"), Some(&2));
    assert_eq!(report.priorities.get("high"), Some(&1));
    assert_eq!(report.done_today, 0);
    assert_eq!(report.pending, 0);
    assert!(report.last_sync.is_some());

    // A line typed by hand that no sync has seen yet is pending.
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let gamma = "restask-01jz0000000000000000000009";
    std::fs::write(
        vault.path().join("TODO.md"),
        format!("{todo}- [ ] gamma 🆔 {gamma}\n"),
    )
    .unwrap();
    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    assert_eq!(report.pending, 1);
    assert_eq!(report.lists.get("inbox"), Some(&3));
    run(Command::Sync, vault.path(), &mock).await.unwrap();
    let report = cli::status_report(vault.path(), clock().as_ref()).unwrap();
    assert_eq!(report.pending, 0);

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
    assert_eq!(report.lists.get("inbox"), Some(&2));
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
    assert!(json.contains("\"pending\":0"));
    assert!(json.contains("\"last_sync\":"));
}

#[tokio::test]
async fn rebuild_drops_the_sync_state_and_the_next_sync_rederives_it() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let alpha = add(&vault, &mock, "alpha", None, None).await;
    add(&vault, &mock, "beta", None, None).await;
    let state = vault.path().join(".restask");
    let alpha_uid = TaskUid::parse(&alpha).unwrap();
    assert!(cache_path(&state, &alpha_uid).exists());

    // A deletion that must stay remembered across the rebuild.
    let ghost = fresh_uid();
    let mut tombstones = restask::store::Tombstones::load(&state).unwrap();
    tombstones.insert(
        ghost.clone(),
        Utc.with_ymd_and_hms(2026, 9, 22, 11, 0, 0).unwrap(),
    );
    tombstones.save(&state).unwrap();
    let todo_before = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    let puts_before = mock.counters().0;

    let code = run(Command::Rebuild, vault.path(), &mock).await.unwrap();
    assert_eq!(code, 0);
    assert!(Index::load(&state).unwrap().entries.is_empty());
    assert!(!state.join("tasks").exists());
    assert!(restask::store::Tombstones::load(&state)
        .unwrap()
        .contains(&ghost));
    // Neither the vault nor the server was touched.
    assert_eq!(
        std::fs::read_to_string(vault.path().join("TODO.md")).unwrap(),
        todo_before
    );
    assert_eq!(mock.resource_names("inbox").len(), 2);

    // The next sync finds both sides equal and simply settles again: no push, no edit.
    run(Command::Sync, vault.path(), &mock).await.unwrap();
    let after = Index::load(&state).unwrap();
    assert_eq!(after.entries.len(), 2);
    assert!(after.get(&alpha_uid).unwrap().caldav_etag.is_some());
    assert!(cache_path(&state, &alpha_uid).exists());
    assert_eq!(mock.counters().0, puts_before);
    assert_eq!(
        std::fs::read_to_string(vault.path().join("TODO.md")).unwrap(),
        todo_before
    );
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
    assert!(contents.contains(&format!("🆔 {}", uid.token())));
    let index = Index::load(&vault.path().join(".restask")).unwrap();
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
async fn lists_and_local_commands_need_no_server() {
    let vault = temp_vault();
    write_vault_file(
        &vault,
        "notes/home.md",
        "---\nrestask-list-root: Home Lab\n---\n\n- [ ] rack the switch\n",
    );
    // `lists` is read-only and offline.
    let code = run(Command::Lists, vault.path(), &MockCaldav::new())
        .await
        .unwrap();
    assert_eq!(code, 0);
    assert!(
        !std::fs::read_to_string(vault.path().join("notes/home.md"))
            .unwrap()
            .contains('🆔'),
        "a read-only command registers nothing"
    );

    // `add` on a machine without a CalDAV endpoint saves to the vault and exits 0.
    let code = cli::run_with(
        Command::Add {
            text: "captured offline".to_string(),
            priority: None,
            due: None,
            repeat: None,
        },
        vault.path().to_path_buf(),
        machine(),
        vault.path().join("machine.toml"),
        restask::caldav::Offline,
        clock(),
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(todo.contains("- [ ] captured offline"));
}

/// `settle` (§13.3) is what an editor runs on save: the local work, and no request —
/// also on a machine that has an endpoint configured.
#[tokio::test]
async fn settle_registers_and_renders_without_asking_the_server() {
    let vault = temp_vault();
    common::switch_vault(&vault);
    write_vault_file(
        &vault,
        "notes/home.md",
        "---\nrestask-list: Home Lab\n---\n\n- [ ] rack the switch 🔺\n",
    );
    let mock = MockCaldav::new();
    let code = run(
        Command::Settle {
            file: Some(vault.path().join("notes/home.md").display().to_string()),
        },
        vault.path(),
        &mock,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let note = std::fs::read_to_string(vault.path().join("notes/home.md")).unwrap();
    let registered = note
        .lines()
        .find_map(|line| line.strip_prefix("- [ ] rack the switch 🔺 🆔 "))
        .and_then(|token| TaskUid::from_token(token).ok());
    assert!(registered.is_some_and(|uid| !uid.is_long()), "{note}");
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(todo.contains("- [ ] rack the switch 🔺 [[home"), "{todo}");
    assert_eq!(mock.counters(), (0, 0, 0));
    assert!(mock.collection_names().is_empty());
}

#[test]
fn an_absolute_file_selector_finds_its_vault_from_the_file() {
    let vault = temp_vault();
    write_vault_file(&vault, "deep/er/note.md", "- [ ] x\n");
    let file = vault.path().join("deep/er/note.md");
    let resolved = cli::resolve_vault_for_file(None, file.to_str()).unwrap();
    assert_eq!(resolved, vault.path());
    // An explicit --vault still wins.
    let flag = PathBuf::from("/elsewhere");
    assert_eq!(
        cli::resolve_vault_for_file(Some(&flag), file.to_str()).unwrap(),
        flag
    );
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
        Err(RestaskError::Config { .. })
    ));
}

#[test]
fn exit_code_follows_the_taxonomy() {
    assert_eq!(
        cli::exit_code(&RestaskError::Config {
            path: "p".into(),
            reason: "r".into()
        }),
        4
    );
    assert_eq!(
        cli::exit_code(&RestaskError::Caldav {
            kind: CaldavErrorKind::Network,
            status: None,
            detail: "d".into(),
        }),
        3
    );
    assert_eq!(
        cli::exit_code(&RestaskError::Validation {
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
        "restask-01jzetq1v2h3k4m5n6p7r8t9w0",
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

/// Machine config for doctor tests: a configured endpoint plus an optional passwd file.
fn doctor_machine(password_file: Option<PathBuf>) -> MachineConfig {
    MachineConfig {
        caldav: CaldavConfig {
            url: Some("http://radicale.local:5232".to_string()),
            username: Some("me".to_string()),
            password_file,
            ..CaldavConfig::default()
        },
        ..MachineConfig::default()
    }
}

/// Runs the doctor check suite directly (checks + exit code assertions).
async fn doctor_report(
    vault: &TempDir,
    machine: &MachineConfig,
    mock: MockCaldav,
) -> restask::cli::DoctorReport {
    cli::doctor(
        vault.path(),
        machine,
        Some(mock),
        &vault.path().join("machine.toml"),
        clock(),
    )
    .await
    .unwrap()
}

fn status_of(report: &restask::cli::DoctorReport, name: &str) -> Option<DoctorStatus> {
    report
        .checks
        .iter()
        .find(|check| check.name == name)
        .map(|check| check.status)
}

#[tokio::test]
async fn doctor_healthy_reports_all_ok_and_exit_zero() {
    let vault = temp_vault();
    std::fs::write(vault.path().join("machine.toml"), "").unwrap();
    let passwd = vault.path().join("radicale.passwd");
    std::fs::write(&passwd, "secret").unwrap();
    let machine = doctor_machine(Some(passwd));
    let mock = MockCaldav::new();

    let report = doctor_report(&vault, &machine, mock.clone()).await;
    assert_eq!(report.exit_code, 0);
    assert_eq!(status_of(&report, "machine-config"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "vault-config"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "routing"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "scan"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "todo-view"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "sync-conflict"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "caldav"), Some(DoctorStatus::Ok));
    // The auth probe runs against the configured endpoint; with no reachable real
    // server it reports a soft warning and must not affect the exit code.
    assert_eq!(status_of(&report, "caldav-auth"), Some(DoctorStatus::Warn));

    // Through the CLI the report's exit code is the process exit code.
    let code = run(Command::Doctor {}, vault.path(), &mock).await.unwrap();
    assert_eq!(code, 0);
}

#[tokio::test]
async fn doctor_hard_warns_when_no_password_is_configured() {
    let vault = temp_vault();
    let machine = doctor_machine(None);

    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    assert_eq!(report.exit_code, 1);
    assert_eq!(status_of(&report, "caldav-auth"), Some(DoctorStatus::Warn));
    // Reachability itself is fine; only the auth layer warns.
    assert_eq!(status_of(&report, "caldav"), Some(DoctorStatus::Ok));
}

#[tokio::test]
async fn doctor_reports_an_unreachable_server_as_exit_three() {
    let vault = temp_vault();
    let passwd = vault.path().join("radicale.passwd");
    std::fs::write(&passwd, "secret").unwrap();
    let machine = doctor_machine(Some(passwd));
    let mock = MockCaldav::new();
    mock.fail_next(CaldavErrorKind::Network);

    let report = doctor_report(&vault, &machine, mock).await;
    assert_eq!(report.exit_code, 3);
    assert_eq!(status_of(&report, "caldav"), Some(DoctorStatus::Fail));
}

#[tokio::test]
async fn doctor_reports_an_invalid_vault_config_as_exit_four() {
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(vault.path().join("restask.toml"), "not [ valid toml").unwrap();
    let machine = doctor_machine(None);

    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    assert_eq!(report.exit_code, 4);
    assert_eq!(status_of(&report, "vault-config"), Some(DoctorStatus::Fail));
}

#[tokio::test]
async fn doctor_reports_duplicated_lines_as_a_warning() {
    let vault = temp_vault();
    let uid = fresh_uid();
    let line = format!("- [ ] shared 🆔 {}", uid.token());
    write_vault_file(
        &vault,
        "notes/a.md",
        &format!("---\nrestask-list: Home\n---\n\n{line}\n"),
    );
    write_vault_file(
        &vault,
        "notes/b.md",
        &format!("---\nrestask-list: Home\n---\n\n{line}\n"),
    );
    let passwd = vault.path().join("radicale.passwd");
    std::fs::write(&passwd, "secret").unwrap();
    let machine = doctor_machine(Some(passwd));

    // A copied line is something the next sync repairs, not a broken vault.
    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    assert_eq!(report.exit_code, 0);
    assert_eq!(status_of(&report, "routing"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "scan"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "duplicates"), Some(DoctorStatus::Warn));
    // Doctor itself changed nothing.
    assert!(std::fs::read_to_string(vault.path().join("notes/b.md"))
        .unwrap()
        .contains(uid.token()));
}

/// A view is known by the seal line in its frontmatter (§7.2).
#[tokio::test]
async fn doctor_accepts_a_sealed_view() {
    let vault = temp_vault();
    std::fs::write(
        vault.path().join("TODO.md"),
        common::sealed("---\nrestask-list: inbox\n---\n# TODO\n\n## Done\n"),
    )
    .unwrap();
    let passwd = vault.path().join("radicale.passwd");
    std::fs::write(&passwd, "secret").unwrap();
    let machine = doctor_machine(Some(passwd));

    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    assert_eq!(status_of(&report, "todo-view"), Some(DoctorStatus::Ok));
}

#[tokio::test]
async fn doctor_warns_on_a_foreign_todo_and_conflict_files_without_failing() {
    let vault = temp_vault();
    std::fs::write(
        vault.path().join("TODO.md"),
        "- [ ] plain, not a restask view\n",
    )
    .unwrap();
    write_vault_file(
        &vault,
        "notes/home.sync-conflict-20260922-101500-ABCDEFG.md",
        "junk\n",
    );
    let passwd = vault.path().join("radicale.passwd");
    std::fs::write(&passwd, "secret").unwrap();
    let machine = doctor_machine(Some(passwd));

    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    assert_eq!(report.exit_code, 0);
    assert_eq!(status_of(&report, "todo-view"), Some(DoctorStatus::Warn));
    assert_eq!(
        status_of(&report, "sync-conflict"),
        Some(DoctorStatus::Warn)
    );
}

#[tokio::test]
async fn doctor_without_a_configured_endpoint_stays_soft() {
    let vault = temp_vault();
    let machine = MachineConfig::default();

    let report = doctor_report(&vault, &machine, MockCaldav::new()).await;
    // With an injected port that answers, reachability is real; with no endpoint
    // configured the §17 auth layers are skipped entirely.
    assert_eq!(report.exit_code, 0);
    assert_eq!(status_of(&report, "caldav"), Some(DoctorStatus::Ok));
    assert_eq!(status_of(&report, "caldav-auth"), None);
}

#[tokio::test]
async fn doctor_on_an_editing_machine_points_at_the_sync_node() {
    // No endpoint here is the intended state of a machine whose vault is synced by the
    // daemon on a node (§14.2): not a warning, and the check says where to look.
    let vault = temp_vault();
    let machine = MachineConfig {
        node: Some(restask::config::NodeSection {
            host: Some("homeserver".to_string()),
            dir: Some("restask".to_string()),
            ..Default::default()
        }),
        ..MachineConfig::default()
    };
    let report = cli::doctor(
        vault.path(),
        &machine,
        None::<MockCaldav>,
        &vault.path().join("machine.toml"),
        clock(),
    )
    .await
    .unwrap();
    assert_eq!(report.exit_code, 0);
    let caldav = report
        .checks
        .iter()
        .find(|check| check.name == "caldav")
        .unwrap();
    assert_eq!(caldav.status, DoctorStatus::Ok);
    assert!(
        caldav.detail.contains("the daemon on homeserver"),
        "{}",
        caldav.detail
    );
    assert!(
        caldav
            .detail
            .contains("ssh homeserver 'cd restask && docker compose logs"),
        "{}",
        caldav.detail
    );
}

#[tokio::test]
async fn add_with_a_repeat_rule() {
    let vault = temp_vault();
    let mock = MockCaldav::new();
    let add = |repeat: &str| Command::Add {
        text: "water the plants".to_string(),
        priority: None,
        due: Some("2026-09-21".to_string()),
        repeat: Some(repeat.to_string()),
    };
    let code = run(add("every 2 weeks on mon"), vault.path(), &mock)
        .await
        .unwrap();
    assert_eq!(code, 0);
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(todo.contains("- [ ] water the plants 🔁 every 2 weeks on Monday 📅 2026-09-21"));
    let uid = uid_of(&vault, "water the plants");
    assert!(mock
        .resource("inbox", &uid)
        .unwrap()
        .body
        .contains("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO\r\n"));

    for bad in ["sometimes", "every week and then some"] {
        let error = run(add(bad), vault.path(), &mock).await.unwrap_err();
        assert!(error.to_string().contains("repeat"), "{error}");
    }
}
