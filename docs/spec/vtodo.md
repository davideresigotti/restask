# Taskres Spec — VTODO Codec & Golden Contract (§8, App. A)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §8 VTODO Codec (`src/vtodo/`)

### 8.1 Serialization

```rust
pub fn to_vcalendar(task: &Task, now_utc: chrono::DateTime<chrono::Utc>) -> String;
```

Emits a `VCALENDAR`/`VTODO` with **CRLF** line endings, properties in this exact order:

`BEGIN:VCALENDAR`, `VERSION:2.0`, `PRODID:-//taskres//restask 0.1.0//EN`, `BEGIN:VTODO`, `UID`, `DTSTAMP`, `CREATED`, `LAST-MODIFIED`, `SUMMARY`, `STATUS`, `PERCENT-COMPLETE`, `PRIORITY`, `DTSTART`, `DUE`, `COMPLETED`, `RELATED-TO`, `X-TASKRES-SCHEDULED`, `X-TASKRES-SOURCE`, `END:VTODO`, `END:VCALENDAR`.

Property rules:
- `UID:<taskres-…>` verbatim. `DTSTAMP`/`LAST-MODIFIED` = `now_utc` (§4). `CREATED` from `➕` date or `now_utc` if absent.
- `SUMMARY:<text>` with iCalendar TEXT escaping: `\` → `\\`, `;` → `\;`, `,` → `\,`, newline → `\n`.
- `STATUS:NEEDS-ACTION` | `COMPLETED`; `PERCENT-COMPLETE:0` | `100`.
- `PRIORITY` per §3.2 (omitted when `None`). `DTSTART`/`DUE`/`COMPLETED`/`X-TASKRES-SCHEDULED;VALUE=DATE` per §4 (omitted when `None`).
- `RELATED-TO;TOREL=PARENT:<uid>` when `parent` is `Some`.
- `X-TASKRES-SOURCE;VALUE=TEXT:<vault-relative path>` (escaped) — lets remote-created tasks route back to the right note.
- Line folding: physical lines ≤ 75 **octets** (UTF-8), folded with a leading SPACE continuation, never splitting a codepoint; short enough lines are never folded.

### 8.2 Parsing

```rust
pub struct RemoteTask {
    pub raw_uid: String,        // UID property verbatim
    pub managed: bool,          // raw_uid parses as TaskUid
    pub task: Task,             // when !managed: fields populated, uid = placeholder (engine replaces on adoption)
    pub source_path: Option<String>, // X-TASKRES-SOURCE
}

/// Unfolds folded lines, accepts CRLF/LF, skips VTIMEZONE/VALARM and unknown properties,
/// tolerates missing optional properties. TZID/UTC datetimes converted via `tz` (chrono-tz lookup by name).
pub fn from_vcalendar(text: &str, tz: chrono::FixedOffset, collection: &ListSlug) -> Result<RemoteTask, TaskresError>;
```

Round-trip property: for any `Task` produced by the parser, `to_vcalendar(parse(to_vcalendar(t, now)), now) == to_vcalendar(t, now)`.

### 8.3 Golden contract

`docs/contracts/vtodo-golden.ics` (Appendix A, byte-exact, CRLF) is asserted byte-equal by `tests/vtodo_codec.rs` **and** by the Obsidian plugin test suite (`plugins/obsidian/test/vtodo.test.ts`) — two implementations, one contract.

## Appendix A — Golden VTODO (`docs/contracts/vtodo-golden.ics`)

Byte-exact contract; every physical line ends CRLF (`0x0D 0x0A`). Nothing here requires folding (longest line is 59 octets).

```ical
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//taskres//restask 0.1.0//EN
BEGIN:VTODO
UID:taskres-01jzetq1v2h3k4m5n6p7r8t9w0
DTSTAMP:20260922T143000Z
CREATED:20260919T000000Z
LAST-MODIFIED:20260922T143000Z
SUMMARY:Setup SSL certificate renew alert
STATUS:NEEDS-ACTION
PERCENT-COMPLETE:0
PRIORITY:1
DUE;VALUE=DATE:20260925
X-TASKRES-SCHEDULED;VALUE=DATE:20260923
X-TASKRES-SOURCE;VALUE=TEXT:Home Lab Test.md
END:VTODO
END:VCALENDAR
```

Source task: uid `taskres-01jzetq1v2h3k4m5n6p7r8t9w0`, text `Setup SSL certificate renew alert`, priority Highest, due 2026-09-25 (date-only), scheduled 2026-09-23, created 2026-09-19, list `home-lab`, `now_utc = 2026-09-22T14:30:00Z`, `last_modified = now_utc`.
