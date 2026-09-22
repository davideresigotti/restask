//! VTODO codec (§8): iCalendar serialization and parsing. Pure — no I/O.

pub mod parse;
pub mod serialize;

pub use parse::{from_vcalendar, RemoteTask};
pub use serialize::to_vcalendar;
