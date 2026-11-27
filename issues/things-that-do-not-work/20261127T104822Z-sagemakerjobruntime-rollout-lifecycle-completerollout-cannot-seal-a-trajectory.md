Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerjobruntime-failed-rollout-seals-trajectory
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerjobruntime-failed-rollout-seals-trajectory sagemakerjobruntime-job-arn-region-and-category-rejected
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime|CompleteRollout|2xx->4xx|$
Issue-Key: sagemakerjobruntime|CompleteRollout|2xx->4xx|status
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->4xx|$.Message
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|CompleteRollout|4xx->4xx|status
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->4xx|$.Message
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sagemakerjobruntime|UpdateReward|4xx->4xx|status
Issue-Commit: 96893890513d5e7e8a4c2241420ddac2d94594cf

# sagemakerjobruntime rollout lifecycle: CompleteRollout cannot seal a trajectory that has no captured turns, and both lifecycle-transition refusals answer the wrong status, class and wording

> _Note: this issue was identified against the code base at commit `96893890513d5e7e8a4c2241420ddac2d94594cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteRollout

## Description
Three causes, all in the rollout-lifecycle pair (CompleteRollout / UpdateReward) of the AgenticRFT job runtime. They share one root: the pair only ever serves a trajectory that a prior sampling call materialised in this service's own map, and it has no model of the lifecycle TRANSITION AWS refuses, so a rollout that was never sampled (or that is already terminal) is reported as a resource that does not exist.

Cause 1 of 4 — wait, there are 3 causes; numbered below.

Cause 1 of 3: CompleteRollout on a trajectory with no captured turns is refused instead of sealing it.
AWS: Smithy: CompleteRollout, POST /complete-rollout, @idempotent; input CompleteRolloutRequest = JobArn (httpHeader X-Amzn-SageMaker-Job-Arn, @required, @length 1..256, ARN @pattern), TrajectoryId (@required, @length 1..256), Status (CompletionStatus enum: ready|failed), ClientToken (@idempotencyToken); output CompleteRolloutResponse has no members; modelled errors ValidationException(400), ConflictException(409), ResourceNotFoundException(404), AccessDeniedException(403). Operation doc: "Marks a rollout as complete, indicating that no further turns will be appended to the trajectory... the trajectory is sealed and eligible for reward submission via the UpdateReward operation". The model does NOT make a sampled turn a precondition. Recorded behavior: for a JobArn the control plane minted and a TrajectoryId that no Sample call ever used, CompleteRollout with Status=failed is answered HTTP 200 with the empty body {} — the seal itself establishes the trajectory's terminal status, which is why the next steps of the same fixture see current status 'failed'.
Emulator: HTTP 404 ResourceNotFoundException, Message "Trajectory <trajectoryId> not found for account <accountId>" (the fixed text is "Trajectory " ... " not found for account ", with the trajectory id taken from the request and the account id from emulator state).
Why: a client whose rollout failed before it issued any inference call — the normal failure path this fixture records — can never seal its trajectory. The call is also mis-classified: SDKs treat 404 ResourceNotFoundException as "the thing you named is gone", so the caller abandons the rollout instead of proceeding to reward submission, and every later lifecycle call of the same rollout fails too.
Fix: the CompleteRollout handler and the shared rollout resolution it is served by — services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/complete_rollout.rs (the crate::state::serve_rollout_trajectory call) and src/state.rs serve_rollout_trajectory, which returns trajectory_not_found_message whenever guard.trajectory_mut(..) misses. Sealing must be able to CREATE the trajectory record for a (resolved job, TrajectoryId) pair, as the sampling path already does for a first turn.
Traces: sagemakerjobruntime-failed-rollout-seals-trajectory#task#1.

Cause 2 of 3: a CompleteRollout that would re-seal an already-terminal trajectory is not refused the way AWS refuses it.
AWS: recorded behavior — a second CompleteRollout for a trajectory already sealed 'failed' is answered HTTP 400 with Message "Cannot transition trajectory <trajectoryId> to failed from current status: failed". The fixed text is "Cannot transition trajectory " / " to " / " from current status: "; the variable parts are the TrajectoryId from the request, the target status from the request's Status member (its CompletionStatus wire value), and the trajectory's stored current status. The recorded response carries an x-amzn-errortype header (asserted as shaped, not compared literally); the only 400 error these operations model is ValidationException. Note the status vocabulary AWS reports in that sentence: 'failed' and 'reward_received'.
Emulator: HTTP 404 ResourceNotFoundException "Trajectory <trajectoryId> not found for account <accountId>" (consequence of cause 1). Independently of that, the handler has no transition rule at all: src/ops/complete_rollout.rs returns 200 for a repeat seal with the SAME status (its @idempotent branch) and, for a different status, a 409 ConflictException worded "Trajectory <id> is already completed with Status <sealed>; it cannot be completed with Status <requested>." — a different status, a different error class and emulator-authored prose. AWS refuses the repeat even when the requested status equals the current one, so fixing cause 1 alone turns this step into a wrong 200.
Why: a client cannot tell a conflicting re-seal from a vanished trajectory, and an idempotent-looking 200 tells it the transition was accepted when AWS rejected it; retry logic keyed on 404/409 vs 400 diverges.
Fix: CompleteRollout's state transition in src/ops/complete_rollout.rs (the closure passed to serve_rollout_trajectory) plus the lifecycle status vocabulary in src/state.rs (TrajectoryStatus::as_str / CompletionStatus) — refuse any seal of a trajectory that already has a completion_status with the 400 transition sentence above.
Traces: sagemakerjobruntime-failed-rollout-seals-trajectory#task#2 (cluster also covers task#3, task#4 and sagemakerjobruntime-job-arn-region-and-category-rejected#task#3, #task#5).

Cause 3 of 3: UpdateReward against a terminal ('failed') trajectory is not refused the way AWS refuses it.
AWS: Smithy: UpdateReward, POST /update-reward, @idempotent; input = JobArn (header, @required), TrajectoryId (@required), Rewards (DoubleList, @required), ClientToken; empty output; same error set. Doc: "Updates the reward values for a trajectory and transitions it to reward-received status". Recorded behavior: for a trajectory sealed 'failed', UpdateReward is answered HTTP 400 with Message "Cannot transition trajectory <trajectoryId> to reward_received from current status: failed" — the same template as cause 2, with the target status being the lifecycle status this operation moves the trajectory to ('reward_received').
Emulator: HTTP 404 ResourceNotFoundException "Trajectory <trajectoryId> not found for account <accountId>". The handler's own branch for this case (src/ops/update_reward.rs) would word it as a 409 ConflictException "Trajectory <id> was completed with Status failed, which is terminal and accepts no rewards." — again the wrong status, class and wording, and it spells the lifecycle status 'reward-received' where AWS reports 'reward_received'.
Why: the reward-submission client sees a missing resource rather than an illegal transition, so it cannot distinguish "this rollout failed, do not score it" from "your trajectory id is wrong".
Fix: UpdateReward's transition check in src/ops/update_reward.rs and the shared lifecycle-status wording in src/state.rs.
Traces: sagemakerjobruntime-failed-rollout-seals-trajectory#task#5 (cluster also covers sagemakerjobruntime-job-arn-region-and-category-rejected#task#6).

No open issue covers these: the related sagemakerjobruntime issues still open are 20261122T160745Z-sagemakerjobruntime-sample-maintainability--r6 (a maintainability duplication) and 20261126T190555Z-sagemakerjobruntime-samplewithresponsestream-wrong-behavior--r3 (stream framing), neither of which is the rollout lifecycle.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 divergence clusters, 9 steps):

Cause 1 — fixture sagemakerjobruntime-failed-rollout-seals-trajectory, step task#1, CompleteRollout:
  expected: status 200, body {}
  actual:   status 404, body {"__type": "ResourceNotFoundException", "Message": "Trajectory sim2real-probe-sjr-tr-8aa42e81 not found for account 000000000000"}

Cause 2 — fixture sagemakerjobruntime-failed-rollout-seals-trajectory, step task#2, CompleteRollout (cluster of 6 steps: task#2, task#3, task#4 and sagemakerjobruntime-job-arn-region-and-category-rejected#task#3, #task#5):
  expected: status 400, $.Message = "Cannot transition trajectory <trajectoryId> to failed from current status: failed"; x-amzn-errortype present (shaped, not asserted literally)
  actual:   status 404, $.Message = "Trajectory <trajectoryId> not found for account <accountId>", x-amzn-errortype: ResourceNotFoundException

Cause 3 — fixture sagemakerjobruntime-failed-rollout-seals-trajectory, step task#5, UpdateReward (cluster of 2 steps, also sagemakerjobruntime-job-arn-region-and-category-rejected#task#6):
  expected: status 400, $.Message = "Cannot transition trajectory <trajectoryId> to reward_received from current status: failed"
  actual:   status 404, $.Message = "Trajectory <trajectoryId> not found for account <accountId>", x-amzn-errortype: ResourceNotFoundException

Code that produces the actual answers: src/state.rs serve_rollout_trajectory (`let Some(trajectory) = guard.trajectory_mut(job_arn, trajectory_id) else { return Err(trajectory_not_found_message(trajectory_id)) }`, ~line 715) and trajectory_not_found_message (~line 293); src/ops/complete_rollout.rs lines 266-290 (idempotent-200 branch and the 409 ConflictException wording, mapped through `not_found`); src/ops/update_reward.rs lines 238-257 (the 409 ConflictException wordings).

## Steps to Reproduce
1. Replay fixture sagemakerjobruntime-failed-rollout-seals-trajectory against the emulator.
2. Step task#1 — POST /complete-rollout to the job-runtime.sagemaker endpoint with header X-Amzn-SageMaker-Job-Arn set to an ARN of a job the SageMaker control plane minted, body {"TrajectoryId": "<id never sampled>", "Status": "failed"}. AWS answers 200 {}; the emulator answers 404 ResourceNotFoundException "Trajectory <id> not found for account <account>".
3. Step task#2 — repeat the same CompleteRollout call. AWS answers 400 "Cannot transition trajectory <id> to failed from current status: failed"; the emulator answers 404 (and, once the trajectory exists, its handler would answer 200 or a 409 ConflictException with its own wording).
4. Step task#5 — POST /update-reward with the same JobArn/TrajectoryId and a Rewards list. AWS answers 400 "Cannot transition trajectory <id> to reward_received from current status: failed"; the emulator answers 404.
5. The same clusters reproduce in fixture sagemakerjobruntime-job-arn-region-and-category-rejected at task#3, task#5 and task#6.
