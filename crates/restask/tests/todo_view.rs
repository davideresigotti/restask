//! §7 TODO.md view tests: full render scenario, empty-section omission, ordering rules,
//! and wikilink rules including hostile stems.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use restask::config::VaultConfig;
use restask::domain::{
    ListSlug, LocalDate, LocalDateTime, Priority, SourceRef, Status, Task, TaskUid, When,
};
use restask::markdown::mutator::{Mutation, WhenField};
use restask::markdown::todo_view::{
    inbox_line, is_sealed, is_view, looks_like_mirror, mirror_edits, mirror_line, render,
    section_priority,
};

mod common;
use common::sealed;

const U1: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const U2: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpc";
const U3: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpd";
const U4: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpe";
const U5: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpf";
const U6: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpg";
const U7: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdph";

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap()
}

fn task(uid: &str, text: &str) -> Task {
    Task {
        uid: TaskUid::parse(uid).unwrap(),
        // The calendar unmarked lines of the view belong to (§7.5).
        list: ListSlug::from_name("inbox").unwrap(),
        text: text.to_string(),
        status: Status::Active,
        priority: None,
        due: None,
        start: None,
        scheduled: None,
        recurrence: None,
        created: None,
        parent: None,
        source: SourceRef {
            path: "TODO.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: fixed_time(),
        last_modified: fixed_time(),
    }
}

fn vault_task(uid: &str, text: &str, path: &str, line: usize) -> Task {
    let mut task = task(uid, text);
    task.source = SourceRef {
        path: path.to_string(),
        line,
    };
    task
}

fn date(value: &str) -> LocalDate {
    LocalDate::parse(value).unwrap()
}

/// The frontmatter block `render` emits for the default (§14) inbox list.
const FRONTMATTER: &str = "---\nrestask-list: inbox\nrestask-render: ";

#[test]
fn render_full_scenario() {
    let mut tasks = BTreeMap::new();

    let mut buy_milk = task(U1, "Buy milk");
    buy_milk.created = Some(date("2026-09-22"));
    tasks.insert(buy_milk.uid.clone(), buy_milk);

    let mut ssl = vault_task(
        U2,
        "Setup SSL certificate renew alert",
        "Home Lab Test.md",
        6,
    );
    ssl.priority = Some(Priority::Highest);
    ssl.source_heading = Some("To Do".to_string());
    tasks.insert(ssl.uid.clone(), ssl);

    let mut review = vault_task(U3, "Review architecture plan", "Project Alpha Test.md", 7);
    review.priority = Some(Priority::High);
    review.source_heading = Some("Tasks".to_string());
    tasks.insert(review.uid.clone(), review);

    let mut docs = vault_task(U4, "Update documentation", "Project Alpha Test.md", 8);
    docs.priority = Some(Priority::Medium);
    docs.source_heading = Some("Tasks".to_string());
    tasks.insert(docs.uid.clone(), docs);

    let mut backup = vault_task(
        U5,
        "Configure automatic backup to NAS",
        "Home Lab Test.md",
        6,
    );
    backup.priority = Some(Priority::Medium);
    backup.source_heading = Some("To Do".to_string());
    tasks.insert(backup.uid.clone(), backup);

    let mut talos = vault_task(U6, "Deploy Talos Linux on mini-PC", "Home Lab Test.md", 7);
    talos.priority = Some(Priority::Low);
    talos.source_heading = Some("To Do".to_string());
    tasks.insert(talos.uid.clone(), talos);

    let mut trash = task(U7, "Take out trash");
    trash.priority = Some(Priority::Low);
    trash.status = Status::Completed {
        on: date("2026-09-19"),
    };
    tasks.insert(trash.uid.clone(), trash);

    let out = render(&tasks, &VaultConfig::default());
    assert_eq!(
        out,
        sealed(concat!(
            "---\n",
            "restask-list: inbox\n",
            "---\n",
                        "# TODO\n",
            "\n",
            "## 🔺 Highest Priority\n",
            "- [ ] Setup SSL certificate renew alert 🔺 [[Home Lab Test#To Do|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
            "\n",
            "## ⏫ High Priority\n",
            "- [ ] Review architecture plan ⏫ [[Project Alpha Test#Tasks|Project Alpha Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd\n",
            "\n",
            "## 🔼 Medium Priority\n",
            "- [ ] Configure automatic backup to NAS 🔼 [[Home Lab Test#To Do|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpf\n",
            "- [ ] Update documentation 🔼 [[Project Alpha Test#Tasks|Project Alpha Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpe\n",
            "\n",
            "## 🔽 Low Priority\n",
            "- [ ] Deploy Talos Linux on mini-PC 🔽 [[Home Lab Test#To Do|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpg\n",
            "\n",
            "## No Priority\n",
            "- [ ] Buy milk ➕ 2026-09-22 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "\n",
            "## Done\n",
            "- [x] Take out trash 🔽 ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdph\n",
        ))
    );
}

#[test]
fn empty_sections_are_omitted() {
    let mut tasks = BTreeMap::new();
    tasks.insert(task(U1, "Buy milk").uid.clone(), task(U1, "Buy milk"));
    let out = render(&tasks, &VaultConfig::default());
    assert_eq!(
        out,
        sealed(concat!(
            "---\n",
            "restask-list: inbox\n",
            "---\n",
            "# TODO\n",
            "\n",
            "## No Priority\n",
            "- [ ] Buy milk 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb\n",
            "\n",
            "## Done\n",
        ))
    );
    // The only section besides Done is the one for tasks without a priority.
    assert_eq!(out.matches("Priority").count(), 1);
    assert!(out.starts_with(FRONTMATTER));
}

#[test]
fn tasks_without_a_priority_come_below_every_priority_section() {
    let mut tasks = BTreeMap::new();
    let quick = task(U1, "Quick capture");
    tasks.insert(quick.uid.clone(), quick);
    let mut someday = task(U2, "Someday");
    someday.priority = Some(Priority::Lowest);
    tasks.insert(someday.uid.clone(), someday);
    let mut bought = task(U3, "Bought");
    bought.status = Status::Completed {
        on: date("2026-09-19"),
    };
    tasks.insert(bought.uid.clone(), bought);

    let out = render(&tasks, &VaultConfig::default());
    let at = |heading: &str| {
        out.find(heading)
            .unwrap_or_else(|| panic!("{heading}: {out}"))
    };
    assert!(at("## ⏬ Lowest Priority\n") < at("## No Priority\n- [ ] Quick capture 🆔"));
    assert!(at("## No Priority\n") < at("## Done\n"));
    assert!(!out.contains("## Inbox"));
}

#[test]
fn done_newest_on_top_and_completed_vault_tasks_omitted() {
    let mut tasks = BTreeMap::new();

    let mut first = task(U2, "B");
    first.status = Status::Completed {
        on: date("2026-09-19"),
    };
    tasks.insert(first.uid.clone(), first);

    let mut second = task(U3, "C");
    second.status = Status::Completed {
        on: date("2026-09-20"),
    };
    tasks.insert(second.uid.clone(), second);

    let mut third = task(U4, "D");
    third.status = Status::Completed {
        on: date("2026-09-19"),
    };
    tasks.insert(third.uid.clone(), third);

    let mut vault_done = vault_task(U1, "A", "Other.md", 3);
    vault_done.priority = Some(Priority::High);
    vault_done.status = Status::Completed {
        on: date("2026-09-21"),
    };
    tasks.insert(vault_done.uid.clone(), vault_done);

    let out = render(&tasks, &VaultConfig::default());
    assert!(!out.contains("- [x] A "));
    assert!(!out.contains("## ⏫ High Priority"));
    assert_eq!(
        out,
        sealed(concat!(
            "---\n",
            "restask-list: inbox\n",
            "---\n",
            "# TODO\n",
            "\n",
            "## Done\n",
            "- [x] C ✅ 2026-09-20 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd\n",
            "- [x] D ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpe\n",
            "- [x] B ✅ 2026-09-19 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc\n",
        ))
    );
}

#[test]
fn render_emits_the_configured_inbox_list() {
    let cfg = VaultConfig {
        inbox_list: "tasks".to_string(),
        ..VaultConfig::default()
    };
    let out = render(&BTreeMap::new(), &cfg);
    assert!(out.starts_with("---\nrestask-list: tasks\nrestask-render: "));
}

#[test]
fn mirror_line_rules() {
    let mut linked = vault_task(U1, "Write docs", "Home Lab Test.md", 5);
    linked.source_heading = Some("To Do".to_string());
    assert_eq!(
        mirror_line(&linked),
        "- [ ] Write docs [[Home Lab Test#To Do|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );

    let mut no_heading = vault_task(U1, "Write docs", "Home Lab Test.md", 5);
    no_heading.status = Status::Completed {
        on: date("2026-09-19"),
    };
    assert_eq!(
        mirror_line(&no_heading),
        "- [x] Write docs [[Home Lab Test|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );

    let hostile = vault_task(U1, "Weird", "We[ird|Task#1.md", 2);
    assert_eq!(
        mirror_line(&hostile),
        "- [ ] Weird We[ird|Task#1 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );

    let mut hostile_heading = vault_task(U1, "Weird", "We[ird|Task#1.md", 2);
    hostile_heading.source_heading = Some("Head".to_string());
    assert_eq!(
        mirror_line(&hostile_heading),
        "- [ ] Weird We[ird|Task#1#Head 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );

    let mut scheduled = vault_task(U1, "Write docs", "Home Lab Test.md", 5);
    scheduled.priority = Some(Priority::Medium);
    scheduled.start = Some(When::Date(date("2026-09-21")));
    scheduled.scheduled = Some(When::DateTime(
        LocalDateTime::parse("2026-09-22 08:00").unwrap(),
    ));
    scheduled.due = Some(When::Date(date("2026-09-23")));
    assert_eq!(
        mirror_line(&scheduled),
        "- [ ] Write docs 🔼 🛫 2026-09-21 ⏳ 2026-09-22 08:00 📅 2026-09-23 [[Home Lab Test|Home Lab Test]] 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );
}

#[test]
fn inbox_line_full_canonical() {
    let mut full = task(U1, "Plan week");
    full.priority = Some(Priority::Medium);
    full.start = Some(When::Date(date("2026-09-21")));
    full.scheduled = Some(When::DateTime(
        LocalDateTime::parse("2026-09-22 08:00").unwrap(),
    ));
    full.due = Some(When::DateTime(
        LocalDateTime::parse("2026-09-23 17:30").unwrap(),
    ));
    full.status = Status::Completed {
        on: date("2026-09-20"),
    };
    full.created = Some(date("2026-09-01"));
    assert_eq!(
        inbox_line(&full, &VaultConfig::default()),
        "- [x] Plan week 🔼 🛫 2026-09-21 ⏳ 2026-09-22 08:00 📅 2026-09-23 17:30 ✅ 2026-09-20 ➕ 2026-09-01 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
    );

    assert_eq!(
        inbox_line(&task(U2, "Buy milk"), &VaultConfig::default()),
        "- [ ] Buy milk 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc"
    );
}

#[test]
fn a_line_of_the_view_names_its_calendar_unless_it_is_the_views_own() {
    // §7.5: the view is bound to `inbox`; `work` is another calendar it shows.
    let cfg = VaultConfig::default();
    let mut tasks = BTreeMap::new();
    let mut readme = task(U1, "Update restask README");
    readme.list = ListSlug::from_name("work").unwrap();
    readme.priority = Some(Priority::Highest);
    let mut review = task(U2, "Ask for review");
    review.list = ListSlug::from_name("work").unwrap();
    let mut shipped = task(U4, "Ship it");
    shipped.list = ListSlug::from_name("work").unwrap();
    shipped.status = Status::Completed {
        on: date("2026-09-20"),
    };
    let milk = task(U3, "Buy milk");
    // A note task of that calendar is a mirror line like any other: its link says
    // where it lives.
    let mut noted = vault_task(U5, "From a note", "Notes/Work.md", 3);
    noted.list = ListSlug::from_name("work").unwrap();
    noted.priority = Some(Priority::Highest);
    for task in [readme.clone(), review, shipped, milk.clone(), noted] {
        tasks.insert(task.uid.clone(), task);
    }

    assert_eq!(
        inbox_line(&readme, &cfg),
        format!("- [ ] Update restask README 🔺 📁 work 🆔 {U1}")
    );
    assert_eq!(inbox_line(&milk, &cfg), format!("- [ ] Buy milk 🆔 {U3}"));
    let out = render(&tasks, &cfg);
    let body = out.split_once("# TODO\n\n").unwrap().1;
    assert_eq!(
        body,
        format!(
            "## 🔺 Highest Priority\n\
             - [ ] From a note 🔺 [[Work|Work]] 🆔 {U5}\n\
             - [ ] Update restask README 🔺 📁 work 🆔 {U1}\n\
             \n\
             ## No Priority\n\
             - [ ] Ask for review 📁 work 🆔 {U2}\n\
             - [ ] Buy milk 🆔 {U3}\n\
             \n\
             ## Done\n\
             - [x] Ship it ✅ 2026-09-20 📁 work 🆔 {U4}\n"
        )
    );
    assert!(out.starts_with("---\nrestask-list: inbox\nrestask-render: "));
    assert!(is_sealed(&out));

    // Bound to `work`, the same tasks are marked the other way round.
    let bound = VaultConfig {
        inbox_list: "work".to_string(),
        ..VaultConfig::default()
    };
    assert_eq!(
        inbox_line(&readme, &bound),
        format!("- [ ] Update restask README 🔺 🆔 {U1}")
    );
    assert_eq!(
        inbox_line(&milk, &bound),
        format!("- [ ] Buy milk 📁 inbox 🆔 {U3}")
    );
}

#[test]
fn unprioritized_note_tasks_stay_in_their_note() {
    // README: "Tasks without an assigned priority remain solely in their source file."
    let mut tasks = BTreeMap::new();
    let idea = vault_task(U1, "Just an idea", "Notes/Ideas.md", 3);
    tasks.insert(idea.uid.clone(), idea);
    let quick = task(U2, "Quick capture");
    tasks.insert(quick.uid.clone(), quick);

    let out = render(&tasks, &VaultConfig::default());
    assert!(!out.contains("Just an idea"));
    assert!(out.contains("## No Priority\n- [ ] Quick capture 🆔"));
}

#[test]
fn lines_with_empty_text_have_no_double_spaces() {
    let mut empty = vault_task(U1, "", "Notes/A.md", 1);
    empty.priority = Some(Priority::High);
    assert_eq!(mirror_line(&empty), format!("- [ ] ⏫ [[A|A]] 🆔 {U1}"));
    assert_eq!(
        inbox_line(&task(U2, ""), &VaultConfig::default()),
        format!("- [ ] 🆔 {U2}")
    );
}

// ---- mirror edits: TODO.md is a two-way view ----

/// A vault with one prioritized, dated note task, and the render that shows it.
fn mirrored() -> (BTreeMap<TaskUid, Task>, String) {
    let mut tasks = BTreeMap::new();
    let mut plan = vault_task(U1, "Review the plan", "Projects/Alpha.md", 4);
    plan.priority = Some(Priority::High);
    plan.due = Some(When::Date(date("2026-09-25")));
    plan.source_heading = Some("Tasks".to_string());
    tasks.insert(plan.uid.clone(), plan);
    let quick = task(U2, "Quick capture");
    tasks.insert(quick.uid.clone(), quick);
    let rendered = render(&tasks, &VaultConfig::default());
    (tasks, rendered)
}

fn edits_for(current: &str, rendered: &str, tasks: &BTreeMap<TaskUid, Task>) -> Vec<Mutation> {
    let mut edits = mirror_edits(
        current,
        rendered,
        tasks,
        &VaultConfig::default(),
        date("2026-09-22"),
    );
    assert!(edits.len() <= 1, "all edits target the one source note");
    edits.remove("Projects/Alpha.md").unwrap_or_default()
}

#[test]
fn an_untouched_view_yields_no_edits() {
    let (tasks, rendered) = mirrored();
    assert!(edits_for(&rendered, &rendered, &tasks).is_empty());
}

#[test]
fn checking_a_mirror_line_completes_the_source_task() {
    let (tasks, rendered) = mirrored();
    let current = rendered.replace("- [ ] Review the plan", "- [x] Review the plan");
    let uid = TaskUid::parse(U1).unwrap();
    assert_eq!(
        edits_for(&current, &rendered, &tasks),
        vec![
            Mutation::SetStatus {
                uid: uid.clone(),
                checked: true,
                completed_on: Some(date("2026-09-22")),
            },
            Mutation::MoveToDone { uid },
        ]
    );
}

#[test]
fn a_plugin_style_completion_keeps_its_own_date() {
    // The plugin stamps ✅ and moves the line under `## Done` inside TODO.md.
    let (tasks, rendered) = mirrored();
    let line = rendered
        .lines()
        .find(|line| line.contains("Review the plan"))
        .unwrap()
        .to_string();
    let moved = line
        .replace("- [ ]", "- [x]")
        .replace(" 🆔", " ✅ 2026-09-20 🆔");
    let current = rendered.replace(&format!("{line}\n"), "") + &moved + "\n";
    let edits = edits_for(&current, &rendered, &tasks);
    assert_eq!(
        edits[0],
        Mutation::SetStatus {
            uid: TaskUid::parse(U1).unwrap(),
            checked: true,
            completed_on: Some(date("2026-09-20")),
        }
    );
}

#[test]
fn field_edits_on_a_mirror_line_reach_the_source() {
    let (tasks, rendered) = mirrored();
    let current = rendered
        .replace("Review the plan ⏫", "Review the whole plan 🔺")
        .replace("📅 2026-09-25", "📅 2026-10-01 09:00");
    let uid = TaskUid::parse(U1).unwrap();
    assert_eq!(
        edits_for(&current, &rendered, &tasks),
        vec![
            Mutation::EditText {
                uid: uid.clone(),
                text: "Review the whole plan".to_string(),
            },
            Mutation::SetPriority {
                uid: uid.clone(),
                priority: Some(Priority::Highest),
            },
            Mutation::SetWhen {
                uid,
                field: WhenField::Due,
                value: Some(When::DateTime(
                    LocalDateTime::parse("2026-10-01 09:00").unwrap()
                )),
            },
        ]
    );
}

#[test]
fn a_stale_view_is_not_an_edit_and_the_note_wins_conflicts() {
    let (mut tasks, rendered) = mirrored();
    // The note changed after the render: the view is merely stale.
    let uid = TaskUid::parse(U1).unwrap();
    tasks.get_mut(&uid).unwrap().priority = Some(Priority::Low);
    assert!(edits_for(&rendered, &rendered, &tasks).is_empty());

    // The user re-prioritized in the view too: the note's own change wins that field,
    // while an untouched-in-the-note field (the checkbox) still carries over.
    let current = rendered
        .replace("Review the plan ⏫", "Review the plan 🔺")
        .replace("- [ ] Review", "- [x] Review");
    let edits = edits_for(&current, &rendered, &tasks);
    assert!(edits
        .iter()
        .all(|edit| !matches!(edit, Mutation::SetPriority { .. })));
    assert!(matches!(
        edits[0],
        Mutation::SetStatus { checked: true, .. }
    ));
}

/// `view` without the line that holds `needle`.
fn without_line(view: &str, needle: &str) -> String {
    let kept: Vec<&str> = view.lines().filter(|line| !line.contains(needle)).collect();
    kept.join("\n") + "\n"
}

#[test]
fn deleting_a_mirror_line_deletes_the_source_task() {
    let (tasks, rendered) = mirrored();
    let current = without_line(&rendered, "Review the plan");
    assert_ne!(current, rendered);
    assert_eq!(
        edits_for(&current, &rendered, &tasks),
        vec![Mutation::Delete {
            uid: TaskUid::parse(U1).unwrap()
        }]
    );
}

/// A missing line proves a deletion only in this engine's own last render, edited and
/// whole (§7.1). Everything else is absence without a cause.
#[test]
fn a_missing_mirror_line_is_no_deletion_without_proof() {
    let (tasks, rendered) = mirrored();
    let gone = without_line(&rendered, "Review the plan");

    // Some device's render (sealed): the note that explains it may be on its way.
    assert!(edits_for(
        &sealed(&without_line(&gone, "restask-render")),
        &rendered,
        &tasks
    )
    .is_empty());
    // Edited from another render than this engine's last one: a stale view.
    let stale = gone.replacen("restask-render: ", "restask-render: 0", 1);
    assert_ne!(stale, gone);
    assert!(edits_for(&stale, &rendered, &tasks).is_empty());
    // A device took the line out itself and says so: the seal claims no render (§7.1).
    let disclaimed: String = gone
        .lines()
        .map(|line| match line.starts_with("restask-render: ") {
            true => "restask-render: 0000000000000000\n".to_string(),
            false => format!("{line}\n"),
        })
        .collect();
    assert_ne!(disclaimed, gone);
    assert!(edits_for(&disclaimed, &rendered, &tasks).is_empty());
    // No seal line at all.
    assert!(edits_for(&without_line(&gone, "restask-render"), &rendered, &tasks).is_empty());
    // Cut short: the frontmatter is there, the rest is not.
    let head: String = rendered
        .lines()
        .take(4)
        .map(|line| format!("{line}\n"))
        .collect();
    assert!(head.contains("restask-render") && !head.contains("## Done"));
    assert!(edits_for(&head, &rendered, &tasks).is_empty());
    assert!(edits_for("", &rendered, &tasks).is_empty());
    // The line is still there, as something the parser does not read as that task.
    let mangled = rendered.replace("- [ ] Review the plan", "Review the plan");
    assert!(edits_for(&mangled, &rendered, &tasks).is_empty());
}

#[test]
fn a_deleted_mirror_line_loses_to_a_note_that_changed_the_task() {
    let (mut tasks, rendered) = mirrored();
    let current = without_line(&rendered, "Review the plan");
    let uid = TaskUid::parse(U1).unwrap();
    if let Some(task) = tasks.get_mut(&uid) {
        task.text = "Review the new plan".to_string();
    }
    assert!(edits_for(&current, &rendered, &tasks).is_empty());
}

#[test]
fn deleting_a_line_of_the_views_own_is_no_mirror_edit() {
    let (tasks, rendered) = mirrored();
    let current = without_line(&rendered, "Quick capture");
    assert!(mirror_edits(
        &current,
        &rendered,
        &tasks,
        &VaultConfig::default(),
        date("2026-09-22")
    )
    .is_empty());
}

#[test]
fn inbox_lines_and_unknown_lines_are_never_mirror_edits() {
    let (tasks, rendered) = mirrored();
    let current = rendered.replace("- [ ] Quick capture", "- [x] Quick capture reworded")
        + "- [ ] typed by hand\n- [ ] pasted 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdph\n";
    assert!(edits_for(&current, &rendered, &tasks).is_empty());
}

#[test]
fn mirror_shape_is_a_wikilink_right_before_the_uid() {
    assert!(looks_like_mirror(&format!(
        "- [ ] Review ⏫ [[Alpha#Tasks|Alpha]] 🆔 {U1}"
    )));
    // An inbox task that merely mentions a note is a task of its own.
    assert!(!looks_like_mirror(&format!(
        "- [ ] Call [[John]] ➕ 2026-09-22 🆔 {U1}"
    )));
    assert!(!looks_like_mirror("- [ ] Call [[John]]"));
}

// ── §7.2 the seal ─────────────────────────────────────────────────────────────────────

#[test]
fn a_render_is_sealed_and_any_edit_breaks_the_seal() {
    let mut tasks = BTreeMap::new();
    let mut alert = vault_task(U2, "Renew the certificate", "Home Lab.md", 5);
    alert.priority = Some(Priority::Highest);
    tasks.insert(alert.uid.clone(), alert);
    let out = render(&tasks, &VaultConfig::default());

    assert!(is_sealed(&out));
    assert!(!is_sealed(&out.replace("Renew", "Renewed")));
    assert!(!is_sealed(&out.replace("- [ ]", "- [x]")));
    assert!(!is_sealed(&format!("{out}- [ ] typed below\n")));
    // CRLF, a seal that is not a line of its own, no seal at all: not sealed.
    assert!(!is_sealed(&out.replace('\n', "\r\n")));
    assert!(!is_sealed(
        &out.replace("restask-render", "x restask-render")
    ));
    // The seal is a frontmatter property: the same line in the body seals nothing.
    let seal = out.lines().nth(2).unwrap();
    assert!(seal.starts_with("restask-render: "));
    let below = out.replace(&format!("{seal}\n---\n"), &format!("---\n{seal}\n"));
    assert!(!is_sealed(&below));
    assert!(!is_sealed("# TODO\n\n## Done\n"));
    assert!(!is_sealed(""));
}

#[test]
fn the_seal_is_fnv1a_64_of_the_view_without_its_seal_line() {
    // Pinned: the Obsidian plugin computes the same digest (`test/filing.test.ts`).
    let out = render(&BTreeMap::new(), &VaultConfig::default());
    assert_eq!(
        out,
        concat!(
            "---\n",
            "restask-list: inbox\n",
            "restask-render: c5cb3ffaf7a952cc\n",
            "---\n",
            "# TODO\n",
            "\n",
            "## Done\n",
        )
    );
    assert_eq!(
        out,
        sealed("---\nrestask-list: inbox\n---\n# TODO\n\n## Done\n")
    );
}

#[test]
fn a_sealed_view_carries_no_edit_even_where_it_differs_from_the_last_render() {
    let mut tasks = BTreeMap::new();
    let mut alert = vault_task(U2, "Renew the certificate", "Home Lab.md", 5);
    alert.priority = Some(Priority::Highest);
    tasks.insert(alert.uid.clone(), alert.clone());
    let cfg = VaultConfig::default();
    let rendered = render(&tasks, &cfg);

    // Another device re-rendered after the task was re-prioritized in its note there.
    let mut elsewhere = tasks.clone();
    elsewhere.get_mut(&alert.uid).unwrap().priority = Some(Priority::Low);
    let theirs = render(&elsewhere, &cfg);
    assert!(mirror_edits(&theirs, &rendered, &tasks, &cfg, date("2026-09-22")).is_empty());

    // The same text typed by hand over this engine's render is an edit.
    let by_hand = rendered.replace("certificate 🔺", "certificate 🔽");
    let edits = mirror_edits(&by_hand, &rendered, &tasks, &cfg, date("2026-09-22"));
    assert_eq!(
        edits.get("Home Lab.md"),
        Some(&vec![Mutation::SetPriority {
            uid: alert.uid.clone(),
            priority: Some(Priority::Low),
        }])
    );
}

// ── §7.3 nothing in the body but the view ─────────────────────────────────────────────

#[test]
fn a_render_writes_no_comment_line() {
    let mut tasks = BTreeMap::new();
    let mut alert = vault_task(U2, "Renew the certificate", "Home Lab.md", 5);
    alert.priority = Some(Priority::Highest);
    tasks.insert(alert.uid.clone(), alert);
    let out = render(&tasks, &VaultConfig::default());
    assert!(!out.contains("<!--"));
    assert!(is_view(&out));
}

#[test]
fn a_view_is_known_by_its_seal_line_or_by_the_comment_lines_of_earlier_renders() {
    // Edited since: the seal no longer matches, the file is still one of restask's.
    let edited = render(&BTreeMap::new(), &VaultConfig::default()).replace("# TODO", "# Tasks");
    assert!(!is_sealed(&edited));
    assert!(is_view(&edited));
    // Views rendered before the seal moved into the frontmatter.
    let legacy = "---\nrestask-list: inbox\n---\n<!-- AUTOGENERATED BY Restask -->\n\n# TODO\n";
    assert!(is_view(legacy));
    assert!(!is_sealed(legacy));
    let comment_seal =
        "---\nrestask-list: inbox\n---\n<!-- restask-render: 5d0d649d4cd83964 -->\n\n# TODO\n\n## Done\n";
    assert!(is_view(comment_seal));
    assert!(
        !is_sealed(comment_seal),
        "the comment seals nothing any more"
    );
    // Quoted in a task, neither is a mark; a file restask never wrote is no view.
    assert!(!is_view(
        "- [ ] remove `<!-- AUTOGENERATED BY Restask -->` from the docs\n"
    ));
    assert!(!is_view("# TODO\n\nrestask-render: 5d0d649d4cd83964\n"));
    assert!(!is_view("# TODO\n\n- [ ] plain\n"));
    assert!(!is_view(""));
}

#[test]
fn a_priority_section_heading_names_its_priority_and_nothing_else_does() {
    assert_eq!(
        section_priority("🔺 Highest Priority"),
        Some(Priority::Highest)
    );
    assert_eq!(section_priority("⏫ High Priority"), Some(Priority::High));
    assert_eq!(
        section_priority("🔼 Medium Priority"),
        Some(Priority::Medium)
    );
    assert_eq!(section_priority("🔽 Low Priority"), Some(Priority::Low));
    assert_eq!(
        section_priority("⏬ Lowest Priority"),
        Some(Priority::Lowest)
    );
    for heading in ["No Priority", "Done", "TODO", "Highest Priority", "🔺", ""] {
        assert_eq!(section_priority(heading), None, "{heading}");
    }
}

// ---- counted UIDs (§3.1) ----

#[test]
fn counted_uids_render_short_and_sort_by_number() {
    let mut tasks = BTreeMap::new();
    for (uid, text) in [
        ("restask-a10", "tenth"),
        ("restask-a9", "ninth"),
        ("restask-b2", "second, another device"),
        ("restask-a2", "second"),
    ] {
        let task = task(uid, text);
        tasks.insert(task.uid.clone(), task);
    }
    let mut done = task("restask-a11", "done later");
    done.status = Status::Completed {
        on: date("2026-09-20"),
    };
    tasks.insert(done.uid.clone(), done);
    let mut done = task("restask-a3", "done earlier");
    done.status = Status::Completed {
        on: date("2026-09-20"),
    };
    tasks.insert(done.uid.clone(), done);
    let rendered = render(&tasks, &VaultConfig::default());
    // `No Priority` in creation order — 9 before 10, not as text — and `Done` on one
    // day the other way round.
    assert!(
        rendered.ends_with(
            "## No Priority\n\
             - [ ] second 🆔 a2\n\
             - [ ] second, another device 🆔 b2\n\
             - [ ] ninth 🆔 a9\n\
             - [ ] tenth 🆔 a10\n\
             \n## Done\n\
             - [x] done later ✅ 2026-09-20 🆔 a11\n\
             - [x] done earlier ✅ 2026-09-20 🆔 a3\n"
        ),
        "{rendered}"
    );
}

/// §7.1: a deleted mirror line deletes its task only when the UID is nowhere in the view
/// any more — and `a4` is not in the view because `a42` is.
#[test]
fn a_deleted_mirror_line_is_told_from_a_uid_that_begins_like_it() {
    let mut tasks = BTreeMap::new();
    for (uid, text, line) in [
        ("restask-a4", "Review the plan", 4),
        ("restask-a42", "Book the room", 5),
    ] {
        let mut task = vault_task(uid, text, "Projects/Alpha.md", line);
        task.priority = Some(Priority::High);
        task.source_heading = Some("Tasks".to_string());
        tasks.insert(task.uid.clone(), task);
    }
    let rendered = render(&tasks, &VaultConfig::default());
    assert!(rendered.contains("- [ ] Review the plan ⏫ [[Alpha#Tasks|Alpha]] 🆔 a4\n"));
    let current = without_line(&rendered, "Review the plan");
    assert_eq!(
        edits_for(&current, &rendered, &tasks),
        vec![Mutation::Delete {
            uid: TaskUid::parse("restask-a4").unwrap()
        }]
    );
}
