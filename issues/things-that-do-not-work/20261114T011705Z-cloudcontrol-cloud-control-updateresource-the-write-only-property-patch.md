Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected
Issue-Key: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected|run#14|AssertionError|assert message == ("Invalid patch update: writeOnlyProperties [/Definition] can only be "
Issue-Commit: 7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69

# Cloud Control UpdateResource: the write-only-property patch refusal message is emulator-authored instead of AWS's "writeOnlyProperties [...] can only be updated using 'add' operation"

> _Note: this issue was identified against the code base at commit `7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateResource

## Description
One net-new root cause in the Cloud Control API UpdateResource patch-validation path.

Cause 1 of 1: a `replace` operation targeting a resource type's write-only property is refused with the right error code but the wrong message text.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) — UpdateResource takes a required `PatchDocument` (length 1..262144, sensitive) and `Identifier`/`TypeName`. The model's error list for UpdateResource does NOT include ValidationException (it models AlreadyExistsException, ClientTokenConflictException, ConcurrentOperationException, GeneralServiceException, HandlerFailureException, HandlerInternalFailureException, InvalidCredentialsException, InvalidRequestException, NetworkFailureException, NotStabilizedException, NotUpdatableException, PrivateTypeException, ResourceConflictException, ResourceNotFoundException, ServiceInternalErrorException, ServiceLimitExceededException, ThrottlingException, TypeNotFoundException, UnsupportedActionException), so for the message itself this is RECORDED BEHAVIOR ONLY: Cloud Control surfaces the resource type's schema-level patch refusal synchronously as error code `ValidationException` with the message

    Invalid patch update: writeOnlyProperties [<paths>] can only be updated using 'add' operation

The fixed text of that message is `Invalid patch update: writeOnlyProperties [` ... `] can only be updated using 'add' operation`. The only variable part is `<paths>`: the JSON-pointer path(s) from the caller's PatchDocument that the resource type's schema declares in `writeOnlyProperties` (for AWS::StepFunctions::StateMachine that is `/Definition`). Nothing in the message is derived from the resource's state, its identifier or the run.

Emulator: the refusal carries the correct error code `ValidationException`, but the message is emulator-authored:

    Invalid PatchDocument: the replace operation at path '/Definition' could not be applied to the resource model

It reports the refusal as a generic "could not be applied" failure of the patch application step rather than as the write-only-property rule, and it never names `writeOnlyProperties` or the required `'add'` operation.

Why: a client that reads Cloud Control refusals — the CLI, the CDK/CloudFormation resource-provider tooling, and any retry wrapper that decides whether to re-send the same change as an `add` — matches on this sentence to tell "this property is write-only, resend with add" apart from "the patch is structurally inapplicable (bad path, type mismatch, missing target)". Against this emulator both classes collapse into one opaque message, so the actionable remedy AWS states in the message is lost and the caller cannot distinguish a recoverable write-only patch from a genuinely malformed patch document.

Fix: the UpdateResource handler's patch-validation step in the cloudcontrol service — the write-only-property gate must run before the patch is applied to the resource model, collect the request's patch paths that the resource type schema marks write-only, and refuse with ValidationException carrying AWS's sentence with those paths interpolated, instead of falling through to the generic patch-application failure message.

Related open issues that do NOT cover this cause (patch validation generally, and readOnlyProperties rather than writeOnlyProperties): 20261102T095426Z-cloudcontrol-9-net-new-causes-desiredstate-patchdocument-are.md and 20261110T221719Z-cloudcontrol-3-net-new-causes-resourcenotfoundexception-answered-at.md.

Traces: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected

## Evidence
Check cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-replace-patch-rejected [mismatch].

Request: UpdateResource(TypeName="AWS::StepFunctions::StateMachine", Identifier=<state machine ARN created through Cloud Control>, PatchDocument=[{"op": "replace", "path": "/Definition", "value": {...}}]).

Expected (AWS): Error.Code == "ValidationException" and Error.Message ==
  "Invalid patch update: writeOnlyProperties [/Definition] can only be updated using 'add' operation"
(fixed text; `/Definition` is the write-only path taken from the request's PatchDocument and the resource type's schema).

Actual (emulator): Error.Code == "ValidationException" (matches), Error.Message ==
  "Invalid PatchDocument: the replace operation at path '/Definition' could not be applied to the resource model"

Replay output:
  File "check_cloudcontrol__iam__stepfunctions_cloudcontrol_state_machine_definition_replace_patch_rejected.py", line 45, in run
    assert message == ("Invalid patch update: writeOnlyProperties [/Definition] can only be "
  AssertionError: the refusal message was "Invalid PatchDocument: the replace operation at path '/Definition' could not be applied to the resource model"

The preceding assertions passed: the Cloud Control create reached OperationStatus SUCCESS and Step Functions DescribeStateMachine read back the created definition, so the divergence is confined to the refusal message of the replace patch.

## Steps to Reproduce
1. iam:CreateRole a role assumable by states.amazonaws.com.
2. cloudcontrol:CreateResource(TypeName="AWS::StepFunctions::StateMachine", DesiredState={"StateMachineName": <name>, "RoleArn": <role arn>, "Definition": {"StartAt": "P", "States": {"P": {"Type": "Pass", "Result": {"version": 1}, "End": true}}}}) and wait for OperationStatus SUCCESS; note ProgressEvent.Identifier.
3. cloudcontrol:UpdateResource(TypeName="AWS::StepFunctions::StateMachine", Identifier=<identifier>, PatchDocument=json([{"op": "replace", "path": "/Definition", "value": {"StartAt": "Q", "States": {"Q": {"Type": "Pass", "Result": {"version": 2}, "End": true}}}}])).
4. Read Error.Code and Error.Message from the refusal. EXPECT ValidationException with "Invalid patch update: writeOnlyProperties [/Definition] can only be updated using 'add' operation"; OBSERVED ValidationException with "Invalid PatchDocument: the replace operation at path '/Definition' could not be applied to the resource model".
5. (Controls that already pass and must keep passing: stepfunctions:DescribeStateMachine still reports the original definition and revisionId after the refusal, and re-sending the same change as {"op": "add", "path": "/Definition", ...} succeeds.)
