// SPDX-License-Identifier: CC0-1.0

//! `GetRepositoryEndpoint` end to end over the wire.
//!
//! Every case drives the same front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by a `codeartifact`
//! endpoint host, speaking restJson1 — so what is asserted is the status, error code and
//! body a CodeArtifact client sees. The `Answer`/`emulator`/`call` harness is the
//! `pub(crate)` one `domain_repository_lifecycle.rs` exposes; this suite adds only the
//! endpoint-shaped helpers.
//!
//! The rules under test, each over a TABLE rather than one recorded example:
//!
//! 1. A repository's endpoint for a package format is built from the repository's own
//!    stored identity and the region the request addressed, under the base this emulator is
//!    reached at:
//!    `<base>/<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/<format>/<repository>/`,
//!    on the `on.aws` suffix when `endpointType` is `dualstack` — so it is reported for
//!    EVERY modelled `PackageFormat` and agrees with what `DescribeRepository` reports
//!    about the same repository. That the announced URL is an address a client can DIAL is
//!    `tests/all/announced_repository_endpoint.rs`.
//! 2. `endpointType` is optional and defaults to `ipv4`.
//! 3. Reporting an endpoint resolves a resource: a domain or a repository the service
//!    does not hold (including a domain pinned to another `domainOwner`) is
//!    `ResourceNotFoundException` (404), while a sibling repository in the same domain
//!    still serves.
//! 4. A value outside a modelled enum's set is refused with `ValidationException` (400),
//!    never answered with an endpoint.
//! 5. The operation only reports: reading an endpoint, repeatedly and in any region,
//!    leaves every later read — of the endpoint and of the repository — unchanged.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The enum catalogues come from this service's Smithy model
// (`smithy_query(query="shape", shape="PackageFormat,EndpointType")`), re-exported as the
// generated `*_VALUES` constants — so a model change moves these tables rather than
// leaving a hand-copied list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::{
    ENDPOINT_TYPE_VALUES, PACKAGE_FORMAT_VALUES,
};

use crate::domain_repository_lifecycle::{create_repository, describe_repository};
use crate::harness::{call, call_in_region, create_domain, emulator, Answer, ACCOUNT, REGION};

/// `GetRepositoryEndpoint`, signed for `region`.
async fn get_endpoint_in_region(
    router: &MultiServiceRouter,
    region: &str,
    domain: &str,
    repository: &str,
    format: &str,
    endpoint_type: Option<&str>,
    domain_owner: Option<&str>,
) -> Answer {
    let mut uri =
        format!("/v1/repository/endpoint?domain={domain}&repository={repository}&format={format}");
    if let Some(endpoint_type) = endpoint_type {
        uri.push_str(&format!("&endpointType={endpoint_type}"));
    }
    if let Some(owner) = domain_owner {
        uri.push_str(&format!("&domain-owner={owner}"));
    }
    call_in_region(router, region, "GET", &uri, None).await
}

/// [`get_endpoint_in_region`] for the harness's default region.
async fn get_endpoint(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    format: &str,
    endpoint_type: Option<&str>,
) -> Answer {
    get_endpoint_in_region(router, REGION, domain, repository, format, endpoint_type, None).await
}

/// The `repositoryEndpoint` of a successful answer.
fn endpoint_of(answer: &Answer) -> String {
    answer
        .body
        .get("repositoryEndpoint")
        .and_then(Value::as_str)
        .unwrap_or("<missing repositoryEndpoint>")
        .to_string()
}

/// The host suffix an `endpointType` selects. `dualstack` is the one documented
/// alternative; every other legal value (today only `ipv4`) is the standard regional
/// host.
fn host_suffix(endpoint_type: &str) -> &'static str {
    if endpoint_type == "dualstack" {
        "on.aws"
    } else {
        "amazonaws.com"
    }
}

/// The endpoint rule, as one string: what this emulator announces for a repository
/// addressed in `region`. The authority is the base the emulator is reachable at — these
/// suites drive an unserved router, which reports
/// [`smithy_server_runtime::UNSERVED_URL_BASE`] — and AWS's own published host travels in
/// the path ahead of the two documented segments.
fn expected_endpoint(
    region: &str,
    domain: &str,
    owner: &str,
    format: &str,
    repository: &str,
    endpoint_type: &str,
) -> String {
    format!(
        "{}/{domain}-{owner}.d.codeartifact.{region}.{}/{format}/{repository}/",
        smithy_server_runtime::UNSERVED_URL_BASE,
        host_suffix(endpoint_type)
    )
}

/// The (domain, repository) name shapes the table spans: the modelled minimum lengths,
/// a plain pair, and the character classes and the maximum lengths `RepositoryName`
/// (len 2..100, `^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$`) and `DomainName` (len 2..50,
/// `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`) admit.
fn repository_cases() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "shortest legal names",
            "ab".to_string(),
            "r1".to_string(),
        ),
        (
            "hyphenated domain, mixed-case dotted repository",
            "my-domain".to_string(),
            "My.Repo_1-2".to_string(),
        ),
        (
            "modelled maximum name lengths",
            format!("d{}e", "x".repeat(48)),
            format!("R{}9", "n".repeat(98)),
        ),
    ]
}

/// RULES 1 and 2: every modelled package format, on every endpoint type (and with none
/// supplied), answers the endpoint built from the repository's stored identity — and that
/// identity is the one `DescribeRepository` reports.
#[tokio::test]
async fn every_package_format_reports_the_endpoint_of_the_stored_repository() {
    let router = emulator();

    // `None` is the "no endpointType supplied" row: it must resolve to the documented
    // `ipv4` default, which is why its expectation uses the same standard host.
    let endpoint_types: Vec<Option<&str>> = std::iter::once(None)
        .chain(ENDPOINT_TYPE_VALUES.iter().copied().map(Some))
        .collect();

    for (label, domain, repository) in repository_cases() {
        let created_domain = create_domain(&router, &domain, None).await;
        assert_eq!(
            created_domain.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created_domain:?}"
        );
        let created_repository = create_repository(&router, &domain, &repository, None, &[]).await;
        assert_eq!(
            created_repository.status, 200,
            "case {label:?}: expected CreateRepository 200, got {created_repository:?}"
        );

        // The owner and domain the endpoint's host must name are the ones the repository
        // record itself reports, not constants this test chose.
        let described = describe_repository(&router, &domain, &repository).await;
        let record = described.body["repository"].clone();
        let owner = record["domainOwner"]
            .as_str()
            .unwrap_or("<missing domainOwner>")
            .to_string();
        let domain_name = record["domainName"]
            .as_str()
            .unwrap_or("<missing domainName>")
            .to_string();
        assert_eq!(
            owner, ACCOUNT,
            "case {label:?}: expected the emulator account as domainOwner, got {record:?}"
        );

        for format in PACKAGE_FORMAT_VALUES {
            for endpoint_type in &endpoint_types {
                let answer =
                    get_endpoint(&router, &domain, &repository, format, *endpoint_type).await;
                assert_eq!(
                    answer.status, 200,
                    "case {label:?} format {format:?} endpointType {endpoint_type:?}: \
                     expected 200, got {answer:?}"
                );
                let expected = expected_endpoint(
                    REGION,
                    &domain_name,
                    &owner,
                    format,
                    &repository,
                    endpoint_type.unwrap_or("ipv4"),
                );
                let reported = endpoint_of(&answer);
                assert_eq!(
                    reported, expected,
                    "case {label:?} format {format:?} endpointType {endpoint_type:?}: \
                     expected {expected:?}, got {reported:?}"
                );
            }
        }

        // A dual-stack endpoint differs from the default one ONLY in its host suffix: the
        // same repository, the same two path segments.
        let ipv4 = endpoint_of(&get_endpoint(&router, &domain, &repository, "npm", Some("ipv4")).await);
        let dualstack =
            endpoint_of(&get_endpoint(&router, &domain, &repository, "npm", Some("dualstack")).await);
        assert_eq!(
            ipv4.replace(".amazonaws.com/", ".on.aws/"),
            dualstack,
            "case {label:?}: dualstack must be the same endpoint on the on.aws suffix, \
             got {ipv4:?} and {dualstack:?}"
        );
    }
}

/// RULE 1, on the region: the endpoint names the regional endpoint the REQUEST
/// addressed, while the repository's stored ARN keeps the region it was created in.
#[tokio::test]
async fn the_endpoint_names_the_region_the_request_addressed() {
    let router = emulator();
    let domain = "travelling";
    let repository = "packages";

    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed before the endpoint is read"
    );
    let created = create_repository(&router, domain, repository, None, &[]).await;
    assert_eq!(created.status, 200, "CreateRepository failed: {created:?}");
    let created_arn = created.body["repository"]["arn"]
        .as_str()
        .unwrap_or("<missing arn>")
        .to_string();

    // The rows are the regional endpoints the same repository is read through,
    // including the one it was created in.
    for region in [REGION, "us-west-2", "eu-central-1", "ap-southeast-2"] {
        for endpoint_type in ENDPOINT_TYPE_VALUES {
            let answer = get_endpoint_in_region(
                &router,
                region,
                domain,
                repository,
                "maven",
                Some(endpoint_type),
                None,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {region:?}/{endpoint_type:?}: expected 200, got {answer:?}"
            );
            let reported = endpoint_of(&answer);
            let expected = expected_endpoint(
                region,
                domain,
                ACCOUNT,
                "maven",
                repository,
                endpoint_type,
            );
            assert_eq!(
                reported, expected,
                "case {region:?}/{endpoint_type:?}: expected {expected:?}, got {reported:?}"
            );
        }

        // Reading through another region neither moves nor re-mints the record.
        let described = describe_repository(&router, domain, repository).await;
        assert_eq!(
            described.body["repository"]["arn"].as_str(),
            Some(created_arn.as_str()),
            "case {region:?}: the stored ARN must keep its creating region, got {described:?}"
        );
    }
}

/// RULE 3: the endpoint of a resource the service does not hold is
/// `ResourceNotFoundException` (404) — and a sibling repository in the same domain still
/// serves.
#[tokio::test]
async fn an_unheld_domain_or_repository_has_no_endpoint() {
    let router = emulator();
    let domain = "held";

    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    for repository in ["alpha", "beta"] {
        assert_eq!(
            create_repository(&router, domain, repository, None, &[])
                .await
                .status,
            200,
            "CreateRepository {repository:?} must succeed"
        );
    }

    // (case, domain, repository, domainOwner)
    let cases: Vec<(&str, &str, &str, Option<String>)> = vec![
        ("no such domain", "absent", "alpha", None),
        ("no such repository in a held domain", domain, "gamma", None),
        (
            "a held domain pinned to another account",
            domain,
            "alpha",
            Some("210987654321".to_string()),
        ),
        (
            "an absent domain pinned to its own account",
            "absent",
            "alpha",
            Some(ACCOUNT.to_string()),
        ),
    ];

    for (case, case_domain, case_repository, owner) in cases {
        let answer = get_endpoint_in_region(
            &router,
            REGION,
            case_domain,
            case_repository,
            "pypi",
            None,
            owner.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert!(
            answer.body.get("repositoryEndpoint").is_none(),
            "case {case:?}: a refusal must carry no endpoint, got {answer:?}"
        );
    }

    // The sibling, and the pinned-to-its-own-owner read of a held domain, still serve.
    for (case, owner) in [
        ("sibling repository", None),
        ("pinned to the owning account", Some(ACCOUNT.to_string())),
    ] {
        let answer = get_endpoint_in_region(
            &router,
            REGION,
            domain,
            "beta",
            "pypi",
            None,
            owner.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            endpoint_of(&answer),
            expected_endpoint(REGION, domain, ACCOUNT, "pypi", "beta", "ipv4"),
            "case {case:?}: unexpected endpoint, got {answer:?}"
        );
    }
}

/// RULE 4: a `format` or `endpointType` outside its modelled value set is refused, and a
/// format the model DOES carry (one this suite's rule table also covers, `swift`) is not.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    let router = emulator();
    let domain = "enums";
    let repository = "repo";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200
    );

    // Rows: a format outside the set, a format differing only in case, and an
    // endpointType outside the set. `ipv6` is the plausible-but-unmodelled neighbour of
    // `dualstack`/`ipv4`.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("an unmodelled format", "brew", None),
        ("a modelled format in the wrong case", "NPM", None),
        ("an unmodelled endpointType", "npm", Some("ipv6")),
        ("an endpointType in the wrong case", "npm", Some("IPv4")),
    ];

    for (case, format, endpoint_type) in cases {
        let answer = get_endpoint(&router, domain, repository, format, endpoint_type).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // The control row: a format the model carries is served.
    let served = get_endpoint(&router, domain, repository, "swift", Some("dualstack")).await;
    assert_eq!(served.status, 200, "swift must be served, got {served:?}");
}

/// RULE 5: reporting an endpoint writes nothing — repeated reads, in any region and for
/// any format, answer the same thing, and the repository record is untouched.
#[tokio::test]
async fn reading_an_endpoint_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let domain = "stable";
    let repository = "repo";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    assert_eq!(
        create_repository(&router, domain, repository, Some("unchanged"), &[])
            .await
            .status,
        200
    );

    let before = describe_repository(&router, domain, repository).await.body;
    let first = endpoint_of(&get_endpoint(&router, domain, repository, "nuget", None).await);

    // Read it again, through other regions and formats, then ask for the first one back.
    for region in [REGION, "eu-west-1"] {
        for format in PACKAGE_FORMAT_VALUES {
            let answer =
                get_endpoint_in_region(&router, region, domain, repository, format, None, None)
                    .await;
            assert_eq!(
                answer.status, 200,
                "case {region:?}/{format:?}: expected 200, got {answer:?}"
            );
        }
    }

    let again = endpoint_of(&get_endpoint(&router, domain, repository, "nuget", None).await);
    assert_eq!(
        again, first,
        "the endpoint must be stable across reads, got {first:?} then {again:?}"
    );

    let after = describe_repository(&router, domain, repository).await.body;
    assert_eq!(
        after, before,
        "reading endpoints must not change the repository record: {before:?} became {after:?}"
    );

    // And the domain still holds exactly the one repository it was given.
    let listed = call(
        &router,
        "POST",
        &format!("/v1/domain/repositories?domain={domain}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(listed.status, 200, "ListRepositoriesInDomain: {listed:?}");
    assert_eq!(
        listed.body["repositories"].as_array().map(Vec::len),
        Some(1),
        "the domain must still hold exactly one repository, got {listed:?}"
    );
}
