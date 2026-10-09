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
            created: Some(date("2026-09-22")),
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
            created: Some(date("2026-09-22")),
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
            created: Some(date("2026-09-22")),
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
                created: Some(date("2026-09-22")),
            },
            Mutation::Register {
                line_no: 2,
                uid: uid(UID1),
                created: Some(date("2026-09-22")),
            },
            Mutation::Register {
                line_no: 99,
                uid: uid(UID1),
                created: Some(date("2026-09-22")),
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
fn the_calendar_token_is_kept_and_written_before_the_uid() {
    let contents =
        "- [ ] A 📁 Work 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb ➕ 2026-09-01 📅 2026-09-03\n";
    let out = run(
        contents,
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Medium),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔼 📅 2026-09-03 ➕ 2026-09-01 📁 work 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
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
fn reassign_gives_a_duplicated_line_its_own_uid() {
    let out = run(
        concat!(
            "- [ ] A ⏫ ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [ ] A copy ⏫ ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        ),
        &[Mutation::Reassign {
            line_no: 2,
            uid: uid(UID2),
        }],
    );
    assert_eq!(out.applied.len(), 1);
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] A ⏫ ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [ ] A copy ⏫ ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        )
    );
}

fn draft(text: &str, uid_value: &str) -> restask::markdown::TaskDraft {
    restask::markdown::TaskDraft {
        uid: Some(uid(uid_value)),
        text: text.to_string(),
        ..Default::default()
    }
}

#[test]
fn insert_joins_the_bottom_of_the_active_region() {
    let out = run(
        concat!(
            "# Notes\n",
            "\n",
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "\n",
            "Some prose.\n",
            "\n",
            "## Done\n",
        ),
        &[Mutation::Insert {
            draft: draft("From the server", UID2),
            under: None,
        }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "# Notes\n",
            "\n",
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "- [ ] From the server 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "\n",
            "Some prose.\n",
            "\n",
            "## Done\n",
        )
    );
}

#[test]
fn insert_into_a_note_without_tasks_goes_above_the_done_heading_or_to_eof() {
    let with_heading = run(
        "# Notes\n\n## Done\n",
        &[Mutation::Insert {
            draft: draft("First", UID1),
            under: None,
        }],
    );
    assert_eq!(
        with_heading.contents,
        "# Notes\n- [ ] First 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n\n## Done\n"
    );
    let bare = run(
        "# Notes",
        &[Mutation::Insert {
            draft: draft("First", UID1),
            under: None,
        }],
    );
    assert_eq!(
        bare.contents,
        "# Notes\n- [ ] First 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn insert_of_a_completed_task_goes_under_the_done_heading() {
    let mut done = draft("Finished elsewhere", UID2);
    done.checked = true;
    done.completed_on = Some(date("2026-09-21"));
    let out = run(
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::Insert {
            draft: done,
            under: None,
        }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "\n",
            "### Done\n",
            "- [x] Finished elsewhere ✅ 2026-09-21 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        )
    );
}

#[test]
fn insert_under_a_parent_nests_one_level_in_the_files_own_indent_style() {
    let spaces = run(
        "- [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n- [ ] Next\n",
        &[Mutation::Insert {
            draft: draft("Child", UID2),
            under: Some(uid(UID1)),
        }],
    );
    assert_eq!(
        spaces.contents,
        concat!(
            "- [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "    - [ ] Child 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "- [ ] Next\n",
        )
    );
    let tabs = run(
        "* [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n\t* [ ] Sibling\n",
        &[Mutation::Insert {
            draft: draft("Child", UID2),
            under: Some(uid(UID1)),
        }],
    );
    assert_eq!(
        tabs.contents,
        concat!(
            "* [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "\t* [ ] Child 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "\t* [ ] Sibling\n",
        )
    );
}

#[test]
fn a_moved_subtask_leaves_its_indentation_behind() {
    // An indented line moved under the done heading must not nest under whatever task
    // happens to precede it there.
    let out = run(
        concat!(
            "- [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "    - [x] Child ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "## Done\n",
        ),
        &[Mutation::MoveToDone { uid: uid(UID2) }],
    );
    assert_eq!(
        out.contents,
        concat!(
            "- [ ] Parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "## Done\n",
            "- [x] Child ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        )
    );
}

#[test]
fn done_heading_creation_uses_the_configured_heading() {
    let cfg = VaultConfig {
        done_heading: "Fatto".to_string(),
        ..VaultConfig::default()
    };
    let out = apply(
        "- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::MoveToDone { uid: uid(UID1) }],
        &cfg,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        out.contents,
        "\n### Fatto\n- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn done_heading_creation_does_not_stack_blank_lines() {
    let out = run(
        "# Notes\n\n- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::MoveToDone { uid: uid(UID1) }],
    );
    assert_eq!(
        out.contents,
        "# Notes\n\n### Done\n- [x] A ✅ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn rekey_swaps_the_uid_of_the_line_carrying_it() {
    let out = run(
        "- [x] A ✅ 2026-09-22 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::Rekey {
            uid: uid(UID1),
            new_uid: uid(UID2),
        }],
    );
    assert_eq!(
        out.contents,
        "- [x] A ✅ 2026-09-22 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n"
    );
    let missing = run(
        "- [ ] B\n",
        &[Mutation::Rekey {
            uid: uid(UID1),
            new_uid: uid(UID2),
        }],
    );
    assert_eq!(missing.skipped.len(), 1);
}

#[test]
fn recurrence_sits_after_the_priority_and_a_record_loses_it() {
    use restask::domain::Recurrence;
    let rule = Recurrence::from_text("every week on Monday").unwrap().0;
    let out = run(
        "- [ ] A ⏫ 📅 2026-09-21 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetRecurrence {
            uid: uid(UID1),
            recurrence: Some(rule),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A ⏫ 🔁 every week on Monday 📅 2026-09-21 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
    // Written by hand in another spelling: any rewrite makes it canonical.
    let out = run(
        "- [ ] A 🔁 every week on mon and thu 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::Low),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] A 🔽 🔁 every week on Monday, Thursday 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
    let record = run(
        &out.contents,
        &[Mutation::Rekey {
            uid: uid(UID1),
            new_uid: uid(UID2),
        }],
    );
    assert_eq!(
        record.contents,
        "- [ ] A 🔽 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n"
    );
    let cleared = run(
        &out.contents,
        &[Mutation::SetRecurrence {
            uid: uid(UID1),
            recurrence: None,
        }],
    );
    assert_eq!(
        cleared.contents,
        "- [ ] A 🔽 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n"
    );
}

#[test]
fn register_writes_no_creation_date_unless_given_one() {
    let out = run(
        "- [ ] Buy milk 🔺\n",
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: None,
        }],
    );
    assert_eq!(out.contents, format!("- [ ] Buy milk 🔺 🆔 {UID1}\n"));
}

#[test]
fn register_answers_a_bare_created_token_and_keeps_a_date_the_line_states() {
    let asked = run(
        "- [ ] Buy milk ➕ 📅 2026-10-05\n",
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: Some(date("2026-09-22")),
        }],
    );
    assert_eq!(
        asked.contents,
        format!("- [ ] Buy milk 📅 2026-10-05 ➕ 2026-09-22 🆔 {UID1}\n")
    );

    let stated = run(
        "- [ ] Buy milk ➕ 2026-09-01\n",
        &[Mutation::Register {
            line_no: 1,
            uid: uid(UID1),
            created: Some(date("2026-09-22")),
        }],
    );
    assert_eq!(
        stated.contents,
        format!("- [ ] Buy milk ➕ 2026-09-01 🆔 {UID1}\n")
    );
}

#[test]
fn set_created_fills_the_date_in_and_never_replaces_one() {
    let op = Mutation::SetCreated {
        uid: uid(UID1),
        created: date("2026-09-01"),
    };
    let asked = run(
        &format!("- [ ] Buy ➕ milk 🔺 🆔 {UID1}\n"),
        std::slice::from_ref(&op),
    );
    assert_eq!(
        asked.contents,
        format!("- [ ] Buy milk 🔺 ➕ 2026-09-01 🆔 {UID1}\n")
    );

    let stated = format!("- [ ] Buy milk ➕ 2026-08-15 🆔 {UID1}\n");
    assert_eq!(run(&stated, &[op]).contents, stated);
}

#[test]
fn an_unanswered_created_request_survives_a_rewrite_of_its_line() {
    let out = run(
        &format!("- [ ] Buy milk ➕ 🆔 {UID1}\n"),
        &[Mutation::SetPriority {
            uid: uid(UID1),
            priority: Some(Priority::High),
        }],
    );
    assert_eq!(out.contents, format!("- [ ] Buy milk ⏫ ➕ 🆔 {UID1}\n"));
}

// ---- counted UIDs (§3.1) and the renumbering of long ones (§11.7) ----

#[test]
fn a_counted_uid_is_written_as_tag_and_number() {
    let counted = uid("restask-a42");
    let out = run(
        "- [ ] Buy milk 🔺\n",
        &[Mutation::Register {
            line_no: 1,
            uid: counted.clone(),
            created: None,
        }],
    );
    assert_eq!(out.contents, "- [ ] Buy milk 🔺 🆔 a42\n");
    // The line is found again by that token, and by no other that begins like it.
    let out = run(
        "- [ ] first 🆔 a4\n- [ ] second 🆔 a42\n",
        &[Mutation::EditText {
            uid: counted,
            text: "second, reworded".to_string(),
        }],
    );
    assert_eq!(
        out.contents,
        "- [ ] first 🆔 a4\n- [ ] second, reworded 🆔 a42\n"
    );
}

#[test]
fn renumbering_changes_the_token_and_nothing_else() {
    use restask::markdown::mutator::renumber;
    let renumbered = [
        (uid(UID1), uid("restask-a1")),
        (uid(UID2), uid("restask-a2")),
    ]
    .into_iter()
    .collect();
    // A line that is not in canonical form stays as it was typed, but for the token;
    // CRLF, prose, fenced blocks and lines of other tasks are not touched.
    let before = format!(
        "---\nrestask-list: Home\n---\n# Home\r\n\
         *   [ ]   odd   spacing 🆔 {UID1}   📅 2026-09-25\r\n\
         - [x] mirror ⏫ [[Alpha#Tasks|Alpha]] 🆔 {UID2}\n\
         - [ ] another device's 🆔 b7\n\
         prose that quotes 🆔 {UID1}\n\
         ```\n- [ ] fenced 🆔 {UID1}\n```\n\
         \t- [ ] nested 🆔 {UID1}"
    );
    let after = "---\nrestask-list: Home\n---\n# Home\r\n\
         *   [ ]   odd   spacing 🆔 a1   📅 2026-09-25\r\n\
         - [x] mirror ⏫ [[Alpha#Tasks|Alpha]] 🆔 a2\n\
         - [ ] another device's 🆔 b7\n"
        .to_string()
        + &format!(
            "prose that quotes 🆔 {UID1}\n```\n- [ ] fenced 🆔 {UID1}\n```\n\t- [ ] nested 🆔 a1"
        );
    let cfg = VaultConfig::default();
    assert_eq!(renumber(&before, &renumbered, &cfg), after);
    // Done once, done: nothing is left to renumber.
    assert_eq!(renumber(&after, &renumbered, &cfg), after);
}
