//! Note → list routing (§5). Pure — no I/O.
//!
//! Local-only by default: a note participates in syncing only when explicitly routed via a
//! `restask-list` frontmatter marker (file-level) or a `restask-list-root` marker (folder
//! level, recursive). The engine-managed inbox file is excluded here; the engine routes it
//! to the Inbox list itself (§5.2).

use std::collections::BTreeMap;

use crate::domain::ListSlug;
use crate::RestaskError;

/// Where a note's tasks belong (§5.2 resolution chain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteRouting {
    /// Unrouted: never parsed (beyond frontmatter), never modified, never synced.
    LocalOnly,
    /// Routed to the wrapped list.
    List(ListSlug),
}

/// Frontmatter scan result for one note (§5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteMeta {
    /// Vault-relative path, `/`-separated.
    pub path: String,
    /// `restask-list` value from the file's own frontmatter, if declared.
    pub file_list: Option<String>,
    /// `restask-list-root` value from the file's own frontmatter, if declared.
    pub folder_list: Option<String>,
}

/// Scans a note for the `restask-list` / `restask-list-root` frontmatter markers (§5.1) and
/// returns `(file_list, folder_list)`.
///
/// The frontmatter block is the lines between a `---` at byte 0 and the next `---`; an
/// unterminated block is not frontmatter. Keys are matched case-sensitively (surrounding
/// whitespace allowed), values are whitespace-trimmed, and an empty value counts as unset.
/// Unknown keys, malformed lines, and markers outside the block are ignored; when a marker
/// occurs more than once the first occurrence wins.
pub fn scan_frontmatter(contents: &str) -> (Option<String>, Option<String>) {
    let mut lines = contents.lines();
    if !lines.next().is_some_and(|line| line.trim() == "---") {
        return (None, None);
    }
    let (mut file_list, mut folder_list) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            return (file_list, folder_list);
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "restask-list" if file_list.is_none() => file_list = Some(value.to_string()),
            "restask-list-root" if folder_list.is_none() => folder_list = Some(value.to_string()),
            _ => {}
        }
    }
    // Unterminated frontmatter block: not a block at all.
    (None, None)
}

/// Deterministic note → list resolver built from a vault scan (§5.2).
#[derive(Debug)]
pub struct Router {
    /// Declared folder roots: directory key (vault-relative, `/`-separated, no trailing
    /// slash; `""` = vault root) → routed list.
    roots: BTreeMap<String, ListSlug>,
}

impl Router {
    /// Builds a router from scanned notes. Every `restask-list-root` declaration maps its
    /// file's directory to the slugified list name; two different roots for the same
    /// directory are a hard [`RestaskError::ListConflict`] (never silent). Declarations that
    /// slugify identically (e.g. differing only in case) count as the same list.
    ///
    /// A root name that produces an empty slug fails with [`RestaskError::Validation`].
    pub fn build(metas: &[NoteMeta]) -> Result<Router, RestaskError> {
        let mut roots: BTreeMap<String, (ListSlug, String)> = BTreeMap::new();
        for meta in metas {
            let Some(name) = meta.folder_list.as_deref() else {
                continue;
            };
            let slug = ListSlug::from_name(name)?;
            let dir = parent_dir(&meta.path);
            match roots.get(dir) {
                Some((existing, first_declared)) if existing == &slug => {}
                Some((_, first_declared)) => {
                    return Err(RestaskError::ListConflict {
                        dir: dir.to_string(),
                        a: first_declared.clone(),
                        b: name.to_string(),
                    });
                }
                None => {
                    roots.insert(dir.to_string(), (slug, name.to_string()));
                }
            }
        }
        Ok(Router {
            roots: roots
                .into_iter()
                .map(|(dir, (slug, _declared))| (dir, slug))
                .collect(),
        })
    }

    /// Resolves a note's routing (§5.2 chain): the file's own `restask-list` marker wins;
    /// otherwise the nearest enclosing directory with a declared root (deeper roots shadow
    /// shallower ones, walking up to the vault root); otherwise
    /// [`NoteRouting::LocalOnly`].
    ///
    /// A `file_list` value that cannot be slugified (e.g. empty or all-punctuation) is
    /// treated as absent, so the chain falls through to folder inheritance — mirroring
    /// [`scan_frontmatter`]'s empty-value handling.
    pub fn resolve(&self, path: &str, file_list: Option<&str>) -> NoteRouting {
        if let Some(name) = file_list {
            if let Ok(slug) = ListSlug::from_name(name) {
                return NoteRouting::List(slug);
            }
        }
        let mut dir = parent_dir(path);
        loop {
            if let Some(slug) = self.roots.get(dir) {
                return NoteRouting::List(slug.clone());
            }
            if dir.is_empty() {
                return NoteRouting::LocalOnly;
            }
            dir = parent_dir(dir);
        }
    }
}

/// Parent directory of a vault-relative path: the text before the last `/`, or `""` when the
/// file sits at the vault root (§5.2 — directory keys carry no trailing slash).
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}
