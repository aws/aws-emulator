// SPDX-License-Identifier: CC0-1.0

//! `ListEnterpriseSupportLinkedAccountCharges`, end to end over the awsJson1_0
//! wire.
//!
//! Every assertion here is about the RULE the operation follows: for a billing
//! month in the past it reports, per account of the caller's consolidated
//! billing family that was subscribed to Enterprise Support that month, the
//! Support-eligible spend persisted in
//! `ComAmazonawsBillingAwsbillingState::support_eligible_spend` and the parts
//! of it that are Reserved Instance, Savings Plan and per-service spend;
//! counts the seconds of the month's billing period the account was subscribed
//! for from the intervals in
//! `ComAmazonawsBillingAwsbillingState::enterprise_support_linked_accounts`;
//! and prorates that spend by the share of the period those seconds are. It
//! writes nothing.
//!
//! The emulator runs no billing pipeline and records no subscription, so both
//! stores are empty on a fresh emulator: like the other Support suites, the
//! tables below put the records in state directly (the same shared engine the
//! router hands every handler) and then assert what the API reports for them.
//! The rows differ along everything the rule generalises over: the billing
//! month and so the length of its period (a 28-, 29-, 30- and 31-day month),
//! which of the three kinds of spend an account's bill is made of, the Support
//! plan it was subscribed to, whether the account is in the billing family at
//! all, where its subscription periods fall relative to the month, how many
//! there are, and the page size the report is read with.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    billing_period, caller_account_id, EnterpriseSupportLinkedAccount, SupportEligibleSpend,
    SupportServiceSpend, SupportTimePeriod, ACCOUNT_TYPE_LINKED, ACCOUNT_TYPE_PAYER,
    SPEND_ITEM_TYPE_USAGE, SUPPORT_PLAN_BUSINESS, SUPPORT_PLAN_ENTERPRISE,
};
use service_com_amazonaws_billing_awsbilling::{ComAmazonawsBillingAwsbillingState, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// Further 12-digit accounts, added to the consolidated billing family so the
/// payer account's Support profile covers their bills. They sort above the
/// caller's own account id, so the ascending walk of a month's accounts starts
/// with the caller and then visits these in this order.
const MEMBER_ACCOUNT_ID: &str = "111111111111";
const OTHER_MEMBER_ACCOUNT_ID: &str = "222222222222";
const THIRD_MEMBER_ACCOUNT_ID: &str = "333333333333";

/// A 12-digit account that is NOT in the consolidated billing family, so no
/// bill of its own belongs to the payer account's Support profile.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// A billing month far enough in the past that the clock cannot move it: March
/// 2024, a 31-day month.
const PAST_MONTH: &str = "2024-03";

/// The first instant of [`PAST_MONTH`], as seconds past the epoch
/// (2024-03-01T00:00:00Z), and how many days the month has — the two numbers
/// every subscription window below is placed with.
const PAST_MONTH_START_SECS: u64 = 1_709_251_200;
const PAST_MONTH_DAYS: i64 = 31;

/// One day, in seconds: the period members are UTC calendar instants, so a day
/// of a billing period is exactly this long.
const DAY_SECS: i64 = 86_400;

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

/// Ask for one billing month's linked-account charges, with whatever request
/// members the case needs.
async fn charges(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(
        router,
        "ListEnterpriseSupportLinkedAccountCharges",
        request,
    )
    .await
}

/// Ask for one billing month's whole linked-account report.
async fn charges_for(router: &MultiServiceRouter, billing_month: &str) -> (u16, Value) {
    charges(router, &json!({ "billingMonth": billing_month })).await
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

/// A whole number of currency units, in the minor units the store holds.
fn units(whole: i64) -> i64 {
    whole * CENTS
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

/// The first instant of the `day`th day of [`PAST_MONTH`]; `PAST_MONTH_DAYS + 1`
/// is the first instant of the month after it.
fn day_of_past_month(day: i64) -> SystemTime {
    let offset = u64::try_from((day - 1) * DAY_SECS).expect("a day inside the month");
    SystemTime::UNIX_EPOCH + Duration::from_secs(PAST_MONTH_START_SECS + offset)
}

/// An instant `days` days before the first instant of [`PAST_MONTH`].
fn days_before_past_month(days: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(PAST_MONTH_START_SECS - days * u64::try_from(DAY_SECS).unwrap())
}

/// The instant the `day`th day of [`PAST_MONTH`] carries on the wire, as whole
/// seconds past the epoch — the form the modeled `Timestamp` members take in
/// awsJson.
fn wire_instant_of_past_month(day: i64) -> i64 {
    i64::try_from(PAST_MONTH_START_SECS).expect("the month start fits") + (day - 1) * DAY_SECS
}

/// A wire `Timestamp` as whole seconds past the epoch, whether it is carried as
/// an integer or a fractional number of seconds.
fn wire_secs(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|secs| secs.round() as i64))
        .unwrap_or_default()
}

/// One account-month of Support-eligible spend to persist: the Support plan the
/// account was subscribed to and the three kinds of spend on its bill, in whole
/// currency units.
struct Seed {
    account_id: &'static str,
    billing_month: &'static str,
    support_plan_name: &'static str,
    usage: i64,
    reserved_instance: i64,
    savings_plan: i64,
}

impl Seed {
    /// An account-month of usage spend inside the Enterprise Support profile —
    /// the shape most rows need.
    fn enterprise(account_id: &'static str, billing_month: &'static str, usage: i64) -> Self {
        Self {
            account_id,
            billing_month,
            support_plan_name: SUPPORT_PLAN_ENTERPRISE,
            usage,
            reserved_instance: 0,
            savings_plan: 0,
        }
    }

    fn record(&self) -> SupportEligibleSpend {
        SupportEligibleSpend {
            account_id: self.account_id.to_string(),
            billing_month: self.billing_month.to_string(),
            support_plan_name: self.support_plan_name.to_string(),
            currency_code: "USD".to_string(),
            by_service: if self.usage == 0 {
                Vec::new()
            } else {
                vec![SupportServiceSpend {
                    contributing_service: "AmazonEC2".to_string(),
                    item_type: SPEND_ITEM_TYPE_USAGE.to_string(),
                    description: None,
                    charge_amount_minor: units(self.usage),
                }]
            },
            discount: None,
            reserved_instance_spend_minor: units(self.reserved_instance),
            savings_plan_spend_minor: units(self.savings_plan),
            is_estimated_bill: false,
        }
    }
}

/// Persist every seeded record, and put every account it names (bar the one
/// deliberately left outside) in the consolidated billing family, so the payer
/// account's Support profile covers their bills.
fn persist(engine: &Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for seed in seeds {
        if seed.account_id != OUTSIDE_ACCOUNT_ID {
            state
                .consolidated_billing_family
                .insert(seed.account_id.to_string(), "Linked Account".to_string());
        }
        assert!(
            state.record_support_eligible_spend(seed.record()),
            "case {:?}: the seeded spend must fit in the store",
            (seed.billing_month, seed.account_id)
        );
    }
}

/// Record when `account_id` was subscribed to Enterprise Support (and linked to
/// the payer account over the same periods, which is what a subscription of a
/// linked account implies).
fn persist_subscription(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    periods: Vec<SupportTimePeriod>,
) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    assert!(
        state.record_enterprise_support_linked_account(
            EnterpriseSupportLinkedAccount::new(account_id)
                .linked_for(periods.clone())
                .subscribed_for(periods)
        ),
        "case {account_id:?}: the seeded subscription must fit in the store"
    );
}

/// How many records each store holds, so a read can be shown not to write.
fn resident(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    (
        state.support_eligible_spend.len(),
        state.enterprise_support_linked_accounts.len(),
    )
}

/// The `linkedAccount` entries of a response.
fn entries(body: &Value) -> Vec<Value> {
    body["linkedAccount"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `accountId` of every entry of a response, in the order they are reported.
fn reported_accounts(body: &Value) -> Vec<String> {
    entries(body)
        .iter()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A billing month, how long its UTC billing period is in days, and the account
/// spend to seed it with.
///
/// The lengths are the civil calendar's: February 2023 has 28 days, February
/// 2024 (a leap year) 29, April 30 and December 31 — so the `totalSeconds` a
/// response reports is the month's own length and never a fixed one.
#[tokio::test]
async fn reports_the_billing_periods_own_length_for_every_month() {
    let cases: [(&str, &'static str, i64); 4] = [
        ("a 28-day February", "2023-02", 28),
        ("a 29-day leap February", "2024-02", 29),
        ("a 30-day April", "2024-04", 30),
        ("a 31-day December", "2023-12", 31),
    ];

    for (case, month, days) in cases {
        let (router, engine) = emulator();
        persist(&engine, &[Seed::enterprise(caller_account_id(), month, 100)]);

        let (status, body) = charges_for(&router, month).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let reported = entries(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the month's one account reported, got {body}"
        );
        assert_eq!(
            reported[0]["totalSeconds"].as_i64(),
            Some(days * DAY_SECS),
            "case {case:?}: expected the month's own length in seconds, got {body}"
        );
        // Nothing records a subscription interval for the account, so its bill
        // for the month under the Enterprise plan makes the whole period
        // billable.
        assert_eq!(
            reported[0]["billableSeconds"].as_i64(),
            Some(days * DAY_SECS),
            "case {case:?}: expected the whole period billable, got {body}"
        );
        assert_eq!(
            reported[0]["proratedTotalSupportEligibleSpend"].as_str(),
            Some("100.00"),
            "case {case:?}: expected the whole spend prorated, got {body}"
        );
    }
}

/// An entry reports the account's whole Support-eligible spend — its usage,
/// Reserved Instance and Savings Plan spend — beside each part of it.
#[tokio::test]
async fn reports_each_kind_of_support_eligible_spend() {
    type SpendCase = (&'static str, i64, i64, i64, &'static str);
    let cases: Vec<SpendCase> = vec![
        ("usage only", 400, 0, 0, "400.00"),
        ("Reserved Instance fees only", 0, 250, 0, "250.00"),
        ("Savings Plan commitment only", 0, 0, 125, "125.00"),
        ("all three kinds of spend", 400, 250, 125, "775.00"),
        ("an account with no spend at all", 0, 0, 0, "0.00"),
    ];

    for (case, usage, reserved_instance, savings_plan, total) in cases {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed {
                account_id: caller_account_id(),
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_ENTERPRISE,
                usage,
                reserved_instance,
                savings_plan,
            }],
        );

        let (status, body) = charges_for(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let entry = entries(&body).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            entry["totalSupportEligibleSpend"].as_str(),
            Some(total),
            "case {case:?}: expected the account's whole spend, got {entry}"
        );
        assert_eq!(
            entry["totalSupportEligibleReservedInstanceSpend"].as_str(),
            Some(format!("{reserved_instance}.00").as_str()),
            "case {case:?}: expected the Reserved Instance part, got {entry}"
        );
        assert_eq!(
            entry["totalSupportEligibleSavingsPlanSpend"].as_str(),
            Some(format!("{savings_plan}.00").as_str()),
            "case {case:?}: expected the Savings Plan part, got {entry}"
        );
        let by_service = entry["supportEligibleSpendByService"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if usage == 0 {
            assert!(
                by_service.is_empty(),
                "case {case:?}: expected no service line for an account with no usage, \
                 got {entry}"
            );
        } else {
            assert_eq!(
                by_service.len(),
                1,
                "case {case:?}: expected the bill's one service line, got {entry}"
            );
            assert_eq!(
                by_service[0]["serviceCode"].as_str(),
                Some("AmazonEC2"),
                "case {case:?}: expected the stored contributing service, got {entry}"
            );
            assert_eq!(
                by_service[0]["totalSupportEligibleSpend"].as_str(),
                Some(format!("{usage}.00").as_str()),
                "case {case:?}: expected the line's own spend, got {entry}"
            );
        }
        // Every entry names the payer account the profile's data belongs to,
        // and the caller is that account's own entry.
        assert_eq!(
            entry["payerAccountId"].as_str(),
            Some(caller_account_id()),
            "case {case:?}: expected the profile's payer account, got {entry}"
        );
        assert_eq!(
            entry["accountType"].as_str(),
            Some(ACCOUNT_TYPE_PAYER),
            "case {case:?}: expected the payer account's type, got {entry}"
        );
    }
}

/// `billableSeconds` is the part of the billing period the account's recorded
/// subscription periods cover, `subscriptionTimePeriods` is exactly those parts,
/// and `proratedTotalSupportEligibleSpend` is the account's spend in that share
/// of the period.
///
/// Every row seeds the same 1,000.00 of spend over the same 31-day month, so
/// only the subscription windows differ: a whole month, part of one, one that
/// has not ended, several, one that misses the month entirely and one that
/// spans past both ends of it.
#[tokio::test]
async fn prorates_the_spend_by_the_recorded_subscription_periods() {
    struct Case {
        case: &'static str,
        periods: Vec<SupportTimePeriod>,
        billable_days: i64,
        prorated: &'static str,
        reported_periods: usize,
    }
    let cases = vec![
        Case {
            case: "subscribed for the whole month",
            periods: vec![SupportTimePeriod::between(
                day_of_past_month(1),
                day_of_past_month(PAST_MONTH_DAYS + 1),
            )],
            billable_days: 31,
            prorated: "1000.00",
            reported_periods: 1,
        },
        Case {
            case: "subscribed for the first ten days",
            periods: vec![SupportTimePeriod::between(
                day_of_past_month(1),
                day_of_past_month(11),
            )],
            billable_days: 10,
            // 10/31 of 1000.00, rounded to the cent.
            prorated: "322.58",
            reported_periods: 1,
        },
        Case {
            case: "subscribed from mid-month on and never unsubscribed",
            periods: vec![SupportTimePeriod::from(day_of_past_month(16))],
            billable_days: 16,
            // 16/31 of 1000.00, rounded to the cent.
            prorated: "516.13",
            reported_periods: 1,
        },
        Case {
            case: "two separate subscription periods",
            periods: vec![
                SupportTimePeriod::between(day_of_past_month(1), day_of_past_month(6)),
                SupportTimePeriod::between(day_of_past_month(21), day_of_past_month(26)),
            ],
            billable_days: 10,
            prorated: "322.58",
            reported_periods: 2,
        },
        Case {
            case: "a subscription that ended before the month began",
            periods: vec![SupportTimePeriod::between(
                days_before_past_month(20),
                day_of_past_month(1),
            )],
            billable_days: 0,
            prorated: "0.00",
            reported_periods: 0,
        },
        Case {
            case: "a subscription spanning past both ends of the month",
            periods: vec![SupportTimePeriod::between(
                days_before_past_month(10),
                day_of_past_month(PAST_MONTH_DAYS + 15),
            )],
            billable_days: 31,
            prorated: "1000.00",
            reported_periods: 1,
        },
    ];

    for Case {
        case,
        periods,
        billable_days,
        prorated,
        reported_periods,
    } in cases
    {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 1_000)],
        );
        persist_subscription(&engine, MEMBER_ACCOUNT_ID, periods);

        let (status, body) = charges_for(&router, PAST_MONTH).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let entry = entries(&body).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            entry["totalSeconds"].as_i64(),
            Some(PAST_MONTH_DAYS * DAY_SECS),
            "case {case:?}: expected the month's own length, got {entry}"
        );
        assert_eq!(
            entry["billableSeconds"].as_i64(),
            Some(billable_days * DAY_SECS),
            "case {case:?}: expected the subscribed part of the period, got {entry}"
        );
        assert_eq!(
            entry["totalSupportEligibleSpend"].as_str(),
            Some("1000.00"),
            "case {case:?}: expected the account's whole spend, got {entry}"
        );
        assert_eq!(
            entry["proratedTotalSupportEligibleSpend"].as_str(),
            Some(prorated),
            "case {case:?}: expected the subscribed share of the spend, got {entry}"
        );
        // A linked account's entry reports it as linked, not as the payer.
        assert_eq!(
            entry["accountType"].as_str(),
            Some(ACCOUNT_TYPE_LINKED),
            "case {case:?}: expected a linked account's type, got {entry}"
        );
        for member in ["subscriptionTimePeriods", "linkedTimePeriods"] {
            let reported = entry[member].as_array().cloned().unwrap_or_default();
            assert_eq!(
                reported.len(),
                reported_periods,
                "case {case:?}: expected {reported_periods} {member} reported, got {entry}"
            );
            let covered: i64 = reported
                .iter()
                .map(|period| wire_secs(&period["endDate"]) - wire_secs(&period["beginDate"]))
                .sum();
            assert_eq!(
                covered,
                billable_days * DAY_SECS,
                "case {case:?}: expected {member} to cover the billable seconds, got {entry}"
            );
        }
    }
}

/// The reported periods are the parts of the recorded ones that fall inside the
/// requested billing period, so a subscription that began before the month is
/// reported from the month's first instant.
#[tokio::test]
async fn clips_a_reported_period_to_the_requested_billing_period() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 500)],
    );
    persist_subscription(
        &engine,
        MEMBER_ACCOUNT_ID,
        vec![SupportTimePeriod::between(
            days_before_past_month(45),
            day_of_past_month(11),
        )],
    );

    let (status, body) = charges_for(&router, PAST_MONTH).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let entry = entries(&body).first().cloned().unwrap_or(Value::Null);
    let periods = entry["subscriptionTimePeriods"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        periods.len(),
        1,
        "expected the one overlapping period reported, got {entry}"
    );
    assert_eq!(
        wire_secs(&periods[0]["beginDate"]),
        wire_instant_of_past_month(1),
        "expected the period to begin with the billing period, got {entry}"
    );
    assert_eq!(
        wire_secs(&periods[0]["endDate"]),
        wire_instant_of_past_month(11),
        "expected the period to end where the subscription did, got {entry}"
    );
}

/// The report covers exactly the accounts of the caller's consolidated billing
/// family that were subscribed to Enterprise Support in the month: an account
/// on another Support plan is billed under that plan, and an account outside
/// the family belongs to nobody's profile.
#[tokio::test]
async fn reports_only_the_enterprise_profiles_accounts() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed::enterprise(caller_account_id(), PAST_MONTH, 100),
            Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 200),
            Seed {
                account_id: OTHER_MEMBER_ACCOUNT_ID,
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_BUSINESS,
                usage: 300,
                reserved_instance: 0,
                savings_plan: 0,
            },
            Seed::enterprise(OUTSIDE_ACCOUNT_ID, PAST_MONTH, 400),
            Seed::enterprise(THIRD_MEMBER_ACCOUNT_ID, "2024-02", 500),
        ],
    );

    let (status, body) = charges_for(&router, PAST_MONTH).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    assert_eq!(
        reported_accounts(&body),
        vec![
            caller_account_id().to_string(),
            MEMBER_ACCOUNT_ID.to_string()
        ],
        "expected only the month's Enterprise-subscribed family accounts, got {body}"
    );

    // The `accountId` filter: "the linked account ID to filter results to a
    // specific account. If you don't specify a value, the response includes
    // charges for all linked accounts."
    for (case, filter, expected) in [
        ("the payer's own account", caller_account_id(), 1),
        ("a member account", MEMBER_ACCOUNT_ID, 1),
        ("an account on another Support plan", OTHER_MEMBER_ACCOUNT_ID, 0),
        ("an account outside the family", OUTSIDE_ACCOUNT_ID, 0),
    ] {
        let (status, body) = charges(
            &router,
            &json!({ "billingMonth": PAST_MONTH, "accountId": filter }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            entries(&body).len(),
            expected,
            "case {case:?}: expected {expected} entries for the filter, got {body}"
        );
    }
}

/// The report pages by `maxResults`, resuming after the account the previous
/// page ended on, and the whole walk is every account of the month exactly
/// once, whatever the page size.
#[tokio::test]
async fn pages_the_accounts_by_max_results() {
    let accounts = [
        caller_account_id(),
        MEMBER_ACCOUNT_ID,
        OTHER_MEMBER_ACCOUNT_ID,
        THIRD_MEMBER_ACCOUNT_ID,
    ];
    let expected: Vec<String> = accounts.iter().map(|id| (*id).to_string()).collect();

    for page_size in [1_i64, 2, 3, 4, 100] {
        let (router, engine) = emulator();
        persist(
            &engine,
            &[
                Seed::enterprise(caller_account_id(), PAST_MONTH, 100),
                Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 200),
                Seed::enterprise(OTHER_MEMBER_ACCOUNT_ID, PAST_MONTH, 300),
                Seed::enterprise(THIRD_MEMBER_ACCOUNT_ID, PAST_MONTH, 400),
            ],
        );

        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "billingMonth": PAST_MONTH, "maxResults": page_size });
            if let Some(token) = next_token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = charges(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: expected HTTP 200, got {status} {body}"
            );
            let page = reported_accounts(&body);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap(),
                "case {page_size:?}: expected at most {page_size} entries a page, got {body}"
            );
            walked.extend(page);
            next_token = body["nextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= accounts.len() + 1,
                "case {page_size:?}: expected the walk to end, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {page_size:?}: expected every account exactly once, in order, got {walked:?}"
        );
    }
}

/// A `nextToken` this service did not issue is refused as a `ValidationException`
/// (HTTP 400), never answered with a page.
#[tokio::test]
async fn refuses_a_page_token_it_did_not_issue() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 100)],
    );

    for case in ["not-a-token", "AAAA", "ZZZZ=="] {
        let (status, body) = charges(
            &router,
            &json!({ "billingMonth": PAST_MONTH, "nextToken": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }
}

/// The `EnterpriseSupportBillingMonth` doc: "This must be a month in the past."
/// The current month's billing period has not closed and a later one has not
/// begun, so neither is answered.
#[tokio::test]
async fn refuses_a_month_that_is_not_in_the_past() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 100)],
    );

    for (case, month) in [
        ("the current month", month_from_now(0)),
        ("next month", month_from_now(1)),
        ("a year from now", month_from_now(12)),
    ] {
        let (status, body) = charges_for(&router, &month).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }

    for (case, month) in [
        ("last month", month_from_now(-1)),
        ("a year ago", month_from_now(-12)),
    ] {
        let (status, body) = charges_for(&router, &month).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a past month answered, got {status} {body}"
        );
        assert!(
            entries(&body).is_empty(),
            "case {case:?}: expected no entries for a month with no bills, got {body}"
        );
    }
}

/// Only the payer account of the Enterprise Support profile may read its data:
/// `LinkedAccountCharge$payerAccountId` is "the payer account ID that is
/// authorized to view Enterprise Support data for all accounts in its Support
/// profile", so a caller that pays for nobody is refused rather than answered
/// with another profile's accounts.
#[tokio::test]
async fn refuses_a_caller_that_is_not_the_profiles_payer_account() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[Seed::enterprise(caller_account_id(), PAST_MONTH, 100)],
    );
    {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        state.billing_family_management_account_id = MEMBER_ACCOUNT_ID.to_string();
    }

    let (status, body) = charges_for(&router, PAST_MONTH).await;
    assert_eq!(status, 403, "expected HTTP 403, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "expected an AccessDeniedException, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ListEnterpriseSupportLinkedAccountCharges"),
        "expected the refused action named, got {body}"
    );
}

/// A reporting operation: reading a month leaves both stores exactly as they
/// were, so a later caller sees the same answer.
#[tokio::test]
async fn reports_without_writing() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed::enterprise(caller_account_id(), PAST_MONTH, 100),
            Seed::enterprise(MEMBER_ACCOUNT_ID, PAST_MONTH, 200),
        ],
    );
    persist_subscription(
        &engine,
        MEMBER_ACCOUNT_ID,
        vec![SupportTimePeriod::from(day_of_past_month(16))],
    );
    let before = resident(&engine);

    let (first_status, first) = charges_for(&router, PAST_MONTH).await;
    let (second_status, second) = charges_for(&router, PAST_MONTH).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "expected both reads answered, got {first} {second}"
    );
    assert_eq!(
        first, second,
        "expected a second read to report the same month, got {first} then {second}"
    );
    assert_eq!(
        resident(&engine),
        before,
        "expected a read to write nothing, got {:?} records",
        resident(&engine)
    );
}

/// The report and `GetEnterpriseSupportChargeSummary` are readings of ONE
/// store: the entries' `totalSupportEligibleSpend` add up to the summary's
/// `totalSupportEligibleSpend` for the same month.
#[tokio::test]
async fn agrees_with_the_charge_summary_for_the_same_month() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            Seed::enterprise(caller_account_id(), PAST_MONTH, 100),
            Seed {
                account_id: MEMBER_ACCOUNT_ID,
                billing_month: PAST_MONTH,
                support_plan_name: SUPPORT_PLAN_ENTERPRISE,
                usage: 200,
                reserved_instance: 50,
                savings_plan: 25,
            },
            Seed::enterprise(OTHER_MEMBER_ACCOUNT_ID, PAST_MONTH, 300),
        ],
    );

    let (status, body) = charges_for(&router, PAST_MONTH).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let listed_minor: i64 = entries(&body)
        .iter()
        .map(|entry| {
            let reported = entry["totalSupportEligibleSpend"]
                .as_str()
                .unwrap_or_default()
                .replace('.', "");
            reported.parse::<i64>().unwrap_or_default()
        })
        .sum();

    let (summary_status, summary) = call(
        &router,
        "GetEnterpriseSupportChargeSummary",
        &json!({ "billingMonth": PAST_MONTH }),
    )
    .await;
    assert_eq!(
        summary_status, 200,
        "expected the summary answered, got {summary_status} {summary}"
    );
    let summary_minor: i64 = summary["totalSupportEligibleSpend"]
        .as_str()
        .unwrap_or_default()
        .replace('.', "")
        .parse()
        .unwrap_or_default();
    assert_eq!(
        listed_minor, summary_minor,
        "expected the entries to add up to the summary's spend, got {body} and {summary}"
    );
}
