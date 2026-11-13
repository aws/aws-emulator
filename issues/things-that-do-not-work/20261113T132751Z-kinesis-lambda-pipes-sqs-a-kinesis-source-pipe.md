Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload
Issue-Key: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload|run#25|AssertionError|assert letter.get("context") == {
Issue-Commit: b70517cf179da71914a82c933e2ac0d438de9119

# kinesis__lambda__pipes__sqs: a Kinesis-source pipe's enrichment dead letter carries a non-AWS `context` object (pipeArn/stage/approximateInvokeCount instead of partnerResourceArn)

> _Note: this issue was identified against the code base at commit `b70517cf179da71914a82c933e2ac0d438de9119`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause (1 of 1).

Cause 1 of 1: a pipe with a Kinesis source, an always-throwing Lambda enrichment and SourceParameters.KinesisStreamParameters.DeadLetterConfig now does write one dead-letter record to the DLQ (progress over fixed issue 20261109T174004Z-kinesis-lambda-pipes-sqs-an-enrichment-failure-on.md and open issue 20261110T174759Z-kinesis-lambda-pipes-sqs-an-enrichment-failure-on-2e6738.md, which both describe an empty DLQ and neither of which covers the payload shape below), but the record's `context` object is not the one AWS publishes: the pipe ARN is carried under an invented member name and two members AWS does not emit are added.

AWS: recorded behavior only for the dead-letter document — Smithy (services/service-com-amazonaws-pipes-pipes) models PipeSourceKinesisStreamParameters.DeadLetterConfig -> DeadLetterConfig.Arn ('Define the target queue to send dead-letter queue events to') and the retry bounds MaximumRetryAttempts (range -1..10000) / MaximumRecordAgeInSeconds, but states nothing about the runtime payload. The recording shows the dead-letter message body is a JSON document whose `context` object has EXACTLY two members: `partnerResourceArn`, whose value is the ARN of the pipe that failed (a value taken from state, not fixed text), and `condition`, whose value is the fixed text `RetryAttemptsExhausted` for a batch that exhausted its retries. The document additionally carries `version` = "1.0" and a `KinesisBatchInfo` object (streamArn, shardId, batchSize, startSequenceNumber, endSequenceNumber), and the record is never delivered to the pipe's target.

Emulator: the record's `context` is {"pipeArn": "<pipe arn>", "stage": "ENRICHMENT", "condition": "RetryAttemptsExhausted", "approximateInvokeCount": 1}. Three divergences in one object: (a) the pipe ARN is published under the member name `pipeArn` instead of AWS's `partnerResourceArn`; (b) a `stage` member is added; (c) an `approximateInvokeCount` member is added. Only `condition` matches. The check asserts `context` first and stops there, so the emulator's `version` and `KinesisBatchInfo` members were not exercised by this replay and are not part of this finding.

Why: a consumer of the pipe's DLQ reads the failed batch's owner from `context.partnerResourceArn` — that is how it attributes a poison record to a pipe and re-drives it. Against the emulator that member is absent, so the attribution step reads null and every documented dead-letter consumer either drops the record or crashes, while the extra `stage`/`approximateInvokeCount` members make a strict schema validation of the documented envelope fail.

Fix: the pipes crate's dead-letter writer for stream sources (services/service-com-amazonaws-pipes-pipes) — the serializer that builds the dead-letter document's `context` object on retry exhaustion in the enrichment stage; it must emit `partnerResourceArn` (the pipe ARN) and `condition` and nothing else.

Traces: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload.

## Evidence
Expected (AWS): dead-letter body `context` == {"partnerResourceArn": "<arn of the pipe>", "condition": "RetryAttemptsExhausted"} — two members only; `partnerResourceArn` is a value from state, `condition` is fixed text for retry exhaustion.
Actual (emulator): context == {'pipeArn': 'arn:aws:pipes:<region>:<account>:pipe/<pipe name>', 'stage': 'ENRICHMENT', 'condition': 'RetryAttemptsExhausted', 'approximateInvokeCount': 1} — `partnerResourceArn` missing, ARN under `pipeArn`, plus extra `stage` and `approximateInvokeCount`.
AssertionError raised at check_kinesis__lambda__pipes__sqs_kinesis_enrichment_failure_dead_letters_to_sqs_with_documented_payload.py line 97: "the dead letter's context must name this pipe and RetryAttemptsExhausted". (The ARN/region/account/pipe-name values above are per-run and are not themselves the requirement.)

## Steps to Reproduce
1. Kinesis CreateStream (1 shard) and wait for ACTIVE; PutRecord a small JSON payload.
2. SQS CreateQueue twice (target and DLQ); deploy a Lambda enrichment whose handler always raises.
3. CreatePipe with Source=<stream arn>, Enrichment=<failing function arn>, Target=<target queue arn>, SourceParameters.KinesisStreamParameters={StartingPosition: TRIM_HORIZON, BatchSize: 1, MaximumRetryAttempts: 0, OnPartialBatchItemFailure: AUTOMATIC_BISECT, DeadLetterConfig:{Arn:<dlq arn>}}; wait for RUNNING.
4. ReceiveMessage from the DLQ and JSON-parse the body; inspect body["context"].
5. It is {pipeArn, stage, condition, approximateInvokeCount} instead of {partnerResourceArn, condition}. Replay kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload.
