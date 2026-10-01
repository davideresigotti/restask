//! T11 isolated suite: VTODO serializer — golden byte-equality, folding, escaping,
//! property order (§8.1, App. A).

use chrono::{DateTime, FixedOffset, Utc};

use restask::domain::dates::{LocalDate, When};
use restask::domain::priority::Priority;
use restask::domain::task::{ListSlug, SourceRef, Status, Task};
use restask::domain::uid::TaskUid;
use restask::vtodo::{from_vcalendar, to_vcalendar};
use restask::RestaskError;

/// Byte-exact golden contract (App. A) — every line CRLF-terminated.
const GOLDEN: &str = include_str!("../../../docs/contracts/vtodo-golden.ics");

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-22T14:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn golden_task() -> Task {
    Task {
        uid: TaskUid::parse("restask-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap(),
        list: ListSlug::from_name("Home Lab").unwrap(),
        text: "Setup SSL certificate renew alert".to_string(),
        status: Status::Active,
        priority: Some(Priority::Highest),
        due: Some(When::Date(LocalDate::parse("2026-09-25").unwrap())),
        start: None,
        scheduled: Some(When::Date(LocalDate::parse("2026-09-23").unwrap())),
        created: Some(LocalDate::parse("2026-09-19").unwrap()),
        parent: None,
        source: SourceRef {
            path: "Home Lab Test.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: now(),
        last_modified: now(),
    }
}

/// Appends CRLF to every line of a multi-line LF string (test helper).
fn crlf(s: &str) -> String {
    s.lines().collect::<Vec<_>>().join("\r\n") + "\r\n"
}

/// Extracts the physical lines forming the SUMMARY logical line (folded parts start with SPACE).
fn summary_lines(out: &str) -> Vec<&str> {
    out.lines()
        .skip_while(|l| !l.starts_with("SUMMARY:"))
        .take_while(|l| l.starts_with("SUMMARY:") || l.starts_with(' '))
        .collect()
}

/// Unfolds physical lines (continuations lose their leading SPACE) into the logical line.
fn unfold(parts: &[&str]) -> String {
    parts
        .iter()
        .enumerate()
        .map(|(i, l)| if i == 0 { *l } else { &l[1..] })
        .collect()
}

#[test]
fn golden_byte_equality() {
    assert_eq!(to_vcalendar(&golden_task(), now()), GOLDEN);
    assert!(GOLDEN.ends_with("END:VCALENDAR\r\n"));
}

#[test]
fn every_physical_line_within_75_octets() {
    for line in to_vcalendar(&golden_task(), now()).lines() {
        assert!(line.len() <= 75, "line is {} octets: {line}", line.len());
    }
}

#[test]
fn folds_long_summary_without_splitting_codepoints() {
    // "SUMMARY:" (8 octets) + 65 ASCII + 4-octet emoji + 1 ASCII = 78 octets: the fold must
    // land before the emoji (73 octets used), not inside it.
    let mut task = golden_task();
    task.text = format!("{}🔺y", "x".repeat(65));
    let out = to_vcalendar(&task, now());
    let summary = summary_lines(&out);
    assert_eq!(summary.len(), 2, "expected one fold: {summary:?}");
    assert_eq!(summary[0].len(), 73);
    assert_eq!(summary[1].len(), 6, "continuation = SPACE + emoji + y");
    assert_eq!(unfold(&summary), format!("SUMMARY:{}🔺y", "x".repeat(65)));
}

#[test]
fn fold_never_splits_a_multibyte_char_anywhere() {
    // Shift a 4-octet emoji through every position around the 75-octet boundary.
    for pad in 58..=75 {
        let mut task = golden_task();
        task.text = format!("{}🔺{}", "a".repeat(pad), "b".repeat(pad));
        let out = to_vcalendar(&task, now());
        for line in out.lines() {
            assert!(line.len() <= 75, "pad {pad}: {} octets", line.len());
            assert!(
                line.is_char_boundary(line.len()),
                "pad {pad}: split codepoint"
            );
        }
        let expected = format!("SUMMARY:{}🔺{}", "a".repeat(pad), "b".repeat(pad));
        assert_eq!(unfold(&summary_lines(&out)), expected);
    }
}

#[test]
fn escapes_summary_text() {
    let mut task = golden_task();
    task.text = "Back\\slash;semi,comma".to_string();
    let out = to_vcalendar(&task, now());
    assert!(out.contains("SUMMARY:Back\\\\slash\\;semi\\,comma\r\n"));
}

#[test]
fn escapes_source_path() {
    let mut task = golden_task();
    task.source.path = "Notes; Sub, Dir/task.md".to_string();
    let out = to_vcalendar(&task, now());
    assert!(out.contains("X-RESTASK-SOURCE;VALUE=TEXT:Notes\\; Sub\\, Dir/task.md\r\n"));
}

#[test]
fn property_order_full_task() {
    let uid = TaskUid::parse("restask-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap();
    let parent = TaskUid::parse("restask-01arz3ndektsv4rrffq69g5fav").unwrap();
    let task = Task {
        uid,
        list: ListSlug::from_name("Home Lab").unwrap(),
        text: "Order & shape; test, 🔼".to_string(),
        status: Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap(),
        },
        priority: Some(Priority::High),
        due: Some(When::parse_date_or_datetime("2026-09-25").unwrap()),
        start: Some(When::parse_date_or_datetime("2026-09-24 08:30").unwrap()),
        scheduled: Some(When::parse_date_or_datetime("2026-09-23 09:15").unwrap()),
        created: Some(LocalDate::parse("2026-09-19").unwrap()),
        parent: Some(parent),
        source: SourceRef {
            path: "Notes; Sub, Dir/task.md".to_string(),
            line: 3,
        },
        source_heading: None,
        source_mtime: now(),
        last_modified: now(),
    };
    let expected = crlf(
        "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//restask//restask 0.1.0//EN
BEGIN:VTODO
UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0
DTSTAMP:20260922T143000Z
CREATED:20260919T000000Z
LAST-MODIFIED:20260922T143000Z
SUMMARY:Order & shape\\; test\\, 🔼
STATUS:COMPLETED
PERCENT-COMPLETE:100
PRIORITY:3
DTSTART:20260924T083000
DUE;VALUE=DATE:20260925
COMPLETED:20260920T000000Z
RELATED-TO;TOREL=PARENT:restask-01arz3ndektsv4rrffq69g5fav
X-RESTASK-SCHEDULED:20260923T091500
X-RESTASK-SOURCE;VALUE=TEXT:Notes\\; Sub\\, Dir/task.md
END:VTODO
END:VCALENDAR",
    );
    assert_eq!(to_vcalendar(&task, now()), expected);
}

#[test]
fn omits_unset_optional_properties() {
    let mut task = golden_task();
    task.priority = None;
    task.due = None;
    task.scheduled = None;
    task.created = None;
    task.status = Status::Active;
    let out = to_vcalendar(&task, now());
    for prefix in [
        "PRIORITY:",
        "DTSTART",
        "DUE",
        "COMPLETED",
        "RELATED-TO",
        "X-RESTASK-SCHEDULED",
    ] {
        assert!(!out.contains(prefix), "unexpected {prefix}");
    }
    // CREATED falls back to now_utc when the ➕ date is absent (§8.1).
    assert!(out.contains("CREATED:20260922T143000Z\r\n"));
    // X-RESTASK-SOURCE is always emitted.
    assert!(out.contains("X-RESTASK-SOURCE;VALUE=TEXT:Home Lab Test.md\r\n"));
}

#[test]
fn datetime_when_emits_floating_form() {
    let mut task = golden_task();
    task.due = Some(When::parse_date_or_datetime("2026-09-25 17:00").unwrap());
    task.start = Some(When::parse_date_or_datetime("2026-09-24 08:00").unwrap());
    task.scheduled = Some(When::parse_date_or_datetime("2026-09-23 07:30").unwrap());
    let out = to_vcalendar(&task, now());
    assert!(out.contains("DTSTART:20260924T080000\r\n"));
    assert!(out.contains("DUE:20260925T170000\r\n"));
    assert!(out.contains("X-RESTASK-SCHEDULED:20260923T073000\r\n"));
}

// ---- T12: parser (§8.2) ----

fn tz_cet() -> FixedOffset {
    FixedOffset::east_opt(2 * 3600).unwrap()
}

fn list() -> ListSlug {
    ListSlug::from_name("Home Lab").unwrap()
}

fn assert_same_task_fields(a: &Task, b: &Task) {
    assert_eq!(a.uid, b.uid);
    assert_eq!(a.list, b.list);
    assert_eq!(a.text, b.text);
    assert_eq!(a.status, b.status);
    assert_eq!(a.priority, b.priority);
    assert_eq!(a.due, b.due);
    assert_eq!(a.start, b.start);
    assert_eq!(a.scheduled, b.scheduled);
    assert_eq!(a.created, b.created);
    assert_eq!(a.parent, b.parent);
}

#[test]
fn parses_own_output_round_trip() {
    let original = golden_task();
    let serialized = to_vcalendar(&original, now());
    let remote = from_vcalendar(&serialized, tz_cet(), &list()).unwrap();
    assert!(remote.managed);
    assert_eq!(remote.raw_uid, original.uid.as_str());
    assert_same_task_fields(&remote.task, &original);
    assert_eq!(
        remote.source_path.as_deref(),
        Some(original.source.path.as_str())
    );
    // Round-trip property (§8.2): parse(to_vcalendar(t, now)) re-serializes byte-identically.
    assert_eq!(to_vcalendar(&remote.task, now()), serialized);
}

#[test]
fn round_trip_full_task_with_datetimes() {
    let parent = TaskUid::parse("restask-01arz3ndektsv4rrffq69g5fav").unwrap();
    let original = Task {
        uid: TaskUid::parse("restask-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap(),
        list: ListSlug::from_name("Home Lab").unwrap(),
        text: "Complex; task, with \\slashes\nand newline".to_string(),
        status: Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap(),
        },
        priority: Some(Priority::Low),
        due: Some(When::parse_date_or_datetime("2026-09-25 17:00").unwrap()),
        start: Some(When::parse_date_or_datetime("2026-09-24 08:00").unwrap()),
        scheduled: Some(When::parse_date_or_datetime("2026-09-23 07:30").unwrap()),
        created: Some(LocalDate::parse("2026-09-19").unwrap()),
        parent: Some(parent),
        source: SourceRef {
            path: "Notes; Sub, Dir/task.md".to_string(),
            line: 3,
        },
        source_heading: None,
        source_mtime: now(),
        last_modified: now(),
    };
    let serialized = to_vcalendar(&original, now());
    let remote = from_vcalendar(&serialized, tz_cet(), &list()).unwrap();
    assert_same_task_fields(&remote.task, &original);
    assert_eq!(to_vcalendar(&remote.task, now()), serialized);
}

#[test]
fn golden_parses_and_round_trips() {
    let remote = from_vcalendar(GOLDEN, tz_cet(), &list()).unwrap();
    assert!(remote.managed);
    assert_eq!(remote.task.text, "Setup SSL certificate renew alert");
    assert_eq!(remote.task.priority, Some(Priority::Highest));
    assert_eq!(remote.task.status, Status::Active);
    assert_eq!(
        remote.task.due,
        Some(When::Date(LocalDate::parse("2026-09-25").unwrap()))
    );
    assert_eq!(
        remote.task.scheduled,
        Some(When::Date(LocalDate::parse("2026-09-23").unwrap()))
    );
    assert_eq!(
        remote.task.created,
        Some(LocalDate::parse("2026-09-19").unwrap())
    );
    assert_eq!(to_vcalendar(&remote.task, now()), GOLDEN);
}

#[test]
fn flags_foreign_uid_and_populates_fields() {
    let text = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTODO\r\n",
        "UID:custom-app-12345@other-client\r\n",
        "SUMMARY:Foreign; item\\, escaped\r\n",
        "PRIORITY:0\r\n",
        "X-RESTASK-SOURCE;VALUE=TEXT:Inbox.md\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert!(!remote.managed);
    assert_eq!(remote.raw_uid, "custom-app-12345@other-client");
    let placeholder = TaskUid::parse("restask-00000000000000000000000000").unwrap();
    assert_eq!(remote.task.uid, placeholder);
    assert_eq!(remote.task.text, "Foreign; item, escaped");
    assert_eq!(remote.task.priority, None, "PRIORITY 0 is unmapped");
    assert_eq!(remote.source_path.as_deref(), Some("Inbox.md"));
}

#[test]
fn missing_source_property_yields_none() {
    let text = concat!(
        "BEGIN:VCALENDAR\r\n",
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:No source here\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert_eq!(remote.source_path, None);
}

#[test]
fn unfolds_folded_lines_and_accepts_lf() {
    let crlf_text = "BEGIN:VTODO\r\nUID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\nSUMMARY:Hello\r\n  world\r\nEND:VTODO\r\n";
    let lf_text = crlf_text.replace("\r\n", "\n");
    let expected = "Hello world";
    let from_crlf = from_vcalendar(crlf_text, tz_cet(), &list()).unwrap();
    let from_lf = from_vcalendar(&lf_text, tz_cet(), &list()).unwrap();
    assert_eq!(from_crlf.task.text, expected);
    assert_eq!(from_lf.task.text, expected);
}

#[test]
fn skips_vtimezone_valarm_and_unknown_properties() {
    let text = concat!(
        "BEGIN:VCALENDAR\r\n",
        "VERSION:2.0\r\n",
        "BEGIN:VTIMEZONE\r\n",
        "TZID:Europe/Rome\r\n",
        "BEGIN:STANDARD\r\n",
        "SUMMARY:Standard time leak\r\n",
        "DTSTART:19701025T030000\r\n",
        "END:STANDARD\r\n",
        "END:VTIMEZONE\r\n",
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "X-CUSTOM-PROP:must be ignored\r\n",
        "SUMMARY:Real summary\r\n",
        "BEGIN:VALARM\r\n",
        "SUMMARY:Alarm leak\r\n",
        "END:VALARM\r\n",
        "END:VTODO\r\n",
        "END:VCALENDAR\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert_eq!(remote.task.text, "Real summary");
}

#[test]
fn utc_instant_due_becomes_local_wall_time() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:UTC due\r\n",
        "DUE:20260919T170000Z\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-09-19 19:00").unwrap())
    );
}

#[test]
fn tzid_due_becomes_local_wall_time() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:TZID due\r\n",
        "DUE;TZID=Europe/Rome:20260919T170000\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-09-19 17:00").unwrap())
    );
}

#[test]
fn completed_uses_utc_calendar_date() {
    // UTC 2026-09-20T23:30Z is already 2026-09-21 in local +02:00 — the §4 reverse rule
    // formats the UTC calendar date, so `on` must stay 2026-09-20.
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Done late\r\n",
        "DTSTAMP:20260922T143000Z\r\n",
        "STATUS:COMPLETED\r\n",
        "COMPLETED:20260920T233000Z\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, tz_cet(), &list()).unwrap();
    assert_eq!(
        remote.task.status,
        Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap()
        }
    );
}

#[test]
fn missing_summary_is_an_error() {
    let text = "BEGIN:VTODO\r\nUID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\nEND:VTODO\r\n";
    match from_vcalendar(text, tz_cet(), &list()) {
        Err(RestaskError::Validation { field, .. }) => assert_eq!(field, "summary"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn malformed_due_is_an_error() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Bad due\r\n",
        "DUE:not-a-date\r\n",
        "END:VTODO\r\n",
    );
    match from_vcalendar(text, tz_cet(), &list()) {
        Err(RestaskError::Validation { field, .. }) => assert_eq!(field, "due"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn legacy_x_properties_and_uid_prefix_still_parse() {
    let body = crlf(
        "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VTODO\nUID:taskres-01jzetq1v2h3k4m5n6p7r8t9w0\n\
         DTSTAMP:20260922T143000Z\nSUMMARY:old\nX-TASKRES-SCHEDULED;VALUE=DATE:20260923\n\
         X-TASKRES-SOURCE;VALUE=TEXT:Home Lab Test.md\nEND:VTODO\nEND:VCALENDAR",
    );
    let remote = from_vcalendar(&body, tz_cet(), &list()).unwrap();
    assert!(remote.managed);
    assert_eq!(remote.raw_uid, "taskres-01jzetq1v2h3k4m5n6p7r8t9w0");
    assert_eq!(remote.source_path.as_deref(), Some("Home Lab Test.md"));
    assert_eq!(
        remote.task.scheduled,
        Some(When::Date(LocalDate::parse("2026-09-23").unwrap()))
    );
}
