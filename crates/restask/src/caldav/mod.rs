//! CalDAV adapter (§10): pure XML protocol builders/parsers, the pure list ⇄ collection
//! resolution, the `CaldavPort` trait, and
//! the reqwest-backed client. Network access happens only here (all of it goes through
//! the port; static dispatch, no `dyn`).

pub mod binding;
pub mod client;
pub mod offline;
pub mod port;
pub mod protocol;

pub use binding::{list_name, resolve_list, Bound};
pub use client::CaldavClient;
pub use offline::Offline;
pub use port::{CaldavPort, RemoteResource};
pub use protocol::CollectionInfo;
