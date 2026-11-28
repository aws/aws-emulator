Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter
Issue-Key: iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter|run#23|Timeout|time.sleep(2)
Issue-Commit: c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6

# iam__sagemaker__scheduler__sts: the minimum-privilege SageMaker pipeline-trigger flow reaches no verdict inside a client budget — a one-time Scheduler target that starts a pipeline never produces an observable execution

> _Note: this issue was identified against the code base at commit `c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: a one-time EventBridge Scheduler schedule whose Target names a SageMaker pipeline (Target.SageMakerPipelineParameters) never produces an observable pipeline execution, and the whole author->fire->observe flow never settles for the client: the check consumed its entire 60s client budget and timed out with no verdict instead of either the execution appearing or a refusal being returned.

NOTE: this is NOT the 501 refusal already filed as open issue 20261127T121249Z-iam-sagemaker-scheduler-sts-implement-sagemaker-listpipelineexecutions-the-operation-is-an-unimplemented.md (ListPipelineExecutions scaffold stub, whose stub issue 20261127T123223Z-sagemaker-listpipelineexecutions-stub.md is now marked fixed). The replay no longer fails on a NotImplemented refusal of ListPipelineExecutions — no error code and no non-2xx is reported at all. The call chain simply never converges, so that issue does not cover this failure; it is named here only so a fixer does not conflate the two.

AWS: Smithy: scheduler#Target.SageMakerPipelineParameters is 'The templated target type for the Amazon SageMaker StartPipelineExecution API operation', with PipelineParameterList @length 0..200 and each SageMakerPipelineParameter.Name (@length 1..256, pattern ^[A-Za-z0-9\-_]*$) and .Value (@length 1..1024) required. AWS behavior for this flow: CreateSchedule by a caller holding only the documented minimum trigger policy (scheduler:ListSchedules/GetSchedule/CreateSchedule/UpdateSchedule/DeleteSchedule plus iam:PassRole conditioned on iam:PassedToService=scheduler.amazonaws.com) is accepted; GetSchedule echoes the stored SageMakerPipelineParameters; when the at() expression elapses, Scheduler assumes the execution role and calls sagemaker:StartPipelineExecution on the named pipeline; sagemaker#ListPipelineExecutions then answers 200 with a PipelineExecutionSummaries entry for that one run, DescribePipelineExecution reports a terminal PipelineExecutionStatus, and ListPipelineParametersForExecution returns exactly the parameters the target passed. Every step in that chain is a bounded, promptly-answered call — none of them is open-ended, so a client polling it converges. Reference: https://docs.aws.amazon.com/sagemaker/latest/dg/pipeline-eventbridge.html

Emulator: the replay produced no response for the flow at all — 'Error: Check timed out after 60.0s'. No status, no error code and no error message were returned to the client; the check never reached any of its assertions (stored-target echo, execution-appeared, terminal status, parameter echo), so one of the calls in this chain (sts:AssumeRole for the minimum-privilege author session, scheduler CreateSchedule, the schedule's firing, or the sagemaker execution reads) does not answer or does not progress within a client's budget. In the same suite run the comparable ECS and FIS scheduler flows do return answers (they fail with assertions, not timeouts), so the non-convergence is specific to this pipeline-trigger chain rather than to schedule firing in general.

Why: a client cannot tell 'the schedule never fired', 'the pipeline never started' and 'the service did not answer' apart. Against real AWS every call here returns promptly and a waiter converges; against the emulator the SDK's call simply hangs until the caller's own deadline, so any automation that creates a SageMaker pipeline trigger and then polls for its run blocks forever rather than succeeding or receiving a modelled error. A hung call is also unretryable: there is no error code to branch on.

Fix: the schedule-firing/target-dispatch path for the SageMaker pipeline target type in the scheduler service (the component that assumes Target.RoleArn and issues sagemaker StartPipelineExecution with Target.SageMakerPipelineParameters), together with the sagemaker pipeline-execution state those reads serve (ListPipelineExecutions / DescribePipelineExecution / ListPipelineParametersForExecution). Each operation in the chain must answer promptly — 200 with an empty list before the trigger fires and the started run afterwards — rather than leaving the caller without a response.

Traces: iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter.

## Evidence
Expected (AWS): the minimum-privilege caller's CreateSchedule is accepted; GetSchedule echoes Target.SageMakerPipelineParameters = {PipelineParameterList: [{Name: <param>, Value: <label>}]}; after the at() time elapses ListPipelineExecutions(PipelineName=<pipeline>) answers HTTP 200 and lists exactly one PipelineExecutionArn; DescribePipelineExecution reports a terminal PipelineExecutionStatus; ListPipelineParametersForExecution returns exactly the one parameter the target passed. Every call answers promptly.

Actual (emulator), verbatim from the replay report for check iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter: 'Error: Check timed out after 60.0s'. No HTTP status, no error code and no error message were delivered to the client; none of the check's assertions was reached.

(The fixed/open ListPipelineExecutions scaffold issues are not this failure: the previously recorded refusal 'An error occurred (NotImplemented) when calling the ListPipelineExecutions operation' no longer appears — there is no refusal at all now.)

## Steps to Reproduce
1. iam CreateRole a pipeline execution role trusted by sagemaker.amazonaws.com, and sagemaker CreatePipeline with a definition whose single step is a Condition on a String parameter (DefaultValue 'unset').
2. iam CreateRole a target role trusted by scheduler.amazonaws.com with an inline policy allowing sagemaker:StartPipelineExecution on that pipeline ARN.
3. iam CreateRole a caller role trusted by the account root carrying ONLY the documented minimum trigger policy (scheduler:ListSchedules/GetSchedule/CreateSchedule/UpdateSchedule/DeleteSchedule on *, plus iam:PassRole on arn:aws:iam::*:role/* with StringLike iam:PassedToService=scheduler.amazonaws.com), then sts AssumeRole into it.
4. With those session credentials call scheduler CreateSchedule(Name=<name>, ScheduleExpression=at(<now+90s>), ScheduleExpressionTimezone=UTC, FlexibleTimeWindow={Mode: OFF}, Target={Arn: <pipeline ARN>, RoleArn: <target role>, SageMakerPipelineParameters: {PipelineParameterList: [{Name: RunLabel, Value: <label>}]}}), then scheduler GetSchedule and assert the target echo.
5. Poll sagemaker ListPipelineExecutions(PipelineName=<pipeline>) for the started run, then DescribePipelineExecution and ListPipelineParametersForExecution.
6. Expect every call to answer promptly (200, empty list before the fire time, the one run after) and the run to end Succeeded with parameters {RunLabel: <label>}; observe instead that the flow returns no response at all and the client's 60s budget is exhausted ('Check timed out after 60.0s').
