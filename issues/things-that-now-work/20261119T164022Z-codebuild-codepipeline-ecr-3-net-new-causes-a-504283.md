Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger
Issue-Key: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions|run#29|AssertionError|assert good_status == "Succeeded", f"positive control did not succeed: {good_status}"
Issue-Key: codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output|run#8|AssertionError|assert status == "Succeeded", f"pipeline execution ended with status={status}"
Issue-Key: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild|run#16|AssertionError|assert status == "Succeeded", f"pipeline execution ended with status={status}"
Issue-Key: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger|_assert_no_new_execution#12|Timeout|time.sleep(interval_s)
Issue-Commit: 1a6df7b32e4c8350e561c708bcd111a16150a036

# codebuild__codepipeline__ecr: 3 net-new causes — a pipeline's CodeBuild Build action ends the execution Failed for a docker build/push buildspec and for a plain file-copy buildspec, and a tag-matching ECR push never yields an observable execution

> _Note: this issue was identified against the code base at commit `1a6df7b32e4c8350e561c708bcd111a16150a036`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
Three distinct root causes in the CodePipeline -> CodeBuild -> ECR chain, clustered from the four failing behavioral checks listed under the `codebuild__codepipeline__ecr` heading that the replay report does NOT mark ALREADY FILED or OWNED. NOTE ON PRIOR ART: open issue 20261119T121756Z-codebuild-codepipeline-ecr-3-net-new-causes-a.md describes the same three causes, but the replay report still lists all four of these checks as unowned (it carries none of their replay keys), so they are re-filed here with their check ids attached; a fixer closing that issue should treat this one as the gated form of it and not as a separate defect.

Cause 1 of 3: a pipeline Build action whose CodeBuild project runs a privilegedMode docker build-and-push buildspec ends the pipeline execution Failed instead of Succeeded.
Failing checks: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions, codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output.
AWS: for a pipeline with an ECR Source action and a Build action (actionTypeId category=Build, owner=AWS, provider=CodeBuild, version=1) whose project has source.type=CODEPIPELINE, artifacts.type=CODEPIPELINE, environment.privilegedMode=true and a buildspec that starts dockerd, runs `aws ecr get-login-password | docker login`, `docker build` and `docker push` into a repository the build role is granted ecr:GetAuthorizationToken plus the push action set on, the build succeeds, the Build action execution status is Succeeded and GetPipelineExecution reports pipelineExecution.status Succeeded; the pushed tag is then visible via ECR ListImages/DescribeImages. Smithy (codepipeline 2015-07-09): GetPipelineExecution input pipelineName (required, length 1-100, pattern ^[A-Za-z0-9.@\-_]+$) and pipelineExecutionId (required, uuid pattern), output pipelineExecution; PipelineExecutionStatus = Cancelled|InProgress|Stopped|Stopping|Succeeded|Superseded|Failed; ActionExecutionStatus = InProgress|Abandoned|Succeeded|Failed; modeled errors are only PipelineExecutionNotFoundException / PipelineNotFoundException / ValidationException (all 400), none of which applies here. The success outcome itself is recorded behavior only (the recording shows a 200 GetPipelineExecution whose status is Succeeded).
Emulator: the execution settles at status `Failed` on the correctly-permitted pipeline. The negative twin inside the same check is correct — with the ECR grants removed the execution does end Failed, the Build action records an externalExecutionId, batch_get_builds reports buildStatus FAILED, and the build log carries the AccessDeniedException line whose fixed text is `An error occurred (AccessDeniedException) when calling the GetAuthorizationToken operation: User: arn:aws:sts::<account>:assumed-role/<role-name>/AWSCodeBuild-<build-uuid> is not authorized to perform: ecr:GetAuthorizationToken on resource: * because no identity-based policy allows the ecr:GetAuthorizationToken action` (the account id, role name and build uuid are values taken from the request and the emulator's own state, not literals). So only the authorized path diverges.
Why: a client cannot distinguish a genuine IAM defect from a correct configuration — the permission differential the whole scenario rests on collapses, and no image is delivered to the destination repository even when the role is fully authorized. Container-image CI pipelines can never be exercised green.
Fix: the CodeBuild build runtime the pipeline's Build action drives (privileged-mode container execution: docker daemon and docker CLI availability inside the build environment) and the Build-action result propagation that maps the build outcome onto the action and pipeline execution status. Related but NOT covering: 20261115T101606Z-codebuild-codepipeline-ecr-3-net-new-causes-privileged.md covers directly started privileged builds, not the pipeline-driven path.
Traces: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions (assertion `positive control did not succeed: Failed`), codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output (assertion `pipeline execution ended with status=Failed`).

Cause 2 of 3: a pipeline Build action with no docker involvement at all — a buildspec that only copies one file out of the source artifact — also ends the execution Failed, so the source-artifact handoff is unobservable.
Failing check: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild.
AWS: with an S3 Source action delivering a versioned source.zip containing input.txt and a CodeBuild project (source CODEPIPELINE, artifacts CODEPIPELINE) whose buildspec is `version: 0.2` / `phases: build: commands: - cp input.txt output.txt` / `artifacts: files: - output.txt`, CodePipeline delivers the source artifact into the build, the build succeeds, the execution status is Succeeded, and ListActionExecutions' Build/Build detail reports output.outputArtifacts containing `BuildOutput` with an s3location (bucket, key) whose stored zip holds output.txt byte-identical to the source input.txt. Smithy: ListActionExecutions output actionExecutionDetails with output.outputArtifacts; PipelineExecutionStatus as above. Success outcome is recorded behavior only.
Emulator: GetPipelineExecution settles this execution at status `Failed` as well, no output artifact is produced, and no BuildOutput s3location is readable. This is a cause distinct from Cause 1: the buildspec needs no docker daemon, only the input artifact and a shell copy, so the privileged-container explanation does not reach it.
Why: the fundamental CodePipeline-to-CodeBuild handoff (input artifact in, output artifact out) is unusable, and a client gets a Failed execution with no way to tell whether its buildspec, its role or the artifact store is at fault.
Fix: the Build-action handler's source-artifact delivery into the build and its output-artifact capture/upload, plus the mapping of the build result onto action and pipeline execution status in the codepipeline execution engine and the codebuild build runtime it calls.
Traces: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild (assertion `pipeline execution ended with status=Failed`).

Cause 3 of 3: an ECR image push whose tag matches an EventBridge rule's image-tag filter produces no observable pipeline execution, so the filter differential never resolves.
Failing check: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger.
AWS: an EventBridge rule on the `aws.ecr` `ECR Image Action` event filtered to a specific `image-tag`, targeting a pipeline ARN with an events.amazonaws.com role granting codepipeline:StartPipelineExecution, starts exactly one new pipeline execution for a push carrying that tag and none for a push of any other tag to the same repository; the admitted execution's source revision id equals the matching push's image digest (readable via ListActionExecutions / GetPipelineExecution). Recorded behavior only — EventBridge delivery and the ECR push event are not expressed in the Smithy model.
Emulator: the check never reaches its positive half; it is terminated with `Check timed out after 60.0s`, and nothing in the replay shows a matching-tag push admitting an execution. Caveat for the fixer: the check's own waits (a 60 s negative window plus a 150 s positive wait) exceed the 60 s budget it was replayed under, so the timeout alone does not prove the inclusion path is broken — re-replay this check and verify directly that a matching-tag push starts an execution whose source revision equals the pushed digest.
Why: a client wiring ECR pushes to a pipeline through a tag-filtered rule sees its pipeline never trigger, while the exclusion half passes trivially because nothing ever triggers — the emulator looks correct for the negative case and is unusable for the positive one.
Fix: the ECR push -> EventBridge event publication and the EventBridge target invocation that calls codepipeline StartPipelineExecution, plus the source-revision attribution recorded on the triggered execution. Related but NOT covering: 20261110T135206Z-codebuild-codepipeline-ecr-an-ecr-image-push-never-58ad0a.md covers the missing-target-permission differential, not this tag filter.
Traces: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger (`Check timed out after 60.0s`).

## Evidence
Replay of suite simparity-behavioral-aws-v1: 162/203 passed (79.8%), 39 mismatched, 2 errored. Service heading `codebuild__codepipeline__ecr`: 18/29 (62%), 11 failed, of which 7 are marked OWNED and 4 survive — all 4 are clustered into the 3 causes above.

Cause 1 — expected vs actual:
  expected: GetPipelineExecution(pipelineName=<pipe>, pipelineExecutionId=<id>).pipelineExecution.status == "Succeeded" for the correctly-permitted twin, with the destination ECR repository holding one image tagged with the project's IMAGE_TAG.
  actual:   status == "Failed". AssertionError `positive control did not succeed: Failed` (check_codebuild__codepipeline__ecr_cli_pipeline_execution_fails_when_codebuild_role_lacks_ecr_permissions.py line 86) and AssertionError `pipeline execution ended with status=Failed` (check_codebuild__codepipeline__ecr_cli_start_pipeline_execution_and_verify_codebuild_log_output.py line 43). The negative half of the first check passed in full: execution Failed, buildStatus FAILED, and the log line `An error occurred (AccessDeniedException) when calling the GetAuthorizationToken operation: User: arn:aws:sts::<account>:assumed-role/<role-name>/AWSCodeBuild-<build-uuid> is not authorized to perform: ecr:GetAuthorizationToken on resource: * because no identity-based policy allows the ecr:GetAuthorizationToken action` matched — so the divergence is confined to the authorized path.

Cause 2 — expected vs actual:
  expected: status == "Succeeded"; ListActionExecutions' Build/Build detail carries output.outputArtifacts[name=BuildOutput].s3location and the stored zip's output.txt equals the source marker string.
  actual:   status == "Failed". AssertionError `pipeline execution ended with status=Failed` (check_codebuild__codepipeline__ecr_pipeline_passes_source_artifact_location_to_codebuild.py line 56). The buildspec is only `cp input.txt output.txt` with artifacts files output.txt — no docker, no ECR.

Cause 3 — expected vs actual:
  expected: the matching-tag ECR push starts exactly one new pipeline execution whose source revision id equals that push's image digest; a mismatched-tag push starts none.
  actual:   `Check timed out after 60.0s`; no execution attributable to the matching push was observable, only the trivially-satisfied exclusion half was exercised.

Smithy grounding (codepipeline 2015-07-09, services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json): GetPipelineExecution input pipelineName (required, 1-100, ^[A-Za-z0-9.@\-_]+$) + pipelineExecutionId (required, uuid), output pipelineExecution; errors PipelineExecutionNotFoundException | PipelineNotFoundException | ValidationException (400); PipelineExecutionStatus = Cancelled|InProgress|Stopped|Stopping|Succeeded|Superseded|Failed; ActionExecutionStatus = InProgress|Abandoned|Succeeded|Failed.

## Steps to Reproduce
Cause 1:
1. Create two ECR repositories (src, dst); push a minimal image tagged `latest` to src; create a versioning-enabled artifact bucket.
2. Create a CodeBuild service role granting logs:*, s3:GetObject/GetObjectVersion/PutObject on the bucket and (positive control only) ecr:GetAuthorizationToken on * plus the ECR push action set on the dst repository ARN.
3. CreateProject with source {type: CODEPIPELINE, buildspec: start dockerd, then `aws ecr get-login-password | docker login`, `docker build`, `docker push "$REPO_URI:$IMAGE_TAG"`}, artifacts {type: CODEPIPELINE}, environment {LINUX_CONTAINER, aws/codebuild/standard:7.0, BUILD_GENERAL1_SMALL, privilegedMode: true, env REPO_URI=<dst uri>, IMAGE_TAG=pipeline-build}.
4. CreatePipeline with an ECR Source action (RepositoryName=src, ImageTag=latest) producing SourceArtifact and a CodeBuild Build action consuming it.
5. StartPipelineExecution, poll GetPipelineExecution. Observe status Failed on the correctly-permitted pipeline (AWS: Succeeded, tagged image present in dst).

Cause 2:
1. Create a versioned artifact bucket; upload source.zip containing input.txt with a marker string.
2. Create a CodeBuild role (logs + s3 on the bucket) and a project with source {type: CODEPIPELINE, buildspec `version: 0.2` / build command `cp input.txt output.txt` / artifacts files `output.txt`}, artifacts {type: CODEPIPELINE}.
3. CreatePipeline: S3 Source action on bucket/source.zip -> SourceArtifact; CodeBuild Build action with ProjectName=<project>, inputArtifacts SourceArtifact, outputArtifacts BuildOutput.
4. StartPipelineExecution and poll GetPipelineExecution. Observe status Failed and no usable BuildOutput s3location from ListActionExecutions (AWS: Succeeded, stored zip's output.txt equals the marker).

Cause 3:
1. Create an artifact bucket, an ECR repository, a CodePipeline service role, and a pipeline whose Source action is ECR with ImageTag=`prod-release`; let the initial execution settle and snapshot the existing execution ids.
2. Create an events.amazonaws.com role granting codepipeline:StartPipelineExecution on the pipeline ARN, and an EventBridge rule on the ECR push event for that repository filtered to image-tag `prod-release`, targeting the pipeline with that role.
3. Push an image tagged `dev-build`: no new execution should appear (this half holds).
4. Push an image tagged `prod-release`: expect exactly one new execution whose source revision id equals that push's image digest. Observe that no such execution becomes observable and the check exhausts its budget.
