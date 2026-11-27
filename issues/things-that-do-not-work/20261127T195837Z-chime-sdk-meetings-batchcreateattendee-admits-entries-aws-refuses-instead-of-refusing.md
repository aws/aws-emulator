Issue-Source: trace-sweep-divergence
Issue-Trace: batch-create-attendee-refused-entry-refuses-whole-batch
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-create-attendee-refused-entry-refuses-whole-batch chime-sdk-meetings-batch-create-attendee-invalid-item-refuses-whole-batch batch-create-attendee-repeated-entry-refused-present-user-kept
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|$.Code
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|$.Message
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|$.RequestId
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|status
Issue-Key: chime-sdk-meetings|ListAttendees|2xx->2xx|$.Attendees[*]
Issue-Key: chime-sdk-meetings|BatchCreateAttendee|4xx->2xx|$.message
Issue-Key: chime-sdk-meetings|ListAttendees|2xx->2xx|$.Attendees[*].AttendeeId
Issue-Key: chime-sdk-meetings|ListAttendees|2xx->2xx|$.Attendees[*].ExternalUserId
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# BatchCreateAttendee admits entries AWS refuses instead of refusing the whole batch

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateAttendee

## Description
BatchCreateAttendee's per-item admission rules are not enforced: a batch containing an entry AWS rejects is accepted with 200 and its attendees are created. Because the refusal is missing, the meeting's attendee list afterwards also diverges, so each refusal cause is paired with a state cause that a fixer must re-check. Four causes.

Cause 1 of 4: an ExternalUserId beginning with the reserved 'aws:' prefix is admitted.
AWS: Smithy: CreateAttendeeRequestItem.ExternalUserId targets ExternalUserId (@required, @length min 2 max 64, @sensitive) and its documentation states "Values that begin with aws: are reserved. You can't configure a value that uses this prefix. Case insensitive." BatchCreateAttendee models BadRequestException (httpError 400) with members Code, Message, RequestId. The recording shows AWS refusing the whole call with HTTP 400, Code "BadRequest" and Message "Validation error for ExternalUserId, prefix 'aws:' is reserved." — the entire text is fixed for this rule, with "ExternalUserId" being the member name that failed and 'aws:' the reserved prefix; nothing in it comes from the request's values. The x-amzn-errortype header is set to the exception's shape name.
Emulator: answers HTTP 200 with a normal BatchCreateAttendeeResponse. No Code, no Message, no RequestId, no x-amzn-errortype.
Why: a client that relies on the reserved-prefix rule to reject builder-supplied user ids never sees the refusal; it believes an attendee identity in a namespace AWS reserves was successfully created, and its own error handling for that case is dead code.
Fix: BatchCreateAttendee's input validation — the per-item ExternalUserId admission check — must reject the whole request before any attendee is created.
Traces: batch-create-attendee-refused-entry-refuses-whole-batch#task#2; chime-sdk-meetings-batch-create-attendee-invalid-item-refuses-whole-batch#task#2 and #task#3.

Cause 2 of 4: the attendees of a batch AWS refuses are left in the meeting.
AWS: BatchCreateAttendee is all-or-nothing for a refused batch — the 400 above means no attendee is created, so a subsequent ListAttendees on that meeting returns no entry for any id in the refused batch (Smithy: ListAttendees output Attendees, type list).
Emulator: ListAttendees returns the attendee built from the refused entry, with a generated AttendeeId, JoinToken and the default SendReceive capabilities, where AWS returns no such element.
Why: the meeting's membership is wrong after a failed call, so capability updates, attendee counts and join flows all operate on an attendee that does not exist on AWS; the leak persists for the life of the meeting.
Fix: BatchCreateAttendee's state mutation — nothing may be written to the attendee store unless every item passed admission.
Traces: batch-create-attendee-refused-entry-refuses-whole-batch#task#3 and #task#5; chime-sdk-meetings-batch-create-attendee-invalid-item-refuses-whole-batch#task#5.

Cause 3 of 4: a batch whose items repeat an attendee entry is admitted.
AWS: BatchCreateAttendee models BadRequestException (httpError 400). The recording shows AWS refusing the call with HTTP 400, a body whose message member is the fixed text "Provided list of attendee items contains duplicates" (no request-derived parts) and the x-amzn-errortype header set.
Emulator: answers HTTP 200 with a normal response; the body carries no message member and no x-amzn-errortype header.
Why: a client batching attendee creations gets no signal that its list was malformed and that duplicate identities were requested; it proceeds as though each duplicate were a distinct attendee.
Fix: BatchCreateAttendee's input validation — the duplicate-item check over the Attendees list, applied before admission of any item.
Traces: batch-create-attendee-repeated-entry-refused-present-user-kept#task#3; chime-sdk-meetings-batch-create-attendee-invalid-item-refuses-whole-batch#task#4.

Cause 4 of 4: after a duplicate batch is wrongly admitted, the meeting lists the wrongly created attendee and the already-present one is displaced.
AWS: because the duplicate batch is refused, the only attendee in the meeting is the one created before it, so ListAttendees returns exactly that one element with its own AttendeeId and ExternalUserId.
Emulator: ListAttendees returns two elements — the attendee the refused batch created appears first, and the pre-existing attendee is pushed to the second position — so both the element count and the first element's AttendeeId/ExternalUserId diverge.
Why: a client reading the attendee list sees a member AWS never created and cannot assume the list reflects only successful creations; positional reads of the list return the wrong attendee.
Fix: BatchCreateAttendee's state mutation, same all-or-nothing requirement as cause 2.
Traces: batch-create-attendee-repeated-entry-refused-present-user-kept#task#4.

Related open issue, which does NOT cover these: "The per-item attendee admission rule is implemented twice, in BatchCreateAttendee and CreateMeetingWithAttendees" (20261127T071342Z-chime-sdk-meetings-batchcreateattendee-maintainability.md) is a duplication finding and carries no replay key for these steps.

## Evidence
Replayed divergences:
- batch-create-attendee-refused-entry-refuses-whole-batch#task#2 (BatchCreateAttendee): "status: expected 400, got 200; $.Code: missing (expected 'BadRequest'); $.Message: missing (expected \"Validation error for ExternalUserId, prefix 'aws:' is reserved.\"); $.RequestId: missing; $headers.x-amzn-errortype: missing". Cluster also covers chime-sdk-meetings-batch-create-attendee-invalid-item-refuses-whole-batch#task#2 and #task#3.
- batch-create-attendee-refused-entry-refuses-whole-batch#task#3 (ListAttendees, 200/200): "$.Attendees[0]: extra (expected None, got {'ExternalUserId': ..., 'AttendeeId': ..., 'JoinToken': ..., 'Capabilities': {'Audio': 'SendReceive', 'Video': 'SendReceive', 'Content': 'SendReceive'}})".
- batch-create-attendee-repeated-entry-refused-present-user-kept#task#3 (BatchCreateAttendee): "status: expected 400, got 200; $.message: missing (expected 'Provided list of attendee items contains duplicates'); $headers.x-amzn-errortype: missing".
- batch-create-attendee-repeated-entry-refused-present-user-kept#task#4 (ListAttendees, 200/200): "$.Attendees[0].AttendeeId: value_mismatch; $.Attendees[0].ExternalUserId: value_mismatch; $.Attendees[1]: extra (expected None, got the pre-existing attendee)".
Smithy: CreateAttendeeRequestItem.ExternalUserId doc — "Values that begin with aws: are reserved. You can't configure a value that uses this prefix. Case insensitive."; BatchCreateAttendee errors include BadRequestException (400).

## Steps to Reproduce
1. Boot the emulator from this checkout and CreateMeeting to get a MeetingId.
2. Call BatchCreateAttendee on that meeting with one entry whose ExternalUserId starts with 'aws:'. Expected HTTP 400 Code 'BadRequest' with the reserved-prefix message; actual HTTP 200 with a created attendee.
3. Call ListAttendees on the meeting: the attendee from the refused batch is present, where AWS lists none.
4. In a fresh meeting, create one attendee, then call BatchCreateAttendee with a list containing a repeated attendee item. Expected HTTP 400 with message 'Provided list of attendee items contains duplicates'; actual HTTP 200.
5. Call ListAttendees again: two attendees are returned with the newly created one first, where AWS returns only the pre-existing one.
