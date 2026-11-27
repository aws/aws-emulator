Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.rotation-function-event-carries-the-rotation-token
Issue-Key: lambda__logs__secretsmanager.rotation-function-event-carries-the-rotation-token|run#13|AssertionError|assert not missing, f"invocations without a RotationToken: {missing!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# secretsmanager: rotation-function invocations omit the RotationToken field, so a rotation Lambda cannot authorise its own GetSecretValue calls

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1: the event Secrets Manager sends to a rotation function carries no `RotationToken`.

AWS: each of the four rotation-step invocations receives an event of `{"Step": <createSecret|setSecret|testSecret|finishSecret>, "SecretId": <secret ARN>, "ClientRequestToken": <version id>, "RotationToken": <token>}`. `RotationToken` is the short-lived credential the rotation function passes back on `GetSecretValue`/`PutSecretValue` when the secret's resource policy restricts access to the rotation identity; it is present on every step invocation. Recorded behavior only for the event envelope (the Smithy model describes `RotateSecret`'s request/response, not the Lambda event). Fixed parts of the envelope are the four key names; `SecretId`, `ClientRequestToken` and `RotationToken` are per-rotation values.

Emulator: all four invocations arrive as `{'ClientRequestToken': <uuid>, 'SecretId': <secret ARN>, 'Step': <step>}` — the `RotationToken` key is absent from every step.

Why: a rotation function written against AWS reads `event["RotationToken"]` and fails with a KeyError, or calls GetSecretValue without the token and is denied by the secret's resource policy. Rotation therefore cannot be exercised at all for any secret that uses the documented token-scoped pattern.

Fix: the RotateSecret handler's rotation-step invoker — mint a rotation token for the rotation and include `RotationToken` in each step's event payload (and accept it on GetSecretValue/PutSecretValue).

Traces: lambda__logs__secretsmanager.rotation-function-event-carries-the-rotation-token.

## Evidence
"invocations without a RotationToken: [{'ClientRequestToken': 'aee3224d-...', 'SecretId': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:<name>-YIJNyM', 'Step': 'createSecret'}, {... 'Step': 'setSecret'}, {... 'Step': 'testSecret'}, {... 'Step': 'finishSecret'}]" — all four steps lack the key.

## Steps to Reproduce
1. Create a secret and a rotation Lambda that logs the whole event it receives.
2. Call RotateSecret and read the function's log group — each of the four step events contains only ClientRequestToken, SecretId and Step; RotationToken is missing.
