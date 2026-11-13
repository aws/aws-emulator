Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected
Issue-Key: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered|run#18|AssertionError|assert set(env) == _ENVELOPE_KEYS, (
Issue-Key: pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected|_rejected_error#16|AssertionError|raise AssertionError(
Issue-Commit: 82ad778ed253471d0ebb5d9587965556bfb040b0

# pipes__sqs: 2 net-new causes — the SQS record envelope a pipe delivers omits md5OfMessageAttributes, and CreatePipe accepts a scalar FilterCriteria pattern leaf

> _Note: this issue was identified against the code base at commit `82ad778ed253471d0ebb5d9587965556bfb040b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two net-new root causes, both in EventBridge Pipes, surfaced now that the pipes runtime actually polls and delivers (the two checks below previously timed out and are listed as replay keys on the still-open issue 20261029T193114Z-pipes-sqs-a-running-pipe-with-an-sqs.md, whose cause — "a RUNNING pipe consumes nothing and delivers nothing" — no longer reproduces; delivery happens, and these two divergences are what the delivered record and the refused create now show. Neither cause is covered by that issue's description, nor by 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md or 20261029T063205Z-pipes-data-plane-a-structured-body-filter-clause.md.)

Cause 1 of 2: the SQS record envelope a pipe puts on its target omits md5OfMessageAttributes.
AWS: recorded behavior only — the Smithy model describes only the control-plane surface (pipes CreatePipe takes Source, Target, RoleArn and SourceParameters.FilterCriteria.Filters[].Pattern / SourceParameters.SqsQueueParameters.BatchSize); it says nothing about the payload a pipe writes to its target. The recording (us-west-2) shows that, for an SQS source message that carries message attributes, the target message's Body is a JSON object with exactly these ten keys: attributes, awsRegion, body, eventSource, eventSourceARN, md5OfBody, md5OfMessageAttributes, messageAttributes, messageId, receiptHandle — the Lambda-ESM-shaped SQS record envelope, with the source body verbatim as a JSON string under body, the source's attributes lowercase-keyed under messageAttributes (dataType/stringValue), and md5OfMessageAttributes equal to the MD5OfMessageAttributes value SendMessage returned for the source message. md5OfMessageAttributes is present whenever the source record has message attributes (it is absent only for a record with none, exactly as in a Lambda SQS event record).
Emulator: the delivered envelope has nine keys — md5OfMessageAttributes is missing — while every other key, including the messageAttributes subtree itself, is present. Replay: "AssertionError: the pipe must deliver the SQS record envelope, not the bare message body; expected keys ['attributes', 'awsRegion', 'body', 'eventSource', 'eventSourceARN', 'md5OfBody', 'md5OfMessageAttributes', 'messageAttributes', 'messageId', 'receiptHandle'], got ['attributes', 'awsRegion', 'body', 'eventSource', 'eventSourceARN', 'md5OfBody', 'messageAttributes', 'messageId', 'receiptHandle']".
Why: a target consumer that validates the forwarded record's attribute integrity — the normal use of md5OfMessageAttributes, mirroring the Lambda SQS record — gets a KeyError/absent-field on every record a pipe forwards, and cannot detect attribute corruption in transit at all. Code written against the documented SQS record shape breaks on the emulator while working on AWS.
Fix: the pipes data-plane target serializer that builds the SQS record envelope at the polling end — it must carry md5OfMessageAttributes, computed from the source record's message attributes, alongside md5OfBody.
Traces: pipes__sqs.pipe-filter-criteria-non-matching-not-delivered

Cause 2 of 2: CreatePipe accepts a FilterCriteria event pattern whose leaf value is a bare scalar.
AWS: Smithy: CreatePipe (POST /v1/pipes/{Name}) models ValidationException with httpError 400; SourceParameters.FilterCriteria.Filters[].Pattern targets EventPattern (@length max 4096, @sensitive) and the model says nothing about the pattern grammar, so the grammar is recorded behavior only. AWS compiles the event pattern at create time: an EventBridge event pattern leaf must be an object or an array, so Pattern {"body": {"tag": "keep"}} is refused with HTTP 400, error code ValidationException and the message document '{"ValidationErrorFields":[{"Name":"Pattern","Message":"Invalid Event Pattern. Reason: \\"tag\\" must be an object or an array."}]}' — the fixed text is 'Invalid Event Pattern. Reason: ' + '<name> must be an object or an array.' wrapped in the ValidationErrorFields document under Name "Pattern"; the only variable part is the offending leaf's member name (here tag), taken from the request's pattern. The same call with the array form {"body": {"tag": ["keep"]}} is accepted and filters for real. Nothing is created by the refused call: ListPipes does not list the name, and DescribePipe on it raises NotFoundException (modeled, httpError 404) with the message 'Pipe <name> does not exist.' (fixed text, with the requested name substituted).
Emulator: the scalar-leaf CreatePipe is ACCEPTED with HTTP 200 and a full create response (Arn, Name, DesiredState RUNNING, CurrentState CREATING, CreationTime, LastModifiedTime), so the pipe is created and no ValidationException is raised. Replay: "AssertionError: create_pipe(...) was ACCEPTED where real AWS rejects it; response {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'CurrentState': 'CREATING', ...}" (the name, ARN and timestamps in that response are per-run values, not part of the requirement).
Why: the pattern language is over-permissive at the control plane. A client that ships a mis-typed filter leaf gets a pipe that AWS would have refused at deploy time; the emulator creates it and the mis-typed clause then behaves as a store-and-echo equality test or silently never matches, so a filtering bug only appears in production. It also means an emulator-only pipe exists under a name the client was told was rejected.
Fix: CreatePipe's FilterCriteria validation — compile each Filters[].Pattern with the EventBridge event-pattern grammar before any state is written, and refuse a non-object/non-array leaf with ValidationException carrying the ValidationErrorFields document above, creating nothing.
Traces: pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected

## Evidence
Cause 1 — pipes__sqs.pipe-filter-criteria-non-matching-not-delivered [mismatch]:
  Expected (recorded AWS, us-west-2): the target message Body parses to the ten-key SQS record envelope ['attributes','awsRegion','body','eventSource','eventSourceARN','md5OfBody','md5OfMessageAttributes','messageAttributes','messageId','receiptHandle'], md5OfMessageAttributes equal to the source SendMessage's MD5OfMessageAttributes.
  Actual (emulator): nine keys — md5OfMessageAttributes absent.
  Replay detail: AssertionError: the pipe must deliver the SQS record envelope, not the bare message body; expected keys [... 'md5OfBody', 'md5OfMessageAttributes', 'messageAttributes' ...], got [... 'md5OfBody', 'messageAttributes' ...]

Cause 2 — pipes__sqs.pipes-create-pipe-scalar-filter-pattern-rejected [mismatch]:
  Expected: CreatePipe with SourceParameters.FilterCriteria.Filters[0].Pattern = '{"body": {"tag": "keep"}}' answers HTTP 400, Code=ValidationException, Message='{"ValidationErrorFields":[{"Name":"Pattern","Message":"Invalid Event Pattern. Reason: \\"tag\\" must be an object or an array."}]}'; the name is absent from ListPipes and DescribePipe raises NotFoundException 'Pipe <name> does not exist.'
  Actual: HTTP 200 with {'Arn': 'arn:aws:pipes:<region>:<account>:pipe/<name>', 'Name': '<name>', 'DesiredState': 'RUNNING', 'CurrentState': 'CREATING', 'CreationTime': ..., 'LastModifiedTime': ...} — the pipe is created.
  Replay detail: AssertionError: create_pipe('<name>') was ACCEPTED where real AWS rejects it.

Both checks got past the pipe's RUNNING transition and (for cause 1) past delivery and filter suppression, so the pipes runtime is live in this run — these are not the timeout cause recorded on 20261029T193114Z-pipes-sqs-a-running-pipe-with-an-sqs.md.

## Steps to Reproduce
Cause 1:
1. Create an SQS source queue (VisibilityTimeout 60) and an SQS target queue, plus a role pipes.amazonaws.com can assume with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the target.
2. CreatePipe(Source=<source arn>, Target=<target arn>, RoleArn=<role>, SourceParameters={FilterCriteria:{Filters:[{Pattern:'{"body":{"kind":["wanted"]}}'}]}, SqsQueueParameters:{BatchSize:10}}); wait for DescribePipe CurrentState=RUNNING.
3. SendMessage on the source with body '{"kind":"wanted",...}' AND MessageAttributes={'probe':{DataType:'String',StringValue:'attr-good'}}.
4. ReceiveMessage on the target and json.loads the message Body: md5OfMessageAttributes is missing from the envelope (AWS includes it, equal to the source SendMessage's MD5OfMessageAttributes).

Cause 2:
1. With the same queues and role, CreatePipe using SourceParameters.FilterCriteria.Filters[0].Pattern = '{"body": {"tag": ["keep"]}}' (array form) — accepted, reaches RUNNING and filters (positive control).
2. CreatePipe a second pipe changing only that leaf to the scalar form '{"body": {"tag": "keep"}}'.
3. Expected HTTP 400 ValidationException with the ValidationErrorFields message above and nothing created; observed HTTP 200 and a created pipe (ListPipes lists it, DescribePipe succeeds).
