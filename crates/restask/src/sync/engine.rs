//! Reconciliation engine (§11, §13.1): the single-writer I/O orchestrator. Owns every
//! mutation of the vault, `.restask/` state, and the server; the planner decides, the
//! engine executes and records.
//!
//! One pass has three phases, ordered so that a crash at any point is repaired by the
//! next pass:
//!
//! 1. **Local** — scan and repair the vault, carry TODO.md edits to their source notes.
//!    Needs no server; always runs.
//! 2. **Remote** — snapshot the server; give the tasks that still have a long UID a
//!    counted one (§11.7); plan, apply vault edits, then server writes.
//! 3. **Record** — re-render TODO.md, then persist the state. The vault is written before
//!    the state that describes it, so the state never claims a line the vault lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration, Utc};

use crate::caldav::{list_name, resolve_list, Bound, CaldavPort, RemoteResource};
use crate::config::{MachineConfig, VaultConfig};
use crate::domain::{
    Clock, Ids, ListSlug, LocalDate, Priority, Recurrence, SourceRef, Status, Task, TaskUid, When,
};
use crate::fsio;
use crate::markdown::mutator::{self, Mutation};
use crate::markdown::{root_view, todo_view};
use crate::store::cache as base_store;
use crate::store::calendars::Calendars;
use crate::store::device::{switched, Device};
use crate::store::index::{Index, IndexEntry};
use crate::store::tombstones::Tombstones;
use crate::store::wires::Wires;
use crate::sync::planner::{self, DeleteOp, Plan, PutOp, Snapshots, DEFER_LIMIT};
use crate::vault::{self, Scan, ScanMode, STATE_DIR};
use crate::{CaldavErrorKind, RestaskError};

/// The engine's own last render of TODO.md, kept under `.restask/`. Comparing the live
/// file against it tells what the user edited in the view (§7).
pub const RENDERED_FILE: &str = "todo.rendered.md";

/// The engine's own last renders of the views root notes hold (§7.6), one file per root
/// note under `.restask/`: what [`RENDERED_FILE`] is for TODO.md.
pub const VIEWS_DIR: &str = "views";

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
    /// Where this machine keeps the identity it mints UIDs under (§9.4); `None` keeps
    /// it in `held`, for as long as the engine lives.
    device_file: Option<PathBuf>,
    /// The identity of an engine without a `device_file`.
    held: Mutex<Option<Device>>,
    caldav: C,
    clock: Arc<dyn Clock>,
}

/// What one pass mints UIDs with (§3.1, §9.4): in a vault that was switched to counted
/// UIDs, this device's identity there and the counter that goes on from the last number
/// it used; in any other vault long UIDs, and no identity.
struct Minting {
    device: Option<Device>,
    ids: Ids,
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
            device_file: machine.device_file,
            held: Mutex::new(None),
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
        let mut wires = Wires::load(&self.state_dir)?;
        // Left over from versions that queued server writes; the planner re-derives them.
        let _ = std::fs::remove_file(self.state_dir.join("outbox.json"));

        // 1 — local.
        let mut minting = self.minting(&index, &tombstones, &wires)?;
        let mut scan = self.scan_local(&index, &mut report, &minting.ids)?;
        tracing::info!(
            files = scan.files_scanned,
            tasks = scan.local.len(),
            "scan_complete"
        );

        // 2 — remote.
        let mut progress = Progress::default();
        let outcome = self
            .sync_remote(
                &mut scan,
                &mut index,
                &tombstones,
                &mut wires,
                &mut minting,
                now,
                &mut progress,
                &mut report,
            )
            .await;
        self.minted(&mut minting)?;

        // 3 — record: the view first, then the state.
        let scan = if progress.vault_changed {
            vault::scan(
                &self.vault,
                &self.cfg,
                self.clock.as_ref(),
                &index,
                ScanMode::ReadOnly,
            )?
        } else {
            scan
        };
        let mut tasks = scan.local;
        if let Some(plan) = &progress.plan {
            for task in &plan.inbox_inserts {
                tasks
                    .entry(task.uid.clone())
                    .or_insert_with(|| task.clone());
            }
        }
        self.render(&tasks)?;
        self.render_views(&tasks, &scan.views, &scan.unreadable)?;
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
        let mut minting = self.minting(
            &index,
            &Tombstones::load(&self.state_dir)?,
            &Wires::load(&self.state_dir)?,
        )?;
        let scan = self.scan_local(&index, &mut report, &minting.ids)?;
        self.minted(&mut minting)?;
        self.render(&scan.local)?;
        self.render_views(&scan.local, &scan.views, &scan.unreadable)?;
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
        let mut minting = self.minting(
            &index,
            &Tombstones::load(&self.state_dir)?,
            &Wires::load(&self.state_dir)?,
        )?;
        // The view is rendered from this scan: what the user edited in it comes first.
        let scan = self.scan_local(&index, &mut ReconcileReport::default(), &minting.ids)?;
        let task = Task {
            uid: minting.ids.mint(),
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
        self.minted(&mut minting)?;
        let mut tasks = scan.local;
        tasks.insert(task.uid.clone(), task.clone());
        self.render(&tasks)?;
        self.render_views(&tasks, &scan.views, &scan.unreadable)?;
        self.sync_after_local_change().await?;
        self.as_now(task)
    }

    /// Completes or reopens a task in its source file, then syncs (§13.3 `restask
    /// done/undone`). The change is safe in the vault even when the server is unreachable;
    /// a machine that is not the sync node only settles the vault.
    pub async fn set_done(&self, uid: &TaskUid, done: bool) -> Result<Task, RestaskError> {
        let _lock = self.lock().await?;
        let index = Index::load(&self.state_dir)?;
        let mut minting = self.minting(
            &index,
            &Tombstones::load(&self.state_dir)?,
            &Wires::load(&self.state_dir)?,
        )?;
        let scan = vault::scan(
            &self.vault,
            &self.cfg,
            self.clock.as_ref(),
            &index,
            ScanMode::Repair(&minting.ids),
        )?;
        self.minted(&mut minting)?;
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
        self.as_now(task)
    }

    // ── phase 1: local ────────────────────────────────────────────────────────────────

    /// `task` under the UID it has now: the pass that followed a command may have been
    /// the one that renumbered it (§11.7).
    fn as_now(&self, mut task: Task) -> Result<Task, RestaskError> {
        if task.uid.is_long() {
            if let Some(counted) = Wires::load(&self.state_dir)?.get(task.uid.as_str()) {
                task.uid = counted.clone();
            }
        }
        Ok(task)
    }

    /// What this pass mints UIDs with (§9.4): counted UIDs in a vault that was switched
    /// to them — some device holds a claim there —, long ones in any other. A vault is
    /// switched by its sync node ([`Engine::switch`]); until then nothing about devices
    /// is written, and a sync node of an earlier version reads every UID made here.
    fn minting(
        &self,
        index: &Index,
        tombstones: &Tombstones,
        wires: &Wires,
    ) -> Result<Minting, RestaskError> {
        if switched(&self.state_dir)? {
            self.counted(index, tombstones, wires)
        } else {
            Ok(Minting {
                device: None,
                ids: Ids::Long,
            })
        }
    }

    /// Counted UIDs for this pass: the device's identity in the vault — claimed when it
    /// has none (§9.4) — and its counter, past every UID the state knows.
    fn counted(
        &self,
        index: &Index,
        tombstones: &Tombstones,
        wires: &Wires,
    ) -> Result<Minting, RestaskError> {
        let known = || {
            index
                .entries
                .keys()
                .chain(tombstones.uids())
                .chain(wires.uids())
        };
        let taken: BTreeSet<String> = known()
            .filter_map(|uid| uid.minted_by())
            .map(|(tag, _)| tag.to_string())
            .collect();
        std::fs::create_dir_all(&self.state_dir)?;
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let device = Device::open(
            &self.state_dir,
            self.device_file.as_deref(),
            held.take(),
            &taken,
        )?;
        *held = Some(device.clone());
        let ids = device.counter();
        known().for_each(|uid| ids.observe(uid));
        Ok(Minting {
            device: Some(device),
            ids: Ids::Counted(ids),
        })
    }

    /// Switches the vault to counted UIDs (§9.4), when it is not yet: this machine
    /// claims a tag — the vault's first claim, which tells every other device that its
    /// sync node reads counted UIDs. Called by the pass that has the server's answer in
    /// hand: only the machine that really syncs the vault makes that promise.
    fn switch(
        &self,
        minting: &mut Minting,
        index: &Index,
        tombstones: &Tombstones,
        wires: &Wires,
    ) -> Result<(), RestaskError> {
        if minting.device.is_none() {
            *minting = self.counted(index, tombstones, wires)?;
            tracing::info!("vault_switched_to_counted_uids");
        }
        Ok(())
    }

    /// Remembers the numbers the pass has used, so the device never uses them again.
    fn minted(&self, minting: &mut Minting) -> Result<(), RestaskError> {
        let (Some(device), Some(ids)) = (&mut minting.device, minting.ids.counter()) else {
            return Ok(());
        };
        if device.used(ids, self.device_file.as_deref())? {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            *held = Some(device.clone());
        }
        Ok(())
    }

    /// Gives every task whose line still carries a long UID a counted one (§11.7): the
    /// sync node's, once per task. The names first (`wires.json`), so that a pass that
    /// dies here is taken up under the same UIDs; then the lines — in the notes, in the
    /// views, and in what the engine remembers of its renders, so that an edit the user
    /// made in a view is still told from the render; then the state that was kept under
    /// the long UID. Nothing is asked of the server: a resource keeps the long UID as
    /// its `UID` and is linked to its task by the plan that follows, like a task another
    /// client created. Returns the scan of the vault as it is afterwards.
    fn renumber(
        &self,
        scan: Scan,
        index: &mut Index,
        wires: &mut Wires,
        ids: &Ids,
    ) -> Result<Scan, RestaskError> {
        let Some(counter) = ids.counter() else {
            return Ok(scan);
        };
        scan.local.keys().for_each(|uid| counter.observe(uid));
        let renumbered = planner::renumbering(scan.local.keys(), wires, counter);
        let scan = if renumbered.is_empty() {
            scan
        } else {
            let mut learnt = false;
            for (long, counted) in &renumbered {
                learnt |= wires.insert(long.as_str().to_string(), counted.clone());
            }
            if learnt {
                wires.save(&self.state_dir)?;
            }
            // A note the scan read is rewritten or the pass ends here: a plan made
            // while a line still carries the UID its task no longer has would take the
            // line and the task for two.
            let notes = scan
                .notes
                .keys()
                .chain(std::iter::once(&self.cfg.inbox_file));
            for path in notes {
                let file = self.vault.join(path);
                let contents = match std::fs::read_to_string(&file) {
                    Ok(contents) => contents,
                    // No inbox file yet: the render makes it.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                let out = mutator::renumber(&contents, &renumbered, &self.cfg);
                if out != contents {
                    fsio::write_atomic(&file, &out)?;
                }
            }
            // What the engine remembers of its renders, as far as it is there.
            let mut remembered = vec![self.state_dir.join(RENDERED_FILE)];
            if let Ok(entries) = std::fs::read_dir(self.state_dir.join(VIEWS_DIR)) {
                for entry in entries {
                    remembered.push(entry?.path());
                }
            }
            for file in remembered {
                let Ok(contents) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let out = mutator::renumber(&contents, &renumbered, &self.cfg);
                if out != contents {
                    fsio::write_atomic(&file, &out)?;
                }
            }
            tracing::info!(count = renumbered.len(), "tasks_renumbered");
            scan.renumbered(&renumbered, &self.cfg)
        };

        // The state follows the lines. Read off the names, not off this pass: a pass
        // that died between the two is finished here.
        let moved: Vec<(TaskUid, TaskUid)> = wires
            .iter()
            .filter_map(|(name, counted)| Some((TaskUid::parse(name).ok()?, counted)))
            .filter(|(long, counted)| {
                long.is_long() && index.get(long).is_some() && scan.local.contains_key(counted)
            })
            .map(|(long, counted)| (long, counted.clone()))
            .collect();
        if moved.is_empty() {
            return Ok(scan);
        }
        let current = |uid: TaskUid| match wires.get(uid.as_str()) {
            Some(counted) if uid.is_long() => counted.clone(),
            _ => uid,
        };
        for (long, counted) in &moved {
            let Some(entry) = index.get(long).cloned() else {
                continue;
            };
            let mut thumbprint = entry.thumbprint;
            // Snapshots only hold date and floating values: the zone is irrelevant.
            if let Some(mut base) = base_store::cache_read(&self.state_dir, long, &Utc) {
                // The snapshot carries no list; the thumbprint covers it.
                base.list = entry.list.clone();
                let vouched = entry.thumbprint == base.thumbprint();
                base.uid = counted.clone();
                base.parent = base.parent.map(current);
                if vouched {
                    thumbprint = base.thumbprint();
                }
                base_store::cache_write(&self.state_dir, &base, base.last_modified)?;
            }
            index.upsert(IndexEntry {
                uid: counted.clone(),
                thumbprint,
                ..entry
            });
            index.remove(long);
        }
        index.save(&self.state_dir)?;
        for (long, _) in &moved {
            base_store::cache_remove(&self.state_dir, long)?;
        }
        Ok(scan)
    }

    /// Scans and repairs the vault, then carries edits made on mirror lines — of TODO.md
    /// and of the views root notes hold (§7.6) — to their source notes and gives lines
    /// moved to another section of a view their new priority (rescanning when that
    /// changed anything).
    fn scan_local(
        &self,
        index: &Index,
        report: &mut ReconcileReport,
        ids: &Ids,
    ) -> Result<Scan, RestaskError> {
        let scan = |report: &mut ReconcileReport| -> Result<Scan, RestaskError> {
            let scan = vault::scan(
                &self.vault,
                &self.cfg,
                self.clock.as_ref(),
                index,
                ScanMode::Repair(ids),
            )?;
            report.registered += scan.registered;
            report.normalized += scan.normalized;
            report.scanned_files = scan.files_scanned;
            Ok(scan)
        };
        let first = scan(report)?;
        let edits = self.local_edits(&first, index);
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

    /// The edits the local phase carries from one file to another (§7.3, §7.6, §6.4),
    /// per target file: what was changed on a mirror line of TODO.md or of a root
    /// note's view, to the task's own line; and the creation date a line asks for.
    fn local_edits(&self, first: &Scan, index: &Index) -> BTreeMap<String, Vec<Mutation>> {
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
        for (path, contents) in &first.views {
            let Ok(remembered) = std::fs::read_to_string(self.view_file(path)) else {
                continue;
            };
            let carried = root_view::mirror_edits(
                contents,
                &remembered,
                path,
                &first.local,
                &self.cfg,
                self.clock.today_local(),
            );
            for (target, ops) in carried {
                edits.entry(target).or_default().extend(ops);
            }
        }
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
        edits
    }

    /// Whether the vault has local work waiting (§11.1 phase 1 and the render): a pass
    /// — or `restask settle` — would write a note, TODO.md or a root note's view.
    /// Nothing is written and the server is not asked. `false` for a vault an
    /// integration has settled: the pass over it only does server work.
    ///
    /// The daemon asks before a pass that follows a change to a note (§13.1): local
    /// work in a note that was written a moment ago is, more often than not, the work
    /// of the device the note is still being edited on.
    pub async fn unsettled(&self) -> Result<bool, RestaskError> {
        let _lock = self.lock().await?;
        let index = Index::load(&self.state_dir)?;
        let scan = vault::scan(
            &self.vault,
            &self.cfg,
            self.clock.as_ref(),
            &index,
            ScanMode::ReadOnly,
        )?;
        if !scan.unsettled.is_empty() || !self.local_edits(&scan, &index).is_empty() {
            return Ok(true);
        }
        let todo = std::fs::read_to_string(self.vault.join(&self.cfg.inbox_file)).ok();
        if todo.as_deref() != Some(todo_view::render(&scan.local, &self.cfg).as_str()) {
            return Ok(true);
        }
        Ok(scan.views.iter().any(|(path, contents)| {
            root_view::render(contents, path, &scan.local, &self.cfg)
                .is_some_and(|rendered| rendered != *contents)
        }))
    }

    // ── phase 2: remote ───────────────────────────────────────────────────────────────

    /// Snapshots the server, plans, and executes: the wire names the plan learnt, then
    /// vault edits (the vault is the source of truth), then server writes. Progress
    /// survives an early error.
    ///
    /// With the server's answer in hand this machine is the one that syncs the vault:
    /// before it plans, it switches the vault to counted UIDs if no device has yet
    /// (§9.4) and gives the tasks that still carry a long UID a counted one (§11.7) —
    /// `scan` and `index` are the vault and the state as they are after that.
    #[allow(clippy::too_many_arguments)]
    async fn sync_remote(
        &self,
        scan: &mut Scan,
        index: &mut Index,
        tombstones: &Tombstones,
        wires: &mut Wires,
        minting: &mut Minting,
        now: DateTime<Utc>,
        progress: &mut Progress,
        report: &mut ReconcileReport,
    ) -> Result<(), RestaskError> {
        let inbox_list = vault::inbox_list(&self.cfg)?;
        let todo_lists = vault::todo_lists(&self.cfg)?;
        let (remote, created, bound, unread) = self
            .remote_snapshot(scan, index, &inbox_list, &todo_lists)
            .await?;
        if self.syncs_here {
            self.switch(minting, index, tombstones, wires)?;
            *scan = self.renumber(std::mem::take(scan), index, wires, &minting.ids)?;
        }
        let ids = minting.ids.counter();
        let snapshots = Snapshots {
            local: scan.local.clone(),
            base: self.load_base(index)?,
            remote,
            unread,
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
            flat: scan.flat.clone(),
            wires: wires.clone(),
            ids: ids.cloned(),
            today: Some(self.clock.today_local()),
        };
        let plan = progress.plan.insert(planner::plan(&snapshots));
        if let Some(ids) = ids.filter(|_| plan.minted > 0) {
            ids.observe(&TaskUid::minted(ids.tag(), plan.minted));
        }
        // Before any line is written: a line whose name is not recorded would be a
        // second task to the pass that finds it (§9.5).
        let mut learnt = false;
        for (name, uid) in &plan.wires {
            learnt |= wires.insert(name.clone(), uid.clone());
        }
        if learnt {
            wires.save(&self.state_dir)?;
        }

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
    /// missing there was never deleted — where the lists found by name are, and per list
    /// the resources that are there but could not be read.
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
            BTreeMap<ListSlug, BTreeSet<String>>,
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
        let mut unread = BTreeMap::new();
        let mut created = BTreeSet::new();
        let mut bound = BTreeMap::new();
        // A collection is one list's. Two names of the vault can find the same one — a
        // note routed to `homelab`, the calendar's path, and `todo_lists` naming it
        // `home-lab` after its display name: synced as two lists, each pass would push
        // the tasks as the one and delete them as strays of the other.
        let own_paths = scope.clone();
        let mut taken: BTreeMap<String, ListSlug> = BTreeMap::new();
        for slug in scope {
            if let Some(listing) = self.caldav.list_tasks(slug.as_str(), &slug).await? {
                // At its own path — also when it was found elsewhere before.
                if calendars.bound.remove(&slug).is_some() {
                    tracing::warn!(list = %slug.as_str(), "collection_changed");
                    created.insert(slug.clone());
                }
                if !listing.unreadable.is_empty() {
                    unread.insert(slug.clone(), listing.unreadable.into_iter().collect());
                }
                remote.insert(slug, listing.resources);
                continue;
            }
            let collections = match &listing {
                Some(collections) => collections,
                None => listing.insert(self.caldav.list_collections().await?),
            };
            match resolve_list(collections, &slug) {
                Bound::At(collection) => {
                    let holder = own_paths
                        .iter()
                        .find(|other| other.as_str() == collection && **other != slug)
                        .or_else(|| taken.get(&collection));
                    if let Some(holder) = holder {
                        // Left out of the pass: unknown, not empty (invariant: nothing is
                        // deleted on evidence that is merely absent).
                        tracing::warn!(
                            list = %slug.as_str(),
                            same_as = %holder.as_str(),
                            calendar = %collection,
                            "two list names of the vault are one calendar of the server; this one is not synced until the vault uses one name"
                        );
                        calendars.bound.remove(&slug);
                        continue;
                    }
                    taken.insert(collection.clone(), slug.clone());
                    // Gone between the two requests: not listed, so nothing is concluded.
                    let Some(listing) = self.caldav.list_tasks(&collection, &slug).await? else {
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
                    if !listing.unreadable.is_empty() {
                        unread.insert(slug.clone(), listing.unreadable.into_iter().collect());
                    }
                    remote.insert(slug, listing.resources);
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
        Ok((remote, created, bound, unread))
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

    /// Where the engine keeps its last render of the view of the root note at `path`.
    fn view_file(&self, path: &str) -> PathBuf {
        self.state_dir
            .join(VIEWS_DIR)
            .join(format!("{}.md", todo_view::digest(path)))
    }

    /// Renders the views root notes hold (§7.6) and remembers each render. `views` is
    /// the scan's: every root note with a view, as the text its tasks were read from. A
    /// note that is no longer that text — an editor or the file sync wrote it meanwhile
    /// — is left for the next pass: rendered from an older reading, the view would drop
    /// what was typed into it since. Nothing is written when the content is unchanged.
    /// A remembered render is dropped once its note is gone or holds no view any more;
    /// `unreadable` are the notes this pass could not read, of which nothing is known.
    fn render_views(
        &self,
        tasks: &BTreeMap<TaskUid, Task>,
        views: &BTreeMap<String, String>,
        unreadable: &BTreeSet<String>,
    ) -> Result<(), RestaskError> {
        let dir = self.state_dir.join(VIEWS_DIR);
        for (path, contents) in views {
            let Some(rendered) = root_view::render(contents, path, tasks, &self.cfg) else {
                continue;
            };
            let note = self.vault.join(path);
            if std::fs::read_to_string(&note).ok().as_ref() != Some(contents) {
                tracing::info!(path = %path, "note changed during the pass; its view is rendered on the next one");
                continue;
            }
            if rendered != *contents {
                fsio::write_atomic(&note, &rendered)?;
                tracing::info!(path = %path, "view_rendered");
            }
            if let Some(remembered) = root_view::remembered(&rendered, path, &self.cfg) {
                std::fs::create_dir_all(&dir)?;
                fsio::write_if_changed(&self.view_file(path), &remembered)?;
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(());
        };
        for entry in entries {
            let file = entry?.path();
            let kept = std::fs::read_to_string(&file).is_ok_and(|remembered| {
                root_view::remembered_path(&remembered)
                    .is_some_and(|path| views.contains_key(path) || unreadable.contains(path))
            });
            if !kept {
                std::fs::remove_file(&file)?;
            }
        }
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
