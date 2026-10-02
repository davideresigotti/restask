//! Daemon conformance (§13.1): `run_once` end-to-end against the mock port, the pure
//! debounce coalescing (explicit instants), event filtering, the live loop reacting to a
//! vault edit and to another client's writes on the server, and clean shutdown/once modes.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone as _;

use common::{temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::caldav::CollectionInfo;
use restask::config::{CaldavConfig, MachineConfig};
use restask::daemon::{run_once_with, run_with, server_tags, DaemonConfig, Debounce};
use restask::domain::TaskUid;
use restask::sync::ReconcileReport;
use tokio::time::Instant;

fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        chrono::Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .unwrap(),
        chrono::FixedOffset::east_opt(0).unwrap(),
    ))
}

fn machine() -> MachineConfig {
    MachineConfig {
        caldav: CaldavConfig {
            allow_create_lists: true,
            ..CaldavConfig::default()
        },
        ..MachineConfig::default()
    }
}

fn seeded_vault() -> tempfile::TempDir {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "notes/home.md",
        "---\nrestask-list: Home\n---\n\n# Home\n\n- [ ] buy milk\n",
    );
    dir
}

#[tokio::test]
async fn run_once_performs_a_full_reconcile() {
    let dir = seeded_vault();
    let mock = MockCaldav::new();
    let report = run_once_with(dir.path(), &machine(), clock(), mock.clone())
        .await
        .unwrap();
    assert_eq!(report.registered, 1);
    assert_eq!(report.pushes, 1);
    assert!(mock.resource("home", &mock_name(&dir)).is_some());

    // Idempotent: a second pass is a no-op.
    let report = run_once_with(dir.path(), &machine(), clock(), mock.clone())
        .await
        .unwrap();
    assert_eq!(report.pushes, 0);
}

/// The single registered task's UID (parse it from the note to stay uid-agnostic).
fn mock_name(dir: &tempfile::TempDir) -> String {
    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    let uid = note.split("\u{1F194}").nth(1).unwrap().trim().to_string();
    TaskUid::parse(&uid).unwrap().as_str().to_string()
}

#[tokio::test]
async fn run_once_missing_config_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = run_once_with(dir.path(), &machine(), clock(), MockCaldav::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("config"), "{error}");
}

#[tokio::test]
async fn daemon_once_mode_reconciles_and_exits() {
    let dir = seeded_vault();
    let mock = MockCaldav::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    drop(tx); // nothing will ever send; `once` must return on its own.
    run_with(
        dir.path().to_path_buf(),
        machine(),
        DaemonConfig {
            once: true,
            ..DaemonConfig::default()
        },
        rx,
        mock.clone(),
        clock(),
    )
    .await
    .unwrap();
    assert!(mock.resource("home", &mock_name(&dir)).is_some());
}

#[tokio::test]
async fn daemon_stops_cleanly_on_shutdown() {
    let dir = seeded_vault();
    let mock = MockCaldav::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(run_with(
        dir.path().to_path_buf(),
        machine(),
        DaemonConfig::default(),
        rx,
        mock.clone(),
        clock(),
    ));
    tx.send(true).unwrap();
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_daemon_reconciles_on_start_and_on_vault_events() {
    let dir = seeded_vault();
    let mock = MockCaldav::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(run_with(
        dir.path().to_path_buf(),
        machine(),
        DaemonConfig {
            debounce_ms: 20,
            poll_secs: 3_600,
            watch_ms: 0,
            once: false,
        },
        rx,
        mock.clone(),
        clock(),
    ));
    // The first poll tick is immediate: the startup pass registers and pushes.
    wait_until(|| mock.resource_names("home").len() == 1).await;

    // A new line in a note wakes the reconciler through the watcher.
    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    write_vault_file(
        &dir,
        "notes/home.md",
        &format!("{note}- [ ] walk the dog\n"),
    );
    wait_until(|| mock.resource_names("home").len() == 2).await;

    // The engine's own writes settle: no endless self-triggered passes.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (puts, _, reports) = mock.counters();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.counters(), (puts, 0, reports), "the daemon went quiet");

    tx.send(true).unwrap();
    worker.await.unwrap().unwrap();
}

const TASKS_ORG_BODY: &str =
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:+//IDN tasks.org//android//EN\r\n\
BEGIN:VTODO\r\nDTSTAMP:20260922T101500Z\r\nUID:5417861935824551742\r\n\
CREATED:20260921T081233Z\r\nLAST-MODIFIED:20260922T101400Z\r\nSUMMARY:Made in Tasks.org\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

/// The owner's report: a task added, changed or deleted in Tasks.org "doesn't sync with
/// the vault". It did, at the next poll — five minutes later. The poll is an hour here:
/// what brings the changes in is the server watch.
#[tokio::test]
async fn changes_made_on_the_server_reach_the_vault_without_waiting_for_the_poll() {
    let dir = seeded_vault();
    let mock = MockCaldav::new();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(run_with(
        dir.path().to_path_buf(),
        machine(),
        DaemonConfig {
            debounce_ms: 20,
            poll_secs: 3_600,
            watch_ms: 20,
            once: false,
        },
        rx,
        mock.clone(),
        clock(),
    ));
    wait_until(|| mock.resource_names("home").len() == 1).await;
    let read = |file: &str| std::fs::read_to_string(dir.path().join(file)).unwrap();

    // Added there: adopted, and a line in the inbox file.
    mock.seed_resource("inbox", "5417861935824551742", TASKS_ORG_BODY);
    wait_until(|| read("TODO.md").contains("- [ ] Made in Tasks.org ")).await;

    // Changed there: the line in the note follows.
    let name = mock_name(&dir);
    let body = mock.resource("home", &name).unwrap().body;
    assert!(body.contains("SUMMARY:buy milk\r\n"), "{body}");
    mock.seed_resource(
        "home",
        &name,
        &body.replace("SUMMARY:buy milk\r\n", "SUMMARY:buy oat milk\r\n"),
    );
    wait_until(|| read("notes/home.md").contains("- [ ] buy oat milk ")).await;

    // Deleted there: the line goes.
    mock.remove_resource("home", &name);
    wait_until(|| !read("notes/home.md").contains("milk")).await;

    // The daemon's own pushes change the tags as well; it still comes to rest.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let counters = mock.counters();
    let files = (read("TODO.md"), read("notes/home.md"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.counters(), counters, "the daemon went quiet");
    assert_eq!((read("TODO.md"), read("notes/home.md")), files);

    tx.send(true).unwrap();
    worker.await.unwrap().unwrap();
}

#[test]
fn the_server_watch_compares_the_change_tags_of_task_collections() {
    let collection = |slug: &str, supports_vtodo: bool, ctag: Option<&str>| CollectionInfo {
        href: format!("/me/{slug}/"),
        slug: slug.to_string(),
        display_name: None,
        supports_vtodo,
        ctag: ctag.map(str::to_string),
    };
    let before = [
        collection("inbox", true, Some("\"a1\"")),
        collection("home", true, Some("\"b1\"")),
        collection("events", false, Some("\"c1\"")),
        collection("me", false, None),
    ];
    assert_eq!(
        server_tags(&before),
        vec![
            ("home".to_string(), "\"b1\"".to_string()),
            ("inbox".to_string(), "\"a1\"".to_string()),
        ]
    );

    // The order of the listing and a calendar without tasks say nothing.
    let same = [
        collection("events", false, Some("\"c2\"")),
        collection("home", true, Some("\"b1\"")),
        collection("inbox", true, Some("\"a1\"")),
    ];
    assert_eq!(server_tags(&same), server_tags(&before));

    // A write to a task list, a new list and a list that is gone do.
    let written = [
        collection("inbox", true, Some("\"a2\"")),
        collection("home", true, Some("\"b1\"")),
    ];
    assert_ne!(server_tags(&written), server_tags(&before));
    assert_ne!(server_tags(&before[..1]), server_tags(&before));

    // A server without change tags always answers the same: the poll is what is left.
    let untagged = [collection("inbox", true, None)];
    assert_eq!(server_tags(&untagged), Vec::new());
}

/// Polls `condition` for up to five seconds.
async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached within 5 s");
}

#[test]
fn debounce_coalesces_a_burst_into_one_fire() {
    let base = Instant::now();
    let ms = Duration::from_millis;
    let mut pending = Debounce::default();
    assert!(!pending.fire(base), "clean: nothing to fire");
    assert_eq!(pending.deadline(), None);

    // Three events in quick succession: each postpones the fire-at instant.
    pending.touch(base + ms(300));
    pending.touch(base + ms(350));
    pending.touch(base + ms(400));
    assert_eq!(pending.deadline(), Some(base + ms(400)));
    assert!(!pending.fire(base + ms(399)));
    assert!(pending.fire(base + ms(400)));
    // Fired once: clean again until the next event.
    assert!(!pending.fire(base + ms(10_000)));
    assert_eq!(pending.deadline(), None);
}

#[test]
fn only_events_that_can_change_a_scan_wake_the_daemon() {
    use restask::config::VaultConfig;
    use restask::vault::is_relevant_event;
    let matchers = VaultConfig::default().matchers().unwrap();
    for relevant in ["TODO.md", "notes/home.md", "notes", "2. Areas/Home Lab"] {
        assert!(is_relevant_event(relevant, &matchers), "{relevant}");
    }
    for ignored in [
        "",
        ".restask",
        ".restask/index.json",
        ".restask/todo.rendered.md",
        ".restask/tasks/restask-01jz0000000000000000000001.ics",
        ".obsidian/workspace.json",
        ".git/HEAD",
        "notes/.home.md.restask-tmp",
        "TODO.pre-restask-20260922-101500.md",
        "notes/home.sync-conflict-20260922-101500-ABCDEFG.md",
        "notes/picture.png",
    ] {
        assert!(!is_relevant_event(ignored, &matchers), "{ignored}");
    }
}

#[test]
fn reconcile_report_defaults_to_zeroes() {
    assert_eq!(
        ReconcileReport::default(),
        ReconcileReport {
            scanned_files: 0,
            registered: 0,
            normalized: 0,
            pushes: 0,
            moves: 0,
            deletes: 0,
            markdown_mutations: 0,
            inserts: 0,
            adoptions: 0,
            deferred: 0,
            failed: 0,
        }
    );
}
