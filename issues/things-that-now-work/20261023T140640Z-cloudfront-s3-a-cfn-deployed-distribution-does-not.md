Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.e2e-cloudfront-oac-s-forbidden-troubleshoot-lifecycle cloudfront__s3.obs-cloudfront-3-standard-logging-to-acl-disabled-bucket-rejected cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket
Issue-Key: cloudfront|GetDistribution|NoSuchDistribution
Issue-Key: cloudfront__s3.obs-cloudfront-3-standard-logging-to-acl-disabled-bucket-rejected|run#11|AssertionError|assert outcome == "error", (
Issue-Key: cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket|_poll#11|Timeout|time.sleep(interval_s)
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudFront/S3: a CFN-deployed distribution does not exist in CloudFront, standard-logging configuration is neither validated nor applied to the log bucket (3 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Three independent defects surfaced by the cloudfront/s3 cross-service behavioral checks. Each is its own root cause and its own work item; this issue bundles THREE causes and all three must be closed before submitting.

Cause 1 of 3: a distribution declared by a CloudFormation stack does not exist in CloudFront after the stack reports success.
AWS: recorded behavior only — a stack declaring AWS::CloudFront::Distribution (here with ConnectionMode tenant-only, an S3 origin and an AWS::CloudFront::OriginAccessControl) reaches CREATE_COMPLETE and its DistributionId output is a real distribution id, so GetDistribution on it returns 200 with the deployed DistributionConfig. Smithy: GetDistribution models NoSuchDistribution with httpError 404 — for an id CloudFront does not hold.
Emulator: the stack reaches CREATE_COMPLETE and hands back a DistributionId, and GetDistribution on that id fails 404 `NoSuchDistribution` "The specified distribution does not exist." — the AWS::CloudFront::Distribution resource was never provisioned into CloudFront (or Ref resolves to a synthesized name CloudFront does not know).
Why: the first read of every CloudFront estate deployed by template fails, and the error blames the client for an id the emulator itself produced; nothing downstream (OAC wiring, tenants, troubleshooting flows) can run.
Fix: the CloudFormation resource providers for AWS::CloudFront::Distribution / ::OriginAccessControl — create the resource in the cloudfront state and resolve Ref/Fn::GetAtt to its real physical id. Related open issue that does NOT carry this replay key: 20261023T000000Z-cloudformation-stacks-report-create-complete-but-provision-no-resources.md.
Traces: cloudfront__s3.e2e-cloudfront-oac-s-forbidden-troubleshoot-lifecycle.

Cause 2 of 3: CreateDistribution accepts standard logging to an S3 bucket that cannot receive it.
AWS: Smithy: CreateDistribution models InvalidArgument(400) among its rejections; a DistributionConfig whose Logging.Bucket names a bucket with ACLs disabled (ObjectOwnership=BucketOwnerEnforced) is refused, because CloudFront standard logging to S3 requires the log-delivery ACL grant.
Emulator: the create succeeds and returns a distribution id.
Why: a client is told logging is configured when no log will ever be delivered — the misconfiguration that AWS surfaces immediately becomes a silent data-loss bug.
Fix: Logging-target validation in the CreateDistribution handler of the cloudfront crate (resolve the bucket's ownership/ACL state before accepting the config).
Traces: cloudfront__s3.obs-cloudfront-3-standard-logging-to-acl-disabled-bucket-rejected.

Cause 3 of 3: enabling CloudFront standard logging does not grant the log-delivery principal FULL_CONTROL on the target bucket, so the check polling the bucket ACL never observes the grant and runs to the 60 s budget.
AWS: recorded behavior only — when a distribution is created with Logging enabled against an ACL-enabled bucket, the awslogsdelivery canonical user appears in the bucket ACL with FULL_CONTROL (that is how CloudFront is able to write log objects).
Emulator: the ACL is never modified; GetBucketAcl keeps returning only the bucket owner, so the poll loop never terminates.
Why: a client cannot verify (or operate) log delivery — the grant that makes logging work is absent, and the absence is only visible as a hang rather than an error.
Fix: the side effect of enabling standard logging in the CreateDistribution/UpdateDistribution handlers — add the log-delivery grant to the target bucket's ACL state in s3 (and keep GetBucketAcl reporting it).
Traces: cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket.

## Evidence
Cause 1: "_PhaseError: [phase 1/5 deploy] NoSuchDistribution: An error occurred (NoSuchDistribution) when calling the GetDistribution operation: The specified distribution does not exist." — raised on cf.get_distribution(Id=out['DistributionId']) where out is the describe_stacks Outputs map of a CREATE_COMPLETE stack whose template declares MultiTenantDistribution (AWS::CloudFront::Distribution) and TenantOAC (AWS::CloudFront::OriginAccessControl).
Cause 2: "AssertionError: CreateDistribution logging to an ACL-disabled bucket must be refused, but it created 'E7IYCGPKTANSF3'".
Cause 3: "Error: Check timed out after 60.0s" while polling the log bucket's ACL for the awslogsdelivery FULL_CONTROL grant after creating a logging-enabled distribution.

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::CloudFront::Distribution (+ AWS::CloudFront::OriginAccessControl, AWS::S3::Bucket) and Outputs DistributionId: !Ref <distribution>; wait for CREATE_COMPLETE; GetDistribution(Id=<that output>) -> expect 200; observe 404 NoSuchDistribution.
2. Cause 2: create a bucket with ObjectOwnership=BucketOwnerEnforced (ACLs disabled), then CreateDistribution with Logging={'Enabled':true,'Bucket':'<bucket>.s3.amazonaws.com','Prefix':''} -> expect 400 InvalidArgument; observe 201.
3. Cause 3: create a bucket with ACLs enabled, CreateDistribution with Logging enabled against it, then GetBucketAcl -> expect a FULL_CONTROL grant for the awslogsdelivery canonical user; observe only the owner grant.
