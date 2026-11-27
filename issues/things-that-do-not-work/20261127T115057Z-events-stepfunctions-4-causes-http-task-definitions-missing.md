Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__stepfunctions.http-task-without-a-connection-reference-rejected events__stepfunctions.http-task-naming-no-connection-rejected-at-create events__stepfunctions.http-task-authentication-without-connection-arn-rejected events__stepfunctions.http-task-invocation-config-without-connection-arn-rejected events__stepfunctions.http-task-with-both-connection-fields-rejected events__stepfunctions.http-task-invocation-config-connection-supplies-api-key events__stepfunctions.http-task-string-body-with-connection-body-parameters-rejected events__stepfunctions.http-task-connection-query-parameter-overrides-task-value events__stepfunctions.http-task-role-without-retrieve-connection-credentials-rejected
Issue-Key: events__stepfunctions.http-task-without-a-connection-reference-rejected|_refusal#21|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: events__stepfunctions.http-task-naming-no-connection-rejected-at-create|_refusal#20|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: events__stepfunctions.http-task-authentication-without-connection-arn-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected the definition to be refused, but it was accepted: {result!r}")
Issue-Key: events__stepfunctions.http-task-invocation-config-without-connection-arn-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected the definition to be refused, but it was accepted: {result!r}")
Issue-Key: events__stepfunctions.http-task-with-both-connection-fields-rejected|_refusal#17|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: events__stepfunctions.http-task-invocation-config-connection-supplies-api-key|run#14|AssertionError|assert served["status"] == "SUCCEEDED", (
Issue-Key: events__stepfunctions.http-task-string-body-with-connection-body-parameters-rejected|run#15|AssertionError|assert sent == {"mode": conn_mode, "origin": conn_origin,
Issue-Key: events__stepfunctions.http-task-connection-query-parameter-overrides-task-value|run#18|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: events__stepfunctions.http-task-role-without-retrieve-connection-credentials-rejected|run#19|AssertionError|assert (got["status"], got.get("error"), got.get("cause")) == (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__stepfunctions: 4 causes — HTTP Task definitions missing/duplicating a connection reference are accepted, connection credentials and InvocationHttpParameters are not applied, and an unauthorized role is not refused

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Cause 1 of 4: a state machine whose HTTP Task does not name exactly one EventBridge connection is accepted. AWS: CreateStateMachine refuses an arn:aws:states:::http:invoke task definition that names no connection (no Authentication.ConnectionArn and no InvocationConfig.ConnectionArn), that supplies an Authentication/InvocationConfig block without a ConnectionArn, or that names BOTH connection fields — the call is refused with a 400 InvalidDefinition whose message names the offending state and field (recorded behavior for the message; Smithy sfn models InvalidDefinition on CreateStateMachine). Emulator: every one of these definitions is accepted, returning a stateMachineArn. Checks: events__stepfunctions.http-task-without-a-connection-reference-rejected, events__stepfunctions.http-task-naming-no-connection-rejected-at-create, events__stepfunctions.http-task-authentication-without-connection-arn-rejected, events__stepfunctions.http-task-invocation-config-without-connection-arn-rejected, events__stepfunctions.http-task-with-both-connection-fields-rejected.

Cause 2 of 4: a connection named through InvocationConfig supplies no credentials. AWS: an HTTP Task whose InvocationConfig.ConnectionArn names a connection with an API-key authorization uses that connection's credentials for the request, so the task SUCCEEDS and the API sees the key header. Recorded behavior only. Emulator: the task ends FAILED with error `States.Runtime` and cause `this task names no 'Authentication', so there is no connection to authorize its request through and nothing was run for it` — only the Authentication form is recognized, so the InvocationConfig connection is ignored and nothing is sent. Checks: events__stepfunctions.http-task-invocation-config-connection-supplies-api-key.

Cause 3 of 4: a connection's InvocationHttpParameters are not merged into the request. AWS: the connection's query-string and body parameters are merged with the task's own — the CONNECTION's value wins on a conflict — so the API receives the merged query string/body; a string body task with connection body parameters is handled per that rule. Recorded behavior only. Emulator: the API received only the task's own values (`{'mode': 'from-task', 'order': {...}}` with the connection's `mode` absent), and the query-parameter variant ends FAILED with `States.Http.StatusCode.500` (an InternalServerErrorException from the invoked endpoint) instead of a successful merged request. Checks: events__stepfunctions.http-task-string-body-with-connection-body-parameters-rejected, events__stepfunctions.http-task-connection-query-parameter-overrides-task-value.

Cause 4 of 4: a role without events:RetrieveConnectionCredentials is allowed to use the connection. AWS: the HTTP Task's execution role must hold events:RetrieveConnectionCredentials (and the secret read) for the connection; without it the task is refused — TestState reports a FAILED status with the authorization error, not SUCCEEDED. Recorded behavior only. Emulator: `TestState answered 'SUCCEEDED', error None, cause None`. Checks: events__stepfunctions.http-task-role-without-retrieve-connection-credentials-rejected.

Why: definitions real Step Functions refuses are deployed here and only fail later; the EventBridge-connection integration that makes HTTP Tasks usable (credentials + connection-level parameters) does not work, so third-party calls go out unauthenticated/unmerged or not at all; and an under-privileged role appears to work, hiding a permission error that will surface in production.
Fix: the sfn definition validator for the http:invoke resource (connection-reference rules) and the HTTP Task execution path's connection handling (resolve Authentication OR InvocationConfig ConnectionArn, apply the connection's credentials and InvocationHttpParameters with the connection winning, and authorize the role's events:RetrieveConnectionCredentials).
Traces: the nine check ids listed per cause above.

## Evidence
Cause 1: `AssertionError: expected the definition to be refused, but it was accepted: 'arn:aws:states:...:stateMachine:<name>'` (five variants)
Cause 2: `AssertionError: the task holding the right key must SUCCEED; got 'FAILED' error='States.Runtime' cause="this task names no 'Authentication', so there is no connection to authorize its request through and nothing was run for it"`
Cause 3: `AssertionError: the API should receive the task body merged with the connection's body parameters, the connection's mode winning; it received {'mode': 'from-task', 'order': {...}}`; and `the HTTP Task execution ended 'FAILED': error='States.Http.StatusCode.500'`
Cause 4: `AssertionError: a role without events:RetrieveConnectionCredentials should be refused the connection; TestState answered 'SUCCEEDED', error None, cause None`

## Steps to Reproduce
1. events CreateConnection (API-key authorization, with InvocationHttpParameters query/body parameters).
2. sfn CreateStateMachine with an arn:aws:states:::http:invoke task: (a) with no connection reference, (b) with Authentication {} lacking ConnectionArn, (c) with InvocationConfig {} lacking ConnectionArn, (d) with both Authentication.ConnectionArn and InvocationConfig.ConnectionArn — each is accepted.
3. StartExecution for a machine naming the connection via InvocationConfig — observe States.Runtime failure.
4. StartExecution for a machine whose task body conflicts with the connection's body parameters — inspect what the endpoint received.
5. TestState with a role lacking events:RetrieveConnectionCredentials — observe SUCCEEDED.
