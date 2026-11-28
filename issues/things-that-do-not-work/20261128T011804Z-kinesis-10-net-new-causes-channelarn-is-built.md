Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.describe-channel-echoes-every-s3-destination-member-set-at-create kinesis.list-channels-summary-agrees-with-describe-channel kinesis.create-channel-with-both-s3-and-s3-tables-destinations-rejected kinesis.s3-channel-refuses-second-source-stream kinesis.s3-tables-channel-refuses-second-table-configuration kinesis.second-s3-channel-on-stream-refused-until-first-deleted kinesis.get-shard-iterator-burst-on-one-shard-throttled-with-exact-message kinesis.update-stream-mode-third-switch-within-a-day-rejected kinesis.stale-shard-iterator-and-sequence-number-refused-after-stream-name-reused kinesis.stop-stream-encryption-leaves-earlier-records-encrypted-and-readable kinesis.subscribe-to-shard-sequence-type-without-number-rejected kinesis.subscribe-to-shard-open-subscription-ends-not-found-when-stream-deleted
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.describe-channel-echoes-every-s3-destination-member-set-at-create|_assert_channel_ids#23|AssertionError|assert created["ChannelARN"] == expected, (
Issue-Key: kinesis.list-channels-summary-agrees-with-describe-channel|_assert_channel_ids#12|AssertionError|assert created["ChannelARN"] == expected, (
Issue-Key: kinesis.create-channel-with-both-s3-and-s3-tables-destinations-rejected|_assert_channel_ids#21|AssertionError|assert created["ChannelARN"] == expected, (
Issue-Key: kinesis.s3-channel-refuses-second-source-stream|run#28|AssertionError|assert code == _TWO_STREAMS_CODE, f"two-stream channel refused with {code!r}: {message!r}"
Issue-Key: kinesis.s3-tables-channel-refuses-second-table-configuration|run#42|AssertionError|raise AssertionError(f"a channel naming two tables was accepted: {body['ChannelDescription']!r}")
Issue-Key: kinesis.second-s3-channel-on-stream-refused-until-first-deleted|run#33|AssertionError|raise AssertionError(f"a second S3 channel on the stream was accepted: {extra['ChannelDescription']!r}")
Issue-Key: kinesis.get-shard-iterator-burst-on-one-shard-throttled-with-exact-message|run#16|AssertionError|assert refused, (
Issue-Key: kinesis.update-stream-mode-third-switch-within-a-day-rejected|expect_error#7|AssertionError|raise AssertionError(f"expected the call to be refused with {code!r}, but it succeeded: {result!r}")
Issue-Key: kinesis.stale-shard-iterator-and-sequence-number-refused-after-stream-name-reused|run#29|AssertionError|assert observed == expected, f"refusals were {observed!r}"
Issue-Key: kinesis.stop-stream-encryption-leaves-earlier-records-encrypted-and-readable|run#32|AssertionError|assert record.get("EncryptionType") == ack.get("EncryptionType"), (
Issue-Key: kinesis.subscribe-to-shard-sequence-type-without-number-rejected|run#19|AssertionError|assert message == want, f"refusal message was {message!r}, expected {want!r}"
Issue-Key: kinesis.subscribe-to-shard-open-subscription-ends-not-found-when-stream-deleted|_healthy_events#12|AssertionError|raise AssertionError(f"the subscription ended after {len(seen)} events on a live stream")
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# kinesis: 10 net-new causes — ChannelARN is built from the channel NAME, CreateChannel's one-stream/one-table/one-S3-channel rules are unenforced or misclassified, GetShardIterator never throttles, UpdateStreamMode has no 24-hour switch limit, stale-iterator refusals and SubscribeToShard behavior diverge

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Ten independent root causes in the Kinesis control and data planes, each with its own failing check ids. None of these checks is marked ALREADY FILED or OWNED in the behavioral report.

=== Cause 1 of 10: ChannelARN names the channel by NAME, not by ChannelId ===
AWS: Smithy: ChannelDescription carries both ChannelId (string, @length 1-64) and ChannelARN (string, @length 1-2048, @pattern "^arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+$"). Recorded behavior: the ARN's resource segment is the generated ChannelId — arn:aws:kinesis:<region>:<account>:channel/<ChannelId> — and the later ResourceNotFoundException for a deleted channel also names it by id ("Channel <ChannelId> ... not found.").
Emulator: builds arn:aws:kinesis:<region>:<account>:channel/<ChannelName> while separately returning a 32-character hex ChannelId, so the ARN and the id name different things on CreateChannel, DescribeChannel and ListChannels alike.
Why: the ARN is the channel's identity in IAM policies, tagging and cross-service references; a client that derives the id from the ARN (or the ARN from the id) gets a value AWS never issues, and a channel renamed/recreated under a reused name is indistinguishable.
Fix: the channel ARN construction in the channel state/handler layer — compose the resource segment from ChannelId.
Traces: kinesis.describe-channel-echoes-every-s3-destination-member-set-at-create, kinesis.list-channels-summary-agrees-with-describe-channel, kinesis.create-channel-with-both-s3-and-s3-tables-destinations-rejected.

=== Cause 2 of 10: the one-source-stream rule is refused with the wrong error class and invented prose ===
AWS: Smithy: CreateChannel.StreamConfigurationList @required targets ChannelStreamConfigurationList (@length 1-10000), so the "exactly one stream" rule is a runtime rule; CreateChannel binds ValidationException and InvalidArgumentException (both 400). Recorded behavior: a request naming two source streams is refused HTTP 400 ValidationException with the entirely fixed text "StreamConfigurationList should contain exactly 1 stream.", and no channel is created.
Emulator: refuses, but as InvalidArgumentException with "StreamConfigurationList names 2 source streams; currently 1 stream is supported per channel" — wrong error code and emulator-authored wording (the "2" is a value from the request, not part of AWS' template, which has no slot for it).
Why: clients branch on the error code, and the sentence is the contract for the class.
Fix: the StreamConfigurationList cardinality check in CreateChannel's validation.
Traces: kinesis.s3-channel-refuses-second-source-stream.

=== Cause 3 of 10: the one-table rule for an S3 Tables destination is not enforced ===
AWS: Smithy: CreateChannel.S3TablesDestinationConfiguration.S3TablesConfigurationList is a list; the "exactly one table configuration" rule is a runtime rule, and CreateChannel binds ValidationException (400). Recorded behavior: HTTP 400 ValidationException "S3TablesConfigurationList should contain exactly 1 table configuration.", no channel on the stream and no table created in the namespace.
Emulator: ACCEPTS the request — CreateChannel returns a ChannelDescription in CREATING whose S3TablesConfigurationList holds both table configurations.
Why: a client that relies on AWS rejecting the request gets a channel that cannot exist in AWS, and its later DescribeChannel/ListChannels state diverges permanently.
Fix: the S3TablesConfigurationList cardinality check in CreateChannel's validation.
Traces: kinesis.s3-tables-channel-refuses-second-table-configuration.

=== Cause 4 of 10: the one-S3-channel-per-stream limit is not enforced ===
AWS: Smithy: CreateChannel binds LimitExceededException (client error, 400). Recorded behavior: a second channel with an S3 destination on a stream that already has one is refused HTTP 400 LimitExceededException with "This request exceeds the limit on the number of channels with destination type S3 that can be associated with stream <the source stream's ARN>. Limit: 1." — fixed text apart from the stream ARN taken from the request; while refused, ListChannels for that stream still lists only the first channel, and the second request succeeds once the first channel is deleted.
Emulator: ACCEPTS the second channel (returns a CREATING ChannelDescription), so the stream carries two S3 channels.
Why: the delivery topology a client builds against the emulator is one AWS refuses, and no LimitExceededException is ever surfaced for this limit.
Fix: the per-stream destination-type limit check in CreateChannel's validation/state.
Traces: kinesis.second-s3-channel-on-stream-refused-until-first-deleted.

=== Cause 5 of 10: GetShardIterator is never rate-limited per shard ===
AWS: Smithy: GetShardIterator binds ProvisionedThroughputExceededException (client error, 400) and AWS documents five GetShardIterator transactions per second per shard. Recorded behavior: a bounded burst (60 calls in waves of 10 against one shard) is partly accepted and partly refused with ProvisionedThroughputExceededException and the message "Rate exceeded for shard <shardId> in stream <streamName> under account <accountId>." — fixed text apart from the shard id, stream name and account id, which come from the request and the emulator's state; once the rate falls, the identical request returns a usable iterator.
Emulator: all 60 calls in the burst succeed; the throttle is never raised.
Why: consumer libraries exist to back off on this exact refusal; against the emulator that path is dead code, and a client's retry/backoff logic is never exercised or validated.
Fix: the per-shard request-rate accounting behind GetShardIterator (and the throttling error it raises). Open issue 20261027T011654Z-kinesis-9-net-new-root-causes-cloudformation-aws.md owns the equivalent GetRecords 5-TPS failure; it does not cover GetShardIterator and does not carry this check.
Traces: kinesis.get-shard-iterator-burst-on-one-shard-throttled-with-exact-message.

=== Cause 6 of 10: UpdateStreamMode allows unlimited capacity-mode switches in 24 hours ===
AWS: Smithy: UpdateStreamMode binds LimitExceededException (client error, 400); AWS allows at most two UpdateStreamMode requests per stream in a rolling 24-hour window. Recorded behavior: the third switch within a day is refused HTTP 400 LimitExceededException.
Emulator: the third UpdateStreamMode answers HTTP 200 with an empty body and applies the switch.
Why: a client that must handle the rolling-window limit (queueing or surfacing the refusal) never sees it, and capacity-mode state diverges from what AWS would hold.
Fix: UpdateStreamMode's handler/state — record the per-stream switch history and raise LimitExceededException past the second switch in 24 hours.
Traces: kinesis.update-stream-mode-third-switch-within-a-day-rejected.

=== Cause 7 of 10: stale iterators and sequence numbers are refused with the wrong class, shape and wording ===
AWS: Smithy: GetRecords and GetShardIterator bind ResourceNotFoundException and InvalidArgumentException (both 400). Recorded behavior, after the stream is deleted and a NEW stream takes the same name: (a) GetRecords with the deleted stream's iterator → ResourceNotFoundException "Shard <shardId> in stream <stream name> under account <account id> does not exist"; (b) the same iterator against the re-created stream → InvalidArgumentException "StartingSequenceNumber used in GetRecords on shard <shardId> in stream <stream name> under account <account id> is invalid because it did not come from this stream." (no sequence number rendered); (c) GetShardIterator with the old stream's sequence number → InvalidArgumentException "StartingSequenceNumber <the sequence number> used in GetShardIterator on shard <shardId> in stream <stream name> under account <account id> is invalid because it did not come from this stream." Fixed text as quoted; the variable parts are the shard id, stream name, account id and (only in (c)) the submitted sequence number.
Emulator: (a) and (b) both answer ResourceNotFoundException "Stream <stream name> under account <account id> not found." — a stream-level miss where AWS reports the shard, and no foreign-iterator detection at all for (b); (c) answers InvalidArgumentException "StartingSequenceNumber <sequence number> is not a sequence number of shard <shardId>", invented prose that names neither the stream nor the account and does not state that the value came from another stream.
Why: a consumer that has checkpointed against a deleted stream cannot tell "this shard is gone" from "this checkpoint belongs to another incarnation of the name", and the (b) case is not refused on the right grounds at all.
Fix: the iterator/sequence-number provenance check in the GetRecords and GetShardIterator handlers, and the messages they raise.
Traces: kinesis.stale-shard-iterator-and-sequence-number-refused-after-stream-name-reused.

=== Cause 8 of 10: GetRecords stamps EncryptionType NONE on records PutRecord acknowledged without one ===
AWS: Smithy: Record.EncryptionType is an optional EncryptionType enum (NONE|KMS). Recorded behavior: after StopStreamEncryption the stream reports EncryptionType NONE and names no key, PutRecord's acknowledgement for a subsequent write carries NO EncryptionType member, and GetRecords returns that record with NO EncryptionType member either — the read-back value matches what the write was acknowledged with (and a record written while SSE-KMS was on still reads back as KMS).
Emulator: PutRecord correctly omits the member, but GetRecords returns the same record with EncryptionType "NONE".
Why: a consumer that keys on the presence of Record.EncryptionType (e.g. to decide whether a payload came from an encrypted window of the stream) classifies every post-stop record differently than against AWS.
Fix: the Record serializer in GetRecords (and the per-record encryption state it reads) — omit EncryptionType for records written while the stream was unencrypted.
Traces: kinesis.stop-stream-encryption-leaves-earlier-records-encrypted-and-readable.

=== Cause 9 of 10: SubscribeToShard's starting-position refusal names the wrong member in its message ===
AWS: Smithy: SubscribeToShard.StartingPosition targets StartingPosition{Type, SequenceNumber, Timestamp}; the Type/SequenceNumber pairing is a runtime rule and the operation binds InvalidArgumentException (400). Recorded behavior: AT_SEQUENCE_NUMBER with no sequence number is refused with "Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and StartingSequenceNumber or (2) TRIM_HORIZON or LATEST and no StartingSequenceNumber. Request specified AT_SEQUENCE_NUMBER and no StartingSequenceNumber." — the sentence names the member "StartingSequenceNumber", and the only variable part is the Type the request specified.
Emulator: the same sentence with every occurrence rewritten to "StartingPosition.SequenceNumber".
Why: the message is the contract for this error class; anything matching or displaying AWS' sentence fails.
Fix: the starting-position pairing check behind SubscribeToShard (and GetShardIterator, which shares the wording).
Traces: kinesis.subscribe-to-shard-sequence-type-without-number-rejected.

=== Cause 10 of 10: an open SubscribeToShard event stream ends on its own on a live stream, so the stream-deleted in-band error never arrives ===
AWS: Smithy: SubscribeToShard returns an event stream (SubscribeToShardEventStream) that may carry ResourceNotFoundException as an in-band event. Recorded behavior: the subscription stays open and idle on a live stream (no events, no close, for at least several seconds after the backlog is delivered) and, when the stream is deleted while it is open, terminates with an in-band ResourceNotFoundException event "Shard <shardId> in stream <stream name> under account <account id> does not exist" and no re-delivered records.
Emulator: the event stream CLOSES by itself after delivering a handful of events while the stream is still live, so the deletion-triggered in-band error is never observed.
Why: an enhanced-fan-out consumer treats a clean end-of-stream as "resubscribe" and never learns the shard/stream is gone; it also re-reads the shard in a tight loop because each subscription ends immediately.
Fix: the SubscribeToShard event-stream lifecycle (hold the subscription open for its window, emit the terminal in-band error on stream deletion). Open issue 20261114T051647Z-kinesis-3-net-new-causes-a-subscribetoshard-subscription.md reports the immediate-end/lease-window behavior and owns two other checks; it does not carry this check or the in-band deletion error.
Traces: kinesis.subscribe-to-shard-open-subscription-ends-not-found-when-stream-deleted.

## Evidence
Cause 1 — kinesis.describe-channel-echoes-every-s3-destination-member-set-at-create and kinesis.list-channels-summary-agrees-with-describe-channel and kinesis.create-channel-with-both-s3-and-s3-tables-destinations-rejected all fail at "ChannelARN is not this Region and account's channel/<ChannelId>" (observed ARN lengths 75/77/78). A CreateChannel response in the same run shows 'ChannelARN': 'arn:aws:kinesis:<region>:<account>:channel/<ChannelName>' beside 'ChannelId': '<32 hex chars>'.

Cause 2 — kinesis.s3-channel-refuses-second-source-stream: expected ('ValidationException', 'StreamConfigurationList should contain exactly 1 stream.'); got ('InvalidArgumentException', 'StreamConfigurationList names 2 source streams; currently 1 stream is supported per channel').

Cause 3 — kinesis.s3-tables-channel-refuses-second-table-configuration: expected ('ValidationException', 'S3TablesConfigurationList should contain exactly 1 table configuration.'); got a 200 CreateChannel whose ChannelDescription.S3TablesDestinationConfiguration.S3TablesConfigurationList holds two entries (readings_primary and readings_secondary) in ChannelStatus CREATING.

Cause 4 — kinesis.second-s3-channel-on-stream-refused-until-first-deleted: expected ('LimitExceededException', 'This request exceeds the limit on the number of channels with destination type S3 that can be associated with stream <stream ARN>. Limit: 1.'); got a 200 CreateChannel for the second channel (ChannelStatus CREATING) on the same source stream.

Cause 5 — kinesis.get-shard-iterator-burst-on-one-shard-throttled-with-exact-message: "all 60 GetShardIterator calls fired in waves of 10 succeeded; expected ProvisionedThroughputExceededException past five per second on one shard". Expected refusal message: "Rate exceeded for shard <shardId> in stream <streamName> under account <accountId>."

Cause 6 — kinesis.update-stream-mode-third-switch-within-a-day-rejected: "expected the call to be refused with 'LimitExceededException', but it succeeded" — HTTPStatusCode 200, content-length 2.

Cause 7 — kinesis.stale-shard-iterator-and-sequence-number-refused-after-stream-name-reused (values templated by the check): expected {'deleted-iterator': ('ResourceNotFoundException', 'Shard <shard> in stream <stream> under account <account> does not exist'), 'reused-name-iterator': ('InvalidArgumentException', 'StartingSequenceNumber used in GetRecords on shard <shard> in stream <stream> under account <account> is invalid because it did not come from this stream.'), 'reused-name-sequence': ('InvalidArgumentException', 'StartingSequenceNumber <sequence number> used in GetShardIterator on shard <shard> in stream <stream> under account <account> is invalid because it did not come from this stream.')}; got {'deleted-iterator': ('ResourceNotFoundException', 'Stream <stream> under account <account> not found.'), 'reused-name-iterator': ('ResourceNotFoundException', 'Stream <stream> under account <account> not found.'), 'reused-name-sequence': ('InvalidArgumentException', 'StartingSequenceNumber <sequence number> is not a sequence number of shard <shard>')}.

Cause 8 — kinesis.stop-stream-encryption-leaves-earlier-records-encrypted-and-readable: "record b'plain-0' written as None reads back as 'NONE'" — PutRecord's ack omitted EncryptionType; the GetRecords record for the same SequenceNumber carried EncryptionType 'NONE'.

Cause 9 — kinesis.subscribe-to-shard-sequence-type-without-number-rejected: expected 'Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and StartingSequenceNumber or (2) TRIM_HORIZON or LATEST and no StartingSequenceNumber. Request specified AT_SEQUENCE_NUMBER and no StartingSequenceNumber.'; got the same sentence with 'StartingPosition.SequenceNumber' in place of each 'StartingSequenceNumber'.

Cause 10 — kinesis.subscribe-to-shard-open-subscription-ends-not-found-when-stream-deleted: "the subscription ended after 4 events on a live stream" — the check requires the event stream to stay open and idle for 6s before DeleteStream, then to end with the in-band ('ResourceNotFoundException', 'Shard <shardId> in stream <name> under account <account> does not exist').

## Steps to Reproduce
1. Cause 1: kinesis.create_channel(...) and compare ChannelDescription.ChannelARN with "arn:aws:kinesis:<region>:<account>:channel/" + ChannelDescription.ChannelId; repeat through DescribeChannel and ListChannels.
2. Cause 2: kinesis.create_channel with StreamConfigurationList naming two source stream ARNs — expect ValidationException 'StreamConfigurationList should contain exactly 1 stream.'
3. Cause 3: kinesis.create_channel with S3TablesDestinationConfiguration.S3TablesConfigurationList holding two table configurations — expect ValidationException 'S3TablesConfigurationList should contain exactly 1 table configuration.' and no channel on the stream.
4. Cause 4: create one channel with an S3DestinationConfiguration on stream S, wait for ACTIVE, then create a second one on S — expect LimitExceededException naming S's ARN and 'Limit: 1.'; delete the first and retry (must then succeed).
5. Cause 5: on a 1-shard PROVISIONED stream, fire 60 GetShardIterator(AT_SEQUENCE_NUMBER) calls in waves of 10 with retries disabled — expect some ProvisionedThroughputExceededException refusals carrying "Rate exceeded for shard ... in stream ... under account ....", then recovery after backoff.
6. Cause 6: UpdateStreamMode on one stream three times inside 24 hours (ON_DEMAND→PROVISIONED→ON_DEMAND→PROVISIONED) — expect the third to be refused LimitExceededException.
7. Cause 7: put a record, take an AT_SEQUENCE_NUMBER iterator, DeleteStream, then GetRecords with that iterator; re-create the stream under the same name, put a record, then GetRecords with the stale iterator and GetShardIterator with the old sequence number; compare all three codes and messages.
8. Cause 8: StartStreamEncryption(KMS), write until the ack reports KMS, StopStreamEncryption, write until the ack carries no EncryptionType, then GetRecords from TRIM_HORIZON and check that the post-stop record carries no EncryptionType member.
9. Cause 9: kinesis.subscribe_to_shard(ConsumerARN=C, ShardId=S, StartingPosition={'Type':'AT_SEQUENCE_NUMBER'}) and read Error.Message.
10. Cause 10: register a consumer, put one record, SubscribeToShard, read the first event, then hold the stream open for 6s (expect no further events and no close); DeleteStream and expect the in-band ResourceNotFoundException event.
11. Re-replay every check id in trace_ids.
