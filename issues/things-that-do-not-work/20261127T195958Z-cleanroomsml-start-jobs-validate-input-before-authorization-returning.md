Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-audience-job-start-refusals
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-audience-job-start-refusals cleanroomsml-trained-model-inference-job-authorization-precedes-validation
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|StartAudienceGenerationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cleanroomsml|StartAudienceGenerationJob|4xx->4xx|status
Issue-Key: cleanroomsml|StartTrainedModelInferenceJob|4xx->4xx|$.message
Issue-Key: cleanroomsml|StartTrainedModelInferenceJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cleanroomsml|StartTrainedModelInferenceJob|4xx->4xx|status
Issue-Related: 20261127T195958Z-cleanroomsml-validationexception-message-echoes-the-rejected-name-value.md
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# cleanroomsml Start* jobs validate input before authorization, returning 400 ValidationException where AWS returns 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAudienceGenerationJob

## Description
On the cleanrooms-ml Start* job operations, AWS evaluates resource-based authorization for the referenced resource BEFORE it reports request-shape constraint violations: a request that both references a resource the caller is not authorized for (or that does not exist) and carries a constraint-violating member is refused with 403 AccessDeniedException, not 400 ValidationException. The emulator has the order inverted — its constraint validation runs first and short-circuits, so the authorization refusal is never reached. Consequence: the wrong HTTP status, the wrong error type and a completely different message.

This bundle has 2 causes, both the same ordering defect on different operations, and BOTH must be fixed before closing.

Cause 1 of 2: StartAudienceGenerationJob with a configuredAudienceModelArn the caller is not authorized for.
  AWS: Smithy model `com.amazonaws.cleanroomsml` — StartAudienceGenerationJob models AccessDeniedException (errorKind client, httpError 403, doc "You do not have sufficient access to perform this action.") alongside ValidationException (400); configuredAudienceModelArn is @required with @length(20,2048) and @pattern "^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-audience-model/[-a-zA-Z0-9_/.]+$". That the authorization decision precedes pattern validation is recorded behavior only: the recording shows HTTP 403 with message "Provided ConfiguredAudienceModelArn: <arn> is invalid. Please provide a valid arn. because no resource-based policy allows the cleanrooms-ml:StartAudienceGenerationJob action". Fixed text: "Provided ConfiguredAudienceModelArn: ", " is invalid. Please provide a valid arn. because no resource-based policy allows the cleanrooms-ml:StartAudienceGenerationJob action". Variable part: the ARN, taken verbatim from the request's configuredAudienceModelArn member. The error type header is not asserted by the fixture, but the modeled 403 shape is AccessDeniedException.
  Emulator: HTTP 400 with x-amzn-errortype ValidationException and message "1 validation error detected: Value '<the submitted arn>' at 'configuredAudienceModelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-audience-model/[-a-zA-Z0-9_/.]+" — the pattern check fires and no authorization check is consulted.
  Why: a client distinguishing a permissions/ownership problem from a malformed request gets the opposite answer. SDK retry and error-handling branches keyed on AccessDeniedException (403) versus ValidationException (400) take the wrong branch, and code that surfaces "fix your IAM/resource policy" instead reports "your input is malformed" — a client exercising the deny path against the emulator concludes authorization is satisfied.
  Fix: the StartAudienceGenerationJob handler's ordering — resolve/authorize the referenced configured audience model (and emit AccessDeniedException) before the request's shape/pattern validation short-circuits, i.e. move the authorization step ahead of the per-member constraint validation in the handler.
  Traces: cleanroomsml-audience-job-start-refusals#task#7.

Cause 2 of 2: StartTrainedModelInferenceJob where the trained model's configured model algorithm association is not authorized/resolvable AND resourceConfig.instanceCount exceeds its range.
  AWS: Smithy model — StartTrainedModelInferenceJob models AccessDeniedException (client, httpError 403) and ValidationException (400); InferenceResourceConfig.instanceCount targets Integer with @range(min: 1, max: 10) and @default 1. Recorded behavior only for the ordering and text: HTTP 403 with message "Configured Model Algorithm Association not found for Trained Model <trainedModelArn>. because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action". Fixed text: "Configured Model Algorithm Association not found for Trained Model ", ". because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action". Variable part: the trained-model ARN, from the request's trainedModelArn member (the membership and trained-model ids inside it come from the request/state, not from any one run).
  Emulator: HTTP 400 with x-amzn-errortype ValidationException and message "1 validation error detected: Value '<instanceCount>' at 'resourceConfig.instanceCount' failed to satisfy constraint: Member must have value less than or equal to 10" — the range check fires first, so the authorization refusal never happens.
  Why: same as Cause 1 — a caller probing the authorization path on inference jobs is told its input is malformed and never sees the 403, so deny-path handling cannot be exercised against the emulator.
  Fix: the StartTrainedModelInferenceJob handler's ordering — resolve and authorize the trained model / configured model algorithm association, emitting AccessDeniedException, before member-constraint validation (instanceCount range) short-circuits.
  Traces: cleanroomsml-trained-model-inference-job-authorization-precedes-validation#task#1 (also task#2).

No existing open cleanroomsml issue covers this; the open cleanroomsml issues are unimplemented-stub issues for CancelTrainedModelInferenceJob, GetMLConfiguration, ListTrainedModelInferenceJobs, PutMLConfiguration and StartTrainedModelExportJob.

## Evidence
Cause 1 — cleanroomsml-audience-job-start-refusals#task#7, StartAudienceGenerationJob:
  expected status 403; actual status 400
  expected $.message = 'Provided ConfiguredAudienceModelArn: arn:aws:cleanrooms-ml:us-west-2:000000000000:audience-model/sim2real-absent-92f47e6a719a is invalid. Please provide a valid arn. because no resource-based policy allows the cleanrooms-ml:StartAudienceGenerationJob action'
  actual   $.message = "1 validation error detected: Value 'arn:aws:cleanrooms-ml:us-west-2:000000000000:audience-model/sim2real-absent-92f47e6a719a' at 'configuredAudienceModelArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:configured-audience-model/[-a-zA-Z0-9_/.]+"
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, actual 'ValidationException'
  divergence_key service=cleanroomsml|StartAudienceGenerationJob|status=403->400|keys=$.message,$headers.x-amzn-errortype,status

Cause 2 — cleanroomsml-trained-model-inference-job-authorization-precedes-validation#task#1, StartTrainedModelInferenceJob (cluster of 2, also task#2):
  expected status 403; actual status 400
  expected $.message = 'Configured Model Algorithm Association not found for Trained Model arn:aws:cleanrooms-ml:us-west-2:000000000000:membership/3e6b1f90-7a2c-4d8e-b4a1-b5130da826ad/trained-model/8c2d4e60-1f5a-4b7c-9e3d-d329b3291e4b. because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action'
  actual   $.message = "1 validation error detected: Value '11' at 'resourceConfig.instanceCount' failed to satisfy constraint: Member must have value less than or equal to 10"
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, actual 'ValidationException'
  divergence_key service=cleanroomsml|StartTrainedModelInferenceJob|status=403->400|keys=$.message,$headers.x-amzn-errortype,status

The specific ARNs, UUIDs and the instanceCount value 11 above are the recordings' request values filling the message template; the requirement is the status (403), the error class (AccessDeniedException) and the message template with request-derived ARNs substituted.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /audience-generation-job (StartAudienceGenerationJob) with a valid name/seedAudience and a configuredAudienceModelArn that names a cleanrooms-ml resource the caller is not authorized for and that does not match the configured-audience-model ARN pattern. Emulator answers 400 ValidationException about the ARN regex; AWS answers 403 with the 'Provided ConfiguredAudienceModelArn: ... because no resource-based policy allows the cleanrooms-ml:StartAudienceGenerationJob action' message.
3. Cause 2: POST /memberships/{membershipIdentifier}/trained-model-inference-jobs (StartTrainedModelInferenceJob) referencing a trainedModelArn with no authorized configured model algorithm association, and resourceConfig.instanceCount above the @range max of 10. Emulator answers 400 ValidationException about instanceCount; AWS answers 403 with the 'Configured Model Algorithm Association not found for Trained Model ... because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action' message.
4. Compare status, x-amzn-errortype and $.message against cleanroomsml-audience-job-start-refusals#task#7 and cleanroomsml-trained-model-inference-job-authorization-precedes-validation#task#1.
