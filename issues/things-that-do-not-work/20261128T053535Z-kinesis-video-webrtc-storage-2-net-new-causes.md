Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-webrtc-storage.join-storage-session-unlinked-channel-reports-no-mapping kinesis-video-webrtc-storage.join-storage-session-as-viewer-never-linked-channel-no-mapping kinesis-video-webrtc-storage.join-storage-session-deleted-channel-arn-not-found
Issue-Shape-Id: com.amazonaws.kinesisvideowebrtcstorage#AWSAcuityRoutingServiceLambda
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-unlinked-channel-reports-no-mapping|_refused#19|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-as-viewer-never-linked-channel-no-mapping|_rejection#15|AssertionError|raise AssertionError(f"expected the call to be rejected, it returned {reply!r}")
Issue-Key: kinesis-video-webrtc-storage.join-storage-session-deleted-channel-arn-not-found|run#22|AssertionError|assert got == ("ResourceNotFoundException", _GONE), f"join on the deleted channel: {got!r}"
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# kinesis-video-webrtc-storage: 2 net-new causes — JoinStorageSession/JoinStorageSessionAsViewer never check the channel's media-storage mapping, and the not-found refusal uses emulator-authored wording

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
JoinStorageSession

## Description
Two independent root causes in this crate's two join operations. Both must be closed before submitting.

Cause 1 of 2: a join on a channel with no ENABLED media-storage configuration is accepted with 200 instead of being refused InvalidArgumentException. Failing checks: kinesis-video-webrtc-storage.join-storage-session-unlinked-channel-reports-no-mapping, kinesis-video-webrtc-storage.join-storage-session-as-viewer-never-linked-channel-no-mapping.
AWS: JoinStorageSession and JoinStorageSessionAsViewer are the WebRTC *storage* plane — they record the negotiated media into the Kinesis video stream the signaling channel is mapped to by kinesisvideo UpdateMediaStorageConfiguration. Smithy (services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda/smithy-model/...json): both operations take only channelArn (JoinStorageSessionAsViewer also clientId) and declare InvalidArgumentException (HTTP 400, "The value for this input parameter is invalid.") alongside AccessDeniedException/ClientLimitExceededException/ResourceNotFoundException. The model does not spell out the mapping rule, so for the wording this is recorded behavior only: when the addressed channel has no media-storage mapping in ENABLED state — either it was never linked, or its configuration was set to DISABLED — the call is refused HTTP 400 InvalidArgumentException with the message `No mapping found for the ChannelARN: <channelArn>`. The fixed text is `No mapping found for the ChannelARN: ` and the only variable part is the channel ARN taken verbatim from the request's channelArn member; no account id, region, channel name or timestamp appears in it. Linking a stream to that channel again (Status=ENABLED) must make the very same join succeed, and the refusal must not disturb the channel's storage configuration or any other channel's session.
Emulator: neither handler consults the media-storage configuration at all. Both answer HTTP 200 with an empty body (`{'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '0'}}`) and create/rotate a storage session for a channel whose configuration reads Status=DISABLED, both for a channel that was linked and then unlinked (JoinStorageSession) and for one never linked at all (JoinStorageSessionAsViewer).
Why: a client that relies on this refusal to discover that ingest is not provisioned gets a success and believes its media is being recorded to a Kinesis video stream; nothing is ever persisted, and the error class that tells an application to call UpdateMediaStorageConfiguration first is unreachable. Code that treats InvalidArgumentException as the "storage not configured" signal (and ENABLED-only joining) can never take that branch.
Fix: the two handlers' state-gating step, between the existing channel-existence check and the session join — resolve the channel's media-storage configuration from the kinesisvideo control-plane state and return the InvalidArgumentException above unless it is ENABLED with a stream ARN.

Cause 2 of 2: the ResourceNotFoundException message for a channel ARN the control plane does not hold is emulator-authored prose. Failing check: kinesis-video-webrtc-storage.join-storage-session-deleted-channel-arn-not-found.
AWS: a join whose channelArn is syntactically a signaling-channel ARN but names no live channel — a deleted channel's ARN, or a live channel's account/region/name with a creation suffix it never had — is refused HTTP 404 ResourceNotFoundException (modeled: "The specified resource is not found.") with the fixed, value-free message `The requested channel is not found or not active.` That message is entirely fixed text: it interpolates nothing from the request or from state (no channel name, no account id, no region). Recorded behavior confirms the same message for both the deleted ARN and the re-suffixed ARN, and the live channel stays joinable after the refusals.
Emulator: the code is right (ResourceNotFoundException) but the message is built from request/state values: `No signaling channel 'sim2real-kvnf-dead-418621ed1ac0' found in account 000000000000 for region us-west-2` — fixed text `No signaling channel '` / `' found in account ` / ` for region `, with the channel name, account id and region interpolated from the parsed ARN. AWS emits none of those parts.
Why: a client matching on the error message (and any test or log pipeline comparing it) sees text AWS never serves, and the emulator leaks the channel name/account/region back into an error AWS deliberately keeps value-free, which also tells an unauthorized prober what it addressed.
Fix: the not-found branch of both join handlers' error construction — replace the formatted message with the fixed AWS string.

Traces: see trace_ids; handler sources are services/service-com-amazonaws-kinesisvideowebrtcstorage-awsacuityroutingservicelambda/src/ops/join_storage_session.rs (lines 253-263 for the not-found message, session join at 271) and .../src/ops/join_storage_session_as_viewer.rs (lines 245-257 and 265).

## Evidence
Cause 1 — kinesis-video-webrtc-storage.join-storage-session-unlinked-channel-reports-no-mapping:
  expected: ClientError InvalidArgumentException, message "No mapping found for the ChannelARN: <channel ARN>" (HTTP 400)
  actual:   AssertionError: the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}}}
  (channel's DescribeMediaStorageConfiguration read Status=DISABLED immediately before the join)
Cause 1 — kinesis-video-webrtc-storage.join-storage-session-as-viewer-never-linked-channel-no-mapping:
  expected: InvalidArgumentException, "No mapping found for the ChannelARN: <channel ARN>"
  actual:   AssertionError: expected the call to be rejected, it returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '0'}} (channel never given a media storage configuration)

Cause 2 — kinesis-video-webrtc-storage.join-storage-session-deleted-channel-arn-not-found:
  expected: ('ResourceNotFoundException', 'The requested channel is not found or not active.')
  actual:   ('ResourceNotFoundException', "No signaling channel 'sim2real-kvnf-dead-418621ed1ac0' found in account 000000000000 for region us-west-2")
  (the name/account/region shown are this run's values filling the emulator's template; AWS's message has no variable parts at all)

## Steps to Reproduce
1. With kinesisvideo: CreateStream (DataRetentionInHours=1) and CreateSignalingChannel (SINGLE_MASTER); wait for stream ACTIVE and channel ACTIVE.
2. UpdateMediaStorageConfiguration(ChannelARN=channel, {StreamARN: stream, Status: ENABLED}); GetSignalingChannelEndpoint(Protocols=[WEBRTC], Role=MASTER) and point a kinesis-video-webrtc-storage client at the returned WEBRTC endpoint.
3. JoinStorageSession(channelArn=channel) -> succeeds (positive control).
4. Cause 1a: UpdateMediaStorageConfiguration(ChannelARN=channel, {Status: DISABLED}); confirm DescribeMediaStorageConfiguration reports DISABLED; call JoinStorageSession(channelArn=channel). Expected InvalidArgumentException "No mapping found for the ChannelARN: <channel>"; observed HTTP 200, empty body.
5. Cause 1b: create a second channel and never link any stream to it; GetSignalingChannelEndpoint(Protocols=[WEBRTC], Role=VIEWER) and call JoinStorageSessionAsViewer(channelArn=<unlinked channel>, clientId=<any>). Expected the same InvalidArgumentException; observed HTTP 200.
6. Cause 2: create a second channel, wait ACTIVE, DeleteSignalingChannel it, wait until DescribeSignalingChannel reports ResourceNotFoundException, then JoinStorageSession(channelArn=<deleted channel ARN>). Also try the live channel's ARN with its creation suffix incremented by 1. Expected ResourceNotFoundException with the fixed message "The requested channel is not found or not active."; observed the emulator's "No signaling channel '<name>' found in account <id> for region <region>".
