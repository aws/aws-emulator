Issue-Source: trace-sweep-divergence
Issue-Trace: application-autoscaling-error-tag-resource-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-autoscaling-error-tag-resource-not-found applicationautoscaling-registration-error-envelopes
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: application-autoscaling|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: application-autoscaling|TagResource|4xx->4xx|$.Message
Issue-Key: application-autoscaling|TagResource|4xx->4xx|$.__type
Issue-Key: application-autoscaling|UntagResource|4xx->4xx|$.Message
Issue-Key: application-autoscaling|UntagResource|4xx->4xx|$.__type
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# application-autoscaling: tag operations on a missing scalable target return ValidationException/"Access Denied" instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
All three resource-tagging operations of Application Auto Scaling share ONE fault: when the ResourceARN names a scalable target that does not exist in the emulator's state, the emulator answers with the wrong error shape entirely — a ValidationException whose Message is the generic string "Access Denied" — rather than the modeled not-found error for the resource. The ARN itself is well-formed in every one of these recordings (it satisfies the modeled AmazonResourceName pattern), so this is not an input-validation failure being reported; it is a missing not-found lookup path in the tagging handlers, which fall through to a catch-all deny/validation envelope. 3 causes, one per operation.

Cause 1 of 3: ListTagsForResource on a non-existent scalable target.
AWS: Smithy: ListTagsForResource input ResourceARN is @required, target AmazonResourceName, @length min 1 max 1011, @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$; the operation's ONLY modeled error is ResourceNotFoundException (errorKind client), documented "The specified resource doesn't exist." with members Message and ResourceName. ValidationException is not even bound to this operation, so it can never be a legal response to it. The recording shows the not-found condition answered with HTTP 400 and body __type "ResourceNotFoundException", Message "Scalable Target not found" (note the model declares httpError 404 for that shape while the recording shows 400; status is not what diverges here, and the recorded 400 is what the emulator must keep returning). In that message, "Scalable Target not found" is the error class's fixed text — it carries no value from the request or from state.
Emulator: HTTP 400 with __type "ValidationException" and Message "Access Denied".
Why: a client that calls ListTagsForResource to discover whether a scalable target is tagged cannot tell "this target does not exist" from "your request was malformed" or "you are not authorized". SDK error handling is keyed on the __type code, so a `catch ResourceNotFoundException` branch (the normal way to treat an absent target as untagged/not registered) never fires; the call surfaces as an unrecoverable validation or authorization failure and aborts the caller instead. "Access Denied" additionally sends operators chasing a credentials/IAM problem that does not exist.
Fix: the ListTagsForResource handler's resource-resolution path — look the ARN up in the scalable-target state and raise the service's ResourceNotFoundException (code ResourceNotFoundException, message "Scalable Target not found") when it resolves to nothing, instead of returning the generic ValidationException/"Access Denied" fallback.
Traces: application-autoscaling-error-tag-resource-not-found#task#1; also reproduced by applicationautoscaling-registration-error-envelopes#task#8 (same cluster).

Cause 2 of 3: TagResource on a non-existent scalable target.
AWS: Smithy: TagResource input ResourceARN @required (same AmazonResourceName constraints) and Tags @required (TagMap); modeled errors are ResourceNotFoundException (client), TooManyTagsException (client, httpError 400) and ValidationException (client, httpError 400). ValidationException IS bound here, but it is documented "An exception was thrown for a validation issue. Review the available parameters for the API request." — it is the envelope for a bad parameter, not for a resource that is absent, which ResourceNotFoundException ("The specified resource doesn't exist.") owns. The recording shows the not-found condition answered with HTTP 400, __type "ResourceNotFoundException", Message "Scalable Target not found" — all fixed text for this error class.
Emulator: HTTP 400 with __type "ValidationException" and Message "Access Denied".
Why: tag-then-verify provisioning flows retry or re-register on ResourceNotFoundException and fail hard on ValidationException. Because the emulator reports the absent target as a validation/authorization error, such a client treats a recoverable "register the target first" condition as a permanent bad-request and never retries; worse, "Access Denied" misdirects it toward a permissions failure.
Fix: the TagResource handler's validation/resource-resolution order — resolve the ARN against scalable-target state and raise ResourceNotFoundException for an unknown target, reserving ValidationException for actual parameter defects.
Traces: application-autoscaling-error-tag-resource-not-found#task#2.

Cause 3 of 3: UntagResource on a non-existent scalable target.
AWS: Smithy: UntagResource input ResourceARN @required (same constraints) and TagKeys @required (TagKeyList, @length min 0 max 200); modeled errors ResourceNotFoundException (client) and ValidationException (client, httpError 400), with the same documented split of responsibility as TagResource. The recording shows HTTP 400, __type "ResourceNotFoundException", Message "Scalable Target not found" (fixed text).
Emulator: HTTP 400 with __type "ValidationException" and Message "Access Denied".
Why: cleanup/teardown code commonly swallows ResourceNotFoundException from UntagResource so that removing tags from an already-deleted target is idempotent. Against the emulator that call raises a ValidationException instead, which such code does not swallow, so idempotent teardown aborts partway and leaves the caller's workflow in a failed state.
Fix: the UntagResource handler's resource-resolution path — same not-found lookup and error construction as causes 1 and 2; the shared generic "Access Denied" fallback the three tagging handlers fall through to is the common root and should be replaced by the service's ResourceNotFoundException for unknown ARNs.
Traces: application-autoscaling-error-tag-resource-not-found#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 130 steps, 126 passed, 4 mismatched, score 96.9%) produced 3 divergence clusters, all in service application-autoscaling, all with the identical detail:

  $.__type:   expected 'ResourceNotFoundException', got 'ValidationException'
  $.Message:  expected 'Scalable Target not found', got 'Access Denied'

Cluster 1 — ListTagsForResource, fixture application-autoscaling-error-tag-resource-not-found step task#1, expected_status 400, actual_status 400, verdict mismatch, affected_count 2 (also applicationautoscaling-registration-error-envelopes#task#8).
Cluster 2 — TagResource, fixture application-autoscaling-error-tag-resource-not-found step task#2, expected_status 400, actual_status 400, verdict mismatch.
Cluster 3 — UntagResource, fixture application-autoscaling-error-tag-resource-not-found step task#3, expected_status 400, actual_status 400, verdict mismatch.

HTTP status (400) matches in all three; what diverges is the error code and the error message. Expected error code: ResourceNotFoundException. Actual: ValidationException. Expected message: "Scalable Target not found" (entirely fixed text for this error class — no request- or state-derived values). Actual message: "Access Denied".

Smithy model corroboration (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json): ListTagsForResource binds ResourceNotFoundException as its ONLY error — ValidationException is not bound to it at all, so the emulator's response is not a legal ListTagsForResource error. TagResource and UntagResource bind both, but ResourceNotFoundException is documented "The specified resource doesn't exist." while ValidationException is documented "An exception was thrown for a validation issue. Review the available parameters for the API request." The ResourceARNs in these steps satisfy the modeled @pattern, so no validation issue exists to report. No open issue carries these replay keys (owned_by empty, stub_path empty for all three); the existing open application-autoscaling issues all concern scaling activities, step/target-tracking policies and CloudWatch alarm behavior, not the tagging error envelope.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus()).
3. Cause 1: fixture application-autoscaling-error-tag-resource-not-found, step task#1 — call application-autoscaling ListTagsForResource with a well-formed scalable-target ResourceARN (matching ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$) that has NOT been registered via RegisterScalableTarget. Observe HTTP 400 with __type 'ValidationException' / Message 'Access Denied'; expected HTTP 400 with __type 'ResourceNotFoundException' / Message 'Scalable Target not found'. Same divergence at applicationautoscaling-registration-error-envelopes#task#8.
4. Cause 2: same fixture, step task#2 — call TagResource with that same unregistered ResourceARN and any valid Tags map. Observe the same ValidationException/'Access Denied' body instead of ResourceNotFoundException/'Scalable Target not found'.
5. Cause 3: same fixture, step task#3 — call UntagResource with that same unregistered ResourceARN and any valid TagKeys list. Observe the same ValidationException/'Access Denied' body instead of ResourceNotFoundException/'Scalable Target not found'.
6. After fixing, re-replay fixture application-autoscaling-error-tag-resource-not-found steps task#1, task#2 and task#3 (and applicationautoscaling-registration-error-envelopes#task#8) — all three causes must be closed before submitting.
