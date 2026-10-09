//! Live end-to-end probe (T30) against the real Radicale server. Fully gated:
//! `#[ignore]` plus the `RESTASK_E2E_URL` / `RESTASK_E2E_USERNAME` /
//! `RESTASK_E2E_PASSWORD` environment variables. Only the dedicated `Restask-Dev`
//! collection is touched — never a production list.
//!
//! Run with: `cargo test -p restask --test e2e_server -- --ignored`

use std::time::{SystemTime, UNIX_EPOCH};

use restask::caldav::{CaldavClient, CaldavPort};
use restask::domain::{DeviceTag, ListSlug, Priority, SourceRef, Status, Task, TaskUid};
use restask::vtodo::WireNames;

/// The dedicated development collection (slug `restask-dev`).
const COLLECTION: &str = "Restask-Dev";

/// Reads a non-empty environment variable.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Wall-clock seconds since the epoch (mtimes only; the engine is not under test here).
fn now_unix() -> chrono::DateTime<chrono::Utc> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    chrono::DateTime::from_timestamp(now.as_secs() as i64, now.subsec_nanos()).unwrap()
}

/// Creates the live client from the environment, or `None` (with a skip note) when
/// the e2e variables are not configured.
fn client_from_env() -> Option<(CaldavClient, ListSlug)> {
    let url = match env("RESTASK_E2E_URL") {
        Some(url) => url,
        None => {
            eprintln!("skipping e2e: RESTASK_E2E_URL is not set");
            return None;
        }
    };
    let username = match env("RESTASK_E2E_USERNAME") {
        Some(username) => username,
        None => {
            eprintln!("skipping e2e: RESTASK_E2E_USERNAME is not set");
            return None;
        }
    };
    let password = env("RESTASK_E2E_PASSWORD");
    let client =
        CaldavClient::new(&url, username, password).expect("live CaldavClient::new failed");
    let slug = ListSlug::from_name(COLLECTION).expect("collection name must slugify");
    Some((client, slug))
}

/// A UID no earlier run left on the server: the number is the instant of the call.
fn probe_uid() -> TaskUid {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before the epoch")
        .as_nanos();
    let tag = DeviceTag::parse("e").expect("a device tag");
    TaskUid::minted(&tag, (nanos % 1_000_000_000_000_000) as u64)
}

fn probe_task(slug: &ListSlug, text: &str) -> Task {
    Task {
        uid: probe_uid(),
        list: slug.clone(),
        text: text.to_string(),
        status: Status::Active,
        priority: Some(Priority::High),
        due: None,
        start: None,
        scheduled: None,
        recurrence: None,
        created: None,
        parent: None,
        source: SourceRef {
            path: "e2e/probe.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: now_unix(),
        last_modified: now_unix(),
    }
}

/// Full resource lifecycle on the live server: MKCOL (idempotent), PUT (create,
/// `If-None-Match`), one REPORT carrying etag + body, PUT (replace, `If-Match`) keeping a
/// foreign property, a stale replace refused, DELETE (`If-Match`), and final absence.
#[tokio::test]
#[ignore = "live server probe; set RESTASK_E2E_URL/USERNAME/PASSWORD and run with -- --ignored"]
async fn e2e_server_restask_dev_lifecycle() {
    let Some((client, slug)) = client_from_env() else {
        return;
    };
    let find = |resources: Vec<restask::caldav::RemoteResource>, uid: &str| {
        resources
            .into_iter()
            .find(|resource| resource.task.raw_uid == uid)
    };

    // MKCOL is idempotent (PROPFIND first), so reruns are safe on the dev collection.
    client
        .ensure_collection(&slug, COLLECTION)
        .await
        .expect("ensure_collection failed");

    let mut task = probe_task(&slug, "restask e2e probe");
    let extras = vec!["DESCRIPTION:kept across pushes".to_string()];
    let created = client
        .put(
            &task,
            slug.as_str(),
            task.uid.as_str(),
            &extras,
            &WireNames::default(),
            None,
            now_unix(),
        )
        .await
        .expect("create failed");

    let listed = client
        .list_tasks(slug.as_str(), &slug)
        .await
        .expect("list_tasks failed")
        .expect("the collection exists");
    let remote = find(listed, task.uid.as_str())
        .unwrap_or_else(|| panic!("PUT resource `{}` not in REPORT", task.uid.as_str()));
    assert_eq!(remote.name, task.uid.as_str());
    assert!(!remote.etag.is_empty(), "REPORT must carry the etag");
    if !created.is_empty() {
        assert_eq!(created, remote.etag, "REPORT etag must echo the PUT etag");
    }
    assert!(remote.task.managed, "round-tripped task must be managed");
    assert_eq!(remote.task.task.uid, task.uid, "UID is eternal");
    assert_eq!(remote.task.task.text, task.text);
    assert_eq!(remote.task.task.priority, task.priority);
    assert_eq!(remote.task.task.status, task.status);
    assert_eq!(
        remote.task.extras, extras,
        "REPORT must inline the full body"
    );

    // Replace exactly that version, handing the foreign property back.
    task.text = "restask e2e probe (edited)".to_string();
    client
        .put(
            &task,
            slug.as_str(),
            &remote.name,
            &remote.task.extras,
            &WireNames::default(),
            Some(&remote.etag),
            now_unix(),
        )
        .await
        .expect("replace failed");
    // The old etag is now stale: a second replace with it must be refused.
    let stale = client
        .put(
            &task,
            slug.as_str(),
            &remote.name,
            &[],
            &WireNames::default(),
            Some(&remote.etag),
            now_unix(),
        )
        .await;
    assert!(stale.is_err(), "a stale If-Match must be refused");

    let listed = client
        .list_tasks(slug.as_str(), &slug)
        .await
        .unwrap()
        .unwrap();
    let replaced = find(listed, task.uid.as_str()).expect("still listed");
    assert_eq!(replaced.task.task.text, "restask e2e probe (edited)");
    assert_eq!(
        replaced.task.extras, extras,
        "foreign content survived the push"
    );
    assert_ne!(replaced.etag, remote.etag);

    client
        .delete(slug.as_str(), &replaced.name, Some(&replaced.etag))
        .await
        .expect("delete failed");
    let listed = client
        .list_tasks(slug.as_str(), &slug)
        .await
        .unwrap()
        .unwrap();
    assert!(
        find(listed, task.uid.as_str()).is_none(),
        "deleted resource must not appear in REPORT"
    );
}
