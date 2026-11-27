Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.eventbridge-dlq-on-target-failure events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters
Issue-Key: events__s3__sqs.eventbridge-dlq-on-target-failure|run#19|AssertionError|assert dlq_attrs.get("ERROR_CODE", {}).get("StringValue") == "NO_RESOURCE", (
Issue-Key: events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq|run#20|AssertionError|assert (error_code, error_message) == ("NO_RESOURCE", expected_message), (
Issue-Key: cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed|run#32|AssertionError|assert attrs.get("ERROR_CODE") == "NO_RESOURCE", (
Issue-Key: ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters|run#38|AssertionError|assert got_attrs.get("ERROR_CODE") == "NO_RESOURCE", (
Issue-Commit: 44d4304dbd56e0f97705f387ff3fc6793140eb84

# EventBridge dead-letters a delivery to an SQS target with no queue policy as ERROR_CODE='NO_PERMISSIONS' with a 403 AccessDenied ERROR_MESSAGE, instead of AWS's NO_RESOURCE with SQS's 400 NonExistentQueue refusal

> _Note: this issue was identified against the code base at commit `44d4304dbd56e0f97705f387ff3fc6793140eb84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
There is 1 root cause in this issue, grounded in 4 failing checks listed by the suite under four different scenario headings (events__s3__sqs, events__lambda__sqs, cloudwatch__events__s3__sqs, ec2__events__sqs). The service token recorded here is the heading of the representative check.

Placement note: those headings are composite scenario tokens, so the per-service bootstrap gate cannot place them by exact equality against the bootstrapped token list; get_bootstrap_status reports an EMPTY pending set, so neither events nor sqs (nor s3/lambda/cloudwatch/ec2) is un-bootstrapped and this is filed rather than deferred.

Note for the fixer: three open issues describe the SAME misclassification from OTHER checks — 20261114T222509Z-events-a-cloudformation-deployed-eventbridge-sqs-target-with.md, 20261115T153528Z-elbv2-events-sqs-an-eventbridge-sqs-target-with.md and 20261115T200918Z-cloudwatch-events-sqs-an-alarm-configuration-change-delivery.md (that last one OWNS a different check in this same run). NONE of them carries the replay keys of the four checks below, so those checks would never be re-replayed by them and are filed here. If one of those issues is fixed first, re-replay the four checks listed in trace_ids to close this one; it is the same classifier reached through four more paths (a custom-bus rule, a plain rule with DeadLetterConfig, an S3->EventBridge rule, and an EC2 instance-state-change rule with an InputTransformer).

Cause 1 of 1: a rule target that is an SQS queue whose resource policy does NOT admit events.amazonaws.com is dead-lettered with ERROR_CODE='NO_PERMISSIONS' and an emulator-authored 403 AccessDenied ERROR_MESSAGE, where AWS dead-letters it as ERROR_CODE='NO_RESOURCE' carrying SQS's own 400 NonExistentQueue refusal.
  AWS: EventBridge assumes no role for an SQS target — the send is authorized solely by the destination queue's resource policy, and what EventBridge classifies is the refusal the queue returns. Smithy (sqs): SendMessage's modeled errors are InvalidAddress (404), InvalidMessageContents (400), InvalidSecurity (403), the KMS.* family (400/403), QueueDoesNotExist (httpError 400, awsQueryError code 'AWS.SimpleQueueService.NonExistentQueue'), RequestThrottled (403) and UnsupportedOperation (400) — there is NO AccessDenied/AccessDeniedException modeled on SendMessage at all; SQS hides a queue it will not let the caller write to behind the not-found class. The dead-letter message attributes are not modeled, so their values are recorded behavior: ERROR_CODE='NO_RESOURCE' (a fixed enumerated token — nothing in it comes from the request), ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' — every word of that sentence is fixed text except the Request ID, which is the per-call id SQS minted — plus RULE_ARN and TARGET_ARN naming the failed delivery's rule and target (values from state) and the dead-lettered body being the event itself. Granting the queue policy afterwards restores delivery, proving the queue existed all along.
  Emulator: the event IS dead-lettered, with the right body, TARGET_ARN and RULE_ARN, but ERROR_CODE='NO_PERMISSIONS' and ERROR_MESSAGE='User: events.amazonaws.com is not authorized to perform: sqs:SendMessage on resource: <queue arn> because no resource-based policy allows the sqs:SendMessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)' — an IAM-style 403 AccessDenied sentence that SQS does not model for SendMessage, classified as a permissions failure instead of the not-found class.
  Why: a client that triages, alarms on or re-drives from the EventBridge DLQ taxonomy reads the wrong class for the commonest EventBridge->SQS misconfiguration (a missing queue policy). Any consumer branch keyed on NO_RESOURCE, or grepping SQS's NonExistentQueue sentence out of ERROR_MESSAGE, is dead code against this emulator; and the 403/AccessDenied wording leaks an authorization decision AWS deliberately hides behind the not-found class.
  Fix: EventBridge's target-delivery failure classifier that fills the dead-letter ERROR_CODE/ERROR_MESSAGE message attributes, together with the SQS SendMessage authorization refusal it consumes — an unauthorized send must surface as SQS's QueueDoesNotExist (400 / AWS.SimpleQueueService.NonExistentQueue / that fixed sentence) and be classified NO_RESOURCE, while a target whose service genuinely answers AccessDenied (a role-assumed target, or KMS on an encrypted queue) stays NO_PERMISSIONS. Do not flip the classifier globally.
  Traces: events__s3__sqs.eventbridge-dlq-on-target-failure, events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq, cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed, ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters.

## Evidence
events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq (the check asserts both attributes at once):
  AssertionError: expected DLQ message attributes ERROR_CODE=NO_RESOURCE / ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' for a target queue with no resource policy, got 'NO_PERMISSIONS' / 'User: events.amazonaws.com is not authorized to perform: sqs:SendMessage on resource: arn:aws:sqs:us-west-2:000000000000:sim2real-q-<id> because no resource-based policy allows the sqs:SendMessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)'

events__s3__sqs.eventbridge-dlq-on-target-failure:
  AssertionError: expected ERROR_CODE=NO_RESOURCE on the DLQ message, got {'TARGET_ARN': {...}, 'ERROR_CODE': {'StringValue': 'NO_PERMISSIONS', 'DataType': 'String'}, 'RULE_ARN': {...}, 'ERROR_MESSAGE': {'StringValue': 'User: events.amazonaws.com is not authorized to perform: sqs:SendMessage ... (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)', ...}}

cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed:
  AssertionError: dead letter carries ERROR_CODE='NO_PERMISSIONS', expected 'NO_RESOURCE' (all attributes: {'TARGET_ARN': ..., 'ERROR_CODE': 'NO_PERMISSIONS', 'RULE_ARN': ..., 'ERROR_MESSAGE': 'User: events.amazonaws.com is not authorized to perform: sqs:SendMessage ... Status Code: 403; Error Code: AccessDenied ...'})

ec2__events__sqs.xsvc-ec2-1-state-change-transformed-delivery-while-rejected-target-dead-letters:
  AssertionError: a queue that grants EventBridge nothing is dead-lettered with ERROR_CODE 'NO_RESOURCE'; got {..., 'ERROR_CODE': 'NO_PERMISSIONS', 'ERROR_MESSAGE': 'User: events.amazonaws.com is not authorized to perform: sqs:SendMessage ... Status Code: 403; Error Code: AccessDenied ...'}

Per attribute, expected vs actual (ReceiveMessage on the DLQ with MessageAttributeNames=['All']):
  ERROR_CODE    expected 'NO_RESOURCE'                        actual 'NO_PERMISSIONS'
  ERROR_MESSAGE expected 'The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' (all fixed text; only the Request ID varies)   actual the 403 AccessDenied sentence above
  RULE_ARN / TARGET_ARN  matched (they name the failed delivery's rule and target)
  dead-lettered body     matched (the event itself)
Everything before the attribute assertions matched in all four checks: the rule and target read back as declared with RetryPolicy/DeadLetterConfig, the positive-control queue (whose policy admits events.amazonaws.com) received the event, and the policy-less target queue held no message.

## Steps to Reproduce
1. CreateQueue 'target' (NO resource policy at all) and CreateQueue 'dlq' with a Policy allowing events.amazonaws.com sqs:SendMessage on the DLQ only.
2. PutRule on the default or a custom bus with an EventPattern matching a probe event, State ENABLED; PutTargets with one target = the 'target' queue ARN plus RetryPolicy {MaximumRetryAttempts: 0..3} and DeadLetterConfig {Arn: <dlq arn>}.
3. PutEvents one matching entry (FailedEntryCount 0). The same divergence reproduces when the event is produced by S3 -> EventBridge (Object Created) or by an EC2 instance-state-change rule with an InputTransformer, and whether or not the rule/target were deployed by CloudFormation.
4. ReceiveMessage on the DLQ with MessageAttributeNames=['All'].
   Expected: ERROR_CODE='NO_RESOURCE'; ERROR_MESSAGE='The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)'; RULE_ARN=<the rule>; TARGET_ARN=<the target queue>; body = the event.
   Emulator: ERROR_CODE='NO_PERMISSIONS' and ERROR_MESSAGE='User: events.amazonaws.com is not authorized to perform: sqs:SendMessage on resource: <target arn> because no resource-based policy allows the sqs:SendMessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)'.
5. Directly: SendMessage to a queue as a principal no resource-based policy admits must be refused 400 AWS.SimpleQueueService.NonExistentQueue 'The specified queue does not exist or you do not have access to it.', never 403 AccessDenied (SendMessage models no AccessDenied error).
6. The policy-less target queue must hold no message; after SetQueueAttributes adds a Policy allowing events.amazonaws.com sqs:SendMessage, a further PutEvents must be DELIVERED to it (proving the queue existed all along).
