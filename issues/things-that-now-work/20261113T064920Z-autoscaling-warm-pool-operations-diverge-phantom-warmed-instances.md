Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group autoscaling-warm-pool-and-instance-refresh-negative autoscaling-warm-pool-defaults-then-deleted
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|PutWarmPool|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeWarmPool|2xx->2xx|$.DescribeWarmPoolResponse.DescribeWarmPoolResult.Instances
Issue-Key: autoscaling|DescribeWarmPool|2xx->2xx|$.DescribeWarmPoolResponse.DescribeWarmPoolResult.WarmPoolConfiguration
Issue-Commit: f7067607673c78bda9a2a88ab2c9b41127d89615

# autoscaling warm-pool operations diverge: phantom warmed instances, a deleted warm pool still reported, and a re-worded PutWarmPool not-found refusal

> _Note: this issue was identified against the code base at commit `f7067607673c78bda9a2a88ab2c9b41127d89615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWarmPool

## Description
Three net-new wire causes on the Auto Scaling warm-pool surface (Cause 1 of 3 .. Cause 3 of 3). All three must be closed before this issue is submitted; each names its own fixture and step.

Related open filings that do NOT carry these replay keys and therefore do not cover these failures: 20261107T081710Z-autoscaling-auto-scaling-not-found-deletion-protection-and-instance.md and 20261110T100847Z-autoscaling-refusal-fidelity-wrong-codes-and-emulator-authored.md (adjacent refusal-text work on other operations) and 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md. The replay marked none of the clusters below OWNED.

Cause 1 of 3: PutWarmPool's group-not-found refusal drops the group name AWS reports.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) PutWarmPool input {AutoScalingGroupName (required, XmlStringMaxLen255, length 1-255), MaxGroupPreparedCapacity (min -1), MinSize (min 0), PoolState (WarmPoolState enum), InstanceReusePolicy}, empty output, modeled errors LimitExceededFault / InstanceRefreshInProgressFault / ResourceContentionFault; the not-found condition is the awsQuery common ValidationError (recorded behavior for the text). For a group that does not exist AWS answers HTTP 400 / Error.Code 'ValidationError' with the message 'AutoScalingGroup name not found - No such Auto Scaling group: <AutoScalingGroupName>' — 'AutoScalingGroup name not found - No such Auto Scaling group: ' is AWS's fixed text and the trailing value is the AutoScalingGroupName taken verbatim from the request.
Emulator: same status and code, but the message 'AutoScalingGroup name not found - null' — the variable half is rendered as the literal 'null' instead of the requested group name.
Why: the refusal no longer says WHICH group was missing, so a client issuing warm-pool configuration across many groups (or a CLI user reading the error) cannot tell which name was wrong; any handler that parses the group name out of the message gets 'null'.
Fix: the not-found refusal in the autoscaling PutWarmPool handler's validation — interpolate the request's AutoScalingGroupName into AWS's sentence instead of emitting 'null'.
Traces: autoscaling-group-and-policy-dependent-operations-on-nonexistent-group#task#3 (also autoscaling-warm-pool-and-instance-refresh-negative#task#1).

Cause 2 of 3: DescribeWarmPool invents a warmed instance for a warm pool that has none.
AWS: Smithy DescribeWarmPool -> {WarmPoolConfiguration, Instances (list), NextToken}, paginated over Instances. A warm pool created with the defaults (MinSize 0, PoolState Stopped) and no prepared capacity contains no instances, so AWS returns the configuration with an EMPTY Instances list; an instance appears only once the pool actually prepares one, and then its LifecycleState is a Warmed:* value for an instance that exists in EC2.
Emulator: DescribeWarmPool returns one Instances member synthesized out of the group's launch template (InstanceId, InstanceType, AvailabilityZone, LifecycleState 'Warmed:Pending' and later 'Warmed:Stopped', HealthStatus 'Healthy', LaunchTemplate, ImageId, ProtectedFromScaleIn 'false') although the pool was configured with MinSize 0 and never prepared anything.
Why: a client reading warm-pool depth sees capacity that does not exist — pool-size assertions, cost/capacity accounting and scale-from-warm-pool logic all act on a phantom instance, and a pool that is genuinely empty is indistinguishable from a populated one.
Fix: the warm-pool state behind DescribeWarmPool in the autoscaling service — derive Instances from instances the pool actually prepared (governed by MinSize/MaxGroupPreparedCapacity) rather than projecting a template-derived placeholder.
Traces: autoscaling-warm-pool-defaults-then-deleted#task#6.

Cause 3 of 3: DeleteWarmPool does not remove the warm pool — DescribeWarmPool keeps reporting its configuration indefinitely.
AWS: Smithy DeleteWarmPool input {AutoScalingGroupName (required), ForceDelete}, empty output, errors LimitExceededFault / ResourceContentionFault / ResourceInUseFault / ScalingActivityInProgressFault; DescribeWarmPool -> {WarmPoolConfiguration, Instances, NextToken}. After a successful DeleteWarmPool the group has no warm pool, so DescribeWarmPool answers 200 with NO WarmPoolConfiguration member and no Instances.
Emulator: after DeleteWarmPool, DescribeWarmPool still returns WarmPoolConfiguration (MinSize/PoolState as configured) together with a warmed instance member, and it is still present after the fixture's full poll budget (40 attempts), so this is not eventual consistency but a deletion that never takes effect.
Why: the warm pool cannot be torn down through the API — a client that deletes a pool and then verifies, or deletes and recreates with different settings, reads the stale pool forever; group deletion and cleanup flows that require the pool to be gone never make progress.
Fix: the DeleteWarmPool handler and the group's warm-pool state in the autoscaling service — clear the stored warm-pool configuration and its instances so DescribeWarmPool omits WarmPoolConfiguration afterwards.
Traces: autoscaling-warm-pool-defaults-then-deleted#task#8.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true): 716/1044 steps passed (68.6%), 78 divergence clusters. These three clusters carry no owning issue and no stub.

Cause 1 — fixture autoscaling-group-and-policy-dependent-operations-on-nonexistent-group, step task#3, operation PutWarmPool, verdict mismatch (expected 400, got 400):
  $.ErrorResponse.Error.Message: value_mismatch (expected 'AutoScalingGroup name not found - No such Auto Scaling group: sim2real-probe-autoscaling-asg-f393fafd', got 'AutoScalingGroup name not found - null')
  affected also: autoscaling-warm-pool-and-instance-refresh-negative#task#1. The group name shown is that run's value; the requirement is that the request's AutoScalingGroupName appears after AWS's fixed prefix.

Cause 2 — fixture autoscaling-warm-pool-defaults-then-deleted, step task#6, operation DescribeWarmPool, verdict mismatch (200 vs 200):
  $.DescribeWarmPoolResponse.DescribeWarmPoolResult.Instances: type_mismatch (expected '' (empty), got {'member': {'InstanceId': 'i-00000000000000001', 'InstanceType': 't3.micro', 'AvailabilityZone': 'us-west-2a', 'LifecycleState': 'Warmed:Pending', 'HealthStatus': 'Healthy', 'LaunchTemplate': {...}, 'ImageId': 'ami-...', 'ProtectedFromScaleIn': 'false'}})
  The instance/template/AMI ids are generated per run; the requirement is that an unprepared pool reports no Instances.

Cause 3 — same fixture, step task#8, operation DescribeWarmPool, verdict mismatch (200 vs 200), after DeleteWarmPool:
  $.DescribeWarmPoolResponse.DescribeWarmPoolResult.WarmPoolConfiguration: unexpected_present (expected None, got {'MinSize': '0', 'PoolState': 'Stopped'})
  $.DescribeWarmPoolResponse.DescribeWarmPoolResult.WarmPoolConfiguration: poll: still present after 40 attempt(s)
  $.DescribeWarmPoolResponse.DescribeWarmPoolResult.Instances: type_mismatch (expected '' (empty), got {'member': {... 'LifecycleState': 'Warmed:Stopped' ...}})

Smithy consulted: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json — PutWarmPool input {AutoScalingGroupName required, MaxGroupPreparedCapacity range min -1, MinSize range min 0, PoolState enum, InstanceReusePolicy}, errors InstanceRefreshInProgressFault/LimitExceededFault/ResourceContentionFault; DescribeWarmPool output {WarmPoolConfiguration, Instances, NextToken} paginated on Instances; DeleteWarmPool input {AutoScalingGroupName required, ForceDelete}, errors LimitExceededFault/ResourceContentionFault/ResourceInUseFault/ScalingActivityInProgressFault.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay trace set simparity-wire-aws-v1; the fixtures/steps named under Traces reproduce.
2. Cause 1: call autoscaling PutWarmPool with an AutoScalingGroupName that does not exist. Expect HTTP 400 / ValidationError with message 'AutoScalingGroup name not found - No such Auto Scaling group: <that name>'; the emulator answers '... - null'.
3. Cause 2: create a launch-template Auto Scaling group, call PutWarmPool with the defaults (no MinSize / PoolState overrides), then DescribeWarmPool. Expect the configuration with an EMPTY Instances list; the emulator reports one synthesized Warmed:Pending instance.
4. Cause 3: call DeleteWarmPool for that group, then poll DescribeWarmPool. Expect 200 with no WarmPoolConfiguration and no Instances; the emulator keeps returning {'MinSize': '0', 'PoolState': 'Stopped'} plus a Warmed:Stopped instance for the whole poll budget.
5. Re-replay autoscaling-group-and-policy-dependent-operations-on-nonexistent-group#task#3 and autoscaling-warm-pool-defaults-then-deleted#task#6 and #task#8 to confirm the fix.
