Issue-Source: trace-sweep-divergence
Issue-Trace: deadline-queue-limit-association-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: deadline-queue-limit-association-lifecycle
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|GetQueueLimitAssociation|2xx->2xx|$
Issue-Key: deadline|GetQueueLimitAssociation|2xx->2xx|$.status
Issue-Key: deadline|GetQueueLimitAssociation|2xx->2xx|$.updatedAt
Issue-Key: deadline|GetQueueLimitAssociation|2xx->2xx|$.updatedBy
Issue-Key: deadline|GetQueueLimitAssociation|4xx->2xx|$.message
Issue-Key: deadline|GetQueueLimitAssociation|4xx->2xx|$.resourceId
Issue-Key: deadline|GetQueueLimitAssociation|4xx->2xx|$.resourceType
Issue-Key: deadline|GetQueueLimitAssociation|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: deadline|GetQueueLimitAssociation|4xx->2xx|status
Issue-Key: deadline|DeleteLimit|2xx->4xx|status
Issue-Commit: 73a889768ef107d9a74aea509f6592e40712e251

# deadline queue-limit-association lifecycle never progresses past ACTIVE: status/updatedAt/updatedBy frozen, no 404 after delete, DeleteLimit stays blocked

> _Note: this issue was identified against the code base at commit `73a889768ef107d9a74aea509f6592e40712e251`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueLimitAssociation

## Description
The deadline queue-limit-association lifecycle (create association -> stop it -> poll to STOPPED -> delete association -> delete limit) is observable only through GetQueueLimitAssociation and DeleteLimit, and all three of its observable transitions diverge. The shared fault: an association, once created, is frozen in the state it was created in — its status never moves off ACTIVE, it never acquires updatedAt/updatedBy, it is never removed, and because it is never removed the limit it references can never be deleted. 3 causes, all on service deadline, all from fixture deadline-queue-limit-association-lifecycle.

Cause 1 of 3: GetQueueLimitAssociation reports an association that was asked to stop as still ACTIVE, and never reports updatedAt/updatedBy (fixture deadline-queue-limit-association-lifecycle, step task#8).
AWS: Smithy (services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json) models UpdateQueueLimitAssociation as PATCH /2023-10-12/farms/{farmId}/queue-limit-associations/{queueId}/{limitId}, @idempotent, with required input member status of UpdateQueueLimitAssociationStatus (ACTIVE | STOP_LIMIT_USAGE_AND_COMPLETE_TASKS | STOP_LIMIT_USAGE_AND_CANCEL_TASKS), and GetQueueLimitAssociation (GET on the same URI, 200) as returning required queueId, limitId, status (QueueLimitAssociationStatus: ACTIVE | STOP_LIMIT_USAGE_AND_COMPLETE_TASKS | STOP_LIMIT_USAGE_AND_CANCEL_TASKS | STOPPED), createdAt (@timestampFormat date-time), createdBy, plus optional updatedAt (date-time) and updatedBy. DeleteQueueLimitAssociation's own model documentation states the contract: "You must use the UpdateQueueLimitAssociation operation to set the status to STOP_LIMIT_USAGE_AND_COMPLETE_TASKS or STOP_LIMIT_USAGE_AND_CANCEL_TASKS. The status does not change immediately. Use the GetQueueLimitAssociation operation to see if the status changed to STOPPED before deleting the association." So after a stop request AWS moves the association to a stopping status and then, server-side, to STOPPED, and every mutation stamps updatedAt (the time of that mutation) and updatedBy (the caller identity that made it) — both values come from the request/state at mutation time, never from a fixed string. The recording asserts exactly that: status becomes STOPPED and updatedAt/updatedBy are present (their concrete values are shaped-not-asserted because they are per-run).
Emulator: GetQueueLimitAssociation keeps answering 200 with status ACTIVE for 13 poll attempts until the poll is capped at 60s, and updatedAt and updatedBy are absent from the response body altogether (got None). createdAt/createdBy are populated. Root cause: UpdateQueueLimitAssociation is still an unimplemented 501 scaffold (services/service-com-amazonaws-deadline-deadline/src/ops/update_queue_limit_association.rs), so nothing ever writes the stored association's status/updatedAt/updatedBy; GetQueueLimitAssociation itself already reads and serializes all three fields correctly (src/ops/get_queue_limit_association.rs lines 206-214), so this is a state-transition gap, not a serializer gap. The open issue 20261113T145922Z-deadline-updatequeuelimitassociation-stub.md reports that operation's 501 but carries no replay key for this fixture step and does NOT cover the lifecycle divergence recorded here.
Why: every documented way to stop a limit's usage hangs. A client follows AWS's documented sequence — PATCH the status, then poll GetQueueLimitAssociation until STOPPED — and the poll never terminates, so the caller either loops to its own timeout or gives up and proceeds to delete, which then fails (cause 3). Clients that render or reconcile on updatedAt/updatedBy (change detection, audit trails, optimistic "has anything changed since" checks) see an association that looks like it was never touched.
Fix: the status-transition state for queue-limit associations — the UpdateQueueLimitAssociation handler plus the shared association record in src/state.rs — must accept the stop statuses, stamp updatedAt/updatedBy on the stored association, and progress a stopping association to STOPPED so a poll converges.
Traces: deadline-queue-limit-association-lifecycle#task#8 (verdict mismatch; expected status 200 with status=STOPPED, got 200 with status=ACTIVE).

Cause 2 of 3: after the association is deleted, GetQueueLimitAssociation still answers 200 with the association instead of 404 ResourceNotFoundException (fixture deadline-queue-limit-association-lifecycle, step task#10).
AWS: Smithy models GetQueueLimitAssociation with ResourceNotFoundException (@error client, @httpError 404; required message, resourceId, resourceType, optional context), and DeleteQueueLimitAssociation (DELETE on the same URI, @idempotent) as actually removing the association. A read of an association that no longer exists is therefore a 404, not a 200. Recorded behavior pins the shape of that refusal: HTTP 404, x-amzn-errortype header present, resourceType = "queue-limit-association" (the path-shaped resource-type token, not a camelCase one), resourceId = the three ids of the association joined as "<farmId>, <queueId> and <limitId>", and message = "Resource of type queue-limit-association with id <farmId>, <queueId> and <limitId> does not exist." The fixed text of that message is "Resource of type " + the resource type + " with id " + the resource id + " does not exist."; the farm, queue and limit ids are values taken from the request path (the concrete ids in the recording are per-run and are not part of the template).
Emulator: the step gets HTTP 200 carrying the association body; message, resourceType, resourceId and the x-amzn-errortype header are all absent (got None). Root cause: DeleteQueueLimitAssociation is an unimplemented 501 scaffold (src/ops/delete_queue_limit_association.rs, handle() returns Scaffolded::not_implemented), so the association is never removed from state and the subsequent read legitimately finds it. GetQueueLimitAssociation already has the correct 404 branch (src/ops/get_queue_limit_association.rs lines 193-204, via crate::state::queue_limit_association_not_found), so once the delete mutates state this read must start refusing — and its resourceType must be the path-shaped "queue-limit-association" (see open issue 20261112T132224Z-deadline-resourcenotfoundexception-spells-the-storage-profile-resource-type, which reports the camelCase spelling defect for storage profiles and does not cover this resource).
Why: a client cannot tell a deleted association from a live one. Delete-then-verify is the standard way to confirm teardown, and here the verification says the association is still ACTIVE, so idempotent cleanup loops re-delete forever, reconcilers recreate nothing and keep reporting drift, and code that branches on ResourceNotFoundException to mean "already gone" never takes that branch.
Fix: DeleteQueueLimitAssociation's handler must remove the association from the service state (and refuse per its model when the association is not STOPPED), so the association store no longer returns it; the read path's existing 404 construction is correct and must keep its recorded wording.
Traces: deadline-queue-limit-association-lifecycle#task#10 (verdict mismatch; expected 404, got 200).

Cause 3 of 3: DeleteLimit refuses with 400 ValidationException a limit whose association has been deleted, instead of succeeding with 200 (fixture deadline-queue-limit-association-lifecycle, step teardown#11).
AWS: Smithy models DeleteLimit as DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, @idempotent, success code 200 with an empty body, errors AccessDeniedException 403, InternalServerErrorException 500, ThrottlingException 429, ValidationException 400. A limit that is no longer referenced by any queue-limit association has no live reference blocking it, so the delete succeeds: the recording shows HTTP 200 for this teardown step after the association was stopped and deleted.
Emulator: HTTP 400. The DeleteLimit handler (src/ops/delete_limit.rs lines 148-156) refuses whenever store.queue_associated_with_limit(farmId, limitId) finds ANY association for the limit, and since DeleteQueueLimitAssociation is a 501 stub the association is never removed, so the reference check never clears and the limit can never be deleted. The refusal is additionally the wrong class for this service (open issue 20261113T145922Z-deadline-deletelimit-on-a-limit-associated-with-a reports that AWS answers 409 ConflictException with the fixed message "Request could not be completed: Limit is associated with one or more Queues." rather than this 400 ValidationException with a locally-invented sentence) — that issue covers the class of the refusal, NOT the fact recorded here that a limit whose association is gone is refused at all.
Why: the limit, and everything created for the scenario, is permanently undeletable. Dependency-ordered teardown aborts at its last step, test fixtures and farms leak, and quota-bearing resources accumulate; a client that correctly follows AWS's documented stop-then-delete-association-then-delete-limit order is told its request is invalid.
Fix: in DeleteLimit's refusal path and the association state it consults, the live-reference check must consult associations that actually still exist — i.e. it must clear once DeleteQueueLimitAssociation removes the record — so the delete succeeds with an empty 200 for an unreferenced limit.
Traces: deadline-queue-limit-association-lifecycle#teardown#11 (verdict mismatch; expected 200, got 400).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 71 steps, 53 passed, score 74.6%). Fixture deadline-queue-limit-association-lifecycle, service deadline.

Cause 1 — step task#8, operation GetQueueLimitAssociation, verdict mismatch, divergence_key service=deadline|GetQueueLimitAssociation|status=200->200|keys=$,$.status,$.updatedAt,$.updatedBy:
  Expected (AWS, recorded): HTTP 200, body {queueId: <queueId>, limitId: <limitId>, status: "STOPPED", createdAt: <date-time>, createdBy: <identity>, updatedAt: <date-time, shaped-not-asserted>, updatedBy: <identity, shaped-not-asserted>}
  Actual (emulator):        HTTP 200, body {queueId: <queueId>, limitId: <limitId>, status: "ACTIVE", createdAt: <date-time>, createdBy: <identity>}  — updatedAt missing (None), updatedBy missing (None)
  Replay detail: "$.status: value_mismatch (expected 'STOPPED', got 'ACTIVE')"; "$.updatedBy: missing"; "$.updatedAt: missing"; "$.status: poll: not in ['STOPPED'] after 13 attempt(s) (expected 'STOPPED', got 'ACTIVE')"; poll capped at 60s.

Cause 2 — step task#10, operation GetQueueLimitAssociation, verdict mismatch, divergence_key service=deadline|GetQueueLimitAssociation|status=404->200|keys=$.message,$.resourceId,$.resourceType,$headers.x-amzn-errortype,status:
  Expected (AWS, recorded): HTTP 404 with x-amzn-errortype header; resourceType = "queue-limit-association"; resourceId = "<farmId>, <queueId> and <limitId>"; message = "Resource of type queue-limit-association with id <farmId>, <queueId> and <limitId> does not exist."  (fixed text: "Resource of type " / " with id " / " does not exist."; the three ids are request-path values — the recording's farm-5eac82f1..., queue-d9734574..., limit-e3880bc2... are per-run and not part of the template)
  Actual (emulator):        HTTP 200 carrying the association body; message / resourceType / resourceId / x-amzn-errortype all absent.

Cause 3 — step teardown#11, operation DeleteLimit, verdict mismatch, divergence_key service=deadline|DeleteLimit|status=200->400|keys=status:
  Expected (AWS, recorded): HTTP 200, empty body (the limit's association had been stopped and deleted).
  Actual (emulator):        HTTP 400 (ValidationException from the live-reference check in src/ops/delete_limit.rs:148-156, which still sees the association that DeleteQueueLimitAssociation never removed).

Model evidence (smithy_query on services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json):
  GetQueueLimitAssociation: GET /2023-10-12/farms/{farmId}/queue-limit-associations/{queueId}/{limitId}, 200, readonly; output requires queueId, limitId, status, createdAt (date-time), createdBy; optional updatedAt (date-time), updatedBy; errors AccessDenied 403, InternalServerError 500, ResourceNotFound 404, Throttling 429, Validation 400.
  UpdateQueueLimitAssociation: PATCH same URI, idempotent, required status of UpdateQueueLimitAssociationStatus = ACTIVE | STOP_LIMIT_USAGE_AND_COMPLETE_TASKS | STOP_LIMIT_USAGE_AND_CANCEL_TASKS.
  DeleteQueueLimitAssociation: DELETE same URI, idempotent, 200 empty output; docs: stop via UpdateQueueLimitAssociation, status does not change immediately, poll GetQueueLimitAssociation for STOPPED before deleting.
  QueueLimitAssociationStatus = ACTIVE | STOP_LIMIT_USAGE_AND_COMPLETE_TASKS | STOP_LIMIT_USAGE_AND_CANCEL_TASKS | STOPPED.
  DeleteLimit: DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, idempotent, 200 empty output; errors AccessDenied 403, InternalServerError 500, Throttling 429, Validation 400.
  ResourceNotFoundException: @error(client) @httpError(404); required message, resourceId, resourceType; optional context.

Code evidence: src/ops/update_queue_limit_association.rs and src/ops/delete_queue_limit_association.rs are both "SCAFFOLDED, NOT IMPLEMENTED" and return Scaffolded::not_implemented (501); src/ops/get_queue_limit_association.rs:193-214 already has the correct 404 branch and serializes updatedAt/updatedBy; src/ops/delete_limit.rs:148-156 refuses on any surviving association.

Related open issues that do NOT cover these steps (no replay key for this fixture): 20261113T145922Z-deadline-updatequeuelimitassociation-stub.md, 20261113T145922Z-deadline-deletelimit-on-a-limit-associated-with-a.md, 20261112T132224Z-deadline-resourcenotfoundexception-spells-the-storage-profile-resource-type.md, 20261102T095011Z-deadline-queue-fleet-association-lifecycle-diverges-on-five.md (queue-FLEET associations, a different resource).

## Steps to Reproduce
1. deadline.CreateFarm(displayName=<unique>) -> farmId
2. deadline.CreateQueue(farmId, displayName=<unique>) -> queueId
3. deadline.CreateLimit(farmId, displayName=<unique>, amountRequirementName="amount.<name>", maxCount=<n>) -> limitId
4. deadline.CreateQueueLimitAssociation(farmId, queueId, limitId)
5. deadline.UpdateQueueLimitAssociation(farmId, queueId, limitId, status="STOP_LIMIT_USAGE_AND_CANCEL_TASKS")
6. Poll deadline.GetQueueLimitAssociation(farmId, queueId, limitId) until status == "STOPPED"  [Cause 1]
   -> AWS: converges to status STOPPED, and the response carries updatedAt and updatedBy.
   -> Emulator: status stays "ACTIVE" for every attempt (13 attempts / 60s cap) and updatedAt/updatedBy are never present.
7. deadline.DeleteQueueLimitAssociation(farmId, queueId, limitId)
8. deadline.GetQueueLimitAssociation(farmId, queueId, limitId)  [Cause 2]
   -> AWS: HTTP 404 ResourceNotFoundException, x-amzn-errortype set, resourceType="queue-limit-association", resourceId="<farmId>, <queueId> and <limitId>", message="Resource of type queue-limit-association with id <farmId>, <queueId> and <limitId> does not exist."
   -> Emulator: HTTP 200 with the association body (status ACTIVE).
9. deadline.DeleteLimit(farmId, limitId)  [Cause 3]
   -> AWS: HTTP 200, empty body.
   -> Emulator: HTTP 400 ValidationException (the deleted association is still in state and the live-reference check refuses).

Replay with: replay_wire_corpus, fixture deadline-queue-limit-association-lifecycle, steps task#8, task#10, teardown#11.
