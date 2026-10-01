//! `.restask/tombstones.json`: deleted UIDs with deletion timestamps (§9.1).
//!
//! Tombstones prevent a stale server copy (or late-arriving Syncthing peer) from
//! resurrecting a deleted task; entries older than 365 days are pruned so the file stays
//! bounded while UIDs are never reused (ARCHITECTURE.md invariant 2).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::uid::TaskUid;
use crate::fsio::write_if_changed;
use crate::RestaskError;

/// Name of the tombstones file inside `.restask/`.
const FILE_NAME: &str = "tombstones.json";

/// Deletion markers (§9.1): UID → instant of deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstones(BTreeMap<TaskUid, DateTime<Utc>>);

impl Tombstones {
    /// Loads `.restask/tombstones.json` from `dir`; a missing file yields an empty set.
    pub fn load(dir: &Path) -> Result<Self, RestaskError> {
        let path = dir.join(FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        serde_json::from_str(&contents)
            .map(Self)
            .map_err(|e| RestaskError::Validation {
                field: "tombstones",
                reason: format!("{}: {e}", path.display()),
            })
    }

    /// Saves the tombstones to `dir/tombstones.json` atomically (tmp + fsync + rename),
    /// creating `dir` if needed.
    pub fn save(&self, dir: &Path) -> Result<(), RestaskError> {
        std::fs::create_dir_all(dir)?;
        let mut json =
            serde_json::to_string_pretty(self).map_err(|e| RestaskError::Validation {
                field: "tombstones",
                reason: e.to_string(),
            })?;
        json.push('\n');
        write_if_changed(&dir.join(FILE_NAME), &json)?;
        Ok(())
    }

    /// Records `uid` as deleted at `at`.
    pub fn insert(&mut self, uid: TaskUid, at: DateTime<Utc>) {
        self.0.insert(uid, at);
    }

    /// Returns whether `uid` carries a deletion marker.
    pub fn contains(&self, uid: &TaskUid) -> bool {
        self.0.contains_key(uid)
    }

    /// Clears the marker for `uid` (the task reappeared in the vault, which is the
    /// source of truth). Returns whether a marker existed.
    pub fn remove(&mut self, uid: &TaskUid) -> bool {
        self.0.remove(uid).is_some()
    }

    /// The tombstoned UIDs, in order.
    pub fn uids(&self) -> impl Iterator<Item = &TaskUid> {
        self.0.keys()
    }

    /// Drops entries deleted more than `older_than` before `now`. Entries deleted exactly
    /// `older_than` ago are kept.
    pub fn prune(&mut self, older_than: Duration, now: DateTime<Utc>) {
        let cutoff = now - older_than;
        self.0.retain(|_, at| *at >= cutoff);
    }
}
