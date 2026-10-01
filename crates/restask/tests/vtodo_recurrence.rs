//! Recurrence rules (§11.6): finding a task's `RRULE` among its extras and computing the
//! occurrence that follows a completed one.

use restask::domain::{LocalDate, When};
use restask::vtodo::Recurrence;

fn rule(value: &str) -> Recurrence {
    Recurrence::find(&[format!("RRULE:{value}")]).unwrap_or_else(|| panic!("no rule in {value}"))
}

fn when(value: &str) -> When {
    When::parse_date_or_datetime(value).unwrap()
}

fn date(value: &str) -> LocalDate {
    LocalDate::parse(value).unwrap()
}

/// Next occurrence after `anchor`, completed on `done`.
fn next(value: &str, anchor: &str, done: &str) -> Option<String> {
    rule(value)
        .next_after(when(anchor), date(done))
        .map(|next| match next {
            When::Date(day) => day.format(),
            When::DateTime(at) => at.format(),
        })
}

#[test]
fn finds_the_tasks_own_rule_only() {
    let extras: Vec<String> = [
        "DESCRIPTION:x",
        "BEGIN:VALARM",
        "RRULE:FREQ=DAILY",
        "END:VALARM",
        "rrule:FREQ=WEEKLY;INTERVAL=2",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect();
    let found = Recurrence::find(&extras).unwrap();
    assert_eq!(found.index, 4, "the alarm's line is not the task's rule");
    assert!(Recurrence::find(&extras[..4]).is_none());
    assert!(Recurrence::find(&["RRULE:INTERVAL=2".to_string()]).is_none());
    assert!(Recurrence::find(&["RRULE:FREQ=SOMETIMES".to_string()]).is_none());
}

#[test]
fn daily_and_intervals() {
    assert_eq!(
        next("FREQ=DAILY", "2026-09-19", "2026-09-19").unwrap(),
        "2026-09-20"
    );
    assert_eq!(
        next("FREQ=DAILY;INTERVAL=3", "2026-09-19", "2026-09-19").unwrap(),
        "2026-09-22"
    );
    // Completed early: still the occurrence after the one that was due.
    assert_eq!(
        next("FREQ=DAILY", "2026-09-19", "2026-09-17").unwrap(),
        "2026-09-20"
    );
    // Completed long overdue: the next upcoming occurrence, on the rule's own grid.
    assert_eq!(
        next("FREQ=DAILY;INTERVAL=3", "2026-09-01", "2026-09-19").unwrap(),
        "2026-09-22"
    );
}

#[test]
fn weekly_plain_and_by_day() {
    // 2026-09-21 is a Monday.
    assert_eq!(
        next("FREQ=WEEKLY", "2026-09-21", "2026-09-21").unwrap(),
        "2026-09-28"
    );
    assert_eq!(
        next("FREQ=WEEKLY;INTERVAL=2", "2026-09-21", "2026-09-30").unwrap(),
        "2026-10-05"
    );
    assert_eq!(
        next("FREQ=WEEKLY;BYDAY=MO,TH", "2026-09-21", "2026-09-21").unwrap(),
        "2026-09-24"
    );
    assert_eq!(
        next("FREQ=WEEKLY;BYDAY=MO,TH", "2026-09-24", "2026-09-24").unwrap(),
        "2026-09-28"
    );
    // Every second week: Thursday of the anchor's week, then the week after next.
    assert_eq!(
        next(
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH",
            "2026-09-24",
            "2026-09-24"
        )
        .unwrap(),
        "2026-10-05"
    );
}

#[test]
fn monthly_same_day_skips_months_that_are_too_short() {
    assert_eq!(
        next("FREQ=MONTHLY", "2026-09-19", "2026-09-19").unwrap(),
        "2026-10-19"
    );
    assert_eq!(
        next("FREQ=MONTHLY", "2026-01-31", "2026-01-31").unwrap(),
        "2026-03-31"
    );
    assert_eq!(
        next("FREQ=MONTHLY;INTERVAL=3", "2026-11-15", "2026-11-15").unwrap(),
        "2027-02-15"
    );
}

#[test]
fn monthly_by_month_day_and_by_weekday() {
    assert_eq!(
        next("FREQ=MONTHLY;BYMONTHDAY=1,15", "2026-09-01", "2026-09-01").unwrap(),
        "2026-09-15"
    );
    assert_eq!(
        next("FREQ=MONTHLY;BYMONTHDAY=-1", "2026-01-31", "2026-01-31").unwrap(),
        "2026-02-28"
    );
    // Second Tuesday: 2026-09-08, then 2026-10-13.
    assert_eq!(
        next("FREQ=MONTHLY;BYDAY=2TU", "2026-09-08", "2026-09-08").unwrap(),
        "2026-10-13"
    );
    // Last Friday: 2026-09-25, then 2026-10-30.
    assert_eq!(
        next("FREQ=MONTHLY;BYDAY=-1FR", "2026-09-25", "2026-09-25").unwrap(),
        "2026-10-30"
    );
}

#[test]
fn yearly_including_leap_day() {
    assert_eq!(
        next("FREQ=YEARLY", "2026-09-19", "2026-09-19").unwrap(),
        "2027-09-19"
    );
    assert_eq!(
        next("FREQ=YEARLY", "2024-02-29", "2024-02-29").unwrap(),
        "2028-02-29"
    );
}

#[test]
fn a_timed_task_keeps_its_time_of_day() {
    assert_eq!(
        next("FREQ=WEEKLY", "2026-09-21 17:30", "2026-09-21").unwrap(),
        "2026-09-28 17:30"
    );
    assert_eq!(
        next("FREQ=HOURLY;INTERVAL=6", "2026-09-21 08:00", "2026-09-21").unwrap(),
        "2026-09-21 14:00"
    );
    // An hourly rule on an all-day task repeats the next day at the earliest.
    assert_eq!(
        next("FREQ=HOURLY", "2026-09-21", "2026-09-21").unwrap(),
        "2026-09-22"
    );
}

#[test]
fn until_ends_the_series_and_count_is_consumed() {
    assert_eq!(
        next(
            "FREQ=DAILY;UNTIL=20260920T000000Z",
            "2026-09-19",
            "2026-09-19"
        )
        .unwrap(),
        "2026-09-20"
    );
    assert_eq!(
        next(
            "FREQ=DAILY;UNTIL=20260920T000000Z",
            "2026-09-20",
            "2026-09-20"
        ),
        None
    );

    let three = rule("FREQ=WEEKLY;COUNT=3;BYDAY=MO");
    assert!(!three.is_last());
    assert_eq!(
        three.after_one_occurrence(),
        "RRULE:FREQ=WEEKLY;COUNT=2;BYDAY=MO"
    );
    assert!(rule("FREQ=WEEKLY;COUNT=1").is_last());
    // Without a COUNT the rule is handed back as written.
    assert_eq!(
        rule("FREQ=MONTHLY;BYDAY=-1FR;WKST=SU").after_one_occurrence(),
        "RRULE:FREQ=MONTHLY;BYDAY=-1FR;WKST=SU"
    );
}
