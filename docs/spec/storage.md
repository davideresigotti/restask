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
├── wires.json           wire name → uid: what tasks go by on the server where that is
│                        not their UID (§9.5); the sync node's
├── devices/<tag>        the claim of the device that mints UIDs under <tag> (§9.4)
└── lock                 advisory lock: one restask process per vault per machine
```

The directory lives inside the vault and **rides the file sync**: the plugin and
`restask settle` on the editing machines read it (the remembered render, the base
snapshots), and a machine that takes over as the sync node starts from the same
knowledge. It is written by the one daemon (§1.1) — and, for the remembered render, by
`settle`; a claim under `devices/` is written by the device it is of. It is never
scanned for tasks, whatever `ignore` says. The index, the snapshots and the remembered
renders are disposable: `restask rebuild` removes them (tombstones, wire names and
claims stay), and the next pass re-derives them. A
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

### 9.4 Devices

A UID is a device's tag and its next number (§3.1), so every device that registers
tasks in a vault — the sync node, an editing machine's `restask settle`, the plugin on
a phone — needs a tag no other device of that vault has, and has to get it without
asking anyone: devices are offline.

- **The identity** of a device in a vault is its tag, a secret of its own (random, never
  shown), and the last number it used. It is kept where no file sync carries it: by the
  engine in the file `device` beside the vault's machine config (§14.2: `<tag> <secret>
  <last>`), by the plugin in the vault's local storage in Obsidian
  (`store::Device`, `MachineConfig::device_file`; the plugin's `claimDevice`).
- **The claim** is `.restask/devices/<tag>`, holding the secret. It rides the file sync:
  every device sees which tags are taken.
- **Switching a vault.** A vault mints counted UIDs once some device holds a claim, and
  the first claim is the sync node's: it is made by the pass that has the server's
  answer in hand (`Engine::switch`, §11.1), which is the machine that really syncs the
  vault and so the one that can promise that counted UIDs are read there. Until then
  every device — the node in its local phase, an editing machine, the plugin — mints
  long UIDs as before (`Ids::Long`, the plugin's `uidGenerator`), and writes nothing
  about devices: a sync node of an earlier version reads every line they make. Updating
  the devices of a vault before its sync node is therefore safe, and the other order
  is not (§11.7). A long UID made after the switch — a device that had not seen the
  claim yet — is renumbered by the node like any other.
- **Taking a tag.** A device without an identity draws its tag at random among the
  shortest tags that no claim holds — a conflict copy of a claim (`a.sync-conflict-…`)
  is a claim — and that no UID of the state carries (index, tombstones, wire names: the
  claims may have been deleted with the state directory). One letter while one is free,
  then two. It writes its identity, then its claim.
- **Keeping it.** Before a device mints — at every pass of the engine; in the plugin
  when it loads and whenever a note is opened — it reads its claim. Its own secret:
  the tag is its. No file: it writes the claim again. Another secret: two devices took
  the tag before they saw each other and the file sync kept the other's claim; the
  device takes a new tag (its count starts again) and leaves the claim alone. The UIDs
  both minted under the lost tag in between are the one case in which two tasks can
  share a UID; on lines of the vault that is a copied line, which gets its own (§6.4).
- **Numbers.** The device counts on from the last number it remembers and from the
  highest it sees under its tag — the engine on every line of the vault and in the
  state, the plugin in the state — and remembers each number as it is handed out.
- Until the plugin knows that its claim holds — the first moments after it loads — the
  UIDs it makes are long ones.

### 9.5 Wire names

```rust
impl Wires {                                   // .restask/wires.json: name → uid
    pub fn get(&self, name: &str) -> Option<&TaskUid>;
    pub fn insert(&mut self, name: String, uid: TaskUid) -> bool;   // true: news
    pub fn uids(&self) -> impl Iterator<Item = &TaskUid>;
}
```

A resource whose `UID` is not its task's UID — a task another client created (§11 R5),
a task that had a long UID before it was renumbered (§11.7) — is that task by a link the
resource carries (`X-RESTASK-UID`, `X-RESTASK-OF`, §8.1). `wires.json` is the same
knowledge on the vault's side: the resource's `UID` → the task's UID, and for a
renumbered task also its long UID → its counted one.

It exists for the moments the link does not: the pass that gives a task its UID writes
the name **before** the line (a pass that dies in between is taken up under the same
UID, not a second one), and a client may write a resource back without the properties.
A name is recorded once and kept; its UID counts as used for the device that minted it.
The file is not disposable the way the index is — `restask rebuild` leaves it — but a
resource that carries its link needs no entry: lost, the names are read off the links
again.

### 9.3 The lock

Each engine entry point (`reconcile`, `add`, `set_done`) takes an exclusive advisory lock
on `.restask/lock` and holds it for the pass. A second process waits (up to 60 s). It
serializes the daemon and CLI commands on one machine; devices are coordinated by the
merge, not by the lock.
