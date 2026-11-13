Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deploy-cloudfront-1-distribution-nonexistent-cache-policy-rejected-rolls-back cloudfront.deploy-cache-policy-enforced-via-cloudformation
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deploy-cloudfront-1-distribution-nonexistent-cache-policy-rejected-rolls-back|run#25|AssertionError|assert needle in reason, (
Issue-Key: cloudfront.deploy-cache-policy-enforced-via-cloudformation|_poll_until_header#21|Timeout|time.sleep(interval_s)
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudfront: 2 net-new causes — the CFN AWS::CloudFront::Distribution handler never defaults CustomOriginConfig HTTPPort/HTTPSPort, and a CFN-deployed cache-policy distribution never reaches a servable state

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Two net-new root causes in the CloudFormation-to-CloudFront provisioning path. Both are reached through CloudFormation but are failures of the AWS::CloudFront::Distribution / AWS::CloudFront::CachePolicy resource bridge into the cloudfront service, which is the service heading the suite files both checks under.

Cause 1 of 2: the CloudFormation handler for AWS::CloudFront::Distribution does not apply the resource type's own defaults for Origin.CustomOriginConfig.HTTPPort and .HTTPSPort, so it submits null for them and the create is refused with a null-member validation error — masking the refusal AWS would actually produce.
AWS: Smithy (cloudfront 2020-05-31): CustomOriginConfig.HTTPPort and .HTTPSPort are `required: true` on the wire, so the CloudFront API itself never accepts null for them. The AWS::CloudFront::Distribution resource type, however, documents both as optional with defaults (HTTPPort 80, HTTPSPort 443); the resource handler is what fills them in before calling CreateDistribution. Consequently a template that gives only OriginProtocolPolicy (and optionally HTTPSPort) is valid and reaches CloudFront with the ports defaulted, and CloudFront then applies its real validation — for a DefaultCacheBehavior.CachePolicyId naming no cache policy it refuses the distribution and the resource ends CREATE_FAILED whose ResourceStatusReason names the missing cache policy (recorded expectation: the reason contains "The specified cache policy does not exist.", the fixed text of CloudFront's NoSuchCachePolicy, httpError 404).
Emulator: the create never gets that far. The Distribution resource ends CREATE_FAILED with ResourceStatusReason `Resource handler returned message: "1 validation error detected: Value null at 'distributionConfigWithTags.distributionConfig.origins.items.1.member.customOriginConfig.hTTPPort' failed to satisfy constraint: Member must not be null" (RequestToken: <per-run token>, HandlerErrorCode: InvalidRequest)`. The fixed text of that message is the constraint sentence and the `Member must not be null` clause; the variable parts are the member path (here origins item index 1, member customOriginConfig.hTTPPort) and the per-run RequestToken. The divergence is that it is produced at all: the handler passed a null the template was never required to supply, so the modeled cache-policy refusal is never evaluated and the ROLLBACK that should delete the already-created CachePolicy and OriginAccessControl is driven by the wrong failure.
Why: every infrastructure-as-code trajectory that declares a CloudFront origin as a custom origin without spelling out both ports — the overwhelmingly common shape for API Gateway, ALB and Lambda-URL origins — fails outright on the emulator while deploying on AWS. And for templates that do fail for a real reason, the caller is handed a synthetic null-member error instead of the CloudFront refusal it must match on.
Fix: the CloudFormation resource handler for AWS::CloudFront::Distribution — its property-to-DistributionConfig mapping must apply the resource type's documented defaults (HTTPPort 80, HTTPSPort 443) for CustomOriginConfig before calling CreateDistribution, and the handler must surface CloudFront's own refusal (NoSuchCachePolicy and friends) as the resource's ResourceStatusReason.
Traces: cloudfront.deploy-cloudfront-1-distribution-nonexistent-cache-policy-rejected-rolls-back.

Cause 2 of 2: a stack that declares an AWS::CloudFront::CachePolicy together with a distribution that references it never reaches a state where the distribution serves, so the whole check expires against its run budget.
AWS: recorded behavior only. The recording shows CreateStack of a template with AWS::S3::Bucket + AWS::CloudFront::OriginAccessControl + AWS::CloudFront::CachePolicy (QueryStringBehavior none) + AWS::CloudFront::Distribution reaching CREATE_COMPLETE, the stack Outputs carrying DistributionDomainName/DistributionId/CachePolicyId, and the edge then answering 200 for the default root object with X-Cache "Miss from cloudfront" and then "Hit from cloudfront" for a request that differs only in its query string (the cache policy excludes query strings from the cache key).
Emulator: the check makes no observable progress and the runner ends it with "Check timed out after 60.0s" — no stack status, no HTTP response. Nothing beyond the stack create was observable, so the stall is in reaching CREATE_COMPLETE or in the edge ever answering. This is the same resource bridge as the already-open 20261110T105638Z-cloudfront-reopens-a-cloudformation-created-aws-cloudfront-cachepolicy.md (GetCachePolicyConfig answers 404 NoSuchCachePolicy for a physical id CFN reported) and 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md, which that issue records as previously owning this check with a NoSuchDistribution refusal; neither issue carries this check's current replay key, so it is filed here.
Why: a caller cannot deploy a cache policy and a distribution in one template and then exercise the cache key — the trajectory hangs rather than refusing, so there is not even an error to program against.
Fix: the CloudFormation resource handlers for AWS::CloudFront::CachePolicy and AWS::CloudFront::Distribution and the cloudfront state they register into (the CFN-to-cloudfront resource bridge), plus the distribution's edge data plane that must answer on the domain the stack output reports.
Traces: cloudfront.deploy-cache-policy-enforced-via-cloudformation.

## Evidence
Cause 1 — cloudfront.deploy-cloudfront-1-distribution-nonexistent-cache-policy-rejected-rolls-back:
  AssertionError: Dist CREATE_FAILED reason must name the missing cache policy ('The specified cache policy does not exist.'), got 'Resource handler returned message: "1 validation error detected: Value null at \'distributionConfigWithTags.distributionConfig.origins.items.1.member.customOriginConfig.hTTPPort\' failed to satisfy constraint: Member must not be null" (RequestToken: e3803256-1e4d-1895-586d-7fe4fc30a1a2, HandlerErrorCode: InvalidRequest)'
  Expected: the Distribution resource reaches CREATE_FAILED with a ResourceStatusReason naming CloudFront's NoSuchCachePolicy text "The specified cache policy does not exist.", and the stack reaches ROLLBACK_COMPLETE having deleted the CachePolicy and OriginAccessControl it created.
  Actual: CREATE_FAILED for a null CustomOriginConfig.HTTPPort the template was not required to supply (AWS::CloudFront::Distribution defaults it to 80).

Cause 2 — cloudfront.deploy-cache-policy-enforced-via-cloudformation:
  Error: Check timed out after 60.0s
  Expected: CreateStack -> CREATE_COMPLETE; Outputs DistributionDomainName/DistributionId/CachePolicyId; GET / -> 200 with X-Cache "Miss from cloudfront", then GET /?v=2 -> 200 with X-Cache "Hit from cloudfront" (query string excluded from the cache key).
  Actual: no stack status and no HTTP response observed before the run budget expired.

## Steps to Reproduce
Cause 1:
1. Point cloudformation and cloudfront clients at the emulator.
2. CreateStack with a template containing AWS::CloudFront::CachePolicy, AWS::CloudFront::OriginAccessControl, and an AWS::CloudFront::Distribution whose Origins include a custom origin giving only OriginProtocolPolicy (no HTTPPort), and whose DefaultCacheBehavior.CachePolicyId is a random UUID that names no cache policy.
3. Poll DescribeStackEvents for the Distribution resource.
4. Observe CREATE_FAILED with "Value null at '...customOriginConfig.hTTPPort' failed to satisfy constraint: Member must not be null" instead of a reason naming "The specified cache policy does not exist.".

Cause 2:
1. CreateStack with a template containing AWS::S3::Bucket + AWS::CloudFront::OriginAccessControl + AWS::CloudFront::CachePolicy (QueryStringBehavior none) + AWS::CloudFront::Distribution referencing both, with Outputs for the distribution domain, distribution id and cache policy id.
2. Wait for stack_create_complete, read the outputs, PUT an object at the DefaultRootObject key, then GET https://<DistributionDomainName>/ and https://<DistributionDomainName>/?v=2.
3. Observe that the run never progresses to a servable distribution.
4. Or replay the two check ids in trace_ids.
