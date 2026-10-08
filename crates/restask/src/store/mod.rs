//! Persistent per-vault state under `.restask/` (§9): the index, the base snapshots, the
//! tombstones and the remembered calendars. Adapter layer over [`crate::fsio`].
//!
//! Every file is written atomically and only when its content changes; all of it is
//! disposable — the next reconcile re-derives it from vault + server.

pub mod cache;
pub mod calendars;
pub mod index;
pub mod tombstones;

pub use cache::{cache_path, cache_read, cache_remove, cache_write};
pub use calendars::Calendars;
pub use index::{Index, IndexEntry};
pub use tombstones::Tombstones;
