// SPDX-License-Identifier: CC0-1.0

//! The ORDER the two caller-supplied maps of an S3 Batch Operations job are
//! written in.
//!
//! The rule under test: a map member of a response body is written entry by entry
//! in the order the map holds them, so a map a job carries — a `LambdaInvoke`
//! job's `UserArguments`, a copy job's `NewObjectMetadata/UserMetadata` — is held
//! in an ORDERED map, and two answers built from the same pairs write the same
//! document whatever order those pairs arrived in. A hashed map iterates in an
//! order that differs per map instance and per process, so two reads of one
//! unchanged job could report the same pairs differently and a client diffing a
//! stored job against its desired state would see a difference that is not there.
//!
//! The rows below differ along what the rule generalises over: which shape holds
//! the map (so which map shape it is), how many pairs it holds (the minimum the
//! modelled `UserArguments` length admits, a middle count, and its maximum of
//! ten), and the order the pairs were inserted in.
//!
//! Each case writes the shape through the SAME `restXml` codec a response is
//! serialized by and reads the entries back off the document, rather than
//! driving `CreateJob`: a nested map member of a request body does not reach a
//! handler populated (a create carrying a `UserMetadata` block is accepted and
//! `DescribeJob` answers `<UserMetadata></UserMetadata>`), so what decides the
//! order of a reported map is the map the shape holds and the codec's walk over
//! it. The sibling suites drive `CreateJob` and `DescribeJob` end to end over
//! HTTP.

use std::collections::BTreeMap;

use aws_smithy_schema::codec::FinishSerializer;
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::xml::XmlShapeSerializer;

use service_com_amazonaws_s3control_awss3controlservicev20180820::ops::types::{
    LambdaInvokeOperation, S3ObjectMetadata,
};

/// One row: the pairs to insert, in the order they are inserted.
struct MapCase {
    case: &'static str,
    inserted: &'static [(&'static str, &'static str)],
}

/// The rows every assertion below runs over.
fn map_cases() -> Vec<MapCase> {
    vec![
        MapCase {
            case: "one pair, the minimum the modelled UserArguments length admits",
            inserted: &[("only", "x")],
        },
        MapCase {
            case: "four pairs, inserted neither sorted nor reverse sorted",
            inserted: &[
                ("zeta", "26"),
                ("alpha", "1"),
                ("mu", "13"),
                ("beta", "2"),
            ],
        },
        MapCase {
            case: "the same four pairs, inserted in another order",
            inserted: &[
                ("mu", "13"),
                ("beta", "2"),
                ("zeta", "26"),
                ("alpha", "1"),
            ],
        },
        MapCase {
            case: "ten pairs, inserted backwards — the maximum that length admits",
            inserted: &[
                ("k9", "v9"),
                ("k8", "v8"),
                ("k7", "v7"),
                ("k6", "v6"),
                ("k5", "v5"),
                ("k4", "v4"),
                ("k3", "v3"),
                ("k2", "v2"),
                ("k1", "v1"),
                ("k0", "v0"),
            ],
        },
        MapCase {
            case: "five pairs whose keys sort differently from their values",
            inserted: &[
                ("stage", "archived"),
                ("owner", "data-platform"),
                ("retention", "7y"),
                ("department", "finance"),
                ("wave", "3"),
            ],
        },
    ]
}

impl MapCase {
    /// The pairs as the map the two shapes hold, populated in the row's own
    /// insertion order. The map type is the shapes' own: a declaration holding a
    /// hashed map would not compile against this.
    fn map(&self) -> BTreeMap<String, String> {
        self.inserted
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    /// The sequence the entries must be written in: ordered by key.
    fn expected(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .inserted
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        pairs.sort();
        pairs
    }
}

/// `value` written through the `restXml` codec, as the document a response
/// carries it in.
fn written_document(case: &str, schema: &Schema, value: &dyn SerializableStruct) -> String {
    let mut serializer = XmlShapeSerializer::new(None);
    serializer
        .write_struct(schema, value)
        .unwrap_or_else(|error| panic!("case {case:?}: the shape serializes: {error}"));
    String::from_utf8(serializer.finish())
        .unwrap_or_else(|error| panic!("case {case:?}: the document is UTF-8: {error}"))
}

/// The map entries a document carries, in the order it carries them: the
/// `restXml` wrapped-map layout is
/// `<entry><key>..</key><value>..</value></entry>…`.
fn entries_of(case: &str, document: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find("<entry>") {
        let after = &rest[start + "<entry>".len()..];
        let Some(end) = after.find("</entry>") else {
            panic!("case {case:?}: an <entry> with no closing tag in {document}")
        };
        let entry = &after[..end];
        let text_of = |element: &str| -> String {
            let open = format!("<{element}>");
            let close = format!("</{element}>");
            let from = entry
                .find(&open)
                .unwrap_or_else(|| panic!("case {case:?}: no <{element}> in {entry}"))
                + open.len();
            let to = entry[from..]
                .find(&close)
                .unwrap_or_else(|| panic!("case {case:?}: no </{element}> in {entry}"))
                + from;
            entry[from..to].to_string()
        };
        entries.push((text_of("key"), text_of("value")));
        rest = &after[end + "</entry>".len()..];
    }
    entries
}

#[test]
fn a_lambda_jobs_user_arguments_are_written_in_key_order() {
    for case in map_cases() {
        let operation = LambdaInvokeOperation {
            function_arn: Some("arn:aws:lambda:us-west-2:123456789012:function:transform".into()),
            invocation_schema_version: Some("2.0".into()),
            user_arguments: Some(case.map()),
        };
        let document = written_document(case.case, &LambdaInvokeOperation::SCHEMA, &operation);
        let written = entries_of(case.case, &document);
        assert_eq!(
            written,
            case.expected(),
            "case {:?}: expected the arguments written ordered by key, got {written:?} in \
             {document}",
            case.case
        );
    }
}

#[test]
fn a_copy_jobs_object_metadata_is_written_in_key_order() {
    for case in map_cases() {
        let metadata = S3ObjectMetadata {
            content_type: Some("text/csv".into()),
            user_metadata: Some(case.map()),
            ..S3ObjectMetadata::default()
        };
        let document = written_document(case.case, &S3ObjectMetadata::SCHEMA, &metadata);
        let written = entries_of(case.case, &document);
        assert_eq!(
            written,
            case.expected(),
            "case {:?}: expected the metadata written ordered by key, got {written:?} in \
             {document}",
            case.case
        );
    }
}

#[test]
fn the_same_pairs_are_written_the_same_way_whatever_order_they_arrived_in() {
    let cases = map_cases();
    // Rows two and three carry the SAME pairs inserted in different orders, so
    // the documents written from them must agree entry for entry.
    let documents: Vec<Vec<(String, String)>> = cases
        .iter()
        .skip(1)
        .take(2)
        .map(|case| {
            let operation = LambdaInvokeOperation {
                function_arn: Some(
                    "arn:aws:lambda:us-west-2:123456789012:function:transform".into(),
                ),
                invocation_schema_version: Some("2.0".into()),
                user_arguments: Some(case.map()),
            };
            let document = written_document(case.case, &LambdaInvokeOperation::SCHEMA, &operation);
            entries_of(case.case, &document)
        })
        .collect();

    assert_eq!(
        documents.first(),
        documents.last(),
        "expected the same pairs written in one order whichever order they arrived in, got \
         {documents:?}"
    );
}
