Issue-Severity: HIGH
Issue-Commit: 93847454bd4acf14d3c65fb9240f255701f52b4f

# codebuild__codepipeline__ecr: 3 net-new causes — a pipeline's CodeBuild Build action ends the execution Failed for both a docker build/push buildspec and a plain file-copy buildspec, and an ECR push matching an EventBridge image-tag filter never yields an observable execution

> _Note: this issue was identified against the code base at commit `93847454bd4acf14d3c65fb9240f255701f52b4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
Three distinct net-new root causes in the CodePipeline -> CodeBuild -> ECR chain. Each is a case where the pipeline's Build stage (actionTypeId category=Build, owner=AWS, provider=CodeBuild, version=1) does not reach the outcome AWS produces, so the whole execution diverges. All three were clustered from failing behavioral checks listed under the `codebuild__codepipeline__ecr` heading; none is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 3: a pipeline Build action whose CodeBuild project runs a privilegedMode docker build-and-push buildspec ends the execution Failed instead of Succeeded.
Failing checks: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions, codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output.
AWS: for a pipeline with an ECR Source action and a CodeBuild Build action whose project has source.type=CODEPIPELINE, artifacts.type=CODEPIPELINE, environment.privilegedMode=true and a buildspec that (a) starts dockerd, (b) `aws ecr get-login-password | docker login`, (c) `docker build`, (d) `docker push` to a repository the build role is granted ecr:GetAuthorizationToken plus the push action set on, the build succeeds, the Build action execution status is Succeeded and the pipeline execution status is Succeeded. Smithy (codepipeline): PipelineExecutionStatus enum = Cancelled|InProgress|Stopped|Stopping|Succeeded|Superseded|Failed; ActionExecutionStatus enum = InProgress|Abandoned|Succeeded|Failed; GetPipelineExecution returns pipelineExecution.status. The success path itself is recorded behavior only (no modeled error applies: the recording shows a 200 GetPipelineExecution whose status is Succeeded).
Emulator: GetPipelineExecution settles the execution at status `Failed`. The negative twin in the same check is correct — with the ECR grants removed the execution does end Failed, the Build action records an externalExecutionId, the CodeBuild run reports buildStatus FAILED and the build log carries exactly `An error occurred (AccessDeniedException) when calling the GetAuthorizationToken operation: User: arn:aws:sts::<account>:assumed-role/<role-name>/AWSCodeBuild-<build-uuid> is not authorized to perform: ecr:GetAuthorizationToken on resource: * because no identity-based policy allows the ecr:GetAuthorizationToken action` (fixed text as quoted; account, role-name and build-uuid are taken from the request/state, never literal). Only the correctly-permitted positive control diverges: it fails too, so permission has no observable effect on the outcome.
Why: a client cannot distinguish a genuine permission defect from a working configuration — the differential that the whole permission-boundary scenario rests on collapses, and no image is ever delivered to the destination repository even when the role is fully authorized. CI pipelines that build and push container images can never be exercised green against the emulator.
Fix: the CodeBuild build runtime that the pipeline's Build action drives — the container/privileged-mode execution path for a buildspec (docker daemon availability and the docker CLI inside the build environment), and the Build-action result propagation that maps the build outcome onto the action and pipeline execution status. Related but NOT covering: open issue 20261115T101606Z-codebuild-codepipeline-ecr-3-net-new-causes-privileged.md describes privileged builds having no docker daemon for directly started builds; these two checks are the pipeline-driven path and are not carried by it.
Traces: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions (assertion: `positive control did not succeed: Failed`), codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output (assertion: `pipeline execution ended with status=Failed`).

Cause 2 of 3: a pipeline Build action with no docker involvement at all — a buildspec that only copies one file out of the source artifact — also ends the execution Failed.
Failing check: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild.
AWS: with an S3 source action delivering a versioned source.zip containing input.txt, and a CodeBuild project (source CODEPIPELINE, artifacts CODEPIPELINE) whose buildspec is `version: 0.2` / `phases: build: commands: - cp input.txt output.txt` / `artifacts: files: - output.txt`, CodePipeline delivers the source artifact into the build, the build succeeds, the Build action's output artifact `BuildOutput` is stored in the artifact bucket, and the pipeline execution status is Succeeded; ListActionExecutions reports the action's output.outputArtifacts with an s3location (bucket,key) whose zip contains output.txt byte-identical to the source input.txt. Smithy (codepipeline): ListActionExecutions returns actionExecutionDetails with output.outputArtifacts; PipelineExecutionStatus as above. Recorded behavior only for the success outcome.
Emulator: GetPipelineExecution settles this execution at status `Failed` as well, so the output artifact is never produced and the source-artifact-delivery contract cannot be observed. This is a separate cause from Cause 1: the buildspec needs no docker daemon, only the source artifact and a shell copy, so the build-container/docker explanation does not reach it.
Why: the fundamental CodePipeline-to-CodeBuild handoff (input artifact in, output artifact out) is unusable. A client that expects its build stage to publish an output artifact for a downstream stage gets a Failed execution and no artifact, with no way to tell whether its buildspec, its role or the artifact store is at fault.
Fix: the Build-action handler's source-artifact delivery into the build and its output-artifact capture/upload, plus whatever maps the build result onto the action and pipeline execution status in the codepipeline execution engine (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops + src/state.rs) and the codebuild build runtime it calls.
Traces: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild (assertion: `pipeline execution ended with status=Failed`).

Cause 3 of 3: an ECR image push whose tag matches an EventBridge rule's image-tag filter produces no observable pipeline execution, so the filter differential never resolves.
Failing check: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger.
AWS: an EventBridge rule on the `aws.ecr` `ECR Image Action` event filtered to a specific `image-tag`, targeting a pipeline ARN with an events.amazonaws.com role granting codepipeline:StartPipelineExecution, starts exactly one new pipeline execution for a push carrying that tag and starts none for a push of any other tag to the same repository; the admitted execution's source revision id equals the matching push's image digest (readable via ListActionExecutions / GetPipelineExecution). Recorded behavior only — EventBridge delivery and the ECR push event are not expressed in the Smithy model.
Emulator: the check never reaches its positive half — it is terminated with `Check timed out after 60.0s`, i.e. no execution attributable to the matching-tag push becomes observable inside the harness budget while the whole scenario (initial execution snapshot, mismatched-tag negative window, matching-tag push) is driven. Nothing in the replay shows the matching push admitting an execution.
Why: a client wiring ECR pushes to a pipeline through a tag-filtered EventBridge rule sees its pipeline never trigger; and because the negative half (wrong tag -> no execution) passes trivially when nothing ever triggers, the emulator looks correct for the exclusion case while being unusable for the inclusion case.
Fix: the ECR push -> EventBridge event publication and the EventBridge target invocation that calls codepipeline StartPipelineExecution, plus the source-revision attribution recorded on the triggered execution. Related but NOT covering: open issue 20261110T135206Z-codebuild-codepipeline-ecr-an-ecr-image-push-never-58ad0a.md covers the missing-target-permission differential; this tag-filter check is not carried by it. Note for the fixer: the check's own waits (a 60 s negative window plus a 150 s positive wait) exceed the 60 s budget it was replayed under, so confirm the fix by re-replaying this check and checking that a matching-tag push does start an execution whose source revision equals the pushed digest.
Traces: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger (Check timed out after 60.0s).

## Evidence
Replay of suite simparity-behavioral-aws-v1 (201/218 passed). Service heading `codebuild__codepipeline__ecr`: 18/29 (62%), 11 failed, of which 7 are marked OWNED/ALREADY FILED and 4 survive.

Cause 1 — expected vs actual:
  expected: GetPipelineExecution(pipelineName=<pipe>, pipelineExecutionId=<id>).pipelineExecution.status == "Succeeded" for the correctly-permitted twin; ECR DescribeImages on the destination repo shows exactly one image tagged with the project's IMAGE_TAG.
  actual:   status == "Failed"; assertion `positive control did not succeed: Failed` (check_..._cli_pipeline_execution_fails_when_codebuild_role_lacks_ecr_permissions.py line 86) and `pipeline execution ended with status=Failed` (check_..._cli_start_pipeline_execution_and_verify_codebuild_log_output.py line 43). In the first check the NEGATIVE half passed: execution Failed, buildStatus FAILED, and the build log line `An error occurred (AccessDeniedException) when calling the GetAuthorizationToken operation: User: arn:aws:sts::<account>:assumed-role/<role>/AWSCodeBuild-<uuid> is not authorized to perform: ecr:GetAuthorizationToken on resource: * because no identity-based policy allows the ecr:GetAuthorizationToken action` matched exactly — so the divergence is confined to the authorized path.

Cause 2 — expected vs actual:
  expected: status == "Succeeded"; ListActionExecutions' Build/Build detail carries output.outputArtifacts[name=BuildOutput].s3location, and the stored zip's output.txt equals the source input.txt content.
  actual:   status == "Failed"; assertion `pipeline execution ended with status=Failed` (check_..._pipeline_passes_source_artifact_location_to_codebuild.py line 56). Buildspec is `cp input.txt output.txt` with artifacts files: output.txt — no docker, no ECR.

Cause 3 — expected vs actual:
  expected: the matching-tag ECR push starts exactly one new pipeline execution whose source revision id equals that push's image digest; a mismatched-tag push starts none.
  actual:   `Check timed out after 60.0s` — no execution attributable to the matching push was observable; only the trivially-satisfied exclusion half was exercised.

Smithy grounding (codepipeline 2015-07-09): GetPipelineExecution input pipelineName (required, 1-100, ^[A-Za-z0-9.@\-_]+$), pipelineExecutionId (required, uuid pattern), output pipelineExecution; PipelineExecutionStatus = Cancelled|InProgress|Stopped|Stopping|Succeeded|Superseded|Failed; ActionExecutionStatus = InProgress|Abandoned|Succeeded|Failed; ListActionExecutions output actionExecutionDetails.

## Steps to Reproduce
Cause 1:
1. Create two ECR repositories (src, dst) and push a minimal image tagged `latest` to src; create a versioning-capable artifact bucket.
2. Create a CodeBuild service role granting logs:*, s3:GetObject/GetObjectVersion/PutObject on the bucket, and (positive control only) ecr:GetAuthorizationToken on * plus the ECR push action set on the dst repo ARN.
3. CreateProject with source {type: CODEPIPELINE, buildspec: starts `dockerd`, then `aws ecr get-login-password | docker login`, `docker build`, `docker push "$REPO_URI:$IMAGE_TAG"`}, artifacts {type: CODEPIPELINE}, environment {LINUX_CONTAINER, aws/codebuild/standard:7.0, BUILD_GENERAL1_SMALL, privilegedMode: true, env REPO_URI=<dst uri>, IMAGE_TAG=pipeline-build}.
4. CreatePipeline with an ECR Source action (RepositoryName=src, ImageTag=latest) producing SourceArtifact and a CodeBuild Build action consuming it.
5. StartPipelineExecution, then poll GetPipelineExecution. Observe status Failed on the correctly-permitted pipeline (AWS: Succeeded, with the tagged image present in dst).

Cause 2:
1. Create a versioned artifact bucket; upload source.zip containing input.txt with a marker string.
2. Create a CodeBuild role (logs + s3 on the bucket) and a project with source {type: CODEPIPELINE, buildspec: `version: 0.2` / build command `cp input.txt output.txt` / artifacts files `output.txt`}, artifacts {type: CODEPIPELINE}.
3. CreatePipeline: S3 Source action on bucket/source.zip -> SourceArtifact; CodeBuild Build action with ProjectName=<project>, inputArtifacts SourceArtifact, outputArtifacts BuildOutput.
4. StartPipelineExecution and poll GetPipelineExecution. Observe status Failed; ListActionExecutions yields no usable BuildOutput s3location (AWS: Succeeded, and the stored zip's output.txt equals the marker).

Cause 3:
1. Create an artifact bucket, an ECR repository, a CodePipeline service role and a pipeline whose Source action is ECR with ImageTag=`prod-release`; let the initial execution settle and snapshot the existing execution ids.
2. Create an events.amazonaws.com role granting codepipeline:StartPipelineExecution on the pipeline ARN, and an EventBridge rule on the ECR push event for that repository filtered to image-tag `prod-release`, targeting the pipeline with that role.
3. Push an image tagged `dev-build`: no new execution should appear (this half holds).
4. Push an image tagged `prod-release`: expect exactly one new execution whose source revision id equals that push's image digest. Observe that no such execution ever becomes observable and the check exhausts its budget.
