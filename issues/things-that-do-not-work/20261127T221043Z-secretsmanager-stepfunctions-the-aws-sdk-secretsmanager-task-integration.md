Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager__stepfunctions.sdk-tasks-read-the-current-value-and-stage-a-derived-one-as-awscurrent secretsmanager__stepfunctions.sdk-task-on-a-missing-secret-rejected-and-caught-by-its-service-error-name secretsmanager__stepfunctions.sdk-task-read-of-a-secret-marked-for-deletion-rejected-until-restored
Issue-Key: secretsmanager__stepfunctions.sdk-tasks-read-the-current-value-and-stage-a-derived-one-as-awscurrent|run#14|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: secretsmanager__stepfunctions.sdk-task-on-a-missing-secret-rejected-and-caught-by-its-service-error-name|_execute#20|AssertionError|assert out["status"] == "SUCCEEDED", (
Issue-Key: secretsmanager__stepfunctions.sdk-task-read-of-a-secret-marked-for-deletion-rejected-until-restored|run#17|Timeout|time.sleep(min(2 ** attempt, 10)) # IAM propagation exposes nothing to poll
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# secretsmanager__stepfunctions: the aws-sdk:secretsmanager Task integration is not dispatched at all, so every SDK-integration secret read fails with States.Runtime

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Task whose Resource is 'arn:aws:states:::aws-sdk:secretsmanager:<operation>' is never dispatched; the execution fails with the emulator's own States.Runtime sentence.
AWS: the AWS SDK integration calls the named Secrets Manager operation and returns its output members in the state result — GetSecretValue returns SecretString/VersionId, PutSecretValue stages a new version under AWSCURRENT — and the execution SUCCEEDs. When the secret does not exist the task fails with the service's own error name 'SecretsManager.ResourceNotFoundException' (so a Catch naming that error catches it), and reading a secret scheduled for deletion fails with 'SecretsManager.InvalidRequestException' until RestoreSecret is called; those error names and the service's messages are the requirement.
Emulator: the execution ends FAILED with error 'States.Runtime' and cause "state 'ReadCurrent': 'arn:aws:states:::aws-sdk:secretsmanager:getSecretValue' names an integration with 'secretsmanager', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it". The negative cases cannot be distinguished at all (one check reports States.Runtime where every branch should have succeeded, and the deletion-window check never settles inside its budget).
Why: no workflow can read or write a secret through the SDK integration, and because the failure is States.Runtime rather than 'SecretsManager.<Error>', a Catch/Retry keyed on the service error name never fires — error handling silently changes shape.
Fix: the aws-sdk Task integration dispatcher in the Step Functions execution engine — register Secrets Manager and map its refusals onto '<Service>.<ErrorCode>' task errors.
Traces: secretsmanager__stepfunctions.sdk-tasks-read-the-current-value-and-stage-a-derived-one-as-awscurrent, secretsmanager__stepfunctions.sdk-task-on-a-missing-secret-rejected-and-caught-by-its-service-error-name, secretsmanager__stepfunctions.sdk-task-read-of-a-secret-marked-for-deletion-rejected-until-restored.

## Evidence
"execution ended 'FAILED': error='States.Runtime' cause=\"state 'ReadCurrent': 'arn:aws:states:::aws-sdk:secretsmanager:getSecretValue' names an integration with 'secretsmanager', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it\""; "every branch ends in success, yet the execution ended 'FAILED': error='States.Runtime'"; and "Error: Check timed out after 60.0s" for the deletion-window case.

## Steps to Reproduce
1. Create a secret and a state machine with Tasks on 'arn:aws:states:::aws-sdk:secretsmanager:getSecretValue' and ':putSecretValue'.
2. Start an execution: it fails with States.Runtime instead of returning the secret value / staging a new AWSCURRENT version.
3. Repeat against a missing secret and a secret scheduled for deletion: the service error names SecretsManager.ResourceNotFoundException / SecretsManager.InvalidRequestException never appear, so Catch rules naming them do not fire.
4. Or replay the check ids in trace_ids.
