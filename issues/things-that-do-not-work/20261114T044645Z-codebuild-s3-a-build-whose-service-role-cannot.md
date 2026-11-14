Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission
Issue-Key: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission|run#27|AssertionError|assert deny_build["buildStatus"] == "FAILED", (
Issue-Commit: 75f948824b47006d8e5270a6b1242dd4c041c19f

# codebuild__s3: a build whose service role cannot s3:PutObject into its artifact bucket reports UPLOAD_ARTIFACTS SUCCEEDED and buildStatus SUCCEEDED instead of failing

> _Note: this issue was identified against the code base at commit `75f948824b47006d8e5270a6b1242dd4c041c19f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One cause.

=== Cause 1 of 1: the UPLOAD_ARTIFACTS phase is not authorized against the build's service role, so an artifact upload that S3 must deny still succeeds ===
AWS: Smithy: Build.buildStatus (StatusType) is documented 'FAILED: The build failed' / 'SUCCEEDED: The build succeeded', and BuildPhase.phaseStatus carries the same enum per phase with BuildPhase.contexts giving 'Additional information about a build phase, especially to help troubleshoot a failed build'. CodeBuild uploads a project's S3 artifacts using the build's SERVICE ROLE: when that role's policy does not allow s3:PutObject on the artifact bucket, the upload is denied by S3, the UPLOAD_ARTIFACTS phase ends phaseStatus FAILED with a context describing the access denial, and the build ends buildStatus FAILED — even though every buildspec command exited zero. The differential is the only thing distinguishing a build that published its artifacts from one that could not; the model states the status enum, and the denial itself is recorded behavior of the role-less build.
Emulator: the build whose role lacks s3:PutObject on its artifact bucket ends buildStatus 'SUCCEEDED' with phases [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','SUCCEEDED'), ('DOWNLOAD_SOURCE','SUCCEEDED'), ('BUILD','SUCCEEDED'), ('UPLOAD_ARTIFACTS','SUCCEEDED'), ('FINALIZING','SUCCEEDED'), ('COMPLETED', None)] — UPLOAD_ARTIFACTS is reported SUCCEEDED, so the emulator performs (or fakes) the upload without evaluating the service role's S3 permissions at all. Replay: 'AssertionError: a build whose role cannot PutObject into its artifact bucket must FAIL, got 'SUCCEEDED''.
Why: a client relying on CodeBuild to surface a misconfigured service role gets a green build for a build that published nothing. A deploy or promotion gated on buildStatus SUCCEEDED proceeds against an artifact that does not exist in the bucket, and the permission defect is invisible until something downstream 404s. It also makes the emulator useless for validating least-privilege CodeBuild service roles, since the denied and the granted role are indistinguishable.
Why this is not covered by an existing issue: the open issues 20261029T204615Z-codebuild-s3-builds-fault-at-provisioning-so-declared.md and 20261109T181128Z-codebuild-s3-buildspec-commands-run-without-a-shell.md describe builds that FAULT at PROVISIONING or whose commands exit 127; here the build runs its commands and SUCCEEDS, and the divergence is the missing authorization of the artifact upload.
Fix: the UPLOAD_ARTIFACTS stage of the build-execution engine behind StartBuild (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs) must perform the artifact PutObject under the build's serviceRole identity so an S3 denial propagates: phaseStatus FAILED on UPLOAD_ARTIFACTS with a phase context naming the denial, and Build.buildStatus FAILED.
Traces: codebuild__s3.codebuild-artifact-upload-denied-without-s3-putobject-permission.

## Evidence
Expected: a build on a project with S3 artifacts whose service role grants only logs:* (no s3:PutObject on the artifact bucket) ends buildStatus FAILED with UPLOAD_ARTIFACTS phaseStatus FAILED, while the otherwise-identical build with a repaired role ends SUCCEEDED.
Actual: 'AssertionError: a build whose role cannot PutObject into its artifact bucket must FAIL, got 'SUCCEEDED'; phases=[('SUBMITTED', 'SUCCEEDED'), ('QUEUED', 'SUCCEEDED'), ('PROVISIONING', 'SUCCEEDED'), ('DOWNLOAD_SOURCE', 'SUCCEEDED'), ('BUILD', 'SUCCEEDED'), ('UPLOAD_ARTIFACTS', 'SUCCEEDED'), ('FINALIZING', 'SUCCEEDED'), ('COMPLETED', None)]' — the denied and the permitted arm are indistinguishable.

## Steps to Reproduce
1. Create an S3 artifact bucket. 2. Create two CodeBuild-assumable service roles: one granting only logs:CreateLogGroup/CreateLogStream/PutLogEvents, one additionally granting s3:PutObject on arn:aws:s3:::<bucket>/*. 3. CreateProject with artifacts {type: S3, location: <bucket>} and a buildspec that writes a file and declares it under `artifacts: files:`, using the DENIED role. 4. StartBuild and poll BatchGetBuilds until buildComplete. 5. Observe buildStatus and the UPLOAD_ARTIFACTS phaseStatus: AWS reports FAILED/FAILED (nothing in the bucket), the emulator reports SUCCEEDED/SUCCEEDED. 6. Repeat with the permitted role to confirm the two arms are indistinguishable in the emulator.
