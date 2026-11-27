Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__pipes__sqs.kinesis-source-automatic-bisect-isolates-a-throwing-record-to-the-dead-letter-queue
Issue-Key: kinesis__lambda__pipes__sqs.kinesis-source-automatic-bisect-isolates-a-throwing-record-to-the-dead-letter-queue|run#43|AssertionError|raise AssertionError(
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# kinesis__lambda__pipes__sqs: OnPartialBatchItemFailure=AUTOMATIC_BISECT is ignored — the pipe retries the whole Kinesis batch and dead-letters all of it instead of halving to isolate the one throwing record (1 cause)

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: a Kinesis-source pipe configured with OnPartialBatchItemFailure=AUTOMATIC_BISECT never bisects a failing batch.
AWS: Smithy (services/service-com-amazonaws-pipes-pipes) documents PipeSourceKinesisStreamParameters.OnPartialBatchItemFailure as: "Define how to handle item process failures. AUTOMATIC_BISECT halves each batch and retry each half until all the records are processed or there is one failed message left in the batch." With MaximumRetryAttempts set and a DeadLetterConfig.Arn, the recorded behavior is: the target Lambda is first invoked with the whole batch and throws; the pipe then halves the batch and retries each half, repeating until the throwing record sits alone in a batch of one; every batch that does not contain it succeeds, so each good record is delivered exactly once in some sub-batch, and exactly ONE dead-letter record is written — its KinesisBatchInfo has batchSize 1 with startSequenceNumber == endSequenceNumber == the throwing record's own sequence number, and streamArn equal to the pipe's source stream.
Emulator: no bisection happens. The function is invoked twice, both times with the full four-record batch (identical keys and identical sequenceNumber lists), both times throwing; no sub-batch is ever formed, so no good record is ever delivered successfully. One dead letter is written but it covers the WHOLE batch: KinesisBatchInfo.batchSize = 4 with startSequenceNumber = the first record's sequence number and endSequenceNumber = the fourth's (condition 'RetryAttemptsExhausted', responseContext.functionError 'Unhandled').
Why: a client relying on AUTOMATIC_BISECT loses every good record in any batch that contains one bad record — the three healthy records are never processed and are dead-lettered together with the poison record, which is precisely the failure mode the setting exists to prevent. The dead-letter record also mis-reports the blast radius (batchSize 4 and a sequence-number span) so an operator cannot identify the offending record from it.
Fix: the pipes stream-source delivery loop's partial-batch failure handling — when SourceParameters.KinesisStreamParameters.OnPartialBatchItemFailure is AUTOMATIC_BISECT, a batch whose target invocation fails must be split in half and each half re-delivered (respecting MaximumRetryAttempts) until a single record remains, and only that single-record batch may be written to DeadLetterConfig.Arn, with KinesisBatchInfo describing that one record.
Traces: kinesis__lambda__pipes__sqs.kinesis-source-automatic-bisect-isolates-a-throwing-record-to-the-dead-letter-queue

## Evidence
Pipe created with SourceParameters={'KinesisStreamParameters': {'StartingPosition': 'TRIM_HORIZON', 'BatchSize': 4, 'MaximumBatchingWindowInSeconds': 1, 'MaximumRetryAttempts': 1, 'OnPartialBatchItemFailure': 'AUTOMATIC_BISECT', 'DeadLetterConfig': {'Arn': <dlq>}}}, TargetParameters={'LambdaFunctionParameters': {'InvocationType': 'REQUEST_RESPONSE'}}; four records put under partition keys rec-alpha, rec-bravo, rec-charlie (the one the handler throws on), rec-delta.

Expected: a failing batch of 4, then halved batches, eventually a failing batch holding exactly ['rec-charlie']; every succeeding batch a proper subset of the original without rec-charlie, together covering {rec-alpha, rec-bravo, rec-delta}; exactly one dead letter whose KinesisBatchInfo = {batchSize: 1, startSequenceNumber == endSequenceNumber == rec-charlie's SequenceNumber, streamArn == the source stream ARN}.

Actual (AssertionError after 34s): the function saw only [{'keys': ['rec-alpha','rec-bravo','rec-charlie','rec-delta'], 'threw': True}, {'keys': ['rec-alpha','rec-bravo','rec-charlie','rec-delta'], 'threw': True}] — the same full batch twice, no sub-batch, no successful invocation — and the dead-letter queue held one record with 'KinesisBatchInfo': {'shardId': 'shardId-000000000000', 'startSequenceNumber': '<seq of record 0>', 'endSequenceNumber': '<seq of record 3>', 'batchSize': 4, 'streamArn': '<source stream>', ...}, 'context': {'condition': 'RetryAttemptsExhausted'}, 'responseContext': {'functionError': 'Unhandled'}.

## Steps to Reproduce
1. Create a Kinesis stream (1 shard), a trace SQS queue, a dead-letter SQS queue, an IAM role for the Lambda and an IAM role trusting pipes.amazonaws.com.
2. PutRecords four records to the stream BEFORE the pipe exists, with partition keys rec-alpha, rec-bravo, rec-charlie, rec-delta (so the first TRIM_HORIZON read is one batch of four).
3. CreateFunction (python3.12) whose handler reports each received batch's partitionKeys/sequenceNumbers to the trace queue and raises RuntimeError when the batch contains rec-charlie.
4. CreatePipe(Source=<stream ARN>, Target=<function ARN>, DesiredState='RUNNING', SourceParameters={'KinesisStreamParameters': {'StartingPosition':'TRIM_HORIZON','BatchSize':4,'MaximumBatchingWindowInSeconds':1,'MaximumRetryAttempts':1,'OnPartialBatchItemFailure':'AUTOMATIC_BISECT','DeadLetterConfig':{'Arn':<dlq arn>}}}, TargetParameters={'LambdaFunctionParameters':{'InvocationType':'REQUEST_RESPONSE'}}); wait for RUNNING.
5. Drain the trace queue and the dead-letter queue. Observe only full-batch invocations, no successful sub-batch, and one dead letter with batchSize 4.
