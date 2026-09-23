//! reqwest-backed [`CaldavPort`] implementation (§10.3–10.4).
//!
//! One client = one server account: `reqwest::Client` (rustls-tls only, ≤ 10 redirects)
//! plus an HTTP `Basic` header built from the configured password source. The header and
//! the password are never logged (§17). Collection URL shape: `{url}/{username}/{slug}/`.
//!
//! Retry budget (§10.4): network errors, `5xx` and `429` are retried with 1 s / 2 s / 4 s
//! backoff (4 attempts total). `401/403` are fatal for the cycle (`CaldavErrorKind::Auth`);
//! `412` on `PUT` returns `CaldavErrorKind::Conflict` immediately (no budget consumed) so
//! the engine can re-fetch and re-plan that UID.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{Local, Utc};
use reqwest::header::{ETAG, IF_MATCH, IF_NONE_MATCH};
use reqwest::{Client, Method, RequestBuilder, Response};

use crate::caldav::port::{CaldavPort, CollectionInfo};
use crate::caldav::protocol::{
    mkcol_body, parse_collections, parse_etags, propfind_collections_body, report_vtodo_etags,
};
use crate::domain::{ListSlug, Task};
use crate::vtodo::{from_vcalendar, to_vcalendar, RemoteTask};
use crate::{CaldavErrorKind, TaskresError};

/// Standard retry budget (§10.4): 1 s, 2 s, 4 s — four attempts total.
const DEFAULT_RETRY_DELAYS: [u64; 3] = [1, 2, 4];

/// The concrete [`CaldavPort`] over HTTP (§10.3). Cloneable; the etag memo is shared
/// between clones.
#[derive(Clone)]
pub struct CaldavClient {
    http: Client,
    base_url: String,
    username: String,
    password: Option<String>,
    delays: Vec<Duration>,
    tz: chrono::FixedOffset,
    /// Etags observed for managed resources, keyed by resource path
    /// (`/{user}/{slug}/{name}.ics`). Filled by `fetch`/`put` responses; `put` uses it to
    /// choose `If-Match` over `If-None-Match: *` (§10.2).
    etags: Arc<Mutex<HashMap<String, String>>>,
}

impl fmt::Debug for CaldavClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaldavClient")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .field("retry_delays", &self.delays)
            .finish_non_exhaustive()
    }
}

impl CaldavClient {
    /// Builds a client for `base_url` with the standard retry budget (§10.4).
    pub fn new(
        base_url: &str,
        username: String,
        password: Option<String>,
    ) -> Result<Self, TaskresError> {
        let delays = DEFAULT_RETRY_DELAYS
            .iter()
            .map(|secs| Duration::from_secs(*secs))
            .collect();
        Self::with_retry_delays(base_url, username, password, delays)
    }

    /// [`CaldavClient::new`] with an explicit retry schedule (attempt count is
    /// `delays.len() + 1`). Production uses [`DEFAULT_RETRY_DELAYS`]; hermetic tests pass
    /// zero-length delays.
    #[doc(hidden)]
    pub fn with_retry_delays(
        base_url: &str,
        username: String,
        password: Option<String>,
        delays: Vec<Duration>,
    ) -> Result<Self, TaskresError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|error| TaskresError::Caldav {
                kind: CaldavErrorKind::Network,
                status: None,
                detail: format!("http client build failed: {error}"),
            })?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            username,
            password,
            delays,
            tz: *Local::now().offset(),
            etags: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// `{url}/{username}/` — the PROPFIND root for [`CaldavPort::list_collections`].
    fn user_home_url(&self) -> String {
        format!("{}/{}/", self.base_url, self.username)
    }

    /// `{url}/{username}/{slug}/` (§10.3).
    fn collection_url(&self, slug: &ListSlug) -> String {
        format!("{}{}/", self.user_home_url(), slug.as_str())
    }

    /// `{url}/{username}/{slug}/{name}.ics` (§10.2).
    fn resource_url(&self, slug: &ListSlug, name: &str) -> String {
        format!("{}{}.ics", self.collection_url(slug), name)
    }

    /// Stable map key for the etag memo.
    fn resource_path(&self, slug: &ListSlug, name: &str) -> String {
        format!("/{}/{}/{}.ics", self.username, slug.as_str(), name)
    }

    fn lock_etags(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.etags.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn known_etag(&self, path: &str) -> Option<String> {
        self.lock_etags().get(path).cloned()
    }

    fn remember_etag(&self, path: &str, etag: &str) {
        self.lock_etags().insert(path.to_string(), etag.to_string());
    }

    fn forget_etag(&self, path: &str) {
        self.lock_etags().remove(path);
    }

    /// Adds the `Basic` header only when a password is configured (§17: with auth
    /// disabled the server must never see an empty-credential header).
    fn authenticate(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.password {
            Some(password) => builder.basic_auth(&self.username, Some(password)),
            None => builder,
        }
    }

    /// Sends `builder` under the retry budget (§10.4).
    async fn send(&self, builder: RequestBuilder) -> Result<Response, TaskresError> {
        let attempts = self.delays.len() + 1;
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                tokio::time::sleep(self.delays[attempt - 1]).await;
            }
            let Some(outgoing) = builder.try_clone() else {
                return Err(TaskresError::Caldav {
                    kind: CaldavErrorKind::Protocol,
                    status: None,
                    detail: "request is not replayable for retries".to_string(),
                });
            };
            let result = outgoing.send().await;
            let retryable = match &result {
                Ok(response) => is_retryable_status(response.status().as_u16()),
                Err(error) => is_retryable_transport_error(error),
            };
            let exhausted = attempt + 1 >= attempts;
            if retryable && !exhausted {
                attempt += 1;
                continue;
            }
            return match result {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if is_retryable_status(status) {
                        Err(TaskresError::Caldav {
                            kind: CaldavErrorKind::Network,
                            status: Some(status),
                            detail: format!("server error after retry budget: {status}"),
                        })
                    } else {
                        Ok(response)
                    }
                }
                Err(error) => Err(TaskresError::Caldav {
                    kind: CaldavErrorKind::Network,
                    status: None,
                    detail: error.to_string(),
                }),
            };
        }
    }

    /// Runs a WebDAV request with an optional `Depth` header and an XML body.
    async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<String>,
        depth: Option<u8>,
    ) -> Result<Response, TaskresError> {
        let mut builder = self.authenticate(self.http.request(method, url));
        if let Some(depth) = depth {
            builder = builder.header("Depth", depth.to_string());
        }
        if let Some(body) = body {
            builder = builder.header("Content-Type", "application/xml").body(body);
        }
        self.send(builder).await
    }

    /// Maps an unexpected HTTP status to the §12.1 taxonomy.
    fn status_error(status: u16, operation: &str) -> TaskresError {
        let kind = match status {
            401 | 403 => CaldavErrorKind::Auth,
            412 => CaldavErrorKind::Conflict,
            _ => CaldavErrorKind::Protocol,
        };
        TaskresError::Caldav {
            kind,
            status: Some(status),
            detail: format!("{operation} failed: HTTP {status}"),
        }
    }

    /// Requires one of `expected` statuses, then reads the body as text.
    async fn expect_body(
        response: Response,
        expected: &[u16],
        operation: &str,
    ) -> Result<String, TaskresError> {
        let status = response.status().as_u16();
        if !expected.contains(&status) {
            return Err(Self::status_error(status, operation));
        }
        response.text().await.map_err(|error| TaskresError::Caldav {
            kind: CaldavErrorKind::Network,
            status: None,
            detail: format!("{operation}: body read failed: {error}"),
        })
    }

    /// Reads the `ETag` response header; a missing one is a protocol violation because
    /// every later `If-Match` depends on it.
    fn response_etag(response: &Response, operation: &str) -> Result<String, TaskresError> {
        response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| TaskresError::Caldav {
                kind: CaldavErrorKind::Protocol,
                status: None,
                detail: format!("{operation}: response lacks an ETag header"),
            })
    }
}

/// `5xx` and `429` consume the retry budget (§10.4).
fn is_retryable_status(status: u16) -> bool {
    status == 429 || status >= 500
}

/// Transport failures worth retrying: connect/timeout plus mid-transfer body/decode
/// errors. Request-construction or redirect-policy failures are not retried.
fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_body() || error.is_decode()
}

/// Builds an HTTP method constant that is not among reqwest's built-ins.
fn webdav_method(name: &'static str) -> Result<Method, TaskresError> {
    Method::from_bytes(name.as_bytes()).map_err(|error| TaskresError::Validation {
        field: "method",
        reason: error.to_string(),
    })
}

impl CaldavPort for CaldavClient {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, TaskresError> {
        let response = self
            .request(
                webdav_method("PROPFIND")?,
                &self.user_home_url(),
                Some(propfind_collections_body()),
                Some(1),
            )
            .await?;
        // A missing user home (fresh server) simply has no collections yet.
        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        let body = Self::expect_body(response, &[207], "list_collections").await?;
        Ok(parse_collections(&body))
    }

    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), TaskresError> {
        let probe = self
            .request(
                webdav_method("PROPFIND")?,
                &self.collection_url(slug),
                Some(propfind_collections_body()),
                Some(0),
            )
            .await?;
        match probe.status().as_u16() {
            207 => Ok(()),
            401 | 403 => Err(Self::status_error(
                probe.status().as_u16(),
                "ensure_collection",
            )),
            404 => {
                let created = self
                    .request(
                        webdav_method("MKCOL")?,
                        &self.collection_url(slug),
                        Some(mkcol_body(display)),
                        None,
                    )
                    .await?;
                match created.status().as_u16() {
                    201 => Ok(()),
                    status => Err(Self::status_error(status, "ensure_collection MKCOL")),
                }
            }
            status => Err(Self::status_error(status, "ensure_collection PROPFIND")),
        }
    }

    async fn list_etags(&self, slug: &ListSlug) -> Result<Vec<(String, String)>, TaskresError> {
        let response = self
            .request(
                webdav_method("REPORT")?,
                &self.collection_url(slug),
                Some(report_vtodo_etags()),
                Some(1),
            )
            .await?;
        let body = Self::expect_body(response, &[207], "list_etags").await?;
        Ok(parse_etags(&body))
    }

    async fn fetch(
        &self,
        slug: &ListSlug,
        name: &str,
    ) -> Result<Option<(RemoteTask, String)>, TaskresError> {
        let response = self
            .request(Method::GET, &self.resource_url(slug, name), None, None)
            .await?;
        match response.status().as_u16() {
            404 => Ok(None),
            200 => {
                let etag = Self::response_etag(&response, "fetch")?;
                let body = response
                    .text()
                    .await
                    .map_err(|error| TaskresError::Caldav {
                        kind: CaldavErrorKind::Network,
                        status: None,
                        detail: format!("fetch: body read failed: {error}"),
                    })?;
                let remote = from_vcalendar(&body, self.tz, slug)?;
                self.remember_etag(&self.resource_path(slug, name), &etag);
                Ok(Some((remote, etag)))
            }
            status => Err(Self::status_error(status, "fetch")),
        }
    }

    async fn put(&self, task: &Task) -> Result<String, TaskresError> {
        let name = task.uid.as_str();
        let url = self.resource_url(&task.list, name);
        let path = self.resource_path(&task.list, name);
        let mut builder = self
            .authenticate(self.http.request(Method::PUT, &url))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .body(to_vcalendar(task, Utc::now()));
        match self.known_etag(&path) {
            Some(etag) => builder = builder.header(IF_MATCH, etag),
            None => builder = builder.header(IF_NONE_MATCH, "*"),
        }
        let response = self.send(builder).await?;
        match response.status().as_u16() {
            201 | 204 => {
                let etag = Self::response_etag(&response, "put")?;
                self.remember_etag(&path, &etag);
                Ok(etag)
            }
            // The memo (if any) is stale; the engine re-fetches and re-plans (§10.4).
            412 => {
                self.forget_etag(&path);
                Err(Self::status_error(412, "put"))
            }
            status => Err(Self::status_error(status, "put")),
        }
    }

    async fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), TaskresError> {
        let mut builder = self.authenticate(
            self.http
                .request(Method::DELETE, self.resource_url(slug, name)),
        );
        if let Some(etag) = etag {
            builder = builder.header(IF_MATCH, etag);
        }
        let response = self.send(builder).await?;
        match response.status().as_u16() {
            // 404 is success: the resource is already gone (idempotent cleanup).
            200 | 204 | 404 => {
                self.forget_etag(&self.resource_path(slug, name));
                Ok(())
            }
            status => Err(Self::status_error(status, "delete")),
        }
    }
}
