Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-meetings-batch-create-attendee-unknown-meeting-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-meetings-batch-create-attendee-unknown-meeting-not-found chime-sdk-meetings-batch-update-capabilities-except-content-without-video-rejected chime-sdk-meetings-batch-update-capabilities-except-unknown-meeting-not-found chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-meetings|BatchUpdateAttendeeCapabilitiesExcept|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|$.RequestId
Issue-Key: chime-sdk-meetings|StopMeetingTranscription|4xx->4xx|$.RequestId
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# Chime meetings error bodies omit the modelled RequestId member on every refusal

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateAttendee

## Description
Every modelled client error of chime-sdk-meetings carries a RequestId member, and AWS populates it on each refusal. The emulator's error serializer never emits it, so the member is absent from the body no matter which operation or which error class refused. Five clusters show the same omission across four operations and three status codes, which is why they are bundled: one error-serialization defect, five causes to close.

Cause 1 of 5: BatchCreateAttendee on a meeting id that names no meeting.
AWS: Smithy: BatchCreateAttendee errors include NotFoundException (httpError 404); NotFoundException members are Code, Message and RequestId, the last documented as "The request ID associated with the call responsible for the exception." AWS answers 404 with Code, Message and a RequestId whose value is the id of the request being refused.
Emulator: answers 404 with Code and Message but no RequestId key at all ($.RequestId: missing).
Why: a client that logs or reports the RequestId for a failed call — which is what AWS Support asks for, and what SDK error objects surface as the request id — gets nothing, and correlation between a client-side failure and a server-side refusal is impossible.
Fix: the service's shared error serializer (the restJson1 error-body writer used by every chime-sdk-meetings handler), which must populate RequestId from the in-flight request id for every modelled exception.
Traces: chime-sdk-meetings-batch-create-attendee-unknown-meeting-not-found#task#2.

Cause 2 of 5: BatchUpdateAttendeeCapabilitiesExcept refusing a Content-without-Video capability combination.
AWS: Smithy: BatchUpdateAttendeeCapabilitiesExcept errors include BadRequestException (httpError 400) with members Code, Message, RequestId. AWS answers 400 with RequestId present.
Emulator: 400 with the right status and message, RequestId missing.
Why: same loss of correlation, on the validation path.
Fix: same shared error serializer; the capability-validation refusal must go through it.
Traces: chime-sdk-meetings-batch-update-capabilities-except-content-without-video-rejected#task#4.

Cause 3 of 5: BatchUpdateAttendeeCapabilitiesExcept on an unknown meeting.
AWS: Smithy: NotFoundException (httpError 404), members Code, Message, RequestId. 404 with RequestId present.
Emulator: 404, RequestId missing.
Why: same.
Fix: same shared error serializer.
Traces: chime-sdk-meetings-batch-update-capabilities-except-unknown-meeting-not-found#task#4.

Cause 4 of 5: GetAttendee refused with ForbiddenException.
AWS: Smithy: GetAttendee errors include ForbiddenException (httpError 403), members Code, Message, RequestId documented "The request id associated with the call responsible for the exception." 403 with RequestId present.
Emulator: 403 with Code 'Forbidden' and the right message, RequestId missing.
Why: same.
Fix: same shared error serializer.
Traces: chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden#task#5.

Cause 5 of 5: StopMeetingTranscription on a meeting that does not exist.
AWS: Smithy: StopMeetingTranscription errors include NotFoundException (httpError 404), members Code, Message, RequestId. 404 with RequestId present.
Emulator: 404, RequestId missing.
Why: same.
Fix: same shared error serializer.
Traces: chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected#task#4.

Note on values: the RequestId value itself is per-run and is not a requirement — the requirement is that the member is present and carries the id of the request being refused. The recorded fixtures assert it as shaped-not-asserted for exactly that reason.

## Evidence
Replayed divergences, all verdict=mismatch with matching status:
- chime-sdk-meetings-batch-create-attendee-unknown-meeting-not-found#task#2 (BatchCreateAttendee, 404 expected / 404 actual): "$.RequestId: missing (expected '<shaped-not-asserted>', got None)".
- chime-sdk-meetings-batch-update-capabilities-except-content-without-video-rejected#task#4 (BatchUpdateAttendeeCapabilitiesExcept, 400/400): "$.RequestId: missing".
- chime-sdk-meetings-batch-update-capabilities-except-unknown-meeting-not-found#task#4 (BatchUpdateAttendeeCapabilitiesExcept, 404/404): "$.RequestId: missing".
- chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden#task#5 (GetAttendee, 403/403): "$.RequestId: missing".
- chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected#task#4 (StopMeetingTranscription, 404/404): "$.RequestId: missing".
Expected: body contains a RequestId member on each of these refusals. Actual: the key is absent in all five.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the chime-sdk-meetings wire corpus.
2. Call BatchCreateAttendee with a well-formed GUID MeetingId that names no meeting; observe the 404 body.
3. Repeat with BatchUpdateAttendeeCapabilitiesExcept (unknown meeting -> 404; Content capability without Video -> 400), GetAttendee on an attendee outside the named meeting (403), and StopMeetingTranscription on an unknown meeting (404).
4. In each response body, look for the RequestId member: it is absent, where AWS returns it alongside Code and Message.
