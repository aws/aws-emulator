Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-predictive-forecast-without-history-has-no-datapoints
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: auto-scaling-plans|CreateScalingPlan|ValidationException
Issue-Commit: 7cb8f835e6eb48db46e23acdee391f97cacfb45c

# REOPENS: CreateScalingPlan again refuses MinCapacity 0 with ValidationException "Minimum capacity cannot be less than 1", so no ForecastOnly / scale-to-zero plan can be created

> _Note: this issue was identified against the code base at commit `7cb8f835e6eb48db46e23acdee391f97cacfb45c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScalingPlan

## Description
REOPENS 20261114T175837Z-autoscaling-plans-reopens-createscalingplan-refuses-mincapacity-0-with-validationexception-minimum.md: its fix left the call refused.

That issue asked the scaling-instruction validation to DROP the invented MinCapacity floor of 1 while keeping the cross-member relation. The floor is back in the current checkout, so the identical request is refused again with the identical sentence. This is not a new defect: it is the same refusal the fixed issue closed.

Cause 1 of 1: CreateScalingPlan refuses a ScalingInstruction whose MinCapacity is 0 with HTTP 400 ValidationException instead of accepting it.

AWS: Smithy (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json) — ScalingInstruction$MinCapacity is @required and targets ResourceCapacity, declared `{"type":"integer"}` with NO @range trait, documented only as "The minimum capacity of the resource." CreateScalingPlan binds ValidationException (errorKind client, httpError 400, "An exception was thrown for a validation issue. Review the parameters provided."). The model therefore states no numeric floor at all, so the accepted case is recorded behavior only: a create carrying one instruction for ServiceNamespace 'autoscaling', ResourceId 'autoScalingGroup/<group>', ScalableDimension 'autoscaling:autoScalingGroup:DesiredCapacity', MinCapacity 0, MaxCapacity 1, one predefined target-tracking configuration, a PredefinedLoadMetricSpecification, PredictiveScalingMode 'ForecastOnly' and DisableDynamicScaling true answers HTTP 200 with ScalingPlanVersion 1 and the plan then settles Active. A zero minimum is the ordinary configuration for an Auto Scaling group whose MinSize is 0, and necessarily so for a ForecastOnly plan, which scales nothing. The only capacity-band refusal the recordings state is the cross-member one, "Maximum capacity cannot be less than minimum capacity".

Emulator: HTTP 400, Error.Code 'ValidationException', Error.Message 'Minimum capacity cannot be less than 1'. That sentence is entirely FIXED text in the emulator's own vocabulary — no part of it is interpolated from the request or from state, and the literal 1 is the emulator's invented floor, not a value echoed from the request. The rule and its wording are both the emulator's: a crate constant MIN_RESOURCE_CAPACITY compared in validate_scaling_instruction, which the fixed issue above had removed.

Why: a client gets a terminal client error on a request AWS accepts, so the whole predictive-scaling surface is unreachable for a scale-to-zero resource — the plan never exists, DescribeScalingPlans never reports it, and GetScalingPlanResourceForecastData can never be exercised against it. To an SDK the refusal is indistinguishable from a genuinely invalid request, so request-repair logic rewrites a request AWS would have taken. It also masks the forecast divergence already filed as 20261120T134203Z-autoscaling-plans-2-net-new-causes-createscalingplan-accepts.md (Cause 2, the fabricated cold-start forecast): the check now aborts at the create and never reaches the forecast reads, so that cause cannot be re-verified until this refusal is lifted.

Note the tension a fixer must resolve rather than flip: the open wire issue 20261121T002126Z-autoscaling-plans-createscalingplan-accepts-a-scalinginstruction-with-mincapacity-below-1.md and 20261120T134203Z (Cause 1) ask for a NEGATIVE MinCapacity to be refused with that same sentence. Both recordings hold simultaneously: MinCapacity < 0 is refused with 'Minimum capacity cannot be less than 1', MinCapacity == 0 is ACCEPTED. The floor must be drawn at 0, not at 1.

Fix: the shared scaling-instruction validation in the service's state module (validate_scaling_instruction, reached from the CreateScalingPlan and UpdateScalingPlan handlers via validate_scaling_instructions) must stop refusing MinCapacity 0 — the per-member capacity bound belongs at "not negative", with the existing cross-member relation unchanged — and a zero minimum must reach the create path and be stored and reported as given. The create handler, the serializer and the error envelope need no change.

Traces: autoscaling-plans.asp-predictive-forecast-without-history-has-no-datapoints. Code sites: src/state.rs:1435-1441 (the MIN_RESOURCE_CAPACITY comparison returning MINIMUM_CAPACITY_BELOW_FLOOR, with the comment "the floor of the band an instruction registers is MIN_RESOURCE_CAPACITY"), reached from src/state.rs:1387-1410 (validate_scaling_instructions) and from src/ops/create_scaling_plan.rs and src/ops/update_scaling_plan.rs.

## Evidence
Check autoscaling-plans.asp-predictive-forecast-without-history-has-no-datapoints [error]:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateScalingPlan operation: Minimum capacity cannot be less than 1
  (raised at client.py:1094 in _make_api_call, from the check's asp.create_scaling_plan call)

Request that was refused (one instruction): ServiceNamespace='autoscaling', ResourceId='autoScalingGroup/<group>', ScalableDimension='autoscaling:autoScalingGroup:DesiredCapacity', MinCapacity=0, MaxCapacity=1, TargetTrackingConfigurations=[{PredefinedScalingMetricSpecification={PredefinedScalingMetricType='ASGAverageCPUUtilization'}, TargetValue=50.0}], PredefinedLoadMetricSpecification={PredefinedLoadMetricType='ASGTotalCPUUtilization'}, PredictiveScalingMode='ForecastOnly', DisableDynamicScaling=True, with ApplicationSource={TagFilters:[{Key:'sim2real-asp',Values:['<tag>']}]}. The group name, tag and plan name are per-run values and are not part of the requirement.

Expected (recorded AWS): HTTP 200, body carrying ScalingPlanVersion 1; the plan then settles StatusCode 'Active' and reads back PredictiveScalingMode 'ForecastOnly', DisableDynamicScaling true and its PredefinedLoadMetricSpecification.
Actual (emulator): HTTP 400, Error.Code 'ValidationException', Error.Message 'Minimum capacity cannot be less than 1' — the full sentence is fixed text with no variable part; the plan is never created, so every later assertion in the check is unreachable.

Model evidence: ScalingInstruction$MinCapacity -> ResourceCapacity, `{"type": "integer"}`, required, NO @range; ScalingInstruction$MaxCapacity likewise; ValidationException = {Message -> ErrorMessage}, errorKind client, httpError 400, awsQueryError code 'ValidationException'.

Emulator source (current checkout): services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1439-1441 — `if instruction.min_capacity < MIN_RESOURCE_CAPACITY { return Err(MINIMUM_CAPACITY_BELOW_FLOOR.to_string()); }`, immediately before the correct cross-member check at :1442-1444. The adjacent cross-member refusal is right, which locates the defect precisely at the per-member floor's value.

## Steps to Reproduce
1. Boot the emulator from this checkout and run behavioral check autoscaling-plans.asp-predictive-forecast-without-history-has-no-datapoints.
2. Or directly: create a launch template and an Auto Scaling group with MinSize 0 / MaxSize 1 / DesiredCapacity 0, then call autoscaling-plans CreateScalingPlan with ApplicationSource={TagFilters:[{Key:'<tag-key>',Values:['<tag>']}]} and one ScalingInstruction exactly as quoted in the evidence (MinCapacity 0, MaxCapacity 1, PredictiveScalingMode 'ForecastOnly', DisableDynamicScaling true).
3. Observe HTTP 400 ValidationException 'Minimum capacity cannot be less than 1' instead of HTTP 200 with ScalingPlanVersion 1.
4. Controls, all of which must hold after the fix: MinCapacity 1 is accepted (it is today); MinCapacity -1 is still refused with 'Minimum capacity cannot be less than 1' (the open wire issue 20261121T002126Z and 20261120T134203Z Cause 1); MinCapacity 9 / MaxCapacity 2 is still refused with 'Maximum capacity cannot be less than minimum capacity'.
5. After the fix, re-replay the whole check so its later, currently unreachable assertions are reached — the plan settling Active, the instruction reading back, and the four GetScalingPlanResourceForecastData reads (which are expected to surface the already-filed fabricated-forecast cause of 20261120T134203Z).
