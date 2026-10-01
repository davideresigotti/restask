# Restask Spec — CalDAV Protocol (§10)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §10 CalDAV Protocol (`src/caldav/`)

Grounded deployment: Radicale 3.x (tomsquest image) at `http://192.168.1.10:5232`, user path segment `me/`, filesystem storage `<data>/collections/collection-root/<user>/<collection>/<resource>.ics`. **Observed: authentication currently disabled — §17.**

### 10.1 `protocol.rs` (pure XML builders/parsers)

```rust
pub fn mkcol_body(display_name: &str) -> String;     // DAV:mkcol, resourcetype collection+calendar, comp VTODO
pub fn report_vtodo_etags() -> String;               // CALDAV:calendar-query, prop getetag, comp-filter VTODO (depth 1)
pub fn propfind_collections_body() -> String;        // prop: resourcetype, displayname, supported-calendar-component-set
pub fn parse_etags(xml: &str) -> Vec<(String, String)>;         // (resource name sans .ics, etag), document order
pub fn parse_collections(xml: &str) -> Vec<CollectionInfo>;     // namespace-prefix agnostic (local-name matching)
pub fn xml_unescape(s: &str) -> String;               // &amp; &lt; &gt; &quot; &apos;
```

`MKCOL` body sets `supported-calendar-component-set` to `VTODO` only and `displayname` to the list's display name.

### 10.2 `port.rs` (the trait; static dispatch — no `dyn`)

```rust
pub struct CollectionInfo { pub href: String, pub slug: String, pub display_name: Option<String>, pub supports_vtodo: bool }

pub trait CaldavPort: Clone + Send + Sync + 'static {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError>;          // PROPFIND depth 1 at <url>/<user>/
    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), RestaskError>; // PROPFIND; MKCOL if absent
    async fn list_etags(&self, slug: &ListSlug) -> Result<Vec<(String, String)>, RestaskError>;   // REPORT (10.1)
    async fn fetch(&self, slug: &ListSlug, name: &str) -> Result<Option<(RemoteTask, String)>, RestaskError>; // GET → (task, etag)
    async fn put(&self, task: &Task) -> Result<String, RestaskError>;     // PUT <url>/<user>/<list>/<uid>.ics; If-None-Match:* on create,
                                                                           // If-Match:<etag> when known; returns new etag
    async fn delete(&self, slug: &ListSlug, name: &str, etag: Option<&str>) -> Result<(), RestaskError>;
}
```

### 10.3 `client.rs`

`reqwest::Client` (rustls-tls only, redirects ≤ 10) + HTTP `Basic` auth header built from `PasswordSource` (§14). Never logs the header or password. Base URLs: collection URL = `{url}/{username}/{slug}/`.

### 10.4 Retry budget

Network errors, 5xx, 429 → backoff 1 s, 2 s, 4 s (4 attempts total). `401/403` → `CaldavErrorKind::Auth` (fatal for the cycle; daemon logs `auth_warning` and continues vault-side). `412` on PUT → re-fetch that resource and re-run the planner for that UID (does not consume budget). Exhausted ops are parked in the outbox with `retry_scheduled` and retried next cycle.

### 10.5 Foreign-resource rules (bound collections)

Bound collections (e.g. existing `university`, `inbox` event calendars) may hold foreign resources:
- **VEVENTs are never read, written, or deleted.** The `REPORT` comp-filter `VTODO` excludes them from listings entirely.
- Foreign **VTODOs** (UID not `restask-*`, e.g. created in Tasks.org inside the list) are **adopted** (§11 R5).
- Only resources whose name/UID matches `restask-*` are managed.
