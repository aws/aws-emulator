Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end|run#30|Timeout|_publish_acked(publisher, source, json.dumps({"marker": marker}).encode())
Issue-Commit: 30d927f99e8e54f81e8ebf641df74b5be52ff769

# iot: a topic rule's republish action never delivers to a native MQTT subscriber — the rules-engine ingress→SQL→egress check reaches no verdict (times out)

> _Note: this issue was identified against the code base at commit `30d927f99e8e54f81e8ebf641df74b5be52ff769`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopicRule

## Description
Cause 1 of 1: the republish rules-engine path produces nothing observable at the iot:Data-ATS MQTT broker, so a publish to a rule's source topic never re-emerges on the rule's destination topic. Failing check id: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end.

AWS: a topic rule created by CreateTopicRule with a `republish` action re-emits every message PUBLISHed to the topic named in its SQL FROM clause onto the action's destination topic, after evaluating the rule SQL, and a native MQTT client subscribed to that destination topic receives the result. Smithy (com.amazonaws.iot): TopicRulePayload.actions -> Action.republish -> RepublishAction{roleArn (required, AwsArn), topic (required, TopicPattern), qos (Qos, @range min 0 max 1, default 0), headers (MqttHeaders)}; the rule SQL's `topic()` function is computed by the rules engine, so `SELECT *, topic() AS src FROM '<source>'` must yield exactly the published keys plus `src`, with `src` equal to the source topic. The model says nothing further about delivery timing — the rest is recorded behavior only: the recording shows the far-end subscriber receiving the republished body on the destination topic, and the original publish NOT reaching that subscriber directly.

Emulator: the check never reaches any of those assertions. The replay reports only `Error: Check timed out after 60.0s` — the emulator produced no republished message, and no step of the chain (DescribeEndpoint -> DNS resolve of the ATS host -> mTLS CONNECT/CONNACK on :8883 -> CreateTopicRule -> QoS 1 publish on the source topic -> delivery on the destination topic) completed far enough to yield an assertion failure within the budget. Every wait in the check is bounded and retried (180 s CONNACK, 180 s republish-arrival), so a stall rather than a refusal is what the emulator served. Hypothesis: the stall is at the same place the open MQTT issues describe (the listener at the published iot:Data-ATS address completes TLS but never answers CONNACK — see 20261117T200355Z-iot-the-published-iot-data-ats-mqtt-listener.md and 20261119T032013Z-iot-7-net-new-causes-the-published-iot.md) and/or the rules engine never executes a `republish` action at all (compare 20261029T175031Z-iot-cfn-deployed-aws-iot-topicrule-is-not.md and 20261112T065656Z-iot-lambda-the-in-function-iot-data-publish.md, which report rule actions never firing for other action types). None of those open issues carries this check's replay key, so this republish path is filed here; if the fix for the broker session also restores republish, re-replay this check to confirm.

Why: a client that wires fan-out/normalization through a topic rule's republish action sees its messages silently dropped — no error, no delivery, no diagnosis. Because the failure is a hang rather than a refusal, device and integration code that waits on the destination topic blocks until its own timeout instead of failing fast, and any test or deployment gated on rules-engine egress never completes.

Fix: the rules-engine action executor for `republish` (evaluate the rule's SQL against the inbound message, including `topic()`, and publish the result to the action's `topic` at the action's `qos`), and the MQTT broker egress/session path at the published iot:Data-ATS endpoint that must carry both the inbound PUBLISH on the source topic and the outbound delivery to the subscriber.

Traces: iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end (mismatch; `Check timed out after 60.0s`).

## Evidence
Expected (real AWS, per the recording the check encodes): after CreateTopicRule(ruleName=<rule>, topicRulePayload={sql: "SELECT *, topic() AS src FROM '<source>'", awsIotSqlVersion: '2016-03-23', actions:[{republish:{roleArn:<role>, topic:'<dest>', qos:1}}]}), a native mTLS MQTT client subscribed to '<dest>' with QoS 1 (SUBACK granted [1]) receives, within the check's 180 s bounded retry, a JSON body whose key set is exactly {marker, src} where marker is the value published to '<source>' and src == '<source>'.

Actual (emulator): no message ever arrives on '<dest>'. The check produces no assertion output at all — the suite records `[mismatch] iot.mqtt-topic-rule-republish-reaches-a-native-subscriber-at-the-far-end  Error: Check timed out after 60.0s`, i.e. the chain stalled rather than returning any status or error code. No HTTP status, error code or error message was served for the stalled step; the divergence is the absence of the republished delivery.

## Steps to Reproduce
1. iot.DescribeEndpoint(endpointType='iot:Data-ATS') and wait for the returned host to resolve.
2. Provision an mTLS device identity: CreateThing, CreatePolicy allowing iot:Connect/Publish/Subscribe/Receive on '*', CreateKeysAndCertificate(setAsActive=true), AttachPolicy, AttachThingPrincipal.
3. iam.CreateRole assumable by iot.amazonaws.com with iot:Publish on arn:aws:iot:<region>:<account>:topic/<dest>.
4. iot.CreateTopicRule(ruleName=<rule>, topicRulePayload={sql: "SELECT *, topic() AS src FROM '<source>'", awsIotSqlVersion: '2016-03-23', actions:[{republish:{roleArn:<role-arn>, topic:'<dest>', qos:1}}]}).
5. Connect a native MQTT 3.1.1 client over mTLS to the ATS host on port 8883 and SUBSCRIBE to '<dest>' at QoS 1.
6. Connect a second mTLS MQTT client and PUBLISH {"marker": "<marker>"} to '<source>' at QoS 1.
7. Observe the subscriber's inbox for '<dest>': expected a body {marker: '<marker>', src: '<source>'}; the emulator delivers nothing and the check hangs until the harness budget expires.
