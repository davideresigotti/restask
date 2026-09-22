//! §6.1 line-grammar conformance tests: grammar table, CRLF, `[X]`, `*`/`+` markers,
//! non-tasks, unknown emoji, token extraction and text normalization.

use restask::domain::{LocalDate, LocalDateTime, Priority, TaskUid, When};
use restask::markdown::parser::{parse_line, TaskDraft, TaskLine};

const UID: &str = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpf";

fn draft(text: &str) -> TaskDraft {
    TaskDraft {
        text: text.to_string(),
        ..TaskDraft::default()
    }
}

fn parse(line: &str) -> TaskLine {
    parse_line(line).expect("line should match the §6.1 grammar")
}

#[test]
fn basic_task_line() {
    let t = parse("- [ ] Buy milk");
    assert_eq!(t.indent_chars, 0);
    assert_eq!(t.marker, '-');
    assert_eq!(t.draft, draft("Buy milk"));
    assert!(!t.draft.checked);
}

#[test]
fn indent_counting() {
    assert_eq!(parse("  - [ ] a").indent_chars, 2);
    assert_eq!(parse("\t- [ ] a").indent_chars, 1);
    assert_eq!(parse(" \t  - [ ] a").indent_chars, 4);
}

#[test]
fn checkbox_case() {
    assert!(!parse("- [ ] a").draft.checked);
    assert!(parse("- [x] a").draft.checked);
    assert!(parse("- [X] a").draft.checked);
}

#[test]
fn list_markers() {
    assert_eq!(parse("- [ ] a").marker, '-');
    assert_eq!(parse("* [ ] a").marker, '*');
    assert_eq!(parse("+ [ ] a").marker, '+');
}

#[test]
fn crlf_and_bare_cr_endings() {
    let expected = parse("- [ ] Buy milk");
    assert_eq!(parse("- [ ] Buy milk\r\n"), expected);
    assert_eq!(parse("- [ ] Buy milk\r"), expected);
}

#[test]
fn non_task_lines() {
    for line in [
        "1. [ ] ordered list",
        "-[ ] no space after marker",
        "- [x]no space after bracket",
        "- [ ]no space after bracket",
        "- [y] unknown check",
        "- [ ]",
        "plain text line",
        "## Heading",
        "",
        "  ",
    ] {
        assert_eq!(parse_line(line), None, "not a task: {line:?}");
    }
}

#[test]
fn unknown_emoji_stay_in_text() {
    let t = parse("- [ ] Buy milk ❗ 🔥 ➜");
    assert_eq!(t.draft.priority, None);
    assert_eq!(t.draft, draft("Buy milk ❗ 🔥 ➜"));
}

#[test]
fn priority_must_be_standalone() {
    let t = parse("- [ ] Buy🔺milk");
    assert_eq!(t.draft.priority, None);
    assert_eq!(t.draft.text, "Buy🔺milk");
}

#[test]
fn all_five_priorities_recognized() {
    for p in Priority::ALL {
        let line = format!("- [ ] x {}", p.emoji());
        assert_eq!(parse(&line).draft.priority, Some(p));
    }
}

#[test]
fn priority_token_at_body_start() {
    let t = parse("- [ ] 🔺 Buy milk");
    assert_eq!(t.draft.priority, Some(Priority::Highest));
    assert_eq!(t.draft.text, "Buy milk");
}

#[test]
fn all_tokens_any_order() {
    let t = parse(
        "- [ ] Pay rent ⏳ 2026-01-02 🛫 2026-01-01 📅 2026-01-03 17:30 ➕ 2025-12-31 \
         ✅ 2026-01-04 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpf 🔼",
    );
    assert_eq!(t.draft.text, "Pay rent");
    assert_eq!(t.draft.priority, Some(Priority::Medium));
    assert_eq!(
        t.draft.scheduled,
        Some(When::parse_date_or_datetime("2026-01-02").unwrap())
    );
    assert_eq!(
        t.draft.start,
        Some(When::parse_date_or_datetime("2026-01-01").unwrap())
    );
    assert_eq!(
        t.draft.due,
        Some(When::parse_date_or_datetime("2026-01-03 17:30").unwrap())
    );
    assert_eq!(
        t.draft.created,
        Some(LocalDate::parse("2025-12-31").unwrap())
    );
    assert_eq!(
        t.draft.completed_on,
        Some(LocalDate::parse("2026-01-04").unwrap())
    );
    assert_eq!(t.draft.uid.as_ref().map(TaskUid::as_str), Some(UID));
    assert!(!t.draft.checked);
}

#[test]
fn due_date_and_datetime_forms() {
    assert_eq!(
        parse("- [ ] a 📅 2026-01-03").draft.due,
        Some(When::Date(LocalDate::parse("2026-01-03").unwrap()))
    );
    assert_eq!(
        parse("- [ ] a 📅 2026-01-03 17:30").draft.due,
        Some(When::DateTime(
            LocalDateTime::parse("2026-01-03 17:30").unwrap()
        ))
    );
    assert_eq!(
        parse("- [ ] a 📅\t2026-01-03").draft.due,
        Some(When::Date(LocalDate::parse("2026-01-03").unwrap()))
    );
}

#[test]
fn token_glued_to_text_is_still_a_token() {
    let t = parse("- [ ] Buy milk📅 2026-01-01");
    assert_eq!(
        t.draft.due,
        Some(When::Date(LocalDate::parse("2026-01-01").unwrap()))
    );
    assert_eq!(t.draft.text, "Buy milk");
}

#[test]
fn invalid_token_values_tolerated() {
    let t = parse("- [ ] a 📅 2026-13-45 ➕ 2026-02-30 ✅ 2026-00-11");
    assert_eq!(t.draft.due, None);
    assert_eq!(t.draft.created, None);
    assert_eq!(t.draft.completed_on, None);
    assert_eq!(t.draft.text, "a");
}

#[test]
fn uid_with_invalid_ulid_body_tolerated() {
    let line = format!("- [ ] a 🆔 taskres-iou{}", "a".repeat(23));
    let t = parse(&line);
    assert_eq!(t.draft.uid, None);
    assert_eq!(t.draft.text, "a");
}

#[test]
fn duplicate_tokens_first_wins_all_removed() {
    let t = parse("- [ ] a 📅 2026-01-03 📅 2026-02-04");
    assert_eq!(
        t.draft.due,
        Some(When::Date(LocalDate::parse("2026-01-03").unwrap()))
    );
    assert_eq!(t.draft.text, "a");
}

#[test]
fn text_whitespace_normalized() {
    let t = parse("- [ ] \tBuy   milk\t \t📅 2026-01-01");
    assert_eq!(t.draft.text, "Buy milk");
}

#[test]
fn empty_body_tasks() {
    assert_eq!(parse("- [ ] ").draft.text, "");
    let t = parse("- [ ]  📅 2026-01-01");
    assert_eq!(t.draft.text, "");
    assert_eq!(
        t.draft.due,
        Some(When::Date(LocalDate::parse("2026-01-01").unwrap()))
    );
}
