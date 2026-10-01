//! restask library: Markdown checkboxes ⇄ CalDAV VTODO sync.
//!
//! Layering (see `ARCHITECTURE.md`): `domain`, `router`, `markdown`, `vtodo`,
//! `sync::merge` and `sync::planner` are pure; `fsio`, `vault`, `store`, `caldav::client`,
//! `sync::engine`, `daemon`, `setup` and `cli` are the adapters around them.

pub mod caldav;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod domain;
pub mod error;
pub mod fsio;
pub mod logging;
pub mod markdown;
pub mod router;
pub mod setup;
pub mod store;
pub mod sync;
pub mod tui;
pub mod vault;
pub mod vtodo;

pub use error::{CaldavErrorKind, RestaskError};
