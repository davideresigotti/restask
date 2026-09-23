//! §10.1 protocol tests: XML builders, Radicale-style multistatus parsing (etags,
//! collections), namespace-prefix agnostic scanning, and entity unescaping.

use restask::caldav::protocol::{
    mkcol_body, parse_collections, parse_etags, propfind_collections_body, report_vtodo_etags,
    xml_unescape,
};

#[test]
fn mkcol_body_requests_vtodo_only_collection() {
    let body = mkcol_body("Home Lab");
    assert_eq!(
        body,
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:mkcol xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:resourcetype>
      <D:collection/>
      <C:calendar/>
    </D:resourcetype>
    <D:displayname>Home Lab</D:displayname>
    <C:supported-calendar-component-set>
      <C:comp name="VTODO"/>
    </C:supported-calendar-component-set>
  </D:set>
</D:mkcol>
"#
    );
}

#[test]
fn mkcol_body_escapes_display_name() {
    let body = mkcol_body(r#"A & <B> "C""#);
    assert!(body.contains(r#"<D:displayname>A &amp; &lt;B&gt; &quot;C&quot;</D:displayname>"#));
}

#[test]
fn report_body_targets_vtodo_components() {
    let body = report_vtodo_etags();
    assert_eq!(
        body,
        r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VTODO"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
"#
    );
}

#[test]
fn propfind_body_requests_collection_properties() {
    let body = propfind_collections_body();
    assert_eq!(
        body,
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
    <D:displayname/>
    <C:supported-calendar-component-set/>
  </D:prop>
</D:propfind>
"#
    );
}

/// Radicale-style REPORT response: two resources plus one 404 propstat without an etag.
const ETAGS_MULTISTATUS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
 <D:response>
  <D:href>/me/inbox/taskres-01jzq4tsvg2c9xkw7n5m8rhdpb.ics</D:href>
  <D:propstat>
   <D:prop><D:getetag>"660d02eccf4b0b45"</D:getetag></D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/inbox/taskres-01jzq4tsvg2c9xkw7n5m8rhdpc.ics</D:href>
  <D:propstat>
   <D:prop><D:getetag>"aaaa1111bbbb2222"</D:getetag></D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/inbox/missing.ics</D:href>
  <D:propstat>
   <D:prop/>
   <D:status>HTTP/1.1 404 Not Found</D:status>
  </D:propstat>
 </D:response>
</D:multistatus>
"#;

#[test]
fn parse_etags_extracts_resources_in_document_order() {
    let etags = parse_etags(ETAGS_MULTISTATUS);
    assert_eq!(
        etags,
        vec![
            (
                "taskres-01jzq4tsvg2c9xkw7n5m8rhdpb".to_string(),
                "\"660d02eccf4b0b45\"".to_string()
            ),
            (
                "taskres-01jzq4tsvg2c9xkw7n5m8rhdpc".to_string(),
                "\"aaaa1111bbbb2222\"".to_string()
            ),
        ]
    );
}

#[test]
fn parse_etags_is_namespace_prefix_agnostic() {
    // Default (unprefixed) namespace.
    let prefixless = r#"<multistatus xmlns="DAV:">
  <response><href>/u/inbox/a.ics</href><propstat><prop><getetag>"z"</getetag></prop></propstat></response>
</multistatus>"#;
    assert_eq!(
        parse_etags(prefixless),
        vec![("a".to_string(), "\"z\"".to_string())]
    );

    // A different prefix on every element must parse identically.
    let other_prefix = r#"<d:multistatus xmlns:d="DAV:">
  <d:response><d:href>/u/inbox/b.ics</d:href><d:propstat><d:prop><d:getetag>"y"</d:getetag></d:prop></d:propstat></d:response>
</d:multistatus>"#;
    assert_eq!(
        parse_etags(other_prefix),
        vec![("b".to_string(), "\"y\"".to_string())]
    );
}

#[test]
fn parse_etags_skips_incomplete_responses() {
    let partial = r#"<D:multistatus xmlns:D="DAV:">
 <D:response><D:propstat><D:prop><D:getetag>"orphan"</D:getetag></D:prop></D:propstat></D:response>
 <D:response><D:href>/u/inbox/no-etag.ics</D:href><D:propstat><D:prop/></D:propstat></D:response>
 <D:response><D:href></D:href><D:propstat><D:prop><D:getetag>"empty"</D:getetag></D:prop></D:propstat></D:response>
 <D:response><D:href>/u/inbox/ok.ics</D:href><D:propstat><D:prop><D:getetag>"kept"</D:getetag></D:prop></D:propstat></D:response>
</D:multistatus>"#;
    assert_eq!(
        parse_etags(partial),
        vec![("ok".to_string(), "\"kept\"".to_string())]
    );
}

/// Radicale-style depth-1 PROPFIND response: the user root, a VTODO calendar, a mixed
/// VEVENT+VTODO calendar, an address book, and a plain resource file.
const COLLECTIONS_MULTISTATUS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
 <D:response>
  <D:href>/me/</D:href>
  <D:propstat>
   <D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
    <D:displayname>me</D:displayname>
   </D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/inbox/</D:href>
  <D:propstat>
   <D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
    <D:displayname>inbox</D:displayname>
    <C:supported-calendar-component-set><C:comp name="VTODO"/></C:supported-calendar-component-set>
   </D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/university/</D:href>
  <D:propstat>
   <D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
    <D:displayname>University &amp; Co</D:displayname>
    <C:supported-calendar-component-set><C:comp name="VEVENT"/><C:comp name="VTODO"/></C:supported-calendar-component-set>
   </D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/contacts/</D:href>
  <D:propstat>
   <D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
   </D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
 <D:response>
  <D:href>/me/contacts.vcf</D:href>
  <D:propstat>
   <D:prop>
    <D:resourcetype/>
   </D:prop>
   <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
 </D:response>
</D:multistatus>
"#;

#[test]
fn parse_collections_classifies_radicale_propfind() {
    let collections = parse_collections(COLLECTIONS_MULTISTATUS);
    assert_eq!(
        collections,
        vec![
            restask::caldav::protocol::CollectionInfo {
                href: "/me/".to_string(),
                slug: "me".to_string(),
                display_name: Some("me".to_string()),
                supports_vtodo: false,
            },
            restask::caldav::protocol::CollectionInfo {
                href: "/me/inbox/".to_string(),
                slug: "inbox".to_string(),
                display_name: Some("inbox".to_string()),
                supports_vtodo: true,
            },
            restask::caldav::protocol::CollectionInfo {
                href: "/me/university/".to_string(),
                slug: "university".to_string(),
                display_name: Some("University & Co".to_string()),
                supports_vtodo: true,
            },
            restask::caldav::protocol::CollectionInfo {
                href: "/me/contacts/".to_string(),
                slug: "contacts".to_string(),
                display_name: None,
                supports_vtodo: false,
            },
        ]
    );
}

#[test]
fn parse_collections_is_namespace_prefix_agnostic() {
    let prefixless = r#"<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>/u/inbox/</href>
    <propstat><prop>
      <resourcetype><collection/></resourcetype>
      <c:supported-calendar-component-set><c:comp name="VTODO"/></c:supported-calendar-component-set>
    </prop></propstat>
  </response>
</multistatus>"#;
    let collections = parse_collections(prefixless);
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].slug, "inbox");
    assert!(collections[0].supports_vtodo);
}

#[test]
fn xml_unescape_decodes_all_five_entities() {
    assert_eq!(xml_unescape("&amp;&lt;&gt;&quot;&apos;"), "&<>\"'");
}

#[test]
fn xml_unescape_is_single_pass_and_preserves_bare_ampersands() {
    // Double-escaped input decodes exactly one level.
    assert_eq!(xml_unescape("&amp;lt;"), "&lt;");
    // Unknown entity-like input stays literal.
    assert_eq!(xml_unescape("a & b &unknown; c"), "a & b &unknown; c");
    assert_eq!(xml_unescape(""), "");
}
