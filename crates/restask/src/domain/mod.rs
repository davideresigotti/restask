//! Pure domain types (§3): UIDs, priorities, dates, tasks; no I/O, time comes from `Clock` or
//! parameters.

pub mod dates;
pub mod priority;
pub mod recurrence;
pub mod task;
pub mod uid;

pub use dates::{Clock, DateError, LocalDate, LocalDateTime, SystemClock, When};
pub use priority::Priority;
pub use recurrence::Recurrence;
pub use task::{ListSlug, SourceRef, Status, Task};
pub use uid::{Counter, DeviceTag, Ids, TaskUid, UidError};
