Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0a5d9cda53656d0e20014bdf39cff0e297709330

# The BILLING_PERIOD breakdown attribute reports the raw YYYY-MM billing period where its modelled doc states the month-name form

_Filed: 2026-11-20T00:35:47Z_
_Operation: GetBillingGroupCostReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0a5d9cda53656d0e20014bdf39cff0e297709330`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A margin-summary row broken down by `GroupBy: ["BILLING_PERIOD"]` carries an `Attributes` entry whose `Key` is `BILLING_PERIOD`. The handler sets that entry's `Value` to the billing-period string the request/range enumerates — the canonical `YYYY-MM` spelling `billing_periods_in_range` renders (`2023-11`). The member's own Smithy documentation states the value for that very case: for a breakdown of a billing group **for November 2023**, the attributes list "will contain ... a key-value pair of `"BILLING_PERIOD"` and `"Nov 2023"`". So the one documented example of this member's value gives `Nov 2023`, and the handler emits `2023-11` instead. Nothing in the crate — neither the handler's own module header (which discusses the rows, the periods and the token scope at `src/ops/get_billing_group_cost_report.rs:1-19`) nor any comment beside the write — states a reason for departing from that documented value, and the model text is not cited anywhere in the crate. The companion `PRODUCT_NAME` attribute in the same sentence is honoured as documented (a product name as a string), so only the period attribute's rendering diverges. The report's cursor key is derived separately from `billing_period`/`product_name` (`row_key`, `src/ops/get_billing_group_cost_report.rs:131-135`), so the reported attribute value is not what orders or pages the rows: only the value a client reads and displays is affected.

## Evidence

Smithy model, `BillingGroupCostReportResultElement$Attributes` (smithy_query query="shape" shape="BillingGroupCostReportResultElement"), verbatim: "The list of key-value pairs that represent the attributes by which the BillingGroupCostReportResults are grouped. For example, if you want the Amazon S3 service-level breakdown of a billing group for November 2023, the attributes list will contain a key-value pair of \"PRODUCT_NAME\" and \"S3\" and a key-value pair of \"BILLING_PERIOD\" and \"Nov 2023\"."

Handler, services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/get_billing_group_cost_report.rs:350-359 (the pre-seeded per-period rows):
```
                    rows.insert(
                        row_key(Some(billing_period), None),
                        ReportRow {
                            attributes: vec![Attribute {
                                key: Some(GROUP_BY_BILLING_PERIOD.to_string()),
                                value: Some(billing_period.clone()),
                            }],
```
and the same value on the rows the charges create, src/ops/get_billing_group_cost_report.rs:374-383:
```
                    attributes: group_by
                        .iter()
                        .map(|attribute| Attribute {
                            key: Some((*attribute).to_string()),
                            value: Some(if *attribute == GROUP_BY_BILLING_PERIOD {
                                billing_period.clone()
```
`billing_period` here is an element of `billing_periods`, which comes from `crate::state::billing_periods_in_range` — documented at src/state.rs:230-232 as "The billing periods of the half-open range ... each in the canonical `YYYY-MM` spelling" and rendered by `billing_period_of_key` as `format!("{year:04}-{month:02}")` (src/state.rs:208-212). So a November 2023 row reports `2023-11`.

Test coverage that currently pins the `YYYY-MM` form (assertion only, no reason given for the divergence from the member's documented value): tests/all/billing_group_cost_report.rs:1277-1282 — `assert_eq!(attributes(row), vec![("BILLING_PERIOD".to_string(), billing_period.clone())], "case {row_case:?}: expected the canonical period as the row's one attribute, got {row}")`; also tests/all/billing_group_cost_report.rs:706 and :863, and tests/all/custom_line_item_versions_by_billing_period.rs:259-284.

## Steps to Reproduce

1. CreateBillingGroup, then CreateCustomLineItem with a flat FEE charge in that group so the group has a charge in the current billing period.
2. POST /get-billing-group-cost-report with {"Arn": <group arn>, "GroupBy": ["BILLING_PERIOD"]}.
3. Read the one row's `Attributes`: the entry with Key `BILLING_PERIOD` carries Value `2026-01` (the canonical `YYYY-MM` spelling of the period).
4. Compare with the member's modelled documentation, which states that for November 2023 the pair is `"BILLING_PERIOD"` / `"Nov 2023"` — i.e. the month-name form, not `YYYY-MM`.
