//! Persistent per-vault state under `.restask/` (§9): the index, the base snapshots, the
//! tombstones, the remembered calendars, the wire names and the device claims. Adapter
//! layer over [`crate::fsio`].
//!
//! Every file is written atomically and only when its content changes; the index, the
//! snapshots and the renders are disposable — the next reconcile re-derives them from
//! vault + server.

pub mod cache;
pub mod calendars;
pub mod device;
pub mod index;
pub mod tombstones;
pub mod wires;

pub use cache::{cache_path, cache_read, cache_remove, cache_write};
pub use calendars::Calendars;
pub use device::{device_file, switched, Device};
pub use index::{Index, IndexEntry};
pub use tombstones::Tombstones;
pub use wires::Wires;
