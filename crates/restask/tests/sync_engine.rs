//! Engine conformance (§11, §13): full converge scenarios against a temporary routed
//! vault and the in-memory [`MockCaldav`]. Every scenario that once lost data — a pasted
//! line deleted, a remote completion reverted, a vault wiped by an emptied collection —
//! is pinned here.

mod common;

use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use tempfile::TempDir;

use common::{temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::caldav::{CaldavPort, CollectionInfo, Offline, RemoteResource};
use restask::config::{CaldavConfig, MachineConfig, VaultConfig};
use restask::domain::{ListSlug, Priority, Task, TaskUid};
use restask::store::index::Index;
use restask::store::tombstones::Tombstones;
use restask::sync::{Engine, ReconcileReport};
use restask::RestaskError;

const UID: &str = "restask-01jz0000000000000000000001";
const UID2: &str = "restask-01jz0000000000000000000002";
const ID: &str = "\u{1F194}";

/// A clock frozen at the real "now": file mtimes and the engine's instants agree, as in
/// production.
fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc::now(),
        chrono::FixedOffset::east_opt(0).unwrap(),
    ))
}

fn today() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

fn engine_with<C: CaldavPort>(dir: &TempDir, caldav: C, allow_create: bool) -> Engine<C> {
    let cfg = VaultConfig::load(&dir.path().join("restask.toml")).unwrap();
    let machine = MachineConfig {
        caldav: CaldavConfig {
            allow_create_lists: allow_create,
            ..CaldavConfig::default()
        },
        ..MachineConfig::default()
    };
    Engine::new(dir.path(), cfg, machine, caldav, clock())
}

fn engine(dir: &TempDir, mock: &MockCaldav) -> Engine<MockCaldav> {
    engine_with(dir, mock.clone(), true)
}

/// Writes `notes/home.md`, routed to list `Home`, with `body` under a `# Home` heading.
fn home_note(dir: &TempDir, body: &str) {
    note(dir, "notes/home.md", "Home", body);
}

fn note(dir: &TempDir, path: &str, list: &str, body: &str) {
    write_vault_file(
        dir,
        path,
        &format!("---\nrestask-list: {list}\n---\n\n# Home\n\n{body}"),
    );
}

fn read(dir: &TempDir, path: &str) -> String {
    std::fs::read_to_string(dir.path().join(path)).unwrap()
}

fn uid(value: &str) -> TaskUid {
    TaskUid::parse(value).unwrap()
}

/// The UID the engine gave the line containing `text` in `path`.
fn uid_of(dir: &TempDir, path: &str, text: &str) -> String {
    let contents = read(dir, path);
    let line = contents
        .lines()
        .find(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line with `{text}` in {path}:\n{contents}"));
    line.rsplit_once(&format!("{ID} "))
        .unwrap_or_else(|| panic!("unregistered line: {line}"))
        .1
        .trim()
        .to_string()
}

fn body(mock: &MockCaldav, list: &str, name: &str) -> String {
    mock.resource(list, name)
        .unwrap_or_else(|| panic!("no resource {list}/{name}"))
        .body
}

/// Rewrites a server resource the way another CalDAV client would: `edit` maps each
/// content line to its replacement lines, and `LAST-MODIFIED` is stamped `age` ago.
fn edit_remote(
    mock: &MockCaldav,
    list: &str,
    name: &str,
    age: Duration,
    edit: impl Fn(&str) -> Vec<String>,
) {
    let stamp = (Utc::now() - age).format("%Y%m%dT%H%M%SZ").to_string();
    let mut out = String::new();
    for line in body(mock, list, name)
        .split("\r\n")
        .filter(|l| !l.is_empty())
    {
        let replaced = if line.starts_with("LAST-MODIFIED") {
            vec![format!("LAST-MODIFIED:{stamp}")]
        } else {
            edit(line)
        };
        for line in replaced {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    mock.seed_resource(list, name, &out);
}

/// Completes a task on the server like Tasks.org does.
fn complete_remotely(mock: &MockCaldav, list: &str, name: &str, age: Duration) {
    edit_remote(mock, list, name, age, |line| {
        if line.starts_with("STATUS") {
            vec![
                "STATUS:COMPLETED".to_string(),
                "COMPLETED:20260920T101500Z".to_string(),
            ]
        } else {
            vec![line.to_string()]
        }
    });
}

fn mtime(dir: &TempDir, path: &str) -> SystemTime {
    std::fs::metadata(dir.path().join(path))
        .unwrap()
        .modified()
        .unwrap()
}

/// Every file under the vault with its mtime (state directory included).
fn snapshot(dir: &TempDir) -> Vec<(String, SystemTime)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, SystemTime)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push((
                    path.strip_prefix(base).unwrap().display().to_string(),
                    entry.metadata().unwrap().modified().unwrap(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir.path(), dir.path(), &mut out);
    out.sort();
    out
}

// ── first sync, idempotence ───────────────────────────────────────────────────────────

#[tokio::test]
async fn first_sync_registers_pushes_and_renders() {
    let dir = temp_vault();
    home_note(
        &dir,
        "- [ ] buy milk\n- [ ] renew the certificate \u{23EB}\n",
    );
    let mock = MockCaldav::new();

    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(report.registered, 2);
    assert_eq!(report.pushes, 2);
    assert_eq!(report.failed, 0);
    // Both routed files were parsed: the note and the engine-owned TODO.md.
    assert_eq!(report.scanned_files, 2);
    assert_eq!(mock.collection_names(), vec!["home", "inbox"]);
    assert_eq!(mock.resource_names("home").len(), 2);

    let note = read(&dir, "notes/home.md");
    assert_eq!(note.matches(ID).count(), 2);
    assert!(note.contains(&format!("buy milk \u{2795} {}", today())));

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

    // TODO.md mirrors the prioritized task; the unprioritized one stays in its note.
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains(
        "## \u{23EB} High Priority\n- [ ] renew the certificate \u{23EB} [[home#Home|home]]"
    ));
    assert!(!todo.contains("buy milk"));
}

#[tokio::test]
async fn a_second_pass_changes_nothing_anywhere() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] with created\n- [ ] without created {ID} {UID}\n- [x] done by hand\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let files = snapshot(&dir);
    let (puts, deletes, _) = mock.counters();
    std::thread::sleep(std::time::Duration::from_millis(20));
    for _ in 0..2 {
        let report = engine.reconcile().await.unwrap();
        assert_eq!(
            report,
            ReconcileReport {
                scanned_files: 2,
                ..ReconcileReport::default()
            }
        );
    }
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
    assert_eq!(snapshot(&dir), files, "an idempotent pass touches no file");
}

#[tokio::test]
async fn a_whole_list_costs_one_request_per_pass() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] a\n- [ ] b\n- [ ] c\n- [ ] d\n");
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let before = mock.counters().2;
    engine.reconcile().await.unwrap();
    // One REPORT for `home`, one for `inbox` — independent of the number of tasks.
    assert_eq!(mock.counters().2 - before, 2);
}

// ── the vault is never the victim ─────────────────────────────────────────────────────

#[tokio::test]
async fn a_cut_and_pasted_line_survives() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] movable {ID} {UID}\n"));
    note(&dir, "notes/work.md", "Work", "");
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    // Cut… (a sync happens in between, as it does with an auto-saving editor)
    home_note(&dir, "");
    engine.reconcile().await.unwrap();
    assert!(mock.resource("home", UID).is_none());
    // …and paste into another note.
    note(
        &dir,
        "notes/work.md",
        "Work",
        &format!("- [ ] movable {ID} {UID}\n"),
    );
    engine.reconcile().await.unwrap();

    assert!(read(&dir, "notes/work.md").contains("movable"));
    assert!(body(&mock, "work", UID).contains("SUMMARY:movable"));
    let tombstones = Tombstones::load(&dir.path().join(".restask")).unwrap();
    assert!(!tombstones.contains(&uid(UID)), "the vault revived it");
}

#[tokio::test]
async fn an_emptied_collection_does_not_wipe_the_vault() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] one\n- [ ] two\n- [ ] three\n");
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    for name in mock.resource_names("home") {
        mock.remove_resource("home", &name);
    }
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(note.contains("one") && note.contains("two") && note.contains("three"));
    assert_eq!(
        mock.resource_names("home").len(),
        3,
        "refilled from the vault"
    );
}

#[tokio::test]
async fn a_leftover_temp_file_does_not_block_the_engine() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] fresh\n");
    std::fs::write(
        dir.path().join("notes/.home.md.restask-tmp"),
        "crash leftover",
    )
    .unwrap();
    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    assert!(read(&dir, "notes/home.md").contains(ID));
}

#[tokio::test]
async fn unrouted_notes_are_never_touched() {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "notes/plain.md",
        "# Journal\n\n- [ ] not tracked\n- [x] nor this\n",
    );
    let before = mtime(&dir, "notes/plain.md");
    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();

    assert_eq!(
        read(&dir, "notes/plain.md"),
        "# Journal\n\n- [ ] not tracked\n- [x] nor this\n"
    );
    assert_eq!(mtime(&dir, "notes/plain.md"), before);
    assert_eq!(mock.collection_names(), vec!["inbox"]);
    assert!(Index::load(&dir.path().join(".restask"))
        .unwrap()
        .entries
        .is_empty());
}

#[tokio::test]
async fn a_duplicated_line_becomes_its_own_task() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] original {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    // Duplicate the line (Ctrl-D in an editor) and reword the copy.
    home_note(
        &dir,
        &format!("- [ ] original {ID} {UID}\n- [ ] the copy {ID} {UID}\n"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.normalized, 1);
    assert_eq!(uid_of(&dir, "notes/home.md", "original"), UID);
    let copy = uid_of(&dir, "notes/home.md", "the copy");
    assert_ne!(copy, UID);
    assert!(body(&mock, "home", UID).contains("SUMMARY:original"));
    assert!(body(&mock, "home", &copy).contains("SUMMARY:the copy"));
}

#[tokio::test]
async fn sync_conflict_copies_are_not_scanned() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] original {ID} {UID}\n"));
    note(
        &dir,
        "notes/home.sync-conflict-20260922-101500-ABCDEFG.md",
        "Home",
        &format!("- [ ] original, other device {ID} {UID}\n"),
    );
    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(mock.resource_names("home"), vec![UID]);
    assert!(body(&mock, "home", UID).contains("SUMMARY:original\r\n"));
    assert!(
        read(&dir, "notes/home.sync-conflict-20260922-101500-ABCDEFG.md").contains("other device")
    );
}

// ── the server side reaches the vault ─────────────────────────────────────────────────

#[tokio::test]
async fn a_remote_completion_survives_an_unrelated_edit_of_the_same_note() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] target {ID} {UID}\n- [ ] other\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    // Completed in Tasks.org ten minutes ago; meanwhile the note was edited elsewhere
    // (its mtime is "now"), which says nothing about *this* task.
    complete_remotely(&mock, "home", UID, Duration::minutes(10));
    let text = read(&dir, "notes/home.md").replace("other", "other, reworded");
    write_vault_file(&dir, "notes/home.md", &text);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "### Done\n- [x] target \u{2705} 2026-09-20 {ID} {UID}"
        )),
        "{note}"
    );
    assert!(body(&mock, "home", UID).contains("STATUS:COMPLETED"));
    let other = uid_of(&dir, "notes/home.md", "other, reworded");
    assert!(body(&mock, "home", &other).contains("SUMMARY:other\\, reworded"));
}

#[tokio::test]
async fn edits_on_both_sides_to_different_fields_are_both_kept() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] alpha {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    complete_remotely(&mock, "home", UID, Duration::minutes(1));
    home_note(
        &dir,
        &format!("- [ ] alpha, reworded in the note {ID} {UID}\n"),
    );
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains("- [x] alpha, reworded in the note \u{2705} 2026-09-20"),
        "{note}"
    );
    let remote = body(&mock, "home", UID);
    assert!(remote.contains("SUMMARY:alpha\\, reworded in the note"));
    assert!(remote.contains("STATUS:COMPLETED"));
}

#[tokio::test]
async fn a_vault_edit_keeps_what_other_clients_stored_on_the_resource() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] pay the rent {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    // Tasks.org adds a description and a reminder.
    edit_remote(&mock, "home", UID, Duration::minutes(5), |line| {
        if line.starts_with("END:VTODO") {
            vec![
                "DESCRIPTION:IBAN in the lease".to_string(),
                "BEGIN:VALARM".to_string(),
                "TRIGGER:-PT15M".to_string(),
                "ACTION:DISPLAY".to_string(),
                "END:VALARM".to_string(),
                line.to_string(),
            ]
        } else {
            vec![line.to_string()]
        }
    });
    engine.reconcile().await.unwrap();
    home_note(&dir, &format!("- [ ] pay the rent \u{23EB} {ID} {UID}\n"));
    engine.reconcile().await.unwrap();

    let remote = body(&mock, "home", UID);
    assert!(remote.contains("PRIORITY:3"));
    assert!(remote.contains("DESCRIPTION:IBAN in the lease\r\n"));
    assert!(remote.contains("BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\n"));
}

#[tokio::test]
async fn one_odd_remote_resource_does_not_block_the_sync() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] fine\n");
    let mock = MockCaldav::new();
    // A VTODO with nothing but a UID, and something that is no calendar at all.
    mock.seed_resource(
        "home",
        "bare",
        "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:bare\r\nDTSTAMP:20260101T000000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    );
    mock.seed_resource("home", "junk", "this is not iCalendar");
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(report.pushes, 1);
    assert_eq!(
        report.adoptions, 1,
        "the bare VTODO is a task like any other"
    );
    assert!(mock.resource("home", "junk").is_some(), "never touched");
}

#[tokio::test]
async fn a_task_deleted_on_the_server_leaves_the_vault() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] doomed {ID} {UID}\n- [ ] stays {ID} {UID2}\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    mock.remove_resource("home", UID);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(!note.contains("doomed") && note.contains("stays"));
    let state = dir.path().join(".restask");
    assert!(Tombstones::load(&state).unwrap().contains(&uid(UID)));
    assert_eq!(Index::load(&state).unwrap().entries.len(), 1);
    assert!(!state.join(format!("tasks/{UID}.ics")).exists());
}

#[tokio::test]
async fn a_line_removed_from_the_vault_leaves_the_server() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] gone {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert!(mock.resource("home", UID).is_some());

    home_note(&dir, "");
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.deletes, 1);
    assert!(mock.resource("home", UID).is_none());
    let state = dir.path().join(".restask");
    assert!(Tombstones::load(&state).unwrap().contains(&uid(UID)));
    assert!(Index::load(&state).unwrap().entries.is_empty());
    assert!(read(&dir, "notes/home.md").contains("# Home"));
}

#[tokio::test]
async fn rerouting_a_note_moves_its_tasks_between_collections() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] commute {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    note(
        &dir,
        "notes/home.md",
        "Work",
        &format!("- [ ] commute {ID} {UID}\n"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.moves, 1);
    assert!(mock.resource("home", UID).is_none());
    assert!(mock.resource("work", UID).is_some());
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries[&uid(UID)].list, slug("work"));
}

#[tokio::test]
async fn a_task_created_under_a_managed_uid_lands_in_its_note() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] anchor {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    // Another device's daemon pushed a task of the same note.
    let pushed = body(&mock, "home", UID)
        .replace(UID, UID2)
        .replace("SUMMARY:anchor", "SUMMARY:from the server");
    mock.seed_resource("home", UID2, &pushed);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.inserts, 1);

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!("- [ ] from the server {ID} {UID2}")),
        "{note}"
    );
    assert!(!note.contains("[["), "a note line, not a mirror line");
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries[&uid(UID2)].source_path, "notes/home.md");
    assert!(index.entries[&uid(UID2)].caldav_etag.is_some());
    // Settled on arrival: the next pass has nothing to do.
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.inserts), (0, 0));
}

// ── adoption ──────────────────────────────────────────────────────────────────────────

const TASKS_ORG_BODY: &str =
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:+//IDN tasks.org//android//EN\r\n\
BEGIN:VTODO\r\nDTSTAMP:20260922T101500Z\r\nUID:5417861935824551742\r\n\
CREATED:20260921T081233Z\r\nLAST-MODIFIED:20260922T101400Z\r\nSUMMARY:Made in Tasks.org\r\n\
DESCRIPTION:with a note\r\nPRIORITY:1\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

#[tokio::test]
async fn a_foreign_task_is_adopted_with_everything_it_carried() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_resource("inbox", "5417861935824551742", TASKS_ORG_BODY);
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.adoptions, 1);
    assert_eq!(report.inserts, 1);
    assert!(mock.resource("inbox", "5417861935824551742").is_none());
    let names = mock.resource_names("inbox");
    assert_eq!(names.len(), 1);
    assert!(names[0].starts_with("restask-"));
    let adopted = body(&mock, "inbox", &names[0]);
    assert!(adopted.contains("SUMMARY:Made in Tasks.org"));
    assert!(adopted.contains("DESCRIPTION:with a note\r\n"));
    assert!(adopted.contains("BEGIN:VALARM\r\nTRIGGER:-PT15M"));

    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] Made in Tasks.org \u{1F53A} \u{2795} 2026-09-21 {ID} {}",
            names[0]
        )),
        "{todo}"
    );
    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries[&uid(&names[0])].source_path, "TODO.md");

    // Settled: nothing more to do, and no second adoption.
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.adoptions, again.pushes, again.inserts), (0, 0, 0));
    assert_eq!(mock.resource_names("inbox"), names);
}

/// A port whose `put` always fails with a non-connection error; everything else works.
#[derive(Debug, Clone, Default)]
struct PutRejected(MockCaldav);

impl CaldavPort for PutRejected {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError> {
        self.0.list_collections().await
    }

    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), RestaskError> {
        self.0.ensure_collection(slug, display).await
    }

    async fn list_tasks(
        &self,
        slug: &ListSlug,
    ) -> Result<Option<Vec<RemoteResource>>, RestaskError> {
        self.0.list_tasks(slug).await
    }

    async fn put(
        &self,
        _task: &Task,
        _name: &str,
        _extras: &[String],
        _if_match: Option<&str>,
        _now: DateTime<Utc>,
    ) -> Result<String, RestaskError> {
        Err(RestaskError::Caldav {
            kind: restask::CaldavErrorKind::Protocol,
            status: Some(507),
            detail: "put rejected".to_string(),
        })
    }

    async fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), RestaskError> {
        self.0.delete(slug, name, etag).await
    }
}

#[tokio::test]
async fn an_interrupted_adoption_never_duplicates_the_task() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_resource("inbox", "5417861935824551742", TASKS_ORG_BODY);

    // First pass: the line reaches TODO.md but the server refuses the write.
    let report = engine_with(&dir, PutRejected(mock.clone()), true)
        .reconcile()
        .await
        .unwrap();
    assert_eq!((report.inserts, report.adoptions, report.failed), (1, 0, 1));
    assert!(
        mock.resource("inbox", "5417861935824551742").is_some(),
        "original kept"
    );
    let adopted = uid_of(&dir, "TODO.md", "Made in Tasks.org");

    // Second pass, healthy server: the same task is pushed, with its extras.
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!((report.inserts, report.adoptions), (0, 1));
    assert_eq!(mock.resource_names("inbox"), vec![adopted.clone()]);
    assert!(body(&mock, "inbox", &adopted).contains("DESCRIPTION:with a note"));
    assert_eq!(
        read(&dir, "TODO.md").matches("Made in Tasks.org").count(),
        1
    );
}

// ── failures ──────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_failed_push_is_replanned_from_the_current_vault_content() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] first wording {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let report = engine_with(&dir, PutRejected(mock.clone()), true)
        .reconcile()
        .await
        .unwrap();
    assert_eq!((report.pushes, report.failed), (0, 1));
    assert!(mock.resource("home", UID).is_none());
    assert!(
        Index::load(&dir.path().join(".restask"))
            .unwrap()
            .entries
            .is_empty(),
        "nothing is recorded as settled"
    );

    // The user keeps editing while the server is unhappy.
    home_note(&dir, &format!("- [ ] second wording {ID} {UID}\n"));
    engine(&dir, &mock).reconcile().await.unwrap();
    let remote = body(&mock, "home", UID);
    assert!(remote.contains("SUMMARY:second wording"));
    assert!(
        read(&dir, "notes/home.md").contains("second wording"),
        "never reverted"
    );
}

#[tokio::test]
async fn an_unreachable_server_still_gets_the_vault_side_done() {
    let dir = temp_vault();
    home_note(
        &dir,
        "- [ ] offline task \u{23EB}\n- [x] checked while offline\n",
    );
    let error = engine_with(&dir, Offline, true)
        .reconcile()
        .await
        .unwrap_err();
    assert!(matches!(error, RestaskError::Caldav { .. }), "{error}");

    // Registered, normalized and mirrored without any server.
    let note = read(&dir, "notes/home.md");
    assert_eq!(note.matches(ID).count(), 2);
    assert!(note.contains("### Done\n- [x] checked while offline"));
    assert!(read(&dir, "TODO.md").contains("- [ ] offline task \u{23EB} [[home#Home|home]]"));

    // Back online: everything converges.
    let mock = MockCaldav::new();
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(report.pushes, 2);
}

#[tokio::test]
async fn a_missing_collection_that_may_not_be_created_just_waits() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] local for now\n");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    let report = engine_with(&dir, mock.clone(), false)
        .reconcile()
        .await
        .unwrap();
    assert_eq!((report.registered, report.pushes, report.failed), (1, 0, 0));
    assert_eq!(mock.collection_names(), vec!["inbox"]);

    // Once the user creates the calendar, the task is pushed.
    mock.seed_collection("home", "Home");
    let report = engine_with(&dir, mock.clone(), false)
        .reconcile()
        .await
        .unwrap();
    assert_eq!(report.pushes, 1);
}

// ── editing by hand, in notes and in TODO.md ──────────────────────────────────────────

#[tokio::test]
async fn a_box_checked_in_any_editor_is_completed_properly() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] by hand {ID} {UID}\n- [ ] stays\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let text = read(&dir, "notes/home.md").replace("- [ ] by hand", "- [x] by hand");
    write_vault_file(&dir, "notes/home.md", &text);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.normalized, 1);

    let note = read(&dir, "notes/home.md");
    assert!(
        note.ends_with(&format!(
            "\n### Done\n- [x] by hand \u{2705} {} {ID} {UID}\n",
            today()
        )),
        "{note}"
    );
    assert!(body(&mock, "home", UID).contains("STATUS:COMPLETED"));

    // Unchecking it under Done restores it to the active list.
    let text = note.replace("- [x] by hand", "- [ ] by hand");
    write_vault_file(&dir, "notes/home.md", &text);
    engine.reconcile().await.unwrap();
    let note = read(&dir, "notes/home.md");
    let done_at = note.find("### Done").unwrap();
    let line_at = note.find("- [ ] by hand").unwrap();
    assert!(line_at < done_at, "{note}");
    assert!(!note.contains('\u{2705}'));
    assert!(body(&mock, "home", UID).contains("STATUS:NEEDS-ACTION"));
}

#[tokio::test]
async fn checking_a_mirrored_task_in_todo_md_completes_it_in_its_note() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] important \u{23EB} {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("- [ ] important"));
    write_vault_file(
        &dir,
        "TODO.md",
        &todo.replace("- [ ] important", "- [x] important"),
    );
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "### Done\n- [x] important \u{23EB} \u{2705} {}",
            today()
        )),
        "{note}"
    );
    assert!(body(&mock, "home", UID).contains("STATUS:COMPLETED"));
    // A completed note task lives under its note's Done heading, not in TODO.md.
    assert!(!read(&dir, "TODO.md").contains("important"));
}

#[tokio::test]
async fn reprioritizing_a_mirrored_task_in_todo_md_reaches_its_note() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] important \u{23EB} {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md").replace("important \u{23EB}", "very important \u{1F53A}");
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();

    assert!(
        read(&dir, "notes/home.md").contains(&format!("- [ ] very important \u{1F53A} {ID} {UID}"))
    );
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains(
        "## \u{1F53A} Highest Priority\n- [ ] very important \u{1F53A} [[home#Home|home]]"
    ));
    assert!(!todo.contains("High Priority\n"));
    let remote = body(&mock, "home", UID);
    assert!(remote.contains("PRIORITY:1") && remote.contains("SUMMARY:very important"));
}

#[tokio::test]
async fn an_inbox_task_mentioning_a_note_is_a_task_not_a_mirror() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let todo = read(&dir, "TODO.md");
    write_vault_file(
        &dir,
        "TODO.md",
        &format!("{todo}- [ ] call [[John]]\n- [ ] plain\n"),
    );
    engine.reconcile().await.unwrap();
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("- [ ] call [[John]] \u{2795}"), "{todo}");
    assert!(todo.contains("- [ ] plain \u{2795}"));
    assert_eq!(mock.resource_names("inbox").len(), 2);
}

#[tokio::test]
async fn a_mirror_line_whose_source_is_gone_does_not_become_an_inbox_task() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] important \u{23EB} {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert!(read(&dir, "TODO.md").contains("important"));

    home_note(&dir, "");
    engine.reconcile().await.unwrap();
    assert!(!read(&dir, "TODO.md").contains("important"));
    assert!(mock.resource("home", UID).is_none());
    assert!(mock.resource_names("inbox").is_empty());
}

#[tokio::test]
async fn subtasks_keep_their_parent_relation() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] parent {ID} {UID}\n    - [ ] child {ID} {UID2}\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert!(body(&mock, "home", UID2).contains(&format!("RELATED-TO;RELTYPE=PARENT:{UID}\r\n")));

    // Completing the child moves it (un-indented) under Done; the server keeps the
    // relation, which the flat done region cannot express.
    engine.set_done(&uid(UID2), true).await.unwrap();
    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "### Done\n- [x] child \u{2705} {} {ID} {UID2}",
            today()
        )),
        "{note}"
    );
    let remote = body(&mock, "home", UID2);
    assert!(remote.contains("STATUS:COMPLETED"));
    assert!(remote.contains(&format!("RELATED-TO;RELTYPE=PARENT:{UID}\r\n")));
}

// ── commands ──────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_and_complete_round_trip() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);

    let task = engine
        .add("hello\nworld", Some(Priority::Medium), None)
        .await
        .unwrap();
    assert_eq!(task.text, "hello world", "one Markdown line");
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("## \u{1F53C} Medium Priority\n- [ ] hello world \u{1F53C} \u{2795}"));
    assert!(todo.contains(task.uid.as_str()));
    assert!(body(&mock, "inbox", task.uid.as_str()).contains("SUMMARY:hello world"));

    engine.set_done(&task.uid, true).await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("## Done\n- [x] hello world"));
    assert!(body(&mock, "inbox", task.uid.as_str()).contains("STATUS:COMPLETED"));
    // Completing twice keeps the original date and changes nothing.
    engine.set_done(&task.uid, true).await.unwrap();
    assert_eq!(read(&dir, "TODO.md"), todo);

    engine.set_done(&task.uid, false).await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("- [ ] hello world") && !todo.contains("- [x] hello world"));
    assert!(!body(&mock, "inbox", task.uid.as_str()).contains("STATUS:COMPLETED"));
}

#[tokio::test]
async fn commands_work_without_a_server_and_sync_later() {
    let dir = temp_vault();
    let offline = engine_with(&dir, Offline, true);
    let task = offline.add("captured offline", None, None).await.unwrap();
    offline.set_done(&task.uid, true).await.unwrap();
    assert!(read(&dir, "TODO.md").contains("## Done\n- [x] captured offline"));

    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    assert!(body(&mock, "inbox", task.uid.as_str()).contains("STATUS:COMPLETED"));
}

#[tokio::test]
async fn set_done_on_an_unknown_uid_is_a_validation_error() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let error = engine(&dir, &mock)
        .set_done(&uid(UID), true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not found"), "{error}");
}

#[tokio::test]
async fn a_pass_waits_for_another_process_holding_the_vault() {
    // The daemon and a CLI command (e.g. from an editor keymap) share one vault: their
    // writes must never interleave.
    let dir = temp_vault();
    home_note(&dir, "- [ ] contended\n");
    let mock = MockCaldav::new();
    std::fs::create_dir_all(dir.path().join(".restask")).unwrap();
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.path().join(".restask/lock"))
        .unwrap();
    held.lock().unwrap();

    let engine = engine(&dir, &mock);
    let pass = tokio::spawn(async move { engine.reconcile().await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!pass.is_finished(), "the pass must wait for the lock");
    assert!(
        !read(&dir, "notes/home.md").contains(ID),
        "nothing written yet"
    );

    held.unlock().unwrap();
    let report = pass.await.unwrap().unwrap();
    assert_eq!(report.pushes, 1);
}

// ── recurring tasks ───────────────────────────────────────────────────────────────────

/// Makes a server task recurring the way Tasks.org does: an `RRULE` (plus a description,
/// to prove unmanaged content rides along).
fn make_recurring(mock: &MockCaldav, list: &str, name: &str, rrule: &str) {
    let rrule = rrule.to_string();
    edit_remote(mock, list, name, Duration::minutes(5), move |line| {
        if line.starts_with("END:VTODO") {
            vec![
                format!("RRULE:{rrule}"),
                "DESCRIPTION:two cups".to_string(),
                line.to_string(),
            ]
        } else {
            vec![line.to_string()]
        }
    });
}

fn day(offset: i64) -> String {
    (Utc::now() + Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

fn ical_day(offset: i64) -> String {
    (Utc::now() + Duration::days(offset))
        .format("%Y%m%d")
        .to_string()
}

#[tokio::test]
async fn completing_a_recurring_task_in_the_vault_moves_the_series_on() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!(
            "- [ ] water the plants \u{1F4C5} {} {ID} {UID}\n- [ ] other\n",
            day(0)
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    make_recurring(&mock, "home", UID, "FREQ=DAILY;INTERVAL=2");
    engine.reconcile().await.unwrap();

    // Check the box by hand.
    let text = read(&dir, "notes/home.md").replace("- [ ] water", "- [x] water");
    write_vault_file(&dir, "notes/home.md", &text);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.failed, 0);

    // The series is open again on the server, due at the next occurrence, rule intact.
    let series = body(&mock, "home", UID);
    assert!(series.contains("STATUS:NEEDS-ACTION"), "{series}");
    assert!(!series.contains("COMPLETED:"));
    assert!(series.contains(&format!("DUE;VALUE=DATE:{}\r\n", ical_day(2))));
    assert!(series.contains("RRULE:FREQ=DAILY;INTERVAL=2\r\n"));
    assert!(series.contains("DESCRIPTION:two cups\r\n"));

    // In the note: the series line is active with the new date; the occurrence that was
    // done is a record under Done with a UID of its own.
    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "- [ ] water the plants \u{1F4C5} {} {ID} {UID}\n",
            day(2)
        )),
        "{note}"
    );
    let record = uid_of(&dir, "notes/home.md", "- [x] water the plants");
    assert_ne!(record, UID);
    assert!(note.contains(&format!(
        "### Done\n- [x] water the plants \u{1F4C5} {} \u{2705} {} {ID} {record}\n",
        day(0),
        today()
    )));

    // The record is an ordinary completed task on the server: no rule, no description.
    let done = body(&mock, "home", &record);
    assert!(done.contains("STATUS:COMPLETED"));
    assert!(!done.contains("RRULE") && !done.contains("DESCRIPTION"));

    // Converged: another pass does nothing, and never rolls the series again.
    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!(
        (again.pushes, again.inserts, again.markdown_mutations),
        (0, 0, 0)
    );
    assert_eq!(snapshot(&dir), files);
    assert_eq!(mock.resource_names("home").len(), 3);
}

#[tokio::test]
async fn the_last_occurrence_of_a_counted_rule_completes_the_task() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] two more times \u{1F4C5} {} {ID} {UID}\n", day(0)),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    make_recurring(&mock, "home", UID, "FREQ=DAILY;COUNT=2");
    engine.reconcile().await.unwrap();

    engine.set_done(&uid(UID), true).await.unwrap();
    let series = body(&mock, "home", UID);
    assert!(series.contains("STATUS:NEEDS-ACTION"));
    assert!(series.contains("RRULE:FREQ=DAILY;COUNT=1\r\n"), "{series}");
    assert!(series.contains(&format!("DUE;VALUE=DATE:{}\r\n", ical_day(1))));

    // The second completion is the last occurrence: now the task itself is done.
    engine.set_done(&uid(UID), true).await.unwrap();
    assert!(body(&mock, "home", UID).contains("STATUS:COMPLETED"));
    let note = read(&dir, "notes/home.md");
    assert_eq!(note.matches("- [x] two more times").count(), 2);
    assert!(!note.contains("- [ ] two more times"));
}

#[tokio::test]
async fn a_recurring_inbox_task_rolls_forward_inside_todo_md() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let task = engine.add("take vitamins", None, None).await.unwrap();
    make_recurring(&mock, "inbox", task.uid.as_str(), "FREQ=DAILY");
    engine.reconcile().await.unwrap();

    engine.set_done(&task.uid, true).await.unwrap();
    let todo = read(&dir, "TODO.md");
    // No date before: the next occurrence becomes the due date.
    assert!(
        todo.contains(&format!(
            "## Inbox\n- [ ] take vitamins \u{1F4C5} {} \u{2795} {} {ID} {}\n",
            day(1),
            today(),
            task.uid
        )),
        "{todo}"
    );
    assert!(todo.contains("## Done\n- [x] take vitamins \u{2705}"));
    let series = body(&mock, "inbox", task.uid.as_str());
    assert!(series.contains("STATUS:NEEDS-ACTION") && series.contains("RRULE:FREQ=DAILY"));
}
