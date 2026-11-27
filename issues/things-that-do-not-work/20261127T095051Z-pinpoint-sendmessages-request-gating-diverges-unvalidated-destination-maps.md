Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-send-messages-addresses-and-endpoints-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-send-messages-addresses-and-endpoints-rejected pinpoint-send-messages-per-channel-results pinpoint-send-messages-malformed-request-rejected pinpoint-send-messages-unrouted-method-access-denied
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint|SendMessages|4xx->2xx|$.Message
Issue-Key: pinpoint|SendMessages|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|SendMessages|4xx->2xx|status
Issue-Key: pinpoint|SendMessages|4xx->4xx|$.Message
Issue-Key: pinpoint|SendMessages|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|SendMessages|2xx->4xx|$.ApplicationId
Issue-Key: pinpoint|SendMessages|2xx->4xx|$.RequestId
Issue-Key: pinpoint|SendMessages|2xx->4xx|$.Result
Issue-Key: pinpoint|SendMessages|2xx->4xx|status
Issue-Key: pinpoint|SendMessages|4xx->5xx|$.message
Issue-Key: pinpoint|SendMessages|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: pinpoint|SendMessages|4xx->5xx|status
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# pinpoint SendMessages request gating diverges: unvalidated destination maps, emulator-authored refusals and an unrouted method

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessages

## Description
SendMessages mis-handles the whole admission path of a direct-send request: a mutually exclusive destination pair is accepted, the empty-destination and bad-enum refusals are emulator-authored (one of them typed as an error the operation does not model), a channel that is not enabled is refused at the top level instead of reported per destination, and a request on the wrong HTTP method is answered NotImplemented instead of the service's pre-dispatch authorization refusal. This bundle carries 5 distinct causes; all 5 must be closed.

Cause 1 of 5: a MessageRequest carrying both Addresses and Endpoints is accepted.
AWS: Pinpoint refuses a MessageRequest that supplies both the Addresses map and the Endpoints map with HTTP 400 BadRequestException whose Message is the fixed text 'Specify one map of addresses or endpoints. Not both.' (no variable part). Smithy: MessageRequest has both Addresses (MapOfAddressConfiguration) and Endpoints (MapOfEndpointSendConfiguration) as optional members with no modeled exclusivity, so the rule is recorded behavior only; the modeled refusal is BadRequestException httpError 400.
Emulator: answers HTTP 200 and emits neither Message nor x-amzn-errortype — the exclusivity rule is never evaluated. Replay: 'status: expected 400, got 200'.
Why: a caller that accidentally populates both maps is told the send succeeded, with no way to know which map (if either) was actually used for delivery; against AWS the same request is a hard 400 the caller must fix.
Fix: SendMessages' input validation (the MessageRequest admission step in the handler, before any send result is built).
Traces: pinpoint-send-messages-addresses-and-endpoints-rejected#task#2 (cluster also covers pinpoint-send-messages-per-channel-results#task#5).

Cause 2 of 5: the empty-destination refusal Message is emulator prose.
AWS: a MessageRequest with neither Addresses nor Endpoints is refused 400 BadRequestException with the fixed text 'No Addresses or Endpoints to send to' — entirely fixed, no request-derived part.
Emulator: returns 400 but with its own sentence 'MessageRequest must specify at least one of Addresses or Endpoints'.
Why: a client or test matching Pinpoint's refusal text does not recognise the emulator's wording, so an error-path assertion that passes against AWS fails here.
Fix: SendMessages' input validation — the message template for the no-destination refusal.
Traces: pinpoint-send-messages-malformed-request-rejected#task#3 (cluster also covers pinpoint-send-messages-per-channel-results#task#4).

Cause 3 of 5: an unknown ChannelType value is refused as a generic constraint violation typed ValidationException.
AWS: an AddressConfiguration$ChannelType that is not a ChannelType member is rejected during payload deserialization with 400 BadRequestException whose Message is the Jackson deserialization text: the fixed frame 'Cannot deserialize value of type `ChannelType` from String "<value>": not one of the values accepted for Enum class: [<member list>]' followed by a newline and 'Line <n>, column <m>'. Fixed text = the frame and 'not one of the values accepted for Enum class:'; variable parts = the offending string taken from the request, the enum member list, and the line/column of the offending token in the submitted body. Smithy: ChannelType enum values are PUSH, GCM, APNS, APNS_SANDBOX, APNS_VOIP, APNS_VOIP_SANDBOX, ADM, SMS, VOICE, EMAIL, BAIDU, CUSTOM, IN_APP; SendMessages models BadRequestException(400) and does NOT model ValidationException.
Emulator: answers 400 with the Smithy-constraint-framework body instead — Message "1 validation error detected: Value '<value>' at 'messageRequest.addresses.<address>.member.channelType' failed to satisfy constraint: Member must satisfy enum value set: [...]" and header x-amzn-errortype: ValidationException.
Why: the error is typed as a code SendMessages does not model, so an SDK client's catch of BadRequestException does not fire and the failure surfaces as an unknown error; the body also reports a member path rather than the deserialization position AWS reports.
Fix: SendMessages' request deserializer and its error serializer — an unknown enum token must be rendered as BadRequestException with the deserialization wording, not routed through the generic constraint framework.
Traces: pinpoint-send-messages-malformed-request-rejected#task#4 (cluster also covers #task#5).

Cause 4 of 5: a direct send to a channel that is not enabled is refused instead of reported per destination.
AWS: SendMessages returns 200 with a MessageResponse carrying ApplicationId (the path's application id), RequestId and Result keyed by the destination address; a channel that the application has not enabled is reported inside that entry as DeliveryStatus PERMANENT_FAILURE, StatusCode 404 and StatusMessage '<CHANNEL> channel not found. Please enable the <CHANNEL> channel for this application <application-id>.' — fixed frame with the channel name and the application id from the request as its variable parts. Smithy: MessageResponse{ApplicationId required, RequestId, Result: MapOfMessageResult of MessageResult{DeliveryStatus, StatusCode, StatusMessage, MessageId}}; the wording is recorded behavior only.
Emulator: answers 400 with no MessageResponse at all — ApplicationId, RequestId and Result are absent. Replay: 'status: expected 200, got 400'.
Why: Pinpoint's contract is that per-destination delivery problems come back inside a 200 Result map, so a client reading Result to learn which addresses failed gets a top-level error, loses the per-address verdict for every other destination in the same request, and may retry a call AWS treated as served.
Fix: SendMessages' handler/state — channel-enablement resolution must produce a MessageResult entry rather than a top-level refusal.
Traces: pinpoint-send-messages-per-channel-results#task#3.

Cause 5 of 5: an unrouted HTTP method on the messages resource is answered NotImplemented instead of the service's authorization refusal.
AWS: SendMessages is bound to POST /v1/apps/{ApplicationId}/messages. A request to that path with a different method (e.g. PUT) matches no operation, so the front end cannot resolve an operation to authorize and answers HTTP 403 with the fixed Message text 'Unable to determine service/operation name to be authorized' (lowercase 'message' member, no variable part). Smithy: SendMessages http {method: POST, uri: /v1/apps/{ApplicationId}/messages}; ForbiddenException is modeled with httpError 403. The 403 wording is recorded behavior only.
Emulator: answers HTTP 501 with header x-amzn-errortype: NotImplemented and Message 'operation PUT /v1/apps/<app-id>/messages is not implemented in this emulator' — an emulator-specific status, error code and sentence, and one that echoes the request path.
Why: a client exercising the service's method-level rejection sees a 5xx NotImplemented, which SDK retry logic and tests treat as an emulator/server defect rather than the 403 client error AWS returns; a negative test for the unrouted method cannot pass.
Fix: the pinpoint router's method dispatch for /v1/apps/{ApplicationId}/messages — an unmatched method on a bound path must fall through to the 403 unresolved-operation refusal, not the NotImplemented fallback.
Traces: pinpoint-send-messages-unrouted-method-access-denied#task#2 (cluster also covers #task#3).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (pinpoint):

Cause 1 — pinpoint-send-messages-addresses-and-endpoints-rejected#task#2 SendMessages: expected 400, got 200; $.Message missing (expected 'Specify one map of addresses or endpoints. Not both.'); $headers.x-amzn-errortype missing. 2 affected steps (also pinpoint-send-messages-per-channel-results#task#5).

Cause 2 — pinpoint-send-messages-malformed-request-rejected#task#3 SendMessages: status 400 both sides; $.Message value_mismatch — expected 'No Addresses or Endpoints to send to', got 'MessageRequest must specify at least one of Addresses or Endpoints'. 2 affected steps.

Cause 3 — pinpoint-send-messages-malformed-request-rejected#task#4 SendMessages: status 400 both sides; $.Message value_mismatch — expected 'Cannot deserialize value of type `ChannelType` from String "TELEGRAPH": not one of the values accepted for Enum class: [IN_APP, ADM, APNS_VOIP, GCM, VOICE, APNS_VOIP_SANDBOX, APNS, SMS, PUSH, BAIDU, EMAIL, CUSTOM, APNS_SANDBOX]\nLine 1, column 59', got "1 validation error detected: Value 'TELEGRAPH' at 'messageRequest.addresses.<address>.member.channelType' failed to satisfy constraint: Member must satisfy enum value set: [...]"; $headers.x-amzn-errortype shape_mismatch — got 'ValidationException'. 2 affected steps.

Cause 4 — pinpoint-send-messages-per-channel-results#task#3 SendMessages: expected 200, got 400; $.ApplicationId, $.RequestId, $.Result all missing. Expected Result entry: DeliveryStatus PERMANENT_FAILURE, StatusCode 404, StatusMessage of the form '<CHANNEL> channel not found. Please enable the <CHANNEL> channel for this application <application-id>.'

Cause 5 — pinpoint-send-messages-unrouted-method-access-denied#task#2 SendMessages: expected 403, got 501; $.message value_mismatch — expected 'Unable to determine service/operation name to be authorized', got 'operation PUT /v1/apps/<app-id>/messages is not implemented in this emulator'; $headers.x-amzn-errortype shape_mismatch — got 'NotImplemented'. 2 affected steps.

Model evidence: services/service-com-amazonaws-pinpoint-pinpoint/smithy-model/com-amazonaws-pinpoint-pinpoint.json — SendMessages http POST /v1/apps/{ApplicationId}/messages, errors BadRequestException(400)/ForbiddenException(403)/NotFoundException(404)/MethodNotAllowedException(405)/PayloadTooLargeException(413)/TooManyRequestsException(429)/InternalServerErrorException(500); ValidationException is not among them. MessageRequest has optional Addresses and Endpoints maps; ChannelType enum = [PUSH, GCM, APNS, APNS_SANDBOX, APNS_VOIP, APNS_VOIP_SANDBOX, ADM, SMS, VOICE, EMAIL, BAIDU, CUSTOM, IN_APP]; MessageResponse = {ApplicationId (required), EndpointResult, RequestId, Result}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Cause 1: fixture pinpoint-send-messages-addresses-and-endpoints-rejected, step task#2 — SendMessages POST /v1/apps/{ApplicationId}/messages with both Addresses and Endpoints populated; emulator answers 200 instead of 400.
4. Cause 2: fixture pinpoint-send-messages-malformed-request-rejected, step task#3 — SendMessages with neither Addresses nor Endpoints; 400 returned with emulator-authored Message.
5. Cause 3: same fixture, step task#4 — SendMessages with an AddressConfiguration ChannelType of 'TELEGRAPH'; emulator answers ValidationException with constraint-framework text.
6. Cause 4: fixture pinpoint-send-messages-per-channel-results, step task#3 — SendMessages to a GCM token on an application with no GCM channel enabled; emulator answers 400 instead of 200 with a per-destination PERMANENT_FAILURE/404 Result entry.
7. Cause 5: fixture pinpoint-send-messages-unrouted-method-access-denied, step task#2 — PUT /v1/apps/{ApplicationId}/messages; emulator answers 501 NotImplemented instead of 403.
