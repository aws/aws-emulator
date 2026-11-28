Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 87114bf6684e0f878cdb2d8b5aad68cc7796cae8

# A 1.4-style `metadata.tools[]` entry's members escape the accessor rule: a mistyped `vendor`/`name`/`version` is copied into the report's tool component instead of refused

_Filed: 2026-11-28T11:39:21Z_
_Operation: ScanSbom | Class: validation_

> _Note: this issue was identified against the code base at commit `87114bf6684e0f878cdb2d8b5aad68cc7796cae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE rule for the submitted document: a member the report is built from, or that the scan reads a value off, carrying a JSON type CycloneDX does not allow there is REFUSED with `ValidationException`, and every such member goes through an accessor (`submitted_string`, `submitted_properties`, `submitted_services`, `submitted_metadata`, `submitted_tools`). `submitted_tools` applies that rule to the deprecated 1.4 array form only at the ENTRY level — it refuses an entry that is not an object, and its doc says why: "[`tool_component`] reads a member off it with none to read, so it would be reported as a tool component carrying nothing the caller sent."

But `tool_component` itself, the function that doc names, reads five members off an accepted entry with no accessor at all: `entry.get("vendor")` is cloned into `publisher`, and `name`, `version`, `hashes`, `externalReferences` are cloned member-for-member. The result is written into the CycloneDX report at `metadata.tools.components[]`, i.e. as a CycloneDX COMPONENT object (`tool_component`'s own doc: "a submitted array is carried over into `components` ... mapping `vendor` onto the component `publisher` the specification names").

So one path in this handler refuses a non-string `name`/`version` on a `components[]` entry — `resolve_component_tree` reads both through `submitted_string` precisely because "a value of another JSON type is not a value that member can hold" — while the path four hundred lines later BUILDS a component whose `name`, `version` and `publisher` are whatever JSON the caller put in the tool entry. A document carrying `metadata.tools: [{"vendor": 7, "name": {"generator": "acme"}, "version": [1], "hashes": "none"}]` is answered HTTP 200 with a report the response presents as a CycloneDX 1.5 BOM (`specVersion` is re-stamped to `1.5`) whose `metadata.tools.components[0]` holds `"publisher": 7`, `"name": {"generator": "acme"}`, `"version": [1]`, while the SAME mistyping one level up (`metadata.tools: ["acme 1.0"]`) and the same mistyping of `name` on a `components[]` entry are both refused HTTP 400.

The escape is not specific to the CycloneDX form: the type check for the document's members runs in `parse_sbom`/`validate_report_members` for every `outputFormat` by design ("whether a submitted document is a CycloneDX 1.5 BOM is a property of the document, not of the form the answer is rendered in"), and this member is unchecked there too, so it escapes in all three forms.

(Mechanically related, same code: because the copy list is fixed, a member of the tool entry outside `{vendor, name, version, hashes, externalReferences}` — e.g. a submitted `publisher` — is silently dropped from the report rather than carried over.)

## Evidence

The rule, module header — services/service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs:21-29:
```
//! The report ADDS to the submitted document rather than restating it, so a
//! member it is built from that carries a JSON type CycloneDX does not allow
//! there is refused with the same `ValidationException` a mistyped `components`
//! gets ([`validate_report_members`]) — a value of the wrong type cannot carry
//! the scan's own data beside it, and a report that wrote the scan's data in its
//! place would state the caller sent something it did not. The members the scan
//! reads a VALUE off are refused by the same rule ([`submitted_string`]): one of
//! the wrong type is not a value that member can hold
```

Side 1 — the accessor every other `name`/`version` goes through, src/ops/scan_sbom.rs:905-913 and :923-931:
```
/// The string a submitted member carries, `None` when the object states none,
/// or a refusal when the member is present and is not a string.
///
/// CycloneDX types every member this scan reads a VALUE off a string —
/// `bomFormat`, `specVersion` and `serialNumber` on the document, and `type`,
/// `name`, `version`, `purl` and `bom-ref` on a component — so a value of
/// another JSON type is not a value that member can hold, and the document is
/// refused for the member it is
...
fn submitted_string<'o>(
    object: &'o Map<String, Value>,
    member: &'static str,
) -> Result<Option<&'o str>, ScanSbomError> {
    match object.get(member) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(invalid(SbomDefect::InvalidValue(member))),
```
applied to a `components[]` entry — src/ops/scan_sbom.rs:1053-1055:
```
        let name = submitted_string(object, "name")?.unwrap_or_default();
        let declared_version = submitted_string(object, "version")?.unwrap_or_default();
        let purl = submitted_string(object, "purl")?;
```

Side 1b — the entry-level check and the reason it names `tool_component`, src/ops/scan_sbom.rs:875-889:
```
/// Which form the submitted `metadata.tools` is in, or a refusal when the
/// member is present in neither.
///
/// An ENTRY of the array form is a tool object; one of another type names no
/// tool, and [`tool_component`] reads a member off it with none to read, so it
/// would be reported as a tool component carrying nothing the caller sent.
fn submitted_tools(metadata: &Map<String, Value>) -> Result<SubmittedTools<'_>, ScanSbomError> {
    match metadata.get("tools") {
        None | Some(Value::Null) => Ok(SubmittedTools::Absent),
        Some(Value::Object(tools)) => Ok(SubmittedTools::Object(tools)),
        Some(Value::Array(entries)) if entries.iter().all(Value::is_object) => {
            Ok(SubmittedTools::Entries(entries))
        }
        Some(_) => Err(invalid(SbomDefect::InvalidValue("tools"))),
```

Side 2 — the path that skips the accessor, src/ops/scan_sbom.rs:1655-1675:
```
/// A deprecated 1.4-style `metadata.tools[]` entry as the tool COMPONENT the
/// 1.5 object form holds.
///
/// CycloneDX 1.5 replaces the array of `{vendor, name, version}` tool objects
/// with an object carrying `components` and `services`, and the scan reports
/// itself under the latter — so a submitted array is carried over into
/// `components` rather than dropped, mapping `vendor` onto the component
/// `publisher` the specification names.
fn tool_component(entry: &Value) -> Value {
    let mut component = Map::new();
    component.insert("type".to_string(), json!("application"));
    if let Some(vendor) = entry.get("vendor") {
        component.insert("publisher".to_string(), vendor.clone());
    }
    for member in ["name", "version", "hashes", "externalReferences"] {
        if let Some(value) = entry.get(member) {
            component.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(component)
}
```
and where its output lands in the report, src/ops/scan_sbom.rs:1819-1830:
```
    let mut tools = match submitted_tools(&metadata)? {
        SubmittedTools::Absent => Map::new(),
        SubmittedTools::Object(existing) => existing.clone(),
        SubmittedTools::Entries(entries) => {
            let mut object = Map::new();
            object.insert(
                "components".to_string(),
                Value::Array(entries.iter().map(tool_component).collect()),
            );
            object
        }
    };
```

The check is claimed to be exhaustive over "the members the report is built from" — src/ops/scan_sbom.rs:973-981 (`validate_report_members`):
```
/// Every member is read through the accessor [`cyclonedx_report`] reads it
/// with, so ONE rule decides both what is refused and what a report carries,
/// and the check runs for every `outputFormat`: whether a submitted document is
/// a CycloneDX 1.5 BOM is a property of the document, not of the form the
/// answer is rendered in.
```

Test coverage — the mistyped-member table pins the entry level and the siblings but never a tool entry's own members. tests/all/scan_sbom_refusals.rs:452-461 states the rule, and :595-607 are the only `tools`-array rows:
```
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
```
asserted at tests/all/scan_sbom_refusals.rs:684-699 (HTTP 400 + `ValidationException` + `Invalid value for "<member>"`), for every entry of `OUTPUT_FORMATS` (:476-481). The only 1.4-array row in the success suite sends well-typed members — tests/all/scan_report_annotations.rs:709-715:
```
        ToolsCase {
            // CycloneDX 1.5 deprecates the array form; its entries are carried
            // over rather than dropped.
            what: "the deprecated 1.4 array form",
            tools: Some(json!([ { "vendor": "Example", "name": "legacy-generator" } ])),
            preserved: Some("legacy-generator"),
        },
```
and it asserts only `tools.to_string().contains("legacy-generator")` (:759-767), so nothing pins the type of what `tool_component` copies.

## Steps to Reproduce

1. POST /scan/sbom with {"outputFormat":"CYCLONE_DX_1_5","sbom":{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"metadata":{"tools":[{"vendor":7,"name":{"generator":"acme"},"version":[1],"hashes":"none"}]},"components":[{"type":"library","bom-ref":"ref-1","name":"left-pad","version":"1.0.0","purl":"pkg:npm/left-pad@1.0.0"}]}}
2. Observe HTTP 200 and read `sbom.metadata.tools.components[0]`: it is `{"type":"application","publisher":7,"name":{"generator":"acme"},"version":[1],"hashes":"none"}` — the caller's values copied verbatim into a component object of a report whose `specVersion` the same response re-stamps to "1.5" (src/ops/scan_sbom.rs:1767-1770)
3. Send the same document with `metadata.tools` as `["acme 1.0"]`: HTTP 400 `Input SBOM failed validation: Invalid value for "tools"` (pinned by tests/all/scan_sbom_refusals.rs:596-599)
4. Send the same document with the component's own `name` as `{"generator":"acme"}` instead: HTTP 400 `Input SBOM failed validation: Invalid value for "name"` (src/ops/scan_sbom.rs:1053 via `submitted_string`) — the identical mistyping of the identical member name is refused on a `components[]` entry and accepted on a tool entry
5. Repeat step 1 with "outputFormat":"INSPECTOR" and with no `outputFormat`: still HTTP 200, so the member escapes the check in every form, contrary to `validate_report_members`' stated scope (src/ops/scan_sbom.rs:973-981)
