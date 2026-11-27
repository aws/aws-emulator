// SPDX-License-Identifier: CC0-1.0

//! What a self-service password reset reports on a pool that cannot send a text message,
//! over the awsJson1_1 wire.
//!
//! The rule: `ForgotPassword` REPORTS the destination it chose
//! (smithy_query(query="operation", operation="ForgotPassword") — "the message delivery
//! method is determined by the user's available attributes and the AccountRecoverySetting
//! configuration of the user pool"), so it takes the mechanisms the pool's
//! `AccountRecoverySetting` ranks in ascending `Priority` and uses the first one whose medium
//! the pool can actually deliver over AND whose attribute the user holds verified; a
//! mechanism the pool has no delivery path for is skipped and the next one by priority is
//! used (smithy_query(query="shape", shape="RecoveryOptionType") — the pool "sends
//! account-recovery messages to" the higher-priority attribute "but falls back to" the other
//! one). A text message leaves a pool only through the caller its `SmsConfiguration` names or
//! the function its `CustomSMSSender` names (smithy_query(query="shape",
//! shape="SmsConfigurationType") and `LambdaConfigType$CustomSMSSender`), so a pool carrying
//! neither never reports an SMS delivery — whatever the user's phone attributes say.
//!
//! The consequence this suite pins is the one a recovery UI branches on: on an SMS-incapable
//! pool a user whose phone IS verified and a user holding no phone at all are answered the
//! SAME way, because the phone made no difference to the mechanism that qualified. The
//! contrast test then gives the very same pool and users an SMS delivery path and shows the
//! phone user recovering over it, so the EMAIL answer is the capability gate talking rather
//! than a fixed reply.
//!
//! `tests/all/recovery_delivery_path.rs` owns the ranked walk itself (including the refusal
//! for a user no qualifying mechanism reaches); the rows here differ along the shape of the
//! ranking, the state of the user's phone, the pool's username form, and the address the
//! masked destination is computed from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state::masked_destination;

/// This service's SigV4 signing name, the 4th field of a credential scope, which is what
/// routes a request to this service. Nothing is signed: the suite turns request
/// authentication off.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// A password satisfying a default pool's password policy, so the user OWNS one and a
/// self-service reset applies to it.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The Amazon SNS caller an `SmsConfiguration` names, which is one of the two paths a text
/// message leaves a pool through — the one the contrast test gives a pool that had none.
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/service-role/cognito-sms";

/// This service alone, over one engine, with request authentication off.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Call `operation` with `body` and return the status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.us-east-1.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Create a pool carrying `configuration` and an app client of it, and answer
/// `(user_pool_id, client_id)`.
async fn pool_with_client(
    router: &MultiServiceRouter,
    name: &str,
    configuration: &Value,
) -> (String, String) {
    let mut body = json!({ "PoolName": name });
    if let Value::Object(members) = configuration.clone() {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPool", &body).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": "app" }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create a CONFIRMED user of `user_pool_id` named `username` and holding `attributes`: the
/// administrative create is suppressed and a permanent password is set, so the user owns a
/// password a self-service reset can replace.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    let user_attributes: Vec<Value> = attributes
        .iter()
        .map(|(attribute, value)| json!({ "Name": attribute, "Value": value }))
        .collect();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "MessageAction": "SUPPRESS",
            "UserAttributes": user_attributes,
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let described = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    assert_eq!(
        described["UserStatus"].as_str(),
        Some("CONFIRMED"),
        "user {username:?} must own a password before a reset, got {described}"
    );
}

/// Assert, from the SERVICE's own read, the premise every row below rests on: the pool ranks
/// `expected_first` highest and carries no `SmsConfiguration` at all.
async fn premise(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    case: &str,
    expected_first: &str,
) {
    let described = ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let pool = &described["UserPool"];
    assert!(
        pool.get("SmsConfiguration").is_none(),
        "case {case:?}: the pool must carry no SmsConfiguration, got {pool}"
    );
    let mechanisms = pool["AccountRecoverySetting"]["RecoveryMechanisms"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let highest = mechanisms
        .iter()
        .min_by_key(|mechanism| mechanism["Priority"].as_i64().unwrap_or(i64::MAX))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        highest["Name"].as_str(),
        Some(expected_first),
        "case {case:?}: expected the pool to rank {expected_first:?} highest, got {mechanisms:?}"
    );
}

/// The `CodeDeliveryDetails` a served reset of `username` reports.
async fn reset_details(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    case: &str,
) -> Value {
    let (status, payload) = call(
        router,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": username }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a reported delivery for {username:?}, got {status} with \
         {payload}"
    );
    payload["CodeDeliveryDetails"].clone()
}

/// Assert `details` reports a code sent to `attribute` over `medium`, addressed to the masked
/// form of `address`.
fn reports_delivery(details: &Value, case: &str, attribute: &str, medium: &str, address: &str) {
    assert_eq!(
        details["AttributeName"].as_str(),
        Some(attribute),
        "case {case:?}: expected AttributeName {attribute:?}, got {details}"
    );
    assert_eq!(
        details["DeliveryMedium"].as_str(),
        Some(medium),
        "case {case:?}: expected DeliveryMedium {medium:?}, got {details}"
    );
    let expected = masked_destination(address);
    let reported = details["Destination"].as_str().unwrap_or_default();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the masked form {expected:?} of {address:?}, got {details}"
    );
    // The properties the masked form has whatever the address is: it identifies the contact
    // by its leading character, it hides the rest behind `*`, and it is not the address
    // itself — so a row cannot pass by reporting an unmasked or unrelated destination.
    let initial = address.chars().next().unwrap_or_default();
    assert!(
        reported.starts_with(initial) && reported.contains('*') && reported != address,
        "case {case:?}: expected a masked form of {address:?} keeping {initial:?}, got {details}"
    );
}

/// One row: an SMS-incapable pool's recovery ranking, and the phone state of the user whose
/// reset it answers.
struct Row {
    case: &'static str,
    /// The pool's `AccountRecoverySetting.RecoveryMechanisms`, or `Value::Null` for a pool
    /// that configures no recovery at all.
    mechanisms: Value,
    /// The mechanism the pool ranks highest, asserted back from `DescribeUserPool`.
    ranks_first: &'static str,
    /// The email both users of the row hold verified, which is where the code must go.
    email: &'static str,
    /// What the user WITH a phone holds for it, beside that email.
    phone: Vec<(&'static str, &'static str)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the phone mechanism ranked first, then the email",
            mechanisms: json!([
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 },
            ]),
            ranks_first: "verified_phone_number",
            email: "a@example.test",
            phone: vec![
                ("phone_number", "+15550100011"),
                ("phone_number_verified", "true"),
            ],
        },
        Row {
            case: "the same ranking declared in the reverse list order",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 2 },
                { "Name": "verified_phone_number", "Priority": 1 },
            ]),
            ranks_first: "verified_phone_number",
            email: "second.user@mail.example.test",
            phone: vec![
                ("phone_number", "+447700900012"),
                ("phone_number_verified", "true"),
            ],
        },
        Row {
            case: "the phone mechanism ranked first and the user's number left unverified",
            mechanisms: json!([
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 },
            ]),
            ranks_first: "verified_phone_number",
            email: "z@e.test",
            phone: vec![
                ("phone_number", "+15550100013"),
                ("phone_number_verified", "false"),
            ],
        },
        Row {
            case: "the email mechanism ranked first, which the phone cannot outrank",
            mechanisms: json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 2 },
            ]),
            ranks_first: "verified_email",
            email: "Fourth_User+tag@example.test",
            phone: vec![
                ("phone_number", "+15550100014"),
                ("phone_number_verified", "true"),
            ],
        },
        Row {
            case: "a pool that configures no recovery of its own",
            mechanisms: Value::Null,
            ranks_first: "verified_email",
            email: "fifth@example.test",
            phone: vec![
                ("phone_number", "+15550100015"),
                ("phone_number_verified", "true"),
            ],
        },
    ]
}

/// A pool that cannot send a text message answers a reset over EMAIL whatever the user's
/// phone says: the user whose number is verified and the user holding no phone at all are
/// reported the same destination, because the phone made no difference to which mechanism
/// qualified.
#[tokio::test]
async fn an_sms_incapable_pool_recovers_by_email_whatever_the_user_s_phone_says() {
    let router = router();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let configuration = if row.mechanisms.is_null() {
            json!({})
        } else {
            json!({ "AccountRecoverySetting": { "RecoveryMechanisms": row.mechanisms } })
        };
        let (user_pool_id, client_id) =
            pool_with_client(&router, &format!("no-sms-path-{index}"), &configuration).await;
        premise(&router, &user_pool_id, case, row.ranks_first).await;

        let verified_email = vec![("email", row.email), ("email_verified", "true")];
        let mut with_phone = verified_email.clone();
        with_phone.extend(row.phone.iter().copied());
        let phone_user = format!("phone-holder-{index}");
        let plain_user = format!("email-only-{index}");
        confirmed_user(&router, &user_pool_id, &phone_user, &with_phone).await;
        confirmed_user(&router, &user_pool_id, &plain_user, &verified_email).await;

        let for_phone_user = reset_details(&router, &client_id, &phone_user, case).await;
        let for_plain_user = reset_details(&router, &client_id, &plain_user, case).await;
        reports_delivery(&for_phone_user, case, "email", "EMAIL", row.email);
        reports_delivery(&for_plain_user, case, "email", "EMAIL", row.email);
        assert_eq!(
            for_phone_user, for_plain_user,
            "case {case:?}: the pool cannot deliver by SMS, so the user holding a phone must \
             be answered exactly as the user holding none: got {for_phone_user} and \
             {for_plain_user}"
        );
    }
}

/// The same pool and the same users, once the pool is given a path a text message leaves it
/// through: the phone mechanism it ranks first now qualifies, so the user whose number is
/// verified recovers over SMS while the user holding no phone still recovers by email.
///
/// So the EMAIL answers above are the pool's delivery capability deciding, not a fixed reply.
#[tokio::test]
async fn giving_the_pool_an_sms_path_is_what_makes_the_phone_mechanism_qualify() {
    let router = router();
    let case = "a phone-first pool given an Amazon SNS caller";
    let mechanisms = json!([
        { "Name": "verified_phone_number", "Priority": 1 },
        { "Name": "verified_email", "Priority": 2 },
    ]);
    let configuration = json!({ "AccountRecoverySetting": { "RecoveryMechanisms": mechanisms } });
    let (user_pool_id, client_id) =
        pool_with_client(&router, "sms-path-added", &configuration).await;
    premise(&router, &user_pool_id, case, "verified_phone_number").await;

    let email = "capability@example.test";
    let number = "+15550100016";
    let verified_email = vec![("email", email), ("email_verified", "true")];
    let mut with_phone = verified_email.clone();
    with_phone.extend([("phone_number", number), ("phone_number_verified", "true")]);
    confirmed_user(&router, &user_pool_id, "phone-holder", &with_phone).await;
    confirmed_user(&router, &user_pool_id, "email-only", &verified_email).await;

    // Before: no path, so the phone mechanism is skipped for both users.
    let before = reset_details(&router, &client_id, "phone-holder", case).await;
    reports_delivery(&before, case, "email", "EMAIL", email);

    // An update that configures Amazon SNS delivery. The ranking is sent again with it
    // because a pool update is a WRITE of the pool's configuration, not a patch of it.
    ok(
        &router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": user_pool_id,
            "AccountRecoverySetting": {
                "RecoveryMechanisms": [
                    { "Name": "verified_phone_number", "Priority": 1 },
                    { "Name": "verified_email", "Priority": 2 },
                ],
            },
            "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN, "ExternalId": "tenant" },
        }),
    )
    .await;
    let described = ok(
        &router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        described["UserPool"]["SmsConfiguration"]["SnsCallerArn"].as_str(),
        Some(SNS_CALLER_ARN),
        "case {case:?}: the updated pool must report the caller it was given, got {described}"
    );

    // After: the mechanism the pool ranks first qualifies for the user that holds a verified
    // number, and the user holding no phone still falls through to the email.
    let after = reset_details(&router, &client_id, "phone-holder", case).await;
    reports_delivery(&after, case, "phone_number", "SMS", number);
    let for_plain_user = reset_details(&router, &client_id, "email-only", case).await;
    reports_delivery(&for_plain_user, case, "email", "EMAIL", email);
}

/// The same rule on a pool whose username IS the email address and whose administrators are
/// the only ones who create users: the pool form a phone-first recovery misconfiguration
/// shows up on. Each user is reported the masked form of its OWN address, so the destination
/// is read off the stored attribute rather than being one address for every caller.
#[tokio::test]
async fn an_email_username_pool_falls_through_to_each_user_s_own_email() {
    let router = router();
    let case = "a phone-first pool whose username is the email address";
    let configuration = json!({
        "UsernameAttributes": ["email"],
        "AutoVerifiedAttributes": ["email"],
        "AdminCreateUserConfig": { "AllowAdminCreateUserOnly": true },
        "AccountRecoverySetting": {
            "RecoveryMechanisms": [
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 },
            ],
        },
    });
    let (user_pool_id, client_id) =
        pool_with_client(&router, "email-username-pool", &configuration).await;
    premise(&router, &user_pool_id, case, "verified_phone_number").await;

    let with_phone = "phone.holder@example.test";
    let without_phone = "e@other.example.test";
    confirmed_user(
        &router,
        &user_pool_id,
        with_phone,
        &[
            ("email", with_phone),
            ("email_verified", "true"),
            ("phone_number", "+15550100017"),
            ("phone_number_verified", "true"),
        ],
    )
    .await;
    confirmed_user(
        &router,
        &user_pool_id,
        without_phone,
        &[("email", without_phone), ("email_verified", "true")],
    )
    .await;

    let first = reset_details(&router, &client_id, with_phone, case).await;
    let second = reset_details(&router, &client_id, without_phone, case).await;
    reports_delivery(&first, case, "email", "EMAIL", with_phone);
    reports_delivery(&second, case, "email", "EMAIL", without_phone);
    assert_eq!(
        (
            first["DeliveryMedium"].as_str(),
            first["AttributeName"].as_str()
        ),
        (
            second["DeliveryMedium"].as_str(),
            second["AttributeName"].as_str()
        ),
        "case {case:?}: both users recover over the same medium and attribute, got {first} and \
         {second}"
    );
    assert_ne!(
        first["Destination"], second["Destination"],
        "case {case:?}: each user's destination is the masked form of its OWN address, got \
         {first} and {second}"
    );
}
