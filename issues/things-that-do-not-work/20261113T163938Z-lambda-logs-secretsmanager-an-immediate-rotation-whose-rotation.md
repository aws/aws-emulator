Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves
Issue-Key: lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves|run#20|Timeout|time.sleep(5.0)
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# lambda__logs__secretsmanager: an immediate rotation whose rotation function raises in testSecret never runs the step protocol, so the 'untested candidate is never promoted' contract is unobservable

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1: RotateSecret(RotateImmediately=true) against a rotation Lambda that raises in testSecret never drives the four-step rotation protocol, so the scenario stalls and is killed at the runner cap with nothing graded. Failing check: lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves.

AWS: Smithy (secretsmanager) RotateSecret takes SecretId (required), RotationLambdaARN, RotationRules and RotateImmediately (boolean, no default), returns ARN/Name/VersionId, and its modeled errors are InternalServiceError / InvalidParameterException / InvalidRequestException / ResourceNotFoundException. Recorded behavior: with RotateImmediately=true Secrets Manager invokes the configured rotation function once per step with the event {SecretId, ClientRequestToken, Step}, in the order createSecret, setSecret, testSecret, finishSecret, and each invocation's stdout lands in that function's CloudWatch Logs group /aws/lambda/<function-name>. When the testSecret invocation raises, the protocol STOPS there: finishSecret is never invoked, so UpdateSecretVersionStage is never called and AWSCURRENT keeps pointing at the pre-rotation version — GetSecretValue still returns the original VersionId and the original SecretString, DescribeSecret's VersionIdsToStages still lists that version as the sole ['AWSCURRENT'] holder, exactly one version carries AWSCURRENT, and the untested candidate keeps only AWSPENDING. The identical function with testSecret succeeding does reach finishSecret and promotes.

Emulator: the scenario makes no progress and the check is terminated at the runner's cap with 'Check timed out after 60.0s' — no step marker is ever observed in the rotation function's log group, so the rotation function is never seen to be invoked for any step and neither the negative (testSecret raised, nothing promoted) nor its positive control can be reached.

Why: 'an untested credential is never made current' is the whole safety property of rotation. A client cannot observe it, cannot see which step its function failed at (the step markers are the only diagnostic), and cannot tell a rotation that stopped safely at testSecret from one that never started at all.

Fix: the RotateSecret handler's rotation driver in the Secrets Manager crate — it must invoke the configured RotationLambdaARN once per step with {SecretId, ClientRequestToken, Step} for createSecret/setSecret/testSecret/finishSecret, abort the protocol when a step invocation errors, and leave the AWSCURRENT/AWSPENDING staging map untouched when testSecret fails — plus the Lambda invocation path that must deliver those invocations' output to /aws/lambda/<function-name>. Related open issue 20261108T035337Z-secretsmanager-5-net-new-causes-a-deferred-rotateimmediately.md records the RotateImmediately=false deferred test invocation never happening; it does not carry this check's replay key and this check exercises the RotateImmediately=true protocol.

Traces: the failing check id above.

## Evidence
Expected (AWS): the rotation function's log group shows the steps createSecret, setSecret, testSecret and NOT finishSecret; GetSecretValue(SecretId) still returns VersionId = the version created by CreateSecret and SecretString = the original value; DescribeSecret's VersionIdsToStages maps that version to exactly ['AWSCURRENT'], no AWSPENDING version carries AWSCURRENT, and exactly one version carries AWSCURRENT. The twin function differing only in one environment variable (testSecret succeeds) reaches finishSecret and promotes.

Actual (emulator): 'Error: Check timed out after 60.0s'. The check never reaches a graded assertion: the poll for the failing function's testSecret step marker never observes any step, i.e. no rotation-step invocation of the Lambda is ever visible.

## Steps to Reproduce
1. CreateSecret with a known SecretString; keep its VersionId.
2. CreateFunction twice from one source (a handler that prints its Step and, for createSecret, PutSecretValue with VersionStages ['AWSPENDING']; for testSecret either raises or validates the pending value) differing only in the FAIL_ON_TEST environment variable; share one execution role; AddPermission for Principal secretsmanager.amazonaws.com on both and confirm GetPolicy lists that service principal.
3. RotateSecret(SecretId=<arn>, RotationLambdaARN=<failing function>, RotationRules={'AutomaticallyAfterDays': 30}, RotateImmediately=True).
4. Poll the failing function's /aws/lambda/<name> log group for its step markers; once testSecret appears, keep polling for a grace window asserting finishSecret never appears and GetSecretValue's VersionId never changes; then read DescribeSecret's VersionIdsToStages.
5. CancelRotateSecret and repeat with the twin function as the positive control.
Expected: steps createSecret, setSecret, testSecret only, and an untouched AWSCURRENT. Actual: no step is ever logged; the scenario never progresses and the check is killed at the runner cap.
Replay: lambda__logs__secretsmanager.rotation-with-a-failing-testsecret-step-rejected-and-awscurrent-never-moves.
