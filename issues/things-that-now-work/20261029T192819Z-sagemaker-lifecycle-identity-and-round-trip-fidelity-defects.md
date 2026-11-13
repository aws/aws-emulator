Issue-Source: trace-sweep-divergence
Issue-Trace: api-model-package-group-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-model-package-group-lifecycle sagemaker-model-package-group-delete-then-recreate api-resource-tagging-lifecycle sagemaker-ai-recommendation-job-workflow sagemaker-hub-content-reference-lifecycle sagemaker-hub-lifecycle sagemaker-image-duplicate-create-idempotent sagemaker-image-version-lifecycle sagemaker-model-card-create-export-verify sagemaker-model-card-lifecycle-and-export-job sagemaker-model-package-group-policy-round-trip-byte-equality sagemaker-notebook-instance-error-paths sagemaker-training-plan-and-transform-job-errors sagemaker-transform-job-missing-model-error
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeModelPackageGroup|2xx->2xx|$.CreatedBy.IamIdentity.Arn
Issue-Key: sagemaker|DescribeModelPackageGroup|2xx->2xx|$.CreatedBy.IamIdentity.PrincipalId
Issue-Key: sagemaker|AddTags|2xx->2xx|$.Tags[*]
Issue-Key: sagemaker|DeleteAIRecommendationJob|2xx->4xx|$.AIRecommendationJobArn
Issue-Key: sagemaker|DeleteAIRecommendationJob|2xx->4xx|status
Issue-Key: sagemaker|DescribeHub|2xx->2xx|$.HubStatus
Issue-Key: sagemaker|DescribeHub|2xx->2xx|$.CreatedBy
Issue-Key: sagemaker|DescribeHub|2xx->2xx|$.LastModifiedBy
Issue-Key: sagemaker|CreateImage|2xx->4xx|$.ImageArn
Issue-Key: sagemaker|CreateImage|2xx->4xx|status
Issue-Key: sagemaker|DescribeModelCardExportJob|2xx->2xx|$.ExportArtifacts
Issue-Key: sagemaker|DescribeModelCardExportJob|2xx->2xx|$.Status
Issue-Key: sagemaker|GetModelPackageGroupPolicy|2xx->2xx|$.ResourcePolicy
Issue-Key: sagemaker|CreatePresignedNotebookInstanceUrl|4xx->4xx|$.message
Issue-Key: sagemaker|CreateTransformJob|4xx->4xx|$.__type
Issue-Key: sagemaker|CreateTransformJob|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker lifecycle, identity and round-trip fidelity defects across ten operations

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeModelPackageGroup

## Description
This issue bundles the remaining 10 sagemaker wire divergences: each is a distinct fidelity defect, grouped here because none of them belongs to the other bundles and each is small and local to one handler.

Cause 1 of 10: DescribeModelPackageGroup omits CreatedBy.IamIdentity.PrincipalId. AWS: the UserContext.IamIdentity carries both Arn and PrincipalId (Smithy UserContext -> IamIdentity). Emulator: PrincipalId absent. (The Arn itself differs only by the emulator's per-run generated principal name, which is not a requirement.) Why: clients that key audit records on PrincipalId see none. Fix: the shared caller-identity (UserContext) builder. Traces: api-model-package-group-lifecycle#task#3 (also sagemaker-model-package-group-delete-then-recreate#task#3 and #task#8).

Cause 2 of 10: AddTags response tag ordering. AWS: the returned Tags list is the resource's full tag set in AWS's order, with the newly added tag at the recorded position. Emulator: returns the list in a different order, so Tags[0] is the pre-existing tag where the recording has the added one. Fix: AddTags response projection ordering. Traces: api-resource-tagging-lifecycle#task#4.

Cause 3 of 10: DeleteAIRecommendationJob in teardown answers 400 where AWS answers 200 with AIRecommendationJobArn, because the emulator already destroyed the job on an earlier ungated delete. Fix: DeleteAIRecommendationJob state gate (the same gate as the delete/stop bundle) and its success response. Traces: sagemaker-ai-recommendation-job-workflow#teardown#7.

Cause 4 of 10: DescribeHub never leaves HubStatus 'Creating'. AWS: a created hub settles to 'InService'. Emulator: stays 'Creating', so a client polling for InService hangs. Fix: the hub status machine behind CreateHub/DescribeHub. Traces: sagemaker-hub-content-reference-lifecycle#task#3.

Cause 5 of 10: DescribeHub omits CreatedBy and LastModifiedBy (each an IamIdentity with Arn and PrincipalId) in addition to the stuck HubStatus of cause 4. Fix: DescribeHub output construction plus the hub status machine. Traces: sagemaker-hub-lifecycle#task#3.

Cause 6 of 10: CreateImage is not idempotent for an identical re-create. AWS: 200 returning the existing ImageArn. Emulator: 400. Why: a retried create (the normal SDK retry path) fails. Fix: CreateImage idempotency check. Traces: sagemaker-image-duplicate-create-idempotent#task#3 (also sagemaker-image-version-lifecycle#task#5).

Cause 7 of 10: DescribeModelCardExportJob reports a failed export. AWS: Status reaches 'Completed' and ExportArtifacts.S3ExportArtifacts holds the s3:// URI of the written PDF. Emulator: Status 'Failed' and no ExportArtifacts. Fix: the model-card export job runner and its output. Traces: sagemaker-model-card-create-export-verify#task#5 (also sagemaker-model-card-lifecycle-and-export-job#task#5).

Cause 8 of 10: GetModelPackageGroupPolicy does not return the policy byte-for-byte. AWS: ResourcePolicy is the exact document that PutModelPackageGroupPolicy was given (member order and spacing preserved). Emulator: re-serializes it - keys reordered (Statement before Version, Resource first inside the statement) and spaces inserted after ':'/','. Why: any caller that hashes or diffs the policy sees a change it did not make. Fix: store and return the raw policy text in the model-package-group policy state instead of a parsed/re-emitted document. Traces: sagemaker-model-package-group-policy-round-trip-byte-equality#task#3.

Cause 9 of 10: CreatePresignedNotebookInstanceUrl refuses for the wrong reason. AWS: the notebook instance does not exist (its create was refused), so the message is 'Notebook Instance <notebook-instance arn> does not exist'. Emulator: reports a status gate instead - 'Notebook instance "<arn>" is Pending, so it is serving no Jupyter server; it must be InService to create a presigned URL for it.' Fix: CreatePresignedNotebookInstanceUrl existence check ahead of the status gate (plus the CreateNotebookInstance validation filed separately). Traces: sagemaker-notebook-instance-error-paths#task#2.

Cause 10 of 10: CreateTransformJob for a missing model answers the wrong error class. AWS: __type 'ValidationException' with message 'Could not find model "<ModelName>".' (recorded behavior only - Smithy CreateTransformJob models ResourceInUse/ResourceLimitExceeded/ResourceNotFound, not ValidationException, so the recording is authoritative). Emulator: __type 'ResourceNotFound' and no message member at all. Why: a client branching on the error code takes the wrong branch and has no message to surface. Fix: CreateTransformJob model-resolution error construction. Traces: sagemaker-training-plan-and-transform-job-errors#task#2 (also sagemaker-transform-job-missing-model-error#task#1).

## Evidence
1: $.CreatedBy.IamIdentity.PrincipalId expected present, got None. 2: $.Tags[0] expected the newly added tag, got the pre-existing one. 3: expected 200 with AIRecommendationJobArn, got 400. 4: $.HubStatus expected 'InService', got 'Creating'. 5: $.CreatedBy and $.LastModifiedBy expected IamIdentity{Arn,PrincipalId}, got None; $.HubStatus expected 'InService', got 'Creating'. 6: expected 200 with ImageArn, got 400. 7: $.Status expected 'Completed', got 'Failed' (poll aborted on fail_on after 2 attempts); $.ExportArtifacts expected {'S3ExportArtifacts': 's3://<bucket>/<key>.pdf'}, got None. 8: $.ResourcePolicy expected the submitted compact document '{"Version":"2012-10-17","Statement":[{"Sid":...}]}', got a re-serialized form with reordered keys and added whitespace. 9: expected 'Notebook Instance <arn> does not exist', got 'Notebook instance "<arn>" is Pending, so it is serving no Jupyter server; ...'. 10: $.__type expected 'ValidationException', got 'ResourceNotFound'; $.message expected 'Could not find model "<ModelName>".', got None. Smithy: UserContext.IamIdentity; CreateTransformJob errors = ResourceInUse, ResourceLimitExceeded, ResourceNotFound.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-model-package-group-lifecycle step task#3. The other nine causes are the fixture#step bindings in trace_ids, each exercising the single operation named in its cause.
