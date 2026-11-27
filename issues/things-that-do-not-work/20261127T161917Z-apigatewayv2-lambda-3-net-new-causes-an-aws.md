Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.http-api-iam-route-body-altered-after-signing-rejected-403 apigatewayv2__lambda.http-api-v2-event-body-is-the-raw-utf8-string-the-client-sent apigatewayv2__lambda.iam-route-request-signed-for-another-service-rejected
Issue-Key: apigatewayv2__lambda.http-api-iam-route-body-altered-after-signing-rejected-403|run#22|AssertionError|assert status == 403, f"a body altered after signing should be refused 403, got {status}: {text!r}"
Issue-Key: apigatewayv2__lambda.http-api-v2-event-body-is-the-raw-utf8-string-the-client-sent|run#17|AssertionError|assert got["is_base64"] is encoded, (
Issue-Key: apigatewayv2__lambda.iam-route-request-signed-for-another-service-rejected|run#33|AssertionError|assert json.loads(text) == {"probe": control_probe, "accountId": ctx.account}, (
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# apigatewayv2__lambda: 3 net-new causes — an AWS_IAM route serves a request whose body was altered after signing, payload 2.0 never base64-encodes a non-text body, and requestContext.authorizer.iam is absent

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api HTTP API data plane)

## Description
Three net-new root causes on HTTP API routes fronting Lambda. Each is numbered with its own failing check ids; all three must be closed before this issue is submitted.

Cause 1 of 3: an AWS_IAM route does not verify the signed payload, so a body altered after signing is served.
AWS: on a route with AuthorizationType=AWS_IAM the gateway verifies the SigV4 signature over the canonical request, which includes the x-amz-content-sha256 / payload hash; a request whose body was changed after signing fails verification and is refused HTTP 403 with body {"message":"Forbidden"} and never reaches the integration.
Emulator: the altered request is served 200 and the function runs on the tampered body ('{"served_by": "function", "body": "{\"transfer\": 990, \"currency\": \"EUR\"}"}').
Why: SigV4 gives no integrity guarantee on the emulator — a proxy or client can rewrite a signed body and the backend executes it, so any test of request-integrity enforcement passes against AWS and fails to protect here.
Fix: the AWS_IAM authorization path of the execute-api front door — include the payload hash in the canonical request it verifies, and refuse 403 {"message":"Forbidden"} on mismatch. Related open issue 20261110T170829Z-apigatewayv2-9-net-new-causes-aws-iam-routes.md records unsigned requests being served and does not carry this replay key.
Traces: apigatewayv2__lambda.http-api-iam-route-body-altered-after-signing-rejected-403.

Cause 2 of 3: under payload format 2.0 a non-text content type is delivered as plain text with isBase64Encoded false.
AWS: payload format 2.0 passes a body whose content type is text-like (application/json, text/*) through as the exact UTF-8 string with isBase64Encoded false, and base64-encodes every other body — including application/x-www-form-urlencoded — setting isBase64Encoded true; event.body is a str in both cases.
Emulator: for application/x-www-form-urlencoded the event carries the raw text with isBase64Encoded false.
Why: a function that branches on isBase64Encoded (decoding when true) receives double-interpreted data: on AWS it base64-decodes the form body, on the emulator it would decode text that was never encoded, so the same handler behaves differently per environment.
Fix: the payload-2.0 event builder's content-type classification and body encoding in the apigatewayv2 data plane.
Traces: apigatewayv2__lambda.http-api-v2-event-body-is-the-raw-utf8-string-the-client-sent.

Cause 3 of 3: an AWS_IAM route's payload-2.0 event carries no requestContext.authorizer.iam identity.
AWS: when a route is authorized with AWS_IAM, the payload-2.0 event's requestContext.authorizer.iam names the verified caller — accountId is the caller's AWS account id (alongside callerId/userArn etc.).
Emulator: the function sees accountId null ('{"probe": "ctl-...", "accountId": null}') — requestContext.authorizer.iam is absent or empty on a request that WAS served 200.
Why: a function that authorizes or partitions by the caller's account id gets null and either denies every caller or treats them all as one, and the request-signed-for-another-service refusal leg the check then asserts cannot be reached.
Fix: the AWS_IAM authorization result → payload-2.0 requestContext.authorizer.iam mapping in the apigatewayv2 data plane.
Traces: apigatewayv2__lambda.iam-route-request-signed-for-another-service-rejected.

## Evidence
Cause 1: expected 403 {"message":"Forbidden"} and nothing delivered; got AssertionError: a body altered after signing should be refused 403, got 200: '{"served_by": "function", "body": "{\\"transfer\\": 990, \\"currency\\": \\"EUR\\"}"}'.
Cause 2: expected isBase64Encoded True and event.body == base64(bytes sent) for content-type application/x-www-form-urlencoded; got AssertionError: application/x-www-form-urlencoded: isBase64Encoded should be True, was False.
Cause 3: expected the backend to report {'probe': <probe>, 'accountId': <caller account id>} from requestContext.authorizer.iam.accountId; got AssertionError: the backend saw '{"probe": "ctl-...", "accountId": null}'.

## Steps to Reproduce
Cause 1: CreateApi(HTTP) + AWS_PROXY integration + CreateRoute(AuthorizationType='AWS_IAM') + $default stage; SigV4-sign a POST for execute-api, then change the body bytes before sending — the request is served 200 instead of refused 403.
Cause 2: same API with PayloadFormatVersion '2.0'; POST a body with content-type application/x-www-form-urlencoded and have the function echo isBase64Encoded — it is False.
Cause 3: same API with an AWS_IAM route; send a correctly signed GET and have the function echo requestContext.authorizer.iam.accountId — it is null.
