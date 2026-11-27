Issue-Source: trace-sweep-divergence
Issue-Trace: sqs-delete-message-forged-receipt-handle-rejection
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sqs-delete-message-forged-receipt-handle-rejection sqs-list-queue-tags-refusals sqs-message-move-task-rejects-malformed-queue-arns sqs-released-receipt-handle-expires-on-fifo-queue-rejection sqs-send-message-malformed-queue-url-rejections sqs-send-message-string-attribute-control-characters
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|DeleteMessage|4xx->4xx|$.message
Issue-Key: sqs|ListQueueTags|4xx->4xx|$.__type
Issue-Key: sqs|ListQueueTags|4xx->4xx|$.message
Issue-Key: sqs|ListQueueTags|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|$.__type
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|$.message
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|StartMessageMoveTask|4xx->4xx|status
Issue-Key: sqs|ChangeMessageVisibility|4xx->2xx|$
Issue-Key: sqs|ChangeMessageVisibility|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: sqs|ChangeMessageVisibility|4xx->2xx|status
Issue-Key: sqs|DeleteMessage|4xx->2xx|$
Issue-Key: sqs|DeleteMessage|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: sqs|DeleteMessage|4xx->2xx|status
Issue-Key: sqs|SendMessage|4xx->4xx|$.__type
Issue-Key: sqs|SendMessage|4xx->4xx|$.message
Issue-Key: sqs|SendMessage|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: sqs|SendMessage|4xx->4xx|status
Issue-Key: sqs|SendMessage|4xx->2xx|$.__type
Issue-Key: sqs|SendMessage|4xx->2xx|$.message
Issue-Key: sqs|SendMessage|4xx->2xx|$headers.x-amzn-query-error
Issue-Key: sqs|SendMessage|4xx->2xx|status
Issue-Commit: a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff

# sqs refusals carry the wrong error class, status and message text

> _Note: this issue was identified against the code base at commit `a668be1c2b7e82cad4ee1de57e0ed4f21d5374ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMessage

## Description
The emulator's SQS input-validation plane diverges from AWS on eight distinct refusal paths: it picks the wrong modelled error shape, the wrong HTTP status class, drops the awsQueryCompatible error header, rewrites AWS's message templates, or does not refuse at all. Service identity from the Smithy model: AmazonSQS 2012-11-05, protocol awsJson1_0 with protocol modifier awsQueryCompatible, so EVERY error response must additionally carry `x-amzn-query-error: <QueryErrorCode>;<Sender|Receiver>`. 8 causes follow; each must be closed before this issue is submitted.

--- Cause 1 of 8: DeleteMessage drops the rejected handle from the ReceiptHandleIsInvalid message ---
AWS: Smithy: DeleteMessage errors include ReceiptHandleIsInvalid (errorKind client, httpError 404, awsQueryError code ReceiptHandleIsInvalid / httpResponseCode 404). For a syntactically well-formed but unissued handle AWS answers 404 and the message names the offending value: fixed text `The input receipt handle "` ... `" is not a valid receipt handle.`, with the quoted variable part taken verbatim from the request's ReceiptHandle member.
Emulator: status 404 and the error shape are right, but the message is the generic `The input receipt handle is invalid.` — the quoted handle is omitted entirely and the sentence is reworded.
Why: a client that submits several handles (a delete loop or a hand-rolled batch) cannot tell WHICH handle was rejected, because the only discriminator AWS provides is the quoted value in the message. Operators grepping logs for a specific handle find nothing.
Fix: the DeleteMessage handler's receipt-handle validation, where the ReceiptHandleIsInvalid message is formatted — it must interpolate the request's ReceiptHandle into AWS's template rather than emit a constant string.
Traces: sqs-delete-message-forged-receipt-handle-rejection#task#3.

--- Cause 2 of 8: ListQueueTags reports a present QueueUrl as a missing parameter ---
AWS: Smithy: ListQueueTags.QueueUrl is @required and was supplied; the operation's modelled errors include QueueDoesNotExist (httpError 400, awsQueryError AWS.SimpleQueueService.NonExistentQueue / 400). A QueueUrl that does not name an existing queue is a nonexistent-queue refusal: 400, `__type` com.amazonaws.sqs#QueueDoesNotExist, message `The specified queue does not exist.` (entirely fixed text), header `x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender`. MissingRequiredParameterException is not in ListQueueTags' modelled error set.
Emulator: 400 with `__type` com.amazon.coral.service#MissingRequiredParameterException, message `The request must contain the parameter QueueUrl.`, header `x-amzn-query-error: MissingParameter;Sender`.
Why: the client is told its request was malformed when the request was well-formed and the queue simply is not there. SDK retry/handling branches keyed on QueueDoesNotExist (create-if-absent, idempotent teardown) never fire, and the caller 'fixes' a parameter that was never wrong. Hypothesis: the handler's QueueUrl resolution treats a URL it cannot map onto a known queue as an absent parameter instead of an unresolved one.
Emulator: see above. Fix: ListQueueTags' QueueUrl resolution / validation ordering — unresolvable must yield QueueDoesNotExist, absent must yield the missing-parameter refusal.
Traces: sqs-list-queue-tags-refusals#task#4.

--- Cause 3 of 8: a ListQueueTags malformed-input refusal omits the x-amzn-query-error header ---
AWS: Smithy: the service carries protocol modifier awsQueryCompatible, so every error response is accompanied by the `x-amzn-query-error` header naming the Query-protocol error code and fault. For this malformed-input refusal the recorded header is `MalformedInput;Sender`.
Emulator: the 400 status and the JSON error body match, but the `x-amzn-query-error` header is absent from the response altogether.
Why: Query-compatible SDKs (and every SDK still mapping SQS errors through the Query error code, which is the point of awsQueryCompatible) read that header to classify the fault. With it missing the error degrades to an unclassified client error, so error-code-specific handling and metrics silently stop matching.
Fix: the shared SQS error serializer that emits query-compat headers — the malformed-input path does not go through it.
Traces: sqs-list-queue-tags-refusals#task#6.

--- Cause 4 of 8: StartMessageMoveTask refuses a malformed SourceArn as InvalidAddress/404 ---
AWS: Smithy: StartMessageMoveTask.SourceArn is @required; the operation's modelled errors are InvalidAddress (404), InvalidSecurity, RequestThrottled, ResourceNotFoundException and UnsupportedOperation. A SourceArn that is not ARN-shaped is refused as a parameter-value fault: 400, `__type` com.amazon.coral.service#InvalidParameterValueException, header `x-amzn-query-error: InvalidParameterValue;Sender`, and the message is the fixed format template `You must use this format to specify the SourceArn: arn:<partition>:<service>:<region>:<account-id>:<resource-id>` — the angle-bracketed parts are AWS's own literal placeholders, not substituted values, so the whole sentence is fixed text.
Emulator: 404 with `__type` com.amazonaws.sqs#InvalidAddress, header `InvalidAddress;Sender`, message `The address supplied for the SourceArn parameter is not a valid Amazon SQS queue ARN.`
Why: the status class flips from 4xx-client-parameter (400) to 404, and the error code from InvalidParameterValue to InvalidAddress. SDK error-type matching on InvalidParameterValueException misses, 404 handlers that mean 'resource absent' fire instead, and the caller never receives the ARN format AWS supplies to help it correct the input.
Fix: StartMessageMoveTask's SourceArn syntax validation — ARN-shape failure belongs on the InvalidParameterValue path, not the address/endpoint path.
Traces: sqs-message-move-task-rejects-malformed-queue-arns#task#5, sqs-message-move-task-rejects-malformed-queue-arns#task#6 (same cluster).

--- Cause 5 of 8: ChangeMessageVisibility accepts an expired receipt handle ---
AWS: Smithy: ChangeMessageVisibility.ReceiptHandle is @required; modelled errors include ReceiptHandleIsInvalid and MessageNotInflight. Once a handle's validity has lapsed (the message was released back to the queue and re-received, so the handle no longer refers to an in-flight receipt) AWS refuses with 400, `__type` com.amazon.coral.service#InvalidParameterValueException, header `x-amzn-query-error: InvalidParameterValue;Sender`, message template `Value <ReceiptHandle> for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired.` Fixed text: `Value ` + ` for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired.`; the single variable part is the request's ReceiptHandle (the recording's long base64 handle is one run's value, not part of the template).
Emulator: HTTP 200 with an empty body and no `x-amzn-query-error` header — the stale handle is accepted as if the visibility change applied.
Why: a consumer that keeps extending visibility while it works believes its lease was renewed. It was not: the message is already visible to other consumers, so the work is done twice and the consumer's own heartbeat loop never learns it lost the lease. This is a silent correctness failure, not a cosmetic one.
Fix: receipt-handle validity tracking in the queue state plus the ChangeMessageVisibility handler's handle check — an expired handle must be refused rather than treated as a no-op success.
Traces: sqs-released-receipt-handle-expires-on-fifo-queue-rejection#task#12.

--- Cause 6 of 8: DeleteMessage accepts an expired receipt handle ---
AWS: as Cause 5, for DeleteMessage (Smithy: DeleteMessage.ReceiptHandle @required; errors include ReceiptHandleIsInvalid). An expired handle is refused 400 com.amazon.coral.service#InvalidParameterValueException with `x-amzn-query-error: InvalidParameterValue;Sender` and the same `Value <ReceiptHandle> for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired.` template.
Emulator: HTTP 200, empty body, no query-error header.
Why: the consumer believes it deleted the message. The message is still on the queue and will be redelivered, so an at-least-once pipeline that relies on the delete to terminate processing loops on the same payload, and the error that would have told it the lease had lapsed never arrives.
Fix: the DeleteMessage handler's handle check against the queue's receipt state (same state tracking as Cause 5, separate call path).
Traces: sqs-released-receipt-handle-expires-on-fifo-queue-rejection#task#13.

--- Cause 7 of 8: SendMessage to a path-less queue URL answers InvalidAddress/404 instead of QueueDoesNotExist/400 ---
AWS: Smithy: SendMessage.QueueUrl is @required; modelled errors include both InvalidAddress (404, awsQueryError InvalidAddress) and QueueDoesNotExist (400, awsQueryError AWS.SimpleQueueService.NonExistentQueue). For a queue URL that is endpoint-valid but names no queue, AWS answers 400 with `__type` com.amazonaws.sqs#QueueDoesNotExist, message `The specified queue does not exist.` (all fixed text) and header `x-amzn-query-error: AWS.SimpleQueueService.NonExistentQueue;Sender`.
Emulator: 404 with `__type` com.amazonaws.sqs#InvalidAddress, header `InvalidAddress;Sender`, message `The address <QueueUrl> is not valid for this endpoint.` (fixed text `The address ` + ` is not valid for this endpoint.`, variable part the request's QueueUrl).
Why: the emulator classifies a missing queue as a bad endpoint. Producers that catch QueueDoesNotExist to provision the queue and retry instead see a 404 address error they treat as a configuration/endpoint fault, so the self-healing path never runs.
Fix: SendMessage's QueueUrl parsing and refusal selection — reserve InvalidAddress for URLs that do not address this endpoint and return QueueDoesNotExist once the URL parses but resolves to nothing. (The open issue 20261127T143715Z-sqs-5-causes-in-sqs-policy-document-limits-unenforced mentions InvalidAddress wording generally; it does not carry this replay key and does not cover this status/code misclassification.)
Traces: sqs-send-message-malformed-queue-url-rejections#task#4.

--- Cause 8 of 8: SendMessage accepts a message attribute value containing a control character ---
AWS: Smithy: SendMessage.MessageAttributes targets MessageBodyAttributeMap and the operation models InvalidParameterValueException/InvalidMessageContents; SQS restricts attribute string values to the XML-safe character set. A value containing an unsupported control codepoint is refused: 400, `__type` com.amazon.coral.service#InvalidParameterValueException, header `x-amzn-query-error: InvalidParameterValue;Sender`, message template `Message (user) attribute '<name>' value contains invalid binary character '<codepoint>'. You must use only the following supported characters: #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF].` Fixed text: `Message (user) attribute '`, `' value contains invalid binary character '`, and the whole `'. You must use only the following supported characters: ...` clause including the character-class list; variable parts are the attribute name from the request and the offending codepoint (e.g. `#x0`).
Emulator: HTTP 200 — the message is accepted with no refusal, no error body and no query-error header.
Why: payloads AWS would never admit are stored and later delivered. A consumer that round-trips attributes through XML/JSON serialization or a downstream store breaks on data the producer was told was fine, and the failure surfaces far from the producer that caused it.
Fix: SendMessage's MessageAttributes validation, before the message is enqueued.
Traces: sqs-send-message-string-attribute-control-characters#task#2, sqs-send-message-string-attribute-control-characters#task#4 (same cluster).

## Evidence
Replay of trace set simparity-wire-aws-v1 (46 fixtures, 411 steps, 389 passed, score 94.6%, emulator_up=true). Eight mismatch clusters, all service=sqs:

Cause 1 — sqs-delete-message-forged-receipt-handle-rejection#task#3 (DeleteMessage), status 404 expected / 404 actual:
  $.message: expected 'The input receipt handle "AQEBforgedHandleForInvoice2291" is not a valid receipt handle.', got 'The input receipt handle is invalid.'

Cause 2 — sqs-list-queue-tags-refusals#task#4 (ListQueueTags), status 400/400:
  $.__type: expected 'com.amazonaws.sqs#QueueDoesNotExist', got 'com.amazon.coral.service#MissingRequiredParameterException'
  $.message: expected 'The specified queue does not exist.', got 'The request must contain the parameter QueueUrl.'
  $headers.x-amzn-query-error: expected 'AWS.SimpleQueueService.NonExistentQueue;Sender', got 'MissingParameter;Sender'

Cause 3 — sqs-list-queue-tags-refusals#task#6 (ListQueueTags), status 400/400:
  $headers.x-amzn-query-error: missing (expected 'MalformedInput;Sender', got None)

Cause 4 — sqs-message-move-task-rejects-malformed-queue-arns#task#5 (StartMessageMoveTask), expected 400, got 404 (cluster also covers task#6):
  $.__type: expected 'com.amazon.coral.service#InvalidParameterValueException', got 'com.amazonaws.sqs#InvalidAddress'
  $.message: expected 'You must use this format to specify the SourceArn: arn:<partition>:<service>:<region>:<account-id>:<resource-id>', got 'The address supplied for the SourceArn parameter is not a valid Amazon SQS queue ARN.'
  $headers.x-amzn-query-error: expected 'InvalidParameterValue;Sender', got 'InvalidAddress;Sender'

Cause 5 — sqs-released-receipt-handle-expires-on-fifo-queue-rejection#task#12 (ChangeMessageVisibility), expected 400, got 200:
  $: empty_body — expected {'__type': 'com.amazon.coral.service#InvalidParameterValueException', 'message': 'Value <handle> for parameter ReceiptHandle is invalid. Reason: The receipt handle has expired.'}, got {}
  $headers.x-amzn-query-error: missing (expected 'InvalidParameterValue;Sender', got None)

Cause 6 — sqs-released-receipt-handle-expires-on-fifo-queue-rejection#task#13 (DeleteMessage), expected 400, got 200: identical empty-body/no-header divergence as Cause 5.

Cause 7 — sqs-send-message-malformed-queue-url-rejections#task#4 (SendMessage), expected 400, got 404:
  $.__type: expected 'com.amazonaws.sqs#QueueDoesNotExist', got 'com.amazonaws.sqs#InvalidAddress'
  $.message: expected 'The specified queue does not exist.', got 'The address https://sqs.us-west-2.amazonaws.com/ is not valid for this endpoint.'
  $headers.x-amzn-query-error: expected 'AWS.SimpleQueueService.NonExistentQueue;Sender', got 'InvalidAddress;Sender'

Cause 8 — sqs-send-message-string-attribute-control-characters#task#2 (SendMessage), expected 400, got 200 (cluster also covers task#4):
  $.__type: missing (expected 'com.amazon.coral.service#InvalidParameterValueException')
  $.message: missing (expected "Message (user) attribute 'k' value contains invalid binary character '#x0'. You must use only the following supported characters: #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF].")
  $headers.x-amzn-query-error: missing (expected 'InvalidParameterValue;Sender')

Model evidence (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): service AmazonSQS 2012-11-05, protocols ['awsJson1_0'], protocol_modifiers ['awsQueryCompatible']; DeleteMessage/ChangeMessageVisibility errors include ReceiptHandleIsInvalid (404) and QueueDoesNotExist (400, awsQueryError AWS.SimpleQueueService.NonExistentQueue); SendMessage errors include QueueDoesNotExist (400), InvalidAddress (404) and InvalidMessageContents; ListQueueTags errors include QueueDoesNotExist (400) with QueueUrl @required; StartMessageMoveTask has SourceArn @required.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Re-replay each cause's fixture step and compare status, `__type`, `message` and the `x-amzn-query-error` response header:
   - sqs-delete-message-forged-receipt-handle-rejection#task#3 — DeleteMessage with an unissued, well-formed ReceiptHandle; expect 404 ReceiptHandleIsInvalid whose message quotes the submitted handle.
   - sqs-list-queue-tags-refusals#task#4 — ListQueueTags with a QueueUrl that resolves to no queue; expect 400 com.amazonaws.sqs#QueueDoesNotExist + 'AWS.SimpleQueueService.NonExistentQueue;Sender'.
   - sqs-list-queue-tags-refusals#task#6 — ListQueueTags malformed input; expect header 'MalformedInput;Sender' present on the 400.
   - sqs-message-move-task-rejects-malformed-queue-arns#task#5 and #task#6 — StartMessageMoveTask with a non-ARN SourceArn; expect 400 InvalidParameterValueException + 'InvalidParameterValue;Sender' and the ARN format sentence.
   - sqs-released-receipt-handle-expires-on-fifo-queue-rejection#task#12 / #task#13 — receive a message on a FIFO queue, let the receipt handle lapse (message released and re-received), then call ChangeMessageVisibility and DeleteMessage with the stale handle; expect 400 InvalidParameterValueException ('... Reason: The receipt handle has expired.') on both, not 200.
   - sqs-send-message-malformed-queue-url-rejections#task#4 — SendMessage to a queue URL with no account/queue path; expect 400 QueueDoesNotExist.
   - sqs-send-message-string-attribute-control-characters#task#2 / #task#4 — SendMessage with a String message attribute whose value contains U+0000; expect 400 InvalidParameterValueException naming the attribute and the invalid codepoint, not 200.
4. All eight must match AWS's status, error shape, message template and query-error header.
