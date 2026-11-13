// SPDX-License-Identifier: CC0-1.0

//! Two services, one principal: the identity STS attributes to a caller is a record THIS
//! account's IAM holds, so a client that reads its own identity can then manage it.
//!
//! `GetCallerIdentity` "returns details about the IAM user or role whose credentials are
//! used to call the operation", and the ARN it answers names an IAM entity of the account
//! — a user for a long-term access key, the assumed role for a session. The documented
//! self-service pattern reads that ARN, takes the entity's name out of it and embeds an
//! inline policy on it (grant `iam:PassRole` to a service, then call the service that
//! needs it), so the service that REPORTS the caller and the service that HOLDS it have
//! to agree about who it is: a name only one of them knows makes that pattern impossible,
//! and a crate-local suite can only ever pin its own half, which is why this one lives
//! here.
//!
//! The rows vary what the rule generalises over:
//!
//! * the ENTITY the credential resolves to — the IAM user behind a long-term access key,
//!   and the role behind a session `AssumeRole` minted — since the ARN's form, the IAM
//!   operations that address it and the inline-policy calls differ for each;
//! * the CASE the name is spelled in, because IAM entity names "are not distinguished by
//!   case";
//! * the policy DOCUMENT embedded on it: a `PassRole` grant to a service principal, a
//!   wildcard allow, a document carrying an explicit `Deny` beside an `Allow`, and one
//!   carrying a `Condition` — and the policy NAME's case with it.
//!
//! The negative control is a name the account does not hold, which still answers
//! `NoSuchEntity`: the rule is that the CALLER's own principal is a record of the
//! account, not that every name resolves.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The form encoder that builds an awsQuery body, and the readers that pull an element's
// text out of the XML answer, are this test crate's own (`query_wire`).
use crate::query_wire::{elements, form_encode, opt_element};

/// A trust policy that grants the account root permission to assume a role, so the
/// session row below can be minted at all.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The access key id the long-term row presents: a key that names no session this
/// emulator minted and no access key IAM holds, which is the position the credential the
/// emulator publishes at start is in.
const LONG_TERM_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The grant the fixture role carries so the session minted on it may manage its own
/// principal: every action of this service, on every resource. IAM decides a call to
/// itself against the caller's identity-based policies, and a session's are the role's.
const SELF_MANAGEMENT_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Action":"iam:*","Resource":"*"}]}"#
);

/// The type prefix AWS gives an IAM user's unique id, from the IAM "unique identifier
/// prefixes" table.
const USER_ID_PREFIX: &str = "AIDA";

/// The type prefix AWS gives an IAM role's unique id, from the same table.
const ROLE_ID_PREFIX: &str = "AROA";

/// How long an AWS unique id is: the four-character type prefix plus 17 more.
const UNIQUE_ID_LEN: usize = 21;

/// The inline policy documents a row embeds on the caller's own principal, each paired
/// with the policy name it is embedded under.
///
/// They span the IAM JSON policy grammar a self-service grant is written in: the
/// `iam:PassRole` grant to a service principal that a client needs before calling a
/// service on its behalf, a bare wildcard allow, an `Allow` beside an explicit `Deny`,
/// and a statement carrying a `Condition` — and the names span the case folding IAM
/// applies to a policy name.
fn documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "SelfPassRole",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:PassRole","Resource":"*","Condition":{"StringEquals":{"iam:PassedToService":"acm-acme.amazonaws.com"}}}]}"#
                .to_string(),
        ),
        (
            "self-read-everything",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "SELF_MIXED",
            concat!(
                r#"{"Version":"2012-10-17","Statement":["#,
                r#"{"Effect":"Allow","Action":["s3:GetObject","s3:ListBucket"],"Resource":"*"},"#,
                r#"{"Effect":"Deny","Action":"s3:DeleteObject","Resource":"*"}]}"#
            )
            .to_string(),
        ),
        (
            "self.conditioned",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"OnlyInRegion","Effect":"Allow","Action":"acm:DescribeCertificate","Resource":"*","Condition":{"StringEquals":{"aws:RequestedRegion":"us-east-1"}}}]}"#
                .to_string(),
        ),
    ]
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// `access_key_id`. The signature is never checked: the router here runs with request
/// authentication off, and the access key id is what resolves the caller.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The emulator with IAM and STS mounted on ONE engine, as `core/router/src/lib.rs`
/// mounts them.
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
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call against `signing_name`, signed (in the routing sense) with
/// `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM action, driven by the caller the row is about.
async fn iam(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        "iam",
        "2010-05-08",
        action,
        parameters,
    )
    .await
}

/// One STS action, signed with `access_key_id`.
async fn sts(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        "sts",
        "2011-06-15",
        action,
        parameters,
    )
    .await
}

/// The member, or a panic naming the case and the whole body.
fn member(case: &str, body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The member with the XML escaping decoded back and the percent-encoding IAM reports a
/// POLICY DOCUMENT in undone, for one that is itself a document — a policy, whose quotes the
/// serializer escapes and which IAM returns "URL-encoded compliant with RFC 3986".
fn decoded_member(case: &str, body: &str, name: &str) -> String {
    let reported = elements(body, name)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"));
    ::smithy_server_runtime::percent_decode(&reported)
}

/// The awsQuery error code an error response carries.
fn error_code(body: &str) -> String {
    opt_element(body, "Code").unwrap_or_default()
}

/// Assert `id` is an AWS unique id of `prefix`'s type: the prefix plus 17 characters, all
/// inside the modelled `idType` pattern `^[\w]+$`.
fn assert_unique_id_shape(case: &str, prefix: &str, id: &str) {
    assert!(
        id.starts_with(prefix),
        "case {case:?}: expected the unique id to start with {prefix}, got {id:?}"
    );
    assert_eq!(
        id.chars().count(),
        UNIQUE_ID_LEN,
        "case {case:?}: expected {UNIQUE_ID_LEN} characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()),
        "case {case:?}: expected idType's ^[\\w]+$ alphabet, got {id:?}"
    );
}

/// Which IAM entity a caller's identity ARN names, and the operation family that
/// addresses it: the two forms `GetCallerIdentity` answers with.
struct Entity {
    /// The entity's name, as a client takes it out of the ARN.
    name: String,
    /// `User` or `Role`: the infix every IAM operation addressing it carries
    /// (`GetUser`/`GetRole`, `PutUserPolicy`/`PutRolePolicy`, …).
    kind: &'static str,
    /// The type prefix the entity's unique id carries.
    id_prefix: &'static str,
}

/// The IAM entity `arn` names, read the way the documented self-service pattern reads it:
/// a user ARN carries `:user/<path…><name>`, an assumed-role ARN
/// `:assumed-role/<role>/<session>`, and only the last path segment of a user ARN (the
/// first segment after the role prefix of a role ARN) is the entity's name.
fn entity_of(case: &str, arn: &str) -> Entity {
    if let Some((_, resource)) = arn.split_once(":user/") {
        return Entity {
            name: resource
                .rsplit('/')
                .next()
                .expect("a user ARN names a user")
                .to_string(),
            kind: "User",
            id_prefix: USER_ID_PREFIX,
        };
    }
    if let Some((_, resource)) = arn.split_once(":assumed-role/") {
        return Entity {
            name: resource
                .split('/')
                .next()
                .expect("an assumed-role ARN names a role")
                .to_string(),
            kind: "Role",
            id_prefix: ROLE_ID_PREFIX,
        };
    }
    panic!("case {case:?}: expected a user or assumed-role ARN from GetCallerIdentity, got {arn:?}")
}

/// Read the caller's own identity, as a client does before it manages that identity.
async fn caller_identity(
    router: &MultiServiceRouter,
    case: &str,
    access_key_id: &str,
) -> (String, String, String) {
    let (status, body) = sts(router, access_key_id, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} with body {body}"
    );
    (
        member(case, &body, "Arn"),
        member(case, &body, "UserId"),
        member(case, &body, "Account"),
    )
}

/// Every spelling of `name` the rule has to resolve: the one the ARN carried, and the two
/// case foldings of it — "user, group, role, and policy names must be unique within the
/// account. Names are not distinguished by case" (`CreateUser$UserName`).
fn spellings(name: &str) -> Vec<String> {
    let mut all = vec![
        name.to_string(),
        name.to_ascii_uppercase(),
        name.to_ascii_lowercase(),
    ];
    all.dedup();
    all
}

/// RULE: the entity `GetCallerIdentity`'s ARN names is a record this account's IAM holds,
/// so reading it and embedding an inline policy on it both succeed, for every credential
/// the emulator resolves a principal for.
#[tokio::test]
async fn the_principal_sts_reports_is_an_iam_entity_of_the_account() {
    let router = build_router();
    let account = engine::emulator_account_id();

    // The role the session row is minted on, so that row's caller is an assumed role of
    // THIS account rather than the long-term user.
    let (status, created) = iam(
        &router,
        LONG_TERM_KEY,
        "CreateRole",
        &[
            ("RoleName", "deployer".to_string()),
            ("Path", "/svc/".to_string()),
            ("AssumeRolePolicyDocument", TRUST_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreateRole must succeed, got {status} with body {created}"
    );
    let role_arn = member("the fixture role", &created, "Arn");
    // IAM authorizes its own API, so the session minted below is a principal that needs a
    // grant to manage anything — including its own role. The grant is attached as a
    // MANAGED policy, by the account's own credential, before the role is assumed: an
    // INLINE one would show up in the `List*Policies` assertions further down, which
    // compare against exactly the documents each row embeds.
    let (status, policy) = iam(
        &router,
        LONG_TERM_KEY,
        "CreatePolicy",
        &[
            ("PolicyName", "SessionSelfManagement".to_string()),
            ("PolicyDocument", SELF_MANAGEMENT_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreatePolicy must succeed, got {status} with body {policy}"
    );
    let (status, attached) = iam(
        &router,
        LONG_TERM_KEY,
        "AttachRolePolicy",
        &[
            ("RoleName", "deployer".to_string()),
            ("PolicyArn", member("the fixture policy", &policy, "Arn")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AttachRolePolicy must succeed, got {status} with body {attached}"
    );
    let (status, assumed) = sts(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", "deploy-1".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AssumeRole must succeed, got {status} with body {assumed}"
    );
    let session_key = member("the fixture session", &assumed, "AccessKeyId");

    // The rows: the two credential classes this emulator resolves a principal for.
    let rows: [(&str, &str); 2] = [
        ("the long-term access key", LONG_TERM_KEY),
        ("a session assumed on a role of the account", &session_key),
    ];

    for (case, access_key_id) in rows {
        let (arn, user_id, reported_account) = caller_identity(&router, case, access_key_id).await;
        assert_eq!(
            reported_account, account,
            "case {case:?}: expected the caller's account, got {reported_account:?}"
        );
        let entity = entity_of(case, &arn);

        // Every spelling of the name IAM accepts resolves the SAME record, and that
        // record's own ARN is the one the identity reported (for a role, the identity
        // reports the SESSION's ARN, so the entity ARN is compared through its name).
        for spelling in spellings(&entity.name) {
            let case = format!("{case} read as {spelling:?}");
            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("Get{}", entity.kind),
                &[(
                    &format!("{}Name", entity.kind),
                    spelling.clone(),
                )],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: IAM must hold the principal STS reported ({arn}), got {status} \
                 with body {body}"
            );
            let held_name = member(&case, &body, &format!("{}Name", entity.kind));
            assert_eq!(
                held_name.to_ascii_lowercase(),
                entity.name.to_ascii_lowercase(),
                "case {case:?}: expected the record of {:?}, got {held_name:?}",
                entity.name
            );
            let held_id = member(&case, &body, &format!("{}Id", entity.kind));
            assert_unique_id_shape(&case, entity.id_prefix, &held_id);
            if entity.kind == "User" {
                assert_eq!(
                    member(&case, &body, "Arn"),
                    arn,
                    "case {case:?}: expected the stored ARN to be the one the identity \
                     reported, got {body}"
                );
                assert_eq!(
                    held_id, user_id,
                    "case {case:?}: expected the stored UserId to be the one the identity \
                     reported ({user_id}), got {held_id:?}"
                );
            } else {
                assert_eq!(
                    user_id,
                    format!("{held_id}:deploy-1"),
                    "case {case:?}: expected the session id to be the stored RoleId and the \
                     session name, got {user_id:?}"
                );
            }
        }

        // And the principal can be MANAGED: each document is embedded under its own name,
        // read back verbatim, enumerated, and removed again.
        let mut embedded: Vec<String> = Vec::new();
        for (policy_name, document) in documents() {
            let case = format!("{case} granting {policy_name:?}");
            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("Put{}Policy", entity.kind),
                &[
                    (&format!("{}Name", entity.kind), entity.name.clone()),
                    ("PolicyName", policy_name.to_string()),
                    ("PolicyDocument", document.clone()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a caller must be able to grant its own principal a scoped \
                 inline policy, got {status} with body {body}"
            );

            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("Get{}Policy", entity.kind),
                &[
                    (&format!("{}Name", entity.kind), entity.name.clone()),
                    ("PolicyName", policy_name.to_string()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 reading the embedded policy back, got \
                 {status} with body {body}"
            );
            assert_eq!(
                decoded_member(&case, &body, "PolicyDocument"),
                document,
                "case {case:?}: expected the document stored verbatim, got {body}"
            );
            assert_eq!(
                member(&case, &body, "PolicyName"),
                policy_name,
                "case {case:?}: expected the policy name the put named, got {body}"
            );

            embedded.push(policy_name.to_string());
            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("List{}Policies", entity.kind),
                &[(&format!("{}Name", entity.kind), entity.name.clone())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 listing the principal's policies, got \
                 {status} with body {body}"
            );
            let mut listed = elements(&body, "member");
            listed.sort_by_key(|name| name.to_ascii_lowercase());
            let mut expected = embedded.clone();
            expected.sort_by_key(|name| name.to_ascii_lowercase());
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the policies embedded so far, got {listed:?}"
            );
        }

        for policy_name in &embedded {
            let case = format!("{case} revoking {policy_name:?}");
            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("Delete{}Policy", entity.kind),
                &[
                    (&format!("{}Name", entity.kind), entity.name.clone()),
                    ("PolicyName", policy_name.clone()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 removing the policy, got {status} with body \
                 {body}"
            );
            let (status, body) = iam(
                &router,
                access_key_id,
                &format!("Get{}Policy", entity.kind),
                &[
                    (&format!("{}Name", entity.kind), entity.name.clone()),
                    ("PolicyName", policy_name.clone()),
                ],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the removed policy to be gone, got {status} with body \
                 {body}"
            );
        }
    }

    // The negative control: the rule is that the CALLER's principal is a record of the
    // account, not that any name resolves. A name nothing created still answers
    // `NoSuchEntity`.
    for absent in ["nobody", "emulator-2", "deployer-2"] {
        let (status, body) = iam(
            &router,
            LONG_TERM_KEY,
            "PutUserPolicy",
            &[
                ("UserName", absent.to_string()),
                ("PolicyName", "SelfPassRole".to_string()),
                ("PolicyDocument", documents()[0].1.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected HTTP 404 for a user the account does not hold, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {body}"
        );
    }
}
