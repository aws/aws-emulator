Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sns__sqs__textract.textract-notification-channel-naming-a-queue-instead-of-a-topic-rejected
Issue-Key: iam__s3__sns__sqs__textract.textract-notification-channel-naming-a-queue-instead-of-a-topic-rejected|_notes#31|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 8c63d5b7f8677a460355a4ac20e9a08b37d56b80

# Textract asynchronous jobs never publish their completion status to the NotificationChannel SNS topic, so no status ever reaches a subscribed SQS queue

> _Note: this issue was identified against the code base at commit `8c63d5b7f8677a460355a4ac20e9a08b37d56b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDocumentTextDetection

## Description
This issue reports 1 root cause, grounded in 1 failing cross-service check.

Cause 1 of 1 — an asynchronous Textract job started with a NotificationChannel stores the channel but never publishes the job's completion status to the named Amazon SNS topic, so a subscriber (here an SQS queue subscribed to that topic) receives nothing.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): StartDocumentTextDetection input {DocumentLocation required, ClientRequestToken, JobTag, NotificationChannel, OutputConfig, KMSKeyId} / output {JobId}; NotificationChannel is documented "The Amazon Simple Notification Service (Amazon SNS) topic to which Amazon Textract publishes the completion status of an asynchronous document operation", with required members SNSTopicArn ("The Amazon SNS topic that Amazon Textract posts the completion status to", pattern ^arn:([a-z\d-]+):sns:...$) and RoleArn (the IAM role "that gives Amazon Textract publishing permissions to the Amazon SNS topic"). Recorded behavior: once the job leaves IN_PROGRESS, Textract publishes ONE message to that topic whose payload is a JSON document carrying at least JobId (the id the Start* call returned), Status ('SUCCEEDED' for a job that completed), API ('StartDocumentTextDetection' for this operation), JobTag (echoed from the request when given) and DocumentLocation ({'S3Bucket': <the request's bucket>, 'S3ObjectName': <the request's key>}) — all variable parts taken from the request and the job's own outcome. Exactly one status per job is published, and only for jobs that were actually started.
Emulator: nothing is ever published. StartDocumentTextDetection accepts the request, mints a JobId and stores the channel on the job — services/service-com-amazonaws-textract-textract/src/ops/start_document_text_detection.rs assigns `notification_channel: input.notification_channel` onto the Job (around line 369) and the job settles by a timer (`settles_at`), with no publish anywhere on the path; `notification_channel` is only ever stored in the state struct (services/service-com-amazonaws-textract-textract/src/state.rs, Job.notification_channel) and never read to send anything. The check consequently spun in its completion-status poll until the harness cut it off: 'Check timed out after 60.0s', having already got past its refusal assertion on the queue-ARN request.
Why: the documented event-driven pattern for Textract's asynchronous API (Start* -> SNS -> SQS/Lambda -> Get*) cannot work against the emulator. A client that waits for the completion notification instead of polling Get* hangs forever, and there is no error or status anywhere to tell it why; this silently breaks every pipeline built the way AWS's own async documentation prescribes.
Fix: the asynchronous job completion path of the textract crate — where a job reaches its terminal outcome (the settling logic in src/state.rs feeding the Get* handlers, driven from the Start* handlers in src/ops/start_document_text_detection.rs and src/ops/start_document_analysis.rs) must, for a job that carries a NotificationChannel, publish the completion-status document to the named SNS topic through this emulator's own Amazon SNS (the same in-process cross-service path the document read already uses to reach Amazon S3), exactly once per job, with the API/JobId/JobTag/Status/DocumentLocation members filled from the job.
Dependency note: the same check's control job submits a PDF document, which this emulator refuses to read; that PDF defect is filed separately as Cause 3 of the textract issue 'Textract document-limit refusals and PDF reading diverge ...' and must also be fixed before this check can pass end to end. No divergence was observed on the check's first assertion (the NotificationChannel naming the SQS queue's ARN as SNSTopicArn was refused as expected), so nothing is claimed about it here.
Traces: iam__s3__sns__sqs__textract.textract-notification-channel-naming-a-queue-instead-of-a-topic-rejected.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 22/39 passed (56.4%), 10 mismatched, 7 errored. bootstrap_signal false. Per-service: iam__s3__sns__sqs__textract 0/1 (0%), 1 failed.

[mismatch] iam__s3__sns__sqs__textract.textract-notification-channel-naming-a-queue-instead-of-a-topic-rejected
Error: Check timed out after 60.0s

What the check does, in order: creates an IAM role trusted by textract.amazonaws.com with sns:Publish on the topic; creates a bucket and PutObject of a PDF at 'in/receipt.pdf'; creates an SQS queue and an SNS topic, sets the queue policy for sns.amazonaws.com and subscribes the queue to the topic; calls StartDocumentTextDetection with NotificationChannel={'SNSTopicArn': <queue ARN>, 'RoleArn': ...} and asserts ('InvalidParameterException', 'Request has invalid parameters') — this assertion did NOT fire, so the refusal matched; then, as the control, calls StartDocumentTextDetection with NotificationChannel={'SNSTopicArn': <topic ARN>, 'RoleArn': ..., JobTag='topic-arn'} and polls ReceiveMessage on the queue for the completion status (up to 120 s per attempt, 4 attempts).

Expected: within seconds, one SNS message on the queue whose body is the job's completion status — JobId equal to the returned JobId, Status 'SUCCEEDED', API 'StartDocumentTextDetection', JobTag 'topic-arn', DocumentLocation {'S3Bucket': <the check's bucket>, 'S3ObjectName': 'in/receipt.pdf'} — and no status at all for the refused request's JobTag.
Actual: no message of any kind reached the queue; the poll ran until the harness's 60 s budget expired. The emulator's Start* handler stores NotificationChannel on the job and no code path publishes to SNS, so the queue can never receive a status. The ARNs, bucket and JobTag values are this run's; the requirement is one published status per job carrying those members from the request and the job's outcome.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, cross-service group.
2. Create an SNS topic and an SQS queue, set the queue's policy to allow sns.amazonaws.com sqs:SendMessage, and subscribe the queue to the topic (protocol 'sqs').
3. Create an IAM role trusted by textract.amazonaws.com with sns:Publish on that topic, and PutObject a document into a bucket.
4. Call StartDocumentTextDetection(DocumentLocation={'S3Object': {'Bucket': b, 'Name': k}}, NotificationChannel={'SNSTopicArn': <topic ARN>, 'RoleArn': <role ARN>}, JobTag='topic-arn'); note the JobId.
5. Poll GetDocumentTextDetection(JobId) until it leaves IN_PROGRESS — the job settles — then ReceiveMessage on the queue: no message ever arrives, where AWS delivers exactly one completion status carrying JobId, Status, API='StartDocumentTextDetection', JobTag and DocumentLocation {'S3Bucket','S3ObjectName'}.
6. Repeat with StartDocumentAnalysis to confirm the same gap on the other asynchronous path, and confirm that a job started WITHOUT a NotificationChannel publishes nothing.
7. After the fix, re-replay iam__s3__sns__sqs__textract.textract-notification-channel-naming-a-queue-instead-of-a-topic-rejected and require it to pass end to end (it also needs the PDF read path from the separately filed textract PDF cause, since its control document is a PDF whose results must hold non-empty LINE text).
