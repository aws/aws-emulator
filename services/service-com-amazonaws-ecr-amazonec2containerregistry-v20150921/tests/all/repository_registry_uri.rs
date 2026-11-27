// SPDX-License-Identifier: CC0-1.0

//! The URI Amazon ECR reports for a repository, end to end over the wire.
//!
//! The rule under test: **`Repository$repositoryUri` is the ADDRESS a client reaches the
//! repository at — the endpoint this emulator answers that caller's registry requests at,
//! followed by `/` and the repository's own name.** The member says what the value is for
//! ("The URI for the repository. You can use this URI for container image `push` and `pull`
//! operations"), and a container client reads it by splitting it on the first `/`: it DIALS
//! the authority ahead of that `/` and asks for the repository the path names.
//!
//! So the authority is an address a connection opened to it reaches, port included, rather
//! than a name the client would first have to resolve. It is never a name AWS publishes its
//! own endpoints in: the registry host AWS serves
//! (`<registryId>.dkr.ecr.<region>.amazonaws.com`, the registry URL form the model states in
//! `AuthorizationData$proxyEndpoint`) resolves at the real Amazon ECR rather than at the
//! machine this emulator runs on, so a client handed it leaves the emulated environment
//! entirely, carrying the credential its `docker login` was issued. The endpoint this
//! emulator answers on is the one address a client on this machine can open a connection to,
//! and this registry's `/v2` plane answers there whatever host a request arrived on.
//!
//! WHICH registry HOLDS the repository is what the very same response reports beside the
//! address — `Repository$registryId` is the owning account and `Repository$repositoryArn`
//! names the Region — off the stored record, so one repository name taken in two Regions is
//! still two repositories, with two ARNs, each reached at the one published endpoint with the
//! credential its Region's `docker login` was issued.
//!
//! The class the rule generalises over is the Region the repository was created in crossed
//! with the repository name: the rows below span Regions of five partitions crossed with the
//! name forms `RepositoryName`'s pattern allows, read back through every operation that
//! reports a `Repository`. What the endpoint the CALL arrived on does to the value — nothing —
//! is driven in `crate::published_registry_address`; every call here arrives on this crate's
//! one harness endpoint.
//!
//! The tests that follow are the invariants that make the value more than a well-shaped
//! string: a container client that splits the reported URI and pulls at it reaches the image
//! the repository holds; two repositories of one name in two Regions are told apart by the
//! identity each reports, and a pull with each Region's own credential reaches that Region's
//! own image; and the name-collision walk — a create refused in the Region that holds the
//! name and accepted in another — leaves both readable.
//!
//! The last section reads the same rule on a repository no `CreateRepository` call ever
//! named: the mirrored one the registry's own import creates when a reference under a pull
//! through cache rule's prefix is first read ("a pull through cache rule provides a way to
//! cache images from an upstream registry source in your Amazon ECR private registry",
//! `CreatePullThroughCacheRule`). The URI is composed off the repository's record, so the
//! path that MINTED the repository cannot change it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, call_in_region, emulator, given_layer_in_region, ok_in_region,
    published_registry_authority, published_repository_uri, registry_get, registry_host_in,
    REGISTRY_ID,
};

// The zones AWS publishes its own endpoints in are the sibling suite's list, imported rather
// than re-spelled, so the two suites cannot drift on what a published address may not name.
use crate::published_registry_address::AWS_ENDPOINT_DNS_SUFFIXES;

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// One Region a repository can be created in.
///
/// Source: the Regions the `dnsSuffix` of five partitions in the AWS SDK partition metadata
/// (`endpoints.json`) covers — `aws`, `aws-us-gov`, `aws-cn`, `aws-iso` and `aws-eusc`. The
/// rows are deliberately not only the commercial partition, and none of them is the Region
/// the recorded case used: the Region keys the stored record and names its ARN, so a value
/// that quietly took it from one constant would pass a one-Region table.
const REGISTRY_REGIONS: &[&str] = &[
    "us-west-2",
    "ap-southeast-2",
    "us-gov-east-1",
    "cn-north-1",
    "us-iso-east-1",
    "eusc-de-east-1",
];

/// The repository names these cases span, which are the forms `RepositoryName`'s pattern
/// allows — `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`
/// with `@length(min: 2, max: 256)`:
///
/// ```text
/// smithy_query(query="shape",shape="Repository",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const REPOSITORY_NAMES: &[&str] = &["app", "ab", "team/app", "org/team/app", "a.b_c--d"];

/// The `repositoryUri` of the `Repository` at `pointer` in `body`, or a panic naming the
/// case and the body that carried none.
fn reported_uri(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositoryUri at {pointer}, got {body}"))
        .to_string()
}

/// Assert `uri` is the address this emulator publishes the repository `name` of `region`'s
/// registry at, with the repository's name as its path: the endpoint this emulator answers
/// the caller's registry requests at — and never the host AWS serves that Region's registry
/// at, because no listener here answers for a name AWS serves.
fn assert_published_address(case: &str, uri: &str, region: &str, name: &str) {
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        authority,
        published_registry_authority(),
        "case {case:?}: expected the host of {uri:?} to be the endpoint this emulator answers \
         registry requests at, got {authority:?}"
    );
    assert_eq!(
        path, name,
        "case {case:?}: expected the path of {uri:?} to be the repository name {name:?}, got \
         {path:?}"
    );
    assert_ne!(
        authority,
        registry_host_in(region),
        "case {case:?}: {uri:?} names the host AWS publishes for this account's registry in \
         {region:?}, which resolves nowhere near this process"
    );
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        assert!(
            !authority.to_ascii_lowercase().ends_with(suffix),
            "case {case:?}: {uri:?} carries a name under {suffix:?} — a zone AWS publishes \
             its own endpoints in, which this emulator does not answer in"
        );
    }
}

#[tokio::test]
async fn every_operation_reports_the_address_the_repository_is_published_at() {
    let router = emulator();
    for region in REGISTRY_REGIONS {
        for name in REPOSITORY_NAMES {
            let case = format!("{region}, repository {name}");
            let expected = published_repository_uri(name);

            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let created_uri = reported_uri(&case, &created, "/repository/repositoryUri");
            assert_eq!(
                created_uri, expected,
                "case {case:?}: expected CreateRepository to report {expected:?}, got \
                 {created_uri:?}"
            );

            // The two parts a reader splits the value into: the address it dials, and the
            // path after the first '/', which is exactly the repository's name.
            assert_published_address(&case, &created_uri, region, name);

            // What a client reads instead of parsing the address: the owning account and the
            // Region, on the very response that carries the URI.
            let registry_id = at(&created, "/repository/registryId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                registry_id, REGISTRY_ID,
                "case {case:?}: expected the registryId of this account, got {registry_id:?}"
            );
            let arn = at(&created, "/repository/repositoryArn")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.starts_with("arn:") && arn.contains(&format!(":{region}:")),
                "case {case:?}: expected the ARN to name the Region {region:?}, got {arn:?}"
            );

            let described = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            let described_uri =
                reported_uri(&case, &described, "/repositories/0/repositoryUri");
            assert_eq!(
                described_uri, expected,
                "case {case:?}: expected DescribeRepositories to report {expected:?}, got \
                 {described_uri:?}"
            );

            let deleted = ok_in_region(
                &router,
                &case,
                region,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let deleted_uri = reported_uri(&case, &deleted, "/repository/repositoryUri");
            assert_eq!(
                deleted_uri, expected,
                "case {case:?}: expected DeleteRepository to report {expected:?}, got \
                 {deleted_uri:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_client_recovers_the_registry_the_account_and_the_region_beside_the_reported_uri() {
    // What a reader DOES with the value, as the ECR-access walk does it: split the URI on the
    // first '/', read the account and the Region off the authority's labels, and ask for the
    // path. The account and the Region are also what the response reports BESIDE it —
    // `registryId` and `repositoryArn` — so a client that composes the repository's own
    // `ecr:` ARN out of either agrees with the ARN the service reports for itself.
    let router = emulator();
    for region in REGISTRY_REGIONS {
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
            let described = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;

            for (what, body, pointer) in [
                ("the create", &created, "/repository"),
                ("the read", &described, "/repositories/0"),
            ] {
                let case = format!("{case}, {what}");
                let uri = reported_uri(&case, body, &format!("{pointer}/repositoryUri"));
                assert_published_address(&case, &uri, region, name);

                // What a client builds out of what the response reports: the repository's own
                // ARN, which that same response carries for itself.
                let reported_registry = at(body, &format!("{pointer}/registryId"))
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    reported_registry, REGISTRY_ID,
                    "case {case:?}: expected the owning account beside {uri:?}, got \
                     {reported_registry:?}"
                );
                let reported_arn = at(body, &format!("{pointer}/repositoryArn"))
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let built =
                    format!("arn:aws:ecr:{region}:{reported_registry}:repository/{name}");
                assert_eq!(
                    reported_arn, built,
                    "case {case:?}: expected the ARN a client builds from the account and the \
                     Region reported beside {uri:?} ({built:?}), got {reported_arn:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn one_name_in_two_registries_is_told_apart_by_its_identity() {
    // One name in several Regions is several repositories, and what tells them apart is the
    // identity each reports — its own Region's ARN — beside the one address every registry
    // this emulator holds is published at.
    let router = emulator();
    let name = "shared-name";
    let mut arns: Vec<String> = Vec::new();
    let mut uris: Vec<String> = Vec::new();
    for region in REGISTRY_REGIONS {
        let case = format!("{region}, repository {name}");
        ok_in_region(
            &router,
            &case,
            region,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        let described = ok_in_region(
            &router,
            &case,
            region,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;
        let uri = reported_uri(&case, &described, "/repositories/0/repositoryUri");
        let expected = published_repository_uri(name);
        assert_eq!(
            uri, expected,
            "case {case:?}: expected {expected:?}, got {uri:?}"
        );
        assert_published_address(&case, &uri, region, name);
        if let Some(first) = uris.first() {
            assert_eq!(
                &uri, first,
                "case {case:?}: expected every registry to publish the one endpoint this \
                 emulator answers registry requests at, {first:?}, got {uri:?}"
            );
        }
        uris.push(uri);
        let arn = at(&described, "/repositories/0/repositoryArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {case:?}: expected the ARN to name {region:?}, got {arn:?}"
        );
        assert!(
            !arns.contains(&arn),
            "case {case:?}: expected an ARN no other Region's registry reports, but {arn:?} \
             was already reported for {arns:?}"
        );
        arns.push(arn);
    }
}

#[tokio::test]
async fn a_pull_at_the_reported_uri_reaches_the_repository_it_names() {
    // The invariant the URI is for: the reported value round-trips back into this registry.
    // Each row pushes one image, then splits the URI into the address and the repository
    // path exactly as a container client does and pulls the manifest there with the
    // credential `GetAuthorizationToken` issued for that Region's registry.
    for region in REGISTRY_REGIONS {
        let router = emulator();
        let name = "pulled/by-a-client";
        let case = format!("{region}, repository {name}");

        let created = ok_in_region(
            &router,
            &case,
            region,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        let reported = reported_uri(&case, &created, "/repository/repositoryUri");
        assert_eq!(
            reported,
            published_repository_uri(name),
            "case {case:?}: expected the address this repository is published at, got \
             {reported:?}"
        );
        let (reported_host, reported_path) = reported
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: {reported:?} names no repository path"));

        let config_bytes = b"reported-config".to_vec();
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

        // `docker login` presents the issued token as a Basic credential, which is what
        // the token decodes to (`AWS:<password>`).
        let issued =
            ok_in_region(&router, &case, region, "GetAuthorizationToken", &json!({})).await;
        let token = at(&issued, "/authorizationData/0/authorizationToken")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an authorizationToken, got {issued}"))
            .to_string();
        // The login URL is the endpoint this caller reached, behind a scheme: it is dialled as
        // it is handed over. The repository's own URI is that same endpoint with the
        // repository's name as its path, and this registry's `/v2` plane answers at either,
        // which is what this pair says.
        let proxy_endpoint = at(&issued, "/authorizationData/0/proxyEndpoint")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            proxy_endpoint,
            crate::wire::announced_registry_url(),
            "case {case:?}: expected the login URL to name the endpoint this emulator \
             answers this caller at, got {proxy_endpoint:?}"
        );
        assert_eq!(
            proxy_endpoint,
            format!("https://{reported_host}"),
            "case {case:?}: expected the repository to be published at the endpoint the login \
             URL names, got {reported:?} beside {proxy_endpoint:?}"
        );

        let pulled = pull_manifest(&router, reported_host, reported_path, &token).await;
        assert_eq!(
            pulled.0, 200,
            "case {case:?}: a pull at the reported URI {reported:?} must reach the \
             repository it names, got HTTP {} with {:?}",
            pulled.0, pulled.1
        );
        assert_eq!(
            pulled.1, manifest,
            "case {case:?}: expected the pushed manifest from the reported URI, got {:?}",
            pulled.1
        );
    }
}

/// The error code a create for a name the addressed registry already holds is refused with
/// (`CreateRepository` models `RepositoryAlreadyExistsException`).
const ALREADY_EXISTS: &str = "RepositoryAlreadyExistsException";

/// One pair of Regions and the repository name taken in both of them, for the
/// name-collision walk below.
struct RegionPair {
    case: &'static str,
    /// The Region the name is taken in first.
    home: &'static str,
    /// A second Region, whose registry the same name is free in.
    other: &'static str,
    /// The repository name, in one of the forms [`REPOSITORY_NAMES`] is read from.
    name: &'static str,
}

/// The pairs the collision walk drives: two commercial Regions, two Regions of different
/// partitions, and a GovCloud Region beside a commercial one, each with a different name
/// form — the Region pair and the name are what the rule generalises over.
const REGION_PAIRS: &[RegionPair] = &[
    RegionPair {
        case: "two commercial Regions",
        home: "us-east-1",
        other: "us-west-2",
        name: "shared",
    },
    RegionPair {
        case: "two commercial Regions, a namespaced name",
        home: "eu-west-1",
        other: "ap-southeast-2",
        name: "team/app",
    },
    RegionPair {
        case: "two Regions of different partitions",
        home: "us-west-2",
        other: "cn-north-1",
        name: "ab",
    },
    RegionPair {
        case: "a GovCloud Region beside a commercial one",
        home: "us-gov-east-1",
        other: "us-east-1",
        name: "a.b_c--d",
    },
];

#[tokio::test]
async fn a_name_one_registry_holds_is_free_in_another_and_both_stay_readable() {
    // The walk a client makes when it hits the collision: the name is taken in one Region,
    // the same create there is refused, and the very same create in ANOTHER Region
    // succeeds — because that is a second registry. Each of the two then reports its own
    // registry's host and its own registry's ARN, so a tool keying on either cannot mistake
    // one for the other.
    for pair in REGION_PAIRS {
        let router = emulator();
        let case = format!("{}, repository {}", pair.case, pair.name);
        let expected = published_repository_uri(pair.name);

        let created = ok_in_region(
            &router,
            &case,
            pair.home,
            "CreateRepository",
            &json!({"repositoryName": pair.name, "imageTagMutability": "IMMUTABLE"}),
        )
        .await;
        let home_uri = reported_uri(&case, &created, "/repository/repositoryUri");
        let home_arn = at(&created, "/repository/repositoryArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            home_uri, expected,
            "case {case:?}: expected {expected:?}, got {home_uri:?}"
        );

        // The same name again in the SAME registry is the collision, refused with the code
        // and the sentence AWS words it with.
        let refused = call_in_region(
            &router,
            pair.home,
            "CreateRepository",
            &json!({"repositoryName": pair.name}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a name this registry holds, got {} with \
             {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            ALREADY_EXISTS,
            "case {case:?}: expected {ALREADY_EXISTS}, got {:?}",
            refused.body
        );
        let expected_message = format!(
            "The repository with name '{}' already exists in the registry with id \
             '{REGISTRY_ID}'",
            pair.name
        );
        assert_eq!(
            refused.message(),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            refused.message()
        );

        // The refused create changed nothing the home Region reports.
        let described = ok_in_region(
            &router,
            &case,
            pair.home,
            "DescribeRepositories",
            &json!({"repositoryNames": [pair.name]}),
        )
        .await;
        let held = at(&described, "/repositories")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, 1,
            "case {case:?}: expected exactly one repository of this name in {:?}, got \
             {described}",
            pair.home
        );
        assert_eq!(
            reported_uri(&case, &described, "/repositories/0/repositoryUri"),
            home_uri,
            "case {case:?}: expected the read to report the URI the create did, got \
             {described}"
        );
        assert_eq!(
            at(&described, "/repositories/0/imageTagMutability")
                .as_str()
                .unwrap_or_default(),
            "IMMUTABLE",
            "case {case:?}: expected the refused create to leave the stored mutability \
             alone, got {described}"
        );

        // The second Region's registry does not hold the name, so the same create succeeds
        // there, reporting its own registry's ARN at its own registry's published authority.
        let elsewhere = ok_in_region(
            &router,
            &case,
            pair.other,
            "CreateRepository",
            &json!({"repositoryName": pair.name}),
        )
        .await;
        let other_uri = reported_uri(&case, &elsewhere, "/repository/repositoryUri");
        let other_arn = at(&elsewhere, "/repository/repositoryArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_elsewhere = published_repository_uri(pair.name);
        assert_eq!(
            other_uri, expected_elsewhere,
            "case {case:?}: expected {expected_elsewhere:?}, got {other_uri:?}"
        );
        assert_eq!(
            other_uri, home_uri,
            "case {case:?}: expected one name in two registries to be published at the one \
             endpoint this emulator answers registry requests at, got {other_uri:?} beside \
             {home_uri:?}"
        );
        // Both are published at an address this emulator answers on, and what tells the two
        // repositories apart is the identity reported beside it: the ARN names that Region.
        for (region, uri) in [(pair.home, &home_uri), (pair.other, &other_uri)] {
            assert_published_address(&case, uri, region, pair.name);
        }
        // The ARN names the Region of the registry holding it.
        for (region, arn) in [(pair.home, &home_arn), (pair.other, &other_arn)] {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case:?}: expected the ARN {arn:?} to name the Region of the registry \
                 holding it, {region:?}"
            );
        }
        assert_ne!(
            other_arn, home_arn,
            "case {case:?}: expected two Regions' repositories to carry two ARNs, got \
             {other_arn:?} for both"
        );

        // Both are this account's registries: it is the Region that tells them apart, and
        // each ARN names its own.
        for (region, arn) in [(pair.home, &home_arn), (pair.other, &other_arn)] {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case:?}: expected the ARN {arn:?} to name the registry in \
                 {region:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_pull_with_each_regions_credential_reaches_that_regions_own_image() {
    // What telling the two apart is FOR: with one name in two registries, the reported URI
    // is followed the way a container client follows it — split at the first '/', dialled,
    // and pulled from with the credential that Region's `docker login` token carries — and
    // answers the manifest THAT Region's repository holds. Each address names its own
    // registry, and this registry's `/v2` plane answers whatever host a request arrived on;
    // the two manifests differ, so a pull that reached the other registry answers the wrong
    // document rather than nothing.
    for pair in REGION_PAIRS {
        let router = emulator();
        let case = format!("{}, repository {}", pair.case, pair.name);
        let mut pushed: Vec<(&str, String, String)> = Vec::new();

        for region in [pair.home, pair.other] {
            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": pair.name}),
            )
            .await;
            let uri = reported_uri(&case, &created, "/repository/repositoryUri");
            let config_bytes = format!("config of {region}").into_bytes();
            let config =
                given_layer_in_region(&router, &case, region, pair.name, &config_bytes).await;
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
                    "repositoryName": pair.name,
                    "imageManifest": manifest,
                    "imageTag": "v1",
                }),
            )
            .await;
            pushed.push((region, uri, manifest));
        }

        for (region, uri, manifest) in &pushed {
            let issued =
                ok_in_region(&router, &case, region, "GetAuthorizationToken", &json!({})).await;
            let token = at(&issued, "/authorizationData/0/authorizationToken")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an authorizationToken, got {issued}")
                })
                .to_string();
            let proxy_endpoint = at(&issued, "/authorizationData/0/proxyEndpoint")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (authority, path) = uri
                .split_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
            // Each Region's repository is published under that Region's registry name, with
            // the repository as the path.
            assert_published_address(&case, uri, region, pair.name);
            // The token's own registry URL is the endpoint this caller reached, behind a
            // scheme: it is DIALLED as it is handed over, so it carries the address and the
            // port this process answers on rather than a name to resolve.
            assert_eq!(
                proxy_endpoint,
                crate::wire::announced_registry_url(),
                "case {case:?}: expected the {region:?} login URL to name the endpoint this \
                 emulator answers at, got {proxy_endpoint:?}"
            );

            let pulled = pull_manifest(&router, authority, path, &token).await;
            assert_eq!(
                pulled.0, 200,
                "case {case:?}: a pull with the {region:?} credential must reach that \
                 Region's repository, got HTTP {} with {:?}",
                pulled.0, pulled.1
            );
            assert_eq!(
                pulled.1, *manifest,
                "case {case:?}: expected the manifest pushed in {region:?} from {uri:?}, \
                 got {:?}",
                pulled.1
            );
        }
    }
}

/// `GET /v2/<repository>/manifests/v1` at `host`, with `token` presented the way a
/// container client presents a `docker login` credential — the HTTP status and the body,
/// through the one dialling convention this crate's suites share
/// (`crate::wire::registry_get`).
async fn pull_manifest(
    router: &MultiServiceRouter,
    host: &str,
    repository: &str,
    token: &str,
) -> (u16, String) {
    registry_get(
        router,
        host,
        &format!("/v2/{repository}/manifests/v1"),
        token,
        Some(DOCKER_V2_MANIFEST),
    )
    .await
}

/// One import through a pull through cache rule: the Region it is made in, the rule's prefix
/// and the path read under it.
#[derive(Clone, Copy)]
struct CachedImport {
    case: &'static str,
    /// The Region the rule is registered and the read is made in — the registry that holds
    /// the mirrored repository, which its ARN names.
    region: &'static str,
    /// `ecrRepositoryPrefix` as the create request carries it.
    prefix: &'static str,
    /// The path under the prefix the read addresses: `<prefix>/<path>` is the repository the
    /// import caches into.
    path: &'static str,
    /// The tag the read asks for.
    tag: &'static str,
}

/// The imports the table drives, spanning the Region — and so the partition — the import is
/// made in, how deep the cached path is, whether the prefix is nested, and the tag asked
/// for.
const CACHED_IMPORTS: &[CachedImport] = &[
    CachedImport {
        case: "a one-level path in a commercial Region",
        region: "us-west-2",
        prefix: "ptc-public",
        path: "pause",
        tag: "latest",
    },
    CachedImport {
        case: "a three-level path under a plain prefix",
        region: "ap-southeast-2",
        prefix: "ptc-public",
        path: "docker/library/busybox",
        tag: "1.36.1",
    },
    CachedImport {
        case: "a nested prefix in a GovCloud Region",
        region: "us-gov-east-1",
        prefix: "team/mirror",
        path: "library/alpine",
        tag: "3.20",
    },
    CachedImport {
        case: "a Region of a partition with its own DNS suffix",
        region: "cn-north-1",
        prefix: "ptc-public",
        path: "docker/library/hello-world",
        tag: "latest",
    },
];

/// The upstream these rules cache from, and the `UpstreamRegistry` value that URL belongs to
/// (`smithy_query(query="operation", operation="CreatePullThroughCacheRule")`, member
/// `upstreamRegistryUrl`).
const UPSTREAM_URL: &str = "public.ecr.aws";
const UPSTREAM_REGISTRY: &str = "ecr-public";

/// The repository an import's read addresses: the rule's prefix, then the path under it.
fn cached_repository(import: &CachedImport) -> String {
    format!("{}/{}", import.prefix, import.path)
}

/// Register the import's rule through `CreatePullThroughCacheRule`. A rule's prefix is
/// unique in the registry, so one call covers the reference however many Regions it is read
/// in.
async fn given_cache_rule(router: &MultiServiceRouter, import: &CachedImport) {
    ok_in_region(
        router,
        import.case,
        import.region,
        "CreatePullThroughCacheRule",
        &json!({
            "ecrRepositoryPrefix": import.prefix,
            "upstreamRegistry": UPSTREAM_REGISTRY,
            "upstreamRegistryUrl": UPSTREAM_URL,
        }),
    )
    .await;
}

/// Read the covered reference so the registry imports it, and answer the
/// `DescribeRepositories` body for the repository that import created.
async fn import_and_describe(
    router: &MultiServiceRouter,
    import: &CachedImport,
    repository: &str,
) -> Value {
    let read = ok_in_region(
        router,
        import.case,
        import.region,
        "BatchGetImage",
        &json!({"repositoryName": repository, "imageIds": [{"imageTag": import.tag}]}),
    )
    .await;
    assert_eq!(
        at(&read, "/failures"),
        json!([]),
        "case {:?}: expected the covered reference to be served by caching it, got {read}",
        import.case
    );
    ok_in_region(
        router,
        import.case,
        import.region,
        "DescribeRepositories",
        &json!({"repositoryNames": [repository]}),
    )
    .await
}

#[tokio::test]
async fn a_cached_repository_is_reported_at_its_registrys_host() {
    for import in CACHED_IMPORTS {
        let router = emulator();
        let case = import.case;
        let repository = cached_repository(import);
        given_cache_rule(&router, import).await;
        let described = import_and_describe(&router, import, &repository).await;

        let reported = reported_uri(case, &described, "/repositories/0/repositoryUri");
        let expected = published_repository_uri(&repository);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );

        // The two parts a reader splits the value into: the registry host it dials and
        // parses, and the path after the first '/', which is exactly the repository's own
        // name — the rule's prefix and all, however many segments it carries.
        assert_published_address(case, &reported, import.region, &repository);
    }
}

#[tokio::test]
async fn one_cached_path_in_two_regions_is_told_apart_by_its_registrys_identity() {
    // What the URI is FOR: a client stores the reported value as an image reference it
    // dials, so each Region's mirror of one cached path is published under that Region's own
    // registry name — which the ARN each carries names as well. The same rule and the same
    // covered path are imported in each Region of a pair.
    const PAIRS: &[(&str, &str, &str)] = &[
        ("two commercial Regions", "us-east-1", "eu-west-1"),
        ("two partitions", "us-west-2", "cn-north-1"),
        (
            "a GovCloud Region beside a commercial one",
            "us-gov-west-1",
            "us-east-2",
        ),
    ];
    for &(case, home, other) in PAIRS {
        let router = emulator();
        let mut arns: Vec<(&str, String)> = Vec::new();
        let rule = CachedImport {
            case,
            region: home,
            prefix: "ptc-public",
            path: "docker/library/busybox",
            tag: "latest",
        };
        given_cache_rule(&router, &rule).await;
        for region in [home, other] {
            let import = CachedImport { region, ..rule };
            let repository = cached_repository(&import);
            let described = import_and_describe(&router, &import, &repository).await;
            let uri = reported_uri(case, &described, "/repositories/0/repositoryUri");
            assert_eq!(
                uri,
                published_repository_uri(&repository),
                "case {case:?}: expected the address this emulator publishes the repository \
                 at in {region:?}, got {uri:?}"
            );
            assert_published_address(case, &uri, region, &repository);
            assert_eq!(
                at(&described, "/repositories/0/repositoryName"),
                json!(repository),
                "case {case:?}: expected the cached repository to be reported in {region:?}, \
                 got {described}"
            );
            let arn = at(&described, "/repositories/0/repositoryArn")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case:?}: expected the mirrored repository's ARN to name {region:?}, \
                 got {arn:?}"
            );
            arns.push((region, arn));
        }
        assert_ne!(
            arns[0].1, arns[1].1,
            "case {case:?}: expected {:?} and {:?} to report two ARNs, one per registry, got \
             {:?} for both",
            arns[0].0, arns[1].0, arns[0].1
        );
    }
}

#[tokio::test]
async fn a_cached_repository_and_a_created_one_are_reported_the_same_way() {
    // The URI is composed off the repository's own record, so the path that MINTED it cannot
    // change it: a repository a client created and one the registry imported are reported at
    // the same registry's host, differing only in the name each carries.
    let router = emulator();
    let import = &CACHED_IMPORTS[0];
    let cached = cached_repository(import);
    given_cache_rule(&router, import).await;
    let described = import_and_describe(&router, import, &cached).await;
    let imported_uri = reported_uri(import.case, &described, "/repositories/0/repositoryUri");

    let name = "created/by-a-client";
    let created = ok_in_region(
        &router,
        import.case,
        import.region,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
    let created_uri = reported_uri(import.case, &created, "/repository/repositoryUri");
    for (what, uri, repository) in [
        ("the imported repository", &imported_uri, cached.as_str()),
        ("the created repository", &created_uri, name),
    ] {
        assert_eq!(
            *uri,
            published_repository_uri(repository),
            "case {what:?}: expected the published address of {repository:?}, got {uri:?}"
        );
        assert_published_address(what, uri, import.region, repository);
    }
}

/// One repository created under a pull through cache rule's prefix: the Region it is created
/// in, the rule's prefix and the path under it, which together are the repository's name.
///
/// This is the shape a client that mirrors an upstream image creates — "a pull through cache
/// rule provides a way to cache images from an upstream registry source in your Amazon ECR
/// private registry" (`CreatePullThroughCacheRule`) — and the name it takes carries `/`
/// separators of its own, which is the part of a URI a reader must get back unchanged.
struct PrefixedRepository {
    case: &'static str,
    region: &'static str,
    prefix: &'static str,
    path: &'static str,
    /// The tag the image is pushed and pulled under.
    tag: &'static str,
}

/// The rows the reference test drives: the Region crossed with how deep the prefix and the
/// cached path are, and the tag the reference carries.
const PREFIXED_REPOSITORIES: &[PrefixedRepository] = &[
    PrefixedRepository {
        case: "a three-level path under a plain prefix",
        region: "us-west-2",
        prefix: "ptc-mirror",
        path: "docker/library/busybox",
        tag: "latest",
    },
    PrefixedRepository {
        case: "a one-level path under a nested prefix, in another commercial Region",
        region: "ap-southeast-2",
        prefix: "team/mirror",
        path: "alpine",
        tag: "3.20",
    },
    PrefixedRepository {
        case: "a two-level path in a partition with its own DNS suffix",
        region: "cn-north-1",
        prefix: "ptc-mirror",
        path: "library/hello-world",
        tag: "1.0.0",
    },
    PrefixedRepository {
        case: "a two-level path in a GovCloud Region",
        region: "us-gov-east-1",
        prefix: "ptc-mirror",
        path: "docker/nginx",
        tag: "stable",
    },
];

#[tokio::test]
async fn the_reported_uri_is_the_reference_a_client_pulls_the_image_by() {
    // The whole point of the value, on the names a mirroring client creates: the URI it is
    // handed is what the client writes an image reference in — `<uri>:<tag>` — so it has to
    // carry the registry host and the repository's whole name, and nothing a reference cannot
    // hold: no scheme and no trailing slash. Following it the way a container client does
    // then reaches the image this registry holds.
    for row in PREFIXED_REPOSITORIES {
        let router = emulator();
        let case = format!("{}, in {}", row.case, row.region);
        let repository = format!("{}/{}", row.prefix, row.path);

        ok_in_region(
            &router,
            &case,
            row.region,
            "CreatePullThroughCacheRule",
            &json!({
                "ecrRepositoryPrefix": row.prefix,
                "upstreamRegistry": UPSTREAM_REGISTRY,
                "upstreamRegistryUrl": UPSTREAM_URL,
            }),
        )
        .await;
        let created = ok_in_region(
            &router,
            &case,
            row.region,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let reported = reported_uri(&case, &created, "/repository/repositoryUri");
        let expected = published_repository_uri(&repository);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_published_address(&case, &reported, row.region, &repository);
        assert!(
            !reported.contains("://"),
            "case {case:?}: a reference carries no scheme, got {reported:?}"
        );
        assert!(
            !reported.ends_with('/'),
            "case {case:?}: expected the repository's name to end the URI, got {reported:?}"
        );

        // One image in that repository, then the reference the client composes on what it was
        // handed, split and followed exactly as a container client follows it.
        let config_bytes = format!("config of {repository}").into_bytes();
        let config =
            given_layer_in_region(&router, &case, row.region, &repository, &config_bytes).await;
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
            row.region,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": row.tag,
            }),
        )
        .await;

        let reference = format!("{reported}:{}", row.tag);
        let (dialled, rest) = reference
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: {reference:?} names no repository path"));
        let (path, tag) = rest
            .rsplit_once(':')
            .unwrap_or_else(|| panic!("case {case:?}: {reference:?} names no tag"));
        assert_eq!(
            (path, tag),
            (repository.as_str(), row.tag),
            "case {case:?}: expected the reference to name {repository:?} at {:?}, got \
             {reference:?}",
            row.tag
        );

        let issued =
            ok_in_region(&router, &case, row.region, "GetAuthorizationToken", &json!({})).await;
        let token = at(&issued, "/authorizationData/0/authorizationToken")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an authorizationToken, got {issued}"))
            .to_string();
        let (status, pulled) = registry_get(
            &router,
            dialled,
            &format!("/v2/{path}/manifests/{tag}"),
            &token,
            Some(DOCKER_V2_MANIFEST),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pull of {reference:?} must reach the image it names, got HTTP \
             {status} with {pulled:?}"
        );
        assert_eq!(
            pulled, manifest,
            "case {case:?}: expected the pushed manifest for {reference:?}, got {pulled:?}"
        );
    }
}
