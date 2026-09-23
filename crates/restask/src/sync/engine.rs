//! Reconciliation engine (§11, §13.1): the single-writer I/O orchestrator. Owns every
//! mutation of the vault, `.restask/` state, and the server; the planner decides, the
//! engine executes and records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::caldav::CaldavPort;
use crate::config::{MachineConfig, VaultConfig, VaultMatchers};
use crate::domain::{Clock, ListSlug, Priority, SourceRef, Status, Task, TaskUid, When};
use crate::markdown::mutator::{self, Mutation};
use crate::markdown::todo_view;
use crate::markdown::{self};
use crate::router::{NoteMeta, NoteRouting, Router};
use crate::store::cache as task_cache;
use crate::store::index::{Index, IndexEntry};
use crate::store::outbox::{OutboundOp, Outbox};
use crate::store::tombstones::Tombstones;
use crate::sync::planner::{self, InsertTarget, MarkdownOp, Snapshots};
use crate::TaskresError;

/// Summary of one reconciliation pass (§13.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Routed files parsed this pass.
    pub scanned_files: usize,
    /// Tasks that received a fresh UID.
    pub registered: usize,
    /// Successful `PUT`s.
    pub pushes: usize,
    /// Successful list moves (PUT + DELETE).
    pub moves: usize,
    /// Successful remote deletions.
    pub deletes: usize,
    /// Applied line mutations (including registrations and deletions).
    pub markdown_mutations: usize,
    /// Task lines inserted into the vault.
    pub inserts: usize,
    /// Foreign tasks adopted.
    pub adoptions: usize,
    /// UIDs deferred (in-flight Syncthing writes).
    pub deferred: usize,
    /// CalDAV operations parked to the outbox after failure.
    pub parked: usize,
}

/// One vault scan: routed tasks keyed by UID plus registration counters.
#[derive(Debug, Default)]
struct VaultScan {
    local: BTreeMap<TaskUid, Task>,
    files_scanned: usize,
    registered: usize,
}

/// The always-on reconciler (§13.1). Generic over the [`CaldavPort`] so tests run against
/// an in-memory mock and the daemon/CLI bind the real client.
pub struct Engine<C: CaldavPort> {
    vault: PathBuf,
    state_dir: PathBuf,
    cfg: VaultConfig,
    machine: MachineConfig,
    caldav: C,
    clock: Arc<dyn Clock>,
}

impl<C: CaldavPort> Engine<C> {
    /// Builds an engine over `vault` (containing `restask.toml`-derived `cfg` and the
    /// `.restask/` state directory).
    pub fn new(
        vault: &Path,
        cfg: VaultConfig,
        machine: MachineConfig,
        caldav: C,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            state_dir: vault.join(".restask"),
            vault: vault.to_path_buf(),
            cfg,
            machine,
            caldav,
            clock,
        }
    }

    /// Performs one full reconciliation (§13.1 `run_once` core): flush the outbox, scan and
    /// register the vault, build snapshots, execute the planner's ops, persist state, and
    /// re-render TODO.md when the plan asks for it.
    pub async fn reconcile(&self) -> Result<ReconcileReport, TaskresError> {
        let mut report = ReconcileReport::default();
        let now = self.clock.now_utc();

        // Outbox flush (§9.2): parked ops replay once each; failures re-park.
        let mut outbox = Outbox::load(&self.state_dir)?;
        let pending = outbox.take_all();
        let mut flushed = 0usize;
        let mut put_etags: BTreeMap<TaskUid, String> = BTreeMap::new();
        for op in pending {
            match &op {
                OutboundOp::Put { task } => match self.caldav.put(task).await {
                    Ok(etag) => {
                        put_etags.insert(task.uid.clone(), etag);
                        flushed += 1;
                    }
                    Err(_) => outbox.push(op),
                },
                OutboundOp::Delete { uid, list, etag } => {
                    match self
                        .caldav
                        .delete(list, uid.as_str(), etag.as_deref())
                        .await
                    {
                        Ok(()) => flushed += 1,
                        Err(_) => outbox.push(op),
                    }
                }
            }
        }
        if flushed > 0 {
            tracing::info!(count = flushed, "outbox_flushed");
        }
        outbox.save(&self.state_dir)?;

        // Vault scan + registration.
        let scan = self.scan_vault()?;
        report.scanned_files = scan.files_scanned;
        report.registered = scan.registered;
        tracing::info!(
            files = scan.files_scanned,
            tasks = scan.local.len(),
            "scan_complete"
        );
        let local = scan.local.clone();

        let index = Index::load(&self.state_dir)?;
        let tombstones = Tombstones::load(&self.state_dir)?;
        let cache = self.load_cache(&index)?;
        let cache_had: BTreeSet<TaskUid> = cache.keys().cloned().collect();

        // Collections in scope: every list the vault, the index, or a binding mentions.
        let mut slugs: BTreeSet<ListSlug> = BTreeSet::new();
        for task in local.values() {
            slugs.insert(task.list.clone());
        }
        for entry in index.entries.values() {
            slugs.insert(entry.list.clone());
        }
        for binding in &self.machine.lists {
            if let Ok(slug) = ListSlug::from_name(&binding.collection) {
                slugs.insert(slug);
            }
        }
        if self.machine.caldav.allow_create_lists {
            for slug in &slugs {
                self.caldav
                    .ensure_collection(slug, &slug.display_name())
                    .await?;
            }
        }

        // Remote snapshot with the etags seen at fetch time.
        let mut remote = BTreeMap::new();
        let mut remote_etags: BTreeMap<(ListSlug, String), String> = BTreeMap::new();
        for slug in &slugs {
            let mut per = BTreeMap::new();
            for (name, _etag) in self.caldav.list_etags(slug).await? {
                if let Some((remote_task, fresh)) = self.caldav.fetch(slug, &name).await? {
                    remote_etags.insert((slug.clone(), name.clone()), fresh);
                    per.insert(name, remote_task);
                }
            }
            remote.insert(slug.clone(), per);
        }
        let had_remote: BTreeSet<TaskUid> = remote
            .values()
            .flat_map(|collection| collection.values())
            .filter(|remote_task| remote_task.managed)
            .map(|remote_task| remote_task.task.uid.clone())
            .collect();

        // Plan.
        let snapshots = Snapshots {
            local: local.clone(),
            cache,
            remote,
            tombstones: tombstone_uids(&tombstones, &local, &cache_had, &had_remote),
            index: index.clone(),
        };
        let plan = planner::plan(snapshots, now);

        // Markdown ops first: the vault is the source of truth.
        let mut inbox_inserts: Vec<Task> = Vec::new();
        for op in &plan.markdown_ops {
            match op {
                MarkdownOp::Insert { task, target } => {
                    self.insert_task(task, target)?;
                    report.inserts += 1;
                    if matches!(target, InsertTarget::TodoInbox) {
                        inbox_inserts.push(task.clone());
                    }
                }
                MarkdownOp::Mutate { path, mutations } => {
                    let applied = self.apply_file(path, mutations)?;
                    report.markdown_mutations += applied;
                }
                MarkdownOp::Delete { path, uid } => {
                    self.apply_file(path, &[Mutation::Delete { uid: uid.clone() }])?;
                    report.markdown_mutations += 1;
                    tracing::info!(uid = %uid, path = %path, "task_deleted");
                }
            }
        }

        // CalDAV ops: deletes first (adoption replaces the foreign resource), then moves
        // (PUT before DELETE so the task never vanishes), then plain puts. Failures park
        // to the outbox and never abort the cycle (§10.4).
        let mut parked: Vec<OutboundOp> = Vec::new();
        for op in &plan.caldav_deletes {
            let etag = op.etag.clone().or_else(|| {
                remote_etags
                    .get(&(op.list.clone(), op.name.clone()))
                    .cloned()
            });
            match self
                .caldav
                .delete(&op.list, &op.name, etag.as_deref())
                .await
            {
                Ok(()) => {
                    report.deletes += 1;
                    tracing::info!(list = %op.list.as_str(), name = %op.name, "caldav_delete");
                }
                Err(error) => {
                    tracing::warn!(list = %op.list.as_str(), name = %op.name, %error, "delete failed; parking");
                    if let Ok(uid) = TaskUid::parse(&op.name) {
                        parked.push(OutboundOp::Delete {
                            uid,
                            list: op.list.clone(),
                            etag,
                        });
                    }
                }
            }
        }
        for mv in &plan.caldav_moves {
            match self.caldav.put(&mv.task).await {
                Ok(etag) => {
                    put_etags.insert(mv.task.uid.clone(), etag);
                    report.moves += 1;
                    tracing::info!(uid = %mv.task.uid, from = %mv.from.as_str(), to = %mv.to.as_str(), "task_moved");
                    if let Err(error) = self
                        .caldav
                        .delete(&mv.from, mv.task.uid.as_str(), None)
                        .await
                    {
                        tracing::warn!(%error, "old copy delete failed; parking");
                        parked.push(OutboundOp::Delete {
                            uid: mv.task.uid.clone(),
                            list: mv.from.clone(),
                            etag: None,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(uid = %mv.task.uid, %error, "move put failed; parking");
                    parked.push(OutboundOp::Put {
                        task: mv.task.clone(),
                    });
                }
            }
        }
        for task in &plan.caldav_puts {
            match self.caldav.put(task).await {
                Ok(etag) => {
                    put_etags.insert(task.uid.clone(), etag);
                    report.pushes += 1;
                    tracing::info!(uid = %task.uid, list = %task.list.as_str(), "caldav_push");
                }
                Err(error) => {
                    tracing::warn!(uid = %task.uid, %error, "push failed; parking");
                    parked.push(OutboundOp::Put { task: task.clone() });
                }
            }
        }
        report.parked = parked.len();
        report.adoptions = plan.adoptions.len();
        report.deferred = plan.deferred.len();
        for (uid, reason) in &plan.deferred {
            tracing::warn!(uid = %uid, reason = ?reason, "sync_lag_deferred");
        }

        // State writes: index (with fresh etags), tombstones, cache, outbox.
        let mut index = index;
        for entry in plan.index_upserts {
            index.upsert(entry);
        }
        for uid in &plan.index_removals {
            index.remove(uid);
        }
        for task in &inbox_inserts {
            if let Some(entry) = index.entries.get_mut(&task.uid) {
                entry.source_path = self.cfg.inbox_file.clone();
            }
        }
        for ((_slug, name), etag) in &remote_etags {
            if let Ok(uid) = TaskUid::parse(name) {
                if let Some(entry) = index.entries.get_mut(&uid) {
                    entry.caldav_etag = Some(etag.clone());
                }
            }
        }
        for (uid, etag) in &put_etags {
            if let Some(entry) = index.entries.get_mut(uid) {
                entry.caldav_etag = Some(etag.clone());
            }
        }
        index.save(&self.state_dir)?;

        let mut tombstones = tombstones;
        for uid in &plan.index_removals {
            let was_local = local.contains_key(uid);
            let was_cached_and_remote = cache_had.contains(uid) && had_remote.contains(uid);
            if (was_local || was_cached_and_remote) && !tombstones.contains(uid) {
                tombstones.insert(uid.clone(), now);
            }
        }
        tombstones.save(&self.state_dir)?;
        for task in &plan.cache_writes {
            task_cache::cache_write(&self.state_dir, task, now)?;
        }
        for uid in &plan.cache_deletes {
            task_cache::cache_remove(&self.state_dir, uid)?;
        }
        for op in parked {
            outbox.push(op);
        }
        outbox.save(&self.state_dir)?;

        // TODO.md re-render (R10) over the post-mutation vault, plus any inbox inserts
        // that have not been written to disk yet.
        if plan.todo_refresh {
            let rescan = self.scan_vault()?;
            let mut rendered_tasks = rescan.local;
            for task in &inbox_inserts {
                let mut task = task.clone();
                task.source.path = self.cfg.inbox_file.clone();
                task.source.line = 0;
                rendered_tasks.insert(task.uid.clone(), task);
            }
            let rendered = todo_view::render(&rendered_tasks, self.clock.as_ref(), &self.cfg);
            let todo_path = self.vault.join(&self.cfg.inbox_file);
            let current = std::fs::read_to_string(&todo_path).unwrap_or_default();
            if current != rendered {
                mutator::write_atomic(&todo_path, &rendered)?;
            }
            tracing::info!("todo_rendered");
        }

        Ok(report)
    }

    /// Registers any new tasks in `path` and runs a full reconcile (§13.1 fs-change path;
    /// the daemon debounces before calling this).
    pub async fn handle_fs_change(&self, path: &Path) -> Result<ReconcileReport, TaskresError> {
        tracing::debug!(path = %path.display(), "fs_event");
        self.reconcile().await
    }

    /// Appends a new task to the TODO.md inbox, renders the view, pushes it, and records
    /// it (§13.3 `restask add`).
    pub async fn add(
        &self,
        text: &str,
        priority: Option<Priority>,
        due: Option<When>,
    ) -> Result<Task, TaskresError> {
        let now = self.clock.now_utc();
        let task = Task {
            uid: TaskUid::generate(),
            list: inbox_slug().ok_or_else(|| TaskresError::Validation {
                field: "inbox",
                reason: "cannot slugify the inbox list name".to_string(),
            })?,
            text: text.to_string(),
            status: Status::Active,
            priority,
            due,
            start: None,
            scheduled: None,
            created: Some(self.clock.today_local()),
            parent: None,
            source: SourceRef {
                path: self.cfg.inbox_file.clone(),
                line: 0,
            },
            source_heading: None,
            source_mtime: now,
            last_modified: now,
        };

        let scan = self.scan_vault()?;
        let mut rendered_tasks = scan.local;
        rendered_tasks.insert(task.uid.clone(), task.clone());
        let rendered = todo_view::render(&rendered_tasks, self.clock.as_ref(), &self.cfg);
        mutator::write_atomic(&self.vault.join(&self.cfg.inbox_file), &rendered)?;

        let mut index = Index::load(&self.state_dir)?;
        let mut outbox = Outbox::load(&self.state_dir)?;
        let mut etag_opt = None;
        match self.caldav.put(&task).await {
            Ok(etag) => {
                tracing::info!(uid = %task.uid, list = %task.list.as_str(), "caldav_push");
                etag_opt = Some(etag);
            }
            Err(error) => {
                tracing::warn!(uid = %task.uid, %error, "push failed; parking");
                outbox.push(OutboundOp::Put { task: task.clone() });
            }
        }
        index.upsert(IndexEntry {
            uid: task.uid.clone(),
            list: task.list.clone(),
            source_path: self.cfg.inbox_file.clone(),
            thumbprint: task.thumbprint(),
            caldav_etag: etag_opt,
            seen_at: now,
            defer_count: 0,
        });
        index.save(&self.state_dir)?;
        outbox.save(&self.state_dir)?;
        task_cache::cache_write(&self.state_dir, &task, now)?;
        Ok(task)
    }

    /// Completes or reopens a task: applies the status mutation plus the done-region
    /// move/restore to its source file, then runs a full reconcile (§13.3 `restask
    /// done/undone`).
    pub async fn set_done(&self, uid: &TaskUid, done: bool) -> Result<Task, TaskresError> {
        let scan = self.scan_vault()?;
        let task = scan
            .local
            .get(uid)
            .cloned()
            .ok_or_else(|| TaskresError::Validation {
                field: "uid",
                reason: format!("task {uid} not found in the vault"),
            })?;
        let ops = if done {
            tracing::info!(uid = %uid, "task_completed");
            vec![
                Mutation::SetStatus {
                    uid: uid.clone(),
                    checked: true,
                    completed_on: Some(self.clock.today_local()),
                },
                Mutation::MoveToDone { uid: uid.clone() },
            ]
        } else {
            tracing::info!(uid = %uid, "task_restored");
            vec![
                Mutation::RestoreFromDone { uid: uid.clone() },
                Mutation::SetStatus {
                    uid: uid.clone(),
                    checked: false,
                    completed_on: None,
                },
            ]
        };
        self.apply_file(&task.source.path, &ops)?;
        self.reconcile().await?;
        Ok(task)
    }

    // ── internals ─────────────────────────────────────────────────────────────────────

    /// Walks the vault (sorted, tracked files only), routes every note, registers
    /// unregistered task lines, and builds the local task set.
    fn scan_vault(&self) -> Result<VaultScan, TaskresError> {
        let matchers = self.cfg.matchers().map_err(|error| TaskresError::Config {
            path: self.vault.join("restask.toml").display().to_string(),
            reason: error.to_string(),
        })?;
        let mut files: Vec<(String, String)> = Vec::new();
        walk(&self.vault, "", &matchers, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let metas: Vec<NoteMeta> = files
            .iter()
            .map(|(path, contents)| {
                let (file_list, folder_list) = crate::router::scan_frontmatter(contents);
                NoteMeta {
                    path: path.to_string(),
                    file_list,
                    folder_list,
                }
            })
            .collect();
        let router = Router::build(&metas)?;

        let mut scan = VaultScan::default();
        let mut seen: BTreeMap<TaskUid, String> = BTreeMap::new();
        // Notes first, then the inbox file: TODO.md mirror lines lose to their source
        // note's line, so the rendered view never shadows the vault.
        let order: Vec<usize> = {
            let mut notes: Vec<usize> = (0..files.len())
                .filter(|&i| files[i].0 != self.cfg.inbox_file)
                .collect();
            let inbox: Vec<usize> = (0..files.len())
                .filter(|&i| files[i].0 == self.cfg.inbox_file)
                .collect();
            notes.extend(inbox);
            notes
        };
        for i in order {
            let (path, contents) = &files[i];
            let meta = &metas[i];
            let routing = if path.as_str() == self.cfg.inbox_file {
                NoteRouting::List(inbox_slug().ok_or_else(|| TaskresError::Validation {
                    field: "inbox",
                    reason: "cannot slugify the inbox list name".to_string(),
                })?)
            } else {
                router.resolve(path, meta.file_list.as_deref())
            };
            let NoteRouting::List(list) = routing else {
                continue;
            };
            scan.files_scanned += 1;
            self.scan_file(path, contents, &list, &mut scan, &mut seen)?;
        }
        Ok(scan)
    }

    /// Parses one routed note, registers unregistered lines, and records its tasks.
    fn scan_file(
        &self,
        path: &str,
        contents: &str,
        list: &ListSlug,
        scan: &mut VaultScan,
        seen: &mut BTreeMap<TaskUid, String>,
    ) -> Result<(), TaskresError> {
        let parsed = markdown::parse(contents, &self.cfg);
        let registers: Vec<Mutation> = parsed
            .tasks
            .iter()
            .filter(|task| task.draft.uid.is_none())
            .map(|task| Mutation::Register {
                line_no: task.line_no,
                uid: TaskUid::generate(),
                created: self.clock.today_local(),
            })
            .collect();
        let parsed = if registers.is_empty() {
            parsed
        } else {
            let out = mutator::apply(contents, &registers, &self.cfg, self.clock.as_ref())?;
            scan.registered += out.applied.len();
            for mutation in &out.applied {
                if let Mutation::Register { uid, .. } = mutation {
                    tracing::info!(uid = %uid, path = %path, "task_registered");
                }
            }
            mutator::write_atomic(&self.vault.join(path), &out.contents)?;
            markdown::parse(&out.contents, &self.cfg)
        };

        let parents = crate::markdown::parser::link_parents(&parsed.tasks);
        let mtime = file_mtime(&self.vault.join(path))?;
        let inbox_file = self.cfg.inbox_file.clone();
        for (task, parent) in parsed.tasks.iter().zip(parents) {
            // Mirror lines in the inbox file are rendered views of note tasks, never
            // sources: skip wikilink-bearing lines and any UID already claimed by a note.
            if path == inbox_file
                && (task.raw.contains("[[")
                    || task
                        .draft
                        .uid
                        .as_ref()
                        .is_some_and(|uid| seen.contains_key(uid)))
            {
                continue;
            }
            let Some(uid) = task.draft.uid.clone() else {
                continue;
            };
            if let Some(first) = seen.insert(uid.clone(), path.to_string()) {
                return Err(TaskresError::UidConflict {
                    uid,
                    a: first,
                    b: path.to_string(),
                });
            }
            let status = if task.in_done_region || task.draft.checked {
                Status::Completed {
                    on: task
                        .draft
                        .completed_on
                        .unwrap_or_else(|| self.clock.today_local()),
                }
            } else {
                Status::Active
            };
            scan.local.insert(
                uid.clone(),
                Task {
                    uid,
                    list: list.clone(),
                    text: task.draft.text.clone(),
                    status,
                    priority: task.draft.priority,
                    due: task.draft.due,
                    start: task.draft.start,
                    scheduled: task.draft.scheduled,
                    created: task.draft.created,
                    parent,
                    source: SourceRef {
                        path: path.to_string(),
                        line: task.line_no,
                    },
                    source_heading: task.heading.clone(),
                    source_mtime: mtime,
                    last_modified: mtime,
                },
            );
        }
        Ok(())
    }

    /// Loads the VTODO cache (`.restask/tasks/*.ics`), overwriting each task's list from
    /// the index (routing truth, D30).
    fn load_cache(&self, index: &Index) -> Result<BTreeMap<TaskUid, Task>, TaskresError> {
        let mut map = BTreeMap::new();
        let entries = match std::fs::read_dir(self.state_dir.join("tasks")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(raw) = name.strip_suffix(".ics") else {
                continue;
            };
            let Ok(uid) = TaskUid::parse(raw) else {
                continue;
            };
            if let Some(mut task) =
                task_cache::cache_read(&self.state_dir, &uid, self.clock.local_offset())
            {
                if let Some(entry) = index.get(&uid) {
                    task.list = entry.list.clone();
                }
                map.insert(uid, task);
            }
        }
        Ok(map)
    }

    /// Applies line mutations to one vault file (single pass) and writes it back
    /// atomically. Returns the number of applied mutations.
    fn apply_file(&self, path: &str, mutations: &[Mutation]) -> Result<usize, TaskresError> {
        let file = self.vault.join(path);
        let contents = std::fs::read_to_string(&file)?;
        let out = mutator::apply(&contents, mutations, &self.cfg, self.clock.as_ref())?;
        mutator::write_atomic(&file, &out.contents)?;
        Ok(out.applied.len())
    }

    /// Executes one [`MarkdownOp::Insert`] against the vault.
    fn insert_task(&self, task: &Task, target: &InsertTarget) -> Result<(), TaskresError> {
        match target {
            // The line materializes through the TODO.md re-render (R10).
            InsertTarget::TodoInbox => Ok(()),
            InsertTarget::FileEnd { path } => {
                let file = self.vault.join(path);
                let contents = std::fs::read_to_string(&file)?;
                let ending = dominant_ending(&contents);
                let mut out = contents;
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&todo_view::mirror_line(task));
                out.push_str(ending);
                mutator::write_atomic(&file, &out)
            }
            InsertTarget::UnderParent {
                path, after_uid, ..
            } => {
                let file = self.vault.join(path);
                let contents = std::fs::read_to_string(&file)?;
                let parsed = markdown::parse(&contents, &self.cfg);
                let parent = parsed
                    .tasks
                    .iter()
                    .find(|task| task.draft.uid.as_ref() == Some(after_uid))
                    .ok_or_else(|| TaskresError::Validation {
                        field: "parent",
                        reason: format!("parent {after_uid} not found in {path}"),
                    })?;
                let indent = " ".repeat(parent.indent_chars + 2);
                let ending = dominant_ending(&contents);
                let mut out = String::with_capacity(contents.len() + 128);
                for (index, line) in split_physical(&contents).iter().enumerate() {
                    out.push_str(&line.0);
                    out.push_str(line.1);
                    if index + 1 == parent.line_no {
                        out.push_str(&indent);
                        out.push_str(&todo_view::mirror_line(task));
                        out.push_str(ending);
                    }
                }
                mutator::write_atomic(&file, &out)
            }
        }
    }
}

/// A physical line with its terminator.
type PhysicalLine = (String, &'static str);

/// Splits `contents` into `(text, terminator)` pairs, keeping each line's own ending.
fn split_physical(contents: &str) -> Vec<PhysicalLine> {
    let mut lines = Vec::new();
    let mut rest = contents;
    while let Some(nl) = rest.find('\n') {
        let (text, tail) = rest.split_at(nl);
        match text.strip_suffix('\r') {
            Some(stripped) => lines.push((stripped.to_string(), "\r\n")),
            None => lines.push((text.to_string(), "\n")),
        }
        rest = &tail[1..];
    }
    if !rest.is_empty() {
        lines.push((rest.to_string(), ""));
    }
    lines
}

/// The majority line terminator of `contents` (CRLF wins when any CRLF is present).
fn dominant_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// File mtime as a UTC instant.
fn file_mtime(path: &Path) -> Result<DateTime<Utc>, TaskresError> {
    let modified = std::fs::metadata(path)?.modified()?;
    Ok(modified.into())
}

/// The engine-managed inbox list (§5.2): TODO.md routes to `Inbox` → `inbox`.
fn inbox_slug() -> Option<ListSlug> {
    static SLUG: std::sync::OnceLock<Option<ListSlug>> = std::sync::OnceLock::new();
    SLUG.get_or_init(|| ListSlug::from_name("Inbox").ok())
        .clone()
}

/// Recursively collects tracked files as `(vault-relative path, contents)` pairs.
fn walk(
    dir: &Path,
    relative: &str,
    matchers: &VaultMatchers,
    out: &mut Vec<(String, String)>,
) -> Result<(), TaskresError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &child, matchers, out)?;
        } else if matchers.is_tracked(&child) {
            match std::fs::read_to_string(entry.path()) {
                Ok(contents) => out.push((child, contents)),
                Err(error) => {
                    tracing::warn!(path = %child, %error, "unreadable vault file skipped");
                }
            }
        }
    }
    Ok(())
}

/// Tombstone membership as the planner sees it: a UID in the tombstone file that can
/// still appear in the snapshots (the local, cache, and remote union).
fn tombstone_uids(
    tombstones: &Tombstones,
    local: &BTreeMap<TaskUid, Task>,
    cache_keys: &BTreeSet<TaskUid>,
    remote_uids: &BTreeSet<TaskUid>,
) -> BTreeSet<TaskUid> {
    let mut uids = BTreeSet::new();
    for uid in local.keys().chain(cache_keys).chain(remote_uids) {
        if tombstones.contains(uid) {
            uids.insert(uid.clone());
        }
    }
    uids
}
