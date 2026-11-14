// SPDX-License-Identifier: CC0-1.0

//! WHERE a resource-based policy document names the resources its statements apply to, end
//! to end over the wire.
//!
//! The rule under test: a statement names the resources it applies to exactly where the
//! document's SCOPE leaves them unsettled. A repository policy is attached to ONE
//! repository, which is the resource every statement of it applies to, so a statement that
//! names a resource of its own — the wildcard and the repository's own ARN alike — is a
//! document ECR will not hold; the registry policy is attached to no repository, so each of
//! its statements must name the resources it covers. Either way the refusal is HTTP 400
//! `InvalidParameterException` carrying that scope's constraint phrase, nothing is written,
//! and a document that satisfies the rule is stored and read back.
//!
//! The rows span what the rule generalises over: which element names the resources
//! (`Resource` and its complement `NotResource`), how the value is written (a scalar, a list
//! of one, a list of several, an empty list), what it names (the wildcard, a repository ARN,
//! this repository's own ARN), how many statements the document carries and which of them
//! breaks the rule.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_document_canonical_form::document_content;
use crate::wire::{call, ok, refused, REGION, REGISTRY_ID};

/// The error code both refusals carry (`smithy_query(query="operation",
/// operation="SetRepositoryPolicy,PutRegistryPolicy")` declares `InvalidParameterException`,
/// HTTP 400, on each).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The template ECR states a refused document parameter with, over the API-documented label
/// of the parameter carrying the document.
fn clause(constraint: &str) -> String {
    format!("Invalid parameter at 'PolicyText' failed to satisfy constraint: '{constraint}'")
}

/// The constraint phrase a repository policy ECR will not accept fails.
fn repository_constraint() -> String {
    clause("Invalid repository policy provided")
}

/// The constraint phrase a registry policy ECR will not accept fails.
fn registry_constraint() -> String {
    clause("Invalid registry policy provided")
}

/// The ARN of a repository of this registry, as a statement would name one.
fn repository_arn(name: &str) -> String {
    format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}")
}

/// A one-statement document granting a pull action to one account, with `resource_elements`
/// merged into its statement — the elements under test.
fn document(resource_elements: Value) -> String {
    let mut statement = json!({
        "Sid": "AllowPull",
        "Effect": "Allow",
        "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
        "Action": "ecr:BatchGetImage",
    });
    let members = statement
        .as_object_mut()
        .expect("the statement is an object");
    for (element, value) in resource_elements
        .as_object()
        .expect("the resource elements are an object")
    {
        members.insert(element.clone(), value.clone());
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// One row: the resource elements a statement carries, named for the assertions.
struct Row {
    /// What the row is about.
    case: &'static str,
    /// The resource elements merged into the document's statement; an empty object for a
    /// statement that names no resource at all.
    elements: Value,
}

/// The ways a statement names the resources it applies to, from naming none to naming
/// several. A row is accepted by exactly one of the two scopes: by the repository policy
/// when it names none, and by the registry policy when it names at least one.
fn rows() -> Vec<Row> {
    let row = |case: &'static str, elements: Value| Row { case, elements };
    vec![
        row("no resource element at all", json!({})),
        row("the wildcard", json!({"Resource": "*"})),
        row(
            "a repository ARN of this registry",
            json!({"Resource": repository_arn("shared/base")}),
        ),
        row(
            "a one-value Resource list",
            json!({"Resource": [repository_arn("shared/base")]}),
        ),
        row(
            "a Resource list of several",
            json!({"Resource": [repository_arn("shared/base"), repository_arn("shared/app")]}),
        ),
        row(
            "the complement element",
            json!({"NotResource": repository_arn("shared/secret")}),
        ),
        row(
            "both elements at once",
            json!({"Resource": "*", "NotResource": repository_arn("shared/secret")}),
        ),
        row("an empty Resource list", json!({"Resource": []})),
    ]
}

/// Whether the row's statement CARRIES a resource element at all, which a repository policy
/// — attached to the repository its statements apply to — may not, however the element is
/// written.
fn carries_a_resource_element(elements: &Value) -> bool {
    ["Resource", "NotResource"]
        .iter()
        .any(|element| elements.get(*element).is_some())
}

/// Whether the row's statement NAMES at least one resource, which is what a registry policy
/// requires of every statement. An empty list names none.
fn names_a_resource(elements: &Value) -> bool {
    ["Resource", "NotResource"].iter().any(|element| {
        elements.get(*element).is_some_and(|value| match value {
            Value::Array(entries) => !entries.is_empty(),
            Value::String(text) => !text.is_empty(),
            _ => false,
        })
    })
}

/// A repository policy is attached to the repository its statements apply to: a statement
/// naming resources of its own is refused, and the policy the repository already carries
/// stays as it was.
#[tokio::test]
async fn a_repository_policy_statement_may_name_no_resource_of_its_own() {
    let router = crate::wire::emulator();
    let name = "shared/base";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    // Every row starts from a document the repository DOES hold, so a refusal is visible as
    // the document it keeps.
    let baseline = document(json!({}));
    ok(
        &router,
        "the baseline",
        "SetRepositoryPolicy",
        &json!({"repositoryName": name, "policyText": &baseline}),
    )
    .await;

    for row in rows() {
        let case = row.case;
        let submitted = document(row.elements.clone());
        if carries_a_resource_element(&row.elements) {
            let body = refused(
                &router,
                case,
                "SetRepositoryPolicy",
                &json!({"repositoryName": name, "policyText": &submitted}),
                INVALID_PARAMETER,
            )
            .await;
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                repository_constraint(),
                "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
                body["message"]
            );
        } else {
            let set = ok(
                &router,
                case,
                "SetRepositoryPolicy",
                &json!({"repositoryName": name, "policyText": &submitted}),
            )
            .await;
            assert_eq!(
                document_content(set["policyText"].as_str().unwrap_or_default()),
                document_content(&submitted),
                "case {case:?}: the document must be stored, got {:?}",
                set["policyText"]
            );
        }

        // What the repository holds: the baseline, whether the row was refused or stored —
        // every accepted row of this table submits that same document.
        let got = ok(
            &router,
            case,
            "GetRepositoryPolicy",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            document_content(got["policyText"].as_str().unwrap_or_default()),
            document_content(&baseline),
            "case {case:?}: the repository must hold the baseline document, got {:?}",
            got["policyText"]
        );
    }
}

/// The registry policy is attached to no repository, so each statement names the resources
/// it applies to: one that names none is refused, and the registry keeps the policy it had.
#[tokio::test]
async fn a_registry_policy_statement_must_name_the_resources_it_applies_to() {
    let router = crate::wire::emulator();

    // The registry starts from a policy it DOES hold, so a refusal is visible as the
    // document it keeps.
    let baseline = document(json!({"Resource": "*"}));
    ok(
        &router,
        "the baseline",
        "PutRegistryPolicy",
        &json!({"policyText": &baseline}),
    )
    .await;

    for row in rows() {
        let case = row.case;
        let submitted = document(row.elements.clone());
        if names_a_resource(&row.elements) {
            let put = ok(
                &router,
                case,
                "PutRegistryPolicy",
                &json!({"policyText": &submitted}),
            )
            .await;
            assert_eq!(
                document_content(put["policyText"].as_str().unwrap_or_default()),
                document_content(&submitted),
                "case {case:?}: the document must be stored, got {:?}",
                put["policyText"]
            );
            // Back to the baseline, so the next row's refusal is read against one document.
            ok(
                &router,
                case,
                "PutRegistryPolicy",
                &json!({"policyText": &baseline}),
            )
            .await;
        } else {
            let body = refused(
                &router,
                case,
                "PutRegistryPolicy",
                &json!({"policyText": &submitted}),
                INVALID_PARAMETER,
            )
            .await;
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                registry_constraint(),
                "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
                body["message"]
            );
        }

        let got = ok(&router, case, "GetRegistryPolicy", &json!({})).await;
        assert_eq!(
            document_content(got["policyText"].as_str().unwrap_or_default()),
            document_content(&baseline),
            "case {case:?}: the registry must hold the baseline document, got {:?}",
            got["policyText"]
        );
    }
}

/// The rule reads EVERY statement of the document, not only the first: a multi-statement
/// document is refused wherever the statement that breaks the rule sits, and the whole
/// document is refused rather than the one statement.
#[tokio::test]
async fn every_statement_of_the_document_is_read() {
    let router = crate::wire::emulator();
    let name = "shared/every-statement";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    let statement = |sid: &str, resource: Option<&str>| {
        let mut members = json!({
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
            "Action": "ecr:BatchGetImage",
        });
        if let Some(resource) = resource {
            members
                .as_object_mut()
                .expect("the statement is an object")
                .insert("Resource".to_string(), json!(resource));
        }
        members
    };
    let arn = repository_arn(name);

    for (case, statements, repository_accepts) in [
        (
            "the resource element in the first of three statements",
            vec![
                statement("one", Some("*")),
                statement("two", None),
                statement("three", None),
            ],
            false,
        ),
        (
            "the resource element in the middle statement",
            vec![
                statement("one", None),
                statement("two", Some(arn.as_str())),
                statement("three", None),
            ],
            false,
        ),
        (
            "the resource element in the last statement",
            vec![
                statement("one", None),
                statement("two", None),
                statement("three", Some("*")),
            ],
            false,
        ),
        (
            "no statement naming a resource",
            vec![
                statement("one", None),
                statement("two", None),
                statement("three", None),
            ],
            true,
        ),
    ] {
        let submitted = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
        let answer = call(
            &router,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": &submitted}),
        )
        .await;
        if repository_accepts {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the document must be stored, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, INVALID_PARAMETER),
                "case {case:?}: the whole document must be refused, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str().unwrap_or_default(),
                repository_constraint(),
                "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
                answer.body["message"]
            );
        }

        // The registry policy reads the same statements under its own half of the rule: a
        // document is a registry policy only when EVERY statement names its resources.
        let answer = call(
            &router,
            "PutRegistryPolicy",
            &json!({"policyText": &submitted}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, INVALID_PARAMETER),
            "case {case:?}: a statement naming no resources is no registry policy, got {} \
             with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            registry_constraint(),
            "case {case:?}: the refusal states the parameter and the constraint, got {:?}",
            answer.body["message"]
        );
    }
}

/// A refused document leaves the resource with NO policy at all when it had none: the
/// refusal is not a write, so the read that follows still reports the absence.
#[tokio::test]
async fn a_refused_document_attaches_nothing() {
    let router: MultiServiceRouter = crate::wire::emulator();
    let name = "shared/nothing-attached";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;

    refused(
        &router,
        "a repository policy naming a resource",
        "SetRepositoryPolicy",
        &json!({"repositoryName": name, "policyText": document(json!({"Resource": "*"}))}),
        INVALID_PARAMETER,
    )
    .await;
    refused(
        &router,
        "the repository policy that was never attached",
        "GetRepositoryPolicy",
        &json!({"repositoryName": name}),
        "RepositoryPolicyNotFoundException",
    )
    .await;

    refused(
        &router,
        "a registry policy naming no resources",
        "PutRegistryPolicy",
        &json!({"policyText": document(json!({}))}),
        INVALID_PARAMETER,
    )
    .await;
    refused(
        &router,
        "the registry policy that was never attached",
        "GetRegistryPolicy",
        &json!({}),
        "RegistryPolicyNotFoundException",
    )
    .await;
}
