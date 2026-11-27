Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-wireless-device-lifecycle
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-wireless-device-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|CreateWirelessDevice|4xx->4xx|$.Message
Issue-Commit: cb7623640c413c0c6c9bd4815152720c2f57d7b6

# CreateWirelessDevice duplicate DevEui conflict returns generic message instead of AWS claimed-DevEui text

> _Note: this issue was identified against the code base at commit `cb7623640c413c0c6c9bd4815152720c2f57d7b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessDevice

## Description
Cause 1 of 1: CreateWirelessDevice rejects a second wireless device carrying an already-claimed LoRaWAN DevEui with the right HTTP status and error shape, but with the emulator's own invented message text rather than the message AWS returns for that error class.

AWS: Smithy (services/service-com-amazonaws-iotwireless-iotwireless) models CreateWirelessDevice POST /wireless-devices with errors including ConflictException (errorKind client, httpError 409), whose `Message` member (@length max 2048) carries the human-readable reason; the model does not specify the message text, so for the text this is recorded behavior only. The recording shows AWS answering 409 ConflictException with a message whose FIXED text is "DevEui" ... "with same key has been claimed" and whose only VARIABLE part is the DevEui value taken from the request's LoRaWAN.DevEui -- recorded as "DevEui bbb78a67f1193555  with same key has been claimed" (the hex string is the request's DevEui, not a constant, and the run shows a doubled space where the template interpolates the identifier).

Emulator: answers 409 as well, but with `$.Message` = "Device with given DevEui already exists" -- a generic sentence that names neither the DevEui from the request nor AWS's "has been claimed" phrasing. Replay verdict: mismatch, expected_status 400/409 matched, key `$.Message` value_mismatch.

Why: the error message is a requirement, not decoration. Clients and SDK error-handling code that key on AWS's wording (or that parse the offending DevEui back out of the message to report which device collided, retry with a different EUI, or decide between "already claimed by this account" and some other conflict) see text that matches nothing AWS ever sends, so duplicate-claim handling that works against AWS silently fails or reports the wrong device against the emulator.

Fix: the CreateWirelessDevice handler's duplicate-DevEui conflict path -- the place that constructs the ConflictException before serializing it -- must build the message from AWS's template, interpolating the DevEui supplied in the request's LoRaWAN.DevEui instead of emitting a fixed generic string. The 409 status and the ConflictException shape are already correct; only the Message member's construction is wrong.

Traces: iot-wireless-wireless-device-lifecycle#task#10 (CreateWirelessDevice, second create with an already-claimed DevEui).

## Evidence
Fixture iot-wireless-wireless-device-lifecycle, step task#10, operation CreateWirelessDevice.
Expected: HTTP 409, ConflictException, $.Message = "DevEui bbb78a67f1193555  with same key has been claimed" (fixed text "DevEui" / "with same key has been claimed"; the hex value is the request's LoRaWAN.DevEui).
Actual:   HTTP 409, $.Message = "Device with given DevEui already exists".
Replay detail: "$.Message: value_mismatch (expected 'DevEui bbb78a67f1193555  with same key has been claimed', got 'Device with given DevEui already exists')"; divergence_key service=iotwireless|CreateWirelessDevice|status=409->409|keys=$.Message.
Status and error shape agree; only the message text diverges.

## Steps to Reproduce
1. Replay fixture iot-wireless-wireless-device-lifecycle against the emulator.
2. The fixture creates a LoRaWAN wireless device (CreateWirelessDevice, POST /wireless-devices) with a given LoRaWAN.DevEui, then at step task#10 issues a second CreateWirelessDevice carrying the same DevEui.
3. Observe the 409 response body: $.Message is the emulator's generic "Device with given DevEui already exists" instead of AWS's "DevEui <DevEui from request> with same key has been claimed".
