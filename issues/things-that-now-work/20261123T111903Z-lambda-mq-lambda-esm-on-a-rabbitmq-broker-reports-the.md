Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle
Issue-Key: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: f3fcdc08bdb09b2741d5bd13ee7a493b1992f806

# Lambda ESM on a RabbitMQ broker reports the wrong LastProcessingResult text when the source cannot be consumed

> _Note: this issue was identified against the code base at commit `f3fcdc08bdb09b2741d5bd13ee7a493b1992f806`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEventSourceMapping

## Description
One net-new root cause.

Cause 1 of 1: the LastProcessingResult recorded for an Amazon MQ (RabbitMQ) event source mapping that cannot consume its source carries emulator-invented wording instead of AWS's fixed text. Failing check: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle.

AWS: recorded behavior only. The Lambda Smithy model documents the member only as EventSourceMappingConfiguration$LastProcessingResult — "The result of the event source mapping's last processing attempt." — and constrains neither its values nor its text, so the requirement comes from the recording. The recording (measured against real AWS, and quoted in the check's own notes) shows that a mapping whose RabbitMQ broker is RUNNING but whose queue was never declared transitions Creating -> Enabled and then reports LastProcessingResult exactly "PROBLEM: Failed to connect with event source." — that whole string is FIXED text with no variable parts (no queue name, broker id, host or timestamp is interpolated into it), and it stays unchanged for as long as the fault persists while State remains "Enabled". Both GetEventSourceMapping and ListEventSourceMappings report it.

Emulator: the mapping does reach Enabled and does record a PROBLEM-prefixed result, but the text is "PROBLEM: Connection error. Please check your event source connection configuration." — different fixed text for the same condition.

Why: a client that diagnoses a broken event source by matching LastProcessingResult against AWS's documented/observed string (operators, runbooks, and the diagnose-style tooling this check models all do exactly that) finds no match and concludes the mapping is healthy, or classifies the fault into the wrong bucket. The status field is the only durable record of the failure, so its wording is the whole contract here.

Fix: the Lambda event-source-mapping poller's status reporting — the value written into LastProcessingResult when an Amazon MQ / RabbitMQ source cannot be connected to or consumed from — and nothing in the serializer, which already surfaces whatever string state holds through Get/ListEventSourceMappings.

Traces: lambda__mq.e2e-rabbitmq-esm-absent-queue-delivers-nothing-lifecycle, phase 1/4 (deploy). Two existing open mq issues (20261118T001804Z-mq-describebrokerinstanceoptions-omits-the-published-rabbitmq-host-instance.md and 20261118T104000Z-mq-3-net-new-causes-createbroker-s-invalid.md) touch RabbitMQ brokers but neither covers the Lambda ESM status text, and neither carries this check's replay key.

## Evidence
Expected (real AWS, measured): GetEventSourceMapping(UUID=<stack output>) -> State "Enabled", LastProcessingResult "PROBLEM: Failed to connect with event source." (entire string fixed text), persisting unchanged.

Actual (emulator): State "Enabled", LastProcessingResult "PROBLEM: Connection error. Please check your event source connection configuration."

Replay assertion: AssertionError: [phase 1/4 deploy] the mapping's LastProcessingResult is 'PROBLEM: Connection error. Please check your event source connection configuration.'; a mapping to a running broker whose queue was never declared reports the connection failure

## Steps to Reproduce
1. Deploy a stack with an AWS::AmazonMQ::Broker (EngineType RABBITMQ) that reaches RUNNING, a Node.js consumer function, and an AWS::Lambda::EventSourceMapping whose Queues name a queue that is never declared on the broker's vhost, with BASIC_AUTH SourceAccessConfigurations pointing at the broker secret.
2. Wait for the mapping to leave Creating (AWS: ~75 s) and poll lambda GetEventSourceMapping(UUID=<mapping id>) until LastProcessingResult starts with "PROBLEM".
3. Compare LastProcessingResult to "PROBLEM: Failed to connect with event source." — the emulator answers "PROBLEM: Connection error. Please check your event source connection configuration." instead.
