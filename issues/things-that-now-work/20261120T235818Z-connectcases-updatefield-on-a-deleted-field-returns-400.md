Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcases.concurrent-create-field-same-name-single-winner
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|UpdateField|ValidationException
Issue-Commit: 4cec1d0eb9aabebbf70530b8e376ceefd6f10029

# connectcases UpdateField on a deleted field returns 400 ValidationException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `4cec1d0eb9aabebbf70530b8e376ceefd6f10029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateField

## Description
ONE cause.

Cause 1 of 1: UpdateField addressed at a field that has already been deleted by DeleteField is refused with the wrong modelled error (failing check: connectcases.concurrent-create-field-same-name-single-winner).

AWS: Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json): UpdateField = PUT /domains/{domainId}/fields/{fieldId}, success 200 with an empty body; its modelled errors include both ResourceNotFoundException (HTTP 404, members message/resourceId/resourceType all required) and ValidationException (HTTP 400, message required). The model's DeleteField prose does say "You cannot update a deleted field by calling UpdateField; it throws a ValidationException", but the recorded behaviour of the live service contradicts that prose and is the requirement here: after DeleteField(domainId, fieldId) succeeds and the field is gone from ListFields, UpdateField(domainId, fieldId, description=<anything>) is refused with HTTP 404, error code ResourceNotFoundException and the message `Requested resource not found.` — entirely fixed text with no variable part; the field that could not be resolved is reported through the error's required resourceId (the fieldId the request carried) and resourceType (this service's lowercase token for the kind, `field`) members, not inside the sentence. Recorded behaviour only for the status/code/message pairing; the member population is what the model makes required.

Emulator: the deleted field's record is kept as a tombstone and UpdateField takes an explicit tombstone branch before anything else, answering HTTP 400 with code ValidationException (the enumerated deleted-fields sentence) instead of the 404. Replay shows `AssertionError: expected ResourceNotFoundException updating deleted field, got ValidationException`. Note the correct sentence already exists in the service state module as RESOURCE_NOT_FOUND_MESSAGE = "Requested resource not found." and is produced by the generic not_found() helper, so only the branch choice diverges, not the wording. Related but NOT covering this: open issue 20261102T010613Z-connectcases-updatefield-stub.md (UpdateField scaffold stub — the operation is in fact implemented now), and fixed issue 20261109T181134Z-connectcases-field-state-system-field-seed-and-updatefield.md (UpdateField mutation durability, a different cause; the call here is not left unrefused by it, only misclassified).

Why: a client that deletes a field and then retries or reconciles an UpdateField against it branches on the error to decide whether the resource is gone (stop / recreate) or the request was malformed (fix the payload and retry). Against the emulator a gone field reads as a 400 client-input error, so idempotent cleanup and reconciliation loops treat a deleted field as a bad request, retry forever or surface a bogus validation failure; and code catching ResourceNotFoundException (botocore `cc.exceptions.ResourceNotFoundException`) never catches it at all, so the exception escapes.

Fix: the UpdateField handler's refusal ordering/classification in services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/update_field.rs — a field whose stored record is a tombstone must raise the modelled ResourceNotFoundException built by crate::state::not_found(ResourceKind::Field, &input.field_id) (404, fixed generic sentence, resourceId = requested fieldId, resourceType = "field"), exactly as the no-record-at-all branch immediately above it already does, rather than the ValidationException deleted-fields branch. The same reading should not be propagated to BatchPutFieldOptions/CreateCase/UpdateCase, whose deleted-field ValidationException refusals are separately documented and are not exercised by this check.

Traces: connectcases.concurrent-create-field-same-name-single-winner (the check's earlier phases — 8 racing CreateField calls with one winner and seven ConflictException losers, ListFields showing exactly one field, DeleteField, and the post-delete ListFields — all pass; it fails only on the final post-delete UpdateField refusal).

## Evidence
Failing check: connectcases.concurrent-create-field-same-name-single-winner (verdict: mismatch).

Replay output:
  File "check_connectcases_concurrent_create_field_same_name_single_winner.py", line 68, in run
    assert code == "ResourceNotFoundException", (
  AssertionError: expected ResourceNotFoundException updating deleted field, got ValidationException

Expected (recorded AWS): UpdateField(domainId=<domain>, fieldId=<deleted field>, description="post-delete-update") -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "Requested resource not found." (fixed text, no variable part), with resourceId = the fieldId sent in the request and resourceType = "field" (values from the request/state, not fixed text).
Actual (emulator): HTTP 400, Error.Code = "ValidationException" from the tombstone branch of update_field.rs (the check asserts on the code first, so the 400 body's sentence is not reported, but it is the enumerated deleted-field template).

Smithy: UpdateField errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; ResourceNotFoundException = {message required, resourceId required, resourceType required}.
Emulator source: services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/update_field.rs lines 174-184 (tombstone -> ValidationException via crate::state::deleted_fields) vs lines 167-172 (missing record -> ResourceNotFoundException via crate::state::not_found); services/service-com-amazonaws-connectcases-amazonconnectcases/src/state.rs line 2037 (RESOURCE_NOT_FOUND_MESSAGE = "Requested resource not found.").

## Steps to Reproduce
1. CreateDomain(name=<unique>) and poll GetDomain until domainStatus == "Active".
2. CreateField(domainId=<domain>, name=<unique>, type="Text") -> fieldId.
3. DeleteField(domainId=<domain>, fieldId=<fieldId>) -> 200.
4. ListFields(domainId=<domain>) -> the fieldId is absent (this already matches AWS).
5. UpdateField(domainId=<domain>, fieldId=<fieldId>, description="post-delete-update").
   Expect: HTTP 404, ResourceNotFoundException, message "Requested resource not found.", resourceId = <fieldId>, resourceType = "field".
   Observed: HTTP 400, ValidationException.
Or replay the behavioral check connectcases.concurrent-create-field-same-name-single-winner, which performs exactly this sequence after its concurrency phase.
