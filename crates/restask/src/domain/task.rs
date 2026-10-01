//! Task aggregate, status, source references, and list slugs (§3.4–3.5). Pure — no I/O.

use std::hash::Hasher;

use chrono::{DateTime, Utc};
use fnv::FnvHasher;

use crate::domain::dates::{LocalDate, When};
use crate::domain::priority::Priority;
use crate::domain::recurrence::Recurrence;
use crate::domain::uid::TaskUid;
use crate::RestaskError;

/// Kebab-case list identifier derived from a display name (§3.4): `"Home Lab"` → `home-lab`.
///
/// Ordering is lexicographic by slug (required by `sync::planner::Snapshots`' per-list
/// remote map, §11.1).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ListSlug(String);

impl ListSlug {
    /// Slugifies a display name: lowercase; every run of non-`[a-z0-9]` characters becomes a
    /// single `-`; leading/trailing `-` trimmed; the result must be non-empty.
    pub fn from_name(name: &str) -> Result<Self, RestaskError> {
        let mut slug = String::with_capacity(name.len());
        let mut separator = true;
        for ch in name.to_lowercase().chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                slug.push(ch);
                separator = false;
            } else if !separator {
                slug.push('-');
                separator = true;
            }
        }
        let trimmed = slug.trim_matches('-');
        if trimmed.is_empty() {
            return Err(RestaskError::Validation {
                field: "list",
                reason: format!("name `{name}` produces an empty list slug"),
            });
        }
        Ok(Self(trimmed.to_string()))
    }

    /// Returns the kebab-case slug, e.g. `home-lab`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Human-readable display name: title-cases each dash-separated word,
    /// e.g. `home-lab` → `Home Lab`.
    pub fn display_name(&self) -> String {
        self.0
            .split('-')
            .map(title_case)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Uppercases the first character of `word`, leaving the rest untouched.
fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Lifecycle state (§3.5): active, or completed on a device-local date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Status {
    /// Not yet done.
    Active,
    /// Done on the wrapped date.
    Completed {
        /// Completion date.
        on: LocalDate,
    },
}

/// Where a task lives in the vault (§3.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceRef {
    /// Vault-relative path, `/`-separated.
    pub path: String,
    /// 1-based line number within the file.
    pub line: usize,
}

/// A task parsed from (or destined for) the vault (§3.5).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Task {
    /// Eternal task identifier (ARCHITECTURE.md invariant 2).
    pub uid: TaskUid,
    /// List this task belongs to.
    pub list: ListSlug,
    /// User summary; metadata tokens stripped.
    pub text: String,
    /// Lifecycle state.
    pub status: Status,
    /// Optional priority.
    pub priority: Option<Priority>,
    /// Optional due value.
    pub due: Option<When>,
    /// Optional start value.
    pub start: Option<When>,
    /// Optional scheduled value.
    pub scheduled: Option<When>,
    /// Optional repeat rule (`🔁 every …` ⇄ `RRULE`); only rules both spellings express.
    pub recurrence: Option<Recurrence>,
    /// Optional creation date.
    pub created: Option<LocalDate>,
    /// Nearest ancestor checkbox (indentation-based), if any.
    pub parent: Option<TaskUid>,
    /// Vault-relative location of the task line.
    pub source: SourceRef,
    /// Nearest preceding ATX heading (render only; NOT persisted to VTODO).
    pub source_heading: Option<String>,
    /// mtime of the source file at parse time.
    pub source_mtime: DateTime<Utc>,
    /// max(mtime, engine mutation time).
    pub last_modified: DateTime<Utc>,
}

impl Task {
    /// FNV-1a (64-bit) over a canonical serialization of:
    /// uid, list, text, status, priority, due, start, scheduled, created, parent,
    /// recurrence.
    /// Excludes: source line number, source_heading, mtimes.
    ///
    /// Fields are length-prefixed and options carry an explicit presence marker, so the
    /// encoding is unambiguous and stable across runs.
    pub fn thumbprint(&self) -> u64 {
        let mut h = FnvHasher::default();
        put(&mut h, self.uid.as_str().as_bytes());
        put(&mut h, self.list.as_str().as_bytes());
        put(&mut h, self.text.as_bytes());
        match self.status {
            Status::Active => put(&mut h, b"active"),
            Status::Completed { on } => {
                put(&mut h, b"completed");
                put(&mut h, on.format().as_bytes());
            }
        }
        put_opt(&mut h, self.priority.map(|p| vec![p.to_ical()]).as_deref());
        put_opt_str(&mut h, self.due.map(When::to_ical).as_deref());
        put_opt_str(&mut h, self.start.map(When::to_ical).as_deref());
        put_opt_str(&mut h, self.scheduled.map(When::to_ical).as_deref());
        put_opt_str(&mut h, self.created.map(LocalDate::format).as_deref());
        put_opt_str(&mut h, self.parent.as_ref().map(TaskUid::as_str));
        put_opt_str(
            &mut h,
            self.recurrence
                .as_ref()
                .map(Recurrence::to_rrule)
                .as_deref(),
        );
        h.finish()
    }
}

/// Feeds `bytes` into the hash with a u64 little-endian length prefix (unambiguous framing).
fn put(h: &mut FnvHasher, bytes: &[u8]) {
    h.write(&(bytes.len() as u64).to_le_bytes());
    h.write(bytes);
}

/// Feeds an optional byte payload: a `0` marker when absent, `1` + framed bytes when present.
fn put_opt(h: &mut FnvHasher, bytes: Option<&[u8]>) {
    match bytes {
        Some(payload) => {
            h.write(&[1]);
            put(h, payload);
        }
        None => h.write(&[0]),
    }
}

/// [`put_opt`] for optional string values.
fn put_opt_str(h: &mut FnvHasher, value: Option<&str>) {
    put_opt(h, value.map(str::as_bytes));
}
