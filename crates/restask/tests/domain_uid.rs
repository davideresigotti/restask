//! Integration tests for `domain::uid` (§3.1): mint / parse / format / order / invalid.

use restask::domain::{Counter, DeviceTag, TaskUid, UidError};

const VALID_BODY: &str = "01jzabcdefghjkmnpqrstvwxyz";

fn tag(letters: &str) -> DeviceTag {
    DeviceTag::parse(letters).unwrap()
}

fn counted(letters: &str, number: u64) -> TaskUid {
    TaskUid::minted(&tag(letters), number)
}

#[test]
fn a_minted_uid_is_the_device_tag_and_a_number() {
    let uid = counted("a", 42);
    assert_eq!(uid.as_str(), "restask-a42");
    // A line shows the short form; a file name and the server get the full one.
    assert_eq!(uid.token(), "a42");
    assert_eq!(uid.to_string(), "restask-a42");
    assert_eq!(uid.minted_by(), Some(("a", 42)));
    assert!(!uid.is_long());
}

#[test]
fn a_counter_counts_on_from_what_it_has_seen() {
    let ids = Counter::new(tag("b"), 3);
    assert_eq!(ids.mint().token(), "b4");
    // A UID of this device that is on a line already is never handed out again.
    ids.observe(&counted("b", 9));
    // Another device's numbers, and long UIDs, say nothing about this one's.
    ids.observe(&counted("c", 500));
    ids.observe(&counted("bb", 500));
    ids.observe(&TaskUid::parse(&format!("restask-{VALID_BODY}")).unwrap());
    assert_eq!(ids.mint().token(), "b10");
    assert_eq!(ids.mint().token(), "b11");
    assert_eq!(ids.last(), 11);
    assert_eq!(ids.tag(), &tag("b"));
    // A copy counts on its own.
    let copy = ids.clone();
    assert_eq!(copy.mint().token(), "b12");
    assert_eq!(ids.last(), 11);
}

#[test]
fn device_tags_are_one_to_four_letters() {
    for good in ["a", "z", "ab", "abcd"] {
        assert_eq!(tag(good).as_str(), good);
    }
    for bad in ["", "abcde", "A", "a1", "a-b", "é"] {
        assert!(DeviceTag::parse(bad).is_err(), "{bad:?} is no tag");
    }
    let single = DeviceTag::all(1);
    assert_eq!(single.len(), 26);
    assert_eq!(single[0].as_str(), "a");
    assert_eq!(single[25].as_str(), "z");
    let double = DeviceTag::all(2);
    assert_eq!(double.len(), 676);
    assert_eq!(double[0].as_str(), "aa");
    assert_eq!(double[675].as_str(), "zz");
}

#[test]
fn parse_round_trip_and_display() {
    for uid in [
        counted("a", 1),
        counted("zz", 123_456),
        TaskUid::parse(&format!("restask-{VALID_BODY}")).unwrap(),
    ] {
        let reparsed = TaskUid::parse(uid.as_str()).unwrap();
        assert_eq!(reparsed, uid);
        assert_eq!(reparsed.to_string(), uid.as_str());
        // What a line says behind `🆔` reads back as the same UID.
        assert_eq!(TaskUid::from_token(uid.token()).unwrap(), uid);
    }
}

#[test]
fn a_line_spells_a_counted_uid_without_the_prefix_and_a_long_one_in_full() {
    assert_eq!(TaskUid::from_token("a42").unwrap().as_str(), "restask-a42");
    let long = format!("restask-{VALID_BODY}");
    assert_eq!(TaskUid::from_token(&long).unwrap().as_str(), long);
    assert_eq!(TaskUid::from_token(&long).unwrap().token(), long);
    for bad in [
        "restask-a42",
        "a042",
        "a0",
        "a",
        "42",
        "abcde1",
        "a42b",
        VALID_BODY,
    ] {
        assert!(TaskUid::from_token(bad).is_err(), "{bad:?} is no token");
    }
    // In a `VTODO` and in a file name a UID is restask's only with the prefix: `a42`
    // alone is whatever another client called its task.
    assert!(TaskUid::parse("a42").is_err());
}

#[test]
fn uids_sort_in_creation_order_as_far_as_they_tell_it() {
    let long_early = TaskUid::parse("restask-01jz0000000000000000000000").unwrap();
    let long_late = TaskUid::parse("restask-01jzzzzzzzzzzzzzzzzzzzzzzz").unwrap();
    let mut uids = vec![
        counted("a", 10),
        counted("b", 2),
        long_late.clone(),
        counted("a", 2),
        counted("a", 9),
        long_early.clone(),
    ];
    uids.sort();
    // The long ones first (they are the older tasks), by their timestamp; the counted
    // ones by number — 9 before 10 — then by tag.
    assert_eq!(
        uids,
        vec![
            long_early,
            long_late,
            counted("a", 2),
            counted("b", 2),
            counted("a", 9),
            counted("a", 10),
        ]
    );
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
        "a42",
        "restask-a042",
        "restask-a0",
        "restask-abcde1",
        "restask-a1234567890123456",
        "taskres-a42",
    ];
    for bad in invalid {
        let err: UidError = TaskUid::parse(bad).unwrap_err();
        assert_eq!(err.0, bad, "error must carry the raw input");
        assert!(err.to_string().starts_with("invalid task UID: "));
    }
}

#[test]
fn serde_round_trip() {
    let uid = counted("a", 42);
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
    assert!(uid.is_long());
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
    // A counted UID tells no day.
    assert_eq!(counted("a", 42).created_on(), None);
    // A task adopted before the counters: the foreign task's own creation instant, late in the UTC day.
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
    // A UID minted for a line of the vault adopts nothing; a counted one never does —
    // its link is bound by `X-RESTASK-OF` (§8.1).
    assert!(!counted("a", 42).adopts("5417861935824551742"));
    assert!(!TaskUid::parse("taskres-01jzetq1v2h3k4m5n6p7r8t9w0")
        .unwrap()
        .adopts("5417861935824551742"));
}
