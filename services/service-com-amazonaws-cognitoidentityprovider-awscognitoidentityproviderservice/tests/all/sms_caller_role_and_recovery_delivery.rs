// SPDX-License-Identifier: CC0-1.0

//! The SMS delivery path a user pool is held to, over the awsJson1_1 wire.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than on one request:
//!
//! * RULE A — a write that configures Amazon SNS delivery (`CreateUserPool`,
//!   `UpdateUserPool`, `SetUserPoolMfaConfig`) is served only where Amazon
//!   Cognito can take the caller role on: the value has to name an IAM role of
//!   this account, this account's IAM has to hold it, and the trust policy it
//!   carries has to grant `cognito-idp.amazonaws.com` the assume this
//!   configuration would make — which presents the `ExternalId` the
//!   configuration carries. Every other answer is
//!   `InvalidSmsRoleTrustRelationshipException` (HTTP 400) with nothing stored.
//!   Where there is no IAM to ask, nothing is known about a role of this account
//!   and the write is served, which the second test pins.
//! * RULE B — an SMS MFA factor needs a caller to send through: a
//!   `SmsMfaConfiguration` that names none, while the pool's MFA is on or
//!   optional, is `InvalidParameterException` (HTTP 400).
//! * RULE C — a reset code goes only over a medium the pool can SEND on: the
//!   `verified_phone_number` recovery mechanism qualifies only while the pool
//!   stores the SMS configuration a text message leaves through, so a pool that
//!   stores none falls through to the mechanism behind it and refuses where
//!   nothing else qualifies.
//!
//! The roles and their trust documents are IAM's OWN records, written through
//! IAM's own `CreateRole` over the same engine, so the documents this service
//! reads are the ones IAM wrote.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use crate::iam_authorization::{form_encode, xml_element};

/// The Region every call below is made in.
const REGION: &str = "us-east-1";

/// A request's SigV4 credential scope is what routes it to this service. Nothing
/// is signed here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0000";

/// The service principal a user pool sends SMS messages as.
const COGNITO_PRINCIPAL: &str = "cognito-idp.amazonaws.com";

/// The sentence a write is refused with when Amazon Cognito cannot assume the
/// Amazon SNS caller role the configuration names.
const SMS_ROLE_TRUST_RELATIONSHIP: &str =
    "Role does not have a trust relationship allowing Cognito to assume the role";

/// The sentence `SetUserPoolMfaConfig` is refused with when its SMS factor names
/// no caller to send through.
const SMS_MFA_CONFIGURATION_INCOMPLETE: &str =
    "Invalid SMS MFA configuration given, external id and a valid caller arn are required.";

/// The sentence a reset is refused with when the user holds no contact a code can
/// be delivered to.
const NO_RECOVERY_DESTINATION: &str =
    "Cannot reset password for the user as there is no registered/verified email or phone_number";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// This service alone, on its own engine: there is no IAM to ask about any role.
fn router_without_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// This service over the SAME engine as IAM, so a caller role resolves in the
/// service that owns it — exactly what `core/router` assembles.
fn router_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` an error body carries.
fn message_of(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The value at a dotted path of `body`, or `None` when the path is not there.
fn at<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = body;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

/// Create one role through IAM's own `CreateRole` and answer the ARN IAM
/// persisted for it.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    path: &str,
    trust: &Value,
) -> String {
    let mut body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(name),
        form_encode(&trust.to_string()),
    );
    if !path.is_empty() {
        body.push_str(&format!("&Path={}", form_encode(path)));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be created in IAM first, got {status} {xml}"
    );
    xml_element(&xml, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole answers the role's Arn, got {xml}"))
}

/// A trust policy with one statement: `effect` on `sts:AssumeRole` for
/// `principal`, narrowed to `external_id` when one is given.
fn trust(effect: &str, principal: &str, external_id: Option<&str>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": { "Service": principal },
        "Action": "sts:AssumeRole",
    });
    if let Some(external_id) = external_id {
        statement["Condition"] = json!({ "StringEquals": { "sts:ExternalId": external_id } });
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// A role ARN of this account under `resource` (`role/<name>`, `user/<name>`, …).
fn account_arn(vendor: &str, region: &str, resource: &str) -> String {
    format!(
        "arn:aws:{vendor}:{region}:{}:{resource}",
        ::engine::emulator_account_id()
    )
}

/// What a row says the `SnsCallerArn` is.
enum Caller {
    /// This account's IAM holds the role, under `path`, with this trust policy.
    Held { path: &'static str, trust: Value },
    /// A well-formed role ARN of this account that IAM holds nothing under.
    AbsentFromIam,
    /// A value spelled out by the row: a resource that is no IAM role, or a role
    /// of another account.
    Literal(String),
}

/// Create a plain pool and answer its id.
async fn plain_pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The stored pool, as `DescribeUserPool` reports it.
async fn described(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// RULE A, over the three writes that configure Amazon SNS delivery: the write is
/// served only where IAM answers that Amazon Cognito can make the assume this
/// configuration would make, and a refusal stores nothing.
#[tokio::test]
async fn an_sms_configuration_is_served_only_for_a_caller_role_cognito_can_assume() {
    let router = router_with_iam();

    // (case, the caller the configuration names, the ExternalId it presents,
    //  whether the write is served)
    let rows: Vec<(&str, Caller, Option<&str>, bool)> = vec![
        (
            "a held role whose trust policy admits Amazon Cognito",
            Caller::Held {
                path: "",
                trust: trust("Allow", COGNITO_PRINCIPAL, None),
            },
            None,
            true,
        ),
        (
            "the same role, with an external id the trust policy does not ask for",
            Caller::Held {
                path: "",
                trust: trust("Allow", COGNITO_PRINCIPAL, None),
            },
            Some("tenant-one"),
            true,
        ),
        (
            "a held role under a path, whose trust policy admits Amazon Cognito",
            Caller::Held {
                path: "/service-role/",
                trust: trust("Allow", COGNITO_PRINCIPAL, None),
            },
            None,
            true,
        ),
        (
            "a held role that requires the external id the configuration presents",
            Caller::Held {
                path: "",
                trust: trust("Allow", COGNITO_PRINCIPAL, Some("tenant-two")),
            },
            Some("tenant-two"),
            true,
        ),
        (
            "a held role that requires an external id the configuration presents none for",
            Caller::Held {
                path: "",
                trust: trust("Allow", COGNITO_PRINCIPAL, Some("tenant-three")),
            },
            None,
            false,
        ),
        (
            "a held role that requires another external id than the one presented",
            Caller::Held {
                path: "",
                trust: trust("Allow", COGNITO_PRINCIPAL, Some("tenant-four")),
            },
            Some("tenant-five"),
            false,
        ),
        (
            "a held role whose trust policy admits another service",
            Caller::Held {
                path: "",
                trust: trust("Allow", "sns.amazonaws.com", None),
            },
            None,
            false,
        ),
        (
            "a held role whose trust policy DENIES Amazon Cognito",
            Caller::Held {
                path: "",
                trust: trust("Deny", COGNITO_PRINCIPAL, None),
            },
            None,
            false,
        ),
        (
            "a role this account's IAM holds nothing under",
            Caller::AbsentFromIam,
            None,
            false,
        ),
        (
            "a value naming an IAM user rather than a role",
            Caller::Literal(account_arn("iam", "", "user/sms-caller")),
            None,
            false,
        ),
        (
            "a value naming no IAM resource at all",
            Caller::Literal(account_arn("sns", REGION, "sms-topic")),
            None,
            false,
        ),
        (
            "a role ARN of another account",
            Caller::Literal("arn:aws:iam::123456789012:role/sms-caller".to_string()),
            None,
            false,
        ),
    ];

    for (index, (case, caller, external_id, served)) in rows.iter().enumerate() {
        let role_name = format!("sms-caller-{index}");
        let role_arn = match caller {
            Caller::Held { path, trust } => {
                given_role(&router, case, &role_name, path, trust).await
            }
            Caller::AbsentFromIam => account_arn("iam", "", &format!("role/{role_name}")),
            Caller::Literal(value) => value.clone(),
        };
        let mut sms_configuration = json!({ "SnsCallerArn": role_arn });
        if let Some(external_id) = external_id {
            sms_configuration["ExternalId"] = json!(external_id);
        }

        // The create path.
        let (status, payload) = call(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": format!("created-{index}"),
                "SmsConfiguration": sms_configuration,
            }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: CreateUserPool, got {status} with {payload}"
        );
        if *served {
            let stored = at(&payload, "UserPool.SmsConfiguration.SnsCallerArn");
            assert_eq!(
                stored,
                Some(&json!(role_arn)),
                "case {case:?}: the created pool carries the caller it was created with, \
                 got {payload}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                "InvalidSmsRoleTrustRelationshipException",
                "case {case:?}: CreateUserPool, got {payload}"
            );
            assert_eq!(
                message_of(&payload),
                SMS_ROLE_TRUST_RELATIONSHIP,
                "case {case:?}: CreateUserPool, got {payload}"
            );
        }

        // The update path, on a pool that carries no SMS configuration yet.
        let user_pool_id = plain_pool(&router, &format!("updated-{index}")).await;
        let (status, payload) = call(
            &router,
            "UpdateUserPool",
            &json!({
                "UserPoolId": user_pool_id,
                "SmsConfiguration": sms_configuration,
            }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: UpdateUserPool, got {status} with {payload}"
        );
        let read_back = described(&router, &user_pool_id).await;
        let stored = at(&read_back, "SmsConfiguration.SnsCallerArn");
        if *served {
            assert_eq!(
                stored,
                Some(&json!(role_arn)),
                "case {case:?}: the updated pool reports the caller it was given, \
                 got {read_back}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                "InvalidSmsRoleTrustRelationshipException",
                "case {case:?}: UpdateUserPool, got {payload}"
            );
            assert_eq!(
                message_of(&payload),
                SMS_ROLE_TRUST_RELATIONSHIP,
                "case {case:?}: UpdateUserPool, got {payload}"
            );
            assert_eq!(
                stored, None,
                "case {case:?}: a refused update stores no SMS configuration, got {read_back}"
            );
        }

        // The MFA-configuration path, which carries the same configuration.
        let (status, payload) = call(
            &router,
            "SetUserPoolMfaConfig",
            &json!({
                "UserPoolId": user_pool_id,
                "MfaConfiguration": "OPTIONAL",
                "SmsMfaConfiguration": { "SmsConfiguration": sms_configuration },
            }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: SetUserPoolMfaConfig, got {status} with {payload}"
        );
        let mfa = ok(
            &router,
            "GetUserPoolMfaConfig",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        if *served {
            assert_eq!(
                at(&mfa, "SmsMfaConfiguration.SmsConfiguration.SnsCallerArn"),
                Some(&json!(role_arn)),
                "case {case:?}: the stored MFA configuration reports the caller, got {mfa}"
            );
            assert_eq!(
                mfa.get("MfaConfiguration"),
                Some(&json!("OPTIONAL")),
                "case {case:?}: the stored MFA mode is the one set, got {mfa}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                "InvalidSmsRoleTrustRelationshipException",
                "case {case:?}: SetUserPoolMfaConfig, got {payload}"
            );
            assert_eq!(
                message_of(&payload),
                SMS_ROLE_TRUST_RELATIONSHIP,
                "case {case:?}: SetUserPoolMfaConfig, got {payload}"
            );
            assert_eq!(
                mfa.get("MfaConfiguration"),
                Some(&json!("OFF")),
                "case {case:?}: a refused MFA call changes nothing, got {mfa}"
            );
        }
    }
}

/// RULE A's complement: with no IAM to ask, nothing is KNOWN about a role of this
/// account, so a write naming one is served — while a value that names no IAM
/// role of this account is refused there too, because that is decided on the
/// request alone.
#[tokio::test]
async fn a_caller_role_nothing_is_known_about_is_served() {
    let router = router_without_iam();

    // (case, the value the configuration names, whether the write is served)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a well-formed role ARN of this account",
            account_arn("iam", "", "role/provisioned-out-of-band"),
            true,
        ),
        (
            "a well-formed role ARN of this account under a path",
            account_arn("iam", "", "role/service-role/provisioned-out-of-band"),
            true,
        ),
        (
            "a value naming no IAM resource at all",
            account_arn("sns", REGION, "sms-topic"),
            false,
        ),
        (
            "a role ARN of another account",
            "arn:aws:iam::123456789012:role/sms-caller".to_string(),
            false,
        ),
    ];

    for (index, (case, role_arn, served)) in rows.iter().enumerate() {
        let (status, payload) = call(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": format!("no-iam-{index}"),
                "SmsConfiguration": { "SnsCallerArn": role_arn },
            }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: got {status} with {payload}"
        );
        if !*served {
            assert_eq!(
                error_code(&payload),
                "InvalidSmsRoleTrustRelationshipException",
                "case {case:?}: got {payload}"
            );
        }
    }
}

/// RULE B: an SMS MFA factor is stored only with a caller to send through, and
/// the refusal is the same sentence whichever way the configuration names none.
#[tokio::test]
async fn an_sms_mfa_factor_needs_a_caller_to_send_through() {
    let router = router_without_iam();
    let caller = account_arn("iam", "", "role/sms-caller");

    // (case, the SmsMfaConfiguration sent, whether the call is served)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "an SMS factor carrying no SmsConfiguration at all",
            json!({ "SmsAuthenticationMessage": "Your code is {####}" }),
            false,
        ),
        (
            "an SmsConfiguration that names no caller",
            json!({ "SmsConfiguration": {} }),
            false,
        ),
        (
            "an SmsConfiguration whose caller is the member's own empty default",
            json!({ "SmsConfiguration": { "SnsCallerArn": "" } }),
            false,
        ),
        (
            "an SmsConfiguration carrying an external id but no caller",
            json!({ "SmsConfiguration": { "ExternalId": "tenant-one" } }),
            false,
        ),
        (
            "an SmsConfiguration naming an Amazon SNS caller",
            json!({ "SmsConfiguration": { "SnsCallerArn": caller.clone() } }),
            true,
        ),
        (
            "an SmsConfiguration naming a caller and an external id",
            json!({
                "SmsConfiguration": { "SnsCallerArn": caller.clone(), "ExternalId": "tenant-two" }
            }),
            true,
        ),
    ];

    // The two modes that offer MFA hold the factor to the rule; the mode that
    // offers none stores what it is sent.
    for mode in ["ON", "OPTIONAL"] {
        for (index, (case, configuration, served)) in rows.iter().enumerate() {
            let user_pool_id = plain_pool(&router, &format!("mfa-{mode}-{index}")).await;
            let (status, payload) = call(
                &router,
                "SetUserPoolMfaConfig",
                &json!({
                    "UserPoolId": user_pool_id,
                    "MfaConfiguration": mode,
                    "SmsMfaConfiguration": configuration,
                }),
            )
            .await;
            assert_eq!(
                status,
                if *served { 200 } else { 400 },
                "case {case:?} with MFA {mode}: got {status} with {payload}"
            );
            let mfa = ok(
                &router,
                "GetUserPoolMfaConfig",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await;
            if *served {
                assert_eq!(
                    mfa.get("MfaConfiguration"),
                    Some(&json!(mode)),
                    "case {case:?} with MFA {mode}: the mode is stored, got {mfa}"
                );
            } else {
                assert_eq!(
                    error_code(&payload),
                    "InvalidParameterException",
                    "case {case:?} with MFA {mode}: got {payload}"
                );
                assert_eq!(
                    message_of(&payload),
                    SMS_MFA_CONFIGURATION_INCOMPLETE,
                    "case {case:?} with MFA {mode}: got {payload}"
                );
                assert_eq!(
                    mfa.get("MfaConfiguration"),
                    Some(&json!("OFF")),
                    "case {case:?} with MFA {mode}: a refused call changes nothing, got {mfa}"
                );
            }
        }
    }
}

/// RULE C: `ForgotPassword` delivers over a medium the pool can send on, so the
/// `verified_phone_number` mechanism qualifies only while the pool stores the SMS
/// configuration a text message leaves through.
#[tokio::test]
async fn a_reset_code_goes_only_over_a_medium_the_pool_can_send_on() {
    let router = router_without_iam();
    let sends_sms =
        json!({ "SnsCallerArn": account_arn("iam", "", "role/sms-caller"), "ExternalId": "tenant" });
    let phone_first = json!([
        { "Name": "verified_phone_number", "Priority": 1 },
        { "Name": "verified_email", "Priority": 2 },
    ]);
    let phone_only = json!([{ "Name": "verified_phone_number", "Priority": 1 }]);

    // (case, the pool's recovery mechanisms, the pool's SmsConfiguration — `null`
    //  for a pool that stores none, the user's attributes, the attribute the code
    //  goes to or "" for the refusal)
    type Row = (
        &'static str,
        Value,
        Value,
        Vec<(&'static str, &'static str)>,
        &'static str,
    );
    let rows: Vec<Row> = vec![
        (
            "a verified number, ranked first, on a pool that sends SMS",
            phone_first.clone(),
            sends_sms.clone(),
            vec![
                ("email", "a@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100001"),
                ("phone_number_verified", "true"),
            ],
            "phone_number",
        ),
        (
            "the same user and ranking, on a pool that sends no SMS",
            phone_first.clone(),
            Value::Null,
            vec![
                ("email", "b@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100002"),
                ("phone_number_verified", "true"),
            ],
            "email",
        ),
        (
            "a verified number as the only contact, on a pool that sends SMS",
            phone_only.clone(),
            sends_sms.clone(),
            vec![
                ("phone_number", "+15550100003"),
                ("phone_number_verified", "true"),
            ],
            "phone_number",
        ),
        (
            "a verified number as the only contact, on a pool that sends no SMS",
            phone_only.clone(),
            Value::Null,
            vec![
                ("phone_number", "+15550100004"),
                ("phone_number_verified", "true"),
            ],
            "",
        ),
        (
            // A stored configuration that names NOBODY to send through is a pool
            // with no SMS delivery path, exactly as a pool with no configuration
            // at all: the two readings are one
            // (`crate::state::names_an_sms_caller`).
            "a verified number alone, on a pool whose SMS configuration names no caller",
            phone_only.clone(),
            json!({ "SnsCallerArn": "" }),
            vec![
                ("phone_number", "+15550100007"),
                ("phone_number_verified", "true"),
            ],
            "",
        ),
        (
            "the phone mechanism alone and a verified email the pool may not use",
            phone_only.clone(),
            Value::Null,
            vec![
                ("email", "e@example.com"),
                ("email_verified", "true"),
                ("phone_number", "+15550100005"),
                ("phone_number_verified", "true"),
            ],
            "",
        ),
        (
            "an unverified number on a pool that sends SMS",
            phone_first.clone(),
            sends_sms.clone(),
            vec![("phone_number", "+15550100006")],
            "",
        ),
    ];

    for (index, (case, mechanisms, sms_configuration, attributes, expected)) in
        rows.iter().enumerate()
    {
        let mut request = json!({
            "PoolName": format!("recovery-{index}"),
            "AccountRecoverySetting": { "RecoveryMechanisms": mechanisms },
        });
        if !sms_configuration.is_null() {
            request["SmsConfiguration"] = sms_configuration.clone();
        }
        let created = ok(&router, "CreateUserPool", &request).await;
        let user_pool_id = created["UserPool"]["Id"]
            .as_str()
            .expect("a minted pool id")
            .to_string();
        let client = ok(
            &router,
            "CreateUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientName": "app" }),
        )
        .await;
        let client_id = client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string();

        let username = format!("resettable-{index}");
        let user_attributes: Vec<Value> = attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect();
        ok(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "UserAttributes": user_attributes,
            }),
        )
        .await;
        // A self-service reset applies to the password the user OWNS, so the
        // user is given a permanent one first.
        ok(
            &router,
            "AdminSetUserPassword",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "Password": GOOD_PASSWORD,
                "Permanent": true,
            }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "ForgotPassword",
            &json!({ "ClientId": client_id, "Username": username }),
        )
        .await;
        if expected.is_empty() {
            assert_eq!(
                status, 400,
                "case {case:?}: a pool with nothing to deliver over refuses, got {status} \
                 with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidParameterException",
                "case {case:?}: got {payload}"
            );
            assert_eq!(
                message_of(&payload),
                NO_RECOVERY_DESTINATION,
                "case {case:?}: got {payload}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected a delivery, got {status} with {payload}"
        );
        let details = &payload["CodeDeliveryDetails"];
        assert_eq!(
            details.get("AttributeName"),
            Some(&json!(expected)),
            "case {case:?}: expected the code sent to {expected}, got {details}"
        );
        let medium = if *expected == "phone_number" {
            "SMS"
        } else {
            "EMAIL"
        };
        assert_eq!(
            details.get("DeliveryMedium"),
            Some(&json!(medium)),
            "case {case:?}: expected DeliveryMedium {medium}, got {details}"
        );
    }
}

/// An `AccountRecoverySetting` that combines `admin_only` with a mechanism that
/// DOES deliver a message is refused by both writes, and stores nothing:
/// `admin_only` "prevents self-service account recovery", so it is not one option
/// among others.
#[tokio::test]
async fn admin_only_recovery_cannot_be_combined_with_another_mechanism() {
    let router = router_without_iam();

    // (case, the RecoveryMechanisms sent, whether the write is served)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "admin_only alone",
            json!([{ "Name": "admin_only", "Priority": 1 }]),
            true,
        ),
        (
            "admin_only ahead of the verified email",
            json!([
                { "Name": "admin_only", "Priority": 1 },
                { "Name": "verified_email", "Priority": 2 },
            ]),
            false,
        ),
        (
            "admin_only behind the verified email",
            json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "admin_only", "Priority": 2 },
            ]),
            false,
        ),
        (
            "admin_only beside the verified phone number",
            json!([
                { "Name": "verified_phone_number", "Priority": 1 },
                { "Name": "admin_only", "Priority": 2 },
            ]),
            false,
        ),
        (
            "the two mechanisms that deliver a message",
            json!([
                { "Name": "verified_email", "Priority": 1 },
                { "Name": "verified_phone_number", "Priority": 2 },
            ]),
            true,
        ),
    ];

    for (index, (case, mechanisms, served)) in rows.iter().enumerate() {
        let setting = json!({ "RecoveryMechanisms": mechanisms });
        let (status, payload) = call(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": format!("recovery-create-{index}"),
                "AccountRecoverySetting": setting,
            }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: CreateUserPool, got {status} with {payload}"
        );
        if *served {
            assert_eq!(
                at(&payload, "UserPool.AccountRecoverySetting.RecoveryMechanisms"),
                Some(mechanisms),
                "case {case:?}: the created pool reports the setting it was given, got {payload}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                "InvalidParameterException",
                "case {case:?}: CreateUserPool, got {payload}"
            );
            assert_eq!(
                message_of(&payload),
                "Invalid account recovery setting parameter. Account Recovery Setting cannot \
                 use admin_only setting with any other recovery mechanisms.",
                "case {case:?}: CreateUserPool, got {payload}"
            );
        }

        // The update path, on a pool whose stored setting the refusal must leave
        // exactly as it was.
        let kept = json!([{ "Name": "verified_email", "Priority": 1 }]);
        let created = ok(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": format!("recovery-update-{index}"),
                "AccountRecoverySetting": { "RecoveryMechanisms": kept },
            }),
        )
        .await;
        let user_pool_id = created["UserPool"]["Id"]
            .as_str()
            .expect("a minted pool id")
            .to_string();
        let (status, payload) = call(
            &router,
            "UpdateUserPool",
            &json!({ "UserPoolId": user_pool_id, "AccountRecoverySetting": setting }),
        )
        .await;
        assert_eq!(
            status,
            if *served { 200 } else { 400 },
            "case {case:?}: UpdateUserPool, got {status} with {payload}"
        );
        let read_back = described(&router, &user_pool_id).await;
        let stored = at(&read_back, "AccountRecoverySetting.RecoveryMechanisms");
        if *served {
            assert_eq!(
                stored,
                Some(mechanisms),
                "case {case:?}: the updated pool reports the setting it was given, \
                 got {read_back}"
            );
        } else {
            assert_eq!(
                error_code(&payload),
                "InvalidParameterException",
                "case {case:?}: UpdateUserPool, got {payload}"
            );
            assert_eq!(
                stored,
                Some(&kept),
                "case {case:?}: a refused update leaves the stored setting alone, \
                 got {read_back}"
            );
        }
    }
}
