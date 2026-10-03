//! The wire form of a task's text (§8.4): wikilinks on a vault line are shown to other
//! clients as Markdown links that open the linked note in Obsidian. Pure — no I/O.
//!
//! `[[Dual HHD caddy]]` → title `[Dual HHD caddy](obsidian://open?vault=<vault>&file=Dual%20HHD%20caddy)`;
//! without a vault name, `Dual HHD caddy`. The vault spelling travels with the resource
//! (`X-RESTASK-TEXT`), so reading the resource back yields the line's text ([`relink`]).

/// The start of the URL of a link into Obsidian, as a Markdown link writes it.
const OPEN: &str = "](obsidian://open?";

/// One wikilink inside a task's text.
struct Wikilink<'a> {
    /// Byte range of the whole `[[…]]` in the text.
    start: usize,
    end: usize,
    /// The link target, before any `|`: `Note`, `Note#Heading`, `#Heading`.
    target: &'a str,
    /// What Obsidian shows for it: the alias, else the target with `#` read as ` > `.
    display: String,
}

impl Wikilink<'_> {
    /// The note the link opens: its target without the `#…` part; a same-note link
    /// (`[[#Heading]]`) opens `source_path`, the note the task lives in.
    fn file(&self, source_path: &str) -> String {
        let note = self.target.split('#').next().unwrap_or_default().trim();
        if note.is_empty() {
            source_path
                .strip_suffix(".md")
                .unwrap_or(source_path)
                .to_string()
        } else {
            note.to_string()
        }
    }
}

/// The wikilinks of `text`, in order. An embed (`![[…]]`) is not a link to open and is
/// left as it is; so is a `[[…]]` that would show nothing.
fn wikilinks(text: &str) -> Vec<Wikilink<'_>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("[[") {
        let start = from + offset;
        let inner_start = start + 2;
        let Some(length) = text[inner_start..].find("]]") else {
            break;
        };
        let inner = &text[inner_start..inner_start + length];
        let end = inner_start + length + 2;
        if inner.contains("[[") {
            from = start + 2;
            continue;
        }
        from = end;
        if text[..start].ends_with('!') {
            continue;
        }
        let (target, alias) = match inner.split_once('|') {
            Some((target, alias)) => (target, Some(alias.trim())),
            None => (inner, None),
        };
        let display = match alias.filter(|alias| !alias.is_empty()) {
            Some(alias) => alias.to_string(),
            None => target
                .split('#')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" > "),
        };
        if display.is_empty() {
            continue;
        }
        found.push(Wikilink {
            start,
            end,
            target: target.trim(),
            display,
        });
    }
    found
}

/// The title other clients show for a task (`SUMMARY`): every wikilink replaced by a
/// Markdown link that opens its note in the Obsidian vault `vault` — or, without a
/// vault name, by its shown text. A text without wikilinks is returned as it is.
pub fn wire_title(text: &str, vault: Option<&str>, source_path: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for link in wikilinks(text) {
        out.push_str(&text[at..link.start]);
        match vault {
            Some(vault) => out.push_str(&format!(
                "[{}{OPEN}vault={}&file={})",
                link.display,
                encode(vault),
                encode(&link.file(source_path))
            )),
            None => out.push_str(&link.display),
        }
        at = link.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A Markdown link into Obsidian found in a title.
struct OpenLink {
    start: usize,
    end: usize,
    /// The link's text.
    shown: String,
    /// The `vault` parameter, decoded.
    vault: String,
    /// The `file` parameter, decoded.
    file: String,
}

/// The Markdown links to `obsidian://open?…` in `title` that name a vault and a file, in
/// order.
fn open_links(title: &str) -> Vec<OpenLink> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = title[from..].find(OPEN) {
        let middle = from + offset;
        let query_start = middle + OPEN.len();
        from = query_start;
        let Some(start) = title[..middle].rfind('[') else {
            continue;
        };
        if found.last().is_some_and(|last: &OpenLink| start < last.end) {
            continue;
        }
        let length = title[query_start..]
            .find(|ch: char| ch == ')' || ch.is_whitespace())
            .unwrap_or(title.len() - query_start);
        if !title[query_start + length..].starts_with(')') {
            continue;
        }
        let end = query_start + length + 1;
        let mut vault = None;
        let mut file = None;
        for pair in title[query_start..query_start + length].split('&') {
            match pair.split_once('=') {
                Some(("vault", value)) => vault = Some(decode(value)),
                Some(("file", value)) => file = Some(decode(value)),
                _ => {}
            }
        }
        let (Some(vault), Some(file)) = (vault, file) else {
            continue;
        };
        found.push(OpenLink {
            start,
            end,
            shown: title[start + 1..middle].to_string(),
            vault,
            file,
        });
        from = end;
    }
    found
}

/// The vault text of a title read from the server, given the vault text it was written
/// from (`reference`, empty when none) and the note the task lives in.
///
/// A title that shows `reference` exactly — with links into any vault, or with plain
/// text — gives `reference` back. A title that holds a wikilink is taken as written.
/// Otherwise, in a title with links into Obsidian each such link becomes the wikilink of
/// `reference` it shows (same text, same note), else a wikilink to the note it opens; in
/// a title without them each wikilink of `reference` whose shown text the title still
/// contains is put back in its place, in order. A link whose text is gone is dropped.
pub fn relink(title: &str, reference: &str, source_path: &str) -> String {
    if !wikilinks(title).is_empty() {
        return title.to_string();
    }
    let found = open_links(title);
    let vault = found.first().map(|link| link.vault.as_str());
    if wire_title(reference, vault, source_path) == title {
        return reference.to_string();
    }
    let links = wikilinks(reference);
    let mut out = String::with_capacity(title.len().max(reference.len()));
    let mut at = 0;
    if found.is_empty() {
        for link in links {
            let Some(offset) = title[at..].find(&link.display) else {
                continue;
            };
            out.push_str(&title[at..at + offset]);
            out.push_str(&reference[link.start..link.end]);
            at += offset + link.display.len();
        }
    } else {
        let mut next = 0;
        for open in found {
            out.push_str(&title[at..open.start]);
            let known = links[next..]
                .iter()
                .position(|link| link.display == open.shown && link.file(source_path) == open.file);
            match known {
                Some(index) => {
                    let link = &links[next + index];
                    out.push_str(&reference[link.start..link.end]);
                    next += index + 1;
                }
                None if open.shown == open.file => out.push_str(&format!("[[{}]]", open.file)),
                None => out.push_str(&format!("[[{}|{}]]", open.file, open.shown)),
            }
            at = open.end;
        }
    }
    out.push_str(&title[at..]);
    out
}

/// Percent-encodes a URL query value: every byte but the unreserved ones (RFC 3986).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reverses [`encode`]; a `%` not followed by two hex digits is taken literally.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
