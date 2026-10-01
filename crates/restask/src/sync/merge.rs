//! Field-level three-way merge of one task (§11.3). Pure — no I/O.
//!
//! Three versions of a task meet in every reconcile: the **vault** line, the **server**
//! resource, and the **base** — the content both last agreed on. Per field:
//!
//! * equal on both sides → nothing to do;
//! * only one side differs from the base → that side changed, its value wins;
//! * both differ from the base (or there is no base) → a true conflict, settled by
//!   `LAST-MODIFIED`: the server wins only when it is newer than the vault file by more
//!   than the tie window; otherwise the vault wins (vault authority).
//!
//! Timestamps therefore matter only for genuine conflicts. That is what makes the merge
//! safe: a vault file's mtime covers *all* its tasks and is bumped by every unrelated
//! edit (and by the engine itself), so it says nothing about whether one given task
//! changed — the base does.

use crate::domain::{Status, Task, TaskUid};
use crate::markdown::mutator::{Mutation, WhenField};

/// Conflict tie window in seconds (§11.3): devices are assumed NTP-synced to ±60 s.
pub const TIE_WINDOW_SECS: i64 = 120;

/// Outcome of merging one task (§11.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    /// The agreed content, with the vault task's identity and placement (uid, list,
    /// source, heading, mtimes).
    pub task: Task,
    /// Vault edits that turn the local line into `task` (empty when the vault already
    /// matches), in application order.
    pub mutations: Vec<Mutation>,
    /// `true` when the server copy differs from `task` and must be replaced.
    pub push: bool,
    /// `true` when some field was changed on both sides to different values.
    pub conflict: bool,
}

/// The server-side view of a task that the merge compares against.
#[derive(Debug, Clone, Copy)]
pub struct RemoteView<'a> {
    /// The parsed server copy.
    pub task: &'a Task,
    /// Its parent relation, already resolved to a managed UID when possible.
    pub parent: Option<&'a TaskUid>,
    /// Its `X-RESTASK-SOURCE` value.
    pub source_path: Option<&'a str>,
}

/// Merges `local` and `remote` over `base` (§11.3).
///
/// `parent_authoritative` says whether the vault decides the parent relation (an active
/// task in a note, where indentation expresses it); otherwise the server's relation is
/// kept as-is, because the done region and the flat TODO.md view cannot express one.
pub fn merge(
    base: Option<&Task>,
    local: &Task,
    remote: RemoteView<'_>,
    parent_authoritative: bool,
) -> Merged {
    let remote_newer = remote
        .task
        .last_modified
        .signed_duration_since(local.last_modified)
        .num_seconds()
        > TIE_WINDOW_SECS;
    let mut conflict = false;
    let mut pick = Picker {
        remote_wins_conflicts: remote_newer,
        conflict: &mut conflict,
    };

    let mut task = local.clone();
    task.text = pick.field(base.map(|b| &b.text), &local.text, &remote.task.text);
    task.status = pick.field(base.map(|b| &b.status), &local.status, &remote.task.status);
    task.priority = pick.field(
        base.map(|b| &b.priority),
        &local.priority,
        &remote.task.priority,
    );
    task.due = pick.field(base.map(|b| &b.due), &local.due, &remote.task.due);
    task.start = pick.field(base.map(|b| &b.start), &local.start, &remote.task.start);
    task.scheduled = pick.field(
        base.map(|b| &b.scheduled),
        &local.scheduled,
        &remote.task.scheduled,
    );
    // Not merged: the creation date is the vault's ➕ token when present (else the
    // server's is kept), and the parent follows the rule documented above.
    task.created = local.created.or(remote.task.created);
    if !parent_authoritative {
        task.parent = remote.parent.cloned();
    }

    let push = fields_differ(&task, remote.task)
        || task.parent.as_ref() != remote.parent
        || task.created != remote.task.created
        || remote.source_path != Some(task.source.path.as_str());

    Merged {
        mutations: mutations_between(local, &task),
        task,
        push,
        conflict,
    }
}

/// `true` when two versions differ in a field the merge manages (text, status, priority,
/// due, start, scheduled).
pub fn fields_differ(a: &Task, b: &Task) -> bool {
    a.text != b.text
        || a.status != b.status
        || a.priority != b.priority
        || a.due != b.due
        || a.start != b.start
        || a.scheduled != b.scheduled
}

/// Per-field three-way choice.
struct Picker<'a> {
    remote_wins_conflicts: bool,
    conflict: &'a mut bool,
}

impl Picker<'_> {
    fn field<T: PartialEq + Clone>(&mut self, base: Option<&T>, local: &T, remote: &T) -> T {
        if local == remote {
            return local.clone();
        }
        match base {
            Some(base) if base == local => remote.clone(),
            Some(base) if base == remote => local.clone(),
            _ => {
                *self.conflict = true;
                if self.remote_wins_conflicts {
                    remote.clone()
                } else {
                    local.clone()
                }
            }
        }
    }
}

/// The line mutations that turn `from` into `to` (§11.3 decomposition): a completion
/// stamps the line and moves it under the done heading, a reopening restores it, every
/// other field maps to its setter. Empty when the managed fields already agree.
pub fn mutations_between(from: &Task, to: &Task) -> Vec<Mutation> {
    let uid = &to.uid;
    let mut ops = Vec::new();
    let mut tail = Vec::new();
    match (from.status, to.status) {
        (Status::Active, Status::Completed { on }) => {
            ops.push(Mutation::SetStatus {
                uid: uid.clone(),
                checked: true,
                completed_on: Some(on),
            });
            tail.push(Mutation::MoveToDone { uid: uid.clone() });
        }
        (Status::Completed { .. }, Status::Active) => {
            ops.push(Mutation::RestoreFromDone { uid: uid.clone() });
            ops.push(Mutation::SetStatus {
                uid: uid.clone(),
                checked: false,
                completed_on: None,
            });
        }
        (Status::Completed { on: before }, Status::Completed { on: after }) if before != after => {
            ops.push(Mutation::SetStatus {
                uid: uid.clone(),
                checked: true,
                completed_on: Some(after),
            });
        }
        _ => {}
    }
    if from.text != to.text {
        ops.push(Mutation::EditText {
            uid: uid.clone(),
            text: to.text.clone(),
        });
    }
    if from.priority != to.priority {
        ops.push(Mutation::SetPriority {
            uid: uid.clone(),
            priority: to.priority,
        });
    }
    for (field, before, after) in [
        (WhenField::Due, from.due, to.due),
        (WhenField::Start, from.start, to.start),
        (WhenField::Scheduled, from.scheduled, to.scheduled),
    ] {
        if before != after {
            ops.push(Mutation::SetWhen {
                uid: uid.clone(),
                field,
                value: after,
            });
        }
    }
    ops.extend(tail);
    ops
}
