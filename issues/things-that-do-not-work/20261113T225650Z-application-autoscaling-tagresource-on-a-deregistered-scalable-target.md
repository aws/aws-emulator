Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected|run#14|AssertionError|assert (error.get('Code'), error.get('Message')) == (
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# application-autoscaling: TagResource on a deregistered scalable-target ARN answers ValidationException "Access Denied" instead of ResourceNotFoundException "Scalable Target not found"

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause.

Cause 1 of 1: the tag-write path does not resolve the scalable-target ARN against registered-target state, so a well-formed ARN whose target no longer exists is refused as an authorization/validation failure rather than a not-found. Failing check id: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected

AWS: Application Auto Scaling's resource-tagging family addresses a scalable target by its scalable-target ARN. Smithy (com-amazonaws-applicationautoscaling-anyscalefrontendservice): TagResource.ResourceARN -> AmazonResourceName, required, @length 1..1011, @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$; TagResource errors = ResourceNotFoundException (httpError 404), TooManyTagsException (400), ValidationException (400). The same ResourceNotFoundException is modeled on UntagResource and ListTagsForResource. When the ARN is syntactically valid but names a target that has been deregistered, AWS refuses with HTTP 404, error code ResourceNotFoundException and the message `Scalable Target not found` — that sentence is wholly fixed text for this error class (it interpolates nothing from the request or from state). ValidationException is reserved for a malformed/unsatisfiable request (e.g. an ARN failing the @pattern), not for a resource that is absent; "Access Denied" is not a message AWS emits on this operation at all and is not one of its modeled error classes.

Emulator: after DeregisterScalableTarget removes the target, TagResource on the ARN that RegisterScalableTarget returned is refused with Code='ValidationException' and Message='Access Denied' (a 400-class refusal). The positive control in the same check passes — TagResource, UntagResource and ListTagsForResource all succeed on the live ARN, and ListTagsForResource on the dead ARN already answers the correct ResourceNotFoundException / 'Scalable Target not found' — so the divergence is specific to the tag-WRITE path's handling of an unresolvable target, not to ARN parsing or to the tagging family as a whole.

Why: a client cannot distinguish "the target is gone" from "you are not permitted / your request is invalid". Retry and cleanup logic keys on ResourceNotFoundException to treat a deregistered target as already-cleaned-up and on 404 vs 400 for whether a retry can ever succeed; an "Access Denied" ValidationException makes idempotent teardown abort, makes paved-road error handlers surface a spurious permissions problem, and breaks any code that catches botocore's ResourceNotFoundException for this operation. The wrong status class (400 vs 404) additionally mis-steers generic SDK/HTTP-level retry and alarm classification.

Fix: the TagResource handler's target resolution/validation in the application-autoscaling crate — the ARN must be resolved against the registered-scalable-target state and a miss must raise the modeled ResourceNotFoundException (404, message 'Scalable Target not found'), the same resolution the ListTagsForResource handler already performs. The same resolution should back UntagResource, which the check exercises immediately after TagResource and which never ran because TagResource failed first. Nothing in this cause asks for the refusal to change when the target is live, and the refusal must remain non-mutating (the check also asserts the refused TagResource does not revive the deregistered target).

Traces: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected (asserts, after DeregisterScalableTarget, that each of ListTagsForResource / TagResource / UntagResource on the target ARN is refused with ('ResourceNotFoundException', 'Scalable Target not found')).

## Evidence
Check: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected [mismatch]

Assertion that failed (check_application_autoscaling_tag_operations_on_a_deregistered_target_arn_rejected.py line 117):
  assert (error.get('Code'), error.get('Message')) == ('ResourceNotFoundException', 'Scalable Target not found')

AssertionError: TagResource on the deregistered target ARN was refused with {'Message': 'Access Denied', 'Code': 'ValidationException'}

Expected (AWS, and the modeled error set): HTTP 404, Error.Code = 'ResourceNotFoundException', Error.Message = 'Scalable Target not found' (entirely fixed text for this class).
Actual (emulator): 400-class refusal, Error.Code = 'ValidationException', Error.Message = 'Access Denied'.

Controls inside the same check that did NOT diverge:
  - TagResource/UntagResource/ListTagsForResource on the LIVE target ARN all succeed; list_tags_for_resource returns {'env': 'test'}.
  - ListTagsForResource on the SAME deregistered ARN is refused correctly (the loop reached TagResource, so the ListTagsForResource iteration asserted equal).

Smithy backing: TagResource errors = [ResourceNotFoundException httpError 404, TooManyTagsException 400, ValidationException 400]; ResourceARN @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$ — the ARN under test satisfies the pattern (it is the value RegisterScalableTarget returned as ScalableTargetARN), so ValidationException is not the applicable class.

## Steps to Reproduce
1. dynamodb: CreateTable with BillingMode=PROVISIONED and ProvisionedThroughput {ReadCapacityUnits:1, WriteCapacityUnits:1}; wait for TableStatus=ACTIVE.
2. application-autoscaling: RegisterScalableTarget(ServiceNamespace='dynamodb', ResourceId='table/<table>', ScalableDimension='dynamodb:table:ReadCapacityUnits', MinCapacity=1, MaxCapacity=5, Tags={'team':'blue'}) and keep the returned ScalableTargetARN.
3. Positive control: TagResource(ResourceARN=<arn>, Tags={'env':'test'}), UntagResource(ResourceARN=<arn>, TagKeys=['team']), ListTagsForResource(ResourceARN=<arn>) -> {'env':'test'}. All succeed.
4. DeregisterScalableTarget(ServiceNamespace='dynamodb', ResourceId='table/<table>', ScalableDimension='dynamodb:table:ReadCapacityUnits').
5. Call TagResource(ResourceARN=<arn>, Tags={'env':'prod'}) on that now-dead ARN.
   Observed: ClientError with Code='ValidationException', Message='Access Denied'.
   Expected: HTTP 404 ResourceNotFoundException with Message='Scalable Target not found'.
6. Also call UntagResource(ResourceARN=<arn>, TagKeys=['env']) on the dead ARN and expect the same 404 ResourceNotFoundException / 'Scalable Target not found' (this leg is unreached today because step 5 fails first).
7. DescribeScalableTargets(ServiceNamespace='dynamodb', ResourceIds=['table/<table>']) must still return an empty ScalableTargets list — the refused TagResource must not re-create the target.
