Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added
Issue-Key: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added|_run_until#16|Timeout|out = sfn.start_sync_execution(
Issue-Commit: 96996ff548d771d992d512709b10ef647dc44007

# iam__secretsmanager__stepfunctions: the denied-then-granted GetSecretValue SDK-integration lifecycle still reaches no verdict — an EXPRESS execution of an aws-sdk:secretsmanager state machine never yields the expected output

> _Note: this issue was identified against the code base at commit `96996ff548d771d992d512709b10ef647dc44007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Cause 1 of 1: an EXPRESS state machine whose states are the Secrets Manager AWS SDK integrations (arn:aws:states:::aws-sdk:secretsmanager:describeSecret, then :getSecretValue) never produces the output those tasks owe, so neither the denied read nor the granted read is observable through StartSyncExecution.
Failing check id for this cause: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added.

  AWS: an AWS SDK integration task runs the named Secrets Manager API under the state machine's execution role and its result becomes the state's output (Step Functions "AWS SDK service integrations": Resource arn:aws:states:::aws-sdk:<service>:<apiAction>). For an EXPRESS machine, StartSyncExecution answers status SUCCEEDED with the final state's JSON output inline. So, with an execution role granted only secretsmanager:DescribeSecret on the secret: (a) the Describe task SUCCEEDS and its ResultSelector can read $.Name, i.e. the output carries the secret's name; (b) the getSecretValue task is REFUSED by Secrets Manager with HTTP 400 and error code AccessDeniedException, which the task surfaces to the ASL Catch as Error 'SecretsManager.SecretsManagerException' with a Cause whose message is Secrets Manager's own authorization sentence — fixed text: "User: " … " is not authorized to perform: secretsmanager:GetSecretValue on resource: " … " because no identity-based policy allows the secretsmanager:GetSecretValue action", where the variable parts are the caller's assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>) and the secret's ARN, both taken from the request and the emulator's state, followed by the SDK trailer "(Service: …; Status Code: 400; Request ID: …)"; (c) after secretsmanager:GetSecretValue is added to the same role, the identical StartSyncExecution SUCCEEDS and the Read state's output carries SecretString. Recorded behavior only for the end-to-end shape; the error class and status are Secrets Manager's modeled authorization refusal.

  Emulator: the check reaches no verdict at all — it ends `Error: Check timed out after 60.0s`. Its first loop calls StartSyncExecution repeatedly (10 attempts, backoff 1→10 s, ~75 s of sleeps in total) and returns only when a SUCCEEDED execution's output contains the key the Describe state writes ($.meta); the budget being exhausted in that loop means no execution produced that output, so the granted DescribeSecret leg never succeeds, and the denied GetSecretValue cause and the repaired read after the grant are never reached.

  Why: a client cannot use the Secrets Manager AWS SDK integration at all: a workflow that should describe a secret and then either fail fast with an authorization cause or read the value hangs the caller on a synchronous execution that yields nothing usable. The denied→granted transition this pattern exists to express, and the AccessDenied sentence an operator reads to find the missing grant, are both unobservable.

  Fix: the Step Functions AWS SDK integration task runner for aws-sdk:secretsmanager (describeSecret and getSecretValue) — assume the state machine's execution role, make the call, map the answer into the state's result (honouring ResultSelector/ResultPath) and drive the execution to a terminal status that StartSyncExecution reports, mapping a Secrets Manager authorization refusal to the SecretsManager.SecretsManagerException task error with Secrets Manager's own message as the Cause.

  Traces: iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added.

Relation to already-open issues (this replay did not report the check as owned by either): 20261127T173710Z-iam-secretsmanager-stepfunctions-stepfunctions-an-sdk-integration-task-reading-a-secret.md describes the same check timing out, and 20261127T221043Z-secretsmanager-stepfunctions-the-aws-sdk-secretsmanager-task-integration.md reports the aws-sdk:secretsmanager integration not being dispatched at all. Both are still open; this filing records that the check continues to reach no verdict in the current checkout.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Service heading `### iam__secretsmanager__stepfunctions`: 0/1 passed.

[mismatch] iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added
  Error: Check timed out after 60.0s

What the check does (GROUP `permissions`, SERVICES [iam, secretsmanager, stepfunctions]):
  - CreateSecret with SecretString 'guarded-value'.
  - CreateRole trusted by states.amazonaws.com, PutRolePolicy 'workflow-secret-access' granting ONLY secretsmanager:DescribeSecret on the secret ARN.
  - CreateStateMachine type EXPRESS with states: Describe -> Resource arn:aws:states:::aws-sdk:secretsmanager:describeSecret, Parameters {"SecretId.$": "$.secret"}, ResultSelector {"name.$": "$.Name"}, ResultPath $.meta; Read -> Resource arn:aws:states:::aws-sdk:secretsmanager:getSecretValue, ResultSelector {"value.$": "$.SecretString"}, ResultPath $.read, Catch States.ALL -> ResultPath $.caught.
  - _run_until(): up to 10 StartSyncExecution calls with input {"secret": <arn>}, returning as soon as a SUCCEEDED execution's output contains 'meta'.

Expected first leg: a SUCCEEDED execution whose output has meta.name == the secret name, no 'read' key, and caught.Error == 'SecretsManager.SecretsManagerException' with Cause starting "User: arn:aws:sts::<account>:assumed-role/<role>/" and ending " is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action".
Expected second leg, after PutRolePolicy adds secretsmanager:GetSecretValue: the identical StartSyncExecution SUCCEEDS with read.value == 'guarded-value' and no 'caught' key.
Actual: the retry loop never sees an output carrying 'meta', so the 60 s budget is consumed and no leg is graded.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. CreateSecret (SecretString 'guarded-value').
3. CreateRole trusted by states.amazonaws.com and PutRolePolicy granting only secretsmanager:DescribeSecret on that secret ARN.
4. CreateStateMachine type=EXPRESS whose first state is Resource arn:aws:states:::aws-sdk:secretsmanager:describeSecret (Parameters {"SecretId.$": "$.secret"}, ResultSelector {"name.$": "$.Name"}, ResultPath "$.meta") and whose second state is Resource arn:aws:states:::aws-sdk:secretsmanager:getSecretValue with a States.ALL Catch into "$.caught".
5. StartSyncExecution with input {"secret": "<secret ARN>"}: expect status SUCCEEDED with output.meta.name == the secret name and output.caught.Error == 'SecretsManager.SecretsManagerException' whose Cause is the Secrets Manager AccessDenied sentence naming the assumed-role session and the secret ARN. Observe that no such output is produced.
6. PutRolePolicy adding secretsmanager:GetSecretValue and repeat step 5: expect SUCCEEDED with output.read.value == 'guarded-value' and no 'caught'.
7. Replay iam__secretsmanager__stepfunctions.sdk-task-read-denied-without-the-getsecretvalue-grant-until-it-is-added and observe 'Check timed out after 60.0s'.
