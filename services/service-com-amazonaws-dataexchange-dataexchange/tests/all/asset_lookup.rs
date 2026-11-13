// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAsset`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch` with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//!
//! Three rules are under test.
//!
//! 1. The answer is the ONE asset the route's `DataSetId`/`RevisionId`/`AssetId`
//!    address, composed from the stored record the import that created it left: the
//!    identity that import minted (`Id`, `Arn`, `Name`, `AssetType`, the ids of the
//!    revision and data set holding it, and the instants it was created and last
//!    updated at), and `AssetDetails` carrying the ONE detail member its `AssetType`
//!    names. Reading is reporting: a second read answers the same thing and the store
//!    is unchanged.
//! 2. The asset is addressed by its own `AssetId` WITHIN the named revision of the
//!    named data set, so an asset of another revision — or of another data set — is as
//!    absent on this route as one that was never imported.
//! 3. An identifier that addresses nothing is the modelled 404 reporting WHICH kind
//!    was missing (`ASSET`, `REVISION`, `DATA_SET`), and an identifier outside the
//!    modelled `Id` pattern is a 400 instead: the request is malformed, not the
//!    resource absent.
//!
//! The tables span what the rules generalise over: both `AssetType` values an import
//! of this emulator can produce, content that is empty, text and many kilobytes,
//! revisions holding one asset and several, an asset addressed through every wrong
//! revision and data set a store with two of each offers, and every way an id can
//! break the modelled pattern plus both of its published bounds.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{
    call, create_data_set, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION,
};
use crate::job_lifecycle::{get_job, settle_past_the_execution_span, start_job};
use crate::jobs::create_job;
use crate::revisions::create_revision;
use crate::signed_url_uploads::{base64_md5, data_set_with_revision, put_upload};

/// The `Type` of an import whose content a client uploads to an announced URL, and
/// whose asset is the uploaded object.
const SIGNED_URL_TYPE: &str = "IMPORT_ASSET_FROM_SIGNED_URL";

/// The `Type` of an import whose asset IS an Amazon API Gateway API.
const API_GATEWAY_TYPE: &str = "IMPORT_ASSET_FROM_API_GATEWAY_API";

/// Values of the model's `com.amazonaws.dataexchange#ResourceType` enum a lookup on
/// this route can report. Source: `smithy_query(query="shape", shape="ResourceType")`.
const ASSET: &str = "ASSET";
const REVISION: &str = "REVISION";
const DATA_SET: &str = "DATA_SET";

/// The whole `Message` AWS answers this error class with, stated here independently of
/// the constant the crate implements it with.
const NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// An identifier that is well formed per the modelled `Id` pattern
/// `^[a-zA-Z0-9]{30,40}$` but names nothing this emulator holds.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// One asset an import adds: how this file refers to it, which import `Type` creates
/// it, the `AssetType` of the data set it lands in, the `Name` the import gives it and
/// the bytes a client uploads for it.
struct ImportCase {
    label: &'static str,
    job_type: &'static str,
    asset_type: &'static str,
    asset_name: &'static str,
    content: Vec<u8>,
}

/// Both kinds of asset an import of this emulator can produce, against content of
/// each shape an upload can carry.
///
/// Source: the two `RequestDetails` members that take their content from an upload URL
/// this service announces (`smithy_query(query="shape",
/// shape="ImportAssetFromSignedUrlRequestDetails,ImportAssetFromApiGatewayApiRequestDetails")`),
/// paired with the `AssetType` of the data set each imports into; the rows then differ
/// along the content, because what a read reports about an asset is about the bytes it
/// holds whatever they are.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            label: "an asset of text",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            asset_name: "orders.csv",
            content: b"id,name\n1,ada\n2,grace\n".to_vec(),
        },
        ImportCase {
            label: "an empty asset",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            asset_name: "empty.csv",
            content: Vec::new(),
        },
        ImportCase {
            label: "an asset of many kilobytes",
            job_type: SIGNED_URL_TYPE,
            asset_type: "S3_SNAPSHOT",
            asset_name: "feed.ndjson",
            content: b"{\"row\":\"value\"}\n".repeat(4_096),
        },
        ImportCase {
            label: "an API specification",
            job_type: API_GATEWAY_TYPE,
            asset_type: "API_GATEWAY_API",
            asset_name: "orders",
            content: b"{\"openapi\":\"3.0.1\"}".to_vec(),
        },
    ]
}

/// `GetAsset` on one asset of one revision of one data set.
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

/// `ListRevisionAssets` on a revision — the reader that hands the test the `AssetId`
/// an import minted, which is how a client learns it.
async fn list_assets(router: &MultiServiceRouter, data_set_id: &str, revision_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets"),
        None,
    )
    .await
}

/// Import `case`'s asset into the revision through the calls a client makes:
/// `CreateJob` announces the upload URL, the client PUTs the content to it, `StartJob`
/// sets the import going and the clock settles the execution. Answers the `AssetId`
/// the import minted, read back through `ListRevisionAssets`.
async fn import_asset(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &ImportCase,
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
                "AssetName": case.asset_name,
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
                "ApiName": case.asset_name,
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

    let listed = list_assets(router, data_set_id, revision_id).await;
    let entries = listed.member("Assets").as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        1,
        "case {label}: expected the one imported asset in the revision, got {entries:?}"
    );
    entries[0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: expected the asset's Id, got {entries:?}"))
        .to_string()
}

/// Write one asset straight into a revision through the shared engine, the way
/// `revision_finalization.rs` writes a revision's assets, and answer its `AssetId`.
///
/// An asset is only ever created by an import job, and [`import_asset`] drives that
/// whole path; a table of several assets writes the record instead, which keeps the
/// table cheap AND is what shows the answer is composed from the stored record rather
/// than from the request.
fn store_asset(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
    name: &str,
    content: Vec<u8>,
) -> String {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    let data_set = store
        .data_sets
        .get_mut(data_set_id)
        .expect("the data set the test just created");
    let asset_type = data_set.asset_type.clone();
    let imported_at = dataexchange::state::now();
    let revision = data_set
        .revisions
        .get_mut(revision_id)
        .expect("the revision the test just created");
    let asset_id = dataexchange::state::mint_id();
    revision.assets.push(dataexchange::state::Asset {
        arn: dataexchange::state::asset_arn(REGION, data_set_id, revision_id, &asset_id),
        id: asset_id.clone(),
        name: name.to_string(),
        asset_type,
        content,
        api: None,
        s3_data_access: None,
        created_at: imported_at,
        updated_at: imported_at,
    });
    asset_id
}

/// How many assets the revision holds, read straight off the store — so a read can be
/// shown to have changed nothing.
fn stored_asset_count(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> usize {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .data_sets
        .get(data_set_id)
        .and_then(|data_set| data_set.revisions.get(revision_id))
        .map_or(0, |revision| revision.assets.len())
}

/// RULE: `GetAsset` answers the asset its route addresses out of the stored record —
/// the identity the import minted and `AssetDetails` carrying the one detail member the
/// asset's `AssetType` names — and reading it changes nothing.
#[tokio::test]
async fn an_imported_asset_reads_back_from_its_own_route() {
    for case in import_cases() {
        let (router, engine) = harness();
        let label = case.label;
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, label, case.asset_type).await;
        let asset_id = import_asset(&router, &engine, &case, &data_set_id, &revision_id).await;

        let answer = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        assert!(
            is_modelled_id(&asset_id),
            "case {label}: expected a minted Id matching the modelled pattern, got {asset_id:?}"
        );
        assert_eq!(
            answer.string("Id"),
            asset_id,
            "case {label}: expected the asset asked about, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("Name"),
            case.asset_name,
            "case {label}: expected the Name the import gave the asset, got {:?}",
            answer.member("Name")
        );
        assert_eq!(
            answer.string("AssetType"),
            case.asset_type,
            "case {label}: expected the data set's AssetType, got {:?}",
            answer.member("AssetType")
        );
        assert_eq!(
            answer.string("DataSetId"),
            data_set_id,
            "case {label}: expected the data set holding the asset, got {:?}",
            answer.member("DataSetId")
        );
        assert_eq!(
            answer.string("RevisionId"),
            revision_id,
            "case {label}: expected the revision holding the asset, got {:?}",
            answer.member("RevisionId")
        );
        assert_eq!(
            answer.string("Arn"),
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}\
                 /revisions/{revision_id}/assets/{asset_id}"
            ),
            "case {label}: expected the Arn built from the asset's ids, got {:?}",
            answer.member("Arn")
        );
        assert!(
            answer.body.get("SourceId").is_none(),
            "case {label}: expected no SourceId on an asset of an owned data set, got {:?}",
            answer.member("SourceId")
        );
        assert!(
            answer.body.get("Tags").is_none(),
            "case {label}: expected no Tags member on an asset carrying none, got {:?}",
            answer.member("Tags")
        );
        let created_at = seconds(answer.member("CreatedAt"), label);
        let updated_at = seconds(answer.member("UpdatedAt"), label);
        assert_eq!(
            created_at, updated_at,
            "case {label}: expected an untouched asset's instants to be equal, got \
             {created_at} and {updated_at}"
        );
        assert!(
            created_at > 0.0,
            "case {label}: expected the instant the import created the asset, got {created_at}"
        );

        // The details are the ONE member the asset's type names.
        let details = answer.member("AssetDetails").clone();
        if case.asset_type == "API_GATEWAY_API" {
            let api = &details["ApiGatewayApiAsset"];
            assert_eq!(
                api["ApiName"].as_str(),
                Some(case.asset_name),
                "case {label}: expected the imported API's name, got {api:?}"
            );
            assert_eq!(
                api["ApiId"].as_str(),
                Some("a1b2c3d4e5"),
                "case {label}: expected the imported API's id, got {api:?}"
            );
            assert_eq!(
                api["Stage"].as_str(),
                Some("prod"),
                "case {label}: expected the imported API's stage, got {api:?}"
            );
            assert_eq!(
                api["ProtocolType"].as_str(),
                Some("REST"),
                "case {label}: expected the imported API's protocol, got {api:?}"
            );
            assert!(
                details.get("S3SnapshotAsset").is_none(),
                "case {label}: expected only the API detail member, got {details:?}"
            );
        } else {
            assert_eq!(
                details["S3SnapshotAsset"]["Size"].as_f64(),
                Some(case.content.len() as f64),
                "case {label}: expected the size of the {} bytes imported, got {details:?}",
                case.content.len()
            );
            assert!(
                details.get("ApiGatewayApiAsset").is_none(),
                "case {label}: expected only the object detail member, got {details:?}"
            );
        }

        // The two readers of an asset answer the same stored record.
        let listed = list_assets(&router, &data_set_id, &revision_id).await;
        let entry = listed.member("Assets")[0].clone();
        for member in [
            "Arn",
            "AssetDetails",
            "AssetType",
            "CreatedAt",
            "DataSetId",
            "Id",
            "Name",
            "RevisionId",
            "UpdatedAt",
        ] {
            assert_eq!(
                answer.member(member),
                &entry[member],
                "case {label}: expected GetAsset and ListRevisionAssets to agree on {member}, \
                 got {:?} and {:?}",
                answer.member(member),
                entry[member]
            );
        }

        // Reading is reporting: a second read answers the same body and the store still
        // holds exactly the one asset the import put there.
        let again = get_asset(&router, &data_set_id, &revision_id, &asset_id).await;
        assert_eq!(
            again.body, answer.body,
            "case {label}: expected the same answer from a second read, got {:?}",
            again.body
        );
        assert_eq!(
            stored_asset_count(&engine, &data_set_id, &revision_id),
            1,
            "case {label}: expected the reads to have changed nothing in the store"
        );
    }
}

/// One asset written into a revision that holds several: its `Name` and the bytes it
/// holds, so each row is told apart by what the answer reports about it.
struct StoredCase {
    name: &'static str,
    size: usize,
}

/// A revision holding several assets, whose names are in no particular order and whose
/// contents differ in size: the answer has to be keyed on the `AssetId` asked for
/// rather than on where the asset sits in the revision.
fn stored_cases() -> Vec<StoredCase> {
    vec![
        StoredCase {
            name: "gamma.csv",
            size: 0,
        },
        StoredCase {
            name: "alpha.csv",
            size: 1,
        },
        StoredCase {
            name: "beta.csv",
            size: 1_024,
        },
        StoredCase {
            name: "beta.csv",
            size: 7,
        },
    ]
}

/// RULE: each asset a revision holds is addressed by its own `AssetId`, and the answer
/// reports that asset's own record — whatever order the revision stores them in and
/// however many it holds.
#[tokio::test]
async fn each_stored_asset_is_addressed_by_its_own_id() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "addressed-by-id", "S3_SNAPSHOT").await;

    let cases = stored_cases();
    let ids: Vec<String> = cases
        .iter()
        .map(|case| {
            store_asset(
                &engine,
                &data_set_id,
                &revision_id,
                case.name,
                vec![b'x'; case.size],
            )
        })
        .collect();

    for (case, asset_id) in cases.iter().zip(&ids) {
        let label = format!("{} of {} bytes", case.name, case.size);
        let answer = get_asset(&router, &data_set_id, &revision_id, asset_id).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("Id"),
            *asset_id,
            "case {label}: expected the asset asked about, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("Name"),
            case.name,
            "case {label}: expected the stored Name, got {:?}",
            answer.member("Name")
        );
        assert_eq!(
            answer.string("Arn"),
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}\
                 /revisions/{revision_id}/assets/{asset_id}"
            ),
            "case {label}: expected the Arn of the asset asked about, got {:?}",
            answer.member("Arn")
        );
        assert_eq!(
            answer.member("AssetDetails")["S3SnapshotAsset"]["Size"].as_f64(),
            Some(case.size as f64),
            "case {label}: expected the size of the stored bytes, got {:?}",
            answer.member("AssetDetails")
        );
    }

    assert_eq!(
        stored_asset_count(&engine, &data_set_id, &revision_id),
        cases.len(),
        "expected the reads to have changed nothing in the store"
    );
}

/// RULE: an asset is addressed within the named revision of the named data set, and an
/// identifier that addresses nothing there is the modelled 404 naming the kind that was
/// missing.
#[tokio::test]
async fn an_asset_outside_the_named_revision_is_not_found() {
    let (router, engine) = harness();
    let (first_data_set, first_revision) =
        data_set_with_revision(&router, "first", "S3_SNAPSHOT").await;
    let sibling_revision = create_revision(&router, &first_data_set, Some("sibling"), None, "setup")
        .await
        .string("Id");
    let (other_data_set, other_revision) =
        data_set_with_revision(&router, "second", "S3_SNAPSHOT").await;

    let here = store_asset(&engine, &first_data_set, &first_revision, "here.csv", Vec::new());
    let sibling = store_asset(
        &engine,
        &first_data_set,
        &sibling_revision,
        "sibling.csv",
        Vec::new(),
    );
    let elsewhere = store_asset(
        &engine,
        &other_data_set,
        &other_revision,
        "elsewhere.csv",
        Vec::new(),
    );

    // Rows differ along WHICH part of the route addresses nothing: the asset (absent,
    // or stored under another revision or another data set), the revision, or the data
    // set.
    let rows: Vec<(String, String, String, String, &str, String)> = vec![
        (
            "an asset id nothing is stored under".to_string(),
            first_data_set.clone(),
            first_revision.clone(),
            ABSENT_ID.to_string(),
            ASSET,
            ABSENT_ID.to_string(),
        ),
        (
            "an asset of a sibling revision of the same data set".to_string(),
            first_data_set.clone(),
            first_revision.clone(),
            sibling.clone(),
            ASSET,
            sibling.clone(),
        ),
        (
            "an asset of another data set".to_string(),
            first_data_set.clone(),
            first_revision.clone(),
            elsewhere.clone(),
            ASSET,
            elsewhere.clone(),
        ),
        (
            "an asset of this data set read through another data set's revision".to_string(),
            other_data_set.clone(),
            other_revision.clone(),
            here.clone(),
            ASSET,
            here.clone(),
        ),
        (
            "a revision nothing is stored under".to_string(),
            first_data_set.clone(),
            ABSENT_ID.to_string(),
            here.clone(),
            REVISION,
            ABSENT_ID.to_string(),
        ),
        (
            "a revision of another data set".to_string(),
            first_data_set.clone(),
            other_revision.clone(),
            here.clone(),
            REVISION,
            other_revision.clone(),
        ),
        (
            "a data set nothing is stored under".to_string(),
            ABSENT_ID.to_string(),
            first_revision.clone(),
            here.clone(),
            DATA_SET,
            ABSENT_ID.to_string(),
        ),
    ];

    for (label, data_set_id, revision_id, asset_id, resource_type, resource_id) in &rows {
        let answer = get_asset(&router, data_set_id, revision_id, asset_id).await;
        assert_eq!(
            answer.status, 404,
            "case {label}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.string("Message"),
            NOT_FOUND_MESSAGE,
            "case {label}: expected the fixed refusal message, got {:?}",
            answer.member("Message")
        );
        assert_eq!(
            answer.member("ResourceType").as_str(),
            Some(*resource_type),
            "case {label}: expected ResourceType {resource_type:?}, got {:?}",
            answer.member("ResourceType")
        );
        assert_eq!(
            answer.member("ResourceId").as_str(),
            Some(resource_id.as_str()),
            "case {label}: expected ResourceId to report what was missing, got {:?}",
            answer.member("ResourceId")
        );
    }

    // The asset that IS on the route still reads back, so the refusals above are about
    // what each row addressed rather than about the store.
    let found = get_asset(&router, &first_data_set, &first_revision, &here).await;
    assert_eq!(
        found.status, 200,
        "case the asset on its own route: expected 200, got {} body {:?}",
        found.status, found.body
    );
    assert_eq!(
        found.string("Name"),
        "here.csv",
        "case the asset on its own route: expected the stored Name, got {:?}",
        found.body
    );
}

/// RULE: an identifier outside the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` is a
/// 400 `ValidationException` in any of this route's three positions — the request is
/// malformed, not the resource absent — while an identifier inside the pattern is
/// acceptable, so the answer is then about the resource.
#[tokio::test]
async fn a_malformed_id_in_any_position_is_a_400() {
    let router = harness().0;
    let data_set_id = create_data_set(&router, "malformed-asset-reads", "m", "S3_SNAPSHOT", None)
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
            "a hyphen is not alphanumeric",
            format!("{}-{}", "a".repeat(16), "b".repeat(15)),
        ),
    ];

    for (how, id) in ids {
        for position in ["DataSetId", "RevisionId", "AssetId"] {
            let case = format!("{position} {how}");
            let answer = match position {
                "DataSetId" => get_asset(&router, &id, ABSENT_ID, ABSENT_ID).await,
                "RevisionId" => get_asset(&router, &data_set_id, &id, ABSENT_ID).await,
                _ => get_asset(&router, &data_set_id, ABSENT_ID, &id).await,
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
