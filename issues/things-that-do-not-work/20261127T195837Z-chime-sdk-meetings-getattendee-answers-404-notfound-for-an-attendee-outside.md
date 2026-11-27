Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|$.Code
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|$.Message
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|status
Issue-Related: 20261127T195837Z-chime-sdk-meetings-chime-meetings-error-bodies-omit-the-modelled-requestid.md
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# GetAttendee answers 404 NotFound for an attendee outside the named meeting where AWS answers 403 Forbidden

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAttendee

## Description
One cause.

AWS: Smithy: GetAttendee binds GET /meetings/{MeetingId}/attendees/{AttendeeId} and models both ForbiddenException (httpError 403, members Code, Message, RequestId, documented "The client is permanently forbidden from making the request.") and NotFoundException (httpError 404). When the request pairs an attendee with a meeting that is not the attendee's meeting, AWS refuses with ForbiddenException: HTTP 403, Code "Forbidden", x-amzn-errortype naming the ForbiddenException shape, and Message whose fixed text is "Attendee <" ... "> is not in the meeting <" ... ">" — the angle brackets are literal and the two variable parts are the AttendeeId and the MeetingId taken from the request path. The refusal does not depend on whether the named meeting resolves: the mismatch itself is forbidden, so the meeting-existence check must not pre-empt it.
Emulator: answers HTTP 404 with Code "NotFound", x-amzn-errortype "NotFoundException" and Message "Meeting <meetingId> not found" — it resolves the MeetingId first and reports the meeting as absent, never reaching the attendee/meeting mismatch rule. (RequestId is also absent, which the separate RequestId issue covers.)
Why: a client distinguishing "this pairing is not permitted" from "this meeting is gone" takes the wrong branch: a 403 is permanent and must not be retried or treated as a deleted meeting, while a 404 typically drives cleanup of local meeting state. SDK error matching on ForbiddenException never fires, and code that recreates or forgets a meeting on NotFound will do so for a meeting that still exists. The same fixture's next step shows the emulator CAN answer 403 here, so only the resolution order and the chosen exception diverge.
Fix: GetAttendee's handler — the order of its refusals. The attendee/meeting membership check must produce ForbiddenException with the "Attendee <id> is not in the meeting <id>" message before, not after, the meeting-store lookup that currently yields NotFoundException.
Traces: chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden#task#4 (the diverging step); #task#5 of the same fixture reaches 403 correctly and is the contrast.

## Evidence
Replayed divergence chime-sdk-meetings-get-attendee-outside-its-meeting-forbidden#task#4, operation GetAttendee, verdict=mismatch:
"status: expected 403, got 404
$.Code: value_mismatch (expected 'Forbidden', got 'NotFound')
$.Message: value_mismatch (expected 'Attendee <3a7c1642-8922-47b8-a074-ee54bdb69157> is not in the meeting <3f6c1a9e-52d4-4b87-9e0a-55a6f013e10a>', got 'Meeting <3f6c1a9e-52d4-4b87-9e0a-55a6f013e10a> not found')
$.RequestId: missing
$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'NotFoundException')"
The GUIDs above are that run's ids and fill the message template; the requirement is the 403/Forbidden/ForbiddenException classification and the "Attendee <attendeeId> is not in the meeting <meetingId>" wording.
Smithy: GetAttendee errors include ForbiddenException (httpError 403) and NotFoundException (httpError 404); ForbiddenException members are Code, Message, RequestId.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateMeeting and CreateAttendee to obtain a real AttendeeId in meeting A.
3. Call GetAttendee with that AttendeeId but a different, well-formed GUID MeetingId that is not meeting A.
4. Expected: HTTP 403, Code 'Forbidden', x-amzn-errortype ForbiddenException, Message 'Attendee <attendeeId> is not in the meeting <meetingId>'. Actual: HTTP 404, Code 'NotFound', x-amzn-errortype 'NotFoundException', Message 'Meeting <meetingId> not found'.
