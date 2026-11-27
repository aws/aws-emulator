// SPDX-License-Identifier: CC0-1.0

//! Which registry a reported repository belongs to, read off the response that carries its
//! published address.
//!
//! The rule under test: **an Amazon ECR registry is one account's in ONE Region, so the
//! identity a repository reports is the `(account, Region, name)` triple — carried by
//! `Repository$registryId`, the Region of `Repository$repositoryArn` and
//! `Repository$repositoryName` — and `Repository$repositoryUri` ("The URI for the repository.
//! You can use this URI for container image `push` and `pull` operations") is the ADDRESS
//! that repository is reached at: the endpoint this emulator answers that caller's registry
//! requests at, with the repository's own name as the path. Two registries holding one name
//! are therefore told apart by that identity, reported beside the address, while the address
//! itself is one endpoint serving every registry this emulator holds.**
//!
//! Where the sibling suite `crate::repository_registry_uri` asserts the composed value
//! against the authority this harness expects, this suite READS the reported value the way a
//! consumer of the member does — splitting it at the first `/` into the authority it dials and
//! the repository it asks for — and holds what it reads against the OTHER members of the same
//! response: the account `Repository$registryId` reports and the Region
//! `Repository$repositoryArn` names ("The ARN contains the `arn:aws:ecr` namespace, followed
//! by the region of the repository, Amazon Web Services account ID of the repository owner,
//! repository namespace, and repository name"). So a response that lost the Region of the
//! registry holding the repository — leaving one name in two Regions indistinguishable —
//! fails here however the address is spelled, and so does a path that is not the repository's
//! own name.
//!
//! The class the rule generalises over is the Region the repository is held in crossed with
//! the repository name, read back through every operation that reports a `Repository`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::wire::{at, call_in_region, emulator, ok_in_region, REGISTRY_ID};

/// The Regions this grid spans: five, of four partitions, and in no sorted order — the
/// registry a repository belongs to is the one its call was signed for, and its Region is
/// part of the repository's identity rather than a value any one constant holds. None of them
/// is a Region the sibling suites drive, so a value taken from somewhere other than the
/// addressed Region cannot pass this grid either.
const REGIONS: &[&str] = &[
    "eu-central-1",
    "ap-northeast-3",
    "us-gov-west-1",
    "cn-northwest-1",
    "sa-east-1",
];

/// The repository names this grid spans — a short one, the `.`/`-` separated form, the
/// doubled `_` separator and a namespaced name ("can be specified with namespace levels"),
/// which are the forms `RepositoryName`'s pattern admits
/// (`^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`, with
/// `@length(min: 2, max: 256)`):
///
/// ```text
/// smithy_query(query="shape",shape="RepositoryName",model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")
/// ```
const NAMES: &[&str] = &["svc", "group-b.service-2", "a__b", "org/team/svc"];

/// The error a create for a name the ADDRESSED registry already holds is refused with.
const ALREADY_EXISTS: &str = "RepositoryAlreadyExistsException";

/// What a reported repository IS, read the way a consumer of the member reads it: the address
/// it is published at, and the identity reported beside that address.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// The account `Repository$registryId` reports as the owner of the registry holding it.
    registry_id: String,
    /// The Region `Repository$repositoryArn` names, which is the registry holding it.
    region: String,
    /// The authority ahead of the first `/` of the published URI — the address a client
    /// dials, which says nothing about WHICH registry it is.
    authority: String,
    /// The repository the path after the first `/` asks for.
    repository: String,
}

/// Read `uri` as a container client does: split it at the first `/` — the authority it dials
/// ahead of that `/`, the repository it asks for after it.
fn published_address(case: &str, uri: &str) -> (String, String) {
    let (authority, repository) = uri.split_once('/').unwrap_or_else(|| {
        panic!("case {case:?}: expected <authority>/<repository>, got {uri:?}")
    });
    assert!(
        !authority.is_empty(),
        "case {case:?}: expected an address ahead of the path, got {uri:?}"
    );
    (authority.to_string(), repository.to_string())
}

/// The Region a repository ARN names: its fourth field, as the member documents the layout
/// ("the `arn:aws:ecr` namespace, followed by the region of the repository, Amazon Web
/// Services account ID of the repository owner, repository namespace, and repository name").
fn arn_region(case: &str, arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: expected a Region in the ARN, got {arn:?}"))
        .to_string()
}

/// What the `Repository` at `pointer` reports: the address it is published at, and the
/// identity members reported beside it in the same response — the account `registryId` names,
/// the Region the ARN names and the repository `repositoryName` is.
fn reported_address(case: &str, body: &Value, pointer: &str) -> Reported {
    let uri = at(body, &format!("{pointer}/repositoryUri"))
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositoryUri, got {body}"))
        .to_string();
    let (authority, repository) = published_address(case, &uri);

    let registry_id = at(body, &format!("{pointer}/registryId"));
    let registry_id = registry_id
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a registryId, got {body}"))
        .to_string();
    let arn = at(body, &format!("{pointer}/repositoryArn"));
    let arn = arn
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositoryArn, got {body}"));
    let name = at(body, &format!("{pointer}/repositoryName"));
    assert_eq!(
        Value::from(repository.as_str()),
        name,
        "case {case:?}: expected the path of {uri:?} to be the repository's own name, got \
         {name:?}"
    );
    Reported {
        registry_id,
        region: arn_region(case, arn),
        authority,
        repository,
    }
}

/// RULE: the identity a reported repository carries names the registry that holds it — the
/// owning account and the Region — so across a grid of Regions crossed with repository names
/// every identity is distinct, every repository is published at the one address this emulator
/// answers registry requests at, and both are the same whichever operation reports it.
#[tokio::test]
async fn one_name_in_many_registries_is_published_at_one_address_each() {
    let router = emulator();
    // Every identity this grid produced, so two registries reported as one are caught
    // however the response was spelled: the key is the identity, the value the row that
    // reported it.
    let mut published: BTreeMap<String, String> = BTreeMap::new();
    // Every address it published, which is one endpoint for the whole grid.
    let mut addresses: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for region in REGIONS {
        for name in NAMES {
            let case = format!("{name} in {region}");
            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let address = reported_address(&case, &created, "/repository");
            assert_eq!(
                (address.registry_id.as_str(), address.region.as_str()),
                (REGISTRY_ID, *region),
                "case {case:?}: expected the identity to name this account's registry in the \
                 Region addressed, got {address:?}"
            );
            assert_eq!(
                address.repository, *name,
                "case {case:?}: expected the repository's own name as the path, got {address:?}"
            );

            // A read of the repository through its own Region reports the very same address.
            let described = ok_in_region(
                &router,
                &case,
                region,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_eq!(
                reported_address(&case, &described, "/repositories/0"),
                address,
                "case {case:?}: expected the read to report the address the create did, got \
                 {described}"
            );

            addresses.insert(address.authority.clone());
            let identity = format!(
                "{}/{}/{}",
                address.registry_id, address.region, address.repository
            );
            if let Some(held_by) = published.insert(identity.clone(), case.clone()) {
                panic!(
                    "case {case:?}: expected an identity of its own, got {identity:?}, already \
                     reported for {held_by:?}"
                );
            }
        }
    }

    assert_eq!(
        published.len(),
        REGIONS.len() * NAMES.len(),
        "expected one identity per Region and name, got {published:?}"
    );
    // The distinctness above comes from the registry each repository belongs to rather than
    // from where it is served: one endpoint answers for every registry this emulator holds,
    // and the credential a `/v2` request presents says which registry it addresses.
    assert_eq!(
        addresses.len(),
        1,
        "expected every registry to publish the one endpoint this emulator answers registry \
         requests at, got {addresses:?}"
    );
}

/// RULE: the duplicate-name guard is per registry, so the create AWS refuses is the one
/// addressed to the Region that already holds the name — reported with the sentence AWS
/// words it in — while the same name in every other Region is a repository of its own,
/// reported with its own registry's identity.
#[tokio::test]
async fn a_name_one_registry_holds_is_published_elsewhere_under_its_own_registry() {
    for name in NAMES {
        let router = emulator();
        let (home, elsewhere) = REGIONS
            .split_first()
            .expect("the Region grid carries a home Region");
        let case = format!("{name} held in {home}");

        let created = ok_in_region(
            &router,
            &case,
            home,
            "CreateRepository",
            &json!({"repositoryName": name, "imageTagMutability": "IMMUTABLE"}),
        )
        .await;
        let home_address = reported_address(&case, &created, "/repository");

        // The same name against the SAME registry is the collision AWS refuses.
        let refused = call_in_region(
            &router,
            home,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a name this registry holds, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            ALREADY_EXISTS,
            "case {case:?}: expected {ALREADY_EXISTS}, got {:?}",
            refused.body
        );
        let expected_message = format!(
            "The repository with name '{name}' already exists in the registry with id \
             '{REGISTRY_ID}'"
        );
        assert_eq!(
            refused.message(),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            refused.message()
        );

        // Every OTHER Region's registry is free to hold the name, each publishing it at its
        // own address — and the delete that reports the repository back reports that address
        // too.
        for region in elsewhere {
            let case = format!("{name} also created in {region}");
            let created = ok_in_region(
                &router,
                &case,
                region,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            let address = reported_address(&case, &created, "/repository");
            assert_eq!(
                address.region, **region,
                "case {case:?}: expected the identity to name the Region holding it, got \
                 {address:?}"
            );
            assert_ne!(
                address, home_address,
                "case {case:?}: expected two registries holding one name to be reported as two \
                 repositories, got {address:?} for both"
            );

            let deleted = ok_in_region(
                &router,
                &case,
                region,
                "DeleteRepository",
                &json!({"repositoryName": name}),
            )
            .await;
            assert_eq!(
                reported_address(&case, &deleted, "/repository"),
                address,
                "case {case:?}: expected the delete to report the address the create did, got \
                 {deleted}"
            );
        }

        // The home registry's repository is untouched by any of that, settings and all.
        let described = ok_in_region(
            &router,
            &case,
            home,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;
        assert_eq!(
            reported_address(&case, &described, "/repositories/0"),
            home_address,
            "case {case:?}: expected the home registry to keep publishing its own address, \
             got {described}"
        );
        assert_eq!(
            at(&described, "/repositories/0/imageTagMutability"),
            json!("IMMUTABLE"),
            "case {case:?}: expected the refused create to leave the stored mutability alone, \
             got {described}"
        );
    }
}
