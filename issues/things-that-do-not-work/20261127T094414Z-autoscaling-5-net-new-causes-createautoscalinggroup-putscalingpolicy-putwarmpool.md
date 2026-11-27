Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.create-asg-invalid-launch-template-id-rejected autoscaling.desired-capacity-type-requires-attribute-based-selection autoscaling.group-tag-key-with-reserved-aws-prefix-rejected autoscaling.step-scaling-policy-step-adjustment-ranges-validated autoscaling.warm-pool-refused-on-weighted-or-spot-mixed-instances-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.create-asg-invalid-launch-template-id-rejected|run#9|AssertionError|assert message == f"Malformed launch template Id: {bogus_lt_id}", (
Issue-Key: autoscaling.desired-capacity-type-requires-attribute-based-selection|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling.group-tag-key-with-reserved-aws-prefix-rejected|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling.step-scaling-policy-step-adjustment-ranges-validated|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling.warm-pool-refused-on-weighted-or-spot-mixed-instances-group|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling: 5 net-new causes — CreateAutoScalingGroup/PutScalingPolicy/PutWarmPool validation is overly permissive and the unresolvable-LaunchTemplateId refusal is re-worded

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Five net-new root causes on the autoscaling control plane, all reached by plain CreateAutoScalingGroup / PutScalingPolicy / PutWarmPool calls. Four of them are refusals AWS makes that the emulator does not make at all (the call returns 200), one is a refusal whose message text is emulator-authored. None of these checks is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 5: CreateAutoScalingGroup refuses a well-formed but nonexistent LaunchTemplateId with the wrong message.
AWS: CreateAutoScalingGroup with LaunchTemplate={LaunchTemplateId: <well-formed, nonexistent>} is refused with HTTP 400, error code ValidationError and the message "Malformed launch template Id: <the LaunchTemplateId from the request>". Fixed text: "Malformed launch template Id: "; variable part: the LaunchTemplateId value taken verbatim from the request. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): CreateAutoScalingGroup takes LaunchTemplate: LaunchTemplateSpecification and models only AlreadyExists/LimitExceeded/ResourceContention/ServiceLinkedRoleFailure — ValidationError is the awsQuery common sender error, so this is recorded behavior for the message text.
Emulator: returns ValidationError (code correct) but with its own wording, "The specified launch template, with template ID <id>, does not exist." — i.e. the by-name/not-found phrasing is used for the by-id path.
Why: a client that branches on the refusal text (or an operator reading it) is told the template is absent rather than that the id is malformed, and any assertion or log scraper matching AWS's fixed text never matches.
Fix: the message construction in the CreateAutoScalingGroup handler's launch-template resolution/validation path.
Note: the open issue 20261127T084135Z-autoscaling-ec2-kms-autoscaling-createautoscalinggroup-refuses-an-unresolvable-launchtemplateid-with-the.md describes the same wording defect but carries a different replay key (an autoscaling__ec2__kms check), so it does not cover this check; see also 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md.
Traces: autoscaling.create-asg-invalid-launch-template-id-rejected

Cause 2 of 5: DesiredCapacityType="vcpu" is accepted on a group that has no attribute-based instance type selection.
AWS: CreateAutoScalingGroup with DesiredCapacityType="vcpu" but a plain LaunchTemplate (no MixedInstancesPolicy with InstanceRequirements) is refused with HTTP 400 ValidationError and the fixed message "The DesiredCapacityType vcpu is only allowed in mixed instances groups containing InstanceRequirements. Please include a MixedInstancesPolicy with InstanceRequirements for your group and try again." (the only variable part is the DesiredCapacityType value from the request). Smithy: CreateAutoScalingGroup.DesiredCapacityType is an XmlStringMaxLen255 member; the rule is not in the model, so the status/code/text above are recorded behavior.
Emulator: the call SUCCEEDS (HTTP 200) and the group is created.
Why: a client that uses vcpu/memory-based desired capacity gets a group that real AWS would never have created, so capacity arithmetic silently means units the group cannot honour, and the negative test of a deployment template passes here and fails against AWS.
Fix: CreateAutoScalingGroup input validation (cross-member check between DesiredCapacityType and MixedInstancesPolicy.InstanceRequirements).
Traces: autoscaling.desired-capacity-type-requires-attribute-based-selection

Cause 3 of 5: a group tag key with the reserved "aws:" prefix is accepted.
AWS: CreateAutoScalingGroup with Tags=[{Key: "aws:<suffix>", ...}] is refused with HTTP 400 ValidationError and the message "Your account is not allowed to create tag <the tag key from the request>" — fixed text "Your account is not allowed to create tag ", variable part the offending tag key. No group is created; the same key without the aws: prefix is accepted. Smithy: CreateAutoScalingGroup.Tags: Tags; the reserved-prefix rule is not modeled, so this is recorded behavior.
Emulator: the call SUCCEEDS (HTTP 200) and stores the aws:-prefixed tag as a user tag.
Why: aws: is the reserved namespace the service itself writes (aws:autoscaling:groupName). Accepting a client-supplied aws: tag lets a caller shadow service-managed tags, and tag-governance tests that rely on the refusal pass here only.
Fix: tag-key validation shared by CreateAutoScalingGroup / CreateOrUpdateTags.
Traces: autoscaling.group-tag-key-with-reserved-aws-prefix-rejected

Cause 4 of 5: PutScalingPolicy does not validate StepAdjustment intervals.
AWS: a StepScaling policy whose StepAdjustments overlap, leave a gap, or carry neither bound is refused with HTTP 400 ValidationError and one of three fixed messages (no variable parts): "StepAdjustment intervals cannot overlap.", "StepAdjustment intervals cannot have gaps between them.", "Both lower and upper bounds of a StepAdjustment cannot be left unspecified." The refused policies are not listed by DescribePolicies; a contiguous set is accepted. Smithy: PutScalingPolicy.StepAdjustments: StepAdjustments with MetricIntervalLowerBound/MetricIntervalUpperBound; the interval rules are not modeled, so these are recorded behavior.
Emulator: all three malformed step sets are ACCEPTED (HTTP 200) and the policies are stored.
Why: a step policy with gaps or overlaps has no deterministic step for a breach value; the client believes it configured valid scaling and gets behavior AWS would have rejected at configuration time.
Fix: PutScalingPolicy input validation for PolicyType StepScaling.
Traces: autoscaling.step-scaling-policy-step-adjustment-ranges-validated

Cause 5 of 5: PutWarmPool is accepted on a mixed-instances group that uses instance weights or Spot allocation.
AWS: PutWarmPool on a group whose MixedInstancesPolicy carries Overrides with WeightedCapacity, or an InstancesDistribution requesting Spot (OnDemandPercentageAboveBaseCapacity 0), is refused with HTTP 400 ValidationError and the fixed message "You can't add a warm pool to an Auto Scaling group that requests Spot Instances or uses a mixed instances policy with Spot allocation or instance weights. Warm pools support mixed instances policies only when they are configured for On-Demand Instances without instance weights." (no variable parts), and the group reports no WarmPoolConfiguration afterwards. Smithy: PutWarmPool models InstanceRefreshInProgress/LimitExceeded/ResourceContention only, so the refusal above is recorded behavior.
Emulator: both calls SUCCEED (HTTP 200).
Why: the client is handed a configuration AWS forbids, so warm-pool capacity planning built on weighted or Spot groups appears supported here and breaks on AWS. (Distinct from the open issue 20261113T064920Z-autoscaling-warm-pool-operations-diverge-phantom-warmed-instances.md, which covers phantom warmed instances and the not-found wording, not this eligibility rule.)
Fix: PutWarmPool handler validation against the group's MixedInstancesPolicy/InstancesDistribution.
Traces: autoscaling.warm-pool-refused-on-weighted-or-spot-mixed-instances-group

## Evidence
Cause 1 — expected (ValidationError, "Malformed launch template Id: lt-00000000000000000"); actual ValidationError with message 'The specified launch template, with template ID lt-00000000000000000, does not exist.' (AssertionError: expected the exact nonexistent-LaunchTemplateId refusal message).

Cause 2 — expected (ValidationError, "The DesiredCapacityType vcpu is only allowed in mixed instances groups containing InstanceRequirements. Please include a MixedInstancesPolicy with InstanceRequirements for your group and try again."); actual: HTTP 200, group created. AssertionError: the call succeeded; real AWS rejects it, so there is no error code.

Cause 3 — expected (ValidationError, "Your account is not allowed to create tag aws:team") for Tags=[{Key: aws:team, Value: web}]; actual: HTTP 200, group created. AssertionError: the call succeeded; real AWS rejects it, so there is no error code.

Cause 4 — expected (ValidationError, "StepAdjustment intervals cannot overlap.") for steps [0,20)+[10,inf), (ValidationError, "StepAdjustment intervals cannot have gaps between them.") for [0,10)+[20,inf), (ValidationError, "Both lower and upper bounds of a StepAdjustment cannot be left unspecified.") for a single bound-less step; actual: all three PutScalingPolicy calls returned HTTP 200. AssertionError: the call succeeded; real AWS rejects it, so there is no error code.

Cause 5 — expected (ValidationError, "You can't add a warm pool to an Auto Scaling group that requests Spot Instances or uses a mixed instances policy with Spot allocation or instance weights. Warm pools support mixed instances policies only when they are configured for On-Demand Instances without instance weights.") for both the weighted-overrides group and the OnDemandPercentageAboveBaseCapacity=0 group; actual: both PutWarmPool calls returned HTTP 200. AssertionError: the call succeeded; real AWS rejects it, so there is no error code.

## Steps to Reproduce
Replay the behavioral suite and re-run the five listed checks.
1. create_auto_scaling_group(LaunchTemplate={LaunchTemplateId: 'lt-00000000000000000', Version: '$Latest'}, ...) — compare the ValidationError message to "Malformed launch template Id: lt-00000000000000000".
2. create_auto_scaling_group(DesiredCapacityType='vcpu', LaunchTemplate={LaunchTemplateId: <real lt>}, MinSize=0, MaxSize=4, VPCZoneIdentifier=<two subnets>) — expect ValidationError, observe 200.
3. create_auto_scaling_group(..., Tags=[{Key: 'aws:team', Value: 'web', PropagateAtLaunch: False}]) — expect ValidationError, observe 200.
4. put_scaling_policy(PolicyType='StepScaling', AdjustmentType='ChangeInCapacity', StepAdjustments=<overlapping / gapped / both-bounds-null>) — expect ValidationError each time, observe 200 and the policies listed by describe_policies.
5. put_warm_pool(AutoScalingGroupName=<mixed-instances group with WeightedCapacity overrides, then one with InstancesDistribution.OnDemandPercentageAboveBaseCapacity=0>, MinSize=1) — expect ValidationError, observe 200.
