//! Engine conformance (§11, §13): full converge scenarios against a temporary routed
//! vault and the in-memory [`MockCaldav`]. Every scenario that once lost data — a pasted
//! line deleted, a remote completion reverted, a vault wiped by an emptied collection —
//! is pinned here.

mod common;

use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use tempfile::TempDir;

use common::{sealed, temp_vault, write_vault_file, FixedClock, MockCaldav};
use restask::caldav::{CaldavPort, CollectionInfo, Offline, RemoteResource};
use restask::config::{CaldavConfig, MachineConfig, VaultConfig};
use restask::domain::{ListSlug, Priority, Task, TaskUid};
use restask::store::index::Index;
use restask::store::tombstones::Tombstones;
use restask::sync::{Engine, ReconcileReport};
use restask::vtodo::WireNames;
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

/// A view the plugin edited and sealed again (§15.6): the old seal line replaced.
fn resealed(view: &str) -> String {
    let unsealed: String = view
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("restask-render: "))
        .collect();
    sealed(&unsealed)
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
    // Registered with a UID only: the creation date is not written unless asked for.
    assert!(note.contains(&format!("- [ ] buy milk {ID} ")), "{note}");
    assert!(!note.contains('\u{2795}'), "{note}");

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

/// The copies a file sync's version archive holds of a list's root note, of a note below
/// it and of the view — the owner's vault on the sync node (T82), under `archive`.
fn archived_copies(dir: &TempDir, archive: &str) -> Vec<(String, String)> {
    let copies = vec![
        (
            format!("{archive}/Areas/Home Lab/Home Lab~20261002-195517.md"),
            format!(
                "---\nrestask-list-root: Home Lab\n---\n# To Do\n- [ ] backup the phone \u{1F53C}\n- [ ] keep \u{23EB} {ID} {UID}\n"
            ),
        ),
        (
            format!("{archive}/Areas/Home Lab/Services/Frigate~20261002-201357.md"),
            "# To Do\n- [ ] add the cameras \u{1F53D}\n".to_string(),
        ),
        (
            format!("{archive}/TODO~20261002-201807.md"),
            "---\nrestask-list: inbox\n---\n# TODO\n\n## \u{1F53C} Medium Priority\n- [ ] an earlier line of the view \u{1F53C}\n\n## Done\n".to_string(),
        ),
    ];
    for (path, contents) in &copies {
        write_vault_file(dir, path, contents);
    }
    copies
}

#[tokio::test]
async fn a_file_syncs_version_archive_is_not_part_of_the_vault() {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "Areas/Home Lab/Home Lab.md",
        &format!(
            "---\nrestask-list-root: Home Lab\n---\n# To Do\n- [ ] keep \u{23EB} {ID} {UID}\n"
        ),
    );
    write_vault_file(
        &dir,
        "Areas/Home Lab/Services/Frigate.md",
        "# To Do\n- [ ] add the cameras \u{1F53D}\n",
    );
    let copies = archived_copies(&dir, ".stversions");
    // Hidden at any depth, and a hidden file as well.
    write_vault_file(
        &dir,
        "Areas/Home Lab/.old/Notes.md",
        "# To Do\n- [ ] in a hidden folder \u{1F53C}\n",
    );
    write_vault_file(
        &dir,
        "Areas/Home Lab/.Draft.md",
        "# To Do\n- [ ] in a hidden file \u{1F53C}\n",
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    // The two notes and the view; one line to register, the note's own.
    assert_eq!(report.scanned_files, 3);
    assert_eq!(report.registered, 1);
    assert_eq!(mock.resource_names("home-lab").len(), 2);
    assert!(mock.resource_names("inbox").is_empty());
    for (path, contents) in &copies {
        assert_eq!(&read(&dir, path), contents, "{path}");
    }
    assert!(!read(&dir, "Areas/Home Lab/.old/Notes.md").contains(ID));
    assert!(!read(&dir, "Areas/Home Lab/.Draft.md").contains(ID));
    // The task both the note and its archived copy hold is the note's: one mirror line,
    // linked to the note.
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "- [ ] keep \u{23EB} [[Home Lab#To Do|Home Lab]] {ID} {UID}\n"
        )),
        "{todo}"
    );
    assert!(todo.contains("[[Frigate#To Do|Frigate]]"), "{todo}");
    assert!(!todo.contains("~2026"), "{todo}");
    assert!(!todo.contains("backup the phone"), "{todo}");
    assert!(!todo.contains("hidden"), "{todo}");

    let files = snapshot(&dir);
    let (puts, deletes, _) = mock.counters();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        report,
        ReconcileReport {
            scanned_files: 3,
            ..ReconcileReport::default()
        }
    );
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
    assert_eq!(snapshot(&dir), files);
}

#[tokio::test]
async fn tasks_an_earlier_engine_took_from_the_version_archive_leave_with_it() {
    // What an engine that walked into the archive left behind: the copies registered,
    // pushed and mirrored. Made here with the archive in a folder the scan does read.
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "Areas/Home Lab/Home Lab.md",
        &format!(
            "---\nrestask-list-root: Home Lab\n---\n# To Do\n- [ ] keep \u{23EB} {ID} {UID}\n"
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    archived_copies(&dir, "0-archive");
    engine.reconcile().await.unwrap();
    // `keep`, and from the copies: its duplicate, `backup the phone`, `add the cameras`.
    assert_eq!(mock.resource_names("home-lab").len(), 4);
    assert_eq!(mock.resource_names("inbox").len(), 1);
    assert!(read(&dir, "TODO.md").contains("Home Lab~20261002-195517"));

    // The archive is where the file sync keeps it: hidden.
    std::fs::rename(dir.path().join("0-archive"), dir.path().join(".stversions")).unwrap();
    let archived = snapshot(&dir)
        .into_iter()
        .filter(|(path, _)| path.starts_with(".stversions"))
        .collect::<Vec<_>>();
    assert_eq!(archived.len(), 3);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.deletes, 4);
    assert_eq!(mock.resource_names("home-lab"), vec![UID]);
    assert!(mock.resource_names("inbox").is_empty());
    let todo = read(&dir, "TODO.md");
    assert_eq!(
        todo,
        sealed(&format!(
            "---\nrestask-list: inbox\n---\n# TODO\n\n## \u{23EB} High Priority\n- [ ] keep \u{23EB} [[Home Lab#To Do|Home Lab]] {ID} {UID}\n\n## Done\n"
        ))
    );
    assert_eq!(
        read(&dir, "Areas/Home Lab/Home Lab.md"),
        format!("---\nrestask-list-root: Home Lab\n---\n# To Do\n- [ ] keep \u{23EB} {ID} {UID}\n")
    );
    // The copies are left as the earlier engine wrote them.
    let after = snapshot(&dir)
        .into_iter()
        .filter(|(path, _)| path.starts_with(".stversions"))
        .collect::<Vec<_>>();
    assert_eq!(after, archived);

    let files = snapshot(&dir);
    let (puts, deletes, _) = mock.counters();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        report,
        ReconcileReport {
            scanned_files: 2,
            ..ReconcileReport::default()
        }
    );
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
    assert_eq!(snapshot(&dir), files);
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
    assert_eq!(
        report.adoptions, 1,
        "the bare VTODO is a task like any other"
    );
    assert_eq!(
        report.pushes, 2,
        "the vault's task, and the link to the bare one"
    );
    assert!(body(&mock, "home", "bare").contains("UID:bare\r\n"));
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

const TASKS_ORG_NAME: &str = "5417861935824551742";

#[tokio::test]
async fn a_foreign_task_is_adopted_where_it_is_with_everything_it_carries() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_resource("inbox", TASKS_ORG_NAME, TASKS_ORG_BODY);
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.adoptions, 1);
    assert_eq!(report.inserts, 1);
    assert_eq!(report.deletes, 0);
    let adopted = uid_of(&dir, "TODO.md", "Made in Tasks.org");
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] Made in Tasks.org \u{1F53A} {ID} {adopted}"
        )),
        "{todo}"
    );

    // The resource is still the one Tasks.org made — its name, its `UID` — and now says
    // which task of the vault it is.
    assert_eq!(mock.resource_names("inbox"), vec![TASKS_ORG_NAME]);
    let linked = body(&mock, "inbox", TASKS_ORG_NAME);
    assert!(linked.contains("UID:5417861935824551742\r\n"), "{linked}");
    assert!(linked.contains(&format!("X-RESTASK-UID:{adopted}\r\n")));
    assert!(linked.contains("X-RESTASK-SOURCE;VALUE=TEXT:TODO.md\r\n"));
    assert!(linked.contains("SUMMARY:Made in Tasks.org"));
    assert!(linked.contains("CREATED:20260921T000000Z\r\n"));
    assert!(linked.contains("DESCRIPTION:with a note\r\n"));
    assert!(linked.contains("BEGIN:VALARM\r\nTRIGGER:-PT15M"));

    let index = Index::load(&dir.path().join(".restask")).unwrap();
    assert_eq!(index.entries[&uid(&adopted)].source_path, "TODO.md");

    // Settled: nothing more to do, and no second adoption.
    let counters = mock.counters();
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.adoptions, again.pushes, again.inserts), (0, 0, 0));
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(read(&dir, "TODO.md"), todo);
}

/// The owner's report: "Changing a priority on the tasks.org app on my phone doesn't
/// change the connected task in the vault but create a new one keeping both the old and
/// new priority." The task had been made in Tasks.org; restask had re-created it under a
/// UID of its own and deleted the original, so Tasks.org — which still held the original
/// — wrote its edit to a resource the vault line was no longer tied to.
#[tokio::test]
async fn a_priority_changed_in_tasks_org_changes_the_task_in_the_vault() {
    let dir = temp_vault();
    home_note(&dir, "");
    let mock = MockCaldav::new();
    mock.seed_resource("home", TASKS_ORG_NAME, TASKS_ORG_BODY);
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let adopted = uid_of(&dir, "notes/home.md", "Made in Tasks.org");
    let one_task = |priority: &str| {
        let line = format!("- [ ] Made in Tasks.org {priority} {ID} {adopted}\n");
        let note = read(&dir, "notes/home.md");
        assert_eq!(note.matches("Made in Tasks.org").count(), 1, "{note}");
        assert!(note.contains(&line), "{note}");
        let todo = read(&dir, "TODO.md");
        assert_eq!(todo.matches("Made in Tasks.org").count(), 1, "{todo}");
        assert!(todo.contains(&format!("Made in Tasks.org {priority} [[")));
        assert_eq!(mock.resource_names("home"), vec![TASKS_ORG_NAME]);
    };
    one_task("\u{1F53A}");

    // Tasks.org uploads its own copy with a new priority, as it holds it: it has not
    // read what restask wrote in between.
    mock.seed_resource(
        "home",
        TASKS_ORG_NAME,
        &TASKS_ORG_BODY.replace("PRIORITY:1\r\n", "PRIORITY:5\r\n"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        (report.adoptions, report.inserts, report.deletes),
        (0, 0, 0)
    );
    one_task("\u{1F53C}");
    let linked = body(&mock, "home", TASKS_ORG_NAME);
    assert!(linked.contains("PRIORITY:5\r\n"), "{linked}");
    assert!(linked.contains(&format!("X-RESTASK-UID:{adopted}\r\n")));

    // And once more on the version restask wrote, properties kept.
    mock.seed_resource(
        "home",
        TASKS_ORG_NAME,
        &linked.replace("PRIORITY:5\r\n", "PRIORITY:9\r\n"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        (
            report.adoptions,
            report.inserts,
            report.deletes,
            report.pushes
        ),
        (0, 0, 0, 0)
    );
    one_task("\u{23EC}");

    // The other way: a priority set in the note is written to that same resource.
    let note = read(&dir, "notes/home.md").replace('\u{23EC}', "\u{23EB}");
    write_vault_file(&dir, "notes/home.md", &note);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.pushes, 1);
    one_task("\u{23EB}");
    let pushed = body(&mock, "home", TASKS_ORG_NAME);
    assert!(pushed.contains("UID:5417861935824551742\r\n"), "{pushed}");
    assert!(pushed.contains("PRIORITY:3\r\n"));
    assert!(pushed.contains("DESCRIPTION:with a note\r\n"));

    // Quiet afterwards.
    let (counters, files) = (
        mock.counters(),
        (read(&dir, "notes/home.md"), read(&dir, "TODO.md")),
    );
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.markdown_mutations), (0, 0));
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!((read(&dir, "notes/home.md"), read(&dir, "TODO.md")), files);
}

#[tokio::test]
async fn an_adopted_task_deleted_in_the_vault_or_by_its_client_is_gone_on_both_sides() {
    let dir = temp_vault();
    home_note(&dir, "");
    let mock = MockCaldav::new();
    mock.seed_resource("home", TASKS_ORG_NAME, TASKS_ORG_BODY);
    mock.seed_resource(
        "home",
        "second",
        &TASKS_ORG_BODY
            .replace("UID:5417861935824551742", "UID:second@tasks.org")
            .replace("Made in Tasks.org", "Second one"),
    );
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert_eq!(mock.resource_names("home").len(), 2);

    // Deleted in Tasks.org: the line goes.
    mock.remove_resource("home", TASKS_ORG_NAME);
    engine.reconcile().await.unwrap();
    let note = read(&dir, "notes/home.md");
    assert!(!note.contains("Made in Tasks.org"), "{note}");
    assert!(note.contains("Second one"));

    // Deleted in the note: the resource of the other client goes.
    let note: String = note
        .lines()
        .filter(|line| !line.contains("Second one"))
        .map(|line| format!("{line}\n"))
        .collect();
    write_vault_file(&dir, "notes/home.md", &note);
    engine.reconcile().await.unwrap();
    assert!(mock.resource_names("home").is_empty());
    assert!(!read(&dir, "TODO.md").contains("Second one"));
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
        collection: &str,
        slug: &ListSlug,
    ) -> Result<Option<Vec<RemoteResource>>, RestaskError> {
        self.0.list_tasks(collection, slug).await
    }

    async fn put(
        &self,
        _task: &Task,
        _collection: &str,
        _name: &str,
        _extras: &[String],
        _wire: &WireNames,
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
        collection: &str,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), RestaskError> {
        self.0.delete(collection, name, etag).await
    }
}

#[tokio::test]
async fn an_interrupted_adoption_never_duplicates_the_task() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_resource("inbox", TASKS_ORG_NAME, TASKS_ORG_BODY);

    // First pass: the line reaches TODO.md but the server refuses the write.
    let report = engine_with(&dir, PutRejected(mock.clone()), true)
        .reconcile()
        .await
        .unwrap();
    assert_eq!((report.inserts, report.pushes, report.failed), (1, 0, 1));
    assert_eq!(
        body(&mock, "inbox", TASKS_ORG_NAME),
        TASKS_ORG_BODY,
        "untouched"
    );
    let adopted = uid_of(&dir, "TODO.md", "Made in Tasks.org");

    // Tasks.org edits the task before the next pass: its change is not overruled by
    // the line the first pass wrote.
    mock.seed_resource(
        "inbox",
        TASKS_ORG_NAME,
        &TASKS_ORG_BODY.replace("PRIORITY:1\r\n", "PRIORITY:9\r\n"),
    );

    // Second pass, healthy server: the same task is linked, with its extras.
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!((report.inserts, report.adoptions, report.pushes), (0, 0, 1));
    assert_eq!(mock.resource_names("inbox"), vec![TASKS_ORG_NAME]);
    let linked = body(&mock, "inbox", TASKS_ORG_NAME);
    assert!(linked.contains("DESCRIPTION:with a note"));
    assert!(linked.contains(&format!("X-RESTASK-UID:{adopted}\r\n")));
    assert!(linked.contains("PRIORITY:9\r\n"), "{linked}");
    let todo = read(&dir, "TODO.md");
    assert_eq!(todo.matches("Made in Tasks.org").count(), 1);
    assert!(todo.contains(&format!("Made in Tasks.org \u{23EC} {ID} {adopted}")));
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

/// §7.1: a mirror line is the task. Deleted in TODO.md (by hand, in any editor), the
/// task is deleted in its note and on the server; nothing else in the note changes.
#[tokio::test]
async fn deleting_a_mirrored_task_in_todo_md_deletes_it_in_its_note() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] important \u{23EB} {ID} {UID}\n- [ ] other \u{23EB} {ID} {UID2}\nprose\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert!(mock.resource("home", UID).is_some());

    let todo = read(&dir, "TODO.md");
    let line = format!("- [ ] important \u{23EB} [[home#Home|home]] {ID} {UID}\n");
    assert!(todo.contains(&line), "{todo}");
    write_vault_file(&dir, "TODO.md", &todo.replace(&line, ""));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.failed, 0);

    let note = read(&dir, "notes/home.md");
    assert!(!note.contains("important"), "{note}");
    assert!(
        note.contains(&format!("- [ ] other \u{23EB} {ID} {UID2}\nprose\n")),
        "{note}"
    );
    assert!(
        mock.resource("home", UID).is_none(),
        "deleted on the server"
    );
    assert!(mock.resource("home", UID2).is_some());
    let todo = read(&dir, "TODO.md");
    assert!(
        !todo.contains("important") && todo.contains("other"),
        "{todo}"
    );
    assert_eq!(todo, resealed(&todo), "the view is a render again");

    // Converged: nothing is written, nothing is sent.
    let files = snapshot(&dir);
    let (puts, deletes, _) = mock.counters();
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.failed), (0, 0));
    assert_eq!(snapshot(&dir), files);
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
}

/// A mirror line that is merely absent deletes nothing (invariants 1 and 2): a TODO.md
/// that is missing, cut short, or edited from another render than the engine's last one
/// is put right by the render, and the note and the server keep the task.
#[tokio::test]
async fn a_mirror_line_that_is_merely_absent_deletes_nothing() {
    let dir = temp_vault();
    let body_of_note = format!("- [ ] important \u{23EB} {ID} {UID}\n");
    home_note(&dir, &body_of_note);
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    let note = read(&dir, "notes/home.md");
    let line = format!("- [ ] important \u{23EB} [[home#Home|home]] {ID} {UID}\n");
    assert!(todo.contains(&line), "{todo}");
    let gone = todo.replace(&line, "");

    let cut_short: String = todo.lines().take(4).map(|l| format!("{l}\n")).collect();
    let stale = gone.replacen("restask-render: ", "restask-render: 0", 1);
    for broken in [Some(cut_short), Some(stale), Some(String::new()), None] {
        match &broken {
            Some(text) => write_vault_file(&dir, "TODO.md", text),
            None => std::fs::remove_file(dir.path().join("TODO.md")).unwrap(),
        }
        let report = engine.reconcile().await.unwrap();
        assert_eq!(report.failed, 0, "{broken:?}");
        assert_eq!(read(&dir, "notes/home.md"), note, "{broken:?}");
        assert!(mock.resource("home", UID).is_some(), "{broken:?}");
        assert_eq!(read(&dir, "TODO.md"), todo, "{broken:?}");
    }

    // The note changed the task since the render: the note wins, the line comes back.
    home_note(&dir, &body_of_note.replace("important", "very important"));
    write_vault_file(&dir, "TODO.md", &gone);
    engine.reconcile().await.unwrap();
    assert!(read(&dir, "notes/home.md").contains("very important"));
    assert!(read(&dir, "TODO.md").contains("very important"));
    assert!(body(&mock, "home", UID).contains("SUMMARY:very important"));
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
    assert!(
        todo.contains(&format!("- [ ] call [[John]] {ID} ")),
        "{todo}"
    );
    assert!(todo.contains(&format!("- [ ] plain {ID} ")));
    assert_eq!(mock.resource_names("inbox").len(), 2);
}

/// §7.4: the section a task is typed in is its priority; a priority written on the line
/// itself wins, and so does one changed later — the section is only where the task starts.
#[tokio::test]
async fn a_task_typed_in_a_priority_section_takes_that_priority() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] important \u{23EB} {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md").replace(
        "\n## Done\n",
        "- [ ] typed here\n- [ ] typed low \u{1F53D}\n\n## Done\n- [ ] typed under done\n",
    );
    write_vault_file(&dir, "TODO.md", &todo);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.registered, 3);

    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{23EB} High Priority\n- [ ] typed here \u{23EB} {ID} "
        )),
        "{todo}"
    );
    assert!(
        todo.contains(&format!(
            "## \u{1F53D} Low Priority\n- [ ] typed low \u{1F53D} {ID} "
        )),
        "{todo}"
    );
    assert!(
        todo.contains(&format!("## No Priority\n- [ ] typed under done {ID} ")),
        "{todo}"
    );
    let typed = uid_of(&dir, "TODO.md", "typed here");
    assert!(body(&mock, "inbox", &typed).contains("PRIORITY:3"));
    // The mirrored note task is not touched by any of it.
    assert!(read(&dir, "notes/home.md").contains(&format!("- [ ] important \u{23EB} {ID} {UID}")));

    // A second pass is quiet.
    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.registered, again.pushes), (0, 0));
    assert_eq!(snapshot(&dir), files);

    // Taking the priority off a registered task moves it out; the section does not
    // give it back.
    let todo = read(&dir, "TODO.md").replace("typed here \u{23EB}", "typed here");
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!("## No Priority\n- [ ] typed here {ID} ")),
        "{todo}"
    );
    assert!(!body(&mock, "inbox", &typed).contains("PRIORITY:"));
}

/// §6.4: a checkbox without text is a task still to be written — the line an editor opens
/// under a TODO heading (§15.7), or the one a list continuation leaves behind. It is not
/// registered, not completed and not pushed until it has text; the plugin leaves it
/// alone likewise (§15.6).
#[tokio::test]
async fn a_checkbox_without_text_is_not_a_task_yet() {
    let dir = temp_vault();
    let body = "# TODO\n- [ ] \n- [ ] \u{1F53A}\n\t- [x] \n- [ ] real";
    home_note(&dir, &format!("{body}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.registered, report.pushes), (1, 1));

    // The three empty boxes are where they were, as they were; nothing was moved under
    // a done heading for the checked one.
    let note = read(&dir, "notes/home.md");
    assert!(note.contains(&format!("{body} {ID} ")), "{note}");
    assert_eq!(note.matches(ID).count(), 1, "{note}");
    assert!(!note.contains("Done"), "{note}");
    assert_eq!(mock.resource_names("home").len(), 1);

    // In TODO.md an empty box under a section is no task of the view either: the render
    // drops it like any line that is none (§7), and nothing reaches the inbox list.
    let todo = read(&dir, "TODO.md").replace("\n## Done\n", "## No Priority\n- [ ] \n\n## Done\n");
    write_vault_file(&dir, "TODO.md", &todo);
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.registered, report.pushes), (0, 0));
    assert!(!read(&dir, "TODO.md").contains("- [ ] \n"));
    assert!(mock.resource_names("inbox").is_empty());

    // A second pass is quiet.
    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.registered, again.pushes), (0, 0));
    assert_eq!(snapshot(&dir), files);

    // Once it has text it is a task like any other.
    write_vault_file(
        &dir,
        "notes/home.md",
        &read(&dir, "notes/home.md").replace("# TODO\n- [ ] \n", "# TODO\n- [ ] written\n"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.registered, report.pushes), (1, 1));
    assert!(read(&dir, "notes/home.md").contains(&format!("- [ ] written {ID} ")));
    assert_eq!(mock.resource_names("home").len(), 2);
}

/// Cuts the line containing `text` out of `todo` and pastes it as the first line under
/// `heading`, writing the heading in front of `## Done` when the view does not have it.
fn move_line(todo: &str, text: &str, heading: &str) -> String {
    let line = todo
        .lines()
        .find(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line with `{text}`:\n{todo}"));
    let without = todo.replacen(&format!("{line}\n"), "", 1);
    if without.contains(&format!("{heading}\n")) {
        without.replacen(&format!("{heading}\n"), &format!("{heading}\n{line}\n"), 1)
    } else {
        without.replacen("## Done\n", &format!("{heading}\n{line}\n\n## Done\n"), 1)
    }
}

/// §7.4: a line moved to another section, its emoji left as it was, takes that section's
/// priority — the render used to put it back.
#[tokio::test]
async fn a_task_moved_to_another_section_of_todo_md_takes_its_priority() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let plain = engine.add("plain", None, None, None).await.unwrap();
    let urgent = engine
        .add("urgent", Some(Priority::Highest), None, None)
        .await
        .unwrap();
    assert!(body(&mock, "inbox", urgent.uid.as_str()).contains("PRIORITY:1"));

    // Up into a section that exists, and down into one written by hand.
    let todo = move_line(
        &read(&dir, "TODO.md"),
        "plain",
        "## \u{1F53A} Highest Priority",
    );
    let todo = move_line(&todo, "urgent", "## \u{1F53D} Low Priority");
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] plain \u{1F53A} {ID} "
        )),
        "{todo}"
    );
    assert!(
        todo.contains(&format!(
            "## \u{1F53D} Low Priority\n- [ ] urgent \u{1F53D} {ID} "
        )),
        "{todo}"
    );
    assert!(!todo.contains("## No Priority"), "{todo}");
    assert!(body(&mock, "inbox", plain.uid.as_str()).contains("PRIORITY:1"));
    assert!(body(&mock, "inbox", urgent.uid.as_str()).contains("PRIORITY:7"));

    // A second pass is quiet: the move was taken once.
    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.normalized), (0, 0));
    assert_eq!(snapshot(&dir), files);

    // Moved under No Priority, the task loses its priority.
    let todo = move_line(&read(&dir, "TODO.md"), "plain", "## No Priority");
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!("## No Priority\n- [ ] plain {ID} ")),
        "{todo}"
    );
    assert!(!body(&mock, "inbox", plain.uid.as_str()).contains("PRIORITY:"));
}

/// §7.4 for a mirror line: the move is a change of priority, made in the note.
#[tokio::test]
async fn a_mirror_line_moved_to_another_section_reprioritizes_its_note_task() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] important \u{23EB} {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let todo = move_line(
        &read(&dir, "TODO.md"),
        "important",
        "## \u{1F53D} Low Priority",
    );
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();

    assert!(read(&dir, "notes/home.md").contains(&format!("- [ ] important \u{1F53D} {ID} {UID}")));
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains("## \u{1F53D} Low Priority\n- [ ] important \u{1F53D} [[home#Home|home]]"),
        "{todo}"
    );
    assert!(!todo.contains("High Priority"), "{todo}");
    assert!(body(&mock, "home", UID).contains("PRIORITY:7"));

    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.normalized), (0, 0));
    assert_eq!(snapshot(&dir), files);
}

/// §7.2 holds for moves too: a sealed view is another device's render, and a line that
/// stands elsewhere in it than in this engine's last render is no move of the user's —
/// the note that explains it may still be on its way.
#[tokio::test]
async fn a_sealed_view_with_a_line_elsewhere_is_not_read_as_a_move() {
    let dir = temp_vault();
    let note = format!("- [ ] important \u{23EB} {ID} {UID}\n");
    home_note(&dir, &note);
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let rendered = read(&dir, "TODO.md");

    let todo = move_line(&rendered, "important", "## \u{1F53D} Low Priority");
    write_vault_file(&dir, "TODO.md", &resealed(&todo));
    let report = engine.reconcile().await.unwrap();

    assert_eq!(report.pushes, 0);
    assert!(read(&dir, "notes/home.md").ends_with(&note));
    assert_eq!(read(&dir, "TODO.md"), rendered);
    assert!(body(&mock, "home", UID).contains("PRIORITY:3"));
}

/// §7.4: an emoji the user changed is the priority they named; where the line stands
/// does not overrule it. And a move under Done says nothing about the priority.
#[tokio::test]
async fn a_changed_emoji_outranks_the_section_the_line_was_moved_to() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let both = engine
        .add("both", Some(Priority::Highest), None, None)
        .await
        .unwrap();
    let parked = engine
        .add("parked", Some(Priority::Highest), None, None)
        .await
        .unwrap();

    let todo = move_line(&read(&dir, "TODO.md"), "both", "## \u{1F53D} Low Priority")
        .replace("both \u{1F53A}", "both \u{1F53C}");
    let todo = move_line(&todo, "parked", "## Done");
    write_vault_file(&dir, "TODO.md", &todo);
    engine.reconcile().await.unwrap();

    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53C} Medium Priority\n- [ ] both \u{1F53C} {ID} "
        )),
        "{todo}"
    );
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] parked \u{1F53A} {ID} "
        )),
        "{todo}"
    );
    assert!(body(&mock, "inbox", both.uid.as_str()).contains("PRIORITY:5"));
    assert!(body(&mock, "inbox", parked.uid.as_str()).contains("PRIORITY:1"));
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
        .add("hello\nworld", Some(Priority::Medium), None, None)
        .await
        .unwrap();
    assert_eq!(task.text, "hello world", "one Markdown line");
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains(&format!(
        "## \u{1F53C} Medium Priority\n- [ ] hello world \u{1F53C} {ID} "
    )));
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
    let task = offline
        .add("captured offline", None, None, None)
        .await
        .unwrap();
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

const REPEAT: &str = "\u{1F501}";
const DUE: &str = "\u{1F4C5}";

/// Adds an `RRULE` (plus a description, to prove unmanaged content rides along) to a
/// server task, the way another client would.
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

/// The owner's line, `Collect feedback 🔽 🔁 every 2 weeks for 5 times`: a rule and no
/// date. Radicale answered 400 to its `VTODO` at every pass (a rule has no first
/// occurrence without a date), so the task never left the vault. The wire now carries an
/// anchor the vault does not show (§8.1).
#[tokio::test]
async fn a_repeating_task_without_a_date_reaches_the_server_and_gets_no_date_in_the_vault() {
    let dir = temp_vault();
    let line = format!("- [ ] Collect feedback {REPEAT} every 2 weeks for 5 times {ID} {UID}\n");
    home_note(&dir, &line);
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.pushes, 1);

    let sent = body(&mock, "home", UID);
    assert!(
        sent.contains("RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=5\r\n"),
        "{sent}"
    );
    let anchor = sent
        .lines()
        .find(|line| line.starts_with("DTSTART"))
        .expect("a rule needs a date on the wire");
    assert!(
        anchor.starts_with("DTSTART;VALUE=DATE;X-RESTASK-ANCHOR=TRUE:"),
        "{anchor}"
    );
    assert!(!sent.contains("\r\nDUE"), "{sent}");

    // The anchor is the wire's: the line keeps the dates it has — none — and a second
    // pass writes no file and sends nothing.
    assert!(read(&dir, "notes/home.md").contains(&line));
    let files = snapshot(&dir);
    let (puts, _, _) = mock.counters();
    let again = engine.reconcile().await.unwrap();
    assert_eq!(again.pushes, 0);
    assert_eq!(snapshot(&dir), files);
    assert_eq!(mock.counters().0, puts);
    assert!(!read(&dir, "notes/home.md").contains('\u{1F6EB}'));

    // A date typed later is the task's own: the anchor gives way to it.
    home_note(
        &dir,
        &format!(
            "- [ ] Collect feedback {REPEAT} every 2 weeks for 5 times {DUE} {} {ID} {UID}\n",
            day(3)
        ),
    );
    engine.reconcile().await.unwrap();
    let sent = body(&mock, "home", UID);
    assert!(
        sent.contains(&format!("DUE;VALUE=DATE:{}\r\n", ical_day(3))),
        "{sent}"
    );
    assert!(!sent.contains("DTSTART"), "{sent}");
}

#[tokio::test]
async fn a_repeat_rule_written_in_the_vault_reaches_the_server_and_back() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] water the plants {REPEAT} every week on mon and thu {ID} {UID}\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    assert!(body(&mock, "home", UID).contains("RRULE:FREQ=WEEKLY;BYDAY=MO,TH\r\n"));
    assert!(body(&mock, "home", UID).contains("SUMMARY:water the plants\r\n"));

    // Changed in another client: the vault line follows, in canonical spelling.
    edit_remote(&mock, "home", UID, Duration::minutes(1), |line| {
        if line.starts_with("RRULE") {
            vec!["RRULE:FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=15".to_string()]
        } else {
            vec![line.to_string()]
        }
    });
    engine.reconcile().await.unwrap();
    assert!(read(&dir, "notes/home.md").contains(&format!(
        "- [ ] water the plants {REPEAT} every month on the 15th {ID} {UID}\n"
    )));

    // Removed in the vault: the rule leaves the server too.
    home_note(&dir, &format!("- [ ] water the plants {ID} {UID}\n"));
    engine.reconcile().await.unwrap();
    assert!(!body(&mock, "home", UID).contains("RRULE"));
}

#[tokio::test]
async fn a_rule_set_in_another_client_shows_up_in_the_vault() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] water the plants \u{23EB} {ID} {UID}\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    make_recurring(&mock, "home", UID, "FREQ=DAILY;INTERVAL=2");
    engine.reconcile().await.unwrap();

    assert!(read(&dir, "notes/home.md").contains(&format!(
        "- [ ] water the plants \u{23EB} {REPEAT} every 2 days {ID} {UID}\n"
    )));
    // …and in the TODO.md view of the task.
    assert!(read(&dir, "TODO.md").contains(&format!(
        "- [ ] water the plants \u{23EB} {REPEAT} every 2 days [[home#Home|home]] {ID} {UID}"
    )));
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.markdown_mutations), (0, 0));
}

#[tokio::test]
async fn completing_a_recurring_task_in_the_vault_moves_the_series_on() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!(
            "- [ ] water the plants {REPEAT} every 2 days {DUE} {} {ID} {UID}\n- [ ] other\n",
            day(0)
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    // Another client stores a note on the task.
    edit_remote(&mock, "home", UID, Duration::minutes(5), |line| {
        if line.starts_with("END:VTODO") {
            vec!["DESCRIPTION:two cups".to_string(), line.to_string()]
        } else {
            vec![line.to_string()]
        }
    });
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

    // In the note: the series line is active with the new date and its rule; the
    // occurrence that was done is a plain record under Done with a UID of its own.
    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "- [ ] water the plants {REPEAT} every 2 days {DUE} {} {ID} {UID}\n",
            day(2)
        )),
        "{note}"
    );
    let record = uid_of(&dir, "notes/home.md", "- [x] water the plants");
    assert_ne!(record, UID);
    assert!(
        note.contains(&format!(
            "### Done\n- [x] water the plants {DUE} {} \u{2705} {} {ID} {record}\n",
            day(0),
            today()
        )),
        "{note}"
    );

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
async fn a_rule_the_vault_cannot_spell_stays_on_the_server_and_still_rolls() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] month-end report {DUE} 2026-09-30 {ID} {UID}\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    // "Last working day of the month" has no vault spelling.
    let rrule = "FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR";
    make_recurring(&mock, "home", UID, rrule);
    engine.reconcile().await.unwrap();
    assert!(!read(&dir, "notes/home.md").contains(REPEAT));

    engine.set_done(&uid(UID), true).await.unwrap();
    let series = body(&mock, "home", UID);
    assert!(series.contains("STATUS:NEEDS-ACTION"), "{series}");
    assert!(
        series.contains(&format!("RRULE:{rrule}\r\n")),
        "kept as written"
    );
    assert!(!series.contains("DUE;VALUE=DATE:20260930"), "moved on");
    let note = read(&dir, "notes/home.md");
    assert!(note.contains("- [ ] month-end report") && note.contains("- [x] month-end report"));
}

#[tokio::test]
async fn the_last_occurrence_of_a_counted_rule_completes_the_task() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!(
            "- [ ] two more times {REPEAT} every day for 2 times {DUE} {} {ID} {UID}\n",
            day(0)
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    engine.set_done(&uid(UID), true).await.unwrap();
    let series = body(&mock, "home", UID);
    assert!(series.contains("STATUS:NEEDS-ACTION"));
    assert!(series.contains("RRULE:FREQ=DAILY;COUNT=1\r\n"), "{series}");
    assert!(series.contains(&format!("DUE;VALUE=DATE:{}\r\n", ical_day(1))));
    assert!(read(&dir, "notes/home.md").contains(&format!(
        "- [ ] two more times {REPEAT} every day for 1 time {DUE} {}",
        day(1)
    )));

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
    let rule = restask::domain::Recurrence::from_text("every day")
        .unwrap()
        .0;
    let task = engine
        .add("take vitamins", None, None, Some(rule))
        .await
        .unwrap();
    assert!(body(&mock, "inbox", task.uid.as_str()).contains("RRULE:FREQ=DAILY\r\n"));

    engine.set_done(&task.uid, true).await.unwrap();
    let todo = read(&dir, "TODO.md");
    // No date before: the next occurrence becomes the due date.
    assert!(
        todo.contains(&format!(
            "## No Priority\n- [ ] take vitamins {REPEAT} every day {DUE} {} {ID} {}\n",
            day(1),
            task.uid
        )),
        "{todo}"
    );
    assert!(
        todo.contains("## Done\n- [x] take vitamins \u{2705}"),
        "{todo}"
    );
    let series = body(&mock, "inbox", task.uid.as_str());
    assert!(series.contains("STATUS:NEEDS-ACTION") && series.contains("RRULE:FREQ=DAILY"));
}

#[tokio::test]
async fn a_recurring_line_first_seen_already_checked_rolls_before_its_first_push() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [x] stretch {REPEAT} every week {DUE} {}\n", day(0)),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    let series = uid_of(&dir, "notes/home.md", "- [ ] stretch");
    assert!(
        note.contains(&format!(
            "- [ ] stretch {REPEAT} every week {DUE} {}",
            day(7)
        )),
        "{note}"
    );
    assert!(note.contains("- [x] stretch"));
    let remote = body(&mock, "home", &series);
    assert!(remote.contains("STATUS:NEEDS-ACTION") && remote.contains("RRULE:FREQ=WEEKLY"));
    assert_eq!(mock.resource_names("home").len(), 2);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.inserts), (0, 0));
}

// ── the creation date (➕) ─────────────────────────────────────────────────────────────

const CREATED: &str = "\u{2795}";

/// The day a UID was minted, as the vault and the server write it.
fn born(value: &str) -> (String, String) {
    let day = uid(value).created_on().unwrap().format();
    let ical = format!("CREATED:{}T000000Z", day.replace('-', ""));
    (day, ical)
}

/// §6.4: a line is registered without a creation date. The server still learns when the
/// task was created — the day its UID was minted — and that never comes back as a token.
#[tokio::test]
async fn a_line_gets_no_creation_date_but_the_server_does() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] typed today\n");
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(!note.contains(CREATED), "{note}");
    let typed = uid_of(&dir, "notes/home.md", "typed today");
    assert!(body(&mock, "home", &typed).contains(&born(&typed).1));

    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.normalized), (0, 0));
    assert_eq!(snapshot(&dir), files);
    assert!(!read(&dir, "notes/home.md").contains(CREATED));
}

/// §6.4: a bare `➕` on a new line is answered with today's date, in a note and in
/// TODO.md alike, and that date is the task's `CREATED`.
#[tokio::test]
async fn a_new_line_that_asks_for_its_creation_date_gets_today() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] dated {CREATED} \u{1F53A}\n"));
    let todo = read(&dir, "TODO.md");
    write_vault_file(&dir, "TODO.md", &format!("{todo}- [ ] quick {CREATED}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains(&format!(
            "- [ ] dated \u{1F53A} {CREATED} {} {ID} ",
            today()
        )),
        "{note}"
    );
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!("- [ ] quick {CREATED} {} {ID} ", today())),
        "{todo}"
    );
    let stamp = format!("CREATED:{}T000000Z", today().replace('-', ""));
    let dated = uid_of(&dir, "notes/home.md", "dated");
    assert!(body(&mock, "home", &dated).contains(&stamp));
    assert!(body(&mock, "home", &dated).contains("SUMMARY:dated\r\n"));
    let quick = uid_of(&dir, "TODO.md", "quick");
    assert!(body(&mock, "inbox", &quick).contains(&stamp));
}

/// §6.4: on a task the server knows, a bare `➕` is answered with the server's creation
/// date — also when it was made elsewhere and is not the day in the UID — and deleting
/// the token takes nothing from the server.
#[tokio::test]
async fn an_existing_task_that_asks_gets_the_servers_creation_date() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] old one {ID} {UID}\n"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    // Another client knows better when the task was created.
    let (_, minted) = born(UID);
    edit_remote(&mock, "home", UID, Duration::minutes(5), |line| {
        if line.starts_with("CREATED:") {
            vec!["CREATED:20260901T081233Z".to_string()]
        } else {
            vec![line.to_string()]
        }
    });
    assert!(!body(&mock, "home", UID).contains(&minted));
    engine.reconcile().await.unwrap();
    assert!(!read(&dir, "notes/home.md").contains(CREATED));

    home_note(&dir, &format!("- [ ] old one {CREATED} {ID} {UID}\n"));
    engine.reconcile().await.unwrap();
    let asked = format!("- [ ] old one {CREATED} 2026-09-01 {ID} {UID}\n");
    assert!(read(&dir, "notes/home.md").ends_with(&asked));

    let files = snapshot(&dir);
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.normalized), (0, 0));
    assert_eq!(snapshot(&dir), files);

    // The token goes, the server's date stays.
    home_note(&dir, &format!("- [ ] old one {ID} {UID}\n"));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.pushes, 0);
    assert!(body(&mock, "home", UID).contains("CREATED:20260901T"));
    assert!(!read(&dir, "notes/home.md").contains(CREATED));
}

/// §6.4: the answer needs no server. A task that was never pushed gets the day its UID
/// was minted; the bare token never reaches the server as text.
#[tokio::test]
async fn the_creation_date_is_answered_offline_and_in_todo_md() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] never pushed {CREATED} {ID} {UID}\n"));
    let todo = read(&dir, "TODO.md");
    write_vault_file(
        &dir,
        "TODO.md",
        &format!("{todo}- [ ] quick {CREATED} {ID} {UID2}\n"),
    );
    let offline = engine_with(&dir, Offline, true);
    assert!(offline.reconcile().await.is_err());

    let note = read(&dir, "notes/home.md");
    assert!(
        note.ends_with(&format!(
            "- [ ] never pushed {CREATED} {} {ID} {UID}\n",
            born(UID).0
        )),
        "{note}"
    );
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "- [ ] quick {CREATED} {} {ID} {UID2}\n",
            born(UID2).0
        )),
        "{todo}"
    );

    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    assert!(body(&mock, "home", UID).contains("SUMMARY:never pushed\r\n"));
    assert!(body(&mock, "home", UID).contains(&born(UID).1));
}

// ── tasks registered on another device ────────────────────────────────────────────────

/// The Obsidian plugin registers a new task where it is typed and files it in TODO.md
/// (§15.6). What it writes must be what the engine would have written: the first pass
/// pushes the tasks and rewrites neither file — a rewrite would travel back to the phone
/// and collide with whatever is being typed there.
#[tokio::test]
async fn lines_registered_and_filed_by_the_plugin_are_taken_as_they_are() {
    let dir = temp_vault();
    let note = format!("- [ ] buy milk {ID} {UID}\n");
    home_note(&dir, &note);
    let todo = sealed(&format!(
        "---\nrestask-list: inbox\n---\n# TODO\n\n## \u{1F53A} Highest Priority\n\
         - [ ] call the bank \u{1F53A} {ID} {UID2}\n\n## Done\n"
    ));
    write_vault_file(&dir, "TODO.md", &todo);
    let written = (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md"));
    std::thread::sleep(std::time::Duration::from_millis(20));

    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let report = engine.reconcile().await.unwrap();

    assert_eq!((report.registered, report.pushes, report.failed), (0, 2, 0));
    assert_eq!(mock.resource_names("home"), vec![UID.to_string()]);
    assert_eq!(mock.resource_names("inbox"), vec![UID2.to_string()]);
    assert!(body(&mock, "inbox", UID2).contains("PRIORITY:1"));
    assert!(read(&dir, "notes/home.md").ends_with(&note));
    assert_eq!(read(&dir, "TODO.md"), todo);
    assert_eq!(
        (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md")),
        written,
        "neither file was rewritten"
    );

    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.registered), (0, 0));
}

/// The plugin completes a task where the box is checked (§15.6): in the note the line is
/// stamped and moved under the done heading, and its mirror line leaves TODO.md. The
/// pass that sees both files completes the task on the server and rewrites neither.
#[tokio::test]
async fn a_completion_made_by_the_plugin_is_taken_as_it_is() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n- [ ] other\n\n## Done\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let mirror = format!(
        "## \u{1F53A} Highest Priority\n- [ ] call the bank \u{1F53A} [[home#Home|home]] {ID} {UID}\n\n"
    );
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains(&mirror), "{todo}");

    // What the plugin writes on the phone.
    let note = read(&dir, "notes/home.md")
        .replace(&format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n"), "")
        .replace(
            "## Done\n",
            &format!(
                "## Done\n- [x] call the bank \u{1F53A} \u{2705} {} {ID} {UID}\n",
                today()
            ),
        );
    write_vault_file(&dir, "notes/home.md", &note);
    write_vault_file(&dir, "TODO.md", &resealed(&todo.replace(&mirror, "")));
    let written = (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md"));
    std::thread::sleep(std::time::Duration::from_millis(20));

    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.pushes, report.failed), (1, 0));
    assert!(body(&mock, "home", UID).contains("STATUS:COMPLETED"));
    assert_eq!(read(&dir, "notes/home.md"), note);
    assert_eq!(
        (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md")),
        written,
        "neither file was rewritten"
    );
}

/// The file sync may deliver the plugin's two files one after the other. TODO.md alone —
/// a mirror line gone, or a mirror line of a task no note has yet — must not make the
/// engine touch a note, complete or create anything: a note it rewrote here would meet
/// the phone's version of the same note as a sync conflict.
#[tokio::test]
async fn the_plugins_todo_arriving_before_its_note_changes_no_note_and_no_task() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n\n## Done\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    let note_written = mtime(&dir, "notes/home.md");
    let (puts, deletes, _) = mock.counters();
    std::thread::sleep(std::time::Duration::from_millis(20));

    // The mirror line of the task completed on the phone is gone; the mirror line of a
    // task typed on the phone is there. Both notes are still on their way.
    let ahead = todo.replace(
        &format!("- [ ] call the bank \u{1F53A} [[home#Home|home]] {ID} {UID}\n"),
        &format!("- [ ] typed on the phone \u{1F53A} [[home#Home|home]] {ID} {UID2}\n"),
    );
    assert_ne!(ahead, todo);
    // The plugin seals what it leaves (§7.2); unsealed, the missing line would be the
    // user's deletion (§7.1).
    write_vault_file(&dir, "TODO.md", &resealed(&ahead));
    engine.reconcile().await.unwrap();

    assert_eq!(mtime(&dir, "notes/home.md"), note_written);
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
    assert!(mock.resource_names("inbox").is_empty());
    assert!(body(&mock, "home", UID).contains("STATUS:NEEDS-ACTION"));
    // The view is the engine's: it shows the vault as the engine knows it.
    assert_eq!(read(&dir, "TODO.md"), todo);
}

/// A device that settles tasks itself seals the TODO.md it leaves (§7.2). When the note
/// behind a rewritten mirror line is still in transit, the engine must not read that
/// line as an edit and write the note itself: its version and the phone's would meet as
/// a sync conflict. The same line changed by hand — the seal broken — is an edit.
#[tokio::test]
async fn a_sealed_view_ahead_of_its_note_is_not_carried_into_the_note() {
    let dir = temp_vault();
    home_note(
        &dir,
        &format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n\n## Done\n"),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    let note_written = mtime(&dir, "notes/home.md");
    std::thread::sleep(std::time::Duration::from_millis(20));

    let reprioritized = todo
        .replace("## \u{1F53A} Highest Priority", "## \u{1F53D} Low Priority")
        .replace("the bank \u{1F53A}", "the bank \u{1F53D}");
    assert_ne!(reprioritized, todo);

    write_vault_file(&dir, "TODO.md", &resealed(&reprioritized));
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.normalized, report.pushes), (0, 0));
    assert_eq!(mtime(&dir, "notes/home.md"), note_written);
    assert!(body(&mock, "home", UID).contains("PRIORITY:1"));

    write_vault_file(&dir, "TODO.md", &reprioritized);
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.pushes, 1);
    assert!(read(&dir, "notes/home.md").contains("call the bank \u{1F53D}"));
    assert!(body(&mock, "home", UID).contains("PRIORITY:7"));
}

// ── nothing in the body but the view (§7.3) ───────────────────────────────────────────

const OLD_DISCLAIMER: &str = "<!-- AUTOGENERATED BY Restask -->";

/// A TODO.md that is not there is created: frontmatter with the seal, title, sections —
/// and no comment line.
#[tokio::test]
async fn a_view_created_by_the_engine_has_no_comment_line() {
    let dir = temp_vault();
    std::fs::remove_file(dir.path().join("TODO.md")).unwrap();
    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        read(&dir, "TODO.md"),
        sealed("---\nrestask-list: inbox\n---\n# TODO\n\n## Done\n")
    );
}

/// A view rendered before the seal moved into the frontmatter — disclaimer and seal as
/// comment lines in the body — loses both at the next render and keeps its tasks; the
/// pass after that is a no-op.
#[tokio::test]
async fn the_comment_lines_of_an_earlier_view_are_dropped_by_the_next_render() {
    let dir = temp_vault();
    home_note(&dir, &format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n"));
    write_vault_file(
        &dir,
        "TODO.md",
        &format!(
            "---\nrestask-list: inbox\n---\n{OLD_DISCLAIMER}\n<!-- restask-render: 0ed09b30e036d3f7 -->\n\n# TODO\n\n\
             ## No Priority\n- [ ] buy milk {ID} {UID2}\n\n## Done\n"
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let view = read(&dir, "TODO.md");
    assert!(!view.contains("<!--"));
    assert!(view.starts_with("---\nrestask-list: inbox\nrestask-render: "));
    assert!(restask::markdown::is_sealed(&view));
    assert!(view.contains("call the bank") && view.contains("buy milk"));

    let written = mtime(&dir, "TODO.md");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let again = engine.reconcile().await.unwrap();
    assert_eq!(
        (again.pushes, again.registered, again.normalized),
        (0, 0, 0)
    );
    assert_eq!(
        mtime(&dir, "TODO.md"),
        written,
        "a converged view is not rewritten"
    );
}

/// The title follows the frontmatter directly (§7). A view rendered when a blank line
/// stood between the two — sealed, as the daemon or the plugin left it — loses that line
/// at the next render and nothing else: the tasks stay, in the view and in the note, no
/// request is sent for it, and the pass after that is a no-op.
#[tokio::test]
async fn the_blank_line_above_the_title_of_an_earlier_view_is_dropped_by_the_next_render() {
    let dir = temp_vault();
    let line = format!("- [ ] call the bank \u{1F53A} {ID} {UID}\n");
    home_note(&dir, &line);
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let note = read(&dir, "notes/home.md");
    let rendered = read(&dir, "TODO.md");
    assert!(rendered.contains("\n---\n# TODO\n\n## "));

    // The same view in the earlier layout, sealed by whoever rendered it.
    let (head, body) = rendered.split_once("---\n# TODO\n").unwrap();
    let list = head.lines().nth(1).unwrap();
    let earlier = sealed(&format!("---\n{list}\n---\n\n# TODO\n{body}"));
    assert_ne!(earlier, rendered);
    write_vault_file(&dir, "TODO.md", &earlier);

    let pass = engine.reconcile().await.unwrap();
    assert_eq!((pass.pushes, pass.registered, pass.normalized), (0, 0, 0));
    assert_eq!(read(&dir, "TODO.md"), rendered);
    assert_eq!(read(&dir, "notes/home.md"), note);

    let written = mtime(&dir, "TODO.md");
    std::thread::sleep(std::time::Duration::from_millis(20));
    engine.reconcile().await.unwrap();
    assert_eq!(
        mtime(&dir, "TODO.md"),
        written,
        "a converged view is not rewritten"
    );
}

/// The view is the file at the inbox path, and nothing else (§7): a routed note that
/// carries a line of the view — the old disclaimer, pasted or left over — is a note. Its
/// tasks are registered where they are and mirrored into TODO.md, and the line is kept.
#[tokio::test]
async fn a_note_with_a_line_of_the_view_in_it_is_still_a_note() {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "notes/home.md",
        &format!(
            "---\nrestask-list: home\n---\n{OLD_DISCLAIMER}\n# Home\n\n- [ ] buy a camera \u{1F53A}\n"
        ),
    );
    let mock = MockCaldav::new();
    let report = engine(&dir, &mock).reconcile().await.unwrap();

    assert_eq!(report.registered, 1);
    let note = read(&dir, "notes/home.md");
    assert!(note.contains(OLD_DISCLAIMER), "a note's own text is kept");
    assert!(note.contains(&format!("- [ ] buy a camera \u{1F53A} {ID} restask-")));
    assert!(
        !note.contains("## "),
        "no section of the view is made in a note"
    );
    assert!(read(&dir, "TODO.md").contains("buy a camera \u{1F53A} [[home#Home|home]]"));
}

/// Invariant 12 for an editor without a port of the rules (Neovim): `settle` is the local
/// phase and the render, with no server and no sync state. What it leaves is what the
/// daemon's pass would leave — so that pass, when the files reach it, writes no vault
/// file — and an edit made in the view is carried to its note on the spot.
#[tokio::test]
async fn settling_does_the_local_work_without_a_server_and_the_pass_writes_no_vault_file() {
    let dir = temp_vault();
    home_note(
        &dir,
        "- [ ] typed in an editor \u{1F53A}\n- [x] ticked by hand\n",
    );
    let offline = engine_with(&dir, Offline, true);
    let report = offline.settle().await.unwrap();
    assert_eq!(report.registered, 2);

    // Registered, completed and mirrored; the view is sealed like any render.
    let note = read(&dir, "notes/home.md");
    assert_eq!(note.matches(ID).count(), 2, "{note}");
    assert!(note.contains("Done\n- [x] ticked by hand"), "{note}");
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains("- [ ] typed in an editor \u{1F53A} [[home#Home|home]]"),
        "{todo}"
    );
    assert_eq!(todo, resealed(&todo));
    assert!(
        !dir.path().join(".restask/index.json").exists(),
        "nothing was agreed with a server, so no state says so"
    );

    // A second run is quiet.
    let files = snapshot(&dir);
    let again = offline.settle().await.unwrap();
    assert_eq!((again.registered, again.normalized), (0, 0));
    assert_eq!(snapshot(&dir), files);

    // The daemon's pass pushes the two tasks and finds the vault as it would have left it.
    let (note_at, todo_at) = (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md"));
    let mock = MockCaldav::new();
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        (report.registered, report.normalized, report.pushes),
        (0, 0, 2)
    );
    assert_eq!(mtime(&dir, "notes/home.md"), note_at);
    assert_eq!(mtime(&dir, "TODO.md"), todo_at);

    // An edit on the mirror line is carried to the note, the view filed again — and the
    // server has not been asked.
    let requests = mock.counters();
    write_vault_file(
        &dir,
        "TODO.md",
        &read(&dir, "TODO.md").replace("editor \u{1F53A}", "editor \u{23EB}"),
    );
    engine(&dir, &mock).settle().await.unwrap();
    assert!(read(&dir, "notes/home.md").contains("- [ ] typed in an editor \u{23EB} "));
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains("High Priority\n- [ ] typed in an editor \u{23EB} [[home#Home|home]]"),
        "{todo}"
    );
    assert_eq!(todo, resealed(&todo));
    assert_eq!(mock.counters(), requests);
}

/// A machine that leaves the syncing to the vault's sync node (`[node]` in its machine
/// config, §14.2) never passes: `restask add` and `done` change the vault, settle it, and
/// ask the server nothing — also when a server could be reached. A pass from a second
/// machine pushes what the file sync is still carrying to the node, and the node then
/// pulls it into its older copy of the note (§1.1).
#[tokio::test]
async fn a_machine_that_is_not_the_sync_node_settles_and_sends_nothing() {
    let dir = temp_vault();
    home_note(&dir, "- [ ] typed in an editor\n");
    let mock = MockCaldav::new();
    let cfg = VaultConfig::load(&dir.path().join("restask.toml")).unwrap();
    let editing = MachineConfig {
        node: Some(restask::config::NodeSection::default()),
        ..MachineConfig::default()
    };
    let engine_here = Engine::new(dir.path(), cfg, editing, mock.clone(), clock());

    let task = engine_here
        .add("from the command line", None, None, None)
        .await
        .unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("- [ ] from the command line"), "{todo}");
    assert_eq!(todo, resealed(&todo));
    // The local work of the whole vault was done with it.
    let registered = uid_of(&dir, "notes/home.md", "typed in an editor");

    engine_here.set_done(&uid(&registered), true).await.unwrap();
    let note = read(&dir, "notes/home.md");
    assert!(note.contains("Done\n- [x] typed in an editor"), "{note}");

    assert_eq!(mock.counters(), (0, 0, 0), "no request");
    assert!(mock.collection_names().is_empty());
    assert!(
        !dir.path().join(".restask/index.json").exists(),
        "nothing was agreed with the server from here"
    );

    // The sync node's pass over the same files pushes both tasks and writes no vault file.
    let (note_at, todo_at) = (mtime(&dir, "notes/home.md"), mtime(&dir, "TODO.md"));
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        (report.registered, report.normalized, report.pushes),
        (0, 0, 2)
    );
    assert_eq!(mock.resource_names("inbox").len(), 1);
    assert_eq!(mock.resource_names("home").len(), 1);
    assert!(read(&dir, "TODO.md").contains(task.uid.as_str()));
    assert_eq!(mtime(&dir, "notes/home.md"), note_at);
    assert_eq!(mtime(&dir, "TODO.md"), todo_at);
}

// ---- §7.5: TODO.md holds the tasks of several calendars ----

const CALENDAR: &str = "\u{1F4C1}";

/// A vault whose TODO.md also shows the calendars `lists` (a TOML array body), bound to
/// `inbox` like every test vault.
fn vault_showing(lists: &str) -> TempDir {
    let dir = temp_vault();
    write_vault_file(
        &dir,
        "restask.toml",
        &format!("done_heading = \"Done\"\ntodo_lists = [{lists}]\n"),
    );
    dir
}

/// `Update restask README`, made in Tasks.org, without a priority.
fn work_task() -> String {
    TASKS_ORG_BODY
        .replace("Made in Tasks.org", "Update restask README")
        .replace("PRIORITY:1\r\n", "")
}

#[tokio::test]
async fn a_task_of_another_calendar_lives_in_todo_md() {
    let dir = vault_showing("\"work\"");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("work", TASKS_ORG_NAME, &work_task());
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        (report.adoptions, report.inserts, report.deletes),
        (1, 1, 0)
    );
    let adopted = uid_of(&dir, "TODO.md", "Update restask README");
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## No Priority\n- [ ] Update restask README {CALENDAR} work {ID} {adopted}\n"
        )),
        "{todo}"
    );
    assert_eq!(todo, resealed(&todo));
    // It stays the task its client made, in the calendar it was made in.
    assert_eq!(mock.resource_names("work"), vec![TASKS_ORG_NAME]);
    assert!(mock.resource_names("inbox").is_empty());
    let linked = body(&mock, "work", TASKS_ORG_NAME);
    assert!(linked.contains(&format!("X-RESTASK-UID:{adopted}\r\n")));
    assert!(linked.contains("DESCRIPTION:with a note\r\n"), "{linked}");

    // Quiet.
    let (counters, files) = (mock.counters(), snapshot(&dir));
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.moves, again.inserts), (0, 0, 0));
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(snapshot(&dir), files);

    // A priority given on the server moves the line to that section, calendar kept.
    mock.seed_resource(
        "work",
        TASKS_ORG_NAME,
        &linked.replace("SUMMARY:", "PRIORITY:1\r\nSUMMARY:"),
    );
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] Update restask README \u{1F53A} {CALENDAR} work {ID} {adopted}\n"
        )),
        "{todo}"
    );

    // Checked in TODO.md, it is completed in its own calendar.
    write_vault_file(
        &dir,
        "TODO.md",
        &todo.replace("- [ ] Update restask README", "- [x] Update restask README"),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.pushes, report.moves, report.deletes), (1, 0, 0));
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## Done\n- [x] Update restask README \u{1F53A} \u{2705} {} {CALENDAR} work {ID} {adopted}\n",
            today()
        )),
        "{todo}"
    );
    assert!(body(&mock, "work", TASKS_ORG_NAME).contains("STATUS:COMPLETED\r\n"));
    assert_eq!(mock.resource_names("work"), vec![TASKS_ORG_NAME]);
    assert!(mock.resource_names("inbox").is_empty());
}

#[tokio::test]
async fn a_line_of_todo_md_lives_in_the_calendar_it_names() {
    let dir = vault_showing("\"work\"");
    write_vault_file(
        &dir,
        "TODO.md",
        &format!(
            "---\nrestask-list: inbox\n---\n# TODO\n\n## No Priority\n- [ ] for work {CALENDAR} Work\n- [ ] for me\n\n## Done\n"
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.registered, report.pushes), (2, 2));
    let (work, mine) = (
        uid_of(&dir, "TODO.md", "for work"),
        uid_of(&dir, "TODO.md", "for me"),
    );
    assert_eq!(mock.resource_names("work"), vec![work.clone()]);
    assert_eq!(mock.resource_names("inbox"), vec![mine.clone()]);
    let todo = read(&dir, "TODO.md");
    // The calendar is written as its slug; the default one is not written at all.
    assert!(
        todo.contains(&format!(
            "## No Priority\n- [ ] for work {CALENDAR} work {ID} {work}\n- [ ] for me {ID} {mine}\n"
        )),
        "{todo}"
    );
    assert!(body(&mock, "work", &work).contains("SUMMARY:for work\r\n"));

    // Another name moves the task — to a calendar TODO.md was not told to show, too.
    write_vault_file(
        &dir,
        "TODO.md",
        &todo.replace(&format!("{CALENDAR} work"), &format!("{CALENDAR} family")),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.moves, report.registered), (1, 0));
    assert!(mock.resource_names("work").is_empty());
    assert_eq!(mock.resource_names("family"), vec![work.clone()]);
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!("- [ ] for work {CALENDAR} family {ID} {work}\n")),
        "{todo}"
    );

    // No name: the calendar TODO.md is bound to. Naming that one is the same.
    write_vault_file(
        &dir,
        "TODO.md",
        &todo
            .replace(&format!(" {CALENDAR} family"), "")
            .replace("for me", &format!("for me {CALENDAR} inbox")),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.moves, 1);
    assert!(mock.resource_names("family").is_empty());
    let mut both = vec![work.clone(), mine.clone()];
    both.sort();
    assert_eq!(mock.resource_names("inbox"), both);
    let todo = read(&dir, "TODO.md");
    assert!(!todo.contains(CALENDAR), "{todo}");

    let (counters, files) = (mock.counters(), snapshot(&dir));
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.moves), (0, 0));
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(snapshot(&dir), files);
}

/// The line itself says where its task lives: with the sync state gone (`restask
/// rebuild`), nothing is moved to the calendar TODO.md is bound to.
#[tokio::test]
async fn a_task_of_another_calendar_stays_there_when_the_state_is_dropped() {
    let dir = vault_showing("\"work\"");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("work", TASKS_ORG_NAME, &work_task());
    engine(&dir, &mock).reconcile().await.unwrap();
    engine(&dir, &mock)
        .add("mine", None, None, None)
        .await
        .unwrap();
    let todo = read(&dir, "TODO.md");
    let stored = body(&mock, "work", TASKS_ORG_NAME);

    std::fs::remove_dir_all(dir.path().join(".restask")).unwrap();
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        (
            report.moves,
            report.deletes,
            report.inserts,
            report.adoptions
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(read(&dir, "TODO.md"), todo);
    assert_eq!(mock.resource_names("work"), vec![TASKS_ORG_NAME]);
    assert_eq!(mock.resource_names("inbox").len(), 1);
    assert_eq!(body(&mock, "work", TASKS_ORG_NAME), stored);
}

#[tokio::test]
async fn a_calendar_todo_md_was_not_told_to_show_is_not_looked_at() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("work", TASKS_ORG_NAME, &work_task());
    let before = mock.resource("work", TASKS_ORG_NAME).unwrap().etag;

    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!((report.adoptions, report.inserts, report.pushes), (0, 0, 0));
    assert_eq!(mock.counters(), (0, 0, 1), "one listing: the inbox");
    assert_eq!(mock.resource("work", TASKS_ORG_NAME).unwrap().etag, before);
    assert!(!read(&dir, "TODO.md").contains("Update restask README"));
}

#[tokio::test]
async fn a_calendar_that_is_no_longer_shown_or_gone_takes_no_line_with_it() {
    let dir = vault_showing("\"work\"");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("work", TASKS_ORG_NAME, &work_task());
    engine(&dir, &mock).reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(todo.contains("Update restask README"), "{todo}");

    // Taken out of `todo_lists`: the line still names its calendar and still syncs
    // with it; only new tasks of that calendar stop coming in.
    write_vault_file(&dir, "restask.toml", "done_heading = \"Done\"\n");
    mock.seed_resource(
        "work",
        "second",
        &work_task()
            .replace("UID:5417861935824551742", "UID:second@tasks.org")
            .replace("Update restask README", "Second one"),
    );
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        (report.deletes, report.markdown_mutations, report.adoptions),
        (0, 0, 0)
    );
    assert_eq!(read(&dir, "TODO.md"), todo);
    assert_eq!(mock.resource_names("work").len(), 2);

    // A server that has no such calendar (and may not be given one) proves nothing.
    let elsewhere = MockCaldav::new();
    elsewhere.seed_collection("inbox", "Inbox");
    let report = engine_with(&dir, elsewhere.clone(), false)
        .reconcile()
        .await
        .unwrap();
    assert_eq!((report.deletes, report.markdown_mutations), (0, 0));
    assert_eq!(read(&dir, "TODO.md"), todo);
    // One that lost the calendar wholesale gets the task again, the line stays.
    let report = engine(&dir, &elsewhere).reconcile().await.unwrap();
    assert_eq!(report.markdown_mutations, 0);
    assert_eq!(read(&dir, "TODO.md"), todo);
    assert_eq!(elsewhere.resource_names("work").len(), 1);
}

#[tokio::test]
async fn a_calendar_with_a_note_of_its_own_keeps_its_tasks_in_that_note() {
    let dir = vault_showing("\"home\", \"work\"");
    home_note(&dir, "");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("home", TASKS_ORG_NAME, &work_task());
    engine(&dir, &mock).reconcile().await.unwrap();

    let note = read(&dir, "notes/home.md");
    assert!(
        note.contains("- [ ] Update restask README \u{1F194}"),
        "{note}"
    );
    assert!(!note.contains(CALENDAR), "{note}");
    // Without a priority it is not mirrored, as before.
    assert!(!read(&dir, "TODO.md").contains("Update restask README"));
}

#[tokio::test]
async fn the_calendar_of_a_line_is_settled_on_the_device_and_the_pass_writes_no_file() {
    let dir = vault_showing("\"work\"");
    write_vault_file(
        &dir,
        "TODO.md",
        &format!(
            "---\nrestask-list: inbox\n---\n# TODO\n\n## \u{1F53A} Highest Priority\n- [ ] for work {CALENDAR} work\n\n## Done\n"
        ),
    );
    let offline = engine_with(&dir, Offline, true);
    assert_eq!(offline.settle().await.unwrap().registered, 1);
    let registered = uid_of(&dir, "TODO.md", "for work");
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## \u{1F53A} Highest Priority\n- [ ] for work \u{1F53A} {CALENDAR} work {ID} {registered}\n"
        )),
        "{todo}"
    );
    assert_eq!(todo, resealed(&todo));

    let at = mtime(&dir, "TODO.md");
    let mock = MockCaldav::new();
    let report = engine(&dir, &mock).reconcile().await.unwrap();
    assert_eq!(
        (report.registered, report.normalized, report.pushes),
        (0, 0, 1)
    );
    assert_eq!(mock.resource_names("work"), vec![registered]);
    assert_eq!(mtime(&dir, "TODO.md"), at);
}

#[tokio::test]
async fn a_recurring_task_of_another_calendar_rolls_forward_in_that_calendar() {
    let dir = vault_showing("\"work\"");
    write_vault_file(
        &dir,
        "TODO.md",
        &format!(
            "---\nrestask-list: inbox\n---\n# TODO\n\n## No Priority\n- [ ] stand-up {REPEAT} every day {CALENDAR} work\n\n## Done\n"
        ),
    );
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let series = uid_of(&dir, "TODO.md", "stand-up");

    engine.set_done(&uid(&series), true).await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "## No Priority\n- [ ] stand-up {REPEAT} every day {DUE} {} {CALENDAR} work {ID} {series}\n",
            day(1)
        )),
        "{todo}"
    );
    assert!(todo.contains("## Done\n- [x] stand-up \u{2705}"), "{todo}");
    assert_eq!(
        todo.matches(&format!("{CALENDAR} work")).count(),
        2,
        "{todo}"
    );
    assert_eq!(mock.resource_names("work").len(), 2);
    assert!(mock.resource_names("inbox").is_empty());
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.moves, again.inserts), (0, 0, 0));
}

// ── wikilinks on the server: the title, and a link into Obsidian (§8.4) ───────────────

/// A vault whose `restask.toml` names its Obsidian vault, with a routed note of links.
fn youtube_vault(obsidian: Option<&str>) -> TempDir {
    let dir = temp_vault();
    let name = obsidian
        .map(|name| format!("obsidian_vault = \"{name}\"\n"))
        .unwrap_or_default();
    write_vault_file(
        &dir,
        "restask.toml",
        &format!("done_heading = \"Done\"\n{name}"),
    );
    note(
        &dir,
        "Projects/YouTube.md",
        "YouTube",
        "- [ ] [[Dual HHD 3d printed caddy]]\n- [ ] edit [[Asahi Linux#Install|Asahi]] video\n- [ ] plain\n",
    );
    dir
}

/// A resource's logical lines (folded continuations joined).
fn logical_lines(mock: &MockCaldav, list: &str, name: &str) -> Vec<String> {
    body(mock, list, name)
        .replace("\r\n ", "")
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Rewrites a resource's logical lines the way another client would: `edit` maps each
/// line to its replacements; `LAST-MODIFIED` is stamped a minute after now.
fn edit_logical(mock: &MockCaldav, list: &str, name: &str, edit: impl Fn(&str) -> Vec<String>) {
    let stamp = (Utc::now() + Duration::minutes(1))
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    let mut out = String::new();
    for line in logical_lines(mock, list, name) {
        let replaced = if line.starts_with("LAST-MODIFIED") {
            vec![format!("LAST-MODIFIED:{stamp}")]
        } else {
            edit(&line)
        };
        for line in replaced {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    mock.seed_resource(list, name, &out);
}

/// Two more passes write nothing anywhere.
async fn assert_quiet(dir: &TempDir, engine: &Engine<MockCaldav>, mock: &MockCaldav) {
    let files = snapshot(dir);
    let (puts, deletes, _) = mock.counters();
    std::thread::sleep(std::time::Duration::from_millis(20));
    for _ in 0..2 {
        engine.reconcile().await.unwrap();
    }
    assert_eq!((mock.counters().0, mock.counters().1), (puts, deletes));
    assert_eq!(snapshot(dir), files, "an idempotent pass touches no file");
}

const CADDY: &str = "[Dual HHD 3d printed caddy](obsidian://open?vault=Obsidian&file=Dual%20HHD%203d%20printed%20caddy)";
const ASAHI: &str = "[Asahi](obsidian://open?vault=Obsidian&file=Asahi%20Linux)";

fn retitle(mock: &MockCaldav, name: &str, title: String) {
    edit_logical(mock, "youtube", name, |line| {
        if line.starts_with("SUMMARY") {
            vec![format!("SUMMARY:{title}")]
        } else {
            vec![line.to_string()]
        }
    });
}

#[tokio::test]
async fn a_wikilink_reaches_other_clients_as_a_link_into_obsidian() {
    let dir = youtube_vault(Some("Obsidian"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();

    let note = read(&dir, "Projects/YouTube.md");
    assert!(
        note.contains(&format!("- [ ] [[Dual HHD 3d printed caddy]] {ID} ")),
        "{note}"
    );
    let caddy = uid_of(&dir, "Projects/YouTube.md", "caddy");
    let lines = logical_lines(&mock, "youtube", &caddy);
    assert!(lines.contains(&format!("SUMMARY:{CADDY}")), "{lines:?}");
    assert!(lines.contains(&"X-RESTASK-TEXT;VALUE=TEXT:[[Dual HHD 3d printed caddy]]".to_string()));
    assert!(!lines.iter().any(|line| line.starts_with("DESCRIPTION")));

    let video = uid_of(&dir, "Projects/YouTube.md", "video");
    let lines = logical_lines(&mock, "youtube", &video);
    assert!(
        lines.contains(&format!("SUMMARY:edit {ASAHI} video")),
        "{lines:?}"
    );

    let plain = body(
        &mock,
        "youtube",
        &uid_of(&dir, "Projects/YouTube.md", "plain"),
    );
    assert!(plain.contains("SUMMARY:plain\r\n"));
    assert!(!plain.contains("X-RESTASK-TEXT"));

    assert_quiet(&dir, &engine, &mock).await;
}

#[tokio::test]
async fn a_title_changed_in_another_client_keeps_the_links_it_still_shows() {
    let dir = youtube_vault(Some("Obsidian"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let video = uid_of(&dir, "Projects/YouTube.md", "video");

    retitle(&mock, &video, format!("edit {ASAHI} video tonight"));
    engine.reconcile().await.unwrap();
    let note = read(&dir, "Projects/YouTube.md");
    assert!(
        note.contains(&format!(
            "- [ ] edit [[Asahi Linux#Install|Asahi]] video tonight {ID} {video}"
        )),
        "{note}"
    );
    assert_quiet(&dir, &engine, &mock).await;

    // The link was deleted from the title: it is gone from the note too.
    retitle(&mock, &video, "edit the video tonight".to_string());
    engine.reconcile().await.unwrap();
    let note = read(&dir, "Projects/YouTube.md");
    assert!(
        note.contains(&format!("- [ ] edit the video tonight {ID} {video}")),
        "{note}"
    );
    let lines = logical_lines(&mock, "youtube", &video);
    assert!(
        lines.contains(&"SUMMARY:edit the video tonight".to_string()),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|line| line.starts_with("X-RESTASK-TEXT")));
    assert_quiet(&dir, &engine, &mock).await;
}

#[tokio::test]
async fn a_client_that_drops_the_vault_text_does_not_take_the_links_out_of_the_note() {
    let dir = youtube_vault(Some("Obsidian"));
    let mock = MockCaldav::new();
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    let caddy = uid_of(&dir, "Projects/YouTube.md", "caddy");
    let before = read(&dir, "Projects/YouTube.md");

    // A client that keeps only what it knows: the title, and the notes typed there.
    edit_logical(&mock, "youtube", &caddy, |line| {
        if line.starts_with("X-RESTASK-TEXT") {
            vec!["DESCRIPTION:print in PETG".to_string()]
        } else {
            vec![line.to_string()]
        }
    });
    engine.reconcile().await.unwrap();
    assert_eq!(read(&dir, "Projects/YouTube.md"), before);
    let lines = logical_lines(&mock, "youtube", &caddy);
    assert!(lines.contains(&format!("SUMMARY:{CADDY}")), "{lines:?}");
    assert!(lines.contains(&"X-RESTASK-TEXT;VALUE=TEXT:[[Dual HHD 3d printed caddy]]".to_string()));
    assert!(
        lines.contains(&"DESCRIPTION:print in PETG".to_string()),
        "{lines:?}"
    );
    assert_quiet(&dir, &engine, &mock).await;
}

#[tokio::test]
async fn a_task_pushed_before_links_were_written_is_written_again_once() {
    let dir = youtube_vault(None);
    let mock = MockCaldav::new();
    engine(&dir, &mock).reconcile().await.unwrap();
    let caddy = uid_of(&dir, "Projects/YouTube.md", "caddy");
    let before = read(&dir, "Projects/YouTube.md");
    let lines = logical_lines(&mock, "youtube", &caddy);
    assert!(
        lines.contains(&"SUMMARY:Dual HHD 3d printed caddy".to_string()),
        "{lines:?}"
    );

    // What an earlier restask wrote: the line's text as the title, nothing else.
    edit_logical(&mock, "youtube", &caddy, |line| {
        if line.starts_with("SUMMARY") {
            vec!["SUMMARY:[[Dual HHD 3d printed caddy]]".to_string()]
        } else if line.starts_with("X-RESTASK-TEXT") {
            Vec::new()
        } else {
            vec![line.to_string()]
        }
    });
    // The vault names its Obsidian vault now.
    write_vault_file(
        &dir,
        "restask.toml",
        "done_heading = \"Done\"\nobsidian_vault = \"Obsidian\"\n",
    );
    let engine = engine(&dir, &mock);
    let (puts, _, _) = mock.counters();
    engine.reconcile().await.unwrap();
    assert_eq!(read(&dir, "Projects/YouTube.md"), before);
    assert_eq!(mock.counters().0, puts + 2, "each task with a link, once");
    let lines = logical_lines(&mock, "youtube", &caddy);
    assert!(lines.contains(&format!("SUMMARY:{CADDY}")), "{lines:?}");
    assert_quiet(&dir, &engine, &mock).await;
}

// ---- §5.4: a list is the calendar of its name, wherever another client put it ----

/// The path a phone gave the calendar it made.
const PHONE_PATH: &str = "56de6126-33a4-46fd-a66e-3cc49ad32fe5";

/// The owner's sandbox: the calendar at `/homelab/` is shown as "Home Lab". The notes
/// are routed to `homelab`, its path; the wizard listed it under its name, so
/// `todo_lists` says `home-lab`. Both names found the one calendar, and every pass
/// pushed the ten tasks as list `homelab` and deleted them as strays of list `home-lab`
/// — for as long as the daemon ran (`collection_reset` every 11 s in its log).
#[tokio::test]
async fn two_names_of_one_calendar_do_not_push_and_delete_its_tasks_in_turns() {
    let dir = vault_showing("\"home-lab\"");
    note(
        &dir,
        "Homelab/Networking.md",
        "homelab",
        &format!(
            "- [ ] Update the firewall rules {ID} {UID}\n- [ ] Check the DNS records {ID} {UID2}\n"
        ),
    );
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_collection("homelab", "Home Lab");
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.pushes, report.deletes, report.failed), (2, 0, 0));
    assert_eq!(mock.resource_names("homelab").len(), 2);
    assert_eq!(mock.collection_names(), vec!["homelab", "inbox"]);

    // The next passes leave the calendar, the notes and the view alone.
    for _ in 0..3 {
        let (counters, files) = (mock.counters(), snapshot(&dir));
        let again = engine.reconcile().await.unwrap();
        assert_eq!((again.pushes, again.deletes, again.inserts), (0, 0, 0));
        assert_eq!(
            (mock.counters().0, mock.counters().1),
            (counters.0, counters.1)
        );
        assert_eq!(snapshot(&dir), files);
        assert_eq!(mock.resource_names("homelab").len(), 2);
    }
    let note = read(&dir, "Homelab/Networking.md");
    assert!(note.contains(UID) && note.contains(UID2), "{note}");
}

#[tokio::test]
async fn a_calendar_made_in_another_client_is_found_by_its_name() {
    let dir = vault_showing("\"prova\"");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_collection(PHONE_PATH, "Prova");
    mock.seed_resource(PHONE_PATH, TASKS_ORG_NAME, &work_task());
    let engine = engine(&dir, &mock);

    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        (report.adoptions, report.inserts, report.deletes),
        (1, 1, 0)
    );
    let adopted = uid_of(&dir, "TODO.md", "Update restask README");
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "- [ ] Update restask README {CALENDAR} prova {ID} {adopted}\n"
        )),
        "{todo}"
    );
    // No empty twin at the list's own path, and the task is where its client made it.
    assert_eq!(mock.collection_names(), vec![PHONE_PATH, "inbox"]);
    assert_eq!(mock.resource_names(PHONE_PATH), vec![TASKS_ORG_NAME]);
    assert!(body(&mock, PHONE_PATH, TASKS_ORG_NAME).contains(&format!("X-RESTASK-UID:{adopted}")));

    // Quiet.
    let (counters, files) = (mock.counters(), snapshot(&dir));
    let again = engine.reconcile().await.unwrap();
    assert_eq!((again.pushes, again.moves, again.inserts), (0, 0, 0));
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(snapshot(&dir), files);

    // Checked in the vault, and a task typed for that calendar: both go to it.
    write_vault_file(
        &dir,
        "TODO.md",
        &todo
            .replace("- [ ] Update restask README", "- [x] Update restask README")
            .replace(
                "## No Priority\n",
                &format!("## No Priority\n- [ ] Typed here {CALENDAR} prova\n"),
            ),
    );
    let report = engine.reconcile().await.unwrap();
    assert_eq!((report.failed, report.deletes), (0, 0));
    assert!(body(&mock, PHONE_PATH, TASKS_ORG_NAME).contains("STATUS:COMPLETED"));
    assert_eq!(mock.resource_names(PHONE_PATH).len(), 2);
    assert_eq!(mock.collection_names(), vec![PHONE_PATH, "inbox"]);
    assert!(mock.resource_names("inbox").is_empty());

    // Deleted in the vault, it leaves the calendar it was in.
    let todo = read(&dir, "TODO.md");
    let typed = todo
        .lines()
        .find(|line| line.contains("Typed here"))
        .unwrap()
        .to_string();
    write_vault_file(&dir, "TODO.md", &todo.replace(&format!("{typed}\n"), ""));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.deletes, 1);
    assert_eq!(mock.resource_names(PHONE_PATH), vec![TASKS_ORG_NAME]);
}

#[tokio::test]
async fn a_list_whose_calendar_becomes_another_one_loses_no_line() {
    let dir = vault_showing("\"prova\"");
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_collection(PHONE_PATH, "Prova");
    mock.seed_resource(PHONE_PATH, TASKS_ORG_NAME, &work_task());
    let engine = engine(&dir, &mock);
    engine.reconcile().await.unwrap();
    engine.reconcile().await.unwrap();
    let line = read(&dir, "TODO.md")
        .lines()
        .find(|line| line.contains("Update restask README"))
        .unwrap()
        .to_string();

    // A second calendar of that name: restask does not choose, and concludes nothing
    // from a list it did not look at.
    mock.seed_collection("0b1f6c1e-3a52-4c0e-9d58-0f3c2f6f1a77", "prova");
    let (counters, files) = (mock.counters(), snapshot(&dir));
    let report = engine.reconcile().await.unwrap();
    assert_eq!(
        (report.pushes, report.deletes, report.markdown_mutations),
        (0, 0, 0)
    );
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(snapshot(&dir), files);
    assert_eq!(mock.collection_names().len(), 3);

    // A collection at the list's own path: it is the list's from now on. The one task
    // settled with the other calendar is not in it — and was not deleted there.
    mock.seed_collection("prova", "Prova");
    let report = engine.reconcile().await.unwrap();
    assert_eq!(report.deletes, 0);
    assert!(read(&dir, "TODO.md").contains(&line), "the line is kept");
    assert_eq!(mock.resource_names("prova").len(), 1);
    assert_eq!(mock.resource_names(PHONE_PATH), vec![TASKS_ORG_NAME]);

    // Quiet again.
    let (counters, files) = (mock.counters(), snapshot(&dir));
    engine.reconcile().await.unwrap();
    assert_eq!(
        (mock.counters().0, mock.counters().1),
        (counters.0, counters.1)
    );
    assert_eq!(snapshot(&dir), files);
}

// ---- §7.5: a calendar that appears on the server is shown in TODO.md ----

#[tokio::test]
async fn a_calendar_that_appears_on_the_server_is_added_to_the_ones_todo_md_shows() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    mock.seed_resource("work", TASKS_ORG_NAME, &work_task());
    let mut engine = engine(&dir, &mock);
    let config = || read(&dir, "restask.toml");
    let before = config();

    // The first look remembers what is there: nothing is new, `work` was not chosen.
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    assert_eq!(config(), before);
    engine.reconcile().await.unwrap();
    assert!(!read(&dir, "TODO.md").contains("Update restask README"));

    // Made on the phone afterwards: one for tasks, one for events.
    mock.seed_collection(PHONE_PATH, "University");
    mock.seed_resource(
        PHONE_PATH,
        "exam",
        &work_task().replace("Update restask README", "Enrol for the exam"),
    );
    mock.seed_event_calendar("7c1d", "Sport");
    assert_eq!(engine.follow_calendars().await.unwrap(), vec!["university"]);
    assert_eq!(
        VaultConfig::load(&dir.path().join("restask.toml"))
            .unwrap()
            .todo_lists,
        vec!["university"]
    );
    engine.reconcile().await.unwrap();
    let todo = read(&dir, "TODO.md");
    assert!(
        todo.contains(&format!(
            "- [ ] Enrol for the exam {CALENDAR} university {ID}"
        )),
        "{todo}"
    );
    assert!(!todo.contains("Update restask README"));
    assert_eq!(mock.collection_names().len(), 4, "no calendar was created");

    // Quiet: a second look adds nothing and writes nothing.
    let files = snapshot(&dir);
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    engine.reconcile().await.unwrap();
    assert_eq!(snapshot(&dir), files);

    // Taken out by the user, it stays out: it is not new any more.
    write_vault_file(&dir, "restask.toml", "done_heading = \"Done\"\n");
    engine.set_config(VaultConfig::load(&dir.path().join("restask.toml")).unwrap());
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    assert_eq!(config(), "done_heading = \"Done\"\n");

    // Without the memory of earlier looks nothing is new either.
    std::fs::remove_file(dir.path().join(".restask/calendars.json")).unwrap();
    mock.seed_collection("9e2f", "Family");
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    assert_eq!(config(), "done_heading = \"Done\"\n");

    // Switched off in the vault's config.
    write_vault_file(
        &dir,
        "restask.toml",
        "done_heading = \"Done\"\ntodo_new_lists = false\n",
    );
    engine.set_config(VaultConfig::load(&dir.path().join("restask.toml")).unwrap());
    mock.seed_collection("4a4a", "Garden");
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    assert!(!config().contains("garden"));
}

#[tokio::test]
async fn a_calendar_restask_created_itself_is_not_one_that_appeared() {
    let dir = temp_vault();
    let mock = MockCaldav::new();
    mock.seed_collection("inbox", "Inbox");
    let mut engine = engine(&dir, &mock);
    assert!(engine.follow_calendars().await.unwrap().is_empty());

    // A routed note's list gets its collection from the engine.
    home_note(&dir, "- [ ] Water the plants\n");
    engine.reconcile().await.unwrap();
    assert_eq!(mock.collection_names(), vec!["home", "inbox"]);
    let before = read(&dir, "restask.toml");
    assert!(engine.follow_calendars().await.unwrap().is_empty());
    assert_eq!(read(&dir, "restask.toml"), before);
}
