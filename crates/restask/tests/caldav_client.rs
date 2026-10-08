//! §10.2–10.4 client tests against an in-process blocking HTTP mock backed by
//! `std::net::TcpListener`: MKCOL / one-REPORT listing / PUT preconditions, 401/412/5xx
//! handling, retry budget, and Basic auth headers. Also covers the shared `MockCaldav`
//! fixture.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::NaiveDate;

use common::{sample_task, MockCaldav};
use restask::caldav::protocol::report_vtodos;
use restask::caldav::{CaldavClient, CaldavPort};
use restask::domain::{ListSlug, LocalDate, When};
use restask::vtodo::WireNames;
use restask::{CaldavErrorKind, RestaskError};

const UID_A: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const UID_B: &str = "restask-01jzq4tsvg2c9xkw7n5m8rhdpc";

/// Radicale-style depth-1 PROPFIND answer: user root, a VTODO calendar, an event calendar.
const COLLECTIONS_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
 <D:response>
  <D:href>/me/</D:href>
  <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:displayname>me</D:displayname></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/inbox/</D:href>
  <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:displayname>inbox</D:displayname><C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/university/</D:href>
  <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:displayname>university</D:displayname><C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
 </D:response>
</D:multistatus>
"#;

/// Radicale-style REPORT answer for two managed VTODOs.
const ETAGS_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
 <D:response>
  <D:href>/me/inbox/restask-01jzq4tsvg2c9xkw7n5m8rhdpb.ics</D:href>
  <D:propstat><D:prop><D:getetag>"etag-a"</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/inbox/restask-01jzq4tsvg2c9xkw7n5m8rhdpc.ics</D:href>
  <D:propstat><D:prop><D:getetag>"etag-b"</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
 </D:response>
</D:multistatus>
"#;

/// A recorded incoming request (header names lowercased for lookups).
#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A scripted outgoing response.
struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl RawResponse {
    fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    fn xml(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/xml".to_string())],
            body: body.to_string(),
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

type HandlerFn = Box<dyn Fn(&RecordedRequest) -> RawResponse + Send>;

struct TestServer {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

fn spawn_server(handler: HandlerFn) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            if let Some(request) = read_request(&mut stream) {
                let response = handler(&request);
                recorded.lock().unwrap().push(request);
                write_response(&mut stream, &response);
            }
        }
    });
    TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        requests,
    }
}

/// Reads one HTTP/1.1 request (headers + Content-Length body) from the stream.
fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .ok()?;
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = find(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    if headers
        .iter()
        .any(|(name, value)| name == "expect" && value.eq_ignore_ascii_case("100-continue"))
    {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    if body.len() < content_length {
        let mut remainder = vec![0u8; content_length - body.len()];
        stream.read_exact(&mut remainder).ok()?;
        body.extend_from_slice(&remainder);
    }
    Some(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_response(stream: &mut TcpStream, response: &RawResponse) {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        412 => "Precondition Failed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\nContent-Length: {}\r\n",
        response.status,
        reason,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(response.body.as_bytes());
    let _ = stream.flush();
}

/// Client with the standard 4-attempt budget but zero-length backoff (hermetic).
fn client(base_url: &str) -> CaldavClient {
    CaldavClient::with_retry_delays(
        base_url,
        "me".to_string(),
        Some("secret".to_string()),
        vec![Duration::ZERO; 3],
    )
    .unwrap()
}

fn slug(name: &str) -> ListSlug {
    ListSlug::from_name(name).unwrap()
}

/// Minimal Radicale-compatible VTODO body (date-only DUE keeps parsing device-tz-free).
fn vtodo(uid: &str, summary: &str) -> String {
    let mut body = String::new();
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Restask//EN",
        "BEGIN:VTODO",
        &format!("UID:{uid}"),
        &format!("SUMMARY:{summary}"),
        "STATUS:NEEDS-ACTION",
        "DTSTAMP:20260922T120000Z",
        "DUE;VALUE=DATE:20260925",
        "END:VTODO",
        "END:VCALENDAR",
    ] {
        body.push_str(line);
        body.push_str("\r\n");
    }
    body
}

#[tokio::test]
async fn ensure_collection_creates_missing_collection() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PROPFIND" => RawResponse::status(404),
        "MKCOL" => RawResponse::status(201),
        _ => RawResponse::status(500),
    }));
    client(&server.base_url)
        .ensure_collection(&slug("home-lab"), "Home Lab")
        .await
        .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "PROPFIND");
    assert_eq!(requests[0].path, "/me/home-lab/");
    assert_eq!(requests[0].header("depth"), Some("0"));
    assert_eq!(requests[1].method, "MKCOL");
    assert_eq!(requests[1].path, "/me/home-lab/");
    assert_eq!(requests[1].header("content-type"), Some("application/xml"));
    assert!(requests[1].body.contains("<C:comp name=\"VTODO\"/>"));
    assert!(requests[1]
        .body
        .contains("<D:displayname>Home Lab</D:displayname>"));
}

#[tokio::test]
async fn ensure_collection_present_is_noop() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PROPFIND" => RawResponse::xml(207, "<D:multistatus xmlns:D=\"DAV:\"/>"),
        _ => RawResponse::status(500),
    }));
    client(&server.base_url)
        .ensure_collection(&slug("inbox"), "Inbox")
        .await
        .unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn list_collections_parses_radicale_propfind() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PROPFIND" if request.path == "/me/" => RawResponse::xml(207, COLLECTIONS_XML),
        _ => RawResponse::status(500),
    }));
    let collections = client(&server.base_url).list_collections().await.unwrap();
    // The user home entry (`/me/`) is the principal container, not a calendar.
    assert_eq!(collections.len(), 2);
    assert_eq!(collections[0].slug, "inbox");
    assert!(collections[0].supports_vtodo);
    assert_eq!(collections[0].display_name.as_deref(), Some("inbox"));
    assert_eq!(collections[1].slug, "university");
    assert!(!collections[1].supports_vtodo);
}

#[tokio::test]
async fn list_collections_missing_user_home_is_empty() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PROPFIND" => RawResponse::status(404),
        _ => RawResponse::status(500),
    }));
    let collections = client(&server.base_url).list_collections().await.unwrap();
    assert!(collections.is_empty());
}

/// Escapes an iCalendar body the way a server inlines it into `calendar-data`.
fn xml_text(body: &str) -> String {
    body.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A REPORT answer inlining the bodies of two managed VTODOs.
fn report_xml() -> String {
    let item = |uid: &str, etag: &str, summary: &str| {
        format!(
            "<D:response><D:href>/me/inbox/{uid}.ics</D:href><D:propstat><D:prop>\
             <D:getetag>\"{etag}\"</D:getetag><C:calendar-data>{}</C:calendar-data>\
             </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
            xml_text(&vtodo(uid, summary))
        )
    };
    format!(
        "<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">{}{}</D:multistatus>",
        item(UID_A, "etag-a", "First & foremost"),
        item(UID_B, "etag-b", "Second")
    )
}

fn stamp() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-09-22T14:30:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

#[tokio::test]
async fn list_tasks_is_one_report_carrying_every_body() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "REPORT" if request.path == "/me/inbox/" => RawResponse::xml(207, &report_xml()),
        _ => RawResponse::status(500),
    }));
    let resources = client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].name, UID_A);
    assert_eq!(resources[0].etag, "\"etag-a\"");
    assert!(resources[0].task.managed);
    assert_eq!(resources[0].task.task.text, "First & foremost");
    assert_eq!(
        resources[0].task.task.due,
        Some(When::Date(LocalDate(
            NaiveDate::from_ymd_opt(2026, 9, 25).unwrap()
        )))
    );
    assert_eq!(resources[1].etag, "\"etag-b\"");
    // The whole snapshot of a list costs exactly one request.
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("depth"), Some("1"));
    assert_eq!(requests[0].body, report_vtodos());
}

#[tokio::test]
async fn list_tasks_reports_a_missing_collection_as_none() {
    let server = spawn_server(Box::new(|_request| RawResponse::status(404)));
    let listed = client(&server.base_url)
        .list_tasks("nowhere", &slug("nowhere"))
        .await
        .unwrap();
    assert!(listed.is_none());
}

#[tokio::test]
async fn list_tasks_fetches_bodies_the_server_did_not_inline() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "REPORT" => RawResponse::xml(207, ETAGS_XML),
        "GET" if request.path.ends_with(&format!("{UID_A}.ics")) => {
            RawResponse::xml(200, &vtodo(UID_A, "Fetched separately"))
        }
        // The second resource vanished between the REPORT and the GET.
        "GET" => RawResponse::status(404),
        _ => RawResponse::status(500),
    }));
    let resources = client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].task.task.text, "Fetched separately");
    assert_eq!(resources[0].etag, "\"etag-a\"");
}

#[tokio::test]
async fn list_tasks_skips_resources_without_a_vtodo() {
    let xml = format!(
        "<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
         <D:response><D:href>/me/inbox/junk.ics</D:href><D:propstat><D:prop>\
         <D:getetag>\"j\"</D:getetag><C:calendar-data>not a calendar</C:calendar-data>\
         </D:prop></D:propstat></D:response>\
         <D:response><D:href>/me/inbox/{UID_A}.ics</D:href><D:propstat><D:prop>\
         <D:getetag>\"a\"</D:getetag><C:calendar-data>{}</C:calendar-data>\
         </D:prop></D:propstat></D:response></D:multistatus>",
        xml_text(&vtodo(UID_A, "Fine"))
    );
    let server = spawn_server(Box::new(move |_request| RawResponse::xml(207, &xml)));
    let resources = client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        resources.len(),
        1,
        "one odd resource must not fail the listing"
    );
    assert_eq!(resources[0].name, UID_A);
}

#[tokio::test]
async fn put_replaces_exactly_the_version_the_caller_saw() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PUT" => RawResponse::status(204).header("ETag", "\"remote-2\""),
        _ => RawResponse::status(500),
    }));
    let extras = vec!["DESCRIPTION:kept".to_string()];
    let pushed = client(&server.base_url)
        .put(
            &sample_task(UID_A, "inbox", "Edited locally"),
            "inbox",
            UID_A,
            &extras,
            &WireNames::default(),
            Some("\"remote-1\""),
            stamp(),
        )
        .await
        .unwrap();
    assert_eq!(pushed, "\"remote-2\"");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, format!("/me/inbox/{UID_A}.ics"));
    assert_eq!(requests[0].header("if-match"), Some("\"remote-1\""));
    assert!(requests[0].header("if-none-match").is_none());
    assert_eq!(
        requests[0].header("content-type"),
        Some("text/calendar; charset=utf-8")
    );
    assert!(requests[0].body.contains(&format!("UID:{UID_A}\r\n")));
    // The caller's instant, not the wall clock, stamps the body.
    assert!(requests[0]
        .body
        .contains("LAST-MODIFIED:20260922T143000Z\r\n"));
    // Unmanaged content of the replaced resource is written back.
    assert!(requests[0].body.contains("DESCRIPTION:kept\r\n"));
}

#[tokio::test]
async fn put_create_uses_if_none_match_star() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PUT" => RawResponse::status(201).header("ETag", "\"fresh\""),
        _ => RawResponse::status(500),
    }));
    let etag = client(&server.base_url)
        .put(
            &sample_task(UID_A, "inbox", "New task"),
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            None,
            stamp(),
        )
        .await
        .unwrap();
    assert_eq!(etag, "\"fresh\"");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("if-none-match"), Some("*"));
    assert!(requests[0].header("if-match").is_none());
}

#[tokio::test]
async fn put_without_an_etag_header_still_succeeds() {
    let server = spawn_server(Box::new(|_request| RawResponse::status(201)));
    let etag = client(&server.base_url)
        .put(
            &sample_task(UID_A, "inbox", "New task"),
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            None,
            stamp(),
        )
        .await
        .unwrap();
    assert_eq!(etag, "");
}

#[tokio::test]
async fn put_precondition_conflict_without_retry() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PUT" => RawResponse::status(412),
        _ => RawResponse::status(500),
    }));
    let result = client(&server.base_url)
        .put(
            &sample_task(UID_A, "inbox", "local edit"),
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            Some("\"stale\""),
            stamp(),
        )
        .await;
    match result {
        Err(RestaskError::Caldav {
            kind: CaldavErrorKind::Conflict,
            status: Some(412),
            ..
        }) => {}
        other => panic!("expected a 412 conflict, got {other:?}"),
    }
    // The 412 is returned immediately: no retries.
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn auth_failure_is_fatal_and_unretried() {
    let server = spawn_server(Box::new(|request| match request.method.as_str() {
        "PROPFIND" => RawResponse::status(401),
        _ => RawResponse::status(500),
    }));
    let result = client(&server.base_url).list_collections().await;
    match result {
        Err(RestaskError::Caldav {
            kind: CaldavErrorKind::Auth,
            status: Some(401),
            ..
        }) => {}
        other => panic!("expected an auth error, got {other:?}"),
    }
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn server_errors_retry_and_recover() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let server = spawn_server(Box::new(move |_request| {
        if counter.fetch_add(1, Ordering::SeqCst) < 2 {
            RawResponse::status(500)
        } else {
            RawResponse::xml(207, &report_xml())
        }
    }));
    let resources = client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resources.len(), 2);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn server_errors_exhaust_retry_budget() {
    let server = spawn_server(Box::new(|_request| RawResponse::status(429)));
    let result = client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await;
    match result {
        Err(RestaskError::Caldav {
            kind: CaldavErrorKind::Network,
            status: Some(429),
            ..
        }) => {}
        other => panic!("expected a network error after the budget, got {other:?}"),
    }
    // 4 attempts total (1 s + 2 s + 4 s backoff, zeroed here).
    assert_eq!(server.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn network_errors_exhaust_retry_budget() {
    // Port 1 refuses every connection: four connect attempts, no HTTP traffic.
    let unreachable = CaldavClient::with_retry_delays(
        "http://127.0.0.1:1",
        "me".to_string(),
        Some("secret".to_string()),
        vec![Duration::ZERO; 3],
    )
    .unwrap();
    let result = unreachable.list_collections().await;
    match result {
        Err(RestaskError::Caldav {
            kind: CaldavErrorKind::Network,
            status: None,
            ..
        }) => {}
        other => panic!("expected a network error, got {other:?}"),
    }
}

#[tokio::test]
async fn basic_auth_header_is_sent() {
    let server = spawn_server(Box::new(|_request| RawResponse::status(404)));
    client(&server.base_url)
        .list_tasks("inbox", &slug("inbox"))
        .await
        .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests[0].header("authorization"),
        Some("Basic bWU6c2VjcmV0")
    );
}

#[tokio::test]
async fn no_password_omits_authorization_header() {
    let server = spawn_server(Box::new(|_request| RawResponse::status(404)));
    let anonymous = CaldavClient::with_retry_delays(
        &server.base_url,
        "me".to_string(),
        None,
        vec![Duration::ZERO; 3],
    )
    .unwrap();
    anonymous.list_tasks("inbox", &slug("inbox")).await.unwrap();
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].header("authorization").is_none());
}

#[tokio::test]
async fn delete_sends_if_match_and_tolerates_404() {
    let deletions = Arc::new(AtomicUsize::new(0));
    let counter = deletions.clone();
    let server = spawn_server(Box::new(move |request| match request.method.as_str() {
        "DELETE" if counter.fetch_add(1, Ordering::SeqCst) == 0 => RawResponse::status(204),
        "DELETE" => RawResponse::status(404),
        _ => RawResponse::status(500),
    }));
    let port_client = client(&server.base_url);
    port_client
        .delete("inbox", UID_A, Some("\"etag-a\""))
        .await
        .unwrap();
    port_client.delete("inbox", UID_A, None).await.unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("if-match"), Some("\"etag-a\""));
    assert!(requests[1].header("if-match").is_none());
}

#[tokio::test]
async fn mock_caldav_enforces_preconditions_like_a_server() {
    let mock = MockCaldav::new();
    let inbox = slug("inbox");
    let task = sample_task(UID_A, "inbox", "Mocked task");

    // No collection yet: listing says so, writing fails.
    assert!(mock
        .list_tasks(inbox.as_str(), &inbox)
        .await
        .unwrap()
        .is_none());
    assert!(mock
        .put(
            &task,
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            None,
            stamp()
        )
        .await
        .is_err());

    mock.seed_collection("inbox", "Inbox");
    let etag = mock
        .put(
            &task,
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            None,
            stamp(),
        )
        .await
        .unwrap();
    let listed = mock
        .list_tasks(inbox.as_str(), &inbox)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].name.as_str(), listed[0].etag.as_str()),
        (UID_A, etag.as_str())
    );
    assert_eq!(listed[0].task.task.text, "Mocked task");

    // Creating twice and replacing a stale version are both precondition failures.
    for stale in [None, Some("\"nope\"")] {
        match mock
            .put(
                &task,
                "inbox",
                UID_A,
                &[],
                &WireNames::default(),
                stale,
                stamp(),
            )
            .await
        {
            Err(RestaskError::Caldav {
                kind: CaldavErrorKind::Conflict,
                ..
            }) => {}
            other => panic!("expected a conflict, got {other:?}"),
        }
    }
    let rewritten = mock
        .put(
            &task,
            "inbox",
            UID_A,
            &[],
            &WireNames::default(),
            Some(&etag),
            stamp(),
        )
        .await
        .unwrap();
    assert_ne!(etag, rewritten);

    assert!(mock
        .delete(inbox.as_str(), UID_A, Some(&etag))
        .await
        .is_err());
    mock.delete(inbox.as_str(), UID_A, Some(&rewritten))
        .await
        .unwrap();
    mock.delete(inbox.as_str(), UID_A, None).await.unwrap();
    assert!(mock.resource_names("inbox").is_empty());
}

#[tokio::test]
async fn mock_caldav_scripted_failures_fire_once() {
    let mock = MockCaldav::new();
    let inbox = slug("inbox");
    mock.fail_next(CaldavErrorKind::Network);
    match mock.list_tasks(inbox.as_str(), &inbox).await {
        Err(RestaskError::Caldav {
            kind: CaldavErrorKind::Network,
            ..
        }) => {}
        other => panic!("expected the scripted network failure, got {other:?}"),
    }
    // The script is consumed: the next call succeeds again.
    assert!(mock.list_tasks(inbox.as_str(), &inbox).await.is_ok());
}
