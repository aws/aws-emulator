// SPDX-License-Identifier: CC0-1.0

//! The earliest billing period a custom line item exists in, end to end over the wire:
//! `CreateBillingGroup` -> `CreateCustomLineItem` -> `ListCustomLineItems` /
//! `ListCustomLineItemVersions` / `GetBillingGroupCostReport`.
//!
//! RULE: a custom line item is in effect no earlier than the billing period the create
//! that minted it was made in — a create that names no `BillingPeriodRange` applies the
//! item from that period on, and the one that names a range applies it from the range's
//! `InclusiveStartBillingPeriod` — so a read scoped to an earlier, already closed billing
//! period reports no version of the item at all and no bill of such a period carries its
//! charge.
//!
//! The rows differ along what the rule generalises over rather than one recorded call:
//! how far the period a read asks for is from the item's first one (the epoch month, a
//! decade back, a year back, two months back, the period before, the item's own period,
//! the period after, a year on, the last month the modelled `BillingPeriod` pattern
//! admits), the form of the create's range (none at all, open-ended, bounded, a
//! one-digit month), the charge kind of the item (a flat `FEE`, a percentage `CREDIT`),
//! how a billing period is spelled (`2026-7` / `2026-07`) and which operation does the
//! reading (the item listing, the version history, the cost report).
//!
//! Every billing period here is named as an offset from the CURRENT one ([`period`]),
//! because the clock decides which period a create lands in.

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

/// How far two reported amounts may differ and still be the same amount.
const TOLERANCE: f64 = 1e-6;

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for earlier),
/// in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
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

/// The billing period as a cost report REPORTS it in a breakdown attribute: the month by
/// name and the four-digit year, rather than the `YYYY-MM` form a request names a period
/// with. `tests/all/billing_group_cost_report_period_attribute.rs` owns that rule; here it
/// is only how the row a cost is read off names the period it is for.
fn period_as_reported(billing_period: &str) -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::reported_billing_period(
        billing_period,
    )
}

/// The same billing period with its month spelled without a leading zero — the other
/// spelling the modelled `BillingPeriod` pattern allows. A month from October on has only
/// the one spelling, so for those this is the value it was given.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — Billing Conductor's control plane is the payer
    /// account's alone, so that is the world in which these calls are reachable at all.
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

    /// Send a JSON body to one of this service's routes and return the status and the
    /// parsed body.
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

    /// The item `arn` as `ListCustomLineItems` reports it for `billing_period` (the
    /// current one when `None`), or `None` when the read reports no such item.
    async fn listed(&self, case: &str, arn: &str, billing_period: Option<&str>) -> Option<Value> {
        let mut request = json!({ "Filters": { "Arns": [arn] } });
        if let Some(billing_period) = billing_period {
            request["BillingPeriod"] = json!(billing_period);
        }
        let (status, body) = self.send("POST", "/list-custom-line-items", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer HTTP 200, got {status} with {body}"
        );
        let elements = body
            .get("CustomLineItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            elements.len() <= 1,
            "case {case:?}: one ARN names at most one item, got {elements:?}"
        );
        elements.into_iter().next()
    }

    /// The versions `ListCustomLineItemVersions` reports for `arn`, oldest first.
    async fn versions(&self, case: &str, arn: &str) -> Vec<Value> {
        let (status, body) = self
            .send(
                "POST",
                "/list-custom-line-item-versions",
                &json!({ "Arn": arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version read to answer HTTP 200, got {status} with \
             {body}"
        );
        body.get("CustomLineItemVersions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The pro forma cost `GetBillingGroupCostReport` attributes to each billing period
    /// of the half-open range `[start, end)`, as pairs in the order it reports them, each
    /// period named as the report NAMES it ([`period_as_reported`]).
    async fn cost_by_period(
        &self,
        case: &str,
        group_arn: &str,
        start: &str,
        end: &str,
    ) -> Vec<(String, f64)> {
        let (status, body) = self
            .send(
                "POST",
                "/get-billing-group-cost-report",
                &json!({
                    "Arn": group_arn,
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": start,
                        "ExclusiveEndBillingPeriod": end,
                    },
                    "GroupBy": ["BILLING_PERIOD"],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the cost report to answer HTTP 200, got {status} with \
             {body}"
        );
        body.get("BillingGroupCostReportResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| {
                let billing_period = row
                    .get("Attributes")
                    .and_then(Value::as_array)
                    .and_then(|attributes| attributes.first())
                    .and_then(|attribute| attribute.get("Value"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let proforma = row
                    .get("ProformaCost")
                    .and_then(Value::as_str)
                    .and_then(|cost| cost.parse::<f64>().ok())
                    .unwrap_or_default();
                (billing_period, proforma)
            })
            .collect()
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

/// One item of the table: the shape of its create and the billing period it is in effect
/// from.
struct ItemCase {
    case: &'static str,
    name: &'static str,
    /// The members the create carries beside the ones every create needs.
    create: Value,
    /// The first billing period the item is in effect in.
    first_period: String,
    /// The first billing period it is no longer in effect in, if its create bounded it.
    after_last_period: Option<String>,
}

/// THE RULE over the whole class: for every shape of create, a read scoped to a billing
/// period reports the item exactly in the periods from the first one it is in effect in
/// up to (but not including) the one its range ends at — so every earlier period, however
/// far back, reports nothing.
#[tokio::test]
async fn an_item_is_reported_only_from_the_billing_period_it_became_effective_in() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("home").await;

    let items: Vec<ItemCase> = vec![
        ItemCase {
            case: "a flat item created with no range at all",
            name: "norange",
            create: json!({}),
            first_period: period(0),
            after_last_period: None,
        },
        ItemCase {
            case: "a percentage item created with no range at all",
            name: "norangepercentage",
            create: json!({
                "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 25.0 } },
            }),
            first_period: period(0),
            after_last_period: None,
        },
        ItemCase {
            case: "an item created from the previous billing period on",
            name: "fromprevious",
            create: json!({
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
            }),
            first_period: period(-1),
            after_last_period: None,
        },
        ItemCase {
            case: "an item bounded to the previous billing period alone",
            name: "previousonly",
            create: json!({
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(0),
                },
            }),
            first_period: period(-1),
            after_last_period: Some(period(0)),
        },
        ItemCase {
            case: "an item created from the current period on, one-digit month",
            name: "onedigitmonth",
            create: json!({
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": one_digit_month(&period(0)),
                },
            }),
            first_period: period(0),
            after_last_period: None,
        },
    ];

    // (case, the billing period a read asks about) — how far the period is from the one
    // the item became effective in, in both spellings of a month, out to both ends of
    // what the modelled `BillingPeriod` pattern admits.
    let probes: Vec<(&str, String)> = vec![
        ("the epoch month", "1970-01".to_string()),
        ("the first month the pattern admits", "0000-01".to_string()),
        ("a decade back", period(-120)),
        ("a year back", period(-12)),
        ("two months back", period(-2)),
        ("one month back", period(-1)),
        ("one month back, one-digit month", one_digit_month(&period(-1))),
        ("the current billing period", period(0)),
        ("the current billing period, one-digit month", one_digit_month(&period(0))),
        ("one month on", period(1)),
        ("a year on", period(12)),
        ("the last month the pattern admits", "9999-12".to_string()),
    ];

    for item in &items {
        let arn = emulator
            .custom_line_item(&group, item.name, item.create.clone())
            .await;
        for (probe_case, probe) in &probes {
            let case = format!("{} read for {probe_case}", item.case);
            let in_effect = key(probe) >= key(&item.first_period)
                && item
                    .after_last_period
                    .as_deref()
                    .is_none_or(|after| key(probe) < key(after));
            let read = emulator.listed(&case, &arn, Some(probe)).await;
            assert_eq!(
                read.is_some(),
                in_effect,
                "case {case:?}: expected the item to be reported for {probe}: {in_effect}, got \
                 {read:?}"
            );
            if let Some(read) = read {
                assert_eq!(
                    read.get("Name").and_then(Value::as_str),
                    Some(item.name),
                    "case {case:?}: expected the item's own Name, got {read}"
                );
            }
        }
        // A read naming NO billing period asks about the current one, so it reports the
        // item exactly when the current period is one the item is in effect in.
        let case = format!("{} read for no billing period at all", item.case);
        let in_effect = key(&period(0)) >= key(&item.first_period)
            && item
                .after_last_period
                .as_deref()
                .is_none_or(|after| key(&period(0)) < key(after));
        let read = emulator.listed(&case, &arn, None).await;
        assert_eq!(
            read.is_some(),
            in_effect,
            "case {case:?}: expected the item to be reported for the current billing period: \
             {in_effect}, got {read:?}"
        );
    }
}

/// A billing period as a comparable month count, so `2026-7` and `2026-07` compare equal
/// and two periods order by time.
fn key(billing_period: &str) -> i64 {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month")
}

/// THE RULE as the VERSION HISTORY reports it: a version is in effect from a billing
/// period, so the first version of an item created with no range starts in the period the
/// create was made in — the same period the item listing first reports it for — and it has
/// no end.
#[tokio::test]
async fn the_first_version_starts_in_the_billing_period_the_create_was_made_in() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("versions").await;

    // (case, the create's members beside the required ones, the StartBillingPeriod and
    // EndBillingPeriod the one version must report)
    let rows: Vec<(&str, Value, String, Option<String>)> = vec![
        ("no range at all", json!({}), period(0), None),
        (
            "a range from the previous billing period on",
            json!({ "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) } }),
            period(-1),
            None,
        ),
        (
            "a range bounded to the previous billing period",
            json!({
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(0),
                },
            }),
            period(-1),
            Some(period(0)),
        ),
    ];

    for (index, (case, create, start, end)) in rows.into_iter().enumerate() {
        let arn = emulator
            .custom_line_item(&group, &format!("versioned{index}"), create)
            .await;
        let versions = emulator.versions(case, &arn).await;
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: a create leaves exactly one version, got {versions:?}"
        );
        assert_eq!(
            versions[0].get("StartBillingPeriod").and_then(Value::as_str),
            Some(start.as_str()),
            "case {case:?}: expected the version to start in {start}, got {}",
            versions[0]
        );
        assert_eq!(
            versions[0]
                .get("EndBillingPeriod")
                .and_then(Value::as_str)
                .map(str::to_string),
            end,
            "case {case:?}: expected the version to end at {end:?}, got {}",
            versions[0]
        );
    }
}

/// THE RULE as the BILL reports it: a billing period carries the charge of the items in
/// effect in it, so the bill of a period that closed before an item was created carries
/// none of that item's charge.
#[tokio::test]
async fn a_billing_period_before_an_item_was_created_carries_none_of_its_charge() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("bills").await;
    // One item with no range of its own (in effect from the current period on) and one
    // applying from the previous period on, so each period of the report distinguishes
    // them.
    emulator
        .custom_line_item(&group, "norange", json!({}))
        .await;
    emulator
        .custom_line_item(
            &group,
            "fromprevious",
            json!({
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 4.0 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
            }),
        )
        .await;

    let case = "a report over the previous, current and next billing periods";
    let reported = emulator
        .cost_by_period(case, &group, &period(-1), &period(2))
        .await;
    // (the billing period, the charge the items in effect in it add up to): the previous
    // period carries only the item that applies from it, and the later ones both items.
    let expected = [
        (period(-1), 4.0),
        (period(0), 5.0),
        (period(1), 5.0),
    ];
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected one row per billing period of the range, got {reported:?}"
    );
    for ((billing_period, proforma), (expected_period, expected_proforma)) in
        reported.iter().zip(expected)
    {
        let expected_reported = period_as_reported(&expected_period);
        assert_eq!(
            billing_period.as_str(),
            expected_reported.as_str(),
            "case {case:?}: expected the {expected_period} row reported as \
             {expected_reported:?}, got {billing_period}"
        );
        assert!(
            (proforma - expected_proforma).abs() < TOLERANCE,
            "case {case:?}: expected {expected_period} to bill {expected_proforma}, got \
             {proforma}"
        );
    }
}
