Issue-Source: trace-sweep-divergence
Issue-Trace: api-model-package-group-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-model-package-group-lifecycle sagemaker-model-package-group-delete-then-recreate sagemaker-hub-content-reference-lifecycle sagemaker-hub-lifecycle sagemaker-model-card-create-export-verify
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeModelPackageGroup|2xx->2xx|$.CreatedBy.IamIdentity.Arn
Issue-Key: sagemaker|DescribeHub|2xx->2xx|$.CreatedBy
Issue-Key: sagemaker|DescribeHub|2xx->2xx|$.LastModifiedBy
Issue-Key: sagemaker|DescribeModelCardExportJob|2xx->2xx|$.ExportArtifacts.S3ExportArtifacts
Issue-Commit: 7461ab1bbe540afe8063ce03ee5477ebb649df48

# sagemaker resource-provenance and export-artifact response fields are synthesized instead of derived from the caller and the request (3 causes)

> _Note: this issue was identified against the code base at commit `7461ab1bbe540afe8063ce03ee5477ebb649df48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeModelPackageGroup

## Description
Three net-new wire divergences in the sagemaker emulator share one fault: a response field that AWS derives from the authenticated caller or from the request is instead synthesized by the emulator (invented, dropped, or composed with the wrong layout). All three steps answer 200, so a client sees a well-formed response carrying wrong provenance/location data. THREE causes must be closed.

Cause 1 of 3: DescribeModelPackageGroup reports a randomly generated IAM principal as the resource's creator (fixture api-model-package-group-lifecycle, step task#3).
AWS: Smithy (sagemaker) DescribeModelPackageGroup output member `CreatedBy` targets `UserContext` and is required; `UserContext.IamIdentity` targets `IamIdentity`, documented as "The IAM Identity details associated with the user. These details are associated with model package groups, model packages, and project entities only", and `UserContext` itself is "Information about the user who created or modified a SageMaker resource". So `CreatedBy.IamIdentity.Arn` is the ARN of the IAM identity whose credentials signed the CreateModelPackageGroup request. Its fixed text is the ARN template `arn:aws:iam::<account-id>:user/<user-name>`; the account id and user name are values taken from the request's SigV4 credential, not generated. The recording's caller signs as the user `emulator`, so the recorded response carries `arn:aws:iam::000000000000:user/emulator`.
Emulator: answers `arn:aws:iam::000000000000:user/EMULATORFCB726C07596` — the principal name is an invented `EMULATOR<hex>` token with no relation to the signing credential. The same cluster covers sagemaker-model-package-group-delete-then-recreate task#3 and task#8, and the neighbouring DescribeModelPackage clusters show different invented names (`EMULATORB7D131F9EBD1`, `EMULATOR2D127C42EB9D`), confirming the name is generated per resource rather than read from the caller.
Why: clients and provenance/audit tooling attribute or filter registry entries by comparing `CreatedBy.IamIdentity.Arn` with their own identity (sts:GetCallerIdentity) or with a known role/user ARN. A generated ARN never matches any real principal, and two groups created by the same principal report different creators, so attribution, ownership filters and least-privilege checks built on CreatedBy all mis-classify.
Fix: the sagemaker crate's UserContext construction — populate IamIdentity from the request's authenticated SigV4 principal when CreateModelPackageGroup records the resource (and when the Describe handler renders it), instead of generating a placeholder name.
Traces: api-model-package-group-lifecycle#task#3 (representative); also sagemaker-model-package-group-delete-then-recreate#task#3, sagemaker-model-package-group-delete-then-recreate#task#8.

Cause 2 of 3: DescribeHub omits CreatedBy and LastModifiedBy entirely (fixture sagemaker-hub-content-reference-lifecycle, step task#3).
AWS: recorded behavior only — the on-disk Smithy model's DescribeHubResponse lists HubName, HubArn, HubDisplayName, HubDescription, HubSearchKeywords, S3StorageConfig, HubStatus, FailureReason, CreationTime and LastModifiedTime, and does not model CreatedBy/LastModifiedBy. The recording shows AWS additionally returning both members as UserContext structures, each with an `IamIdentity` holding `Arn` and `PrincipalId` (the recording asserts their shape, not their values), for a hub created earlier in the same fixture.
Emulator: both members are absent from the DescribeHub body (expected an object, got None), for both the hub-content-reference and the plain hub lifecycle fixtures.
Why: a client reading hub provenance gets nothing: code that dereferences `CreatedBy.IamIdentity.Arn` sees a null/absent field and either fails or reports the hub as having no creator, and "who last changed this hub" is unanswerable against the emulator while it is answerable against AWS.
Fix: the DescribeHub handler/serializer and the hub record in the sagemaker crate's state — record the creating and last-modifying principal at CreateHub/UpdateHub and render both UserContext members. The open issue 20261102T105242Z-sagemaker-describe-responses-omit-members-present-in-the.md covers omitted members on other Describe operations (e.g. DescribeFeatureGroup) and does NOT own this DescribeHub cluster, which the replay reports unowned.
Traces: sagemaker-hub-content-reference-lifecycle#task#3 (representative); also sagemaker-hub-lifecycle#task#3.

Cause 3 of 3: the model-card export artifact S3 key flattens the per-card folder and injects the card version (fixture sagemaker-model-card-create-export-verify, step task#5).
AWS: Smithy (sagemaker) DescribeModelCardExportJob output member `ExportArtifacts` targets `ModelCardExportArtifacts`, whose required member `S3ExportArtifacts` is an `S3Uri` (@length 0..1024, @pattern ^(https|s3)://([^/]+)/?(.*)$) documented as "The Amazon S3 URI of the exported model artifacts"; the key layout itself is recorded behavior only. The recording shows AWS writing the PDF under the job's `OutputConfig.S3OutputPath` bucket at `model-card/export/<ModelCardName>/<ModelCardExportJobName>.pdf` — i.e. the model-card name is its own path segment and the export-job name is the file stem. Fixed text: the `model-card/export/` prefix and the `.pdf` suffix; variable parts: the bucket from OutputConfig.S3OutputPath, the model card name and the export job name from the request.
Emulator: joins those two names into a single segment and inserts the model card version, producing `model-card/export/<ModelCardName>-<ModelCardVersion>-<ModelCardExportJobName>.pdf`. In the recorded step the expected URI ends `.../export/<card>/<job>.pdf` while the emulator answered `.../export/<card>-1-<job>.pdf`.
Why: clients retrieve the exported card by using this URI, and tooling lists or lifecycles the per-card folder by the `model-card/export/<card>/` prefix. With the folder flattened, a prefix listing of a card's export folder returns nothing, bucket policies/lifecycle rules scoped to that prefix do not apply, and any client that re-derives the key from the card and job names points at a nonexistent object.
Fix: the model-card export job handler in the sagemaker crate, where the artifact key for S3ExportArtifacts is composed (and the PDF object is written) — use the `<card>/<job>.pdf` layout and drop the version from the key.
Traces: sagemaker-model-card-create-export-verify#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 497 steps, score 79.3%). Three unowned, non-stub divergence clusters:

1) sagemaker DescribeModelPackageGroup, api-model-package-group-lifecycle#task#3 — status 200 expected, 200 got; $.CreatedBy.IamIdentity.Arn: value_mismatch (expected 'arn:aws:iam::000000000000:user/emulator', got 'arn:aws:iam::000000000000:user/EMULATORFCB726C07596'). affected_count 3: also sagemaker-model-package-group-delete-then-recreate#task#3, sagemaker-model-package-group-delete-then-recreate#task#8.

2) sagemaker DescribeHub, sagemaker-hub-content-reference-lifecycle#task#3 — status 200 expected, 200 got; $.CreatedBy: missing (expected {'IamIdentity': {'Arn': '<shaped-not-asserted>', 'PrincipalId': '<shaped-not-asserted>'}}, got None); $.LastModifiedBy: missing (same shape, got None). affected_count 2: also sagemaker-hub-lifecycle#task#3.

3) sagemaker DescribeModelCardExportJob, sagemaker-model-card-create-export-verify#task#5 — status 200 expected, 200 got; $.ExportArtifacts.S3ExportArtifacts: value_mismatch (expected 's3://<bucket>/model-card/export/sim2real-mc-5399cea1-4322-4fdc-85ce-b2a7976f49ce/sim2real-probe-sagemaker-mc-export-79d25042.pdf', got 's3://<bucket>/model-card/export/sim2real-mc-5399cea1-4322-4fdc-85ce-b2a7976f49ce-1-sim2real-probe-sagemaker-mc-export-79d25042.pdf') — one segment instead of two, with the model card version '1' injected.

Model evidence: DescribeModelPackageGroup.CreatedBy -> UserContext (required), UserContext.IamIdentity -> IamIdentity; DescribeModelCardExportJob.ExportArtifacts -> ModelCardExportArtifacts.S3ExportArtifacts (S3Uri, required); DescribeHubResponse models neither CreatedBy nor LastModifiedBy (recorded behavior only).

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.

Cause 1: replay fixture api-model-package-group-lifecycle, step task#3 (CreateModelPackageGroup with signed credentials for user 'emulator', then DescribeModelPackageGroup) and read $.CreatedBy.IamIdentity.Arn — it is an invented arn:aws:iam::<account>:user/EMULATOR<hex> rather than the signing principal's ARN. Same via sagemaker-model-package-group-delete-then-recreate#task#3 / #task#8.

Cause 2: replay fixture sagemaker-hub-content-reference-lifecycle, step task#3 (CreateHub then DescribeHub) and look for $.CreatedBy / $.LastModifiedBy — both are absent. Same via sagemaker-hub-lifecycle#task#3.

Cause 3: replay fixture sagemaker-model-card-create-export-verify, step task#5 (CreateModelCard, CreateModelCardExportJob with OutputConfig.S3OutputPath, then DescribeModelCardExportJob) and read $.ExportArtifacts.S3ExportArtifacts — the key is model-card/export/<card>-<version>-<job>.pdf instead of model-card/export/<card>/<job>.pdf.
