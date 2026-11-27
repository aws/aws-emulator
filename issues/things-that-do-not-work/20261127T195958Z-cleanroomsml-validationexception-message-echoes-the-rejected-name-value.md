Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-audience-job-request-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-audience-job-request-errors cleanroomsml-audience-job-start-refusals
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|StartAudienceExportJob|4xx->4xx|$.message
Issue-Key: cleanroomsml|StartAudienceGenerationJob|4xx->4xx|$.message
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# cleanroomsml ValidationException message echoes the rejected name value instead of AWS's value-less constraint text

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAudienceExportJob

## Description
The emulator's Smithy constraint-violation message for the `name` member of the cleanrooms-ml audience job Start* operations is built from the wrong template: it interpolates the offending value into the message, whereas AWS emits the value-less form for this member. Status (400) and error type (ValidationException) are correct; only the message body diverges, and the message is a requirement.

This bundle has 2 causes, both the same root template defect, and BOTH must be fixed before closing.

Cause 1 of 2: StartAudienceExportJob, `name` longer than 63 characters.
  AWS: Smithy model `com.amazonaws.cleanroomsml` — StartAudienceExportJob.name targets NameString with @required and @length(min: 1, max: 63); the modeled failure is ValidationException (httpError 400, member `message` @required). The exact message text is recorded behavior only: AWS answers HTTP 400 with x-amzn-errortype ValidationException and message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 63". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have length less than or equal to ". Variable parts: the member path (`name`, from the request shape) and the limit (63, from the member's @length max). NOTE: AWS does NOT include the submitted value for this member — there is no "Value '<x>' at" segment.
  Emulator: HTTP 400 / ValidationException with message "1 validation error detected: Value '<the submitted name, e.g. a 64-char string>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 63" — i.e. the generic "Value '<value>' at '<path>'" template, so the rejected name is echoed back.
  Why: a client (or a test) that asserts on the AWS message, or that logs/compares the error text, sees an extra value segment that AWS never sends. Echoing the submitted member value back also leaks request content into the error string for a member AWS deliberately keeps out of it.
  Fix: the input-validation / constraint-message formatting for StartAudienceExportJob (the operation's validation layer, where the length-constraint message template is chosen). The `name` member must use the value-less "Value at '<path>'" template rather than the value-interpolating one.
  Traces: cleanroomsml-audience-job-request-errors#task#1 (also task#2, task#3; cleanroomsml-audience-job-start-refusals#task#2, task#3).

Cause 2 of 2: StartAudienceGenerationJob, `name` longer than 63 characters.
  AWS: Smithy model — StartAudienceGenerationJob.name targets NameString with @required and @length(min: 1, max: 63); error ValidationException (httpError 400). Recorded behavior only for the text: HTTP 400, x-amzn-errortype ValidationException, message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 63" — same value-less template, same fixed/variable split as Cause 1.
  Emulator: HTTP 400 / ValidationException with "1 validation error detected: Value '<the submitted name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 63".
  Why: same as Cause 1 — message text diverges from AWS for every over-long name on this operation.
  Fix: same validation layer, for StartAudienceGenerationJob's `name` member.
  Traces: cleanroomsml-audience-job-request-errors#task#9 (also cleanroomsml-audience-job-start-refusals#task#5, task#6).

No existing open cleanroomsml issue covers this; the open cleanroomsml issues are unimplemented-stub issues for CancelTrainedModelInferenceJob, GetMLConfiguration, ListTrainedModelInferenceJobs, PutMLConfiguration and StartTrainedModelExportJob.

## Evidence
Cause 1 — cleanroomsml-audience-job-request-errors#task#1, StartAudienceExportJob:
  expected: 400, $.message = "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 63"
  actual:   400, $.message = "1 validation error detected: Value 'xxxxxxxx...(64 chars)' at 'name' failed to satisfy constraint: Member must have length less than or equal to 63"
  cluster of 5 steps (divergence_key service=cleanroomsml|StartAudienceExportJob|status=400->400|keys=$.message)

Cause 2 — cleanroomsml-audience-job-request-errors#task#9, StartAudienceGenerationJob:
  expected: 400, $.message = "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 63"
  actual:   400, $.message = "1 validation error detected: Value 'yyyyyyyy...(64 chars)' at 'name' failed to satisfy constraint: Member must have length less than or equal to 63"
  cluster of 3 steps (divergence_key service=cleanroomsml|StartAudienceGenerationJob|status=400->400|keys=$.message)

Status and x-amzn-errortype match in both clusters; the sole difference is the inserted "Value '<submitted value>' at" segment. The 64-character strings above are per-recording filler, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /audience-export-job (StartAudienceExportJob) with a valid audienceGenerationJobArn and audienceSize, and `name` set to a 64-character string. Observe HTTP 400 ValidationException whose message contains "Value '<name>' at 'name'"; AWS returns "Value at 'name'" with no value.
3. Cause 2: POST /audience-generation-job (StartAudienceGenerationJob) with a valid configuredAudienceModelArn and seedAudience, and `name` set to a 64-character string. Same divergence.
4. Compare $.message against the recorded response for cleanroomsml-audience-job-request-errors steps task#1 and task#9.
