//! VTODO serialization (§8.1): a `Task` → `VCALENDAR`/`VTODO` with CRLF line endings,
//! exact property order, iCalendar TEXT escaping, and 75-octet line folding. Pure — no I/O;
//! determinism invariant: same `Task` + same `now_utc` → byte-identical output
//! (ARCHITECTURE.md invariant 10).

use chrono::{DateTime, Utc};

use crate::domain::dates::When;
use crate::domain::task::{Status, Task};

/// `PRODID` value emitted in every serialized calendar (§8.1).
const PRODID: &str = "-//restask//restask 0.1.0//EN";

/// Maximum octets per physical line before folding (§8.1).
const FOLD_LIMIT: usize = 75;

/// Serializes a task into a complete `VCALENDAR`/`VTODO` (§8.1), terminated by CRLF.
///
/// Properties are emitted in the exact §8.1 order; `PRIORITY`, `DTSTART`, `DUE`,
/// `COMPLETED`, `RELATED-TO`, and `X-RESTASK-SCHEDULED` are omitted when their source
/// value is `None`. `DTSTAMP` and `LAST-MODIFIED` carry `now_utc`; `CREATED` derives from
/// the creation date (midnight UTC) or falls back to `now_utc`.
pub fn to_vcalendar(task: &Task, now_utc: DateTime<Utc>) -> String {
    let mut out = String::with_capacity(512);
    push_line(&mut out, "BEGIN:VCALENDAR");
    push_line(&mut out, "VERSION:2.0");
    push_line(&mut out, &format!("PRODID:{PRODID}"));
    push_line(&mut out, "BEGIN:VTODO");
    push_line(&mut out, &format!("UID:{}", task.uid.as_str()));
    let now = format_utc_instant(now_utc);
    push_line(&mut out, &format!("DTSTAMP:{now}"));
    let created = match task.created {
        Some(day) => format_utc_midnight(day.0),
        None => now.clone(),
    };
    push_line(&mut out, &format!("CREATED:{created}"));
    push_line(&mut out, &format!("LAST-MODIFIED:{now}"));
    push_line(&mut out, &format!("SUMMARY:{}", escape_text(&task.text)));
    let (status, percent) = match task.status {
        Status::Active => ("NEEDS-ACTION", 0),
        Status::Completed { .. } => ("COMPLETED", 100),
    };
    push_line(&mut out, &format!("STATUS:{status}"));
    push_line(&mut out, &format!("PERCENT-COMPLETE:{percent}"));
    if let Some(priority) = task.priority {
        push_line(&mut out, &format!("PRIORITY:{}", priority.to_ical()));
    }
    if let Some(start) = task.start {
        push_line(&mut out, &when_property("DTSTART", start));
    }
    if let Some(due) = task.due {
        push_line(&mut out, &when_property("DUE", due));
    }
    if let Status::Completed { on } = task.status {
        push_line(
            &mut out,
            &format!("COMPLETED:{}", format_utc_midnight(on.0)),
        );
    }
    if let Some(parent) = &task.parent {
        push_line(
            &mut out,
            &format!("RELATED-TO;TOREL=PARENT:{}", parent.as_str()),
        );
    }
    if let Some(scheduled) = task.scheduled {
        push_line(&mut out, &when_property("X-RESTASK-SCHEDULED", scheduled));
    }
    push_line(
        &mut out,
        &format!(
            "X-RESTASK-SOURCE;VALUE=TEXT:{}",
            escape_text(&task.source.path)
        ),
    );
    push_line(&mut out, "END:VTODO");
    push_line(&mut out, "END:VCALENDAR");
    out
}

/// Formats a UTC instant as iCalendar `YYYYMMDDTHHMMSSZ` (§4).
fn format_utc_instant(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Synthesizes midnight UTC of a calendar date, e.g. `2026-09-19` → `20260919T000000Z` (§4).
fn format_utc_midnight(day: chrono::NaiveDate) -> String {
    format!("{}T000000Z", day.format("%Y%m%d"))
}

/// Emits a date property: all-day values carry `VALUE=DATE`, floating date-times carry no
/// parameter and no `Z` (§4).
fn when_property(name: &str, when: When) -> String {
    match when {
        When::Date(_) => format!("{};VALUE=DATE:{}", name, when.to_ical()),
        When::DateTime(_) => format!("{}:{}", name, when.to_ical()),
    }
}

/// Escapes a value as iCalendar TEXT (§8.1): `\` → `\\`, `;` → `\;`, `,` → `\,`,
/// newline → `\n`. Backslash is rewritten first so already-escaped sequences survive.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Appends a logical content line to `out`, folding it into physical lines of at most
/// [`FOLD_LIMIT`] octets each (§8.1). Continuation lines begin with a single SPACE; a fold
/// is never placed inside a UTF-8 codepoint. Every physical line ends with CRLF.
fn push_line(out: &mut String, line: &str) {
    let mut rest = line;
    let mut first = true;
    loop {
        let used = if first { 0 } else { 1 };
        let budget = FOLD_LIMIT - used;
        if rest.len() <= budget {
            if !first {
                out.push(' ');
            }
            out.push_str(rest);
            out.push_str("\r\n");
            return;
        }
        let mut take = budget;
        while !rest.is_char_boundary(take) {
            take -= 1;
        }
        let (chunk, remainder) = rest.split_at(take);
        if !first {
            out.push(' ');
        }
        out.push_str(chunk);
        out.push_str("\r\n");
        rest = remainder;
        first = false;
    }
}
