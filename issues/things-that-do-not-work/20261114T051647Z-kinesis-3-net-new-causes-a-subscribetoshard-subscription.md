Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.subscribe-to-shard-lease-expires-after-5-minutes kinesis.subscribe-to-shard-subscription-expires-after-max-duration kinesis.subscribe-to-shard-second-subscription-rejected kinesis.subscribe-to-shard-honors-starting-position-latest kinesis.subscribe-to-shard-missing-permission-denied kinesis.subscribe-to-shard-without-permission-denied kinesis.subscribe-to-shard-without-permission-must-be-denied kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.subscribe-to-shard-lease-expires-after-5-minutes|run#24|AssertionError|assert 200.0 <= elapsed <= 340.0, (
Issue-Key: kinesis.subscribe-to-shard-subscription-expires-after-max-duration|run#18|AssertionError|assert 60 <= elapsed <= 400, f"subscription closed after an implausible duration: {elapsed}s"
Issue-Key: kinesis.subscribe-to-shard-second-subscription-rejected|run#13|AssertionError|assert quiet_events, (
Issue-Key: kinesis.subscribe-to-shard-honors-starting-position-latest|run#24|AssertionError|assert after_payload in observed_data, f"post-subscription record never delivered: {observed_data}"
Issue-Key: kinesis.subscribe-to-shard-missing-permission-denied|_error#15|AssertionError|raise AssertionError(f"expected the call to be denied, but it succeeded with {result!r}")
Issue-Key: kinesis.subscribe-to-shard-without-permission-denied|_poll_for_expected_denial#19|AssertionError|raise AssertionError(f"SubscribeToShard succeeded for a principal with no permissions: {result!r}")
Issue-Key: kinesis.subscribe-to-shard-without-permission-must-be-denied|_error#18|AssertionError|raise AssertionError(f"subscribe_to_shard unexpectedly succeeded for an unauthorized user: {result!r}")
Issue-Key: kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards|run#23|AssertionError|assert final.get("ContinuationSequenceNumber") is None, (
Issue-Commit: 38eb3eea268d26fefced1d4090ebb24025af21c0

# kinesis: 3 net-new causes — a SubscribeToShard subscription ends immediately instead of being held open for its ~5-minute window, the operation is served to principals with no kinesis:SubscribeToShard permission, and the terminal event of a closed shard still carries a ContinuationSequenceNumber

> _Note: this issue was identified against the code base at commit `38eb3eea268d26fefced1d4090ebb24025af21c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubscribeToShard

## Description
Three distinct root causes in the kinesis emulator's enhanced-fan-out data plane, all grounded in failing behavioral checks of this suite. None of the eight checks is marked ALREADY FILED or OWNED in the replay report. The open issue 20261027T201859Z-kinesis-4-net-new-root-causes-subscribetoshard-is.md lists several of these check names, but its cause is "SubscribeToShard answers 501 NotImplemented / is not implemented at all", which no longer reproduces: the operation now answers HTTP 200 with an event-stream body. The causes below are the behaviours that diverge once it does answer, and none of them is covered by an open issue.

=== Cause 1 of 3: the subscription is a single response that ends at once, so no window, no continuous push, and LATEST never delivers ===
AWS: Smithy: SubscribeToShard output { EventStream: SubscribeToShardEventStream, required, @streaming }, a tagged union whose SubscribeToShardEvent is documented as what "Kinesis Data Streams sends ... over an HTTP/2 connection to your consumer" after "the connection is successfully established". AWS holds that connection open and pushes a SubscribeToShardEvent roughly every second — including empty-Records events on an idle shard, which is why ContinuationSequenceNumber "captures your shard progress even when no data is written to the shard" — and the service itself tears the subscription down after its documented maximum window of about 5 minutes (the recordings accept 200-340 s, and at least 60 s). StartingPosition {Type: LATEST} therefore delivers records PUT AFTER the subscription was established, because the connection is still open when they arrive.
Emulator: the event stream ends immediately after a single frame. The lease checks measure the stream's lifetime as 0.0 s and 0.00011876 s where AWS gives roughly 300 s; an uncontested subscription to an idle shard produces no further events at all within 10 s; and a LATEST subscription delivers an empty record set ([]) because nothing is pushed after the one-shot read. The service crate's own handler docs state the design explicitly: "The subscription this emulator serves is ONE response rather than a connection held open" (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/subscribe_to_shard.rs, lines 17-29).
Why: every enhanced-fan-out consumer (the KCL's default path) is a long-lived push loop. A client gets one burst and then silence: tail-reading with LATEST returns nothing, idle-shard checkpointing is impossible because no periodic event arrives, and code that waits for the service to end the subscription before renewing either spins or renews hundreds of times per minute. Nothing about the 5-minute window or the one-subscription-per-consumer-and-shard contract is observable.
Fix: the SubscribeToShard handler and the event-stream response path it uses (services/.../src/ops/subscribe_to_shard.rs plus the service_common::event_stream framing and the routing seam in src/service.rs) must hold the response open for the documented window, emitting a SubscribeToShardEvent per interval — empty Records included — from the shared shard read, and close it when the window expires.
Traces: kinesis.subscribe-to-shard-lease-expires-after-5-minutes, kinesis.subscribe-to-shard-subscription-expires-after-max-duration, kinesis.subscribe-to-shard-second-subscription-rejected, kinesis.subscribe-to-shard-honors-starting-position-latest.

=== Cause 2 of 3: SubscribeToShard performs no authorization check and serves an unprivileged principal ===
AWS: Smithy: SubscribeToShard models AccessDeniedException (HTTP 400) — "Specifies that you do not have the permissions required to perform this operation." A caller whose IAM principal was never granted kinesis:SubscribeToShard (an IAM user with no attached policy, or a role whose policy omits that action) is refused with HTTP 400, error code 'AccessDeniedException', before any data is streamed; the same call by the permitted caller succeeds and delivers the record. AWS's message for this class is the standard authorization sentence "User: <caller principal ARN> is not authorized to perform: kinesis:SubscribeToShard on resource: <stream or consumer ARN> because no identity-based policy allows the kinesis:SubscribeToShard action" — the fixed text is that frame, and the variable parts are the caller's principal ARN and the target resource ARN taken from the request and the emulator's IAM state.
Emulator: the unprivileged call is ACCEPTED. It answers HTTP 200 with content-type application/vnd.amazon.eventstream and a populated EventStream, and the records of the shard are handed to the caller (one check captured the delivered record payload b'sim2real-behav-efoperm-probe'). No AccessDeniedException is ever produced, for either the no-policy IAM user or the role missing the action.
Why: this is overly permissive on the read path of stream data — an identity that an AWS deployment would refuse can read every record on the shard. A client or test that proves least-privilege (asserting the denial, or asserting that a consumer role must be granted kinesis:SubscribeToShard) passes against the emulator and fails in AWS, and the modeled AccessDeniedException branch of the operation is unreachable.
Fix: the authorization pre-flight of the SubscribeToShard handler (services/.../src/ops/subscribe_to_shard.rs) — it must consult the caller's identity-based policy the way the other kinesis data-plane handlers do and refuse with the 400 AccessDeniedException before establishing the stream.
Traces: kinesis.subscribe-to-shard-missing-permission-denied, kinesis.subscribe-to-shard-without-permission-denied, kinesis.subscribe-to-shard-without-permission-must-be-denied.

=== Cause 3 of 3: the terminal event of a closed (resharded) parent shard still carries a ContinuationSequenceNumber ===
AWS: Smithy: SubscribeToShardEvent members Records (required), ContinuationSequenceNumber (required, pattern ^(0|([1-9]\d{0,128}))$) — "Use this as SequenceNumber in the next call to SubscribeToShard" — MillisBehindLatest (required), and ChildShards, "the list of the child shards of the current shard, returned only at the end of the current shard". Recorded behavior: on a shard that a SplitShard closed, once the subscription has delivered the parent's retained records it emits a terminal event carrying ChildShards, and that event's ContinuationSequenceNumber is NULL/absent — a closed shard's end has nowhere to continue from, which is the signal the consumer uses to stop reading the parent and start on the children.
Emulator: the terminal ChildShards event is emitted (the parent's records and the child shard ids match ListShards), but it still carries a ContinuationSequenceNumber value, so the event looks like an ordinary mid-shard checkpoint.
Why: a resharding-aware consumer decides "parent exhausted, move to children" from the absence of a continuation token. With one present, it keeps resubscribing to the closed parent from that token and never advances to the child shards, which stalls the consumer after every split or merge.
Fix: the end-of-shard branch of the SubscribeToShard handler (services/.../src/ops/subscribe_to_shard.rs) and its SubscribeToShardEvent serialization in src/ops/types/subscribe_to_shard_types.rs — when the subscription has reached the end of a closed shard and is emitting ChildShards, the continuation member must be left unset rather than filled from the reader position.
Traces: kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards.

## Evidence
Cause 1 — subscription ends immediately (4 checks):
  kinesis.subscribe-to-shard-lease-expires-after-5-minutes: AssertionError: subscription lease lasted 0.0s, expected roughly the documented 5-minute (300s) window (assert 200.0 <= elapsed <= 340.0)
  kinesis.subscribe-to-shard-subscription-expires-after-max-duration: AssertionError: subscription closed after an implausible duration: 0.00011876000007760013s (assert 60 <= elapsed <= 400)
  kinesis.subscribe-to-shard-second-subscription-rejected: AssertionError: an uncontested subscription must keep pushing events (Kinesis emits one roughly every second even on an idle shard); none arrived in 10s
  kinesis.subscribe-to-shard-honors-starting-position-latest: AssertionError: post-subscription record never delivered: []
  Expected: one HTTP 200 event stream held open ~200-340 s, emitting a SubscribeToShardEvent about every second (empty Records allowed), and delivering records PutRecord'd after a LATEST subscription was established.

Cause 2 — authorization not enforced (3 checks):
  kinesis.subscribe-to-shard-without-permission-denied: AssertionError: SubscribeToShard succeeded for a principal with no permissions: {'EventStream': <botocore.eventstream.EventStream ...>, 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/vnd.amazon.eventstream', ...}}}
  kinesis.subscribe-to-shard-without-permission-must-be-denied: AssertionError: subscribe_to_shard unexpectedly succeeded for an unauthorized user: {'EventStream': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/vnd.amazon.eventstream', ...}}}
  kinesis.subscribe-to-shard-missing-permission-denied: AssertionError: expected the call to be denied, but it succeeded with [{'SequenceNumber': ..., 'Data': b'sim2real-behav-efoperm-probe', 'PartitionKey': 'pk', 'EncryptionType': 'NONE'}]
  Expected: HTTP 400 with error code AccessDeniedException and no records delivered; the permitted caller's identical call still returns 200 and the record.

Cause 3 — terminal event keeps its continuation token (1 check):
  kinesis.deep-kinesis-resharding-efo-subscription-on-split-parent-ends-with-child-shards: AssertionError: the terminal event still carries ContinuationSequenceNumber '<sequence number>'; a closed shard's end has nowhere to continue
  Expected: the ChildShards-bearing terminal event of the split parent has no ContinuationSequenceNumber. (The sequence number in the message is a per-run value.)

## Steps to Reproduce
Common setup: CreateStream(ShardCount=1), wait ACTIVE, read StreamARN (DescribeStreamSummary) and ShardId (ListShards), RegisterStreamConsumer and wait for the consumer to be ACTIVE.

Cause 1: SubscribeToShard(ConsumerARN, ShardId, StartingPosition={'Type':'LATEST'}) and time how long the event stream stays open while counting events — it ends immediately with no further events, instead of pushing an event about every second for ~300 s. With the subscription open, PutRecord a new payload and observe it is never delivered (observed data []). Repeat with TRIM_HORIZON on an idle shard and observe no periodic events within 10 s.

Cause 2: create an IAM user (or role) with no attached policy, build a kinesis client from its credentials, and call SubscribeToShard(ConsumerARN, ShardId, StartingPosition={'Type':'TRIM_HORIZON'}). Observe HTTP 200 and the shard's records instead of a 400 AccessDeniedException; the admin caller's identical call also succeeds (as it should).

Cause 3: PutRecord several records under one partition key, SplitShard(ShardToSplit=<parent>, NewStartingHashKey=2**127), wait for ListShards to show two children of the parent, PutRecord until a write lands on a child, then SubscribeToShard to the PARENT shard and read until an event carries ChildShards. Inspect that terminal event: ContinuationSequenceNumber is present where AWS leaves it unset.
