Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.force-delete-keeps-the-group-in-delete-in-progress-until-instances-end autoscaling__ec2.group-placement-group-applies-to-launched-instances autoscaling__ec2.zero-percent-on-demand-launches-spot-instances autoscaling__ec2.instance-refresh-desired-configuration-keeps-the-template-name autoscaling__ec2.removing-a-group-subnet-moves-its-instance-to-the-remaining-subnet
Issue-Key: autoscaling__ec2.force-delete-keeps-the-group-in-delete-in-progress-until-instances-end|run#11|AssertionError|assert [g.get("Status") for g in groups] == ["Delete in progress"], (
Issue-Key: autoscaling__ec2.group-placement-group-applies-to-launched-instances|run#13|AssertionError|assert placement.get("GroupName") == pg, (
Issue-Key: autoscaling__ec2.zero-percent-on-demand-launches-spot-instances|run#9|AssertionError|assert instance.get("InstanceLifecycle") == "spot", (
Issue-Key: autoscaling__ec2.instance-refresh-desired-configuration-keeps-the-template-name|run#15|AssertionError|assert spec == {"LaunchTemplateId": lt["id"], "LaunchTemplateName": lt["name"], "Version": "2"}, (
Issue-Key: autoscaling__ec2.removing-a-group-subnet-moves-its-instance-to-the-remaining-subnet|_poll#11|Timeout|last = probe()
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__ec2: 5 net-new causes — ForceDelete drops the group instantly, PlacementGroup and Spot (0% On-Demand) are not applied on launch, the refreshed group's LaunchTemplateName is dropped, and a VPCZoneIdentifier change never rebalances

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAutoScalingGroup

## Description
Five net-new root causes on the Auto Scaling + EC2 launch/teardown path. None of these checks is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 5: DeleteAutoScalingGroup(ForceDelete=True) removes the group from DescribeAutoScalingGroups immediately, while its instances are still running.
AWS: ForceDelete starts the delete and terminates the members; until the instances are gone, DescribeAutoScalingGroups still returns the group with Status "Delete in progress" (the AutoScalingGroup.Status member exists exactly for this transient state). Only after the instances end does the group disappear. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): DeleteAutoScalingGroup{AutoScalingGroupName, ForceDelete}; DescribeAutoScalingGroups projects AutoScalingGroup.Status — the "Delete in progress" value itself is recorded behavior.
Emulator: the group describes as [] the moment ForceDelete returns, with a member instance still running.
Why: the documented way to wait for a group to drain is to poll DescribeAutoScalingGroups until it is empty. Here it is empty while instances still exist, so cleanup/orchestration code proceeds to delete the subnets, security groups or VPC and then fails on dependency violations, and a client can never observe the delete-in-progress state.
Fix: DeleteAutoScalingGroup handler + group state machine — keep the group in state Delete in progress and project it from DescribeAutoScalingGroups until its instances have terminated.
Traces: autoscaling__ec2.force-delete-keeps-the-group-in-delete-in-progress-until-instances-end

Cause 2 of 5: the group's PlacementGroup is not applied to launched instances.
AWS: when a group carries PlacementGroup=<name> (Smithy: CreateAutoScalingGroup.PlacementGroup, XmlStringMaxLen255), every instance Auto Scaling launches is placed in it, so EC2 DescribeInstances reports Placement.GroupName == <that name>.
Emulator: the launched instance's Placement.GroupName is the empty string — the group's PlacementGroup is stored but never passed to the launch.
Why: a client verifying placement (cluster placement for low-latency workloads) sees instances outside the placement group, and any placement-dependent assertion or capacity assumption is wrong.
Fix: the launch path that builds the RunInstances request from the group's configuration (propagate the group's PlacementGroup into Placement.GroupName).
Traces: autoscaling__ec2.group-placement-group-applies-to-launched-instances

Cause 3 of 5: a mixed-instances policy with OnDemandPercentageAboveBaseCapacity=0 launches On-Demand instances, not Spot.
AWS: with InstancesDistribution.OnDemandPercentageAboveBaseCapacity=0 and no On-Demand base capacity, all capacity above the base is Spot, so EC2 DescribeInstances reports InstanceLifecycle == "spot" for the launched instance.
Emulator: the instance is launched with no InstanceLifecycle at all (None), i.e. as an On-Demand instance.
Why: the Spot/On-Demand split is the entire point of a mixed-instances policy; a client testing Spot behavior (interruption handling, capacity-rebalance, cost assumptions) is silently given On-Demand instances, and any filter or branch on InstanceLifecycle=spot finds nothing.
Fix: the mixed-instances launch path — honour InstancesDistribution when choosing the purchase option and set the instance's lifecycle to spot.
Traces: autoscaling__ec2.zero-percent-on-demand-launches-spot-instances

Cause 4 of 5: after an instance refresh with a DesiredConfiguration, the group's LaunchTemplate projection loses LaunchTemplateName.
AWS: StartInstanceRefresh with DesiredConfiguration.LaunchTemplate applies that specification to the group, and DescribeAutoScalingGroups then projects the group's LaunchTemplate with all three members — LaunchTemplateId, LaunchTemplateName and Version (Smithy: LaunchTemplateSpecification carries LaunchTemplateId, LaunchTemplateName, Version).
Emulator: the group's LaunchTemplate after the refresh is {LaunchTemplateId, Version} only — LaunchTemplateName is absent.
Why: clients and tools (CloudFormation drift, SDK round-trips, IaC reconciliation) read LaunchTemplateName from the group; a missing member is a KeyError or an unnecessary re-resolution by id. Related but distinct from the open issue 20261124T194609Z-autoscaling-describeautoscalinggroups-mis-projects-the-group-s-launch.md, which is about the projected version/mixed-instances policy, not this member being dropped on the refresh path.
Fix: the instance-refresh DesiredConfiguration state update plus the DescribeAutoScalingGroups serializer for LaunchTemplateSpecification.
Traces: autoscaling__ec2.instance-refresh-desired-configuration-keeps-the-template-name

Cause 5 of 5: replacing the group's only subnet in VPCZoneIdentifier never moves the instance.
AWS: UpdateAutoScalingGroup replacing VPCZoneIdentifier with a subnet in another zone makes the existing instance's zone no longer part of the group; Auto Scaling rebalances by terminating that instance and launching a replacement in the new subnet's zone, so the group ends with exactly one InService instance in the new zone and EC2 reports it in the new subnet.
Emulator: no rebalance is observable — the check makes no progress to that state and ends at its replay budget (60 s) while polling the group's instances. The group keeps the instance in the removed zone.
Why: AZ rebalancing is how a client migrates a group between subnets/zones with zero manual intervention; without it the group permanently holds capacity in a zone it no longer declares, and any migration scenario hangs instead of converging.
Fix: the group's rebalance/zone-reconciliation logic invoked after UpdateAutoScalingGroup changes VPCZoneIdentifier (terminate out-of-zone members and launch replacements in the declared subnets).
Traces: autoscaling__ec2.removing-a-group-subnet-moves-its-instance-to-the-remaining-subnet

## Evidence
Cause 1 — expected [g['Status'] for g in DescribeAutoScalingGroups] == ['Delete in progress'] right after ForceDelete; actual AssertionError: right after ForceDelete with instance i-0365ff4a86c0ddb42 running the group describes as [] (the instance id is a per-run value).

Cause 2 — expected DescribeInstances Placement.GroupName == the group's PlacementGroup; actual AssertionError: the group's instance is placed in '', not placement group 'sim2real-asgbb-pg-6335e0cad602' (the name is a per-run value).

Cause 3 — expected InstanceLifecycle == 'spot'; actual AssertionError: 0% On-Demand launched i-02f552ca481cafa0d with InstanceLifecycle None, not a Spot Instance.

Cause 4 — expected the group's LaunchTemplate == {'LaunchTemplateId': <id>, 'LaunchTemplateName': <name>, 'Version': '2'}; actual AssertionError: after the refresh the group's LaunchTemplate is {'LaunchTemplateId': 'lt-000000000000000b6', 'Version': '2'} — LaunchTemplateName missing.

Cause 5 — expected the group's instances to become [(<new zone>, 'InService')] with a new instance id in the new subnet; actual: Check timed out after 60.0s with no such transition observable.

## Steps to Reproduce
Re-replay the five listed checks.
1. Create a group with DesiredCapacity 1, wait for the instance, call delete_auto_scaling_group(ForceDelete=True) and immediately call describe_auto_scaling_groups(AutoScalingGroupNames=[group]) — expect one group with Status 'Delete in progress' while the instance runs.
2. Create a placement group, create an ASG with PlacementGroup=<name> and DesiredCapacity 1, then ec2.describe_instances on the member and read Placement.GroupName.
3. Create an ASG with MixedInstancesPolicy InstancesDistribution.OnDemandPercentageAboveBaseCapacity=0, DesiredCapacity 1, then read the member's InstanceLifecycle.
4. Create an ASG from a launch template, add version 2, start_instance_refresh(DesiredConfiguration={'LaunchTemplate': {...Version '2'}}), then describe_auto_scaling_groups and inspect LaunchTemplate.
5. Create an ASG with MinSize/DesiredCapacity 1 in subnet A only, wait for InService, update_auto_scaling_group(VPCZoneIdentifier=<subnet B in another zone>), and poll for the instance to be replaced in B's zone.
