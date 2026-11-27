Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation
Issue-Key: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger|_assert_no_new_execution#12|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation|_assert_no_new_execution#16|Timeout|time.sleep(interval_s)
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# codebuild__codepipeline__ecr: an ECR image push still never starts the pipeline its EventBridge rule targets, so neither the image-tag filter nor the target-permission differential can be graded

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: an EventBridge rule whose target is a CodePipeline pipeline ARN never starts a pipeline execution when a matching ECR image-push event occurs, even when the target role is trusted by events.amazonaws.com and grants codepipeline:StartPipelineExecution on that pipeline. Both surviving failing checks of this suite are grounded in this single missing delivery path (failing check ids: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger, codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation).

AWS: completing a layer upload and calling ECR PutImage emits an event whose envelope carries source 'aws.ecr', detail-type 'ECR Image Action' and a detail with action-type PUSH, repository-name <repository> and result SUCCESS (recorded behavior only for the envelope — the Smithy models do not describe event payloads). An ENABLED EventBridge rule matching that pattern, with a target whose Arn is the pipeline ARN and whose RoleArn is a role permitted to call codepipeline:StartPipelineExecution, causes EventBridge to start a NEW pipeline execution under that role. Smithy (eventbridge): PutTargets input Target$Id/Target$Arn/Target$RoleArn, output FailedEntryCount/FailedEntries — FailedEntryCount 0 means the target was accepted. Smithy (codepipeline): ListPipelineExecutions output pipelineExecutionSummaries (PipelineExecutionSummaryList), each PipelineExecutionSummary carrying pipelineExecutionId (pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$); StartPipelineExecution returns that pipelineExecutionId; GetPipelineExecution returns PipelineExecution whose artifactRevisions[].revisionId is the triggering ECR image digest. The rule's EventPattern is the only admission control: a detail with an 'image-tag' list admits ONLY pushes whose tag is in that list and filters every other tag out, and a target role lacking codepipeline:StartPipelineExecution starts nothing while an otherwise identical rule with a permitted role does start it. Both halves of each differential are therefore observable as the presence or absence of a new pipelineExecutionId.

Emulator: PutRule and PutTargets accept the rule with FailedEntryCount 0, and every ECR push (InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/PutImage) succeeds, but ListPipelineExecutions never reports any pipelineExecutionId outside the baseline for the targeted pipeline — not for a tag that matches the rule's image-tag filter, and not for the correctly-permissioned positive-control rule. The only execution ever listed is the one CreatePipeline auto-starts. The bounded poll for a new execution therefore never satisfies and both checks are killed at their 60 s budget with 'Check timed out after 60.0s'. No error code, HTTP status or message is involved: the invocation is silently absent on a path AWS completes with 2xx throughout.

Why: a client wiring ECR pushes to CodePipeline through EventBridge — the standard image-driven CI trigger — sees its pushes land, its rule and target accepted with no failed entry, and nothing ever run, with no error anywhere to diagnose. Worse, both security/filter differentials become vacuous: a rule whose image-tag filter should EXCLUDE a push looks correct only because nothing is ever delivered, and a target role missing StartPipelineExecution looks correctly blocked for the same reason, so the emulator cannot distinguish 'withheld by the pattern/permission' from 'never delivered at all' and an authorization or pattern-matching regression passes unnoticed.

Fix: the EventBridge delivery path for a CodePipeline target — event emission on ECR PutImage, rule EventPattern matching (including the detail.image-tag list), and the target invocation that calls StartPipelineExecution under the target role and authorizes against that role's policy — together with the CodePipeline execution plane that must register the resulting execution so ListPipelineExecutions/GetPipelineExecution report it with artifactRevisions[].revisionId set to the triggering image digest. Related issues that look similar but do not carry these checks' replay keys: open 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md; resolved 20261110T135206Z-codebuild-codepipeline-ecr-an-ecr-image-push-never-58ad0a.md and 20261102T124528Z-codebuild-codepipeline-ecr-an-ecr-image-push-never.md, both of which claimed this delivery path and left it still undelivered (the checks now exhaust their budget rather than being refused, so this is filed as a live divergence, not a refusal regression).

Traces: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger, codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation.

## Evidence
Cause 1 — both checks, verdict mismatch, error: "Check timed out after 60.0s".

codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger
Expected (AWS): with put_rule(EventPattern={"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"action-type":["PUSH"],"repository-name":[<repo>],"result":["SUCCESS"],"image-tag":["prod-release"]}}, State="ENABLED") and put_targets(Targets=[{Id:"1", Arn:<pipeline arn>, RoleArn:<role granting codepipeline:StartPipelineExecution>}]), a push tagged 'dev-build' produces NO new pipelineExecutionId for at least 60 s, and a subsequent push tagged 'prod-release' produces a new pipelineExecutionId whose GetPipelineExecution artifactRevisions[0].revisionId equals that push's image digest.
Actual (emulator): put_targets returns FailedEntryCount 0 and both PutImage calls succeed, but list_pipeline_executions never reports an id outside the baseline — the matching-tag push produces nothing, so the poll for the admitted execution exhausts the budget and the filter's positive half is never graded.

codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation
Expected (AWS): two rules on the same pipeline ARN, one with a role granting codepipeline:StartPipelineExecution and one granting only sqs:GetQueueAttributes. A push into the first rule's repository yields a new pipelineExecutionId (positive control); a push into the second rule's repository yields none for at least 90 s.
Actual (emulator): put_targets returns FailedEntryCount 0 for both rules and both pushes succeed, but no new pipelineExecutionId ever appears for either. The positive control's retried-push loop never terminates and the check is killed at its 60 s budget, so the permissioned-vs-unpermissioned differential yields no verdict.

In both cases every request involved returns 2xx; the divergence is an absent invocation (no new PipelineExecutionSummary), not a wrong status, error code or message. The only execution the emulator ever lists for the pipeline is the one CreatePipeline auto-starts.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the two checks above, or reproduce manually:
2. Create an S3 artifact bucket (versioned, tagged) and one or two ECR repositories.
3. Create an IAM role trusted by codepipeline.amazonaws.com with s3:PutObject/GetObject/GetObjectVersion on the bucket and ecr:DescribeImages; create a CodePipeline pipeline with a Source action (owner AWS, provider ECR, configuration RepositoryName=<repo>, ImageTag=<tag>) plus a Manual Approval stage. Note the pipeline ARN arn:aws:codepipeline:<region>:<account>:<name>.
4. Snapshot list_pipeline_executions(pipelineName=<name>) as the baseline (CreatePipeline auto-starts one execution).
5. Create IAM roles trusted by events.amazonaws.com: one granting codepipeline:StartPipelineExecution on the pipeline ARN, one granting only sqs:GetQueueAttributes.
6. events.put_rule with EventPattern {"source":["aws.ecr"],"detail-type":["ECR Image Action"],"detail":{"action-type":["PUSH"],"repository-name":[<repo>],"result":["SUCCESS"]}} (add "image-tag":["prod-release"] for the filter case), State=ENABLED, then events.put_targets(Rule=<rule>, Targets=[{Id:"1", Arn:<pipeline arn>, RoleArn:<role>}]) — expect FailedEntryCount 0.
7. Push an image into the repository via initiate_layer_upload / upload_layer_part / complete_layer_upload / put_image with a Docker v2 manifest and the matching tag.
8. Poll list_pipeline_executions for a pipelineExecutionId outside the baseline. On AWS a new execution appears (and GetPipelineExecution artifactRevisions[0].revisionId equals the pushed image digest); on the emulator none ever appears and the poll exhausts its budget.
9. For the filter case, push the same repository with a non-matching tag first and confirm no execution appears, then with the matching tag and confirm one does — on the emulator both halves are indistinguishable because neither delivers.
