Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload
Issue-Key: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload|run#21|AssertionError|raise _expired(ctx, "a dead letter on the pipe's DLQ", dlq_wait_s, "the DLQ stayed empty")
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# kinesis__lambda__pipes__sqs: an enrichment failure on a Kinesis-source pipe never dead-letters to the pipe's SQS DLQ

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: records whose Lambda enrichment fails are never written to the pipe's configured dead-letter queue.
AWS: when a pipe's Enrichment invocation fails and the Kinesis source parameters configure a DeadLetterConfig (plus MaximumRetryAttempts), the batch is retried to the configured limit and then dead-lettered: a message is sent to the configured SQS queue whose body is the documented Pipes dead-letter payload (the failure context — the pipe's identity, the failed stage, the Kinesis shard/sequence-number range of the discarded batch — not the record payload itself). The DLQ is non-empty well inside the recorded window.
Emulator: after the enrichment is driven to failure, the DLQ stays empty for the full 240 s wait ("a dead letter on the pipe's DLQ: not met within 240s; the DLQ stayed empty"). No dead letter is produced and the discarded batch is reported nowhere.
Why: dead-lettering is the only record a client has of data a pipe gave up on. With no dead letter, failed records vanish silently: an operator pipeline that alarms on DLQ depth never fires, and a replay-from-DLQ recovery path has nothing to replay. A client cannot distinguish "nothing failed" from "everything failed and was dropped".
Fix: the Pipes runtime's enrichment-stage error handling — retry accounting against MaximumRetryAttempts and the dead-letter emitter that must publish the failure payload to SourceParameters...DeadLetterConfig.Arn. Open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md describes the absent runtime generally but does not carry this dead-letter replay key.
Traces: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload.

## Evidence
AssertionError: a dead letter on the pipe's DLQ: not met within 240s; the DLQ stayed empty. Expected at least one message on the configured DeadLetterConfig SQS queue carrying the documented Pipes dead-letter payload after the enrichment Lambda failed every attempt.

## Steps to Reproduce
1. Create a Kinesis stream, an SQS DLQ, an SQS target, and a Lambda enrichment function that always raises.
2. CreatePipe with Source=<stream ARN>, SourceParameters.KinesisStreamParameters {StartingPosition, MaximumRetryAttempts: small, DeadLetterConfig: {Arn: <DLQ ARN>}}, Enrichment=<function ARN>, Target=<target queue ARN>, DesiredState=RUNNING; wait for RUNNING.
3. PutRecord onto the stream and ReceiveMessage on the DLQ for up to 240 s — it stays empty.
