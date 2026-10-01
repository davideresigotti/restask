# restask spec — Domain Model & Timestamp Contract (§3–§4)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §3 Domain model (`crates/restask/src/domain/`)

Pure — no I/O. Public types derive `Debug, Clone, PartialEq` and serde where persisted.

### 3.1 `uid.rs`

```rust
pub struct TaskUid(String);   // "restask-" + 26-char lowercase Crockford base32 (ULID)

impl TaskUid {
    pub fn generate() -> Self;                         // monotonic within a process
    pub fn derived(foreign_uid: &str, created_at: Option<DateTime<Utc>>) -> Self;
    pub fn parse(raw: &str) -> Result<Self, UidError>; // trims, lowercases, validates
    pub fn as_str(&self) -> &str;
}
```

- A UID is filename-safe: it names the base snapshot `.restask/tasks/<uid>.ics` and the
  server resource `<uid>.ics`.
- `parse` also accepts the legacy prefix `taskres-`; such a UID is kept verbatim forever.
  `generate` and `derived` only produce `restask-`.
- `derived` is the adoption UID (§11 R5): a pure function of a foreign task's `UID` and
  `CREATED`. The ULID timestamp is `created_at` (epoch when unknown), so adopted tasks
  sort by creation; the 80 random bits are a hash of the foreign UID.

### 3.2 `priority.rs`

```rust
pub enum Priority { Highest, High, Medium, Low, Lowest }
```

| Priority | Emoji | Codepoint | VTODO `PRIORITY` |
|---|---|---|---|
| Highest | 🔺 | U+1F53A | 1 |
| High | ⏫ | U+23EB | 3 |
| Medium | 🔼 | U+1F53C | 5 |
| Low | 🔽 | U+1F53D | 7 |
| Lowest | ⏬ | U+23EC | 9 |

Reverse mapping (lossy for even values, rounding toward higher priority):
`1,2→Highest  3,4→High  5,6→Medium  7,8→Low  9→Lowest  0|absent→None`.
Accessors: `emoji`, `from_emoji`, `to_ical`, `from_ical`, `heading` (TODO.md section
name), `cli_name`, `from_cli_name`, `ALL`.

### 3.3 `dates.rs`

```rust
pub struct LocalDate(pub NaiveDate);          // device-local calendar date, "YYYY-MM-DD"
pub struct LocalDateTime(pub NaiveDateTime);  // device-local wall time, "YYYY-MM-DD HH:MM"
pub enum When { Date(LocalDate), DateTime(LocalDateTime) }

pub trait Clock: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
    fn today_local(&self) -> LocalDate;
    fn local_offset(&self) -> FixedOffset;
}
pub struct SystemClock;
```

Pure code never calls `Utc::now()` / `Local::now()`; time comes from a `Clock` or a
parameter (tests inject `FixedClock`).

### 3.4 `ListSlug`

`ListSlug::from_name("Home Lab") == "home-lab"`: lowercase; every run of
non-`[a-z0-9]` becomes one `-`; trimmed; must be non-empty. `display_name()` title-cases
the words back (`Home Lab`).

### 3.5 `task.rs`

```rust
pub enum Status { Active, Completed { on: LocalDate } }
pub struct SourceRef { pub path: String, pub line: usize }  // vault-relative, '/'-separated; 1-based

pub struct Task {
    pub uid: TaskUid,
    pub list: ListSlug,
    pub text: String,                  // one line; metadata tokens stripped
    pub status: Status,
    pub priority: Option<Priority>,
    pub due: Option<When>,
    pub start: Option<When>,
    pub scheduled: Option<When>,
    pub created: Option<LocalDate>,
    pub parent: Option<TaskUid>,
    pub source: SourceRef,
    pub source_heading: Option<String>,   // render only; not persisted to VTODO
    pub source_mtime: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,     // vault: file mtime; server: LAST-MODIFIED
}
```

`Task::thumbprint()` is FNV-1a (64-bit) over uid, list, text, status, priority, due,
start, scheduled, created, parent. It identifies a base snapshot (§9); it is **not** how
two versions are compared for syncing — the merge compares fields (§11.3).

The fields the merge manages ("sync fields") are `text`, `status`, `priority`, `due`,
`start`, `scheduled`.

## §4 Timestamp contract (Markdown ⇄ VTODO)

Markdown dates are **device-local**. VTODO instants (`CREATED`, `COMPLETED`,
`LAST-MODIFIED`, `DTSTAMP`) are **UTC with `Z`**.

| Markdown | VTODO | Rule |
|---|---|---|
| `➕ 2026-09-19` | `CREATED:20260919T000000Z` | date-only → midnight UTC |
| (no `➕`) | (no `CREATED`) | never synthesized: an invented date would differ from the vault forever |
| `✅ 2026-09-19` | `COMPLETED:20260919T000000Z` | date-only → midnight UTC |
| `📅 2026-09-19` | `DUE;VALUE=DATE:20260919` | all-day |
| `📅 2026-09-19 17:00` | `DUE:20260919T170000` | **floating** local time — no `Z`, no `TZID` |
| `🛫 …` / `⏳ …` | `DTSTART` / `X-RESTASK-SCHEDULED` | same rules as `DUE` |
| — (every write) | `DTSTAMP`, `LAST-MODIFIED` | the caller's `now`, never read from a clock by the codec |

Reading what other clients wrote:

| VTODO | Markdown | Rule |
|---|---|---|
| `COMPLETED:20260920T033000Z` | `✅ 2026-09-20` | UTC calendar date of the instant (stable round trip with the midnight-UTC rule) |
| `DUE:…Z`, `DUE;TZID=…:…` | `📅 <local wall time>` | converted to device-local wall time **with the offset valid on that date** (DST-correct; the client passes `chrono::Local`) |
| `DUE;TZID=<unknown zone>:…` | `📅 <as written>` | read as floating |
| `DUE;VALUE=DATE:…` | `📅 <date>` | stays date-only |
| seconds in a time | dropped | Markdown has minute precision |
| malformed optional value | absent | never an error (§8.2) |

Determinism: `to_vcalendar_with(task, now, extras)` with identical inputs yields
byte-identical output. Devices are assumed NTP-synced (±60 s); conflicts use a 120 s tie
window (§11.3).
