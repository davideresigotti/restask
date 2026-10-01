//! T04 isolated suite: `ListSlug` slugify/display, `Status`, `Task` thumbprint (§3.4–3.5).

use chrono::Utc;

use restask::domain::dates::{LocalDate, When};
use restask::domain::priority::Priority;
use restask::domain::task::{ListSlug, SourceRef, Status, Task};
use restask::domain::uid::TaskUid;
use restask::RestaskError;

fn slug_of(name: &str) -> String {
    ListSlug::from_name(name).unwrap().as_str().to_string()
}

fn ts(s: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&Utc)
}

fn sample_task() -> Task {
    Task {
        uid: TaskUid::parse("restask-01arz3ndektsv4rrffq69g5fav").unwrap(),
        list: ListSlug::from_name("Home Lab").unwrap(),
        text: "Fix the router".to_string(),
        status: Status::Active,
        priority: Some(Priority::High),
        due: Some(When::parse_date_or_datetime("2026-09-19").unwrap()),
        start: None,
        scheduled: None,
        recurrence: None,
        created: Some(LocalDate::parse("2026-09-01").unwrap()),
        parent: None,
        source: SourceRef {
            path: "notes/home.md".to_string(),
            line: 12,
        },
        source_heading: Some("Home".to_string()),
        source_mtime: ts("2026-09-19T10:00:00Z"),
        last_modified: ts("2026-09-19T10:00:00Z"),
    }
}

fn mutated(f: impl FnOnce(&mut Task)) -> Task {
    let mut task = sample_task();
    f(&mut task);
    task
}

#[test]
fn slug_from_display_names() {
    assert_eq!(slug_of("Home Lab"), "home-lab");
    assert_eq!(slug_of("HOME LAB"), "home-lab");
    assert_eq!(slug_of("home-lab"), "home-lab");
    assert_eq!(slug_of("  Home  Lab  "), "home-lab");
    assert_eq!(slug_of("Home & Lab"), "home-lab");
    assert_eq!(slug_of("A--B"), "a-b");
    assert_eq!(slug_of("project/Alpha 2"), "project-alpha-2");
    assert_eq!(slug_of("-wrap-"), "wrap");
    assert_eq!(slug_of("Lab 42"), "lab-42");
    assert_eq!(slug_of("héllo"), "h-llo");
}

#[test]
fn slug_case_insensitive_equivalence() {
    assert_eq!(slug_of("Home-Lab"), slug_of("home-lab"));
    assert_eq!(
        ListSlug::from_name("Home-Lab").unwrap(),
        ListSlug::from_name("home-lab").unwrap()
    );
}

#[test]
fn slug_rejects_empty_result() {
    for bad in ["", " ", "!!!", "---"] {
        assert!(
            matches!(
                ListSlug::from_name(bad),
                Err(RestaskError::Validation { field: "list", .. })
            ),
            "input {bad:?}"
        );
    }
}

#[test]
fn slug_display_name() {
    assert_eq!(
        ListSlug::from_name("home-lab").unwrap().display_name(),
        "Home Lab"
    );
    assert_eq!(
        ListSlug::from_name("server").unwrap().display_name(),
        "Server"
    );
    assert_eq!(
        ListSlug::from_name("home-lab-2").unwrap().display_name(),
        "Home Lab 2"
    );
}

#[test]
fn status_variants_compare() {
    let on = LocalDate::parse("2026-09-19").unwrap();
    assert_ne!(Status::Active, Status::Completed { on });
    assert_eq!(Status::Completed { on }, Status::Completed { on });
}

#[test]
fn thumbprint_stable_for_identical_tasks() {
    assert_eq!(sample_task().thumbprint(), sample_task().thumbprint());
}

#[test]
fn thumbprint_ignores_source_metadata() {
    let base = sample_task();
    let variants = [
        mutated(|t| t.source.line += 1),
        mutated(|t| t.source.path = "elsewhere.md".to_string()),
        mutated(|t| t.source_heading = None),
        mutated(|t| t.source_mtime = ts("2026-09-20T11:30:00Z")),
        mutated(|t| t.last_modified = ts("2026-09-21T08:00:00Z")),
    ];
    for variant in variants {
        assert_eq!(variant.thumbprint(), base.thumbprint());
    }
}

#[test]
fn thumbprint_changes_on_canonical_fields() {
    let base = sample_task();
    let variants = [
        (
            "uid",
            mutated(|t| t.uid = TaskUid::parse("restask-01arz3ndektsv4rrffq69g5faw").unwrap()),
        ),
        (
            "list",
            mutated(|t| t.list = ListSlug::from_name("Work").unwrap()),
        ),
        (
            "text",
            mutated(|t| t.text = "Fix the router now".to_string()),
        ),
        (
            "status",
            mutated(|t| {
                t.status = Status::Completed {
                    on: LocalDate::parse("2026-09-19").unwrap(),
                }
            }),
        ),
        ("priority", mutated(|t| t.priority = None)),
        (
            "due",
            mutated(|t| t.due = Some(When::parse_date_or_datetime("2026-09-20").unwrap())),
        ),
        (
            "start",
            mutated(|t| t.start = Some(When::parse_date_or_datetime("2026-09-18").unwrap())),
        ),
        (
            "scheduled",
            mutated(|t| t.scheduled = When::parse_date_or_datetime("2026-09-17").ok()),
        ),
        ("created", mutated(|t| t.created = None)),
        (
            "parent",
            mutated(|t| {
                t.parent = Some(TaskUid::parse("restask-01arz3ndektsv4rrffq69g5fav").unwrap())
            }),
        ),
    ];
    for (label, variant) in variants {
        assert_ne!(
            variant.thumbprint(),
            base.thumbprint(),
            "thumbprint ignores {label}"
        );
    }
}

#[test]
fn task_serde_roundtrip() {
    let task = sample_task();
    let json = serde_json::to_string(&task).unwrap();
    assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), task);
}
