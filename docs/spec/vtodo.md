# restask spec — VTODO Codec & Golden Contract (§8, App. A)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §8 VTODO codec (`crates/restask/src/vtodo/`) — pure

### 8.1 Serialization

```rust
pub fn to_vcalendar(task: &Task, now_utc: DateTime<Utc>) -> String;
pub fn to_vcalendar_with(task: &Task, now_utc: DateTime<Utc>, extras: &[String]) -> String;
pub struct WireNames { pub uid: Option<String>, pub parent: Option<String> }
pub fn to_vcalendar_as(task: &Task, now_utc: DateTime<Utc>, extras: &[String], wire: &WireNames) -> String;
// WireNames also carries `obsidian_vault: Option<String>` (§8.4)
```

A `VCALENDAR` with one `VTODO`, **CRLF** line endings, in this exact order:

`BEGIN:VCALENDAR`, `VERSION:2.0`, `PRODID:-//restask//restask 0.1.0//EN`, `BEGIN:VTODO`,
`UID`, `DTSTAMP`, `CREATED`, `LAST-MODIFIED`, `SUMMARY`, `STATUS`, `PERCENT-COMPLETE`,
`PRIORITY`, `DTSTART`, `DUE`, `COMPLETED`, `RRULE`, `RELATED-TO`, `X-RESTASK-SCHEDULED`,
`X-RESTASK-SOURCE`, `X-RESTASK-TEXT`, `X-RESTASK-UID`, *extras*, `END:VTODO`,
`END:VCALENDAR`. `X-RESTASK-TEXT` only for a text with wikilinks (§8.4).

- `DTSTAMP` / `LAST-MODIFIED` = `now_utc`. `CREATED` only when the task has a creation
  date (§4).
- `SUMMARY` with TEXT escaping: `\` → `\\`, `;` → `\;`, `,` → `\,`, newline → `\n` — the
  text's wire form (§8.4): its wikilinks as Markdown links into Obsidian.
- `STATUS:NEEDS-ACTION` + `PERCENT-COMPLETE:0`, or `STATUS:COMPLETED` +
  `PERCENT-COMPLETE:100` + `COMPLETED`.
- `PRIORITY` per §3.2; `DTSTART` / `DUE` / `X-RESTASK-SCHEDULED` per §4 — each omitted
  when absent.
- `RRULE:<canonical rule>` when the task has a repeat rule (§3.6). It is written once: an
  `RRULE` among the extras is dropped when the task carries its own.
- `RELATED-TO;RELTYPE=PARENT:<uid>` when the task has a parent.
- **Wire names** (`to_vcalendar_as`): a task another client created keeps the `UID` that
  client gave it (§11 R5). With `wire.uid` the `UID` property is that value and
  `X-RESTASK-UID:<the task's restask UID>` is written — the link between the resource
  and its vault line. With `wire.parent` the parent relation names the parent by that
  value (the parent is such a task). Without them — every task made in the vault — the
  output is as above and there is no `X-RESTASK-UID`.
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
    pub adopted_as: Option<TaskUid>,        // X-RESTASK-UID (as written; §11 R5 checks it)
    pub task: Task,                         // uid is a placeholder when !managed
    pub source_path: Option<String>,        // X-RESTASK-SOURCE
    pub created_at: Option<DateTime<Utc>>,  // CREATED as an instant
    pub parent_raw: Option<String>,         // parent relation's UID, possibly foreign
    pub extras: Vec<String>,                // unmanaged content, unfolded, in order
    pub summary: String,                    // SUMMARY as found, on one line (§8.4)
    pub vault_text: Option<String>,         // X-RESTASK-TEXT (§8.4)
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
- `X-RESTASK-UID` is read as a restask UID or not at all; whether it is believed is the
  planner's decision (§11 R5).
- `RRULE`: a rule the vault can spell exactly (§3.6) becomes `task.recurrence`; any other
  rule is an extra, handed back as written.
- **Extras**: every property the serializer does not own, and every nested component
  (`VALARM` blocks) with its lines, in document order. Managed properties: `UID`,
  `DTSTAMP`, `CREATED`, `LAST-MODIFIED`, `SUMMARY`, `STATUS`, `PERCENT-COMPLETE`,
  `PRIORITY`, `DTSTART`, `DUE`, `COMPLETED`, parent `RELATED-TO`, `X-RESTASK-*`
  (and their legacy `X-TASKRES-*` spellings).
- With `X-RESTASK-TEXT`, `task.text` is `relink(SUMMARY, X-RESTASK-TEXT,
  X-RESTASK-SOURCE)` (§8.4); without it, `SUMMARY`.

Round trip: `parse(serialize(t, now, extras))` yields the same sync fields, creation
date, parent, source path and extras — and the same `thumbprint` (§9 relies on it).

### 8.4 Wikilinks on the wire (`vtodo::links`)

A vault line links notes (`- [ ] [[Dual HHD caddy]]`). Other clients cannot follow a
wikilink, but Tasks.org renders Markdown in a title, so the text of a task with
wikilinks travels in a wire form:

- **`SUMMARY`** (`wire_title`) — each wikilink becomes a Markdown link that opens its
  note in Obsidian: `[<shown>](obsidian://open?vault=<vault>&file=<note>)`. *Shown* is
  what Obsidian shows: the alias of `[[Note|alias]]`, else the target with `#` read as
  ` > ` (`[[Note#Part]]` → `Note > Part`, `[[#Part]]` → `Part`). *Note* is the target
  without its `#…` part; a same-note link (`[[#Part]]`) opens the task's own note (its
  path without `.md`). Both query values are percent-encoded (everything but the RFC 3986
  unreserved characters). *Vault* is `vault.obsidian_vault` (§14.1;
  `WireNames::obsidian_vault`); without it each wikilink is written as its shown text.
  Obsidian opens such a link only in a vault of that name — the mobile app, given
  another name, reloads to switch vaults and, finding none, stays on the last note (read
  in the 1.13.7 bundle: `execCapacitorUrl`) — so the vault must have one name on every
  device that opens the links.
  An embed (`![[…]]`) and a `[[…]]` that would show nothing are left as written. A text
  without wikilinks is unchanged.
- **`X-RESTASK-TEXT;VALUE=TEXT:<the vault text>`** — written when that differs from
  `SUMMARY`, so the resource can be read back as the line it came from.

`DESCRIPTION` is not touched: it stays the other clients' (an extra).

Read back (§8.2), `relink(title, reference, source)` turns a title into the vault text:

- a title equal to the wire form of `reference` — with links into the vault the title
  names, or as plain text — gives `reference`;
- a title that holds a wikilink is taken as written;
- otherwise, in a title with links into Obsidian, each such link becomes the wikilink of
  `reference` it shows (same text, same note, in order), else a wikilink to the note it
  opens (`[[note]]`, or `[[note|shown]]` when the text differs); in a title without such
  links, each wikilink of `reference` whose shown text the title still contains is put
  back in its place, in order. A link whose text is gone is dropped.

So an edit of the title in Tasks.org keeps the links it did not touch, and a link typed
there in the same shape becomes a wikilink in the note. The planner puts a copy again
when its `SUMMARY` or `X-RESTASK-TEXT` is not what a push would write (§11.3).

Round trip: `parse(serialize(t))` yields `t.text`, the same thumbprint, and the extras
it was given. The golden contract (no wikilinks) is unchanged.

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
