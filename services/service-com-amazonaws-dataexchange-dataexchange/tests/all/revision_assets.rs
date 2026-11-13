// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListRevisionAssets`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch` with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//!
//! Four rules are under test.
//!
//! 1. The answer is the revision's stored assets: every asset the revision holds is
//!    reported once, with the identity the import that created it minted (`Id`, `Arn`,
//!    `Name`, `DataSetId`, `RevisionId`, `AssetType` and the instants it was created
//!    and last updated at), and with `AssetDetails` carrying the ONE detail member the
//!    asset's `AssetType` names, filled from what that import recorded.
//! 2. The page is sorted by `Name` alphabetically in descending order, which is what
//!    this operation's documentation says it answers.
//! 3. The page is bounded by `maxResults` and resumable through `NextToken`: walking
//!    the pages yields every asset exactly once in that order, and the last page
//!    carries no token.
//! 4. A revision holding nothing is an empty page, a data set or revision that is not
//!    there is a 404 naming what was missing, and a `NextToken` this caller was not
//!    issued is a 400.
//!
//! The tables span what the rules generalise over: both `AssetType` values an import
//! of this emulator can produce, content that is empty, text and many kilobytes, asset
//! counts of none, one, a few and more than a page, page sizes at and inside the
//! published `maxResults` bounds, names whose descending order is neither their import
//! order nor its reverse, and every way a continuation token can fail to be one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION};
use crate::job_lifecycle::{get_job, settle_past_the_execution_span, start_job};
use crate::jobs::create_job;
use crate::signed_url_uploads::{base64_md5, data_set_with_revision, put_upload};

/// The `Type` of an import whose content a client uploads to an announced URL, and
/// whose asset is the uploaded object.
const SIGNED_URL_TYPE: &str = "IMPORT_ASSET_FROM_SIGNED_URL";

/// The `Type` of an import whose asset IS an Amazon API Gateway API.
const API_GATEWAY_TYPE: &str = "IMPORT_ASSET_FROM_API_GATEWAY_API";

/// The published bounds of `com.amazonaws.dataexchange#MaxResults`
/// (`@range(min: 1, max: 200)`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 200;

/// Values of the model's `com.amazonaws.dataexchange#ResourceType` enum a lookup on
/// this route can report. Source: `smithy_query(query="shape", shape="ResourceType")`.
const DATA_SET: &str = "DATA_SET";
const REVISION: &str = "REVISION";

/// The whole `Message` AWS answers a continuation token this caller was not issued
/// with, stated here independently of the constant the crate implements it with.
const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "Invalid NextToken. Account Id does not match Caller's Account Id.";

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
/// along the content, because an asset's reported details are about the bytes it holds
/// whatever they are.
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

/// Import `case`'s asset into the revision through the calls a client makes:
/// `CreateJob` announces the upload URL, the client PUTs the content to it, `StartJob`
/// sets the import going and the clock settles the execution.
async fn import_asset(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &ImportCase,
    data_set_id: &str,
    revision_id: &str,
) {
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
}

/// Put one asset per name into the revision, the way an import leaves it there.
///
/// An asset is only ever created by an import job, so the full path is driven through
/// `CreateJob`/`StartJob` by [`import_asset`]; the ordering and paging tables write the
/// record straight through the shared engine instead — the way `revision_finalization.rs`
/// writes a revision's assets — which keeps a table of many assets cheap AND is what
/// shows the answer is composed from the stored record.
/// Hands back the ids it minted, in the order the names were given, so a caller that
/// has to address one of the stored assets (`tests/all/asset_deletion.rs`) can.
pub(crate) fn store_assets(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
    names: &[&str],
) -> Vec<String> {
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
    let mut minted = Vec::with_capacity(names.len());
    for name in names {
        let asset_id = dataexchange::state::mint_id();
        minted.push(asset_id.clone());
        revision.assets.push(dataexchange::state::Asset {
            arn: dataexchange::state::asset_arn(REGION, data_set_id, revision_id, &asset_id),
            id: asset_id,
            name: (*name).to_string(),
            asset_type: asset_type.clone(),
            content: Vec::new(),
            api: None,
            s3_data_access: None,
            created_at: imported_at,
            updated_at: imported_at,
        });
    }
    minted
}

/// The ids of the assets a revision holds, in the order the store keeps them, read
/// straight off the store — so an assertion is about what a call persisted and not only
/// about what a reader says.
///
/// `pub(crate)` so the suites that assert on persisted assets
/// (`tests/all/asset_deletion.rs`) read them through this one store walk.
pub(crate) fn stored_asset_ids(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> Vec<String> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .data_sets
        .get(data_set_id)
        .and_then(|data_set| data_set.revisions.get(revision_id))
        .map(|revision| {
            revision
                .assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// How many assets the revision holds, read straight off the store — so a read can be
/// shown to have changed nothing.
fn stored_asset_count(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> usize {
    stored_asset_ids(engine, data_set_id, revision_id).len()
}

/// `ListRevisionAssets` on a revision, with `query` appended verbatim (empty for a
/// request that carries no query string at all).
///
/// `pub(crate)` so every suite that reads a revision's assets back
/// (`tests/all/asset_deletion.rs`) sends the same request shape.
pub(crate) async fn list_assets(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets{query}"),
        None,
    )
    .await
}

/// The `Assets` the answer carries.
pub(crate) fn entries(answer: &Answer) -> Vec<Value> {
    answer
        .member("Assets")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `Name` of each entry, in the order the answer listed them.
fn names(answer: &Answer) -> Vec<String> {
    entries(answer)
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Hex of `text`, the encoding the opaque `NextToken` this service issues carries.
fn hex_of(text: &str) -> String {
    hex::encode(text)
}

/// RULE: every asset a revision holds is reported once, with the identity its import
/// minted and with `AssetDetails` carrying the one detail member its `AssetType` names
/// — the object's size for an asset that is a data object, and the API it fulfils for
/// an asset that is an API — and reading it changes nothing.
#[tokio::test]
async fn an_imported_asset_is_reported_from_the_stored_record() {
    for case in import_cases() {
        let (router, engine) = harness();
        let label = case.label;
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, label, case.asset_type).await;

        // Before the import the revision holds nothing, and that is an empty page.
        let before = list_assets(&router, &data_set_id, &revision_id, "").await;
        assert_eq!(
            before.status, 200,
            "case {label}: expected 200 on a revision holding nothing, got {} body {:?}",
            before.status, before.body
        );
        assert_eq!(
            before.member("Assets"),
            &json!([]),
            "case {label}: expected an empty page before the import, got {:?}",
            before.member("Assets")
        );

        import_asset(&router, &engine, &case, &data_set_id, &revision_id).await;

        let answer = list_assets(&router, &data_set_id, &revision_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        let listed = entries(&answer);
        assert_eq!(
            listed.len(),
            1,
            "case {label}: expected the one imported asset, got {listed:?}"
        );
        let entry = &listed[0];
        assert!(
            answer.member("NextToken").is_null(),
            "case {label}: expected no NextToken on a page that exhausted the revision, got {:?}",
            answer.member("NextToken")
        );

        let id = entry["Id"].as_str().unwrap_or_default();
        assert!(
            is_modelled_id(id),
            "case {label}: expected a minted Id matching the modelled pattern, got {id:?}"
        );
        assert_eq!(
            entry["Name"].as_str(),
            Some(case.asset_name),
            "case {label}: expected the Name the import gave the asset, got {:?}",
            entry["Name"]
        );
        assert_eq!(
            entry["AssetType"].as_str(),
            Some(case.asset_type),
            "case {label}: expected the data set's AssetType, got {:?}",
            entry["AssetType"]
        );
        assert_eq!(
            entry["DataSetId"].as_str(),
            Some(data_set_id.as_str()),
            "case {label}: expected the revision's data set, got {:?}",
            entry["DataSetId"]
        );
        assert_eq!(
            entry["RevisionId"].as_str(),
            Some(revision_id.as_str()),
            "case {label}: expected the revision asked about, got {:?}",
            entry["RevisionId"]
        );
        assert_eq!(
            entry["Arn"].as_str(),
            Some(
                format!(
                    "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}\
                     /revisions/{revision_id}/assets/{id}"
                )
                .as_str()
            ),
            "case {label}: expected the Arn built from the asset's ids, got {:?}",
            entry["Arn"]
        );
        assert!(
            entry.get("SourceId").is_none(),
            "case {label}: expected no SourceId on an asset of an owned data set, got {:?}",
            entry["SourceId"]
        );
        let created_at = seconds(&entry["CreatedAt"], label);
        let updated_at = seconds(&entry["UpdatedAt"], label);
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
        let details = &entry["AssetDetails"];
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
                api.get("ApiSpecificationDownloadUrl").is_none(),
                "case {label}: expected no download URL this emulator serves nothing on, \
                 got {api:?}"
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

        // Reading is reporting: a second read answers the same page and the store still
        // holds exactly the one asset the import put there.
        let again = list_assets(&router, &data_set_id, &revision_id, "").await;
        assert_eq!(
            again.body, answer.body,
            "case {label}: expected the same page from a second read, got {:?}",
            again.body
        );
        assert_eq!(
            stored_asset_count(&engine, &data_set_id, &revision_id),
            1,
            "case {label}: expected the reads to have changed nothing in the store"
        );
    }
}

/// One revision of assets to list, and the order the answer has to put them in.
struct OrderCase {
    label: &'static str,
    /// The assets, in the order they were imported.
    imported: Vec<&'static str>,
    /// The `Name`s in the order this operation answers them, written out rather than
    /// derived, so the expectation is independent of the comparison the handler makes.
    expected: Vec<&'static str>,
}

/// Revisions whose descending order is neither the import order nor its reverse.
///
/// The expected order is `Name` alphabetically descending, which is the order this
/// operation's own documentation states
/// (`smithy_query(query="digest")` -> "This operation lists a revision's assets sorted
/// alphabetically in descending order").
fn order_cases() -> Vec<OrderCase> {
    vec![
        OrderCase {
            label: "no assets",
            imported: vec![],
            expected: vec![],
        },
        OrderCase {
            label: "one asset",
            imported: vec!["beta.csv"],
            expected: vec!["beta.csv"],
        },
        OrderCase {
            label: "two assets imported in ascending order",
            imported: vec!["alpha.csv", "beta.csv"],
            expected: vec!["beta.csv", "alpha.csv"],
        },
        OrderCase {
            label: "two assets imported in descending order",
            imported: vec!["beta.csv", "alpha.csv"],
            expected: vec!["beta.csv", "alpha.csv"],
        },
        OrderCase {
            label: "assets whose names start with a digit",
            imported: vec!["2024-06.csv", "report.csv", "2024-05.csv"],
            expected: vec!["report.csv", "2024-06.csv", "2024-05.csv"],
        },
        OrderCase {
            label: "six assets imported in no order",
            imported: vec![
                "beta.csv",
                "delta.csv",
                "alpha.csv",
                "epsilon.csv",
                "charlie.csv",
                "gamma.csv",
            ],
            expected: vec![
                "gamma.csv",
                "epsilon.csv",
                "delta.csv",
                "charlie.csv",
                "beta.csv",
                "alpha.csv",
            ],
        },
        OrderCase {
            label: "two assets sharing a name",
            imported: vec!["same.csv", "other.csv", "same.csv"],
            expected: vec!["same.csv", "same.csv", "other.csv"],
        },
    ]
}

/// RULE: a revision's assets are answered sorted by `Name` alphabetically in
/// descending order, whatever order they were imported in.
#[tokio::test]
async fn assets_are_listed_by_name_alphabetically_descending() {
    for case in order_cases() {
        let (router, engine) = harness();
        let label = case.label;
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, label, "S3_SNAPSHOT").await;
        store_assets(&engine, &data_set_id, &revision_id, &case.imported);

        let answer = list_assets(&router, &data_set_id, &revision_id, "").await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            names(&answer),
            case.expected,
            "case {label}: expected {:?}, got {:?}",
            case.expected,
            names(&answer)
        );
        // Every stored asset is reported, exactly once.
        assert_eq!(
            entries(&answer).len(),
            case.imported.len(),
            "case {label}: expected every one of the {} stored assets, got {:?}",
            case.imported.len(),
            names(&answer)
        );
    }
}

/// RULE: a page is bounded by the `maxResults` the request asked for and resumed by
/// the `NextToken` it was handed: walking the pages yields every asset exactly once,
/// in the operation's descending order, and only the last page carries no token.
#[tokio::test]
async fn a_page_is_bounded_by_max_results_and_resumable() {
    // Seven assets, so a page size of one, of two and of more than the collection all
    // land differently.
    let imported = [
        "beta.csv", "delta.csv", "alpha.csv", "epsilon.csv", "charlie.csv", "gamma.csv",
        "zeta.csv",
    ];
    let expected = vec![
        "zeta.csv".to_string(),
        "gamma.csv".to_string(),
        "epsilon.csv".to_string(),
        "delta.csv".to_string(),
        "charlie.csv".to_string(),
        "beta.csv".to_string(),
        "alpha.csv".to_string(),
    ];

    // Rows differ along the page size asked for: the published minimum, a size inside
    // the bounds, one that is the whole collection, one larger than it, the published
    // maximum, and no page size at all.
    for page_size in [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(7),
        Some(8),
        Some(MAX_RESULTS_MAX),
        None,
    ] {
        let (router, engine) = harness();
        let label = match page_size {
            Some(size) => format!("maxResults={size}"),
            None => "no maxResults".to_string(),
        };
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, &label, "S3_SNAPSHOT").await;
        store_assets(&engine, &data_set_id, &revision_id, &imported);

        let bound = page_size.unwrap_or(MAX_RESULTS_MAX) as usize;
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let suffix = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let answer = list_assets(&router, &data_set_id, &revision_id, &suffix).await;
            assert_eq!(
                answer.status, 200,
                "case {label} page {pages}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            let page = names(&answer);
            assert!(
                page.len() <= bound,
                "case {label} page {pages}: expected at most {bound} assets, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= imported.len() + 1,
                "case {label}: expected the walk to terminate, got {walked:?}"
            );
            match answer.member("NextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case {label}: expected every asset exactly once in descending order, got {walked:?}"
        );
        let expected_pages = expected.len().div_ceil(bound).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {label}: expected {expected_pages} page(s) of at most {bound}, got {pages}"
        );
        assert_eq!(
            stored_asset_count(&engine, &data_set_id, &revision_id),
            imported.len(),
            "case {label}: expected the walk to have changed nothing in the store"
        );
    }
}

/// RULE: a request that addresses a data set or a revision this emulator does not hold
/// is a 404 reporting which of the two was missing, and one carrying a `NextToken`
/// this caller was not issued is a 400 — neither is an empty page.
#[tokio::test]
async fn a_request_about_something_absent_is_refused() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "refusals", "S3_SNAPSHOT").await;
    store_assets(&engine, &data_set_id, &revision_id, &["only.csv"]);
    let (other_data_set_id, other_revision_id) =
        data_set_with_revision(&router, "another", "S3_SNAPSHOT").await;

    // A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) that names nothing stored, at the
    // published minimum and maximum lengths.
    let absent_ids = ["a".repeat(30), "Z".repeat(40)];
    for absent in &absent_ids {
        // Rows differ along WHICH of the two the request got wrong: the data set, the
        // revision, and a revision that exists but belongs to another data set.
        let rows: Vec<(String, String, &str, &str)> = vec![
            (
                format!("an absent data set ({} chars)", absent.len()),
                format!("/v1/data-sets/{absent}/revisions/{revision_id}/assets"),
                DATA_SET,
                absent.as_str(),
            ),
            (
                format!("an absent revision ({} chars)", absent.len()),
                format!("/v1/data-sets/{data_set_id}/revisions/{absent}/assets"),
                REVISION,
                absent.as_str(),
            ),
            (
                "a revision of another data set".to_string(),
                format!("/v1/data-sets/{data_set_id}/revisions/{other_revision_id}/assets"),
                REVISION,
                other_revision_id.as_str(),
            ),
        ];
        for (case, uri, resource_type, resource_id) in rows {
            let answer = call(&router, "GET", &uri, None).await;
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
                answer.member("ResourceType").as_str(),
                Some(resource_type),
                "case {case}: expected ResourceType {resource_type}, got {:?}",
                answer.member("ResourceType")
            );
            assert_eq!(
                answer.member("ResourceId").as_str(),
                Some(resource_id),
                "case {case}: expected ResourceId {resource_id:?}, got {:?}",
                answer.member("ResourceId")
            );
        }
    }

    // Rows differ along HOW the token is wrong: not hex, hex of the wrong layout, hex
    // of an unknown scheme version, hex naming another account, and one longer than any
    // this service can issue.
    let tokens = [
        ("not hex at all", "zzzz".to_string()),
        ("hex of a bare word", hex_of("nonsense")),
        (
            "hex of an unknown version",
            hex_of(&format!("9|{ACCOUNT}|0|abc")),
        ),
        (
            "hex naming another account",
            hex_of("1|210987654321|0|abc"),
        ),
        (
            "longer than any issued token",
            hex_of(&format!("1|{ACCOUNT}|0|{}", "a".repeat(4096))),
        ),
    ];
    for (case, token) in tokens {
        let answer = list_assets(
            &router,
            &data_set_id,
            &revision_id,
            &format!("?maxResults=10&nextToken={token}"),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.string("Message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {case}: expected the NextToken refusal, got {:?}",
            answer.body
        );
    }

    // A revision of a data set that exists and holds nothing is an empty page, not a
    // refusal: the two answers above are about what is missing.
    let empty = list_assets(&router, &other_data_set_id, &other_revision_id, "").await;
    assert_eq!(
        empty.status, 200,
        "case \"a revision holding nothing\": expected 200, got {} body {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.member("Assets"),
        &json!([]),
        "case \"a revision holding nothing\": expected an empty page, got {:?}",
        empty.member("Assets")
    );
    assert!(
        empty.member("NextToken").is_null(),
        "case \"a revision holding nothing\": expected no NextToken, got {:?}",
        empty.member("NextToken")
    );
}

/// RULE: a cursor this service issued whose page no longer exists — the assets it was
/// minted against are gone — comes back as an empty final page, never a panic or a
/// 500.
#[tokio::test]
async fn a_stale_cursor_past_the_end_serves_an_empty_final_page() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "stale", "S3_SNAPSHOT").await;
    store_assets(&engine, &data_set_id, &revision_id, &["only.csv"]);

    // Rows differ along how far past the end the cursor's recorded offset sits,
    // including an offset no collection could ever reach.
    for offset in [1_usize, 2, 9_999, usize::MAX] {
        let token = hex_of(&format!(
            "1|{ACCOUNT}|{offset}|00000000000000000000000000000000"
        ));
        let answer = list_assets(
            &router,
            &data_set_id,
            &revision_id,
            &format!("?maxResults=10&nextToken={token}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case offset={offset}: expected 200 with an empty page, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("Assets"),
            &json!([]),
            "case offset={offset}: expected an empty page, got {:?}",
            answer.member("Assets")
        );
        assert!(
            answer.member("NextToken").is_null(),
            "case offset={offset}: expected no NextToken on a final page, got {:?}",
            answer.member("NextToken")
        );
    }
}

/// RULE: the `AssetType` values this emulator composes an asset's details for are
/// values of the model's own enum, so the two cannot drift.
#[tokio::test]
async fn the_asset_types_details_are_composed_for_are_modelled_values() {
    for value in [
        dataexchange::state::ASSET_TYPE_S3_SNAPSHOT,
        dataexchange::state::ASSET_TYPE_API_GATEWAY_API,
        dataexchange::state::ASSET_TYPE_S3_DATA_ACCESS,
    ] {
        assert!(
            dataexchange::ops::types::ASSET_TYPE_VALUES.contains(&value),
            "case {value}: expected a value of the model's AssetType enum, got {:?}",
            dataexchange::ops::types::ASSET_TYPE_VALUES
        );
    }
}
