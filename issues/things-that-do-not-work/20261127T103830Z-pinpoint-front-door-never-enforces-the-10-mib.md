Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pinpoint.otp-send-oversized-request-rejected pinpoint.send-messages-rejects-body-over-payload-limit
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint.otp-send-oversized-request-rejected|_refusal#11|AssertionError|raise AssertionError(f"request over the size limit was accepted: {result!r}")
Issue-Key: pinpoint.send-messages-rejects-body-over-payload-limit|_refusal#11|AssertionError|raise AssertionError(f"an oversized SendMessages body was accepted: {got.get('MessageResponse')!r}")
Issue-Commit: 7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21

# pinpoint front door never enforces the 10 MiB request content-length cap: oversized SendMessages/SendOTPMessage bodies are accepted with 200 instead of HTTP 413

> _Note: this issue was identified against the code base at commit `7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessages

## Description
The pinpoint emulator has no request-size gate in front of its operation handlers: a request body larger than the service's 10 MiB (10485760-byte) HTTP content-length cap is deserialized and served normally instead of being refused before the operation runs. This bundle carries 1 net-new root cause; it must be closed.

Cause 1 of 1: the HTTP content-length cap is not applied to any pinpoint request, so an over-cap body is answered 200 rather than 413.
AWS: pinpoint's request front door rejects a request whose HTTP body exceeds 10485760 bytes (10 MiB) before the operation executes. The refusal is HTTP 413 with a body carrying NO modelled error type and no x-amzn-errortype header, so the SDK reports the HTTP status itself as the error code (botocore Error.Code == "413"), and Error.Message is the fixed text "HTTP content length exceeded <limit> bytes." — fixed frame "HTTP content length exceeded " + " bytes." with the service's own byte limit as its only variable part (10485760 for pinpoint); nothing from the request is echoed. A body exactly at the cap is accepted and served normally, and the application keeps serving sends after a refusal. Smithy: both SendMessages (POST /v1/apps/{ApplicationId}/messages) and SendOTPMessage (POST /v1/apps/{ApplicationId}/otp) model PayloadTooLargeException with httpError 413 (members Message, RequestID), confirming 413 is part of these operations' contract; the byte value of the cap and the unmodelled/status-as-code shape of the front-door refusal are recorded behavior only.
Emulator: there is no size check at all. A SendOTPMessage body of 10485761 bytes (one over the cap) is accepted and answered HTTP 200 with a full MessageResponse (ApplicationId/RequestId/Result with a per-destination PERMANENT_FAILURE entry); a SendMessages body padded to ~11,000,000 bytes via MessageRequest$Context is likewise answered HTTP 200 with a MessageResponse whose Result carries the per-address APNS-channel-not-found entry. The replay assertions are "request over the size limit was accepted: {...'HTTPStatusCode': 200...}" and "an oversized SendMessages body was accepted: {...}". The same bodies at/below the cap are served identically, so the emulator draws no boundary anywhere.
Why: a client that relies on pinpoint refusing an over-quota payload gets a success for a request AWS never accepts. It believes an OTP was issued or a push was handed off when against AWS the call failed with 413, so its PayloadTooLargeException/413 handling (splitting the batch, trimming Context, surfacing the quota to the caller) never runs and it has no signal that it is over the documented quota. Negative tests for the payload limit cannot pass, and the emulator also buffers and deserializes unbounded request bodies that AWS drops at the front door.
Fix: the pinpoint service's request front door / router — the pre-dispatch stage that reads the HTTP body, ahead of any operation handler and ahead of input validation, must compare the content length against the 10485760-byte cap and emit the 413 refusal (status-as-code, Message "HTTP content length exceeded 10485760 bytes.") for every operation, not just the two exercised here. This is distinct from the already-open pinpoint issues 20261127T095051Z-pinpoint-sendmessages-request-gating-diverges-unvalidated-destination-maps and 20261127T095051Z-pinpoint-otp-request-parameters-are-not-bounds-checked, which cover per-member admission rules inside the handlers and do not mention the content-length cap.
Traces: pinpoint.otp-send-oversized-request-rejected, pinpoint.send-messages-rejects-body-over-payload-limit.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (pinpoint, 4/8 passed):

[mismatch] pinpoint.otp-send-oversized-request-rejected — the check sends SendOTPMessage with a body padded through SendOTPMessageRequestParameters$EntityId to exactly 10485760 bytes (accepted on both sides), then one byte more (10485761). Expected refusal tuple (status, Code, Message) == (413, "413", "HTTP content length exceeded 10485760 bytes."). Got instead HTTP 200: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'MessageResponse': {'ApplicationId': <app id>, 'RequestId': <id>, 'Result': {'+1206555XXXX': {'DeliveryStatus': 'PERMANENT_FAILURE', 'MessageId': <id>, 'StatusCode': 400, 'StatusMessage': 'The provided number does not exist or does not belong to the account, or its resource policy does not allow Pinpoint to use it'}}}}. AssertionError: "request over the size limit was accepted".

[mismatch] pinpoint.send-messages-rejects-body-over-payload-limit — the check sends SendMessages with MessageRequest$Context padded by 6,000,000 bytes (accepted on both sides, per-address PERMANENT_FAILURE/404 for an app with no APNS channel) and then by 11,000,000 bytes. Expected (Code, status) == ("413", 413) with Message matching the regex r"HTTP content length exceeded \d+ bytes\.". Got instead HTTP 200 with MessageResponse {'ApplicationId': <app id>, 'RequestId': <id>, 'Result': {'sim2real-device-token-0001': {'DeliveryStatus': 'PERMANENT_FAILURE', 'StatusCode': 404, 'StatusMessage': 'APNS channel not found. Please enable the APNS channel for this application <app id>.'}}}. AssertionError: "an oversized SendMessages body was accepted".

(App ids, request ids, message ids and phone numbers above are per-run values and are not part of the requirement; the requirement is the 413 status, the "413" code and the fixed "HTTP content length exceeded <limit> bytes." wording.)

Model evidence: services/service-com-amazonaws-pinpoint-pinpoint/smithy-model/com-amazonaws-pinpoint-pinpoint.json — SendMessages POST /v1/apps/{ApplicationId}/messages and SendOTPMessage POST /v1/apps/{ApplicationId}/otp both list PayloadTooLargeException (errorKind client, httpError 413, members Message/RequestID) among their errors; no @length or size constraint exists on MessageRequest or SendOTPMessageRequestParameters, so the cap is a front-door rule rather than a member constraint.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the behavioral suite (simparity-behavioral-aws-v1).
3. Cause 1, check pinpoint.otp-send-oversized-request-rejected: create an application with CreateApp, then call SendOTPMessage on it with SendOTPMessageRequestParameters padded (e.g. through EntityId) so the serialized HTTP body is exactly 10485760 bytes — this is accepted and answers 200 per destination. Repeat with the body at 10485761 bytes: the emulator answers 200 with a MessageResponse, where AWS answers HTTP 413, error code "413" and Message "HTTP content length exceeded 10485760 bytes.".
4. Cause 1, check pinpoint.send-messages-rejects-body-over-payload-limit: on the same kind of application, call SendMessages with MessageRequest {Addresses: {<token>: {ChannelType: APNS}}, Context: {filler: 'x' * 6000000}, MessageConfiguration: {APNSMessage: {...}}} — accepted on both sides with a per-address PERMANENT_FAILURE/404. Then repeat with 'x' * 11000000: the emulator answers 200 instead of the 413 refusal.
5. Confirm the gate is missing rather than mis-sized: no request size is refused at any padding above the cap, and a small send after the attempt still succeeds on both sides.
