Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-connector-event-unknown-connector
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-connector-event-unknown-connector iot-managed-integrations-connector-event-validation-errors iot-managed-integrations-managed-thing-command-validation
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|SendConnectorEvent|4xx->4xx|$.Message
Issue-Key: iot-managed-integrations|SendManagedThingCommand|4xx->4xx|$.Message
Issue-Commit: 4866adc0993f8fa67cc0ae020b08b6d4f21694d6

# ValidationException messages diverge from AWS wording for enum and null-member violations

> _Note: this issue was identified against the code base at commit `4866adc0993f8fa67cc0ae020b08b6d4f21694d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendConnectorEvent

## Description
The emulator's iot-managed-integrations input-validation layer builds ValidationException (HTTP 400) messages with the right fixed scaffolding but the wrong rendering of the violated constraint. Two distinct causes, both in the shared message formatter. HTTP status (400) and error shape (ValidationException) are correct in both; only the $.Message text diverges, and the message text is a requirement.

Cause 1 of 2: enum-value-set list is rendered in Smithy declaration order instead of AWS's order.
AWS: Smithy: SendConnectorEvent.Operation targets enum ConnectorEventOperation, @required; modeled error ValidationException (httpError 400, member Message). The model does not specify the message text, so the wording is recorded behavior only. The recording shows AWS answering 400 with Message = "1 validation error detected: Value at 'operation' failed to satisfy constraint: Member must satisfy enum value set: [DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST, DEVICE_COMMAND_RESPONSE]". Fixed text: "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]". Variable parts: the error count, the lowerCamel wire member name taken from the request (here 'operation'), and the comma-space-joined permitted value list for that member. The permitted value list AWS emits for ConnectorEventOperation is DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST, DEVICE_COMMAND_RESPONSE, in that order.
Emulator: answers 400 ValidationException with the same fixed text but the list in the Smithy model's declaration order: "... enum value set: [DEVICE_COMMAND_RESPONSE, DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST]". Hypothesis: the formatter iterates the generated enum's member order rather than a fixed per-enum ordering captured from the service.
Why: clients and test suites that assert on, diff, or parse the enum list out of the validation message (a common way to surface the allowed values to a user) see a different set ordering than AWS returns, so byte-level and string-equality assertions on the error message fail against the emulator while passing against AWS.
Fix: the operation's validation layer for iot-managed-integrations - the ValidationException message formatter that renders the "Member must satisfy enum value set" constraint - must emit the service's enum value ordering rather than the model's declaration ordering.
Traces: iot-managed-integrations-connector-event-unknown-connector#task#2 (representative); also iot-managed-integrations-connector-event-unknown-connector#task#2, iot-managed-integrations-connector-event-validation-errors#task#1, iot-managed-integrations-connector-event-validation-errors#task#5.

Cause 2 of 2: required-member-null message interpolates the literal "null" as a value.
AWS: Smithy: SendManagedThingCommand.Endpoints targets CommandEndpoints, @required, @length min 1 max 5; modeled error ValidationException (httpError 400). Message text is recorded behavior only. The recording shows AWS answering 400 with Message = "1 validation error detected: Value at 'endpoints' failed to satisfy constraint: Member must not be null". Fixed text: "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must not be null". Variable parts: the error count and the wire member name taken from the request (here 'endpoints'). Note that for a missing/null member AWS renders NO value token between "Value" and "at" - the value is deliberately absent, unlike constraint violations on a present value, where the value is interpolated.
Emulator: answers 400 ValidationException with "1 validation error detected: Value null at 'endpoints' failed to satisfy constraint: Member must not be null" - it interpolates the absent value as the literal word "null", inserting a token AWS never emits for this constraint class.
Why: a client matching or asserting on AWS's null-member validation message does not match the emulator's, so required-field error handling and negative tests that compare the message diverge; any consumer extracting the offending value from the message reads a bogus "null" value.
Fix: the operation's validation layer for iot-managed-integrations - the same ValidationException message formatter - must omit the value token entirely for the "Member must not be null" constraint instead of rendering the absent value.
Traces: iot-managed-integrations-managed-thing-command-validation#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, 126 steps, 95 passed) reported both clusters as verdict=mismatch with expected_status=400 and actual_status=400, diverging only on $.Message.

Cause 1 - cluster key service=iot-managed-integrations|SendConnectorEvent|status=400->400|keys=$.Message, 4 affected steps:
  expected $.Message: "1 validation error detected: Value at 'operation' failed to satisfy constraint: Member must satisfy enum value set: [DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST, DEVICE_COMMAND_RESPONSE]"
  actual   $.Message: "1 validation error detected: Value at 'operation' failed to satisfy constraint: Member must satisfy enum value set: [DEVICE_COMMAND_RESPONSE, DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST]"
  The actual order matches the Smithy model's declaration order for ConnectorEventOperation (smithy_query shape ConnectorEventOperation -> values [DEVICE_COMMAND_RESPONSE, DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST]), confirming the formatter reuses declaration order.

Cause 2 - cluster key service=iot-managed-integrations|SendManagedThingCommand|status=400->400|keys=$.Message, 1 affected step:
  expected $.Message: "1 validation error detected: Value at 'endpoints' failed to satisfy constraint: Member must not be null"
  actual   $.Message: "1 validation error detected: Value null at 'endpoints' failed to satisfy constraint: Member must not be null"
  Difference is the inserted " null" token after "Value".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: POST /connector-event/{ConnectorId} (SendConnectorEvent) with an Operation value outside the ConnectorEventOperation enum. Observe 400 ValidationException; compare $.Message's bracketed enum list against AWS's order [DEVICE_DISCOVERY, DEVICE_EVENT, DEVICE_COMMAND_REQUEST, DEVICE_COMMAND_RESPONSE]. Fixtures: iot-managed-integrations-connector-event-unknown-connector step task#2, iot-managed-integrations-connector-event-validation-errors steps task#1 and task#5.
3. Cause 2: POST /managed-things-command/{ManagedThingId} (SendManagedThingCommand) omitting the required Endpoints member. Observe 400 ValidationException; compare $.Message against AWS's "1 validation error detected: Value at 'endpoints' failed to satisfy constraint: Member must not be null" - the emulator inserts " null" after "Value". Fixture: iot-managed-integrations-managed-thing-command-validation step task#3.
