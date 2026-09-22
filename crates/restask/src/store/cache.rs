//! `.taskres/tasks/<uid>.ics`: VTODO cache holding the same bytes pushed to Radicale (§9).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, Utc};

use crate::domain::task::{ListSlug, Task};
use crate::domain::uid::TaskUid;
use crate::markdown::mutator::write_atomic;
use crate::vtodo::{from_vcalendar, to_vcalendar};
use crate::TaskresError;

/// Subdirectory of `.taskres/` holding the cached VTODOs.
const TASKS_DIR: &str = "tasks";

/// Returns the cache path `.taskres/tasks/<uid>.ics` for `uid`.
pub fn cache_path(dir: &Path, uid: &TaskUid) -> PathBuf {
    dir.join(TASKS_DIR).join(format!("{}.ics", uid.as_str()))
}

/// Placeholder list for cached VTODOs: the cache file carries no list property (routing
/// truth lives in the index), so callers overwrite `Task::list` from the index entry.
/// `"cache"` is pure ASCII letters, hence always a valid slug; the `Option` mirrors the
/// no-unwrap handling of the static regex table in `markdown::parser`.
fn cache_list() -> Option<ListSlug> {
    static LIST: OnceLock<Result<ListSlug, TaskresError>> = OnceLock::new();
    LIST.get_or_init(|| ListSlug::from_name("cache"))
        .as_ref()
        .ok()
        .cloned()
}

/// Reads and parses the cached VTODO for `uid`; `None` when absent, unreadable, foreign,
/// or unparseable (a corrupt cache is disposable — `rebuild` re-derives it).
///
/// The returned `Task`'s `list` field is a placeholder; overwrite it from the index.
pub fn cache_read(dir: &Path, uid: &TaskUid, tz: FixedOffset) -> Option<Task> {
    let path = cache_path(dir, uid);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unreadable VTODO cache entry");
            return None;
        }
    };
    let list = cache_list()?;
    let remote = match from_vcalendar(&contents, tz, &list) {
        Ok(remote) => remote,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unparseable VTODO cache entry");
            return None;
        }
    };
    if !remote.managed || remote.task.uid != *uid {
        tracing::warn!(path = %path.display(), uid = %remote.task.uid, "foreign VTODO cache entry");
        return None;
    }
    Some(remote.task)
}

/// Serializes `task` (with `now_utc` as DTSTAMP/LAST-MODIFIED) and writes it to the cache
/// atomically, creating the `tasks/` directory if needed. The bytes match what is pushed
/// to Radicale for the same task and instant.
pub fn cache_write(dir: &Path, task: &Task, now_utc: DateTime<Utc>) -> Result<(), TaskresError> {
    std::fs::create_dir_all(dir.join(TASKS_DIR))?;
    write_atomic(&cache_path(dir, &task.uid), &to_vcalendar(task, now_utc))
}

/// Removes the cached VTODO for `uid`; a missing entry is not an error.
pub fn cache_remove(dir: &Path, uid: &TaskUid) -> Result<(), TaskresError> {
    match std::fs::remove_file(cache_path(dir, uid)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
