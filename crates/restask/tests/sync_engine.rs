//! Engine conformance (§11, §13): full converge scenarios against a temporary routed
//! vault and the in-memory [`MockCaldav`] — registration + pushes, remote inserts,
//! tombstones, vault-side deletions, list moves, duels, outbox park/flush, add/complete.

mod common;

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use common::{temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::caldav::CaldavPort;
use restask::config::{CaldavConfig, MachineConfig, VaultConfig};
use restask::domain::TaskUid;
use restask::domain::{ListSlug, Status, Task};
use restask::store::index::Index;
use restask::store::tombstones::Tombstones;
use restask::sync::Engine;
use restask::vtodo::to_vcalendar;

const MARKER_TIME: i64 = 1_700_000_000;

fn clock() -> FixedClock {
    FixedClock(
        Utc.timestamp_opt(MARKER_TIME, 0).single().unwrap(),
        chrono::FixedOffset::east_opt(0).unwrap(),
    )
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

fn engine(dir: &TempDir, mock: MockCaldav, allow_create: bool) -> Engine<MockCaldav> {
    engine_with(dir, mock, allow_create, Vec::new())
}

fn engine_with<C: CaldavPort>(
    dir: &TempDir,
    caldav: C,
    allow_create: bool,
    bindings: Vec<restask::config::ListBinding>,
) -> Engine<C> {
    let cfg = VaultConfig::load(&dir.path().join("restask.toml")).unwrap();
    let machine = MachineConfig {
        caldav: CaldavConfig {
            allow_create_lists: allow_create,
            ..CaldavConfig::default()
        },
        lists: bindings,
        ..MachineConfig::default()
    };
    Engine::new(dir.path(), cfg, machine, caldav, Arc::new(clock()))
}

/// A [`MockCaldav`] wrapper whose `put` always fails with a network error — for outbox
/// parking paths that scripted `fail_next` cannot reach (it would abort the cycle at the
/// first `list_etags`).
#[derive(Debug, Clone, Default)]
struct PutAlwaysFails(MockCaldav);

impl CaldavPort for PutAlwaysFails {
    async fn list_collections(
        &self,
    ) -> Result<Vec<restask::caldav::CollectionInfo>, restask::TaskresError> {
        self.0.list_collections().await
    }

    async fn ensure_collection(
        &self,
        slug: &ListSlug,
        display: &str,
    ) -> Result<(), restask::TaskresError> {
        self.0.ensure_collection(slug, display).await
    }

    async fn list_etags(
        &self,
        slug: &ListSlug,
    ) -> Result<Vec<(String, String)>, restask::TaskresError> {
        self.0.list_etags(slug).await
    }

    async fn fetch(
        &self,
        slug: &ListSlug,
        name: &str,
    ) -> Result<Option<(restask::vtodo::RemoteTask, String)>, restask::TaskresError> {
        self.0.fetch(slug, name).await
    }

    async fn put(&self, _task: &Task) -> Result<String, restask::TaskresError> {
        Err(restask::TaskresError::Caldav {
            kind: restask::CaldavErrorKind::Network,
            status: None,
            detail: "put always fails".to_string(),
        })
    }

    async fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), restask::TaskresError> {
        self.0.delete(slug, name, etag).await
    }
}

/// A remote body for `task` stamped at a fixed instant (deterministic LAST-MODIFIED).
fn seeded_body(task: &Task, stamp_secs: i64) -> String {
    to_vcalendar(task, Utc.timestamp_opt(stamp_secs, 0).single().unwrap())
}

fn task(uid: &str, list: &str, text: &str, source: &str) -> Task {
    let mut task = common::sample_task(uid, list, text);
    task.source.path = source.to_string();
    task
}

/// Writes a routed note with one already-registered task line.
fn seeded_note(dir: &TempDir, uid: &str, text: &str) {
    write_vault_file(
        dir,
        "notes/home.md",
        &format!("---\nrestask-list: Home\n---\n\n# Home\n\n- [ ] {text} \u{1F194} {uid}\n"),
    );
}

#[tokio::test]
async fn first_sync_registers_pushes_and_renders() {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "notes/home.md",
        "---\nrestask-list: Home\n---\n\n# Home\n\n- [ ] buy milk\n- [ ] walk dog\n",
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.registered, 2);
    assert_eq!(report.pushes, 2);
    // Both routed files were parsed: the note and the engine-owned TODO.md.
    assert_eq!(report.scanned_files, 2);
    assert_eq!(mock.collection_names(), vec!["home"]);
    assert_eq!(mock.resource_names("home").len(), 2);

    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert_eq!(note.matches("\u{1F194}").count(), 2);

    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries.len(), 2);
    assert!(index
        .entries
        .values()
        .all(|entry| entry.caldav_etag.is_some() && entry.list == slug("home")));
    assert_eq!(
        std::fs::read_dir(dir.path().join(".restask/tasks"))
            .unwrap()
            .count(),
        2
    );

    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("buy milk"));
    assert!(todo.contains("[[home#Home|home]]"));
    assert!(todo.contains("\u{1F194}"));
}

#[tokio::test]
async fn remote_created_task_is_inserted_into_its_routed_note() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "anchor");
    let mock = MockCaldav::new();
    mock.seed_collection("home", "Home");
    let remote = task(
        "taskres-01jz0000000000000000000002",
        "home",
        "from the server",
        "notes/home.md",
    );
    mock.seed_resource(
        "home",
        "taskres-01jz0000000000000000000002",
        &seeded_body(&remote, MARKER_TIME),
    );
    let engine = engine(&dir, mock.clone(), false);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.inserts, 1);

    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert!(note.contains("from the server"));
    assert!(note.contains("taskres-01jz0000000000000000000002"));

    let index = Index::load(&dir.path().join(".restask")).unwrap();
    let entry = &index.entries[&TaskUid::parse("taskres-01jz0000000000000000000002").unwrap()];
    assert_eq!(entry.source_path, "notes/home.md");
    assert!(entry.caldav_etag.is_some());
    assert!(dir
        .path()
        .join(".restask/tasks/taskres-01jz0000000000000000000002.ics")
        .exists());

    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("from the server"));
}

#[tokio::test]
async fn server_side_deletion_tombstones_and_removes_the_line() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "doomed");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();

    mock.delete(&slug("home"), "taskres-01jz0000000000000000000001", None)
        .await
        .unwrap();
    engine.reconcile().await.unwrap();

    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert!(!note.contains("doomed"));
    let tombstones = Tombstones::load(&dir.path().join(".restask")).unwrap();
    assert!(tombstones.contains(&TaskUid::parse("taskres-01jz0000000000000000000001").unwrap()));
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert!(index.entries.is_empty());
    assert!(!dir
        .path()
        .join(".restask/tasks/taskres-01jz0000000000000000000001.ics")
        .exists());
}

#[tokio::test]
async fn vault_side_deletion_purges_cache_and_remote() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "gone");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();
    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_some());

    // The user deletes the line from the note; the server copy survives.
    seeded_note_without_task(&dir);
    engine.reconcile().await.unwrap();

    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_none());
    let tombstones = Tombstones::load(&dir.path().join(".restask")).unwrap();
    assert!(tombstones.contains(&TaskUid::parse("taskres-01jz0000000000000000000001").unwrap()));
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert!(index.entries.is_empty());
    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert!(note.contains("# Home"));
}

/// Rewrites the routed note with its heading but no task lines.
fn seeded_note_without_task(dir: &TempDir) {
    write_vault_file(
        dir,
        "notes/home.md",
        "---\nrestask-list: Home\n---\n\n# Home\n",
    );
}

#[tokio::test]
async fn rerouting_a_note_moves_the_task_between_collections() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "commute");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();
    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_some());

    write_vault_file(
        &dir,
        "notes/home.md",
        "---\nrestask-list: Work\n---\n\n# Home\n\n- [ ] commute \u{1F194} taskres-01jz0000000000000000000001\n",
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.moves, 1);
    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_none());
    assert!(mock
        .resource("work", "taskres-01jz0000000000000000000001")
        .is_some());
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(
        index
            .entries
            .get(&TaskUid::parse("taskres-01jz0000000000000000000001").unwrap())
            .unwrap()
            .list,
        slug("work")
    );
}

#[tokio::test]
async fn remote_wins_duel_mutates_the_note() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "alpha");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();

    // A far-future remote edit (beyond the tie window vs. any filesystem mtime).
    let mut remote = task(
        "taskres-01jz0000000000000000000001",
        "home",
        "beta",
        "notes/home.md",
    );
    remote.status = Status::Active;
    mock.seed_resource(
        "home",
        "taskres-01jz0000000000000000000001",
        &seeded_body(&remote, 1_900_000_000),
    );
    engine.reconcile().await.unwrap();

    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert!(note.contains("beta"));
    assert!(!note.contains("alpha"));
    let cached = std::fs::read_to_string(
        dir.path()
            .join(".restask/tasks/taskres-01jz0000000000000000000001.ics"),
    )
    .unwrap();
    assert!(cached.contains("beta"));
}

#[tokio::test]
async fn local_wins_duel_pushes_the_vault_copy() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "alpha");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();

    // A far-past remote edit: the vault authority wins.
    let mut remote = task(
        "taskres-01jz0000000000000000000001",
        "home",
        "stale beta",
        "notes/home.md",
    );
    remote.status = Status::Active;
    mock.seed_resource(
        "home",
        "taskres-01jz0000000000000000000001",
        &seeded_body(&remote, 10),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.pushes, 1);
    let body = mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .unwrap()
        .body;
    assert!(body.contains("alpha"));
    assert!(!body.contains("stale beta"));
    let note = std::fs::read_to_string(dir.path().join("notes/home.md")).unwrap();
    assert!(note.contains("alpha"));
}

#[tokio::test]
async fn failed_push_parks_to_the_outbox_and_flushes_later() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "queued");
    let failing = PutAlwaysFails::default();
    let failing_engine = engine_with(&dir, failing.clone(), true, Vec::new());

    let report = failing_engine.reconcile().await.unwrap();
    assert_eq!(report.parked, 1);
    assert!(failing
        .0
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_none());
    let mut outbox = restask::store::outbox::Outbox::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(outbox.take_all().len(), 1);

    // A healthy engine over the same vault replays the parked op and converges.
    let mock = MockCaldav::new();
    let healthy = engine(&dir, mock.clone(), true);
    healthy.reconcile().await.unwrap();
    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_some());
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert!(index
        .entries
        .get(&TaskUid::parse("taskres-01jz0000000000000000000001").unwrap())
        .unwrap()
        .caldav_etag
        .is_some());
    let mut outbox = restask::store::outbox::Outbox::load(&dir.path().join(".restask")).unwrap();
    assert!(outbox.take_all().is_empty());
}

#[tokio::test]
async fn foreign_task_is_adopted_into_the_inbox() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_collection("home", "Home");
    let foreign = task(
        "taskres-01jz0000000000000000000001",
        "home",
        "made in Tasks.org",
        "",
    );
    let body = to_vcalendar(
        &foreign,
        Utc.timestamp_opt(MARKER_TIME, 0).single().unwrap(),
    )
    .replace("taskres-01jz0000000000000000000001", "1789@example.com");
    mock.seed_resource("home", "1789@example.com", &body);
    // The collection is bound by the wizard (§13.2 step 5), which is what puts it in the
    // engine's scan scope.
    let engine = engine_with(
        &dir,
        mock.clone(),
        false,
        vec![restask::config::ListBinding {
            name: "Home".to_string(),
            collection: "home".to_string(),
        }],
    );

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.adoptions, 1);
    assert!(mock.resource("home", "1789@example.com").is_none());
    let adopted = mock
        .resource_names("home")
        .into_iter()
        .find(|name| name.starts_with("taskres-"))
        .expect("adopted resource pushed");
    assert!(mock
        .resource("home", &adopted)
        .unwrap()
        .body
        .contains("made in Tasks.org"));

    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("made in Tasks.org"));
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries.len(), 1);
    assert_eq!(
        index.entries.values().next().unwrap().source_path,
        "TODO.md"
    );
}

#[tokio::test]
async fn add_and_complete_round_trip() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);

    let task = engine.add("hello world", None, None).await.unwrap();
    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("- [ ] hello world"));
    assert!(todo.contains(task.uid.as_str()));
    let body = mock.resource("inbox", task.uid.as_str()).unwrap().body;
    assert!(body.contains("hello world"));

    engine.set_done(&task.uid, true).await.unwrap();
    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("## Done"));
    assert!(todo.contains("- [x] hello world"));
    let body = mock.resource("inbox", task.uid.as_str()).unwrap().body;
    assert!(body.contains("STATUS:COMPLETED"));

    engine.set_done(&task.uid, false).await.unwrap();
    let todo = std::fs::read_to_string(dir.path().join("TODO.md")).unwrap();
    assert!(todo.contains("- [ ] hello world"));
    assert!(!todo.contains("- [x] hello world"));
    let body = mock.resource("inbox", task.uid.as_str()).unwrap().body;
    assert!(!body.contains("STATUS:COMPLETED"));
}

#[tokio::test]
async fn unrouted_notes_are_never_touched() {
    let dir = temp_vault();
    write_vault_file(&dir, "notes/plain.md", "# Journal\n\n- [ ] not tracked\n");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    engine.reconcile().await.unwrap();

    let plain = std::fs::read_to_string(dir.path().join("notes/plain.md")).unwrap();
    assert_eq!(plain, "# Journal\n\n- [ ] not tracked\n");
    assert!(mock.collection_names().is_empty());
    assert!(Index::load(&dir.path().join(".restask"))
        .unwrap()
        .entries
        .is_empty());
}

#[tokio::test]
async fn duplicate_uids_across_notes_are_a_hard_error() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "first");
    write_vault_file(
        &dir,
        "notes/other.md",
        "---\nrestask-list: Home\n---\n\n- [ ] second \u{1F194} taskres-01jz0000000000000000000001\n",
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock, true);
    let error = engine.reconcile().await.unwrap_err();
    assert!(error.to_string().contains("uid conflict"), "{error}");
}

#[tokio::test]
async fn handle_fs_change_runs_a_full_reconcile() {
    let dir = temp_vault();
    seeded_note(&dir, "taskres-01jz0000000000000000000001", "watched");
    let mock = MockCaldav::new();
    let engine = engine(&dir, mock.clone(), true);
    let report = engine
        .handle_fs_change(&dir.path().join("notes/home.md"))
        .await
        .unwrap();
    assert_eq!(report.pushes, 1);
    assert!(mock
        .resource("home", "taskres-01jz0000000000000000000001")
        .is_some());
}
