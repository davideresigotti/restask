//! Vault scan (§5, §6.4): walks the vault, routes notes, repairs task lines, and builds
//! the local task set. The one place that reads notes — the engine and the CLI share it.
//!
//! Local-only by default (§5): for an unrouted note only the frontmatter block is read;
//! its body is never loaded, parsed, or modified.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::config::{VaultConfig, VaultMatchers};
use crate::domain::{Clock, ListSlug, LocalDate, SourceRef, Status, Task, TaskUid};
use crate::fsio;
use crate::markdown::mutator::{self, Mutation};
use crate::markdown::parser::{link_parents, ParsedTask};
use crate::markdown::root_view::{self, Section};
use crate::markdown::todo_view::{looks_like_mirror, section_priority};
use crate::markdown::{self};
use crate::router::{scan_frontmatter, NoteMeta, NoteRouting, Router};
use crate::store::index::Index;
use crate::RestaskError;

/// Name of the per-vault state directory; hidden, so never scanned (§5.1).
pub const STATE_DIR: &str = ".restask";

/// Name of the vault config at the vault root (§14.1).
pub const CONFIG_FILE: &str = "restask.toml";

/// Longest frontmatter block the router looks at (lines). A note whose block is longer is
/// treated as having none — routing markers belong at the top.
const FRONTMATTER_MAX_LINES: usize = 512;

/// Whether the scan may write to the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Report what is there; touch nothing (`status`, `doctor`).
    ReadOnly,
    /// Register new task lines and normalize the ones edited by hand (§6.4).
    Repair,
}

/// Result of one vault scan.
#[derive(Debug, Default)]
pub struct Scan {
    /// Every routed, registered task, keyed by UID.
    pub local: BTreeMap<TaskUid, Task>,
    /// Routed notes (the inbox file excluded): vault-relative path → list.
    pub notes: BTreeMap<String, ListSlug>,
    /// Per list, the note that receives tasks created on the server: its root note when
    /// it has one, else its first routed note in path order.
    pub homes: BTreeMap<ListSlug, String>,
    /// Routed (or possibly routed) files that could not be read this pass.
    pub unreadable: BTreeSet<String>,
    /// Syncthing conflict copies found in the vault (never scanned for tasks).
    pub conflict_files: Vec<String>,
    /// Routed files parsed (the inbox file included when present).
    pub files_scanned: usize,
    /// Task lines that received a fresh UID.
    pub registered: usize,
    /// Task lines normalized (hand-checked boxes moved to the done region, reopened ones
    /// restored, duplicated UIDs reassigned).
    pub normalized: usize,
    /// Lines in routed notes that share a UID with an earlier line (read-only scans
    /// report them; repair scans reassign them).
    pub duplicates: Vec<(TaskUid, String)>,
    /// Registered tasks whose line asks for its creation date with a bare `➕` (§6.4):
    /// UID → vault-relative path of the file the line is in. A line registered in this
    /// scan is not among them: it got today's date with its UID.
    pub created_requests: BTreeMap<TaskUid, String>,
    /// Root notes that hold a view (§7.6): vault-relative path → the note's text as this
    /// scan read it, its own repairs included. The line numbers of the tasks in `local`
    /// are lines of this text, so a render is made from it — and written only while the
    /// note still is this text.
    pub views: BTreeMap<String, String>,
    /// Tasks whose line is a view's own (§7.6): a view is flat, so their indentation
    /// says nothing about a parent — as for the lines of the inbox file.
    pub flat: BTreeSet<TaskUid>,
}

/// One routed file loaded for scanning.
struct Loaded {
    path: String,
    list: ListSlug,
    contents: String,
    /// The file's mtime when `contents` was read.
    read_at: Option<SystemTime>,
    /// The view the note holds, when it is a root note with a TODO section (§7.6).
    view: Option<Section>,
}

/// `true` when a line of the note `file_no` is a mirror line that strayed (§7.6): it
/// has the exact shape of one, and its UID is owned by a line of another note — the
/// task it shows. Such a line is no task, wherever it is: a view that lost its heading,
/// a root note that is one no more, a line pasted out of a view. Read as a task it
/// would be the copy of another line and be given a UID of its own — a second task.
fn is_stray_mirror(
    task: &ParsedTask,
    file_no: usize,
    owner: &BTreeMap<TaskUid, (usize, usize)>,
) -> bool {
    task.draft.uid.as_ref().is_some_and(|uid| {
        owner.get(uid).is_some_and(|(holder, _)| *holder != file_no)
            && root_view::is_mirror_shaped(&task.raw)
    })
}

/// Writes a repaired note back — unless the file changed on disk since it was read (an
/// editor or file sync saved it meanwhile), in which case nothing is written and the
/// caller leaves the file alone for this pass. Returns whether the write happened.
fn write_back(vault: &Path, file: &Loaded, contents: &str) -> Result<bool, RestaskError> {
    let path = vault.join(&file.path);
    let unchanged = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        == file.read_at;
    if !unchanged {
        tracing::info!(path = %file.path, "note changed during the scan; repaired on the next pass");
        return Ok(false);
    }
    fsio::write_atomic(&path, contents)?;
    Ok(true)
}

/// Scans the vault (§5–§6). In [`ScanMode::Repair`] routed notes are rewritten where
/// needed — atomically, one write per file — before their tasks are collected:
///
/// * a task line without a UID gets a fresh `🆔` — and `➕ <today>` when it asks for its
///   creation date with a bare `➕` (§6.4);
/// * in the inbox file, such a line under a priority section's heading also gets that
///   priority, unless it names one itself (§7.4);
/// * a line sharing its UID with an earlier line (a duplicated line) gets its own UID;
/// * a checked line outside the done region is stamped `✅` and moved under the done
///   heading; an unchecked line inside it is restored to the active region — so checking
///   a box in any editor behaves like the plugin command;
/// * in the view of a root note, as in the inbox file, only identity is repaired and a
///   new line takes the priority of its section: placement is the render's (§7.6).
///
/// `index` breaks duplicate-UID ties: the note the index knows as the task's source
/// keeps the UID.
pub fn scan(
    vault: &Path,
    cfg: &VaultConfig,
    clock: &dyn Clock,
    index: &Index,
    mode: ScanMode,
) -> Result<Scan, RestaskError> {
    let matchers = cfg.matchers().map_err(|error| RestaskError::Config {
        path: vault.join("restask.toml").display().to_string(),
        reason: error.to_string(),
    })?;
    let inbox_list = inbox_list(cfg)?;
    let mut scan = Scan::default();

    // Pass 1 — routing: frontmatter of every tracked note.
    let mut paths = Vec::new();
    walk(vault, "", &matchers, &mut paths, &mut scan.conflict_files)?;
    paths.sort();
    let mut metas: Vec<NoteMeta> = Vec::with_capacity(paths.len());
    for path in paths {
        match read_frontmatter(&vault.join(&path)) {
            Ok(head) => {
                let (file_list, folder_list) = scan_frontmatter(&head);
                metas.push(NoteMeta {
                    path,
                    file_list,
                    folder_list,
                });
            }
            Err(error) => {
                tracing::debug!(path = %path, %error, "unreadable vault file skipped");
                scan.unreadable.insert(path);
            }
        }
    }
    let router = Router::build(&metas)?;

    // Pass 2 — load the routed notes (and only those).
    let mut loaded: Vec<Loaded> = Vec::new();
    let mut inbox: Option<Loaded> = None;
    let mut roots: BTreeMap<ListSlug, String> = BTreeMap::new();
    for meta in &metas {
        let is_inbox = meta.path == cfg.inbox_file;
        let list = if is_inbox {
            inbox_list.clone()
        } else {
            match router.resolve(&meta.path, meta.file_list.as_deref()) {
                NoteRouting::List(list) => list,
                NoteRouting::LocalOnly => continue,
            }
        };
        let full_path = vault.join(&meta.path);
        let read_at = std::fs::metadata(&full_path)
            .and_then(|m| m.modified())
            .ok();
        let contents = match std::fs::read_to_string(&full_path) {
            Ok(contents) => contents,
            Err(error) => {
                tracing::warn!(path = %meta.path, %error, "unreadable vault file skipped");
                scan.unreadable.insert(meta.path.clone());
                continue;
            }
        };
        // A root note shows its folder in its TODO section (§7.6); the inbox file is a
        // view already.
        let root = !is_inbox
            && meta
                .folder_list
                .as_deref()
                .is_some_and(|name| ListSlug::from_name(name).is_ok());
        let view = root.then(|| root_view::section(&contents, cfg)).flatten();
        let file = Loaded {
            path: meta.path.clone(),
            list,
            contents,
            read_at,
            view,
        };
        if is_inbox {
            inbox = Some(file);
            continue;
        }
        scan.notes.insert(file.path.clone(), file.list.clone());
        let declares_root = meta
            .folder_list
            .as_deref()
            .and_then(|name| ListSlug::from_name(name).ok())
            .is_some_and(|root| root == file.list);
        if declares_root {
            roots
                .entry(file.list.clone())
                .or_insert_with(|| file.path.clone());
        }
        scan.homes
            .entry(file.list.clone())
            .or_insert_with(|| file.path.clone());
        loaded.push(file);
    }
    // A list's root note beats its first note in path order.
    scan.homes.extend(roots);
    scan.files_scanned = loaded.len() + usize::from(inbox.is_some());

    // Pass 3 — duplicate UIDs across notes: the index's source note (else the first
    // occurrence in path order) owns the UID. The lines in the view of a root note are
    // not asked: a mirror line there has the UID of the task it shows (pass 5). Nor
    // does a line with the shape of a mirror line take a UID from a line without it:
    // it is the view of that line, strayed out of its view.
    let mut owner: BTreeMap<TaskUid, (usize, usize)> = BTreeMap::new();
    let mut shaped: BTreeSet<TaskUid> = BTreeSet::new();
    let parsed: Vec<Vec<ParsedTask>> = loaded
        .iter()
        .map(|file| markdown::parse(&file.contents, cfg).tasks)
        .collect();
    for (file_no, tasks) in parsed.iter().enumerate() {
        let view = loaded[file_no].view.as_ref();
        for task in tasks {
            if view.is_some_and(|view| view.holds(task.line_no)) {
                continue;
            }
            let Some(uid) = &task.draft.uid else {
                continue;
            };
            let preferred = index
                .get(uid)
                .is_some_and(|entry| entry.source_path == loaded[file_no].path);
            let mirror = root_view::is_mirror_shaped(&task.raw);
            let takes = match owner.get(uid) {
                None => true,
                Some((held_by, _)) => {
                    let held_preferred = index
                        .get(uid)
                        .is_some_and(|entry| entry.source_path == loaded[*held_by].path);
                    let held_mirror = shaped.contains(uid);
                    (held_mirror && !mirror)
                        || (held_mirror == mirror && preferred && !held_preferred)
                }
            };
            if takes {
                owner.insert(uid.clone(), (file_no, task.line_no));
                if mirror {
                    shaped.insert(uid.clone());
                } else {
                    shaped.remove(uid);
                }
            }
        }
    }

    // Pass 4 — repair and collect, note by note. A root note with a view waits for
    // pass 5: what its view shows is known only once the other notes are read.
    let today = clock.today_local();
    let mut rooted: Vec<(usize, Vec<ParsedTask>)> = Vec::new();
    for (file_no, (file, tasks)) in loaded.iter().zip(parsed).enumerate() {
        if file.view.is_some() {
            rooted.push((file_no, tasks));
            continue;
        }
        let tasks: Vec<ParsedTask> = tasks
            .into_iter()
            .filter(|task| !is_stray_mirror(task, file_no, &owner))
            .collect();
        let duplicate = |task: &ParsedTask| {
            task.draft
                .uid
                .as_ref()
                .is_some_and(|uid| owner.get(uid) != Some(&(file_no, task.line_no)))
        };
        for task in tasks.iter().filter(|task| duplicate(task)) {
            if let Some(uid) = &task.draft.uid {
                scan.duplicates.push((uid.clone(), file.path.clone()));
            }
        }
        let tasks = match mode {
            ScanMode::ReadOnly => tasks
                .into_iter()
                .filter(|task| !duplicate(task))
                .collect::<Vec<_>>(),
            ScanMode::Repair => {
                let (mut ops, status) = repairs(&tasks, &duplicate, today, true);
                ops.extend(status);
                if ops.is_empty() {
                    tasks
                } else {
                    let out = mutator::apply(&file.contents, &ops, cfg, clock)?;
                    if !write_back(vault, file, &out.contents)? {
                        // Its tasks are unknown this pass (not gone): decide nothing.
                        scan.unreadable.insert(file.path.clone());
                        continue;
                    }
                    count_repairs(&mut scan, &out.applied, &file.path);
                    markdown::parse(&out.contents, cfg)
                        .tasks
                        .into_iter()
                        .filter(|task| !is_stray_mirror(task, file_no, &owner))
                        .collect()
                }
            }
        };
        let mtime = file_mtime(&vault.join(&file.path))?;
        let parents = link_parents(&tasks);
        for (task, parent) in tasks.iter().zip(parents) {
            collect(
                &mut scan, task, parent, &file.path, &file.list, mtime, today,
            );
        }
    }

    // Pass 5 — the root notes that hold a view (§7.6), the deepest folder first: a
    // view shows the own lines of the views below it. A line in a view is a source or a
    // view of another note's task, as in the inbox file; the rest of the note is a note.
    rooted
        .sort_by_key(|(file_no, _)| std::cmp::Reverse(loaded[*file_no].path.matches('/').count()));
    for (file_no, tasks) in rooted {
        let file = &loaded[file_no];
        let duplicate = |task: &ParsedTask| {
            task.draft
                .uid
                .as_ref()
                .is_some_and(|uid| owner.get(uid) != Some(&(file_no, task.line_no)))
        };
        // The lines of the note in three kinds: outside the view, the view's own, and
        // — third — the own lines that repeat a UID of an earlier one (copies).
        let sort = |tasks: &[ParsedTask], view: &Section, local: &BTreeMap<TaskUid, Task>| {
            // A line of this note claims a UID while it stands outside the view: one
            // that a repair has just moved into it (checked, under the done heading) is
            // a line of the view's own now, not the view of a task elsewhere.
            let beside: BTreeSet<&TaskUid> = tasks
                .iter()
                .filter(|task| !view.holds(task.line_no))
                .filter(|task| !is_stray_mirror(task, file_no, &owner))
                .filter_map(|task| task.draft.uid.as_ref())
                .collect();
            let claimed = |uid: &TaskUid| {
                owner.get(uid).is_some_and(|(holder, _)| *holder != file_no)
                    || beside.contains(uid)
                    || local.contains_key(uid)
            };
            let mut outside = Vec::new();
            let mut own = Vec::new();
            let mut copies: BTreeSet<usize> = BTreeSet::new();
            let mut seen: BTreeSet<TaskUid> = BTreeSet::new();
            for task in tasks {
                if !view.holds(task.line_no) {
                    if !is_stray_mirror(task, file_no, &owner) {
                        outside.push(task.clone());
                    }
                } else if !is_view_of_task(task, &file.path, &claimed, index) {
                    if let Some(uid) = &task.draft.uid {
                        if !seen.insert(uid.clone()) {
                            copies.insert(task.line_no);
                        }
                    }
                    own.push(task.clone());
                }
            }
            (outside, own, copies)
        };
        let Some(view) = &file.view else {
            continue;
        };
        let (outside, own, copies) = sort(&tasks, view, &scan.local);
        for task in outside.iter().filter(|task| duplicate(task)) {
            if let Some(uid) = &task.draft.uid {
                scan.duplicates.push((uid.clone(), file.path.clone()));
            }
        }
        let (contents, outside, own, view) = match mode {
            ScanMode::ReadOnly => (
                file.contents.clone(),
                outside
                    .into_iter()
                    .filter(|task| !duplicate(task))
                    .collect::<Vec<_>>(),
                own.into_iter()
                    .filter(|task| !copies.contains(&task.line_no))
                    .collect::<Vec<_>>(),
                view.clone(),
            ),
            ScanMode::Repair => {
                // Outside the view the note is repaired like any note; inside it only
                // identity is, and a new line takes its section's priority (§7.4).
                let (mut ops, status) = repairs(&outside, &duplicate, today, true);
                let copied = |task: &ParsedTask| copies.contains(&task.line_no);
                let (identity, _) = repairs(&own, &copied, today, false);
                let ranked = section_ranks(&identity, &own);
                ops.extend(identity);
                ops.extend(ranked);
                ops.extend(status);
                if ops.is_empty() {
                    (file.contents.clone(), outside, own, view.clone())
                } else {
                    let out = mutator::apply(&file.contents, &ops, cfg, clock)?;
                    if !write_back(vault, file, &out.contents)? {
                        // Its tasks are unknown this pass (not gone): decide nothing.
                        scan.unreadable.insert(file.path.clone());
                        continue;
                    }
                    count_repairs(&mut scan, &out.applied, &file.path);
                    let Some(view) = root_view::section(&out.contents, cfg) else {
                        scan.unreadable.insert(file.path.clone());
                        continue;
                    };
                    let tasks = markdown::parse(&out.contents, cfg).tasks;
                    let (outside, own, _) = sort(&tasks, &view, &scan.local);
                    (out.contents, outside, own, view)
                }
            }
        };
        let mtime = file_mtime(&vault.join(&file.path))?;
        let parents = link_parents(&outside);
        for (task, parent) in outside.iter().zip(parents) {
            collect(
                &mut scan, task, parent, &file.path, &file.list, mtime, today,
            );
        }
        for mut task in own {
            // A mirror line of the task links to the view, not to the section of it the
            // task happens to be filed in.
            task.heading = Some(view.title.clone());
            if let Some(uid) = &task.draft.uid {
                if !scan.local.contains_key(uid) {
                    scan.flat.insert(uid.clone());
                }
            }
            collect(&mut scan, &task, None, &file.path, &file.list, mtime, today);
        }
        scan.views.insert(file.path.clone(), contents);
    }

    // Pass 6 — the inbox file: its own tasks are sources, mirror lines are views.
    if let Some(file) = inbox {
        let tasks = markdown::parse(&file.contents, cfg).tasks;
        let views: BTreeSet<usize> = tasks
            .iter()
            .filter(|task| is_view_of_note(task, &scan.local, index, cfg))
            .map(|task| task.line_no)
            .collect();
        let mut seen: BTreeSet<&TaskUid> = BTreeSet::new();
        let mut duplicates: BTreeSet<usize> = BTreeSet::new();
        for task in tasks.iter().filter(|task| !views.contains(&task.line_no)) {
            if let Some(uid) = &task.draft.uid {
                if !seen.insert(uid) {
                    duplicates.insert(task.line_no);
                }
            }
        }
        let tasks = match mode {
            ScanMode::ReadOnly => tasks,
            ScanMode::Repair => {
                // Placement inside TODO.md is the render's job: only identity is repaired
                // here, which rewrites lines in place and keeps every line number.
                let own: Vec<ParsedTask> = tasks
                    .iter()
                    .filter(|task| !views.contains(&task.line_no))
                    .cloned()
                    .collect();
                let duplicate = |task: &ParsedTask| duplicates.contains(&task.line_no);
                let (mut ops, _) = repairs(&own, &duplicate, today, false);
                let ranked = section_ranks(&ops, &own);
                ops.extend(ranked);
                if ops.is_empty() {
                    tasks
                } else {
                    let out = mutator::apply(&file.contents, &ops, cfg, clock)?;
                    if write_back(vault, &file, &out.contents)? {
                        count_repairs(&mut scan, &out.applied, &file.path);
                        markdown::parse(&out.contents, cfg).tasks
                    } else {
                        // Registered lines only; the rest is picked up next pass.
                        scan.unreadable.insert(file.path.clone());
                        tasks
                    }
                }
            }
        };
        let mtime = file_mtime(&vault.join(&file.path))?;
        for task in tasks.iter().filter(|task| !views.contains(&task.line_no)) {
            // A line of the view's own lives in the calendar it names, else in the one
            // the file is bound to (§7.5).
            let list = task.draft.list.as_ref().unwrap_or(&file.list);
            collect(&mut scan, task, None, &file.path, list, mtime, today);
        }
    }

    Ok(scan)
}

/// The inbox list (§5.2): the inbox file routes to `vault.inbox_list` (§14.1), whose slug
/// names the CalDAV collection the user bound TODO.md to during setup.
pub fn inbox_list(cfg: &VaultConfig) -> Result<ListSlug, RestaskError> {
    ListSlug::from_name(&cfg.inbox_list).map_err(|_| RestaskError::Validation {
        field: "inbox_list",
        reason: format!("cannot slugify the inbox list name `{}`", cfg.inbox_list),
    })
}

/// The further calendars whose tasks live in the inbox file (§7.5): `vault.todo_lists`
/// as slugs, without the inbox list itself. A task created on the server in one of them
/// gets its line in the inbox file when no note is the home of that list.
pub fn todo_lists(cfg: &VaultConfig) -> Result<BTreeSet<ListSlug>, RestaskError> {
    let inbox = inbox_list(cfg)?;
    let mut lists = BTreeSet::new();
    for name in &cfg.todo_lists {
        let slug = ListSlug::from_name(name).map_err(|_| RestaskError::Validation {
            field: "todo_lists",
            reason: format!("cannot slugify the calendar name `{name}`"),
        })?;
        if slug != inbox {
            lists.insert(slug);
        }
    }
    Ok(lists)
}

/// `true` when an inbox-file line is a rendered view of a note task rather than a task of
/// its own: its UID is claimed by a note; or — the source line being gone — the index
/// remembers the task lived in a note; or, with no index entry, it has the rendered shape.
fn is_view_of_note(
    task: &ParsedTask,
    local: &BTreeMap<TaskUid, Task>,
    index: &Index,
    cfg: &VaultConfig,
) -> bool {
    let Some(uid) = &task.draft.uid else {
        return false;
    };
    if local.contains_key(uid) {
        return true;
    }
    match index.get(uid) {
        Some(entry) => entry.source_path != cfg.inbox_file,
        None => looks_like_mirror(&task.raw),
    }
}

/// `true` when a line in the view of the root note at `path` is a rendered view of
/// another note's task rather than a task of the root note (§7.6): its UID is claimed
/// by a line outside the view — in another note, or in this one — or, no line claiming
/// it, it is a mirror line as a render writes one and the index does not know the task
/// as this note's: the leftover of a task that is gone. A line that merely ends in a
/// wikilink is a task of the note: someone may have moved it here.
fn is_view_of_task(
    task: &ParsedTask,
    path: &str,
    claimed: &dyn Fn(&TaskUid) -> bool,
    index: &Index,
) -> bool {
    let Some(uid) = &task.draft.uid else {
        return false;
    };
    claimed(uid)
        || (root_view::is_mirror_shaped(&task.raw)
            && index.get(uid).is_none_or(|entry| entry.source_path != path))
}

/// The priorities the lines registered by `identity` take from the section of the view
/// they were typed in (§7.4): a line under a priority's heading gets that priority; one
/// that names a priority itself keeps its own.
fn section_ranks(identity: &[Mutation], own: &[ParsedTask]) -> Vec<Mutation> {
    identity
        .iter()
        .filter_map(|op| match op {
            Mutation::Register { line_no, uid, .. } => own
                .iter()
                .find(|task| task.line_no == *line_no)
                .filter(|task| task.draft.priority.is_none())
                .and_then(|task| task.heading.as_deref())
                .and_then(section_priority)
                .map(|priority| Mutation::SetPriority {
                    uid: uid.clone(),
                    priority: Some(priority),
                }),
            _ => None,
        })
        .collect()
}

/// The repair mutations for one file's task lines, in two batches: identity
/// (line-number based, so it is applied first), and — when `placement` —
/// status/placement fixes (UID based).
fn repairs(
    tasks: &[ParsedTask],
    duplicate: &dyn Fn(&ParsedTask) -> bool,
    today: LocalDate,
    placement: bool,
) -> (Vec<Mutation>, Vec<Mutation>) {
    let mut identity = Vec::new();
    let mut status = Vec::new();
    for task in tasks {
        let uid = match &task.draft.uid {
            // A checkbox without text is a task still to be written (§6.4): it has no
            // identity yet, and nothing to bring in line with its box.
            None if task.draft.text.is_empty() => continue,
            None => {
                let uid = TaskUid::generate();
                identity.push(Mutation::Register {
                    line_no: task.line_no,
                    uid: uid.clone(),
                    created: task.draft.wants_created.then_some(today),
                });
                uid
            }
            Some(_) if duplicate(task) => {
                let uid = TaskUid::generate();
                identity.push(Mutation::Reassign {
                    line_no: task.line_no,
                    uid: uid.clone(),
                });
                uid
            }
            Some(uid) => uid.clone(),
        };
        if !placement {
            continue;
        }
        let draft = &task.draft;
        match (draft.checked, task.in_done_region) {
            (true, false) => {
                status.push(Mutation::SetStatus {
                    uid: uid.clone(),
                    checked: true,
                    completed_on: Some(draft.completed_on.unwrap_or(today)),
                });
                status.push(Mutation::MoveToDone { uid });
            }
            (true, true) if draft.completed_on.is_none() => {
                status.push(Mutation::SetStatus {
                    uid,
                    checked: true,
                    completed_on: Some(today),
                });
            }
            (false, true) => {
                status.push(Mutation::RestoreFromDone { uid: uid.clone() });
                status.push(Mutation::SetStatus {
                    uid,
                    checked: false,
                    completed_on: None,
                });
            }
            (false, false) if draft.completed_on.is_some() => {
                status.push(Mutation::SetStatus {
                    uid,
                    checked: false,
                    completed_on: None,
                });
            }
            _ => {}
        }
    }
    (identity, status)
}

/// Books applied repairs into the scan counters and the log.
fn count_repairs(scan: &mut Scan, applied: &[Mutation], path: &str) {
    let mut normalized: BTreeSet<TaskUid> = BTreeSet::new();
    for mutation in applied {
        match mutation {
            Mutation::Register { uid, .. } => {
                scan.registered += 1;
                tracing::info!(uid = %uid, path = %path, "task_registered");
            }
            Mutation::Reassign { uid, .. } => {
                normalized.insert(uid.clone());
                tracing::info!(uid = %uid, path = %path, "duplicate uid reassigned");
            }
            Mutation::MoveToDone { uid } => {
                normalized.insert(uid.clone());
                tracing::info!(uid = %uid, path = %path, "task_completed");
            }
            Mutation::RestoreFromDone { uid } => {
                normalized.insert(uid.clone());
                tracing::info!(uid = %uid, path = %path, "task_restored");
            }
            Mutation::SetStatus { uid, .. } => {
                normalized.insert(uid.clone());
            }
            _ => {}
        }
    }
    scan.normalized += normalized.len();
}

/// Adds one registered task line to the scan.
fn collect(
    scan: &mut Scan,
    task: &ParsedTask,
    parent: Option<TaskUid>,
    path: &str,
    list: &ListSlug,
    mtime: DateTime<Utc>,
    today: LocalDate,
) {
    let Some(uid) = task.draft.uid.clone() else {
        return;
    };
    if task.draft.wants_created && task.draft.created.is_none() {
        scan.created_requests
            .entry(uid.clone())
            .or_insert_with(|| path.to_string());
    }
    let status = if task.draft.checked {
        Status::Completed {
            on: task.draft.completed_on.unwrap_or(today),
        }
    } else {
        Status::Active
    };
    scan.local.entry(uid.clone()).or_insert_with(|| Task {
        uid,
        list: list.clone(),
        text: task.draft.text.clone(),
        status,
        priority: task.draft.priority,
        due: task.draft.due,
        start: task.draft.start,
        scheduled: task.draft.scheduled,
        recurrence: task.draft.recurrence.clone(),
        created: task.draft.created,
        parent,
        source: SourceRef {
            path: path.to_string(),
            line: task.line_no,
        },
        source_heading: task.heading.clone(),
        source_mtime: mtime,
        last_modified: mtime,
    });
}

/// Reads a note's frontmatter block only: nothing past the closing `---` (or past the
/// first line when the note does not open with `---`) is read from disk.
fn read_frontmatter(path: &Path) -> std::io::Result<String> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut head = String::new();
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 || line.trim_end_matches(['\r', '\n']).trim() != "---" {
        return Ok(head);
    }
    head.push_str(&line);
    for _ in 0..FRONTMATTER_MAX_LINES {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        head.push_str(&line);
        if line.trim() == "---" {
            return Ok(head);
        }
    }
    // Unterminated (or oversized) block: not frontmatter.
    Ok(String::new())
}

/// File mtime as a UTC instant.
fn file_mtime(path: &Path) -> Result<DateTime<Utc>, RestaskError> {
    Ok(std::fs::metadata(path)?.modified()?.into())
}

/// `true` for files the scan never treats as notes: setup backups of the inbox file and
/// Syncthing conflict copies (which duplicate their original's UIDs).
fn is_artifact(name: &str) -> bool {
    name.contains(".pre-restask-") || is_conflict_copy(name)
}

/// `true` for Syncthing conflict copies (`<name>.sync-conflict-<date>-<time>-<id>.md`).
pub fn is_conflict_copy(name: &str) -> bool {
    name.contains(".sync-conflict-")
}

/// `true` for a file or directory name the vault does not consist of (§5.1): a hidden
/// one. The state directory is one; so are the places other tools keep copies of notes
/// in — a file sync's version archive (`.stversions`), `.trash`, `.git` — and a copy of
/// a routed note is not a note: scanned, each would be registered as tasks of its own.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Recursively collects tracked note paths (vault-relative, `/`-separated). Hidden files
/// and directories are skipped at every depth, whatever `track` says.
fn walk(
    dir: &Path,
    relative: &str,
    matchers: &VaultMatchers,
    out: &mut Vec<String>,
    conflicts: &mut Vec<String>,
) -> Result<(), RestaskError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if is_hidden(&name) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk(&entry.path(), &child, matchers, out, conflicts)?;
        } else if matchers.is_tracked(&child) {
            if is_conflict_copy(&name) {
                conflicts.push(child);
            } else if !is_artifact(&name) {
                out.push(child);
            }
        }
    }
    Ok(())
}

/// Whether a file event on a vault-relative path can change what a scan sees: a note the
/// scan would read, a directory (renames move notes), or the vault config, which says
/// what the scan reads and which calendars TODO.md shows. Events on hidden paths (the
/// state directory, a file sync's version archive), on ignored paths, on artifacts and on
/// atomic-write temp files never do — in particular the engine's own state writes do not
/// wake the daemon.
pub fn is_relevant_event(relative: &str, matchers: &VaultMatchers) -> bool {
    let hidden = relative.split('/').any(is_hidden);
    if relative.is_empty() || hidden || matchers.is_ignored(relative) {
        return false;
    }
    let name = relative.rsplit('/').next().unwrap_or(relative);
    if is_artifact(name) || name.ends_with(".restask-tmp") {
        return false;
    }
    relative == CONFIG_FILE || matchers.is_tracked(relative) || !name.contains('.')
}
