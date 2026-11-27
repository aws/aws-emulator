Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.activity-failure-named-lambda-unknown-retried-then-succeeds stepfunctions.activity-states-permissions-retried-by-named-retrier-then-succeeds stepfunctions.retry-on-states-taskfailed-reruns-a-worker-failure-until-success stepfunctions.task-retry-states-all-reruns-failed-attempt-then-succeeds stepfunctions.activity-task-after-activity-deleted-rejected-as-states-runtime stepfunctions.execution-timeout-history-event-reports-states-timeout stepfunctions.distributed-map-parent-history-records-map-run-not-child-states stepfunctions.distributed-map-run-fails-once-failures-exceed-the-tolerated-count stepfunctions.distributed-map-zero-tolerance-records-map-run-failed-in-parent-history stepfunctions.map-run-listed-at-start-agrees-with-describe-and-history stepfunctions.map-tolerated-failure-count-holds-at-limit-and-fails-one-past-it stepfunctions.parallel-sibling-failure-aborts-distributed-map-run stepfunctions.stopping-parent-execution-ends-distributed-map-run-aborted stepfunctions.distributed-map-failed-batch-counts-every-item-in-it-failed stepfunctions.distributed-map-zero-tolerance-failure-leaves-unstarted-items-pending stepfunctions.distributed-map-stopped-parent-leaves-unstarted-items-pending stepfunctions.map-run-abort-rejects-worker-success-for-its-started-item stepfunctions.map-run-counts-item-running-while-its-child-awaits-activity-worker stepfunctions.map-run-stop-counts-started-item-aborted-and-unstarted-item-pending stepfunctions.parallel-catches-distributed-map-tolerated-failure-threshold-from-branch stepfunctions.inline-map-heartbeat-timeout-catcher-catches-silent-iteration-worker stepfunctions.map-catch-on-states-timeout-routes-a-timed-out-iteration stepfunctions.map-catch-states-taskfailed-takes-an-iteration-activity-failure stepfunctions.map-iteration-reference-path-conflict-escapes-map-catchers stepfunctions.jsonata-task-arguments-evaluation-error-retried-by-named-retrier stepfunctions.map-sibling-failure-aborts-the-iteration-holding-an-activity-task stepfunctions.parallel-branch-failure-aborts-sibling-activity-task-and-rejects-its-token stepfunctions.update-map-run-negative-max-concurrency-rejected stepfunctions.update-map-run-on-an-aborted-map-run-records-the-limit-without-restarting stepfunctions.inline-map-threshold-catcher-does-not-catch-iteration-error-fails
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.activity-failure-named-lambda-unknown-retried-then-succeeds|run#24|AssertionError|assert failed == [{"error": _RETRIED, "cause": _FIRST_CAUSE}], (
Issue-Key: stepfunctions.activity-states-permissions-retried-by-named-retrier-then-succeeds|run#26|AssertionError|assert failed == [{"error": _ERROR, "cause": _CAUSE}], f"ActivityFailed events were {failed!r}"
Issue-Key: stepfunctions.retry-on-states-taskfailed-reruns-a-worker-failure-until-success|run#24|AssertionError|assert [(f.get("error"), f.get("cause")) for f in failed] == [(_WORKER_ERROR, _WORKER_CAUSE)], (
Issue-Key: stepfunctions.task-retry-states-all-reruns-failed-attempt-then-succeeds|run#26|AssertionError|assert failed == [{"error": _ERROR, "cause": _CAUSE}], f"failed attempt details: {failed!r}"
Issue-Key: stepfunctions.activity-task-after-activity-deleted-rejected-as-states-runtime|run#29|AssertionError|assert activity_arn in second_desc.get("cause", ""), (
Issue-Key: stepfunctions.execution-timeout-history-event-reports-states-timeout|run#14|AssertionError|assert details == {"error": "States.Timeout"}, (
Issue-Key: stepfunctions.distributed-map-parent-history-records-map-run-not-child-states|run#7|AssertionError|assert "MapRunStarted" in parent and "MapRunSucceeded" in parent, (
Issue-Key: stepfunctions.distributed-map-run-fails-once-failures-exceed-the-tolerated-count|_map_run#9|AssertionError|assert len(started) == 1, f"expected one MapRunStarted event, got {len(started)}"
Issue-Key: stepfunctions.distributed-map-zero-tolerance-records-map-run-failed-in-parent-history|_map_run_of#18|AssertionError|assert len(started) == 1, f"expected one MapRunStarted event, saw types {[e['type'] for e in events]!r}"
Issue-Key: stepfunctions.map-run-listed-at-start-agrees-with-describe-and-history|run#24|AssertionError|assert started == [run_arn], (
Issue-Key: stepfunctions.map-tolerated-failure-count-holds-at-limit-and-fails-one-past-it|run#14|AssertionError|assert "MapRunSucceeded" in at_types and "MapRunFailed" not in at_types, (
Issue-Key: stepfunctions.parallel-sibling-failure-aborts-distributed-map-run|run#28|AssertionError|assert kinds.count(kind) == 1, f"expected one {kind}, parent history {kinds!r}"
Issue-Key: stepfunctions.stopping-parent-execution-ends-distributed-map-run-aborted|run#42|AssertionError|assert "MapRunStarted" in types, f"parent history types: {types!r}"
Issue-Key: stepfunctions.distributed-map-failed-batch-counts-every-item-in-it-failed|run#19|AssertionError|assert items == want_items, f"Map Run item counts were {items!r}, expected {want_items!r}"
Issue-Key: stepfunctions.distributed-map-zero-tolerance-failure-leaves-unstarted-items-pending|run#20|AssertionError|assert started < len(_TICKETS), (
Issue-Key: stepfunctions.distributed-map-stopped-parent-leaves-unstarted-items-pending|_wait_for#13|Timeout|return value
Issue-Key: stepfunctions.map-run-abort-rejects-worker-success-for-its-started-item|_start_until_running#18|AssertionError|raise AssertionError(f"Map Run ended {desc['status']!r} before running: {cause[:200]!r}")
Issue-Key: stepfunctions.map-run-counts-item-running-while-its-child-awaits-activity-worker|_start_until_running#18|AssertionError|raise AssertionError(f"Map Run ended {desc['status']!r} before running: {cause[:200]!r}")
Issue-Key: stepfunctions.map-run-stop-counts-started-item-aborted-and-unstarted-item-pending|_start_until_running#17|AssertionError|raise AssertionError(f"Map Run ended {desc['status']!r} before running: {cause[:200]!r}")
Issue-Key: stepfunctions.parallel-catches-distributed-map-tolerated-failure-threshold-from-branch|run#6|AssertionError|assert isinstance(out, list) and len(out) == 1 and len(out[0]) == len(_ROWS), (
Issue-Key: stepfunctions.inline-map-heartbeat-timeout-catcher-catches-silent-iteration-worker|_claim#4|AssertionError|assert task.get("taskToken"), f"the worker {worker!r} received no activity task: {task!r}"
Issue-Key: stepfunctions.map-catch-on-states-timeout-routes-a-timed-out-iteration|_claim#10|AssertionError|raise AssertionError(f"no activity task was handed to the worker within {_TASK_WAIT_S} s")
Issue-Key: stepfunctions.map-catch-states-taskfailed-takes-an-iteration-activity-failure|_work_one#8|AssertionError|assert token, f"no activity task was handed out for the Map iteration: {task!r}"
Issue-Key: stepfunctions.map-iteration-reference-path-conflict-escapes-map-catchers|_work_one_item#10|AssertionError|raise AssertionError("the Map iteration's activity task was never handed out")
Issue-Key: stepfunctions.jsonata-task-arguments-evaluation-error-retried-by-named-retrier|run#16|AssertionError|assert task.get("taskToken"), f"the retried attempt handed out no activity task: {task!r}"
Issue-Key: stepfunctions.map-sibling-failure-aborts-the-iteration-holding-an-activity-task|_claim#10|Timeout|task = worker.get_activity_task(activityArn=activity_arn,
Issue-Key: stepfunctions.parallel-branch-failure-aborts-sibling-activity-task-and-rejects-its-token|_claim#14|Timeout|task = sfn.get_activity_task(activityArn=activity_arn, workerName=worker)
Issue-Key: stepfunctions.update-map-run-negative-max-concurrency-rejected|_until#31|AssertionError|raise AssertionError(f"{what} not reached within {budget_s:.0f}s; last={value!r}")
Issue-Key: stepfunctions.update-map-run-on-an-aborted-map-run-records-the-limit-without-restarting|_until#14|Timeout|time.sleep(2)
Issue-Key: stepfunctions.inline-map-threshold-catcher-does-not-catch-iteration-error-fails|run#11|KeyError|assert json.loads(ok["output"])["caught"] == {"Error": _ITEM_ERROR, "Cause": _ITEM_CAUSE}, (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 8 causes — execution history events drop their cause/error, Distributed Map emits no MapRun* events and mis-counts items, Map-iteration activity tasks are never handed out, and UpdateMapRun does not apply maxConcurrency

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Cause 1 of 8: ActivityFailed history events carry only `error`, never the worker's `cause`.
AWS: SendTaskFailure's error and cause are both recorded on the ActivityFailed event's activityFailedEventDetails, so a retried-then-succeeded execution's history shows the failed attempt with {error: <the worker's error>, cause: <the worker's cause>}.
Emulator: the event details hold only the error — [{'error': 'Lambda.Unknown'}], [{'error': 'States.Permissions'}], [{'error': 'Worker.Flaky'}], [{'error': 'Gateway.Busy'}].
Why: the diagnostic text a worker reported for a failed attempt is lost, so post-mortem tooling and tests that read the cause out of the history see nothing.
Fix: the ActivityFailed history-event construction in the Step Functions execution/history layer (carry SendTaskFailure's cause).
Traces: stepfunctions.activity-failure-named-lambda-unknown-retried-then-succeeds, stepfunctions.activity-states-permissions-retried-by-named-retrier-then-succeeds, stepfunctions.retry-on-states-taskfailed-reruns-a-worker-failure-until-success, stepfunctions.task-retry-states-all-reruns-failed-attempt-then-succeeds.

Cause 2 of 8: the execution cause for a deleted activity truncates the activity ARN.
AWS: the cause names the activity in full — "An error occurred while executing the state '<state>' (entered at the event id #<n>). The activity <full activity ARN> does not exist." The fixed text is AWS's; the state name, event id and ARN come from the run.
Emulator: the ARN is cut mid-string and ellipsised — "The activity arn:aws:states:us-west-2:000000000000:activity:sim2real-behav-sfn-ada-e39e210c7… does not exist."
Why: the cause does not identify which activity is missing, which is the only thing that message carries.
Fix: the States.Runtime cause composition for a missing activity in the Task executor (do not truncate the ARN).
Traces: stepfunctions.activity-task-after-activity-deleted-rejected-as-states-runtime.

Cause 3 of 8: executionTimedOutEventDetails omits the error.
AWS: an execution that exceeds its TimeoutSeconds records ExecutionTimedOut with executionTimedOutEventDetails {error: 'States.Timeout'}.
Emulator: the details are {}.
Why: a client reading the timeout reason from the history event gets nothing, and 'States.Timeout' — the error a Catch/Retry names — is absent from the record.
Fix: the ExecutionTimedOut history-event construction in the execution engine.
Traces: stepfunctions.execution-timeout-history-event-reports-states-timeout.

Cause 4 of 8: a Distributed Map emits no MapRun* events in the parent execution's history.
AWS: a DISTRIBUTED Map records MapRunStarted in the parent history naming the Map Run ARN, and the run's terminal state as MapRunSucceeded/MapRunFailed; the parent history does NOT contain the child states. ListMapRuns and DescribeMapRun agree with that MapRunStarted.
Emulator: the parent history is the inline-Map shape — ['ExecutionStarted', 'MapStateEntered', 'MapStateStarted', 'MapStateSucceeded', 'MapStateExited', 'ExecutionSucceeded'] — with zero MapRunStarted events, and the same for the failed, zero-tolerance, aborted and parallel-sibling variants.
Why: a Distributed Map is indistinguishable from an inline Map in the history; nothing can correlate a Map Run ARN with the parent execution, so Map Run monitoring and redrive tooling cannot work.
Fix: the Distributed Map executor and the history-event writer in the Step Functions execution engine (emit MapRunStarted/MapRunSucceeded/MapRunFailed and keep child states out of the parent history).
Traces: stepfunctions.distributed-map-parent-history-records-map-run-not-child-states, stepfunctions.distributed-map-run-fails-once-failures-exceed-the-tolerated-count, stepfunctions.distributed-map-zero-tolerance-records-map-run-failed-in-parent-history, stepfunctions.map-run-listed-at-start-agrees-with-describe-and-history, stepfunctions.map-tolerated-failure-count-holds-at-limit-and-fails-one-past-it, stepfunctions.parallel-sibling-failure-aborts-distributed-map-run, stepfunctions.stopping-parent-execution-ends-distributed-map-run-aborted.

Cause 5 of 8: Map Run item accounting and the tolerated-failure threshold are wrong.
AWS: DescribeMapRun's itemCounts reflect every item — a failed batch counts each item in it as failed (6 items: 4 succeeded, 2 failed) — and the threshold is evaluated against the failures actually seen: at 0% tolerance the run stops before every item is started, leaving the unstarted ones `pending`; a stop leaves the started item `aborted` and the unstarted one `pending`; and a tolerated-failure threshold reached inside a branch is caught by the enclosing Parallel with the branch's item results intact.
Emulator: counts are per-batch rather than per-item ({'total': 3, 'succeeded': 2, 'failed': 1} where AWS reports {'total': 6, 'succeeded': 4, 'failed': 2}); at 0% tolerance all 8 of 8 items got a child; and three runs end FAILED immediately with 'The specified tolerated failure threshold was exceeded' before any item runs.
Why: itemCounts and executionCounts — the only way to observe a Map Run's progress — are wrong, and a run that should process its items fails up front, so a client's throughput, pending/aborted bookkeeping and partial-failure handling are all unusable.
Fix: the Map Run item-state accounting and tolerated-failure evaluation in the Distributed Map executor.
Traces: stepfunctions.distributed-map-failed-batch-counts-every-item-in-it-failed, stepfunctions.distributed-map-zero-tolerance-failure-leaves-unstarted-items-pending, stepfunctions.distributed-map-stopped-parent-leaves-unstarted-items-pending, stepfunctions.map-run-abort-rejects-worker-success-for-its-started-item, stepfunctions.map-run-counts-item-running-while-its-child-awaits-activity-worker, stepfunctions.map-run-stop-counts-started-item-aborted-and-unstarted-item-pending, stepfunctions.parallel-catches-distributed-map-tolerated-failure-threshold-from-branch.

Cause 6 of 8: an activity Task inside a Map iteration is never handed to a GetActivityTask poller.
AWS: a Task on an activity ARN inside a Map (or a Parallel branch) schedules an activity task that the next GetActivityTask long poll returns, carrying a taskToken; that is what lets heartbeat timeouts, Catch on States.Timeout/States.TaskFailed, retriers and sibling-abort token rejection be exercised.
Emulator: GetActivityTask returns an empty body ({} with content-length 2) for the whole poll — "the worker 'answering-worker' received no activity task", "no activity task was handed to the worker within 150 s", "the Map iteration's activity task was never handed out" — and two of these checks are killed at the 60 s harness cap.
Why: no workflow can use an activity worker from inside a Map iteration or a Parallel branch; iterations hang instead of running, and every error path built on them (heartbeat timeout, iteration catch, retry, sibling abort) is unreachable.
Fix: activity-task scheduling/dispatch for states running inside Map iterations and Parallel branches in the Step Functions execution engine.
Traces: stepfunctions.inline-map-heartbeat-timeout-catcher-catches-silent-iteration-worker, stepfunctions.map-catch-on-states-timeout-routes-a-timed-out-iteration, stepfunctions.map-catch-states-taskfailed-takes-an-iteration-activity-failure, stepfunctions.map-iteration-reference-path-conflict-escapes-map-catchers, stepfunctions.jsonata-task-arguments-evaluation-error-retried-by-named-retrier, stepfunctions.map-sibling-failure-aborts-the-iteration-holding-an-activity-task, stepfunctions.parallel-branch-failure-aborts-sibling-activity-task-and-rejects-its-token.

Cause 7 of 8: UpdateMapRun does not apply maxConcurrency to a running or aborted Map Run.
AWS: UpdateMapRun changes the running Map Run's limits and DescribeMapRun reports the new maxConcurrency; applied to an already-aborted Map Run it records the new limit without restarting it. Smithy: UpdateMapRun.maxConcurrency targets MaxConcurrency with @range min 0; errors are InvalidArn, ResourceNotFound, ValidationException.
Emulator: after UpdateMapRun(maxConcurrency=0) DescribeMapRun still reports maxConcurrency 2147483647 (and the aborted-run variant is killed at the harness cap), so the update is silently dropped.
Why: a client cannot throttle or unthrottle a running Map Run; the read-back contradicts the accepted write.
Fix: the UpdateMapRun handler and the Map Run state it writes to.
Traces: stepfunctions.update-map-run-negative-max-concurrency-rejected, stepfunctions.update-map-run-on-an-aborted-map-run-records-the-limit-without-restarting.

Cause 8 of 8: an inline Map's ToleratedFailure threshold error is not routed to the catcher that names it.
AWS: when an inline Map exceeds its tolerated failure threshold, the resulting error is caught by a Catcher naming it and the execution follows the catch transition, so the caught result is present in the output.
Emulator: the output has no 'caught' member at all — the check fails with KeyError: 'caught'.
Why: an inline Map's threshold failure escapes its catcher, so the workflow's recovery branch never runs.
Fix: the inline Map executor's error classification and Catch matching in the execution engine.
Traces: stepfunctions.inline-map-threshold-catcher-does-not-catch-iteration-error-fails.

## Evidence
Cause 1: "the history must record the one failed attempt with its error and cause; got [{'error': 'Lambda.Unknown'}]"; "ActivityFailed events were [{'error': 'States.Permissions'}]"; "ActivityFailed events carried [{'error': 'Worker.Flaky'}], expected one with the worker's error and cause"; "failed attempt details: [{'error': 'Gateway.Busy'}]".
Cause 2: "execution cause does not name the deleted activity: \"... The activity arn:aws:states:us-west-2:000000000000:activity:sim2real-behav-sfn-ada-e39e210c7… does not exist.\"".
Cause 3: "executionTimedOutEventDetails was {}, expected {'error': 'States.Timeout'}".
Cause 4: "the parent history must record the Map Run, got ['ExecutionStarted','MapStateEntered','MapStateStarted','MapStateSucceeded','MapStateExited','ExecutionSucceeded']"; "expected one MapRunStarted event, got 0"; "history should hold one MapRunStarted naming the listed run; held 0".
Cause 5: "Map Run item counts were {'total': 3, 'succeeded': 2, 'failed': 1, ...}, expected {'total': 6, 'succeeded': 4, 'failed': 2, ...}"; "at 0% tolerance 8 of 8 items got a child"; "Map Run ended 'FAILED' before running: 'The specified tolerated failure threshold was exceeded'" (three checks); "the Parallel output was [[{'counted': 's-1'}, {'counted': 's-3'}]], expected one branch holding 3 item results".
Cause 6: "the worker 'answering-worker' received no activity task: {... 'content-length': '2' ...}"; "no activity task was handed to the worker within 150 s"; "no activity task was handed out for the Map iteration"; "the Map iteration's activity task was never handed out"; "the retried attempt handed out no activity task"; plus "Error: Check timed out after 60.0s".
Cause 7: "maxConcurrency 0 on the Map Run not reached within 5s; last={... 'maxConcurrency': 2147483647 ...}".
Cause 8: KeyError: 'caught'.

## Steps to Reproduce
1. Fail an activity task via SendTaskFailure(error, cause) and read activityFailedEventDetails in GetExecutionHistory (cause 1); delete an activity while a Task on it runs and read the execution cause (cause 2); let an execution exceed TimeoutSeconds and read executionTimedOutEventDetails (cause 3).
2. Run a DISTRIBUTED Map and inspect the parent GetExecutionHistory for MapRunStarted/MapRunSucceeded/MapRunFailed (cause 4), then DescribeMapRun's itemCounts for a failed batch, a 0%-tolerance run and a stopped run (cause 5).
3. Put an activity Task inside a Map iteration or Parallel branch and long-poll GetActivityTask (cause 6).
4. Call UpdateMapRun(maxConcurrency=0) on a running Map Run and read DescribeMapRun (cause 7).
5. Run an inline Map past its ToleratedFailureCount with a Catcher naming the threshold error (cause 8).
6. Or replay the check ids in trace_ids.
