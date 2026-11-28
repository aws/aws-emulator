Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__sqs.aws-proxy-subtype-sqs-deletemessage-required-parameters-refused-else-deletes-the-message apigatewayv2__sqs.aws-proxy-subtype-sqs-purgequeue-required-queueurl-refused-else-empties-the-queue apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-required-queueurl-refused-else-reads-the-queue apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-deletemessage-required-parameters-refused-else-deletes-the-message|run#22|PublishedUrlRejected|url = (f"{ctx.stage_url(api['ApiEndpoint'], '$default')}"
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-purgequeue-required-queueurl-refused-else-empties-the-queue|run#21|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/purge"
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-required-queueurl-refused-else-reads-the-queue|run#21|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/recv"
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue|run#18|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/send"
Issue-Commit: ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a

# apigatewayv2__sqs: the SQS AWS_PROXY subtype data plane cannot be reached — the published ApiEndpoint is an unserved '<api-id>.execute-api.<region>.localhost:<port>' host (4 checks)

> _Note: this issue was identified against the code base at commit `ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the invoke address published for the HTTP API is not an address the emulator answers on, so no request reaches an SQS AWS_PROXY subtype route.

AWS: Smithy (apigatewayv2) documents Api$ApiEndpoint as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." On AWS that host serves the API: a POST on a SQS-SendMessage subtype route lands the message on the queue and passes SQS's response through; a SQS-DeleteMessage / SQS-PurgeQueue / SQS-ReceiveMessage subtype route acts on the queue through the mapped parameters, while a subtype created with its required parameters unmapped is refused at CreateIntegration with 400 BadRequestException naming the missing parameters.

Emulator: it publishes 'https://<api-id>.execute-api.<region>.localhost:<ephemeral-port>' while listening only on 127.0.0.1:<core-port>. The dial is refused before any request is sent:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.execute-api.<region>.localhost:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<core-port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The api-id, region label and ports are per-run values; the requirement is that the published address is one this emulator answers on.

Why: the half of each scenario that proves the integration actually acts on the queue (message delivered, deleted, purged, received) can never run, because the API has no reachable address.

Fix: the ApiEndpoint publication in the apigatewayv2 Api response serializers must announce an address the emulator serves, and the execute-api front door must route that host into the AWS_PROXY subtype dispatcher. Context: fixed issues 20261109T154749Z-apigatewayv2-sqs-2-net-new-causes-createintegration-accepts.md, 20261110T170829Z-apigatewayv2-sqs-the-sqs-sendmessage-aws-proxy-subtype.md and 20261118T133240Z-apigatewayv2-sqs-3-net-new-causes-the-sqs.md are the same family; the published host is now '.localhost' and still unserved.

Traces: the 4 failing checks in trace_ids.

## Evidence
Expected: POST '<ApiEndpoint>/<route path>' on each SQS AWS_PROXY subtype route performs the queue action (SendMessage lands the body on the queue and relays SQS's SendMessageResponse; DeleteMessage removes the message; PurgeQueue empties it; ReceiveMessage returns it), and a subtype created with required parameters unmapped is refused 400 BadRequestException at CreateIntegration.

Actual (all 4 checks under '### apigatewayv2__sqs'): no request is delivered. Each fails with
  simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.localhost:37313 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34187/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(host/port values differ per check and per run.)

## Steps to Reproduce
1. CreateQueue; CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='SQS-SendMessage' | 'SQS-DeleteMessage' | 'SQS-PurgeQueue' | 'SQS-ReceiveMessage') with the required RequestParameters mapped; CreateRoute + CreateStage.
2. Read ApiEndpoint and POST the scenario's body to '<ApiEndpoint>/<route path>'.
3. AWS: the queue action happens and SQS's response is relayed. Emulator: the published host '<api-id>.execute-api.<region>.localhost:<port>' is not served, so nothing is delivered.
