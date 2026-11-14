// SPDX-License-Identifier: CC0-1.0

//! One role, two services: the id STS reports for an assumed role IS the `RoleId` IAM
//! holds for that role.
//!
//! A role's unique id is generated when the ROLE is created, so IAM — the service that
//! holds the role — owns the value, and `AssumedRoleUser.AssumedRoleId` is the pair
//! `<that stored RoleId>:<RoleSessionName>`. That is what lets a client correlate a
//! session back to the role `CreateRole`/`GetRole` reported, so the two halves must be
//! read from one place rather than minted twice; a crate-local suite can only ever pin
//! its own half, which is why this one lives here.
//!
//! Beside the id itself, both services report an AWS role unique id of ONE form — the
//! `AROA` type prefix plus 17 characters of the uppercase base32 alphabet, 21 in total,
//! which is what the modelled `idType` (length 16..128, pattern `^[\w]+$`) admits — so a
//! client that parses or stores one sees the same shape whichever service produced it.
//!
//! The rows vary what the rule generalises over: the role's name and the path its ARN
//! embeds, the session name (its length, from `roleSessionNameType`'s 2-character floor
//! to its 64-character ceiling, and its character set), how many sessions one role has,
//! and how many roles the account holds.
//!
//! Cross-service routing for the `iam` signing name the two IAM-family services share is
//! covered by `shared_signing_name_routing.rs`; the legs it does not reach are in
//! `iam_signing_name_routing.rs`.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The form encoder that builds an awsQuery body, and the reader that pulls one element's
// text out of the XML answer, are this test crate's own (`query_wire`), written once there.
use crate::query_wire::{form_encode, opt_element};

/// A trust policy that grants the account root permission to assume a role. It travels
/// through the suite's form encoder like every other value the body carries.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The type prefix AWS gives a role's unique id.
const ROLE_ID_PREFIX: &str = "AROA";

/// How long an AWS unique id is: the four-character type prefix plus 17 more.
const ROLE_ID_LEN: usize = 21;

/// The access key id every call that is not signed with a minted session presents.
///
/// Shared with the sibling IAM/STS suites in this crate, which drive the same two services
/// over the same wire through the harness below.
pub(crate) const LONG_TERM_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

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
/// mounts them. STS reads the assumed role's id from IAM through the in-process
/// callback, which re-enters this same router.
pub(crate) fn build_router() -> MultiServiceRouter {
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

/// One IAM action.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    iam_as(router, LONG_TERM_KEY, action, parameters).await
}

/// One IAM action, signed with `access_key_id` — the credential IAM resolves the calling
/// principal from, which for a key STS minted is that session.
pub(crate) async fn iam_as(
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
pub(crate) async fn sts(
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
pub(crate) fn member(case: &str, body: &str, name: &str) -> String {
    opt_element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Create one role over IAM's awsQuery wire and return its `Arn` and `RoleId`.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) -> (String, String) {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", path.to_string()),
            ("AssumeRolePolicyDocument", TRUST_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from CreateRole, got {status} with body {body}"
    );
    (
        member(role_name, &body, "Arn"),
        member(role_name, &body, "RoleId"),
    )
}

/// Assert `id` is an AWS role unique id: the `AROA` prefix plus 17 characters, all of
/// them inside `idType`'s `^[\w]+$` pattern.
fn assert_role_id_shape(case: &str, source: &str, id: &str) {
    assert!(
        id.starts_with(ROLE_ID_PREFIX),
        "case {case:?}: expected {source}'s role id to start with {ROLE_ID_PREFIX}, got {id:?}"
    );
    assert_eq!(
        id.chars().count(),
        ROLE_ID_LEN,
        "case {case:?}: expected {source}'s role id to be {ROLE_ID_LEN} characters, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "case {case:?}: expected {source}'s role id to satisfy idType's ^[\\w]+$, got {id:?}"
    );
}

/// Assume `role_arn` as `session_name` and return the `AssumedRoleId` with the session's
/// access key id.
async fn assume_role(
    router: &MultiServiceRouter,
    case: &str,
    role_arn: &str,
    session_name: &str,
) -> (String, String) {
    let (status, body) = sts(
        router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn.to_string()),
            ("RoleSessionName", session_name.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssumeRole, got {status} with body {body}"
    );
    (
        member(case, &body, "AssumedRoleId"),
        member(case, &body, "AccessKeyId"),
    )
}

/// Every id STS reports for an assumed role is `<the RoleId IAM holds for that
/// role>:<RoleSessionName>`, for every role of the account and every session of it —
/// and `GetCallerIdentity` under the minted credential reports the same value as its
/// `UserId`.
#[tokio::test]
async fn an_assumed_role_id_is_the_role_id_iam_holds_for_the_role() {
    let router = build_router();

    // The rows differ in what goes into the role, and so into its ARN (the name and the
    // path the ARN embeds, `role<path><RoleName>`), and in the session names assumed on
    // it: one at `roleSessionNameType`'s 2-character floor, one at its 64-character
    // ceiling, one holding the punctuation the pattern `^[\w+=,.@-]*$` admits, and
    // several sessions of one role.
    let sixty_four = "s".repeat(64);
    let rows: [(&str, &str, &[&str]); 5] = [
        ("plain-role", "/", &["sim2real-trusted"]),
        ("plain-role-2", "/", &["ab", "probe", "a.b-c_d"]),
        ("pathed-role", "/engineering/", &["one"]),
        ("deeply-pathed-role", "/engineering/data/", &[&sixty_four]),
        (
            "punctuated.role-name@example",
            "/",
            &["user@example.com", "key=1+2,3"],
        ),
    ];

    let mut seen = Vec::new();
    for (role_name, path, session_names) in rows {
        let (arn, role_id) = create_role(&router, role_name, path).await;
        assert_role_id_shape(role_name, "IAM", &role_id);

        // What a client observes before the assume: the id IAM reports for the role.
        let (status, read_back) = iam(
            &router,
            "GetRole",
            &[("RoleName", role_name.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: expected HTTP 200 from GetRole, got {status} {read_back}"
        );
        assert_eq!(
            member(role_name, &read_back, "RoleId"),
            role_id,
            "case {role_name:?}: GetRole must report the id CreateRole minted, got {read_back}"
        );

        for session_name in session_names {
            let case = format!("{role_name} as {session_name}");
            let (assumed_role_id, access_key_id) =
                assume_role(&router, &case, &arn, session_name).await;
            assert_eq!(
                assumed_role_id,
                format!("{role_id}:{session_name}"),
                "case {case:?}: expected the assumed session id to be the role's stored \
                 RoleId and the session name, got {assumed_role_id:?}"
            );
            assert_role_id_shape(
                &case,
                "STS",
                assumed_role_id
                    .split(':')
                    .next()
                    .expect("the role id before the session name"),
            );

            // The session the assume minted reports the same identifier as its `UserId`,
            // so a caller correlating a session with a role reads one value.
            let (status, identity) = sts(&router, &access_key_id, "GetCallerIdentity", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetCallerIdentity, got {status} {identity}"
            );
            assert_eq!(
                member(&case, &identity, "UserId"),
                assumed_role_id,
                "case {case:?}: the minted session must report the assumed-role id as its \
                 UserId, got {identity}"
            );
        }

        seen.push(role_id);
    }

    // One id per role: IAM never issues one twice, and STS reports no id of its own
    // beside them.
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seen.len(),
        "expected one unique id per role, got {seen:?}"
    );
}
