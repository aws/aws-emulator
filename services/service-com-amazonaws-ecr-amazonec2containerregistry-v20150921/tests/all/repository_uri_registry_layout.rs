// SPDX-License-Identifier: CC0-1.0

//! The LAYOUT of the URI a repository is reported at, read off the response that carries it.
//!
//! The rule under test: **`Repository$repositoryUri` is the endpoint this emulator answers
//! that caller's registry requests at — host and port whole — followed by the repository's
//! name as its only path component.** The member says what the value is for ("The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations"), so a
//! client splits it on the first `/`, DIALS the authority and asks for the repository the path
//! names: the authority has to be an address a connection opened to it reaches, which is why
//! it is an endpoint of this process rather than a name the client would first have to
//! resolve. It is the same endpoint the URL a `docker login` is pointed at is built on
//! (`AuthorizationData$proxyEndpoint`), so the two addresses this service publishes cannot
//! disagree.
//!
//! The fixed part of the value is therefore that pairing — one published endpoint, the
//! repository's own name as the path — while the address itself, the account id and the Region
//! come from state and the request. No name in a zone AWS publishes its own endpoints in is
//! ever reported
//! (`service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`): such a name resolves
//! at genuine Amazon ECR rather than at the machine this emulator runs on, so a client handed
//! one would leave the emulated environment carrying the credential its `docker login` was
//! issued.
//!
//! WHICH registry holds the repository is reported BESIDE the address, on the same response:
//! `registryId` is the owning account and `repositoryArn` names the Region. That is what a
//! consumer deriving a task definition's `image`, a function's `ImageUri` or a push target
//! reads, and what a check that the image belongs to this account is made on.
//!
//! Nothing here composes the expected value from a constant: every assertion compares the
//! published authority against the endpoint the SAME emulator reports for a `docker login` of
//! the same caller, and the identity members against the record the same response carries, so
//! a value that took its address, its identity or its path from anywhere else fails these
//! rows. The class the rule generalises over is the registry the repository is held in crossed
//! with the repository's name, so the rows span Regions of four partitions — none of them the
//! Region this crate's harness signs for by default — crossed with the name forms
//! `RepositoryName`'s pattern allows, through every operation that reports a `Repository`.
//!
//! One invariant closes it: a container client that follows the value — dialling the authority
//! it was handed with the credential `GetAuthorizationToken` issued and asking for the path —
//! reaches the image that repository holds.

use serde_json::{json, Value};

use crate::wire::{at, emulator, given_layer_in_region, ok_in_region, registry_get};

/// The DNS suffixes AWS publishes its own endpoints under, spelled out here rather than
/// imported so a published address slipping into one of them fails these rows.
///
/// Source: `service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`, the tree's one
/// list of them.
const AWS_ENDPOINT_DNS_SUFFIXES: [&str; 4] =
    [".amazonaws.com", ".amazonaws.com.cn", ".api.aws", ".on.aws"];

/// How many digits an account id — and so `Repository$registryId` — carries, as its pattern
/// `^[0-9]{12}$` states.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The Regions the rows hold repositories in: one of four partitions, so a value that took
/// the Region from anywhere but the record shows up here. None is the Region this crate's
/// harness signs for by default.
const HOLDING_REGIONS: [&str; 4] = ["us-west-2", "ap-southeast-2", "us-gov-east-1", "cn-north-1"];

/// The repository names the rows span, which are the forms `RepositoryName`'s pattern allows
/// — `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$` with
/// `@length(min: 2, max: 256)` — the shortest the pattern admits, a plain name, a namespaced
/// one, a twice-namespaced one, and one carrying the separators the pattern allows:
///
/// ```text
/// smithy_query(query="shape",shape="RepositoryName",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: [&str; 5] = ["ab", "app", "team/app", "org/team/app", "a.b_c--d"];

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// The Region an ECR repository ARN names: its fourth colon-separated field,
/// `arn:<partition>:ecr:<region>:<account>:repository/<name>`.
fn arn_region(case: &str, arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} names no Region"))
        .to_string()
}

/// The endpoint a `docker login` of a caller in `region` is pointed at, read off
/// `AuthorizationData$proxyEndpoint` ("The registry URL to use for this authorization token in
/// a `docker login` command") with its scheme removed — the address this emulator answers that
/// caller's registry requests at, as the emulator itself reports it.
///
/// Read from the service rather than composed here, so the rows compare the two addresses this
/// service publishes against each other instead of against a constant of this suite's.
async fn login_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    region: &str,
) -> String {
    let issued = ok_in_region(router, case, region, "GetAuthorizationToken", &json!({})).await;
    let url = text(case, &issued, "/authorizationData/0/proxyEndpoint");
    url.split_once("://")
        .map(|(_scheme, authority)| authority.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: expected a registry URL, got {url:?}"))
}

/// Assert that the `Repository` at `pointer` in `body` reports `name` at the layout above, and
/// answer the authority it was published at.
///
/// `published_endpoint` is the endpoint the emulator itself reports for a `docker login` of
/// the same caller, and the identity is read off the SAME response, so nothing of the expected
/// value is composed here.
fn assert_published_layout(
    case: &str,
    body: &Value,
    pointer: &str,
    name: &str,
    published_endpoint: &str,
) -> String {
    let uri = text(case, body, &format!("{pointer}/repositoryUri"));
    let registry_id = text(case, body, &format!("{pointer}/registryId"));
    let region = arn_region(case, &text(case, body, &format!("{pointer}/repositoryArn")));

    // A client reads the value by splitting it on the FIRST `/`: what is ahead of it is the
    // address it dials, and what follows is the repository it asks for.
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository name {name:?}, got \
         {path:?}"
    );
    // The authority is the endpoint this emulator answers that caller's registry requests at,
    // which is the one the login URL of the same caller carries — host and port together, so
    // a client that resolves nothing can open a connection to it.
    assert_eq!(
        authority, published_endpoint,
        "case {case:?}: expected {uri:?} to be published at the endpoint this emulator answers \
         registry requests at, {published_endpoint:?}, got {authority:?}"
    );
    // And never a name in a zone AWS publishes its own endpoints in, which resolves at
    // genuine Amazon ECR rather than near this process.
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    assert!(
        !AWS_ENDPOINT_DNS_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix)),
        "case {case:?}: expected an address this emulator answers on, got {authority:?}, which \
         AWS publishes its own endpoints in"
    );

    // WHICH registry holds the repository is reported BESIDE the address, off the same
    // record: the owning account, and the Region the ARN names.
    assert!(
        registry_id.len() == ACCOUNT_ID_DIGITS
            && registry_id.chars().all(|digit| digit.is_ascii_digit()),
        "case {case:?}: expected a {ACCOUNT_ID_DIGITS}-digit account id beside {uri:?}, got \
         {registry_id:?}"
    );
    assert!(
        !region.is_empty(),
        "case {case:?}: expected the Region of the registry holding {name:?} beside {uri:?}, \
         got {region:?}"
    );

    authority.to_string()
}

/// RULE: every operation that reports a `Repository` publishes it at the endpoint this
/// emulator answers the caller's registry requests at, with the repository's name as the path.
#[tokio::test]
async fn every_reported_repository_uri_carries_the_published_endpoint_layout() {
    for region in HOLDING_REGIONS {
        let router = emulator();
        let endpoint = login_endpoint(&router, region, region).await;
        for name in REPOSITORY_NAMES {
            let case = format!("{region}, repository {name}");

            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let published = assert_published_layout(&case, &created, "/repository", name, &endpoint);

            // The read a client discovers the push target through — both the form that names
            // the repositories and the one that enumerates the registry's own.
            let described = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            let read =
                assert_published_layout(&case, &described, "/repositories/0", name, &endpoint);
            assert_eq!(
                read, published,
                "case {case:?}: expected the read to publish the authority the create did, \
                 got {read:?}"
            );

            let page = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name, name]}),
            )
            .await;
            // A name listed twice is answered twice, and each entry carries the same address.
            for index in 0..2 {
                let entry = assert_published_layout(
                    &case,
                    &page,
                    &format!("/repositories/{index}"),
                    name,
                    &endpoint,
                );
                assert_eq!(
                    entry, published,
                    "case {case:?}: expected entry {index} of the projected read to publish \
                     {published:?}, got {entry:?}"
                );
            }

            let deleted = ok_in_region(
                &router,
                &case,
                region,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let echoed = assert_published_layout(&case, &deleted, "/repository", name, &endpoint);
            assert_eq!(
                echoed, published,
                "case {case:?}: expected the delete to echo the authority the create \
                 published, got {echoed:?}"
            );
        }
    }
}

/// RULE: the authority a repository is published at is one this registry's own Docker
/// Registry v2 plane answers — so a client that splits the value it was handed, dials that
/// authority with the credential `GetAuthorizationToken` issued and asks for the path reaches
/// the image the repository holds.
#[tokio::test]
async fn a_pull_at_the_published_registry_authority_reaches_the_image() {
    for region in HOLDING_REGIONS {
        let router = emulator();
        let name = "followed/by-a-client";
        let case = format!("{region}, repository {name}");
        let endpoint = login_endpoint(&router, &case, region).await;

        let created = ok_in_region(
            &router,
            &case,
            region,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        let authority = assert_published_layout(&case, &created, "/repository", name, &endpoint);

        // One image, pushed through the documented sequence, so the pull answers what the
        // push stored rather than anything this test composed.
        let config_bytes = format!("config of {region}").into_bytes();
        let config = given_layer_in_region(&router, &case, region, name, &config_bytes).await;
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
        ok_in_region(
            &router,
            &case,
            region,
            "PutImage",
            &json!({
                "repositoryName": name,
                "imageManifest": manifest,
                "imageTag": "v1",
            }),
        )
        .await;

        // `docker login` presents the issued token as a Basic credential, which is what the
        // token decodes to (`AWS:<password>`).
        let issued =
            ok_in_region(&router, &case, region, "GetAuthorizationToken", &json!({})).await;
        let token = text(&case, &issued, "/authorizationData/0/authorizationToken");

        let (status, served) = registry_get(
            &router,
            &authority,
            &format!("/v2/{name}/manifests/v1"),
            &token,
            Some(DOCKER_V2_MANIFEST),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pull at the published authority {authority:?} must reach this \
             registry, got HTTP {status} with {served:?}"
        );
        assert_eq!(
            served, manifest,
            "case {case:?}: expected the pushed manifest from {authority:?}, got {served:?}"
        );
    }
}
