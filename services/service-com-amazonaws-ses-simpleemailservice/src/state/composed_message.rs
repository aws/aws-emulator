// SPDX-License-Identifier: CC0-1.0

//! The RFC 5322 document Amazon SES COMPOSES for a message a caller handed it as parts, and
//! the pieces of that document every composition of this crate shares.
//!
//! `SendEmail` states whose document it is: "you specify the sender, the recipient, and the
//! message body, and Amazon SES assembles the message for you" — so a send that carries a
//! `Message` rather than a raw MIME document still has one message, written here, and that
//! document is what the account's own receive path saves
//! ([`super::inbound_delivery`]) and what a DKIM signature covers
//! ([`super::dkim`]).
//!
//! One composer, so the message a template renders to
//! ([`super::template_rendering`]) and the message a send composes cannot disagree about how a
//! part is declared: a message carrying BOTH body parts is `multipart/alternative` with the
//! text part and then the HTML part — least rich first, as MIME orders alternatives — and one
//! carrying a single part IS that part, under the UTF-8 character set its content may need,
//! declared `7bit` when every byte of it is ASCII and `8bit` otherwise.

use std::time::SystemTime;

/// The line ending a MIME message's headers and part delimiters are written with (RFC 5322).
pub const CRLF: &str = "\r\n";

/// The domain Amazon SES's own message identifiers are scoped to, which the `Message-ID` of a
/// message it composes is written under ([`message_id_header`]).
const MESSAGE_ID_DOMAIN: &str = "amazonses.com";

/// The parts ONE message is composed from.
///
/// Every field is what the request that was accepted carried, or what this service minted for
/// it; a field the message does not carry contributes no header line at all, which is what
/// makes this one composer serve a rendered template (subject and body parts only) and a
/// `SendEmail` (an addressed message) both.
#[derive(Debug)]
pub struct Composition<'a> {
    /// The `Message-ID` field's value, already in the `<...>` form a message identifier takes
    /// ([`message_id_header`]), and `None` for a composition that is not a message being sent.
    pub message_id: Option<&'a str>,
    /// The `From` address.
    pub from: Option<&'a str>,
    /// The `To` addresses.
    pub to: &'a [String],
    /// The `Cc` addresses.
    pub cc: &'a [String],
    /// The `Reply-To` addresses.
    pub reply_to: &'a [String],
    /// The `Subject`.
    pub subject: Option<&'a str>,
    /// The header fields the request asked for beside those, in the order it sent them.
    pub headers: &'a [(String, String)],
    /// The text body part.
    pub text: Option<&'a str>,
    /// The HTML body part.
    pub html: Option<&'a str>,
    /// The instant the message was composed, which is its `Date` — the origination date every
    /// mail message carries.
    pub composed_at: SystemTime,
}

impl<'a> Composition<'a> {
    /// A composition carrying only a subject and body parts: the message a RENDERED template
    /// is, which addresses nobody of itself.
    #[must_use]
    pub fn of_parts(
        subject: Option<&'a str>,
        text: Option<&'a str>,
        html: Option<&'a str>,
        composed_at: SystemTime,
    ) -> Self {
        Self {
            message_id: None,
            from: None,
            to: &[],
            cc: &[],
            reply_to: &[],
            subject,
            headers: &[],
            text,
            html,
            composed_at,
        }
    }
}

/// The `Message-ID` value a message Amazon SES accepted in `region` carries: the id the send
/// was accepted under, in the addr-spec form RFC 5322 §3.6.4 gives a message identifier.
#[must_use]
pub fn message_id_header(message_id: &str, region: &str) -> String {
    format!("<{message_id}@{region}.{MESSAGE_ID_DOMAIN}>")
}

/// The RFC 5322 document `message` composes to.
#[must_use]
pub fn compose(message: &Composition<'_>) -> String {
    let mut composed = String::new();
    push_field(&mut composed, "Date", &date_header(message.composed_at));
    if let Some(message_id) = message.message_id {
        push_field(&mut composed, "Message-ID", message_id);
    }
    if let Some(from) = message.from {
        push_field(&mut composed, "From", from);
    }
    for (name, addresses) in [
        ("To", message.to),
        ("Cc", message.cc),
        ("Reply-To", message.reply_to),
    ] {
        if !addresses.is_empty() {
            push_field(&mut composed, name, &address_list(addresses));
        }
    }
    if let Some(subject) = message.subject {
        push_field(&mut composed, "Subject", subject);
    }
    composed.push_str("MIME-Version: 1.0");
    composed.push_str(CRLF);
    for (name, value) in message.headers {
        push_field(&mut composed, name, value);
    }
    match (message.text, message.html) {
        (Some(text), Some(html)) => {
            let boundary = part_boundary(&[text, html]);
            composed.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"{CRLF}{CRLF}"
            ));
            for (content_type, body) in [("text/plain", text), ("text/html", html)] {
                composed.push_str(&format!("--{boundary}{CRLF}"));
                push_body(&mut composed, content_type, body);
                composed.push_str(CRLF);
            }
            composed.push_str(&format!("--{boundary}--{CRLF}"));
        }
        (Some(text), None) => push_body(&mut composed, "text/plain", text),
        (None, Some(html)) => push_body(&mut composed, "text/html", html),
        // A message with no body part at all still composes to a message, whose body is the
        // nothing it carries.
        (None, None) => push_body(&mut composed, "text/plain", ""),
    }
    composed
}

/// The addresses of one address field, as that field's value: a comma-separated list, which is
/// how RFC 5322 §3.4 writes one.
fn address_list(addresses: &[String]) -> String {
    addresses.join(", ")
}

/// Append the header field `name: value` to a message being composed.
fn push_field(message: &mut String, name: &str, value: &str) {
    message.push_str(name);
    message.push_str(": ");
    message.push_str(&header_text(value));
    message.push_str(CRLF);
}

/// Append one body — its content type, its transfer encoding and the body itself — to a
/// message being composed.
fn push_body(message: &mut String, content_type: &str, body: &str) {
    message.push_str(&format!("Content-Type: {content_type}; charset=UTF-8{CRLF}"));
    message.push_str(&format!(
        "Content-Transfer-Encoding: {}{CRLF}{CRLF}",
        transfer_encoding(body)
    ));
    message.push_str(body);
}

/// The content transfer encoding a body is declared under: `7bit` for a body every byte of
/// which is ASCII, and `8bit` for one carrying text that is not — the body travels as it was
/// submitted either way, so the declaration is what tells a reader which it is.
fn transfer_encoding(body: &str) -> &'static str {
    if body.is_ascii() {
        "7bit"
    } else {
        "8bit"
    }
}

/// `at` as a mail message's `Date` header states an instant: the date-and-time form RFC 5322
/// lays out for it, in UTC, written by the maintained calendar crate rather than by laying the
/// form out here.
fn date_header(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at).to_rfc2822()
}

/// `text` as one header line: a header is a single line, so a line break inside one is carried
/// as the space that folding leaves.
fn header_text(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

/// The delimiter the alternatives of one message are separated by: a value no part of this
/// message carries, which is what a MIME boundary has to be.
///
/// Drawn from the system's randomness rather than from the request or from anything stored:
/// the boundary names nothing, it only has to be absent from the content. A draw that does
/// occur in the content is re-drawn, a bounded number of times.
fn part_boundary(bodies: &[&str]) -> String {
    let mut boundary = drawn_boundary();
    for _ in 0..BOUNDARY_DRAWS {
        if !bodies.iter().any(|body| body.contains(&boundary)) {
            break;
        }
        boundary = drawn_boundary();
    }
    boundary
}

/// How many times a boundary is re-drawn before the composition carries on with the last
/// draw: each draw carries 64 bits this process did not choose, so a single collision is
/// already beyond reach.
const BOUNDARY_DRAWS: usize = 4;

/// One boundary draw, in the form a MIME multipart message delimits its parts with.
fn drawn_boundary() -> String {
    format!("----=_Part_{:016x}", super::system_entropy())
}
