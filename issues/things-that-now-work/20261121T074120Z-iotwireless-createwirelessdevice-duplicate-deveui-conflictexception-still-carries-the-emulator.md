Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.duplicate-dev-eui-rejected-within-account
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.duplicate-dev-eui-rejected-within-account|run#22|AssertionError|assert message == "Device with given DevEui already exists", f"unexpected message {message!r}"
Issue-Commit: 992eebbb1a1c5db8bb6bfb43357b4e2de2d0ccf2

# CreateWirelessDevice duplicate-DevEui ConflictException still carries the emulator's claimed-key sentence instead of AWS's within-account text

> _Note: this issue was identified against the code base at commit `992eebbb1a1c5db8bb6bfb43357b4e2de2d0ccf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessDevice

## Description
REOPENS 20261111T155554Z-iotwireless-createwirelessdevice-duplicate-deveui-conflictexception-carries-non-aws-message.md: its fix left the call refused. That issue is marked fixed, but the same behavioral check still fails on the same assertion against the same refusal: the duplicate-DevEui ConflictException body is unchanged.

One net-new cause.

Cause 1 of 1: a second CreateWirelessDevice that reuses a DevEui already claimed by another wireless device IN THE SAME ACCOUNT is refused with the correct status and error code, but the Message member is the emulator's claimed-key sentence, which interpolates the request's DevEui (and a doubled space), rather than the fixed sentence AWS answers for the within-account duplicate. Failing check: iotwireless.duplicate-dev-eui-rejected-within-account.

AWS: Smithy (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json) models CreateWirelessDevice as POST /wireless-devices, success code 201, with errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); ConflictException carries Message (length 0..2048, no jsonName, so the wire key is `Message`) plus optional ResourceId/ResourceType. The model does not state the text, so for the message this is recorded behavior only: the recorded within-account duplicate refusal is HTTP 409, Error.Code "ConflictException", Error.Message "Device with given DevEui already exists" — entirely FIXED text with NO variable parts: neither the rejected DevEui nor any identity of the device holding it is echoed.

Emulator: status 409 and code ConflictException match (the check's `code == "ConflictException"` assertion passes, and the following state assertions pass too — the claimed DevEui still resolves to the first device and a sibling DevEui registers as a distinct device), but Error.Message is "DevEui <LoRaWAN.DevEui from the request>  with same key has been claimed" (two spaces before "with"). Replay output: AssertionError: unexpected message 'DevEui 1451ad8d1734e7db  with same key has been claimed' — that hex value is the per-run DevEui the check generated and is not part of any requirement.

Why: the error message is a requirement. A client that reads the refusal text to tell this duplicate-DevEui conflict apart from the other ConflictException the same operation raises (a replayed @idempotencyToken ClientRequestToken used with different parameters), or whose log/alarm/test patterns key on AWS's sentence, matches nothing the emulator sends for an otherwise correct request/response pair. The interpolated DevEui additionally leaks a request value into a message AWS keeps value-free.

Fix: the duplicate-DevEui refusal body composed in this crate's wireless-device create path — the helper that builds the ConflictException message, which the CreateWirelessDevice handler calls when the account's DevEui index already holds the submitted DevEui. Status, code, conflict detection and state are already correct; only the Message member's construction diverges. Note the open issue 20261120T152925Z-iotwireless-createwirelessdevice-duplicate-deveui-conflict-returns-generic-message-instead.md: it is grounded in a WIRE fixture and asserts the OPPOSITE text (it expects "DevEui <devEui>  with same key has been claimed" and calls "Device with given DevEui already exists" the emulator's invention), so it does NOT cover this check — the two recordings disagree, and the fix must reconcile them rather than flip the single unconditional string back and forth (Hypothesis: the two recordings are different conflict contexts — a plain within-account CreateWirelessDevice duplicate versus the fixture's path — and the message has to be chosen per context).

Traces: behavioral check iotwireless.duplicate-dev-eui-rejected-within-account.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: iotwireless 7/8 (88%), 1 failed.

[mismatch] iotwireless.duplicate-dev-eui-rejected-within-account
  check_iotwireless_duplicate_dev_eui_rejected_within_account.py, line 105, in run
    assert message == "Device with given DevEui already exists", f"unexpected message {message!r}"
  AssertionError: unexpected message 'DevEui 1451ad8d1734e7db  with same key has been claimed'

Expected (AWS, recorded behavior): HTTP 409, Error.Code = "ConflictException", Error.Message = "Device with given DevEui already exists" (all fixed text; no request- or state-derived parts).
Actual (emulator): HTTP 409, Error.Code = "ConflictException" (matches), Error.Message = "DevEui <DevEui from request>  with same key has been claimed" (fixed text "DevEui" / "with same key has been claimed" with the request's LoRaWAN.DevEui interpolated and a doubled space).

Crate evidence for the unchanged text: services/service-com-amazonaws-iotwireless-iotwireless/src/state.rs:320-322 — `duplicate_dev_eui_message` formats "DevEui {dev_eui}  with same key has been claimed"; state.rs:331-337 — `duplicate_dev_eui` builds the ConflictException from it with ResourceId/ResourceType unset.
Model evidence: ConflictException @httpError 409 with members Message/ResourceId/ResourceType; CreateWirelessDevice http { POST /wireless-devices, code 201 }.

## Steps to Reproduce
1. Boot the emulator built from this checkout and run the behavioral suite simparity-behavioral-aws-v1.
2. Create a device profile (LoRaWAN MacVersion 1.0.3), a service profile, an IAM role for iotwireless.amazonaws.com, and a destination (ExpressionType RuleName).
3. Call CreateWirelessDevice with Type=LoRaWAN, that DestinationName, and LoRaWAN { DeviceProfileId, ServiceProfileId, DevEui=<fresh 8-byte hex EUI>, OtaaV1_0_x { AppKey, AppEui } } — it succeeds (201).
4. Call CreateWirelessDevice again with a different Name but the SAME DevEui and fresh OTAA key material.
5. Read the 409 body: Error.Code is ConflictException (correct) but Error.Message is "DevEui <devEui>  with same key has been claimed" instead of AWS's fixed "Device with given DevEui already exists".
6. Equivalent: replay behavioral check iotwireless.duplicate-dev-eui-rejected-within-account.
