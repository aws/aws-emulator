Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative
Issue-Key: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative|_poll_until#37|Timeout|time.sleep(interval_s)
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# lambda__sfn__sqs: detaching the state machine role's lambda:InvokeFunction grant produces no observable task failure — the check never gets a control execution to grade

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Step Functions execution whose Task state invokes a Lambda function produces nothing to observe when the role's lambda:InvokeFunction grant is detached, because the control path never completes either.

AWS: a standard state machine whose Task is an optimized Lambda invocation succeeds while the execution role allows lambda:InvokeFunction on the function; detaching that grant makes the task fail with Lambda.AccessDeniedException (surfaced through the execution history / the Catch path) while the untouched control still succeeds. Smithy (sfn StartExecution / GetExecutionHistory, states DescribeExecution -> ExecutionStatus) models the shapes but not the interpreter's behavior — recorded behavior only.

Emulator: the check exhausts the 60 s harness budget ('Error: Check timed out after 60.0s'); no execution reaches a terminal status in either leg, so the permission differential cannot be measured.

Why: a client cannot validate least-privilege state-machine roles: a missing invoke grant and a non-executing interpreter look identical, and nothing reports why the workflow stalled.

Fix: the Step Functions task-execution path for Lambda invocations, including surfacing the authorization refusal as the task's error. Related open issues that do NOT carry this replay key: 20261029T183117Z-lambda-sfn-sqs-nine-net-new-causes-sqs.md, 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md.

Traces: lambda__sfn__sqs.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative.

## Evidence
Expected: with the grant attached the execution reaches SUCCEEDED; after detaching lambda:InvokeFunction the task fails with the Lambda access-denied error while the control execution still succeeds.
Actual: 'Error: Check timed out after 60.0s' — no execution settles in the budget.

## Steps to Reproduce
1. CreateFunction and a state machine whose Task invokes it, with a role allowing lambda:InvokeFunction. 2. StartExecution and poll DescribeExecution/GetExecutionHistory for the terminal status (control). 3. Detach the invoke policy from the role, StartExecution again and poll for the task failure. Replay check lambda__sfn__sqs.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative.
