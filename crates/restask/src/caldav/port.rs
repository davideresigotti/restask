//! The CalDAV port (§10.2): the only interface sync code uses to reach the server.
//! Static dispatch only (`C: CaldavPort` generics, no `dyn`); the reqwest implementation
//! lives in [`crate::caldav::client`], tests use `tests/common::MockCaldav`.

pub use crate::caldav::protocol::CollectionInfo;

use std::future::Future;

use crate::domain::{ListSlug, Task};
use crate::vtodo::RemoteTask;
use crate::RestaskError;

/// Transport port to a CalDAV server (§10.2). One impl = one server account; collections
/// are addressed by [`ListSlug`], resources by their `.ics`-sans-suffix name (the form
/// [`crate::caldav::protocol::parse_etags`] returns).
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

    /// `REPORT` calendar-query (§10.1): `(resource name, etag)` pairs of the collection's
    /// VTODOs, in document order.
    fn list_etags(
        &self,
        slug: &ListSlug,
    ) -> impl Future<Output = Result<Vec<(String, String)>, RestaskError>> + Send;

    /// `GET <url>/<user>/<slug>/<name>.ics`: `Ok(None)` when the resource is gone,
    /// otherwise the parsed task and its current etag.
    fn fetch(
        &self,
        slug: &ListSlug,
        name: &str,
    ) -> impl Future<Output = Result<Option<(RemoteTask, String)>, RestaskError>> + Send;

    /// `PUT <url>/<user>/<list>/<uid>.ics`: serializes `task` (§8.1) and stores it.
    /// `If-None-Match: *` on create, `If-Match: <etag>` when the etag is known; returns
    /// the new etag. A stale precondition surfaces as `CaldavErrorKind::Conflict` (§10.4).
    fn put(&self, task: &Task) -> impl Future<Output = Result<String, RestaskError>> + Send;

    /// `DELETE <url>/<user>/<slug>/<name>.ics` with `If-Match` when an etag is given.
    fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> impl Future<Output = Result<(), RestaskError>> + Send;
}
