Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.set-rule-priorities-conflict-with-existing-fails elbv2.set-rule-priorities-duplicate-priority-fails elbv2.zero-weight-target-group-is-never-selected elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.set-rule-priorities-conflict-with-existing-fails|run#25|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.set-rule-priorities-duplicate-priority-fails|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elbv2.zero-weight-target-group-is-never-selected|_error_code#33|AssertionError|raise AssertionError(
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|_poll_stable#42|Timeout|time.sleep(interval_s)
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# elbv2: 3 net-new causes — the SetRulePriorities PriorityInUse message is emulator-authored, ModifyListener accepts an all-zero weighted forward action, and the CFN rule-priority-collision UPDATE flow still never reaches a terminal state

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRulePriorities

## Description
Three net-new root causes on the ELBv2 surface, grounded in 4 failing behavioral checks the replay report lists under `### elbv2` and does not mark ALREADY FILED or OWNED. Open issues that look adjacent but do NOT carry these checks' replay keys, and so do not cover them: 20261030T195554Z-elasticloadbalancing-elbv2-setrulepriorities-response-emits-an-empty-transforms-element.md (the success-path response shape, not the refusal message) and 20261104T094656Z-elbv2-ten-overly-permissive-control-plane-gaps-calls.md (a different set of over-permissive control-plane calls). A fixer must close all three causes below before submitting.

=== Cause 1 of 3: SetRulePriorities refuses a priority collision with the right code but an emulator-authored message ===
Failing checks: elbv2.set-rule-priorities-conflict-with-existing-fails, elbv2.set-rule-priorities-duplicate-priority-fails.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#SetRulePriorities (input RulePriorities required) models PriorityInUseException with awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}; the error shape's doc is 'The specified priority is in use.' The model does not state the message text, so recorded behavior only: when a SetRulePriorities entry moves a rule onto a priority another non-default rule on the same listener still holds, AWS answers HTTP 400, Error.Code 'PriorityInUse', and the Error.Message is the fixed collection text 'One or more priorities not found' — the whole message is fixed text, it carries NO variable part (no priority number, no rule ARN, no listener ARN). Freeing the priority (moving or deleting the holder) makes the identical request succeed.
Emulator: the status and code are right (400 / PriorityInUse), but the message is an emulator-authored per-resource template that interpolates the requested priority: "Priority '21' is currently in use" for the move onto held priority 21, and "Priority '10' is currently in use" for the move onto held priority 10. The varying integer is the request's Priority member; the fixed text 'Priority ' / "' is currently in use" is not AWS wording at all.
Why: a client that branches on the refusal text — error-message matching is the only way to distinguish AWS's priority refusals, which share one code across SetRulePriorities and CreateRule — never matches, so a deploy tool treats a well-understood collision as an unknown failure and aborts instead of re-numbering. The same wrong-template family was already fixed once for other ELBv2 refusals (20261027T051252Z-elbv2-addtags-on-a-deleted-load-balancer-the.md, 20261027T105755Z-elbv2-removetags-the-targetgroupnotfound-refusal-uses-the-collection.md), so this is the SetRulePriorities instance of it.
Fix: the SetRulePriorities handler's priority-uniqueness validation, where the PriorityInUse error is constructed — it must emit AWS's fixed message text instead of formatting the requested priority into a sentence. Nothing about the code, the HTTP status or the no-partial-effect behavior needs to change.

=== Cause 2 of 3: ModifyListener accepts a forward action whose every target-group weight is 0 ===
Failing check: elbv2.zero-weight-target-group-is-never-selected.
AWS: the Smithy model carries no cross-member constraint here — TargetGroupTuple.Weight is documented only as 'The weight. The range is 0 to 999.' and ModifyListener's modeled errors do not include a validation error — so recorded behavior only: a forward action with two TargetGroupTuples is accepted at weights 1/0 and 0/1, but a ModifyListener whose forward action sets EVERY target group's Weight to 0 is refused with HTTP 400 and Error.Code 'ValidationError' (the awsQuery generic validation refusal, not one of ModifyListener's modeled shapes), and the refused change leaves the listener's stored DefaultActions exactly as they were, so the listener keeps serving its surviving configuration. The check pins the code and the effect-absence; it does not pin the message, so the message is not a requirement of this cause.
Emulator: the call SUCCEEDS with HTTP 200 and echoes the all-zero configuration back — DefaultActions[0].ForwardConfig.TargetGroups is returned as both tuples with 'Weight': 0 — and the all-zero weighting is then stored on the listener, which is a routing state AWS never lets a listener hold (every weight zero means no target group can ever be selected). The weight-respecting data plane itself is correct: the same check's 1/0 and 0/1 phases both served exclusively from the non-zero group across 20 requests each.
Why: a client that uses weighted forward actions for blue/green shifting relies on the all-zero request being refused as the arithmetic error it is. Against the emulator the call reports success, the listener silently enters a configuration with no selectable target, and the operator discovers the outage from traffic instead of from the API refusal — the exact inversion that makes a canary rollout script unsafe.
Fix: ModifyListener's (and, for the same shape, CreateListener's and CreateRule's/ModifyRule's) forward-action validation — add the 'at least one target group weight must be non-zero' cross-member check and refuse with ValidationError (HTTP 400) before the action reaches the listener's stored state.

=== Cause 3 of 3: the CloudFormation-driven rule-priority-collision UPDATE flow still reaches no terminal state ===
Failing check: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.
AWS: Smithy SetRulePriorities and CreateRule both model PriorityInUseException with awsQueryError {code: 'PriorityInUse', httpResponseCode: 400}; CreateRule's Priority is @range 1..50000. Recorded behavior for the stack-level strings: a stack holding two AWS::ElasticLoadBalancingV2::ListenerRule resources settles CREATE_COMPLETE and its ALB serves the first rule's fixed response; an update moving one rule onto the other's priority is refused by ELBv2 with PriorityInUse, so the stack settles UPDATE_ROLLBACK_COMPLETE with an UPDATE_FAILED event on that rule's logical id and no partial effect (same rule ARN, original priority, original action body, unchanged served body); a later update onto a FREE priority settles UPDATE_COMPLETE and the served body changes.
Emulator: no response is produced to compare at all — the replay reports 'Check timed out after 60.0s'. No stack status, no stack event and no rule state is ever read back. Note that this cause was previously filed and marked fixed as 20261024T182404Z-elbv2-listener-rule-priority-collision-update-path-never.md (and 20261028T123303Z-elbv2-listener-rule-priority-collision-update-never-resolves.md); that issue's own Resolution states the fix is PARTIAL — the ALB data plane was implemented but CloudFormation still synthesises ELBv2 identifiers instead of creating the resources through the ELBv2 API — and the check still never converges, so the behavior is still divergent and no open issue carries its replay key. Hypothesis: one of the check's polling loops never satisfies its predicate (either the stack never leaves *_IN_PROGRESS, or the data-plane GET through the stack-reported ALB DNS name never returns the expected 200 body because no ELBv2 load balancer exists for the synthesised name). The 60s cutoff is the harness budget for an in-memory emulator; the check's own 900s/420s budgets are real-AWS provisioning times and are not the requirement.
Why: a hung call is worse than a wrong one — a client waiting for a terminal stack status blocks forever, never sees the PriorityInUse-driven UPDATE_FAILED/UPDATE_ROLLBACK_COMPLETE outcome, and cannot rely on the no-partial-effect guarantee for a rejected rule move.
Fix: whatever leaves this flow without a terminal state — the CloudFormation provisioner path for AWS::ElasticLoadBalancingV2::{LoadBalancer,Listener,ListenerRule,TargetGroup} must create those resources through the ELBv2 state the data plane and the rule handlers read, so the stack settles and the colliding update is refused by SetRulePriorities/CreateRule's priority-uniqueness check.

Traces: elbv2.set-rule-priorities-conflict-with-existing-fails, elbv2.set-rule-priorities-duplicate-priority-fails, elbv2.zero-weight-target-group-is-never-selected, elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.

## Evidence
Replay report, service elbv2 (4 failing checks, none marked ALREADY FILED or OWNED):

Cause 1 — [mismatch] elbv2.set-rule-priorities-conflict-with-existing-fails
  check_elbv2_set_rule_priorities_conflict_with_existing_fails.py:196
  AssertionError: expected message 'One or more priorities not found', got "Priority '21' is currently in use"
  (the preceding assertion `code == "PriorityInUse"` passed, so only the message diverges)
[mismatch] elbv2.set-rule-priorities-duplicate-priority-fails
  check_elbv2_set_rule_priorities_duplicate_priority_fails.py:36
  AssertionError: expected message 'One or more priorities not found', got "Priority '10' is currently in use"
  Expected (AWS): HTTP 400, Error.Code 'PriorityInUse', Error.Message 'One or more priorities not found' (entirely fixed text).
  Actual (emulator): HTTP 400, Error.Code 'PriorityInUse', Error.Message "Priority '<requested priority>' is currently in use".

Cause 2 — [mismatch] elbv2.zero-weight-target-group-is-never-selected
  check_elbv2_zero_weight_target_group_is_never_selected.py:562 -> _error_code (line 461)
  AssertionError: call was expected to be rejected but SUCCEEDED, returning
  {'Listeners': [{'ListenerArn': 'arn:aws:elasticloadbalancing:...:listener/app/<lb>/<id>/<lid>', 'Port': 80, 'Protocol': 'HTTP',
   'DefaultActions': [{'Type': 'forward', 'ForwardConfig': {'TargetGroups': [{'TargetGroupArn': '...targetgroup/<tg-a>/<id>', 'Weight': 0},
   {'TargetGroupArn': '...targetgroup/<tg-b>/<id>', 'Weight': 0}], 'TargetGroupStickinessConfig': {'Enabled': False}}}], ...}],
   'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected (AWS): the ModifyListener that zeroes every weight is refused with HTTP 400 Error.Code 'ValidationError' and the stored weights stay at their previous 0/1.
  Actual (emulator): HTTP 200 and the all-zero weighting is echoed and stored.
  (ARNs, load-balancer/target-group names and request ids above are per-run values, not part of the requirement.)
  The check's earlier phases passed: weights 1/0 served only 'TG-A' and 0/1 served only 'TG-B' across 20 requests each.

Cause 3 — [mismatch] elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
  Error: Check timed out after 60.0s
  Expected (AWS): CREATE_COMPLETE + served first-rule body; the colliding update refused with PriorityInUse (HTTP 400) so the stack settles UPDATE_ROLLBACK_COMPLETE with an UPDATE_FAILED event on the rule's logical id and no partial effect; a later update onto a free priority settles UPDATE_COMPLETE.
  Actual (emulator): no stack status, event or rule state observed at all — the check is killed at the harness budget.

Smithy citations (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json):
  SetRulePriorities: input RulePriorities required; errors OperationNotPermittedException, PriorityInUseException {awsQueryError code 'PriorityInUse', httpResponseCode 400}, RuleNotFoundException.
  PriorityInUseException: errorKind client, httpError 400, doc 'The specified priority is in use.', member Message -> ErrorDescription.
  TargetGroupTuple.Weight -> TargetGroupWeight, doc 'The weight. The range is 0 to 999.' (no all-zero constraint modeled).
  ModifyListener: modeled errors do not include a generic validation error, so the all-zero refusal is recorded behavior (ValidationError / 400).

## Steps to Reproduce
Cause 1 (SetRulePriorities message):
1. Boot the emulator. Create a VPC with two subnets, an application load balancer and an HTTP:80 listener.
2. CreateRule twice on that listener: rule A at Priority 10 and rule B at Priority 20 (any path-pattern conditions, fixed-response actions).
3. Positive control: SetRulePriorities moving rule B to a free priority (15) succeeds and DescribeRules shows 15.
4. SetRulePriorities with RulePriorities=[{RuleArn: ruleB, Priority: 10}] — priority 10 is still held by rule A. Observe HTTP 400 PriorityInUse with Message "Priority '10' is currently in use"; AWS answers Message 'One or more priorities not found'. DescribeRules must show neither rule moved (it does not).
5. Free priority 10 (move rule A to 30, or delete it) and repeat the identical call — it must succeed.
6. Re-replay elbv2.set-rule-priorities-conflict-with-existing-fails and elbv2.set-rule-priorities-duplicate-priority-fails.

Cause 2 (all-zero forward weights):
1. Create an ALB, an HTTP:80 listener and two target groups, each with one serving target.
2. ModifyListener with DefaultActions = one forward action over both target groups at weights 1 and 0 — accepted; all traffic lands on the weight-1 group. Repeat with 0 and 1 — accepted; traffic flips.
3. ModifyListener with the same forward action but BOTH weights 0. Observe HTTP 200 and the all-zero config echoed and stored; AWS refuses with HTTP 400 ValidationError and leaves the previous 0/1 weighting in place.
4. DescribeListeners and confirm the stored weights are unchanged after the refusal; GET through the ALB must still serve the surviving group.
5. Re-replay elbv2.zero-weight-target-group-is-never-selected.

Cause 3 (rule-priority-collision UPDATE never terminal):
1. create_stack with a self-contained template: VPC + IGW + two public subnets + security group, an internet-facing ALB, an HTTP:80 listener with a fixed-response default, and two AWS::ElasticLoadBalancingV2::ListenerRule resources — rule A at Priority 10 (fixed-response body 'a1') and rule B at Priority 20. Output the ALB DNSName, the listener ARN and both rule ARNs.
2. Poll describe_stacks until the status leaves *_IN_PROGRESS and GET http://<AlbDns>/a; on AWS this is CREATE_COMPLETE and 200 'a1'. Observe that the emulator never converges here.
3. update_stack with rule A moved to Priority 20 and body 'a2'. On AWS ELBv2 refuses with PriorityInUse (400), the stack settles UPDATE_ROLLBACK_COMPLETE with an UPDATE_FAILED event for rule A, and rule A keeps its ARN, Priority 10 and body 'a1'.
4. update_stack again with rule A at free Priority 15 and body 'a2': UPDATE_COMPLETE, same rule ARN, ALB serves 'a2'.
5. Re-replay elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.
