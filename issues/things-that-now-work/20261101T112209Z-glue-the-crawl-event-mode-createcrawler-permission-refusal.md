Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative
Issue-Key: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative|run#11|AssertionError|assert "SQS:ReceiveMessage" in reasons, f"expected the rejection to name the missing action: {reasons}"
Issue-Commit: cfa0ed4d05301b32ad97f282bfccfa128d53f379

# Glue: the CRAWL_EVENT_MODE CreateCrawler permission refusal does not name the missing SQS action, so the CloudFormation Crawler(CREATE_FAILED) reason is emulator-authored

> _Note: this issue was identified against the code base at commit `cfa0ed4d05301b32ad97f282bfccfa128d53f379`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCrawler

## Description
1 cause; it must be closed before submitting.

Related open issues, named for the fixer, neither of which carries the replay key listed here: 20261029T185330Z-glue-9-net-new-root-causes-in-control.md (its Cause 3) filed the ABSENCE of this create-time check — its replay key is the earlier assertion `assert status in {"ROLLBACK_COMPLETE", "ROLLBACK_FAILED"}`, which now PASSES; the check now fails further on, at the message assertion, which is the net-new cause described here. 20261029T193114Z-glue-sqs-5-net-new-causes-job-runs.md is a different glue__sqs bundle and does not cover this. The fixed issue 20261022T192452Z-glue-createcrawler-skips-role-validation-and-deleteschemaversions-raises.md covers CreateCrawler's ROLE TRUST-POLICY validation ("Service is unable to assume provided role..."), a different refusal condition, and is not reopened by this.

Cause 1 of 1: the event-mode SQS-permission refusal of CreateCrawler carries the wrong error message — it reports the queue as absent/inaccessible instead of naming the IAM action the crawler role is missing.

AWS: Smithy com.amazonaws.glue#CreateCrawler takes required Name/Role/Targets and models exactly AlreadyExistsException, InvalidInputException, OperationTimeoutException and ResourceNumberLimitExceededException (all client, httpError 400 defaulted). Targets.S3Targets[].EventQueueArn together with RecrawlPolicy.RecrawlBehavior = CRAWL_EVENT_MODE makes Glue validate AT CREATE TIME that the crawler's Role can consume the named queue. Recorded behavior only (the model does not state the message text): when the role carries no SQS consume permission, Glue's refusal MESSAGE names the missing action — the recorded reason contains the literal action token `SQS:ReceiveMessage` — and through CloudFormation the AWS::Glue::Crawler resource is CREATE_FAILED with `HandlerErrorCode: InvalidRequest`, the stack rolls back to ROLLBACK_COMPLETE, and GetCrawler afterwards answers EntityNotFoundException `Crawler entry with name <crawler-name> does not exist`. In that reason the fixed text is the missing-action naming (the literal `SQS:ReceiveMessage`) and the `HandlerErrorCode: InvalidRequest` classification; the variable parts are the logical resource id (`Crawler`), the queue ARN and role name taken from the request, and the per-request Glue RequestToken/Request ID (which the check deliberately does not assert).

Emulator: the refusal, the CREATE_FAILED status, the rollback and the InvalidRequest handler-error code are all now produced, but the message is emulator-authored: `Resource handler returned message: "SQS queue arn:aws:sqs:<region>:<account>:<queue-name> does not exist or the role provided does not have access to it." (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. It conflates two distinct conditions (queue absent OR role unauthorized) and never names the action that is actually missing, so the assertion that the rejection names `SQS:ReceiveMessage` fails.

Why: the whole point of this create-time validation is that the deployer is told WHICH grant to add. A client (or a CloudFormation operator reading the stack event's ResourceStatusReason) is told the queue may not exist, so the first remedial action is to go looking for a missing queue that is in fact present and correct, rather than adding sqs:ReceiveMessage/sqs:DeleteMessage to the crawler role. Automation that greps the stack-event reason for the missing action — the documented way to distinguish this failure from a genuinely absent queue — never matches, and the two causes become indistinguishable.

Fix: the CreateCrawler handler's event-mode permission validation and the message it builds (services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs, and whatever shared refusal-message constructor it calls) must emit two distinct messages: the absent-queue message only when the queue does not resolve, and for the authorization failure a message naming the missing action(s) — `SQS:ReceiveMessage` for the receive grant — with the queue ARN and role interpolated from the request. The CloudFormation resource-provider layer must surface that message verbatim as the Crawler resource's ResourceStatusReason; its status, HandlerErrorCode and rollback behavior are already correct and must not change.

## Evidence
glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative (verdict: mismatch).

Passing already: the stack reaches a rollback status and `Crawler(CREATE_FAILED)` is the failing resource.

Failing assertion (check line 45): `assert "SQS:ReceiveMessage" in reasons, f"expected the rejection to name the missing action: {reasons}"`

Actual stack-event reasons from the emulator:
`sim2real-xsvc-stk-<id>(CREATE_FAILED): The following resource(s) failed to create: [Crawler]. ; Crawler(CREATE_FAILED): Resource handler returned message: "SQS queue arn:aws:sqs:us-west-2:000000000000:sim2real-xsvc-evq-<id> does not exist or the role provided does not have access to it." (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`

Expected: a reason that additionally contains the literal `SQS:ReceiveMessage` (alongside the already-correct `Crawler(CREATE_FAILED)` and `HandlerErrorCode: InvalidRequest`). The stack id, queue ARN, RequestToken and crawler name above are per-run values that fill the template; they are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-aws-v1, check `glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative`.
2. Or by hand: CreateStack with AWS::SQS::Queue (QueueName=Q), AWS::S3::Bucket (B), AWS::IAM::Role (R) whose inline policy grants only s3:GetObject/s3:ListBucket on B plus sqs:SetQueueAttributes/sqs:GetQueueUrl/sqs:PurgeQueue on Q (deliberately NOT sqs:ReceiveMessage or sqs:DeleteMessage) and whose trust policy allows glue.amazonaws.com, AWS::Glue::Database (D), and AWS::Glue::Crawler with Role={Fn::GetAtt R.Arn}, DatabaseName=D, Targets.S3Targets=[{Path: s3://B/data/, EventQueueArn: {Fn::GetAtt Q.Arn}}] and RecrawlPolicy.RecrawlBehavior=CRAWL_EVENT_MODE.
3. Poll DescribeStacks until terminal, then DescribeStackEvents and collect the *_FAILED events' ResourceStatusReason.
4. Observe: the Crawler(CREATE_FAILED) reason says the SQS queue "does not exist or the role provided does not have access to it" and never names `SQS:ReceiveMessage`.
5. Expected: that reason names the missing action `SQS:ReceiveMessage`, still with HandlerErrorCode: InvalidRequest and the stack still reaching ROLLBACK_COMPLETE, and GetCrawler then answering EntityNotFoundException `Crawler entry with name <crawler-name> does not exist`.
6. Control: grant sqs:ReceiveMessage/sqs:DeleteMessage on the same role/queue pair and re-issue the identical CreateCrawler — it must succeed.
