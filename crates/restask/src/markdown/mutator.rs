//! Line mutations (§6.3): pure `String → String` rewriting of registered task lines.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::config::VaultConfig;
use crate::domain::{Clock, LocalDate, Priority, TaskUid, When};
use crate::error::TaskresError;
use crate::markdown::parser::{parse, parse_line, TaskDraft, TaskLine};

/// Which date-bearing token a [`Mutation::SetWhen`] targets (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenField {
    /// The `📅` due token.
    Due,
    /// The `🛫` start token.
    Start,
    /// The `⏳` scheduled token.
    Scheduled,
}

/// A single line-level mutation (§6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    /// Appends `➕ <created>` and `🆔 <uid>` to the task line at `line_no`.
    Register {
        /// 1-based line number of the unregistered task line (referring to the contents
        /// passed to [`apply`]).
        line_no: usize,
        /// UID to assign.
        uid: TaskUid,
        /// Creation date to record.
        created: LocalDate,
    },
    /// Sets the checkbox and the `✅` completion stamp.
    SetStatus {
        /// Target task.
        uid: TaskUid,
        /// `true` renders `[x]`, `false` renders `[ ]`.
        checked: bool,
        /// Completion date; `None` with `checked: true` stamps the clock's local today.
        completed_on: Option<LocalDate>,
    },
    /// Sets or clears the priority emoji.
    SetPriority {
        /// Target task.
        uid: TaskUid,
        /// New priority; `None` removes the token.
        priority: Option<Priority>,
    },
    /// Sets or clears a date-bearing token.
    SetWhen {
        /// Target task.
        uid: TaskUid,
        /// Which token.
        field: WhenField,
        /// New value; `None` removes the token.
        value: Option<When>,
    },
    /// Replaces the task text, preserving all metadata.
    EditText {
        /// Target task.
        uid: TaskUid,
        /// New text.
        text: String,
    },
    /// Moves the task line under the done heading, newest-on-top (§6.3).
    MoveToDone {
        /// Target task.
        uid: TaskUid,
    },
    /// Moves the task line to the bottom of the file's active region (§6.3).
    RestoreFromDone {
        /// Target task.
        uid: TaskUid,
    },
    /// Removes the task line entirely (§6.3).
    Delete {
        /// Target task.
        uid: TaskUid,
    },
}

/// Why a mutation was not applied (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// No line carries the mutation's `🆔` token.
    UidNotFound,
    /// The line at the recorded `line_no` is no longer that task (or not a task at all).
    LineChanged,
}

/// Result of one [`apply`] call (§6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    /// The rewritten file contents.
    pub contents: String,
    /// Mutations applied, in argument order.
    pub applied: Vec<Mutation>,
    /// Mutations skipped, in argument order, with the reason.
    pub skipped: Vec<(Mutation, SkipReason)>,
}

/// One physical line: text without its terminator plus the terminator itself
/// (`"\n"`, `"\r\n"` or empty on a final unterminated line).
struct RawLine {
    text: String,
    ending: &'static str,
}

/// Splits `contents` into physical lines, keeping each line's own terminator.
fn split_lines(contents: &str) -> Vec<RawLine> {
    let mut lines = Vec::new();
    let mut rest = contents;
    while let Some(nl) = rest.find('\n') {
        let (text, tail) = rest.split_at(nl);
        match text.strip_suffix('\r') {
            Some(stripped) => lines.push(RawLine {
                text: stripped.to_string(),
                ending: "\r\n",
            }),
            None => lines.push(RawLine {
                text: text.to_string(),
                ending: "\n",
            }),
        }
        rest = &tail[1..];
    }
    if !rest.is_empty() {
        lines.push(RawLine {
            text: rest.to_string(),
            ending: "",
        });
    }
    lines
}

/// Rejoins physical lines into file contents.
fn join_lines(lines: &[RawLine]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.text);
        out.push_str(line.ending);
    }
    out
}

/// The file's dominant line ending (`\r\n` when at least half of the `\n`s are preceded by
/// `\r`), used for lines the mutator inserts or moves.
fn dominant_ending(contents: &str) -> &'static str {
    let lf = contents.matches('\n').count();
    let crlf = contents.matches("\r\n").count();
    if lf > 0 && crlf * 2 >= lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Gives the last line the dominant ending when it was unterminated, so lines appended
/// after it start on a fresh line.
fn ensure_trailing_terminator(lines: &mut [RawLine], ending: &'static str) {
    if let Some(last) = lines.last_mut() {
        if last.ending.is_empty() {
            last.ending = ending;
        }
    }
}

/// Formats a [`When`] in its Markdown form (`YYYY-MM-DD` / `YYYY-MM-DD HH:MM`).
pub(crate) fn fmt_when(when: When) -> String {
    match when {
        When::Date(d) => d.format(),
        When::DateTime(dt) => dt.format(),
    }
}

/// Renders a task line: indent + marker + checkbox + text + canonical metadata tail
/// (`<priority> 🛫 ⏳ 📅 ✅ ➕ 🆔`, §6.1). Only present fields are emitted.
fn canonical_line(indent: &str, marker: char, draft: &TaskDraft) -> String {
    let mut tail: Vec<String> = Vec::new();
    if let Some(p) = draft.priority {
        tail.push(p.emoji().to_string());
    }
    if let Some(w) = draft.start {
        tail.push(format!("🛫 {}", fmt_when(w)));
    }
    if let Some(w) = draft.scheduled {
        tail.push(format!("⏳ {}", fmt_when(w)));
    }
    if let Some(w) = draft.due {
        tail.push(format!("📅 {}", fmt_when(w)));
    }
    if let Some(d) = draft.completed_on {
        tail.push(format!("✅ {}", d.format()));
    }
    if let Some(d) = draft.created {
        tail.push(format!("➕ {}", d.format()));
    }
    if let Some(u) = &draft.uid {
        tail.push(format!("🆔 {u}"));
    }
    let mut out = format!(
        "{indent}{marker} [{}]",
        if draft.checked { "x" } else { " " }
    );
    if !draft.text.is_empty() {
        out.push(' ');
        out.push_str(&draft.text);
    }
    for item in tail {
        out.push(' ');
        out.push_str(&item);
    }
    out
}

/// Finds the first line carrying `🆔 uid`, returning its index and parsed form.
fn locate_uid(lines: &[RawLine], uid: &TaskUid) -> Option<(usize, TaskLine)> {
    lines.iter().enumerate().find_map(|(i, line)| {
        parse_line(&line.text)
            .filter(|t| t.draft.uid.as_ref() == Some(uid))
            .map(|t| (i, t))
    })
}

/// Re-renders the line at `idx` canonically as a standalone [`RawLine`] with the given
/// ending (used when a mutation moves a line).
fn render_moved(lines: &[RawLine], idx: usize, task: TaskLine, ending: &'static str) -> RawLine {
    let indent = lines[idx].text[..task.indent_chars].to_string();
    let text = canonical_line(&indent, task.marker, &task.draft);
    RawLine { text, ending }
}

/// Rewrites the first line carrying `🆔 uid` by applying `change` to its draft; `false`
/// when no line carries the UID.
fn with_uid_line(
    lines: &mut [RawLine],
    uid: &TaskUid,
    change: impl FnOnce(&mut TaskDraft),
) -> bool {
    let Some((i, task)) = locate_uid(lines, uid) else {
        return false;
    };
    let line = &mut lines[i];
    let indent = line.text[..task.indent_chars].to_string();
    let mut draft = task.draft;
    change(&mut draft);
    line.text = canonical_line(&indent, task.marker, &draft);
    true
}

/// Applies line mutations to `contents` (§6.3). Pure `String → String`. Lines are located
/// by their `🆔` token — by 1-based `line_no` for [`Mutation::Register`], which refers to
/// the `contents` argument, so batches that also delete or move lines send their
/// `Register` ops first (the engine does). Targeted lines are re-rendered with a canonical
/// metadata tail; task text is never altered except by [`Mutation::EditText`];
/// indentation, list marker, and every non-targeted line — including its line ending —
/// are preserved byte-for-byte. A `Register` on a line that already carries the same UID
/// is an idempotent re-render. Structural ops: [`Mutation::MoveToDone`] re-inserts the
/// line directly under the done heading (newest-on-top), creating a level-3 `### Done`
/// heading at the end of the file when absent; [`Mutation::RestoreFromDone`] re-inserts
/// it at the bottom of the active region (immediately before the done heading, or end of
/// file when the heading is absent); [`Mutation::Delete`] removes the line. Inserted and
/// moved lines use the file's dominant line ending.
pub fn apply(
    contents: &str,
    ops: &[Mutation],
    cfg: &VaultConfig,
    clock: &dyn Clock,
) -> Result<MutationOutcome, TaskresError> {
    let mut lines = split_lines(contents);
    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    for op in ops {
        match op {
            Mutation::Register {
                line_no,
                uid,
                created,
            } => match line_no.checked_sub(1).and_then(|i| lines.get_mut(i)) {
                Some(line) => match parse_line(&line.text) {
                    Some(task) if task.draft.uid.as_ref().is_none_or(|u| u == uid) => {
                        let indent = line.text[..task.indent_chars].to_string();
                        let marker = task.marker;
                        let mut draft = task.draft;
                        draft.created = Some(*created);
                        draft.uid = Some(uid.clone());
                        line.text = canonical_line(&indent, marker, &draft);
                        applied.push(op.clone());
                    }
                    Some(_) => skipped.push((op.clone(), SkipReason::LineChanged)),
                    None => skipped.push((op.clone(), SkipReason::LineChanged)),
                },
                None => skipped.push((op.clone(), SkipReason::LineChanged)),
            },
            Mutation::SetStatus {
                uid,
                checked,
                completed_on,
            } => {
                let checked = *checked;
                let completed_on = *completed_on;
                let today = clock.today_local();
                if with_uid_line(&mut lines, uid, |draft| {
                    draft.checked = checked;
                    draft.completed_on = completed_on.or_else(|| checked.then_some(today));
                }) {
                    applied.push(op.clone());
                } else {
                    skipped.push((op.clone(), SkipReason::UidNotFound));
                }
            }
            Mutation::SetPriority { uid, priority } => {
                let priority = *priority;
                if with_uid_line(&mut lines, uid, |draft| draft.priority = priority) {
                    applied.push(op.clone());
                } else {
                    skipped.push((op.clone(), SkipReason::UidNotFound));
                }
            }
            Mutation::SetWhen { uid, field, value } => {
                let field = *field;
                let value = *value;
                if with_uid_line(&mut lines, uid, |draft| match field {
                    WhenField::Due => draft.due = value,
                    WhenField::Start => draft.start = value,
                    WhenField::Scheduled => draft.scheduled = value,
                }) {
                    applied.push(op.clone());
                } else {
                    skipped.push((op.clone(), SkipReason::UidNotFound));
                }
            }
            Mutation::EditText { uid, text } => {
                let text = text.clone();
                if with_uid_line(&mut lines, uid, |draft| draft.text = text) {
                    applied.push(op.clone());
                } else {
                    skipped.push((op.clone(), SkipReason::UidNotFound));
                }
            }
            Mutation::MoveToDone { uid } => match locate_uid(&lines, uid) {
                Some((idx, task)) => {
                    let ending = dominant_ending(contents);
                    let moved = render_moved(&lines, idx, task, ending);
                    lines.remove(idx);
                    let current = join_lines(&lines);
                    match parse(&current, cfg).done_heading_line {
                        Some(heading_no) => {
                            lines.insert(heading_no, moved);
                        }
                        None => {
                            ensure_trailing_terminator(&mut lines, ending);
                            lines.push(RawLine {
                                text: String::new(),
                                ending,
                            });
                            lines.push(RawLine {
                                text: "### Done".to_string(),
                                ending,
                            });
                            lines.push(moved);
                        }
                    }
                    applied.push(op.clone());
                }
                None => skipped.push((op.clone(), SkipReason::UidNotFound)),
            },
            Mutation::RestoreFromDone { uid } => match locate_uid(&lines, uid) {
                Some((idx, task)) => {
                    let ending = dominant_ending(contents);
                    let moved = render_moved(&lines, idx, task, ending);
                    lines.remove(idx);
                    let current = join_lines(&lines);
                    match parse(&current, cfg).done_heading_line {
                        Some(heading_no) => lines.insert(heading_no - 1, moved),
                        None => {
                            ensure_trailing_terminator(&mut lines, ending);
                            lines.push(moved);
                        }
                    }
                    applied.push(op.clone());
                }
                None => skipped.push((op.clone(), SkipReason::UidNotFound)),
            },
            Mutation::Delete { uid } => match locate_uid(&lines, uid) {
                Some((idx, _)) => {
                    lines.remove(idx);
                    applied.push(op.clone());
                }
                None => skipped.push((op.clone(), SkipReason::UidNotFound)),
            },
        }
    }

    Ok(MutationOutcome {
        contents: join_lines(&lines),
        applied,
        skipped,
    })
}

/// Writes `contents` to `path` atomically: a hidden `<dir>/.<name>.restask-tmp` file is
/// created, written, flushed and fsynced, then renamed over `path` (same directory ⇒
/// atomic rename on POSIX).
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), TaskresError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name =
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| TaskresError::Validation {
                field: "path",
                reason: format!("{}: not a UTF-8 file name", path.display()),
            })?;
    let tmp = dir.join(format!(".{name}.restask-tmp"));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())?;
    fs::rename(&tmp, path)?;
    Ok(())
}
