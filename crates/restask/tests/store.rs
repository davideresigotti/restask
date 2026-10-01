//! §9.1 storage tests: index + tombstones (atomic save/load, missing → empty, prune).

use std::fs;

use chrono::{Duration, Utc};
use tempfile::tempdir;

use restask::domain::{ListSlug, TaskUid};
use restask::store::{Index, IndexEntry, Tombstones};

const UID1: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const UID2: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpc";

fn uid(raw: &str) -> TaskUid {
    TaskUid::parse(raw).unwrap()
}

fn entry(u: TaskUid, list: &str, path: &str, thumb: u64, etag: Option<&str>) -> IndexEntry {
    IndexEntry {
        uid: u,
        list: ListSlug::from_name(list).unwrap(),
        source_path: path.to_string(),
        thumbprint: thumb,
        caldav_etag: etag.map(str::to_string),
        seen_at: Utc::now(),
        defer_count: 0,
    }
}

#[test]
fn index_missing_file_loads_empty() {
    let dir = tempdir().unwrap();
    let index = Index::load(dir.path()).unwrap();
    assert!(index.entries.is_empty());
    // Loading from a directory that does not exist at all is also empty.
    let nested = dir.path().join("nope");
    assert!(Index::load(&nested).unwrap().entries.is_empty());
}

#[test]
fn index_save_load_roundtrip() {
    let dir = tempdir().unwrap();
    let mut index = Index::default();
    index.upsert(entry(
        uid(UID1),
        "home-lab",
        "Home.md",
        0xdead_beef,
        Some("\"abc\""),
    ));
    index.upsert(entry(uid(UID2), "Work", "Work/Todo.md", 42, None));

    index.save(dir.path()).unwrap();
    let loaded = Index::load(dir.path()).unwrap();
    assert_eq!(loaded, index);
    // Both entries present, keyed by UID, with etag and routing intact.
    let first = loaded.get(&uid(UID1)).unwrap();
    assert_eq!(first.list.as_str(), "home-lab");
    assert_eq!(first.source_path, "Home.md");
    assert_eq!(first.caldav_etag.as_deref(), Some("\"abc\""));
    assert_eq!(loaded.get(&uid(UID2)).unwrap().caldav_etag, None);
}

#[test]
fn index_save_is_atomic_and_rewritable() {
    let dir = tempdir().unwrap();
    let mut index = Index::default();
    index.upsert(entry(uid(UID1), "Home", "Home.md", 1, None));
    index.save(dir.path()).unwrap();

    // Second save replaces the file wholesale.
    let mut updated = Index::default();
    updated.upsert(entry(uid(UID2), "Work", "Work/Todo.md", 2, Some("\"e2\"")));
    updated.save(dir.path()).unwrap();

    let loaded = Index::load(dir.path()).unwrap();
    assert_eq!(loaded, updated);
    assert!(loaded.get(&uid(UID1)).is_none());

    // No tmp residue: the atomic write leaves only the state file behind.
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".restask-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "tmp residue: {leftovers:?}");
}

#[test]
fn index_upsert_get_remove() {
    let mut index = Index::default();
    assert!(index.get(&uid(UID1)).is_none());

    index.upsert(entry(uid(UID1), "Home", "Home.md", 7, None));
    assert_eq!(index.get(&uid(UID1)).unwrap().thumbprint, 7);

    // Upserting the same UID replaces the entry.
    index.upsert(entry(uid(UID1), "Home", "Home/Other.md", 9, Some("\"t\"")));
    let replaced = index.get(&uid(UID1)).unwrap();
    assert_eq!(replaced.source_path, "Home/Other.md");
    assert_eq!(index.entries.len(), 1);

    index.remove(&uid(UID1));
    assert!(index.get(&uid(UID1)).is_none());
    assert!(index.entries.is_empty());
    // Removing an unknown UID is a no-op.
    index.remove(&uid(UID2));
}

#[test]
fn index_corrupt_file_is_an_error() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path()).unwrap();
    fs::write(dir.path().join("index.json"), "{ not json").unwrap();
    let err = Index::load(dir.path()).unwrap_err();
    assert!(err.to_string().contains("index.json"), "{err}");
}

#[test]
fn tombstones_missing_file_loads_empty() {
    let dir = tempdir().unwrap();
    let tombstones = Tombstones::load(dir.path()).unwrap();
    assert!(!tombstones.contains(&uid(UID1)));
}

#[test]
fn tombstones_roundtrip_and_contains() {
    let dir = tempdir().unwrap();
    let now = Utc::now();
    let mut tombstones = Tombstones::default();
    tombstones.insert(uid(UID1), now);
    tombstones.insert(uid(UID2), now - Duration::hours(3));
    assert!(tombstones.contains(&uid(UID1)));
    assert!(tombstones.contains(&uid(UID2)));
    assert!(!tombstones.contains(&uid("restask-01arz3ndektsv4rrffq69g5fav")));

    tombstones.save(dir.path()).unwrap();
    let loaded = Tombstones::load(dir.path()).unwrap();
    assert!(loaded.contains(&uid(UID1)));
    assert!(loaded.contains(&uid(UID2)));
    assert!(!loaded.contains(&uid("restask-01arz3ndektsv4rrffq69g5fav")));
}

#[test]
fn tombstones_prune_old_entries() {
    let dir = tempdir().unwrap();
    let now = Utc::now();
    let mut tombstones = Tombstones::default();
    tombstones.insert(uid(UID1), now - Duration::days(400));
    tombstones.insert(uid(UID2), now - Duration::hours(1));

    tombstones.prune(Duration::days(365));
    assert!(
        !tombstones.contains(&uid(UID1)),
        "400-day-old tombstone must be pruned"
    );
    assert!(
        tombstones.contains(&uid(UID2)),
        "recent tombstone must survive"
    );

    // Survivors persist across save/load.
    tombstones.save(dir.path()).unwrap();
    let loaded = Tombstones::load(dir.path()).unwrap();
    assert!(loaded.contains(&uid(UID2)));
    assert!(!loaded.contains(&uid(UID1)));
}

#[test]
fn tombstones_prune_empty_is_noop() {
    let mut tombstones = Tombstones::default();
    tombstones.prune(Duration::days(365));
    assert!(!tombstones.contains(&uid(UID1)));
}
