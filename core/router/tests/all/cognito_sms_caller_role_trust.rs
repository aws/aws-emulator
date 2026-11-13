// SPDX-License-Identifier: CC0-1.0

//! Cognito's SMS-caller-role check, across the two services it spans.
//!
//! RULE: a user pool sends SMS messages by assuming the IAM role its
//! `SmsConfiguration` names — "to send SMS messages with Amazon SNS in the Amazon
//! Web Services Region that you want, the Amazon Cognito user pool uses an
//! Identity and Access Management (IAM) role in your Amazon Web Services account"
//! (`SmsConfigurationType` in Cognito's model) — so a write that names a role
//! whose trust policy withholds `cognito-idp.amazonaws.com` is refused with
//! `InvalidSmsRoleTrustRelationshipException` rather than stored as a delivery
//! path that could never send. The role is resolved against the ACCOUNT's IAM
//! records, which is why this suite lives here: the role is created through the
//! real `CreateRole` on the same engine that serves Cognito, so what the check
//! reads is the document IAM persisted and not a copy.
//!
//! The rows differ along the form of the trust policy (which service principal it
//! names, whether it names several, whether the grant is an `Allow`, which action
//! it names) and along the operation that writes the configuration —
//! `CreateUserPool`, `UpdateUserPool` and `SetUserPoolMfaConfig` all carry an
//! `SmsConfiguration`. The second suite is the other half of the same rule: the
//! check is the ASSUME, so an `SnsCallerArn` this account's IAM holds no role for
//! is a role Amazon Cognito cannot take on either, and such a write is refused
//! with the same error — which is what the recorded wire corpus shows AWS doing
//! for a configuration naming a role that was never created
//! (`cognito-idp-acr-traj-enable-phone-recovery-update-mistakes#task#5`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

// The percent-encoder this suite needs is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns: an
// `application/x-www-form-urlencoded` component is escaped exactly as a URI path
// segment is, so the codec is shared rather than written twice.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The local name of Cognito's Smithy shape, which is the `X-Amz-Target` prefix.
const COGNITO_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The service principal a user pool assumes its SMS caller role as.
const COGNITO_PRINCIPAL: &str = "cognito-idp.amazonaws.com";

/// The sentence a write naming a role Cognito cannot assume is refused with.
const TRUST_REFUSAL_MESSAGE: &str =
    "Role does not have a trust relationship allowing Cognito to assume the role";

/// Cognito and IAM over ONE shared engine, as `router/src/lib.rs` mounts them, so
/// Cognito's in-process callback reaches this very IAM.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// The text of the first `<tag>` element of an XML body.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"))
        + start;
    body[start..end].to_string()
}

/// One Cognito call: `awsJson1_1`, addressed by `X-Amz-Target`.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{COGNITO_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cognito-idp/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A Cognito call that must succeed, with its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A trust policy naming `principals` in one `Allow` statement.
fn trust_allowing(principals: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principals },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// Create a role carrying `document` and return its ARN.
async fn role(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole must persist {name}, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// One row: a trust policy, and whether a pool write naming the role it is on is
/// served.
struct Row {
    case: &'static str,
    document: String,
    served: bool,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "cognito-idp alone",
            document: trust_allowing(&[COGNITO_PRINCIPAL]),
            served: true,
        },
        Row {
            case: "cognito-idp among several services",
            document: trust_allowing(&["sns.amazonaws.com", COGNITO_PRINCIPAL]),
            served: true,
        },
        Row {
            case: "cognito-idp named as a single string rather than a list",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": COGNITO_PRINCIPAL },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            served: true,
        },
        Row {
            case: "cognito-idp allowed through an sts wildcard",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": COGNITO_PRINCIPAL },
                    "Action": "sts:*"
                }]
            })
            .to_string(),
            served: true,
        },
        Row {
            case: "another service only",
            document: trust_allowing(&["sns.amazonaws.com"]),
            served: false,
        },
        Row {
            case: "cognito-idp named by a statement that denies it",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "Service": COGNITO_PRINCIPAL },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            served: false,
        },
        Row {
            case: "cognito-idp allowed an action that is not assuming the role",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": COGNITO_PRINCIPAL },
                    "Action": "s3:GetObject"
                }]
            })
            .to_string(),
            served: false,
        },
    ]
}

/// Assert `payload` refuses the write with the SMS-role trust refusal.
fn assert_trust_refusal(case: &str, operation: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must refuse the role, got {status} with {payload}"
    );
    assert_eq!(
        payload["__type"].as_str(),
        Some("InvalidSmsRoleTrustRelationshipException"),
        "case {case:?}: {operation} must name the trust-relationship error, got {payload}"
    );
    assert_eq!(
        payload["message"].as_str(),
        Some(TRUST_REFUSAL_MESSAGE),
        "case {case:?}: {operation} must carry the trust refusal sentence, got {payload}"
    );
}

#[tokio::test]
async fn a_pool_write_refuses_an_sms_caller_role_whose_trust_policy_withholds_cognito() {
    let router = router();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let role_arn = role(&router, &format!("sms-caller-{index}"), &row.document).await;
        let sms_configuration = json!({ "SnsCallerArn": role_arn });

        // CreateUserPool: a refused create stores no pool.
        let pool_name = format!("sms-role-create-{index}");
        let (status, payload) = cognito(
            &router,
            "CreateUserPool",
            &json!({ "PoolName": pool_name, "SmsConfiguration": sms_configuration }),
        )
        .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateUserPool must serve the role, got {status} with {payload}"
            );
            let user_pool_id = payload["UserPool"]["Id"].as_str().expect("a minted id");
            let stored = ok(
                &router,
                "DescribeUserPool",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await["UserPool"]
                .clone();
            assert_eq!(
                stored["SmsConfiguration"]["SnsCallerArn"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the stored pool carries the SNS caller it was created with, got \
                 {stored}"
            );
        } else {
            assert_trust_refusal(case, "CreateUserPool", status, &payload);
            let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            let named = listed["UserPools"]
                .as_array()
                .expect("a listing of pools")
                .iter()
                .any(|entry| entry["Name"] == json!(pool_name));
            assert!(
                !named,
                "case {case:?}: a refused create stores no pool, got {listed}"
            );
        }

        // UpdateUserPool and SetUserPoolMfaConfig, against a pool that holds no
        // SMS configuration of its own: a refused write leaves it holding none.
        let user_pool_id = ok(
            &router,
            "CreateUserPool",
            &json!({ "PoolName": format!("sms-role-write-{index}") }),
        )
        .await["UserPool"]["Id"]
            .as_str()
            .expect("a minted id")
            .to_string();

        let (status, payload) = cognito(
            &router,
            "UpdateUserPool",
            &json!({
                "UserPoolId": user_pool_id,
                "SmsConfiguration": sms_configuration
            }),
        )
        .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateUserPool must serve the role, got {status} with {payload}"
            );
        } else {
            assert_trust_refusal(case, "UpdateUserPool", status, &payload);
        }
        let stored = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await["UserPool"]
            .clone();
        let expected = if row.served {
            Some(role_arn.as_str())
        } else {
            None
        };
        assert_eq!(
            stored["SmsConfiguration"]["SnsCallerArn"].as_str(),
            expected,
            "case {case:?}: the pool must hold {expected:?} as its SNS caller, got {stored}"
        );

        let (status, payload) = cognito(
            &router,
            "SetUserPoolMfaConfig",
            &json!({
                "UserPoolId": user_pool_id,
                "MfaConfiguration": "ON",
                "SmsMfaConfiguration": { "SmsConfiguration": sms_configuration }
            }),
        )
        .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: SetUserPoolMfaConfig must serve the role, got {status} with \
                 {payload}"
            );
            assert_eq!(
                payload["MfaConfiguration"].as_str(),
                Some("ON"),
                "case {case:?}: the stored MFA mode must be the one that was set, got {payload}"
            );
        } else {
            assert_trust_refusal(case, "SetUserPoolMfaConfig", status, &payload);
            let read = ok(
                &router,
                "GetUserPoolMfaConfig",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await;
            assert_eq!(
                read["MfaConfiguration"].as_str(),
                Some("OFF"),
                "case {case:?}: a refused set leaves the pool's MFA off, got {read}"
            );
        }
    }
}

/// RULE (the other half): the check is the ASSUME Amazon Cognito would make, so an
/// `SnsCallerArn` this account cannot hand it a role for is refused with the same
/// error — IAM answering that it holds no role under that name leaves nothing to
/// assume, and a value naming another account's role is no role of this account to
/// begin with (`SnsCallerArn` is "the ARN of the IAM role in your Amazon Web
/// Services account", `SmsConfigurationType` in Cognito's model). The recorded
/// corpus pins it: AWS refuses such a write with
/// `InvalidSmsRoleTrustRelationshipException`.
#[tokio::test]
async fn a_pool_write_refuses_an_sms_caller_arn_this_account_holds_no_role_for() {
    let router = router();

    // The rows differ along the shape of the unheld ARN: this account's own
    // namespace, a role with a path, another account, and another partition.
    for (index, (case, role_arn)) in [
        ("this account", "arn:aws:iam::000000000000:role/not-created"),
        (
            "a role with a path",
            "arn:aws:iam::000000000000:role/service-role/not-created",
        ),
        ("another account", "arn:aws:iam::123456789012:role/remote"),
        (
            "another partition",
            "arn:aws-us-gov:iam::000000000000:role/remote",
        ),
    ]
    .iter()
    .enumerate()
    {
        let pool_name = format!("sms-role-unheld-{index}");
        let (status, payload) = cognito(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": pool_name,
                "SmsConfiguration": { "SnsCallerArn": role_arn }
            }),
        )
        .await;
        assert_trust_refusal(case, "CreateUserPool", status, &payload);
        let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
        let named = listed["UserPools"]
            .as_array()
            .expect("a listing of pools")
            .iter()
            .any(|entry| entry["Name"] == json!(pool_name));
        assert!(
            !named,
            "case {case:?}: a refused create stores no pool, got {listed}"
        );
    }
}

/// RULE (the complement): a pool write that configures no Amazon SNS caller at all
/// asks for no assume, so it is served whatever this account's IAM holds — the
/// member's own default is the empty string.
#[tokio::test]
async fn a_pool_write_that_names_no_sms_caller_is_served() {
    let router = router();

    for (index, (case, sms_configuration)) in [
        ("no SmsConfiguration at all", Value::Null),
        ("an SmsConfiguration that names no caller", json!({})),
        (
            "an SmsConfiguration carrying the member's empty default",
            json!({ "SnsCallerArn": "" }),
        ),
    ]
    .iter()
    .enumerate()
    {
        let mut body = json!({ "PoolName": format!("sms-role-absent-{index}") });
        if !sms_configuration.is_null() {
            body["SmsConfiguration"] = sms_configuration.clone();
        }
        let created = ok(&router, "CreateUserPool", &body).await;
        assert!(
            created["UserPool"]["Id"].is_string(),
            "case {case:?}: the pool must be created, got {created}"
        );
    }
}
