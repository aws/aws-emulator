// SPDX-License-Identifier: CC0-1.0

//! `GetBillingGroupCostReport` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` (-> `BatchAssociateResourcesToCustomLineItem`) ->
//! `GetBillingGroupCostReport`.
//!
//! RULE: the margin summary of a billing group REPORTS, for each group of the breakdown
//! `GroupBy` names over the billing periods the half-open `BillingPeriodRange` covers —
//! the billing period the call is made in when the request names no range — the actual
//! Amazon Web Services charges of the group (its accounts' metered usage, which this
//! emulator records none of), the pro forma charges computed from the stored custom line
//! items in effect in those periods (a flat item contributes its charge value and a
//! percentage item its percentage of the combined value of the resources associated to
//! it, each signed by its `FEE`/`CREDIT` type and counted under the product its
//! presentation details name), the margin between the two and that margin as a share of
//! the pro forma amount; it writes nothing, so a later read sees exactly what the first
//! one saw.
//!
//! Every group of that breakdown the request itself enumerates is a row: the billing group
//! as a whole when `GroupBy` names nothing, and each billing period the range covers when
//! it names `BILLING_PERIOD` — reported with its all-zero amounts where the group holds no
//! charge there. The products the charges are presented under are the one attribute nothing
//! enumerates, so that breakdown has the rows the stored charges name.
//!
//! The rows span what the rule generalises over rather than one call: the charge kind of
//! an item (flat / percentage / a percentage of a percentage / a percentage of the
//! billing group), its type (fee / credit), its value (zero, fractional, whole), how many
//! items a group holds, how the association was made (at create / by a later batch call),
//! how the associations between items are arranged (a chain, two items sharing one base,
//! two items feeding each other), where an item's own billing period range falls relative
//! to the requested one, which attributes the breakdown names and in which order, how
//! many periods the range covers (one, several, the published maximum and one past it),
//! how a billing period is spelled (`2026-1` / `2026-01`), which form the billing group
//! ARN takes (full ARN / bare id) and the page size a caller asks for.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// The product a custom line item with no `PresentationDetails` presents its charges
/// under: the name this service's own model gives it
/// (`smithy_query(query="service_info")`: `AWSBillingConductor`).
const OWN_PRODUCT_NAME: &str = "AWSBillingConductor";

/// The currency of the `aws` partition every ARN this emulator mints lives in
/// (`CurrencyCode` has exactly `CNY` and `USD`;
/// `smithy_query(query="shape",shape="CurrencyCode")`).
const PARTITION_CURRENCY: &str = "USD";

/// How far two amounts may differ and still be the same amount: the reported strings are
/// decimals rounded well inside this, so a comparison at this tolerance pins the amount
/// without pinning the rendering.
const TOLERANCE: f64 = 1e-6;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): the store records no payer account by default, so this harness designates
    /// one — the world in which the read under test is reachable at all. The denial every
    /// other caller gets is asserted in `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send a JSON body to one of this service's routes with the method the model binds
    /// that route to, and return the status plus the parsed response body.
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a billing group and return its ARN.
    async fn billing_group(&self, name: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PRICING_PLAN },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case billing group {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// Create a custom line item from the members `request` carries, filling in the ones
    /// every create needs, and return its ARN.
    async fn custom_line_item(&self, group_arn: &str, name: &str, request: Value) -> String {
        let mut body = json!({
            "Name": name,
            "Description": format!("the {name} custom line item"),
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
        });
        for (member, value) in members(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/create-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {response}"
        );
        arn_of(&response, &format!("custom line item {name:?}"))
    }

    /// Associate `resources` to the percentage custom line item `target`.
    async fn associate(&self, target: &str, resources: &[String]) {
        let (status, body) = self
            .send(
                "PUT",
                "/batch-associate-resources-to-custom-line-item",
                &json!({ "TargetArn": target, "ResourceArns": resources }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case associate {resources:?} to {target}: expected HTTP 200, got {status} with {body}"
        );
    }

    /// `GetBillingGroupCostReport` with whatever members the caller wants on the request.
    async fn report(&self, request: Value) -> (u16, Value) {
        self.send("POST", "/get-billing-group-cost-report", &request)
            .await
    }

    /// The rows of a SUCCESSFUL report, in response order.
    async fn rows(&self, case: &str, request: Value) -> Vec<Value> {
        let (status, body) = self.report(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: one page holds every row here, so no NextToken is due; got {body}"
        );
        results(&body)
    }
}

/// The members of a JSON object, or none when it is not one.
fn members(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// The elements of a report's `BillingGroupCostReportResults`, in response order.
fn results(body: &Value) -> Vec<Value> {
    body.get("BillingGroupCostReportResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One reported amount, read as the decimal its string carries.
fn amount(row: &Value, member: &str, case: &str) -> f64 {
    let text = row
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {row}"));
    text.parse::<f64>().unwrap_or_else(|_| {
        panic!("case {case:?}: expected {member} to be a decimal amount, got {text:?}")
    })
}

/// Assert one reported amount, naming the case and the actual value.
fn assert_amount(row: &Value, member: &str, expected: f64, case: &str) {
    let actual = amount(row, member, case);
    assert!(
        (actual - expected).abs() < TOLERANCE,
        "case {case:?}: expected {member} {expected}, got {actual} in {row}"
    );
}

/// Assert the whole money side of one row: the actual cost is zero, the pro forma cost is
/// `proforma`, the margin is the difference and the margin percentage is that margin as a
/// share of the pro forma amount (zero where there is no pro forma amount).
fn assert_summary(row: &Value, group_arn: &str, proforma: f64, case: &str) {
    assert_eq!(
        row.get("Arn").and_then(Value::as_str),
        Some(group_arn),
        "case {case:?}: expected the billing group's own Arn, got {row}"
    );
    assert_eq!(
        row.get("Currency").and_then(Value::as_str),
        Some(PARTITION_CURRENCY),
        "case {case:?}: expected the partition's currency, got {row}"
    );
    assert_amount(row, "AWSCost", 0.0, case);
    assert_amount(row, "ProformaCost", proforma, case);
    assert_amount(row, "Margin", proforma, case);
    let expected_percentage = if proforma == 0.0 { 0.0 } else { 100.0 };
    assert_amount(row, "MarginPercentage", expected_percentage, case);
}

/// The `Attributes` of one row as `(Key, Value)` pairs, in response order.
fn attributes(row: &Value) -> Vec<(String, String)> {
    row.get("Attributes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|attribute| {
                    (
                        attribute
                            .get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        attribute
                            .get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The bare resource id of a full ARN — the OTHER form the `BillingGroupArn` pattern
/// admits, so a request may name the group either way.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for
/// earlier), in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// A custom line item may be created only into the current or the previous billing
/// period, which the wall clock decides, so every span an item here is created with is
/// named relative to the service's own current billing period rather than by a fixed
/// month.
fn period(months: i64) -> String {
    let current =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    let (year, month) = current
        .split_once('-')
        .expect("the current billing period is spelled YYYY-MM");
    let total = year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month")
        - 1
        + months;
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// The same billing period with its month spelled without a leading zero — the other
/// spelling the modelled `BillingPeriod` pattern allows. A month from October on has
/// only the one spelling, so for those this is the value it was given.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// The billing period as a report REPORTS it in a breakdown attribute: the month by name
/// and the four-digit year, rather than the `YYYY-MM` form a request names a period with.
///
/// How that value is rendered is pinned against the member's own documented value, over
/// every month of the year, in `tests/all/billing_group_cost_report_period_attribute.rs`,
/// which owns that rule; here it is only how a row of this file's tables names the period
/// it is for.
fn reported_period(billing_period: &str) -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::reported_billing_period(
        billing_period,
    )
}

/// RULE: the pro forma cost of a billing group is the SIGNED sum of the charges of the
/// custom line items in effect for it, the actual cost is the metered usage this emulator
/// records (none), and the margin and its percentage follow from the two.
#[tokio::test]
async fn the_proforma_cost_is_the_signed_sum_of_the_custom_line_items_in_effect() {
    let em = Emulator::new();

    // Each row is its OWN billing group, so the groups differ along the charge kinds they
    // hold while the request stays the same. (case, group ARN, expected pro forma cost)
    let mut cases: Vec<(&str, String, f64)> = Vec::new();

    cases.push(("a group with no custom line item", em.billing_group("empty").await, 0.0));

    let one_fee = em.billing_group("onefee").await;
    em.custom_line_item(
        &one_fee,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    cases.push(("one flat fee", one_fee, 100.0));

    let two_fees = em.billing_group("twofees").await;
    em.custom_line_item(
        &two_fees,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    em.custom_line_item(
        &two_fees,
        "feehalf",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 0.5 } } }),
    )
    .await;
    cases.push(("two flat fees, one of them fractional", two_fees, 100.5));

    let only_credit = em.billing_group("onlycredit").await;
    em.custom_line_item(
        &only_credit,
        "credit25",
        json!({ "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 25.0 } } }),
    )
    .await;
    cases.push(("one flat credit, which comes OFF the bill", only_credit, -25.0));

    let fee_and_credit = em.billing_group("feeandcredit").await;
    em.custom_line_item(
        &fee_and_credit,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    em.custom_line_item(
        &fee_and_credit,
        "credit25",
        json!({ "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 25.0 } } }),
    )
    .await;
    cases.push(("a flat fee and a flat credit", fee_and_credit, 75.0));

    let zero_valued = em.billing_group("zerovalued").await;
    em.custom_line_item(
        &zero_valued,
        "fee0",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 0.0 } } }),
    )
    .await;
    cases.push(("a fee worth the modelled minimum charge value", zero_valued, 0.0));

    // A percentage item is "multiplied against the combined value of its associated
    // resources": 10% of the 200 the flat item charges, beside that 200 itself.
    let percentage = em.billing_group("percentage").await;
    let percentage_base = em
        .custom_line_item(
            &percentage,
            "fee200",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 200.0 } } }),
        )
        .await;
    em.custom_line_item(
        &percentage,
        "tenpercent",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 10.0, "AssociatedValues": [percentage_base] },
            }
        }),
    )
    .await;
    cases.push(("a percentage fee associated at create", percentage, 220.0));

    // The same arithmetic with the association made by a LATER batch call, and the
    // percentage item a CREDIT: half of the 10 charged, taken off the bill.
    let percentage_credit = em.billing_group("percentagecredit").await;
    let credit_base = em
        .custom_line_item(
            &percentage_credit,
            "fee10",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } } }),
        )
        .await;
    let half_credit = em
        .custom_line_item(
            &percentage_credit,
            "halfcredit",
            json!({
                "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 50.0 } }
            }),
        )
        .await;
    em.associate(&half_credit, &[credit_base]).await;
    cases.push((
        "a percentage credit associated by a later batch call",
        percentage_credit,
        5.0,
    ));

    // A percentage item may feed another one: 100 flat, 10% of it, then 50% of that.
    let chained = em.billing_group("chained").await;
    let chain_base = em
        .custom_line_item(
            &chained,
            "fee100",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
        )
        .await;
    let chain_middle = em
        .custom_line_item(
            &chained,
            "tenpercent",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": { "PercentageValue": 10.0, "AssociatedValues": [chain_base] },
                }
            }),
        )
        .await;
    em.custom_line_item(
        &chained,
        "halfofthat",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 50.0, "AssociatedValues": [chain_middle] },
            }
        }),
    )
    .await;
    cases.push(("a percentage of a percentage", chained, 115.0));

    // A percentage of the BILLING GROUP is a percentage of its metered usage, of which
    // this emulator records none — so it adds nothing beside the flat item.
    let group_percentage = em.billing_group("grouppercentage").await;
    em.custom_line_item(
        &group_percentage,
        "fee40",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 40.0 } } }),
    )
    .await;
    em.custom_line_item(
        &group_percentage,
        "groupshare",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": {
                    "PercentageValue": 10.0,
                    "AssociatedValues": [group_percentage.clone()],
                },
            }
        }),
    )
    .await;
    cases.push((
        "a percentage of the billing group's own metered usage",
        group_percentage,
        40.0,
    ));

    let unassociated = em.billing_group("unassociated").await;    em.custom_line_item(
        &unassociated,
        "quarter",
        json!({
            "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 25.0 } }
        }),
    )
    .await;
    cases.push(("a percentage with nothing associated to it", unassociated, 0.0));

    // Two percentage items associated to the SAME flat item: each takes its own share of
    // the one value, which is counted once beside them.
    let shared = em.billing_group("shared").await;
    let shared_base = em
        .custom_line_item(
            &shared,
            "fee100",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
        )
        .await;
    em.custom_line_item(
        &shared,
        "tenth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": {
                    "PercentageValue": 10.0,
                    "AssociatedValues": [shared_base.clone()],
                },
            }
        }),
    )
    .await;
    em.custom_line_item(
        &shared,
        "fifth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 20.0, "AssociatedValues": [shared_base] },
            }
        }),
    )
    .await;
    cases.push((
        "two percentage items sharing one associated item",
        shared,
        130.0,
    ));

    // Nothing stops two percentage items from being associated to each other. Neither has
    // a settled value to lend the other, so the pair is worth nothing — and the report
    // still answers rather than walking the loop.
    let cyclic = em.billing_group("cyclic").await;
    let first_half = em
        .custom_line_item(
            &cyclic,
            "firsthalf",
            json!({
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 50.0 } }
            }),
        )
        .await;
    let second_half = em
        .custom_line_item(
            &cyclic,
            "secondhalf",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": {
                        "PercentageValue": 50.0,
                        "AssociatedValues": [first_half.clone()],
                    },
                }
            }),
        )
        .await;
    em.associate(&first_half, &[second_half]).await;
    cases.push((
        "two percentage items associated to each other",
        cyclic,
        0.0,
    ));

    // An item whose own billing period range ended before the period the report covers is
    // in effect in none of them, so none of its charge is reported.
    let expired = em.billing_group("expired").await;
    em.custom_line_item(
        &expired,
        "fee500",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 500.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(-1),
                "ExclusiveEndBillingPeriod": period(0),
            },
        }),
    )
    .await;
    cases.push(("an item in effect only in a past billing period", expired, 0.0));

    for (case, group_arn, proforma) in cases {
        let rows = em.rows(case, json!({ "Arn": group_arn.clone() })).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: a report with no GroupBy is one row for the group, got {rows:?}"
        );
        assert_summary(&rows[0], &group_arn, proforma, case);
        assert_eq!(
            rows[0].get("Attributes"),
            None,
            "case {case:?}: no breakdown was asked for, so no Attributes are due; got {:?}",
            rows[0]
        );
    }
}

/// RULE: a report grouped by billing period reports one row per period of the requested
/// range — the periods the half-open range enumerates — each carrying that period's own
/// charges and the period as its attribute, reported by month name and year.
#[tokio::test]
async fn a_report_grouped_by_billing_period_reports_each_period_of_the_range() {
    let em = Emulator::new();
    let group = em.billing_group("periods").await;

    // Three items differing in where their own half-open range falls: every period of
    // the report, one period only, and from a period on with no end at all. An item is
    // in effect no earlier than the period it was created in, so the one that spans the
    // whole report names the first of them as its inclusive start.
    em.custom_line_item(
        &group,
        "fromprevious",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "previousonly",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(-1),
                "ExclusiveEndBillingPeriod": period(0),
            },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "fromcurrent",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 5.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
        }),
    )
    .await;

    // The range is spelled with a single-digit month, which the `BillingPeriod` pattern
    // admits; the reported attribute names the same month either way.
    let rows = em
        .rows(
            "three periods grouped by billing period",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": one_digit_month(&period(-1)),
                    "ExclusiveEndBillingPeriod": period(2),
                },
                "GroupBy": ["BILLING_PERIOD"],
            }),
        )
        .await;

    // (the period the row is for, its pro forma cost)
    let expected = [
        (period(-1), 11.0),
        (period(0), 6.0),
        (period(1), 6.0),
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "case three periods grouped by billing period: expected one row per period, got {rows:?}"
    );
    for (row, (period, proforma)) in rows.iter().zip(expected) {
        let case = format!("the {period} row");
        assert_eq!(
            attributes(row),
            vec![("BILLING_PERIOD".to_string(), reported_period(&period))],
            "case {case:?}: expected the period as the row's one attribute, got {row}"
        );
        assert_summary(row, &group, proforma, &case);
    }

    // The same range with NO breakdown is the one row that aggregates those periods.
    let whole = em
        .rows(
            "the same range with no breakdown",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(2),
                },
            }),
        )
        .await;
    assert_eq!(
        whole.len(),
        1,
        "case the same range with no breakdown: expected one row, got {whole:?}"
    );
    assert_summary(&whole[0], &group, 23.0, "the same range with no breakdown");
}

/// RULE: a report grouped by product name reports one row per product the group's charges
/// are presented under, and naming several attributes breaks the summary down by all of
/// them, in the order the request listed them.
#[tokio::test]
async fn a_report_grouped_by_product_name_reports_each_presented_product() {
    let em = Emulator::new();
    let group = em.billing_group("products").await;

    // Items differing in the product they present under: two under one service, one under
    // another, one under none at all, and one of them bounded to the second period. Each
    // item that is meant to be in effect over the whole report names the first period of
    // it as its inclusive start, since an item is in effect no earlier than the period it
    // was created in.
    em.custom_line_item(
        &group,
        "storage",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "PresentationDetails": { "Service": "S3" },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "storageextra",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 2.0 } },
            "PresentationDetails": { "Service": "S3" },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "computecredit",
        json!({
            "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 3.0 } },
            "PresentationDetails": { "Service": "EC2" },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "unpresented",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 7.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
    )
    .await;

    let two_months = json!({
        "InclusiveStartBillingPeriod": period(-1),
        "ExclusiveEndBillingPeriod": period(1),
    });

    // By product over both periods: each product's charges summed across the range.
    for (case, group_by) in [
        ("grouped by product name", json!(["PRODUCT_NAME"])),
        (
            // Naming one attribute twice breaks the summary down no further.
            "grouped by product name twice",
            json!(["PRODUCT_NAME", "PRODUCT_NAME"]),
        ),
    ] {
        let rows = em
            .rows(
                case,
                json!({
                    "Arn": group.clone(),
                    "BillingPeriodRange": two_months.clone(),
                    "GroupBy": group_by,
                }),
            )
            .await;
        // Ordered by product name: this service's own, then EC2, then S3.
        let expected = [(OWN_PRODUCT_NAME, 14.0), ("EC2", -6.0), ("S3", 22.0)];
        assert_eq!(
            rows.len(),
            expected.len(),
            "case {case:?}: expected one row per presented product, got {rows:?}"
        );
        for (row, (product, proforma)) in rows.iter().zip(expected) {
            let row_case = format!("{case}: the {product} row");
            assert_eq!(
                attributes(row),
                vec![("PRODUCT_NAME".to_string(), product.to_string())],
                "case {row_case:?}: expected the product as the row's one attribute, got {row}"
            );
            assert_summary(row, &group, proforma, &row_case);
        }
    }

    // By product AND period: one row per pair, with the attributes in the order asked for.
    let case = "grouped by product name and billing period";
    let rows = em
        .rows(
            case,
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": two_months,
                "GroupBy": ["PRODUCT_NAME", "BILLING_PERIOD"],
            }),
        )
        .await;
    // (period, product, pro forma cost), in the order the rows run.
    let expected = [
        (period(-1), OWN_PRODUCT_NAME, 7.0),
        (period(-1), "EC2", -3.0),
        (period(-1), "S3", 10.0),
        (period(0), OWN_PRODUCT_NAME, 7.0),
        (period(0), "EC2", -3.0),
        (period(0), "S3", 12.0),
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "case {case:?}: expected one row per product and period, got {rows:?}"
    );
    for (row, (period, product, proforma)) in rows.iter().zip(expected) {
        let row_case = format!("{case}: the {product} row of {period}");
        assert_eq!(
            attributes(row),
            vec![
                ("PRODUCT_NAME".to_string(), product.to_string()),
                ("BILLING_PERIOD".to_string(), reported_period(&period)),
            ],
            "case {row_case:?}: expected both attributes in the requested order, got {row}"
        );
        assert_summary(row, &group, proforma, &row_case);
    }
}

/// RULE: a request outside the rules the model and the documentation state is REFUSED —
/// a range that covers no period or more than the published maximum, and a billing group
/// that does not exist — while every request inside them is served, in either form the
/// `BillingGroupArn` pattern admits and at either end of the modelled page size.
#[tokio::test]
async fn a_report_outside_the_stated_rules_is_refused() {
    let em = Emulator::new();
    let group = em.billing_group("refusals").await;
    em.custom_line_item(
        &group,
        "fee8",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 8.0 } } }),
    )
    .await;

    // (case, the BillingPeriodRange member, the expected status)
    // The published maximum is 12 months ("You can specify up to 12 months",
    // `smithy_query(query="doc",shape="GetBillingGroupCostReportInput",member="BillingPeriodRange")`):
    // each bound of the range is tested, and one month past it. Every span is named
    // relative to the service's own current billing period, which is the period the group
    // under test was created in — a report is of a group that was billing in the periods it
    // covers (`tests/all/billing_group_cost_report_period_scope.rs` owns that rule), so a
    // row that is to be SERVED names a range covering that period.
    let ranges: Vec<(&str, Value, u16)> = vec![
        (
            "a one-month range",
            json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            }),
            200,
        ),
        (
            "a range of the published maximum twelve months",
            json!({
                "InclusiveStartBillingPeriod": period(-11),
                "ExclusiveEndBillingPeriod": period(1),
            }),
            200,
        ),
        (
            "a range one month past the published maximum",
            json!({
                "InclusiveStartBillingPeriod": period(-11),
                "ExclusiveEndBillingPeriod": period(2),
            }),
            400,
        ),
        (
            "a range whose exclusive end is its inclusive start",
            json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(0),
            }),
            400,
        ),
        (
            "a range whose end precedes its start",
            json!({
                "InclusiveStartBillingPeriod": period(1),
                "ExclusiveEndBillingPeriod": period(0),
            }),
            400,
        ),
        (
            "a reversed range spelled with single-digit months",
            json!({
                "InclusiveStartBillingPeriod": one_digit_month(&period(1)),
                "ExclusiveEndBillingPeriod": one_digit_month(&period(0)),
            }),
            400,
        ),
    ];

    for (case, range, expected) in ranges {
        let (status, body) = em
            .report(json!({ "Arn": group.clone(), "BillingPeriodRange": range }))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {body}"
            );
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("ILLEGAL_BILLING_PERIOD_RANGE"),
                "case {case:?}: expected the modelled range reason, got {body}"
            );
        }
    }

    // A billing group nothing created is a reference to a resource that doesn't exist.
    let (status, body) = em.report(json!({ "Arn": "NOSUCHGROUP0" })).await;
    assert_eq!(
        status, 404,
        "case an unknown billing group: expected HTTP 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case an unknown billing group: expected the modelled not-found, got {body}"
    );
    assert_eq!(
        body.get("ResourceType").and_then(Value::as_str),
        Some("billinggroup"),
        "case an unknown billing group: expected the resource type, got {body}"
    );

    // The bare resource id names the same group the full ARN does, so it reports the same
    // summary; and both ends of the modelled MaxResults range serve the whole page.
    for (case, request) in [
        ("the group by bare resource id", json!({ "Arn": bare_id(&group) })),
        (
            "the minimum modelled page size",
            json!({ "Arn": group.clone(), "MaxResults": 200 }),
        ),
        (
            "the maximum modelled page size",
            json!({ "Arn": group.clone(), "MaxResults": 300 }),
        ),
    ] {
        let rows = em.rows(case, request).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected the group's one row, got {rows:?}"
        );
        assert_summary(&rows[0], &group, 8.0, case);
    }

    // A page size outside the modelled range, and an attribute outside the enumerated
    // breakdown, are refused at the boundary.
    for (case, request) in [
        (
            "a page size below the modelled minimum",
            json!({ "Arn": group.clone(), "MaxResults": 100 }),
        ),
        (
            "a page size above the modelled maximum",
            json!({ "Arn": group.clone(), "MaxResults": 301 }),
        ),
        (
            "an attribute outside the enumerated breakdown",
            json!({ "Arn": group.clone(), "GroupBy": ["NOT_AN_ATTRIBUTE"] }),
        ),
    ] {
        let (status, body) = em.report(request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {body}"
        );
    }
}

/// RULE: the report is a READ — it writes nothing, so reading it again, and reading it
/// after another read, reports exactly the same summary.
#[tokio::test]
async fn a_report_writes_nothing_so_a_later_read_sees_the_same_summary() {
    let em = Emulator::new();
    let group = em.billing_group("stable").await;
    let base = em
        .custom_line_item(
            &group,
            "fee60",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 60.0 } } }),
        )
        .await;
    em.custom_line_item(
        &group,
        "fifth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 20.0, "AssociatedValues": [base] },
            }
        }),
    )
    .await;

    // Rows differ along the request that is repeated: no breakdown, each attribute, and
    // both of them over a multi-period range.
    let requests: Vec<(&str, Value)> = vec![
        ("no breakdown", json!({ "Arn": group.clone() })),
        (
            "grouped by billing period",
            json!({ "Arn": group.clone(), "GroupBy": ["BILLING_PERIOD"] }),
        ),
        (
            "grouped by product name",
            json!({ "Arn": group.clone(), "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "grouped by both over three periods",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(2),
                },
                "GroupBy": ["BILLING_PERIOD", "PRODUCT_NAME"],
            }),
        ),
    ];

    for (case, request) in requests {
        let (first_status, first) = em.report(request.clone()).await;
        let (second_status, second) = em.report(request).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case:?}: a read changes nothing, so the second read must answer the same; \
             got {first_status} with {first} then {second_status} with {second}"
        );
    }

    // And the pro forma cost is still the one the stored items describe: 60 plus a fifth
    // of it.
    let rows = em
        .rows("after four reads", json!({ "Arn": group.clone() }))
        .await;
    assert_eq!(
        rows.len(),
        1,
        "case after four reads: expected the group's one row, got {rows:?}"
    );
    assert_summary(&rows[0], &group, 72.0, "after four reads");
}

/// One row of the table of [`a_billing_period_breakdown_reports_every_period_of_the_range`]:
/// what the billing group holds, the range the report is asked over, and the pro forma cost
/// each period of that range carries.
struct PeriodRowsCase {
    case: &'static str,
    /// The group's name, in the characters the modelled `BillingGroupName` pattern
    /// `^[a-zA-Z0-9_\+=\.\-@]+$` allows
    /// (`smithy_query(query="shape",shape="BillingGroupName")`).
    name: &'static str,
    /// The create of each custom line item the group holds, beside the members every
    /// create needs.
    items: Vec<Value>,
    /// The range the request names, or `None` for a request that names none and so asks
    /// about the billing period the call is made in.
    range: Option<(String, String)>,
    /// `(the billing period the row is for, its pro forma cost)` — one entry per period
    /// the requested range covers, in the order the rows are due.
    expected: Vec<(String, f64)>,
}

/// A report request naming `group`, the range `range` (no range at all when it is `None`)
/// and the breakdown `group_by` (no breakdown at all when it is empty).
fn period_report(group: &str, range: Option<&(String, String)>, group_by: &[&str]) -> Value {
    let mut request = json!({ "Arn": group });
    if let Some((start, end)) = range {
        request["BillingPeriodRange"] = json!({
            "InclusiveStartBillingPeriod": start,
            "ExclusiveEndBillingPeriod": end,
        });
    }
    if !group_by.is_empty() {
        request["GroupBy"] = json!(group_by);
    }
    request
}

/// The create of a flat custom line item worth `value` of type `charge_type`, in effect over
/// the half-open billing period range `[start, end)` — from `start` on when `end` is `None`,
/// and over every period from the create on when `start` is `None` too.
fn flat_item(charge_type: &str, value: f64, start: Option<&str>, end: Option<&str>) -> Value {
    let mut create = json!({
        "ChargeDetails": { "Type": charge_type, "Flat": { "ChargeValue": value } },
    });
    if let Some(start) = start {
        let mut range = json!({ "InclusiveStartBillingPeriod": start });
        if let Some(end) = end {
            range["ExclusiveEndBillingPeriod"] = json!(end);
        }
        create["BillingPeriodRange"] = range;
    }
    create
}

/// RULE: every group of the requested breakdown whose members the REQUEST itself enumerates
/// is a row of the margin summary, carrying the summary it has whether or not the billing
/// group holds a charge there — so a breakdown by billing period reports one row per period
/// of the range, a period with no custom line item in effect being the row whose amounts are
/// all zero, and those rows add up to the one row the same range answers with no breakdown.
///
/// The rows span what the rule generalises over: how many periods the range covers (one,
/// two, three, the published maximum of twelve), where the periods carrying a charge fall in
/// it (none of them, the first only, the last only, every one, one whose fee and credit
/// cancel), whether the group holds any custom line item at all, whether the request names a
/// range or leaves it implicit, how a month is spelled and whether the attribute is named
/// once or twice.
#[tokio::test]
async fn a_billing_period_breakdown_reports_every_period_of_the_range() {
    let em = Emulator::new();

    // Twelve months is the longest range the model publishes ("You can specify up to 12
    // months",
    // `smithy_query(query="doc",shape="GetBillingGroupCostReportInput",member="BillingPeriodRange")`),
    // and a custom line item may be created no earlier than the previous billing period, so
    // the longest range here runs from that period on.
    let twelve_months = Some((period(-1), period(11)));

    let cases: Vec<PeriodRowsCase> = vec![
        PeriodRowsCase {
            case: "a group with no custom line item at all, over one month",
            name: "Bare_One",
            items: Vec::new(),
            range: Some((period(0), period(1))),
            expected: vec![(period(0), 0.0)],
        },
        PeriodRowsCase {
            case: "a group with no custom line item at all, over no range at all",
            name: "Bare_Implicit",
            items: Vec::new(),
            range: None,
            expected: vec![(period(0), 0.0)],
        },
        PeriodRowsCase {
            case: "a group with no custom line item at all, over the maximum twelve months",
            name: "Bare_Twelve",
            items: Vec::new(),
            range: twelve_months.clone(),
            expected: (-1..11).map(|month| (period(month), 0.0)).collect(),
        },
        PeriodRowsCase {
            case: "a charge in the first of twelve periods, so eleven carry none",
            name: "First_Only",
            items: vec![flat_item("FEE", 10.0, Some(&period(-1)), Some(&period(0)))],
            range: twelve_months.clone(),
            expected: std::iter::once((period(-1), 10.0))
                .chain((0..11).map(|month| (period(month), 0.0)))
                .collect(),
        },
        PeriodRowsCase {
            case: "a charge in the last of two periods, so the first carries none",
            name: "Last_Only",
            items: vec![flat_item("FEE", 7.5, None, None)],
            range: Some((period(-1), period(1))),
            expected: vec![(period(-1), 0.0), (period(0), 7.5)],
        },
        PeriodRowsCase {
            case: "a charge in every one of three periods",
            name: "Every_Period",
            items: vec![flat_item("FEE", 3.0, Some(&period(-1)), None)],
            range: Some((period(-1), period(2))),
            expected: vec![(period(-1), 3.0), (period(0), 3.0), (period(1), 3.0)],
        },
        PeriodRowsCase {
            case: "a period whose fee and credit cancel, beside one with no charge at all",
            name: "Cancelled.Charge",
            items: vec![
                flat_item("FEE", 5.0, Some(&period(-1)), Some(&period(0))),
                flat_item("CREDIT", 5.0, Some(&period(-1)), Some(&period(0))),
            ],
            range: Some((period(-1), period(1))),
            expected: vec![(period(-1), 0.0), (period(0), 0.0)],
        },
        PeriodRowsCase {
            case: "a range whose months are spelled without their leading zero",
            name: "One@Digit",
            items: vec![flat_item("FEE", 4.0, Some(&period(-1)), Some(&period(0)))],
            range: Some((one_digit_month(&period(-1)), one_digit_month(&period(2)))),
            expected: vec![(period(-1), 4.0), (period(0), 0.0), (period(1), 0.0)],
        },
    ];

    for case in cases {
        let group = em.billing_group(case.name).await;
        for (index, create) in case.items.iter().enumerate() {
            em.custom_line_item(
                &group,
                &format!("{}item{index}", case.name),
                create.clone(),
            )
            .await;
        }

        // Naming the attribute twice breaks the summary down no further, so both spellings
        // of this breakdown are due the same rows.
        for group_by in [
            vec!["BILLING_PERIOD"],
            vec!["BILLING_PERIOD", "BILLING_PERIOD"],
        ] {
            let label = format!("{} grouped by {} attribute(s)", case.case, group_by.len());
            let request = period_report(&group, case.range.as_ref(), &group_by);
            let rows = em.rows(&label, request.clone()).await;
            assert_eq!(
                rows.len(),
                case.expected.len(),
                "case {label:?}: expected one row per period of the requested range, got \
                 {rows:?}"
            );
            for (row, (billing_period, proforma)) in rows.iter().zip(&case.expected) {
                let row_case = format!("{label}: the {billing_period} row");
                assert_eq!(
                    attributes(row),
                    vec![(
                        "BILLING_PERIOD".to_string(),
                        reported_period(billing_period)
                    )],
                    "case {row_case:?}: expected the reported period as the row's one \
                     attribute, got {row}"
                );
                assert_summary(row, &group, *proforma, &row_case);
            }

            // The rows a period with no charge is reported with are not state: the read
            // writes nothing, so the same request answers the same thing again.
            let (first_status, first) = em.report(request.clone()).await;
            let (second_status, second) = em.report(request).await;
            assert_eq!(
                (first_status, &first),
                (second_status, &second),
                "case {label:?}: the report writes nothing, so a second read must answer \
                 the same; got {first_status} with {first} then {second_status} with \
                 {second}"
            );
        }

        // The same range with NO breakdown is the ONE row those period rows add up to.
        let label = format!("{} with no breakdown", case.case);
        let whole = em
            .rows(&label, period_report(&group, case.range.as_ref(), &[]))
            .await;
        assert_eq!(
            whole.len(),
            1,
            "case {label:?}: a report asking for no breakdown is one row, got {whole:?}"
        );
        let total: f64 = case.expected.iter().map(|(_, proforma)| proforma).sum();
        assert_summary(&whole[0], &group, total, &label);
    }
}

/// One expected row of a breakdown: its `Attributes` as `(Key, Value)` pairs in response
/// order, and the pro forma cost it carries.
type ExpectedRow = (Vec<(String, String)>, f64);

/// One row of the table of
/// [`a_product_breakdown_reports_only_the_products_the_charges_name`]: the billing group
/// read, the breakdown it is read under and the rows that breakdown is due.
struct ProductRowsCase {
    case: &'static str,
    group: String,
    group_by: Vec<&'static str>,
    expected: Vec<ExpectedRow>,
}

/// RULE's boundary: the products charges are presented under are the one breakdown attribute
/// a request does NOT enumerate, so a breakdown naming `PRODUCT_NAME` reports exactly the
/// products the stored charges name — no row for a period that holds no charge, and no row
/// at all for a group that holds none — while the same range under `BILLING_PERIOD` alone
/// still reports every period of it.
#[tokio::test]
async fn a_product_breakdown_reports_only_the_products_the_charges_name() {
    let em = Emulator::new();

    // Two groups over the same two-month range: one holding nothing at all, one charged in
    // the FIRST of the two periods under the product its presentation details name.
    let bare = em.billing_group("productsbare").await;
    let charged = em.billing_group("productscharged").await;
    em.custom_line_item(
        &charged,
        "storageonce",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 9.0 } },
            "PresentationDetails": { "Service": "S3" },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(-1),
                "ExclusiveEndBillingPeriod": period(0),
            },
        }),
    )
    .await;
    let range = Some((period(-1), period(1)));

    let cases: Vec<ProductRowsCase> = vec![
        ProductRowsCase {
            case: "a chargeless group broken down by product name",
            group: bare.clone(),
            group_by: vec!["PRODUCT_NAME"],
            expected: Vec::new(),
        },
        ProductRowsCase {
            case: "a chargeless group broken down by billing period and product name",
            group: bare.clone(),
            group_by: vec!["BILLING_PERIOD", "PRODUCT_NAME"],
            expected: Vec::new(),
        },
        ProductRowsCase {
            case: "a chargeless group broken down by billing period",
            group: bare.clone(),
            group_by: vec!["BILLING_PERIOD"],
            expected: vec![
                (
                    vec![("BILLING_PERIOD".to_string(), reported_period(&period(-1)))],
                    0.0,
                ),
                (
                    vec![("BILLING_PERIOD".to_string(), reported_period(&period(0)))],
                    0.0,
                ),
            ],
        },
        ProductRowsCase {
            case: "a group charged in one period, broken down by product name",
            group: charged.clone(),
            group_by: vec!["PRODUCT_NAME"],
            expected: vec![(vec![("PRODUCT_NAME".to_string(), "S3".to_string())], 9.0)],
        },
        ProductRowsCase {
            case: "a group charged in one period, broken down by period and product name",
            group: charged.clone(),
            group_by: vec!["BILLING_PERIOD", "PRODUCT_NAME"],
            expected: vec![(
                vec![
                    ("BILLING_PERIOD".to_string(), reported_period(&period(-1))),
                    ("PRODUCT_NAME".to_string(), "S3".to_string()),
                ],
                9.0,
            )],
        },
        ProductRowsCase {
            case: "a group charged in one period, broken down by billing period",
            group: charged.clone(),
            group_by: vec!["BILLING_PERIOD"],
            expected: vec![
                (
                    vec![("BILLING_PERIOD".to_string(), reported_period(&period(-1)))],
                    9.0,
                ),
                (
                    vec![("BILLING_PERIOD".to_string(), reported_period(&period(0)))],
                    0.0,
                ),
            ],
        },
    ];

    for case in cases {
        let rows = em
            .rows(
                case.case,
                period_report(&case.group, range.as_ref(), &case.group_by),
            )
            .await;
        assert_eq!(
            rows.len(),
            case.expected.len(),
            "case {:?}: expected {} row(s), got {rows:?}",
            case.case,
            case.expected.len()
        );
        for (row, (row_attributes, proforma)) in rows.iter().zip(&case.expected) {
            let row_case = format!("{}: the {row_attributes:?} row", case.case);
            assert_eq!(
                &attributes(row),
                row_attributes,
                "case {row_case:?}: expected those attributes in the requested order, got \
                 {row}"
            );
            assert_summary(row, &case.group, *proforma, &row_case);
        }
    }

    // The product an item with no presentation details of its own presents its charges
    // under is this service's own; the chargeless group has no item naming it, and the
    // universe of products is not enumerated, so no row is invented for it.
    let bare_products = em
        .rows(
            "a chargeless group's products",
            period_report(&bare, range.as_ref(), &["PRODUCT_NAME"]),
        )
        .await;
    assert!(
        bare_products.is_empty(),
        "case a chargeless group's products: no charge names {OWN_PRODUCT_NAME} or any \
         other product, so no product row is due; got {bare_products:?}"
    );
}
