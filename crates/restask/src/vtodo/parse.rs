//! VTODO parsing (§8.2): an iCalendar body → [`RemoteTask`]. Pure — no I/O.
//!
//! The parser is deliberately forgiving: calendars bound to a list are shared with other
//! clients (Tasks.org, Thunderbird), so anything optional that is missing or malformed
//! degrades to "absent" instead of failing the resource — one odd task must never block a
//! sync cycle. Only a body without any `VTODO` component is an error.
//!
//! Everything the codec does not manage (`DESCRIPTION`, `CATEGORIES`, `RRULE`, `VALARM`
//! blocks, vendor `X-` properties, …) is collected verbatim into [`RemoteTask::extras`] so
//! a later `PUT` can hand it back untouched.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::domain::dates::{LocalDate, LocalDateTime, When};
use crate::domain::priority::Priority;
use crate::domain::recurrence::Recurrence;
use crate::domain::task::{ListSlug, SourceRef, Status, Task};
use crate::domain::uid::TaskUid;
use crate::RestaskError;

/// A VTODO resource fetched from a CalDAV collection (§8.2).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTask {
    /// `UID` property verbatim.
    pub raw_uid: String,
    /// `true` when `raw_uid` parses as a [`TaskUid`] (i.e. a restask-managed resource).
    pub managed: bool,
    /// `X-RESTASK-UID`: the UID restask adopted this task under, on a resource that
    /// keeps another client's `UID`. The planner believes it only when it can have been
    /// derived from that `UID` ([`TaskUid::adopts`]).
    pub adopted_as: Option<TaskUid>,
    /// Parsed task; when `!managed`, `uid` is a placeholder the planner replaces on adoption.
    pub task: Task,
    /// `X-RESTASK-SOURCE` value — the vault-relative path the task routes back to.
    pub source_path: Option<String>,
    /// `CREATED` as a full instant (the task only keeps its calendar date).
    pub created_at: Option<DateTime<Utc>>,
    /// Raw UID of the parent relation (`RELATED-TO` with `RELTYPE=PARENT` or no `RELTYPE`),
    /// which may point at a foreign task; `task.parent` holds it only when it is managed.
    pub parent_raw: Option<String>,
    /// Unfolded content lines of everything inside the `VTODO` that the codec does not
    /// manage, in document order (nested components such as `VALARM` included).
    pub extras: Vec<String>,
}

/// Placeholder UID for foreign (unmanaged) tasks; the planner replaces it on adoption.
const PLACEHOLDER_UID: &str = "restask-00000000000000000000000000";

/// Properties the serializer owns; everything else inside the `VTODO` is an extra.
const MANAGED: [&str; 16] = [
    "UID",
    "DTSTAMP",
    "CREATED",
    "LAST-MODIFIED",
    "SUMMARY",
    "STATUS",
    "PERCENT-COMPLETE",
    "PRIORITY",
    "DTSTART",
    "DUE",
    "COMPLETED",
    "X-RESTASK-SCHEDULED",
    "X-TASKRES-SCHEDULED",
    "X-RESTASK-SOURCE",
    "X-TASKRES-SOURCE",
    "X-RESTASK-UID",
];

/// Parses an iCalendar body into a [`RemoteTask`] (§8.2).
///
/// The first `VTODO` component is used. `tz` is the device-local zone: `TZID`-qualified
/// and UTC (`Z`) date-times become device-local wall time through it (§4) — pass
/// `chrono::Local` in production so each instant gets the offset valid on *its* date
/// (DST-correct), or a `FixedOffset` in tests.
pub fn from_vcalendar<Z: TimeZone>(
    text: &str,
    tz: &Z,
    collection: &ListSlug,
) -> Result<RemoteTask, RestaskError> {
    let lines = vtodo_lines(unfold(text)).ok_or_else(|| RestaskError::Validation {
        field: "vtodo",
        reason: "the resource holds no VTODO component".to_string(),
    })?;

    let mut raw_uid: Option<String> = None;
    let mut dtstamp: Option<DateTime<Utc>> = None;
    let mut last_modified: Option<DateTime<Utc>> = None;
    let mut created_at: Option<DateTime<Utc>> = None;
    let mut summary: Option<String> = None;
    let mut completed_status = false;
    let mut completed_at: Option<DateTime<Utc>> = None;
    let mut priority: Option<u8> = None;
    let mut due: Option<When> = None;
    let mut start: Option<When> = None;
    let mut scheduled: Option<When> = None;
    let mut parent_raw: Option<String> = None;
    let mut source_path: Option<String> = None;
    let mut adopted_as: Option<TaskUid> = None;
    let mut recurrence: Option<Recurrence> = None;
    let mut extras: Vec<String> = Vec::new();

    let mut nested = 0usize;
    for line in lines {
        let Some(prop) = parse_property(&line) else {
            continue;
        };
        if prop.name == "BEGIN" {
            nested += 1;
            extras.push(line);
            continue;
        }
        if prop.name == "END" {
            nested = nested.saturating_sub(1);
            extras.push(line);
            continue;
        }
        if nested > 0 {
            extras.push(line);
            continue;
        }
        match prop.name.as_str() {
            "UID" if raw_uid.is_none() => raw_uid = Some(prop.value.trim().to_string()),
            "DTSTAMP" => dtstamp = instant(&prop, tz),
            "LAST-MODIFIED" => last_modified = instant(&prop, tz),
            "CREATED" => created_at = instant(&prop, tz),
            "SUMMARY" if summary.is_none() => {
                summary = Some(single_line(&unescape_text(&prop.value)));
            }
            "STATUS" => {
                completed_status = prop.value.trim().eq_ignore_ascii_case("COMPLETED");
            }
            "COMPLETED" => completed_at = instant(&prop, tz),
            "PRIORITY" => priority = prop.value.trim().parse::<u8>().ok(),
            "DUE" => due = when(&prop, tz),
            "DTSTART" => start = when(&prop, tz),
            "X-RESTASK-SCHEDULED" | "X-TASKRES-SCHEDULED" => scheduled = when(&prop, tz),
            "X-RESTASK-SOURCE" | "X-TASKRES-SOURCE" if source_path.is_none() => {
                source_path = Some(unescape_text(&prop.value)).filter(|path| !path.is_empty());
            }
            "X-RESTASK-UID" if adopted_as.is_none() => {
                adopted_as = TaskUid::parse(&prop.value).ok();
            }
            "RELATED-TO" if is_parent_relation(&prop) => {
                if parent_raw.is_none() {
                    parent_raw = Some(prop.value.trim().to_string());
                }
            }
            // A rule the vault can spell exactly is managed; a richer one is the
            // server's own and travels as an extra.
            "RRULE" if recurrence.is_none() => match Recurrence::from_rrule(&prop.value) {
                Some((rule, true)) => recurrence = Some(rule),
                _ => extras.push(line),
            },
            name if MANAGED.contains(&name) => {}
            _ => extras.push(line),
        }
    }

    let raw_uid = raw_uid.unwrap_or_default();
    let uid = TaskUid::parse(&raw_uid).ok();
    let stamp = last_modified.or(dtstamp).unwrap_or(DateTime::UNIX_EPOCH);
    let status = if completed_status {
        Status::Completed {
            on: LocalDate(completed_at.unwrap_or(stamp).date_naive()),
        }
    } else {
        Status::Active
    };
    let task = Task {
        uid: uid.clone().unwrap_or_else(placeholder_uid),
        list: collection.clone(),
        text: summary.unwrap_or_default(),
        status,
        priority: priority.and_then(Priority::from_ical),
        due,
        start,
        scheduled,
        recurrence,
        created: created_at.map(|at| LocalDate(at.date_naive())),
        parent: parent_raw
            .as_deref()
            .and_then(|raw| TaskUid::parse(raw).ok()),
        source: SourceRef {
            path: source_path.clone().unwrap_or_default(),
            line: 0,
        },
        source_heading: None,
        source_mtime: stamp,
        last_modified: stamp,
    };
    Ok(RemoteTask {
        raw_uid,
        managed: uid.is_some(),
        adopted_as,
        task,
        source_path,
        created_at,
        parent_raw,
        extras,
    })
}

/// Placeholder UID (deterministic all-zero ULID); the planner replaces it on adoption.
fn placeholder_uid() -> TaskUid {
    match TaskUid::parse(PLACEHOLDER_UID) {
        Ok(uid) => uid,
        Err(_) => TaskUid::generate(),
    }
}

/// A parsed content line: `NAME;PARAM=value:value` with quoted-parameter support.
struct Property {
    /// Upper-cased property name.
    name: String,
    /// Parameters as (upper-cased name, unquoted value) pairs.
    params: Vec<(String, String)>,
    /// Raw (still escaped) value.
    value: String,
}

impl Property {
    /// Returns a parameter value by upper-case name.
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// `RELATED-TO` names the parent when `RELTYPE` is absent (RFC 5545 default) or `PARENT`.
/// `TOREL=PARENT` is what restask wrote before it used the standard parameter.
fn is_parent_relation(prop: &Property) -> bool {
    match prop.param("RELTYPE").or_else(|| prop.param("TOREL")) {
        Some(kind) => kind.eq_ignore_ascii_case("PARENT"),
        None => true,
    }
}

/// Splits an iCalendar body into logical lines: CRLF or LF separated, with folded
/// continuations (lines starting with SPACE or TAB) merged into their predecessor.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            if let Some(previous) = lines.last_mut() {
                previous.push_str(&raw[1..]);
            }
        } else if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

/// The logical lines strictly inside the first `VTODO` component (nested components
/// included), or `None` when the body has no `VTODO`.
fn vtodo_lines(lines: Vec<String>) -> Option<Vec<String>> {
    let mut inside: Option<Vec<String>> = None;
    let mut depth = 0usize;
    for line in lines {
        let marker = parse_property(&line)
            .filter(|prop| prop.name == "BEGIN" || prop.name == "END")
            .map(|prop| (prop.name == "BEGIN", prop.value.trim().to_ascii_uppercase()));
        let Some(collected) = inside.as_mut() else {
            if matches!(&marker, Some((true, component)) if component == "VTODO") {
                inside = Some(Vec::new());
            }
            continue;
        };
        match marker {
            Some((true, _)) => depth += 1,
            Some((false, component)) if depth == 0 && component == "VTODO" => return inside,
            Some((false, _)) => depth = depth.saturating_sub(1),
            None => {}
        }
        collected.push(line);
    }
    // An unterminated VTODO still yields what was read (truncated bodies are tolerated).
    inside
}

/// Splits a logical line into a [`Property`]; returns `None` for lines without a value
/// separator.
fn parse_property(line: &str) -> Option<Property> {
    let colon = find_top_level_separator(line, b':')?;
    let head = &line[..colon];
    let value = &line[colon + 1..];
    let mut segments = split_top_level(head, b';').into_iter();
    let name = segments.next()?.trim().to_ascii_uppercase();
    let mut params = Vec::new();
    for segment in segments {
        let (key, value) = match segment.split_once('=') {
            Some((key, value)) => (key.trim().to_ascii_uppercase(), value.trim()),
            None => (String::new(), segment),
        };
        params.push((key, value.trim_matches('"').to_string()));
    }
    Some(Property {
        name,
        params,
        value: value.to_string(),
    })
}

/// Finds the first occurrence of `separator` outside double quotes.
fn find_top_level_separator(line: &str, separator: u8) -> Option<usize> {
    let mut in_quotes = false;
    for (index, byte) in line.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            _ if byte == separator && !in_quotes => return Some(index),
            _ => {}
        }
    }
    None
}

/// Splits `text` on `separator` occurrences outside double quotes.
fn split_top_level(text: &str, separator: u8) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            _ if byte == separator && !in_quotes => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Parses `YYYYMMDDTHHMMSS` (the date-time shape shared by every form).
fn naive_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()
}

/// The UTC instant of a wall time in `zone`. A time inside a DST gap does not exist; it
/// is read one hour earlier and shifted back, i.e. with the offset valid before the gap.
fn resolve_local<Z: TimeZone>(zone: &Z, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    let hour = chrono::Duration::hours(1);
    zone.from_local_datetime(&naive)
        .earliest()
        .map(|at| at.with_timezone(&Utc))
        .or_else(|| {
            zone.from_local_datetime(&(naive - hour))
                .earliest()
                .map(|at| at.with_timezone(&Utc) + hour)
        })
}

/// Parses an instant-valued property (`DTSTAMP`, `CREATED`, `LAST-MODIFIED`,
/// `COMPLETED`). RFC 5545 requires the UTC `Z` form; other shapes seen in the wild are
/// accepted too: `TZID`-qualified, floating (read in the device zone), and date-only
/// (midnight UTC). Anything else is absent.
fn instant<Z: TimeZone>(prop: &Property, tz: &Z) -> Option<DateTime<Utc>> {
    let value = prop.value.trim();
    if let Some(body) = value.strip_suffix('Z') {
        return naive_datetime(body).map(|naive| naive.and_utc());
    }
    if let Some(naive) = naive_datetime(value) {
        return match prop.param("TZID").and_then(|id| id.parse::<Tz>().ok()) {
            Some(zone) => resolve_local(&zone, naive),
            None => resolve_local(tz, naive),
        };
    }
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|naive| naive.and_utc())
}

/// Parses a date property (§4): `VALUE=DATE` stays date-only, a floating date-time stays
/// floating, a `Z`-suffixed instant and a `TZID`-qualified date-time become device-local
/// wall time via `tz`. An unknown `TZID` is read as floating; a malformed value is absent.
fn when<Z: TimeZone>(prop: &Property, tz: &Z) -> Option<When> {
    let value = prop.value.trim();
    let local_wall = |utc: DateTime<Utc>| {
        minute_precision(When::DateTime(LocalDateTime(
            utc.with_timezone(tz).naive_local(),
        )))
    };
    if let Some(body) = value.strip_suffix('Z') {
        return naive_datetime(body).map(|naive| local_wall(naive.and_utc()));
    }
    if let Some(zone) = prop.param("TZID").and_then(|id| id.parse::<Tz>().ok()) {
        if let Some(naive) = naive_datetime(value) {
            return resolve_local(&zone, naive).map(local_wall);
        }
    }
    When::from_ical(value).ok().map(minute_precision)
}

/// Markdown wall times carry minute precision (§3.3); seconds are dropped so a value
/// survives the Markdown round trip unchanged.
fn minute_precision(value: When) -> When {
    match value {
        When::DateTime(LocalDateTime(at)) => {
            use chrono::Timelike;
            When::DateTime(LocalDateTime(
                at.with_second(0)
                    .and_then(|at| at.with_nanosecond(0))
                    .unwrap_or(at),
            ))
        }
        date => date,
    }
}

/// Reverses [`crate::vtodo::serialize`]'s TEXT escaping: `\n`/`\N` → newline, `\\` → `\`,
/// `\;` → `;`, `\,` → `,`; any other escaped character is taken literally.
fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Collapses every whitespace run (newlines included) to one space and trims: a task's
/// text lives on a single Markdown line (§6.1), so a multi-line `SUMMARY` must not be
/// able to split it.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
