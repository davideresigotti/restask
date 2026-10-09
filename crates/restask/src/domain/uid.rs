//! Task UIDs (§3.1): `"restask-"` + the tag of the device that minted the UID and that
//! device's counter (`restask-a42`, written `🆔 a42` on a task line). UIDs minted before
//! the counters were 26-char lowercase Crockford base32 (ULID) behind `"restask-"` — or
//! the legacy `"taskres-"` of before the project rename; they are read wherever they
//! still are, and the sync node renumbers the ones in the vault (§11.7).

use std::cmp::Ordering;
use std::fmt;
use std::hash::Hasher;
use std::sync::atomic::{AtomicU64, Ordering as Atomic};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::{DateTime, Utc};

use crate::domain::LocalDate;
use fnv::FnvHasher;

/// Prefix of every UID this build generates.
pub const UID_PREFIX: &str = "restask-";

/// Prefix of UIDs minted before the project was renamed; parsed and kept verbatim.
pub const LEGACY_UID_PREFIX: &str = "taskres-";

/// Longest device tag, in letters.
const TAG_MAX: usize = 4;

/// Most digits of a UID's counter: any such number fits a `u64`.
const NUMBER_MAX: usize = 15;

/// The UID a task of another client is parsed under until the planner names it (§8.2):
/// a long UID no device mints.
const PLACEHOLDER: &str = "restask-00000000000000000000000000";

/// The name a device mints UIDs under (§3.1): one to four lowercase letters, its own in
/// the vault (§9.4) — so two devices never mint the same UID, online or not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceTag(String);

impl DeviceTag {
    /// Parses a tag: one to four letters `a`–`z`.
    pub fn parse(raw: &str) -> Result<Self, UidError> {
        let fits = (1..=TAG_MAX).contains(&raw.len());
        if fits && raw.bytes().all(|byte| byte.is_ascii_lowercase()) {
            Ok(Self(raw.to_string()))
        } else {
            Err(UidError(raw.to_string()))
        }
    }

    /// The tag's letters.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Every tag of `letters` letters, in alphabetical order: what a device chooses its
    /// own from.
    pub fn all(letters: usize) -> Vec<DeviceTag> {
        let mut tags = vec![String::new()];
        for _ in 0..letters.clamp(1, TAG_MAX) {
            tags = tags
                .iter()
                .flat_map(|head| ('a'..='z').map(move |letter| format!("{head}{letter}")))
                .collect();
        }
        tags.into_iter().map(DeviceTag).collect()
    }
}

impl fmt::Display for DeviceTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The UIDs one device mints (§3.1): its tag and the last number it used. Counting goes
/// on from the highest number seen under the tag, so a UID is never handed out twice —
/// not one the device minted before, not one that is on a line already.
#[derive(Debug)]
pub struct Counter {
    tag: DeviceTag,
    last: AtomicU64,
}

impl Clone for Counter {
    fn clone(&self) -> Self {
        Self::new(self.tag.clone(), self.last())
    }
}

impl PartialEq for Counter {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.last() == other.last()
    }
}

impl Eq for Counter {}

impl Counter {
    /// A counter for the device `tag` whose highest number so far is `last` (0: none).
    pub fn new(tag: DeviceTag, last: u64) -> Self {
        Self {
            tag,
            last: AtomicU64::new(last),
        }
    }

    /// Takes note of a UID that exists: one of this device's is never minted again.
    pub fn observe(&self, uid: &TaskUid) {
        if let Some((tag, number)) = uid.minted_by() {
            if tag == self.tag.as_str() {
                self.last.fetch_max(number, Atomic::Relaxed);
            }
        }
    }

    /// A UID no task has.
    pub fn mint(&self) -> TaskUid {
        let number = self.last.fetch_add(1, Atomic::Relaxed).saturating_add(1);
        TaskUid::minted(&self.tag, number)
    }

    /// The device the counter is of.
    pub fn tag(&self) -> &DeviceTag {
        &self.tag
    }

    /// The highest number used so far.
    pub fn last(&self) -> u64 {
        self.last.load(Atomic::Relaxed)
    }
}

/// Where a device takes fresh UIDs from in a vault (§3.1, §9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ids {
    /// The vault's sync node mints counted UIDs, and so does this device: its counter.
    Counted(Counter),
    /// The vault's sync node has not switched it to counted UIDs (§9.4): long ones, as
    /// before the counters — a sync node of an earlier version would not read another.
    Long,
}

impl Ids {
    /// Takes note of a UID that exists ([`Counter::observe`]).
    pub fn observe(&self, uid: &TaskUid) {
        if let Ids::Counted(counter) = self {
            counter.observe(uid);
        }
    }

    /// A UID no task has.
    pub fn mint(&self) -> TaskUid {
        match self {
            Ids::Counted(counter) => counter.mint(),
            Ids::Long => TaskUid::generate(),
        }
    }

    /// The counter, when the UIDs are counted ones.
    pub fn counter(&self) -> Option<&Counter> {
        match self {
            Ids::Counted(counter) => Some(counter),
            Ids::Long => None,
        }
    }
}

/// A stable, filename-safe task identifier: `restask-` + a device tag and a number
/// (`restask-a42`), or — minted before the counters — `restask-` + 26-char lowercase
/// Crockford base32.
///
/// Used verbatim as `.restask/tasks/<uid>.ics` and as the Radicale resource name
/// `<uid>.ics` of a task restask creates. A UID is assigned once and stays with its task
/// (ARCHITECTURE.md invariant 3). Ordering is creation order as far as a UID tells it —
/// the long UIDs first, by their timestamp; then the counted ones by number, then tag —
/// which is what the renders sort by (§7).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskUid(String);

impl Ord for TaskUid {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for TaskUid {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Error returned when a string is not a well-formed task UID.
#[derive(Debug, thiserror::Error)]
#[error("invalid task UID: {0}")]
pub struct UidError(pub String);

impl TaskUid {
    /// A fresh long UID, from the process-wide monotonic ULID generator, lowercased:
    /// what a device mints in a vault that has not been switched to counted UIDs
    /// ([`Ids::Long`]). Successive calls within one process never produce a smaller
    /// ULID than an earlier one.
    pub fn generate() -> Self {
        let mut generator = global_generator();
        let ulid = generator.generate().unwrap_or_else(|_| ulid::Ulid::new());
        Self(format!("{}{}", UID_PREFIX, ulid.to_string().to_lowercase()))
    }

    /// The UID number `number` of the device `tag` (§3.1): `restask-<tag><number>`.
    pub fn minted(tag: &DeviceTag, number: u64) -> Self {
        Self(format!("{UID_PREFIX}{tag}{number}"))
    }

    /// The tag and the number of a counted UID; `None` for a long one.
    pub fn minted_by(&self) -> Option<(&str, u64)> {
        split_counted(self.0.strip_prefix(UID_PREFIX)?)
    }

    /// Whether this is a long UID, of before the counters: one the sync node renumbers
    /// where it finds it on a line (§11.7).
    pub fn is_long(&self) -> bool {
        self.minted_by().is_none()
    }

    /// The UID as a task line spells it behind `🆔` (§6.1): tag and number alone
    /// (`a42`) — the line stays short — and a long UID in full.
    pub fn token(&self) -> &str {
        match self.minted_by() {
            Some(_) => &self.0[UID_PREFIX.len()..],
            None => &self.0,
        }
    }

    /// The UID a task of another client carries until the planner names it (§8.2).
    pub fn placeholder() -> Self {
        Self(PLACEHOLDER.to_string())
    }

    /// The sort key (see the type): long UIDs first, as strings; counted ones by number,
    /// then tag.
    fn rank(&self) -> (bool, u64, &str) {
        match self.minted_by() {
            Some((tag, number)) => (true, number, tag),
            None => (false, 0, &self.0),
        }
    }

    /// Derives the long UID a foreign task was adopted under before the counters (§11
    /// R5): a pure function of the foreign `UID` and its creation instant. It is what a
    /// planner without a counter still adopts under, and what [`TaskUid::adopts`]
    /// recognises. The ULID timestamp is `created_at` (the epoch when unknown), its 80
    /// random bits are a hash of `foreign_uid`.
    pub fn derived(foreign_uid: &str, created_at: Option<DateTime<Utc>>) -> Self {
        let millis = created_at
            .map(|at| at.timestamp_millis())
            .and_then(|ms| u64::try_from(ms).ok())
            .unwrap_or(0);
        let ulid = ulid::Ulid::from_parts(millis, adoption_bits(foreign_uid));
        Self(format!("{}{}", UID_PREFIX, ulid.to_string().to_lowercase()))
    }

    /// Whether this UID is one the foreign task `foreign_uid` was adopted under before
    /// the counters: some [`TaskUid::derived`] of it, whatever the creation instant. It
    /// is what makes an `X-RESTASK-UID` property with a long UID (§8.2) believable: a
    /// copy of the resource that another client made under a new `UID` does not inherit
    /// the original's identity. Never true of a counted UID, which says nothing of the
    /// task it names (`X-RESTASK-OF` binds that link, §8.1).
    pub fn adopts(&self, foreign_uid: &str) -> bool {
        self.is_long()
            && self
                .0
                .len()
                .checked_sub(ulid::ULID_LEN)
                .and_then(|at| self.0.get(at..))
                .and_then(|body| ulid::Ulid::from_string(body).ok())
                .is_some_and(|ulid| ulid.random() == adoption_bits(foreign_uid))
    }

    /// The day the task was created, where the UID tells: the UTC calendar date of a
    /// long UID's timestamp — the instant its line was registered, or the foreign task's
    /// `CREATED` for an adopted one ([`TaskUid::derived`]). `None` for a counted UID, and
    /// for a derived one whose creation was unknown. Stands in for the server's
    /// `CREATED` where the server has none yet (§8.1).
    pub fn created_on(&self) -> Option<LocalDate> {
        if !self.is_long() {
            return None;
        }
        let body = self.0.get(self.0.len().checked_sub(ulid::ULID_LEN)?..)?;
        let millis = ulid::Ulid::from_string(body).ok()?.timestamp_ms();
        if millis == 0 {
            return None;
        }
        let at = DateTime::<Utc>::from_timestamp_millis(i64::try_from(millis).ok()?)?;
        Some(LocalDate(at.date_naive()))
    }

    /// Returns the full UID string, e.g. `restask-a42`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a UID in full: trims surrounding whitespace, lowercases, then reads
    /// `restask-` and a counted body (`restask-a42`), or a long UID — the 26-char
    /// Crockford base32 body after the `restask-` (or legacy `taskres-`) prefix. It is
    /// how a UID is read from a `VTODO` and from a file name: without the prefix a value
    /// is another client's, whatever it looks like.
    pub fn parse(raw: &str) -> Result<Self, UidError> {
        let lowered = raw.trim().to_lowercase();
        let counted = lowered
            .strip_prefix(UID_PREFIX)
            .is_some_and(|body| split_counted(body).is_some());
        if counted {
            return Ok(Self(lowered));
        }
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

    /// Parses a UID as a task line spells it behind `🆔` ([`TaskUid::token`]): a counted
    /// UID without the prefix (`a42`), or a long one in full.
    pub fn from_token(raw: &str) -> Result<Self, UidError> {
        let token = raw.trim();
        if split_counted(token).is_some() {
            return Ok(Self(format!("{UID_PREFIX}{token}")));
        }
        Self::parse(token).and_then(|uid| {
            if uid.is_long() {
                Ok(uid)
            } else {
                Err(UidError(raw.to_string()))
            }
        })
    }
}

/// Splits the body of a counted UID into its tag and its number: one to four letters
/// `a`–`z`, then a number without a leading zero. `None` for anything else.
fn split_counted(body: &str) -> Option<(&str, u64)> {
    let letters = body.bytes().take_while(u8::is_ascii_lowercase).count();
    let (tag, digits) = body.split_at(letters);
    let counted = (1..=TAG_MAX).contains(&letters)
        && (1..=NUMBER_MAX).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    counted
        .then(|| digits.parse().ok())
        .flatten()
        .map(|number| (tag, number))
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
