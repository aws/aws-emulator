Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected
Issue-Key: autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected|run#10|KeyError|good_pool = asgc.describe_warm_pool(AutoScalingGroupName=good_asg)["WarmPoolConfiguration"]
Issue-Commit: 4370547da342f3cac1e3d2a524b52e751b2d7b18

# autoscaling__cloudformation: a CREATE_COMPLETE stack declaring AWS::AutoScaling::WarmPool leaves the group with no warm pool — DescribeWarmPool returns no WarmPoolConfiguration

> _Note: this issue was identified against the code base at commit `4370547da342f3cac1e3d2a524b52e751b2d7b18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWarmPool

## Description
Cause 1 of 1: a CloudFormation stack whose template contains an AWS::AutoScaling::WarmPool resource reaches CREATE_COMPLETE, but no warm pool is actually configured on the Auto Scaling group the same stack created.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): PutWarmPool input {AutoScalingGroupName required, MinSize @range min 0, MaxGroupPreparedCapacity @range min -1, PoolState WarmPoolState enum, InstanceReusePolicy}, empty output; DescribeWarmPool output {WarmPoolConfiguration (MaxGroupPreparedCapacity, MinSize, PoolState, Status, InstanceReusePolicy), Instances, NextToken}. CloudFormation's AWS::AutoScaling::WarmPool resource is backed by that API: a stack that creates the group and the warm pool only reports CREATE_COMPLETE once the pool exists, after which DescribeWarmPool on that group answers 200 WITH a WarmPoolConfiguration whose PoolState / MinSize / MaxGroupPreparedCapacity are the values the template declared (recorded behavior for the read-back).
Emulator: the stack reaches CREATE_COMPLETE and DescribeAutoScalingGroups finds the group with the name the stack's AsgRef output reports, but DescribeWarmPool for that group returns a response with NO WarmPoolConfiguration member at all — reading it raises KeyError: 'WarmPoolConfiguration'. The stack therefore claims to have provisioned a resource it never created; the WarmPool resource is accepted and ignored.
Why: a stack is the contract that its declared resources exist. A client that deploys a warm pool through infrastructure-as-code and then relies on it (pool depth, PoolState, instance-reuse policy, or simply a verification read after deploy) gets a group with no pool and no error anywhere to explain it — and a template-driven negative test cannot distinguish 'the pool was refused' from 'the pool was silently dropped', because both read back as absent. It also makes the stack's declared PoolState unverifiable, so an out-of-enum PoolState cannot be shown to be refused by the resource handler.
Fix: the CloudFormation resource handler for AWS::AutoScaling::WarmPool in the emulator's CFN resource provisioning — it must call through to the autoscaling warm-pool state (PutWarmPool semantics) with the template's AutoScalingGroupName / MinSize / MaxGroupPreparedCapacity / PoolState, fail the resource when those values are invalid, and make the pool readable through DescribeWarmPool afterwards.
Traces: autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected.

Related open filing that does NOT carry this check and therefore does not cover it: 20261109T172356Z-autoscaling-cloudformation-2-net-new-causes-describestacks-outputs.md (Outputs entries with no OutputValue and a stack-created AWS::AutoScaling::AutoScalingGroup). The replay marked this cluster neither OWNED nor ALREADY FILED.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator.

[error] autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected:
  File "check_autoscaling__cloudformation_deploy_asg_warmpool_poolstate_invalid_enum_rejected.py", line 236, in run
  good_pool = asgc.describe_warm_pool(AutoScalingGroupName=good_asg)["WarmPoolConfiguration"]
  KeyError: 'WarmPoolConfiguration'

The failure is in the check's POSITIVE control, after the preceding assertions passed: the stack (AWS::EC2::LaunchTemplate + AWS::AutoScaling::AutoScalingGroup + AWS::AutoScaling::WarmPool with MinSize 0, MaxGroupPreparedCapacity 0, PoolState 'Stopped') reached CREATE_COMPLETE and DescribeAutoScalingGroups returned the group named by the stack's AsgRef output. The next read expects ('Stopped', 0, 0) from DescribeWarmPool's WarmPoolConfiguration and finds the member absent.

Because the control never establishes a pool, the check's negative half — the same template with PoolState 'Terminated' must ROLLBACK_COMPLETE with the handler reason "Invalid request provided: 1 validation error detected: Value 'Terminated' at 'poolState' failed to satisfy constraint: Member must satisfy enum value set: [Stopped, Hibernated, Running] (Service: AutoScaling, Status Code: 400, ...)" / HandlerErrorCode 'InvalidRequest' — was never reached. Stack names and the ASG name are generated per run.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay the behavioral suite; autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected reproduces.
2. CreateStack with a template containing an AWS::EC2::LaunchTemplate, an AWS::AutoScaling::AutoScalingGroup (MinSize 0, MaxSize 1, DesiredCapacity 0, VPCZoneIdentifier a subnet, LaunchTemplate referencing the template) and an AWS::AutoScaling::WarmPool with Properties {AutoScalingGroupName: {Ref: ASG}, MinSize: 0, MaxGroupPreparedCapacity: 0, PoolState: 'Stopped'}. Wait for CREATE_COMPLETE.
3. Call autoscaling DescribeWarmPool with that group name. Expect 200 with WarmPoolConfiguration = PoolState 'Stopped', MinSize 0, MaxGroupPreparedCapacity 0; the emulator returns a response with no WarmPoolConfiguration member.
4. Deploy the same template with PoolState 'Terminated' and confirm the stack rolls back with the WarmPool resource's handler reason naming the enum value set [Stopped, Hibernated, Running] and HandlerErrorCode 'InvalidRequest'.
5. Re-replay autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected to confirm the fix.
