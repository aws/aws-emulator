Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-proxy-event-request-context-carries-protocol apigateway__lambda.apigw-proxy-event-identity-carries-api-key-and-key-id apigateway__lambda.apigw-proxy-event-authorizer-carries-integration-latency apigateway__lambda.request-authorizer-event-carries-request-headers-query-and-stage-variables apigateway__lambda.apigw-lambda-proxy-integration-declared-with-get-rejected apigateway__lambda.apigw-lambda-custom-integration-non-json-body-rejected apigateway__lambda.lambda-unsigned-get-to-an-iam-rest-api-is-rejected-with-missing-token apigateway__lambda.imported-openapi-lambda-integration-rejected-until-permission-granted apigateway__lambda.invoke-grant-naming-the-request-path-rejected-template-grant-admits apigateway__lambda.streamed-pipeline-reply-keeps-metadata-apart-from-a-json-looking-payload
Issue-Key: apigateway__lambda.apigw-proxy-event-request-context-carries-protocol|run#12|AssertionError|assert rc.get("protocol") == "HTTP/1.1", f"requestContext.protocol was {rc.get('protocol')!r}; keys {sorted(rc)!r}"
Issue-Key: apigateway__lambda.apigw-proxy-event-identity-carries-api-key-and-key-id|run#10|AssertionError|assert identity.get("apiKey") == key_value, f"identity.apiKey was {identity.get('apiKey')!r}; identity {identity!r}"
Issue-Key: apigateway__lambda.apigw-proxy-event-authorizer-carries-integration-latency|run#15|AssertionError|assert isinstance(auth.get("integrationLatency"), int), (
Issue-Key: apigateway__lambda.request-authorizer-event-carries-request-headers-query-and-stage-variables|run#17|AssertionError|assert (seen["resource"], seen["pathParameters"]) == ("/", {}), (
Issue-Key: apigateway__lambda.apigw-lambda-proxy-integration-declared-with-get-rejected|run#17|AssertionError|assert status == 500, (
Issue-Key: apigateway__lambda.apigw-lambda-custom-integration-non-json-body-rejected|run#25|AssertionError|assert (status, headers.get("x-amzn-errortype")) == (400, "BadRequestException"), (
Issue-Key: apigateway__lambda.lambda-unsigned-get-to-an-iam-rest-api-is-rejected-with-missing-token|run#13|AssertionError|assert (got["status"], json.loads(got["body"])) == (
Issue-Key: apigateway__lambda.imported-openapi-lambda-integration-rejected-until-permission-granted|run#14|AssertionError|assert method["authorizationType"] == "AWS_IAM", (
Issue-Key: apigateway__lambda.invoke-grant-naming-the-request-path-rejected-template-grant-admits|_await#21|Timeout|time.sleep(min(2 ** attempt, 30))
Issue-Key: apigateway__lambda.streamed-pipeline-reply-keeps-metadata-apart-from-a-json-looking-payload|run#19|Timeout|time.sleep(3)
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__lambda: 10 net-new causes — AWS_IAM is not enforced on a deployed stage, the proxy event drops protocol/identity.apiKey/authorizer.integrationLatency/pathParameters, a GET-declared integration is served, and an imported OpenAPI method loses sigv4

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Ten independent causes on the REST API -> Lambda path (AWS_PROXY and custom AWS integrations) of a deployed stage. Cause 7 is an authorization bypass. Distinct from the already-open 20261122T081303Z-apigateway-lambda-5-net-new-causes-a-deployed (which owns the response-streaming and language-runtime round-trip checks).

Cause 1 of 10: requestContext.protocol is absent from the AWS_PROXY event.
AWS: recorded behavior only. The payload-1.0 proxy event's requestContext carries `protocol` (the request's HTTP protocol, e.g. `HTTP/1.1`) alongside accountId, apiId, domainName, httpMethod, identity, path, requestId, stage and the rest.
Emulator: `requestContext.protocol` is None; the delivered keys are ['accountId','apiId','domainName','domainPrefix','extendedRequestId','httpMethod','identity','path','requestId','requestTime','requestTimeEpoch','resourceId','resourcePath','stage'] — protocol is simply not emitted.
Fix: the AWS_PROXY event builder (requestContext).
Traces: apigateway__lambda.apigw-proxy-event-request-context-carries-protocol

Cause 2 of 10: requestContext.identity.apiKey and apiKeyId are absent.
AWS: recorded behavior only. For a request carrying an API key against an apiKeyRequired method, the proxy event's `requestContext.identity.apiKey` is the key's value and `identity.apiKeyId` is its id.
Emulator: `identity.apiKey` is None and the whole identity map is `{'sourceIp': '127.0.0.1'}` — every identity member except sourceIp is missing.
Fix: the AWS_PROXY event builder (requestContext.identity).
Traces: apigateway__lambda.apigw-proxy-event-identity-carries-api-key-and-key-id

Cause 3 of 10: requestContext.authorizer.integrationLatency is absent.
AWS: recorded behavior only. When a Lambda authorizer runs, API Gateway adds `integrationLatency` (the authorizer invocation's latency in ms) to the authorizer context passed to the integration, alongside the authorizer's own context keys. (The latency VALUE is per-request and is not the requirement; its presence as a numeric member is.)
Emulator: `authorizer.integrationLatency` is None; the authorizer map contains only the authorizer's own keys (`{'principalId': 'user-1', 'tier': 'gold'}`).
Fix: the AWS_PROXY event builder (requestContext.authorizer) / the authorizer invocation path.
Traces: apigateway__lambda.apigw-proxy-event-authorizer-carries-integration-latency

Cause 4 of 10: a REQUEST authorizer's event omits pathParameters (and reports the raw path as `resource`).
AWS: recorded behavior only. A REQUEST-type Lambda authorizer receives `resource` as the method's RESOURCE PATH TEMPLATE (e.g. `/orders/{id}`) and `pathParameters` as the extracted map (e.g. `{"id": "42"}`), together with headers, queryStringParameters and stageVariables.
Emulator: the authorizer event carries `('/orders/42', None)` for (resource, pathParameters) — the concrete path instead of the template, and no pathParameters map at all.
Why: an authorizer that authorizes per path parameter (the common multi-tenant pattern) has nothing to authorize on.
Fix: the REQUEST-authorizer event builder.
Traces: apigateway__lambda.request-authorizer-event-carries-request-headers-query-and-stage-variables

Cause 5 of 10: a Lambda integration declared with a GET backend verb is served instead of failing.
AWS: recorded behavior only. Lambda's Invoke endpoint only accepts POST, so an AWS_PROXY/AWS integration whose integrationHttpMethod is GET makes the stage answer HTTP 500 (an integration failure), not the function's reply.
Emulator: the stage answers 200 with the function's proxy response (`{"served_by": "arn:aws:lambda:...:function:<name>", "resource": "/broken"}`) — the declared backend verb is ignored.
Fix: the AWS-integration request builder — send the declared integrationHttpMethod, and surface Lambda's refusal as a 500.
Traces: apigateway__lambda.apigw-lambda-proxy-integration-declared-with-get-rejected

Cause 6 of 10: a custom (non-proxy) Lambda integration given a non-JSON payload answers 500 instead of 400.
AWS: recorded behavior only; BadRequestException is @httpError 400 in the model. A plain-text payload sent to a custom Lambda integration is refused as a CLIENT error: HTTP 400 with `x-amzn-ErrorType: BadRequestException`.
Emulator: HTTP 500 with `x-amzn-ErrorType: InternalServerErrorException` and `{"message":"Internal server error"}`.
Why: the caller's malformed payload is reported as a server fault, so clients and SDKs retry a request that can never succeed.
Fix: the custom-integration request path's error classification.
Traces: apigateway__lambda.apigw-lambda-custom-integration-non-json-body-rejected

Cause 7 of 10: AWS_IAM authorization is not enforced — an unsigned GET is served.
AWS: recorded behavior only. An unsigned request to a method with authorizationType AWS_IAM is refused HTTP 403 with the body `{"message":"Missing Authentication Token"}` (fixed text) and the integration is never invoked.
Emulator: the unsigned GET is answered `{'status': 200, 'body': '{"reply": "pong-<id>"}'}` — the function ran and its reply was served.
Why: an authorization bypass: any IAM-protected REST API is open to unauthenticated callers.
Fix: the execute-api authorizer gate for authorizationType AWS_IAM (refuse when no SigV4 credential is presented).
Traces: apigateway__lambda.lambda-unsigned-get-to-an-iam-rest-api-is-rejected-with-missing-token

Cause 8 of 10: ImportRestApi drops a method's sigv4 security requirement.
AWS: recorded behavior only. An OpenAPI document whose operation carries a `sigv4`-typed security scheme (x-amazon-apigateway-authtype awsSigv4) imports as a method with authorizationType AWS_IAM; GetMethod reports AWS_IAM.
Emulator: GetMethod reports `NONE` for the imported method, so the API is open and the permission-gated negative leg can never occur.
Fix: the ImportRestApi/PutRestApi OpenAPI importer — map security schemes to authorizationType.
Traces: apigateway__lambda.imported-openapi-lambda-integration-rejected-until-permission-granted

Cause 9 of 10: a lambda:InvokeFunction resource-policy grant scoped to a specific request path is not distinguished from a template grant.
AWS: recorded behavior only. A Lambda resource policy whose AWS:SourceArn names the concrete request path does NOT admit the invoke (API Gateway signs with the method's resource-path template), while a grant naming the path TEMPLATE does; so the first leg is refused and the second is served.
Emulator: the check reaches no verdict inside its 60 s budget — neither the refusal nor the admission is observed.
Fix: the Lambda resource-policy evaluation for the SourceArn API Gateway presents (the method's resource-path template).
Traces: apigateway__lambda.invoke-grant-naming-the-request-path-rejected-template-grant-admits

Cause 10 of 10: a streamed pipeline reply does not keep its metadata apart from a JSON-looking payload.
AWS: recorded behavior only. A response-streaming function whose payload itself looks like JSON still has its prelude metadata (status/headers) parsed separately from the body, so the client receives the declared status and headers and the payload bytes unchanged.
Emulator: the check reaches no verdict inside its 60 s budget — the stage never served a graded answer.
Fix: the response-streaming relay's prelude/payload framing.
Traces: apigateway__lambda.streamed-pipeline-reply-keeps-metadata-apart-from-a-json-looking-payload

## Evidence
Cause 1 — "requestContext.protocol was None; keys ['accountId','apiId','domainName','domainPrefix','extendedRequestId','httpMethod','identity','path','requestId','requestTime','requestTimeEpoch','resourceId','resourcePath','stage']".
Cause 2 — "identity.apiKey was None; identity {'sourceIp': '127.0.0.1'}".
Cause 3 — "authorizer.integrationLatency was None; authorizer {'principalId': 'user-1', 'tier': 'gold'}".
Cause 4 — "resource and pathParameters were ('/orders/42', None)".
Cause 5 — "GET-declared Lambda integration answered 200 with '{\"served_by\": \"arn:aws:lambda:...:function:<name>\", \"resource\": \"/broken\"}', expected 500".
Cause 6 — "a plain-text payload should be refused with 400 BadRequestException, got 500 'InternalServerErrorException': '{\"message\":\"Internal server error\"}'".
Cause 7 — "the unsigned GET answered {'status': 200, 'body': '{\"reply\": \"pong-<id>\"}'}, expected 403 Missing Authentication Token".
Cause 8 — "the imported method must require SigV4, got 'NONE'".
Causes 9 and 10 — each check killed at the 60 s cap with no graded answer.

## Steps to Reproduce
1. Create a Lambda function echoing its event; CreateRestApi with an AWS_PROXY integration on /{proxy+} and on /orders/{id}; CreateDeployment + stage.
2. Invoke through the stage and inspect the delivered event: requestContext.protocol, requestContext.identity.apiKey/apiKeyId (with an API key on an apiKeyRequired method), requestContext.authorizer.integrationLatency (behind a TOKEN authorizer) and the REQUEST authorizer's resource/pathParameters (Causes 1-4).
3. PutIntegration with integrationHttpMethod=GET and invoke (Cause 5); send text/plain to a custom (non-proxy) integration (Cause 6).
4. PutMethod authorizationType=AWS_IAM and issue an UNSIGNED GET to the stage (Cause 7).
5. ImportRestApi an OpenAPI document whose operation declares a sigv4 security scheme and read GetMethod.authorizationType (Cause 8).
6. Grant lambda:InvokeFunction with AWS:SourceArn naming the concrete request path, then the path template, and invoke (Cause 9); invoke a response-streaming function whose payload is JSON-looking text (Cause 10).
