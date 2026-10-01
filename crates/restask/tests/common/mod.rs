//! Shared integration-test fixtures (T17+): a fixed [`Clock`], an in-memory
//! [`CaldavPort`] mock, and a temporary routed vault. Individual test binaries use
//! different subsets, so unused helpers are tolerated here.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, FixedOffset, Offset as _, TimeZone, Utc};
use tempfile::TempDir;

use restask::caldav::{CaldavPort, CollectionInfo, RemoteResource};
use restask::domain::{Clock, ListSlug, LocalDate, Task, TaskUid};
use restask::markdown::MARKER;
use restask::vtodo::{from_vcalendar, to_vcalendar_with};
use restask::{CaldavErrorKind, RestaskError};

/// Fixed-instant clock (§3.3): `now_utc` is the first field, the device-local offset the
/// second, so `today_local` is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(pub DateTime<Utc>, pub FixedOffset);

impl Clock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.0
    }

    fn today_local(&self) -> LocalDate {
        LocalDate(self.0.with_timezone(&self.1).date_naive())
    }

    fn local_offset(&self) -> FixedOffset {
        self.1
    }
}

/// One stored resource in the mock server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockResource {
    /// Raw iCalendar body.
    pub body: String,
    /// Quoted etag (changes on every `put`).
    pub etag: String,
}

#[derive(Debug, Default)]
struct MockState {
    collections: BTreeMap<String, String>,
    resources: BTreeMap<(String, String), MockResource>,
    etag_counter: u64,
    failures: VecDeque<CaldavErrorKind>,
    puts: usize,
    deletes: usize,
    reports: usize,
}

/// In-memory [`CaldavPort`] stand-in behaving like a correct Radicale: bodies go through
/// the real codec both ways, etags change on every write, `If-Match`/`If-None-Match`
/// preconditions are enforced, and writing into a missing collection fails. Operations
/// scripted via [`MockCaldav::fail_next`] fail once each, in call order.
#[derive(Debug, Clone, Default)]
pub struct MockCaldav {
    state: Arc<Mutex<MockState>>,
}

impl MockCaldav {
    /// Empty server (no collections, no resources, no scripted failures).
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-creates a collection, as if bound by the setup wizard.
    pub fn seed_collection(&self, slug: &str, display: &str) {
        self.lock()
            .collections
            .insert(slug.to_string(), display.to_string());
    }

    /// Stores a resource the way another CalDAV client would (fresh etag); the
    /// collection is created when missing.
    pub fn seed_resource(&self, slug: &str, name: &str, body: &str) {
        let etag = self.next_etag();
        let mut state = self.lock();
        state
            .collections
            .entry(slug.to_string())
            .or_insert_with(|| slug.to_string());
        state.resources.insert(
            (slug.to_string(), name.to_string()),
            MockResource {
                body: body.to_string(),
                etag,
            },
        );
    }

    /// Removes a resource the way another CalDAV client would.
    pub fn remove_resource(&self, slug: &str, name: &str) {
        self.lock()
            .resources
            .remove(&(slug.to_string(), name.to_string()));
    }

    /// Scripts the next operation of any kind to fail with `kind` (§10.4 paths).
    pub fn fail_next(&self, kind: CaldavErrorKind) {
        self.lock().failures.push_back(kind);
    }

    /// Current stored resource, if any.
    pub fn resource(&self, slug: &str, name: &str) -> Option<MockResource> {
        self.lock()
            .resources
            .get(&(slug.to_string(), name.to_string()))
            .cloned()
    }

    /// Slugs of all known collections, sorted.
    pub fn collection_names(&self) -> Vec<String> {
        self.lock().collections.keys().cloned().collect()
    }

    /// Resource names of one collection (sans `.ics`), sorted.
    pub fn resource_names(&self, slug: &str) -> Vec<String> {
        self.lock()
            .resources
            .keys()
            .filter(|(list, _)| list == slug)
            .map(|(_, name)| name.clone())
            .collect()
    }

    /// `(puts, deletes, reports)` served so far.
    pub fn counters(&self) -> (usize, usize, usize) {
        let state = self.lock();
        (state.puts, state.deletes, state.reports)
    }

    fn lock(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_etag(&self) -> String {
        let mut state = self.lock();
        state.etag_counter += 1;
        format!("\"mock-{}\"", state.etag_counter)
    }

    fn scripted_failure(&self) -> Option<RestaskError> {
        self.lock()
            .failures
            .pop_front()
            .map(|kind| RestaskError::Caldav {
                kind,
                status: None,
                detail: "scripted mock failure".to_string(),
            })
    }
}

fn mock_error(kind: CaldavErrorKind, status: u16, detail: &str) -> RestaskError {
    RestaskError::Caldav {
        kind,
        status: Some(status),
        detail: detail.to_string(),
    }
}

impl CaldavPort for MockCaldav {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError> {
        if let Some(error) = self.scripted_failure() {
            return Err(error);
        }
        Ok(self
            .lock()
            .collections
            .iter()
            .map(|(slug, display)| CollectionInfo {
                href: format!("/{slug}/"),
                slug: slug.clone(),
                display_name: Some(display.clone()),
                supports_vtodo: true,
            })
            .collect())
    }

    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), RestaskError> {
        if let Some(error) = self.scripted_failure() {
            return Err(error);
        }
        self.lock()
            .collections
            .entry(slug.as_str().to_string())
            .or_insert_with(|| display.to_string());
        Ok(())
    }

    async fn list_tasks(
        &self,
        slug: &ListSlug,
    ) -> Result<Option<Vec<RemoteResource>>, RestaskError> {
        if let Some(error) = self.scripted_failure() {
            return Err(error);
        }
        let mut state = self.lock();
        state.reports += 1;
        if !state.collections.contains_key(slug.as_str()) {
            return Ok(None);
        }
        let mut resources = Vec::new();
        for ((list, name), resource) in &state.resources {
            if list != slug.as_str() {
                continue;
            }
            // Like the real client: a body without a VTODO is skipped, never fatal.
            if let Ok(task) = from_vcalendar(&resource.body, &Utc.fix(), slug) {
                resources.push(RemoteResource {
                    name: name.clone(),
                    etag: resource.etag.clone(),
                    task,
                });
            }
        }
        Ok(Some(resources))
    }

    async fn put(
        &self,
        task: &Task,
        name: &str,
        extras: &[String],
        if_match: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<String, RestaskError> {
        if let Some(error) = self.scripted_failure() {
            return Err(error);
        }
        let etag = self.next_etag();
        let mut state = self.lock();
        let key = (task.list.as_str().to_string(), name.to_string());
        if !state.collections.contains_key(&key.0) {
            return Err(mock_error(
                CaldavErrorKind::Protocol,
                409,
                "put into a missing collection",
            ));
        }
        match (if_match, state.resources.get(&key)) {
            (Some(expected), Some(current)) if current.etag == expected => {}
            (None, None) => {}
            _ => {
                return Err(mock_error(
                    CaldavErrorKind::Conflict,
                    412,
                    "precondition failed",
                ))
            }
        }
        state.puts += 1;
        state.resources.insert(
            key,
            MockResource {
                body: to_vcalendar_with(task, now, extras),
                etag: etag.clone(),
            },
        );
        Ok(etag)
    }

    async fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), RestaskError> {
        if let Some(error) = self.scripted_failure() {
            return Err(error);
        }
        let mut state = self.lock();
        let key = (slug.as_str().to_string(), name.to_string());
        if let (Some(expected), Some(current)) = (etag, state.resources.get(&key)) {
            if current.etag != expected {
                return Err(mock_error(
                    CaldavErrorKind::Conflict,
                    412,
                    "precondition failed",
                ));
            }
        }
        if state.resources.remove(&key).is_some() {
            state.deletes += 1;
        }
        Ok(())
    }
}

/// Creates a temporary vault with a routed inbox (`TODO.md`) and a default
/// `restask.toml`; the caller keeps the [`TempDir`] alive for the test's duration.
pub fn temp_vault() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("restask.toml"), "done_heading = \"Done\"\n").unwrap();
    write_vault_file(
        &dir,
        "TODO.md",
        &format!("---\nrestask-list: inbox\n---\n\n{MARKER}\n\n# TODO\n\n## Inbox\n"),
    );
    dir
}

/// Writes (or overwrites) a file inside the temporary vault.
pub fn write_vault_file(vault: &TempDir, relative: &str, contents: &str) {
    let path: &Path = &vault.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// Builds a minimal valid [`Task`] for tests, with fixed timestamps and source.
pub fn sample_task(uid: &str, list: &str, text: &str) -> Task {
    let stamp = Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap();
    Task {
        uid: TaskUid::parse(uid).unwrap(),
        list: ListSlug::from_name(list).unwrap(),
        text: text.to_string(),
        status: restask::domain::Status::Active,
        priority: None,
        due: None,
        start: None,
        scheduled: None,
        recurrence: None,
        created: None,
        parent: None,
        source: restask::domain::SourceRef {
            path: "TODO.md".to_string(),
            line: 1,
        },
        source_heading: None,
        source_mtime: stamp,
        last_modified: stamp,
    }
}
