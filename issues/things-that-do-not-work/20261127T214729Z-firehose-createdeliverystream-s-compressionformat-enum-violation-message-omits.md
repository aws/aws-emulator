Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-invalid-compression-type-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-invalid-compression-type-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 5c66e8f363b2794f9d87fe11d0e172b4338f59d2

# firehose: CreateDeliveryStream's compressionFormat enum-violation message omits ZSTD from the enum value set it lists (and ZSTD is not an accepted value)

> _Note: this issue was identified against the code base at commit `5c66e8f363b2794f9d87fe11d0e172b4338f59d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Cause 1 of 1: the ValidationException CreateDeliveryStream serves for an out-of-enum ExtendedS3DestinationConfiguration.CompressionFormat lists an enum value set that is missing ZSTD, and the emulator's accepted set is likewise missing it.

AWS: CreateDeliveryStream with an ExtendedS3DestinationConfiguration.CompressionFormat outside the enum is refused with HTTP 400 and error code ValidationException, whose message is the standard constraint-violation sentence listing EVERY accepted value: "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]". Fixed text of that message class: "<n> validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the member path taken from the request (here extendedS3DestinationConfiguration.compressionFormat) and the service's accepted-value set. The repo's on-disk Smithy model (com.amazonaws.firehose#CompressionFormat) carries only [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY]; the recorded AWS response names a sixth value, ZSTD, so for ZSTD this is recorded behavior only — AWS accepts ZSTD and names it in the refusal, and the on-disk model predates it. The rejected create must also leave no stream behind (DescribeDeliveryStream on the name answers ResourceNotFoundException), and the same create with CompressionFormat=GZIP must succeed; both of those legs pass.
Emulator: same status and same error code (ValidationException), same member path, same sentence, but the enum value set it renders is "[ZIP, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]" — ZSTD is absent. The emulator is reproducing the stale five-value model rather than the service's six-value set.
Why: the refusal message is how a client discovers the accepted values. A caller reading it concludes ZSTD is not a supported compression format and never sends it; and because the emulator's ACCEPTED set is the same five modelled values, a legitimate CreateDeliveryStream with CompressionFormat=ZSTD is itself refused rather than accepted, so a pipeline that works on AWS cannot be created against the emulator at all. The enum's membership, not merely the message text, is wrong.
Fix: the firehose CompressionFormat enum (add ZSTD to the accepted set) and the enum-violation message builder that renders the value set, in CreateDeliveryStream's input validation for ExtendedS3DestinationConfiguration.CompressionFormat (the same set must be shared with S3DestinationConfiguration and with UpdateDestination's ExtendedS3DestinationUpdate).
Related: open issue 20261127T173710Z-firehose-iam-s3-firehose-the-compressionformat-enum-violation-message-omits-zstd.md records the identical defect on UpdateDestination's extendedS3DestinationUpdate.compressionFormat path and explicitly says the same set must be used by CreateDeliveryStream; it owns only the firehose__iam__s3 UpdateDestination check and the suite does not report this CreateDeliveryStream check as owned by it, so it is filed here. Fixing the shared enum/message builder should close both.
Traces: firehose.create-stream-invalid-compression-type-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (service heading `### firehose`, 183/187 passed).

[mismatch] firehose.create-stream-invalid-compression-type-rejected
  File "check_firehose_create_stream_invalid_compression_type_rejected.py", line 40, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]", got "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]"

Expected (AWS): ValidationException whose enum value set lists six values including ZSTD.
Actual (emulator): ValidationException with the identical fixed text and member path, listing five values — ZSTD missing.

Status and error code match; the single difference is the missing ZSTD member of the listed set. The generated stream name in the request (sim2real-fh-badcompress-<suffix>) is a per-run value and is not part of the requirement.

Smithy (on disk, services/service-com-amazonaws-firehose-firehose-20150804): com.amazonaws.firehose#CompressionFormat = [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY] — i.e. the emulator matches the stale model, not the recorded service.

## Steps to Reproduce
1. Boot the emulator from this checkout. Create an S3 bucket and an IAM role trusting firehose.amazonaws.com with s3:PutObject/GetBucketLocation/ListBucket on that bucket.
2. CreateDeliveryStream(DeliveryStreamName=<new name>, ExtendedS3DestinationConfiguration={RoleARN, BucketARN, CompressionFormat: "BOGUS_FORMAT"}).
3. The refusal must be HTTP 400 ValidationException with message "1 validation error detected: Value at 'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]"; observe the same sentence with ZSTD absent from the set.
4. Also call CreateDeliveryStream with CompressionFormat="ZSTD": it must be ACCEPTED and the stream must reach ACTIVE; the emulator's five-value set refuses it.
5. Re-replay the check in trace_ids to confirm the fix.
