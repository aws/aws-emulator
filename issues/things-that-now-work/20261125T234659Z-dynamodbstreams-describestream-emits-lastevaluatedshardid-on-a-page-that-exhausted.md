Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.describe-stream-shard-pagination-covers-all-shards
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams.describe-stream-shard-pagination-covers-all-shards|run#11|AssertionError|assert "LastEvaluatedShardId" not in desc, (
Issue-Commit: c11f37d9666aeec66c301c48be9695cec998b743

# DescribeStream emits LastEvaluatedShardId on a page that exhausted the shard list, so shard paging never terminates

> _Note: this issue was identified against the code base at commit `c11f37d9666aeec66c301c48be9695cec998b743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStream

## Description
Cause 1 of 1: DescribeStream sets StreamDescription.LastEvaluatedShardId to the last shard of the page it just served WHENEVER the page carried any shard, instead of only when a further page of shards remains. The cursor is therefore offered on a terminal page — including an unlimited DescribeStream that returned the stream's entire shard set.

AWS: Smithy: DescribeStream input {StreamArn @required @length 37..1024, Limit (PositiveIntegerObject @range min 1), ExclusiveStartShardId (ShardId @length 28..65), ShardFilter}; output StreamDescription. StreamDescription.LastEvaluatedShardId targets ShardId and is documented as "The shard ID of the item where the operation stopped, inclusive of the previous result set. Use this value to start a new operation, excluding this value in the new request" and "If LastEvaluatedShardId is empty, then the 'last page' of results has been processed and there is currently no more data to be retrieved." Emptiness is thus AWS's ONLY terminator for a shard walk, and AWS stops offering the cursor once a response has stopped at the end of the shard list: a DescribeStream issued with no Limit that returns every shard of the stream comes back with the member absent, and the final page of a Limit-bounded walk comes back with the member absent as well. Status is 200 in both the diverging and the correct case; this is a response-shape defect, not an error-path one.

Emulator: the handler computes the cursor as `page.last()` — the last shard ON the page, with no look-ahead for a further shard — so the member is populated on every page that evaluated at least one shard. The replayed check created a table with StreamEnabled/NEW_AND_OLD_IMAGES, waited for the stream to be ENABLED with shards, then called DescribeStream(StreamArn) with no Limit and no ExclusiveStartShardId; the response listed the stream's whole shard set AND carried LastEvaluatedShardId equal to that single shard id (a generated `shardId-<digits>-<hex>` value; the generated id itself is not the requirement, its PRESENCE is). The handler's own doc comment states this is intentional ("reports where this page stopped WITHOUT looking past it for a further shard"), so the rule, not just one call, is wrong. Note that the open issue 20261029T172902Z-dynamodbstreams-describestream-projection-omits-keyschema-type-and-never-emits.md reports the OPPOSITE defect (the member never emitted at all) against an earlier commit; it does not cover this one — the behavior has since flipped from never-emitted to always-emitted, and the correct behavior is neither.

Why: a client that pages shards the documented way — call DescribeStream, pass LastEvaluatedShardId back as ExclusiveStartShardId, stop when the member is empty — never sees the terminator on the page that actually ended the shard list. It issues at least one extra DescribeStream per walk (against a service documented at 10 calls/second, so a tight consumer loop burns its own throttle budget), and a client that treats a present cursor as "more shards exist" (the ordinary reading, and the reading every boto/SDK shard-walk helper uses) loops or reports a shard page that is not there. The same miscount corrupts the Limit-bounded walk: after the last real shard the walk is driven one page further, which returns zero shards, so a consumer that expects Limit shards per non-terminal page sees an empty page it never asked for.

Fix: DescribeStream's handler shard-paging logic in services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/ops/describe_stream.rs — the cursor must be derived from whether shards REMAIN after the page (i.e. only when start_at + page.len() < shards.len()), not from the page's last element; the StreamDescription serializer must then omit the member entirely rather than send an empty string.

Traces: behavioral check dynamodbstreams.describe-stream-shard-pagination-covers-all-shards (the assertion that fails is the unlimited-call one; the Limit=1 walk in the same check exercises the terminal-page half of the same rule). Code citation: describe_stream.rs lines 294-308 (page slice and `let last_evaluated_shard_id = page.last().map(...)`) and lines 23-26 / 296-307 (the doc comment asserting the wrong rule).

## Evidence
Check dynamodbstreams.describe-stream-shard-pagination-covers-all-shards, verdict mismatch:

  File "check_dynamodbstreams_describe_stream_shard_pagination_covers_all_shards.py", line 46, in run
    assert "LastEvaluatedShardId" not in desc, (
  AssertionError: an unlimited DescribeStream returned every shard, so it must not offer a page token; got 'shardId-00000001795650251892-0aed7254'

Expected (AWS): DescribeStream(StreamArn=<arn>) with no Limit returns StreamDescription with Shards = the stream's full shard set and NO LastEvaluatedShardId member (HTTP 200).
Actual (emulator): HTTP 200, StreamDescription contains the full shard set AND LastEvaluatedShardId = the id of the last shard in that set.

The shard id in the message is a per-run generated value and is not itself a requirement; the requirement is that the member be absent on a page that stopped at the end of the shard list.

Source of the rule, services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/ops/describe_stream.rs:308
    let last_evaluated_shard_id = page.last().map(|shard| shard.shard_id.clone());

Smithy (com.amazonaws.dynamodbstreams#DescribeStream): StreamDescription.LastEvaluatedShardId -> ShardId (@length 28..65), doc: "If LastEvaluatedShardId is empty, then the 'last page' of results has been processed and there is currently no more data to be retrieved."

## Steps to Reproduce
1. Boot the emulator and replay behavioral check dynamodbstreams.describe-stream-shard-pagination-covers-all-shards.
2. CreateTable with KeySchema [{pk,HASH}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}; take LatestStreamArn from the TableDescription.
3. Poll dynamodbstreams DescribeStream(StreamArn=<arn>) until StreamStatus is ENABLED and Shards is non-empty and every shard yields a TRIM_HORIZON iterator.
4. Call DescribeStream(StreamArn=<arn>) with no Limit and no ExclusiveStartShardId. The response lists every shard of the stream, so it is the last page — but StreamDescription.LastEvaluatedShardId is present (set to the last listed shard id). AWS omits the member here.
5. Walk the same stream with Limit=1, feeding each LastEvaluatedShardId back as ExclusiveStartShardId: the page that returns the final shard still carries a cursor, so the walk is driven one page past the end and that extra page returns zero shards instead of terminating.
