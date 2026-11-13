Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-destination-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-destination-lifecycle iot-wireless-device-profile-lifecycle iot-wireless-network-analyzer-configuration-lifecycle iot-wireless-wireless-gateway-lifecycle iot-wireless-multicast-group-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|CreateDestination|4xx->4xx|$.Message
Issue-Key: iotwireless|CreateDeviceProfile|4xx->4xx|$.Message
Issue-Key: iotwireless|CreateNetworkAnalyzerConfiguration|4xx->4xx|$.Message
Issue-Key: iotwireless|CreateWirelessGateway|4xx->4xx|$.Message
Issue-Key: iotwireless|CreateMulticastGroup|4xx->4xx|$.message
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# iotwireless validation refusals carry no Message and wrong enum value set wording

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDestination

## Description
Five net-new causes, all one family: when IoT Wireless refuses a constraint-violating create, the emulator does not reproduce AWS's ValidationException body. Four refusals carry NO message member at all, and the fifth carries the sentence with an enum value set in the wrong order. AWS's refusal sentence is a fixed template — "1 validation error detected: Value[ '<rejected value>'] at '<member path>' failed to satisfy constraint: <constraint clause>" — whose only variable parts are the rejected value and the member path taken from the request; the constraint clause (the accepted enum value set and its order, the numeric limit, the regular expression) is fixed text decided by the service, not by the request.

Cause 1 of 5 — CreateDestination, an ExpressionType outside the accepted set (fixture iot-wireless-destination-lifecycle, step task#7).
AWS: 400 ValidationException (Smithy: CreateDestination errors include ValidationException @httpError 400; ValidationException$Message, length 0..2048, no jsonName, so the wire key is `Message`). Message = "1 validation error detected: Value at 'expressionType' failed to satisfy constraint: Member must satisfy enum value set: [RuleName, MqttTopic, SnsTopic]". Fixed text: everything except the member path 'expressionType', which comes from the request. The accepted set is recorded behavior only and is wider than the on-disk model's ExpressionType enum ([RuleName, MqttTopic]) — the recorded set, including SnsTopic, is the requirement.
Emulator: status 400 matches, but the body has no message member whatever ($.Message: missing, got None). Replay also shows the crate's own handler-side wording for this member is non-AWS prose ("ExpressionType must be one of [...], got ...").
Why: a client that reads the refusal to learn which member it got wrong is handed an empty body; SDK error.message() is empty and nothing identifies expressionType or the legal values, so a retry cannot be corrected.
Fix: the validation/refusal composition for this operation — the input validation that refuses the enum member, and the ValidationException body it serializes (the message member must be emitted, under the modeled `Message` key).
Traces: iot-wireless-destination-lifecycle#task#7.

Cause 2 of 5 — CreateDeviceProfile, LoRaWAN.MaxEirp above its ceiling (fixture iot-wireless-device-profile-lifecycle, step task#4).
AWS: 400 ValidationException (Smithy: CreateDeviceProfile errors include ValidationException @httpError 400). Message = "1 validation error detected: Value at 'loRaWAN.maxEirp' failed to satisfy constraint: Member must have value less than or equal to 15". Fixed text: the whole sentence apart from the member path 'loRaWAN.maxEirp'; the ceiling 15 is the service's rule, not a recorded value.
Emulator: 400 with no message member at all ($.Message: missing, got None).
Why: a client cannot tell a rejected EIRP from any other refusal of the same status; the limit it must respect is never stated.
Fix: CreateDeviceProfile's input validation plus the ValidationException body it serializes.
Traces: iot-wireless-device-profile-lifecycle#task#4.

Cause 3 of 5 — CreateNetworkAnalyzerConfiguration, TraceContent.LogLevel outside its enum (fixture iot-wireless-network-analyzer-configuration-lifecycle, step task#6).
AWS: 400 ValidationException (Smithy: CreateNetworkAnalyzerConfiguration errors include ValidationException @httpError 400). Message = "1 validation error detected: Value at 'traceContent.logLevel' failed to satisfy constraint: Member must satisfy enum value set: [ERROR, INFO, DISABLED]". Fixed text: all of it except the member path; note the order AWS prints is ERROR, INFO, DISABLED, which is NOT the model's declaration order (Smithy: LogLevel values [INFO, ERROR, DISABLED]) — the printed order is recorded behavior and is part of the message.
Emulator: 400 with no message member at all ($.Message: missing, got None).
Why: the caller is not told which member was refused or which log levels exist.
Fix: CreateNetworkAnalyzerConfiguration's input validation plus the ValidationException body it serializes.
Traces: iot-wireless-network-analyzer-configuration-lifecycle#task#6.

Cause 4 of 5 — CreateWirelessGateway, LoRaWAN.GatewayEui failing its pattern (fixture iot-wireless-wireless-gateway-lifecycle, step task#6).
AWS: 400 ValidationException (Smithy: CreateWirelessGateway errors include ValidationException @httpError 400; LoRaWAN is a required member). Message = "1 validation error detected: Value at 'loRaWAN.gatewayEui' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(([0-9A-Fa-f]{2}-){7}|([0-9A-Fa-f]{2}:){7}|([0-9A-Fa-f]{2}\\s){7}|([0-9A-Fa-f]{2}){7})([0-9A-Fa-f]{2})$". Fixed text: the sentence and the pattern verbatim; the only request-derived part is the member path.
Emulator: 400 with no message member at all ($.Message: missing, got None).
Why: the EUI spellings the service accepts are only discoverable from this message; without it a client cannot fix a malformed gatewayEui.
Fix: CreateWirelessGateway's input validation plus the ValidationException body it serializes.
Traces: iot-wireless-wireless-gateway-lifecycle#task#6.

Cause 5 of 5 — CreateMulticastGroup, LoRaWAN.RfRegion outside its enum: the sentence is present but the value set is printed in the model's declaration order instead of AWS's (fixture iot-wireless-multicast-group-lifecycle, step task#6).
AWS: 400 ValidationException. Message = "1 validation error detected: Value 'NOT_A_REGION' at 'loRaWAN.rfRegion' failed to satisfy constraint: Member must satisfy enum value set: [CN470, EU433, AU915, EU868, AS923-1, AS923-2, CN779, AS923-3, AS923-4, KR920, IN865, RU864, US915]". Variable parts: the rejected value (quoted from the request, 'NOT_A_REGION' in this recording) and the member path. Fixed part: the bracketed value set AND its order, which is recorded behavior only — it is not the model's order (Smithy: SupportedRfRegion values [EU868, US915, AU915, AS923-1, AS923-2, AS923-3, AS923-4, EU433, CN470, CN779, RU864, KR920, IN865]).
Emulator: 400 with the right template and the right rejected value, but the set printed in the model's declaration order: "... enum value set: [EU868, US915, AU915, AS923-1, AS923-2, AS923-3, AS923-4, EU433, CN470, CN779, RU864, KR920, IN865]". The message is also written under the lowercase key `message` (the crate's synthetic framework ValidationException schema, which names its member `message`), whereas the modeled iotwireless ValidationException member is `Message`.
Why: a client or test that compares the refusal text against AWS's sees a mismatch for a legal request/response pair, and the key casing differs from the modeled shape, so a strict decoder of the modeled error finds no message.
Fix: the shared validation-refusal composition for this crate — the enum-value-set clause must print the service's own value order, and the modeled ValidationException's `Message` member must be the one serialized.
Traces: iot-wireless-multicast-group-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:

1) iot-wireless-destination-lifecycle#task#7 CreateDestination — expected 400, actual 400; $.Message: missing (expected "1 validation error detected: Value at 'expressionType' failed to satisfy constraint: Member must satisfy enum value set: [RuleName, MqttTopic, SnsTopic]", got None).
2) iot-wireless-device-profile-lifecycle#task#4 CreateDeviceProfile — expected 400, actual 400; $.Message: missing (expected "1 validation error detected: Value at 'loRaWAN.maxEirp' failed to satisfy constraint: Member must have value less than or equal to 15", got None).
3) iot-wireless-network-analyzer-configuration-lifecycle#task#6 CreateNetworkAnalyzerConfiguration — expected 400, actual 400; $.Message: missing (expected "1 validation error detected: Value at 'traceContent.logLevel' failed to satisfy constraint: Member must satisfy enum value set: [ERROR, INFO, DISABLED]", got None).
4) iot-wireless-wireless-gateway-lifecycle#task#6 CreateWirelessGateway — expected 400, actual 400; $.Message: missing (expected "1 validation error detected: Value at 'loRaWAN.gatewayEui' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(([0-9A-Fa-f]{2}-){7}|([0-9A-Fa-f]{2}:){7}|([0-9A-Fa-f]{2}\\s){7}|([0-9A-Fa-f]{2}){7})([0-9A-Fa-f]{2})$", got None).
5) iot-wireless-multicast-group-lifecycle#task#6 CreateMulticastGroup — expected 400, actual 400; $.message: value_mismatch — expected "... enum value set: [CN470, EU433, AU915, EU868, AS923-1, AS923-2, CN779, AS923-3, AS923-4, KR920, IN865, RU864, US915]", got "... enum value set: [EU868, US915, AU915, AS923-1, AS923-2, AS923-3, AS923-4, EU433, CN470, CN779, RU864, KR920, IN865]" (the model's declaration order).

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): ValidationException { Message: Message (length 0..2048) } @httpError 400, no jsonName — the wire key is `Message`; ExpressionType enum [RuleName, MqttTopic]; LogLevel enum [INFO, ERROR, DISABLED]; SupportedRfRegion enum [EU868, US915, AU915, AS923-1, AS923-2, AS923-3, AS923-4, EU433, CN470, CN779, RU864, KR920, IN865].
Crate evidence: services/service-com-amazonaws-iotwireless-iotwireless/src/scaffolded.rs:117-144 defines the synthetic ValidationException whose only member is named `message`; services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_destination.rs:172-182 refuses an ExpressionType with the non-AWS sentence "ExpressionType must be one of {..}, got {..}".

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture iot-wireless-destination-lifecycle step task#7 (CreateDestination with an ExpressionType outside [RuleName, MqttTopic, SnsTopic]) and read the 400 body — no message member is present.
3. Cause 2: replay iot-wireless-device-profile-lifecycle step task#4 (CreateDeviceProfile with LoRaWAN.MaxEirp above 15).
4. Cause 3: replay iot-wireless-network-analyzer-configuration-lifecycle step task#6 (CreateNetworkAnalyzerConfiguration with an unmodeled TraceContent.LogLevel).
5. Cause 4: replay iot-wireless-wireless-gateway-lifecycle step task#6 (CreateWirelessGateway with a LoRaWAN.GatewayEui that fails the EUI pattern).
6. Cause 5: replay iot-wireless-multicast-group-lifecycle step task#6 (CreateMulticastGroup with LoRaWAN.RfRegion = a value outside the set) and compare the bracketed value set and its order, and the body key casing, against the recording.
