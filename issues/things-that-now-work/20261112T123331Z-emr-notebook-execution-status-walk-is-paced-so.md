Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.list-notebook-executions-ordering-and-status-filter
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_poll_until#9|Timeout|return last
Issue-Commit: 4839448a699fa31c23507e6b5ab2cdd919186720

# EMR notebook-execution status walk is paced so slowly that the Status-filter/ordering check never reaches its assertions (1 cause)

> _Note: this issue was identified against the code base at commit `4839448a699fa31c23507e6b5ab2cdd919186720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNotebookExecution

## Description
ONE root cause.

Cause 1 of 1: an EMR notebook execution takes tens of seconds of wall clock to reach a terminal status, and StartNotebookExecution does not answer until the notebook itself has been run, so a client that starts two executions on one cluster and then reads ListNotebookExecutions cannot get through that sequence inside the replay budget. Failing check: emr.list-notebook-executions-ordering-and-status-filter (verdict: mismatch, 'Check timed out after 60.0s').

AWS: recorded behavior only for the timing; the model fixes the states. Smithy (com.amazonaws.emr#ElasticMapReduce): StartNotebookExecution input requires ExecutionEngine (ExecutionEngineConfig) + ServiceRole, output NotebookExecutionId (XmlStringMaxLen256), modeled errors InvalidRequestException (HTTP 400) / InternalServerException (HTTP 500); NotebookExecution$Status is NotebookExecutionStatus = START_PENDING | STARTING | RUNNING | FINISHING | FINISHED | FAILING | FAILED | STOP_PENDING | STOPPING | STOPPED. StartNotebookExecution returns the execution id as soon as the cluster has RECEIVED the request ('START_PENDING indicates that the cluster has received the execution request but execution has not begun', NotebookExecution$Status doc) — the notebook is run asynchronously on the cluster, not inside the API call. ListNotebookExecutions(Status=..., ExecutionEngineId=...) then partitions the executions of that engine by Status (output NotebookExecutions: NotebookExecutionSummaryList) and two consecutive unfiltered listings return the same order. The recording shows one notebook whose only code cell is `print(1 + 1)` polled to FINISHED and a sibling whose cell raises polled to FAILED, with the engine's own exception text in LastStateChangeReason.

Emulator: the execution's status walk and the start call are both paced by the emulator itself, and together they exceed the suite's 60 s per-check budget, so the replay is cut off before the first Status-filtered listing is ever issued. The walk in the notebook-execution lifecycle holds RUNNING for NOTEBOOK_RUN_SPAN = 20 s and spends one NOTEBOOK_PHASE_SPAN = 2 s on each of the other moves (START_PENDING->STARTING, STARTING->RUNNING, FINISHING->FINISHED), i.e. ~26 s before an execution is terminal; the check settles TWO executions sequentially on one freshly launched cluster, and its own polls are 15 s (execution) and 30 s (cluster state) apart, so the observable completion of the pair lands at ~60-90 s. On top of that, StartNotebookExecution runs the notebook INSIDE the request: the handler awaits the notebook fetch and the cell run before it mints the id and answers, waiting up to the run's wall clock plus grace (5 s + 5 s) and up to a further 5 s for a free run permit when the process already has 4 runs in flight. Hypothesis (the replay reports only the timeout, not which call stalled): the 60 s overrun is the sum of these emulator-chosen spans plus the synchronous start call, not a refusal or a wrong value — the previous replay of this same check got as far as an AssertionError inside the check's own poll helper, so the path is reachable and only its pacing changed.

Why: a client that starts an EMR notebook execution sees StartNotebookExecution block for seconds instead of returning immediately with START_PENDING (so a caller that starts several executions serialises on the emulator's run pool), and any workflow that starts more than one execution and then filters/orders ListNotebookExecutions cannot complete in a time a test or a CI step allows — the Status=FINISHED / Status=FAILED partition and the order-stability behavior remain unverifiable even though the individual states are modelled correctly.

Fix: the notebook-execution lifecycle's pacing and the start handler's synchrony in the emr service — the phase spans of the execution status walk in the notebook-execution state module (RUNNING must still be observable, but the whole walk must settle in a small multiple of a client poll, not ~26 s), and StartNotebookExecution's handler, which should record the execution and answer at START_PENDING and perform the notebook run off the request path instead of awaiting it before responding.

Related already-filed issues (named, not duplicated): open issue 20261107T182201Z-emr-notebook-executions-never-run-a-healthy-notebook.md carries this same check id, but its recorded replay key is an AssertionError from _run_execution_until ('a healthy notebook ends FAILED with an emulator-authored reason'); the notebook now really runs (the checkout fetches the notebook from the emulator's S3 and executes its code cells), so that failure mode is gone and the replay did not mark this failure as owned by it. Fixed issue 20261105T093917Z-emr-6-net-new-root-causes-the-emr.md covered the 'notebook executions never run' family. This is not a refusal, so it is not a reopening of either.

Traces: emr.list-notebook-executions-ordering-and-status-filter.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (overall 128/283; emr 114/220).

[mismatch] emr.list-notebook-executions-ordering-and-status-filter
  Error: Check timed out after 60.0s

Expected: after the keep-alive cluster reports WAITING/RUNNING, StartNotebookExecution answers immediately with a NotebookExecutionId at Status START_PENDING; each execution then advances through STARTING/RUNNING/FINISHING and settles (FINISHED for the `print(1 + 1)` notebook, FAILED with the notebook's own exception for the one that raises) quickly enough for a client polling every 15 s to read both; ListNotebookExecutions(Status='FINISHED'|'FAILED', ExecutionEngineId=<cluster id>) then partitions the two ids and two consecutive unfiltered listings return identical order.
Actual: the check is cut off at the suite's 60 s budget before any listing assertion runs. In the checkout, the execution walk is START_PENDING --2 s--> STARTING --2 s--> RUNNING --20 s--> FINISHING --2 s--> FINISHED (NOTEBOOK_RUN_SPAN = 20 s, NOTEBOOK_PHASE_SPAN = 2 s), ~26 s per execution for the two executions this check runs one after the other, and StartNotebookExecution itself awaits the notebook run (wall clock 5 s + grace 5 s, plus up to 5 s waiting for one of 4 run permits) before it answers.
Fixed text vs variable parts: no error code or message is involved on this path — the requirement at issue is the STATE sequence (the NotebookExecutionStatus values above) and that each of START_PENDING/STARTING/RUNNING/FINISHING/FINISHED is observable without the whole walk costing tens of seconds; the cluster id, execution ids, bucket and keys are values from the request and are not part of the requirement.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Replay simparity-behavioral-aws-v1 and read the '### emr' section: emr.list-notebook-executions-ordering-and-status-filter is reported '[mismatch] ... Check timed out after 60.0s'.
3. By hand: create an S3 bucket; upload two one-code-cell notebooks, one whose cell is `print(1 + 1)` and one whose cell raises; RunJobFlow a keep-alive cluster with an EMR service role and EC2 instance profile and wait for Status WAITING/RUNNING.
4. Call emr StartNotebookExecution with ExecutionEngine={'Id': <cluster id>, 'Type': 'EMR'}, the service role and NotebookS3Location pointing at the healthy notebook. Time the call: expect an immediate answer; observe the call blocking while the notebook is run inside the request.
5. Poll emr DescribeNotebookExecution on the returned id and record the instant each Status is first seen: expect the walk to settle in a few seconds; observe ~2 s to STARTING, ~2 s to RUNNING, then ~20 s pinned at RUNNING before FINISHING/FINISHED (~26 s total).
6. Repeat 4-5 for the raising notebook (expect FAILED with the notebook's own exception) and then call emr ListNotebookExecutions(Status='FINISHED'|'FAILED', ExecutionEngineId=<cluster id>) plus two unfiltered listings — the two executions together with the cluster launch exceed the 60 s per-check budget, which is why the assertions are never reached.
