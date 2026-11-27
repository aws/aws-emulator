Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__events__sqs.putevents-payload-format-version-2-rejected
Issue-Key: apigatewayv2__events__sqs.putevents-payload-format-version-2-rejected|run#13|AssertionError|assert got == ("BadRequestException", _EXPECTED_MESSAGE), (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# apigatewayv2__events__sqs: an EventBridge-PutEvents integration with PayloadFormatVersion 2.0 is refused with the emulator's own sentence instead of AWS's "Operation: EventBridge-PutEvents is not supported."

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Cause 1 of 1: the refusal MESSAGE for an AWS_PROXY EventBridge-PutEvents integration declared with PayloadFormatVersion 2.0 is emulator-authored.

AWS: API Gateway v2 refuses this integration with HTTP 400, error code BadRequestException and the fixed message `Operation: EventBridge-PutEvents is not supported.` — the refusal is about the *subtype/operation* not supporting a 2.0 payload at all, not about the version value. The fixed text is the whole sentence; the only variable part is the integration subtype operation name (`EventBridge-PutEvents`) taken from the request's IntegrationSubtype. Recorded behavior for the message; the code and status come from the recorded response.
Emulator: answers BadRequestException with `Unsupported PayloadFormatVersion: 2.0` — right code, wrong message (it echoes the version instead of naming the unsupported operation).
Why: clients and IaC tools that match on the refusal text (and users reading it) are told the version is at fault rather than that the subtype does not support 2.0, so the documented fix (drop to 1.0 for this subtype) is not discoverable; message-asserting tests break.
Fix: the validation in the apigatewayv2 CreateIntegration handler that rejects PayloadFormatVersion for AWS_PROXY integration subtypes.
Traces: apigatewayv2__events__sqs.putevents-payload-format-version-2-rejected.

## Evidence
Expected: ('BadRequestException', 'Operation: EventBridge-PutEvents is not supported.')
Actual:   ('BadRequestException', 'Unsupported PayloadFormatVersion: 2.0')

## Steps to Reproduce
1. CreateApi (HTTP API).
2. CreateIntegration with IntegrationType=AWS_PROXY, IntegrationSubtype=EventBridge-PutEvents, PayloadFormatVersion="2.0", a credentials role and the PutEvents request parameters.
3. Observe the BadRequestException message.
