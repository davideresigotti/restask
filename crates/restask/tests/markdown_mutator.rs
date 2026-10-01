//! §6.3 line-mutation tests: Register/SetStatus/SetPriority/SetWhen/EditText, canonical
//! tail order, text and non-task line preservation.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use restask::config::VaultConfig;
use restask::domain::{Clock, LocalDate, LocalDateTime, Priority, TaskUid, When};
use restask::markdown::mutator::{apply, Mutation, MutationOutcome, SkipReason, WhenField};

const UID1: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const UID2: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpc";

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
        "- [ ] Buy milk ➕ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
        "  * [ ] Existing ⏫ 📅 2026-10-01 ➕ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn register_is_idempotent_for_same_uid() {
    let contents = "- [ ] Buy milk ➕ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
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
        "plain text line\n- [ ] Other 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
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
        "plain text line\n- [ ] Other 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n"
    );
}

#[test]
fn set_status_complete_and_uncomplete() {
    let contents = "- [ ] Buy milk ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
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
        "- [x] Buy milk ✅ 2026-09-21 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
        "- [ ] Buy milk ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn set_status_none_stamps_clock_today() {
    let out = run(
        "- [ ] Buy milk 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetStatus {
            uid: uid(UID1),
            checked: true,
            completed_on: None,
        }],
    );
    assert_eq!(
        out.contents,
        "- [x] Buy milk ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn set_priority_add_change_remove() {
    let contents = "- [ ] Buy milk 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::High),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy milk ⏫ 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
        "- [ ] Buy milk ⏬ 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
    let contents = "- [ ] Buy milk 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
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
        "- [ ] Buy milk 🛫 2026-09-30 08:15 ⏳ 2026-09-29 📅 2026-10-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
        "- [ ] Buy milk 🛫 2026-09-30 08:15 ⏳ 2026-09-29 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn edit_text_preserves_metadata() {
    let contents = "- [ ] Buy milk 🔼 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[Mutation::EditText {
            uid: uid(UID1),
            text: "Buy oat milk and bread".to_string(),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] Buy oat milk and bread 🔼 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn canonical_tail_order_full_rewrite() {
    let contents = "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb ➕ 2026-09-01 ✅ 2026-09-20 📅 2026-09-03 17:30 ⏳ 2026-09-02 🛫 2026-09-01 ⏬\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Medium),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔼 🛫 2026-09-01 ⏳ 2026-09-02 📅 2026-09-03 17:30 ✅ 2026-09-20 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn non_targeted_lines_and_text_preserved() {
    let contents = concat!(
        "# Title\n",
        "\n",
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        "plain line\n",
        "  - [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
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
    let contents = "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\r\nplain\r\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::High),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A ⏫ 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\r\nplain\r\n"
    );
}

#[test]
fn uid_not_found_is_skipped() {
    let out = run(
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
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
        "- [ ] A 📅 2026-13-45 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Low),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔽 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn multiple_ops_on_same_uid_apply_sequentially() {
    let out = run(
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
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
        "- [x] A 🔺 ✅ 2026-09-21 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn move_to_done_newest_on_top() {
    let out = run(
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "## Done\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc ✅ 2026-09-18\n",
        ),
        &[
            Mutation::SetStatus {
                uid: uid(UID1),
                checked: true,
                completed_on: Some(date("2026-09-22")),
            },
            Mutation::MoveToDone { uid: uid(UID1) },
        ],
    );
    assert_eq!(out.applied.len(), 2);
    assert_eq!(
        out.contents,
        concat!(
            "## Done\n",
            "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc ✅ 2026-09-18\n",
        )
    );
}

#[test]
fn move_to_done_creates_level3_heading_when_absent() {
    let out = run(
        "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::MoveToDone { uid: uid(UID1) }],
    );
    assert_eq!(
        out.contents,
        "\n### Done\n- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn move_to_done_terminates_unterminated_last_line() {
    let out = run(
        "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n- [ ] Z 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc",
        &[Mutation::MoveToDone {
            uid: uid(UID1),
        }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] Z 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "\n",
            "### Done\n",
            "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        )
    );
}

#[test]
fn restore_from_done_bottom_of_active_region() {
    let out = run(
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "## Done\n",
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd ✅ 2026-09-18\n",
        ),
        &[Mutation::RestoreFromDone { uid: uid(UID2) }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "## Done\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd ✅ 2026-09-18\n",
        )
    );
}

#[test]
fn restore_from_done_without_heading_goes_to_eof() {
    let out = run(
        concat!(
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        ),
        &[Mutation::RestoreFromDone { uid: uid(UID2) }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        )
    );
}

#[test]
fn delete_removes_line_entirely() {
    let out = run(
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "plain\n",
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        ),
        &[Mutation::Delete { uid: uid(UID2) }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\nplain\n"
    );
}

#[test]
fn structural_ops_skip_when_uid_missing() {
    let contents = "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n";
    let out = run(
        contents,
        &[
            Mutation::MoveToDone { uid: uid(UID2) },
            Mutation::RestoreFromDone { uid: uid(UID2) },
            Mutation::Delete { uid: uid(UID2) },
        ],
    );
    assert!(out.applied.is_empty());
    assert_eq!(out.skipped.len(), 3);
    assert!(out
        .skipped
        .iter()
        .all(|(_, reason)| *reason == SkipReason::UidNotFound));
    assert_eq!(out.contents, contents);
}

#[test]
fn crlf_move_to_done_uses_dominant_ending() {
    let out = run(
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\r\n",
            "## Done\r\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc ✅ 2026-09-18\r\n",
        ),
        &[
            Mutation::SetStatus {
                uid: uid(UID1),
                checked: true,
                completed_on: Some(date("2026-09-22")),
            },
            Mutation::MoveToDone { uid: uid(UID1) },
        ],
    );
    assert_eq!(
        out.contents,
        concat!(
            "## Done\r\n",
            "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\r\n",
            "- [x] C 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc ✅ 2026-09-18\r\n",
        )
    );
}

#[test]
fn write_atomic_renames_hidden_tmp() {
    use std::fs;
    use std::path::PathBuf;

    use restask::markdown::mutator::write_atomic;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");

    write_atomic(&path, "hello\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    let mut entries: Vec<PathBuf> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    assert_eq!(entries, vec![path.clone()]);

    write_atomic(&path, "second\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
