Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|$.Message
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|status
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# GetHLSStreamingSessionURL ON_DEMAND request validation returns the wrong error class and the wrong refusal wording

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHLSStreamingSessionURL

## Description
Two net-new causes, both in the request-validation path of GetHLSStreamingSessionURL for PlaybackMode ON_DEMAND. The shared fault is that the emulator's ON_DEMAND pre-checks do not reproduce the refusals AWS raises for the requested HLS timestamp range / fragment selector: one refusal is raised as the wrong error class and HTTP status, the other with wording of the emulator's own invention. Both are reached with a well-formed ON_DEMAND session request, so a client that keys on the status/error code or surfaces the service message to a user sees a different API than AWS.

Cause 1 of 2: an ON_DEMAND HLSTimestampRange whose EndTimestamp falls outside the stream's retention period is answered 404 ResourceNotFoundException instead of a 400 refusal naming the retention period.
AWS: GetHLSStreamingSessionURL models InvalidArgumentException with httpError 400 ("A specified parameter exceeds its restrictions, is not supported, or can't be used.") alongside ResourceNotFoundException with httpError 404, whose documentation scopes the 404 narrowly: GetHLSStreamingSessionURL/GetDASHStreamingSessionURL "throw this error if a session with a PlaybackMode of ON_DEMAND or LIVE_REPLAY is requested for a stream that has no fragments within the requested time range" (smithy_query shape=ResourceNotFoundException,InvalidArgumentException; operation=GetHLSStreamingSessionURL errors). A range bound that the stream may not be asked about at all is a parameter restriction, not an empty result set. The recorded response pins status 400 with Message "End timestamp <endTimestamp> is outside of the stream retention period." — the fixed text is "End timestamp " ... " is outside of the stream retention period." and the only variable part is the request's own HLSFragmentSelector.TimestampRange.EndTimestamp, rendered as epoch milliseconds; the recorded x-amzn-errortype was not asserted. Hypothesis: the 400 is InvalidArgumentException, the only modeled 400 whose documented meaning covers an out-of-range parameter.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and Message "No fragments found in the stream for the streaming request." — i.e. the retention-window bound is never evaluated and the request falls through to the no-fragments-found path.
Why: a client distinguishing "you asked for a window the stream cannot retain" (fix the request, 400, non-retryable argument error) from "the stream currently holds nothing in that window" (404, retry later / widen the window) takes the wrong branch. SDK error-shape dispatch binds ResourceNotFoundException handlers instead of InvalidArgumentException handlers, and the message a user sees no longer names the retention period that actually blocked the session.
Fix: the ON_DEMAND range validation of the GetHLSStreamingSessionURL handler (the shared session-parameter validation used by both streaming-session operations), which must compare the selector's EndTimestamp against the stream's retention period and refuse with the 400 argument error BEFORE the fragment lookup decides there is nothing to serve.
Traces: kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules#task#5.

Cause 2 of 2: the refusal for an ON_DEMAND request with no usable fragment selector is worded differently from AWS.
AWS: GetHLSStreamingSessionURLInput$HLSFragmentSelector documents "This parameter is required if PlaybackMode is ON_DEMAND or LIVE_REPLAY ... If PlaybackMode is ON_DEMAND or LIVE_REPLAY, both FragmentSelectorType and TimestampRange must be set" (smithy_query doc=GetHLSStreamingSessionURLInput$HLSFragmentSelector), and the refusal is a 400. The recorded response pins status 400 with Message "The FragmentSelector is required for ON_DEMAND PlaybackMode." — that string is entirely fixed text for this error class; it has no variable parts taken from the request or from state (notably it says FragmentSelector, not HLSFragmentSelector, and it does not name TimestampRange).
Emulator: answers the correct 400 but with Message "HLSFragmentSelector with a TimestampRange is required when PlaybackMode is ON_DEMAND" — different fixed text, a different member name, and no terminating period.
Why: the error message is part of the contract for this error class. Clients, CLI output and test assertions that match on the service's refusal text (and support flows that quote it) see a string AWS never emits, so the emulator cannot be used to validate behavior that depends on the refusal wording.
Fix: the message constant used by GetHLSStreamingSessionURL's missing-fragment-selector validation; the status is already right, only the emitted text diverges. The same wording must be used for every ON_DEMAND request that lacks a selector or lacks its timestamp range (the cluster covers three such steps).
Traces: kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules#task#6, #task#7, #task#8.

Neither cause is owned by an open issue: the replay reported no owning issue and no stub for either cluster. The nearest open issues concern different rules and do not cover these — 20261126T151438Z-kinesis-video-archived-media-gethlsstreamingsessionurl-validation--r2.md (the unimplemented "SERVER_TIMESTAMP EndTimestamp must be in the past" clause), 20261127T043220Z-kinesis-video-archived-media-gethlsstreamingsessionurl-wrong-behavior--r4.md (producer-timestamp deduplication order) and 20261127T101002Z-kinesis-video-archived-media-8-net-new-causes.md (session-URL scheme/manifest naming and codec refusals).

## Evidence
Replay of fixture kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules (trace set simparity-wire-aws-v1; 76 steps, 72 passed, 4 mismatched, 2 divergence clusters, both kinesis-video-archived-media / GetHLSStreamingSessionURL).

Cause 1 — step task#5 (1 step, verdict mismatch):
  status: expected 400, got 404
  $.Message: expected 'End timestamp 1795843689000 is outside of the stream retention period.', got 'No fragments found in the stream for the streaming request.'
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'ResourceNotFoundException'
  (1795843689000 is this recording's EndTimestamp value, not part of the required text: the template is 'End timestamp <request EndTimestamp in epoch millis> is outside of the stream retention period.')

Cause 2 — steps task#6, task#7, task#8 (3 steps, verdict mismatch, status 400 on both sides):
  $.Message: expected 'The FragmentSelector is required for ON_DEMAND PlaybackMode.', got 'HLSFragmentSelector with a TimestampRange is required when PlaybackMode is ON_DEMAND'

Model grounding: smithy_query(query="operation", operation="GetHLSStreamingSessionURL") — errors InvalidArgumentException (400), ResourceNotFoundException (404), http POST /getHLSStreamingSessionURL; smithy_query(query="shape", shape="ResourceNotFoundException") — 404 is documented only for "a stream that has no fragments within the requested time range"; smithy_query(query="doc", shape="GetHLSStreamingSessionURLInput", member="HLSFragmentSelector") — required for ON_DEMAND, both FragmentSelectorType and TimestampRange must be set.

## Steps to Reproduce
Cause 1:
1. Seed one ACTIVE stream with a finite data-retention period holding fragments.
2. POST /getHLSStreamingSessionURL with PlaybackMode: ON_DEMAND and an HLSFragmentSelector whose TimestampRange.EndTimestamp lies outside the stream's retention period (ordering and 24-hour span rules satisfied).
3. Observe HTTP 404 / x-amzn-errortype ResourceNotFoundException with Message "No fragments found in the stream for the streaming request."; AWS answers HTTP 400 with Message "End timestamp <that EndTimestamp in epoch millis> is outside of the stream retention period."

Cause 2:
1. Seed one ACTIVE stream with retention > 0.
2. POST /getHLSStreamingSessionURL with PlaybackMode: ON_DEMAND and no HLSFragmentSelector (and again with an HLSFragmentSelector that omits TimestampRange).
3. Observe HTTP 400 with Message "HLSFragmentSelector with a TimestampRange is required when PlaybackMode is ON_DEMAND"; AWS answers HTTP 400 with Message "The FragmentSelector is required for ON_DEMAND PlaybackMode."

Replay both with: replay fixture kinesisvideoarchivedmedia-hls-session-url-fragment-selector-rules, steps task#5 (cause 1) and task#6/task#7/task#8 (cause 2).
