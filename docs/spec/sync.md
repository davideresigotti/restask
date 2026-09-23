# Taskres Spec — Reconciliation Algorithm (§11)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §11 Reconciliation Algorithm (`src/sync/`)

### 11.1 Snapshots & plan

```rust
pub struct Snapshots {
    pub local: std::collections::BTreeMap<TaskUid, Task>,        // from vault scan
    pub cache: std::collections::BTreeMap<TaskUid, Task>,        // from .taskres/tasks
    pub remote: std::collections::BTreeMap<ListSlug, std::collections::BTreeMap<String, RemoteTask>>, // per bound/managed collection
    pub tombstones: std::collections::BTreeSet<TaskUid>,
    pub index: Index,
}

pub struct Plan {
    pub markdown_ops: Vec<MarkdownOp>, pub todo_refresh: bool,
    pub caldav_puts: Vec<Task>,
    pub caldav_moves: Vec<MoveOp>,           // { task, from: ListSlug, to: ListSlug }
    pub caldav_deletes: Vec<DeleteOp>,       // { list, name, etag }
    pub adoptions: Vec<AdoptOp>,             // { remote: RemoteTask, collection: ListSlug }
    pub cache_writes: Vec<Task>, pub cache_deletes: Vec<TaskUid>,
    pub index_upserts: Vec<IndexEntry>, pub index_removals: Vec<TaskUid>,
    pub deferred: Vec<(TaskUid, DeferReason)>,
}
pub enum MarkdownOp {
    Insert { task: Task, target: InsertTarget },
    Mutate { path: String, mutations: Vec<Mutation> },   // ALL mutations for one file applied in ONE pass
    Delete { path: String, uid: TaskUid },
}
pub enum InsertTarget { TodoInbox, FileEnd { path: String }, UnderParent { path: String, after_uid: TaskUid, indent_chars: usize } }
pub enum DeferReason { CacheNewerThanVault }

/// PURE. The rule table below is normative; tests cover every row.
pub fn plan(s: Snapshots, now: chrono::DateTime<chrono::Utc>) -> Plan;
```

### 11.2 Rule table (evaluation order; per UID over `local ∪ cache ∪ remote`)

| # | Condition | Action |
|---|---|---|
| **R0** | UID ∈ tombstones | Delete from markdown, cache, and every remote collection where present; never resurrect. |
| **R1** | `tp(cache) != tp(local)` AND `cache.last_modified > local.last_modified + 120s` | **Defer** (in-flight Syncthing write from a mobile plugin); `defer_count++`; ≥ 3 consecutive cycles → log `error` `vault_divergence`. Reset counter on any successful reconcile of the UID. |
| **R2** | local only; `index.caldav_etag == None` | New local task → `caldav_put` + cache write + index upsert. |
| **R3** | local only; `index.caldav_etag == Some(_)` | Server-side deletion → markdown `Delete` + tombstone + cache delete + index removal. |
| **R4** | remote only (managed UID) | Remote-created/renamed task → markdown `Insert`. Routing: `X-TASKRES-SOURCE` if that note exists and is routed to the same list; else TODO Inbox. Subtask with known parent in same file → `UnderParent`; unknown parent → TodoInbox, drop link, log `orphan_subtask`. Cache write + index upsert. |
| **R5** | remote only (foreign UID) in a **managed/bound** collection | **Adopt**: generate `TaskUid`, insert into the bound note (X-TASKRES-SOURCE if routed; else list's root note if any; else TodoInbox), `caldav_put` new UID, `caldav_delete` foreign resource. Log `task_adopted`. |
| **R6** | cache only | Stale cache → cache delete. |
| **R7** | local + remote, `tp(local) == tp(remote)` | No-op; refresh `index.caldav_etag`; cache write only if cache differs. |
| **R8** | local + remote, differ | LAST-MODIFIED duel: `|Δ| > 120s` → newer wins (remote wins ⇒ markdown `Mutate` incl. done-region placement; local wins ⇒ `caldav_put`). `|Δ| ≤ 120s` → **local wins** (vault authority). Log `conflict_resolved` when cache differs from both (true divergence). |
| **R9** | resolved `local.list != index.list` (or UID found in a second collection) | **List move**: `caldav_put` to new collection + `caldav_delete` from old (UID preserved). Stray duplicate copies in wrong collections are deleted. Log `task_moved`. |
| **R10** | any markdown op OR routed task set changed | Re-render TODO.md (§7). |

### 11.3 Remote-wins field decomposition (R8)

`STATUS:COMPLETED` vs local Active → `SetStatus{checked:true}` + `MoveToDone` with `✅` date from `COMPLETED` (§4 reverse rule). Active vs local Completed → `RestoreFromDone` + `SetStatus{checked:false}`. Text → `EditText`. Priority → `SetPriority`. DUE/DTSTART/X-TASKRES-SCHEDULED → `SetWhen` per field. Known trade-off (v1): conflict resolution is task-level, not field-level; the loser's text edit is overwritten and the event is logged.
