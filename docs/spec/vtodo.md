# restask spec — VTODO Codec & Golden Contract (§8, App. A)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §8 VTODO codec (`crates/restask/src/vtodo/`) — pure

### 8.1 Serialization

```rust
pub fn to_vcalendar(task: &Task, now_utc: DateTime<Utc>) -> String;
pub fn to_vcalendar_with(task: &Task, now_utc: DateTime<Utc>, extras: &[String]) -> String;
```

A `VCALENDAR` with one `VTODO`, **CRLF** line endings, in this exact order:

`BEGIN:VCALENDAR`, `VERSION:2.0`, `PRODID:-//restask//restask 0.1.0//EN`, `BEGIN:VTODO`,
`UID`, `DTSTAMP`, `CREATED`, `LAST-MODIFIED`, `SUMMARY`, `STATUS`, `PERCENT-COMPLETE`,
`PRIORITY`, `DTSTART`, `DUE`, `COMPLETED`, `RELATED-TO`, `X-RESTASK-SCHEDULED`,
`X-RESTASK-SOURCE`, *extras*, `END:VTODO`, `END:VCALENDAR`.

- `DTSTAMP` / `LAST-MODIFIED` = `now_utc`. `CREATED` only when the task has a creation
  date (§4).
- `SUMMARY` with TEXT escaping: `\` → `\\`, `;` → `\;`, `,` → `\,`, newline → `\n`.
- `STATUS:NEEDS-ACTION` + `PERCENT-COMPLETE:0`, or `STATUS:COMPLETED` +
  `PERCENT-COMPLETE:100` + `COMPLETED`.
- `PRIORITY` per §3.2; `DTSTART` / `DUE` / `X-RESTASK-SCHEDULED` per §4 — each omitted
  when absent.
- `RELATED-TO;RELTYPE=PARENT:<uid>` when the task has a parent.
- `X-RESTASK-SOURCE;VALUE=TEXT:<vault-relative path>` always: it routes the task back to
  its note when it reaches another device's vault first.
- **Extras** — the unmanaged content of the resource being replaced (§8.2) — are written
  back verbatim, re-folded. A `DURATION` extra is dropped when a `DUE` is written (RFC
  5545 forbids both).
- Folding: physical lines ≤ 75 octets, continuation lines start with one SPACE, never
  splitting a UTF-8 codepoint.

### 8.2 Parsing

```rust
pub struct RemoteTask {
    pub raw_uid: String,                    // UID verbatim
    pub managed: bool,                      // raw_uid is a restask UID
    pub task: Task,                         // uid is a placeholder when !managed
    pub source_path: Option<String>,        // X-RESTASK-SOURCE
    pub created_at: Option<DateTime<Utc>>,  // CREATED as an instant
    pub parent_raw: Option<String>,         // parent relation's UID, possibly foreign
    pub extras: Vec<String>,                // unmanaged content, unfolded, in order
}
pub fn from_vcalendar<Z: TimeZone>(text: &str, tz: &Z, collection: &ListSlug)
    -> Result<RemoteTask, RestaskError>;
```

Collections are shared with other clients, so the parser is forgiving:

- CRLF or LF; folded lines unfolded; the first `VTODO` is used; `VTIMEZONE` and other
  sibling components are ignored.
- **The only error is a body without a `VTODO`.** Everything optional that is missing or
  malformed is simply absent: no `SUMMARY` → empty text; a bad date → no date; an unknown
  `TZID` → floating time.
- `SUMMARY` is reduced to one line (whitespace runs, newlines included, become one
  space): a task's text lives on a single Markdown line.
- `STATUS:COMPLETED` → completed on the UTC date of `COMPLETED` (else of
  `LAST-MODIFIED`/`DTSTAMP`). Any other status is active.
- Parent relation: `RELATED-TO` with no `RELTYPE`, with `RELTYPE=PARENT`, or with the
  legacy `TOREL=PARENT`. Other relation types are extras.
- `tz` is the device zone used for `Z`/`TZID` values (§4). Production passes
  `chrono::Local`; tests pass a `FixedOffset` or a `chrono_tz::Tz`.
- **Extras**: every property the serializer does not own, and every nested component
  (`VALARM` blocks) with its lines, in document order. Managed properties: `UID`,
  `DTSTAMP`, `CREATED`, `LAST-MODIFIED`, `SUMMARY`, `STATUS`, `PERCENT-COMPLETE`,
  `PRIORITY`, `DTSTART`, `DUE`, `COMPLETED`, parent `RELATED-TO`, `X-RESTASK-*`
  (and their legacy `X-TASKRES-*` spellings).

Round trip: `parse(serialize(t, now, extras))` yields the same sync fields, creation
date, parent, source path and extras — and the same `thumbprint` (§9 relies on it).

### 8.3 Golden contract

`docs/contracts/vtodo-golden.ics` (Appendix A, byte-exact, CRLF) is asserted byte-equal by
`tests/vtodo_codec.rs`. Changing it changes what every server copy looks like.

## Appendix A — Golden VTODO

```ical
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//restask//restask 0.1.0//EN
BEGIN:VTODO
UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0
DTSTAMP:20260922T143000Z
CREATED:20260919T000000Z
LAST-MODIFIED:20260922T143000Z
SUMMARY:Setup SSL certificate renew alert
STATUS:NEEDS-ACTION
PERCENT-COMPLETE:0
PRIORITY:1
DUE;VALUE=DATE:20260925
X-RESTASK-SCHEDULED;VALUE=DATE:20260923
X-RESTASK-SOURCE;VALUE=TEXT:Home Lab Test.md
END:VTODO
END:VCALENDAR
```

Source task: uid `restask-01jzetq1v2h3k4m5n6p7r8t9w0`, text `Setup SSL certificate renew
alert`, priority Highest, due 2026-09-25, scheduled 2026-09-23, created 2026-09-19,
`now_utc = 2026-09-22T14:30:00Z`, no extras.
