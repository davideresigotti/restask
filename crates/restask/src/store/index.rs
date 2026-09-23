//! `.restask/index.json`: per-UID routing/etag bookkeeping (§9.1).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::task::ListSlug;
use crate::domain::uid::TaskUid;
use crate::markdown::mutator::write_atomic;
use crate::TaskresError;

/// Name of the index file inside `.restask/`.
const FILE_NAME: &str = "index.json";

/// Bookkeeping for one known task UID (§9.1).
///
/// `caldav_etag: Some(_)` means "was on the server" — the flag that distinguishes a *new
/// local task* (push) from a *server-side deletion* (tombstone locally).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Eternal task identifier (duplicated from the map key for entry-wise access).
    pub uid: TaskUid,
    /// List the task routes to.
    pub list: ListSlug,
    /// Vault-relative path of the source note.
    pub source_path: String,
    /// Content thumbprint (`Task::thumbprint`) at last reconciliation.
    pub thumbprint: u64,
    /// ETag of the server copy; `None` = never pushed.
    pub caldav_etag: Option<String>,
    /// Last time the engine saw this task.
    pub seen_at: DateTime<Utc>,
    /// Consecutive reconcile cycles in which a conflict decision was deferred.
    pub defer_count: u8,
}

/// UID → entry map (§9.1). A missing file loads as empty; saves are atomic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// All known entries, ordered by UID (lexicographic = chronological for ULIDs).
    pub entries: BTreeMap<TaskUid, IndexEntry>,
}

impl Index {
    /// Loads `.restask/index.json` from `dir`; a missing file yields an empty index.
    pub fn load(dir: &Path) -> Result<Index, TaskresError> {
        let path = dir.join(FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Index::default()),
            Err(e) => return Err(e.into()),
        };
        serde_json::from_str(&contents).map_err(|e| TaskresError::Validation {
            field: "index",
            reason: format!("{}: {e}", path.display()),
        })
    }

    /// Saves the index to `dir/index.json` atomically (tmp + fsync + rename), creating
    /// `dir` if needed.
    pub fn save(&self, dir: &Path) -> Result<(), TaskresError> {
        std::fs::create_dir_all(dir)?;
        let mut json =
            serde_json::to_string_pretty(self).map_err(|e| TaskresError::Validation {
                field: "index",
                reason: e.to_string(),
            })?;
        json.push('\n');
        write_atomic(&dir.join(FILE_NAME), &json)
    }

    /// Returns the entry for `uid`, if known.
    pub fn get(&self, uid: &TaskUid) -> Option<&IndexEntry> {
        self.entries.get(uid)
    }

    /// Inserts or replaces the entry for `e.uid`.
    pub fn upsert(&mut self, e: IndexEntry) {
        self.entries.insert(e.uid.clone(), e);
    }

    /// Forgets `uid` entirely.
    pub fn remove(&mut self, uid: &TaskUid) {
        self.entries.remove(uid);
    }
}
