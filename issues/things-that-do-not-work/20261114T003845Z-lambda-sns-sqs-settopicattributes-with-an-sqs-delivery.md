Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.iam-trust-sns-wrong-principal-negative
Issue-Key: lambda__sns__sqs.iam-trust-sns-wrong-principal-negative|_set_feedback_role_rejected#19|Timeout|time.sleep(interval_s)
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda__sns__sqs: SetTopicAttributes with an SQS delivery-status feedback role never settles, so neither the accepted sns-trusting role nor the refused wrong-principal role can be observed

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
Cause 1 of 1: the SNS delivery-status feedback-role control path does not converge — the check that contrasts an accepted role with a refused one exhausts its whole budget.
AWS: SetTopicAttributes with AttributeName `SQSSuccessFeedbackRoleArn` (and its Failure/SampleRate siblings) accepts a role whose trust policy names the `sns.amazonaws.com` service principal and reads it back from GetTopicAttributes. A role whose trust policy names a different service principal (e.g. `ec2.amazonaws.com`) is refused on every attempt across the IAM propagation window — with AuthorizationError or InvalidParameter — and never appears in GetTopicAttributes.
Emulator: the check never reaches its assertions; it exhausts the 60 s budget ('Check timed out after 60.0s'), so neither the positive read-back nor the stable refusal of the wrong-principal role is produced. Because the refusal has to hold for every retry inside the propagation window, a call that neither succeeds nor refuses is indistinguishable from an accepted bad role.
Why: delivery-status logging is configured once and then relied on. A client that cannot get a decision out of SetTopicAttributes cannot tell whether its feedback role was accepted, and a role trusting the wrong principal may silently be left in place, so delivery-status records never appear and the misconfiguration is never reported.
Fix: the SNS SetTopicAttributes handler for the delivery-status feedback-role attributes — validate the supplied role's trust policy against the sns.amazonaws.com service principal, answer (accept or refuse) rather than hang, and reflect only accepted values in GetTopicAttributes.
Traces: lambda__sns__sqs.iam-trust-sns-wrong-principal-negative.

The open issue 20261102T093047Z-lambda-sns-sqs-settopicattributes-accepts-an-sqssuccessfeedbackrolearn-whose.md records the accepting-variant of this family; it does not carry this check's replay key.

## Evidence
lambda__sns__sqs.iam-trust-sns-wrong-principal-negative — Error: Check timed out after 60.0s.
Expected per AWS: SetTopicAttributes(SQSSuccessFeedbackRoleArn=<role trusting sns.amazonaws.com>) succeeds and GetTopicAttributes reads the ARN back; SetTopicAttributes with a role trusting ec2.amazonaws.com is refused (AuthorizationError or InvalidParameter) on every retry across the propagation window and the attribute stays absent.

## Steps to Reproduce
1. Create an SNS topic, an IAM role trusting sns.amazonaws.com and a second role trusting ec2.amazonaws.com.
2. SetTopicAttributes(TopicArn, AttributeName="SQSSuccessFeedbackRoleArn", AttributeValue=<sns-trusting role ARN>) and read it back with GetTopicAttributes.
3. Repeat with the ec2-trusting role across the IAM propagation window: AWS refuses every attempt and the attribute never appears; the emulator produces no decision before the check's budget expires.
