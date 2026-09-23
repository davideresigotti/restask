# Taskres Spec — Domain Model & Timestamp Contract (§3–§4)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §3 Domain Model (`crates/restask/src/domain/`)

All public types derive `Debug, Clone, PartialEq, Serialize, Deserialize` unless noted. Pure — no I/O.

### 3.1 `uid.rs`

```rust
pub struct TaskUid(String);          // "taskres-" + 26-char lowercase Crockford base32 (ULID)

#[derive(Debug, thiserror::Error)]
#[error("invalid task UID: {0}")]
pub struct UidError(pub String);

impl TaskUid {
    pub fn generate() -> Self;                       // ulid::Ulid::new(), lowercase, monotonic in-process
    pub fn as_str(&self) -> &str;
    pub fn parse(raw: &str) -> Result<Self, UidError>; // trims, lowercases, validates ULID body
}
impl std::fmt::Display for TaskUid;                  // writes self.0
```

Filename-safe: `taskres-01jz…` is used verbatim as `.taskres/tasks/<uid>.ics` and as the Radicale resource name `<uid>.ics`.

### 3.2 `priority.rs`

```rust
pub enum Priority { Highest, High, Medium, Low, Lowest }

impl Priority {
    pub const ALL: [Priority; 5];
    pub fn emoji(self) -> &'static str;       // 🔺 ⏫ 🔼 🔽 ⏬
    pub fn from_emoji(s: &str) -> Option<Self>;
    pub fn to_ical(self) -> u8;               // 1 3 5 7 9
    pub fn from_ical(v: u8) -> Option<Self>;  // see table; 0 → None
    pub fn heading(self) -> &'static str;     // "Highest".."Lowest" (TODO.md section names)
    pub fn cli_name(self) -> &'static str;    // "highest".."lowest"
    pub fn from_cli_name(s: &str) -> Option<Self>;
}
```

**Exact codepoints** (fixture-derived from `test-vault/`; `🔼` — not obsidian-tasks' `▶️` — is Medium):

| Priority | Emoji | Codepoint | VTODO `PRIORITY` |
|---|---|---|---|
| Highest | 🔺 | U+1F53A | 1 |
| High | ⏫ | U+23EB | 3 |
| Medium | 🔼 | U+1F53C | 5 |
| Low | 🔽 | U+1F53D | 7 |
| Lowest | ⏬ | U+23EC | 9 |

Reverse mapping (deterministic, lossy for even values which round toward *higher* priority): `1,2→Highest  3,4→High  5,6→Medium  7,8→Low  9→Lowest  0|absent→None`.

### 3.3 `dates.rs`

```rust
pub struct LocalDate(pub chrono::NaiveDate);          // device-local calendar date
pub struct LocalDateTime(pub chrono::NaiveDateTime);  // minute precision, device-local wall time

pub enum When { Date(LocalDate), DateTime(LocalDateTime) }

pub enum DateError { InvalidFormat(String), OutOfRange(String) }

impl LocalDate     { pub fn parse(s: &str) -> Result<Self, DateError>; pub fn format(self) -> String; }
impl LocalDateTime { pub fn parse(s: &str) -> Result<Self, DateError>; pub fn format(self) -> String; }
impl When {
    pub fn parse_date_or_datetime(s: &str) -> Result<When, DateError>; // "YYYY-MM-DD" | "YYYY-MM-DD HH:MM"
    pub fn to_ical(self) -> String;   // "20260919" | "20260919T170000" (floating, never Z)
    pub fn from_ical(s: &str) -> Result<When, DateError>;
    pub fn is_date_only(self) -> bool;
}

pub trait Clock: Send + Sync {
    fn now_utc(&self) -> chrono::DateTime<chrono::Utc>;
    fn today_local(&self) -> LocalDate;
    fn local_offset(&self) -> chrono::FixedOffset;
}
pub struct SystemClock;   // implements Clock from system time / iana-time-zone
```

Domain code **never** calls `chrono::Utc::now()` / `Local::now()` directly — only through a `Clock` (tests inject `FixedClock`).

### 3.4 `router.rs` types (defined here, implemented in `src/router.rs`)

```rust
pub struct ListSlug(String);   // kebab-case: "Home Lab" → "home-lab"

impl ListSlug {
    pub fn from_name(name: &str) -> Result<Self, TaskresError>; // lowercase; [^a-z0-9]+→'-'; trim '-'; non-empty
    pub fn as_str(&self) -> &str;
    pub fn display_name(&self) -> String;                       // "home-lab" → "Home Lab" (title-case words)
}
```

### 3.5 `task.rs`

```rust
pub enum Status { Active, Completed { on: LocalDate } }

pub struct SourceRef { pub path: String, pub line: usize }   // vault-relative, '/'-separated; line 1-based

pub struct Task {
    pub uid: TaskUid,
    pub list: ListSlug,
    pub text: String,                     // user summary; metadata tokens stripped
    pub status: Status,
    pub priority: Option<Priority>,
    pub due: Option<When>,
    pub start: Option<When>,
    pub scheduled: Option<When>,
    pub created: Option<LocalDate>,
    pub parent: Option<TaskUid>,          // nearest ancestor checkbox (indentation-based)
    pub source: SourceRef,
    pub source_heading: Option<String>,   // nearest preceding ATX heading (render only; NOT persisted to VTODO)
    pub source_mtime: chrono::DateTime<chrono::Utc>, // mtime of source file at parse
    pub last_modified: chrono::DateTime<chrono::Utc>, // max(mtime, engine mutation time)
}

impl Task {
    /// FNV-1a (64-bit) over a canonical serialization of:
    /// uid, list, text, status, priority, due, start, scheduled, created, parent.
    /// Excludes: source line number, source_heading, mtimes.
    pub fn thumbprint(&self) -> u64;
}
```

## §4 Timestamp Contract (Markdown ⇄ VTODO) — normative

All Markdown dates are **device-local**. All VTODO instants (`CREATED`, `COMPLETED`, `LAST-MODIFIED`, `DTSTAMP`) are **UTC with `Z`**. Date-only Markdown values synthesize **midnight UTC** of that date; the reverse direction formats the **UTC calendar date** (deterministic, no timezone drift).

| Markdown token | VTODO property | Rule |
|---|---|---|
| `➕ 2026-09-19` | `CREATED:20260919T000000Z` | date-only → midnight UTC |
| `✅ 2026-09-19` | `COMPLETED:20260919T000000Z` | date-only → midnight UTC |
| `📅 2026-09-19` | `DUE;VALUE=DATE:20260919` | all-day (RFC 5545 §3.8.5.3) |
| `📅 2026-09-19 17:00` | `DUE:20260919T170000` | **floating** local time — no `Z`, no TZID |
| `🛫 …` / `⏳ …` | `DTSTART` / `X-TASKRES-SCHEDULED` | same DATE vs floating DATE-TIME rules as DUE |
| — (engine mutation) | `LAST-MODIFIED:<now>Z` | UTC now at the moment the engine rewrites the line/file |
| — (every emission) | `DTSTAMP:<now>Z` | UTC now at serialization; caller-supplied, never read from clock |
| (remote) `COMPLETED:20260920T033000Z` | `✅ 2026-09-20` | reverse rule: format UTC calendar date of the instant |
| (remote) `DUE:…Z` or `TZID=…:` | `📅 <local>` | UTC/TZID instant → device-local wall time via `Clock::local_offset`/`chrono-tz`; `VALUE=DATE` stays date-only |

Determinism: `to_vcalendar(task, now_utc)` with identical inputs yields **byte-identical** output. Tests T13 assert every row of this table both directions. Devices are assumed NTP-synced (±60 s); conflicts use a 120 s tie window (§11 R8).
