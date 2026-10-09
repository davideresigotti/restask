//! `.restask/wires.json`: the names tasks go by on the server where those are not their
//! UIDs (§9.5) — the `UID` another client gave a task, the long UID a task had before
//! it was renumbered (§11.7) — each with the UID the task has in the vault.
//!
//! It is written before the vault line it speaks of, so a pass that dies half way is
//! taken up by the next one under the same UIDs. The sync node's alone.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::uid::TaskUid;
use crate::fsio::write_if_changed;
use crate::RestaskError;

const FILE_NAME: &str = "wires.json";

/// Wire name → the UID of the task it names (§9.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wires(BTreeMap<String, TaskUid>);

impl Wires {
    /// Loads `<dir>/wires.json`; a missing file yields no names.
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
                field: "wires",
                reason: format!("{}: {e}", path.display()),
            })
    }

    /// Writes the names atomically (only when they changed), creating `dir` if absent.
    pub fn save(&self, dir: &Path) -> Result<(), RestaskError> {
        std::fs::create_dir_all(dir)?;
        let mut json =
            serde_json::to_string_pretty(self).map_err(|e| RestaskError::Validation {
                field: "wires",
                reason: e.to_string(),
            })?;
        json.push('\n');
        write_if_changed(&dir.join(FILE_NAME), &json)?;
        Ok(())
    }

    /// The UID of the task the server knows as `name`.
    pub fn get(&self, name: &str) -> Option<&TaskUid> {
        self.0.get(name)
    }

    /// Records that the server's `name` is the task `uid`; `true` when that is news.
    pub fn insert(&mut self, name: String, uid: TaskUid) -> bool {
        self.0.insert(name, uid.clone()) != Some(uid)
    }

    /// Every UID a name stands for.
    pub fn uids(&self) -> impl Iterator<Item = &TaskUid> {
        self.0.values()
    }

    /// Every name with its UID.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TaskUid)> {
        self.0.iter().map(|(name, uid)| (name.as_str(), uid))
    }

    /// `true` when no name is recorded.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
