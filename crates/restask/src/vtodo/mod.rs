//! VTODO codec (§8): iCalendar serialization and parsing. Pure — no I/O.

pub mod serialize;

pub use serialize::to_vcalendar;
