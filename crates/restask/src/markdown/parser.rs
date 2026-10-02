//! Line grammar (§6.1): the task-line regex, metadata tokens, and text extraction.
//!
//! Pure string→structure parsing; never fails and never panics — lines that do not match
//! the grammar yield [`None`]. File-level rules (§6.2: frontmatter, fences, done region,
//! parenting) are layered on top of [`parse_line`].

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use crate::config::VaultConfig;
use crate::domain::{LocalDate, Priority, Recurrence, TaskUid, When};

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

/// UID token (§6.1): `🆔` plus `restask-` (or the legacy `taskres-`) and 26 lowercase
/// alphanumerics.
const UID_PATTERN: &str = r"🆔[ \t]+((?:restask|taskres)-[0-9a-z]{26})";

/// ATX heading (§6.2): 1–6 `#`, one space, text with an optional closing hash sequence.
const HEADING_PATTERN: &str = r"^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$";

/// All compiled §6.1/§6.2 patterns. Stored as a `Result` so a (test-proven impossible) bad
/// fixed literal degrades to "no line is a task" instead of panicking.
struct Patterns {
    line: Regex,
    due: Regex,
    start: Regex,
    scheduled: Regex,
    completed: Regex,
    created: Regex,
    uid: Regex,
    heading: Regex,
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
                heading: Regex::new(HEADING_PATTERN)?,
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
    /// `true` when the line asks for its creation date: a `➕` standing alone, without a
    /// date (§6.1). Whoever settles the line writes the date behind it (§6.4).
    pub wants_created: bool,
    /// `✅` value.
    pub completed_on: Option<LocalDate>,
    /// `🔁` rule.
    pub recurrence: Option<Recurrence>,
}

impl From<&crate::domain::Task> for TaskDraft {
    /// The Markdown line content of a task.
    fn from(task: &crate::domain::Task) -> Self {
        let completed_on = match task.status {
            crate::domain::Status::Completed { on } => Some(on),
            crate::domain::Status::Active => None,
        };
        Self {
            uid: Some(task.uid.clone()),
            text: task.text.clone(),
            checked: completed_on.is_some(),
            priority: task.priority,
            due: task.due,
            start: task.start,
            scheduled: task.scheduled,
            created: task.created,
            wants_created: false,
            completed_on,
            recurrence: task.recurrence.clone(),
        }
    }
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
    let bare_created = standalone_created(body, &spans);
    let wants_created = !bare_created.is_empty();
    spans.extend(bare_created);

    let (uid_spans, uid) = scan_token(&patterns.uid, body);
    spans.extend(uid_spans);
    let uid = uid.and_then(|v| TaskUid::parse(&v).ok());

    let recurrence = scan_recurrence(body).map(|(span, rule)| {
        spans.push(span);
        rule
    });

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
            wants_created,
            completed_on,
            recurrence,
        },
    })
}

/// Recurrence token (§6.1): `🔁`, whitespace, then a rule in the vault spelling
/// ([`Recurrence::from_text`]). The span covers the emoji and exactly the words that form
/// the rule; a `🔁` not followed by a rule is ordinary text.
fn scan_recurrence(body: &str) -> Option<(Range<usize>, Recurrence)> {
    const SYMBOL: &str = "🔁";
    let at = body.find(SYMBOL)?;
    let after = &body[at + SYMBOL.len()..];
    let rule_text = after.trim_start_matches([' ', '\t']);
    let gap = after.len() - rule_text.len();
    if gap == 0 {
        return None;
    }
    let (rule, len) = Recurrence::from_text(rule_text)?;
    let start = at + SYMBOL.len() + gap;
    Some((at..start + len, rule))
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

/// Spans of every `➕` that stands alone in `body` — bounded by the string start/end or a
/// space/tab on both sides — and is not the head of a dated created token (`taken`): the
/// request for the creation date (§6.1).
fn standalone_created(body: &str, taken: &[Range<usize>]) -> Vec<Range<usize>> {
    const SYMBOL: &str = "➕";
    let bytes = body.as_bytes();
    body.match_indices(SYMBOL)
        .map(|(at, _)| at..at + SYMBOL.len())
        .filter(|span| {
            let bounded_before = span.start == 0 || matches!(bytes[span.start - 1], b' ' | b'\t');
            let bounded_after = span.end == bytes.len() || matches!(bytes[span.end], b' ' | b'\t');
            let dated = taken.iter().any(|token| token.contains(&span.start));
            bounded_before && bounded_after && !dated
        })
        .collect()
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

/// One task line with its file-level context (§6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTask {
    /// 1-based line number in the file.
    pub line_no: usize,
    /// Number of leading space/tab characters.
    pub indent_chars: usize,
    /// The verbatim source line (no line terminator).
    pub raw: String,
    /// Extracted metadata and text.
    pub draft: TaskDraft,
    /// Whether the line sits in the completed-records region (first `done_heading`
    /// heading to end of file).
    pub in_done_region: bool,
    /// Nearest preceding ATX heading text, if any.
    pub heading: Option<String>,
}

/// Result of parsing one file (§6.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedFile {
    /// Task lines in source order; non-task lines are absent.
    pub tasks: Vec<ParsedTask>,
    /// 1-based line number of the first heading matching `done_heading`, if present.
    pub done_heading_line: Option<usize>,
}

/// Parses a whole file into task records (§6.2). Pure: never fails; non-task lines are
/// simply absent. Invalid dates inside a matched token are tolerated (token kept verbatim
/// in [`ParsedTask::raw`], field left `None`).
///
/// Lines inside YAML frontmatter are never tasks: a `---` at byte 0 opens the block and a
/// matching `---` closes it; an unterminated block is not frontmatter (routing decision
/// D21). Lines inside fenced code blocks (` ``` ` or `~~~`, including ` ```tasks ` query
/// blocks) are never tasks. `in_done_region` becomes true at the first heading whose text
/// equals `cfg.done_heading` (case-sensitive, any ATX level) and persists to end of file.
pub fn parse(contents: &str, cfg: &VaultConfig) -> ParsedFile {
    let Some(patterns) = patterns() else {
        return ParsedFile {
            tasks: Vec::new(),
            done_heading_line: None,
        };
    };
    let lines: Vec<&str> = contents.lines().collect();
    let mut first = 0usize;
    if lines.first().is_some_and(|open| *open == "---") {
        if let Some(close) = lines.iter().skip(1).position(|l| l.trim() == "---") {
            first = close + 2;
        }
    }

    let mut tasks = Vec::new();
    let mut done_heading_line = None;
    let mut in_done = false;
    let mut heading: Option<String> = None;
    let mut fence: Option<&'static str> = None;

    for (idx, line) in lines.iter().enumerate().skip(first) {
        let line_no = idx + 1;
        let line = *line;

        if let Some(marker) = fence {
            if line.trim_start().starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_open(line) {
            fence = Some(marker);
            continue;
        }
        if let Some(text) = heading_text(&patterns.heading, line) {
            if text == cfg.done_heading && done_heading_line.is_none() {
                done_heading_line = Some(line_no);
                in_done = true;
            }
            heading = Some(text.to_string());
            continue;
        }
        let Some(task) = parse_line(line) else {
            continue;
        };
        tasks.push(ParsedTask {
            line_no,
            indent_chars: task.indent_chars,
            raw: line.to_string(),
            draft: task.draft,
            in_done_region: in_done,
            heading: heading.clone(),
        });
    }

    ParsedFile {
        tasks,
        done_heading_line,
    }
}

/// Resolves each task's parent UID (§6.2 subtasks): a task whose `indent_chars` exceeds a
/// previous task's is its child; the parent is the nearest ancestor checkbox. Ancestors
/// without a registered UID yield `None` (child treated as root). Nesting never crosses a
/// heading, and tasks in the done region have no parent (completed records are a flat
/// log). The returned vector aligns with `tasks` by position; children serialize as
/// `RELATED-TO;RELTYPE=PARENT`.
pub fn link_parents(tasks: &[ParsedTask]) -> Vec<Option<TaskUid>> {
    let mut parents = Vec::with_capacity(tasks.len());
    let mut stack: Vec<(usize, Option<TaskUid>)> = Vec::new();
    let mut section: Option<&Option<String>> = None;
    for task in tasks {
        if section != Some(&task.heading) {
            stack.clear();
            section = Some(&task.heading);
        }
        if task.in_done_region {
            parents.push(None);
            continue;
        }
        while stack
            .last()
            .is_some_and(|(indent, _)| *indent >= task.indent_chars)
        {
            stack.pop();
        }
        parents.push(stack.last().and_then(|(_, uid)| uid.clone()));
        stack.push((task.indent_chars, task.draft.uid.clone()));
    }
    parents
}

/// The fence marker a line opens (``` or ~~~, tolerating indentation), if any.
fn fence_open(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// ATX heading text of a line, with the optional closing hash sequence stripped (§6.2).
fn heading_text<'a>(re: &Regex, line: &'a str) -> Option<&'a str> {
    re.captures(line)?.get(1).map(|m| m.as_str())
}
