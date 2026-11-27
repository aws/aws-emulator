Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-selling-engagement-from-opportunity-request-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-selling-engagement-from-opportunity-request-validation partnercentral-selling-engagement-from-opportunity-task-validation-errors partnercentral-selling-engagement-task-start-validation partnercentral-selling-engagement-invitation-request-validation partnercentral-selling-opportunity-from-engagement-task-validation-errors partnercentral-selling-start-opportunity-from-engagement-task-malformed-input-rejected partnercentral-selling-opportunity-association-validation partnercentral-selling-prospecting-task-request-validation partnercentral-selling-resource-snapshot-job-request-validation partnercentral-selling-stop-resource-snapshot-job-malformed-input-rejected partnercentral-selling-resource-snapshot-job-start-refusals partnercentral-selling-start-resource-snapshot-job-malformed-input-rejected
Issue-Shape-Id: com.amazonaws.partnercentralselling#AWSPartnerCentralSelling
Issue-Key: partnercentral-selling|StartEngagementFromOpportunityTask|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StartEngagementFromOpportunityTask|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StartEngagementFromOpportunityTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|RejectEngagementInvitation|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|RejectEngagementInvitation|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|RejectEngagementInvitation|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|StartOpportunityFromEngagementTask|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StartOpportunityFromEngagementTask|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StartOpportunityFromEngagementTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|AssociateOpportunity|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|AssociateOpportunity|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|AssociateOpportunity|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|GetAwsOpportunitySummary|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|GetAwsOpportunitySummary|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|GetAwsOpportunitySummary|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|GetProspectingFromEngagementTask|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|GetProspectingFromEngagementTask|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|GetProspectingFromEngagementTask|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|StopResourceSnapshotJob|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StopResourceSnapshotJob|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StopResourceSnapshotJob|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|GetResourceSnapshotJob|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|GetResourceSnapshotJob|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|GetResourceSnapshotJob|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|DeleteResourceSnapshotJob|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|DeleteResourceSnapshotJob|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|DeleteResourceSnapshotJob|4xx->4xx|$.Reason
Issue-Key: partnercentral-selling|StartResourceSnapshotJob|4xx->4xx|$.ErrorList
Issue-Key: partnercentral-selling|StartResourceSnapshotJob|4xx->4xx|$.Message
Issue-Key: partnercentral-selling|StartResourceSnapshotJob|4xx->4xx|$.Reason
Issue-Commit: 7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f

# partnercentral-selling: pattern-constraint refusals omit ValidationException Reason, ErrorList and Message

> _Note: this issue was identified against the code base at commit `7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartEngagementFromOpportunityTask

## Description
TEN causes, all @pattern (INVALID_STRING_FORMAT) refusals, sharing one root fault: a modeled-constraint refusal in this service is rendered from the FRAMEWORK's synthetic `smithy.framework#ValidationException` instead of the service's own `com.amazonaws.partnercentralselling#ValidationException`. The synthetic shape declares exactly one member, lowercase `message` (services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/src/scaffolded.rs lines 117-178), so the HTTP status (400) and the wire error code (`ValidationException`) are correct while the body carries NONE of the three members a client reads.

AWS (envelope, common to all ten causes) — Smithy: `ValidationException` has `Message` (String, @required), `Reason` (ValidationExceptionReason, @required; values REQUEST_VALIDATION_FAILED | BUSINESS_VALIDATION_FAILED) and `ErrorList` (ValidationExceptionErrorList of ValidationExceptionError{`Code` @required of ValidationExceptionErrorCode, `FieldName`, `Message` @required}); errorKind client, httpError 400. For a request that violates a modeled input constraint AWS answers 400 with `Reason` = "REQUEST_VALIDATION_FAILED", one `ErrorList` entry per violated member, and a top-level `Message` that is the entries joined as "<Code> <FieldName>:<entry Message>". For a @pattern violation the entry's `Code` is "INVALID_STRING_FORMAT" and its `Message` template is the fixed text `Value '<value>' at '<fieldName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — fixed text: "Value ", " at ", " failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the rejected value from the request, the member name LOWER-CAMEL-CASED (the model member `Identifier` is reported as `identifier`), and the member's @pattern with the `^` and `$` anchors STRIPPED. The recorded values below are examples that fill that template, never part of it.

Emulator (common to all ten causes) — the replay shows status 400 as expected, but `$.Reason`, `$.ErrorList` and `$.Message` are all absent from the body ("got None"). Only the synthetic lowercase `message` member is emitted, so an SDK deserializing the modeled shape sees its two @required members missing and no field-level detail at all.

Why (common) — a client cannot distinguish REQUEST_VALIDATION_FAILED from BUSINESS_VALIDATION_FAILED, cannot learn WHICH field was rejected or with which code, and reading `ValidationException.Message`/`.Reason` (both @required, so non-optional in typed SDKs) yields null. Form/retry logic keyed on `ErrorList[].FieldName` and `ErrorList[].Code` breaks entirely, and the lowercase `message` is not the member the model declares, so even the summary text is unreachable.

Fix (common) — the service's boundary constraint-validation refusal path, not any one operation handler: these operations are refused BEFORE their handler runs (several, e.g. StopResourceSnapshotJob, are still scaffolded stubs yet answer 400 here), so the defect lives in the shared request-walk validation stage and in this crate's error channel, which must stop reaching for the synthetic `smithy.framework#ValidationException` and instead construct and serialize the modeled `com.amazonaws.partnercentralselling#ValidationException` (Message + Reason + ErrorList, PascalCase member names), accumulating one ValidationExceptionError per violated member with the code/field/message mapping above.

Cause 1 of 10: StartEngagementFromOpportunityTask, input member `Identifier` (target OpportunityIdentifier, @pattern ^O[0-9]{1,19}$). AWS: 400, Reason REQUEST_VALIDATION_FAILED, ErrorList [{Code INVALID_STRING_FORMAT, FieldName identifier, Message "Value '<value>' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}], Message "INVALID_STRING_FORMAT identifier:<that message>". Emulator: 400 with Reason/ErrorList/Message all missing. Traces: partnercentral-selling-engagement-from-opportunity-request-validation#task#1 (cluster of 13 steps, also spanning partnercentral-selling-engagement-from-opportunity-task-validation-errors and partnercentral-selling-engagement-task-start-validation#task#4).

Cause 2 of 10: RejectEngagementInvitation, input member `RejectionReason` (target RejectionReasonString, @pattern ^[\u0020-\u007E\u00A0-\uD7FF\uE000-\uFFFD]{1,80}$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName rejectionReason, Message "Value '<value>' at 'rejectionReason' failed to satisfy constraint: Member must satisfy regular expression pattern: [\u0020-\u007E\u00A0-\uD7FF\uE000-\uFFFD]{1,80}"}], Reason REQUEST_VALIDATION_FAILED, Message the joined summary. Emulator: 400 with all three missing. Traces: partnercentral-selling-engagement-invitation-request-validation#task#5 (cluster of 2).

Cause 3 of 10: StartOpportunityFromEngagementTask, input member `ContextIdentifier` (target ContextIdentifier, @pattern ^[1-9][0-9]*$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName contextIdentifier, Message "Value '<value>' at 'contextIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [1-9][0-9]*"}]. Emulator: 400 with all three missing. Traces: partnercentral-selling-engagement-task-start-validation#task#1 (cluster of 9, also spanning partnercentral-selling-opportunity-from-engagement-task-validation-errors and partnercentral-selling-start-opportunity-from-engagement-task-malformed-input-rejected).

Cause 4 of 10: AssociateOpportunity, input member `OpportunityIdentifier` (@pattern ^O[0-9]{1,19}$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName opportunityIdentifier, Message "Value '<value>' at 'opportunityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}]. Emulator: 400 with all three missing. Traces: partnercentral-selling-opportunity-association-validation#task#1.

Cause 5 of 10: GetAwsOpportunitySummary, input member `RelatedOpportunityIdentifier` (@pattern ^O[0-9]{1,19}$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName relatedOpportunityIdentifier, Message "Value '<value>' at 'relatedOpportunityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}]. Emulator: 400 with all three missing. Traces: partnercentral-selling-opportunity-association-validation#task#2.

Cause 6 of 10: GetProspectingFromEngagementTask, input member `TaskIdentifier` (@pattern ^task-[0-9a-z]{14}$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName taskIdentifier, Message "Value '<value>' at 'taskIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: task-[0-9a-z]{14}"}]. Emulator: 400 with all three missing. Traces: partnercentral-selling-prospecting-task-request-validation#task#8.

Cause 7 of 10: StopResourceSnapshotJob, input member `ResourceSnapshotJobIdentifier` (@pattern ^job-[0-9a-z]{13}$). AWS: ErrorList [{Code INVALID_STRING_FORMAT, FieldName resourceSnapshotJobIdentifier, Message "Value '<value>' at 'resourceSnapshotJobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: job-[0-9a-z]{13}"}]. Emulator: 400 with all three missing. Traces: partnercentral-selling-resource-snapshot-job-request-validation#task#1 (cluster of 5, also spanning partnercentral-selling-stop-resource-snapshot-job-malformed-input-rejected).

Cause 8 of 10: GetResourceSnapshotJob, same member and same expected ErrorList entry as cause 7 (FieldName resourceSnapshotJobIdentifier, Code INVALID_STRING_FORMAT, pattern job-[0-9a-z]{13}); it is a separate refusal path because it is a separate operation. Emulator: 400 with Reason/ErrorList/Message missing. Traces: partnercentral-selling-resource-snapshot-job-request-validation#task#3.

Cause 9 of 10: DeleteResourceSnapshotJob, same member and expected ErrorList entry as cause 7. Emulator: 400 with Reason/ErrorList/Message missing. Traces: partnercentral-selling-resource-snapshot-job-request-validation#task#4.

Cause 10 of 10: StartResourceSnapshotJob, same member and expected ErrorList entry as cause 7 (including the empty-suffix case `job-`). Emulator: 400 with Reason/ErrorList/Message missing. Traces: partnercentral-selling-resource-snapshot-job-request-validation#task#5 (cluster of 7, also spanning partnercentral-selling-resource-snapshot-job-start-refusals and partnercentral-selling-start-resource-snapshot-job-malformed-input-rejected).

No open issue covers this service; the nearest filed neighbours (e.g. the `events` and `logs` validation-fidelity issues) are other services and do not own these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1: 15 clusters / 76 steps against partnercentral-selling, 0 passed. All ten causes below report expected_status 400 and actual_status 400 with the body divergence `$.ErrorList: missing`, `$.Message: missing`, `$.Reason: missing`.

Cause 1 — StartEngagementFromOpportunityTask, partnercentral-selling-engagement-from-opportunity-request-validation#task#1 (affected_count 13):
  expected $.Reason = 'REQUEST_VALIDATION_FAILED'; got None
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'identifier', 'Message': "Value 'opp-9081726354' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}]; got None
  expected $.Message = "INVALID_STRING_FORMAT identifier:Value 'opp-9081726354' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"; got None

Cause 2 — RejectEngagementInvitation, partnercentral-selling-engagement-invitation-request-validation#task#5 (affected_count 2):
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'rejectionReason', 'Message': "Value 'rrrr…' (81 chars) at 'rejectionReason' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\u0020-\\u007E\\u00A0-\\uD7FF\\uE000-\\uFFFD]{1,80}"}]; got None. $.Reason and $.Message likewise None.

Cause 3 — StartOpportunityFromEngagementTask, partnercentral-selling-engagement-task-start-validation#task#1 (affected_count 9):
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'contextIdentifier', 'Message': "Value '0' at 'contextIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [1-9][0-9]*"}]; got None. $.Reason/$.Message None.

Cause 4 — AssociateOpportunity, partnercentral-selling-opportunity-association-validation#task#1:
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'opportunityIdentifier', 'Message': "Value 'O12345678901234567890' at 'opportunityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}]; got None. $.Reason/$.Message None.

Cause 5 — GetAwsOpportunitySummary, partnercentral-selling-opportunity-association-validation#task#2:
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'relatedOpportunityIdentifier', 'Message': "Value 'o1234567' at 'relatedOpportunityIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: O[0-9]{1,19}"}]; got None. $.Reason/$.Message None.

Cause 6 — GetProspectingFromEngagementTask, partnercentral-selling-prospecting-task-request-validation#task#8:
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'taskIdentifier', 'Message': "Value 'task-7h2k9d4m1q8w3' at 'taskIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: task-[0-9a-z]{14}"}]; got None. $.Reason/$.Message None.

Cause 7 — StopResourceSnapshotJob, partnercentral-selling-resource-snapshot-job-request-validation#task#1 (affected_count 5):
  expected $.ErrorList = [{'Code': 'INVALID_STRING_FORMAT', 'FieldName': 'resourceSnapshotJobIdentifier', 'Message': "Value 'job-4N6B8V2C5X7Z9' at 'resourceSnapshotJobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: job-[0-9a-z]{13}"}]; got None. $.Reason/$.Message None.

Cause 8 — GetResourceSnapshotJob, partnercentral-selling-resource-snapshot-job-request-validation#task#3: same three keys missing; expected ErrorList entry FieldName resourceSnapshotJobIdentifier, Code INVALID_STRING_FORMAT (recorded value 'job-4n6b8v2c5x7z9q').

Cause 9 — DeleteResourceSnapshotJob, partnercentral-selling-resource-snapshot-job-request-validation#task#4: same three keys missing (recorded value 'rsj-4n6b8v2c5x7z9').

Cause 10 — StartResourceSnapshotJob, partnercentral-selling-resource-snapshot-job-request-validation#task#5 (affected_count 7): same three keys missing (recorded value 'job-').

Source evidence: services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/src/scaffolded.rs:117-178 defines the synthetic `smithy.framework#ValidationException` with a single lowercase `message` member and no Reason/ErrorList, and that schema is what the 400 responses above are serialized from. The modeled shape is `com.amazonaws.partnercentralselling#ValidationException` (Message @required, Reason @required, ErrorList) in services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/smithy-model/com-amazonaws-partnercentralselling-awspartnercentralselling.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Representative step: fixture partnercentral-selling-engagement-from-opportunity-request-validation, step task#1 — POST /StartEngagementFromOpportunityTask with Catalog="AWS", a valid ClientToken, AwsSubmission, and Identifier set to a value that does not match ^O[0-9]{1,19}$ (the recording uses 'opp-9081726354').
4. Observe HTTP 400 with error code ValidationException but a body carrying no `Message`, no `Reason` and no `ErrorList` — only a lowercase `message`.
5. Repeat for each bundled cause by replaying the fixture#step listed in trace_ids: RejectEngagementInvitation with an 81-character RejectionReason; StartOpportunityFromEngagementTask with ContextIdentifier='0'; AssociateOpportunity with a 20-digit OpportunityIdentifier; GetAwsOpportunitySummary with a lowercase-'o' RelatedOpportunityIdentifier; GetProspectingFromEngagementTask with a 13-char task suffix; and StopResourceSnapshotJob / GetResourceSnapshotJob / DeleteResourceSnapshotJob / StartResourceSnapshotJob each with a ResourceSnapshotJobIdentifier that breaks ^job-[0-9a-z]{13}$.
6. Each of the ten returns 400 with Reason, ErrorList and Message absent.
