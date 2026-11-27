// SPDX-License-Identifier: CC0-1.0

//! The address every Repository-shaped response publishes, across the RESPONSE SHAPES that
//! carry one.
//!
//! The rule under test: **every response that carries a `Repository` publishes
//! `Repository$repositoryUri` as the endpoint this emulator answers that caller's registry
//! requests at — host and port whole — followed by `/` and the repository's own name; the
//! value is composed in ONE place, so the single-repository bodies, every element of a
//! multi-repository page and the echo a delete answers with carry one and the same
//! authority.** The member says what the value is for ("The URI for the repository. You can
//! use this URI for container image `push` and `pull` operations"):
//!
//! ```text
//! smithy_query(query="doc",shape="Repository",member="repositoryUri",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
//! ```
//!
//! so a container client splits it on the first `/` and DIALS what is ahead of it. The
//! authority therefore has to be an address a connection opened to it reaches this process at,
//! which is why it is the endpoint this emulator answers on and never a name the client would
//! first have to resolve: a host under a zone AWS publishes its own endpoints in resolves at
//! genuine Amazon ECR, nowhere near the machine this emulator runs on, and a registry-shaped
//! name of this emulator's own is served by nothing either — the core binds its listeners on
//! the loopback interface and admits only a loopback `Host` there. WHICH registry holds the
//! repository is reported BESIDE the address, by `Repository$registryId` (the owning account,
//! `^[0-9]{12}$`) and the Region `Repository$repositoryArn` names, both off the stored record.
//!
//! What this suite adds to its siblings — which drive the endpoint a call arrived on
//! (`crate::published_registry_address`), the address the emulator is served at
//! (`crate::repository_uri_is_dialable_wherever_served`) and the Region and name a repository
//! was created with (`crate::repository_registry_uri`) — is the SERIALIZATION PATH as the
//! dimension: the rows read the member off the create's `repository`, off a named read's
//! `repositories[0]`, off EVERY element of a page carrying several repositories (the form a
//! client enumerating a registry to build a push map reads, where a one-element page proves
//! nothing about the second entry), off a paged read's second page, and off the delete's
//! echoed `repository` — each crossed with the address this emulator is served at and with
//! the repository-name forms `RepositoryName`'s pattern allows.
//!
//! Nothing here composes the expected authority from a constant of its own: every row compares
//! the published authority against the endpoint the emulator reports to that same caller for a
//! `docker login` (`AuthorizationData$proxyEndpoint`, "The registry URL to use for this
//! authorization token in a `docker login` command"), so the two addresses this service
//! publishes are held against each other. The last test then does what the value is FOR: a
//! client holding only a page's entry splits it, dials the authority with the credential that
//! same registry issued, and is served the manifest the repository holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, emulator_served_at, given_layer, ok, registry_get, REGION, REGISTRY_ID};

/// The DNS suffixes AWS publishes its own endpoints under, spelled out here rather than
/// imported so a published address slipping into one of them fails these rows.
///
/// Source: `service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`, the tree's one
/// list of them.
const AWS_ENDPOINT_DNS_SUFFIXES: [&str; 4] =
    [".amazonaws.com", ".amazonaws.com.cn", ".api.aws", ".on.aws"];

/// How many digits `Repository$registryId` carries, as its own pattern `^[0-9]{12}$` bounds it.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The addresses the rows serve this emulator at — a loopback socket, another port on it, an
/// IPv6 socket and a host an operator serves the process under — because the published
/// authority is that address and nothing of the rows' own.
const SERVED_BASES: [&str; 4] = [
    "https://127.0.0.1:9297",
    "https://127.0.0.1:34567",
    "https://[::1]:9443",
    "https://registry.emulator.test:8443",
];

/// The repository names the rows span, which are the forms `RepositoryName`'s pattern allows
/// — `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$` with
/// `@length(min: 2, max: 256)` — the shortest the pattern admits, a plain name, a namespaced
/// one and one carrying the separators the pattern allows:
///
/// ```text
/// smithy_query(query="shape",shape="RepositoryName",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: [&str; 4] = ["ab", "worker", "team/worker", "a.b_c--d"];

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// The endpoint this emulator answers the caller's registry requests at, as the emulator
/// itself reports it: the authority of `AuthorizationData$proxyEndpoint`, with its scheme
/// removed.
///
/// Read from the service rather than composed here, so every row holds the two addresses this
/// service publishes against each other instead of against a constant of this suite's.
async fn login_endpoint(router: &MultiServiceRouter, case: &str) -> String {
    let issued = ok(router, case, "GetAuthorizationToken", &json!({})).await;
    let url = text(case, &issued, "/authorizationData/0/proxyEndpoint");
    url.split_once("://")
        .map(|(_scheme, authority)| authority.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: expected a registry URL, got {url:?}"))
}

/// The authorization token this registry issues the caller, as a `docker login` is handed it.
async fn login_token(router: &MultiServiceRouter, case: &str) -> String {
    let issued = ok(router, case, "GetAuthorizationToken", &json!({})).await;
    text(case, &issued, "/authorizationData/0/authorizationToken")
}

/// Assert that the `Repository` at `pointer` in `body` publishes `name` at `published` — the
/// endpoint this emulator answers that caller at — with the repository's own name as the path,
/// and the registry holding it reported beside the address.
///
/// Answers the authority it was published at, so a caller can compare two serialization paths
/// against each other as well as against the endpoint.
fn assert_published(
    case: &str,
    body: &Value,
    pointer: &str,
    name: &str,
    published: &str,
) -> String {
    let uri = text(case, body, &format!("{pointer}/repositoryUri"));
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository's own name \
         {name:?}, got {path:?}"
    );
    assert_eq!(
        authority, published,
        "case {case:?}: expected {uri:?} to be published at the endpoint this emulator answers \
         registry requests at, {published:?}, got {authority:?}"
    );
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    assert!(
        !AWS_ENDPOINT_DNS_SUFFIXES
            .iter()
            .any(|suffix| host.to_ascii_lowercase().ends_with(suffix)),
        "case {case:?}: expected an address this emulator answers on, got {authority:?}, which \
         AWS publishes its own endpoints in"
    );

    // The registry that HOLDS the repository, reported beside the address off the same record.
    let registry_id = text(case, body, &format!("{pointer}/registryId"));
    assert!(
        registry_id.len() == ACCOUNT_ID_DIGITS
            && registry_id.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a {ACCOUNT_ID_DIGITS}-digit account id beside {uri:?}, got \
         {registry_id:?}"
    );
    let arn = text(case, body, &format!("{pointer}/repositoryArn"));
    let expected_arn = format!("arn:aws:ecr:{REGION}:{registry_id}:repository/{name}");
    assert_eq!(
        arn, expected_arn,
        "case {case:?}: expected the ARN of the repository this address names, {expected_arn:?}, \
         got {arn:?}"
    );
    authority.to_string()
}

/// RULE: every response shape that carries a `Repository` publishes it at the endpoint this
/// emulator answers the caller's registry requests at — the create, a named read, EVERY
/// element of a page carrying several repositories, the next page of a paged read, and the
/// delete's echo — all at one authority, whatever address the emulator is served at.
#[tokio::test]
async fn every_repository_shaped_response_publishes_the_endpoint_this_emulator_answers_at() {
    for served_base in SERVED_BASES {
        let router = emulator_served_at(served_base);
        let case = format!("served at {served_base}");
        let published = login_endpoint(&router, &case).await;

        // One repository per name, so the page below carries several of them at once.
        let mut created_authorities = Vec::new();
        for name in REPOSITORY_NAMES {
            let case = format!("{case}, repository {name}");
            let created = ok(
                &router,
                &case,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            created_authorities.push(assert_published(
                &case,
                &created,
                "/repository",
                name,
                &published,
            ));

            // The read a client discovers one repository's push target through.
            let named = ok(
                &router,
                &case,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_published(&case, &named, "/repositories/0", name, &published);
        }

        // The page a client enumerating the registry reads: EVERY element carries the
        // published authority, not only the first.
        let page = ok(&router, &case, "DescribeRepositories", &json!({})).await;
        let entries = at(&page, "/repositories");
        let entries = entries
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a repository list, got {page}"));
        assert_eq!(
            entries.len(),
            REPOSITORY_NAMES.len(),
            "case {case:?}: expected every created repository in the page, got {page}"
        );
        for index in 0..entries.len() {
            let pointer = format!("/repositories/{index}");
            let name = text(&case, &page, &format!("{pointer}/repositoryName"));
            let entry = format!("{case}, page entry {index}");
            assert_published(&entry, &page, &pointer, &name, &published);
        }

        // The same read PAGED, so the entries behind a token are published alike: one
        // repository per page, walked to the end.
        let mut token = Value::Null;
        let mut walked = 0;
        loop {
            let mut request = json!({"maxResults": 1});
            if let Some(next) = token.as_str() {
                request["nextToken"] = json!(next);
            }
            let answered = ok(&router, &case, "DescribeRepositories", &request).await;
            let in_page = at(&answered, "/repositories");
            let in_page = in_page.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected a repository list, got {answered}")
            });
            for index in 0..in_page.len() {
                let pointer = format!("/repositories/{index}");
                let name = text(&case, &answered, &format!("{pointer}/repositoryName"));
                let entry = format!("{case}, page {walked} entry {index}");
                assert_published(&entry, &answered, &pointer, &name, &published);
                walked += 1;
            }
            token = at(&answered, "/nextToken");
            if token.as_str().is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            REPOSITORY_NAMES.len(),
            "case {case:?}: expected the paged walk to report every repository, got {walked}"
        );

        // The echo a teardown reads carries that same one authority.
        for (name, created) in REPOSITORY_NAMES.iter().zip(&created_authorities) {
            let case = format!("{case}, deleting {name}");
            let deleted = ok(
                &router,
                &case,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let echoed = assert_published(&case, &deleted, "/repository", name, &published);
            assert_eq!(
                &echoed, created,
                "case {case:?}: expected the delete to echo the authority the create published, \
                 {created:?}, got {echoed:?}"
            );
        }
    }
}

/// RULE: the address a page's entry publishes is one this registry's own Docker Registry v2
/// plane answers — so a client that enumerated the registry, split an entry's URI and dialled
/// the authority with the credential `GetAuthorizationToken` issued is served the manifest
/// that repository holds.
#[tokio::test]
async fn a_pull_at_an_enumerated_entrys_address_serves_that_repositorys_manifest() {
    for served_base in SERVED_BASES {
        let router = emulator_served_at(served_base);
        let case = format!("served at {served_base}");

        // Two repositories, each holding a manifest of its own, so an entry that published
        // another entry's address would answer the wrong document rather than nothing.
        let mut pushed = Vec::new();
        for name in ["pulled/first", "pulled/second"] {
            let case = format!("{case}, repository {name}");
            ok(
                &router,
                &case,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let config_bytes = format!("config of {name}").into_bytes();
            let config = given_layer(&router, &case, name, &config_bytes).await;
            let manifest = json!({
                "schemaVersion": 2,
                "mediaType": DOCKER_V2_MANIFEST,
                "config": {
                    "mediaType": DOCKER_CONFIG_MEDIA_TYPE,
                    "digest": config,
                    "size": config_bytes.len(),
                },
                "layers": [],
            })
            .to_string();
            ok(
                &router,
                &case,
                "PutImage",
                &json!({
                    "repositoryName": name,
                    "imageManifest": manifest,
                    "imageTag": "v1",
                }),
            )
            .await;
            pushed.push((name, manifest));
        }

        let page = ok(&router, &case, "DescribeRepositories", &json!({})).await;
        let token = login_token(&router, &case).await;
        for (index, (name, manifest)) in pushed.iter().enumerate() {
            let entry = format!("{case}, entry {index}");
            let uri = text(
                &entry,
                &page,
                &format!("/repositories/{index}/repositoryUri"),
            );
            let reported = text(
                &entry,
                &page,
                &format!("/repositories/{index}/repositoryName"),
            );
            assert_eq!(
                &reported, name,
                "case {entry:?}: expected the entries in creation order, got {page}"
            );
            let (authority, path) = uri
                .split_once('/')
                .unwrap_or_else(|| panic!("case {entry:?}: {uri:?} names no repository path"));
            let (status, served) = registry_get(
                &router,
                authority,
                &format!("/v2/{path}/manifests/v1"),
                &token,
                Some(DOCKER_V2_MANIFEST),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {entry:?}: a pull at the published address {authority:?} must reach this \
                 registry, got HTTP {status} with {served:?}"
            );
            assert_eq!(
                &served, manifest,
                "case {entry:?}: expected the manifest {name:?} holds, got {served:?}"
            );
        }

        // And the account the entries are published for is this registry's own, so a client
        // composing an `ecr:` ARN from the page agrees with the ARNs it carries.
        let registry_id = text(&case, &page, "/repositories/0/registryId");
        assert_eq!(
            registry_id, REGISTRY_ID,
            "case {case:?}: expected this account's registry id beside the addresses, got \
             {registry_id:?}"
        );
    }
}
