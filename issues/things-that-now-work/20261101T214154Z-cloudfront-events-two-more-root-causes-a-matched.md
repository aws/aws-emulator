Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.cfn-deploy-eventbridge-rule-missing-permission-not-invoked cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge
Issue-Key: cloudfront__events.cfn-deploy-eventbridge-rule-missing-permission-not-invoked|_poll_until#12|Timeout|last = probe()
Issue-Key: cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation|_find_matching_invalidation#18|Timeout|return None
Issue-Key: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge|_await_sinks_live#43|Timeout|got_envs = _drain(sqs, u)
Issue-Commit: 446e5d40b4aa72232e72e50ee85b856a1859ebd6

# cloudfront__events: two more root causes — a matched rule's permitted Lambda target relays nothing, and the S3→EventBridge→Lambda→CreateInvalidation chain never produces an invalidation or refreshes the edge

> _Note: this issue was identified against the code base at commit `446e5d40b4aa72232e72e50ee85b856a1859ebd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents / CreateInvalidation (EventBridge rule → Lambda target)

## Description
This issue bundles 2 net-new root causes in the cloudfront__events behavioral service. Each cause below is a separate defect that must be closed before this issue is submitted. Neither of these causes' checks was marked ALREADY FILED or OWNED by the replay report. Adjacent open issues that do NOT carry these checks' replay keys, named only for context: 20261023T140640Z-cloudfront-events-3-of-the-4-causes-remain-open.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md, 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md, 20261025T002357Z-cloudfront-events-cloudfront-read-only-calls-emit-no-events-and.md.

=== Cause 1 of 2: a CFN-deployed rule with two Lambda targets invokes neither — the PERMITTED target never relays the event ===
AWS: PutEvents accepts an entry (Source, DetailType, Detail) and the default bus matches it against every ENABLED rule. For a rule with two Lambda targets where exactly one function carries an AWS::Lambda::Permission for Principal events.amazonaws.com, AWS invokes that one function asynchronously and does not invoke the sibling. The permitted function's own SendMessage then places a message on the relay queue whose body carries the event's detail (here: the nonce written into detail.requestParameters). The observable contract is: the permitted target's message arrives, the unpermitted target's never does, and once drained ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible are both "0". Fixed parts: the two queue attribute names and the principal string events.amazonaws.com; variable parts: function names, queue URL, the event's Source/DetailType/Detail values.
Emulator: the permitted target never relays anything. The check polls the relay queue for up to 120 s for a message tagged with the permitted target's label and the request's nonce, receives none, and is cut off at the suite's 60 s per-check cap. Because the positive half of the twin control never fires, the calibrated absence window for the blocked target is never even entered, so the resource-policy boundary the check exists to prove is unobservable rather than merely wrong.
Why: a client that deploys the canonical "rule fans out to two Lambdas, only one is permitted" stack sees no invocation at all. A correctly permitted target and a deliberately unpermitted one produce the identical (empty) outcome, so the emulator silently accepts stacks whose permissions are wrong and gives no signal for stacks whose permissions are right.
Fix: the EventBridge rule-target delivery path for Lambda targets — matching a PutEvents entry to an ENABLED rule, evaluating the target function's resource-based policy for events.amazonaws.com, and performing the asynchronous invoke — together with the CloudFormation provisioning of AWS::Events::Rule targets and AWS::Lambda::Permission that the stack declares.
Traces: cloudfront__events.cfn-deploy-eventbridge-rule-missing-permission-not-invoked

=== Cause 2 of 2: S3 Object Created → EventBridge rule → Lambda → CreateInvalidation produces no invalidation, and the edge never reflects one ===
AWS: a bucket with NotificationConfiguration.EventBridgeConfiguration emits "Object Created" (source aws.s3, detail.bucket.name = the bucket, detail.object.key = the key). A matching ENABLED rule with a permitted Lambda target invokes the function, whose CreateInvalidation (Smithy: CreateInvalidation, POST /2020-05-31/distribution/{DistributionId}/invalidation, HTTP 201, output Invalidation; errors AccessDenied 403, BatchTooLarge 413, NoSuchDistribution 404) creates an invalidation visible through ListInvalidations and GetInvalidation: the same Id, InvalidationBatch.Paths.Items equal to the submitted paths, and Status in {InProgress, Completed}. On the data plane, the distribution's edge serves the previously cached bytes until that invalidation completes and the newly written bytes afterwards. Fixed parts: the "Object Created" detail-type, the aws.s3 source, and the Status enum values; variable parts: bucket, key, distribution id, invalidation id and caller reference.
Emulator: no invalidation ever appears. Both checks poll ListInvalidations (240 s budget in one, a staged multi-leg budget in the other, with repeated PutObject retries to allow for S3's eventually-consistent EventBridgeConfiguration) and never see an entry for the written key, so GetInvalidation's Id/Status assertions are never reached; each run is cut off at the suite's 60 s per-check cap. The edge-refresh half of the second check (stale bytes before, new bytes after the invalidation completes) is likewise never reached, consistent with the edge data plane serving nothing.
Why: the most common CloudFront automation in practice — invalidate the CDN when an origin object is overwritten — is completely inert. A client sees no invalidation record to poll, no Status transition, and an edge that never reflects the new object, so cache-coherency logic written against AWS cannot be exercised at all.
Fix: the EventBridge rule-target Lambda delivery path and S3's EventBridge Object Created emission, so the invoked function's CreateInvalidation lands in the distribution's invalidation state and is returned by ListInvalidations/GetInvalidation; and the CloudFront edge data-plane path that must serve stale bytes until that invalidation completes.
Traces: cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation, cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge

## Evidence
Suite: simparity-behavioral-aws-v1, service cloudfront__events — 10/61 passed (16%), 51 failed; 48 of those are already filed/owned, these 3 are not.

Cause 1 — [mismatch] cloudfront__events.cfn-deploy-eventbridge-rule-missing-permission-not-invoked
  Expected: after PutEvents of one matching entry, the relay queue yields >=1 message whose body.target == "allowed" and whose body.detail.requestParameters.originAccessControlConfig.name == the request's nonce; no message for the "blocked" target during a window of max(3x the observed positive latency, 15 s); then ApproximateNumberOfMessages == "0" and ApproximateNumberOfMessagesNotVisible == "0".
  Actual: the permitted target never relays — no matching message within the check's 120 s poll; "Error: Check timed out after 60.0s".

Cause 2 — [mismatch] cloudfront__events.eventbridge-rule-invokes-lambda-to-create-cloudfront-invalidation
  Expected: PutObject on the EventBridge-enabled bucket yields an invalidation whose InvalidationBatch.Paths.Items == ["/<key>"]; GetInvalidation returns the same Id and a Status of InProgress or Completed.
  Actual: no invalidation with that path appears within the check's 240 s poll (with repeated PutObject retries); "Error: Check timed out after 60.0s".

Cause 2 — [mismatch] cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge
  Expected: the S3 Object Created event drives the Lambda's CreateInvalidation; the edge serves the stale cached bytes before completion and the newly written bytes after.
  Actual: the chain never delivers and the edge never answers; "Error: Check timed out after 60.0s".

## Steps to Reproduce
Cause 1: deploy a CFN stack with an AWS::Events::Rule (pattern on a custom source/detail-type) carrying two AWS::Lambda::Function targets that both SendMessage to one AWS::SQS::Queue with a distinguishing target label, and an AWS::Lambda::Permission for Principal events.amazonaws.com on exactly ONE of them. PutEvents a single matching entry, then ReceiveMessage/DeleteMessage-drain the queue: expect a message from the permitted target, none from the unpermitted one, and both message counters to settle at "0".

Cause 2: create a bucket with NotificationConfiguration {EventBridgeConfiguration:{}}; CreateDistribution over it with the managed CachingDisabled/CachingOptimized policy; create a lambda.amazonaws.com-trusted role allowing cloudfront:CreateInvalidation; create a Lambda that calls CreateInvalidation with Paths ["/" + detail.object.key]; PutRule {"source":["aws.s3"],"detail-type":["Object Created"],"detail":{"bucket":{"name":["<bucket>"]}}} ENABLED; AddPermission for events.amazonaws.com with SourceArn=<rule arn>; PutTargets with the function ARN. PutObject a key, then poll ListInvalidations for Paths.Items == ["/<key>"] and call GetInvalidation to confirm the Id and a Status of InProgress/Completed. For the edge leg, cache the object through the distribution domain, overwrite it without triggering the rule, confirm the stale bytes are still served, then trigger the invalidation and confirm the new bytes.
