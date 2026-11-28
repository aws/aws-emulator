Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c524fdf245ab30ebeb0886af25e07dc8ca59cca3

# The CycloneDX report's own `version` member escapes the accessor rule every sibling document member goes through: a null is not read as absent and a non-numeric value is neither refused nor replaced

_Filed: 2026-11-24T17:39:50Z_
_Operation: ScanSbom | Class: validation_

> _Note: this issue was identified against the code base at commit `c524fdf245ab30ebeb0886af25e07dc8ca59cca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`cyclonedx_report` writes two document-level members whose rule the handler states identically — "PRESERVED as submitted ... and only defaulted when absent": `serialNumber` and `version`. `serialNumber` is read through the `submitted_*` accessor family, so (a) a value of a JSON type the member cannot hold is refused with `Invalid value for "serialNumber"` and (b) a value present as `null` is read as ABSENT, which makes the scan stamp its derived URN. `version` is read with a bare `report.contains_key("version")` instead, so it gets neither behaviour:

1. `"version": null` — `contains_key` is true for an explicit null, so the scan's default is NOT applied and the report answers `"version": null`. That breaks the crate's own null rule, which it states twice and implements in every accessor (`None | Some(Value::Null) => Ok(None)`): "A member present as `null` states no value, which is read as absent exactly as an absent member is." Read as absent, the report would carry the `1` the handler itself names as the schema default; instead the emitted CycloneDX 1.5 document carries a null revision number the caller never asked for.
2. `"version": "one"` / `{}` / `[]` — accepted silently and echoed into the report, while the member it is explicitly paired with (`serialNumber`) is refused 400 for exactly the same mistyping. No accessor states the type this member can hold, so `validate_report_members` — documented as "Every member is read through the accessor [`cyclonedx_report`] reads it with, so ONE rule decides both what is refused and what a report carries" — never sees it, and `version` is the only member the report writes that the rule does not cover.

`sbom` is a Smithy `document` (arbitrary caller JSON), so both inputs are reachable over the wire; no test covers them because the suite's `bom()` helper always sends `"version": 1`.

## Evidence

The rule, stated by the handler and implemented by every accessor — services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:861-872:
```
/// A member present as `null` states no value, which is read as absent exactly
/// as an absent member is.
fn submitted_string<'o>(
    object: &'o Map<String, Value>,
    member: &'static str,
) -> Result<Option<&'o str>, ScanSbomError> {
    match object.get(member) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(invalid(SbomDefect::InvalidValue(member))),
    }
}
```
And the one-rule claim for the members the report is built from — src/ops/scan_sbom.rs:884-905:
```
/// Refuse a submitted document that carries one of the members the report is
/// built from with a JSON type CycloneDX does not allow there.
///
/// Every member is read through the accessor [`cyclonedx_report`] reads it
/// with, so ONE rule decides both what is refused and what a report carries,
/// ... A member present as `null` states no value, which is
/// read as absent exactly as `components` already is.
fn validate_report_members(document: &Map<String, Value>) -> Result<(), ScanSbomError> {
    let _serial_number = submitted_serial_number(document)?;
```
(the body checks `serialNumber`, `metadata`, `properties`, `tools`, `services` — never `version`.)

The two sides in ONE function, ten lines apart — src/ops/scan_sbom.rs:1659-1679:
```
    // `serialNumber` identifies the BOM INSTANCE, so a submitted one is
    // PRESERVED for the same reason `version` is below — the report must not
    // misreport what the caller sent — and the content-derived URN is stamped
    // only when the submitted document names none (or names an empty one,
    // which identifies nothing).
    let submitted_serial =
        submitted_serial_number(&report)?.is_some_and(|serial| !serial.is_empty());
    if !submitted_serial {
        report.insert(
            "serialNumber".to_string(),
            Value::String(stamp.serial_number.to_string()),
        );
    }
    // `version` is the BOM's own revision number. It is PRESERVED as submitted —
    // the report must not misreport what the caller sent — and only defaulted
    // when absent, to the `1` the CycloneDX 1.5 schema defaults it to.
    if !report.contains_key("version") {
        report.insert("version".to_string(), json!(1));
    }
```
`contains_key` is true for `Some(Value::Null)`, so the "only defaulted when absent" comment does not hold for a null member under the crate's own null-is-absent rule.

Test coverage proving the gap is untested and that the sibling members ARE pinned — tests/all/scan_sbom_refusals.rs:910-921:
```
    // The document's own string-typed members.
    for member in ["bomFormat", "specVersion", "serialNumber"] {
        for (shape, value) in non_string_values() {
            let mut sbom = bom(vec![vulnerable_component()]);
            sbom[member] = value;
```
and the accepted side pins null-as-absent for component members only — tests/all/scan_sbom_refusals.rs:997-1000:
```
        ("no version", component_without("version")),
        ("a null version", component_with("version", Value::Null)),
```
The document-level `version` appears in the suite only as the always-valid `"version": 1` of the `bom()` helper — tests/all/scan_sbom.rs:90.

## Steps to Reproduce

1. POST /scan/sbom with {"outputFormat":"CYCLONE_DX_1_5","sbom":{"bomFormat":"CycloneDX","specVersion":"1.5","version":null,"components":[{"type":"library","bom-ref":"ref-1","name":"lodash","version":"4.17.20","purl":"pkg:npm/lodash@4.17.20"}]}}
2. Read the 200 body: `sbom.version` is `null`, not the `1` the handler's own comment names as the default for a member it says is "only defaulted when absent" — although every other member of the document present as `null` is read as absent (a component's null `version`/`purl` are scanned, tests/all/scan_sbom_refusals.rs:997-1000)
3. Compare with the adjacent member in the same function: send the same document with `"serialNumber": null` instead — the report carries the stamped `urn:uuid:` serial, i.e. null WAS read as absent there
4. POST the same document with "version": "one" (or {} or []): observe HTTP 200 and `sbom.version == "one"` echoed into a document the response declares to be CycloneDX 1.5, while the same document carrying "serialNumber": 7 is refused HTTP 400 `Input SBOM failed validation: Invalid value for "serialNumber"`
