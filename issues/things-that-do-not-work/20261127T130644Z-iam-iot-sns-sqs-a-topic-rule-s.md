Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__iot__sns__sqs.iot-rule-sns-action-denied-publish-goes-to-the-error-action-until-the-role-is-granted iam__iot__sns__sqs.iot-rule-sns-action-publishes-only-the-selected-matching-mqtt-messages-to-the-topic
Issue-Key: iam__iot__sns__sqs.iot-rule-sns-action-denied-publish-goes-to-the-error-action-until-the-role-is-granted|_drain#25|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: iam__iot__sns__sqs.iot-rule-sns-action-publishes-only-the-selected-matching-mqtt-messages-to-the-topic|_receive#25|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# iam__iot__sns__sqs: a topic rule's sns action delivers nothing — neither the selected matching messages reach the topic nor does a role-denied publish reach the rule's errorAction

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopicRule

## Description
One net-new root cause, found by the behavioral suite under the `iam__iot__sns__sqs` service heading (2 checks). It is the same defect class as the one filed under the `iot__sns__sqs` heading, but it is a separate work item because these two checks are reported under a different service heading and are not carried by that issue.

Cause 1 of 1: the rules engine does not execute a rule's `sns` action, and it does not run the rule's errorAction when the action's role is not permitted to publish.
AWS: Smithy CreateTopicRule's TopicRulePayload carries `actions` (here an SnsAction {targetArn, roleArn, messageFormat}) and an optional `errorAction`; the behavior is documented at https://docs.aws.amazon.com/iot/latest/developerguide/sns-rule-action.html and the error-action behavior at https://docs.aws.amazon.com/iot/latest/developerguide/rule-error-handling.html. Recorded behavior: (a) a message ingested on a topic the rule's sql selects causes the engine to publish the SQL projection to the target SNS topic, which delivers it to the topic's SQS subscriber in a Notification envelope, and ONLY the selected matching messages are published; (b) while the action role's policy does not grant sns:Publish on the target topic, the action fails and the engine instead delivers a failure record to the rule's errorAction target (an SQS queue here), and once the role is granted the publish the normal delivery resumes.
Emulator: both checks create the IAM role, the SNS topic, the subscribed queue(s) and the rule (GetTopicRule reads the action back), publish on the rule's topic, and then poll their queues; no message ever arrives on either the subscriber queue or the error-action queue, so each check is cut off at the harness budget and reported as "Check timed out after 60.0s". No sns action is dispatched and no error action is dispatched.
Why: a client fanning IoT telemetry out through SNS receives nothing, and — worse for diagnosis — the errorAction that exists precisely to surface a failed action is silent too, so a misconfigured rule role is indistinguishable from a working rule. The authorization-dependent transition (denied -> errorAction, granted -> topic) that this pair of checks encodes can never be observed.
Fix: execute the sns action in the rule engine's action dispatch (assume roleArn, publish the SQL projection to targetArn, honour messageFormat) AND dispatch the rule's errorAction when an action fails authorization, instead of dropping the message silently (the emulator's topic-rule engine action dispatch and error-action path, plus services/service-com-amazonaws-iot-awsiotservice topic-rule state).
Traces: iam__iot__sns__sqs.iot-rule-sns-action-denied-publish-goes-to-the-error-action-until-the-role-is-granted, iam__iot__sns__sqs.iot-rule-sns-action-publishes-only-the-selected-matching-mqtt-messages-to-the-topic.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Expected = real-AWS behavior the checks encode; Actual = the emulator's answer. Generated names and the 60 s budget below are one run's values.

1. iam__iot__sns__sqs.iot-rule-sns-action-denied-publish-goes-to-the-error-action-until-the-role-is-granted — iam CreateRole trusted by iot.amazonaws.com (initially WITHOUT sns:Publish), sns CreateTopic, an SQS queue subscribed to the topic plus a second SQS queue as the rule's errorAction target, CreateTopicRule with the sns action and that errorAction; a message is published on the rule's topic and both queues are drained in a loop (receive_message with WaitTimeSeconds=2). Expected a failure record on the errorAction queue while the role lacks sns:Publish, and the notification on the subscriber queue after PutRolePolicy grants it; actual nothing arrives on either queue — "Check timed out after 60.0s".

2. iam__iot__sns__sqs.iot-rule-sns-action-publishes-only-the-selected-matching-mqtt-messages-to-the-topic — same wiring with a selecting sql/WHERE clause and MQTT publishes of matching and non-matching messages. Expected exactly the matching messages delivered to the topic's queue subscriber; actual no delivery at all — "Check timed out after 60.0s".

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and observe both checks named in Traces timing out.
3. Equivalently, by hand: iam CreateRole trusted by iot.amazonaws.com with NO sns:Publish yet; sns CreateTopic; sqs CreateQueue x2 (one subscribed to the topic, one as the rule's errorAction target); iot CreateTopicRule with an sns action on that topic/role and the second queue as errorAction; publish a matching message on the rule's topic; ReceiveMessage on the errorAction queue — observe nothing where AWS delivers a failure record. Then iam PutRolePolicy granting sns:Publish on the topic, publish again, and ReceiveMessage on the subscriber queue — observe nothing where AWS delivers an SNS Notification envelope.
