// SPDX-License-Identifier: CC0-1.0

//! The VERIFICATION STATE of a user's contact attributes decides what a self-service
//! password reset answers, over the awsJson1_1 wire.
//!
//! The rule: `ForgotPassword` picks the reset channel from the attributes the USER holds
//! VERIFIED, among the mechanisms the pool's `AccountRecoverySetting` ranks, taken in
//! ascending priority — "the message delivery method is determined by the user's available
//! attributes and the AccountRecoverySetting configuration of the user pool", and "if
//! neither a verified phone number nor a verified email exists, Amazon Cognito responds
//! with an InvalidParameterException error"
//! (smithy_query(query="operation", operation="ForgotPassword"), which models that error
//! with httpError 400). A contact value a user holds UNVERIFIED is therefore no channel at
//! all: the pool falls through to the next mechanism it ranks, and a user with no verified
//! contact value is refused rather than told a code was sent. What a served reset reports is
//! the triple for the attribute it chose — `AttributeName`, `DeliveryMedium` (`SMS` for
//! `phone_number`, `EMAIL` for `email`, the two values of `DeliveryMediumType`) and the
//! masked `Destination` read off that attribute's stored value
//! (smithy_query(query="shape", shape="CodeDeliveryDetailsType")).
//!
//! Every pool here can deliver over BOTH media (it names an Amazon SNS caller), so the
//! delivery path never decides anything: the rows differ only along which contact values the
//! user holds, which verified flag each carries (`true`, `false`, or none at all), and which
//! mechanism the pool ranks first. `tests/all/recovery_delivery_path.rs` covers the other
//! half of the choice, the medium the pool has a path for.
//!
//! The last test drives the state through the three answers in turn on ONE user, so what the
//! operation reports is pinned to the verification state as it CHANGES, not just to how a
//! user was created.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state::masked_destination;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// A password satisfying a default pool's password policy, so the user owns one and a
/// self-service reset applies to it.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The sentence a reset is refused with when the user holds no verified contact value. It
/// names the attribute CLASSES, not the user, so it is fixed text for this error class.
const NO_RECOVERY_DESTINATION: &str =
    "Cannot reset password for the user as there is no registered/verified email or phone_number";

/// The Amazon SNS caller an `SmsConfiguration` names, so every pool in this suite has a path
/// for a text message and the medium never disqualifies a mechanism here.
const SNS_CALLER_ARN: &str = "arn:aws:iam::000000000000:role/service-role/cognito-sms";

/// This service alone, over one engine, with request authentication off.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// What a call answered: the status, the `x-amzn-errortype` header it carried (empty when
/// it carried none) and the JSON body.
struct Answer {
    status: u16,
    error_type: String,
    payload: Value,
}

impl Answer {
    /// The `__type` of an error body, which is the wire error code.
    fn code(&self) -> &str {
        self.payload
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
    }

    /// The `message` an error body carries.
    fn message(&self) -> &str {
        self.payload
            .get("message")
            .or_else(|| self.payload.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// Call `operation` with `body`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        payload,
    }
}

/// Call `operation`, requiring success, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let answered = call(router, operation, body).await;
    assert_eq!(
        answered.status,
        200,
        "{operation} must succeed, got {} with {}",
        answered.status,
        answered.payload
    );
    answered.payload
}

/// Create a pool ranking `mechanisms` and able to send over both media, an app client of it,
/// and a user named `name` holding `attributes` with a permanent password. Answers
/// `(user_pool_id, client_id)`.
async fn pool_with_user(
    router: &MultiServiceRouter,
    name: &str,
    mechanisms: &Value,
    attributes: &[(&str, &str)],
) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": name,
            "AccountRecoverySetting": { "RecoveryMechanisms": mechanisms },
            "SmsConfiguration": { "SnsCallerArn": SNS_CALLER_ARN, "ExternalId": "tenant" },
        }),
    )
    .await;
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
    let user_attributes: Vec<Value> = attributes
        .iter()
        .map(|(attribute, value)| json!({ "Name": attribute, "Value": value }))
        .collect();
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": name,
            "MessageAction": "SUPPRESS",
            "UserAttributes": user_attributes,
        }),
    )
    .await;
    // A self-service reset replaces the password the user OWNS, so the user is given a
    // permanent one first.
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": name,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    (user_pool_id, client_id)
}

/// The medium a code sent to `attribute` travels over: the `DeliveryMediumType` value for
/// that attribute's channel.
fn medium_of(attribute: &str) -> &'static str {
    if attribute == "phone_number" {
        "SMS"
    } else {
        "EMAIL"
    }
}

/// Assert `answered` reports a code sent to `expected`, with the medium and the masked value
/// of what `attributes` holds for it.
fn assert_delivered(case: &str, answered: &Answer, expected: &str, attributes: &[(&str, &str)]) {
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected a reported delivery, got {} with {}",
        answered.status,
        answered.payload
    );
    let details = &answered.payload["CodeDeliveryDetails"];
    assert_eq!(
        details["AttributeName"].as_str(),
        Some(expected),
        "case {case:?}: expected the code sent to {expected:?}, got {details}"
    );
    let medium = medium_of(expected);
    assert_eq!(
        details["DeliveryMedium"].as_str(),
        Some(medium),
        "case {case:?}: expected DeliveryMedium {medium:?}, got {details}"
    );
    // The destination is the masked form of the value the CHOSEN attribute holds, so the
    // three reported members cannot disagree about which contact was used.
    let address = attributes
        .iter()
        .find(|(attribute, _)| *attribute == expected)
        .map(|(_, value)| *value)
        .unwrap_or_default();
    let expected_destination = masked_destination(address);
    assert_eq!(
        details["Destination"].as_str(),
        Some(expected_destination.as_str()),
        "case {case:?}: expected the masked form {expected_destination:?} of {address:?}, got \
         {details}"
    );
}

/// Assert `answered` is the refusal owed a user with no verified contact value.
fn assert_refused(case: &str, answered: &Answer) {
    assert_eq!(
        answered.status,
        400,
        "case {case:?}: a user holding nothing verified must be refused, got {} with {}",
        answered.status,
        answered.payload
    );
    assert_eq!(
        answered.code(),
        "InvalidParameterException",
        "case {case:?}: got {}",
        answered.payload
    );
    assert!(
        answered.error_type.starts_with("InvalidParameterException"),
        "case {case:?}: the refusal must name its error type on the wire, got {:?}",
        answered.error_type
    );
    assert_eq!(
        answered.message(),
        NO_RECOVERY_DESTINATION,
        "case {case:?}: got {}",
        answered.payload
    );
}

/// One row: the case, the mechanisms the pool ranks, the attributes the user holds, and the
/// attribute a reset code must go to — `""` for the refusal.
struct Row {
    case: &'static str,
    mechanisms: Value,
    attributes: Vec<(&'static str, &'static str)>,
    expected: &'static str,
}

fn rows() -> Vec<Row> {
    let phone_first = json!([
        { "Name": "verified_phone_number", "Priority": 1 },
        { "Name": "verified_email", "Priority": 2 },
    ]);
    let email_first = json!([
        { "Name": "verified_email", "Priority": 1 },
        { "Name": "verified_phone_number", "Priority": 2 },
    ]);
    vec![
        Row {
            // The pool prefers the number, but the number this user holds is not verified:
            // the mechanism has nothing to deliver to, so the pool falls to the email it
            // ranks next.
            case: "the phone mechanism first, an unverified number and a verified email",
            mechanisms: phone_first.clone(),
            attributes: vec![
                ("email", "falls.to.email@example.test"),
                ("email_verified", "true"),
                ("phone_number", "+15550100100"),
                ("phone_number_verified", "false"),
            ],
            expected: "email",
        },
        Row {
            // A value carrying NO verified flag at all is in the same state as one whose
            // flag says it was never confirmed.
            case: "the phone mechanism first, a number with no verified flag and a verified email",
            mechanisms: phone_first.clone(),
            attributes: vec![
                ("email", "no.flag@example.test"),
                ("email_verified", "true"),
                ("phone_number", "+15550100101"),
            ],
            expected: "email",
        },
        Row {
            case: "the phone mechanism first, both values verified",
            mechanisms: phone_first.clone(),
            attributes: vec![
                ("email", "both@example.test"),
                ("email_verified", "true"),
                ("phone_number", "+15550100102"),
                ("phone_number_verified", "true"),
            ],
            expected: "phone_number",
        },
        Row {
            // The reverse of the first row: the preferred mechanism's value is unverified,
            // so the pool falls to the other one — which here is the number.
            case: "the email mechanism first, an unverified email and a verified number",
            mechanisms: email_first.clone(),
            attributes: vec![
                ("email", "unverified@example.test"),
                ("email_verified", "false"),
                ("phone_number", "+15550100103"),
                ("phone_number_verified", "true"),
            ],
            expected: "phone_number",
        },
        Row {
            case: "the email mechanism first, both values verified",
            mechanisms: email_first.clone(),
            attributes: vec![
                ("email", "email.first@example.test"),
                ("email_verified", "true"),
                ("phone_number", "+15550100104"),
                ("phone_number_verified", "true"),
            ],
            expected: "email",
        },
        Row {
            // Both contacts are registered and NEITHER is verified: there is no channel at
            // all, whatever the pool ranks.
            case: "the phone mechanism first, both values held unverified",
            mechanisms: phone_first.clone(),
            attributes: vec![
                ("email", "nothing.verified@example.test"),
                ("email_verified", "false"),
                ("phone_number", "+15550100105"),
                ("phone_number_verified", "false"),
            ],
            expected: "",
        },
        Row {
            case: "the email mechanism first, both values held with no verified flag",
            mechanisms: email_first.clone(),
            attributes: vec![
                ("email", "no.flags@example.test"),
                ("phone_number", "+15550100106"),
            ],
            expected: "",
        },
        Row {
            // A user holding neither contact value is the same refusal: a flag cannot make a
            // channel out of an attribute with no value behind it.
            case: "the phone mechanism first and no contact value at all",
            mechanisms: phone_first.clone(),
            attributes: vec![("given_name", "Nobody")],
            expected: "",
        },
        Row {
            case: "the phone mechanism first, a verified number and an unverified email",
            mechanisms: phone_first.clone(),
            attributes: vec![
                ("email", "unverified.too@example.test"),
                ("email_verified", "false"),
                ("phone_number", "+15550100107"),
                ("phone_number_verified", "true"),
            ],
            expected: "phone_number",
        },
        Row {
            case: "the email mechanism first, a verified email alone",
            mechanisms: email_first.clone(),
            attributes: vec![
                ("email", "only.email@example.test"),
                ("email_verified", "true"),
            ],
            expected: "email",
        },
    ]
}

#[tokio::test]
async fn a_reset_channel_is_chosen_among_the_values_the_user_holds_verified() {
    let router = router();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let name = format!("verified-state-{index}");
        let (_, client_id) = pool_with_user(&router, &name, &row.mechanisms, &row.attributes).await;

        let answered = call(
            &router,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": name }),
        )
        .await;

        if row.expected.is_empty() {
            assert_refused(case, &answered);
        } else {
            assert_delivered(case, &answered, row.expected, &row.attributes);
        }
    }
}

/// The answer tracks the verification state as it CHANGES: the same request is refused while
/// nothing is verified, falls to the email once that address is confirmed, and moves to the
/// pool's preferred number once that is confirmed too.
#[tokio::test]
async fn verifying_a_contact_attribute_moves_the_reset_channel_to_it() {
    let router = router();
    let name = "verification-ladder";
    let attributes = [
        ("email", "ladder@example.test"),
        ("email_verified", "false"),
        ("phone_number", "+15550100110"),
        ("phone_number_verified", "false"),
    ];
    // The pool PREFERS the number, so the email is only ever chosen because the number is
    // not a verified value yet.
    let mechanisms = json!([
        { "Name": "verified_phone_number", "Priority": 1 },
        { "Name": "verified_email", "Priority": 2 },
    ]);
    let (user_pool_id, client_id) = pool_with_user(&router, name, &mechanisms, &attributes).await;
    let reset = json!({ "ClientId": client_id, "Username": name });

    assert_refused(
        "both contacts registered, neither verified",
        &call(&router, "ForgotPassword", &reset).await,
    );

    // Confirming the email makes it the one verified value, so the preferred mechanism
    // still has nothing and the reset falls to the email.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": name,
            "UserAttributes": [{ "Name": "email_verified", "Value": "true" }],
        }),
    )
    .await;
    assert_delivered(
        "the email confirmed, the number still unverified",
        &call(&router, "ForgotPassword", &reset).await,
        "email",
        &attributes,
    );

    // Confirming the number too hands the channel to the mechanism the pool ranks first.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": name,
            "UserAttributes": [{ "Name": "phone_number_verified", "Value": "true" }],
        }),
    )
    .await;
    assert_delivered(
        "both confirmed, so the preferred mechanism is used",
        &call(&router, "ForgotPassword", &reset).await,
        "phone_number",
        &attributes,
    );

    // And withdrawing both flags leaves the account with no channel again, which is the
    // refusal — the state, not the history, decides.
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": name,
            "UserAttributes": [
                { "Name": "email_verified", "Value": "false" },
                { "Name": "phone_number_verified", "Value": "false" },
            ],
        }),
    )
    .await;
    assert_refused(
        "both flags withdrawn again",
        &call(&router, "ForgotPassword", &reset).await,
    );
}
