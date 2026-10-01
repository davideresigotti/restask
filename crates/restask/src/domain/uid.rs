//! Task UIDs (§3.1): `"restask-"` + 26-char lowercase Crockford base32 (ULID). UIDs minted
//! before the project rename carry the legacy `"taskres-"` prefix and stay valid forever
//! (UIDs are eternal).

use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Prefix of every UID this build generates.
pub const UID_PREFIX: &str = "restask-";

/// Prefix of UIDs minted before the project was renamed; parsed and kept verbatim.
pub const LEGACY_UID_PREFIX: &str = "taskres-";

/// A stable, filename-safe task identifier: `restask-` + 26-char lowercase Crockford base32.
///
/// Used verbatim as `.restask/tasks/<uid>.ics` and as the Radicale resource name `<uid>.ics`.
/// UIDs are assigned once and never regenerated (ARCHITECTURE.md invariant 2). Ordering is
/// lexicographic, which for fixed-width lowercase ULIDs equals chronological creation order
/// (required by the TODO.md render's `BTreeMap<TaskUid, Task>`, §7).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct TaskUid(String);

/// Error returned when a string is not a well-formed task UID.
#[derive(Debug, thiserror::Error)]
#[error("invalid task UID: {0}")]
pub struct UidError(pub String);

impl TaskUid {
    /// Generates a fresh UID from the process-wide monotonic ULID generator, lowercased.
    ///
    /// Successive calls within one process never produce a smaller ULID than an earlier
    /// one. If the generator's random space overflows within a single millisecond
    /// (unreachable in practice), a plain random ULID is used instead.
    pub fn generate() -> Self {
        let mut generator = global_generator();
        let ulid = generator.generate().unwrap_or_else(|_| ulid::Ulid::new());
        Self(format!("{}{}", UID_PREFIX, ulid.to_string().to_lowercase()))
    }

    /// Returns the full UID string, e.g. `restask-01jzabcdefghjkmnpqrstvwxyz`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a UID: trims surrounding whitespace, lowercases, then validates the
    /// 26-char Crockford base32 body after the `restask-` (or legacy `taskres-`) prefix.
    pub fn parse(raw: &str) -> Result<Self, UidError> {
        let lowered = raw.trim().to_lowercase();
        let body = lowered
            .strip_prefix(UID_PREFIX)
            .or_else(|| lowered.strip_prefix(LEGACY_UID_PREFIX))
            .ok_or_else(|| UidError(raw.to_string()))?;
        if body.len() != ulid::ULID_LEN {
            return Err(UidError(raw.to_string()));
        }
        ulid::Ulid::from_string(body)
            .map(|_| Self(lowered))
            .map_err(|_| UidError(raw.to_string()))
    }
}

impl fmt::Display for TaskUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Locks the process-wide monotonic ULID generator, recovering it if poisoned.
fn global_generator() -> MutexGuard<'static, ulid::Generator> {
    static GENERATOR: OnceLock<Mutex<ulid::Generator>> = OnceLock::new();
    GENERATOR
        .get_or_init(|| Mutex::new(ulid::Generator::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
