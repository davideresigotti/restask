# restask spec — Reconciliation (§11)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §11 Reconciliation (`crates/restask/src/sync/`)

### 11.1 One pass (`engine::Engine::reconcile`)

A pass has three phases. They are ordered so that dying at any point leaves a state the
next pass repairs.

1. **Local** — needs no server, always runs.
   Scan and repair the routed notes (§6.4). Carry edits made on TODO.md mirror lines to
   their source notes (§7.1) and rescan if that changed anything.
2. **Remote.**
   a. Snapshot: one `REPORT` per list in scope (§5.4). A routed list without a collection
      is created if allowed, else left out of the snapshot.
   b. Plan: `planner::plan(&Snapshots) -> Plan` — pure.
   c. Apply vault mutations (one pass per file), then server writes: puts, moves (put,
      then delete the old copy), adoptions (put, then delete the foreign original),
      deletes. A dependent delete runs only if its put succeeded.
3. **Record.**
   Re-render TODO.md from the vault as it is now (plus tasks the plan placed in the
   inbox), then persist the state: a base + index entry for every task that was settled
   or successfully written, defer counters, forgotten UIDs, tombstones.

If the server is unreachable, phase 1 and the render still happen and the error is
returned afterwards: registration, hand-edit repair and the TODO.md view work offline.

The vault is written before the state that describes it. So the state can lag the vault
(the next pass then finds both sides equal and settles) but never claim a line the vault
does not have.

### 11.2 Snapshots and plan

```rust
pub struct Snapshots {
    pub local: BTreeMap<TaskUid, Task>,                    // vault scan
    pub base: BTreeMap<TaskUid, Task>,                     // .restask/tasks (validated against the index)
    pub remote: BTreeMap<ListSlug, Vec<RemoteResource>>,   // ONLY collections listed this pass
    pub created: BTreeSet<ListSlug>,                       // collections created this pass
    pub tombstones: BTreeSet<TaskUid>,
    pub index: Index,
    pub notes: BTreeMap<String, ListSlug>,                 // every routed note → list
    pub homes: BTreeMap<ListSlug, String>,                 // list → home note (§5.4)
    pub unreadable: BTreeSet<String>,                      // routed files not readable this pass
    pub inbox_file: String,
    pub inbox_list: Option<ListSlug>,
}

pub struct Plan {
    pub mutations: BTreeMap<String, Vec<Mutation>>,   // vault edits per file
    pub inbox_inserts: Vec<Task>,                     // server tasks landing in TODO.md
    pub puts: Vec<PutOp>,                             // { task, name, extras, if_match }
    pub moves: Vec<MoveOp>,                           // { put, from }
    pub adoptions: Vec<AdoptOp>,                      // { put, foreign }
    pub deletes: Vec<DeleteOp>,                       // { list, name, etag }
    pub settled: Vec<Settled>,                        // { task, etag }: base refresh, no push
    pub forgets: Vec<TaskUid>,                        // drop base + index entry
    pub tombstones: Vec<TaskUid>,
    pub revived: Vec<TaskUid>,                        // tombstones to clear
    pub deferred: Vec<(TaskUid, DeferReason)>,
}
```

The planner is deterministic: no clock, no randomness (adoption UIDs are derived, §3.1).
Two readings that the snapshots keep apart on purpose: a list missing from `remote` is
**unknown**, not empty; a path in `unreadable` holds **unknown** tasks, not deleted ones.

### 11.3 The merge (`merge::merge`)

For a task present in the vault and in its collection, with `base` its settled content (or
none):

For each **sync field** (text, status, priority, due, start, scheduled):

| local vs remote | base | result |
|---|---|---|
| equal | any | that value |
| differ | equals local | **remote** — only the server changed it |
| differ | equals remote | **local** — only the vault changed it |
| differ | equals neither, or no base | **conflict**: remote wins iff `remote.LAST-MODIFIED − local.mtime > 120 s`; otherwise local |

Timestamps are consulted only in the last row. A file's mtime covers all its tasks and
moves on any unrelated edit; using it to decide whether *this* task changed loses data.

Not merged:

- `created`: the vault's `➕` when present, else the server's `CREATED` is kept.
- `parent`: the vault decides for an **active task in a note** (indentation). For done
  records and TODO.md lines — which cannot express nesting — the server's relation is
  kept as it is.
- `X-RESTASK-SOURCE` follows the vault.

Outputs: the merged task; the mutations that turn the vault line into it (a completion is
`SetStatus` + `MoveToDone`, a reopening `RestoreFromDone` + `SetStatus`, the rest
`EditText` / `SetPriority` / `SetWhen`); and whether the server copy must be replaced
(merged ≠ remote in any sync field, parent, creation date or source path). Both can
happen at once: fields changed on different sides are all kept.

### 11.4 Rule table (per UID over local ∪ base ∪ index ∪ managed remote)

Task **not in the vault**:

| # | Condition | Action |
|---|---|---|
| — | its index entry's source note is unreadable | nothing (unknown) |
| **R0** | tombstoned | delete every server copy; forget state |
| **R6** | no server copy | forget state |
| **Dv** | known (index/base) and a server copy exists | deleted in the vault → delete server copies, tombstone, forget |
| **R4** | unknown and a server copy exists | created on the server → insert a line: in the note `X-RESTASK-SOURCE` names if it still routes to that list; else the list's home note; else the inbox. Under its parent if the parent is an active task of the same note. Settle. |

Task **in the vault**:

| # | Condition | Action |
|---|---|---|
| **R0′** | tombstoned | **revive**: clear the tombstone, continue below. The vault outranks tombstones. |
| — | its list was not listed this pass | nothing (unknown) |
| **R1** | valid base, base ≠ vault in a sync field, and the base is newer than the vault file by > 120 s, fewer than 3 consecutive deferrals | **defer**: a vault copy that file sync has not caught up yet. After 3 passes the vault is taken at its word. |
| **R7/R8** | a copy in its list | merge (§11.3). Vault mutations and/or a put (`If-Match` the listed etag, extras carried); if neither, settle when base/index are missing or stale. Other copies of the UID are strays → delete. |
| **R8r** | as R8, the merge says *completed*, the server copy is still open and carries an `RRULE` with a next occurrence | **roll forward** (§11.6) instead of completing the series |
| **R9** | copies only in other lists | **move**: merge with the first copy, put into the task's list (create), then delete the old copy; further copies are strays. |
| — | no copy, an adoption for this UID is in flight | handled by R5 |
| **R3** | no copy; settled; the collection it was settled in was listed and is not a *reset* | deleted on the server → delete the vault line, tombstone, forget |
| **R2** | no copy otherwise | new (or lost wholesale) → put (create) |

**Reset collections** (R3 guard): a collection created in this pass, or one where two or
more settled tasks all vanished at once, did not have its tasks deleted one by one — it
was emptied, recreated or restored. Its tasks are re-pushed (R2), never deleted from the
vault.

Foreign resources (after the UID loop), in lists that have a home in the vault:

| # | Condition | Action |
|---|---|---|
| **R5** | foreign `VTODO`, adopted UID `U = derived(uid, created)` not in vault, not on server, not tombstoned | **adopt**: insert a line with UID `U` (placement as R4); put `U` carrying the foreign extras; when the put succeeded, delete the foreign resource |
| | `U` in the vault, not on the server | resume: put the vault's `U` with the foreign extras, then delete the original |
| | `U` already on the server | delete the foreign original only |
| | `U` tombstoned and not in the vault | delete the foreign original |
| | a second resource with the same foreign UID | delete it |

A parent relation between foreign tasks is preserved: the child's parent becomes the
parent's adopted UID, and inserts are ordered parents-first.

### 11.6 Recurring tasks

restask does not author recurrence; a rule is set in another client (Tasks.org) and
travels with the task as unmanaged content (`RRULE` among the extras, §8.2). Recurring
clients complete one occurrence by moving the task to its next date and leaving it open.
restask does the same when an occurrence is completed **in the vault** — pushing
`STATUS:COMPLETED` would end the whole series.

When the merge yields a completed task while the server copy is open and has a rule
(`vtodo::Recurrence`):

- **The series** keeps its UID, becomes active again, and is dated at the next occurrence:
  the first one after the occurrence just done that also lies after the completion date
  (a long-overdue task jumps to its next upcoming date, not to another overdue one). The
  occurrence is identified by the due date — else the start, else the scheduled date,
  else the completion day (the task then gets a due date). The other dates move by the
  same number of days; a time of day is kept. It is put with its extras; a `COUNT` in the
  rule is decremented.
- **The occurrence that was done** stays in the vault as what it is — the checked line
  under the done heading — and becomes a task of its own: its UID is derived from the
  series UID and the occurrence date (`Rekey`), and it is created on the server as an
  ordinary completed task, without the rule.
- In the vault this is `Rekey` on the checked line plus `Insert` of the series line at
  the bottom of the active list.

Not rolled, i.e. an ordinary completion: a task without a rule; the rule's last
occurrence (`COUNT=1`); a rule that has ended (`UNTIL` passed); a completion made on the
server side (that client decided to end the task).

Supported rule parts: `FREQ` (`MINUTELY` … `YEARLY`), `INTERVAL`, `BYDAY` (weekly lists;
monthly with or without ordinals such as `2TU`, `-1FR`), `BYMONTHDAY` (negative values
count from the month's end), `COUNT`, `UNTIL`. Months or years lacking the anchor's day
have no occurrence (RFC 5545). Other parts are ignored for the computation and kept in
the rule as written.

### 11.5 Failure and idempotence

- A failed put leaves no record: the next pass sees the same difference and plans the
  same write from *current* content. Nothing is replayed from a stored snapshot.
- A failed delete is re-derived too: by the tombstone (R0), by the stray rule (R7/R9), or
  by the leftover foreign original (R5).
- A pass over a converged vault plans nothing, writes no file and sends no write request.
- Crash windows: after vault mutations, before state — the next pass finds vault and
  server equal (or merges) and settles. After a put, before state — same. After an
  adoption's line insert, before its put — resumed (R5).
