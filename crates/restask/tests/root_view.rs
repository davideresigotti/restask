//! §7.6 the view of a root note: where the section is, what a render makes of it and
//! what it leaves alone, the seal over the section, and the shape of a mirror line.

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use restask::config::VaultConfig;
use restask::domain::{ListSlug, LocalDate, Priority, SourceRef, Status, Task, TaskUid};
use restask::markdown::mutator::Mutation;
use restask::markdown::root_view::{
    is_mirror_shaped, is_sealed, mirror_edits, remembered, render, section,
};

const U1: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const U2: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpc";
const U3: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpd";
const U4: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpe";
const U5: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpf";
const ROOT: &str = "Homelab/Home Lab.md";

/// FNV-1a (64-bit), computed here independently of the crate.
fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A root note: prose `above` its view, the `view`, and what follows it.
fn rooted(above: &str, view: &str, below: &str) -> String {
    format!(
        "---\nrestask-list-root: Home Lab\nrestask-render: {}\n---\n{above}{view}{below}",
        fnv(view)
    )
}

fn task(uid: &str, text: &str, path: &str, line: usize) -> Task {
    let stamp = Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap();
    Task {
        uid: TaskUid::parse(uid).unwrap(),
        list: ListSlug::from_name("home-lab").unwrap(),
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
            path: path.to_string(),
            line,
        },
        source_heading: Some("TODO".to_string()),
        source_mtime: stamp,
        last_modified: stamp,
    }
}

fn set(tasks: Vec<Task>) -> BTreeMap<TaskUid, Task> {
    tasks
        .into_iter()
        .map(|task| (task.uid.clone(), task))
        .collect()
}

#[test]
fn the_view_is_the_todo_section_of_the_note() {
    let cfg = VaultConfig::default();
    let head = "---\nrestask-list-root: Home Lab\n---\n";
    let found = |body: &str| {
        section(&format!("{head}{body}"), &cfg)
            .map(|view| (view.start - 3, view.end - 3, view.level))
    };

    // To the end of the note; the done heading is part of it, whatever its rank.
    assert_eq!(
        found("# Notes\nprose\n# TODO\n\n## Done\n"),
        Some((2, 5, 1))
    );
    assert_eq!(
        found("## todo\n- [ ] a\n## Done\n- [x] b\n"),
        Some((0, 4, 2))
    );
    // To the next heading of the same or a higher rank; lower ones stay inside.
    assert_eq!(
        found("## Todo\n### Sub\n- [ ] a\n## Links\nprose\n"),
        Some((0, 3, 2))
    );
    assert_eq!(found("## TODO\n# Next\n"), Some((0, 1, 2)));
    // The first such heading is the one.
    assert_eq!(found("# TODO\n# TODO\n"), Some((0, 1, 1)));
    // A heading in a fenced block is none; nor is other text with the word in it.
    assert_eq!(found("```\n# TODO\n```\n"), None);
    assert_eq!(found("# TODO list\n# To Do\n"), None);
    // A heading of the lowest rank leaves no rank for the sections.
    assert_eq!(found("###### TODO\n"), None);
    assert_eq!(found("##### TODO\n"), Some((0, 1, 5)));

    // No view where the seal could not be written.
    assert!(section("# TODO\n", &cfg).is_none(), "no frontmatter");
    assert!(section("--- \nrestask-list-root: x\n---\n# TODO\n", &cfg).is_none());
    assert!(section("---\r\nrestask-list-root: x\r\n---\r\n# TODO\r\n", &cfg).is_none());
}

#[test]
fn a_render_files_the_folder_under_the_heading_and_keeps_the_rest_of_the_note() {
    let cfg = VaultConfig::default();
    let above = "# Notes\nprose with a [[link]]\n\n- [ ] not in the view \u{1F53A} \u{1F194} restask-01jzq4tsvg2c9xkw7n5m8rhdpz\n";
    let note = format!(
        "---\nrestask-list-root: Home Lab\ntags: x\n---\n{above}## TODO\nstray prose\n- [ ] own low \u{1F53D} \u{1F194} {U1}\n- [ ] own plain \u{1F194} {U2}\n- [x] own done \u{2705} 2026-09-20 \u{1F194} {U3}\n# Links\nmore prose\n"
    );
    // Lines 9–13 of the note are the view (1-based).
    let mut low = task(U1, "own low", ROOT, 11);
    low.priority = Some(Priority::Low);
    let plain = task(U2, "own plain", ROOT, 12);
    let mut done = task(U3, "own done", ROOT, 13);
    done.status = Status::Completed {
        on: LocalDate::parse("2026-09-20").unwrap(),
    };
    let mut near = task(U4, "rotate the keys", "Homelab/Networking.md", 4);
    near.priority = Some(Priority::Highest);
    let mut deep = task(U5, "scrub", "Homelab/Storage/Disks.md", 2);
    deep.priority = Some(Priority::Low);
    deep.source_heading = None;
    // Not shown: outside the folder, in the inbox file, without a priority, completed.
    let mut elsewhere = task(
        "restask-01jzq4tsvg2c9xkw7n5m8rhdq0",
        "elsewhere",
        "Projects.md",
        3,
    );
    elsewhere.priority = Some(Priority::Highest);
    let mut inbox = task("restask-01jzq4tsvg2c9xkw7n5m8rhdq1", "inbox", "TODO.md", 5);
    inbox.priority = Some(Priority::Highest);
    let unranked = task(
        "restask-01jzq4tsvg2c9xkw7n5m8rhdq2",
        "unranked",
        "Homelab/Networking.md",
        5,
    );
    let mut finished = task(
        "restask-01jzq4tsvg2c9xkw7n5m8rhdq3",
        "finished",
        "Homelab/Networking.md",
        9,
    );
    finished.priority = Some(Priority::Highest);
    finished.status = Status::Completed {
        on: LocalDate::parse("2026-09-21").unwrap(),
    };
    // The note's own line above the view is in the note already.
    let mut above_view = task(
        "restask-01jzq4tsvg2c9xkw7n5m8rhdpz",
        "not in the view",
        ROOT,
        8,
    );
    above_view.priority = Some(Priority::Highest);
    let tasks = set(vec![
        low, plain, done, near, deep, elsewhere, inbox, unranked, finished, above_view,
    ]);

    let view = format!(
        "## TODO\n\n### \u{1F53A} Highest Priority\n- [ ] rotate the keys \u{1F53A} [[Networking#TODO|Networking]] \u{1F194} {U4}\n\n### \u{1F53D} Low Priority\n- [ ] own low \u{1F53D} \u{1F194} {U1}\n- [ ] scrub \u{1F53D} [[Disks|Disks]] \u{1F194} {U5}\n\n### No Priority\n- [ ] own plain \u{1F194} {U2}\n\n### Done\n- [x] own done \u{2705} 2026-09-20 \u{1F194} {U3}\n\n"
    );
    let expected = format!(
        "---\nrestask-list-root: Home Lab\nrestask-render: {}\ntags: x\n---\n{above}{view}# Links\nmore prose\n",
        fnv(&view)
    );
    let rendered = render(&note, ROOT, &tasks, &cfg).unwrap();
    assert_eq!(rendered, expected);
    assert!(is_sealed(&rendered, &cfg));
    assert!(!is_sealed(&note, &cfg));

    // Prose outside the view does not break the seal; an edit inside it does.
    assert!(is_sealed(
        &rendered.replace("more prose", "other prose"),
        &cfg
    ));
    assert!(!is_sealed(
        &rendered.replace("own plain", "own, plain"),
        &cfg
    ));
}

#[test]
fn an_empty_view_is_the_heading_and_the_done_heading() {
    let cfg = VaultConfig::default();
    let note = "---\nrestask-list-root: homelab\n---\n# Notes\n...\n# TODO\n\n## Done\n";
    let rendered = render(note, ROOT, &BTreeMap::new(), &cfg).unwrap();
    // The same view and digest are pinned in plugins/obsidian/test/filing.test.ts.
    assert!(rendered.contains("restask-render: 20eaf3e3ad473666\n"));
    assert_eq!(
        rendered,
        rooted("# Notes\n...\n", "# TODO\n\n## Done\n", "").replace("Home Lab", "homelab")
    );
    // A second render is the first.
    assert_eq!(
        render(&rendered, ROOT, &BTreeMap::new(), &cfg).unwrap(),
        rendered
    );
    // A note without the heading has no view.
    assert!(render(
        "---\nrestask-list-root: homelab\n---\n# To Do\n",
        ROOT,
        &BTreeMap::new(),
        &cfg
    )
    .is_none());
}

#[test]
fn a_mirror_line_is_known_by_its_exact_shape() {
    let id = "\u{1F194}";
    assert!(is_mirror_shaped(&format!(
        "- [ ] rotate \u{1F53A} [[Networking#TODO|Networking]] {id} {U1}"
    )));
    assert!(is_mirror_shaped(&format!(
        "- [ ] scrub \u{1F53D} \u{1F4C5} 2026-10-01 [[Disks|Disks]] {id} {U1}"
    )));
    // A task that ends in a wikilink is a task.
    assert!(!is_mirror_shaped(&format!(
        "- [ ] Install [[Vaultwarden]] {id} {U1}"
    )));
    assert!(!is_mirror_shaped(&format!(
        "- [ ] Install [[Vaultwarden]] \u{1F53A} {id} {U1}"
    )));
    assert!(!is_mirror_shaped(&format!(
        "- [ ] read [[Storage|the storage note]] \u{1F53A} {id} {U1}"
    )));
    // No priority in front of the link, no UID: not as a render writes it.
    assert!(!is_mirror_shaped(&format!(
        "- [ ] see [[Disks|Disks]] {id} {U1}"
    )));
    // Ticked by the user, it still is the line a render wrote.
    assert!(is_mirror_shaped(&format!(
        "- [x] scrub \u{1F53D} [[Disks|Disks]] {id} {U1}"
    )));
    assert!(!is_mirror_shaped("- [ ] scrub \u{1F53D} [[Disks|Disks]]"));
    assert!(!is_mirror_shaped("prose [[Disks|Disks]]"));
}

#[test]
fn edits_in_the_view_are_read_against_the_remembered_render() {
    let cfg = VaultConfig::default();
    let today = LocalDate::parse("2026-09-22").unwrap();
    let mut near = task(U4, "rotate the keys", "Homelab/Networking.md", 4);
    near.priority = Some(Priority::Highest);
    let mut other = task(U5, "scrub", "Homelab/Networking.md", 5);
    other.priority = Some(Priority::Highest);
    let tasks = set(vec![near, other]);
    let note = "---\nrestask-list-root: Home Lab\n---\n# TODO\n\n## Done\n";
    let rendered = render(note, ROOT, &tasks, &cfg).unwrap();
    let memory = remembered(&rendered, ROOT, &cfg).unwrap();
    let line =
        format!("- [ ] rotate the keys \u{1F53A} [[Networking#TODO|Networking]] \u{1F194} {U4}\n");
    assert!(rendered.contains(&line), "{rendered}");

    // A render holds no edit.
    assert!(mirror_edits(&rendered, &memory, ROOT, &tasks, &cfg, today).is_empty());

    // A changed emoji goes to the task's note.
    let edited = rendered.replace(&line, &line.replace('\u{1F53A}', "\u{1F53D}"));
    let edits = mirror_edits(&edited, &memory, ROOT, &tasks, &cfg, today);
    assert_eq!(
        edits.get("Homelab/Networking.md"),
        Some(&vec![Mutation::SetPriority {
            uid: TaskUid::parse(U4).unwrap(),
            priority: Some(Priority::Low),
        }])
    );

    // A deleted mirror line deletes the task in its note…
    let gone = rendered.replace(&line, "");
    let delete = vec![Mutation::Delete {
        uid: TaskUid::parse(U4).unwrap(),
    }];
    assert_eq!(
        mirror_edits(&gone, &memory, ROOT, &tasks, &cfg, today).get("Homelab/Networking.md"),
        Some(&delete)
    );
    // …but not in a view that is not this render edited: another seal, no done heading,
    // a render remembered for another note, or no view at all.
    let stale = gone.replacen("restask-render: ", "restask-render: 0", 1);
    assert!(mirror_edits(&stale, &memory, ROOT, &tasks, &cfg, today).is_empty());
    let cut = gone.replace("## Done\n", "");
    assert!(mirror_edits(&cut, &memory, ROOT, &tasks, &cfg, today).is_empty());
    assert!(mirror_edits(&gone, &memory, "Homelab/Home.md", &tasks, &cfg, today).is_empty());
    let unheaded = gone.replace("# TODO\n", "# Tasks\n");
    assert!(mirror_edits(&unheaded, &memory, ROOT, &tasks, &cfg, today).is_empty());
}
