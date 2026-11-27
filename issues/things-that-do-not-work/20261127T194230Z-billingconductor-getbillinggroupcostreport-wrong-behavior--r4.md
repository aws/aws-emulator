Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# A percentage chain that overflows f64 makes the margin summary report the strings "inf" and "NaN", breaking the module's own "undefined ... reported as zero" rule

_Filed: 2026-11-27T19:42:30Z_
_Operation: GetBillingGroupCostReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pro forma cost of a billing group is settled by multiplying each percentage custom line item's `PercentageValue` into the combined charge of the items associated to it, with no guard on the result being finite. Every input needed to overflow an `f64` is inside the modelled ranges and inside this crate's own caps: `CustomLineItemChargeValue` admits up to 1,000,000, `CustomLineItemPercentageChargeValue` admits up to 10,000 (i.e. a x100 multiplier per level of the chain), and the store admits `MAX_CUSTOM_LINE_ITEMS: usize = 1000` items. A chain of ~153 items therefore settles to `+inf`.

Two things then go wrong in the one margin-summary computation BOTH cost reports share (`MarginSummary::of_proforma`, used by `GetBillingGroupCostReport` and `ListBillingGroupCostReports`):

1. `margin_percentage` becomes `inf / inf * 100.0`, i.e. `NaN`. The module states the rule for an undefined percentage explicitly — "undefined, and so reported as zero, where there is no pro forma amount to take a share of" — and guards exactly one way of being undefined (`proforma_cost == 0.0`). An infinite pro forma amount is the other, and the code under that sentence reports `NaN` instead of the zero the sentence promises.
2. `reported_amount` renders the value with `format!("{rounded}")`, which prints `inf` and `NaN` verbatim. `AWSCost` / `ProformaCost` / `Margin` / `MarginPercentage` are modelled strings whose stated contract in this very module is "a decimal rounded to [REPORTED_DECIMALS] places"; `"inf"` and `"NaN"` are not decimals at all, so a client parsing the amount gets a value it cannot read, on a 200 response that claims to have computed the report.

The defect is that the computation has no finiteness guard: neither the settle step nor the one renderer every reported amount of this service passes through decides what to report when the arithmetic leaves the representable range, so the response carries the raw floating-point sentinel.

## Evidence

The rule the code under it breaks, in the crate's own words (services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:800-811):

```
    /// The margin a billing group earns on a set of charges is what it is charged for
    /// them (pro forma) less what they actually cost (AWS), and the margin percentage is
    /// that margin as a share of the pro forma amount — undefined, and so reported as
    /// zero, where there is no pro forma amount to take a share of.
    pub fn of_proforma(proforma_cost: f64) -> Self {
        let margin = proforma_cost - METERED_AWS_COST;
        let margin_percentage = if proforma_cost == 0.0 {
            0.0
        } else {
            margin / proforma_cost * 100.0
        };
```
With `proforma_cost == f64::INFINITY` the `== 0.0` guard does not fire and `margin / proforma_cost` is `inf / inf` = `NaN` — undefined, and NOT reported as zero.

The renderer, which states the contract it then breaks (state.rs:772-780):

```
/// An amount as a reported `AWSCost` / `ProformaCost` / `Margin` / `MarginPercentage`
/// string carries one: a decimal rounded to [`REPORTED_DECIMALS`] places.
pub fn reported_amount(value: f64) -> String {
    let scale = 10f64.powi(REPORTED_DECIMALS);
    // Adding zero normalises a negative zero (a credit cancelling a fee) to the one
    // amount it is, so no row reports `-0`.
    let rounded = (value * scale).round() / scale + 0.0;
    format!("{rounded}")
}
```
`(inf * scale).round() / scale + 0.0` is `inf`, and `(NaN * scale).round() / scale + 0.0` is `NaN`; `format!` prints them as `inf` and `NaN`. Note the author did add a guard for one special floating-point value here (negative zero), so the special-value cases are in scope for this function.

The unguarded multiplication (src/state/charges.rs:276-286):

```
                    charges[item] = match items[item].percentage {
                        None => items[item].amount,
                        Some(percentage) => {
                            let combined: f64 = items[item]
                                .lenders
                                .iter()
                                .map(|lender| charges[*lender])
                                .sum();
                            percentage / 100.0 * combined
                        }
                    };
```

The inputs are all modelled or crate-sanctioned:
* `smithy_query(query="shape",shape="CustomLineItemFlatChargeDetails")`: `ChargeValue` ... `"range": {"min": 0, "max": 1000000}`;
* `smithy_query(query="shape",shape="CustomLineItemPercentageChargeDetails")`: `PercentageValue` ... `"range": {"min": 0, "max": 10000}` — "This will be multiplied against the combined value of its associated resources to determine its charge value";
* state.rs:4601 `pub const MAX_CUSTOM_LINE_ITEMS: usize = 1000;` and state.rs:4621 `pub const MAX_RESOURCES_PER_CUSTOM_LINE_ITEM: usize = MAX_CUSTOM_LINE_ITEMS;`, so a 153-deep chain is well inside the store's own bounds.

Both consumers pass the summary straight to the renderer, so both report the sentinel: src/ops/get_billing_group_cost_report.rs:411-421 (`MarginSummary::of_proforma(row.proforma_cost)` then `crate::state::reported_amount(summary.proforma_cost)` / `reported_amount(summary.margin_percentage)`) and src/ops/list_billing_group_cost_reports.rs:248-256 (the same two calls).

Test coverage: `tests/all/billing_group_cost_report.rs`, `tests/all/billing_group_cost_report_period_scope.rs` and `tests/all/cost_report_mutual_percentage_associations.rs` exercise the margin summary and the percentage graph, but none of them drives the amounts out of the representable range, so nothing pins the current `inf`/`NaN` rendering.

## Steps to Reproduce

1. CreateBillingGroup (any valid AccountGrouping/ComputationPreference) and note its Arn.
2. CreateCustomLineItem in that billing group with ChargeDetails = { Type: FEE, Flat: { ChargeValue: 1000000 } } — the modelled maximum. Note its Arn as item_1.
3. For k = 2..=153, CreateCustomLineItem in the same billing group with ChargeDetails = { Type: FEE, Percentage: { PercentageValue: 10000, AssociatedValues: [item_{k-1}] } } — each value is inside the modelled range, and 153 items is inside MAX_CUSTOM_LINE_ITEMS = 1000. Each level multiplies the previous charge by 100, so item_153 settles to 1e6 * 100^152 > f64::MAX.
4. Call GetBillingGroupCostReport with the billing group's Arn (no GroupBy).
5. Observe the response row: ProformaCost and Margin are the string "inf" and MarginPercentage is the string "NaN", instead of decimal amounts — and MarginPercentage in particular is an undefined share, which state.rs:803-804 states is "reported as zero".
6. Call ListBillingGroupCostReports for the same billing period: the same group's element carries the same "inf"/"NaN" strings, since both operations render through MarginSummary::of_proforma and reported_amount.
