// SPDX-License-Identifier: CC0-1.0

//! What a cost report prices a custom line item at in ONE billing period, end to end over
//! the wire: `CreateCustomLineItem` (-> `BatchAssociateResourcesToCustomLineItem`) ->
//! `UpdateCustomLineItem` / `DeleteCustomLineItem` -> `ListBillingGroupCostReports` and
//! `GetBillingGroupCostReport`.
//!
//! RULE: the charge a custom line item puts on a billing period's pro forma bill is the
//! charge of the GENERATION of that item that applies in that period, and an item no
//! generation of which applies in the period charges nothing there — however the report
//! reaches the item, so a percentage item is lent nothing by an associated item the period
//! does not hold. Both cost reports compute their pro forma amount from that one rule, so
//! they answer the same amount for the same period.
//!
//! The rows span what the rule generalises over rather than one call: which billing period
//! the item was taken out of (the period asked about, the one before it, none at all),
//! which periods are left to it (the earlier ones, the later ones, both), how many
//! generations the item has (one, two, with neither of them applying in the period asked
//! about), how the report reaches it (as a top-level item of the group, as the child of a
//! percentage item, as the child of a child), how many children a percentage item has (one,
//! two, one of them gone), how many percentage items share a child, how the association was
//! made (at create / by a later batch call), the charge type of the items (fee / credit) and
//! the charge value (whole, fractional).

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

/// How far two amounts may differ and still be the same amount: the reported strings are
/// decimals rounded well inside this, so a comparison at this tolerance pins the amount
/// without pinning the rendering.
const TOLERANCE: f64 = 1e-6;

/// One billing group, arranged so that the rule has something to say about each of
/// several billing periods.
struct Case {
    /// What the arrangement is, for the message of every assertion about it.
    label: &'static str,
    group: String,
    /// The pro forma cost due in each billing period: `(billing period, amount)`.
    expected: Vec<(String, f64)>,
}

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
    /// one — the world in which the reads under test are reachable at all. The denial every
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
        member_str(&body, "Arn", &format!("billing group {name:?}"))
    }

    /// Create a custom line item of `group` carrying `charge_details`, in effect from the
    /// billing period `from` on, and return its ARN.
    async fn item(&self, group: &str, name: &str, from: &str, charge_details: Value) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-custom-line-item",
                &json!({
                    "Name": name,
                    "Description": format!("the {name} custom line item"),
                    "BillingGroupArn": group,
                    "ChargeDetails": charge_details,
                    "BillingPeriodRange": { "InclusiveStartBillingPeriod": from },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {body}"
        );
        member_str(&body, "Arn", &format!("custom line item {name:?}"))
    }

    /// A FLAT custom line item charging `value`, in effect from `from` on.
    async fn flat(
        &self,
        group: &str,
        name: &str,
        charge_type: &str,
        value: f64,
        from: &str,
    ) -> String {
        self.item(
            group,
            name,
            from,
            json!({ "Type": charge_type, "Flat": { "ChargeValue": value } }),
        )
        .await
    }

    /// A PERCENTAGE custom line item taking `percentage_value` percent of the resources
    /// `associated` names, associated AT CREATE, and in effect from `from` on.
    async fn percentage(
        &self,
        group: &str,
        name: &str,
        charge_type: &str,
        percentage_value: f64,
        from: &str,
        associated: &[&str],
    ) -> String {
        let mut details = json!({
            "Type": charge_type,
            "Percentage": { "PercentageValue": percentage_value },
        });
        if !associated.is_empty() {
            details["Percentage"]["AssociatedValues"] = json!(associated);
        }
        self.item(group, name, from, details).await
    }

    /// Associate `resources` to the percentage custom line item `target` by a LATER call,
    /// which is in effect from the billing period the call is made in on.
    async fn associate(&self, target: &str, resources: &[&str]) {
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

    /// Change the flat charge value of an item from the billing period `from` on, which
    /// opens a NEW generation of it and leaves the earlier periods to the one before it.
    async fn update_charge(&self, case: &str, arn: &str, value: f64, from: &str) {
        let (status, body) = self
            .send(
                "POST",
                "/update-custom-line-item",
                &json!({
                    "Arn": arn,
                    "ChargeDetails": { "Flat": { "ChargeValue": value } },
                    "BillingPeriodRange": { "InclusiveStartBillingPeriod": from },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer HTTP 200, got {status} with {body}"
        );
    }

    /// Delete an item from the half-open billing period range `[start, end)` only, so its
    /// record survives for every other period it applied in.
    async fn delete_from(&self, case: &str, arn: &str, start: &str, end: &str) {
        let (status, body) = self
            .send(
                "POST",
                "/delete-custom-line-item",
                &json!({
                    "Arn": arn,
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": start,
                        "ExclusiveEndBillingPeriod": end,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer HTTP 200, got {status} with {body}"
        );
    }

    /// The `ProformaCost` `ListBillingGroupCostReports` reports for one group in one
    /// billing period.
    async fn listed_proforma(&self, case: &str, group: &str, billing_period: &str) -> f64 {
        let (status, body) = self
            .send(
                "POST",
                "/list-billing-group-cost-reports",
                &json!({
                    "BillingPeriod": billing_period,
                    "Filters": { "BillingGroupArns": [group] },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let rows = rows_of(&body, "BillingGroupCostReports");
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: the filter names one stored group, so one row is due; got {body}"
        );
        amount(&rows[0], "ProformaCost", case)
    }

    /// The `ProformaCost` `GetBillingGroupCostReport` reports for one group over the one
    /// billing period `billing_period`, asked for as the half-open range that covers it.
    async fn reported_proforma(
        &self,
        case: &str,
        group: &str,
        billing_period: &str,
        next_period: &str,
    ) -> f64 {
        let (status, body) = self
            .send(
                "POST",
                "/get-billing-group-cost-report",
                &json!({
                    "Arn": group,
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": billing_period,
                        "ExclusiveEndBillingPeriod": next_period,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let rows = rows_of(&body, "BillingGroupCostReportResults");
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: no breakdown was asked for, so one row is due; got {body}"
        );
        amount(&rows[0], "ProformaCost", case)
    }

    /// Does `ListCustomLineItems` report a version of the item for `billing_period`?
    async fn lists_item(&self, case: &str, arn: &str, billing_period: &str) -> bool {
        let (status, body) = self
            .send(
                "POST",
                "/list-custom-line-items",
                &json!({
                    "BillingPeriod": billing_period,
                    "Filters": { "Arns": [arn] },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        !rows_of(&body, "CustomLineItems").is_empty()
    }
}

/// A string member of a response body.
fn member_str(body: &Value, member: &str, label: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected a {member} member, got {body}"))
        .to_string()
}

/// The elements of a list member of a response body, in response order.
fn rows_of(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
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

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for earlier),
/// in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// A custom line item may be created into, and deleted from, only the current or the
/// previous billing period, which the wall clock decides, so every period named here is an
/// offset from the service's own current one rather than a fixed month.
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

/// The billing period one month after `billing_period` — the exclusive end of the
/// half-open range that covers exactly that one period.
///
/// `pub(crate)` so the sibling suites that name a one-period range step the month
/// through this one helper (see
/// `tests/all/create_custom_line_item_association_periods.rs`).
pub(crate) fn month_after(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    let total = year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month");
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// RULE: the pro forma charge of a custom line item in a billing period is the charge of
/// the generation of it that applies in that period, and an item no generation of which
/// applies there charges nothing in it — whether the report reaches it as an item of the
/// group or through a percentage item associated to it.
#[tokio::test]
async fn an_item_no_generation_of_which_applies_in_a_period_charges_nothing_in_it() {
    let em = Emulator::new();
    let previous = period(-1);
    let current = period(0);
    let next = period(1);
    let mut cases: Vec<Case> = Vec::new();

    // A flat child taken out of the period the report covers: the parent percentage item
    // still exists there, and the child's record survives for the periods on either side
    // of the delete — but in the period the delete took, no generation of the child
    // applies, so neither the child nor the parent's percentage of it is charged.
    {
        let group = em.billing_group("deletedhere").await;
        let child = em.flat(&group, "childhere", "FEE", 100.0, &previous).await;
        em.percentage(&group, "parenthere", "FEE", 10.0, &previous, &[&child])
            .await;
        em.delete_from("a flat child deleted here", &child, &current, &next)
            .await;
        cases.push(Case {
            label: "a flat child deleted out of the period the report covers",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 0.0),
                (next.clone(), 110.0),
            ],
        });
    }

    // The same arrangement with the delete taking the EARLIER period instead: the period
    // that lost the child charges nothing, and the periods that kept it charge in full.
    {
        let group = em.billing_group("deletedbefore").await;
        let child = em.flat(&group, "childbefore", "FEE", 100.0, &previous).await;
        em.percentage(&group, "parentbefore", "FEE", 10.0, &previous, &[&child])
            .await;
        em.delete_from("a flat child deleted before", &child, &previous, &current)
            .await;
        cases.push(Case {
            label: "a flat child deleted out of the period before the one the report covers",
            group,
            expected: vec![
                (previous.clone(), 0.0),
                (current.clone(), 110.0),
                (next.clone(), 110.0),
            ],
        });
    }

    // No delete at all: the whole arrangement is charged in every period it applies in,
    // which is the row the rule has to leave alone.
    {
        let group = em.billing_group("undeleted").await;
        let child = em.flat(&group, "childkept", "FEE", 100.0, &previous).await;
        em.percentage(&group, "parentkept", "FEE", 10.0, &previous, &[&child])
            .await;
        cases.push(Case {
            label: "a flat child no delete touched",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 110.0),
                (next.clone(), 110.0),
            ],
        });
    }

    // A chain: a percentage item of a percentage item of a flat child. The period the
    // child was deleted from charges nothing anywhere along the chain.
    {
        let group = em.billing_group("chain").await;
        let base = em.flat(&group, "chainbase", "FEE", 200.0, &previous).await;
        let middle = em
            .percentage(&group, "chainmiddle", "FEE", 50.0, &previous, &[&base])
            .await;
        em.percentage(&group, "chaintop", "FEE", 10.0, &previous, &[&middle])
            .await;
        em.delete_from("a chained child deleted here", &base, &current, &next)
            .await;
        cases.push(Case {
            label: "the child of the child of a percentage item deleted out of the period",
            group,
            expected: vec![
                (previous.clone(), 310.0),
                (current.clone(), 0.0),
                (next.clone(), 310.0),
            ],
        });
    }

    // The PARENT is the one deleted out of the period: the child it was taken over still
    // applies there, so that period charges the child alone.
    {
        let group = em.billing_group("parentgone").await;
        let child = em.flat(&group, "childstays", "FEE", 100.0, &previous).await;
        let parent = em
            .percentage(&group, "parentdeleted", "FEE", 10.0, &previous, &[&child])
            .await;
        em.delete_from("a parent deleted here", &parent, &current, &next)
            .await;
        cases.push(Case {
            label: "the percentage item deleted out of the period, its child kept",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 100.0),
                (next.clone(), 110.0),
            ],
        });
    }

    // Two generations of the child, both of which apply somewhere: the period before the
    // write is charged what the child charged then, and the periods from it on what it
    // charges now.
    {
        let group = em.billing_group("twogenerations").await;
        let child = em.flat(&group, "childraised", "FEE", 100.0, &previous).await;
        em.percentage(&group, "parentraised", "FEE", 10.0, &previous, &[&child])
            .await;
        em.update_charge("a child whose charge was raised", &child, 300.0, &current)
            .await;
        cases.push(Case {
            label: "a child whose charge a later write changed from the period on",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 330.0),
                (next.clone(), 330.0),
            ],
        });
    }

    // Two generations of the child and a delete that takes the period the NEWER one
    // claimed: neither generation applies in that period, so it charges nothing there
    // although both generations' charge values are stored.
    {
        let group = em.billing_group("neithergeneration").await;
        let child = em.flat(&group, "childsplit", "FEE", 100.0, &previous).await;
        em.percentage(&group, "parentsplit", "FEE", 10.0, &previous, &[&child])
            .await;
        em.update_charge("a child split into two generations", &child, 300.0, &current)
            .await;
        em.delete_from("a child split into two generations", &child, &current, &next)
            .await;
        cases.push(Case {
            label: "a child with two generations, neither of which applies in the period",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 0.0),
                (next.clone(), 330.0),
            ],
        });
    }

    // A CREDIT arrangement: the charges move the bill the other way, and the period the
    // child was deleted from credits nothing.
    {
        let group = em.billing_group("credits").await;
        let child = em
            .flat(&group, "creditchild", "CREDIT", 80.0, &previous)
            .await;
        em.percentage(&group, "creditparent", "CREDIT", 25.0, &previous, &[&child])
            .await;
        em.delete_from("a credit child deleted here", &child, &current, &next)
            .await;
        cases.push(Case {
            label: "a credit child deleted out of the period",
            group,
            expected: vec![
                (previous.clone(), -100.0),
                (current.clone(), 0.0),
                (next.clone(), -100.0),
            ],
        });
    }

    // A fractional charge, so the amount is not a whole number of dollars.
    {
        let group = em.billing_group("fractional").await;
        let child = em
            .flat(&group, "fractionalchild", "FEE", 12.5, &previous)
            .await;
        em.percentage(&group, "fractionalparent", "FEE", 8.0, &previous, &[&child])
            .await;
        em.delete_from("a fractional child deleted here", &child, &current, &next)
            .await;
        cases.push(Case {
            label: "a child charging a fractional amount deleted out of the period",
            group,
            expected: vec![
                (previous.clone(), 13.5),
                (current.clone(), 0.0),
                (next.clone(), 13.5),
            ],
        });
    }

    // ONE of two children deleted out of the period: the percentage is taken over the
    // other child alone there, and over both in the periods that hold both.
    {
        let group = em.billing_group("twochildren").await;
        let kept = em.flat(&group, "keptchild", "FEE", 100.0, &previous).await;
        let dropped = em.flat(&group, "droppedchild", "FEE", 40.0, &previous).await;
        em.percentage(
            &group,
            "twochildparent",
            "FEE",
            10.0,
            &previous,
            &[&kept, &dropped],
        )
        .await;
        em.delete_from("one of two children deleted here", &dropped, &current, &next)
            .await;
        cases.push(Case {
            label: "one of a percentage item's two children deleted out of the period",
            group,
            expected: vec![
                (previous.clone(), 154.0),
                (current.clone(), 110.0),
                (next.clone(), 154.0),
            ],
        });
    }

    // One child SHARED by two percentage items — the charge the report computes once and
    // lends to both. The later batch association is in effect from the current billing
    // period on, so the second parent is lent nothing in the earlier period.
    {
        let group = em.billing_group("sharedchild").await;
        let child = em.flat(&group, "sharedbase", "FEE", 100.0, &previous).await;
        em.percentage(&group, "sharedfirst", "FEE", 10.0, &previous, &[&child])
            .await;
        let second = em
            .percentage(&group, "sharedsecond", "FEE", 20.0, &previous, &[])
            .await;
        em.associate(&second, &[&child]).await;
        em.delete_from("a shared child deleted here", &child, &current, &next)
            .await;
        cases.push(Case {
            label: "a child two percentage items share, deleted out of the period",
            group,
            expected: vec![
                (previous.clone(), 110.0),
                (current.clone(), 0.0),
                (next.clone(), 130.0),
            ],
        });
    }

    for case in &cases {
        for (billing_period, expected) in &case.expected {
            let label = format!("{} — the report for {billing_period}", case.label);
            let via_report = em
                .reported_proforma(
                    &label,
                    &case.group,
                    billing_period,
                    &month_after(billing_period),
                )
                .await;
            assert!(
                (via_report - expected).abs() < TOLERANCE,
                "case {label:?}: expected GetBillingGroupCostReport to report ProformaCost \
                 {expected}, got {via_report}"
            );
            // The listing reports a billing group only from the period the group was
            // created in on — every group here was created in the current one — so the
            // earlier period is asserted through the report that takes a range.
            if billing_period.as_str() >= current.as_str() {
                let via_listing = em.listed_proforma(&label, &case.group, billing_period).await;
                assert!(
                    (via_listing - expected).abs() < TOLERANCE,
                    "case {label:?}: expected ListBillingGroupCostReports to report \
                     ProformaCost {expected}, got {via_listing}"
                );
            }
        }
    }
}

/// RULE: the periods a cost report charges an item's value in are exactly the periods a
/// period-scoped read reports a version of that item for, for a child of a percentage item
/// as much as for an item of the group.
#[tokio::test]
async fn the_report_charges_an_item_only_in_the_periods_a_read_reports_it_for() {
    let em = Emulator::new();
    let previous = period(-1);
    let current = period(0);
    let next = period(1);

    let group = em.billing_group("agreement").await;
    let child = em
        .flat(&group, "agreementchild", "FEE", 100.0, &previous)
        .await;
    em.percentage(&group, "agreementparent", "FEE", 10.0, &previous, &[&child])
        .await;
    em.delete_from("the agreement child", &child, &current, &next)
        .await;

    for billing_period in [&previous, &current, &next] {
        let case = format!("the agreement arrangement in {billing_period}");
        let reported = em.lists_item(&case, &child, billing_period).await;
        let proforma = em
            .reported_proforma(
                &case,
                &group,
                billing_period,
                &month_after(billing_period),
            )
            .await;
        // The child charges 100 and the parent 10% of it, so a period that holds the
        // child is charged 110 and a period that does not is charged nothing at all.
        let expected = if reported { 110.0 } else { 0.0 };
        assert!(
            (proforma - expected).abs() < TOLERANCE,
            "case {case:?}: ListCustomLineItems reports the child = {reported}, so \
             ProformaCost {expected} is due; got {proforma}"
        );
    }

    // The two deleted-from and kept periods really are different answers from the same
    // store, so the agreement above is not vacuous.
    assert!(
        em.lists_item("the agreement child in the previous period", &child, &previous)
            .await,
        "case the agreement child in the previous period: the delete did not name that \
         period, so a read of it must still report the child"
    );
    assert!(
        !em.lists_item("the agreement child in the current period", &child, &current)
            .await,
        "case the agreement child in the current period: the delete named that period, so \
         no read of it may report the child"
    );
}
