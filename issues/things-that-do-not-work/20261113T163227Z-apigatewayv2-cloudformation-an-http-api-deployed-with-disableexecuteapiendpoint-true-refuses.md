Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain|run#17|AssertionError|assert status == 404, (
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# An HTTP API deployed with DisableExecuteApiEndpoint=true refuses its default execute-api domain with 403 Forbidden instead of 404 Not Found

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new cause.

Cause 1 of 1: a request to the default execute-api domain of an HTTP API whose DisableExecuteApiEndpoint is true is refused with the wrong status and the wrong refusal envelope. Failing check: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain.

AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) Api.DisableExecuteApiEndpoint: "Specifies whether clients can invoke your API by using the default execute-api endpoint. By default, clients can invoke your API with the default https://{api_id}.execute-api.{region}.amazonaws.com endpoint. To require that clients use a custom domain name to invoke your API, disable the default endpoint." The data-plane refusal itself is recorded behavior only: a plain request to that API's default execute-api host is refused by API Gateway with HTTP 404 and a body that is exactly {"message": "Not Found"} — all fixed text, no variable parts — while a sibling API in the same stack with the flag false serves its integration (HTTP 200 and the upstream's body).

Emulator: the same request is answered HTTP 403 with body {"message":"Forbidden"}. The control legs all pass: GetApi reports DisableExecuteApiEndpoint true/false correctly for the two APIs, and the enabled twin proxies through with 200 and the upstream token, so only the disabled API's data-plane refusal diverges.

Why: 403/Forbidden is the status and envelope API Gateway uses for authorization failures (missing authentication token, bad signature, WAF/resource-policy denial). A client, health probe or deployment test that distinguishes "the default endpoint is disabled, use the custom domain" (404 Not Found) from "this call was forbidden" (403) draws the opposite conclusion — it refreshes credentials or re-signs instead of switching to the custom domain name — and any assertion on the Not Found envelope fails.

Fix: the execute-api data-plane front door for apigatewayv2 — the branch that enforces an API's DisableExecuteApiEndpoint flag must emit API Gateway's 404 Not Found refusal document instead of the 403 Forbidden one.

Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain. Open issue 20261026T235900Z-apigatewayv2-cloudformation-2-net-new-root-causes-aws.md records an EARLIER form of this cause (the request fell through to the emulator's router and returned an {"__type":"UnknownService"} envelope) and does not carry this check's current replay key; the emulator now refuses from the API Gateway path, but with the wrong status and message.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (810/2121 checks passed; the emulator is up and answering).

apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain [mismatch]:
  assert status == 404, (f"the disabled twin's default domain must answer 404, got {status}: {text[:300]!r}")
  AssertionError: the disabled twin's default domain must answer 404, got 403: '{"message":"Forbidden"}'

The check deploys one stack with two AWS::ApiGatewayV2::Api resources that differ only in DisableExecuteApiEndpoint (true/false), both ProtocolType HTTP with Target+RouteKey "ANY /" quick-create against a mock upstream. The assertions that passed before this one: GetApi(DisabledApiId).DisableExecuteApiEndpoint is True, GetApi(EnabledApiId).DisableExecuteApiEndpoint is False, and GET on the ENABLED twin's default execute-api domain answers 200 with the upstream token. The assertion immediately after the failing one requires the body to be exactly {"message": "Not Found"}; the emulator sends {"message":"Forbidden"}. Status 403 and the word "Forbidden" are the divergence; there are no per-run values in the required body or status.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain.
2. By hand: CreateStack with a template holding two AWS::ApiGatewayV2::Api resources, ProtocolType HTTP, Target=<any reachable upstream URL>, RouteKey "ANY /", one with DisableExecuteApiEndpoint: true and one with false; wait for CREATE_COMPLETE and read both API ids from the stack outputs.
3. GET the ENABLED api's default execute-api domain ($default stage) — expect 200 and the upstream body (this works today).
4. GET the DISABLED api's default execute-api domain — expect HTTP 404 with body exactly {"message": "Not Found"}. The emulator answers HTTP 403 with {"message":"Forbidden"}.
