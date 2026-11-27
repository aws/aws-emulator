// SPDX-License-Identifier: CC0-1.0

//! The `rpcv2Cbor` route of this service, end to end over the wire.
//!
//! The model declares TWO protocol traits — `awsJson1_1` and `smithy.protocols#rpcv2Cbor`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-comprehend-comprehend-20171127/smithy-model/com-amazonaws-comprehend-comprehend-20171127.json")`)
//! — and a Smithy service that declares several serves a client that speaks any of them.
//!
//! RULE: a service declaring more than one protocol serves every operation it binds over
//! each of them; the marker the REQUEST carries chooses which — `rpcv2Cbor` by its
//! `POST /service/{Service}/operation/{Operation}` path or its `smithy-protocol` header,
//! `awsJson1_1` by its `X-Amz-Target` — and the protocol only FRAMES the call: the input
//! decodes to the same handler against the same store, and the answer, success and failure
//! alike, carries the same values and the same status encoded in the protocol the request
//! named, under that protocol's own media type.
//!
//! Each suite asserts that over a SET of inputs rather than one call. The rows differ along
//! what the rule generalises over:
//!
//! * the OUTPUT SHAPE a success is encoded from — an enum value beside a nested structure of
//!   floating-point members, a list of structures, two lists of which one is empty, a list of
//!   key/value structures, and a nested structure carrying timestamps, whose CBOR form (a
//!   tagged epoch value) is not its JSON one at all;
//! * whether the request carries a body at all, and whether it names the protocol by header or
//!   by path alone;
//! * which route WROTE a resource and which route READ it back;
//! * the LAYER a failure came from — a handler's modelled error, a refusal this service words
//!   ahead of the handler, the framework's own constraint walk, and the scaffold channel — and
//!   the status it travels under (400, 501);
//! * the operation NAME a request addresses that this service binds no operation for.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use serde_json::{json, Value};

use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    BatchDetectSentimentInput, BatchDetectSentimentOutput, CreateDocumentClassifierInput,
    CreateDocumentClassifierOutput, DescribeDocumentClassifierInput,
    DescribeDocumentClassifierOutput, DetectDominantLanguageInput, DetectDominantLanguageOutput,
    DetectSentimentInput, DetectSentimentOutput, DetectTargetedSentimentInput,
    DocumentClassifierInputDataConfig, ListDatasetsInput, ListEndpointsInput, ListEndpointsOutput,
    ListTagsForResourceInput, ListTagsForResourceOutput, Tag, TagResourceInput,
    BATCH_DETECT_SENTIMENT_INPUT_SCHEMA, CREATE_DOCUMENT_CLASSIFIER_INPUT_SCHEMA,
    DESCRIBE_DOCUMENT_CLASSIFIER_INPUT_SCHEMA, DETECT_DOMINANT_LANGUAGE_INPUT_SCHEMA,
    DETECT_SENTIMENT_INPUT_SCHEMA, DETECT_TARGETED_SENTIMENT_INPUT_SCHEMA,
    LIST_DATASETS_INPUT_SCHEMA, LIST_ENDPOINTS_INPUT_SCHEMA, LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
    TAG_RESOURCE_INPUT_SCHEMA,
};

use crate::comprehend_client::{
    cbor_uri, error_code, error_message, json_target, Comprehend, CBOR_CONTENT_TYPE, CBOR_FRAMING,
    CBOR_PROTOCOL, JSON_CONTENT_TYPE, JSON_URI,
};

/// The IAM role every create below names. Its value is beside the point: the suite is
/// about the wire, and the create only has to be a call the service accepts.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ComprehendRole";

/// An English document with an unambiguous positive sentiment.
const POSITIVE: &str = "This is a wonderful and delightful day, and I am very happy.";

/// An English document with an unambiguous negative sentiment.
const NEGATIVE: &str = "This is an awful, terrible disaster and I am extremely angry.";

/// The per-document quota `DetectSentiment` refuses a longer document with, in UTF-8
/// bytes (`crate::state::REALTIME_TEXT_QUOTA_BYTES`, asserted as the operation's own
/// quota in `realtime_text_quotas.rs`). Used here only to build a document one byte
/// past it, so this suite pins the refusal's wording against the OTHER route rather
/// than against a number.
const REALTIME_TEXT_QUOTA_BYTES: usize = 5_000;

// =====================================================================================
// Reading and writing the CBOR wire
// =====================================================================================

/// `input` as the `rpcv2Cbor` request body a client sends: written through the protocol's
/// own codec against the operation's input schema, so nothing here encodes CBOR by hand.
fn cbor_body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// The answer `body` decoded against `O`'s own schema, as a client decodes one.
fn decoded<O: DeserializableStruct>(case: &str, body: &[u8]) -> O {
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    O::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the answer must decode against its output schema, got {error}")
    })
}

/// The TEXT entries of the CBOR map `body` carries, in the order the map carries them,
/// or `None` when it is no CBOR map at all.
///
/// Read with the CBOR implementation the framework's own codec is built on, because the
/// members of an ANSWERED error map are not known from a shape: the schema-driven reader
/// can only read a body against a shape it knows, and what a client reads here is every
/// member the answer wrote. An item that is not text is read PAST rather than stopping
/// the read.
fn cbor_entries(body: &[u8]) -> Option<Vec<(String, String)>> {
    let mut decoder = minicbor::Decoder::new(body);
    // A map of a declared length, or an indefinite one closed by a break item: an
    // encoder is free to write either, and the framework's own codec writes the second.
    let declared = decoder.map().ok()?;
    let mut entries = Vec::new();
    let mut read = 0_u64;
    loop {
        match declared {
            Some(count) if read == count => break,
            None if matches!(decoder.datatype().ok()?, minicbor::data::Type::Break) => {
                decoder.skip().ok()?;
                break;
            }
            _ => {}
        }
        let key = decoder.str().ok()?.to_string();
        if matches!(decoder.datatype().ok()?, minicbor::data::Type::String) {
            entries.push((key, decoder.str().ok()?.to_string()));
        } else {
            decoder.skip().ok()?;
        }
        read = read.saturating_add(1);
    }
    Some(entries)
}

/// The error map of a CBOR answer, panicking when the body is not one.
fn error_map(case: &str, body: &[u8]) -> Vec<(String, String)> {
    cbor_entries(body).unwrap_or_else(|| {
        panic!("case {case:?}: the failure body must decode as a CBOR map, got {body:?}")
    })
}

/// The value of `name` on `response`, or `"<absent>"`.
fn header(response: &::http::Response<bytes::Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<absent>")
        .to_string()
}

/// Assert that `response` is framed the way the `rpcv2Cbor` specification requires an
/// answer of that route to be: the CBOR media type, and the `smithy-protocol` header
/// naming the protocol. A client that reads neither cannot tell the answer apart from
/// another protocol's.
fn assert_cbor_framing(case: &str, response: &::http::Response<bytes::Bytes>) {
    assert_eq!(
        header(response, "content-type"),
        CBOR_CONTENT_TYPE,
        "case {case:?}: expected the CBOR media type, got {:?}",
        header(response, "content-type")
    );
    assert_eq!(
        header(response, "smithy-protocol"),
        CBOR_PROTOCOL,
        "case {case:?}: expected the protocol named on the answer, got {:?}",
        header(response, "smithy-protocol")
    );
}

/// One `rpcv2Cbor` call that must succeed: its framing checked, its body handed back.
async fn cbor_ok(
    comprehend: &Comprehend,
    case: &str,
    operation: &str,
    body: bytes::Bytes,
) -> bytes::Bytes {
    let response = comprehend.cbor(operation, body).await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200 over {CBOR_PROTOCOL}, got {status} with \
         {:?}",
        cbor_entries(response.body().as_ref())
    );
    assert_cbor_framing(case, &response);
    response.into_body()
}

// =====================================================================================
// A classifier to read back, written over each route
// =====================================================================================

/// The `CreateDocumentClassifier` input this suite writes a classifier with.
///
/// The training itself cannot be carried out here (this suite's emulator mounts no object
/// storage), which is beside the point: the create persists the model record, and the
/// record is what a read reports.
fn create_input(name: &str) -> CreateDocumentClassifierInput {
    CreateDocumentClassifierInput {
        document_classifier_name: name.to_string(),
        data_access_role_arn: ROLE_ARN.to_string(),
        language_code: "en".to_string(),
        input_data_config: DocumentClassifierInputDataConfig {
            s3_uri: Some(format!("s3://cbor-route-bucket/{name}.csv")),
            ..DocumentClassifierInputDataConfig::default()
        },
        ..CreateDocumentClassifierInput::default()
    }
}

/// The same create as a JSON request body, member for member.
fn create_json(name: &str) -> Value {
    json!({
        "DocumentClassifierName": name,
        "DataAccessRoleArn": ROLE_ARN,
        "LanguageCode": "en",
        "InputDataConfig": { "S3Uri": format!("s3://cbor-route-bucket/{name}.csv") },
    })
}

/// Write a classifier over the CBOR route and hand back the ARN the create minted.
async fn create_over_cbor(comprehend: &Comprehend, case: &str, name: &str) -> String {
    let answered = cbor_ok(
        comprehend,
        case,
        "CreateDocumentClassifier",
        cbor_body(CREATE_DOCUMENT_CLASSIFIER_INPUT_SCHEMA, &create_input(name)),
    )
    .await;
    let output: CreateDocumentClassifierOutput = decoded(case, answered.as_ref());
    output.document_classifier_arn.unwrap_or_else(|| {
        panic!("case {case:?}: the create must report the ARN it minted, got no member")
    })
}

/// Write a classifier over the JSON route and hand back the ARN the create minted.
async fn create_over_json(comprehend: &Comprehend, case: &str, name: &str) -> String {
    let output = comprehend
        .ok("CreateDocumentClassifier", &create_json(name))
        .await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the create must report the ARN it minted, got {output}")
        })
        .to_string()
}

// =====================================================================================
// Every output shape, encoded by the CBOR codec
// =====================================================================================

/// One row of the success table: a call over the CBOR route, and what a client must be
/// able to read out of the answer.
struct Success {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The operation the row calls.
    operation: &'static str,
    /// The body of the call, already encoded.
    body: bytes::Bytes,
    /// Decodes the answer against its own output schema and asserts its contents.
    check: fn(&str, &[u8]),
}

fn check_sentiment(case: &str, body: &[u8]) {
    let output: DetectSentimentOutput = decoded(case, body);
    assert_eq!(
        output.sentiment.as_deref(),
        Some("POSITIVE"),
        "case {case:?}: expected the arg-max sentiment of the document, got {:?}",
        output.sentiment
    );
    let score = output.sentiment_score.unwrap_or_else(|| {
        panic!("case {case:?}: the answer must carry the nested confidence structure")
    });
    let positive = score.positive.unwrap_or_default();
    assert!(
        positive > 0.5,
        "case {case:?}: the winning confidence must decode as the float it was encoded \
         from, got {positive}"
    );
}

fn check_languages(case: &str, body: &[u8]) {
    let output: DetectDominantLanguageOutput = decoded(case, body);
    let languages = output.languages.unwrap_or_default();
    assert!(
        !languages.is_empty(),
        "case {case:?}: the answer must carry the ranked language list, got none"
    );
    assert_eq!(
        languages[0].language_code.as_deref(),
        Some("en"),
        "case {case:?}: expected English first in the ranking, got {:?}",
        languages
            .iter()
            .map(|language| language.language_code.clone())
            .collect::<Vec<_>>()
    );
}

fn check_batch_sentiment(case: &str, body: &[u8]) {
    let output: BatchDetectSentimentOutput = decoded(case, body);
    let sentiments: Vec<Option<String>> = output
        .result_list
        .iter()
        .map(|item| item.sentiment.clone())
        .collect();
    assert_eq!(
        sentiments,
        vec![
            Some("POSITIVE".to_string()),
            Some("NEGATIVE".to_string())
        ],
        "case {case:?}: every submitted document must come back in order, got {sentiments:?}"
    );
    assert!(
        output.error_list.is_empty(),
        "case {case:?}: no document failed, so the error list must decode as empty, got {:?}",
        output.error_list
    );
}

fn check_empty_endpoint_list(case: &str, body: &[u8]) {
    let output: ListEndpointsOutput = decoded(case, body);
    assert_eq!(
        output.endpoint_properties_list.unwrap_or_default().len(),
        0,
        "case {case:?}: nothing created an endpoint, so the list must decode as empty"
    );
    assert_eq!(
        output.next_token, None,
        "case {case:?}: the last page carries no token, got {:?}",
        output.next_token
    );
}

/// A document one byte past the per-document quota, which is the longest request this
/// suite sends.
fn oversized_document() -> String {
    "a".repeat(REALTIME_TEXT_QUOTA_BYTES + 1)
}

#[tokio::test]
async fn every_output_shape_is_encoded_by_the_protocol_the_request_named() {
    let comprehend = Comprehend::new();
    for row in [
        Success {
            case: "an enum value beside a nested structure of floating-point members",
            operation: "DetectSentiment",
            body: cbor_body(
                DETECT_SENTIMENT_INPUT_SCHEMA,
                &DetectSentimentInput {
                    text: POSITIVE.to_string(),
                    language_code: "en".to_string(),
                },
            ),
            check: check_sentiment,
        },
        Success {
            case: "a list of structures",
            operation: "DetectDominantLanguage",
            body: cbor_body(
                DETECT_DOMINANT_LANGUAGE_INPUT_SCHEMA,
                &DetectDominantLanguageInput {
                    text: POSITIVE.to_string(),
                },
            ),
            check: check_languages,
        },
        Success {
            case: "two lists, one of them empty",
            operation: "BatchDetectSentiment",
            body: cbor_body(
                BATCH_DETECT_SENTIMENT_INPUT_SCHEMA,
                &BatchDetectSentimentInput {
                    text_list: vec![POSITIVE.to_string(), NEGATIVE.to_string()],
                    language_code: "en".to_string(),
                },
            ),
            check: check_batch_sentiment,
        },
        Success {
            case: "an empty list and an absent pagination token",
            operation: "ListEndpoints",
            body: cbor_body(LIST_ENDPOINTS_INPUT_SCHEMA, &ListEndpointsInput::default()),
            check: check_empty_endpoint_list,
        },
    ] {
        let answered = cbor_ok(&comprehend, row.case, row.operation, row.body).await;
        (row.check)(row.case, answered.as_ref());
    }
}

/// An operation whose input members are every one optional may be called with NO body at
/// all on this route: the protocol reads an absent body as the empty map.
#[tokio::test]
async fn a_call_with_no_body_at_all_is_the_empty_input() {
    let case = "no request body";
    let comprehend = Comprehend::new();
    let response = comprehend
        .framed(&cbor_uri("ListEndpoints"), bytes::Bytes::new(), CBOR_FRAMING)
        .await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {:?}",
        cbor_entries(response.body().as_ref())
    );
    assert_cbor_framing(case, &response);
    check_empty_endpoint_list(case, response.into_body().as_ref());
}

// =====================================================================================
// The request chooses its protocol
// =====================================================================================

/// The same call framed three ways: a service declaring two protocols answers each
/// request in the one that request named, and in no other.
#[tokio::test]
async fn the_marker_the_request_carries_chooses_the_protocol_of_the_answer() {
    let comprehend = Comprehend::new();
    let target = json_target("DetectSentiment");
    let cbor = cbor_body(
        DETECT_SENTIMENT_INPUT_SCHEMA,
        &DetectSentimentInput {
            text: POSITIVE.to_string(),
            language_code: "en".to_string(),
        },
    );
    let as_json = bytes::Bytes::from(
        serde_json::to_vec(&json!({"Text": POSITIVE, "LanguageCode": "en"}))
            .expect("the JSON body serializes"),
    );
    // Each row: what names the protocol, where the call is addressed, the headers it
    // carries, the body, and the media type the answer must come back under.
    for (case, uri, headers, body, content_type) in [
        (
            "the CBOR path and the protocol header",
            cbor_uri("DetectSentiment"),
            CBOR_FRAMING.to_vec(),
            cbor.clone(),
            CBOR_CONTENT_TYPE,
        ),
        (
            "the CBOR path alone, with no protocol header",
            cbor_uri("DetectSentiment"),
            vec![("content-type", CBOR_CONTENT_TYPE)],
            cbor.clone(),
            CBOR_CONTENT_TYPE,
        ),
        (
            "the protocol header alone, on the CBOR path",
            cbor_uri("DetectSentiment"),
            vec![("smithy-protocol", CBOR_PROTOCOL)],
            cbor.clone(),
            CBOR_CONTENT_TYPE,
        ),
        (
            "the JSON target header, posted to the root",
            JSON_URI.to_string(),
            vec![
                ("content-type", JSON_CONTENT_TYPE),
                ("x-amz-target", target.as_str()),
            ],
            as_json,
            JSON_CONTENT_TYPE,
        ),
    ] {
        let response = comprehend.framed(&uri, body, &headers).await;
        let status = response.status().as_u16();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {:?}",
            String::from_utf8_lossy(response.body().as_ref())
        );
        assert_eq!(
            header(&response, "content-type"),
            content_type,
            "case {case:?}: the answer must come back in the protocol the request named, got \
             {:?}",
            header(&response, "content-type")
        );
        // Whichever framing carried it, the answer states the same sentiment.
        let sentiment = if content_type == CBOR_CONTENT_TYPE {
            assert_cbor_framing(case, &response);
            decoded::<DetectSentimentOutput>(case, response.body().as_ref()).sentiment
        } else {
            let parsed: Value = serde_json::from_slice(response.body().as_ref())
                .expect("the JSON answer parses");
            parsed["Sentiment"].as_str().map(str::to_string)
        };
        assert_eq!(
            sentiment.as_deref(),
            Some("POSITIVE"),
            "case {case:?}: the handler's answer does not depend on the framing, got \
             {sentiment:?}"
        );
    }
}

// =====================================================================================
// One store, whichever route reached it
// =====================================================================================

/// A resource is state of the service, not of a wire: what one route wrote, the other
/// reads back unchanged.
///
/// The rows differ along which route wrote and which read, and the read side is an
/// implemented `Describe`, so the create is proved by reading the resource back.
#[tokio::test]
async fn a_resource_written_over_one_route_is_read_over_the_other() {
    let comprehend = Comprehend::new();

    let written_over_cbor = create_over_cbor(&comprehend, "written over CBOR", "cbor-written").await;
    let written_over_json = create_over_json(&comprehend, "written over JSON", "json-written").await;

    for (case, arn, name) in [
        (
            "written over CBOR, read over CBOR",
            written_over_cbor.clone(),
            "cbor-written",
        ),
        (
            "written over JSON, read over CBOR",
            written_over_json.clone(),
            "json-written",
        ),
    ] {
        let answered = cbor_ok(
            &comprehend,
            case,
            "DescribeDocumentClassifier",
            cbor_body(
                DESCRIBE_DOCUMENT_CLASSIFIER_INPUT_SCHEMA,
                &DescribeDocumentClassifierInput {
                    document_classifier_arn: arn.clone(),
                },
            ),
        )
        .await;
        let output: DescribeDocumentClassifierOutput = decoded(case, answered.as_ref());
        let properties = output.document_classifier_properties.unwrap_or_else(|| {
            panic!("case {case:?}: the read must report the classifier's properties")
        });
        assert_eq!(
            properties.document_classifier_arn.as_deref(),
            Some(arn.as_str()),
            "case {case:?}: the read reports the resource the create minted, got {:?}",
            properties.document_classifier_arn
        );
        assert_eq!(
            properties.language_code.as_deref(),
            Some("en"),
            "case {case:?}: the read reports what was persisted, got {:?}",
            properties.language_code
        );
        assert!(
            properties
                .document_classifier_arn
                .as_deref()
                .is_some_and(|reported| reported.ends_with(name)),
            "case {case:?}: the ARN names the classifier that was created, got {:?}",
            properties.document_classifier_arn
        );
        // A timestamp's CBOR form is a tagged epoch value and its JSON form is a
        // number, so a member that decodes at all here proves the codec wrote the
        // protocol's own form.
        assert!(
            properties.submit_time.is_some(),
            "case {case:?}: the read reports the instant the create was submitted, got none"
        );
    }

    for (case, arn) in [
        ("written over CBOR, read over JSON", written_over_cbor),
        ("written over JSON, read over JSON", written_over_json),
    ] {
        let output = comprehend
            .ok(
                "DescribeDocumentClassifier",
                &json!({"DocumentClassifierArn": arn}),
            )
            .await;
        assert_eq!(
            output["DocumentClassifierProperties"]["DocumentClassifierArn"]
                .as_str()
                .unwrap_or_default(),
            arn,
            "case {case:?}: the read reports the resource the create minted, got {output}"
        );
    }
}

/// Tags written over one route are the tags the other route lists, key for key.
#[tokio::test]
async fn tags_written_over_one_route_are_listed_by_the_other() {
    let case = "tags written over CBOR, listed over JSON and back over CBOR";
    let comprehend = Comprehend::new();
    let arn = create_over_cbor(&comprehend, case, "cbor-tagged").await;
    // Two entries, one of them with no value at all, because `Tag$Value` is optional.
    let written = vec![
        Tag {
            key: "route".to_string(),
            value: Some("cbor".to_string()),
        },
        Tag {
            key: "valueless".to_string(),
            value: None,
        },
    ];
    cbor_ok(
        &comprehend,
        case,
        "TagResource",
        cbor_body(
            TAG_RESOURCE_INPUT_SCHEMA,
            &TagResourceInput {
                resource_arn: arn.clone(),
                tags: written.clone(),
            },
        ),
    )
    .await;

    let listed_over_json = comprehend
        .ok("ListTagsForResource", &json!({"ResourceArn": arn}))
        .await;
    let over_json: Vec<(String, Option<String>)> = listed_over_json["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().map(str::to_string),
            )
        })
        .collect();

    let answered = cbor_ok(
        &comprehend,
        case,
        "ListTagsForResource",
        cbor_body(
            LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
            &ListTagsForResourceInput {
                resource_arn: arn.clone(),
            },
        ),
    )
    .await;
    let output: ListTagsForResourceOutput = decoded(case, answered.as_ref());
    let over_cbor: Vec<(String, Option<String>)> = output
        .tags
        .unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect();

    let expected: Vec<(String, Option<String>)> = written
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect();
    assert_eq!(
        over_cbor, expected,
        "case {case:?}: expected the tags that were written, got {over_cbor:?}"
    );
    assert_eq!(
        over_json, expected,
        "case {case:?}: the other route lists the same tags, got {over_json:?}"
    );
    assert_eq!(
        output.resource_arn.as_deref(),
        Some(arn.as_str()),
        "case {case:?}: the answer names the resource that was asked for, got {:?}",
        output.resource_arn
    );
}

// =====================================================================================
// Failures
// =====================================================================================

/// One row of the failure table: the same failing call over both routes.
struct Failure {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The operation the row calls.
    operation: &'static str,
    /// The call as a CBOR body.
    cbor: bytes::Bytes,
    /// The same call as a JSON body.
    json: Value,
    /// The error code a client reads, whichever route answered.
    code: &'static str,
    /// The status the failure travels under, whichever route answered.
    status: u16,
}

/// Every failing call, over both routes.
///
/// The rows differ along the LAYER the refusal came from and the status it carries, so
/// the rule is asserted over the whole span of ways this service can fail a call rather
/// than over one of them.
fn failures() -> Vec<Failure> {
    let absent_arn =
        "arn:aws:comprehend:eu-west-1:000000000000:document-classifier/none-of-this-name";
    let oversized = oversized_document();
    vec![
        Failure {
            case: "a modelled not-found a handler raised",
            operation: "DescribeDocumentClassifier",
            cbor: cbor_body(
                DESCRIBE_DOCUMENT_CLASSIFIER_INPUT_SCHEMA,
                &DescribeDocumentClassifierInput {
                    document_classifier_arn: absent_arn.to_string(),
                },
            ),
            json: json!({"DocumentClassifierArn": absent_arn}),
            code: "ResourceNotFoundException",
            status: 400,
        },
        Failure {
            case: "a per-document quota the handler reports",
            operation: "DetectSentiment",
            cbor: cbor_body(
                DETECT_SENTIMENT_INPUT_SCHEMA,
                &DetectSentimentInput {
                    text: oversized.clone(),
                    language_code: "en".to_string(),
                },
            ),
            json: json!({"Text": oversized, "LanguageCode": "en"}),
            code: "TextSizeLimitExceededException",
            status: 400,
        },
        Failure {
            case: "a language outside the operation's own set, refused ahead of the handler",
            operation: "DetectTargetedSentiment",
            cbor: cbor_body(
                DETECT_TARGETED_SENTIMENT_INPUT_SCHEMA,
                &DetectTargetedSentimentInput {
                    text: POSITIVE.to_string(),
                    language_code: "fr".to_string(),
                },
            ),
            json: json!({"Text": POSITIVE, "LanguageCode": "fr"}),
            code: "ValidationException",
            status: 400,
        },
        Failure {
            case: "a value under the length bound its own model binds",
            operation: "DetectSentiment",
            cbor: cbor_body(
                DETECT_SENTIMENT_INPUT_SCHEMA,
                &DetectSentimentInput {
                    text: String::new(),
                    language_code: "en".to_string(),
                },
            ),
            json: json!({"Text": "", "LanguageCode": "en"}),
            code: "ValidationException",
            status: 400,
        },
        Failure {
            case: "an operation of this service that is still a scaffold",
            operation: "ListDatasets",
            cbor: cbor_body(LIST_DATASETS_INPUT_SCHEMA, &ListDatasetsInput::default()),
            json: json!({}),
            code: "NotImplemented",
            status: 501,
        },
    ]
}

/// A failure is the same failure on either route: the same class, the same sentence and
/// the same status, encoded in the protocol the request named.
#[tokio::test]
async fn a_failure_states_the_same_class_and_sentence_on_both_routes() {
    let comprehend = Comprehend::new();
    for row in failures() {
        let case = row.case;

        let (json_status, json_body) = comprehend.call(row.operation, &row.json).await;
        assert_eq!(
            json_status, row.status,
            "case {case:?}: expected status {} on the JSON route, got {json_status} with \
             {json_body}",
            row.status
        );
        assert_eq!(
            error_code(&json_body),
            row.code,
            "case {case:?}: expected {} on the JSON route, got {json_body}",
            row.code
        );

        let response = comprehend.cbor(row.operation, row.cbor.clone()).await;
        let status = response.status().as_u16();
        let answered = error_map(case, response.body().as_ref());
        assert_eq!(
            status, row.status,
            "case {case:?}: expected status {} on the CBOR route, got {status} with \
             {answered:?}",
            row.status
        );
        assert_cbor_framing(case, &response);
        // The class, as a client matching on it reads it: the discriminator with any
        // `namespace#` prefix stripped, exactly as on the other route.
        let class = answered
            .iter()
            .find(|(key, _)| key == "__type")
            .map(|(_, value)| value.rsplit('#').next().unwrap_or(value).to_string())
            .unwrap_or_else(|| panic!("case {case:?}: the map must name the class, got {answered:?}"));
        assert_eq!(
            class, row.code,
            "case {case:?}: expected the class {}, got {answered:?}",
            row.code
        );
        // The sentence is the failure's own, so it is read off the SAME failure in the
        // other envelope rather than restated here.
        let sentence = answered
            .iter()
            .find(|(key, _)| key == "Message" || key == "message")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert_eq!(
            sentence,
            error_message(&json_body),
            "case {case:?}: the sentence does not depend on the framing, got {answered:?}"
        );
    }
}

/// A request addressed to an operation this service binds none of is refused on the CBOR
/// path, framed as that protocol requires, with the routing class and nothing served.
///
/// The rows differ along WHY the name resolves to nothing: a name no API has, the name of
/// another service's operation, and a real operation's name in the wrong case.
#[tokio::test]
async fn an_operation_this_service_does_not_bind_is_refused_on_the_cbor_path() {
    let comprehend = Comprehend::new();
    for (case, operation) in [
        ("a name no API has", "NoSuchOperation"),
        ("another service's operation", "PutMetricData"),
        ("a bound operation's name in the wrong case", "detectSentiment"),
    ] {
        let response = comprehend.cbor(operation, bytes::Bytes::new()).await;
        let status = response.status().as_u16();
        let answered = error_map(case, response.body().as_ref());
        // 404, not 501: mandated by the rpcv2Cbor operation-routing rules.
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {answered:?}"
        );
        assert_cbor_framing(case, &response);
        let class = answered
            .iter()
            .find(|(key, _)| key == "__type")
            .map(|(_, value)| value.rsplit('#').next().unwrap_or(value).to_string())
            .unwrap_or_default();
        assert_eq!(
            class, "UnknownOperationError",
            "case {case:?}: expected the routing class, got {answered:?}"
        );
    }
}
