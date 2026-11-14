// SPDX-License-Identifier: CC0-1.0

//! ECR resolves the identities a resource-based policy GRANTS TO against this account's
//! IAM.
//!
//! RULE: a resource-based policy document states WHO a permission is granted to, so ECR
//! resolves each identity a submitted `policyText` names against this account's IAM before
//! it stores the document and refuses the request with the operation's modelled
//! `InvalidParameterException` (HTTP 400) — leaving the policy the resource already carries
//! exactly as it was — when IAM answers that one of those identities is absent. A refusal
//! has to be grounded in what IAM ANSWERED, so a principal that names no IAM role or user of
//! this account — the `*` wildcard, an account id or an account's root, a service or
//! federated principal, an identity of ANOTHER account whose IAM this emulator is not the
//! one to ask — is resolved by nothing and refuses nothing.
//!
//! Both documents ECR stores go through that rule: a repository policy
//! (`SetRepositoryPolicy`) and the registry policy (`PutRegistryPolicy`).
//!
//! The principals are resolved through the in-process callback, so a single-service router
//! has no IAM to ask: that is why this suite lives here rather than in the service's own
//! crate. The rows span what the rule generalises over — the IAM entity type the ARN names
//! (role, user, an account's root), the path the entity lives under, the form the
//! `Principal` element takes (a bare string, a typed object, a list), the principal type key
//! (`AWS`, `Service`, `Federated`), which account the ARN names, and whether the entity was
//! ever created.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The form encoder this suite needs for its awsQuery calls to IAM is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The region every request here is signed for.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`), which is also
/// this registry's id.
const ACCOUNT: &str = "000000000000";

/// A syntactically valid account id (`RegistryId` is `^[0-9]{12}$`) that is NOT this one.
const OTHER_ACCOUNT: &str = "123456789012";

/// The `X-Amz-Target` prefix ECR routes on.
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// What ECR answers for a policy document naming an identity it cannot resolve: the
/// service's refusal template (`Invalid parameter at '<Parameter>' failed to satisfy
/// constraint: '<constraint>'`) over the API-documented label of the `policyText` parameter.
const PRINCIPAL_NOT_FOUND: &str =
    "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Principal not found'";

/// ECR and IAM over ONE shared engine, as `router/src/lib.rs` mounts them, so ECR's
/// in-process callback reaches this very IAM.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM: `POST /` with the parameters in the form body.
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

/// One awsJson1_1 call to ECR, answered with the status and the JSON body.
async fn ecr(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("api.ecr.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
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

/// The awsJson error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A policy document granting the pull actions to `principal`, whatever form that element
/// takes.
fn policy(principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowPull",
            "Effect": "Allow",
            "Principal": principal,
            "Action": ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer"],
        }]
    })
    .to_string()
}

/// The ARN of an IAM entity of `account`: `arn:aws:iam::<account>:<resource>`.
fn iam_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:iam::{account}:{resource}")
}

/// A trust policy any service may assume, so the fixture roles are ordinary roles: what this
/// suite asks IAM is whether the entity EXISTS, not what may assume it.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ecs-tasks.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }]
    })
    .to_string()
}

/// Create the IAM entities the rows name: two roles (one under the default path, one under a
/// path) and one user.
async fn create_identities(router: &MultiServiceRouter) {
    let document = trust_policy();
    for (role_name, path) in [("pull-role", None), ("path-role", Some("/service-role/"))] {
        let mut parameters = vec![
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", document.as_str()),
        ];
        if let Some(path) = path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, "CreateRole", &parameters).await;
        assert_eq!(
            status, 200,
            "setup: CreateRole {role_name} must succeed, got {status} {body}"
        );
    }
    let (status, body) = iam(router, "CreateUser", &[("UserName", "builder")]).await;
    assert_eq!(
        status, 200,
        "setup: CreateUser builder must succeed, got {status} {body}"
    );
}

/// One row: the `Principal` element a policy names, and whether the document must be stored.
struct PrincipalCase {
    case: &'static str,
    principal: Value,
    accepted: bool,
    /// The `Principal` element the STORED document names, where ECR respells it: a whole
    /// account named by its bare id is held as that account's root ARN, the other spelling
    /// of the same principal (the ECR crate's own
    /// `policy_document_account_principal` suite pins that rule). `None` for a row stored
    /// exactly as it was submitted.
    stored_principal: Option<Value>,
}

fn principal_cases() -> Vec<PrincipalCase> {
    let row = |case: &'static str, principal: Value, accepted: bool| PrincipalCase {
        case,
        principal,
        accepted,
        stored_principal: None,
    };
    // The same row, with the principal the stored document names.
    let stored = |mut row: PrincipalCase, principal: Value| {
        row.stored_principal = Some(principal);
        row
    };
    vec![
        row(
            "a role of this account that exists",
            json!({"AWS": iam_arn(ACCOUNT, "role/pull-role")}),
            true,
        ),
        row(
            "a role of this account under a path",
            json!({"AWS": iam_arn(ACCOUNT, "role/service-role/path-role")}),
            true,
        ),
        row(
            "a user of this account that exists",
            json!({"AWS": iam_arn(ACCOUNT, "user/builder")}),
            true,
        ),
        row(
            "a role of this account that was never created",
            json!({"AWS": iam_arn(ACCOUNT, "role/ghost-role")}),
            false,
        ),
        row(
            "a role under a path that was never created",
            json!({"AWS": iam_arn(ACCOUNT, "role/service-role/ghost-role")}),
            false,
        ),
        row(
            "a user of this account that was never created",
            json!({"AWS": iam_arn(ACCOUNT, "user/ghost-user")}),
            false,
        ),
        row(
            // A repository is shared publicly by granting to every principal.
            "the wildcard, as a typed object",
            json!({"AWS": "*"}),
            true,
        ),
        row("the wildcard, as a bare string", json!("*"), true),
        row(
            "this account's root, which names no role or user",
            json!({"AWS": iam_arn(ACCOUNT, "root")}),
            true,
        ),
        stored(
            row("this account by id", json!({"AWS": ACCOUNT}), true),
            json!({"AWS": iam_arn(ACCOUNT, "root")}),
        ),
        row(
            // Another account's IAM is not this emulator's to ask, so nothing is known
            // about the identity and the document is not refused on an assumption.
            "a role of another account that was never created",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "role/ghost-role")}),
            true,
        ),
        row(
            "another account's root",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "root")}),
            true,
        ),
        row(
            "a service principal",
            json!({"Service": "lambda.amazonaws.com"}),
            true,
        ),
        row(
            "a federated principal, which is no IAM role or user",
            json!({"Federated": iam_arn(ACCOUNT, "saml-provider/corp")}),
            true,
        ),
        row(
            "a list whose identities all exist",
            json!({"AWS": [
                iam_arn(ACCOUNT, "role/pull-role"),
                iam_arn(ACCOUNT, "user/builder"),
            ]}),
            true,
        ),
        row(
            "a list in which one identity is absent",
            json!({"AWS": [
                iam_arn(ACCOUNT, "role/pull-role"),
                iam_arn(ACCOUNT, "role/ghost-role"),
            ]}),
            false,
        ),
        row(
            "two principal types together, one of them absent",
            json!({
                "Service": "lambda.amazonaws.com",
                "AWS": iam_arn(ACCOUNT, "user/ghost-user"),
            }),
            false,
        ),
    ]
}

/// The document a repository already carries before each row's set is attempted, so a
/// refusal can be shown to leave it in place.
fn baseline_policy() -> String {
    policy(json!({"AWS": iam_arn(ACCOUNT, "role/pull-role")}))
}

/// The CONTENT of the policy document `document` holds, as a comparison can see it: ECR
/// reports an attached policy re-indented in its canonical rendering, so what a stored
/// document is compared by here is the parsed JSON and not the layout — which the ECR
/// crate's own `policy_document_canonical_form` suite pins byte for byte. Every document
/// below writes its one-value elements as strings rather than as one-element lists, the
/// other difference that rendering makes.
fn content(document: &str) -> Value {
    serde_json::from_str::<Value>(document).unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_repository_policy_is_stored_only_when_iam_resolves_every_identity_it_grants_to() {
    let router = emulator();
    create_identities(&router).await;

    for (index, row) in principal_cases().into_iter().enumerate() {
        let repository = format!("principal-row-{index}");
        let (status, created) = ecr(
            &router,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: setup CreateRepository must succeed, got {status} ({created})",
            row.case
        );

        // Every row starts from a policy that IS stored, so a refusal below is visible as
        // the document the repository keeps.
        let baseline = baseline_policy();
        let (status, body) = ecr(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": repository, "policyText": &baseline}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: setup SetRepositoryPolicy must succeed, got {status} ({body})",
            row.case
        );

        let document = policy(row.principal.clone());
        let (status, body) = ecr(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": repository, "policyText": &document}),
        )
        .await;
        let expected_stored = if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?} ({}): the policy must be stored, got {status} ({body})",
                row.case, row.principal
            );
            // The document ECR holds names the principal in the spelling the store writes,
            // which differs from the submitted one only where the row says so.
            policy(
                row.stored_principal
                    .clone()
                    .unwrap_or_else(|| row.principal.clone()),
            )
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterException"),
                "case {:?} ({}): an identity IAM does not hold must be refused 400 \
                 InvalidParameterException, got {status} {} ({body})",
                row.case,
                row.principal,
                error_code(&body)
            );
            assert_eq!(
                text(&body, "message"),
                PRINCIPAL_NOT_FOUND,
                "case {:?}: the refusal names the parameter and the constraint, got {}",
                row.case,
                text(&body, "message")
            );
            baseline.clone()
        };

        // What the repository holds: the row's document when it was accepted, and the
        // untouched baseline when it was refused.
        let (status, read) = ecr(
            &router,
            "GetRepositoryPolicy",
            &json!({"repositoryName": repository}),
        )
        .await;
        assert_eq!(
            (status, content(&text(&read, "policyText"))),
            (200, content(&expected_stored)),
            "case {:?}: the repository must hold {expected_stored}, got {status} ({read})",
            row.case
        );
    }
}

/// Every statement of the document is resolved, wherever the absent identity sits in the
/// list — and the whole document is refused, not the one statement.
#[tokio::test]
async fn an_absent_identity_is_found_in_any_statement_of_the_document() {
    let router = emulator();
    create_identities(&router).await;

    let repository = "multi-statement";
    let (status, created) = ecr(
        &router,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateRepository must succeed, got {status} ({created})"
    );

    let statement = |sid: &str, principal: Value| {
        json!({
            "Sid": sid,
            "Effect": "Allow",
            "Principal": principal,
            "Action": "ecr:BatchGetImage",
        })
    };
    let present = || json!({"AWS": iam_arn(ACCOUNT, "role/pull-role")});
    let absent = || json!({"AWS": iam_arn(ACCOUNT, "role/ghost-role")});

    for (case, statements, accepted) in [
        (
            "the absent identity in the first of three statements",
            vec![
                statement("one", absent()),
                statement("two", present()),
                statement("three", present()),
            ],
            false,
        ),
        (
            "the absent identity in the middle statement",
            vec![
                statement("one", present()),
                statement("two", absent()),
                statement("three", present()),
            ],
            false,
        ),
        (
            "the absent identity in the last statement",
            vec![
                statement("one", present()),
                statement("two", present()),
                statement("three", absent()),
            ],
            false,
        ),
        (
            "every statement naming an identity that exists",
            vec![
                statement("one", present()),
                statement("two", json!({"AWS": iam_arn(ACCOUNT, "user/builder")})),
                statement("three", json!("*")),
            ],
            true,
        ),
    ] {
        let document = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
        let (status, body) = ecr(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": repository, "policyText": &document}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the document must be stored, got {status} ({body})"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), text(&body, "message")),
                (
                    400,
                    "InvalidParameterException",
                    PRINCIPAL_NOT_FOUND.to_string()
                ),
                "case {case:?}: the whole document must be refused, got {status} ({body})"
            );
        }
    }
}

/// The registry policy is the OTHER resource-based policy this service stores, and the same
/// rule holds of it: a registry policy grants to other accounts, whose identities are not
/// resolved here, but one naming an identity of THIS account that IAM does not hold is
/// refused and the stored registry policy is left as it was.
#[tokio::test]
async fn the_registry_policy_resolves_the_identities_it_grants_to_as_well() {
    let router = emulator();
    create_identities(&router).await;

    // A registry policy may not grant to every principal, so each row names identities.
    let registry_policy = |principal: Value| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowReplication",
                "Effect": "Allow",
                "Principal": principal,
                "Action": "ecr:ReplicateImage",
                "Resource": "*",
            }]
        })
        .to_string()
    };

    let baseline = registry_policy(json!({"AWS": iam_arn(OTHER_ACCOUNT, "root")}));
    let (status, body) = ecr(
        &router,
        "PutRegistryPolicy",
        &json!({"policyText": &baseline}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup PutRegistryPolicy must succeed, got {status} ({body})"
    );

    // What the registry holds, which only an ACCEPTED put replaces.
    let mut stored = baseline;

    for (case, principal, accepted) in [
        (
            "a role of this account that exists",
            json!({"AWS": iam_arn(ACCOUNT, "role/pull-role")}),
            true,
        ),
        (
            "a role of this account that was never created",
            json!({"AWS": iam_arn(ACCOUNT, "role/ghost-role")}),
            false,
        ),
        (
            "another account's root, which this emulator's IAM is not asked about",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "root")}),
            true,
        ),
    ] {
        let document = registry_policy(principal.clone());
        let (status, body) = ecr(
            &router,
            "PutRegistryPolicy",
            &json!({"policyText": &document}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({principal}): the registry policy must be stored, got {status} \
                 ({body})"
            );
            stored = document;
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), text(&body, "message")),
                (
                    400,
                    "InvalidParameterException",
                    PRINCIPAL_NOT_FOUND.to_string()
                ),
                "case {case:?} ({principal}): an identity IAM does not hold must be refused, got \
                 {status} ({body})"
            );
        }

        let (status, read) = ecr(&router, "GetRegistryPolicy", &json!({})).await;
        assert_eq!(
            (status, content(&text(&read, "policyText"))),
            (200, content(&stored)),
            "case {case:?}: the registry must hold {stored}, got {status} ({read})"
        );
    }
}
