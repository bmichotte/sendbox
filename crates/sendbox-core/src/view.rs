//! The Email view: what a reader (the window or an agent) sees of one Email,
//! parsed at read time from its raw source with `mail-parser`.
//!
//! The raw source stays the record of the Email, so a parser bug never loses
//! data and the parser can be swapped without migrating the Inbox.

use std::collections::HashMap;
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use mail_parser::{HeaderForm, Message, MessageParser, MessagePart, PartKind};
use regex::{Captures, Regex};

use crate::EmailSummary;

/// One Email as read: its summary plus its content, parsed from the raw source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailView {
    pub summary: EmailSummary,
    /// The plain-text body, if the Email has one (several text parts are
    /// joined by a line break).
    pub text: Option<String>,
    /// The HTML body as written by the sender, if the Email has one.
    pub html: Option<String>,
    /// The HTML body ready to display: as written, but with each `cid:`
    /// reference to a part of the Email (an inline image, say) replaced by a
    /// `data:` URI holding that part, so a frame shows it without fetching
    /// anything. Agents get `html` instead: no attachment bytes over MCP.
    pub html_with_inline_images: Option<String>,
    /// Every header field, in order, with its value unfolded and decoded.
    pub headers: Vec<Header>,
    /// The link targets of the HTML body, then the URLs in the text body,
    /// each once, in order of appearance.
    pub links: Vec<String>,
    /// What is attached, without the content.
    pub attachments: Vec<Attachment>,
}

/// One header field of an Email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The name as written, such as `Subject` or `X-Mailer`.
    pub name: String,
    /// The value, unfolded and with encoded words decoded.
    pub value: String,
}

/// What a reader may know about an attachment before saving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The file name the sender gave, if any.
    pub name: Option<String>,
    /// The media type, such as `application/pdf`.
    pub content_type: String,
    /// The size of the content in bytes, once decoded.
    pub size: u64,
}

/// An attachment with its content, as saved to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentContent {
    pub attachment: Attachment,
    /// The content, decoded from its transfer encoding (base64, say).
    pub bytes: Vec<u8>,
}

impl AttachmentContent {
    /// The attachment at `index` in [`EmailView::attachments`] of the Email
    /// with this raw source, if there is one.
    pub(crate) fn parse(raw_source: &[u8], index: usize) -> Option<Self> {
        let message = MessageParser::default().parse(raw_source)?;
        let part = message.attachments().nth(index)?;
        Some(Self {
            attachment: attachment(part),
            bytes: part.decoded().into_owned(),
        })
    }
}

impl EmailView {
    /// Builds the view of `summary`'s Email from its raw source.
    pub(crate) fn parse(summary: EmailSummary, raw_source: &[u8]) -> Self {
        let Some(message) = MessageParser::default().parse(raw_source) else {
            // Not a message at all (SMTP accepts any bytes): show them as text.
            let text = String::from_utf8_lossy(raw_source).into_owned();
            return Self {
                summary,
                links: links(None, Some(&text)),
                text: Some(text),
                html: None,
                html_with_inline_images: None,
                headers: Vec::new(),
                attachments: Vec::new(),
            };
        };
        let text = text_body(&message);
        let html = html_body(&message);
        Self {
            summary,
            links: links(html.as_deref(), text.as_deref()),
            html_with_inline_images: html.as_deref().map(|html| inline_parts(&message, html)),
            text,
            html,
            headers: headers(&message),
            attachments: attachments(&message),
        }
    }
}

/// The real text parts. When an Email has only HTML, `mail-parser` offers
/// its HTML part as the text body too; that is not text, so it is skipped.
fn text_body(message: &Message<'_>) -> Option<String> {
    message
        .text_body()
        .filter(|part| matches!(part.kind(), PartKind::Text))
        .filter_map(|part| part.text())
        .map(|text| text.into_owned())
        .reduce(|joined, text| joined + "\n" + &text)
}

/// The real HTML part, with any `<meta charset>` set to UTF-8 to match the
/// decoded text. An Email without one gets no generated HTML.
fn html_body(message: &Message<'_>) -> Option<String> {
    message
        .html_body()
        .find(|part| matches!(part.kind(), PartKind::Html))
        .and_then(|part| part.html_utf8())
        .map(|html| html.into_owned())
}

/// A `cid:` URL (RFC 2392) in an attribute or a CSS `url()`, quoted or not;
/// the scheme ignores case. Captures the id, which may be URL-encoded.
static CID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bcid:([^\s"'<>()&]+)"#).expect("valid regex"));

/// `html` with its `cid:` references to parts of `message` replaced by the
/// parts themselves, as `data:` URIs. A reference to no part stays as is.
fn inline_parts(message: &Message<'_>, html: &str) -> String {
    let parts: HashMap<&str, MessagePart<'_>> = message
        .parts()
        .filter_map(|part| Some((part.content_id()?, part)))
        .collect();
    CID.replace_all(html, |reference: &Captures<'_>| {
        let id = &reference[1];
        percent_decode(id)
            .and_then(|decoded| parts.get(decoded.as_str()))
            .or_else(|| parts.get(id))
            .map_or_else(|| reference[0].to_owned(), |part| data_uri(*part))
    })
    .into_owned()
}

/// `encoded` with its `%XX` escapes decoded, or `None` if that is not UTF-8.
fn percent_decode(encoded: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let escaped = (byte == b'%')
            .then(|| after.get(..2))
            .flatten()
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(decoded) => {
                bytes.push(decoded);
                rest = &after[2..];
            }
            None => {
                bytes.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8(bytes).ok()
}

/// `part`'s decoded content as a `data:` URI.
fn data_uri(part: MessagePart<'_>) -> String {
    format!(
        "data:{};base64,{}",
        media_type(part),
        BASE64.encode(part.decoded())
    )
}

fn headers(message: &Message<'_>) -> Vec<Header> {
    message
        .headers()
        .into_iter()
        .map(|header| Header {
            name: header.raw_name().to_owned(),
            value: header
                .parse_as(HeaderForm::Text)
                .value()
                .as_text()
                .unwrap_or_default()
                .to_owned(),
        })
        .collect()
}

/// The attachments, in the order [`AttachmentContent::parse`] indexes them.
fn attachments(message: &Message<'_>) -> Vec<Attachment> {
    message.attachments().map(attachment).collect()
}

fn attachment(part: MessagePart<'_>) -> Attachment {
    Attachment {
        name: part.attachment_name().map(str::to_owned),
        content_type: media_type(part),
        size: part.decoded_len() as u64,
    }
}

/// `part`'s media type, such as `image/png`, lowercased. A part without one
/// is plain text (RFC 2045).
fn media_type(part: MessagePart<'_>) -> String {
    part.content_type()
        .map(|content_type| match content_type.subtype() {
            Some(subtype) => format!("{}/{subtype}", content_type.ctype()),
            None => content_type.ctype().to_owned(),
        })
        .map_or_else(|| "text/plain".to_owned(), |media| media.to_lowercase())
}

static HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).expect("valid regex")
});
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s<>"']+"#).expect("valid regex"));

/// Link targets in `html`, then URLs in `text`, each once. Agents use them
/// to follow verification or reset links.
fn links(html: Option<&str>, text: Option<&str>) -> Vec<String> {
    let hrefs = HREF
        .captures_iter(html.unwrap_or_default())
        .filter_map(|href| href.iter().skip(1).flatten().next())
        .map(|target| decode_entities(target.as_str().trim()))
        .filter(|target| !target.is_empty() && !target.starts_with('#'));
    let urls = URL
        .find_iter(text.unwrap_or_default())
        .map(|url| trim_trailing_punctuation(url.as_str()).to_owned());
    let mut links: Vec<String> = Vec::new();
    for link in hrefs.chain(urls) {
        if !links.contains(&link) {
            links.push(link);
        }
    }
    links
}

/// Drops what usually ends a sentence rather than a URL: "see
/// https://acme.test/a." or "(https://acme.test/a)".
fn trim_trailing_punctuation(url: &str) -> &str {
    let mut url = url;
    loop {
        let unbalanced = |open, close| {
            url.ends_with(close) && url.matches(open).count() < url.matches(close).count()
        };
        if url.ends_with(['.', ',', ';', ':', '!', '?'])
            || unbalanced('(', ')')
            || unbalanced('[', ']')
        {
            url = &url[..url.len() - 1];
        } else {
            return url;
        }
    }
}

/// Decodes the character references an attribute value may hold, such as
/// `&amp;` between query parameters.
fn decode_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        rest = &rest[start..];
        let reference = rest[1..]
            .find(';')
            .map(|end| &rest[1..=end])
            .and_then(|name| Some((decode_reference(name)?, name.len() + 2)));
        match reference {
            Some((character, length)) => {
                decoded.push(character);
                rest = &rest[length..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The character `&name;` stands for, if it is one links use.
fn decode_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}
