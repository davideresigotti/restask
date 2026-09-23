//! Taskres library: Markdown checkboxes ⇄ CalDAV VTODO sync.

pub mod caldav;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod domain;
pub mod error;
pub mod markdown;
pub mod router;
pub mod store;
pub mod sync;
pub mod vtodo;

pub use error::{CaldavErrorKind, TaskresError};
