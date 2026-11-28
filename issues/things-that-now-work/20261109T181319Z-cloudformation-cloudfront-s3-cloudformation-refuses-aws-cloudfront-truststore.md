Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__s3.deploy-cloudfront-truststore-round-trip-and-delete cloudformation__cloudfront__s3.deploy-cloudfront-truststore-invalid-ca-bundle-rejected
Issue-Key: cloudformation__cloudfront__s3|CreateStack|ValidationError
Issue-Key: cloudformation__cloudfront__s3|UpdateStack|ValidationError
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudformation__cloudfront__s3: CloudFormation refuses AWS::CloudFront::TrustStore as an unrecognized resource type, so no trust-store stack can be created or updated

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::CloudFront::TrustStore.

AWS: AWS::CloudFront::TrustStore is a supported CloudFormation resource type. CreateStack with a template declaring one is accepted (HTTP 200 with a StackId) and the stack provisions a CloudFront trust store from the referenced S3 CA bundle; an invalid CA bundle is refused later, by the resource handler, with the stack ending ROLLBACK_COMPLETE and a resource-handler failure reason — not at template validation. UpdateStack behaves the same way (recorded behavior only — CloudFormation's Smithy model carries the ValidationError class but not the type registry).

Emulator: both CreateStack and UpdateStack are refused 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::CloudFront::TrustStore]". The fixed text is "Template format error: Unrecognized resource types: [" + the declared types the registry does not know + "]"; the bracketed list is request-derived.

Why: the stack never comes into existence, so a client cannot provision a CloudFront trust store through IaC at all, and the negative contract (an invalid CA bundle must fail inside the resource handler and roll the stack back) is unreachable — the two outcomes AWS distinguishes, template error vs. handler error, collapse into one.

Fix: register AWS::CloudFront::TrustStore in CloudFormation's resource-type registry and add its provisioner, wired to the CloudFront service's trust-store create/update/delete path, so template validation passes and refusals come from the resource handler.

Traces: cloudformation__cloudfront__s3.deploy-cloudfront-truststore-round-trip-and-delete (CreateStack refused), cloudformation__cloudfront__s3.deploy-cloudfront-truststore-invalid-ca-bundle-rejected (UpdateStack refused). Note: CreateTrustStore itself is a separate, already-filed unimplemented scaffold stub in the cloudfront service; this issue is about CloudFormation's template handling under the cloudformation__cloudfront__s3 heading.

## Evidence
Expected: CreateStack/UpdateStack accepted (200 + StackId); an invalid CA bundle surfaces as a resource-handler failure with the stack ROLLBACK_COMPLETE.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudFront::TrustStore]
and botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the UpdateStack operation: Template format error: Unrecognized resource types: [AWS::CloudFront::TrustStore]

## Steps to Reproduce
1. Put a CA bundle object in an S3 bucket.
2. CreateStack with a template whose Resources declare a single AWS::CloudFront::TrustStore pointing at that object.
3. The call is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::CloudFront::TrustStore]"; the same template passed to UpdateStack is refused identically.
