Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation
Issue-Key: codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation|_assert_no_new_execution#16|Timeout|time.sleep(interval_s)
Issue-Commit: 51ca4c2291cddeccaa5c76d2f3d0dac1588f79fd

# codebuild__codepipeline__ecr: an ECR image push never starts the pipeline its EventBridge rule targets, so the missing-target-permission differential is ungradeable

> _Note: this issue was identified against the code base at commit `51ca4c2291cddeccaa5c76d2f3d0dac1588f79fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: an EventBridge rule whose target is a CodePipeline pipeline ARN never starts an execution when a matching ECR push event occurs, even when the target role grants codepipeline:StartPipelineExecution.
AWS: pushing an image through the ECR data plane (`PutImage` after the layer uploads) emits an event with `source` aws.ecr, `detail-type` "ECR Image Action" and a detail carrying `action-type` PUSH, `repository-name` <repo> and `result` SUCCESS. An ENABLED rule matching that pattern, with a target whose `Arn` is the pipeline ARN and whose `RoleArn` is a role trusted by events.amazonaws.com and granted codepipeline:StartPipelineExecution on that pipeline, causes EventBridge to start a new pipeline execution, which `ListPipelineExecutions` then reports as a `pipelineExecutionSummary` id outside the baseline. The negative half of the same contract is that a rule whose target role grants only an unrelated action (here sqs:GetQueueAttributes) starts nothing, so the two rules differ only in the role's permission. Recorded behavior only for the event envelope; the model states the wire shapes (`PutTargets` -> `Target$Arn`/`Target$RoleArn`, `FailedEntryCount` 0 on acceptance; codepipeline `ListPipelineExecutions` -> `PipelineExecutionSummary$pipelineExecutionId`).
Emulator: `PutTargets` accepts both rules with FailedEntryCount 0, and the ECR pushes succeed, but no new pipeline execution is ever produced for either rule. `ListPipelineExecutions` keeps reporting only the execution `CreatePipeline` auto-starts (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/create_pipeline.rs lines 296-315 start that one), so the positive control spins through its retried-push loop with no new id and the check is killed at its 60 s budget before the negative assertion is ever reached. Because the permitted rule never fires either, the emulator cannot distinguish "withheld for lack of permission" from "never delivered at all".
Why: a client that wires ECR pushes to CodePipeline through EventBridge — the standard image-driven CI trigger — sees pushes land and nothing run, with no failed entry and no error to diagnose. Worse for a security test: a rule whose role lacks StartPipelineExecution looks correctly blocked purely because nothing is ever delivered, so an emulator run cannot tell a permission boundary from a dead event path, and an authorization regression would pass unnoticed.
Fix: the EventBridge delivery path for a CodePipeline target (event emission on ECR PutImage, rule matching, and the target invocation that calls StartPipelineExecution under the target role, authorizing against that role's policy), plus the codepipeline execution plane that must register the resulting execution. Related open issues that look similar but do not carry this check's replay key: 20261102T124528Z-codebuild-codepipeline-ecr-an-ecr-image-push-never.md, 20261029T174654Z-codebuild-codepipeline-ecr-build-actions-are-never-executed.md and 20261029T064353Z-codepipeline-events-sqs-codepipeline-publishes-no-pipeline-stage-execution-state-change.md.
Traces: codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation.

## Evidence
Check codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation, verdict mismatch, error: "Check timed out after 60.0s".

Expected (AWS): after put_rule(EventPattern={"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"action-type":["PUSH"],"repository-name":[<repo>],"result":["SUCCESS"]}}, State=ENABLED) and put_targets(Targets=[{Id:"1", Arn:<pipeline arn>, RoleArn:<role with codepipeline:StartPipelineExecution>}]), an ECR PutImage into <repo> yields a new pipelineExecutionId from list_pipeline_executions outside the baseline set; the same push into the repository whose rule's role grants only sqs:GetQueueAttributes yields no new execution for at least 90 s.

Actual (emulator): put_targets returns FailedEntryCount 0 for both rules and PutImage succeeds for both repositories, but list_pipeline_executions never reports an id outside the baseline for either. The positive control's retried-push loop therefore never terminates and the check is killed at its 60 s budget, so the permissioned-vs-unpermissioned differential produces no verdict. The only execution the emulator ever lists for the pipeline is the one CreatePipeline auto-starts.

No error code or message is involved: the divergence is a silently absent invocation (no new PipelineExecutionSummary) on a delivery path AWS completes.

## Steps to Reproduce
1. Create an S3 artifact bucket and two ECR repositories (repo_good, repo_bad).
2. Create a CodePipeline pipeline (Source action provider ECR on repo_good with ImageTag=latest, plus a Manual Approval stage) with a role trusted by codepipeline.amazonaws.com; note its ARN and snapshot list_pipeline_executions as the baseline.
3. Create two IAM roles trusted by events.amazonaws.com: eb-role-good with codepipeline:StartPipelineExecution on the pipeline ARN, eb-role-noperm with only sqs:GetQueueAttributes.
4. events.put_rule + events.put_targets for repo_good -> pipeline ARN with eb-role-good, and for repo_bad -> pipeline ARN with eb-role-noperm (both accepted, FailedEntryCount 0).
5. Push an image into repo_good via initiate_layer_upload/upload_layer_part/complete_layer_upload/put_image, then poll list_pipeline_executions for an id outside the baseline. On AWS a new execution appears; on the emulator none ever does and the check exhausts its budget here.
6. Push into repo_bad and confirm no new execution appears — on the emulator this passes vacuously because step 5 never delivered either.
