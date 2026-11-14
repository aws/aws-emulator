// SPDX-License-Identifier: CC0-1.0

//! The identities a resource-based policy document GRANTS TO, as this service reads them
//! and as it resolves them.
//!
//! RULE: a resource policy states WHO a permission is granted to, so ECR resolves each
//! identity a submitted `policyText` names against this account's IAM before it stores the
//! document and refuses the parameter when IAM answers that one of them is absent. A
//! refusal has to be grounded in what IAM ANSWERED, which has two halves, and this suite
//! owns both of the ones a single-service emulator can show:
//!
//! * the READING — which identities a document names, out of the string, the list and the
//!   typed object the policy language writes a `Principal` as;
//! * the SILENCE — with no IAM mounted there is nothing to ask, so every document is stored
//!   as submitted rather than refused on an assumption.
//!
//! The resolution itself travels over the in-process callback, so the refusal is pinned in
//! `core/router/tests/all/ecr_repository_policy_principal.rs`, where ECR and IAM are
//! mounted over one engine.

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;

use crate::policy_document_canonical_form::document_content;
use crate::wire::{ok, REGISTRY_ID};

/// A syntactically valid account id that is NOT this registry's.
const OTHER_ACCOUNT: &str = "123456789012";

/// The ARN of an IAM entity of `account`.
fn iam_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:iam::{account}:{resource}")
}

/// A repository policy granting one action to `principal`.
fn policy(principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowPull",
            "Effect": "Allow",
            "Principal": principal,
            "Action": "ecr:BatchGetImage",
        }]
    })
    .to_string()
}

/// The `Principal` forms a resource policy's statements are written in, each with the
/// identities the document names. The forms are the policy language's own — "a string or
/// an array of strings", per principal type, or the bare string shorthand — and the rows
/// span them: the shorthand, each typed object, a list, several types in one element,
/// several statements, and a repeat of one identity.
fn principal_readings() -> Vec<(&'static str, String, Vec<String>)> {
    let role = iam_arn(REGISTRY_ID, "role/pull-role");
    let user = iam_arn(REGISTRY_ID, "user/builder");
    let foreign = iam_arn(OTHER_ACCOUNT, "root");
    vec![
        (
            "the bare-string shorthand",
            policy(json!("*")),
            vec!["*".to_string()],
        ),
        (
            "one AWS identity",
            policy(json!({"AWS": role})),
            vec![role.clone()],
        ),
        (
            "a service principal",
            policy(json!({"Service": "lambda.amazonaws.com"})),
            vec!["lambda.amazonaws.com".to_string()],
        ),
        (
            "a list of AWS identities",
            policy(json!({"AWS": [role, user]})),
            vec![role.clone(), user.clone()],
        ),
        (
            "two principal types in one element",
            policy(json!({"AWS": role, "Service": "lambda.amazonaws.com"})),
            vec![role.clone(), "lambda.amazonaws.com".to_string()],
        ),
        (
            "one statement written as the object itself",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": foreign},
                    "Action": "ecr:BatchGetImage",
                }
            })
            .to_string(),
            vec![foreign.clone()],
        ),
        (
            "two statements, each naming its own identity",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Principal": {"AWS": role}, "Action": "ecr:BatchGetImage"},
                    {"Effect": "Deny", "Principal": {"AWS": user}, "Action": "ecr:DeleteRepository"},
                ]
            })
            .to_string(),
            vec![role.clone(), user.clone()],
        ),
        (
            // The same identity named twice is ONE identity to resolve.
            "one identity named by two statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Principal": {"AWS": role}, "Action": "ecr:BatchGetImage"},
                    {"Effect": "Deny", "Principal": {"AWS": role}, "Action": "ecr:DeleteRepository"},
                ]
            })
            .to_string(),
            vec![role.clone()],
        ),
        (
            "text that is not a policy at all, which names nobody",
            "not a policy".to_string(),
            Vec::new(),
        ),
        (
            "a document with no statement, which names nobody",
            json!({"Version": "2012-10-17"}).to_string(),
            Vec::new(),
        ),
    ]
}

#[test]
fn the_identities_a_document_grants_to_are_read_out_of_every_principal_form() {
    for (case, document, expected) in principal_readings() {
        let read = ecr::state::policy_principals(&document);
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?}, got {read:?} for {document}"
        );
    }
}

/// The principal forms a `SetRepositoryPolicy` carries, each with the principal the stored
/// document names once ECR has written it: identities of this account that were never
/// created included, because with no IAM mounted there is nothing to ask, so each one is
/// STORED rather than refused on an assumption. A whole ACCOUNT named by its bare id is the
/// one form a policy store respells — as that account's root ARN, the other spelling of the
/// same principal (`policy_document_account_principal.rs` pins that rule).
fn principal_forms() -> Vec<(&'static str, Value, Value)> {
    // The form a stored document reports, where it is the submitted form itself.
    let kept = |case: &'static str, principal: Value| (case, principal.clone(), principal);
    vec![
        kept("every principal", json!("*")),
        kept("every principal, as a typed object", json!({"AWS": "*"})),
        kept(
            "a role of this account that was never created",
            json!({"AWS": iam_arn(REGISTRY_ID, "role/ghost-role")}),
        ),
        kept(
            "a user of this account that was never created",
            json!({"AWS": iam_arn(REGISTRY_ID, "user/ghost-user")}),
        ),
        kept(
            "this account's root",
            json!({"AWS": iam_arn(REGISTRY_ID, "root")}),
        ),
        (
            "this account by id",
            json!({"AWS": REGISTRY_ID}),
            json!({"AWS": iam_arn(REGISTRY_ID, "root")}),
        ),
        kept(
            "another account's role",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "role/ghost-role")}),
        ),
        kept(
            "a service principal",
            json!({"Service": "replication.ecr.amazonaws.com"}),
        ),
        kept(
            "a list of identities of this account",
            json!({"AWS": [
                iam_arn(REGISTRY_ID, "role/ghost-role"),
                iam_arn(REGISTRY_ID, "user/ghost-user"),
            ]}),
        ),
    ]
}

#[tokio::test]
async fn with_no_iam_to_ask_every_principal_is_stored_as_submitted() {
    let router = crate::wire::emulator();

    for (index, (case, principal, stored_principal)) in
        principal_forms().into_iter().enumerate()
    {
        let name = format!("ungrounded-principal-{index}");
        ok(&router, case, "CreateRepository", &json!({"repositoryName": name})).await;

        let document = policy(principal);
        let stored = policy(stored_principal);
        let set = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": &document}),
        )
        .await;
        assert_eq!(
            document_content(set["policyText"].as_str().unwrap_or_default()),
            document_content(&stored),
            "case {case:?}: the set must report the document it stored, got {:?}",
            set["policyText"]
        );

        // And the repository holds it: the read reports the same document.
        let got = ok(
            &router,
            case,
            "GetRepositoryPolicy",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            document_content(got["policyText"].as_str().unwrap_or_default()),
            document_content(&stored),
            "case {case:?}: the repository must hold the stored document, got {:?}",
            got["policyText"]
        );
    }
}
