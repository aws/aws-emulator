Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row
Issue-Key: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row|_poll#28|Timeout|time.sleep(interval_s)
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# dynamodb__lambda__rekognition__s3: the S3-notification → Lambda → Rekognition/DynamoDB pipeline never completes, so a swallowed conditional-put cannot be observed

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject (S3 ObjectCreated notification to a Lambda target)

## Description
Cause 1 of 1: the whole S3-notification-driven pipeline never produces its observable effects within the run budget, so the behaviour under test — a handler that swallows a DynamoDB conditional-check failure and returns, leaving a clean invocation record and a stale row — is never reached.
AWS: a bucket with an ObjectCreated notification targeting a Lambda function invokes that function once per uploaded object; the handler runs, calls Rekognition and DynamoDB, and a ConditionalCheckFailedException it catches and ignores leaves the invocation recorded as successful (no function error) while the pre-existing item stays unchanged. The combination is recorded behavior only — the S3/lambda Smithy models describe PutBucketNotificationConfiguration and Invoke, not the end-to-end delivery — but the delivery itself is AWS's documented S3 Event Notifications contract.
Emulator: the check exhausts its whole 60 s budget ('Check timed out after 60.0s') with no completed invocation to inspect, so neither the clean invocation record nor the stale row can be read back.
Why: a client that builds an ingest pipeline on S3 notifications gets no invocation at all, so none of the behaviour that distinguishes a silently-swallowed write failure from a successful write is observable; code that is correct against AWS and code that is broken look identical.
Fix: the S3 bucket-notification dispatcher and its Lambda target delivery path (and whatever it depends on to run the handler to completion), plus the DynamoDB conditional-write refusal the handler catches.
Traces: dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row.

The open issue 20261106T031525Z-dynamodb-lambda-rekognition-s3-the-rekognition-s3-trigger-1933c4.md records the sibling check in this family; it does not carry this check's replay key.

## Evidence
dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row — Error: Check timed out after 60.0s.
Expected per AWS: the upload triggers exactly one invocation of the notification target; the invocation completes without a function error, and the DynamoDB item that failed the conditional put is unchanged.

## Steps to Reproduce
1. Create a bucket, a DynamoDB table with a pre-existing item, and a Node.js Lambda whose handler calls Rekognition and then a conditional DynamoDB PutItem, catching and only logging the conditional-check failure before returning.
2. Wire an S3 ObjectCreated notification on the bucket to the function and grant s3.amazonaws.com invoke permission.
3. PutObject into the bucket and wait for the invocation: AWS runs the handler and records a clean invocation while leaving the item stale; the emulator produces no observable invocation before the check's budget expires.
