Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__stepfunctions.http-api-startexecution-reused-execution-name-rejected-new-name-starts apigatewayv2__stepfunctions.startexecution-subtype-header-mapped-name-replays-idempotently
Issue-Key: apigatewayv2__stepfunctions.http-api-startexecution-reused-execution-name-rejected-new-name-starts|run#29|AssertionError|assert (status, json.loads(text)) == (400, expected_body), (
Issue-Key: apigatewayv2__stepfunctions.startexecution-subtype-header-mapped-name-replays-idempotently|run#32|AssertionError|assert refusal.get("message") == f"Execution Already Exists: '{expected_arn}'", (
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# apigatewayv2__stepfunctions: the ExecutionAlreadyExists refusal is serialized with a bare __type and a capitalised Message key, so clients cannot read it

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: Step Functions' ExecutionAlreadyExists refusal, as served through an HTTP API AWS_PROXY StartExecution integration, carries the wrong awsJson error envelope.

AWS: StartExecution reusing an existing execution's name with a different input is refused HTTP 400 with the awsJson error envelope {"__type": "com.amazonaws.swf.service.v2.model#ExecutionAlreadyExists", "message": "Execution Already Exists: '<execution arn>'"} — `__type` is the fully qualified shape id, the message key is lowercase `message`, and the message's fixed text is "Execution Already Exists: " with the execution ARN from the request/state in single quotes. Smithy: com.amazonaws.sfn#ExecutionAlreadyExists, client error, httpError 400, single member `message` (the shape is documented as "The execution has the same name as another execution (but a different input)"; same name AND same input is idempotent and must answer 200 with the original executionArn).

Emulator: the status (400) and the message text are right, but the envelope is {"__type": "ExecutionAlreadyExists", "Message": "Execution Already Exists: '<arn>'"} — `__type` is the bare shape name with no namespace, and the message is under `Message` instead of `message`.

Why: a client that reads the refusal by the documented keys gets nothing: botocore-style parsing of `__type` cannot resolve the modelled error (the namespace is missing) and `message` reads as None, so the error surfaces as an unknown 400 with no message — which is exactly what the second check observes ("the refusal carried message None") after a correct idempotent replay.

Fix: the Step Functions error serializer for the awsJson error envelope (the `__type` shape-id qualification and the `message` member name), which the HTTP API AWS_PROXY/subtype integration relays verbatim.

Traces: the two checks in trace_ids.

## Evidence
Check http-api-startexecution-reused-execution-name-rejected-new-name-starts: "reusing a closed execution's name answered 400 {\"__type\":\"ExecutionAlreadyExists\",\"Message\":\"Execution Already Exists: '<arn>'\"}, expected 400 {'__type': 'com.amazonaws.swf.service.v2.model#ExecutionAlreadyExists', 'message': \"Execution Already Exists: '<arn>'\"}".
Check startexecution-subtype-header-mapped-name-replays-idempotently: assertion `refusal.get("message") == "Execution Already Exists: '<arn>'"` fails with "the refusal carried message None" — the key served is `Message`.

## Steps to Reproduce
1. Create a state machine; create an HTTP API with an AWS_PROXY StartExecution integration (or the StepFunctions-StartExecution subtype with the execution name mapped from a request header) and a $default stage.
2. POST a signed request that starts an execution with name N and input A — 200.
3. POST again with name N and a DIFFERENT input B — the response is 400, but its body is {"__type":"ExecutionAlreadyExists","Message":"Execution Already Exists: '<arn>'"} instead of {"__type":"com.amazonaws.swf.service.v2.model#ExecutionAlreadyExists","message":"Execution Already Exists: '<arn>'"}.
