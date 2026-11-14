Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.lambda-apigateway-get-rest-apis-positive apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-api-id-negative
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-get-rest-apis-positive|run#9|AssertionError|assert payload["success"] is True, payload
Issue-Key: apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-api-id-negative|run#9|AssertionError|assert good_payload["success"] is True, good_payload
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# apigateway__dynamodb__lambda__logs: in-function API Gateway SDK calls report failure with no error code or message

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestApis

## Description
One root cause behind 2 failing checks.

Cause 1 of 1: an in-function API Gateway SDK call neither succeeds nor is refused with an AWS error code.
AWS: a handler calling GetRestApis or GetDeployment returns 200 with the modelled output; against a rest API id that does not exist AWS refuses with HTTP 404 and error code `NotFoundException`, message `Invalid REST API identifier specified` (fixed text; the identifier comes from the request). With no apigateway grant the refusal is HTTP 403 `AccessDeniedException` with the standard `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: <action> on resource: <arn> because no identity-based policy allows the <action> action` message.
Emulator: both checks observe `{'success': False, 'code': None}` / `{'success': False, 'code': None, 'message': None}` — a failure with no code and no message. The already-filed siblings in this same service show the mechanism verbatim: the handler raises errorType `UnknownServiceError`, errorMessage `Unknown service: 'apigateway'. Valid service names are: sqs, events, eventbridge, dynamodb`, so the in-process runtime's bundled SDK has no API Gateway service model and the client can never be built.
Why: a client cannot distinguish "API Gateway said no" from "the SDK could not be constructed". Negative-path code keyed on NotFoundException or AccessDeniedException never fires, and a function that automates API Gateway from inside Lambda cannot work at all.
Fix: the in-process Lambda Python runtime's bundled AWS SDK service-model catalog and its client factory; and the error-surfacing path that must put the service's own error code and message into the handler's exception. Related open issue 20261105T111951Z-apigateway-dynamodb-lambda-logs-the-lambda-python-runtime.md names the absent SDK for other checks of this service but does not carry these two replay keys.
Traces: the two failing checks listed in trace_ids, under `### apigateway__dynamodb__lambda__logs`.

## Evidence
Expected (AWS): lambda-apigateway-get-rest-apis-positive -> handler returns the REST API list; lambda-apigateway-wrong-api-id-negative -> handler sees HTTP 404 NotFoundException with message `Invalid REST API identifier specified`.

Actual (emulator):
- apigateway__dynamodb__lambda__logs.lambda-apigateway-get-rest-apis-positive -> AssertionError: {'success': False, 'code': None}
- apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-api-id-negative -> AssertionError: {'success': False, 'code': None, 'message': None}

Same-service already-filed sibling shows the cause: {'errorMessage': "Unknown service: 'apigateway'. Valid service names are: sqs, events, eventbridge, dynamodb", 'errorType': 'UnknownServiceError'}.

## Steps to Reproduce
1. Create a REST API plus a Lambda whose handler calls boto3.client('apigateway').get_rest_apis().
2. Invoke it -> expected 200 and the item list; observed failure with code None.
3. Invoke a handler calling get_deployment with a nonexistent restApiId -> expected NotFoundException `Invalid REST API identifier specified`; observed failure with code None and message None.
