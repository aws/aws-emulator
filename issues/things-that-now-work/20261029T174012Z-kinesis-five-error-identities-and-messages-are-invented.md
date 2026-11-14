Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-arn-based-error-paths-and-account-settings
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-arn-based-error-paths-and-account-settings kinesis-duplicate-create-and-delete-nonexistent-stream kinesis-stream-mutation-error-paths kinesis-get-shard-iterator-types-and-errors kinesis-stream-not-found-error-paths kinesis-list-streams-pagination-error kinesis-put-record-field-validation-errors
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|UpdateStreamWarmThroughput|4xx->4xx|$.__type
Issue-Key: kinesis|UpdateStreamWarmThroughput|4xx->4xx|$.message
Issue-Key: kinesis|CreateStream|4xx->4xx|$.message
Issue-Key: kinesis|GetShardIterator|4xx->4xx|$.message
Issue-Key: kinesis|ListStreams|4xx->4xx|$.__type
Issue-Key: kinesis|ListStreams|4xx->4xx|$.message
Issue-Key: kinesis|PutRecord|4xx->4xx|$.message
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesis: five error identities and messages are invented instead of AWS's codes and fixed templates

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStreamWarmThroughput

## Description
Five net-new root causes, all of the same family: the emulator returns a different error identity and/or a locally invented message where AWS returns a specific code and a fixed message template. None is marked ALREADY FILED or OWNED in the replay report. The open issue 20261029T140704Z-kinesis-liststreamconsumers-misclassifies-a-malformed-nexttoken-as-expirednexttokenexception reports the same misclassification for ListStreamConsumers; Cause 4 below is the ListStreams occurrence, a different operation and a different handler, so it is not covered by it — but the two should be fixed together in the shared pagination-token path.

=== Cause 1 of 5: UpdateStreamWarmThroughput resolves the stream before checking account enablement, so the account-level refusal is reported as ResourceNotFoundException ===
AWS: Smithy: UpdateStreamWarmThroughput { StreamARN, StreamName, StreamId, WarmThroughputMiBps (required, range min 0) } -> { StreamARN, StreamName, WarmThroughput }; errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException, ValidationException (all HTTP 400). Recorded behavior: for an account that is not enabled for the minimum-throughput billing commitment, AWS refuses the call with 400 ValidationException and the fixed message "Account <accountId> is not enabled for minimum throughput billing commitment" — the only variable part is the 12-digit account id taken from the caller's credentials — and it does so regardless of whether the named stream exists, i.e. the account-level precondition is evaluated before the stream is resolved.
Emulator: resolves the stream first and answers 400 ResourceNotFoundException "Stream <streamName> under account <accountId> not found.", so the warm-throughput account precondition is never evaluated and its ValidationException is unreachable.
Why: a client cannot distinguish "this feature is not enabled for my account" (an account/billing action) from "my stream name is wrong" (a request fix); it will chase a nonexistent naming bug. The ordering also means the account check does not exist for streams that DO exist.
Fix: the UpdateStreamWarmThroughput handler — evaluate the account's minimum-throughput-commitment enablement (and raise ValidationException with AWS's template) before the stream lookup.
Traces: kinesis-arn-based-error-paths-and-account-settings#task#12.

=== Cause 2 of 5: CreateStream's duplicate-name refusal is reworded ===
AWS: Smithy: CreateStream { StreamName (required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), ShardCount, StreamModeDetails, ... }; errors InvalidArgumentException, LimitExceededException, ResourceInUseException, ValidationException. For a name that already exists in the account and region AWS answers 400 ResourceInUseException with the fixed template "Stream <streamName> under account <accountId> already exists." — fixed words "Stream", "under account", "already exists."; variable parts are the stream name from the request and the 12-digit account id from state.
Emulator: same status and code, but the message is a locally authored sentence: "stream <streamName> already exists in this account and region; delete it before creating another under that name" — lower-case 'stream', no account id in AWS's position, and advisory prose AWS never emits.
Why: idempotent create-if-absent logic and tests commonly match this message to confirm the already-exists cause (ResourceInUseException is shared with the not-ACTIVE cases), and they do not match; the emulator teaches wording that does not exist in AWS.
Fix: the CreateStream handler's duplicate-name refusal message (or the shared stream-message helper that composes "under account" messages).
Traces: kinesis-duplicate-create-and-delete-nonexistent-stream#task#2 (cluster also covers kinesis-stream-mutation-error-paths#task#1).

=== Cause 3 of 5: GetShardIterator's iterator-type/sequence-number combination refusal is a different, shorter sentence ===
AWS: Smithy: GetShardIterator { StreamName, ShardId (required), ShardIteratorType (required, enum), StartingSequenceNumber (@pattern ^(0|([1-9]\\d{0,128}))$), Timestamp, ... }; errors include InvalidArgumentException (HTTP 400: "A specified parameter exceeds its restrictions, is not supported, or can't be used"). When the iterator type and StartingSequenceNumber are inconsistent, AWS answers 400 InvalidArgumentException with the fixed template "Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and StartingSequenceNumber or (2) TRIM_HORIZON or LATEST and no StartingSequenceNumber. Request specified <shardIteratorType> and <no StartingSequenceNumber | StartingSequenceNumber>." — the whole enumerating first sentence is fixed text, and the trailing "Request specified ..." clause restates the iterator type from the request and whether a sequence number was supplied.
Emulator: answers 400 with the single clause "StartingSequenceNumber is required with the <shardIteratorType> shard iterator type", dropping both the enumeration of valid combinations and the "Request specified ..." echo.
Why: the message is the only thing distinguishing this InvalidArgumentException from the many others GetShardIterator can raise; a client that parses it, or a user reading it, does not get AWS's statement of the legal combinations.
Fix: the GetShardIterator handler's argument-combination validation message.
Traces: kinesis-get-shard-iterator-types-and-errors#task#13 (cluster also covers #task#14 and kinesis-stream-not-found-error-paths#task#7).

=== Cause 4 of 5: ListStreams classifies a malformed NextToken as ExpiredNextTokenException instead of InvalidArgumentException "Invalid NextToken." ===
AWS: Smithy: ListStreams { Limit 1..10000, ExclusiveStartStreamName, NextToken (@length 1..1048576) }; errors ExpiredNextTokenException ("The pagination token passed to the operation is expired"), InvalidArgumentException, LimitExceededException. A token that is not a token this service issued is a malformed argument, not an expired one: AWS answers 400 InvalidArgumentException with the fixed message "Invalid NextToken." (complete text, no variable part). ExpiredNextTokenException is reserved for a token that WAS issued and has since aged out.
Emulator: answers 400 ExpiredNextTokenException with the invented message "the pagination token is expired or was not issued by this service; a NextToken is usable for 300 seconds after the page that returned it", collapsing the two distinct conditions into the expired one and adding prose AWS does not emit.
Why: the two codes drive different client behaviour — ExpiredNextTokenException means "restart the pagination", InvalidArgumentException means "your token is wrong, do not retry"; a paginator that auto-restarts on expiry will loop on a genuinely malformed token. Same defect as the filed ListStreamConsumers issue, so the shared token path is suspect.
Fix: the pagination-token decode path used by ListStreams (shared with ListStreamConsumers) — distinguish undecodable/unissued tokens (InvalidArgumentException "Invalid NextToken.") from decodable-but-aged tokens (ExpiredNextTokenException).
Traces: kinesis-list-streams-pagination-error#task#1.

=== Cause 5 of 5: the constraint-violation message truncates the offending value and appends a byte count AWS never emits ===
AWS: Smithy: PutRecord.PartitionKey targets PartitionKey (@length 1..256). For an over-long partition key AWS answers 400 with the standard constraint-walk message "1 validation error detected: Value '<the offending value, in full>' at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256" — the frame is fixed, and the variable parts are the rejected value (reproduced in full, however long), the member path and the limit from the model.
Emulator: emits the same frame but renders the value as the first ~255 characters followed by the literal marker "... [257 bytes]" — i.e. it truncates the rejected value and appends a byte-count annotation that is not part of AWS's template.
Why: this is the generic constraint-message renderer, so every over-long string member of every kinesis operation reports a value AWS would have echoed in full plus a trailing annotation; a client or test asserting on the rendered value never matches, and a user debugging a key cannot see the whole value that was rejected.
Fix: the framework constraint-walk/validation message renderer that formats the rejected value — echo it whole, with no truncation marker or byte count.
Traces: kinesis-put-record-field-validation-errors#task#5 (cluster also covers #task#6).

## Evidence
Cause 1 — fixture kinesis-arn-based-error-paths-and-account-settings, step task#12, UpdateStreamWarmThroughput, expected 400 / actual 400:
  $.__type: expected 'ValidationException', got 'ResourceNotFoundException'
  $.message: expected 'Account <accountId> is not enabled for minimum throughput billing commitment',
             got 'Stream <streamName> under account <accountId> not found.'

Cause 2 — fixture kinesis-duplicate-create-and-delete-nonexistent-stream, step task#2, CreateStream, 400 / 400 (code matches, message diverges):
  expected '<name> ...' -> 'Stream <streamName> under account <accountId> already exists.'
  got 'stream <streamName> already exists in this account and region; delete it before creating another under that name'

Cause 3 — fixture kinesis-get-shard-iterator-types-and-errors, step task#13, GetShardIterator, 400 / 400:
  expected 'Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and StartingSequenceNumber or (2) TRIM_HORIZON or LATEST and no StartingSequenceNumber. Request specified AT_SEQUENCE_NUMBER and no StartingSequenceNumber.'
  got 'StartingSequenceNumber is required with the AT_SEQUENCE_NUMBER shard iterator type'

Cause 4 — fixture kinesis-list-streams-pagination-error, step task#1, ListStreams, 400 / 400:
  $.__type: expected 'InvalidArgumentException', got 'ExpiredNextTokenException'
  $.message: expected 'Invalid NextToken.', got 'the pagination token is expired or was not issued by this service; a NextToken is usable for 300 seconds after the page that returned it'

Cause 5 — fixture kinesis-put-record-field-validation-errors, step task#5, PutRecord, 400 / 400:
  expected "1 validation error detected: Value '<257-character key, echoed in full>' at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256"
  got      "1 validation error detected: Value '<same key truncated>... [257 bytes]' at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256"
  (the key's characters are a value from the request; the divergence is the truncation and the '... [N bytes]' annotation in the fixed template)

## Steps to Reproduce
Cause 1: with an account not enabled for the minimum-throughput billing commitment, call UpdateStreamWarmThroughput(StreamName=<a name that does not exist>, WarmThroughputMiBps=1). Expect 400 ValidationException 'Account <accountId> is not enabled for minimum throughput billing commitment'; observe 400 ResourceNotFoundException instead. Repeat with an existing stream to confirm the account check is absent entirely.

Cause 2: CreateStream(StreamName=<name>, ShardCount=1) twice. Read Error.Message of the second call's 400 ResourceInUseException and compare with 'Stream <name> under account <accountId> already exists.'

Cause 3: CreateStream, wait for ACTIVE, then GetShardIterator(StreamName, ShardId, ShardIteratorType='AT_SEQUENCE_NUMBER') with no StartingSequenceNumber. Read the 400 InvalidArgumentException message and compare with AWS's two-clause template.

Cause 4: ListStreams(NextToken='not-a-real-token'). Read __type and message: expect InvalidArgumentException / 'Invalid NextToken.'

Cause 5: CreateStream, wait for ACTIVE, then PutRecord(StreamName, Data=b'x', PartitionKey='k'*257). Read the 400 validation message and check whether the rejected value is echoed in full or truncated with a '... [N bytes]' marker.
