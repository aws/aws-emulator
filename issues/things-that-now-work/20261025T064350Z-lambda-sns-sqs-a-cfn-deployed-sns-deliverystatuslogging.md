Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# lambda__sns__sqs: a CFN-deployed SNS DeliveryStatusLogging role is never assumed — no delivery-status log entry ever lands and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: SNS never exercises a topic's delivery-status logging role, so a successful SQS delivery produces no delivery-status log entry.
AWS: recorded behavior only (the effect is an out-of-band log write, not a modelled response). The recording shows: a CloudFormation stack declaring an AWS::IAM::Role that trusts sns.amazonaws.com for sts:AssumeRole (with logs:CreateLogGroup/CreateLogStream/PutLogEvents) and an AWS::SNS::Topic whose DeliveryStatusLogging sets SuccessFeedbackRoleArn to that role and SuccessFeedbackSampleRate '100' for the sqs protocol makes SNS itself assume the role and write a delivery-status entry for each successful delivery into the log group named sns/<region>/<account-id>/<topic-name>. That log entry is the proof the trust policy was exercised by the service rather than merely persisted.
Emulator: after the stack deploys, a real SQS queue is subscribed and a message is published, no such log group, log stream or delivery-status entry ever appears; the check polls for it and is killed at the 60.0 s harness cap.
Why: a client that relies on SNS delivery-status logging for observability (confirming deliveries, auditing which role SNS assumed, alarming on failure feedback) sees an empty log forever, with no error to indicate the feature is inert — it reads as 'no deliveries happened'.
Fix: SNS delivery-status logging in the subscription delivery path (assume the configured SuccessFeedbackRoleArn and emit the delivery-status entry into sns/<region>/<account-id>/<topic-name> at the configured sample rate), plus the CloudFormation AWS::SNS::Topic DeliveryStatusLogging property mapping onto the topic's feedback-role/sample-rate attributes.
Related open issue that does not carry this replay key: 20261025T022656Z-lambda-sns-sqs-sns-sqs-to-lambda-delivery.md covers SNS/SQS-to-Lambda delivery in the same check group; this check asserts delivery-status logging for an sqs-protocol subscription.
Traces: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive.

## Evidence
Expected: after the stack deploys and a message is published to the topic with a confirmed SQS subscription, SNS assumes the deployed FeedbackRole and a delivery-status entry for that delivery appears in CloudWatch Logs under log group sns/<region>/<account-id>/<topic-name> (SuccessFeedbackSampleRate '100', protocol sqs).
Actual: [mismatch] Error: Check timed out after 60.0s — the delivery-status log group/stream/entry never appears, so the poll never terminates.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator.
2. Deploy the stack from the check: AWS::IAM::Role trusting sns.amazonaws.com with logs:CreateLogGroup/CreateLogStream/PutLogEvents, plus AWS::SNS::Topic with DeliveryStatusLogging [{Protocol: sqs, SuccessFeedbackRoleArn: !GetAtt FeedbackRole.Arn, SuccessFeedbackSampleRate: '100'}].
3. Create an SQS queue, subscribe it to the topic, Publish a message, then poll CloudWatch Logs for log group sns/<region>/<account-id>/<topic-name> and its delivery-status entry.
4. Observe no log group/entry and the check being killed at 60.0 s. Re-replay lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive to confirm the fix.
