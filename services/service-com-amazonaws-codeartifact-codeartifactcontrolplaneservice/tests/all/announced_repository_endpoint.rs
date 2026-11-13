// SPDX-License-Identifier: CC0-1.0

//! The address `GetRepositoryEndpoint` ANNOUNCES, and that a client can actually dial it.
//!
//! The RULE under test: a URL this emulator hands a client to send later requests to is
//! rooted on the endpoint THIS emulator answers on — the address the request reached it at,
//! or the base it is served at — and the resource's own identity travels in the PATH
//! beneath it. For a repository endpoint that path is the host AWS publishes
//! (`<domain>-<domainOwner>.d.codeartifact.<region>.<suffix>`, on `on.aws` for the
//! `dualstack` `EndpointType`) followed by the two documented segments
//! `/<format>/<repository>/`. A URL on AWS's own authority would resolve away from this
//! process, so nothing a client then sends to it — an npm publish, a pip install — would
//! ever arrive.
//!
//! Each case is a TABLE rather than one example, spanning what the rule generalises over:
//!
//! 1. the base the emulator is reachable at, the `EndpointType` asked for and every
//!    modelled `PackageFormat`;
//! 2. the address the REQUEST reached, which is what a client configured with one local
//!    endpoint URL resolves — against a request that addressed the service's own published
//!    host, which resolves nothing here and is answered the served base;
//! 3. the npm registry, driven end to end at the announced URL and nowhere else: ping,
//!    publish, packument and tarball, with the control plane describing what was stored;
//! 4. two domains holding a repository of the SAME name, whose announced endpoints stay
//!    distinct and each serve their own repository;
//! 5. the pypi index, served at its own announced URL.

use serde_json::Value;
use smithy_server_runtime::{MultiServiceRouter, UNSERVED_URL_BASE};

// The enum catalogues come from this service's Smithy model
// (`smithy_query(query="shape", shape="PackageFormat,EndpointType")`), re-exported as the
// generated `*_VALUES` constants — so a model change moves these tables rather than leaving
// a hand-copied list behind.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::{
    ENDPOINT_TYPE_VALUES, PACKAGE_FORMAT_VALUES,
};

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{
    auth_for, call, create_domain, emulator_served_at, Answer, ACCOUNT, REGION,
};
use crate::npm_registry_endpoint::{publish_document, registry, tarball_bytes, token_for};

/// The base a served emulator in these cases reports, with the loopback address and port a
/// data port is published at.
const SERVED_BASE: &str = "https://127.0.0.1:9297";

/// `GetRepositoryEndpoint`, sent to `host` — the address the client reached this emulator
/// at, which is what the announced URL has to lead back to.
async fn get_endpoint_at(
    router: &MultiServiceRouter,
    host: &str,
    domain: &str,
    repository: &str,
    format: &str,
    endpoint_type: Option<&str>,
) -> Answer {
    let mut uri =
        format!("/v1/repository/endpoint?domain={domain}&repository={repository}&format={format}");
    if let Some(endpoint_type) = endpoint_type {
        uri.push_str(&format!("&endpointType={endpoint_type}"));
    }
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", host)
        .header("authorization", auth_for(REGION))
        .header("content-type", "application/json")
        .body(bytes::Bytes::new())
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    Answer {
        status,
        error_type,
        body,
    }
}

/// The `repositoryEndpoint` of an answer that must have succeeded.
fn announced(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetRepositoryEndpoint 200, got {answer:?}"
    );
    answer
        .body
        .get("repositoryEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no repositoryEndpoint in {answer:?}"))
        .to_string()
}

/// `GetRepositoryEndpoint` through the shared harness call (which addresses this service's
/// own published host), reduced to the announced URL.
async fn announced_endpoint(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    format: &str,
) -> String {
    let answer = call(
        router,
        "GET",
        &format!(
            "/v1/repository/endpoint?domain={domain}&repository={repository}&format={format}"
        ),
        None,
    )
    .await;
    announced(&answer, &format!("{domain}/{repository}/{format}"))
}

/// The host suffix an `EndpointType` names: `dualstack` is the one documented alternative,
/// and every other legal value (today only `ipv4`) is the standard regional host.
fn host_suffix(endpoint_type: &str) -> &'static str {
    if endpoint_type == "dualstack" {
        "on.aws"
    } else {
        "amazonaws.com"
    }
}

/// The rule, as one string: the base the emulator is reached at, then AWS's published host
/// for this repository, then the two documented path segments.
fn expected_endpoint(
    base: &str,
    region: &str,
    domain: &str,
    owner: &str,
    format: &str,
    repository: &str,
    endpoint_type: &str,
) -> String {
    format!(
        "{base}/{domain}-{owner}.d.codeartifact.{region}.{}/{format}/{repository}/",
        host_suffix(endpoint_type)
    )
}

/// The `(authority, path)` of an announced URL — what a client splits it into to dial it.
fn dialable(url: &str) -> (String, String) {
    let without_scheme = url
        .split_once("://")
        .map(|(_scheme, rest)| rest)
        .unwrap_or_else(|| panic!("the announced URL {url:?} carries no scheme"));
    match without_scheme.split_once('/') {
        Some((authority, path)) => (authority.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    }
}

/// A domain with one repository in it, and a token for that domain.
async fn given_repository(router: &MultiServiceRouter, domain: &str, repository: &str) -> String {
    assert_eq!(
        create_domain(router, domain, None).await.status,
        200,
        "CreateDomain {domain:?} must succeed"
    );
    assert_eq!(
        create_repository(router, domain, repository, None, &[])
            .await
            .status,
        200,
        "CreateRepository {repository:?} must succeed"
    );
    token_for(router, domain).await
}

/// CASE 1: the announced URL is the base the emulator is reachable at, AWS's published host
/// in the path, and the documented segments — for every base, `EndpointType` and
/// `PackageFormat`.
#[tokio::test]
async fn the_announced_endpoint_is_rooted_on_the_base_the_emulator_is_reached_at() {
    // The rows are the bases `OperationContext::endpoint` reports: the unserved default, a
    // loopback data port, a bracketed IPv6 bind and a named host with a port.
    let bases = [
        UNSERVED_URL_BASE,
        SERVED_BASE,
        "https://[::1]:9297",
        "https://emulator.test.invalid:8443",
    ];
    // `None` is the "no endpointType supplied" row, which must resolve to the documented
    // `ipv4` default.
    let endpoint_types: Vec<Option<&str>> = std::iter::once(None)
        .chain(ENDPOINT_TYPE_VALUES.iter().copied().map(Some))
        .collect();
    let domain = "announced";
    let repository = "Releases.one";

    for base in bases {
        let router = emulator_served_at(base);
        given_repository(&router, domain, repository).await;

        for format in PACKAGE_FORMAT_VALUES {
            for endpoint_type in &endpoint_types {
                let case = format!("base {base} format {format} endpointType {endpoint_type:?}");
                let answer = call(
                    &router,
                    "GET",
                    &format!(
                        "/v1/repository/endpoint?domain={domain}&repository={repository}\
                         &format={format}{}",
                        match endpoint_type {
                            Some(kind) => format!("&endpointType={kind}"),
                            None => String::new(),
                        }
                    ),
                    None,
                )
                .await;
                let url = announced(&answer, &case);
                let expected = expected_endpoint(
                    base,
                    REGION,
                    domain,
                    ACCOUNT,
                    format,
                    repository,
                    endpoint_type.unwrap_or("ipv4"),
                );
                assert_eq!(
                    url, expected,
                    "case {case:?}: expected {expected:?}, got {url:?}"
                );

                // The AUTHORITY is the emulator's own, never the host AWS publishes: a
                // client dials what precedes the first path separator.
                let (authority, path) = dialable(&url);
                assert_eq!(
                    format!("https://{authority}"),
                    base,
                    "case {case:?}: expected the emulator's own authority, got {url:?}"
                );
                assert!(
                    path.ends_with(&format!("/{format}/{repository}/")),
                    "case {case:?}: expected the documented endpoint path, got {path:?}"
                );
            }
        }
    }
}

/// CASE 2: the announced URL leads back to the address the REQUEST reached — which is what
/// a client configured with one local endpoint URL can resolve — and a request that
/// addressed this service's own published host, resolving nothing here, is answered the
/// base the emulator is served at.
#[tokio::test]
async fn the_announced_endpoint_leads_back_to_the_address_the_request_reached() {
    let router = emulator_served_at(SERVED_BASE);
    let domain = "reached";
    let repository = "packages";
    given_repository(&router, domain, repository).await;

    // (case, the host the client addressed, the authority the answer must name)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a client on this machine, by address and port",
            "127.0.0.1:44035",
            "127.0.0.1:44035",
        ),
        ("a client by loopback name", "localhost:9297", "localhost:9297"),
        ("a client by IPv6 literal", "[::1]:9297", "[::1]:9297"),
        (
            "a client that resolves this service's own published host",
            "codeartifact.us-east-1.amazonaws.com",
            "127.0.0.1:9297",
        ),
    ];

    for (case, host, expected_authority) in rows {
        let answer = get_endpoint_at(&router, host, domain, repository, "npm", None).await;
        let url = announced(&answer, case);
        let (authority, path) = dialable(&url);
        assert_eq!(
            authority, *expected_authority,
            "case {case:?}: expected the authority {expected_authority:?}, got {url:?}"
        );
        assert_eq!(
            path,
            format!(
                "/{domain}-{ACCOUNT}.d.codeartifact.{REGION}.amazonaws.com/npm/{repository}/"
            ),
            "case {case:?}: expected the documented endpoint path, got {url:?}"
        );
    }
}

/// CASE 3: an npm client pointed at the announced URL — and at no other address — reaches
/// the repository: it pings, publishes, reads the packument back and fetches the tarball the
/// packument names, and the control plane describes the version that publish stored.
#[tokio::test]
async fn an_npm_client_pointed_at_the_announced_endpoint_reaches_the_repository() {
    let router = emulator_served_at(SERVED_BASE);
    let domain = "npm-announced";
    let repository = "releases";
    let token = given_repository(&router, domain, repository).await;

    let url = announced_endpoint(&router, domain, repository, "npm").await;
    let (host, endpoint) = dialable(&url);

    // The write-authorization route an npm client checks before publishing — at the
    // endpoint announced for EVERY `EndpointType`, since each of them is announced on the
    // address this emulator answers at.
    for endpoint_type in std::iter::once(None).chain(ENDPOINT_TYPE_VALUES.iter().copied().map(Some))
    {
        let case = format!("ping at the endpoint announced for {endpoint_type:?}");
        let announced_for_type = announced(
            &get_endpoint_at(
                &router,
                "127.0.0.1:9297",
                domain,
                repository,
                "npm",
                endpoint_type,
            )
            .await,
            &case,
        );
        let (typed_host, typed_endpoint) = dialable(&announced_for_type);
        let ping = registry(
            &router,
            "GET",
            &typed_host,
            &format!("{typed_endpoint}-/ping?write=true"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            ping.status, 200,
            "case {case:?}: expected 200 at {announced_for_type:?}, got {ping:?}"
        );
    }

    // The rows span what a publish varies: a plain name and a scoped one under its
    // URL-encoded namespace, with one and with several versions.
    type Row = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        &'static [&'static str],
    );
    let rows: [Row; 2] = [
        ("a plain package", "left-pad", None, "left-pad", &["1.0.0"]),
        (
            "a scoped package with two versions",
            "@acme%2fwidget",
            Some("acme"),
            "widget",
            &["2.1.0", "2.2.0"],
        ),
    ];

    for (case, url_path, namespace, name, versions) in rows {
        let display = match namespace {
            Some(scope) => format!("@{scope}/{name}"),
            None => name.to_string(),
        };
        let published = registry(
            &router,
            "PUT",
            &host,
            &format!("{endpoint}{url_path}"),
            Some(&token),
            Some(publish_document(&display, name, versions)),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected the publish at the announced endpoint to answer 200, got \
             {published:?}"
        );

        let packument = registry(
            &router,
            "GET",
            &host,
            &format!("{endpoint}{url_path}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            packument.status, 200,
            "case {case:?}: expected the packument to answer 200, got {packument:?}"
        );
        let document = packument.json();

        for version in versions {
            // The tarball URL the registry publishes is under the address the client
            // reached, so the same client fetches it.
            let tarball_url = document
                .get("versions")
                .and_then(|listed| listed.get(*version))
                .and_then(|entry| entry.get("dist"))
                .and_then(|dist| dist.get("tarball"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (tarball_host, tarball_path) = dialable(&tarball_url);
            assert_eq!(
                tarball_host, host,
                "case {case:?} version {version:?}: expected a tarball URL at the address the \
                 client reached, got {tarball_url:?}"
            );
            let fetched = registry(&router, "GET", &host, &tarball_path, Some(&token), None).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} version {version:?}: expected the tarball at {tarball_path:?} to \
                 answer 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.body,
                tarball_bytes(name, version),
                "case {case:?} version {version:?}: expected the published bytes back"
            );

            // And the control plane holds the version that publish created.
            let mut uri = format!(
                "/v1/package/version?domain={domain}&repository={repository}&format=npm\
                 &package={name}&version={version}"
            );
            if let Some(scope) = namespace {
                uri.push_str(&format!("&namespace={scope}"));
            }
            let described = call(&router, "GET", &uri, None).await;
            assert_eq!(
                described.status, 200,
                "case {case:?} version {version:?}: expected DescribePackageVersion 200, got \
                 {described:?}"
            );
            assert_eq!(
                described.body["packageVersion"]["version"].as_str(),
                Some(*version),
                "case {case:?} version {version:?}: expected the stored version, got \
                 {described:?}"
            );
        }
    }
}

/// CASE 4: a repository name two domains both hold is announced as two DIFFERENT endpoints,
/// and each of them serves only its own domain's repository — the announced address carries
/// the domain, so neither publish can land in the other domain.
#[tokio::test]
async fn each_domain_announces_its_own_endpoint_for_a_shared_repository_name() {
    let router = emulator_served_at(SERVED_BASE);
    let repository = "shared-name";
    let first = given_repository(&router, "ca-first", repository).await;
    let second = given_repository(&router, "ca-second", repository).await;

    let first_url = announced_endpoint(&router, "ca-first", repository, "npm").await;
    let second_url = announced_endpoint(&router, "ca-second", repository, "npm").await;
    assert_ne!(
        first_url, second_url,
        "case \"two domains, one repository name\": expected distinct endpoints, got \
         {first_url:?} twice"
    );

    // (case, the domain, its token, its announced endpoint, the package published there)
    let rows = [
        ("the first domain", "ca-first", &first, &first_url, "alpha"),
        ("the second domain", "ca-second", &second, &second_url, "beta"),
    ];

    for (case, domain, token, url, package) in rows {
        let (host, endpoint) = dialable(url);
        let published = registry(
            &router,
            "PUT",
            &host,
            &format!("{endpoint}{package}"),
            Some(token),
            Some(publish_document(package, package, &["1.0.0"])),
        )
        .await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: expected the publish at its own endpoint to answer 200, got \
             {published:?}"
        );

        // It landed in THAT domain's repository, and in no other.
        for (other_case, other_domain, expected) in [
            ("its own domain", domain, 200),
            (
                "the other domain",
                if domain == "ca-first" {
                    "ca-second"
                } else {
                    "ca-first"
                },
                404,
            ),
        ] {
            let described = call(
                &router,
                "GET",
                &format!(
                    "/v1/package/version?domain={other_domain}&repository={repository}\
                     &format=npm&package={package}&version=1.0.0"
                ),
                None,
            )
            .await;
            assert_eq!(
                described.status, expected,
                "case {case:?} read through {other_case:?}: expected HTTP {expected}, got \
                 {described:?}"
            );
        }
    }
}

/// CASE 5: the other plane this service serves is reached at its own announced URL too —
/// the `pypi` endpoint's Simple Repository API index, which is the address `pip` is
/// configured with.
#[tokio::test]
async fn a_python_client_pointed_at_the_announced_endpoint_reads_the_index() {
    let router = emulator_served_at(SERVED_BASE);
    let domain = "pypi-announced";
    let repository = "wheels";
    let token = given_repository(&router, domain, repository).await;

    let url = announced_endpoint(&router, domain, repository, "pypi").await;
    let (host, endpoint) = dialable(&url);
    assert!(
        endpoint.ends_with(&format!("/pypi/{repository}/")),
        "expected the pypi endpoint path, got {url:?}"
    );

    // PEP 503: the index a client lands on is `<endpoint>simple/`.
    let index = registry(
        &router,
        "GET",
        &host,
        &format!("{endpoint}simple/"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        index.status, 200,
        "case \"the index at the announced endpoint\": expected 200, got {index:?}"
    );

    // Without the token it is a 401 from this plane, not an unrouted request: the address
    // is served, the credential is what is missing.
    let unauthenticated =
        registry(&router, "GET", &host, &format!("{endpoint}simple/"), None, None).await;
    assert_eq!(
        unauthenticated.status, 401,
        "case \"the index with no credential\": expected 401, got {unauthenticated:?}"
    );
}
