// SPDX-License-Identifier: CC0-1.0

//! The value a cost report REPORTS a billing period as, in the `Attributes` of a margin
//! summary row.
//!
//! RULE: a reported breakdown attribute carries the value the attribute DISPLAYS, so a
//! billing period is reported as its month name and four-digit year (`Nov 2023`) rather
//! than in the `YYYY-MM` form the wire names a period with — the one documented value of
//! that member (`BillingGroupCostReportResultElement$Attributes`,
//! `smithy_query(query="shape",shape="BillingGroupCostReportResultElement")`) — while the
//! product attribute beside it carries the product name as it stands.
//!
//! The rows span what the rule generalises over rather than one call: EVERY month of the
//! year (a twelve-month range, which crosses a year boundary), both spellings the
//! `BillingPeriod` pattern admits for the range's bounds, a request that names no range at
//! all, a breakdown that names the period attribute alone and one that names it beside the
//! product attribute, and a period with a charge beside one without.
//!
//! The reported form is for a client to display: it is not what orders or pages the rows,
//! so the rows still run chronologically rather than by the alphabet of those month names.

use bytes::Bytes;
use serde_json::{json, Value};
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

/// The month names a reported billing period is written with, January first.
///
/// Written out here rather than derived from the emulator, because this file is what pins
/// the rendering: the member's one documented value spells November 2023 `Nov 2023`
/// (`BillingGroupCostReportResultElement$Attributes`,
/// `smithy_query(query="shape",shape="BillingGroupCostReportResultElement")`), so the month
/// is the three-letter abbreviation of its English name.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A live emulator holding only this service, with the emulated account designated the
/// organization's PAYER account — the world in which this read is reachable at all (see
/// the `crate::service` module docs; `tests/all/payer_account_gate.rs` owns the denial
/// every other caller gets).
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
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

    /// Send a JSON body to one of this service's routes and return the status plus the
    /// parsed response body.
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
    /// every create needs.
    async fn custom_line_item(&self, group_arn: &str, name: &str, request: Value) {
        let mut body = json!({
            "Name": name,
            "Description": format!("the {name} custom line item"),
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
        });
        for (member, value) in request.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/create-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {response}"
        );
    }

    /// The rows of a SUCCESSFUL report, in response order.
    async fn rows(&self, case: &str, request: Value) -> Vec<Value> {
        let (status, body) = self
            .send("POST", "/get-billing-group-cost-report", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("BillingGroupCostReportResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// The `Attributes` of one row as `(Key, Value)` pairs, in response order.
fn attributes(row: &Value) -> Vec<(String, String)> {
    row.get("Attributes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|attribute| (member(attribute, "Key"), member(attribute, "Value")))
                .collect()
        })
        .unwrap_or_default()
}

/// One string member of an object, or the empty string when it carries none.
fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for earlier),
/// in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// A billing group bills from the period it was created in, which the wall clock decides,
/// so every period here is named as an offset from that one rather than by a fixed month.
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
/// spelling the modelled `BillingPeriod` pattern allows.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// The value a report is DUE for the billing period `billing_period`, built from
/// [`MONTH_NAMES`] rather than from the emulator: the month by name and the four-digit
/// year.
fn due_reported_period(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    let index = month.parse::<usize>().expect("a month") - 1;
    format!("{} {year}", MONTH_NAMES[index])
}

/// RULE: every month of the year is reported by its own name beside its four-digit year,
/// and the rows still run chronologically rather than by the alphabet of those names.
#[tokio::test]
async fn every_month_of_a_year_is_reported_by_its_month_name() {
    let em = Emulator::new();
    let group = em.billing_group("monthnames").await;

    // The twelve months up to and including the current one: every month name, and a year
    // boundary wherever the current month falls. Twelve is the published maximum one range
    // may cover ("You can specify up to 12 months",
    // `smithy_query(query="doc",shape="GetBillingGroupCostReportInput",member="BillingPeriodRange")`).
    let covered: Vec<String> = (-11..=0).map(period).collect();
    let rows = em
        .rows(
            "the twelve months up to the current one",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": covered[0].clone(),
                    "ExclusiveEndBillingPeriod": period(1),
                },
                "GroupBy": ["BILLING_PERIOD"],
            }),
        )
        .await;
    assert_eq!(
        rows.len(),
        covered.len(),
        "case the twelve months up to the current one: expected one row per month, got \
         {rows:?}"
    );

    let mut reported: Vec<String> = Vec::new();
    for (row, billing_period) in rows.iter().zip(&covered) {
        let case = format!("the {billing_period} row");
        let due = due_reported_period(billing_period);
        assert_eq!(
            attributes(row),
            vec![("BILLING_PERIOD".to_string(), due.clone())],
            "case {case:?}: expected the period reported as {due:?}, got {row}"
        );
        reported.push(due);
    }

    // Every month name of the catalog was exercised, each with the year of its own period.
    let mut names: Vec<&str> = reported
        .iter()
        .map(|value| value.split(' ').next().unwrap_or_default())
        .collect();
    names.sort_unstable();
    let mut catalog: Vec<&str> = MONTH_NAMES.to_vec();
    catalog.sort_unstable();
    assert_eq!(
        names, catalog,
        "case the twelve months up to the current one: expected every month name of the \
         catalog to be reported once, got {reported:?}"
    );

    // The reported form is not what orders the rows: they run by period, which the
    // alphabet of the month names does not.
    let mut alphabetical = reported.clone();
    alphabetical.sort_unstable();
    assert_ne!(
        reported, alphabetical,
        "case the twelve months up to the current one: the rows must run chronologically, \
         not by the alphabet of the reported names; got {reported:?}"
    );
}

/// One row of the table of [`a_reported_period_is_the_month_whatever_the_request_spelled`]:
/// how a request names the periods it asks about, and which periods it is then due.
struct SpellingCase {
    case: &'static str,
    /// The `BillingPeriodRange` member of the request, if it carries one.
    range: Option<Value>,
    /// The periods the report is due a row for, in `YYYY-MM`.
    due: Vec<String>,
}

/// RULE (its reach): the value reported for a period is the month that period names,
/// whichever spelling the request wrote its bounds in and whether or not it named a range
/// at all; it is the period the row is FOR, never a value echoed off the request.
#[tokio::test]
async fn a_reported_period_is_the_month_whatever_the_request_spelled() {
    let em = Emulator::new();
    let group = em.billing_group("spellings").await;

    let cases = vec![
        SpellingCase {
            case: "no range at all, so the current billing period",
            range: None,
            due: vec![period(0)],
        },
        SpellingCase {
            case: "a range whose bounds carry their leading zero",
            range: Some(json!({
                "InclusiveStartBillingPeriod": period(-1),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            due: vec![period(-1), period(0)],
        },
        SpellingCase {
            case: "the same range with its months spelled without their leading zero",
            range: Some(json!({
                "InclusiveStartBillingPeriod": one_digit_month(&period(-1)),
                "ExclusiveEndBillingPeriod": one_digit_month(&period(1)),
            })),
            due: vec![period(-1), period(0)],
        },
        SpellingCase {
            case: "a range of a single month",
            range: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            })),
            due: vec![period(0)],
        },
    ];

    for case in cases {
        let mut request = json!({ "Arn": group.clone(), "GroupBy": ["BILLING_PERIOD"] });
        if let Some(range) = &case.range {
            request["BillingPeriodRange"] = range.clone();
        }
        let rows = em.rows(case.case, request).await;
        let reported: Vec<Vec<(String, String)>> = rows.iter().map(attributes).collect();
        let due: Vec<Vec<(String, String)>> = case
            .due
            .iter()
            .map(|billing_period| {
                vec![(
                    "BILLING_PERIOD".to_string(),
                    due_reported_period(billing_period),
                )]
            })
            .collect();
        assert_eq!(
            reported, due,
            "case {:?}: expected those periods reported by month name, got {rows:?}",
            case.case
        );
    }
}

/// RULE (its boundary): only the period attribute is rendered — the product attribute
/// beside it carries the product name as the charges present it — and a period the group
/// holds a charge in is reported the same way as one it does not.
#[tokio::test]
async fn only_the_period_attribute_is_rendered_as_a_month() {
    let em = Emulator::new();
    let group = em.billing_group("twoattrs").await;

    // One charge presented under a named product, in the previous period only, and one
    // with no presentation details of its own, from the previous period on: so the two
    // periods of the report differ in how many products they are charged under.
    em.custom_line_item(
        &group,
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
    em.custom_line_item(
        &group,
        "unpresented",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 2.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
    )
    .await;

    let range = json!({
        "InclusiveStartBillingPeriod": period(-1),
        "ExclusiveEndBillingPeriod": period(1),
    });

    // Both attributes, in the order the request listed them: the period by month name,
    // the product as it stands.
    for group_by in [
        json!(["BILLING_PERIOD", "PRODUCT_NAME"]),
        json!(["PRODUCT_NAME", "BILLING_PERIOD"]),
    ] {
        let case = format!("grouped by {group_by}");
        let rows = em
            .rows(
                &case,
                json!({
                    "Arn": group.clone(),
                    "BillingPeriodRange": range.clone(),
                    "GroupBy": group_by.clone(),
                }),
            )
            .await;
        // (the period of the row, the product of the row), in the order the rows run.
        let expected = [
            (period(-1), OWN_PRODUCT_NAME.to_string()),
            (period(-1), "S3".to_string()),
            (period(0), OWN_PRODUCT_NAME.to_string()),
        ];
        assert_eq!(
            rows.len(),
            expected.len(),
            "case {case:?}: expected one row per period and product, got {rows:?}"
        );
        for (row, (billing_period, product)) in rows.iter().zip(&expected) {
            let row_case = format!("{case}: the {product} row of {billing_period}");
            let period_pair = (
                "BILLING_PERIOD".to_string(),
                due_reported_period(billing_period),
            );
            let product_pair = ("PRODUCT_NAME".to_string(), product.clone());
            let due = if group_by[0] == json!("BILLING_PERIOD") {
                vec![period_pair, product_pair]
            } else {
                vec![product_pair, period_pair]
            };
            assert_eq!(
                attributes(row),
                due,
                "case {row_case:?}: expected the period by month name and the product as it \
                 stands, in the requested order, got {row}"
            );
        }
    }
}
