// SPDX-License-Identifier: CC0-1.0

//! The SESSION POLICIES a temporary credential was minted with, enforced at this service's
//! own gate, driven over the real wire path.
//!
//! THE RULE: the permissions of a request signed with a temporary credential are the
//! INTERSECTION of the identity's own policies and the session policies its mint passed —
//! "the resulting session's permissions are the intersection of the role's identity-based
//! policy and the session policies… You cannot use session policies to grant more
//! permissions than those allowed by the identity-based policy of the role that is being
//! assumed" (`AssumeRoleRequest$PolicyArns`, the STS model) — and an explicit `Deny`
//! anywhere refuses whatever a grant says. So a session policy TAKES permissions away and
//! grants none of its own: an action the role's policies allow and the session policy does
//! not is `AccessDeniedException`, and an action only the session policy allows is refused
//! as if it had allowed nothing.
//!
//! The scope is the one the MINT recorded on the credential, read through the tree's one
//! resolution of it, so what this service enforces is what `sts:AssumeRole` /
//! `sts:GetFederationToken` were passed rather than a document this suite poked into a
//! store of its own.
//!
//! The table spans what the rule generalises over: which document carries the scope (an
//! inline session policy and a managed one named by ARN), what it says about the action (a
//! grant of this one, a grant of a DIFFERENT action of this service, a grant of another
//! service's actions only, an explicit deny beside the role's allow, a grant scoped to
//! another secret), which operation asks (an account-wide list and a read of one secret),
//! and whether the grant the scope caps came from the identity's policies or from the
//! secret's own resource-based policy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{Policy, PolicyVersion};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, role_arn, router_with_identity,
    session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS, ACCOUNT_KEY_ID, SECRET_VALUE,
};

/// The access key id every scoped session of this suite is minted under.
const SCOPED_KEY_ID: &str = "ASIAEXAMPLESCOPED001";

/// The role the scoped sessions are sessions of, as the sibling suite names it.
const ROLE: &str = "secretsmanager-reader";

/// How long a seeded session lasts: an hour, STS's own default for `AssumeRole`.
const SESSION_SECONDS: i32 = 3_600;

/// The name of the managed session policy a row names by ARN, and the ARN that names it.
const MANAGED_POLICY_NAME: &str = "scoped-reads";

/// The secret every row addresses.
const SECRET_NAME: &str = "scoped/secret";

/// The ARN the account holds [`MANAGED_POLICY_NAME`] under, as `iam:CreatePolicy` mints one.
fn managed_policy_arn() -> String {
    iam::policy_arn("/", MANAGED_POLICY_NAME)
}

/// A policy document allowing `actions` on `resource`.
fn allow(actions: Value, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }],
    })
}

/// A policy document allowing everything of this service and DENYING `actions`.
fn allow_all_but(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "secretsmanager:*", "Resource": "*" },
            { "Effect": "Deny", "Action": actions, "Resource": "*" },
        ],
    })
}

/// Record the role the sessions below are sessions of, as `iam:CreateRole` records one.
fn given_role(engine: &Arc<engine::InMemoryEngine>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLESECRETSROLE".to_string(),
            arn: role_arn(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
}

/// Put `document` in force as the account's managed policy [`MANAGED_POLICY_NAME`], the way
/// `iam:CreatePolicy` records one: a managed session policy named by ARN stands for whatever
/// its default version says at the instant of the request.
fn given_managed_policy(engine: &Arc<engine::InMemoryEngine>, document: &Value) {
    let arn = managed_policy_arn();
    let version = iam::FIRST_POLICY_VERSION_ID.to_string();
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().policies.insert(
        iam::name_key(MANAGED_POLICY_NAME),
        iam::ManagedPolicy {
            policy: Policy {
                policy_name: Some(MANAGED_POLICY_NAME.to_string()),
                policy_id: Some("ANPAEXAMPLESCOPEDPOLICY".to_string()),
                arn: Some(arn),
                path: Some("/".to_string()),
                default_version_id: Some(version.clone()),
                is_attachable: true,
                ..Policy::default()
            },
            versions: BTreeMap::from([(
                version.clone(),
                PolicyVersion {
                    document: Some(document.to_string()),
                    version_id: Some(version),
                    is_default_version: true,
                    ..PolicyVersion::default()
                },
            )]),
        },
    );
}

/// Record the assumed-role session [`SCOPED_KEY_ID`] speaks for, scoped by `scope` — what
/// `sts:AssumeRole` records for a mint that passed session policies.
fn given_scoped_session(engine: &Arc<engine::InMemoryEngine>, scope: sts::SessionPolicies) {
    let account = ::engine::emulator_account_id();
    let vend = sts::NewSession {
        role_arn: Some(role_arn()),
        policies: scope,
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn()),
                account: account.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SCOPED_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Create the secret every row addresses with the account's own credential, which this gate
/// serves, and report the ARN the account holds it under.
async fn given_secret(router: &MultiServiceRouter) -> String {
    let (status, created) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "CreateSecret",
        &json!({ "Name": SECRET_NAME, "SecretString": SECRET_VALUE }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seeding the secret: expected 200, got {status} {created}"
    );
    created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case seeding the secret: no ARN in {created}"))
        .to_string()
}

/// How the scope carries one inline document.
fn inline(document: &Value) -> sts::SessionPolicies {
    sts::SessionPolicies {
        inline: Some(document.to_string()),
        managed_arns: Vec::new(),
        // An assumed-role session keeps the ROLE's permissions, so an empty scope here
        // would cap nothing; these rows all carry a document anyway.
        none_means_no_permissions: false,
    }
}

/// How it carries one managed policy, named by ARN.
fn managed() -> sts::SessionPolicies {
    sts::SessionPolicies {
        inline: None,
        managed_arns: vec![managed_policy_arn()],
        none_means_no_permissions: false,
    }
}

/// The request `operation` sends, addressing `secret` where it addresses one.
fn request_body(operation: &str, secret: &str) -> Value {
    match operation {
        "ListSecrets" => json!({ "MaxResults": 1 }),
        "GetSecretValue" | "DescribeSecret" => json!({ "SecretId": secret }),
        other => panic!("case {other:?}: this suite states no request for that operation"),
    }
}

/// RULE: a request signed with a scoped credential is allowed only where the identity's
/// policies AND the session policies both allow it; an explicit `Deny` in either refuses it.
///
/// The role's own policy allows every action of this service in every row, so what each row
/// varies is the SCOPE — which is what makes the outcome the scope's decision rather than
/// the role's.
#[tokio::test]
async fn a_scoped_credential_is_allowed_only_where_its_session_policy_also_allows() {
    let (router, engine) = router_with_identity();
    given_role(&engine);
    // The ROLE may do everything of this service, so every refusal below is the scope's.
    given_role_policy(&engine, Some(&allow(json!("secretsmanager:*"), "*")));
    let secret_arn = given_secret(&router).await;
    given_managed_policy(
        &engine,
        &allow(json!("secretsmanager:GetSecretValue"), &secret_arn),
    );

    // One row: the case, the operation, the scope the credential was minted with, and
    // whether the call is served.
    let rows: Vec<(&str, &str, sts::SessionPolicies, bool)> = vec![
        (
            "an inline scope granting exactly the action asked for",
            "ListSecrets",
            inline(&allow(json!("secretsmanager:ListSecrets"), "*")),
            true,
        ),
        (
            "an inline scope granting a DIFFERENT action of this service",
            "ListSecrets",
            inline(&allow(json!("secretsmanager:GetSecretValue"), "*")),
            false,
        ),
        (
            // The recorded shape of the report: a session scoped to one action of ANOTHER
            // service has no permission here at all.
            "an inline scope granting only another service's action",
            "ListSecrets",
            inline(&allow(json!("s3:ListAllMyBuckets"), "*")),
            false,
        ),
        (
            "an inline scope granting every action of this service",
            "ListSecrets",
            inline(&allow(json!("secretsmanager:*"), "*")),
            true,
        ),
        (
            "an inline scope granting everything",
            "GetSecretValue",
            inline(&allow(json!("*"), "*")),
            true,
        ),
        (
            "an inline scope whose explicit deny covers the action the role allows",
            "GetSecretValue",
            inline(&allow_all_but(json!("secretsmanager:GetSecretValue"))),
            false,
        ),
        (
            "an inline scope granting the action on ANOTHER secret",
            "GetSecretValue",
            inline(&allow(
                json!("secretsmanager:GetSecretValue"),
                "arn:aws:secretsmanager:us-west-2:000000000000:secret:other-AbCdEf",
            )),
            false,
        ),
        (
            "an inline scope granting the action on THIS secret",
            "GetSecretValue",
            inline(&allow(json!("secretsmanager:GetSecretValue"), &secret_arn)),
            true,
        ),
        (
            "a MANAGED scope granting the action on this secret",
            "GetSecretValue",
            managed(),
            true,
        ),
        (
            "that same managed scope, which does not grant the account-wide list",
            "ListSecrets",
            managed(),
            false,
        ),
    ];

    for (case, operation, scope, served) in rows {
        given_scoped_session(&engine, scope);
        let (status, body) = call_as(
            &router,
            SCOPED_KEY_ID,
            operation,
            &request_body(operation, &secret_arn),
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {operation} to be served, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            "case {case:?}: expected the {operation} to be refused, got {status} {body}"
        );
        let message = error_message(&body);
        assert!(
            message.starts_with(&format!(
                "User: {} is not authorized to perform: secretsmanager:{operation}",
                session_arn()
            )),
            "case {case:?}: expected the refusal to name the session and the action, got \
             {message:?}"
        );
        assert!(
            message.contains("session policy"),
            "case {case:?}: expected the refusal to blame the session policy, got {message:?}"
        );
    }
}

/// RULE: the scope caps a grant WHEREVER it came from — including one in the secret's own
/// resource-based policy, which stands on its own against a permissions boundary but not
/// against the session policy, since a session's permissions are the intersection its mint
/// fixed.
#[tokio::test]
async fn the_scope_caps_a_grant_in_the_secrets_own_policy_too() {
    let (router, engine) = router_with_identity();
    given_role(&engine);
    // The role itself is granted NOTHING, so the only grant in play is the secret's own.
    given_role_policy(&engine, None);
    let secret_arn = given_secret(&router).await;
    let (status, body) = call_as(
        &router,
        ACCOUNT_KEY_ID,
        "PutResourcePolicy",
        &json!({
            "SecretId": secret_arn,
            "ResourcePolicy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": role_arn() },
                    "Action": "secretsmanager:GetSecretValue",
                    "Resource": "*",
                }],
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seeding the resource policy: expected 200, got {status} {body}"
    );

    // One row: the case, the scope, and whether the read is served.
    let rows: Vec<(&str, sts::SessionPolicies, bool)> = vec![
        (
            "no session policy at all, so the resource grant stands",
            sts::SessionPolicies::default(),
            true,
        ),
        (
            "a session policy that allows the read",
            inline(&allow(json!("secretsmanager:GetSecretValue"), "*")),
            true,
        ),
        (
            "a session policy that allows something else",
            inline(&allow(json!("secretsmanager:DescribeSecret"), "*")),
            false,
        ),
        (
            "a session policy that denies the read",
            inline(&allow_all_but(json!("secretsmanager:GetSecretValue"))),
            false,
        ),
    ];

    for (case, scope, served) in rows {
        given_scoped_session(&engine, scope);
        let (status, body) = call_as(
            &router,
            SCOPED_KEY_ID,
            "GetSecretValue",
            &json!({ "SecretId": secret_arn }),
        )
        .await;
        if served {
            assert_eq!(
                (status, body["SecretString"].as_str()),
                (200, Some(SECRET_VALUE)),
                "case {case:?}: expected the stored value, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            "case {case:?}: expected the read to be refused, got {status} {body}"
        );
        assert!(
            !body.to_string().contains(SECRET_VALUE),
            "case {case:?}: a refused read must disclose no material: {body}"
        );
    }
}
