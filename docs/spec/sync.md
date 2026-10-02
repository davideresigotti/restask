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
      then delete the old copy), deletes. A dependent delete runs only if its put
      succeeded.
3. **Record.**
   Re-render TODO.md from the vault as it is now (plus tasks the plan placed in the
   inbox), then persist the state: a base + index entry for every task that was settled
   or successfully written, defer counters, forgotten UIDs, tombstones.

If the server is unreachable, phase 1 and the render still happen and the error is
returned afterwards: registration, hand-edit repair and the TODO.md view work offline.

`Engine::settle` is that part on its own — phase 1, then the render from the scan —
with no request and no state written: the local work of invariant 12, as an editor
integration runs it (§13.3 `restask settle`, §16). The render has no plan to take inbox
tasks from, so a task the server holds and the vault does not yet is not shown by it;
the daemon's pass adds it.

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
    pub todo_lists: BTreeSet<ListSlug>,                    // further lists the inbox file shows (§7.5)
}

pub struct Plan {
    pub mutations: BTreeMap<String, Vec<Mutation>>,   // vault edits per file
    pub inbox_inserts: Vec<Task>,                     // server tasks landing in TODO.md
    pub puts: Vec<PutOp>,                             // { task, name, extras, wire, if_match }
    pub moves: Vec<MoveOp>,                           // { put, from }
    pub adopted: Vec<TaskUid>,                        // foreign tasks entering the vault (R5)
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

For each **sync field** (text, status, priority, repeat rule, due, start, scheduled):

| local vs remote | base | result |
|---|---|---|
| equal | any | that value |
| differ | equals local | **remote** — only the server changed it |
| differ | equals remote | **local** — only the vault changed it |
| differ | equals neither, or no base | **conflict**: remote wins iff `remote.LAST-MODIFIED − local.mtime > 120 s`; otherwise local |

Timestamps are consulted only in the last row. A file's mtime covers all its tasks and
moves on any unrelated edit; using it to decide whether *this* task changed loses data.

Not merged:

- `created`: the vault's `➕` when present, else the server's `CREATED` is kept. A line
  without `➕` therefore never changes the server's value, and the value never comes
  back as a token (§6.4).
- `parent`: the vault decides for an **active task in a note** (indentation). For done
  records and TODO.md lines — which cannot express nesting — the server's relation is
  kept as it is.
- `X-RESTASK-SOURCE` follows the vault.

Outputs: the merged task; the mutations that turn the vault line into it (a completion is
`SetStatus` + `MoveToDone`, a reopening `RestoreFromDone` + `SetStatus`, the rest
`EditText` / `SetPriority` / `SetRecurrence` / `SetWhen`); and whether the server copy must be replaced
(merged ≠ remote in any sync field, parent, creation date or source path). Both can
happen at once: fields changed on different sides are all kept.

### 11.4 Rule table (per UID over local ∪ base ∪ index ∪ remote)

A server copy of a UID is a resource whose `UID` is that restask UID, or a foreign
resource adopted under it (R5, below). The rules do not tell the two apart.

Task **not in the vault**:

| # | Condition | Action |
|---|---|---|
| — | its index entry's source note is unreadable | nothing (unknown) |
| **R0** | tombstoned | delete every server copy; forget state |
| **R6** | no server copy | forget state |
| **Dv** | known (index/base) and a server copy exists | deleted in the vault → delete server copies, tombstone, forget |
| **R4** | unknown and a server copy exists | created on the server → insert a line: in the note `X-RESTASK-SOURCE` names if it still routes to that list; else the list's home note; else the inbox (the inbox list, and a list of `todo_lists` without a note; its line then names the list, §7.5). Under its parent if the parent is an active task of the same note. The line carries no `➕` (§6.4); the settled task keeps the server's date. Settle. |

Task **in the vault**:

| # | Condition | Action |
|---|---|---|
| **R0′** | tombstoned | **revive**: clear the tombstone, continue below. The vault outranks tombstones. |
| — | its list was not listed this pass | nothing (unknown) |
| **R1** | valid base, base ≠ vault in a sync field, and the base is newer than the vault file by > 120 s, fewer than 3 consecutive deferrals | **defer**: a vault copy that file sync has not caught up yet. After 3 passes the vault is taken at its word. |
| **R7/R8** | a copy in its list | merge (§11.3). Vault mutations and/or a put (`If-Match` the listed etag, extras carried); if neither, settle when base/index are missing or stale. Other copies of the UID are strays → delete. |
| **R8r** | the task is completed in the vault, the server copy is open or absent, and the task has a repeat rule with a next occurrence (applies to R8, R9 and R2) | **roll forward** (§11.6) instead of completing the series |
| **R9** | copies only in other lists | **move**: merge with the first copy, put into the task's list (create), then delete the old copy; further copies are strays. |
| **R3** | no copy; settled; the collection it was settled in was listed and is not a *reset* | deleted on the server → delete the vault line, tombstone, forget |
| **R2** | no copy otherwise | new (or lost wholesale) → put (create). A task whose line states no creation date is put with `CREATED` = the day its UID was minted (§4) |

**Reset collections** (R3 guard): a collection created in this pass, or one where two or
more settled tasks all vanished at once, did not have its tasks deleted one by one — it
was emptied, recreated or restored. Its tasks are re-pushed (R2), never deleted from the
vault.

**R5 — foreign resources** (a `VTODO` whose `UID` is not a restask UID), in lists that
have a home in the vault. Such a task is **adopted where it is**: the resource stays the
one its client created — same name, same `UID`, never replaced by a copy — and counts as
the server copy of the adoption UID `U`:

1. the resource's `X-RESTASK-UID`, when it can have been derived from the resource's
   `UID` (`TaskUid::adopts`; a copy another client made under a new `UID`, properties
   included, is a task of its own);
2. else the UID the vault, the index or a tombstone already knows that `UID` by (a
   client may drop the property and rewrite `CREATED`);
3. else `derived(uid, created)` — the resource name stands in for a missing `UID`.

"A home" is the inbox file for the inbox list and the lists of `todo_lists`, the home
note for a routed list (§5.4). In a list without one, a resource is still the server
copy of `U` when `U` is a line in the vault — a task adopted while its calendar was
shown in TODO.md and taken out of `todo_lists` since: left unmatched, its line would
read as deleted on the server (R3). Nothing new is adopted there.

With that the table above applies as to any task:

| Situation | Rule | Action |
|---|---|---|
| `U` nowhere in the vault or the state | R4 | **adopt** (`task_adopted`): insert a line with UID `U`, settle what was read, and put the resource in place (`If-Match`) with `X-RESTASK-SOURCE` and `X-RESTASK-UID` |
| `U` in the vault | R7/R8 | merge; an edit made by the client that owns the task reaches the line, an edit of the line is put to that resource. A resource without the link (never written, or written back without it) is put again |
| `U` known, its line gone | Dv | deleted in the vault → the resource is deleted |
| `U` in the vault and settled, resource gone | R3 | deleted by its client → the line is deleted |
| `U` tombstoned and not in the vault | R0 | the resource is deleted (a client re-uploading from its cache) |
| a resource `U.ics` exists as well (left by versions that adopted by copy), or a second resource with the same foreign `UID` | R7/R4 | the extra copy is a stray → deleted; `U.ics` is the canonical one |

Every put of such a task writes the `UID` the resource came with (`PutOp::wire`, §8.1).
A parent relation between foreign tasks is preserved: in the vault the child hangs under
the parent's adopted UID (inserts are ordered parents-first), on the server the relation
keeps naming the parent by its own `UID` — also for a task made in the vault under an
adopted parent.

Why in place: a client keeps the task under the resource it created. Replacing that
resource by a copy under `U` (what earlier versions did) left the client holding a task
the vault was no longer tied to; its next edit went nowhere, or showed up twice.

### 11.6 Recurring tasks

A repeat rule comes from the vault (`🔁 every …`, §6.1) or from another client (`RRULE`).
A rule both can spell is a managed field of the task (§3.6) and merges like any other
sync field: set, changed or removed on either side, it reaches the other. A richer rule
stays on the server as unmanaged content and is not shown in the vault.

Either way, recurring clients complete one occurrence by moving the task to its next date
and leaving it open, and restask does the same when an occurrence is completed **in the
vault** — pushing `STATUS:COMPLETED` would end the whole series.

When a task is completed in the vault while the server copy is open (or does not exist
yet) and the task has a rule — its own, else an unmanaged one among the server's extras:

- **The series** keeps its UID, becomes active again, and is dated at the next occurrence:
  the first one after the occurrence just done that also lies after the completion date
  (a long-overdue task jumps to its next upcoming date, not to another overdue one). The
  occurrence is identified by the due date — else the start, else the scheduled date,
  else the completion day (the task then gets a due date). The other dates move by the
  same number of days; a time of day is kept. A `COUNT` is decremented (`for 5 times` →
  `for 4 times`; in an unmanaged rule, in place, the rest handed back as written).
- **The occurrence that was done** stays in the vault as what it is — the checked line
  under the done heading — and becomes a task of its own, without the rule: its UID is
  derived from the series UID and the occurrence date, and it is created on the server
  as an ordinary completed task.
- In the vault this is `Rekey` on the checked line (new UID, `🔁` removed) plus `Insert`
  of the series line at the bottom of the active list.

Not rolled, i.e. an ordinary completion: a task without a rule; the rule's last
occurrence (`for 1 time` / `COUNT=1`); a rule that has ended (`until` passed); a
completion made on the server side (that client decided to end the task).

The roll is decided by the planner, so it happens at the first pass that reaches the
server; until then the checked line simply waits under the done heading.

### 11.5 Failure and idempotence

- A failed put leaves no record: the next pass sees the same difference and plans the
  same write from *current* content. Nothing is replayed from a stored snapshot.
- A failed delete is re-derived too: by the tombstone (R0) or by the stray rule (R7/R9).
- A pass over a converged vault plans nothing, writes no file and sends no write request.
- Crash windows: after vault mutations, before state — the next pass finds vault and
  server equal (or merges) and settles. After a put, before state — same. After an
  adoption's line insert, before its put — the resource is linked by the next pass's
  merge (R5); a put the server refused is retried over what the adoption settled.
