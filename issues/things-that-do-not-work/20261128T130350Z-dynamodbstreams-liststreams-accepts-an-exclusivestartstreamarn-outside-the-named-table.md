Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-list-streams-start-arn-outside-named-table-rejected
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-list-streams-start-arn-outside-named-table-rejected
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|ListStreams|4xx->2xx|$.__type
Issue-Key: dynamodbstreams|ListStreams|4xx->2xx|$.message
Issue-Key: dynamodbstreams|ListStreams|4xx->2xx|status
Issue-Commit: c3e5fb924e64874a3c7124e31a5bf7fe6194984f

# ListStreams accepts an ExclusiveStartStreamArn outside the named table instead of refusing with ValidationException

> _Note: this issue was identified against the code base at commit `c3e5fb924e64874a3c7124e31a5bf7fe6194984f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreams

## Description
One cause.

Cause 1 of 1: ListStreams does not validate ExclusiveStartStreamArn against the TableName it is scoped to, so a start ARN that does not name a stream of that table is silently accepted and paginated from instead of being refused.

AWS: recorded behavior only for the error itself — the Smithy model declares ListStreams with only InternalServerError and ResourceNotFoundException (Smithy: ListStreams input members TableName -> TableName @length min 3 max 255 @pattern ^[a-zA-Z0-9_.-]+$, Limit -> PositiveIntegerObject @range min 1, ExclusiveStartStreamArn -> StreamArn @length min 37 max 1024; errors InternalServerError (500, server), ResourceNotFoundException (400, client)), so the coral input-validation fault is not modeled. The recording shows AWS rejecting the request with HTTP 400 and the JSON body __type = "com.amazon.coral.validate#ValidationException" and message = "One or more parameter values were invalid: ExclusiveStartStreamArn provided is invalid: <ExclusiveStartStreamArn>". The fixed text of that message is "One or more parameter values were invalid: ExclusiveStartStreamArn provided is invalid: "; the only variable part is the ExclusiveStartStreamArn value echoed verbatim from the request (the recording's particular table name, account id and stream label are one run's values, not part of the requirement). The rule is that an ExclusiveStartStreamArn whose stream does not belong to the request's TableName is not a valid continuation token for that listing and is rejected before any page is produced.

Emulator: answers HTTP 200 with a normal ListStreams page. No __type and no message member are emitted at all (replay detail: "status: expected 400, got 200; $.__type: missing; $.message: missing"). The emulator evidently treats ExclusiveStartStreamArn as an opaque cursor and only length/格式-checks it, if that, without joining it back to TableName.

Why: a client relying on AWS's behavior uses the ValidationException as the signal that its pagination state is corrupt or belongs to a different listing (for example a cursor carried over from a ListStreams call for another table, or a stream whose table was replaced). Against the emulator that mistake returns 200 and a page of results, so the client silently paginates a listing it did not ask for, or loops on a cursor that will never advance, and never surfaces the error its retry/validation path is written around. Code that asserts a cross-table cursor is rejected passes on AWS and fails only in production-shaped testing.

Fix: ListStreams' input validation in the dynamodbstreams operation handler — resolve ExclusiveStartStreamArn to a stream and confirm it belongs to the requested TableName before building the page, and on failure return HTTP 400 with __type "com.amazon.coral.validate#ValidationException" and the message above, with the request's ExclusiveStartStreamArn interpolated.

No existing open dynamodbstreams issue covers this: 20261029T172902Z-dynamodbstreams-describestream-projection-omits-keyschema-type-and-never-emits.md is about DescribeStream's projection and 20261127T204218Z-dynamodbstreams-getsharditerator-refuses-a-sequencenumber-below-an-untrimmed-shard.md is about GetShardIterator sequence-number positioning.

Traces: dynamodbstreams-list-streams-start-arn-outside-named-table-rejected#task#8 and #task#9.

## Evidence
Fixture dynamodbstreams-list-streams-start-arn-outside-named-table-rejected, step task#8 (cluster also covers task#9), operation ListStreams.

Expected (recorded AWS): HTTP 400, body $.__type = "com.amazon.coral.validate#ValidationException", $.message = "One or more parameter values were invalid: ExclusiveStartStreamArn provided is invalid: <the ExclusiveStartStreamArn sent in the request>".

Actual (emulator): HTTP 200; $.__type missing (got None); $.message missing (got None) — a successful ListStreams page was returned instead of the validation fault.

Divergence key: service=dynamodbstreams|ListStreams|status=400->200|keys=$.__type,$.message,status

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture dynamodbstreams-list-streams-start-arn-outside-named-table-rejected, which creates a table with a stream, then calls ListStreams with TableName set to that table and ExclusiveStartStreamArn set to a stream ARN that does not belong to it.
3. Step task#8 (and task#9): observe HTTP 200 with a Streams page, instead of HTTP 400 with __type com.amazon.coral.validate#ValidationException and message "One or more parameter values were invalid: ExclusiveStartStreamArn provided is invalid: <arn>".
