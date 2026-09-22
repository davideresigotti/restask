//! Pure domain types (§3): UIDs, priorities; no I/O, time comes from `Clock` or parameters.

pub mod priority;
pub mod uid;

pub use priority::Priority;
pub use uid::{TaskUid, UidError};
