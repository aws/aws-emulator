Issue-Source: trace-sweep-divergence
Issue-Trace: inspector-scan-error-sbom-cannot-parse
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector-scan-error-sbom-cannot-parse inspector-scan-error-sbom-missing-required-fields inspector-scan-error-sbom-unsupported-type
Issue-Shape-Id: com.amazonaws.inspectorscan#InspectorScan
Issue-Key: inspector-scan|ScanSbom|4xx->4xx|$.reason
Issue-Key: inspector-scan|ScanSbom|4xx->4xx|$.fields
Issue-Commit: 7bfef4555d8ff23b4aee8bc4649f9354551130b5

# ScanSbom ValidationException body carries reason and fields members AWS does not return

> _Note: this issue was identified against the code base at commit `7bfef4555d8ff23b4aee8bc4649f9354551130b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScanSbom

## Description
ScanSbom refuses an unscannable SBOM with the right status and the right message, but the JSON error body it serializes does not have the shape the recorded AWS responses have: the emulator appends members (`reason`, and `fields` when the failure names a member) at top level that the recorded AWS ValidationException bodies for these same refusals do not carry. Two causes, one per distinct refusal class in the corpus.

Cause 1 of 2 - unparseable SBOM document adds `reason`.
  AWS: Smithy: ScanSbom (POST /scan/sbom, restJson1) models error ValidationException @httpError 400 with members message (@required), reason (@required, ValidationExceptionReason: UNKNOWN_OPERATION | CANNOT_PARSE | FIELD_VALIDATION_FAILED | UNSUPPORTED_SBOM_TYPE | OTHER) and fields (ValidationExceptionFields). Recorded behavior only for what is actually serialized: the recorded 400 response for a request whose `sbom` document cannot be read as a CycloneDX BOM carries the error's `message` only - the recording has no value at `$.reason` (and none at `$.fields`). The message itself matched on replay, so the fixed prefix plus the condition clause ("Invalid json" for a body that is not a readable BOM object) is the recorded wording; its only variable parts are the member name/limit the other condition clauses interpolate.
  Emulator: answers 400 with the same message but adds `$.reason` = "CANNOT_PARSE" to the body. Replay detail: `$.reason: type_mismatch (expected None, got 'CANNOT_PARSE')` for fixture inspector-scan-error-sbom-cannot-parse step task#1 (expected_status 400, actual_status 400).
  Why: a client that branches on the presence of `reason` to distinguish a parse refusal from a field refusal takes a branch against this emulator that it can never take against AWS, and a client that asserts on the exact error body (SDK round-trip comparisons, recorded-response assertions, proxies that re-serialize the error) sees a member AWS never sent. The error class is then not reproducible between the emulator and AWS even though status, code and message agree.
  Fix: the ScanSbom refusal serializer / its error rendering - the `refused()` helper that builds ValidationException for this operation, which currently always populates `reason` from RefusalReason::wire(). Hypothesis: the member is populated because the model marks it @required, but the recorded wire shows AWS omits it for this operation's refusals; the recorded body is the requirement.
  Traces: inspector-scan-error-sbom-cannot-parse#task#1.

Cause 2 of 2 - member-level refusal adds both `reason` and `fields`.
  AWS: same modeled ValidationException as above (message @required, reason, fields of ValidationExceptionField{name @required, message @required}). Recorded behavior only for the wire: the recorded 400 responses for a document that states no/unsupported `bomFormat` carry `message` only - nothing at `$.reason`, nothing at `$.fields`. The recorded message is the fixed validation prefix plus the condition clause `Invalid value for "<member>"`, whose variable part is the CycloneDX member name taken from the request (here the document member that failed), not a literal from any one run.
  Emulator: answers 400 with the matching message but adds `$.reason` = "FIELD_VALIDATION_FAILED" and `$.fields` = a one-element list `[{"name": "<member>", "message": "Invalid value for \"<member>\""}]` naming the failing member. Replay detail: `$.fields: type_mismatch (expected None, got [{'name': 'bomFormat', 'message': 'Invalid value for "bomFormat"'}])` and `$.reason: type_mismatch (expected None, got 'FIELD_VALIDATION_FAILED')`. Cluster covers fixtures inspector-scan-error-sbom-missing-required-fields and inspector-scan-error-sbom-unsupported-type, both step task#1.
  Why: a client reading `fields[].name` to point a user at the offending SBOM member gets a populated list from the emulator and an absent one from AWS, so code developed against the emulator mis-handles the real service; and any byte- or body-level comparison against AWS fails on two extra members. It also hides the divergence class: the refusal looks richer than the service it emulates.
  Fix: the same ScanSbom refusal rendering / validation path - the member-level defect classes (unsupported format, invalid/absent member value) feed reason and a ValidationExceptionField list into the serialized error; the serialized body must carry only what AWS returns for these refusals.
  Traces: inspector-scan-error-sbom-missing-required-fields#task#1, inspector-scan-error-sbom-unsupported-type#task#1.

Not covered by any open issue: the open inspector-scan issues (20261124T173950Z-inspector-scan-scansbom-validation--r2, 20261123T220716Z-inspector-scan-scansbom-wrong-behavior--r2, 20261125T163407Z-inspector-scan-scansbom-maintainability--r2) concern the report's `version` accessor, finding order and duplicated severity classification respectively; none of them owns these replay keys or the refusal body shape.

## Evidence
Replay of simparity-wire-aws-v1 (9 fixtures, 11 steps, 8 passed, 3 mismatched, score 72.7%).

Cluster 1 - fixture inspector-scan-error-sbom-cannot-parse, step task#1, operation ScanSbom, verdict mismatch, expected_status 400, actual_status 400:
  detail: $.reason: type_mismatch (expected None, got 'CANNOT_PARSE')
  Expected (recorded AWS body): { "message": "<validation prefix>Invalid json" } - no reason, no fields.
  Actual (emulator body): same message plus "reason": "CANNOT_PARSE".

Cluster 2 - fixture inspector-scan-error-sbom-missing-required-fields, step task#1 (also affects inspector-scan-error-sbom-unsupported-type#task#1), operation ScanSbom, verdict mismatch, expected_status 400, actual_status 400:
  detail: $.fields: type_mismatch (expected None, got [{'name': 'bomFormat', 'message': 'Invalid value for "bomFormat"'}])
           $.reason: type_mismatch (expected None, got 'FIELD_VALIDATION_FAILED')
  Expected (recorded AWS body): { "message": "<validation prefix>Invalid value for \"<member>\"" } only.
  Actual (emulator body): same message plus "reason": "FIELD_VALIDATION_FAILED" and a "fields" list naming the member.

Status (400) and the error message matched in all three steps; the ONLY divergence is the extra top-level members in the error body. Neither cluster carries a stub_path or an owned_by, so neither is already filed.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: POST /scan/sbom (ScanSbom) with a `sbom` document that cannot be read as a CycloneDX 1.5 BOM object (fixture inspector-scan-error-sbom-cannot-parse, step task#1). Observe HTTP 400 whose JSON body contains a `reason` member ("CANNOT_PARSE") that the recorded AWS response does not contain.
3. Cause 2: POST /scan/sbom with a document whose `bomFormat` is absent or names a format other than CycloneDX (fixtures inspector-scan-error-sbom-missing-required-fields and inspector-scan-error-sbom-unsupported-type, step task#1). Observe HTTP 400 whose JSON body contains both `reason` ("FIELD_VALIDATION_FAILED") and a `fields` list naming the member, neither of which is present in the recorded AWS response.
4. Diff each response body against the recorded fixture response: only `$.reason` / `$.fields` differ; status and message agree.
