// SPDX-License-Identifier: CC0-1.0

//! The FORMS a resource policy ECR will hold names its principals in, end to end over the
//! wire.
//!
//! The rule under test: a statement's `Principal`/`NotPrincipal` is not opaque text — the
//! policy language writes it either as the bare wildcard or as a map from a principal TYPE
//! to that type's values, and each type has its own forms (an account by its id, an identity
//! by its ARN, a service by its principal name, an identity provider, a canonical user). A
//! value in none of the forms its type defines, or a type the language does not define,
//! names no identity any policy engine could evaluate, so the document is refused HTTP 400
//! `InvalidParameterException` carrying the scope's constraint phrase and nothing is
//! written; a document whose every principal names someone is stored.
//!
//! The rows span what the rule generalises over: the principal type, the form the element
//! takes (the bare shorthand, a typed map, a list under one type, several types at once),
//! which identity an ARN names, and HOW a value misses its type's forms.
//!
//! Both documents this service stores are asserted, because the rule is the policy
//! language's rather than one operation's: a repository policy (`SetRepositoryPolicy`) and
//! the registry policy (`PutRegistryPolicy`), each with the resource element its own scope
//! requires (`policy_document_scope.rs`).

use serde_json::{json, Value};

use crate::wire::{call, ok, refused, REGION, REGISTRY_ID};

/// The error code both refusals carry.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A syntactically valid account id that is NOT this registry's.
const OTHER_ACCOUNT: &str = "123456789012";

/// The ARN of an IAM entity of `account`.
fn iam_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:iam::{account}:{resource}")
}

/// The clause a refusal states, over the API-documented label of the parameter carrying the
/// document and the constraint phrase of the document's scope.
fn clause(constraint: &str) -> String {
    format!("Invalid parameter at 'PolicyText' failed to satisfy constraint: '{constraint}'")
}

/// One row: the `Principal` element a document carries, and whether it names principals.
struct Form {
    case: &'static str,
    principal: Value,
    /// Whether every value of the element is in one of its type's forms.
    names_principals: bool,
    /// Whether the row is asserted for the REGISTRY policy too: a registry policy may not
    /// grant to every principal, which is a rule of its own (`registry_policy.rs`), so a
    /// row naming the wildcard is asserted for the repository policy alone.
    in_registry_policy: bool,
}

/// The principal forms a policy document is written in, those the language defines and those
/// it does not.
fn forms() -> Vec<Form> {
    let named = |case: &'static str, principal: Value| Form {
        case,
        principal,
        names_principals: true,
        in_registry_policy: true,
    };
    let unnamed = |case: &'static str, principal: Value| Form {
        case,
        principal,
        names_principals: false,
        in_registry_policy: true,
    };
    // A row naming the wildcard: a repository is shared publicly that way, and a registry
    // policy may not say it at all.
    let wildcard = |case: &'static str, principal: Value| Form {
        case,
        principal,
        names_principals: true,
        in_registry_policy: false,
    };
    vec![
        wildcard("the bare wildcard shorthand", json!("*")),
        wildcard("the wildcard under the AWS type", json!({"AWS": "*"})),
        named("an account by its id", json!({"AWS": OTHER_ACCOUNT})),
        named(
            "an account's root",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "root")}),
        ),
        named(
            "an IAM user and a role under a path",
            json!({"AWS": [
                iam_arn(OTHER_ACCOUNT, "user/builder"),
                iam_arn(OTHER_ACCOUNT, "role/service-role/replicator"),
            ]}),
        ),
        named(
            "an STS assumed-role session",
            json!({"AWS": format!("arn:aws:sts::{OTHER_ACCOUNT}:assumed-role/replicator/session")}),
        ),
        named(
            "a service principal, plain and regional",
            json!({"Service": ["replication.ecr.amazonaws.com", format!("logs.{REGION}.amazonaws.com")]}),
        ),
        named(
            "a SAML provider of this account",
            json!({"Federated": iam_arn(REGISTRY_ID, "saml-provider/corp")}),
        ),
        named(
            "a web-identity provider by host",
            json!({"Federated": "accounts.google.com"}),
        ),
        named(
            "a canonical user id",
            json!({"CanonicalUser": "b".repeat(64)}),
        ),
        named(
            "two principal types at once",
            json!({
                "AWS": iam_arn(OTHER_ACCOUNT, "root"),
                "Service": "replication.ecr.amazonaws.com",
            }),
        ),
        unnamed(
            "an entity name rather than an ARN",
            json!({"AWS": "replicator"}),
        ),
        unnamed(
            "an account id of the wrong width",
            json!({"AWS": "12345678901"}),
        ),
        unnamed(
            "an ARN of an IAM entity that is no principal",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "group/builders")}),
        ),
        unnamed(
            "a principal ARN carrying a wildcard",
            json!({"AWS": iam_arn(OTHER_ACCOUNT, "user/*")}),
        ),
        unnamed(
            "the second value of a list",
            json!({"AWS": [iam_arn(OTHER_ACCOUNT, "root"), "nobody"]}),
        ),
        unnamed(
            "a service principal that is no DNS name",
            json!({"Service": "Amazon ECR"}),
        ),
        unnamed(
            "a canonical user id of the wrong length",
            json!({"CanonicalUser": "b".repeat(63)}),
        ),
        unnamed(
            "a principal type the language does not define",
            json!({"IAM": iam_arn(OTHER_ACCOUNT, "root")}),
        ),
        unnamed(
            "the shorthand carrying anything but the wildcard",
            json!(OTHER_ACCOUNT),
        ),
        unnamed(
            "the element written as a list",
            json!([iam_arn(OTHER_ACCOUNT, "root")]),
        ),
    ]
}

/// A document granting one action to `principal`, with `extra` merged into its statement —
/// the resource element the document's scope requires.
fn document(principal: &Value, extra: Value) -> String {
    let mut statement = json!({
        "Sid": "AllowPull",
        "Effect": "Allow",
        "Principal": principal,
        "Action": "ecr:BatchGetImage",
    });
    let into = statement
        .as_object_mut()
        .expect("the statement is an object");
    for (element, value) in extra.as_object().expect("the members are an object") {
        into.insert(element.clone(), value.clone());
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// A repository policy is stored when every principal it names is in one of its type's
/// forms, and refused when one is not.
#[tokio::test]
async fn a_repository_policy_is_stored_only_when_its_principals_name_identities() {
    let router = crate::wire::emulator();
    let name = "shared/principal-forms";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    for form in forms() {
        let case = form.case;
        // A repository policy names no resource of its own.
        let submitted = document(&form.principal, json!({}));
        let answer = call(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": &submitted}),
        )
        .await;
        if form.names_principals {
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({}): the document must be stored, got {} with {:?}",
                form.principal, answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, INVALID_PARAMETER),
                "case {case:?} ({}): the document must be refused, got {} with {:?}",
                form.principal,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.message(),
                clause("Invalid repository policy provided"),
                "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
                answer.message()
            );
        }
    }
}

/// The registry policy reads its principals the same way, and words its own constraint.
#[tokio::test]
async fn the_registry_policy_is_stored_only_when_its_principals_name_identities() {
    let router = crate::wire::emulator();
    // Each statement of a registry policy names the resources it applies to.
    let resource = json!({"Resource": format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/*")});

    for form in forms() {
        if !form.in_registry_policy {
            continue;
        }
        let case = form.case;
        let submitted = document(&form.principal, resource.clone());
        let answer = call(
            &router,
            "PutRegistryPolicy",
            &json!({"policyText": &submitted}),
        )
        .await;
        if form.names_principals {
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({}): the document must be stored, got {} with {:?}",
                form.principal, answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, INVALID_PARAMETER),
                "case {case:?} ({}): the document must be refused, got {} with {:?}",
                form.principal,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.message(),
                clause("Invalid registry policy provided"),
                "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
                answer.message()
            );
        }
    }
}

/// A refused document changes nothing: the policy the repository already carries is the one
/// the read reports.
#[tokio::test]
async fn a_document_naming_no_identity_leaves_the_stored_policy_as_it_was() {
    let router = crate::wire::emulator();
    let name = "shared/principal-forms-kept";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    let baseline = document(&json!({"AWS": iam_arn(OTHER_ACCOUNT, "root")}), json!({}));
    let set = ok(
        &router,
        "the baseline",
        "SetRepositoryPolicy",
        &json!({"repositoryName": name, "policyText": &baseline}),
    )
    .await;

    refused(
        &router,
        "a principal that names nobody",
        "SetRepositoryPolicy",
        &json!({
            "repositoryName": name,
            "policyText": document(&json!({"AWS": "nobody"}), json!({})),
        }),
        INVALID_PARAMETER,
    )
    .await;

    let got = ok(
        &router,
        "the document the repository kept",
        "GetRepositoryPolicy",
        &json!({"repositoryName": name}),
    )
    .await;
    assert_eq!(
        got["policyText"], set["policyText"],
        "the refused document must leave the stored policy as it was, got {:?}",
        got["policyText"]
    );
}
