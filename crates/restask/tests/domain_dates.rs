//! T04 isolated suite: date/datetime parse & format, iCalendar mapping, `Clock` smoke (§3.3).

use restask::domain::dates::{Clock, DateError, LocalDate, LocalDateTime, SystemClock, When};

fn date(s: &str) -> LocalDate {
    LocalDate::parse(s).unwrap()
}

fn datetime(s: &str) -> LocalDateTime {
    LocalDateTime::parse(s).unwrap()
}

fn when(s: &str) -> When {
    When::parse_date_or_datetime(s).unwrap()
}

#[test]
fn date_roundtrip() {
    assert_eq!(date("2026-09-19").format(), "2026-09-19");
    assert_eq!(date("2026-01-02").format(), "2026-01-02");
    assert_eq!(date("1999-12-31").format(), "1999-12-31");
}

#[test]
fn date_rejects_invalid_format() {
    for bad in [
        "",
        "20260919",
        "2026/09/19",
        "2026-9-19",
        "2026-09-1",
        "2026-09-190",
        "2026-09-19 ",
        "xxxx-09-19",
        "2026-0x-19",
        "2026-09-1x",
        "2026-09-19T17:00",
        "202é-09-1",
        "+2026-09-19",
        "-2026-09-19",
    ] {
        assert_eq!(
            LocalDate::parse(bad),
            Err(DateError::InvalidFormat(bad.to_string())),
            "input {bad:?}"
        );
    }
}

#[test]
fn date_rejects_out_of_range() {
    for bad in ["2026-02-30", "2026-13-01", "2026-00-10", "2026-04-31"] {
        assert_eq!(
            LocalDate::parse(bad),
            Err(DateError::OutOfRange(bad.to_string())),
            "input {bad:?}"
        );
    }
}

#[test]
fn datetime_roundtrip() {
    assert_eq!(datetime("2026-09-19 17:00").format(), "2026-09-19 17:00");
    assert_eq!(datetime("2026-12-31 23:59").format(), "2026-12-31 23:59");
    assert_eq!(datetime("2026-01-01 00:00").format(), "2026-01-01 00:00");
}

#[test]
fn datetime_rejects_invalid_format() {
    for bad in [
        "2026-09-19T17:00",
        "2026-09-19  17:00",
        "2026-09-19 17:0",
        "2026-09-19 17:000",
        "2026-09-19 1700",
        "2026-09-19 17-00",
        "2026-09-19",
        "2026-09-19 17:00:00",
    ] {
        assert_eq!(
            LocalDateTime::parse(bad),
            Err(DateError::InvalidFormat(bad.to_string())),
            "input {bad:?}"
        );
    }
}

#[test]
fn datetime_rejects_out_of_range() {
    for bad in [
        "2026-09-19 24:00",
        "2026-09-19 17:60",
        "2026-13-01 10:00",
        "2026-02-30 08:15",
    ] {
        assert_eq!(
            LocalDateTime::parse(bad),
            Err(DateError::OutOfRange(bad.to_string())),
            "input {bad:?}"
        );
    }
}

#[test]
fn when_parses_date_or_datetime() {
    assert_eq!(when("2026-09-19"), When::Date(date("2026-09-19")));
    assert_eq!(
        when("2026-09-19 17:00"),
        When::DateTime(datetime("2026-09-19 17:00"))
    );
    assert!(When::parse_date_or_datetime("2026-9-19").is_err());
    assert!(When::parse_date_or_datetime("2026-09-19T17:00").is_err());
    assert!(When::parse_date_or_datetime("").is_err());
}

#[test]
fn when_to_ical_is_floating_never_z() {
    assert_eq!(When::Date(date("2026-09-19")).to_ical(), "20260919");
    assert_eq!(
        When::DateTime(datetime("2026-09-19 17:00")).to_ical(),
        "20260919T170000"
    );
    assert!(!When::DateTime(datetime("2026-09-19 17:00"))
        .to_ical()
        .contains('Z'));
}

#[test]
fn when_from_ical_roundtrip() {
    assert_eq!(
        When::from_ical("20260919"),
        Ok(When::Date(date("2026-09-19")))
    );
    assert_eq!(
        When::from_ical("20260919T170000"),
        Ok(When::DateTime(datetime("2026-09-19 17:00")))
    );
    for w in [when("2026-09-19"), when("2026-09-19 17:00")] {
        assert_eq!(When::from_ical(&w.to_ical()), Ok(w));
    }
}

#[test]
fn when_from_ical_rejects_other_shapes() {
    for bad in [
        "2026-09-19",
        "20260919T1700",
        "20260919t170000",
        "20260919T170000Z",
        "260919",
        "",
    ] {
        assert_eq!(
            When::from_ical(bad),
            Err(DateError::InvalidFormat(bad.to_string())),
            "input {bad:?}"
        );
    }
    assert_eq!(
        When::from_ical("20261319"),
        Err(DateError::OutOfRange("20261319".to_string()))
    );
    assert_eq!(
        When::from_ical("20261319T250000"),
        Err(DateError::OutOfRange("20261319T250000".to_string()))
    );
}

#[test]
fn is_date_only() {
    assert!(When::Date(date("2026-09-19")).is_date_only());
    assert!(!When::DateTime(datetime("2026-09-19 17:00")).is_date_only());
}

#[test]
fn when_serde_roundtrip() {
    for w in [when("2026-09-19"), when("2026-09-19 17:00")] {
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<When>(&json).unwrap(), w);
    }
}

#[test]
fn system_clock_smoke() {
    let clock = SystemClock;
    let anchor = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(clock.now_utc() > anchor);
    let today = clock.today_local();
    assert_eq!(LocalDate::parse(&today.format()), Ok(today));
    let _offset: chrono::FixedOffset = clock.local_offset();
}
