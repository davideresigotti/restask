//! §6.3 line-mutation tests: Register/SetStatus/SetPriority/SetWhen/EditText, canonical
//! tail order, text and non-task line preservation.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use restask::config::VaultConfig;
use restask::domain::{Clock, LocalDate, LocalDateTime, Priority, TaskUid, When};
use restask::markdown::mutator::{apply, Mutation, MutationOutcome, SkipReason, WhenField};

const UID1: &str = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpb";
const UID2: &str = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpc";

struct FixedClock;

impl Clock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 22, 15, 30, 0).unwrap()
    }

    fn today_local(&self) -> LocalDate {
        LocalDate::parse("2026-09-22").unwrap()
    }

    fn local_offset(&self) -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }
}

fn uid(value: &str) -> TaskUid {
    TaskUid::parse(value).unwrap()
}

fn run(contents: &str, ops: &[Mutation]) -> MutationOutcome {
    apply(contents, ops, &VaultConfig::default(), &FixedClock).unwrap()
}

fn date(value: &str) -> LocalDate {
    LocalDate::parse(value).unwrap()
}

#[test]
fn register_appends_created_and_uid() {
    let out = run(
        "- [ ] Buy milk\n",
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: date("2026-09-22"),
        }],
    );
    assert_eq!(out.applied.len(), 1);
    assert!(out.skipped.is_empty());
    assert_eq!(
        out.contents,
        "- [ ] Buy milk ➕ 2026-09-22 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn register_preserves_indent_marker_and_metadata() {
    let out = run(
        "  * [ ] Existing ⏫ 📅 2026-10-01\n",
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: date("2026-09-22"),
        }],
    );
    assert_eq!(
        out.contents,
        "  * [ ] Existing ⏫ 📅 2026-10-01 ➕ 2026-09-22 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn register_is_idempotent_for_same_uid() {
    let contents = "- [ ] Buy milk ➕ 2026-09-22 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: date("2026-09-22"),
        }],
    );
    assert_eq!(out.applied.len(), 1);
    assert_eq!(out.contents, contents);
}

#[test]
fn register_skips_line_changed() {
    let out = run(
        "plain text line\n- [ ] Other 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        &[
            Mutation::Register {
                line_no: 1,
                uid: uid(UID1),
                created: date("2026-09-22"),
            },
            Mutation::Register {
                line_no: 2,
                uid: uid(UID1),
                created: date("2026-09-22"),
            },
            Mutation::Register {
                line_no: 99,
                uid: uid(UID1),
                created: date("2026-09-22"),
            },
        ],
    );
    assert!(out.applied.is_empty());
    assert_eq!(out.skipped.len(), 3);
    assert!(out
        .skipped
        .iter()
        .all(|(_, reason)| *reason == SkipReason::LineChanged));
    assert_eq!(
        out.contents,
        "plain text line\n- [ ] Other 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpc\n"
    );
}

#[test]
fn set_status_complete_and_uncomplete() {
    let contents = "- [ ] Buy milk ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::SetStatus {
            uid: uid(UID1),
            checked: true,
            completed_on: Some(date("2026-09-21")),
        }],
    );
    assert_eq!(
        out.contents,
        "- [x] Buy milk ✅ 2026-09-21 ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );

    let out = run(
        &out.contents,
        &[Mutation::SetStatus {
            uid: uid(UID1),
            checked: false,
            completed_on: None,
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn set_status_none_stamps_clock_today() {
    let out = run(
        "- [ ] Buy milk 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetStatus {
            uid: uid(UID1),
            checked: true,
            completed_on: None,
        }],
    );
    assert_eq!(
        out.contents,
        "- [x] Buy milk ✅ 2026-09-22 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn set_priority_add_change_remove() {
    let contents = "- [ ] Buy milk 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::High),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk ⏫ 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );

    let out = run(
        &out.contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Lowest),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk ⏬ 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );

    let out = run(
        &out.contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: None,
        }],
    );
    assert_eq!(out.contents, contents);
}

#[test]
fn set_when_date_datetime_and_remove() {
    let contents = "- [ ] Buy milk 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[
            Mutation::SetWhen {
                uid: uid(UID1),
                field: WhenField::Due,
                value: Some(When::Date(date("2026-10-01"))),
            },
            Mutation::SetWhen {
                uid: uid(UID1),
                field: WhenField::Start,
                value: Some(When::DateTime(
                    LocalDateTime::parse("2026-09-30 08:15").unwrap(),
                )),
            },
            Mutation::SetWhen {
                uid: uid(UID1),
                field: WhenField::Scheduled,
                value: Some(When::Date(date("2026-09-29"))),
            },
        ],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk 🛫 2026-09-30 08:15 ⏳ 2026-09-29 📅 2026-10-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );

    let out = run(
        &out.contents,
        &[Mutation::SetWhen {
            uid: uid(UID1),
            field: WhenField::Due,
            value: None,
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk 🛫 2026-09-30 08:15 ⏳ 2026-09-29 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn edit_text_preserves_metadata() {
    let contents = "- [ ] Buy milk 🔼 ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::EditText {
            uid: uid(UID1),
            text: "Buy oat milk and bread".to_string(),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy oat milk and bread 🔼 ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn canonical_tail_order_full_rewrite() {
    let contents = "- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb ➕ 2026-09-01 ✅ 2026-09-20 📅 2026-09-03 17:30 ⏳ 2026-09-02 🛫 2026-09-01 ⏬\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Medium),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔼 🛫 2026-09-01 ⏳ 2026-09-02 📅 2026-09-03 17:30 ✅ 2026-09-20 ➕ 2026-09-01 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn non_targeted_lines_and_text_preserved() {
    let contents = concat!(
        "# Title\n",
        "\n",
        "- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        "plain line\n",
        "  - [x] B ✅ 2026-09-19 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
    );
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID2),
            priority: None,
        }],
    );
    assert_eq!(out.contents, contents);
}

#[test]
fn crlf_line_endings_preserved_on_rewrite() {
    let contents = "- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\r\nplain\r\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::High),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A ⏫ 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\r\nplain\r\n"
    );
}

#[test]
fn uid_not_found_is_skipped() {
    let out = run(
        "- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[
            Mutation::SetStatus {
                uid: uid(UID2),
                checked: true,
                completed_on: None,
            },
            Mutation::SetPriority {
                uid: uid(UID2),
                priority: None,
            },
            Mutation::SetWhen {
                uid: uid(UID2),
                field: WhenField::Due,
                value: None,
            },
            Mutation::EditText {
                uid: uid(UID2),
                text: "x".to_string(),
            },
        ],
    );
    assert!(out.applied.is_empty());
    assert_eq!(out.skipped.len(), 4);
    assert!(out
        .skipped
        .iter()
        .all(|(_, reason)| *reason == SkipReason::UidNotFound));
}

#[test]
fn invalid_token_value_dropped_on_rewrite() {
    let out = run(
        "- [ ] A 📅 2026-13-45 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Low),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔽 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn multiple_ops_on_same_uid_apply_sequentially() {
    let out = run(
        "- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[
            Mutation::SetStatus {
                uid: uid(UID1),
                checked: true,
                completed_on: Some(date("2026-09-21")),
            },
            Mutation::SetPriority {
                uid: uid(UID1),
                priority: Some(Priority::Highest),
            },
        ],
    );
    assert_eq!(out.applied.len(), 2);
    assert_eq!(
        out.contents,
        "- [x] A 🔺 ✅ 2026-09-21 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}
