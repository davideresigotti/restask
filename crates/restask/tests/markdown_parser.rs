//! §6.1 line-grammar conformance tests: grammar table, CRLF, `[X]`, `*`/`+` markers,
//! non-tasks, unknown emoji, token extraction and text normalization.

use restask::config::VaultConfig;
use restask::domain::{ListSlug, LocalDate, LocalDateTime, Priority, TaskUid, When};
use restask::markdown::parser::{
    link_parents, parse as parse_file, parse_line, ParsedTask, TaskDraft, TaskLine,
};

const UID: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpf";

const HOME_LAB: &str = include_str!("fixtures/Home Lab Test.md");
const PROJECT_ALPHA: &str = include_str!("fixtures/Project Alpha Test.md");
const TODO: &str = include_str!("fixtures/TODO.md");

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
         ✅ 2026-01-04 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpf 🔼",
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
    let line = format!("- [ ] a 🆔 restask-iou{}", "a".repeat(23));
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

#[test]
fn home_lab_note_done_split_and_headings() {
    let f = parse_file(HOME_LAB, &VaultConfig::default());
    assert_eq!(f.done_heading_line, Some(10));
    assert_eq!(f.tasks.len(), 5);

    let active: Vec<&ParsedTask> = f.tasks.iter().filter(|t| !t.in_done_region).collect();
    assert_eq!(active.len(), 4);
    assert_eq!(active[0].line_no, 5);
    assert_eq!(active[0].draft.text, "Clean up cable management");
    assert_eq!(active[0].heading.as_deref(), Some("TODO"));
    assert_eq!(active[1].draft.priority, Some(Priority::Medium));
    assert_eq!(active[2].draft.priority, Some(Priority::Low));
    assert_eq!(active[3].draft.text, "Test the plugin high");
    assert_eq!(active[3].draft.priority, None);

    let done: Vec<&ParsedTask> = f.tasks.iter().filter(|t| t.in_done_region).collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].line_no, 11);
    assert!(done[0].draft.checked);
    assert_eq!(done[0].draft.priority, Some(Priority::Highest));
    assert_eq!(
        done[0].draft.completed_on,
        Some(LocalDate::parse("2026-09-19").unwrap())
    );
    assert_eq!(done[0].heading.as_deref(), Some("Done"));
    assert!(link_parents(&f.tasks).iter().all(Option::is_none));
}

#[test]
fn project_alpha_done_region_extends_to_eof() {
    let f = parse_file(PROJECT_ALPHA, &VaultConfig::default());
    assert_eq!(f.done_heading_line, Some(6));
    assert_eq!(f.tasks.len(), 2);
    assert!(f.tasks.iter().all(|t| t.in_done_region));
    assert_eq!(f.tasks[0].draft.priority, Some(Priority::High));
    assert_eq!(f.tasks[0].draft.text, "Review architecture plan");
    assert_eq!(f.tasks[1].draft.priority, Some(Priority::Medium));
    assert_eq!(f.tasks[1].heading.as_deref(), Some("Done"));
}

#[test]
fn todo_md_view_file_parses() {
    // The §7 fixture is the canonical empty view: frontmatter, marker, title, `## Done`.
    let f = parse_file(TODO, &VaultConfig::default());
    assert_eq!(f.done_heading_line, Some(10));
    assert!(f.tasks.is_empty());
}

#[test]
fn todo_md_view_with_tasks_parses() {
    // The pre-0.1.0 view shape (marker first, timestamp callout, `### Done`): the parser
    // is format-agnostic, so the historical scenario is pinned inline.
    let contents = concat!(
        "<!-- AUTOGENERATED BY Restask -->\n",
        "\n",
        "> [!NOTE] Synchronized automatically with vault notes. Last updated: `2026-09-19 12:25`.\n",
        "\n",
        "## 🔺 Highest Priority\n",
        "- [ ] Setup SSL certificate renew alert 🔺 [[Home Lab Test#To Do|Home Lab Test]]\n",
        "- [ ] test task 🔺 [[Home Lab Test#To Do|Home Lab Test]]\n",
        "\n",
        "## ⏫ High Priority\n",
        "- [ ] Review architecture plan ⏫ [[Project Alpha Test#Tasks|Project Alpha Test]]\n",
        "\n",
        "## 🔼 Medium Priority\n",
        "- [ ] Configure automatic backup to NAS 🔼 [[Home Lab Test#To Do|Home Lab Test]]\n",
        "- [ ] Update documentation 🔼 [[Project Alpha Test#Tasks|Project Alpha Test]]\n",
        "\n",
        "## 🔽 Low Priority\n",
        "- [ ] Deploy Talos Linux on mini-PC 🔽 [[Home Lab Test#To Do|Home Lab Test]]\n",
        "\n",
        "\n",
        "### Done\n",
        "- [x] Take out trash 🔽 ✅ 2026-09-19\n",
    );
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.done_heading_line, Some(20));
    assert_eq!(f.tasks.len(), 7);

    let first = &f.tasks[0];
    assert_eq!(first.line_no, 6);
    assert_eq!(first.heading.as_deref(), Some("🔺 Highest Priority"));
    assert_eq!(first.draft.priority, Some(Priority::Highest));
    assert_eq!(
        first.draft.text,
        "Setup SSL certificate renew alert [[Home Lab Test#To Do|Home Lab Test]]"
    );

    let done: Vec<&ParsedTask> = f.tasks.iter().filter(|t| t.in_done_region).collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].line_no, 21);
    assert_eq!(done[0].draft.text, "Take out trash");
}

#[test]
fn frontmatter_lines_are_never_tasks() {
    let contents = "---\nrestask-list: Home\n- [ ] ghost in frontmatter\n---\n- [ ] real task\n";
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.tasks.len(), 1);
    assert_eq!(f.tasks[0].line_no, 5);
    assert_eq!(f.tasks[0].draft.text, "real task");
}

#[test]
fn unterminated_frontmatter_is_not_frontmatter() {
    let contents = "---\n- [ ] ghost\n";
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.tasks.len(), 1);
    assert_eq!(f.tasks[0].line_no, 2);
    assert_eq!(f.tasks[0].draft.text, "ghost");
}

#[test]
fn fenced_blocks_are_never_tasks() {
    let contents = concat!(
        "```tasks\n",
        "- [ ] in tasks query\n",
        "```\n",
        "- [ ] real one\n",
        "~~~\n",
        "- [ ] in tilde fence\n",
        "~~~\n",
        "- [ ] another real\n",
    );
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.tasks.len(), 2);
    assert_eq!(f.tasks[0].draft.text, "real one");
    assert_eq!(f.tasks[1].draft.text, "another real");
}

#[test]
fn crlf_file_lines() {
    let contents = "# T\r\n\r\n- [ ] a 📅 2026-01-01\r\n";
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.tasks.len(), 1);
    assert_eq!(f.tasks[0].raw, "- [ ] a 📅 2026-01-01");
    assert_eq!(f.tasks[0].line_no, 3);
}

#[test]
fn done_heading_config_case_and_persistence() {
    let cfg = VaultConfig {
        done_heading: "Completed".to_string(),
        ..VaultConfig::default()
    };
    let contents = concat!(
        "## done\n",
        "- [ ] a\n",
        "## Done ###\n",
        "- [ ] b\n",
        "## Completed\n",
        "- [x] c\n",
        "## Later\n",
        "- [x] d\n",
    );
    let f = parse_file(contents, &cfg);
    assert_eq!(f.done_heading_line, Some(5));
    assert!(!f.tasks[0].in_done_region);
    assert!(!f.tasks[1].in_done_region);
    assert!(f.tasks[2].in_done_region);
    assert!(f.tasks[3].in_done_region);
    assert_eq!(f.tasks[1].heading.as_deref(), Some("Done"));
    assert_eq!(f.tasks[3].heading.as_deref(), Some("Later"));
}

#[test]
fn link_parents_nearest_ancestor() {
    let u1 = TaskUid::parse("restask-01jzq4tsvg2c9xkw7n5m8rhdpb").unwrap();
    let contents = concat!(
        "- [ ] A 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        "    - [ ] B 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        "    - [ ] C\n",
        "        - [ ] D 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd\n",
        "- [ ] E\n",
        "    - [ ] F 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpe\n",
    );
    let f = parse_file(contents, &VaultConfig::default());
    assert_eq!(f.tasks[1].indent_chars, 4);
    let parents = link_parents(&f.tasks);
    assert_eq!(parents[0], None);
    assert_eq!(parents[1], Some(u1.clone()));
    assert_eq!(parents[2], Some(u1));
    assert_eq!(parents[3], None);
    assert_eq!(parents[4], None);
    assert_eq!(parents[5], None);
}

#[test]
fn legacy_uid_prefix_is_still_a_uid_token() {
    let legacy = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpf";
    let task = parse_line(&format!("- [ ] old task 🆔 {legacy}")).unwrap();
    assert_eq!(task.draft.uid, Some(TaskUid::parse(legacy).unwrap()));
    assert_eq!(task.draft.text, "old task");
}

#[test]
fn nesting_never_crosses_a_heading_and_done_records_are_flat() {
    let contents = concat!(
        "# Tasks\n",
        "- [ ] parent 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
        "    - [ ] child 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        "# Other\n",
        "    - [ ] indented under a new heading 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd\n",
        "## Done\n",
        "- [x] done parent ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpe\n",
        "    - [x] done child ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpf\n",
    );
    let parsed = parse_file(contents, &VaultConfig::default());
    let parents = link_parents(&parsed.tasks);
    assert_eq!(
        parents,
        vec![
            None,
            Some(TaskUid::parse("restask-01jzq4tsvg2c9xkw7n5m8rhdpb").unwrap()),
            None,
            None,
            None,
        ]
    );
}

#[test]
fn recurrence_token_takes_exactly_the_words_of_the_rule() {
    use restask::domain::Recurrence;
    let rule = |text: &str| Some(Recurrence::from_text(text).unwrap().0);

    let line = parse_line("- [ ] water plants 🔁 every 2 weeks on Monday, Thursday 📅 2026-09-21")
        .unwrap();
    assert_eq!(line.draft.text, "water plants");
    assert_eq!(
        line.draft.recurrence,
        rule("every 2 weeks on Monday, Thursday")
    );
    assert!(line.draft.due.is_some());

    // Text may follow the rule; a 🔁 that introduces no rule is just text.
    let line = parse_line("- [ ] 🔁 every month on the 15th pay the rent").unwrap();
    assert_eq!(line.draft.text, "pay the rent");
    assert_eq!(line.draft.recurrence, rule("every month on the 15th"));
    let line = parse_line("- [ ] replay 🔁 the song").unwrap();
    assert_eq!(line.draft.text, "replay 🔁 the song");
    assert_eq!(line.draft.recurrence, None);
    let line = parse_line("- [ ] glued 🔁every day").unwrap();
    assert_eq!(line.draft.recurrence, None);
}

#[test]
fn a_bare_created_token_asks_for_the_date() {
    let t = parse("- [ ] Buy milk ➕");
    assert_eq!(t.draft.text, "Buy milk");
    assert!(t.draft.wants_created);
    assert_eq!(t.draft.created, None);

    // Anywhere on the line, next to other tokens.
    let t = parse(&format!("- [ ] ➕ Buy milk 🔺 🆔 {UID}"));
    assert_eq!(t.draft.text, "Buy milk");
    assert!(t.draft.wants_created);
    assert_eq!(t.draft.priority, Some(Priority::Highest));
}

#[test]
fn a_dated_created_token_is_no_request_and_a_glued_one_is_text() {
    let t = parse("- [ ] Buy milk ➕ 2026-09-22");
    assert!(!t.draft.wants_created);
    assert_eq!(
        t.draft.created,
        Some(LocalDate::parse("2026-09-22").unwrap())
    );

    // A date the calendar does not have is still the token's value, not a request.
    let t = parse("- [ ] Buy milk ➕ 2026-13-45");
    assert!(!t.draft.wants_created);
    assert_eq!(t.draft.created, None);

    let t = parse("- [ ] 2➕2 and a➕");
    assert!(!t.draft.wants_created);
    assert_eq!(t.draft.text, "2➕2 and a➕");

    assert!(!parse("- [ ] Buy milk").draft.wants_created);
}

// ---- the calendar token (§6.1, §7.5) ----

#[test]
fn calendar_token_names_a_list_and_is_not_text() {
    let t = parse("- [ ] Update restask README 🔺 📁 work");
    assert_eq!(t.draft.text, "Update restask README");
    assert_eq!(t.draft.priority, Some(Priority::Highest));
    assert_eq!(t.draft.list, Some(ListSlug::from_name("work").unwrap()));

    // Anywhere in the body, any case; hyphens inside a name; the first one wins.
    let t = parse("- [ ] 📁 Home-Lab fix the rack 📁 other");
    assert_eq!(t.draft.text, "fix the rack");
    assert_eq!(t.draft.list, Some(ListSlug::from_name("home-lab").unwrap()));
    let t = parse(&format!("- [ ] a 📁\twork2 🆔 {UID}"));
    assert_eq!(t.draft.list.unwrap().as_str(), "work2");
    assert_eq!(t.draft.uid.unwrap().as_str(), UID);
}

#[test]
fn a_calendar_emoji_without_a_name_is_text() {
    for line in ["- [ ] tidy the 📁", "- [ ] tidy the 📁 -x", "- [ ] 📁work"] {
        let t = parse(line);
        assert_eq!(t.draft.list, None, "{line}");
        assert!(t.draft.text.contains('📁'), "{line}");
    }
    // The name ends where the word of letters, digits and inner hyphens ends.
    let t = parse("- [ ] a 📁 work, then more");
    assert_eq!(t.draft.list.unwrap().as_str(), "work");
    assert_eq!(t.draft.text, "a , then more");
}

// ---- counted UIDs (§3.1): the token a line carries is the tag and the number ----

#[test]
fn a_counted_uid_token_is_read_as_the_uid_it_spells() {
    let t = parse("- [ ] Buy milk 🔺 🆔 a42");
    assert_eq!(t.draft.uid, Some(TaskUid::parse("restask-a42").unwrap()));
    assert_eq!(t.draft.text, "Buy milk");
    assert_eq!(t.draft.priority, Some(Priority::Highest));
    // Up to four letters, any number without a leading zero; tabs as blanks.
    for (line, uid) in [
        ("- [ ] x 🆔 z1", "restask-z1"),
        (
            "- [ ] x 🆔\tabcd123456789012345",
            "restask-abcd123456789012345",
        ),
        ("- [ ] x 🆔 a42 trailing words", "restask-a42"),
    ] {
        assert_eq!(
            parse(line).draft.uid,
            Some(TaskUid::parse(uid).unwrap()),
            "{line}"
        );
    }
    assert_eq!(
        parse("- [ ] x 🆔 a42 trailing words").draft.text,
        "x trailing words"
    );
}

#[test]
fn a_token_is_a_whole_word_or_no_uid_at_all() {
    // Not a tag and a number, or the number runs into something else: the line has no
    // UID, and the text keeps what was typed.
    for line in [
        "- [ ] x 🆔 a42b",
        "- [ ] x 🆔 a42_1",
        "- [ ] x 🆔 a042",
        "- [ ] x 🆔 a0",
        "- [ ] x 🆔 abcde1",
        "- [ ] x 🆔 A42",
        "- [ ] x 🆔 42",
        "- [ ] x 🆔 restask-a42",
        "- [ ] x 🆔a42",
    ] {
        let t = parse(line);
        assert_eq!(t.draft.uid, None, "{line}");
        assert_eq!(t.draft.text, line.strip_prefix("- [ ] ").unwrap(), "{line}");
    }
    // A long UID is still read in full, and only in full.
    assert_eq!(
        parse(&format!("- [ ] x 🆔 {UID}")).draft.uid,
        Some(TaskUid::parse(UID).unwrap())
    );
}

#[test]
fn uid_tokens_are_found_by_the_whole_token() {
    use restask::markdown::parser::uid_tokens;
    let text = "- [ ] one 🆔 a4\n- [ ] two 🆔 a42\nprose with a4 in it\n";
    let a4 = TaskUid::parse("restask-a4").unwrap();
    let a42 = TaskUid::parse("restask-a42").unwrap();
    let found = uid_tokens(text, &a4);
    assert_eq!(
        found.len(),
        1,
        "`a4` is not found in `a42`, nor in the prose"
    );
    assert_eq!(&text[found[0].clone()], "🆔 a4");
    assert_eq!(uid_tokens(text, &a42).len(), 1);
    assert!(uid_tokens(text, &TaskUid::parse("restask-a421").unwrap()).is_empty());
    let long = TaskUid::parse(UID).unwrap();
    assert_eq!(uid_tokens(&format!("- [ ] x 🆔 {UID}\n"), &long).len(), 1);
}
