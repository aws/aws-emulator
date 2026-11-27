// SPDX-License-Identifier: CC0-1.0

//! The SMS-caller probe a user-pool write makes, pinned as a decision that
//! SETTLES on the records the account holds.
//!
//! RULE: a pool write that binds an Amazon SNS caller role decides the binding
//! from this account's IAM records as they stand AT THE MOMENT OF THE CALL — a
//! pool sends a text message by ASSUMING the role its `SmsConfiguration` names
//! and PUBLISHING as it, `SnsCallerArn` being "the ARN of the IAM role in your
//! Amazon Web Services account that Amazon Cognito will use to send SMS
//! messages" (`SmsConfigurationType` in Cognito's model) — so every such write
//! reaches one of three outcomes on its FIRST call, identical writes answer
//! identically however often they are repeated, and the two defects of such a
//! role are reported apart:
//!
//! * a document that withholds the assume Amazon Cognito would make is
//!   `InvalidSmsRoleTrustRelationshipException`;
//! * a role it can assume whose permissions withhold the publish is
//!   `InvalidSmsRoleAccessPolicyException`, whose sentence is fixed text.
//!
//! Nothing about the verdict is latched, deferred or retried: a role whose
//! records CHANGE is judged by the records in force at the next call, in either
//! direction. That is the property a client depends on when it tolerates IAM
//! propagation by retrying the create — a probe that never settles leaves such a
//! client looping on one refusal — and it is why this suite lives here rather
//! than in the Cognito crate: the roles are written through IAM's own
//! `CreateRole`, `PutRolePolicy`, `DeleteRolePolicy` and
//! `UpdateAssumeRolePolicy` over the same engine that serves Cognito, so what
//! the probe reads is what IAM persisted.
//!
//! The two refusals' own tables — which trust narrowing admits the assume, and
//! which permissions document allows the publish — are
//! `all/cognito_sms_caller_role_trust.rs` and the Cognito crate's
//! `tests/all/sms_caller_role_publish_grant.rs`; this suite is about the
//! decision being reached, stable and re-read.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

// The form encoder a request's body is written with and the reader an XML
// answer's element is read with are this test crate's own, shared rather than
// written a second time (`all/query_wire.rs`).
use crate::query_wire::{element, form_encode};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The service principal a user pool takes its SMS caller role on as.
const COGNITO_PRINCIPAL: &str = "cognito-idp.amazonaws.com";

/// The error code of a write naming a role Amazon Cognito cannot assume.
const TRUST_RELATIONSHIP_CODE: &str = "InvalidSmsRoleTrustRelationshipException";

/// The sentence that refusal carries.
const TRUST_RELATIONSHIP_MESSAGE: &str =
    "Role does not have a trust relationship allowing Cognito to assume the role";

/// The error code of a write naming a role that cannot publish once assumed.
const ACCESS_POLICY_CODE: &str = "InvalidSmsRoleAccessPolicyException";

/// The sentence that refusal carries: fixed text, naming neither the role nor
/// the pool.
const ACCESS_POLICY_MESSAGE: &str = "Role does not have permission to publish with SNS";

/// The name the inline permissions document of every role below is written
/// under.
const PERMISSIONS_POLICY_NAME: &str = "sms-caller-permissions";

/// The external id every configuration of the first table presents, so a row
/// differs only in what its grant is narrowed to.
const PRESENTED_EXTERNAL_ID: &str = "tenant-one";

/// How many times one identical write is repeated: the answer may not depend on
/// how often the role has already been probed.
const ATTEMPTS: usize = 3;

/// Cognito and IAM over ONE shared engine, as `router/src/lib.rs` mounts them,
/// so Cognito's in-process callback reaches this very IAM.
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
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// An IAM call that must succeed, with its XML body.
async fn iam_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = iam(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must be served by IAM, got {status} with {body}"
    );
    body
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
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cognito-idp/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
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

/// A Cognito call that must succeed, with its response body.
async fn cognito_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = cognito(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// This account's id, as IAM mints every ARN with.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A trust policy allowing `COGNITO_PRINCIPAL` the assume, narrowed by
/// `condition` when one is given.
fn trust(condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": COGNITO_PRINCIPAL },
        "Action": "sts:AssumeRole",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// The `Condition` an SMS caller's trust policy is narrowed by when it names
/// every key the assume really carries: the external id the configuration
/// presents, the account that owns the pool, and the pools of this account in
/// this Region.
fn every_assume_key(external_id: &str) -> Value {
    json!({
        "StringEquals": {
            "sts:ExternalId": external_id,
            "aws:SourceAccount": account(),
        },
        "ArnLike": {
            "aws:SourceArn": format!("arn:aws:cognito-idp:{REGION}:{}:userpool/*", account())
        },
    })
}

/// A permissions policy with one statement: `effect` on `action` over
/// `resource`.
fn permissions(effect: &str, action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }],
    })
}

/// The grant AWS documents for an Amazon SNS caller role: the publish, which
/// names no resource of its own, an SMS message going to a phone number rather
/// than to a topic.
fn publish_grant() -> Value {
    permissions("Allow", "sns:Publish", "*")
}

/// Create a role named `name` carrying `trust_document`, with the inline
/// permissions document `document` when one is given, and answer the ARN IAM
/// persisted for it.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    trust_document: &Value,
    document: Option<&Value>,
) -> String {
    let created = iam_ok(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust_document.to_string()),
        ],
    )
    .await;
    if let Some(document) = document {
        iam_ok(
            router,
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", PERMISSIONS_POLICY_NAME),
                ("PolicyDocument", &document.to_string()),
            ],
        )
        .await;
    }
    element(&created, "Arn")
}

/// What a pool write naming a role answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The configuration is stored.
    Served,
    /// `InvalidSmsRoleTrustRelationshipException`.
    Unassumable,
    /// `InvalidSmsRoleAccessPolicyException`.
    NoPublish,
}

impl Verdict {
    /// The wire error code and sentence this verdict is carried by, and `None`
    /// for a write that is served.
    fn refusal(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Served => None,
            Self::Unassumable => Some((TRUST_RELATIONSHIP_CODE, TRUST_RELATIONSHIP_MESSAGE)),
            Self::NoPublish => Some((ACCESS_POLICY_CODE, ACCESS_POLICY_MESSAGE)),
        }
    }
}

/// Whether `ListUserPools` reports a pool named `pool_name`.
async fn listed(router: &MultiServiceRouter, pool_name: &str) -> bool {
    let listing = cognito_ok(router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
    listing["UserPools"]
        .as_array()
        .expect("a listing of pools")
        .iter()
        .any(|entry| entry["Name"] == json!(pool_name))
}

/// Bind the caller `role_arn` with `external_id` through `CreateUserPool`,
/// holding the answer to `expected`, and check what the account holds
/// afterwards: a served write round-trips through `DescribeUserPool` and appears
/// in `ListUserPools`, and a refused one leaves no pool behind.
async fn create_binding(
    router: &MultiServiceRouter,
    case: &str,
    pool_name: &str,
    role_arn: &str,
    external_id: &str,
    expected: Verdict,
) {
    let (status, payload) = cognito(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": pool_name,
            "SmsConfiguration": { "SnsCallerArn": role_arn, "ExternalId": external_id },
        }),
    )
    .await;
    match expected.refusal() {
        None => {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateUserPool must bind the role, got {status} with {payload}"
            );
            let user_pool_id = payload["UserPool"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted pool id, got {payload}"))
                .to_string();
            let described = cognito_ok(
                router,
                "DescribeUserPool",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await["UserPool"]
                .clone();
            assert_eq!(
                described["SmsConfiguration"]["SnsCallerArn"].as_str(),
                Some(role_arn),
                "case {case:?}: the stored pool must echo the caller it was bound to, got \
                 {described}"
            );
            assert_eq!(
                described["SmsConfiguration"]["ExternalId"].as_str(),
                Some(external_id),
                "case {case:?}: the stored pool must echo the external id it was bound with, got \
                 {described}"
            );
            assert!(
                listed(router, pool_name).await,
                "case {case:?}: a served create stores the pool {pool_name:?}"
            );
        }
        Some((code, message)) => {
            assert_eq!(
                status, 400,
                "case {case:?}: CreateUserPool must refuse the role, got {status} with {payload}"
            );
            assert_eq!(
                payload["__type"].as_str(),
                Some(code),
                "case {case:?}: CreateUserPool must name {code}, got {payload}"
            );
            assert_eq!(
                payload["message"].as_str(),
                Some(message),
                "case {case:?}: CreateUserPool must carry the refusal sentence, got {payload}"
            );
            assert!(
                !listed(router, pool_name).await,
                "case {case:?}: a refused create stores no pool {pool_name:?}"
            );
        }
    }
}

/// One row of the stability table: the records a role holds, and the verdict
/// every write naming it must answer.
struct Row {
    case: &'static str,
    /// The trust policy the role carries.
    trust_document: Value,
    /// The inline permissions document the role holds, `None` for a role
    /// holding none.
    document: Option<Value>,
    expected: Verdict,
}

/// RULE, first half: the probe reaches its verdict on the FIRST call and answers
/// the same way for every identical call after it, whichever of the three
/// outcomes the role's records lead to.
///
/// The rows differ along what the rule generalises over: how the trust grant is
/// narrowed (not at all, by the external id alone, by every key the assume
/// carries, by another tenant's id, by another service principal), what the
/// permissions document grants (the documented publish, every Amazon SNS
/// action, a publish scoped to one topic, nothing at all), and which of the
/// three outcomes follows.
#[tokio::test]
async fn an_sms_caller_binding_settles_on_the_first_call_and_stays_there() {
    let router = router();

    let rows: Vec<Row> = vec![
        Row {
            case: "an unconditioned grant beside the documented publish",
            trust_document: trust(None),
            document: Some(publish_grant()),
            expected: Verdict::Served,
        },
        Row {
            case: "a grant narrowed by every key the assume carries",
            trust_document: trust(Some(every_assume_key(PRESENTED_EXTERNAL_ID))),
            document: Some(publish_grant()),
            expected: Verdict::Served,
        },
        Row {
            case: "a grant narrowed by the external id alone, with every Amazon SNS action",
            trust_document: trust(Some(json!({
                "StringEquals": { "sts:ExternalId": PRESENTED_EXTERNAL_ID }
            }))),
            document: Some(permissions("Allow", "sns:*", "*")),
            expected: Verdict::Served,
        },
        Row {
            case: "a role Amazon Cognito may assume that holds no permissions document",
            trust_document: trust(Some(every_assume_key(PRESENTED_EXTERNAL_ID))),
            document: None,
            expected: Verdict::NoPublish,
        },
        Row {
            case: "a role Amazon Cognito may assume whose publish is scoped to one topic",
            trust_document: trust(None),
            document: Some(permissions(
                "Allow",
                "sns:Publish",
                &format!("arn:aws:sns:{REGION}:{}:verification", account()),
            )),
            expected: Verdict::NoPublish,
        },
        Row {
            case: "a grant narrowed to another tenant's external id",
            trust_document: trust(Some(json!({
                "StringEquals": { "sts:ExternalId": "another-tenant" }
            }))),
            document: Some(publish_grant()),
            expected: Verdict::Unassumable,
        },
        Row {
            case: "a document that grants another service principal only",
            trust_document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "sns.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            }),
            document: Some(publish_grant()),
            expected: Verdict::Unassumable,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let role_arn = given_role(
            &router,
            row.case,
            &format!("settling-caller-{index}"),
            &row.trust_document,
            row.document.as_ref(),
        )
        .await;
        for attempt in 0..ATTEMPTS {
            let case = format!("{} (attempt {attempt})", row.case);
            create_binding(
                &router,
                &case,
                &format!("settling-{index}-{attempt}"),
                &role_arn,
                PRESENTED_EXTERNAL_ID,
                row.expected,
            )
            .await;
        }
    }
}

/// What one step of the re-reading table does to the role's IAM records before
/// it writes a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Leave the records as the previous step left them.
    Nothing,
    /// Write the documented publish grant as the role's inline policy.
    GrantThePublish,
    /// Rewrite that policy to deny the publish beside granting it.
    DenyThePublish,
    /// Delete the role's inline policy, leaving it with no permissions at all.
    DropThePermissions,
    /// Rewrite the trust policy so it admits another tenant's external id only.
    TrustAnotherTenant,
    /// Rewrite the trust policy back to the grant this pool's assume satisfies.
    TrustThisTenant,
}

/// RULE, second half: nothing about the verdict is latched — the records in
/// force at the next call decide it, in either direction — so a role that gains
/// the publish grant is bound by the very next write, and one that loses it (or
/// loses the trust) is refused again, each with its own error.
///
/// The steps differ along which record changes and which way it moves: the
/// permissions document gained, denied, dropped and regained, and the trust
/// document narrowed away from this pool's assume and back.
#[tokio::test]
async fn an_sms_caller_binding_follows_the_records_in_force_at_each_call() {
    let router = router();
    let external_id = "settling-tenant";
    let role_name = "re-read-caller";
    let role_arn = given_role(
        &router,
        "a role that trusts Amazon Cognito and holds no permissions",
        role_name,
        &trust(Some(every_assume_key(external_id))),
        None,
    )
    .await;

    let steps: Vec<(&str, Change, Verdict)> = vec![
        (
            "before any permissions document is written",
            Change::Nothing,
            Verdict::NoPublish,
        ),
        (
            "once the publish grant is written",
            Change::GrantThePublish,
            Verdict::Served,
        ),
        (
            "the same role, written again with the publish denied beside the grant",
            Change::DenyThePublish,
            Verdict::NoPublish,
        ),
        (
            "once the grant is restored",
            Change::GrantThePublish,
            Verdict::Served,
        ),
        (
            "once the permissions document is deleted",
            Change::DropThePermissions,
            Verdict::NoPublish,
        ),
        (
            "once the grant is restored a second time",
            Change::GrantThePublish,
            Verdict::Served,
        ),
        (
            "once the trust document admits another tenant only",
            Change::TrustAnotherTenant,
            Verdict::Unassumable,
        ),
        (
            "once the trust document admits this pool's assume again",
            Change::TrustThisTenant,
            Verdict::Served,
        ),
    ];

    for (index, (case, change, expected)) in steps.into_iter().enumerate() {
        match change {
            Change::Nothing => {}
            Change::GrantThePublish | Change::DenyThePublish => {
                let document = if change == Change::GrantThePublish {
                    publish_grant()
                } else {
                    json!({
                        "Version": "2012-10-17",
                        "Statement": [
                            { "Effect": "Allow", "Action": "sns:Publish", "Resource": "*" },
                            { "Effect": "Deny", "Action": "sns:Publish", "Resource": "*" },
                        ],
                    })
                };
                iam_ok(
                    &router,
                    case,
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name),
                        ("PolicyName", PERMISSIONS_POLICY_NAME),
                        ("PolicyDocument", &document.to_string()),
                    ],
                )
                .await;
            }
            Change::DropThePermissions => {
                iam_ok(
                    &router,
                    case,
                    "DeleteRolePolicy",
                    &[
                        ("RoleName", role_name),
                        ("PolicyName", PERMISSIONS_POLICY_NAME),
                    ],
                )
                .await;
            }
            Change::TrustAnotherTenant | Change::TrustThisTenant => {
                let tenant = if change == Change::TrustThisTenant {
                    external_id
                } else {
                    "another-tenant"
                };
                let document = trust(Some(every_assume_key(tenant)));
                iam_ok(
                    &router,
                    case,
                    "UpdateAssumeRolePolicy",
                    &[
                        ("RoleName", role_name),
                        ("PolicyDocument", &document.to_string()),
                    ],
                )
                .await;
            }
        }
        create_binding(
            &router,
            case,
            &format!("re-read-{index}"),
            &role_arn,
            external_id,
            expected,
        )
        .await;
    }
}
