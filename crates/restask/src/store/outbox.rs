//! `.restask/outbox.json`: CalDAV operations parked after retry exhaustion (§9).
//!
//! Parked operations are replayed by later reconcile cycles; the vault and index remain
//! reconstructible regardless of outbox contents.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::task::{ListSlug, Task};
use crate::domain::uid::TaskUid;
use crate::markdown::mutator::write_atomic;
use crate::TaskresError;

/// Name of the outbox file inside `.restask/`.
const FILE_NAME: &str = "outbox.json";

/// One pending CalDAV operation (§9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutboundOp {
    /// Push the task (PUT); etag handling is resolved at replay time.
    Put {
        /// The task to push.
        task: Task,
    },
    /// Delete the resource for `uid` from its collection, guarded by `etag` when known.
    Delete {
        /// UID of the resource to delete.
        uid: TaskUid,
        /// Collection the resource lives in.
        list: ListSlug,
        /// ETag from the index (precondition guard); `None` = delete unconditionally.
        etag: Option<String>,
    },
}

/// FIFO queue of parked operations (§9.2). A missing file loads as empty; saves are atomic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Outbox {
    /// Parked operations, oldest first.
    queue: VecDeque<OutboundOp>,
}

impl Outbox {
    /// Loads `.restask/outbox.json` from `dir`; a missing file yields an empty outbox.
    pub fn load(dir: &Path) -> Result<Self, TaskresError> {
        let path = dir.join(FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        serde_json::from_str(&contents).map_err(|e| TaskresError::Validation {
            field: "outbox",
            reason: format!("{}: {e}", path.display()),
        })
    }

    /// Saves the outbox to `dir/outbox.json` atomically (tmp + fsync + rename), creating
    /// `dir` if needed.
    pub fn save(&self, dir: &Path) -> Result<(), TaskresError> {
        std::fs::create_dir_all(dir)?;
        let mut json =
            serde_json::to_string_pretty(self).map_err(|e| TaskresError::Validation {
                field: "outbox",
                reason: e.to_string(),
            })?;
        json.push('\n');
        write_atomic(&dir.join(FILE_NAME), &json)
    }

    /// Appends `op` to the end of the queue.
    pub fn push(&mut self, op: OutboundOp) {
        self.queue.push_back(op);
    }

    /// Drains the queue in FIFO order, leaving the outbox empty.
    pub fn take_all(&mut self) -> Vec<OutboundOp> {
        std::mem::take(&mut self.queue).into()
    }
}
