// SPDX-License-Identifier: CC0-1.0

//! The single-document inference family read over the `rpcv2Cbor` route, against the
//! same answers the `awsJson1_1` route gives for the same document.
//!
//! The model declares both protocols
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-comprehend-comprehend-20171127/smithy-model/com-amazonaws-comprehend-comprehend-20171127.json")`),
//! so every one of these analysers is reachable over either, and the protocol only FRAMES
//! the call. `cbor_wire_route.rs` asserts that rule on the framing, the store and the
//! failures; this suite is its counterpart for the ANSWER CONTENTS, and the CBOR
//! counterpart of `zz_probe.rs`: ONE document is read by all five implemented
//! single-document analysers over both routes, and each answer's discrete parts must be
//! the same whichever route carried it.
//!
//! Asserting that catches what a per-route success check cannot: a codec that writes a
//! list element with its member name as a key, drops a nested structure, or loses the
//! order of a list would still answer 200 and still decode, and only a comparison with
//! the other route's answer shows the difference.
//!
//! Only the DISCRETE parts of each answer are compared — the tags, the spans and their
//! types, the label names, the token ids. A confidence is a 32-bit float, and the two
//! protocols put one on the wire in different forms (a 4-byte CBOR float, a decimal
//! number in JSON), so the two routes' values agree to that type's precision rather than
//! bit for bit; `cbor_wire_route.rs` asserts the confidence decodes as the number it was
//! encoded from, and the rules each analyser's confidences obey are asserted in the
//! suites that own them.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use serde_json::{json, Value};

use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ContainsPiiEntitiesInput, ContainsPiiEntitiesOutput, DetectEntitiesInput, DetectEntitiesOutput,
    DetectPiiEntitiesInput, DetectPiiEntitiesOutput, DetectSyntaxInput, DetectSyntaxOutput,
    DetectToxicContentInput, DetectToxicContentOutput, TextSegment,
    CONTAINS_PII_ENTITIES_INPUT_SCHEMA, DETECT_ENTITIES_INPUT_SCHEMA,
    DETECT_PII_ENTITIES_INPUT_SCHEMA, DETECT_SYNTAX_INPUT_SCHEMA,
    DETECT_TOXIC_CONTENT_INPUT_SCHEMA,
};

use crate::comprehend_client::Comprehend;

/// The one document every analyser below reads: a company name followed by a product of
/// its own, a personal name in running text, a postal address, a mail address and a date,
/// so each analyser has several spans of its own kind to report and the comparison is over
/// a LIST rather than over a single value.
const DOCUMENT: &str = "Amazon Comprehend is a natural language processing service. \
                        Contact John Smith at 123 Main Street, Seattle, WA 98101 or \
                        john.smith@example.com for more details today.";

/// `input` as the `rpcv2Cbor` request body a client sends, written through the protocol's
/// own codec against the operation's input schema.
fn cbor_body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// One call over the CBOR route, decoded against its own output schema.
async fn over_cbor<O: DeserializableStruct>(
    comprehend: &Comprehend,
    case: &str,
    operation: &str,
    body: bytes::Bytes,
) -> O {
    let response = comprehend.cbor(operation, body).await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200 over rpc-v2-cbor, got {status} with {:?}",
        String::from_utf8_lossy(response.body().as_ref())
    );
    let answered = response.into_body();
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(answered.as_ref());
    O::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the answer must decode against its output schema, got {error}")
    })
}

/// A single-document English request as the JSON route carries it.
fn json_request() -> Value {
    json!({ "Text": DOCUMENT, "LanguageCode": "en" })
}

/// The string at `key` of a JSON object, or the empty string.
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// The integer at `key` of a JSON object, or `-1` — a value no offset or id takes, so an
/// absent member cannot be read as a present one.
fn number(value: &Value, key: &str) -> i64 {
    value[key].as_i64().unwrap_or(-1)
}

/// The members of a JSON list, or an empty list.
fn list(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn the_entity_spans_are_the_same_on_both_routes() {
    let case = "DetectEntities over both routes";
    let comprehend = Comprehend::new();
    let over_json = comprehend.ok("DetectEntities", &json_request()).await;
    let expected: Vec<(String, String, i64, i64)> = list(&over_json, "Entities")
        .iter()
        .map(|entity| {
            (
                text(entity, "Text"),
                text(entity, "Type"),
                number(entity, "BeginOffset"),
                number(entity, "EndOffset"),
            )
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "case {case:?}: the document carries entities, so the comparison must have rows, \
         got {over_json}"
    );

    let output: DetectEntitiesOutput = over_cbor(
        &comprehend,
        case,
        "DetectEntities",
        cbor_body(
            DETECT_ENTITIES_INPUT_SCHEMA,
            &DetectEntitiesInput {
                text: Some(DOCUMENT.to_string()),
                language_code: Some("en".to_string()),
                ..DetectEntitiesInput::default()
            },
        ),
    )
    .await;
    let actual: Vec<(String, String, i64, i64)> = output
        .entities
        .unwrap_or_default()
        .into_iter()
        .map(|entity| {
            (
                entity.text.unwrap_or_default(),
                entity.r#type.unwrap_or_default(),
                i64::from(entity.begin_offset.unwrap_or(-1)),
                i64::from(entity.end_offset.unwrap_or(-1)),
            )
        })
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn the_pii_spans_and_labels_are_the_same_on_both_routes() {
    let case = "DetectPiiEntities over both routes";
    let comprehend = Comprehend::new();
    let over_json = comprehend.ok("DetectPiiEntities", &json_request()).await;
    let expected: Vec<(String, i64, i64)> = list(&over_json, "Entities")
        .iter()
        .map(|entity| {
            (
                text(entity, "Type"),
                number(entity, "BeginOffset"),
                number(entity, "EndOffset"),
            )
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "case {case:?}: the document carries PII, so the comparison must have rows, got \
         {over_json}"
    );

    let output: DetectPiiEntitiesOutput = over_cbor(
        &comprehend,
        case,
        "DetectPiiEntities",
        cbor_body(
            DETECT_PII_ENTITIES_INPUT_SCHEMA,
            &DetectPiiEntitiesInput {
                text: DOCUMENT.to_string(),
                language_code: "en".to_string(),
            },
        ),
    )
    .await;
    let actual: Vec<(String, i64, i64)> = output
        .entities
        .unwrap_or_default()
        .into_iter()
        .map(|entity| {
            (
                entity.r#type.unwrap_or_default(),
                i64::from(entity.begin_offset.unwrap_or(-1)),
                i64::from(entity.end_offset.unwrap_or(-1)),
            )
        })
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );

    let labels_case = "ContainsPiiEntities over both routes";
    let labelled = comprehend.ok("ContainsPiiEntities", &json_request()).await;
    let expected: Vec<String> = list(&labelled, "Labels")
        .iter()
        .map(|label| text(label, "Name"))
        .collect();
    let output: ContainsPiiEntitiesOutput = over_cbor(
        &comprehend,
        labels_case,
        "ContainsPiiEntities",
        cbor_body(
            CONTAINS_PII_ENTITIES_INPUT_SCHEMA,
            &ContainsPiiEntitiesInput {
                text: DOCUMENT.to_string(),
                language_code: "en".to_string(),
            },
        ),
    )
    .await;
    let actual: Vec<String> = output
        .labels
        .unwrap_or_default()
        .into_iter()
        .map(|label| label.name.unwrap_or_default())
        .collect();
    assert_eq!(
        actual, expected,
        "case {labels_case:?}: expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn the_syntax_tokens_are_the_same_on_both_routes() {
    let case = "DetectSyntax over both routes";
    let comprehend = Comprehend::new();
    let over_json = comprehend.ok("DetectSyntax", &json_request()).await;
    let expected: Vec<(i64, String, String, i64, i64)> = list(&over_json, "SyntaxTokens")
        .iter()
        .map(|token| {
            (
                number(token, "TokenId"),
                text(token, "Text"),
                text(&token["PartOfSpeech"], "Tag"),
                number(token, "BeginOffset"),
                number(token, "EndOffset"),
            )
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "case {case:?}: the document carries tokens, so the comparison must have rows, got \
         {over_json}"
    );

    let output: DetectSyntaxOutput = over_cbor(
        &comprehend,
        case,
        "DetectSyntax",
        cbor_body(
            DETECT_SYNTAX_INPUT_SCHEMA,
            &DetectSyntaxInput {
                text: DOCUMENT.to_string(),
                language_code: "en".to_string(),
            },
        ),
    )
    .await;
    let actual: Vec<(i64, String, String, i64, i64)> = output
        .syntax_tokens
        .unwrap_or_default()
        .into_iter()
        .map(|token| {
            (
                i64::from(token.token_id.unwrap_or(-1)),
                token.text.unwrap_or_default(),
                token
                    .part_of_speech
                    .and_then(|part| part.tag)
                    .unwrap_or_default(),
                i64::from(token.begin_offset.unwrap_or(-1)),
                i64::from(token.end_offset.unwrap_or(-1)),
            )
        })
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

/// `DetectToxicContent` takes a LIST of segments and answers a list of per-segment label
/// lists, so it is the row where a nested list has to survive the codec.
#[tokio::test]
async fn the_toxicity_labels_are_the_same_on_both_routes() {
    let case = "DetectToxicContent over both routes";
    let comprehend = Comprehend::new();
    // Two segments, so the answer is a list of more than one element and their ORDER is
    // part of what the comparison pins.
    let segments = [DOCUMENT, "The weather in London is rather pleasant today."];
    let over_json = comprehend
        .ok(
            "DetectToxicContent",
            &json!({
                "LanguageCode": "en",
                "TextSegments": segments.iter().map(|text| json!({"Text": text})).collect::<Vec<_>>(),
            }),
        )
        .await;
    let expected: Vec<Vec<String>> = list(&over_json, "ResultList")
        .iter()
        .map(|result| {
            list(result, "Labels")
                .iter()
                .map(|label| text(label, "Name"))
                .collect()
        })
        .collect();
    assert_eq!(
        expected.len(),
        segments.len(),
        "case {case:?}: one result per submitted segment, got {over_json}"
    );

    let output: DetectToxicContentOutput = over_cbor(
        &comprehend,
        case,
        "DetectToxicContent",
        cbor_body(
            DETECT_TOXIC_CONTENT_INPUT_SCHEMA,
            &DetectToxicContentInput {
                language_code: "en".to_string(),
                text_segments: segments
                    .iter()
                    .map(|text| TextSegment {
                        text: (*text).to_string(),
                    })
                    .collect(),
            },
        ),
    )
    .await;
    let actual: Vec<Vec<String>> = output
        .result_list
        .unwrap_or_default()
        .into_iter()
        .map(|result| {
            result
                .labels
                .unwrap_or_default()
                .into_iter()
                .map(|label| label.name.unwrap_or_default())
                .collect()
        })
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}
