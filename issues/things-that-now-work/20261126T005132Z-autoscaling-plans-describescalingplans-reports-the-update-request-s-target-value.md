Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-scaling-plan-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-scaling-plan-lifecycle
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].ScalingInstructions[*].TargetTrackingConfigurations[*].TargetValue
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].StatusMessage
Issue-Commit: bf205090aef11dede75ce528b7b57e0b8feca92b

# DescribeScalingPlans reports the update request's target value and an invented update narrative instead of the applied plan state

> _Note: this issue was identified against the code base at commit `bf205090aef11dede75ce528b7b57e0b8feca92b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPlans

## Description
One root cause with two observable symptoms on the same DescribeScalingPlans read, taken after an UpdateScalingPlan earlier in the scaling-plan lifecycle fixture: the emulator keeps and projects its OWN bookkeeping of "this plan was updated" — the request's scaling-instruction set written straight over the one in force, plus a per-mutation status narrative — where the recorded service reports the plan's APPLIED configuration and the one Active narrative. The step's HTTP status matches (200 -> 200); only the body diverges.

Cause 1 of 2: ScalingPlan$StatusMessage carries an emulator-authored sentence after an in-place update.
  AWS: Smithy: ScalingPlan$StatusMessage is documented only as "A simple message about the current status of the scaling plan" — the model constrains it to no value set, so this is recorded behavior only. In the recording, a plan whose StatusCode is the applied status reports the fixed sentence "Scaling plan has been created and applied to all resources." — and it still reports exactly that sentence after an UpdateScalingPlan against the same plan. The whole sentence is fixed text; it interpolates no value from the request or from state.
  Emulator: reports "Scaling plan has been updated successfully." for the same plan and the same (matching) StatusCode. That string exists nowhere in the Smithy model or in the recording; it is minted by the service itself: state.rs defines APPLIED_AFTER_UPDATE_MESSAGE and ScalingPlanRecord::status_message swaps to it whenever status_code == "Active" and applied_mutation == AppliedMutation::Updated, which UpdateScalingPlan sets via ScalingPlanRecord::begin_update(applied_change, now).
  Why: a client that keys off StatusMessage — the modeled member whose own status-code docs tell a caller to "Check the status message for more information" — sees a sentence AWS never emits, so any equality/contains test on the applied narrative fails, and a caller cannot distinguish an applied plan on the emulator from an unknown status.
  Fix: the plan record's status narrative in the service state (ScalingPlanRecord::status_message / the AppliedMutation discriminator it reads) — the applied status must have ONE narrative regardless of which mutation reached it; DescribeScalingPlans' projection then needs no change.
  Traces: autoscalingplans-scaling-plan-lifecycle#task#7 ($.ScalingPlans[0].StatusMessage: expected 'Scaling plan has been created and applied to all resources.', got 'Scaling plan has been updated successfully.').

Cause 2 of 2: the described plan echoes the update request's TargetTrackingConfiguration target value rather than the value in force on the plan.
  AWS: Smithy: DescribeScalingPlans returns ScalingPlans[*].ScalingInstructions[*].TargetTrackingConfigurations[*].TargetValue (TargetTrackingConfiguration$TargetValue, required double); UpdateScalingPlan "Updates the specified scaling plan" and its output shape is empty, so what an update actually took is observable ONLY through this read. Recorded behavior only: in the recording, this read reports the target value the plan had from CreateScalingPlan, together with the create narrative of Cause 1 — i.e. the recorded service reports the configuration applied to the plan, not the one most recently requested. Hypothesis: the recorded update did not take on the described plan's target tracking configuration (the two mismatching members are exactly the two the emulator moves on an update, and every other member of the entry — name, version, status code, application source, timestamps — matched), so the requirement the replay establishes is that this read projects the applied configuration.
  Emulator: projects the request's instruction set immediately. ops/update_scaling_plan.rs assigns plan.scaling_instructions = input.scaling_instructions verbatim, and ops/describe_scaling_plans.rs project() clones record.scaling_instructions, so the first target tracking configuration's target value reported after the update is the requested one (got 75.0 where the recording reports 50.0). The record keeps no separate applied snapshot — only applied_footprint(), computed on the fly for the narrative decision.
  Why: a client polls DescribeScalingPlans to learn what the plan is actually enforcing (UpdateScalingPlan returns nothing). Against the emulator the read echoes the caller's own request, so it can never detect an update that did not take and will believe a target value is enforced that the recording shows is not.
  Fix: UpdateScalingPlan's handler and the plan record in the service state — keep what the plan has APPLIED distinct from what a request asked for, and have DescribeScalingPlans' projection report the applied instruction set.
  Traces: autoscalingplans-scaling-plan-lifecycle#task#7 ($.ScalingPlans[0].ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue: expected 50.0, got 75.0).

No open issue covers either cause: the only other open autoscaling-plans issue is 20261121T060302Z-autoscaling-plans-reopens-createscalingplan-again-refuses-mincapacity-0-with-validationexception.md (CreateScalingPlan MinCapacity 0 refusal), which is unrelated. The divergence carried no stub_path and no owned_by.

## Evidence
Replay of trace set simparity-wire-aws-v1 (49 steps, 48 passed, 1 mismatched, score 98.0%), emulator up (/__admin/health 200).

Divergence cluster (verdict: mismatch), fixture autoscalingplans-scaling-plan-lifecycle, step task#7, service autoscaling-plans, operation DescribeScalingPlans, expected_status 200, actual_status 200, affected_count 1:
  $.ScalingPlans[0].ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue: value_mismatch (expected 50.0, got 75.0)
  $.ScalingPlans[0].StatusMessage: value_mismatch (expected 'Scaling plan has been created and applied to all resources.', got 'Scaling plan has been updated successfully.')

Expected (recorded AWS), both members of the same ScalingPlans[0] entry:
  StatusMessage = "Scaling plan has been created and applied to all resources."  (fixed text, no interpolation)
  ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue = the value in force on the plan (50.0 in this recording)

Actual (emulator):
  StatusMessage = "Scaling plan has been updated successfully."  (fixed text, likewise un-interpolated, but a sentence AWS does not emit)
  ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue = the value the preceding UpdateScalingPlan requested (75.0)

Every other member of the entry matched, including StatusCode, ScalingPlanName, ScalingPlanVersion, ApplicationSource and the timestamps — so this is not an ordering or identity difference, and no per-run value (timestamp, request id, generated name) is involved.

Source of the emulator behavior, in the checkout:
  services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:146 — const APPLIED_AFTER_UPDATE_MESSAGE = "Scaling plan has been updated successfully."
  .../src/state.rs:358-363 — ScalingPlanRecord::status_message() returns APPLIED_AFTER_UPDATE_MESSAGE when status_code == "Active" && applied_mutation == AppliedMutation::Updated (the applied row at state.rs:129 holds AWS's recorded sentence)
  .../src/state.rs:332-337 — begin_update() sets applied_mutation = AppliedMutation::Updated
  .../src/ops/update_scaling_plan.rs:260-269 — plan.scaling_instructions = input.scaling_instructions; then begin_update(applied_change, now)
  .../src/ops/describe_scaling_plans.rs:309-323 — project() clones record.scaling_instructions and record.status_message() straight into the response

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture `autoscalingplans-scaling-plan-lifecycle` from trace set simparity-wire-aws-v1 (replay_wire_corpus). Steps 1..6 pass; step `task#7` mismatches.
2. Equivalently, by hand against the autoscaling-plans endpoint:
   a. CreateScalingPlan with one scaling instruction whose TargetTrackingConfigurations[0].TargetValue is V1 (50.0 in the fixture).
   b. Wait for the plan to reach the applied status, then UpdateScalingPlan (same ScalingPlanName, ScalingPlanVersion 1) re-sending that instruction with TargetTrackingConfigurations[0].TargetValue = V2 (75.0 in the fixture).
   c. DescribeScalingPlans with that ScalingPlanNames entry once the plan is back in the applied status.
3. Observe on ScalingPlans[0]: StatusMessage is "Scaling plan has been updated successfully." instead of the recorded "Scaling plan has been created and applied to all resources.", and ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue is V2 (the requested value) where the recording reports the applied value V1.

## Resolution

Both symptoms are fixed by the one root cause they share, and the recorded fixture replays clean (11 of 11 steps).

The rule: a scaling plan reports the configuration it has APPLIED to the resources it covers, so the members an in-place update asks for — its `ScalingInstructions`, its `ApplicationSource` and the narrative of its applied status alike — become what a read reports only once the plan has applied them, which the lifecycle clock owes after the request. `UpdateScalingPlan` answers an empty body, so the read-back is the only place a caller learns what an update took; a read that echoed the request could never tell an update that has been applied from one that has not.

The stored plan now keeps what an update ASKED for (`ScalingPlanRecord::staged`) apart from what it has applied: `UpdateScalingPlan` stages the members the request carries and the clock puts them in force (`ScalingPlanRecord::apply_staged`, `APPLY_AFTER`), where the applied-footprint comparison that drives the status narrative also happens. `DescribeScalingPlans` is unchanged — it was already a pure projection of the applied record. Cause 1's narrative is therefore not removed: a plan that HAS applied an update still reports "Scaling plan has been updated successfully." (the behaviour recorded in 20261115T121354Z, where a poller converges on it), while a read taken before the application reports the narrative of the application in force, which is what this recording shows.

