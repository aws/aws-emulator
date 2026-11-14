// SPDX-License-Identifier: CC0-1.0

//! `GetCredits`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: it REPORTS the
//! credits persisted in `ComAmazonawsBillingAwsbillingState::credits` — those owned
//! by the account named in `accountId`, or every account of the consolidated billing
//! family when the caller is the management account and sets `payerAccountFlag`,
//! narrowed to the credits whose own life overlaps the requested period — and it
//! changes nothing.
//! The tables span the class of inputs the rule generalises over (credit id length,
//! currency, which optional members are recorded, list sizes, every value of each
//! modeled enum, period placement, the account asked about, and the documented
//! bounds on `accountId`/`startDate`/`endDate`), not one recorded request.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{Credit, CreditAmount};
use service_com_amazonaws_billing_awsbilling::{
    ComAmazonawsBillingAwsbillingState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// which is the management account of this emulator's consolidated billing family.
const ACCOUNT_ID: &str = "000000000000";

/// A second, linked account of the same family — a 12-digit id that is not the
/// management account.
const LINKED_ACCOUNT_ID: &str = "210987654321";

/// A third 12-digit account id this emulator holds no credit for.
const UNKNOWN_ACCOUNT_ID: &str = "999999999999";

/// Seconds in a day, for placing stored and requested periods.
const DAY: u64 = 24 * 60 * 60;

/// One router over ONE shared engine — what `core/router` builds in production, so
/// every call in a test observes the same state. The engine comes back too: the
/// credits a real account holds are minted by AWS's billing systems rather than by
/// an API of this service (`RedeemCredits` is still a scaffold stub), so a test
/// seeds the store the way those systems would and then reads it back over the wire.
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

/// `days` before the current instant.
fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(days * DAY)
}

/// An instant as the epoch seconds a request carries for a modeled `Timestamp`.
fn epoch(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs() as i64
}

/// The `creditId`s a `GetCredits` response lists, in the order it listed them.
fn credit_ids(body: &Value) -> Vec<String> {
    body["credits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|credit| credit["creditId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A stored amount in `currency_code`.
fn money(currency_code: &str, currency_amount: &str) -> CreditAmount {
    CreditAmount {
        currency_code: currency_code.to_string(),
        currency_amount: currency_amount.to_string(),
    }
}

/// A credit with only its required members recorded: the minimum a store can hold.
///
/// Rows below vary from here along exactly what the reported rule generalises over.
fn credit(credit_id: &str, account_id: &str) -> Credit {
    Credit {
        credit_id: credit_id.to_string(),
        account_id: account_id.to_string(),
        credit_type: "PROMOTIONAL".to_string(),
        initial_amount: money("USD", "100.00"),
        remaining_amount: money("USD", "100.00"),
        estimated_amount: None,
        applicable_product_names: Vec::new(),
        description: format!("credit {credit_id}"),
        start_date: days_ago(30),
        end_date: None,
        exhaust_date: None,
        application_type: None,
        shareable_accounts: Vec::new(),
        account_has_credit_sharing_enabled: None,
        credit_console_visibility: None,
        credit_sharing_type: None,
        cost_category_arn: None,
        rule_name: None,
        credit_status: None,
        purchase_type_applications: Vec::new(),
    }
}

/// Put `credits` in the store, as the billing systems that mint them would.
fn seed(engine: &Arc<engine::InMemoryEngine>, credits: Vec<Credit>) {
    let store = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut store = store.lock();
    for one in credits {
        assert!(
            store.insert_credit(one.clone()),
            "case {:?}: the store must accept the seeded credit",
            one.credit_id
        );
    }
}

/// A `GetCredits` request body for `account_id` over the last `lookback_days`.
fn request(account_id: &str, lookback_days: u64) -> Value {
    json!({
        "accountId": account_id,
        "startDate": epoch(days_ago(lookback_days)),
        "endDate": epoch(SystemTime::now()),
    })
}

/// The reported entry for `credit_id`, or a panic naming the case.
fn reported<'a>(body: &'a Value, case: &str, credit_id: &str) -> &'a Value {
    body["credits"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: credits must be a list, got {body:?}"))
        .iter()
        .find(|credit| credit["creditId"].as_str() == Some(credit_id))
        .unwrap_or_else(|| panic!("case {case:?}: {credit_id} must be reported, got {body:?}"))
}

/// Assert the reported `CreditData` carries exactly what the store holds.
///
/// A timestamp is compared on the second, the scale the modeled `Timestamp` travels
/// on; an unrecorded optional list is an ABSENT member rather than an empty one.
fn assert_reports_stored(case: &str, got: &Value, stored: &Credit) {
    assert_eq!(
        got["accountId"].as_str(),
        Some(stored.account_id.as_str()),
        "case {case:?}: expected accountId {:?}, got {:?}",
        stored.account_id,
        got["accountId"]
    );
    assert_eq!(
        got["creditType"].as_str(),
        Some(stored.credit_type.as_str()),
        "case {case:?}: expected creditType {:?}, got {:?}",
        stored.credit_type,
        got["creditType"]
    );
    assert_eq!(
        got["description"].as_str(),
        Some(stored.description.as_str()),
        "case {case:?}: expected description {:?}, got {:?}",
        stored.description,
        got["description"]
    );
    assert_amount(case, "initialAmount", &got["initialAmount"], Some(&stored.initial_amount));
    assert_amount(
        case,
        "remainingAmount",
        &got["remainingAmount"],
        Some(&stored.remaining_amount),
    );
    assert_amount(
        case,
        "estimatedAmount",
        &got["estimatedAmount"],
        stored.estimated_amount.as_ref(),
    );
    assert_timestamp(case, "startDate", &got["startDate"], Some(stored.start_date));
    assert_timestamp(case, "endDate", &got["endDate"], stored.end_date);
    assert_timestamp(case, "exhaustDate", &got["exhaustDate"], stored.exhaust_date);
    assert_optional_string(
        case,
        "applicationType",
        &got["applicationType"],
        stored.application_type.as_deref(),
    );
    assert_optional_string(
        case,
        "creditConsoleVisibility",
        &got["creditConsoleVisibility"],
        stored.credit_console_visibility.as_deref(),
    );
    assert_optional_string(
        case,
        "creditSharingType",
        &got["creditSharingType"],
        stored.credit_sharing_type.as_deref(),
    );
    assert_optional_string(
        case,
        "costCategoryArn",
        &got["costCategoryArn"],
        stored.cost_category_arn.as_deref(),
    );
    assert_optional_string(case, "ruleName", &got["ruleName"], stored.rule_name.as_deref());
    assert_optional_string(
        case,
        "creditStatus",
        &got["creditStatus"],
        stored.credit_status.as_deref(),
    );
    assert_eq!(
        got["accountHasCreditSharingEnabled"].as_bool(),
        stored.account_has_credit_sharing_enabled,
        "case {case:?}: expected accountHasCreditSharingEnabled {:?}, got {:?}",
        stored.account_has_credit_sharing_enabled,
        got["accountHasCreditSharingEnabled"]
    );
    assert_list(
        case,
        "applicableProductNames",
        &got["applicableProductNames"],
        &stored.applicable_product_names,
    );
    assert_list(
        case,
        "shareableAccounts",
        &got["shareableAccounts"],
        &stored.shareable_accounts,
    );
    assert_list(
        case,
        "purchaseTypeApplications",
        &got["purchaseTypeApplications"],
        &stored.purchase_type_applications,
    );
}

fn assert_amount(case: &str, member: &str, got: &Value, stored: Option<&CreditAmount>) {
    match stored {
        None => assert!(
            got.is_null(),
            "case {case:?}: {member} is not recorded, so it must be absent, got {got:?}"
        ),
        Some(amount) => assert_eq!(
            (
                got["currencyCode"].as_str(),
                got["currencyAmount"].as_str()
            ),
            (
                Some(amount.currency_code.as_str()),
                Some(amount.currency_amount.as_str())
            ),
            "case {case:?}: expected {member} {amount:?}, got {got:?}"
        ),
    }
}

fn assert_timestamp(case: &str, member: &str, got: &Value, stored: Option<SystemTime>) {
    match stored {
        None => assert!(
            got.is_null(),
            "case {case:?}: {member} is not recorded, so it must be absent, got {got:?}"
        ),
        Some(at) => {
            let reported = got
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: {member} must be epoch seconds, got {got:?}"));
            let expected = epoch(at) as f64;
            assert!(
                (reported - expected).abs() < 1.0,
                "case {case:?}: expected {member} {expected}, got {reported}"
            );
        }
    }
}

fn assert_optional_string(case: &str, member: &str, got: &Value, stored: Option<&str>) {
    assert_eq!(
        got.as_str(),
        stored,
        "case {case:?}: expected {member} {stored:?}, got {got:?}"
    );
}

fn assert_list(case: &str, member: &str, got: &Value, stored: &[String]) {
    if stored.is_empty() {
        assert!(
            got.is_null(),
            "case {case:?}: nothing is recorded for {member}, so it must be absent, got {got:?}"
        );
        return;
    }
    let reported: Vec<String> = got
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a list, got {got:?}"))
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, stored,
        "case {case:?}: expected {member} {stored:?}, got {reported:?}"
    );
}

/// One row of the report table: a stored credit and the case it stands for.
struct CreditRow {
    case: &'static str,
    credit: Credit,
}

/// The rows differ along everything the rule generalises over: how long the
/// `creditId` is (the modeled `CreditId` bounds are 1..32 characters of `^[0-9]+$`),
/// which currency the amounts are in, whether the optional amounts, dates, lists and
/// enum members are recorded, how many entries each list holds, and which value of
/// each modeled enum is carried — `ApplicationType`
/// (BEFORE_CROSS_SERVICE_DISCOUNTS | AFTER_DISCOUNTS), `CreditSharingType`
/// (DEFAULT | DISABLED | CUSTOM | COST_CATEGORY_RULE) and `CreditStatus`
/// (ENABLED | DISABLED), as
/// `smithy_query(query="shape",shape="ApplicationType,CreditSharingType,CreditStatus")`
/// lists them.
fn report_rows() -> Vec<CreditRow> {
    let mut rows = Vec::new();

    rows.push(CreditRow {
        case: "only required members, one-character credit id",
        credit: credit("1", ACCOUNT_ID),
    });

    let mut maximal = credit(&"9".repeat(32), ACCOUNT_ID);
    maximal.credit_type = "AWS_ACTIVATE".to_string();
    maximal.initial_amount = money("EUR", "2500.50");
    maximal.remaining_amount = money("EUR", "1250.25");
    maximal.estimated_amount = Some(money("EUR", "900.00"));
    maximal.applicable_product_names =
        vec!["AmazonEC2".to_string(), "AmazonS3".to_string(), "AWSLambda".to_string()];
    maximal.end_date = Some(days_ago(1));
    maximal.exhaust_date = Some(days_ago(2));
    maximal.application_type = Some("AFTER_DISCOUNTS".to_string());
    maximal.shareable_accounts = vec![LINKED_ACCOUNT_ID.to_string(), ACCOUNT_ID.to_string()];
    maximal.account_has_credit_sharing_enabled = Some(true);
    maximal.credit_console_visibility = Some("VISIBLE".to_string());
    maximal.credit_sharing_type = Some("COST_CATEGORY_RULE".to_string());
    maximal.cost_category_arn =
        Some("arn:aws:ce::000000000000:costcategory/credit-sharing".to_string());
    maximal.rule_name = Some("share-with-linked".to_string());
    maximal.credit_status = Some("ENABLED".to_string());
    maximal.purchase_type_applications = vec!["OnDemand".to_string(), "Reserved".to_string()];
    rows.push(CreditRow {
        case: "every member recorded, 32-character credit id",
        credit: maximal,
    });

    let mut zero_decimal_currency = credit("20", ACCOUNT_ID);
    zero_decimal_currency.initial_amount = money("JPY", "15000.00");
    zero_decimal_currency.remaining_amount = money("JPY", "0.00");
    zero_decimal_currency.exhaust_date = Some(days_ago(3));
    zero_decimal_currency.credit_status = Some("DISABLED".to_string());
    zero_decimal_currency.application_type = Some("BEFORE_CROSS_SERVICE_DISCOUNTS".to_string());
    zero_decimal_currency.credit_sharing_type = Some("DISABLED".to_string());
    rows.push(CreditRow {
        case: "exhausted credit in a zero-decimal currency, disabled",
        credit: zero_decimal_currency,
    });

    let mut single_product = credit("30", ACCOUNT_ID);
    single_product.applicable_product_names = vec!["AmazonEC2".to_string()];
    single_product.credit_sharing_type = Some("DEFAULT".to_string());
    single_product.account_has_credit_sharing_enabled = Some(false);
    rows.push(CreditRow {
        case: "one applicable product, sharing off for the account",
        credit: single_product,
    });

    let mut custom_sharing = credit("40", ACCOUNT_ID);
    custom_sharing.credit_sharing_type = Some("CUSTOM".to_string());
    custom_sharing.shareable_accounts = vec![LINKED_ACCOUNT_ID.to_string()];
    custom_sharing.end_date = Some(SystemTime::now() + Duration::from_secs(90 * DAY));
    rows.push(CreditRow {
        case: "custom sharing, expires in the future",
        credit: custom_sharing,
    });

    rows
}

#[tokio::test]
async fn get_credits_reports_every_stored_credit_of_the_account() {
    let (router, engine) = emulator();
    let rows = report_rows();
    seed(&engine, rows.iter().map(|row| row.credit.clone()).collect());

    let (status, body) = call(&router, "GetCredits", &request(ACCOUNT_ID, 60)).await;
    assert_eq!(
        status, 200,
        "case \"report the account's credits\": expected 200, got {status} {body:?}"
    );

    for row in &rows {
        let got = reported(&body, row.case, &row.credit.credit_id);
        assert_reports_stored(row.case, got, &row.credit);
    }

    // Deterministic order: the store is keyed and ordered by `creditId`.
    let mut ordered = credit_ids(&body);
    let reported_order = ordered.clone();
    ordered.sort();
    assert_eq!(
        reported_order, ordered,
        "case \"ordering\": credits must be reported in creditId order, got {reported_order:?}"
    );
    assert_eq!(
        reported_order.len(),
        rows.len(),
        "case \"ordering\": expected {} credits, got {reported_order:?}",
        rows.len()
    );
}

#[tokio::test]
async fn an_account_with_no_credits_gets_an_empty_list() {
    let (router, _engine) = emulator();

    // A fresh store, and a 12-digit account this emulator holds nothing for: both
    // answer with the list the model documents as empty, never with an absent member.
    for case in ["fresh store, own account", "fresh store, unknown account"] {
        let account = if case.ends_with("own account") {
            ACCOUNT_ID
        } else {
            UNKNOWN_ACCOUNT_ID
        };
        let (status, body) = call(&router, "GetCredits", &request(account, 30)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["credits"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected an empty credits list, got {:?}",
            body["credits"]
        );
    }
}

#[tokio::test]
async fn only_credits_overlapping_the_requested_period_are_reported() {
    let (router, engine) = emulator();

    // Three credits placed differently against the clock: one closed long ago, one
    // closed recently, and one still open.
    let mut closed_long_ago = credit("11", ACCOUNT_ID);
    closed_long_ago.start_date = days_ago(300);
    closed_long_ago.end_date = Some(days_ago(200));
    let mut closed_recently = credit("12", ACCOUNT_ID);
    closed_recently.start_date = days_ago(100);
    closed_recently.end_date = Some(days_ago(10));
    let mut open = credit("13", ACCOUNT_ID);
    open.start_date = days_ago(50);
    open.end_date = None;
    seed(
        &engine,
        vec![closed_long_ago, closed_recently, open.clone()],
    );

    // (case, startDate days ago, endDate days ago, the creditIds the period selects)
    let windows: Vec<(&str, u64, u64, Vec<&str>)> = vec![
        ("a period entirely inside the oldest credit", 350, 250, vec!["11"]),
        ("a period straddling the oldest credit's end", 210, 190, vec!["11"]),
        ("a period between the first two credits", 190, 150, vec![]),
        ("the last 150 days", 150, 0, vec!["12", "13"]),
        ("the last 5 days, after the second credit ended", 5, 0, vec!["13"]),
        ("the last 360 days", 360, 0, vec!["11", "12", "13"]),
    ];

    for (case, start_days_ago, end_days_ago, expected) in windows {
        let body = json!({
            "accountId": ACCOUNT_ID,
            "startDate": epoch(days_ago(start_days_ago)),
            "endDate": epoch(days_ago(end_days_ago)),
        });
        let (status, got) = call(&router, "GetCredits", &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {got:?}");
        assert_eq!(
            credit_ids(&got),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            credit_ids(&got)
        );
    }

    // "Defaults to the current date when omitted": an omitted endDate selects the
    // same credits as one at the current instant.
    let (status, defaulted) = call(
        &router,
        "GetCredits",
        &json!({ "accountId": ACCOUNT_ID, "startDate": epoch(days_ago(5)) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"omitted endDate\": expected 200, got {status} {defaulted:?}"
    );
    assert_eq!(
        credit_ids(&defaulted),
        vec!["13".to_string()],
        "case \"omitted endDate\": expected the open credit only, got {:?}",
        credit_ids(&defaulted)
    );
}

/// One row of the `payerAccountFlag` table below.
struct FamilyCase {
    /// What the row varies, for the assertion messages.
    case: &'static str,
    /// Which account pays the consolidated billing family's bill. The caller is
    /// always `caller_account_id()` (= [`ACCOUNT_ID`]), so any other value here is a
    /// caller that does NOT pay for the family.
    payer_account_id: &'static str,
    /// The account the request asks about.
    account_id: &'static str,
    /// `payerAccountFlag`, absent when the row omits the member.
    flag: Option<bool>,
    /// The `creditId`s the request must select, in the order they are reported.
    expected: Vec<&'static str>,
}

/// `payerAccountFlag` is keyed on the CALLER, not on the account the request asks
/// about: when the caller pays the consolidated billing family's bill, the flag
/// aggregates the whole family whichever account `accountId` names, and when it does
/// not, the flag changes nothing.
///
/// From `GetCreditsRequest$payerAccountFlag`: "When true and the caller is the
/// management account, the response aggregates credits across the entire consolidated
/// billing family. When false or omitted, returns only credits for the specified
/// accountId."
#[tokio::test]
async fn payer_account_flag_aggregates_the_whole_billing_family_for_the_paying_caller() {
    let cases = vec![
        FamilyCase {
            case: "paying caller, own account, flag omitted",
            payer_account_id: ACCOUNT_ID,
            account_id: ACCOUNT_ID,
            flag: None,
            expected: vec!["21"],
        },
        FamilyCase {
            case: "paying caller, own account, flag false",
            payer_account_id: ACCOUNT_ID,
            account_id: ACCOUNT_ID,
            flag: Some(false),
            expected: vec!["21"],
        },
        FamilyCase {
            case: "paying caller, own account, flag true",
            payer_account_id: ACCOUNT_ID,
            account_id: ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["21", "22", "23"],
        },
        FamilyCase {
            case: "paying caller, a member account, flag omitted",
            payer_account_id: ACCOUNT_ID,
            account_id: LINKED_ACCOUNT_ID,
            flag: None,
            expected: vec!["22"],
        },
        FamilyCase {
            case: "paying caller, a member account, flag false",
            payer_account_id: ACCOUNT_ID,
            account_id: LINKED_ACCOUNT_ID,
            flag: Some(false),
            expected: vec!["22"],
        },
        FamilyCase {
            case: "paying caller, a member account, flag true",
            payer_account_id: ACCOUNT_ID,
            account_id: LINKED_ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["21", "22", "23"],
        },
        FamilyCase {
            case: "paying caller, a third account, flag true",
            payer_account_id: ACCOUNT_ID,
            account_id: UNKNOWN_ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["21", "22", "23"],
        },
        FamilyCase {
            case: "non-paying caller, own account, flag omitted",
            payer_account_id: LINKED_ACCOUNT_ID,
            account_id: ACCOUNT_ID,
            flag: None,
            expected: vec!["21"],
        },
        FamilyCase {
            case: "non-paying caller cannot aggregate its own account",
            payer_account_id: LINKED_ACCOUNT_ID,
            account_id: ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["21"],
        },
        FamilyCase {
            case: "non-paying caller cannot aggregate another account",
            payer_account_id: LINKED_ACCOUNT_ID,
            account_id: LINKED_ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["22"],
        },
        FamilyCase {
            case: "caller of a family paid by a third account, flag true",
            payer_account_id: UNKNOWN_ACCOUNT_ID,
            account_id: UNKNOWN_ACCOUNT_ID,
            flag: Some(true),
            expected: vec!["23"],
        },
    ];

    for FamilyCase {
        case,
        payer_account_id,
        account_id,
        flag,
        expected,
    } in cases
    {
        let (router, engine) = emulator();
        seed(
            &engine,
            vec![
                credit("21", ACCOUNT_ID),
                credit("22", LINKED_ACCOUNT_ID),
                credit("23", UNKNOWN_ACCOUNT_ID),
            ],
        );
        {
            let store = engine.state::<ComAmazonawsBillingAwsbillingState>();
            let mut store = store.lock();
            store.billing_family_management_account_id = payer_account_id.to_string();
        }

        let mut body = request(account_id, 60);
        if let Some(flag) = flag {
            body["payerAccountFlag"] = json!(flag);
        }
        let (status, got) = call(&router, "GetCredits", &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {got:?}");
        assert_eq!(
            credit_ids(&got),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            credit_ids(&got)
        );
    }
}

#[tokio::test]
async fn a_request_outside_the_documented_bounds_is_refused() {
    let (router, engine) = emulator();
    seed(&engine, vec![credit("31", ACCOUNT_ID)]);

    let now = SystemTime::now();
    // (case, request, the member the refusal must name, the message it must carry
    // where the service's wording for the rule is known)
    let refusals: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "accountId too short",
            json!({ "accountId": "12345", "startDate": epoch(days_ago(30)) }),
            "accountId",
            None,
        ),
        (
            "accountId not numeric",
            json!({ "accountId": "abcdefghijkl", "startDate": epoch(days_ago(30)) }),
            "accountId",
            None,
        ),
        (
            "accountId thirteen digits",
            json!({ "accountId": "0000000000000", "startDate": epoch(days_ago(30)) }),
            "accountId",
            None,
        ),
        (
            "accountId empty",
            json!({ "accountId": "", "startDate": epoch(days_ago(30)) }),
            "accountId",
            None,
        ),
        (
            "startDate in the future",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(now + Duration::from_secs(3600)),
            }),
            "startDate",
            None,
        ),
        (
            // One past the documented bound: "not more than one year before the
            // current date", a year taken as 365 days.
            "startDate 366 days ago",
            json!({ "accountId": ACCOUNT_ID, "startDate": epoch(days_ago(366)) }),
            "startDate",
            None,
        ),
        (
            "endDate in the future",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(days_ago(30)),
                "endDate": epoch(now + Duration::from_secs(3600)),
            }),
            "endDate",
            None,
        ),
        (
            "endDate before startDate",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(days_ago(10)),
                "endDate": epoch(days_ago(20)),
            }),
            "endDate",
            // The sentence the service answers for the ordering rule.
            Some("End date cannot be before start date."),
        ),
        (
            "endDate one second before startDate",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(days_ago(10)),
                "endDate": epoch(days_ago(10)) - 1,
            }),
            "endDate",
            Some("End date cannot be before start date."),
        ),
    ];

    for (case, body, field, message) in refusals {
        let (status, got) = call(&router, "GetCredits", &body).await;
        assert_eq!(
            (status, error_code(&got).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {got:?}"
        );
        assert_eq!(
            got["reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {case:?}: expected reason fieldValidationFailed, got {:?}",
            got["reason"]
        );
        assert_eq!(
            got["fieldList"][0]["name"].as_str(),
            Some(field),
            "case {case:?}: expected the refusal to name {field}, got {:?}",
            got["fieldList"]
        );
        if let Some(message) = message {
            assert_eq!(
                got["message"].as_str(),
                Some(message),
                "case {case:?}: expected the message {message:?}, got {:?}",
                got["message"]
            );
        }
    }

    // On the accepted side of each bound: 364 days of lookback, a period of zero
    // length, and an endDate at the current instant.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "startDate 364 days ago",
            json!({ "accountId": ACCOUNT_ID, "startDate": epoch(days_ago(364)) }),
        ),
        (
            "endDate equal to startDate",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(days_ago(10)),
                "endDate": epoch(days_ago(10)),
            }),
        ),
        (
            "endDate at the current instant",
            json!({
                "accountId": ACCOUNT_ID,
                "startDate": epoch(days_ago(1)),
                "endDate": epoch(now),
            }),
        ),
    ];

    for (case, body) in accepted {
        let (status, got) = call(&router, "GetCredits", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {got:?}"
        );
        assert!(
            got["credits"].is_array(),
            "case {case:?}: expected a credits list, got {got:?}"
        );
    }
}

#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let (router, engine) = emulator();
    seed(
        &engine,
        vec![credit("41", ACCOUNT_ID), credit("42", LINKED_ACCOUNT_ID)],
    );

    let own = request(ACCOUNT_ID, 60);
    let mut family = own.clone();
    family["payerAccountFlag"] = json!(true);

    let (_, first) = call(&router, "GetCredits", &own).await;
    for round in 1..=3 {
        let case = format!("repeat {round}");
        let (status, family_body) = call(&router, "GetCredits", &family).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the family read must answer 200, got {status} {family_body:?}"
        );
        let (status, again) = call(&router, "GetCredits", &own).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {again:?}");
        assert_eq!(
            again, first,
            "case {case:?}: a read must not change what a later caller sees, got {again:?}"
        );
    }

    let store = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let store = store.lock();
    assert_eq!(
        store.credits.len(),
        2,
        "case \"store size\": reads must store nothing, got {:?}",
        store.credits.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .credits
            .get("41")
            .map(|held| held.remaining_amount.currency_amount.clone()),
        Some("100.00".to_string()),
        "case \"remaining amount\": a read must not consume a credit, got {:?}",
        store.credits.get("41").map(|held| held.remaining_amount.clone())
    );
}
