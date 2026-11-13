// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAsset`, and for the `GetAsset` read and the
//! `DeleteAsset` write that stand beside it on the same asset route.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch` with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//!
//! Five rules are under test.
//!
//! 1. `UpdateAsset` writes the `Name` the request carries onto the stored asset the
//!    route addresses and answers out of that record: the asset's identity (`Id`,
//!    `Arn`, `DataSetId`, `RevisionId`), its `AssetType`, its `AssetDetails` and its
//!    `CreatedAt` are the stored ones, and every later read of the asset — `GetAsset`
//!    and `ListRevisionAssets` — reports the new name.
//! 2. `UpdatedAt` moves only when the record really changed, so a rename to the name
//!    the asset already carries leaves the whole record as it was, and renaming one
//!    asset leaves every other asset of the revision untouched.
//! 3. `DeleteAsset` removes the named asset and answers 204, so a following read of it
//!    is the modelled 404 while the revision's other assets stay.
//! 4. An action on a revision the model's `Finalized` documentation calls "this
//!    read-only state" cannot hold: it is refused with the modelled 409
//!    `ConflictException` naming the revision, and changes nothing.
//! 5. An identifier that addresses nothing stored is the modelled 404, reporting the
//!    identifier that missed and the kind of resource it named — on the read, the
//!    update and the delete alike.
//!
//! The rows span what the rules generalise over: both kinds of asset an import of this
//! emulator produces (a data object and an API), content that is empty, text and many
//! kilobytes, and new names of each form an asset name takes — a plain file name, an
//! Amazon S3 object key with prefixes, a name at the store's length bound, a non-ASCII
//! name, the name the asset already has — against revisions holding one asset and
//! several.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION};
use crate::job_lifecycle::{get_job, settle_past_the_execution_span, start_job};
use crate::jobs::create_job;
use crate::revision_finalization::import_assets;
use crate::signed_url_uploads::{base64_md5, data_set_with_revision, put_upload};

/// The `Type` of an import whose content a client uploads to an announced URL, and
/// whose asset is the uploaded object.
const SIGNED_URL_TYPE: &str = "IMPORT_ASSET_FROM_SIGNED_URL";

/// The `Type` of an import whose asset IS an Amazon API Gateway API.
const API_GATEWAY_TYPE: &str = "IMPORT_ASSET_FROM_API_GATEWAY_API";

/// Values of the model's `com.amazonaws.dataexchange#ResourceType` enum a lookup on
/// this route can report. Source: `smithy_query(query="shape", shape="ResourceType")` —
/// `ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`.
const DATA_SET: &str = "DATA_SET";
const REVISION: &str = "REVISION";
const ASSET: &str = "ASSET";

/// The whole `Message` AWS answers this error class with, stated here independently of
/// the constant the crate implements it with.
const NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// The whole `Message` AWS answers an action on a finalized revision with — fixed text
/// with no variable part, stated here independently of the constant the crate
/// implements it with. Source: the refusal recorded for this error class on this
/// service's asset surface.
const FINALIZED_ACTION_MESSAGE: &str = "Cannot take action on a Finalized Revision.";

/// A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) nothing is stored under.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// One asset to rename: which import creates it, the `AssetType` of the data set it
/// lands in, the `Name` the import gives it, the bytes a client uploads for it, and the
/// new `Name` the rename under test writes.
struct RenameCase {
    label: &'static str,
    job_type: &'static str,
    asset_type: &'static str,
    imported_name: &'static str,
    content: Vec<u8>,
    renamed_to: String,
}

/// Both kinds of asset an import of this emulator can produce, each renamed to a name
/// of a different form.
///
/// Source for the asset kinds: the two `RequestDetails` members that take their content
/// from an upload URL this service announces (`smithy_query(query="shape",
/// shape="ImportAssetFromSignedUrlRequestDetails,ImportAssetFromApiGatewayApiRequestDetails")`).
/// The new names span the forms the model's `AssetName` documentation describes — "the
/// Amazon S3 object key is used as the asset name", "the API name is used as the asset
/// name" — plus a long one and a non-ASCII one, because `AssetName` is a plain string
/// with no modelled pattern or length.
fn rename_cases() -> Vec<RenameCase> {
    vec![
        RenameCase {
            label: "a data object renamed to another file name",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            imported_name: "orders.csv",
            content: b"id,name\n1,ada\n2,grace\n".to_vec(),
            renamed_to: "orders-2024.csv".to_string(),
        },
        RenameCase {
            label: "an empty data object renamed to an object key with prefixes",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            imported_name: "empty.csv",
            content: Vec::new(),
            renamed_to: "exports/2024/06/orders.csv".to_string(),
        },
        RenameCase {
            label: "a data object of many kilobytes renamed to a name at the store's bound",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            imported_name: "feed.ndjson",
            content: b"{\"row\":\"value\"}\n".repeat(4_096),
            renamed_to: format!(
                "{}.ndjson",
                "n".repeat(dataexchange::state::MAX_NAME_CHARS - ".ndjson".len())
            ),
        },
        RenameCase {
            label: "a data object renamed to a non-ASCII name",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            imported_name: "data.csv",
            content: b"id\n1\n".to_vec(),
            renamed_to: "ventas-año-2024.csv".to_string(),
        },
        RenameCase {
            label: "an API asset renamed",
            job_type: API_GATEWAY_TYPE,
            asset_type: "API_GATEWAY_API",
            imported_name: "orders",
            content: b"{\"openapi\":\"3.0.1\"}".to_vec(),
            renamed_to: "orders-v2".to_string(),
        },
    ]
}

/// `GetAsset` on one asset of a revision.
async fn get_asset(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    asset_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets/{asset_id}"),
        None,
    )
    .await
}

/// `UpdateAsset` on one asset of a revision, with `name` as the whole body.
async fn update_asset(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    asset_id: &str,
    name: &str,
) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets/{asset_id}"),
        Some(json!({ "Name": name })),
    )
    .await
}

/// `DeleteAsset` on one asset of a revision.
///
/// `pub(crate)` so the suite that spans the deletion catalog in breadth
/// (`tests/all/asset_deletion.rs`) sends the same request shape.
pub(crate) async fn delete_asset(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    asset_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets/{asset_id}"),
        None,
    )
    .await
}

/// `ListRevisionAssets` on a revision.
async fn list_assets(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
) -> Vec<Value> {
    let answer = call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "ListRevisionAssets: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
        .member("Assets")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Each asset of the revision as `(Name, Id, UpdatedAt)`, read through
/// `ListRevisionAssets` — the other reader that has to agree with what a rename wrote.
async fn listed_assets(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
) -> Vec<(String, String, f64)> {
    list_assets(router, data_set_id, revision_id)
        .await
        .iter()
        .map(|entry| {
            (
                entry["Name"].as_str().unwrap_or_default().to_string(),
                entry["Id"].as_str().unwrap_or_default().to_string(),
                seconds(&entry["UpdatedAt"], "ListRevisionAssets"),
            )
        })
        .collect()
}

/// The `Id` of the one asset the revision holds, failing the case if it holds another
/// number of them.
async fn only_asset_id(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    case: &str,
) -> String {
    let listed = list_assets(router, data_set_id, revision_id).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case}: expected the revision to hold exactly one asset, got {listed:?}"
    );
    listed[0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the listed asset carries no Id: {listed:?}"))
        .to_string()
}

/// Import `case`'s asset into the revision through the calls a client makes: `CreateJob`
/// announces the upload URL, the client PUTs the content to it, `StartJob` sets the
/// import going and the clock settles the execution. Hands back the asset's `Id`.
async fn import_asset(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &RenameCase,
    data_set_id: &str,
    revision_id: &str,
) -> String {
    let label = case.label;
    let digest = base64_md5(&case.content);
    let (member, url_member, detail) = if case.job_type == SIGNED_URL_TYPE {
        (
            "ImportAssetFromSignedUrl",
            "SignedUrl",
            json!({
                "AssetName": case.imported_name,
                "DataSetId": data_set_id,
                "Md5Hash": digest,
                "RevisionId": revision_id,
            }),
        )
    } else {
        (
            "ImportAssetFromApiGatewayApi",
            "ApiSpecificationUploadUrl",
            json!({
                "ApiDescription": "an imported REST API",
                "ApiId": "a1b2c3d4e5",
                "ApiKey": "a-key",
                "ApiName": case.imported_name,
                "ApiSpecificationMd5Hash": digest,
                "DataSetId": data_set_id,
                "ProtocolType": "REST",
                "RevisionId": revision_id,
                "Stage": "prod",
            }),
        )
    };

    let created = create_job(router, case.job_type, json!({ member: detail }), None).await;
    assert_eq!(
        created.status, 201,
        "case {label}: expected 201 from CreateJob, got {} body {:?}",
        created.status, created.body
    );
    let job_id = created.string("Id");
    let announced = created.member("Details")[member][url_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {label}: expected {url_member} in the answered details, got {:?}",
                created.member("Details")
            )
        })
        .to_string();

    let uploaded = put_upload(router, &announced, &case.content).await;
    assert_eq!(
        uploaded, 200,
        "case {label}: expected 200 from the PUT to the announced URL, got {uploaded}"
    );
    let started = start_job(router, &job_id).await;
    assert_eq!(
        started.status, 202,
        "case {label}: expected 202 from StartJob, got {} body {:?}",
        started.status, started.body
    );
    settle_past_the_execution_span(engine);
    let settled = get_job(router, &job_id).await;
    assert_eq!(
        settled.string("State"),
        "COMPLETED",
        "case {label}: expected the import to complete, got {:?}",
        settled.body
    );

    only_asset_id(router, data_set_id, revision_id, label).await
}

/// Finalize the revision, the state the model's `Finalized` documentation calls
/// read-only.
///
/// `pub(crate)` so the suite that spans the deletion catalog
/// (`tests/all/asset_deletion.rs`) puts a revision into that state the same way.
pub(crate) async fn finalize_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
) {
    let answer = call(
        router,
        "PATCH",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        Some(json!({ "Finalized": true })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected 200 from UpdateRevision finalizing the revision, got {} body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member("Finalized"),
        &json!(true),
        "expected the revision to be finalized, got {:?}",
        answer.body
    );
}

/// RULE: `UpdateAsset` writes the requested `Name` onto the stored asset and answers out
/// of that record — the identity, `AssetType`, `AssetDetails` and `CreatedAt` the store
/// holds, with `UpdatedAt` at the write — so `GetAsset` and `ListRevisionAssets` both
/// report the new name afterwards.
#[tokio::test]
async fn renaming_an_asset_persists_and_is_answered_from_the_stored_record() {
    for case in rename_cases() {
        let (router, engine) = harness();
        let label = case.label;
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, label, case.asset_type).await;
        let asset_id = import_asset(&router, &engine, &case, &data_set_id, &revision_id).await;

        let before = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
        assert_eq!(
            before.status, 200,
            "case {label}: expected 200 from GetAsset on the imported asset, got {} body {:?}",
            before.status, before.body
        );
        assert_eq!(
            before.string("Name"),
            case.imported_name,
            "case {label}: expected the imported name before the rename, got {:?}",
            before.body
        );
        assert!(
            is_modelled_id(&before.string("Id")),
            "case {label}: expected a modelled Id, got {:?}",
            before.member("Id")
        );
        assert_eq!(
            before.string("Arn"),
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:\
                 data-sets/{data_set_id}/revisions/{revision_id}/assets/{asset_id}"
            ),
            "case {label}: expected the asset's minted ARN, got {:?}",
            before.member("Arn")
        );
        let created_at = seconds(before.member("CreatedAt"), label);
        let updated_before = seconds(before.member("UpdatedAt"), label);
        let details_before = before.member("AssetDetails").clone();

        // The rename's instant is bracketed by two instants this test observes, so the
        // assertion below is about WHEN the write happened rather than about how long
        // anything took. The asset's own `CreatedAt` cannot be that bracket: a settled
        // import is created at the instant its execution came due, which the harness
        // reaches by moving the store's clock rather than by waiting.
        let opened = dataexchange::state::now().as_secs_f64();
        let renamed = update_asset(
            &router,
            &data_set_id,
            &revision_id,
            &asset_id,
            &case.renamed_to,
        )
        .await;
        let closed = dataexchange::state::now().as_secs_f64();
        assert_eq!(
            renamed.status, 200,
            "case {label}: expected 200 from UpdateAsset, got {} body {:?}",
            renamed.status, renamed.body
        );
        assert_eq!(
            renamed.string("Name"),
            case.renamed_to,
            "case {label}: expected the new name in the answer, got {:?}",
            renamed.body
        );
        for (member, expected) in [
            ("Id", json!(asset_id)),
            ("Arn", before.member("Arn").clone()),
            ("AssetType", json!(case.asset_type)),
            ("DataSetId", json!(data_set_id)),
            ("RevisionId", json!(revision_id)),
            ("AssetDetails", details_before.clone()),
        ] {
            assert_eq!(
                renamed.member(member),
                &expected,
                "case {label}: expected {member} to be the stored {expected:?}, got {:?}",
                renamed.member(member)
            );
        }
        assert_eq!(
            seconds(renamed.member("CreatedAt"), label),
            created_at,
            "case {label}: expected CreatedAt to stay at the import instant, got {:?}",
            renamed.member("CreatedAt")
        );
        let updated_after = seconds(renamed.member("UpdatedAt"), label);
        assert!(
            updated_after >= opened && updated_after <= closed,
            "case {label}: expected UpdatedAt at the rename instant, in [{opened}, {closed}], \
             got {updated_after} (it was {updated_before} before the rename)"
        );
        assert!(
            renamed.member("SourceId").is_null(),
            "case {label}: expected no SourceId on an owned asset, got {:?}",
            renamed.member("SourceId")
        );

        // The read has to agree with the write, and so does the list.
        let after = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
        assert_eq!(
            after.status, 200,
            "case {label}: expected 200 from GetAsset after the rename, got {} body {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.string("Name"),
            case.renamed_to,
            "case {label}: expected the stored name to be the new one, got {:?}",
            after.body
        );
        assert_eq!(
            seconds(after.member("UpdatedAt"), label),
            updated_after,
            "case {label}: expected the stored UpdatedAt to be the one answered, got {:?}",
            after.member("UpdatedAt")
        );
        assert_eq!(
            after.member("AssetDetails"),
            &details_before,
            "case {label}: expected the rename to leave the details alone, got {:?}",
            after.member("AssetDetails")
        );
        let listed = listed_assets(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            listed,
            vec![(case.renamed_to.clone(), asset_id.clone(), updated_after)],
            "case {label}: expected ListRevisionAssets to report the renamed asset once"
        );
    }
}

/// RULE: `UpdatedAt` moves only when the record really changed, so renaming an asset to
/// the name it already carries leaves the whole stored record as it was.
#[tokio::test]
async fn renaming_an_asset_to_its_current_name_changes_nothing() {
    let (router, engine) = harness();
    let case = "a rename to the stored name";
    let (data_set_id, revision_id) = data_set_with_revision(&router, "same-name", "S3_SNAPSHOT").await;
    import_assets(&engine, &data_set_id, &revision_id, 1);
    let asset_id = only_asset_id(&router, &data_set_id, &revision_id, case).await;

    let before = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
    assert_eq!(
        before.status, 200,
        "case {case}: expected 200 from GetAsset, got {} body {:?}",
        before.status, before.body
    );
    let stored_name = before.string("Name");

    let again = update_asset(
        &router,
        &data_set_id,
        &revision_id,
        &asset_id,
        &stored_name,
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case {case}: expected 200 from UpdateAsset, got {} body {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.string("Name"),
        stored_name,
        "case {case}: expected the name to be unchanged, got {:?}",
        again.body
    );
    assert_eq!(
        seconds(again.member("UpdatedAt"), case),
        seconds(before.member("UpdatedAt"), case),
        "case {case}: expected UpdatedAt not to move, got {:?} against {:?}",
        again.member("UpdatedAt"),
        before.member("UpdatedAt")
    );
    assert_eq!(
        seconds(again.member("CreatedAt"), case),
        seconds(before.member("CreatedAt"), case),
        "case {case}: expected CreatedAt not to move, got {:?} against {:?}",
        again.member("CreatedAt"),
        before.member("CreatedAt")
    );
}

/// RULE: a rename addresses ONE asset, so every other asset of the revision keeps its
/// own name and its own `UpdatedAt`.
#[tokio::test]
async fn renaming_one_asset_leaves_the_other_assets_of_the_revision_alone() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "many-assets", "S3_SNAPSHOT").await;
    // Rows differ along WHICH of the revision's assets is renamed: the first listed,
    // one in the middle and the last listed.
    let count = 4;
    for position in [0usize, 2, count - 1] {
        let case = format!("renaming the asset at position {position} of {count}");
        import_assets(&engine, &data_set_id, &revision_id, count);
        let before = listed_assets(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            before.len(),
            count,
            "case {case}: expected {count} assets to start from, got {before:?}"
        );
        let (_, target_id, _) = before[position].clone();
        let new_name = format!("renamed-{position}.csv");

        // The rename's instant, bracketed by two instants this test observes.
        let opened = dataexchange::state::now().as_secs_f64();
        let renamed =
            update_asset(&router, &data_set_id, &revision_id, &target_id, &new_name).await;
        let closed = dataexchange::state::now().as_secs_f64();
        assert_eq!(
            renamed.status, 200,
            "case {case}: expected 200 from UpdateAsset, got {} body {:?}",
            renamed.status, renamed.body
        );

        let after = listed_assets(&router, &data_set_id, &revision_id).await;
        for (name, id, updated_at) in &before {
            let still = after
                .iter()
                .find(|(_, listed_id, _)| listed_id == id)
                .unwrap_or_else(|| panic!("case {case}: asset {id} is no longer listed: {after:?}"));
            let expected_name = if id == &target_id {
                new_name.clone()
            } else {
                name.clone()
            };
            assert_eq!(
                still.0, expected_name,
                "case {case}: expected asset {id} to be named {expected_name:?}, got {:?}",
                still.0
            );
            if id == &target_id {
                assert!(
                    still.2 >= opened && still.2 <= closed,
                    "case {case}: expected the renamed asset's UpdatedAt at the rename \
                     instant, in [{opened}, {closed}], got {} (it was {updated_at})",
                    still.2
                );
            } else {
                assert_eq!(
                    still.2, *updated_at,
                    "case {case}: expected asset {id} to keep its UpdatedAt, got {} against {updated_at}",
                    still.2
                );
            }
        }

        // Clear the revision so the next row starts from its own assets.
        clear_assets(&engine, &data_set_id, &revision_id);
    }
}

/// Empty the revision between rows, so each row's table is the one it wrote.
fn clear_assets(engine: &Arc<engine::InMemoryEngine>, data_set_id: &str, revision_id: &str) {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    store
        .revision_mut(data_set_id, revision_id)
        .expect("the revision the test just created")
        .assets
        .clear();
}

/// RULE: an action on a finalized revision — the state the model's `Finalized`
/// documentation calls read-only — is refused with the modelled 409 carrying the fixed
/// sentence for this error class and naming the revision, and changes nothing. The rows
/// are the two writes this surface admits: the rename and the delete.
#[tokio::test]
async fn acting_on_an_asset_of_a_finalized_revision_is_refused_and_changes_nothing() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "finalized", "S3_SNAPSHOT").await;
    import_assets(&engine, &data_set_id, &revision_id, 1);
    let asset_id = only_asset_id(&router, &data_set_id, &revision_id, "setup").await;
    let before = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
    let stored_name = before.string("Name");

    finalize_revision(&router, &data_set_id, &revision_id).await;

    for action in ["UpdateAsset", "DeleteAsset"] {
        let case = format!("{action} inside a finalized revision");
        let refused = if action == "UpdateAsset" {
            update_asset(
                &router,
                &data_set_id,
                &revision_id,
                &asset_id,
                "renamed-after-finalization.csv",
            )
            .await
        } else {
            delete_asset(&router, &data_set_id, &revision_id, &asset_id).await
        };
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.string("Message"),
            FINALIZED_ACTION_MESSAGE,
            "case {case}: expected the fixed refusal sentence, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.member("ResourceId").as_str(),
            Some(revision_id.as_str()),
            "case {case}: expected ResourceId to name the revision, got {:?}",
            refused.member("ResourceId")
        );
        assert_eq!(
            refused.member("ResourceType").as_str(),
            Some(REVISION),
            "case {case}: expected ResourceType {REVISION}, got {:?}",
            refused.member("ResourceType")
        );

        let after = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected the asset to still be there, got {} body {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.string("Name"),
            stored_name,
            "case {case}: expected the refusal to leave the name alone, got {:?}",
            after.body
        );
        assert_eq!(
            seconds(after.member("UpdatedAt"), &case),
            seconds(before.member("UpdatedAt"), &case),
            "case {case}: expected the refusal to leave UpdatedAt alone, got {:?}",
            after.member("UpdatedAt")
        );
        let listed = listed_assets(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case}: expected the revision to still hold its asset, got {listed:?}"
        );
    }
}

/// RULE: `DeleteAsset` removes the named asset from the revision and answers 204 with no
/// body, so every later read of it — `GetAsset` and `ListRevisionAssets` — reports it
/// gone, while the revision's other assets stay.
#[tokio::test]
async fn deleting_an_asset_removes_only_that_asset() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "deletions", "S3_SNAPSHOT").await;
    // Rows differ along WHICH asset of the revision is deleted: the only one, then one
    // of several.
    for held in [1usize, 3] {
        let case = format!("deleting one asset of {held}");
        clear_assets(&engine, &data_set_id, &revision_id);
        import_assets(&engine, &data_set_id, &revision_id, held);
        let before = listed_assets(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            before.len(),
            held,
            "case {case}: expected {held} assets to start from, got {before:?}"
        );
        let (_, target_id, _) = before[held - 1].clone();

        let deleted = delete_asset(&router, &data_set_id, &revision_id, &target_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteAsset, got {} body {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null(),
            "case {case}: expected no body on the 204, got {:?}",
            deleted.body
        );

        let gone = get_asset(&router, &data_set_id, &revision_id, &target_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case}: expected 404 reading the deleted asset, got {} body {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.string("Message"),
            NOT_FOUND_MESSAGE,
            "case {case}: expected the fixed not-found sentence, got {:?}",
            gone.body
        );
        assert_eq!(
            gone.member("ResourceType").as_str(),
            Some(ASSET),
            "case {case}: expected ResourceType {ASSET}, got {:?}",
            gone.member("ResourceType")
        );

        // A second delete of the same asset addresses nothing, and the assets that were
        // not named are all still there.
        let again = delete_asset(&router, &data_set_id, &revision_id, &target_id).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected 404 deleting the asset twice, got {} body {:?}",
            again.status, again.body
        );
        let after = listed_assets(&router, &data_set_id, &revision_id).await;
        let expected: Vec<(String, String, f64)> = before
            .iter()
            .filter(|(_, id, _)| id != &target_id)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case}: expected only the named asset to go"
        );
    }
}

/// RULE: `AssetName` is a caller-sized member the model bounds with neither a pattern
/// nor a length, so the store's own cap bounds it: a name at that cap is accepted and
/// one past it is refused with a 400 `ValidationException` naming the member, with the
/// stored record left as it was — and the resource is resolved first, so an id naming
/// nothing answers its 404 even when the body would also have been refused.
#[tokio::test]
async fn a_name_past_the_store_bound_is_refused() {
    use dataexchange::state::MAX_NAME_CHARS;

    let (router, engine) = harness();
    let case = "a name past the store's bound";
    let (data_set_id, revision_id) = data_set_with_revision(&router, "bounds", "S3_SNAPSHOT").await;
    import_assets(&engine, &data_set_id, &revision_id, 1);
    let asset_id = only_asset_id(&router, &data_set_id, &revision_id, case).await;

    // The bound itself is accepted.
    let at_bound = "n".repeat(MAX_NAME_CHARS);
    let accepted = update_asset(&router, &data_set_id, &revision_id, &asset_id, &at_bound).await;
    assert_eq!(
        accepted.status, 200,
        "case a name at the bound: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.string("Name"),
        at_bound,
        "case a name at the bound: expected the name to be written, got {:?}",
        accepted.member("Name")
    );

    // One character past it is a 400, and the stored record keeps the name it had.
    let past_bound = "n".repeat(MAX_NAME_CHARS + 1);
    let refused = update_asset(&router, &data_set_id, &revision_id, &asset_id, &past_bound).await;
    assert_eq!(
        refused.status, 400,
        "case {case}: expected 400, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case {case}: expected ValidationException, got {:?}",
        refused.error
    );
    assert!(
        refused.string("Message").contains("Name"),
        "case {case}: expected the refusal to name the member, got {:?}",
        refused.body
    );
    let after = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
    assert_eq!(
        after.string("Name"),
        at_bound,
        "case {case}: expected the refused rename to leave the stored name alone, got {:?}",
        after.body
    );

    // The asset is resolved before the body is judged.
    let missing = update_asset(
        &router,
        &data_set_id,
        &revision_id,
        ABSENT_ID,
        &past_bound,
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "case {case} on an id naming nothing: expected the resource's 404, got {} body {:?}",
        missing.status, missing.body
    );
}

/// RULE: an identifier that addresses nothing stored is the modelled 404 carrying the
/// fixed not-found sentence, the identifier that missed and the kind it named — on the
/// read, the update and the delete alike, for each of the three identifiers the route
/// carries.
#[tokio::test]
async fn an_identifier_that_addresses_nothing_is_a_404() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "absent", "S3_SNAPSHOT").await;
    import_assets(&engine, &data_set_id, &revision_id, 1);
    let asset_id = only_asset_id(&router, &data_set_id, &revision_id, "setup").await;

    // Rows differ along WHICH of the three identifiers names nothing.
    let rows: Vec<(&str, String, String, String, &str, String)> = vec![
        (
            "a data set that is not there",
            ABSENT_ID.to_string(),
            revision_id.clone(),
            asset_id.clone(),
            DATA_SET,
            ABSENT_ID.to_string(),
        ),
        (
            "a revision that is not there",
            data_set_id.clone(),
            ABSENT_ID.to_string(),
            asset_id.clone(),
            REVISION,
            ABSENT_ID.to_string(),
        ),
        (
            "an asset that is not there",
            data_set_id.clone(),
            revision_id.clone(),
            ABSENT_ID.to_string(),
            ASSET,
            ABSENT_ID.to_string(),
        ),
    ];

    for (how, data_set, revision, asset, resource_type, resource_id) in &rows {
        for operation in ["GetAsset", "UpdateAsset", "DeleteAsset"] {
            let case = format!("{operation} on {how}");
            let answer = match operation {
                "GetAsset" => get_asset(&router, data_set, revision, asset).await,
                "UpdateAsset" => {
                    update_asset(&router, data_set, revision, asset, "renamed.csv").await
                }
                _ => delete_asset(&router, data_set, revision, asset).await,
            };
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
                answer.string("Message"),
                NOT_FOUND_MESSAGE,
                "case {case}: expected the fixed refusal message, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("ResourceId").as_str(),
                Some(resource_id.as_str()),
                "case {case}: expected ResourceId to report the identifier that missed, got {:?}",
                answer.member("ResourceId")
            );
            assert_eq!(
                answer.member("ResourceType").as_str(),
                Some(*resource_type),
                "case {case}: expected ResourceType {resource_type}, got {:?}",
                answer.member("ResourceType")
            );
        }
    }

    // The asset that IS there still reads back, so the refusals above are about the
    // identifiers rather than about the store.
    let present = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
    assert_eq!(
        present.status, 200,
        "expected the stored asset to still read back, got {} body {:?}",
        present.status, present.body
    );
}
