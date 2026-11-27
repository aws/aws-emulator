Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.notification-plus-in-prefix-filter-matches-a-space-in-the-key s3__sqs.object-restore-completed-notification-follows-the-restore-post s3__sqs.s3-notification-to-aws-managed-key-queue-rejected
Issue-Key: s3__sqs.notification-plus-in-prefix-filter-matches-a-space-in-the-key|run#22|AssertionError|assert time.monotonic() < deadline, (
Issue-Key: s3__sqs.object-restore-completed-notification-follows-the-restore-post|run#28|AssertionError|assert completed, (
Issue-Key: s3__sqs.s3-notification-to-aws-managed-key-queue-rejected|expect_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected with {(code, message)!r}, but it SUCCEEDED: {result!r}")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 3 causes in S3 event notifications: prefix filter matched against the raw key, no ObjectRestore:Completed, and an AWS-managed-key queue accepted

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Three causes.

Cause 1 of 3: a notification key prefix filter is matched against the raw object key instead of the `+`-encoded key form S3 uses.
AWS: recorded behavior only (key encoding is not in the Smithy model). In an S3 event record the object key is URL-encoded with spaces written as `+`, and the notification configuration's Prefix/Suffix FilterRules are matched against that same encoded form. So a stored prefix rule containing `+` matches a key containing a space, and a key containing a literal `+` (encoded `%2B`) does not match it.
Emulator: the opposite happens — the object whose key contains a literal `+` produces an event and the object whose key contains a space produces none.
Why: a client filtering on a prefix that contains `+` (or storing keys with spaces) receives events for exactly the wrong objects; a consumer keyed on the prefix processes unrelated objects and misses the ones it was configured for.
Fix: the S3 notification filter evaluator (match Prefix/Suffix against the `+`-encoded key, the same form written into the event record).
Traces: s3__sqs.notification-plus-in-prefix-filter-matches-a-space-in-the-key.

Cause 2 of 3: `s3:ObjectRestore:Completed` is never emitted after a restore finishes.
AWS: recorded behavior only. A RestoreObject request emits `s3:ObjectRestore:Post` immediately and, once the temporary copy is available — observable because HeadObject reports `Restore: ongoing-request="false", expiry-date="..."` — emits `s3:ObjectRestore:Completed` to the configured destination.
Emulator: only `ObjectRestore:Post` arrives; no `ObjectRestore:Completed` is emitted even after HeadObject already reports the restore finished with an expiry date.
Why: a client's restore pipeline waits on the Completed event to pick up the restored object; it never fires, so the pipeline stalls forever while S3's own HeadObject says the restore is done.
Fix: the S3 RestoreObject completion path's notification emission.
Traces: s3__sqs.object-restore-completed-notification-follows-the-restore-post.

Cause 3 of 3: PutBucketNotificationConfiguration accepts a destination queue encrypted with the AWS-managed SQS key.
AWS: S3 cannot publish to a queue encrypted with the AWS-managed `alias/aws/sqs` key (it cannot be granted to the S3 service principal), so S3's destination validation refuses the configuration with HTTP 400 `InvalidArgument` and the message `Unable to validate the following destination configurations`, and no configuration is stored.
Emulator: the call succeeds with HTTP 200 and the configuration is stored.
Why: the client is told its notification destination is valid; S3 would have refused it, so the client ships a bucket whose events can never be delivered and finds out only when the queue stays empty.
Fix: the S3 PutBucketNotificationConfiguration destination validation (it must attempt the test publish / key-policy check that AWS performs).
Traces: s3__sqs.s3-notification-to-aws-managed-key-queue-rejected.

## Evidence
Cause 1: `AssertionError: no event for 'year end/ledger.csv' arrived; keys seen were ['year+end/ledger.csv']` — the key with the literal `+` matched the filter, the key with the space did not. The keys are per-run names; the rule is that the filter is applied to the `+`-encoded key.
Cause 2: `AssertionError: no ObjectRestore:Completed arrived within 10s (3 x the ObjectRestore:Post latency ..., floor 10.0s) of HeadObject reporting Restore 'ongoing-request="false", expiry-date="..."'; events seen ['ObjectRestore:Post']`.
Cause 3: expected `('InvalidArgument', 'Unable to validate the following destination configurations')` with HTTP 400 and nothing stored; actual `it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The fixed text is `Unable to validate the following destination configurations`; the offending destination ARN is the variable part AWS appends.

## Steps to Reproduce
Cause 1: configure a queue notification with a Prefix FilterRule containing `+`; put one object whose key contains a space at that position and one whose key contains a literal `+`; only the literal-`+` object produces an event.
Cause 2: configure `s3:ObjectRestore:*` to a queue, PutObject into an archive storage class, RestoreObject, then poll until HeadObject reports `ongoing-request="false"` — only ObjectRestore:Post ever arrives.
Cause 3: create a queue with SSE using the AWS-managed `alias/aws/sqs` key and call PutBucketNotificationConfiguration naming it — the call is accepted.
Replay: the three check ids in trace_ids.
