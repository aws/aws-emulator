Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__sns__sqs.iot-rule-sns-action-publishes-the-sql-projection-to-a-queue-subscriber iot__sns__sqs.iot-rule-where-clause-withholds-non-matching-messages-from-the-sns-topic
Issue-Key: iot__sns__sqs.iot-rule-sns-action-publishes-the-sql-projection-to-a-queue-subscriber|_drain#21|Timeout|msgs = sqs.receive_message(
Issue-Key: iot__sns__sqs.iot-rule-where-clause-withholds-non-matching-messages-from-the-sns-topic|_drain#18|Timeout|msgs = sqs.receive_message(
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# iot__sns__sqs: a topic rule's sns action never publishes to the target SNS topic, so no notification reaches the topic's queue subscriber and neither the SQL projection nor the WHERE-clause filtering can be observed

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopicRule

## Description
One net-new root cause, found by the behavioral suite under the `iot__sns__sqs` service heading (2 checks).

Cause 1 of 1: the rules engine does not execute a rule's `sns` action — a message ingested on the rule's topic produces no SNS publish at all.
AWS: Smithy CreateTopicRule's TopicRulePayload.actions accepts an SnsAction {targetArn, roleArn, messageFormat}; the behavior is documented at https://docs.aws.amazon.com/iot/latest/developerguide/sns-rule-action.html. Recorded behavior: with a rule whose sql is `SELECT <projection> FROM '<topic filter>'`, ruleDisabled false and a single sns action with messageFormat RAW and a role the rule engine can assume for sns:Publish, a message ingested on a topic matching the filter causes the engine to publish the rule's SQL PROJECTION (not the raw payload) to the target topic. SNS then delivers it to the topic's SQS subscriber inside the standard envelope: Body.Type == "Notification", Body.TopicArn == the rule's target topic, and Body.Message == the JSON projection (only the selected fields, e.g. a selected field plus topic(4) aliased as a device name; unselected fields are absent). A message that does not satisfy the rule's WHERE clause produces no publish, so the subscriber receives nothing for it.
Emulator: GetTopicRule shows the action stored correctly (its sns.targetArn is the created topic), and the data-plane Publish to the matching topic is accepted, but no notification ever arrives at the subscribed queue: both checks poll the queue (receive_message with WaitTimeSeconds=2, re-publishing up to six times) and never read a message, so each is cut off at the harness budget and reported as "Check timed out after 60.0s". The positive delivery is what never happens; the negative (non-matching message withheld) is therefore also unverifiable.
Why: a client wiring IoT telemetry to SNS fan-out gets silence — no notification, no envelope, and no signal that the action failed. The two behaviors the sns action exists to provide (publishing the SQL projection rather than the raw payload, and publishing only for messages the WHERE clause selects) are both unobservable, so every downstream subscriber of an IoT rule is dead.
Fix: execute the sns action in the rule engine's action dispatch — assume the action's roleArn, publish the rule's SQL projection to targetArn honouring messageFormat (RAW vs JSON), and evaluate the WHERE clause before dispatching (the emulator's topic-rule engine action dispatch plus services/service-com-amazonaws-iot-awsiotservice topic-rule state; the SNS->SQS subscription leg itself delivers for direct sns Publish, so the gap is on the IoT side). Related but distinct open/resolved issues name other actions only: 20261029T175031Z-iot-cfn-deployed-aws-iot-topicrule-is-not.md (CFN-deployed rules and an HTTP destination) and the resolved republish-action issue 20261120T113301Z.
Traces: iot__sns__sqs.iot-rule-sns-action-publishes-the-sql-projection-to-a-queue-subscriber, iot__sns__sqs.iot-rule-where-clause-withholds-non-matching-messages-from-the-sns-topic.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Expected = real-AWS behavior the checks encode; Actual = the emulator's answer. Generated names/ids and the 60 s budget below are one run's values.

1. iot__sns__sqs.iot-rule-sns-action-publishes-the-sql-projection-to-a-queue-subscriber — CreateTopic, a queue subscribed to it, an IAM role trusted by iot.amazonaws.com with sns:Publish on the topic, then CreateTopicRule(sql="SELECT temp, topic(4) AS device FROM 'sim2real/<run>/sensors/+'", awsIotSqlVersion='2016-03-23', ruleDisabled=False, actions=[{sns:{targetArn, roleArn, messageFormat:'RAW'}}]). GetTopicRule confirms the stored action targets the topic. iot-data Publish(topic='sim2real/<run>/sensors/dev7', qos=1, payload={"temp":41,"humidity":77,"serial":<run>}) is accepted, repeated up to six times, each followed by up to 30 s of receive_message polling. Expected an SNS envelope on the queue with Type 'Notification', TopicArn == the rule's target topic and Message == {"temp": 41, "device": "dev7"}; actual no message ever arrives — "Check timed out after 60.0s" (the check's own assertion would be "no notification reached the queue subscriber after six MQTT publishes").

2. iot__sns__sqs.iot-rule-where-clause-withholds-non-matching-messages-from-the-sns-topic — same wiring with a WHERE clause; the matching publish is expected to produce exactly one notification and the non-matching one none. Actual: the matching publish produces nothing either, so the check never reaches its verdict — "Check timed out after 60.0s".

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and observe both checks named in Traces timing out.
3. Equivalently, by hand: sns CreateTopic; sqs CreateQueue and sns Subscribe the queue to the topic; iam CreateRole trusted by iot.amazonaws.com with an inline sns:Publish policy on the topic; iot CreateTopicRule with sql "SELECT temp, topic(4) AS device FROM 'sim2real/<run>/sensors/+'" and one sns action (messageFormat RAW) targeting that topic; iot-data Publish to 'sim2real/<run>/sensors/dev7' with {"temp":41,"humidity":77}; then sqs ReceiveMessage on the queue — observe no message at all where AWS delivers an SNS Notification envelope whose Message is {"temp":41,"device":"dev7"}. Verify the SNS->SQS leg independently with a direct sns Publish to the same topic.
