Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves
Issue-Key: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves|_poll#29|Timeout|time.sleep(_POLL_S)
Issue-Commit: 62a489da4fc45e0eadb1189eb41434873a830e66

# cloudwatch__sfn: an activity-backed Task state never delivers a task to GetActivityTask, so task-timeout refusal and the AWS/States activity counters cannot be observed (1 cause)

> _Note: this issue was identified against the code base at commit `62a489da4fc45e0eadb1189eb41434873a830e66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActivityTask

## Description
Cause 1 of 1 — a RUNNING execution whose Task state names an activity ARN never enqueues a task for that activity, so GetActivityTask long-polls without ever returning a taskToken and the whole timed-out-activity / CloudWatch-counter scenario is unreachable.
Failing check id: cloudwatch__sfn.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model) models GetActivityTask with required input activityArn (@length 1..256) and optional workerName, and output {taskToken (@length 1..2048, @sensitive), input (@length 0..1048576, @sensitive)}; modeled errors are ActivityDoesNotExist, ActivityWorkerLimitExceeded, InvalidArn, KmsAccessDeniedException, KmsInvalidStateException, KmsThrottlingException. In AWS, entering a Task state whose Resource is an activity ARN enqueues the task, and the next long-poll returns a non-empty taskToken with the state input. The behavior this check grades on top of that: an answer arriving AFTER the Task state's TimeoutSeconds is refused — SendTaskSuccess with that token returns HTTP 400 with error code TaskTimedOut (a modeled SendTaskSuccess error) — the execution is already FAILED with error "States.Timeout" and carries no output, and the refused late answer changes nothing. In CloudWatch, namespace AWS/States then shows ActivitiesTimedOut (dimension ActivityArn) and ExecutionsFailed (dimension StateMachineArn) moving for the subject while ActivitiesSucceeded / ExecutionsSucceeded / ExecutionsTimedOut stay at zero, and the in-time control machine on a second activity moves exactly ActivitiesSucceeded and ExecutionsSucceeded and not ActivitiesTimedOut / ExecutionsFailed. (Metric emission here is recorded behavior only — the Smithy model does not describe AWS/States publication.)
Emulator: the check is killed with "Error: Check timed out after 60.0s" while still inside its _poll_task helper, which calls GetActivityTask in a loop with a 120 s budget for the subject activity. No taskToken is ever returned for either the subject or the control activity, so SendTaskSuccess is never called, neither execution leaves RUNNING, and no AWS/States datapoint is ever read.
Why: a client that observes workflow health through AWS/States counters sees nothing. An activity-worker application never receives work, its executions never reach a terminal state, and the late-answer refusal (TaskTimedOut) that AWS uses to keep a worker from corrupting an already-failed execution is never exercised — so an emulator-based test would wrongly pass code that accepts a late result.
Fix: the activity task scheduling path shared by the interpreter's Task-state handler and GetActivityTask's handler/state (the Task state must enqueue against the activity ARN; GetActivityTask must dequeue and mint a token), plus the Task-state timeout path that must mark the token TaskTimedOut and FAIL the execution with States.Timeout, and the AWS/States metric emission for ActivitiesTimedOut / ActivitiesSucceeded / ExecutionsFailed / ExecutionsSucceeded. Related but not covering this: open issues 20261024T190039Z-sfn-step-functions-activities-and-teststate-diverge-task-states.md and 20261025T020208Z-cloudwatch-sfn-step-functions-publishes-no-alias-version-attributed-aws.md; the replay report did not mark this check as owned by either.
Traces: cloudwatch__sfn.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.

## Evidence
Expected: GetActivityTask(activityArn=<subject activity>, workerName="sim2real-worker") returns a non-empty taskToken; after the Task state's TimeoutSeconds (5 s) elapses, DescribeExecution shows status FAILED with error "States.Timeout" and no output; SendTaskSuccess with the stale token is refused with HTTP 400 TaskTimedOut; AWS/States sums settle at {slow ActivitiesTimedOut: 1.0, slow ExecutionsFailed: 1.0, ctrl ActivitiesSucceeded: 1.0, ctrl ExecutionsSucceeded: 1.0} while slow ActivitiesSucceeded / slow ExecutionsSucceeded / slow ExecutionsTimedOut / ctrl ActivitiesTimedOut / ctrl ExecutionsFailed stay at 0.
Actual: "Error: Check timed out after 60.0s" — the harness killed the check inside _poll_task, i.e. GetActivityTask returned no taskToken for the entire window. SendTaskSuccess, DescribeExecution and every GetMetricStatistics assertion were never reached.

## Steps to Reproduce
1. iam:CreateRole trusted by states.amazonaws.com.
2. sfn:CreateActivity twice -> <slow_act>, <ctrl_act>.
3. sfn:CreateStateMachine twice with definition {"StartAt":"Work","States":{"Work":{"Type":"Task","Resource":"<activity arn>","TimeoutSeconds":N,"End":true}}} — N=5 for the subject, N=300 for the control.
4. sfn:StartExecution on both with input "{}".
5. sfn:GetActivityTask(activityArn=<slow_act>, workerName="sim2real-worker") in a loop. Expected: a non-empty taskToken within seconds. Observed: no taskToken for the whole window, and the check is killed at the 60 s cap.
6. (unreachable today) wait past TimeoutSeconds, then sfn:SendTaskSuccess with the subject token -> HTTP 400 TaskTimedOut; sfn:DescribeExecution -> FAILED / error "States.Timeout" / no output; cloudwatch:GetMetricStatistics in namespace AWS/States for ActivitiesTimedOut (ActivityArn=<slow_act>) and ExecutionsFailed (StateMachineArn=<slow_sm>) -> Sum 1.0, with ActivitiesSucceeded / ExecutionsSucceeded / ExecutionsTimedOut for the subject and ActivitiesTimedOut / ExecutionsFailed for the control staying 0.
Replay: cloudwatch__sfn.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.
