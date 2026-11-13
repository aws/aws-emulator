// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS its refusals, asserted end-to-end over the wire.
//!
//! Every case drives a real `restJson1` request through the `MultiServiceRouter` the
//! production emulator serves with (the harness in
//! [`super::domain_repository_lifecycle`]), so what is pinned here is the status, the
//! error code and the `message` a CodeArtifact client reads — not a handler's return
//! value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than on one recorded
//! example:
//!
//! 1. A refusal that names a resource the caller addressed IDENTIFIES it: the resource
//!    kind, its name quoted, the parent domain when the resource is nested in one, and the
//!    12-digit account the request addressed the domain to (its `domainOwner`, or the
//!    account the emulator serves when the request named none). So a `ResourceNotFound`
//!    reads `Domain not found. Domain '<d>' owned by account '<a>' does not exist.` and
//!    `Repository not found. Repository '<r>' in domain '<d>' owned by account '<a>' does
//!    not exist.`, and a domain create conflict reads `Domain '<d>' owned by '<a>' already
//!    exists.` A conflict on a name that is unique WITHIN another resource names that
//!    scope instead of the account: `Repository <r> already exists in domain <d>.`
//! 2. A refusal that states a PRECONDITION interpolates nothing: `DeleteDomain` on a
//!    non-empty domain answers the same sentence whatever domain was addressed and
//!    however many repositories it holds.
//! 3. A `@pattern` constraint violation quotes the expression a value has to satisfy,
//!    WITHOUT the `^`/`$` a model writes to demand a whole-string match; every other
//!    violation class keeps the framework's own wording.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::domain_repository_lifecycle::{call, emulator, Answer, ACCOUNT};
use crate::harness::{domain_not_found, repository_not_found};

/// A 12-digit account that is NOT the one this emulator serves: the cross-account
/// `domainOwner` a caller can pin a domain to. The account a refusal reports is the one
/// the REQUEST addressed, which is the whole reason the message carries it.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The error codes this service's refusals use, with the status each carries.
/// Source: `smithy_query(query="operation", operation="DescribeDomain,CreateDomain")` —
/// `ResourceNotFoundException` httpError 404, `ConflictException` 409,
/// `ValidationException` 400.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");
const CONFLICT: (u16, &str) = (409, "ConflictException");
const INVALID: (u16, &str) = (400, "ValidationException");

// The `@pattern` of each member exercised by the validation table, as a refusal must
// state it: the model's expression with the anchors removed. Source:
// `smithy_query(query="operation",
// operation="CreateDomain,CreateRepository,DescribeDomain")` — `DomainName`
// `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`, `RepositoryName`
// `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$`, `AccountId` `^[0-9]{12}$`, `Arn` (the
// `encryptionKey` member) `^\S+$`, `Description` `^\P{C}*$`.
const DOMAIN_NAME_PATTERN: &str = r"[a-z][a-z0-9\-]{0,48}[a-z0-9]";
const REPOSITORY_NAME_PATTERN: &str = r"[A-Za-z0-9][A-Za-z0-9._\-]{1,99}";
const ACCOUNT_ID_PATTERN: &str = "[0-9]{12}";
const ARN_PATTERN: &str = r"\S+";
const DESCRIPTION_PATTERN: &str = r"\P{C}*";

/// One row of a refusal table: a request, and the answer the rule requires.
struct Case {
    label: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// Status and error code.
    answer: (u16, &'static str),
    message: String,
}

// RULE 1, for a resource that is not held: the `ResourceNotFoundException` sentences for a
// domain and for a repository (the parent domain is named there too) come from the shared
// harness, so this suite and `tests/all/error_envelopes.rs` assert one wording.

/// RULE 1, for a create whose name is already held.
fn domain_exists(domain: &str, owner: &str) -> String {
    format!("Domain '{domain}' owned by '{owner}' already exists.")
}

/// RULE 1, for a repository create whose name is already held in the addressed domain:
/// a repository name is taken per DOMAIN, so the sentence names the two things it is unique
/// within and not the account.
fn repository_exists(domain: &str, repository: &str) -> String {
    format!("Repository {repository} already exists in domain {domain}.")
}

/// RULE 2: the precondition `DeleteDomain` states, which interpolates nothing.
const DOMAIN_NOT_EMPTY: &str = "Domain must contain 0 repositories before deletion";

/// RULE 3: the one clause a constraint violation contributes, in the framework's own
/// shape, with `requirement` as the constraint half.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// RULE 3: the `@pattern` requirement, quoting `pattern` as the caller must see it.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The single-violation envelope.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// Assert one row, naming the case and the actual answer on every failure.
fn assert_case(case: &Case, answer: &Answer) {
    let label = &case.label;
    assert_eq!(
        answer.status, case.answer.0,
        "case {label:?}: expected status {}, got {} (body {:?})",
        case.answer.0, answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        case.answer.1,
        "case {label:?}: expected error code {:?}, got {:?}",
        case.answer.1,
        answer.code()
    );
    assert_eq!(
        answer.message(),
        case.message,
        "case {label:?}: expected message {:?}, got {:?}",
        case.message,
        answer.message()
    );
}

async fn run(router: &MultiServiceRouter, cases: &[Case]) {
    for case in cases {
        let answer = call(router, case.method, &case.uri, case.body.clone()).await;
        assert_case(case, &answer);
    }
}

/// Create a domain, and fail loudly if the setup call did not succeed.
async fn seed_domain(router: &MultiServiceRouter, domain: &str) {
    let created = call(
        router,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating domain {domain:?} answered {} ({:?})",
        created.status, created.body
    );
}

/// Create a repository, and fail loudly if the setup call did not succeed.
async fn seed_repository(router: &MultiServiceRouter, domain: &str, repository: &str) {
    let created = call(
        router,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating repository {repository:?} in {domain:?} answered {} ({:?})",
        created.status, created.body
    );
}

/// RULE 1: every not-found refusal names the resource kind, the resource, its parent
/// domain where it has one, and the account the request addressed the domain to.
///
/// The rows differ along what the rule generalises over: the resource kind (domain,
/// repository), the operation that refuses (describe, delete — of the DOMAIN SCOPE it
/// addresses, since the absence of a delete's own target is not a refusal — list, create,
/// and a create whose UPSTREAM names a missing repository), whether `domainOwner` was
/// omitted, pinned to the account served, or pinned to another account, and the extremes
/// of the modelled name lengths (2 and 50 for a domain, 2 and 100 for a repository).
#[tokio::test]
async fn not_found_refusals_name_the_resource_its_parent_and_the_owning_account() {
    let router = emulator();

    // One held domain holding one repository, so a 404 below is the addressed resource
    // being absent rather than the store being empty.
    let held = "held-domain";
    let held_repo = "held-repo";
    seed_domain(&router, held).await;
    seed_repository(&router, held, held_repo).await;

    // The modelled extremes: a 2-character and a 50-character domain name, and a
    // 100-character repository name.
    let shortest_domain = "ab".to_string();
    let longest_domain = format!("d{}e", "x".repeat(48));
    let longest_repo = format!("R{}9", "n".repeat(98));

    let mut cases = vec![
        Case {
            label: "DescribeDomain, absent domain, no domainOwner".to_string(),
            method: "GET",
            uri: format!("/v1/domain?domain={shortest_domain}"),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(&shortest_domain, ACCOUNT),
        },
        Case {
            label: "DescribeDomain, absent 50-character domain".to_string(),
            method: "GET",
            uri: format!("/v1/domain?domain={longest_domain}"),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(&longest_domain, ACCOUNT),
        },
        Case {
            label: "DescribeDomain, HELD domain pinned to another account".to_string(),
            method: "GET",
            uri: format!("/v1/domain?domain={held}&domain-owner={FOREIGN_ACCOUNT}"),
            body: None,
            answer: NOT_FOUND,
            // The domain exists, but not in the account the request addressed — which is
            // exactly the distinction the account in the message carries.
            message: domain_not_found(held, FOREIGN_ACCOUNT),
        },
        Case {
            label: "DescribeDomain, absent domain pinned to the served account".to_string(),
            method: "GET",
            uri: format!("/v1/domain?domain={shortest_domain}&domain-owner={ACCOUNT}"),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(&shortest_domain, ACCOUNT),
        },
        Case {
            label: "DeleteRepository, absent domain".to_string(),
            method: "DELETE",
            uri: format!("/v1/repository?domain={longest_domain}&repository={held_repo}"),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(&longest_domain, ACCOUNT),
        },
        Case {
            label: "ListRepositoriesInDomain, absent domain".to_string(),
            method: "POST",
            uri: format!("/v1/domain/repositories?domain={shortest_domain}"),
            body: Some(json!({})),
            answer: NOT_FOUND,
            message: domain_not_found(&shortest_domain, ACCOUNT),
        },
        Case {
            label: "CreateRepository, absent domain".to_string(),
            method: "POST",
            uri: format!("/v1/repository?domain={shortest_domain}&repository={longest_repo}"),
            body: Some(json!({})),
            answer: NOT_FOUND,
            message: domain_not_found(&shortest_domain, ACCOUNT),
        },
        Case {
            label: "DescribeRepository, absent domain".to_string(),
            method: "GET",
            uri: format!("/v1/repository?domain={shortest_domain}&repository=r1"),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(&shortest_domain, ACCOUNT),
        },
        Case {
            label: "DeleteRepository, held domain pinned to another account".to_string(),
            method: "DELETE",
            uri: format!(
                "/v1/repository?domain={held}&repository={held_repo}&domain-owner={FOREIGN_ACCOUNT}"
            ),
            body: None,
            answer: NOT_FOUND,
            message: domain_not_found(held, FOREIGN_ACCOUNT),
        },
        Case {
            label: "DescribeRepository, absent repository".to_string(),
            method: "GET",
            uri: format!("/v1/repository?domain={held}&repository={longest_repo}"),
            body: None,
            answer: NOT_FOUND,
            message: repository_not_found(held, ACCOUNT, &longest_repo),
        },
        Case {
            label: "DescribeRepository, absent repository, owner pinned".to_string(),
            method: "GET",
            uri: format!("/v1/repository?domain={held}&repository=ab&domain-owner={ACCOUNT}"),
            body: None,
            answer: NOT_FOUND,
            message: repository_not_found(held, ACCOUNT, "ab"),
        },
        Case {
            label: "CreateRepository, upstream names an absent repository".to_string(),
            method: "POST",
            uri: format!("/v1/repository?domain={held}&repository=new-repo"),
            body: Some(json!({ "upstreams": [{ "repositoryName": longest_repo }] })),
            answer: NOT_FOUND,
            message: repository_not_found(held, ACCOUNT, &longest_repo),
        },
    ];

    // The held resources are still readable: the rows above refuse because the resource
    // they address is absent, not because every read refuses.
    cases.sort_by(|a, b| a.label.cmp(&b.label));
    run(&router, &cases).await;

    let still_there = call(
        &router,
        "GET",
        &format!("/v1/repository?domain={held}&repository={held_repo}"),
        None,
    )
    .await;
    assert_eq!(
        still_there.status, 200,
        "the held repository must still be readable, got {} ({:?})",
        still_there.status, still_there.body
    );
}

/// RULE 1, for a conflict: a create whose name is already held quotes the name and the
/// account that owns the colliding resource — and the resource it collided with is
/// readable, so the refusal is about stored state.
///
/// Rows differ along the resource kind and the extremes of the modelled name lengths.
#[tokio::test]
async fn duplicate_create_refusals_quote_the_name_and_the_owning_account() {
    let router = emulator();

    let shortest_domain = "ab".to_string();
    let longest_domain = format!("d{}e", "x".repeat(48));
    let longest_repo = format!("R{}9", "n".repeat(98));

    let mut cases = Vec::new();
    for domain in [shortest_domain.clone(), "my-domain".to_string(), longest_domain] {
        seed_domain(&router, &domain).await;

        // Round trip: the stored record is readable before the duplicate is refused.
        let read = call(&router, "GET", &format!("/v1/domain?domain={domain}"), None).await;
        assert_eq!(
            read.body.pointer("/domain/name").and_then(Value::as_str),
            Some(domain.as_str()),
            "round trip for domain {domain:?}: read back {:?}",
            read.body
        );

        cases.push(Case {
            label: format!("CreateDomain twice, domain {domain:?}"),
            method: "POST",
            uri: format!("/v1/domain?domain={domain}"),
            body: Some(json!({})),
            answer: CONFLICT,
            message: domain_exists(&domain, ACCOUNT),
        });
    }

    for repository in ["ab".to_string(), "My.Repo_1-2".to_string(), longest_repo] {
        seed_repository(&router, &shortest_domain, &repository).await;

        let read = call(
            &router,
            "GET",
            &format!("/v1/repository?domain={shortest_domain}&repository={repository}"),
            None,
        )
        .await;
        assert_eq!(
            read.body
                .pointer("/repository/name")
                .and_then(Value::as_str),
            Some(repository.as_str()),
            "round trip for repository {repository:?}: read back {:?}",
            read.body
        );

        cases.push(Case {
            label: format!("CreateRepository twice, repository {repository:?}"),
            method: "POST",
            uri: format!("/v1/repository?domain={shortest_domain}&repository={repository}"),
            body: Some(json!({})),
            answer: CONFLICT,
            message: repository_exists(&shortest_domain, &repository),
        });
    }

    cases.sort_by(|a, b| a.label.cmp(&b.label));
    run(&router, &cases).await;
}

/// RULE 2: `DeleteDomain` on a domain that still holds repositories states the
/// precondition and interpolates NOTHING — the same sentence for every domain name and
/// every repository count, naming neither.
#[tokio::test]
async fn non_empty_domain_delete_states_the_precondition_only() {
    let router = emulator();

    // Rows differ along the repository count (1, 2, 3) and the domain name, including
    // the modelled extremes.
    let cases: Vec<(String, usize)> = vec![
        ("ab".to_string(), 1),
        ("my-domain".to_string(), 2),
        (format!("d{}e", "x".repeat(48)), 3),
    ];

    for (domain, repositories) in cases {
        seed_domain(&router, &domain).await;
        for index in 0..repositories {
            seed_repository(&router, &domain, &format!("repo-{index}")).await;
        }

        let refused = call(
            &router,
            "DELETE",
            &format!("/v1/domain?domain={domain}"),
            None,
        )
        .await;
        let case = format!("domain {domain:?} holding {repositories} repositories");
        assert_eq!(
            refused.status, CONFLICT.0,
            "case {case:?}: expected status {}, got {} ({:?})",
            CONFLICT.0, refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            CONFLICT.1,
            "case {case:?}: expected error code {:?}, got {:?}",
            CONFLICT.1,
            refused.code()
        );
        assert_eq!(
            refused.message(),
            DOMAIN_NOT_EMPTY,
            "case {case:?}: expected message {DOMAIN_NOT_EMPTY:?}, got {:?}",
            refused.message()
        );
        assert!(
            !refused.message().contains(domain.as_str()),
            "case {case:?}: the precondition names no domain, got {:?}",
            refused.message()
        );
        assert!(
            !refused.message().contains(&repositories.to_string()),
            "case {case:?}: the precondition reports no live count, got {:?}",
            refused.message()
        );

        // The refusal removed nothing: the domain is still readable.
        let read = call(&router, "GET", &format!("/v1/domain?domain={domain}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must remove nothing, got {} ({:?})",
            read.status, read.body
        );
    }
}

/// RULE 3: a `@pattern` violation quotes the expression WITHOUT the model's anchors, for
/// every member that carries one — a path or query member and a body member alike — and
/// every other violation class keeps the framework's own wording.
#[tokio::test]
async fn pattern_refusals_quote_the_expression_without_the_models_anchors() {
    let router = emulator();

    let cases = vec![
        Case {
            label: "CreateDomain, domain outside DomainName's pattern".to_string(),
            method: "POST",
            uri: "/v1/domain?domain=Invalid_Domain_Name".to_string(),
            body: Some(json!({})),
            answer: INVALID,
            message: one_error(&clause(
                "Invalid_Domain_Name",
                "domain",
                &pattern_requirement(DOMAIN_NAME_PATTERN),
            )),
        },
        Case {
            label: "CreateRepository, repository outside RepositoryName's pattern".to_string(),
            method: "POST",
            uri: "/v1/repository?domain=ok-domain&repository=bad~repo".to_string(),
            body: Some(json!({})),
            answer: INVALID,
            message: one_error(&clause(
                "bad~repo",
                "repository",
                &pattern_requirement(REPOSITORY_NAME_PATTERN),
            )),
        },
        Case {
            label: "DescribeDomain, domainOwner outside AccountId's pattern".to_string(),
            method: "GET",
            uri: "/v1/domain?domain=ok-domain&domain-owner=abcdefghijkl".to_string(),
            body: None,
            answer: INVALID,
            message: one_error(&clause(
                "abcdefghijkl",
                "domainOwner",
                &pattern_requirement(ACCOUNT_ID_PATTERN),
            )),
        },
        Case {
            label: "CreateDomain, body member encryptionKey outside Arn's pattern".to_string(),
            method: "POST",
            uri: "/v1/domain?domain=ok-domain".to_string(),
            body: Some(json!({ "encryptionKey": "key with space" })),
            answer: INVALID,
            message: one_error(&clause(
                "key with space",
                "encryptionKey",
                &pattern_requirement(ARN_PATTERN),
            )),
        },
        Case {
            label: "CreateRepository, description outside Description's pattern".to_string(),
            method: "POST",
            uri: "/v1/repository?domain=ok-domain&repository=ok-repo".to_string(),
            body: Some(json!({ "description": "a\u{1}b" })),
            answer: INVALID,
            message: one_error(&clause(
                "a\u{1}b",
                "description",
                &pattern_requirement(DESCRIPTION_PATTERN),
            )),
        },
        Case {
            // A violation class that is NOT `@pattern` keeps the framework's wording:
            // `maxResults` is modelled `@range(min: 1, max: 1000)`.
            label: "ListRepositoriesInDomain, maxResults below its modelled minimum".to_string(),
            method: "POST",
            uri: "/v1/domain/repositories?domain=ok-domain&max-results=0".to_string(),
            body: Some(json!({})),
            answer: INVALID,
            message: one_error(&clause(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        },
    ];

    run(&router, &cases).await;

    // No row above reached a handler, so nothing was created: the refusals are the
    // boundary's, before any state was touched.
    let nothing_stored = call(&router, "GET", "/v1/domain?domain=ok-domain", None).await;
    assert_eq!(
        nothing_stored.status, NOT_FOUND.0,
        "a refused request must store nothing, got {} ({:?})",
        nothing_stored.status, nothing_stored.body
    );

    // Two violations in one request are counted and joined, and the pattern clause is
    // unanchored in that envelope too. The order the boundary reports them in is not
    // part of the rule, so each clause is asserted as present rather than positioned.
    let both = call(
        &router,
        "POST",
        "/v1/repository?domain=BAD&repository=bad~repo",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        both.status, INVALID.0,
        "two violations: expected status {}, got {} ({:?})",
        INVALID.0, both.status, both.body
    );
    let message = both.message();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "two violations: expected the 2-error envelope, got {message:?}"
    );
    for expected in [
        clause("BAD", "domain", &pattern_requirement(DOMAIN_NAME_PATTERN)),
        clause(
            "bad~repo",
            "repository",
            &pattern_requirement(REPOSITORY_NAME_PATTERN),
        ),
    ] {
        assert!(
            message.contains(&expected),
            "two violations: expected a clause {expected:?} in {message:?}"
        );
    }
}
