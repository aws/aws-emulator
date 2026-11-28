// SPDX-License-Identifier: CC0-1.0

//! The ONE digest two caller-supplied JSON DOCUMENTS are told apart by.
//!
//! Several emulated services take an opaque `document` member — a XARF abuse report, a
//! workflow's override parameters, an entity's details — and later have to answer whether
//! two calls carried the SAME one: an idempotency token repeated with the same payload is
//! a replay, and the same token with a different payload is a conflict. That question is
//! about the document's VALUE, so the comparison has to be a function of the value and of
//! nothing else.
//!
//! A document's own [`std::fmt::Debug`] rendering is NOT that function: an object inside
//! it is a [`std::collections::HashMap`], which iterates in the order its own instance
//! chose, so two deserializations of the same bytes render differently and one payload
//! digests two ways. The walk here therefore emits an object's members in SORTED KEY
//! order, and leads every node with its KIND and every collection with its SIZE, so one
//! node's fields can never be read as another's: two documents digest alike exactly when
//! they are the same document. Member order and the whitespace the wire carried are no
//! part of a value and do not reach the digest; an array's own order is part of its value
//! and does.
//!
//! The walk is iterative, over an explicit worklist, so it costs heap rather than stack
//! however deeply a caller nested the document, and the mixing itself is the maintained
//! `sha2` crate's, through this crate's one stable derivation
//! ([`crate::stable_hex_id`]).
//!
//! Three service crates still carry their own copy of this walk
//! (`grep -rln 'fn append_document' services/*/src/`); they are to be migrated onto this
//! one, and nothing new should write another.

use ::aws_smithy_types::{Document, Number};

/// What separates the parts of one digested field, so a value carrying it cannot be read
/// as two fields.
const FIELD_SEPARATOR: char = '\u{1f}';

/// The digest of `document`, taken under `parts` — the other values the comparison is
/// about (the request members that travel with the document, the collection it belongs
/// to), digested ahead of it.
///
/// Two calls digest alike exactly when they carry the same `parts` and the same document.
/// Truncated as [`crate::stable_hex_id`] truncates, so this is an OPAQUE digest, not an
/// unguessable one: nothing may authenticate on it.
#[must_use]
pub fn document_fingerprint(parts: &[&str], document: &Document) -> String {
    let mut fields: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
    append_document_fields(&mut fields, document);
    let borrowed: Vec<&str> = fields.iter().map(String::as_str).collect();
    crate::stable_hex_id(&borrowed)
}

/// Append `document`'s canonical field sequence to `fields`, for a caller that digests
/// several documents, or a document beside other fields, in one sequence of its own.
pub fn append_document_fields(fields: &mut Vec<String>, document: &Document) {
    /// One entry of the worklist: a node still to be walked, or an object member's key,
    /// which is appended immediately before that member's value.
    enum Step<'a> {
        Node(&'a Document),
        Key(&'a str),
    }

    let mut pending = vec![Step::Node(document)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Key(key) => fields.push(format!("key{FIELD_SEPARATOR}{key}")),
            // Every node leads with the kind of value it is, so a string carrying a
            // number's text and that number never digest alike.
            Step::Node(Document::Null) => fields.push("null".to_string()),
            Step::Node(Document::Bool(flag)) => {
                fields.push(format!("boolean{FIELD_SEPARATOR}{flag}"));
            }
            Step::Node(Document::Number(Number::PosInt(number))) => {
                fields.push(format!("positive-integer{FIELD_SEPARATOR}{number}"));
            }
            Step::Node(Document::Number(Number::NegInt(number))) => {
                fields.push(format!("negative-integer{FIELD_SEPARATOR}{number}"));
            }
            Step::Node(Document::Number(Number::Float(number))) => {
                // The kind field is what keeps a float apart from an integer of the same
                // value: the text is `f64`'s own rendering, deterministic for a given
                // value but no canonical encoding of one (every NaN renders alike).
                fields.push(format!("float{FIELD_SEPARATOR}{number}"));
            }
            Step::Node(Document::String(text)) => {
                fields.push(format!("string{FIELD_SEPARATOR}{text}"));
            }
            // A collection announces how many children follow, and its children are
            // walked in order — an array's own order, which belongs to the value.
            Step::Node(Document::Array(items)) => {
                fields.push(format!("array{FIELD_SEPARATOR}{}", items.len()));
                pending.extend(items.iter().rev().map(Step::Node));
            }
            // An object's members are walked in SORTED KEY ORDER, because the order the
            // wire carried them in, and the order the map iterates them in, are no part
            // of the value.
            Step::Node(Document::Object(members)) => {
                fields.push(format!("object{FIELD_SEPARATOR}{}", members.len()));
                let mut sorted: Vec<(&String, &Document)> = members.iter().collect();
                sorted.sort_unstable_by_key(|(key, _)| *key);
                for (key, member) in sorted.into_iter().rev() {
                    // Pushed last-first, so the key is popped before its value.
                    pending.push(Step::Node(member));
                    pending.push(Step::Key(key.as_str()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A document object from its members.
    fn object(members: &[(&str, Document)]) -> Document {
        Document::Object(
            members
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect::<HashMap<String, Document>>(),
        )
    }

    /// A string value.
    fn string(value: &str) -> Document {
        Document::String(value.to_string())
    }

    /// The document every case below varies from: nested objects, an array, and a value
    /// of every kind the document model has.
    fn baseline() -> Document {
        object(&[
            ("category", string("content")),
            ("type", string("phishing")),
            (
                "reporter",
                object(&[("org", string("example")), ("confirmed", Document::Bool(true))]),
            ),
            (
                "evidence",
                Document::Array(vec![
                    object(&[("description", string("one"))]),
                    object(&[("description", string("two"))]),
                ]),
            ),
            ("count", Document::Number(Number::PosInt(3))),
            ("offset", Document::Number(Number::NegInt(-3))),
            ("ratio", Document::Number(Number::Float(0.5))),
            ("note", Document::Null),
        ])
    }

    /// The digest is a function of the VALUE: the same document digests alike however its
    /// members are ordered, and under however many instances of the map behind it.
    #[test]
    fn one_document_digests_alike_however_its_members_are_ordered() {
        let expected = document_fingerprint(&[], &baseline());
        // Built again from scratch each time, so each one gets its own `HashMap`
        // instance, which is what chooses an iteration order.
        for attempt in 1..=8 {
            let actual = document_fingerprint(&[], &baseline());
            assert_eq!(
                actual, expected,
                "case attempt {attempt}: expected one digest for one document, got {actual} \
                 instead of {expected}"
            );
        }
    }

    /// Two documents that differ in any way digest differently, and so do two calls that
    /// carry the same document under different leading parts.
    #[test]
    fn documents_that_differ_anywhere_digest_differently() {
        // The rows differ along what the rule generalises over: a changed scalar, a
        // changed value deep inside, an added member, a removed member, a value of
        // another KIND carrying the same text, a reordered array, an array of another
        // length, and an empty collection against an absent member.
        let rows: Vec<(&str, Document)> = vec![
            ("a changed scalar", {
                let mut members = vec![("type", string("spam"))];
                members.push(("category", string("content")));
                object(&members)
            }),
            (
                "a number as the string of its own text",
                object(&[("count", string("3"))]),
            ),
            (
                "an integer where a float of the same value went",
                object(&[("count", Document::Number(Number::Float(3.0)))]),
            ),
            (
                "an array in another order",
                object(&[(
                    "evidence",
                    Document::Array(vec![string("two"), string("one")]),
                )]),
            ),
            (
                "an array of another length",
                object(&[(
                    "evidence",
                    Document::Array(vec![string("one"), string("one"), string("two")]),
                )]),
            ),
            (
                "an empty object where none went",
                object(&[("reporter", object(&[]))]),
            ),
            (
                "an empty array where an empty object went",
                object(&[("reporter", Document::Array(Vec::new()))]),
            ),
            ("a null where a member went", object(&[("note", Document::Null)])),
            ("no members at all", object(&[])),
            ("not an object at all", string("content")),
        ];

        let mut seen: HashMap<String, String> = HashMap::new();
        seen.insert(
            document_fingerprint(&[], &baseline()),
            "the baseline".to_string(),
        );
        for (case, document) in rows {
            let digest = document_fingerprint(&[], &document);
            if let Some(other) = seen.insert(digest.clone(), case.to_string()) {
                panic!(
                    "case {case:?}: expected a digest no other document produces, got {digest} \
                     which is also the digest of {other:?}"
                );
            }
        }

        // The leading parts are part of the comparison too: one document under two of
        // them digests two ways, and an absent part is not the empty one. (No parts at
        // all is the baseline above, already seen.)
        let document = baseline();
        for (case, parts) in [
            ("one empty part", vec![""]),
            ("one part", vec!["case-1"]),
            ("another part", vec!["case-2"]),
            ("two parts", vec!["case-1", "case-2"]),
            ("the two parts joined", vec!["case-1case-2"]),
        ] {
            let digest = document_fingerprint(&parts, &document);
            if let Some(other) = seen.insert(digest.clone(), case.to_string()) {
                panic!(
                    "case {case:?}: expected a digest no other call produces, got {digest} which \
                     is also the digest of {other:?}"
                );
            }
        }
    }

    /// A document nested deeper than the request walk itself would take is digested,
    /// because this walk keeps its worklist on the heap.
    ///
    /// The depths stop well short of what a recursive walk overflows on, because a
    /// `Document` that deep cannot be DROPPED at the end of the case either: the type's
    /// own destructor recurses.
    #[test]
    fn a_deeply_nested_document_is_digested_without_recursing() {
        for depth in [1_usize, 64, 4_096] {
            let mut document = string("leaf");
            for _ in 0..depth {
                document = Document::Array(vec![document]);
            }
            let digest = document_fingerprint(&[], &document);
            assert_eq!(
                digest.chars().count(),
                16,
                "case a document nested {depth} deep: expected a digest, got {digest:?}"
            );
            assert_eq!(
                digest,
                document_fingerprint(&[], &document),
                "case a document nested {depth} deep: expected the same digest twice"
            );
        }
    }
}
