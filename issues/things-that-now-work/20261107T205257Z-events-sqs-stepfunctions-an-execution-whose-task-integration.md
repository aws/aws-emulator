Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change|run#15|AssertionError|assert failed is not None, (
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events__sqs__stepfunctions: an execution whose Task integration cannot be made is reported FAILED with error States.Timeout instead of States.Runtime, so the redrive status-change event never matches

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution / PutEvents

## Description
Cause 1 of 1: the error code on a failed execution's status-change event is States.Timeout where AWS reports States.Runtime.

AWS: when a state machine's Task state cannot complete its service integration call, the execution fails with error States.Runtime (States.Timeout is reserved for a state that exceeded its TimeoutSeconds/HeartbeatSeconds), and the "Step Functions Execution Status Change" event delivered to the bus carries that error in its detail, so a rule/consumer selecting the FAILED-with-States.Runtime case receives it.

Emulator: all four executions in the check are delivered as ('FAILED', 'States.Timeout') and no FAILED event with error States.Runtime is ever delivered within the 150 s window, so the redrive status-change contract cannot be observed. The error code string is the requirement here; the number of executions and the window are the check's own values.

Why: a consumer that branches on the execution's error code — alarms, redrive tooling, retry policies keyed to States.Runtime vs States.Timeout — takes the wrong branch, and a timeout is reported where no timeout occurred.

Fix: the Step Functions execution failure classification (the Task integration-dispatch failure path must set error States.Runtime with its cause) and the status-change event it publishes.

Traces: events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change.

## Evidence
Expected: within the window, a FAILED Step Functions execution status-change event whose error is States.Runtime for one of the executions.
Actual: AssertionError: within 150s the bus must deliver a FAILED event with error States.Runtime for one of 4 execution(s); delivered [('FAILED', 'States.Timeout'), ('FAILED', 'States.Timeout'), ('FAILED', 'States.Timeout'), ('FAILED', 'States.Timeout')].

## Steps to Reproduce
1. CreateStateMachine with a Task state whose service-integration call the emulator cannot make.
2. PutRule {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} with an SQS target.
3. StartExecution and let it fail; read the delivered envelopes' detail.status/detail.error.
