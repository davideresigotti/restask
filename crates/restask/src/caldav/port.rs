//! The CalDAV port (§10.2): the only interface sync code uses to reach the server.
//! Static dispatch only (`C: CaldavPort` generics, no `dyn`); the reqwest implementation
//! lives in [`crate::caldav::client`], tests use `tests/common::MockCaldav`.

pub use crate::caldav::protocol::CollectionInfo;

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::domain::{ListSlug, Task};
use crate::vtodo::{RemoteTask, WireNames};
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

/// What one `REPORT` says a collection holds (§10.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    /// Every `VTODO` that could be read, in document order.
    pub resources: Vec<RemoteResource>,
    /// Names (sans `.ics`) of resources the collection lists but whose body could not
    /// be read as a `VTODO`. They exist: a task that may be one of them is unknown this
    /// pass, never deleted (§11.2).
    pub unreadable: Vec<String>,
}

/// Transport port to a CalDAV server (§10.2). One impl = one server account; collections
/// are addressed by their path segment below the account (`collection`), resources by
/// their `.ics`-sans-suffix name. For most lists the segment is the list's slug; which
/// collection a list is, is the caller's to say ([`crate::caldav::resolve_list`], §5.4).
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

    /// The whole remote snapshot of one collection in a single `REPORT` (§10.1): every
    /// `VTODO` with its etag and parsed body, read as tasks of `list`, in document order. `Ok(None)` means the collection
    /// does not exist. A resource that holds no parseable `VTODO` never fails the
    /// listing: it is logged and named in [`Listing::unreadable`], so that it is not
    /// taken for a resource that is gone.
    fn list_tasks(
        &self,
        collection: &str,
        list: &ListSlug,
    ) -> impl Future<Output = Result<Option<Listing>, RestaskError>> + Send;

    /// `PUT <url>/<user>/<collection>/<name>.ics`: serializes `task` (§8.1) with `extras` (the
    /// replaced resource's unmanaged content) and `now` as `DTSTAMP`/`LAST-MODIFIED`.
    /// `name` is the resource to write — the UID for a new resource, the listed name
    /// when replacing one (another client may have stored the task under its own name).
    /// `wire` holds the `UID`s to write where they are not restask's own (a task another
    /// client created keeps its `UID`).
    /// `if_match: Some(etag)` replaces exactly that version; `None` creates
    /// (`If-None-Match: *`). Returns the new etag (empty when the server sends none). A
    /// failed precondition surfaces as `CaldavErrorKind::Conflict` (§10.4).
    // Every precondition is an argument (the port is stateless), and each of these is one.
    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        task: &Task,
        collection: &str,
        name: &str,
        extras: &[String],
        wire: &WireNames,
        if_match: Option<&str>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<String, RestaskError>> + Send;

    /// `DELETE <url>/<user>/<collection>/<name>.ics` with `If-Match` when an etag is given.
    /// Deleting a resource that is already gone succeeds.
    fn delete(
        &self,
        collection: &str,
        name: &str,
        etag: Option<&str>,
    ) -> impl Future<Output = Result<(), RestaskError>> + Send;
}
