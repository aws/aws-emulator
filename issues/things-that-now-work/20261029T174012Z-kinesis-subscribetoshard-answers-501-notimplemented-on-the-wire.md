Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-subscribe-to-shard-not-found-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-subscribe-to-shard-not-found-error kinesis-subscribe-to-shard-unregistered-consumer
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|SubscribeToShard|4xx->5xx|$.__type
Issue-Key: kinesis|SubscribeToShard|4xx->5xx|$.message
Issue-Key: kinesis|SubscribeToShard|4xx->5xx|status
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesis: SubscribeToShard answers 501 NotImplemented on the wire instead of its modeled 400 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubscribeToShard

## Description
One net-new root cause. It is NOT marked ALREADY FILED (stub_path is empty) and NOT marked OWNED in the replay report, so no open issue carries this fixture step's replay key; the open behavioral issue 20261027T201859Z-kinesis-4-net-new-root-causes-subscribetoshard-is describes the same missing operation from the behavioral suite (Issue-Key kinesis|SubscribeToShard|NotImplemented), and this wire finding should be fixed with it — but it adds the wire-level requirement that the *error* paths of SubscribeToShard return modeled 400 exceptions, which that issue's traces (all success/DryRun/IAM paths) do not pin down.

=== Cause 1 of 1: SubscribeToShard is not implemented, so even its not-found path returns an unmodeled HTTP 501/NotImplemented ===
AWS: Smithy: SubscribeToShard { ConsumerARN (required, @length 1..2048, @pattern ^(arn):aws.*:kinesis:.*:\\d{12}:.*stream\\/[a-zA-Z0-9_.-]+\\/consumer\\/[a-zA-Z0-9_.-]+:[0-9]+$), ShardId (required), StartingPosition (required), StreamId, DryRun } -> SubscribeToShardOutput { EventStream: SubscribeToShardEventStream, required, @streaming }; modeled errors AccessDeniedException, DryRunOperationException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException — every one of them HTTP 400. The service's protocol_config declares eventStreamHttp: h2, so a successful subscription is HTTP 200 with an application/vnd.amazon.eventstream body over HTTP/2. When the consumer ARN names a stream that does not exist, AWS answers 400 ResourceNotFoundException with the fixed template "Stream <streamName> under account <accountId> not found." (fixed words "Stream", "under account", "not found."; variable parts are the stream name parsed out of the request's ConsumerARN and the 12-digit account id). The second clustered step shows the same 501 where an unregistered consumer must likewise be refused with a modeled 400.
Emulator: every call is refused with HTTP 501, __type 'NotImplemented' and the message "operation SubscribeToShard is not implemented in this emulator" — a status outside the modeled 200/400 set and an error code that appears nowhere in the kinesis model.
Why: enhanced fan-out is entirely unavailable (the KCL's default read path reads nothing), and because 501/NotImplemented is not a modeled error, SDK error handling cannot classify it: a client probing for a missing stream or an unregistered consumer gets a transport-looking server error instead of the 400 ResourceNotFoundException it branches on, and generic retry policies may retry a permanent condition.
Fix: an operation handler for SubscribeToShard in the kinesis service crate's ops module, plus application/vnd.amazon.eventstream output framing in that service's serializer; its precondition checks must resolve the stream and the registered consumer from the ARN and raise the modeled 400 ResourceNotFoundException (AWS's "Stream <streamName> under account <accountId> not found.") before any stream is established.
Traces: kinesis-subscribe-to-shard-not-found-error#task#2 (cluster also covers kinesis-subscribe-to-shard-unregistered-consumer#task#3).

## Evidence
Fixture kinesis-subscribe-to-shard-not-found-error, step task#2, SubscribeToShard:
  status: expected 400, got 501
  $.__type: expected 'ResourceNotFoundException', got 'NotImplemented'
  $.message: expected 'Stream <streamName> under account <accountId> not found.',
             got 'operation SubscribeToShard is not implemented in this emulator'
  cluster (divergence_key service=kinesis|SubscribeToShard|status=400->501) also covers kinesis-subscribe-to-shard-unregistered-consumer#task#3, where the same 501/NotImplemented replaces that step's modeled 400.
  Expected on the success path (not exercised by this fixture, stated from the model): HTTP 200 with an application/vnd.amazon.eventstream body of SubscribeToShardEvent messages over HTTP/2.

## Steps to Reproduce
Negative path (this fixture): call SubscribeToShard(ConsumerARN='arn:aws:kinesis:<region>:<account>:stream/<a stream that does not exist>/consumer/<name>:<epoch>', ShardId='shardId-000000000000', StartingPosition={'Type': 'TRIM_HORIZON'}). Observe HTTP 501 with __type 'NotImplemented' instead of HTTP 400 ResourceNotFoundException 'Stream <streamName> under account <accountId> not found.'

Unregistered-consumer path: CreateStream, wait for ACTIVE, then call SubscribeToShard with a well-formed ConsumerARN for that stream naming a consumer that was never registered — expect a modeled 400, observe 501.

Success path: CreateStream -> RegisterStreamConsumer -> PutRecord -> SubscribeToShard(ConsumerARN=<the registered consumer>, ShardId=<the shard>, StartingPosition={'Type':'TRIM_HORIZON'}) over HTTP/2 — expect HTTP 200 and an event-stream body.
