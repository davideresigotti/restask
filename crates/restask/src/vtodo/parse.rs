//! VTODO parsing (§8.2): an iCalendar body → [`RemoteTask`]. Pure — no I/O.
//!
//! Unfolds folded lines, accepts CRLF or LF, skips `VTIMEZONE`/`VALARM`/unknown components
//! and unknown properties, tolerates missing optional properties. `TZID`-qualified and UTC
//! (`Z`) date-times are converted to device-local wall time via the `tz` parameter
//! (chrono-tz lookup by name, §4).

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::domain::dates::{LocalDate, LocalDateTime, When};
use crate::domain::priority::Priority;
use crate::domain::task::{ListSlug, SourceRef, Status, Task};
use crate::domain::uid::TaskUid;
use crate::TaskresError;

/// A VTODO resource fetched from a CalDAV collection (§8.2).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTask {
    /// `UID` property verbatim.
    pub raw_uid: String,
    /// `true` when `raw_uid` parses as a [`TaskUid`] (i.e. a Taskres-managed resource).
    pub managed: bool,
    /// Parsed task; when `!managed`, `uid` is a placeholder the engine replaces on adoption.
    pub task: Task,
    /// `X-TASKRES-SOURCE` value — the vault-relative path the task routes back to.
    pub source_path: Option<String>,
}

/// Placeholder UID for foreign (unmanaged) tasks; the engine replaces it on adoption.
const PLACEHOLDER_UID: &str = "taskres-00000000000000000000000000";

/// Parses an iCalendar body into a [`RemoteTask`] (§8.2).
///
/// The first `VTODO` component is used; `VTIMEZONE`, `VALARM`, and any other component are
/// skipped, as are unknown properties. Missing optional properties leave the corresponding
/// `Task` field at its absent value; a missing `SUMMARY` or `VTODO` component is an error.
pub fn from_vcalendar(
    text: &str,
    tz: FixedOffset,
    collection: &ListSlug,
) -> Result<RemoteTask, TaskresError> {
    let props = collect_vtodo_properties(unfold(text));
    let mut raw_uid: Option<String> = None;
    let mut dtstamp: Option<DateTime<Utc>> = None;
    let mut last_modified: Option<DateTime<Utc>> = None;
    let mut created: Option<LocalDate> = None;
    let mut summary: Option<String> = None;
    let mut completed_status = false;
    let mut completed_at: Option<DateTime<Utc>> = None;
    let mut priority: Option<u8> = None;
    let mut due: Option<When> = None;
    let mut start: Option<When> = None;
    let mut scheduled: Option<When> = None;
    let mut parent: Option<String> = None;
    let mut source_path: Option<String> = None;

    for prop in &props {
        match prop.name.as_str() {
            "UID" if raw_uid.is_none() => raw_uid = Some(prop.value.clone()),
            "DTSTAMP" => dtstamp = Some(instant(prop, "dtstamp")?),
            "LAST-MODIFIED" => last_modified = Some(instant(prop, "last-modified")?),
            "CREATED" => created = Some(LocalDate(instant(prop, "created")?.date_naive())),
            "SUMMARY" if summary.is_none() => summary = Some(unescape_text(&prop.value)),
            "STATUS" => {
                if prop.value.trim().eq_ignore_ascii_case("COMPLETED") {
                    completed_status = true;
                }
            }
            "COMPLETED" => completed_at = Some(instant(prop, "completed")?),
            "PRIORITY" => priority = prop.value.trim().parse::<u8>().ok(),
            "DUE" => due = Some(when(prop, tz, "due")?),
            "DTSTART" => start = Some(when(prop, tz, "dtstart")?),
            "X-TASKRES-SCHEDULED" => scheduled = Some(when(prop, tz, "scheduled")?),
            "RELATED-TO" => {
                let is_parent = prop.params.is_empty()
                    || prop.params.iter().any(|(name, value)| {
                        name == "TOREL" && value.eq_ignore_ascii_case("PARENT")
                    });
                if is_parent && parent.is_none() {
                    parent = Some(prop.value.clone());
                }
            }
            "X-TASKRES-SOURCE" if source_path.is_none() => {
                source_path = Some(unescape_text(&prop.value));
            }
            _ => {}
        }
    }

    let raw_uid = raw_uid.unwrap_or_default();
    let managed = TaskUid::parse(&raw_uid).is_ok();
    let stamp = last_modified.or(dtstamp).unwrap_or(DateTime::UNIX_EPOCH);
    let status = match (completed_status, completed_at.or(dtstamp)) {
        (true, Some(at)) => Status::Completed {
            on: LocalDate(at.date_naive()),
        },
        _ => Status::Active,
    };
    let task = Task {
        uid: TaskUid::parse(&raw_uid).unwrap_or_else(|_| placeholder_uid()),
        list: collection.clone(),
        text: summary.ok_or_else(|| TaskresError::Validation {
            field: "summary",
            reason: "VTODO has no SUMMARY property".to_string(),
        })?,
        status,
        priority: priority.and_then(Priority::from_ical),
        due,
        start,
        scheduled,
        created,
        parent: parent.and_then(|raw| TaskUid::parse(&raw).ok()),
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
        managed,
        task,
        source_path,
    })
}

/// Placeholder UID (deterministic all-zero ULID); the engine replaces it on adoption.
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
    /// Returns a parameter value by (case-insensitive) name.
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
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
        } else {
            lines.push(raw.to_string());
        }
    }
    lines
}

/// Splits a logical line into a [`Property`]; returns `None` for lines without a value
/// separator (e.g. blank lines).
fn parse_property(line: &str) -> Option<Property> {
    let colon = find_top_level_separator(line, b':')?;
    let head = &line[..colon];
    let value = &line[colon + 1..];
    let mut segments = head.split(';');
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

/// Collects the properties of the first `VTODO` component, skipping `VTIMEZONE`, `VALARM`,
/// and any other nested or sibling component, plus unknown properties.
fn collect_vtodo_properties(lines: Vec<String>) -> Vec<Property> {
    let mut props = Vec::new();
    let mut in_todo = false;
    let mut todo_done = false;
    let mut skip_stack: Vec<String> = Vec::new();
    for line in lines {
        let Some(prop) = parse_property(&line) else {
            continue;
        };
        match prop.name.as_str() {
            "BEGIN" => {
                let component = prop.value.trim().to_ascii_uppercase();
                if !skip_stack.is_empty() {
                    skip_stack.push(component);
                } else if component == "VTODO" && !todo_done {
                    in_todo = true;
                } else if component != "VCALENDAR" {
                    skip_stack.push(component);
                }
            }
            "END" => {
                let component = prop.value.trim().to_ascii_uppercase();
                if let Some(top) = skip_stack.last() {
                    if *top == component {
                        skip_stack.pop();
                    }
                } else if component == "VTODO" && in_todo {
                    in_todo = false;
                    todo_done = true;
                }
            }
            _ => {
                if in_todo && skip_stack.is_empty() {
                    props.push(prop);
                }
            }
        }
    }
    props
}

/// Parses an iCalendar UTC instant (`YYYYMMDDTHHMMSSZ`) into a `DateTime<Utc>`.
fn instant(prop: &Property, field: &'static str) -> Result<DateTime<Utc>, TaskresError> {
    let value = prop.value.trim();
    let body = value
        .strip_suffix('Z')
        .ok_or_else(|| invalid(field, value))?;
    let naive =
        NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").map_err(|_| invalid(field, value))?;
    Ok(naive.and_utc())
}

/// Parses a date property (§4): `VALUE=DATE` stays date-only, a floating date-time stays
/// floating, a `Z`-suffixed instant and a `TZID`-qualified date-time are converted to
/// device-local wall time via `tz` (chrono-tz lookup by name).
fn when(prop: &Property, tz: FixedOffset, field: &'static str) -> Result<When, TaskresError> {
    let value = prop.value.trim();
    if let Ok(parsed) = When::from_ical(value) {
        return Ok(parsed);
    }
    if let Some(body) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S")
            .map_err(|_| invalid(field, value))?;
        return Ok(When::DateTime(LocalDateTime(
            naive.and_utc().with_timezone(&tz).naive_local(),
        )));
    }
    if let Some(tzid) = prop.param("TZID") {
        let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            .map_err(|_| invalid(field, value))?;
        let zone: Tz = tzid.parse().map_err(|_| invalid(field, value))?;
        let utc = zone
            .from_local_datetime(&naive)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(|| local_fallback(naive, tz));
        return Ok(When::DateTime(LocalDateTime(
            utc.with_timezone(&tz).naive_local(),
        )));
    }
    Err(invalid(field, value))
}

/// Resolves a wall time that fell into a DST gap by reading it in the device-local offset.
fn local_fallback(naive: NaiveDateTime, tz: FixedOffset) -> DateTime<Utc> {
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Builds a validation error for a malformed property value.
fn invalid(field: &'static str, value: &str) -> TaskresError {
    TaskresError::Validation {
        field,
        reason: format!("malformed iCalendar value `{value}`"),
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
