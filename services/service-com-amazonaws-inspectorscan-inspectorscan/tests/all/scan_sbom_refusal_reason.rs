// SPDX-License-Identifier: CC0-1.0

//! What a refused `ScanSbom` call REPORTS about the failure, end-to-end over
//! HTTP.
//!
//! RULE: a refusal states its class in the `message` and in nothing else — the
//! HTTP 400 `ValidationException` body Amazon Inspector Scan answers an
//! unscannable `sbom` with carries `message` alone, with no classification
//! member and no per-member field list beside it, so the sentence is the whole
//! of what a caller reads. The sentence is the fixed text
//! `Input SBOM failed validation: ` followed by the clause for the CLASS of
//! defect:
//!
//! * nothing the request carries reads as a BOM document at all ->
//!   `Invalid json`;
//! * the document STATES a BOM format or revision this operation does not scan,
//!   or a member is absent where the specification requires it or carries a
//!   value it cannot hold -> `Invalid value for "<member>"`, naming the member
//!   as CycloneDX spells it;
//! * the document carries more components than one scan accepts ->
//!   `Max number of components exceeded (<limit>)`.
//!
//! The rows differ along what the rule generalises over: the class of defect,
//! the JSON type the request sent, the member that failed (the document's own
//! and a component's), the nesting level it failed at, and — for the component
//! ceiling — the count.
//!
//! Shares the HTTP helpers of `scan_sbom.rs` through the `super` path the test
//! crate root gives it.

use super::scan_sbom::{bom, component, emulator, post_scan};
use serde_json::{json, Value};

/// The fixed text every input-SBOM refusal's `message` opens with.
const PREFIX: &str = "Input SBOM failed validation: ";

/// The clause a request carrying no readable BOM document is refused with.
const INVALID_JSON: &str = "Invalid json";

/// The clause a member stating something no scan reads — or holding a value it
/// cannot hold — is refused with.
fn invalid_value(member: &str) -> String {
    format!("Invalid value for \"{member}\"")
}

/// The component ceiling the handler enforces, as `scan_sbom_refusals.rs`
/// states it: one past it is refused for the `components` member.
const COMPONENT_LIMIT: usize = 5000;

/// One row: a `sbom` value and the clause the refusal's sentence has to end
/// with for it.
#[derive(Debug)]
struct Case {
    what: &'static str,
    sbom: Value,
    condition: String,
}

/// A component whose every member is readable, so a row's defect is the only
/// one in the document.
fn clean_component() -> Value {
    component("ref-clean", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0")
}

/// A JSON document nested `levels` arrays deep: more than this operation can
/// take in, so nothing of a BOM document is read out of it.
fn deeply_nested(levels: usize) -> Value {
    let mut value = json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" });
    for _ in 0..levels {
        value = Value::Array(vec![value]);
    }
    value
}

/// `count` distinct clean components.
fn many_components(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| {
            component(
                &format!("ref-{i}"),
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )
        })
        .collect()
}

fn cases() -> Vec<Case> {
    vec![
        // Nothing a BOM document can be read out of: whatever JSON type was
        // sent instead, and JSON too deep to be taken in.
        Case {
            what: "a string document",
            sbom: json!("this-is-not-a-cyclonedx-sbom-just-a-string"),
            condition: INVALID_JSON.to_string(),
        },
        Case {
            what: "an array document",
            sbom: json!([1, 2, 3]),
            condition: INVALID_JSON.to_string(),
        },
        Case {
            what: "a number document",
            sbom: json!(7),
            condition: INVALID_JSON.to_string(),
        },
        Case {
            what: "a boolean document",
            sbom: json!(true),
            condition: INVALID_JSON.to_string(),
        },
        Case {
            what: "a document nested deeper than it can be parsed",
            sbom: deeply_nested(96),
            condition: INVALID_JSON.to_string(),
        },
        // A document STATING a BOM type this operation does not scan.
        Case {
            what: "a bomFormat naming another SBOM standard",
            sbom: json!({ "bomFormat": "SPDX", "specVersion": "1.5", "components": [] }),
            condition: invalid_value("bomFormat"),
        },
        Case {
            what: "a bomFormat in the wrong case",
            sbom: json!({ "bomFormat": "cyclonedx", "specVersion": "1.5", "components": [] }),
            condition: invalid_value("bomFormat"),
        },
        Case {
            what: "an earlier CycloneDX revision",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.4", "components": [] }),
            condition: invalid_value("specVersion"),
        },
        Case {
            what: "a later CycloneDX revision",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "components": [] }),
            condition: invalid_value("specVersion"),
        },
        // A member of the document that is absent where it is required.
        Case {
            what: "an empty document, stating no bomFormat",
            sbom: json!({}),
            condition: invalid_value("bomFormat"),
        },
        Case {
            what: "a document stating no specVersion",
            sbom: json!({ "bomFormat": "CycloneDX", "components": [] }),
            condition: invalid_value("specVersion"),
        },
        // A member carrying a value it cannot hold: the document's own members
        // and a component's, at the top level and one level down.
        Case {
            what: "a bomFormat sent as a number",
            sbom: json!({ "bomFormat": 1, "specVersion": "1.5", "components": [] }),
            condition: invalid_value("bomFormat"),
        },
        Case {
            what: "a specVersion sent as a number",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": 1.5, "components": [] }),
            condition: invalid_value("specVersion"),
        },
        Case {
            what: "components sent as an object",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": {} }),
            condition: invalid_value("components"),
        },
        Case {
            what: "a component entry that is not an object",
            sbom: bom(vec![json!("left-pad@1.0.0")]),
            condition: invalid_value("components"),
        },
        Case {
            what: "a nested components member that is not an array",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": { "name": "left-pad" },
            })]),
            condition: invalid_value("components"),
        },
        Case {
            what: "a component stating no type",
            sbom: bom(vec![json!({ "name": "left-pad", "version": "1.0.0" })]),
            condition: invalid_value("type"),
        },
        Case {
            what: "a component whose type is not a CycloneDX component type",
            sbom: bom(vec![json!({
                "type": "package",
                "name": "left-pad",
                "version": "1.0.0",
            })]),
            condition: invalid_value("type"),
        },
        Case {
            what: "a component whose name is empty",
            sbom: bom(vec![json!({ "type": "library", "name": "", "version": "1.0.0" })]),
            condition: invalid_value("name"),
        },
        Case {
            what: "a purl that is not a package URL",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "left-pad",
                "version": "1.0.0",
                "purl": "left-pad@1.0.0",
            })]),
            condition: invalid_value("purl"),
        },
        Case {
            what: "two components sharing one bom-ref",
            sbom: bom(vec![
                component("same", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
                component("same", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
            ]),
            condition: invalid_value("bom-ref"),
        },
        Case {
            what: "a metadata member sent as a string",
            sbom: {
                let mut sbom = bom(vec![clean_component()]);
                sbom["metadata"] = json!("generated by acme");
                sbom
            },
            condition: invalid_value("metadata"),
        },
        Case {
            what: "a serialNumber sent as a number",
            sbom: {
                let mut sbom = bom(vec![clean_component()]);
                sbom["serialNumber"] = json!(42);
                sbom
            },
            condition: invalid_value("serialNumber"),
        },
        // More entries than the `components` member may hold: the ceiling is a
        // bound on that member, and the clause states the limit.
        Case {
            what: "one component more than a scan accepts",
            sbom: bom(many_components(COMPONENT_LIMIT + 1)),
            condition: format!("Max number of components exceeded ({COMPONENT_LIMIT})"),
        },
        Case {
            what: "an assembly whose tree is one component over the ceiling",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-assembly",
                "name": "assembly",
                "version": "1.0.0",
                "components": many_components(COMPONENT_LIMIT),
            })]),
            condition: format!("Max number of components exceeded ({COMPONENT_LIMIT})"),
        },
    ]
}

/// Every class of defect is refused with HTTP 400 `ValidationException` whose
/// body is the sentence for that class and nothing else: no classification
/// member, no field list, nothing a client could read as a second statement
/// about the failure.
#[tokio::test]
async fn every_refusal_states_its_class_in_the_message_alone() {
    let (_engine, router) = emulator();
    for case in cases() {
        let (status, response) = post_scan(&router, &json!({ "sbom": case.sbom })).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status}: {response}",
            case.what
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ValidationException"),
            "case {:?}: expected the ValidationException error code, got {response}",
            case.what
        );
        let expected = format!("{PREFIX}{}", case.condition);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected message {expected:?}, got {response}",
            case.what
        );
        // `__type` is the protocol's error discriminator; every other member of
        // the body is something the refusal states, and it states only the
        // sentence above.
        let members: Vec<String> = response
            .as_object()
            .map(|body| {
                body.keys()
                    .filter(|key| *key != "__type")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            members,
            vec!["message".to_string()],
            "case {:?}: the refusal carries the message alone, got {members:?} in \
             {response}",
            case.what
        );
    }
}

/// A document that IS a CycloneDX 1.5 BOM is still scanned, and its report
/// carries none of a refusal's members: the refusal above is reached for
/// defective input only.
#[tokio::test]
async fn a_scanned_document_carries_no_refusal_members() {
    let (_engine, router) = emulator();
    let request = json!({ "sbom": bom(vec![clean_component()]) });
    let (status, response) = post_scan(&router, &request).await;
    assert_eq!(
        status, 200,
        "a minimal valid CycloneDX 1.5 document is scanned, got {status}: {response}"
    );
    for member in ["__type", "message", "reason", "fields"] {
        assert!(
            response.get(member).is_none(),
            "a scanned document carries no refusal member, got {member:?} in {response}"
        );
    }
}
