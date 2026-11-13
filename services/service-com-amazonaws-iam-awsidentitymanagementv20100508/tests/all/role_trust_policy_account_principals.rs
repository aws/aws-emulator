// SPDX-License-Identifier: CC0-1.0

//! The ONE spelling IAM saves a trust policy's ACCOUNT principals in, end to end over the
//! awsQuery wire.
//!
//! RULE: when IAM saves a policy document that names an AWS account as a principal by the
//! account's twelve-digit id alone, it stores and reports that principal as the account's
//! root-user ARN — "the ARN for an account uses the following syntax:
//! `arn:aws:iam::AWS-account-ID:root`". The document is saved RENDERED, as every policy store in
//! this emulator renders one: the whitespace between its JSON tokens is gone and a one-element
//! string-or-array element is written as the string it holds, while its member order, its
//! statement count and every other principal (a user or role ARN, the `*` wildcard, a service,
//! federated or canonical-user principal) are stored as they were written. A role's
//! `AssumeRolePolicyDocument` is the policy document this service saves principals in, so the
//! rule holds for both of its writers — `CreateRole` and `UpdateAssumeRolePolicy` — and every
//! read of the role (`GetRole`, `ListRoles`) reports the stored text, percent-encoded as every
//! response carries a policy document.
//!
//! No single recorded request is the specification, so the suite is a TABLE whose rows differ
//! along what the rule generalises over: WHICH account is named (this emulator's own and
//! another), where the principal sits (alone, in a one-element list, in a list of several,
//! beside principals of other types, under the complement `NotPrincipal`), the spacing of the
//! submitted document, and the values the rule does not touch (an ARN, a wildcard, a service
//! principal, a figure that is not an account id, an account id under another element).
//!
//! The awsQuery harness (router, call, element, elements) is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, router};

/// An account other than this emulator's own, so a cross-account trust is one of the rows.
const OTHER_ACCOUNT: &str = "210987654321";

/// The root-user ARN of the account `id`.
///
/// Source: the `Principal` element's own syntax for an account — "the ARN for an account uses
/// the following syntax: `arn:aws:iam::AWS-account-ID:root`" — in the partition every ARN this
/// emulator mints carries, read off a role's ARN rather than written here.
fn root_arn(id: &str) -> String {
    let partition = partition_of(&format!("arn:aws:iam::{}:role/x", account_id()));
    format!("arn:{partition}:iam::{id}:root")
}

/// The partition segment of `arn` — the second of its colon-separated fields.
fn partition_of(arn: &str) -> String {
    arn.split(':').nth(1).unwrap_or_default().to_string()
}

/// A one-statement trust policy whose `Principal` element is `principal`.
fn trust(principal: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{principal},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// One row: what a failing assertion calls it, the document the request carries, and the
/// document IAM must store and report for it.
struct Row {
    case: &'static str,
    submitted: String,
    stored: String,
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    let own = account_id();
    let own_root = root_arn(own);
    let other_root = root_arn(OTHER_ACCOUNT);
    vec![
        Row {
            case: "this account named by its id alone",
            submitted: trust(&format!(r#"{{"AWS":"{own}"}}"#)),
            stored: trust(&format!(r#"{{"AWS":"{own_root}"}}"#)),
        },
        Row {
            case: "another account named by its id alone",
            submitted: trust(&format!(r#"{{"AWS":"{OTHER_ACCOUNT}"}}"#)),
            stored: trust(&format!(r#"{{"AWS":"{other_root}"}}"#)),
        },
        Row {
            case: "an account already named by its root ARN",
            submitted: trust(&format!(r#"{{"AWS":"{other_root}"}}"#)),
            stored: trust(&format!(r#"{{"AWS":"{other_root}"}}"#)),
        },
        Row {
            case: "a one-element list of an account id is written as that string",
            submitted: trust(&format!(r#"{{"AWS":["{OTHER_ACCOUNT}"]}}"#)),
            stored: trust(&format!(r#"{{"AWS":"{other_root}"}}"#)),
        },
        Row {
            case: "a list of two account ids, each written",
            submitted: trust(&format!(r#"{{"AWS":["{own}","{OTHER_ACCOUNT}"]}}"#)),
            stored: trust(&format!(r#"{{"AWS":["{own_root}","{other_root}"]}}"#)),
        },
        Row {
            case: "a list mixing an id, a root ARN, a role ARN and the wildcard",
            submitted: trust(&format!(
                r#"{{"AWS":["{OTHER_ACCOUNT}","{own_root}","arn:aws:iam::{OTHER_ACCOUNT}:role/deploy","*"]}}"#
            )),
            stored: trust(&format!(
                r#"{{"AWS":["{other_root}","{own_root}","arn:aws:iam::{OTHER_ACCOUNT}:role/deploy","*"]}}"#
            )),
        },
        Row {
            case: "a service principal alone is stored as written",
            submitted: trust(r#"{"Service":"lambda.amazonaws.com"}"#),
            stored: trust(r#"{"Service":"lambda.amazonaws.com"}"#),
        },
        Row {
            case: "an account id beside a service and a federated principal",
            submitted: trust(&format!(
                r#"{{"AWS":"{OTHER_ACCOUNT}","Service":"ec2.amazonaws.com","Federated":"accounts.google.com"}}"#
            )),
            stored: trust(&format!(
                r#"{{"AWS":"{other_root}","Service":"ec2.amazonaws.com","Federated":"accounts.google.com"}}"#
            )),
        },
        Row {
            case: "the complement element names an account id too",
            submitted: format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","NotPrincipal":{{"AWS":"{OTHER_ACCOUNT}"}},"Action":"sts:AssumeRole"}}]}}"#
            ),
            stored: format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","NotPrincipal":{{"AWS":"{other_root}"}},"Action":"sts:AssumeRole"}}]}}"#
            ),
        },
        Row {
            // A trust policy is STORED rendered, so the whitespace between an indented
            // document's JSON tokens is gone from what every read reports.
            case: "an indented document is stored without the whitespace between its tokens",
            submitted: format!(
                "{{\r\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": {{\"AWS\": \"{OTHER_ACCOUNT}\"}},\n\t\t\t\"Action\": \"sts:AssumeRole\"\n\t\t}}\n\t]\n}}"
            ),
            stored: trust(&format!(r#"{{"AWS":"{other_root}"}}"#)),
        },
        Row {
            case: "an indented list of ids keeps its list form and loses its whitespace",
            submitted: format!(
                "{{\n  \"Statement\" : [ {{\n    \"Effect\" : \"Allow\",\n    \"Principal\" : {{ \"AWS\" : [ \"{own}\", \"{OTHER_ACCOUNT}\" ] }},\n    \"Action\" : \"sts:AssumeRole\"\n  }} ]\n}}"
            ),
            stored: format!(
                r#"{{"Statement":[{{"Effect":"Allow","Principal":{{"AWS":["{own_root}","{other_root}"]}},"Action":"sts:AssumeRole"}}]}}"#
            ),
        },
        Row {
            // An account id is twelve digits; a figure of any other length names no account,
            // so it is stored as it was written.
            case: "figures one digit either side of an account id",
            submitted: trust(r#"{"AWS":["21098765432","2109876543210"]}"#),
            stored: trust(r#"{"AWS":["21098765432","2109876543210"]}"#),
        },
        Row {
            case: "an account id under another element is not a principal",
            submitted: format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{OTHER_ACCOUNT}","Effect":"Allow","Principal":{{"Service":"{OTHER_ACCOUNT}"}},"Action":"sts:AssumeRole"}}]}}"#
            ),
            stored: format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{OTHER_ACCOUNT}","Effect":"Allow","Principal":{{"Service":"{OTHER_ACCOUNT}"}},"Action":"sts:AssumeRole"}}]}}"#
            ),
        },
    ]
}

/// The trust policy `GetRole` reports for a role the account holds, percent-decoded back to
/// plain JSON as IAM's contract tells a caller to read a policy document.
async fn reported_trust_policy(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
    );
    crate::user_lifecycle::document_element(&body, "AssumeRolePolicyDocument")
}

/// Create one role carrying `document`.
async fn create_role(
    router: &MultiServiceRouter,
    role_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", document),
        ],
    )
    .await
}

/// RULE: `CreateRole` stores the trust policy it was handed RENDERED — the whitespace between
/// its JSON tokens gone, a one-element string-or-array element written as its string, and every
/// account principal named by an id alone written as that account's root-user ARN — and every
/// read of the role reports that one stored text.
#[tokio::test]
async fn create_role_stores_an_account_principal_as_the_account_root_arn() {
    let router = router();
    for (index, row) in rows().into_iter().enumerate() {
        let role_name = format!("TrustCreate{index}");
        let (status, body) = create_role(&router, &role_name, &row.submitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateRole must succeed, got {status}: {body}",
            row.case
        );

        // The create's own answer and the later read report the ONE stored document.
        let created = crate::user_lifecycle::document_element(&body, "AssumeRolePolicyDocument");
        assert_eq!(
            created, row.stored,
            "case {:?}: expected CreateRole to report {:?}, got {created:?}",
            row.case, row.stored
        );
        let reported = reported_trust_policy(&router, &role_name).await;
        assert_eq!(
            reported, row.stored,
            "case {:?}: expected GetRole to report {:?}, got {reported:?}",
            row.case, row.stored
        );

        // The stored text is still the JSON policy that was sent, not an escaped or
        // re-encoded form of it, and it says the same thing as the submitted document once
        // the principal is read.
        let parsed: serde_json::Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
            panic!("case {:?}: the reported policy must be JSON ({error}): {reported:?}", row.case)
        });
        let expected: serde_json::Value = serde_json::from_str(&row.stored)
            .unwrap_or_else(|error| panic!("case {:?}: {error}", row.case));
        assert_eq!(
            parsed, expected,
            "case {:?}: expected the reported policy to parse to {expected:?}, got {parsed:?}",
            row.case
        );
    }
}

/// RULE: `UpdateAssumeRolePolicy` saves principals the same way `CreateRole` does — one rule,
/// both writers of the member — and re-submitting what was read stores the same text again.
#[tokio::test]
async fn updating_a_trust_policy_stores_the_same_account_principal_spelling() {
    let router = router();
    let role_name = "TrustUpdateTarget";
    let (status, body) = create_role(
        &router,
        role_name,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole must succeed, got {status}: {body}"
    );

    for row in rows() {
        let (status, body) = call(
            &router,
            "UpdateAssumeRolePolicy",
            &[("RoleName", role_name), ("PolicyDocument", &row.submitted)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateAssumeRolePolicy must succeed, got {status}: {body}",
            row.case
        );
        let reported = reported_trust_policy(&router, role_name).await;
        assert_eq!(
            reported, row.stored,
            "case {:?}: expected the update to store {:?}, got {reported:?}",
            row.case, row.stored
        );

        // Putting back what was read stores the same text: the spelling IAM writes is one it
        // leaves alone, so a client that reads and re-submits sees no drift.
        let (status, body) = call(
            &router,
            "UpdateAssumeRolePolicy",
            &[("RoleName", role_name), ("PolicyDocument", &reported)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: re-submitting the stored document must succeed, got {status}: {body}",
            row.case
        );
        let again = reported_trust_policy(&router, role_name).await;
        assert_eq!(
            again, row.stored,
            "case {:?}: expected the stored document to be a fixed point, got {again:?}",
            row.case
        );
    }
}

/// RULE: the stored document is what EVERY read of the role reports, so a listing reports the
/// same text a single read does.
#[tokio::test]
async fn every_read_of_a_role_reports_the_stored_trust_policy() {
    let router = router();
    let mut expected: Vec<String> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        let role_name = format!("TrustListed{index}");
        let (status, body) = create_role(&router, &role_name, &row.submitted).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateRole must succeed, got {status}: {body}",
            row.case
        );
        expected.push(row.stored);
    }

    let (status, body) = call(&router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "ListRoles must succeed, got {status}: {body}");
    let listed = crate::user_lifecycle::document_elements(&body, "AssumeRolePolicyDocument");
    for document in &expected {
        assert!(
            listed.contains(document),
            "expected ListRoles to report {document:?}, got {listed:?}"
        );
    }
}
