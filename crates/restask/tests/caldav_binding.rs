//! §5.4: which of the server's collections a list is — by its path, else by its name.

use restask::caldav::{list_name, resolve_list, Bound, CollectionInfo};
use restask::domain::ListSlug;

fn calendar(path: &str, display: Option<&str>, tasks: bool) -> CollectionInfo {
    CollectionInfo {
        href: format!("/me/{path}/"),
        slug: path.to_string(),
        display_name: display.map(str::to_string),
        supports_vtodo: tasks,
        ctag: None,
    }
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

const PHONE: &str = "56de6126-33a4-46fd-a66e-3cc49ad32fe5";
const OTHER: &str = "0b1f6c1e-3a52-4c0e-9d58-0f3c2f6f1a77";

#[test]
fn a_list_is_the_collection_at_its_path_else_the_one_calendar_of_its_name() {
    let server = vec![
        calendar("personal", Some("Personal"), true),
        calendar("Tasks", None, true),
        calendar(PHONE, Some("Home Lab"), true),
        calendar("sport", Some("Sport"), false),
    ];
    let at = |path: &str| Bound::At(path.to_string());
    assert_eq!(resolve_list(&server, &slug("personal")), at("personal"));
    // A path spelled another way.
    assert_eq!(resolve_list(&server, &slug("tasks")), at("Tasks"));
    // A calendar another client made: found by the name it shows.
    assert_eq!(resolve_list(&server, &slug("Home Lab")), at(PHONE));
    assert_eq!(resolve_list(&server, &slug(PHONE)), at(PHONE));
    assert_eq!(resolve_list(&server, &slug("work")), Bound::Missing);
    // The collection at the list's own path is taken whatever it holds.
    assert_eq!(resolve_list(&server, &slug("sport")), at("sport"));
}

#[test]
fn a_calendar_that_holds_no_tasks_is_not_found_by_its_name() {
    let server = vec![calendar(PHONE, Some("Sport"), false)];
    assert_eq!(resolve_list(&server, &slug("sport")), Bound::Missing);
}

#[test]
fn the_path_wins_over_a_name_and_two_of_one_name_are_not_chosen_between() {
    // The pair a daemon without this rule left behind: its own empty collection at the
    // path, and the calendar the phone made.
    let pair = vec![
        calendar("restask", Some("Restask"), true),
        calendar(PHONE, Some("restask"), true),
    ];
    assert_eq!(
        resolve_list(&pair, &slug("restask")),
        Bound::At("restask".to_string())
    );
    let twins = vec![
        calendar(PHONE, Some("Prova"), true),
        calendar(OTHER, Some("prova"), true),
    ];
    assert_eq!(
        resolve_list(&twins, &slug("prova")),
        Bound::Ambiguous(vec![PHONE.to_string(), OTHER.to_string()])
    );
}

#[test]
fn a_calendar_is_named_as_the_vault_reaches_it() {
    let server = vec![
        calendar("personal", Some("Personal"), true),
        calendar(PHONE, Some("University"), true),
        calendar("restask", Some("Restask"), true),
        calendar(OTHER, Some("restask"), true),
        calendar("work", None, true),
    ];
    let name = |at: usize| list_name(&server[at], &server);
    assert_eq!(name(0), Some(slug("personal")));
    assert_eq!(name(1), Some(slug("university")));
    assert_eq!(name(2), Some(slug("restask")));
    // Its name is taken by the collection at that path: reached by its own path.
    assert_eq!(name(3), Some(slug(OTHER)));
    assert_eq!(name(4), Some(slug("work")));
}
