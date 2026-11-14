Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.notebook-execution-lifecycle-transitions emr.list-notebook-executions-ordering-and-status-filter
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.notebook-execution-lifecycle-transitions|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_poll_until#18|Timeout|return last
Issue-Commit: 67fbfda0367de56101c32e0461f095f83dd68482

# EMR notebook executions never become observable past START_PENDING within a client's polling budget: two notebook-execution checks are killed at the harness cut-off with nothing settled (1 cause)

> _Note: this issue was identified against the code base at commit `67fbfda0367de56101c32e0461f095f83dd68482`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNotebookExecution

## Description
ONE net-new root cause, reached by two failing checks of the `### emr` suite section. Both are killed by the harness after 60.0 s while still polling a notebook-execution status the emulator never presents, and neither is marked ALREADY FILED or OWNED in this replay report.

Cause 1 of 1: the notebook-execution lifecycle does not advance to an observable, pollable status, so no client-side wait on a notebook execution ever terminates. Both checks do the same two things and neither gets past them: start an execution of a real notebook on a cluster the emulator accepted, then poll DescribeNotebookExecution for the execution to leave START_PENDING and settle (one then stops it and waits for STOPPED; the other waits for one execution to reach FINISHED and a sibling to reach FAILED so Status= can partition them).

AWS: Smithy model `com.amazonaws.emr#ElasticMapReduce`. StartNotebookExecution (input requires ExecutionEngine + ServiceRole; output NotebookExecutionId: XmlStringMaxLen256; errors InvalidRequestException HTTP 400, InternalServerException HTTP 500) records an execution and the execution then advances on its own through the modeled statuses — `NotebookExecutionStatus` = START_PENDING | STARTING | RUNNING | FINISHING | FINISHED | FAILING | FAILED | STOP_PENDING | STOPPING | STOPPED. The model states each one: "START_PENDING indicates that the cluster has received the execution request but execution has not begun", "STARTING indicates that the execution is starting on the cluster", "RUNNING indicates that the execution is being processed by the cluster", "FINISHING indicates that execution processing is in the final stages", "FINISHED indicates that the execution has completed without error", "FAILING ... will not finish successfully", "FAILED indicates that the execution failed", "STOP_PENDING indicates that the cluster has received a StopNotebookExecution request and the stop is pending", "STOPPING indicates that the cluster is in the process of stopping the execution as a result of a StopNotebookExecution request", "STOPPED indicates that the execution stopped because of a StopNotebookExecution request". So: a notebook whose code cells all succeed reaches FINISHED (DescribeNotebookExecution.NotebookExecution.Status), with EndTime set and OutputNotebookURI pointing at the output file in the request's OutputNotebookS3Location (NotebookExecution$OutputNotebookURI: "The location of the notebook execution's output file in Amazon S3"); a notebook whose cell raises reaches FAILED with NotebookExecution$LastStateChangeReason ("The reason for the latest status change of the notebook execution") carrying the engine's own exception; StopNotebookExecution (input NotebookExecutionId, required) drives a live execution STOP_PENDING -> STOPPING -> STOPPED; and ListNotebookExecutions with the Status filter (ListNotebookExecutionsInput$Status -> NotebookExecutionStatus, "The status filter for listing notebook executions") returns exactly the executions in that status, with a stable order across two consecutive unfiltered calls. Every one of these is reached by polling, which is the documented and only way to consume a notebook execution.

Emulator: neither check ever observes any of it. Each fails with the single detail `Error: Check timed out after 60.0s` and no other diagnostic — i.e. the runner was still inside its DescribeNotebookExecution / DescribeCluster poll loop (or inside its own bounded RunJobFlow retry, which absorbs a refusal and so is indistinguishable from a hang) when the harness killed it. Nothing in either check waits on wall-clock time for its own sake: every wait is on an EMR state transition, and 144 checks of this same suite under the same 60 s budget do complete, so the cut-off means the emulator parked the notebook execution (or the cluster it names) in a state it never leaves inside the budget. Hypothesis (neither the model nor the report states which half it is): the notebook execution is recorded at START_PENDING and the clock-owed walk to STARTING/RUNNING and on to FINISHED/FAILED/STOPPED is not materialised on the read path, and/or RunJobFlow refuses the notebook-engine cluster shape these two checks launch (ReleaseLabel emr-7.9.0 with Applications Hadoop + Spark + Livy + JupyterEnterpriseGateway) so the execution is never started at all; either way no status a client can act on is ever served.

Why: a client that starts an EMR notebook execution and waits for it — the normal headless-notebook workflow — never gets a terminal status, never gets an OutputNotebookURI, and never sees STOPPED after StopNotebookExecution, so its wait loop cannot terminate: it hangs instead of completing or failing. ListNotebookExecutions' Status filter is equally unusable, because no execution ever occupies FINISHED or FAILED to be partitioned, and the ordering guarantee across two unfiltered listings is never exercised.

Fix: the notebook-execution state machine behind StartNotebookExecution / DescribeNotebookExecution / StopNotebookExecution in the emr service — the record's status progression and the point at which a read materialises the transitions the clock already owes — plus whatever refuses or stalls the notebook-engine cluster launch these executions hang off, and the Status filter and ordering in the ListNotebookExecutions handler once both terminal states are reachable. Relevant components: the EMR notebook-execution lifecycle and its runner in the emr service state, and the StartNotebookExecution handler.

Related open issues that do NOT cover these two failures (named rather than duplicated silently): 20261107T182201Z-emr-notebook-executions-never-run-a-healthy-notebook.md and 20261110T142808Z-emr-3-net-new-root-causes-the-spark.md both describe the same notebook family, but the replay report marks neither of these two checks as owned by them, and 20261107T182201Z's recorded key is an AssertionError on an emulator-authored FAILED reason rather than the timeout this replay produced.

Traces: emr.notebook-execution-lifecycle-transitions, emr.list-notebook-executions-ordering-and-status-filter.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Overall 144/283 passed; service heading `### emr` 124/220 (96 failed), of which only these two are neither ALREADY FILED nor OWNED:

  [mismatch] emr.notebook-execution-lifecycle-transitions
    Error: Check timed out after 60.0s
  [mismatch] emr.list-notebook-executions-ordering-and-status-filter
    Error: Check timed out after 60.0s

That single line is the whole detail the report carries for each: no exception type, no status code, no response body — the runner never got an answer it could assert on, it was still polling.

Expected (AWS, per the Smithy model's NotebookExecutionStatus documentation):
  - emr.notebook-execution-lifecycle-transitions: after StartNotebookExecution(ExecutionEngine={Id:<cluster id>,Type:EMR}, ServiceRole=<role>, NotebookS3Location={Bucket,Key}, OutputNotebookS3Location={...}, OutputNotebookFormat=HTML), DescribeNotebookExecution(NotebookExecutionId=<id>).NotebookExecution.Status leaves START_PENDING (to STARTING/RUNNING, not to FAILED/FAILING), and after StopNotebookExecution it reaches STOPPED, with NotebookExecutionId echoing the id that was started.
  - emr.list-notebook-executions-ordering-and-status-filter: of two executions on one cluster, the one whose cells succeed reaches FINISHED and the one whose cell raises reaches FAILED (LastStateChangeReason = the engine's own exception, i.e. the exception type and message the notebook raised — not emulator prose); ListNotebookExecutions(Status='FINISHED', ExecutionEngineId=<cluster id>) and Status='FAILED' then partition the two, and two consecutive unfiltered ListNotebookExecutions calls return the same order.

Actual (emulator): no polled condition is ever satisfied. Both checks are killed at the 60 s harness budget with the execution/cluster still in a non-terminal state (or with the check still inside its own RunJobFlow retry loop). The emulator serves these reads from memory, so 60 s is ample — the cut-off is a state that never advances, not slow provisioning.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Run the behavioral suite and read the '### emr' section; both checks below report only 'Error: Check timed out after 60.0s'.

Lifecycle (emr.notebook-execution-lifecycle-transitions):
3. Create an S3 bucket and PutObject a minimal Jupyter notebook whose single code cell is `1 + 1` (kernelspec language python) at key notebooks/<name>.ipynb.
4. Create the EMR service role and EC2 instance profile, then RunJobFlow with ReleaseLabel=emr-7.9.0, Applications=[Hadoop, Spark, Livy, JupyterEnterpriseGateway], Instances={Ec2SubnetId:<subnet>, KeepJobFlowAliveWhenNoSteps:true, InstanceGroups:[MASTER m5.xlarge x1, CORE m5.xlarge x1]}. Note whether RunJobFlow is accepted at all.
5. Poll DescribeCluster until Status.State == WAITING.
6. StartNotebookExecution(ExecutionEngine={Id:<cluster id>, Type:'EMR'}, ServiceRole=<service role>, NotebookS3Location={Bucket:<bucket>, Key:<key>}, OutputNotebookS3Location={Bucket:<bucket>, Key:'notebooks/output/'}, OutputNotebookFormat='HTML', NotebookParams='{}').
7. Poll DescribeNotebookExecution(NotebookExecutionId=<id>) — expect Status to leave START_PENDING; observe that it never does inside a client's polling budget.
8. StopNotebookExecution(NotebookExecutionId=<id>) and poll DescribeNotebookExecution — expect Status STOPPED; observe it is never reached.

List/filter (emr.list-notebook-executions-ordering-and-status-filter):
9. On the same kind of cluster, StartNotebookExecution twice: once for a notebook whose only cell succeeds, once for a notebook whose cell raises.
10. Poll DescribeNotebookExecution for both — expect FINISHED and FAILED respectively; observe neither terminal status arrives.
11. Call ListNotebookExecutions(Status='FINISHED', ExecutionEngineId=<cluster id>) and ListNotebookExecutions(Status='FAILED', ExecutionEngineId=<cluster id>) — expect the two executions partitioned; and call ListNotebookExecutions unfiltered twice — expect identical ordering.
