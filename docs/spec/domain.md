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
    pub fn adopts(&self, foreign_uid: &str) -> bool;   // some derived(foreign_uid, _)
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
  sort by creation; the 80 random bits are a hash of the foreign UID. `adopts` compares
  those bits: it tells whether a UID can be the adoption UID of a given foreign `UID`,
  whatever the creation instant.

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
    pub recurrence: Option<Recurrence>,   // §3.6
    pub created: Option<LocalDate>,
    pub parent: Option<TaskUid>,
    pub source: SourceRef,
    pub source_heading: Option<String>,   // render only; not persisted to VTODO
    pub source_mtime: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,     // vault: file mtime; server: LAST-MODIFIED
}
```

`Task::thumbprint()` is FNV-1a (64-bit) over uid, list, text, status, priority, due,
start, scheduled, created, parent, recurrence. It identifies a base snapshot (§9); it is **not** how
two versions are compared for syncing — the merge compares fields (§11.3).

The fields the merge manages ("sync fields") are `text`, `status`, `priority`,
`recurrence`, `due`, `start`, `scheduled`.

### 3.6 `recurrence.rs`

A repeat rule has two spellings: the vault's `🔁 every …` text and iCalendar's `RRULE`.

```rust
pub struct Recurrence { /* freq, interval, by_day, by_month_day, count, until */ }
impl Recurrence {
    pub fn from_text(text: &str) -> Option<(Recurrence, usize)>;  // rule + bytes it occupies
    pub fn to_text(&self) -> String;                              // canonical vault spelling
    pub fn from_rrule(value: &str) -> Option<(Recurrence, bool)>; // rule + "understood exactly"
    pub fn to_rrule(&self) -> String;                             // canonical RRULE value
    pub fn next_after(&self, anchor: When, done_on: LocalDate) -> Option<When>;
    pub fn is_last(&self) -> bool;                                // COUNT ≤ 1
    pub fn consumed(&self) -> Recurrence;                         // COUNT − 1
}
```

Vault spelling (case-insensitive; weekday names may be abbreviated to three letters;
list items are separated by commas and/or `and`):

| Vault | `RRULE` |
|---|---|
| `every day`, `every 3 days` | `FREQ=DAILY`, `FREQ=DAILY;INTERVAL=3` |
| `every week`, `every 2 weeks` | `FREQ=WEEKLY`, `…;INTERVAL=2` |
| `every week on Monday, Thursday` | `FREQ=WEEKLY;BYDAY=MO,TH` |
| `every weekday` | `FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR` |
| `every month`, `every month on the 15th` | `FREQ=MONTHLY`, `…;BYMONTHDAY=15` |
| `every month on the 1st, 15th, last day` | `FREQ=MONTHLY;BYMONTHDAY=1,15,-1` |
| `every month on the 2nd Tuesday` / `the last Friday` | `FREQ=MONTHLY;BYDAY=2TU` / `-1FR` |
| `every year`, `every 6 hours`, `every 30 minutes` | `FREQ=YEARLY`, `HOURLY`, `MINUTELY` |
| `… for 5 times` | `…;COUNT=5` |
| `… until 2026-12-31` | `…;UNTIL=20261231` |

- `from_text` reads a rule at the **start** of the text and stops where it ends: in
  `every week on Monday call mom` the rule is `every week on Monday`. Text that does not
  start with a rule (`every now and then`) yields none.
- `to_text` is canonical (`every week on mon and thu` → `every week on Monday,
  Thursday`); lists are sorted and de-duplicated, so equal rules compare equal whichever
  spelling they came from.
- **Managed vs. unmanaged.** A rule is *managed* — a field of the task, shown and editable
  in the vault — only when `from_rrule` understood it exactly: every part known
  (`FREQ`, `INTERVAL`, `BYDAY`, `BYMONTHDAY`, `COUNT`, `UNTIL`, `WKST=MO`) and expressible
  in the table above. Any richer rule (`BYSETPOS`, `BYMONTH`, a daily rule with `BYDAY`,
  …) stays the server's: it travels as an extra (§8.2) and never appears in the vault,
  but still drives the roll-forward (§11.6), computed from its known parts.
- `next_after`: the first occurrence after `anchor` that also lies after `done_on`. A
  month or year lacking the anchor's day has no occurrence; a time of day is kept.

## §4 Timestamp contract (Markdown ⇄ VTODO)

Markdown dates are **device-local**. VTODO instants (`CREATED`, `COMPLETED`,
`LAST-MODIFIED`, `DTSTAMP`) are **UTC with `Z`**.

| Markdown | VTODO | Rule |
|---|---|---|
| `➕ 2026-09-19` | `CREATED:20260919T000000Z` | date-only → midnight UTC |
| (no `➕`) | `CREATED` of the server copy, kept as it is; on the first push the day the UID was minted (UTC date of its ULID timestamp, `TaskUid::created_on`) | the line does not have to show the date (§6.4); the merge keeps the server's value when the vault has none (§11.3), so the two do not disagree |
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
