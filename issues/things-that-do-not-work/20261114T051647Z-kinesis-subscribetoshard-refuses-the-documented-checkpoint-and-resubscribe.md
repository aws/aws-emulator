Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.subscribe-to-shard-expires-after-five-minutes
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|SubscribeToShard|ResourceInUseException
Issue-Commit: 38eb3eea268d26fefced1d4090ebb24025af21c0

# kinesis: SubscribeToShard refuses the documented checkpoint-and-resubscribe renewal with ResourceInUseException once a consumer has subscribed to a shard

> _Note: this issue was identified against the code base at commit `38eb3eea268d26fefced1d4090ebb24025af21c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubscribeToShard

## Description
REOPENS 20261029T174012Z-kinesis-subscribetoshard-answers-501-notimplemented-on-the-wire.md: its fix left the call refused. SubscribeToShard no longer answers 501, but the second (renewal) SubscribeToShard call of the documented enhanced-fan-out consumer loop is now refused with a 400 ResourceInUseException, so the operation still cannot be used end to end.

Cause 1 of 1: a resubscribe to the same ConsumerARN+ShardId is refused even though the previous subscription has already terminated.

AWS: Smithy: SubscribeToShard input { ConsumerARN (required), ShardId (required), StartingPosition (required), DryRun }, output { EventStream: SubscribeToShardEventStream, required, @streaming }; modeled errors AccessDeniedException, DryRunOperationException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all HTTP 400). SubscribeToShardEvent.ContinuationSequenceNumber is documented as the checkpoint to "use as SequenceNumber in the next call to SubscribeToShard, with StartingPosition set to AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER". AWS's ResourceInUseException for this operation guards an ACTIVE subscription being contended: once a consumer's event stream has ended (the service tore it down at the end of its window, or the client closed the HTTP/2 stream), the consumer resubscribes from its checkpoint and the call is ACCEPTED with HTTP 200 and an event stream. The recording this check replays performs exactly that sequence — consume one event, close the event stream, PutRecord, SubscribeToShard again with StartingPosition {Type: AFTER_SEQUENCE_NUMBER, SequenceNumber: <ContinuationSequenceNumber from the prior event>} — and AWS serves the renewal, delivering only records written after that checkpoint.

Emulator: the renewal call is refused with HTTP 400, error code 'ResourceInUseException' and message "Consumer <consumerARN> is already subscribed to shard <shardId>; you can call SubscribeToShard again for the same consumer and shard 5 seconds or more after a successful call". Fixed text: "Consumer ", " is already subscribed to shard ", "; you can call SubscribeToShard again for the same consumer and shard 5 seconds or more after a successful call"; the variable parts are the consumer ARN and the shard id from the request. No subscription is in fact alive at that moment — the emulator's subscription is a single response that has already completed and the client closed it — so the refusal is unconditional for any consumer that resubscribes promptly.

Why: the enhanced-fan-out consumer loop (the KCL's default read path) is checkpoint-then-resubscribe. A client that closes a finished subscription and immediately resumes from its ContinuationSequenceNumber gets a hard 400 that SDKs surface as a non-retryable resource conflict, so the consumer stalls and no record written after the first event is ever read. The emulator also teaches that a terminated subscription keeps holding its shard.

Fix: the SubscribeToShard handler's pre-flight contention check in services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/subscribe_to_shard.rs, together with the per-consumer subscription bookkeeping it judges against in that crate's state module — the recorded marker must represent a subscription that is still live (and be released when the subscription ends or the client closes it), not the timestamp of the last successful call.

Traces: kinesis.subscribe-to-shard-expires-after-five-minutes (the handler's own module docs, lines 41-46, describe recording the subscription on ConsumerRecord::subscribed_at and judging the next call against it).

## Evidence
Check kinesis.subscribe-to-shard-expires-after-five-minutes:
  botocore.errorfactory.ResourceInUseException: An error occurred (ResourceInUseException) when calling the SubscribeToShard operation: Consumer arn:aws:kinesis:us-west-2:000000000000:stream/<stream>/consumer/<consumer>:<ts> is already subscribed to shard shardId-000000000000; you can call SubscribeToShard again for the same consumer and shard 5 seconds or more after a successful call

The call that was refused is the renewal, issued AFTER first["EventStream"].close():
  first = subscribe_to_shard(ConsumerARN=..., ShardId=..., StartingPosition={'Type':'TRIM_HORIZON'})  -> 200, one event consumed, ContinuationSequenceNumber captured
  first['EventStream'].close()
  put_record(Data=b'second-record')
  resumed = subscribe_to_shard(ConsumerARN=..., ShardId=..., StartingPosition={'Type':'AFTER_SEQUENCE_NUMBER','SequenceNumber':<continuation>})

Expected: HTTP 200 with an application/vnd.amazon.eventstream body delivering 'second-record' and NOT redelivering 'first-record'.
Actual: HTTP 400 ResourceInUseException; no records are delivered at all.
(The account id, stream/consumer names, shard id and sequence numbers above are per-run values; the requirement is that the renewal is accepted.)

## Steps to Reproduce
1. CreateStream(ShardCount=1), wait for ACTIVE; read StreamARN via DescribeStreamSummary and the ShardId via ListShards.
2. RegisterStreamConsumer, wait for the consumer to become ACTIVE.
3. PutRecord(Data=b'first-record').
4. SubscribeToShard(ConsumerARN, ShardId, StartingPosition={'Type':'TRIM_HORIZON'}); read one SubscribeToShardEvent, keep its ContinuationSequenceNumber, then close the event stream.
5. PutRecord(Data=b'second-record').
6. SubscribeToShard(ConsumerARN, ShardId, StartingPosition={'Type':'AFTER_SEQUENCE_NUMBER','SequenceNumber':<continuation from step 4>}).
Observe the 400 ResourceInUseException instead of a 200 event stream carrying only 'second-record'.
