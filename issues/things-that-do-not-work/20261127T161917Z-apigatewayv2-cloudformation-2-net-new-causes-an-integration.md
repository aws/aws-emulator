Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.cfn-apigatewayv2-integration-response-parameters-map-round-trips apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-deployment-getatt-id-staged-on-another-api-rejected
Issue-Key: apigatewayv2__cloudformation.cfn-apigatewayv2-integration-response-parameters-map-round-trips|run#5|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-deployment-getatt-id-staged-on-another-api-rejected|run#22|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# apigatewayv2__cloudformation: 2 net-new causes — an Integration's CFN ResponseParameters list form is refused 400 and rolls the stack back, and an in-place AWS::ApiGatewayV2::Deployment update fails

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Two net-new root causes in the CloudFormation handling of apigatewayv2 resources. Each is numbered with its own failing check ids; both must be closed before this issue is submitted.

Cause 1 of 2: AWS::ApiGatewayV2::Integration ResponseParameters given in CloudFormation's ResponseParameterMap list form is passed to CreateIntegration verbatim and refused, failing the stack.
AWS: the CloudFormation resource accepts ResponseParameters as a status-keyed map of {"ResponseParameters": [ {"Source": ..., "Destination": ...}, ... ]} (the ResponseParameterMap / ResponseParameter list form) and translates it into the API shape before calling CreateIntegration, so the stack reaches CREATE_COMPLETE and GetIntegration reports the parameter map {"200": {"append:header.header2": "headerValue"}}. Smithy: CreateIntegration$ResponseParameters targets ResponseParameters = map<__string, IntegrationParameters>, i.e. map<status, map<destination, source>> whose values are strings (StringWithLengthBetween1And512) — a JSON array is not a legal value there, which is exactly why the resource handler must translate.
Emulator: the list form is forwarded unchanged and the service refuses it: the Integ resource fails with `Resource handler returned message: "expected string, got array start (Service: ApiGatewayV2, Status Code: 400, Request ID: ...)" ... HandlerErrorCode: InvalidRequest`, and the stack ends ROLLBACK_COMPLETE.
Why: every stack that configures HTTP API response parameters the documented CloudFormation way fails to deploy, and the deserializer's 400 ("expected string, got array start") is a parse complaint rather than a modelled refusal.
Fix: the AWS::ApiGatewayV2::Integration resource handler in the CloudFormation service — translate the ResponseParameterMap list form into the status-keyed destination→source map before calling CreateIntegration.
Traces: apigatewayv2__cloudformation.cfn-apigatewayv2-integration-response-parameters-map-round-trips.

Cause 2 of 2: an in-place update of an AWS::ApiGatewayV2::Deployment fails, rolling the stack back.
AWS: a stack update that only changes a Deployment's Description updates the deployment in place: the resource keeps its physical id (the DeploymentId Fn::GetAtt and Ref both still report it), GetDeployment shows the new Description, the stage pinned to that DeploymentId stays pinned, and the stack ends UPDATE_COMPLETE.
Emulator: the update ends `UPDATE_ROLLBACK_COMPLETE` with the Deployment resource (logical id `Snapshot`) reported `UPDATE_FAILED`, so the whole update is rolled back.
Why: any stack that parameterises a Deployment's Description (a common way to force/annotate a redeploy) cannot be updated at all, and because the update rolls back, the downstream contract the check then asserts — that a DeploymentId is valid only on its own API, CreateStage elsewhere naming it being refused ('BadRequestException', 'Deployment id does not exist') — is never reached.
Fix: the AWS::ApiGatewayV2::Deployment resource handler's update path in the CloudFormation service (in-place update of Description, keeping the physical id).
Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-deployment-getatt-id-staged-on-another-api-rejected.

## Evidence
Cause 1: expected stack CREATE_COMPLETE and GetIntegration ResponseParameters == {'200': {'append:header.header2': 'headerValue'}} for Properties.ResponseParameters = {'200': {'ResponseParameters': [{'Source': 'headerValue', 'Destination': 'append:header.header2'}]}}; got AssertionError: create ended ROLLBACK_COMPLETE: [('Integ', 'Resource handler returned message: "expected string, got array start (Service: ApiGatewayV2, Status Code: 400, Request ID: ...)" ... HandlerErrorCode: InvalidRequest')].
Cause 2: expected UPDATE_COMPLETE after update_stack(UsePreviousTemplate=True) changing only the Deployment's Description parameter; got AssertionError: update ended 'UPDATE_ROLLBACK_COMPLETE'; failed resources [('Snapshot', 'UPDATE_FAILED')].

## Steps to Reproduce
Cause 1: CreateStack with AWS::ApiGatewayV2::Api (HTTP) and AWS::ApiGatewayV2::Integration whose Properties.ResponseParameters = {"200": {"ResponseParameters": [{"Source": "headerValue", "Destination": "append:header.header2"}]}} — the Integration resource fails 400 and the stack rolls back.
Cause 2: CreateStack with a WEBSOCKET Api, a MOCK Integration, a $default Route, an AWS::ApiGatewayV2::Deployment whose Description is a template parameter, and a Stage using Fn::GetAtt [Deployment, DeploymentId]; wait for CREATE_COMPLETE; UpdateStack(UsePreviousTemplate=True) with a new Description value — the Deployment resource ends UPDATE_FAILED and the stack rolls back.
