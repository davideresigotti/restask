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
use crate::markdown::todo_view::{looks_like_mirror, section_priority};
use crate::markdown::{self};
use crate::router::{scan_frontmatter, NoteMeta, NoteRouting, Router};
use crate::store::index::Index;
use crate::RestaskError;

/// Name of the per-vault state directory; hidden, so never scanned (§5.1).
pub const STATE_DIR: &str = ".restask";

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
}

/// One routed file loaded for scanning.
struct Loaded {
    path: String,
    list: ListSlug,
    contents: String,
    /// The file's mtime when `contents` was read.
    read_at: Option<SystemTime>,
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
///   a box in any editor behaves like the plugin command.
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
        let file = Loaded {
            path: meta.path.clone(),
            list,
            contents,
            read_at,
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
    // occurrence in path order) owns the UID.
    let mut owner: BTreeMap<TaskUid, (usize, usize)> = BTreeMap::new();
    let parsed: Vec<Vec<ParsedTask>> = loaded
        .iter()
        .map(|file| markdown::parse(&file.contents, cfg).tasks)
        .collect();
    for (file_no, tasks) in parsed.iter().enumerate() {
        for task in tasks {
            let Some(uid) = &task.draft.uid else {
                continue;
            };
            let preferred = index
                .get(uid)
                .is_some_and(|entry| entry.source_path == loaded[file_no].path);
            match owner.get(uid) {
                None => {
                    owner.insert(uid.clone(), (file_no, task.line_no));
                }
                Some((held_by, _)) => {
                    let held_preferred = index
                        .get(uid)
                        .is_some_and(|entry| entry.source_path == loaded[*held_by].path);
                    if preferred && !held_preferred {
                        owner.insert(uid.clone(), (file_no, task.line_no));
                    }
                }
            }
        }
    }

    // Pass 4 — repair and collect, note by note.
    let today = clock.today_local();
    for (file_no, (file, tasks)) in loaded.iter().zip(parsed).enumerate() {
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
                let ops = repairs(&tasks, &duplicate, today, true);
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
                    markdown::parse(&out.contents, cfg).tasks
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

    // Pass 5 — the inbox file: its own tasks are sources, mirror lines are views.
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
                let mut ops = repairs(&own, &duplicate, today, false);
                // A line typed under a priority's heading takes that priority as it is
                // registered (§7.4); one that names a priority itself keeps its own.
                let ranked: Vec<Mutation> = ops
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
                    .collect();
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
            collect(&mut scan, task, None, &file.path, &file.list, mtime, today);
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

/// The repair mutations for one file's task lines: identity first (line-number based),
/// then — when `placement` — status/placement fixes (UID based).
fn repairs(
    tasks: &[ParsedTask],
    duplicate: &dyn Fn(&ParsedTask) -> bool,
    today: LocalDate,
    placement: bool,
) -> Vec<Mutation> {
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
    identity.extend(status);
    identity
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
/// scan would read, or a directory (renames move notes). Events on hidden paths (the
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
    matchers.is_tracked(relative) || !name.contains('.')
}
