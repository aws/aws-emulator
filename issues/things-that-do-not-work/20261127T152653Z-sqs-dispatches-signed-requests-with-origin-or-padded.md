Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-http-origin-header-on-signed-json-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-http-origin-header-on-signed-json-requests sqs-unsigned-and-interior-tab-header-requests-to-a-queue
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|GetQueueUrl|4xx->4xx|$.__type
Issue-Key: sqs|GetQueueUrl|4xx->4xx|$.message
Issue-Key: sqs|GetQueueUrl|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|GetQueueUrl|4xx->4xx|status
Issue-Key: sqs|CreateQueue|2xx->4xx|$.QueueUrl
Issue-Key: sqs|CreateQueue|2xx->4xx|status
Issue-Key: sqs|ListQueues|2xx->4xx|$.QueueUrls
Issue-Key: sqs|ListQueues|2xx->4xx|status
Issue-Key: sqs|GetQueueUrl|4xx->4xx|$headers.content-type
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$.Message
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$.__type
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$headers.content-type
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|GetQueueAttributes|4xx->4xx|status
Issue-Key: sqs|GetQueueAttributes|2xx->4xx|$.Attributes
Issue-Key: sqs|GetQueueAttributes|2xx->4xx|status
Issue-Commit: a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff

# sqs dispatches signed requests with Origin or padded headers to no service

> _Note: this issue was identified against the code base at commit `a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueUrl

## Description
The emulator's HTTP ingress in front of SQS makes admission and dispatch decisions on request headers that AWS does not use for either purpose. A SigV4-signed request that merely carries an `Origin` header is blanket-refused 403 AccessDenied, and a request whose header value contains interior whitespace fails service resolution entirely and is answered by a generic unknown-service responder in the wrong JSON protocol version. Service identity from the Smithy model: AmazonSQS 2012-11-05, authSchemes ['sigv4'], protocols ['awsJson1_0'] (so the response content type is `application/x-amz-json-1.0`) with protocol modifier awsQueryCompatible (so every error carries `x-amzn-query-error: <code>;<fault>`). 6 causes follow; each must be closed before this issue is submitted.

--- Cause 1 of 6: GetQueueUrl on a signed request with an Origin header is refused 403 instead of answering ---
AWS: Smithy: the service's only auth scheme is sigv4; `Origin` is not part of the signature, not an authorization input, and SQS models no browser-origin refusal (GetQueueUrl's modelled errors are InvalidAddress, InvalidSecurity, QueueDoesNotExist, RequestThrottled, UnsupportedOperation). A correctly signed GetQueueUrl for a name that does not exist is therefore answered on its merits: 400, `__type` com.amazonaws.sqs#QueueDoesNotExist, message `The specified queue does not exist.` (all fixed text), header `x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender`.
Emulator: 403 with `__type` AccessDenied and message `Requests a browser sends from another site are refused`. `AccessDenied` is not a modelled SQS error shape and the sentence is the emulator's own prose; the `x-amzn-query-error` header is absent.
Why: any caller that forwards an Origin header — a browser-side SDK, a console, an API Gateway or reverse proxy that passes the client's Origin through — is locked out of SQS entirely, and gets an unclassifiable error shape telling it about a cross-site rule AWS does not apply to signed API calls.
Fix: the HTTP ingress/CORS layer ahead of the SQS dispatcher — an Origin header must not gate a sigv4-signed request.
Traces: sqs-http-origin-header-on-signed-json-requests#task#1.

--- Cause 2 of 6: CreateQueue on a signed request with an Origin header is refused 403 instead of creating the queue ---
AWS: Smithy: same sigv4-only identity and no origin-based refusal; CreateQueue succeeds and answers 200 with the `QueueUrl` member naming the created queue (the recorded host/account/name are environment values, not requirements — what is required is a 200 carrying a QueueUrl for the queue just created).
Emulator: 403 and no `QueueUrl` member at all.
Why: provisioning fails outright for any client whose requests carry an Origin header, and because the whole fixture's later steps depend on the queue existing, the failure cascades.
Fix: as Cause 1 — the ingress Origin check, on the CreateQueue path.
Traces: sqs-http-origin-header-on-signed-json-requests#task#2.

--- Cause 3 of 6: ListQueues on a signed request with an Origin header is refused 403 instead of listing ---
AWS: Smithy: same sigv4-only identity; ListQueues answers 200 with the `QueueUrls` list containing the account's queue URLs.
Emulator: 403 and no `QueueUrls` member.
Why: the same blanket refusal denies reads as well as writes, so no operation at all is reachable for an Origin-carrying client.
Fix: as Cause 1 — the ingress Origin check, on the ListQueues path.
Traces: sqs-http-origin-header-on-signed-json-requests#task#3.

--- Cause 4 of 6: a request whose header value contains an interior tab is answered 'UnknownService' in JSON 1.1 instead of an SQS InvalidAddress ---
AWS: Smithy: GetQueueUrl's modelled errors include InvalidAddress (errorKind client, httpError 404, awsQueryError code InvalidAddress / 404). For a request addressed to an endpoint that does not serve the queue, AWS answers 404 with `__type` com.amazonaws.sqs#InvalidAddress, `content-type: application/x-amz-json-1.0` (the model's awsJson1_0 protocol), header `x-amzn-query-error: InvalidAddress;Sender`, and the message template `The address <url> is not valid for this endpoint.` — fixed text `The address ` + ` is not valid for this endpoint.`, with the request's address as the only variable part.
Emulator: 404, but `__type` is `UnknownService` and the message is the emulator's own router text `no emulated service is registered for 'amazonsqs'`; `x-amzn-query-error` is absent and `content-type` is `application/x-amz-json-1.1`.
Why: the client receives an internal routing error instead of a modelled SQS error — `UnknownService` corresponds to no shape any SDK can classify, and the 1.1 content type contradicts the service's declared awsJson1_0 protocol, so a strict SDK may fail to deserialize the body at all. Hypothesis: the service-resolution step does not normalize header values, so a header carrying an interior tab fails to match the SQS target and dispatch falls through to a generic unknown-service responder that answers in JSON 1.1.
Fix: request header normalization in service dispatch, plus the fallback responder's protocol/content-type and error shape for an endpoint that does resolve to SQS.
Traces: sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#2, sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#3 (same cluster).

--- Cause 5 of 6: an anonymous GetQueueAttributes is answered 404 UnknownService instead of 403 AccessDeniedException ---
AWS: Smithy: GetQueueAttributes requires sigv4 and models InvalidSecurity (403) among its errors; an unsigned (anonymous) call against a queue whose resource policy grants nothing is refused 403 with `__type` com.amazon.coral.service#AccessDeniedException, header `x-amzn-query-error: AccessDenied;Sender`, `content-type: application/x-amz-json-1.0`, and the message delivered in the `Message` member (capital M) using the template `User: <principal> is not authorized to perform: <action> on resource: <resource-arn> because no resource-based policy allows the <action> action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no resource-based policy allows the `, ` action`; variable parts are the principal (`anonymous` for an unsigned call), the lower-cased action (`sqs:getqueueattributes`) and the target queue's ARN taken from the emulator's state.
Emulator: 404 with `__type` UnknownService and the router message `no emulated service is registered for 'amazonsqs'`; no `x-amzn-query-error`, and `content-type: application/x-amz-json-1.1`. The message is not even placed in the `Message` member AWS uses for this error class.
Why: an unauthenticated caller is told the service does not exist rather than that it is not authorized. Status class flips from 403 to 404, so clients cannot distinguish 'needs credentials' from 'wrong endpoint', and nothing reports which principal/action/resource was denied.
Fix: the same dispatch/header-normalization defect as Cause 4, plus the anonymous-authorization path for GetQueueAttributes which must produce AccessDeniedException with AWS's `Message` template.
Traces: sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#4.

--- Cause 6 of 6: a properly signed GetQueueAttributes is refused 403 instead of returning the queue's attributes ---
AWS: Smithy: GetQueueAttributes answers 200 with the `Attributes` map holding the requested attribute names and their values (here DelaySeconds, MaximumMessageSize, VisibilityTimeout — the attribute NAMES and the fact that each requested name is present are the requirement; the specific numbers come from the queue's own configuration/state).
Emulator: 403 and no `Attributes` member at all — a legitimately signed read of an existing queue is denied.
Why: the queue's configuration becomes unreadable to its owner. Anything that reconciles against queue attributes (redrive setup, size/visibility checks, CFN drift, autoscaling on queue config) cannot read the state it needs and sees an authorization error for a request that was authorized.
Fix: the GetQueueAttributes authorization decision — a signed, authorized caller must not be refused; the preceding unsigned/odd-header steps must not poison the admission decision for the subsequent signed request.
Traces: sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (46 fixtures, 411 steps, 389 passed, score 94.6%, emulator_up=true, emulator_detail: '/__admin/health answered 200 and the :46483 data listener accepts'). Six mismatch clusters, all service=sqs:

Cause 1 — sqs-http-origin-header-on-signed-json-requests#task#1 (GetQueueUrl), expected 400, got 403:
  $.__type: expected 'com.amazonaws.sqs#QueueDoesNotExist', got 'AccessDenied'
  $.message: expected 'The specified queue does not exist.', got 'Requests a browser sends from another site are refused'
  $headers.x-amzn-query-error: missing (expected 'AWS.SimpleQueueService.NonExistentQueue;Sender', got None)

Cause 2 — sqs-http-origin-header-on-signed-json-requests#task#2 (CreateQueue), expected 200, got 403:
  $.QueueUrl: missing (expected a QueueUrl for the created queue, got None)

Cause 3 — sqs-http-origin-header-on-signed-json-requests#task#3 (ListQueues), expected 200, got 403:
  $.QueueUrls: missing (expected ['{{queue_url}}'], got None)

Cause 4 — sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#2 (GetQueueUrl), status 404/404 (cluster also covers task#3):
  $.__type: expected 'com.amazonaws.sqs#InvalidAddress', got 'UnknownService'
  $.message: expected 'The address https://sqs.us-west-2.amazonaws.com/ is not valid for this endpoint.', got "no emulated service is registered for 'amazonsqs'"
  $headers.x-amzn-query-error: missing (expected 'InvalidAddress;Sender', got None)
  $headers.content-type: expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1'

Cause 5 — sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#4 (GetQueueAttributes), expected 403, got 404:
  $.__type: expected 'com.amazon.coral.service#AccessDeniedException', got 'UnknownService'
  $.Message: expected 'User: anonymous is not authorized to perform: sqs:getqueueattributes on resource: arn:aws:sqs:us-west-2:000000000000:sim2real-probe-sqsp-e6f619f5 because no resource-based policy allows the sqs:getqueueattributes action', got "no emulated service is registered for 'amazonsqs'"
  $headers.x-amzn-query-error: missing (expected 'AccessDenied;Sender', got None)
  $headers.content-type: expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1'

Cause 6 — sqs-unsigned-and-interior-tab-header-requests-to-a-queue#task#6 (GetQueueAttributes), expected 200, got 403:
  $.Attributes: missing (expected {'DelaySeconds': '0', 'MaximumMessageSize': '1048576', 'VisibilityTimeout': '30'}, got None)

Model evidence (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): service AmazonSQS 2012-11-05, sdk_id SQS, authSchemes ['sigv4'], protocols ['awsJson1_0'], protocol_modifiers ['awsQueryCompatible'], common_errors [InvalidAddress, InvalidSecurity, RequestThrottled, UnsupportedOperation]. GetQueueUrl errors: InvalidAddress (404, awsQueryError InvalidAddress), InvalidSecurity (403), QueueDoesNotExist (400, awsQueryError AWS.SimpleQueueService.NonExistentQueue), RequestThrottled, UnsupportedOperation. GetQueueAttributes errors: InvalidAddress (404), InvalidAttributeName (400), InvalidSecurity (403), QueueDoesNotExist (400), RequestThrottled, UnsupportedOperation; output member Attributes (QueueAttributeMap). No modelled error corresponds to `AccessDenied` as a bare __type or to `UnknownService`.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Origin-header causes — re-replay sqs-http-origin-header-on-signed-json-requests: send correctly SigV4-signed awsJson1_0 SQS requests that additionally carry an `Origin: <some-site>` request header.
   - task#1 GetQueueUrl for a nonexistent name must answer 400 com.amazonaws.sqs#QueueDoesNotExist / 'The specified queue does not exist.' with x-amzn-query-error 'AWS.SimpleQueueService.NonExistentQueue;Sender' — not 403 AccessDenied.
   - task#2 CreateQueue must answer 200 with a QueueUrl.
   - task#3 ListQueues must answer 200 with QueueUrls containing that queue.
4. Header-dispatch causes — re-replay sqs-unsigned-and-interior-tab-header-requests-to-a-queue:
   - task#2 (and task#3) send a request whose header value contains an interior tab; the response must be 404 com.amazonaws.sqs#InvalidAddress with content-type application/x-amz-json-1.0 and x-amzn-query-error 'InvalidAddress;Sender' — not __type UnknownService in application/x-amz-json-1.1.
   - task#4 send an unsigned (anonymous) GetQueueAttributes for the probe queue; the response must be 403 com.amazon.coral.service#AccessDeniedException with the message in the `Message` member using AWS's 'User: <principal> is not authorized to perform: <action> on resource: <arn> because no resource-based policy allows the <action> action' template, content-type application/x-amz-json-1.0, x-amzn-query-error 'AccessDenied;Sender'.
   - task#6 send a properly signed GetQueueAttributes for the same queue; it must answer 200 with the Attributes map containing the requested attribute names (DelaySeconds, MaximumMessageSize, VisibilityTimeout) — not 403.
5. Confirm no step's admission decision depends on the Origin header or on whitespace inside a header value.
