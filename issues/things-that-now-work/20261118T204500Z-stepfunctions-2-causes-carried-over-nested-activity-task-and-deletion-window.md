Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel stepfunctions.start-sync-execution-on-deleting-state-machine-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel|_poll_activity_task#8|Timeout|resp = sfn.get_activity_task(activityArn=activity_arn, workerName="sim2real-behav")
Issue-Key: stepfunctions.start-sync-execution-on-deleting-state-machine-rejected|_poll_until_stable_error#8|Timeout|time.sleep(interval_s)

# stepfunctions: 2 causes carried over — an activity task inside a `Map` inside a `Parallel` branch is never scheduled, and the span a started `DeleteStateMachine` is observable over is bounded by what CloudFormation's own teardown waits out

> _Note: these two causes were split out of
> `20261110T140310Z-stepfunctions-5-net-new-causes-activity-backed-task.md`, whose other
> three causes (an activity-backed `Task` state's `Catch`, a tolerated-failure Distributed
> Map, the `states:StateMachineQualifier` condition and the 1000-version cap) are served —
> five of that issue's seven checks replay PASSED. These two are the remainder, each needing
> a change this emulator cannot make inside one service crate._

## Operation
GetActivityTask, StartSyncExecution

## Description

Cause 1 of 2 — an activity task a NESTED state list schedules is never scheduled at all, so
`GetActivityTask` hands out no token for it.
Failing check id: stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel.
AWS: an execution whose `Parallel` branch contains a `Map` whose `ItemProcessor` (or whose
sibling branch) has a `Task` state naming an activity ARN enqueues that task exactly as a
top-level `Task` state does: the next long poll of the activity is handed its `taskToken`
and the state input, and the task can then be answered, or aborted when a sibling branch
fails — which is what produces the recorded history chain `EvaluationFailed`,
`MapIterationFailed`, `TaskStateAborted` (the sibling branch's in-flight `Task`),
`MapStateFailed`, `ExecutionFailed` with `States.QueryEvaluationError` whose cause names the
failing task's own entered-event id.
Emulator: `crate::state::asl::walk::Interpreter::activity_task_state` refuses a `Task` state
that names an activity at `depth > 0` with `States.Runtime` ("state '<name>' schedules an
activity task inside a 'Map' iteration or a 'Parallel' branch, which this emulator does not
wait at, so nothing was run for it"), because a nested state list is walked to its own end
inside the enclosing state's turn: there is nowhere for one of its states to WAIT. The check
is killed at the 60 s harness cap inside its first `GetActivityTask` poll.
Why: a definition that fans work out to workers through a `Map` or a `Parallel` — the shape
every batch-of-human-steps workflow has — cannot be run at all, and no amount of worker
polling reaches it.
Fix: the interpreter's suspension model. A walk currently suspends only at the TOP level
(`crate::state::asl::Run`/`plan::Planned` carry at most one `waiting` task, resolved by a
continuation that re-enters the definition at a top-level state name), so serving this needs
a suspension that can be resumed INSIDE a frame — the `Parallel` branch and `Map` iteration
the task belongs to, their accumulated results and their sibling-abort bookkeeping preserved
across the request that answers the token. Do not make the nested state enqueue a task
without that: a token no reply can resume leaves the execution RUNNING for ever, which is
worse than the refusal it answers today.

Cause 2 of 2 — how long a started `DeleteStateMachine` stays observable is decided by what
CloudFormation's teardown waits out, so it is too short for a client polling for a stable
refusal.
Failing check id: stepfunctions.start-sync-execution-on-deleting-state-machine-rejected.
AWS: "DeleteStateMachine ... is an asynchronous operation. It sets the state machine's status
to DELETING and begins the deletion process", and for the EXPRESS workflow type the deletion
happens "eventually (usually less than a minute)". Within that window a call that would start
work on the ARN is refused `StateMachineDeleting` (HTTP 400); the recorded client only needs
that refusal to be STABLE — three consecutive reads of the same code inside 120 s.
Emulator: `crate::state::DELETE_SETTLE_DELAY` is 500 ms, shorter than one poll interval, so
the DELETING status is effectively unobservable: the second read already answers
`StateMachineDoesNotExist` and the check is killed at the 60 s harness cap inside its
stable-refusal poll.
Why: a client cannot distinguish "still deleting" from "gone", so it cannot write a teardown
that waits for the right one.
Fix: this span is NOT this service's alone, which is why it is carried over. Everything that
waits for the deletion waits for the RECORD to go, so lengthening it lengthens every
teardown that holds a state machine, and two contracts another crate owns bound it from
above:
* `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning/step_functions_state_machine.rs::removal` polls `DescribeStateMachine` until it answers for
  nothing, and that crate's suites give a stack operation 20 looks at ~750 ms
  (`tests/all/stack_owned_secret_teardown.rs::poll_to_terminal`, `harness::wait_for_the_transition`):
  measured here, a 10 s span leaves
  `tests/all/step_functions_state_machine_replacement.rs::a_state_machine_whose_type_changes_is_replaced_and_the_stack_completes`
  reporting `UPDATE_IN_PROGRESS`, and a 30 s span leaves
  `tests/all/step_functions_state_machine_stack_provenance.rs` reporting
  `DELETE_IN_PROGRESS` and `core/router/tests/all/cloudformation_provisions_sfn_state_machines.rs`
  reading a machine that is still there;
* the same crate's
  `tests/all/step_functions_state_machine_stack_provenance.rs:409-427` pins the other side —
  after the stack reaches `DELETE_COMPLETE`, `DescribeStateMachine` AND `StartExecution` must
  both answer `StateMachineDoesNotExist` — so a refusal that outlives the record on the start
  path is refused there too.
  A span long enough for the recorded client (≥ ~10 s, since 3 s was measured as not enough)
  therefore needs the deletion to be modelled as a transition those waiters can follow
  without waiting it out: CloudFormation's own teardown polling the DELETING status rather
  than only the record's absence, and its suites' budgets stated against the span the owning
  service publishes. That is one change across both crates, which is why it is filed here
  rather than forced through one of them.

## Evidence
Both checks: verdict mismatch, detail "Error: Check timed out after 60.0s".

1) stepfunctions.jsonata-task-error-two-levels-deep-propagates-through-map-and-parallel —
Expected: a `QueryLanguage=JSONata` `Task` error inside a `Map` inside a `Parallel` branch
produces `EvaluationFailed` -> `MapIterationFailed` -> `TaskStateAborted` (the sibling
branch's in-flight `Task`) -> `MapStateFailed` -> `ExecutionFailed` with
`States.QueryEvaluationError`, the cause naming the failing task's own entered-event id.
Actual: the branch's activity task is never claimed — the poll is handed no token — so the
chain is never emitted.

2) stepfunctions.start-sync-execution-on-deleting-state-machine-rejected — Expected: after
`DeleteStateMachine` on an EXPRESS machine, `StartSyncExecution` settles on a stable HTTP 400
`StateMachineDeleting` (three consecutive reads of that code within 120 s). Actual: the
refusal is `StateMachineDeleting` for the 500 ms the record stands and
`StateMachineDoesNotExist` after, so the code never streaks and the check is killed at the
60 s cap.

## Steps to Reproduce
Cause 1:
1. `sfn:CreateActivity` -> `<act>`. 2. `sfn:CreateStateMachine` (STANDARD,
`QueryLanguage: JSONata`) whose `StartAt` is a `Parallel` with two branches: one a `Map`
whose `ItemProcessor` has a `Task` whose JSONata `Arguments` misevaluate, the other a `Task`
whose `Resource` is `<act>`. 3. `sfn:StartExecution`. 4. `sfn:GetActivityTask(activityArn=<act>)`
in a loop — expected: a non-empty `taskToken` within seconds; observed: none, and
`sfn:GetExecutionHistory` shows the execution FAILED at the `Parallel` with `States.Runtime`
naming the nested task state.

Cause 2:
1. `sfn:CreateStateMachine` type=EXPRESS with a single `Pass`. 2. `sfn:DeleteStateMachine`.
3. `sfn:StartSyncExecution` in a loop — expected: the same refusal code on three consecutive
reads; observed: `StateMachineDeleting` once and `StateMachineDoesNotExist` thereafter.
4. Also assert what bounds it: deploy a CloudFormation stack holding an
`AWS::StepFunctions::StateMachine`, delete the stack, and read `DescribeStacks` — the
teardown must still reach `DELETE_COMPLETE` inside that crate's own poll budget.
