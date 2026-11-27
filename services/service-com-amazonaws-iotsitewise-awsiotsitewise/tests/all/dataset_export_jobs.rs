// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise dataset export job slice
//! (`CreateDatasetExportJob` -> `DescribeDatasetExportJob`), driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created job is PERSISTED and reads back exactly as it was started — its
//!   destination, its error report location and the processing input the create
//!   carried, whatever shape that input took;
//! * a job's `status` is owed by the CLOCK, not by the read: a job reports
//!   `SUBMITTED` straight after the create, then `RUNNING`, then the terminal
//!   `COMPLETED`, and reports no `completedAt` until that terminal status falls
//!   due — so a read never moves a job on and two reads of one job agree;
//! * a job is addressed by `(workspaceName, jobId)`, so a job of one workspace is
//!   not reachable through another, and a job no workspace holds is the modelled
//!   not-found refusal;
//! * a create whose input breaks a rule the input shapes state is the modelled
//!   `InvalidRequestException` and starts NO job, and one naming a dataset the
//!   store does not hold is the modelled not-found refusal;
//! * a retried create — the SAME request under the same `clientToken` — replays the
//!   job the first one started rather than starting a second, and rewrites nothing
//!   on the job it replays, while a DIFFERENT request reusing that token repeats no
//!   call and is refused the modelled conflict naming the job the token's own call
//!   started, the workspace the create names being one of the members that call is
//!   recognised by;
//! * a create answers in the workspace its request names, whatever form that name
//!   takes within the modelled constraint, and the store bounds how many jobs one
//!   workspace holds, past which the create is the modelled refusal and starts no
//!   job;
//! * a job carries a NAME, resolved once by the create: the `jobName` the request
//!   carried, kept verbatim, or — for a request that carried none — one built from
//!   the kind of export and the instant the job started, which every read of that
//!   job then reports.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, EXPORT_JOB_STEP, MAX_DATASET_EXPORT_JOBS_PER_WORKSPACE,
};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router, router_with_engine};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// `POST /workspaces/{workspaceName}/dataset-export-jobs`, as `OPERATIONS.md`
/// reports the route.
fn jobs_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs")
}

/// `GET /workspaces/{workspaceName}/dataset-export-jobs/{jobId}`.
fn job_uri(workspace: &str, job_id: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs/{job_id}")
}

/// An S3 URI of the form `S3Uri`'s pattern admits
/// (`^s3://[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]/.+$`).
pub(crate) const DESTINATION: &str = "s3://sitewise-exports/clips/";

/// The same member's error-report form, so a response reporting one in place of
/// the other cannot pass.
pub(crate) const ERROR_REPORT: &str = "s3://sitewise-exports/errors/";

/// The destination a RETRY names, so a replay that reported the retry's request
/// instead of the job's own cannot pass.
pub(crate) const OTHER_DESTINATION: &str = "s3://sitewise-exports/other-clips/";

/// The error report location a RETRY names, for the same reason.
const OTHER_ERROR_REPORT: &str = "s3://sitewise-exports/other-errors/";

/// The `DatasetExportJobStatus` values, from the enum itself (smithy_query
/// query="shape" shape="DatasetExportJobStatus").
const JOB_STATUSES: [&str; 5] = [
    "SUBMITTED",
    "RUNNING",
    "COMPLETED",
    "COMPLETED_WITH_ERRORS",
    "FAILED",
];

/// The `ExportDataType` values a `DatasetItem` may name a subset of (smithy_query
/// query="shape" shape="ExportDataType").
const EXPORT_DATA_TYPES: [&str; 3] = ["VIDEO", "TELEMETRY", "ANNOTATION"];

/// How long after a step falls due a read is taken, so a run whose scheduler is
/// late cannot observe the previous status.
const SLACK: Duration = Duration::from_millis(250);

/// A `TimeSeriesId` of the associated form, which this service builds as
/// `<assetId>.<propertyId>` — the longest the modelled length (36..73) admits.
pub(crate) const ASSOCIATED_SERIES: &str =
    "11111111-2222-3333-4444-555555555555.66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A `TimeSeriesId` of the shortest form that length admits: a single id, which
/// is how this service names a stream no asset property owns.
pub(crate) const DISASSOCIATED_SERIES: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// A `TimeInNanos`, as the two members the shape declares.
fn nanos(seconds: i64, offset: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": offset })
}

/// A `trimSettings` spanning `[start, end)`.
fn trim(start: Value, end: Value) -> Value {
    json!({ "startTime": start, "endTime": end })
}

/// `CreateDatasetExportJob` with `body`.
///
/// A job is started INSIDE a workspace, which the create resolves through the
/// workspace registry, so this helper opens the workspace it is pointed at first.
/// A suite that is about an unopened workspace addresses the route directly
/// instead.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    crate::support::ensure_workspace(router, workspace).await;
    call(router, "POST", &jobs_uri(workspace), Some(body)).await
}

/// `DescribeDatasetExportJob`.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    workspace: &str,
    job_id: &str,
) -> (u16, Value) {
    call(router, "GET", &job_uri(workspace, job_id), None).await
}

/// The body a create sends: the destination, the error report location, the
/// processing input, and `clientToken` only when the row carries one.
pub(crate) fn create_body(input: &Value, token: Option<&str>) -> Value {
    create_body_from(DESTINATION, ERROR_REPORT, input, token)
}

/// The same, with every member a row may vary spelled out, so a retry can send a
/// request that differs from the one it retries.
pub(crate) fn create_body_from(
    destination: &str,
    error_report: &str,
    input: &Value,
    token: Option<&str>,
) -> Value {
    let mut body = json!({
        "destinationS3Uri": destination,
        "errorReportLocation": { "s3Uri": error_report },
        "input": input.clone(),
    });
    if let Some(token) = token {
        body.as_object_mut()
            .expect("an object body")
            .insert("clientToken".to_string(), json!(token));
    }
    body
}

/// How many jobs the store holds, across every workspace — the only way to
/// observe that a refused or replayed create wrote nothing, since no reader
/// enumerates a workspace's jobs yet.
pub(crate) fn jobs_held(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("the service store");
    let store = state.lock();
    store.dataset_export_jobs.len()
}

/// Create a dataset through `CreateDataset` and answer its id, so the rows whose
/// `ProcessingInput` names a dataset name one the store really holds.
pub(crate) async fn create_dataset(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
        })),
    )
    .await;
    assert_eq!(status, 202, "create dataset {name}: got {status} {body}");
    body["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create dataset {name}: no datasetId in {body}"))
        .to_string()
}

/// Start a job with `input` and answer its id.
async fn start_job(router: &MultiServiceRouter, workspace: &str, input: &Value) -> String {
    let (status, body) = create(router, workspace, create_body(input, None)).await;
    assert_eq!(status, 202, "the create got {status} {body}");
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("no jobId in {body}"))
        .to_string()
}

/// One row of the create-and-read-back table: the processing input a create
/// sends, built from the id of the dataset this suite created.
struct InputCase {
    label: &'static str,
    input: fn(&str) -> Value,
    client_token: Option<&'static str>,
}

/// The rows: `ProcessingInput` spanning both members the union declares — a
/// `timeseries` list of one and of several items, identified by `timeSeriesId` of
/// each length the type admits and by `propertyAlias`, with and without
/// `trimSettings` and `formatSettings`, and a `dataset` item with no options, with
/// a trim range, and with one and with every `ExportDataType` — crossed with a
/// present / absent `clientToken`.
fn input_cases() -> Vec<InputCase> {
    vec![
        InputCase {
            label: "one-timeseries-by-the-longest-id",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] }),
            client_token: None,
        },
        InputCase {
            label: "one-timeseries-by-the-shortest-id",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] }),
            client_token: Some("11111111-1111-1111-1111-111111111111"),
        },
        InputCase {
            label: "one-timeseries-by-alias-trimmed-and-formatted",
            input: |_| {
                json!({
                    "timeseries": [{
                        "propertyAlias": "/company/windfarm/3/turbine/7/temperature",
                        "trimSettings": trim(nanos(1_760_000_000, 0), nanos(1_760_003_600, 0)),
                        "formatSettings": {
                            "framesPerSecond": 30,
                            "widthInPixels": 1_920,
                            "heightInPixels": 1_080,
                        },
                    }]
                })
            },
            client_token: None,
        },
        InputCase {
            label: "several-timeseries-mixing-ids-and-aliases",
            input: |_| {
                json!({
                    "timeseries": [
                        { "timeSeriesId": ASSOCIATED_SERIES },
                        {
                            "propertyAlias": "/company/windfarm/3/turbine/7/vibration",
                            "trimSettings": trim(
                                nanos(1_760_000_000, 1),
                                nanos(1_760_000_000, 2),
                            ),
                        },
                        {
                            "timeSeriesId": DISASSOCIATED_SERIES,
                            "formatSettings": { "framesPerSecond": 1 },
                        },
                    ]
                })
            },
            client_token: Some("22222222-2222-2222-2222-222222222222"),
        },
        InputCase {
            label: "dataset-with-no-options",
            input: |dataset_id| json!({ "dataset": { "datasetId": dataset_id } }),
            client_token: None,
        },
        InputCase {
            label: "dataset-trimmed-to-one-data-type",
            input: |dataset_id| {
                json!({
                    "dataset": {
                        "datasetId": dataset_id,
                        "trimSettings": trim(nanos(1_700_000_000, 0), nanos(1_700_000_001, 0)),
                        "exportDataTypes": [EXPORT_DATA_TYPES[1]],
                    }
                })
            },
            client_token: Some("33333333-3333-3333-3333-333333333333"),
        },
        InputCase {
            label: "dataset-with-every-data-type",
            input: |dataset_id| {
                json!({
                    "dataset": {
                        "datasetId": dataset_id,
                        "exportDataTypes": EXPORT_DATA_TYPES,
                    }
                })
            },
            client_token: None,
        },
    ]
}

#[tokio::test]
async fn a_started_job_reads_back_through_describe_for_every_processing_input_shape() {
    let router = router();
    let dataset_id = create_dataset(&router, "Export source").await;

    for case in input_cases() {
        let label = case.label;
        let input = (case.input)(&dataset_id);
        let (status, created) =
            create(&router, WORKSPACE, create_body(&input, case.client_token)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("");
        assert!(
            is_modelled_id(job_id),
            "case {label:?}: jobId must have the modelled form, got {created}"
        );
        assert_eq!(
            created["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {label:?}: unexpected workspaceName in {created}"
        );

        let (status, described) = describe(&router, WORKSPACE, job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeDatasetExportJob expected 200, got {status} {described}"
        );
        assert_eq!(
            described["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: a read must report the job it was asked for, got {described}"
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {label:?}: unexpected workspaceName in {described}"
        );
        assert_eq!(
            described["destinationS3Uri"].as_str().unwrap_or(""),
            DESTINATION,
            "case {label:?}: unexpected destinationS3Uri in {described}"
        );
        assert_eq!(
            described["errorReportLocation"],
            json!({ "s3Uri": ERROR_REPORT }),
            "case {label:?}: unexpected errorReportLocation in {described}"
        );
        assert_eq!(
            described["input"], input,
            "case {label:?}: a read must report the processing input the create carried, \
             got {described}"
        );
        assert_eq!(
            described["status"].as_str().unwrap_or(""),
            JOB_STATUSES[0],
            "case {label:?}: a job reports {:?} until the clock starts it, got {described}",
            JOB_STATUSES[0]
        );
        assert!(
            described.get("completedAt").is_none(),
            "case {label:?}: a job that has not completed reports no completedAt, got {described}"
        );
        assert!(
            described["startedAt"].as_f64().unwrap_or(0.0) > 0.0,
            "case {label:?}: startedAt must be a real instant, got {described}"
        );
    }
}

#[tokio::test]
async fn the_status_a_job_reports_is_the_one_the_clock_has_made_due() {
    let router = router();
    let input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let job_id = start_job(&router, WORKSPACE, &input).await;

    // (case, how long to wait before the read, the status then due, whether
    // `completedAt` is reported by then) — the lifecycle every job walks, read
    // through the API alone.
    let steps: [(&str, Duration, &str, bool); 3] = [
        ("straight after the create", Duration::ZERO, "SUBMITTED", false),
        ("once processing is due", EXPORT_JOB_STEP + SLACK, "RUNNING", false),
        ("once completion is due", EXPORT_JOB_STEP + SLACK, "COMPLETED", true),
    ];

    let mut started_at = None;
    for (label, wait, expected, completed) in steps {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let (status, body) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["status"].as_str().unwrap_or(""),
            expected,
            "case {label:?}: expected status {expected:?}, got {body}"
        );

        let reported_start = body["startedAt"].as_f64().unwrap_or(0.0);
        match started_at {
            None => started_at = Some(reported_start),
            Some(first) => assert_eq!(
                reported_start, first,
                "case {label:?}: startedAt must not move as the job runs, got {body}"
            ),
        }

        match (completed, body["completedAt"].as_f64()) {
            (false, reported) => assert!(
                reported.is_none(),
                "case {label:?}: a job still running reports no completedAt, got {body}"
            ),
            (true, Some(completed_at)) => {
                let elapsed = completed_at - reported_start;
                assert!(
                    elapsed >= 2.0 * EXPORT_JOB_STEP.as_secs_f64(),
                    "case {label:?}: completedAt must be when the terminal status fell due, \
                     {elapsed}s after startedAt in {body}"
                );
            }
            (true, None) => panic!("case {label:?}: a completed job reports completedAt, got {body}"),
        }

        // A read is a read: taking it again answers the same thing, so nothing the
        // first read did moved the job on.
        let (status, again) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the second read got {status} {again}"
        );
        assert_eq!(
            again["status"].as_str().unwrap_or(""),
            expected,
            "case {label:?}: two reads of one job must agree, got {again} after {body}"
        );
    }

    // COMPLETED is terminal: the status stays there however long the job is left.
    tokio::time::sleep(EXPORT_JOB_STEP * 3).await;
    let (status, settled) = describe(&router, WORKSPACE, &job_id).await;
    assert_eq!(status, 200, "the read after the job settled got {status} {settled}");
    assert_eq!(
        settled["status"].as_str().unwrap_or(""),
        "COMPLETED",
        "COMPLETED is terminal, got {settled}"
    );
}

#[tokio::test]
async fn a_job_is_addressed_within_the_workspace_that_holds_it() {
    let router = router();
    let input = json!({ "timeseries": [{ "propertyAlias": "/plant/line-1/rate" }] });

    let here = start_job(&router, WORKSPACE, &input).await;
    let elsewhere = start_job(&router, "other-floor", &input).await;
    assert_ne!(
        here, elsewhere,
        "two creates must start two jobs, got {here} twice"
    );

    // (case, workspace, job, found) — a job resolves in its OWN workspace only,
    // and an id no create ever minted resolves nowhere.
    let unknown = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let rows: [(&str, &str, &str, bool); 5] = [
        ("its own workspace", WORKSPACE, here.as_str(), true),
        ("the other workspace's own", "other-floor", elsewhere.as_str(), true),
        ("another workspace", "other-floor", here.as_str(), false),
        ("a workspace no job lives in", "empty-floor", here.as_str(), false),
        ("an id no create minted", WORKSPACE, unknown, false),
    ];

    for (label, workspace, job_id, found) in rows {
        let (status, body) = describe(&router, workspace, job_id).await;
        if found {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {body}"
            );
            assert_eq!(
                body["jobId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: unexpected jobId in {body}"
            );
            assert_eq!(
                body["workspaceName"].as_str().unwrap_or(""),
                workspace,
                "case {label:?}: unexpected workspaceName in {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {label:?}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_that_breaks_a_rule_the_shapes_state_starts_no_job() {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Export source").await;

    // (case, input, expected status, expected error code). The rules: a
    // `TimeseriesItem` names "exactly one of timeSeriesId or propertyAlias", a
    // `TrimSettings` end time "must be greater than startTime", and a `DatasetItem`
    // names a dataset this service holds.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "both identifiers on one item",
            json!({
                "timeseries": [{
                    "timeSeriesId": ASSOCIATED_SERIES,
                    "propertyAlias": "/plant/line-1/rate",
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "neither identifier on one item",
            json!({ "timeseries": [{ "formatSettings": { "framesPerSecond": 5 } }] }),
            400,
            "InvalidRequestException",
        ),
        (
            "both identifiers on a later item",
            json!({
                "timeseries": [
                    { "timeSeriesId": ASSOCIATED_SERIES },
                    { "timeSeriesId": DISASSOCIATED_SERIES, "propertyAlias": "/plant/line-1/rate" },
                ]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a timeseries trim range that ends where it starts",
            json!({
                "timeseries": [{
                    "timeSeriesId": ASSOCIATED_SERIES,
                    "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 7)),
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a timeseries trim range that ends a nanosecond before it starts",
            json!({
                "timeseries": [{
                    "propertyAlias": "/plant/line-1/rate",
                    "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 6)),
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset trim range that ends before it starts",
            json!({
                "dataset": {
                    "datasetId": dataset_id,
                    "trimSettings": trim(nanos(1_760_000_100, 0), nanos(1_760_000_000, 0)),
                }
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset no store holds",
            json!({ "dataset": { "datasetId": "ffffffff-ffff-ffff-ffff-ffffffffffff" } }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (label, input, expected_status, expected_code) in rows {
        let (status, body) = create(&router, WORKSPACE, create_body(&input, None)).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {label:?}: unexpected error code in {body}"
        );

        // A refused create writes NOTHING. The store is the only place that shows
        // it: no reader enumerates a workspace's jobs yet.
        let held = jobs_held(&engine);
        assert_eq!(
            held, 0,
            "case {label:?}: a refused create must start no job, the store holds {held}"
        );
    }

    // The same range one nanosecond wider is accepted, so the refusals above are
    // about the rule and not about trimming at all.
    let input = json!({
        "timeseries": [{
            "timeSeriesId": ASSOCIATED_SERIES,
            "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 8)),
        }]
    });
    let (status, accepted) = create(&router, WORKSPACE, create_body(&input, None)).await;
    assert_eq!(
        status, 202,
        "the narrowest accepted trim range got {status} {accepted}"
    );
}

#[tokio::test]
async fn a_retried_create_carrying_the_same_client_token_replays_the_first_job() {
    let (router, engine) = router_with_engine();
    let input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let token = "44444444-4444-4444-4444-444444444444";
    let other_token = "55555555-5555-5555-5555-555555555555";

    let (status, first) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
    assert_eq!(status, 202, "the first create got {status} {first}");
    let job_id = first["jobId"].as_str().unwrap_or("").to_string();

    // (case, workspace, token, whether the answer must be the first job's id)
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        ("the same token again", WORKSPACE, Some(token), true),
        ("the same token once more", WORKSPACE, Some(token), true),
        ("another token", WORKSPACE, Some(other_token), false),
        ("no token at all", WORKSPACE, None, false),
    ];

    for (label, workspace, retry_token, replayed) in rows {
        let (status, body) = create(&router, workspace, create_body(&input, retry_token)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {body}"
        );
        let answered = body["jobId"].as_str().unwrap_or("");
        if replayed {
            assert_eq!(
                answered, job_id,
                "case {label:?}: a retry must replay the job the first create started, got {body}"
            );
        } else {
            assert_ne!(
                answered, job_id,
                "case {label:?}: a create that is not a retry must start a job of its own, \
                 got {body}"
            );
        }
        let (status, described) = describe(&router, workspace, answered).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the answered job must read back, got {status} {described}"
        );
    }

    // Three jobs: the one the token started, and the two creates that were not
    // retries of it.
    let held = jobs_held(&engine);
    assert_eq!(
        held, 3,
        "the two replays must have written nothing, the store holds {held}"
    );
}

#[tokio::test]
async fn a_retry_replays_the_first_request_and_a_differing_one_is_refused() {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Export source").await;
    let first_input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let token = "66666666-6666-6666-6666-666666666666";

    let (status, first) = create(&router, WORKSPACE, create_body(&first_input, Some(token))).await;
    assert_eq!(status, 202, "the first create got {status} {first}");
    let job_id = first["jobId"].as_str().unwrap_or("").to_string();

    // (case, the whole body the second call sends, whether it REPEATS the first
    // request) — the token is the one the first create spent, and the rows differ
    // in each member the create is recognised by in turn: the repeat replays that
    // call's answer, every differing request repeats no call at all.
    let calls: Vec<(&str, Value, bool)> = vec![
        (
            "the same request again",
            create_body(&first_input, Some(token)),
            true,
        ),
        (
            "another destination",
            create_body_from(OTHER_DESTINATION, ERROR_REPORT, &first_input, Some(token)),
            false,
        ),
        (
            "another error report location",
            create_body_from(DESTINATION, OTHER_ERROR_REPORT, &first_input, Some(token)),
            false,
        ),
        (
            "another processing input",
            create_body(&json!({ "dataset": { "datasetId": dataset_id } }), Some(token)),
            false,
        ),
    ];

    for (label, body, repeats) in calls {
        let (status, answered) = create(&router, WORKSPACE, body).await;
        if repeats {
            assert_eq!(
                status, 202,
                "case {label:?}: a retry expected 202, got {status} {answered}"
            );
            assert_eq!(
                answered["jobId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: a retry must replay the job the first create started, \
                 got {answered}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {label:?}: a request differing from the call its token was spent on \
                 expected 409, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ConflictingOperationException",
                "case {label:?}: unexpected error code in {answered}"
            );
            assert_eq!(
                answered["resourceId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: the conflict must name the job the token's own call \
                 started, got {answered}"
            );
            assert!(
                answered["resourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(&format!("{WORKSPACE}/{job_id}")),
                "case {label:?}: the conflict must carry that job's ARN, got {answered}"
            );
        }

        let (status, described) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read of the first job got {status} {described}"
        );
        assert_eq!(
            described["destinationS3Uri"].as_str().unwrap_or(""),
            DESTINATION,
            "case {label:?}: the second call must not rewrite the job's destination, \
             got {described}"
        );
        assert_eq!(
            described["errorReportLocation"],
            json!({ "s3Uri": ERROR_REPORT }),
            "case {label:?}: the second call must not rewrite the job's error report \
             location, got {described}"
        );
        assert_eq!(
            described["input"], first_input,
            "case {label:?}: the second call must not rewrite the job's processing input, \
             got {described}"
        );
    }

    // A token is spent by one CALL of this operation, and the workspace the create
    // names is a member of that call: the same token in ANOTHER workspace is a
    // different request, so it repeats no call and is refused too — the rule
    // `StartSearch` already states for a token reused against another workspace.
    let (status, elsewhere) =
        create(&router, "other-floor", create_body(&first_input, Some(token))).await;
    assert_eq!(
        status, 409,
        "the same token in another workspace expected 409, got {status} {elsewhere}"
    );
    assert_eq!(
        error_code(&elsewhere),
        "ConflictingOperationException",
        "the same token in another workspace: unexpected error code in {elsewhere}"
    );
    assert_eq!(
        elsewhere["resourceId"].as_str().unwrap_or(""),
        job_id,
        "the conflict must name the job the token's own call started, got {elsewhere}"
    );

    // One job: the one the first create started. The replay and the four refusals
    // wrote nothing.
    let held = jobs_held(&engine);
    assert_eq!(
        held, 1,
        "the replay and the refused creates must have written nothing, the store holds {held}"
    );
}

#[tokio::test]
async fn a_job_is_created_in_the_workspace_the_request_names_whatever_its_form() {
    let router = router();
    let input = json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] });

    // The forms `WorkspaceName` admits (len 1..64, pattern ^[a-zA-Z0-9_-]+$ —
    // smithy_query query="shape" shape="WorkspaceName"): the shortest name, a
    // digit-only one, one spanning every character class, and the longest.
    let longest = "w".repeat(64);
    let workspaces: [&str; 4] = ["a", "0", "Plant_Floor-7", longest.as_str()];

    let mut minted: Vec<String> = Vec::new();
    for workspace in workspaces {
        let (status, created) = create(&router, workspace, create_body(&input, None)).await;
        assert_eq!(
            status, 202,
            "case {workspace:?}: expected 202, got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("");
        assert!(
            is_modelled_id(job_id),
            "case {workspace:?}: jobId must have the modelled form, got {created}"
        );
        assert_eq!(
            created["workspaceName"].as_str().unwrap_or(""),
            workspace,
            "case {workspace:?}: a create answers the workspace it was asked for, got {created}"
        );

        let (status, described) = describe(&router, workspace, job_id).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: the read got {status} {described}"
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            workspace,
            "case {workspace:?}: unexpected workspaceName in {described}"
        );
        assert_eq!(
            described["input"], input,
            "case {workspace:?}: unexpected processing input in {described}"
        );
        minted.push(job_id.to_string());
    }

    for (at, job_id) in minted.iter().enumerate() {
        assert!(
            !minted[at + 1..].contains(job_id),
            "case {:?}: every create mints a job id of its own, {job_id} was minted twice",
            workspaces[at]
        );
    }
}

#[tokio::test]
async fn a_workspace_holds_no_more_jobs_than_the_store_bounds_it_to() {
    let (router, engine) = router_with_engine();
    let input = json!({ "timeseries": [{ "propertyAlias": "/plant/line-1/rate" }] });

    for at in 0..MAX_DATASET_EXPORT_JOBS_PER_WORKSPACE {
        let (status, body) = create(&router, WORKSPACE, create_body(&input, None)).await;
        assert_eq!(
            status, 202,
            "job {at} of the bound: expected 202, got {status} {body}"
        );
    }

    // One past the bound the create is the modelled refusal, and it starts no job.
    let (status, refused) = create(&router, WORKSPACE, create_body(&input, None)).await;
    assert_eq!(
        status, 429,
        "one past the bound: expected 429, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ThrottlingException",
        "one past the bound: unexpected error code in {refused}"
    );

    // The bound is per workspace, so another workspace still accepts a create.
    let (status, accepted) = create(&router, "other-floor", create_body(&input, None)).await;
    assert_eq!(
        status, 202,
        "a create in another workspace got {status} {accepted}"
    );

    let held = jobs_held(&engine);
    assert_eq!(
        held,
        MAX_DATASET_EXPORT_JOBS_PER_WORKSPACE + 1,
        "the refused create must have started no job, the store holds {held}"
    );
}

// ---------------------------------------------------------------------------
// A job's NAME.
//
// The rule these suites pin: a dataset export job carries a name, resolved once
// by the create — the `jobName` the request carried, kept verbatim, or, for a
// request that carried none, one this service builds from the kind of export and
// the instant the job started — and every read of that job reports it.
// ---------------------------------------------------------------------------

/// The published bounds of the name a create may carry (smithy_query
/// query="shape" shape="DatasetExportJobNameInput": `@length(min: 1, max: 256)`,
/// `pattern ^[^\x00-\x1f\x7f]+$`).
const JOB_NAME_MIN: usize = 1;
/// See [`JOB_NAME_MIN`].
const JOB_NAME_MAX: usize = 256;

/// The upper bound of the name a READ reports, which is the wider shape
/// (smithy_query query="shape" shape="DatasetExportJobName":
/// `@length(min: 1, max: 350)`).
const REPORTED_JOB_NAME_MAX: usize = 350;

/// The members `ProcessingInput` declares (smithy_query query="shape"
/// shape="ProcessingInput"), which are what a create says the KIND of its export
/// with.
const INPUT_KINDS: [&str; 2] = ["timeseries", "dataset"];

/// The body a create sends, with `jobName` only when the row carries one.
fn create_body_named(input: &Value, job_name: Option<&str>, token: Option<&str>) -> Value {
    let mut body = create_body(input, token);
    if let Some(name) = job_name {
        body.as_object_mut()
            .expect("an object body")
            .insert("jobName".to_string(), json!(name));
    }
    body
}

/// `ListDatasetExportJobs` on `workspace`, answering the summary of `job_id`.
async fn list_summary(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> Value {
    let (status, body) = call(router, "GET", &jobs_uri(workspace), None).await;
    assert_eq!(
        status, 200,
        "list {workspace}: expected 200, got {status} {body}"
    );
    body["jobs"]
        .as_array()
        .and_then(|jobs| jobs.iter().find(|job| job["jobId"] == json!(job_id)))
        .cloned()
        .unwrap_or_else(|| panic!("list {workspace}: no summary for {job_id} in {body}"))
}

/// The instant the store says the job started, rendered the way this service's
/// timestamps are rendered on the wire.
///
/// Read from the store rather than from a response: a read reports `startedAt` as
/// a JSON number, which cannot be turned back into the instant's full precision.
fn stored_start_instant(engine: &engine::InMemoryEngine, workspace: &str, job_id: &str) -> String {
    let state = service_state(engine).expect("the service store");
    let store = state.lock();
    let record = store
        .dataset_export_jobs
        .get(&(workspace.to_string(), job_id.to_string()))
        .unwrap_or_else(|| panic!("the store holds no job {job_id} in {workspace}"));
    ::aws_smithy_types::DateTime::from(record.started_at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable instant")
}

/// Whether `name` is a value the member a read reports it through admits: within
/// the modelled length, and carrying none of the characters its pattern excludes
/// (the C0 controls and DEL).
fn is_reportable_name(name: &str) -> bool {
    let characters = name.chars().count();
    (JOB_NAME_MIN..=REPORTED_JOB_NAME_MAX).contains(&characters)
        && !name.chars().any(char::is_control)
}

/// One row of the name table: a `jobName` a create carries.
struct NameCase {
    label: &'static str,
    name: String,
    /// Whether `DatasetExportJobNameInput`'s constraints admit it at all.
    admitted: bool,
}

/// The rows: `DatasetExportJobNameInput` is `@length 1..256` with
/// `pattern ^[^\x00-\x1f\x7f]+$` (smithy_query query="shape"
/// shape="DatasetExportJobNameInput"), so the forms walk both published bounds, a
/// punctuated name and a non-ASCII one between them — the pattern excludes only
/// the C0 controls and DEL — plus one character past each bound and a name
/// carrying an excluded character.
fn job_name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            label: "one character, the shortest the length admits",
            name: "x".to_string(),
            admitted: true,
        },
        NameCase {
            label: "a punctuated and spaced name between the bounds",
            name: "Nightly export — plant 3 / line 7 (v2)".to_string(),
            admitted: true,
        },
        NameCase {
            label: "a non-ASCII name between the bounds",
            name: "设备导出 ß ø π".to_string(),
            admitted: true,
        },
        NameCase {
            label: "the longest the length admits",
            name: "n".repeat(JOB_NAME_MAX),
            admitted: true,
        },
        NameCase {
            label: "one character past the longest",
            name: "n".repeat(JOB_NAME_MAX + 1),
            admitted: false,
        },
        NameCase {
            label: "one character short of the shortest",
            name: String::new(),
            admitted: false,
        },
        NameCase {
            label: "a name carrying a character the pattern excludes",
            name: "tab\there".to_string(),
            admitted: false,
        },
    ]
}

#[tokio::test]
async fn a_job_reads_back_under_the_name_its_create_carried() {
    for case in job_name_cases() {
        let label = case.label;
        let name = case.name.as_str();
        // A router of its own per row, so the job count below is about THIS row.
        let (router, engine) = router_with_engine();
        let input = json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] });

        let (status, created) =
            create(&router, WORKSPACE, create_body_named(&input, Some(name), None)).await;

        if !case.admitted {
            assert_eq!(
                status, 400,
                "case {label:?}: a name outside what the member admits expected 400, \
                 got {status} {created}"
            );
            let code = error_code(&created);
            assert!(
                matches!(code, "InvalidRequestException" | "ValidationException"),
                "case {label:?}: expected a client refusal of the name, got {code} in {created}"
            );
            let held = jobs_held(&engine);
            assert_eq!(
                held, 0,
                "case {label:?}: a refused create must start no job, the store holds {held}"
            );
            continue;
        }

        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {label:?}: jobId must have the modelled form, got {created}"
        );

        let (status, described) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read expected 200, got {status} {described}"
        );
        assert_eq!(
            described["jobName"].as_str().unwrap_or_default(),
            name,
            "case {label:?}: a read must report the name the create carried, got {described}"
        );

        let summary = list_summary(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            summary["jobName"].as_str().unwrap_or_default(),
            name,
            "case {label:?}: a listed job must carry the same name the read reports, \
             got {summary}"
        );
    }
}

#[tokio::test]
async fn a_create_that_names_no_job_gets_one_built_from_the_type_and_the_start_time() {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Unnamed export source").await;

    // One row per member `ProcessingInput` declares: the kind of export is one of
    // the two things a generated name is built from.
    let inputs: [(&str, Value); 2] = [
        (
            INPUT_KINDS[0],
            json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] }),
        ),
        (
            INPUT_KINDS[1],
            json!({ "dataset": { "datasetId": dataset_id } }),
        ),
    ];

    let mut generated: Vec<String> = Vec::new();
    for (kind, input) in inputs {
        let (status, created) = create(&router, WORKSPACE, create_body(&input, None)).await;
        assert_eq!(
            status, 202,
            "case {kind:?}: expected 202, got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("").to_string();

        let (status, described) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the read expected 200, got {status} {described}"
        );
        let name = described["jobName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_reportable_name(&name),
            "case {kind:?}: a job with no name of its own must read back under a name the \
             member admits, got {name:?}"
        );
        assert!(
            name.contains(kind),
            "case {kind:?}: the created name must state the kind of export, got {name:?}"
        );
        let other = INPUT_KINDS
            .iter()
            .find(|candidate| **candidate != kind)
            .expect("the other kind");
        assert!(
            !name.contains(*other),
            "case {kind:?}: the created name must not state another kind of export, \
             got {name:?}"
        );
        let started = stored_start_instant(&engine, WORKSPACE, &job_id);
        assert!(
            name.contains(&started),
            "case {kind:?}: the created name must state the instant the job started \
             ({started}), got {name:?}"
        );

        // The name is resolved ONCE, by the create: a second read and the listing
        // report the same one.
        let (status, again) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the second read expected 200, got {status} {again}"
        );
        assert_eq!(
            again["jobName"].as_str().unwrap_or_default(),
            name,
            "case {kind:?}: two reads of one job must report one name, got {again}"
        );
        let summary = list_summary(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            summary["jobName"].as_str().unwrap_or_default(),
            name,
            "case {kind:?}: a listed job must carry the same name the read reports, \
             got {summary}"
        );
        generated.push(name);
    }

    assert_ne!(
        generated[0], generated[1],
        "two jobs of different kinds must not be created under one name, got {generated:?}"
    );
}

#[tokio::test]
async fn a_jobs_name_is_one_of_the_members_its_client_token_identifies() {
    let (router, engine) = router_with_engine();
    let input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let token = "44444444-4444-4444-4444-444444444444";
    let named = "Nightly plant-3 export";

    let (status, first) = create(
        &router,
        WORKSPACE,
        create_body_named(&input, Some(named), Some(token)),
    )
    .await;
    assert_eq!(
        status, 202,
        "the first create expected 202, got {status} {first}"
    );
    let job_id = first["jobId"].as_str().unwrap_or("").to_string();

    // The rows: the SAME request under that token repeats the call it was spent
    // on, and a request differing only in the name it gives the job repeats no
    // call — a name present where the first carried none is such a difference too.
    let calls = vec![
        (
            "the same request under the token",
            create_body_named(&input, Some(named), Some(token)),
            true,
        ),
        (
            "another jobName under the token",
            create_body_named(&input, Some("Hourly plant-3 export"), Some(token)),
            false,
        ),
        (
            "no jobName under the token",
            create_body(&input, Some(token)),
            false,
        ),
    ];

    for (label, body, repeats) in calls {
        let (status, answered) = create(&router, WORKSPACE, body).await;
        if repeats {
            assert_eq!(
                status, 202,
                "case {label:?}: a retry expected 202, got {status} {answered}"
            );
            assert_eq!(
                answered["jobId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: a retry must replay the job the first create started, \
                 got {answered}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {label:?}: a request differing from the call its token was spent on \
                 expected 409, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ConflictingOperationException",
                "case {label:?}: unexpected error code in {answered}"
            );
            assert_eq!(
                answered["resourceId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: the conflict must name the job the token's own call \
                 started, got {answered}"
            );
        }

        let (status, described) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read of the first job got {status} {described}"
        );
        assert_eq!(
            described["jobName"].as_str().unwrap_or_default(),
            named,
            "case {label:?}: the second call must not rewrite the job's name, got {described}"
        );
    }

    let held = jobs_held(&engine);
    assert_eq!(
        held, 1,
        "the replay and the refused creates must have written nothing, the store holds {held}"
    );
}
