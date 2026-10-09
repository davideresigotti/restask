# restask spec — Reconciliation (§11)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §11 Reconciliation (`crates/restask/src/sync/`)

### 11.1 One pass (`engine::Engine::reconcile`)

A pass has three phases. They are ordered so that dying at any point leaves a state the
next pass repairs.

1. **Local** — needs no server, always runs.
   Scan and repair the routed notes (§6.4), minting what UIDs it takes from this
   device's counter — long ones in a vault that is not switched to counted UIDs (§3.1,
   §9.4). Carry edits made on mirror lines — of TODO.md (§7.1) and of the views root
   notes hold (§7.6) — to their source notes, and rescan if that changed anything.
2. **Remote.**
   a. Snapshot: one `REPORT` per list in scope, at the collection that is the list's
      (§5.4: its own path, else by name from one listing per pass that needs it). A
      routed list no calendar answers to is created if allowed, else left out of the
      snapshot; so is a list two calendars answer to.
   b. With the server's answer in hand, on the machine that syncs the vault: switch the
      vault to counted UIDs if no device has (§9.4), and give the tasks that still
      carry a long UID a counted one (§11.7).
   c. Plan: `planner::plan(&Snapshots) -> Plan` — pure.
   d. Record the wire names the plan learnt (§9.5), then apply vault mutations (one
      pass per file), then server writes: puts, moves (put, then delete the old copy),
      deletes. A dependent delete runs only if its put succeeded.
3. **Record.**
   Re-render TODO.md from the vault as it is now (plus tasks the plan placed in the
   inbox), then the view of every root note that holds one (§7.6), then persist the
   state: a base + index entry for every task that was settled
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
    pub wires: Wires,                                      // names tasks go by on the server (§9.5)
    pub ids: Option<Counter>,                              // this device's counter (§3.1)
    pub today: Option<LocalDate>,                          // the local day of the pass (R2)
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
    pub wires: Vec<(String, TaskUid)>,                // wire names learnt in this pass (§9.5)
    pub minted: u64,                                  // highest number taken from `ids`; 0: none
}

pub fn renumbering(local: impl Iterator<Item = &TaskUid>, wires: &Wires, ids: &Counter)
    -> BTreeMap<TaskUid, TaskUid>;                    // long UID → counted UID (§11.7)
```

The planner is deterministic: no clock, no randomness. The UIDs it mints — for a task
another client created (R5), for the record of a recurring task's occurrence (§11.6) —
are the next numbers of `ids`, counted on a copy that has seen every UID of the
snapshots: the same snapshots give the same plan. Without `ids` such a task gets a long
UID derived from what it is (§3.1), as before the counters.
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

The remote text compared is the one the codec read (§8.4: the vault spelling from
`X-RESTASK-TEXT`). A resource another client wrote back without that property holds the
title only; its text is first relinked against the base (else the local text), so a
client that drops the property never takes wikilinks out of a line (`relink`), and the
property is written again.

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
happen at once: fields changed on different sides are all kept. The planner also puts a
copy that does not *show* the merged task as a push would (§8.4: its `SUMMARY` and its
`X-RESTASK-TEXT`) — a resource written before its text had links, before the vault named
its Obsidian vault, or still holding the text of a link removed since, is written once.

### 11.4 Rule table (per UID over local ∪ base ∪ index ∪ remote)

A server copy of a UID is a resource whose `UID` is that restask UID, or a resource
under another `UID` that is the task's: a foreign resource adopted under it (R5,
below), or one that keeps the long UID the task had before it was renumbered (§11.7).
The rules do not tell them apart.

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
| **R2** | no copy otherwise | new (or lost wholesale) → put (create). A task whose line states no creation date is put with `CREATED` = the day of the pass — for a long UID, the day it was minted (§4) |

**Reset collections** (R3 guard): a collection created in this pass, a list found at
another collection than in the pass before (§5.4), or one where two or
more settled tasks all vanished at once, did not have its tasks deleted one by one — it
was emptied, recreated or restored. Its tasks are re-pushed (R2), never deleted from the
vault.

**R5 — foreign resources** (a `VTODO` whose `UID` is not a restask UID), in lists that
have a home in the vault. Such a task is **adopted where it is**: the resource stays the
one its client created — same name, same `UID`, never replaced by a copy — and counts as
the server copy of the task `U`:

1. the resource's `X-RESTASK-UID`, when that link is bound to the resource's `UID`: by
   `X-RESTASK-OF` naming that `UID` (§8.1) — or, for a link to a long UID, by the UID
   having been derived from it (`TaskUid::adopts`). A copy another client made under a
   new `UID`, properties included, is bound to the old one: a task of its own;
2. else the UID the state knows that `UID` by (`wires.json`, §9.5): a client may drop
   the properties, and the pass that adopted the task may have died before it wrote
   them;
3. else a long UID the vault, the index or a tombstone knows that was derived from that
   `UID` — a task adopted before the counters (and the UID it was renumbered to since,
   §11.7);
4. else the next UID of this device's counter. The resource name stands in for a
   missing `UID`.

A UID found by 1, 3 or 4 is recorded with the `UID` it was found for (`Plan::wires`),
before the task's line is written.

"A home" is the inbox file for the inbox list and the lists of `todo_lists`, the home
note for a routed list (§5.4). In a list without one, a resource is still the server
copy of `U` when `U` is a line in the vault — a task adopted while its calendar was
shown in TODO.md and taken out of `todo_lists` since: left unmatched, its line would
read as deleted on the server (R3). Nothing new is adopted there.

With that the table above applies as to any task:

| Situation | Rule | Action |
|---|---|---|
| `U` nowhere in the vault or the state | R4 | **adopt** (`task_adopted`): insert a line with UID `U`, settle what was read, and put the resource in place (`If-Match`) with `X-RESTASK-SOURCE`, `X-RESTASK-UID` and `X-RESTASK-OF` |
| `U` in the vault | R7/R8 | merge; an edit made by the client that owns the task reaches the line, an edit of the line is put to that resource. A resource without the link, or without its binding (never written, or written back without it), is put again |
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
  the next of the sync node's counter (§3.1), and it is created on the server as an
  ordinary completed task.
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
  adoption's line insert, before its put — the name of the resource was recorded
  before the line was written (§9.5), so the next pass knows the line as that
  resource's task and links it by its merge (R5); a put the server refused is retried
  over what the adoption settled. After the name was recorded, before the line — the
  next pass adopts the task under the UID it was given.

### 11.7 Renumbering long UIDs

Before the counters a UID was `restask-<ULID>` (§3.1), 34 characters behind the `🆔` of
every line. The sync node gives every task of the vault that still carries such a UID
a counted one, once — in the pass that has listed the server (§11.1 step 2b: only the
machine that really syncs the vault renumbers it), before it plans, and with no
request of its own:

1. **Which.** `planner::renumbering`: every long UID of a task of the vault (the scan's
   `local`), in the order of creation, gets the next number of the node's counter — or
   the UID it was given before, when `wires.json` knows one for it (a pass that died,
   an old copy of a note that a file sync brought back).
2. **The names first.** long UID → counted UID is recorded in `wires.json` (§9.5) and
   saved before a line is touched.
3. **The lines.** `mutator::renumber` rewrites the UID token — and nothing else, not a
   byte — on every task line that carries one of those UIDs: in the routed notes, in
   TODO.md and the views of root notes (their mirror lines), and in the engine's
   remembered renders (§9), so that what the user edited in a view is still told from
   the render it was edited from. A note the scan read that cannot be rewritten ends
   the pass before it plans: a plan made while a line still carries the UID its task
   no longer has would take line and task for two. The scan itself is not made again —
   its tasks are the same lines under their new UIDs (`Scan::renumbered`), so nothing
   is read differently than before.
4. **The state.** Index entry and base snapshot move from the long UID to the counted
   one — the snapshot's own UID and its parent rewritten, its `LAST-MODIFIED` kept, the
   thumbprint taken anew when the index vouched for the old one — so the three-way
   merge keeps its base. This step is read off `wires.json`, not off the pass: one that
   died between 3 and 4 is finished by the next. Tombstones stay under the long UIDs
   they were made for.

**The server is not asked, and nothing there is created or deleted.** A resource keeps
its name and its `UID`: the long UID becomes its wire name, like the `UID` of a task
another client created (R5). The remote phase of the same pass finds it by that name
(`wires.json`; once it is linked, by the link), merges, and — the resource does not say
which task it is yet — puts it in place (`If-Match`) with `X-RESTASK-UID` and
`X-RESTASK-OF` (§8.1). A parent relation keeps naming the parent by the parent's
`UID` on the server. Other clients see one edit of each task and no new task.

A long UID that is **not** on a line of the vault is not renumbered: a task deleted in
the vault is still deleted on the server under it, a tombstoned one is still purged
(R0), and a resource of restask's own that the vault does not know is pulled under its
long UID (R4) and renumbered by the next pass. A task adopted before the counters has a
long, derived UID on its line: it is renumbered like any other, and its resource — its
client's — gets the new link.

Renumbering is the sync node's alone (the one writer of the state, §1.1): an editing
machine and the plugin read long UIDs as they always did and leave them where they are.
They must run a version that reads counted UIDs before the node renumbers: an older
`restask settle` takes `🆔 a42` for text and registers the line a second time. The
other way round nothing can go wrong: a device of this version mints long UIDs for as
long as its vault's sync node has not switched the vault (§9.4), so it can be updated
any time before the node — and a vault whose node is never updated stays as it was.
