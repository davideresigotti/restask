//! The CalDAV port (§10.2): the only interface sync code uses to reach the server.
//! Static dispatch only (`C: CaldavPort` generics, no `dyn`); the reqwest implementation
//! lives in [`crate::caldav::client`], tests use `tests/common::MockCaldav`.

pub use crate::caldav::protocol::CollectionInfo;

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::domain::{ListSlug, Task};
use crate::vtodo::RemoteTask;
use crate::RestaskError;

/// One `VTODO` resource as the server currently holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteResource {
    /// Resource name (sans `.ics`), exactly as the server spells it in its `href`.
    pub name: String,
    /// Current etag — the `If-Match` precondition for replacing or deleting the resource.
    pub etag: String,
    /// The parsed body.
    pub task: RemoteTask,
}

/// Transport port to a CalDAV server (§10.2). One impl = one server account; collections
/// are addressed by [`ListSlug`], resources by their `.ics`-sans-suffix name.
///
/// The port is stateless: every precondition (`If-Match`) and every timestamp is passed
/// in by the caller, so a one-shot CLI run and the long-lived daemon behave identically.
///
/// Methods are written as RPITIT with an explicit `+ Send` bound (the desugaring of
/// `async fn` plus a Send future): the daemon and CLI spawn the reconciler, so futures
/// must be `Send`, while call sites keep the plain `.await` syntax.
pub trait CaldavPort: Clone + Send + Sync + 'static {
    /// `PROPFIND` depth 1 at `<url>/<user>/`: all collections of the account.
    fn list_collections(
        &self,
    ) -> impl Future<Output = Result<Vec<CollectionInfo>, RestaskError>> + Send;

    /// Ensures the collection for `slug` exists: `PROPFIND`, then `MKCOL` (VTODO-only,
    /// §10.1) when absent. Must be idempotent.
    fn ensure_collection(
        &self,
        slug: &ListSlug,
        display: &str,
    ) -> impl Future<Output = Result<(), RestaskError>> + Send;

    /// The whole remote snapshot of one list in a single `REPORT` (§10.1): every `VTODO`
    /// with its etag and parsed body, in document order. `Ok(None)` means the collection
    /// does not exist. Resources that hold no parseable `VTODO` are skipped with a
    /// warning — one odd resource never fails the listing.
    fn list_tasks(
        &self,
        slug: &ListSlug,
    ) -> impl Future<Output = Result<Option<Vec<RemoteResource>>, RestaskError>> + Send;

    /// `PUT <url>/<user>/<list>/<name>.ics`: serializes `task` (§8.1) with `extras` (the
    /// replaced resource's unmanaged content) and `now` as `DTSTAMP`/`LAST-MODIFIED`.
    /// `name` is the resource to write — the UID for a new resource, the listed name
    /// when replacing one (another client may have stored the task under its own name).
    /// `if_match: Some(etag)` replaces exactly that version; `None` creates
    /// (`If-None-Match: *`). Returns the new etag (empty when the server sends none). A
    /// failed precondition surfaces as `CaldavErrorKind::Conflict` (§10.4).
    fn put(
        &self,
        task: &Task,
        name: &str,
        extras: &[String],
        if_match: Option<&str>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<String, RestaskError>> + Send;

    /// `DELETE <url>/<user>/<slug>/<name>.ics` with `If-Match` when an etag is given.
    /// Deleting a resource that is already gone succeeds.
    fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> impl Future<Output = Result<(), RestaskError>> + Send;
}
