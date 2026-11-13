Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag[Key="aws:cloudformation:logical-id"]
Issue-Key: s3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-id"]
Issue-Key: s3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-name"]
Issue-Commit: 63c64ce081b9c54ca8821134bfa1b94c44bc83e7

# Stack-created bucket tag set lacks the three aws:cloudformation reserved keys

> _Note: this issue was identified against the code base at commit `63c64ce081b9c54ca8821134bfa1b94c44bc83e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketTagging

## Description
One net-new root cause of service `s3`. The replay reports this cluster with stub_path='' and owned_by=[], so no open issue carries its replay key. Two open issues describe the same shape on the same fixture step but neither OWNS this key — 20261103T171935Z-s3-getbuckettagging-omits-the-aws-cloudformation-tags-set.md (Issue-Key 's3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag') and 20261104T173700Z-s3-getbuckettagging-returns-no-aws-cloudformation-reserved-tags.md (Issue-Key 's3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag[Key="aws'), while the cluster reported now keys on the per-tag paths $.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-id"] / stack-name / logical-id. The divergence is therefore still live and must be closed here; closing all three together is correct.

Cause 1 of 1: a bucket provisioned by a CloudFormation stack carries none of the three reserved stack-ownership tags, so its tag set cannot be used to find the owning stack.
AWS: recorded behavior only for the tag provenance — the S3 Smithy model only states the container (Smithy: GetBucketTagging GET /{Bucket}?tagging, output shape GetBucketTaggingOutput @xmlName Tagging with required member TagSet, a list of Key/Value Tag pairs; no modeled errors) and says nothing about reserved keys. AWS CloudFormation stamps every resource it provisions — here an AWS::S3::Bucket — with three reserved tags IN ADDITION to the tags the template declared: aws:cloudformation:stack-id = the owning stack's ARN, aws:cloudformation:stack-name = the stack's name, aws:cloudformation:logical-id = the bucket's logical id in the template. GetBucketTagging answers 200 and returns all of them inside Tagging.TagSet. The three KEYS are the fixed requirement; their values are taken from the owning stack (ARN, name) and from the template (the logical id).
Emulator: GetBucketTagging answers 200 but Tagging.TagSet holds only the template-declared tags — all three aws:cloudformation:* Tag entries are absent.
Why: those three tags ARE the bucket-to-stack reverse lookup. A client holding only a bucket name cannot discover the stack that owns it, cannot tell a stack-managed bucket from a hand-made one, and any stack-scoped tag report or cost/ownership attribution over buckets comes back with no stack attribution.
Fix: the AWS::S3::Bucket provisioning path in the CloudFormation resource provider must merge the three reserved tags into the bucket tag set that the S3 GetBucketTagging handler serializes out of bucket state, alongside the template's Tags.
Traces: cloudformation-refarch-s3ev-stack-lookup-from-bucket#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Divergence cluster: divergence_key 'service=s3|GetBucketTagging|status=200->200|keys=$.Tagging.TagSet.Tag[Key="aws:cloudformation:logical-id"],$.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-id"],$.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-name"]', verdict mismatch, affected_count 1, stub_path='', owned_by=[]. Fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket, step task#3, operation GetBucketTagging. Status 200 expected and 200 actual, so only the body diverges:
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-id"]: missing — expected {Key: 'aws:cloudformation:stack-id', Value: the owning stack's ARN}, got None.
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-name"]: missing — expected {Key: 'aws:cloudformation:stack-name', Value: the owning stack's name}, got None.
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:logical-id"]: missing — expected {Key: 'aws:cloudformation:logical-id', Value: the bucket's logical id in the template ('EventNotificationBucket' in this recording)}, got None.
The stack ARN, its uuid, the generated stack name and the probe name suffixes in the recording are per-run values filling those slots; the requirement is the presence of the three reserved keys with values derived from the owning stack and the template.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Replay fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket step task#3: deploy a CloudFormation stack declaring an AWS::S3::Bucket (logical id EventNotificationBucket) with its own template tag, wait for CREATE_COMPLETE, then call S3 GetBucketTagging on the created bucket.
3. Inspect Tagging.TagSet: it must contain aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (EventNotificationBucket) in addition to the template tag. Observe that only the template tag is returned.
