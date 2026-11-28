Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-trained-model-inference-job-refusals-in-membership
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-trained-model-inference-job-refusals-in-membership
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|StartTrainedModelInferenceJob|4xx->4xx|$.message
Issue-Commit: a5844fb9ceebf5cc993216dbda518c9083ab8615

# cleanroomsml StartTrainedModelInferenceJob AccessDeniedException message omits the collaboration-scoped not-found text

> _Note: this issue was identified against the code base at commit `a5844fb9ceebf5cc993216dbda518c9083ab8615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTrainedModelInferenceJob

## Description
Cause 1 of 1: when StartTrainedModelInferenceJob is called with a configuredModelAlgorithmAssociationArn that does not resolve inside the membership's collaboration, the emulator returns the right HTTP status and error shape but an error message with the wrong subject and wrong fixed text.

AWS: Smithy (services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice): StartTrainedModelInferenceJob POST /memberships/{membershipIdentifier}/trained-model-inference-jobs, input members trainedModelArn (required, ConfiguredModelAlgorithmAssociationArn-shaped siblings) and configuredModelAlgorithmAssociationArn (optional, pattern ^arn:aws[-a-z]*:cleanrooms-ml:...:membership/<uuid>/configured-model-algorithm-association/...$); modeled error AccessDeniedException @httpError 403 with a required `message` member, documented "You do not have sufficient access to perform this action." The model does not fix the message string, so the message requirement is recorded behavior only, taken from the recorded response: AWS answers HTTP 403 with AccessDeniedException whose message is built from the resource-type token for the resource that could not be resolved, the ARN the request asked for, and the collaboration it was looked up in, followed by the policy-denial clause for the attempted action. Its FIXED text is "CONFIGURED_MODEL_ALGORITHM_ASSOCIATION: ", " not found in the collaboration: ", ". Please ensure the arn is correct and that it exists in the collaboration." and " because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action". Its VARIABLE parts are the configuredModelAlgorithmAssociationArn taken from the request, the collaboration id taken from the membership in state, and the action name taken from the operation being invoked. The recorded run's particular ARN and the collaboration UUID in it fill that template; they are not part of it.

Emulator: answers HTTP 403 with AccessDeniedException (status and error shape correct) but the message reads "Configured Model Algorithm Association not found for Trained Model <trainedModelArn>. because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action". Three defects: (a) the resource-type token is prose-cased "Configured Model Algorithm Association" instead of AWS's upper-snake "CONFIGURED_MODEL_ALGORITHM_ASSOCIATION:" prefix; (b) the ARN interpolated is the request's trainedModelArn, not the configuredModelAlgorithmAssociationArn that actually failed to resolve, so the message names the wrong resource; (c) the collaboration id and the "Please ensure the arn is correct and that it exists in the collaboration." remediation sentence are dropped entirely.

Why: a client relying on AWS's behavior cannot tell which resource the refusal is about. The message names the trained model, which does exist, so error-handling and operator diagnostics that key on the resource-type token or parse the failing ARN out of the message are pointed at the wrong object, and the collaboration scope that explains WHY the lookup failed is absent, so a caller cannot see that the association is simply not a member of this collaboration. Only the message diverges here, so the fault is invisible to status- or code-based handling and surfaces as a misleading operator message.

Fix: the StartTrainedModelInferenceJob handler's association-resolution path in the cleanroomsml service crate, where the AccessDeniedException is constructed: resolve and report the configuredModelAlgorithmAssociationArn from the request (not trainedModelArn), emit the upper-snake resource-type prefix, and include the membership's collaboration id plus the remediation sentence before appending the policy-denial clause. This is message-construction, not status or shape; the 403 and the AccessDeniedException shape are already correct.

Related but NOT covering this: open issue 20261127T195958Z-cleanroomsml-validationexception-message-echoes-the-rejected-name-value.md concerns ValidationException @length message text on StartAudienceExportJob/StartAudienceGenerationJob and does not carry this operation or this error class.

Traces: cleanroomsml-trained-model-inference-job-refusals-in-membership#task#4.

## Evidence
Fixture cleanroomsml-trained-model-inference-job-refusals-in-membership, step task#4, operation StartTrainedModelInferenceJob, service cleanroomsml.
Verdict: mismatch on $.message. Expected status 403, actual status 403 (status agrees; only the message diverges).

Expected (recorded AWS) $.message:
  "CONFIGURED_MODEL_ALGORITHM_ASSOCIATION: arn:aws:cleanrooms-ml:us-west-2:000000000000:membership/0668a917-ae3e-435d-8bdf-dc7d8d9926b7/configured-model-algorithm-association/5a9c1e30-8b4d-4f2a-a7e6-84102e8defe0 not found in the collaboration: 469ea049-e720-4832-bad8-47b933d087c6. Please ensure the arn is correct and that it exists in the collaboration. because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action"

Actual (emulator) $.message:
  "Configured Model Algorithm Association not found for Trained Model arn:aws:cleanrooms-ml:us-west-2:000000000000:membership/0668a917-ae3e-435d-8bdf-dc7d8d9926b7/trained-model/7f3a2b10-0c4d-4e8f-a1b2-4a6413a07fa1. because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action"

Template decomposition (the requirement; the ARNs/UUIDs above are this run's values filling it):
  FIXED: "CONFIGURED_MODEL_ALGORITHM_ASSOCIATION: " | " not found in the collaboration: " | ". Please ensure the arn is correct and that it exists in the collaboration." | " because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action"
  VARIABLE: configuredModelAlgorithmAssociationArn (from the request), collaboration id (from the membership in state), action name (from the operation).

Agreeing facts: HTTP 403 and the AccessDeniedException shape (Smithy: AccessDeniedException, errorKind client, @httpError 403, required member `message`) both match; the divergence is confined to the message string.

Cluster: affected_count 1 (this step is the whole cluster). owned_by is empty — no open issue carries this replay key. No stub_path — the operation is implemented and answering.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout.
2. Replay the wire corpus (replay_wire_corpus()); trace set simparity-wire-aws-v1.
3. Go to fixture cleanroomsml-trained-model-inference-job-refusals-in-membership, step task#4.
   Equivalently, by hand: create a cleanrooms-ml membership in a collaboration and a trained model inside it, then call StartTrainedModelInferenceJob POST /memberships/{membershipIdentifier}/trained-model-inference-jobs with a valid trainedModelArn for that membership plus a configuredModelAlgorithmAssociationArn that is well-formed for the membership but is not associated in the collaboration.
4. Observe the response: HTTP 403, AccessDeniedException.
5. Compare $.message against AWS's. The emulator names the TRAINED MODEL arn under a prose-cased "Configured Model Algorithm Association not found for Trained Model ..." prefix; AWS names the CONFIGURED_MODEL_ALGORITHM_ASSOCIATION arn from the request, states the collaboration id it was not found in, and appends "Please ensure the arn is correct and that it exists in the collaboration." before the "because no resource-based policy allows the cleanrooms-ml:StartTrainedModelInferenceJob action" clause.
6. The step is reported as verdict=mismatch with expected_status 403 and actual_status 403, diverging key $.message.
