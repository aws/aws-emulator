Issue-Source: trace-sweep-divergence
Issue-Trace: create-export-duplicate-name-rejected-until-deleted
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-export-duplicate-name-rejected-until-deleted
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports|CreateExport|4xx->4xx|$.Message
Issue-Key: bcm-data-exports|CreateExport|2xx->2xx|$.ExportArn
Issue-Commit: dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e

# bcm-data-exports CreateExport returns emulator-authored duplicate-name message and a mis-shaped ExportArn

> _Note: this issue was identified against the code base at commit `dd9cf420697e5d0dd5f9eb36b5f6cf849c89f05e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
Two net-new wire root causes in the CreateExport handler of this service. Both are reached by the same fixture's name-reuse sequence (create, re-create under the live name, delete, re-create), and both are in the response CreateExport serializes — one on its refusal, one on its success.

Cause 1 of 2: the duplicate-export-name refusal carries the emulator's own sentence instead of AWS's fixed one.
AWS: a second CreateExport that reuses the name of an export the account currently holds is refused HTTP 400 with the modelled client error ValidationException (Smithy: CreateExport errors include ValidationException @httpError 400; ValidationException.Message is @required, @length(0,1024); the model does not state the sentence, so this is recorded behavior for the wording). The recorded response is status 400 with Message exactly `Cannot create duplicate export name.` — the ENTIRE sentence is fixed text; no part of it is taken from the request or from state, in particular not the requested export name. The live export is untouched and the name becomes creatable again after DeleteExport.
Emulator: answers the right status (400) and the right error code, but Message `An export named <Export.Name> already exists; export names must be unique.` — different fixed text, and it interpolates the requested name, a value AWS interpolates nowhere in this message. In the replayed step the emulator sent `An export named sim2real-probe-dxdup-b1908305 already exists; export names must be unique.` where AWS sends `Cannot create duplicate export name.` (the `sim2real-probe-dxdup-...` token is the fixture's per-run generated name and is not itself the finding; the finding is that the message has any variable part at all and different fixed text).
Why: the error message is part of this error's contract. ValidationException covers every bad CreateExport request, so the message is the only thing distinguishing a name collision from a bad query, a bad destination or a bad table property. A client (or a log matcher) that tests for AWS's fixed duplicate-name sentence to retry under a new name never matches, and falls through to generic validation handling.
Fix: the duplicate-name branch of the CreateExport handler — the `store.export_named(&export.name).is_some()` branch, which must emit the fixed sentence rather than a formatted one.
Traces: create-export-duplicate-name-rejected-until-deleted#task#5.

Cause 2 of 2: the server-assigned ExportArn joins the export name and the generated export id with a path separator instead of a hyphen.
AWS: CreateExport returns ExportArn (Smithy: CreateExportResponse.ExportArn -> Arn, @length(20,2048), @pattern `^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`). The pattern admits both forms, so the resource-id shape is recorded behavior only: the recording shows the resource part is `export/` followed by the submitted export name, then a HYPHEN, then the server-generated export id (a UUID) — one path segment after `export/`, i.e. `.../export/<Export.Name>-<export-id>`. Variable parts: the account id, the Region, the submitted name and the generated id; the fixed parts are `export/` and the single `-` that joins name to id.
Emulator: mints `.../export/<Export.Name>/<export-id>` — a SLASH between the name and the id, making the resource part two segments. In the replayed step the emulator returned `arn:aws:bcm-data-exports:us-east-1:000000000000:export/sim2real-probe-dxdup-b1908305/93cdffc1-...` where the recorded AWS form is `arn:aws:bcm-data-exports:us-east-1:000000000000:export/sim2real-probe-dxdup-b1908305-<uuid>` (the UUID itself differs per run and is not the finding; the separator and the segment count are).
Why: clients and IaC tooling parse this ARN to recover the export name (last segment after `export/`, up to the final hyphen group) and build ARN patterns for IAM resource statements and tag/listing filters. With an extra `/` segment, name extraction yields the bare name or the bare UUID depending on the parser, and an IAM/condition pattern written against AWS's single-segment form does not match the emulator's ARN. The same malformed ARN is then echoed by GetExport/ListExports and used as the ExportArn argument to UpdateExport/DeleteExport, so the divergence propagates to every operation keyed by it.
Fix: the export ARN minter in this crate's state layer — `crate::state::export_arn(name, id)`, called from the CreateExport handler before validation; it must join the name and the generated id with `-` into one resource segment. No handler change is needed beyond that one builder.
Traces: create-export-duplicate-name-rejected-until-deleted#task#8.

Relation to already-open issues: the open behavioral issue `20261127T114409Z-bcm-data-exports-createexport-updateexport-aggregate-query-accepted.md` describes the same duplicate-name wording as its cause 4, but it is filed from the behavioral corpus and carries no wire replay key (its Issue-Trace-Ids are check ids), so this wire fixture step is not covered by it; the replay reports no owner for either cause here. Cause 2 (ExportArn shape) is not mentioned in any open issue.

## Evidence
Replayed fixture: create-export-duplicate-name-rejected-until-deleted (trace set simparity-wire-aws-v1).

Cause 1 — step task#5, verdict mismatch, expected status 400, actual status 400.
  $.Message: expected 'Cannot create duplicate export name.'
                actual 'An export named sim2real-probe-dxdup-b1908305 already exists; export names must be unique.'
  Status and error code match; only the message diverges. Fixed text expected: the whole sentence 'Cannot create duplicate export name.' with NO interpolated values. Emulator source of the diverging text: the `store.export_named(...)` duplicate branch in services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/create_export.rs:289-301, which formats the requested name into the message via `crate::state::field_validation_failed`.

Cause 2 — step task#8 (re-create after DeleteExport frees the name), verdict mismatch, expected status 200, actual status 200.
  $.ExportArn: expected 'arn:aws:bcm-data-exports:us-east-1:000000000000:export/sim2real-probe-dxdup-b1908305-9260f6d4-d045-4a49-a30f-93f17566ec88'
                  actual 'arn:aws:bcm-data-exports:us-east-1:000000000000:export/sim2real-probe-dxdup-b1908305/93cdffc1-af10-4e86-854f-5b7f1bf0d72c'
  The name and the UUID are per-run values; the structural difference is `<name>-<uuid>` (AWS, one resource segment) vs `<name>/<uuid>` (emulator, two segments). Emulator source: `crate::state::export_arn(&input.export.name, &crate::state::new_export_id())` at src/ops/create_export.rs:197.

Corpus summary for this replay: 83 steps, 80 passed, 3 mismatched, score 96.4%; the third mismatch (GetTable column descriptions) is already owned by an open issue and is not part of this filing.

## Steps to Reproduce
Against the emulator with a delivery bucket whose policy grants bcm-data-exports.amazonaws.com s3:PutObject:
1. CreateExport with a valid Export definition over COST_AND_USAGE_REPORT under some Export.Name N -> HTTP 200.
2. CreateExport again with the SAME Export.Name N and an otherwise valid definition -> read Error.Code and Error.Message. Observed: ValidationException / 'An export named N already exists; export names must be unique.'; expected: ValidationException / the fixed sentence 'Cannot create duplicate export name.' (cause 1).
3. DeleteExport the export created in step 1, then CreateExport again under the same name N -> read the returned ExportArn. Observed resource part: 'export/N/<uuid>'; expected: 'export/N-<uuid>' (cause 2). GetExport/ListExports echo the same ARN.
Replay to confirm: fixture create-export-duplicate-name-rejected-until-deleted, steps task#5 and task#8.
