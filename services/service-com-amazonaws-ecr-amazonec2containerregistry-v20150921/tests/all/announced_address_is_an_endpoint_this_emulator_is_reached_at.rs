// SPDX-License-Identifier: CC0-1.0

//! What the address this registry announces is COMPOSED of — nothing of the CALLER's.
//!
//! RULE under test: **every address this registry hands a caller leads back to one of the two
//! endpoints this emulator is really reached at — the endpoint the CALLER arrived on, or the
//! address this emulator is SERVED at — and nothing of what it carries is taken from the host
//! that caller happened to address.** Both members are read as addresses, and that is what
//! each is composed of:
//!
//! * `AuthorizationData$proxyEndpoint` ("The registry URL to use for this authorization token
//!   in a `docker login` command") is a URL a client DIALS as it was handed it, with no name to
//!   resolve, so its authority IS one of those two endpoints, `host[:port]` whole, and nothing
//!   is composed on top of either.
//! * `Repository$repositoryUri` ("The URI for the repository. You can use this URI for
//!   container image `push` and `pull` operations") is split on the first `/` and the authority
//!   ahead of it DIALLED, so it is that same endpoint, with the repository's own name as the
//!   path.
//!
//! What neither may take anything from is the host the caller addressed. A caller that arrives
//! on another account's registry layout, on another Region's, or on a name in a zone AWS
//! publishes its own endpoints in is published an address of this process instead, and told
//! the account and the Region the RECORD carries beside it: AWS's own spelling resolves at the
//! real Amazon ECR, and an account or a Region echoed from the arrival host would name a
//! registry this emulator does not hold.
//!
//! What this suite adds to its siblings is WHAT each value is made of rather than which of the
//! two endpoints wins. `crate::published_registry_address` varies the endpoint a call arrived
//! on, `crate::repository_uri_is_dialable_wherever_served` the address the emulator is served
//! at, and `crate::repository_uri_is_served` crosses the two — each spelling the expected value
//! out per row. The rows here assert the derivation-free invariant those per-row values are
//! instances of. Each row then does what the value is FOR: a client holding ONLY the announced
//! URI splits it on the first `/`, dials the authority with the credential that same call
//! issued and is served the manifest the repository holds.
//!
//! The rows differ along what the rule generalises over: the address the emulator is served at
//! (a loopback socket, another port on it, an IPv6 socket, a host of this service, a host of an
//! operator's own) crossed with the endpoint the caller arrived on (that socket itself, a
//! loopback name, a host of another service, the registry host AWS publishes, and that registry
//! layout composed in the loopback zone, in an operator's zone, for another account and for
//! another partition's Region) — crossed with the Region whose registry HOLDS the repository
//! and with the repository-name forms `RepositoryName`'s pattern allows, because the Region in
//! the published name is the RECORD's and the name is the path.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, emulator_served_at, given_layer_in_region, registry_get, REGISTRY_ID,
};

// The zones AWS publishes its own endpoints in are a sibling suite's list, imported rather
// than re-spelled, so the suites cannot drift on what a DIALLED address may not name.
use crate::published_registry_address::AWS_ENDPOINT_DNS_SUFFIXES;

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The label run every Amazon ECR registry host carries between the owning account's id and
/// the Region, as the registry URL format `AuthorizationData$proxyEndpoint` documents spells it
/// (`https://aws_account_id.dkr.ecr.region.amazonaws.com`). Every repository address this
/// registry publishes carries it, over a zone of this emulator's own; the URL a `docker login`
/// is pointed at carries none, being an endpoint rather than a name. Spelled out here so a
/// value that lost — or gained — that spelling fails these rows.
const REGISTRY_LABEL_RUN: &str = ".dkr.ecr.";

/// The tag the image of every row is pushed under.
const TAG: &str = "v1";

/// One pairing: where this emulator is SERVED, the endpoint a caller ARRIVED on, and the
/// endpoint this emulator answers that caller's registry requests at — the address both
/// members it announces to that caller are built on.
///
/// Which of the two endpoints that is, is the sibling suites' subject; what is asserted here
/// is that it is ONE OF THEM, and that the repository's own name is the path behind it with
/// nothing taken from the arrival host.
struct Pairing {
    case: &'static str,
    /// The base URL the router is served at, which is what
    /// `smithy_server_runtime::OperationContext::endpoint` reports.
    served_base: &'static str,
    /// The `Host` every call of the row carries.
    reached: &'static str,
    /// The endpoint this emulator answers that caller's registry requests at: the arrival
    /// endpoint where a registry request addressed there reaches this registry, and the served
    /// address where it does not. Spelled out per row rather than derived.
    published: &'static str,
}

/// The pairings the rows drive.
const PAIRINGS: &[Pairing] = &[
    Pairing {
        case: "a client of a local emulator, talking to the socket it is served on",
        served_base: "https://127.0.0.1:9297",
        reached: "127.0.0.1:9297",
        published: "127.0.0.1:9297",
    },
    Pairing {
        case: "a caller on the registry layout composed in the loopback zone",
        served_base: "https://127.0.0.1:36505",
        reached: "000000000000.dkr.ecr.us-west-2.localhost:36505",
        published: "127.0.0.1:36505",
    },
    Pairing {
        case: "a caller on the registry host AWS publishes for this account",
        served_base: "https://127.0.0.1:9297",
        reached: "000000000000.dkr.ecr.eu-west-1.amazonaws.com",
        published: "127.0.0.1:9297",
    },
    Pairing {
        case: "a caller on this service's own endpoint host, served under an operator's name",
        served_base: "https://registry.emulator.test:8443",
        reached: "api.ecr.eu-west-1.amazonaws.com",
        published: "registry.emulator.test:8443",
    },
    Pairing {
        case: "a caller on the loopback name, served on an IPv6 socket",
        served_base: "https://[::1]:9443",
        reached: "localhost:9443",
        published: "localhost:9443",
    },
    Pairing {
        case: "a caller on ANOTHER service's host, served at a host of this service",
        served_base: "https://ecr.localhost:8443",
        reached: "dynamodb.localhost:8443",
        published: "ecr.localhost:8443",
    },
    Pairing {
        case: "a caller on the registry layout composed in an operator's own zone",
        served_base: "https://127.0.0.1:9297",
        reached: "000000000000.dkr.ecr.eu-west-1.registry.emulator.test:9297",
        published: "127.0.0.1:9297",
    },
    Pairing {
        case: "a caller on the registry layout of ANOTHER account",
        served_base: "https://127.0.0.1:9297",
        reached: "111122223333.dkr.ecr.eu-west-1.localhost:9297",
        published: "127.0.0.1:9297",
    },
    Pairing {
        case: "a caller on the registry layout of another partition's Region",
        served_base: "https://127.0.0.1:9297",
        reached: "000000000000.dkr.ecr.cn-north-1.amazonaws.com.cn",
        published: "127.0.0.1:9297",
    },
];

/// The Regions the rows hold repositories in: a commercial Region and one of a partition with a
/// DNS suffix of its own. The Region keys the stored record and names its ARN, and it is the
/// Region label the published URI carries, so a URI built on anything else shows up here.
const HOLDING_REGIONS: [&str; 2] = ["eu-west-1", "cn-north-1"];

/// The repository names the rows span — the shortest the pattern allows and one carrying a
/// namespace path, which is the part of a URI a client must get back unchanged:
/// `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$` with
/// `@length(min: 2, max: 256)`:
///
/// ```text
/// smithy_query(query="shape",shape="Repository",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: [&str; 2] = ["ab", "team/app"];

/// The authority a caller reaches this emulator at when it is served at `base`: the base URL
/// without its scheme, host and port together.
fn served_authority(base: &str) -> &str {
    base.split_once("://")
        .map_or(base, |(_scheme, authority)| authority)
        .trim_end_matches('/')
}

/// Call one operation of this service in `region`, addressed to `host` — the pairing's own
/// arrival endpoint, which this crate's shared harness fixes and this suite varies.
async fn call_at(
    router: &MultiServiceRouter,
    case: &str,
    host: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ecr/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answered: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: {operation} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {answered}"
    );
    answered
}

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// The authority of the URI `uri` the repository `name` of `region`'s registry was published
/// at, asserted to be the endpoint this emulator answers that caller's registry requests at —
/// and to take nothing from the host the caller addressed.
///
/// Three readings, each of them what makes the value usable by whoever was handed it:
///
/// * it is `<authority>/<name>`, so a client that splits it on the first `/` dials the
///   authority and asks for the repository the path names;
/// * the authority is one of the two endpoints this emulator is reached at, whole — never the
///   account or the Region of the host the caller happened to address, which would be a name
///   this process does not answer for;
/// * a client on this machine can reach it: it names the loopback interface this emulator's
///   listeners are bound on
///   (`service_common::data_plane_addressing::names_the_loopback_interface`, the tree's one
///   reading of that) or it is the address this emulator is served at;
/// * it names no zone AWS publishes its own endpoints in, so nothing sends the next push out
///   of the emulated environment carrying the credential it is holding.
fn published_registry_authority(
    pairing: &Pairing,
    case: &str,
    uri: &str,
    region: &str,
    name: &str,
) -> String {
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository name {name:?}, got \
         {path:?}"
    );
    assert_eq!(
        authority, pairing.published,
        "case {case:?}: expected the authority of {uri:?} to be the endpoint this emulator \
         answers this caller's registry requests at — nothing of the host the caller addressed \
         ({:?}), and nothing of the Region {region:?} the record is held in — got {authority:?}",
        pairing.reached
    );
    let served = served_authority(pairing.served_base);
    assert!(
        ::service_common::data_plane_addressing::names_the_loopback_interface(authority)
            || authority == served,
        "case {case:?}: expected an address a client on this machine reaches — the loopback \
         interface this emulator's listeners are bound on, or the address it is served at \
         ({served:?}) — got {authority:?}"
    );
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        assert!(
            !authority.to_ascii_lowercase().contains(suffix),
            "case {case:?}: {authority:?} carries a name under {suffix:?} — a zone AWS publishes \
             its own endpoints in, which this emulator does not answer in"
        );
    }
    authority.to_string()
}

/// The authority of the registry URL `url` a `docker login` of that caller is pointed at,
/// asserted to be one of the two endpoints this emulator is reached at with nothing composed
/// on top of either: it is DIALLED as it was handed over, so it carries a `host[:port]` and no
/// name to resolve.
fn dialled_endpoint(pairing: &Pairing, case: &str, url: &str) -> String {
    let authority = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: expected an https registry URL, got {url:?}"));
    let served = served_authority(pairing.served_base);
    assert!(
        authority == pairing.reached || authority == served,
        "case {case:?}: expected the authority of {url:?} to be the endpoint the caller reached \
         ({:?}) or the address this emulator is served at ({served:?}) — a client dials it as it \
         was handed it, so nothing may be composed on top of either — got {authority:?}",
        pairing.reached
    );
    assert!(
        authority == served
            || ::service_common::data_plane_addressing::names_the_loopback_interface(authority),
        "case {case:?}: expected an address a client on this machine can dial — the served \
         address {served:?} or a name of the loopback interface this emulator's listeners are \
         bound on — got {authority:?}"
    );
    assert!(
        !authority.to_ascii_lowercase().contains(REGISTRY_LABEL_RUN),
        "case {case:?}: expected an endpoint of this emulator, got the registry host spelling \
         {authority:?}, which is a name rather than an address"
    );
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        assert!(
            !authority.to_ascii_lowercase().contains(suffix),
            "case {case:?}: {authority:?} carries a name under {suffix:?} — a zone AWS publishes \
             its own endpoints in, which this emulator does not answer in"
        );
    }
    authority.to_string()
}

// ---------------------------------------------------------------------------
// RULE: the login URL is one of the two endpoints this emulator is reached at, the repository's
// own URI is its registry's name over that endpoint's zone, neither takes anything from the
// host the caller addressed, and a client holding only the published URI is served the
// repository's own image at it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_published_uri_names_the_registry_and_a_login_url_an_endpoint_answered_on() {
    for pairing in PAIRINGS {
        for region in HOLDING_REGIONS {
            let router = emulator_served_at(pairing.served_base);
            for name in REPOSITORY_NAMES {
                let case = format!("{}, {region}, repository {name}", pairing.case);

                let created = call_at(
                    &router,
                    &case,
                    pairing.reached,
                    region,
                    "CreateRepository",
                    &json!({"repositoryName": name}),
                )
                .await;
                let announced = published_registry_authority(
                    pairing,
                    &case,
                    &text(&case, &created, "/repository/repositoryUri"),
                    region,
                    name,
                );

                // The identity the authority carries is the stored record's, and the response
                // reports it beside the address as well — the account and the Region, neither
                // of them the arrival host's.
                let registry_id = text(&case, &created, "/repository/registryId");
                let arn = text(&case, &created, "/repository/repositoryArn");
                assert_eq!(
                    (registry_id.as_str(), arn.as_str()),
                    (
                        REGISTRY_ID,
                        format!("arn:aws:ecr:{region}:{REGISTRY_ID}:repository/{name}").as_str()
                    ),
                    "case {case:?}: expected the owning account and this Region's ARN beside the \
                     address, got {registry_id:?} and {arn:?}"
                );

                // The read a client discovers the push target through reports that same one
                // address.
                let described = call_at(
                    &router,
                    &case,
                    pairing.reached,
                    region,
                    "DescribeRepositories",
                    &json!({"repositoryNames": [name]}),
                )
                .await;
                let read_back = published_registry_authority(
                    pairing,
                    &case,
                    &text(&case, &described, "/repositories/0/repositoryUri"),
                    region,
                    name,
                );
                assert_eq!(
                    read_back, announced,
                    "case {case:?}: expected the read to publish the repository at the address \
                     the create did ({announced:?}), got {read_back:?}"
                );

                // One image in that repository, pushed through the documented sequence, so the
                // pull below answers what the push stored rather than anything this suite made.
                let config_bytes = format!("config of {name} in {region}").into_bytes();
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
                call_at(
                    &router,
                    &case,
                    pairing.reached,
                    region,
                    "PutImage",
                    &json!({
                        "repositoryName": name,
                        "imageManifest": manifest,
                        "imageTag": TAG,
                    }),
                )
                .await;

                // The URL a `docker login` of that same caller is pointed at is one of the two
                // endpoints this emulator is reached at, behind a scheme: a client dials it as
                // given, and this registry's `/v2` plane answers there and at the published
                // name alike.
                let issued = call_at(
                    &router,
                    &case,
                    pairing.reached,
                    region,
                    "GetAuthorizationToken",
                    &json!({}),
                )
                .await;
                let proxy_endpoint = text(&case, &issued, "/authorizationData/0/proxyEndpoint");
                let dialled = dialled_endpoint(pairing, &case, &proxy_endpoint);

                // What the addresses are FOR: a client holding only the published URI dials
                // the authority with the credential this call issued and is served the manifest
                // the repository holds — and so is the endpoint the login URL names.
                let token = text(&case, &issued, "/authorizationData/0/authorizationToken");
                for address in [announced.as_str(), dialled.as_str()] {
                    let (status, served_body) = registry_get(
                        &router,
                        address,
                        &format!("/v2/{name}/manifests/{TAG}"),
                        &token,
                        Some(DOCKER_V2_MANIFEST),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a pull at the announced address {address:?} must reach \
                         this registry, got HTTP {status} with {served_body:?}"
                    );
                    assert_eq!(
                        served_body, manifest,
                        "case {case:?}: expected the manifest the push stored, got \
                         {served_body:?}"
                    );
                }

                // The echo a teardown reads carries that same one address too.
                let removed = call_at(
                    &router,
                    &case,
                    pairing.reached,
                    region,
                    "DeleteRepository",
                    &json!({"repositoryName": name, "force": true}),
                )
                .await;
                let on_delete = published_registry_authority(
                    pairing,
                    &case,
                    &text(&case, &removed, "/repository/repositoryUri"),
                    region,
                    name,
                );
                assert_eq!(
                    on_delete, announced,
                    "case {case:?}: expected the delete to echo the repository at the address it \
                     was published at ({announced:?}), got {on_delete:?}"
                );
            }
        }
    }
}
