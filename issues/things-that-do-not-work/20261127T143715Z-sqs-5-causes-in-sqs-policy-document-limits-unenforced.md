Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.create-queue-policy-over-fifty-principals-creates-nothing-rejected sqs.create-queue-policy-over-twenty-statements-creates-nothing-rejected sqs.create-queue-policy-statement-over-ten-conditions-creates-nothing-rejected sqs.policy-principal-limit-counts-distinct-principals sqs.set-queue-attributes-policy-over-twenty-statements-rejected sqs.change-visibility-path-only-word-account-url-refused sqs.purge-queue-path-only-word-account-url-refused sqs.change-message-visibility-receipt-handle-with-prepended-characters-rejected sqs.receipt-handle-padded-past-1024-characters-still-acts-on-its-message sqs.receive-message-ignores-a-system-attribute-name-outside-the-enum sqs.queue-policy-org-perimeter-and-principal-is-aws-service-conditions
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.create-queue-policy-over-fifty-principals-creates-nothing-rejected|run#12|AssertionError|assert refused == (
Issue-Key: sqs.create-queue-policy-over-twenty-statements-creates-nothing-rejected|_refusal#14|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {answer!r}")
Issue-Key: sqs.create-queue-policy-statement-over-ten-conditions-creates-nothing-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {answer!r}")
Issue-Key: sqs.policy-principal-limit-counts-distinct-principals|run#14|AssertionError|assert collapsed == root, (
Issue-Key: sqs.set-queue-attributes-policy-over-twenty-statements-rejected|_refusal#11|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {answer!r}")
Issue-Key: sqs.change-visibility-path-only-word-account-url-refused|run#13|AssertionError|assert message == expected, f"InvalidAddress message was {message!r}, expected {expected!r}"
Issue-Key: sqs.purge-queue-path-only-word-account-url-refused|run#10|AssertionError|assert message == expected, f"InvalidAddress message was {message!r}, expected {expected!r}"
Issue-Key: sqs|ChangeMessageVisibility|ReceiptHandleIsInvalid
Issue-Key: sqs|ReceiveMessage|InvalidParameterValue
Issue-Key: sqs.queue-policy-org-perimeter-and-principal-is-aws-service-conditions|_until_allowed#30|TimeoutError|raise TimeoutError("an allowed send: the run budget ended first; not graded")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 5 causes in SQS: policy document limits unenforced, InvalidAddress wording, padded receipt handles, system-attribute enum, and policy condition keys

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
Five causes.

Cause 1 of 5: the `Policy` queue attribute's document limits are not enforced, on CreateQueue or SetQueueAttributes.
AWS: SQS validates the submitted policy document's structure before storing it, and a document over any limit is refused with nothing created or changed (Smithy: com.amazonaws.sqs CreateQueue and SetQueueAttributes both model InvalidAttributeValue, HTTP 400, and SetQueueAttributes also models OverLimit, awsQueryError `OverLimit`, HTTP 403). The required refusals, whose counts and limits are the only variable parts:
- more than 50 principals in one statement: `InvalidAttributeValue` / `Invalid value for the parameter <n> principals were found in a statement from the submitted policy, max allowed is 50.` — and the count is of *distinct* principals, so duplicate entries naming the same principal collapse (60 entries naming 50 distinct sessions store 50 and are accepted).
- more than 20 statements on CreateQueue: `InvalidParameterValue` / `<n> statements were found in the submitted policy, max allowed is 20`.
- more than 10 conditions in one statement on CreateQueue: `InvalidParameterValue` / `<n> conditions were found in a statement from the submitted policy, max allowed is 10`.
- more than 20 statements on SetQueueAttributes: `OverLimit` / `<n> statements were found in the submitted policy, max allowed is 20`, leaving the stored policy in place.
After a refused CreateQueue the queue does not exist, so GetQueueUrl answers `AWS.SimpleQueueService.NonExistentQueue` / `The specified queue does not exist.`
Emulator: every one of these is accepted. CreateQueue with 51 principals, 21 statements and an 11-condition statement all return HTTP 200 with a QueueUrl; SetQueueAttributes with 21 statements returns HTTP 200; and duplicate principal entries are stored verbatim (60 root-ARN entries stored as 60 identical principals) rather than collapsed to distinct ones.
Why: a client's over-limit policy is silently stored, so an authorization document AWS would have rejected is live in the emulator; the queue's effective permissions differ from production, and the client never learns its policy is invalid until it deploys.
Fix: the `Policy` queue-attribute validator shared by the CreateQueue and SetQueueAttributes handlers (statement, principal — counted distinctly — and condition limits, each with its own error code).
The open issue 20261109T185037Z-sqs-7-net-new-causes-redrive-move-task-identity.md reports the adjacent SetQueueAttributes size/condition/action limits; it does not carry these replay keys.
Traces: sqs.create-queue-policy-over-fifty-principals-creates-nothing-rejected, sqs.create-queue-policy-over-twenty-statements-creates-nothing-rejected, sqs.create-queue-policy-statement-over-ten-conditions-creates-nothing-rejected, sqs.policy-principal-limit-counts-distinct-principals, sqs.set-queue-attributes-policy-over-twenty-statements-rejected.

Cause 2 of 5: the `InvalidAddress` message names the request path instead of the endpoint address.
AWS: a request whose QueueUrl is a path-only form SQS does not serve is refused `InvalidAddress` (Smithy: com.amazonaws.sqs InvalidAddress, awsQueryError `InvalidAddress`, HTTP 404) with the message `The address <endpoint> is not valid for this endpoint.`, where `<endpoint>` is the service endpoint the request was sent to (scheme, host and trailing `/`) — the rest of the string is fixed text.
Emulator: the message substitutes the request's URL path instead, e.g. `The address /queue-owner/<queue-name> is not valid for this endpoint.`
Why: the client is told a path it never supplied as an address is invalid, which misdirects every diagnosis of a bad queue URL; a handler matching on the endpoint value sees the wrong token.
Fix: the SQS request router's InvalidAddress message construction (both ChangeMessageVisibility and PurgeQueue reach it).
Traces: sqs.change-visibility-path-only-word-account-url-refused, sqs.purge-queue-path-only-word-account-url-refused.

Cause 3 of 5: a receipt handle with trailing padding is refused instead of acting on its message, and the refusal message for an unusable handle is wrong.
AWS: SQS matches a receipt handle by its leading content, so characters appended *after* a real handle are tolerated and ChangeMessageVisibility acts on the handle's message (the check pads to 1025 characters, past the 1024 bound, and the call succeeds). Characters placed *before* a handle make it unmatchable and the call is refused `ReceiptHandleIsInvalid` (Smithy: com.amazonaws.sqs ReceiptHandleIsInvalid, awsQueryError `ReceiptHandleIsInvalid`, HTTP 404) with the message `The input receipt handle "<handle>" is not a valid receipt handle.` — the fixed text is `The input receipt handle "` … `" is not a valid receipt handle.` and the quoted handle is the value from the request.
Emulator: the suffixed handle is refused too — `ReceiptHandleIsInvalid: The input receipt handle is invalid.` — so the legal call fails; and that message omits the quoted handle AWS includes.
Why: a client whose handle is round-tripped through a store or a URL that appends padding can no longer extend or release its message, and the refusal does not say which handle was rejected, so a consumer handling many handles cannot tell which one failed.
Fix: the receipt-handle parser used by ChangeMessageVisibility/DeleteMessage (match on the handle's leading content rather than exact length/equality) and its ReceiptHandleIsInvalid message.
Traces: sqs.change-message-visibility-receipt-handle-with-prepended-characters-rejected, sqs.receipt-handle-padded-past-1024-characters-still-acts-on-its-message.

Cause 4 of 5: ReceiveMessage rejects a `MessageSystemAttributeNames` value outside the enum instead of ignoring it.
AWS: ReceiveMessage tolerates a system-attribute name it does not recognise — it simply returns no such attribute — so the call succeeds and the recognised names in the same list are still honoured. The modelled enum (Smithy: com.amazonaws.sqs MessageSystemAttributeName) lists All, SenderId, SentTimestamp, ApproximateReceiveCount, ApproximateFirstReceiveTimestamp, SequenceNumber, MessageDeduplicationId, MessageGroupId, AWSTraceHeader, DeadLetterQueueSourceArn, and the model places no server-side validation on the member beyond that.
Emulator: the call is refused `InvalidParameterValue` with `Value (<name>) for parameter MessageSystemAttributeNames is invalid. Reason: Must be one of: All, SenderId, ...`.
Why: a client built against a newer SDK (or one that passes a name the service has since added) has its receives refused outright rather than degraded, so a consumer stops draining its queue on a request AWS accepts.
Fix: the ReceiveMessage handler's MessageSystemAttributeNames handling (ignore unrecognised names instead of refusing the request).
Traces: sqs.receive-message-ignores-a-system-attribute-name-outside-the-enum.

Cause 5 of 5: a queue policy using organization-perimeter and service-principal condition keys leaves an allowed send denied.
AWS: a queue policy whose statements are guarded by `aws:PrincalOrgID`-style organization conditions and `aws:PrincipalIsAWSService` evaluates each variant to a definite outcome: a request the policy allows is accepted (SendMessage succeeds), a policy variant SQS considers malformed is refused at SetQueueAttributes with `InvalidAttributeValue` / `Invalid value for the parameter Policy.`, and a request an explicit deny covers is refused `AccessDenied` with `User: <principal-arn> is not authorized to perform: sqs:sendmessage on resource: <queue-arn> with an explicit deny in a resource-based policy` (the principal and queue ARNs are request/state values; the rest is fixed text).
Emulator: the allowed variant never becomes allowed — the check exhausts its budget waiting for the send the policy permits to succeed, so the policy's allow path is never satisfied.
Why: a client that applies the standard data-perimeter queue policy locks itself out of its own queue: the sends the policy allows stay refused, which looks like a credentials problem rather than an unimplemented condition key.
Fix: the SQS resource-policy evaluator's handling of the organization and `aws:PrincipalIsAWSService` condition keys.
Traces: sqs.queue-policy-org-perimeter-and-principal-is-aws-service-conditions.

## Evidence
Cause 1 — `CreateQueue with 51 principals in one statement answered {'QueueUrl': ..., 'HTTPStatusCode': 200}`; `the call was expected to be refused but answered {'QueueUrl': ..., 'HTTPStatusCode': 200}` (21 statements, and 11 conditions); `the call was expected to be refused but answered {... 'HTTPStatusCode': 200 ...}` (SetQueueAttributes, 21 statements); and `60 entries naming only this account (root ARN and bare id) stored as ['arn:aws:iam::<acct>:root', ... repeated 60x]` instead of collapsing to the distinct principals.
Cause 2 — expected `The address https://<host>:<port>/ is not valid for this endpoint.`; actual `The address /queue-owner/<queue-name> is not valid for this endpoint.` for both ChangeMessageVisibility and PurgeQueue.
Cause 3 — expected the suffix-padded handle to be accepted and the prefix-padded handle refused with `The input receipt handle "<handle>" is not a valid receipt handle.`; actual `botocore.errorfactory.ReceiptHandleIsInvalid: ... The input receipt handle is invalid.` raised on the *suffixed* call in both checks.
Cause 4 — expected the call to succeed, ignoring the unknown name; actual `An error occurred (InvalidParameterValue) when calling the ReceiveMessage operation: Value (QueueArn) for parameter MessageSystemAttributeNames is invalid. Reason: Must be one of: All, SenderId, SentTimestamp, ApproximateReceiveCount, ApproximateFirstReceiveTimestamp, SequenceNumber, MessageDeduplicationId, MessageGroupId, AWSTraceHeader, DeadLetterQueueSourceArn.`
Cause 5 — expected the allowed variant to answer ('OK', None); actual `TimeoutError: an allowed send: the run budget ended first` — the send the policy allows never succeeded.

## Steps to Reproduce
Replay each listed check id. In outline: (1) CreateQueue/SetQueueAttributes with a Policy of 51 principals in one statement, 21 statements, or an 11-condition statement, and with 60 duplicate entries naming 50 distinct principals; (2) call ChangeMessageVisibility and PurgeQueue with a path-only QueueUrl such as `/<account-word>/<queue-name>`; (3) receive a message, append padding after the receipt handle past 1024 characters and call ChangeMessageVisibility, then prepend padding instead; (4) ReceiveMessage with MessageSystemAttributeNames=['QueueArn']; (5) set a queue policy whose statements use organization/aws:PrincipalIsAWSService conditions and send from a session the allow statement covers.
