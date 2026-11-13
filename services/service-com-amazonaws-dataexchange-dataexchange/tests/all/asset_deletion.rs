// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteAsset`, spanning the deletion catalog in breadth.
//!
//! `asset_updates.rs` pins the two writes of the asset route against one and three
//! assets; this suite is the wider table beside it — positions in the import order,
//! names that collide, sibling revisions and data sets, several `AssetType` values, and
//! every way an identifier can fail to address an asset.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `call`, `create_data_set`, `is_modelled_id`) is
//! the one `data_sets.rs` already uses, with `create_revision` from `revisions.rs`,
//! `store_assets` from `revision_assets.rs`, `get_asset` from `asset_lookup.rs` and
//! `delete_asset` from `asset_updates.rs`.
//!
//! Three rules are under test.
//!
//! 1. An asset belongs to exactly one revision of one data set, so `DeleteAsset`
//!    removes the ONE asset the route addresses from that revision's stored assets and
//!    answers 204 with no body; every other asset — of that revision, of the data
//!    set's other revisions and of other data sets — is untouched, and the readers
//!    (`GetAsset` and `ListRevisionAssets`) report exactly what is left.
//! 2. A revision its owner has finalized is read-only, so removing an asset from one
//!    is refused with the modelled `ConflictException` (409) and changes nothing: the
//!    asset is still stored and still listed. The revision's state is judged before the
//!    asset identifier is resolved, so the refusal of a read-only revision does not
//!    depend on which asset of it the route names.
//! 3. An identifier that addresses nothing stored — the data set, the revision of that
//!    data set, or the asset of that revision — is the modelled
//!    `ResourceNotFoundException` (404) naming which kind was absent, so a delete is
//!    not idempotent: the second delete of one asset is that 404. An identifier outside
//!    the modelled `Id` pattern is the modelled `ValidationException` (400) in any of
//!    the three label positions.
//!
//! The tables span what the rules generalise over: how many assets the revision holds
//! (one, a few, more), which position in the import order is addressed (only, first,
//! middle, last), assets sharing a `Name` (so the removal is shown to be by id),
//! several `AssetType` values, whether the revision is finalized, which named resource
//! is absent, and every way an id can break the published pattern.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lookup::get_asset;
use crate::asset_updates::{delete_asset, finalize_revision};
use crate::data_sets::{call, create_data_set, emulator, harness, is_modelled_id};
use crate::revision_assets::{entries, list_assets, store_assets, stored_asset_ids};
use crate::revisions::create_revision;

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` (32 characters)
/// but never created.
const NEVER_CREATED: &str = "ffffffffffffffffffffffffffffffff";

/// The whole `Message` every `ResourceNotFoundException` of this service carries: the
/// identity of the missing resource travels in the exception's own `ResourceId` and
/// `ResourceType` members. Stated here independently of the constant the crate
/// implements it with.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// The model's `ResourceType` values the refusals below report, from the
/// `ResourceType` enum (`DATA_SET | REVISION | ASSET | JOB | EVENT_ACTION |
/// DATA_GRANT`). Source: `smithy_query(query="shape", shape="ResourceType")`.
const RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";
const RESOURCE_TYPE_REVISION: &str = "REVISION";
const RESOURCE_TYPE_ASSET: &str = "ASSET";

/// The whole `Message` AWS answers a call that would change a finalized revision with,
/// recorded in `dataexchange.delete-asset-after-finalize-rejected`. It is fixed text:
/// the identity of the revision travels in the exception's own `ResourceId` and
/// `ResourceType` members. Stated here independently of the constant the crate
/// implements it with.
const NO_ACTION_ON_A_FINALIZED_REVISION: &str = "Cannot take action on a Finalized Revision.";

/// The ids `ListRevisionAssets` reports for a revision, in the order it pages them,
/// through the request shape and the `Assets` reader `revision_assets.rs` owns.
async fn listed_asset_ids(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
) -> Vec<String> {
    let listed = list_assets(router, data_set_id, revision_id, "?maxResults=200").await;
    assert_eq!(
        listed.status, 200,
        "case list {revision_id}: expected 200 from ListRevisionAssets, got {} body {:?}",
        listed.status, listed.body
    );
    entries(&listed)
        .iter()
        .map(|entry| {
            entry["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed asset carries no Id: {entry:?}"))
                .to_string()
        })
        .collect()
}

/// A data set of `asset_type` holding one revision that holds one asset per name,
/// handed back as (data set id, revision id, asset ids in import order).
async fn revision_holding_assets(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    asset_type: &str,
    names: &[&str],
) -> (String, String, Vec<String>) {
    let data_set = create_data_set(router, case, "asset deletion", asset_type, None).await;
    assert_eq!(
        data_set.status, 201,
        "case {case}: expected 201 from CreateDataSet, got {} body {:?}",
        data_set.status, data_set.body
    );
    let data_set_id = data_set.string("Id");
    let revision_id = create_revision(router, &data_set_id, Some("asset deletion"), None, case)
        .await
        .string("Id");
    let asset_ids = store_assets(engine, &data_set_id, &revision_id, names);
    (data_set_id, revision_id, asset_ids)
}

/// One row of the deletion catalog: the `AssetType` of the data set, the `Name` of each
/// asset the revision holds in import order, and which of them (by that position) the
/// call addresses.
struct DeleteCase {
    case: &'static str,
    asset_type: &'static str,
    names: &'static [&'static str],
    position: usize,
}

/// The rows differ along how many assets the revision holds, which position is
/// addressed (only / first / middle / last), whether two assets share a `Name`, and the
/// `AssetType` of the data set they belong to.
///
/// The asset types are values of the model's `AssetType` enum
/// (`smithy_query(query="shape", shape="AssetType")`).
fn delete_cases() -> &'static [DeleteCase] {
    &[
        DeleteCase {
            case: "the only asset of the revision",
            asset_type: "S3_SNAPSHOT",
            names: &["orders.csv"],
            position: 0,
        },
        DeleteCase {
            case: "the first of three",
            asset_type: "S3_SNAPSHOT",
            names: &["alpha.csv", "beta.csv", "gamma.csv"],
            position: 0,
        },
        DeleteCase {
            case: "the middle of five",
            asset_type: "S3_SNAPSHOT",
            names: &["a.csv", "b.csv", "c.csv", "d.csv", "e.csv"],
            position: 2,
        },
        DeleteCase {
            case: "the last of two, whose names do not sort in import order",
            asset_type: "S3_SNAPSHOT",
            names: &["zulu.csv", "alpha.csv"],
            position: 1,
        },
        DeleteCase {
            case: "one of two assets sharing a name",
            asset_type: "S3_SNAPSHOT",
            names: &["monthly.csv", "monthly.csv"],
            position: 0,
        },
        DeleteCase {
            case: "an asset that is an API",
            asset_type: "API_GATEWAY_API",
            names: &["orders", "payments"],
            position: 1,
        },
        DeleteCase {
            case: "an asset that is an S3 data access",
            asset_type: "S3_DATA_ACCESS",
            names: &["access"],
            position: 0,
        },
    ]
}

/// RULE: `DeleteAsset` removes the one asset the route addresses from the named
/// revision of the named data set and answers 204 with no body; afterwards the stored
/// record and the list reader both report exactly the assets that are left, and every
/// other revision keeps its own.
#[tokio::test]
async fn deleting_an_asset_removes_only_that_asset() {
    let (router, engine) = harness();

    for DeleteCase {
        case,
        asset_type,
        names,
        position,
    } in delete_cases()
    {
        let (data_set_id, revision_id, asset_ids) =
            revision_holding_assets(&router, &engine, case, asset_type, names).await;
        // A second revision of the same data set, holding its own assets: the delete
        // below is scoped to one revision, so these are untouched.
        let sibling_id = create_revision(&router, &data_set_id, None, None, case)
            .await
            .string("Id");
        let sibling_assets = store_assets(&engine, &data_set_id, &sibling_id, names);

        let target = asset_ids[*position].clone();

        // The single-asset reader addresses it before the delete, and reports the
        // identity the import minted.
        let before = get_asset(&router, &data_set_id, &revision_id, &target).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected 200 from GetAsset before the delete, got {} body {:?}",
            before.status, before.body
        );
        assert_eq!(
            before.member("Id"),
            &json!(target),
            "case {case}: expected GetAsset to report the addressed asset, got {:?}",
            before.member("Id")
        );
        assert_eq!(
            before.member("Name"),
            &json!(names[*position]),
            "case {case}: expected the stored Name, got {:?}",
            before.member("Name")
        );
        assert_eq!(
            before.member("AssetType"),
            &json!(asset_type),
            "case {case}: expected the data set's AssetType, got {:?}",
            before.member("AssetType")
        );
        assert_eq!(
            before.member("RevisionId"),
            &json!(revision_id),
            "case {case}: expected the asset's own RevisionId, got {:?}",
            before.member("RevisionId")
        );

        let deleted = delete_asset(&router, &data_set_id, &revision_id, &target).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteAsset, got {} body {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case}: expected no response body, got {:?}",
            deleted.body
        );

        // Exactly the addressed asset left state — the others keep their order, and an
        // asset that merely shares its `Name` is still there.
        let surviving: Vec<String> = asset_ids
            .iter()
            .filter(|id| **id != target)
            .cloned()
            .collect();
        assert_eq!(
            stored_asset_ids(&engine, &data_set_id, &revision_id),
            surviving,
            "case {case}: expected only the addressed asset to leave state"
        );

        // The list reader reports the survivors and nothing else.
        let mut listed = listed_asset_ids(&router, &data_set_id, &revision_id).await;
        listed.sort();
        let mut expected = surviving.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected ListRevisionAssets to report the surviving assets"
        );
        assert!(
            !listed.contains(&target),
            "case {case}: expected the deleted asset {target} to be gone from the listing, \
             got {listed:?}"
        );

        // The single-asset reader no longer addresses it, and still addresses each
        // survivor.
        let after = get_asset(&router, &data_set_id, &revision_id, &target).await;
        assert_eq!(
            after.status, 404,
            "case {case}: expected 404 from GetAsset after the delete, got {} body {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException from GetAsset, got {:?}",
            after.error
        );
        assert_eq!(
            after.member("Message"),
            &json!(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case}: expected the recorded not-found wording, got {:?}",
            after.member("Message")
        );
        assert_eq!(
            after.member("ResourceType"),
            &json!(RESOURCE_TYPE_ASSET),
            "case {case}: expected the 404 to name the asset, got {:?}",
            after.member("ResourceType")
        );
        for survivor in &surviving {
            let kept = get_asset(&router, &data_set_id, &revision_id, survivor).await;
            assert_eq!(
                kept.status, 200,
                "case {case}: expected the surviving asset {survivor} to stay readable, \
                 got {} body {:?}",
                kept.status, kept.body
            );
        }

        // The sibling revision kept every asset it held.
        let mut sibling_listed = listed_asset_ids(&router, &data_set_id, &sibling_id).await;
        sibling_listed.sort();
        let mut sibling_expected = sibling_assets.clone();
        sibling_expected.sort();
        assert_eq!(
            sibling_listed, sibling_expected,
            "case {case}: expected the sibling revision's assets to survive the delete"
        );

        // The revision itself is still there — only its asset went.
        let revision = call(
            &router,
            "GET",
            &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
            None,
        )
        .await;
        assert_eq!(
            revision.status, 200,
            "case {case}: expected the revision to survive the asset delete, got {} body {:?}",
            revision.status, revision.body
        );
    }
}

/// RULE: a revision its owner has finalized is read-only, so removing one of its assets
/// is refused with a 409 `ConflictException` carrying the fixed finalized-revision
/// sentence and naming the revision as the resource in conflict — and the revision's
/// assets are left exactly as they were.
#[tokio::test]
async fn deleting_an_asset_of_a_finalized_revision_conflicts() {
    let (router, engine) = harness();

    // Rows differ along how many assets the finalized revision holds, which of them is
    // addressed, and the `AssetType` of the data set: none of that makes a finalized
    // revision writable.
    let rows: &[(&str, &str, &[&str], usize)] = &[
        ("one asset", "S3_SNAPSHOT", &["only.csv"], 0),
        (
            "the first of three",
            "S3_SNAPSHOT",
            &["alpha.csv", "beta.csv", "gamma.csv"],
            0,
        ),
        (
            "the last of three",
            "S3_SNAPSHOT",
            &["alpha.csv", "beta.csv", "gamma.csv"],
            2,
        ),
        ("an asset that is an API", "API_GATEWAY_API", &["orders"], 0),
    ];

    for (case, asset_type, names, position) in rows {
        let (data_set_id, revision_id, asset_ids) =
            revision_holding_assets(&router, &engine, case, asset_type, names).await;
        finalize_revision(&router, &data_set_id, &revision_id).await;

        let target = asset_ids[*position].clone();
        let refused = delete_asset(&router, &data_set_id, &revision_id, &target).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409 deleting an asset of a finalized revision, got {} \
             body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.member("Message"),
            &json!(NO_ACTION_ON_A_FINALIZED_REVISION),
            "case {case}: expected the recorded refusal wording, got {:?}",
            refused.member("Message")
        );
        assert_eq!(
            refused.member("ResourceType"),
            &json!(RESOURCE_TYPE_REVISION),
            "case {case}: expected the conflict to name the revision, got {:?}",
            refused.member("ResourceType")
        );
        assert_eq!(
            refused.member("ResourceId"),
            &json!(revision_id),
            "case {case}: expected the conflict to carry the revision's id, got {:?}",
            refused.member("ResourceId")
        );

        // Nothing moved: every asset is still stored, in its order, and still listed.
        assert_eq!(
            stored_asset_ids(&engine, &data_set_id, &revision_id),
            asset_ids,
            "case {case}: expected the refused delete to change nothing in state"
        );
        let mut listed = listed_asset_ids(&router, &data_set_id, &revision_id).await;
        listed.sort();
        let mut expected = asset_ids.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected the refused delete to leave every asset listed"
        );
        let kept = get_asset(&router, &data_set_id, &revision_id, &target).await;
        assert_eq!(
            kept.status, 200,
            "case {case}: expected the refused delete to leave the asset readable, got {} \
             body {:?}",
            kept.status, kept.body
        );
        assert_eq!(
            kept.member("Id"),
            &json!(target),
            "case {case}: expected GetAsset to still report the asset, got {:?}",
            kept.member("Id")
        );

        // The refusal is about the revision's state, not about which asset the route
        // names: an asset id that addresses nothing is refused the same way, because the
        // read-only revision is judged before the asset identifier is resolved.
        let absent = delete_asset(&router, &data_set_id, &revision_id, NEVER_CREATED).await;
        assert_eq!(
            absent.status, 409,
            "case {case}: expected 409 for any asset id inside a finalized revision, got {} \
             body {:?}",
            absent.status, absent.body
        );
        assert_eq!(
            absent.member("ResourceType"),
            &json!(RESOURCE_TYPE_REVISION),
            "case {case}: expected the conflict to name the revision, got {:?}",
            absent.member("ResourceType")
        );
    }
}

/// RULE: a well-formed id that addresses nothing stored — the data set, the revision of
/// that data set, or the asset of that revision — is a 404 `ResourceNotFoundException`
/// whose `ResourceType` names which kind was absent and whose `ResourceId` is the id the
/// caller wrote; so a delete is not idempotent in state it no longer holds.
#[tokio::test]
async fn deleting_an_absent_asset_is_a_404() {
    let (router, engine) = harness();

    let (data_set_id, revision_id, asset_ids) = revision_holding_assets(
        &router,
        &engine,
        "absent-deletes",
        "S3_SNAPSHOT",
        &["orders.csv", "payments.csv"],
    )
    .await;
    let other_revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");
    let other_revision_assets =
        store_assets(&engine, &data_set_id, &other_revision_id, &["sibling.csv"]);
    let (other_data_set_id, foreign_revision_id, foreign_assets) = revision_holding_assets(
        &router,
        &engine,
        "other-deletes",
        "S3_SNAPSHOT",
        &["foreign.csv"],
    )
    .await;

    // Rows differ along WHICH named resource is absent, and along the way it is absent:
    // never created, or created but not under the pair the route names.
    let rows: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "unknown data set",
            NEVER_CREATED.to_string(),
            revision_id.clone(),
            asset_ids[0].clone(),
            RESOURCE_TYPE_DATA_SET,
            NEVER_CREATED.to_string(),
        ),
        (
            "known data set, unknown revision",
            data_set_id.clone(),
            NEVER_CREATED.to_string(),
            asset_ids[0].clone(),
            RESOURCE_TYPE_REVISION,
            NEVER_CREATED.to_string(),
        ),
        (
            "known data set, a revision of another data set",
            data_set_id.clone(),
            foreign_revision_id.clone(),
            foreign_assets[0].clone(),
            RESOURCE_TYPE_REVISION,
            foreign_revision_id.clone(),
        ),
        (
            "known pair, unknown asset",
            data_set_id.clone(),
            revision_id.clone(),
            NEVER_CREATED.to_string(),
            RESOURCE_TYPE_ASSET,
            NEVER_CREATED.to_string(),
        ),
        (
            "known pair, an asset of another revision of the same data set",
            data_set_id.clone(),
            revision_id.clone(),
            other_revision_assets[0].clone(),
            RESOURCE_TYPE_ASSET,
            other_revision_assets[0].clone(),
        ),
        (
            "known pair, an asset of another data set's revision",
            data_set_id.clone(),
            revision_id.clone(),
            foreign_assets[0].clone(),
            RESOURCE_TYPE_ASSET,
            foreign_assets[0].clone(),
        ),
        (
            "known pair, the revision id used as an asset id",
            data_set_id.clone(),
            revision_id.clone(),
            revision_id.clone(),
            RESOURCE_TYPE_ASSET,
            revision_id.clone(),
        ),
    ];

    for (case, data_set, revision, asset, resource_type, resource_id) in &rows {
        let answer = delete_asset(&router, data_set, revision, asset).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.member("ResourceType"),
            &json!(resource_type),
            "case {case}: expected ResourceType {resource_type}, got {:?}",
            answer.member("ResourceType")
        );
        assert_eq!(
            answer.member("ResourceId"),
            &json!(resource_id),
            "case {case}: expected the 404 to name the absent id, got {:?}",
            answer.member("ResourceId")
        );
    }

    // None of the refused calls removed anything, anywhere.
    assert_eq!(
        stored_asset_ids(&engine, &data_set_id, &revision_id),
        asset_ids,
        "case refused deletes: expected the addressed revision's assets to survive"
    );
    assert_eq!(
        stored_asset_ids(&engine, &data_set_id, &other_revision_id),
        other_revision_assets,
        "case refused deletes: expected the sibling revision's assets to survive"
    );
    assert_eq!(
        stored_asset_ids(&engine, &other_data_set_id, &foreign_revision_id),
        foreign_assets,
        "case refused deletes: expected the other data set's assets to survive"
    );

    // A delete removes the asset once; every call after that names nothing.
    let first = delete_asset(&router, &data_set_id, &revision_id, &asset_ids[0]).await;
    assert_eq!(
        first.status, 204,
        "case first delete: expected 204, got {} body {:?}",
        first.status, first.body
    );
    for round in 1..=2 {
        let case = format!("repeat delete {round}");
        let again = delete_asset(&router, &data_set_id, &revision_id, &asset_ids[0]).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected 404 for an asset already deleted, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.member("ResourceType"),
            &json!(RESOURCE_TYPE_ASSET),
            "case {case}: expected the 404 to name the asset, got {:?}",
            again.member("ResourceType")
        );
    }

    // Once the revision is gone, the revision is the absent resource; once the data set
    // is gone, the data set is.
    let revision_deleted = call(
        &router,
        "DELETE",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await;
    assert_eq!(
        revision_deleted.status, 204,
        "case delete the revision: expected 204 from DeleteRevision, got {} body {:?}",
        revision_deleted.status, revision_deleted.body
    );
    let after_revision = delete_asset(&router, &data_set_id, &revision_id, &asset_ids[1]).await;
    assert_eq!(
        (after_revision.status, after_revision.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case delete after the revision went: expected 404 ResourceNotFoundException, got {} \
         body {:?}",
        after_revision.status,
        after_revision.body
    );
    assert_eq!(
        after_revision.member("ResourceType"),
        &json!(RESOURCE_TYPE_REVISION),
        "case delete after the revision went: expected the revision to be the absent \
         resource, got {:?}",
        after_revision.member("ResourceType")
    );
    assert_eq!(
        after_revision.member("ResourceId"),
        &json!(revision_id),
        "case delete after the revision went: expected the 404 to name the revision, got {:?}",
        after_revision.member("ResourceId")
    );
    let data_set_deleted =
        call(&router, "DELETE", &format!("/v1/data-sets/{data_set_id}"), None).await;
    assert_eq!(
        data_set_deleted.status, 204,
        "case delete the data set: expected 204 from DeleteDataSet, got {} body {:?}",
        data_set_deleted.status, data_set_deleted.body
    );
    let after_data_set = delete_asset(&router, &data_set_id, &revision_id, &asset_ids[1]).await;
    assert_eq!(
        (after_data_set.status, after_data_set.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case delete after the data set went: expected 404 ResourceNotFoundException, got {} \
         body {:?}",
        after_data_set.status,
        after_data_set.body
    );
    assert_eq!(
        after_data_set.member("ResourceType"),
        &json!(RESOURCE_TYPE_DATA_SET),
        "case delete after the data set went: expected the data set to be the absent \
         resource, got {:?}",
        after_data_set.member("ResourceType")
    );
    assert_eq!(
        after_data_set.member("ResourceId"),
        &json!(data_set_id),
        "case delete after the data set went: expected the 404 to name the data set, got {:?}",
        after_data_set.member("ResourceId")
    );
}

/// RULE: an id outside the modelled `^[a-zA-Z0-9]{30,40}$` is a 400
/// `ValidationException` in any of the three label positions, not a 404; an id inside
/// the pattern is acceptable, so the answer is then about the resource.
#[tokio::test]
async fn a_malformed_id_in_any_position_is_a_400() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "malformed-deletes", "m", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = create_revision(&router, &data_set_id, None, None, "setup")
        .await
        .string("Id");

    // Rows differ along HOW the id breaks the pattern — too short, too long, a
    // disallowed character — plus both published bounds, which are well formed.
    let ids = [
        ("one short of the minimum length", "a".repeat(29)),
        ("at the minimum length", "a".repeat(30)),
        ("at the maximum length", "a".repeat(40)),
        ("one past the maximum length", "a".repeat(41)),
        (
            "a plus sign is not alphanumeric",
            format!("{}+{}", "a".repeat(16), "b".repeat(15)),
        ),
        ("a dash is not alphanumeric", format!("{}-1", "a".repeat(31))),
    ];

    for (how, id) in ids {
        for position in ["DataSetId", "RevisionId", "AssetId"] {
            let case = format!("{position} {how}");
            let answer = match position {
                "DataSetId" => delete_asset(&router, &id, &revision_id, NEVER_CREATED).await,
                "RevisionId" => delete_asset(&router, &data_set_id, &id, NEVER_CREATED).await,
                _ => delete_asset(&router, &data_set_id, &revision_id, &id).await,
            };
            if is_modelled_id(&id) {
                assert_eq!(
                    answer.status, 404,
                    "case {case}: expected 404 for a well-formed but unknown id {id:?}, \
                     got {} body {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400 for {id:?}, got {} body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("ValidationException"),
                    "case {case}: expected ValidationException for {id:?}, got {:?}",
                    answer.error
                );
            }
        }
    }
}
