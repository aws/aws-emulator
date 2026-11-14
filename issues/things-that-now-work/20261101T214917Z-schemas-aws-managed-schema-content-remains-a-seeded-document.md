Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-export-aws-managed-schema
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-export-aws-managed-schema
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.Content
Issue-Key: schemas|ExportSchema|2xx->2xx|$.Content
Issue-Commit: 7724bb70857031c7f73076fed813e724b0c7ffb5

# AWS-managed schema Content remains a seeded document that is not the published one (DescribeSchema, ExportSchema)

> _Note: this issue was identified against the code base at commit `7724bb70857031c7f73076fed813e724b0c7ffb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchema

## Description
Reading a service-owned (AWS-managed) catalogue schema still answers 200 with a `Content` document that is not the document AWS publishes, on BOTH the describe path and the export path. 2 causes, one per fixture step/operation: the export path converts the document the describe path serves, so they share the seeded source document but are two separate replay units that must each be re-run.

NOTE on prior issues: this exact pair of steps has been reported and RESOLVED twice — `20261029T203553Z-schemas-aws-managed-schema-content-is-wrong-for-both.md` and `20261030T170146Z-schemas-aws-managed-schema-content-document-is-still-not.md` (and earlier `20261023T194150Z-schemas-describeschema-and-exportschema-return-the-wrong-content.md`) — yet `schemas-export-aws-managed-schema#task#2` and `#task#3` still diverge in this replay; both clusters carry an empty `owned_by` and an empty `stub_path`, so they are net-new. The specific defect those issues named (a propertyless `userIdentity.sessionContext`) IS now fixed in this checkout — `services/service-com-amazonaws-schemas-schemas/src/state.rs` lines 2381-2412 declare `sessionContext` inline with its own `properties` — so what remains is a DIFFERENT part of the same seeded document. This is also NOT the open `20261030T050932Z-schemas-listschemas-serves-a-fabricated-aws-managed-catalogue-digest.md` (which owns the 55 ListSchemas clusters of this run: which entries are listed and what `LastModified` they report, not the document a schema read returns), and not the open `20261030T004517Z-schemas-5-net-new-causes-deleteschemaversion-deletes-a.md`.

Cause 1 of 2: DescribeSchema of an AWS-managed CloudTrail-delivered catalogue entry serves an OpenApi3 document that differs from the published one over the whole `Content` value.
AWS: Smithy: `DescribeSchema` GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName} -> 200, output member `Content` targets `__string` (errors BadRequestException 400, NotFoundException 404, ForbiddenException 403, UnauthorizedException 401, InternalServerErrorException 500, ServiceUnavailableException 503). WHAT the document says is recorded behavior only: the model states nothing about a document's text. The recording for the catalogue entry `aws.events@AWSAPICallviaCloudTrail` is an OpenAPI 3.0.0 document of the form `{"openapi":"3.0.0","info":{"title":"<detail type> published by <source>","version":"1.0.0"},"paths":{},"components":{"schemas":{"<EventName>":{"type":"object","properties":{...}}}}}` whose single component declares the full CloudTrail record with every nested structure declared INLINE and carrying its own `properties`, the four free-form members (`addendum`, `additionalEventData`, `requestParameters`, `responseElements`) as `{"type":"object","nullable":true}`, and `eventTime` as `{"pattern":"yyyy-MM-dd'T'HH:mm:ss'Z'","type":"string","format":"date-time"}`. Member ORDER is part of the document, because `Content` is text a client caches, diffs and hashes. The requirement is the published document for the entry, not a plausible reconstruction of it.
Emulator: it serves the document rendered from its own seeded template (`CLOUD_TRAIL_COMPONENTS`, one template fanned out over the whole CloudTrail event family and every event source), and replay reports `$.Content: value_mismatch` for the very first catalogue read of the fixture — status 200 on both sides, the WHOLE string compared. The portion of the expected document the replay detail shows (the `openapi`/`info`/`paths`/`components` envelope, the record's members from `addendum` through `userAgent`, and `userIdentity` down to `onBehalfOf.{identityStoreArn,userId}`) agrees member-for-member and order-for-order with the seeded template, so the residual difference is in the part the detail clips: the `userIdentity.sessionContext` subtree (`assumedRoot`, `attributes`, `ec2RoleDelivery`, `sessionIssuer`, `sourceIdentity`, `webIdFederationData`) and whatever follows it. The seeded member set/spelling/order there is the emulator's own reconstruction from CloudTrail documentation, not the published text.
Why: `Content` is the entire point of the operation — it is the document a client validates delivered events against, caches, diffs, hashes, or generates bindings from. Any member of the record the seeded document declares differently (missing, extra, differently typed, or in another order) makes generated bindings and validators disagree with the ones built from AWS's document, and makes any digest or cached copy of the schema text differ from the published one.
Fix: the AWS-managed catalogue seed in the schemas service's state module — the CloudTrail components template the catalogue publishes every CloudTrail-delivered event under, reached by the DescribeSchema handler through the seeded schema row. This is catalogue DATA; nothing in the handler or the serializer needs to change. The fixer must diff the SERVED document against the recorded one byte for byte rather than spot-checking one subtree, since two previous fixes each corrected one visible subtree and left the step diverging. File/line citations under Traces.
Traces: `schemas-export-aws-managed-schema#task#2` (representative) — divergence_key `service=schemas|DescribeSchema|status=200->200|keys=$.Content`, expected 200 / actual 200, affected_count 1.

Cause 2 of 2: ExportSchema serves the JSONSchemaDraft4 translation of that same seeded document, so the exported `definitions` entry differs from the published export in the same place.
AWS: Smithy: `ExportSchema` GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/export with required `@httpQuery("type")` member `Type` -> 200, output `Content` targets `__string`; the document is recorded behavior only. The recording is the draft-04 rendering of the describe document: `{"$schema":"http://json-schema.org/draft-04/schema#","title":"<detail type> published by <source>","definitions":{"<EventName>":{"properties":{...},"type":"object"}}}` — `components.schemas` becomes `definitions`, the OpenAPI `nullable` form becomes the draft-04 union `{"type":["object","null"]}`, the OpenAPI-only keyword `readOnly` does not travel, members are rendered in name order (`properties` before `type`, `format` before `pattern` on `eventTime`), and `userIdentity.sessionContext` appears as `{"properties":{"assumedRoot":{"type":"string"},...}}`.
Emulator: replay reports `$.Content: value_mismatch` on the export step too, status 200 on both sides. The envelope promotion, nullable-union lowering, `readOnly` dropping and name ordering the recorded export shows are already implemented; what is wrong is the document being converted — the same seeded source document as cause 1.
Why: a client exporting to draft-04 to drive a JSON-schema validator gets a document that constrains the CloudTrail record differently from AWS's export, so validation verdicts for real events disagree with AWS, and anything keyed on the exported text (a cached copy, a digest) differs from the published export.
Fix: the same catalogue seed as cause 1; the ExportSchema handler's OpenApi3 -> JSONSchemaDraft4 conversion is only the renderer. This step must be re-replayed separately after the seed is corrected even though no conversion code changes — fixing only the describe step leaves this one diverging.
Traces: `schemas-export-aws-managed-schema#task#3` — divergence_key `service=schemas|ExportSchema|status=200->200|keys=$.Content`, expected 200 / actual 200, affected_count 1.

## Evidence
Replay of trace set `simparity-wire-aws-v1` against the emulator built from this checkout. Run summary: emulator_up true (`/__admin/health` 200, data listener accepting), bootstrap_signal false, fixtures_replayed 13, steps_total 289, steps_passed 74, steps_mismatched 215, steps_blocked 0, steps_errored 0, score_pct 25.6, divergence_count 57. 55 of those 57 clusters are owned by the open issue `20261030T050932Z-schemas-listschemas-serves-a-fabricated-aws-managed-catalogue-digest.md`; the two clusters below are the ONLY ones in the whole corpus with an empty `owned_by` and an empty `stub_path`.

Step task#2 — DescribeSchema, verdict mismatch, expected_status 200 / actual_status 200, divergence_key `service=schemas|DescribeSchema|status=200->200|keys=$.Content`, affected `["schemas-export-aws-managed-schema#task#2"]`, affected_count 1. Detail: `$.Content: value_mismatch`. The expected document, as far as the replay detail shows it: `{"openapi":"3.0.0","info":{"title":"AWS API Call via CloudTrail published by aws.events","version":"1.0.0"},"paths":{},"components":{"schemas":{"AWSAPICallviaCloudTrail":{"type":"object","properties":{"addendum":{"type":"object","nullable":true},"additionalEventData":{"type":"object","nullable":true},"annotation":{"type":"string"},...,"eventTime":{"pattern":"yyyy-MM-dd'T'HH:mm:ss'Z'","type":"string","format":"date-time"},...,"readOnly":{"type":"boolean"},...,"tlsDetails":{"type":"object","properties":{"cipherSuite":{"type":"string"},...}},"userAgent":{"type":"string"},"userIdentity":{"type":"object","properties":{"accessKeyId":{"type":"string"},...,"inScopeOf":{"type":"object"},"invokedBy":{"type":"string"},"invokedByDelegate":{"type":"object"},"onBehalfOf":{"type":"object","properties":{"identityStoreArn":{"type":"string"},"userId":{"type":"` — the detail is clipped at that point and the comparison is of the whole string, so the residual difference lies at or after `userIdentity.sessionContext`.

Step task#3 — ExportSchema, verdict mismatch, expected_status 200 / actual_status 200, divergence_key `service=schemas|ExportSchema|status=200->200|keys=$.Content`, affected `["schemas-export-aws-managed-schema#task#3"]`, affected_count 1. Detail: `$.Content: value_mismatch`. The expected export begins `{"$schema":"http://json-schema.org/draft-04/schema#","title":"AWS API Call via CloudTrail published by aws.events","definitions":{"AWSAPICallviaCloudTrail":{"properties":{"addendum":{"type":["object","null"]},"additionalEventData":{"type":["object","null"]},...,"eventTime":{"format":"date-time","pattern":"yyyy-MM-dd'T'HH:mm:ss'Z'","type":"string"},...,"requestParameters":{"type":["object","null"]},"responseElements":{"type":["object","null"]},...,"userIdentity":{"properties":{...,"principalId":{"type":"string"},"sessionContext":{"properties":{"assumedRoot":{"type":"string` — likewise clipped, and note the expected export carries no `readOnly` member and uses the draft-04 nullable union, both of which the conversion already does.

Where the served document comes from, in this checkout:
* `services/service-com-amazonaws-schemas-schemas/src/state.rs` line 2311 `const CLOUD_TRAIL_COMPONENTS` (through line 2417) — the one components template every CloudTrail-delivered catalogue entry of every source in `AWS_EVENT_SOURCES` (line 2430) is published under. Line 2362 `userIdentity`, line 2381 `sessionContext` (now declared inline WITH `properties`, lines 2383-2412: `assumedRoot`, `attributes.{creationDate,mfaAuthenticated}`, `ec2RoleDelivery`, `sessionIssuer.{accountId,arn,principalId,type,userName}`, `sourceIdentity`, `webIdFederationData.{attributes,federatedProvider}`) — this subtree is the region the replay detail clips and the only region whose agreement with the recording the replay output does not show.
* `services/service-com-amazonaws-schemas-schemas/src/state.rs` line 2974 `fn event_schema_document` (envelope `openapi`/`info.title`/`info.version`/`paths`/`components.schemas`, rendered through `JsonNode` so member order is deliberate), line 3025 `managed_schema_title` (`<detail type> published by <source>`), line 3036 `aws_managed_event_types` and line 3057 `aws_managed_schemas` — how the template becomes the stored `Content` DescribeSchema serves.
* `services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs` line 319 `fn to_json_schema_draft4`, line 390 `fn in_name_order`, line 405 `fn to_draft4` — the conversion ExportSchema serves; its recorded behaviours (definitions, nullable union, dropping the OpenAPI-only `readOnly`, case-insensitive name ordering) are already in place, so its export is wrong only because its source document is.

No per-run value is involved: the seeded document reads no clock and carries no id, request id or generated name, so both steps diverge identically on every boot. The two earlier reports of these same two steps are in the RESOLVED folder, so neither covers this failure.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it (data listener accepting, `/__admin/health` 200).
2. Replay fixture `schemas-export-aws-managed-schema` from trace set `simparity-wire-aws-v1`, or issue the two recorded calls directly.
3. Cause 1 (step `task#2`): `GET /v1/registries/name/aws.events/schemas/name/aws.events@AWSAPICallviaCloudTrail`. Observe 200, then compare the returned `Content` with the recorded document byte for byte. It differs — the replay reports the whole `$.Content` as a value_mismatch. The envelope and the record members from `addendum` through `userIdentity.onBehalfOf` match the recording, so start the diff at `components.schemas.AWSAPICallviaCloudTrail.properties.userIdentity.properties.sessionContext` and run it to the end of the document; do not stop at the first subtree that looks right.
4. Cause 2 (step `task#3`): `GET /v1/registries/name/aws.events/schemas/name/aws.events@AWSAPICallviaCloudTrail/export?type=JSONSchemaDraft4`. Observe 200 whose `Content` is the draft-04 translation of the document from step 3, and compare it with the recorded export byte for byte — it diverges in the same place, under `definitions.AWSAPICallviaCloudTrail.properties.userIdentity.properties.sessionContext`.
5. After correcting the seeded CloudTrail components template, re-replay BOTH steps: `task#3` converts the document `task#2` serves, so the seed correction has to be confirmed on the describe step and on the export step separately — the two previous fixes for these steps each left one of them diverging.
6. Both reproduce on every boot; nothing here reads the clock or mints an id.

## Resolution

The rule both read paths owe: a published catalogue document declares its event's record
WHOLE — every member the record states for a structure it carries is declared on that
structure, so a client that generates a binding or feeds a validator from the document
knows every member of a delivered event. For the CloudTrail-delivered family that record
is the CloudTrail record, whose identity element states, beside the credential and session
facts, WHAT KIND of principal made the call and the friendly NAME of a principal that has
one of its own (CloudTrail User Guide, "CloudTrail userIdentity element"); the CloudTrail
records this emulator itself writes carry both (`services/common`'s record writer).

The seeded catalogue data — `CLOUD_TRAIL_COMPONENTS` in
`services/service-com-amazonaws-schemas-schemas/src/state.rs` — ended its `userIdentity`
member list at `sessionContext`, so `type` and `userName` were declared nowhere, on either
read path: `DescribeSchema` serves that one template for the whole family of every source
and `ExportSchema` converts the document it serves. Both members are now declared, in the
order the document writes its members. No handler and no serializer changed.
`tests/all/published_record_identity.rs` holds both read paths to the whole identity
element over a table (every member of the CloudTrail-delivered family × four publishing
sources), and `tests/all/published_document_bytes.rs` carries the identity's member set
beside the record's.

CAVEAT: the replay key for each cause compares the WHOLE recorded `Content` string, and
what the document SAYS is recorded behaviour only — the model states nothing about a
document's text. Every replay detail filed for these two steps (this report and the three
before it) clips the recorded document at or before
`userIdentity.properties.sessionContext`, and the replay tool now withholds recorded
values altogether, so the one region in which the residual difference can lie has never
been shown. Beyond declaring the record as its documentation states it — which is what
this change does — the remaining bytes are reachable only by fitting that one recorded
document, which is not a rule and not a fix. Both steps still report
`$.Content: value_mismatch` after this change.

