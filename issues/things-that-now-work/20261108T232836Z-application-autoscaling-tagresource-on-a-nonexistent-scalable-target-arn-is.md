Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.tag-resource-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.tag-resource-nonexistent-resource-rejected|run#4|AssertionError|assert error.get("Code") == "ValidationException", error
Issue-Commit: 3b62b2c2884780aa78691e31a574f6e99c14f5dd

# TagResource on a nonexistent scalable-target ARN is refused with ResourceNotFoundException instead of ValidationException "Access Denied"

> _Note: this issue was identified against the code base at commit `3b62b2c2884780aa78691e31a574f6e99c14f5dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource misclassifies an unknown-but-well-formed scalable-target ARN.
Failing check: application-autoscaling.tag-resource-nonexistent-resource-rejected

AWS: Smithy (com-amazonaws-applicationautoscaling-anyscalefrontendservice) models TagResource with input member ResourceARN (target AmazonResourceName, required, @length 1..1011, @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$) and three modeled errors: ResourceNotFoundException (client, httpError 404), TooManyTagsException (client, 400) and ValidationException (client, 400). The recorded AWS response for TagResource against an ARN that matches the pattern but names no registered scalable target is HTTP 400 with error code ValidationException and message "Access Denied" — AWS does not disclose whether the target exists, it refuses the tagging call as unauthorized/invalid. Here the fixed text of the message is the whole string "Access Denied"; it has no variable parts taken from the request or from state. (Beyond the error list, the model does not state which of the two errors applies to an unknown ARN; the status 400, the code ValidationException and the message text are taken from the recorded response.) ResourceNotFoundException remains the correct answer only where AWS actually reports a missing resource; it is not the answer for this input class.

Emulator: the same call is refused with error code ResourceNotFoundException and message "Scalable Target not found" (the replay captured botocore's Error dict as {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'}), i.e. a 404-class not-found instead of the 400-class ValidationException/"Access Denied". The emulator is deciding the outcome by looking the ARN up in its scalable-target state and reporting absence, rather than refusing the tag call.

Why: a client that distinguishes AWS's error classes breaks. Code that treats ValidationException as a terminal, non-retryable client fault and ResourceNotFoundException as "resource not yet visible" (a common pattern around freshly registered targets) will retry or poll forever against the emulator, and the HTTP status differs too (404 vs 400), so retry/throttle middleware keyed on status takes a different branch. Tests that assert the AWS-documented refusal for a bogus ARN fail against the emulator even though the call is correctly refused.

Fix: belongs in the application-autoscaling TagResource handler's input validation / refusal path — the ARN-resolution branch must emit ValidationException with message "Access Denied" (HTTP 400) for a pattern-valid ARN that resolves to no registered scalable target, instead of mapping the lookup miss onto ResourceNotFoundException. The success path (tagging a registered target, then reading it back via ListTagsForResource) is unaffected and is exercised by the same check after the refusal assertion.

Traces: application-autoscaling.tag-resource-nonexistent-resource-rejected — TagResource(ResourceARN=arn:aws:application-autoscaling:<region>:<account>:scalable-target/00000000-0000-0000-0000-000000000000, Tags={env: ...}).

## Evidence
Expected (AWS, per the check's recorded behavior and the modeled ValidationException): HTTP 400, Error.Code = "ValidationException", Error.Message = "Access Denied".

Actual (emulator): Error.Code = "ResourceNotFoundException", Error.Message = "Scalable Target not found".

Replay output:
  File "check_application_autoscaling_tag_resource_nonexistent_resource_rejected.py", line 27, in run
    assert error.get("Code") == "ValidationException", error
  AssertionError: {'Message': 'Scalable Target not found', 'Code': 'ResourceNotFoundException'}

Smithy: TagResource.ResourceARN @pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$ (the bogus ARN satisfies it); errors ResourceNotFoundException (404), TooManyTagsException (400), ValidationException (400).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. With an application-autoscaling client, call TagResource with a well-formed but unregistered target ARN:
   aas.tag_resource(ResourceARN=f"arn:aws:application-autoscaling:{region}:{account}:scalable-target/00000000-0000-0000-0000-000000000000", Tags={"env": "sim2real"})
3. Observe the ClientError: the emulator answers Code=ResourceNotFoundException / Message="Scalable Target not found"; AWS answers HTTP 400 Code=ValidationException / Message="Access Denied".
4. Re-replay application-autoscaling.tag-resource-nonexistent-resource-rejected to confirm; its second half (register a dynamodb:table:WriteCapacityUnits target, TagResource on the real ARN, ListTagsForResource returns {"env": "sim2real"}) must keep passing.
