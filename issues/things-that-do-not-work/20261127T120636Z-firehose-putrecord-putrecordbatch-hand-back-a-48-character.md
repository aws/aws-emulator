Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.put-record-batch-into-cloudwatch-decompression-stream-rejected-invalid-source firehose.put-record-on-decompression-only-stream-rejected-until-processing-disabled
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.put-record-batch-into-cloudwatch-decompression-stream-rejected-invalid-source|run#35|AssertionError|assert len(ids) == _BATCH and not malformed and len(set(ids)) == _BATCH, (
Issue-Key: firehose.put-record-on-decompression-only-stream-rejected-until-processing-disabled|run#36|AssertionError|assert _looks_like_record_id(record_id), (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose: PutRecord/PutRecordBatch hand back a 48-character RecordId instead of the long padded-base64 RecordId AWS returns

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: the RecordId the ingest operations mint is not shaped like a Firehose RecordId. The emulator returns a short (48-character) string, where AWS returns a long padded standard-base64 token.

AWS: Smithy: PutRecord output RecordId targets PutResponseRecordId with @length min 1 only, and PutRecordBatch's RequestResponses entries carry the same RecordId shape — the model states no upper bound and no character set, so this is recorded behavior only. The recording shows every accepted record answered with a RecordId that is a padded standard base64 string (A-Z a-z 0-9 + / with '=' padding) comfortably longer than 64 characters, distinct per record within a batch. The requirement is the FORM of the value — long padded standard base64, one distinct value per accepted record — not any particular recorded token, which is a per-run value.

Emulator: PutRecord and PutRecordBatch return RecordId values of type str and length 48 — three distinct 48-character strings for a 3-record batch, and a single 48-character string for PutRecord. The checks report: "the twin stream returned 3 entries with 3 distinct record ids; malformed (type, length): [('str', 48), ('str', 48), ('str', 48)]; expected 3 distinct base64 ids of at least 64 characters" and "PutRecord after disabling processing returned a RecordId of type str and length 48, expected a padded standard base64 string of at least 64 characters". Uniqueness and the 200 are correct; only the token's length/encoding diverge.

Why: a client that persists, logs, correlates or length-validates Firehose RecordIds (column widths, fixed-size buffers, regex validation of the base64 form, de-duplication keys) is written against AWS's long base64 token. Code that round-trips a RecordId through base64 decoding, or a fixture that asserts the documented token shape, passes on AWS and fails here; conversely a consumer sized for the emulator's short id overflows against AWS.

Fix: the RecordId minting used by the PutRecord and PutRecordBatch handlers in the firehose service crate (the shared record-id generator feeding both the single-record response and each PutRecordBatchResponseEntry) — produce a padded standard-base64 token of AWS's length rather than the current short identifier, keeping one distinct value per accepted record. Both operations must draw from the same generator so a batch entry and a single put are indistinguishable in form.

Traces: firehose.put-record-batch-into-cloudwatch-decompression-stream-rejected-invalid-source, firehose.put-record-on-decompression-only-stream-rejected-until-processing-disabled.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

[mismatch] firehose.put-record-batch-into-cloudwatch-decompression-stream-rejected-invalid-source
  File "check_firehose_put_record_batch_into_cloudwatch_decompression_stream_rejected_invalid_source.py", line 264, in run
    assert len(ids) == _BATCH and not malformed and len(set(ids)) == _BATCH, (
  AssertionError: the twin stream returned 3 entries with 3 distinct record ids; malformed (type, length): [('str', 48), ('str', 48), ('str', 48)]; expected 3 distinct base64 ids of at least 64 characters

[mismatch] firehose.put-record-on-decompression-only-stream-rejected-until-processing-disabled
  File "check_firehose_put_record_on_decompression_only_stream_rejected_until_processing_disabled.py", line 251, in run
    assert _looks_like_record_id(record_id), (
  AssertionError: PutRecord after disabling processing returned a RecordId of type str and length 48, expected a padded standard base64 string of at least 64 characters

Expected (AWS): PutRecord returns HTTP 200 with RecordId as a padded standard base64 string (validate=True decodable) of at least 64 characters; PutRecordBatch returns FailedPutCount 0 and one such distinct RecordId per RequestResponses entry.
Actual (emulator): HTTP 200 with RecordId strings of length 48 that do not satisfy the padded-base64/length form, both for PutRecord and for every PutRecordBatch entry.

The individual token values are per-run and are not the requirement; the form (long padded standard base64, distinct per record) is.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateDeliveryStream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={RoleARN, BucketARN, BufferingHints}) and wait for ACTIVE.
3. PutRecord(DeliveryStreamName=<stream>, Record={'Data': b'hello\n'}). Read Output.RecordId: on AWS it is a padded standard base64 string of at least 64 characters; observe a 48-character string instead.
4. PutRecordBatch(DeliveryStreamName=<stream>, Records=[{'Data': b'a'}, {'Data': b'b'}, {'Data': b'c'}]). Each RequestResponses[i].RecordId must have the same form; observe three 48-character strings.
5. Re-replay the two checks in trace_ids to confirm a fix.
