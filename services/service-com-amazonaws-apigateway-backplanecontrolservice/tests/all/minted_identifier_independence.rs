// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identifiers this control plane ASSIGNS: that each one is drawn
//! for the record it names alone, and that the record then keeps it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the request builder and the create helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle`, `documentation_parts` and `api_key_import`.
//!
//! The RULE under test: an id this service mints for a record it creates is an opaque handle
//! drawn independently for that record — a function neither of how many ids the process
//! minted before it nor of any other record — and the record is addressable by exactly that
//! id afterwards. The sibling `minted_identifier_widths` suite covers the WIDTH and the
//! alphabet an id is drawn from; this one covers where it comes from.
//!
//! The tables span what the rule generalises over:
//!
//! * the record KIND and the operation that mints its id — an account-wide record and one
//!   created inside a REST API, a create that mints one id, a create that mints two at once
//!   (the API and its root resource) and a bulk import that mints one per row;
//! * the POSITION in the creation sequence: several records of each kind in a row, so a
//!   value that moves with the creation count is exercised rather than sampled once;
//! * the PROCESS the id was drawn in: the same calls in the same order against two emulators
//!   that each start from an empty store;
//! * the character POSITION inside an id, over enough draws of each kind that any position
//!   held constant by the way the id is produced shows up.

use std::collections::BTreeMap;

use serde_json::json;

use crate::api_key_import::{import, imported_ids};
use crate::documentation_parts::content_map;
use crate::rest_api_lifecycle::{create_api, make_deployable, string_member, Gateway};

/// How many records of each kind a case mints when it compares two emulators: more than one,
/// so an id that is a function of the creation count collides on every draw rather than only
/// on the first.
const RECORDS_PER_KIND: usize = 4;

/// How many ids of one kind the character-position case draws. A position pinned to one
/// character by the way an id is produced — a counter rendered at a fixed width leaves its
/// high-order characters constant — is then certain to show, while a drawn id repeats a
/// character in every position of this many draws with probability `36 * 36^-12`.
const DRAWS_PER_POSITION_CASE: usize = 12;

/// Every kind of record whose minted id a case drives through the operation that creates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `CreateRestApi`, which mints TWO ids in one call: the API's own and the root
    /// resource created with it.
    RestApi,
    /// `CreateResource`, a record created inside one REST API.
    Resource,
    /// `CreateDeployment`, a record created inside one REST API.
    Deployment,
    /// `CreateDocumentationPart`, a record created inside one REST API.
    DocumentationPart,
    /// `CreateUsagePlan`, a record of the account.
    UsagePlan,
    /// `CreateApiKey`, a record of the account.
    ApiKey,
    /// `ImportApiKeys`, which mints one id per row of the API key file it reads.
    ImportedApiKeys,
}

/// The kinds the tables walk.
const KINDS: &[Kind] = &[
    Kind::RestApi,
    Kind::Resource,
    Kind::Deployment,
    Kind::DocumentationPart,
    Kind::UsagePlan,
    Kind::ApiKey,
    Kind::ImportedApiKeys,
];

/// One minted id, with the request that reads the record it names back.
#[derive(Debug, Clone)]
struct Minted {
    id: String,
    read: String,
}

/// A well-formed API key value for row `row` of round `index`: 21 characters, inside the
/// published 20..=128 range an API key value is accepted in, and distinct per row.
fn imported_value(index: usize, row: usize) -> String {
    format!("imported-api-key-{index:02}-{row}")
}

/// Create one record of `kind` in `api` and answer the ids that create minted, each with the
/// request that reads its record back.
async fn mint(
    gateway: &Gateway,
    api: &str,
    root: &str,
    kind: Kind,
    index: usize,
) -> Vec<Minted> {
    let case = format!("{kind:?} #{index}");
    match kind {
        Kind::RestApi => {
            let created = gateway
                .call("POST", "/restapis", Some(json!({ "name": format!("minted-{index}") })))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateRestApi, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            let root_id = string_member(&created.body, "rootResourceId", &case);
            vec![
                Minted {
                    read: format!("/restapis/{id}"),
                    id: id.clone(),
                },
                Minted {
                    read: format!("/restapis/{id}/resources/{root_id}"),
                    id: root_id,
                },
            ]
        }
        Kind::Resource => {
            let created = gateway
                .call(
                    "POST",
                    &format!("/restapis/{api}/resources/{root}"),
                    Some(json!({ "pathPart": format!("part{index}") })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateResource, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            vec![Minted {
                read: format!("/restapis/{api}/resources/{id}"),
                id,
            }]
        }
        Kind::Deployment => {
            let created = gateway
                .call(
                    "POST",
                    &format!("/restapis/{api}/deployments"),
                    Some(json!({ "description": format!("snapshot {index}") })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            vec![Minted {
                read: format!("/restapis/{api}/deployments/{id}"),
                id,
            }]
        }
        Kind::DocumentationPart => {
            // One location per round: a location already documented under this API is a
            // conflict, so each round documents another resource path.
            let created = gateway
                .call(
                    "POST",
                    &format!("/restapis/{api}/documentation/parts"),
                    Some(json!({
                        "location": { "type": "RESOURCE", "path": format!("/doc{index}") },
                        "properties": content_map(&format!("part {index}")),
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateDocumentationPart, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            vec![Minted {
                read: format!("/restapis/{api}/documentation/parts/{id}"),
                id,
            }]
        }
        Kind::UsagePlan => {
            let created = gateway
                .call("POST", "/usageplans", Some(json!({ "name": format!("plan-{index}") })))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateUsagePlan, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            vec![Minted {
                read: format!("/usageplans/{id}"),
                id,
            }]
        }
        Kind::ApiKey => {
            let created = gateway
                .call("POST", "/apikeys", Some(json!({ "name": format!("key-{index}") })))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateApiKey, got {} with {:?}",
                created.status, created.body
            );
            let id = string_member(&created.body, "id", &case);
            vec![Minted {
                read: format!("/apikeys/{id}"),
                id,
            }]
        }
        Kind::ImportedApiKeys => {
            // Two rows per import, so one call mints two ids: each row is its own record and
            // gets its own id.
            let document = format!(
                "key,name\n{},imported-{index}-1\n{},imported-{index}-2\n",
                imported_value(index, 1),
                imported_value(index, 2)
            );
            let answer = import(gateway, &document, true).await;
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201 from ImportApiKeys, got {} with {:?}",
                answer.status, answer.body
            );
            imported_ids(&answer.body, &case)
                .into_iter()
                .map(|id| Minted {
                    read: format!("/apikeys/{id}"),
                    id,
                })
                .collect()
        }
    }
}

/// Assert the record `minted` names is addressable by exactly the id the create reported.
async fn assert_reads_back(gateway: &Gateway, minted: &Minted, case: &str) {
    let read = gateway.call("GET", &minted.read, None).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GET {}, got {} with {:?}",
        minted.read, read.status, read.body
    );
    assert_eq!(
        string_member(&read.body, "id", case),
        minted.id,
        "case {case}: the read of GET {} must report the minted id {:?}, got {:?}",
        minted.read,
        minted.id,
        read.body
    );
}

/// An emulator with one REST API that can carry every record a case creates inside one, and
/// the API's id and root resource id.
async fn fresh_emulator() -> (Gateway, String, String) {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "minted-identifiers").await;
    // A deployment is the snapshot of the API's methods, so the API must carry one.
    make_deployable(&gateway, &api).await;
    (gateway, api, root)
}

/// The ids `rounds` creations of `kind` mint in a FRESH emulator, in creation order, each
/// asserted to read its own record back.
async fn minted_in_a_fresh_emulator(kind: Kind, rounds: usize) -> Vec<String> {
    let (gateway, api, root) = fresh_emulator().await;
    let mut ids: Vec<String> = Vec::new();
    for index in 0..rounds {
        let case = format!("{kind:?} #{index}");
        for minted in mint(&gateway, &api, &root, kind, index).await {
            assert_reads_back(&gateway, &minted, &case).await;
            ids.push(minted.id);
        }
    }
    ids
}

#[tokio::test]
async fn an_id_is_drawn_for_its_record_rather_than_from_the_creation_count() {
    for kind in KINDS {
        // The SAME calls in the SAME order against two emulators that each start from an
        // empty store. An id that is a function of how many ids the process minted before it
        // is the same sequence in both; an id drawn for its record is not.
        let first = minted_in_a_fresh_emulator(*kind, RECORDS_PER_KIND).await;
        let second = minted_in_a_fresh_emulator(*kind, RECORDS_PER_KIND).await;
        assert!(
            !first.is_empty() && first.len() == second.len(),
            "case {kind:?}: both runs must mint the same number of ids, got {first:?} and \
             {second:?}"
        );

        let mut distinct = first.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            first.len(),
            "case {kind:?}: no two records may share an id, got {first:?}"
        );

        let repeated: Vec<&String> = first.iter().filter(|id| second.contains(id)).collect();
        assert!(
            repeated.is_empty(),
            "case {kind:?}: the ids {repeated:?} were handed out by both runs, so the id is a \
             function of the creation sequence rather than of the record; got {first:?} and \
             {second:?}"
        );
    }
}

#[tokio::test]
async fn one_id_names_one_record_whatever_was_created_before_it() {
    // Every kind created in ONE emulator, interleaved round by round, so each kind's ids are
    // drawn with unrelated creations in between.
    let (gateway, api, root) = fresh_emulator().await;
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut records: Vec<(Minted, String)> = Vec::new();
    for index in 0..RECORDS_PER_KIND {
        for kind in KINDS {
            let case = format!("{kind:?} #{index}");
            for minted in mint(&gateway, &api, &root, *kind, index).await {
                assert_reads_back(&gateway, &minted, &case).await;
                if let Some(held_by) = owner.insert(minted.id.clone(), case.clone()) {
                    panic!(
                        "case {case}: the id {:?} was already handed to {held_by}, so one id \
                         names two records",
                        minted.id
                    );
                }
                records.push((minted, case.clone()));
            }
        }
    }

    // Each id still reads its OWN record back after every other record was created: the
    // binding survives every creation that followed it.
    for (minted, case) in &records {
        assert_reads_back(&gateway, minted, case).await;
    }
}

#[tokio::test]
async fn no_character_position_of_a_minted_id_is_a_constant() {
    // The kinds whose ids a client correlates a record by across calls, at both widths an id
    // is spelled at, drawn enough times that a position the production method holds constant
    // shows up as one that never varies.
    for kind in [
        Kind::Deployment,
        Kind::DocumentationPart,
        Kind::UsagePlan,
        Kind::ImportedApiKeys,
    ] {
        let ids = minted_in_a_fresh_emulator(kind, DRAWS_PER_POSITION_CASE).await;
        let width = ids
            .first()
            .map(|id| id.chars().count())
            .unwrap_or_else(|| panic!("case {kind:?}: expected minted ids, got none"));
        for id in &ids {
            assert_eq!(
                id.chars().count(),
                width,
                "case {kind:?}: every id of one kind is the same width, got {id:?} beside \
                 {ids:?}"
            );
        }
        for position in 0..width {
            let seen: std::collections::BTreeSet<Option<char>> =
                ids.iter().map(|id| id.chars().nth(position)).collect();
            assert!(
                seen.len() > 1,
                "case {kind:?}: character {position} of a minted id never varies across \
                 {ids:?}, so that position is a literal rather than drawn"
            );
        }
    }
}
