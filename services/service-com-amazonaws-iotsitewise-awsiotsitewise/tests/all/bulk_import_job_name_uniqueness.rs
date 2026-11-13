// SPDX-License-Identifier: CC0-1.0

//! One bulk import job name is held by at most one bulk import job.
//!
//! The rule these suites pin: a name member this service documents as the unique
//! name of its resource is part of that resource's identity in the account, so a
//! create that claims a name another resource of the same kind already holds is
//! refused with the modelled `ResourceAlreadyExistsException` (HTTP 409) naming
//! the holder — its own `resourceId` and the ARN it was minted with — and stores
//! nothing. `jobName` is one of those members (`smithy_query(query="doc",
//! shape="CreateBulkImportJobRequest", member="jobName")`: "The unique name that
//! helps identify the job request."), beside `gatewayName` and `assetModelName`,
//! whose refusals the `gateway_lifecycle` and `asset_model_name_uniqueness`
//! suites pin; `datasetName` and `computationModelName` are documented only as
//! "The name of the dataset" / "the computation model", so they carry no such
//! rule.
//!
//! The rows span what the rule generalises over: the VALUE and SIZE of the name
//! (both published bounds of `BulkImportJobName`, the punctuation and the
//! non-ASCII letters its pattern admits), the DESTINATION each create names (a
//! session dataset of a workspace, and the time series outside one), what else
//! the colliding request carries (another role ARN, other files, a job
//! configuration, each ingestion flag, another destination), the CASE and spacing
//! of a name that only resembles one taken, the region the colliding request is
//! signed for, and the state change that frees a name again.
//!
//! The wire harness lives in `super::asset_lifecycle`, the workspace fixture in
//! `super::search_lifecycle`, the storage put in `super::storage_configuration`,
//! the dataset create in `super::dataset_data_segments` and the region-scoped
//! dispatch in `super::refusal_resource_arn`; all are reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router, REGION};
use crate::dataset_data_segments::create_dataset;
use crate::refusal_resource_arn::call_signed_for;
use crate::search_lifecycle::create_workspace;
use crate::storage_configuration::put as put_storage;

/// `POST /jobs` and `GET /jobs`, as `OPERATIONS.md` reports the routes.
const JOBS_URI: &str = "/jobs";

/// A page covering every job these suites create. `maxResults` is range 1..250
/// (`smithy_query(query="operation", operation="ListBulkImportJobs")`).
const JOBS_PAGE: &str = "/jobs?maxResults=250";

/// The workspace whose session dataset the destination rows name.
const WORKSPACE: &str = "plant-floor";

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// The OTHER regional endpoint one store is reached from, so a refusal can be
/// shown to carry the ARN its holder was minted with rather than the region the
/// refused request was signed for.
const AWAY: &str = "eu-central-1";

/// Where a job ingests: into the session dataset of [`WORKSPACE`], or into time
/// series outside every workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Dataset,
    Tiered,
}

/// The create body for `job_name` ingesting into `destination`, with `extra`'s
/// members added on top.
fn body_for(job_name: &str, destination: Destination, dataset_id: &str, extra: Value) -> Value {
    let mut body = json!({
        "jobName": job_name,
        "jobRoleArn": JOB_ROLE_ARN,
        "files": [{ "bucket": "sitewise-import", "key": "turbines/2026/01.csv" }],
        "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
    });
    let object = body.as_object_mut().expect("an object body");
    if destination == Destination::Dataset {
        object.insert("workspaceName".to_string(), json!(WORKSPACE));
        object.insert("datasetId".to_string(), json!(dataset_id));
    }
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        object.insert(member, value);
    }
    body
}

/// `CreateBulkImportJob` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", JOBS_URI, Some(body)).await
}

/// An ARN of this service as an INDEPENDENT oracle: built here from a region and
/// the emulator's account id rather than through the helper the handlers use.
fn job_arn_in(region: &str, job_id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{region}:{}:bulk-import-job/{job_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// How many jobs the account holds, read through `ListBulkImportJobs`.
async fn jobs_listed(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", JOBS_PAGE, None).await;
    assert_eq!(status, 200, "ListBulkImportJobs: got {status} {body}");
    body["jobSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobSummaries list, got {body}"))
        .len()
}

/// The `jobName` `DescribeBulkImportJob` reports for `job_id`.
async fn described_name(router: &MultiServiceRouter, job_id: &str) -> String {
    let (status, body) = call(router, "GET", &format!("{JOBS_URI}/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeBulkImportJob {job_id}: got {status} {body}"
    );
    body["jobName"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeBulkImportJob {job_id}: no jobName in {body}"))
        .to_string()
}

/// Start the job holding `job_name` and answer its minted `jobId`.
async fn hold_name(
    router: &MultiServiceRouter,
    job_name: &str,
    destination: Destination,
    dataset_id: &str,
) -> String {
    let (status, created) = create(router, body_for(job_name, destination, dataset_id, json!({})))
        .await;
    assert_eq!(
        status, 202,
        "holding the name {job_name:?}: expected 202, got {status} {created}"
    );
    let job_id = created["jobId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&job_id),
        "holding the name {job_name:?}: expected a modelled jobId, got {created}"
    );
    job_id
}

/// Assert that `answered` is the modelled 409 naming the job `holder`, minted in
/// `region`.
fn assert_name_conflict(case: &str, status: u16, answered: &Value, holder: &str, region: &str) {
    assert_eq!(
        status, 409,
        "case {case:?}: a name already held expected 409, got {status} {answered}"
    );
    assert_eq!(
        error_code(answered),
        "ResourceAlreadyExistsException",
        "case {case:?}: unexpected error code in {answered}"
    );
    assert_eq!(
        answered["resourceId"].as_str().unwrap_or(""),
        holder,
        "case {case:?}: the refusal must name the job holding the name, got {answered}"
    );
    assert_eq!(
        answered["resourceArn"].as_str().unwrap_or(""),
        job_arn_in(region, holder),
        "case {case:?}: the refusal must carry that job's own ARN, got {answered}"
    );
    assert_eq!(
        answered["message"].as_str().unwrap_or(""),
        "Another bulk import job is already using the same name",
        "case {case:?}: the refusal must name the collided dimension, got {answered}"
    );
}

/// A router with the warm tier enabled — the documented precondition of a job
/// ingesting outside a workspace — plus [`WORKSPACE`] and a session dataset of
/// it, so both destinations a job may have are available. Answers the dataset id.
async fn fixture(router: &MultiServiceRouter) -> String {
    let (status, body) = put_storage(
        router,
        json!({ "storageType": "SITEWISE_DEFAULT_STORAGE", "warmTier": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "PutStorageConfiguration got {status} {body}");
    create_workspace(router, WORKSPACE).await;
    create_dataset(router, "Import target", Some("SESSION"), Some(WORKSPACE)).await
}

/// The `BulkImportJobName` shape bounds the member at 1..256 characters with the
/// pattern `^[\p{L}\p{N}\p{Zs}._:/-]+$` (`smithy_query(query="doc",
/// shape="CreateBulkImportJobRequest", member="jobName")`), so the rows carry
/// both published bounds as well as values inside them — including a name the
/// issue's own example did not cite.
fn name_rows() -> Vec<(&'static str, String, Destination)> {
    vec![
        (
            "a plain name",
            "Historical import".to_string(),
            Destination::Tiered,
        ),
        (
            "the shortest name the shape admits",
            "a".to_string(),
            Destination::Dataset,
        ),
        (
            "the longest name the shape admits",
            "L".repeat(256),
            Destination::Tiered,
        ),
        (
            "every punctuation mark the pattern admits",
            "csv_import:2026/01-part.7".to_string(),
            Destination::Dataset,
        ),
        (
            "non-ASCII letters and digits",
            "Füllstand-Ⅲ-蒸気-7".to_string(),
            Destination::Tiered,
        ),
    ]
}

#[tokio::test]
async fn a_job_name_is_held_by_at_most_one_bulk_import_job() {
    let router = router();
    let dataset_id = fixture(&router).await;

    for (index, (case, name, destination)) in name_rows().into_iter().enumerate() {
        let holder = hold_name(&router, &name, destination, &dataset_id).await;
        let before = jobs_listed(&router).await;

        // The second create claims the same name, from the OTHER destination, so
        // the conflict is the name and not the destination it ingests into.
        let other = match destination {
            Destination::Dataset => Destination::Tiered,
            Destination::Tiered => Destination::Dataset,
        };
        let (status, answered) =
            create(&router, body_for(&name, other, &dataset_id, json!({}))).await;
        assert_name_conflict(case, status, &answered, &holder, REGION);
        let after = jobs_listed(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must start no job, {before} -> {after}"
        );
        assert_eq!(
            described_name(&router, &holder).await,
            name,
            "case {case:?}: the holder must keep its name"
        );

        // A name no job holds is free, whatever its shape: the refusal above is
        // about the name being taken, not about the value. The value stays inside
        // the member's own pattern, which admits no parentheses.
        let free = format!("free.name.{index}");
        let (status, created) =
            create(&router, body_for(&free, destination, &dataset_id, json!({}))).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a free name expected 202, got {status} {created}"
        );
    }
}

#[tokio::test]
async fn the_name_alone_is_the_conflict_whatever_else_the_create_carries() {
    let router = router();
    let dataset_id = fixture(&router).await;
    let name = "Historical import";
    let holder = hold_name(&router, name, Destination::Tiered, &dataset_id).await;

    // Rows differ along what the second create varies BESIDES the name: the role
    // ARN, the file list's size and the formats it names, a job configuration,
    // each ingestion flag, and the destination. None of them frees the name.
    let rows: Vec<(&str, Destination, Value)> = vec![
        ("the bare required members", Destination::Tiered, json!({})),
        (
            "another role ARN",
            Destination::Tiered,
            json!({ "jobRoleArn": "arn:aws:iam::000000000000:role/OtherImport" }),
        ),
        (
            "several files of other formats",
            Destination::Tiered,
            json!({
                "files": [
                    {
                        "bucket": "sitewise-import",
                        "key": "p/part-0",
                        "fileFormat": { "parquet": {} },
                    },
                    {
                        "bucket": "sitewise-import",
                        "key": "a/labels.json",
                        "fileFormat": { "annotation": {} },
                    },
                ],
            }),
        ),
        (
            "a job configuration",
            Destination::Tiered,
            json!({ "jobConfiguration": { "fileFormat": { "parquet": {} } } }),
        ),
        (
            "both ingestion flags",
            Destination::Tiered,
            json!({ "adaptiveIngestion": true, "deleteFilesAfterImport": true }),
        ),
        (
            "another error report location",
            Destination::Tiered,
            json!({ "errorReportLocation": { "bucket": "other-errors", "prefix": "e/" } }),
        ),
        (
            "a session dataset as the destination",
            Destination::Dataset,
            json!({}),
        ),
    ];

    let before = jobs_listed(&router).await;
    for (case, destination, extra) in rows {
        let (status, answered) =
            create(&router, body_for(name, destination, &dataset_id, extra)).await;
        assert_name_conflict(case, status, &answered, &holder, REGION);
        let after = jobs_listed(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must start no job, {before} -> {after}"
        );
    }
}

#[tokio::test]
async fn a_name_that_only_resembles_one_taken_is_another_name() {
    let router = router();
    let dataset_id = fixture(&router).await;
    let taken = "Historical import";
    let holder = hold_name(&router, taken, Destination::Tiered, &dataset_id).await;

    // The modelled `BulkImportJobName` carries no case-insensitivity and no
    // trimming, so each row below is a name of its own and is served.
    let rows: Vec<(&str, String)> = vec![
        ("the same letters in lower case", "historical import".to_string()),
        ("the same letters in upper case", "HISTORICAL IMPORT".to_string()),
        (
            "an extra space the pattern admits",
            "Historical  import".to_string(),
        ),
        ("a trailing space", format!("{taken} ")),
        ("the name with a suffix", format!("{taken} 2")),
    ];

    let mut minted = vec![holder.clone()];
    for (case, name) in rows {
        let (status, created) =
            create(&router, body_for(&name, Destination::Dataset, &dataset_id, json!({}))).await;
        assert_eq!(
            status, 202,
            "case {case:?}: {name:?} is not the name {taken:?}, so it must be served, \
             got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every served create mints a job of its own, got {created}"
        );
        assert_eq!(
            described_name(&router, &job_id).await,
            name,
            "case {case:?}: the job must read back the name it was started with"
        );
        minted.push(job_id);
    }

    // The name itself is still taken: the rows above freed nothing.
    let (status, answered) =
        create(&router, body_for(taken, Destination::Tiered, &dataset_id, json!({}))).await;
    assert_name_conflict(
        "the taken name itself, after every near miss",
        status,
        &answered,
        &holder,
        REGION,
    );
}

#[tokio::test]
async fn the_refusal_names_the_arn_the_holder_was_minted_with() {
    let router = router();
    let dataset_id = fixture(&router).await;
    let name = "Historical import";
    // The holder is created through the shared harness, which signs for `REGION`.
    let holder = hold_name(&router, name, Destination::Tiered, &dataset_id).await;

    // The colliding create is signed for the OTHER regional endpoint of the same
    // store: the ARN the refusal reports is the one the HOLDER carries, never one
    // rebuilt from the refused request's region.
    let (status, answered) = call_signed_for(
        &router,
        AWAY,
        "POST",
        JOBS_URI,
        Some(body_for(name, Destination::Dataset, &dataset_id, json!({}))),
        &[],
    )
    .await;
    assert_name_conflict(
        "a collision reached from another region",
        status,
        &answered,
        &holder,
        REGION,
    );
    assert!(
        !answered["resourceArn"]
            .as_str()
            .unwrap_or_default()
            .contains(AWAY),
        "the refusal must not carry the requesting region, got {answered}"
    );
}

#[tokio::test]
async fn deleting_the_jobs_destination_frees_their_name() {
    let router = router();
    let dataset_id = fixture(&router).await;
    let name = "Session import";
    let holder = hold_name(&router, name, Destination::Dataset, &dataset_id).await;

    let (status, answered) =
        create(&router, body_for(name, Destination::Dataset, &dataset_id, json!({}))).await;
    assert_name_conflict(
        "while the holder is stored",
        status,
        &answered,
        &holder,
        REGION,
    );

    // The rule follows the STATE: a delete that takes the job with it — a job's
    // destination dataset going leaves no job behind — frees the name it held.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/datasets/{dataset_id}?workspaceName={WORKSPACE}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "DeleteDataset {dataset_id}: got {status} {deleted}"
    );
    assert_eq!(
        jobs_listed(&router).await,
        0,
        "the delete must take the jobs of that dataset with it"
    );

    let (status, created) =
        create(&router, body_for(name, Destination::Tiered, &dataset_id, json!({}))).await;
    assert_eq!(
        status, 202,
        "a name no stored job holds must be free, got {status} {created}"
    );
    let job_id = created["jobId"].as_str().unwrap_or("").to_string();
    assert_ne!(
        job_id, holder,
        "the create must mint a job of its own, got {created}"
    );
    assert_eq!(
        described_name(&router, &job_id).await,
        name,
        "the new holder must read back the name it was started with"
    );
}
