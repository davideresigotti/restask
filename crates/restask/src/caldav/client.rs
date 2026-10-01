//! reqwest-backed [`CaldavPort`] implementation (§10.3–10.4).
//!
//! One client = one server account: `reqwest::Client` (rustls-tls only, ≤ 10 redirects)
//! plus an HTTP `Basic` header built from the configured password source. The header and
//! the password are never logged (§17). Collection URL shape: `{url}/{username}/{slug}/`.
//!
//! Retry budget (§10.4): network errors, `5xx` and `429` are retried with 1 s / 2 s / 4 s
//! backoff (4 attempts total). `401/403` are fatal for the cycle (`CaldavErrorKind::Auth`);
//! `412` returns `CaldavErrorKind::Conflict` immediately (no budget consumed): the engine
//! leaves that task unsettled and the next cycle re-plans it from a fresh snapshot.
//!
//! The client holds no sync state: etags come from the caller (§10.2).

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use reqwest::header::{ETAG, IF_MATCH, IF_NONE_MATCH};
use reqwest::{Client, Method, RequestBuilder, Response};

use crate::caldav::port::{CaldavPort, CollectionInfo, RemoteResource};
use crate::caldav::protocol::{
    mkcol_body, parse_collections, parse_report, propfind_collections_body, report_vtodos,
};
use crate::domain::{ListSlug, Task};
use crate::vtodo::{from_vcalendar, to_vcalendar_with};
use crate::{CaldavErrorKind, RestaskError};

/// Standard retry budget (§10.4): 1 s, 2 s, 4 s — four attempts total.
const DEFAULT_RETRY_DELAYS: [u64; 3] = [1, 2, 4];

/// The concrete [`CaldavPort`] over HTTP (§10.3).
#[derive(Clone)]
pub struct CaldavClient {
    http: Client,
    base_url: String,
    username: String,
    password: Option<String>,
    delays: Vec<Duration>,
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
    ) -> Result<Self, RestaskError> {
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
    ) -> Result<Self, RestaskError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|error| RestaskError::Caldav {
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

    /// Adds the `Basic` header only when a password is configured (§17: with auth
    /// disabled the server must never see an empty-credential header).
    fn authenticate(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.password {
            Some(password) => builder.basic_auth(&self.username, Some(password)),
            None => builder,
        }
    }

    /// Sends `builder` under the retry budget (§10.4).
    async fn send(&self, builder: RequestBuilder) -> Result<Response, RestaskError> {
        let attempts = self.delays.len() + 1;
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                tokio::time::sleep(self.delays[attempt - 1]).await;
            }
            let Some(outgoing) = builder.try_clone() else {
                return Err(RestaskError::Caldav {
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
                        Err(RestaskError::Caldav {
                            kind: CaldavErrorKind::Network,
                            status: Some(status),
                            detail: format!("server error after retry budget: {status}"),
                        })
                    } else {
                        Ok(response)
                    }
                }
                Err(error) => Err(RestaskError::Caldav {
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
    ) -> Result<Response, RestaskError> {
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
    fn status_error(status: u16, operation: &str) -> RestaskError {
        let kind = match status {
            401 | 403 => CaldavErrorKind::Auth,
            412 => CaldavErrorKind::Conflict,
            _ => CaldavErrorKind::Protocol,
        };
        RestaskError::Caldav {
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
    ) -> Result<String, RestaskError> {
        let status = response.status().as_u16();
        if !expected.contains(&status) {
            return Err(Self::status_error(status, operation));
        }
        response.text().await.map_err(|error| RestaskError::Caldav {
            kind: CaldavErrorKind::Network,
            status: None,
            detail: format!("{operation}: body read failed: {error}"),
        })
    }

    /// Reads the `ETag` response header; empty when the server sends none (allowed by
    /// RFC 4791 when it stored a modified body — the next listing supplies the real one).
    fn response_etag(response: &Response) -> String {
        response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// `GET`s one resource body; `None` when it is gone.
    async fn get_body(&self, slug: &ListSlug, name: &str) -> Result<Option<String>, RestaskError> {
        let response = self
            .request(Method::GET, &self.resource_url(slug, name), None, None)
            .await?;
        match response.status().as_u16() {
            404 => Ok(None),
            200 => response
                .text()
                .await
                .map(Some)
                .map_err(|error| RestaskError::Caldav {
                    kind: CaldavErrorKind::Network,
                    status: None,
                    detail: format!("fetch: body read failed: {error}"),
                }),
            status => Err(Self::status_error(status, "fetch")),
        }
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
fn webdav_method(name: &'static str) -> Result<Method, RestaskError> {
    Method::from_bytes(name.as_bytes()).map_err(|error| RestaskError::Validation {
        field: "method",
        reason: error.to_string(),
    })
}

/// Path equality modulo trailing slashes (`/me/` == `/me`).
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

impl CaldavPort for CaldavClient {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError> {
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
        // A depth-1 PROPFIND always includes the queried user home itself (e.g. href
        // `/me/`): that entry is the principal container, never a bindable calendar
        // (§10.2), so it is dropped here — the pure parser stays prefix-agnostic.
        let home_path = reqwest::Url::parse(&self.user_home_url())
            .map(|url| url.path().trim_end_matches('/').to_string())
            .unwrap_or_default();
        Ok(parse_collections(&body)
            .into_iter()
            .filter(|collection| !same_path(&collection.href, &home_path))
            .collect())
    }

    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), RestaskError> {
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

    async fn list_tasks(
        &self,
        slug: &ListSlug,
    ) -> Result<Option<Vec<RemoteResource>>, RestaskError> {
        let response = self
            .request(
                webdav_method("REPORT")?,
                &self.collection_url(slug),
                Some(report_vtodos()),
                Some(1),
            )
            .await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let body = Self::expect_body(response, &[207], "list_tasks").await?;
        let mut resources = Vec::new();
        for item in parse_report(&body) {
            // Servers that do not inline calendar-data get one GET per resource.
            let data = if item.data.is_empty() {
                match self.get_body(slug, &item.name).await? {
                    Some(data) => data,
                    None => continue,
                }
            } else {
                item.data
            };
            // `Local` resolves the device offset valid at each instant (DST-correct, §4).
            match from_vcalendar(&data, &Local, slug) {
                Ok(task) => resources.push(RemoteResource {
                    name: item.name,
                    etag: item.etag,
                    task,
                }),
                Err(error) => {
                    tracing::warn!(list = %slug.as_str(), name = %item.name, %error, "unreadable remote resource skipped");
                }
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
        let url = self.resource_url(&task.list, name);
        let mut builder = self
            .authenticate(self.http.request(Method::PUT, &url))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .body(to_vcalendar_with(task, now, extras));
        builder = match if_match {
            Some(etag) => builder.header(IF_MATCH, etag),
            None => builder.header(IF_NONE_MATCH, "*"),
        };
        let response = self.send(builder).await?;
        match response.status().as_u16() {
            200 | 201 | 204 => Ok(Self::response_etag(&response)),
            status => Err(Self::status_error(status, "put")),
        }
    }

    async fn delete(
        &self,
        slug: &ListSlug,
        name: &str,
        etag: Option<&str>,
    ) -> Result<(), RestaskError> {
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
            200 | 204 | 404 => Ok(()),
            status => Err(Self::status_error(status, "delete")),
        }
    }
}
