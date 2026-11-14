Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged
Issue-Key: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged|run#36|Timeout|raise AssertionError(
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# Stage-scoped lambda:InvokeFunction permission check never converges on a deployed REST stage

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: a REST stage whose backing function's resource policy is scoped with SourceArn to one stage never produces the observable outcome for the other stage, and the check is killed at the harness deadline.

AWS: recorded behavior only. With a lambda:InvokeFunction permission whose SourceArn names only `.../<apiId>/<stageA>/GET/<resource>`, a request served through stageA is answered 200 and a request served through stageB is answered 500 with an API_CONFIGURATION_ERROR execution log line naming the denied invocation. The recording shows the check driving both stages and then reading the execution log.

Emulator: the check never reaches a verdict — the replay detail is `Error: Check timed out after 180.0s`. Neither leg produces the status pair nor the execution-log line the recording shows, so the polling loop exhausts the harness deadline.

Why: a client relying on per-stage SourceArn scoping to fence production from a test stage gets no signal at all — neither the allow on the granted stage nor the refusal on the ungranted one — so it cannot verify its own permission boundary and any automation that waits for the refusal hangs.

Fix: the execute-api stage request path — SourceArn matching when evaluating the target function's resource policy per stage/method, and the execution-log emission that must record the configuration error.

Traces: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged.

## Evidence
Expected: granted stage answers 200; ungranted stage answers 500 and its execution log records the configuration error naming the denied Lambda invocation.
Actual: `Error: Check timed out after 180.0s` — no verdict for either stage.

## Steps to Reproduce
1. Create a REST API with a Lambda proxy integration and deploy it to two stages.
2. Add a lambda:InvokeFunction permission for apigateway.amazonaws.com with SourceArn scoped to stage A only.
3. Send a request through stage A (expect 200) and through stage B (expect 500 + execution log line).
4. Replay apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged: it runs to the 180 s deadline without a verdict.
