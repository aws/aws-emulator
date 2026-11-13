// SPDX-License-Identifier: CC0-1.0

//! The GRAMMAR the tag set a write delivers IN A HEADER has to be in, and the refusal a
//! header outside it earns.
//!
//! "The tag-set for the object. The tag-set must be encoded as URL Query parameters. (For
//! example, "Key1=Value1")" — `PutObjectRequest$Tagging`, and the same sentence on
//! `CopyObjectRequest$Tagging` and `CreateMultipartUploadRequest$Tagging`. So the header is a
//! URL query string: each `&`-separated pair NAMES a tag and gives its value as URL-ENCODED
//! text, and — being a query string, which maps one parameter to one value — it names each
//! tag once.
//!
//! A header outside that grammar carries no tag set at all, so S3 refuses it for the HEADER:
//! `InvalidArgument` (400) with the sentence that states the grammar
//! ([`INLINE_TAGGING_ENCODING_MESSAGE`]) and the `<ArgumentName>` / `<ArgumentValue>` pair
//! naming the header and echoing it as sent.
//!
//! That is a different fault from a tag set the header DID encode whose own key or value
//! breaks one of the published tag restrictions (`InvalidTag`, [`super::TagSetRejection`]):
//! the first says the caller has to encode the header, the second that a tag in it is not
//! storable. Which of the two a header earns follows from WHERE the fault is: a pair's NAME is
//! the tag's key, read as it stands and judged by the tag restrictions, while its VALUE is
//! URL-encoded text — so text in the value half that no URL-encoder would have left there is
//! the header's encoding, and everything the value DECODES to is the tag's.
//!
//! ONE grammar and one reading, so every write that takes a tag set in a header — the object
//! write, the two forms of copy, the multipart initiation and the browser-form upload — judges
//! it alike.

/// The header a write delivers its tag set in, as S3's `InvalidArgument` document names the
/// argument it rejected (`smithy_query(query="operation",operation="PutObject")` reports
/// `Tagging: {httpHeader: "x-amz-tagging"}`).
pub const INLINE_TAGGING_HEADER: &str = "x-amz-tagging";

/// The `<Message>` S3 sends with [`super::INVALID_ARGUMENT_CODE`] for a header outside that
/// grammar.
///
/// Fixed text, naming the header and the whole requirement — encoded as UTF-8, then
/// URL-encoded as query parameters, with no tag name twice — because any one of those is what
/// the caller has to fix; the header as sent travels in the `<ArgumentValue>` of the same
/// document.
pub const INLINE_TAGGING_ENCODING_MESSAGE: &str =
    "The header 'x-amz-tagging' shall be encoded as UTF-8 then URLEncoded URL query parameters \
     without tag name duplicates.";

/// Whether `header` is in the grammar [`INLINE_TAGGING_ENCODING_MESSAGE`] states: a URL query
/// string of UTF-8 text that names no tag twice.
///
/// Two conditions, one about each pair's VALUE half and one about the header as a whole:
///
/// 1. the value is the URL-ENCODED form of a tag's value: its bytes decode as UTF-8 (the text
///    having been encoded "as UTF-8 then URLEncoded"), and what they decode to is text a tag
///    value may carry ([`super::tag_text_is_allowed`]) — text outside that class cannot be the
///    encoding of any tag value, so the header is not the URL query string it is declared to
///    be;
/// 2. no tag NAME appears twice, because a query string maps one parameter to one value.
///
/// A pair's NAME is not judged here: it is the tag's KEY as it stands, and the published
/// restrictions on a tag key are what judge it ([`super::validate_tag_set`]), so a key outside
/// them is refused as that tag rather than as the header's encoding. Nor is the LENGTH of a
/// decoded value: a value of text a tag may carry is a tag value, and how long one may be is a
/// published restriction on the tag.
///
/// An EMPTY header carries no pair, which is the empty tag set and is in the grammar.
pub fn inline_tagging_header_is_well_formed(header: &str) -> bool {
    header
        .split('&')
        .filter(|pair| !pair.is_empty())
        .all(|pair| {
            let encoded = pair.split_once('=').map(|(_, value)| value).unwrap_or("");
            decoded_tag_value(encoded).is_some_and(|value| super::tag_text_is_allowed(&value))
        })
        && names_each_tag_once(header)
}

/// The text `encoded` is the URL-encoded form of, or `None` when it is not the encoding of any
/// text: escapes whose bytes are not UTF-8.
///
/// Decoded by the percent-encoding crate this service already reads and writes URL-encoded text
/// with ([`percent_encoding`]), so the grammar judges what a decoder really produces. An escape
/// that is not a complete `%XX` is no escape at all and survives as the literal `%` it was
/// written with, which is then text a tag value may not carry.
fn decoded_tag_value(encoded: &str) -> Option<String> {
    ::percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .ok()
        .map(::std::borrow::Cow::into_owned)
}

/// Whether `header` names each tag at most once, read through the same form-urlencoded parse
/// the tag set itself is built by ([`super::tag_set_from_header`]), so the names judged here
/// are the names that would be stored.
fn names_each_tag_once(header: &str) -> bool {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    ::aws_smithy_schema_server::query::parse_form_urlencoded(header.as_bytes())
        .into_iter()
        .all(|(key, _)| seen.insert(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammar admits every header a client encodes as the member's documentation says,
    /// and refuses each way a header can be outside it.
    #[test]
    fn the_header_is_a_url_query_string_of_utf8_naming_each_tag_once() {
        // (case, the header as sent, whether it is in the grammar)
        let rows: [(&str, &str, bool); 16] = [
            ("the example the member's documentation gives", "Key1=Value1", true),
            ("no header text at all", "", true),
            ("several pairs", "zeta=3&alpha=1&middle=2", true),
            ("a pair with no value", "emptied=", true),
            ("a pair with no = at all", "novalue", true),
            ("a plus for a space", "project=two+words", true),
            ("percent escapes", "project=two%20words", true),
            ("escaped reserved characters", "expression=a%20%2B%20b%20%3D%20c%2Fd", true),
            ("an escaped non-ASCII pair", "k%C3%A9y=v%C3%A4lue", true),
            ("punctuation a tag value may carry", "path=logs/2026:01@host", true),
            ("a space, which a tag value may carry", "summary=two words", true),
            // A pair's NAME is read as it stands and judged as the TAG's key, so a character
            // no tag may carry says nothing about the header's encoding when it is there.
            ("a bracket in the name", "summary[q3]=yes", true),
            ("a bracket in the value", "summary=two[words]", false),
            ("an escape cut short", "project=two%2", false),
            ("an escape that is not hexadecimal", "project=two%zzwords", false),
            ("the same tag name twice", "project=first&project=second", false),
        ];
        for (case, header, expected) in rows {
            let actual = inline_tagging_header_is_well_formed(header);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the header {header:?} to be {}, got {actual:?}",
                match expected {
                    true => "in the grammar",
                    false => "outside it",
                }
            );
        }
        // An escape naming a byte that is no UTF-8 sequence is outside the grammar too: the
        // text was to be encoded as UTF-8 before it was URL-encoded. So is an escaped
        // character no tag value may carry, which no encoding of a tag value can produce.
        for (case, header) in [
            ("an escape decoding to a byte that is not UTF-8", "key=%FF"),
            ("an escaped character no tag value may carry", "mood=%F0%9F%98%80"),
        ] {
            assert!(
                !inline_tagging_header_is_well_formed(header),
                "case {case:?}: the header {header:?} must be outside the grammar"
            );
        }    }
}
