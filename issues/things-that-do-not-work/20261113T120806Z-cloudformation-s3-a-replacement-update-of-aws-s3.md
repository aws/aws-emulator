Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#31|AssertionError|assert _in_order(
Issue-Commit: d0f76c0ba03e1debbd23029512a6c9d3948c5ea6

# cloudformation__s3: a replacement update of AWS::S3::Bucket is emitted as CREATE_*/DELETE_* events and ignores UpdateReplacePolicy Retain (1 cause)

> _Note: this issue was identified against the code base at commit `d0f76c0ba03e1debbd23029512a6c9d3948c5ea6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: an UpdateStack that changes the BucketName of an AWS::S3::Bucket declaring UpdateReplacePolicy: Retain is reported with the create/delete vocabulary instead of the UPDATE_* replacement vocabulary, and the old (retained) bucket is deleted anyway. Failing check: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete.

AWS: recorded behavior only — CloudFormation resource attributes (UpdateReplacePolicy) and stack-event vocabulary are template/provisioner level, not Smithy-modelled; the S3 refusal quoted below is S3's own. When a property change requires replacement, CloudFormation emits, for that logical resource and in this order: UPDATE_IN_PROGRESS with ResourceStatusReason exactly "Requested update requires the creation of a new physical resource; hence creating one.", then UPDATE_IN_PROGRESS "Resource creation Initiated", then UPDATE_COMPLETE carrying the NEW PhysicalResourceId, and finally — because UpdateReplacePolicy: Retain is declared — DELETE_SKIPPED carrying the OLD PhysicalResourceId. It issues no DeleteBucket for the retained resource at all, so no DELETE_IN_PROGRESS / DELETE_COMPLETE / DELETE_FAILED event for it exists and the old bucket and its objects survive the update. The fixed text in those reasons is the quoted sentences; the physical ids are values taken from the request/state (the templated bucket names).

Emulator: for the replaced logical resource the update's events are ('CREATE_IN_PROGRESS', None), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', <new bucket>), ('CREATE_COMPLETE', <new bucket>) — no UPDATE_IN_PROGRESS carrying the replacement sentence and no UPDATE_COMPLETE row at all — followed by ('DELETE_IN_PROGRESS', <old bucket>) and ('DELETE_FAILED', 'Resource handler returned message: "The bucket you tried to delete is not empty. You must delete all versions in the bucket. (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: 3)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)', <old bucket>). So the DELETE_SKIPPED row AWS emits is missing, and a DeleteBucket is issued against a resource the template told CloudFormation to retain (the request ids, extended request id and request token in that reason are per-run values, not requirements).

Why: a client reading stack events cannot tell a replacement from a fresh create (no UPDATE_* rows, no replacement reason), and a client relying on UpdateReplacePolicy: Retain to carry data across a rename loses that guarantee: the emulator tries to delete the retained bucket, so any non-empty bucket turns the rename into a DELETE_FAILED event (and, on other paths, a rolled-back update) where AWS would never have touched the data.

Related open issue, which does NOT carry this check's replay key: 20261107T060626Z-s3-cloudformation-updatereplacepolicy-retain-is-ignored-when-an.md describes the same Retain defect from check s3.deploy-s3-1-rename-onto-owned-bucket-rejected; the same provisioner behaviour is also filed per-service for sqs/kms/logs/events/iam/lambda/glue/dynamodb etc. This issue adds the AWS::S3::Bucket replacement-event vocabulary and the missing DELETE_SKIPPED row observed by the check named above.

Fix: the CloudFormation stack provisioner's update/replacement step — the resource-event emitter must use the UPDATE_* replacement vocabulary (UPDATE_IN_PROGRESS with the replacement reason, UPDATE_COMPLETE on the new physical id) and the cleanup step must honour the resource's UpdateReplacePolicy, emitting DELETE_SKIPPED for the old physical id and issuing no delete when Retain is declared.

Traces: cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete.

## Evidence
Check cloudformation__s3.cfn-s3-bucket-create-update-in-place-replace-retained-then-delete [mismatch].

Expected (rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical resource, oldest first): ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', *), then ('UPDATE_COMPLETE', *, <new bucket>), then ('DELETE_SKIPPED', *, <old bucket>), with NO DELETE_IN_PROGRESS and NO DELETE_COMPLETE row.

Actual: AssertionError — "the replacement must report creating a new resource, complete as '<new bucket>' and skip deleting '<old bucket>' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new bucket>'), ('CREATE_COMPLETE', None, '<new bucket>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_FAILED', 'Resource handler returned message: \"The bucket you tried to delete is not empty. You must delete all versions in the bucket. (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: 3)\" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)', '<old bucket>')]".

## Steps to Reproduce
1) CreateStack with one resource: Bucket: {Type: AWS::S3::Bucket, UpdateReplacePolicy: Retain, DeletionPolicy: Delete, Properties: {BucketName: <name-a>, Tags: [...]}}; wait CREATE_COMPLETE.
2) PutObject one or more objects into <name-a> (so a delete of it would fail), then enable versioning via an in-place UpdateStack.
3) UpdateStack with the same template but BucketName: <name-b> (a free name) — a replacing change.
4) DescribeStackEvents(StackName=<stack>) and read the rows for the Bucket logical id produced by that update: the emulator reports CREATE_IN_PROGRESS/CREATE_COMPLETE for <name-b> and then DELETE_IN_PROGRESS + DELETE_FAILED for <name-a>, instead of UPDATE_IN_PROGRESS with "Requested update requires the creation of a new physical resource; hence creating one.", UPDATE_COMPLETE on <name-b> and DELETE_SKIPPED on <name-a>.
