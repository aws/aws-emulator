Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge
Issue-Key: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge|run#58|Timeout|assert time.monotonic() < deadline, (
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront__events: the S3 ObjectCreated -> EventBridge -> Lambda -> CreateInvalidation chain never completes, so an updated object is never refreshed at the distribution edge

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: the cross-service invalidation chain driven by an S3 object-created event produces no edge refresh.
AWS: recorded behavior only for the chain as a whole (each hop is modelled: S3 EventBridge notification configuration, events PutRule/PutTargets with a Lambda target, lambda Invoke, cloudfront CreateInvalidation). Recorded behavior: writing a new version of an object to a bucket whose EventBridge notifications are enabled emits an "Object Created" event; the enabled rule matches it and invokes its permitted Lambda target; the function calls CreateInvalidation for the object's path; and once the invalidation reaches Completed, the next viewer request through the distribution's DomainName is a Miss that re-reads the origin and serves the NEW bytes instead of the stale cached ones.
Emulator: the check exhausts its 180 s budget — the chain never reaches the observable end state, so the edge keeps serving (or never serves) and the refreshed object never appears.
Why: the single most common CDN automation — invalidate on upload — cannot be built or validated against the emulator. A client wiring S3 notifications to an invalidation Lambda sees nothing happen and cannot tell which hop is at fault, and any deploy pipeline that depends on content being refreshed after upload silently serves stale content.
Fix: the hop that does not fire must be made to fire end to end: S3's EventBridge "Object Created" emission, the events rule's delivery to its Lambda target, the in-Lambda CloudFront CreateInvalidation call, and the edge's honouring of a Completed invalidation by re-reading the origin on the next request (the events rule-delivery path, the s3 notification emission, and the cloudfront invalidation effect in the edge cache).
Traces: cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge.

Note: several open cloudfront__events issues describe individual hops of this family — 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md, 20261107T192835Z-cloudfront-events-3-net-new-causes-a-matched.md, 20261107T042814Z-cloudfront-events-the-in-lambda-aws-sdk-has.md and 20261105T094343Z-cloudfront-events-the-lambda-python-runtime-provides-no.md. None of them carries this check's replay key, so none covers this failure; a fixer should expect the remaining gap to be whichever hop those fixes did not reach.

## Evidence
"Error: Check timed out after 180.0s".
Expected: after a new version of the object is written to the EventBridge-enabled bucket, the matched rule invokes the Lambda target, the target's CreateInvalidation completes, and the next GET through the distribution's DomainName answers with the NEW object bytes (X-Cache: Miss from cloudfront).
Actual: the end state is never reached within the 180 s budget — no refreshed content is observed.

## Steps to Reproduce
1. Point s3, events, lambda, iam and cloudfront (us-east-1) clients at the emulator.
2. Create a bucket, enable EventBridge notifications on it, put version 1 of an object.
3. Create a distribution over that bucket as origin and wait for Status Deployed; GET the object once so it is cached at the edge.
4. Create a Lambda function (with a role granting cloudfront:CreateInvalidation) that calls CreateInvalidation for the event's object key; create an events rule matching source aws.s3 / detail-type "Object Created" for that bucket, add the function as a target, and add the lambda:InvokeFunction permission for events.amazonaws.com.
5. Put version 2 of the object.
6. Poll ListInvalidations for a Completed invalidation naming the object's path, then GET the object through the distribution's DomainName -> the new bytes never arrive within the budget.
7. Or replay cloudfront__events.xsvc-cloudfront-1-s3-object-created-lambda-invalidation-refreshes-edge.
