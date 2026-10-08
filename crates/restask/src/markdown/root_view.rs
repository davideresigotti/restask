//! The view a root note holds (§7.6): the TODO section of a note that declares
//! `restask-list-root` shows the prioritized tasks of every note in its folder and below,
//! filed like TODO.md — and nothing else of the note is restask's. Finding the section,
//! rendering it, sealing it, and reading back what the user edited in it. Pure — no I/O.

use std::collections::BTreeMap;

use crate::config::VaultConfig;
use crate::domain::{LocalDate, Priority, Status, Task, TaskUid};
use crate::markdown::mutator::{canonical_line, Mutation};
use crate::markdown::parser::{headings, parse_line, TaskDraft};
use crate::markdown::todo_view::{
    completed_on, digest, edits_between, mirror_line, priority_heading, seal_claim, seal_range,
    ViewText, NO_PRIORITY_HEADING, SEAL_PREFIX,
};

/// Text of the heading that opens the view, compared without regard to letter case.
const VIEW_HEADING: &str = "todo";

/// Frontmatter key that makes a note a root note (§5.1).
const ROOT_KEY: &str = "restask-list-root";

/// First line of a remembered render: the root note it is of.
const PATH_PREFIX: &str = "path: ";

/// The view of a root note: where its TODO section is in the note's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// 0-based index of the section's heading line.
    pub start: usize,
    /// 0-based index of the first line after the section.
    pub end: usize,
    /// Rank of the section's heading (1–5); the view's own headings are one below it.
    pub level: usize,
    /// Text of the section's heading, as the note spells it.
    pub title: String,
    /// Byte offset of the heading line.
    from: usize,
    /// Byte offset of the first line after the section.
    to: usize,
}

impl Section {
    /// Whether the 1-based line `line_no` of the note lies in the view.
    pub fn holds(&self, line_no: usize) -> bool {
        line_no > self.start && line_no <= self.end
    }

    /// The view's text within `contents`, the note it was found in.
    fn text<'a>(&self, contents: &'a str) -> &'a str {
        &contents[self.from..self.to]
    }

    /// The `#`s of the view's own headings: the priority sections and the done heading.
    fn hashes(&self) -> String {
        "#".repeat(self.level + 1)
    }

    /// The heading line every render of the view ends its active part with.
    fn done_line(&self, cfg: &VaultConfig) -> String {
        format!("{} {}", self.hashes(), cfg.done_heading)
    }
}

/// The view of a root note (§7.6), or `None` when the note holds none.
///
/// The view is the note's TODO section: from the first heading whose text is `TODO` (in
/// any letter case, rank 1–5) down to the next heading of the same or a higher rank —
/// the done heading never ends it, whatever its rank — or to the end of the note.
/// Frontmatter and fenced blocks hold no heading (§6.2). A note without such a heading
/// has no view and is an ordinary note. Neither has a note the seal cannot be written
/// to: one with CRLF line endings, or whose frontmatter block is not delimited by two
/// lines that are exactly `---`.
pub fn section(contents: &str, cfg: &VaultConfig) -> Option<Section> {
    if contents.contains('\r') {
        return None;
    }
    frontmatter_end(contents)?;
    let heads = headings(contents);
    let at = heads
        .iter()
        .position(|(_, level, text)| *level < 6 && text.eq_ignore_ascii_case(VIEW_HEADING))?;
    let (start, level, title) = heads[at];
    let starts: Vec<usize> = std::iter::once(0)
        .chain(contents.match_indices('\n').map(|(nl, _)| nl + 1))
        .filter(|offset| *offset < contents.len())
        .collect();
    let end = heads[at + 1..]
        .iter()
        .find(|(_, rank, text)| *rank <= level && *text != cfg.done_heading)
        .map_or(starts.len(), |(idx, _, _)| *idx);
    Some(Section {
        start,
        end,
        level,
        title: title.to_string(),
        from: *starts.get(start)?,
        to: starts.get(end).copied().unwrap_or(contents.len()),
    })
}

/// Byte offset of the closing `---` line of a frontmatter block the seal can live in:
/// the note opens with a line that is exactly `---` and has another one below it.
fn frontmatter_end(contents: &str) -> Option<usize> {
    let mut at = contents.strip_prefix("---\n").map(|_| 4)?;
    loop {
        let length = contents[at..].find('\n')?;
        if &contents[at..at + length] == "---" {
            return Some(at);
        }
        at += length + 1;
    }
}

/// `contents` with its seal line saying `digest`: replaced where the frontmatter has
/// one, else written below the `restask-list-root` line (below the last property when
/// the block has no such line).
fn with_seal(contents: &str, digest: &str) -> Option<String> {
    let line = format!("{SEAL_PREFIX}{digest}\n");
    if let Some((start, end)) = seal_range(contents) {
        return Some(format!("{}{line}{}", &contents[..start], &contents[end..]));
    }
    let close = frontmatter_end(contents)?;
    let mut at = 4;
    let mut slot = close;
    while at < close {
        let length = contents[at..].find('\n')?;
        let key = contents[at..at + length]
            .split_once(':')
            .map(|(key, _)| key);
        at += length + 1;
        if key.is_some_and(|key| key.trim() == ROOT_KEY) {
            slot = at;
            break;
        }
    }
    Some(format!("{}{line}{}", &contents[..slot], &contents[slot..]))
}

/// The folder a root note declares, as the prefix of every path in it or below: the
/// note's own directory with its `/`, empty for a note at the vault root.
fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..=slash])
}

/// `true` when `raw` is a mirror line as a render writes one (§7), ticked since or not:
/// with a priority, and ending in the link to its note — `[[<stem>|<stem>]]` or
/// `[[<stem>#<heading>|<stem>]]` — right before the `🆔` token. In a root note the
/// shape, not a mere wikilink at the end of the text, is what tells the leftover of a
/// view from a task line someone moved there.
pub fn is_mirror_shaped(raw: &str) -> bool {
    let Some(draft) = parse_line(raw).map(|line| line.draft) else {
        return false;
    };
    let Some(uid) = &draft.uid else {
        return false;
    };
    let Some(before) = raw
        .trim_end()
        .strip_suffix(uid.as_str())
        .map(str::trim_end)
        .and_then(|rest| rest.strip_suffix("🆔"))
        .map(str::trim_end)
        .and_then(|rest| rest.strip_suffix("]]"))
    else {
        return false;
    };
    let Some(open) = before.rfind("[[") else {
        return false;
    };
    let Some((target, alias)) = before[open + 2..].rsplit_once('|') else {
        return false;
    };
    let stem = target.split('#').next().unwrap_or(target);
    !stem.is_empty()
        && stem == alias
        && !stem.contains(['[', ']'])
        // The priority stands in front of the link, where the render writes it.
        && parse_line(&before[..open]).is_some_and(|head| head.draft.priority.is_some())
}

/// The view's text for `tasks`, to stand in the place of `section` (§7.6): the heading
/// line as the note has it, the priority sections, `No Priority`, and the done heading
/// with the note's own completed tasks. `follows` says the note goes on below the view.
fn view_text(
    heading_line: &str,
    section: &Section,
    path: &str,
    tasks: &BTreeMap<TaskUid, Task>,
    cfg: &VaultConfig,
    follows: bool,
) -> String {
    let folder = folder_of(path);
    let own = |task: &Task| task.source.path == path && section.holds(task.source.line);
    let mut unprioritized: Vec<&Task> = Vec::new();
    let mut prioritized: [Vec<&Task>; 5] =
        [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut done: Vec<&Task> = Vec::new();
    for task in tasks.values() {
        let mine = own(task);
        // What the view mirrors: a task of a note in the folder or below. The inbox
        // file is no note, and the root note's own lines outside the view are in the
        // note already.
        let mirrored = task.source.path != path
            && task.source.path != cfg.inbox_file
            && task.source.path.starts_with(folder);
        match (task.status, task.priority) {
            (Status::Active, Some(priority)) if mine || mirrored => {
                let slot = Priority::ALL
                    .iter()
                    .position(|candidate| *candidate == priority)
                    .unwrap_or_default();
                prioritized[slot].push(task);
            }
            (Status::Active, None) if mine => unprioritized.push(task),
            (Status::Completed { .. }, _) if mine => done.push(task),
            _ => {}
        }
    }
    for tasks in &mut prioritized {
        tasks.sort_by(|a, b| (&a.source.path, a.source.line).cmp(&(&b.source.path, b.source.line)));
    }
    done.sort_by(|a, b| {
        completed_on(b)
            .cmp(&completed_on(a))
            .then_with(|| b.uid.as_str().cmp(a.uid.as_str()))
    });

    let hashes = section.hashes();
    let line_for = |task: &Task| {
        if own(task) {
            canonical_line("", '-', &TaskDraft::from(task))
        } else {
            mirror_line(task)
        }
    };
    let block = |heading: String, tasks: &[&Task]| {
        let mut block = format!("{hashes} {heading}\n");
        for task in tasks {
            block.push_str(&line_for(task));
            block.push('\n');
        }
        block
    };
    let mut blocks: Vec<String> = Vec::new();
    for (slot, priority) in Priority::ALL.iter().enumerate() {
        if !prioritized[slot].is_empty() {
            blocks.push(block(priority_heading(*priority), &prioritized[slot]));
        }
    }
    if !unprioritized.is_empty() {
        blocks.push(block(NO_PRIORITY_HEADING.to_string(), &unprioritized));
    }
    blocks.push(block(cfg.done_heading.clone(), &done));

    let mut out = format!("{heading_line}\n\n{}", blocks.join("\n"));
    if follows {
        out.push('\n');
    }
    out
}

/// Renders the view of the root note at `path` (§7.6): `contents` with its TODO section
/// regenerated from `tasks` — every routed, registered task of the vault, as the scan
/// that read `contents` collected them — and sealed. Everything outside the section is
/// kept byte for byte, but for the seal line in the frontmatter. Same input,
/// byte-identical output. `None` when the note holds no view ([`section`]).
///
/// The sections are TODO.md's (§7), their headings one rank below the section's own:
/// each priority section holds the active tasks with that priority — the note's own
/// lines of the view and the mirror lines of tasks in the folder's other notes,
/// together, by (source path, line); `No Priority` the note's own active lines without
/// one, by UID; the done heading the note's own completed lines, newest first. Empty
/// sections are omitted, the done heading is always there.
pub fn render(
    contents: &str,
    path: &str,
    tasks: &BTreeMap<TaskUid, Task>,
    cfg: &VaultConfig,
) -> Option<String> {
    let section = section(contents, cfg)?;
    let heading_line = contents[section.from..].lines().next()?;
    let follows = section.to < contents.len();
    let view = view_text(heading_line, &section, path, tasks, cfg, follows);
    let seal = digest(&view);
    with_seal(
        &format!(
            "{}{view}{}",
            &contents[..section.from],
            &contents[section.to..]
        ),
        &seal,
    )
}

/// `true` when the view of the root note `contents` is exactly as some device rendered
/// it (§7.2): the seal in the note's frontmatter is the digest of the TODO section.
/// What the user changes in the section afterwards breaks the seal; what they change
/// in the rest of the note does not.
pub fn is_sealed(contents: &str, cfg: &VaultConfig) -> bool {
    section(contents, cfg)
        .is_some_and(|section| seal_claim(contents) == Some(&digest(section.text(contents))))
}

/// What the engine remembers of a render of the root note at `path` (§7.6), kept under
/// `.restask/views/`: the note's path, the seal the render carried, and the view's
/// text. `None` when `contents` holds no view.
pub fn remembered(contents: &str, path: &str, cfg: &VaultConfig) -> Option<String> {
    let section = section(contents, cfg)?;
    Some(format!(
        "{PATH_PREFIX}{path}\n{SEAL_PREFIX}{}\n{}",
        seal_claim(contents).unwrap_or_default(),
        section.text(contents)
    ))
}

/// The root note a remembered render is of.
pub fn remembered_path(remembered: &str) -> Option<&str> {
    remembered.lines().next()?.strip_prefix(PATH_PREFIX)
}

/// What the user changed in the view of the root note at `path` since the engine last
/// rendered it (§7.6), as mutations keyed by vault-relative path — TODO.md's rules
/// (§7.1, §7.4; `todo_view::mirror_edits`) for a view that is a section of a note:
/// an edit of a mirror line goes to the task's own note, a deleted mirror line deletes
/// the task there, and a line moved to another section takes that section's priority.
/// `contents` is the note as it is, `remembered` what [`remembered`] made of the last
/// render. Nothing when the note holds no view, the view is sealed, or `remembered` is
/// of another note.
pub fn mirror_edits(
    contents: &str,
    remembered: &str,
    path: &str,
    local: &BTreeMap<TaskUid, Task>,
    cfg: &VaultConfig,
    today: LocalDate,
) -> BTreeMap<String, Vec<Mutation>> {
    let (Some(section), Some((head, body))) = (
        section(contents, cfg),
        remembered
            .strip_prefix(PATH_PREFIX)
            .and_then(|rest| rest.strip_prefix(path))
            .and_then(|rest| rest.strip_prefix('\n'))
            .and_then(|rest| rest.split_once('\n')),
    ) else {
        return BTreeMap::new();
    };
    let done_line = section.done_line(cfg);
    let current = ViewText {
        body: section.text(contents),
        claim: seal_claim(contents),
        sealed: is_sealed(contents, cfg),
        done_line: &done_line,
    };
    let rendered = ViewText {
        body,
        claim: head.strip_prefix(SEAL_PREFIX),
        sealed: true,
        done_line: &done_line,
    };
    edits_between(&current, &rendered, local, path, cfg, today)
}
