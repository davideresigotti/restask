//! Task UIDs (§3.1): `"restask-"` + 26-char lowercase Crockford base32 (ULID). UIDs minted
//! before the project rename carry the legacy `"taskres-"` prefix and stay valid forever
//! (UIDs are eternal).

use std::fmt;
use std::hash::Hasher;
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::{DateTime, Utc};

use crate::domain::LocalDate;
use fnv::FnvHasher;

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

    /// Derives the UID a foreign task is adopted under (§11 R5): a pure function of the
    /// foreign `UID` and its creation instant, so adopting the same resource twice — a
    /// retry after a crash, or two devices racing — yields the same task instead of a
    /// duplicate. The ULID timestamp is `created_at` (keeps creation order in the inbox;
    /// the epoch when unknown), its 80 random bits are a hash of `foreign_uid`.
    pub fn derived(foreign_uid: &str, created_at: Option<DateTime<Utc>>) -> Self {
        let millis = created_at
            .map(|at| at.timestamp_millis())
            .and_then(|ms| u64::try_from(ms).ok())
            .unwrap_or(0);
        let ulid = ulid::Ulid::from_parts(millis, adoption_bits(foreign_uid));
        Self(format!("{}{}", UID_PREFIX, ulid.to_string().to_lowercase()))
    }

    /// Whether this UID is one the foreign task `foreign_uid` is adopted under: some
    /// [`TaskUid::derived`] of it, whatever the creation instant. It is what makes an
    /// `X-RESTASK-UID` property (§8.2) believable: a copy of the resource that another
    /// client made under a new `UID` does not inherit the original's identity.
    pub fn adopts(&self, foreign_uid: &str) -> bool {
        self.0
            .len()
            .checked_sub(ulid::ULID_LEN)
            .and_then(|at| self.0.get(at..))
            .and_then(|body| ulid::Ulid::from_string(body).ok())
            .is_some_and(|ulid| ulid.random() == adoption_bits(foreign_uid))
    }

    /// The day the task was created, read off the UID: the UTC calendar date of the
    /// ULID's timestamp — the instant a line was registered, or the foreign task's
    /// `CREATED` for an adopted one ([`TaskUid::derived`]). `None` when the UID carries
    /// no instant (a derived UID whose creation was unknown). Stands in for the server's
    /// `CREATED` where the server has none yet (§8.1).
    pub fn created_on(&self) -> Option<LocalDate> {
        let body = self.0.get(self.0.len().checked_sub(ulid::ULID_LEN)?..)?;
        let millis = ulid::Ulid::from_string(body).ok()?.timestamp_ms();
        if millis == 0 {
            return None;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(i64::try_from(millis).ok()?)?;
        Some(LocalDate(at.date_naive()))
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

/// The 80 random bits of every UID `foreign_uid` is adopted under: a hash of it.
fn adoption_bits(foreign_uid: &str) -> u128 {
    let hash = |salt: &[u8]| {
        let mut hasher = FnvHasher::default();
        hasher.write(salt);
        hasher.write(foreign_uid.as_bytes());
        u128::from(hasher.finish())
    };
    ((hash(b"restask-adopt-a") << 64) | hash(b"restask-adopt-b")) & ((1u128 << 80) - 1)
}

/// Locks the process-wide monotonic ULID generator, recovering it if poisoned.
fn global_generator() -> MutexGuard<'static, ulid::Generator> {
    static GENERATOR: OnceLock<Mutex<ulid::Generator>> = OnceLock::new();
    GENERATOR
        .get_or_init(|| Mutex::new(ulid::Generator::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
