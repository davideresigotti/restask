//! Persistent per-vault state under `.taskres/` (§9). Adapter layer — together with
//! `sync::engine`, `setup`, and `daemon`, the only place that touches `std::fs`.
//!
//! Every state file is written atomically (hidden `.<name>.restask-tmp` + fsync + rename,
//! shared with the markdown mutator) once per reconcile cycle and is fully reconstructible
//! via `restask rebuild` from vault + Radicale.

pub mod cache;
pub mod index;
pub mod outbox;
pub mod tombstones;

pub use cache::{cache_path, cache_read, cache_remove, cache_write};
pub use index::{Index, IndexEntry};
pub use outbox::{OutboundOp, Outbox};
pub use tombstones::Tombstones;
