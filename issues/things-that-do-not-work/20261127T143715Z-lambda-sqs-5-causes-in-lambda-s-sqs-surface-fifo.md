Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.fifo-queue-as-dead-letter-target-rejected-and-the-standard-queue-keeps-receiving lambda__sqs.lambda-vendored-boto3-sends-to-sqs-by-default-endpoint-and-the-message-arrives lambda__sqs.sqs-esm-event-message-attributes-carry-list-value-members lambda__sqs.sqs-esm-exists-filter-invokes-only-bodies-with-the-leaf-and-deletes-the-rest lambda__sqs.sqs-esm-more-than-five-filters-rejected
Issue-Key: lambda__sqs.fifo-queue-as-dead-letter-target-rejected-and-the-standard-queue-keeps-receiving|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: lambda__sqs.lambda-vendored-boto3-sends-to-sqs-by-default-endpoint-and-the-message-arrives|_invoke#11|AssertionError|assert "FunctionError" not in response, (
Issue-Key: lambda__sqs.sqs-esm-event-message-attributes-carry-list-value-members|run#14|AssertionError|assert attr == {"stringValue": "order", "stringListValues": [], "binaryListValues": [],
Issue-Key: lambda__sqs.sqs-esm-exists-filter-invokes-only-bodies-with-the-leaf-and-deletes-the-rest|run#29|AssertionError|assert got == _EXPECTED, (
Issue-Key: lambda__sqs.sqs-esm-more-than-five-filters-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 5 causes in Lambda's SQS surface: FIFO DLQ accepted, broken vendored boto3, ESM record attributes, exists-filter leak and the 5-filter cap

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Five causes.

Cause 1 of 5: CreateFunction accepts a FIFO queue as the function's DeadLetterConfig target.
AWS: an asynchronous-invocation dead-letter target must be a standard queue or an SNS topic; a FIFO queue is refused with `InvalidParameterValueException` (Smithy: com.amazonaws.lambda CreateFunction error InvalidParameterValueException, HTTP 400) and the function is not created.
Emulator: CreateFunction returns HTTP 200 with `'DeadLetterConfig': {'TargetArn': 'arn:aws:sqs:<region>:<account>:<name>.fifo'}` echoed back and the function `State: 'Active'`.
Why: the client believes failed async invocations will land on the FIFO queue. AWS would have refused the configuration, so nothing is ever written there and the failures are lost.
Fix: CreateFunction/UpdateFunctionConfiguration validation of DeadLetterConfig.TargetArn (reject `.fifo` queues).
Traces: lambda__sqs.fifo-queue-as-dead-letter-target-rejected-and-the-standard-queue-keeps-receiving.

Cause 2 of 5: the managed Python runtime's vendored boto3/botocore cannot be imported.
AWS: a function on a managed `python3.x` runtime can `import boto3` with no dependency in its package, construct a default-endpoint SQS client and send a message successfully.
Emulator: the invocation fails with `Runtime.ImportModuleError` / errorType `Unhandled`: `Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)` — the runtime's bundled botocore is incompatible with the interpreter it runs on, so no handler that imports boto3 can start.
Why: the single most common Lambda pattern — a Python handler calling another AWS service through the vendored SDK — fails at import time for every function, before any user code runs.
Fix: the managed Python runtime image's vendored boto3/botocore (and the `http.client` shim it imports from).
Traces: lambda__sqs.lambda-vendored-boto3-sends-to-sqs-by-default-endpoint-and-the-message-arrives.

Cause 3 of 5: SQS event-source-mapping records omit the `stringListValues` and `binaryListValues` members of each message attribute.
AWS: recorded behavior only for the event shape — every user message attribute in the SQS event is rendered with `stringValue`, `dataType` and empty `stringListValues` and `binaryListValues` arrays.
Emulator: the record's attribute reads `{'dataType': 'String', 'stringValue': 'order'}`.
Why: typed SDK event models and hand-written handlers index those members unconditionally, so an ordinary message makes the handler throw and the whole batch is retried to the dead-letter queue.
Fix: the SQS event-source-mapping record serializer's messageAttributes projection.
The same defect is filed under the iam__lambda__logs__sqs heading for its own check id; an issue carries one service token, so the two headings are filed separately.
Traces: lambda__sqs.sqs-esm-event-message-attributes-carry-list-value-members.

Cause 4 of 5: an `exists` filter in the mapping's FilterCriteria matches an intermediate object node.
AWS: EventBridge/Lambda filter `exists` semantics match only when the named key is present with a leaf (scalar) value; a key whose value is an object or array does not match, so a message whose body has the key only as an intermediate node is filtered out and deleted from the queue without invoking the function.
Emulator: the function is invoked for the intermediate-node body as well — it saw `['hit-false', 'hit-string', 'miss-intermediate']` where only `['hit-false', 'hit-string']` may be delivered.
Why: the client's filter is more permissive than AWS's, so messages it expected to be dropped reach the handler; a handler written to assume the filter already excluded them mis-processes or throws on them.
Fix: the event-source-mapping FilterCriteria pattern matcher's `exists` operator (leaf-only semantics).
Traces: lambda__sqs.sqs-esm-exists-filter-invokes-only-bodies-with-the-leaf-and-deletes-the-rest.

Cause 5 of 5: CreateEventSourceMapping accepts more than five FilterCriteria filters.
AWS: a mapping's FilterCriteria may hold at most five filters; a sixth is refused with `InvalidParameterValueException` (Smithy: com.amazonaws.lambda CreateEventSourceMapping error InvalidParameterValueException, HTTP 400) and no mapping is created.
Emulator: the call returns HTTP 202 and the mapping is created with all six patterns echoed back in FilterCriteria.Filters.
Why: the client builds a filter set AWS will refuse and only discovers it on deploy; meanwhile the emulator's matching behaviour across six filters has no AWS counterpart to agree with.
Fix: CreateEventSourceMapping/UpdateEventSourceMapping validation of FilterCriteria.Filters length.
Traces: lambda__sqs.sqs-esm-more-than-five-filters-rejected.

## Evidence
Cause 1: `AssertionError: the call was expected to be refused but succeeded: {... 'DeadLetterConfig': {'TargetArn': 'arn:aws:sqs:us-west-2:000000000000:sim2real-xsvc-fifo-4b8a3a83cf5c.fifo'}, 'State': 'Active' ...}` (HTTP 200).
Cause 2: `AssertionError: <fn> raised 'Unhandled': {'errorMessage': "Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)", 'errorType': 'Runtime.ImportModuleError'}`.
Cause 3: `AssertionError: the record's messageAttributes.kind is {'dataType': 'String', 'stringValue': 'order'}; the documented record carries stringValue, dataType and empty stringListValues/binaryListValues`.
Cause 4: `AssertionError: the function saw ['hit-false', 'hit-string', 'miss-intermediate']; expected exactly ['hit-false', 'hit-string'] (leaked: ['miss-intermediate'], missing: [])`.
Cause 5: `AssertionError: expected the call to be refused, but it succeeded: {... 'HTTPStatusCode': 202 ..., 'FilterCriteria': {'Filters': [six patterns]} ...}`.

## Steps to Reproduce
Cause 1: CreateFunction with `DeadLetterConfig={'TargetArn': <fifo queue arn>}`.
Cause 2: deploy a python3.x function whose handler does `import boto3` and invoke it.
Cause 3: send a message with one String attribute through an SQS event-source mapping and inspect the record's messageAttributes.
Cause 4: configure FilterCriteria with an `exists: true` pattern on a nested key and publish a body where that key is an object; the function is invoked.
Cause 5: CreateEventSourceMapping with six entries in FilterCriteria.Filters.
Replay: the five check ids in trace_ids.
