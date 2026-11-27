Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue|run#24|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/orders"
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing|run#22|PublishedUrlRejected|allowed_url = f"{ctx.stage_url(api_endpoint, '$default')}/allowed"
Issue-Commit: ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a

# apigatewayv2__events__sqs: the EventBridge-PutEvents AWS_PROXY subtype cannot be driven — the published ApiEndpoint is an unserved '<api-id>.execute-api.<region>.localhost:<port>' host (2 checks)

> _Note: this issue was identified against the code base at commit `ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the invoke address published for the HTTP API is not an address the emulator answers on, so no request reaches the EventBridge-PutEvents subtype route.

AWS: Smithy (apigatewayv2) documents Api$ApiEndpoint as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." On AWS that host serves the API: a POST on an EventBridge-PutEvents subtype route puts the mapped event on the named custom bus, a matching rule then delivers it to its SQS target, and a route whose credentials carry no events:PutEvents grant delivers nothing (the gateway answers 5xx and the queue stays empty).

Emulator: it publishes 'https://<api-id>.execute-api.<region>.localhost:<ephemeral-port>' while listening only on 127.0.0.1:<core-port>. The dial is refused before any request is sent:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.execute-api.<region>.localhost:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<core-port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The api-id, region label and ports are per-run values; the requirement is that the published address is one this emulator answers on.

Why: the API has no reachable address, so neither the happy-path bus→rule→queue delivery nor the permission-denied no-delivery case can be observed.

Fix: the ApiEndpoint publication in the apigatewayv2 Api response serializers must announce an address the emulator serves, and the execute-api front door must route that host into the AWS_PROXY subtype dispatcher. Context: fixed issue 20261109T154749Z-apigatewayv2-events-sqs-the-http-api-s-published.md is the same family; the published host is now '.localhost' and still unserved.

Traces: the 2 failing checks in trace_ids.

## Evidence
Expected: POST '<ApiEndpoint>/<route path>' puts the mapped event on the custom bus and the rule's SQS target receives it; with no events:PutEvents grant the queue receives nothing.

Actual (both checks under '### apigatewayv2__events__sqs'): no request is delivered. Each fails with
  simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.localhost:40649 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:43269/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(host/port values differ per check and per run.)

## Steps to Reproduce
1. CreateEventBus + PutRule with an SQS target; CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='EventBridge-PutEvents') with Detail/DetailType/Source/EventBusName mapped; CreateRoute + CreateStage.
2. Read ApiEndpoint and POST the event body to '<ApiEndpoint>/<route path>'.
3. ReceiveMessage on the rule's target queue.
4. AWS: the event arrives (or does not, when the grant is absent). Emulator: the published host '<api-id>.execute-api.<region>.localhost:<port>' is not served, so nothing is delivered.
