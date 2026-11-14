Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes|_deploy_cfn#6|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain|run#18|AssertionError|assert json.loads(text) == {"message": "Not Found"}, (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# apigatewayv2__cloudformation: 2 net-new root causes — AWS::ApiGatewayV2::Api declared by its Body (inline OpenAPI) is never created, and a DisableExecuteApiEndpoint API's execute-api host answers the router's UnknownService envelope instead of API Gateway's Not Found body

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct net-new root causes grounded in two failing behavioral checks. A fixer must close both before submitting.

Cause 1 of 2 — a CloudFormation AWS::ApiGatewayV2::Api declared with the `Body` property (inline OpenAPI/Swagger import) is not provisioned; the stack rolls back.
AWS: recorded behavior only — AWS::ApiGatewayV2::Api accepts either quick-create properties (Target/RouteKey/ProtocolType) or a `Body` holding an OpenAPI document, and in the Body case CloudFormation imports the document: the API, its routes and its AWS_PROXY integrations exist afterwards and the deployed stage serves those routes. The stack reaches CREATE_COMPLETE.
Emulator: the resource fails to create and the stack ends ROLLBACK_COMPLETE with ResourceStatusReason `The following resource(s) failed to create: [<logical-id>]. NotImplemented: this emulator does not create a AWS::ApiGatewayV2::Api from its Body property; the resource was not created.`
Why: every HTTP-API stack that imports its definition (the standard SAM/OpenAPI-first deployment shape) is undeployable, and the failure is reported as a stack rollback rather than as an unsupported property, so a caller cannot tell a bad template from a missing provisioner.
Fix: the AWS::ApiGatewayV2::Api resource provisioner in the cloudformation crate's provisioner layer — the Body branch must drive the apigatewayv2 import path (ImportApi semantics) and register the resulting api/routes/integrations in apigatewayv2 state. Related issue 20261026T023803Z-apigatewayv2-cloudformation-stack-outputs-carry-no-outputvalue-and.md is marked fixed and covers the DomainName/WEBSOCKET quick-create gaps, not the Body branch.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes

Cause 2 of 2 — with DisableExecuteApiEndpoint=true the default execute-api endpoint is not refused by API Gateway; the request falls through to the emulator's service router.
AWS: recorded behavior only — when an AWS::ApiGatewayV2::Api sets DisableExecuteApiEndpoint: true, a request to that api's default execute-api host is refused by API Gateway itself with the standard API Gateway refusal document, body exactly `{"message": "Not Found"}` (fixed text, no variable parts), while the sibling API with DisableExecuteApiEndpoint: false serves normally.
Emulator: the same request is answered with the emulator's own router envelope `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — an emulator-internal error shape that no AWS service emits, with an `__type` of UnknownService.
Why: a client probing whether the default endpoint is disabled gets an unparseable non-AWS body; any code distinguishing "endpoint disabled" (API Gateway Not Found) from "wrong host/unknown service" misclassifies it, and the leak of the router envelope onto a data-plane path means the disabled-endpoint rule is simply not implemented.
Fix: the execute-api data-plane entry point for apigatewayv2 — it must own the request for a known api id and apply the api's DisableExecuteApiEndpoint flag, emitting API Gateway's `{"message": "Not Found"}` refusal instead of letting the request reach the generic service-resolution path.
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain

## Evidence
Cause 1 — apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-inline-body-openapi-proxy-serves-its-routes:
  AssertionError: stack sim2real-xsvc-bodyimport-stk-<id> ended in ROLLBACK_COMPLETE: The following resource(s) failed to create: [Api]. NotImplemented: this emulator does not create a AWS::ApiGatewayV2::Api from its Body property; the resource was not created.
  (template: AWS::ApiGatewayV2::Api with a Body holding an OpenAPI document whose operations use AWS_PROXY / payloadFormatVersion 2.0 integrations)

Cause 2 — apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-disable-execute-api-endpoint-blocks-default-domain:
  assert json.loads(text) == {"message": "Not Found"}
  AssertionError: the disabled twin's refusal body must be the standard Not Found envelope, got '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  (template deploys two AWS::ApiGatewayV2::Api resources differing only in DisableExecuteApiEndpoint true/false)

## Steps to Reproduce
1. Cause 1: CreateStack with a template whose only resource is {"Type": "AWS::ApiGatewayV2::Api", "Properties": {"Body": <OpenAPI document with an x-amazon-apigateway-integration of type AWS_PROXY>}}; wait for the stack. Expect CREATE_COMPLETE and GetApi/GetRoutes to show the imported routes; the emulator rolls the stack back with the NotImplemented reason above.
2. Cause 2: CreateStack with two AWS::ApiGatewayV2::Api resources, ProtocolType HTTP, Target+RouteKey quick-create, one with DisableExecuteApiEndpoint: true and one false. Issue a plain HTTP GET to the DISABLED api's default execute-api endpoint and read the body: it must be exactly {"message": "Not Found"}; the emulator returns the router's {"__type":"UnknownService", ...} envelope.
