//! CalDAV adapter (§10): pure XML protocol builders/parsers, the `CaldavPort` trait, and
//! the reqwest-backed client. Network access happens only here (all of it goes through
//! the port; static dispatch, no `dyn`).

pub mod client;
pub mod offline;
pub mod port;
pub mod protocol;

pub use client::CaldavClient;
pub use offline::Offline;
pub use port::{CaldavPort, RemoteResource};
pub use protocol::CollectionInfo;
