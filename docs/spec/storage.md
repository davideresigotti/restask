# Taskres Spec — Storage & State — .taskres/ (§9)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §9 Storage & State (`.taskres/`, per-vault)

`.taskres/` lives inside the vault and **is synced by Syncthing** (the offline mobile cache rides the vault). It is fully reconstructible via `restask rebuild`. `*.sync-conflict*` files are ignored for indexing; `doctor` reports them.

```
.taskres/
├── tasks/<uid>.ics        # VTODO cache (same bytes as pushed to Radicale)
├── index.json             # uid → routing/etag bookkeeping
├── tombstones.json        # deleted UIDs with timestamps (pruned after 365 days)
├── outbox.json            # CalDAV ops parked after retry exhaustion
└── restaskd.log           # JSON-lines log (daemon only; never synced secrets)
```

### 9.1 `store/index.rs`

```rust
pub struct IndexEntry {
    pub uid: TaskUid, pub list: ListSlug, pub source_path: String,
    pub thumbprint: u64, pub caldav_etag: Option<String>,
    pub seen_at: chrono::DateTime<chrono::Utc>, pub defer_count: u8,
}
pub struct Index { pub entries: std::collections::BTreeMap<TaskUid, IndexEntry> }
impl Index {
    pub fn load(dir: &Path) -> Result<Index, TaskresError>;      // missing file → empty
    pub fn save(&self, dir: &Path) -> Result<(), TaskresError>;  // atomic tmp+rename
    pub fn get(&self, uid: &TaskUid) -> Option<&IndexEntry>;
    pub fn upsert(&mut self, e: IndexEntry);
    pub fn remove(&mut self, uid: &TaskUid);
}
```

`caldav_etag: Some(_)` means "was on the server" — the flag that distinguishes *new local task* (push) from *server-side deletion* (tombstone locally).

### 9.2 `store/tombstones.rs`, `store/cache.rs`, `store/outbox.rs`

```rust
pub struct Tombstones(std::collections::BTreeMap<TaskUid, chrono::DateTime<chrono::Utc>>);
impl Tombstones {
    pub fn load(dir: &Path) -> Result<Self, TaskresError>;
    pub fn save(&self, dir: &Path) -> Result<(), TaskresError>;
    pub fn insert(&mut self, uid: TaskUid, at: chrono::DateTime<chrono::Utc>);
    pub fn contains(&self, uid: &TaskUid) -> bool;
    pub fn prune(&mut self, older_than: chrono::Duration);
}

pub fn cache_path(dir: &Path, uid: &TaskUid) -> std::path::PathBuf;  // .taskres/tasks/<uid>.ics
pub fn cache_read(dir: &Path, uid: &TaskUid, tz: chrono::FixedOffset) -> Option<Task>;
pub fn cache_write(dir: &Path, task: &Task, now_utc: chrono::DateTime<chrono::Utc>) -> Result<(), TaskresError>;
pub fn cache_remove(dir: &Path, uid: &TaskUid) -> Result<(), TaskresError>;

pub enum OutboundOp { Put { task: Task }, Delete { uid: TaskUid, list: ListSlug, etag: Option<String> } }
pub struct Outbox { queue: std::collections::VecDeque<OutboundOp> }
impl Outbox {
    pub fn load(dir: &Path) -> Result<Self, TaskresError>;
    pub fn save(&self, dir: &Path) -> Result<(), TaskresError>;
    pub fn push(&mut self, op: OutboundOp);
    pub fn take_all(&mut self) -> Vec<OutboundOp>;
}
```

Crash safety: state files are saved atomically **once per reconcile cycle**; a crash mid-cycle loses nothing because the next cycle re-derives everything from vault + Radicale.
