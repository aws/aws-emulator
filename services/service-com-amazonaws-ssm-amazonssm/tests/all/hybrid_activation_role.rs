// SPDX-License-Identifier: CC0-1.0

//! The IAM role a hybrid activation is minted for, driven end to end over this service
//! and IAM on one shared engine.
//!
//! RULE: a machine that registers with an activation is managed UNDER the role the
//! activation names — "This IAM role must provide AssumeRole permissions for the Amazon
//! Web Services Systems Manager service principal ssm.amazonaws.com"
//! (`smithy_query(query="doc", shape="CreateActivationRequest", member="IamRole")`) — so
//! the create resolves that role against the account's IAM records first: a name IAM holds
//! no role under, or a role whose trust policy withholds `sts:AssumeRole` from
//! `ssm.amazonaws.com`, is refused with HTTP 400 `ValidationException` and NO activation is
//! minted, while a role whose trust policy grants it is accepted and the activation it
//! minted is reported by `DescribeActivations`.
//!
//! The rows span what the rule generalises over: how the trust policy states the grant
//! (the principal alone, among several, through a wildcard action, in another casing),
//! what it states instead (another service's principal, an explicit deny beside the grant,
//! an action that is not the assume, no statement at all), where IAM holds the role (at the
//! root or under a path) and a name IAM holds nothing under.
//!
//! Every role here is created through IAM's OWN `CreateRole`, and every activation through
//! this service's own operations: nothing reaches into either store.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, error_message, ACCOUNT, REGION};

/// The service principal a hybrid-registration role's trust policy has to admit.
const SSM_PRINCIPAL: &str = "ssm.amazonaws.com";

/// The action such a trust policy grants that principal.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// Another service's principal, for the rows whose trust policy names the wrong one.
const OTHER_PRINCIPAL: &str = "ec2.amazonaws.com";

/// What a row expects of a create naming its role.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// The role admits this service: the activation is minted and reported.
    Minted,
    /// IAM answered that the role cannot be assumed: the create is refused and nothing is
    /// minted.
    Refused,
}

/// One role the rule is asserted over.
struct Row {
    case: &'static str,
    /// The IAM path the role is created under.
    path: &'static str,
    /// The role name, unique within the row's own IAM store.
    role_name: &'static str,
    /// The role's `AssumeRolePolicyDocument`, or `None` for a row that creates no role at
    /// all and names one IAM holds nothing under.
    trust: Option<Value>,
    verdict: Verdict,
}

/// A trust policy carrying `statements`.
fn trust(statements: Value) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// One trust statement: `effect` the actions `action` names for the service principals
/// `principals` names.
fn statement(effect: &str, principals: Value, action: Value) -> Value {
    json!({ "Effect": effect, "Principal": { "Service": principals }, "Action": action })
}

/// The trust policy of a role this service can act as, in its plainest form.
fn trusted_by_ssm() -> Value {
    trust(json!([statement(
        "Allow",
        json!(SSM_PRINCIPAL),
        json!(ASSUME_ROLE)
    )]))
}

/// The role table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the principal and the assume, as strings",
            path: "/",
            role_name: "ssm-hybrid-plain",
            trust: Some(trusted_by_ssm()),
            verdict: Verdict::Minted,
        },
        Row {
            case: "the principal among several, the actions as a list",
            path: "/",
            role_name: "ssm-hybrid-among-several",
            trust: Some(trust(json!([statement(
                "Allow",
                json!([OTHER_PRINCIPAL, SSM_PRINCIPAL]),
                json!(["sts:TagSession", ASSUME_ROLE])
            )]))),
            verdict: Verdict::Minted,
        },
        Row {
            case: "the grant through a wildcard action",
            path: "/",
            role_name: "ssm-hybrid-wildcard",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(SSM_PRINCIPAL),
                json!("sts:*")
            )]))),
            verdict: Verdict::Minted,
        },
        Row {
            case: "the grant written in another casing",
            path: "/",
            role_name: "ssm-hybrid-casing",
            trust: Some(trust(json!([statement(
                "Allow",
                json!("SSM.AmazonAWS.com"),
                json!("STS:AssumeRole")
            )]))),
            verdict: Verdict::Minted,
        },
        Row {
            case: "a role IAM holds under a path",
            path: "/service-role/",
            role_name: "ssm-hybrid-under-a-path",
            trust: Some(trusted_by_ssm()),
            verdict: Verdict::Minted,
        },
        Row {
            case: "the grant beside a statement for another service",
            path: "/",
            role_name: "ssm-hybrid-two-statements",
            trust: Some(trust(json!([
                statement("Allow", json!(OTHER_PRINCIPAL), json!(ASSUME_ROLE)),
                statement("Allow", json!(SSM_PRINCIPAL), json!(ASSUME_ROLE)),
            ]))),
            verdict: Verdict::Minted,
        },
        Row {
            case: "a role trusting another service only",
            path: "/",
            role_name: "ssm-hybrid-other-service",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(OTHER_PRINCIPAL),
                json!(ASSUME_ROLE)
            )]))),
            verdict: Verdict::Refused,
        },
        Row {
            case: "a role granting this service another action, not the assume",
            path: "/",
            role_name: "ssm-hybrid-other-action",
            trust: Some(trust(json!([statement(
                "Allow",
                json!(SSM_PRINCIPAL),
                json!("sts:TagSession")
            )]))),
            verdict: Verdict::Refused,
        },
        Row {
            case: "a role whose trust policy denies this service explicitly",
            path: "/",
            role_name: "ssm-hybrid-denied",
            trust: Some(trust(json!([
                statement("Allow", json!(SSM_PRINCIPAL), json!(ASSUME_ROLE)),
                statement("Deny", json!(SSM_PRINCIPAL), json!(ASSUME_ROLE)),
            ]))),
            verdict: Verdict::Refused,
        },
        Row {
            case: "a role whose trust policy grants nothing at all",
            path: "/",
            role_name: "ssm-hybrid-no-statement",
            trust: Some(trust(json!([]))),
            verdict: Verdict::Refused,
        },
        Row {
            case: "a role trusting an account principal rather than a service",
            path: "/",
            role_name: "ssm-hybrid-account-principal",
            trust: Some(trust(json!([{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                "Action": ASSUME_ROLE,
            }]))),
            verdict: Verdict::Refused,
        },
        Row {
            case: "a name IAM holds no role under",
            path: "/",
            role_name: "ssm-hybrid-nobody-created",
            trust: None,
            verdict: Verdict::Refused,
        },
    ]
}

/// This service and IAM over ONE engine, so the role a create names is the record IAM's
/// own handlers hold.
fn router_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here; the router still reads the SigV4
    // credential scope, which is where the request's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM that must succeed, answering the XML body it served.
async fn iam(router: &MultiServiceRouter, case: &str, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from IAM {action}, got {status} with {text}"
    );
    text
}

/// Create `role_name` under `path` carrying `trust` through IAM's own `CreateRole`.
async fn create_role(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    role_name: &str,
    trust: &Value,
) {
    let created = iam(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", path.to_string()),
            ("AssumeRolePolicyDocument", trust.to_string()),
        ],
    )
    .await;
    assert!(
        created.contains(role_name),
        "case {case:?}: IAM must report the role it created, got {created}"
    );
}

/// `value` as an `application/x-www-form-urlencoded` field: everything but the unreserved
/// characters percent-encoded, which is how an awsQuery client writes a policy document
/// into a form body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The `IamRole` value a row's create names: the role NAME, carrying the path IAM holds
/// it under, which is what a hybrid activation's member takes.
fn iam_role_value(row: &Row) -> String {
    match row.path.trim_matches('/') {
        "" => row.role_name.to_string(),
        path => format!("{path}/{}", row.role_name),
    }
}

/// The refusal a create naming an unassumable role carries, written out here independently
/// of the service: the fixed sentence plus the ARN of the role, composed from the account
/// and the `IamRole` the request supplied.
fn expected_refusal(iam_role: &str) -> String {
    format!(
        "Nonexistent role or missing ssm service principal in trust policy: \
         arn:aws:iam::{ACCOUNT}:role/{iam_role}"
    )
}

/// The activation ids `DescribeActivations` reports in [`REGION`].
async fn listed_activations(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "DescribeActivations", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeActivations must succeed, got {status} with {body}"
    );
    body["ActivationList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeActivations reports a list, got {body}"))
        .iter()
        .map(|activation| {
            activation["ActivationId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: the create resolves the `IamRole` against the account's IAM records, and mints an
/// activation only for a role whose trust policy lets this service assume it.
#[tokio::test]
async fn an_activation_is_minted_only_for_a_role_this_service_may_assume() {
    for row in rows() {
        let case = row.case;
        // A router per row, so each row's IAM store holds only its own role.
        let router = router_with_iam();
        if let Some(document) = row.trust.as_ref() {
            create_role(&router, case, row.path, row.role_name, document).await;
        }
        let iam_role = iam_role_value(&row);

        let (status, body) = call(
            &router,
            "CreateActivation",
            &json!({
                "IamRole": iam_role,
                "Description": case,
                "RegistrationLimit": 1,
            }),
        )
        .await;

        match row.verdict {
            Verdict::Minted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} with {body}"
                );
                let id = body["ActivationId"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: a served create reports an ActivationId, got {body}")
                });
                assert!(
                    body["ActivationCode"].as_str().is_some_and(|code| !code.is_empty()),
                    "case {case:?}: a served create reports an ActivationCode, got {body}"
                );
                let listed = listed_activations(&router, case).await;
                assert_eq!(
                    listed,
                    vec![id.to_string()],
                    "case {case:?}: the minted activation must be reported, got {listed:?}"
                );
            }
            Verdict::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body}"
                );
                let expected = expected_refusal(&iam_role);
                assert_eq!(
                    error_message(&body),
                    expected,
                    "case {case:?}: expected the refusal {expected:?}, got {:?}",
                    error_message(&body)
                );
                let listed = listed_activations(&router, case).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: a refused create must mint no activation, got {listed:?}"
                );
            }
        }
    }
}

/// RULE, grounded: the refusal rests on what IAM ANSWERED about the role, so an emulator
/// with no IAM mounted to ask refuses nothing — the create is served, whatever role it
/// names.
#[tokio::test]
async fn a_create_is_served_where_no_iam_answers_about_the_role() {
    let router = crate::parameter_store::router();
    for iam_role in ["ssm-hybrid-nobody-created", "service-role/also-not-created"] {
        let (status, body) = call(
            &router,
            "CreateActivation",
            &json!({ "IamRole": iam_role }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {iam_role:?}: with no IAM to ask, the create is served, got {status} with {body}"
        );
        assert!(
            body["ActivationId"].as_str().is_some(),
            "case {iam_role:?}: a served create reports an ActivationId, got {body}"
        );
    }
}

/// RULE: the role is resolved per CREATE, so replacing a role's trust policy through IAM's
/// own `UpdateAssumeRolePolicy` changes what the next create is answered — the decision is
/// read from IAM's records at the moment of the call rather than cached.
#[tokio::test]
async fn the_trust_policy_in_force_at_the_create_decides_it() {
    let case = "a role whose trust policy changes between creates";
    let router = router_with_iam();
    let role_name = "ssm-hybrid-retrusted";
    // Created trusting another service: the first create is refused.
    create_role(
        &router,
        case,
        "/",
        role_name,
        &trust(json!([statement(
            "Allow",
            json!(OTHER_PRINCIPAL),
            json!(ASSUME_ROLE)
        )])),
    )
    .await;
    let body = json!({ "IamRole": role_name });
    let (status, refused) = call(&router, "CreateActivation", &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the first create must be refused, got {status} with {refused}"
    );

    // Re-trusted through IAM itself: the next create is served.
    iam(
        &router,
        case,
        "UpdateAssumeRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyDocument", trusted_by_ssm().to_string()),
        ],
    )
    .await;
    let (status, served) = call(&router, "CreateActivation", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: after the trust policy admits this service, the create must be \
         served, got {status} with {served}"
    );
    let id = served["ActivationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a served create reports an id, got {served}"));
    let listed = listed_activations(&router, case).await;
    assert_eq!(
        listed,
        vec![id.to_string()],
        "case {case:?}: exactly the one activation the served create minted is reported, got \
         {listed:?}"
    );
}
