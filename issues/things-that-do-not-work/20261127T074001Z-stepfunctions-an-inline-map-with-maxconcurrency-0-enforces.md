Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap|run#8|AssertionError|assert 40 <= many_count <= 60, (
Issue-Commit: 33932bf6f81e8cc72c4071b6c64638fb9bd78d71

# stepfunctions: an inline Map with MaxConcurrency 0 enforces no internal concurrency cap — 150 items all run in flight at once

> _Note: this issue was identified against the code base at commit `33932bf6f81e8cc72c4071b6c64638fb9bd78d71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause in the Step Functions interpreter's Map concurrency scheduler, reached through CreateStateMachine + StartExecution + GetExecutionHistory on a plain STANDARD state machine. Failing check id: stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap.

Cause 1 of 1: with MaxConcurrency 0 (unbounded) and more items than the service's internal concurrency cap, the emulator starts every item at once instead of plateauing at that cap.
AWS:      recorded behavior only — MaxConcurrency is an ASL definition field, not a member of the Smithy API model (Smithy com.amazonaws.sfn#GetExecutionHistory returns the modeled HistoryEventList, from which in-flight iterations are counted as MapIterationStarted minus MapIterationSucceeded/MapIterationFailed/MapIterationAborted). The recording shows that MaxConcurrency 0 means UNBOUNDED only up to the service's OWN internal concurrency ceiling: over an inline ItemProcessor whose single state is a 20-second Wait, an item count well under that ceiling (10) has all 10 iterations in flight simultaneously, while a count well above it (150) plateaus at the internal cap — observed in the 40-60 band, i.e. the service never has all 150 iterations running at once. No API call here is refused; there is no error code or non-2xx status involved in this divergence — the divergence is in how many iterations the history shows as concurrently running.
Emulator: the positive control now passes (10 items under MaxConcurrency 0 plateau at 10 in flight — this is past the serialization defect that cause 2 of the OPEN issue 20261125T001009Z-stepfunctions-5-net-new-causes-credentials-assume-denial.md describes, where the plateau was 1 and the 150-item arm was never reached). With 150 items the emulator's plateaued in-flight count is 150: MapIterationStarted is emitted for every item immediately and no iteration has ended, so the scheduler applies no ceiling at all. That issue's recorded Issue-Key for this check is the few-items assertion (`assert few_count == _FEW_ITEMS`), so it does not carry this failure; the absence of an internal cap is a distinct cause from running items serially. The OPEN issue 20261123T063602Z-stepfunctions-2-net-new-causes-map-maxconcurrency-maxconcurrencypath.md covers out-of-range MaxConcurrency/MaxConcurrencyPath VALIDATION, also a different cause.
Why:      a client relying on AWS's behavior sees unbounded fan-out that AWS never produces: every downstream integration a large Map drives (Lambda invokes, SDK calls, writes) is issued at once instead of at the service's bounded concurrency, so throttling, ordering and aggregate-throughput behavior that depends on the cap cannot be observed, and load-shedding or quota behavior a workflow is designed around (ConcurrencyLimit-sensitive retries, downstream TooManyRequests) never appears. Conversely a test asserting that AWS bounds the fan-out of an unbounded Map passes against AWS and fails against the emulator.
Fix:      the Map state's concurrency scheduler in the interpreter (the related report named services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl/map_items.rs for the inline-Map item scheduling seam) — treat MaxConcurrency 0 as unbounded only up to a service-internal ceiling, admitting new iterations as earlier ones end rather than starting the whole item list, so the in-flight count plateaus at that ceiling for item counts above it and at the item count for counts below it.
Traces:   stepfunctions.map-maxconcurrency-zero-over-many-items-plateaus-at-an-internal-cap.

## Evidence
The check is listed under the suite report's ### stepfunctions heading, verdict [mismatch]. Role/machine names, ARNs and timestamps are per-run values and never part of the requirement; the item counts, MaxConcurrency value and Wait duration are the definition's.

Definition (STANDARD): {"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","MaxConcurrency":0,"ItemProcessor":{"ProcessorConfig":{"Mode":"INLINE"},"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":20,"End":true}}},"End":true}}}
In-flight iterations are measured from GetExecutionHistory: count(MapIterationStarted) - count(MapIterationSucceeded|MapIterationFailed|MapIterationAborted), polled every 2 s until the same positive value is read twice in a row.

Arm 1 (positive control, 10 items): expected plateau 10, actual plateau 10 — PASSES. This establishes that the plateau asserted in arm 2 is a real ceiling, not general throttling or dropped iterations.
Arm 2 (150 items): expected the plateau to fall at the service's internal concurrency cap (observed band 40-60), actual plateau 150 — every one of the 150 iterations is in flight at the same time.
Failure text: check_stepfunctions_map_maxconcurrency_zero_over_many_items_plateaus_at_an_internal_cap.py line 151, `assert 40 <= many_count <= 60` — AssertionError: "with 150 items and MaxConcurrency 0, concurrently running iterations must plateau between 40 and 60 (the service's internal concurrency cap); the plateaued in-flight count was 150".
No refusal is involved: StartExecution and GetExecutionHistory all answer 2xx.

## Steps to Reproduce
1. CreateRole an execution role trusting states.amazonaws.com (sts:AssumeRole).
2. CreateStateMachine (type STANDARD) with definition {"StartAt":"M","States":{"M":{"Type":"Map","ItemsPath":"$.items","MaxConcurrency":0,"ItemProcessor":{"ProcessorConfig":{"Mode":"INLINE"},"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":20,"End":true}}},"End":true}}}.
3. StartExecution with input {"items":[0..9]} (10 items). Poll GetExecutionHistory (following nextToken) and compute MapIterationStarted minus the ended iterations until the value is stable.
   Expected and actual: the plateau is 10 (control passes).
4. StartExecution on the same machine with input {"items":[0..149]} (150 items) and poll the same way.
   Expected: the plateau sits at the service's internal concurrency cap — far below 150 (recorded in the 40-60 band), with further iterations starting only as earlier ones end.
   Actual: the plateau is 150 — all iterations are started immediately and none has ended, so no ceiling is applied.
5. StopExecution both executions to clean up.
