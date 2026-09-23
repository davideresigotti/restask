//! CalDAV adapter (§10): pure XML protocol builders/parsers, the `CaldavPort` trait, and
//! the reqwest-backed client. Network access happens only here (ARCHITECTURE.md rule:
//! all network access goes through the port; static dispatch, no `dyn`).

pub mod client;
pub mod port;
pub mod protocol;

pub use client::CaldavClient;
pub use port::CaldavPort;
pub use protocol::CollectionInfo;
