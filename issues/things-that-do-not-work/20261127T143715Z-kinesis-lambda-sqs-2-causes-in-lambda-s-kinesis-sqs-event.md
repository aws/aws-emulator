Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__sqs.esm-on-failure-destination-is-accepted-for-a-kinesis-source-and-rejected-for-an-sqs-source kinesis__lambda__sqs.kinesis-esm-record-age-limit-discards-the-failing-record-to-the-on-failure-queue
Issue-Key: kinesis__lambda__sqs.esm-on-failure-destination-is-accepted-for-a-kinesis-source-and-rejected-for-an-sqs-source|_refusal#17|AssertionError|raise AssertionError(f"the SQS mapping was accepted with an OnFailure destination: {made!r}")
Issue-Key: kinesis__lambda__sqs.kinesis-esm-record-age-limit-discards-the-failing-record-to-the-on-failure-queue|run#35|AssertionError|assert time.monotonic() < deadline, (
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 2 causes in Lambda's Kinesis/SQS event-source mapping: OnFailure destination accepted for an SQS source, and MaximumRecordAgeInSeconds never discards to it

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two causes.

Cause 1 of 2: CreateEventSourceMapping accepts a `DestinationConfig.OnFailure` destination for an SQS event source.
AWS: on-failure destinations are supported only for stream sources (Kinesis, DynamoDB Streams, Kafka). For an SQS event source the configuration is not supported, so CreateEventSourceMapping is refused with `InvalidParameterValueException` (Smithy: com.amazonaws.lambda CreateEventSourceMapping error InvalidParameterValueException, HTTP 400) and no mapping is created; the same call against a Kinesis source is accepted.
Emulator: the SQS mapping is created and returned with HTTP 202 and `'DestinationConfig': {'OnFailure': {'Destination': 'arn:aws:sqs:...'}}` echoed back.
Why: the client is told a failure destination is configured for its SQS consumer; nothing will ever be written there, so failed batches are silently lost instead of being refused at configuration time when the mistake is cheap to fix.
Fix: CreateEventSourceMapping's validation of DestinationConfig against the event source's type.
Traces: kinesis__lambda__sqs.esm-on-failure-destination-is-accepted-for-a-kinesis-source-and-rejected-for-an-sqs-source.

Cause 2 of 2: a Kinesis event-source mapping's `MaximumRecordAgeInSeconds` does not discard the aged failing record to the on-failure destination.
AWS: when a record in a stream mapping exceeds MaximumRecordAgeInSeconds, Lambda stops retrying it, writes a failure record (the KinesisBatchInfo envelope) to the mapping's DestinationConfig.OnFailure destination, and moves the iterator past it so the shard drains.
Emulator: no on-failure record ever arrives; the handler is re-invoked repeatedly on the same batch (8 batches observed) and the record is never discarded.
Why: the mapping never makes progress past a poison record and the client never receives the failure envelope, so a single bad record stalls the shard indefinitely with no diagnostic.
Fix: the Kinesis event-source-mapping poller's record-age expiry path and its on-failure destination writer.
Traces: kinesis__lambda__sqs.kinesis-esm-record-age-limit-discards-the-failing-record-to-the-on-failure-queue.

## Evidence
Cause 1 — expected: CreateEventSourceMapping on an SQS source with DestinationConfig.OnFailure refused with InvalidParameterValueException (HTTP 400) and no mapping created. Actual: `AssertionError: the SQS mapping was accepted with an OnFailure destination: {... 'HTTPStatusCode': 202 ..., 'EventSourceArn': 'arn:aws:sqs:...', 'DestinationConfig': {'OnFailure': {'Destination': 'arn:aws:sqs:...'}}, ...}`.
Cause 2 — expected: one failure record on the on-failure queue once the record exceeds MaximumRecordAgeInSeconds, and the shard moving past it. Actual: `AssertionError: no on-failure record arrived; the handler saw 8 batch(es)`.

## Steps to Reproduce
Cause 1: CreateEventSourceMapping with EventSourceArn of an SQS queue and `DestinationConfig={'OnFailure': {'Destination': <queue arn>}}` — the call is accepted.
Cause 2: create a Kinesis stream mapping with a small MaximumRecordAgeInSeconds and DestinationConfig.OnFailure naming a queue, put a record, and make the handler throw; the handler is retried indefinitely and the on-failure queue stays empty.
Replay: the two check ids in trace_ids.
