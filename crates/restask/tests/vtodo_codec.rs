//! T11 isolated suite: VTODO serializer — golden byte-equality, folding, escaping,
//! property order (§8.1, App. A).

use chrono::{DateTime, FixedOffset, Utc};

use restask::domain::dates::{LocalDate, When};
use restask::domain::priority::Priority;
use restask::domain::task::{ListSlug, SourceRef, Status, Task};
use restask::domain::uid::TaskUid;
use restask::vtodo::{from_vcalendar, to_vcalendar, to_vcalendar_as, to_vcalendar_with, WireNames};
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
        recurrence: None,
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
        recurrence: None,
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
RELATED-TO;RELTYPE=PARENT:restask-01arz3ndektsv4rrffq69g5fav
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
        "CREATED",
        "PRIORITY:",
        "DTSTART",
        "DUE",
        "COMPLETED",
        "RELATED-TO",
        "X-RESTASK-SCHEDULED",
    ] {
        assert!(!out.contains(prefix), "unexpected {prefix}");
    }
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
    let remote = from_vcalendar(&serialized, &tz_cet(), &list()).unwrap();
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
        text: "Complex; task, with \\slashes and no newline".to_string(),
        status: Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap(),
        },
        priority: Some(Priority::Low),
        due: Some(When::parse_date_or_datetime("2026-09-25 17:00").unwrap()),
        start: Some(When::parse_date_or_datetime("2026-09-24 08:00").unwrap()),
        scheduled: Some(When::parse_date_or_datetime("2026-09-23 07:30").unwrap()),
        recurrence: None,
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
    let remote = from_vcalendar(&serialized, &tz_cet(), &list()).unwrap();
    assert_same_task_fields(&remote.task, &original);
    assert_eq!(to_vcalendar(&remote.task, now()), serialized);
}

#[test]
fn golden_parses_and_round_trips() {
    let remote = from_vcalendar(GOLDEN, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.source_path, None);
}

#[test]
fn unfolds_folded_lines_and_accepts_lf() {
    let crlf_text = "BEGIN:VTODO\r\nUID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\nSUMMARY:Hello\r\n  world\r\nEND:VTODO\r\n";
    let lf_text = crlf_text.replace("\r\n", "\n");
    let expected = "Hello world";
    let from_crlf = from_vcalendar(crlf_text, &tz_cet(), &list()).unwrap();
    let from_lf = from_vcalendar(&lf_text, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
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
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert_eq!(
        remote.task.status,
        Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap()
        }
    );
}

// ---- tolerance: shared calendars hold other clients' resources ----

#[test]
fn a_task_without_a_summary_parses_with_empty_text() {
    let text = "BEGIN:VTODO\r\nUID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\nEND:VTODO\r\n";
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert!(remote.managed);
    assert_eq!(remote.task.text, "");
    assert_eq!(remote.task.created, None);
}

#[test]
fn malformed_optional_values_are_absent_not_fatal() {
    let text = concat!(
        "BEGIN:VTODO\r\n",
        "UID:restask-01jzetq1v2h3k4m5n6p7r8t9w0\r\n",
        "SUMMARY:Odd values\r\n",
        "DUE:not-a-date\r\n",
        "DTSTART;TZID=Not/AZone:20260925T170000\r\n",
        "CREATED:yesterday\r\n",
        "PRIORITY:high\r\n",
        "END:VTODO\r\n",
    );
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.task.due, None);
    // An unknown TZID is read as floating wall time.
    assert_eq!(
        remote.task.start,
        Some(When::parse_date_or_datetime("2026-09-25 17:00").unwrap())
    );
    assert_eq!(remote.task.created, None);
    assert_eq!(remote.task.priority, None);
}

#[test]
fn a_body_without_a_vtodo_is_the_only_error() {
    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Party\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    match from_vcalendar(text, &tz_cet(), &list()) {
        Err(RestaskError::Validation { field, .. }) => assert_eq!(field, "vtodo"),
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn a_multi_line_summary_becomes_one_markdown_line() {
    let text = "BEGIN:VTODO\r\nUID:x\r\nSUMMARY:first\\nsecond   third\\n\r\nEND:VTODO\r\n";
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.task.text, "first second third");
}

#[test]
fn standard_and_legacy_parent_relations_parse() {
    let parent = "restask-01arz3ndektsv4rrffq69g5fav";
    for line in [
        format!("RELATED-TO:{parent}"),
        format!("RELATED-TO;RELTYPE=PARENT:{parent}"),
        format!("RELATED-TO;TOREL=PARENT:{parent}"),
    ] {
        let text = format!("BEGIN:VTODO\r\nUID:x\r\nSUMMARY:child\r\n{line}\r\nEND:VTODO\r\n");
        let remote = from_vcalendar(&text, &tz_cet(), &list()).unwrap();
        assert_eq!(remote.parent_raw.as_deref(), Some(parent), "{line}");
        assert_eq!(remote.task.parent, Some(TaskUid::parse(parent).unwrap()));
        assert!(remote.extras.is_empty());
    }
    // A sibling/child relation is not a parent: it is foreign content, kept verbatim.
    let text = format!(
        "BEGIN:VTODO\r\nUID:x\r\nSUMMARY:c\r\nRELATED-TO;RELTYPE=SIBLING:{parent}\r\nEND:VTODO\r\n"
    );
    let remote = from_vcalendar(&text, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.parent_raw, None);
    assert_eq!(remote.extras.len(), 1);
    // A foreign parent UID is reported raw; the planner resolves it on adoption.
    let text = "BEGIN:VTODO\r\nUID:x\r\nSUMMARY:c\r\nRELATED-TO:abc@tasks.org\r\nEND:VTODO\r\n";
    let remote = from_vcalendar(text, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.parent_raw.as_deref(), Some("abc@tasks.org"));
    assert_eq!(remote.task.parent, None);
}

// ---- extras: what other clients store survives a push ----

/// A Tasks.org-style resource: description, tags, reminder, recurrence, vendor property.
const FOREIGN: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:+//IDN tasks.org//android//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Rome\r\nEND:VTIMEZONE\r\n\
BEGIN:VTODO\r\nDTSTAMP:20260922T101500Z\r\nUID:5417861935824551742\r\n\
CREATED:20260921T081233Z\r\nLAST-MODIFIED:20260922T101400Z\r\nSUMMARY:Pay the rent\r\n\
DESCRIPTION:IBAN in the lease\\, page 2\r\nCATEGORIES:home,money\r\nPRIORITY:1\r\n\
DUE;TZID=Europe/Rome:20261215T170000\r\nRRULE:FREQ=MONTHLY\r\n\
X-APPLE-SORT-ORDER:123\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nDESCRIPTION:Default Tasks.org description\r\nEND:VALARM\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

#[test]
fn unmanaged_content_is_collected_as_extras() {
    let remote = from_vcalendar(FOREIGN, &tz_cet(), &list()).unwrap();
    assert!(!remote.managed);
    assert_eq!(remote.raw_uid, "5417861935824551742");
    assert_eq!(remote.task.text, "Pay the rent");
    assert_eq!(remote.task.priority, Some(Priority::Highest));
    assert_eq!(
        remote.created_at,
        Some(
            DateTime::parse_from_rfc3339("2026-09-21T08:12:33Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
    assert_eq!(
        remote.extras,
        vec![
            "DESCRIPTION:IBAN in the lease\\, page 2",
            "CATEGORIES:home,money",
            "X-APPLE-SORT-ORDER:123",
            "BEGIN:VALARM",
            "TRIGGER:-PT15M",
            "ACTION:DISPLAY",
            "DESCRIPTION:Default Tasks.org description",
            "END:VALARM",
        ]
    );
}

#[test]
fn an_expressible_rrule_is_the_tasks_repeat_rule_a_richer_one_is_an_extra() {
    use restask::domain::Recurrence;
    let remote = from_vcalendar(FOREIGN, &tz_cet(), &list()).unwrap();
    assert_eq!(
        remote.task.recurrence,
        Some(Recurrence::from_text("every month").unwrap().0)
    );
    let rich = FOREIGN.replace(
        "RRULE:FREQ=MONTHLY",
        "RRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,FR",
    );
    let remote = from_vcalendar(&rich, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.task.recurrence, None);
    assert!(remote
        .extras
        .contains(&"RRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,FR".to_string()));

    // Written: the task's rule, once — it replaces a rule kept among the extras.
    let mut task = golden_task();
    task.recurrence = Some(Recurrence::from_text("every 2 weeks on Monday").unwrap().0);
    let out = to_vcalendar_with(&task, now(), &remote.extras);
    assert_eq!(out.matches("RRULE").count(), 1);
    assert!(out.contains(
        "PRIORITY:1\r\nDUE;VALUE=DATE:20260925\r\nRRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO\r\n"
    ));
    let back = from_vcalendar(&out, &tz_cet(), &list()).unwrap();
    assert_eq!(back.task.recurrence, task.recurrence);
    // Without a rule of its own, the task hands the server's rule back untouched.
    task.recurrence = None;
    let out = to_vcalendar_with(&task, now(), &remote.extras);
    assert!(out.contains("RRULE:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,FR\r\n"));
}

#[test]
fn extras_are_written_back_and_survive_the_round_trip() {
    let remote = from_vcalendar(FOREIGN, &tz_cet(), &list()).unwrap();
    let mut task = remote.task.clone();
    task.uid = TaskUid::parse("restask-01jzetq1v2h3k4m5n6p7r8t9w0").unwrap();
    task.text = "Pay the rent (edited in the vault)".to_string();
    let out = to_vcalendar_with(&task, now(), &remote.extras);
    for line in &remote.extras {
        assert!(out.contains(&format!("{line}\r\n")), "lost: {line}");
    }
    let alarm = out.find("BEGIN:VALARM").unwrap();
    assert!(alarm < out.find("END:VTODO").unwrap());
    assert!(alarm > out.find("X-RESTASK-SOURCE").unwrap());
    let again = from_vcalendar(&out, &tz_cet(), &list()).unwrap();
    assert_eq!(again.extras, remote.extras);
    assert_eq!(again.task.text, "Pay the rent (edited in the vault)");
}

#[test]
fn a_duration_extra_yields_to_a_written_due() {
    let extras = vec!["DURATION:PT1H".to_string(), "DESCRIPTION:keep".to_string()];
    let mut task = golden_task();
    let with_due = to_vcalendar_with(&task, now(), &extras);
    assert!(!with_due.contains("DURATION"));
    assert!(with_due.contains("DESCRIPTION:keep\r\n"));
    task.due = None;
    assert!(to_vcalendar_with(&task, now(), &extras).contains("DURATION:PT1H\r\n"));
}

#[test]
fn tzid_due_uses_the_offset_valid_on_its_own_date() {
    // 17:00 Rome in December is 16:00 UTC (CET, +01:00) — whatever the offset is today.
    // A zone with DST rules as the device zone must give the December offset back.
    let remote = from_vcalendar(FOREIGN, &chrono_tz::Europe::Rome, &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-12-15 17:00").unwrap())
    );
    let summer = FOREIGN.replace("20261215T170000", "20260715T170000");
    let remote = from_vcalendar(&summer, &chrono_tz::Europe::Rome, &list()).unwrap();
    assert_eq!(
        remote.task.due,
        Some(When::parse_date_or_datetime("2026-07-15 17:00").unwrap())
    );
}

#[test]
fn legacy_x_properties_and_uid_prefix_still_parse() {
    let body = crlf(
        "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VTODO\nUID:taskres-01jzetq1v2h3k4m5n6p7r8t9w0\n\
         DTSTAMP:20260922T143000Z\nSUMMARY:old\nX-TASKRES-SCHEDULED;VALUE=DATE:20260923\n\
         X-TASKRES-SOURCE;VALUE=TEXT:Home Lab Test.md\nEND:VTODO\nEND:VCALENDAR",
    );
    let remote = from_vcalendar(&body, &tz_cet(), &list()).unwrap();
    assert!(remote.managed);
    assert_eq!(remote.raw_uid, "taskres-01jzetq1v2h3k4m5n6p7r8t9w0");
    assert_eq!(remote.source_path.as_deref(), Some("Home Lab Test.md"));
    assert_eq!(
        remote.task.scheduled,
        Some(When::Date(LocalDate::parse("2026-09-23").unwrap()))
    );
}

// ── a task another client created keeps its UID (§8.1, §11 R5) ────────────────────────

#[test]
fn a_task_of_another_client_is_written_under_the_uid_it_was_given() {
    let remote = from_vcalendar(FOREIGN, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.adopted_as, None);
    let mut task = remote.task.clone();
    task.uid = TaskUid::derived(&remote.raw_uid, remote.created_at);
    task.parent = Some(TaskUid::derived("parent@tasks.org", None));
    task.source.path = "TODO.md".to_string();
    let wire = WireNames {
        uid: Some(remote.raw_uid.clone()),
        parent: Some("parent@tasks.org".to_string()),
    };
    let out = to_vcalendar_as(&task, now(), &remote.extras, &wire);
    assert!(
        out.contains("BEGIN:VTODO\r\nUID:5417861935824551742\r\n"),
        "{out}"
    );
    assert!(out.contains("RELATED-TO;RELTYPE=PARENT:parent@tasks.org\r\n"));
    assert!(out.contains(&format!(
        "X-RESTASK-SOURCE;VALUE=TEXT:TODO.md\r\nX-RESTASK-UID:{}\r\n",
        task.uid.as_str()
    )));
    assert_eq!(out.matches("\r\nUID:").count(), 1);

    // Read back: still the other client's resource, now saying which task it is; the
    // property is restask's own and never becomes an extra.
    let back = from_vcalendar(&out, &tz_cet(), &list()).unwrap();
    assert!(!back.managed);
    assert_eq!(back.raw_uid, "5417861935824551742");
    assert_eq!(back.adopted_as, Some(task.uid.clone()));
    assert_eq!(back.parent_raw.as_deref(), Some("parent@tasks.org"));
    assert_eq!(back.extras, remote.extras);
    assert!(task.uid.adopts(&back.raw_uid));

    // A task of restask's own is written as before.
    assert_eq!(
        to_vcalendar_as(&golden_task(), now(), &[], &WireNames::default()),
        GOLDEN
    );
    assert_eq!(to_vcalendar_with(&golden_task(), now(), &[]), GOLDEN);
}

#[test]
fn a_malformed_link_property_is_no_link() {
    let body = FOREIGN.replace("END:VTODO", "X-RESTASK-UID:not-a-uid\r\nEND:VTODO");
    let remote = from_vcalendar(&body, &tz_cet(), &list()).unwrap();
    assert_eq!(remote.adopted_as, None);
    assert!(!remote
        .extras
        .iter()
        .any(|line| line.contains("X-RESTASK-UID")));
}
