//! Integration tests for `domain::uid` (§3.1): generate / parse / format / invalid.

use restask::domain::{TaskUid, UidError};

const VALID_BODY: &str = "01jzabcdefghjkmnpqrstvwxyz";

#[test]
fn generated_uid_has_canonical_shape() {
    let uid = TaskUid::generate();
    let s = uid.as_str();
    assert!(s.starts_with("restask-"), "missing prefix: {s}");
    let body = &s["restask-".len()..];
    assert_eq!(body.len(), 26, "body length: {s}");
    assert!(
        body.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "non-lowercase Crockford body: {s}"
    );
    assert!(!body.contains(['i', 'l', 'o', 'u']), "excluded letter: {s}");
}

#[test]
fn generate_is_monotonic_in_process() {
    let first = TaskUid::generate();
    let second = TaskUid::generate();
    assert!(first.as_str() < second.as_str());
}

#[test]
fn parse_round_trip_and_display() {
    let uid = TaskUid::generate();
    let reparsed = TaskUid::parse(uid.as_str()).unwrap();
    assert_eq!(reparsed, uid);
    assert_eq!(reparsed.to_string(), uid.as_str());
    assert_eq!(reparsed.as_str(), uid.as_str());
}

#[test]
fn parse_trims_and_lowercases() {
    let uid = TaskUid::parse(format!("  RESTASK-{VALID_BODY}  ").as_str()).unwrap();
    assert_eq!(uid.as_str(), format!("restask-{VALID_BODY}"));
}

#[test]
fn parse_rejects_invalid() {
    let invalid = [
        "",
        "   ",
        VALID_BODY,
        &format!("other-{VALID_BODY}"),
        &format!("restask-{}", &VALID_BODY[..25]),
        &format!("restask-{VALID_BODY}0"),
        &format!("restask-{}i", &VALID_BODY[..25]),
        &format!("restask-{}u", &VALID_BODY[..25]),
        &format!("restask-{}-", &VALID_BODY[..25]),
    ];
    for bad in invalid {
        let err: UidError = TaskUid::parse(bad).unwrap_err();
        assert_eq!(err.0, bad, "error must carry the raw input");
        assert!(err.to_string().starts_with("invalid task UID: "));
    }
}

#[test]
fn serde_round_trip() {
    let uid = TaskUid::generate();
    let json = serde_json::to_string(&uid).unwrap();
    assert_eq!(json, format!("\"{}\"", uid.as_str()));
    let back: TaskUid = serde_json::from_str(&json).unwrap();
    assert_eq!(back, uid);
}

#[test]
fn legacy_prefix_parses_and_is_kept_verbatim() {
    let legacy = format!("taskres-{VALID_BODY}");
    let uid = TaskUid::parse(&legacy).unwrap();
    assert_eq!(
        uid.as_str(),
        legacy,
        "legacy UIDs are eternal, never rewritten"
    );
    assert!(TaskUid::generate().as_str().starts_with("restask-"));
}

#[test]
fn derived_uids_are_deterministic_valid_and_time_ordered() {
    use chrono::{TimeZone, Utc};
    let early = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let late = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let a = TaskUid::derived("abc@tasks.org", Some(early));
    assert_eq!(a, TaskUid::derived("abc@tasks.org", Some(early)));
    assert_eq!(TaskUid::parse(a.as_str()).unwrap(), a, "a well-formed UID");
    assert_ne!(a, TaskUid::derived("abd@tasks.org", Some(early)));
    // The creation instant leads the ULID, so adopted tasks sort by creation.
    assert!(a < TaskUid::derived("abc@tasks.org", Some(late)));
    assert!(TaskUid::derived("zzz", Some(early)) < TaskUid::derived("aaa", Some(late)));
    // No creation instant: still deterministic (epoch-stamped).
    assert_eq!(TaskUid::derived("x", None), TaskUid::derived("x", None));
    assert!(TaskUid::derived("x", None) < a);
}

#[test]
fn a_uid_tells_the_day_it_was_minted() {
    use chrono::TimeZone;
    // A generated UID: today, by the UTC calendar.
    assert_eq!(
        TaskUid::generate().created_on().map(|day| day.0),
        Some(chrono::Utc::now().date_naive())
    );
    // An adopted task: the foreign task's own creation instant, late in the UTC day.
    let at = chrono::Utc
        .with_ymd_and_hms(2026, 9, 21, 23, 59, 59)
        .unwrap();
    assert_eq!(
        TaskUid::derived("5417861935824551742", Some(at))
            .created_on()
            .map(|day| day.format()),
        Some("2026-09-21".to_string())
    );
    // Legacy prefix reads the same; an unknown creation carries no day.
    assert_eq!(
        TaskUid::parse("taskres-01jzq4tsvg2c9xkw7n5m8rhdpb")
            .unwrap()
            .created_on(),
        TaskUid::parse("restask-01jzq4tsvg2c9xkw7n5m8rhdpb")
            .unwrap()
            .created_on()
    );
    assert_eq!(TaskUid::derived("no-date", None).created_on(), None);
}

#[test]
fn a_uid_tells_which_foreign_uid_it_was_derived_from() {
    use chrono::{TimeZone, Utc};
    let at = Utc.with_ymd_and_hms(2026, 9, 21, 8, 12, 33).unwrap();
    // Whatever the creation instant: the link holds when a client rewrites `CREATED`.
    for uid in [
        TaskUid::derived("5417861935824551742", Some(at)),
        TaskUid::derived("5417861935824551742", None),
    ] {
        assert!(uid.adopts("5417861935824551742"));
        assert!(!uid.adopts("5417861935824551743"));
        assert!(!uid.adopts(""));
    }
    // A UID minted for a line of the vault adopts nothing.
    assert!(!TaskUid::generate().adopts("5417861935824551742"));
    assert!(!TaskUid::parse("taskres-01jzetq1v2h3k4m5n6p7r8t9w0")
        .unwrap()
        .adopts("5417861935824551742"));
}
