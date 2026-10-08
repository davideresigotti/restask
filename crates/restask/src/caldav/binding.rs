//! Which of the server's collections a list is (§5.4). Pure — no I/O.
//!
//! A list is named in the vault (`restask-list`, `todo_lists`, `📁`); a collection has a
//! path, and a calendar another client made has a path nobody chose. The two meet here:
//! by the path when a collection has the list's, else by the name its clients show.

use crate::caldav::protocol::CollectionInfo;
use crate::domain::ListSlug;

/// Where a list's collection is among the server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    /// The collection with this path segment.
    At(String),
    /// Several collections answer to the name (their path segments): none is taken.
    Ambiguous(Vec<String>),
    /// No collection is the list's.
    Missing,
}

/// Finds the collection of `list` in the server's listing. The collection at the list's
/// own path wins; else the one task calendar whose path spells the list's name in
/// another way (`Tasks` for `tasks`); else the one task calendar whose display name
/// does (`Home Lab` for `home-lab`). Two that answer alike are not chosen between.
pub fn resolve_list(collections: &[CollectionInfo], list: &ListSlug) -> Bound {
    if let Some(exact) = collections
        .iter()
        .find(|collection| collection.slug == list.as_str())
    {
        return Bound::At(exact.slug.clone());
    }
    let named = |name: &str| ListSlug::from_name(name).is_ok_and(|slug| slug == *list);
    let by_path: Vec<&CollectionInfo> = collections
        .iter()
        .filter(|collection| collection.supports_vtodo && named(&collection.slug))
        .collect();
    let by_name: Vec<&CollectionInfo> = collections
        .iter()
        .filter(|collection| {
            collection.supports_vtodo && collection.display_name.as_deref().is_some_and(named)
        })
        .collect();
    let found = if by_path.is_empty() { by_name } else { by_path };
    match found.as_slice() {
        [] => Bound::Missing,
        [only] => Bound::At(only.slug.clone()),
        several => Bound::Ambiguous(
            several
                .iter()
                .map(|collection| collection.slug.clone())
                .collect(),
        ),
    }
}

/// The list name under which the vault reaches `collection`: its display name as a slug
/// when that finds it, else its path as one. `None` when neither does — another
/// collection answers to both first — or neither makes a slug.
pub fn list_name(collection: &CollectionInfo, collections: &[CollectionInfo]) -> Option<ListSlug> {
    [
        collection.display_name.as_deref(),
        Some(collection.slug.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter_map(|name| ListSlug::from_name(name).ok())
    .find(|slug| resolve_list(collections, slug) == Bound::At(collection.slug.clone()))
}
