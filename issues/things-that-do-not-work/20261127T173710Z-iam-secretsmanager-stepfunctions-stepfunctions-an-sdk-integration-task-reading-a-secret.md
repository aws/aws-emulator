Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added
Issue-Key: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added|_run_until#16|Timeout|time.sleep(min(2 ** attempt, 10)) # IAM propagation exposes nothing to poll
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# stepfunctions: an SDK-integration task reading a secret never settles — the denied-then-granted GetSecretValue lifecycle exhausts the check budget

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
=== Cause 1 of 1: the GetSecretValue SDK-integration task lifecycle never reaches a verdict ===
AWS: recorded behavior only for the end-to-end shape. A state machine whose state is the Secrets Manager SDK integration (`arn:aws:states:::aws-sdk:secretsmanager:getSecretValue`) runs under the state machine's execution role. With no `secretsmanager:GetSecretValue` grant the execution FAILS with the Step Functions SDK-integration error for a denied call (`SecretsManager.AccessDeniedException` cause carrying Secrets Manager's own `AccessDenied` sentence naming the execution role and the secret ARN); once the grant is added to the role, the identical `StartExecution` SUCCEEDS and the task output carries the secret. Both executions reach a terminal status promptly — the check polls only to absorb IAM propagation.
Emulator: the check never reaches a verdict at all — it ends `Check timed out after 60.0s`, so neither the denied execution's failure cause nor the granted execution's success is ever observed.
Why: a client cannot use the Secrets Manager SDK integration: an execution that should fail fast with an authorization cause, and one that should succeed once granted, both leave the caller polling a non-terminal execution. The denied/granted transition that this pattern exists to express is unobservable.
Fix: the Step Functions SDK-integration task runner for `aws-sdk:secretsmanager:getSecretValue` — assume the execution role, make the call, and drive the execution to a terminal status carrying either the task result or the denied-call failure cause.
Traces: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added.

## Evidence
From one replay of the behavioral suite (service heading `### iam__secretsmanager__stepfunctions`, 0/1 passed).

[mismatch] iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added
  Error: Check timed out after 60.0s

The check (GROUP `permissions`, SERVICES [iam, secretsmanager, stepfunctions]) creates the secret, the execution role and the state machine, starts an execution WITHOUT the `secretsmanager:GetSecretValue` grant, expects a terminal FAILED execution carrying the denied-call cause, then adds the `workflow-secret-access` policy to the role and expects the identical StartExecution to SUCCEED. No leg reaches a terminal state inside the budget.

## Steps to Reproduce
1. CreateSecret in Secrets Manager.
2. Create an IAM role trusted by states.amazonaws.com with NO `secretsmanager:GetSecretValue` grant, and CreateStateMachine whose single task is the SDK integration `arn:aws:states:::aws-sdk:secretsmanager:getSecretValue` on that secret's ARN.
3. StartExecution and poll DescribeExecution: it must reach a terminal FAILED status whose cause is the denied Secrets Manager call (AccessDenied naming the execution role and the secret ARN).
4. PutRolePolicy adding `secretsmanager:GetSecretValue` on that secret, then StartExecution again: it must reach SUCCEEDED with the secret in the task output.
5. Observed: neither execution reaches a terminal status; the check exhausts its 60 s budget.
