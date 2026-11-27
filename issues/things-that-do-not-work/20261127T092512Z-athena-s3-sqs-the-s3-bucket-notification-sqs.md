Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__sqs.s3-bucket-notification-to-sqs-needs-queue-permission-then-delivers-athenas-result-object
Issue-Key: athena__s3__sqs.s3-bucket-notification-to-sqs-needs-queue-permission-then-delivers-athenas-result-object|_collect_notification#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# athena__s3__sqs: the S3-bucket-notification → SQS delivery of Athena's result object reaches no verdict — the check times out at 60s

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: the S3 event-notification path that must deliver a message to SQS when Athena writes its result object never completes inside the check's whole 60-second budget, so the differential (refused without a queue policy, delivered once the policy allows s3.amazonaws.com) reaches no graded assertion at all.

AWS: recorded behavior only (this is a cross-service event path, not a single modeled response). S3 PutBucketNotificationConfiguration with a QueueConfiguration for s3:ObjectCreated:* under the prefix "results/" is refused while the destination queue's policy does not allow s3.amazonaws.com to SendMessage, and is accepted once it does; thereafter an object written under that prefix — here the CSV result object Athena writes for a query whose OutputLocation is s3://<bucket>/results/ — produces an s3:ObjectCreated event that is delivered to the queue and is readable with ReceiveMessage within seconds.
Emulator: no step of the flow reaches a verdict — the check is killed with "Check timed out after 60.0s", i.e. the emulator never completes the configure → write → deliver → receive sequence inside the budget. Nothing in the failure output names an AWS error code or a non-2xx response, so the divergence is the absence of the delivery (or of one of the calls returning at all), not a wrong response body.
Why: a client that wires Athena's result bucket to a queue to drive downstream processing gets no event, so every consumer of "a new result landed" stalls indefinitely; and because the configure-time refusal is never observed either, a missing queue policy is not reported as the misconfiguration it is.
Fix: the S3 bucket-notification subsystem — the PutBucketNotificationConfiguration destination-policy validation and the event emission on object creation that must enqueue to the SQS target — together with whatever makes the sequence not terminate within the budget. Hypothesis: the object-created event is never emitted for the object Athena itself writes, so the ReceiveMessage poll spins until the check's deadline.
Traces: athena__s3__sqs.s3-bucket-notification-to-sqs-needs-queue-permission-then-delivers-athenas-result-object

## Evidence
Suite verdict for the check: [mismatch] with detail "Error: Check timed out after 60.0s" — the only output; no HTTP status, error code or response body was reached, so no step of the configure/write/deliver/receive flow produced a graded assertion. Expected: the PutBucketNotificationConfiguration refusal while the queue policy denies s3.amazonaws.com, then acceptance, then an s3:ObjectCreated message for the results/ prefix readable from the queue after the Athena query writes its result object.

## Steps to Reproduce
Re-run the check in trace_ids. Create an S3 bucket and an SQS queue; call s3 PutBucketNotificationConfiguration with a QueueConfiguration (Events ['s3:ObjectCreated:*'], Filter prefix 'results/', QueueArn = the queue) before granting the queue a policy allowing s3.amazonaws.com:SendMessage and observe the refusal; add that policy and repeat to observe acceptance; then run an Athena query whose OutputLocation is s3://<bucket>/results/ to SUCCEEDED and poll ReceiveMessage for the s3:ObjectCreated event naming the result object.
