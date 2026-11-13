Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied
Issue-Key: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied|_invoke_lambda#8|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: f50d8ce9e37ffa71976cdbe00198995770403bcc

# lambda__secretsmanager__sqs: a granted in-function GetSecretValue never returns the secret, so the AccessDenied-after-revocation check never reaches its negative arm

> _Note: this issue was identified against the code base at commit `f50d8ce9e37ffa71976cdbe00198995770403bcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
One net-new root cause.

Cause 1 of 1: a Lambda handler whose execution role is granted secretsmanager:GetSecretValue on the secret named in its environment never returns the secret value, so the check's positive control never passes and the revocation arm — which pins the exact AccessDeniedException message — is never reached; the check polls to the harness's 60 s cap and is killed. Failing check id: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied.

AWS: GetSecretValue(SecretId=<ARN>) returns ARN, Name, VersionId, VersionStages and SecretString with the stored value (Smithy: secretsmanager GetSecretValue; SecretString is sensitive, length 1..65536; modeled errors DecryptionFailure, InternalServiceError, InvalidParameterException, InvalidRequestException, ResourceNotFoundException, all 400 except InternalServiceError 500). Authorization is enforced by IAM, not by the operation's modeled errors: once the identity-based grant is removed, the identical call is refused AccessDeniedException and botocore surfaces the message "An error occurred (AccessDeniedException) when calling the GetSecretValue operation: User: <principal> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action". Everything outside the angle brackets is fixed text; <principal> is the function's assumed-role session arn:aws:sts::<account>:assumed-role/<role name>/<function name> and <secret ARN> is the SecretId from the request. Restoring the grant must make the identical call succeed again.

Emulator: the check never completes. The replay reports only `Error: Check timed out after 60.0s`. The positive control invokes the function every 3 s for up to 60 s waiting for the handler payload {"ok": true, "secret": "top-secret-value"}; that never arrives, so the role's policy is never swapped for the logs-only baseline and the AccessDeniedException message is never exercised. No status, no error code and no secret value is observed.

Why: a client cannot read its own secret from a function that AWS would authorize, and — because the positive path never works — cannot distinguish "denied by IAM" from "wiring broken". A caller relying on the documented refusal text to detect a missing grant gets no refusal at all, and secret-backed configuration simply never loads.

Fix: the in-function AWS SDK path that lets a handler call Secrets Manager and the IAM authorization applied to that call, plus the GetSecretValue handler/serializer that must return SecretString for a granted principal and refuse an un-granted one with AccessDeniedException and AWS's message text. The open issue 20261106T153504Z-lambda-secretsmanager-sqs-a-function-whose-role-grants.md describes a function whose role grants secretsmanager:GetSecretValue never succeeding in reading it, and 20261105T081139Z-lambda-secretsmanager-sqs-the-in-process-lambda-python.md and 20261106T134622Z-lambda-secretsmanager-sqs-the-in-lambda-sdk-reports.md cover the in-process runtime's SDK; none of them carries this check's replay key.

Traces: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied (verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: with the role granting secretsmanager:GetSecretValue on the secret, the handler returns {"ok": true, "secret": "<the stored SecretString>"}; after the inline policy is replaced by a logs-only baseline the identical invoke returns {"ok": false, "error": "ClientError", "message": "An error occurred (AccessDeniedException) when calling the GetSecretValue operation: User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action"}; restoring the grant makes the call succeed again.
Actual: the granted invoke never returns ok=true, so the check never leaves its positive control; it is terminated by the harness with `Error: Check timed out after 60.0s` and the AccessDeniedException message is never produced.

## Steps to Reproduce
1. CreateSecret with SecretString "top-secret-value".
2. Create a python Lambda function with env SECRET_ARN=<secret ARN> and a role whose inline policy grants logs:* plus secretsmanager:GetSecretValue on that ARN; the handler calls get_secret_value(SecretId=os.environ["SECRET_ARN"]) and returns {"ok": True, "secret": ...} or the caught exception's type and message.
3. Invoke the function repeatedly and observe that {"ok": true} never arrives.
4. (Unreached) PutRolePolicy replacing the inline policy with the logs-only baseline and invoke again, expecting the AccessDeniedException message above, then restore the grant and expect success.
