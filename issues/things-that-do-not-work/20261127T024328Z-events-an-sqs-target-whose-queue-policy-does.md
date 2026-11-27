Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy|run#19|AssertionError|assert attrs.get("ERROR_CODE") == "NO_RESOURCE", f"dead-letter ERROR_CODE was {attrs.get('ERROR_CODE')!r}"
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# events: an SQS target whose queue policy does not authorize events.amazonaws.com dead-letters as NO_PERMISSIONS/403 AccessDenied instead of NO_RESOURCE/NonExistentQueue

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause, grounded in one failing check the suite lists under `events`.

Cause 1 of 1: the dead letter EventBridge writes when an SQS target's resource policy does not authorize the delivery is classified NO_PERMISSIONS, where AWS classifies it NO_RESOURCE. Failing check: events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy.

AWS: EventBridge assumes no role for an SQS target — the send is authorized solely by the destination queue's resource policy, and the dead letter records the refusal SQS itself returned. Smithy (sqs): SendMessage's modeled errors are InvalidAddress (404), InvalidMessageContents (400), InvalidSecurity (403), the KMS.* family, QueueDoesNotExist (httpError 400, awsQueryError code `AWS.SimpleQueueService.NonExistentQueue`), RequestThrottled (403) and UnsupportedOperation (400) — there is NO AccessDenied modeled on SendMessage, so a 403 AccessDenied in SQS's name is not a refusal SQS can emit; SQS hides a queue the caller may not write to behind the not-found class. The dead-letter message attributes themselves are not modeled, so their values are recorded behavior only: ERROR_CODE is the fixed token `NO_RESOURCE`; ERROR_MESSAGE is the fixed sentence `The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)` — every word fixed text except `<id>`, the per-call request id taken from the call (a per-run value, not part of the requirement); RULE_ARN and TARGET_ARN carry the failing delivery's rule and target ARNs from state. This holds identically whether the queue has no policy at all or a policy that explicitly Denies events.amazonaws.com, and whether the rule/target/queue-policy were created through the EventBridge API or deployed by a CloudFormation stack.

Emulator: the event IS dead-lettered and the DLQ copy arrives, but the attributes carry ERROR_CODE='NO_PERMISSIONS' (with the emulator-authored 403 AccessDenied sentence `User: events.amazonaws.com is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no resource-based policy allows the sqs:SendMessage action (Service: AmazonSQS; Status Code: 403; Error Code: AccessDenied; Request ID: <id>; Proxy: null)`). The queue ARN interpolated into that sentence has no place in this message class at all.

Why: ERROR_CODE/ERROR_MESSAGE are the only fields that tell an operator or a redrive tool why a delivery failed. A consumer that branches on NO_RESOURCE — the documented symptom of the commonest EventBridge→SQS misconfiguration — never fires, and one that parses the status/code pair out of the message reads 403/AccessDenied where AWS reports 400/AWS.SimpleQueueService.NonExistentQueue.

Fix: the EventBridge target-delivery failure path that classifies a failed send and fills the dead-letter ERROR_CODE/ERROR_MESSAGE attributes, together with the SQS SendMessage authorization refusal it records. CAUTION: do not flip the classifier globally — a target whose service genuinely answers AccessDenied (a role-assumed target, or KMS on an encrypted queue) must keep AccessDenied/NO_PERMISSIONS; the discriminator is which refusal the target service actually returns. Context: open issue 20261123T070333Z-events-3-net-new-causes-an-unauthorized-sqs.md (Cause 1) and open issue 20261115T165311Z-events-the-eventbridge-sqs-target-dead-letter-error.md describe this same classification for OTHER checks, and fixed issue 20261111T033241Z-events-eventbridge-sqs-target-dead-letters-misclassify-an-unauthorized.md claimed it; none of them carries this check's replay key, so this instance is still open and is reported here so the CFN-deployed-target arm is re-replayed.

Traces: events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy (assertion at check line 47).

## Evidence
events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy, line 47:
  assert attrs.get("ERROR_CODE") == "NO_RESOURCE", f"dead-letter ERROR_CODE was {attrs.get('ERROR_CODE')!r}"
  AssertionError: dead-letter ERROR_CODE was 'NO_PERMISSIONS'

Expected dead-letter attributes for a target queue whose policy does not authorize events.amazonaws.com:
  ERROR_CODE    = 'NO_RESOURCE' (fixed token)
  ERROR_MESSAGE = 'The specified queue does not exist or you do not have access to it. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>; Proxy: null)' — all fixed text except the per-run Request ID
  RULE_ARN / TARGET_ARN = the failing rule's and target's ARNs from state
Actual:
  ERROR_CODE    = 'NO_PERMISSIONS'
  ERROR_MESSAGE = the 403 AccessDenied sentence naming sqs:SendMessage on the target queue ARN

Smithy (sqs) SendMessage errors: InvalidAddress(404), InvalidMessageContents(400), InvalidSecurity(403), KMS.*(400/403), QueueDoesNotExist(400, awsQueryError AWS.SimpleQueueService.NonExistentQueue), RequestThrottled(403), UnsupportedOperation(400). No AccessDenied is modeled, so the emulator's 403/AccessDenied pair cannot come from SQS.

## Steps to Reproduce
1. CreateStack with a template containing an AWS::SQS::Queue used as a DLQ (policy allowing events.amazonaws.com sqs:SendMessage), a second AWS::SQS::Queue as the rule target with NO queue policy, and an AWS::Events::Rule whose target carries RetryPolicy and DeadLetterConfig.Arn = the DLQ. Wait for CREATE_COMPLETE.
2. PutEvents one entry matching the rule's pattern.
3. ReceiveMessage the DLQ with MessageAttributeNames=['All'] and read the dead letter's attributes.
   Expected: ERROR_CODE='NO_RESOURCE' and ERROR_MESSAGE = SQS's NonExistentQueue sentence (Status Code 400 / AWS.SimpleQueueService.NonExistentQueue), with RULE_ARN/TARGET_ARN naming the rule and target.
   Emulator: ERROR_CODE='NO_PERMISSIONS' with the 403 AccessDenied sentence.
4. UpdateStack adding the AWS::SQS::QueuePolicy that allows events.amazonaws.com and PutEvents again — delivery must succeed to the same queue, proving it existed all along.
