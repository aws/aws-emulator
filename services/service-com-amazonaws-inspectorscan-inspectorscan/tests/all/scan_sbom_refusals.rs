// SPDX-License-Identifier: CC0-1.0

//! The WIRE CONTRACT of a `ScanSbom` input refusal, end-to-end over HTTP.
//!
//! One rule, asserted over every class of input defect this operation refuses:
//! a bad input SBOM is answered HTTP 400 `ValidationException` whose `message`
//! is the fixed text `Input SBOM failed validation: ` followed by a clause
//! naming the CLASS of defect (never an index, a member path, or a value read
//! back out of the submitted document).
//!
//! The refusal body carries that sentence ALONE: `message` is the whole of it,
//! as `scan_sbom_refusal_reason.rs` pins over the same classes of defect.
//!
//! The table below is that rule's cases, grouped by class. Each class is
//! exercised in several forms — an absent member and a wrongly-valued one, a
//! top-level member and the same member nested inside an assembly, the bound of
//! a limit and one past it — so a row is one instance of the rule and not the
//! rule itself. Two documents defective the same way must be refused with the
//! same sentence, which is what the repeated `expect` values pin.
//!
//! Shares the HTTP helpers of `scan_sbom.rs` through the `super` path the test
//! crate root gives it.

use super::scan_sbom::{bom, component, emulator, post_scan};
use serde_json::{json, Value};

/// The fixed text every input-SBOM refusal opens with.
const PREFIX: &str = "Input SBOM failed validation: ";

/// The component ceiling the handler enforces (`MAX_COMPONENTS` in
/// `src/ops/scan_sbom.rs`). The limit clause states this same number, which the
/// bound cases below check by accepting exactly this many components and
/// refusing one more. It is the limit the service ENFORCES rather than the stale
/// 2000 the `sbom` member's modeled documentation still states.
const COMPONENT_LIMIT: usize = 5000;

/// One row: a request's `sbom` value and the whole refusal message it must be
/// answered with.
#[derive(Debug)]
struct Case {
    what: &'static str,
    sbom: Value,
    expect: String,
}

/// `PREFIX` plus the clause for a document that does not parse.
fn cannot_parse() -> String {
    format!("{PREFIX}Invalid json")
}

/// `PREFIX` plus the clause for a CycloneDX member that is absent where the
/// specification requires it, or carries a value it does not allow there.
///
/// `member` is the member's name as the CycloneDX 1.5 specification spells it —
/// the same member names `src/ops/scan_sbom.rs` validates: `bomFormat`,
/// `specVersion`, `components`, `type`, `name`, `purl`, `bom-ref`.
fn invalid_value(member: &str) -> String {
    format!("{PREFIX}Invalid value for \"{member}\"")
}

/// `PREFIX` plus the clause for a document carrying more components than one
/// scan accepts; the limit is the clause's only variable part.
fn too_many_components(limit: usize) -> String {
    format!("{PREFIX}Max number of components exceeded ({limit})")
}

/// A JSON document nested `levels` arrays deep: more than this operation can
/// take in, so it does not parse as a document at all.
fn deeply_nested(levels: usize) -> Value {
    let mut value = json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" });
    for _ in 0..levels {
        value = Value::Array(vec![value]);
    }
    value
}

/// An assembly whose `components[]` nests `levels` deep: deeper than a scan
/// resolves, so the `components` member is one it cannot read.
fn deep_assembly(levels: usize) -> Value {
    let mut entry = component("ref-leaf", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20");
    for level in 0..levels {
        entry = json!({
            "type": "application",
            "bom-ref": format!("ref-assembly-{level}"),
            "name": "app",
            "version": "1.0.0",
            "components": [entry],
        });
    }
    entry
}

/// `count` distinct components, each a clean coordinate.
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
    let mut cases = vec![
        // A `sbom` that is not a CycloneDX document at all: the class is "it
        // does not parse", whatever JSON type was sent instead.
        Case {
            what: "a string document",
            sbom: json!("this-is-not-a-cyclonedx-sbom-just-a-string"),
            expect: cannot_parse(),
        },
        Case {
            what: "an array document",
            sbom: json!([1, 2, 3]),
            expect: cannot_parse(),
        },
        Case {
            what: "a number document",
            sbom: json!(7),
            expect: cannot_parse(),
        },
        Case {
            what: "a boolean document",
            sbom: json!(true),
            expect: cannot_parse(),
        },
        Case {
            what: "a document nested deeper than it can be parsed",
            sbom: deeply_nested(96),
            expect: cannot_parse(),
        },
        // `bomFormat`: absent and wrongly-valued are one class.
        Case {
            what: "an empty document, carrying no bomFormat",
            sbom: json!({}),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a document carrying specVersion but no bomFormat",
            sbom: json!({ "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a non-CycloneDX bomFormat",
            sbom: json!({ "bomFormat": "SPDX", "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            // CycloneDX fixes `bomFormat` to the literal "CycloneDX".
            what: "a bomFormat in the wrong case",
            sbom: json!({ "bomFormat": "cyclonedx", "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a bomFormat that is not a string",
            sbom: json!({ "bomFormat": 1, "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        // `specVersion`: the revision either side of the one a scan reads, and
        // the member absent or of the wrong type.
        Case {
            what: "a missing specVersion",
            sbom: json!({ "bomFormat": "CycloneDX", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "an earlier CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.4", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "a later CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "a specVersion that is not a string",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": 1.5, "components": [] }),
            expect: invalid_value("specVersion"),
        },
        // `components`: the member itself, its entries, and the same member one
        // level down inside an assembly.
        Case {
            what: "components that are not an array",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": {} }),
            expect: invalid_value("components"),
        },
        Case {
            what: "components sent as a string",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": "none" }),
            expect: invalid_value("components"),
        },
        Case {
            what: "a component entry that is not an object",
            sbom: bom(vec![json!("lodash@4.17.20")]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a nested components member that is not an array",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": { "name": "lodash" },
            })]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a nested component entry that is not an object",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": ["lodash@4.17.20"],
            })]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a component tree nested deeper than a scan resolves",
            sbom: bom(vec![deep_assembly(20)]),
            expect: invalid_value("components"),
        },
        // A component's own required members.
        Case {
            what: "a component carrying no type",
            sbom: bom(vec![json!({ "name": "lodash", "version": "4.17.20" })]),
            expect: invalid_value("type"),
        },
        Case {
            what: "a component whose type is not a CycloneDX component type",
            sbom: bom(vec![json!({
                "type": "package",
                "name": "lodash",
                "version": "4.17.20",
            })]),
            expect: invalid_value("type"),
        },
        Case {
            what: "a component carrying no name",
            sbom: bom(vec![json!({
                "type": "library",
                "version": "4.17.20",
                "purl": "pkg:npm/lodash@4.17.20",
            })]),
            expect: invalid_value("name"),
        },
        Case {
            what: "a component whose name is empty",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "",
                "version": "4.17.20",
            })]),
            expect: invalid_value("name"),
        },
        // `purl`: every form that is not a package URL is one class.
        Case {
            what: "a purl carrying no pkg: scheme",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl whose scheme is not the lowercase \"pkg\"",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "PKG:npm/lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl naming a type but no package",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:npm",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl whose type is outside the purl grammar",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:np m/lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        // `bom-ref`: CycloneDX requires it to be unique within a document, and
        // a scan bounds its length.
        Case {
            what: "two components sharing one bom-ref",
            sbom: bom(vec![
                component("same", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
                component("same", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
            ]),
            expect: invalid_value("bom-ref"),
        },
        Case {
            what: "a bom-ref longer than a scan resolves",
            sbom: bom(vec![json!({
                "type": "library",
                "bom-ref": "r".repeat(513),
                "name": "lodash",
                "version": "4.17.20",
            })]),
            expect: invalid_value("bom-ref"),
        },
    ];

    // One past the component ceiling, flat and through an assembly: every entry
    // of the tree counts, so the same total is refused either way.
    cases.push(Case {
        what: "one component more than a scan accepts",
        sbom: bom(many_components(COMPONENT_LIMIT + 1)),
        expect: too_many_components(COMPONENT_LIMIT),
    });
    let assembly = json!({
        "type": "application",
        "bom-ref": "ref-assembly",
        "name": "assembly",
        "version": "1.0.0",
        "components": many_components(COMPONENT_LIMIT),
    });
    cases.push(Case {
        what: "an assembly whose tree is one component over the ceiling",
        sbom: bom(vec![assembly]),
        expect: too_many_components(COMPONENT_LIMIT),
    });
    cases
}

#[tokio::test]
async fn every_class_of_input_defect_is_refused_with_the_services_own_sentence() {
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
        assert_eq!(
            response["message"].as_str(),
            Some(case.expect.as_str()),
            "case {:?}: expected message {:?}, got {response}",
            case.what,
            case.expect
        );
        // The body carries the one member the recorded refusals of this
        // operation carry: `message`. `__type` is the protocol's error
        // discriminator, not a member of the shape.
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
            "case {:?}: the refusal body carries the message alone, got {members:?} in \
             {response}",
            case.what
        );
    }
}

/// The limit the clause states is the limit that is enforced: exactly
/// [`COMPONENT_LIMIT`] components are scanned, and one more is refused naming
/// that same number.
#[tokio::test]
async fn the_component_limit_clause_states_the_limit_that_is_enforced() {
    let (_engine, router) = emulator();
    for (count, expected_status) in [
        (1_usize, 200_u16),
        (COMPONENT_LIMIT - 1, 200),
        (COMPONENT_LIMIT, 200),
        (COMPONENT_LIMIT + 1, 400),
        (COMPONENT_LIMIT + 500, 400),
    ] {
        let (status, response) =
            post_scan(&router, &json!({ "sbom": bom(many_components(count)) })).await;
        assert_eq!(
            status, expected_status,
            "case {count} components: expected HTTP {expected_status}, got {status}: {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                response["message"].as_str(),
                Some(too_many_components(COMPONENT_LIMIT).as_str()),
                "case {count} components: expected the limit clause to name {COMPONENT_LIMIT}, \
                 got {response}"
            );
        }
    }
}

/// A valid document is still scanned: the refusal path above is reached for
/// defective input only, not for every request.
#[tokio::test]
async fn a_valid_document_is_not_refused() {
    let (_engine, router) = emulator();
    let (status, response) = post_scan(
        &router,
        &json!({
            "sbom": bom(vec![component(
                "ref-clean",
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a minimal valid CycloneDX 1.5 document is scanned, got {status}: {response}"
    );
    assert!(
        response["message"].is_null(),
        "a scanned document carries no refusal message, got {response}"
    );
}

// ---------------------------------------------------------------------------
// A submitted member whose JSON type CycloneDX does not allow
// ---------------------------------------------------------------------------
//
// The same class as "components that are not an array" above, at every other
// member the report is built from: `metadata`, `metadata.tools` (and its
// `services`), `metadata.properties`, a component's `properties` and
// `serialNumber`. A value of a type the specification does not allow there is
// REFUSED, naming that member, rather than scanned — a report that replaced it
// with the scan's own data would state the caller sent something it did not.
//
// The rows differ along the member, the JSON type sent for it, the nesting
// level (a component's own `properties` at the top level and inside an
// assembly), whether the advisory data rules on the component carrying it, and
// the `outputFormat` the scan was asked for: the document's validity is a
// property of the document, so every form refuses it alike.

/// The `outputFormat` values a request may carry, plus the absent member: the
/// modeled set (`smithy_query(query="shape", shape="OutputFormat")`) and the
/// default.
const OUTPUT_FORMATS: &[Option<&str>] = &[
    None,
    Some("CYCLONE_DX_1_5"),
    Some("INSPECTOR"),
    Some("INSPECTOR_ALT"),
];

/// One row: a document carrying one mistyped member, and the member the refusal
/// clause must name.
#[derive(Debug)]
struct MistypedCase {
    what: &'static str,
    sbom: Value,
    member: &'static str,
}

/// A valid document carrying one clean component — the base each mistyped
/// member below is written into.
fn clean_document() -> Value {
    bom(vec![component(
        "ref-clean",
        "left-pad",
        "1.0.0",
        "pkg:npm/left-pad@1.0.0",
    )])
}

/// [`clean_document`] with the member at `path` (from the document root) set to
/// `value`.
fn document_with(path: &[&str], value: Value) -> Value {
    let mut sbom = clean_document();
    let mut cursor = &mut sbom;
    for key in path {
        cursor = &mut cursor[*key];
    }
    *cursor = value;
    sbom
}

/// A document whose only component carries `properties` as `value`. `left-pad`
/// is a package the advisory data holds no rule for, so the scan annotates it.
fn component_properties(value: Value) -> Value {
    let mut entry = component("ref-clean", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0");
    entry["properties"] = value;
    bom(vec![entry])
}

/// A document whose component one level down inside an assembly carries
/// `properties` as `value`. `lodash@4.17.20` is a release the advisory data
/// rules on, so the member is checked whether or not the scan annotates the
/// component.
fn nested_component_properties(value: Value) -> Value {
    let mut child = component("ref-child", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20");
    child["properties"] = value;
    bom(vec![json!({
        "type": "application",
        "bom-ref": "ref-assembly",
        "name": "app",
        "version": "1.0.0",
        "components": [child],
    })])
}

fn mistyped_cases() -> Vec<MistypedCase> {
    vec![
        // `metadata` is an OBJECT.
        MistypedCase {
            what: "metadata sent as a string",
            sbom: document_with(&["metadata"], json!("generated by acme")),
            member: "metadata",
        },
        MistypedCase {
            what: "metadata sent as a number",
            sbom: document_with(&["metadata"], json!(7)),
            member: "metadata",
        },
        MistypedCase {
            what: "metadata sent as an array",
            sbom: document_with(&["metadata"], json!([{ "timestamp": "2026-01-01T00:00:00Z" }])),
            member: "metadata",
        },
        MistypedCase {
            what: "metadata sent as a boolean",
            sbom: document_with(&["metadata"], json!(true)),
            member: "metadata",
        },
        // `metadata.properties` is an ARRAY of name/value pairs.
        MistypedCase {
            what: "metadata.properties sent as an object",
            sbom: document_with(&["metadata", "properties"], json!({ "internal:build": "42" })),
            member: "properties",
        },
        MistypedCase {
            what: "metadata.properties sent as a string",
            sbom: document_with(&["metadata", "properties"], json!("internal:build=42")),
            member: "properties",
        },
        MistypedCase {
            what: "metadata.properties sent as a number",
            sbom: document_with(&["metadata", "properties"], json!(42)),
            member: "properties",
        },
        // `metadata.tools` is the 1.5 object or the deprecated 1.4 array of
        // tool OBJECTS; nothing else names a tool.
        MistypedCase {
            what: "metadata.tools sent as a string",
            sbom: document_with(&["metadata", "tools"], json!("acme-sbom-generator 1.0")),
            member: "tools",
        },
        MistypedCase {
            what: "metadata.tools sent as a number",
            sbom: document_with(&["metadata", "tools"], json!(1)),
            member: "tools",
        },
        MistypedCase {
            what: "metadata.tools sent as a boolean",
            sbom: document_with(&["metadata", "tools"], json!(false)),
            member: "tools",
        },
        MistypedCase {
            what: "a metadata.tools array of strings",
            sbom: document_with(&["metadata", "tools"], json!(["acme-sbom-generator 1.0"])),
            member: "tools",
        },
        MistypedCase {
            what: "a metadata.tools array mixing a tool object and a string",
            sbom: document_with(
                &["metadata", "tools"],
                json!([{ "vendor": "Example", "name": "legacy-generator" }, "acme 1.0"]),
            ),
            member: "tools",
        },
        // `metadata.tools.services` is an ARRAY: the scan appends itself to it.
        MistypedCase {
            what: "metadata.tools.services sent as an object",
            sbom: document_with(
                &["metadata", "tools", "services"],
                json!({ "name": "another-scanner" }),
            ),
            member: "services",
        },
        MistypedCase {
            what: "metadata.tools.services sent as a string",
            sbom: document_with(&["metadata", "tools", "services"], json!("another-scanner")),
            member: "services",
        },
        // `serialNumber` is a STRING identifying the BOM instance.
        MistypedCase {
            what: "serialNumber sent as a number",
            sbom: document_with(&["serialNumber"], json!(42)),
            member: "serialNumber",
        },
        MistypedCase {
            what: "serialNumber sent as an object",
            sbom: document_with(&["serialNumber"], json!({ "uuid": "x" })),
            member: "serialNumber",
        },
        MistypedCase {
            what: "serialNumber sent as an array",
            sbom: document_with(&["serialNumber"], json!(["urn:uuid:0"])),
            member: "serialNumber",
        },
        // A component's `properties` is an ARRAY: the scan appends its
        // annotation to it.
        MistypedCase {
            what: "a component's properties sent as an object",
            sbom: component_properties(json!({ "internal:team": "platform" })),
            member: "properties",
        },
        MistypedCase {
            what: "a component's properties sent as a string",
            sbom: component_properties(json!("internal:team=platform")),
            member: "properties",
        },
        MistypedCase {
            what: "a component's properties sent as a number",
            sbom: component_properties(json!(1)),
            member: "properties",
        },
        MistypedCase {
            what: "a nested component's properties sent as an object",
            sbom: nested_component_properties(json!({ "internal:team": "platform" })),
            member: "properties",
        },
        MistypedCase {
            what: "a nested component's properties sent as a boolean",
            sbom: nested_component_properties(json!(true)),
            member: "properties",
        },
    ]
}

/// How a row's `outputFormat` is named in an assertion message.
fn named_format(format: Option<&str>) -> &str {
    format.unwrap_or("no outputFormat")
}

#[tokio::test]
async fn a_mistyped_member_of_the_submitted_document_is_refused_rather_than_rewritten() {
    let (_engine, router) = emulator();
    for case in mistyped_cases() {
        for format in OUTPUT_FORMATS {
            let mut request = json!({ "sbom": case.sbom.clone() });
            if let Some(format) = format {
                request["outputFormat"] = json!(format);
            }
            let (status, response) = post_scan(&router, &request).await;
            assert_eq!(
                status,
                400,
                "case {:?} ({}): expected HTTP 400, got {status}: {response}",
                case.what,
                named_format(*format)
            );
            assert_eq!(
                response["__type"].as_str(),
                Some("ValidationException"),
                "case {:?} ({}): expected the ValidationException error code, got {response}",
                case.what,
                named_format(*format)
            );
            let expected = invalid_value(case.member);
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {:?} ({}): expected message {expected:?}, got {response}",
                case.what,
                named_format(*format)
            );
            assert!(
                response.get("sbom").is_none(),
                "case {:?} ({}): a refused call carries no report, got {response}",
                case.what,
                named_format(*format)
            );
        }
    }
}

/// The ACCEPTED side of the same rule: every member above carries the types
/// CycloneDX does allow — including `null`, which states no value and is read as
/// absent exactly as an absent `components` is — so the refusal above is the
/// type check and not a refusal of the member itself.
#[tokio::test]
async fn the_types_cyclonedx_allows_at_those_members_are_still_scanned() {
    let accepted = vec![
        ("metadata as an object", document_with(&["metadata"], json!({ "authors": [] }))),
        ("metadata as null", document_with(&["metadata"], Value::Null)),
        (
            "metadata.properties as an array",
            document_with(
                &["metadata", "properties"],
                json!([{ "name": "internal:build", "value": "42" }]),
            ),
        ),
        (
            "metadata.properties as an empty array",
            document_with(&["metadata", "properties"], json!([])),
        ),
        (
            "metadata.properties as null",
            document_with(&["metadata", "properties"], Value::Null),
        ),
        (
            "metadata.tools as the 1.5 object form",
            document_with(
                &["metadata", "tools"],
                json!({ "components": [{ "type": "application", "name": "sbom-generator" }] }),
            ),
        ),
        (
            "metadata.tools as the deprecated 1.4 array of tool objects",
            document_with(
                &["metadata", "tools"],
                json!([{ "vendor": "Example", "name": "legacy-generator" }]),
            ),
        ),
        (
            "metadata.tools as an empty array",
            document_with(&["metadata", "tools"], json!([])),
        ),
        (
            "metadata.tools as null",
            document_with(&["metadata", "tools"], Value::Null),
        ),
        (
            "metadata.tools.services as an array",
            document_with(
                &["metadata", "tools", "services"],
                json!([{ "name": "another-scanner" }]),
            ),
        ),
        (
            "metadata.tools.services as null",
            document_with(&["metadata", "tools", "services"], Value::Null),
        ),
        (
            "serialNumber as a string",
            document_with(
                &["serialNumber"],
                json!("urn:uuid:3e671687-395b-41f5-a30f-a58921a69b79"),
            ),
        ),
        ("serialNumber as null", document_with(&["serialNumber"], Value::Null)),
        (
            "a component's properties as an array",
            component_properties(json!([{ "name": "internal:team", "value": "platform" }])),
        ),
        (
            "a component's properties as an empty array",
            component_properties(json!([])),
        ),
        (
            "a component's properties as null",
            component_properties(Value::Null),
        ),
        (
            "a nested component's properties as an array",
            nested_component_properties(json!([{ "name": "internal:team", "value": "platform" }])),
        ),
    ];

    let (_engine, router) = emulator();
    for (what, sbom) in accepted {
        for format in OUTPUT_FORMATS {
            let mut request = json!({ "sbom": sbom.clone() });
            if let Some(format) = format {
                request["outputFormat"] = json!(format);
            }
            let (status, response) = post_scan(&router, &request).await;
            assert_eq!(
                status,
                200,
                "case {what:?} ({}): expected HTTP 200, got {status}: {response}",
                named_format(*format)
            );
            assert!(
                response["sbom"].is_object(),
                "case {what:?} ({}): expected a report, got {response}",
                named_format(*format)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A member CycloneDX types as a string, carrying another JSON type
// ---------------------------------------------------------------------------
//
// The same class once more, at the members a scan reads a VALUE off instead of
// carrying over: a component's `type`, `name`, `version`, `purl` and `bom-ref`,
// and the document's `bomFormat`, `specVersion` and `serialNumber`. CycloneDX
// types every one of them a string, so a value of another JSON type is not a
// value that member can hold and the document is refused naming it.
//
// Read as ABSENT instead, such a member decides the scan on a coordinate the
// caller never submitted: an unreadable `purl` resolves the component under no
// ecosystem and an unreadable `version` under no release, so a package the
// advisory data does rule on is answered 200 — reported clean, or annotated as
// having no supported rules — and an unreadable `bom-ref` escapes the
// uniqueness rule the specification states for it.
//
// The rows differ along the member, the JSON type sent for it, the nesting
// level (the top level and inside an assembly) and the site (a component's
// member and the document's own). Each one keeps every OTHER member readable on
// a release the advisory data holds a rule for, so a row that fell through
// would be a finding the scan failed to report.

/// One row: a document carrying one string-typed member sent as another JSON
/// type, and the member the refusal clause must name.
#[derive(Debug)]
struct MistypedStringCase {
    what: String,
    sbom: Value,
    member: &'static str,
}

/// Every JSON type that is not a string, excluding `null` — which states no
/// value and is read as absent, as the accepted side below pins.
fn non_string_values() -> Vec<(&'static str, Value)> {
    vec![
        ("a number", json!(42)),
        ("a boolean", json!(true)),
        ("an array", json!(["lodash"])),
        ("an object", json!({ "value": "lodash" })),
    ]
}

/// A component whose every member is readable, naming `lodash@4.17.20` — the
/// release `src/state.rs::seed_vulnerability_db` records CVE-2021-23337 for.
fn vulnerable_component() -> Value {
    component("ref-lodash", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20")
}

/// [`vulnerable_component`] with `member` replaced by `value`.
fn vulnerable_component_with(member: &str, value: Value) -> Value {
    let mut entry = vulnerable_component();
    entry[member] = value;
    entry
}

/// A document whose only component is [`vulnerable_component_with`].
fn component_with(member: &str, value: Value) -> Value {
    bom(vec![vulnerable_component_with(member, value)])
}

/// The same, one level down inside an assembly: a component's members are read
/// at every level of the tree, so what is refused does not depend on the depth.
fn nested_component_with(member: &str, value: Value) -> Value {
    let child = vulnerable_component_with(member, value);
    bom(vec![json!({
        "type": "application",
        "bom-ref": "ref-assembly",
        "name": "app",
        "version": "1.0.0",
        "components": [child],
    })])
}

/// A document whose only component is [`vulnerable_component`] carrying no
/// `member` at all.
fn component_without(member: &str) -> Value {
    let mut entry = vulnerable_component();
    entry
        .as_object_mut()
        .expect("the component helper builds a JSON object")
        .remove(member);
    bom(vec![entry])
}

fn mistyped_string_cases() -> Vec<MistypedStringCase> {
    let mut cases = Vec::new();

    // A component's own string-typed members, at the top level and nested.
    for member in ["type", "name", "version", "purl", "bom-ref"] {
        for (shape, value) in non_string_values() {
            cases.push(MistypedStringCase {
                what: format!("a component's {member} sent as {shape}"),
                sbom: component_with(member, value.clone()),
                member,
            });
            cases.push(MistypedStringCase {
                what: format!("a nested component's {member} sent as {shape}"),
                sbom: nested_component_with(member, value),
                member,
            });
        }
    }

    // The document's own string-typed members.
    for member in ["bomFormat", "specVersion", "serialNumber"] {
        for (shape, value) in non_string_values() {
            let mut sbom = bom(vec![vulnerable_component()]);
            sbom[member] = value;
            cases.push(MistypedStringCase {
                what: format!("a document's {member} sent as {shape}"),
                sbom,
                member,
            });
        }
    }

    // A `version` that is not a string, on a `purl` that names no version
    // either: nothing is left to resolve the release from, so falling through
    // would compare a vulnerable component against no version at all.
    let mut unversioned = vulnerable_component();
    unversioned["purl"] = json!("pkg:npm/lodash");
    unversioned["version"] = json!(4.17);
    cases.push(MistypedStringCase {
        what: "a component's version sent as a number, with a purl naming none".to_string(),
        sbom: bom(vec![unversioned]),
        member: "version",
    });

    // Two components whose refs are both unreadable: the uniqueness rule cannot
    // be applied to a ref that is not a string, so the member is refused first.
    cases.push(MistypedStringCase {
        what: "two components whose bom-ref is the same non-string value".to_string(),
        sbom: bom(vec![
            vulnerable_component_with("bom-ref", json!(5)),
            {
                let mut other =
                    component("ref-minimist", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5");
                other["bom-ref"] = json!(5);
                other
            },
        ]),
        member: "bom-ref",
    });

    cases
}

#[tokio::test]
async fn a_string_typed_member_carrying_another_json_type_is_refused_naming_it() {
    let (_engine, router) = emulator();
    for case in mistyped_string_cases() {
        let (status, response) = post_scan(&router, &json!({ "sbom": case.sbom.clone() })).await;
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
        let expected = invalid_value(case.member);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {:?}: expected message {expected:?}, got {response}",
            case.what
        );
        assert!(
            response.get("sbom").is_none(),
            "case {:?}: a refused call carries no report, got {response}",
            case.what
        );
    }
}

/// The ACCEPTED side of that rule: the same members carrying the string
/// CycloneDX types them as, absent, or present as `null` — which states no
/// value — are all scanned, so the refusals above are the type check and not a
/// refusal of the members themselves.
#[tokio::test]
async fn the_string_typed_members_are_read_when_they_carry_a_string() {
    let accepted = vec![
        ("every member readable", bom(vec![vulnerable_component()])),
        (
            "a component nested inside an assembly",
            nested_component_with("name", json!("lodash")),
        ),
        ("no version", component_without("version")),
        ("a null version", component_with("version", Value::Null)),
        ("no purl", component_without("purl")),
        ("a null purl", component_with("purl", Value::Null)),
        ("no bom-ref", component_without("bom-ref")),
        ("a null bom-ref", component_with("bom-ref", Value::Null)),
    ];

    let (_engine, router) = emulator();
    for (what, sbom) in accepted {
        let (status, response) = post_scan(&router, &json!({ "sbom": sbom })).await;
        assert_eq!(
            status, 200,
            "case {what:?}: expected HTTP 200, got {status}: {response}"
        );
        assert!(
            response["sbom"].is_object(),
            "case {what:?}: expected a report, got {response}"
        );
    }
}

/// And the component those rows are written around IS one the advisory data
/// rules on: with every member readable the scan reports the finding, so a row
/// refused above is a finding that silently went missing before.
#[tokio::test]
async fn a_readable_component_is_ruled_on_rather_than_reported_clean() {
    let (_engine, router) = emulator();
    let (status, response) =
        post_scan(&router, &json!({ "sbom": bom(vec![vulnerable_component()]) })).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    let ids: Vec<String> = response["sbom"]["vulnerabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|finding| finding["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        ids,
        vec!["CVE-2021-23337".to_string()],
        "a readable lodash@4.17.20 is ruled on by the stored advisory, got {response}"
    );
}
