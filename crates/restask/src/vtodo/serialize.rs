//! VTODO serialization (§8.1): a `Task` → `VCALENDAR`/`VTODO` with CRLF line endings,
//! exact property order, iCalendar TEXT escaping, and 75-octet line folding. Pure — no I/O;
//! determinism invariant: same `Task` + same `now_utc` (+ same extras) → byte-identical
//! output.

use chrono::{DateTime, Utc};

use crate::domain::dates::{LocalDate, When};
use crate::domain::task::{Status, Task};
use crate::vtodo::links::wire_title;
use crate::vtodo::recurrence::is_rrule;

/// `PRODID` value emitted in every serialized calendar (§8.1).
const PRODID: &str = "-//restask//restask 0.1.0//EN";

/// Parameter that marks a `DTSTART` as the anchor of a repeating task without a date
/// (§8.1): a date the rule needs on the wire and the task does not have in the vault.
pub const ANCHOR_PARAM: &str = "X-RESTASK-ANCHOR";

/// Maximum octets per physical line before folding (§8.1).
const FOLD_LIMIT: usize = 75;

/// The names a task goes by on the server where they are not restask UIDs (§8.1). A
/// task another client created stays that client's resource: it keeps the `UID` it was
/// given, and a relation to it is written with that `UID`, so the client that owns the
/// task still finds it — and its subtasks — after restask wrote to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireNames {
    /// The `UID` to write instead of the task's own; the task's UID then travels as
    /// `X-RESTASK-UID`, bound to this one by `X-RESTASK-OF`.
    pub uid: Option<String>,
    /// The `UID` the parent goes by on the server, when it is not the parent's own.
    pub parent: Option<String>,
    /// The Obsidian vault the task's wikilinks open in (`vault.obsidian_vault`, §8.4);
    /// without it the title shows them as plain text.
    pub obsidian_vault: Option<String>,
}

/// Serializes a task into a complete `VCALENDAR`/`VTODO` (§8.1) with no foreign content.
pub fn to_vcalendar(task: &Task, now_utc: DateTime<Utc>) -> String {
    to_vcalendar_with(task, now_utc, &[])
}

/// Serializes a task under its own UID, with the unmanaged content `extras` of the
/// resource being replaced: [`to_vcalendar_as`] with no other names.
pub fn to_vcalendar_with(task: &Task, now_utc: DateTime<Utc>, extras: &[String]) -> String {
    to_vcalendar_as(task, now_utc, extras, &WireNames::default())
}

/// Serializes a task into a complete `VCALENDAR`/`VTODO` (§8.1), terminated by CRLF.
///
/// Managed properties are emitted in the exact §8.1 order; `CREATED`, `PRIORITY`,
/// `DTSTART`, `DUE`, `COMPLETED`, `RRULE`, `RELATED-TO`, and `X-RESTASK-SCHEDULED` are omitted when
/// their source value is `None`. `DTSTAMP` and `LAST-MODIFIED` carry `now_utc`.
///
/// `extras` are the unmanaged content lines of the resource being replaced
/// ([`crate::vtodo::RemoteTask::extras`]): they are written back verbatim (re-folded)
/// before `END:VTODO`, so descriptions, reminders, recurrence rules and other clients'
/// properties survive a push. A `DURATION` extra is dropped when a `DUE` is written (RFC
/// 5545 forbids both).
///
/// `wire` names the task and its parent as the server knows them: with `wire.uid` the
/// `UID` is that one and `X-RESTASK-UID` carries the task's own (after
/// `X-RESTASK-SOURCE`) — followed, for a counted UID, by `X-RESTASK-OF` with the `UID`
/// the link is for: a counted UID says nothing of the resource it names, and a copy of
/// the resource under a new `UID` must not pass for the task.
///
/// A text with wikilinks is written in its wire form (§8.4): `SUMMARY` shows each as a
/// Markdown link into the Obsidian vault `wire.obsidian_vault` (as plain text without
/// one), and `X-RESTASK-TEXT` keeps the vault spelling.
pub fn to_vcalendar_as(
    task: &Task,
    now_utc: DateTime<Utc>,
    extras: &[String],
    wire: &WireNames,
) -> String {
    let mut out = String::with_capacity(512);
    push_line(&mut out, "BEGIN:VCALENDAR");
    push_line(&mut out, "VERSION:2.0");
    push_line(&mut out, &format!("PRODID:{PRODID}"));
    push_line(&mut out, "BEGIN:VTODO");
    let uid = wire.uid.as_deref().unwrap_or(task.uid.as_str());
    push_line(&mut out, &format!("UID:{uid}"));
    let now = format_utc_instant(now_utc);
    push_line(&mut out, &format!("DTSTAMP:{now}"));
    if let Some(day) = task.created {
        push_line(&mut out, &format!("CREATED:{}", format_utc_midnight(day.0)));
    }
    push_line(&mut out, &format!("LAST-MODIFIED:{now}"));
    let summary = wire_title(
        &task.text,
        wire.obsidian_vault.as_deref(),
        &task.source.path,
    );
    push_line(&mut out, &format!("SUMMARY:{}", escape_text(&summary)));
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
    } else if let Some(day) = series_anchor(task, now_utc, extras) {
        push_line(
            &mut out,
            &format!(
                "DTSTART;VALUE=DATE;{ANCHOR_PARAM}=TRUE:{}",
                When::Date(day).to_ical()
            ),
        );
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
    if let Some(rule) = &task.recurrence {
        push_line(&mut out, &format!("RRULE:{}", rule.to_rrule()));
    }
    if let Some(parent) = &task.parent {
        let parent = wire.parent.as_deref().unwrap_or(parent.as_str());
        push_line(&mut out, &format!("RELATED-TO;RELTYPE=PARENT:{parent}"));
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
    if summary != task.text {
        push_line(
            &mut out,
            &format!("X-RESTASK-TEXT;VALUE=TEXT:{}", escape_text(&task.text)),
        );
    }
    if let Some(wire_uid) = &wire.uid {
        push_line(&mut out, &format!("X-RESTASK-UID:{}", task.uid.as_str()));
        if !task.uid.is_long() {
            push_line(
                &mut out,
                &format!("X-RESTASK-OF;VALUE=TEXT:{}", escape_text(wire_uid)),
            );
        }
    }
    let mut nested = 0usize;
    for extra in extras {
        let upper = extra.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            nested += 1;
        } else if upper.starts_with("END:") {
            nested = nested.saturating_sub(1);
        } else if nested == 0
            && ((task.due.is_some() && property_name(&upper) == "DURATION")
                || (task.recurrence.is_some() && is_rrule(&upper)))
        {
            // One DUE-or-DURATION, one rule: what the task itself carries wins.
            continue;
        }
        push_line(&mut out, extra);
    }
    push_line(&mut out, "END:VTODO");
    push_line(&mut out, "END:VCALENDAR");
    out
}

/// The property name of an (upper-cased) content line: the text before the first `;`/`:`.
fn property_name(line: &str) -> &str {
    line.split([';', ':']).next().unwrap_or(line)
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
/// The day a repeating task without a start and without a due date is anchored at on
/// the wire (§8.1), `None` for every other task. A rule has no first occurrence without
/// a date, and servers refuse such a `VTODO` (Radicale answers 400). The day is the
/// scheduled date, else the creation date — the server's own from the first put on
/// (§11 R2), so the same at every put — else the day a long UID was minted, and only
/// then today.
fn series_anchor(task: &Task, now_utc: DateTime<Utc>, extras: &[String]) -> Option<LocalDate> {
    if task.due.is_some() {
        return None;
    }
    let mut nested = 0usize;
    let mut ruled = task.recurrence.is_some();
    for extra in extras {
        let upper = extra.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            nested += 1;
        } else if upper.starts_with("END:") {
            nested = nested.saturating_sub(1);
        } else if nested == 0 {
            // Another client's own dates are its anchor.
            if matches!(property_name(&upper), "DTSTART" | "DUE" | "DURATION") {
                return None;
            }
            ruled |= is_rrule(&upper);
        }
    }
    if !ruled {
        return None;
    }
    let scheduled = task.scheduled.map(|when| match when {
        When::Date(day) => day,
        When::DateTime(at) => LocalDate(at.0.date()),
    });
    Some(
        scheduled
            .or(task.created)
            .or_else(|| task.uid.created_on())
            .unwrap_or(LocalDate(now_utc.date_naive())),
    )
}

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
