//! Reconciliation planner (§11): a pure three-way decision function over the vault, cache,
//! and remote snapshots. Zero I/O; time enters only as the `now` parameter.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::domain::{ListSlug, Status, Task, TaskUid};
use crate::markdown::mutator::{Mutation, WhenField};
use crate::store::index::{Index, IndexEntry};
use crate::vtodo::RemoteTask;

/// Conflict tie window in seconds (§11.2 R1/R8): a cache strictly newer than the vault by
/// more than this defers the UID; content duels within it resolve LOCAL (vault authority).
const TIE_WINDOW_SECS: i64 = 120;

/// Consecutive defer cycles (R1) after which the planner logs a hard `vault_divergence`.
const DEFER_ERROR_THRESHOLD: u8 = 3;

/// Everything one reconciliation pass needs (§11.1).
#[derive(Debug, Default, PartialEq)]
pub struct Snapshots {
    /// Tasks parsed from the vault scan.
    pub local: BTreeMap<TaskUid, Task>,
    /// Tasks parsed from `.taskres/tasks` (the engine overwrites `list` from the index).
    pub cache: BTreeMap<TaskUid, Task>,
    /// Remote VTODOs per bound/managed collection, keyed by resource name (sans `.ics`).
    pub remote: BTreeMap<ListSlug, BTreeMap<String, RemoteTask>>,
    /// UIDs deleted on some device; never resurrected (R0).
    pub tombstones: BTreeSet<TaskUid>,
    /// Routing/etag bookkeeping (§9.1).
    pub index: Index,
}

/// The mutation plan for one reconciliation pass (§11.1).
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Vault edits; all mutations for one file are grouped into a single [`MarkdownOp::Mutate`].
    pub markdown_ops: Vec<MarkdownOp>,
    /// `true` when TODO.md must be re-rendered (R10).
    pub todo_refresh: bool,
    /// Tasks to serialize and `PUT` (new or locally-won content).
    pub caldav_puts: Vec<Task>,
    /// List moves: `PUT` to `to` + `DELETE` from `from`, UID preserved (R9).
    pub caldav_moves: Vec<MoveOp>,
    /// Remote resources to delete (tombstones, strays, replaced foreign tasks).
    pub caldav_deletes: Vec<DeleteOp>,
    /// Foreign resources adopted into the system (R5).
    pub adoptions: Vec<AdoptOp>,
    /// Cache files to (re)write.
    pub cache_writes: Vec<Task>,
    /// Cache files to remove.
    pub cache_deletes: Vec<TaskUid>,
    /// Index entries to upsert.
    pub index_upserts: Vec<IndexEntry>,
    /// Index entries to remove.
    pub index_removals: Vec<TaskUid>,
    /// UIDs whose reconciliation was postponed, with the reason.
    pub deferred: Vec<(TaskUid, DeferReason)>,
}

/// A vault-side operation (§11.1).
// The normative §11.1 shape carries `task: Task` by value; the size skew between variants
// is irrelevant at plan scale (dozens of ops per cycle).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownOp {
    /// Inserts a new task line (remote-created or adopted task).
    Insert {
        /// The task to insert (UID already assigned).
        task: Task,
        /// Where the line goes.
        target: InsertTarget,
    },
    /// Applies every queued mutation for one file in a single pass (§6.3).
    Mutate {
        /// Vault-relative path of the file.
        path: String,
        /// All mutations for the file, in deterministic order.
        mutations: Vec<Mutation>,
    },
    /// Removes the task line entirely.
    Delete {
        /// Vault-relative path of the file.
        path: String,
        /// The task to remove.
        uid: TaskUid,
    },
}

/// Where a [`MarkdownOp::Insert`] places its line (§11.2 R4/R5).
#[derive(Debug, Clone, PartialEq)]
pub enum InsertTarget {
    /// The TODO.md inbox section.
    TodoInbox,
    /// Appends a mirror line at the end of the routed note.
    FileEnd {
        /// Vault-relative path of the note.
        path: String,
    },
    /// Inserts the line directly after its parent task.
    UnderParent {
        /// Vault-relative path of the note.
        path: String,
        /// The parent task UID; the engine re-locates the line and its indent.
        after_uid: TaskUid,
        /// Indentation of the inserted line; `0` = the engine resolves it from the file.
        indent_chars: usize,
    },
}

/// A list move (§11.2 R9).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveOp {
    /// The task (with `list` already set to `to`).
    pub task: Task,
    /// The collection the resource currently lives in.
    pub from: ListSlug,
    /// The collection the task resolves to.
    pub to: ListSlug,
}

/// A remote resource deletion (§11.2).
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOp {
    /// The collection holding the resource.
    pub list: ListSlug,
    /// Resource name (sans `.ics`).
    pub name: String,
    /// ETag for `If-Match`; `None` = the engine fills it from its scan.
    pub etag: Option<String>,
}

/// A foreign resource awaiting adoption (§11.2 R5); the work itself is carried by the
/// `markdown_ops`/`caldav_puts`/`caldav_deletes` entries emitted alongside it.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptOp {
    /// The original parsed foreign task (placeholder UID).
    pub remote: RemoteTask,
    /// The collection the foreign resource was found in.
    pub collection: ListSlug,
}

/// Why a UID's reconciliation was postponed (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// The cache is newer than the vault by more than the tie window (in-flight write).
    CacheNewerThanVault,
}

/// Computes the reconciliation plan (§11.1). PURE: same inputs → same plan, except that
/// adoptions (R5) mint a fresh UID by design.
pub fn plan(s: Snapshots, now: DateTime<Utc>) -> Plan {
    let mut p = Plan::default();
    let mut mutations: BTreeMap<String, Vec<Mutation>> = BTreeMap::new();
    let mut adopted: BTreeSet<String> = BTreeSet::new();

    let mut uids: BTreeSet<TaskUid> = BTreeSet::new();
    uids.extend(s.local.keys().cloned());
    uids.extend(s.cache.keys().cloned());
    for collection in s.remote.values() {
        for remote in collection.values() {
            if remote.managed {
                uids.insert(remote.task.uid.clone());
            }
        }
    }

    for uid in uids {
        let local = s.local.get(&uid);
        let cache = s.cache.get(&uid);
        let entry = s.index.get(&uid);

        // R0 — tombstoned UIDs are purged everywhere and never resurrected.
        if s.tombstones.contains(&uid) {
            purge_tombstoned(&mut p, &s, &uid, local, cache, entry);
            continue;
        }

        let Some(local) = local else {
            let copies = remote_copies(&s, &uid);
            if copies.is_empty() {
                // R6 — stale cache entry (and dead bookkeeping for it).
                if cache.is_some() {
                    p.cache_deletes.push(uid.clone());
                    if entry.is_some() {
                        p.index_removals.push(uid.clone());
                    }
                }
            } else {
                reconcile_remote_only(&mut p, &s, &uid, copies, entry, now);
            }
            continue;
        };

        // R1 — in-flight Syncthing write from a mobile plugin: the cache is newer than the
        // vault by more than the tie window. Defer until the vault catches up.
        if let Some(cache) = cache {
            let drift = (cache.last_modified - local.last_modified).num_seconds();
            if cache.thumbprint() != local.thumbprint() && drift > TIE_WINDOW_SECS {
                let mut next = entry
                    .cloned()
                    .unwrap_or_else(|| upsert_entry(&uid, local, None, now));
                next.defer_count = next.defer_count.saturating_add(1);
                next.seen_at = now;
                if next.defer_count >= DEFER_ERROR_THRESHOLD {
                    tracing::error!(uid = %uid, defer_count = next.defer_count, "vault_divergence");
                }
                p.index_upserts.push(next);
                p.deferred
                    .push((uid.clone(), DeferReason::CacheNewerThanVault));
                continue;
            }
        }

        let copies = remote_copies(&s, &uid);
        let at_list = copies
            .iter()
            .find(|(slug, _, _)| *slug == &local.list)
            .map(|(_, _, remote)| *remote);
        let strays: Vec<(&ListSlug, &str)> = copies
            .iter()
            .filter(|(slug, _, _)| *slug != &local.list)
            .map(|(slug, name, _)| (*slug, *name))
            .collect();

        if let Some(remote_task) = at_list {
            if !strays.is_empty() {
                for (slug, name) in &strays {
                    p.caldav_deletes.push(DeleteOp {
                        list: (*slug).clone(),
                        name: (*name).to_string(),
                        etag: None,
                    });
                }
                tracing::info!(uid = %uid, count = strays.len(), "task_moved");
            }
            if remote_task.task.thumbprint() == local.thumbprint() {
                // R7 — converged; refresh bookkeeping and mirror the cache if needed.
                if cache_missing_or_differs(cache, local) {
                    p.cache_writes.push(local.clone());
                }
                if bookkeeping_stale(entry, local) {
                    p.index_upserts.push(upsert_entry(&uid, local, entry, now));
                }
            } else {
                // R8 — LAST-MODIFIED duel.
                let drift = local
                    .last_modified
                    .signed_duration_since(remote_task.task.last_modified)
                    .num_seconds();
                let true_divergence = cache.is_some_and(|c| {
                    c.thumbprint() != local.thumbprint()
                        && c.thumbprint() != remote_task.task.thumbprint()
                });
                if true_divergence {
                    tracing::warn!(uid = %uid, "conflict_resolved");
                }
                let remote_wins = drift < -TIE_WINDOW_SECS;
                let mut ms = if remote_wins {
                    decompose(&uid, local, &remote_task.task)
                } else {
                    Vec::new()
                };
                if remote_wins && !ms.is_empty() {
                    let merged = merge_remote(&remote_task.task, local);
                    mutations
                        .entry(local.source.path.clone())
                        .or_default()
                        .append(&mut ms);
                    p.cache_writes.push(merged.clone());
                    p.index_upserts
                        .push(upsert_entry(&uid, &merged, entry, now));
                } else {
                    // Local wins (vault authority), or the remote-won diff only touches
                    // fields the grammar cannot represent: push the vault copy instead.
                    p.caldav_puts.push(local.clone());
                    if cache_missing_or_differs(cache, local) {
                        p.cache_writes.push(local.clone());
                    }
                    p.index_upserts.push(upsert_entry(&uid, local, entry, now));
                }
            }
        } else if copies.is_empty() {
            let was_pushed = entry.is_some_and(|e| e.caldav_etag.is_some());
            if was_pushed {
                // R3 — server-side deletion: remove the vault copy; the engine records the
                // tombstone when it executes this plan.
                p.markdown_ops.push(MarkdownOp::Delete {
                    path: local.source.path.clone(),
                    uid: uid.clone(),
                });
                p.cache_deletes.push(uid.clone());
                p.index_removals.push(uid.clone());
            } else {
                // R2 — new local task: push, cache, and remember it.
                p.caldav_puts.push(local.clone());
                if cache_missing_or_differs(cache, local) {
                    p.cache_writes.push(local.clone());
                }
                p.index_upserts.push(upsert_entry(&uid, local, entry, now));
            }
        } else {
            // R9 — the UID lives on the server but not in the resolved list: move it there
            // and delete every misplaced copy.
            let (from, _, _) = copies[0];
            p.caldav_moves.push(MoveOp {
                task: local.clone(),
                from: from.clone(),
                to: local.list.clone(),
            });
            for (slug, name, _) in &copies[1..] {
                p.caldav_deletes.push(DeleteOp {
                    list: (*slug).clone(),
                    name: (*name).to_string(),
                    etag: None,
                });
            }
            p.index_upserts.push(upsert_entry(&uid, local, entry, now));
            tracing::info!(uid = %uid, from = %from.as_str(), to = %local.list.as_str(), "task_moved");
        }
    }

    adopt_foreign(&mut p, &s, now, &mut adopted);

    for (path, ms) in mutations {
        if !ms.is_empty() {
            p.markdown_ops.push(MarkdownOp::Mutate {
                path,
                mutations: ms,
            });
        }
    }
    // R10 — any markdown op or routed task set change re-renders TODO.md.
    p.todo_refresh = !p.markdown_ops.is_empty() || !p.caldav_moves.is_empty();
    p
}

/// R0 — deletes a tombstoned UID from the vault, the cache, and every remote collection.
fn purge_tombstoned(
    p: &mut Plan,
    s: &Snapshots,
    uid: &TaskUid,
    local: Option<&Task>,
    cache: Option<&Task>,
    entry: Option<&IndexEntry>,
) {
    if let Some(task) = local {
        p.markdown_ops.push(MarkdownOp::Delete {
            path: task.source.path.clone(),
            uid: uid.clone(),
        });
    }
    if cache.is_some() {
        p.cache_deletes.push(uid.clone());
    }
    for (slug, name, _) in remote_copies(s, uid) {
        p.caldav_deletes.push(DeleteOp {
            list: slug.clone(),
            name: name.to_string(),
            etag: None,
        });
    }
    if entry.is_some() {
        p.index_removals.push(uid.clone());
    }
}

/// R4 — inserts a remote-only managed task into the vault from its canonical copy and
/// deletes duplicate resources in other collections.
fn reconcile_remote_only(
    p: &mut Plan,
    s: &Snapshots,
    uid: &TaskUid,
    copies: Vec<(&ListSlug, &str, &RemoteTask)>,
    entry: Option<&IndexEntry>,
    now: DateTime<Utc>,
) {
    let origin = copies
        .iter()
        .position(|(slug, _, _)| entry.is_some_and(|e| *slug == &e.list))
        .unwrap_or(0);
    let (_, _, origin_remote) = copies[origin];
    let mut task = origin_remote.task.clone();

    let candidate = origin_remote.source_path.clone().or_else(|| {
        entry
            .map(|e| e.source_path.clone())
            .filter(|p| !p.is_empty())
    });
    let routed = candidate.filter(|path| note_is_routed(s, path, &task.list));
    let target = match (routed, task.parent.clone()) {
        (Some(path), Some(parent)) if parent_in_file(s, &parent, &path) => {
            task.source.path = path.clone();
            InsertTarget::UnderParent {
                path,
                after_uid: parent,
                indent_chars: 0,
            }
        }
        (Some(path), _) => {
            task.source.path = path;
            InsertTarget::FileEnd {
                path: task.source.path.clone(),
            }
        }
        (None, Some(parent)) => {
            task.parent = None;
            task.source.path = String::new();
            tracing::warn!(uid = %uid, parent = %parent, "orphan_subtask");
            InsertTarget::TodoInbox
        }
        (None, None) => {
            task.source.path = String::new();
            InsertTarget::TodoInbox
        }
    };
    task.source.line = 0;
    task.source_heading = None;

    p.markdown_ops.push(MarkdownOp::Insert {
        task: task.clone(),
        target,
    });
    if cache_missing_or_differs(s.cache.get(uid), &task) {
        p.cache_writes.push(task.clone());
    }
    p.index_upserts.push(upsert_entry(uid, &task, entry, now));
    for (index, (slug, name, _)) in copies.iter().enumerate() {
        if index == origin {
            continue;
        }
        p.caldav_deletes.push(DeleteOp {
            list: (*slug).clone(),
            name: (*name).to_string(),
            etag: None,
        });
    }
}

/// R5 — adopts foreign VTODOs in managed collections: fresh UID, routed insert, `PUT` of
/// the new UID resource, deletion of the foreign resource.
fn adopt_foreign(p: &mut Plan, s: &Snapshots, now: DateTime<Utc>, adopted: &mut BTreeSet<String>) {
    for (slug, collection) in &s.remote {
        for (name, remote) in collection {
            if remote.managed {
                continue;
            }
            if !adopted.insert(remote.raw_uid.clone()) {
                // Duplicate resource for an already-adopted foreign task.
                p.caldav_deletes.push(DeleteOp {
                    list: slug.clone(),
                    name: name.clone(),
                    etag: None,
                });
                continue;
            }
            let mut task = remote.task.clone();
            task.uid = TaskUid::generate();
            task.list = slug.clone();
            let routed = remote
                .source_path
                .clone()
                .filter(|path| note_is_routed(s, path, slug));
            let target = match routed {
                Some(path) => {
                    task.source.path = path;
                    InsertTarget::FileEnd {
                        path: task.source.path.clone(),
                    }
                }
                None => {
                    task.source.path = String::new();
                    InsertTarget::TodoInbox
                }
            };
            task.source.line = 0;
            task.source_heading = None;
            p.markdown_ops.push(MarkdownOp::Insert {
                task: task.clone(),
                target,
            });
            p.caldav_puts.push(task.clone());
            p.caldav_deletes.push(DeleteOp {
                list: slug.clone(),
                name: name.clone(),
                etag: None,
            });
            p.adoptions.push(AdoptOp {
                remote: remote.clone(),
                collection: slug.clone(),
            });
            p.cache_writes.push(task.clone());
            p.index_upserts
                .push(upsert_entry(&task.uid, &task, None, now));
            tracing::info!(uid = %task.uid, collection = %slug.as_str(), "task_adopted");
        }
    }
}

/// R8 remote-wins field decomposition (§11.3). Returns the mutations for the vault line;
/// empty when the two copies differ only in fields the grammar cannot represent.
fn decompose(uid: &TaskUid, local: &Task, remote: &Task) -> Vec<Mutation> {
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    match (remote.status, local.status) {
        (Status::Completed { on }, Status::Active) => {
            prefix.push(Mutation::SetStatus {
                uid: uid.clone(),
                checked: true,
                completed_on: Some(on),
            });
            suffix.push(Mutation::MoveToDone { uid: uid.clone() });
        }
        (Status::Active, Status::Completed { .. }) => {
            prefix.push(Mutation::RestoreFromDone { uid: uid.clone() });
            prefix.push(Mutation::SetStatus {
                uid: uid.clone(),
                checked: false,
                completed_on: None,
            });
        }
        _ => {}
    }
    let mut ms = prefix;
    if remote.text != local.text {
        ms.push(Mutation::EditText {
            uid: uid.clone(),
            text: remote.text.clone(),
        });
    }
    if remote.priority != local.priority {
        ms.push(Mutation::SetPriority {
            uid: uid.clone(),
            priority: remote.priority,
        });
    }
    if remote.due != local.due {
        ms.push(Mutation::SetWhen {
            uid: uid.clone(),
            field: WhenField::Due,
            value: remote.due,
        });
    }
    if remote.start != local.start {
        ms.push(Mutation::SetWhen {
            uid: uid.clone(),
            field: WhenField::Start,
            value: remote.start,
        });
    }
    if remote.scheduled != local.scheduled {
        ms.push(Mutation::SetWhen {
            uid: uid.clone(),
            field: WhenField::Scheduled,
            value: remote.scheduled,
        });
    }
    ms.extend(suffix);
    ms
}

/// Merges a remote-won task with the local source references (§11.3): remote content,
/// local placement.
fn merge_remote(remote: &Task, local: &Task) -> Task {
    let mut merged = remote.clone();
    merged.list = local.list.clone();
    merged.source = local.source.clone();
    merged.source_heading = local.source_heading.clone();
    merged.source_mtime = local.source_mtime;
    merged.last_modified = remote.last_modified;
    merged
}

/// Collections holding a managed UID, as `(slug, resource name, parsed task)`, in
/// collection order (lexicographic by slug).
fn remote_copies<'a>(
    s: &'a Snapshots,
    uid: &TaskUid,
) -> Vec<(&'a ListSlug, &'a str, &'a RemoteTask)> {
    let mut found = Vec::new();
    for (slug, collection) in &s.remote {
        for (name, remote) in collection {
            if remote.managed && remote.task.uid == *uid {
                found.push((slug, name.as_str(), remote));
            }
        }
    }
    found
}

/// `true` when the note at `path` currently holds routed tasks for `list` — the planner's
/// evidence that the note exists and routes there.
fn note_is_routed(s: &Snapshots, path: &str, list: &ListSlug) -> bool {
    s.local
        .values()
        .any(|t| t.source.path == path && t.list == *list)
}

/// `true` when `parent` is a local task in the same note file.
fn parent_in_file(s: &Snapshots, parent: &TaskUid, path: &str) -> bool {
    s.local.get(parent).is_some_and(|t| t.source.path == path)
}

/// `true` when the cache entry is missing or its content differs from `task`.
fn cache_missing_or_differs(cache: Option<&Task>, task: &Task) -> bool {
    cache.is_none_or(|c| c.thumbprint() != task.thumbprint())
}

/// `true` when the index entry is missing or lags behind the local task (including a
/// non-zero defer counter, which any successful reconcile resets).
fn bookkeeping_stale(entry: Option<&IndexEntry>, task: &Task) -> bool {
    entry.is_none_or(|e| {
        e.list != task.list
            || e.source_path != task.source.path
            || e.thumbprint != task.thumbprint()
            || e.defer_count != 0
    })
}

/// Builds a fresh index entry for `task`, keeping any known etag and resetting the defer
/// counter (a successful reconcile).
fn upsert_entry(
    uid: &TaskUid,
    task: &Task,
    entry: Option<&IndexEntry>,
    now: DateTime<Utc>,
) -> IndexEntry {
    IndexEntry {
        uid: uid.clone(),
        list: task.list.clone(),
        source_path: task.source.path.clone(),
        thumbprint: task.thumbprint(),
        caldav_etag: entry.and_then(|e| e.caldav_etag.clone()),
        seen_at: now,
        defer_count: 0,
    }
}
