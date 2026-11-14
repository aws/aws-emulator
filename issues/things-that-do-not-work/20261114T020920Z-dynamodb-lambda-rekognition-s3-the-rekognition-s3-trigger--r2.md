Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle
Issue-Key: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle|_poll#7|Timeout|time.sleep(interval_s)
Issue-Commit: 3c44e49694959b3de2a0545e79b8c4fbe0cf166e

# dynamodb__lambda__rekognition__s3: the Rekognition S3-trigger lifecycle scenario still reaches no verdict — the run hangs in a poll to the harness cap with no stack verdict and no notification delivery

> _Note: this issue was identified against the code base at commit `3c44e49694959b3de2a0545e79b8c4fbe0cf166e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject (S3 ObjectCreated notification to a Lambda target; CreateStack for the deploy leg)

## Description
Cause 1 of 1: the end-to-end Rekognition S3-trigger lifecycle check makes no observable progress — every poll it issues keeps returning a non-terminal answer, so the check is killed by the harness cap (60 s) with no status, no error code and no verdict from the emulator.

AWS: the scenario's CloudFormation stack (AWS::DynamoDB::Table with ProvisionedThroughput 5/5 + AWS::IAM::Role + AWS::Lambda::Function nodejs + AWS::Lambda::Permission for s3.amazonaws.com + AWS::S3::Bucket whose NotificationConfiguration declares three suffix-filtered s3:ObjectCreated:* LambdaConfigurations) always settles into a TERMINAL StackStatus, and DescribeStacks reports it — Smithy: cloudformation StackStatus enum includes CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED; a create never remains *_IN_PROGRESS forever. On the success path a PutObject of a key matching a stored LambdaFunctionConfigurations suffix rule invokes the target function once per object write, and the handler's Rekognition DetectLabels + conditional DynamoDB PutItem write the row the next phase reads back. The arming latency is recorded behavior only (AWS documents S3 notification arming as "usually about five minutes" with no stated bound), but the delivery itself is AWS's documented S3 Event Notifications contract. Either way the caller gets a decidable answer: a terminal stack status, or a delivered event.

Emulator: the check produces only `Error: Check timed out after 60.0s`. Nothing was refused and nothing succeeded — the emulator answered every poll with a value that is neither the terminal state nor the delivered effect, so the scenario stalls inside a poll loop whose own budgets (600 s for the stack-terminal poll, 600 s for the notification-arming probe) far outlast the 60 s harness cap. The two possible stall points are both the emulator's: (a) DescribeStacks never reports a terminal StackStatus for the created stack, or (b) the stack completes and the three installed s3:ObjectCreated:* -> Lambda configurations never deliver, so the probe row never appears in the labels table. Hypothesis: (b) is the active stall — the sibling check dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row times out the same way at 60 s waiting on exactly that delivery, which shows S3 object-write notification dispatch is inert in this build.

Why: a client deploying this reference architecture gets neither a working pipeline nor a failure it can act on. CreateStack returns, the uploads return 200, and then nothing ever happens: no stack verdict and/or no invocation, so the client cannot distinguish "still arming", "filtered out" and "permanently broken". Every downstream behaviour of the scenario — the stale conditional-put row, the swallowed ConditionalCheckFailedException in the handler's log group, the diagnosis sequence, the restore and the stack destroy — is unreachable, and an automated deployer waits forever instead of erroring.

Fix: whichever of the two components is stalling — cloudformation's stack-create driver and status projection (it must advance every created stack to a terminal StackStatus and report it through DescribeStacks), and s3's object-write notification dispatcher (match the stored LambdaFunctionConfigurations suffix rules on PutObject and invoke the Lambda destination so the handler runs to completion), together with the lambda invoke path and the DynamoDB conditional-write refusal the handler catches.

Related open issues, neither of which covers this failure: 20261106T031525Z-dynamodb-lambda-rekognition-s3-the-rekognition-s3-trigger-1933c4.md names this same check id, but its replay key pins an older stall point (`_pre_invoke#7`), so the replay report lists this check as unowned; 20261114T003845Z-dynamodb-lambda-rekognition-s3-the-s3-notification-lambda.md carries only the sibling swallowed-conditional-put check's key. A fixer closing either should re-run the check id in trace_ids as well.

Traces: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle.

## Evidence
dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle — Error: Check timed out after 60.0s.

Expected per AWS: the deploy phase reads a terminal StackStatus from DescribeStacks (CREATE_COMPLETE on the success path) and GetBucketNotificationConfiguration reports the three suffix-filtered LambdaFunctionConfigurations; the pre_invoke phase then sees the probe upload's row appear in the labels table because the s3:ObjectCreated:* suffix-filtered configuration invoked the handler; the check proceeds through solve / post_invoke / cleanup / destroy to a verdict.

Actual: no emulator status, error code or message at all — the only output is `Error: Check timed out after 60.0s`. No terminal stack status and/or no notification delivery was ever observed, so the check dies inside a poll loop (stack-terminal poll, 600 s budget; notification-arming probe loop, 600 s budget), both of which outlast the harness's 60 s cap.

Corroboration that the delivery leg is inert: the sibling check dynamodb__lambda__rekognition__s3.swallowed-conditional-put-leaves-a-clean-invocation-and-a-stale-row also reports `Check timed out after 60.0s` while waiting for an S3-triggered Lambda invocation.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the check id in trace_ids.
2. CreateStack with the scenario template (DynamoDB table with ProvisionedThroughput 5/5, IAM role, nodejs function from Code.ZipFile with the Rekognition + conditional-PutItem handler, AWS::Lambda::Permission for s3.amazonaws.com scoped to the bucket ARN, and an S3 bucket whose NotificationConfiguration declares three s3:ObjectCreated:* LambdaConfigurations filtered on suffix .jpg/.jpeg/.png).
3. Poll DescribeStacks and record the StackStatus sequence: it must reach one of CREATE_COMPLETE / CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED rather than remaining *_IN_PROGRESS.
4. Call GetBucketNotificationConfiguration on the stack's bucket and assert the three suffix filter rules are installed.
5. If the stack completes, PutObject a key ending in .jpg into the bucket and poll GetItem (ConsistentRead) on the labels table for that key's row; the row must appear because the notification invoked the handler. Then re-upload different bytes under the same key and assert the row is unchanged while the invocation stays clean.
6. Observe that step 3 and/or step 5 never converges, so the scenario is killed by the harness cap with no emulator-side status or error.
