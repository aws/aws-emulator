Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds
Issue-Key: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds|run#35|AssertionError|assert statistics.get("SucceededActions") == 2, (
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# glue__s3: a CONDITIONAL workflow trigger never fires on an upstream job's SUCCEEDED state, so a workflow run completes with only its entry action

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowRun

## Description
Cause 1 of 1: a workflow whose entry ON_DEMAND trigger runs a producer job and whose CONDITIONAL trigger (Predicate {Logical: ANY, Conditions: [{LogicalOperator: EQUALS, JobName: <producer>, State: SUCCEEDED}]}) should then run a consumer job completes after the producer alone: the gate trigger never starts its action.

AWS: once the producer job run inside the workflow reaches SUCCEEDED, the CONDITIONAL trigger watching that job/state starts its Actions, so the run accumulates two actions — GetWorkflowRun reports Statistics with TotalActions 2 and SucceededActions 2, the run's Graph carries a JOB node for each job with one SUCCEEDED JobRun each, and the consumer's StartedOn is at or after the producer's CompletedOn (its output is derived from the producer's object, so it cannot run earlier). Smithy: glue#CreateTrigger accepts Type CONDITIONAL with Predicate{Logical, Conditions[{LogicalOperator, JobName, State}]} and StartOnCreation; glue#GetWorkflowRun returns WorkflowRun{Status, Statistics{TotalActions, SucceededActions, FailedActions, RunningActions, ...}, Graph{Nodes[]}}.
Emulator: the workflow run reaches a terminal Status with Statistics {'TotalActions': 1, 'TimeoutActions': 0, 'FailedActions': 0, 'StoppedActions': 0, 'SucceededActions': 1, 'RunningActions': 0, 'ErroredActions': 0, 'WaitingActions': 0} — the gate trigger contributed no action, so the consumer job never ran and its object is never written.
Why: a client that expresses an ETL dependency chain as Glue triggers gets only the first stage; the workflow reports a healthy, completed run, so a silently truncated pipeline is indistinguishable from a successful one, and anything consuming the second stage's output waits forever on data that was never produced.
Fix: the Glue workflow engine's trigger evaluation — CONDITIONAL predicate matching against job-run state transitions inside a workflow run — and the workflow-run action accounting it feeds (Statistics and Graph node JobRuns).
Traces: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds.

## Evidence
glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds — AssertionError: workflow statistics were {'TotalActions': 1, 'TimeoutActions': 0, 'FailedActions': 0, 'StoppedActions': 0, 'SucceededActions': 1, 'RunningActions': 0, 'ErroredActions': 0, 'WaitingActions': 0}, expected 2 succeeded actions.
Expected per AWS: Statistics.TotalActions == Statistics.SucceededActions == 2, Graph JOB nodes for both the producer and the consumer each with one SUCCEEDED JobRun, the consumer's object present and derived from the producer's (seed*3+7 from the producer's seed*3), and consumer StartedOn >= producer CompletedOn.

## Steps to Reproduce
1. Create a bucket and two pythonshell Glue jobs: a producer that writes seed*3 to s3://<bucket>/wf/produced.txt and a consumer that reads it and writes upstream+7 to wf/consumed.txt.
2. CreateWorkflow; CreateTrigger Type=ON_DEMAND with Actions=[producer] in that workflow; CreateTrigger Type=CONDITIONAL, StartOnCreation=true, Predicate ANY/EQUALS on (producer, SUCCEEDED), Actions=[consumer].
3. Start the workflow and poll GetWorkflowRun to a terminal Status: AWS reports two succeeded actions and both objects exist; the emulator reports one action and wf/consumed.txt is never written.
