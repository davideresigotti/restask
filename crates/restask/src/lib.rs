//! Taskres library: Markdown checkboxes ⇄ CalDAV VTODO sync.

pub mod config;
pub mod domain;
pub mod error;
pub mod router;

pub use error::{CaldavErrorKind, TaskresError};
