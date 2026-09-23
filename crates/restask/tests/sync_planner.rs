//! Planner conformance (§11.2): one test per rule row plus defer counting, the tie
//! window, list moves, and foreign adoption. Fixed instants only — no wall clock.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};

use restask::domain::{ListSlug, LocalDate, Priority, SourceRef, Status, Task, TaskUid, When};
use restask::markdown::mutator::{Mutation, WhenField};
use restask::store::index::{Index, IndexEntry};
use restask::sync::{plan, DeferReason, InsertTarget, MarkdownOp, Plan, Snapshots};
use restask::vtodo::RemoteTask;

/// Fixed base instant: 2023-11-14T22:13:20Z.
const BASE: i64 = 1_700_000_000;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(BASE + secs, 0).single().unwrap()
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

fn uid(n: u64) -> TaskUid {
    TaskUid::parse(&format!("taskres-01jz{:0>22}", n)).unwrap()
}

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
        created: None,
        parent: None,
        source: SourceRef {
            path: "notes/a.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: at(0),
        last_modified: at(0),
    }
}

/// A managed remote copy of `t`, keyed by its UID as the resource name.
fn remote_of(t: &Task) -> RemoteTask {
    RemoteTask {
        raw_uid: t.uid.as_str().to_string(),
        managed: true,
        task: t.clone(),
        source_path: None,
    }
}

fn remote_map(
    entries: Vec<(&str, &str, RemoteTask)>,
) -> BTreeMap<ListSlug, BTreeMap<String, RemoteTask>> {
    let mut map: BTreeMap<ListSlug, BTreeMap<String, RemoteTask>> = BTreeMap::new();
    for (list, name, remote) in entries {
        map.entry(slug(list))
            .or_default()
            .insert(name.to_string(), remote);
    }
    map
}

/// A realistic index entry for `t` (thumbprint matches, etag as given).
fn entry_for(t: &Task, etag: Option<&str>) -> IndexEntry {
    IndexEntry {
        uid: t.uid.clone(),
        list: t.list.clone(),
        source_path: t.source.path.clone(),
        thumbprint: t.thumbprint(),
        caldav_etag: etag.map(str::to_string),
        seen_at: at(0),
        defer_count: 0,
    }
}

fn snapshots(
    local: Vec<Task>,
    cache: Vec<Task>,
    remote: BTreeMap<ListSlug, BTreeMap<String, RemoteTask>>,
    tombstones: Vec<TaskUid>,
    entries: Vec<IndexEntry>,
) -> Snapshots {
    Snapshots {
        local: local.into_iter().map(|t| (t.uid.clone(), t)).collect(),
        cache: cache.into_iter().map(|t| (t.uid.clone(), t)).collect(),
        remote,
        tombstones: tombstones.into_iter().collect(),
        index: Index {
            entries: entries.into_iter().map(|e| (e.uid.clone(), e)).collect(),
        },
    }
}

fn plan_now(s: Snapshots) -> Plan {
    plan(s, at(1_000))
}

#[test]
fn empty_snapshots_yield_an_empty_plan() {
    let p = plan_now(snapshots(vec![], vec![], BTreeMap::new(), vec![], vec![]));
    assert_eq!(p, Plan::default());
}

// ── R0 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r0_tombstone_removes_the_task_everywhere() {
    let t = task(1, "home", "alpha");
    let s = snapshots(
        vec![t.clone()],
        vec![t.clone()],
        remote_map(vec![("home", t.uid.as_str(), remote_of(&t))]),
        vec![t.uid.clone()],
        vec![entry_for(&t, Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert_eq!(
        p.markdown_ops,
        vec![MarkdownOp::Delete {
            path: "notes/a.md".to_string(),
            uid: t.uid.clone(),
        }]
    );
    assert_eq!(p.cache_deletes, vec![t.uid.clone()]);
    assert_eq!(p.caldav_deletes.len(), 1);
    assert_eq!(p.caldav_deletes[0].list, slug("home"));
    assert_eq!(p.caldav_deletes[0].name, t.uid.as_str());
    assert_eq!(p.index_removals, vec![t.uid.clone()]);
    assert!(p.caldav_puts.is_empty() && p.caldav_moves.is_empty());
    assert!(p.cache_writes.is_empty() && p.index_upserts.is_empty());
    assert!(p.todo_refresh);
}

#[test]
fn r0_tombstone_purges_a_remote_only_copy() {
    let t = task(1, "home", "alpha");
    let s = snapshots(
        vec![],
        vec![],
        remote_map(vec![("home", t.uid.as_str(), remote_of(&t))]),
        vec![t.uid.clone()],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_deletes.len(), 1);
    assert_eq!(p.caldav_deletes[0].name, t.uid.as_str());
    assert!(p.markdown_ops.is_empty() && p.cache_deletes.is_empty());
    assert!(p.index_removals.is_empty() && p.index_upserts.is_empty());
}

// ── R1 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r1_defers_when_the_cache_is_newer_beyond_the_window() {
    let mut t = task(1, "home", "alpha");
    t.last_modified = at(0);
    let mut cached = t.clone();
    cached.text = "edited on the phone".to_string();
    cached.last_modified = at(1_500);
    let s = snapshots(vec![t], vec![cached], BTreeMap::new(), vec![], vec![]);
    let p = plan_now(s);
    assert_eq!(p.deferred, vec![(uid(1), DeferReason::CacheNewerThanVault)]);
    assert_eq!(p.index_upserts.len(), 1);
    assert_eq!(p.index_upserts[0].defer_count, 1);
    assert!(p.markdown_ops.is_empty());
    assert!(p.caldav_puts.is_empty() && p.cache_writes.is_empty());
    assert!(!p.todo_refresh);
}

#[test]
fn r1_within_the_window_does_not_defer() {
    let mut t = task(1, "home", "alpha");
    t.last_modified = at(0);
    let mut cached = t.clone();
    cached.text = "edited on the phone".to_string();
    cached.last_modified = at(120);
    let mut remote_t = t.clone();
    remote_t.last_modified = at(0);
    let s = snapshots(
        vec![t],
        vec![cached],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert!(p.deferred.is_empty());
    // Converged with the remote (R7); the diverged cache is rewritten from the vault.
    assert_eq!(
        p.cache_writes,
        vec![{
            let mut c = task(1, "home", "alpha");
            c.last_modified = at(0);
            c
        }]
    );
    assert!(p.caldav_puts.is_empty());
}

#[test]
fn r1_defer_counter_increments_and_resets_on_success() {
    let mut t = task(1, "home", "alpha");
    t.last_modified = at(0);
    let mut cached = t.clone();
    cached.text = "edited on the phone".to_string();
    cached.last_modified = at(5_000);
    let mut e = entry_for(&t, None);
    e.defer_count = 2;

    // Third consecutive defer cycle: counter reaches the error threshold.
    let s = snapshots(
        vec![t.clone()],
        vec![cached],
        BTreeMap::new(),
        vec![],
        vec![e.clone()],
    );
    let p = plan_now(s);
    assert_eq!(p.deferred.len(), 1);
    assert_eq!(p.index_upserts[0].defer_count, 3);

    // The vault caught up: a successful reconcile resets the counter to zero.
    let s = snapshots(
        vec![t.clone()],
        vec![t.clone()],
        BTreeMap::new(),
        vec![],
        vec![e],
    );
    let p = plan_now(s);
    assert!(p.deferred.is_empty());
    assert_eq!(p.index_upserts.len(), 1);
    assert_eq!(p.index_upserts[0].defer_count, 0);
}

// ── R2 / R3 ───────────────────────────────────────────────────────────────────────────

#[test]
fn r2_new_local_task_is_pushed() {
    let t = task(1, "home", "alpha");
    let s = snapshots(vec![t.clone()], vec![], BTreeMap::new(), vec![], vec![]);
    let p = plan_now(s);
    assert_eq!(p.caldav_puts, vec![t.clone()]);
    assert_eq!(p.cache_writes, vec![t.clone()]);
    assert_eq!(p.index_upserts.len(), 1);
    let e = &p.index_upserts[0];
    assert_eq!(e.uid, t.uid);
    assert_eq!(e.list, slug("home"));
    assert_eq!(e.source_path, "notes/a.md");
    assert_eq!(e.thumbprint, t.thumbprint());
    assert_eq!(e.caldav_etag, None);
    assert_eq!(e.defer_count, 0);
    assert!(p.markdown_ops.is_empty() && p.caldav_deletes.is_empty());
    assert!(!p.todo_refresh);
}

#[test]
fn r3_server_side_deletion_removes_the_vault_copy() {
    let t = task(1, "home", "alpha");
    let s = snapshots(
        vec![t.clone()],
        vec![],
        BTreeMap::new(),
        vec![],
        vec![entry_for(&t, Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert_eq!(
        p.markdown_ops,
        vec![MarkdownOp::Delete {
            path: "notes/a.md".to_string(),
            uid: t.uid.clone(),
        }]
    );
    assert_eq!(p.cache_deletes, vec![t.uid.clone()]);
    assert_eq!(p.index_removals, vec![t.uid.clone()]);
    assert!(p.caldav_puts.is_empty() && p.caldav_deletes.is_empty());
    assert!(p.todo_refresh);
}

// ── R4 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r4_remote_task_inserts_into_its_routed_note() {
    let anchor = task(1, "home", "anchor");
    let mut remote_t = task(2, "home", "from the server");
    remote_t.source.path = String::new();
    remote_t.source.line = 0;
    let mut rt = remote_of(&remote_t);
    rt.source_path = Some("notes/a.md".to_string());
    let s = snapshots(
        vec![anchor],
        vec![],
        remote_map(vec![("home", uid(2).as_str(), rt)]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.markdown_ops.len(), 1);
    match &p.markdown_ops[0] {
        MarkdownOp::Insert { task, target } => {
            assert_eq!(task.text, "from the server");
            assert_eq!(task.list, slug("home"));
            assert_eq!(task.source.path, "notes/a.md");
            assert_eq!(
                target,
                &InsertTarget::FileEnd {
                    path: "notes/a.md".to_string(),
                }
            );
        }
        other => panic!("expected Insert, got {other:?}"),
    }
    // The vault anchor task (local-only) rides along as an R2 push.
    let anchor = task(1, "home", "anchor");
    assert_eq!(p.caldav_puts, vec![anchor.clone()]);
    assert!(p.cache_writes.iter().any(|t| t.text == "from the server"));
    let inserted = p
        .index_upserts
        .iter()
        .find(|e| e.uid == uid(2))
        .expect("inserted task upserted");
    assert_eq!(inserted.source_path, "notes/a.md");
    assert!(p.todo_refresh);
}

#[test]
fn r4_subtask_places_under_a_known_parent_in_the_same_file() {
    let parent = task(1, "home", "parent");
    let mut remote_t = task(2, "home", "child");
    remote_t.parent = Some(parent.uid.clone());
    remote_t.source.path = String::new();
    let mut rt = remote_of(&remote_t);
    rt.source_path = Some("notes/a.md".to_string());
    let s = snapshots(
        vec![parent],
        vec![],
        remote_map(vec![("home", uid(2).as_str(), rt)]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    match &p.markdown_ops[0] {
        MarkdownOp::Insert { task, target } => {
            assert_eq!(task.parent, Some(uid(1)));
            assert_eq!(
                target,
                &InsertTarget::UnderParent {
                    path: "notes/a.md".to_string(),
                    after_uid: uid(1),
                    indent_chars: 0,
                }
            );
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn r4_orphan_subtask_drops_the_link_into_the_inbox() {
    let mut remote_t = task(2, "home", "orphan child");
    remote_t.parent = Some(uid(9));
    remote_t.source.path = String::new();
    let rt = remote_of(&remote_t);
    let s = snapshots(
        vec![],
        vec![],
        remote_map(vec![("home", uid(2).as_str(), rt)]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    match &p.markdown_ops[0] {
        MarkdownOp::Insert { task, target } => {
            assert_eq!(task.parent, None);
            assert_eq!(task.source.path, "");
            assert_eq!(target, &InsertTarget::TodoInbox);
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn r4_unrouted_source_falls_back_to_the_inbox() {
    let mut remote_t = task(2, "home", "lost note");
    remote_t.source.path = String::new();
    let mut rt = remote_of(&remote_t);
    rt.source_path = Some("notes/gone.md".to_string());
    let s = snapshots(
        vec![],
        vec![],
        remote_map(vec![("home", uid(2).as_str(), rt)]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    match &p.markdown_ops[0] {
        MarkdownOp::Insert { task, target } => {
            assert_eq!(task.source.path, "");
            assert_eq!(target, &InsertTarget::TodoInbox);
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

// ── R5 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r5_foreign_task_is_adopted_with_a_fresh_uid() {
    let mut foreign = task(0, "home", "made in Tasks.org");
    foreign.uid = TaskUid::parse("taskres-00000000000000000000000000").unwrap();
    foreign.source.path = String::new();
    let rt = RemoteTask {
        raw_uid: "1789@example.com".to_string(),
        managed: false,
        task: foreign,
        source_path: None,
    };
    let s = snapshots(
        vec![],
        vec![],
        remote_map(vec![("home", "1789@example.com", rt)]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.adoptions.len(), 1);
    assert_eq!(p.adoptions[0].collection, slug("home"));
    assert_eq!(p.adoptions[0].remote.raw_uid, "1789@example.com");
    assert_eq!(p.caldav_puts.len(), 1);
    let adopted = &p.caldav_puts[0];
    assert_ne!(
        adopted.uid,
        TaskUid::parse("taskres-00000000000000000000000000").unwrap()
    );
    assert!(adopted.uid.as_str().starts_with("taskres-"));
    assert_eq!(adopted.text, "made in Tasks.org");
    assert_eq!(adopted.list, slug("home"));
    assert_eq!(p.caldav_deletes.len(), 1);
    assert_eq!(p.caldav_deletes[0].list, slug("home"));
    assert_eq!(p.caldav_deletes[0].name, "1789@example.com");
    match &p.markdown_ops[0] {
        MarkdownOp::Insert { target, .. } => assert_eq!(target, &InsertTarget::TodoInbox),
        other => panic!("expected Insert, got {other:?}"),
    }
    assert_eq!(p.cache_writes.len(), 1);
    assert_eq!(p.index_upserts.len(), 1);
    assert_eq!(p.index_upserts[0].uid, adopted.uid);
    assert!(p.todo_refresh);
}

// ── R6 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r6_stale_cache_entry_is_deleted() {
    let t = task(1, "home", "alpha");
    let s = snapshots(
        vec![],
        vec![t.clone()],
        BTreeMap::new(),
        vec![],
        vec![entry_for(&t, None)],
    );
    let p = plan_now(s);
    assert_eq!(p.cache_deletes, vec![t.uid.clone()]);
    assert_eq!(p.index_removals, vec![t.uid.clone()]);
    assert!(p.caldav_puts.is_empty() && p.caldav_deletes.is_empty());
    assert!(p.markdown_ops.is_empty());
}

// ── R7 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r7_converged_task_is_a_no_op() {
    let t = task(1, "home", "alpha");
    let remote_t = task(1, "home", "alpha");
    let s = snapshots(
        vec![t.clone()],
        vec![t.clone()],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![entry_for(&t, Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert!(p.markdown_ops.is_empty());
    assert!(p.caldav_puts.is_empty() && p.caldav_moves.is_empty() && p.caldav_deletes.is_empty());
    assert!(p.cache_writes.is_empty() && p.cache_deletes.is_empty());
    assert!(p.index_upserts.is_empty() && p.index_removals.is_empty());
    assert!(p.deferred.is_empty());
    assert!(!p.todo_refresh);
}

#[test]
fn r7_rewrites_a_diverged_cache() {
    let t = task(1, "home", "alpha");
    let remote_t = task(1, "home", "alpha");
    let mut cached = t.clone();
    cached.text = "stale".to_string();
    let s = snapshots(
        vec![t.clone()],
        vec![cached],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![entry_for(&t, Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert_eq!(p.cache_writes, vec![t.clone()]);
    assert!(p.caldav_puts.is_empty() && p.markdown_ops.is_empty());
}

// ── R8 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r8_remote_wins_decomposes_all_fields() {
    let mut local = task(1, "home", "alpha");
    local.last_modified = at(0);
    let mut remote_t = task(1, "home", "beta");
    remote_t.status = Status::Completed {
        on: LocalDate::parse("2026-09-23").unwrap(),
    };
    remote_t.priority = Some(Priority::High);
    remote_t.due = Some(When::parse_date_or_datetime("2026-09-25").unwrap());
    remote_t.start = Some(When::parse_date_or_datetime("2026-09-24").unwrap());
    remote_t.scheduled = Some(When::parse_date_or_datetime("2026-09-26").unwrap());
    remote_t.last_modified = at(300);
    let s = snapshots(
        vec![local],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert!(p.caldav_puts.is_empty());
    assert_eq!(p.markdown_ops.len(), 1);
    match &p.markdown_ops[0] {
        MarkdownOp::Mutate { path, mutations } => {
            assert_eq!(path, "notes/a.md");
            assert_eq!(
                mutations,
                &vec![
                    Mutation::SetStatus {
                        uid: uid(1),
                        checked: true,
                        completed_on: Some(LocalDate::parse("2026-09-23").unwrap()),
                    },
                    Mutation::EditText {
                        uid: uid(1),
                        text: "beta".to_string(),
                    },
                    Mutation::SetPriority {
                        uid: uid(1),
                        priority: Some(Priority::High),
                    },
                    Mutation::SetWhen {
                        uid: uid(1),
                        field: WhenField::Due,
                        value: Some(When::parse_date_or_datetime("2026-09-25").unwrap()),
                    },
                    Mutation::SetWhen {
                        uid: uid(1),
                        field: WhenField::Start,
                        value: Some(When::parse_date_or_datetime("2026-09-24").unwrap()),
                    },
                    Mutation::SetWhen {
                        uid: uid(1),
                        field: WhenField::Scheduled,
                        value: Some(When::parse_date_or_datetime("2026-09-26").unwrap()),
                    },
                    Mutation::MoveToDone { uid: uid(1) },
                ]
            );
        }
        other => panic!("expected Mutate, got {other:?}"),
    }
    // The cache mirrors the remote-won content with local placement.
    assert_eq!(p.cache_writes.len(), 1);
    assert_eq!(p.cache_writes[0].text, "beta");
    assert_eq!(p.cache_writes[0].source.path, "notes/a.md");
    assert_eq!(p.index_upserts.len(), 1);
    assert_eq!(
        p.index_upserts[0].thumbprint,
        p.cache_writes[0].thumbprint()
    );
    assert!(p.todo_refresh);
}

#[test]
fn r8_remote_uncheck_restores_from_done() {
    let mut local = task(1, "home", "alpha");
    local.status = Status::Completed {
        on: LocalDate::parse("2026-09-20").unwrap(),
    };
    local.last_modified = at(0);
    let mut remote_t = task(1, "home", "alpha");
    remote_t.last_modified = at(500);
    let s = snapshots(
        vec![local],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    match &p.markdown_ops[0] {
        MarkdownOp::Mutate { mutations, .. } => assert_eq!(
            mutations,
            &vec![
                Mutation::RestoreFromDone { uid: uid(1) },
                Mutation::SetStatus {
                    uid: uid(1),
                    checked: false,
                    completed_on: None,
                },
            ]
        ),
        other => panic!("expected Mutate, got {other:?}"),
    }
}

#[test]
fn r8_local_wins_within_the_tie_window() {
    let mut local = task(1, "home", "alpha");
    local.last_modified = at(0);
    let mut remote_t = task(1, "home", "beta");
    remote_t.last_modified = at(120);
    let s = snapshots(
        vec![local.clone()],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_puts, vec![local.clone()]);
    assert!(p.markdown_ops.is_empty());
}

#[test]
fn r8_local_wins_when_the_remote_is_older() {
    let mut local = task(1, "home", "alpha");
    local.last_modified = at(0);
    let mut remote_t = task(1, "home", "beta");
    remote_t.last_modified = at(-5_000);
    let s = snapshots(
        vec![local.clone()],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_puts, vec![local.clone()]);
    assert!(p.markdown_ops.is_empty());
}

#[test]
fn r8_unrepresentable_diff_pushes_the_vault_copy() {
    let mut local = task(1, "home", "alpha");
    local.created = None;
    local.last_modified = at(0);
    let mut remote_t = task(1, "home", "alpha");
    remote_t.created = Some(LocalDate::parse("2026-09-01").unwrap());
    remote_t.last_modified = at(300);
    let s = snapshots(
        vec![local.clone()],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_puts, vec![local.clone()]);
    assert!(p.markdown_ops.is_empty());
}

#[test]
fn r8_mutations_group_into_one_pass_per_file() {
    let mut local1 = task(1, "home", "alpha");
    local1.last_modified = at(0);
    let mut local2 = task(2, "home", "gamma");
    local2.source.line = 5;
    local2.last_modified = at(0);
    let mut remote1 = task(1, "home", "alpha2");
    remote1.last_modified = at(300);
    let mut remote2 = task(2, "home", "gamma2");
    remote2.last_modified = at(400);
    let s = snapshots(
        vec![local1, local2],
        vec![],
        remote_map(vec![
            ("home", uid(1).as_str(), remote_of(&remote1)),
            ("home", uid(2).as_str(), remote_of(&remote2)),
        ]),
        vec![],
        vec![],
    );
    let p = plan_now(s);
    assert_eq!(p.markdown_ops.len(), 1);
    match &p.markdown_ops[0] {
        MarkdownOp::Mutate { path, mutations } => {
            assert_eq!(path, "notes/a.md");
            assert_eq!(mutations.len(), 2);
            assert_eq!(
                mutations[0],
                Mutation::EditText {
                    uid: uid(1),
                    text: "alpha2".to_string(),
                }
            );
            assert_eq!(
                mutations[1],
                Mutation::EditText {
                    uid: uid(2),
                    text: "gamma2".to_string(),
                }
            );
        }
        other => panic!("expected Mutate, got {other:?}"),
    }
}

// ── R9 ────────────────────────────────────────────────────────────────────────────────

#[test]
fn r9_moves_a_rerouted_task_between_collections() {
    let mut local = task(1, "work", "alpha");
    local.source.path = "notes/work.md".to_string();
    let home_copy = task(1, "home", "alpha");
    let s = snapshots(
        vec![local.clone()],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&home_copy))]),
        vec![],
        vec![entry_for(&task(1, "home", "alpha"), Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_moves.len(), 1);
    assert_eq!(p.caldav_moves[0].task, local);
    assert_eq!(p.caldav_moves[0].from, slug("home"));
    assert_eq!(p.caldav_moves[0].to, slug("work"));
    assert!(p.caldav_puts.is_empty() && p.caldav_deletes.is_empty());
    assert_eq!(p.index_upserts.len(), 1);
    assert_eq!(p.index_upserts[0].list, slug("work"));
    assert!(p.todo_refresh);
}

#[test]
fn r9_deletes_stray_duplicate_copies() {
    let t = task(1, "home", "alpha");
    let home_copy = task(1, "home", "alpha");
    let work_copy = task(1, "work", "alpha");
    let s = snapshots(
        vec![t.clone()],
        vec![],
        remote_map(vec![
            ("home", uid(1).as_str(), remote_of(&home_copy)),
            ("work", uid(1).as_str(), remote_of(&work_copy)),
        ]),
        vec![],
        vec![entry_for(&t, Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert!(p.caldav_moves.is_empty() && p.caldav_puts.is_empty());
    assert_eq!(p.caldav_deletes.len(), 1);
    assert_eq!(p.caldav_deletes[0].list, slug("work"));
    assert_eq!(p.caldav_deletes[0].name, t.uid.as_str());
    // Content converged: only the stray cleanup and the cache mirror happened.
    assert_eq!(p.cache_writes, vec![t]);
}

#[test]
fn r9_recovers_a_copy_stranded_in_the_wrong_collection() {
    let mut local = task(1, "work", "alpha");
    local.source.path = "notes/work.md".to_string();
    let home_copy = task(1, "home", "alpha");
    let s = snapshots(
        vec![local.clone()],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&home_copy))]),
        vec![],
        vec![entry_for(&task(1, "work", "alpha"), Some("\"e1\""))],
    );
    let p = plan_now(s);
    assert_eq!(p.caldav_moves.len(), 1);
    assert_eq!(p.caldav_moves[0].from, slug("home"));
    assert_eq!(p.caldav_moves[0].to, slug("work"));
    assert!(p.caldav_deletes.is_empty());
}

// ── R10 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn r10_todo_refresh_flags() {
    // A pure push changes no vault render.
    let t = task(1, "home", "alpha");
    let push = plan_now(snapshots(
        vec![t.clone()],
        vec![],
        BTreeMap::new(),
        vec![],
        vec![],
    ));
    assert!(!push.todo_refresh);
    // A markdown mutation does.
    let mut local = t.clone();
    local.last_modified = at(0);
    let mut remote_t = t.clone();
    remote_t.text = "beta".to_string();
    remote_t.last_modified = at(300);
    let mutate = plan_now(snapshots(
        vec![local],
        vec![],
        remote_map(vec![("home", uid(1).as_str(), remote_of(&remote_t))]),
        vec![],
        vec![],
    ));
    assert!(mutate.todo_refresh);
    // A list move does.
    let mut moved = task(2, "work", "alpha");
    moved.source.path = "notes/work.md".to_string();
    let home_copy = task(2, "home", "alpha");
    let move_plan = plan_now(snapshots(
        vec![moved],
        vec![],
        remote_map(vec![("home", uid(2).as_str(), remote_of(&home_copy))]),
        vec![],
        vec![entry_for(&task(2, "home", "alpha"), Some("\"e1\""))],
    ));
    assert!(move_plan.todo_refresh);
}
