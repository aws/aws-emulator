Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__lambda__s3.transform-function-error-routes-the-record-to-processing-failed firehose__lambda__s3.transform-omitting-a-record-id-routes-the-record-to-processing-failed firehose__lambda__s3.transform-repeating-a-record-id-routes-the-record-to-processing-failed
Issue-Key: firehose__lambda__s3.transform-function-error-routes-the-record-to-processing-failed|_poll_blob#9|AssertionError|raise AssertionError(f"nothing expected arrived under {prefix!r}; saw {blob[:300]!r}")
Issue-Key: firehose__lambda__s3.transform-omitting-a-record-id-routes-the-record-to-processing-failed|_poll_blob#9|AssertionError|raise AssertionError(f"nothing expected arrived under {prefix!r}; saw {blob[:300]!r}")
Issue-Key: firehose__lambda__s3.transform-repeating-a-record-id-routes-the-record-to-processing-failed|_poll_blob#9|AssertionError|raise AssertionError(f"nothing expected arrived under {prefix!r}; saw {blob[:300]!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose__lambda__s3: a DirectPut stream with an enabled Lambda ProcessingConfiguration delivers nothing to S3 — not even records the transform returns with result 'Ok'

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: enabling a Lambda processor on a delivery stream makes the destination write path inert. Records the transform returns as 'Ok' never reach the configured Prefix, so no transformation scenario — success or failure — can be observed.

AWS: recorded behavior only for the delivery timing; the surface is modelled as CreateDeliveryStream's ExtendedS3DestinationConfiguration {Prefix, ErrorOutputPrefix, BufferingHints, ProcessingConfiguration{Enabled, Processors[Type='Lambda', Parameters LambdaArn/NumberOfRetries/BufferIntervalInSeconds/BufferSizeInMBs]}} (Smithy; the Prefix member documents custom prefixes per https://docs.aws.amazon.com/firehose/latest/dev/s3-prefixes.html). In the recording, a DirectPut stream with an enabled Lambda processor invokes the function per buffered batch, and every record the function returns with result 'Ok' is written under the destination Prefix ('landed/') with its returned data; records the function drops or fails, and records lost to an invocation error, are written instead under ErrorOutputPrefix with !{firehose:error-output-type} resolved to 'processing-failed', as one JSON document per record carrying errorCode (e.g. 'Lambda.FunctionError'), base64 rawData equal to the original bytes, and lambdaARN naming the transform. A later successful record still lands, proving a failed record's outcome is final and the stream keeps working.

Emulator: PutRecordBatch is accepted (FailedPutCount 0, a RecordId per entry), but nothing ever appears under the destination Prefix. All three transformation checks fail at their FIRST poll, on the plain control record that the transform returns verbatim as 'Ok': "AssertionError: nothing expected arrived under 'landed/'; saw b''" after the full delivery budget. Since the same emulator delivers for streams WITHOUT a processor, the regression is in the processing stage: with ProcessingConfiguration.Enabled true the records are consumed and never handed to the S3 writer, so neither the success prefix nor the processing-failed error prefix is ever populated.

Why: Firehose data transformation is unusable. A client that puts records through a Lambda-transformed stream gets a successful ingest acknowledgement and then permanent, silent data loss — no delivered object, no error object, no way to tell. Every downstream behaviour that depends on it is unreachable too: the Lambda.FunctionError routing, the processing-failed output type under ErrorOutputPrefix, the per-record rawData/lambdaARN error envelope, and the record-id contract (a transform result that omits a recordId or repeats one must route that record to processing-failed).

Fix: the record-processing stage of the firehose service crate that sits between ingest and the S3 destination writer (the ProcessingConfiguration/Lambda processor path and its hand-off to the delivery writer) — invoke the configured LambdaArn for each buffered batch, write records returned with result 'Ok' under Prefix with their returned data, and route dropped records, records whose returned recordId is missing or duplicated, and records lost to an invocation error to ErrorOutputPrefix with !{firehose:error-output-type} resolved to 'processing-failed', one JSON document per record with errorCode, base64 rawData and lambdaARN. A stream with an enabled processor must deliver at least as reliably as one without.

Traces: firehose__lambda__s3.transform-function-error-routes-the-record-to-processing-failed, firehose__lambda__s3.transform-omitting-a-record-id-routes-the-record-to-processing-failed, firehose__lambda__s3.transform-repeating-a-record-id-routes-the-record-to-processing-failed.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout. All three checks configure the same stream shape — ExtendedS3DestinationConfiguration {Prefix: 'landed/', ErrorOutputPrefix: 'refused/!{firehose:error-output-type}/', BufferingHints {SizeInMBs: 1, IntervalInSeconds: 0}, ProcessingConfiguration {Enabled: true, Processors: [Lambda with LambdaArn/NumberOfRetries=0/BufferIntervalInSeconds=0/BufferSizeInMBs=1]}} — and all three fail on the first, plain control record:

[mismatch] firehose__lambda__s3.transform-function-error-routes-the-record-to-processing-failed
  line 61: landed = _poll_blob(ctx, s3, bucket, _OK_PREFIX, lambda blob: control in blob)
  line 119: raise AssertionError(f"nothing expected arrived under {prefix!r}; saw {blob[:300]!r}")
  AssertionError: nothing expected arrived under 'landed/'; saw b''

[mismatch] firehose__lambda__s3.transform-omitting-a-record-id-routes-the-record-to-processing-failed
  AssertionError: nothing expected arrived under 'landed/'; saw b''

[mismatch] firehose__lambda__s3.transform-repeating-a-record-id-routes-the-record-to-processing-failed
  AssertionError: nothing expected arrived under 'landed/'; saw b''

Expected (AWS): after PutRecordBatch of one record whose transform returns {'recordId': <same>, 'result': 'Ok', 'data': <same>}, an object appears under the Prefix 'landed/' containing that record exactly once; a record the transform fails/drops/mis-ids appears instead under 'refused/processing-failed/' as one JSON document with errorCode (e.g. 'Lambda.FunctionError'), base64 rawData equal to the original bytes and lambdaARN naming the function.
Actual (emulator): PutRecordBatch returns FailedPutCount 0 with a RecordId, and ListObjectsV2 under 'landed/' (and under 'refused/') stays empty for the whole delivery budget — the concatenated blob is b''.

The generated bucket, function, role and stream names are per-run values; the requirement is that an 'Ok' record lands under Prefix and a failed record lands under ErrorOutputPrefix with the processing-failed output type.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket. Create a Lambda function whose handler base64-decodes each event['records'][i]['data'], raises for payloads starting b'poison-', and otherwise returns {'records': [{'recordId': item['recordId'], 'result': 'Ok', 'data': item['data']}]}.
3. Create a firehose.amazonaws.com-trusted role allowed the S3 write set on the bucket and lambda:InvokeFunction/lambda:GetFunctionConfiguration on the function.
4. CreateDeliveryStream(DeliveryStreamName=<n>, DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={RoleARN, BucketARN, Prefix: 'landed/', ErrorOutputPrefix: 'refused/!{firehose:error-output-type}/', BufferingHints: {SizeInMBs: 1, IntervalInSeconds: 0}, ProcessingConfiguration: {Enabled: True, Processors: [{Type: 'Lambda', Parameters: [{LambdaArn: <fn arn>}, {NumberOfRetries: '0'}, {BufferIntervalInSeconds: '0'}, {BufferSizeInMBs: '1'}]}]}}); wait ACTIVE.
5. PutRecordBatch(Records=[{'Data': b'plain-<marker>\n'}]) — FailedPutCount must be 0.
6. Poll ListObjectsV2/GetObject under prefix 'landed/'. On AWS an object containing 'plain-<marker>' appears; observe that nothing ever appears (empty listing) for the whole budget.
7. PutRecordBatch(Records=[{'Data': b'poison-<marker>\n'}]) and poll 'refused/'. On AWS one JSON document appears under 'refused/processing-failed/' with errorCode 'Lambda.FunctionError', rawData base64 of the poison bytes and lambdaARN naming the function; observe nothing.
8. Repeat with a transform that omits the recordId of a returned record, and with one that repeats a recordId: those records must route to 'refused/processing-failed/' while the others still land.
9. Re-replay the three checks in trace_ids to confirm a fix.
