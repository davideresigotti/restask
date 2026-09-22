//! Taskres library: Markdown checkboxes ⇄ CalDAV VTODO sync.

pub mod config;
pub mod domain;
pub mod error;
pub mod markdown;
pub mod router;
pub mod store;
pub mod vtodo;

pub use error::{CaldavErrorKind, TaskresError};
