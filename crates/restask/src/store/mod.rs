//! Persistent per-vault state under `.restask/` (§9): the index, the base snapshots and
//! the tombstones. Adapter layer over [`crate::fsio`].
//!
//! Every file is written atomically and only when its content changes; all of it is
//! disposable — the next reconcile re-derives it from vault + server.

pub mod cache;
pub mod index;
pub mod tombstones;

pub use cache::{cache_path, cache_read, cache_remove, cache_write};
pub use index::{Index, IndexEntry};
pub use tombstones::Tombstones;
