Issue-Source: trace-sweep-divergence
Issue-Trace: healthlake-agent-conversation-refusals
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: healthlake-agent-conversation-refusals healthlake-agent-input-constraint-errors healthlake-data-transformation-job-start-validation healthlake-fhir-export-import-start-refusals healthlake-fhir-import-job-request-validation
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|UpdateProfileWithAgent|4xx->4xx|$.message
Issue-Key: healthlake|StartDataTransformationJob|4xx->4xx|$.message
Issue-Key: healthlake|StartFHIRExportJob|4xx->4xx|$.Message
Issue-Key: healthlake|StartFHIRImportJob|4xx->4xx|$.Message
Issue-Commit: 65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5

# HealthLake ValidationException message text diverges from AWS wording

> _Note: this issue was identified against the code base at commit `65b4e45f54c45e7e9fcce9255bb6d1e04dbae5a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProfileWithAgent

## Description
Four causes, all in how this service words the ValidationException message it returns for a constraint violation. The status (400) and the error code are correct in every one; only the message text diverges. Causes 1-3 are the same clause class (a scalar @length failure) on three different operations and members, each recorded separately; cause 4 is the enum clause.

Cause 1 of 4 — UpdateProfileWithAgent, member conversationId, @length max. AWS: Smithy: UpdateProfileWithAgent.ConversationId -> ConversationIdString, @length 1..36; error ValidationException (httpError 400, member Message; the data-transformation endpoint carries the same sentence under the lowercase key message). The model does not state the sentence, so recorded behavior only: the recorded 400 body's message is "1 validation error detected: Value at 'conversationId' failed to satisfy constraint: Member must have length less than or equal to 36". Fixed text: "N validation error(s) detected: Value at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>"; the variable parts are the lowerCamel member path from the request and the bound from the model — AWS does NOT quote the rejected value for this clause. Emulator: emits "1 validation error detected: Value '<rejected value>' at 'conversationId' failed to satisfy constraint: Member must have length less than or equal to 36", i.e. it interpolates the caller's value into the clause. Why: the message is a requirement; a client (or a test) asserting on AWS's sentence sees a different string, and the emulator echoes back a caller-supplied value AWS deliberately withholds. Fix: the shared validation-message renderer of this service — the scalar @length branch must word the clause without the value, like the enum, pattern and collection-length branches already do. Traces: healthlake-agent-conversation-refusals#task#4 (also healthlake-agent-input-constraint-errors#task#3, #task#4). Code: services/service-com-amazonaws-healthlake-healthlake/src/service.rs clause_for (~lines 232-255) falls through to Violation::message() for a scalar length, and length_requirement (~lines 292-302).

Cause 2 of 4 — StartDataTransformationJob, member profileId, @length min (same clause class as cause 1, separate recording). AWS: Smithy: StartDataTransformationJob.ProfileId -> ProfileIdString, @length 32..32, @pattern ^[a-f0-9]{32}$, required; error ValidationException 400. Recorded behavior only: the recorded message is "2 validation errors detected: Value at 'profileId' failed to satisfy constraint: Member must have length greater than or equal to 32; Value at 'profileId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{32}" — both clauses omit the value, and the clause order (length then pattern) and the "; " join are fixed text. Emulator: the pattern clause matches, but the length clause reads "Value '<rejected value>' at 'profileId' failed to satisfy constraint: Member must have length greater than or equal to 32". Why: same as cause 1, and it shows the divergence is the @length clause specifically, on a min bound as well as a max. Fix: same renderer as cause 1. Traces: healthlake-data-transformation-job-start-validation#task#3 (also #task#5).

Cause 3 of 4 — StartFHIRExportJob, member jobName, @length max, reported under the modelled key Message (same clause class as causes 1-2, separate recording and a different error key). AWS: Smithy: StartFHIRExportJob.JobName -> JobName, @length 1..64, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$; error ValidationException (httpError 400, member Message). Recorded behavior only: $.Message is "1 validation error detected: Value at 'jobName' failed to satisfy constraint: Member must have length less than or equal to 64". Emulator: "1 validation error detected: Value '<rejected jobName>' at 'jobName' failed to satisfy constraint: Member must have length less than or equal to 64" — the recorded run's value (a 65-character probe name) is a request value, not part of the template. Why: same as cause 1, on the classic HealthLake endpoint and the capitalised Message member, so the defect reaches both error keys this service serves. Fix: same renderer as cause 1. Traces: healthlake-fhir-export-import-start-refusals#task#3.

Cause 4 of 4 — StartFHIRImportJob, member validationLevel, enum clause renders the permitted set in the wrong order. AWS: Smithy: StartFHIRImportJob.ValidationLevel -> ValidationLevel (enum, model declaration order strict | structure-only | minimal); error ValidationException 400. Recorded behavior only: the recorded $.Message is "1 validation error detected: Value at 'validationLevel' failed to satisfy constraint: Member must satisfy enum value set: [minimal, structure-only, strict]" — the set is rendered minimal, structure-only, strict, which is neither alphabetical nor the on-disk model's declaration order. The whole sentence including the bracketed, ", "-joined set is fixed text; only the member path varies. Emulator: renders "...enum value set: [strict, structure-only, minimal]", i.e. the snapshot's declaration order. Why: the message is a requirement, and the enum clause is precisely the clause a caller reads to correct its request; a client or test comparing against AWS's sentence fails on a permutation. Fix: this service's violation reconciliation — the ValidationLevel enum clause must name the value set in the order the service reports it, in the one place the SourceFormat set is already reconciled, leaving the enforced constraint as the model's. Traces: healthlake-fhir-export-import-start-refusals#task#6 (also healthlake-fhir-import-job-request-validation#task#2, #task#3, #task#5). Code: services/service-com-amazonaws-healthlake-healthlake/src/service.rs clause_for EnumValue branch (~lines 234-238) and reported_violations (~lines 326-347).

Unrelated to the one open healthlake issue ("TransformData has no handler: the synchronous transform endpoint is unserved"), which covers a different, unserved surface.

## Evidence
All four causes: status 400 and error code matched; only the message body diverged.

Cause 1 — fixture healthlake-agent-conversation-refusals step task#4 (UpdateProfileWithAgent), key $.message:
  expected "1 validation error detected: Value at 'conversationId' failed to satisfy constraint: Member must have length less than or equal to 36"
  actual   "1 validation error detected: Value 'ccccccccccccccccccccccccccccccccccccc' at 'conversationId' failed to satisfy constraint: Member must have length less than or equal to 36"
  cluster also covers healthlake-agent-input-constraint-errors#task#3 and #task#4.

Cause 2 — fixture healthlake-data-transformation-job-start-validation step task#3 (StartDataTransformationJob), key $.message:
  expected "2 validation errors detected: Value at 'profileId' failed to satisfy constraint: Member must have length greater than or equal to 32; Value at 'profileId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{32}"
  actual   "2 validation errors detected: Value '0123456789abcdef0123456789abcde' at 'profileId' failed to satisfy constraint: Member must have length greater than or equal to 32; Value at 'profileId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{32}"
  cluster also covers #task#5.

Cause 3 — fixture healthlake-fhir-export-import-start-refusals step task#3 (StartFHIRExportJob), key $.Message:
  expected "1 validation error detected: Value at 'jobName' failed to satisfy constraint: Member must have length less than or equal to 64"
  actual   "1 validation error detected: Value 'sim2real-probe-healthlake-36612e8b4f7a475ecd69cbb60ffd599370a09ax' at 'jobName' failed to satisfy constraint: Member must have length less than or equal to 64"

Cause 4 — fixture healthlake-fhir-export-import-start-refusals step task#6 (StartFHIRImportJob), key $.Message:
  expected "1 validation error detected: Value at 'validationLevel' failed to satisfy constraint: Member must satisfy enum value set: [minimal, structure-only, strict]"
  actual   "1 validation error detected: Value at 'validationLevel' failed to satisfy constraint: Member must satisfy enum value set: [strict, structure-only, minimal]"
  cluster also covers healthlake-fhir-import-job-request-validation#task#2, #task#3, #task#5.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the four fixture steps: healthlake-agent-conversation-refusals#task#4, healthlake-data-transformation-job-start-validation#task#3, healthlake-fhir-export-import-start-refusals#task#3 and healthlake-fhir-export-import-start-refusals#task#6.
2. By hand (awsJson1_0, X-Amz-Target per operation):
   a. UpdateProfileWithAgent with a valid ProfileId, an InputMessage, and a ConversationId longer than 36 characters -> compare the 400 body's message against the AWS sentence (no quoted value).
   b. StartDataTransformationJob with a ProfileId of 31 characters (so both @length min and @pattern fail) -> compare both clauses.
   c. StartFHIRExportJob with a JobName longer than 64 characters -> compare the 400 body's Message.
   d. StartFHIRImportJob with ValidationLevel set to a value outside the enum -> compare the bracketed value set and its order.
3. In each case the status and error code match; the message text differs as shown in the evidence.
