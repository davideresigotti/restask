//! CalDAV XML request builders and response parsers (§10.1). Pure — no I/O.
//!
//! Radicale answers with namespace-prefixed WebDAV/CALDAV XML (`<D:multistatus>`,
//! `<C:comp name="VTODO"/>`), but the parsers match elements by **local name** only, so
//! responses using different prefixes or a default namespace parse identically. There is
//! deliberately no XML crate: the documents we produce or accept are small, well-formed,
//! and text-only, so a tolerant tag scanner suffices.

/// A collection discovered by a depth-1 PROPFIND (§10.2). Defined here because
/// [`parse_collections`] produces it; `caldav::port` re-exports it for the trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionInfo {
    /// `href` of the collection, verbatim (e.g. `/me/inbox/`).
    pub href: String,
    /// Last path segment of `href` with trailing slashes removed, e.g. `inbox`.
    pub slug: String,
    /// `displayname` when present and non-empty (entity-unescaped).
    pub display_name: Option<String>,
    /// `true` when `supported-calendar-component-set` advertises a `VTODO` comp.
    pub supports_vtodo: bool,
    /// `getctag` when the server reports one: an opaque value that changes whenever
    /// anything in the collection does. The daemon compares it between two looks to
    /// learn that another client wrote to the server (§13.1).
    pub ctag: Option<String>,
}

/// Builds the `MKCOL` body that creates a VTODO-only calendar collection (§10.1):
/// resourcetype `collection` + `calendar`, `displayname` set to the list's display name,
/// and `supported-calendar-component-set` restricted to `VTODO`.
/// The properties sit in `set` / `prop` (RFC 5689): a server that finds no `prop` there
/// sees a request for a plain folder, which Radicale refuses below a user's home (403).
pub fn mkcol_body(display_name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:mkcol xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:prop>
      <D:resourcetype>
        <D:collection/>
        <C:calendar/>
      </D:resourcetype>
      <D:displayname>{}</D:displayname>
      <C:supported-calendar-component-set>
        <C:comp name="VTODO"/>
      </C:supported-calendar-component-set>
    </D:prop>
  </D:set>
</D:mkcol>
"#,
        xml_escape(display_name)
    )
}

/// One `VTODO` resource from a [`report_vtodos`] answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportItem {
    /// Resource name: last path segment of the `href` with the `.ics` suffix stripped
    /// (kept percent-encoded exactly as the server sent it, so it can be reused in URLs).
    pub name: String,
    /// `getetag`, quoting preserved verbatim for `If-Match` use.
    pub etag: String,
    /// `calendar-data`: the iCalendar body; empty when the server did not inline it.
    pub data: String,
}

/// Builds the `REPORT` body asking for the `getetag` **and** the `calendar-data` of every
/// `VTODO` under the target collection (§10.1) — the whole remote snapshot of a list in
/// one request. The depth-1 scope comes from the request's `Depth: 1` header; the
/// `VCALENDAR`/`VTODO` comp-filter excludes VEVENTs and every other component (§10.5).
pub fn report_vtodos() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VTODO"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
"#
    .to_string()
}

/// Builds the `PROPFIND` body asking for the properties needed to classify collections
/// (§10.1): `resourcetype`, `displayname`, and `supported-calendar-component-set` — and
/// `getctag`, the collection's change tag.
pub fn propfind_collections_body() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">
  <D:prop>
    <D:resourcetype/>
    <D:displayname/>
    <C:supported-calendar-component-set/>
    <CS:getctag/>
  </D:prop>
</D:propfind>
"#
    .to_string()
}

/// Parses a `REPORT` multistatus into [`ReportItem`]s in document order (§10.1).
/// Responses without an `href` or a `getetag` (e.g. a 404 propstat) are skipped.
pub fn parse_report(xml: &str) -> Vec<ReportItem> {
    let mut items = Vec::new();
    for response in scan_elements(xml).iter().filter(|e| e.local == "response") {
        let props = scan_elements(&response.inner);
        let Some(href) = props.iter().find(|e| e.local == "href") else {
            continue;
        };
        let Some(etag) = props.iter().find(|e| e.local == "getetag") else {
            continue;
        };
        let href = element_text(&href.inner);
        let etag = element_text(&etag.inner);
        if href.is_empty() || etag.is_empty() {
            continue;
        }
        let name = last_path_segment(&href);
        let name = name.strip_suffix(".ics").unwrap_or(name);
        let data = props
            .iter()
            .find(|e| e.local == "calendar-data")
            .map(|e| character_data(&e.inner))
            .unwrap_or_default();
        items.push(ReportItem {
            name: name.to_string(),
            etag,
            data,
        });
    }
    items
}

/// Parses a `PROPFIND` multistatus into [`CollectionInfo`] entries in document order
/// (§10.1). Namespace-prefix agnostic. Only responses whose `resourcetype` includes a
/// `collection` element are returned; plain files, address books and foreign resource
/// types are excluded. A missing `supported-calendar-component-set` yields
/// `supports_vtodo = false`; a missing or empty `getctag` (a server without the
/// extension, or its "not found" answer for a container) yields `ctag = None`.
pub fn parse_collections(xml: &str) -> Vec<CollectionInfo> {
    let mut collections = Vec::new();
    for response in scan_elements(xml).iter().filter(|e| e.local == "response") {
        let props = scan_elements(&response.inner);
        let Some(href) = props.iter().find(|e| e.local == "href") else {
            continue;
        };
        let href = element_text(&href.inner);
        if href.is_empty() {
            continue;
        }
        let Some(resourcetype) = props.iter().find(|e| e.local == "resourcetype") else {
            continue;
        };
        if !scan_elements(&resourcetype.inner)
            .iter()
            .any(|e| e.local == "collection")
        {
            continue;
        }
        let display_name = props
            .iter()
            .find(|e| e.local == "displayname")
            .map(|e| element_text(&e.inner))
            .filter(|text| !text.is_empty())
            .map(|text| xml_unescape(&text));
        let supports_vtodo = props
            .iter()
            .find(|e| e.local == "supported-calendar-component-set")
            .map(|set| {
                scan_elements(&set.inner).iter().any(|comp| {
                    comp.local == "comp"
                        && comp.attrs.iter().any(|(name, value)| {
                            name == "name" && value.eq_ignore_ascii_case("VTODO")
                        })
                })
            })
            .unwrap_or(false);
        let ctag = props
            .iter()
            .find(|e| e.local == "getctag")
            .map(|e| xml_unescape(&element_text(&e.inner)))
            .filter(|text| !text.is_empty());
        collections.push(CollectionInfo {
            slug: last_path_segment(&href).to_string(),
            href,
            display_name,
            supports_vtodo,
            ctag,
        });
    }
    collections
}

/// Decodes the five predefined XML entities (`&amp; &lt; &gt; &quot; &apos;`) and numeric
/// character references (`&#13;`, `&#xD;`) in a single pass, so `&amp;lt;` decodes to
/// `&lt;` and never twice. A bare `&` that does not open a known entity is preserved
/// literally.
pub fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let (decoded, remainder) = if let Some((ch, t)) = numeric_reference(tail) {
            (ch, t)
        } else if let Some(t) = tail.strip_prefix("&amp;") {
            ('&', t)
        } else if let Some(t) = tail.strip_prefix("&lt;") {
            ('<', t)
        } else if let Some(t) = tail.strip_prefix("&gt;") {
            ('>', t)
        } else if let Some(t) = tail.strip_prefix("&quot;") {
            ('"', t)
        } else if let Some(t) = tail.strip_prefix("&apos;") {
            ('\'', t)
        } else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        out.push(decoded);
        rest = remainder;
    }
    out.push_str(rest);
    out
}

/// Decodes a numeric character reference at the start of `tail` (`&#NN;` or `&#xHH;`),
/// returning the character and the text after the reference.
fn numeric_reference(tail: &str) -> Option<(char, &str)> {
    let body = tail.strip_prefix("&#")?;
    let end = body.find(';')?;
    let digits = &body[..end];
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    Some((char::from_u32(code)?, &body[end + 1..]))
}

/// Character data of an element whose content is text (`calendar-data`): `CDATA` sections
/// are taken raw, everything else is entity-decoded; surrounding whitespace is trimmed.
fn character_data(inner: &str) -> String {
    const OPEN: &str = "<![CDATA[";
    const CLOSE: &str = "]]>";
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&xml_unescape(&rest[..start]));
        let body = &rest[start + OPEN.len()..];
        match body.find(CLOSE) {
            Some(end) => {
                out.push_str(&body[..end]);
                rest = &body[end + CLOSE.len()..];
            }
            None => {
                out.push_str(body);
                rest = "";
            }
        }
    }
    out.push_str(&xml_unescape(rest));
    out.trim().to_string()
}

/// Escapes text for XML element and attribute content (the inverse of [`xml_unescape`]).
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Returns the last path segment of `href` with trailing slashes removed.
fn last_path_segment(href: &str) -> &str {
    let trimmed = href.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// One scanned XML element: local name, attributes, and the raw inner XML span.
struct XmlElement {
    local: String,
    attrs: Vec<(String, String)>,
    inner: String,
}

/// An opening tag awaiting its closing counterpart.
struct OpenElement {
    local: String,
    attrs: Vec<(String, String)>,
    inner_start: usize,
}

/// Scans `xml` and returns every element in document order, flattened. Comments,
/// processing instructions, and `<!...>` declarations are skipped; attribute values are
/// parsed quote-aware; self-closing tags yield an empty `inner`. Malformed input never
/// panics — unmatched or unterminated tags are tolerated and simply dropped.
fn scan_elements(xml: &str) -> Vec<XmlElement> {
    let mut elements = Vec::new();
    let mut stack: Vec<OpenElement> = Vec::new();
    let mut pos = 0usize;
    while let Some(offset) = xml[pos..].find('<') {
        let tag_start = pos + offset;
        let rest = &xml[tag_start..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => pos = tag_start + end + 3,
                None => break,
            }
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            match rest.find("]]>") {
                Some(end) => pos = tag_start + end + 3,
                None => break,
            }
            continue;
        }
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(end) => pos = tag_start + end + 2,
                None => break,
            }
            continue;
        }
        if rest.starts_with("<!") {
            match rest.find('>') {
                Some(end) => pos = tag_start + end + 1,
                None => break,
            }
            continue;
        }
        let closing = rest.starts_with("</");
        let name_start = tag_start + if closing { 2 } else { 1 };
        let bytes = xml.as_bytes();
        let mut quote: Option<u8> = None;
        let mut end: Option<usize> = None;
        let mut cursor = name_start;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            match quote {
                Some(open) if byte == open => quote = None,
                Some(_) => {}
                None => match byte {
                    b'"' | b'\'' => quote = Some(byte),
                    b'>' => {
                        end = Some(cursor);
                        break;
                    }
                    _ => {}
                },
            }
            cursor += 1;
        }
        let Some(tag_end) = end else { return elements };
        if closing {
            let local = local_name(xml[name_start..tag_end].trim());
            if let Some(index) = stack.iter().rposition(|open| open.local == local) {
                let open = stack.remove(index);
                elements.push(XmlElement {
                    local: open.local,
                    attrs: open.attrs,
                    inner: xml[open.inner_start..tag_start].to_string(),
                });
            }
        } else {
            let head = xml[name_start..tag_end]
                .strip_suffix('/')
                .unwrap_or(&xml[name_start..tag_end]);
            let (local, attrs) = tag_head(head);
            if bytes[tag_end - 1] == b'/' {
                elements.push(XmlElement {
                    local,
                    attrs,
                    inner: String::new(),
                });
            } else {
                stack.push(OpenElement {
                    local,
                    attrs,
                    inner_start: tag_end + 1,
                });
            }
        }
        pos = tag_end + 1;
    }
    elements
}

/// Splits an opening tag's head into its qualified name and parsed attributes.
fn tag_head(head: &str) -> (String, Vec<(String, String)>) {
    let mut tokens = split_outside_quotes(head).into_iter();
    let name = tokens.next().unwrap_or_default();
    let mut attrs = Vec::new();
    for token in tokens {
        if let Some((attr, value)) = token.split_once('=') {
            attrs.push((attr.to_string(), unquote(value.trim()).to_string()));
        }
    }
    (local_name(name), attrs)
}

/// Splits `text` on whitespace runs that are not inside single- or double-quoted spans.
fn split_outside_quotes(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if start < index {
                    tokens.push(&text[start..index]);
                }
                start = index + ch.len_utf8();
            }
            None => {}
        }
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

/// Removes a matching pair of surrounding single or double quotes, if present.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(stripped) = value.strip_prefix(quote) {
            if let Some(inner) = stripped.strip_suffix(quote) {
                return inner;
            }
        }
    }
    value
}

/// Extracts the local name from a possibly prefixed XML name (`D:href` → `href`).
fn local_name(qualified: &str) -> String {
    match qualified.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => qualified.to_string(),
    }
}

/// Text content of a text-only element: strips any nested tags and trims whitespace.
fn element_text(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(index) = rest.find('<') {
        out.push_str(&rest[..index]);
        let Some(end) = rest[index..].find('>') else {
            return out.trim().to_string();
        };
        rest = &rest[index + end + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}
