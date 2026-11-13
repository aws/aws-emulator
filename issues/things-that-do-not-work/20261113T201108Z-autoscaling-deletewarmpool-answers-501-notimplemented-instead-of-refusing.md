Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.delete-warm-pool-in-use-without-force-rejected autoscaling.start-instance-refresh-during-warm-pool-deletion-blocked
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.delete-warm-pool-in-use-without-force-rejected|run#17|AssertionError|assert code == "ResourceInUse", f"expected ResourceInUse, got {code!r}"
Issue-Key: autoscaling.start-instance-refresh-during-warm-pool-deletion-blocked|run#18|AssertionError|assert code == "ResourceInUse", f"unexpected code={code!r}"
Issue-Commit: 751e4cd59e0742391b3a649cdf34c459ab8203ec

# autoscaling DeleteWarmPool answers 501 NotImplemented instead of refusing an in-use warm pool with ResourceInUse (and instead of accepting a ForceDelete teardown)

> _Note: this issue was identified against the code base at commit `751e4cd59e0742391b3a649cdf34c459ab8203ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWarmPool

## Description
One root cause, grounded in two failing behavioral checks that the replay report did NOT mark as covered (both are verdict 'mismatch': they catch the error and assert on its code, so the suite's scaffold-stub marker did not claim them). The open issue 20261113T130916Z-autoscaling-deletewarmpool-stub.md names the same operation and handler file; this issue records the refusal contract the two checks assert and carries their replay keys so a fixer can re-replay them. It should be closed together with that stub issue, not as a second implementation task.

Cause 1 of 1: DeleteWarmPool is an unimplemented scaffold and answers HTTP 501 `NotImplemented` for every request, so neither its refusal nor its success path exists.
AWS: Smithy (com.amazonaws.autoscaling#DeleteWarmPool): input AutoScalingGroupName (XmlStringMaxLen255, required) + ForceDelete (ForceDelete, "Specifies that the warm pool is to be deleted along with all of its associated instances, without waiting for all instances to be terminated"); modeled errors LimitExceededFault (400), ResourceContentionFault (500), ResourceInUseFault (400, "The operation can't be performed because the resource is in use"), ScalingActivityInProgressFault (400). Recorded behavior: calling DeleteWarmPool WITHOUT ForceDelete while the pool still holds instances is refused with HTTP 400, awsQuery error code `ResourceInUse` (the wire code drops the modeled `Fault` suffix) and the message "You cannot delete a warm pool while there are instances still in it unless you use the force delete option." — that message is entirely fixed text, it has no variable parts. The pool and its instances survive the refusal. With ForceDelete=true the call is accepted (HTTP 200, empty DeleteWarmPoolResult) and the teardown is asynchronous: the pool drains to zero instances afterwards, and a StartInstanceRefresh issued on the same group during that teardown is accepted rather than refused as a resource conflict. Transiently AWS may answer `ScalingActivityInProgress` or `ResourceContention` while instances settle; the steady-state answer is the ResourceInUse refusal above.
Emulator: every DeleteWarmPool call — with or without ForceDelete — answers HTTP 501 with error code `NotImplemented` and message "operation DeleteWarmPool is scaffolded but not implemented in this emulator". Both checks fail on `code == 'NotImplemented'` where they expect `'ResourceInUse'`, and the ForceDelete branch (and the concurrent StartInstanceRefresh it guards) is never reached.
Why: a client that deletes a warm pool sees a 501 that no AWS SDK retry/branch logic models — it is neither the documented safety refusal nor a success — so warm-pool teardown cannot be performed at all, and a client that relies on the refusal to protect live warmed instances gets no protection signal and no deletion either.
Fix: implement the DeleteWarmPool handler: validate the group exists, refuse with ResourceInUseFault and AWS's fixed message when the pool still holds instances and ForceDelete is absent/false, and on ForceDelete=true accept the request and drive the pool's instances to termination asynchronously while leaving StartInstanceRefresh on the same group acceptable.
Traces: autoscaling.delete-warm-pool-in-use-without-force-rejected (assert at check line 71: expected ResourceInUse, got 'NotImplemented'); autoscaling.start-instance-refresh-during-warm-pool-deletion-blocked (assert at check line 85, same code). Handler: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_warm_pool.rs:165 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteWarmPool"))`), SCAFFOLDED marker at line 4.

## Evidence
Check [mismatch] autoscaling.delete-warm-pool-in-use-without-force-rejected
  Expected (AWS): after PutWarmPool(MinSize=1, PoolState=Stopped) and the pool reporting >=1 instance in Warmed:Stopped, DeleteWarmPool(AutoScalingGroupName=<group>) with no ForceDelete -> HTTP 400, code `ResourceInUse`, message "You cannot delete a warm pool while there are instances still in it unless you use the force delete option."; the warm pool and its instance remain.
  Actual (emulator): AssertionError: expected ResourceInUse, got 'NotImplemented' — HTTP 501, message "operation DeleteWarmPool is scaffolded but not implemented in this emulator".

Check [mismatch] autoscaling.start-instance-refresh-during-warm-pool-deletion-blocked
  Expected (AWS): the same steady-state ResourceInUse refusal (retrying through transient ScalingActivityInProgress / ResourceContention), then DeleteWarmPool(ForceDelete=True) accepted with HTTP 200, a concurrent StartInstanceRefresh accepted and reported by DescribeInstanceRefreshes in Pending/InProgress/Baking/Successful/Cancelling/Cancelled, and the pool draining to zero instances.
  Actual (emulator): AssertionError: unexpected code='NotImplemented' at the first DeleteWarmPool; the ForceDelete and concurrent-refresh assertions are never exercised.

## Steps to Reproduce
1. ec2 CreateLaunchTemplate with LaunchTemplateData {ImageId: <AL2023 AMI>, InstanceType: t3.micro}.
2. autoscaling CreateAutoScalingGroup(AutoScalingGroupName=<group>, LaunchTemplate={...}, MinSize=0, MaxSize=1 (or 2), DesiredCapacity=0, VPCZoneIdentifier=<subnet>).
3. autoscaling PutWarmPool(AutoScalingGroupName=<group>, MinSize=1, PoolState="Stopped") and poll DescribeWarmPool until it reports at least one instance in Warmed:Stopped/Stopped.
4. autoscaling DeleteWarmPool(AutoScalingGroupName=<group>) with NO ForceDelete.
   Observed: HTTP 501 / code NotImplemented. Expected: HTTP 400 / code ResourceInUse with the message "You cannot delete a warm pool while there are instances still in it unless you use the force delete option.", and DescribeWarmPool still reporting the instance.
5. autoscaling DeleteWarmPool(AutoScalingGroupName=<group>, ForceDelete=True), then StartInstanceRefresh(AutoScalingGroupName=<group>) and DescribeInstanceRefreshes.
   Observed: the DeleteWarmPool call again answers 501, so no teardown happens. Expected: 200, the refresh accepted, and the pool draining to zero instances.
