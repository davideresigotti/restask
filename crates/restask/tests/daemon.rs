//! Daemon conformance (§13.1): `run_once` end-to-end against the mock port, the pure
//! debounce coalescing (explicit instants — no real watcher or wall-clock timing), and
//! clean shutdown/once modes.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone as _;

use common::{temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::config::{CaldavConfig, MachineConfig};
use restask::daemon::{run_once_with, run_with, DaemonConfig, DebounceQueue};
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

#[test]
fn debounce_queue_coalesces_bursts_by_path() {
    let base = Instant::from_std(std::time::Instant::now());
    let mut queue = DebounceQueue::default();
    assert!(queue.is_empty());

    // Three writes to the same file inside the window coalesce into one firing.
    queue.push(
        std::path::PathBuf::from("notes/a.md"),
        base + Duration::from_millis(300),
    );
    queue.push(
        std::path::PathBuf::from("notes/a.md"),
        base + Duration::from_millis(500),
    );
    queue.push(
        std::path::PathBuf::from("notes/a.md"),
        base + Duration::from_millis(700),
    );

    assert_eq!(
        queue.due(base + Duration::from_millis(299)),
        Vec::<std::path::PathBuf>::new()
    );
    assert!(!queue.is_empty());
    assert_eq!(
        queue.due(base + Duration::from_millis(700)),
        vec![std::path::PathBuf::from("notes/a.md")]
    );
    assert!(queue.is_empty());
    assert_eq!(
        queue.due(base + Duration::from_millis(800)),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn debounce_queue_tracks_paths_independently() {
    let base = Instant::from_std(std::time::Instant::now());
    let mut queue = DebounceQueue::default();
    queue.push(
        std::path::PathBuf::from("notes/a.md"),
        base + Duration::from_millis(300),
    );
    queue.push(
        std::path::PathBuf::from("notes/b.md"),
        base + Duration::from_millis(500),
    );

    assert_eq!(
        queue.due(base + Duration::from_millis(300)),
        vec![std::path::PathBuf::from("notes/a.md")]
    );
    assert_eq!(
        queue.next_deadline(),
        Some(base + Duration::from_millis(500))
    );
    assert_eq!(
        queue.due(base + Duration::from_millis(500)),
        vec![std::path::PathBuf::from("notes/b.md")]
    );
    assert_eq!(queue.next_deadline(), None);
}

#[test]
fn reconcile_report_defaults_to_zeroes() {
    assert_eq!(
        ReconcileReport::default(),
        ReconcileReport {
            scanned_files: 0,
            registered: 0,
            pushes: 0,
            moves: 0,
            deletes: 0,
            markdown_mutations: 0,
            inserts: 0,
            adoptions: 0,
            deferred: 0,
            parked: 0,
        }
    );
}
