// SPDX-License-Identifier: CC0-1.0

//! The whole JSON body a refused `ScanSbom` answers with, compared byte for byte.
//!
//! RULE: the refusal a client parses is the error code and the sentence, and nothing else —
//! Amazon Inspector Scan answers an unscannable `sbom` with HTTP 400 and a body whose only
//! member is `message`, under the protocol's `__type` discriminator. So the body is a function
//! of the CLASS of defect alone: two documents defective the same way are answered the same
//! bytes, and two defective in different ways differ only in the sentence.
//!
//! `scan_sbom_refusals.rs` pins the sentence for every class of defect and
//! `scan_sbom_refusal_reason.rs` pins the member set over that same table; this suite pins the
//! document as a WHOLE, which is what forbids a member being added beside the sentence.
//!
//! The rows differ along what the rule generalises over: the class of defect (a document no
//! BOM is read out of, a BOM type no scan reads, a member absent where the specification
//! requires it, a member carrying a value it cannot hold) and, within one class, the form the
//! defect takes — so each class is driven by two unlike documents.
//!
//! Shares the HTTP helpers of `scan_sbom.rs` through the `super` path the test crate root
//! gives it.

use super::scan_sbom::{bom, component, emulator, post_scan};
use serde_json::{json, Value};

/// The error code the model gives an input-SBOM refusal, as the protocol's discriminator
/// carries it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The fixed text every input-SBOM refusal's `message` opens with.
const PREFIX: &str = "Input SBOM failed validation: ";

/// The whole body a refusal stating `condition` answers with: the discriminator and the
/// sentence, and no other member.
fn refusal_body(condition: &str) -> Value {
    json!({ "__type": VALIDATION_EXCEPTION, "message": format!("{PREFIX}{condition}") })
}

/// One class of defect: two unlike documents that fail it, and the clause the sentence states
/// for the class.
struct Class {
    what: &'static str,
    condition: &'static str,
    documents: Vec<Value>,
}

fn classes() -> Vec<Class> {
    vec![
        Class {
            what: "nothing a BOM document is read out of",
            condition: "Invalid json",
            documents: vec![json!("not-an-sbom"), json!([1, 2, 3])],
        },
        Class {
            what: "a bomFormat naming a standard no scan reads",
            condition: "Invalid value for \"bomFormat\"",
            documents: vec![
                json!({ "bomFormat": "SPDX", "specVersion": "1.5", "components": [] }),
                json!({ "bomFormat": "cyclonedx", "specVersion": "1.5", "components": [] }),
            ],
        },
        Class {
            what: "a specVersion naming another CycloneDX revision",
            condition: "Invalid value for \"specVersion\"",
            documents: vec![
                json!({ "bomFormat": "CycloneDX", "specVersion": "1.4", "components": [] }),
                json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "components": [] }),
            ],
        },
        Class {
            what: "a components member carrying a value it cannot hold",
            condition: "Invalid value for \"components\"",
            documents: vec![
                json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": {} }),
                bom(vec![json!("left-pad@1.0.0")]),
            ],
        },
    ]
}

/// A component whose every member is readable, so the control document below is scanned.
fn clean_component() -> Value {
    component("ref-clean", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0")
}

#[tokio::test]
async fn a_refusal_body_is_the_error_code_and_the_sentence_and_nothing_else() {
    let (_engine, router) = emulator();
    for class in classes() {
        let expected = refusal_body(class.condition);
        for (index, document) in class.documents.iter().enumerate() {
            let case = format!("{} ({index})", class.what);
            let (status, response) = post_scan(&router, &json!({ "sbom": document })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {response}"
            );
            assert_eq!(
                response, expected,
                "case {case:?}: expected the body {expected}, got {response}"
            );
        }
    }

    // The control: a document that IS a CycloneDX 1.5 BOM is scanned, so none of the bodies
    // above is what a readable document gets.
    let (status, response) = post_scan(&router, &json!({ "sbom": bom(vec![clean_component()]) }))
        .await;
    assert_eq!(
        status, 200,
        "a readable CycloneDX 1.5 document is scanned, got {status}: {response}"
    );
}
