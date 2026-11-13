Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-instance-full-lifecycle timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-instance-full-lifecycle|run#16|AssertionError|assert message == "The DB instance with db id does not exist", message
Issue-Key: timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds|run#10|AssertionError|assert message == (
Issue-Commit: d1ed6d82cb6e069fe1b5dc1fba14ae1b2ea73c86

# timestream-influxdb: 2 net-new causes — RebootDbInstance ResourceNotFoundException and not-rebootable-state ValidationException messages both diverge from AWS's wording

> _Note: this issue was identified against the code base at commit `d1ed6d82cb6e069fe1b5dc1fba14ae1b2ea73c86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDbInstance

## Description
Two net-new root causes, both in the RebootDbInstance handler's refusal messages. In both the HTTP status and the error code are now correct — only the message text diverges — so each is a message-template defect, not a missing refusal.

Context on neighbouring issues (neither covers these):
- The FIXED issue 20261104T042504Z-timestream-influxdb-rebootdbinstance-stub.md stood the handler up; this is NOT a reopen of it, because the recorded AWS behaviour for both calls below is itself a refusal and the emulator now refuses with the right code. What its fix left behind is the wrong wording.
- The OPEN issue 20261104T044753Z-timestream-influxdb-rebootdbinstance-on-a-deleted-db-instance.md carries the same check id for Cause 1, but it describes the OLD symptom (501/NotImplemented) and its Issue-Key is pinned to the `assert code == "ResourceNotFoundException"` line, which now passes. The failure has moved on to the following `assert message == ...` line, so that issue's key no longer matches the live divergence; this issue carries it.
- The OPEN issue 20261104T042504Z-timestream-influxdb-validationexception-constraint-violation-message-wording-diverges.md covers ValidationException wording for INPUT-CONSTRAINT violations. Cause 2 below is a RESOURCE-STATE refusal, a different message class produced by a different code path, so it is not covered there.

Cause 1 of 2: RebootDbInstance aimed at an id whose DB instance has been deleted is refused with the correct 404/ResourceNotFoundException but with the emulator's own invented message instead of AWS's. Failing check id: timestream-influxdb.create-db-instance-full-lifecycle.

AWS: Smithy (com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB) models RebootDbInstance with input member `identifier` (target DbInstanceIdentifier, required, @length min 3 max 64, @pattern ^[a-zA-Z0-9]+$), @idempotent, and errors [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]. ResourceNotFoundException is a @error("client") structure with required members message, resourceId, resourceType. The model does not state the message text, so: recorded behavior only — for a DB-instance lookup miss AWS answers HTTP 404, Error.Code "ResourceNotFoundException" and Error.Message "The DB instance with db id does not exist". That whole sentence is the error class's FIXED text: it interpolates no id, name, value or timestamp, so the requirement is the literal sentence and nothing is filled in from the request or from state.

Emulator: returns 404 / ResourceNotFoundException (correct) with Error.Message "DB instance <identifier> not found" — a different sentence that additionally interpolates the request's `identifier`, a value AWS's message for this class does not contain. Replay: AssertionError: DB instance 3a23a94f9d not found, raised by `assert message == "The DB instance with db id does not exist"`. Everything before it in the check passes (CreateDbInstance → CREATING, GetDbInstance polls to AVAILABLE with non-empty endpoint and influxAuthParametersSecretArn, DeleteDbInstance drives it to DELETED/absent).

Why: a client that matches on the message to distinguish a deleted DB instance from other 404-bearing lookup misses (teardown confirmation, reconciliation loops, log/alert matchers, recorded-fixture comparisons) does not match, and a client that parses the id out of the message gets an id AWS never puts there — so code written against AWS behaves differently against the emulator even though the code is right.

Fix: the RebootDbInstance operation handler's lookup-miss branch, in the handler (not the serializer): it must emit AWS's fixed sentence for this error class rather than composing one from the identifier. The same fixed text must be used wherever the service refuses a DB-instance lookup miss, so the message belongs in the shared not-found constructor the handler calls, not inlined per operation.

Traces: timestream-influxdb.create-db-instance-full-lifecycle (fails only at the final assertion, on the message).

Cause 2 of 2: RebootDbInstance on an instance that is still CREATING is refused with the correct 400/ValidationException but with a generic "wrong status" sentence instead of AWS's rebootable-state sentence. Failing check id: timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds.

AWS: same operation and modeled error set as above; ValidationException is a @error("client") structure (HTTP 400) with required members message and reason (enum ValidationExceptionReason). The model does not state the message text, so: recorded behavior only — a reboot requested while the instance is not in a rebootable state is refused with HTTP 400, Error.Code "ValidationException" and Error.Message "DbInstance with identifier <id> and customer id <accountId> is not in a rebootable state". The fixed text is "DbInstance with identifier " … " and customer id " … " is not in a rebootable state"; the only variable parts are <id>, the DB instance id from the request, and <accountId>, the 12-digit account id that owns it (the check derives it from field 4 of the instance ARN). Notably AWS does NOT name the current status or the required status in this message, and does not classify this as ConflictException — the state conflict surfaces as request validation.

Emulator: returns 400 / ValidationException (correct code) with Error.Message "The DbInstance with id [<id>] is in status [<status>] and must be in status [AVAILABLE] for the requested operation" — a different sentence built from a generic resource-not-in-status helper; it brackets the id, adds the current and required status, and omits the account id entirely. Replay: AssertionError: The DbInstance with id [85cf52ed7c] is in status [CREATING] and must be in status [AVAILABLE] for the requested operation, raised by the `assert message == f"DbInstance with identifier {instance_id} and customer id {account_id} is not in a rebootable state"` assertion.

Why: the message is the only part of this refusal that tells a client WHY the reboot was rejected, and clients key retry-until-rebootable loops and operator-facing errors off it. Against AWS the sentence names the owning account and the rebootable-state rule; against the emulator it names a status pair instead, so matchers fail and the account id a client expects to echo is absent. Because this assertion aborts the check, the rest of the recorded behaviour it guards — that the refused reboot starts nothing (the instance stays CREATING/AVAILABLE, never REBOOTING) and that the identical reboot succeeds and returns to AVAILABLE once the instance is AVAILABLE — is left unverified and must be re-replayed with the fix.

Fix: the RebootDbInstance operation handler's state-guard branch: the not-available refusal must build this operation's own rebootable-state message (id from the request, account id from the stored instance's ownership/ARN) instead of delegating to the service's generic resource-not-in-status ValidationException helper, whose generic wording is right for other operations but not for a reboot. Validation-message construction, not the serializer.

Traces: timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds.

File/line citations for both causes: services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/reboot_db_instance.rs — line 223 composes the Cause 1 message with format!("DB instance {} not found", input.identifier); lines 239-246 produce the Cause 2 message via crate::state::resource_not_in_status(NAMED_DB_INSTANCE, &record.id, &record.status, STATUS_AVAILABLE).

## Evidence
Cause 1 — check timestream-influxdb.create-db-instance-full-lifecycle [mismatch]
  Expected (real AWS): RebootDbInstance(identifier=<id of a deleted instance>) -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "The DB instance with db id does not exist". The entire message is fixed text for this error class; no part of it comes from the request or from state.
  Actual (emulator): HTTP 404, Error.Code = "ResourceNotFoundException" (correct), Error.Message = "DB instance <identifier> not found" — wrong fixed text, plus an interpolated identifier AWS does not include.
  Replay output:
    File "check_timestream_influxdb_create_db_instance_full_lifecycle.py", line 69, in run
      assert message == "The DB instance with db id does not exist", message
    AssertionError: DB instance 3a23a94f9d not found
  (The instance id in that line is a per-run generated value and is not part of the requirement.)
  Steps before the failure all passed: CreateDbInstance -> CREATING; GetDbInstance -> AVAILABLE with non-empty endpoint and influxAuthParametersSecretArn; DeleteDbInstance -> DELETED/absent. The earlier assertion `assert code == "ResourceNotFoundException"` now PASSES, which is why open issue 20261104T044753Z (keyed on that line, symptom 501/NotImplemented) no longer matches this divergence.

Cause 2 — check timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds [mismatch]
  Expected (real AWS): RebootDbInstance(identifier=<id>) while that instance is CREATING -> HTTP 400, Error.Code = "ValidationException", Error.Message = "DbInstance with identifier <id> and customer id <accountId> is not in a rebootable state".
    Fixed text: "DbInstance with identifier " + <id> + " and customer id " + <accountId> + " is not in a rebootable state".
    Variable parts: <id> = the identifier sent in the request; <accountId> = the 12-digit owning account id (field 4 of the instance's arn).
    AWS does not include the current status, the required status, or square brackets, and does not use ConflictException for this.
  Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = "The DbInstance with id [<id>] is in status [<status>] and must be in status [AVAILABLE] for the requested operation".
  Replay output:
    File "check_timestream_influxdb_reboot_db_instance_conflict_while_creating_then_succeeds.py", line 42, in run
      assert message == (
    AssertionError: The DbInstance with id [85cf52ed7c] is in status [CREATING] and must be in status [AVAILABLE] for the requested operation
  (85cf52ed7c is a per-run generated id, not part of the requirement.)
  Unverified because the check aborts here: that the refused reboot mutates nothing (status stays CREATING/AVAILABLE, never REBOOTING), and that the identical reboot succeeds once the instance is AVAILABLE and settles back to AVAILABLE.

Source of both messages — services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/reboot_db_instance.rs:
  223:  message: format!("DB instance {} not found", input.identifier),
  240-245: let refusal = st::resource_not_in_status(st::NAMED_DB_INSTANCE, &record.id, &record.status, st::STATUS_AVAILABLE);
  246:  return Err(RebootDbInstanceErrorModeled::ValidationException(refusal).into());

Model citation (shared by both causes): RebootDbInstance input `identifier` required, @length 3..64, @pattern ^[a-zA-Z0-9]+$, @idempotent; errors = [AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]; ResourceNotFoundException requires message+resourceId+resourceType; ValidationException requires message+reason. The model fixes the codes and statuses (both of which the emulator now gets right) but not the message text, so the message requirements above are from the recordings.

## Steps to Reproduce
Cause 1 (deleted-id not-found message):
1. Boot the timestream-influxdb emulator.
2. CreateDbInstance(name=<unique>, password=<random>, dbInstanceType=db.influx.medium, vpcSubnetIds=[<two subnets>], vpcSecurityGroupIds=[<one sg>], allocatedStorage=20, dbStorageType=InfluxIOIncludedT1, deploymentType=SINGLE_AZ). Keep the returned `id`.
3. Poll GetDbInstance(identifier=<id>) until status == AVAILABLE.
4. DeleteDbInstance(identifier=<id>) and poll GetDbInstance until it returns status DELETED or refuses with ResourceNotFoundException.
5. RebootDbInstance(identifier=<id>) on that dead id.
6. Observe HTTP 404 / ResourceNotFoundException with Error.Message "DB instance <id> not found". Real AWS answers the same status and code with Error.Message "The DB instance with db id does not exist".
7. Equivalent: replay timestream-influxdb.create-db-instance-full-lifecycle.

Cause 2 (not-rebootable-state message):
1. Boot the timestream-influxdb emulator.
2. CreateDbInstance with the same arguments as above. Keep `id` and `arn`; derive <accountId> from field 4 of `arn`.
3. Immediately — while GetDbInstance(identifier=<id>) still reports CREATING — call RebootDbInstance(identifier=<id>).
4. Observe HTTP 400 / ValidationException with Error.Message "The DbInstance with id [<id>] is in status [CREATING] and must be in status [AVAILABLE] for the requested operation". Real AWS answers the same status and code with Error.Message "DbInstance with identifier <id> and customer id <accountId> is not in a rebootable state".
5. After the fix, also confirm the steps the assertion currently blocks: GetDbInstance still reports CREATING or AVAILABLE (never REBOOTING) after the refusal; then poll to AVAILABLE, call RebootDbInstance again, and confirm it succeeds and the instance settles back to AVAILABLE.
6. Equivalent: replay timestream-influxdb.reboot-db-instance-conflict-while-creating-then-succeeds.
