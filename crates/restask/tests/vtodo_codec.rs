//! T11 isolated suite: VTODO serializer — golden byte-equality, folding, escaping,
//! property order (§8.1, App. A).

use chrono::{DateTime, Utc};

use restask::domain::dates::{LocalDate, When};
use restask::domain::priority::Priority;
use restask::domain::task::{ListSlug, SourceRef, Status, Task};
use restask::domain::uid::TaskUid;
use restask::vtodo::to_vcalendar;

/// Byte-exact golden contract (App. A) — every line CRLF-terminated.
const GOLDEN: &str = include_str!("../../../docs/contracts/vtodo-golden.ics");

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-22T14:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn golden_task() -> Task {
    Task {
        uid: TaskUid::parse("taskres-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap(),
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
    assert!(out.contains("X-TASKRES-SOURCE;VALUE=TEXT:Notes\\; Sub\\, Dir/task.md\r\n"));
}

#[test]
fn property_order_full_task() {
    let uid = TaskUid::parse("taskres-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap();
    let parent = TaskUid::parse("taskres-01arz3ndektsv4rrffq69g5fav").unwrap();
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
PRODID:-//taskres//restask 0.1.0//EN
BEGIN:VTODO
UID:taskres-01jzetq1v2h3k4m5n6p7r8t9w0
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
RELATED-TO;TOREL=PARENT:taskres-01arz3ndektsv4rrffq69g5fav
X-TASKRES-SCHEDULED:20260923T091500
X-TASKRES-SOURCE;VALUE=TEXT:Notes\\; Sub\\, Dir/task.md
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
        "X-TASKRES-SCHEDULED",
    ] {
        assert!(!out.contains(prefix), "unexpected {prefix}");
    }
    // CREATED falls back to now_utc when the ➕ date is absent (§8.1).
    assert!(out.contains("CREATED:20260922T143000Z\r\n"));
    // X-TASKRES-SOURCE is always emitted.
    assert!(out.contains("X-TASKRES-SOURCE;VALUE=TEXT:Home Lab Test.md\r\n"));
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
    assert!(out.contains("X-TASKRES-SCHEDULED:20260923T073000\r\n"));
}
