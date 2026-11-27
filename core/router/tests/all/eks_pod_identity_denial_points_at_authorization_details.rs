// SPDX-License-Identifier: CC0-1.0

//! A refusal served to a POD IDENTITY credential points at the authorization details of
//! that one request, and the id it names resolves.
//!
//! The rule under test: an identity-service refusal decided by evaluating the policies in
//! force closes with a pointer naming an authorization id minted for that one refused
//! request — the same id in the console address and in the `GetRequestAuthorizationDetails`
//! pointer — whatever KIND of principal signed it, including a session of a role another
//! service minted ("the temporary security credentials … have the permissions of the role",
//! `sts:AssumeRole`), and that id names the record of that decision: the context it was made
//! in, the one `(action, resource)` evaluation it was made on with the effect that decided
//! it, and the documents in force. A call those documents allow is served and mints no id at
//! all, because only a refusal is a decision to point at.
//!
//! Only a cross-service suite can pin this: the credential is minted by EKS Auth
//! (`service-com-amazonaws-eksauth-eksauthfrontend`, restJson1
//! `POST /clusters/{clusterName}/assume-role-for-pod-identity`), the refusal is the identity
//! service's (`service-com-amazonaws-iam-awsidentitymanagementv20100508`, awsQuery) and the
//! record it points at belongs to IAM Toolbox
//! (`service-com-amazonaws-iamtoolbox-authrequestservice`, restJson1
//! `GET /authorization-details/{authorizationId}`) — three services that meet only over the
//! one engine the assembled emulator mounts them on. The suite beside this file that drives
//! the pod-identity path from the published cluster endpoint pins what such a credential MAY
//! DO; this one pins what a refusal of one HANDS BACK, and takes the shorter road to the
//! credential (the cluster's own registration and token seams) because the Kubernetes path is
//! that suite's subject and not this one's.
//!
//! The rows vary what the rule generalises over: which operation is refused, the resource
//! form the decision is made on, which document decided it (nothing in force, a grant for
//! another service's action, a statement saying `Deny`, a session policy the association
//! carries) and whether the call is refused at all.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    service_state as pod_identity_state, ProgrammedAssociation, ServiceAccountTokenRequest,
    DEFAULT_REGION,
};

use crate::iam_authorization_details_pointer::assert_identity_refusal;
use crate::query_wire::{element, form_encode, opt_element};

/// The access-key id standing for the account's own published credential: IAM holds no
/// record for it, so it carries no identity-based policies and the fixture calls below are
/// served.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// IAM's API version on the awsQuery wire.
const VERSION: &str = "2010-05-08";

/// The Kubernetes namespace every association here is written for — the one Kubernetes
/// creates with the cluster.
const NAMESPACE: &str = "default";

/// The cluster whose Kubernetes API server issues the tokens below.
const CLUSTER: &str = "pod-identity-pointer";

/// The `EvaluatedEffect` the record of a call nothing in force allowed carries.
const IMPLICIT_DENY: &str = "IMPLICIT_DENY";

/// The `EvaluatedEffect` the record of a call a statement explicitly denied carries.
const EXPLICIT_DENY_EFFECT: &str = "EXPLICIT_DENY";

/// The reason clause of a refusal a statement about the request denied.
const EXPLICIT_DENY_REASON: &str = "with an explicit deny in an identity-based policy";

/// The `PolicyType` of an identity-based document in the record of a decision.
const IDENTITY_BASED_POLICY: &str = "IDENTITY_BASED_POLICY";

/// The `PolicyType` of a session policy in that record.
const SESSION_POLICY: &str = "SESSION_POLICY";

/// The condition key an assumed-role session's unique id is published under.
const USER_ID_KEY: &str = "aws:userid";

/// The condition key naming the class of credential the request was signed with.
const PRINCIPAL_TYPE_KEY: &str = "aws:PrincipalType";

/// What that key answers for a session of a role.
const ASSUMED_ROLE: &str = "AssumedRole";

/// The session tag EKS Pod Identity puts the pod's namespace on, under the principal-tag
/// family a policy reads it through.
const NAMESPACE_TAG_KEY: &str = "aws:PrincipalTag/kubernetes-namespace";

/// The trust policy EKS Pod Identity documents for an association's role: the pods service
/// principal, with the assume and the session tagging it performs.
const POD_IDENTITY_TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"pods.eks.amazonaws.com"},"#,
    r#""Action":["sts:AssumeRole","sts:TagSession"]}]}"#
);

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// `access_key_id`. The signature is never checked — this router runs with request
/// authentication off — and the access key id is what resolves the calling principal.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{DEFAULT_REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

/// The emulator these rows drive: the service that mints a pod identity credential, the
/// identity service that decides a request signed with it, and the service that owns the
/// authorization details of a refused request — on ONE engine, as `core/router/src/lib.rs`
/// mounts them.
fn build() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iamtoolbox_authrequestservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (engine, router)
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
        .header("authorization", authorization(access_key_id, "iam"))
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

/// `GET /authorization-details/{id}`: the read IAM Toolbox serves, signed as the account.
async fn authorization_details(router: &MultiServiceRouter, authorization_id: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://iam.amazonaws.com/authorization-details/{authorization_id}"
        ))
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization(ACCOUNT_KEY, "iam"))
        .body(Bytes::new())
        .expect("a well-formed GetRequestAuthorizationDetails request");
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

/// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`: the exchange the
/// EKS Pod Identity Agent makes, answered as the credential's access key id, the ARN of the
/// session it speaks for and that session's unique id.
async fn exchange(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    token: &str,
) -> (String, String, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://eks-auth.{DEFAULT_REGION}.amazonaws.com/clusters/{cluster}\
             /assume-role-for-pod-identity"
        ))
        .header("host", format!("eks-auth.{DEFAULT_REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(ACCOUNT_KEY, "eks-auth"))
        .body(Bytes::from(
            serde_json::json!({ "token": token }).to_string(),
        ))
        .expect("a well-formed restJson1 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the pod identity exchange must be served, got {status}: {text}"
    );
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let access_key_id = body["credentials"]["accessKeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the exchange must vend credentials, got {text}"))
        .to_string();
    let session_arn = body["assumedRoleUser"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the exchange must report the session, got {text}"))
        .to_string();
    let assume_role_id = body["assumedRoleUser"]["assumeRoleId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the exchange must report the id, got {text}"))
        .to_string();
    (access_key_id, session_arn, assume_role_id)
}

/// Program `associations` into the cluster's pod identity plane, as the EKS control plane
/// publishes them, registering the cluster with them.
fn program(engine: &engine::InMemoryEngine, associations: &[ProgrammedAssociation]) {
    let state = pod_identity_state(engine).expect("the engine carries the pod identity store");
    let mut guard = state.lock();
    let programmed = guard.program_pod_identity_associations(CLUSTER, DEFAULT_REGION, associations);
    assert_eq!(
        programmed,
        associations.len(),
        "setup: every association must be programmed, got {programmed}"
    );
}

/// A projected service account token for `service_account`, issued by the cluster's own
/// Kubernetes API server and bound to the pod it is projected into — the only token class
/// the exchange accepts.
fn pod_token(engine: &engine::InMemoryEngine, service_account: &str, pod: &str) -> String {
    let state = pod_identity_state(engine).expect("the engine carries the pod identity store");
    let mut guard = state.lock();
    guard
        .cluster_mut(CLUSTER)
        .expect("the cluster was registered with its associations")
        .issue_service_account_token(&ServiceAccountTokenRequest::for_pod(
            NAMESPACE,
            service_account,
            pod,
        ))
        .expect("the cluster issues a token for a service account it was asked about")
}

/// A one-statement document about `action`, on every resource.
fn statement(effect: &str, action: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"*"}}]}}"#
    )
}

/// What one row expects of the call the pod then makes.
enum Outcome {
    /// The documents in force allow it, so it is served and nothing is filed.
    Served,
    /// Nothing in force allows it, or a statement denies it: the clause the refusal closes
    /// with before the pointer, and the effect the record reports.
    Refused {
        reason: String,
        effect: &'static str,
    },
}

/// One row: what the association's role and the association itself say about the call, the
/// awsQuery call the pod makes with the vended credential, and what it earns.
struct Row {
    case: &'static str,
    /// The inline documents embedded in the association's role.
    role_policies: Vec<(&'static str, String)>,
    /// The association's own session policy, when it carries one.
    session_policy: Option<String>,
    /// The awsQuery `Action` and its form parameters.
    action: &'static str,
    params: Vec<(&'static str, String)>,
    /// The action the decision is made on.
    iam_action: &'static str,
    /// The resource the decision is made against, as the refusal names it.
    resource: String,
    outcome: Outcome,
}

/// The reason clause for a call no identity-based document in force allows.
fn no_identity_policy(iam_action: &str) -> String {
    format!("because no identity-based policy allows the {iam_action} action")
}

/// The reason clause for a call the role allows and the credential's own scope does not.
fn no_session_policy(iam_action: &str) -> String {
    format!("because no session policy allows the {iam_action} action")
}

/// The rows, over the account the fixture calls run against.
fn rows(account: &str) -> Vec<Row> {
    vec![
        // A role carrying no document at all: the call is withheld on the identity-based
        // policies, decided on the collection the request addresses.
        Row {
            case: "a role carrying no identity-based policy",
            role_policies: Vec::new(),
            session_policy: None,
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            resource: format!("arn:aws:iam::{account}:role/"),
            outcome: Outcome::Refused {
                reason: no_identity_policy("iam:ListRoles"),
                effect: IMPLICIT_DENY,
            },
        },
        // A role granting another service's action only — the narrow pod role a customer
        // writes: a document IS in force and does not cover the call.
        Row {
            case: "a role granting another service's action only",
            role_policies: vec![("pod-scope", statement("Allow", "cloudwatch:ListMetrics"))],
            session_policy: None,
            action: "ListUsers",
            params: Vec::new(),
            iam_action: "iam:ListUsers",
            resource: format!("arn:aws:iam::{account}:user/"),
            outcome: Outcome::Refused {
                reason: no_identity_policy("iam:ListUsers"),
                effect: IMPLICIT_DENY,
            },
        },
        // A statement about the request saying Deny over a grant of everything, on a
        // path-scoped collection: the other decision, and the other resource form.
        Row {
            case: "a statement denying the action beside a grant of everything",
            role_policies: vec![
                ("allow-everything", statement("Allow", "iam:*")),
                ("deny-the-action", statement("Deny", "iam:ListGroups")),
            ],
            session_policy: None,
            action: "ListGroups",
            params: vec![("PathPrefix", "/eng/".to_string())],
            iam_action: "iam:ListGroups",
            resource: format!("arn:aws:iam::{account}:group/eng/"),
            outcome: Outcome::Refused {
                reason: EXPLICIT_DENY_REASON.to_string(),
                effect: EXPLICIT_DENY_EFFECT,
            },
        },
        // The association's own session policy allows another action only: the role allows
        // the call and the credential's scope caps it away, which is the third clause.
        Row {
            case: "an association whose session policy allows another action only",
            role_policies: vec![("allow-the-action", statement("Allow", "iam:ListRoles"))],
            session_policy: Some(statement("Allow", "cloudwatch:ListMetrics")),
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            resource: format!("arn:aws:iam::{account}:role/"),
            outcome: Outcome::Refused {
                reason: no_session_policy("iam:ListRoles"),
                effect: IMPLICIT_DENY,
            },
        },
        // The call the role allows: served, and no decision to point at.
        Row {
            case: "a role allowing the call",
            role_policies: vec![("allow-the-action", statement("Allow", "iam:ListRoles"))],
            session_policy: None,
            action: "ListRoles",
            params: Vec::new(),
            iam_action: "iam:ListRoles",
            resource: format!("arn:aws:iam::{account}:role/"),
            outcome: Outcome::Served,
        },
    ]
}

/// The entries of the record's `policies` list of one `PolicyType`.
fn policies_of<'a>(detail: &'a Value, policy_type: &str) -> Vec<&'a Value> {
    detail["policies"]
        .as_array()
        .map(|policies| {
            policies
                .iter()
                .filter(|policy| policy["type"].as_str() == Some(policy_type))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the identity service's refusal of a request signed with a pod identity credential
/// closes with the pointer at that request's own authorization details — the same minted id
/// in the console address and in the API pointer — and that id names the record of THAT
/// decision in the service that serves it; a call the documents in force allow is served and
/// points at nothing.
#[tokio::test]
async fn a_pod_identity_refusal_points_at_the_authorization_details_of_that_request() {
    let (engine, router) = build();
    let account = engine::emulator_account_id();
    let rows = rows(account);

    // The roles the associations bind, and the associations the EKS control plane published
    // for the cluster — written before any token is issued, exactly as a cluster is set up.
    let mut associations = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let role_name = format!("pod-identity-pointer-{index}");
        fixture(
            &router,
            row.case,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", POD_IDENTITY_TRUST_POLICY),
            ],
        )
        .await;
        for (policy_name, document) in &row.role_policies {
            fixture(
                &router,
                row.case,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.as_str()),
                    ("PolicyName", policy_name),
                    ("PolicyDocument", document.as_str()),
                ],
            )
            .await;
        }
        let association_id = format!("a-podidentity{index:06}");
        associations.push(ProgrammedAssociation {
            association_arn: format!(
                "arn:aws:eks:{DEFAULT_REGION}:{account}:podidentityassociation/{CLUSTER}\
                 /{association_id}"
            ),
            association_id,
            namespace: NAMESPACE.to_string(),
            service_account: format!("pointer-{index}"),
            role_arn: format!("arn:aws:iam::{account}:role/{role_name}"),
            policy: row.session_policy.clone(),
            target_role_arn: None,
            external_id: None,
            // "When policy is specified, disableSessionTags must be set to true" — what the
            // control plane requires of an association carrying a session policy.
            disable_session_tags: row.session_policy.is_some().then_some(true),
            // None of these rows enables workload identity, so the exchange reports no
            // provider ARN for them.
            workload_identity_provider_arn: None,
        });
    }
    program(&engine, &associations);

    let mut minted: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let service_account = format!("pointer-{index}");
        let token = pod_token(&engine, &service_account, &format!("{service_account}-pod"));
        let (access_key_id, session_arn, assume_role_id) =
            exchange(&router, row.case, CLUSTER, &token).await;
        let role_name = format!("pod-identity-pointer-{index}");
        assert!(
            session_arn.starts_with(&format!("arn:aws:sts::{account}:assumed-role/{role_name}/")),
            "case {:?}: expected a session of the association's role, got {session_arn:?}",
            row.case
        );

        let params: Vec<(&str, &str)> = row
            .params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (status, body) = iam(&router, &access_key_id, row.action, &params).await;

        let Outcome::Refused { reason, effect } = &row.outcome else {
            assert_eq!(
                status, 200,
                "case {:?}: a call the role allows must be served, got {status}: {body}",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 403,
            "case {:?}: a call nothing in force allows must be refused 403, got {status}: {body}",
            row.case
        );
        assert_eq!(
            opt_element(&body, "Code").as_deref(),
            Some("AccessDenied"),
            "case {:?}: expected the error code AccessDenied, got {body}",
            row.case
        );
        // The whole message: the sentence naming the decision, and the pointer at that
        // decision's authorization details naming one minted id twice.
        let authorization_id = assert_identity_refusal(
            row.case,
            &element(&body, "Message"),
            &format!(
                "User: {session_arn} is not authorized to perform: {} on resource: {} {reason}",
                row.iam_action, row.resource
            ),
        );
        minted.push(authorization_id.clone());

        // The id resolves — in the service that owns the record, over its own wire.
        let (detail_status, detail) = authorization_details(&router, &authorization_id).await;
        assert_eq!(
            detail_status, 200,
            "case {:?}: expected the authorization id {authorization_id:?} to resolve, got \
             {detail_status}: {detail:?}",
            row.case
        );

        // The record is about THIS request: the action and resource the decision was made
        // on, and the effect that decided it.
        let evaluations = detail["evaluations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            evaluations.len(),
            1,
            "case {:?}: expected one evaluation, got {detail:?}",
            row.case
        );
        let evaluation = &evaluations[0];
        assert_eq!(
            evaluation["action"].as_str(),
            Some(row.iam_action),
            "case {:?}: expected the evaluation to name the action decided, got {evaluation:?}",
            row.case
        );
        assert_eq!(
            evaluation["resource"].as_str(),
            Some(row.resource.as_str()),
            "case {:?}: expected the evaluation to name the resource decided against, got \
             {evaluation:?}",
            row.case
        );
        assert_eq!(
            evaluation["evaluatedEffect"].as_str(),
            Some(*effect),
            "case {:?}: expected the effect {effect:?}, got {evaluation:?}",
            row.case
        );

        // The context the decision was made in describes the POD's session: the unique id
        // the exchange reported for it, and the class of credential it is.
        assert_eq!(
            detail["requestContext"][USER_ID_KEY].as_str(),
            Some(assume_role_id.as_str()),
            "case {:?}: expected the context to name the session's unique id, got {detail:?}",
            row.case
        );
        assert_eq!(
            detail["requestContext"][PRINCIPAL_TYPE_KEY].as_str(),
            Some(ASSUMED_ROLE),
            "case {:?}: expected the context to name the credential's class, got {detail:?}",
            row.case
        );
        // A session EKS Pod Identity tagged carries those tags into the decision, which is
        // what an association that turns them off takes away.
        let namespace_tag = detail["requestContext"][NAMESPACE_TAG_KEY].as_str();
        assert_eq!(
            namespace_tag,
            row.session_policy.is_none().then_some(NAMESPACE),
            "case {:?}: expected the pod identity session tags of a session that carries \
             them and none otherwise, got {detail:?}",
            row.case
        );

        // The documents the decision was made against: one entry per document in force on
        // the role, and one per session policy the credential was minted with, attached to
        // the session itself.
        let identity_based = policies_of(&detail, IDENTITY_BASED_POLICY);
        assert_eq!(
            identity_based.len(),
            row.role_policies.len(),
            "case {:?}: expected one entry per document in force on the role, got {detail:?}",
            row.case
        );
        for (policy_name, _) in &row.role_policies {
            assert!(
                identity_based.iter().any(|policy| {
                    policy["inline"].as_bool() == Some(true)
                        && policy["uri"]
                            .as_str()
                            .is_some_and(|uri| uri.ends_with(policy_name))
                }),
                "case {:?}: expected the evaluated policies to carry {policy_name:?}, got \
                 {detail:?}",
                row.case
            );
        }
        let session_policies = policies_of(&detail, SESSION_POLICY);
        assert_eq!(
            session_policies.len(),
            usize::from(row.session_policy.is_some()),
            "case {:?}: expected one entry per session policy the mint scoped the credential \
             with, got {detail:?}",
            row.case
        );
        for policy in &session_policies {
            assert_eq!(
                policy["attachedTo"][0]["arn"].as_str(),
                Some(session_arn.as_str()),
                "case {:?}: expected the session policy to be attached to the session, got \
                 {policy:?}",
                row.case
            );
        }
    }

    // Every refusal is pointed at a record of its own.
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "expected every refusal to mint its own authorization id, got {minted:?}"
    );
    assert!(
        minted.len() >= 4,
        "expected every refused row to mint an id, got {minted:?}"
    );
}
