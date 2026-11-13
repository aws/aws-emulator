Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task|run#12|AssertionError|assert std_final.get("cause") == expected_cause, (
Issue-Commit: 3bfb7cd772781c7f8fd33f0bb610479e140a70f3

# stepfunctions: the assume-role-denied failure cause is emulator-authored — "Neither the global service principal ..., nor the regional one ..., is not authorized to assume the provided role <arn>." instead of AWS's "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <arn>)"

> _Note: this issue was identified against the code base at commit `3bfb7cd772781c7f8fd33f0bb610479e140a70f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Cause 1 of 1: when a Task state cannot run because Step Functions may not assume the state machine's execution role, the execution's failure cause is emulator-invented text instead of AWS's assume-role-denied wording.
Failing check id: stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task.
AWS: recorded behaviour only (Smithy models DescribeExecution's output members error (SensitiveError, max 256) and cause (SensitiveCause, max 32768) but not their text; services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json). An execution role whose trust policy Condition (here ArnLike aws:SourceArn naming another account's state machines) does not match the starting state machine can never be assumed, so the Task fails: DescribeExecution reports status FAILED, error "States.TaskFailed" and cause whose text is entirely fixed except the role ARN — "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <role ARN>)", where <role ARN> is the roleArn configured on the state machine. The same text is what an EXPRESS StartSyncExecution response carries in its cause. (On the CMK/StartExecution refusal path the same sentence appears prefixed with "com.amazonaws.sfn.invoker.sts.AssumeRoleFailedCacheableException: "; the trailing "(role: ...)" parenthesis is part of the fixed form in both.)
Emulator: status FAILED and error States.TaskFailed are produced, but the cause reads "Neither the global service principal states.amazonaws.com, nor the regional one states.<region>.amazonaws.com, is not authorized to assume the provided role <role ARN>." — invented wording that enumerates two service principals, is ungrammatical ("Neither ... nor ... is not"), drops the "(role: <arn>)" parenthesised form in favour of the ARN inline with a trailing period, and leaks the emulator's regional-principal implementation detail.
Why: the cause string is the only diagnosis a client gets for the single most common Step Functions misconfiguration. Runbooks, log-based alarms and catchers that match AWS's sentence (or parse the role out of "(role: ...)") do not fire, and a human reading the cause is told something about a "regional principal" that AWS never says.
Fix: the execution runtime's assume-role failure path — the cause text it writes into the execution's cause (and into the TaskFailed history event / StartSyncExecution response) must be AWS's fixed sentence with only the role ARN substituted. Related but not covering: 20261102T101202Z-cloudformation-stepfunctions-3-net-new-root-causes-role.md (role failure classified States.Runtime) is fixed and does not carry this check.
Traces: stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task.

## Evidence
stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task, verdict mismatch.
Setup (from the check): a Lambda function; an execution role trusting states.amazonaws.com with Condition ArnLike aws:SourceArn = arn:aws:states:<region>:111122223333:stateMachine:* (a foreign account) and a policy allowing lambda:InvokeFunction; a STANDARD state machine with a Task state invoking that function; StartExecution, then poll DescribeExecution to a terminal status.
Expected: status FAILED, error "States.TaskFailed", cause 'The principal states.amazonaws.com is not authorized to assume the provided role. (role: arn:aws:iam::<account>:role/<role-name>)'.
Actual (AssertionError from the check): cause 'Neither the global service principal states.amazonaws.com, nor the regional one states.us-west-2.amazonaws.com, is not authorized to assume the provided role arn:aws:iam::<account>:role/<role-name>.'
Status and error match; only the cause text diverges. Fixed text in the requirement: "The principal states.amazonaws.com is not authorized to assume the provided role. (role: " ... ")". The only variable part is the role ARN, taken from the state machine's roleArn (the account id and generated role name in the recorded strings are per-run values); the region named in the emulator's text is likewise not part of any AWS message.

## Steps to Reproduce
1. lambda:CreateFunction (any handler).
2. iam:CreateRole with trust {Effect: Allow, Principal: {Service: states.amazonaws.com}, Action: sts:AssumeRole, Condition: {ArnLike: {"aws:SourceArn": "arn:aws:states:<region>:111122223333:stateMachine:*"}}} and an inline policy allowing lambda:InvokeFunction on the function.
3. sfn:CreateStateMachine(type=STANDARD, roleArn=<that role>, definition with one Task state whose Resource is the function ARN).
4. sfn:StartExecution, then poll sfn:DescribeExecution until status != RUNNING.
5. Expected: status FAILED, error "States.TaskFailed", cause "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <role ARN>)". Observed: the "Neither the global service principal ..., nor the regional one ..., is not authorized to assume the provided role <arn>." wording.
6. Control: the same machine under a role whose aws:SourceArn names this account's state machines must SUCCEED.
