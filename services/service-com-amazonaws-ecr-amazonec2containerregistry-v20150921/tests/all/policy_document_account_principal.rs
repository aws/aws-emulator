// SPDX-License-Identifier: CC0-1.0

//! How an attached policy document SPELLS an account principal, end to end over the wire.
//!
//! The rule under test: the policy language lets a statement name a whole AWS account by its
//! bare 12-digit account id, and that id and the account's root ARN name the same principal —
//! so the document ECR stores and reports carries the root ARN
//! (`arn:aws:iam::<account id>:root`) wherever an `AWS` principal was submitted as a bare
//! account id, and every other value of that element is reported exactly as the caller wrote
//! it. The account id is the caller's own; nothing else about the document moves, so a client
//! that reads the policy back and compares it against the one it sent sees one spelling
//! rather than rewriting the policy forever.
//!
//! The rows span what the rule generalises over: WHICH account is named (this registry's and
//! another's), where the value sits (alone, in a list of one, in a list of several, in a
//! second statement), which element carries it (`Principal` and its complement
//! `NotPrincipal`), and the values the rule does not touch (the wildcard, an identity's ARN,
//! a root ARN already, a service principal, and an account id under another element).
//!
//! Both documents this service stores are asserted, through the operation that writes each
//! and the one that reads it back: a repository policy (`SetRepositoryPolicy` /
//! `GetRepositoryPolicy`) and the registry policy (`PutRegistryPolicy` /
//! `GetRegistryPolicy`).

use serde_json::{json, Value};

use crate::policy_document_canonical_form::document_content;
use crate::wire::{ok, REGION, REGISTRY_ID};

/// A syntactically valid account id (`RegistryId` is `^[0-9]{12}$`) that is NOT this
/// registry's, so the rule is shown not to be about this account alone.
const OTHER_ACCOUNT: &str = "123456789012";

/// The root ARN of `account`: the spelling a stored document names a whole account by.
fn account_root(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// One row: what it is about, the statement members a document carries, and the members the
/// stored document reports for them.
struct Row {
    case: &'static str,
    submitted: Value,
    stored: Value,
}

/// The statement members a document carries, and the members ECR reports for them.
///
/// Every row is submitted as both documents this service stores, so the members below carry
/// no resource element: the repository policy refuses one and the registry policy requires
/// one (`policy_document_scope.rs`), and each test adds its own scope's.
fn rows() -> Vec<Row> {
    let row = |case: &'static str, submitted: Value, stored: Value| Row {
        case,
        submitted,
        stored,
    };
    let kept = |case: &'static str, members: Value| row(case, members.clone(), members);
    vec![
        row(
            "this registry's account by its id",
            json!({"Principal": {"AWS": REGISTRY_ID}}),
            json!({"Principal": {"AWS": account_root(REGISTRY_ID)}}),
        ),
        row(
            "another account by its id",
            json!({"Principal": {"AWS": OTHER_ACCOUNT}}),
            json!({"Principal": {"AWS": account_root(OTHER_ACCOUNT)}}),
        ),
        row(
            "an account id in a list of one",
            json!({"Principal": {"AWS": [OTHER_ACCOUNT]}}),
            json!({"Principal": {"AWS": [account_root(OTHER_ACCOUNT)]}}),
        ),
        row(
            "a list mixing account ids with values the rule leaves alone",
            json!({"Principal": {"AWS": [
                OTHER_ACCOUNT,
                account_root(REGISTRY_ID),
                format!("arn:aws:iam::{OTHER_ACCOUNT}:role/replicator"),
                REGISTRY_ID,
            ]}}),
            json!({"Principal": {"AWS": [
                account_root(OTHER_ACCOUNT),
                account_root(REGISTRY_ID),
                format!("arn:aws:iam::{OTHER_ACCOUNT}:role/replicator"),
                account_root(REGISTRY_ID),
            ]}}),
        ),
        row(
            "an account id under the complement element",
            json!({
                "Principal": {"AWS": account_root(REGISTRY_ID)},
                "NotPrincipal": {"AWS": OTHER_ACCOUNT},
            }),
            json!({
                "Principal": {"AWS": account_root(REGISTRY_ID)},
                "NotPrincipal": {"AWS": account_root(OTHER_ACCOUNT)},
            }),
        ),
        kept(
            "the wildcard, which names no one account",
            json!({"Principal": {"AWS": "*"}}),
        ),
        kept(
            "an identity's ARN, which already names its principal",
            json!({"Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:user/builder")}}),
        ),
        kept(
            "a root ARN, which is the spelling the rule writes",
            json!({"Principal": {"AWS": account_root(OTHER_ACCOUNT)}}),
        ),
        kept(
            "a service principal, whose type the rule does not spell",
            json!({"Principal": {"Service": "replication.ecr.amazonaws.com"}}),
        ),
        kept(
            // The account id sits under `Sid` and inside an action and a condition value:
            // the rule writes the values of one principal type and nothing else.
            "the same digits under another element",
            json!({
                "Sid": REGISTRY_ID,
                "Principal": {"Service": format!("{REGISTRY_ID}.ecr.amazonaws.com")},
                "Condition": {"StringEquals": {"aws:PrincipalAccount": REGISTRY_ID}},
            }),
        ),
    ]
}

/// A document built from the statement members `members`, plus `extra` merged into the
/// statement (the resource element each scope needs, or nothing).
fn document(members: &Value, extra: Value) -> String {
    let mut statement = json!({
        "Sid": "AllowPull",
        "Effect": "Allow",
        "Action": "ecr:BatchGetImage",
    });
    let into = statement
        .as_object_mut()
        .expect("the statement is an object");
    for source in [members, &extra] {
        for (element, value) in source.as_object().expect("the members are an object") {
            into.insert(element.clone(), value.clone());
        }
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// The CONTENT of the `policyText` an answer carries, so a comparison reads the document's
/// content rather than its layout or the form it writes a one-value list in — which
/// `policy_document_canonical_form.rs` pins byte for byte.
fn reported(body: &Value) -> Value {
    let text = body["policyText"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer must carry a policyText, got {body:?}"));
    document_content(text)
}

/// The repository policy: the set reports the stored spelling, the read reports the same
/// document, and setting the document that was read back changes nothing.
#[tokio::test]
async fn a_repository_policy_reports_an_account_principal_as_that_accounts_root() {
    let router = crate::wire::emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let name = format!("account-principal-{index}");
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;

        // A repository policy names no resource of its own.
        let submitted = document(&row.submitted, json!({}));
        let expected = document_content(&document(&row.stored, json!({})));

        let set = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": &submitted}),
        )
        .await;
        assert_eq!(
            reported(&set),
            expected,
            "case {case:?}: the set must report {expected}, got {}",
            reported(&set)
        );

        let got = ok(
            &router,
            case,
            "GetRepositoryPolicy",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            reported(&got),
            expected,
            "case {case:?}: the repository must hold {expected}, got {}",
            reported(&got)
        );

        // The spelling is a fixed point: a client that sets back the document it read
        // stores the same document again, which is what makes a drift comparison settle.
        let again = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": got["policyText"].clone()}),
        )
        .await;
        assert_eq!(
            reported(&again),
            expected,
            "case {case:?}: setting the reported document back must change nothing, got {}",
            reported(&again)
        );
    }
}

/// The registry policy: the same spelling, from the put and the read beside it. A registry
/// policy may not grant to every principal, so the wildcard row is submitted here with the
/// account beside it — the rule is about the account value, not about the element's arity.
#[tokio::test]
async fn the_registry_policy_reports_an_account_principal_as_that_accounts_root() {
    let router = crate::wire::emulator();

    // Each statement of a registry policy names the resources it applies to.
    let resource = json!({"Resource": format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/*")});

    for row in rows() {
        let case = row.case;
        if row.submitted.to_string().contains("\"*\"") {
            // A registry policy may not open the registry to every principal; that rule is
            // pinned by `registry_policy.rs`.
            continue;
        }
        let submitted = document(&row.submitted, resource.clone());
        let expected = document_content(&document(&row.stored, resource.clone()));

        let put = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": &submitted}),
        )
        .await;
        assert_eq!(
            reported(&put),
            expected,
            "case {case:?}: the put must report {expected}, got {}",
            reported(&put)
        );

        let got = ok(&router, case, "GetRegistryPolicy", &json!({})).await;
        assert_eq!(
            reported(&got),
            expected,
            "case {case:?}: the registry must hold {expected}, got {}",
            reported(&got)
        );

        let again = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": got["policyText"].clone()}),
        )
        .await;
        assert_eq!(
            reported(&again),
            expected,
            "case {case:?}: putting the reported document back must change nothing, got {}",
            reported(&again)
        );
    }
}

/// The account id is read off the SUBMITTED document, so a value that only looks like one is
/// left alone: an id of the wrong width, one that is not all digits, and one carrying
/// whitespace are each reported as the caller wrote them — and each is refused as a
/// principal, because a value in none of the forms the policy language defines names no
/// principal at all.
#[tokio::test]
async fn only_a_12_digit_account_id_is_respelled() {
    let router = crate::wire::emulator();
    let name = "account-principal-lookalikes";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    // The rows differ along HOW the value misses the account-id form: too few digits, too
    // many, a non-digit, surrounding whitespace, and the empty string.
    for (case, value) in [
        ("eleven digits", "12345678901"),
        ("thirteen digits", "1234567890123"),
        ("a non-digit among twelve characters", "12345678901x"),
        ("twelve digits with a trailing space", "123456789012 "),
        ("the empty string", ""),
    ] {
        let submitted = document(&json!({"Principal": {"AWS": value}}), json!({}));
        let answer = crate::wire::call(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": &submitted}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "InvalidParameterException"),
            "case {case:?}: a value in none of the principal forms must be refused, got {} \
             with {:?}",
            answer.status,
            answer.body
        );
    }
}
