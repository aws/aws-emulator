Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions__xray.tracing-enabled-state-machine-publishes-a-trace-and-a-disabled-twin-does-not
Issue-Key: stepfunctions__xray.tracing-enabled-state-machine-publishes-a-trace-and-a-disabled-twin-does-not|_await_trace#14|Timeout|time.sleep(_XRAY_INTERVAL_S)
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# stepfunctions__xray: a tracingConfiguration.enabled state machine publishes no X-Ray trace, so the traced/untraced twin comparison never settles (1 cause)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: an execution of a state machine created with `tracingConfiguration.enabled=true` publishes no trace to X-Ray, so the trace is never retrievable and the check waits out its deadline instead of comparing the traced twin against the untraced one.

AWS: `tracingConfiguration.enabled` decides whether Step Functions publishes an execution's trace to X-Ray. With it enabled, the execution's trace id (the `Root=1-xxxxxxxx-xxxxxxxxxxxxxxxxxxxxxxxx` of the supplied or generated trace header, echoed back by StartSyncExecution) becomes retrievable at X-Ray — GetTraceSummaries over the execution's time window lists it and BatchGetTraces/GetTraceGraph return its segments; with it disabled, the twin execution's trace is NOT retrievable. Recorded behavior only for the segment shape; the requirement is the enabled/disabled asymmetry and that a supplied trace header with `Sampled=1` is honoured. Smithy: CreateStateMachine's tracingConfiguration and StartExecution's `traceHeader` (@length 0..256, pattern ^\p{ASCII}*$) are modelled in services/service-com-amazonaws-sfn-awsstepfunctions.

Emulator: the check reports only `Error: Check timed out after 60.0s`. The traced twin's trace never appears at X-Ray, so the positive control (a Sampled=1 header that must be retrievable) never resolves and the negative control is meaningless — tracingConfiguration has no observable effect at all.

Why: a client relying on AWS's behavior gets no distributed-tracing data from Step Functions. Enabling tracing on a state machine is a silent no-op: an operator cannot find an execution's trace, cannot see which states contributed segments, and cannot tell a sampling decision from a missing integration — and any dashboard or alarm built on Step Functions traces stays empty while the control plane reports tracing as enabled.

Fix: the trace-publication path of the sfn service crate's execution runtime and its X-Ray integration — when `tracingConfiguration.enabled` is true, honour the execution's trace header (or generate a Root id), emit the execution's segments to X-Ray so GetTraceSummaries/BatchGetTraces resolve them, and publish nothing when tracing is disabled. The open X-Ray issues (e.g. 20261025T043615Z-xray-3-net-new-behavioral-root-causes-getservicegraph.md) concern X-Ray's own surfaces and do not cover the Step Functions publisher or carry this check's replay key.

Traces: stepfunctions__xray.tracing-enabled-state-machine-publishes-a-trace-and-a-disabled-twin-does-not.

## Evidence
Expected (AWS): two state machines identical but for `tracingConfiguration.enabled`; an execution of the traced one (started with a trace header carrying `Sampled=1`, echoed back by StartSyncExecution) has its `Root=1-<8 hex>-<24 hex>` trace retrievable at X-Ray within the polling window, while the untraced twin's is not.

Actual (emulator): `Error: Check timed out after 60.0s` — the traced twin's trace is never retrievable, so neither the positive nor the negative arm produces a verdict.

## Steps to Reproduce
1. Create one IAM role trusted by states.amazonaws.com.
2. CreateStateMachine twice with that role, identical definitions, differing only in tracingConfiguration.enabled (true / false).
3. Start an execution of each, supplying a trace header with Sampled=1 and capturing the echoed Root= id.
4. Poll X-Ray (GetTraceSummaries over the execution window, then BatchGetTraces) for each Root id.
5. Expected: the traced twin's trace is found, the untraced twin's is not. Observed: neither is ever found; the check runs out its deadline.
