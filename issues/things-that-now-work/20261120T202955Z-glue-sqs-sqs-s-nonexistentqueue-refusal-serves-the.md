Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.sdk-glue-job-wrong-queue-url-negative
Issue-Key: glue__sqs.sdk-glue-job-wrong-queue-url-negative|run#18|AssertionError|assert error_bodies[0] == expected_report, (
Issue-Commit: c92e3b432fbf96aacd88dca0401270508fc4377e

# glue__sqs: SQS's NonExistentQueue refusal serves the JSON-protocol message on the query/awsQuery wire too, so a query-protocol client (an in-Glue-job boto3) never sees AWS's "...does not exist for this wsdl version." sentence

> _Note: this issue was identified against the code base at commit `c92e3b432fbf96aacd88dca0401270508fc4377e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
One net-new cause: the emulator serves a single NonExistentQueue message for every wire protocol, but AWS words that message differently on the legacy SQS query (awsQuery) protocol than on the JSON protocol.

Cause 1 of 1: the AWS.SimpleQueueService.NonExistentQueue message is protocol-independent in the emulator.
  AWS:      Recorded behavior only — the SQS Smithy model carries the error shape and its code, not per-protocol message text. Real SQS refuses an operation naming a queue that does not exist with HTTP 400 and error code 'AWS.SimpleQueueService.NonExistentQueue' on BOTH wires, but with two different messages: on the modern JSON protocol the message is the fixed text 'The specified queue does not exist.', while on the legacy query/awsQuery protocol (spoken by the older botocore pinned into the Glue 3.0 pythonshell runtime, and by any client that still negotiates the query wire) the message is the fixed text 'The specified queue does not exist for this wsdl version.'. Both are wholly fixed text — no queue name, URL, account id or region appears in either; which of the two is served depends only on the protocol of the incoming request.
  Emulator: serves 'The specified queue does not exist.' on both wires. The JSON-protocol half is now correct — the suite's in-process precheck (GetQueueUrl on the nonexistent queue over JSON) passes with exactly that message. The query-protocol half is not: a boto3 client inside a Glue pythonshell job run, calling SendMessage against a queue URL that resolves to no queue, observes code 'AWS.SimpleQueueService.NonExistentQueue' with message 'The specified queue does not exist.' where AWS gives 'The specified queue does not exist for this wsdl version.'. This looks like the over-reach of the fixes recorded in the resolved issues 20261115T165311Z-sqs-the-aws-simplequeueservice-nonexistentqueue-refusal-carries-the.md, 20261115T171209Z-sqs-the-queuedoesnotexist-refusal-carries-the-legacy-wsdl.md and 20261115T163456Z-cloudwatch-sns-sqs-sendmessage-to-a-deleted-queue-carries-the-legacy.md: those correctly removed the legacy wording from the JSON wire, but it was removed from the query wire as well, where AWS still emits it. (This is NOT a reopen of those: the call is correctly REFUSED with the correct code; only the query-protocol wording is wrong. The many open 'legacy WSDL wording' issues listed under query 'sqs' describe the opposite direction on the JSON wire and do not cover this.)
  Why:      any client still on the SQS query protocol — notably application code running on pinned older botocore/boto3, which is exactly what AWS's own Glue 3.0 pythonshell runtime ships — gets a message AWS never sends it. Code and operators that string-match the refusal to distinguish "queue genuinely gone" from other 400s take the wrong branch, and job logs/DLQ reports written from that message diverge from production. It also means the emulator's SQS error serializer has no notion that message text can be protocol-scoped, so the same flattening will hit any other error whose query-wire wording differs.
  Fix:      the SQS NonExistentQueue refusal's message selection in the error serializer / protocol layer — it must choose the wording from the request's negotiated protocol (query/awsQuery vs JSON) rather than from one global constant, and every queue-resolving operation (SendMessage, GetQueueUrl, GetQueueAttributes, SetQueueAttributes, ReceiveMessage, DeleteMessage, DeleteQueue, PurgeQueue, …) must go through that selection.
  Traces:   check glue__sqs.sdk-glue-job-wrong-queue-url-negative. The check first confirms over the JSON wire that GetQueueUrl on the nonexistent queue gives code 'AWS.SimpleQueueService.NonExistentQueue' and message 'The specified queue does not exist.' (passes), then runs a Glue pythonshell job whose boto3 SendMessage to the same nonexistent URL reports the error back through a real queue; the reported message is the JSON-wire sentence.

## Evidence
Check glue__sqs.sdk-glue-job-wrong-queue-url-negative

Precheck, JSON wire (passes today):
  GetQueueUrl(QueueName=<nonexistent>) -> HTTP 400, Code 'AWS.SimpleQueueService.NonExistentQueue', Message 'The specified queue does not exist.'

In-job call, query/awsQuery wire (pinned Glue 3.0 pythonshell boto3):
  SendMessage(QueueUrl=<same-account/region URL naming no queue>, MessageBody='should-not-arrive')
  Expected (AWS): Code 'AWS.SimpleQueueService.NonExistentQueue', Message 'The specified queue does not exist for this wsdl version.'
  Actual (emulator): Code 'AWS.SimpleQueueService.NonExistentQueue', Message 'The specified queue does not exist.'

Assertion: "expected the job to observe 'error:AWS.SimpleQueueService.NonExistentQueue|The specified queue does not exist for this wsdl version.', got 'error:AWS.SimpleQueueService.NonExistentQueue|The specified queue does not exist.'"

Fixed text vs values: both candidate messages are 100% fixed text — neither embeds the queue name, the queue URL, the account id or the region. The divergence is the trailing clause ' for this wsdl version' and the position of the period, selected purely by request protocol. The positive controls in the same run pass: the real queue receives 'control-ok' and the job run reaches JobRunState SUCCEEDED, and the error code is correct, so only the query-wire message text diverges.

## Steps to Reproduce
1. Boot the emulator.
2. sqs create-queue --queue-name <good> ; note its QueueUrl. Build <bad> by replacing the queue name in that URL with a name that was never created.
3. JSON wire control: with a current botocore, sqs get-queue-url --queue-name <bad-name>. Observe code AWS.SimpleQueueService.NonExistentQueue and message 'The specified queue does not exist.' (correct today).
4. Query wire: issue SendMessage for <bad> over the SQS query/awsQuery protocol — e.g. from a Glue pythonshell job run (create an IAM role trusted by glue.amazonaws.com with sqs:SendMessage, upload a script that calls boto3 sqs.send_message on <good> then on <bad>, catches ClientError and sends 'error:<Code>|<Message>' to <good>, then glue create-job --command Name=pythonshell,PythonVersion=3.9 --glue-version 3.0 and glue start-job-run), or by posting a form-encoded Action=SendMessage request directly.
5. Receive from <good> and read the reported error.
6. Observe the message is 'The specified queue does not exist.' where AWS's query wire gives 'The specified queue does not exist for this wsdl version.'

Replay with: glue__sqs.sdk-glue-job-wrong-queue-url-negative
