Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-selling-engagement-by-accepting-invitation-task-validation-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-selling-engagement-by-accepting-invitation-task-validation-errors partnercentral-selling-engagement-invitation-request-validation partnercentral-selling-engagement-task-start-validation partnercentral-selling-prospecting-task-request-validation partnercentral-selling-start-prospecting-from-engagement-task-unknown-catalog-rejected partnercentral-selling-opportunity-association-validation
Issue-Shape-Id: com.amazonaws.partnercentralselling#AWSPartnerCentralSelling
Issue-Key: partnercentral-selling|StartEngagementByAcceptingInvitationTask|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StartEngagementByAcceptingInvitationTask|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StartEngagementByAcceptingInvitationTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|AcceptEngagementInvitation|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|AcceptEngagementInvitation|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|AcceptEngagementInvitation|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|StartProspectingFromEngagementTask|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StartProspectingFromEngagementTask|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StartProspectingFromEngagementTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|ListOpportunities|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|ListOpportunities|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|ListOpportunities|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|ListProspectingFromEngagementTasks|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|ListProspectingFromEngagementTasks|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|ListProspectingFromEngagementTasks|4xx->4xx|$.Reason
Issue-Commit: 7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f

# partnercentral-selling: required-field, length and range refusals omit ValidationException Reason/ErrorList

> _Note: this issue was identified against the code base at commit `7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartEngagementByAcceptingInvitationTask

## Description
FIVE causes covering the non-@pattern constraint classes (@required, @length, @range), sharing the same root fault as the pattern-refusal bundle filed alongside this one: a modeled-constraint refusal is rendered from the FRAMEWORK's synthetic `smithy.framework#ValidationException`, which declares exactly one member, lowercase `message` (services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/src/scaffolded.rs lines 117-178), instead of the service's own `com.amazonaws.partnercentralselling#ValidationException`. The 400 status and the `ValidationException` wire code are right; the body carries none of the modeled members. This bundle is filed separately because each constraint class needs its OWN ValidationExceptionErrorCode and its own message template — fixing the envelope alone still leaves these four codes unmapped.

AWS (envelope, common to all five causes) — Smithy: `ValidationException` has `Message` (String, @required), `Reason` (ValidationExceptionReason, @required; REQUEST_VALIDATION_FAILED | BUSINESS_VALIDATION_FAILED) and `ErrorList` (list of ValidationExceptionError{`Code` @required of ValidationExceptionErrorCode — REQUIRED_FIELD_MISSING, INVALID_ENUM_VALUE, INVALID_STRING_FORMAT, INVALID_VALUE, NOT_ENOUGH_VALUES, TOO_MANY_VALUES, INVALID_RESOURCE_STATE, DUPLICATE_KEY_VALUE, VALUE_OUT_OF_RANGE, ACTION_NOT_PERMITTED —, `FieldName`, `Message` @required}); errorKind client, httpError 400. A modeled-input-constraint violation answers 400 with `Reason` = "REQUEST_VALIDATION_FAILED", one ErrorList entry per violated member, and a top-level `Message` equal to the entries joined as "<Code> <FieldName>:<entry Message>". `FieldName` is the model member name LOWER-CAMEL-CASED (`Identifier` -> `identifier`, `TaskName` -> `taskName`, `MaxResults` -> `maxResults`). The per-class entry templates (fixed text verbatim, variable parts taken from the request and from the member's modelled constraint):
  * @required member absent -> Code "REQUIRED_FIELD_MISSING", Message `Value null at '<fieldName>' failed to satisfy constraint: Member must not be null`.
  * string @length max exceeded -> Code "TOO_MANY_VALUES", Message `Value '<value>' at '<fieldName>' failed to satisfy constraint: Member must have length less than or equal to <max>`.
  * integer @range max exceeded -> Code "INVALID_VALUE", Message `Value '<value>' at '<fieldName>' failed to satisfy constraint: Member must have value less than or equal to <max>` (note the value is quoted even though the member is an integer).
The concrete values in the recordings are examples filling these templates, not requirements.

Emulator (common to all five causes) — the replay shows the expected status 400, but `$.Reason`, `$.ErrorList` and `$.Message` are all absent from the body ("got None"); only the synthetic lowercase `message` is emitted. No ValidationExceptionErrorCode is produced at all, so none of the four code mappings above exists.

Why (common) — `Message` and `Reason` are @required on the modeled shape, so typed SDKs surface null for non-optional fields; a caller cannot tell a missing required field from an over-long one from an out-of-range page size, cannot learn which member was rejected, and cannot branch on REQUIRED_FIELD_MISSING vs TOO_MANY_VALUES vs INVALID_VALUE. Clients that retry with a corrected page size, or that highlight the offending form field, have nothing to key on.

Fix (common) — the service's boundary constraint-validation refusal path and this crate's error channel, not individual handlers: the refusal happens before the handler runs, so the shared request-walk validation stage must build and serialize the modeled `com.amazonaws.partnercentralselling#ValidationException` (PascalCase `Message`, `Reason`, `ErrorList`), accumulating one ValidationExceptionError per violated member and mapping the violated constraint to the ValidationExceptionErrorCode and message template listed above instead of returning `Scaffolded::ValidationException`.

Cause 1 of 5: StartEngagementByAcceptingInvitationTask with its @required input member `Identifier` absent. AWS: 400, Reason REQUEST_VALIDATION_FAILED, ErrorList [{Code REQUIRED_FIELD_MISSING, FieldName identifier, Message "Value null at 'identifier' failed to satisfy constraint: Member must not be null"}], Message "REQUIRED_FIELD_MISSING identifier:<that message>". Emulator: 400 with Reason/ErrorList/Message all missing. Traces: partnercentral-selling-engagement-by-accepting-invitation-task-validation-errors#task#3 (cluster of 3, also spanning #task#4 and partnercentral-selling-engagement-invitation-request-validation#task#2).

Cause 2 of 5: AcceptEngagementInvitation with its @required input member `Identifier` (target EngagementInvitationArnOrIdentifier) absent. AWS: ErrorList [{Code REQUIRED_FIELD_MISSING, FieldName identifier, Message "Value null at 'identifier' failed to satisfy constraint: Member must not be null"}], Reason REQUEST_VALIDATION_FAILED, Message the joined summary. Emulator: 400 with all three missing. Traces: partnercentral-selling-engagement-invitation-request-validation#task#4.

Cause 3 of 5: StartProspectingFromEngagementTask with input member `TaskName` (target TaskName, @length min 1 max 128) longer than 128 characters. AWS: ErrorList [{Code TOO_MANY_VALUES, FieldName taskName, Message "Value '<value>' at 'taskName' failed to satisfy constraint: Member must have length less than or equal to 128"}], Reason REQUEST_VALIDATION_FAILED. Note the code for a string length-max overflow is TOO_MANY_VALUES, not INVALID_STRING_FORMAT. Emulator: 400 with all three missing. Traces: partnercentral-selling-engagement-task-start-validation#task#3 (cluster of 10, also spanning partnercentral-selling-prospecting-task-request-validation and partnercentral-selling-start-prospecting-from-engagement-task-unknown-catalog-rejected).

Cause 4 of 5: ListOpportunities with input member `MaxResults` (target PageSize, @range min 1 max 100) above the maximum. AWS: ErrorList [{Code INVALID_VALUE, FieldName maxResults, Message "Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"}], Reason REQUEST_VALIDATION_FAILED. Emulator: 400 with all three missing. Traces: partnercentral-selling-opportunity-association-validation#task#3 (cluster of 2, also #task#4).

Cause 5 of 5: ListProspectingFromEngagementTasks with input member `MaxResults` above its @range maximum of 100 — the same expected entry as cause 4 (Code INVALID_VALUE, FieldName maxResults), but a separate operation and therefore a separate refusal path. Emulator: 400 with Reason/ErrorList/Message missing. Traces: partnercentral-selling-prospecting-task-request-validation#task#9.

No open issue covers this service; the nearest filed neighbours (the `events`, `logs` and `textract` validation-message issues) belong to other services and do not own these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1: 15 clusters / 76 steps against partnercentral-selling, 0 passed. All five causes report expected_status 400 and actual_status 400 with `$.ErrorList: missing`, `$.Message: missing`, `$.Reason: missing`.

Cause 1 — StartEngagementByAcceptingInvitationTask, partnercentral-selling-engagement-by-accepting-invitation-task-validation-errors#task#3 (affected_count 3):
  expected $.Reason = 'REQUEST_VALIDATION_FAILED'; got None
  expected $.ErrorList = [{'Code': 'REQUIRED_FIELD_MISSING', 'FieldName': 'identifier', 'Message': "Value null at 'identifier' failed to satisfy constraint: Member must not be null"}]; got None
  expected $.Message = "REQUIRED_FIELD_MISSING identifier:Value null at 'identifier' failed to satisfy constraint: Member must not be null"; got None

Cause 2 — AcceptEngagementInvitation, partnercentral-selling-engagement-invitation-request-validation#task#4:
  expected $.ErrorList = [{'Code': 'REQUIRED_FIELD_MISSING', 'FieldName': 'identifier', 'Message': "Value null at 'identifier' failed to satisfy constraint: Member must not be null"}]; got None. $.Reason = 'REQUEST_VALIDATION_FAILED' and the joined $.Message likewise None.

Cause 3 — StartProspectingFromEngagementTask, partnercentral-selling-engagement-task-start-validation#task#3 (affected_count 10):
  expected $.ErrorList = [{'Code': 'TOO_MANY_VALUES', 'FieldName': 'taskName', 'Message': "Value 'tttt…' (131 chars) at 'taskName' failed to satisfy constraint: Member must have length less than or equal to 128"}]; got None
  expected $.Message = "TOO_MANY_VALUES taskName:<that message>"; got None. $.Reason None.

Cause 4 — ListOpportunities, partnercentral-selling-opportunity-association-validation#task#3 (affected_count 2):
  expected $.ErrorList = [{'Code': 'INVALID_VALUE', 'FieldName': 'maxResults', 'Message': "Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"}]; got None
  expected $.Message = "INVALID_VALUE maxResults:<that message>"; got None. $.Reason None.

Cause 5 — ListProspectingFromEngagementTasks, partnercentral-selling-prospecting-task-request-validation#task#9:
  expected $.ErrorList = [{'Code': 'INVALID_VALUE', 'FieldName': 'maxResults', 'Message': "Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"}]; got None. $.Reason/$.Message None.

Source evidence: services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/src/scaffolded.rs:117-178 declares the synthetic `smithy.framework#ValidationException` with a single lowercase `message` member (no Reason, no ErrorList) and lines 285-291 fix its status at 400 — that is the shape these refusals serialize. The modeled shape, with Message/Reason @required plus ErrorList and the ten-value ValidationExceptionErrorCode enum, is in services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/smithy-model/com-amazonaws-partnercentralselling-awspartnercentralselling.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Representative step: fixture partnercentral-selling-engagement-by-accepting-invitation-task-validation-errors, step task#3 — POST /StartEngagementByAcceptingInvitationTask with Catalog and ClientToken present but the @required `Identifier` member omitted.
4. Observe HTTP 400, error code ValidationException, and a body with no `Message`, no `Reason` and no `ErrorList` — only a lowercase `message`.
5. Repeat for each bundled cause by replaying the fixture#step in trace_ids: AcceptEngagementInvitation with `Identifier` omitted (partnercentral-selling-engagement-invitation-request-validation#task#4); StartProspectingFromEngagementTask with a `TaskName` longer than 128 characters (partnercentral-selling-engagement-task-start-validation#task#3); ListOpportunities with `MaxResults` above 100 (partnercentral-selling-opportunity-association-validation#task#3); ListProspectingFromEngagementTasks with `MaxResults` above 100 (partnercentral-selling-prospecting-task-request-validation#task#9).
6. Each of the five returns 400 with Reason, ErrorList and Message absent, and with no ValidationExceptionErrorCode (REQUIRED_FIELD_MISSING / TOO_MANY_VALUES / INVALID_VALUE) anywhere in the body.
