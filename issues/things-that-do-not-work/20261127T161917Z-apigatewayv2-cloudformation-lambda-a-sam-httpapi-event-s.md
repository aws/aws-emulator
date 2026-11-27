Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.sam-httpapi-event-payload-format-version-1-reaches-the-function
Issue-Key: apigatewayv2__cloudformation__lambda.sam-httpapi-event-payload-format-version-1-reaches-the-function|run#14|AssertionError|assert got == [("1.0", 5000)], f"integrations (PayloadFormatVersion, TimeoutInMillis) {got!r}"
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# apigatewayv2__cloudformation__lambda: a SAM HttpApi event's PayloadFormatVersion and TimeoutInMillis are ignored — the generated integration is created with the 2.0/30000 defaults

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Cause 1 of 1: the SAM transform drops the HttpApi event's PayloadFormatVersion and TimeoutInMillis when it generates the AWS::ApiGatewayV2::Integration.

AWS: a SAM AWS::Serverless::Function with an event of Type: HttpApi and Properties {Path, Method, PayloadFormatVersion: "1.0", TimeoutInMillis: 5000} expands into an AWS_PROXY integration carrying exactly those values, so GetIntegrations reports PayloadFormatVersion '1.0' and TimeoutInMillis 5000, and a request through the API hands the function a format-1.0 event (event.version == '1.0'). Smithy: CreateIntegration$PayloadFormatVersion (StringWithLengthBetween1And64) and CreateIntegration$TimeoutInMillis (IntegerWithLengthBetween50And30000, range 50..30000) are the members the transform must set; the AWS default when unset is 2.0 / 30000.

Emulator: the stack reaches CREATE_COMPLETE but GetIntegrations reports [('2.0', 30000)] — the event's two properties were not carried onto the generated integration, so the function would be handed a 2.0 event and the 5 s timeout is not applied.

Why: a SAM template that pins payload format 1.0 (because its handler reads 1.0 fields) silently gets 2.0 on the emulator, so the function sees a different event shape than on AWS, and an integration timeout the template set is not honoured.

Fix: the SAM transform's HttpApi event expansion in the CloudFormation service — propagate PayloadFormatVersion and TimeoutInMillis from the event properties onto the generated AWS::ApiGatewayV2::Integration.

Traces: the check in trace_ids; related open issue 20261109T154749Z-apigatewayv2-cloudformation-lambda-3-net-new-causes-the.md covers other SAM/endpoint causes and does not carry this replay key.

## Evidence
Expected GetIntegrations [(PayloadFormatVersion, TimeoutInMillis)] == [('1.0', 5000)] for an HttpApi event declared `Get: {Type: HttpApi, Properties: {Path: /v, Method: GET, PayloadFormatVersion: "1.0", TimeoutInMillis: 5000}}`.
Actual: AssertionError: integrations (PayloadFormatVersion, TimeoutInMillis) [('2.0', 30000)].

## Steps to Reproduce
1. CreateStack with Transform: AWS::Serverless-2016-10-31 and an AWS::Serverless::Function whose Events include `Get: {Type: HttpApi, Properties: {Path: /v, Method: GET, PayloadFormatVersion: "1.0", TimeoutInMillis: 5000}}`.
2. Wait for CREATE_COMPLETE, take the ServerlessHttpApi id from the outputs.
3. GetIntegrations(ApiId=<id>): the single integration reports PayloadFormatVersion '2.0' and TimeoutInMillis 30000 instead of '1.0' and 5000.
