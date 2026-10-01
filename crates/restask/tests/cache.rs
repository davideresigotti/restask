//! §9.2 tests: VTODO cache (`tasks/<uid>.ics`).

use std::fs;

use chrono::{DateTime, TimeZone, Utc};
use tempfile::tempdir;

use restask::domain::{
    ListSlug, LocalDate, LocalDateTime, Priority, SourceRef, Status, Task, TaskUid, When,
};

const UID: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const PARENT: &str = "restask-01arz3ndektsv4rrffq69g5fav";

fn uid(raw: &str) -> TaskUid {
    TaskUid::parse(raw).unwrap()
}

fn when_date(s: &str) -> When {
    When::Date(LocalDate::parse(s).unwrap())
}

fn when_dt(s: &str) -> When {
    When::DateTime(LocalDateTime::parse(s).unwrap())
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap()
}

fn sample_task() -> Task {
    Task {
        uid: uid(UID),
        list: ListSlug::from_name("home-lab").unwrap(),
        text: "Buy oat milk; then \"escape\" me".to_string(),
        status: Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap(),
        },
        priority: Some(Priority::High),
        due: Some(when_dt("2026-10-01 17:30")),
        start: Some(when_date("2026-09-21")),
        scheduled: Some(when_date("2026-09-19")),
        recurrence: None,
        created: Some(LocalDate::parse("2026-09-01").unwrap()),
        parent: Some(uid(PARENT)),
        source: SourceRef {
            path: "Home/Tasks.md".to_string(),
            line: 7,
        },
        source_heading: Some("Errands".to_string()),
        source_mtime: now(),
        last_modified: now(),
    }
}

#[test]
fn cache_path_layout() {
    let dir = tempdir().unwrap();
    let path = restask::store::cache_path(dir.path(), &uid(UID));
    assert_eq!(
        path.strip_prefix(dir.path()).unwrap(),
        std::path::Path::new("tasks").join(format!("{UID}.ics"))
    );
}

#[test]
fn cache_write_then_read_roundtrips() {
    let dir = tempdir().unwrap();
    let task = sample_task();
    restask::store::cache_write(dir.path(), &task, now()).unwrap();

    let tz = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
    let loaded = restask::store::cache_read(dir.path(), &uid(UID), &tz).unwrap();

    // Fields not representable in a VTODO are normalized by the parser:
    // line = 0, heading dropped, mtimes = LAST-MODIFIED (= now), list = placeholder.
    let expected = Task {
        source: SourceRef {
            path: "Home/Tasks.md".to_string(),
            line: 0,
        },
        source_heading: None,
        source_mtime: now(),
        last_modified: now(),
        list: ListSlug::from_name("cache").unwrap(),
        ..task.clone()
    };
    assert_eq!(loaded, expected);

    // Content fields survive the round trip exactly.
    assert_eq!(loaded.uid, uid(UID));
    assert_eq!(loaded.text, task.text);
    assert_eq!(
        loaded.status,
        Status::Completed {
            on: LocalDate::parse("2026-09-20").unwrap()
        }
    );
    assert_eq!(loaded.priority, Some(Priority::High));
    assert_eq!(loaded.due, Some(when_dt("2026-10-01 17:30")));
    assert_eq!(loaded.start, Some(when_date("2026-09-21")));
    assert_eq!(loaded.scheduled, Some(when_date("2026-09-19")));
    assert_eq!(
        loaded.created,
        Some(LocalDate::parse("2026-09-01").unwrap())
    );
    assert_eq!(loaded.parent, Some(uid(PARENT)));
}

#[test]
fn cache_write_bytes_match_push() {
    let dir = tempdir().unwrap();
    let task = sample_task();
    restask::store::cache_write(dir.path(), &task, now()).unwrap();

    let cached = fs::read_to_string(dir.path().join("tasks").join(format!("{UID}.ics"))).unwrap();
    assert_eq!(cached, restask::vtodo::to_vcalendar(&task, now()));
}

#[test]
fn cache_read_missing_is_none() {
    let dir = tempdir().unwrap();
    let tz = chrono::FixedOffset::east_opt(0).unwrap();
    assert!(restask::store::cache_read(dir.path(), &uid(UID), &tz).is_none());
    // A missing tasks/ directory is equally absent.
    let nested = dir.path().join("nope");
    assert!(restask::store::cache_read(&nested, &uid(UID), &tz).is_none());
}

#[test]
fn cache_read_unparseable_is_none() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("tasks")).unwrap();
    fs::write(
        dir.path().join("tasks").join(format!("{UID}.ics")),
        "not ics",
    )
    .unwrap();
    let tz = chrono::FixedOffset::east_opt(0).unwrap();
    assert!(restask::store::cache_read(dir.path(), &uid(UID), &tz).is_none());
}

#[test]
fn cache_read_foreign_is_none() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("tasks")).unwrap();
    let foreign = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:not-a-restask-uid\r\nSUMMARY:X\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    fs::write(dir.path().join("tasks").join(format!("{UID}.ics")), foreign).unwrap();
    let tz = chrono::FixedOffset::east_opt(0).unwrap();
    assert!(restask::store::cache_read(dir.path(), &uid(UID), &tz).is_none());
}

#[test]
fn cache_remove_is_idempotent() {
    let dir = tempdir().unwrap();
    // Removing an absent entry is fine.
    restask::store::cache_remove(dir.path(), &uid(UID)).unwrap();

    restask::store::cache_write(dir.path(), &sample_task(), now()).unwrap();
    restask::store::cache_remove(dir.path(), &uid(UID)).unwrap();
    assert!(!dir.path().join("tasks").join(format!("{UID}.ics")).exists());

    let tz = chrono::FixedOffset::east_opt(0).unwrap();
    assert!(restask::store::cache_read(dir.path(), &uid(UID), &tz).is_none());
    restask::store::cache_remove(dir.path(), &uid(UID)).unwrap();
}
