Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery
Issue-Key: cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery|run#22|AssertionError|assert bad_envelope is None, (
Issue-Commit: 48706b3c21ddfe25663178800b6a36ee362c124c

# cloudwatch__sns: a CFN-deployed AWS::SNS::TopicPolicy that omits SNS:Publish does not stop alarm-action delivery — the ALARM notification still reaches the topic's subscriber

> _Note: this issue was identified against the code base at commit `48706b3c21ddfe25663178800b6a36ee362c124c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: the CloudWatch alarm-action dispatcher publishes to its SNS topic without evaluating the topic's resource (access) policy, so a topic whose policy grants only SNS:Subscribe / SNS:Receive / SNS:GetTopicAttributes still receives and fans out the alarm notification.

AWS: an alarm action is an SNS Publish performed by the cloudwatch.amazonaws.com service principal, and SNS authorizes it against the topic's access policy. Smithy (sns 2010-03-31): Publish declares AuthorizationErrorException (awsQueryError code `AuthorizationError`, HTTP 403) exactly for a principal that the topic policy does not permit to publish. When an AWS::SNS::TopicPolicy replaces the default topic policy with statements that allow only `SNS:Subscribe`, `SNS:Receive` and `SNS:GetTopicAttributes`, nothing is published: the subscribed SQS queue stays empty for the whole observation window, and the alarm's own Action history records the failure rather than a delivery. The identical control stack, which keeps the default topic policy, DOES deliver — the policy statement list is the only difference between the two stacks. Recorded behavior only for the absence itself (there is no modelled response for "a notification that is not sent").

Emulator: both stacks deliver. After SetAlarmState(StateValue="ALARM") on the restricted stack's alarm, the SNS notification envelope arrives on the queue subscribed to the restricted topic, so the check's absence assertion fails with `AssertionError: expected NO delivery when the SNS topic's access policy omits Publish permission` after holding an absence window of at least 60 s (and at least as long as the positive control's measured latency). The positive control delivered correctly, so the alarm→SNS→SQS path works; what is missing is the authorization step on the publish.

Why: a customer's least-privilege topic policy is silently ineffective. Against the emulator, alerting appears healthy on a configuration that is mute in a real account; conversely a genuinely broken topic policy — the single most common reason alarm notifications never arrive — can never be reproduced or regression-tested. Any test that asserts "this topic cannot be published to" passes vacuously.

Fix: the CloudWatch alarm-action dispatcher (and the SNS Publish authorization path it calls) must evaluate the destination topic's resource policy before delivering, and refuse the publish with AuthorizationError when the policy does not allow Publish for cloudwatch.amazonaws.com with the alarm's SourceArn; the CloudFormation provider for AWS::SNS::TopicPolicy must write the PolicyDocument through to the topic's stored Policy attribute so the dispatcher sees it.

Related open issue, which does NOT carry this replay key: 20261029T182452Z-cloudwatch-sns-alarm-actions-cfn-alarms-are-never.md lists this check id under Cause 1 (CFN alarms were not created at all and SetAlarmState answered an empty-message ResourceNotFound). That symptom is gone — the stack now creates the alarm and SetAlarmState succeeds — and the check now fails one step later, on the unauthorized delivery. The topic-policy half of that issue's Cause 2 is the same family, but this check's key (an AWS::SNS::TopicPolicy whose statements omit Publish) is not registered on it, so it is filed here.

Traces: cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery.

## Evidence
Check cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery [mismatch]:
  File "check_cloudwatch__sns_cfn_alarm_missing_sns_topic_policy_no_delivery.py", line 53, in run
    assert bad_envelope is None, (
  AssertionError: expected NO delivery when the SNS topic's access policy omits Publish permission

Expected: after SetAlarmState(AlarmName=<restricted alarm>, StateValue="ALARM"), the queue subscribed to the topic carrying a TopicPolicy of {Effect: Allow, Principal: {AWS: "*"}, Action: ["SNS:Subscribe","SNS:Receive","SNS:GetTopicAttributes"]} receives nothing for the whole hold window (>= 60 s, and >= the control's measured delivery latency); the publish is refused with AuthorizationError (HTTP 403, awsQueryError code `AuthorizationError`).
Actual: the alarm notification envelope (a JSON body whose Message carries AlarmName=<restricted alarm>, NewStateValue="ALARM") is delivered to that queue, identically to the control stack whose topic keeps the default policy. (Stack/topic/queue/alarm names and timestamps are per-run; the divergence is that a delivery occurred at all.)

## Steps to Reproduce
1. CreateStack a template with AWS::SQS::Queue, AWS::SNS::Topic, AWS::SNS::Subscription (sqs → the queue), AWS::SQS::QueuePolicy allowing sns.amazonaws.com:SendMessage, and AWS::CloudWatch::Alarm with AlarmActions=[{"Ref": "Topic"}]; wait for CREATE_COMPLETE. This is the control.
2. CreateStack the same template PLUS an AWS::SNS::TopicPolicy on the same topic whose single statement allows only ["SNS:Subscribe","SNS:Receive","SNS:GetTopicAttributes"] (no SNS:Publish); wait for CREATE_COMPLETE.
3. cloudwatch SetAlarmState(StateValue="ALARM") on the restricted stack's alarm, then on the control stack's alarm.
4. Poll the control queue until the notification arrives (measures latency L), then poll the restricted stack's queue for max(60 s, L) and assert it stays empty.
Observed: the restricted queue also receives the ALARM notification.
