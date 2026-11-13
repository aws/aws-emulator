Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found
Issue-Key: cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found|_expect#16|AssertionError|raise AssertionError(
Issue-Commit: f1473582b6746d116724ae43d0fc6ffbe3d6e185

# AWS::S3Tables::TableBucket with an UnreferencedFileRemoval property is refused CREATE_FAILED (InvalidRequest, emulator-authored prose), so the stack rolls back

> _Note: this issue was identified against the code base at commit `f1473582b6746d116724ae43d0fc6ffbe3d6e185`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource provider for AWS::S3Tables::TableBucket rejects the resource type's own UnreferencedFileRemoval property instead of applying it, so every stack that templates a table bucket's unreferenced-file-removal maintenance settings fails to create and rolls back.

AWS: AWS::S3Tables::TableBucket carries an optional UnreferencedFileRemoval property with members Status, UnreferencedDays and NoncurrentDays (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-s3tables-tablebucket.html). The handler creates the bucket and applies those settings as the bucket's maintenance configuration — Smithy (com-amazonaws-s3tables-s3tablebuckets): PutTableBucketMaintenanceConfiguration PUT /buckets/{tableBucketARN}/maintenance/{type}, HTTP 204, type: TableBucketMaintenanceType enum whose only value is `icebergUnreferencedFileRemoval`, value: TableBucketMaintenanceConfigurationValue{status: MaintenanceStatus, settings: IcebergUnreferencedFileRemovalSettings{unreferencedDays, nonCurrentDays, @range min 1}}; the settings read back through GetTableBucketMaintenanceConfiguration GET /buckets/{tableBucketARN}/maintenance (200, required members tableBucketARN + configuration). The resource therefore reaches CREATE_COMPLETE, and UnreferencedFileRemoval is not create-only, so changing only its day counts is an in-place UPDATE_COMPLETE that keeps the bucket (same physical id, same createdAt).

Emulator: the Bucket resource goes CREATE_FAILED with HandlerErrorCode: InvalidRequest and the reason `Resource handler returned message: "this emulator does not apply the UnreferencedFileRemoval property of AWS::S3Tables::TableBucket"`, and the stack settles ROLLBACK_COMPLETE with StackStatusReason `The following resource(s) failed to create: [Bucket].` That text is emulator-authored prose about the emulator's own implementation; no AWS refusal class or message exists for a validly-typed UnreferencedFileRemoval block, so the whole in-place-update / retained-replacement sequence after it is unreachable.

Why: a client that templates the documented maintenance block — the normal way to configure unreferenced file removal declaratively — cannot deploy at all. The stack rolls back rather than producing the bucket, so every downstream resource and output of that template is lost, and the emulator reports a client-side InvalidRequest for a template AWS accepts, which sends a user hunting a non-existent template error.

Fix: the AWS::S3Tables::TableBucket resource provider in the CloudFormation resource-handler layer — it must accept the UnreferencedFileRemoval property, translate it to the bucket's icebergUnreferencedFileRemoval maintenance configuration (status + unreferencedDays/nonCurrentDays) at create, apply day-count-only changes as an in-place update, and never emit prose naming the emulator.

Traces: cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found (fails at step 1, the initial create).

## Evidence
Expected: CreateStack of a template whose only resource is {"Type": "AWS::S3Tables::TableBucket", "UpdateReplacePolicy": "Retain", "Properties": {"TableBucketName": <name>, "UnreferencedFileRemoval": {"Status": "Enabled", "UnreferencedDays": 4, "NoncurrentDays": 12}}} settles CREATE_COMPLETE, and GetTableBucketMaintenanceConfiguration on the created bucket reports configuration[icebergUnreferencedFileRemoval] = {status: Enabled, settings.icebergUnreferencedFileRemoval: {unreferencedDays: 4, nonCurrentDays: 12}} (the day values are the request's, not fixed text).

Actual: the stack settles 'ROLLBACK_COMPLETE' with reason 'The following resource(s) failed to create: [Bucket]. ' and the resource event ('Bucket', 'CREATE_FAILED', 'Resource handler returned message: "this emulator does not apply the UnreferencedFileRemoval property of AWS::S3Tables::TableBucket" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'). Fixed text of the diverging reason: `Resource handler returned message: "this emulator does not apply the UnreferencedFileRemoval property of AWS::S3Tables::TableBucket"`; the RequestToken is a per-run value. AssertionError raised by the check: "stack ended 'ROLLBACK_COMPLETE' ..., expected 'CREATE_COMPLETE'".

## Steps to Reproduce
1. CreateStack with a template declaring one AWS::S3Tables::TableBucket, UpdateReplacePolicy Retain, Properties {TableBucketName: <lowercase name>, UnreferencedFileRemoval: {Status: Enabled, UnreferencedDays: 4, NoncurrentDays: 12}} and Outputs {BucketRef: {Ref: Bucket}, BucketArn: {Fn::GetAtt: [Bucket, TableBucketARN]}}.
2. Poll DescribeStacks until the stack reaches a terminal status: it is ROLLBACK_COMPLETE, not CREATE_COMPLETE.
3. DescribeStackEvents shows Bucket CREATE_FAILED with HandlerErrorCode InvalidRequest and the emulator-authored message above.
4. Replay check cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found.
