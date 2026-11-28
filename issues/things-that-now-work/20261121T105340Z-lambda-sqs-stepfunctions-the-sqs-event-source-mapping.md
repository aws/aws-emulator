Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative|run#11|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 77bfd47bd7799ecf9c5ff0eea8e18d7399a37670

# lambda__sqs__stepfunctions: the SQS event-source-mapping mismatched-queue-ARN scenario never settles — its enabled-mapping positive control is never delivered

> _Note: this issue was identified against the code base at commit `77bfd47bd7799ecf9c5ff0eea8e18d7399a37670`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the scenario that proves a Lambda SQS event-source mapping bound to one queue is NOT triggered by a message sent to a different queue makes no observable progress against the emulator: the enabled mapping's positive-control delivery never arrives, so the check is cut off at the harness's 60 s cap and reaches no verdict.

AWS: Smithy (lambda): `CreateEventSourceMapping` binds exactly one `EventSourceArn` to one `FunctionName`; `GetEventSourceMapping` reports `State`. Recorded behaviour: a mapping created with `Enabled=true` over queue A reaches `State: "Enabled"` and invokes the function for messages sent to A within seconds, while a message sent to an unrelated queue B is never delivered to that function and remains on B. The positive control (delivery from A) must be observed first, because an absence from B measured against a pipeline that delivers nothing proves nothing.

Emulator: nothing is delivered. The check never gets past waiting for the mapping's own positive-control delivery; the suite reports `Check timed out after 60.0s` with no assertion graded.

Why: a mapping that reports a healthy `Enabled` state and polls nothing makes every SQS-triggered function silently inert, and makes correct ARN scoping unverifiable — a client cannot tell "my ESM names the wrong queue" from "event-source mappings never deliver".

Fix: the Lambda event-source-mapping poller for SQS sources (receive from the mapping's own `EventSourceArn`, invoke the mapped function, delete on success, and deliver nothing for a queue the mapping does not name). The same underlying behaviour is filed under other service headings by open issues 20261118T062834Z-lambda-pipes-sqs-an-enabled-sqs-event-source.md and 20261118T150346Z-events-lambda-sqs-an-enabled-sqs-event-source-f5f960.md; neither carries this check's replay key.

Traces: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative.

## Evidence
Failing check `lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative` [mismatch]:

    Error: Check timed out after 60.0s

Expected (AWS): the mapping over the bound queue reaches State='Enabled' and delivers the positive-control message within seconds, after which a message sent to the unrelated queue is observed NOT to invoke the function and to remain on that queue.
Actual (emulator): no delivery is observed at all; the check makes no progress and is killed at the 60 s cap before any assertion is graded.

## Steps to Reproduce
1. Create two SQS queues A and B, and a python3.12 Lambda function that records each received record; wait for State=Active.
2. CreateEventSourceMapping(EventSourceArn=<A arn>, FunctionName=<fn>, BatchSize=1, Enabled=true); poll GetEventSourceMapping until State == 'Enabled'.
3. SendMessage a marker to A and poll the function's record destination. Expected: delivered within seconds. Observed: never — the check is killed at its 60 s cap.
4. (Blocked by 3) SendMessage a different marker to B and assert the function is never invoked with it and that B's ApproximateNumberOfMessages stays >= 1.

## Resolution

PARTIAL, and the recorded check is STILL FAILING: the replay of
`lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative` continues to
answer `Check timed out after 60.0s` with no assertion graded. This report is filed here
because the one way the emulator itself can produce the reported symptom was found and fixed,
and because the rule the check is about is now pinned by tests; it is NOT a claim that the
recorded check passes.

What was fixed: a mapping was polled only while the background task the create or the update
armed was alive, and nothing else armed one — so a mapping whose task ended, or that was
created where there was no runtime to spawn onto, went on reporting `State: "Enabled"` with
nothing reading its source and no later call could recover it. Arming now has one owner
(`crate::state::poller_supervision`), and the operations that REPORT a mapping —
`GetEventSourceMapping` and `ListEventSourceMappings` — make sure of the pollers the states
they report imply, for the mappings they reported and no others.

What remains open: the divergence the recording carries is the harness's 60 s cap over the
whole check rather than any answer the emulator gives, and it is not reproducible here. The
recorded shape — three queues, the execution role carrying the policies AWS publishes for a
queue-triggered function, `CreateFunction` and the wait for `Active`, the mapping and the wait
for `Enabled`, the message to the unnamed queue, the absence window and the positive delivery —
was driven end to end both in process and against the started binary (over TLS, with request
authentication, signed), and completes in a few seconds of emulator work per run, each delivery
inside a second; the two sibling shapes the same recorded file reports (a FIFO source with two
message groups, and the `Enabled` -> `Disabled` -> `Enabled` gate) deliver too. What spends the
cap in the replay environment is not observable from inside the emulator, which is why this
cause is recorded as partial rather than closed.

Tests pinning the rule:
* `services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/zz_repro_mismatched.rs`
* `core/router/tests/all/queue_mapping_delivers_only_from_the_queue_it_names.rs`
