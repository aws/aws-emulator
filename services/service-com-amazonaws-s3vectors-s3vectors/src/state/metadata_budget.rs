// SPDX-License-Identifier: CC0-1.0

//! How much metadata ONE vector may carry, in bytes.
//!
//! A vector's metadata is bounded by two SEPARATE per-vector budgets ("Limitations and
//! restrictions" of the Amazon S3 Vectors user guide): the FILTERABLE part — every key the
//! index does not declare non-filterable — has the smaller one, and the whole document,
//! filterable and non-filterable together, has the larger. A put whose vector crosses
//! either is refused with `ValidationException` (HTTP 400) and nothing of the batch is
//! stored.
//!
//! The two budgets are what `MetadataConfiguration$nonFilterableMetadataKeys` is FOR:
//! declaring a key non-filterable takes its bytes out of the smaller budget (the key can
//! then no longer be used in a query filter, which is the trade), and leaves them in the
//! larger one. So the same document is accepted under one index definition and refused
//! under another, decided by the index's own declaration rather than by the request.
//!
//! The size of a document is the size of the JSON it travelled as, with no insignificant
//! whitespace: the bytes the service would store and report back. It is measured by
//! WALKING the document rather than by re-serializing it, so measuring a document costs no
//! allocation of its own — and the filterable portion is measured as the object it would
//! be, over the keys that are in it.

use ::aws_smithy_types::{Document, Number};

/// The per-vector budget for FILTERABLE metadata: 2 KB, the smaller of the two budgets the
/// user guide's limits table states for a vector's metadata.
pub const MAX_FILTERABLE_METADATA_BYTES: usize = 2 * 1024;

/// The per-vector budget for ALL of a vector's metadata, filterable and non-filterable
/// together: 40 KB, the larger of the two budgets in that same table.
pub const MAX_TOTAL_METADATA_BYTES: usize = 40 * 1024;

/// Why `metadata` is too big to store on one vector of an index whose non-filterable keys
/// are `non_filterable` — or `None` when it fits both budgets.
///
/// ONE sentence about the document, as with the other defects a record is refused for:
/// which record of a batch it belongs to is the path the refusal carries, not something
/// the sentence says.
///
/// Each sentence names the budget that was crossed and the bound it has, in the phrasing
/// AWS states a size bound in, and carries no value from the request: which bytes they
/// were is the caller's own document, and the two budgets are told apart by the sentence
/// so a client can see which one it has to trim for.
pub fn metadata_budget_defect(metadata: &Document, non_filterable: &[String]) -> Option<String> {
    if document_bytes(metadata) > MAX_TOTAL_METADATA_BYTES {
        return Some(over_budget(TOTAL_BUDGET, MAX_TOTAL_METADATA_BYTES));
    }
    if filterable_bytes(metadata, non_filterable) > MAX_FILTERABLE_METADATA_BYTES {
        return Some(over_budget(FILTERABLE_BUDGET, MAX_FILTERABLE_METADATA_BYTES));
    }
    None
}

/// How a refusal names the budget for ALL of a vector's metadata: the subject of the
/// sentence, so it is spelled as a sentence begins.
pub const TOTAL_BUDGET: &str = "Metadata";

/// How a refusal names the budget for the FILTERABLE part of a vector's metadata.
pub const FILTERABLE_BUDGET: &str = "Filterable metadata";

/// The sentence a metadata document over a budget is refused with: the budget that was
/// crossed as the sentence's subject, and the bound it has, in the phrasing AWS states a
/// size bound in — the budget "must have at most" so many bytes — so a client can tell
/// which budget it has to trim for. Nothing in it comes from the request: `bytes` is the
/// published bound and never the submitted size, and WHICH record of a batch it is about
/// is carried by the refusal built around it.
pub fn over_budget(budget: &str, bytes: usize) -> String {
    format!("{budget} must have at most {bytes} bytes")
}

/// How many bytes the filterable part of `metadata` takes: the document restricted to the
/// top-level keys `non_filterable` does not name, measured as the object it would be.
///
/// Only a top-level key can be declared non-filterable (`nonFilterableMetadataKeys` names
/// metadata keys), so the restriction is over the document's own members; a document that
/// is not an object carries no keys to declare and is filterable whole.
pub fn filterable_bytes(metadata: &Document, non_filterable: &[String]) -> usize {
    let Document::Object(members) = metadata else {
        return document_bytes(metadata);
    };
    let filterable: Vec<(&String, &Document)> = members
        .iter()
        .filter(|(key, _)| !non_filterable.iter().any(|declared| declared == *key))
        .collect();
    object_bytes(filterable.into_iter())
}

/// How many bytes `document` takes as minified JSON — the bytes it travelled as, and the
/// bytes a read of the stored vector reports.
pub fn document_bytes(document: &Document) -> usize {
    // An explicit stack rather than recursion, so measuring costs no stack whatever the
    // caller nested (the same walk `super::metadata_defect` makes, which has already
    // bounded how many elements and how deep a stored document may be).
    let mut pending: Vec<&Document> = vec![document];
    let mut bytes: usize = 0;
    while let Some(node) = pending.pop() {
        bytes += match node {
            // `null`, `true`, `false`: the literal itself.
            Document::Null => 4,
            Document::Bool(true) => 4,
            Document::Bool(false) => 5,
            Document::Number(number) => number_bytes(number),
            Document::String(text) => string_bytes(text),
            Document::Array(elements) => {
                pending.extend(elements.iter());
                // The brackets, and one comma between every two elements.
                2 + elements.len().saturating_sub(1)
            }
            Document::Object(members) => {
                pending.extend(members.values());
                // The braces, one comma between every two members, and each member's
                // quoted key with the colon after it.
                2 + members.len().saturating_sub(1)
                    + members
                        .keys()
                        .map(|key| string_bytes(key) + 1)
                        .sum::<usize>()
            }
        };
    }
    bytes
}

/// How many bytes an object holding exactly `members` takes as minified JSON, each value
/// measured by [`document_bytes`].
fn object_bytes<'a>(members: impl Iterator<Item = (&'a String, &'a Document)>) -> usize {
    let mut bytes: usize = 2;
    let mut count: usize = 0;
    for (key, value) in members {
        count += 1;
        bytes += string_bytes(key) + 1 + document_bytes(value);
    }
    bytes + count.saturating_sub(1)
}

/// How many bytes a JSON string literal of `text` takes: the two quotes, plus one byte per
/// byte of the value and one more for each byte JSON requires an escape for.
///
/// The characters JSON cannot carry unescaped are the quote, the backslash and the control
/// characters below `0x20`; the five with a two-character escape (`\n`, `\r`, `\t`, `\b`,
/// `\f`) cost one extra byte, and every other control character travels as a `\u00XX`
/// escape, which is six bytes for one.
fn string_bytes(text: &str) -> usize {
    2 + text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
            control if control < 0x20 => 6,
            _ => 1,
        })
        .sum::<usize>()
}

/// How many bytes a JSON number literal takes, as the wire spells the value.
///
/// An integer is its own digits. A FLOAT is measured through the JSON serializer rather
/// than through Rust's `Display`, because the two disagree: `1.0` displays as `1` where
/// JSON writes `1.0`, and `1e100` displays as 101 digits where JSON writes `1e100`. A
/// value JSON cannot carry — an infinity or a NaN, which no request can have arrived with —
/// travels as `null`, the four bytes that serializer writes for it.
fn number_bytes(number: &Number) -> usize {
    match number {
        Number::PosInt(value) => value.to_string().len(),
        Number::NegInt(value) => value.to_string().len(),
        Number::Float(value) => {
            ::serde_json::to_string(&::serde_json::Value::from(*value))
                .map_or(4, |rendered| rendered.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document's measured size is the size of the JSON `serde_json` renders it as, over
    /// the forms a metadata document takes: scalars, an empty object, nested objects,
    /// arrays, and values needing escapes.
    #[test]
    fn a_documents_measured_size_is_the_size_of_its_json() {
        for (case, json) in [
            ("null", "null"),
            ("true", "true"),
            ("false", "false"),
            ("an integer", "17"),
            ("a negative integer", "-17"),
            ("a float", "1.0"),
            ("a negative fraction", "-0.5"),
            ("a float in exponent form", "1e100"),
            ("a small float in exponent form", "1e-7"),
            ("a float with many digits", "3.141592653589793"),
            ("a string", "\"genre\""),
            ("an empty string", "\"\""),
            ("a string needing escapes", "\"a\\\"b\\\\c\\nd\""),
            ("an empty object", "{}"),
            ("an empty array", "[]"),
            ("one member", "{\"genre\":\"scifi\"}"),
            ("two members", "{\"a\":1,\"b\":2}"),
            ("an array of scalars", "[1,2,3]"),
            (
                "a nested document",
                "{\"a\":{\"b\":[1,\"two\",null]},\"c\":true}",
            ),
        ] {
            let parsed: ::serde_json::Value =
                ::serde_json::from_str(json).expect("the case is JSON");
            let document = document_of(&parsed);
            let rendered = ::serde_json::to_string(&parsed).expect("it renders");
            assert_eq!(
                document_bytes(&document),
                rendered.len(),
                "case {case:?}: expected the JSON's own length ({rendered:?})"
            );
        }
    }

    /// Declaring a key non-filterable takes its bytes out of the filterable budget and
    /// leaves them in the total.
    #[test]
    fn declaring_a_key_non_filterable_takes_its_bytes_out_of_the_filterable_budget() {
        let parsed: ::serde_json::Value =
            ::serde_json::from_str("{\"small\":\"a\",\"big\":\"0123456789\"}")
                .expect("the document is JSON");
        let document = document_of(&parsed);
        let whole = document_bytes(&document);
        let filterable = filterable_bytes(&document, &["big".to_string()]);
        assert!(
            filterable < whole,
            "case \"one key declared\": expected the filterable part to be smaller than \
             the whole {whole}, got {filterable}"
        );
        assert_eq!(
            filterable_bytes(&document, &[]),
            whole,
            "case \"nothing declared\": expected the whole document to be filterable"
        );
        assert_eq!(
            filterable_bytes(&document, &["small".to_string(), "big".to_string()]),
            2,
            "case \"every key declared\": expected an empty object's two bytes"
        );
    }

    /// A value over the filterable budget is refused when its key is filterable and
    /// accepted when the index declares it non-filterable, while a document over the total
    /// budget is refused either way.
    #[test]
    fn each_budget_refuses_exactly_what_it_bounds() {
        let filterable_only = metadata_of(&format!(
            "{{\"note\":\"{}\"}}",
            "x".repeat(MAX_FILTERABLE_METADATA_BYTES)
        ));
        assert!(
            metadata_budget_defect(&filterable_only, &[]).is_some(),
            "case \"a filterable key over the filterable budget\": expected a refusal"
        );
        assert_eq!(
            metadata_budget_defect(&filterable_only, &["note".to_string()]),
            None,
            "case \"the same key declared non-filterable\": expected it to be accepted"
        );

        let over_total = metadata_of(&format!(
            "{{\"note\":\"{}\"}}",
            "x".repeat(MAX_TOTAL_METADATA_BYTES)
        ));
        for declared in [Vec::new(), vec!["note".to_string()]] {
            assert!(
                metadata_budget_defect(&over_total, &declared).is_some(),
                "case {declared:?}: a document over the total budget must be refused"
            );
        }
    }

    /// The `Document` a JSON text parses to, as the restJson1 deserializer hands it to a
    /// handler.
    fn metadata_of(json: &str) -> Document {
        let parsed: ::serde_json::Value = ::serde_json::from_str(json).expect("the case is JSON");
        document_of(&parsed)
    }

    /// `value` as a Smithy `Document`.
    fn document_of(value: &::serde_json::Value) -> Document {
        match value {
            ::serde_json::Value::Null => Document::Null,
            ::serde_json::Value::Bool(flag) => Document::Bool(*flag),
            ::serde_json::Value::Number(number) => Document::Number(match number.as_i64() {
                Some(integer) if integer < 0 => Number::NegInt(integer),
                Some(integer) => Number::PosInt(integer.unsigned_abs()),
                None => Number::Float(number.as_f64().unwrap_or_default()),
            }),
            ::serde_json::Value::String(text) => Document::String(text.clone()),
            ::serde_json::Value::Array(elements) => {
                Document::Array(elements.iter().map(document_of).collect())
            }
            ::serde_json::Value::Object(members) => Document::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), document_of(value)))
                    .collect(),
            ),
        }
    }
}
