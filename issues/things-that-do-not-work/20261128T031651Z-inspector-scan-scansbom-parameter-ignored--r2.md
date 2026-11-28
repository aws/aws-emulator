Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8ebfccff504c117aa638595512a8ca5cbfbe1aaf

# The CycloneDX report erases a submitted `vulnerabilities` array instead of appending to it, the only array the scan writes into that is neither preserved nor type-checked

_Filed: 2026-11-28T03:16:51Z_
_Operation: ScanSbom | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8ebfccff504c117aa638595512a8ca5cbfbe1aaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`cyclonedx_report` builds the `CYCLONE_DX_1_5` report as "the input BOM augmented with the scan's results" and, for every array member the scan writes into, it reads the submitted value through a `submitted_*` accessor, keeps the caller's entries and APPENDS the scan's own: `metadata.properties` (`properties.extend(summary.cyclonedx_properties())`), `metadata.tools.services` (`services.push(scanner_service(..))`) and each component's `properties` (`list.push(property)` in `annotate_skipped_components`). The crate states WHY: a submitted value "left to fall through ... would be REPLACED by an array holding the scan's own entries alone — a 200 response stating the caller sent something it did not", which is also why a value of the wrong JSON type at those members is refused with `ValidationException`.

The document-level `vulnerabilities` member — the fourth array the scan writes into, and the one the report's whole result lives at — is handled by neither half of that rule:

1. `report.insert("vulnerabilities", Value::Array(findings...))` overwrites whatever the submitted document carried there. `sbom` is a Smithy `document` (arbitrary caller JSON), so a submitted BOM that already carries `vulnerabilities` entries (the member the report itself writes at that path) has every one of them silently discarded in a 200 response — exactly the "stating the caller sent something it did not" harm the sibling accessor's doc names, and the opposite of the handler's own header rule that "the report ADDS to the submitted document rather than restating it".
2. There is no accessor and therefore no type check for it: `"vulnerabilities": "none"` or `{}` is accepted and silently dropped, while the same mistyping at `metadata`, `metadata.properties`, `metadata.tools.services`, `components` or `serialNumber` is refused HTTP 400 `Input SBOM failed validation: Invalid value for "<member>"`. `validate_report_members` — documented as "Every member is read through the accessor [`cyclonedx_report`] reads it with, so ONE rule decides both what is refused and what a report carries" — never looks at it.

Side effect of 1: two submitted documents that differ ONLY in their `vulnerabilities` entries are answered byte-identical reports (they differ in digest, so they are two distinct scans, but nothing of the difference survives into the response).

This is a different member and a different harm from the open issue on the document-level `version` member (which is echoed back rather than erased): here the caller's data is destroyed, not misreported.

No test covers a submitted `vulnerabilities` member — the suite's `bom()` helper never sends one — while the three sibling arrays each have a "submitted entries first, scan's appended" row.

## Evidence

The rule the crate states for every array the scan writes into — services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:825-835:
```
/// The `properties` array of a submitted object — a `components[]` entry's or
/// the document's `metadata` — `None` when it carries none, or a refusal when
/// the member is present and is not an array.
///
/// CycloneDX declares `properties` an array of name/value pairs at BOTH sites,
/// and the report APPENDS the scan's own entries to it ([`annotate_skipped_components`]
/// and the summary in [`cyclonedx_report`]), so one rule reads it for both: a
/// value of another type cannot carry the scan's entries beside what the caller
/// submitted, and left to fall through it would be REPLACED by an array holding
/// the scan's own entries alone — a 200 response stating the caller sent
/// something it did not.
```
and the same rule carried to `services` — src/ops/scan_sbom.rs:892-903:
```
/// The `services` array of a submitted 1.5 `metadata.tools`, `None` when it
/// names none, or a refusal when the member is present and is not an array.
///
/// The scan appends ITSELF to this array, so the same rule as
/// [`submitted_properties`] holds for it.
fn submitted_services(tools: &Map<String, Value>) -> Result<Option<&Vec<Value>>, ScanSbomError> {
```
The handler's own header rule — src/ops/scan_sbom.rs:21-24:
```
//! The report ADDS to the submitted document rather than restating it, so a
//! member it is built from that carries a JSON type CycloneDX does not allow
//! there is refused with the same `ValidationException` a mistyped `components`
//! gets ([`validate_report_members`])
```
The three siblings, preserved and appended — src/ops/scan_sbom.rs:1798-1813:
```
    let mut services = match submitted_services(&tools)? {
        Some(existing) => existing.clone(),
        None => Vec::new(),
    };
    services.push(scanner_service(stamp.scanner_version));
...
    let mut properties = match submitted_properties(&metadata)? {
        Some(existing) => existing.clone(),
        None => Vec::new(),
    };
    properties.extend(summary.cyclonedx_properties());
```
The one that is neither read nor preserved, four lines later — src/ops/scan_sbom.rs:1817-1820:
```
    report.insert(
        "vulnerabilities".to_string(),
        Value::Array(findings.iter().map(cyclonedx_vulnerability).collect()),
    );
```
(no `submitted_*` accessor, no type check, no `existing.clone()`.)

`validate_report_members` never sees the member — src/ops/scan_sbom.rs:953-964:
```
fn validate_report_members(document: &Map<String, Value>) -> Result<(), ScanSbomError> {
    let _serial_number = submitted_serial_number(document)?;
    let Some(metadata) = submitted_metadata(document)? else {
        return Ok(());
    };
    let _properties = submitted_properties(metadata)?;
    if let SubmittedTools::Object(tools) = submitted_tools(metadata)? {
        let _services = submitted_services(tools)?;
    }
    Ok(())
}
```

Test coverage proving the siblings are pinned and this member is untested — tests/all/scan_report_annotations.rs:993-1012 drives the preserve-and-append table over `components[0].properties`, the nested component's `properties`, `metadata.properties` and `metadata.tools.services`:
```
            path: &["metadata", "properties"],
            submitted: json!([{ "name": "internal:build", "value": "42" }]),
...
            path: &["metadata", "tools", "services"],
            submitted: json!([{ "name": "another-scanner" }]),
```
asserted at tests/all/scan_report_annotations.rs:1034-1040:
```
        assert_eq!(
            reported.get(..submitted.len()).map(<[Value]>::to_vec),
            Some(submitted.clone()),
            "case {:?}: expected the submitted entries {:?} first, got {reported:?}",
```
No row and no refusal case anywhere in tests/all/ names `vulnerabilities` on the INPUT document (the `bom()` helper, tests/all/scan_sbom.rs:86-96, never sends one), while the mistyped-member refusal table does cover every sibling — tests/all/scan_sbom_refusal_reason.rs:221-237 (`metadata` sent as a string, `serialNumber` sent as a number -> HTTP 400).

## Steps to Reproduce

1. POST /scan/sbom with {"outputFormat":"CYCLONE_DX_1_5","sbom":{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"metadata":{"properties":[{"name":"internal:build","value":"42"}]},"components":[{"type":"library","bom-ref":"ref-1","name":"lodash","version":"4.17.20","purl":"pkg:npm/lodash@4.17.20"}],"vulnerabilities":[{"bom-ref":"vuln-known","id":"CVE-1999-0001","description":"an advisory the caller already knew about"}]}}
2. Read the 200 body: `sbom.vulnerabilities` holds only the scan's own node for CVE-2021-23337; the submitted `vuln-known` entry is gone, while in the SAME response `sbom.metadata.properties` still carries the submitted `internal:build` entry with the scan summary appended after it (tests/all/scan_report_annotations.rs:1034-1040 pins that behaviour for the sibling member)
3. Send the same document twice, once with the `vulnerabilities` entry above and once with a different one: both are answered the identical report body, so nothing of the difference the caller sent survives
4. POST the same document with "vulnerabilities": "none" (or {} or 7): observe HTTP 200 and the member silently replaced, while the same document carrying "metadata": "generated by acme" is refused HTTP 400 `Input SBOM failed validation: Invalid value for "metadata"` (tests/all/scan_sbom_refusal_reason.rs:221-228)
