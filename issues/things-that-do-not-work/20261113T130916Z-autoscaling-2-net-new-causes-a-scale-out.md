Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.e2e-warm-pool-scale-out-consumes-pool-instances autoscaling.e2e-warm-pool-scale-then-drain autoscaling.op-contract-describeWarmPool
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.e2e-warm-pool-scale-out-consumes-pool-instances|run#22|AssertionError|assert in_service_ids == warm_ids_before, (
Issue-Key: autoscaling.e2e-warm-pool-scale-then-drain|run#22|AssertionError|assert in_service_ids <= warm_ids_before, (
Issue-Key: autoscaling.op-contract-describeWarmPool|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 4370547da342f3cac1e3d2a524b52e751b2d7b18

# autoscaling: 2 net-new causes — a scale-out never consumes the warm pool (fresh instances are launched instead), and DescribeWarmPool's invalid-NextToken message is re-worded

> _Note: this issue was identified against the code base at commit `4370547da342f3cac1e3d2a524b52e751b2d7b18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
Two net-new root causes on the Auto Scaling warm-pool surface (Cause 1 of 2 .. Cause 2 of 2). BOTH must be closed before this issue is submitted; each names its own failing checks.

Cause 1 of 2: raising capacity on a group that has a populated warm pool launches brand-new instances instead of moving the already-prepared warm instances into service.
AWS: Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): PutWarmPool input {AutoScalingGroupName required, MinSize @range min 0, MaxGroupPreparedCapacity @range min -1, PoolState WarmPoolState enum, InstanceReusePolicy}; DescribeWarmPool -> {WarmPoolConfiguration, Instances, NextToken} (paginated on Instances); the LifecycleState enum models the warmed states Warmed:Pending / Warmed:Stopped / Warmed:Running / Warmed:Hibernated in the SAME value set as Pending / InService / Terminating, i.e. a warmed instance is a member of that group that later transitions into service. SetDesiredCapacity input {AutoScalingGroupName required, DesiredCapacity required, HonorCooldown}, empty output. Recorded behavior for the identity rule: when DesiredCapacity rises, the group draws from the instances the warm pool already prepared — the same InstanceIds leave the pool (Warmed:Stopped -> Pending -> InService) and the pool depth drops by the number consumed; no new instance is launched while the pool can satisfy the increase.
Emulator: the pool does report prepared warmed instances (DescribeWarmPool lists them in Warmed:Stopped), but the scale-out ignores them: the set of InService InstanceIds after the increase is disjoint from the warm set, i.e. fresh instances are launched while the warmed ones stay parked in the pool. (In the two replays the warm set was two ids and the InService set two different ids, and one id vs one different id respectively; the ids themselves are generated per run — the requirement is the identity relation, that the InService instances are drawn FROM the warm set, not which ids appear.)
Why: the whole point of a warm pool is that pre-initialized instances enter service without a cold launch. A client that verifies reuse by identity (the documented way to prove the pool was used), that counts pool depth, or that budgets for the pool's instances sees capacity it never asked for: warm instances are never consumed, so the group's total footprint is pool + freshly launched instances, launch latency is never reduced, and scale-then-drain flows that expect the pool to shrink never converge.
Fix: the capacity-reconciliation path in the autoscaling service's group state (behind SetDesiredCapacity / UpdateAutoScalingGroup) together with the warm-pool state — consume warm-pool members (oldest-prepared first) before launching new instances, transition them through Pending to InService keeping their InstanceId, and remove them from the pool.
Traces: autoscaling.e2e-warm-pool-scale-out-consumes-pool-instances, autoscaling.e2e-warm-pool-scale-then-drain.

Cause 2 of 2: DescribeWarmPool's invalid-pagination-token refusal uses a sentence AWS does not emit.
AWS: Smithy DescribeWarmPool declares error InvalidNextToken (errorKind client, httpError 400, awsQueryError code 'InvalidNextToken', httpResponseCode 400), documented 'The NextToken value is not valid.'. Recorded behavior for the text: HTTP 400, Error.Code 'InvalidNextToken', Error.Message "The token '<NextToken>' is invalid" — "The token '" and "' is invalid" are AWS's fixed text and the quoted value is the NextToken taken verbatim from the request.
Emulator: same status and same code, but the message "The token '<NextToken>' was invalid" — the fixed half is re-worded from 'is invalid' to 'was invalid'.
Why: the message is part of the refusal contract. A client (or operator runbook, or log-based alarm) that matches AWS's sentence for a stale/forged pagination token does not match the emulator's, so resumable-paging error handling silently falls through to a generic failure path.
Fix: the message built for InvalidNextToken by the pagination-token validation the autoscaling describe handlers share, as reached through DescribeWarmPool.
Traces: autoscaling.op-contract-describeWarmPool.

Related open filings that do NOT carry these checks and therefore do not cover them: 20261113T064920Z-autoscaling-warm-pool-operations-diverge-phantom-warmed-instances.md (phantom warmed instances for an unprepared pool, a deleted pool still reported, and PutWarmPool's not-found text — none of them the consumption-on-scale-out rule or the DescribeWarmPool token sentence) and 20261111T102954Z-autoscaling-describetags-never-returns-group-tags-and-its.md cause 5 (the same 'was invalid' wording on DescribeTags, where AWS answers a DIFFERENT code and message, 'ValidationError' / 'NextToken: <token>'). The replay marked neither cluster OWNED or ALREADY FILED.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator (141/441 checks passed).

Cause 1 — [mismatch] autoscaling.e2e-warm-pool-scale-out-consumes-pool-instances:
  AssertionError: expected scale-out to draw from warm pool {'i-00000000000000002', 'i-00000000000000001'}, got fresh instances {'i-00000000000000003', 'i-00000000000000004'}
  (check body: PutWarmPool MinSize=2/PoolState=Stopped, poll DescribeWarmPool until the warmed instances are ready, then raise DesiredCapacity and compare the InService id set with the warm id set.)

Cause 1 — [mismatch] autoscaling.e2e-warm-pool-scale-then-drain:
  AssertionError: expected the InService instance to be drawn from the warm pool {'i-00000000000000002', 'i-00000000000000001'}, got a fresh launch {'i-00000000000000003'}
  (check body: PutWarmPool MinSize=1, MaxGroupPreparedCapacity=2, PoolState=Stopped; the pool prepares 2; DesiredCapacity 0 -> 1 must promote one of them.)
  The instance ids above are this run's generated values; the divergence is that the InService set is disjoint from the warm set.

Cause 2 — [mismatch] autoscaling.op-contract-describeWarmPool:
  AssertionError: expected message "The token 'sim2real-not-a-real-token' is invalid", got "The token 'sim2real-not-a-real-token' was invalid"
  The code asserted before it (InvalidNextToken) matched; only the message diverges. 'sim2real-not-a-real-token' is the token this check sent — the requirement is that the request's NextToken appears inside AWS's fixed sentence.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay the behavioral suite; the three checks named in trace_ids reproduce.
2. Cause 1: create a launch-template Auto Scaling group with MinSize=0, DesiredCapacity=0, MaxSize>=3; call PutWarmPool (e.g. MinSize=2, PoolState='Stopped'); poll DescribeWarmPool until it lists the prepared instances in Warmed:Stopped and record their InstanceIds; then SetDesiredCapacity / UpdateAutoScalingGroup to raise DesiredCapacity by the pool's depth and poll DescribeAutoScalingGroups. Expect the InService InstanceIds to be exactly the recorded warm ids and the pool to shrink by that many; the emulator reports new ids and leaves the pool untouched.
3. Cause 2: call DescribeWarmPool with AutoScalingGroupName=<an existing group> and NextToken='sim2real-not-a-real-token'. Expect HTTP 400 / InvalidNextToken / "The token 'sim2real-not-a-real-token' is invalid"; the emulator answers "... was invalid".
4. Re-replay autoscaling.e2e-warm-pool-scale-out-consumes-pool-instances, autoscaling.e2e-warm-pool-scale-then-drain and autoscaling.op-contract-describeWarmPool to confirm the fix.
