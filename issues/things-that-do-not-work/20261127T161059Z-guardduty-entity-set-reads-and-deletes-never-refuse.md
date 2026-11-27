Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-all-set-types-coexist-on-detector
Issue-Step: task#19
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-all-set-types-coexist-on-detector guardduty-threatentityset-lifecycle-with-metadata guardduty-trusted-entity-set-lifecycle guardduty-trustedentityset-lifecycle-with-metadata guardduty-error-paths-invalid-ids guardduty-threat-entity-set-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.__type
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.message
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$.type
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetThreatEntitySet|4xx->2xx|status
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.__type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.message
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$.type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetTrustedEntitySet|4xx->2xx|status
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetThreatEntitySet|4xx->4xx|status
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetTrustedEntitySet|4xx->4xx|status
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteThreatEntitySet|4xx->4xx|status
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->4xx|$.__type
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->4xx|$.type
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteTrustedEntitySet|4xx->4xx|status
Issue-Commit: 2453612a9812cc17d3b11d7e571f512669259467

# guardduty: entity-set reads and deletes never refuse with ResourceNotFoundException (6 causes)

> _Note: this issue was identified against the code base at commit `2453612a9812cc17d3b11d7e571f512669259467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThreatEntitySet

## Description
Six causes, one shared fault: no threat/trusted entity-set operation in the emulator can ever produce GuardDuty's not-found refusal. An id that does not resolve is either served as if it still existed (HTTP 200 with a full entity-set body) or refused as a malformed input (HTTP 400, error type InvalidInputException) — never with HTTP 404 and error type ResourceNotFoundException. All six causes must be closed before this issue is submitted.

Shared AWS reference for all six causes: Smithy (GetThreatEntitySet / GetTrustedEntitySet / DeleteThreatEntitySet / DeleteTrustedEntitySet) models only BadRequestException (httpError 400) and InternalServerErrorException (httpError 500), so the not-found refusal itself is recorded behavior only. The recordings show that GuardDuty answers a non-resolving entity-set id with HTTP status 404, header x-amzn-errortype: ResourceNotFoundException, body __type: ResourceNotFoundException, body type: ResourceNotFoundException (the BadRequestException shape's modeled `type` member, jsonName "type", documented as "The error type"), and body message: "The specified entity set does not exist." — that message is entirely fixed text, with no request or state values interpolated, and the error code and status are the same for every non-resolving id regardless of detector, set id or region. Shared Fix region: the entity-set lookup in the service state plus the four entity-set handlers and the error serializer that maps their failure to the wire; the id-resolution path must have a distinct not-found outcome that serializes as 404 / ResourceNotFoundException / that message, instead of falling through to "found" or to the generic bad-request/invalid-input mapping.

Cause 1 of 6 — GetThreatEntitySet keeps serving a deleted threat entity set. AWS: once the set's removal has completed, GetThreatEntitySet on that id is refused as described above (404, ResourceNotFoundException, "The specified entity set does not exist."). Emulator: answers HTTP 200 with a normal GetThreatEntitySetResponse body; none of message, __type, type or x-amzn-errortype is present at all. Why: a client that confirms a delete by polling the getter until it 404s — the standard confirm-delete loop, and what SDK waiters do — never sees the terminal state and either hangs or concludes the set still exists, so delete-then-recreate orchestration never proceeds. Fix: the DeleteThreatEntitySet handler and the entity-set lifecycle in the service state must drive the retained DELETE_PENDING/DELETED record (Smithy ThreatEntitySetStatus values include DELETE_PENDING and DELETED) to a state where the id stops resolving. Traces: guardduty-all-set-types-coexist-on-detector#task#19; also guardduty-threatentityset-lifecycle-with-metadata#task#6.

Cause 2 of 6 — GetTrustedEntitySet keeps serving a deleted trusted entity set: the same fault on the trusted-set twin. AWS: 404 / ResourceNotFoundException / "The specified entity set does not exist." after the delete completes. Emulator: HTTP 200 with a full GetTrustedEntitySetResponse body and no error fields. Why: identical — a deletion of a trusted entity set is unobservable to a client. Fix: DeleteTrustedEntitySet handler plus the trusted-set lifecycle in state. Traces: guardduty-all-set-types-coexist-on-detector#task#21; also guardduty-trusted-entity-set-lifecycle#task#6, guardduty-trustedentityset-lifecycle-with-metadata#task#6.

Cause 3 of 6 — GetThreatEntitySet misclassifies an unknown/ill-formed id as an input error. AWS: 404, x-amzn-errortype: ResourceNotFoundException, __type and type both ResourceNotFoundException. Emulator: HTTP 400 with __type and type both "InvalidInputException" and header x-amzn-errortype: "BadRequestException" (the message matched, so only the status and the three error-type carriers diverge; note also that the header and the body disagree with each other in the emulator's own response). Why: a 4xx that is not 404 and an error code that is not ResourceNotFoundException defeat the standard not-found branch — clients treat InvalidInputException as a non-retryable caller bug and surface a validation failure instead of "no such entity set". Fix: the id-resolution step of the GetThreatEntitySet handler must distinguish "does not resolve" from "malformed parameter", and the error serializer must emit one consistent error code in both the header and the body. Traces: guardduty-error-paths-invalid-ids#task#4.

Cause 4 of 6 — GetTrustedEntitySet misclassifies an unknown/ill-formed id the same way: expected 404 / ResourceNotFoundException (header, __type and type), got HTTP 400 with __type and type "InvalidInputException" and x-amzn-errortype "BadRequestException". Why and Fix: as cause 3, in the GetTrustedEntitySet handler. Traces: guardduty-error-paths-invalid-ids#task#5.

Cause 5 of 6 — DeleteThreatEntitySet misclassifies a non-resolving id as an input error. AWS: deleting an id that no longer resolves is refused with 404 / ResourceNotFoundException (header, __type and type) and the fixed message "The specified entity set does not exist.". Emulator: HTTP 400 with __type and type "InvalidInputException" and x-amzn-errortype "BadRequestException". Why: an idempotent delete-until-gone loop never terminates on the expected 404, and a client cannot tell "already gone" from "you sent a bad id". Fix: the DeleteThreatEntitySet handler's id-resolution branch plus the shared error serializer. Traces: guardduty-threat-entity-set-lifecycle#task#6; also guardduty-threatentityset-lifecycle-with-metadata#task#7.

Cause 6 of 6 — DeleteTrustedEntitySet misclassifies a non-resolving id the same way: expected 404 / ResourceNotFoundException (header, __type and type), got HTTP 400 with __type and type "InvalidInputException" and x-amzn-errortype "BadRequestException". Why and Fix: as cause 5, in the DeleteTrustedEntitySet handler. Traces: guardduty-trustedentityset-lifecycle-with-metadata#task#7.

Related open issue, which does NOT cover this one: 20261127T113846Z-guardduty-an-accepted-deletethreatentityset-never-completes-getthreatentityset-keeps.md describes the DeleteThreatEntitySet half of cause 1 from the behavioral corpus and carries no wire replay key, so it does not gate any of the six fixture steps listed here, and it says nothing about the trusted-set twin or about the InvalidInputException misclassification in causes 3-6.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (87 fixtures, 560 steps, 13 mismatched).

Cause 1 — guardduty-all-set-types-coexist-on-detector#task#19, GetThreatEntitySet: status expected 404, got 200; $.message missing (expected 'The specified entity set does not exist.'); $.__type missing (expected 'ResourceNotFoundException'); $.type missing (expected 'ResourceNotFoundException'); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException'). Cluster also covers guardduty-threatentityset-lifecycle-with-metadata#task#6.

Cause 2 — guardduty-all-set-types-coexist-on-detector#task#21, GetTrustedEntitySet: status expected 404, got 200; $.message, $.__type, $.type and $headers.x-amzn-errortype all missing (expected 'The specified entity set does not exist.' / 'ResourceNotFoundException' x3). Cluster also covers guardduty-trusted-entity-set-lifecycle#task#6 and guardduty-trustedentityset-lifecycle-with-metadata#task#6.

Cause 3 — guardduty-error-paths-invalid-ids#task#4, GetThreatEntitySet: status expected 404, got 400; $.__type expected 'ResourceNotFoundException', got 'InvalidInputException'; $.type expected 'ResourceNotFoundException', got 'InvalidInputException'; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'BadRequestException'.

Cause 4 — guardduty-error-paths-invalid-ids#task#5, GetTrustedEntitySet: identical divergence set (404->400; 'ResourceNotFoundException' -> 'InvalidInputException' in body, -> 'BadRequestException' in header).

Cause 5 — guardduty-threat-entity-set-lifecycle#task#6, DeleteThreatEntitySet: status expected 404, got 400; $.__type and $.type expected 'ResourceNotFoundException', got 'InvalidInputException'; $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'BadRequestException'. Cluster also covers guardduty-threatentityset-lifecycle-with-metadata#task#7.

Cause 6 — guardduty-trustedentityset-lifecycle-with-metadata#task#7, DeleteTrustedEntitySet: identical divergence set to cause 5.

Smithy support: GetThreatEntitySet / GetTrustedEntitySet / DeleteThreatEntitySet / DeleteTrustedEntitySet each model errors [BadRequestException (400), InternalServerErrorException (500)]; BadRequestException members are message (jsonName 'message') and type (jsonName 'type', doc 'The error type') — the member the emulator fills with 'InvalidInputException' where the recording requires 'ResourceNotFoundException'. ThreatEntitySetStatus enum includes DELETE_PENDING and DELETED.

## Steps to Reproduce
Causes 1 and 2 (deleted set keeps answering 200):
1. CreateDetector (or reuse the account's detector).
2. CreateThreatEntitySet(DetectorId, Name=<unique>, Format='TXT', Location='s3://<bucket>/list.txt', Activate=true) and, separately, CreateTrustedEntitySet with the same shape.
3. DeleteThreatEntitySet(DetectorId, ThreatEntitySetId) -> 200; DeleteTrustedEntitySet(DetectorId, TrustedEntitySetId) -> 200.
4. GetThreatEntitySet(DetectorId, ThreatEntitySetId) and GetTrustedEntitySet(DetectorId, TrustedEntitySetId) -> emulator answers 200 with a full body; AWS refuses with 404 / ResourceNotFoundException / 'The specified entity set does not exist.'.

Causes 3 and 4 (unknown id on the getters):
1. CreateDetector.
2. GetThreatEntitySet(DetectorId, ThreatEntitySetId=<id that was never created>) -> emulator answers 400 with __type/type 'InvalidInputException' and x-amzn-errortype 'BadRequestException'; AWS answers 404 with ResourceNotFoundException in the header, __type and type.
3. Repeat with GetTrustedEntitySet(DetectorId, TrustedEntitySetId=<never created>).

Causes 5 and 6 (non-resolving id on the deleters):
1. CreateDetector and create a threat entity set and a trusted entity set.
2. DeleteThreatEntitySet / DeleteTrustedEntitySet once -> 200.
3. Call the same delete again (or with an id that was never created) -> emulator answers 400 with __type/type 'InvalidInputException' and x-amzn-errortype 'BadRequestException'; AWS answers 404 with ResourceNotFoundException and the message 'The specified entity set does not exist.'.
