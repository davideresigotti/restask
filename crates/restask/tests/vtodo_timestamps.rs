//! T13 isolated suite: §4 timestamp contract conformance — every table row, both
//! directions, through a `FixedClock`. No new source files; this suite only exercises the
//! T11 serializer and T12 parser contracts.

use chrono::{DateTime, FixedOffset, Utc};

use restask::domain::dates::{Clock, LocalDate, When};
use restask::domain::priority::Priority;
use restask::domain::task::{ListSlug, SourceRef, Status, Task};
use restask::domain::uid::TaskUid;
use restask::vtodo::{from_vcalendar, to_vcalendar};

/// Deterministic clock (§3.3 port) so tests never depend on wall-clock time.
#[derive(Debug, Clone, Copy)]
struct FixedClock {
    now: DateTime<Utc>,
    today: LocalDate,
    offset: FixedOffset,
}

impl Clock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now
    }
    fn today_local(&self) -> LocalDate {
        self.today
    }
    fn local_offset(&self) -> FixedOffset {
        self.offset
    }
}

fn clock() -> FixedClock {
    FixedClock {
        now: ts("2026-09-22T14:30:00Z"),
        today: LocalDate::parse("2026-09-22").unwrap(),
        offset: FixedOffset::east_opt(2 * 3600).unwrap(),
    }
}

fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn list() -> ListSlug {
    ListSlug::from_name("Home Lab").unwrap()
}

fn base_task() -> Task {
    Task {
        uid: TaskUid::parse("restask-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap(),
        list: list(),
        text: "Timestamp contract".to_string(),
        status: Status::Active,
        priority: None,
        due: None,
        start: None,
        scheduled: None,
        created: None,
        parent: None,
        source: SourceRef {
            path: "Notes/Timestamps.md".to_string(),
            line: 4,
        },
        source_heading: None,
        source_mtime: ts("2026-09-19T10:00:00Z"),
        last_modified: ts("2026-09-19T10:00:00Z"),
    }
}

// ---- Serialize direction: Markdown tokens → VTODO properties ----

#[test]
fn created_date_synthesizes_midnight_utc() {
    let mut task = base_task();
    task.created = Some(LocalDate::parse("2026-09-19").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("CREATED:20260919T000000Z\r\n"));
}

#[test]
fn created_absent_falls_back_to_now_utc() {
    let out = to_vcalendar(&base_task(), clock().now_utc());
    assert!(out.contains("CREATED:20260922T143000Z\r\n"));
}

#[test]
fn completed_date_synthesizes_midnight_utc() {
    let mut task = base_task();
    task.status = Status::Completed {
        on: LocalDate::parse("2026-09-19").unwrap(),
    };
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("STATUS:COMPLETED\r\n"));
    assert!(out.contains("COMPLETED:20260919T000000Z\r\n"));
}

#[test]
fn date_due_uses_value_date() {
    let mut task = base_task();
    task.due = Some(When::parse_date_or_datetime("2026-09-19").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("DUE;VALUE=DATE:20260919\r\n"));
}

#[test]
fn datetime_due_is_floating() {
    let mut task = base_task();
    task.due = Some(When::parse_date_or_datetime("2026-09-19 17:00").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("DUE:20260919T170000\r\n"));
    assert!(!out.contains("DUE:20260919T170000Z"));
}

#[test]
fn dtstart_follows_the_same_date_vs_floating_rules() {
    let mut task = base_task();
    task.start = Some(When::parse_date_or_datetime("2026-09-18").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("DTSTART;VALUE=DATE:20260918\r\n"));
    task.start = Some(When::parse_date_or_datetime("2026-09-18 09:30").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("DTSTART:20260918T093000\r\n"));
    assert!(!out.contains("DTSTART:20260918T093000Z"));
}

#[test]
fn scheduled_follows_the_same_date_vs_floating_rules() {
    let mut task = base_task();
    task.scheduled = Some(When::parse_date_or_datetime("2026-09-23").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("X-RESTASK-SCHEDULED;VALUE=DATE:20260923\r\n"));
    task.scheduled = Some(When::parse_date_or_datetime("2026-09-23 07:15").unwrap());
    let out = to_vcalendar(&task, clock().now_utc());
    assert!(out.contains("X-RESTASK-SCHEDULED:20260923T071500\r\n"));
    assert!(!out.contains("X-RESTASK-SCHEDULED:20260923T071500Z"));
}

#[test]
fn last_modified_and_dtstamp_carry_now_utc() {
    let out = to_vcalendar(&base_task(), clock().now_utc());
    assert!(out.contains("DTSTAMP:20260922T143000Z\r\n"));
    assert!(out.contains("LAST-MODIFIED:20260922T143000Z\r\n"));
}

#[test]
fn same_task_and_now_is_byte_identical() {
    let task = base_task();
    assert_eq!(
        to_vcalendar(&task, clock().now_utc()),
        to_vcalendar(&task, clock().now_utc())
    );
}

// ---- Parse direction: VTODO properties → Markdown values (reverse rows) ----

#[test]
fn reverse_created_formats_the_utc_calendar_date() {
    // 23:30Z is already the next day in +05:30 local — the UTC date must win.
    let tz = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Created late\r\n",
        "CREATED:20260919T233000Z\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, tz, &list()).unwrap();
    assert_eq!(
        remote.task.created,
        Some(LocalDate::parse("2026-09-19").unwrap())
    );
}

#[test]
fn reverse_completed_formats_the_utc_calendar_date() {
    // §4 row: COMPLETED:20260920T033000Z → ✅ 2026-09-20 (UTC calendar date of the instant).
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Done\r\n",
        "STATUS:COMPLETED\r\n",
        "COMPLETED:20260920T033000Z\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, clock().local_offset(), &list()).unwrap();
    assert_eq!(
        remote.task.status,
        Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap()
        }
    );
}

#[test]
fn reverse_utc_due_becomes_device_local_wall_time() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:UTC due\r\n",
        "DUE:20260919T170000Z\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, clock().local_offset(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-09-19 19:00").unwrap())
    );
}

#[test]
fn reverse_tzid_due_becomes_device_local_wall_time() {
    // 17:00 in America/New_York (EDT, -04:00) = 21:00Z = 23:00 in device-local +02:00.
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Remote-zone due\r\n",
        "DUE;TZID=America/New_York:20260919T170000\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, clock().local_offset(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-09-19 23:00").unwrap())
    );
}

#[test]
fn reverse_value_date_stays_date_only() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:All-day\r\n",
        "DUE;VALUE=DATE:20260919\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, clock().local_offset(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::Date(LocalDate::parse("2026-09-19").unwrap()))
    );
}

#[test]
fn reverse_floating_due_stays_floating() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Floating\r\n",
        "DUE:20260919T170000\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, clock().local_offset(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-09-19 17:00").unwrap())
    );
}

// ---- Both directions through the fixed clock ----

#[test]
fn contract_round_trips_both_directions() {
    let clock = clock();
    let mut task = base_task();
    task.created = Some(LocalDate::parse("2026-09-19").unwrap());
    task.status = Status::Completed {
        on: LocalDate::parse("2026-09-20").unwrap(),
    };
    task.priority = Some(Priority::Medium);
    task.due = Some(When::parse_date_or_datetime("2026-09-25").unwrap());
    task.start = Some(When::parse_date_or_datetime("2026-09-24 08:00").unwrap());
    task.scheduled = Some(When::parse_date_or_datetime("2026-09-23 07:30").unwrap());

    let serialized = to_vcalendar(&task, clock.now_utc());
    let remote = from_vcalendar(&serialized, clock.local_offset(), &list()).unwrap();
    assert_eq!(remote.task.created, task.created);
    assert_eq!(remote.task.status, task.status);
    assert_eq!(remote.task.priority, task.priority);
    assert_eq!(remote.task.due, task.due);
    assert_eq!(remote.task.start, task.start);
    assert_eq!(remote.task.scheduled, task.scheduled);
    assert_eq!(to_vcalendar(&remote.task, clock.now_utc()), serialized);
}

#[test]
fn fixed_clock_drives_every_observed_instant() {
    // The whole suite observes time only through FixedClock — sanity-check the port.
    let clock = clock();
    assert_eq!(clock.now_utc(), ts("2026-09-22T14:30:00Z"));
    assert_eq!(clock.today_local().format(), "2026-09-22");
    assert_eq!(clock.local_offset().to_string(), "+02:00");
}
