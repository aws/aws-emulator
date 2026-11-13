Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-workflow-state-change-to-sqs-positive
Issue-Key: glue__sqs.glue-workflow-state-change-to-sqs-positive|run#22|AssertionError|assert workflow_run["Status"] == "COMPLETED", (
Issue-Commit: a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c

# glue__sqs: a Glue workflow run whose only job run SUCCEEDED still settles terminal Status ERROR, so GetWorkflowRun never reports COMPLETED

> _Note: this issue was identified against the code base at commit `a8ab2e52bd77f613dd033c9fe2ca82b26e42a18c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowRun

## Description
One net-new root cause. Nothing before this point diverges: the Python-shell job is created, CreateWorkflow and an ON_DEMAND CreateTrigger succeed, StartTrigger starts the workflow, the workflow's job run actually runs and SUCCEEDS, and the resulting "Glue Job State Change" event with state SUCCEEDED is published and delivered to the SQS target through EventBridge. GetWorkflowRuns then lists the run and GetWorkflowRun answers 200 with a fully shaped Run — it just reports the wrong terminal status.

Cause 1 of 1: the workflow run's terminal Status is ERROR even though every action in the run succeeded.
AWS: Smithy models WorkflowRun.Status as the WorkflowRunStatus enum (RUNNING | COMPLETED | STOPPING | STOPPED | ERROR) and WorkflowRun.Statistics as WorkflowRunStatistics{TotalActions, SucceededActions, FailedActions, StoppedActions, ErroredActions ("the count of job runs in the ERROR state in the workflow run"), TimeoutActions, RunningActions, WaitingActions}. WorkflowRun.ErrorMessage is documented as describing "any error that may have occurred in starting the workflow run. Currently the only error message is 'Concurrent runs exceeded for workflow: foo.'" A terminal run's Status is therefore derived from its actions' outcomes: a run in which no action errored, failed, stopped or timed out settles COMPLETED, and ERROR is reserved for a run that actually errored. In this scenario the workflow holds exactly one JOB node whose job run ends SUCCEEDED (the delivered event's detail.state is SUCCEEDED), so AWS reports Status COMPLETED with SucceededActions == TotalActions, ErroredActions/FailedActions 0 and ErrorMessage unset.
Emulator: GetWorkflowRun(Name, RunId, IncludeGraph=true) answers 200 with Run.Status 'ERROR'. ERROR is terminal, so a client polling for a terminal status settles on ERROR rather than passing through it; no error cause accompanies it and the job run the run contains succeeded.
Why: Run.Status is the signal every orchestration client polls to decide whether a Glue workflow finished. A client waiting for COMPLETED never sees it and concludes the ETL workflow failed, then skips or re-runs downstream work even though the workflow's only job succeeded; branching on Statistics.ErroredActions/FailedActions finds no failure that explains the ERROR, and ErrorMessage carries nothing to act on. Any wait-and-branch loop (CLI, SDK, Step Functions, an EventBridge-driven pipeline) over a Glue workflow is unusable: the happy path is indistinguishable from a real failure.
Fix: the Glue workflow-run lifecycle/state behind StartTrigger / StartWorkflowRun / GetWorkflowRun / GetWorkflowRuns in services/service-com-amazonaws-glue-awsglue must derive a run's terminal Status from the outcomes of the actions in its graph instead of defaulting to ERROR — a run all of whose job runs reached SUCCEEDED must settle COMPLETED, with WorkflowRunStatistics counters consistent with that status and ErrorMessage left unset. The GetWorkflowRun serializer is otherwise correct (the Graph and its JobDetails.JobRuns are present), so this belongs in the run-state engine, not the response projection.
Related open issues that do NOT cover this check (the replay reports no owner for it): 20261110T094158Z-glue-workflow-run-settles-terminal-status-error-so.md is a WIRE finding keyed on glue|GetWorkflow|$.Workflow.LastRun.Status from the run-properties round-trip fixture and carries no replay key for this behavioral check; 20261109T085123Z-glue-s3-a-workflow-s-conditional-trigger-never.md is about a CONDITIONAL trigger never firing after its predicate job succeeds (a different trigger type and a different observation, filed under glue__s3); and 20261027T003436Z-glue-sqs-an-on-demand-trigger-that-starts.md describes an ON_DEMAND trigger initiating no job run at all and no event being published — which no longer reproduces here, since the job run executes and the SUCCEEDED event is delivered to SQS.
Traces: glue__sqs.glue-workflow-state-change-to-sqs-positive.

## Evidence
Check glue__sqs.glue-workflow-state-change-to-sqs-positive, verdict mismatch.
Reached without divergence: CreateJob (pythonshell) + CreateWorkflow + CreateTrigger(Type=ON_DEMAND, Actions=[{JobName}]) + StartTrigger all succeed; an EventBridge rule on {source: aws.glue, detail-type: "Glue Job State Change", detail.state: SUCCEEDED} delivers an event to SQS whose detail.state == 'SUCCEEDED' and whose detail.jobRunId names the workflow's job run; GetWorkflowRuns lists the workflow run.
Expected (AWS): GetWorkflowRun(Name=<workflow>, RunId=<run>, IncludeGraph=True) -> HTTP 200 with Run.Status == 'COMPLETED' (terminal), Statistics showing SucceededActions == TotalActions and ErroredActions == 0, ErrorMessage unset.
Actual (emulator): HTTP 200 with Run.Status == 'ERROR'. Failure detail:
  File "check_glue__sqs_glue_workflow_state_change_to_sqs_positive.py", line 86, in run
    assert workflow_run["Status"] == "COMPLETED", ...
  AssertionError: expected workflow run terminal status COMPLETED, got 'ERROR'
The poll ran to a terminal value, so 'ERROR' is the settled status and not an intermediate one. Status code and response shape are otherwise correct; the single diverging value is the WorkflowRunStatus enum member. Workflow/trigger/job names, run ids and timestamps in this run are per-run values and are not part of the requirement — the requirement is that a run with no errored, failed, stopped or timed-out action reports COMPLETED.

## Steps to Reproduce
1. glue CreateJob for a trivial pythonshell job (script in S3, a Glue-trusting role).
2. glue CreateWorkflow(Name=<wf>), then glue CreateTrigger(Name=<trig>, WorkflowName=<wf>, Type=ON_DEMAND, Actions=[{JobName: <job>}]).
3. events PutRule on {source:[aws.glue], detail-type:["Glue Job State Change"], detail:{jobName:[<job>], state:[SUCCEEDED]}} with an SQS queue target (queue policy allowing the rule).
4. glue StartTrigger(Name=<trig>) and receive from the queue until an event with detail.jobName == <job> arrives; observe detail.state == 'SUCCEEDED'.
5. glue GetWorkflowRuns(Name=<wf>) to obtain WorkflowRunId, then poll glue GetWorkflowRun(Name=<wf>, RunId=<id>, IncludeGraph=True) until Run.Status is one of COMPLETED/ERROR/STOPPED.
6. Observe terminal Run.Status 'ERROR' where AWS reports 'COMPLETED', with no errored/failed action in the run's graph.
Replay check glue__sqs.glue-workflow-state-change-to-sqs-positive.
