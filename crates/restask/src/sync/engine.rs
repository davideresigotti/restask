//! Reconciliation engine (§11, §13.1): the single-writer I/O orchestrator. Owns every
//! mutation of the vault, `.restask/` state, and the server; the planner decides, the
//! engine executes and records.
//!
//! One pass has three phases, ordered so that a crash at any point is repaired by the
//! next pass:
//!
//! 1. **Local** — scan and repair the vault, carry TODO.md edits to their source notes.
//!    Needs no server; always runs.
//! 2. **Remote** — snapshot the server, plan, apply vault edits, then server writes.
//! 3. **Record** — re-render TODO.md, then persist the state. The vault is written before
//!    the state that describes it, so the state never claims a line the vault lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use crate::caldav::{list_name, resolve_list, Bound, CaldavPort, RemoteResource};
use crate::config::{MachineConfig, VaultConfig};
use crate::domain::{
    Clock, ListSlug, LocalDate, Priority, Recurrence, SourceRef, Status, Task, TaskUid, When,
};
use crate::fsio;
use crate::markdown::mutator::{self, Mutation};
use crate::markdown::todo_view;
use crate::store::cache as base_store;
use crate::store::calendars::Calendars;
use crate::store::index::{Index, IndexEntry};
use crate::store::tombstones::Tombstones;
use crate::sync::planner::{self, DeleteOp, Plan, PutOp, Snapshots, DEFER_LIMIT};
use crate::vault::{self, Scan, ScanMode, STATE_DIR};
use crate::{CaldavErrorKind, RestaskError};

/// The engine's own last render of TODO.md, kept under `.restask/`. Comparing the live
/// file against it tells what the user edited in the view (§7).
pub const RENDERED_FILE: &str = "todo.rendered.md";

/// Tombstones older than this are pruned (§9.1).
const TOMBSTONE_TTL_DAYS: i64 = 365;

/// Summary of one reconciliation pass (§13.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Routed files parsed this pass.
    pub scanned_files: usize,
    /// Task lines that received a fresh UID.
    pub registered: usize,
    /// Hand edits normalized: checked boxes moved to the done region, reopened ones
    /// restored, duplicated UIDs reassigned, TODO.md edits carried to their notes.
    pub normalized: usize,
    /// Successful `PUT`s of new or changed tasks.
    pub pushes: usize,
    /// Successful list moves (PUT + DELETE).
    pub moves: usize,
    /// Successful remote deletions.
    pub deletes: usize,
    /// Vault line mutations applied from the plan (inserts and deletions included).
    pub markdown_mutations: usize,
    /// Task lines created in the vault for tasks that came from the server.
    pub inserts: usize,
    /// Tasks another client created that got their line in the vault.
    pub adoptions: usize,
    /// UIDs deferred (vault file older than its base).
    pub deferred: usize,
    /// Server operations that failed; the next pass re-plans them.
    pub failed: usize,
}

/// The always-on reconciler (§13.1). Generic over the [`CaldavPort`] so tests run against
/// an in-memory mock and the daemon/CLI bind the real client.
pub struct Engine<C: CaldavPort> {
    vault: PathBuf,
    state_dir: PathBuf,
    cfg: VaultConfig,
    allow_create_lists: bool,
    /// `false` on a machine that leaves the syncing to the vault's sync node (§1.1): a
    /// command that changed the vault here settles it and does not start a pass.
    syncs_here: bool,
    caldav: C,
    clock: Arc<dyn Clock>,
}

/// Name of the advisory lock file under `.restask/`.
const LOCK_FILE: &str = "lock";

/// How long a command waits for another restask process working on the same vault.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Exclusive advisory lock on the vault for this machine: the daemon and a CLI command
/// (or an editor integration calling one) never interleave their writes. Released when
/// dropped. Other devices are coordinated by the merge, not by this lock.
struct VaultLock(#[allow(dead_code)] std::fs::File);

/// What the remote phase got done before it returned (possibly with an error).
#[derive(Default)]
struct Progress {
    /// The plan being executed; `None` when the server snapshot could not be taken.
    plan: Option<Plan>,
    /// Whether plan mutations touched vault files.
    vault_changed: bool,
    /// Tasks whose server write succeeded, with the new etag.
    pushed: Vec<(Task, String)>,
}

impl<C: CaldavPort> Engine<C> {
    /// Builds an engine over `vault` (holding `restask.toml`-derived `cfg` and the
    /// `.restask/` state directory).
    pub fn new(
        vault: &Path,
        cfg: VaultConfig,
        machine: MachineConfig,
        caldav: C,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            state_dir: vault.join(STATE_DIR),
            vault: vault.to_path_buf(),
            cfg,
            allow_create_lists: machine.caldav.allow_create_lists,
            syncs_here: machine.node.is_none(),
            caldav,
            clock,
        }
    }

    /// The vault config the engine works from.
    pub fn config(&self) -> &VaultConfig {
        &self.cfg
    }

    /// Replaces the vault config for the passes that follow: `restask.toml` is a file
    /// of the vault and changes like one — edited by hand, or arriving through the file
    /// sync — while the daemon runs (§13.1).
    pub fn set_config(&mut self, cfg: VaultConfig) {
        self.cfg = cfg;
    }

    /// Performs one full reconciliation (§13.1). When the server cannot be reached the
    /// local phase and the TODO.md render still happen, and the error is returned after.
    pub async fn reconcile(&self) -> Result<ReconcileReport, RestaskError> {
        let _lock = self.lock().await?;
        self.reconcile_locked().await
    }

    /// Waits for the vault lock (see [`VaultLock`]).
    async fn lock(&self) -> Result<VaultLock, RestaskError> {
        std::fs::create_dir_all(&self.state_dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.state_dir.join(LOCK_FILE))?;
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(VaultLock(file)),
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(RestaskError::Validation {
                        field: "vault",
                        reason: "another restask process is still working on this vault"
                            .to_string(),
                    })
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    async fn reconcile_locked(&self) -> Result<ReconcileReport, RestaskError> {
        let now = self.clock.now_utc();
        let mut report = ReconcileReport::default();
        let mut index = Index::load(&self.state_dir)?;
        let mut tombstones = Tombstones::load(&self.state_dir)?;
        // Left over from versions that queued server writes; the planner re-derives them.
        let _ = std::fs::remove_file(self.state_dir.join("outbox.json"));

        // 1 — local.
        let scan = self.scan_local(&index, &mut report)?;
        tracing::info!(
            files = scan.files_scanned,
            tasks = scan.local.len(),
            "scan_complete"
        );

        // 2 — remote.
        let mut progress = Progress::default();
        let outcome = self
            .sync_remote(&scan, &index, &tombstones, now, &mut progress, &mut report)
            .await;

        // 3 — record: the view first, then the state.
        let mut tasks = if progress.vault_changed {
            vault::scan(
                &self.vault,
                &self.cfg,
                self.clock.as_ref(),
                &index,
                ScanMode::ReadOnly,
            )?
            .local
        } else {
            scan.local
        };
        if let Some(plan) = &progress.plan {
            for task in &plan.inbox_inserts {
                tasks
                    .entry(task.uid.clone())
                    .or_insert_with(|| task.clone());
            }
        }
        self.render(&tasks)?;
        if let Some(plan) = progress.plan {
            self.record(plan, progress.pushed, &mut index, &mut tombstones, now)?;
        }

        outcome.map(|()| report)
    }

    /// Does the local work of a pass and nothing else (§11.1 phase 1, then the render):
    /// what an editor integration runs when a note was written (§13.3 `restask settle`).
    /// The server is not contacted and no sync state is written, so it works offline and
    /// leaves the vault as the daemon's pass would — which then finds nothing to do in it.
    pub async fn settle(&self) -> Result<ReconcileReport, RestaskError> {
        let _lock = self.lock().await?;
        self.settle_locked()
    }

    fn settle_locked(&self) -> Result<ReconcileReport, RestaskError> {
        let mut report = ReconcileReport::default();
        let index = Index::load(&self.state_dir)?;
        let scan = self.scan_local(&index, &mut report)?;
        self.render(&scan.local)?;
        Ok(report)
    }

    /// Appends a new task to the TODO.md inbox and syncs (§13.3 `restask add`). The task
    /// is safe in the vault even when the server is unreachable; a machine that is not
    /// the sync node only settles the vault.
    pub async fn add(
        &self,
        text: &str,
        priority: Option<Priority>,
        due: Option<When>,
        recurrence: Option<Recurrence>,
    ) -> Result<Task, RestaskError> {
        let _lock = self.lock().await?;
        let now = self.clock.now_utc();
        let index = Index::load(&self.state_dir)?;
        // The view is rendered from this scan: what the user edited in it comes first.
        let scan = self.scan_local(&index, &mut ReconcileReport::default())?;
        let task = Task {
            uid: TaskUid::generate(),
            list: vault::inbox_list(&self.cfg)?,
            text: text.split_whitespace().collect::<Vec<_>>().join(" "),
            status: Status::Active,
            priority,
            due,
            start: None,
            scheduled: None,
            recurrence,
            created: None,
            parent: None,
            source: SourceRef {
                path: self.cfg.inbox_file.clone(),
                line: 0,
            },
            source_heading: None,
            source_mtime: now,
            last_modified: now,
        };
        let mut tasks = scan.local;
        tasks.insert(task.uid.clone(), task.clone());
        self.render(&tasks)?;
        self.sync_after_local_change().await?;
        Ok(task)
    }

    /// Completes or reopens a task in its source file, then syncs (§13.3 `restask
    /// done/undone`). The change is safe in the vault even when the server is unreachable;
    /// a machine that is not the sync node only settles the vault.
    pub async fn set_done(&self, uid: &TaskUid, done: bool) -> Result<Task, RestaskError> {
        let _lock = self.lock().await?;
        let index = Index::load(&self.state_dir)?;
        let scan = vault::scan(
            &self.vault,
            &self.cfg,
            self.clock.as_ref(),
            &index,
            ScanMode::Repair,
        )?;
        let task = scan
            .local
            .get(uid)
            .cloned()
            .ok_or_else(|| RestaskError::Validation {
                field: "uid",
                reason: format!("task {uid} not found in the vault"),
            })?;
        let is_done = matches!(task.status, Status::Completed { .. });
        if is_done != done {
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
        }
        self.sync_after_local_change().await?;
        Ok(task)
    }

    // ── phase 1: local ────────────────────────────────────────────────────────────────

    /// Scans and repairs the vault, then carries edits made on TODO.md mirror lines to
    /// their source notes and gives lines moved to another section of TODO.md their new
    /// priority (rescanning when that changed anything).
    fn scan_local(
        &self,
        index: &Index,
        report: &mut ReconcileReport,
    ) -> Result<Scan, RestaskError> {
        let scan = |report: &mut ReconcileReport| -> Result<Scan, RestaskError> {
            let scan = vault::scan(
                &self.vault,
                &self.cfg,
                self.clock.as_ref(),
                index,
                ScanMode::Repair,
            )?;
            report.registered += scan.registered;
            report.normalized += scan.normalized;
            report.scanned_files = scan.files_scanned;
            Ok(scan)
        };
        let first = scan(report)?;
        let mut edits = match (
            std::fs::read_to_string(self.vault.join(&self.cfg.inbox_file)),
            std::fs::read_to_string(self.state_dir.join(RENDERED_FILE)),
        ) {
            (Ok(current), Ok(rendered)) => todo_view::mirror_edits(
                &current,
                &rendered,
                &first.local,
                &self.cfg,
                self.clock.today_local(),
            ),
            _ => BTreeMap::new(),
        };
        // A line that asks for its creation date gets it here, before anything renders
        // or pushes the task (§6.4).
        for (uid, path) in &first.created_requests {
            edits
                .entry(path.clone())
                .or_default()
                .push(Mutation::SetCreated {
                    uid: uid.clone(),
                    created: self.created_on(uid, index),
                });
        }
        if edits.is_empty() {
            return Ok(first);
        }
        for (path, ops) in &edits {
            let applied = self.apply_file(path, ops)?;
            tracing::info!(path = %path, count = applied, "edit applied to its task");
        }
        report.normalized += edits.len();
        scan(report)
    }

    // ── phase 2: remote ───────────────────────────────────────────────────────────────

    /// Snapshots the server, plans, and executes: vault edits first (the vault is the
    /// source of truth), then server writes. Progress survives an early error.
    async fn sync_remote(
        &self,
        scan: &Scan,
        index: &Index,
        tombstones: &Tombstones,
        now: DateTime<Utc>,
        progress: &mut Progress,
        report: &mut ReconcileReport,
    ) -> Result<(), RestaskError> {
        let inbox_list = vault::inbox_list(&self.cfg)?;
        let todo_lists = vault::todo_lists(&self.cfg)?;
        let (remote, created, bound) = self
            .remote_snapshot(scan, index, &inbox_list, &todo_lists)
            .await?;
        let snapshots = Snapshots {
            local: scan.local.clone(),
            base: self.load_base(index)?,
            remote,
            created,
            tombstones: tombstones.uids().cloned().collect(),
            index: index.clone(),
            notes: scan.notes.clone(),
            homes: scan.homes.clone(),
            unreadable: scan.unreadable.clone(),
            inbox_file: self.cfg.inbox_file.clone(),
            inbox_list: Some(inbox_list),
            todo_lists,
            obsidian_vault: self.cfg.obsidian_vault.clone(),
        };
        let plan = progress.plan.insert(planner::plan(&snapshots));

        report.deferred = plan.deferred.len();
        report.adoptions = plan.adopted.len();
        report.inserts = plan.inbox_inserts.len();
        for (path, mutations) in &plan.mutations {
            report.inserts += mutations
                .iter()
                .filter(|m| matches!(m, Mutation::Insert { .. }))
                .count();
            report.markdown_mutations += self.apply_file(path, mutations)?;
            progress.vault_changed = true;
        }

        // Server writes. A connection-level failure ends the phase (retrying every
        // remaining operation would only burn the retry budget); anything else — a
        // stale etag, a rejected body — fails that one operation and the next pass
        // re-plans it from a fresh snapshot.
        let mut fatal: Option<RestaskError> = None;
        let mut failed = |error: RestaskError, what: &str, report: &mut ReconcileReport| {
            tracing::warn!(%error, "{what} failed; the next pass retries");
            report.failed += 1;
            if is_connection_failure(&error) {
                fatal = Some(error);
                true
            } else {
                false
            }
        };

        'ops: {
            for put in &plan.puts {
                match self.put(put, &bound, now).await {
                    Ok(etag) => {
                        report.pushes += 1;
                        progress.pushed.push((put.task.clone(), etag));
                    }
                    Err(error) => {
                        if failed(error, "push", report) {
                            break 'ops;
                        }
                    }
                }
            }
            for mv in &plan.moves {
                match self.put(&mv.put, &bound, now).await {
                    Ok(etag) => {
                        report.moves += 1;
                        progress.pushed.push((mv.put.task.clone(), etag));
                        if let Err(error) = self.delete(&mv.from, &bound).await {
                            if failed(error, "removing the moved task's old copy", report) {
                                break 'ops;
                            }
                        }
                    }
                    Err(error) => {
                        if failed(error, "list move", report) {
                            break 'ops;
                        }
                    }
                }
            }
            for delete in &plan.deletes {
                match self.delete(delete, &bound).await {
                    Ok(()) => report.deletes += 1,
                    Err(error) => {
                        if failed(error, "remote delete", report) {
                            break 'ops;
                        }
                    }
                }
            }
        }
        match fatal {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Lists every collection in scope with one `REPORT` each: the inbox list and the
    /// further lists the inbox file shows (§7.5), every list a note routes to or a line
    /// of the inbox file names, and every list the index still references (so moves and
    /// deletions see the old copy). A list is first looked for at its own path; when
    /// nothing is there, among the server's calendars by name (§5.4) — one another
    /// client made has a path of its own. Only a routed list that no calendar answers
    /// to is created, when `caldav.allow_create_lists` is set; otherwise its tasks
    /// simply wait. A list two calendars answer to is not listed: unknown, not empty.
    ///
    /// Returns the snapshot, the lists whose collection is not the one of the pass
    /// before — just created, or found at another path than last time, so what is
    /// missing there was never deleted — and where the lists found by name are.
    async fn remote_snapshot(
        &self,
        scan: &Scan,
        index: &Index,
        inbox_list: &ListSlug,
        todo_lists: &BTreeSet<ListSlug>,
    ) -> Result<
        (
            BTreeMap<ListSlug, Vec<RemoteResource>>,
            BTreeSet<ListSlug>,
            BTreeMap<ListSlug, String>,
        ),
        RestaskError,
    > {
        let mut routed: BTreeSet<ListSlug> = scan.notes.values().cloned().collect();
        routed.insert(inbox_list.clone());
        routed.extend(todo_lists.iter().cloned());
        routed.extend(scan.local.values().map(|task| task.list.clone()));
        let mut scope = routed.clone();
        scope.extend(index.entries.values().map(|entry| entry.list.clone()));

        let before = Calendars::load(&self.state_dir)?;
        let mut calendars = before.clone();
        let mut listing = None;
        let mut remote = BTreeMap::new();
        let mut created = BTreeSet::new();
        let mut bound = BTreeMap::new();
        for slug in scope {
            if let Some(resources) = self.caldav.list_tasks(slug.as_str(), &slug).await? {
                // At its own path — also when it was found elsewhere before.
                if calendars.bound.remove(&slug).is_some() {
                    tracing::warn!(list = %slug.as_str(), "collection_changed");
                    created.insert(slug.clone());
                }
                remote.insert(slug, resources);
                continue;
            }
            let collections = match &listing {
                Some(collections) => collections,
                None => listing.insert(self.caldav.list_collections().await?),
            };
            match resolve_list(collections, &slug) {
                Bound::At(collection) => {
                    // Gone between the two requests: not listed, so nothing is concluded.
                    let Some(resources) = self.caldav.list_tasks(&collection, &slug).await? else {
                        continue;
                    };
                    if before
                        .bound
                        .get(&slug)
                        .is_some_and(|earlier| *earlier != collection)
                    {
                        tracing::warn!(list = %slug.as_str(), "collection_changed");
                        created.insert(slug.clone());
                    }
                    calendars.bound.insert(slug.clone(), collection.clone());
                    bound.insert(slug.clone(), collection);
                    remote.insert(slug, resources);
                }
                Bound::Ambiguous(found) => {
                    tracing::warn!(
                        list = %slug.as_str(),
                        calendars = %found.join(", "),
                        "several calendars of the server have this name; its tasks stay local until one is renamed"
                    );
                }
                Bound::Missing if !routed.contains(&slug) => {}
                Bound::Missing if self.allow_create_lists => {
                    self.caldav
                        .ensure_collection(&slug, &slug.display_name())
                        .await?;
                    tracing::info!(list = %slug.as_str(), "collection_created");
                    // restask's own: not a calendar that appeared on the server (§7.5).
                    if let Some(known) = &mut calendars.known {
                        known.insert(slug.as_str().to_string());
                    }
                    calendars.bound.remove(&slug);
                    created.insert(slug.clone());
                    remote.insert(slug, Vec::new());
                }
                Bound::Missing => {
                    tracing::warn!(
                        list = %slug.as_str(),
                        "no such collection on the server and caldav.allow_create_lists is off; its tasks stay local"
                    );
                }
            }
        }
        if calendars != before {
            calendars.save(&self.state_dir)?;
        }
        Ok((remote, created, bound))
    }

    async fn put(
        &self,
        put: &PutOp,
        bound: &BTreeMap<ListSlug, String>,
        now: DateTime<Utc>,
    ) -> Result<String, RestaskError> {
        let etag = self
            .caldav
            .put(
                &put.task,
                collection_of(bound, &put.task.list),
                &put.name,
                &put.extras,
                &put.wire,
                put.if_match.as_deref(),
                now,
            )
            .await?;
        tracing::info!(uid = %put.task.uid, list = %put.task.list.as_str(), "caldav_push");
        Ok(etag)
    }

    async fn delete(
        &self,
        delete: &DeleteOp,
        bound: &BTreeMap<ListSlug, String>,
    ) -> Result<(), RestaskError> {
        self.caldav
            .delete(
                collection_of(bound, &delete.list),
                &delete.name,
                delete.etag.as_deref(),
            )
            .await?;
        tracing::info!(list = %delete.list.as_str(), name = %delete.name, "caldav_delete");
        Ok(())
    }

    /// Adds the calendars that appeared on the server to the ones TODO.md shows
    /// (§7.5): a calendar that can hold tasks, was not there at the look before, and is
    /// neither the bound one nor shown already, is appended to `vault.todo_lists` under
    /// the name the vault reaches it by — `restask.toml` is written, then what was seen.
    /// The first look only remembers: without a look before, nothing is new. Server
    /// work, so the sync node's alone; off with `todo_new_lists = false`. Returns the
    /// names added.
    pub async fn follow_calendars(&mut self) -> Result<Vec<String>, RestaskError> {
        if !self.syncs_here || !self.cfg.todo_new_lists {
            return Ok(Vec::new());
        }
        let collections = self.caldav.list_collections().await?;
        let _lock = self.lock().await?;
        let mut calendars = Calendars::load(&self.state_dir)?;
        let seen: BTreeSet<String> = collections
            .iter()
            .map(|collection| collection.slug.clone())
            .collect();
        let Some(known) = calendars.known.clone() else {
            calendars.known = Some(seen);
            calendars.save(&self.state_dir)?;
            return Ok(Vec::new());
        };
        let inbox_list = vault::inbox_list(&self.cfg)?;
        let mut shown = vault::todo_lists(&self.cfg)?;
        let mut cfg = self.cfg.clone();
        let mut added = Vec::new();
        for collection in &collections {
            if known.contains(&collection.slug) || !collection.supports_vtodo {
                continue;
            }
            let Some(name) = list_name(collection, &collections) else {
                continue;
            };
            if name != inbox_list && shown.insert(name.clone()) {
                cfg.todo_lists.push(name.as_str().to_string());
                added.push(name.as_str().to_string());
            }
        }
        if !added.is_empty() {
            let path = self.vault.join("restask.toml");
            cfg.save(&path).map_err(|error| RestaskError::Config {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
            tracing::info!(calendars = %added.join(", "), "calendars_added");
        }
        if known != seen {
            calendars.known = Some(known.union(&seen).cloned().collect());
            calendars.save(&self.state_dir)?;
        }
        drop(_lock);
        self.cfg = cfg;
        Ok(added)
    }

    // ── phase 3: record ───────────────────────────────────────────────────────────────

    /// Renders TODO.md (§7) and remembers the render; neither file is touched when its
    /// content is unchanged.
    fn render(&self, tasks: &BTreeMap<TaskUid, Task>) -> Result<(), RestaskError> {
        let todo = self.vault.join(&self.cfg.inbox_file);
        let rendered = todo_view::render(tasks, &self.cfg);
        if let Some(parent) = todo.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if fsio::write_if_changed(&todo, &rendered)? {
            tracing::info!("todo_rendered");
        }
        std::fs::create_dir_all(&self.state_dir)?;
        fsio::write_if_changed(&self.state_dir.join(RENDERED_FILE), &rendered)?;
        Ok(())
    }

    /// Persists what the pass established: bases for settled and pushed tasks, defer
    /// counters, forgotten UIDs, tombstones.
    fn record(
        &self,
        plan: Plan,
        pushed: Vec<(Task, String)>,
        index: &mut Index,
        tombstones: &mut Tombstones,
        now: DateTime<Utc>,
    ) -> Result<(), RestaskError> {
        let settled = plan
            .settled
            .into_iter()
            .map(|settled| (settled.task, settled.etag))
            .chain(pushed);
        for (task, etag) in settled {
            base_store::cache_write(&self.state_dir, &task, now)?;
            index.upsert(IndexEntry {
                thumbprint: task.thumbprint(),
                uid: task.uid,
                list: task.list,
                source_path: task.source.path,
                caldav_etag: Some(etag),
                seen_at: now,
                defer_count: 0,
            });
        }
        for (uid, reason) in &plan.deferred {
            tracing::warn!(uid = %uid, reason = ?reason, "sync_lag_deferred");
            if let Some(entry) = index.entries.get_mut(uid) {
                entry.defer_count = entry.defer_count.saturating_add(1);
                if entry.defer_count >= DEFER_LIMIT {
                    tracing::error!(uid = %uid, "vault_divergence: taking the vault copy next pass");
                }
            }
        }
        for uid in &plan.forgets {
            base_store::cache_remove(&self.state_dir, uid)?;
            index.remove(uid);
        }
        for uid in plan.tombstones {
            tombstones.insert(uid, now);
        }
        for uid in &plan.revived {
            tombstones.remove(uid);
        }
        tombstones.prune(Duration::days(TOMBSTONE_TTL_DAYS), now);
        index.save(&self.state_dir)?;
        tombstones.save(&self.state_dir)?;
        Ok(())
    }

    // ── helpers ───────────────────────────────────────────────────────────────────────

    /// Runs a reconcile after a local command already changed the vault: a server that
    /// cannot be reached is reported, not fatal — the change is saved and syncs later.
    /// On a machine that is not the sync node the vault is settled instead: the pass is
    /// the node's, once the file sync has carried the change there (§1.1).
    async fn sync_after_local_change(&self) -> Result<(), RestaskError> {
        if !self.syncs_here {
            return self.settle_locked().map(|_| ());
        }
        match self.reconcile_locked().await {
            Ok(_) => Ok(()),
            Err(error @ RestaskError::Caldav { .. }) => {
                tracing::warn!(%error, "saved in the vault; the server will catch up on the next sync");
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// The creation date of a registered task (§6.4): the server's `CREATED` as the base
    /// snapshot recorded it at the last sync — when the index vouches for the snapshot —
    /// else the day the UID was minted, else today.
    fn created_on(&self, uid: &TaskUid, index: &Index) -> LocalDate {
        // Snapshots only hold date and floating values: the zone is irrelevant.
        base_store::cache_read(&self.state_dir, uid, &Utc)
            .zip(index.get(uid))
            .filter(|(base, entry)| {
                // The snapshot carries no list; the thumbprint covers it.
                let mut base = base.clone();
                base.list = entry.list.clone();
                entry.thumbprint == base.thumbprint()
            })
            .and_then(|(base, _)| base.created)
            .or_else(|| uid.created_on())
            .unwrap_or_else(|| self.clock.today_local())
    }

    /// Loads the base snapshots (`.restask/tasks/*.ics`), taking each task's list from
    /// the index (the snapshot itself carries none).
    fn load_base(&self, index: &Index) -> Result<BTreeMap<TaskUid, Task>, RestaskError> {
        let mut map = BTreeMap::new();
        let entries = match std::fs::read_dir(self.state_dir.join("tasks")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().to_string();
            let Some(uid) = name
                .strip_suffix(".ics")
                .and_then(|raw| TaskUid::parse(raw).ok())
            else {
                continue;
            };
            // Snapshots only hold date and floating values: the zone is irrelevant.
            if let Some(mut task) = base_store::cache_read(&self.state_dir, &uid, &Utc) {
                if let Some(entry) = index.get(&uid) {
                    task.list = entry.list.clone();
                    task.source.path = entry.source_path.clone();
                }
                map.insert(uid, task);
            }
        }
        Ok(map)
    }

    /// Applies line mutations to one vault file (single pass) and writes it back
    /// atomically when it changed. Returns the number of applied mutations.
    fn apply_file(&self, path: &str, mutations: &[Mutation]) -> Result<usize, RestaskError> {
        let file = self.vault.join(path);
        let contents = std::fs::read_to_string(&file)?;
        let out = mutator::apply(&contents, mutations, &self.cfg, self.clock.as_ref())?;
        for (mutation, reason) in &out.skipped {
            tracing::warn!(path = %path, ?mutation, ?reason, "vault mutation skipped");
        }
        if out.contents != contents {
            fsio::write_atomic(&file, &out.contents)?;
        }
        Ok(out.applied.len())
    }
}

/// `true` for failures that mean "the server is not usable right now": unreachable,
/// TLS broken, or credentials rejected.
fn is_connection_failure(error: &RestaskError) -> bool {
    matches!(
        error,
        RestaskError::Caldav {
            kind: CaldavErrorKind::Network | CaldavErrorKind::Tls | CaldavErrorKind::Auth,
            ..
        }
    )
}

/// The path segment of a list's collection: where the pass found it by name, else the
/// list's own slug.
fn collection_of<'a>(bound: &'a BTreeMap<ListSlug, String>, list: &'a ListSlug) -> &'a str {
    bound.get(list).map_or(list.as_str(), String::as_str)
}
