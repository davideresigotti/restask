//! Reconciliation planner (§11): a pure decision function over the vault, the base
//! snapshots, and the server. Zero I/O; time enters only as the `now` parameter and the
//! same snapshots always yield the same plan.

use std::collections::{BTreeMap, BTreeSet};

use crate::caldav::RemoteResource;
use crate::domain::{
    Counter, ListSlug, LocalDate, LocalDateTime, Recurrence, Status, Task, TaskUid, When,
};
use crate::markdown::mutator::Mutation;
use crate::markdown::TaskDraft;
use crate::store::index::{Index, IndexEntry};
use crate::store::wires::Wires;
use crate::sync::merge::{fields_differ, merge, RemoteView, TIE_WINDOW_SECS};
use crate::vtodo::links::wire_title;
use crate::vtodo::recurrence::{consume_count, find_in_extras};
use crate::vtodo::WireNames;

/// Consecutive cycles a stale-looking vault file is waited on (R1) before the vault is
/// taken at its word again (vault authority).
pub const DEFER_LIMIT: u8 = 3;

/// Everything one reconciliation pass needs (§11.1).
#[derive(Debug, Default, PartialEq)]
pub struct Snapshots {
    /// Tasks parsed from the vault scan.
    pub local: BTreeMap<TaskUid, Task>,
    /// Base snapshots read from `.restask/tasks` (`list` taken from the index). A base
    /// only counts when the index vouches for it (see [`IndexEntry::thumbprint`]).
    pub base: BTreeMap<TaskUid, Task>,
    /// Server resources per collection — **only** collections that were listed this
    /// cycle. A list missing here is unknown, never "empty".
    pub remote: BTreeMap<ListSlug, Vec<RemoteResource>>,
    /// Per listed collection, the names of the resources it holds whose body could not
    /// be read this cycle. Such a resource exists: the task it may be is unknown, never
    /// "gone from the server".
    pub unread: BTreeMap<ListSlug, BTreeSet<String>>,
    /// Collections created during this cycle (they cannot hold deletions).
    pub created: BTreeSet<ListSlug>,
    /// UIDs deleted on some device.
    pub tombstones: BTreeSet<TaskUid>,
    /// Routing/etag bookkeeping (§9.1).
    pub index: Index,
    /// Every routed note, with or without tasks: vault-relative path → list.
    pub notes: BTreeMap<String, ListSlug>,
    /// Per list, the note that receives tasks created on the server.
    pub homes: BTreeMap<ListSlug, String>,
    /// Routed files that could not be read this cycle; their tasks are unknown, not gone.
    pub unreadable: BTreeSet<String>,
    /// Vault-relative path of the engine-managed inbox file.
    pub inbox_file: String,
    /// The list the inbox file routes to; `None` only in an empty default snapshot.
    pub inbox_list: Option<ListSlug>,
    /// Further lists whose tasks live in the inbox file when no note is their home
    /// (`vault.todo_lists`, §7.5).
    pub todo_lists: BTreeSet<ListSlug>,
    /// The Obsidian vault the notes linked from a task open in (`vault.obsidian_vault`,
    /// §8.4); `None` writes no links.
    pub obsidian_vault: Option<String>,
    /// Tasks whose line is in the view of a root note (§7.6): flat, like the lines of the
    /// inbox file — their indentation names no parent.
    pub flat: BTreeSet<TaskUid>,
    /// The names tasks go by on the server where those are not their UIDs (§9.5).
    pub wires: Wires,
    /// This device's counter (§3.1): the UIDs of the tasks that enter the vault in this
    /// pass — another client's (R5), the record of a recurring task's occurrence (§11.6).
    /// The plan counts on from a copy of it. `None`: such a task gets a long UID derived
    /// from what it is ([`TaskUid::derived`]), as before the counters.
    pub ids: Option<Counter>,
    /// The local day of the pass: the creation date the server is given for a task
    /// whose line states none and whose UID tells none (R2).
    pub today: Option<LocalDate>,
}

/// The mutation plan for one reconciliation pass (§11.1).
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Vault edits per file, in application order (one pass per file).
    pub mutations: BTreeMap<String, Vec<Mutation>>,
    /// Server-created tasks that land in the inbox; the TODO.md render writes them.
    pub inbox_inserts: Vec<Task>,
    /// Resources to create or replace.
    pub puts: Vec<PutOp>,
    /// List moves: `PUT` into the new collection, then `DELETE` from the old one.
    pub moves: Vec<MoveOp>,
    /// Tasks another client created that enter the vault in this pass (R5). Their
    /// lines are among the inserts and their links among `puts`; this only names them.
    pub adopted: Vec<TaskUid>,
    /// Resources to delete.
    pub deletes: Vec<DeleteOp>,
    /// Tasks whose base is refreshed without a push (already equal on the server).
    pub settled: Vec<Settled>,
    /// UIDs whose base snapshot and index entry are dropped.
    pub forgets: Vec<TaskUid>,
    /// UIDs to tombstone.
    pub tombstones: Vec<TaskUid>,
    /// Tombstoned UIDs that reappeared in the vault: the tombstone is cleared.
    pub revived: Vec<TaskUid>,
    /// UIDs whose reconciliation was postponed, with the reason.
    pub deferred: Vec<(TaskUid, DeferReason)>,
    /// Wire names learnt in this pass, each with the UID of its task (§9.5). They are
    /// recorded before the vault is touched: a pass that dies after it wrote a line
    /// must be taken up under the same UID.
    pub wires: Vec<(String, TaskUid)>,
    /// The highest number the plan took from the counter; 0 when it minted no UID.
    pub minted: u64,
}

impl Plan {
    /// `true` when the plan changes nothing anywhere.
    pub fn is_noop(&self) -> bool {
        *self == Plan::default()
    }
}

/// A resource write. When it succeeds, `task` becomes the base for its UID.
#[derive(Debug, Clone, PartialEq)]
pub struct PutOp {
    /// The task to serialize (its `list` names the target collection).
    pub task: Task,
    /// Resource name to write (sans `.ics`): the UID, or the name the server lists the
    /// task under when another client stored it differently.
    pub name: String,
    /// Unmanaged content of the resource being replaced, written back verbatim.
    pub extras: Vec<String>,
    /// The `UID`s to write where they are not restask's own: a task another client
    /// created keeps the one it was given (R5).
    pub wire: WireNames,
    /// Etag of the version being replaced; `None` creates the resource.
    pub if_match: Option<String>,
}

/// A list move (§11.2 R9).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveOp {
    /// The write into the collection the task now routes to.
    pub put: PutOp,
    /// The copy to remove afterwards — only once the write succeeded.
    pub from: DeleteOp,
}

/// A remote resource deletion (§11.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteOp {
    /// The collection holding the resource.
    pub list: ListSlug,
    /// Resource name (sans `.ics`).
    pub name: String,
    /// Etag for `If-Match`.
    pub etag: Option<String>,
}

/// A base refresh without a push.
#[derive(Debug, Clone, PartialEq)]
pub struct Settled {
    /// The agreed content.
    pub task: Task,
    /// Etag of the server copy that holds it.
    pub etag: String,
}

/// Why a UID's reconciliation was postponed (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// The vault file is older than the base it disagrees with: most likely a replica
    /// that file sync has not caught up yet (R1).
    VaultFileStale,
}

/// One copy of a task on the server.
type Copy<'a> = (&'a ListSlug, &'a RemoteResource);

/// A task line waiting to be inserted into the vault.
struct PendingInsert {
    /// Target note; `None` = the inbox (rendered by TODO.md).
    path: Option<String>,
    task: Task,
    parent: Option<TaskUid>,
}

/// Computes the reconciliation plan (§11). PURE.
pub fn plan(s: &Snapshots) -> Plan {
    let mut p = Plan::default();
    let ctx = Context::new(s);
    let mut inserts: Vec<PendingInsert> = Vec::new();

    let mut uids: BTreeSet<&TaskUid> = BTreeSet::new();
    uids.extend(s.local.keys());
    uids.extend(s.base.keys());
    uids.extend(s.index.entries.keys());
    uids.extend(ctx.managed.keys());

    for uid in uids {
        let entry = s.index.get(uid);
        let copies: &[Copy<'_>] = ctx.managed.get(uid).map(Vec::as_slice).unwrap_or_default();
        let known = entry.is_some() || s.base.contains_key(uid);
        if copies.is_empty() && ctx.unread(uid, s.local.get(uid), entry) {
            // The server holds a resource that may be this task and could not be read:
            // unknown, neither deleted there nor new to it.
            continue;
        }

        let Some(local) = s.local.get(uid) else {
            if entry.is_some_and(|e| s.unreadable.contains(&e.source_path)) {
                // The note exists but could not be read: unknown, not deleted.
                continue;
            }
            if s.tombstones.contains(uid) {
                // R0 — deleted somewhere: purge every surviving copy, never resurrect.
                p.deletes.extend(copies.iter().map(delete_of));
                if known {
                    p.forgets.push(uid.clone());
                }
            } else if copies.is_empty() {
                // R6 — state for a task that exists nowhere any more.
                if known && entry.is_none_or(|e| s.remote.contains_key(&e.list)) {
                    p.forgets.push(uid.clone());
                }
            } else if known {
                // Vault-side deletion: the line is gone while the server still holds it.
                p.deletes.extend(copies.iter().map(delete_of));
                p.tombstones.push(uid.clone());
                p.forgets.push(uid.clone());
                tracing::info!(uid = %uid, "task_deleted");
            } else {
                // R4 — created on the server (or state was lost): bring it into the vault.
                let (origin, rest) = (copies[0], &copies[1..]);
                p.deletes.extend(rest.iter().map(delete_of));
                let (path, mut task) = ctx.place(origin.0, origin.1);
                task.uid = uid.clone();
                p.settled.push(Settled {
                    task: task.clone(),
                    etag: origin.1.etag.clone(),
                });
                if origin.1.task.managed {
                    tracing::info!(uid = %uid, list = %origin.0.as_str(), "caldav_pull");
                } else {
                    // R5 — another client's task: it stays that client's resource and
                    // is linked to its line. Settled as found first, so a link the
                    // server refuses (the client wrote again meanwhile) is retried as a
                    // merge over what was read here, never as a conflict.
                    p.puts.push(PutOp {
                        wire: ctx.wire(&task),
                        task: task.clone(),
                        name: origin.1.name.clone(),
                        extras: origin.1.task.extras.clone(),
                        if_match: Some(origin.1.etag.clone()),
                    });
                    p.adopted.push(uid.clone());
                    tracing::info!(uid = %uid, list = %origin.0.as_str(), "task_adopted");
                }
                inserts.push(PendingInsert {
                    path,
                    parent: ctx.parent_of(origin.1).cloned(),
                    task,
                });
            }
            continue;
        };

        if s.tombstones.contains(uid) {
            // The vault is the source of truth: a line that is (back) in the vault lives.
            p.revived.push(uid.clone());
        }
        if !s.remote.contains_key(&local.list) {
            // Its collection was not listed this cycle: nothing can be decided.
            continue;
        }
        let base = ctx.base(uid);

        // R1 — the vault file predates the base it disagrees with: a replica that file
        // sync has not caught up. Wait a few cycles, then trust the vault.
        if let (Some(base), Some(entry)) = (base, entry) {
            let lag = base
                .last_modified
                .signed_duration_since(local.last_modified)
                .num_seconds();
            if fields_differ(base, local)
                && lag > TIE_WINDOW_SECS
                && entry.defer_count < DEFER_LIMIT
            {
                p.deferred.push((uid.clone(), DeferReason::VaultFileStale));
                continue;
            }
        }

        let authoritative = ctx.parent_authoritative(local);
        let at_list = copies.iter().find(|(slug, _)| **slug == local.list);
        if let Some((_, resource)) = at_list {
            // R7/R8 — both sides hold it: three-way merge. Every other copy (another
            // collection, or a second resource in this one) is a stray.
            p.deletes.extend(
                copies
                    .iter()
                    .filter(|(_, other)| !std::ptr::eq(*other, *resource))
                    .map(delete_of),
            );
            let merged = merge(base, local, ctx.view(resource), authoritative);
            if merged.conflict {
                tracing::warn!(uid = %uid, "conflict_resolved");
            }
            if !merged.mutations.is_empty() {
                p.mutations
                    .entry(local.source.path.clone())
                    .or_default()
                    .extend(merged.mutations);
            }
            if let Some(roll) = roll_forward(&merged.task, Some(resource), ctx.ids.as_ref()) {
                // R8r — one occurrence of a recurring task was completed in the vault:
                // the checked line becomes a record of its own, the series moves on.
                let (task, extras) = apply_roll(&mut p, &ctx, local, roll);
                p.puts.push(PutOp {
                    wire: ctx.wire(&task),
                    task,
                    name: resource.name.clone(),
                    extras,
                    if_match: Some(resource.etag.clone()),
                });
            } else if merged.push
                || !ctx.linked(uid, resource)
                || !ctx.shown(&merged.task, resource)
            {
                p.puts.push(PutOp {
                    wire: ctx.wire(&merged.task),
                    task: merged.task,
                    name: resource.name.clone(),
                    extras: resource.task.extras.clone(),
                    if_match: Some(resource.etag.clone()),
                });
            } else if needs_settle(entry, s.base.get(uid), &merged.task, &resource.etag) {
                p.settled.push(Settled {
                    task: merged.task,
                    etag: resource.etag.clone(),
                });
            }
        } else if let Some((origin, rest)) = copies.split_first() {
            // R9 — the server holds it in another collection: move it, UID preserved.
            let merged = merge(base, local, ctx.view(origin.1), authoritative);
            if !merged.mutations.is_empty() {
                p.mutations
                    .entry(local.source.path.clone())
                    .or_default()
                    .extend(merged.mutations);
            }
            tracing::info!(uid = %uid, from = %origin.0.as_str(), to = %local.list.as_str(), "task_moved");
            let (task, extras) = match roll_forward(&merged.task, Some(origin.1), ctx.ids.as_ref())
            {
                Some(roll) => apply_roll(&mut p, &ctx, local, roll),
                None => (merged.task, origin.1.task.extras.clone()),
            };
            p.moves.push(MoveOp {
                put: PutOp {
                    wire: ctx.wire(&task),
                    // A task of restask's own is named after its UID; another client's
                    // keeps the name that client gave it.
                    name: match ctx.aliases.get(uid) {
                        Some(_) => origin.1.name.clone(),
                        None => uid.as_str().to_string(),
                    },
                    task,
                    extras,
                    if_match: None,
                },
                from: delete_of(origin),
            });
            p.deletes.extend(rest.iter().map(delete_of));
        } else if ctx.deleted_on_server(local, entry) {
            // R3 — it was settled and the server no longer has it: deleted remotely.
            p.mutations
                .entry(local.source.path.clone())
                .or_default()
                .push(Mutation::Delete { uid: uid.clone() });
            p.tombstones.push(uid.clone());
            p.forgets.push(uid.clone());
            tracing::info!(uid = %uid, path = %local.source.path, "task_deleted");
        } else {
            // R2 — the server has never seen it (or lost it wholesale): push. A recurring
            // task that is already checked rolls forward first (R8r).
            let (mut task, extras) = match roll_forward(local, None, ctx.ids.as_ref()) {
                Some(roll) => apply_roll(&mut p, &ctx, local, roll),
                None => (local.clone(), Vec::new()),
            };
            // The server records when the task was created even though its line does
            // not say (§8.1): the day a long UID was minted, else the day of this pass.
            task.created = task.created.or_else(|| uid.created_on()).or(s.today);
            p.puts.push(PutOp {
                wire: ctx.wire(&task),
                task,
                name: uid.as_str().to_string(),
                extras,
                if_match: None,
            });
        }
    }

    schedule_inserts(&mut p, s, inserts);
    p.wires = ctx.learnt;
    if let (Some(ids), Some(before)) = (&ctx.ids, &s.ids) {
        if ids.last() > before.last() {
            p.minted = ids.last();
        }
    }
    p
}

/// The counted UIDs the long UIDs on the vault's lines get (§11.7): `local` are the UIDs
/// of the vault's tasks. A long UID that was given one before — the pass died, or a file
/// sync brought an old copy of a note back — gets that one again (`wires`); the others
/// are numbered in the order of their creation, from `ids`. Empty when every task has a
/// counted UID. PURE (but for the counter).
pub fn renumbering<'a>(
    local: impl Iterator<Item = &'a TaskUid>,
    wires: &Wires,
    ids: &Counter,
) -> BTreeMap<TaskUid, TaskUid> {
    local
        .filter(|uid| uid.is_long())
        .map(|uid| {
            let counted = wires.get(uid.as_str()).cloned();
            (uid.clone(), counted.unwrap_or_else(|| ids.mint()))
        })
        .collect()
}

/// Lookup tables derived once from the snapshots.
struct Context<'a> {
    s: &'a Snapshots,
    /// Task UID → every server copy, in collection order. A resource that keeps another
    /// `UID` — another client's (R5), or the long one its task had before it was
    /// renumbered (§11.7) — counts under the UID of the task it is.
    managed: BTreeMap<TaskUid, Vec<Copy<'a>>>,
    /// Task UID → the `UID` its resource carries on the server, where that is another.
    aliases: BTreeMap<TaskUid, &'a str>,
    /// Such a `UID` → the UID of its task (resolves parent relations).
    adopted_uids: BTreeMap<&'a str, TaskUid>,
    /// The counter the plan mints from: a copy of the snapshots', which has seen every
    /// UID of the snapshots.
    ids: Option<Counter>,
    /// Wire names not in the snapshots' record yet, with the UID of their task.
    learnt: Vec<(String, TaskUid)>,
    /// Lists whose settled tasks vanished wholesale: a reset collection, not deletions.
    reset: BTreeSet<&'a ListSlug>,
}

impl<'a> Context<'a> {
    fn new(s: &'a Snapshots) -> Self {
        let mut managed: BTreeMap<TaskUid, Vec<Copy<'_>>> = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut adopted_uids = BTreeMap::new();
        let mut learnt: Vec<(String, TaskUid)> = Vec::new();
        // No UID that exists anywhere is minted again.
        let ids = s.ids.clone();
        if let Some(ids) = &ids {
            let resources = s.remote.values().flatten();
            let remote = resources.flat_map(|resource| {
                let own = resource.task.managed.then_some(&resource.task.task.uid);
                own.into_iter().chain(&resource.task.adopted_as)
            });
            s.local
                .keys()
                .chain(s.base.keys())
                .chain(s.index.entries.keys())
                .chain(&s.tombstones)
                .chain(s.wires.uids())
                .chain(remote)
                .for_each(|uid| ids.observe(uid));
        }
        // The task a long UID names now: the one it was renumbered to (§11.7).
        let current = |uid: TaskUid| match s.wires.get(uid.as_str()) {
            Some(counted) if uid.is_long() => counted.clone(),
            _ => uid,
        };
        for (slug, resources) in &s.remote {
            for resource in resources {
                let seed = seed_of(resource);
                let own = resource.task.managed.then_some(&resource.task.task.uid);
                // The link a resource carries to a counted UID, when it is bound to
                // the resource's own `UID` (`X-RESTASK-OF`).
                let bound = resource.task.adopted_as.as_ref().filter(|linked| {
                    !linked.is_long() && resource.task.adopted_for.as_deref() == Some(seed)
                });
                let renumbered = own
                    .filter(|uid| uid.is_long())
                    .and_then(|uid| bound.or_else(|| s.wires.get(uid.as_str())));
                let uid = match (own, renumbered) {
                    (Some(own), None) => {
                        managed
                            .entry(own.clone())
                            .or_default()
                            .push((slug, resource));
                        continue;
                    }
                    // A resource of restask's own that still has the long UID of a task
                    // since renumbered: it stays what it is, linked like another client's.
                    (_, Some(counted)) => counted.clone(),
                    (None, None) => {
                        // Foreign tasks are adopted only where the vault has a place for
                        // them.
                        let has_home = s.inbox_list.as_ref() == Some(slug)
                            || s.todo_lists.contains(slug)
                            || s.homes.contains_key(slug);
                        // The UID the resource says it is linked under (`X-RESTASK-UID`),
                        // when the link is bound to this `UID`; else the one the state
                        // knows this `UID` by (a client may drop the properties); else a
                        // long one the vault or the state knows that was derived from it
                        // (a link of before the counters); else a new one.
                        let linked = bound.or_else(|| {
                            let linked = resource.task.adopted_as.as_ref();
                            linked.filter(|linked| linked.adopts(seed))
                        });
                        let known = linked
                            .or_else(|| s.wires.get(seed))
                            .or_else(|| {
                                s.local
                                    .keys()
                                    .chain(s.index.entries.keys())
                                    .chain(&s.tombstones)
                                    .find(|known| known.adopts(seed))
                            })
                            .cloned()
                            .map(current);
                        // A list that lost its place — taken out of `todo_lists` — takes
                        // in nothing new, but a task whose line is in the vault is still
                        // that resource: unseen here, it would read as deleted on the
                        // server (R3).
                        let in_vault = known.as_ref().is_some_and(|uid| s.local.contains_key(uid));
                        if !has_home && !in_vault {
                            continue;
                        }
                        known.unwrap_or_else(|| match &ids {
                            Some(ids) => ids.mint(),
                            None => TaskUid::derived(seed, resource.task.created_at),
                        })
                    }
                };
                if !uid.is_long() && s.wires.get(seed) != Some(&uid) {
                    learnt.push((seed.to_string(), uid.clone()));
                }
                aliases.insert(uid.clone(), seed);
                adopted_uids.insert(seed, uid.clone());
                managed.entry(uid).or_default().push((slug, resource));
            }
        }
        // Canonical copy first: the one in the collection the index knows, and within a
        // collection the resource named after the UID.
        for (uid, copies) in &mut managed {
            copies.sort_by_key(|(slug, resource)| {
                let known_list = s.index.get(uid).is_some_and(|entry| entry.list == **slug);
                let canonical_name = resource.name == uid.as_str();
                (!known_list, (*slug).clone(), !canonical_name)
            });
        }

        let mut settled_per_list: BTreeMap<&ListSlug, (usize, usize)> = BTreeMap::new();
        for (uid, local) in &s.local {
            let Some(entry) = s.index.get(uid) else {
                continue;
            };
            if entry.caldav_etag.is_none() || entry.list != local.list {
                continue;
            }
            let counts = settled_per_list.entry(&local.list).or_default();
            counts.0 += 1;
            if managed.contains_key(uid) {
                counts.1 += 1;
            }
        }
        let mut reset: BTreeSet<&ListSlug> = s.created.iter().collect();
        for (slug, (settled, present)) in settled_per_list {
            if settled >= 2 && present == 0 {
                tracing::warn!(list = %slug.as_str(), count = settled, "collection_reset");
                reset.insert(slug);
            }
        }

        Self {
            s,
            managed,
            aliases,
            adopted_uids,
            ids,
            learnt,
            reset,
        }
    }

    /// The base for `uid`, when the index vouches for the snapshot.
    fn base(&self, uid: &TaskUid) -> Option<&'a Task> {
        let entry = self.s.index.get(uid)?;
        let base = self.s.base.get(uid)?;
        (entry.caldav_etag.is_some() && entry.thumbprint == base.thumbprint()).then_some(base)
    }

    /// The names `task` and its parent go by on the server where those are not their
    /// restask UIDs: tasks another client created keep the `UID` it gave them.
    fn wire(&self, task: &Task) -> WireNames {
        let alias = |uid: &TaskUid| self.aliases.get(uid).map(|raw| raw.to_string());
        WireNames {
            uid: alias(&task.uid),
            parent: task.parent.as_ref().and_then(alias),
            obsidian_vault: self.s.obsidian_vault.clone(),
        }
    }

    /// Whether `resource` shows `task` the way a push of it would (§8.4): its title,
    /// with the links into Obsidian, and the vault text beside it. A resource written
    /// before its text had links — or before the vault named its Obsidian vault, or
    /// still holding the text of a link since removed — is written again.
    fn shown(&self, task: &Task, resource: &RemoteResource) -> bool {
        let vault = self.s.obsidian_vault.as_deref();
        let title = wire_title(&task.text, vault, &task.source.path);
        let vault_text = (title != task.text).then_some(&task.text);
        resource.task.summary == title && resource.task.vault_text.as_ref() == vault_text
    }

    /// The calendar a line of `task` has to name (`📁`, §7.5): its list, when the task
    /// lives in the inbox file and belongs to another calendar than unmarked lines do.
    fn mark(&self, task: &Task) -> Option<ListSlug> {
        (task.source.path == self.s.inbox_file && self.s.inbox_list.as_ref() != Some(&task.list))
            .then(|| task.list.clone())
    }

    /// Whether `resource` says which task it is: always for a resource whose `UID` is
    /// the task's; for one that keeps another `UID`, once it carries the link
    /// (`X-RESTASK-UID`) — bound to that `UID` (`X-RESTASK-OF`) when the task's UID is a
    /// counted one. A client that drops a property on its next write has it written
    /// again.
    fn linked(&self, uid: &TaskUid, resource: &RemoteResource) -> bool {
        let remote = &resource.task;
        (remote.managed && remote.task.uid == *uid)
            || (remote.adopted_as.as_ref() == Some(uid)
                && (uid.is_long() || remote.adopted_for.as_deref() == Some(seed_of(resource))))
    }

    /// The parent of a server resource as a task UID: the task its parent relation
    /// names — by another client's `UID`, by a long UID since renumbered, or by the
    /// parent's own.
    fn parent_of<'b>(&'b self, resource: &'b RemoteResource) -> Option<&'b TaskUid> {
        let raw = resource.task.parent_raw.as_deref()?;
        self.adopted_uids
            .get(raw)
            .or_else(|| self.s.wires.get(raw))
            .or(resource.task.task.parent.as_ref())
    }

    /// The server-side view the merge compares against.
    fn view<'b>(&'b self, resource: &'b RemoteResource) -> RemoteView<'b> {
        RemoteView {
            task: &resource.task.task,
            parent: self.parent_of(resource),
            source_path: resource.task.source_path.as_deref(),
        }
    }

    /// Whether the vault decides `local`'s parent: only for active tasks in notes, where
    /// indentation expresses nesting. The done region, TODO.md and the view of a root
    /// note are flat.
    fn parent_authoritative(&self, local: &Task) -> bool {
        local.status == Status::Active
            && local.source.path != self.s.inbox_file
            && !self.s.flat.contains(&local.uid)
    }

    /// Whether a resource that could not be read this cycle may be `uid`'s. Only a task
    /// the server has held can be one (a line never pushed is new, R2); its resource is
    /// the one named after the UID, or — for a task the server knows by another name
    /// (§9.5), which need not be what its resource is called — any unread one, in the
    /// list the task is in or the one it was settled in.
    fn unread(&self, uid: &TaskUid, local: Option<&Task>, entry: Option<&IndexEntry>) -> bool {
        if !entry.is_some_and(|entry| entry.caldav_etag.is_some()) {
            return false;
        }
        let lists = [local.map(|task| &task.list), entry.map(|entry| &entry.list)];
        let mut names = lists
            .into_iter()
            .flatten()
            .filter_map(|list| self.s.unread.get(list))
            .flatten()
            .peekable();
        names.peek().is_some()
            && (self.s.wires.uids().any(|other| other == uid)
                || names.any(|name| name == uid.as_str()))
    }

    /// R3 precondition: the task was settled, the collection it was settled in was
    /// listed this cycle and is not a reset one, and the task is gone from it.
    fn deleted_on_server(&self, local: &Task, entry: Option<&IndexEntry>) -> bool {
        entry.is_some_and(|entry| {
            entry.caldav_etag.is_some()
                && self.s.remote.contains_key(&entry.list)
                && !self.reset.contains(&entry.list)
                && !self.reset.contains(&local.list)
        })
    }

    /// Where a server-created task goes, and the task as it will exist there: its
    /// `X-RESTASK-SOURCE` note when that note still routes to the list, else the list's
    /// home note, else the inbox (`None`). The inbox list has no other home than the
    /// inbox; a list of `todo_lists` goes there only when no note is its home (§7.5).
    fn place(&self, slug: &ListSlug, resource: &RemoteResource) -> (Option<String>, Task) {
        let s = self.s;
        let mut task = resource.task.task.clone();
        task.list = slug.clone();
        task.source_heading = None;
        task.source.line = 0;
        let routed = resource
            .task
            .source_path
            .as_deref()
            .filter(|path| *path != s.inbox_file && s.notes.get(*path) == Some(slug))
            .map(str::to_string);
        let path = routed.or_else(|| {
            if s.inbox_list.as_ref() == Some(slug) {
                None
            } else {
                s.homes.get(slug).cloned()
            }
        });
        task.source.path = path.clone().unwrap_or_else(|| s.inbox_file.clone());
        // Only a parent in the same note can be expressed (by indentation).
        task.parent = self.parent_of(resource).cloned();
        (path, task)
    }
}

/// A recurring task rolled forward by one occurrence (§11.6).
struct Roll {
    /// The series: same UID, active again, dated at the next occurrence.
    series: Task,
    /// The completed occurrence, as a task of its own.
    record: Task,
    /// The series' extras, with the rule's `COUNT` decremented.
    extras: Vec<String>,
}

/// Rolls a recurring task forward when it is completed in the vault while the server
/// copy (if any) is still open. `None` when the task does not recur, when this was the
/// rule's last occurrence, or when the rule has ended: then the completion is an
/// ordinary one. The record's UID is the next of `ids`; without a counter, a long one
/// derived from the series and the occurrence.
fn roll_forward(
    merged: &Task,
    remote: Option<&RemoteResource>,
    ids: Option<&Counter>,
) -> Option<Roll> {
    let Status::Completed { on } = merged.status else {
        return None;
    };
    if remote.is_some_and(|resource| resource.task.task.status != Status::Active) {
        return None;
    }
    let mut extras = remote
        .map(|resource| resource.task.extras.clone())
        .unwrap_or_default();
    // The task's own rule (`🔁`), else a rule only the server can express.
    let (rule, unmanaged) = match &merged.recurrence {
        Some(rule) => (rule.clone(), None),
        None => {
            let (index, rule) = find_in_extras(&extras)?;
            (rule, Some(index))
        }
    };
    if rule.is_last() {
        return None;
    }
    // The occurrence just done is identified by its due date; a task without one by its
    // start, its scheduled date, or the day it was completed.
    let anchor = merged
        .due
        .or(merged.start)
        .or(merged.scheduled)
        .unwrap_or(When::Date(on));
    let next = rule.next_after(anchor, on)?;
    let shift = when_day(next) - when_day(anchor);
    let moved = |value: Option<When>| value.map(|value| shift_days(value, shift));

    let mut series = merged.clone();
    series.status = Status::Active;
    series.recurrence = merged.recurrence.as_ref().map(Recurrence::consumed);
    if merged.due.is_some() || (merged.start.is_none() && merged.scheduled.is_none()) {
        series.due = Some(next);
    }
    if merged.due.is_some() {
        series.start = moved(merged.start);
        series.scheduled = moved(merged.scheduled);
    } else if merged.start.is_some() {
        series.start = Some(next);
        series.scheduled = moved(merged.scheduled);
    } else if merged.scheduled.is_some() {
        series.scheduled = Some(next);
    }

    let mut record = merged.clone();
    record.uid = match ids {
        Some(ids) => ids.mint(),
        None => TaskUid::derived(
            &format!("{}/{}", merged.uid.as_str(), anchor.to_ical()),
            on.0.and_hms_opt(0, 0, 0).map(|at| at.and_utc()),
        ),
    };
    record.parent = None;
    record.recurrence = None;

    if let Some(line) = unmanaged.and_then(|index| extras.get_mut(index)) {
        *line = consume_count(line);
    }
    Some(Roll {
        series,
        record,
        extras,
    })
}

/// Queues the vault edits and the record's creation for a roll of the vault task
/// `local`; returns the series write.
fn apply_roll(p: &mut Plan, ctx: &Context<'_>, local: &Task, roll: Roll) -> (Task, Vec<String>) {
    let uid = &local.uid;
    tracing::info!(uid = %uid, record = %roll.record.uid, "task_recurred");
    let ops = p.mutations.entry(local.source.path.clone()).or_default();
    ops.push(Mutation::Rekey {
        uid: uid.clone(),
        new_uid: roll.record.uid.clone(),
    });
    // The series' new line shows a creation date only if its line did (§6.4), and names
    // its calendar if its line had to (§7.5).
    let mut draft = TaskDraft::from(&roll.series);
    draft.created = local.created;
    draft.list = ctx.mark(local);
    ops.push(Mutation::Insert { draft, under: None });
    p.puts.push(PutOp {
        name: roll.record.uid.as_str().to_string(),
        wire: ctx.wire(&roll.record),
        task: roll.record,
        extras: Vec::new(),
        if_match: None,
    });
    (roll.series, roll.extras)
}

/// The calendar day of a date or wall time.
fn when_day(value: When) -> chrono::NaiveDate {
    match value {
        When::Date(day) => day.0,
        When::DateTime(at) => at.0.date(),
    }
}

/// `value` moved by a whole number of days (its time of day, if any, is kept).
fn shift_days(value: When, shift: chrono::Duration) -> When {
    match value {
        When::Date(LocalDate(day)) => When::Date(LocalDate(day + shift)),
        When::DateTime(LocalDateTime(at)) => When::DateTime(LocalDateTime(at + shift)),
    }
}

/// What a resource is known by on the server: its `UID`, or — it has none — its name.
fn seed_of(resource: &RemoteResource) -> &str {
    if resource.task.raw_uid.is_empty() {
        &resource.name
    } else {
        &resource.task.raw_uid
    }
}

/// The delete operation for one server copy.
fn delete_of(copy: &Copy<'_>) -> DeleteOp {
    DeleteOp {
        list: copy.0.clone(),
        name: copy.1.name.clone(),
        etag: Some(copy.1.etag.clone()),
    }
}

/// `true` when the base snapshot or the index entry must be (re)written for `task`.
fn needs_settle(entry: Option<&IndexEntry>, base: Option<&Task>, task: &Task, etag: &str) -> bool {
    let (Some(entry), Some(base)) = (entry, base) else {
        return true;
    };
    let thumbprint = task.thumbprint();
    entry.thumbprint != thumbprint
        || base.thumbprint() != thumbprint
        || entry.caldav_etag.as_deref() != Some(etag)
        || entry.list != task.list
        || entry.source_path != task.source.path
        || entry.defer_count != 0
}

/// Turns the pending inserts into plan operations. A child goes under its parent when
/// the parent is an active task of the same note (already there, or inserted in this
/// plan — parents are emitted first); otherwise it becomes a root line (`orphan_subtask`).
fn schedule_inserts(p: &mut Plan, s: &Snapshots, mut pending: Vec<PendingInsert>) {
    let mut placed: BTreeMap<TaskUid, Option<String>> = BTreeMap::new();
    while !pending.is_empty() {
        // Emit every insert whose parent (if it is itself pending) is already emitted.
        let waiting: BTreeSet<TaskUid> = pending.iter().map(|i| i.task.uid.clone()).collect();
        let (ready, blocked): (Vec<_>, Vec<_>) = pending.into_iter().partition(|insert| {
            insert
                .parent
                .as_ref()
                .is_none_or(|parent| !waiting.contains(parent))
        });
        // A parent cycle between foreign tasks: break it by emitting everything.
        let (ready, blocked) = if ready.is_empty() {
            (blocked, Vec::new())
        } else {
            (ready, blocked)
        };
        for insert in ready {
            let PendingInsert {
                path,
                mut task,
                parent,
            } = insert;
            // A line does not show its creation date unless the user asks for it (§6.4);
            // the server keeps it.
            task.created = None;
            placed.insert(task.uid.clone(), path.clone());
            let Some(path) = path else {
                p.inbox_inserts.push(task);
                continue;
            };
            let under = parent.filter(|parent| {
                let in_vault = s
                    .local
                    .get(parent)
                    .is_some_and(|t| t.source.path == path && t.status == Status::Active);
                let in_plan = placed.get(parent) == Some(&Some(path.clone()));
                in_vault || in_plan
            });
            if task.parent.is_some() && under.is_none() {
                tracing::warn!(uid = %task.uid, "orphan_subtask");
            }
            p.mutations.entry(path).or_default().push(Mutation::Insert {
                draft: TaskDraft::from(&task),
                under,
            });
        }
        pending = blocked;
    }
}
