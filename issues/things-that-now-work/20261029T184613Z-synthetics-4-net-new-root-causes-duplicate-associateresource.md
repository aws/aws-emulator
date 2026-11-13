Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.associate-resource-duplicate-association-conflict-rejected synthetics.concurrent-associate-resource-single-winner synthetics.stop-canary-when-not-running-conflict-rejected synthetics.delete-canary-while-running-conflict-rejected synthetics.canary-run-fails-without-execution-role-permissions synthetics.start-canary-transitions-running-then-stopped
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.associate-resource-duplicate-association-conflict-rejected|_client_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: synthetics.concurrent-associate-resource-single-winner|run#18|AssertionError|assert len(successes) == 1, f"expected exactly one winning AssociateResource, got {results}"
Issue-Key: synthetics.stop-canary-when-not-running-conflict-rejected|run#13|AssertionError|assert error["Message"] == expected_message, f"expected message {expected_message!r}, got {error['Message']!r}"
Issue-Key: synthetics.delete-canary-while-running-conflict-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: synthetics.canary-run-fails-without-execution-role-permissions|run#22|AssertionError|assert good_run_state == "PASSED", (
Issue-Key: synthetics.start-canary-transitions-running-then-stopped|_wait_for_canary_state#10|Timeout|time.sleep(interval_s)
Issue-Commit: f2009969ccef167db9c30e6e9450d638add1f0e3

# synthetics: 4 net-new root causes — duplicate AssociateResource accepted instead of ConflictException, state-conflict messages are emulator-authored, canary runs never execute (always EXECUTION_FAILURE), and a one-shot canary never leaves RUNNING

> _Note: this issue was identified against the code base at commit `f2009969ccef167db9c30e6e9450d638add1f0e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResource

## Description
Four distinct root causes in the emulated CloudWatch Synthetics service, all grounded in failing behavioral checks of the `synthetics` suite. Each cause is numbered with its own failing check ids; all four must be closed before this issue is resubmitted.

=== Cause 1 of 4: AssociateResource treats a duplicate association as idempotent success instead of refusing it with ConflictException ===
AWS: Smithy model (services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json) binds `AssociateResource` (PATCH /group/{GroupIdentifier}/associate) to error `ConflictException` (httpError 409, errorKind client, documented "A conflicting operation is already in progress."). Recorded behavior: associating a canary ARN that is ALREADY a member of the group is refused with HTTP 409 `ConflictException` and the message `Canary <canaryArn> already exists in group <groupName>` — fixed text is "Canary " / " already exists in group "; the variable parts are the `ResourceArn` from the request and the group's name resolved from `GroupIdentifier`. The group's membership stays a set: exactly one entry for that canary; after DisassociateResource the canary can be associated again. Under a race of N concurrent AssociateResource calls for the same (group, canary) pair, exactly one call succeeds and the other N-1 are refused with ConflictException.
Emulator: the duplicate call SUCCEEDS with HTTP 200 and an empty body. services/service-com-amazonaws-synthetics-synthetics/src/ops/associate_resource.rs deliberately returns `Ok(AssociateResourceOutput {})` when `already_associated` is true ("Membership is a set: a canary already in the group is already associated, so repeating the call succeeds and changes nothing"), so no ConflictException is ever produced by this operation, and in the 6-way race all six callers returned success (`[None, None, None, None, None, None]`).
Why: a client that relies on AssociateResource to serialize group membership (leader election / "first writer wins" semantics, or simply detecting that it is re-adding a canary) sees every caller win. Error-handling paths for ConflictException are never exercised, and code that treats a successful associate as proof it was the one that created the membership draws the wrong conclusion.
Fix: the duplicate-membership branch of the `AssociateResource` handler — it must return the modeled `ConflictException` (409) with the message above instead of early-returning Ok; the already-held store lock is the correct serialization point for the concurrent case.
Traces: synthetics.associate-resource-duplicate-association-conflict-rejected, synthetics.concurrent-associate-resource-single-winner.

=== Cause 2 of 4: state-transition ConflictException messages are emulator-authored prose instead of AWS's text for that error class ===
AWS: `StopCanary` and `DeleteCanary` both model `ConflictException` (409). Recorded behavior: a wrong-status transition is refused with a message of the fixed form `Canary is in a state that can't be stopped: <STATE>` for StopCanary and `Canary is in a state that can't be deleted: <STATE>` for DeleteCanary — the fixed text is "Canary is in a state that can't be stopped: " / "Canary is in a state that can't be deleted: ", and the single variable part is the canary's current `CanaryState` wire value taken from the emulator's state (e.g. READY for a never-started canary, RUNNING for a recurring canary). The canary NAME does not appear in the message.
Emulator: the error code and status are right but the text is rewritten and additionally leaks the canary name: StopCanary on a READY canary answers `Canary <name> cannot be stopped while it is in state READY` (services/.../src/ops/stop_canary.rs, the `canary.state != CanaryState::Running` branch) and DeleteCanary on a RUNNING canary answers `Canary <name> cannot be deleted while it is in state RUNNING`. StartCanary (services/.../src/ops/start_canary.rs) composes the same non-AWS template ("Canary <name> cannot be started while it is in state <STATE>") and must be corrected with them.
Why: clients and test suites that match on the refusal message (or that parse the state out of it) do not recognise the refusal; the message also surfaces an identifier AWS deliberately leaves out of this class of message.
Fix: the ConflictException construction in each state-transition handler's validation (stop_canary.rs, delete_canary.rs, start_canary.rs), ideally via one shared message helper in src/state.rs next to `CANARY_NOT_FOUND` so the three operations cannot drift.
Traces: synthetics.stop-canary-when-not-running-conflict-rejected, synthetics.delete-canary-while-running-conflict-rejected.

=== Cause 3 of 4: every canary run ends FAILED/EXECUTION_FAILURE and no artifact is ever produced, so a correctly-permissioned canary can never PASS ===
AWS: `GetCanaryRuns` returns `CanaryRun$Status` with `State` in the modeled `CanaryRunState` enum (RUNNING | PASSED | FAILED). Recorded behavior: a canary whose script is a trivial successful handler and whose execution role grants the S3/logs permissions it needs completes with run `Status.State = PASSED` (no StateReason/StateReasonCode) and uploads an artifact under the configured `ArtifactS3Location`; only a run that genuinely could not execute (e.g. a role granting nothing) is FAILED with `StateReasonCode = EXECUTION_FAILURE`.
Emulator: the outcome is hard-coded to failure independent of the canary, its script or its role. src/state.rs defines `RUN_NOT_EXECUTED_REASON_CODE = "EXECUTION_FAILURE"` and `RUN_NOT_EXECUTED_REASON = "This emulator has no canary runtime, so the canary script was not executed and the run produced no result."`, and both `StoredCanary::settle` and `StoredCanary::close_open_run` set every run to `CanaryRunState::Failed` with that reason. The check observed `State='FAILED' (StateReasonCode='EXECUTION_FAILURE', StateReason='This emulator has no canary runtime, …')` for the well-permissioned canary. Nothing is written to the artifact bucket for any run either.
Why: PASSED is unreachable, so the whole success side of the Synthetics data plane cannot be emulated: alarms/dashboards built on SuccessPercent, pass/fail branching in automation, and any check that distinguishes a failing canary from a passing one all see uniform failure. Worse, the failure reason-code is the same one a genuinely broken canary gets, so a client cannot tell a real EXECUTION_FAILURE (missing role permissions) from the emulator's blanket one — the negative case stops being distinguishable from the positive.
Fix: the run-outcome model in src/state.rs (`settle` / `close_open_run` and the RUN_NOT_EXECUTED_* constants) plus whatever StartCanary records for a run: the emulator must derive a run verdict — PASSED for a canary whose configuration/role is sound, FAILED/EXECUTION_FAILURE only when the modeled preconditions (e.g. role without S3/logs permissions) are not met — and must materialise the run artifact at the canary's ArtifactS3Location on a passing run.
Traces: synthetics.canary-run-fails-without-execution-role-permissions.

=== Cause 4 of 4: a one-shot (rate(0 minute)) canary never leaves RUNNING, so it never settles to STOPPED after its single run ===
AWS: `StartCanary` drives `CanaryState` READY -> STARTING/RUNNING and, for a run-once schedule (`rate(0 minute)`), the canary settles back to STOPPED on its own once that single run finishes; at least one run record is then readable through GetCanaryRuns. Recorded behavior only for the exact dwell times.
Emulator: StartCanary (src/ops/start_canary.rs) sets `canary.state = CanaryState::Running` and schedules only the RUN's outcome (`outcome_due = now + RUN_SETTLE`); `StoredCanary::settle` in src/state.rs materialises the CREATING->READY transition and the run outcome but never transitions the canary itself out of `Running`. Only an explicit StopCanary moves it to STOPPED. A client polling GetCanary for {STOPPED, ERROR} after a one-shot start therefore waits forever — the check hung and was killed (`Check timed out after 60.0s`).
Why: the one-shot canary lifecycle cannot complete. Any workflow that starts a run-once canary and waits for it to finish (the standard way to execute a canary on demand) blocks indefinitely instead of observing STOPPED, and the schedule's `DurationInSeconds`/rate(0) semantics are unobservable.
Fix: the canary lifecycle clock in src/state.rs (`StoredCanary::settle`, alongside `ready_due`/`outcome_due`) together with StartCanary: a started canary with a run-once schedule must record a due instant at which it transitions RUNNING -> STOPPED once its single run has completed, and recurring schedules must keep holding RUNNING until StopCanary.
Traces: synthetics.start-canary-transitions-running-then-stopped.

## Evidence
Suite: simparity-behavioral-aws-v1, synthetics 10/23 passed (43%).

Cause 1:
- synthetics.associate-resource-duplicate-association-conflict-rejected [mismatch]: `AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. Expected: Code=ConflictException, Message="Canary <canaryArn> already exists in group <groupName>".
- synthetics.concurrent-associate-resource-single-winner [mismatch]: `AssertionError: expected exactly one winning AssociateResource, got [None, None, None, None, None, None]` (all 6 racers succeeded; expected 1 success + 5 ConflictException).
- Source: src/ops/associate_resource.rs lines 171-177 return Ok when `already_associated`.

Cause 2:
- synthetics.stop-canary-when-not-running-conflict-rejected [mismatch]: `expected message "Canary is in a state that can't be stopped: READY", got 'Canary sim2real-canary-5530d0582f5b cannot be stopped while it is in state READY'` (code ConflictException was correct).
- synthetics.delete-canary-while-running-conflict-rejected [mismatch]: `expected message "Canary is in a state that can't be deleted: RUNNING", got 'Canary sim2real-canary-966a1c98b103 cannot be deleted while it is in state RUNNING'`.
- Source: src/ops/stop_canary.rs lines 161-170; src/ops/start_canary.rs lines 165-174 carry the same template.
(The canary names above are per-run values and are NOT part of the requirement; the requirement is that the name does not appear at all and the state token comes from the record.)

Cause 3:
- synthetics.canary-run-fails-without-execution-role-permissions [mismatch]: `AssertionError: expected the correctly-permissioned canary to PASS, got 'FAILED' (StateReasonCode='EXECUTION_FAILURE', StateReason='This emulator has no canary runtime, so the canary script was not executed and the run produced no result.')`.
- Source: src/state.rs RUN_NOT_EXECUTED_REASON / RUN_NOT_EXECUTED_REASON_CODE (lines ~177-184), applied unconditionally in `StoredCanary::settle` (lines ~494-506) and `close_open_run` (lines ~551-561).

Cause 4:
- synthetics.start-canary-transitions-running-then-stopped [mismatch]: `Error: Check timed out after 60.0s` while polling GetCanary for {STOPPED, ERROR} after StartCanary on a `rate(0 minute)` canary.
- Source: src/ops/start_canary.rs sets state=Running and only `outcome_due` for the run; src/state.rs `settle` has no transition out of `CanaryState::Running`.

## Steps to Reproduce
Cause 1: CreateGroup; CreateCanary (wait READY); AssociateResource(GroupIdentifier=group, ResourceArn=canaryArn); call AssociateResource again with the same arguments -> emulator answers 200; AWS answers 409 ConflictException "Canary <canaryArn> already exists in group <groupName>". Then run 6 AssociateResource calls concurrently for the same pair -> all 6 succeed; AWS lets exactly one win.

Cause 2: CreateCanary and wait for READY, then StopCanary(Name) -> ConflictException with message 'Canary <name> cannot be stopped while it is in state READY' instead of "Canary is in a state that can't be stopped: READY". Separately, create a canary with schedule rate(1 minute), StartCanary, wait for RUNNING, DeleteCanary(Name) -> message 'Canary <name> cannot be deleted while it is in state RUNNING' instead of "Canary is in a state that can't be deleted: RUNNING".

Cause 3: create an S3 artifact bucket and an execution role that grants the canary's S3/logs permissions, CreateCanary with a trivially-passing handler and schedule rate(0 minute), wait READY, StartCanary, poll GetCanaryRuns until the run is terminal -> Status.State='FAILED', StateReasonCode='EXECUTION_FAILURE'; AWS reports PASSED and an uploaded artifact in the bucket.

Cause 4: CreateCanary with schedule rate(0 minute), wait READY, StartCanary, then poll GetCanary(Name).Canary.Status.State -> it stays RUNNING indefinitely; AWS settles it to STOPPED once the single run finishes.
