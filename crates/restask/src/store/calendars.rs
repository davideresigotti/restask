//! `.restask/calendars.json` (§9): what the sync node remembers about the server's
//! calendars — which of them it has seen, and which lists it found under another path
//! than their own. Disposable like all state: without it nothing is new and nothing
//! was bound.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::ListSlug;
use crate::fsio::write_if_changed;
use crate::RestaskError;

const FILE: &str = "calendars.json";

/// The remembered calendars of the server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calendars {
    /// Path segments of the collections seen so far; `None` until the first look, when
    /// every calendar is taken as known and none as new (§7.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known: Option<BTreeSet<String>>,
    /// Lists whose collection is not at their own path, with the path segment it was
    /// found at (§5.4). A list that is found somewhere else than last time has changed
    /// collections, and what is missing there was not deleted.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bound: BTreeMap<ListSlug, String>,
}

impl Calendars {
    /// Loads the file from the state directory `dir`; a missing file is the empty state.
    pub fn load(dir: &Path) -> Result<Self, RestaskError> {
        let contents = match std::fs::read_to_string(dir.join(FILE)) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_str(&contents).map_err(|error| RestaskError::Validation {
            field: "calendars.json",
            reason: error.to_string(),
        })
    }

    /// Writes the file when its content changed.
    pub fn save(&self, dir: &Path) -> Result<(), RestaskError> {
        let contents =
            serde_json::to_string_pretty(self).map_err(|error| RestaskError::Validation {
                field: "calendars.json",
                reason: error.to_string(),
            })?;
        std::fs::create_dir_all(dir)?;
        write_if_changed(&dir.join(FILE), &contents)?;
        Ok(())
    }
}
