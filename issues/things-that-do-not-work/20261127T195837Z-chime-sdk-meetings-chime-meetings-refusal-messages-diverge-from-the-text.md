Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-meetings-get-attendee-non-guid-ids-validation-error
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-meetings-get-attendee-non-guid-ids-validation-error chime-sdk-meetings-get-meeting-non-guid-id-validation-error chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected chime-sdk-meetings-list-attendees-invalid-paging-input-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|GetAttendee|4xx->4xx|$.message
Issue-Key: chime-sdk-meetings|GetMeeting|4xx->4xx|$.message
Issue-Key: chime-sdk-meetings|StopMeetingTranscription|4xx->4xx|$.message
Issue-Key: chime-sdk-meetings|ListAttendees|4xx->4xx|$.Message
Issue-Key: chime-sdk-meetings|ListAttendees|4xx->4xx|$.RequestId
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# Chime meetings refusal messages diverge from the text AWS words for them

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAttendee

## Description
The emulator refuses these calls with the right status but the wrong message text. An error message is part of the contract, so each of these is a divergence a client parsing or asserting on the message sees. Four causes: three share one root cause in the constraint-violation message renderer, the fourth is the pagination refusal's wording.

Cause 1 of 4: GetAttendee's pattern-violation message quotes the regex with anchors AWS does not include.
AWS: Smithy: GetAttendee.AttendeeId targets GuidString with @pattern "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$" and @required; BadRequestException is httpError 400. The recording shows AWS rendering the standard constraint-violation message, whose fixed text is "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>": the count, the rejected value taken from the request, the lowerCamel member name ('attendeeId') and the pattern are the variable parts, and the pattern is rendered WITHOUT the leading ^ and trailing $ anchors, i.e. "[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}".
Emulator: emits the same sentence but with the model's anchors left in: "... pattern: ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$".
Why: a client that matches or displays the advertised pattern gets a regex that differs from the one AWS publishes in the error; anything comparing the message to AWS's text, or extracting the pattern to re-validate input locally, diverges.
Fix: the shared constraint-violation message renderer used by the service's input validation — it must strip the anchors from the @pattern value when formatting the message.
Traces: chime-sdk-meetings-get-attendee-non-guid-ids-validation-error#task#4 and #task#5.

Cause 2 of 4: GetMeeting's meetingId pattern-violation message carries the same anchors.
AWS: Smithy: GetMeeting.MeetingId targets GuidString with the same @pattern; BadRequestException httpError 400. Same fixed sentence, member name 'meetingId', pattern rendered unanchored.
Emulator: same anchored pattern in the message.
Why: same.
Fix: same renderer (one fix should close causes 1-3; each must still be re-replayed).
Traces: chime-sdk-meetings-get-meeting-non-guid-id-validation-error#task#3.

Cause 3 of 4: StopMeetingTranscription's meetingId pattern-violation message carries the same anchors.
AWS: Smithy: StopMeetingTranscription.MeetingId targets GuidString with the same @pattern; BadRequestException httpError 400. Same unanchored rendering, member name 'meetingId'.
Emulator: same anchored pattern in the message.
Why: same.
Fix: same renderer.
Traces: chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected#task#3.

Cause 4 of 4: ListAttendees refuses an unusable NextToken with its own wording instead of AWS's.
AWS: Smithy: ListAttendees.NextToken is httpQuery 'next-token' (@length 0..4096) and the operation models BadRequestException (httpError 400) with members Code, Message, RequestId. The recording shows AWS answering 400 with Message set to the fixed text "Invalid pagination token" — the whole message is fixed, with no request-derived part.
Emulator: answers 400 but with Message "NextToken is not a token this service issued", and omits RequestId.
Why: a client that branches on the pagination-failure message, or asserts it, does not match; the text is not one AWS ever returns.
Fix: ListAttendees' pagination-token validation, where the refusal's Message is built.
Traces: chime-sdk-meetings-list-attendees-invalid-paging-input-rejected#task#6.
Related open issue, which does NOT cover this: "A NextToken this listing never issued is accepted, although the crate states such a token must be refused" (20261124T130804Z-chime-sdk-meetings-listattendees-pagination.md) is about a token being accepted and carries no replay key for this step; here the token IS refused, with the wrong message.

The RequestId omission these refusals also show is filed separately as the chime-sdk-meetings error-body RequestId issue; the requirement here is the message text.

## Evidence
Replayed divergences, all verdict=mismatch with the expected status matched:
- chime-sdk-meetings-get-attendee-non-guid-ids-validation-error#task#4 (GetAttendee, 400/400): "$.message: value_mismatch (expected \"1 validation error detected: Value 'sim2real-probe-not-a-guid-27b31929' at 'attendeeId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}\", got the same sentence with pattern '^...$')". Cluster also covers #task#5.
- chime-sdk-meetings-get-meeting-non-guid-id-validation-error#task#3 (GetMeeting, 400/400): same mismatch at 'meetingId'.
- chime-sdk-meetings-stop-meeting-transcription-malformed-meeting-id-rejected#task#3 (StopMeetingTranscription, 400/400): same mismatch at 'meetingId'.
- chime-sdk-meetings-list-attendees-invalid-paging-input-rejected#task#6 (ListAttendees, 400/400): "$.Message: value_mismatch (expected 'Invalid pagination token', got 'NextToken is not a token this service issued')".
Smithy: GuidString @pattern "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$" on GetAttendee.AttendeeId/MeetingId, GetMeeting.MeetingId, StopMeetingTranscription.MeetingId; BadRequestException (400) members Code, Message, RequestId.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call GetAttendee with a MeetingId/AttendeeId that is not a GUID; read the 400 body's message member — the quoted regular expression pattern carries a leading ^ and trailing $ that AWS's message does not.
3. Repeat with GetMeeting and with StopMeetingTranscription on a non-GUID meeting id; the same anchored pattern appears.
4. CreateMeeting, then call ListAttendees with a next-token the service never issued; the 400 body's Message reads 'NextToken is not a token this service issued' where AWS returns 'Invalid pagination token'.
