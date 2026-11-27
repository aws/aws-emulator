Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.s3-notification-to-lambda-without-permission-rejected dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery dynamodb__lambda__s3.s3-event-record-key-decoding-format dynamodb__lambda__s3.s3-notification-prefix-suffix-filter-scoping dynamodb__lambda__s3.pipeline-s3-lambda-dynamodb-lambda-s3 dynamodb__lambda__s3.shared-state-concurrent-s3-uploads-write-dynamodb dynamodb__lambda__s3.ddb-stream-modify-event-captures-images dynamodb__lambda__s3.ddb-stream-remove-event-triggers-lambda dynamodb__lambda__s3.arn-resolution-ddb-stream-esm-matches-table-stream-arn dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack
Issue-Key: dynamodb__lambda__s3.s3-notification-to-lambda-without-permission-rejected|_refused#12|AssertionError|raise AssertionError(f"{what}: expected {'/'.join(codes)} HTTP {status}, the call succeeded")
Issue-Key: dynamodb__lambda__s3.s3-notification-without-invoke-permission-blocks-delivery|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {resp!r}")
Issue-Key: dynamodb__lambda__s3.s3-event-record-key-decoding-format|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.s3-notification-prefix-suffix-filter-scoping|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.pipeline-s3-lambda-dynamodb-lambda-s3|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.shared-state-concurrent-s3-uploads-write-dynamodb|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.ddb-stream-modify-event-captures-images|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.ddb-stream-remove-event-triggers-lambda|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda__s3.arn-resolution-ddb-stream-esm-matches-table-stream-arn|_error#11|AssertionError|raise AssertionError(f"expected a refusal but the call succeeded: {result!r}")
Issue-Key: dynamodb__lambda__s3.ddb-stream-esm-creation-fails-without-permissions|_error#11|AssertionError|raise AssertionError(f"expected a refusal but the call succeeded: {result!r}")
Issue-Key: dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# S3 notifications never invoke Lambda, are accepted without invoke permission, ESMs accept bogus stream ARNs, and the notification stack rolls back (4 causes, 11 checks)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Four net-new root causes in the dynamodb__lambda__s3 group. Each needs its own fix.

Cause 1 of 4: PutBucketNotificationConfiguration is accepted for a Lambda that has no S3 invoke permission.
AWS: S3 validates the destination at configuration time — without a `lambda:InvokeFunction` resource policy statement allowing `s3.amazonaws.com` for that bucket, the call is refused with `InvalidArgument` and HTTP 400, naming the unable-to-invoke destination.
Emulator: the call SUCCEEDS (HTTP 200, empty body).
Why: the misconfiguration a caller is supposed to be told about at configure time is accepted, so the notification silently never fires and the negative path is untested.
Fix: the s3 notification-configuration validator (check the destination's resource policy).
Traces: dynamodb__lambda__s3.s3-notification-to-lambda-without-permission-rejected, .s3-notification-without-invoke-permission-blocks-delivery

Cause 2 of 4: S3 object events never invoke the configured Lambda.
AWS: a PutObject that matches the notification's prefix/suffix filter invokes the destination function with the S3 event record (the object key URL-decoded as AWS encodes it), and non-matching keys do not.
Emulator: no invocation; each check polls for the effect and ends with `Check timed out after 60.0s`, so neither the delivery nor the filter scoping can be observed.
Why: the most common serverless trigger is inert, and prefix/suffix scoping (which is what keeps a function from seeing objects it must not) cannot be verified.
Fix: the s3 notification dispatcher (event construction, key encoding, prefix/suffix filtering) and the lambda invoke it performs. (The open issues 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md and 20261025T192630Z-dynamodb-lambda-s3-a-dynamodb-streams-event-source.md carry other checks in this group but not these replay keys.)
Traces: dynamodb__lambda__s3.s3-event-record-key-decoding-format, .s3-notification-prefix-suffix-filter-scoping, .pipeline-s3-lambda-dynamodb-lambda-s3, .shared-state-concurrent-s3-uploads-write-dynamodb, .ddb-stream-modify-event-captures-images, .ddb-stream-remove-event-triggers-lambda

Cause 3 of 4: CreateEventSourceMapping accepts a stream ARN that resolves to nothing.
AWS: an `EventSourceArn` that names a stream label the table does not have, or a function role without the stream-read permissions, is refused with `InvalidParameterValueException` (HTTP 400).
Emulator: the call is ACCEPTED (HTTP 202) and returns a mapping in `State: Creating` carrying the bogus ARN (e.g. `.../table/<table>/stream/label/nonexistent-stream-label`).
Why: a mapping is created that can never deliver, with no error for the caller to act on.
Fix: CreateEventSourceMapping validation (resolve the stream ARN against table state, authorize the role).
Traces: dynamodb__lambda__s3.arn-resolution-ddb-stream-esm-matches-table-stream-arn, .ddb-stream-esm-creation-fails-without-permissions

Cause 4 of 4: the S3-notification + Lambda + DynamoDB stack rolls back.
AWS: the stack reaches `CREATE_COMPLETE` with the bucket, notification configuration, function and table created.
Emulator: the stack ends in `ROLLBACK_COMPLETE`, so none of the resources exist.
Why: a valid template cannot be deployed; the caller gets a failed stack.
Fix: the cloudformation provisioner for the resource that fails (read the stack events) — AWS::S3::Bucket NotificationConfiguration / AWS::Lambda::Permission.
Traces: dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack

## Evidence
- Cause 1: AssertionError: PutBucketNotificationConfiguration to a Lambda without an S3 invoke permission: expected InvalidArgument HTTP 400, the call succeeded; and "expected a ClientError, but the call succeeded: {'ResponseMetadata': {..., 'HTTPStatusCode': 200, ...}}".
- Cause 2 (six checks): Error: Check timed out after 60.0s.
- Cause 3: AssertionError: expected a refusal but the call succeeded: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'EventSourceArn': 'arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/label/nonexistent-stream-label', 'State': 'Creating', ...}.
- Cause 4: AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE.

## Steps to Reproduce
Boot the emulator and replay the 11 checks listed in trace_ids.
