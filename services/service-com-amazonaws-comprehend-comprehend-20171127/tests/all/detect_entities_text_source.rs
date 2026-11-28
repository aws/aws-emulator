// SPDX-License-Identifier: CC0-1.0

//! Which input member `DetectEntities` takes the text it analyses from, when no
//! custom-model endpoint is named.
//!
//! THE RULE, asserted over a TABLE of member combinations rather than one recorded
//! call: on the pre-trained path the analysed text is the `Text` member and nothing
//! else — `Bytes` carries an input document for a custom entity recognition model
//! reached through an `EndpointArn` ("This field applies only when you use a custom
//! entity recognition model that was trained with PDF annotations. For other cases,
//! enter your text input in the Text field",
//! `smithy_query(query="shape",shape="DetectEntitiesRequest")`). So a request naming
//! no endpoint and no `Text` carries no text at all, whatever its `Bytes` holds, and
//! is refused with HTTP 400 `ValidationException` and the message
//! [`NO_TEXT_MESSAGE`]; a request that carries `Text` is analysed whatever else it
//! carries, and answers that text's entities.

use aws_smithy_types::base64;
use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend};

/// The error code an input-validation refusal carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The message the service refuses a request carrying no text with: fixed text, with
/// nothing substituted from the request.
const NO_TEXT_MESSAGE: &str = "Request text cannot be null";

/// The document the accepted rows submit in `Text`, written so several rule classes
/// of the recogniser find a mention in it.
const DOCUMENT: &str = "Jane Doe of Acme Widgets LLC visited Seattle on 2024-01-15.";

/// A DIFFERENT document for the `Bytes` member, so a response built from `Bytes`
/// rather than from `Text` would show up in the entities reported.
const OTHER_DOCUMENT: &str = "Grace Hopper joined Example Corp in Boston on 2019-03-04.";

/// A PDF input document: the `%PDF-` header its own specification defines.
const PDF_DOCUMENT: &[u8] = b"%PDF-1.7 1 0 obj << /Type /Catalog >>";

/// An input document that is not text at all.
const BINARY_DOCUMENT: &[u8] = &[0xff, 0xfe, 0xfd, 0xfc, 0x00, 0x01];

/// A `DocumentReaderConfig` for the rows that carry one: `DocumentReadAction` is its
/// one required member and `TEXTRACT_DETECT_DOCUMENT_TEXT` one of its two modelled
/// values (`smithy_query(query="shape",shape="DocumentReaderConfig")`).
fn reader_config() -> Value {
    json!({ "DocumentReadAction": "TEXTRACT_DETECT_DOCUMENT_TEXT" })
}

/// The `Entities` of a successful answer.
fn entities(output: &Value, case: &str) -> Vec<Value> {
    output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
        .clone()
}

/// Every request that carries `Text` is analysed, whatever else it carries, and
/// answers the entities of THAT text.
///
/// The rows differ along what the rule generalises over: whether a `Bytes` document
/// is there at all, what kind of document it is (plain text, a PDF, bytes that are
/// not text), and whether a `DocumentReaderConfig` for extracting it is there too.
#[tokio::test]
async fn a_request_carrying_text_is_analysed_whatever_else_it_carries() {
    let comprehend = Comprehend::new();

    let baseline = comprehend
        .ok(
            "DetectEntities",
            &json!({ "Text": DOCUMENT, "LanguageCode": "en" }),
        )
        .await;
    let expected = entities(&baseline, "Text alone");
    assert!(
        !expected.is_empty(),
        "case \"Text alone\": expected the document's entities, got {baseline}"
    );

    for (case, body) in [
        (
            "Text alone",
            json!({ "Text": DOCUMENT, "LanguageCode": "en" }),
        ),
        (
            "Text beside a plain-text Bytes document",
            json!({
                "Text": DOCUMENT,
                "Bytes": base64::encode(OTHER_DOCUMENT),
                "LanguageCode": "en",
            }),
        ),
        (
            "Text beside a PDF Bytes document",
            json!({
                "Text": DOCUMENT,
                "Bytes": base64::encode(PDF_DOCUMENT),
                "LanguageCode": "en",
            }),
        ),
        (
            "Text beside Bytes that are not text at all",
            json!({
                "Text": DOCUMENT,
                "Bytes": base64::encode(BINARY_DOCUMENT),
                "LanguageCode": "en",
            }),
        ),
        (
            "Text beside a Bytes document and a reader configuration",
            json!({
                "Text": DOCUMENT,
                "Bytes": base64::encode(PDF_DOCUMENT),
                "DocumentReaderConfig": reader_config(),
                "LanguageCode": "en",
            }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectEntities", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
        assert_eq!(
            entities(&response, case),
            expected,
            "case {case:?}: expected the entities of the Text member, got {response}"
        );
        // The extraction members report an input document the service read text out
        // of, which the pre-trained path never does: its text came from `Text`.
        for member in ["DocumentMetadata", "DocumentType", "Blocks", "Errors"] {
            assert!(
                response[member].is_null(),
                "case {case:?}: {member} reports a document extraction this request \
                 asked for none of, got {response}"
            );
        }
    }
}

/// A request naming no endpoint and no `Text` carries no text at all, whatever its
/// `Bytes` holds, and is refused as an input-validation failure.
///
/// The rows differ along the same axes as the accepted ones: no document member at
/// all, and each kind of `Bytes` document, with and without a reader configuration.
#[tokio::test]
async fn a_request_with_no_text_is_refused_whatever_its_bytes_hold() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("neither Text nor Bytes", json!({ "LanguageCode": "en" })),
        (
            "a plain-text Bytes document alone",
            json!({ "Bytes": base64::encode(OTHER_DOCUMENT), "LanguageCode": "en" }),
        ),
        (
            "a PDF Bytes document alone",
            json!({ "Bytes": base64::encode(PDF_DOCUMENT), "LanguageCode": "en" }),
        ),
        (
            "Bytes that are not text at all",
            json!({ "Bytes": base64::encode(BINARY_DOCUMENT), "LanguageCode": "en" }),
        ),
        (
            "a Bytes document with a reader configuration",
            json!({
                "Bytes": base64::encode(PDF_DOCUMENT),
                "DocumentReaderConfig": reader_config(),
                "LanguageCode": "en",
            }),
        ),
        (
            "a Bytes document and no LanguageCode",
            json!({ "Bytes": base64::encode(OTHER_DOCUMENT) }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectEntities", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {response}"
        );
        assert_eq!(
            error_message(&response),
            NO_TEXT_MESSAGE,
            "case {case:?}: expected the no-text refusal, got {response}"
        );
        assert!(
            response["Entities"].is_null(),
            "case {case:?}: a refused request reports no entities, got {response}"
        );
    }
}

/// The same document submitted twice answers the same entities, and submitting it
/// beside a `Bytes` document changes nothing a later caller reads: the answer is a
/// function of the `Text` member alone.
#[tokio::test]
async fn the_answer_is_a_function_of_the_text_member_alone() {
    let comprehend = Comprehend::new();
    let plain = json!({ "Text": DOCUMENT, "LanguageCode": "en" });
    let with_bytes = json!({
        "Text": DOCUMENT,
        "Bytes": base64::encode(OTHER_DOCUMENT),
        "LanguageCode": "en",
    });

    let first = comprehend.ok("DetectEntities", &plain).await;
    let beside_bytes = comprehend.ok("DetectEntities", &with_bytes).await;
    let again = comprehend.ok("DetectEntities", &plain).await;

    assert_eq!(
        first, beside_bytes,
        "case \"the same Text beside a Bytes document\": expected the same answer, got \
         {first} then {beside_bytes}"
    );
    assert_eq!(
        first, again,
        "case \"the same Text twice\": expected the same answer, got {first} then {again}"
    );
}
