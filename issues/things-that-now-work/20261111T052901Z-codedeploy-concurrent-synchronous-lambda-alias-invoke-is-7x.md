Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.lambda-canary-diverts-live-invokes codedeploy.alarm-rollback-returns-live-traffic-to-prior-version
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.lambda-canary-diverts-live-invokes|wait_for_observations#16|AssertionError|assert got >= n, (
Issue-Key: codedeploy.alarm-rollback-returns-live-traffic-to-prior-version|wait_for_observations#21|AssertionError|assert got >= n, (
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# codedeploy: concurrent synchronous Lambda alias Invoke is ~7x slower than AWS (~2.6 s/call, ~2.7 invokes/s across 8 callers), so live-traffic checks cannot bank a measurement window

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's synchronous Lambda alias-invoke data plane serves concurrent invokers at roughly one call per 2.6 s per caller, so a steady requester cannot produce traffic at anything like real-AWS rates. Failing checks: codedeploy.lambda-canary-diverts-live-invokes, codedeploy.alarm-rollback-returns-live-traffic-to-prior-version.

AWS: recorded behavior only — neither the Lambda nor the CodeDeploy Smithy model constrains invoke latency or concurrency (Invoke is a non-paginated synchronous data-plane op; Lambda's RequestResponse invocation of an already-warm alias returns in tens of milliseconds and serves concurrent callers in parallel up to the account's concurrency limit). The recordings these checks were built from measured exactly that: 8 concurrent workers each pausing 0.4 s between calls sustain ~20 invokes/s (500 observations in ~25 s), a 200-invoke sample completes in well under a second of wall clock, and a weight written to a live alias is honoured by the invoke path ~0.83 s later. Both checks drive the alias of a trivial handler that only returns a constant marker, so there is no handler work to account for the gap.

Emulator: the same 8-worker requester against the emulator produced 485 observations (lambda-canary-diverts-live-invokes) and 490 observations (alarm-rollback-returns-live-traffic-to-prior-version) in 180 s — ~2.7 observations/s instead of ~20/s. Each observation is appended unconditionally after every invoke attempt, success or failure (the requester records a failure tuple for a ClientError, a non-200 StatusCode or a FunctionError, and the runs reported no such failures), so the shortfall is wall-clock latency, not dropped or refused calls. Subtracting the fixed 0.4 s per-worker pause leaves ~2.6 s inside each Invoke call. Hypothesis: concurrent alias invokes are being serialized behind a single lock or a single worker in the invoke path (8 callers at ~0.37 s of serialized service time each would yield the same aggregate ~2.7/s), rather than each invoke genuinely costing 2.6 s.

Why: a client cannot observe the effect a CodeDeploy traffic shift is for. Every property that distinguishes a real weighted shift from a rewritten alias record — that a minority of live invokes reach the canary version while the shift is in flight, that all invokes reach the new version after cutover, and that an alarm-triggered rollback makes live invokes stop reaching the new version — is a statement about a population of invokes inside a time window. At ~2.7/s a window bounded by the shift's own clock pacing cannot hold enough invokes to assert anything, so the shift and the rollback are unverifiable through the data plane even when the control plane reports them. More generally, any client that fans out synchronous invokes (a load generator, a canary, a fan-out step in an orchestration) sees throughput two orders of magnitude off AWS and will time out on budgets calibrated against real Lambda.

Fix: the Lambda data-plane Invoke handler and its concurrency/dispatch path (the component that admits a RequestResponse invocation, resolves the alias and its weighted routing, and runs the handler) — concurrent invocations of the same alias must be served in parallel, and a warm trivial handler's round trip must not cost seconds. Nothing in the alias/weight state model needs to change for this; it is the admission and execution path.

Related but not covering this: open issue 20261109T135826Z-codedeploy-traffic-shifting-is-not-clock-paced-or.md owns the control-plane side of traffic shifting (pacing, weight routing, hook status, ALARM_ACTIVE) for other checks; it says nothing about invoke throughput, and these two checks fail before any shift is created (the pre-shift baseline window never fills).

## Evidence
codedeploy.lambda-canary-diverts-live-invokes — AssertionError at check line 126 -> wait_for_observations (line 388): "the requester produced only 485 observations within 180s, needing 500; at 8 workers x 0.4s it should manage ~20/s". The failure is in phase 1, the PRE-SHIFT baseline, before CreateDeployment is called.

codedeploy.alarm-rollback-returns-live-traffic-to-prior-version — AssertionError at check line 134 -> wait_for_observations (line 454): "the requester produced only 490 observations within 180s, needing 500; at 8 workers x 0.4s it should manage ~20/s". Same phase, same shortfall.

Expected (real AWS, per the recordings the checks encode): 8 workers x 0.4 s interval sustain ~20 invokes/s, so PRE_TARGET=500 observations are banked in ~25 s, well inside the 180 s budget.
Actual (emulator): ~2.7 invokes/s aggregate; 500 observations are not reached in 180 s. Derived per-call cost inside Invoke ~2.6 s after removing the 0.4 s pause. Fixed text of the assertion above is the harness's; the variable parts are the observation count reached (485 / 490), the target (500) and the timeout (180 s) — the requirement is the RATE, not those specific counts.

No invoke in either run was recorded as a ClientError, a non-200 StatusCode or a FunctionError, so the emulator answered every call correctly; it answered them too slowly.

## Steps to Reproduce
1. Create a Lambda function with a trivial handler (returns a constant JSON marker), publish a version and point an alias at it.
2. Start 8 concurrent threads, each looping: Invoke(FunctionName="<fn>:<alias>", Payload=b"{}"), record the completion, sleep 0.4 s.
3. Count completed invokes after 180 s. Real AWS: ~3600 (rate ~20/s); 500 are reached in ~25 s. Emulator: ~490 total (rate ~2.7/s), 500 never reached.
4. Equivalently, time a single Invoke of that warm alias while the other 7 threads are running: the emulator takes ~2.6 s, real Lambda tens of milliseconds.
5. Replay codedeploy.lambda-canary-diverts-live-invokes and codedeploy.alarm-rollback-returns-live-traffic-to-prior-version; both abort in their pre-shift baseline.
