//! §9.2 tests: CalDAV outbox (parked operations).

use tempfile::tempdir;

use restask::domain::{ListSlug, SourceRef, Status, Task, TaskUid};
use restask::store::{OutboundOp, Outbox};

const UID1: &str = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpb";
const UID2: &str = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpc";

fn uid(raw: &str) -> TaskUid {
    TaskUid::parse(raw).unwrap()
}

fn task_for(raw: &str) -> Task {
    Task {
        uid: uid(raw),
        list: ListSlug::from_name("home-lab").unwrap(),
        text: "Buy milk".to_string(),
        status: Status::Active,
        priority: None,
        due: None,
        start: None,
        scheduled: None,
        created: None,
        parent: None,
        source: SourceRef {
            path: "Home.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: chrono::Utc::now(),
        last_modified: chrono::Utc::now(),
    }
}

#[test]
fn outbox_missing_file_loads_empty() {
    let dir = tempdir().unwrap();
    let mut outbox = Outbox::load(dir.path()).unwrap();
    assert_eq!(outbox.take_all(), Vec::new());
}

#[test]
fn outbox_roundtrip_preserves_order_and_variants() {
    let dir = tempdir().unwrap();
    let mut outbox = Outbox::default();
    outbox.push(OutboundOp::Put {
        task: task_for(UID1),
    });
    outbox.push(OutboundOp::Delete {
        uid: uid(UID2),
        list: ListSlug::from_name("work").unwrap(),
        etag: Some("\"abc\"".to_string()),
    });
    outbox.push(OutboundOp::Delete {
        uid: uid(UID1),
        list: ListSlug::from_name("home-lab").unwrap(),
        etag: None,
    });

    outbox.save(dir.path()).unwrap();
    let loaded = Outbox::load(dir.path()).unwrap();
    assert_eq!(loaded, outbox);
}

#[test]
fn take_all_drains_in_fifo_order() {
    let mut outbox = Outbox::default();
    outbox.push(OutboundOp::Put {
        task: task_for(UID1),
    });
    outbox.push(OutboundOp::Delete {
        uid: uid(UID2),
        list: ListSlug::from_name("work").unwrap(),
        etag: None,
    });

    let drained = outbox.take_all();
    assert!(matches!(&drained[0], OutboundOp::Put { task } if task.uid == uid(UID1)));
    assert!(matches!(&drained[1], OutboundOp::Delete { uid: u, .. } if *u == uid(UID2)));

    // The queue is empty afterwards; taking again yields nothing.
    assert!(outbox.take_all().is_empty());
}

#[test]
fn outbox_save_overwrites_atomically() {
    let dir = tempdir().unwrap();
    let mut outbox = Outbox::default();
    outbox.push(OutboundOp::Put {
        task: task_for(UID1),
    });
    outbox.save(dir.path()).unwrap();

    // Replaying the queue empties it; saving again persists the empty state.
    outbox.take_all();
    outbox.save(dir.path()).unwrap();
    let mut loaded = Outbox::load(dir.path()).unwrap();
    assert_eq!(loaded.take_all(), Vec::new());
}
