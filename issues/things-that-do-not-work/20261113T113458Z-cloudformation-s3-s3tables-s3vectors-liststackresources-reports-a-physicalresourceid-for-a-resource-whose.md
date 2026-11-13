Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back
Issue-Key: cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back|run#13|AssertionError|assert rows == [("AWS::S3Tables::TableBucket", "DELETE_COMPLETE", None)], (
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# ListStackResources reports a PhysicalResourceId for a resource whose create was refused and rolled back

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
Cause 1 of 1: a CREATE_FAILED/rolled-back stack resource is still given a physical id.

AWS: StackResourceSummary.PhysicalResourceId is an OPTIONAL member (Smithy: com.amazonaws.cloudformation#StackResourceSummary — LogicalResourceId, ResourceType, LastUpdatedTimestamp and ResourceStatus are required; PhysicalResourceId and ResourceStatusReason are not), documented as "the name or unique identifier that corresponds to a physical instance ID of the resource". When the underlying service REFUSES the create (here S3 Tables rejecting a TableBucketName that breaks its lower-case/charset rule), no physical instance ever exists, so CloudFormation records no physical id: the resource row returned by ListStackResources (and DescribeStackResources) after the stack reaches ROLLBACK_COMPLETE carries ResourceStatus DELETE_COMPLETE with PhysicalResourceId ABSENT. Only resources that were actually created and then deleted during rollback carry an id.
Emulator: the rollback itself is modelled correctly — the resource emits CREATE_FAILED and the stack settles in ROLLBACK_COMPLETE — but the resource summary still carries a fabricated physical id for the refused resource. The replayed row was ('AWS::S3Tables::TableBucket', 'DELETE_COMPLETE', 'arn:aws:s3tables:us-west-2:000000000000:bucket/Upper_Case_Tab') where AWS answers ('AWS::S3Tables::TableBucket', 'DELETE_COMPLETE', None). The id is derived from the template's requested name (ARN template arn:aws:s3tables:<region>:<account>:bucket/<requested TableBucketName>; region, account and name are request/state values, not part of the requirement), i.e. the provisioner mints the physical id from the template BEFORE the create call and does not clear it when the create is refused.
Why: a client that reconciles CloudFormation state against reality — drift detection, importers, cleanup/garbage-collection tooling, or any caller that treats a present PhysicalResourceId as proof the resource exists — reads the failed row as a real resource and will try to describe, tag, import or delete an identifier that was never created, getting NotFound errors or deleting the wrong thing. It also makes a refused create indistinguishable from a created-then-rolled-back one.
Fix: the CloudFormation provisioner's resource-record bookkeeping — the physical id must be assigned only from the provider's successful create result, and must be left unset (and omitted from the ListStackResources/DescribeStackResources serializer) for a resource whose create failed.
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back.

## Evidence
AssertionError: the rolled-back stack lists [('AWS::S3Tables::TableBucket', 'DELETE_COMPLETE', 'arn:aws:s3tables:us-west-2:000000000000:bucket/Upper_Case_Tab')]; the refused bucket never got a physical id

Expected (AWS): rows == [("AWS::S3Tables::TableBucket", "DELETE_COMPLETE", None)] — PhysicalResourceId absent from the StackResourceSummary.
Actual (emulator): rows == [("AWS::S3Tables::TableBucket", "DELETE_COMPLETE", "arn:aws:s3tables:<region>:<account>:bucket/<requested-name>")] — a physical id minted from the rejected template property.

Preceding assertions in the same check passed, so the refusal and rollback are correct: the stack settled in ROLLBACK_COMPLETE and the 'Tables' logical resource emitted CREATE_FAILED. The only divergence is the surviving PhysicalResourceId.

## Steps to Reproduce
1. CreateStack with a template whose single resource is an AWS::S3Tables::TableBucket with a TableBucketName that S3 Tables refuses (e.g. an upper-case/underscored name such as "Upper_Case_Tab").
2. Poll DescribeStacks until the stack leaves *_IN_PROGRESS; it reaches ROLLBACK_COMPLETE and DescribeStackEvents shows CREATE_FAILED for the logical resource.
3. Call ListStackResources (paginated) for that stack and read the single StackResourceSummary.
4. Observe PhysicalResourceId is present (an s3tables bucket ARN built from the rejected name); AWS omits the member entirely because the bucket was never created.
