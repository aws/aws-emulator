Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.start-canary-dry-run-while-canary-updating-rejected synthetics.start-canary-dry-run-while-dry-run-in-progress-rejected
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.start-canary-dry-run-while-canary-updating-rejected|run#12|AssertionError|assert (code, message) == (
Issue-Key: synthetics.start-canary-dry-run-while-dry-run-in-progress-rejected|_error_of#10|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {got!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# synthetics: 2 net-new root causes — StartCanaryDryRun refuses a busy canary with ConflictException instead of AWS's ValidationException, and accepts a second dry run while one is still in flight

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCanaryDryRun

## Description
Two net-new root causes in the Synthetics StartCanaryDryRun handler (services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary_dry_run.rs). Both concern the state preconditions that gate starting a dry run. Neither is covered by the open synthetics issues (20261030T142023Z-synthetics-describeruntimeversions-... and 20261103T072339Z-synthetics-disassociateresource-...), which are about DescribeRuntimeVersions and DisassociateResource.

Cause 1 of 2: StartCanaryDryRun on a canary whose own operation is in flight is refused with the wrong error class and message.
AWS: recorded behavior only — the Smithy model declares both ValidationException (HTTP 400) and ConflictException (HTTP 409) for StartCanaryDryRun and documents neither case specifically (Smithy: StartCanaryDryRun errors = AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400). The recording shows that while the canary is being updated (UpdateCanary accepted, canary state UPDATING), AWS refuses the dry run with HTTP 400, error code `ValidationException`, message `Cannot dryrun canary as canary is not in a valid state.` That message is entirely fixed text — it names neither the canary nor its state, so no part of it is taken from the request or from state. The refusal starts nothing: GetCanaryRuns(RunType=DRY_RUN) still lists no runs afterwards, and the update stays applied.
Emulator: answers HTTP 409, code `ConflictException`, message `Canary is in a state that can't be dry run: UPDATING` — a different error class, a different status, and a message built from the canary's current state, which AWS's message never mentions. In the handler this is the `canary.state.operation_in_progress()` branch that constructs `ConflictException { message: canary_state_conflict(CanaryTransition::DryRun, state) }` (both the pre-work branch and the re-check after the run work).
Why: a client that distinguishes retryable conflicts (409 ConflictException) from invalid-state rejections (400 ValidationException) — including botocore `except synthetics.exceptions.ValidationException` handlers and safe-upgrade wrappers that match AWS's message — never sees the refusal AWS produces. It will either retry a call AWS classifies as a validation failure, or crash on an unexpected exception class.
Fix: the StartCanaryDryRun handler's state precondition in the Synthetics service crate: the busy-canary branch must raise the modeled ValidationException with AWS's fixed message instead of ConflictException with a state-derived message.
Traces: synthetics.start-canary-dry-run-while-canary-updating-rejected.

Cause 2 of 2: a second StartCanaryDryRun is accepted while the canary's previous dry run is still in flight.
AWS: recorded behavior only — a canary runs one dry run at a time. With one dry run in progress (first StartCanaryDryRun returned a DryRunId and its run is RUNNING), a second StartCanaryDryRun is refused with HTTP 400, error code `ValidationException`, message `Cannot dryrun canary as canary's existing dryrun is not in a valid state.` That message is entirely fixed text; no part of it comes from the request or state. The refusal starts nothing: GetCanaryRuns(RunType=DRY_RUN) records runs for exactly one DryRunId (the accepted first one), and the same call succeeds with a NEW DryRunId once the first dry run has ended.
Emulator: accepts the second call with HTTP 200 and returns a fresh `DryRunConfig` (`DryRunId` = a new uuid, `LastDryRunExecutionStatus` = RUNNING), so two concurrent dry runs exist for one canary. The handler checks only `canary.state.operation_in_progress()` for the canary itself and never inspects the canary's existing dry run's state before minting a new `dry_run_id` and calling `canary.begin_dry_run(...)`.
Why: a client relying on AWS's one-dry-run-at-a-time guarantee loses its serialization point. Code that catches the ValidationException to wait and retry never waits; GetCanaryRuns(RunType=DRY_RUN) then returns runs owned by more than one DryRunId, so result-attribution logic in safe-upgrade flows reads the wrong dry run's verdict.
Fix: the StartCanaryDryRun handler's state precondition / state model in the Synthetics service crate: before admitting a dry run, reject when the canary already holds a dry run that is not in a terminal state, with the modeled ValidationException and AWS's fixed message.
Traces: synthetics.start-canary-dry-run-while-dry-run-in-progress-rejected.

## Evidence
Cause 1 — check synthetics.start-canary-dry-run-while-canary-updating-rejected:
  Expected (AWS, recorded): HTTP 400 ('ValidationException', 'Cannot dryrun canary as canary is not in a valid state.')
  Actual (emulator): HTTP 409 ('ConflictException', "Canary is in a state that can't be dry run: UPDATING")
  Assertion: AssertionError: dry run on an updating canary answered ('ConflictException', "Canary is in a state that can't be dry run: UPDATING")

Cause 2 — check synthetics.start-canary-dry-run-while-dry-run-in-progress-rejected:
  Expected (AWS, recorded): HTTP 400 ('ValidationException', "Cannot dryrun canary as canary's existing dryrun is not in a valid state.")
  Actual (emulator): HTTP 200 with {'DryRunConfig': {'DryRunId': <new uuid>, 'LastDryRunExecutionStatus': 'RUNNING'}} — a second concurrent dry run was started.
  Assertion: AssertionError: expected a refusal, the call succeeded: {... 'HTTPStatusCode': 200 ... 'DryRunConfig': {'DryRunId': '<uuid>', 'LastDryRunExecutionStatus': 'RUNNING'}}
  (The DryRunId value and timestamps are per-run and not part of the requirement.)

Code: services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary_dry_run.rs lines 261-269 and 438-446 build ConflictException from canary_state_conflict(CanaryTransition::DryRun, state) for a busy canary; lines 371-447 mint a new DryRunId and call canary.begin_dry_run(...) with no check of any existing dry run's state.
Model: StartCanaryDryRun POST /canary/{Name}/dry-run/start, success 200, errors ValidationException(400) and ConflictException(409) both modeled.

## Steps to Reproduce
Cause 1:
1. Create a canary and wait until GetCanary reports Status.State == READY.
2. Call UpdateCanary(Name=<canary>, RunConfig={'TimeoutInSeconds': 45}) so the canary enters UPDATING.
3. Immediately call StartCanaryDryRun(Name=<canary>).
   Expect HTTP 400 ValidationException 'Cannot dryrun canary as canary is not in a valid state.'; the emulator answers HTTP 409 ConflictException "Canary is in a state that can't be dry run: UPDATING".
4. After the update settles, GetCanaryRuns(Name=<canary>, RunType='DRY_RUN') must still be empty.

Cause 2:
1. Create a canary and wait until READY.
2. Call StartCanaryDryRun(Name=<canary>) and keep the returned DryRunConfig.DryRunId; the dry run's run is RUNNING.
3. Call StartCanaryDryRun(Name=<canary>) again while the first dry run is still in flight.
   Expect HTTP 400 ValidationException "Cannot dryrun canary as canary's existing dryrun is not in a valid state."; the emulator answers HTTP 200 with a second DryRunId.
4. GetCanaryRuns(Name=<canary>, RunType='DRY_RUN') must record runs for exactly one DryRunId (the first); after the first dry run ends, StartCanaryDryRun must succeed with a different DryRunId.
