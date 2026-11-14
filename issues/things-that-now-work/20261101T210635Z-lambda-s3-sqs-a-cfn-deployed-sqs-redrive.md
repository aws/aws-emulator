Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive
Issue-Key: lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive|_poll_until#8|Timeout|last = probe()
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# lambda__s3__sqs: a CFN-deployed SQS redrive policy never moves a repeatedly-failing message to its DLQ

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the deployed queue/DLQ redrive path produces no dead letter when the consuming function keeps failing.

AWS: recorded behavior only (the SQS model carries the RedrivePolicy queue attribute and Lambda's carries the event-source mapping, but neither states the redrive outcome). AWS behaviour exercised by the check: with a stack that provisions a source queue whose RedrivePolicy names a DLQ with maxReceiveCount N and a Lambda event-source mapping whose handler always raises, the message is redelivered up to N times and then moved to the DLQ — the DLQ receives exactly that message and the source queue drains.

Emulator: the check never reaches that verdict — `Error: Check timed out after 60.0s`. The DLQ never receives the message (the mapping does not invoke/redeliver), so the redrive policy's effect is unobservable.

Why: the redrive policy is the standard way a client quarantines poison messages. With nothing ever landing in the DLQ, a failed message is indistinguishable from one that was processed, operators lose the only durable copy of it, and alarms on DLQ depth never fire.

Fix: the SQS event-source-mapping poller (invoke, count receives on failure) and the queue's RedrivePolicy enforcement that must move a message to the named DLQ once maxReceiveCount is exceeded. Related open issues 20261029T072224Z-lambda-s3-sqs-an-sqs-event-source-mapping.md and 20261030T110325Z-lambda-s3-sqs-createeventsourcemapping-s-unauthorized-execution-role.md cover neighbouring defects of this group but do not carry this check.

Traces: lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive.

## Evidence
Replay verdict [mismatch] for lambda__s3__sqs.deploy-sqs-dlq-redrive-policy-lambda-failure-positive; detail: `Error: Check timed out after 60.0s`. Expected: after maxReceiveCount failed receives the message is in the DLQ and the source queue is empty. Actual: the DLQ stays empty for the whole 60s budget.

## Steps to Reproduce
1. Deploy a stack with a source queue (RedrivePolicy -> DLQ, maxReceiveCount=2), a DLQ, and a Lambda event-source mapping whose handler always raises.
2. Send one message to the source queue.
3. Poll the DLQ: AWS places the message there after the receive count is exceeded.
4. Against the emulator the DLQ never receives it; the check is killed at the 60s harness budget.
