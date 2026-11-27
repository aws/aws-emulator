Issue-Source: trace-sweep-divergence
Issue-Trace: asglf-mixed-instances-policy-refusals
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asglf-mixed-instances-policy-refusals autoscaling-group-tag-key-aws-prefix autoscaling-step-scaling-overlapping-range-adjustments autoscaling-target-tracking-resource-label-unattached-target-group autoscaling-warm-pool-weighted-mixed-instances-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|CreateAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeTags|2xx->2xx|$.DescribeTagsResponse.DescribeTagsResult.Tags
Issue-Key: autoscaling|DeleteAutoScalingGroup|4xx->2xx|status
Issue-Key: autoscaling|PutScalingPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|PutScalingPolicy|4xx->2xx|status
Issue-Key: autoscaling|DescribePolicies|2xx->2xx|$.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies
Issue-Key: autoscaling|PutWarmPool|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|PutWarmPool|4xx->2xx|status
Issue-Related: 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# autoscaling write-path validation gaps let invalid mixed-instances, step-policy, warm-pool and reserved-tag requests through (6 causes)

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Six net-new root causes on the autoscaling control plane, all reached by plain CreateAutoScalingGroup / PutScalingPolicy / PutWarmPool calls and their read-back. The shared fault is the write path: configurations AWS refuses at request time are accepted (HTTP 200), so the refusal never happens, the invalid configuration is stored, and the following read (DescribeTags / DescribePolicies) and teardown (DeleteAutoScalingGroup) then project state that cannot exist on AWS. None of these replay clusters is reported as blocked by a stub or OWNED by an open issue.

Related but NOT covering: the open behavioral issue 20261127T094414Z-autoscaling-5-net-new-causes-createautoscalinggroup-putscalingpolicy-putwarmpool.md describes the same permissiveness for the reserved aws: tag key, the step-adjustment intervals and the warm-pool eligibility rule, but it is grounded in behavioral check ids and carries none of the wire replay keys below, so these fixture steps are un-gated by it. 20261127T094414Z-autoscaling-ec2-elbv2-a-target-tracking-policy-whose.md likewise covers the unattached-target-group ResourceLabel refusal in behavioral vocabulary only.

Cause 1 of 6: CreateAutoScalingGroup does not validate MixedInstancesPolicy members, and the refusal that does come back is a name collision.
AWS: a CreateAutoScalingGroup whose MixedInstancesPolicy.InstancesDistribution carries an unsupported allocation strategy is refused with HTTP 400, awsQuery error Type 'Sender', Code 'ValidationError' and a message naming the offending member and its accepted values, e.g. "OnDemandAllocationStrategy is not valid. Valid options are: [prioritized, lowest-price]." — the fixed text is "<member> is not valid. Valid options are: [<list>]."; the variable parts are the member name taken from the request and that member's accepted-value list. Because the group is NOT created, a later create of the same name is still a fresh create, not a collision. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): CreateAutoScalingGroup.MixedInstancesPolicy -> MixedInstancesPolicy.InstancesDistribution.OnDemandAllocationStrategy is a plain XmlString with no enum, and the operation models only AlreadyExists/LimitExceeded/ResourceContention/ServiceLinkedRoleFailure, so ValidationError is awsQuery's common sender error and the value enumeration plus the message text are recorded behavior only.
Emulator: the status matches (400) but the body is the wrong refusal — Code 'AlreadyExists' with message "AutoScalingGroup by this name already exists - A group with the name <group name> already exists". The emulator performs no MixedInstancesPolicy validation at all, so the first malformed create in the sequence was accepted with 200 and every subsequent malformed create of that name is answered as a duplicate name.
Why: a client validating its mixed-instances configuration is told its NAME is taken rather than that its policy is invalid, so it renames and retries forever while the invalid configuration it meant to have rejected is already live; any code branching on ValidationError vs AlreadyExists takes the wrong branch.
Fix: CreateAutoScalingGroup input validation — validate MixedInstancesPolicy (InstancesDistribution allocation strategies, Overrides) before the name-collision check in the operation's handler.
Traces: asglf-mixed-instances-policy-refusals#task#9, and the same cluster's task#10, task#11, task#13.

Cause 2 of 6: DescribeTags lists group tags, including one whose key uses the reserved "aws:" prefix, for a group AWS never created.
AWS: a group tag key beginning with the reserved "aws:" prefix is refused at create with HTTP 400 ValidationError ("Your account is not allowed to create tag <the tag key from the request>" — fixed text plus the offending key), so neither the group nor any of its tags exists and DescribeTags answers 200 with an EMPTY Tags list. Smithy: DescribeTags returns Tags: TagDescriptionList, and TagDescription.Key is a TagKey (length 1..128) with no prefix constraint — the reserved-prefix rule is not modeled, so this is recorded behavior only.
Emulator: DescribeTags answers 200 with two Tags members for the group — Key 'aws:team' and Key 'team' — i.e. the client-supplied reserved-namespace tag is stored and projected as a user tag.
Why: "aws:" is the namespace the service itself writes (aws:autoscaling:groupName). A caller can shadow service-managed tags, and tag-governance checks that rely on the refusal pass here and fail on AWS.
Fix: tag-key validation shared by CreateAutoScalingGroup and CreateOrUpdateTags (reject the reserved prefix before the group/tags are stored); the DescribeTags projection then follows from state.
Traces: autoscaling-group-tag-key-aws-prefix#task#8.

Cause 3 of 6: DeleteAutoScalingGroup succeeds on a group that AWS never created.
AWS: the group creation in this fixture is refused (Cause 2), so the teardown DeleteAutoScalingGroup for that name is refused with HTTP 400 — AWS's refusal for an absent group is ValidationError with the group-not-found wording ("AutoScalingGroup name not found - AutoScalingGroup <name> not found"); the recorded step asserts the 400 status. Smithy: DeleteAutoScalingGroup models only ResourceInUse/ScalingActivityInProgress/ResourceContention, so the not-found refusal is awsQuery's common ValidationError and is recorded behavior.
Emulator: answers HTTP 200 and deletes the phantom group.
Why: a client cleaning up cannot distinguish "deleted" from "never existed", so a negative teardown assertion passes here and fails on AWS; it is the direct consequence of the accepted create.
Fix: the same create-path validation as Cause 2 (so no group exists), and the group-lookup refusal in the DeleteAutoScalingGroup handler.
Traces: autoscaling-group-tag-key-aws-prefix#teardown#9.

Cause 4 of 6: PutScalingPolicy accepts a step-scaling policy whose StepAdjustment intervals are not a valid partition, and a target-tracking policy whose ResourceLabel names an unattached target group.
AWS: a StepScaling policy whose StepAdjustments overlap is refused with HTTP 400, Type 'Sender', Code 'ValidationError' and the fixed message "StepAdjustment intervals cannot overlap." (no variable parts). The same cluster's other fixture records the sibling refusal for a target-tracking policy whose ALBRequestCountPerTarget ResourceLabel names a target group not attached to the group. Smithy: PutScalingPolicy.StepAdjustments -> StepAdjustment{MetricIntervalLowerBound, MetricIntervalUpperBound, ScalingAdjustment}; the interval-partition rule is not modeled and PutScalingPolicy models only LimitExceeded/ResourceContention/ServiceLinkedRoleFailure, so the status/code/text are recorded behavior only.
Emulator: answers HTTP 200 with a PolicyARN and stores the policy; no ErrorResponse is produced.
Why: a step set with overlapping intervals has no deterministic step for a breach value, so the client believes it configured valid scaling and gets undefined step selection; the configuration-time refusal AWS uses as the guard rail is missing.
Fix: PutScalingPolicy input validation for PolicyType StepScaling (interval partition) and for TargetTrackingConfiguration.PredefinedMetricSpecification.ResourceLabel (target group attached to the group).
Traces: autoscaling-step-scaling-overlapping-range-adjustments#task#8, and the same cluster's autoscaling-target-tracking-resource-label-unattached-target-group#task#8.

Cause 5 of 6: DescribePolicies lists the scaling policy AWS refused to create.
AWS: because the PutScalingPolicy of Cause 4 is refused, the group has no policies and DescribePolicies answers 200 with an EMPTY ScalingPolicies list. Smithy: DescribePolicies returns ScalingPolicies: ScalingPolicies (paginated on NextToken/MaxRecords).
Emulator: answers 200 with a ScalingPolicies member describing the refused policy, including its StepAdjustments (the overlapping pair) and Enabled 'true'.
Why: the invalid policy is not merely accepted, it is discoverable, so a client auditing a group's policies sees scaling configuration AWS would never hold and may act on it (execute it, copy it to another group).
Fix: follows from Cause 4 — the policy must never enter the group's policy state; the DescribePolicies projection reads that state.
Traces: autoscaling-step-scaling-overlapping-range-adjustments#task#9, and the same cluster's autoscaling-target-tracking-resource-label-unattached-target-group#task#9.

Cause 6 of 6: PutWarmPool is accepted on a mixed-instances group that uses instance weights / Spot allocation.
AWS: PutWarmPool on a group whose MixedInstancesPolicy carries weighted Overrides, or an InstancesDistribution requesting Spot, is refused with HTTP 400, Type 'Sender', Code 'ValidationError' and the fixed message "You can't add a warm pool to an Auto Scaling group that requests Spot Instances or uses a mixed instances policy with Spot allocation or instance weights. Warm pools support mixed instances policies only when they are configured for On-Demand Instances without instance weights." (no variable parts), and the group keeps no warm-pool configuration. Smithy: PutWarmPool models only InstanceRefreshInProgress/LimitExceeded/ResourceContention, so this eligibility refusal is awsQuery's common ValidationError and is recorded behavior only.
Emulator: answers HTTP 200 and configures the warm pool; no ErrorResponse is produced.
Why: the client is handed a configuration AWS forbids, so warm-pool capacity planning on weighted or Spot groups appears supported here and breaks on AWS. Distinct from 20261113T064920Z-autoscaling-warm-pool-operations-diverge-phantom-warmed-instances.md, which covers phantom warmed instances and the not-found wording, not this eligibility rule.
Fix: PutWarmPool handler validation against the target group's MixedInstancesPolicy Overrides (WeightedCapacity) and InstancesDistribution (Spot allocation / OnDemandPercentageAboveBaseCapacity).
Traces: autoscaling-warm-pool-weighted-mixed-instances-group#task#8.

## Evidence
All six are mismatch clusters from the simparity-wire-aws-v1 replay (score 94.3%, 66 mismatched steps); none carries a stub_path or an owned_by.

Cause 1 — fixture asglf-mixed-instances-policy-refusals step task#9 (CreateAutoScalingGroup), expected 400 / actual 400 but: $.ErrorResponse.Error.Code expected 'ValidationError', got 'AlreadyExists'; $.ErrorResponse.Error.Message expected 'OnDemandAllocationStrategy is not valid. Valid options are: [prioritized, lowest-price].', got 'AutoScalingGroup by this name already exists - A group with the name <group> already exists'. Cluster also covers task#10, task#11, task#13 of that fixture.

Cause 2 — fixture autoscaling-group-tag-key-aws-prefix step task#8 (DescribeTags), 200/200: $.DescribeTagsResponse.DescribeTagsResult.Tags expected '' (empty), got {'member': [{Key: 'aws:team', Value: 'web', ResourceType: 'auto-scaling-group', PropagateAtLaunch: 'false'}, {Key: 'team', Value: 'web', ...}]}.

Cause 3 — fixture autoscaling-group-tag-key-aws-prefix step teardown#9 (DeleteAutoScalingGroup): status expected 400, got 200.

Cause 4 — fixture autoscaling-step-scaling-overlapping-range-adjustments step task#8 (PutScalingPolicy): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'StepAdjustment intervals cannot overlap.'}}, got None. Same cluster: autoscaling-target-tracking-resource-label-unattached-target-group#task#8.

Cause 5 — fixture autoscaling-step-scaling-overlapping-range-adjustments step task#9 (DescribePolicies), 200/200: $.DescribePoliciesResponse.DescribePoliciesResult.ScalingPolicies expected '' (empty), got a member with PolicyType 'StepScaling' and StepAdjustments [{MetricIntervalLowerBound 0.0, MetricIntervalUpperBound 20.0, ScalingAdjustment 1}, {MetricIntervalLowerBound 10.0, ScalingAdjustment 2}] — the overlapping pair. Same cluster: autoscaling-target-tracking-resource-label-unattached-target-group#task#9.

Cause 6 — fixture autoscaling-warm-pool-weighted-mixed-instances-group step task#8 (PutWarmPool): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': "You can't add a warm pool to an Auto Scaling group that requests Spot Instances or uses a mixed instances policy with Spot allocation or instance weights. Warm pools support mixed instances policies only when they are configured for On-Demand Instances without instance weights."}}, got None.

(Group names, policy names, ARNs and timestamps in the recordings are per-run values; the requirement is the status, the error code and the message template, not those literals.)

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator and re-run these fixtures/steps:
1. asglf-mixed-instances-policy-refusals — steps task#9 (representative), task#10, task#11, task#13: CreateAutoScalingGroup with a MixedInstancesPolicy whose InstancesDistribution carries an unsupported allocation strategy. Expect 400 ValidationError naming the invalid member; observe 400 AlreadyExists (and note the earlier create in the same fixture was accepted with 200).
2. autoscaling-group-tag-key-aws-prefix — task#8: DescribeTags after a create carrying Tags=[{Key:'aws:<suffix>'}]. Expect an empty Tags list (the create is refused); observe the aws:-prefixed tag listed.
3. autoscaling-group-tag-key-aws-prefix — teardown#9: DeleteAutoScalingGroup for that name. Expect 400; observe 200.
4. autoscaling-step-scaling-overlapping-range-adjustments — task#8: PutScalingPolicy PolicyType='StepScaling' with StepAdjustments [0,20) and [10,inf). Expect 400 ValidationError 'StepAdjustment intervals cannot overlap.'; observe 200. Same cluster: autoscaling-target-tracking-resource-label-unattached-target-group#task#8.
5. autoscaling-step-scaling-overlapping-range-adjustments — task#9: DescribePolicies for that group. Expect an empty ScalingPolicies list; observe the refused policy listed. Same cluster: autoscaling-target-tracking-resource-label-unattached-target-group#task#9.
6. autoscaling-warm-pool-weighted-mixed-instances-group — task#8: PutWarmPool on a mixed-instances group with weighted Overrides / Spot allocation. Expect 400 ValidationError with the warm-pool eligibility message; observe 200.
