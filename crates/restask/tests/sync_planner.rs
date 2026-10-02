//! Planner conformance (§11): every rule of the reconciliation table, exercised on pure
//! snapshots. Server resources are built through the real codec, so what the planner
//! sees is what a server would hand back.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, TimeZone, Utc};

use restask::caldav::RemoteResource;
use restask::domain::{
    ListSlug, LocalDate, Priority, Recurrence, SourceRef, Status, Task, TaskUid, When,
};
use restask::markdown::mutator::{Mutation, WhenField};
use restask::store::index::{Index, IndexEntry};
use restask::sync::{plan, DeferReason, DeleteOp, Plan, Snapshots, DEFER_LIMIT};
use restask::vtodo::{from_vcalendar, to_vcalendar_with};

/// The instant everything was last in agreement.
const T0: i64 = 1_800_000_000;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap()
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

fn uid(n: u64) -> TaskUid {
    TaskUid::parse(&format!("restask-01jz{n:022}")).unwrap()
}

fn date(value: &str) -> LocalDate {
    LocalDate::parse(value).unwrap()
}

/// An active task in `notes/<list>.md`, created 2026-09-01, file last written at `T0`.
fn task(n: u64, list: &str, text: &str) -> Task {
    Task {
        uid: uid(n),
        list: slug(list),
        text: text.to_string(),
        status: Status::Active,
        priority: None,
        due: None,
        start: None,
        scheduled: None,
        recurrence: None,
        created: Some(date("2026-09-01")),
        parent: None,
        source: SourceRef {
            path: format!("notes/{list}.md"),
            line: 5,
        },
        source_heading: None,
        source_mtime: at(T0),
        last_modified: at(T0),
    }
}

/// The server copy of `task` as written at `stamp` (through the codec), with extras.
fn resource_with(task: &Task, stamp: i64, etag: &str, extras: &[&str]) -> RemoteResource {
    let extras: Vec<String> = extras.iter().map(|line| line.to_string()).collect();
    let body = to_vcalendar_with(task, at(stamp), &extras);
    RemoteResource {
        name: task.uid.as_str().to_string(),
        etag: etag.to_string(),
        task: from_vcalendar(&body, &Utc, &task.list).unwrap(),
    }
}

fn resource(task: &Task, stamp: i64, etag: &str) -> RemoteResource {
    resource_with(task, stamp, etag, &[])
}

/// A resource another client created: raw iCalendar with a non-restask UID.
fn foreign(list: &str, name: &str, body_lines: &[&str]) -> RemoteResource {
    let mut body = String::from("BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n");
    for line in body_lines {
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.push_str("END:VTODO\r\nEND:VCALENDAR\r\n");
    RemoteResource {
        name: name.to_string(),
        etag: format!("\"{name}\""),
        task: from_vcalendar(&body, &Utc, &slug(list)).unwrap(),
    }
}

/// Snapshot builder: the vault routes `notes/home.md` → `home`, `notes/work.md` → `work`,
/// and `TODO.md` → `inbox`; all three collections are listed (empty unless filled).
struct World {
    s: Snapshots,
}

impl World {
    fn new() -> Self {
        let mut s = Snapshots {
            inbox_file: "TODO.md".to_string(),
            inbox_list: Some(slug("inbox")),
            ..Snapshots::default()
        };
        for list in ["home", "work"] {
            s.notes.insert(format!("notes/{list}.md"), slug(list));
            s.homes.insert(slug(list), format!("notes/{list}.md"));
        }
        for list in ["home", "work", "inbox"] {
            s.remote.insert(slug(list), Vec::new());
        }
        Self { s }
    }

    fn local(mut self, task: &Task) -> Self {
        self.s.local.insert(task.uid.clone(), task.clone());
        self
    }

    fn remote(mut self, resource: RemoteResource, list: &str) -> Self {
        self.s.remote.entry(slug(list)).or_default().push(resource);
        self
    }

    /// Records `task` as settled: base snapshot (written at `T0`) plus index entry.
    fn settled(mut self, task: &Task, etag: &str) -> Self {
        let mut base = task.clone();
        base.last_modified = at(T0);
        self.s.index.upsert(IndexEntry {
            uid: task.uid.clone(),
            list: task.list.clone(),
            source_path: task.source.path.clone(),
            thumbprint: base.thumbprint(),
            caldav_etag: Some(etag.to_string()),
            seen_at: at(T0),
            defer_count: 0,
        });
        self.s.base.insert(task.uid.clone(), base);
        self
    }

    fn tombstone(mut self, n: u64) -> Self {
        self.s.tombstones.insert(uid(n));
        self
    }

    fn plan(&self) -> Plan {
        plan(&self.s)
    }
}

fn mutations_for<'a>(p: &'a Plan, path: &str) -> &'a [Mutation] {
    p.mutations.get(path).map(Vec::as_slice).unwrap_or_default()
}

// ── nothing to do ─────────────────────────────────────────────────────────────────────

#[test]
fn empty_snapshots_yield_an_empty_plan() {
    assert!(plan(&Snapshots::default()).is_noop());
    assert!(World::new().plan().is_noop());
}

#[test]
fn a_settled_task_that_nobody_touched_is_a_no_op() {
    let t = task(1, "home", "steady");
    let p = World::new()
        .local(&t)
        .settled(&t, "\"e1\"")
        .remote(resource(&t, T0, "\"e1\""), "home")
        .plan();
    assert!(p.is_noop(), "{p:?}");
}

#[test]
fn the_plan_is_deterministic() {
    let t = task(1, "home", "x");
    let world = World::new().local(&t).remote(
        foreign("inbox", "f1", &["UID:f1@tasks.org", "SUMMARY:adopt me"]),
        "inbox",
    );
    assert_eq!(world.plan(), world.plan());
}

// ── R2: new in the vault ──────────────────────────────────────────────────────────────

#[test]
fn r2_a_new_vault_task_is_created_on_the_server() {
    let t = task(1, "home", "new");
    let p = World::new().local(&t).plan();
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].task, t);
    assert_eq!(p.puts[0].if_match, None, "create, never overwrite");
    assert!(p.puts[0].extras.is_empty());
    assert!(p.mutations.is_empty() && p.settled.is_empty());
}

#[test]
fn nothing_is_decided_for_a_list_that_was_not_listed() {
    // The collection is missing and may not be created: its tasks simply wait.
    let t = task(1, "home", "waiting");
    let mut world = World::new().local(&t).settled(&t, "\"e1\"");
    world.s.remote.remove(&slug("home"));
    assert!(world.plan().is_noop());
}

// ── R7/R8: the three-way merge ────────────────────────────────────────────────────────

#[test]
fn first_contact_with_equal_content_just_settles() {
    let t = task(1, "home", "same on both sides");
    let p = World::new()
        .local(&t)
        .remote(resource(&t, T0, "\"e1\""), "home")
        .plan();
    assert!(p.puts.is_empty() && p.mutations.is_empty());
    assert_eq!(p.settled.len(), 1);
    assert_eq!(p.settled[0].etag, "\"e1\"");
    assert_eq!(p.settled[0].task, t);
}

#[test]
fn a_server_side_change_reaches_the_vault_however_new_the_file_is() {
    // The regression this design exists for: the file's mtime covers all its tasks and
    // is bumped by any unrelated edit. Only the base can tell that this task did not
    // change locally.
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.last_modified = at(T0 + 86_400);
    let mut theirs = base.clone();
    theirs.text = "alpha (reworded on the phone)".to_string();
    theirs.priority = Some(Priority::High);
    theirs.due = Some(When::Date(date("2026-10-01")));
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 60, "\"e2\""), "home")
        .plan();
    assert!(p.puts.is_empty(), "the server already has the result");
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [
            Mutation::EditText {
                uid: uid(1),
                text: "alpha (reworded on the phone)".to_string(),
            },
            Mutation::SetPriority {
                uid: uid(1),
                priority: Some(Priority::High),
            },
            Mutation::SetWhen {
                uid: uid(1),
                field: WhenField::Due,
                value: Some(When::Date(date("2026-10-01"))),
            },
        ]
    );
    assert_eq!(p.settled.len(), 1);
    assert_eq!(p.settled[0].task.text, "alpha (reworded on the phone)");
    assert_eq!(p.settled[0].etag, "\"e2\"");
}

#[test]
fn a_vault_side_change_is_pushed_however_new_the_server_copy_is() {
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.text = "alpha, edited in the note".to_string();
    // Another client touched the resource later without changing our fields.
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(
            resource_with(
                &base,
                T0 + 86_400,
                "\"e2\"",
                &["DESCRIPTION:notes from Tasks.org"],
            ),
            "home",
        )
        .plan();
    assert!(p.mutations.is_empty());
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].task.text, "alpha, edited in the note");
    assert_eq!(p.puts[0].if_match.as_deref(), Some("\"e2\""));
    assert_eq!(p.puts[0].name, uid(1).as_str());
    assert_eq!(
        p.puts[0].extras,
        vec!["DESCRIPTION:notes from Tasks.org"],
        "other clients' content rides along"
    );
}

#[test]
fn changes_to_different_fields_on_both_sides_are_both_kept() {
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.text = "alpha, reworded in the note".to_string();
    let mut theirs = base.clone();
    theirs.status = Status::Completed {
        on: date("2026-09-20"),
    };
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 30, "\"e2\""), "home")
        .plan();
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [
            Mutation::SetStatus {
                uid: uid(1),
                checked: true,
                completed_on: Some(date("2026-09-20")),
            },
            Mutation::MoveToDone { uid: uid(1) },
        ]
    );
    assert_eq!(p.puts.len(), 1);
    let pushed = &p.puts[0].task;
    assert_eq!(pushed.text, "alpha, reworded in the note");
    assert_eq!(
        pushed.status,
        Status::Completed {
            on: date("2026-09-20")
        }
    );
}

#[test]
fn a_true_conflict_goes_to_the_vault_inside_the_tie_window() {
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.text = "vault wording".to_string();
    let mut theirs = base.clone();
    theirs.text = "server wording".to_string();
    // The server copy is newer, but by no more than the 120 s tie window.
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 120, "\"e2\""), "home")
        .plan();
    assert!(p.mutations.is_empty());
    assert_eq!(p.puts[0].task.text, "vault wording");
}

#[test]
fn a_true_conflict_goes_to_the_server_when_it_is_clearly_newer() {
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.text = "vault wording".to_string();
    local.priority = Some(Priority::Low);
    let mut theirs = base.clone();
    theirs.text = "server wording".to_string();
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 121, "\"e2\""), "home")
        .plan();
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [Mutation::EditText {
            uid: uid(1),
            text: "server wording".to_string(),
        }]
    );
    // Only the conflicting field was lost; the vault's other change is still pushed.
    assert_eq!(p.puts[0].task.text, "server wording");
    assert_eq!(p.puts[0].task.priority, Some(Priority::Low));
}

#[test]
fn without_a_base_every_difference_is_a_conflict() {
    let mut local = task(1, "home", "vault wording");
    local.last_modified = at(T0);
    let mut theirs = local.clone();
    theirs.text = "server wording".to_string();
    let older = World::new()
        .local(&local)
        .remote(resource(&theirs, T0 - 500, "\"e1\""), "home")
        .plan();
    assert_eq!(older.puts[0].task.text, "vault wording");
    let newer = World::new()
        .local(&local)
        .remote(resource(&theirs, T0 + 500, "\"e1\""), "home")
        .plan();
    assert!(newer.puts.is_empty());
    assert_eq!(newer.settled[0].task.text, "server wording");
}

#[test]
fn a_base_the_index_does_not_vouch_for_is_ignored() {
    // Something else rewrote the snapshot (e.g. an old plugin): it is not the base.
    let base = task(1, "home", "alpha");
    let mut theirs = base.clone();
    theirs.text = "server wording".to_string();
    let mut world = World::new()
        .local(&base)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 - 500, "\"e2\""), "home");
    world.s.base.get_mut(&uid(1)).unwrap().text = "tampered".to_string();
    let p = world.plan();
    // With a valid base the server would win (only it changed); without one this is a
    // conflict, and the older server copy loses to the vault.
    assert_eq!(p.puts[0].task.text, "alpha");
}

#[test]
fn a_server_side_reopen_restores_the_line() {
    let mut base = task(1, "home", "alpha");
    base.status = Status::Completed {
        on: date("2026-09-20"),
    };
    let mut theirs = base.clone();
    theirs.status = Status::Active;
    let p = World::new()
        .local(&base)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 30, "\"e2\""), "home")
        .plan();
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [
            Mutation::RestoreFromDone { uid: uid(1) },
            Mutation::SetStatus {
                uid: uid(1),
                checked: false,
                completed_on: None,
            },
        ]
    );
}

#[test]
fn mutations_for_one_file_are_grouped_into_one_pass() {
    let a = task(1, "home", "a");
    let b = task(2, "home", "b");
    let mut a2 = a.clone();
    a2.text = "a2".to_string();
    let mut b2 = b.clone();
    b2.text = "b2".to_string();
    let p = World::new()
        .local(&a)
        .local(&b)
        .settled(&a, "\"a\"")
        .settled(&b, "\"b\"")
        .remote(resource(&a2, T0 + 5, "\"a2\""), "home")
        .remote(resource(&b2, T0 + 5, "\"b2\""), "home")
        .plan();
    assert_eq!(p.mutations.len(), 1);
    assert_eq!(mutations_for(&p, "notes/home.md").len(), 2);
}

// ── what is not merged: creation date, source, parent ─────────────────────────────────

#[test]
fn a_line_without_a_created_token_is_not_pushed_just_for_that() {
    let mut local = task(1, "home", "no plus token");
    local.created = None;
    let mut theirs = local.clone();
    theirs.created = Some(date("2026-08-15"));
    let p = World::new()
        .local(&local)
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    assert!(p.puts.is_empty() && p.mutations.is_empty());
    assert_eq!(p.settled[0].task.created, Some(date("2026-08-15")));
}

#[test]
fn the_vaults_created_token_and_source_path_are_pushed_when_they_differ() {
    let local = task(1, "home", "t");
    let mut theirs = local.clone();
    theirs.created = Some(date("2026-08-15"));
    let p = World::new()
        .local(&local)
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(p.puts[0].task.created, Some(date("2026-09-01")));

    let mut elsewhere = local.clone();
    elsewhere.source.path = "notes/old-name.md".to_string();
    let p = World::new()
        .local(&local)
        .remote(resource(&elsewhere, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(p.puts.len(), 1, "X-RESTASK-SOURCE must follow the note");
}

#[test]
fn indentation_decides_the_parent_of_an_active_note_task() {
    let local = task(2, "home", "now a root line");
    let mut theirs = local.clone();
    theirs.parent = Some(uid(1));
    let p = World::new()
        .local(&local)
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].task.parent, None);
}

#[test]
fn the_servers_parent_is_kept_where_the_vault_cannot_express_one() {
    // Done records and TODO.md lines are flat: their parent relation lives on the server.
    let mut done = task(2, "home", "finished child");
    done.status = Status::Completed {
        on: date("2026-09-20"),
    };
    let mut theirs = done.clone();
    theirs.parent = Some(uid(1));
    let p = World::new()
        .local(&done)
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    assert!(p.puts.is_empty());
    assert_eq!(p.settled[0].task.parent, Some(uid(1)));

    let mut inbox = task(3, "inbox", "captured child");
    inbox.source.path = "TODO.md".to_string();
    let mut theirs = inbox.clone();
    theirs.parent = Some(uid(1));
    theirs.text = "captured child, reworded".to_string();
    let p = World::new()
        .local(&inbox)
        .settled(&inbox, "\"e1\"")
        .remote(resource(&theirs, T0 + 5, "\"e2\""), "inbox")
        .plan();
    assert!(p.puts.is_empty(), "the parent is not stripped");
}

// ── R3: deleted on the server ─────────────────────────────────────────────────────────

#[test]
fn r3_a_settled_task_gone_from_its_collection_is_deleted_in_the_vault() {
    let t = task(1, "home", "deleted in Tasks.org");
    let p = World::new().local(&t).settled(&t, "\"e1\"").plan();
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [Mutation::Delete { uid: uid(1) }]
    );
    assert_eq!(p.tombstones, vec![uid(1)]);
    assert_eq!(p.forgets, vec![uid(1)]);
    assert!(p.puts.is_empty() && p.deletes.is_empty());
}

#[test]
fn r3_never_fires_for_a_collection_that_was_not_listed() {
    // The task was settled in `home` and now routes to `work`; `home` is unknown this
    // cycle, so "it is not there" proves nothing.
    let settled = task(1, "home", "t");
    let mut local = settled.clone();
    local.list = slug("work");
    local.source.path = "notes/work.md".to_string();
    let mut world = World::new().local(&local).settled(&settled, "\"e1\"");
    world.s.remote.remove(&slug("home"));
    let p = world.plan();
    assert!(p.mutations.is_empty() && p.tombstones.is_empty());
    assert_eq!(p.puts.len(), 1, "re-created where it routes now");
}

#[test]
fn a_collection_that_lost_everything_at_once_is_a_reset_not_a_deletion_spree() {
    let a = task(1, "home", "a");
    let b = task(2, "home", "b");
    let p = World::new()
        .local(&a)
        .local(&b)
        .settled(&a, "\"a\"")
        .settled(&b, "\"b\"")
        .plan();
    assert!(p.mutations.is_empty(), "the vault is not wiped");
    assert!(p.tombstones.is_empty());
    assert_eq!(p.puts.len(), 2, "the collection is refilled from the vault");
    assert!(p.puts.iter().all(|put| put.if_match.is_none()));
}

#[test]
fn a_collection_created_this_cycle_cannot_hold_deletions() {
    let t = task(1, "home", "the only one");
    let mut world = World::new().local(&t).settled(&t, "\"e1\"");
    world.s.created.insert(slug("home"));
    let p = world.plan();
    assert!(p.mutations.is_empty() && p.tombstones.is_empty());
    assert_eq!(p.puts.len(), 1);
}

#[test]
fn one_of_several_settled_tasks_missing_is_a_real_deletion() {
    let a = task(1, "home", "kept");
    let b = task(2, "home", "deleted remotely");
    let p = World::new()
        .local(&a)
        .local(&b)
        .settled(&a, "\"a\"")
        .settled(&b, "\"b\"")
        .remote(resource(&a, T0, "\"a\""), "home")
        .plan();
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [Mutation::Delete { uid: uid(2) }]
    );
}

// ── deleted in the vault, tombstones ──────────────────────────────────────────────────

#[test]
fn a_line_removed_from_the_vault_is_deleted_on_the_server() {
    let t = task(1, "home", "removed from the note");
    let p = World::new()
        .settled(&t, "\"e1\"")
        .remote(resource(&t, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(
        p.deletes,
        vec![DeleteOp {
            list: slug("home"),
            name: uid(1).as_str().to_string(),
            etag: Some("\"e1\"".to_string()),
        }]
    );
    assert_eq!(p.tombstones, vec![uid(1)]);
    assert_eq!(p.forgets, vec![uid(1)]);
    assert!(p.mutations.is_empty());
}

#[test]
fn tasks_of_an_unreadable_note_are_unknown_not_deleted() {
    let t = task(1, "home", "its note could not be read");
    let mut world = World::new()
        .settled(&t, "\"e1\"")
        .remote(resource(&t, T0, "\"e1\""), "home");
    world.s.unreadable.insert("notes/home.md".to_string());
    assert!(world.plan().is_noop());
}

#[test]
fn r0_a_tombstoned_task_is_purged_from_the_server() {
    let t = task(1, "home", "deleted earlier, delete failed");
    let p = World::new()
        .tombstone(1)
        .remote(resource(&t, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(p.deletes.len(), 1);
    assert!(p.mutations.is_empty() && p.inbox_inserts.is_empty());
}

#[test]
fn r0_the_vault_outranks_a_tombstone() {
    // Cut a line, sync, paste it elsewhere: the task is back, so it lives. A tombstone
    // must never delete what the user has in the vault.
    let mut pasted = task(1, "work", "cut and pasted");
    pasted.source.path = "notes/work.md".to_string();
    let p = World::new().tombstone(1).local(&pasted).plan();
    assert_eq!(p.revived, vec![uid(1)]);
    assert!(p.mutations.is_empty(), "the pasted line stays");
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].task.list, slug("work"));
}

#[test]
fn r6_state_for_a_task_that_exists_nowhere_is_forgotten() {
    let t = task(1, "home", "gone everywhere");
    let p = World::new().settled(&t, "\"e1\"").plan();
    assert_eq!(p.forgets, vec![uid(1)]);
    assert!(p.tombstones.is_empty() && p.deletes.is_empty());
}

// ── R4: created on the server under a managed UID ─────────────────────────────────────

#[test]
fn r4_a_server_task_goes_back_to_the_note_it_names() {
    let mut theirs = task(1, "home", "from another device");
    theirs.source.path = "notes/home.md".to_string();
    let p = World::new()
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    match mutations_for(&p, "notes/home.md") {
        [Mutation::Insert { draft, under: None }] => {
            assert_eq!(draft.text, "from another device");
            assert_eq!(draft.uid, Some(uid(1)));
            // The line does not show the creation date; the settled task keeps it.
            assert_eq!(draft.created, None);
        }
        other => panic!("expected one insert, got {other:?}"),
    }
    assert_eq!(p.settled.len(), 1);
    assert_eq!(p.settled[0].task.source.path, "notes/home.md");
    assert_eq!(p.settled[0].task.created, Some(date("2026-09-01")));
    assert!(p.puts.is_empty() && p.inbox_inserts.is_empty());
}

#[test]
fn r4_falls_back_to_the_lists_home_note_then_to_the_inbox() {
    // Its note is gone: the list's home note takes it.
    let mut theirs = task(1, "home", "t");
    theirs.source.path = "notes/deleted.md".to_string();
    let p = World::new()
        .remote(resource(&theirs, T0, "\"e1\""), "home")
        .plan();
    assert_eq!(mutations_for(&p, "notes/home.md").len(), 1);

    // An inbox-list task lands in TODO.md, which the render writes.
    let mut inbox = task(2, "inbox", "captured on the phone");
    inbox.source.path = "TODO.md".to_string();
    let p = World::new()
        .remote(resource(&inbox, T0, "\"e2\""), "inbox")
        .plan();
    assert!(p.mutations.is_empty());
    assert_eq!(p.inbox_inserts.len(), 1);
    assert_eq!(p.inbox_inserts[0].source.path, "TODO.md");
    assert_eq!(p.settled[0].task.source.path, "TODO.md");
}

#[test]
fn r4_a_child_is_inserted_under_its_parent_in_the_same_note() {
    let parent = task(1, "home", "parent");
    let mut child = task(2, "home", "child");
    child.parent = Some(uid(1));
    let p = World::new()
        .local(&parent)
        .settled(&parent, "\"p\"")
        .remote(resource(&parent, T0, "\"p\""), "home")
        .remote(resource(&child, T0, "\"c\""), "home")
        .plan();
    match mutations_for(&p, "notes/home.md") {
        [Mutation::Insert { under, .. }] => assert_eq!(*under, Some(uid(1))),
        other => panic!("expected one insert, got {other:?}"),
    }
}

#[test]
fn r4_a_child_whose_parent_is_elsewhere_becomes_a_root_line() {
    let parent = task(1, "work", "parent in another note");
    let mut child = task(2, "home", "child");
    child.parent = Some(uid(1));
    let p = World::new()
        .local(&parent)
        .settled(&parent, "\"p\"")
        .remote(resource(&parent, T0, "\"p\""), "work")
        .remote(resource(&child, T0, "\"c\""), "home")
        .plan();
    match mutations_for(&p, "notes/home.md") {
        [Mutation::Insert { under, .. }] => assert_eq!(*under, None),
        other => panic!("expected one insert, got {other:?}"),
    }
}

// ── R5: adoption of foreign tasks ─────────────────────────────────────────────────────

const TASKS_ORG_UID: &str = "5417861935824551742";

const TASKS_ORG: [&str; 6] = [
    "UID:5417861935824551742",
    "CREATED:20260921T081233Z",
    "LAST-MODIFIED:20260922T101400Z",
    "SUMMARY:Made in Tasks.org",
    "DESCRIPTION:with a note",
    "PRIORITY:1",
];

/// The UID the Tasks.org task is adopted under.
fn tasks_org_uid() -> TaskUid {
    TaskUid::derived(
        TASKS_ORG_UID,
        Some(Utc.with_ymd_and_hms(2026, 9, 21, 8, 12, 33).unwrap()),
    )
}

/// The Tasks.org resource once restask linked it to its line in TODO.md.
fn linked(changes: &[&str]) -> RemoteResource {
    let link = format!("X-RESTASK-UID:{}", tasks_org_uid().as_str());
    let mut lines: Vec<&str> = TASKS_ORG.to_vec();
    lines.extend(["X-RESTASK-SOURCE;VALUE=TEXT:TODO.md", link.as_str()]);
    for change in changes {
        let name = change.split(':').next().unwrap();
        lines.retain(|line| !line.starts_with(name));
        lines.push(change);
    }
    foreign("inbox", "5417", &lines)
}

/// The adopted task as vault and server agreed on it when it was adopted.
fn adopted_base() -> Task {
    let world = World::new().remote(foreign("inbox", "5417", &TASKS_ORG), "inbox");
    world.plan().settled[0].task.clone()
}

/// Its line in TODO.md (no creation date shown), in a file last written at `T0`.
fn adopted_line() -> Task {
    let mut line = adopted_base();
    line.created = None;
    line.source_mtime = at(T0);
    line.last_modified = at(T0);
    line
}

/// A world where the adopted task is settled and its line is in the vault.
fn adopted_world(line: &Task) -> World {
    World::new()
        .local(line)
        .settled(&adopted_base(), "\"5417\"")
}

#[test]
fn r5_a_foreign_task_is_adopted_where_it_is() {
    let world = World::new().remote(foreign("inbox", "5417", &TASKS_ORG), "inbox");
    let p = world.plan();
    assert_eq!(p.adopted, vec![tasks_org_uid()]);

    // The resource stays the other client's: same name, same `UID`, replaced in place
    // only to say which task it is and where its line lives.
    assert!(p.deletes.is_empty() && p.moves.is_empty());
    assert_eq!(p.puts.len(), 1);
    let link = &p.puts[0];
    assert_eq!(link.name, "5417");
    assert_eq!(link.if_match.as_deref(), Some("\"5417\""));
    assert_eq!(link.wire.uid.as_deref(), Some(TASKS_ORG_UID));
    assert_eq!(link.extras, vec!["DESCRIPTION:with a note"]);

    let adopted = &link.task;
    assert_eq!(adopted.uid, tasks_org_uid());
    assert_eq!(adopted.text, "Made in Tasks.org");
    assert_eq!(adopted.priority, Some(Priority::Highest));
    assert_eq!(adopted.list, slug("inbox"));
    assert_eq!(adopted.source.path, "TODO.md");

    // What was read is settled at once, under the etag it was read with: a link the
    // server refuses is then a merge over this, not a conflict.
    assert_eq!(p.settled.len(), 1);
    assert_eq!(p.settled[0].task, *adopted);
    assert_eq!(p.settled[0].etag, "\"5417\"");

    // The server copy keeps its creation date; the line in TODO.md does not show it.
    assert!(adopted.created.is_some());
    let mut shown = adopted.clone();
    shown.created = None;
    assert_eq!(p.inbox_inserts, vec![shown]);
    // Same snapshot, same UID: a retried adoption can never duplicate the task.
    assert_eq!(world.plan().puts[0].task.uid, adopted.uid);
}

#[test]
fn r5_a_change_made_by_the_client_that_owns_the_task_reaches_its_line() {
    // Regression: the task was re-created under a restask UID and the original deleted,
    // so the next edit in Tasks.org — a priority — landed on a resource the vault line
    // was no longer tied to, and was dropped with it.
    let line = adopted_line();
    // Nothing happened: nothing to do.
    let p = adopted_world(&line).remote(linked(&[]), "inbox").plan();
    assert!(p.is_noop(), "{p:?}");

    // Tasks.org lowers the priority and keeps restask's properties.
    let p = adopted_world(&line)
        .remote(linked(&["PRIORITY:9"]), "inbox")
        .plan();
    assert_eq!(
        p.mutations.get("TODO.md").map(Vec::as_slice),
        Some(
            &[Mutation::SetPriority {
                uid: line.uid.clone(),
                priority: Some(Priority::Lowest),
            }][..]
        )
    );
    assert!(p.puts.is_empty() && p.deletes.is_empty() && p.adopted.is_empty());
    assert!(p.inbox_inserts.is_empty(), "no second line");

    // A client that writes the resource back without them: the change still arrives,
    // and the link is written again — to the same resource.
    let mut bare: Vec<&str> = TASKS_ORG.to_vec();
    bare.retain(|line| !line.starts_with("PRIORITY"));
    bare.push("PRIORITY:9");
    let p = adopted_world(&line)
        .remote(foreign("inbox", "5417", &bare), "inbox")
        .plan();
    assert_eq!(mutations_for(&p, "TODO.md").len(), 1);
    assert!(p.inbox_inserts.is_empty() && p.deletes.is_empty());
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].name, "5417");
    assert_eq!(p.puts[0].wire.uid.as_deref(), Some(TASKS_ORG_UID));
    assert_eq!(p.puts[0].task.priority, Some(Priority::Lowest));
}

#[test]
fn r5_a_change_in_the_vault_is_written_to_the_resource_of_the_other_client() {
    let line = adopted_line();
    let mut edited = line.clone();
    edited.text = "Made in Tasks.org, reworded in the vault".to_string();
    let p = adopted_world(&edited).remote(linked(&[]), "inbox").plan();
    assert!(p.mutations.is_empty());
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].name, "5417");
    assert_eq!(p.puts[0].if_match.as_deref(), Some("\"5417\""));
    assert_eq!(p.puts[0].wire.uid.as_deref(), Some(TASKS_ORG_UID));
    assert_eq!(p.puts[0].task.uid, line.uid, "the UID of the line");
    assert_eq!(p.puts[0].extras, vec!["DESCRIPTION:with a note"]);
}

#[test]
fn r5_the_link_holds_when_the_other_client_rewrites_the_creation_date() {
    let line = adopted_line();
    // Linked: the property says which task it is.
    let p = adopted_world(&line)
        .remote(linked(&["CREATED:20260101T000000Z"]), "inbox")
        .plan();
    assert!(p.adopted.is_empty() && p.mutations.is_empty() && p.deletes.is_empty());

    // Property dropped as well: the vault still knows the task by that `UID`.
    let mut bare: Vec<&str> = TASKS_ORG.to_vec();
    bare.retain(|line| !line.starts_with("CREATED"));
    let p = adopted_world(&line)
        .remote(foreign("inbox", "5417", &bare), "inbox")
        .plan();
    assert!(p.adopted.is_empty() && p.mutations.is_empty() && p.deletes.is_empty());
    assert_eq!(p.puts.len(), 1, "linked again");
    assert_eq!(p.puts[0].task.uid, line.uid);
}

#[test]
fn r5_a_copy_made_under_another_uid_is_a_task_of_its_own() {
    // Another client duplicated the resource, restask's properties included: the copy
    // must not pass for the original (it would be deleted as a stray).
    let line = adopted_line();
    let link = format!("X-RESTASK-UID:{}", line.uid.as_str());
    let p = adopted_world(&line)
        .remote(linked(&[]), "inbox")
        .remote(
            foreign(
                "inbox",
                "copy",
                &[
                    "UID:another-uid",
                    "SUMMARY:Made in Tasks.org",
                    link.as_str(),
                ],
            ),
            "inbox",
        )
        .plan();
    assert!(p.deletes.is_empty(), "{:?}", p.deletes);
    assert_eq!(p.adopted, vec![TaskUid::derived("another-uid", None)]);
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].name, "copy");
    assert_eq!(p.puts[0].wire.uid.as_deref(), Some("another-uid"));
}

#[test]
fn r5_an_interrupted_adoption_is_resumed_not_repeated() {
    // The line made it into the vault, the state did not: the resource is linked, and
    // no second line appears.
    let resumed = World::new()
        .local(&adopted_line())
        .remote(foreign("inbox", "5417", &TASKS_ORG), "inbox")
        .plan();
    assert!(resumed.inbox_inserts.is_empty() && resumed.mutations.is_empty());
    assert!(resumed.adopted.is_empty() && resumed.deletes.is_empty());
    assert_eq!(resumed.puts.len(), 1);
    assert_eq!(resumed.puts[0].name, "5417");
    assert_eq!(resumed.puts[0].extras, vec!["DESCRIPTION:with a note"]);
}

#[test]
fn r5_an_original_left_behind_by_an_earlier_adoption_is_removed() {
    // Earlier versions re-created the task under its restask UID and deleted the
    // original; a client that uploads the original again must not get a second task.
    let mut adopted = adopted_line();
    adopted.created = Some(date("2026-09-21"));
    let leftover = World::new()
        .local(&adopted)
        .settled(&adopted, "\"new\"")
        .remote(resource(&adopted, T0, "\"new\""), "inbox")
        .remote(foreign("inbox", "5417", &TASKS_ORG), "inbox")
        .plan();
    assert!(leftover.adopted.is_empty() && leftover.puts.is_empty());
    assert_eq!(leftover.deletes.len(), 1);
    assert_eq!(leftover.deletes[0].name, "5417");
}

#[test]
fn r5_an_adopted_task_deleted_in_the_vault_is_deleted_for_its_client_too() {
    // The line is gone and the state knows the task: deleted in the vault.
    let line = adopted_line();
    let mut world = World::new()
        .settled(&adopted_base(), "\"5417\"")
        .remote(linked(&[]), "inbox");
    let p = world.plan();
    assert!(p.adopted.is_empty() && p.inbox_inserts.is_empty());
    assert_eq!(p.deletes.len(), 1);
    assert_eq!(p.deletes[0].name, "5417");
    assert_eq!(p.tombstones, vec![line.uid.clone()]);

    // Uploaded again from that client's cache afterwards: it does not come back.
    world = World::new().remote(foreign("inbox", "5417", &TASKS_ORG), "inbox");
    world.s.tombstones.insert(line.uid);
    let p = world.plan();
    assert!(p.adopted.is_empty() && p.inbox_inserts.is_empty() && p.puts.is_empty());
    assert_eq!(p.deletes.len(), 1);
}

#[test]
fn r5_foreign_tasks_are_adopted_only_where_the_vault_has_a_place_for_them() {
    // `archive` is in scope only because the index still references it.
    let mut world = World::new();
    world.s.remote.insert(
        slug("archive"),
        vec![foreign(
            "archive",
            "x",
            &["UID:x@other", "SUMMARY:not ours"],
        )],
    );
    assert!(world.plan().is_noop());

    // A routed list adopts into its home note.
    let p = World::new()
        .remote(
            foreign("home", "x", &["UID:x@other", "SUMMARY:for the note"]),
            "home",
        )
        .plan();
    assert_eq!(p.adopted.len(), 1);
    assert_eq!(p.puts[0].task.source.path, "notes/home.md");
    assert_eq!(mutations_for(&p, "notes/home.md").len(), 1);
}

#[test]
fn r5_a_list_the_inbox_file_shows_adopts_into_it_unless_a_note_is_its_home() {
    // §7.5: `family` has no note; TODO.md is told to show it.
    let mut world = World::new();
    world.s.todo_lists.insert(slug("family"));
    world.s.remote.insert(
        slug("family"),
        vec![foreign(
            "family",
            "x",
            &["UID:x@other", "SUMMARY:for the view"],
        )],
    );
    let p = world.plan();
    assert_eq!(p.adopted.len(), 1);
    assert!(p.mutations.is_empty());
    assert_eq!(p.inbox_inserts.len(), 1);
    assert_eq!(p.inbox_inserts[0].list, slug("family"));
    assert_eq!(p.inbox_inserts[0].source.path, "TODO.md");
    assert_eq!(p.puts[0].task.list, slug("family"));

    // A list with a note of its own keeps adopting into that note.
    let mut world = World::new().remote(
        foreign("home", "x", &["UID:x@other", "SUMMARY:for the note"]),
        "home",
    );
    world.s.todo_lists.insert(slug("home"));
    let p = world.plan();
    assert!(p.inbox_inserts.is_empty());
    assert_eq!(mutations_for(&p, "notes/home.md").len(), 1);
}

#[test]
fn r5_a_task_in_the_vault_stays_tied_to_its_resource_when_its_list_lost_its_place() {
    // Adopted into TODO.md while `family` was shown there; `family` is shown no more.
    let seed = foreign("family", "x", &["UID:x@other", "SUMMARY:kept"]);
    let adopted = TaskUid::derived("x@other", seed.task.created_at);
    let mut line = task(1, "family", "kept");
    line.uid = adopted.clone();
    line.created = None;
    line.source.path = "TODO.md".to_string();
    let linked = foreign(
        "family",
        "x",
        &[
            "UID:x@other",
            "SUMMARY:kept",
            &format!("X-RESTASK-UID:{adopted}"),
            "X-RESTASK-SOURCE:TODO.md",
        ],
    );
    let etag = linked.etag.clone();
    let mut world = World::new().local(&line).settled(&line, &etag);
    world.s.remote.insert(slug("family"), vec![linked]);
    let p = world.plan();
    assert!(p.mutations.is_empty(), "{:?}", p.mutations);
    assert!(p.deletes.is_empty() && p.tombstones.is_empty());

    // What is not in the vault is not taken in.
    let mut world = World::new();
    world.s.remote.insert(slug("family"), vec![seed]);
    assert!(world.plan().is_noop());
}

#[test]
fn r5_a_foreign_family_keeps_its_hierarchy_parents_first() {
    let parent = TaskUid::derived("p@tasks.org", None);
    let child = TaskUid::derived("c@tasks.org", None);
    // Document order is child first: the plan must still insert the parent first.
    let p = World::new()
        .remote(
            foreign(
                "home",
                "a-child",
                &[
                    "UID:c@tasks.org",
                    "SUMMARY:child",
                    "RELATED-TO;RELTYPE=PARENT:p@tasks.org",
                ],
            ),
            "home",
        )
        .remote(
            foreign("home", "b-parent", &["UID:p@tasks.org", "SUMMARY:parent"]),
            "home",
        )
        .plan();
    match mutations_for(&p, "notes/home.md") {
        [Mutation::Insert {
            draft: first,
            under: None,
        }, Mutation::Insert {
            draft: second,
            under,
        }] => {
            assert_eq!(first.uid, Some(parent.clone()));
            assert_eq!(second.uid, Some(child.clone()));
            assert_eq!(*under, Some(parent.clone()));
        }
        other => panic!("expected parent then child, got {other:?}"),
    }
    // In the vault the child hangs under the parent's restask UID; on the server the
    // relation keeps naming the parent as its client does.
    let pushed_child = p.puts.iter().find(|put| put.task.uid == child).unwrap();
    assert_eq!(pushed_child.task.parent, Some(parent));
    assert_eq!(pushed_child.wire.parent.as_deref(), Some("p@tasks.org"));
    assert_eq!(pushed_child.wire.uid.as_deref(), Some("c@tasks.org"));
}

#[test]
fn r5_a_task_of_the_vault_under_an_adopted_parent_names_it_as_its_client_does() {
    let line = adopted_line();
    let mut child = task(7, "inbox", "typed under it");
    child.source.path = "TODO.md".to_string();
    child.parent = Some(line.uid.clone());
    let p = adopted_world(&line)
        .local(&child)
        .remote(linked(&[]), "inbox")
        .plan();
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].wire.uid, None, "a task of restask's own");
    assert_eq!(p.puts[0].wire.parent.as_deref(), Some(TASKS_ORG_UID));
}

#[test]
fn r5_a_second_resource_with_the_same_foreign_uid_is_just_removed() {
    let p = World::new()
        .remote(
            foreign("inbox", "one", &["UID:dup@x", "SUMMARY:t"]),
            "inbox",
        )
        .remote(
            foreign("inbox", "two", &["UID:dup@x", "SUMMARY:t"]),
            "inbox",
        )
        .plan();
    assert_eq!(p.adopted.len(), 1);
    assert_eq!(p.puts.len(), 1);
    assert_eq!(p.puts[0].name, "one");
    assert_eq!(p.deletes.len(), 1);
    assert_eq!(p.deletes[0].name, "two");
}

// ── R9: list moves ────────────────────────────────────────────────────────────────────

#[test]
fn r9_a_rerouted_task_moves_between_collections_with_its_extras() {
    let settled = task(1, "home", "moving");
    let mut local = settled.clone();
    local.list = slug("work");
    local.source.path = "notes/work.md".to_string();
    let p = World::new()
        .local(&local)
        .settled(&settled, "\"e1\"")
        .remote(
            resource_with(&settled, T0, "\"e1\"", &["DESCRIPTION:keep me"]),
            "home",
        )
        .plan();
    assert_eq!(p.moves.len(), 1);
    let mv = &p.moves[0];
    assert_eq!(mv.put.task.list, slug("work"));
    assert_eq!(mv.put.task.uid, uid(1), "the UID survives the move");
    assert_eq!(mv.put.if_match, None);
    assert_eq!(mv.put.extras, vec!["DESCRIPTION:keep me"]);
    assert_eq!(mv.from.list, slug("home"));
    assert_eq!(mv.from.etag.as_deref(), Some("\"e1\""));
    assert!(p.puts.is_empty() && p.deletes.is_empty());
}

#[test]
fn r9_stray_copies_in_other_collections_are_deleted() {
    let t = task(1, "home", "in two places");
    let mut stray = t.clone();
    stray.list = slug("work");
    let p = World::new()
        .local(&t)
        .settled(&t, "\"e1\"")
        .remote(resource(&t, T0, "\"e1\""), "home")
        .remote(resource(&stray, T0, "\"s\""), "work")
        .plan();
    assert_eq!(p.deletes.len(), 1);
    assert_eq!(p.deletes[0].list, slug("work"));
    assert!(p.moves.is_empty() && p.puts.is_empty());
}

#[test]
fn a_task_another_client_stored_under_its_own_name_is_replaced_in_place() {
    // Same UID, different resource name: the write must target that resource (a PUT to
    // `<uid>.ics` would create a second copy), and a duplicate in the same collection
    // is a stray like any other.
    let base = task(1, "home", "alpha");
    let mut local = base.clone();
    local.text = "alpha, edited".to_string();
    let mut renamed = resource(&base, T0, "\"r\"");
    renamed.name = "ABCD-1234".to_string();
    let p = World::new()
        .local(&local)
        .settled(&base, "\"r\"")
        .remote(renamed.clone(), "home")
        .plan();
    assert_eq!(p.puts[0].name, "ABCD-1234");
    assert_eq!(p.puts[0].if_match.as_deref(), Some("\"r\""));

    let p = World::new()
        .local(&base)
        .settled(&base, "\"e1\"")
        .remote(renamed, "home")
        .remote(resource(&base, T0, "\"e1\""), "home")
        .plan();
    assert!(p.puts.is_empty());
    assert_eq!(p.deletes.len(), 1);
    assert_eq!(
        p.deletes[0].name, "ABCD-1234",
        "the UID-named resource is canonical"
    );
}

// ── R8r: recurring tasks ──────────────────────────────────────────────────────────────

fn repeat(text: &str) -> Recurrence {
    Recurrence::from_text(text).unwrap().0
}

#[test]
fn completing_a_recurring_task_rolls_the_series_and_leaves_a_record() {
    let mut base = task(1, "home", "water the plants");
    base.due = Some(When::Date(date("2026-09-21")));
    base.start = Some(When::Date(date("2026-09-20")));
    base.recurrence = Some(repeat("every week for 5 times"));
    let mut local = base.clone();
    local.status = Status::Completed {
        on: date("2026-09-21"),
    };
    let world = World::new().local(&local).settled(&base, "\"e1\"").remote(
        resource_with(&base, T0, "\"e1\"", &["DESCRIPTION:d"]),
        "home",
    );
    let p = world.plan();

    assert_eq!(p.puts.len(), 2);
    let series = p.puts.iter().find(|put| put.task.uid == uid(1)).unwrap();
    assert_eq!(series.task.status, Status::Active);
    assert_eq!(series.task.due, Some(When::Date(date("2026-09-28"))));
    assert_eq!(
        series.task.start,
        Some(When::Date(date("2026-09-27"))),
        "the other dates move with the due date"
    );
    assert_eq!(
        series.task.recurrence,
        Some(repeat("every week for 4 times"))
    );
    assert_eq!(series.if_match.as_deref(), Some("\"e1\""));
    assert_eq!(series.extras, vec!["DESCRIPTION:d"]);

    let record = p.puts.iter().find(|put| put.task.uid != uid(1)).unwrap();
    assert_eq!(record.task.status, local.status);
    assert_eq!(record.task.due, base.due);
    assert_eq!(record.task.recurrence, None, "a record does not repeat");
    assert!(record.extras.is_empty() && record.if_match.is_none());
    assert_eq!(record.name, record.task.uid.as_str());

    match mutations_for(&p, "notes/home.md") {
        [Mutation::Rekey { uid: from, new_uid }, Mutation::Insert { draft, under: None }] => {
            assert_eq!((from, new_uid), (&uid(1), &record.task.uid));
            assert_eq!(draft.uid, Some(uid(1)));
            assert!(!draft.checked);
            assert_eq!(draft.due, Some(When::Date(date("2026-09-28"))));
            assert_eq!(draft.recurrence, Some(repeat("every week for 4 times")));
        }
        other => panic!("expected rekey + insert, got {other:?}"),
    }
    // Deterministic: the same occurrence always yields the same record.
    assert_eq!(world.plan(), p);
}

#[test]
fn a_rule_only_the_server_can_express_rolls_too_and_is_handed_back() {
    let mut base = task(1, "home", "month-end report");
    base.due = Some(When::Date(date("2026-09-30")));
    let mut local = base.clone();
    local.status = Status::Completed {
        on: date("2026-09-30"),
    };
    let rule = "RRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR;COUNT=3";
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource_with(&base, T0, "\"e1\"", &[rule]), "home")
        .plan();
    let series = p.puts.iter().find(|put| put.task.uid == uid(1)).unwrap();
    assert_eq!(series.task.status, Status::Active);
    assert_eq!(series.task.recurrence, None, "not a vault rule");
    assert_ne!(series.task.due, base.due);
    assert_eq!(
        series.extras,
        vec!["RRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR;COUNT=2"]
    );
}

#[test]
fn a_recurring_task_is_not_rolled_when_the_server_side_completed_it() {
    // Another client marked the whole task completed: that is its decision.
    let mut base = task(1, "home", "water the plants");
    base.due = Some(When::Date(date("2026-09-21")));
    base.recurrence = Some(repeat("every week"));
    let mut theirs = base.clone();
    theirs.status = Status::Completed {
        on: date("2026-09-21"),
    };
    let p = World::new()
        .local(&base)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 5, "\"e2\""), "home")
        .plan();
    assert!(p.puts.is_empty());
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [
            Mutation::SetStatus {
                uid: uid(1),
                checked: true,
                completed_on: Some(date("2026-09-21")),
            },
            Mutation::MoveToDone { uid: uid(1) },
        ]
    );
}

#[test]
fn the_repeat_rule_is_a_field_like_any_other() {
    let base = task(1, "home", "water the plants");
    // Set in another client → written into the vault line.
    let mut theirs = base.clone();
    theirs.recurrence = Some(repeat("every 2 days"));
    let p = World::new()
        .local(&base)
        .settled(&base, "\"e1\"")
        .remote(resource(&theirs, T0 + 5, "\"e2\""), "home")
        .plan();
    assert!(p.puts.is_empty());
    assert_eq!(
        mutations_for(&p, "notes/home.md"),
        [Mutation::SetRecurrence {
            uid: uid(1),
            recurrence: Some(repeat("every 2 days")),
        }]
    );
    // Set in the vault → pushed; a rule the server kept as an extra is replaced by it.
    let mut local = base.clone();
    local.recurrence = Some(repeat("every month on the 15th"));
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&base, T0, "\"e1\""), "home")
        .plan();
    assert!(p.mutations.is_empty());
    assert_eq!(p.puts[0].task.recurrence, local.recurrence);
}

// ── R1: a vault file that file sync has not caught up yet ─────────────────────────────

fn stale_world(defer_count: u8) -> World {
    // The base says "beta" and was written at T0; the vault file still says "alpha"
    // and predates the base by an hour: this replica has not received the note yet.
    let base = task(1, "home", "beta");
    let mut local = task(1, "home", "alpha");
    local.last_modified = at(T0 - 3_600);
    let mut world = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&base, T0, "\"e1\""), "home");
    world.s.index.entries.get_mut(&uid(1)).unwrap().defer_count = defer_count;
    world
}

#[test]
fn r1_a_vault_file_older_than_the_base_it_disagrees_with_is_deferred() {
    let p = stale_world(0).plan();
    assert_eq!(p.deferred, vec![(uid(1), DeferReason::VaultFileStale)]);
    assert!(p.puts.is_empty() && p.mutations.is_empty() && p.settled.is_empty());
}

#[test]
fn r1_gives_up_waiting_and_trusts_the_vault() {
    let p = stale_world(DEFER_LIMIT).plan();
    assert!(p.deferred.is_empty());
    assert_eq!(p.puts[0].task.text, "alpha", "vault authority");
}

#[test]
fn r1_does_not_fire_inside_the_tie_window_or_when_content_agrees() {
    let base = task(1, "home", "beta");
    let mut local = task(1, "home", "alpha");
    local.last_modified = at(T0 - 120);
    let p = World::new()
        .local(&local)
        .settled(&base, "\"e1\"")
        .remote(resource(&base, T0, "\"e1\""), "home")
        .plan();
    assert!(p.deferred.is_empty());

    let mut old_but_equal = base.clone();
    old_but_equal.last_modified = at(T0 - 3_600);
    let p = World::new()
        .local(&old_but_equal)
        .settled(&base, "\"e1\"")
        .remote(resource(&base, T0, "\"e1\""), "home")
        .plan();
    assert!(p.is_noop(), "{p:?}");
}

#[test]
fn a_deferred_counter_is_cleared_by_the_next_settle() {
    let t = task(1, "home", "steady");
    let mut world = World::new()
        .local(&t)
        .settled(&t, "\"e1\"")
        .remote(resource(&t, T0, "\"e1\""), "home");
    world.s.index.entries.get_mut(&uid(1)).unwrap().defer_count = 2;
    let p = world.plan();
    assert_eq!(
        p.settled.len(),
        1,
        "settling rewrites the entry with a zero counter"
    );
}

#[test]
fn snapshots_default_has_no_scope() {
    let s = Snapshots::default();
    assert!(s.remote.is_empty() && s.inbox_list.is_none());
    assert_eq!(s.tombstones, BTreeSet::new());
    assert_eq!(s.index, Index::default());
    assert_eq!(s.notes, BTreeMap::new());
}
