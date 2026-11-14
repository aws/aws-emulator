// SPDX-License-Identifier: CC0-1.0

//! The bound a caller's MIME tree is held to BEFORE anything parses it — the one pre-parse
//! reading of a raw message every path of this service that reads one applies.
//!
//! Parsing a MIME tree descends once per nesting level, and the nesting is the CALLER's to
//! choose, so the depth of that descent is caller-chosen too: a message nesting deeply enough
//! exhausts the stack of whatever thread parses it, which ends the process rather than the
//! request. What stands between the two is this module: the containers a message DECLARES are
//! counted on the bytes, linearly, with no recursion at all
//! ([`multipart_containers`]), and a message declaring more than
//! [`super::MAX_MULTIPART_CONTAINERS`] of them — or carrying more than
//! [`super::MAX_MESSAGE_BYTES`] — is one no reader of this service hands to a parser
//! ([`within_parse_bounds`]).
//!
//! The count is an UPPER bound on the descent: every container it missed would be a level the
//! descent could take unbidden, which is why a field this emulator cannot read at all is
//! counted on its raw bytes rather than skipped ([`multipart_declarations`]).
//!
//! One module, because two paths need the same bound: `SendRawEmail`, which REFUSES a message
//! past it ("the maximum message size, including attachments, is 10 MB"), and the receiving
//! half's content scan ([`super::received_scan`]), which scans the message's own bytes and
//! leaves its parts alone rather than refusing anything — a message is RECEIVED after it was
//! accepted, so there is nobody left to refuse.

/// The media type every MIME container carries, which is what one level of nesting costs a
/// message: `Content-Type: multipart/<subtype>; boundary=…`.
const MULTIPART_MEDIA_TYPE: &[u8] = b"multipart/";

/// The header field a part's media type is declared in, which is the only place a media type
/// DECLARES anything: the same bytes in a body are prose.
const CONTENT_TYPE_FIELD_NAME: &[u8] = b"content-type";

/// Whether `raw` is a message a parser may be handed: no larger than one this service accepts
/// at all, and declaring no more containers than the descent that builds its tree may take.
///
/// Read on the BYTES, so nothing recursive runs before the answer is known.
#[must_use]
pub fn within_parse_bounds(raw: &[u8]) -> bool {
    raw.len() <= super::MAX_MESSAGE_BYTES
        && multipart_containers(raw) <= super::MAX_MULTIPART_CONTAINERS
}

/// How many times `needle` occurs in `line`, however either is cased.
fn occurrences_ignore_ascii_case(line: &[u8], needle: &[u8]) -> usize {
    let mut found = 0;
    let mut at = 0usize;
    while let Some(window) = line.get(at..at.saturating_add(needle.len())) {
        if window.eq_ignore_ascii_case(needle) {
            found += 1;
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

/// Whether `line` opens the header field named `Content-Type`.
///
/// A field name is the text before the first colon, and it "is case-insensitive" (RFC 5322);
/// white space around it is not part of the name, which is how a reader of the message takes
/// it, so `Content-Type :` names the same field as `content-type:`. A field NAME carries no
/// RFC 2047 encoded word — encoded words occur in field VALUES — so the name is decided on the
/// raw bytes.
fn opens_content_type_field(line: &[u8]) -> bool {
    line.iter()
        .position(|byte| *byte == b':')
        .and_then(|colon| line.get(..colon))
        .is_some_and(|name| {
            let name = name
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .map_or(&[][..], |first| &name[first..]);
            let name = name
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace())
                .map_or(&[][..], |last| &name[..=last]);
            name.eq_ignore_ascii_case(CONTENT_TYPE_FIELD_NAME)
        })
}

/// How many `multipart` media types ONE `Content-Type` field declares.
///
/// Read through the same decoding the MIME reader applies, rather than off the raw bytes:
/// `mailparse::parse_header` unfolds the field and decodes the RFC 2047 encoded words in its
/// value, so a media type a caller wrote as an encoded word is counted here exactly as the
/// parser will read it. A field this emulator cannot read at all is counted on its raw bytes
/// instead, because this count must never report FEWER containers than the parser may descend
/// into.
fn multipart_declarations(field: &[u8]) -> usize {
    match ::mailparse::parse_header(field) {
        Ok((header, _)) => {
            occurrences_ignore_ascii_case(header.get_value().as_bytes(), MULTIPART_MEDIA_TYPE)
        }
        Err(_) => occurrences_ignore_ascii_case(field, MULTIPART_MEDIA_TYPE),
    }
}

/// How many `multipart` containers the raw message DECLARES.
///
/// Counted on the BYTES, before the message is parsed: parsing a MIME tree descends once per
/// nesting level, and each level is one part declaring a `multipart` media type in its own
/// `Content-Type` field, so this count is what stands between a caller's chosen nesting and the
/// depth of that descent.
///
/// Only a `Content-Type` field DECLARES a media type, so only those fields are counted — the
/// same bytes in a body are prose, and a message that merely discusses MIME is not a deeply
/// nested one. A field value may be folded over several lines (RFC 5322), so a continuation
/// line, which opens with white space, stays inside the field it continues and is read as part
/// of it. The scan is linear in the bytes, and allocates one field at a time.
#[must_use]
pub fn multipart_containers(data: &[u8]) -> usize {
    let mut found = 0;
    // The `Content-Type` field currently being collected, folded lines included, and `None`
    // while the scan is not inside one.
    let mut field: Option<Vec<u8>> = None;
    for line in data.split(|byte| *byte == b'\n') {
        // A line of a `CRLF`-delimited message keeps its carriage return when split on the line
        // feed alone; it is part of the ending, not the field.
        let line = match line.split_last() {
            Some((last, rest)) if *last == b'\r' => rest,
            _ => line,
        };
        let continuation = line
            .first()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'));
        if continuation {
            if let Some(collected) = field.as_mut() {
                collected.push(b'\n');
                collected.extend_from_slice(line);
            }
            continue;
        }
        // Any line that is not a continuation ends the field before it.
        if let Some(collected) = field.take() {
            found += multipart_declarations(&collected);
        }
        if opens_content_type_field(line) {
            field = Some(line.to_vec());
        }
    }
    if let Some(collected) = field {
        found += multipart_declarations(&collected);
    }
    found
}
