Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.e2e-put-record-kinesis-source-rejected firehose.put-record-invalid-source-stream-rejected firehose.kinesis-source-preserves-sequence-number-order firehose.put-record-batch-invalid-source-kinesis-backed-stream
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.e2e-put-record-kinesis-source-rejected|_error_code#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: firehose.put-record-invalid-source-stream-rejected|_error_code#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: firehose.kinesis-source-preserves-sequence-number-order|_error_code#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: firehose.put-record-batch-invalid-source-kinesis-backed-stream|run#19|AssertionError|raise AssertionError(
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Firehose PutRecord/PutRecordBatch are accepted with 200 on a KinesisStreamAsSource delivery stream instead of being refused with InvalidArgumentException

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: direct-put ingestion is not gated on the delivery stream's source, so a stream created with DeliveryStreamType=KinesisStreamAsSource (KinesisStreamSourceConfiguration) still accepts PutRecord and PutRecordBatch.

Failing checks for this cause: firehose.e2e-put-record-kinesis-source-rejected, firehose.put-record-invalid-source-stream-rejected, firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-batch-invalid-source-kinesis-backed-stream.

AWS: a Firehose stream whose source is a Kinesis data stream cannot also take direct puts — "When you configure a Kinesis data stream as the source of a Firehose stream, the PutRecord and PutRecordBatch operations are disabled" (https://docs.aws.amazon.com/firehose/latest/dev/writing-with-kinesis-streams.html). Smithy: PutRecord and PutRecordBatch both declare InvalidArgumentException (errorKind client, HTTP 400) — model_path services/service-com-amazonaws-firehose-firehose-20150804/smithy-model/com-amazonaws-firehose-firehose-20150804.json. Recorded behavior: the call is refused SYNCHRONOUSLY with 400 InvalidArgumentException, and the record is never delivered; InvalidSourceException is NOT used here (it is reserved for the CloudWatch-Logs-decompression case). The requirement is therefore the status 400 and the error code InvalidArgumentException; the recorded message is only an example.

Emulator: the put succeeds with HTTP 200 and mints a RecordId — e.g. {'RecordId': '0000...0001', 'Encrypted': False, 'ResponseMetadata': {'HTTPStatusCode': 200}} — and PutRecordBatch likewise answers a success envelope rather than refusing the batch wholesale. The refusal set the handler can produce has no variant for this case at all: crate::state::StreamRefusal offers only NoSuchStream, NotActive(status) and SourceNotCloudWatchLogs, so the source type is never consulted.

Why: a producer that mistakenly points direct puts at a KDS-sourced stream is told its write was accepted and is handed a RecordId, but on real AWS that write is rejected and the client must repoint or create a DirectPut stream. The emulator therefore hides a configuration error, and any test that asserts the refusal (or counts on only KDS-originated records reaching the destination) passes against the emulator and fails against AWS. It also makes the KDS-ordering check unable to reach its ordering assertions at all.

Fix: the source-type gate belongs in the firehose state's ingest path (a new StreamRefusal variant beside SourceNotCloudWatchLogs) with both put handlers mapping it to InvalidArgumentException; the two handlers must agree, PutRecordBatch refusing the whole request rather than reporting per-record failures.

Traces: see the four failing check ids above; file/line citations: services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs lines 204-223 (the match on StreamRefusal), the sibling services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record_batch.rs, and crate::state::ingest_record / StreamRefusal in services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs.

Related but not covering this: open issue 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md collects other over-permissive put/create cases (size ceilings, malformed ARNs) and does not carry these replay keys; open issue 20261022T194351Z-firehose-logs-s3-...-putrecord-is-accepted-on-a-cloudwatch-logs... (fixed) covered only the CloudWatch-Logs InvalidSourceException gate, which is a different source class.

## Evidence
Expected (AWS): firehose.put_record(DeliveryStreamName=<KinesisStreamAsSource stream>, Record={'Data': ...}) -> HTTP 400, Error.Code = InvalidArgumentException; nothing delivered. Same for firehose.put_record_batch on that stream (whole request refused, no RequestResponses).
Actual (emulator): HTTP 200 with a RecordId.
  * firehose.e2e-put-record-kinesis-source-rejected: AssertionError: expected the call to be refused, but it succeeded: {'RecordId': '000000000000000018e19dae50e391020000000000000001', 'Encrypted': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  * firehose.put-record-invalid-source-stream-rejected: AssertionError: expected the call to be rejected, but it succeeded: {'RecordId': '000000000000000018e19cc1bb6197bb...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  * firehose.kinesis-source-preserves-sequence-number-order: AssertionError: expected the call to be refused, but it succeeded: {'RecordId': '000000000000000018e19d6e2c939cec...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  * firehose.put-record-batch-invalid-source-kinesis-backed-stream: AssertionError: expected InvalidArgumentException for PutRecordBatch on a Kinesis-sourced stream
In each case the positive control (the sibling DirectPut stream in the same check) is expected to keep succeeding, so the fix must be conditional on the source type, not a blanket refusal.

## Steps to Reproduce
1. CreateDeliveryStream with DeliveryStreamType=KinesisStreamAsSource and a KinesisStreamSourceConfiguration pointing at a Kinesis stream ARN, plus an (Extended)S3DestinationConfiguration; wait for DeliveryStreamStatus=ACTIVE.
2. Call PutRecord on that stream with any Record.Data -> emulator answers 200 + RecordId; AWS answers 400 InvalidArgumentException.
3. Call PutRecordBatch on the same stream -> emulator answers a success envelope; AWS refuses the whole request with 400 InvalidArgumentException.
4. Create a sibling DirectPut stream and repeat both calls: these must still succeed (positive control).
5. Re-replay: firehose.e2e-put-record-kinesis-source-rejected, firehose.put-record-invalid-source-stream-rejected, firehose.kinesis-source-preserves-sequence-number-order, firehose.put-record-batch-invalid-source-kinesis-backed-stream.
