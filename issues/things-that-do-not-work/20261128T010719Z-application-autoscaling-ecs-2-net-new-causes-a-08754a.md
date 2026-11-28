Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch
Issue-Key: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits|run#64|AssertionError|assert after_4 == after_3 + _OUT_ADJ, (
Issue-Key: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch|run#48|AssertionError|assert held == after_out['subject'], (
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# application-autoscaling__ecs: 2 net-new causes — a scale-OUT inside a scale-in cooldown is suppressed, and resuming DynamicScalingInSuspended retroactively replays the withheld scale-in

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Two root causes, grounded in 2 failing checks the suite report lists under `### application-autoscaling__ecs`, neither marked "ALREADY FILED" nor "OWNED by" an open issue.

Cause 1 of 2: the scaling cooldown is implemented as a single direction-agnostic "in cooldown" flag, so a scale-OUT breach that lands inside an unexpired scale-IN cooldown is suppressed instead of applying immediately.

  AWS: Smithy (com.amazonaws.applicationautoscaling#AnyScaleFrontendService) models StepScalingPolicyConfiguration.Cooldown — "The amount of time, in seconds, to wait for a previous scaling activity to take effect. If not specified, the default value is 300" — together with AdjustmentType (ChangeInCapacity|ExactCapacity|PercentChangeInCapacity) and StepAdjustments. AWS documents the direction-selectivity rule for step scaling (step-scaling-policy-overview.html): scale-in activities are BLOCKED until the scale-in cooldown period has expired; however, if another alarm triggers a scale-OUT activity during the scale-in cooldown period, Application Auto Scaling scales out the target IMMEDIATELY, and the scale-in cooldown period stops and does not complete. So with a scale-in policy and a scale-out policy on the same scalable target (ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount): a scale-in activity starts a cooldown window measured from that activity's reported EndTime plus the policy's reported Cooldown; a scale-in breach inside that window is withheld; the SAME breach after the window applies in full; and a scale-OUT breach inside that window applies in full and at once, moving the ECS service's desiredCount by the policy's ChangeInCapacity. The window arithmetic is derived from the API's own reported EndTime/Cooldown values, not from any fixed clock.

  Emulator: legs 1-3 of the lifecycle behave (the scale-in is withheld inside the window and permitted outside it), but leg 4 — a scale-out alarm transition that lands strictly before the second window's expiry — moves nothing. ECS keeps reporting the pre-breach desiredCount for the whole apply budget, i.e. the capacity stays at the value the scale-in left it at instead of advancing by the scale-out policy's ChangeInCapacity. That is exactly the signature of one boolean covering both directions.

  Why: a client that relies on cooldowns for damping loses its ability to react to load. A target that has just scaled in and then immediately breaches its scale-out alarm is left under-provisioned for the whole remaining cooldown (up to the policy's Cooldown, 300s by default) — the one case the documented rule exists to prevent. Automation and capacity dashboards see a scale-out alarm in ALARM with no activity and no NotScaledReason explaining it.

  Fix: the scaling-decision path in the application-autoscaling crate — the cooldown bookkeeping that gates a policy invocation must be keyed per DIRECTION (scale-in cooldown blocks only scale-in; a scale-out during a scale-in cooldown proceeds and terminates that window), in the scaling-policy evaluation/activity recording and the scalable-target state that stores the last activity's EndTime.

  Traces: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits.

Cause 2 of 2: lifting DynamicScalingInSuspended via RegisterScalableTarget replays the scale-in that was withheld while the flag was true, so the resume CALL itself moves capacity.

  AWS: Smithy models SuspendedState {DynamicScalingInSuspended, DynamicScalingOutSuspended, ScheduledScalingSuspended}, with DynamicScalingInSuspended documented as "Whether scale in by a target tracking scaling policy or a step scaling policy is suspended. Set the value to true if you don't want Application Auto Scaling to remove capacity when a scaling policy is triggered." Suspension withholds the activity; it does not queue it. A scaling policy is invoked by a CloudWatch alarm STATE TRANSITION, so an alarm that is still sitting in the same ALARM state it held before the resume produces no fresh invocation and therefore no activity: after RegisterScalableTarget sets DynamicScalingInSuspended=false, the target's capacity is unchanged until the next transition. Recorded behavior confirms it: resuming does not retroactively replay a stale breach.

  Emulator: with the subject's scale-in alarm held continuously in ALARM, RegisterScalableTarget(SuspendedState={DynamicScalingInSuspended: false, ScheduledScalingSuspended: false}) is immediately followed by the ECS service's desiredCount dropping by the scale-in policy's ChangeInCapacity (the withheld adjustment) within 60s, with no new alarm transition in between. The emulator either treats the resume call as a trigger or replays every invocation it withheld.

  Why: a client that suspends scale-in to protect capacity during a migration or a drain gets the opposite of what it asked for the moment it un-suspends: a burst of stale, possibly long-obsolete scale-ins fires against capacity that may no longer warrant them, from a breach that may have ended hours earlier. The suspension becomes a deferral rather than a cancellation, which is unsafe to use at all.

  Fix: the suspension handling in the application-autoscaling crate — RegisterScalableTarget's SuspendedState update must only clear the flag on the scalable target and must not enqueue, retain or re-dispatch withheld policy invocations; the policy-invocation path must remain driven solely by alarm state transitions, and each withheld decision must be dropped (recorded as a not-scaled activity) rather than stored.

  Traces: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch.

## Evidence
Cause 1 — expected (AWS, documented step-scaling rule): with a registered scalable target (ServiceNamespace=ecs, ResourceId=service/<cluster>/<service>, ScalableDimension=ecs:service:DesiredCount, MinCapacity=1, MaxCapacity<desiredCount+out adjustment), a ChangeInCapacity scale-in policy and a ChangeInCapacity scale-out policy, each driven by its own CloudWatch alarm: (leg 1) a scale-in breach lowers desiredCount by the scale-in adjustment and reports a Successful activity; (leg 2) the same breach inside EndTime+Cooldown moves nothing; (leg 3) the same breach after that window lowers it again; (leg 4) a scale-out alarm transition inside the NEW scale-in cooldown window raises desiredCount by the scale-out adjustment immediately.

Cause 1 — actual (emulator):

  File "check_application_autoscaling__ecs_step_scaling_scale_in_cooldown_suppresses_then_permits.py", line 559, in run
    assert after_4 == after_3 + _OUT_ADJ
  AssertionError: leg 4: ECS reports desiredCount 13 for '<service>'; a scale-out inside a scale-in cooldown is documented to proceed IMMEDIATELY, so ChangeInCapacity 3 must take 8 to 11. An implementation with a single "in cooldown" flag covering both directions leaves 8

(The service name, and the absolute counts 13/8/11, are this run's values; the requirement is that the scale-out adjustment is applied in full during a scale-in cooldown. The final capacity stayed strictly inside the registered bounds, so no clamp explains it.)

Cause 2 — expected (AWS): with the subject's scale-in alarm held in ALARM across the whole leg and no new transition, `aas register-scalable-target --suspended-state DynamicScalingInSuspended=false,ScheduledScalingSuspended=false` leaves ECS's desiredCount at the value it held while suspended, and DescribeScalingActivities (IncludeNotScaledActivities=true) shows no Successful activity setting the withheld desired count.

Cause 2 — actual (emulator):

  File "check_application_autoscaling__ecs_suspended_scaling_dimensions_are_independent_not_one_switch.py", line 784, in run
    assert held == after_out['subject']
  AssertionError: leg 4a: the subject reports desiredCount 8 within 60s of resuming DynamicScalingInSuspended, expected still 11. The alarm sat in the SAME ALARM state it was already in before the resume, with no fresh transition -- an implementation that treats the resume CALL ITSELF as a trigger, or that replays every withheld invocation on resume, scales here and would be wrong: measured on real AWS, resuming does not retroactively replay a stale breach

(11 and 8 are this run's capacities — 11 is what was held under suspension and 8 is 11 plus the withheld ChangeInCapacity -3. The requirement is that no capacity change follows the resume absent a new alarm transition.)

## Steps to Reproduce
Cause 1:
1. Create an ECS cluster and a Fargate service with desiredCount 12; `application-autoscaling register-scalable-target --service-namespace ecs --resource-id service/<c>/<s> --scalable-dimension ecs:service:DesiredCount --min-capacity 1 --max-capacity 40`.
2. `put-scaling-policy` twice on that target: a step policy OUT (AdjustmentType=ChangeInCapacity, ScalingAdjustment=+3, Cooldown=120) and a step policy IN (ChangeInCapacity, ScalingAdjustment=-3, Cooldown=120), each wired as the alarm action of its own CloudWatch alarm over a custom metric.
3. Publish a scale-in breach; wait for the first Successful activity and read its EndTime; compute window = EndTime + policy Cooldown.
4. Return the alarm to OK and breach again INSIDE the window: desiredCount must not move (this leg passes today).
5. Wait past the window, breach again: desiredCount drops by 3 and a second Successful activity is reported; recompute window from its EndTime.
6. Inside that new scale-in cooldown window, transition the SCALE-OUT alarm to ALARM: on AWS `ecs describe-services` reports desiredCount+3 promptly; on the emulator desiredCount never moves for the whole apply budget.

Cause 2:
1. Register two identical scalable targets (subject and twin) over two Fargate services, each with a scale-out and a scale-in step policy and their alarms.
2. `register-scalable-target ... --suspended-state DynamicScalingInSuspended=true,ScheduledScalingSuspended=true` on the subject only.
3. Breach both scale-in alarms: the twin scales in (positive control); the subject holds its capacity while its alarm stays in ALARM, and no Successful activity for the withheld count appears.
4. With the subject's alarm still in the SAME ALARM state, `register-scalable-target ... --suspended-state DynamicScalingInSuspended=false,ScheduledScalingSuspended=false`.
5. Poll `ecs describe-services` for 60s: on AWS the subject's desiredCount is unchanged; on the emulator it drops by the withheld ChangeInCapacity right after the resume call.
