Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__s3.update-destination-invalid-compression-format-rejected
Issue-Key: firehose__iam__s3.update-destination-invalid-compression-format-rejected|expect_error#14|AssertionError|assert got_message == message, f"expected message {message!r}, got {got_message!r}"
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# firehose: the compressionFormat enum-violation message omits ZSTD from the enum value set it lists

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDestination

## Description
=== Cause 1 of 1: the ValidationException's enum value set is missing ZSTD ===
AWS: `UpdateDestination` with an `ExtendedS3DestinationUpdate.CompressionFormat` outside the enum is refused `ValidationException` whose message is the standard constraint-violation sentence listing EVERY accepted value: `1 validation error detected: Value at 'extendedS3DestinationUpdate.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]`. Fixed text: `<n> validation error(s) detected: Value at '`, `' failed to satisfy constraint: Member must satisfy enum value set: [`, `]`; variable parts: the member path from the request and the service's accepted-value set. The repo's Smithy model (`com.amazonaws.firehose#CompressionFormat`) carries only `UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY` — the recorded AWS response lists a sixth value, `ZSTD`, so for ZSTD this is recorded behavior only (AWS accepts ZSTD and names it in the refusal; the on-disk model predates it).
Emulator: the same sentence with the same member path and the same status/code, but the enum value set is `[ZIP, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]` — `ZSTD` is absent.
Why: the message is how a client discovers the accepted values. A caller reading the refusal concludes ZSTD is not supported and never sends it; and because the emulator's accepted set is the five modelled values, a legitimate `CompressionFormat=ZSTD` update is itself refused rather than applied. The enum's membership, not just the message text, is wrong.
Fix: the firehose `CompressionFormat` enum (add `ZSTD` to the accepted set) and the enum-violation message builder that renders the value set for `extendedS3DestinationUpdate.compressionFormat`; the same set must be used by `CreateDeliveryStream`'s `ExtendedS3DestinationConfiguration.CompressionFormat`.
Traces: firehose__iam__s3.update-destination-invalid-compression-format-rejected.

## Evidence
From one replay of the behavioral suite (service heading `### firehose__iam__s3`, 0/1 passed).

[mismatch] firehose__iam__s3.update-destination-invalid-compression-format-rejected
  File "check_firehose__iam__s3_update_destination_invalid_compression_format_rejected.py", line 47, in expect_error
    assert got_message == message, f"expected message {message!r}, got {got_message!r}"
  AssertionError: expected message "1 validation error detected: Value at 'extendedS3DestinationUpdate.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]", got "1 validation error detected: Value at 'extendedS3DestinationUpdate.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]"

Status and error code match (`ValidationException`); the single difference is the missing `ZSTD` member of the listed enum value set.
Smithy (on disk): `com.amazonaws.firehose#CompressionFormat` = [UNCOMPRESSED, GZIP, ZIP, Snappy, HADOOP_SNAPPY] — i.e. the emulator matches the stale model, not the recorded service.

## Steps to Reproduce
1. Create an S3 bucket and a Firehose delivery role, then `CreateDeliveryStream` with an `ExtendedS3DestinationConfiguration`.
2. Call `UpdateDestination` with `ExtendedS3DestinationUpdate.CompressionFormat` set to a value outside the enum (e.g. `BROTLI`).
3. The refusal must be `ValidationException` with message `1 validation error detected: Value at 'extendedS3DestinationUpdate.compressionFormat' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, ZSTD, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED]`; observed the same sentence without `ZSTD`.
4. Also call `UpdateDestination` with `CompressionFormat=ZSTD`: it must be ACCEPTED; the emulator's five-value set refuses it.
