//! Persistent per-vault state under `.taskres/` (§9). Adapter layer — together with
//! `sync::engine`, `setup`, and `daemon`, the only place that touches `std::fs`.
//!
//! Every state file is written atomically (hidden `.<name>.restask-tmp` + fsync + rename,
//! shared with the markdown mutator) once per reconcile cycle and is fully reconstructible
//! via `restask rebuild` from vault + Radicale.

pub mod index;
pub mod tombstones;

pub use index::{Index, IndexEntry};
pub use tombstones::Tombstones;
