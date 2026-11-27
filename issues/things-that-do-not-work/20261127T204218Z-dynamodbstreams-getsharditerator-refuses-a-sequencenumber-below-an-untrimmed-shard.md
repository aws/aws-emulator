Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.sequence-number-below-shard-start-reads-from-first-record
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb-streams|GetShardIterator|ValidationException
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# GetShardIterator refuses a SequenceNumber below an untrimmed shard's StartingSequenceNumber instead of positioning at the shard's first record

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
One cause.

Cause 1 of 1: AT_SEQUENCE_NUMBER / AFTER_SEQUENCE_NUMBER with a sequence number that sorts below the addressed shard's advertised StartingSequenceNumber is refused as a bad argument, instead of yielding an iterator at the shard's oldest retained record.

AWS: recorded behavior only for the positioning rule. The Smithy model (GetShardIterator) declares input SequenceNumber -> SequenceNumber @length min 21 max 40 (no ordering/range constraint relative to the shard), and models exactly three errors for the operation: InternalServerError (500), ResourceNotFoundException (400) and TrimmedDataAccessException (400) — there is no modeled error for "a sequence number the shard's range does not cover". The only documented refusal for a start point that is too old is TrimmedDataAccessException, and that is explicitly limited to "a sequence number older than the trim point (24 hours)". For an UNTRIMMED shard (nothing has aged out, so the shard still retains its first record) the recorded behavior is: a sequence number one below the shard's StartingSequenceNumber is not a trimmed read and not an invalid argument — GetShardIterator answers HTTP 200 with a ShardIterator, and GetRecords on that iterator begins at the shard's FIRST record for BOTH AT_SEQUENCE_NUMBER and AFTER_SEQUENCE_NUMBER (the record's own Keys and NEW_IMAGE are delivered). Only the position the iterator lands on is the requirement; the concrete sequence numbers and shard ids in any one run are generated values.

Emulator: the same request is refused with HTTP 400, error code ValidationException, message "Invalid SequenceNumber for the shard". The whole message is fixed text — it interpolates nothing from the request or state, so there is no per-run value in it. The refusal is produced before any record lookup: the handler classifies any number comparing Less than the shard's starting_sequence_number as "outside the shard range" and maps that classification to a validation refusal, so the call fails on the first get_shard_iterator of the check and the subsequent read of the shard's first record is never reached. Separately, ValidationException is not even one of this operation's modeled errors, so a client that handles the modeled error set cannot classify this response at all.

Why: a consumer that resumes from a checkpoint it did not mint itself — a sequence number taken from a sibling/parent shard, a persisted watermark from before the stream was re-enabled, or a conservative "start a little earlier than my last position" value — gets a hard, permanently-failing 400 instead of being positioned at the beginning of the shard. Against AWS that consumer replays the shard from its oldest record and loses nothing; against the emulator it cannot obtain an iterator for that shard at all, so every record in the shard is unreadable through that code path, and because the code is ValidationException (not TrimmedDataAccessException) a client's retry/fallback-to-TRIM_HORIZON logic keyed on the documented trimmed-data error never fires.

Fix: GetShardIterator's handler, in its sequence-number positioning (the ShardPosition classification used by the AT_/AFTER_SEQUENCE_NUMBER arm). A number below the shard's StartingSequenceNumber on a shard that has not trimmed must resolve to the shard's trim horizon (its first retained record) for both AT_ and AFTER_SEQUENCE_NUMBER rather than being classified as outside the shard's range; the "outside range"/validation refusal must not swallow this case, and TrimmedDataAccessException must stay reserved for a position that has actually fallen behind the retention trim point. The already-correct neighbouring behavior (AFTER at the record's own number skips it, AT at its own number includes it) must be preserved.

Traces: dynamodbstreams.sequence-number-below-shard-start-reads-from-first-record. Handler and classification under test: services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/ops/get_shard_iterator.rs lines 292-328 (the AT_/AFTER_SEQUENCE_NUMBER arm and its OutsideShardRange -> validation mapping), line 378 (the fixed INVALID_SEQUENCE_NUMBER message) and lines 425-441 (position_at_sequence_number, where a value comparing Less than shard.starting_sequence_number returns OutsideShardRange).

Note: the one open dynamodbstreams issue (20261029T172902Z-dynamodbstreams-describestream-projection-omits-keyschema-type-and-never-emits.md) concerns DescribeStream's StreamDescription projection (KeySchema __type, LastEvaluatedShardId) and does not cover this GetShardIterator positioning/refusal defect.

## Evidence
Failing behavioral check: dynamodbstreams.sequence-number-below-shard-start-reads-from-first-record (verdict: error).

Observed: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the GetShardIterator operation: Invalid SequenceNumber for the shard — raised at client.py:1094 (_make_api_call) on the first GetShardIterator call of the check.

Request shape that drew it: StreamArn = the LatestStreamArn of a freshly created PAY_PER_REQUEST table with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_IMAGE}; ShardId = the shard that holds the single put record; ShardIteratorType = AT_SEQUENCE_NUMBER (and, on the next iteration, AFTER_SEQUENCE_NUMBER); SequenceNumber = str(int(StartingSequenceNumber) - 1) zero-padded to the same width, i.e. exactly one below the shard's own advertised StartingSequenceNumber. The shard is untrimmed: the check first asserts the record's SequenceNumber >= the shard's StartingSequenceNumber and reads the record itself from TRIM_HORIZON, so nothing has aged out.

Expected (AWS): HTTP 200 with a ShardIterator; GetRecords on it returns the shard's first record — dynamodb.SequenceNumber equal to that record's sequence number, dynamodb.Keys == {"id": {"S": "first"}}, dynamodb.NewImage.n == {"N": "11"} — for both AT_SEQUENCE_NUMBER and AFTER_SEQUENCE_NUMBER.

Actual (emulator): HTTP 400, Error.Code = "ValidationException", Error.Message = "Invalid SequenceNumber for the shard" (fixed text, no variable parts). ValidationException is not among the operation's modeled errors (InternalServerError, ResourceNotFoundException, TrimmedDataAccessException).

Smithy: GetShardIterator input SequenceNumber -> SequenceNumber @length 21..40, not required; errors InternalServerError(500), ResourceNotFoundException(400), TrimmedDataAccessException(400). TrimmedDataAccessException is documented for "a shard iterator with a sequence number older than the trim point (24 hours)" only.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the check dynamodbstreams.sequence-number-below-shard-start-reads-from-first-record errors.
2. Manually: CreateTable with BillingMode PAY_PER_REQUEST, key schema HASH id (S), StreamSpecification {StreamEnabled: true, StreamViewType: NEW_IMAGE}; wait for table_exists.
3. DescribeStream on TableDescription.LatestStreamArn until StreamStatus is ENABLED and at least one shard serves a TRIM_HORIZON iterator; record that shard's SequenceNumberRange.StartingSequenceNumber.
4. PutItem {id: {S: "first"}, n: {N: "11"}} and read it back from the shard's TRIM_HORIZON iterator to confirm the shard is untrimmed and holds the record.
5. Call GetShardIterator with that StreamArn/ShardId, ShardIteratorType=AT_SEQUENCE_NUMBER and SequenceNumber = str(int(StartingSequenceNumber) - 1) zero-padded to the original width. Expect HTTP 200 and an iterator whose first GetRecords batch begins at the shard's first record; the emulator answers HTTP 400 ValidationException "Invalid SequenceNumber for the shard".
6. Repeat step 5 with ShardIteratorType=AFTER_SEQUENCE_NUMBER — same refusal.
7. For contrast, the same call with SequenceNumber equal to the record's own sequence number succeeds, confirming the refusal is triggered solely by the value sorting below the shard's StartingSequenceNumber.
