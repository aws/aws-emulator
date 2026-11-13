// SPDX-License-Identifier: CC0-1.0

//! The sentence `AssociateExternalConnection` refuses an unknown registry with.
//!
//! RULE: a not-found refusal identifies the missing resource the way that resource is
//! ADDRESSED and reports nothing the caller did not send — the registries CodeArtifact can
//! connect to are a service-wide catalog addressed by name alone, so the refusal for one
//! that is not in it carries the `externalConnection` the request sent and no repository,
//! domain or owner-account clause; a resource that exists only INSIDE another (a
//! repository in a domain, a domain in an account) still names its containers.
//!
//! Asserted over a TABLE whose rows vary everything the sentence must be independent of —
//! the repository name, the domain name, whether the call named a `domainOwner`, and which
//! unsupported name was asked for (length, scope prefix, case, suffix) — plus the control
//! rows for the two container-scoped refusals of the same operation, which do name what
//! holds the missing resource.

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    create_domain, domain_not_found, emulator, external_connection_not_found,
    repository_not_found, Answer, ACCOUNT,
};

use crate::external_connection::{associate, associate_as_owner, create_repository};

/// One row of the table: the context a call addresses, and the unsupported connection it
/// asks for.
#[derive(Debug)]
struct Case {
    label: &'static str,
    domain: &'static str,
    repository: &'static str,
    /// Sent as `domain-owner` when set — the account the caller addressed.
    domain_owner: Option<&'static str>,
    connection: &'static str,
}

/// Associate, naming the `domainOwner` the call addresses where the row pins one.
async fn associate_case(router: &MultiServiceRouter, case: &Case) -> Answer {
    associate_as_owner(
        router,
        case.domain,
        case.repository,
        case.connection,
        case.domain_owner,
    )
    .await
}

/// The refusal for a registry that is not in the catalog names that registry only — the
/// same sentence whatever repository, domain or owner account the call addressed.
#[tokio::test]
async fn an_unknown_registry_is_refused_by_name_alone() {
    let router = emulator();
    // Two domains, so a row's sentence cannot be read off "the" domain of the suite. Both
    // satisfy `DomainName` (len 2..50, pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`).
    for domain in ["ecs-one", "ecs-another-much-longer-domain-name"] {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "domain {domain:?} setup must succeed"
        );
    }

    // Rows vary the context (domain, repository name and length, whether the request named
    // the owning account) AND how the name misses the catalog — an unknown registry, a
    // bare registry name with no scope, a prefix of a published name, a case variant, a
    // published name with a suffix, and a two-character name at the modelled minimum
    // length. Every connection name satisfies `ExternalConnectionName` (len 2..100,
    // pattern `^[A-Za-z0-9][A-Za-z0-9._\-:]{1,99}$`), so the boundary admits each one and
    // what is missing is the registry.
    let cases: &[Case] = &[
        Case {
            label: "unknown registry, short repository",
            domain: "ecs-one",
            repository: "r1",
            domain_owner: None,
            connection: "public:does-not-exist",
        },
        Case {
            label: "unscoped registry name, owner named",
            domain: "ecs-one",
            repository: "ecs-repository-with-a-considerably-longer-name",
            domain_owner: Some(ACCOUNT),
            connection: "npmjs",
        },
        Case {
            label: "prefix of a published name, other domain",
            domain: "ecs-another-much-longer-domain-name",
            repository: "ecs.dotted_repo-2",
            domain_owner: None,
            connection: "public:maven",
        },
        Case {
            label: "case variant of a published name, owner named",
            domain: "ecs-another-much-longer-domain-name",
            repository: "r3",
            domain_owner: Some(ACCOUNT),
            connection: "PUBLIC:NPMJS",
        },
        Case {
            label: "published name with a suffix",
            domain: "ecs-one",
            repository: "ecs-repo-4",
            domain_owner: None,
            connection: "public:npmjs.org",
        },
        Case {
            label: "shortest name the member admits",
            domain: "ecs-one",
            repository: "ecs-repo-5",
            domain_owner: None,
            connection: "x9",
        },
    ];

    for case in cases {
        assert_eq!(
            create_repository(&router, case.domain, case.repository)
                .await
                .status,
            200,
            "case {label:?}: repository setup must succeed",
            label = case.label
        );

        let answer = associate_case(&router, case).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: a registry the catalog does not hold is not found, got {answer:?}"
        );
        let message = answer.message();
        assert_eq!(
            message,
            external_connection_not_found(case.connection),
            "case {case:?}: expected the connection-only sentence, got {message:?}"
        );
        // The same thing, stated as the invariant: nothing the sentence carries comes from
        // anywhere but the request's `externalConnection`.
        for (part, value) in [
            ("repository", case.repository),
            ("domain", case.domain),
            ("account", ACCOUNT),
        ] {
            assert!(
                !message.contains(value),
                "case {case:?}: the sentence must not name the {part} ({value:?}), got \
                 {message:?}"
            );
        }
        assert_eq!(
            answer.body["resourceId"].as_str(),
            Some(case.connection),
            "case {case:?}: the refusal names the connection asked for, got {answer:?}"
        );
    }

    // One name asked for in two different contexts gets the identical sentence: the
    // message is a function of the connection name alone.
    let repeated = "public:does-not-exist";
    let first = associate(&router, "ecs-one", "r1", repeated).await;
    let second = associate(
        &router,
        "ecs-another-much-longer-domain-name",
        "r3",
        repeated,
    )
    .await;
    assert_eq!(
        first.message(),
        second.message(),
        "the same name in two contexts must get one sentence, got {:?} and {:?}",
        first.message(),
        second.message()
    );
}

/// The control: the SAME operation's container-scoped refusals keep naming what holds the
/// missing resource, so the rule is about how a resource is addressed and not about
/// stripping every refusal of its context.
#[tokio::test]
async fn a_container_scoped_miss_still_names_its_containers() {
    let router = emulator();
    let domain = "ecs-controls";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );
    assert_eq!(
        create_repository(&router, domain, "ecs-present").await.status,
        200,
        "repository setup must succeed"
    );

    // Rows differ in WHICH container-scoped resource is missing, over a supported registry
    // name so the refusal is about the target and not about the catalog.
    let cases: &[(&str, Case, String)] = &[
        (
            "an absent domain",
            Case {
                label: "absent domain",
                domain: "ecs-absent",
                repository: "ecs-present",
                domain_owner: None,
                connection: "public:npmjs",
            },
            domain_not_found("ecs-absent", ACCOUNT),
        ),
        (
            "an absent repository in a domain that is held",
            Case {
                label: "absent repository",
                domain: "ecs-controls",
                repository: "ecs-absent-repo",
                domain_owner: None,
                connection: "public:pypi",
            },
            repository_not_found("ecs-controls", ACCOUNT, "ecs-absent-repo"),
        ),
        (
            "a domainOwner that owns no such domain",
            Case {
                label: "wrong owner",
                domain: "ecs-controls",
                repository: "ecs-present",
                domain_owner: Some("123456789012"),
                connection: "public:npmjs",
            },
            domain_not_found("ecs-controls", "123456789012"),
        ),
    ];

    for (label, case, expected) in cases {
        let answer = associate_case(&router, case).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            &answer.message(),
            expected,
            "case {label:?}: expected the container-scoped sentence, got {:?}",
            answer.message()
        );
    }
}
