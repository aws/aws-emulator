Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__pipes__sqs.kinesis-target-pipe-without-stream-parameters-rejected kinesis__pipes__sqs.kinesis-target-partition-key-malformed-json-path-rejected kinesis__pipes__sqs.kinesis-target-partition-key-path-resolves-per-message-from-the-sqs-body kinesis__pipes__sqs.sqs-source-pipe-puts-each-message-to-a-kinesis-target-under-its-partition-key
Issue-Key: kinesis__pipes__sqs.kinesis-target-pipe-without-stream-parameters-rejected|_refusal#22|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: kinesis__pipes__sqs.kinesis-target-partition-key-malformed-json-path-rejected|_refusal#23|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: kinesis__pipes__sqs.kinesis-target-partition-key-path-resolves-per-message-from-the-sqs-body|run#32|AssertionError|assert len(ours) == len(_TENANTS), (
Issue-Key: kinesis__pipes__sqs.sqs-source-pipe-puts-each-message-to-a-kinesis-target-under-its-partition-key|_poll#9|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting until {what}; last={last!r}")
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# kinesis__pipes__sqs: 3 net-new causes — CreatePipe accepts a Kinesis target with no KinesisStreamParameters and with a malformed JSON-path PartitionKey, and a RUNNING SQS→Kinesis pipe delivers nothing to the stream

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Three distinct root causes in the EventBridge Pipes Kinesis-target path. All three are in the pipes service's CreatePipe validation and in the pipes SQS-source→Kinesis-target delivery loop.

Cause 1 of 3: CreatePipe accepts a Kinesis-stream Target with no TargetParameters.KinesisStreamParameters at all.
AWS: Smithy (services/service-com-amazonaws-pipes-pipes) models PipeTargetKinesisStreamParameters.PartitionKey as @required, and CreatePipe declares ValidationException (client, httpError 400) with members `message` and `fieldList` (ValidationExceptionField = {name, message}). A CreatePipe whose Target is a Kinesis stream ARN but whose TargetParameters omit KinesisStreamParameters is refused 400 ValidationException; the `Message` is itself a JSON document whose fixed shape is {"ValidationErrorFields":[{"Name":<field>,"Message":<reason>}]} and whose variable parts are the offending member path and its reason — here the fixed text is the field path `TargetParameters.KinesisStreamParameters` and the reason `Missing required target parameter.` The modelled `fieldList` carries the same pair lower-cased: [{"name":"TargetParameters.KinesisStreamParameters","message":"Missing required target parameter."}]. No pipe is created, so a following DescribePipe is refused NotFoundException.
Emulator: the call returns HTTP 200 with a full CreatePipeResponse (Arn/Name/DesiredState=STOPPED/CurrentState=CREATING) and a pipe is left behind, so the check's follow-up DescribePipe finds it rather than NotFoundException.
Why: a client that relies on CreatePipe rejecting an unconfigured Kinesis target gets a pipe that can never route (no PartitionKey), discovers the misconfiguration only as silent non-delivery, and its negative-path error handling (catching ValidationException and reading fieldList) never runs.
Fix: the CreatePipe handler's target-parameter validation in the pipes service — the target-type-vs-TargetParameters cross-check must run before any state is written, and must serialize ValidationException with both the JSON `message` document and the modelled `fieldList`.
Traces: kinesis__pipes__sqs.kinesis-target-pipe-without-stream-parameters-rejected

Cause 2 of 3: CreatePipe accepts a Kinesis target PartitionKey that begins as a JSON path but does not parse.
AWS: Smithy models PartitionKey as a @required, @sensitive string of length 0–256; the JSON-path form of a target parameter is validated at CreatePipe. A PartitionKey such as `$.body.order..id[` — it starts with `$.` so it is read as a path, and it does not parse — is refused 400 ValidationException with the same JSON `Message` envelope, field name `PartitionKey` and reason `Invalid JSON path <path> provided in the service target parameters.`, where the fixed text is `Invalid JSON path ` / ` provided in the service target parameters.` and the variable part is the submitted path echoed verbatim; `fieldList` carries [{"name":"PartitionKey","message":<same reason>}]. No pipe is created. A well-formed path (`$.body.order.id`) is accepted and stored verbatim.
Emulator: the malformed path is accepted — HTTP 200 CreatePipeResponse, CurrentState CREATING — and the pipe persists.
Why: malformed path templates are caught only at run time (or never), so a client sees records routed under a literal/garbage key instead of a create-time refusal naming the member it must fix.
Fix: JSON-path syntax validation for target parameters in the CreatePipe handler's validation layer of the pipes service (shared with the other path-valued target parameters), emitting ValidationException as above.
Traces: kinesis__pipes__sqs.kinesis-target-partition-key-malformed-json-path-rejected

Cause 3 of 3: a RUNNING pipe from an SQS queue to a Kinesis stream never puts anything on the stream.
AWS: recorded behavior only (the data plane is not modelled). The recording shows that once the pipe reports CurrentState RUNNING, each SQS message sent to the source queue is put on the target stream as one Kinesis record whose Data is the pipe's SQS record JSON (messageId, body, eventSourceARN...), keyed by the configured PartitionKey — either the literal value or, when PartitionKey is a JSON path such as `$.body.tenant`, the value that path resolves to in that individual message's body — and the delivered message then leaves the source queue (ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible both reach 0).
Emulator: the pipe reaches RUNNING but nothing ever arrives: GetRecords from TRIM_HORIZON on the target shard returns no matching record for the whole polling budget (`0 record(s) reached the stream for 3 messages: []`, and `timed out after 47.7s waiting until the message reaches the stream; last=[]`).
Why: any SQS→Kinesis fan-in built on Pipes silently loses every record; neither the per-message partition-key resolution nor the source-queue drain can be observed, so a client cannot distinguish a misconfigured pipe from a broken one.
Fix: the pipes data-plane delivery loop for the Kinesis-stream target (SQS source poller → Kinesis PutRecord/PutRecords), including PartitionKey resolution (literal vs JSON path evaluated per record) and the post-delivery DeleteMessage on the source queue.
Traces: kinesis__pipes__sqs.kinesis-target-partition-key-path-resolves-per-message-from-the-sqs-body, kinesis__pipes__sqs.sqs-source-pipe-puts-each-message-to-a-kinesis-target-under-its-partition-key

## Evidence
Cause 1 — expected: HTTP 400 ValidationException, Message = {"ValidationErrorFields":[{"Name":"TargetParameters.KinesisStreamParameters","Message":"Missing required target parameter."}]}, fieldList = [{"name":"TargetParameters.KinesisStreamParameters","message":"Missing required target parameter."}], then DescribePipe → NotFoundException. Actual: HTTPStatusCode 200 with {'Arn': 'arn:aws:pipes:<region>:<account>:pipe/<name>', 'Name': '<name>', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', ...} — AssertionError: expected a refusal, but the call succeeded.

Cause 2 — PartitionKey '$.body.order..id[' expected: HTTP 400 ValidationException, Message = {"ValidationErrorFields":[{"Name":"PartitionKey","Message":"Invalid JSON path $.body.order..id[ provided in the service target parameters."}]}, fieldList = [{"name":"PartitionKey","message":<same>}], no pipe created. Actual: HTTPStatusCode 200 CreatePipeResponse with CurrentState 'CREATING' — AssertionError: expected a refusal, but the call succeeded.

Cause 3 — expected: 3 Kinesis records (one per queued message), each PartitionKey equal to the `tenant` value in that message's own body, each Data carrying the message's messageId, and the source queue draining to 0/0. Actual: AssertionError: 0 record(s) reached the stream for 3 messages: []; and in the single-message variant AssertionError: timed out after 47.74732846300003s waiting until the message reaches the stream; last=[].

## Steps to Reproduce
1. Create a Kinesis stream (ShardCount=1), an SQS queue, and an IAM role trusting pipes.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the queue and kinesis:PutRecord/PutRecords on the stream.
2. Cause 1: CreatePipe(Name=..., RoleArn=..., Source=<queue ARN>, Target=<stream ARN>, DesiredState='STOPPED') with NO TargetParameters. Observe HTTP 200 instead of 400 ValidationException, then DescribePipe(Name=...) and observe the pipe exists.
3. Cause 2: CreatePipe with TargetParameters={'KinesisStreamParameters': {'PartitionKey': '$.body.order..id['}}. Observe HTTP 200 instead of 400 ValidationException naming PartitionKey.
4. Cause 3: CreatePipe with SourceParameters={'SqsQueueParameters': {'BatchSize': 10}} and TargetParameters={'KinesisStreamParameters': {'PartitionKey': '$.body.tenant'}}; wait for CurrentState RUNNING; SendMessageBatch three bodies {"tenant": "tenant-amber|tenant-birch|tenant-cedar", "run": "<marker>"}; then GetShardIterator(TRIM_HORIZON)+GetRecords on the stream's shard. Observe no records arrive and the source queue never drains.
