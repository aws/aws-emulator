Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-send-otp-parameter-bounds
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-send-otp-parameter-bounds pinpoint-send-otp-parameter-bounds-rejected pinpoint-otp-identity-and-reference-validation
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint|SendOTPMessage|4xx->2xx|$.Message
Issue-Key: pinpoint|SendOTPMessage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|SendOTPMessage|4xx->2xx|status
Issue-Key: pinpoint|SendOTPMessage|4xx->4xx|$.Message
Issue-Key: pinpoint|VerifyOTPMessage|4xx->2xx|$.Message
Issue-Key: pinpoint|VerifyOTPMessage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|VerifyOTPMessage|4xx->2xx|status
Issue-Key: pinpoint|VerifyOTPMessage|4xx->4xx|$.Message
Issue-Key: pinpoint|VerifyOTPMessage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|SendOTPMessage|2xx->4xx|$.ApplicationId
Issue-Key: pinpoint|SendOTPMessage|2xx->4xx|$.RequestId
Issue-Key: pinpoint|SendOTPMessage|2xx->4xx|$.Result
Issue-Key: pinpoint|SendOTPMessage|2xx->4xx|status
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# pinpoint OTP request parameters are not bounds-checked and OTP refusal bodies are emulator-authored

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendOTPMessage

## Description
The OTP pair (SendOTPMessage, VerifyOTPMessage) does not apply Pinpoint's request-parameter admission rules: out-of-range and over-length parameters are accepted and answered 200, required-member omissions are refused with a generic constraint-framework sentence instead of Pinpoint's BadRequestException wording, and an undeliverable origination identity is refused outright instead of being reported per destination in a 200 MessageResponse. This bundle carries 5 distinct causes; all 5 must be closed.

Cause 1 of 5: SendOTPMessage accepts a CodeLength outside the permitted range.
AWS: Pinpoint refuses a SendOTPMessageRequestParameters$CodeLength outside 5..8 with HTTP 400 BadRequestException whose Message is the fixed text 'Invalid CodeLength value for OTP. The CodeLength must be between 5 and 8.' (no variable part; the bound 5 and 8 are the service's own limits, not request values). Smithy: SendOTPMessage -> error BadRequestException httpError 400, body members Message/RequestID; SendOTPMessageRequestParameters$CodeLength targets __integer and the model states NO @range, so the bound is recorded behavior only.
Emulator: answers HTTP 200 with a MessageResponse and emits no Message and no x-amzn-errortype header at all — the bound is never evaluated. Replay: 'status: expected 400, got 200 / $.Message: missing'. The same cluster covers every out-of-range CodeLength probe across two fixtures (18 steps).
Why: a client that relies on Pinpoint rejecting an impossible code length sees a success, believes an OTP of that length was issued, and then can never verify it — the bad request is silently accepted instead of surfacing as a 400 the caller can correct.
Fix: SendOTPMessage's input validation (the handler's parameter-admission step, before any send result is built), which must emit BadRequestException/400 for CodeLength outside 5..8.
Traces: pinpoint-send-otp-parameter-bounds#task#4 (representative; cluster also covers task#5..#8, #10, #11, #13, #14 and pinpoint-send-otp-parameter-bounds-rejected#task#3..#19 odd steps).

Cause 2 of 5: the AllowedAttempts refusal Message is emulator prose.
AWS: an AllowedAttempts outside 1..5 is refused 400 BadRequestException with the fixed text 'Invalid AllowedAttempts value for OTP. The AllowedAttempts must be between 1 and 5.' — entirely fixed text; the bounds are the service's, and the offending request value is NOT echoed. Smithy: SendOTPMessageRequestParameters$AllowedAttempts targets __integer with no @range, so the bound and wording are recorded behavior only.
Emulator: returns the right status (400) but writes its own sentence 'AllowedAttempts must be at least 1; this request asks for 0' — different fixed text, and it interpolates the request's value, which AWS's message never does.
Why: a client or test that matches on the documented refusal text does not recognise the emulator's sentence, so an error-path assertion that passes against AWS fails here (and the emulator's value-echoing shape invites tests to depend on text AWS does not produce).
Fix: SendOTPMessage's input validation — the message template for the AllowedAttempts refusal.
Traces: pinpoint-send-otp-parameter-bounds#task#9 (cluster also covers #task#12 and pinpoint-send-otp-parameter-bounds-rejected#task#18).

Cause 3 of 5: VerifyOTPMessage accepts an out-of-range ReferenceId length.
AWS: a VerifyOTPMessageRequestParameters$ReferenceId whose length is outside 1..48 is refused 400 BadRequestException with the fixed text 'ReferenceId length must be between 1 and 48 characters.' Smithy: VerifyOTPMessageRequestParameters$ReferenceId targets __string with no @length trait, so the limit is recorded behavior only; the modeled error is BadRequestException/400.
Emulator: answers 200 with no Message and no x-amzn-errortype — the length is never checked. Replay: 'status: expected 400, got 200'.
Why: a caller that sent an unusable reference identifier is told the verification request was accepted, so an invalid correlation key is treated as a live one instead of being rejected at the boundary.
Fix: VerifyOTPMessage's input validation.
Traces: pinpoint-otp-identity-and-reference-validation#task#3 (cluster also covers #task#4).

Cause 4 of 5: a missing required VerifyOTPMessage member is refused by the generic constraint framework, not as Pinpoint's BadRequestException.
AWS: omitting the required DestinationIdentity yields 400 BadRequestException whose Message is the fixed text 'DestinationIdentity is a required value.' — the only variable part is the member name. Smithy: VerifyOTPMessageRequestParameters$DestinationIdentity is required (clientOptional), and VerifyOTPMessage models BadRequestException/400; the service does NOT model or return ValidationException.
Emulator: returns 400 but with the Smithy-constraint-framework body: Message "1 validation error detected: Value null at 'verifyOTPMessageRequestParameters.destinationIdentity' failed to satisfy constraint: Member must not be null" and header x-amzn-errortype: ValidationException — an error code this operation does not model.
Why: a client catching Pinpoint's BadRequestException does not match an error typed ValidationException, so the SDK surfaces an unmodeled/unknown error and the caller's error handling for required-member omissions never runs.
Fix: VerifyOTPMessage's input validation and its error serializer — required-member failures must be rendered as BadRequestException with Pinpoint's '<MemberName> is a required value.' wording.
Traces: pinpoint-otp-identity-and-reference-validation#task#6.

Cause 5 of 5: SendOTPMessage refuses an unusable OriginationIdentity instead of reporting it per destination in a 200 response.
AWS: when the request parameters are well formed, SendOTPMessage returns 200 with a MessageResponse carrying ApplicationId (the path's application id), RequestId, and Result keyed by the destination phone number; an origination identity the account does not own is reported inside that entry as DeliveryStatus PERMANENT_FAILURE, StatusCode 400 and the fixed StatusMessage 'The provided number does not exist or does not belong to the account, or its resource policy does not allow Pinpoint to use it' (fixed text; the keying address comes from the request). Smithy: SendOTPMessage -> MessageResponse with required ApplicationId plus RequestId and Result (MapOfMessageResult of MessageResult{DeliveryStatus, StatusCode, StatusMessage, MessageId}); the per-destination failure wording is recorded behavior only.
Emulator: answers 400 with no MessageResponse at all — ApplicationId, RequestId and Result are all absent. Replay: 'status: expected 200, got 400'.
Why: Pinpoint's send operations report delivery problems per destination rather than failing the call, so a client that reads Result to learn which addresses failed gets a top-level error instead, loses the per-address verdict, and may retry a request AWS considered served.
Fix: SendOTPMessage's handler/state — origination-identity resolution must produce a MessageResult entry, not a top-level refusal.
Traces: pinpoint-otp-identity-and-reference-validation#task#8.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (pinpoint, 13 clusters total):

Cause 1 — pinpoint-send-otp-parameter-bounds#task#4 SendOTPMessage: expected 400, got 200; $.Message missing (expected 'Invalid CodeLength value for OTP. The CodeLength must be between 5 and 8.', got None); $headers.x-amzn-errortype missing. 18 affected steps.

Cause 2 — pinpoint-send-otp-parameter-bounds#task#9 SendOTPMessage: status 400 both sides; $.Message value_mismatch — expected 'Invalid AllowedAttempts value for OTP. The AllowedAttempts must be between 1 and 5.', got 'AllowedAttempts must be at least 1; this request asks for 0'. 3 affected steps.

Cause 3 — pinpoint-otp-identity-and-reference-validation#task#3 VerifyOTPMessage: expected 400, got 200; $.Message missing (expected 'ReferenceId length must be between 1 and 48 characters.'); $headers.x-amzn-errortype missing. 2 affected steps.

Cause 4 — pinpoint-otp-identity-and-reference-validation#task#6 VerifyOTPMessage: status 400 both sides; $.Message value_mismatch — expected 'DestinationIdentity is a required value.', got "1 validation error detected: Value null at 'verifyOTPMessageRequestParameters.destinationIdentity' failed to satisfy constraint: Member must not be null"; $headers.x-amzn-errortype shape_mismatch — got 'ValidationException'.

Cause 5 — pinpoint-otp-identity-and-reference-validation#task#8 SendOTPMessage: expected 200, got 400; $.ApplicationId, $.RequestId and $.Result all missing. Expected Result entry for the destination number: DeliveryStatus PERMANENT_FAILURE, StatusCode 400, StatusMessage 'The provided number does not exist or does not belong to the account, or its resource policy does not allow Pinpoint to use it'.

Model evidence: services/service-com-amazonaws-pinpoint-pinpoint/smithy-model/com-amazonaws-pinpoint-pinpoint.json — SendOTPMessage POST /v1/apps/{ApplicationId}/otp, VerifyOTPMessage POST /v1/apps/{ApplicationId}/verify-otp; both model BadRequestException(400)/NotFoundException(404) and neither models ValidationException. BadRequestException members: Message, RequestID.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Cause 1: fixture pinpoint-send-otp-parameter-bounds, step task#4 — SendOTPMessage POST /v1/apps/{ApplicationId}/otp with a CodeLength outside 5..8; emulator answers 200 instead of 400 BadRequestException.
4. Cause 2: same fixture, step task#9 — SendOTPMessage with AllowedAttempts 0; 400 is returned but with emulator-authored Message text.
5. Cause 3: fixture pinpoint-otp-identity-and-reference-validation, step task#3 — VerifyOTPMessage with a ReferenceId longer than 48 characters; emulator answers 200.
6. Cause 4: same fixture, step task#6 — VerifyOTPMessage omitting DestinationIdentity; emulator answers ValidationException instead of BadRequestException.
7. Cause 5: same fixture, step task#8 — SendOTPMessage with an OriginationIdentity the account does not own; emulator answers 400 instead of 200 with a per-destination PERMANENT_FAILURE Result entry.
