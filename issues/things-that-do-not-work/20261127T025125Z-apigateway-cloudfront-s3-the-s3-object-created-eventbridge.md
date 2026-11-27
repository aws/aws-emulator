Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative|run#13|Timeout|time.sleep(5)
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative|run#14|Timeout|time.sleep(5)
Issue-Commit: 65ee76328c73fce07b2a3bd288dbf2c587c2316a

# apigateway__cloudfront__s3: the S3 Object Created → EventBridge → Lambda → CreateInvalidation pipeline still never delivers, so both negative-delivery checks reach no verdict

> _Note: this issue was identified against the code base at commit `65ee76328c73fce07b2a3bd288dbf2c587c2316a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets / PutObject (EventBridge rule target delivery) → CreateInvalidation

## Description
This issue carries 1 net-new root cause in the apigateway__cloudfront__s3 behavioral service, grounded in 2 failing checks. Neither check was marked ALREADY FILED, and the replay report lists neither as OWNED by an open issue. Open issues that look adjacent but do NOT carry these checks' replay keys: 20261101T214154Z-apigateway-cloudfront-s3-three-more-edge-eventing-root.md (it names these two check ids but its recorded replay keys point at different frames, so the report does not treat it as owning them), 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md, 20261125T053510Z-cloudfront-events-the-s3-object-created-eventbridge-lambda.md, 20261125T162859Z-cloudfront-events-the-distribution-s-published-domainname-is.md — they are named only so a fixer can see the family.

=== Cause 1 of 1: a matched S3 "Object Created" EventBridge event never invokes its Lambda target, so no CloudFront invalidation is ever created ===
AWS: a bucket carrying NotificationConfiguration.EventBridgeConfiguration emits an event with source "aws.s3", detail-type "Object Created", envelope resources = ["arn:aws:s3:::<bucket>"] and detail.object.key = the written key (fixed text: the source and detail-type strings; variable: bucket name and object key). An ENABLED rule whose EventPattern matches that event, with a Lambda target wired by PutTargets and a lambda:AddPermission grant for Principal events.amazonaws.com scoped to the rule ARN, invokes the function. The function's CreateInvalidation (Smithy cloudfront: POST /2020-05-31/distribution/{DistributionId}/invalidation, success 201, output Invalidation; errors AccessDenied 403, BatchTooLarge 413, NoSuchDistribution 404, InvalidArgument 400) then makes ListInvalidations report an entry whose InvalidationBatch.Paths.Items contains "/<key>" and whose Status is one of the modeled values InProgress / Completed. Conversely, with the invoke permission withheld, or with the rule's pattern scoped to a different bucket ARN, no invocation and therefore no invalidation occurs.
Emulator: the delivery leg is inert. In both checks the deliberately-broken negative window (60 s) passes only because nothing is ever delivered, and the positive control — grant lambda:InvokeFunction to events.amazonaws.com for the rule ARN (check 1), or PutRule the pattern back to arn:aws:s3:::<bucket> (check 2), then PutObject a new key — never produces an invalidation for that key. Both checks consume the whole per-check budget and end "Check timed out after 60.0s" with no invalidation ever listed for any key.
Why: a client wiring CloudFront cache invalidation off S3 uploads gets no invalidation at all. Worse for a permission-boundary test: a correct wiring and a deliberately broken one (missing events.amazonaws.com invoke grant; pattern scoped to the wrong bucket ARN) are indistinguishable, so the emulator silently green-lights misconfigured infrastructure instead of proving the boundary.
Fix: the EventBridge rule-target delivery path for Lambda targets — including the evaluation of the target function's resource-based policy and the rule EventPattern match against the envelope's resources — together with S3's EventBridge "Object Created" notification emission, so the invoked function's CreateInvalidation lands in the distribution's invalidation state and is visible to ListInvalidations.
Traces: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative, apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative

## Evidence
Suite: simparity-behavioral-aws-v1, service apigateway__cloudfront__s3 — 56/61 passed (92%), 5 failed; 3 of those are OWNED/ALREADY FILED, these 2 are not.

Cause 1 — [mismatch] apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative
  Expected: after lam.add_permission(StatementId=..., Action="lambda:InvokeFunction", Principal="events.amazonaws.com", SourceArn=<rule arn>), a PutObject of "with-permission/object.txt" yields, within 120 s, a CloudFront invalidation whose InvalidationBatch.Paths.Items contains "/with-permission/object.txt".
  Actual: no invalidation is ever listed; the check ends "Error: Check timed out after 60.0s" having never observed the pipeline deliver.

Cause 1 — [mismatch] apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative
  Expected: after events.put_rule corrects EventPattern.resources to ["arn:aws:s3:::<bucket>"] (State=ENABLED), a PutObject of "correct-arn/object.txt" yields an invalidation with Status in (InProgress, Completed) whose Paths.Items contains "/correct-arn/object.txt".
  Actual: no invalidation appears for the matching key either; the check ends "Error: Check timed out after 60.0s".

In both runs the negative phase is satisfied vacuously (nothing is ever delivered for the mis-permissioned target or the mismatched ARN), so the checks cannot distinguish the enforced boundary from a permanently inert pipeline.

## Steps to Reproduce
1. CreateBucket and PutBucketNotificationConfiguration with {"EventBridgeConfiguration": {}}.
2. CreateDistribution (any origin) and note its Id.
3. Create an IAM role trusted by lambda.amazonaws.com allowing cloudfront:CreateInvalidation; create a Lambda function that calls CreateInvalidation for detail.object.key of the event it receives.
4. PutRule Name=<rule> EventPattern={"source":["aws.s3"],"detail-type":["Object Created"],"resources":["arn:aws:s3:::<bucket>"]} State=ENABLED; PutTargets with the function ARN; AddPermission on the function for Principal events.amazonaws.com with SourceArn=<rule arn>.
5. PutObject a key into the bucket and poll ListInvalidations for an entry whose InvalidationBatch.Paths.Items contains "/<key>" — AWS produces one; the emulator produces none.
6. Repeat with (a) the AddPermission grant withheld and (b) the rule pattern's resources scoped to a different bucket ARN, then apply the fix (AddPermission / PutRule with the correct ARN) and write a new key: AWS stays silent for the broken wiring and delivers after the fix; the emulator is silent in both cases.
