Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__s3.channel-s3-destination-sends-invalid-json-records-to-the-dead-letter-queue iam__kinesis__s3.create-channel-expected-bucket-owner-outside-twelve-characters-rejected
Issue-Key: iam__kinesis__s3.channel-s3-destination-sends-invalid-json-records-to-the-dead-letter-queue|_wait_until#37|Timeout|time.sleep(interval_s)
Issue-Key: iam__kinesis__s3.create-channel-expected-bucket-owner-outside-twelve-characters-rejected|run#35|AssertionError|assert got == want, f"an {_OWNER_LEN - 1}-character ExpectedBucketOwner: expected {want!r}, got {got!r}"
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# kinesis: invalid-JSON records never reach the S3 destination's dead-letter prefix, and the ExpectedBucketOwner constraint message quotes the offending value AWS withholds

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStreamingChannel

## Description
Two net-new root causes in the Kinesis streaming-channel S3 destination.

=== Cause 1 of 2: records that are not valid JSON are never delivered to the S3 dead-letter prefix ===
AWS: recorded behavior only. A channel with an S3 destination configured with a dead-letter (error) prefix delivers records it cannot parse as JSON to that prefix instead of dropping them: within the destination's data-freshness window, objects appear under `errors/` in the destination bucket carrying the unparseable records, while the well-formed records land under the normal prefix. The recording shows both deliveries completing inside the freshness window.
Emulator: nothing is ever delivered to the dead-letter prefix — the check polls the bucket for the whole 600 s budget and ends `Check timed out after 600.0s`.
Why: the dead-letter prefix is the only place a client can recover records the destination rejected. Against the emulator those records are lost silently: the stream reports them accepted, the normal prefix does not contain them, and the error prefix stays empty forever, so a pipeline cannot reconcile what it ingested.
Fix: the channel's S3 delivery path — route a record whose payload fails JSON parsing to the destination's configured error/dead-letter prefix rather than discarding it.
Traces: iam__kinesis__s3.channel-s3-destination-sends-invalid-json-records-to-the-dead-letter-queue.

=== Cause 2 of 2: the constraint-violation message quotes the offending ExpectedBucketOwner value, which AWS withholds ===
AWS: `ExpectedBucketOwner` is an account id — Smithy: `com.amazonaws.kinesis#ExpectedBucketOwner`, `@length(min: 12, max: 12)`, `@pattern("^\\d{12}$")`, and `S3StorageConfiguration.ExpectedBucketOwner` is required. An 11-character value violates both constraints and is refused `ValidationException` whose message names each violated constraint WITHOUT echoing the value: `2 validation errors detected: Value at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must have length greater than or equal to 12; Value at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}`. Fixed text: `<n> validation errors detected: `, `Value at '`, `' failed to satisfy constraint: Member must have length greater than or equal to `, `Member must satisfy regular expression pattern: `; variable parts: the member path, the length bound and the pattern, all from the model — the member's VALUE is deliberately not part of it (it is an account id).
Emulator: the status and code match, and both constraint clauses are present and correctly worded, but each clause is prefixed `Value '11111111111' at '...'` — the account id the caller sent is echoed back into the message.
Why: an account id is the kind of value AWS keeps out of error text; a client (or a log aggregator) that stores refusal messages now stores the caller's account identifier, and a client matching on AWS's message shape does not match.
Fix: the constraint-violation message builder for this member — render the `Value at '<path>'` form (no echoed value) for `expectedBucketOwner`, as the recording does.
Traces: iam__kinesis__s3.create-channel-expected-bucket-owner-outside-twelve-characters-rejected.

## Evidence
From one replay of the behavioral suite (service heading `### iam__kinesis__s3`).

[mismatch] iam__kinesis__s3.channel-s3-destination-sends-invalid-json-records-to-the-dead-letter-queue
  Error: Check timed out after 600.0s
  (the check polls the destination bucket's `errors/` prefix within a 300 s data-freshness window; nothing ever appears)

[mismatch] iam__kinesis__s3.create-channel-expected-bucket-owner-outside-twelve-characters-rejected
  AssertionError: an 11-character ExpectedBucketOwner: expected ('ValidationException', "2 validation errors detected: Value at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must have length greater than or equal to 12; Value at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"), got ('ValidationException', "2 validation errors detected: Value '11111111111' at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must have length greater than or equal to 12; Value '11111111111' at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}")
  (the 11-character value is the request's; the requirement is that no value is echoed at all)

Smithy: com.amazonaws.kinesis#ExpectedBucketOwner — string, @length 12..12, @pattern ^\d{12}$; required member of S3StorageConfiguration.

## Steps to Reproduce
1. Cause 1: create an S3 bucket and a delivery role, then create a streaming channel with an S3 destination configured with a dead-letter/error prefix (`errors/`). Put a mix of well-formed JSON records and records that are not valid JSON. Within the destination's data-freshness window, objects carrying the unparseable records must appear under `errors/`; observed nothing appears there for 600 s.
2. Cause 2: call the channel-create operation with `s3DestinationConfiguration.storageConfiguration.expectedBucketOwner` set to an 11-digit string. The refusal must be ValidationException whose two clauses each begin `Value at 's3DestinationConfiguration.storageConfiguration.expectedBucketOwner' failed to satisfy constraint: ...`; observed each clause begins `Value '<the 11 digits>' at '...'`.
