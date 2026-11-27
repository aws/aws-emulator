Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-signaling-alexa-offer-unenforced-payload-pattern
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-signaling-alexa-offer-unenforced-payload-pattern kinesis-video-signaling-signaling-member-length-and-enum-limits
Issue-Shape-Id: com.amazonaws.kinesisvideosignaling#AWSAcuitySignalingService
Issue-Key: kinesis-video-signaling|SendAlexaOfferToMaster|2xx->4xx|status
Issue-Key: kinesis-video-signaling|SendAlexaOfferToMaster|2xx->2xx|$
Issue-Key: kinesis-video-signaling|GetIceServerConfig|4xx->4xx|$.message
Issue-Key: kinesis-video-signaling|GetIceServerConfig|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-signaling|SendAlexaOfferToMaster|4xx->2xx|$
Issue-Key: kinesis-video-signaling|SendAlexaOfferToMaster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-signaling|SendAlexaOfferToMaster|4xx->2xx|status
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kinesis-video-signaling request validation and SendAlexaOfferToMaster response diverge from recorded AWS behavior

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendAlexaOfferToMaster

## Description
FOUR distinct causes, all in the Kinesis Video Signaling (AWSAcuitySignalingService, restJson1) request-validation and response-serialization path. The shared fault is that the emulator derives its wire behavior from the Smithy member constraints mechanically, while the recorded AWS front end enforces a different subset of them and reports failures with its own generic error body; and that the SendAlexaOfferToMaster success body is serialized without its output member. All four MUST be closed before this issue is submitted.

=== Cause 1 of 4: SendAlexaOfferToMaster rejects a MessagePayload that AWS accepts (pattern enforced that AWS does not enforce) ===
AWS: Smithy: SendAlexaOfferToMaster.MessagePayload targets MessagePayload (@length min 1 max 10000, @pattern ^[a-zA-Z0-9+/=]+$), required. Recorded behavior shows AWS does NOT reject a payload that violates that character-class pattern: the recorded response for this step is HTTP 200 with the normal SendAlexaOfferToMasterResponse body. The pattern is a modeled client-side hint only; the service accepts the request and answers 200. Recorded behavior only for the non-enforcement.
Emulator: answers HTTP 400 for the same request instead of 200.
Why: a client that sends a payload AWS accepts (any base64-ish SDP offer whose bytes stray outside the modeled character class, e.g. padding/whitespace/newlines inside the SDP blob) gets a hard client error from the emulator and the Alexa offer exchange never happens, so the signaling handshake cannot be tested at all against the emulator even though it works against AWS.
Fix: the SendAlexaOfferToMaster input validation — drop the @pattern check on MessagePayload (keep only what the recording shows AWS enforcing, i.e. length).
Traces: kinesis-video-signaling-alexa-offer-unenforced-payload-pattern#task#4 (cluster also covers task#5).

=== Cause 2 of 4: SendAlexaOfferToMaster returns an empty body, omitting the Answer member ===
AWS: Smithy: SendAlexaOfferToMasterResponse has one member, Answer (target Answer, @length min 1 max 10000); http code 200. The recorded 200 response body carries the Answer key (the SDP answer relayed from the master peer). The requirement is the member's presence in the response body, not any particular SDP text — the value is request/state derived.
Emulator: returns HTTP 200 with body {} — the Answer member is absent entirely (replay verdict: "$: empty_body (expected {'Answer': ...}, got {})").
Why: an SDK client reads response.answer() to complete the WebRTC handshake. Against the emulator it is always unset/None, so the caller cannot distinguish "no answer yet" from "answer delivered" and every Alexa-to-master negotiation stalls; code that unwraps the answer panics or NPEs.
Fix: the SendAlexaOfferToMaster handler/serializer and the signaling-channel state it reads — it must record the offer against the channel and emit the Answer member in the response body.
Traces: kinesis-video-signaling-alexa-offer-unenforced-payload-pattern#task#6 (cluster also covers kinesis-video-signaling-signaling-member-length-and-enum-limits#task#7).

=== Cause 3 of 4: GetIceServerConfig over-length Username returns a Smithy-style validation message instead of AWS's "Invalid request body" ===
AWS: Smithy: GetIceServerConfigRequest.Username targets Username (@length min 1 max 256, @pattern ^[a-zA-Z0-9_.-]+$); modeled errors ClientLimitExceededException / InvalidArgumentException / InvalidClientException (400), NotAuthorizedException (401), ResourceNotFoundException (404), SessionExpiredException (400). For a Username longer than 256 characters the recording shows HTTP 400 with body {"message": "Invalid request body"} and an x-amzn-errortype header whose value differs in shape from InvalidArgumentException. "Invalid request body" is the WHOLE fixed text of this error class — it has no variable parts: AWS does not echo the offending value, the member name or the limit. Recorded behavior only for the exact error code behind that header.
Emulator: right status (400) but the wrong error identity and message: x-amzn-errortype: InvalidArgumentException, and body message "1 validation error detected: Value 'uuuu…' [257 bytes] at 'username' failed to satisfy constraint: Member must have length less than or equal to 256" — an EC2/Query-style synthesized validation string that this service never emits, and which also leaks the rejected value back to the caller.
Why: clients and tests match on the error code (x-amzn-errortype) and on the message text to classify a signaling failure. Against the emulator they see a different exception type and a message of a different family, so retry/classification logic that works on AWS takes the wrong branch, and assertions on the AWS message fail.
Fix: GetIceServerConfig's input validation and the service's error serializer — emit this service's own 400 shape with the fixed message "Invalid request body" and the recorded errortype, rather than a generic Smithy constraint-violation renderer.
Traces: kinesis-video-signaling-signaling-member-length-and-enum-limits#task#5 (cluster also covers task#6).

=== Cause 4 of 4: SendAlexaOfferToMaster accepts an out-of-limit request that AWS rejects with 400 ===
AWS: Smithy: SendAlexaOfferToMaster input members ChannelARN (ResourceARN, @length 1..1024, ARN @pattern), SenderClientId (ClientId, @length 1..256), MessagePayload (@length 1..10000) — all required. When a member exceeds its modeled length bound the recording shows HTTP 400 with body {"message": "Invalid request body"} plus an x-amzn-errortype header. Again "Invalid request body" is entirely fixed text with no request- or state-derived parts.
Emulator: answers HTTP 200 with body {} — no status error, no message, no x-amzn-errortype header at all (replay: "status: expected 400, got 200", "$headers.x-amzn-errortype: missing").
Why: the emulator silently accepts a request AWS refuses, so a client bug (an over-long client id or payload) looks successful in testing and only fails in production; negative tests that assert the 400 cannot pass.
Fix: the SendAlexaOfferToMaster handler's input validation — enforce the modeled @length bounds on ChannelARN, SenderClientId and MessagePayload and fail them through the same 400 "Invalid request body" path as Cause 3 (note the asymmetry with Cause 1: lengths ARE enforced by AWS, the character-class pattern is not).
Traces: kinesis-video-signaling-signaling-member-length-and-enum-limits#task#8.

Related but NOT covering any of the above: open issue 20261125T092321Z-kinesis-video-signaling-geticeserverconfig-announces-turn-relay-uris-on-an-authority.md owns the GetIceServerConfig TURN-URI authority divergences only.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 21 steps, 12 passed, 9 mismatched, score 57.1%; emulator up, /__admin/health 200).

Cause 1 — SendAlexaOfferToMaster, fixture kinesis-video-signaling-alexa-offer-unenforced-payload-pattern step task#4 (+task#5):
  expected status 200 (recorded AWS)
  actual   status 400
  detail: "status: expected 200, got 400"

Cause 2 — SendAlexaOfferToMaster, fixture kinesis-video-signaling-alexa-offer-unenforced-payload-pattern step task#6 (+kinesis-video-signaling-signaling-member-length-and-enum-limits#task#7):
  expected 200 with body containing member Answer
  actual   200 with body {}
  detail: "$: empty_body (expected {'Answer': None}, got {})"

Cause 3 — GetIceServerConfig, fixture kinesis-video-signaling-signaling-member-length-and-enum-limits step task#5 (+task#6):
  expected 400, body {"message": "Invalid request body"}, x-amzn-errortype of a different shape
  actual   400, body message "1 validation error detected: Value 'uuuu…' [257 bytes] at 'username' failed to satisfy constraint: Member must have length less than or equal to 256", x-amzn-errortype: InvalidArgumentException
  detail: "$.message: value_mismatch"; "$headers.x-amzn-errortype: shape_mismatch"

Cause 4 — SendAlexaOfferToMaster, fixture kinesis-video-signaling-signaling-member-length-and-enum-limits step task#8:
  expected 400, body {"message": "Invalid request body"}, x-amzn-errortype present
  actual   200, body {}, no x-amzn-errortype
  detail: "status: expected 400, got 200"; "$: empty_body"; "$headers.x-amzn-errortype: missing (got None)"

Model cited: services/service-com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice/smithy-model/com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice.json (sdkId "Kinesis Video Signaling", restJson1, sigv4; POST /v1/send-alexa-offer-to-master, POST /v1/get-ice-server-config).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator (data listener + /__admin/health).
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: replay fixture kinesis-video-signaling-alexa-offer-unenforced-payload-pattern step task#4 — POST /v1/send-alexa-offer-to-master with a valid ChannelARN and SenderClientId and a MessagePayload containing characters outside ^[a-zA-Z0-9+/=]+$ but within 1..10000 bytes. Expect 200; emulator answers 400.
4. Cause 2: replay the same fixture step task#6 — a well-formed SendAlexaOfferToMaster. Expect a 200 body carrying the Answer member; emulator answers 200 with {}.
5. Cause 3: replay fixture kinesis-video-signaling-signaling-member-length-and-enum-limits step task#5 — POST /v1/get-ice-server-config with Username of 257 characters. Expect 400 with body {"message":"Invalid request body"} and the recorded x-amzn-errortype; emulator answers 400 with a "1 validation error detected: ... at 'username' ..." message and x-amzn-errortype: InvalidArgumentException.
6. Cause 4: replay fixture kinesis-video-signaling-signaling-member-length-and-enum-limits step task#8 — POST /v1/send-alexa-offer-to-master with an input member past its modeled @length bound. Expect 400 with body {"message":"Invalid request body"} and an x-amzn-errortype header; emulator answers 200 with {} and no errortype header.
7. After fixing, re-replay both fixtures listed in Issue-Trace/Issue-Step and trace_ids — all four steps must match.
