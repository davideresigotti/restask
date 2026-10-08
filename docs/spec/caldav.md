# restask spec — CalDAV (§10)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §10 CalDAV (`crates/restask/src/caldav/`)

Target: Radicale 3.x. Collection URL: `{url}/{username}/{slug}/`; resource URL:
`{collection}{name}.ics`.

### 10.1 `protocol.rs` — pure XML builders and parsers

```rust
pub fn mkcol_body(display_name: &str) -> String;       // collection + calendar, VTODO only
pub fn propfind_collections_body() -> String;          // resourcetype, displayname, comp set, getctag
pub fn report_vtodos() -> String;                      // calendar-query: getetag + calendar-data, comp-filter VTODO
pub fn parse_collections(xml: &str) -> Vec<CollectionInfo>;   // { href, slug, display_name, supports_vtodo, ctag }
pub fn parse_report(xml: &str) -> Vec<ReportItem>;     // { name, etag, data }
pub fn xml_unescape(s: &str) -> String;                // the five entities + numeric references
```

`mkcol_body` is an extended `MKCOL` (RFC 5689): the properties are children of
`D:set` / `D:prop`. Without the `prop` element Radicale reads no calendar type from the
request, takes it for a plain folder and refuses it below a user's home (403).

Parsers match elements by local name (any prefix), tolerate malformed input without
panicking, and read `calendar-data` as entity-escaped text or `CDATA`. A resource `name`
is the last path segment of its `href` without `.ics`, kept exactly as the server spelled
it (percent-encoding included) so it can be reused in URLs.

### 10.2 `port.rs` — the trait (static dispatch)

```rust
pub struct RemoteResource { pub name: String, pub etag: String, pub task: RemoteTask }

pub trait CaldavPort: Clone + Send + Sync + 'static {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError>;
    async fn ensure_collection(&self, slug: &ListSlug, display: &str) -> Result<(), RestaskError>;
    async fn list_tasks(&self, collection: &str, list: &ListSlug) -> Result<Option<Vec<RemoteResource>>, RestaskError>;
    async fn put(&self, task: &Task, collection: &str, name: &str, extras: &[String], wire: &WireNames,
                 if_match: Option<&str>, now: DateTime<Utc>) -> Result<String, RestaskError>;
    async fn delete(&self, collection: &str, name: &str, etag: Option<&str>) -> Result<(), RestaskError>;
}
// `collection` is the path segment below the account, as the listing spells it. For
// most lists it is the list's slug; which collection a list is, the caller says
// (`caldav::resolve_list`, §5.4) — the port holds no mapping.
```

- **`list_collections`** also returns each collection's change tag (`CS:getctag`,
  `None` when the server reports none): an opaque value that changes with every write to
  the collection. The daemon's server watch compares it between two looks (§13.1);
  nothing else reads it.
- **`list_tasks`** is the whole remote snapshot of a list in **one `REPORT`**: every
  `VTODO` with its etag and body. `Ok(None)` = the collection does not exist. A resource
  with no parseable `VTODO` is skipped with a warning, never fatal. (A server that does
  not inline `calendar-data` costs one `GET` per resource; Radicale inlines.)
- **`put`** writes resource `name` — the UID for a new resource, the listed name when
  replacing one — with `wire`, the `UID`s to write where they are not restask's own
  (§8.1). `if_match: Some(etag)` replaces exactly that version; `None` creates
  (`If-None-Match: *`). Returns the new etag (empty if the server sends none).
- **`delete`** sends `If-Match` when given an etag; deleting what is already gone succeeds.
- The port is **stateless**: no etag memo, no clock. Preconditions come from the snapshot
  the plan was made from, timestamps from the engine's `Clock`.

Implementations: `client::CaldavClient` (reqwest, rustls), `offline::Offline` (every call
fails with a network error — for machines with no endpoint configured), and
`tests/common::MockCaldav` (in-memory; runs bodies through the real codec and enforces
preconditions like a server).

### 10.3 `client.rs`

`reqwest::Client` (rustls only, ≤ 10 redirects), HTTP Basic auth when a password is
configured. The password and the header are never logged.

### 10.4 Failures

- Network errors, `5xx`, `429` → retried with 1 s, 2 s, 4 s backoff (4 attempts), then
  `CaldavErrorKind::Network`.
- `401` / `403` → `CaldavErrorKind::Auth`, not retried.
- `412` → `CaldavErrorKind::Conflict`, not retried: the resource changed since it was
  listed.
- The engine treats Network/Tls/Auth as "the server is not usable now": the pass stops
  issuing requests and returns the error (the vault side of the pass is already done).
  Any other failure fails that one operation; it is counted in the report and **re-planned
  from fresh snapshots in the next pass**. There is no retry queue.

### 10.5 Shared collections

- `VEVENT`s are never listed, read, written or deleted (the `REPORT` filters on `VTODO`).
- Foreign `VTODO`s (UID not a restask UID) in a list that has a home in the vault are
  adopted (§11 R5); elsewhere they are left alone. An adopted task stays the resource
  its client created — same name, same `UID` — so that client keeps editing the task
  the vault line is tied to. restask writes to it like to its own (managed properties
  replaced, the rest handed back) and adds `X-RESTASK-SOURCE` and `X-RESTASK-UID`.
- Unmanaged content of any `VTODO` is preserved across writes (§8).
