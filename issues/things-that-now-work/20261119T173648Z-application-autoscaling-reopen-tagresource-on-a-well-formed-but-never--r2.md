Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.tag-resource-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.tag-resource-nonexistent-resource-rejected|run#4|AssertionError|assert error.get("Code") == "ValidationException", error
Issue-Commit: 41e8a84fade3b4fb2b03f13caad89af531f42c2d

# REOPEN: TagResource on a well-formed but never-registered scalable-target ARN is still refused 404 ResourceNotFoundException "Scalable Target not found" instead of 400 ValidationException "Access Denied"

> _Note: this issue was identified against the code base at commit `41e8a84fade3b4fb2b03f13caad89af531f42c2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
REOPENS 20261114T040100Z-application-autoscaling-reopen-tagresource-on-a-well-formed-but-never.md: its fix left the call refused. (That issue itself reopened 20261108T232836Z-application-autoscaling-tagresource-on-a-nonexistent-scalable-target-arn-is.md, also marked fixed; the refusal is unchanged — same code, same message, same assertion line.)

One net-new root cause.

Cause 1 of 1: the TagResource refusal path still decides a pattern-valid scalable-target ARN by looking it up in registered-target state and reporting the miss as absence, so an ARN that names no target the caller ever registered is answered as a not-found instead of the service's authorization-style validation refusal. Failing check id: application-autoscaling.tag-resource-nonexistent-resource-rejected

AWS: Application Auto Scaling's tagging family addresses a scalable target by its scalable-target ARN. Smithy (com.amazonaws.applicationautoscaling#AnyScaleFrontendService): TagResource.ResourceARN -> AmazonResourceName, required, @length 1..1011, @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$; TagResource.Tags -> TagMap, required; errors = ResourceNotFoundException (client, httpError 404), TooManyTagsException (client, 400), ValidationException (client, 400, awsQueryError code ValidationException). The model does not say which class covers an ARN that resolves to no target, so for that: recorded behavior only — the recording shows AWS refusing with HTTP 400, Error.Code = "ValidationException", Error.Message = "Access Denied". For this error class the message is wholly FIXED text: "Access Denied" interpolates nothing from the request or from state (no ARN, account, region, uuid or member name appears in it). AWS does not disclose whether an unknown target exists; it refuses the tagging call outright.

Emulator: the same call — TagResource with a pattern-satisfying ARN of the form arn:aws:application-autoscaling:<region>:<account>:scalable-target/<uuid never registered> — is refused with Error.Code = "ResourceNotFoundException" and Error.Message = "Scalable Target not found" (a 404-class not-found). Replay captured botocore's Error dict as {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'} at the assertion `assert error.get("Code") == "ValidationException"`.

Why: a client branching on AWS's error classes takes the wrong branch. Code that treats ValidationException as a terminal, non-retryable client fault and ResourceNotFoundException as "the target is not visible yet" (the usual pattern right after RegisterScalableTarget) polls or retries indefinitely against the emulator. The HTTP status class also differs (404 vs 400), so SDK retry/throttle middleware keyed on status takes a different path, and any test asserting AWS's documented refusal for a bogus ARN fails even though the call was correctly refused.

Fix: the application-autoscaling TagResource handler's ARN-resolution / refusal path — a pattern-valid ARN that resolves to no registered scalable target must produce the modeled ValidationException (HTTP 400) with the fixed message "Access Denied", not a mapping of the lookup miss onto ResourceNotFoundException. The success leg in the same check (register a dynamodb:table:WriteCapacityUnits target, TagResource its real ScalableTargetARN, read it back with ListTagsForResource) passes today and must keep passing.

Caution for the fixer (not a second cause): open issues 20261113T225650Z-application-autoscaling-tagresource-on-a-deregistered-scalable-target.md and 20261115T113432Z-application-autoscaling-tagging-family-answers-validationexception-access-denied.md require the OPPOSITE answer for an ARN whose target the account once held or that the wire corpus recorded (404/ResourceNotFoundException, "Scalable Target not found"). The blanket not-found in place today satisfies those and breaks this one; a blanket swap would invert the breakage. Hypothesis: the discriminator is the ARN's provenance (an identity this account once held vs. an arbitrary ARN it never owned), not merely a miss in the live-target map. All of these must pass together.

Traces: application-autoscaling.tag-resource-nonexistent-resource-rejected (assertion at check line 27 on Error.Code, then line 29 on Error.Message == "Access Denied").

## Evidence
Check: application-autoscaling.tag-resource-nonexistent-resource-rejected [mismatch]

Replay output:
  File "run.py", line 456, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_application_autoscaling_tag_resource_nonexistent_resource_rejected.py", line 27, in run
    assert error.get("Code") == "ValidationException", error
  AssertionError: {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'}

Expected (AWS, recorded): HTTP 400, Error.Code = "ValidationException", Error.Message = "Access Denied" — the message is entirely fixed text for this error class, with no request- or state-derived part.
Actual (emulator): 404-class refusal, Error.Code = "ResourceNotFoundException", Error.Message = "Scalable Target not found".

Request under test (shape, not literal values): TagResource(ResourceARN="arn:aws:application-autoscaling:<region>:<account>:scalable-target/<uuid never registered>", Tags={<one key/value>}). Region, account and uuid are per-run values and are not part of the requirement; the ARN only has to satisfy the modeled @pattern and name no registered target.

Smithy (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json): TagResource.ResourceARN @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$ (the ARN under test satisfies it, so a pattern rejection is not what is asked for); errors = ResourceNotFoundException (404), TooManyTagsException (400), ValidationException (400).

Prior filings carrying this exact check id and assertion, both marked FIXED, refusal unchanged: 20261108T232836Z-application-autoscaling-tagresource-on-a-nonexistent-scalable-target-arn-is.md and 20261114T040100Z-application-autoscaling-reopen-tagresource-on-a-well-formed-but-never.md.

Control that still passes in the same check after the refusal leg (the fix must not disturb it): TagResource on a freshly registered dynamodb:table:WriteCapacityUnits target's ScalableTargetARN succeeds and ListTagsForResource returns exactly the tag map just written.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. With an application-autoscaling client, call TagResource with a well-formed but never-registered target ARN:
   aas.tag_resource(ResourceARN=f"arn:aws:application-autoscaling:{region}:{account}:scalable-target/00000000-0000-0000-0000-000000000000", Tags={"env": "sim2real"})
   Observed: ClientError with Code="ResourceNotFoundException", Message="Scalable Target not found".
   Expected: HTTP 400 with Code="ValidationException", Message="Access Denied".
3. Positive control that must keep working: dynamodb CreateTable(BillingMode=PROVISIONED, ProvisionedThroughput Read/WriteCapacityUnits), wait ACTIVE; RegisterScalableTarget(ServiceNamespace="dynamodb", ResourceId="table/<table>", ScalableDimension="dynamodb:table:WriteCapacityUnits", MinCapacity=1, MaxCapacity=5); TagResource on the returned ScalableTargetARN; ListTagsForResource returns exactly those tags.
4. Re-replay application-autoscaling.tag-resource-nonexistent-resource-rejected; both legs must pass.
5. Also re-replay the deregistered-ARN tagging checks owned by 20261113T225650Z / 20261115T113432Z, which must stay 404 ResourceNotFoundException / "Scalable Target not found"; both classes have to pass together.
