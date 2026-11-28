// SPDX-License-Identifier: CC0-1.0

//! The document-level `version` of a submitted CycloneDX BOM, end-to-end over
//! HTTP.
//!
//! One rule, the same one every other member the report is built from goes
//! through: a member present with a JSON type the CycloneDX 1.5 schema does not
//! allow there is refused HTTP 400 `ValidationException` naming that member, a
//! member present as `null` states no value and is read exactly as an absent
//! one is (so the schema's own default is stamped), and a value of the type the
//! schema allows is preserved as submitted.
//!
//! `version` is the BOM's own revision number, which the CycloneDX 1.5 schema
//! types an integer defaulting to `1` — the type and default the rows below are
//! written against, and the same pair the handler names.
//!
//! The accepted rows differ along the integer's VALUE (the default, another
//! revision, one past 32 bits), and the refused rows along the JSON type sent
//! (string, boolean, object, array) and along the spellings a number can carry
//! that state no integer (a fractional part, a float literal) — and along the
//! `outputFormat` the scan was asked for: whether a document is a CycloneDX 1.5
//! BOM is a property of the document, not of the form the answer is rendered
//! in.
//!
//! Shares the HTTP helpers of `scan_sbom.rs` through the `super` path the test
//! crate root gives it.

use super::scan_sbom::{bom, component, emulator, post_scan};
use super::scan_sbom_refusals::{invalid_value, named_format, OUTPUT_FORMATS};
use serde_json::{json, Value};

/// A component the advisory data rules on, so a document carrying it is one the
/// scan reports a finding for rather than one it skips.
fn scannable_component() -> Value {
    component("ref-lodash", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20")
}

/// A valid document whose document-level `version` member is set to `value`.
fn document_with_version(value: Value) -> Value {
    let mut sbom = bom(vec![scannable_component()]);
    sbom["version"] = value;
    sbom
}

/// The same document carrying no `version` member at all.
fn document_without_version() -> Value {
    let mut sbom = bom(vec![scannable_component()]);
    sbom.as_object_mut()
        .expect("the bom helper builds a JSON object")
        .remove("version");
    sbom
}

/// A `version` of a JSON type the schema does not allow there is refused — the
/// same refusal the `serialNumber` beside it already answers — whatever form
/// the report was asked for.
#[tokio::test]
async fn a_document_version_of_another_json_type_is_refused_naming_the_member() {
    // One row per JSON spelling a revision number cannot be stated in. A number
    // with a fractional part states no integer, and so does one WRITTEN as a
    // floating-point literal: the report preserves what the caller sent, so a
    // value it could only carry by rewriting is refused here.
    let mistyped: Vec<(&str, Value)> = vec![
        ("a string", json!("one")),
        ("an empty string", json!("")),
        ("a boolean", json!(true)),
        ("an empty object", json!({})),
        ("an object", json!({ "revision": 1 })),
        ("an empty array", json!([])),
        ("an array", json!([1])),
        ("a fractional number", json!(1.5)),
        ("a negative fractional number", json!(-0.25)),
        ("an integer written as a float", json!(7.0)),
        ("a number too large to state an integer", json!(1e300)),
    ];

    let (_engine, router) = emulator();
    // The refusal is worded by the same helper every other member's refusal is
    // asserted through, so one rule is stated once.
    let expected = invalid_value("version");
    for (shape, value) in mistyped {
        for format in OUTPUT_FORMATS {
            let mut request = json!({ "sbom": document_with_version(value.clone()) });
            if let Some(format) = format {
                request["outputFormat"] = json!(format);
            }
            let (status, response) = post_scan(&router, &request).await;
            let case = format!(
                "a document version sent as {shape}, {}",
                named_format(*format)
            );
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {response}"
            );
            assert_eq!(
                response["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected the ValidationException error code, got {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {response}"
            );
            assert!(
                response.get("sbom").is_none(),
                "case {case:?}: a refused call carries no report, got {response}"
            );
        }
    }
}

/// The ACCEPTED side of that rule: an integer revision number is scanned and
/// PRESERVED as submitted, so the refusals above are the type check and not a
/// refusal of the member itself.
#[tokio::test]
async fn an_integer_document_version_is_scanned_and_preserved() {
    // Rows differ along the value: the schema's default, a later revision, and
    // one past 32 bits.
    let accepted: Vec<(&str, Value)> = vec![
        ("the schema default", json!(1)),
        ("a later revision", json!(7)),
        ("a revision past 32 bits", json!(4_294_967_296_i64)),
    ];

    let (_engine, router) = emulator();
    for (what, value) in accepted {
        let request = json!({
            "sbom": document_with_version(value.clone()),
            "outputFormat": "CYCLONE_DX_1_5",
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {what:?}: expected HTTP 200, got {status}: {response}"
        );
        assert_eq!(
            response["sbom"]["version"], value,
            "case {what:?}: expected the submitted version {value}, got {}",
            response["sbom"]["version"]
        );
        // The document was scanned, not merely echoed: the component it carries
        // is one the advisory data rules on.
        assert!(
            response["sbom"]["vulnerabilities"]
                .as_array()
                .is_some_and(|findings| !findings.is_empty()),
            "case {what:?}: expected the scan's findings in the report, got {response}"
        );
    }
}

/// A `version` present as `null` states no value, so it is read as absent and
/// the report carries the schema's default — exactly as a document naming no
/// `version` at all is answered.
#[tokio::test]
async fn a_null_document_version_is_read_as_absent_and_defaulted() {
    // Both rows state no revision number, so both must answer the same one.
    let stated_none: Vec<(&str, Value)> = vec![
        ("no version member", document_without_version()),
        ("a null version", document_with_version(Value::Null)),
    ];

    let (_engine, router) = emulator();
    for (what, sbom) in stated_none {
        let request = json!({ "sbom": sbom, "outputFormat": "CYCLONE_DX_1_5" });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {what:?}: expected HTTP 200, got {status}: {response}"
        );
        assert_eq!(
            response["sbom"]["version"],
            json!(1),
            "case {what:?}: expected the CycloneDX default revision 1, got {}",
            response["sbom"]["version"]
        );
    }
}

/// And the sibling member the handler states the identical rule for answers the
/// same way for the same input: a `null` `serialNumber` is read as absent, so
/// the scan's own URN is stamped, while a `null` `version` is defaulted.
#[tokio::test]
async fn a_null_document_member_is_read_as_absent_at_both_sites() {
    let mut sbom = document_with_version(Value::Null);
    sbom["serialNumber"] = Value::Null;

    let (_engine, router) = emulator();
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": sbom, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    assert!(
        response["sbom"]["serialNumber"]
            .as_str()
            .is_some_and(|serial| serial.starts_with("urn:uuid:")),
        "case \"a null serialNumber\": expected a stamped urn:uuid serial number, got {}",
        response["sbom"]["serialNumber"]
    );
    assert_eq!(
        response["sbom"]["version"],
        json!(1),
        "case \"a null version\": expected the CycloneDX default revision 1, got {}",
        response["sbom"]["version"]
    );
}
