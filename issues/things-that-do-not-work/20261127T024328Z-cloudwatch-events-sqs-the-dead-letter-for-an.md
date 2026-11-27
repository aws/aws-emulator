Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations|run#29|AssertionError|assert _attr(m, "ERROR_CODE") == "NO_RESOURCE", (
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# cloudwatch/events/sqs: the dead letter for an alarm-state-change delivery denied by the target queue policy carries ERROR_CODE=NO_PERMISSIONS instead of NO_RESOURCE

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause, grounded in one failing check the suite lists under `cloudwatch__events__sqs`.

Cause 1 of 1: when an EventBridge rule on CloudWatch alarm state-change events delivers to an SQS queue whose resource policy explicitly Denies events.amazonaws.com, the dead letter is classified NO_PERMISSIONS, where AWS classifies it NO_RESOURCE. Failing check: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations.

AWS: EventBridge assumes no role for an SQS target — the send is authorized solely by the destination queue's resource policy, and the dead letter records the refusal SQS itself returned. Smithy (sqs): SendMessage's modeled errors are InvalidAddress (404), InvalidMessageContents (400), InvalidSecurity (403), the KMS.* family, QueueDoesNotExist (httpError 400, awsQueryError code `AWS.SimpleQueueService.NonExistentQueue`), RequestThrottled (403) and UnsupportedOperation (400) — no AccessDenied is modeled on SendMessage, and SQS hides a queue the caller may not write to behind the not-found class. The dead-letter message attributes are not modeled, so their values are recorded behavior only: ERROR_CODE is the fixed token `NO_RESOURCE`; ERROR_MESSAGE is the fixed sentence `The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)` — all fixed text except `<id>`, the per-call request id (a per-run value, not part of the requirement); RULE_ARN and TARGET_ARN carry the failing rule's and target's ARNs from state. An explicit Deny and a missing Allow are indistinguishable in this classification.

Emulator: the alarm state-change event is dead-lettered and the DLQ copy names the right rule, but it carries ERROR_CODE='NO_PERMISSIONS' and ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' — i.e. the NonExistentQueue sentence is paired with the wrong ERROR_CODE token, so the code and the message disagree about the failure class.

Why: ERROR_CODE is the field a redrive tool or operator branches on to decide whether the target is missing/unauthorized (NO_RESOURCE) or the delivery principal lacked permission it could be granted (NO_PERMISSIONS). A consumer keyed on NO_RESOURCE never fires for the commonest EventBridge→SQS misconfiguration, and a consumer that cross-checks ERROR_CODE against ERROR_MESSAGE sees a self-contradictory dead letter.

Fix: the EventBridge target-delivery failure path that classifies a failed send and fills the dead-letter ERROR_CODE attribute (it already emits the NonExistentQueue ERROR_MESSAGE on this path, so only the classifier token diverges). Do not flip the classifier globally — a target whose service genuinely answers AccessDenied keeps NO_PERMISSIONS; the discriminator is the refusal the target service actually returned. Context: open issue 20261123T070333Z-events-3-net-new-causes-an-unauthorized-sqs.md (Cause 1) describes the same classifier for a check under `events`; it does not carry this check's replay key, so this instance is filed here.

Traces: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations (assertion at check line 234).

## Evidence
cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations, line 234:
  assert _attr(m, "ERROR_CODE") == "NO_RESOURCE", ...
  AssertionError: the DLQ copy carries ERROR_CODE='NO_PERMISSIONS', expected 'NO_RESOURCE' for an SQS target whose policy denies events.amazonaws.com (ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <per-run id>; Proxy: null)')

Expected: ERROR_CODE='NO_RESOURCE' (fixed token) paired with that same NonExistentQueue sentence (400 / AWS.SimpleQueueService.NonExistentQueue), RULE_ARN/TARGET_ARN naming the failing rule and target.
Actual:   ERROR_CODE='NO_PERMISSIONS' with the correct NonExistentQueue message — code and message disagree.

Smithy (sqs) SendMessage errors: InvalidAddress(404), InvalidMessageContents(400), InvalidSecurity(403), KMS.*(400/403), QueueDoesNotExist(400, awsQueryError AWS.SimpleQueueService.NonExistentQueue), RequestThrottled(403), UnsupportedOperation(400).

The check stops at this assertion, so the downstream requirements it also carries (AWS/Events FailedInvocations and InvocationsSentToDlq attributed to the failing rule only, zero FailedInvocations on the healthy sibling rule, and restored delivery after the policy is repaired) were never graded in this run.

## Steps to Reproduce
1. PutMetricAlarm an alarm and arrange for it to change state (so CloudWatch publishes an `aws.cloudwatch` / 'CloudWatch Alarm State Change' event).
2. CreateQueue three times: a DLQ whose policy allows events.amazonaws.com sqs:SendMessage, a target queue whose policy explicitly Denies events.amazonaws.com, and a healthy sibling target queue that allows it.
3. PutRule twice (one rule per target) on the alarm event pattern; PutTargets with MaximumRetryAttempts=0 and DeadLetterConfig.Arn = the DLQ for the denied target.
4. Drive the alarm state change, then ReceiveMessage the DLQ with MessageAttributeNames=['All'].
   Expected: ERROR_CODE='NO_RESOURCE', ERROR_MESSAGE = SQS's NonExistentQueue sentence, RULE_ARN = the denied rule's ARN.
   Emulator: ERROR_CODE='NO_PERMISSIONS'.
