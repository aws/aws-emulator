Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes lambda__sqs.deleted-event-invoke-config-stops-destination-delivery-until-put-again lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes
Issue-Key: lambda__sqs.alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes|run#30|AssertionError|assert latest_rc.get("functionArn") == f"{fn['arn']}:$LATEST", (
Issue-Key: lambda__sqs.deleted-event-invoke-config-stops-destination-delivery-until-put-again|run#26|AssertionError|assert record.get("requestContext", {}).get("functionArn") == f"{subject['arn']}:$LATEST", (
Issue-Key: lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record|run#22|AssertionError|assert rc.get("functionArn") == f"{source['arn']}:$LATEST", (
Issue-Key: lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones|run#25|AssertionError|assert rec.get("requestContext", {}).get("functionArn") == f"{subject['arn']}:$LATEST", (
Issue-Key: lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message|run#20|AssertionError|assert payload == {
Issue-Key: lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte|run#15|AssertionError|assert len(got) == 1024, (
Issue-Key: lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes|run#16|AssertionError|assert attrs["ErrorCode"].get("DataType") == "Number", f"ErrorCode attribute: {attrs['ErrorCode']!r}"
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda__sqs: 4 net-new causes in the asynchronous invocation record and dead-letter metadata — functionArn unqualified, timeout responsePayload shape, ErrorMessage not truncated to 1 KB, and ErrorCode sent as a String attribute

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionEventInvokeConfig

## Description
Four net-new root causes in Lambda's asynchronous invocation plane as observed through SQS destinations and dead-letter queues. Each has its own failing check ids.

Cause 1 of 4: the invocation record's requestContext.functionArn omits the executed-version qualifier.
AWS: the destination invocation record Lambda writes to an OnSuccess/OnFailure destination carries `requestContext.functionArn` as the QUALIFIED ARN of the version that actually ran — `<function ARN>:$LATEST` for an unqualified invoke, and `<function ARN>:<alias>` (the invoked qualifier) for an alias invoke, which is what lets a consumer tell an alias-config delivery from an unqualified one. Documented in the Lambda developer guide, 'Asynchronous invocation' / 'Retaining records of asynchronous invocations'; recorded behavior as far as the model goes.
Emulator: `requestContext.functionArn` is the bare function ARN with no `:<qualifier>` suffix, e.g. `arn:aws:lambda:<region>:<account>:function:<name>` where `<...>:$LATEST` is required; the rest of requestContext (requestId, condition, approximateInvokeCount) is present.
Why: the record is the only place a destination consumer learns which version/alias produced it. Unqualified, a consumer cannot attribute a record to the alias whose event-invoke config routed it, so per-alias destination routing and per-version accounting both break.
Fix: the lambda asynchronous-invocation record builder — render the executed version/alias qualifier into requestContext.functionArn.
Traces: lambda__sqs.alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes, lambda__sqs.deleted-event-invoke-config-stops-destination-delivery-until-put-again, lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record, lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones.

Cause 2 of 4: the timeout responsePayload in an OnFailure record has the wrong keys and the wrong message text.
AWS: when an asynchronous invocation is killed by the function timeout, the OnFailure record's `responsePayload` is exactly `{"errorType": "Sandbox.Timedout", "errorMessage": "RequestId: <request id> Error: Task timed out after <timeout> seconds"}` — two keys only; the fixed text is `RequestId: ` / ` Error: Task timed out after ` / ` seconds`, with the invocation's request id and the function's configured timeout rendered to two decimals as the variable parts.
Emulator: the payload is `{'errorMessage': '<request id> Task timed out after 1.00 seconds', 'errorType': 'Sandbox.Timedout', 'requestId': '<id>', 'stackTrace': []}` — the `RequestId: ` and `Error: ` framing is missing from the message, and two keys AWS does not return are added.
Why: destination consumers parse responsePayload as Lambda's error document; the extra members change the stored artifact and the reframed message breaks any match on Lambda's timeout sentence, which is how a timeout is told apart from a handler exception.
Fix: the asynchronous timeout error-payload serializer in the lambda invoke path.
Traces: lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message.

Cause 3 of 4: the dead-letter ErrorMessage attribute is not truncated to the first kilobyte.
AWS: when Lambda dead-letters an asynchronous invocation it attaches the message attribute `ErrorMessage` carrying only the FIRST 1 KB (1024 characters) of the handler's error message; longer messages are cut at that boundary. Documented in the Lambda developer guide ('Dead-letter queues'); recorded behavior as far as the model goes.
Emulator: the full message is attached — a 3000-character error message arrives as 3000 characters.
Why: the 1 KB bound is part of the contract and keeps the DLQ message inside SQS's attribute budget; a consumer that slices or sizes on it gets a different payload, and a sufficiently long handler error can push the dead-letter message past limits AWS never lets it reach.
Fix: the lambda dead-letter publisher's ErrorMessage attribute construction — truncate to 1024 characters.
Traces: lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte.

Cause 4 of 4: the dead-letter ErrorCode attribute is sent with DataType String instead of Number.
AWS: the dead-letter message's `ErrorCode` attribute is a NUMBER attribute (`DataType: "Number"`) whose value is the invocation's status code; `RequestID` and `ErrorMessage` are String attributes. Documented in the Lambda developer guide ('Dead-letter queues'); recorded behavior as far as the model goes.
Emulator: the attribute is `{'StringValue': '200', 'DataType': 'String'}` — the right value with the wrong SQS attribute data type.
Why: SQS message-attribute data types are part of the wire contract: a consumer reading ErrorCode as a numeric attribute (or a filter policy matching numerically on it) does not see the attribute at all, and attribute-based routing of dead-lettered invocations silently stops matching.
Fix: the lambda dead-letter publisher's message-attribute builder — declare ErrorCode as a Number attribute.
Traces: lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes.

## Evidence
Cause 1 — lambda__sqs.on-success-destination-function-is-invoked-with-the-invocation-record: AssertionError: functionArn is 'arn:aws:lambda:<region>:<account>:function:<name>', expected the SOURCE function's $LATEST ARN. lambda__sqs.put-event-invoke-config-without-destinations-clears-the-previous-ones: record requestContext: {'requestId': ..., 'functionArn': 'arn:...:function:<name>', 'condition': 'Success', 'approximateInvokeCount': 1}. Same for .deleted-event-invoke-config-stops-destination-delivery-until-put-again and .alias-event-invoke-config-overrides-the-unqualified-config-for-alias-invokes.
Cause 2 — lambda__sqs.async-timeout-is-a-function-error-recorded-on-failure-with-the-timeout-message: the timeout responsePayload is {'errorMessage': '<id> Task timed out after 1.00 seconds', 'errorType': 'Sandbox.Timedout', 'requestId': '<id>', 'stackTrace': []}; expected exactly {"errorType": "Sandbox.Timedout", "errorMessage": "RequestId: <id> Error: Task timed out after 1.00 seconds"}.
Cause 3 — lambda__sqs.dead-letter-error-message-attribute-keeps-only-the-first-kilobyte: AssertionError: ErrorMessage must keep the first 1 KB (1024 chars) of a 3000-char message, got 3000 chars.
Cause 4 — lambda__sqs.dead-letter-queue-receives-the-original-event-with-request-id-and-error-attributes: AssertionError: ErrorCode attribute: {'StringValue': '200', 'DataType': 'String'}; expected DataType 'Number'.

## Steps to Reproduce
Cause 1: PutFunctionEventInvokeConfig with an OnSuccess SQS destination, Invoke with InvocationType Event, receive the record and read requestContext.functionArn (repeat qualified against an alias).
Cause 2: set Timeout=1, PutFunctionEventInvokeConfig(MaximumRetryAttempts=0, OnFailure=<queue>), invoke a handler that sleeps 10 s asynchronously, and compare the record's responsePayload.
Cause 3: configure a DeadLetterConfig queue, invoke asynchronously a handler that raises with a 3000-character message, and measure the ErrorMessage attribute's length.
Cause 4: with the same DLQ, invoke a failing handler asynchronously and inspect the ErrorCode message attribute's DataType.
Then replay the check ids in trace_ids.
