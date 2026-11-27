// SPDX-License-Identifier: CC0-1.0

//! A custom line item is VERSIONED by billing period, end to end over the wire:
//! `CreateBillingGroup` -> `CreateCustomLineItem` -> `UpdateCustomLineItem` ->
//! `ListCustomLineItems` / `ListCustomLineItemVersions` / `GetBillingGroupCostReport`.
//!
//! RULE: a write applied from a billing period on changes the item only for that period
//! and the later ones — it opens a new version there — while every earlier billing period
//! keeps the version that was in force in it, so a read scoped to a billing period
//! reports the version in force in THAT period, whichever operation does the reading. A
//! write that names NO billing period range is applied to exactly the periods the version
//! in force now holds, so it takes no period from a version an earlier write already
//! froze.
//!
//! The rows span what the rule generalises over rather than one recorded call: which
//! member the update changes (`Name`, `Description`, the flat charge value, the
//! percentage value, the `LineItemFilters`), the charge kind of the item (flat /
//! percentage), the form of the billing-period range the write names (open-ended,
//! bounded, one month, a month spelled without its leading zero), how many writes have
//! landed (one, three), how far the period read is from the write (the period before it,
//! several periods before it, the period itself, years after it) and which operation does
//! the read (the item listing, the version history, the cost report).

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

/// The billing period every item here is created to apply from, so each probe period is
/// either inside the item's life or knowingly before it: a create may put an item only
/// into the current or the PREVIOUS billing period, so this is the previous one and
/// every other period here is named as an offset from it.
fn created_from() -> String {
    period(-1)
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for
/// earlier), in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// Which period is the current one the wall clock decides, so no fixture here names a
/// fixed month.
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
/// is only how the row a bill is read off names the period it is for.
fn period_as_reported(billing_period: &str) -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::reported_billing_period(
        billing_period,
    )
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

/// How far two reported amounts may differ and still be the same amount.
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
    /// one — the world in which these calls are reachable at all. The denial every other
    /// caller gets is asserted in `tests/all/payer_account_gate.rs`.
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

    /// Create a custom line item applying from [`created_from`] on, overriding the
    /// default members with whatever `request` carries, and return its ARN.
    async fn custom_line_item(&self, group_arn: &str, name: &str, request: Value) -> String {
        let mut body = json!({
            "Name": name,
            "Description": "the original custom line item",
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": created_from() },
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

    /// `UpdateCustomLineItem` that is expected to succeed.
    async fn update_ok(&self, case: &str, arn: &str, request: Value) {
        let mut body = json!({ "Arn": arn });
        for (member, value) in members(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/update-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
    }

    /// The item as `ListCustomLineItems` reports it for `billing_period`, or `None` when
    /// that read reports no such item.
    async fn listed(&self, case: &str, arn: &str, billing_period: &str) -> Option<Value> {
        let (status, response) = self
            .send(
                "POST",
                "/list-custom-line-items",
                &json!({ "BillingPeriod": billing_period, "Filters": { "Arns": [arn] } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read for {billing_period} to answer HTTP 200, got \
             {status} with {response}"
        );
        let elements = response
            .get("CustomLineItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            elements.len() <= 1,
            "case {case:?}: one ARN names at most one item in {billing_period}, got {elements:?}"
        );
        elements.into_iter().next()
    }

    /// The item as the read of `billing_period` is required to report it.
    async fn listed_ok(&self, case: &str, arn: &str, billing_period: &str) -> Value {
        self.listed(case, arn, billing_period)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the item to be reported for {billing_period}")
            })
    }

    /// Every generation `ListCustomLineItemVersions` reports for the item, oldest first.
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
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("CustomLineItemVersions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The pro forma cost of each billing period a cost report over `[start, end)`
    /// covers, keyed by the period it is reported for, as the report NAMES that period
    /// ([`period_as_reported`]).
    async fn proforma_by_period(
        &self,
        case: &str,
        group: &str,
        start: &str,
        end: &str,
    ) -> Vec<(String, f64)> {
        let (status, body) = self
            .send(
                "POST",
                "/get-billing-group-cost-report",
                &json!({
                    "Arn": group,
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
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("BillingGroupCostReportResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| {
                let period = row
                    .get("Attributes")
                    .and_then(Value::as_array)
                    .and_then(|attributes| attributes.first().cloned())
                    .and_then(|attribute| {
                        attribute
                            .get("Value")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a BILLING_PERIOD attribute, got {row}")
                    });
                let proforma = row
                    .get("ProformaCost")
                    .and_then(Value::as_str)
                    .and_then(|amount| amount.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a decimal ProformaCost, got {row}")
                    });
                (period, proforma)
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

/// The value a reported item carries at `path`, a slash-separated member path, as JSON —
/// `Value::Null` when the item carries nothing there.
fn at(item: &Value, path: &str) -> Value {
    item.pointer(&format!("/{path}"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// One `LineItemFilter`, as the model shapes it
/// (`smithy_query(query="shape",shape="LineItemFilter")`: `@required Attribute`,
/// `@required MatchOption`, and the value lists).
fn line_item_filter(attribute: &str, match_option: &str, attribute_value: &str) -> Value {
    json!({
        "Attribute": attribute,
        "MatchOption": match_option,
        "AttributeValues": [attribute_value],
    })
}

/// A fresh emulator holding one billing group and one custom line item created from
/// `create`, returning the emulator, the group ARN and the item ARN.
async fn fixture(name: &str, create: Value) -> (Emulator, String, String) {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, name, create).await;
    (emulator, group, item)
}

/// One row of the single-write table: what the item is created as, what the update
/// changes and from which billing periods each value is then expected.
struct Row {
    case: &'static str,
    /// The `ChargeDetails` the item is CREATED with, which fix its charge kind.
    create: Value,
    /// The update body, including the `BillingPeriodRange` it is applied in.
    update: Value,
    /// The member path the row compares, slash-separated.
    path: &'static str,
    /// The value the billing periods before the write keep.
    kept: Value,
    /// The value the billing periods the write claims report.
    updated: Value,
    /// The periods that must report `kept`.
    keeping: Vec<String>,
    /// The periods that must report `updated`.
    updating: Vec<String>,
    /// The periods that must report no item at all.
    absent: Vec<String>,
}

/// THE RULE on a single write: the update's values answer for the billing period it is
/// applied from and the later ones its range covers, and every earlier period still
/// reports the value the item had there.
#[tokio::test]
async fn an_update_changes_only_the_billing_periods_it_is_applied_in() {
    let filter = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    // Every write here is applied from one of the two periods this operation may act in —
    // the current one ([`period(0)`](period)) and the previous one, which is also the one
    // the item was created from — while the EXCLUSIVE end, which that window does not
    // judge, reaches as far forward as the row needs.
    let rows: Vec<Row> = vec![
        Row {
            case: "the name, applied from an open-ended range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Name": "renamed",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            }),
            path: "Name",
            kept: json!("versioned"),
            updated: json!("renamed"),
            keeping: vec![period(-1)],
            updating: vec![period(0), period(5), period(70)],
            absent: vec![],
        },
        Row {
            case: "the description, applied from an open-ended range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Description": "a new description",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            }),
            path: "Description",
            kept: json!("the original custom line item"),
            updated: json!("a new description"),
            keeping: vec![period(-1)],
            updating: vec![period(0), period(11)],
            absent: vec![],
        },
        Row {
            case: "the flat charge value, applied in a bounded range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "ChargeDetails": { "Flat": { "ChargeValue": 42.5 } },
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(0),
                    "ExclusiveEndBillingPeriod": period(3),
                },
            }),
            path: "ChargeDetails/Flat/ChargeValue",
            kept: json!(10.0),
            updated: json!(42.5),
            keeping: vec![period(-1)],
            updating: vec![period(0), period(1), period(2)],
            // The write named the periods the item applies in from its own start on, so
            // the item applies in none of the periods from its exclusive end.
            absent: vec![period(3), period(11)],
        },
        Row {
            case: "the percentage value of a percentage item",
            create: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.0 } }),
            update: json!({
                "ChargeDetails": { "Percentage": { "PercentageValue": 7.5 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            }),
            path: "ChargeDetails/Percentage/PercentageValue",
            kept: json!(2.0),
            updated: json!(7.5),
            keeping: vec![period(-1)],
            updating: vec![period(0), period(37)],
            absent: vec![],
        },
        Row {
            case: "the line item filters",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "ChargeDetails": { "LineItemFilters": [filter.clone()] },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            }),
            path: "ChargeDetails/LineItemFilters",
            kept: Value::Null,
            updated: json!([filter.clone()]),
            keeping: vec![period(-1)],
            updating: vec![period(0), period(10)],
            absent: vec![],
        },
        Row {
            case: "a one-month range, with the month spelled without its leading zero",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Name": "one_month",
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": one_digit_month(&period(0)),
                    "ExclusiveEndBillingPeriod": one_digit_month(&period(1)),
                },
            }),
            path: "Name",
            kept: json!("versioned"),
            updated: json!("one_month"),
            keeping: vec![period(-1), one_digit_month(&period(-1))],
            updating: vec![period(0), one_digit_month(&period(0))],
            absent: vec![period(1), period(2)],
        },
        Row {
            // Applied from the PREVIOUS billing period, the other period this operation
            // may act in and the one the item was created from: the write takes over the
            // item's whole life, so no period is left reporting an earlier version.
            case: "applied from the previous billing period, bounded years later",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Name": "from_the_previous_period",
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(24),
                },
            }),
            path: "Name",
            kept: json!("versioned"),
            updated: json!("from_the_previous_period"),
            keeping: vec![],
            updating: vec![period(-1), period(0), period(23)],
            absent: vec![period(24), period(25)],
        },
    ];

    for row in rows {
        let case = row.case;
        let (emulator, _group, item) =
            fixture("versioned", json!({ "ChargeDetails": row.create })).await;
        emulator.update_ok(case, &item, row.update).await;

        for period in &row.keeping {
            let read = emulator.listed_ok(case, &item, period).await;
            assert_eq!(
                at(&read, row.path),
                row.kept,
                "case {case:?}: expected {period} to keep {} at {}, got {read}",
                row.kept,
                row.path
            );
        }
        for period in &row.updating {
            let read = emulator.listed_ok(case, &item, period).await;
            assert_eq!(
                at(&read, row.path),
                row.updated,
                "case {case:?}: expected {period} to report {} at {}, got {read}",
                row.updated,
                row.path
            );
        }
        for period in &row.absent {
            let read = emulator.listed(case, &item, period).await;
            assert!(
                read.is_none(),
                "case {case:?}: expected no item at all for {period}, got {read:?}"
            );
        }
    }
}

/// THE RULE swept over a contiguous run of billing periods: after one open-ended write,
/// every period of the item's life reports exactly one version — the earlier periods the
/// one the item had, the rest the one the write left — and no period the item applied in
/// before the write stops reporting it.
#[tokio::test]
async fn every_billing_period_of_the_item_reports_one_version_after_an_open_ended_write() {
    let (emulator, _group, item) = fixture("swept", json!({})).await;
    let case = "a sweep across the write";
    // The write is applied from the CURRENT billing period, one of the two this operation
    // may act in, so the swept run has the period the item was created in before it and
    // two years of periods after it — plus the period before the item existed at all.
    let applied_from_offset = 0;
    let applied_from = period(applied_from_offset);
    emulator
        .update_ok(
            case,
            &item,
            json!({
                "Name": "swept_update",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": applied_from },
            }),
        )
        .await;

    let before_the_item = period(-2);
    assert!(
        emulator.listed(case, &item, &before_the_item).await.is_none(),
        "case {case:?}: the item applies from {} on, so the period before it reports \
         nothing",
        created_from()
    );
    // The two years of billing periods from the one the item was created in on.
    for offset in -1..=22 {
        let probe = period(offset);
        let read = emulator.listed_ok(case, &item, &probe).await;
        let expected = if offset < applied_from_offset {
            "swept"
        } else {
            "swept_update"
        };
        assert_eq!(
            at(&read, "Name"),
            json!(expected),
            "case {case:?}: expected {probe} to report {expected:?}, got {read}"
        );
    }
}

/// THE RULE under REPEATED writes: each one takes over from the billing period it is
/// applied in, so the item holds one version per stretch of periods, every stretch keeps
/// the values its own write left, and the history reports each generation with the
/// stretch it applies in. A write applied from the period the current generation already
/// starts in opens no new stretch — one billing period holds one version of the item — so
/// it replaces that generation instead.
#[tokio::test]
async fn repeated_writes_leave_one_version_per_stretch_of_billing_periods() {
    let (emulator, group, item) = fixture("history", json!({})).await;
    let case = "three writes across the periods this operation may act in";
    // (the period the write is applied from, the name and the flat charge value it
    // leaves). The writes walk the two periods this operation may act in, oldest first:
    // the first re-states the item's whole life from the period it was created in, the
    // second opens a stretch at the current period, and the third lands in that same
    // period and so replaces the generation holding it.
    let writes = [
        (created_from(), "second", 20.0),
        (period(0), "third", 30.0),
        (period(0), "fourth", 40.0),
    ];
    for (from, name, charge_value) in &writes {
        emulator
            .update_ok(
                case,
                &item,
                json!({
                    "Name": name,
                    "ChargeDetails": { "Flat": { "ChargeValue": charge_value } },
                    "BillingPeriodRange": { "InclusiveStartBillingPeriod": from },
                }),
            )
            .await;
    }

    // (the period read, the name and the charge value the version in force there holds)
    let expected = [
        (period(-1), "second", 20.0),
        (period(0), "fourth", 40.0),
        (period(1), "fourth", 40.0),
        (period(3), "fourth", 40.0),
        (period(50), "fourth", 40.0),
    ];
    for (probe, name, charge_value) in &expected {
        let read = emulator.listed_ok(case, &item, probe).await;
        assert_eq!(
            at(&read, "Name"),
            json!(name),
            "case {case:?}: expected {probe} to report {name:?}, got {read}"
        );
        assert_eq!(
            at(&read, "ChargeDetails/Flat/ChargeValue"),
            json!(charge_value),
            "case {case:?}: expected {probe} to charge {charge_value}, got {read}"
        );
    }

    // The history is one generation per stretch, each with the stretch it applies in: the
    // generation the create left was re-stated for every period it applied in and the
    // third write replaced the second's generation in the period they share, so neither is
    // a version of the item in any billing period.
    let reported = emulator.versions(case, &item).await;
    let generations = [
        ("second", created_from(), Some(period(0))),
        ("fourth", period(0), None),
    ];
    assert_eq!(
        reported.len(),
        generations.len(),
        "case {case:?}: expected {} generations, got {reported:?}",
        generations.len()
    );
    for (version, (name, start, end)) in reported.iter().zip(generations) {
        assert_eq!(
            at(version, "Name"),
            json!(name),
            "case {case:?}: expected the generation named {name:?}, got {version}"
        );
        assert_eq!(
            at(version, "StartBillingPeriod"),
            json!(start),
            "case {case:?}: expected the {name:?} generation to start in {start}, got {version}"
        );
        assert_eq!(
            at(version, "EndBillingPeriod"),
            end.as_ref().map_or(Value::Null, |end| json!(end)),
            "case {case:?}: expected the {name:?} generation to end at {end:?}, got {version}"
        );
    }

    // The bill of a billing period is computed from the version in force in it, so the
    // charge each period carries is that generation's own charge value.
    let reported = emulator
        .proforma_by_period(case, &group, &period(-1), &period(9))
        .await;
    let expected = [
        (period(-1), 20.0),
        (period(0), 40.0),
        (period(1), 40.0),
        (period(2), 40.0),
        (period(3), 40.0),
        (period(4), 40.0),
        (period(5), 40.0),
        (period(6), 40.0),
        (period(7), 40.0),
        (period(8), 40.0),
    ];
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected one row per billing period, got {reported:?}"
    );
    for ((period, proforma), (expected_period, expected_proforma)) in reported.iter().zip(expected)
    {
        let expected_reported = period_as_reported(&expected_period);
        assert_eq!(
            period.as_str(),
            expected_reported.as_str(),
            "case {case:?}: expected the {expected_period} row reported as \
             {expected_reported:?}, got {period}"
        );
        assert!(
            (proforma - expected_proforma).abs() < TOLERANCE,
            "case {case:?}: expected the {expected_period} bill to be {expected_proforma}, got \
             {proforma}"
        );
    }
}

/// A write that names NO billing period range takes over wherever the item applies, so it
/// leaves no earlier version behind: every period of the item's life reports the values it
/// left, and the history is still the one generation.
#[tokio::test]
async fn a_write_naming_no_range_replaces_the_version_it_applies_in() {
    let (emulator, _group, item) = fixture("whole", json!({})).await;
    let case = "an update naming no billing period range";
    emulator
        .update_ok(case, &item, json!({ "Name": "replaced" }))
        .await;

    for period in [period(-1), period(4), period(21)] {
        let read = emulator.listed_ok(case, &item, &period).await;
        assert_eq!(
            at(&read, "Name"),
            json!("replaced"),
            "case {case:?}: expected {period} to report the replaced version, got {read}"
        );
    }
    let reported = emulator.versions(case, &item).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the item's one generation, got {reported:?}"
    );
}

/// One row of the no-range table: what the item is created as, the range-bearing writes
/// that freeze a generation under it, the writes that name NO billing period range, and
/// what every read then reports.
struct NoRangeRow {
    case: &'static str,
    /// The `ChargeDetails` the item is CREATED with, which fix its charge kind.
    create: Value,
    /// The range-bearing update bodies, oldest first: each freezes the generation it
    /// supersedes into the billing periods before the one it is applied from.
    framed_writes: Vec<Value>,
    /// The update bodies that name no `BillingPeriodRange` at all, in order.
    no_range_writes: Vec<Value>,
    /// The member path every probe compares, slash-separated.
    path: &'static str,
    /// `(the period read, the value at `path` the item reported for it carries)`, or
    /// `None` where the read must report no item at all.
    probes: Vec<(String, Option<Value>)>,
    /// `(Name, StartBillingPeriod, EndBillingPeriod)` of every generation the history
    /// must report, oldest first.
    generations: Vec<(&'static str, String, Option<String>)>,
    /// `(the offset of the billing period from the current one, the pro forma cost its
    /// bill must carry)`, contiguous and ascending; empty for a row whose charge the cost
    /// report prices from nothing the row sets up.
    bills: Vec<(i64, f64)>,
}

/// THE RULE for a write that names NO billing period range: it is applied to exactly the
/// billing periods the generation in effect holds, so it takes no period away from a
/// generation an earlier write already froze — every such period still reports the item
/// as it stood in it, the history still holds that generation, and the bill of that
/// period still carries its charge.
///
/// The rows span what the rule generalises over rather than one recorded call: which
/// member the no-range write changes (`Name`, `Description`, the flat charge value, the
/// `LineItemFilters`, the percentage value), the charge kind of the item, whether the
/// generation the write replaces is open-ended or bounded, how many no-range writes land
/// (one, two), the spelling of the month the frozen generation ends at, and a no-range
/// write that changes nothing at all.
#[tokio::test]
async fn a_write_naming_no_range_keeps_every_superseded_generation() {
    let filter = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    let flat = json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } });
    let rows: Vec<NoRangeRow> = vec![
        NoRangeRow {
            case: "the name, over one frozen generation",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "ChargeDetails": { "Flat": { "ChargeValue": 20.0 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({ "Name": "third" })],
            path: "Name",
            probes: vec![
                (period(-2), None),
                (period(-1), Some(json!("original"))),
                (period(0), Some(json!("third"))),
                (period(9), Some(json!("third"))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("third", period(0), None),
            ],
            bills: vec![(-1, 10.0), (0, 20.0), (1, 20.0)],
        },
        NoRangeRow {
            case: "the description, over one frozen generation",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({ "Description": "a later description" })],
            path: "Description",
            probes: vec![
                (period(-1), Some(json!("the original custom line item"))),
                (period(0), Some(json!("a later description"))),
                (period(31), Some(json!("a later description"))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("second", period(0), None),
            ],
            bills: vec![(-1, 10.0), (0, 10.0)],
        },
        NoRangeRow {
            case: "the flat charge value, over a frozen generation and a bounded current one",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(0),
                    "ExclusiveEndBillingPeriod": period(3),
                },
            })],
            no_range_writes: vec![json!({ "ChargeDetails": { "Flat": { "ChargeValue": 55.0 } } })],
            path: "ChargeDetails/Flat/ChargeValue",
            probes: vec![
                (period(-1), Some(json!(10.0))),
                (period(0), Some(json!(55.0))),
                (period(2), Some(json!(55.0))),
                // The generation the no-range write replaced was bounded, and such a
                // write claims no period beyond it.
                (period(3), None),
                (period(14), None),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("second", period(0), Some(period(3))),
            ],
            bills: vec![(-1, 10.0), (0, 55.0), (1, 55.0), (2, 55.0), (3, 0.0)],
        },
        NoRangeRow {
            case: "the line item filters, over one frozen generation",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({
                "ChargeDetails": { "LineItemFilters": [filter.clone()] },
            })],
            path: "ChargeDetails/LineItemFilters",
            probes: vec![
                (period(-1), Some(Value::Null)),
                (period(0), Some(json!([filter.clone()]))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("second", period(0), None),
            ],
            bills: vec![],
        },
        NoRangeRow {
            case: "the percentage value of a percentage item, over one frozen generation",
            create: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.0 } }),
            framed_writes: vec![json!({
                "Name": "second",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({
                "ChargeDetails": { "Percentage": { "PercentageValue": 9.0 } },
            })],
            path: "ChargeDetails/Percentage/PercentageValue",
            probes: vec![
                (period(-1), Some(json!(2.0))),
                (period(0), Some(json!(9.0))),
                (period(6), Some(json!(9.0))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("second", period(0), None),
            ],
            bills: vec![],
        },
        NoRangeRow {
            case: "two no-range writes in a row, over one frozen generation",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "ChargeDetails": { "Flat": { "ChargeValue": 20.0 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({ "Name": "third" }), json!({ "Name": "fourth" })],
            path: "Name",
            probes: vec![
                (period(-1), Some(json!("original"))),
                (period(0), Some(json!("fourth"))),
                (period(23), Some(json!("fourth"))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("fourth", period(0), None),
            ],
            bills: vec![(-1, 10.0), (0, 20.0)],
        },
        NoRangeRow {
            case: "a frozen generation ending at a month spelled without its leading zero",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": one_digit_month(&period(0)),
                },
            })],
            no_range_writes: vec![json!({ "Description": "spelled either way" })],
            path: "Description",
            probes: vec![
                (period(-1), Some(json!("the original custom line item"))),
                (
                    one_digit_month(&period(-1)),
                    Some(json!("the original custom line item")),
                ),
                (period(0), Some(json!("spelled either way"))),
                (
                    one_digit_month(&period(0)),
                    Some(json!("spelled either way")),
                ),
            ],
            // Each bound is reported as the call that set it spelled it.
            generations: vec![
                ("original", period(-1), Some(one_digit_month(&period(0)))),
                ("second", one_digit_month(&period(0)), None),
            ],
            bills: vec![(-1, 10.0), (0, 10.0)],
        },
        NoRangeRow {
            case: "a no-range write that restates what the item already holds",
            create: flat.clone(),
            framed_writes: vec![json!({
                "Name": "second",
                "ChargeDetails": { "Flat": { "ChargeValue": 20.0 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
            })],
            no_range_writes: vec![json!({ "Name": "second" })],
            path: "Name",
            probes: vec![
                (period(-1), Some(json!("original"))),
                (period(0), Some(json!("second"))),
            ],
            generations: vec![
                ("original", period(-1), Some(period(0))),
                ("second", period(0), None),
            ],
            bills: vec![(-1, 10.0), (0, 20.0)],
        },
    ];

    for row in rows {
        let case = row.case;
        let (emulator, group, item) =
            fixture("original", json!({ "ChargeDetails": row.create })).await;
        for write in row.framed_writes {
            emulator.update_ok(case, &item, write).await;
        }
        for write in row.no_range_writes {
            emulator.update_ok(case, &item, write).await;
        }

        for (probe, expected) in &row.probes {
            let read = emulator.listed(case, &item, probe).await;
            match expected {
                Some(expected) => {
                    let read = read.unwrap_or_else(|| {
                        panic!("case {case:?}: expected the item to be reported for {probe}")
                    });
                    assert_eq!(
                        at(&read, row.path),
                        *expected,
                        "case {case:?}: expected {probe} to report {expected} at {}, got {read}",
                        row.path
                    );
                }
                None => assert!(
                    read.is_none(),
                    "case {case:?}: expected no item at all for {probe}, got {read:?}"
                ),
            }
        }

        let reported = emulator.versions(case, &item).await;
        assert_eq!(
            reported.len(),
            row.generations.len(),
            "case {case:?}: expected {} generations, got {reported:?}",
            row.generations.len()
        );
        for (version, (name, start, end)) in reported.iter().zip(&row.generations) {
            assert_eq!(
                at(version, "Name"),
                json!(name),
                "case {case:?}: expected the generation named {name:?}, got {version}"
            );
            assert_eq!(
                at(version, "StartBillingPeriod"),
                json!(start),
                "case {case:?}: expected the {name:?} generation to start in {start}, got \
                 {version}"
            );
            assert_eq!(
                at(version, "EndBillingPeriod"),
                end.as_ref().map_or(Value::Null, |end| json!(end)),
                "case {case:?}: expected the {name:?} generation to end at {end:?}, got {version}"
            );
        }

        if let (Some((first, _)), Some((last, _))) = (row.bills.first(), row.bills.last()) {
            let reported = emulator
                .proforma_by_period(case, &group, &period(*first), &period(last + 1))
                .await;
            assert_eq!(
                reported.len(),
                row.bills.len(),
                "case {case:?}: expected one row per billing period, got {reported:?}"
            );
            for ((reported_period, proforma), (offset, expected_proforma)) in
                reported.iter().zip(&row.bills)
            {
                let expected_period = period(*offset);
                let expected_reported = period_as_reported(&expected_period);
                assert_eq!(
                    reported_period.as_str(),
                    expected_reported.as_str(),
                    "case {case:?}: expected the {expected_period} row reported as \
                     {expected_reported:?}, got {reported_period}"
                );
                assert!(
                    (proforma - expected_proforma).abs() < TOLERANCE,
                    "case {case:?}: expected the {expected_period} bill to be \
                     {expected_proforma}, got {proforma}"
                );
            }
        }
    }
}
