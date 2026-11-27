Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.update-worker-refuses-stopping-a-worker-that-never-started-rejected deadline.update-worker-rejects-starting-a-stopping-worker-until-it-has-stopped deadline.update-worker-rejects-started-without-capabilities deadline.update-worker-schedule-refuses-a-worker-that-is-not-started-rejected deadline.batch-get-job-entity-refuses-an-unknown-stopped-or-deleted-worker deadline.assume-queue-role-for-worker-malformed-queue-id-rejected deadline.batch-get-job-entity-rejects-more-than-ten-identifiers deadline.create-worker-rejects-malformed-host-properties-rejected deadline.create-worker-rejected-until-the-fleet-max-worker-count-is-raised deadline.batch-get-job-entity-reports-an-environment-the-job-does-not-define-as-not-found deadline.batch-get-job-entity-reports-per-job-why-attachments-are-not-returned deadline.update-worker-schedule-reports-a-missing-queue-log-group-as-the-session-log-error
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.update-worker-refuses-stopping-a-worker-that-never-started-rejected|_refused#5|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {answer!r}")
Issue-Key: deadline.update-worker-rejects-starting-a-stopping-worker-until-it-has-stopped|_refusal_with_reason#12|AssertionError|raise AssertionError(f"expected a refusal, but the call answered with {sorted(answer)!r}")
Issue-Key: deadline.update-worker-rejects-started-without-capabilities|_refusal_with_reason#6|AssertionError|raise AssertionError(f"expected a refusal, but the call answered with {sorted(answer)!r}")
Issue-Key: deadline.update-worker-schedule-refuses-a-worker-that-is-not-started-rejected|_refused#6|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {answer!r}")
Issue-Key: deadline.batch-get-job-entity-refuses-an-unknown-stopped-or-deleted-worker|_refusal#20|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded with members {sorted(out)!r}")
Issue-Key: deadline.assume-queue-role-for-worker-malformed-queue-id-rejected|run#9|AssertionError|assert (refused["Code"], refused["Message"]) == (
Issue-Key: deadline.batch-get-job-entity-rejects-more-than-ten-identifiers|run#14|AssertionError|assert got == ("ValidationException",
Issue-Key: deadline.create-worker-rejects-malformed-host-properties-rejected|_refused#5|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {answer!r}")
Issue-Key: deadline.create-worker-rejected-until-the-fleet-max-worker-count-is-raised|run#5|AssertionError|assert capped == ("ConflictException", f"Fleet {fleet} has reached its maxWorkerCount of 0 workers."), (
Issue-Key: deadline.batch-get-job-entity-reports-an-environment-the-job-does-not-define-as-not-found|run#27|AssertionError|assert (err["code"], err["message"]) == expected, (
Issue-Key: deadline.batch-get-job-entity-reports-per-job-why-attachments-are-not-returned|run#26|AssertionError|assert failures == {
Issue-Key: deadline.update-worker-schedule-reports-a-missing-queue-log-group-as-the-session-log-error|run#10|AssertionError|assert first == {"logDriver": "awslogs", "options": {"logGroupName": group, "logStreamName": first_id},
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# deadline worker/scheduling data plane: ten divergences — missing status-conflict and field-validation refusals in UpdateWorker/UpdateWorkerSchedule/BatchGetJobEntity/CreateWorker, plus wrong ValidationException/ConflictException/per-entity error wording and a missing logConfiguration member

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorker

## Description
Ten net-new root causes in the deadline (Deadline Cloud) emulator's worker and scheduling data plane. Each is numbered with its own failing check ids; all ten must be closed before submitting. None of them is the GetSessionAction / ListWorkers / AssumeFleetRoleForWorker / AssumeQueueRoleForRead scaffold stubs (already filed), and none is covered by the open issues 20261124T074142Z-deadline-deletequeuefleetassociation-... or 20261125T140150Z-deadline-getqueueenvironment-... , which concern other operations' messages.

--- Cause 1 of 10: UpdateWorker accepts any reported status without checking the worker's current status ---
AWS: Smithy UpdateWorker (PATCH /2023-10-12/farms/{farmId}/fleets/{fleetId}/workers/{workerId}) models ConflictException (HTTP 409), whose required members are message, reason (ConflictExceptionReason, one of CONFLICT_EXCEPTION|CONCURRENT_MODIFICATION|RESOURCE_ALREADY_EXISTS|RESOURCE_IN_USE|STATUS_CONFLICT), resourceId and resourceType, plus optional context. Recorded behavior: a worker still in CREATED that reports status=STOPPING is refused 409 ConflictException with message "Request could not be completed. Status <currentStatus> of resource <workerId> does not permit the operation. The valid statuses are <list>" — the fixed text is "Request could not be completed. Status ", " of resource ", " does not permit the operation. The valid statuses are "; the variable parts are the worker's current status from state (CREATED), the workerId from the request path, and the list of statuses that permit the transition ([STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE] for a stop report, [STARTED, STOPPED, CREATED, NOT_RESPONDING, IDLE, RUNNING] for a start report from STOPPING). The refusal also carries reason=STATUS_CONFLICT, resourceType="worker", resourceId=<workerId> and context.status=<currentStatus>, and leaves the worker's status untouched.
Emulator: both transitions answer HTTP 200 with an empty body and move the worker. services/service-com-amazonaws-deadline-deadline/src/ops/update_worker.rs assigns `worker.status = status` unconditionally (its comment asserts the model orders the three UpdatedWorkerStatus values in no way, so no transition is refused).
Why: a worker agent (or any client) that relies on Deadline Cloud serialising its lifecycle sees an illegal transition succeed — a shutting-down worker can resurrect itself and a never-started worker can be stopped, so agent state machines and conflict-retry paths are never exercised and status is silently corrupted.
Fix: the UpdateWorker handler's status handling (src/ops/update_worker.rs) plus a worker status-transition rule and the ConflictException constructor in the service state helpers (crate::state).
Traces: deadline.update-worker-refuses-stopping-a-worker-that-never-started-rejected, deadline.update-worker-rejects-starting-a-stopping-worker-until-it-has-stopped.

--- Cause 2 of 10: UpdateWorker accepts status=STARTED with no capabilities ---
AWS: UpdateWorker models ValidationException (HTTP 400) with required members message and reason (ValidationExceptionReason: UNKNOWN_OPERATION|CANNOT_PARSE|FIELD_VALIDATION_FAILED|OTHER). Recorded behavior: reporting status=STARTED without the `capabilities` member is refused 400 ValidationException, message "Required parameter capabilities is missing from the request" (fixed text, with the member name `capabilities` as its only variable part) and reason=FIELD_VALIDATION_FAILED; the worker stays CREATED.
Emulator: HTTP 200 with a body carrying no members, and the worker moves to STARTED.
Why: a worker may register as available with no advertised capabilities, so the scheduler can hand it work it cannot run; clients never see the conditional-required rule AWS enforces.
Fix: UpdateWorker's input validation in src/ops/update_worker.rs (status=STARTED requires capabilities), raising the modeled ValidationException with reason FIELD_VALIDATION_FAILED.
Traces: deadline.update-worker-rejects-started-without-capabilities.

--- Cause 3 of 10: UpdateWorkerSchedule serves a worker that is not started ---
AWS: UpdateWorkerSchedule (PATCH .../workers/{workerId}/schedule) models ConflictException (409). Recorded behavior: a CREATED or STOPPED worker asking for its schedule is refused 409 ConflictException with message "Request could not be completed. Status <currentStatus> of resource <workerId> does not permit the operation. The valid statuses are [STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE]" (fixed text as in Cause 1; variables = the worker's current status and the workerId), reason=STATUS_CONFLICT, resourceType="worker", resourceId=<workerId>, and the status is unchanged. Only a started worker gets a (possibly empty) schedule.
Emulator: HTTP 200 with {"assignedSessions": {}, "cancelSessionActions": {}, "updateIntervalSeconds": <n>} for a CREATED worker.
Why: an agent that never reported itself STARTED is handed a schedule contract, so the "start before you poll" protocol is unverifiable and a stopped agent keeps polling successfully.
Fix: the UpdateWorkerSchedule handler (src/ops/update_worker_schedule.rs) — gate on the worker's status before building the schedule, reusing the status-conflict constructor from Cause 1.
Traces: deadline.update-worker-schedule-refuses-a-worker-that-is-not-started-rejected.

--- Cause 4 of 10: BatchGetJobEntity answers a STOPPED worker instead of refusing it ---
AWS: BatchGetJobEntity (POST .../workers/{workerId}/batchGetJobEntity). Recorded behavior: once the worker has reported STOPPED the call is refused with ConflictException and message "Request could not be completed. Status STOPPED of resource <workerId> does not permit the operation. The valid statuses are [STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE]" (same fixed text as Cause 1; variables = current status and workerId). The emulator already refuses an unknown worker and an unknown fleet correctly with ResourceNotFoundException "Resource of type worker with id <id> does not exist.", so only the status gate is missing.
Emulator: HTTP 200 with members entities/errors for the STOPPED worker.
Why: a client cannot tell a stopped worker from a running one through the data plane, so a decommissioned agent keeps reading job entities.
Fix: the BatchGetJobEntity handler (src/ops/batch_get_job_entity.rs) — check the worker's status before resolving identifiers.
Traces: deadline.batch-get-job-entity-refuses-an-unknown-stopped-or-deleted-worker.

--- Cause 5 of 10: deadline ValidationException messages for constraint violations use the generic AWS constraint wording, not Deadline Cloud's ---
AWS: Deadline Cloud renders modeled-constraint violations in its own format, not the "N validation error(s) detected: ..." sentence most services use. Recorded behavior for a @pattern violation (Smithy: AssumeQueueRoleForWorker.queueId, target QueueId, pattern ^queue-[0-9a-f]{32}$): 400 ValidationException with message 'Total 1 validation error found\n queueId\n string does not match regex "^queue-[0-9a-f]{32}$"' — fixed text is "Total ", " validation error found", then a newline-indented block of the member name and 'string does not match regex "<pattern>"'; variables are the count, the offending member name (queueId) and the pattern from the model. Recorded behavior for a list @length violation (Smithy: BatchGetJobEntity.identifiers, @length min 1 max 10): 400 ValidationException with message "[array is too long: must have at most 10 elements but instance has 11 elements]" — fixed text "[array is too long: must have at most ", " elements but instance has ", " elements]"; variables are the modeled maximum and the number of elements sent.
Emulator: generic wording — "1 validation error detected: Value 'queue-D1D187E0F81A20CA437A938D4FA3EC16' at 'queueId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^queue-[0-9a-f]{32}$" and "1 validation error detected: Value '11 items' at 'identifiers' failed to satisfy constraint: Member must have length less than or equal to 10". Status (400) and code (ValidationException) are right; the message is not.
Why: Deadline Cloud clients and the worker agent surface and match on these messages; a client parsing the Deadline format gets nothing it recognises, and an operator sees a message AWS never produces.
Fix: the request-validation layer that renders constraint violations into this service's ValidationException (the deadline crate's validation/error mapping over the shared smithy-server-runtime validation hook), for both pattern and list-length violations.
Traces: deadline.assume-queue-role-for-worker-malformed-queue-id-rejected, deadline.batch-get-job-entity-rejects-more-than-ten-identifiers.

--- Cause 6 of 10: CreateWorker accepts malformed hostProperties ---
AWS: Smithy CreateWorker.hostProperties is HostPropertiesRequest {ipAddresses: IpAddresses{ipV4Addresses: list of IpV4Address, @pattern ^(?:[0-9]{1,3}\\.){3}[0-9]{1,3}$}, hostName @pattern ^[a-zA-Z0-9_\\.\\-]{0,255}$} and the operation models ValidationException (400). The model's IPv4 pattern alone allows a dotted quad with out-of-range octets, so AWS applies a further range check: recorded behavior for ipV4Addresses=["192.0.2.256"] is 400 ValidationException, message "The IP address: <address> does not appear to be formatted correctly. Please use standard format for IPv4." (fixed text, with the rejected address from the request as the only variable) and reason=FIELD_VALIDATION_FAILED, and no worker is registered. The same check next asserts that a hostName containing spaces is refused with message '[ECMA 262 regex "^[a-zA-Z0-9_\\.\\-]{0,255}$" does not match input string "<hostName>"]' (fixed text around the modeled pattern and the rejected input) — that call was never reached because the IPv4 case already succeeded, so fix both.
Emulator: HTTP 200 with a minted workerId for the out-of-range address (observed body: {"workerId": "worker-<32 hex>"}).
Why: a client gets a registered worker carrying an impossible host address, and the negative path a fleet's host-property validation depends on silently disappears.
Fix: CreateWorker's hostProperties validation (src/ops/create_worker.rs) — IPv4 octet-range check with Deadline's message and reason FIELD_VALIDATION_FAILED, and the hostName pattern refusal in the ECMA-262 form, both before anything is written to state.
Traces: deadline.create-worker-rejects-malformed-host-properties-rejected.

--- Cause 7 of 10: CreateWorker's maxWorkerCount conflict message is worded differently from AWS's ---
AWS: recorded behavior — registering a worker in a fleet whose maxWorkerCount is already reached is refused 409 ConflictException with message "Fleet <fleetId> has reached its maxWorkerCount of <n> workers." The fixed text is "Fleet ", " has reached its maxWorkerCount of ", " workers."; the variables are the fleetId from the request and the fleet's configured maxWorkerCount.
Emulator: same code (ConflictException) but message "The fleet <fleetId> already has its maximum worker count of <n> workers."
Why: a client that matches on the refusal text to decide whether to raise the cap does not recognise the emulator's sentence.
Fix: the cap-exceeded ConflictException message in the CreateWorker handler (src/ops/create_worker.rs).
Traces: deadline.create-worker-rejected-until-the-fleet-max-worker-count-is-raised.

--- Cause 8 of 10: BatchGetJobEntity's environmentDetails not-found message swaps the resource type and id ---
AWS: recorded behavior — an environmentDetails identifier naming an environment the job does not define comes back in the response's per-entity `errors` list with code ResourceNotFoundException and message "Resource of type <environmentId> with id Environment does not exist." AWS puts the environment id where the resource type belongs and the literal word "Environment" where the id belongs; the fixed text is "Resource of type ", " with id Environment does not exist." and the only variable is the environmentId from the request (shaped JOB:<jobId>:<name>).
Emulator: code is right but the message is the conventional order — "Resource of type environment with id JOB:<jobId>:<name> does not exist."
Whey/Why: a worker agent matching Deadline Cloud's own per-entity message does not recognise the emulator's, so the "environment not defined" branch is never taken.
Fix: the per-entity error rendering for environmentDetails in the BatchGetJobEntity handler (src/ops/batch_get_job_entity.rs).
Traces: deadline.batch-get-job-entity-reports-an-environment-the-job-does-not-define-as-not-found.

--- Cause 9 of 10: BatchGetJobEntity's "job has no attachments" per-entity message is wrong ---
AWS: recorded behavior — a jobAttachmentDetails identifier for a job the worker IS assigned but which has no job attachments comes back in `errors` with code ResourceNotFoundException and message "The <jobId> does not have job attachments" (fixed text "The ", " does not have job attachments", no trailing period; the variable is the jobId). A jobAttachmentDetails identifier for a job the worker cannot see keeps the ordinary "Resource of type job with id <jobId> does not exist." form, which the emulator already gets right.
Emulator: "Job <jobId> has no job attachments." for the assigned-but-attachment-less job.
Why: the agent cannot distinguish "no attachments on this job" from other not-found causes, so it retries or fails a session it should simply run without attachments.
Fix: the per-entity error rendering for jobAttachmentDetails in the BatchGetJobEntity handler (src/ops/batch_get_job_entity.rs).
Traces: deadline.batch-get-job-entity-reports-per-job-why-attachments-are-not-returned.

--- Cause 10 of 10: an assigned session's logConfiguration omits the `parameters` member ---
AWS: Smithy LogConfiguration = {logDriver (required), options (map), parameters (map), error}. Recorded behavior: UpdateWorkerSchedule's assignedSessions[<sessionId>].logConfiguration is {"logDriver": "awslogs", "options": {"logGroupName": "/aws/deadline/<farmId>/<queueId>", "logStreamName": "<sessionId>"}, "parameters": {}} — the empty `parameters` map is always serialized, and when the queue's log group is missing the member set becomes options without logStreamName plus "error": "ResourceNotFoundException" (still with "parameters": {}).
Emulator: the member is absent — observed {"logDriver": "awslogs", "options": {"logStreamName": "<sessionId>", "logGroupName": "/aws/deadline/<farmId>/<queueId>"}}.
Why: a client that reads logConfiguration.parameters without a default (as the agent's log-driver setup does) raises a KeyError/None instead of getting the empty map AWS always sends.
Fix: the serialization of LogConfiguration for assigned sessions in the UpdateWorkerSchedule handler (src/ops/update_worker_schedule.rs) — emit `parameters` as an empty map rather than omitting it.
Traces: deadline.update-worker-schedule-reports-a-missing-queue-log-group-as-the-session-log-error (the same check also asserts the missing-log-group `error` member and the restored-group case, which were not reached).

## Evidence
Replay output (behavioral suite, service heading '### deadline'):

Cause 1 — [mismatch] deadline.update-worker-refuses-stopping-a-worker-that-never-started-rejected: AssertionError: expected a refusal, the call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (expected ConflictException "Request could not be completed. Status CREATED of resource <workerId> does not permit the operation. The valid statuses are [STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE]", reason STATUS_CONFLICT, resourceType 'worker', context.status 'CREATED').
[mismatch] deadline.update-worker-rejects-starting-a-stopping-worker-until-it-has-stopped: AssertionError: expected a refusal, but the call answered with ['ResponseMetadata'] (expected ConflictException "... Status STOPPING of resource <workerId> ... The valid statuses are [STARTED, STOPPED, CREATED, NOT_RESPONDING, IDLE, RUNNING]", reason STATUS_CONFLICT).
Source: src/ops/update_worker.rs lines 201-203 — `if let Some(status) = input.status { worker.status = status; }`, no transition check.

Cause 2 — [mismatch] deadline.update-worker-rejects-started-without-capabilities: AssertionError: expected a refusal, but the call answered with ['ResponseMetadata']; expected ('ValidationException', 'Required parameter capabilities is missing from the request', 'FIELD_VALIDATION_FAILED').

Cause 3 — [mismatch] deadline.update-worker-schedule-refuses-a-worker-that-is-not-started-rejected: AssertionError: expected a refusal, the call succeeded with {... 'assignedSessions': {}, 'cancelSessionActions': {}, 'updateIntervalSeconds': 5}; expected ConflictException "Request could not be completed. Status CREATED of resource <workerId> does not permit the operation. The valid statuses are [STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE]" with reason STATUS_CONFLICT, resourceType 'worker', resourceId <workerId>.

Cause 4 — [mismatch] deadline.batch-get-job-entity-refuses-an-unknown-stopped-or-deleted-worker: AssertionError: expected a refusal, but the call succeeded with members ['ResponseMetadata', 'entities', 'errors'] after the worker reported STOPPED; expected ConflictException "Request could not be completed. Status STOPPED of resource <workerId> does not permit the operation. The valid statuses are [STARTED, STOPPING, NOT_COMPATIBLE, RUNNING, IDLE]".

Cause 5 — [mismatch] deadline.assume-queue-role-for-worker-malformed-queue-id-rejected: got {'Code': 'ValidationException', 'Message': "1 validation error detected: Value 'queue-D1D187E0F81A20CA437A938D4FA3EC16' at 'queueId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^queue-[0-9a-f]{32}$"}; expected message 'Total 1 validation error found\n queueId\n string does not match regex "^queue-[0-9a-f]{32}$"'.
[mismatch] deadline.batch-get-job-entity-rejects-more-than-ten-identifiers: got ('ValidationException', "1 validation error detected: Value '11 items' at 'identifiers' failed to satisfy constraint: Member must have length less than or equal to 10"); expected ('ValidationException', '[array is too long: must have at most 10 elements but instance has 11 elements]').

Cause 6 — [mismatch] deadline.create-worker-rejects-malformed-host-properties-rejected: AssertionError: expected a refusal, the call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'workerId': 'worker-<32 hex>'} for hostProperties {'hostName': 'render-node-01', 'ipAddresses': {'ipV4Addresses': ['192.0.2.256']}}; expected ValidationException "The IP address: 192.0.2.256 does not appear to be formatted correctly. Please use standard format for IPv4." with reason FIELD_VALIDATION_FAILED.

Cause 7 — [mismatch] deadline.create-worker-rejected-until-the-fleet-max-worker-count-is-raised: got ('ConflictException', 'The fleet <fleetId> already has its maximum worker count of 0 workers.'); expected ('ConflictException', 'Fleet <fleetId> has reached its maxWorkerCount of 0 workers.').

Cause 8 — [mismatch] deadline.batch-get-job-entity-reports-an-environment-the-job-does-not-define-as-not-found: got ('ResourceNotFoundException', 'Resource of type environment with id JOB:<jobId>:<name> does not exist.'); expected ('ResourceNotFoundException', 'Resource of type JOB:<jobId>:<name> with id Environment does not exist.').

Cause 9 — [mismatch] deadline.batch-get-job-entity-reports-per-job-why-attachments-are-not-returned: got {(True, 'ResourceNotFoundException', 'Job <jobId> has no job attachments.'), (False, 'ResourceNotFoundException', 'Resource of type job with id <otherJobId> does not exist.')}; expected the assigned job's message to be 'The <jobId> does not have job attachments'.

Cause 10 — [mismatch] deadline.update-worker-schedule-reports-a-missing-queue-log-group-as-the-session-log-error: got {'logDriver': 'awslogs', 'options': {'logStreamName': '<sessionId>', 'logGroupName': '/aws/deadline/<farmId>/<queueId>'}}; expected {'logDriver': 'awslogs', 'options': {'logGroupName': '<group>', 'logStreamName': '<sessionId>'}, 'parameters': {}}.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite; the twelve checks listed in trace_ids fail. By hand (deadline client, scheduling endpoint), after CreateFarm + CreateFleet(maxWorkerCount>=1) + CreateWorker:
1. UpdateWorker(status='STOPPING', capabilities=<caps>) on the fresh CREATED worker -> emulator 200, AWS 409 ConflictException/STATUS_CONFLICT (Cause 1). Then UpdateWorker STARTED, UpdateWorker STOPPING, UpdateWorker STARTED -> the last is 200 here, 409 on AWS (Cause 1).
2. UpdateWorker(status='STARTED') with no capabilities -> 200 here, 400 ValidationException/FIELD_VALIDATION_FAILED on AWS (Cause 2).
3. UpdateWorkerSchedule on a CREATED (or STOPPED) worker -> 200 with an empty schedule here, 409 ConflictException on AWS (Cause 3).
4. UpdateWorker(status='STOPPED'), then BatchGetJobEntity(identifiers=[{'jobDetails':{'jobId':'job-' + '0'*32}}]) -> 200 here, 409 ConflictException on AWS (Cause 4).
5. AssumeQueueRoleForWorker(queueId=<queue id with upper-case hex>) and BatchGetJobEntity with 11 identifiers -> 400 ValidationException with the generic constraint sentence instead of Deadline's wording (Cause 5).
6. CreateWorker(hostProperties={'hostName':'render-node-01','ipAddresses':{'ipV4Addresses':['192.0.2.256']}}) -> 200 with a workerId here, 400 ValidationException on AWS (Cause 6); CreateWorker(hostProperties={'hostName':'render node 01', ...}) likewise.
7. CreateFleet(maxWorkerCount=0) then CreateWorker -> 409 with the emulator's own sentence (Cause 7).
8/9. Submit a job to a queue served by the fleet, take the assignment through UpdateWorkerSchedule, then BatchGetJobEntity with {'environmentDetails':{'jobId':<job>,'environmentId':'JOB:<job>:<undefined name>'}} and with {'jobAttachmentDetails':{'jobId':<assigned job without attachments>}} -> per-entity messages diverge (Causes 8, 9).
10. Read assignedSessions[<id>].logConfiguration from that same UpdateWorkerSchedule response -> no 'parameters' member (Cause 10).
