Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 89bd99e3e826befdb8e4111b5c2027c562b7fa6c

# A deleted pricing rule leaves its plan associations behind, so a plan keeps reporting an association to a rule that exists nowhere

_Filed: 2026-10-28T23:02:25Z_
_Operation: DeletePricingRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `89bd99e3e826befdb8e4111b5c2027c562b7fa6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePricingRule` removes the rule record, its name-index entry and the idempotency entries that named it — and nothing else. The `pricing_rule_associations` that pricing plans hold TO that rule are left in place, so after the delete a plan still carries association records naming a rule the store no longer holds.

The crate states and implements the opposite rule for its two other resource removals: `remove_billing_group` and `remove_custom_line_item` both strip every association naming the removed resource, with the reason spelled out in the comment — "an association names a resource that exists". `remove_pricing_rule` is the one removal that skips that step, although a pricing plan's rule association is exactly such an association.

The dangling record is reachable because the delete gate only looks at the associations in force in the CURRENT billing period: a rule associated in one billing period and disassociated in a LATER one leaves an ENDED association record (in force only in the earlier periods, kept deliberately so a read of a closed period still reports it), and that record no longer blocks the delete. After the delete:

* `ListPricingRulesAssociatedToPricingPlan` with `BillingPeriod` = the earlier period reports the deleted rule's ARN, because `associated_rule_arns` filters only on whether the association covers the period, never on whether the rule still exists;
* `ListPricingPlans` counts that phantom association in the plan's `Size` for the same period;
* `ListPricingPlansAssociatedWithPricingRule` for that very ARN answers 404 "the pricing rule does not exist".

So two readers of the same association disagree, and one of them hands the caller an ARN that resolves to nothing. The fix is the step the crate's other two removals already take, applied where the rule's associations live.

## Evidence

The removal that skips it — `src/state.rs:3033-3047`:
```
    /// Delete a pricing rule and the index entries that only existed because of
    /// it: its name-index entry and the idempotency entries that named it, so a
    /// `ClientToken` cannot replay into the ARN of a rule that no longer exists.
    /// `None` when no such rule is stored.
    pub fn remove_pricing_rule(&mut self, rule_arn: &str) -> Option<PricingRuleRecord> {
        let removed = self.pricing_rules.remove(rule_arn)?;
        self.pricing_rule_names.remove(&removed.name);

        self.idempotency_tokens.retain(|_, arn| arn != rule_arn);
```

The rule the sibling removals state and apply — `src/state.rs:2766-2784`:
```
        // A custom line item "applies to" one billing group, so it cannot outlive
        // it; and an association names a resource that exists, so every association
        // of the deleted group or of one of its items goes with them.
...
        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != group_arn);
        }
```
and `src/state.rs:2860-2871`:
```
    /// Delete a custom line item and everything that only existed because of it: the
    /// associations other percentage custom line items held TO it (an association
    /// names a resource that exists, and its own associations go with the record) ...
    pub fn remove_custom_line_item(&mut self, item_arn: &str) -> Option<CustomLineItemRecord> {
        let removed = self.custom_line_items.remove(item_arn)?;

        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != item_arn);
        }
```

Why an ended association does not block the delete — `src/ops/delete_pricing_rule.rs:183-187`:
```
        // caller disassociates the rule from its plans first. The delete takes effect
        // in the current billing period, so the associations that block it are the
        // ones in force then.
        let holders = state
            .pricing_plans_referencing_rule(&rule_arn, &crate::state::current_billing_period());
```
and the ended records are kept on purpose — `src/state.rs:826-830`:
```
    /// The rules this plan associates, each with the billing periods its association
    /// is in force over — including the ones that have ENDED, which a read scoped to a
    /// period they were in force in still reports; see
    /// [`PricingRuleAssociationRecord`].
    pub pricing_rule_associations: Vec<PricingRuleAssociationRecord>,
```

The reader that then reports the phantom — `src/state.rs:848-856`:
```
    pub fn associated_rule_arns(&self, billing_period: &str) -> Vec<String> {
        self.pricing_rule_associations
            .iter()
            .filter(|association| association.covers(billing_period))
            .map(|association| normalize_pricing_rule_arn(&association.arn))
```
(no existence check), served by `src/ops/list_pricing_rules_associated_to_pricing_plan.rs:200-212`.

Test coverage: none. `tests/all/pricing_rule_lifecycle.rs:379` (`delete_removes_exactly_the_rule_its_arn_names`) and `tests/all/disassociate_pricing_rules.rs:319-355` only delete rules whose associations were dropped within one billing period (`end_rule_associations` drops a record that covers no period, `src/state.rs:886-887`), so no suite reaches the ended-association case.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. In billing period M: POST /create-pricing-rule (keep the rule ARN R) and POST /create-pricing-plan (keep the plan ARN P), then PUT /associate-pricing-rules with {"Arn": P, "PricingRuleArns": [R]}.
3. Confirm POST /list-pricing-rules-associated-to-pricing-plan with {"PricingPlanArn": P, "BillingPeriod": M} reports R.
4. In the NEXT billing period M+1: PUT /disassociate-pricing-rules with {"Arn": P, "PricingRuleArns": [R]} — the association record is kept, ended at M+1, so period M still reports it.
5. Still in M+1: POST /delete-pricing-rule with {"Arn": R} — answered 200, because the only blocking associations are the ones in force in M+1.
6. POST /list-pricing-rules-associated-to-pricing-plan with {"PricingPlanArn": P, "BillingPeriod": M} — it still reports R, and POST /list-pricing-plans with {"BillingPeriod": M} counts it in P's Size, while POST /list-pricing-rules with {"BillingPeriod": M} does not report R and POST /list-pricing-plans-associated-with-pricing-rule with {"PricingRuleArn": R} answers 404 "the pricing rule does not exist".
7. Compare: DeleteBillingGroup and DeleteCustomLineItem strip every association naming the resource they remove, so no reader of theirs can report one.
