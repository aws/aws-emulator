Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.tag-resource-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.tag-resource-nonexistent-resource-rejected|run#4|AssertionError|assert error.get("Code") == "ValidationException", error
Issue-Commit: c5debdf9a60f2ac5328f40025fe52a8cd126983b

# REOPEN: TagResource on a well-formed but never-registered scalable-target ARN is still refused with 404 ResourceNotFoundException "Scalable Target not found" instead of 400 ValidationException "Access Denied"

> _Note: this issue was identified against the code base at commit `c5debdf9a60f2ac5328f40025fe52a8cd126983b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
REOPENS 20261108T232836Z-application-autoscaling-tagresource-on-a-nonexistent-scalable-target-arn-is.md: its fix left the call refused.

One net-new root cause.

Cause 1 of 1: the TagResource refusal path still decides the outcome of a pattern-valid scalable-target ARN by looking it up in registered-target state and reporting absence, so an ARN that names no target the caller ever registered is answered as a not-found instead of the service's authorization-style validation refusal. Failing check id: application-autoscaling.tag-resource-nonexistent-resource-rejected

AWS: Application Auto Scaling's tagging family addresses a scalable target by its scalable-target ARN. Smithy (com-amazonaws-applicationautoscaling-anyscalefrontendservice): TagResource.ResourceARN -> AmazonResourceName, required, @length 1..1011, @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$; TagResource.Tags -> TagMap, required; errors = ResourceNotFoundException (client, httpError 404), TooManyTagsException (client, 400), ValidationException (client, 400, awsQueryError code ValidationException). The model does not state which error class covers an ARN that resolves to no target, so for that: recorded behavior only — the recording shows AWS refusing with HTTP 400, Error.Code = ValidationException and Error.Message = "Access Denied". For this error class the whole message is fixed text: "Access Denied" interpolates nothing from the request or from state (no ARN, id, account or member name appears in it). AWS does not disclose whether the unknown target exists; it refuses the tagging call.

Emulator: the same call — TagResource with a pattern-satisfying ARN of the form arn:aws:application-autoscaling:<region>:<account>:scalable-target/<uuid> naming a target that was never registered — is refused with Error.Code = 'ResourceNotFoundException' and Error.Message = 'Scalable Target not found' (a 404-class not-found). Replay captured botocore's Error dict as {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'} at the assertion `assert error.get("Code") == "ValidationException"`. This is byte-for-byte the same refusal the earlier issue reported, so the earlier fix did not change this input class.

Why: a client that branches on AWS's error classes takes the wrong branch. Code that treats ValidationException as a terminal, non-retryable client fault and ResourceNotFoundException as "the resource is not visible yet" (the usual pattern around freshly registered targets) will poll or retry indefinitely against the emulator. The HTTP status also differs (404 vs 400), so generic SDK retry, throttle and alarm middleware keyed on status class takes a different path, and a test asserting AWS's documented refusal for a bogus ARN fails even though the call was correctly refused.

Fix: the application-autoscaling TagResource handler's ARN-resolution / refusal path — a pattern-valid ARN that resolves to no registered scalable target must produce the modeled ValidationException (HTTP 400) with message "Access Denied", not a mapping of the lookup miss onto ResourceNotFoundException. The success path is unaffected: the same check goes on to register a dynamodb:table:WriteCapacityUnits target, TagResource its real ARN and read it back via ListTagsForResource, and that half must keep passing.

Caution for the fixer (not a second cause): open issue 20261113T225650Z-application-autoscaling-tagresource-on-a-deregistered-scalable-target.md requires the OPPOSITE answer for a DEREGISTERED target's ARN (404 ResourceNotFoundException / "Scalable Target not found"). A blanket swap of the refusal class will break that check, and the blanket not-found in place today breaks this one. Hypothesis: AWS distinguishes an ARN whose target identity the account once held from an arbitrary ARN the account never owned, so the discriminator is the ARN's provenance, not merely a miss in the live-target map. Both checks must pass together.

Traces: application-autoscaling.tag-resource-nonexistent-resource-rejected (assertion at check line 27 on Error.Code, then line 29 on Error.Message == "Access Denied").

## Evidence
Check: application-autoscaling.tag-resource-nonexistent-resource-rejected [mismatch]

Replay output:
  File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_application_autoscaling_tag_resource_nonexistent_resource_rejected.py", line 27, in run
    assert error.get("Code") == "ValidationException", error
  AssertionError: {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'}

Expected (AWS, recorded): HTTP 400, Error.Code = "ValidationException", Error.Message = "Access Denied" — the message is entirely fixed text for this error class.
Actual (emulator): 404-class refusal, Error.Code = "ResourceNotFoundException", Error.Message = "Scalable Target not found".

Request under test (shape, not literal values): TagResource(ResourceARN="arn:aws:<partition-scoped>:application-autoscaling:<region>:<account>:scalable-target/<uuid never registered>", Tags={<one key/value>}). The region, account and uuid are per-run values and are not part of the requirement; the ARN only has to satisfy the modeled @pattern and name no registered target.

Smithy (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json): TagResource.ResourceARN @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$ (the ARN under test satisfies it, so a pattern rejection is not what is being asked for); errors = ResourceNotFoundException (404), TooManyTagsException (400), ValidationException (400).

Prior filing: 20261108T232836Z-application-autoscaling-tagresource-on-a-nonexistent-scalable-target-arn-is.md is marked FIXED and carries this exact check id and assertion, yet the refusal is unchanged — same code, same message, same assertion line.

Control that still passes in the same check after the refusal leg (so the fix must not disturb it): TagResource on a freshly registered dynamodb:table:WriteCapacityUnits target's ScalableTargetARN succeeds and ListTagsForResource returns the tag map just written.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. With an application-autoscaling client, call TagResource with a well-formed but never-registered target ARN:
   aas.tag_resource(ResourceARN=f"arn:aws:application-autoscaling:{region}:{account}:scalable-target/00000000-0000-0000-0000-000000000000", Tags={"env": "sim2real"})
   Observed: ClientError with Code="ResourceNotFoundException", Message="Scalable Target not found".
   Expected: HTTP 400 with Code="ValidationException", Message="Access Denied".
3. Positive control that must keep working: dynamodb CreateTable(BillingMode=PROVISIONED, ProvisionedThroughput ReadCapacityUnits/WriteCapacityUnits), wait for ACTIVE; application-autoscaling RegisterScalableTarget(ServiceNamespace="dynamodb", ResourceId="table/<table>", ScalableDimension="dynamodb:table:WriteCapacityUnits", MinCapacity=1, MaxCapacity=5); TagResource on the returned ScalableTargetARN; ListTagsForResource on it returns exactly the tags written.
4. Re-replay application-autoscaling.tag-resource-nonexistent-resource-rejected to confirm both legs.
5. Also re-replay application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected (owned by open issue 20261113T225650Z) to confirm the fix did not flip the deregistered-ARN case, which must stay 404 ResourceNotFoundException / "Scalable Target not found".
