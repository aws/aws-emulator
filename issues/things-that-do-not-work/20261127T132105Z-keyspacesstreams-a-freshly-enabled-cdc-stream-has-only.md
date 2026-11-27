Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspacesstreams.get-stream-forged-next-token-rejected keyspacesstreams.get-stream-next-token-of-other-request-rejected keyspacesstreams.get-stream-shard-pages-list-each-shard-once keyspacesstreams.list-streams-next-token-under-changed-filter-rejected
Issue-Shape-Id: com.amazonaws.keyspacesstreams#KeyspacesStreams
Issue-Key: keyspacesstreams.get-stream-forged-next-token-rejected|run#8|AssertionError|assert len(all_ids) >= 2, f"a new CDC stream needs several shards to page; got {len(all_ids)}"
Issue-Key: keyspacesstreams.get-stream-next-token-of-other-request-rejected|run#14|AssertionError|assert token_a, f"page 1 of {len(shards_a)} shards with maxResults=1 carried no nextToken"
Issue-Key: keyspacesstreams.get-stream-shard-pages-list-each-shard-once|run#9|AssertionError|assert len(expected) >= 4, f"want at least 4 shards to page by 3, got {len(expected)}"
Issue-Key: keyspacesstreams.list-streams-next-token-under-changed-filter-rejected|run#13|AssertionError|assert got == ("ValidationException", FILTER_CHANGED), f"added tableName: got {got!r}"
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# KeyspacesStreams: a freshly enabled CDC stream has only one shard, and a nextToken replayed under a changed filter is refused with the wrong message

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStream

## Description
Two net-new root causes in the KeyspacesStreams emulator, both reached through its control-plane-seeded stream state and its pagination refusals.

Cause 1 of 2: a newly enabled CDC stream is created with exactly ONE shard, so GetStream can never page its shards.
  AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json) states only that GetStream.shards is a `ShardDescriptionList` where "Each shard contains a subset of the stream's data records" and that GetStream is @paginated {inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: shards} with maxResults @range 1..100 and nextToken @length 80..3000. The recordings show that a Keyspaces CDC stream is token-range partitioned at creation (the same way Kinesis/DynamoDB Streams are): immediately after the table reaches ACTIVE with cdcSpecification.status=ENABLED and GetStream reports streamStatus=ENABLED, the unpaged GetStream answer already carries SEVERAL shards (the recordings require at least 2, and at least 4 for paging by 3), every shard id of the form `shardId-<digits>-<hex>`, and the unpaged answer carries no nextToken. Which shard a given partition key lands in is not derivable from the API, which is exactly why there is more than one.
  Emulator: GetStream reports a single shard for every newly enabled stream, so maxResults=1 returns that one shard and emits NO nextToken, and there is never a second page. The replay fails at the stream's own shard count: "a new CDC stream needs several shards to page; got 1", "page 1 of 1 shards with maxResults=1 carried no nextToken", "want at least 4 shards to page by 3, got 1". The single shard is seeded where the stream is created: services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/src/state.rs, `enable_stream`, which builds one shard (`let shard = self.new_shard(created, Vec::new());`, line 523) and stores `shards: vec![shard]` (line 542).
  Why: a client that enumerates a stream's shards — which is the only supported way to consume it, since a partition's shard is not derivable — sees one shard and reads only the fraction of the change stream that shard holds, silently missing every other partition's records. A client that pages shards with maxResults never exercises pagination at all: it gets one page with no nextToken and concludes the stream is fully enumerated. Any consumer written against a multi-shard stream (one iterator per shard, fan-out across shards) degenerates to a single reader against the emulator and then loses records against real AWS.
  Fix: the stream-creation path in this service's state — the shard set `enable_stream` seeds for a new stream must be a token-range partitioning of several shards, not one, with ids of the modeled `shardId-<n>-<hex>` form and disjoint shard ids across streams; GetStream's existing paging over `stream.shards` then needs no change.
  Traces: keyspacesstreams.get-stream-forged-next-token-rejected, keyspacesstreams.get-stream-next-token-of-other-request-rejected, keyspacesstreams.get-stream-shard-pages-list-each-shard-once.

Cause 2 of 2: a nextToken replayed with CHANGED filtering parameters is refused with "Invalid nextToken." instead of the filter-changed message.
  AWS: recorded behavior only for the message text; the model gives ValidationException (HTTP 400) as the refusal shape for ListStreams and GetStream, with members `message` and `errorCode` (ValidationExceptionType). The recordings distinguish TWO refusal classes on a pagination token. A token that was never issued (a forged token of valid length) is refused ValidationException with the fixed message "Invalid nextToken." A token that WAS issued but is replayed under a different filter — ListStreams page 1 taken with keyspaceName=<ks> and then replayed with tableName=<table> added, or with keyspaceName dropped; a GetStream shard token of stream A replayed against stream B, or a ListStreams token passed to GetStream — is refused ValidationException with the fixed message "The same filtering request parameters used in the initial pagination request are required to all subsequent pagination requests except for maxResults." That whole sentence is fixed text, with no variable parts; only maxResults may differ between pages, and the same filter with a different maxResults must still complete the listing.
  Emulator: both classes collapse to the never-issued message. services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/src/ops/list_streams.rs lines 222-224 map `PageTokenError::NotIssued | PageTokenError::WrongQuery` to the same `invalid_next_token()`, i.e. `crate::service::refusals::INVALID_NEXT_TOKEN` with errorCode "InvalidFormat"; src/ops/get_stream.rs lines 248-250 do the identical collapse. Replay: adding tableName to a genuine ListStreams token got ('ValidationException', 'Invalid nextToken.') where the filter-changed message is required.
  Why: a client that changes or drops a filter mid-enumeration is told its token is malformed rather than that the filter changed, so it discards a token that is in fact still valid and restarts the listing (or reports a bug in its own token handling) instead of re-issuing the page with the original filter. The two conditions have different remedies and the message is the only thing that distinguishes them.
  Fix: the pagination-refusal mapping in each operation's handler — `PageTokenError::WrongQuery` needs its own ValidationException message (the filter-changed sentence) distinct from `NotIssued`'s "Invalid nextToken.", in both ListStreams and GetStream, with the message constants carried alongside INVALID_NEXT_TOKEN in this service's refusals module.
  Traces: keyspacesstreams.list-streams-next-token-under-changed-filter-rejected (the GetStream half of the same mapping is asserted by keyspacesstreams.get-stream-next-token-of-other-request-rejected, which cannot reach it while Cause 1 stands).

## Evidence
Cause 1 — expected: GetStream on a just-enabled CDC stream returns several shards (>=2, and >=4 for the page-by-3 walk), ids matching shardId-<digits>-<hex>, no nextToken on the unpaged answer, and maxResults=1 yields page 1 + a nextToken that returns a different, unseen shard.
Actual: one shard only.
  keyspacesstreams.get-stream-forged-next-token-rejected -> AssertionError: a new CDC stream needs several shards to page; got 1
  keyspacesstreams.get-stream-next-token-of-other-request-rejected -> AssertionError: page 1 of 1 shards with maxResults=1 carried no nextToken
  keyspacesstreams.get-stream-shard-pages-list-each-shard-once -> AssertionError: want at least 4 shards to page by 3, got 1
Source: src/state.rs enable_stream — line 523 `let shard = self.new_shard(created, Vec::new());`, line 542 `shards: vec![shard],`.

Cause 2 — expected: ListStreams(keyspaceName=<ks>, nextToken=<token issued for keyspaceName=<ks>>, tableName=<table> added) -> HTTP 400 ValidationException, message "The same filtering request parameters used in the initial pagination request are required to all subsequent pagination requests except for maxResults." (entirely fixed text); same for dropping keyspaceName, and for a GetStream token replayed against another streamArn.
Actual: HTTP 400 ValidationException, message "Invalid nextToken." (the never-issued-token message), errorCode "InvalidFormat".
  keyspacesstreams.list-streams-next-token-under-changed-filter-rejected -> AssertionError: added tableName: got ('ValidationException', 'Invalid nextToken.')
Source: src/ops/list_streams.rs:222-224 and src/ops/get_stream.rs:248-250 both match `PageTokenError::NotIssued | PageTokenError::WrongQuery` to `invalid_next_token()`.

## Steps to Reproduce
Cause 1:
1. keyspaces.CreateKeyspace(keyspaceName=<ks>).
2. keyspaces.CreateTable(keyspaceName=<ks>, tableName=<tbl>, partition key only, capacitySpecification PAY_PER_REQUEST, cdcSpecification={status: ENABLED, viewType: KEYS_ONLY}).
3. Poll keyspaces.GetTable until status=ACTIVE and cdcSpecification.status=ENABLED; take latestStreamArn.
4. Poll keyspacesstreams.GetStream(streamArn=<arn>) until streamStatus=ENABLED.
5. Observe len(shards) == 1 (AWS: several). 
6. keyspacesstreams.GetStream(streamArn=<arn>, maxResults=1) -> one shard and NO nextToken, so no second page exists.

Cause 2:
1. As above, create TWO CDC tables in one keyspace so the keyspace has two streams.
2. t = keyspacesstreams.ListStreams(keyspaceName=<ks>, maxResults=1)['nextToken'].
3. keyspacesstreams.ListStreams(keyspaceName=<ks>, tableName=<tbl_a>, nextToken=t) -> observe ValidationException "Invalid nextToken."; AWS answers ValidationException "The same filtering request parameters used in the initial pagination request are required to all subsequent pagination requests except for maxResults."
4. keyspacesstreams.ListStreams(nextToken=t) (keyspaceName dropped) -> same divergence.
5. keyspacesstreams.ListStreams(keyspaceName=<ks>, maxResults=5, nextToken=t) must still complete the listing with no nextToken on the last page.
