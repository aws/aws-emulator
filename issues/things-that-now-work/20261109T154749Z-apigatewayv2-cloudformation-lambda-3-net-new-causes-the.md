Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route apigatewayv2__cloudformation__lambda.sam-websocketapi-missing-route-selection-expression-rejected apigatewayv2__cloudformation__lambda.sam-websocketapi-round-trip-and-removed-on-delete
Issue-Key: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected|_http#27|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Key: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route|run#12|PublishedUrlRejected|base, headers = ctx.endpoint_route(outputs["Endpoint"])
Issue-Key: apigatewayv2__cloudformation__lambda.sam-websocketapi-missing-route-selection-expression-rejected|run#7|AssertionError|assert status is None, f"the refused change set must not be executed; stack ended {status!r}"
Issue-Key: apigatewayv2__cloudformation__lambda.sam-websocketapi-round-trip-and-removed-on-delete|run#15|AssertionError|assert [a["ApiId"] for a in listed] == [api_id], (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudformation__lambda: 3 net-new causes — the published ApiEndpoint is an AWS execute-api host, a SAM WebSocketApi with no RouteSelectionExpression is deployed instead of refused, and a SAM-declared WebSocket api never appears in GetApis

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes behind the SAM/CFN + Lambda HTTP/WebSocket API checks. All three must be closed.

Cause 1 of 3: a CFN/SAM-deployed HTTP API publishes its invoke address as an AWS public execute-api hostname the emulator does not serve, so no request reaches the route (2 checks).
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint, documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is actually served on, and what AWS::ApiGatewayV2::Api / AWS::Serverless::HttpApi expose for callers. On AWS, GET <ApiEndpoint>/<path> reaches the AWS_PROXY integration; a route whose Lambda lacks an AWS::Lambda::Permission for apigateway.amazonaws.com is answered 500 with body {"message":"Internal Server Error"} while the permitted twin serves the function's payload-2.0 response.
Emulator: the published value is "https://<api-id>.execute-api.<region>.amazonaws.com" (one recording shows it with the route path appended), refused by replay with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"). No request is issued, so the missing-permission refusal and the SAM payload-v2 serving contract reach no verdict.
Why: a SAM/CDK estate deployed against the emulator cannot be invoked at all; the only address its stack publishes points outside the emulator.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi/ImportApi handlers plus the execute-api front-door listener that must serve what they publish.
Traces: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected, apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

Cause 2 of 3: a SAM WebSocket API template that omits the required route-selection expression is deployed instead of refused, and the refused change set is executed (1 check).
AWS: recorded behavior only. A WEBSOCKET protocol api requires RouteSelectionExpression (apigatewayv2 CreateApi refuses a WEBSOCKET api without it with 400 BadRequestException), so CloudFormation refuses the change set and the stack does not reach CREATE_COMPLETE — the check asserts "the refused change set must not be executed".
Emulator: the stack ends 'CREATE_COMPLETE'; the invalid template is deployed.
Why: a template AWS rejects deploys here, so a WebSocket api with no route-selection expression exists in the emulator and the stack's failure path is never exercised; estates validated only against the emulator break in production.
Fix: the AWS::Serverless::Api / AWS::ApiGatewayV2::Api resource handler's WEBSOCKET validation in the cloudformation crate (and the underlying apigatewayv2 CreateApi RouteSelectionExpression requirement), plus the change-set refusal path.
Traces: apigatewayv2__cloudformation__lambda.sam-websocketapi-missing-route-selection-expression-rejected.

Cause 3 of 3: a SAM-declared WebSocket api is never created in the apigatewayv2 control plane — Ref resolves to nothing GetApis knows (1 check).
AWS: Smithy: GetApis — GET /v2/apis, output Items : __listOfApi with ApiId/Name/ProtocolType. A SAM (AWS::Serverless-2016-10-31) WebSocket api resource creates a real apigatewayv2 api: after CREATE_COMPLETE, Ref is its ApiId and GetApis lists the api under the template's Name; after DeleteStack it is gone.
Emulator: GetApis lists nothing for the stack's api name ("Ref '<name>' must be the apiId GetApis lists for '<name>'; listed []"), so neither the round trip nor the removal-on-delete can be graded.
Why: a SAM estate's WebSocket api cannot be found, updated or invoked through the API Gateway control plane even though the stack reports success.
Fix: the SAM transform / AWS::ApiGatewayV2::Api resource handler in the cloudformation crate must create the api in the same apigatewayv2 state GetApis reads, and propagate the id as Ref.
Related but not covering: 20261105T194642Z-apigatewayv2-cloudformation-lambda-a-sam-aws-serverless-2016.md and 20261107T184737Z-apigatewayv2-cloudformation-lambda-a-sam-httpapi-route-on-the-default-stage.md concern SAM HttpApi serving and own other replay keys.
Traces: apigatewayv2__cloudformation__lambda.sam-websocketapi-round-trip-and-removed-on-delete.

## Evidence
Cause 1 — expected: GET <ApiEndpoint>/<route> reaches the Lambda (payload format 2.0 response) and the permission-stripped twin is answered 500 {"message":"Internal Server Error"}. Actual: simparity.published.PublishedUrlRejected "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com/allowed?marker=... (published by the emulator or built by the check): its host is not one of the emulator's own hosts".
Cause 2 — expected: the SAM WebSocket template without a route-selection expression is refused and the stack does not execute. Actual: AssertionError "the refused change set must not be executed; stack ended 'CREATE_COMPLETE'".
Cause 3 — expected: Ref of the SAM WebSocket api == the ApiId GetApis lists for its Name. Actual: AssertionError "Ref 'sim2real-sam-wsapi-api-<id>' must be the apiId GetApis lists for 'sim2real-sam-wsapi-api-<id>'; listed []".

## Steps to Reproduce
1. Deploy a SAM/CFN template with an HTTP API + AWS_PROXY Lambda route; read the stack's ApiEndpoint output and request the route — the host is not served by the emulator. Replay apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.
2. Deploy a SAM WebSocket api template that omits the route-selection expression — the stack reaches CREATE_COMPLETE instead of being refused. Replay apigatewayv2__cloudformation__lambda.sam-websocketapi-missing-route-selection-expression-rejected.
3. Deploy a valid SAM WebSocket api, then call apigatewayv2 GetApis — the api is not listed. Replay apigatewayv2__cloudformation__lambda.sam-websocketapi-round-trip-and-removed-on-delete.
