// SPDX-License-Identifier: CC0-1.0

//! The gallery ADDRESS this service publishes — for the registry and for every repository
//! in it — driven end to end over the wire.
//!
//! ONE rule is under test, because the model states one kind of value twice:
//! `Registry$registryUri` ("The URI of a public registry. The URI contains a universal
//! prefix and the registry alias") and `Repository$repositoryUri` ("The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations") are
//! each DIALLED by a container client and PARSED by a client that has to tell which registry
//! holds the repository, so each carries a PUBLIC-REGISTRY NAME — the fixed `public.ecr.`
//! labels AWS leads one with, over the DNS zone this emulator announces its own names in, and
//! no port — then the account's primary registry alias (`RegistryAlias$primaryRegistryAlias`,
//! "displayed in both the repository URL and the image URI used in the docker pull commands")
//! — the opaque eight-character identifier the registry was assigned — and, for a repository,
//! its own name. Nothing else belongs in either: the owning account is reported in
//! `registryId`, and a public registry is global, so no Region appears either.
//!
//! The zone those fixed labels are written over is never one AWS serves — the gallery's own
//! zone, or any zone AWS publishes its endpoints in — since a name under either resolves to
//! AWS rather than to this process and would send a push or a pull out of the emulated
//! environment. It is the zone of the endpoint the CALLER reached, whenever this gallery
//! answers requests there, and otherwise the zone of the address this emulator is served at;
//! neither the port nor an address literal of either reaches the published name, so what is
//! published does not move with the socket this process happens to be bound on.
//!
//! The class the rule generalises over is the endpoint a call arrived on crossed with the
//! repository name and the operation reporting the URI, so the rows span the endpoint forms
//! a caller reaches this emulator by — the emulator's own socket, a loopback name with
//! another port, a bracketed IPv6 literal, an endpoint carried only on the request URI, this
//! service's own endpoint host (which AWS publishes, so no registry client can use it), the
//! gallery AWS serves, a `Host` that is no endpoint at all, a routable address a wildcard
//! bind was reached by, a host name an operator points a client at, and no `Host` at all —
//! crossed
//! with the repository-name forms the modelled `RepositoryName` pattern admits. No row is the
//! recorded request. The ZONE itself is spanned by the last two tests, which serve one
//! emulator per address form this process is served at and assert that the published name
//! moves with the zone and with nothing else — a port included.
//!
//! The composition AWS documents is preserved throughout: a repository's URI is the
//! registry's URI followed by the repository's name, and the four operations that publish
//! one (`CreateRepository`, `DescribeRepositories` filtered and unfiltered,
//! `DeleteRepository`, `DescribeRegistries`) agree for the same repository. The last two
//! tests are what the rule is FOR: the authority the published URI names is dialled and this
//! emulator answers the registry API there, and an image pushed to a repository is pulled
//! back through a container reference composed on the `repositoryUri` the API handed out.
//!
//! A PAGE of repositories is where the composition is read element by element: every element
//! of `DescribeRepositories$repositories` carries the registry's URI followed by THAT
//! element's own `repositoryName`, so a client that enumerates push targets and then pushes
//! to each one reaches the repository it meant rather than its neighbour. That is asserted
//! over the page a registry holding several repositories answers with, over each page of a
//! `maxResults` walk, over a named subset asked for in another order, and over what is left
//! after one repository is deleted.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::registry_address;
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{gallery_pull, GALLERY_HOST};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{
    call_addressed, registry_alias, ARN_PREFIX, PUBLIC_REGISTRY_LABELS, REGISTRY_ID,
};

/// The address this emulator is SERVED at in this suite: the base a call that reached it on
/// no usable endpoint of its own is told to come back to.
const SERVED_URL_BASE: &str = "https://127.0.0.1:9297";

/// The `host[:port]` half of [`SERVED_URL_BASE`], which is the authority published to such a
/// call.
const SERVED_ENDPOINT: &str = "127.0.0.1:9297";

/// The Region every call in this suite is signed for, and the one Region string a published
/// URI is checked against: a public registry is global, so no Region may appear in one.
const SIGNED_REGION: &str = "us-east-1";

/// The two fixed labels every public-registry authority AWS publishes leads with, spelled out
/// here rather than taken from the crate so that a reworded prefix fails this suite.
const PUBLIC_REGISTRY_PREFIX: &str = "public.ecr.";

/// The public-registry NAME every URI published to a call in this suite carries: the fixed
/// labels over the LOOPBACK zone, which is the zone this emulator announces its own names in
/// while it is reached at a loopback address and while it is served at one
/// ([`SERVED_URL_BASE`]) — spelled out rather than derived, so what is asserted is the name
/// itself and not a recomputation of the implementation's own reading.
///
/// It carries no port and no address literal: both change with the socket this process is
/// bound on, and neither is part of a registry's name.
const PUBLISHED_AUTHORITY: &str = "public.ecr.localhost";

/// One endpoint a caller reaches this emulator on: where the endpoint travels (the `Host`
/// header, the request URI's authority, or neither) and the authority every URI published to
/// that call is built on.
struct Endpoint {
    case: &'static str,
    host: Option<&'static str>,
    uri_authority: Option<&'static str>,
    /// The authority this registry publishes to such a call. Spelled out per row rather than
    /// derived, so what is asserted is the name itself and not a recomputation of the
    /// implementation's own reading.
    published: &'static str,
}

/// The endpoints the table drives, spanning the forms a caller reaches this emulator by.
///
/// Every one of them is answered the same public-registry name ([`PUBLISHED_AUTHORITY`]): the
/// zone read off the endpoint a registry client can dial is the loopback zone, and so is the
/// zone of the address this emulator is served at, which every other form falls back to. That
/// is the point of the table — nothing a caller wrote, and no port of the socket it reached,
/// reaches the name this registry publishes.
fn endpoints() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "the emulator's own socket",
            host: Some(SERVED_ENDPOINT),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            case: "a loopback name with another port",
            host: Some("localhost:39355"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            case: "a bracketed IPv6 loopback literal",
            host: Some("[::1]:38001"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            case: "an endpoint carried only on the request URI",
            host: None,
            uri_authority: Some("127.0.0.1:8443"),
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            // A host AWS publishes its own endpoints at routes to a service rather than to a
            // registry, so the zone of the served address is read instead.
            case: "this service's endpoint host, which AWS publishes",
            host: Some("api.ecr-public.us-east-1.amazonaws.com"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            // AWS's own gallery zone resolves to the real public registry, so it is not a zone
            // this emulator may publish in: the served address's zone is read instead.
            case: "the gallery AWS serves",
            host: Some(GALLERY_HOST),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            case: "a Host that is no endpoint at all",
            host: Some("elsewhere.example.com/gallery"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            // A routable literal a caller reached a wildcard bind by is not an address the
            // reading of a container reference recognises, so no zone is read off it: the
            // served address's zone is read instead, and publishing and reading agree.
            case: "a routable address a wildcard bind was reached by",
            host: Some("10.1.2.3:5000"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            // A name an operator points a client at carries a first label the front door
            // reads as a service, so it is of no use to a registry client either.
            case: "a host name an operator points a client at",
            host: Some("emulator.internal:8080"),
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
        Endpoint {
            case: "no endpoint on the request at all",
            host: None,
            uri_authority: None,
            published: PUBLISHED_AUTHORITY,
        },
    ]
}

/// The repository names the table creates, spanning the forms the modelled `RepositoryName`
/// pattern `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$` (length
/// 2..205) admits — a name at the shortest the bound allows, a flat name, one carrying each
/// separator, and namespaced names of two and three segments. Source: the model, via
/// `smithy_query(query="shape", shape="Repository")`.
const REPOSITORY_NAMES: [&str; 5] = [
    "ab",
    "nginx",
    "team.app_v1-2",
    "project-a/nginx-web-app",
    "org/team/app",
];

/// One emulator: a router carrying only this service over one shared engine, served at
/// [`SERVED_URL_BASE`], with the engine it keeps its registry in handed back — so a pull
/// decided in process reads the very registry the wire calls wrote.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope below
    // still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.set_url_base(
        ::smithy_server_runtime::EndpointUrl::parse(SERVED_URL_BASE)
            .expect("the base URL must parse"),
    );
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One emulator, for a case that drives it over the wire alone.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// One `awsJson1_1` call that must answer 200, addressed to the endpoint `endpoint`
/// describes.
async fn call_at(
    router: &MultiServiceRouter,
    endpoint: &Endpoint,
    operation: &str,
    body: Value,
) -> Value {
    call_addressed(
        router,
        endpoint.case,
        endpoint.host,
        endpoint.uri_authority,
        operation,
        body,
    )
    .await
}

/// The string `value` carries, or a panic naming the case and what was read instead.
fn string_at(case: &str, value: &Value, label: &str) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {label}, got {value}"))
        .to_string()
}

/// The registry URI `DescribeRegistries` publishes to a call that arrived at `endpoint`,
/// asserted to be the published endpoint scoped by the registry's PRIMARY alias — the one
/// flagged `RegistryAlias$primaryRegistryAlias` on that very answer, which is the alias a
/// pull URL shows — and to carry neither the owning account nor a Region.
async fn published_registry_uri(router: &MultiServiceRouter, endpoint: &Endpoint) -> String {
    let described = call_at(router, endpoint, "DescribeRegistries", json!({})).await;
    let registry = &described["registries"][0];
    let uri = string_at(endpoint.case, &registry["registryUri"], "registryUri");
    let primary = registry["aliases"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the registry's alias records, got {described}",
                endpoint.case
            )
        })
        .iter()
        .find(|alias| alias["primaryRegistryAlias"] == json!(true))
        .map(|alias| string_at(endpoint.case, &alias["name"], "aliases[].name"))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected one alias flagged primaryRegistryAlias, got {described}",
                endpoint.case
            )
        });
    assert_eq!(
        uri,
        format!("{}/{primary}", endpoint.published),
        "case {:?}: expected the registry URI on the public-registry name this emulator \
         announces, scoped by the primary alias, got {uri:?}",
        endpoint.case
    );
    assert!(
        !uri.contains(REGISTRY_ID),
        "case {:?}: the owning account is reported in registryId, never in {uri:?}",
        endpoint.case
    );
    assert!(
        !uri.contains(SIGNED_REGION),
        "case {:?}: a public registry is global, so no Region belongs in {uri:?}",
        endpoint.case
    );
    uri
}

/// The authority a published URI names, and the path a client extends on it.
fn dialled(case: &str, uri: &str) -> (String, String) {
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no path"));
    assert_public_registry_name(case, authority);
    // And it has to lead back to this process: the zone announced is one this emulator
    // answers in, by the one reading of that question this service states — the READING side
    // of the publishing, so a container reference composed on what was handed out is
    // recognised as this gallery's.
    assert!(
        registry_address::names_this_emulator(authority),
        "case {case:?}: expected {uri:?} in a zone this emulator answers in"
    );
    // The addresses a published URI may never name: a host AWS serves and this emulator does
    // not — the gallery's own name, and any zone AWS publishes its endpoints in.
    let [public, ecr] = PUBLIC_REGISTRY_LABELS;
    assert_ne!(
        authority, GALLERY_HOST,
        "case {case:?}: {uri:?} names the real gallery host ({public}.{ecr}.…), which this \
         emulator does not answer on"
    );
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        // The authority may carry a `:port`, and it is the HOST that says whose endpoint
        // this is, so the port goes first — by the tree's one reading of that.
        let host = service_common::data_plane_addressing::host_only(authority);
        assert!(
            !host.ends_with(suffix),
            "case {case:?}: {uri:?} is served in a zone AWS publishes its own endpoints in, \
             which this emulator does not answer in"
        );
    }
    (authority.to_string(), path.to_string())
}

/// Assert that `authority` is a PUBLIC-REGISTRY NAME: the fixed labels AWS leads one with,
/// then at least one further non-empty label, and nothing of the socket this process happens
/// to be bound on — no port and no address literal, either of which moves on every boot.
fn assert_public_registry_name(case: &str, authority: &str) {
    let zone = authority
        .strip_prefix(PUBLIC_REGISTRY_PREFIX)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {authority:?} to lead with {PUBLIC_REGISTRY_PREFIX:?}"
            )
        });
    assert!(
        !zone.is_empty() && zone.split('.').all(|label| !label.is_empty()),
        "case {case:?}: expected a real DNS zone behind the fixed labels, got {authority:?}"
    );
    assert!(
        !authority.contains(':'),
        "case {case:?}: a registry's name carries no port, got {authority:?}"
    );
    assert!(
        zone.parse::<std::net::IpAddr>().is_err(),
        "case {case:?}: a registry's name is not an address literal, got {authority:?}"
    );
}

/// The DNS suffixes AWS publishes its own endpoints under, spelled out here rather than
/// imported so that a published address slipping into one of them fails this suite.
///
/// Source: `service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES`, the tree's one
/// list of them, which this service's reading of the addresses it may publish is written on.
const AWS_ENDPOINT_DNS_SUFFIXES: [&str; 4] =
    [".amazonaws.com", ".amazonaws.com.cn", ".api.aws", ".on.aws"];

/// The form of the alias AWS assigns an account's public registry, asserted on the value
/// reported — spelled out here rather than taken from the crate so a reworded alias fails:
/// an OPAQUE identifier of eight characters alternating a lowercase letter and a digit, well
/// inside the modelled `RegistryAliasName` (`^[a-z][a-z0-9]+(?:[._-][a-z0-9]+)*$`, length
/// 2..50 — source: `smithy_query(query="shape", shape="RegistryAliasName")`).
fn assert_alias_form(case: &str, alias: &str) {
    let characters: Vec<char> = alias.chars().collect();
    assert_eq!(
        characters.len(),
        8,
        "case {case:?}: expected an eight-character registry alias, got {alias:?}"
    );
    for (position, character) in characters.iter().enumerate() {
        let (wanted, holds) = if position % 2 == 0 {
            ("a lowercase letter", character.is_ascii_lowercase())
        } else {
            ("a digit", character.is_ascii_digit())
        };
        assert!(
            holds,
            "case {case:?}: expected {wanted} at position {position} of {alias:?}, got \
             {character:?}"
        );
    }
}

#[tokio::test]
async fn the_registry_alias_every_published_uri_carries_is_the_identifier_aws_assigns() {
    // RULE's other half: the alias scoping every published URI is the opaque identifier the
    // registry was assigned — reported on `Registry$aliases` as both the primary and the
    // default alias — and the same one in every operation that publishes a URI.
    let router = emulator();
    let endpoint = &endpoints()[0];
    let described = call_at(&router, endpoint, "DescribeRegistries", json!({})).await;
    let registry = &described["registries"][0];
    let reported = string_at(
        endpoint.case,
        &registry["aliases"][0]["name"],
        "aliases[0].name",
    );
    assert_alias_form("the alias the registry reports", &reported);
    assert_eq!(
        reported,
        registry_alias(),
        "expected the alias this account's registry is assigned, got {reported:?}"
    );
    for flag in ["primaryRegistryAlias", "defaultRegistryAlias"] {
        assert_eq!(
            registry["aliases"][0][flag],
            json!(true),
            "expected the assigned alias to be flagged {flag}, got {described}"
        );
    }

    // Every operation that publishes a URI scopes it by that very alias, for each repository
    // name form the modelled pattern admits.
    for name in REPOSITORY_NAMES {
        let case = format!("repository {name}");
        let created = call_at(
            &router,
            endpoint,
            "CreateRepository",
            json!({"repositoryName": name}),
        )
        .await;
        let listed = call_at(&router, endpoint, "DescribeRepositories", json!({})).await;
        let deleted = call_at(
            &router,
            endpoint,
            "DeleteRepository",
            json!({"repositoryName": name}),
        )
        .await;
        let published = [
            ("CreateRepository", created["repository"].clone()),
            (
                "DescribeRepositories",
                listed["repositories"]
                    .as_array()
                    .and_then(|page| {
                        page.iter()
                            .find(|repository| repository["repositoryName"] == json!(name))
                            .cloned()
                    })
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected it listed, got {listed}")
                    }),
            ),
            ("DeleteRepository", deleted["repository"].clone()),
        ];
        for (operation, repository) in published {
            let uri = string_at(&case, &repository["repositoryUri"], "repositoryUri");
            let (_authority, path) = dialled(&case, &uri);
            let alias = path
                .split_once('/')
                .map(|(alias, _repository)| alias.to_string())
                .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository"));
            assert_alias_form(&format!("{operation} on {case}"), &alias);
            assert_eq!(
                alias, reported,
                "case {case:?}: expected {operation} to scope {uri:?} by the alias the \
                 registry reports, got {alias:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_published_repository_uri_names_the_public_registry_this_emulator_announces() {
    for endpoint in endpoints() {
        let router = emulator();
        let registry_uri = published_registry_uri(&router, &endpoint).await;
        for name in REPOSITORY_NAMES {
            let case = format!("{}, repository {name}", endpoint.case);
            // The registry's URI followed by the repository's own name, which is the
            // composition a client splits and extends.
            let expected = format!("{registry_uri}/{name}");

            let created = call_at(
                &router,
                &endpoint,
                "CreateRepository",
                json!({"repositoryName": name}),
            )
            .await;
            let created_uri = string_at(
                &case,
                &created["repository"]["repositoryUri"],
                "repository.repositoryUri",
            );
            assert_eq!(
                created_uri, expected,
                "case {case:?}: expected CreateRepository to publish {expected:?}, got \
                 {created_uri:?}"
            );
            let (authority, path) = dialled(&case, &created_uri);
            assert_eq!(
                authority, endpoint.published,
                "case {case:?}: expected the dialled authority to be the public-registry \
                 name this emulator announces, got {authority:?}"
            );
            assert_eq!(
                path,
                format!("{}/{name}", registry_alias()),
                "case {case:?}: expected the alias-scoped repository path, got {path:?}"
            );

            // The ARN and the registry id are LABELS of the stored record, so the endpoint
            // the call arrived on does not touch them.
            let arn = string_at(
                &case,
                &created["repository"]["repositoryArn"],
                "repository.repositoryArn",
            );
            assert_eq!(
                arn,
                format!("{ARN_PREFIX}{name}"),
                "case {case:?}: expected the stored repository ARN, got {arn:?}"
            );
            assert_eq!(
                string_at(
                    &case,
                    &created["repository"]["registryId"],
                    "repository.registryId"
                ),
                REGISTRY_ID,
                "case {case:?}: expected the registry id of this account, got {created}"
            );

            // The named read, and the unfiltered listing a client discovers push targets
            // through, publish the same address.
            let named = call_at(
                &router,
                &endpoint,
                "DescribeRepositories",
                json!({"repositoryNames": [name]}),
            )
            .await;
            let named_uri = string_at(
                &case,
                &named["repositories"][0]["repositoryUri"],
                "repositories[0].repositoryUri",
            );
            assert_eq!(
                named_uri, expected,
                "case {case:?}: expected the named DescribeRepositories to publish \
                 {expected:?}, got {named_uri:?}"
            );
            let listed = call_at(&router, &endpoint, "DescribeRepositories", json!({})).await;
            let listed_uris: Vec<String> = listed["repositories"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a page, got {listed}"))
                .iter()
                .map(|repository| {
                    string_at(&case, &repository["repositoryUri"], "repositoryUri")
                })
                .collect();
            assert!(
                listed_uris.contains(&expected),
                "case {case:?}: expected the listing to publish {expected:?}, got \
                 {listed_uris:?}"
            );
            for uri in &listed_uris {
                let (listed_authority, _) = dialled(&case, uri);
                assert_eq!(
                    listed_authority, endpoint.published,
                    "case {case:?}: every discovered push target is on the public-registry \
                     name this emulator announces, got {uri:?}"
                );
            }

            // The echoed repository a delete reports carries the same address.
            let deleted = call_at(
                &router,
                &endpoint,
                "DeleteRepository",
                json!({"repositoryName": name}),
            )
            .await;
            let deleted_uri = string_at(
                &case,
                &deleted["repository"]["repositoryUri"],
                "repository.repositoryUri",
            );
            assert_eq!(
                deleted_uri, expected,
                "case {case:?}: expected DeleteRepository to echo {expected:?}, got \
                 {deleted_uri:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_published_uri_names_the_public_registry_the_read_was_answered_by() {
    // The stored repository is one record, and its ARN — a LABEL — follows from that record
    // alone. Its URI is an ADDRESS, so it is composed per answer, over the zone this emulator
    // announces its own names in to that reader, and never stored with the record.
    let router = emulator();
    let endpoints = endpoints();
    let creating = &endpoints[0];
    let name = "published/per-reader";
    let created = call_at(
        &router,
        creating,
        "CreateRepository",
        json!({"repositoryName": name}),
    )
    .await;
    let created_at = created["repository"]["createdAt"].clone();
    assert_eq!(
        string_at(
            creating.case,
            &created["repository"]["repositoryUri"],
            "repository.repositoryUri"
        ),
        format!("{}/{}/{name}", creating.published, registry_alias()),
        "case {:?}: expected the create to publish the name this emulator announces, got \
         {created}",
        creating.case
    );

    for endpoint in &endpoints {
        let case = endpoint.case;
        let registry_uri = published_registry_uri(&router, endpoint).await;
        let listed = call_at(&router, endpoint, "DescribeRepositories", json!({})).await;
        let listed_uri = string_at(
            case,
            &listed["repositories"][0]["repositoryUri"],
            "repositories[0].repositoryUri",
        );
        assert_eq!(
            listed_uri,
            format!("{registry_uri}/{name}"),
            "case {case:?}: expected the read to publish the name this emulator announces, \
             got {listed_uri:?}"
        );
        // A read publishes an address; it changes nothing a later caller sees, so the
        // record's own labels are the ones the create stored.
        let arn = string_at(
            case,
            &listed["repositories"][0]["repositoryArn"],
            "repositories[0].repositoryArn",
        );
        assert_eq!(
            arn,
            format!("{ARN_PREFIX}{name}"),
            "case {case:?}: expected the stored ARN to be unchanged by the read's endpoint, \
             got {arn:?}"
        );
        assert_eq!(
            listed["repositories"][0]["createdAt"], created_at,
            "case {case:?}: expected the stored createdAt to be unchanged by the read's \
             endpoint, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_authority_a_published_uri_names_answers_the_registry_api() {
    // What the rule is FOR: a client takes the authority it was handed and dials it. Each
    // row dials the authority the URI published and asks this registry about the very
    // repository that URI names, so the address is proved to lead back to this emulator
    // rather than merely to carry the right labels.
    for endpoint in endpoints() {
        let router = emulator();
        let name = "followed/by-a-client";
        let case = endpoint.case;
        let created = call_at(
            &router,
            &endpoint,
            "CreateRepository",
            json!({"repositoryName": name}),
        )
        .await;
        let published = string_at(
            case,
            &created["repository"]["repositoryUri"],
            "repository.repositoryUri",
        );
        let (authority, _path) = dialled(case, &published);

        let answered = call_addressed(
            &router,
            case,
            Some(&authority),
            None,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
        )
        .await;
        let reported = string_at(
            case,
            &answered["repositories"][0]["repositoryUri"],
            "repositories[0].repositoryUri",
        );
        assert_eq!(
            reported, published,
            "case {case:?}: expected the registry dialled at {authority:?} to publish the \
             repository {published:?} names, got {reported:?}"
        );
        assert_eq!(
            string_at(
                case,
                &answered["repositories"][0]["repositoryName"],
                "repositories[0].repositoryName"
            ),
            name,
            "case {case:?}: expected the repository the published URI names, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_reference_composed_on_a_published_uri_pulls_the_image_pushed_to_it() {
    // The other half of what the rule is FOR: a client pushes to the URI the API handed it,
    // and a container runtime pulls the reference it composed on that URI — so the address
    // this registry publishes and the address it answers for are the same one. The rows span
    // the reference forms a runtime writes (a tag, the implicit tag, and a digest) over a
    // namespaced repository name, and the reference a container definition spells by hand on
    // the gallery AWS serves, which names this registry by its alias.
    let (router, engine) = emulator_with_engine();
    let endpoints = endpoints();
    let endpoint = &endpoints[0];
    let case = endpoint.case;
    let name = "team/app";
    let created = call_at(
        &router,
        endpoint,
        "CreateRepository",
        json!({"repositoryName": name}),
    )
    .await;
    let published = string_at(
        case,
        &created["repository"]["repositoryUri"],
        "repository.repositoryUri",
    );
    let pushed = call_at(
        &router,
        endpoint,
        "PutImage",
        json!({
            "repositoryName": name,
            "imageManifest": json!({"schemaVersion": 2, "layers": []}).to_string(),
            "imageTag": "latest",
        }),
    )
    .await;
    let digest = string_at(
        case,
        &pushed["image"]["imageId"]["imageDigest"],
        "image.imageId.imageDigest",
    );

    for reference in [
        format!("{published}:latest"),
        published.clone(),
        format!("{published}@{digest}"),
        format!("{GALLERY_HOST}/{}/{name}:latest", registry_alias()),
    ] {
        let parsed = gallery_pull::parse_gallery_reference(&reference).unwrap_or_else(|| {
            panic!("case {reference:?}: expected a reference into this gallery")
        });
        assert_eq!(
            parsed.registry_alias,
            registry_alias(),
            "case {reference:?}: expected the alias the URI was published with, got {:?}",
            parsed.registry_alias
        );
        assert_eq!(
            parsed.repository_name, name,
            "case {reference:?}: expected the repository the URI names, got {:?}",
            parsed.repository_name
        );
        assert!(
            gallery_pull::serves_registry_alias(&*engine, &parsed.registry_alias),
            "case {reference:?}: expected this registry to serve the alias it published"
        );
        let served = gallery_pull::serve_pull(&*engine, &parsed);
        assert_eq!(
            served,
            Ok(Some(digest.clone())),
            "case {reference:?}: expected the pushed manifest digest, got {served:?}"
        );
    }

    // A reference on another registry's authority is not this gallery's to answer, however
    // the one this registry published is spelled.
    for foreign in [
        format!("quay.io/{}/{name}:latest", registry_alias()),
        format!("{REGISTRY_ID}.dkr.ecr.{SIGNED_REGION}.amazonaws.com/{name}:latest"),
    ] {
        assert!(
            gallery_pull::parse_gallery_reference(&foreign).is_none(),
            "case {foreign:?}: expected no reference into this gallery"
        );
    }
}

/// The page sizes a paginated read of the repositories is driven at.
///
/// `DescribeRepositories$maxResults` is modelled `range 1..1000` (source:
/// `smithy_query(query="operation", operation="DescribeRepositories")`), so the rows span its
/// published lower bound, sizes that split the stored repositories unevenly, the size that is
/// exactly the number held, and the published upper bound, which holds them all in one page.
const PAGE_SIZES: [usize; 5] = [1, 2, 3, REPOSITORY_NAMES.len(), 1000];

/// Every `(repositoryName, repositoryUri)` pair one `DescribeRepositories` page carries, in
/// the order the page carries them — the two members a client has to read TOGETHER, since the
/// URI is the address of the repository the same element names.
fn page_pairs(case: &str, answered: &Value) -> Vec<(String, String)> {
    answered["repositories"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a page of repositories, got {answered}")
        })
        .iter()
        .map(|repository| {
            (
                string_at(case, &repository["repositoryName"], "repositoryName"),
                string_at(case, &repository["repositoryUri"], "repositoryUri"),
            )
        })
        .collect()
}

/// Assert that each pair of `page` publishes the URI of its OWN repository under
/// `registry_uri`, and that the page names exactly `expected`, in that order.
fn assert_page_publishes_each_repositorys_own_uri(
    case: &str,
    registry_uri: &str,
    expected: &[&str],
    page: &[(String, String)],
) {
    assert_eq!(
        page.len(),
        expected.len(),
        "case {case:?}: expected {} repositories on the page, got {page:?}",
        expected.len()
    );
    for (position, (wanted, (listed_name, listed_uri))) in
        expected.iter().zip(page.iter()).enumerate()
    {
        assert_eq!(
            listed_name, wanted,
            "case {case:?}: expected {wanted:?} at index {position} of the page, got \
             {listed_name:?}"
        );
        assert_eq!(
            listed_uri,
            &format!("{registry_uri}/{listed_name}"),
            "case {case:?}: expected index {position} to publish the registry URI followed by \
             its own repository name, got {listed_uri:?}"
        );
        // The authority half is still an address this emulator answers on, and never one AWS
        // serves — read element by element, because a page is a list of addresses.
        dialled(case, listed_uri);
    }
}

#[tokio::test]
async fn every_element_of_a_repository_page_publishes_its_own_repositorys_uri() {
    // RULE read element by element. A page is a list of ADDRESSES, so element i carries the
    // registry's own URI followed by element i's `repositoryName` — never a neighbour's —
    // whatever endpoint the read was answered at, however many repositories the registry
    // holds, however the page is split, and whichever of them are still there. The rows span
    // the endpoint forms a caller reaches this emulator by, crossed with the repository-name
    // forms the modelled `RepositoryName` pattern admits, the page sizes `maxResults` admits,
    // and the three read shapes a client enumerates push targets through (the whole page, a
    // `maxResults` walk, a named subset). No row is the recorded request.
    for endpoint in endpoints() {
        let router = emulator();
        // The registry's own URI, taken from `DescribeRegistries` on this very endpoint, is
        // what every element of every page below has to extend.
        let registry_uri = published_registry_uri(&router, &endpoint).await;
        for name in REPOSITORY_NAMES {
            call_at(
                &router,
                &endpoint,
                "CreateRepository",
                json!({"repositoryName": name}),
            )
            .await;
        }
        // The listing walks one stable `repositoryName` order — which is what makes a page
        // token resume exactly after the page it came with — so that is the order a page
        // carries its elements in.
        let mut ordered: Vec<&str> = REPOSITORY_NAMES.to_vec();
        ordered.sort_unstable();

        let whole_case = format!("{}, the whole page", endpoint.case);
        let whole = page_pairs(
            &whole_case,
            &call_at(&router, &endpoint, "DescribeRepositories", json!({})).await,
        );
        assert_page_publishes_each_repositorys_own_uri(
            &whole_case,
            &registry_uri,
            &ordered,
            &whole,
        );

        for page_size in PAGE_SIZES {
            let case = format!("{}, maxResults {page_size}", endpoint.case);
            let mut walked: Vec<(String, String)> = Vec::new();
            let mut next_token: Option<String> = None;
            // One request per repository at the smallest page size, plus the request that
            // finds the walk already finished: a token that fails to advance ends the loop
            // with repositories still unseen, which the comparison below then reports, rather
            // than spinning here.
            for _ in 0..=ordered.len() {
                let mut request = json!({"maxResults": page_size});
                if let Some(token) = next_token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let answered =
                    call_at(&router, &endpoint, "DescribeRepositories", request).await;
                let pairs = page_pairs(&case, &answered);
                assert!(
                    pairs.len() <= page_size,
                    "case {case:?}: expected at most {page_size} elements on a page, got {}",
                    pairs.len()
                );
                for (name, uri) in &pairs {
                    assert_eq!(
                        uri,
                        &format!("{registry_uri}/{name}"),
                        "case {case:?}: expected the page element {name:?} to publish its own \
                         URI, got {uri:?}"
                    );
                    dialled(&case, uri);
                }
                walked.extend(pairs);
                next_token = answered["nextToken"].as_str().map(str::to_string);
                if next_token.is_none() {
                    break;
                }
            }
            assert!(
                next_token.is_none(),
                "case {case:?}: expected the walk to end, got the token {next_token:?} after \
                 {walked:?}"
            );
            // Split into pages or served whole, the walk hands back the same repositories in
            // the same order, each with the same address: nothing is published twice and
            // nothing is dropped at a page boundary.
            assert_eq!(
                walked, whole,
                "case {case:?}: expected the paginated walk to publish every repository \
                 exactly once in the page's order, got {walked:?}"
            );
        }

        // A named subset is answered for exactly the repositories asked for, each carrying
        // its own URI, whatever order the request listed them in.
        let requested: Vec<&str> = ordered.iter().rev().copied().take(3).collect();
        let mut wanted = requested.clone();
        wanted.sort_unstable();
        let named_case = format!("{}, a named subset", endpoint.case);
        let named = page_pairs(
            &named_case,
            &call_at(
                &router,
                &endpoint,
                "DescribeRepositories",
                json!({"repositoryNames": requested}),
            )
            .await,
        );
        assert_page_publishes_each_repositorys_own_uri(
            &named_case,
            &registry_uri,
            &wanted,
            &named,
        );

        // The repository a delete echoes carries its own URI, and the page that is left
        // publishes each remaining repository's own URI — the address does not shift onto a
        // neighbour when the registry's contents change.
        let removed = ordered
            .first()
            .copied()
            .unwrap_or_else(|| panic!("case {:?}: the table names repositories", endpoint.case));
        let deleted_case = format!("{}, after deleting {removed}", endpoint.case);
        let deleted = call_at(
            &router,
            &endpoint,
            "DeleteRepository",
            json!({"repositoryName": removed}),
        )
        .await;
        let deleted_uri = string_at(
            &deleted_case,
            &deleted["repository"]["repositoryUri"],
            "repository.repositoryUri",
        );
        assert_eq!(
            deleted_uri,
            format!("{registry_uri}/{removed}"),
            "case {deleted_case:?}: expected the echoed repository to publish its own URI, got \
             {deleted_uri:?}"
        );
        let remaining: Vec<&str> = ordered
            .iter()
            .copied()
            .filter(|name| *name != removed)
            .collect();
        let left = page_pairs(
            &deleted_case,
            &call_at(&router, &endpoint, "DescribeRepositories", json!({})).await,
        );
        assert_page_publishes_each_repositorys_own_uri(
            &deleted_case,
            &registry_uri,
            &remaining,
            &left,
        );
    }
}

/// One address this emulator is served at: the base it reports as its own endpoint, and the
/// public-registry name every URI it publishes then carries.
struct ServedAt {
    case: &'static str,
    /// The base this emulator is served at, or `None` for a process served on no port at all,
    /// which reports the unserved base (`smithy_server_runtime::UNSERVED_URL_BASE`, a name RFC
    /// 6761 reserves so that nothing resolves it).
    base: Option<&'static str>,
    /// The authority published to a call that address answers. Spelled out per row rather than
    /// derived, so what is asserted is the name itself.
    published: &'static str,
}

/// The addresses this emulator is served at, one row each: the class the ZONE of a published
/// name generalises over.
fn served_addresses() -> Vec<ServedAt> {
    vec![
        ServedAt {
            case: "served on the loopback IPv4 socket the core binds",
            base: Some(SERVED_URL_BASE),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            // An address literal names the interface the loopback zone resolves to and names
            // no zone of its own, so the zone RFC 6761 reserves for it is what is announced.
            case: "served on another loopback address and port",
            base: Some("https://127.3.2.1:34427"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            case: "served on the loopback IPv6 socket",
            base: Some("https://[::1]:38001"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            case: "served under the zone RFC 6761 reserves for that interface",
            base: Some("https://localhost:39355"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            case: "served under a name an operator runs this process under",
            base: Some("https://gallery.internal.example:8443"),
            published: "public.ecr.gallery.internal.example",
        },
        ServedAt {
            // A host is case-insensitive, and a DNS name is published in one spelling.
            case: "served under that name written in capitals",
            base: Some("https://GALLERY.INTERNAL.EXAMPLE:8443"),
            published: "public.ecr.gallery.internal.example",
        },
        ServedAt {
            // A zone AWS publishes its own endpoints in resolves at genuine AWS rather than
            // here, so it is never a zone this emulator announces a name of its own in.
            case: "served at a host AWS publishes its own endpoints at",
            base: Some("https://api.ecr-public.us-east-1.amazonaws.com"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            // Neither is the zone AWS serves its own gallery in: the fixed labels written over
            // it would spell the real gallery host, so a push to the published name would
            // leave the emulated environment altogether.
            case: "served under the zone AWS serves its own gallery in",
            base: Some("https://gallery.ecr.aws:8443"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            // An endpoint that is ITSELF a name this registry published: the fixed labels come
            // off before the name is composed again, so publishing is idempotent rather than
            // nesting them.
            case: "served under a name this registry itself publishes",
            base: Some("https://public.ecr.localhost:9297"),
            published: PUBLISHED_AUTHORITY,
        },
        ServedAt {
            case: "served on no port at all",
            base: None,
            published: "public.ecr.emulator.invalid",
        },
    ]
}

/// One emulator carrying only this service over one shared engine, served at `base` — and the
/// engine it keeps its registry in, so a pull decided in process reads the registry the wire
/// calls wrote.
fn emulator_served_at(base: Option<&str>) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope still
    // routes the request to `ecr-public`.
    router.set_request_authentication(false);
    if let Some(base) = base {
        router.set_url_base(
            ::smithy_server_runtime::EndpointUrl::parse(base).expect("the base URL must parse"),
        );
    }
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One `awsJson1_1` call that must answer 200 and states no endpoint of its own, so what the
/// answer publishes is read off the address this emulator is served at.
async fn call_unaddressed(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    call_addressed(router, case, None, None, operation, body).await
}

#[tokio::test]
async fn every_published_uri_names_the_public_registry_in_the_zone_this_emulator_is_served_in() {
    // RULE's other side: the fixed labels are written over the zone this emulator announces
    // its own names in, which is read off the address it is SERVED at for a call that states
    // no endpoint of its own. The rows span that address — a loopback socket, another loopback
    // address and port, the IPv6 loopback socket, the zone RFC 6761 reserves for that
    // interface, an operator's own name in two spellings, a host AWS publishes its own
    // endpoints at, and a process served on no port at all — crossed with the repository-name
    // forms the modelled `RepositoryName` pattern admits and the four operations that publish
    // such a URI.
    for served in served_addresses() {
        let (router, _engine) = emulator_served_at(served.base);
        let case = served.case;
        let registries = call_unaddressed(&router, case, "DescribeRegistries", json!({})).await;
        let registry_uri = string_at(
            case,
            &registries["registries"][0]["registryUri"],
            "registries[0].registryUri",
        );
        assert_eq!(
            registry_uri,
            format!("{}/{}", served.published, registry_alias()),
            "case {case:?}: expected the published registry name scoped by the registry's \
             alias, got {registry_uri:?}"
        );
        assert_public_registry_name(case, served.published);

        for name in REPOSITORY_NAMES {
            let case = format!("{case}, repository {name}");
            let expected = format!("{registry_uri}/{name}");
            let created = call_unaddressed(
                &router,
                &case,
                "CreateRepository",
                json!({"repositoryName": name}),
            )
            .await;
            let listed = call_unaddressed(&router, &case, "DescribeRepositories", json!({})).await;
            let named = call_unaddressed(
                &router,
                &case,
                "DescribeRepositories",
                json!({"repositoryNames": [name]}),
            )
            .await;
            let deleted = call_unaddressed(
                &router,
                &case,
                "DeleteRepository",
                json!({"repositoryName": name}),
            )
            .await;
            let published = [
                ("CreateRepository", created["repository"].clone()),
                (
                    "DescribeRepositories",
                    listed["repositories"]
                        .as_array()
                        .and_then(|page| {
                            page.iter()
                                .find(|repository| repository["repositoryName"] == json!(name))
                                .cloned()
                        })
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected it listed, got {listed}")
                        }),
                ),
                (
                    "DescribeRepositories by name",
                    named["repositories"][0].clone(),
                ),
                ("DeleteRepository", deleted["repository"].clone()),
            ];
            for (operation, repository) in published {
                let uri = string_at(&case, &repository["repositoryUri"], "repositoryUri");
                assert_eq!(
                    uri, expected,
                    "case {case:?}: expected {operation} to publish {expected:?}, got {uri:?}"
                );
                let (authority, path) = uri
                    .split_once('/')
                    .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no path"));
                assert_public_registry_name(&case, authority);
                assert_eq!(
                    authority, served.published,
                    "case {case:?}: expected {operation} to publish the registry name of the \
                     zone this emulator is served in, got {authority:?}"
                );
                // The path is the alias that says which public registry holds the repository,
                // then the repository's name exactly as the request spelled it — a
                // slash-nested name included.
                assert_eq!(
                    path,
                    format!("{}/{name}", registry_alias()),
                    "case {case:?}: expected {operation} to publish the alias-scoped \
                     repository path, got {path:?}"
                );
                assert!(
                    !uri.contains(REGISTRY_ID) && !uri.contains(SIGNED_REGION),
                    "case {case:?}: the account is reported in registryId and a public \
                     registry is global, so neither belongs in {uri:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_published_uri_does_not_move_with_the_socket_this_emulator_is_bound_on() {
    // What the port in the published value cost a client: it is ephemeral unless an operator
    // chose it, so a URI carrying it would move on every boot and nothing derived from it
    // would resolve twice. The rows are one emulator per socket of the same host — ports a
    // caller cannot predict included — each asked on a socket of its own as well, and every
    // one of them publishes the identical pair of URIs.
    let name = "stable/across-boots";
    let expected = (
        format!("{PUBLISHED_AUTHORITY}/{}/{name}", registry_alias()),
        format!("{PUBLISHED_AUTHORITY}/{}", registry_alias()),
    );
    for base in [
        "https://127.0.0.1:9297",
        "https://127.0.0.1:35093",
        "https://127.0.0.1:34427",
        "https://localhost:40831",
        "https://[::1]:44001",
    ] {
        let (router, _engine) = emulator_served_at(Some(base));
        let created = call_addressed(
            &router,
            base,
            Some("127.0.0.1:35093"),
            None,
            "CreateRepository",
            json!({"repositoryName": name}),
        )
        .await;
        let repository_uri = string_at(
            base,
            &created["repository"]["repositoryUri"],
            "repository.repositoryUri",
        );
        let registries = call_unaddressed(&router, base, "DescribeRegistries", json!({})).await;
        let registry_uri = string_at(
            base,
            &registries["registries"][0]["registryUri"],
            "registries[0].registryUri",
        );
        assert_eq!(
            (repository_uri, registry_uri),
            expected,
            "case {base:?}: expected the same published URIs whatever socket this emulator is \
             served on"
        );
    }
}

#[tokio::test]
async fn a_reference_composed_on_a_published_uri_names_this_gallery_in_every_zone() {
    // The reading side of the rule, over every zone this emulator can be served in: a client
    // pushes to the URI the API handed it and a container runtime pulls the reference it
    // composed on that URI, so the name this registry publishes and the name it answers for
    // are the same one. The rows cross the served address with the reference forms a runtime
    // writes (a tag, the implicit tag, and a digest).
    let name = "team/app";
    for served in served_addresses() {
        let (router, engine) = emulator_served_at(served.base);
        let created = call_unaddressed(
            &router,
            served.case,
            "CreateRepository",
            json!({"repositoryName": name}),
        )
        .await;
        let published = string_at(
            served.case,
            &created["repository"]["repositoryUri"],
            "repository.repositoryUri",
        );
        let pushed = call_unaddressed(
            &router,
            served.case,
            "PutImage",
            json!({
                "repositoryName": name,
                "imageManifest": json!({"schemaVersion": 2, "layers": []}).to_string(),
                "imageTag": "latest",
            }),
        )
        .await;
        let digest = string_at(
            served.case,
            &pushed["image"]["imageId"]["imageDigest"],
            "image.imageId.imageDigest",
        );

        for reference in [
            format!("{published}:latest"),
            published.clone(),
            format!("{published}@{digest}"),
        ] {
            let case = format!("{}, reference {reference}", served.case);
            let parsed = gallery_pull::parse_gallery_reference(&reference).unwrap_or_else(|| {
                panic!("case {case:?}: expected a reference into this gallery")
            });
            assert_eq!(
                parsed.registry_alias,
                registry_alias(),
                "case {case:?}: expected the alias the URI was published with, got {:?}",
                parsed.registry_alias
            );
            assert_eq!(
                parsed.repository_name, name,
                "case {case:?}: expected the repository the URI names, got {:?}",
                parsed.repository_name
            );
            assert!(
                gallery_pull::serves_registry_alias(&*engine, &parsed.registry_alias),
                "case {case:?}: expected this registry to serve the alias it published"
            );
            let answered = gallery_pull::serve_pull(&*engine, &parsed);
            assert_eq!(
                answered,
                Ok(Some(digest.clone())),
                "case {case:?}: expected the pushed manifest digest, got {answered:?}"
            );
        }
    }
}
