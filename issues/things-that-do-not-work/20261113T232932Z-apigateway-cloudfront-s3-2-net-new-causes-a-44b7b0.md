Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive
Issue-Key: apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive|run#18|AssertionError|assert final_status == "UPDATE_COMPLETE", f"stack update ended in {final_status}"
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# apigateway__cloudfront__s3: 2 net-new causes — a CFN stack with a CloudFront distribution on an API Gateway custom origin rolls back on create, and repointing a distribution's S3 origin by stack UPDATE rolls back instead of completing

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Two net-new root causes in the CloudFormation-driven CloudFront provisioning path that the apigateway__cloudfront__s3 checks exercise.

Cause 1 of 2: a stack whose AWS::CloudFront::Distribution origins are API Gateway custom origins is refused and rolled back at create.
AWS: recorded behavior only for the stack outcome — the recording shows CreateStack of a template containing AWS::ApiGateway::RestApi + Deployment + Stage and two AWS::CloudFront::Distribution resources over that API Gateway origin reaching CREATE_COMPLETE, after which the distribution WITHOUT the stage OriginPath passes API Gateway's 403 through to the viewer while the one with OriginPath /prod reaches the mock integration. The origin in that template is a CustomOriginConfig giving OriginProtocolPolicy https-only, OriginSSLProtocols [TLSv1.2] and HTTPSPort 443, and no HTTPPort — AWS::CloudFront::Distribution documents HTTPPort as optional and defaults it to 80 (Smithy, cloudfront 2020-05-31: CustomOriginConfig.HTTPPort/.HTTPSPort are `required: true` on the WIRE, which is exactly why the resource handler, not the template author, must supply them).
Emulator: the stack never reaches CREATE_COMPLETE; it ends in ROLLBACK_COMPLETE, so neither distribution exists and the pass-through/OriginPath contract is unreachable. Hypothesis: the same handler defect the cloudfront suite records verbatim for a CustomOriginConfig without HTTPPort — the resource handler forwards null and the create is refused with `1 validation error detected: Value null at 'distributionConfigWithTags.distributionConfig.origins.items.<n>.member.customOriginConfig.hTTPPort' failed to satisfy constraint: Member must not be null` (see cloudfront.deploy-cloudfront-1-distribution-nonexistent-cache-policy-rejected-rolls-back, filed separately under the cloudfront service). The recorded evidence here is only the ROLLBACK_COMPLETE status.
Why: the single most common CloudFront-over-API-Gateway template shape cannot be deployed at all, so no edge trajectory in front of a REST API — including the negative one that must see API Gateway's 403 passed through — is reachable.
Fix: the CloudFormation resource handler for AWS::CloudFront::Distribution — apply the resource type's documented CustomOriginConfig port defaults before calling CreateDistribution, and record the underlying resource failure in the rolled-back stack's events.
Traces: apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative.

Cause 2 of 2: UpdateStack that repoints a distribution's S3 origin DomainName (through a parameter and Fn::If) fails and rolls the stack back.
AWS: recorded behavior only. The recording shows a stack with two private buckets, an OriginAccessControl and one AWS::CloudFront::Distribution whose single origin's DomainName is Fn::If on the ActiveBucket parameter reaching CREATE_COMPLETE and serving bucket A's object; UpdateStack with UsePreviousTemplate and ActiveBucket=B then reaches UPDATE_COMPLETE, and the edge subsequently serves bucket B's object — an in-place distribution update, never a replacement.
Emulator: the update terminates in UPDATE_ROLLBACK_COMPLETE, so the distribution keeps the old origin and the new-origin content is never served. No per-resource reason was observable from the check.
Why: a client cannot change a live distribution's origin by template — the standard blue/green origin cutover. The update appears to fail and silently reverts, which a caller reading only the final status interprets as its own template being invalid.
Fix: the CloudFormation update path for AWS::CloudFront::Distribution — an origin DomainName change must be applied as an in-place UpdateDistribution (read the current config/ETag, replace the origin, write it back) rather than failing the resource and rolling the stack back.
Traces: apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive.

## Evidence
Cause 1 — apigateway__cloudfront__s3.deploy-cloudfront-apigateway-origin-stack-negative:
  AssertionError: Stack sim2real-xsvc-stk-01292a8d6871 ended in ROLLBACK_COMPLETE
  Expected: CREATE_COMPLETE, then the no-OriginPath distribution passes API Gateway's 403 to the viewer and the /prod-OriginPath distribution reaches the mock integration.
  Actual: ROLLBACK_COMPLETE — the stack's CloudFront distributions were never created. (Stack name is a per-run value.)

Cause 2 — apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive:
  AssertionError: stack update ended in UPDATE_ROLLBACK_COMPLETE
  Expected: UPDATE_COMPLETE after UpdateStack(UsePreviousTemplate=True, Parameters=[ActiveBucket=B]), and the edge then serving the bucket-B object for GET /index.html.
  Actual: UPDATE_ROLLBACK_COMPLETE; the distribution keeps the bucket-A origin.

## Steps to Reproduce
Cause 1:
1. Point cloudformation, apigateway, cloudfront and s3 clients at the emulator.
2. CreateStack with a template containing AWS::ApiGateway::RestApi + Method (MOCK integration) + Deployment + Stage "prod", and two AWS::CloudFront::Distribution resources whose Origins entry uses CustomOriginConfig {OriginProtocolPolicy: https-only, OriginSSLProtocols: [TLSv1.2], HTTPSPort: 443} (no HTTPPort) over the API's execute-api domain — one with OriginPath /prod, one without.
3. Poll DescribeStacks until a terminal status.
4. Observe ROLLBACK_COMPLETE instead of CREATE_COMPLETE.

Cause 2:
1. CreateStack with a template declaring an ActiveBucket parameter (A|B), buckets BucketA/BucketB, an AWS::CloudFront::OriginAccessControl, and one AWS::CloudFront::Distribution whose origin DomainName is Fn::If [IsBucketA, GetAtt BucketA.RegionalDomainName, GetAtt BucketB.RegionalDomainName]; wait for CREATE_COMPLETE.
2. PutObject index.html into both buckets and confirm the edge serves bucket A's body.
3. UpdateStack(StackName, UsePreviousTemplate=True, Parameters=[ActiveBucket=B], Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]).
4. Poll DescribeStacks — observe UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
5. Or replay the two check ids in trace_ids.
