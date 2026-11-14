// SPDX-License-Identifier: CC0-1.0

//! `ListBusinessSupportSubscriptionHistory`, end to end over the awsJson1_0
//! wire.
//!
//! Every assertion here is about the RULE the operation follows: a Support
//! subscription contract is the unbroken run of billing months whose persisted
//! bills record one account on one Support plan the modeled `planName`
//! documents, its period is the UTC calendar period of that run, and the
//! operation reports every such contract of the caller's consolidated billing
//! family whose period OVERLAPS the period the request asks about — the billing
//! month it names, or the month the clock is in when it names none, with each
//! end of that period replaced by `startDate` or `endDate` when the request
//! carries one — ascending by account and then by contract start, narrowed by
//! the optional `accountId` and paged by `maxResults`/`nextToken`. It writes
//! nothing.
//!
//! The emulator runs no billing pipeline, so nothing accrues bills and the
//! store is empty on a fresh emulator: like the Support charge suites, the
//! tables below put the bills in state directly (the same shared engine the
//! router hands every handler) and then assert what the API reports for them.
//! The rows differ along everything the rule generalises over: the Support
//! plan, the number of months a run covers, where a run is broken (a plan
//! change, a missing month), the account, which period the request asks about,
//! the page size and the position of the page boundary.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    billing_period, caller_account_id, SupportEligibleSpend, SupportServiceSpend,
    SPEND_ITEM_TYPE_USAGE, SUPPORT_PLAN_BASIC, SUPPORT_PLAN_BUSINESS, SUPPORT_PLAN_DEVELOPER,
    SUPPORT_PLAN_ENTERPRISE,
};
use service_com_amazonaws_billing_awsbilling::{ComAmazonawsBillingAwsbillingState, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// A second and third 12-digit account, added to the consolidated billing
/// family so the payer account's answer covers their bills.
const MEMBER_ACCOUNT_ID: &str = "123456789012";
const OTHER_MEMBER_ACCOUNT_ID: &str = "210987654321";

/// A 12-digit account that is NOT in the consolidated billing family, so no
/// bill of its own belongs to the payer account's answer.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// Billing months far enough in the past that the clock cannot move them, so
/// only the rows about the default month depend on today's date.
const JANUARY: &str = "2024-01";
const FEBRUARY: &str = "2024-02";
const MARCH: &str = "2024-03";
const APRIL: &str = "2024-04";

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

/// One `ListBusinessSupportSubscriptionHistory` call.
async fn history(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "ListBusinessSupportSubscriptionHistory", request).await
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

/// An RFC 3339 UTC instant as the epoch seconds the modeled `Timestamp` carries
/// on the awsJson wire — read from the date text, so an expected contract bound
/// is never computed by the arithmetic under test.
fn instant(rfc3339: &str) -> f64 {
    DateTime::from_str(rfc3339, Format::DateTime)
        .unwrap_or_else(|error| panic!("{rfc3339:?} is an instant: {error}"))
        .as_secs_f64()
}

/// Persist one account-month bill recording `plan`, and put the account in the
/// consolidated billing family unless it is the one deliberately left outside.
fn persist(
    engine: &Arc<engine::InMemoryEngine>,
    bills: &[(&'static str, &'static str, &'static str)],
) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for (account_id, billing_month, plan) in bills {
        if *account_id != OUTSIDE_ACCOUNT_ID {
            state
                .consolidated_billing_family
                .insert((*account_id).to_string(), "Linked Account".to_string());
        }
        assert!(
            state.record_support_eligible_spend(SupportEligibleSpend {
                account_id: (*account_id).to_string(),
                billing_month: (*billing_month).to_string(),
                support_plan_name: (*plan).to_string(),
                currency_code: "USD".to_string(),
                by_service: vec![SupportServiceSpend {
                    contributing_service: "Amazon Elastic Compute Cloud".to_string(),
                    item_type: SPEND_ITEM_TYPE_USAGE.to_string(),
                    description: None,
                    charge_amount_minor: 1_000,
                }],
                reserved_instance_spend_minor: 0,
                savings_plan_spend_minor: 0,
                is_estimated_bill: false,
                discount: None,
            }),
            "case {:?}: the seeded bill must fit in the store",
            (billing_month, account_id)
        );
    }
}

/// How many bills are resident, so a read can be shown not to write.
fn resident(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    state.support_eligible_spend.len()
}

/// Every contract a response reports, in the order it reports them, as
/// (account, plan, contract start, contract end).
fn reported(body: &Value) -> Vec<(String, String, f64, f64)> {
    body["subscriptionContracts"]
        .as_array()
        .unwrap_or_else(|| panic!("the response must carry subscriptionContracts, got {body}"))
        .iter()
        .map(|contract| {
            (
                contract["accountId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                contract["planName"].as_str().unwrap_or_default().to_string(),
                contract["contractStartDate"].as_f64().unwrap_or_default(),
                contract["contractEndDate"].as_f64().unwrap_or_default(),
            )
        })
        .collect()
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

/// One row of the derivation table: the case, the bills to persist, the request
/// to send, and the contracts the response must report in order.
type DerivationCase = (
    &'static str,
    Vec<(&'static str, &'static str, &'static str)>,
    Value,
    Vec<(&'static str, &'static str, &'static str, &'static str)>,
);

/// The contracts the persisted bills imply, and the period each request asks
/// about.
///
/// A contract is one account's unbroken run of months on one plan: a plan
/// change or a month with no bill ends a run and starts the next, and the
/// run's period is the first instant of its first month to the first instant of
/// the month after its last — the half-open reading of a period this service's
/// charge periods carry.
fn derivation_cases() -> Vec<DerivationCase> {
    let caller = caller_account_id();
    vec![
        (
            "one month on one plan",
            vec![(caller, MARCH, SUPPORT_PLAN_BUSINESS)],
            json!({ "billingMonth": MARCH }),
            vec![(
                caller,
                SUPPORT_PLAN_BUSINESS,
                "2024-03-01T00:00:00Z",
                "2024-04-01T00:00:00Z",
            )],
        ),
        (
            "three consecutive months on one plan are one contract",
            vec![
                (caller, JANUARY, SUPPORT_PLAN_BUSINESS),
                (caller, FEBRUARY, SUPPORT_PLAN_BUSINESS),
                (caller, MARCH, SUPPORT_PLAN_BUSINESS),
            ],
            json!({ "billingMonth": FEBRUARY }),
            vec![(
                caller,
                SUPPORT_PLAN_BUSINESS,
                "2024-01-01T00:00:00Z",
                "2024-04-01T00:00:00Z",
            )],
        ),
        (
            "a plan change splits the run, and the month after it reports the new plan",
            vec![
                (caller, JANUARY, SUPPORT_PLAN_BUSINESS),
                (caller, FEBRUARY, SUPPORT_PLAN_BUSINESS),
                (caller, MARCH, SUPPORT_PLAN_DEVELOPER),
            ],
            json!({ "billingMonth": MARCH }),
            vec![(
                caller,
                SUPPORT_PLAN_DEVELOPER,
                "2024-03-01T00:00:00Z",
                "2024-04-01T00:00:00Z",
            )],
        ),
        (
            "a plan change splits the run, and the months before it report the old plan",
            vec![
                (caller, JANUARY, SUPPORT_PLAN_BUSINESS),
                (caller, FEBRUARY, SUPPORT_PLAN_BUSINESS),
                (caller, MARCH, SUPPORT_PLAN_DEVELOPER),
            ],
            json!({ "billingMonth": JANUARY }),
            vec![(
                caller,
                SUPPORT_PLAN_BUSINESS,
                "2024-01-01T00:00:00Z",
                "2024-03-01T00:00:00Z",
            )],
        ),
        (
            "a month with no bill breaks the run in two",
            vec![
                (caller, JANUARY, SUPPORT_PLAN_BUSINESS),
                (caller, MARCH, SUPPORT_PLAN_BUSINESS),
            ],
            json!({ "startDate": instant("2024-01-01T00:00:00Z"), "billingMonth": MARCH }),
            vec![
                (
                    caller,
                    SUPPORT_PLAN_BUSINESS,
                    "2024-01-01T00:00:00Z",
                    "2024-02-01T00:00:00Z",
                ),
                (
                    caller,
                    SUPPORT_PLAN_BUSINESS,
                    "2024-03-01T00:00:00Z",
                    "2024-04-01T00:00:00Z",
                ),
            ],
        ),
        (
            "a plan changed and changed back is three contracts",
            vec![
                (caller, JANUARY, SUPPORT_PLAN_BUSINESS),
                (caller, FEBRUARY, SUPPORT_PLAN_DEVELOPER),
                (caller, MARCH, SUPPORT_PLAN_BUSINESS),
            ],
            json!({ "startDate": instant("2024-01-01T00:00:00Z"), "billingMonth": MARCH }),
            vec![
                (
                    caller,
                    SUPPORT_PLAN_BUSINESS,
                    "2024-01-01T00:00:00Z",
                    "2024-02-01T00:00:00Z",
                ),
                (
                    caller,
                    SUPPORT_PLAN_DEVELOPER,
                    "2024-02-01T00:00:00Z",
                    "2024-03-01T00:00:00Z",
                ),
                (
                    caller,
                    SUPPORT_PLAN_BUSINESS,
                    "2024-03-01T00:00:00Z",
                    "2024-04-01T00:00:00Z",
                ),
            ],
        ),
        (
            // The third plan name the modeled `planName` documents: Basic
            // Support is a subscription like the others, free of charge.
            "the Basic Support plan is a contract too",
            vec![
                (caller, FEBRUARY, SUPPORT_PLAN_BASIC),
                (caller, MARCH, SUPPORT_PLAN_BASIC),
            ],
            json!({ "billingMonth": MARCH }),
            vec![(
                caller,
                SUPPORT_PLAN_BASIC,
                "2024-02-01T00:00:00Z",
                "2024-04-01T00:00:00Z",
            )],
        ),
        (
            // One account per bill, so the three accounts' contracts are
            // reported in ascending account order whatever order the bills
            // were written in.
            "every family account's contract, ascending by account",
            vec![
                (OTHER_MEMBER_ACCOUNT_ID, MARCH, SUPPORT_PLAN_DEVELOPER),
                (caller, MARCH, SUPPORT_PLAN_BUSINESS),
                (MEMBER_ACCOUNT_ID, MARCH, SUPPORT_PLAN_BASIC),
            ],
            json!({ "billingMonth": MARCH }),
            vec![
                (
                    caller,
                    SUPPORT_PLAN_BUSINESS,
                    "2024-03-01T00:00:00Z",
                    "2024-04-01T00:00:00Z",
                ),
                (
                    MEMBER_ACCOUNT_ID,
                    SUPPORT_PLAN_BASIC,
                    "2024-03-01T00:00:00Z",
                    "2024-04-01T00:00:00Z",
                ),
                (
                    OTHER_MEMBER_ACCOUNT_ID,
                    SUPPORT_PLAN_DEVELOPER,
                    "2024-03-01T00:00:00Z",
                    "2024-04-01T00:00:00Z",
                ),
            ],
        ),
        (
            "an account outside the consolidated billing family is nobody's history",
            vec![
                (OUTSIDE_ACCOUNT_ID, MARCH, SUPPORT_PLAN_BUSINESS),
                (caller, MARCH, SUPPORT_PLAN_BUSINESS),
            ],
            json!({ "billingMonth": MARCH }),
            vec![(
                caller,
                SUPPORT_PLAN_BUSINESS,
                "2024-03-01T00:00:00Z",
                "2024-04-01T00:00:00Z",
            )],
        ),
        (
            // `planName`'s documented values are the Business, Developer and
            // Basic Support plans; a month billed under the Enterprise Support
            // plan belongs to that profile's own contract.
            "a month on the Enterprise Support plan is not one of these contracts",
            vec![
                (caller, FEBRUARY, SUPPORT_PLAN_ENTERPRISE),
                (caller, MARCH, SUPPORT_PLAN_ENTERPRISE),
            ],
            json!({ "billingMonth": MARCH }),
            vec![],
        ),
        (
            "a month nobody was subscribed in reports no contract",
            vec![(caller, MARCH, SUPPORT_PLAN_BUSINESS)],
            json!({ "billingMonth": APRIL }),
            vec![],
        ),
    ]
}

#[tokio::test]
async fn reports_the_contracts_the_persisted_bills_record() {
    for (case, bills, request, expected) in derivation_cases() {
        let (router, engine) = emulator();
        persist(&engine, &bills);

        let (status, body) = history(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let expected: Vec<(String, String, f64, f64)> = expected
            .into_iter()
            .map(|(account, plan, start, end)| {
                (
                    account.to_string(),
                    plan.to_string(),
                    instant(start),
                    instant(end),
                )
            })
            .collect();
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page holds every contract, so expected no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn reads_nothing_into_the_store() {
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            (caller_account_id(), FEBRUARY, SUPPORT_PLAN_BUSINESS),
            (caller_account_id(), MARCH, SUPPORT_PLAN_BUSINESS),
        ],
    );
    let before = resident(&engine);

    let request = json!({ "billingMonth": MARCH });
    let (_, first) = history(&router, &request).await;
    let (_, second) = history(&router, &request).await;

    assert_eq!(
        reported(&first),
        reported(&second),
        "case \"repeated read\": a read must not change what the next caller sees, got {first} then {second}"
    );
    assert_eq!(
        resident(&engine),
        before,
        "case \"repeated read\": expected {before} bills still resident, got {}",
        resident(&engine)
    );
}

#[tokio::test]
async fn an_absent_billing_month_asks_about_the_current_month() {
    // (case, the month the bill is for, whether the contract covers the month
    // the clock is in and so must be reported).
    let cases: [(&str, i32, bool); 4] = [
        ("this month", 0, true),
        ("last month", -1, false),
        ("two months ago", -2, false),
        ("next month", 1, false),
    ];

    for (case, offset, expected) in cases {
        let (router, engine) = emulator();
        let month = month_from_now(offset);
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        {
            let mut state = state.lock();
            assert!(
                state.record_support_eligible_spend(SupportEligibleSpend {
                    account_id: caller_account_id().to_string(),
                    billing_month: month.clone(),
                    support_plan_name: SUPPORT_PLAN_BUSINESS.to_string(),
                    currency_code: "USD".to_string(),
                    by_service: Vec::new(),
                    reserved_instance_spend_minor: 0,
                    savings_plan_spend_minor: 0,
                    is_estimated_bill: true,
                    discount: None,
                }),
                "case {case:?}: the seeded bill must fit in the store"
            );
        }

        let (status, body) = history(&router, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            reported(&body).len(),
            usize::from(expected),
            "case {case:?}: expected the month the clock is in to select {expected}, got {body}"
        );
    }
}

/// One row of the requested-period table: the case, the request, and the
/// contract start dates the response must report for the bills below.
type PeriodCase = (&'static str, Value, Vec<&'static str>);

#[tokio::test]
async fn a_requested_period_selects_the_contracts_it_overlaps() {
    // One account on Business Support for January and February, then on
    // Developer Support for March: two contracts, [Jan 1, Mar 1) and
    // [Mar 1, Apr 1).
    let bills = vec![
        (caller_account_id(), JANUARY, SUPPORT_PLAN_BUSINESS),
        (caller_account_id(), FEBRUARY, SUPPORT_PLAN_BUSINESS),
        (caller_account_id(), MARCH, SUPPORT_PLAN_DEVELOPER),
    ];
    let cases: Vec<PeriodCase> = vec![
        (
            "the month a contract covers",
            json!({ "billingMonth": FEBRUARY }),
            vec!["2024-01-01T00:00:00Z"],
        ),
        (
            "the first month of a contract",
            json!({ "billingMonth": JANUARY }),
            vec!["2024-01-01T00:00:00Z"],
        ),
        (
            "the month after the last contract ends",
            json!({ "billingMonth": APRIL }),
            vec![],
        ),
        (
            // `startDate` replaces the period's start, so a period that
            // reaches back across months reports every contract it overlaps.
            "a startDate reaching back across both contracts",
            json!({ "billingMonth": MARCH, "startDate": instant("2024-01-15T00:00:00Z") }),
            vec!["2024-01-01T00:00:00Z", "2024-03-01T00:00:00Z"],
        ),
        (
            "a startDate inside the second contract",
            json!({ "billingMonth": MARCH, "startDate": instant("2024-03-15T00:00:00Z") }),
            vec!["2024-03-01T00:00:00Z"],
        ),
        (
            // The first contract's period ends at the instant March begins, and
            // a period that begins where another ends overlaps nothing of it.
            "a startDate at the instant the first contract ends",
            json!({ "billingMonth": MARCH, "startDate": instant("2024-03-01T00:00:00Z") }),
            vec!["2024-03-01T00:00:00Z"],
        ),
        (
            // `endDate` replaces the period's end, and is exclusive.
            "an endDate at the instant the first contract begins",
            json!({ "billingMonth": JANUARY, "endDate": instant("2024-01-01T00:00:00Z") }),
            vec![],
        ),
        (
            "an endDate one second into the first contract",
            json!({ "billingMonth": JANUARY, "endDate": instant("2024-01-01T00:00:01Z") }),
            vec!["2024-01-01T00:00:00Z"],
        ),
        (
            "an endDate before the requested month begins",
            json!({ "billingMonth": MARCH, "endDate": instant("2024-02-15T00:00:00Z") }),
            vec![],
        ),
        (
            "both dates, spanning both contracts",
            json!({
                "startDate": instant("2024-01-01T00:00:00Z"),
                "endDate": instant("2024-04-01T00:00:00Z"),
            }),
            vec!["2024-01-01T00:00:00Z", "2024-03-01T00:00:00Z"],
        ),
        (
            "both dates, inside the first contract alone",
            json!({
                "startDate": instant("2024-01-10T00:00:00Z"),
                "endDate": instant("2024-02-10T00:00:00Z"),
            }),
            vec!["2024-01-01T00:00:00Z"],
        ),
        (
            "both dates, in a period no contract covers",
            json!({
                "startDate": instant("2023-01-01T00:00:00Z"),
                "endDate": instant("2023-06-01T00:00:00Z"),
            }),
            vec![],
        ),
        (
            "an accountId naming an account with no bills",
            json!({ "billingMonth": FEBRUARY, "accountId": MEMBER_ACCOUNT_ID }),
            vec![],
        ),
        (
            "an accountId naming the account that has them",
            json!({ "billingMonth": FEBRUARY, "accountId": caller_account_id() }),
            vec!["2024-01-01T00:00:00Z"],
        ),
    ];

    for (case, request, expected) in cases {
        let (router, engine) = emulator();
        persist(&engine, &bills);

        let (status, body) = history(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let starts: Vec<f64> = reported(&body)
            .into_iter()
            .map(|(_, _, start, _)| start)
            .collect();
        let expected: Vec<f64> = expected.into_iter().map(instant).collect();
        assert_eq!(
            starts, expected,
            "case {case:?}: expected the contracts starting {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn pages_the_contracts_by_max_results() {
    // Three family accounts, each subscribed for the same month: three
    // contracts, ascending by account.
    let (router, engine) = emulator();
    persist(
        &engine,
        &[
            (MEMBER_ACCOUNT_ID, MARCH, SUPPORT_PLAN_BUSINESS),
            (OTHER_MEMBER_ACCOUNT_ID, MARCH, SUPPORT_PLAN_DEVELOPER),
            (caller_account_id(), MARCH, SUPPORT_PLAN_BASIC),
        ],
    );
    // The caller's own account id sorts below the two members', so ascending
    // account order is the caller's contract first.
    let every_account = vec![
        caller_account_id().to_string(),
        MEMBER_ACCOUNT_ID.to_string(),
        OTHER_MEMBER_ACCOUNT_ID.to_string(),
    ];

    // (case, page size, how many pages the walk must take)
    let cases: [(&str, i64, usize); 4] = [
        ("one at a time", 1, 3),
        ("two at a time", 2, 2),
        ("exactly the contract count", 3, 1),
        ("more than the contract count", 100, 1),
    ];

    for (case, page_size, expected_pages) in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0_usize;
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "billingMonth": MARCH, "maxResults": page_size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = history(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: expected at most {page_size} contracts a page, got {body}"
            );
            walked.extend(page.into_iter().map(|(account, _, _, _)| account));
            pages += 1;
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= every_account.len(),
                "case {case:?}: the walk must end within one page per contract, got {walked:?}"
            );
        }

        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages} ({walked:?})"
        );
        assert_eq!(
            walked, every_account,
            "case {case:?}: expected every contract exactly once, ascending by account, got {walked:?}"
        );
    }
}

/// One row of the refusal table: the case, the request, the HTTP status, the
/// error code and the message the service must answer with.
type RefusalCase = (&'static str, Value, u16, &'static str, &'static str);

#[tokio::test]
async fn refuses_a_request_it_cannot_answer() {
    let cases: Vec<RefusalCase> = vec![
        (
            "startDate after endDate",
            json!({
                "startDate": instant("2024-03-01T00:00:00Z"),
                "endDate": instant("2024-02-01T00:00:00Z"),
            }),
            400,
            "ValidationException",
            "InvalidArgument: startDate (DetailedCode=startDate must be strictly before endDate \
             (endDate is exclusive))",
        ),
        (
            "startDate at endDate, a period of no length",
            json!({
                "startDate": instant("2024-03-01T00:00:00Z"),
                "endDate": instant("2024-03-01T00:00:00Z"),
            }),
            400,
            "ValidationException",
            "InvalidArgument: startDate (DetailedCode=startDate must be strictly before endDate \
             (endDate is exclusive))",
        ),
        (
            "a nextToken this service never issued",
            json!({ "billingMonth": MARCH, "nextToken": "notatokenwemade" }),
            400,
            "ValidationException",
            "nextToken is not a token this service issued",
        ),
        (
            "a billingMonth outside the modeled pattern",
            json!({ "billingMonth": "2024-13" }),
            400,
            "ValidationException",
            "",
        ),
        (
            "a maxResults above the modeled range",
            json!({ "billingMonth": MARCH, "maxResults": 101 }),
            400,
            "ValidationException",
            "",
        ),
        (
            "an accountId outside the modeled pattern",
            json!({ "billingMonth": MARCH, "accountId": "12345" }),
            400,
            "ValidationException",
            "",
        ),
    ];

    for (case, request, status_expected, code, message) in cases {
        let (router, engine) = emulator();
        persist(&engine, &[(caller_account_id(), MARCH, SUPPORT_PLAN_BUSINESS)]);

        let (status, body) = history(&router, &request).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the {code} error code, got {body}"
        );
        if !message.is_empty() {
            assert_eq!(
                body["message"].as_str(),
                Some(message),
                "case {case:?}: expected the refusal message {message:?}, got {body}"
            );
        }
    }
}
