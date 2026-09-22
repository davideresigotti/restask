//! Routing conformance tests (§5): frontmatter scanning, resolution chain, shadowing,
//! conflicts, local-only default.

use restask::domain::ListSlug;
use restask::router::{scan_frontmatter, NoteMeta, NoteRouting, Router};
use restask::TaskresError;

fn meta(path: &str, file_list: Option<&str>, folder_list: Option<&str>) -> NoteMeta {
    NoteMeta {
        path: path.to_string(),
        file_list: file_list.map(str::to_string),
        folder_list: folder_list.map(str::to_string),
    }
}

fn list(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

#[test]
fn scan_finds_both_markers_and_ignores_unknown_keys_and_body() {
    let src = concat!(
        "---\n",
        "title: Security\n",
        "restask-list: University\n",
        "restask-list-root: Home Lab\n",
        "tags: [x]\n",
        "---\n",
        "# Notes\n",
        "restask-list: Forge\n",
    );
    let (file_list, folder_list) = scan_frontmatter(src);
    assert_eq!(file_list.as_deref(), Some("University"));
    assert_eq!(folder_list.as_deref(), Some("Home Lab"));
}

#[test]
fn scan_requires_frontmatter_at_byte_zero_and_termination() {
    assert_eq!(scan_frontmatter(""), (None, None));
    assert_eq!(scan_frontmatter("# Just a note\n"), (None, None));
    // Unterminated block: not frontmatter at all.
    assert_eq!(scan_frontmatter("---\nrestask-list: X\n"), (None, None));
    // Opening delimiter is not at byte 0.
    assert_eq!(
        scan_frontmatter("\n---\nrestask-list: X\n---\n"),
        (None, None)
    );
    // Delimiter line must be exactly `---`.
    assert_eq!(
        scan_frontmatter("----\nrestask-list: X\n----\n"),
        (None, None)
    );
}

#[test]
fn scan_trims_values_and_tolerates_crlf() {
    let src = "---\r\nrestask-list:   University  \r\nrestask-list-root:\tHome Lab\r\n---\r\n";
    let (file_list, folder_list) = scan_frontmatter(src);
    assert_eq!(file_list.as_deref(), Some("University"));
    assert_eq!(folder_list.as_deref(), Some("Home Lab"));
}

#[test]
fn scan_empty_value_counts_as_unset() {
    let src = "---\nrestask-list:\nrestask-list-root:   \n---\n";
    assert_eq!(scan_frontmatter(src), (None, None));
}

#[test]
fn scan_first_occurrence_wins() {
    let src = "---\nrestask-list: A\nrestask-list: B\n---\n";
    let (file_list, _) = scan_frontmatter(src);
    assert_eq!(file_list.as_deref(), Some("A"));
}

#[test]
fn scan_keys_are_case_sensitive() {
    let src = "---\nRESTASK-LIST: University\nRestask-List-Root: Home Lab\n---\n";
    assert_eq!(scan_frontmatter(src), (None, None));
}

#[test]
fn worked_example_5_3() {
    let metas = vec![
        meta("2. Areas/Home Lab/Home Lab.md", None, Some("Home Lab")),
        meta("2. Areas/Home Lab/Security.md", None, None),
        meta("2. Areas/Home Lab/Alarm.md", None, None),
        meta("University.md", Some("University"), None),
        meta("Inbox.md", None, None),
    ];
    let router = Router::build(&metas).unwrap();
    assert_eq!(
        router.resolve("2. Areas/Home Lab/Home Lab.md", None),
        NoteRouting::List(list("Home Lab"))
    );
    assert_eq!(
        router.resolve("2. Areas/Home Lab/Security.md", None),
        NoteRouting::List(list("home-lab"))
    );
    assert_eq!(
        router.resolve("2. Areas/Home Lab/Alarm.md", None),
        NoteRouting::List(list("home-lab"))
    );
    assert_eq!(
        router.resolve("University.md", Some("University")),
        NoteRouting::List(list("university"))
    );
    assert_eq!(router.resolve("Inbox.md", None), NoteRouting::LocalOnly);
    // The engine-managed inbox file is excluded from Router (§5.2); from the router's point
    // of view an unmarked TODO.md is just a note with no markers.
    assert_eq!(router.resolve("TODO.md", None), NoteRouting::LocalOnly);
}

#[test]
fn file_marker_wins_and_root_still_declares_the_folder() {
    let metas = vec![meta("Projects/Plan.md", Some("Work"), Some("Home Lab"))];
    let router = Router::build(&metas).unwrap();
    // `restask-list` wins for the file itself (§5.1)…
    assert_eq!(
        router.resolve("Projects/Plan.md", Some("Work")),
        NoteRouting::List(list("Work"))
    );
    // …while `restask-list-root` still covers the folder for siblings.
    assert_eq!(
        router.resolve("Projects/Ideas.md", None),
        NoteRouting::List(list("Home Lab"))
    );
}

#[test]
fn deeper_root_shadows_shallower_one() {
    let metas = vec![
        meta("Areas/Home Lab.md", None, Some("Home Lab")),
        meta("Areas/Projects/Deep.md", None, Some("Work")),
    ];
    let router = Router::build(&metas).unwrap();
    assert_eq!(
        router.resolve("Areas/Projects/Deep.md", None),
        NoteRouting::List(list("work"))
    );
    assert_eq!(
        router.resolve("Areas/Projects/Child.md", None),
        NoteRouting::List(list("work"))
    );
    assert_eq!(
        router.resolve("Areas/Other.md", None),
        NoteRouting::List(list("home-lab"))
    );
    // The file's own marker beats even the nearest folder root.
    assert_eq!(
        router.resolve("Areas/Projects/Deep.md", Some("University")),
        NoteRouting::List(list("university"))
    );
}

#[test]
fn resolution_walks_up_to_the_vault_root() {
    let metas = vec![meta("Root Note.md", None, Some("Home Lab"))];
    let router = Router::build(&metas).unwrap();
    assert_eq!(
        router.resolve("a/b/deep.md", None),
        NoteRouting::List(list("home-lab"))
    );
    assert_eq!(
        router.resolve("top.md", None),
        NoteRouting::List(list("home-lab"))
    );
}

#[test]
fn conflicting_roots_for_one_directory_are_a_hard_error() {
    let metas = vec![
        meta("Areas/A.md", None, Some("Home Lab")),
        meta("Areas/B.md", None, Some("Work")),
    ];
    match Router::build(&metas) {
        Err(TaskresError::ListConflict { dir, a, b }) => {
            assert_eq!(dir, "Areas");
            assert_eq!(a, "Home Lab");
            assert_eq!(b, "Work");
        }
        other => panic!("expected ListConflict, got {other:?}"),
    }
}

#[test]
fn conflicting_roots_at_the_vault_root_are_a_hard_error() {
    let metas = vec![
        meta("One.md", None, Some("Alpha")),
        meta("Two.md", None, Some("Beta")),
    ];
    match Router::build(&metas) {
        Err(TaskresError::ListConflict { dir, a, b }) => {
            assert_eq!(dir, "");
            assert_eq!(a, "Alpha");
            assert_eq!(b, "Beta");
        }
        other => panic!("expected ListConflict, got {other:?}"),
    }
}

#[test]
fn identical_list_declared_twice_is_not_a_conflict() {
    let metas = vec![
        meta("Areas/A.md", None, Some("Home Lab")),
        meta("Areas/B.md", None, Some("home lab")),
    ];
    let router = Router::build(&metas).unwrap();
    assert_eq!(
        router.resolve("Areas/B.md", None),
        NoteRouting::List(list("home-lab"))
    );
}

#[test]
fn root_name_that_slugifies_to_empty_is_rejected() {
    let metas = vec![meta("a.md", None, Some("!!!"))];
    match Router::build(&metas) {
        Err(TaskresError::Validation { field, .. }) => assert_eq!(field, "list"),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn unslugifiable_file_marker_falls_through_to_inheritance() {
    let metas = vec![meta("Areas/Home Lab.md", None, Some("Home Lab"))];
    let router = Router::build(&metas).unwrap();
    assert_eq!(router.resolve("n.md", Some("???")), NoteRouting::LocalOnly);
    assert_eq!(
        router.resolve("Areas/x.md", Some("???")),
        NoteRouting::List(list("home-lab"))
    );
}
