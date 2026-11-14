Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-baidu-channel-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-baidu-channel-lifecycle pinpoint-send-messages-channel-not-enabled
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint|DeleteBaiduChannel|2xx->2xx|$.Enabled
Issue-Key: pinpoint|DeleteBaiduChannel|2xx->2xx|$.HasCredential
Issue-Key: pinpoint|DeleteBaiduChannel|2xx->2xx|$.Id
Issue-Key: pinpoint|DeleteBaiduChannel|2xx->2xx|$.IsArchived
Issue-Key: pinpoint|GetBaiduChannel|4xx->4xx|$.Message
Issue-Key: pinpoint|SendMessages|2xx->2xx|$.Result.sim2real-probe-gcm-token.StatusMessage
Issue-Commit: 042a4a492fb2cb15accf4d750ae4de40332fcbc6

# pinpoint Baidu channel and SendMessages responses diverge: deleted-channel body members and the 'channel not found. Please enable ...' message template

> _Note: this issue was identified against the code base at commit `042a4a492fb2cb15accf4d750ae4de40332fcbc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBaiduChannel

## Description
3 net-new root causes in the Pinpoint emulator, all on operations that ARE implemented and answer the right HTTP status: the emulator's response BODIES diverge from the service's. Two of them are the same family — the service's "<CHANNEL> channel not found. Please enable the <CHANNEL> channel for this application <ApplicationId>." text, which the emulator does not produce anywhere — and one is the deleted-channel body DeleteBaiduChannel returns. Causes are numbered; a fixer must close ALL THREE before submitting, and each names its own fixture and step.

Note on overlap with open issues: 20261112T053500Z-pinpoint-2-net-new-root-causes-getapp-s describes Cause 3 below from the BEHAVIORAL corpus (check pinpoint.push-channel-state-transition-changes-send-messages-outcome); the replay did not report that issue as owning this wire step, so Cause 3 is filed here with its wire grounding. 20261111T071029Z-pinpoint-notfoundexception-body-diverges-requestid-member-omitted-and owns GetApp's NotFoundException body (fixture pinpoint-app-lifecycle#task#4) and is NOT re-filed here. Neither open issue covers Cause 1 or Cause 2.

=== Cause 1 of 3: DeleteBaiduChannel's returned BaiduChannelResponse misreports Id, IsArchived, Enabled and HasCredential ===
AWS: Smithy: DeleteBaiduChannel -> DELETE /v1/apps/{ApplicationId}/channels/baidu, success 200 with BaiduChannelResponse as the httpPayload. BaiduChannelResponse.Id is documented "(Deprecated) An identifier for the Baidu channel. This property is retained only for backward compatibility." and HasCredential "(Not used) This property is retained only for backward compatibility."; IsArchived is "Specifies whether the Baidu channel is archived" and Enabled "Specifies whether the Baidu channel is enabled for the application". The model does not state the VALUES, so the following is recorded behavior only: deleting a configured, enabled Baidu channel answers 200 with Id = the fixed channel token "baidu" (NOT the application id — ApplicationId is the member that carries that), IsArchived = true (deletion archives the channel), Enabled = true (the response describes the channel's stored enabled state at deletion; the service does not flip it to false), and HasCredential = false (the back-compat member is not derived from whether a credential was stored).
Emulator: status 200 is correct, but all four members are inverted or wrong: Id = the ApplicationId from the request path, IsArchived = false, Enabled = false, HasCredential = true (true because a credential WAS supplied when the channel was configured).
Why: a client reading the delete response cannot tell the channel was archived (IsArchived false reads as a live channel), sees Enabled flipped so a read-after-delete audit records the wrong prior state, and gets the application id where the channel identifier belongs — so code keying channel records by Id collapses all of an application's channels onto one key. HasCredential, documented as unused, becomes a signal that varies with the request.
Fix: the shared BaiduChannelResponse projection in the service state (Channel::to_baidu_response, which UpdateBaiduChannel/GetBaiduChannel/DeleteBaiduChannel all build from) for Id and HasCredential, plus the DeleteBaiduChannel handler's delete branch, which must archive the channel rather than clear Enabled.
Traces: pinpoint-baidu-channel-lifecycle#task#4 (expected Id 'baidu', IsArchived true, Enabled true, HasCredential false; got Id = the generated application id, IsArchived false, Enabled false, HasCredential true). The hex application id in the replay output is a per-run generated value and is not part of the requirement. Code basis: services/service-com-amazonaws-pinpoint-pinpoint/src/state.rs:208-224 (id: Some(self.application_id.clone()), has_credential: Some(self.has_credential)); services/service-com-amazonaws-pinpoint-pinpoint/src/ops/delete_baidu_channel.rs:181-189 (sets deleted.enabled = false, never sets is_archived).

=== Cause 2 of 3: GetBaiduChannel on an application whose Baidu channel is not configured returns the generic 'Resource not found' instead of the channel-specific message ===
AWS: Smithy: GetBaiduChannel -> GET /v1/apps/{ApplicationId}/channels/baidu, modeled error NotFoundException @httpError 404, a structure of Message ("The message that's returned from the API.") and RequestID. Recorded behavior for the text: when the application EXISTS but has no Baidu channel configured (here: right after DeleteBaiduChannel), the service answers 404 NotFoundException with Message "BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>." The FIXED parts are " channel not found. Please enable the ", " channel for this application " and the terminating "."; the VARIABLE parts are the channel type (BAIDU for this operation, appearing twice) and the ApplicationId, taken from the request path. This is distinct from the application-level case: an application that does not exist is "Application not found", and the generic "Resource not found" is not what a missing channel on a live application returns.
Emulator: status 404 and error code NotFoundException are correct, but Message is the generic fixed string "Resource not found" — it drops the channel classification, the imperative "Please enable the <CHANNEL> channel" clause, the ApplicationId and the trailing period.
Why: this message is the only thing that tells the caller WHICH resource is missing and what to do about it; a client (or operator tooling) that distinguishes "application missing" from "channel not enabled on this application" by message text cannot, and the response never names the application the channel has to be enabled on.
Fix: the GetBaiduChannel handler's channel-missing branch (the NotFoundException it constructs when the application is present but app.channels has no BAIDU entry), which needs the service's channel-not-found template rather than a generic literal. The same template is needed in Cause 3, so it belongs in one shared helper.
Traces: pinpoint-baidu-channel-lifecycle#task#5 (expected Message 'BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>.', got 'Resource not found'). Code basis: services/service-com-amazonaws-pinpoint-pinpoint/src/ops/get_baidu_channel.rs:175-181.

=== Cause 3 of 3: SendMessages' per-destination StatusMessage for a channel the application has not enabled is emulator prose ===
AWS: Smithy: SendMessages -> POST /v1/apps/{ApplicationId}/messages, success 200 with MessageResponse; MessageResult.StatusMessage is "The status message for delivering the message" and StatusCode "The downstream service status code for delivering the message" — the model does not state the text, so recorded behavior only. When a direct send names an address whose ChannelType is a push channel the application has not enabled, the service answers 200 overall and that destination's MessageResult carries StatusMessage "GCM channel not found. Please enable the GCM channel for this application <ApplicationId>." — the same template as Cause 2: fixed " channel not found. Please enable the ", " channel for this application " and the trailing ".", with the channel type (taken from the destination's ChannelType in the request) and the ApplicationId (from the request path) as the variable parts.
Emulator: the overall 200 and the destination's other members are right, but StatusMessage is "The GCM channel is not enabled for this application" — emulator-authored prose with no "channel not found" classification, no "Please enable" clause, no ApplicationId and no trailing period.
Why: the per-destination StatusMessage is the only per-address explanation a direct send returns, so clients surface it to operators and some match on it to separate "channel not configured" from a provider-level rejection. Against the emulator that text never matches AWS's, and it does not name the application the channel must be enabled on.
Fix: the SendMessages handler's per-destination verdict branch for an unconfigured/disabled channel (the Target::verdict formatting of "The {channel} channel is not enabled for this application"), using the shared channel-not-found template from Cause 2.
Traces: pinpoint-send-messages-channel-not-enabled#task#2 ($.Result.<address>.StatusMessage expected 'GCM channel not found. Please enable the GCM channel for this application <ApplicationId>.', got 'The GCM channel is not enabled for this application'). The address key and the hex application id in the replay output are per-run values, not requirements. Code basis: services/service-com-amazonaws-pinpoint-pinpoint/src/ops/send_messages.rs (Target::verdict, ~line 542).

## Evidence
Corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up true, 38 steps, 16 passed, 22 mismatched, score 42.1%. All three causes are verdict 'mismatch' — the emulator answered with the right status and the wrong body.

Cause 1 — fixture pinpoint-baidu-channel-lifecycle, step task#4, DeleteBaiduChannel, expected 200 / actual 200:
  $.Id: expected 'baidu', got '<the request's ApplicationId>'
  $.IsArchived: expected true, got false
  $.Enabled: expected true, got false
  $.HasCredential: expected false, got true
  Model basis: DeleteBaiduChannel DELETE /v1/apps/{ApplicationId}/channels/baidu -> 200, output member BaiduChannelResponse (httpPayload, required); BaiduChannelResponse.Id '(Deprecated) ... retained only for backward compatibility', HasCredential '(Not used) ... retained only for backward compatibility', IsArchived, Enabled, ApplicationId.
  Code basis: src/state.rs:208-224 Channel::to_baidu_response sets id from application_id and has_credential from the stored flag; src/ops/delete_baidu_channel.rs:184 sets deleted.enabled = false and leaves is_archived untouched.

Cause 2 — fixture pinpoint-baidu-channel-lifecycle, step task#5, GetBaiduChannel, expected 404 / actual 404:
  $.Message: expected 'BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>.', got 'Resource not found'
  Model basis: GetBaiduChannel GET /v1/apps/{ApplicationId}/channels/baidu, error NotFoundException (errorKind client, httpError 404) {Message: __string, RequestID: __string}.
  Code basis: src/ops/get_baidu_channel.rs:175-181 returns the literal "Resource not found" for a live application with no BAIDU channel.

Cause 3 — fixture pinpoint-send-messages-channel-not-enabled, step task#2, SendMessages, expected 200 / actual 200:
  $.Result.<address>.StatusMessage: expected 'GCM channel not found. Please enable the GCM channel for this application <ApplicationId>.', got 'The GCM channel is not enabled for this application'
  Model basis: SendMessages POST /v1/apps/{ApplicationId}/messages -> 200 MessageResponse; MessageResult {DeliveryStatus required, StatusCode required, StatusMessage, MessageId, UpdatedToken}.
  Code basis: src/ops/send_messages.rs Target::verdict (~542) formats "The {channel} channel is not enabled for this application".

Skipped as already owned: pinpoint-app-lifecycle#task#4 (GetApp NotFoundException body), owned by 20261111T071029Z-pinpoint-notfoundexception-body-diverges-requestid-member-omitted-and.

## Steps to Reproduce
Boot the emulator from this checkout and replay corpus simparity-wire-aws-v1.

Cause 1 (fixture pinpoint-baidu-channel-lifecycle, step task#4):
1. CreateApp with any name; note ApplicationResponse.Id.
2. UpdateBaiduChannel on that application with Enabled=true and an ApiKey/SecretKey (a stored credential).
3. DeleteBaiduChannel on that application. The call answers 200, but the BaiduChannelResponse reports Id = the ApplicationId (AWS: 'baidu'), IsArchived false (AWS: true), Enabled false (AWS: true) and HasCredential true (AWS: false).

Cause 2 (fixture pinpoint-baidu-channel-lifecycle, step task#5):
4. Immediately call GetBaiduChannel on the SAME, still-existing application. It is correctly rejected 404 NotFoundException, but Message is 'Resource not found'; AWS answers 'BAIDU channel not found. Please enable the BAIDU channel for this application <that ApplicationId>.'

Cause 3 (fixture pinpoint-send-messages-channel-not-enabled, step task#2):
5. CreateApp with any name; note ApplicationResponse.Id. Configure NO channels.
6. Call SendMessages on it with MessageRequest.Addresses = {<some push token>: {ChannelType: 'GCM'}} and a matching MessageConfiguration. The call answers 200, but Result[<token>].StatusMessage is 'The GCM channel is not enabled for this application'; AWS returns 'GCM channel not found. Please enable the GCM channel for this application <that ApplicationId>.'

Re-replay fixtures pinpoint-baidu-channel-lifecycle (steps task#4 and task#5) and pinpoint-send-messages-channel-not-enabled (step task#2) to confirm all three causes are closed.
