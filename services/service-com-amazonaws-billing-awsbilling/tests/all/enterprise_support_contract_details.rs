// SPDX-License-Identifier: CC0-1.0

//! `GetEnterpriseSupportContractDetails`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: for a billing
//! month in the past it reports the Enterprise Support contract persisted in
//! `ComAmazonawsBillingAwsbillingState::enterprise_support_contract` as it
//! stood for that month — the pricing plans it was billed under in the month's
//! billing period, and so whether it was active in that month at all; the
//! method that distributes the total Support charge and each charged payer
//! account's pre-configured share of it; how Reserved Instance and Savings
//! Plans fees are treated; the accounts of
//! `ComAmazonawsBillingAwsbillingState::consolidated_billing_family` the
//! contract covers; and the month's own proration instant and additional
//! charges. It writes nothing.
//!
//! The emulator runs no billing pipeline, so nothing accrues a month's
//! proration or additional charges and the contract carries the standard plan
//! and nothing else on a fresh emulator: like the charge-summary suite, the
//! tables below put the contract in state directly (the same shared engine the
//! router hands every handler) and then assert what the API reports for it. The
//! rows differ along everything the rule generalises over: the period a pricing
//! plan is in effect for relative to the requested month, the number of plans,
//! the allocation method and the percentages configured under it, each
//! documented treatment method, the covered accounts and which of them are
//! charged before private discounts, the billing month, and whether a month
//! carries proration and additional charges at all.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    billing_period, caller_account_id, EnterpriseSupportAdditionalCharge, EnterpriseSupportContract,
    EnterpriseSupportPricingPlan, EnterpriseSupportPricingTier, ENTERPRISE_STANDARD_PLAN_ID,
    ENTERPRISE_STANDARD_PLAN_NAME, MAX_ADDITIONAL_SUPPORT_CHARGES,
    SUPPORT_ALLOCATION_FIXED_PERCENTAGE, SUPPORT_ALLOCATION_PROPORTIONAL,
    SUPPORT_TREATMENT_AMORTIZED, SUPPORT_TREATMENT_AMORTIZED_ALL,
    SUPPORT_TREATMENT_AMORTIZED_CUSTOM, SUPPORT_TREATMENT_NONE, SUPPORT_TREATMENT_UPFRONT,
};
use service_com_amazonaws_billing_awsbilling::{ComAmazonawsBillingAwsbillingState, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// A second and third 12-digit account, added to the consolidated billing
/// family so the contract covers them.
const MEMBER_ACCOUNT_ID: &str = "123456789012";
const OTHER_MEMBER_ACCOUNT_ID: &str = "210987654321";

/// A billing month far enough in the past that the clock cannot move it, and
/// the month before it, so a month's answer can be shown to carry only its own
/// detail.
const PAST_MONTH: &str = "2024-03";
const EARLIER_MONTH: &str = "2024-02";

/// The first instant of [`PAST_MONTH`] and of the month after it, as seconds
/// past the epoch: the half-open UTC billing period the month's charges accrue
/// in, which a pricing plan's own period is compared against.
const PAST_MONTH_START_SECS: u64 = 1_709_251_200;
const PAST_MONTH_END_SECS: u64 = 1_711_929_600;

/// An instant inside [`PAST_MONTH`] — ten days into it — so a plan can start or
/// end part way through the month.
const MID_PAST_MONTH_SECS: u64 = PAST_MONTH_START_SECS + 10 * 86_400;

/// One minor unit of currency (a cent), the scale the stored amounts use.
const CENTS: i64 = 100;

/// One router over ONE shared engine — what `core/router` builds in production,
/// so every call in a test observes the same state.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Ask for one billing month's Enterprise Support contract details.
async fn details(router: &MultiServiceRouter, billing_month: &str) -> (u16, Value) {
    call(
        router,
        "GetEnterpriseSupportContractDetails",
        &json!({ "billingMonth": billing_month }),
    )
    .await
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A month as the count of whole calendar months before it, so a month can be
/// named relative to today's.
fn month_index(year: i32, month: i32) -> i32 {
    year * 12 + month - 1
}

/// The billing month `offset` whole months from the one the clock is in now.
fn month_from_now(offset: i32) -> String {
    let (year, month) = billing_period(SystemTime::now()).expect("the clock names a month");
    let index = month_index(year, month) + offset;
    format!("{:04}-{:02}", index.div_euclid(12), index.rem_euclid(12) + 1)
}

/// An instant `secs` seconds past the epoch.
fn instant(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// Change this emulator's stored Enterprise Support contract.
fn with_contract(
    engine: &Arc<engine::InMemoryEngine>,
    change: impl FnOnce(&mut EnterpriseSupportContract),
) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    change(&mut state.enterprise_support_contract);
}

/// The stored contract, so a read can be shown to change nothing in it.
fn stored_contract(engine: &Arc<engine::InMemoryEngine>) -> EnterpriseSupportContract {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    state.enterprise_support_contract.clone()
}

/// Put `account_id` in the consolidated billing family, so the contract covers
/// it.
fn add_family_account(engine: &Arc<engine::InMemoryEngine>, account_id: &str) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    state
        .consolidated_billing_family
        .insert(account_id.to_string(), "Linked Account".to_string());
}

/// A pricing plan in effect over `start_secs..end_secs`, with one tier charging
/// a tenth of the spend. Only its period and its id matter to the reads below.
fn plan(id: &str, start_secs: u64, end_secs: Option<u64>) -> EnterpriseSupportPricingPlan {
    EnterpriseSupportPricingPlan {
        pricing_plan_id: id.to_string(),
        name: "Enterprise Support (negotiated)".to_string(),
        description: None,
        start_date: instant(start_secs),
        end_date: end_secs.map(instant),
        plan_discount_basis_points: 0,
        discount_applies_to_minimum_charge: false,
        minimum_charge_minor: 15_000 * CENTS,
        tiered: true,
        tiers: vec![EnterpriseSupportPricingTier {
            tier_minimum_minor: 0,
            tier_maximum_minor: None,
            base_charge_minor: 0,
            additional_rate_basis_points: 1_000,
            aggregate_charges_adjustment_minor: 0,
            incremental: true,
        }],
    }
}

/// The `pricingPlanId` of each plan a response reports, in the order reported.
fn reported_plan_ids(body: &Value) -> Vec<String> {
    body["pricingPlans"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plan| {
            plan["pricingPlanId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A contract is active in a billing month when it was billed under a pricing
/// plan in that month: the plans it reports are the ones whose own period
/// overlaps the month's billing period, and a month no plan of the contract was
/// in effect for is reported inactive with no plan at all.
#[tokio::test]
async fn reports_the_plans_in_effect_for_the_month_and_whether_the_contract_was_active() {
    // (case, the plans stored on the contract, isContractActive, the plan ids
    //  the answer must report, earliest in effect first).
    let cases: Vec<(&str, Vec<EnterpriseSupportPricingPlan>, bool, Vec<&str>)> = vec![
        (
            "the standard plan, in effect from the epoch with no end date",
            vec![EnterpriseSupportPricingPlan::standard()],
            true,
            vec![ENTERPRISE_STANDARD_PLAN_ID],
        ),
        (
            "a plan that ended when the month began",
            vec![plan("ended", 0, Some(PAST_MONTH_START_SECS))],
            false,
            vec![],
        ),
        (
            "a plan that takes effect when the month ends",
            vec![plan("later", PAST_MONTH_END_SECS, None)],
            false,
            vec![],
        ),
        (
            "a plan that ends part way through the month",
            vec![plan("ending", 0, Some(MID_PAST_MONTH_SECS))],
            true,
            vec!["ending"],
        ),
        (
            "a plan replaced part way through the month",
            vec![
                plan("replaced", 0, Some(MID_PAST_MONTH_SECS)),
                plan("replacement", MID_PAST_MONTH_SECS, None),
            ],
            true,
            vec!["replaced", "replacement"],
        ),
        ("no plan at all", vec![], false, vec![]),
    ];

    for (case, plans, is_active, plan_ids) in cases {
        let (router, engine) = emulator();
        with_contract(&engine, |contract| contract.pricing_plans = plans);

        let (status, body) = details(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["isContractActive"].as_bool(),
            Some(is_active),
            "case {case:?}: expected isContractActive {is_active}, got {body}"
        );
        assert_eq!(
            reported_plan_ids(&body),
            plan_ids,
            "case {case:?}: expected the plans in effect for the month, got {body}"
        );
    }
}

/// The reported plan is the stored plan, member for member — the same
/// projection the charge summary reports the plan it priced a month by.
#[tokio::test]
async fn reports_the_stored_pricing_plan_member_for_member() {
    let (router, _engine) = emulator();

    let (status, body) = details(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"the standard plan\": expected HTTP 200, got {status} {body}"
    );
    let plan = &body["pricingPlans"][0];
    assert_eq!(
        plan["pricingPlanId"].as_str(),
        Some(ENTERPRISE_STANDARD_PLAN_ID),
        "case \"the standard plan\": expected the stored plan id, got {plan}"
    );
    assert_eq!(
        plan["name"].as_str(),
        Some(ENTERPRISE_STANDARD_PLAN_NAME),
        "case \"the standard plan\": expected the stored plan name, got {plan}"
    );
    assert_eq!(
        plan["minimumCharge"].as_str(),
        Some("15000.00"),
        "case \"the standard plan\": expected the stored monthly minimum, got {plan}"
    );
    assert_eq!(
        plan["planDiscountPercent"].as_str(),
        Some("0.00"),
        "case \"the standard plan\": expected no plan discount, got {plan}"
    );
    assert_eq!(
        plan["tiered"].as_str(),
        Some("true"),
        "case \"the standard plan\": expected a tiered plan, got {plan}"
    );
    assert_eq!(
        plan["startDate"].as_f64(),
        Some(0.0),
        "case \"the standard plan\": expected the stored start date, got {plan}"
    );
    assert!(
        plan["endDate"].is_null(),
        "case \"the standard plan\": expected no end date on a plan still in effect, got {plan}"
    );
    // The published schedule's first tier: 10% of the first 150,000 of spend.
    let tier = &plan["tiers"][0];
    assert_eq!(
        (
            tier["tierMinimum"].as_str(),
            tier["tierMaximum"].as_str(),
            tier["additionalPercentageOfAggregateCharges"].as_str(),
            tier["incremental"].as_bool(),
        ),
        (Some("0.00"), Some("150000.00"), Some("0.10"), Some(true)),
        "case \"the standard plan\": expected the stored first tier, got {tier}"
    );
}

/// Every treatment method the model documents is reported as stored, and an
/// amortization start date is reported only for the method the model says
/// carries one: `supportReservedInstanceAmortizationStartDate` "is Null for all
/// other treatment methods" than `AmortizedCustom`, and
/// `supportSavingsPlansAmortizationStartDate` "is applicable when
/// supportSavingsPlansTreatmentMethod = Amortized and is Null for all other
/// methods".
///
/// Source: the `supportReservedInstanceTreatmentMethod` member doc, which lists
/// the five valid values, and the two amortization-date member docs.
#[tokio::test]
async fn reports_each_treatment_method_and_only_its_own_amortization_date() {
    /// The stored amortization start date, an instant before the month.
    const AMORTIZATION_SECS: u64 = 1_700_000_000;

    // (case/method, whether the Reserved Instance amortization start date is
    //  reported, whether the Savings Plans one is).
    let cases: [(&str, bool, bool); 5] = [
        (SUPPORT_TREATMENT_NONE, false, false),
        (SUPPORT_TREATMENT_UPFRONT, false, false),
        (SUPPORT_TREATMENT_AMORTIZED, false, true),
        (SUPPORT_TREATMENT_AMORTIZED_CUSTOM, true, false),
        (SUPPORT_TREATMENT_AMORTIZED_ALL, false, false),
    ];

    for (method, reserved_instance_date, savings_plans_date) in cases {
        let (router, engine) = emulator();
        with_contract(&engine, |contract| {
            contract.reserved_instance_treatment_method = method.to_string();
            contract.savings_plans_treatment_method = method.to_string();
            contract.reserved_instance_amortization_start_date = Some(instant(AMORTIZATION_SECS));
            contract.savings_plans_amortization_start_date = Some(instant(AMORTIZATION_SECS));
        });

        let (status, body) = details(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {method:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            (
                body["supportReservedInstanceTreatmentMethod"].as_str(),
                body["supportSavingsPlansTreatmentMethod"].as_str(),
            ),
            (Some(method), Some(method)),
            "case {method:?}: expected both stored treatment methods reported, got {body}"
        );
        assert_eq!(
            body["supportReservedInstanceAmortizationStartDate"].as_f64(),
            reserved_instance_date.then_some(AMORTIZATION_SECS as f64),
            "case {method:?}: expected the Reserved Instance amortization date \
             {reserved_instance_date} reported, got {body}"
        );
        assert_eq!(
            body["supportSavingsPlansAmortizationStartDate"].as_f64(),
            savings_plans_date.then_some(AMORTIZATION_SECS as f64),
            "case {method:?}: expected the Savings Plans amortization date \
             {savings_plans_date} reported, got {body}"
        );
    }
}

/// The accounts the contract covers are the accounts of the consolidated
/// billing family, in ascending account order, each carrying whether its
/// Support charges are calculated before its private discounts.
#[tokio::test]
async fn reports_the_covered_accounts_in_ascending_order() {
    let (router, engine) = emulator();
    add_family_account(&engine, MEMBER_ACCOUNT_ID);
    add_family_account(&engine, OTHER_MEMBER_ACCOUNT_ID);
    with_contract(&engine, |contract| {
        contract
            .gdn_accounts
            .insert(MEMBER_ACCOUNT_ID.to_string());
    });

    let (status, body) = details(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"three covered accounts\": expected HTTP 200, got {status} {body}"
    );

    let mut expected: Vec<(&str, bool)> = vec![
        (caller_account_id(), false),
        (MEMBER_ACCOUNT_ID, true),
        (OTHER_MEMBER_ACCOUNT_ID, false),
    ];
    expected.sort();
    let reported: Vec<(String, bool)> = body["contractPayerAccountIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|account| {
            (
                account["accountId"].as_str().unwrap_or_default().to_string(),
                account["isGdn"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    let expected: Vec<(String, bool)> = expected
        .into_iter()
        .map(|(account_id, is_gdn)| (account_id.to_string(), is_gdn))
        .collect();
    assert_eq!(
        reported, expected,
        "case \"three covered accounts\": expected the family's accounts with their \
         pre-discount flag, got {body}"
    );
}

/// One row of the charge-allocation table: the case, the contract's allocation
/// method, the percentages it names in basis points, and the
/// `accountId`/`chargePercentage` pairs the answer must report.
type ChargeAllocationCase = (
    &'static str,
    &'static str,
    Vec<(&'static str, i64)>,
    Vec<(&'static str, &'static str)>,
);

/// Each charged payer account is reported with the share of the total Support
/// charge the contract pre-configures for it — zero under the proportional
/// method, which distributes the charge by each account's eligible spend
/// instead ("This is 0.0 when supportAllocationMethod = Proportional").
#[tokio::test]
async fn reports_each_charged_accounts_configured_percentage() {
    // (case, allocation method, the percentages the contract names in basis
    //  points, the accountId/chargePercentage pairs the answer must report).
    let cases: Vec<ChargeAllocationCase> = vec![
        (
            "proportional, which pre-configures no percentage",
            SUPPORT_ALLOCATION_PROPORTIONAL,
            vec![],
            vec![(caller_account_id(), "0.00")],
        ),
        (
            "proportional with percentages stored beside it",
            SUPPORT_ALLOCATION_PROPORTIONAL,
            vec![(MEMBER_ACCOUNT_ID, 5_000)],
            vec![(caller_account_id(), "0.00")],
        ),
        (
            "fixed percentages for the payer and a member",
            SUPPORT_ALLOCATION_FIXED_PERCENTAGE,
            vec![(caller_account_id(), 2_500), (MEMBER_ACCOUNT_ID, 7_500)],
            vec![(caller_account_id(), "0.25"), (MEMBER_ACCOUNT_ID, "0.75")],
        ),
        (
            "a fixed percentage finer than a whole percent",
            SUPPORT_ALLOCATION_FIXED_PERCENTAGE,
            vec![(caller_account_id(), 1_250)],
            vec![(caller_account_id(), "0.1250")],
        ),
        (
            "the whole charge on one account",
            SUPPORT_ALLOCATION_FIXED_PERCENTAGE,
            vec![(caller_account_id(), 10_000)],
            vec![(caller_account_id(), "1.00")],
        ),
        (
            "fixed, with no percentage configured for anyone",
            SUPPORT_ALLOCATION_FIXED_PERCENTAGE,
            vec![],
            vec![(caller_account_id(), "0.00")],
        ),
    ];

    for (case, method, configured, expected) in cases {
        let (router, engine) = emulator();
        add_family_account(&engine, MEMBER_ACCOUNT_ID);
        with_contract(&engine, |contract| {
            contract.allocation_method = method.to_string();
            for (account_id, rate_basis_points) in configured {
                contract
                    .charge_allocation_basis_points
                    .insert(account_id.to_string(), rate_basis_points);
            }
        });

        let (status, body) = details(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["supportAllocationMethod"].as_str(),
            Some(method),
            "case {case:?}: expected the stored allocation method, got {body}"
        );
        let reported: Vec<(String, String)> = body["chargedPayerAccountIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|account| {
                (
                    account["accountId"].as_str().unwrap_or_default().to_string(),
                    account["chargePercentage"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let mut expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(account_id, percentage)| (account_id.to_string(), percentage.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected each charged account's configured share, got {body}"
        );
    }
}

/// A month's proration instant and additional charges belong to that month
/// alone: the month they were recorded for reports them, and a month nothing
/// was recorded for reports none of them at all.
#[tokio::test]
async fn reports_the_months_own_proration_and_additional_charges() {
    let (router, engine) = emulator();
    with_contract(&engine, |contract| {
        assert!(
            contract.set_support_prorate_start_date(PAST_MONTH, instant(MID_PAST_MONTH_SECS)),
            "case \"the month's detail\": the proration instant must be stored"
        );
        assert!(
            contract.record_additional_support_charge(
                PAST_MONTH,
                EnterpriseSupportAdditionalCharge {
                    description: "Premium onboarding".to_string(),
                    amount_minor: Some(1_250 * CENTS),
                    charge_type: Some("One_Time".to_string()),
                },
            ),
            "case \"the month's detail\": the additional charge must be stored"
        );
        assert!(
            contract.record_additional_support_charge(
                PAST_MONTH,
                EnterpriseSupportAdditionalCharge {
                    description: "Contract adjustment".to_string(),
                    amount_minor: None,
                    charge_type: None,
                },
            ),
            "case \"the month's detail\": the second additional charge must be stored"
        );
        assert!(
            contract.record_additional_support_eligible_usage_spend(
                PAST_MONTH,
                EnterpriseSupportAdditionalCharge {
                    description: "Marketplace usage".to_string(),
                    amount_minor: Some(400 * CENTS),
                    charge_type: Some("Usage".to_string()),
                },
            ),
            "case \"the month's detail\": the additional eligible spend must be stored"
        );
    });

    let (status, body) = details(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"the month's detail\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["supportProrateStartDate"].as_f64(),
        Some(MID_PAST_MONTH_SECS as f64),
        "case \"the month's detail\": expected the stored proration instant, got {body}"
    );
    let charges = body["additionalSupportCharge"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        charges.len(),
        2,
        "case \"the month's detail\": expected both additional charges, got {body}"
    );
    assert_eq!(
        (
            charges[0]["description"].as_str(),
            charges[0]["amount"].as_str(),
            charges[0]["chargeType"].as_str(),
        ),
        (
            Some("Premium onboarding"),
            Some("1250.00"),
            Some("One_Time")
        ),
        "case \"the month's detail\": expected the stored charge reported, got {body}"
    );
    assert_eq!(
        charges[1]["description"].as_str(),
        Some("Contract adjustment"),
        "case \"the month's detail\": expected the second charge reported, got {body}"
    );
    assert!(
        charges[1]["amount"].is_null() && charges[1]["chargeType"].is_null(),
        "case \"the month's detail\": expected a charge with no amount to report none, got {body}"
    );
    let spend = body["additionalSupportEligibleUsageSpend"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        spend.len(),
        1,
        "case \"the month's detail\": expected the additional eligible spend, got {body}"
    );
    assert_eq!(
        (
            spend[0]["description"].as_str(),
            spend[0]["amount"].as_str()
        ),
        (Some("Marketplace usage"), Some("400.00")),
        "case \"the month's detail\": expected the stored eligible spend reported, got {body}"
    );

    // The month before carries none of it.
    let (status, body) = details(&router, EARLIER_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"a month with no detail\": expected HTTP 200, got {status} {body}"
    );
    assert!(
        body["supportProrateStartDate"].is_null()
            && body["additionalSupportCharge"].is_null()
            && body["additionalSupportEligibleUsageSpend"].is_null(),
        "case \"a month with no detail\": expected no proration or additional charges, got {body}"
    );
}

/// A month holds at most the modeled `AdditionalChargeList` maximum of
/// additional charges, so the answer cannot exceed the modeled list bound.
#[tokio::test]
async fn holds_no_more_additional_charges_than_the_modeled_list_bound() {
    let (router, engine) = emulator();
    with_contract(&engine, |contract| {
        for index in 0..=MAX_ADDITIONAL_SUPPORT_CHARGES {
            let stored = contract.record_additional_support_charge(
                PAST_MONTH,
                EnterpriseSupportAdditionalCharge {
                    description: format!("Additional charge {index}"),
                    amount_minor: Some(CENTS),
                    charge_type: None,
                },
            );
            assert_eq!(
                stored,
                index < MAX_ADDITIONAL_SUPPORT_CHARGES,
                "case \"charge {index}\": expected a write past the bound refused"
            );
        }
    });

    let (status, body) = details(&router, PAST_MONTH).await;
    assert_eq!(
        status, 200,
        "case \"a full month\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body["additionalSupportCharge"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        MAX_ADDITIONAL_SUPPORT_CHARGES,
        "case \"a full month\": expected the modeled list bound reported, got {body}"
    );
}

#[tokio::test]
async fn refuses_a_month_that_is_not_in_the_past() {
    let (router, _engine) = emulator();

    // The modeled `EnterpriseSupportBillingMonth` is "a month in the past", so
    // the current month — whose billing period has not closed — and every later
    // one are refused rather than answered.
    for offset in [0_i32, 1, 2, 13] {
        let month = month_from_now(offset);
        let case = format!("{offset} months from now");
        let (status, body) = details(&router, &month).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {month}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"].as_str(),
            Some("billingMonth"),
            "case {case:?}: expected the refusal to name billingMonth, got {body}"
        );
    }

    // The month before this one has closed, so it is answered.
    let (status, body) = details(&router, &month_from_now(-1)).await;
    assert_eq!(
        status, 200,
        "case \"last month\": expected HTTP 200, got {status} {body}"
    );
}

#[tokio::test]
async fn refuses_a_caller_that_is_not_the_payer_account_of_the_profile() {
    let (router, engine) = emulator();
    {
        // The family is paid for by another account, so the caller pays for
        // nobody and the profile's contract is not its to read.
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        state.billing_family_management_account_id = MEMBER_ACCOUNT_ID.to_string();
    }

    let (status, body) = details(&router, PAST_MONTH).await;
    assert_eq!(
        status, 403,
        "case \"not the payer account\": expected HTTP 403, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case \"not the payer account\": expected an AccessDeniedException, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("GetEnterpriseSupportContractDetails")),
        "case \"not the payer account\": expected the refusal to name the action, got {body}"
    );
}

#[tokio::test]
async fn reports_without_writing() {
    let (router, engine) = emulator();
    add_family_account(&engine, MEMBER_ACCOUNT_ID);
    with_contract(&engine, |contract| {
        contract.allocation_method = SUPPORT_ALLOCATION_FIXED_PERCENTAGE.to_string();
        contract
            .charge_allocation_basis_points
            .insert(MEMBER_ACCOUNT_ID.to_string(), 10_000);
    });
    let before = stored_contract(&engine);

    let (first_status, first) = details(&router, PAST_MONTH).await;
    let (second_status, second) = details(&router, PAST_MONTH).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"two reads\": expected both reads answered, got {first} {second}"
    );
    assert_eq!(
        first, second,
        "case \"two reads\": expected a second read to see what the first did, got {first} {second}"
    );
    assert_eq!(
        stored_contract(&engine),
        before,
        "case \"two reads\": expected a read to persist nothing"
    );
}
