//! Line grammar (§6.1): the task-line regex, metadata tokens, and text extraction.
//!
//! Pure string→structure parsing; never fails and never panics — lines that do not match
//! the grammar yield [`None`]. File-level rules (§6.2: frontmatter, fences, done region,
//! parenting) are layered on top of [`parse_line`].

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::domain::{LocalDate, Priority, TaskUid, When};

/// The §6.1 task-line regex, verbatim: single-line unordered-list checkbox.
const TASK_LINE_PATTERN: &str =
    r"^(?P<indent>[ \t]*)(?P<marker>[-*+])[ \t]+\[(?P<check>[ xX])\][ \t]+(?P<body>.*)$";

/// Due token (§6.1): `📅` plus a date or date-time value.
const DUE_PATTERN: &str = r"📅[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)";

/// Start token (§6.1): `🛫` plus a date or date-time value.
const START_PATTERN: &str = r"🛫[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)";

/// Scheduled token (§6.1): `⏳` plus a date or date-time value.
const SCHEDULED_PATTERN: &str = r"⏳[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)";

/// Completed token (§6.1): `✅` plus a date-only value.
const COMPLETED_PATTERN: &str = r"✅[ \t]+(\d{4}-\d{2}-\d{2})";

/// Created token (§6.1): `➕` plus a date-only value.
const CREATED_PATTERN: &str = r"➕[ \t]+(\d{4}-\d{2}-\d{2})";

/// UID token (§6.1): `🆔` plus `taskres-` and 26 lowercase alphanumerics.
const UID_PATTERN: &str = r"🆔[ \t]+(taskres-[0-9a-z]{26})";

/// All compiled §6.1 patterns. Stored as a `Result` so a (test-proven impossible) bad
/// fixed literal degrades to "no line is a task" instead of panicking.
struct Patterns {
    line: Regex,
    due: Regex,
    start: Regex,
    scheduled: Regex,
    completed: Regex,
    created: Regex,
    uid: Regex,
}

fn patterns() -> Option<&'static Patterns> {
    static PATTERNS: OnceLock<Result<Patterns, regex::Error>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            Ok(Patterns {
                line: Regex::new(TASK_LINE_PATTERN)?,
                due: Regex::new(DUE_PATTERN)?,
                start: Regex::new(START_PATTERN)?,
                scheduled: Regex::new(SCHEDULED_PATTERN)?,
                completed: Regex::new(COMPLETED_PATTERN)?,
                created: Regex::new(CREATED_PATTERN)?,
                uid: Regex::new(UID_PATTERN)?,
            })
        })
        .as_ref()
        .ok()
}

/// Metadata and text extracted from one task-line body (§6.1 token table).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskDraft {
    /// `🆔` token, when present with a valid Crockford ULID body.
    pub uid: Option<TaskUid>,
    /// Body with all matched token spans removed, whitespace runs collapsed, then trimmed.
    pub text: String,
    /// `true` for `[x]`/`[X]`.
    pub checked: bool,
    /// One of the five priority emoji appearing as a standalone token.
    pub priority: Option<Priority>,
    /// `📅` value.
    pub due: Option<When>,
    /// `🛫` value.
    pub start: Option<When>,
    /// `⏳` value.
    pub scheduled: Option<When>,
    /// `➕` value.
    pub created: Option<LocalDate>,
    /// `✅` value.
    pub completed_on: Option<LocalDate>,
}

/// One line matched by the §6.1 grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    /// Number of leading space/tab characters.
    pub indent_chars: usize,
    /// List marker: `-`, `*` or `+`.
    pub marker: char,
    /// Extracted metadata and text.
    pub draft: TaskDraft,
}

/// Parses a single line against the §6.1 grammar; [`None`] when the line is not a task.
///
/// A trailing `\n` or `\r\n` is ignored (LF and CRLF files). Metadata tokens are
/// recognized anywhere in the body, in any order (§6.1 order-insensitive parse); when the
/// same token appears more than once the first occurrence wins and every matched span is
/// removed from [`TaskDraft::text`]. A token whose value is shaped correctly but
/// semantically invalid (impossible date, non-ULID body) leaves its field [`None`] — the
/// invalid value survives verbatim only in the caller's original line.
pub fn parse_line(line: &str) -> Option<TaskLine> {
    let patterns = patterns()?;
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let caps = patterns.line.captures(line)?;
    let indent = caps.name("indent")?.as_str();
    let marker = caps.name("marker")?.as_str().chars().next()?;
    let check = caps.name("check")?.as_str();
    let body = caps.name("body")?.as_str();

    let mut spans: Vec<Range<usize>> = Vec::new();

    let mut priority = None;
    for (at, found) in standalone_priorities(body) {
        spans.push(at..at + found.emoji().len());
        if priority.is_none() {
            priority = Some(found);
        }
    }

    let (due_spans, due) = scan_token(&patterns.due, body);
    spans.extend(due_spans);
    let due = due.and_then(|v| When::parse_date_or_datetime(&v).ok());

    let (start_spans, start) = scan_token(&patterns.start, body);
    spans.extend(start_spans);
    let start = start.and_then(|v| When::parse_date_or_datetime(&v).ok());

    let (scheduled_spans, scheduled) = scan_token(&patterns.scheduled, body);
    spans.extend(scheduled_spans);
    let scheduled = scheduled.and_then(|v| When::parse_date_or_datetime(&v).ok());

    let (completed_spans, completed) = scan_token(&patterns.completed, body);
    spans.extend(completed_spans);
    let completed_on = completed.and_then(|v| LocalDate::parse(&v).ok());

    let (created_spans, created) = scan_token(&patterns.created, body);
    spans.extend(created_spans);
    let created = created.and_then(|v| LocalDate::parse(&v).ok());

    let (uid_spans, uid) = scan_token(&patterns.uid, body);
    spans.extend(uid_spans);
    let uid = uid.and_then(|v| TaskUid::parse(&v).ok());

    let text = extract_text(body, &spans);
    Some(TaskLine {
        indent_chars: indent.chars().count(),
        marker,
        draft: TaskDraft {
            uid,
            text,
            checked: check.eq_ignore_ascii_case("x"),
            priority,
            due,
            start,
            scheduled,
            created,
            completed_on,
        },
    })
}

/// Byte offsets of every priority emoji occurring as a standalone token in `body`,
/// left-to-right, paired with its [`Priority`]. "Standalone" means bounded by the string
/// start/end or by a space/tab on both sides.
fn standalone_priorities(body: &str) -> Vec<(usize, Priority)> {
    let bytes = body.as_bytes();
    let mut found: Vec<(usize, Priority)> = Vec::new();
    for priority in Priority::ALL {
        let emoji = priority.emoji();
        let mut from = 0;
        while let Some(rel) = body[from..].find(emoji) {
            let at = from + rel;
            let end = at + emoji.len();
            let bounded_before = at == 0 || matches!(bytes[at - 1], b' ' | b'\t');
            let bounded_after = end == bytes.len() || matches!(bytes[end], b' ' | b'\t');
            if bounded_before && bounded_after {
                found.push((at, priority));
            }
            from = end;
        }
    }
    found.sort_by_key(|&(at, _)| at);
    found
}

/// Whole-match spans of `re` in `body`, plus group 1 of the first match.
fn scan_token(re: &Regex, body: &str) -> (Vec<Range<usize>>, Option<String>) {
    let mut spans = Vec::new();
    let mut first_value = None;
    for caps in re.captures_iter(body) {
        let Some(whole) = caps.get(0) else { continue };
        spans.push(whole.start()..whole.end());
        if first_value.is_none() {
            first_value = caps.get(1).map(|m| m.as_str().to_string());
        }
    }
    (spans, first_value)
}

/// Removes every span from `body`, collapses runs of spaces/tabs to single spaces and
/// trims the residue (§6.1 `text` definition).
fn extract_text(body: &str, spans: &[Range<usize>]) -> String {
    let mut ordered: Vec<&Range<usize>> = spans.iter().collect();
    ordered.sort_by_key(|span| span.start);
    let mut residue = String::with_capacity(body.len());
    let mut cursor = 0;
    for span in ordered {
        if span.start < cursor {
            continue;
        }
        residue.push_str(&body[cursor..span.start]);
        cursor = span.end;
    }
    residue.push_str(&body[cursor..]);

    let mut collapsed = String::with_capacity(residue.len());
    let mut in_run = false;
    for ch in residue.chars() {
        if ch == ' ' || ch == '\t' {
            if !in_run {
                collapsed.push(' ');
            }
            in_run = true;
        } else {
            collapsed.push(ch);
            in_run = false;
        }
    }
    collapsed.trim().to_string()
}
