// SPDX-License-Identifier: CC0-1.0

//! Embedding, removing and RE-embedding an inline role policy, end to end over the
//! awsQuery wire.
//!
//! RULE: whether the account holds the named (role, inline policy name) pair at the
//! instant of the call is the ONLY thing that decides `DeleteRolePolicy`'s answer — a
//! pair `PutRolePolicy` embedded is removed with HTTP 200 and an empty awsQuery
//! envelope, however many times that pair is embedded and removed in turn and whatever
//! else the role holds, and the modelled `NoSuchEntityException`
//! (`smithy_query(query="operation", operation="DeleteRolePolicy")`: errorKind client,
//! httpError 404, awsQueryError NoSuchEntity/404) is answered only for a pair the
//! account does not hold.
//!
//! That is what makes the two flows a client builds on this operation terminate: the
//! IAM-role teardown (create, embed, remove, delete the role) and the policy ROTATION
//! that removes a document before putting the next one under the same name. Neither is
//! covered by a single recorded request, so each suite here is a TABLE whose rows
//! differ along what the rule generalises over: the role's path, the alphabet and case
//! of the role and policy names, the form and size of the document, how many policies
//! the role holds beside the one under test, and how many put/delete rotations the pair
//! goes through. The awsQuery harness (router, call, element, elements, error_code) is
//! the one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, elements, error_code, router};

/// A trust policy every role this suite creates carries: an IAM policy document, which
/// is all `CreateRole` asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"mediapackage.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// How many times each row embeds and removes its pair again, so the rule is asserted
/// on a rotation rather than on one delete.
const ROTATIONS: usize = 3;

/// One inline policy to embed: the role that carries it, that role's path, the policy
/// name, and the document.
struct Case {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    policy_name: &'static str,
    document: &'static str,
}

/// Rows spanning the root, a one-segment and a nested path; role and policy names drawn
/// from the whole `^[\w+=,.@-]+$` alphabet `roleNameType` and `policyNameType` admit
/// (`smithy_query(query="operation", operation="PutRolePolicy")`) in mixed case; and
/// documents spanning a statement list, a bare statement object, one carrying an `Id`,
/// one with no `Version`, one indented with the tab, line feed and carriage return
/// `policyDocumentType`'s pattern admits, and the smallest document the IAM policy
/// grammar admits.
const CASES: &[Case] = &[
    Case {
        case: "an S3 read policy at the account root",
        role_name: "IngestRole",
        path: "/",
        policy_name: "AllowSourceRead",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject","s3:GetBucketLocation"],"Resource":"*"}]}"#,
    },
    Case {
        case: "a bare statement object under a one-segment path",
        role_name: "ingest_2",
        path: "/media/",
        policy_name: "deny_everything",
        document: r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#,
    },
    Case {
        case: "a document carrying an Id, under a nested path",
        role_name: "ingest.3",
        path: "/media/vod/",
        policy_name: "Policy.With.Id",
        document: r#"{"Version":"2012-10-17","Id":"one","Statement":[{"Effect":"Allow","Action":"s3:ListBucket"}]}"#,
    },
    Case {
        case: "a document with no Version, name spanning the alphabet",
        role_name: "ingest+4=a@b-c",
        path: "/media/",
        policy_name: "no+version=a@b-c",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"mediapackagevod:CreateAsset"}]}"#,
    },
    Case {
        case: "a document indented with tab, line feed and carriage return",
        role_name: "ingest-5",
        path: "/",
        policy_name: "indented-5",
        document: "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": [\r\n\t\t{\"Effect\": \"Allow\", \"Action\": \"s3:GetObject\"}\r\n\t]\r\n}",
    },
    Case {
        case: "the smallest document the grammar admits",
        role_name: "ingest@6",
        path: "/media/vod/west/",
        policy_name: "minimal@6",
        document: r#"{"Statement":[]}"#,
    },
];

/// Create `role_name` at `path`; every row needs the role before it can embed anything.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: the fixture CreateRole must succeed, got {status}: {body}"
    );
}

/// Embed `document` in `role_name` under `policy_name`.
async fn put_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await
}

/// Read one embedded policy back.
async fn get_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        "GetRolePolicy",
        &[("RoleName", role_name), ("PolicyName", policy_name)],
    )
    .await
}

/// Remove one embedded policy.
async fn delete_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteRolePolicy",
        &[("RoleName", role_name), ("PolicyName", policy_name)],
    )
    .await
}

/// Assert `body` is the SUCCESS envelope of `DeleteRolePolicy`: the operation's output
/// is `Unit` (`smithy_query(query="operation", operation="DeleteRolePolicy")`), so the
/// awsQuery response carries the operation's own response element and no error code.
fn assert_empty_success_envelope(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteRolePolicy 200 for an embedded policy, got \
         {status}: {body}"
    );
    assert!(
        elements(body, "Code").is_empty(),
        "case {case:?}: a success must carry no error code, got {body}"
    );
    assert!(
        body.contains("DeleteRolePolicyResponse"),
        "case {case:?}: expected the operation's own awsQuery response envelope, got {body}"
    );
}

/// RULE: an embedded (role, policy name) pair is removed with HTTP 200 and the empty
/// awsQuery envelope, the removal clears exactly that pair, and re-embedding the same
/// name makes the next removal succeed again — so a rotation of put/delete terminates
/// however many times it goes round.
#[tokio::test]
async fn an_embedded_inline_policy_is_removed_on_every_rotation() {
    for Case {
        case,
        role_name,
        path,
        policy_name,
        document,
    } in CASES
    {
        let router = router();
        create_role(&router, role_name, path).await;

        for rotation in 0..ROTATIONS {
            // Each rotation embeds a DIFFERENT document under the same name, which is
            // what a policy rotation does.
            let rotated = format!(r#"{{"Statement":[{{"Sid":"rotation{rotation}"}}]}}"#);
            let embedded: &str = if rotation == 0 { document } else { &rotated };
            let (status, body) = put_policy(&router, role_name, policy_name, embedded).await;
            assert_eq!(
                status, 200,
                "case {case:?} rotation {rotation}: PutRolePolicy must succeed, got \
                 {status}: {body}"
            );

            let (status, body) = get_policy(&router, role_name, policy_name).await;
            assert_eq!(
                status, 200,
                "case {case:?} rotation {rotation}: the embedded policy must read back, got \
                 {status}: {body}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&body, "PolicyDocument"),
                *embedded,
                "case {case:?} rotation {rotation}: expected the document just embedded, got \
                 {body}"
            );

            let (status, body) = delete_policy(&router, role_name, policy_name).await;
            assert_empty_success_envelope(&format!("{case} rotation {rotation}"), status, &body);

            // The pair is gone: the reporting half no longer resolves it, and a repeat
            // removal is the modelled refusal rather than a second success.
            let (status, body) = get_policy(&router, role_name, policy_name).await;
            assert_eq!(
                status, 404,
                "case {case:?} rotation {rotation}: a removed policy must resolve nowhere, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?} rotation {rotation}: expected the modelled awsQuery code, got \
                 {body}"
            );
            let (status, body) = delete_policy(&router, role_name, policy_name).await;
            assert_eq!(
                status, 404,
                "case {case:?} rotation {rotation}: expected 404 for a repeated removal, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?} rotation {rotation}: expected the modelled awsQuery code on the \
                 repeat, got {body}"
            );
        }

        // The role holds nothing after the last rotation, so the teardown the client
        // builds on this operation terminates: `DeleteRole` releases the role.
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", role_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRole 200 once the inline policy was removed, got \
             {status}: {body}"
        );
    }
}

/// RULE: the pair is the identity — a removal reaches the policy of the role it names
/// and no other, whatever case either name is spelled in, and the policies the role
/// holds beside it are untouched.
#[tokio::test]
async fn a_removal_reaches_only_the_pair_it_names() {
    // Rows: how many policies each role holds, so a removal is asserted against a role
    // holding one, a few and many.
    let roles = [("Shared", "/", 1_usize), ("shared_two", "/media/", 4)];
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject"}]}"#;

    let router = router();
    for (role_name, path, count) in roles {
        create_role(&router, role_name, path).await;
        for index in 0..count {
            // The SAME policy names are embedded in both roles, so a removal that
            // reached the wrong role would be visible.
            let policy_name = format!("shared-policy-{index}");
            let (status, body) = put_policy(&router, role_name, &policy_name, document).await;
            assert_eq!(
                status, 200,
                "case {role_name:?}/{policy_name:?}: PutRolePolicy must succeed, got \
                 {status}: {body}"
            );
        }
    }

    // Removing a pair from the role that holds the FEWER policies must leave every
    // policy of the other role in place.
    let (status, body) = delete_policy(&router, "SHARED", "SHARED-POLICY-0").await;
    assert_empty_success_envelope("names spelled in upper case", status, &body);
    let (status, body) = get_policy(&router, "shared", "shared-policy-0").await;
    assert_eq!(
        status, 404,
        "the removal must clear the pair whatever case it was spelled in, got {status}: {body}"
    );
    for index in 0..4 {
        let policy_name = format!("shared-policy-{index}");
        let (status, body) = get_policy(&router, "shared_two", &policy_name).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: the other role's policy must survive, got {status}: {body}"
        );
    }

    // Each of the other role's pairs is removable in turn, and removing one leaves the
    // rest.
    for removed in 0..4 {
        let policy_name = format!("shared-policy-{removed}");
        let (status, body) = delete_policy(&router, "shared_two", &policy_name).await;
        assert_empty_success_envelope(&format!("one of four: {policy_name}"), status, &body);
        for index in 0..4 {
            let other = format!("shared-policy-{index}");
            let (status, body) = get_policy(&router, "shared_two", &other).await;
            let expected = if index <= removed { 404 } else { 200 };
            assert_eq!(
                status, expected,
                "case {policy_name:?}: expected {other:?} to answer {expected} after the \
                 removal, got {status}: {body}"
            );
        }
    }
}

/// RULE: `NoSuchEntity` (HTTP 404) is answered only for a pair the account does not
/// hold — a role it holds no role under, or a policy name that role does not carry —
/// and the refusal changes nothing.
#[tokio::test]
async fn only_a_pair_the_account_does_not_hold_is_not_found() {
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject"}]}"#;
    let router = router();
    create_role(&router, "held", "/media/").await;
    let (status, body) = put_policy(&router, "held", "held-policy", document).await;
    assert_eq!(
        status, 200,
        "the fixture PutRolePolicy must succeed, got {status}: {body}"
    );

    // Rows: a role that was never created, one whose name differs from the held role
    // only at its end, a name spanning the whole accepted alphabet, and the held role
    // paired with policy names it does not carry (a near miss, a never-embedded name,
    // and a name spanning the alphabet).
    let refused = [
        ("a role never created", "ghost", "held-policy"),
        ("a near miss on the role name", "helds", "held-policy"),
        ("a role name spanning the alphabet", "n0_+=,.@-", "held-policy"),
        ("a near miss on the policy name", "held", "held-policy-2"),
        ("a policy nobody embedded", "held", "Absent"),
        ("a policy name spanning the alphabet", "held", "a_b+c=d,e.f@g-h"),
    ];
    for (case, role_name, policy_name) in refused {
        let (status, body) = delete_policy(&router, role_name, policy_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a pair the account does not hold, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // Nothing the refusals named was held, and the pair that IS held is untouched by
    // them: it still reads back, and it still removes with 200.
    let (status, body) = get_policy(&router, "held", "held-policy").await;
    assert_eq!(
        status, 200,
        "the refusals must leave the held policy in place, got {status}: {body}"
    );
    assert_eq!(
        crate::user_lifecycle::document_element(&body, "PolicyDocument"),
        document,
        "the refusals must leave the held document unchanged, got {body}"
    );
    let (status, body) = delete_policy(&router, "held", "held-policy").await;
    assert_empty_success_envelope("the held pair after the refusals", status, &body);
}
