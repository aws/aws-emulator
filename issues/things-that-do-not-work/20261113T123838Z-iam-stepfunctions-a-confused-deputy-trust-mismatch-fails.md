Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied
Issue-Key: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied|run#16|AssertionError|assert bad["cause"] == want_cause, (
Issue-Commit: 3bfb7cd772781c7f8fd33f0bb610479e140a70f3

# iam__stepfunctions: a confused-deputy trust mismatch fails the execution with emulator-authored cause text ("Neither the global service principal ..., nor the regional one ...") instead of AWS's "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <arn>)"

> _Note: this issue was identified against the code base at commit `3bfb7cd772781c7f8fd33f0bb610479e140a70f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Cause 1 of 1: an execution whose role carries a confused-deputy guard (aws:SourceArn / aws:SourceAccount Condition) that does not match the starting state machine fails with an emulator-invented cause instead of AWS's assume-role-denied sentence.
Failing check id: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied.
AWS: recorded behaviour only (Smithy models the cause member as SensitiveCause, @length 0..32768, on DescribeExecution and GetExecutionHistory; services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json — it does not state the text). A role trusting Service states.amazonaws.com under a Condition that the starting state machine does not satisfy cannot be assumed, so the Task that calls out through the role fails and the execution's cause names exactly the un-assumable role in a fixed sentence: "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <role ARN>)". Only <role ARN> varies and it is the roleArn configured on the state machine (path included); everything else, including the terminating period inside the sentence and the parenthesised "(role: ...)" suffix, is fixed.
Emulator: the execution does fail, but the cause is "Neither the global service principal states.amazonaws.com, nor the regional one states.<region>.amazonaws.com, is not authorized to assume the provided role <role ARN>." — wording AWS never emits: it enumerates a second, regional service principal that is an emulator implementation detail, is ungrammatical, and drops the "(role: <arn>)" form so the role appears inline followed by a period.
Why: this check exists because aws:SourceAccount/aws:SourceArn on a service trust policy is the documented confused-deputy guard; a client verifying that the guard bites must be able to read back which role was refused, and anything matching or parsing AWS's sentence (catchers on the cause, log alarms, support runbooks) fails. The cause is also the only place the refused role is named, and the emulator's form cannot be parsed by the "(role: ...)" convention.
Fix: the execution runtime's assume-role failure path — the single cause-text builder shared by the execution's cause, the TaskFailed history event and the StartSyncExecution response must emit AWS's fixed sentence with only the role ARN substituted. The same defect surfaces on the plain-stepfunctions surface as stepfunctions.execution-role-trust-source-arn-naming-another-account-blocks-task (filed separately because that check is reported under a different service heading); no open issue carries this check's key.
Traces: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied.

## Evidence
iam__stepfunctions.confused-deputy-source-arn-mismatch-denied, verdict mismatch.
Setup (from the check): iam:CreateRole at Path /sim2real/ with trust {Effect: Allow, Principal: {Service: states.amazonaws.com}, Action: sts:AssumeRole, Condition: <a SourceArn/SourceAccount guard that does not match the machine>} plus an inline policy allowing sts:GetCallerIdentity; a state machine whose single Task is Resource arn:aws:states:::aws-sdk:sts:getCallerIdentity (so the role is genuinely assumed — a Pass-only definition would never need it).
Expected cause: 'The principal states.amazonaws.com is not authorized to assume the provided role. (role: arn:aws:iam::<account>:role/sim2real/<role-name>)'.
Actual cause: 'Neither the global service principal states.amazonaws.com, nor the regional one states.us-west-2.amazonaws.com, is not authorized to assume the provided role arn:aws:iam::<account>:role/sim2real/<role-name>.'
The failure itself is produced; only the cause text diverges. Fixed text required: "The principal states.amazonaws.com is not authorized to assume the provided role. (role: " <role ARN> ")". The account id, role path and generated role name in both strings are per-run values from the request; the region in the emulator's text belongs to no AWS message.

## Steps to Reproduce
1. iam:CreateRole(Path="/sim2real/", AssumeRolePolicyDocument allowing sts:AssumeRole to Service states.amazonaws.com under a Condition whose aws:SourceArn (or aws:SourceAccount) does not match the state machine that will be started); iam:PutRolePolicy allowing sts:GetCallerIdentity on *.
2. sfn:CreateStateMachine(roleArn=<that role>, definition={"StartAt":"WhoAmI","States":{"WhoAmI":{"Type":"Task","Resource":"arn:aws:states:::aws-sdk:sts:getCallerIdentity","Parameters":{},"End":true}}}).
3. sfn:StartExecution and poll sfn:DescribeExecution to a terminal status.
4. Read cause — expected "The principal states.amazonaws.com is not authorized to assume the provided role. (role: <role ARN>)"; observed the "Neither the global service principal ..., nor the regional one ..., is not authorized to assume the provided role <arn>." wording.
5. Control: the same role with a matching aws:SourceArn must let the execution SUCCEED.
