Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.activity-cancel-without-permission-rejected swf.activity-completion-explicitly-denied-rejected swf.child-failure-recorded-in-parent-history swf.failed-execution-listed-under-failed-close-status
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.activity-cancel-without-permission-rejected|_task#22|AssertionError|raise AssertionError(f"no task arrived for {kw!r}")
Issue-Key: swf.activity-completion-explicitly-denied-rejected|_next#21|AssertionError|raise AssertionError(f"no task was handed out for {kw!r}")
Issue-Key: swf.child-failure-recorded-in-parent-history|_next_decision#12|AssertionError|raise AssertionError(f"no decision task arrived on task list {task_list!r}")
Issue-Key: swf.failed-execution-listed-under-failed-close-status|_decide#11|AssertionError|raise AssertionError(f"no decision task arrived on task list {task_list!r}")
Issue-Commit: 8c63d5b7f8677a460355a4ac20e9a08b37d56b80

# swf: a run's initial decision task is never handed out — PollForDecisionTask long-polls forever with an empty taskToken after a successful StartWorkflowExecution

> _Note: this issue was identified against the code base at commit `8c63d5b7f8677a460355a4ac20e9a08b37d56b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForDecisionTask

## Description
Cause 1 of 1: after a StartWorkflowExecution that returns a runId (HTTP 200), PollForDecisionTask on the execution's effective task list never hands the run's first DecisionTask over; successive long polls keep answering the documented EMPTY DecisionTask until the caller gives up. Reproduced in both ways the effective task list can be resolved — taken from the workflow type's defaultTaskList with StartWorkflowExecution omitting taskList (swf.activity-cancel-without-permission-rejected, swf.activity-completion-explicitly-denied-rejected, swf.failed-execution-listed-under-failed-close-status) and taken from StartWorkflowExecution's own taskList while the type defines no defaultTaskList (swf.child-failure-recorded-in-parent-history). Failing check ids for this cause: swf.activity-cancel-without-permission-rejected, swf.activity-completion-explicitly-denied-rejected, swf.child-failure-recorded-in-parent-history, swf.failed-execution-listed-under-failed-close-status.

AWS: Smithy model, SWF (com.amazonaws.swf). StartWorkflowExecution 'starts an execution of the workflow type in the specified domain using the provided workflowId'; an execution that has just started has one decision task scheduled on its effective task list (the request's taskList, else the workflow type's defaultTaskList — the model's StartWorkflowExecution.taskList doc: 'If not specified, the default task list specified when registering the workflow type is used'), recorded as DecisionTaskScheduled right after WorkflowExecutionStarted. PollForDecisionTask is documented as: 'A decision task may be returned for any open workflow execution that is using the specified task list. ... This action initiates a long poll, where the service holds the HTTP connection open and responds as soon a task becomes available. If no decision task is available in the specified task list before the timeout of 60 seconds expires, an empty result is returned. An empty result, in this context, means that a DecisionTask is returned, but that the value of taskToken is an empty string.' So the first poll of that list by a decider must come back with a non-empty taskToken naming the started run, carrying its WorkflowExecutionStarted/DecisionTaskScheduled history. The empty result is reserved for a list that genuinely holds no waiting task.

Emulator: StartWorkflowExecution answers 200 with a runId, but every PollForDecisionTask on that task list answers 200 with the empty DecisionTask (taskToken = "", startedEventId = 0, previousStartedEventId = 0, no workflowExecution/workflowType, events = []) — not an error code, just a permanently empty list. Four checks loop the documented long poll until their time budget runs out and then assert: 'no task arrived for {...taskList...}', 'no task was handed out for {...}', 'no decision task arrived on task list ...'. Nothing downstream of the first decision can be exercised: no decision is ever responded to, so no activity is scheduled, no child is started, and no execution closes.

Why: a decider is the only way work advances in SWF. A client that starts an execution and polls its task list — the canonical SWF loop — hangs forever against this emulator, while on AWS it is handed the task within the first long poll. Every workflow built on StartWorkflowExecution + PollForDecisionTask + RespondDecisionTaskCompleted is unrunnable, and the symptom is indistinguishable from an idle task list, so clients see no error to retry on.

Fix: the PollForDecisionTask handler's hand-out path (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/poll_for_decision_task.rs — answer_poll's None/new-task branch, and whether the long-poll loop can observe a task that appears while it waits) together with the state that scheduled it (src/state.rs: Domain::next_decision_task_on / next_task_on, Execution::waiting_decision_tasks_on, Execution::schedule_decision_task_if_idle as called from Domain::start_run, and the expiry/reaping run on every poll: materialise_due_timeouts_in / expire_due_decision_tasks, which must not take a freshly scheduled task or a freshly started open run out of the list). Hypothesis (not stated by the model or the recording): the run's DecisionTaskScheduled event is either not matched to the polled task-list name or is removed again by the reaping that each poll performs before answering, so the task list reads as empty.

Not covered by any open issue: 20261127T121936Z-swf-iam-authorization-is-never-evaluated-unpermitted-condition.md is about SWF answering 200 instead of AccessDeniedException for unpermitted calls — the two access-control checks here never reach their AccessDenied assertion, they die earlier on the missing decision task; 20261127T121936Z-swf-decisions-that-aws-fails-are-applied-silently.md and 20261117T125351Z-swf-a-failed-decision-earlier-in-the-same.md concern how decisions already handed to a decider are applied.

## Evidence
Expected (AWS): StartWorkflowExecution -> 200 {runId}; the first PollForDecisionTask(domain, taskList=<effective task list>) -> 200 with taskToken non-empty (modeled TaskToken, length 1..1024, required), workflowExecution = {workflowId, runId} of the started run, workflowType = the started type, events containing WorkflowExecutionStarted and DecisionTaskScheduled, startedEventId = the id of the DecisionTaskStarted event this poll records.

Actual (emulator): StartWorkflowExecution -> 200 {runId}, then every PollForDecisionTask on that task list -> 200 with the empty DecisionTask (taskToken = "", startedEventId = 0, previousStartedEventId = 0, events = []), repeated for the whole check budget. Harness assertions, verbatim (the domain name, task-list names and ids are per-run values, the fixed text is the sentence):
- swf.activity-cancel-without-permission-rejected: AssertionError: no task arrived for {'domain': 'sim2real-swf-shared', 'taskList': {'name': 'sim2real-swfcx-tl-6d6587846ced'}}
- swf.activity-completion-explicitly-denied-rejected: AssertionError: no task was handed out for {'domain': 'sim2real-swf-shared', 'taskList': {'name': 'sim2real-swfdeny-tl-aac9099d171c'}}
- swf.child-failure-recorded-in-parent-history: AssertionError: no decision task arrived on task list 'sim2real-cwf-parent-tl-1d8ff3246a8f'
- swf.failed-execution-listed-under-failed-close-status: AssertionError: no decision task arrived on task list 'sim2real-fcs-tl-b2dc66e65d42'

In all four the failure is at the FIRST PollForDecisionTask of the run; no error code is returned at any point, so the divergence is a 200 empty result where AWS returns the task.

## Steps to Reproduce
1. RegisterDomain (or reuse) a REGISTERED domain D.
2. Variant A (type default): RegisterWorkflowType(domain=D, name=T, version=1, defaultTaskList={name: L}, defaultTaskStartToCloseTimeout=30, defaultExecutionStartToCloseTimeout=300, defaultChildPolicy=TERMINATE), then StartWorkflowExecution(domain=D, workflowId=W, workflowType={T,1}) with no taskList.
   Variant B (request override): RegisterWorkflowType(domain=D, name=T2, version=1, defaultTaskStartToCloseTimeout=30, defaultExecutionStartToCloseTimeout=300, defaultChildPolicy=TERMINATE) with NO defaultTaskList, then StartWorkflowExecution(domain=D, workflowId=W2, workflowType={T2,1}, taskList={name: L2}).
3. Observe StartWorkflowExecution answers 200 with a runId.
4. Call PollForDecisionTask(domain=D, taskList={name: L}) (resp. L2) in a loop, as a decider does.
   Expected: the first poll returns within the 60s long poll with a non-empty taskToken for W (resp. W2).
   Actual: every poll returns 200 with taskToken = "" and no events, indefinitely.
5. GetWorkflowExecutionHistory(domain=D, execution={workflowId: W, runId}) to confirm the run is OPEN and shows WorkflowExecutionStarted/DecisionTaskScheduled while no poll is ever served.
