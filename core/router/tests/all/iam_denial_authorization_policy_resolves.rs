// SPDX-License-Identifier: CC0-1.0

//! The policy URI an authorization-details record publishes RESOLVES — to the policy
//! document the decision was actually made from.
//!
//! The rule under test: a policy URI that `GetRequestAuthorizationDetails` returns for an
//! authorization id reads back, through `GetRequestAuthorizationPolicy` for that same id,
//! the JSON document of that policy as it was captured with the denied request; a URI no
//! policy of that record carries, and an id nothing was refused under, resolve to nothing.
//!
//! Only a cross-service suite can pin this: the policy documents are IAM's
//! (`service-com-amazonaws-iam-awsidentitymanagementv20100508`, awsQuery) — written by its
//! own `PutUserPolicy` / `CreatePolicy` / `PutUserPermissionsBoundary` calls and read by
//! the gate that refuses a request — while the two reads belong to IAM Toolbox
//! (`service-com-amazonaws-iamtoolbox-authrequestservice`, restJson1, `GET
//! /authorization-details/{authorizationId}` and `.../policy`), and the two services meet
//! only over the one engine the assembled emulator mounts them on.
//!
//! The rows vary what the rule generalises over: where the policy in force comes from (an
//! inline policy embedded in the identity, a managed policy attached to it, the
//! permissions boundary capping it), how many documents are in force at once, and what
//! each document holds (one statement, several, a condition block, a resource list).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every call below is signed for. IAM is global, so this is the Region its
/// endpoint lives in.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential, which the
/// fixture calls are signed as.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The signing name IAM and IAM Toolbox share.
const SIGNING_NAME: &str = "iam";

/// IAM's API version on the awsQuery wire.
const VERSION: &str = "2010-05-08";

/// What a refusal's pointer clause names just before the authorization id. Written out
/// here rather than taken from the service under test, so this suite asserts AWS's text.
const POINTER_API: &str =
    " for complete details, or call the GetRequestAuthorizationDetails API with the \
     following authorization id: ";

/// A SigV4 `Authorization` header whose credential scope names IAM's signing name. The
/// signature itself is never checked — the router here runs with request authentication
/// off — and the access key id is what resolves the caller's identity.
fn authorization(access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{SIGNING_NAME}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

/// The emulator these rows drive: IAM beside the service that owns the authorization
/// details of a refused request, on ONE engine, as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iamtoolbox_authrequestservice::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, signed with `access_key_id`.
async fn iam(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One awsQuery call signed as the account itself, which must succeed: the fixture legs.
async fn fixture(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = iam(router, ACCOUNT_KEY, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {action} must succeed, got {status}: {body}"
    );
    body
}

/// One restJson1 GET to IAM Toolbox, signed as the account: the status and the parsed body.
async fn toolbox(router: &MultiServiceRouter, path_and_query: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://iam.amazonaws.com{path_and_query}"))
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization(ACCOUNT_KEY))
        .body(Bytes::new())
        .expect("a well-formed IAM Toolbox request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GET /authorization-details/{id}`: the record IAM Toolbox serves for an authorization
/// id.
async fn authorization_details(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    toolbox(router, &format!("/authorization-details/{id}")).await
}

/// `GET /authorization-details/{id}/policy?policyUri=…`: the policy document IAM Toolbox
/// serves for one of that record's policy URIs. `policy_uri` is `None` to omit the
/// parameter entirely.
async fn authorization_policy(
    router: &MultiServiceRouter,
    id: &str,
    policy_uri: Option<&str>,
) -> (u16, Value) {
    let query = match policy_uri {
        Some(uri) => format!("?policyUri={}", form_encode(uri)),
        None => String::new(),
    };
    toolbox(router, &format!("/authorization-details/{id}/policy{query}")).await
}

/// The authorization id the pointer clause of `message` names.
fn pointed_at(case: &str, message: &str) -> String {
    message
        .split_once(POINTER_API)
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the refusal to point at its authorization details, \
                 got {message:?}"
            )
        })
}

/// One policy document a row puts in force, as the JSON it is: the fixture submits its
/// text and the policy read must answer this same document, parsed.
fn document(index: usize, action: &str) -> Value {
    match index % 3 {
        0 => json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
        }),
        1 => json!({
            "Version": "2012-10-17",
            "Statement": [
                { "Sid": "First", "Effect": "Allow", "Action": action, "Resource": "*" },
                {
                    "Sid": "Second",
                    "Effect": "Allow",
                    "Action": ["iam:GetUser", "iam:GetRole"],
                    "Resource": ["*"],
                    "Condition": { "Bool": { "aws:MultiFactorAuthPresent": "true" } },
                },
            ],
        }),
        _ => json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": [action, "iam:GetUser"],
                "Resource": [
                    "arn:aws:iam::000000000000:user/*",
                    "arn:aws:iam::000000000000:group/*",
                ],
                "Condition": { "StringEquals": { "aws:RequestedRegion": REGION } },
            }],
        }),
    }
}

/// One policy a row puts in force: how it reaches the identity, the name it is created
/// under and the document it carries.
struct Policy {
    /// Inline in the user, attached to it as a managed policy, or its permissions
    /// boundary.
    kind: Kind,
    name: String,
    document: Value,
}

/// How a policy reaches the identity whose request is refused.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inline,
    Managed,
    Boundary,
}

/// One row: the user it builds, the policies in force on it, and the call that is refused.
struct Row {
    case: &'static str,
    user: String,
    policies: Vec<Policy>,
    refused_action: &'static str,
}

/// The rows. `prefix` keeps every row's entity and policy names its own.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one inline policy in force",
            user: "policyreader0".to_string(),
            policies: vec![Policy {
                kind: Kind::Inline,
                name: "inline0".to_string(),
                document: document(0, "iam:ListUsers"),
            }],
            refused_action: "ListRoles",
        },
        Row {
            case: "three inline policies, each a different document",
            user: "policyreader1".to_string(),
            policies: (0..3)
                .map(|index| Policy {
                    kind: Kind::Inline,
                    name: format!("inline1{index}"),
                    document: document(index, "iam:ListUsers"),
                })
                .collect(),
            refused_action: "ListRoles",
        },
        Row {
            case: "a managed policy attached beside an inline one",
            user: "policyreader2".to_string(),
            policies: vec![
                Policy {
                    kind: Kind::Inline,
                    name: "inline2".to_string(),
                    document: document(1, "iam:ListUsers"),
                },
                Policy {
                    kind: Kind::Managed,
                    name: "managed2".to_string(),
                    document: document(2, "iam:ListGroups"),
                },
            ],
            refused_action: "ListRoles",
        },
        Row {
            case: "a permissions boundary capping a grant for everything",
            user: "policyreader3".to_string(),
            policies: vec![
                Policy {
                    kind: Kind::Inline,
                    name: "inline3".to_string(),
                    document: document(0, "iam:*"),
                },
                Policy {
                    kind: Kind::Boundary,
                    name: "boundary3".to_string(),
                    document: document(0, "s3:ListAllMyBuckets"),
                },
            ],
            refused_action: "ListRoles",
        },
    ]
}

/// Build `row`'s user with its policies in force, and answer the access-key id to sign its
/// refused call with.
async fn build_principal(router: &MultiServiceRouter, row: &Row) -> String {
    fixture(router, row.case, "CreateUser", &[("UserName", &row.user)]).await;
    for policy in &row.policies {
        let text = policy.document.to_string();
        match policy.kind {
            Kind::Inline => {
                fixture(
                    router,
                    row.case,
                    "PutUserPolicy",
                    &[
                        ("UserName", &row.user),
                        ("PolicyName", &policy.name),
                        ("PolicyDocument", &text),
                    ],
                )
                .await;
            }
            Kind::Managed | Kind::Boundary => {
                let created = fixture(
                    router,
                    row.case,
                    "CreatePolicy",
                    &[("PolicyName", &policy.name), ("PolicyDocument", &text)],
                )
                .await;
                let arn = element(&created, "Arn");
                let action = if policy.kind == Kind::Managed {
                    "AttachUserPolicy"
                } else {
                    "PutUserPermissionsBoundary"
                };
                let parameter = if policy.kind == Kind::Managed {
                    "PolicyArn"
                } else {
                    "PermissionsBoundary"
                };
                fixture(
                    router,
                    row.case,
                    action,
                    &[("UserName", &row.user), (parameter, &arn)],
                )
                .await;
            }
        }
    }
    let minted = fixture(
        router,
        row.case,
        "CreateAccessKey",
        &[("UserName", &row.user)],
    )
    .await;
    element(&minted, "AccessKeyId")
}

/// Every `uri` the record for `id` publishes: in its top-level `policies` list and in the
/// `matchedPolicies` of its evaluations and checks.
fn published_policy_uris(detail: &Value) -> Vec<String> {
    let mut uris: Vec<String> = Vec::new();
    let mut collect = |value: &Value| {
        for entry in value.as_array().into_iter().flatten() {
            if let Some(uri) = entry.get("uri").and_then(Value::as_str) {
                if !uris.iter().any(|seen| seen == uri) {
                    uris.push(uri.to_string());
                }
            }
        }
    };
    collect(&detail["policies"]);
    for evaluation in detail["evaluations"].as_array().into_iter().flatten() {
        collect(&evaluation["matchedPolicies"]);
        for check in evaluation["checks"].as_array().into_iter().flatten() {
            collect(&check["matchedPolicies"]);
        }
    }
    uris
}

/// RULE: every policy URI the authorization-details record of a refused request publishes
/// reads back, for that same authorization id, the document of that policy as the account
/// holds it — whichever way the policy reached the identity.
#[tokio::test]
async fn a_published_policy_uri_reads_back_the_document_the_decision_was_made_from() {
    let router = build_router();

    for row in rows() {
        let access_key_id = build_principal(&router, &row).await;

        let (status, body) = iam(&router, &access_key_id, row.refused_action, &[]).await;
        assert_eq!(
            status, 403,
            "case {:?}: expected the refused call to answer HTTP 403, got {status}: {body}",
            row.case
        );
        let authorization_id = pointed_at(row.case, &element(&body, "Message"));

        let (detail_status, detail) = authorization_details(&router, &authorization_id).await;
        assert_eq!(
            detail_status, 200,
            "case {:?}: expected the authorization id to resolve, got {detail_status}: \
             {detail:?}",
            row.case
        );

        let uris = published_policy_uris(&detail);
        assert!(
            !uris.is_empty(),
            "case {:?}: expected the record to publish a policy URI, got {detail:?}",
            row.case
        );

        let mut answered: Vec<(String, Value)> = Vec::new();
        for uri in &uris {
            let case = format!("{} (policyUri={uri})", row.case);
            // Which policy this URI names: a policy is published under its ARN when it is
            // managed and under an opaque identifier ending in its own name when it is
            // inline, so either form ends with the name the fixture created it under.
            let policy = row
                .policies
                .iter()
                .find(|policy| uri.ends_with(&policy.name))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: the record published a URI that names none of the \
                         policies in force, got {uris:?}"
                    )
                });

            let (status, body) = authorization_policy(&router, &authorization_id, Some(uri)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected a published URI to resolve, got {status}: {body:?}"
            );
            assert_eq!(
                body,
                json!({ "policyDocument": policy.document }),
                "case {case}: expected the captured document of {:?}, got {body:?}",
                policy.name
            );
            answered.push((case, body));
        }

        // The document is read out of the named policy, not canned: two policies with
        // different documents answer differently.
        for (left_case, left) in &answered {
            for (right_case, right) in &answered {
                if left_case != right_case && left == right {
                    let left_document = &row
                        .policies
                        .iter()
                        .find(|policy| left_case.contains(&policy.name))
                        .map(|policy| policy.document.clone());
                    let right_document = &row
                        .policies
                        .iter()
                        .find(|policy| right_case.contains(&policy.name))
                        .map(|policy| policy.document.clone());
                    assert_eq!(
                        left_document, right_document,
                        "case {left_case} vs {right_case}: two different documents must \
                         answer two different bodies, got {left:?}"
                    );
                }
            }
        }

        // A URI no policy of the record carries, and the same id under another request's
        // policy URI, resolve to nothing rather than to some other record's document.
        for (what, uri) in [
            ("a policy that was never evaluated", "policy://user/nobody/none"),
            (
                "a managed policy ARN the account never created",
                "arn:aws:iam::000000000000:policy/never-created",
            ),
        ] {
            let case = format!("{} ({what})", row.case);
            let (status, body) = authorization_policy(&router, &authorization_id, Some(uri)).await;
            assert_eq!(
                status, 404,
                "case {case}: expected HTTP 404, got {status}: {body:?}"
            );
        }
    }
}

/// RULE: the policy read resolves only against a live record and only with the required
/// `policyUri` — an id nothing was refused under is a 404, and an omitted `policyUri` is a
/// 400, never the 501 of an operation nobody wrote.
#[tokio::test]
async fn a_policy_read_without_a_record_or_without_a_uri_is_refused() {
    let router = build_router();

    // An id of the modelled alphabet that this emulator never handed out.
    let unknown_id = "3x9q1p7m2k5h8d4s6g0b1n2v7";
    let (status, body) = authorization_policy(
        &router,
        unknown_id,
        Some("arn:aws:iam::000000000000:policy/anything"),
    )
    .await;
    assert_eq!(
        status, 404,
        "case an id nothing was refused under: expected HTTP 404, got {status}: {body:?}"
    );
    assert_eq!(
        body.get("__type")
            .and_then(Value::as_str)
            .map(|code| code.rsplit('#').next().unwrap_or(code).to_string()),
        Some("ResourceNotFoundException".to_string()),
        "case an id nothing was refused under: expected ResourceNotFoundException, got \
         {body:?}"
    );

    let (status, body) = authorization_policy(&router, unknown_id, None).await;
    assert_eq!(
        status, 400,
        "case an omitted policyUri: expected HTTP 400, got {status}: {body:?}"
    );
    assert_eq!(
        body.get("__type")
            .and_then(Value::as_str)
            .map(|code| code.rsplit('#').next().unwrap_or(code).to_string()),
        Some("ValidationException".to_string()),
        "case an omitted policyUri: expected ValidationException, got {body:?}"
    );
}
