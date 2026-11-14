Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8e9edc8d240f3f1cb2351290b9e21b75ec6080be

# An AssociatedValues entry is stored over billing periods the NAMED resource applies in in no generation, so a period reports an association to an item it reports no version of

_Filed: 2026-11-12T09:01:28Z_
_Operation: CreateCustomLineItem | Class: validation_

> _Note: this issue was identified against the code base at commit `8e9edc8d240f3f1cb2351290b9e21b75ec6080be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCustomLineItem` resolves each `ChargeDetails.Percentage.AssociatedValues` entry and stores a `ResourceAssociationRecord` whose span is the NEW item's own `BillingPeriodRange`. The only things it checks about the named resource are that it exists (`resolve_association_element`) and that it is in scope of the same billing group (`out_of_scope_association`). It never compares the span it is about to store against the billing periods the NAMED resource itself applies in, so a create whose range reaches past the periods a child custom line item applies in stores an association that is in effect in periods no generation of that child covers.

That is the exact state this crate's own writer goes out of its way to prevent: `remove_custom_line_item_periods` narrows every association keyed on an item by the periods the item loses, and states the invariant flatly — an association is in effect only in periods the resource it names is there to be associated in, so "no read of that period can report an association to an item it reports no version of". After such a create that is precisely what happens: `ListResourcesAssociatedToCustomLineItem` scoped to that period reports the child (its only test is `association.covers(billing_period)`), while `ListCustomLineItems` for the same period reports no such item and `ListCustomLineItemVersions` reports no version covering it.

This is the OTHER side of the association from the already-open `BatchAssociateResourcesToCustomLineItem` finding (`20261109T105402Z-...-batchassociateresourcestocustomlineitem-validation--r2`), which compares the request's range against the TARGET item's periods and explicitly states that `CreateCustomLineItem` "can never reach past the item". That is true of the target only: the create derives the association span from the target's own range, but neither path compares it against the periods the ASSOCIATED resource applies in, so the fix proposed there does not close this hole.

## Evidence

The invariant the crate states, in the writer that maintains it — `src/state.rs:3437-3447`:

    /// A resource association is in effect only in billing periods the resource it names
    /// is there to be associated in, so narrowing the periods a resource applies in
    /// narrows by the same periods every association keyed on that resource — the ones
    /// the resource itself holds and the ones other resources hold TO it — and an
    /// association left in effect in no period at all goes
    /// ([`remove_association_periods`]). It is the bounded form of the rule a delete that
    /// names no range enforces by removing the record outright
    /// ([`Self::remove_custom_line_item`]): a period the item applies in in no generation
    /// of it holds no association of it either, so no read of that period can report an
    /// association to an item it reports no version of.

The create resolves each entry and stores the span with no comparison against that resource's own periods — `src/ops/create_custom_line_item.rs:312-355`:

        for value in &associated_values {
            let Some(arn) = state.resolve_association_element(value) else { ... };
            // The resources a percentage item's charge is computed over are the ones
            // its own billing group carries: that group, and the custom line items of
            // that same group.
            if let Some(fault) = state.out_of_scope_association(&billing_group_arn, &arn) { ... }
            ...
            associated_resources.push(crate::state::ResourceAssociationRecord::new(
                arn,
                crate::state::BillingPeriodSpan::new(
                    period_range
                        .as_ref()
                        .map(|range| range.inclusive_start_billing_period.clone()),
                    period_range
                        .as_ref()
                        .and_then(|range| range.exclusive_end_billing_period.clone()),
                ),
            ));

`out_of_scope_association` is the only predicate applied to the named resource, and it reads the resource's BILLING GROUP, never its periods — `src/state.rs:3768-3779`:

        match self.custom_line_items.get(resource_arn) {
            Some(item) => (item.billing_group_arn != billing_group_arn)
                .then_some(OutOfScopeAssociation::OtherGroupsCustomLineItem),
            None => (resource_arn != billing_group_arn)
                .then_some(OutOfScopeAssociation::OtherBillingGroup),
        }

The reader that then disagrees with the named item's own periods — `src/ops/list_resources_associated_to_custom_line_item.rs:212-217`:

        for association in &item.associated_resources {
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }

against the item listing for the same period — `src/ops/list_custom_line_items.rs:297-302`:

            // The version of the item in effect in the billing period asked about is
            // what this read reports; an item no version of which applies in that period
            // is not reported at all.
            let Some(version) = item.effective_in(&billing_period) else {
                continue;
            };

Test coverage: `tests/all/percentage_custom_line_item_resource_associations.rs` and `tests/all/resource_association_billing_periods.rs` cover associations whose ranges lie inside the periods both items apply in; `tests/all/bounded_delete_narrows_resource_associations.rs` pins the narrowing half of the invariant (`src/state.rs:3437-3447`). No test creates a percentage item whose range outlives the periods an `AssociatedValues` child applies in.

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as the suites do: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`).
2. POST /create-billing-group with `ComputationPreference.PricingPlanArn = arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan`.
3. POST /create-custom-line-item for the CHILD: `{Name:"child", Description:"d", BillingGroupArn:<group>, ChargeDetails:{Type:"FEE", Flat:{ChargeValue:10}}, BillingPeriodRange:{InclusiveStartBillingPeriod:<previous billing period>, ExclusiveEndBillingPeriod:<current billing period>}}` — the child applies in the PREVIOUS period only.
4. POST /create-custom-line-item for the PARENT: `{Name:"parent", Description:"d", BillingGroupArn:<group>, ChargeDetails:{Type:"FEE", Percentage:{PercentageValue:10, AssociatedValues:[<child arn>]}}, BillingPeriodRange:{InclusiveStartBillingPeriod:<previous billing period>}}` — open-ended, so the parent applies in the previous period AND the current one -> HTTP 200.
5. POST /list-resources-associated-to-custom-line-item with `{Arn:<parent>, BillingPeriod:<current billing period>}` -> the child is reported as an associated resource.
6. POST /list-custom-line-items with `{BillingPeriod:<current billing period>, Filters:{Arns:[<child>]}}` -> empty, and POST /list-custom-line-item-versions with `{Arn:<child>}` -> no version whose [StartBillingPeriod, EndBillingPeriod) covers the current period.
7. Observed: the current period reports an association to an item it reports no version of. Expected (per `src/state.rs:3443-3447`): the association is not in effect in a period the named resource applies in in no generation — either the entry is refused (the create already refuses an out-of-scope or duplicate entry with `ValidationException`) or the stored span is bounded to the periods the named resource applies in.
