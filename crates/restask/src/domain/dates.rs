//! Dates, `When`, and the `Clock` port (§3.3). Pure — no I/O; [`SystemClock`] is the single
//! sanctioned system-time call site.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Utc};

/// Error returned when parsing a Markdown or iCalendar date value (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    /// The input does not match the expected `YYYY-MM-DD` / `YYYY-MM-DD HH:MM` shape.
    #[error("invalid date format: {0}")]
    InvalidFormat(String),
    /// The input is well-formed but names an impossible calendar value.
    #[error("date out of range: {0}")]
    OutOfRange(String),
}

/// A device-local calendar date (§3.3), serialized as `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LocalDate(
    /// Wrapped chrono date.
    pub NaiveDate,
);

/// A device-local wall time at minute precision (§3.3), serialized as `YYYY-MM-DD HH:MM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LocalDateTime(
    /// Wrapped chrono date-time (second and subsecond fields are always zero).
    pub NaiveDateTime,
);

/// A due/start/scheduled value: an all-day date or a floating local wall time (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum When {
    /// All-day date.
    Date(LocalDate),
    /// Floating local wall time (minute precision; no timezone).
    DateTime(LocalDateTime),
}

/// `YYYY-MM-DD` byte shape: 4 digits, `-`, 2 digits, `-`, 2 digits.
fn valid_date_shape(b: &[u8]) -> bool {
    b.len() == 10
        && all_digits(&b[..4])
        && b[4] == b'-'
        && all_digits(&b[5..7])
        && b[7] == b'-'
        && all_digits(&b[8..10])
}

/// Every byte is an ASCII digit.
fn all_digits(b: &[u8]) -> bool {
    b.iter().all(u8::is_ascii_digit)
}

impl LocalDate {
    /// Parses a strict `YYYY-MM-DD` date; rejects any other shape as
    /// [`DateError::InvalidFormat`] and impossible calendar values as
    /// [`DateError::OutOfRange`].
    pub fn parse(s: &str) -> Result<Self, DateError> {
        if !valid_date_shape(s.as_bytes()) {
            return Err(DateError::InvalidFormat(s.to_string()));
        }
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(LocalDate)
            .map_err(|_| DateError::OutOfRange(s.to_string()))
    }

    /// Formats as `YYYY-MM-DD`.
    pub fn format(self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }
}

impl LocalDateTime {
    /// Parses a strict `YYYY-MM-DD HH:MM` wall time; rejects any other shape as
    /// [`DateError::InvalidFormat`] and impossible clock values as
    /// [`DateError::OutOfRange`].
    pub fn parse(s: &str) -> Result<Self, DateError> {
        let b = s.as_bytes();
        let shaped = b.len() == 16
            && valid_date_shape(&b[..10])
            && b[10] == b' '
            && all_digits(&b[11..13])
            && b[13] == b':'
            && all_digits(&b[14..16]);
        if !shaped {
            return Err(DateError::InvalidFormat(s.to_string()));
        }
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
            .map(LocalDateTime)
            .map_err(|_| DateError::OutOfRange(s.to_string()))
    }

    /// Formats as `YYYY-MM-DD HH:MM`.
    pub fn format(self) -> String {
        self.0.format("%Y-%m-%d %H:%M").to_string()
    }
}

impl When {
    /// Parses the two Markdown forms: `"YYYY-MM-DD"` or `"YYYY-MM-DD HH:MM"`.
    pub fn parse_date_or_datetime(s: &str) -> Result<When, DateError> {
        match s.len() {
            10 => LocalDate::parse(s).map(When::Date),
            16 => LocalDateTime::parse(s).map(When::DateTime),
            _ => Err(DateError::InvalidFormat(s.to_string())),
        }
    }

    /// iCalendar form: `"20260919"` for dates, `"20260919T170000"` for datetimes
    /// (floating; never `Z`).
    pub fn to_ical(self) -> String {
        match self {
            When::Date(d) => d.0.format("%Y%m%d").to_string(),
            When::DateTime(dt) => dt.0.format("%Y%m%dT%H%M%S").to_string(),
        }
    }

    /// Parses the iCalendar forms produced by [`When::to_ical`] (basic date or floating
    /// date-time). UTC/TZID instants are converted to local values by the caller (§4).
    pub fn from_ical(s: &str) -> Result<When, DateError> {
        let b = s.as_bytes();
        if b.len() == 8 && all_digits(b) {
            return NaiveDate::parse_from_str(s, "%Y%m%d")
                .map(|d| When::Date(LocalDate(d)))
                .map_err(|_| DateError::OutOfRange(s.to_string()));
        }
        let shaped = b.len() == 15 && all_digits(&b[..8]) && b[8] == b'T' && all_digits(&b[9..15]);
        if shaped {
            return NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S")
                .map(|dt| When::DateTime(LocalDateTime(dt)))
                .map_err(|_| DateError::OutOfRange(s.to_string()));
        }
        Err(DateError::InvalidFormat(s.to_string()))
    }

    /// `true` for all-day dates.
    pub fn is_date_only(self) -> bool {
        matches!(self, When::Date(_))
    }
}

/// Time port for pure code (§3.3): the only way domain logic observes time.
pub trait Clock: Send + Sync {
    /// Current UTC instant.
    fn now_utc(&self) -> DateTime<Utc>;
    /// Today's calendar date in the device-local timezone.
    fn today_local(&self) -> LocalDate;
    /// The device-local UTC offset.
    fn local_offset(&self) -> FixedOffset;
}

/// [`Clock`] backed by the system clock and the platform local timezone (via chrono's
/// `Local`, which resolves the IANA timezone through `iana-time-zone`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn today_local(&self) -> LocalDate {
        LocalDate(chrono::Local::now().date_naive())
    }

    fn local_offset(&self) -> FixedOffset {
        *chrono::Local::now().offset()
    }
}
