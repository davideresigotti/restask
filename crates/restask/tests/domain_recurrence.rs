//! Recurrence (§3.6, §11.6): the two spellings of a repeat rule — the vault's
//! `🔁 every …` text and iCalendar's `RRULE` — and the occurrence that follows a
//! completed one.

use restask::domain::{LocalDate, Recurrence, When};
use restask::vtodo::recurrence::{consume_count, find_in_extras};

fn rule(value: &str) -> Recurrence {
    Recurrence::from_rrule(value)
        .unwrap_or_else(|| panic!("no rule in {value}"))
        .0
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

/// The rule written in the vault as `text`; the whole text must be the rule.
fn spoken(text: &str) -> Recurrence {
    let (rule, len) = Recurrence::from_text(text).unwrap_or_else(|| panic!("not a rule: {text}"));
    assert_eq!(len, text.len(), "trailing text in `{text}`");
    rule
}

// ── the two spellings ─────────────────────────────────────────────────────────────────

#[test]
fn every_vault_spelling_maps_to_its_rrule_and_back() {
    for (text, rrule) in [
        ("every day", "FREQ=DAILY"),
        ("every 3 days", "FREQ=DAILY;INTERVAL=3"),
        ("every week", "FREQ=WEEKLY"),
        ("every 2 weeks", "FREQ=WEEKLY;INTERVAL=2"),
        ("every week on Monday, Thursday", "FREQ=WEEKLY;BYDAY=MO,TH"),
        ("every weekday", "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"),
        ("every month", "FREQ=MONTHLY"),
        ("every month on the 15th", "FREQ=MONTHLY;BYMONTHDAY=15"),
        (
            "every month on the 1st, 22nd, last day",
            "FREQ=MONTHLY;BYMONTHDAY=1,22,-1",
        ),
        (
            "every 2 months on the 2nd Tuesday",
            "FREQ=MONTHLY;INTERVAL=2;BYDAY=2TU",
        ),
        ("every month on the last Friday", "FREQ=MONTHLY;BYDAY=-1FR"),
        ("every year", "FREQ=YEARLY"),
        ("every 6 hours", "FREQ=HOURLY;INTERVAL=6"),
        ("every week for 5 times", "FREQ=WEEKLY;COUNT=5"),
        ("every day for 1 time", "FREQ=DAILY;COUNT=1"),
        (
            "every month until 2026-12-31",
            "FREQ=MONTHLY;UNTIL=20261231",
        ),
        (
            "every 2 weeks on Saturday, Sunday for 10 times until 2027-01-01",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SA,SU;COUNT=10;UNTIL=20270101",
        ),
    ] {
        let from_text = spoken(text);
        assert_eq!(from_text.to_rrule(), rrule, "{text}");
        assert_eq!(from_text.to_text(), text, "canonical spelling of {rrule}");
        let (from_rrule, exact) = Recurrence::from_rrule(rrule).unwrap();
        assert!(exact, "{rrule}");
        assert_eq!(from_rrule, from_text, "{text} ⇄ {rrule}");
    }
}

#[test]
fn the_vault_spelling_is_read_leniently_and_written_canonically() {
    for (written, canonical) in [
        (
            "Every Week on mon and thu",
            "every week on Monday, Thursday",
        ),
        ("every 1 day", "every day"),
        (
            "every week on Thursday, Monday, Thursday",
            "every week on Monday, Thursday",
        ),
        ("every month on the 15", "every month on the 15th"),
        (
            "every month on 1st and the 15th",
            "every month on the 1st, 15th",
        ),
        ("every weekdays", "every weekday"),
        (
            "every week on Monday, Tuesday, Wednesday, Thursday, Friday",
            "every weekday",
        ),
    ] {
        assert_eq!(spoken(written).to_text(), canonical, "{written}");
    }
}

#[test]
fn a_rule_ends_where_the_task_text_resumes() {
    let cases = [
        ("every week buy milk", "every week"),
        ("every week on Monday call mom", "every week on Monday"),
        (
            "every month on the 15th pay rent",
            "every month on the 15th",
        ),
        ("every day on time", "every day"),
        ("every 2 weeks for good", "every 2 weeks"),
        ("every month until further notice", "every month"),
    ];
    for (text, rule_part) in cases {
        let (rule, len) = Recurrence::from_text(text).unwrap();
        assert_eq!(&text[..len], rule_part, "{text}");
        assert_eq!(rule, spoken(rule_part));
    }
    for not_a_rule in [
        "",
        "every",
        "every now and then",
        "every 0 days",
        "daily",
        "each day",
    ] {
        assert!(Recurrence::from_text(not_a_rule).is_none(), "{not_a_rule}");
    }
}

#[test]
fn rules_the_vault_cannot_spell_are_inexact_but_still_usable() {
    for rrule in [
        "FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR",
        "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR",
        "FREQ=MONTHLY;BYDAY=MO",
        "FREQ=MONTHLY;BYMONTHDAY=-2",
        "FREQ=YEARLY;BYMONTH=3",
        "FREQ=WEEKLY;WKST=SU;INTERVAL=2;BYDAY=MO",
        "FREQ=WEEKLY;BYDAY=XX",
    ] {
        let (_, exact) = Recurrence::from_rrule(rrule).unwrap();
        assert!(!exact, "{rrule}");
    }
    // Other clients' formatting of an expressible rule is still exact.
    for rrule in [
        "FREQ=WEEKLY;INTERVAL=1;BYDAY=MO;WKST=MO",
        "freq=daily;interval=2",
        "FREQ=DAILY;UNTIL=20261231T225959Z",
    ] {
        assert!(Recurrence::from_rrule(rrule).unwrap().1, "{rrule}");
    }
    assert!(Recurrence::from_rrule("INTERVAL=2").is_none());
    assert!(Recurrence::from_rrule("FREQ=SOMETIMES").is_none());
    // Working days as a daily rule: occurrences skip the weekend (2026-09-25 is a Friday).
    assert_eq!(
        next(
            "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR",
            "2026-09-25",
            "2026-09-25"
        )
        .unwrap(),
        "2026-09-28"
    );
}

// ── rules that stay on the server ─────────────────────────────────────────────────────

#[test]
fn an_unmanaged_rule_is_found_among_the_extras_and_handed_back() {
    let extras: Vec<String> = [
        "DESCRIPTION:x",
        "BEGIN:VALARM",
        "RRULE:FREQ=DAILY",
        "END:VALARM",
        "rrule:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR;COUNT=3",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect();
    let (index, found) = find_in_extras(&extras).unwrap();
    assert_eq!(index, 4, "the alarm's line is not the task's rule");
    assert!(!found.is_last());
    assert!(find_in_extras(&extras[..4]).is_none());
    // Handed back exactly as written, one occurrence fewer.
    assert_eq!(
        consume_count(&extras[4]),
        "rrule:FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU,WE,TH,FR;COUNT=2"
    );
    assert_eq!(consume_count("RRULE:FREQ=DAILY"), "RRULE:FREQ=DAILY");
}

// ── occurrences ───────────────────────────────────────────────────────────────────────

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
    assert_eq!(three.consumed().to_rrule(), "FREQ=WEEKLY;BYDAY=MO;COUNT=2");
    assert!(rule("FREQ=WEEKLY;COUNT=1").is_last());
    // Without a COUNT nothing changes.
    assert_eq!(
        rule("FREQ=MONTHLY;BYDAY=-1FR").consumed(),
        rule("FREQ=MONTHLY;BYDAY=-1FR")
    );
}
