Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.get-transcript-unissued-next-token-rejected connectparticipant.get-transcript-malformed-paging-inputs-rejected connectparticipant.get-attachment-uncompleted-upload-refused-then-serves-bytes connectparticipant.send-event-read-receipt-malformed-content-rejected connectparticipant.disconnect-participant-overlong-client-token-rejected
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant.get-transcript-unissued-next-token-rejected|run#13|AssertionError|assert got == _EXPECTED, f"an unissued NextToken drew {got!r}, expected {_EXPECTED!r}"
Issue-Key: connectparticipant.get-transcript-malformed-paging-inputs-rejected|run#18|AssertionError|assert forged == ("ValidationException", _INVALID_PARAMS), (
Issue-Key: connectparticipant.get-attachment-uncompleted-upload-refused-then-serves-bytes|run#10|AssertionError|assert early == ("ValidationException", _INVALID_PARAMS), (
Issue-Key: connectparticipant.send-event-read-receipt-malformed-content-rejected|run#17|AssertionError|assert got == want, f"a receipt keyed messageID answered {got!r}, expected {want!r}"
Issue-Key: connectparticipant.disconnect-participant-overlong-client-token-rejected|run#4|AssertionError|assert got == ("InvalidRequestException", "Invalid request body"), (
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# connectparticipant: 4 net-new causes — refusal messages are emulator-authored instead of AWS's fixed sentences, and an overlong DisconnectParticipant ClientToken draws the wrong error class

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTranscript

## Description
Four independent refusal-fidelity root causes on connectparticipant, from behavioral suite simparity-behavioral-aws-v1. In the first three the HTTP status (400) and the error code (ValidationException) are right and ONLY the message text diverges: the handler writes its own English sentence, which additionally leaks emulator internals (stored artifact status, the emulator's own token scheme, member names). In the fourth both the error code and the message diverge. A fixer must close ALL FOUR before submitting. Smithy model consulted: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/smithy-model/com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda.json (sdkId ConnectParticipant, restJson1; ValidationException is a service-common error on every operation, errorKind client, httpError 400, member Message required).

Cause 1 of 4: GetTranscript refuses a NextToken it never issued with its own sentence instead of AWS's fixed one. Failing checks: connectparticipant.get-transcript-unissued-next-token-rejected, connectparticipant.get-transcript-malformed-paging-inputs-rejected.
AWS: GetTranscriptRequest$NextToken is modeled NextToken @length 1..1000, and the operation is @paginated (inputToken/outputToken NextToken). A value that is well-formed for the member but was never handed out by GetTranscript is refused HTTP 400 ValidationException with the fixed sentence "Specified request parameter(s) are invalid." — the entire message is fixed text for this class; no part of it is taken from the request or from state, and it never names the member or the token.
Emulator: status 400 and code ValidationException are correct, but the message is "NextToken is not a transcript pagination token this service issued" — wording AWS never emits, and it advertises that pagination tokens are self-describing artifacts of this service.
Why: a client that branches on, logs or surfaces the refusal text for a stale or forged pagination token never matches AWS, and a resumed paging loop reports a reason AWS does not give. The leaked wording also tells a caller more about the emulator's token scheme than AWS discloses.
Fix: GetTranscript's input validation in its handler — the ValidationException it constructs for an undecodable/foreign NextToken must carry AWS's fixed sentence.
Traces: connectparticipant.get-transcript-unissued-next-token-rejected (NextToken="c2ltMnJlYWwtdW5pc3N1ZWQtcGFnZS1tYXJrZXI="), connectparticipant.get-transcript-malformed-paging-inputs-rejected (NextToken="forged-page-token"; its later assertion — a StartPosition naming both Id and MostRecent must be refused with the same fixed sentence — is ungraded because the check aborts here, so re-replay it after the fix). Code read for grounding: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/get_transcript.rs lines 214-225 (the two emulator-authored NextToken refusals) and lines 202-208 (the same pattern for ContactId).

Cause 2 of 4: GetAttachment refuses a not-yet-completed attachment with a sentence that names the attachment id and its stored status. Failing check: connectparticipant.get-attachment-uncompleted-upload-refused-then-serves-bytes.
AWS: GetAttachment "Provides a pre-signed URL for download of a completed attachment"; asking for one whose upload has not been confirmed by CompleteAttachmentUpload is refused HTTP 400 ValidationException with the fixed sentence "Specified request parameter(s) are invalid." — wholly fixed text, naming neither the attachment nor any internal status value. After CompleteAttachmentUpload the same call must serve the bytes.
Emulator: status 400 / ValidationException are right, but the message is "Attachment with id <attachment-id> is not available for download; its status is IN_PROGRESS" — emulator-authored, interpolating the request's AttachmentId and the crate's internal ARTIFACT_STATUS_* value.
Why: the refusal text never matches AWS for a client that classifies on it, and it discloses an internal artifact-status vocabulary that is not part of the API surface.
Fix: GetAttachment's handler validation — the ValidationException for a non-approved attachment must carry AWS's fixed sentence (and so must the sibling refusal for an attachment that does not exist for the contact).
Traces: connectparticipant.get-attachment-uncompleted-upload-refused-then-serves-bytes. Code: src/ops/get_attachment.rs lines 199-220.

Cause 3 of 4: SendEvent refuses a message-receipt whose Content does not name a message with its own multi-clause sentence. Failing check: connectparticipant.send-event-read-receipt-malformed-content-rejected.
AWS: SendEventRequest$Content documents that receipt content is "supported in the form of a JSON string. Sample Content: "{\"messageId\":\"11111111-aaaa-bbbb-cccc-EXAMPLE01234\"}"". A receipt whose Content keys the id under anything else (e.g. "messageID") is refused HTTP 400 ValidationException with the fixed sentence "Specified request parameter(s) are invalid." — whole message fixed; the rejected content is not echoed. The refused receipt must leave the named message unmarked.
Emulator: status 400 / ValidationException are right, but the message is "The input fails to satisfy the constraints specified by Amazon Connect: Content must name the acknowledged message as a JSON string of the form {\"messageId\":\"<chat item id>\"} for a message-receipt ContentType" — the ValidationException shape's own doc sentence concatenated with an emulator-written explanation. The sibling refusal in the same branch ("Content must name a messageId of this chat's transcript") has the same defect.
Why: a client that matches the refusal text for a malformed receipt never matches AWS; the emulator's sentence also mixes the exception's documentation string into the per-request message, which no AWS service does.
Fix: SendEvent's receipt validation in its handler — both ValidationExceptions must carry AWS's fixed sentence.
Traces: connectparticipant.send-event-read-receipt-malformed-content-rejected. Code: src/ops/send_event.rs lines 211-224.

Cause 4 of 4: DisconnectParticipant answers the WRONG error class and the framework's generic constraint text for an overlong ClientToken. Failing check: connectparticipant.disconnect-participant-overlong-client-token-rejected.
AWS: recorded behavior only — DisconnectParticipantRequest$ClientToken is modeled ClientToken @length 0..500 @idempotencyToken, and the model declares only AccessDeniedException/InternalServerException/ThrottlingException/ValidationException for the operation, so InvalidRequestException is not in the model. The recording shows a 501-character ClientToken refused with error code InvalidRequestException and the message "Invalid request body" (HTTP 400): the request is rejected at the body-parsing frontend, ahead of member-level validation, and the message is entirely fixed text — it names no member, no value and no bound. The refused disconnect must leave the participant connected (SendMessage and GetTranscript still work afterwards).
Emulator: code ValidationException with the boundary validator's generic rendering "1 validation error detected: Value '<the 501 characters>... [501 bytes]' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 500" — a different error class, and a message that echoes the rejected value, the emulator's lowercased member name 'clientToken' and the raw bound.
Why: a client branching on the error code takes the wrong branch entirely (it sees a modeled ValidationException where AWS reports InvalidRequestException), and the echoed value plus internal member spelling leak request content and emulator internals into an error body AWS keeps fixed.
Fix: the request-body rejection path for this operation — the over-length ClientToken must be refused as a malformed request body (code InvalidRequestException, message "Invalid request body") rather than falling through to the generic per-member length validator in the centralized request walk; see src/ops/disconnect_participant.rs and the shared validation layer it runs under.
Traces: connectparticipant.disconnect-participant-overlong-client-token-rejected.

Related open issue, which does NOT cover any of these four: 20261109T180950Z-connectparticipant-error-paths-miss-accessdenied-and-aws-message.md carries wire replay keys for CreateParticipantConnection and SendMessage AccessDenied/empty-Type message text only; none of the five checks above is bound to it.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service connectparticipant: 18/34 passed (53%), 16 failed.

Cause 1 — [mismatch] connectparticipant.get-transcript-unissued-next-token-rejected:
  AssertionError: an unissued NextToken drew ('ValidationException', 'NextToken is not a transcript pagination token this service issued'), expected ('ValidationException', 'Specified request parameter(s) are invalid.')
  [mismatch] connectparticipant.get-transcript-malformed-paging-inputs-rejected:
  AssertionError: a NextToken GetTranscript never issued should be refused, got ('ValidationException', 'NextToken is not a transcript pagination token this service issued')
  Status 400 and code ValidationException match in both; only the message diverges. Expected message is wholly fixed text.

Cause 2 — [mismatch] connectparticipant.get-attachment-uncompleted-upload-refused-then-serves-bytes:
  AssertionError: GetAttachment before CompleteAttachmentUpload should be refused, got ('ValidationException', 'Attachment with id <attachment-id> is not available for download; its status is IN_PROGRESS')
  Expected ('ValidationException', 'Specified request parameter(s) are invalid.'). The attachment id and the IN_PROGRESS status in the actual text are interpolated request/state values; AWS's message carries neither.

Cause 3 — [mismatch] connectparticipant.send-event-read-receipt-malformed-content-rejected:
  AssertionError: a receipt keyed messageID answered ('ValidationException', 'The input fails to satisfy the constraints specified by Amazon Connect: Content must name the acknowledged message as a JSON string of the form {"messageId":"<chat item id>"} for a message-receipt ContentType'), expected ('ValidationException', 'Specified request parameter(s) are invalid.')

Cause 4 — [mismatch] connectparticipant.disconnect-participant-overlong-client-token-rejected:
  AssertionError: a 501-character ClientToken answered ('ValidationException', "1 validation error detected: Value 'kkkk...[501 bytes]' at 'clientToken' failed to satisfy constraint: Member must have length less than or equal to 500"), expected InvalidRequestException 'Invalid request body'
  Both the code and the message diverge. The expected pair is ('InvalidRequestException', 'Invalid request body'), entirely fixed text.

The statuses, error codes and message texts quoted as expected are the requirement; the attachment ids and the 'k'*501 filler in the actual texts are per-run values and are not.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a connectparticipant client at its data listener; open a chat (Connect instance + flow + StartChatContact + CreateParticipantConnection(Type=["CONNECTION_CREDENTIALS"])) to get a ConnectionToken.
2. Cause 1: send three messages, then GetTranscript(ConnectionToken=<token>, SortOrder="ASCENDING", MaxResults=1, NextToken="forged-page-token"). The 400 body's message is 'NextToken is not a transcript pagination token this service issued' instead of 'Specified request parameter(s) are invalid.'. Also call GetTranscript(StartPosition={"Id": <an item id>, "MostRecent": 1}) and check that refusal carries the same fixed sentence.
3. Cause 2: associate ATTACHMENTS storage, StartAttachmentUpload for a .txt file, PUT its bytes, and call GetAttachment(AttachmentId=<id>) BEFORE CompleteAttachmentUpload — the message is 'Attachment with id <id> is not available for download; its status is IN_PROGRESS' instead of 'Specified request parameter(s) are invalid.'. Complete the upload and verify GetAttachment then serves the bytes.
4. Cause 3: SendMessage to get an item Id, then SendEvent(ContentType="application/vnd.amazonaws.connect.event.message.read", Content=json.dumps({"messageID": <that id>})) (note the capital D) — the message is the emulator's multi-clause sentence instead of 'Specified request parameter(s) are invalid.'; verify the message is left unmarked by the refused receipt.
5. Cause 4: with client-side parameter validation disabled, DisconnectParticipant(ConnectionToken=<token>, ClientToken="k"*501) and read the RAW 400 body — it carries code ValidationException and the generic '1 validation error detected: ... at 'clientToken' ...' text; AWS returns code InvalidRequestException with message 'Invalid request body'. Then confirm the participant is still connected: SendMessage and GetTranscript must still succeed.
6. Re-replay the five checks listed in trace_ids to confirm the fix; all four causes must be closed.
