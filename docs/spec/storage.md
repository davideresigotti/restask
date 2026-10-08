# restask spec — Storage & State (§9)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §9 `.restask/` (per vault)

```
.restask/
├── index.json           uid → { list, source_path, thumbprint, caldav_etag, seen_at, defer_count }
├── tasks/<uid>.ics      base snapshots: what vault and server last agreed on
├── tombstones.json      uid → deletion instant (pruned after 365 days)
├── todo.rendered.md     the engine's own last render of the inbox file (§7.1)
├── views/<digest>.md    its last render of the view each root note holds (§7.6); the
│                        name is the digest (§7.2) of the note's path
├── calendars.json       the server's collections seen so far (§7.5) and the lists found
│                        under another path than their own (§5.4); the sync node's
└── lock                 advisory lock: one restask process per vault per machine
```

The directory lives inside the vault and **rides the file sync**: the plugin and
`restask settle` on the editing machines read it (the remembered render, the base
snapshots), and a machine that takes over as the sync node starts from the same
knowledge. It is written by the one daemon (§1.1) — and, for the remembered render, by
`settle`. It is never scanned for tasks, whatever
`ignore` says. All of it is disposable: `restask rebuild` removes the index, the snapshots
and the remembered renders (tombstones stay), and the next pass re-derives them. A
remembered render of a view is dropped by the pass that finds its root note gone, or
without a view.

Every file is written atomically through `fsio` (hidden temp sibling + fsync + rename; a
leftover temp file from a crash is overwritten, never an obstacle) and **only when its
content changes**, so a pass over a converged vault touches nothing — no mtime churn, no
file-sync traffic, no watcher echo.

### 9.1 Index and base snapshots

```rust
pub struct IndexEntry {
    pub uid: TaskUid, pub list: ListSlug, pub source_path: String,
    pub thumbprint: u64, pub caldav_etag: Option<String>,
    pub seen_at: DateTime<Utc>, pub defer_count: u8,
}
pub fn cache_read<Z: TimeZone>(dir, uid, tz: &Z) -> Option<Task>;
pub fn cache_write(dir, task, now_utc) -> Result<(), RestaskError>;
pub fn cache_remove(dir, uid) -> Result<(), RestaskError>;
```

An entry with `caldav_etag: Some(_)` means *settled*: at `seen_at` the vault line and the
server resource held the content stored in `tasks/<uid>.ics`. That snapshot is the **base**
of the three-way merge (§11.3). Two things follow:

- The base is valid only if the index vouches for it: `entry.thumbprint` must equal the
  snapshot's thumbprint. A snapshot something else rewrote is ignored (the merge then
  treats every difference as a conflict), never trusted.
- Entry and snapshot are written only after the thing they describe is true: after the
  server confirmed a write, or after both sides were found equal. A failed write leaves
  the previous base in place.

The snapshot carries no list or source path; both come from the entry. `LAST-MODIFIED` of
a snapshot is the instant it was written (§11 R1 uses it).

### 9.2 Tombstones

```rust
impl Tombstones {
    pub fn insert(&mut self, uid, at); pub fn contains(&self, uid) -> bool;
    pub fn remove(&mut self, uid) -> bool; pub fn uids(&self) -> impl Iterator<Item = &TaskUid>;
    pub fn prune(&mut self, older_than: Duration, now: DateTime<Utc>);
}
```

A tombstone records that a task was deleted (in the vault or on the server). Its job is
narrow: a **server copy** of a tombstoned UID that shows up again — a delete that failed,
a client re-uploading from its offline cache — is deleted, not pulled back into the vault.
It never acts against the vault: a tombstoned UID that is present in the vault is alive,
and its tombstone is cleared (§11 R0).

### 9.3 The lock

Each engine entry point (`reconcile`, `add`, `set_done`) takes an exclusive advisory lock
on `.restask/lock` and holds it for the pass. A second process waits (up to 60 s). It
serializes the daemon and CLI commands on one machine; devices are coordinated by the
merge, not by the lock.
