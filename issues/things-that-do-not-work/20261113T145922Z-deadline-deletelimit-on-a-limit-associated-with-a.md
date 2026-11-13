Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.limit-delete-blocked-by-association-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.limit-delete-blocked-by-association-rejected|run#10|AssertionError|assert error.get("Code") == "ConflictException", f"expected ConflictException, got {error}"
Issue-Commit: 5204f1e06ae8734c38957c0a72a3ce6f6769fb9f

# deadline DeleteLimit on a limit associated with a queue returns ValidationException (400) with a locally-invented message instead of ConflictException (409)

> _Note: this issue was identified against the code base at commit `5204f1e06ae8734c38957c0a72a3ce6f6769fb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLimit

## Description
Cause 1 of 1: DeleteLimit refuses a delete that is blocked by an existing queue-limit association with the wrong error class, HTTP status, error code and error message.

AWS: Deadline refuses DeleteLimit while the limit is still associated with a queue (a QueueLimitAssociation exists for it). The refusal class comes from recorded behavior only — Smithy (services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json) lists on DeleteLimit (DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, idempotent) only AccessDeniedException 403, InternalServerErrorException 500, ThrottlingException 429 and ValidationException 400, so the model itself does not enumerate the conflict. Recorded/asserted AWS behavior: HTTP 409, Error.Code = "ConflictException", Error.Message exactly "Request could not be completed: Limit is associated with one or more Queues." That message is ENTIRELY FIXED TEXT for this error class: it interpolates no request or state value — no limitId, no queueId, no farmId. Smithy: com.amazonaws.deadline#ConflictException is @error(client) with @httpError(409) and required members message, reason (ConflictExceptionReason: CONFLICT_EXCEPTION | CONCURRENT_MODIFICATION | RESOURCE_ALREADY_EXISTS | RESOURCE_IN_USE | STATUS_CONFLICT), resourceId and resourceType, plus optional context; a conformant refusal therefore also carries reason=RESOURCE_IN_USE together with the blocked limit's resourceId/resourceType, not just a message. The rest of the contract the same check pins, and which must keep holding: the limit SURVIVES the rejected delete (GetLimit still returns it), and the identical DeleteLimit succeeds once the association is stopped (UpdateQueueLimitAssociation status=STOP_LIMIT_USAGE_AND_CANCEL_TASKS, polled to STOPPED) and deleted, after which GetLimit fails with ResourceNotFoundException "Resource of type limit with id <limitId> does not exist." (fixed text with the requested limitId interpolated).

Emulator: the call IS refused, but as HTTP 400 with Error.Code = "ValidationException" and the message "The limit <limitId> can't be deleted because it is associated with the queue <queueId>." — a sentence AWS never emits for this class, whose two ids are interpolated from the request and the emulator's state (in the replay they were limit-a8308df3d74feca9f6bf4b98bd758235 and queue-91095eee367083f0e37d4a65ebbf7957, per-run values that are not part of any template). Three things diverge at once: status 400 vs 409, code ValidationException vs ConflictException, and the invented message in place of AWS's fixed sentence; the ConflictException-only members (reason, resourceId, resourceType) are absent entirely.

Why: a client that distinguishes a dependency conflict from a malformed request gets it backwards. Callers catch deadline.exceptions.ConflictException (or branch on Error.Code == "ConflictException", or on HTTP 409) to mean "something still holds this resource — detach it and retry", and treat 400 ValidationException as "the request itself is invalid", i.e. non-retryable. Against this emulator such code aborts the teardown, surfaces a bogus validation error, or classifies a transient dependency ordering problem as permanent. Dependency-ordered cleanup and reconciliation loops are hit hardest: the suite's own teardown helper retries on ConflictException and re-raises everything else, which is exactly how real-world cleanup is written. Clients that read reason/resourceId/resourceType to discover what is holding the limit get nothing to parse.

Fix: the refusal path of the DeleteLimit handler (services/service-com-amazonaws-deadline-deadline/src/ops/delete_limit.rs and the shared error construction it calls) must raise the service's ConflictException — HTTP 409, code ConflictException, the fixed message above, reason RESOURCE_IN_USE and the limit's resourceId/resourceType — instead of the ValidationException it builds today. The dependency check itself (finding queue-limit associations for the limit) is correct and must stay, as must leaving the limit intact on refusal and allowing the delete once the association is removed. NOTE: the open issue 20261112T162948Z-deadline-deletestorageprofile-on-a-profile-referenced-by-a.md reports the same misclassification on DeleteStorageProfile and does NOT cover DeleteLimit; if a shared conflict-error helper is introduced, both operations' refusals should go through it, but this operation needs its own fix and its own fixed message text.

Traces: failing check deadline.limit-delete-blocked-by-association-rejected (verdict: mismatch).

## Evidence
Check: deadline.limit-delete-blocked-by-association-rejected (verdict: mismatch)

Assertion that failed (check_deadline_limit_delete_blocked_by_association_rejected.py line 40):
  assert error.get("Code") == "ConflictException", f"expected ConflictException, got {error}"
AssertionError: expected ConflictException, got {'Message': "The limit limit-a8308df3d74feca9f6bf4b98bd758235 can't be deleted because it is associated with the queue queue-91095eee367083f0e37d4a65ebbf7957.", 'Code': 'ValidationException'}

Expected (AWS, as recorded and as the check asserts):
  HTTP 409
  Error.Code    = "ConflictException"
  Error.Message = "Request could not be completed: Limit is associated with one or more Queues."  (entirely fixed text; no interpolated ids)
  plus ConflictException's required reason / resourceId / resourceType members (reason = RESOURCE_IN_USE, resourceId = the limit id, resourceType = the limit resource type)

Actual (emulator):
  HTTP 400
  Error.Code    = "ValidationException"
  Error.Message = "The limit <limitId> can't be deleted because it is associated with the queue <queueId>."  (both ids interpolated from request/state; the recorded values limit-a8308df3d74feca9f6bf4b98bd758235 / queue-91095eee367083f0e37d4a65ebbf7957 are per-run, not part of the template)
  no reason / resourceId / resourceType members

Model evidence (smithy_query on services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json):
  DeleteLimit: DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, idempotent, hostPrefix "management.", errors = [AccessDeniedException 403, InternalServerErrorException 500, ThrottlingException 429, ValidationException 400]
  ConflictException: @error(client) @httpError(409); required message, reason (ConflictExceptionReason), resourceId, resourceType; optional context
  ConflictExceptionReason values: CONFLICT_EXCEPTION, CONCURRENT_MODIFICATION, RESOURCE_ALREADY_EXISTS, RESOURCE_IN_USE, STATUS_CONFLICT

Surrounding steps of the check that are part of the same contract: GetLimit after the refused delete still returns the limit (passes today and must not regress); after the association is stopped and deleted, DeleteLimit succeeds and GetLimit fails with ResourceNotFoundException "Resource of type limit with id <limitId> does not exist."

## Steps to Reproduce
1. deadline.CreateFarm(displayName=<unique>) -> farmId
2. deadline.CreateQueue(farmId, displayName=<unique>) -> queueId
3. deadline.CreateLimit(farmId, displayName=<unique>, amountRequirementName="amount.Sim2realLimitTest", maxCount=3) -> limitId
4. deadline.CreateQueueLimitAssociation(farmId, queueId, limitId)
5. deadline.DeleteLimit(farmId, limitId)
   -> AWS: HTTP 409, Error.Code=ConflictException, Error.Message="Request could not be completed: Limit is associated with one or more Queues." (plus reason=RESOURCE_IN_USE, resourceId, resourceType)
   -> Emulator: HTTP 400, Error.Code=ValidationException, Error.Message="The limit <limitId> can't be deleted because it is associated with the queue <queueId>."
6. deadline.GetLimit(farmId, limitId) still returns the limit (matches AWS; must not regress).
7. deadline.UpdateQueueLimitAssociation(farmId, queueId, limitId, status="STOP_LIMIT_USAGE_AND_CANCEL_TASKS"), poll GetQueueLimitAssociation until status=="STOPPED", then DeleteQueueLimitAssociation and DeleteLimit: the delete must now succeed and GetLimit must then fail with ResourceNotFoundException "Resource of type limit with id <limitId> does not exist."

Replay with: run_behavioral_suite, check deadline.limit-delete-blocked-by-association-rejected
