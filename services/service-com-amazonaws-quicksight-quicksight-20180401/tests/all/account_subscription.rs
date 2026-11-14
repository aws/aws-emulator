// SPDX-License-Identifier: CC0-1.0

//! The account subscription: sign-up and the describe that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see.
//!
//! The rule under test: every account a QuickSight request addresses has a
//! subscription, which `DescribeAccountSubscription` reports in the `ACCOUNT_CREATED`
//! status from the sign-up `CreateAccountSubscription` stored for that account, or
//! from this service's defaults for an account that never signed up. So the rows below
//! vary the edition, the authentication method and the account id — the inputs the rule
//! generalises over — rather than re-running one recorded call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The `AuthenticationMethodOption` members whose account is authenticated through
/// IAM, which is what `SignupResponse$IAMUser` reports ("TRUE if the Amazon Quick
/// Sight uses IAM as an authentication method"). Catalog source:
/// `smithy_query(query="shape", shape="AuthenticationMethodOption")` and
/// `smithy_query(query="shape", shape="SignupResponse")`.
const IAM_AUTHENTICATION_METHODS: &[&str] = &["IAM_AND_QUICKSIGHT", "IAM_ONLY"];

/// The status a QuickSight account that exists reports.
const ACCOUNT_CREATED: &str = "ACCOUNT_CREATED";

/// The edition an account that never chose one reports.
const DEFAULT_EDITION: &str = "ENTERPRISE";

/// The authentication type an account that never chose one reports: a QuickSight
/// subscription with no identity store of its own, so its users are the Amazon Web
/// Services identities alone until a sign-up chooses otherwise.
const DEFAULT_AUTHENTICATION_TYPE: &str = "IAM_ONLY";

/// One sign-up and what the account must report afterwards.
struct SignupCase {
    /// Case name, and the account the row signs up.
    account: &'static str,
    account_name: &'static str,
    /// The `Edition` the request names, or `None` to leave the optional member out.
    edition: Option<&'static str>,
    /// The edition the account must then report: the one requested, or the service
    /// default for a request that names none.
    expected_edition: &'static str,
    authentication_method: &'static str,
    notification_email: &'static str,
    /// The members the model documents as required for this edition or method.
    extras: Value,
    expected_identity_center_arn: Option<&'static str>,
}

/// Rows spanning BOTH model enums: every `Edition` member (`STANDARD`,
/// `ENTERPRISE`, `ENTERPRISE_AND_Q`), every `AuthenticationMethodOption` member
/// (`IAM_ONLY`, `IAM_AND_QUICKSIGHT`, `IAM_IDENTITY_CENTER`, `ACTIVE_DIRECTORY`), and
/// a request that names no `Edition` at all. Catalog source:
/// `smithy_query(query="shape", shape="Edition,AuthenticationMethodOption")`.
fn signup_cases() -> Vec<SignupCase> {
    vec![
        SignupCase {
            account: "100000000001",
            account_name: "sales-analytics",
            edition: Some("STANDARD"),
            expected_edition: "STANDARD",
            authentication_method: "IAM_ONLY",
            notification_email: "ops+standard@example.com",
            extras: json!({}),
            expected_identity_center_arn: None,
        },
        SignupCase {
            account: "100000000002",
            account_name: "data-platform",
            edition: Some("ENTERPRISE"),
            expected_edition: "ENTERPRISE",
            authentication_method: "IAM_AND_QUICKSIGHT",
            notification_email: "ops+enterprise@example.com",
            extras: json!({}),
            expected_identity_center_arn: None,
        },
        SignupCase {
            account: "100000000003",
            account_name: "q-insights",
            edition: Some("ENTERPRISE_AND_Q"),
            expected_edition: "ENTERPRISE_AND_Q",
            authentication_method: "IAM_IDENTITY_CENTER",
            notification_email: "ops+q@example.com",
            // ENTERPRISE_AND_Q requires the author's contact details, and
            // IAM_IDENTITY_CENTER an admin group.
            extras: json!({
                "FirstName": "Ada",
                "LastName": "Lovelace",
                "EmailAddress": "ada@example.com",
                "ContactNumber": "5550100100",
                "AdminGroup": ["quicksight-admins"],
                "IAMIdentityCenterInstanceArn":
                    "arn:aws:sso:::instance/ssoins-1234567890abcdef",
            }),
            expected_identity_center_arn: Some("arn:aws:sso:::instance/ssoins-1234567890abcdef"),
        },
        SignupCase {
            account: "100000000004",
            account_name: "corp-directory",
            edition: Some("ENTERPRISE"),
            expected_edition: "ENTERPRISE",
            authentication_method: "ACTIVE_DIRECTORY",
            notification_email: "ops+ad@example.com",
            // ACTIVE_DIRECTORY requires the directory's name and realm, plus an
            // admin group.
            extras: json!({
                "ActiveDirectoryName": "corp.example.com",
                "Realm": "CORP.EXAMPLE.COM",
                "DirectoryId": "d-906787654321",
                "AdminProGroup": ["corp-admins"],
            }),
            expected_identity_center_arn: None,
        },
        SignupCase {
            account: "100000000005",
            account_name: "edition-left-out",
            edition: None,
            expected_edition: DEFAULT_EDITION,
            authentication_method: "IAM_AND_QUICKSIGHT",
            notification_email: "ops+default@example.com",
            extras: json!({}),
            expected_identity_center_arn: None,
        },
    ]
}

/// The `CreateAccountSubscription` request body one case sends.
fn signup_body(case: &SignupCase) -> Value {
    let mut body = json!({
        "AccountName": case.account_name,
        "AuthenticationMethod": case.authentication_method,
        "NotificationEmail": case.notification_email,
    });
    if let Some(edition) = case.edition {
        body["Edition"] = json!(edition);
    }
    for (member, value) in case.extras.as_object().expect("the extras are an object") {
        body[member] = value.clone();
    }
    body
}

async fn create_account_subscription(
    harness: &Harness,
    account: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call("POST", &format!("/account/{account}"), Some(body))
        .await
}

async fn describe_account_subscription(harness: &Harness, account: &str) -> (u16, Value) {
    harness.call("GET", &format!("/account/{account}"), None).await
}

#[tokio::test]
async fn a_signed_up_account_reports_its_own_subscription() {
    let harness = Harness::new();

    for case in signup_cases() {
        let account = case.account;
        let (status, body) =
            create_account_subscription(&harness, account, signup_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {account}: sign-up expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["SignupResponse"]["accountName"],
            json!(case.account_name),
            "case {account}: SignupResponse must name the account signed up, got {body}"
        );
        assert_eq!(
            body["SignupResponse"]["IAMUser"],
            json!(IAM_AUTHENTICATION_METHODS.contains(&case.authentication_method)),
            "case {account}: IAMUser must follow AuthenticationMethod {}, got {body}",
            case.authentication_method
        );
        assert!(
            body["RequestId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {account}: sign-up expected a RequestId, got {body}"
        );
        // `Status` is `@httpResponseCode`-bound, so it IS the HTTP status asserted
        // above rather than a body member.
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {account}: Status is bound to the HTTP status, so it is not a body member: {body}"
        );

        // The sign-up is stored, not just answered.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .account_subscriptions
                .get(account)
                .unwrap_or_else(|| panic!("case {account}: the sign-up stored no record"));
            assert_eq!(
                record.account_name, case.account_name,
                "case {account}: stored the wrong AccountName, got {:?}",
                record.account_name
            );
            assert_eq!(
                record.edition, case.expected_edition,
                "case {account}: stored the wrong Edition, got {:?}",
                record.edition
            );
        }

        let (status, body) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {account}: describe expected 200, got {status} with body {body}"
        );
        let info = &body["AccountInfo"];
        assert_eq!(
            info["AccountName"],
            json!(case.account_name),
            "case {account}: expected the signed-up AccountName, got {info}"
        );
        assert_eq!(
            info["Edition"],
            json!(case.expected_edition),
            "case {account}: expected Edition {}, got {info}",
            case.expected_edition
        );
        assert_eq!(
            info["NotificationEmail"],
            json!(case.notification_email),
            "case {account}: expected the signed-up NotificationEmail, got {info}"
        );
        assert_eq!(
            info["AuthenticationType"],
            json!(case.authentication_method),
            "case {account}: expected AuthenticationType {}, got {info}",
            case.authentication_method
        );
        assert_eq!(
            info["AccountSubscriptionStatus"],
            json!(ACCOUNT_CREATED),
            "case {account}: a signed-up account is {ACCOUNT_CREATED}, got {info}"
        );
        assert_eq!(
            info["IAMIdentityCenterInstanceArn"],
            match case.expected_identity_center_arn {
                Some(arn) => json!(arn),
                None => Value::Null,
            },
            "case {account}: expected IAMIdentityCenterInstanceArn {:?}, got {info}",
            case.expected_identity_center_arn
        );
    }
}

/// Account ids an account-subscription read is asked about without any sign-up: the
/// account the emulator presents, a second unrelated one, and the lowest and highest
/// 12-digit ids the model's `AwsAccountId` pattern (`^[0-9]{12}$`) admits.
const UNSIGNED_ACCOUNTS: &[&str] = &[
    "000000000000",
    "210987654321",
    "999999999999",
    "123456789012",
];

#[tokio::test]
async fn an_account_that_never_signed_up_reports_the_service_defaults() {
    let harness = Harness::new();

    for account in UNSIGNED_ACCOUNTS {
        let (status, body) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {account}: describe expected 200, got {status} with body {body}"
        );
        let info = &body["AccountInfo"];
        assert_eq!(
            info["AccountSubscriptionStatus"],
            json!(ACCOUNT_CREATED),
            "case {account}: every served account is {ACCOUNT_CREATED}, got {info}"
        );
        assert_eq!(
            info["Edition"],
            json!(DEFAULT_EDITION),
            "case {account}: expected the default Edition, got {info}"
        );
        assert_eq!(
            info["AuthenticationType"],
            json!(DEFAULT_AUTHENTICATION_TYPE),
            "case {account}: expected the default AuthenticationType, got {info}"
        );
        assert_eq!(
            info["AccountName"],
            json!(format!("quicksight-{account}")),
            "case {account}: the default AccountName is derived from the account id, got {info}"
        );
        // Nothing supplied a notification address, so none is invented.
        assert_eq!(
            info["NotificationEmail"],
            Value::Null,
            "case {account}: an account that never named a NotificationEmail reports none, got {info}"
        );
    }

    // A second read answers the same, and the reads stored nothing: a describe must
    // not change what a later caller sees.
    for account in UNSIGNED_ACCOUNTS {
        let (status, body) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {account}: a repeated describe expected 200, got {status} with body {body}"
        );
    }
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let stored: Vec<String> = state.lock().account_subscriptions.keys().cloned().collect();
    assert!(
        stored.is_empty(),
        "case reads-do-not-write: describing an account must store nothing, got {stored:?}"
    );
}

#[tokio::test]
async fn a_signup_is_invisible_under_every_other_account_id() {
    let harness = Harness::new();

    let case = &signup_cases()[1];
    let (status, body) = create_account_subscription(&harness, case.account, signup_body(case)).await;
    assert_eq!(
        status, 200,
        "case setup: sign-up expected 200, got {status} with body {body}"
    );

    for account in UNSIGNED_ACCOUNTS {
        let (status, body) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {account}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["AccountInfo"]["AccountName"],
            json!(format!("quicksight-{account}")),
            "case {account}: another account's sign-up must not be reported here, got {body}"
        );
        assert_eq!(
            body["AccountInfo"]["NotificationEmail"],
            Value::Null,
            "case {account}: another account's NotificationEmail must not leak here, got {body}"
        );
    }

    let (_, body) = describe_account_subscription(&harness, case.account).await;
    assert_eq!(
        body["AccountInfo"]["AccountName"],
        json!(case.account_name),
        "case {}: the signed-up account still reports its own name, got {body}",
        case.account
    );
}

#[tokio::test]
async fn a_second_signup_for_one_account_is_refused_and_changes_nothing() {
    let harness = Harness::new();

    for case in signup_cases() {
        let account = case.account;
        let (status, body) =
            create_account_subscription(&harness, account, signup_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {account}: the first sign-up expected 200, got {status} with body {body}"
        );

        // A second sign-up differs in every member it may differ in; none of it lands.
        let second = json!({
            "AccountName": "resubscribed",
            "AuthenticationMethod": "IAM_ONLY",
            "NotificationEmail": "someone-else@example.com",
            "Edition": "STANDARD",
        });
        let (status, body) = create_account_subscription(&harness, account, second).await;
        assert_eq!(
            status, 409,
            "case {account}: a second sign-up expected 409, got {status} with body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceExistsException"),
            "case {account}: expected ResourceExistsException, got {body}"
        );

        let (status, body) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {account}: describe expected 200, got {status} with body {body}"
        );
        let info = &body["AccountInfo"];
        assert_eq!(
            info["AccountName"],
            json!(case.account_name),
            "case {account}: the refused sign-up must not replace the stored one, got {info}"
        );
        assert_eq!(
            info["Edition"],
            json!(case.expected_edition),
            "case {account}: the refused sign-up must not change the Edition, got {info}"
        );
        assert_eq!(
            info["NotificationEmail"],
            json!(case.notification_email),
            "case {account}: the refused sign-up must not change the NotificationEmail, got {info}"
        );
    }
}

/// Sign-ups this service refuses: a value outside a modelled enum's catalog (which
/// the framework refuses with a `ValidationException` before the handler runs), and a
/// request missing a member the chosen edition or authentication method requires
/// (which the handler refuses with the operation's modelled
/// `InvalidParameterValueException`). Each row is
/// `(case, account, body, expected error)`.
fn refused_signups() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        (
            "edition outside the catalog",
            "200000000001",
            json!({
                "AccountName": "bad-edition",
                "AuthenticationMethod": "IAM_AND_QUICKSIGHT",
                "NotificationEmail": "ops@example.com",
                "Edition": "PREMIUM",
            }),
            "ValidationException",
        ),
        (
            "authentication method outside the catalog",
            "200000000002",
            json!({
                "AccountName": "bad-method",
                "AuthenticationMethod": "PASSWORD",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            }),
            "ValidationException",
        ),
        (
            "ENTERPRISE_AND_Q without the author's contact details",
            "200000000003",
            json!({
                "AccountName": "q-without-contact",
                "AuthenticationMethod": "IAM_AND_QUICKSIGHT",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE_AND_Q",
                "FirstName": "Ada",
                "LastName": "Lovelace",
            }),
            "InvalidParameterValueException",
        ),
        (
            "ACTIVE_DIRECTORY without the directory it authenticates against",
            "200000000004",
            json!({
                "AccountName": "ad-without-directory",
                "AuthenticationMethod": "ACTIVE_DIRECTORY",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            }),
            "InvalidParameterValueException",
        ),
        (
            "IAM_IDENTITY_CENTER without an admin group",
            "200000000005",
            json!({
                "AccountName": "idc-without-admins",
                "AuthenticationMethod": "IAM_IDENTITY_CENTER",
                "NotificationEmail": "ops@example.com",
                "Edition": "ENTERPRISE",
            }),
            "InvalidParameterValueException",
        ),
    ]
}

#[tokio::test]
async fn a_signup_missing_what_its_choices_require_is_refused_and_stores_nothing() {
    let harness = Harness::new();

    for (case, account, body, expected) in refused_signups() {
        let (status, response) = create_account_subscription(&harness, account, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with body {response}"
        );
        assert_eq!(
            response["__type"],
            json!(expected),
            "case {case}: expected {expected}, got {response}"
        );

        // The refusal stored nothing, so the account still reports the defaults.
        let (status, response) = describe_account_subscription(&harness, account).await;
        assert_eq!(
            status, 200,
            "case {case}: describe expected 200, got {status} with body {response}"
        );
        assert_eq!(
            response["AccountInfo"]["AccountName"],
            json!(format!("quicksight-{account}")),
            "case {case}: a refused sign-up must store nothing, got {response}"
        );
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let stored = state.lock().account_subscriptions.contains_key(account);
        assert!(
            !stored,
            "case {case}: a refused sign-up must store no record for {account}"
        );
    }
}
