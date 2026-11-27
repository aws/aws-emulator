Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.lambda-cloudfront-createinvalidation-response-propagated-to-sqs
Issue-Key: cloudfront__events.lambda-cloudfront-createinvalidation-response-propagated-to-sqs|_drain_queue#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 41629e50f0504ce89b4649b5e84534dabeb050dd

# cloudfront__events: a CreateInvalidation response never propagates from a rule-invoked Lambda through PutEvents into SQS — the data-format check exhausts the run budget with no downstream message

> _Note: this issue was identified against the code base at commit `41629e50f0504ce89b4649b5e84534dabeb050dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation (with GetInvalidation readback) / PutEvents

## Description
Cause 1 of 1: the end-to-end chain EventBridge rule → Lambda → CloudFront CreateInvalidation → events PutEvents → second EventBridge rule → SQS never produces its downstream message, so the representation contract of a CreateInvalidation response (Id, Status, CreateTime, InvalidationBatch.CallerReference surviving the round trip untruncated, unretyped and without CreateTime drift, with the Id resolving via GetInvalidation) is never exercised. Failing check: cloudfront__events.lambda-cloudfront-createinvalidation-response-propagated-to-sqs.

AWS: Smithy (cloudfront, 2020-05-31): CreateInvalidation POST /2020-05-31/distribution/{DistributionId}/invalidation answers 201 with an Invalidation payload whose members Id (string), Status (string), CreateTime (timestamp) and InvalidationBatch (carrying the caller-supplied CallerReference) are ALL @required; errors are AccessDenied 403, BatchTooLarge 413, InconsistentQuantities 400, InvalidArgument 400, MissingBody 400, NoSuchDistribution 404, TooManyInvalidationsInProgress 400. Status is documented as reaching "Completed" when the batch is finished ("InProgress" before that). GetInvalidation GET /2020-05-31/distribution/{DistributionId}/invalidation/{Id} answers 200 with the SAME Invalidation: identical Id, the authoritative Status, the identical CreateTime and the identical InvalidationBatch.CallerReference. A Lambda whose execution role allows cloudfront:CreateInvalidation and events:PutEvents can make both calls with the in-runtime AWS SDK, and the PutEvents entry (Source "sim2real.invalidator", DetailType "invalidation-result") is matched by a rule on exactly those fields and delivered to its permitted SQS target, where envelope.detail still holds the four values verbatim.

Emulator: nothing ever arrives on the downstream queue. The check's drain allows 180 s for one message and the harness ends the check with the only diagnosis it has: "Error: Check timed out after 60.0s". No assertion about the propagated Id/Status/CreateTime/CallerReference, and none of the GetInvalidation cross-checks, is ever reached. Hypothesis (the replay output carries no emulator-side diagnosis): the first hop or the in-function SDK call fails silently — the open issue 20261107T042814Z-cloudfront-events-the-in-lambda-aws-sdk-has.md records that the in-Lambda AWS SDK has NO cloudfront client (boto3.client("cloudfront") raises UnknownServiceError), and 20261110T171004Z-cloudfront-events-3-net-new-causes-a-matched-acf6fa.md / 20261106T170430Z-cloudfront-events-eventbridge-delivery-of-a-putevents-entry.md record matched rules not invoking their targets; neither issue carries this check, and this check additionally grades the CreateInvalidation response representation and its GetInvalidation agreement, which those do not.

Why: a client that reacts to an event by invalidating a distribution and republishing the result onto the bus observes nothing at all — no invalidation record it can correlate, no forwarded event, and no way to tell a dropped delivery from a failed CloudFront call. Any downstream consumer keyed on the invalidation Id/Status/CreateTime stalls.

Fix: the chain's two emulator components this check depends on — the Lambda runtime's bundled AWS SDK/credential surface (it must expose a cloudfront client so the handler's CreateInvalidation + PutEvents run) and the EventBridge rule-target invocation path for a Lambda target and for an SQS target permitted by a queue policy — plus the CreateInvalidation/GetInvalidation handlers and their serializers, which must return and then re-serve the same required Id/Status/CreateTime/InvalidationBatch.CallerReference.

Traces: cloudfront__events.lambda-cloudfront-createinvalidation-response-propagated-to-sqs.

## Evidence
Expected (AWS): within the drain window the downstream queue holds exactly one message whose Body is an EventBridge envelope with detail = {"invalidation_id": <the 201 response's Invalidation.Id>, "status": "InProgress" or "Completed", "create_time": <Invalidation.CreateTime as ISO-8601>, "caller_reference": "dataformat-<the Lambda request id>"}; create_time is not in the future relative to receipt; and GetInvalidation(DistributionId, Id) returns Invalidation.Id == detail.invalidation_id, Invalidation.Status == detail.status, Invalidation.CreateTime == the propagated create_time and InvalidationBatch.CallerReference == detail.caller_reference.

Actual (emulator): `Error: Check timed out after 60.0s` — zero messages on the downstream queue (the check's own wait allows 180 s), so neither the propagated representation nor the GetInvalidation agreement is ever observed. The invalidation id, caller reference ("dataformat-<request id>") and timestamps are per-run values that fill the template; the requirement is the presence and agreement of the four required members, not any one run's values.

## Steps to Reproduce
1. cloudfront CreateDistributionWithTags for a minimal custom-origin distribution (Enabled false, one origin example.com, DefaultCacheBehavior with the managed CachingDisabled cache policy); note its Id.
2. sqs CreateQueue (downstream) and set a queue Policy allowing events.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = the forwarding rule ARN.
3. iam CreateRole trusting lambda.amazonaws.com with logs:*, cloudfront:CreateInvalidation and events:PutEvents; lambda CreateFunction (python3.12) whose handler calls cloudfront CreateInvalidation (Paths ["/data-format/*"], CallerReference "dataformat-"+context.aws_request_id) and then events PutEvents with Source "sim2real.invalidator", DetailType "invalidation-result" and Detail carrying invalidation id, status, create_time (ISO-8601) and caller_reference; wait State=Active.
4. events PutRule trigger {"source": ["sim2real.dataformat-trigger"], "detail-type": ["invalidation-request"]}; lambda AddPermission for events.amazonaws.com with that rule ARN; PutTargets the function.
5. events PutRule forward {"source": ["sim2real.invalidator"], "detail-type": ["invalidation-result"]}; PutTargets the queue ARN.
6. events PutEvents one entry Source "sim2real.dataformat-trigger", DetailType "invalidation-request", Detail "{}".
7. Receive from the downstream queue. Observed: no message arrives before the budget is exhausted. Expected: one message whose detail carries the invalidation's Id/Status/CreateTime/CallerReference, each agreeing with GetInvalidation on the same Id.
