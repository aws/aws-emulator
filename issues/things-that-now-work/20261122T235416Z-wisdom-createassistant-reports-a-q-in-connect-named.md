Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-assistant-association-error-paths
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-assistant-association-error-paths
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.integrationConfiguration.topicIntegrationArn
Issue-Commit: e0fb349ad5587458fdb6b3f0cfb96430e488506b

# wisdom CreateAssistant reports a Q-in-Connect-named streaming topic for integrationConfiguration.topicIntegrationArn

> _Note: this issue was identified against the code base at commit `e0fb349ad5587458fdb6b3f0cfb96430e488506b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
One cause.

Cause 1 of 1: `wisdom` `CreateAssistant` answers 200 with the right status and the right body shape, but the single member `$.assistant.integrationConfiguration.topicIntegrationArn` does not carry the assistant's integrated streaming-topic ARN the fixture tracks — it carries an ARN the emulator mints for itself out of the OTHER service's topic-naming scheme. Fixture `wisdom-assistant-association-error-paths`, step `setup#1`.

AWS: Smithy (services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json) — `CreateAssistant` POST /assistants, success 200, output `CreateAssistantResponse.assistant` -> `AssistantData`; `AssistantData.integrationConfiguration` -> `AssistantIntegrationConfiguration`, documented "The configuration information for the Wisdom assistant integration."; `AssistantIntegrationConfiguration.topicIntegrationArn` -> `GenericArn` (@length 1..2048, pattern `^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$`), documented "The Amazon Resource Name (ARN) of the integrated Amazon SNS topic used for streaming chat messages." `CreateAssistantRequest` models NO member for the topic, so the value is entirely service-chosen: the Wisdom service provisions the assistant's streaming topic and reports the ARN of THAT topic, and the same ARN is what every later read of the assistant reports. The model says nothing further about how the topic is named, so for the rest this is recorded behavior only: the recording pins this member to the fixture-level variable `topic_arn` — the one ARN the fixture's remaining steps resolve the assistant's integrated topic against — rather than to a literal, precisely because the ARN is per-run.

Emulator: the response's `topicIntegrationArn` is an SNS ARN the emulator composes from the Amazon Q in Connect naming scheme — the topic is created in the in-process SNS as `qconnect-assistant-<assistantId>` and its ARN reported verbatim (services/service-com-amazonaws-qconnect-wisdomservice/src/state/chat_streaming_topic.rs:27-29, :38-60). So a request on the `wisdom` wire (endpointPrefix/sigv4Name `wisdom`, shared by both models) is answered by the Q-in-Connect assistant implementation, and the member it fills names a `qconnect-assistant-*` topic; the legacy Wisdom crate's own projection of the same member is hardcoded absent (`integration_configuration: None`, services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs:932-934, and `AssistantSummary` likewise at :951), so the two halves of this endpoint disagree about whether an assistant has an integrated topic at all. Replay reports `value_mismatch` on `$.assistant.integrationConfiguration.topicIntegrationArn`: the fixture's `topic_arn` binding is never satisfied by anything the emulator returns (expected `{{topic_arn}}`, got an `arn:aws:sns:<region>:<account>:qconnect-assistant-<assistantId>`). Fixed text of the requirement is the ARN FORM and its provenance — `arn:aws:sns:<region>:<account-id>:<topic-name>` naming the topic the Wisdom service provisioned for THIS assistant and resolvable in SNS; the region, account id, topic name and assistant id are per-run values from the request and the emulator's state and are NOT part of the requirement, so the literal ARN in the recording is not what must be reproduced.

Why: a client that reads `topicIntegrationArn` to subscribe to (or publish on) the assistant's chat-message stream is handed a topic that belongs to the Q in Connect naming scheme rather than the assistant's integrated Wisdom topic, and nothing else in the Wisdom surface reports that same ARN — `ListAssistants`/`GetAssistant` paths that go through the legacy projection report NO `integrationConfiguration` at all. So the member is both unstable across reads of one assistant and unusable as the handle the fixture (and a real client) resolves the stream against: the streaming integration cannot be wired up, and a correlation keyed on that ARN silently matches nothing.

Fix: the `CreateAssistant` handler's assistant projection and the streaming-topic provisioning it calls — one consistent topic identity per assistant, reported identically by create and by every later read, for whichever of the two co-hosted models answered the request. Make the legacy Wisdom projection report the same integrated topic the create provisioned instead of hardcoding it absent, and derive the topic name from the service that is answering rather than from the Q in Connect scheme.

Traces: wisdom-assistant-association-error-paths#setup#1 (CreateAssistant, 200 vs 200, `$.assistant.integrationConfiguration.topicIntegrationArn` value_mismatch).

## Evidence
Replay divergence (trace set simparity-wire-aws-v1), verdict=mismatch, service=wisdom, operation=CreateAssistant, fixture=wisdom-assistant-association-error-paths, step=setup#1, affected_count=1:

  expected_status 200, actual_status 200
  detail: $.assistant.integrationConfiguration.topicIntegrationArn: value_mismatch (expected '{{topic_arn}}', got 'arn:aws:sns:us-west-2:000000000000:qconnect-assistant-18fd856e-745b-3d86-2582-63fd4888ae1b')
  divergence_key: service=wisdom|CreateAssistant|status=200->200|keys=$.assistant.integrationConfiguration.topicIntegrationArn

Expected (AWS / recording): `$.assistant.integrationConfiguration.topicIntegrationArn` is the ARN of the SNS topic the Wisdom service provisioned for this assistant — form `arn:aws:sns:<region>:<account-id>:<topic-name>`, matching `GenericArn`'s pattern — and it is the value the fixture binds as `topic_arn` and resolves its later steps against; the same assistant reports the same ARN on every later read.

Actual (emulator): an ARN of a topic the emulator creates in the in-process SNS under the Q in Connect naming scheme, `qconnect-assistant-<assistantId>` (services/service-com-amazonaws-qconnect-wisdomservice/src/state/chat_streaming_topic.rs:27-29); the legacy Wisdom projection of the same member is hardcoded absent (services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs:932-934, :951), so no read of the assistant reproduces the value the create reported.

The region, account id, assistant id and topic name in the quoted strings are per-run environment values, not requirements; the diverging thing is which topic the member names and whether every read of the assistant agrees on it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (`simparity-wire-aws-v1`).
2. Replay fixture `wisdom-assistant-association-error-paths`, step `setup#1`: POST /assistants on the `wisdom` endpoint (restJson1, sigv4 service name `wisdom`) with a `CreateAssistantRequest` carrying `name` and `type=AGENT`.
3. The call answers 200. Read `$.assistant.integrationConfiguration.topicIntegrationArn` in the response body: it is `arn:aws:sns:<region>:<account>:qconnect-assistant-<assistantId>` — a topic named under the Q in Connect scheme — instead of the assistant's integrated Wisdom streaming topic the fixture binds as `topic_arn`.
4. Call `GetAssistant`/`ListAssistants` for the same assistant through the legacy Wisdom projection: `integrationConfiguration` is absent entirely, so no read reproduces the ARN the create reported.
