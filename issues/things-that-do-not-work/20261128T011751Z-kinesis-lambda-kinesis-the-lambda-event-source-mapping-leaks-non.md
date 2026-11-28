Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit
Issue-Key: kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf|run#31|AssertionError|assert seen == _EXPECTED, (
Issue-Key: kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes|_poll#8|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Key: kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit|wait_for#14|Timeout|out = self._kin.get_records(ShardIterator=self._it, Limit=100)
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# kinesis: the Lambda event source mapping leaks non-matching records through an `exists` filter and never splits a backlog past the 6 MiB invoke limit (2 causes)

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in the Kinesis-to-Lambda event source mapping. An open issue 20261029T180026Z-dynamodb-kinesis-lambda-event-source-mappings-are-created-for-non-existent.md covers ESM creation against absent streams and carries neither replay key below.

Cause 1 of 2: a FilterCriteria `exists` pattern matches records that do not carry the leaf.
AWS: `{"data": {"a": {"b": [{"exists": true}]}}}` matches a record only when the *leaf* at that path is present; a record whose path stops at an intermediate object does not match, and the mapping does not invoke the function for it.
Emulator: delivers the intermediate-only record too — the function saw `['hit-last', 'hit-string', 'hit-zero', 'miss-intermediate']` where AWS delivers exactly `['hit-last', 'hit-string', 'hit-zero']`.
Why: a client that filters at the mapping to keep unwanted traffic out of its function is invoked for records it excluded, so its per-invocation cost and its handler's assumptions about record shape are both wrong.
Fix: the `exists` operator in the event-source-mapping filter evaluator (leaf-presence semantics, not path-prefix presence).
Traces: kinesis__lambda.kinesis-esm-exists-filter-passes-only-records-carrying-the-leaf.

Cause 2 of 2: a backlog larger than the 6 MiB invoke payload limit is never delivered, so the mapping neither splits it nor accounts for the base64 expansion.
AWS: the mapping measures the *base64-encoded* record data against the 6291456-byte invoke limit and splits a batch that would exceed it into several invocations; all records in the backlog eventually reach the function.
Emulator: nothing is delivered — the splitting check times out after ~44s with `last=[]` (no records ever reached the function) and the base64-accounting check hits the 60s check budget with no delivery at all.
Why: a producer writing records whose encoded size crosses the limit sees its stream silently stop being processed, instead of being processed across several invokes; the backlog is lost to the client with no error anywhere.
Fix: batch sizing in the Kinesis event-source-mapping poller — measure the encoded payload, split at the 6291456-byte boundary, and keep draining the shard.
Traces: kinesis__lambda.kinesis-esm-splits-a-backlog-larger-than-six-mb-into-several-invokes, kinesis__lambda.kinesis-esm-counts-the-base64-encoded-size-against-the-six-mib-invoke-limit.

## Evidence
Cause 1: `AssertionError: the function saw ['hit-last', 'hit-string', 'hit-zero', 'miss-intermediate']; expected exactly ['hit-last', 'hit-string', 'hit-zero'] (leaked: ['miss-intermediate'], missing: [])`.
Cause 2: `AssertionError: timed out after 44.307441988999926s waiting until all eight records reach the function; last=[]`; and for the base64-accounting check, `Error: Check timed out after 60.0s` with no invocation observed.

## Steps to Reproduce
Replay the 3 check ids in trace_ids. (1) Create a Kinesis ESM with `--filter-criteria 'Filters=[{Pattern="{\"data\":{\"a\":{\"b\":[{\"exists\":true}]}}}"}]'`, put a record whose JSON stops at `a` (no `b`), and observe the function being invoked for it. (2) Put eight records whose combined base64-encoded size exceeds 6291456 bytes onto the stream behind an enabled ESM and watch for any invocation — none arrives.
