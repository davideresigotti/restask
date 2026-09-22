//! Integration tests for `domain::uid` (§3.1): generate / parse / format / invalid.

use restask::domain::{TaskUid, UidError};

const VALID_BODY: &str = "01jzabcdefghjkmnpqrstvwxyz";

#[test]
fn generated_uid_has_canonical_shape() {
    let uid = TaskUid::generate();
    let s = uid.as_str();
    assert!(s.starts_with("taskres-"), "missing prefix: {s}");
    let body = &s["taskres-".len()..];
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
    let uid = TaskUid::parse(format!("  TASKRES-{VALID_BODY}  ").as_str()).unwrap();
    assert_eq!(uid.as_str(), format!("taskres-{VALID_BODY}"));
}

#[test]
fn parse_rejects_invalid() {
    let invalid = [
        "",
        "   ",
        VALID_BODY,
        &format!("other-{VALID_BODY}"),
        &format!("taskres-{}", &VALID_BODY[..25]),
        &format!("taskres-{VALID_BODY}0"),
        &format!("taskres-{}i", &VALID_BODY[..25]),
        &format!("taskres-{}u", &VALID_BODY[..25]),
        &format!("taskres-{}-", &VALID_BODY[..25]),
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
