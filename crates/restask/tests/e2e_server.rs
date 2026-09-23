//! Live end-to-end probe (T30) against the real Radicale server. Fully gated:
//! `#[ignore]` plus the `RESTASK_E2E_URL` / `RESTASK_E2E_USERNAME` /
//! `RESTASK_E2E_PASSWORD` environment variables. Only the dedicated `Taskres-Dev`
//! collection is touched — never a production list.
//!
//! Run with: `cargo test -p restask --test e2e_server -- --ignored`

use std::time::{SystemTime, UNIX_EPOCH};

use restask::caldav::{CaldavClient, CaldavPort};
use restask::domain::{ListSlug, Priority, SourceRef, Status, Task, TaskUid};

/// The dedicated development collection (slug `taskres-dev`).
const COLLECTION: &str = "Taskres-Dev";

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

fn probe_task(slug: &ListSlug, text: &str) -> Task {
    Task {
        uid: TaskUid::generate(),
        list: slug.clone(),
        text: text.to_string(),
        status: Status::Active,
        priority: Some(Priority::High),
        due: None,
        start: None,
        scheduled: None,
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

/// Full resource lifecycle on the live server: MKCOL (idempotent), PUT (If-None-Match),
/// REPORT (etag visible), GET (round-trip equality + etag echo), DELETE (If-Match),
/// and final absence.
#[tokio::test]
#[ignore = "live server probe; set RESTASK_E2E_URL/USERNAME/PASSWORD and run with -- --ignored"]
async fn e2e_server_taskres_dev_lifecycle() {
    let Some((client, slug)) = client_from_env() else {
        return;
    };

    // MKCOL is idempotent (PROPFIND first), so reruns are safe on the dev collection.
    client
        .ensure_collection(&slug, COLLECTION)
        .await
        .expect("ensure_collection failed");

    let task = probe_task(&slug, "Taskres e2e probe");
    let etag = client.put(&task).await.expect("put failed");
    assert!(!etag.is_empty(), "PUT must return a fresh etag");

    let pairs = client.list_etags(&slug).await.expect("list_etags failed");
    let name = pairs
        .iter()
        .find(|(name, _)| name == task.uid.as_str())
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| panic!("PUT resource `{}` not in REPORT", task.uid.as_str()));
    let listed_etag = pairs
        .iter()
        .find(|(name, _)| name == task.uid.as_str())
        .map(|(_, etag)| etag.clone())
        .unwrap();
    assert_eq!(etag, listed_etag, "REPORT etag must echo the PUT etag");

    let fetched = client
        .fetch(&slug, &name)
        .await
        .expect("fetch failed")
        .expect("resource must exist after PUT");
    let (remote, fetch_etag) = fetched;
    assert!(remote.managed, "round-tripped task must be managed");
    assert_eq!(remote.task.uid, task.uid, "UID is eternal");
    assert_eq!(remote.task.text, task.text);
    assert_eq!(remote.task.priority, task.priority);
    assert_eq!(remote.task.status, task.status);
    assert_eq!(fetch_etag, etag, "GET etag must be stable between writes");

    client
        .delete(&slug, &name, Some(&fetch_etag))
        .await
        .expect("delete failed");
    let gone = client.fetch(&slug, &name).await.expect("post-delete fetch");
    assert!(gone.is_none(), "resource must be gone after DELETE");
    let pairs = client
        .list_etags(&slug)
        .await
        .expect("post-delete list_etags");
    assert!(
        pairs.iter().all(|(name, _)| name != task.uid.as_str()),
        "deleted resource must not appear in REPORT"
    );
}
