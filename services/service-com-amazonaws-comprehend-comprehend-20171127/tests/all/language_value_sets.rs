// SPDX-License-Identifier: CC0-1.0

//! How Comprehend admits the `LanguageCode` of a request whose operation accepts
//! fewer languages than the shared `LanguageCode` enum declares.
//!
//! THE RULE, asserted over a SET of operations and over every value the model's
//! `LanguageCode` enum names rather than over one recorded call: where an operation's
//! own accepted set is narrower than the enumerated type its member targets, a value
//! outside that set is refused at the REQUEST-VALIDATION boundary — HTTP 400,
//! `ValidationException`, in the validator's own enum-constraint clause naming the
//! member path, the value the request carried and that operation's accepted set — and
//! a value inside the set is not refused for its language at all. The modelled
//! `UnsupportedLanguageException` is a different condition: a language the service
//! cannot PROCESS, which the operations whose analyser is language-bound
//! (`DetectSyntax`, `DetectEntities`, `DetectToxicContent` and their `BatchDetect*`
//! siblings) keep answering with and `syntax`, `entity_detection` and
//! `toxic_content` assert. Sentiment is bound to no language — Comprehend infers it
//! in every value the enum names — so `sentiment_languages` asserts the other side
//! of that distinction.
//!
//! The clause is the validator's own down to its punctuation: no
//! `<N> validation error(s) detected: ` envelope, and no space between the quoted
//! value and `failed`.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::LANGUAGE_CODE_VALUES;

use crate::comprehend_client::{error_code, error_message, Comprehend};

/// The error code a request-validation refusal answers with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A document carrying PII in forms the recognizer places, so an accepted row is
/// answered by the analysis rather than by an empty result.
const DOCUMENT: &str = "Write to jane.roe@example.com or call 206-555-0147 today.";

/// One operation that applies an accepted-`LanguageCode` set of its own, with that
/// set and the status a request inside the set answers with.
struct Case {
    /// What the row is, for an assertion message.
    label: &'static str,
    operation: &'static str,
    /// The set the operation admits, as the refusal has to list it.
    accepted: &'static [&'static str],
    /// The status an accepted language answers with: 200 for the real-time
    /// inference operations, and 200 for the job submission, which is accepted and
    /// mints a job.
    accepted_status: u16,
}

/// Every operation the rule covers, with the set each one admits.
///
/// The sets come from the model's own member documentation where it names one —
/// `DetectPiiEntitiesRequest$LanguageCode` and
/// `StartPiiEntitiesDetectionJobRequest$LanguageCode` both read "Enter the language
/// code for English (en) or Spanish (es)", and
/// `DetectTargetedSentimentRequest$LanguageCode` reads "Currently, English is the only
/// supported language"
/// (`smithy_query(query="doc",shape="DetectPiiEntitiesRequest",member="LanguageCode")`)
/// — and from the recorded refusal for `ContainsPiiEntities`, whose member
/// documentation names no set and which admits English alone.
const CASES: &[Case] = &[
    Case {
        label: "DetectPiiEntities accepts English and Spanish",
        operation: "DetectPiiEntities",
        accepted: &["en", "es"],
        accepted_status: 200,
    },
    Case {
        label: "ContainsPiiEntities accepts English alone",
        operation: "ContainsPiiEntities",
        accepted: &["en"],
        accepted_status: 200,
    },
    Case {
        label: "DetectTargetedSentiment accepts English alone",
        operation: "DetectTargetedSentiment",
        accepted: &["en"],
        accepted_status: 200,
    },
    Case {
        label: "StartPiiEntitiesDetectionJob accepts English and Spanish",
        operation: "StartPiiEntitiesDetectionJob",
        accepted: &["en", "es"],
        accepted_status: 200,
    },
];

/// The request body for `operation` carrying `language_code`.
///
/// Every member but `LanguageCode` is one the operation accepts, so the language is
/// the only thing a refusal below can be about.
fn body(operation: &str, language_code: &str) -> Value {
    if operation == "StartPiiEntitiesDetectionJob" {
        return json!({
            "JobName": format!("pii-job-{language_code}"),
            "LanguageCode": language_code,
            "Mode": "ONLY_OFFSETS",
            "InputDataConfig": { "S3Uri": "s3://bucket/in/", "InputFormat": "ONE_DOC_PER_LINE" },
            "OutputDataConfig": { "S3Uri": "s3://bucket/out/" },
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        });
    }
    json!({ "Text": DOCUMENT, "LanguageCode": language_code })
}

/// The clause the validator refuses `submitted` with, for an operation admitting
/// `accepted`: the member path, the submitted value and the accepted set, with no
/// envelope and no space before `failed`.
fn expected_clause(submitted: &str, accepted: &[&str]) -> String {
    format!(
        "Value '{submitted}' at 'languageCode'failed to satisfy constraint: Member must \
         satisfy enum value set: [{}]",
        accepted.join(", ")
    )
}

#[tokio::test]
async fn a_language_outside_an_operations_own_set_is_refused_as_an_enum_constraint() {
    let comprehend = Comprehend::new();

    for case in CASES {
        for code in LANGUAGE_CODE_VALUES {
            let (status, response) = comprehend
                .call(case.operation, &body(case.operation, code))
                .await;
            let label = format!("{}: {} with {code:?}", case.label, case.operation);

            if case.accepted.contains(code) {
                assert_eq!(
                    status, case.accepted_status,
                    "case {label:?}: this language is inside the operation's own set, \
                     expected {}, got {status}: {response}",
                    case.accepted_status
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                VALIDATION_EXCEPTION,
                "case {label:?}: a value outside the operation's own set is a \
                 request-validation refusal, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_clause(code, case.accepted),
                "case {label:?}: the clause must name the member, the submitted value \
                 and the accepted set, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_refused_language_decides_before_the_document_is_read() {
    let comprehend = Comprehend::new();

    // The restriction is a precondition of the REQUEST, so it is answered whatever
    // else the document is: rows span an empty-of-PII document, one carrying PII and
    // one past the operation's published byte quota (which would otherwise be refused
    // with `TextSizeLimitExceededException`).
    let oversized = "a".repeat(100_001);
    for (case, text) in [
        ("prose with no PII", "The quarterly report is attached."),
        ("a document carrying PII", DOCUMENT),
        ("a document past the size quota", oversized.as_str()),
    ] {
        for operation in ["DetectPiiEntities", "ContainsPiiEntities"] {
            let accepted: &[&str] = if operation == "ContainsPiiEntities" {
                &["en"]
            } else {
                &["en", "es"]
            };
            let (status, response) = comprehend
                .call(
                    operation,
                    &json!({ "Text": text, "LanguageCode": "fr" }),
                )
                .await;
            let label = format!("{operation}: {case}");
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                VALIDATION_EXCEPTION,
                "case {label:?}: the language decides first, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_clause("fr", accepted),
                "case {label:?}: unexpected refusal clause, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_language_is_analysed_by_the_documents_own_forms() {
    let comprehend = Comprehend::new();

    // The other side of the rule: every language the operation DOES admit is
    // analysed, and the recognizer places entities by each identifier's published
    // form, so the same document answers the same entities whichever accepted
    // language it is submitted under.
    let mut baseline: Option<Value> = None;
    for code in ["en", "es"] {
        let output = comprehend
            .ok(
                "DetectPiiEntities",
                &json!({ "Text": DOCUMENT, "LanguageCode": code }),
            )
            .await;
        let entities = output["Entities"].as_array().cloned().unwrap_or_default();
        assert!(
            !entities.is_empty(),
            "case {code:?}: expected the document's entities, got {output}"
        );
        match &baseline {
            None => baseline = Some(output),
            Some(first) => assert_eq!(
                &output, first,
                "case {code:?}: an accepted language is analysed by the document's own \
                 forms, so expected {first}, got {output}"
            ),
        }
    }
}
