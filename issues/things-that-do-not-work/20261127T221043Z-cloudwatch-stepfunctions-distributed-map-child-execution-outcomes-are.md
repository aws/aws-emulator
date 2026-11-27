Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.distributed-map-child-outcomes-stay-off-parent-execution-metrics
Issue-Key: cloudwatch__stepfunctions.distributed-map-child-outcomes-stay-off-parent-execution-metrics|run#15|AssertionError|assert parent == {"ExecutionsStarted": 2, "ExecutionsSucceeded": 2, "ExecutionsFailed": 0}, (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# cloudwatch__stepfunctions: Distributed Map child-execution outcomes are counted into the parent state machine's own AWS/States metrics

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the AWS/States ExecutionsStarted/ExecutionsSucceeded/ExecutionsFailed series dimensioned by the parent's StateMachineArn also counts the child executions a Distributed Map starts.
AWS: a Distributed Map's child executions are separate executions and their outcomes are published under the Map Run's own dimensions, not under the parent state machine's StateMachineArn dimension. A parent StateMachineArn series therefore counts only the parent runs — two parent runs give ExecutionsStarted 2 and ExecutionsSucceeded/Failed summing to 2. Recorded behavior only (the metric dimensions are not in the Smithy model).
Emulator: for two parent runs the parent's StateMachineArn series reports {'ExecutionsStarted': 2.0, 'ExecutionsSucceeded': 7.0, 'ExecutionsFailed': 1.0} — the per-item child executions are folded into the parent's counts, so Succeeded+Failed (8) far exceeds Started (2).
Why: a client alarming or dashboarding on a state machine's execution counts reads inflated, internally inconsistent numbers as soon as the machine contains a Distributed Map; error-rate alarms built from Failed/Started are wrong.
Fix: the metric-emission path for execution lifecycle transitions in the Step Functions state/execution layer — child executions of a Map Run must not be attributed to the parent's StateMachineArn dimension.
Traces: cloudwatch__stepfunctions.distributed-map-child-outcomes-stay-off-parent-execution-metrics.

## Evidence
Expected: the parent's own StateMachineArn series counts only the two parent runs.
Actual: {'ExecutionsStarted': 2.0, 'ExecutionsSucceeded': 7.0, 'ExecutionsFailed': 1.0}.

## Steps to Reproduce
1. Create a STANDARD state machine containing a Distributed Map over several items.
2. Start two executions of it (one succeeding, one failing).
3. Read AWS/States ExecutionsStarted/ExecutionsSucceeded/ExecutionsFailed dimensioned by the parent StateMachineArn and observe the child executions included.
4. Or replay the check id in trace_ids.
