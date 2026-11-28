// SPDX-License-Identifier: CC0-1.0

//! Comprehend's `DetectEntities`: named-entity recognition over a plain-text
//! document.
//!
//! The operation is a pure function of the submitted document, so every suite here
//! asserts the RULE over a table of documents rather than one recorded request: each
//! entity the recogniser reports carries a modelled `EntityType`, a confidence in
//! `(0, 1]`, and zero-based CHARACTER offsets whose slice of the document is exactly
//! the entity's own `Text`; the entities come back in document order and never
//! overlap; and the evidence that types a span is the document's own — a closed-form
//! lexical pattern, a lexicon entry, or a cue beside the span.

use aws_smithy_types::base64;
use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ENTITY_TYPE_VALUES, LANGUAGE_CODE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// One document per rule class the recogniser applies, with the entity that rule
/// must find in it.
///
/// The rows differ along what the rule generalises over: which kind of evidence the
/// document carries (pattern, lexicon entry, cue token), how many tokens the name
/// spans, where in the sentence it sits, and which of the nine modelled
/// `EntityType` values it ends up with. Every modelled type is covered.
const DOCUMENTS: &[(&str, &str, &str, &str)] = &[
    (
        "person behind an honorific",
        "Dr. Jane Doe reviewed the report.",
        "PERSON",
        "Jane Doe",
    ),
    (
        "person behind an office",
        "Senator Grace Hopper opened the hearing.",
        "PERSON",
        "Grace Hopper",
    ),
    (
        "person from a given name",
        "Michael Brown joined the call.",
        "PERSON",
        "Michael Brown",
    ),
    (
        "city in the place lexicon",
        "The replacement part shipped to Seattle.",
        "LOCATION",
        "Seattle",
    ),
    (
        "country in the place lexicon",
        "They expanded into Portugal.",
        "LOCATION",
        "Portugal",
    ),
    (
        "place from a geographic designator",
        "They met on Abbey Road.",
        "LOCATION",
        "Abbey Road",
    ),
    (
        "organization from a legal-form designator",
        "Acme Widgets LLC filed the papers.",
        "ORGANIZATION",
        "Acme Widgets LLC",
    ),
    (
        "organization in the lexicon, extended by the name it heads",
        "Amazon Web Services announced a new region.",
        "ORGANIZATION",
        "Amazon Web Services",
    ),
    (
        "organization written with an ampersand",
        "The audit was signed by Johnson & Johnson.",
        "ORGANIZATION",
        "Johnson & Johnson",
    ),
    (
        "event from an event designator",
        "She trained all winter for the Boston Marathon.",
        "EVENT",
        "Boston Marathon",
    ),
    (
        "date in ISO-8601 form",
        "The outage began on 2024-01-15 and lasted an hour.",
        "DATE",
        "2024-01-15",
    ),
    (
        "date written with its month name",
        "The clerk filed it on January 5, 2024 in the morning.",
        "DATE",
        "January 5, 2024",
    ),
    (
        "date written with an abbreviated month name",
        "The hearing is set for Sept 5, 2024 at the courthouse.",
        "DATE",
        "Sept 5, 2024",
    ),
    (
        "quantity as a percentage",
        "Revenue grew 12 percent over the quarter.",
        "QUANTITY",
        "12 percent",
    ),
    (
        "quantity as a currency amount",
        "The deal was worth $4.5 million.",
        "QUANTITY",
        "$4.5 million",
    ),
    (
        "quantity as a measurement",
        "The parcel weighs 12 kg.",
        "QUANTITY",
        "12 kg",
    ),
    (
        "title in quotation marks",
        "He read \"The Old Man and the Sea\" twice.",
        "TITLE",
        "The Old Man and the Sea",
    ),
    (
        "branded product marked with the trademark sign",
        "She bought a ThinkPad\u{2122} for the lab.",
        "COMMERCIAL_ITEM",
        "ThinkPad",
    ),
    (
        "email address, which fits no other category",
        "Write to ops@example.com for help.",
        "OTHER",
        "ops@example.com",
    ),
    (
        "url, which fits no other category",
        "See https://example.com/docs for details.",
        "OTHER",
        "https://example.com/docs",
    ),
    (
        "named phrase no rule places",
        "The Zorblax Initiative published its findings.",
        "OTHER",
        "Zorblax Initiative",
    ),
];

/// The `Entities` array of a successful response.
fn entities(output: &Value, case: &str) -> Vec<Value> {
    output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
        .clone()
}

/// Assert the invariants every entity carries, whatever rule typed it: a modelled
/// type, a confidence in `(0, 1]`, and character offsets delimiting exactly its
/// `Text`.
fn assert_entity_invariants(case: &str, text: &str, found: &[Value]) {
    let mut previous_end = 0i64;
    for entity in found {
        let entity_type = entity["Type"].as_str().unwrap_or_default();
        assert!(
            ENTITY_TYPE_VALUES.contains(&entity_type),
            "case {case:?}: {entity_type:?} is not a modelled EntityType value, in {entity}"
        );

        let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a Score in (0, 1], got {score} in {entity}"
        );

        let begin = entity["BeginOffset"].as_i64().unwrap_or(-1);
        let end = entity["EndOffset"].as_i64().unwrap_or(-1);
        let entity_text = entity["Text"].as_str().unwrap_or_default();
        assert!(
            begin >= 0 && end > begin,
            "case {case:?}: expected 0 <= BeginOffset < EndOffset, got {begin}..{end} in {entity}"
        );
        let slice: String = text
            .chars()
            .skip(usize::try_from(begin).unwrap_or_default())
            .take(usize::try_from(end - begin).unwrap_or_default())
            .collect();
        assert_eq!(
            slice, entity_text,
            "case {case:?}: the characters {begin}..{end} of the document are {slice:?}, which \
             is not the entity's Text {entity_text:?}"
        );

        assert!(
            begin >= previous_end,
            "case {case:?}: entities must come back in document order without overlapping, got \
             {begin}..{end} after an entity ending at {previous_end} in {found:?}"
        );
        previous_end = end;
    }
}

#[tokio::test]
async fn each_rule_types_the_span_its_evidence_names() {
    let comprehend = Comprehend::new();

    for (case, text, expected_type, expected_text) in DOCUMENTS {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(&output, case);
        assert_entity_invariants(case, text, &found);

        let matching: Vec<&Value> = found
            .iter()
            .filter(|entity| entity["Text"].as_str() == Some(*expected_text))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one entity reading {expected_text:?}, got {found:?}"
        );
        let entity_type = matching[0]["Type"].as_str().unwrap_or_default();
        assert_eq!(
            entity_type, *expected_type,
            "case {case:?}: expected {expected_text:?} to be typed {expected_type}, got \
             {entity_type} in {:?}",
            matching[0]
        );
    }
}

/// The offsets are CHARACTER offsets, not byte offsets: a document carrying
/// multi-byte characters in front of an entity still delimits it exactly.
#[tokio::test]
async fn offsets_are_character_offsets_into_the_document() {
    let comprehend = Comprehend::new();

    for (case, text) in [
        ("ascii only", "Jane Doe met Michael Brown in Seattle."),
        (
            "multi-byte characters before the entities",
            "The caf\u{e9} \u{2014} \u{201c}Les Halles\u{201d} \u{2014} hosted Jane Doe and \
             Michael Brown in Paris.",
        ),
        (
            "emoji before the entities",
            "\u{1f680} Amazon Web Services shipped 12 kg to Seattle on 2024-01-15.",
        ),
    ] {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(&output, case);
        assert!(
            !found.is_empty(),
            "case {case:?}: expected at least one entity in {text:?}, got {output}"
        );
        assert_entity_invariants(case, text, &found);
    }
}

/// Words that join two names into a longer one are where one name ends and the next
/// begins — unless the whole phrase is a name in its own right, or the word in front
/// of the connector is the institution or geographic head noun that takes its
/// complement. Each row names the entities the document carries, in order.
#[tokio::test]
async fn a_phrase_connector_divides_two_names_but_not_one_name_that_carries_it() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in [
        (
            "two people joined by \"and\"",
            "Bill Gates and Paul Allen signed it.",
            vec![("PERSON", "Bill Gates"), ("PERSON", "Paul Allen")],
        ),
        (
            "a person and the organization they belong to",
            "Dr. Emily Carter of Stanford University spoke.",
            vec![
                ("PERSON", "Emily Carter"),
                ("ORGANIZATION", "Stanford University"),
            ],
        ),
        (
            "an institution head noun takes its complement",
            "The grant came from the University of Washington.",
            vec![("ORGANIZATION", "University of Washington")],
        ),
        (
            "a geographic head noun takes its complement",
            "They sailed across the Gulf of Mexico.",
            vec![("LOCATION", "Gulf of Mexico")],
        ),
        (
            "a name the lexicon holds as a whole keeps its connector",
            "She banks with Bank of America.",
            vec![("ORGANIZATION", "Bank of America")],
        ),
        (
            "a name the lexicon holds as a whole keeps its article",
            "The New York Times ran the story.",
            vec![("ORGANIZATION", "The New York Times")],
        ),
        (
            "a name particle never divides a name",
            "Vincent van Gogh painted in Paris.",
            vec![("PERSON", "Vincent van Gogh"), ("LOCATION", "Paris")],
        ),
    ] {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(&output, case);
        assert_entity_invariants(case, text, &found);

        let reported: Vec<(&str, &str)> = found
            .iter()
            .map(|entity| {
                (
                    entity["Type"].as_str().unwrap_or_default(),
                    entity["Text"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from {output}"
        );
    }
}

/// A document that names nothing gets an EMPTY entity list — the recogniser reports
/// what the document carries, never an entity it had no evidence for.
#[tokio::test]
async fn a_document_that_names_nothing_yields_no_entities() {
    let comprehend = Comprehend::new();

    for (case, text) in [
        ("no capitalized word at all", "there is nothing to report."),
        (
            "only a sentence-initial word and function words",
            "The meeting was moved to the afternoon.",
        ),
        ("punctuation only", "!!! ... ???"),
    ] {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(&output, case);
        assert!(
            found.is_empty(),
            "case {case:?}: expected no entity in {text:?}, got {found:?}"
        );
    }
}

#[tokio::test]
async fn detect_entities_is_deterministic() {
    let comprehend = Comprehend::new();
    let body = json!({
        "Text": "Dr. Jane Doe of Amazon Web Services spoke in Seattle on 2024-01-15.",
        "LanguageCode": "en",
    });

    let first = comprehend.ok("DetectEntities", &body).await;
    let second = comprehend.ok("DetectEntities", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical entities, got {first} then {second}"
    );
}

/// The analysed text comes from the `Text` member: `Bytes` carries an input document
/// for a custom entity recognition model reached through an `EndpointArn`, so a
/// request naming no endpoint and no `Text` carries no text at all, whatever its
/// `Bytes` holds. Asserted over the whole table of member combinations — including
/// each kind of `Bytes` document — by `detect_entities_text_source.rs`.
#[tokio::test]
async fn a_request_with_no_text_carries_no_document_whatever_its_bytes_hold() {
    let comprehend = Comprehend::new();
    let encoded = base64::encode("Jane Doe of Acme Widgets LLC visited Seattle on 2024-01-15.");

    let (status, body) = comprehend
        .call(
            "DetectEntities",
            &json!({ "Bytes": encoded, "LanguageCode": "en" }),
        )
        .await;

    assert_eq!(
        status, 400,
        "case \"a plain-text document in Bytes alone\": expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a plain-text document in Bytes alone\": expected ValidationException, got \
         {body}"
    );
    assert!(
        body["Entities"].is_null(),
        "case \"a plain-text document in Bytes alone\": a refused request reports no \
         entities, got {body}"
    );
}

#[tokio::test]
async fn a_language_the_recogniser_has_no_lexicon_for_is_refused() {
    let comprehend = Comprehend::new();

    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "DetectEntities",
                &json!({ "Text": "Jane Doe visited Seattle.", "LanguageCode": code }),
            )
            .await;

        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: the recogniser reads English, expected 200, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguageException",
                "case {code:?}: expected UnsupportedLanguageException, got {body}"
            );
        }
    }
}

/// `LanguageCode` is what names the model to analyse with when no custom endpoint
/// does, so a request carrying `Text` and no language is refused.
///
/// Which member the analysed text comes FROM — `Text` on the pre-trained path, with
/// `Bytes` reserved for a custom model behind an `EndpointArn` — is the rule
/// `detect_entities_text_source.rs` asserts over the whole table of member
/// combinations.
#[tokio::test]
async fn a_request_that_names_no_language_is_refused() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        (
            "no LanguageCode",
            json!({ "Text": "Jane Doe visited Seattle." }),
        ),
        (
            "no LanguageCode, with a longer document",
            json!({ "Text": "Jane Doe of Acme Widgets LLC visited Seattle on 2024-01-15." }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectEntities", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {response}"
        );
    }
}

/// An `EndpointArn` names a custom entity recognizer endpoint, and no operation in
/// this service persists one yet, so every endpoint a caller can name is a resource
/// that does not exist: the modelled client refusal, not a 501.
#[tokio::test]
async fn an_endpoint_that_does_not_exist_is_reported_as_unavailable() {
    let comprehend = Comprehend::new();

    for (case, arn) in [
        (
            "an endpoint in the request's own region",
            "arn:aws:comprehend:eu-west-1:000000000000:entity-recognizer-endpoint/analytics",
        ),
        (
            "an endpoint in another region",
            "arn:aws:comprehend:us-east-1:123456789012:entity-recognizer-endpoint/other",
        ),
    ] {
        let (status, body) = comprehend
            .call(
                "DetectEntities",
                &json!({ "Text": "Jane Doe visited Seattle.", "EndpointArn": arn }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceUnavailableException",
            "case {case:?}: expected ResourceUnavailableException, got {body}"
        );
    }
}

/// The per-document byte quota this operation enforces, from its OWN modelled member
/// documentation: `DetectEntitiesRequest$Text` reads "A UTF-8 text string. The
/// maximum string size is 100 KB." Read, as this crate reads every quota, as a count
/// of BYTES rather than a power-of-two multiple of 1024.
const TEXT_QUOTA_BYTES: usize = 100_000;

/// The size-report sentence the service answers an oversized document with — the one
/// `crate::state::text_size_refusal` builds for every real-time inference operation,
/// reporting the quota that applies and the submitted document's byte length.
fn expected_size_message(quota_bytes: usize, actual_bytes: usize) -> String {
    format!(
        "Input text size exceeds limit. Max length of request text allowed is \
         {quota_bytes} bytes while in this request the text size is {actual_bytes} bytes"
    )
}

/// A document is accepted up to the quota and refused one byte past it, and the quota
/// counts UTF-8 BYTES rather than characters — so a document of multi-byte characters
/// is refused on its byte length while its character count is still far below it.
#[tokio::test]
async fn a_document_is_accepted_up_to_the_quota_and_refused_past_it() {
    let comprehend = Comprehend::new();

    // `(case, document)`: the modelled `@length` floor, well inside the quota, the
    // last accepted length, the first refused one, the length a quota misread as
    // kibibytes would still accept, far past it, and the same boundary walked with
    // two-, three- and four-byte characters.
    let mut cases: Vec<(String, String)> = Vec::new();
    for size in [
        1usize,
        TEXT_QUOTA_BYTES / 2,
        TEXT_QUOTA_BYTES - 1,
        TEXT_QUOTA_BYTES,
        TEXT_QUOTA_BYTES + 1,
        100 * 1024,
        TEXT_QUOTA_BYTES * 2,
    ] {
        cases.push((format!("{size} ascii bytes"), "a".repeat(size)));
    }
    for (character, width) in [("é", 2usize), ("ก", 3), ("😀", 4)] {
        let fitting = TEXT_QUOTA_BYTES / width;
        cases.push((
            format!("{fitting} {width}-byte characters"),
            character.repeat(fitting),
        ));
        cases.push((
            format!("{} {width}-byte characters", fitting + 1),
            character.repeat(fitting + 1),
        ));
    }

    for (case, text) in cases {
        let actual_bytes = text.len();
        let (status, response) = comprehend
            .call(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;

        if actual_bytes > TEXT_QUOTA_BYTES {
            assert_eq!(
                status, 400,
                "case {case:?} ({actual_bytes} bytes): expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TextSizeLimitExceededException",
                "case {case:?}: expected TextSizeLimitExceededException, got {response}"
            );
            assert_eq!(
                response
                    .get("Message")
                    .or_else(|| response.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("<no Message>"),
                expected_size_message(TEXT_QUOTA_BYTES, actual_bytes),
                "case {case:?}: the refusal must report the quota and the submitted byte \
                 length, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?} ({actual_bytes} bytes): expected 200, got {status}: {response}"
            );
        }
    }
}

/// The modelled constraints the boundary validator enforces before the handler
/// runs: the `@length` floor of `Text` and `Bytes`, and the ARN pattern of
/// `EndpointArn`. Pinned here so a later change that moves one of these checks into
/// the handler cannot lose it.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_before_the_handler_runs() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("empty Bytes", json!({ "Bytes": "", "LanguageCode": "en" })),
        (
            "an EndpointArn that is not an ARN",
            json!({ "Text": "Jane Doe visited Seattle.", "EndpointArn": "not-an-arn" }),
        ),
        (
            "an EndpointArn for another service",
            json!({
                "Text": "Jane Doe visited Seattle.",
                "EndpointArn": "arn:aws:s3:::bucket/endpoint",
            }),
        ),
        (
            "an EndpointArn past the modelled 256-character ceiling",
            json!({
                "Text": "Jane Doe visited Seattle.",
                "EndpointArn": format!(
                    "arn:aws:comprehend:eu-west-1:000000000000:entity-recognizer-endpoint/{}",
                    "a".repeat(256),
                ),
            }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectEntities", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a value outside its modelled constraint, got \
             {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the boundary validator's ValidationException, got {response}"
        );
    }
}
