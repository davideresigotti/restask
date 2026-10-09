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

    tombstones.prune(Duration::days(365), now);
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
    tombstones.prune(Duration::days(365), Utc::now());
    assert!(!tombstones.contains(&uid(UID1)));
}

#[test]
fn tombstones_remove_clears_a_marker() {
    let mut tombstones = Tombstones::default();
    tombstones.insert(uid(UID1), Utc::now());
    assert!(tombstones.remove(&uid(UID1)));
    assert!(!tombstones.remove(&uid(UID1)));
    assert_eq!(tombstones.uids().count(), 0);
}

#[test]
fn unchanged_state_files_are_not_rewritten() {
    // An idempotent sync pass must not bump mtimes: every rewrite is a file-sync event
    // on every device.
    let dir = tempdir().unwrap();
    let mut tombstones = Tombstones::default();
    tombstones.insert(uid(UID1), Utc::now());
    tombstones.save(dir.path()).unwrap();
    Index::default().save(dir.path()).unwrap();
    let stamp = |name: &str| {
        std::fs::metadata(dir.path().join(name))
            .unwrap()
            .modified()
            .unwrap()
    };
    let before = (stamp("tombstones.json"), stamp("index.json"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    tombstones.save(dir.path()).unwrap();
    Index::default().save(dir.path()).unwrap();
    assert_eq!(before, (stamp("tombstones.json"), stamp("index.json")));
}

// ---- wire names (§9.5) ----

#[test]
fn wires_roundtrip_and_say_what_is_news() {
    use restask::store::Wires;
    let dir = tempdir().unwrap();
    assert!(Wires::load(dir.path()).unwrap().is_empty());
    let mut wires = Wires::default();
    let counted = uid("restask-a1");
    assert!(wires.insert(UID1.to_string(), counted.clone()));
    assert!(
        !wires.insert(UID1.to_string(), counted.clone()),
        "known already"
    );
    assert!(wires.insert("5417@tasks.org".to_string(), uid("restask-a2")));
    wires.save(dir.path()).unwrap();
    let back = Wires::load(dir.path()).unwrap();
    assert_eq!(back, wires);
    assert_eq!(back.get(UID1), Some(&counted));
    assert_eq!(back.get("unknown"), None);
    assert_eq!(back.uids().count(), 2);
    // Unchanged content is not written again.
    let path = dir.path().join("wires.json");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    wires.save(dir.path()).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

// ---- device identity (§9.4) ----

mod device {
    use std::collections::BTreeSet;
    use std::fs;

    use tempfile::tempdir;

    use restask::domain::DeviceTag;
    use restask::store::{device_file, Device};

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn a_device_claims_a_tag_once_and_counts_on_from_where_it_was() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("vault/.restask");
        let file = device_file(&dir.path().join("config/config.toml"));
        assert_eq!(file, dir.path().join("config/device"));

        let mut device = Device::open(&state, Some(&file), None, &none()).unwrap();
        let tag = device.tag().clone();
        assert_eq!(tag.as_str().len(), 1, "the shortest free tag");
        // The claim is in the vault, where the file sync carries it to every device;
        // what makes it this device's is kept beside the machine config.
        let claim = fs::read_to_string(state.join("devices").join(tag.as_str())).unwrap();
        let identity = fs::read_to_string(&file).unwrap();
        assert_eq!(identity, format!("{tag} {} 0\n", claim.trim()));

        let ids = device.counter();
        assert_eq!(ids.mint().token(), format!("{tag}1"));
        assert_eq!(ids.mint().token(), format!("{tag}2"));
        assert!(device.used(&ids, Some(&file)).unwrap());
        assert!(!device.used(&ids, Some(&file)).unwrap(), "nothing new");

        // The next run is the same device, and never hands a number out again.
        let again = Device::open(&state, Some(&file), None, &none()).unwrap();
        assert_eq!(again, device);
        assert_eq!(again.counter().mint().token(), format!("{tag}3"));
    }

    #[test]
    fn no_two_devices_of_a_vault_get_the_same_tag() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        let mut tags = BTreeSet::new();
        for n in 0..26 {
            let file = dir.path().join(format!("device-{n}"));
            let device = Device::open(&state, Some(&file), None, &none()).unwrap();
            assert_eq!(device.tag().as_str().len(), 1);
            assert!(tags.insert(device.tag().clone()), "{} twice", device.tag());
        }
        // Every letter is taken: the next device gets two.
        let file = dir.path().join("device-26");
        let device = Device::open(&state, Some(&file), None, &none()).unwrap();
        assert_eq!(device.tag().as_str().len(), 2);
    }

    #[test]
    fn a_tag_that_uids_of_the_vault_carry_is_not_taken() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        // The state directory was emptied: the claims are gone, the UIDs are not.
        let taken: BTreeSet<String> = DeviceTag::all(1)
            .into_iter()
            .map(|tag| tag.as_str().to_string())
            .filter(|tag| tag != "q")
            .collect();
        let device = Device::open(&state, Some(&dir.path().join("device")), None, &taken).unwrap();
        assert_eq!(device.tag().as_str(), "q");
    }

    #[test]
    fn a_device_whose_claim_another_device_holds_takes_another_tag() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        let file = dir.path().join("device");
        let mut device = Device::open(&state, Some(&file), None, &none()).unwrap();
        let ids = device.counter();
        ids.mint();
        device.used(&ids, Some(&file)).unwrap();
        let lost = device.tag().clone();

        // Two devices took the tag before they saw each other; the file sync kept the
        // other one's claim.
        let claim = state.join("devices").join(lost.as_str());
        fs::write(&claim, "someone-else\n").unwrap();
        let device = Device::open(&state, Some(&file), None, &none()).unwrap();
        assert_ne!(device.tag(), &lost);
        assert_eq!(
            device.counter().last(),
            0,
            "a new tag counts from the start"
        );
        assert_eq!(fs::read_to_string(&claim).unwrap(), "someone-else\n");
        // And it stays with the new one.
        assert_eq!(
            Device::open(&state, Some(&file), None, &none()).unwrap(),
            device
        );
    }

    #[test]
    fn a_claim_that_is_gone_is_made_again() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        let file = dir.path().join("device");
        let device = Device::open(&state, Some(&file), None, &none()).unwrap();
        fs::remove_dir_all(&state).unwrap();
        let again = Device::open(&state, Some(&file), None, &none()).unwrap();
        assert_eq!(again, device);
        assert!(state.join("devices").join(device.tag().as_str()).is_file());
    }

    #[test]
    fn without_a_file_the_identity_is_the_one_that_is_handed_back() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        let first = Device::open(&state, None, None, &none()).unwrap();
        let again = Device::open(&state, None, Some(first.clone()), &none()).unwrap();
        assert_eq!(again, first);
        // Nobody handing it back is another device.
        let other = Device::open(&state, None, None, &none()).unwrap();
        assert_ne!(other.tag(), first.tag());
    }

    #[test]
    fn an_identity_file_that_is_not_one_is_replaced() {
        let dir = tempdir().unwrap();
        let state = dir.path().join(".restask");
        let file = dir.path().join("device");
        fs::write(&file, "not an identity").unwrap();
        let device = Device::open(&state, Some(&file), None, &none()).unwrap();
        assert!(fs::read_to_string(&file)
            .unwrap()
            .starts_with(&format!("{} ", device.tag())));
    }
}
