// SPDX-License-Identifier: CC0-1.0

//! The repository address this registry publishes, read across the address the EMULATOR
//! ITSELF is served at.
//!
//! The rule under test: **`Repository$repositoryUri` is the endpoint this emulator answers
//! that caller's registry requests at — host and port whole — followed by `/` and the
//! repository's own name.** The member states what the value is for ("The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations"), so a
//! container client splits it on the first `/` and DIALS what is ahead of that `/`: the
//! authority has to be an address a connection opened to it reaches. It is never a name AWS
//! publishes its own endpoints in — the registry host AWS serves
//! (`<registryId>.dkr.ecr.<region>.amazonaws.com`) resolves at the real Amazon ECR, nowhere
//! near this process — and the registry HOLDING the repository is reported BESIDE the
//! address, by `registryId` and `repositoryArn`, off the stored record.
//!
//! What this suite adds to its siblings is the axis they hold fixed: they drive the endpoint a
//! CALL arrived on (`crate::published_registry_address`) and the Region and name a repository
//! was created with (`crate::repository_registry_uri`) against one served address, while the
//! rows here vary the address the emulator is SERVED at — an IPv4 socket, a second port on it,
//! an IPv6 socket, a host of this service an operator serves it at with and without a port,
//! and a host of an operator's own — crossed with the Region whose registry holds the
//! repository (three partitions) and the repository name forms `RepositoryName`'s pattern
//! allows. Every call arrives on this service's own regional endpoint host, which names a
//! service and so is an endpoint no registry client can use: such a caller is published the
//! address the emulator is served at.
//!
//! Each row then does what the value is FOR: it pushes an image and pulls the manifest at the
//! very authority the URI named, with the credential `GetAuthorizationToken` issued, so the
//! address is proved to reach this registry rather than merely to carry the right shape —
//! every served form included, because a `/v2` request presenting a `docker login` credential
//! is this registry's whatever host it arrived on (`crate::service`'s registry claim).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, emulator_served_at, given_layer_in_region, ok_in_region, registry_get, REGISTRY_ID,
};

// The zones AWS publishes its own endpoints in are the sibling suite's list, imported rather
// than re-spelled, so the suites cannot drift on what a published address may not name.
use crate::published_registry_address::AWS_ENDPOINT_DNS_SUFFIXES;

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// One address this emulator is served at: the base URL it is reached by, which is what
/// `smithy_server_runtime::OperationContext::endpoint` reports.
struct Served {
    case: &'static str,
    base: &'static str,
}

/// The addresses the rows serve this emulator at: a loopback socket, a second port on it, an
/// IPv6 socket, a host of THIS service an operator serves it at with and without a port, and
/// a host of an operator's own — the forms an operator reaches a process on this machine by.
const SERVED: &[Served] = &[
    Served {
        case: "an IPv4 loopback socket",
        base: "https://127.0.0.1:9297",
    },
    Served {
        case: "another port on that socket",
        base: "https://127.0.0.1:34567",
    },
    Served {
        case: "an IPv6 loopback socket",
        base: "https://[::1]:9443",
    },
    Served {
        case: "a host of this service an operator serves the emulator at",
        base: "https://ecr.localhost:8443",
    },
    Served {
        case: "that host carrying no port",
        base: "https://ecr.localhost",
    },
    Served {
        case: "a host of an operator's own",
        base: "https://registry.emulator.test:8443",
    },
];

/// The Regions the rows hold repositories in: two commercial Regions and one of a partition
/// with a DNS suffix of its own. The Region keys the stored record and names its ARN, so an
/// address that took the Region from the record shows up here.
const HOLDING_REGIONS: [&str; 3] = ["us-west-2", "eu-west-1", "cn-north-1"];

/// The repository names these rows span, which are the forms `RepositoryName`'s pattern
/// allows — `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$` with
/// `@length(min: 2, max: 256)`:
///
/// ```text
/// smithy_query(query="shape",shape="Repository",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: [&str; 3] = ["ab", "team/app", "a.b_c--d"];

/// The authority a caller reaches this emulator at when it is served at `base`: the base URL
/// without its scheme, host and port together, which is the address every value this emulator
/// announces is built on.
fn served_authority(base: &str) -> &str {
    base.split_once("://")
        .map_or(base, |(_scheme, authority)| authority)
        .trim_end_matches('/')
}

/// Call one operation of this service in `region`, which has to be served: the shared
/// harness's call, addressed to this service's own regional endpoint host — a host that names
/// a SERVICE, so no registry client can use it and the address reported to such a caller is
/// the one this emulator is served at.
async fn call(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    ok_in_region(router, case, region, operation, body).await
}

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// Assert `uri` is the address this emulator, served at `base`, publishes the repository
/// `name` at: the endpoint it answers that caller's registry requests at, followed by the
/// repository's own name, and never a name AWS publishes its own endpoints in.
fn assert_published_at_the_served_address(case: &str, base: &str, uri: &str, name: &str) {
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        authority,
        served_authority(base),
        "case {case:?}: expected the authority of {uri:?} to be the address this emulator is \
         served at, got {authority:?}"
    );
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository name {name:?}, got \
         {path:?}"
    );
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        assert!(
            !host.to_ascii_lowercase().ends_with(suffix),
            "case {case:?}: {uri:?} carries a name under {suffix:?} — a zone AWS publishes its \
             own endpoints in, which this emulator does not answer in"
        );
    }
}

#[tokio::test]
async fn every_operation_publishes_a_repository_at_the_served_address() {
    for served in SERVED {
        for region in HOLDING_REGIONS {
            let router = emulator_served_at(served.base);
            for name in REPOSITORY_NAMES {
                let case = format!("{}, {region}, repository {name}", served.case);
                let expected = format!("{}/{name}", served_authority(served.base));

                let created = call(
                    &router,
                    &case,
                    region,
                    "CreateRepository",
                    &json!({"repositoryName": name}),
                )
                .await;
                let created_uri = text(&case, &created, "/repository/repositoryUri");
                assert_eq!(
                    created_uri, expected,
                    "case {case:?}: expected CreateRepository to report {expected:?}, got \
                     {created_uri:?}"
                );
                assert_published_at_the_served_address(&case, served.base, &created_uri, name);

                // The registry that HOLDS the repository is reported beside the address, off
                // the record: the owning account and the Region.
                let registry_id = text(&case, &created, "/repository/registryId");
                let arn = text(&case, &created, "/repository/repositoryArn");
                assert_eq!(
                    (registry_id.as_str(), arn.as_str()),
                    (
                        REGISTRY_ID,
                        format!("arn:aws:ecr:{region}:{REGISTRY_ID}:repository/{name}").as_str()
                    ),
                    "case {case:?}: expected the owning account and this Region's ARN beside \
                     the address, got {registry_id:?} and {arn:?}"
                );

                // The read and the delete publish the repository at that same address.
                for (operation, body, pointer) in [
                    (
                        "DescribeRepositories",
                        json!({"repositoryNames": [name]}),
                        "/repositories/0/repositoryUri",
                    ),
                    (
                        "DeleteRepository",
                        json!({"repositoryName": name}),
                        "/repository/repositoryUri",
                    ),
                ] {
                    let answered = call(&router, &case, region, operation, &body).await;
                    let reported = text(&case, &answered, pointer);
                    assert_eq!(
                        reported, expected,
                        "case {case:?}: expected {operation} to publish {expected:?}, got \
                         {reported:?}"
                    );
                    assert_published_at_the_served_address(&case, served.base, &reported, name);
                }
            }
        }
    }
}

#[tokio::test]
async fn a_pull_at_the_published_address_reaches_the_image_this_registry_holds() {
    // What the address is FOR: split into the authority and the repository path exactly as a
    // container client splits it, dialled with the credential `GetAuthorizationToken` issued,
    // it is this registry's own Docker Registry v2 plane that answers — wherever the emulator
    // is served, and whichever Region's registry holds the repository.
    for served in SERVED {
        for region in HOLDING_REGIONS {
            let router = emulator_served_at(served.base);
            let name = "pulled/at-the-published-address";
            let case = format!("{}, {region}", served.case);

            let created = call(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let reported = text(&case, &created, "/repository/repositoryUri");
            assert_published_at_the_served_address(&case, served.base, &reported, name);
            let (authority, path) = reported
                .split_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: {reported:?} names no path"));

            // One image in that repository, pushed through the documented sequence, so the
            // pull answers what the push stored.
            let config_bytes = format!("config of {region}").into_bytes();
            let config =
                given_layer_in_region(&router, &case, region, name, &config_bytes).await;
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
            call(
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

            // The `docker login` credential this registry issued that caller, and the URL it
            // is pointed at: a URL dialled as given, so it carries the address this emulator
            // is served at — the very endpoint the repository's own URI is published at.
            let issued = call(&router, &case, region, "GetAuthorizationToken", &json!({})).await;
            let token = text(&case, &issued, "/authorizationData/0/authorizationToken");
            let proxy_endpoint = text(&case, &issued, "/authorizationData/0/proxyEndpoint");
            assert_eq!(
                proxy_endpoint,
                format!("https://{}", served_authority(served.base)),
                "case {case:?}: expected the login URL to name the address this emulator is \
                 served at, got {proxy_endpoint:?}"
            );

            let (status, served_body) = registry_get(
                &router,
                authority,
                &format!("/v2/{path}/manifests/v1"),
                &token,
                Some(DOCKER_V2_MANIFEST),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a pull at {reported:?} must reach this registry, got HTTP \
                 {status} with {served_body:?}"
            );
            assert_eq!(
                served_body, manifest,
                "case {case:?}: expected the manifest pushed in {region:?}, got {served_body:?}"
            );
        }
    }
}
