Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.rule-action-overrides-downgrade-a-rule-group-block-to-count-at-the-edge
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.rule-action-overrides-downgrade-a-rule-group-block-to-count-at-the-edge|_poll#47|Timeout|time.sleep(interval_s)
Issue-Commit: 64d0ad8ca6aaaab9abdf38629b1249e9b439a361

# wafv2 data plane: a rule group reference's RuleActionOverrides / OverrideAction downgrade is never observable at the edge — the check never reaches a verdict

> _Note: this issue was identified against the code base at commit `64d0ad8ca6aaaab9abdf38629b1249e9b439a361`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSampledRequests

## Description
One root cause (Cause 1 of 1): with a REGIONAL web ACL associated to an API Gateway REST stage, none of the observable effects of a RuleGroupReferenceStatement become visible, so the check that exercises the two documented override levers never reaches any assertion verdict — it is still polling when the 60 s harness cap kills it.

AWS: Smithy (com.amazonaws.wafv2) models the two independent levers this check drives and the record WAF keeps of what it did. RuleGroupReferenceStatement.RuleActionOverrides is documented as "Action settings to use in the place of the rule actions that are configured inside the rule group" — i.e. an override with ActionToUse={"Count":{}} replaces the referenced rule's own Block, so the matching request is counted and served rather than refused. OverrideAction.Count is documented as a separate lever that "Override[s] the rule group evaluation result to count only" and explicitly "does not affect how the rules in the rule group are evaluated" — same edge outcome, different field. SampledHTTPRequest.Action is "The action that WAF applied to the request" and SampledHTTPRequest.OverriddenAction is "Used only for rule group rules that have a rule action override in place in the web ACL. This is the action that the rule group rule is configured for, and not the action that was applied" — so with an override to Count in place the sample must report Action COUNT and OverriddenAction BLOCK. SampledHTTPRequest.RuleNameWithinRuleGroup is documented as "<rule group name>#<rule name>" for a customer-owned rule group. Against real AWS each of these effects becomes observable within the check's own budgets (association ~40 s, rule propagation 20–30 s). Hypothesis for the split of blame between the legs is not available from this run: the harness cap cut the check before any assertion ran, so the only measurement is that no polled effect was reached.

Emulator: the check ends as `Error: Check timed out after 60.0s` with no assertion message at all. Its bounded polls are the only things in it that can consume wall clock: AssociateWebACL becoming available (420 s budget), the referenced rule group's own Block producing 403 on four successive requests (360 s), GetSampledRequests returning a sample under RuleMetricName 'groupref' carrying the request's marker header (300 s), the same request being served 200 once RuleActionOverrides sets the inner rule to Count (360 s), the same with OverrideAction Count on the whole reference (360 s), and the 403 returning once both overrides are removed (360 s). None completed inside 60 s, so at minimum the first of them — the rule group reference reaching the edge, or the sample store that records it — never produces its observable effect.

Why: overrides are how every real WAF rollout is staged (ship a rule group to Count, watch the metrics, then promote it). A client that stores RuleActionOverrides or OverrideAction against the emulator gets a 200 from the control plane and a stored configuration, but the edge result and WAF's own account of it (Action vs OverriddenAction, RuleNameWithinRuleGroup) are indistinguishable from an emulator that parses the field and ignores it. Nothing a caller can read tells it whether a staged rollout is in Count or in Block.

Fix: the data-plane rule-evaluation path for RuleGroupReferenceStatement (apply per-rule RuleActionOverrides to the referenced rule's action, and apply OverrideAction Count to the group's result independently of it) and the GetSampledRequests sample store it feeds (record Action as the APPLIED action, OverriddenAction as the CONFIGURED one, and RuleNameWithinRuleGroup as '<rule group>#<rule>'), for a web ACL associated with an API Gateway REST stage.

Related open issue, which does NOT cover this failure: 20261030T002309Z-wafv2-data-plane-the-captcha-challenge-edge-response.md names this same check in its trace list, but the replay report did not mark this check OWNED or ALREADY FILED — its recorded replay key no longer matches this run's failure (the current failure carries no traceback frame, only the 60 s cap), so it is filed here as the grounded failure it is. Also related: 20261029T103720Z-apigateway-wafv2-wafv2-getsampledrequests-rulenamewithinrulegroup-reports-the-bare-rule-name.md (RuleNameWithinRuleGroup form for managed rule groups).

Traces: wafv2.rule-action-overrides-downgrade-a-rule-group-block-to-count-at-the-edge.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator (152/194 passed; wafv2 142/175). Failing check, verdict mismatch, not marked ALREADY FILED and not marked OWNED:

  [mismatch] wafv2.rule-action-overrides-downgrade-a-rule-group-block-to-count-at-the-edge
  Error: Check timed out after 60.0s

Expected (AWS), per the check's legs and the Smithy docs quoted above:
  * leg 1: GET the stage URL with header x-waf-probe=<matching prefix> -> 403 on four successive requests (the referenced rule group's own Block), body must not contain the origin marker; GetSampledRequests(RuleMetricName='groupref') must yield a sample with Action == "BLOCK" and RuleNameWithinRuleGroup == "<rule group name>#innerblock".
  * leg 1b: a non-matching x-waf-probe value must still be served 200 with the origin marker.
  * leg 2: after UpdateWebACL stores RuleActionOverrides [{Name: "innerblock", ActionToUse: {Count: {}}}] on the same reference, the same matching request must be served 200 with the origin marker, and the sample must report Action == "COUNT" with OverriddenAction == "BLOCK".
  * leg 3: with no per-rule override but OverrideAction {Count: {}} on the whole reference, the matching request must again be served 200.
  * leg 4: with both overrides removed, the identical matching request must be refused 403 again, and GetWebACLForResource must still report the web ACL attached to the stage ARN.

Actual (emulator): no leg produced a verdict. The check reports only the harness cap, so no status, error code or message is available for any leg; every one of its waits is a bounded poll on an observable WAF effect (edge status transition or a GetSampledRequests sample), none of which was reached in 60 s. The marker names and nonces in the check are per-run values and are not part of the requirement; the requirements are the statuses (403 / 200), the sample fields (Action, OverriddenAction) and the '<rule group>#<rule>' form of RuleNameWithinRuleGroup.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite (run_behavioral_suite); the check id in trace_ids reproduces this.
2. Manually: create an API Gateway REST API (REGIONAL) with a MOCK integration under /override returning a JSON body containing a marker, create a deployment and a stage, and confirm GET <stage url>/override returns 200 with the marker BEFORE WAF is attached.
3. CheckCapacity(Scope=REGIONAL, Rules=[rule 'innerblock' Priority 0, Action Block, ByteMatchStatement on SingleHeader x-waf-probe STARTS_WITH <prefix>]) and CreateRuleGroup with that capacity and that single rule.
4. CreateWebACL(Scope=REGIONAL, DefaultAction=Allow, Rules=[rule 'groupref' Priority 0, Statement.RuleGroupReferenceStatement={ARN: <rule group ARN>}, OverrideAction={None:{}}]) and AssociateWebACL(WebACLArn, ResourceArn=arn:aws:apigateway:<region>::/restapis/<api id>/stages/<stage>).
5. GET <stage url>/override with x-waf-probe=<prefix>-a... -> expect 403 and no origin marker in the body; then GetSampledRequests(WebAclArn, RuleMetricName='groupref', Scope=REGIONAL, TimeWindow=now-15m..now+1m) -> expect a sample whose Request.Headers carry that probe value, Action == "BLOCK", RuleNameWithinRuleGroup == "<rule group name>#innerblock". A non-matching x-waf-probe value must be served 200.
6. UpdateWebACL (fresh LockToken) with the same reference plus RuleActionOverrides=[{Name:"innerblock", ActionToUse:{Count:{}}}]; GetWebACL must round-trip the overrides, and the SAME matching request must now be served 200 with the origin marker, with GetSampledRequests reporting Action == "COUNT" and OverriddenAction == "BLOCK".
7. UpdateWebACL again with no RuleActionOverrides but OverrideAction={Count:{}} -> the matching request must still be served 200. Then UpdateWebACL back to OverrideAction={None:{}} with no overrides -> the matching request must be refused 403 again, and GetWebACLForResource(ResourceArn=<stage ARN>) must still return the web ACL ARN.
8. Against the emulator none of steps 5–7 is reachable: the check consumes the whole 60 s harness window polling and is killed with `Check timed out after 60.0s`.
