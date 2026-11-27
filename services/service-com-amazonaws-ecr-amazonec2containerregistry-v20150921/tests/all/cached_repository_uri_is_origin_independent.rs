// SPDX-License-Identifier: CC0-1.0

//! The address a repository held under a PULL THROUGH CACHE rule's prefix is published at,
//! across every path such a repository comes to EXIST by.
//!
//! The rule under test: **`Repository$repositoryUri` is composed off the repository's own
//! stored record and the endpoint this emulator answers the caller's registry requests at —
//! that endpoint, followed by `/` and the repository's name exactly as it was submitted — so
//! nothing about how the repository came to exist and nothing about the cache rule covering
//! its name enters the value.** "The URI for the repository. You can use this URI for
//! container image `push` and `pull` operations"
//!
//! ```text
//! smithy_query(query="doc",shape="Repository",member="repositoryUri",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
//! ```
//!
//! is a value a container client splits on the first `/`: it DIALS the authority ahead of that
//! `/` and asks for the repository the path names, so that authority has to be an address a
//! connection opened to it reaches. The registry HOLDING the repository is read beside the
//! address instead — the account `registryId` reports and the Region the `repositoryArn`
//! names — which is how a consumer derives a task definition's `image`, a function's
//! `ImageUri` or a `docker login` target from it, and both readings come off the same stored
//! record.
//!
//! What this suite adds to its siblings — `crate::repository_uri_registry_layout`, which reads
//! the label layout off every operation that reports a `Repository`;
//! `crate::cached_repository_registry_address`, which reads the mirror the registry itself
//! minted; and `crate::repository_registry_uri`, which crosses the endpoint a call arrived on
//! with the Region and the name — is the ORIGIN of the repository as a dimension of its own.
//! "A pull through cache rule provides a way to cache images from an upstream registry source
//! in your Amazon ECR private registry" (`CreatePullThroughCacheRule`) says what such a rule
//! DOES, and nothing about it is part of a repository's address: so a name under a rule's
//! prefix is published identically whether the registry minted the repository on the first
//! covered read, a client created it with the rule already in force, a client created it with
//! no rule at all, or the rule was registered after the repository existed.
//!
//! The class the rule generalises over is therefore that origin, crossed with the Region
//! holding the repository (four Regions, of four partitions, none of them the Region this
//! crate's harness signs for by default) and with how deep the rule's prefix and the path
//! under it are. Every assertion names its case and reads the identity it compares against
//! off the SAME response — `registryId` and the Region the `repositoryArn` names — so a value
//! that took its account, its Region or its path from anywhere else fails these rows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    at, emulator, ok_in_region, published_repository_uri, registry_host_in, REGISTRY_ID,
};

/// The upstream the rules below cache from, and the `UpstreamRegistry` that URL belongs to:
///
/// ```text
/// smithy_query(query="operation",operation="CreatePullThroughCacheRule",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const UPSTREAM_URL: &str = "public.ecr.aws";
const UPSTREAM_REGISTRY: &str = "ecr-public";

/// How the repository under the rule's prefix came to EXIST — the dimension this suite adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// The registry minted it itself, on the first read covered by the rule.
    MintedOnTheFirstCoveredRead,
    /// A client created it by name, with the rule already registered.
    CreatedUnderTheRule,
    /// A client created it by name, with no cache rule in the registry at all.
    CreatedWithNoRule,
    /// A client created it by name, and the rule covering that name was registered afterwards.
    RuleRegisteredAfterwards,
}

impl Origin {
    /// The case words for this origin.
    fn describe(self) -> &'static str {
        match self {
            Origin::MintedOnTheFirstCoveredRead => "minted by the registry on the first read",
            Origin::CreatedUnderTheRule => "created by name under the rule",
            Origin::CreatedWithNoRule => "created by name with no rule at all",
            Origin::RuleRegisteredAfterwards => "created by name before the rule existed",
        }
    }

    /// Every origin, so a row crossing them cannot silently drop one.
    const ALL: [Origin; 4] = [
        Origin::MintedOnTheFirstCoveredRead,
        Origin::CreatedUnderTheRule,
        Origin::CreatedWithNoRule,
        Origin::RuleRegisteredAfterwards,
    ];
}

/// One repository under a cache rule's prefix: the Region its registry holds it in, the rule's
/// `ecrRepositoryPrefix`, the path under that prefix, and the tag a covered read asks for.
#[derive(Clone, Copy)]
struct Cached {
    case: &'static str,
    region: &'static str,
    prefix: &'static str,
    path: &'static str,
    tag: &'static str,
}

impl Cached {
    /// The repository's own name: the rule's prefix followed by the path under it, which is
    /// the part of the URI a reader must get back unchanged.
    fn repository(&self) -> String {
        format!("{}/{}", self.prefix, self.path)
    }
}

/// The repositories the rows drive: the Region (four partitions), crossed with a plain and a
/// nested prefix and a one-, two- and three-level path under it.
const CACHED: &[Cached] = &[
    Cached {
        case: "a three-level path under a plain prefix, in a commercial Region",
        region: "us-west-2",
        prefix: "ptc-mirror",
        path: "docker/library/busybox",
        tag: "latest",
    },
    Cached {
        case: "a one-level path under a nested prefix, in another commercial Region",
        region: "ap-southeast-2",
        prefix: "team/mirror",
        path: "pause",
        tag: "3.9",
    },
    Cached {
        case: "a two-level path in a GovCloud Region",
        region: "us-gov-east-1",
        prefix: "ptc-mirror",
        path: "library/alpine",
        tag: "3.20",
    },
    Cached {
        case: "a two-level path in a partition with a DNS suffix of its own",
        region: "cn-north-1",
        prefix: "ptc-mirror",
        path: "library/hello-world",
        tag: "1.0.0",
    },
];

/// Register the row's rule.
async fn given_rule(router: &MultiServiceRouter, case: &str, row: &Cached) {
    ok_in_region(
        router,
        case,
        row.region,
        "CreatePullThroughCacheRule",
        &json!({
            "ecrRepositoryPrefix": row.prefix,
            "upstreamRegistry": UPSTREAM_REGISTRY,
            "upstreamRegistryUrl": UPSTREAM_URL,
        }),
    )
    .await;
}

/// Make the covered read that MINTS the repository, and assert the reference was served by
/// caching it — so the row that drives this origin really drove it.
async fn given_minted_repository(router: &MultiServiceRouter, case: &str, row: &Cached) {
    let read = ok_in_region(
        router,
        case,
        row.region,
        "BatchGetImage",
        &json!({
            "repositoryName": row.repository(),
            "imageIds": [{"imageTag": row.tag}],
        }),
    )
    .await;
    assert_eq!(
        at(&read, "/failures"),
        json!([]),
        "case {case:?}: expected the covered reference to be served by caching it, got {read}"
    );
}

/// `CreateRepository` for the row's name, and the AUTHORITY of the `repositoryUri` that
/// create reported.
async fn given_created_repository(
    router: &MultiServiceRouter,
    case: &str,
    row: &Cached,
) -> String {
    let created = ok_in_region(
        router,
        case,
        row.region,
        "CreateRepository",
        &json!({"repositoryName": row.repository()}),
    )
    .await;
    assert_published_uri(case, &created, "/repository", row)
}

/// Bring the repository into existence the way `origin` says, and answer the authority the
/// operation that created it published, where there was one to report.
async fn given_repository(
    router: &MultiServiceRouter,
    case: &str,
    row: &Cached,
    origin: Origin,
) -> Option<String> {
    match origin {
        Origin::MintedOnTheFirstCoveredRead => {
            given_rule(router, case, row).await;
            given_minted_repository(router, case, row).await;
            None
        }
        Origin::CreatedUnderTheRule => {
            given_rule(router, case, row).await;
            Some(given_created_repository(router, case, row).await)
        }
        Origin::CreatedWithNoRule => Some(given_created_repository(router, case, row).await),
        Origin::RuleRegisteredAfterwards => {
            let reported = given_created_repository(router, case, row).await;
            given_rule(router, case, row).await;
            Some(reported)
        }
    }
}

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn text(case: &str, body: &Value, pointer: &str) -> String {
    at(body, pointer)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// Assert that the `Repository` at `pointer` in `body` is published at the endpoint this
/// emulator answers the caller at, and answer the authority it was published at.
///
/// The identity the value is reported beside is read off the SAME response — `registryId`,
/// and the Region the `repositoryArn` names — so nothing of it is taken from the reader's own
/// endpoint or from this test's own idea of where the repository lives.
fn assert_published_uri(case: &str, body: &Value, pointer: &str, row: &Cached) -> String {
    let repository = row.repository();
    let uri = text(case, body, &format!("{pointer}/repositoryUri"));
    let registry_id = text(case, body, &format!("{pointer}/registryId"));
    let arn = text(case, body, &format!("{pointer}/repositoryArn"));
    let region = arn
        .split(':')
        .nth(3)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} names no Region"))
        .to_string();

    // The value, in full: the endpoint this emulator answers the caller's registry requests
    // at, and the repository's own name as the path.
    let expected = published_repository_uri(&repository);
    assert_eq!(
        uri, expected,
        "case {case:?}: expected {expected:?}, got {uri:?}"
    );
    assert_eq!(
        (registry_id.as_str(), region.as_str()),
        (REGISTRY_ID, row.region),
        "case {case:?}: expected the repository to be held by this account's registry in \
         {:?}, got {registry_id:?} in {region:?}",
        row.region
    );

    // The two halves a container client splits the value into.
    let (authority, path) = uri
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} names no repository path"));
    assert_eq!(
        path, repository,
        "case {case:?}: expected the path of {uri:?} to be the repository name \
         {repository:?}, got {path:?}"
    );
    // An address this emulator answers on rather than the host AWS serves this registry at,
    // which resolves nowhere near this process.
    assert_ne!(
        authority,
        registry_host_in(row.region),
        "case {case:?}: expected an address this emulator answers on for {:?}, got the host AWS \
         serves this registry at",
        row.region
    );

    authority.to_string()
}

/// `DescribeRepositories` for the row's repository, read in the Region holding it.
async fn described(router: &MultiServiceRouter, case: &str, row: &Cached) -> Value {
    ok_in_region(
        router,
        case,
        row.region,
        "DescribeRepositories",
        &json!({"repositoryNames": [row.repository()]}),
    )
    .await
}

/// RULE: a repository under a pull-through-cache rule's prefix is published at the endpoint
/// this emulator answers the caller's registry requests at, with its own name as the path —
/// whichever path brought the repository into existence, and whether or not a rule covers its
/// name.
#[tokio::test]
async fn a_cached_repositorys_uri_names_its_registry_however_the_repository_came_to_exist() {
    for row in CACHED {
        // The one address every origin in this Region has to publish, taken from the first
        // origin's own answer rather than composed here, so the rows are compared against each
        // other as well as against the identity each response reports.
        let mut published: Option<(Origin, String)> = None;

        for origin in Origin::ALL {
            let case = format!("{}, {}", row.case, origin.describe());
            // One emulator per origin: each row is the whole history of a registry, and the
            // origins differ in exactly that history.
            let router = emulator();

            let on_create = given_repository(&router, &case, row, origin).await;

            // The read a client discovers the push and pull target through.
            let read = described(&router, &case, row).await;
            let authority = assert_published_uri(&case, &read, "/repositories/0", row);
            if let Some(reported) = &on_create {
                assert_eq!(
                    reported, &authority,
                    "case {case:?}: expected the read to publish the authority the create did, \
                     {reported:?}, got {authority:?}"
                );
            }

            // The delete echoes the record it removed, which is the same record again.
            let deleted = ok_in_region(
                &router,
                &case,
                row.region,
                "DeleteRepository",
                &json!({"repositoryName": row.repository(), "force": true}),
            )
            .await;
            let echoed = assert_published_uri(&case, &deleted, "/repository", row);
            assert_eq!(
                echoed, authority,
                "case {case:?}: expected the delete to echo {authority:?}, got {echoed:?}"
            );

            match &published {
                Some((first, expected)) => assert_eq!(
                    &authority, expected,
                    "case {case:?}: expected the address published for the repository {:?} \
                     ({}), got {authority:?}",
                    expected,
                    first.describe()
                ),
                None => published = Some((origin, authority)),
            }
        }
    }
}
