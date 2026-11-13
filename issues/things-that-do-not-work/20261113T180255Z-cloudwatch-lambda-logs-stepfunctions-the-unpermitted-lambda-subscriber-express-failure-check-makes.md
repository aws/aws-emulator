Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected
Issue-Key: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected|_poll_until#28|Timeout|time.sleep(interval_s)
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# The unpermitted-Lambda-subscriber EXPRESS failure check makes no progress and hits the replay cap: nothing the scenario needs (the PutSubscriptionFilter refusal plus EXPRESS vended-log delivery) completes in time

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Cause 1 of 1: the check that grades (a) PutSubscriptionFilter's refusal when the destination Lambda grants logs.amazonaws.com no invoke permission and (b) that an EXPRESS state machine's FailStateEntered events reach its vended log group and are then forwarded once the grant exists, never reaches any of its own 360 s/420 s budgets — the emulator leaves it with no progress and the replay cap ends it at 60 s. Failing check: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): PutSubscriptionFilter(logGroupName, filterName, filterPattern, destinationArn required) models InvalidParameterException (httpError 400); a Lambda destinationArn whose resource policy does not allow logs.amazonaws.com to invoke it is refused with code InvalidParameterException and message "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function." (that whole sentence is fixed text — it carries no request values), and the refused call leaves DescribeSubscriptionFilters empty. Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): CreateStateMachine.loggingConfiguration{level, includeExecutionData, destinations[cloudWatchLogsLogGroup.logGroupArn]}. Beyond that the scenario is recorded behavior only: an EXPRESS run at level ALL delivers its execution-history events to the named group as JSON log events carrying `type` (e.g. FailStateEntered), `id` and `execution_arn`; after lambda.AddPermission for logs.amazonaws.com the same PutSubscriptionFilter is accepted and forwards exactly the matching events of executions started after the filter existed, and never the earlier unwired one.

Emulator: the replay produces only `Check timed out after 60.0s` — no assertion ever evaluates, so none of the surfaces above is observable. Hypothesis: the stall is the EXPRESS vended-log delivery readiness gate (_await_delivery_ready starts successful warm-up executions and waits for one of them to appear in the vended group), which is exactly the behavior the open issue 20261104T035532Z-cloudwatch-lambda-logs-stepfunctions-express-execution-logs-never.md records as never happening; the sibling check obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights is reported OWNED by 20261111T004529Z-cloudwatch-lambda-logs-stepfunctions-a-cloudwatch-logs-metric.md. Neither open issue carries this check's replay key, so this check is ungated and is filed here.

Why: a client cannot observe any log-derived signal of Step Functions EXPRESS failures, and cannot distinguish "the subscription filter was correctly refused" from "the call hung": a workflow that fails in production looks identical to a healthy one, and the deploy-time guard that an unpermitted Lambda destination is rejected up front is unverifiable.

Fix: the Step Functions execution-logging path that materializes an EXPRESS state machine's loggingConfiguration as CloudWatch Logs delivery (log-stream creation plus a per-history-event emitter honouring level/includeExecutionData) in the sfn crate's execution engine/state, and whatever in that path blocks instead of answering; then the Logs PutSubscriptionFilter destination-permission validation and subscription delivery for the repaired leg.

Traces: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected (detail: `Check timed out after 60.0s`).

## Evidence
Expected (AWS, per the recording): PutSubscriptionFilter(logGroupName=/aws/vendedlogs/states/<group>, filterName="fail-state-entered", filterPattern='{$.type = "FailStateEntered"}', destinationArn=<unpermitted function arn>) is refused with ('InvalidParameterException', 'Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.') and DescribeSubscriptionFilters on the group returns []; a warm-up EXPRESS execution's events then appear in the vended group; the first failing execution's FailStateEntered reaches the group while the subscriber has received nothing; after lambda.AddPermission(Principal=logs.amazonaws.com) the same PutSubscriptionFilter is accepted and the subscriber receives exactly the FailStateEntered record of the second failing execution.

Actual (emulator): no step of the above is ever observed — the replay reports only:
  Error: Check timed out after 60.0s
No response was produced for the graded calls, so neither the refusal's code/message nor any delivery could be compared.

## Steps to Reproduce
1. logs.CreateLogGroup for /aws/vendedlogs/states/<unique>.
2. Create a subscriber Lambda (python3.12, inline zip) with a role allowing only logs:CreateLogStream/logs:PutLogEvents on its own /aws/lambda/<fn> group, and do NOT call lambda.AddPermission for logs.amazonaws.com.
3. logs.PutSubscriptionFilter(logGroupName=<group>, filterName="fail-state-entered", filterPattern='{$.type = "FailStateEntered"}', destinationArn=<that function's arn>). Expected: InvalidParameterException "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.", and logs.DescribeSubscriptionFilters(<group>) == [].
4. sfn.CreateStateMachine(type=EXPRESS, role trusted by states.amazonaws.com, loggingConfiguration={level:"ALL", includeExecutionData:true, destinations:[{cloudWatchLogsLogGroup:{logGroupArn:"arn:aws:logs:<region>:<account>:log-group:<group>:*"}}]}).
5. sfn.StartSyncExecution with {"ok": true} and poll logs.FilterLogEvents on <group> for that execution's events. Expected: they arrive. Observed: the replay makes no progress and the check is killed at the 60 s cap.
6. Then StartSyncExecution with {"ok": false}, confirm FailStateEntered reaches <group> and the subscriber's group holds nothing; lambda.AddPermission(Principal=logs.amazonaws.com, SourceArn=<group>:*), re-run PutSubscriptionFilter (now accepted), StartSyncExecution {"ok": false} again, and expect the subscriber to log exactly that second execution's FailStateEntered.
