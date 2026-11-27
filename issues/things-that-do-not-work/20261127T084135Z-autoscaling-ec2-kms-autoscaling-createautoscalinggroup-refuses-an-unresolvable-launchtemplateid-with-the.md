Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates
Issue-Key: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates|run#22|AssertionError|assert message == f"Malformed launch template Id: {bogus_lt_id}", (
Issue-Commit: 891bdb4ec3b1dd6099e92756c6128712280d1708

# autoscaling: CreateAutoScalingGroup refuses an unresolvable LaunchTemplateId with the by-name "does not exist" wording instead of AWS's "Malformed launch template Id: <id>"

> _Note: this issue was identified against the code base at commit `891bdb4ec3b1dd6099e92756c6128712280d1708`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
One net-new cause on the Auto Scaling (awsQuery, 2011-01-01) CreateAutoScalingGroup validation path: the launch-template-identity refusal message.

Cause 1 of 1: CreateAutoScalingGroup whose LaunchTemplate names a LaunchTemplateId that the service cannot resolve is refused with the wrong message.

AWS: HTTP 400, ErrorResponse.Error.Type Sender, Error.Code ValidationError, Error.Message 'Malformed launch template Id: <launch-template-id>' — the fixed text is 'Malformed launch template Id: ' and the only variable part is the LaunchTemplateId value copied from the request (in the recording a syntactically well-formed but non-resolvable id). This refusal also wins over the group-name uniqueness check: the same request re-uses an existing AutoScalingGroupName, and AWS still reports the launch-template fault rather than AlreadyExists. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): CreateAutoScalingGroupType.LaunchTemplate targets LaunchTemplateSpecification; the operation's modeled client errors are only AlreadyExistsFault (awsQueryError AlreadyExists, 400) and LimitExceededFault, so a launch-template identity fault is reported through the service-common ValidationError — the sentence itself is recorded behavior only.

Emulator: HTTP 400 with Error.Code ValidationError (correct) but Error.Message 'The specified launch template, with template ID <launch-template-id>, does not exist.' — i.e. it applies the by-NAME not-found wording ('The specified launch template, with template name <name>, does not exist.') to an id reference, where AWS words the id case as a malformed-id refusal. The group-name/launch-template validation ordering is correct here (the launch-template fault does win), so only the message text diverges.

Why: the Code is the generic ValidationError, so the message is the only machine-visible explanation of the fault. A client (or a CLI/CDK error matcher) keyed on AWS's 'Malformed launch template Id:' prefix treats the refusal as an unknown failure, and a client reading 'does not exist' concludes the template was deleted rather than that the id it passed is not a valid template id.

Fix: the CreateAutoScalingGroup handler's launch-template identity validation must select AWS's wording by reference kind — 'Malformed launch template Id: <id>' when the request carries LaunchTemplateId, reserving 'The specified launch template, with template name <name>, does not exist.' for LaunchTemplateName. Message text only; status and Code are already right.

Related but not covering: open issue 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md describes the same refusal from the WIRE corpus (its Cause 2), but recorded the emulator answering Code AlreadyExists there; the emulator has since moved to ValidationError with this re-worded sentence, and that issue's replay keys do not include this behavioral check.

Traces: autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1, emulator up), check autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates:

  File "check_autoscaling__ec2__kms_asglf_rebuilding_template_and_group_rejected_as_duplicates.py", line 65, in run
    assert message == f"Malformed launch template Id: {bogus_lt_id}"
  AssertionError: expected the exact malformed-template-id message, got 'The specified launch template, with template ID lt-00000000000000000, does not exist.'

The check first asserts the error Code is 'ValidationError' (that assertion passed, so the emulator's status/Code are correct and the launch-template check correctly beats the duplicate-group-name check), then asserts the exact message.

Expected: Error.Code 'ValidationError', Error.Message 'Malformed launch template Id: <LaunchTemplateId from the request>'.
Actual:   Error.Code 'ValidationError', Error.Message 'The specified launch template, with template ID <LaunchTemplateId from the request>, does not exist.'

Fixed text vs values: 'Malformed launch template Id: ' is AWS's fixed prefix; the trailing id is the LaunchTemplateId supplied in the request (the recording's 'lt-00000000000000000' is one run's value, not part of the requirement). The group name, VPC/subnet ids and KMS key in the surrounding scenario are per-run values.

Smithy grounding: CreateAutoScalingGroup errors = AlreadyExistsFault (AlreadyExists/400), LimitExceededFault, ResourceContentionFault (500), ServiceLinkedRoleFailure (500); no launch-template error is modeled, so the refusal is the service-common ValidationError.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite check autoscaling__ec2__kms.asglf-rebuilding-template-and-group-rejected-as-duplicates.
2. Minimal repro: create a KMS key, a VPC with one subnet, and a launch template; CreateAutoScalingGroup once with that template to take a group name.
3. Call CreateAutoScalingGroup again with the SAME AutoScalingGroupName, MinSize=0, MaxSize=2, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, and LaunchTemplate={LaunchTemplateId: 'lt-00000000000000000', Version: '$Latest'} (a well-formed id that resolves to no template).
4. Observe HTTP 400 Error.Code 'ValidationError' with Error.Message 'The specified launch template, with template ID lt-00000000000000000, does not exist.'; AWS answers Error.Message 'Malformed launch template Id: lt-00000000000000000'.
5. Confirm the refusal left the existing group's bounds unchanged (the check also asserts MinSize/DesiredCapacity/MaxSize are untouched).
6. Re-replay the check above after the fix.
