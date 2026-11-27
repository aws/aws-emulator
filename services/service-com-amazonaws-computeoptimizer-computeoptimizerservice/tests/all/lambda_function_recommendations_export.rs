// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportLambdaFunctionRecommendations`.
//!
//! Every call goes through the same `MultiServiceRouter` dispatch a client uses, so what is
//! asserted is what a client sees. The RULES asserted here are the ones this export shares
//! with every other `Export*Recommendations` operation of the service — it RECORDS one job
//! for the Region its endpoint serves, answers that job's id and the destination keys derived
//! from the request's bucket and key prefix, the account and the resource type, the job's
//! `JobStatus` then walks the lifecycle the clock drives, and only one Lambda function export
//! job runs per Region at a time — plus the ones specific to it: the job carries the
//! `LambdaFunction` resource type, its in-progress limit counts Lambda function jobs only,
//! and the request is refused for every input rule the operation documents. Each is checked
//! over a TABLE of inputs rather than one call.
//!
//! The reader of what these calls record, `DescribeRecommendationExportJobs`, has its own
//! suite (`recommendation_export_jobs.rs`); it is used here only to read a created job back.
//!
//! The second half of this suite is about the CONTENT of the file this export writes: a
//! recommendations export file reports, in each cell, the value the reader of the SAME analysis
//! reports for the recommendation member that exportable field names, and a cell is empty only
//! where the analysis holds no value for it. So every `ExportableLambdaFunctionField` a request
//! selects — the version analysed, the function's own configuration, the utilization observed
//! on it, the memory size its option would move it to with the durations and costs that option
//! carries, its tags and what adopting it saves — must carry what
//! `GetLambdaFunctionRecommendations` reports for the same function version, and the file holds
//! exactly the recommendations that reader answers the same filter list with.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    round_hundredths, EXPORT_JOB_STEP, FINDING_NOT_OPTIMIZED, FINDING_OPTIMIZED,
    JOB_STATUS_COMPLETE, JOB_STATUS_IN_PROGRESS, JOB_STATUS_QUEUED, LAMBDA_FILTER_FINDING,
    LAMBDA_FILTER_FINDING_REASON_CODE, LAMBDA_FINDING_UNAVAILABLE,
    LAMBDA_PRICE_PER_GB_SECOND, LAMBDA_PRICE_PER_REQUEST, LAMBDA_REASON_INCONCLUSIVE,
    LAMBDA_REASON_INSUFFICIENT_DATA, MB_PER_GB, METRIC_REASON_MEMORY_OVERPROVISIONED,
    MILLISECONDS_PER_SECOND, RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in_region, call_lambda, cell, create_bucket, emulator, emulator_with_engine,
    error_code, exported_recommendation_rows, exported_recommendation_rows_selecting,
    make_analysis_due, opt_in, publish_function_runs, publish_function_version,
    register_analysed_functions, ObservedRuns, ACCOUNT, LATEST_VERSION, REGION,
};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it;
/// the request just names it.
const BUCKET: &str = "lambda-recommendation-exports";

/// Enough slack over one lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the step itself is the service's own ([`EXPORT_JOB_STEP`]).
const SLACK: Duration = Duration::from_millis(120);

/// An emulator with the account opted in, which every export call requires.
async fn enrolled_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    router
}

/// Export the account's Lambda function recommendations, asserting the call was accepted, and
/// answer the whole response body.
async fn export_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(
        router,
        "ExportLambdaFunctionRecommendations",
        request.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// Export with the default destination and answer the job id.
async fn export_job_id(router: &MultiServiceRouter, case: &str) -> String {
    let body = export_ok(
        router,
        case,
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a jobId, got {body}"))
        .to_string()
}

/// Read the export jobs of the request's Region back, asserting the read was accepted.
async fn describe_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "DescribeRecommendationExportJobs", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// The job ids a `DescribeRecommendationExportJobs` body reports, in the order it reported
/// them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("the reader reports a job list, got {body}"))
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The one job the reader reports for `job_id`.
fn reported_job(case: &str, body: &Value, job_id: &str) -> Value {
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"))
        .iter()
        .find(|job| job["jobId"] == json!(job_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no job reported for {job_id}, got {body}"))
}

/// The destination keys an export answers are derived from the bucket and key prefix the
/// request named, the account the export is for, the resource type and the job id — one row
/// per form a key prefix can take, including none at all — and the job the call recorded
/// names exactly the objects the call answered.
#[tokio::test]
async fn the_destination_is_derived_from_the_request_and_the_minted_job_id() {
    let account = engine::emulator_account_id();

    // (case, the `keyPrefix` the request carries, the prefix the keys are expected under)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no key prefix at all", None, ""),
        ("an empty key prefix", Some(""), ""),
        ("a single-segment prefix", Some("exports"), "exports/"),
        (
            "a prefix with a trailing slash",
            Some("exports/"),
            "exports/",
        ),
        (
            "a prefix with leading and trailing slashes",
            Some("/nested/exports/"),
            "nested/exports/",
        ),
        ("a multi-segment prefix", Some("a/b/c"), "a/b/c/"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, key_prefix, expected_prefix) in rows {
        // A fresh emulator per row: one Lambda function export job may be in progress per
        // Region at a time, so each row gets its own store rather than waiting on the
        // previous row's job.
        let router = enrolled_emulator().await;
        let mut destination_config = json!({"bucket": BUCKET});
        if let Some(key_prefix) = key_prefix {
            destination_config["keyPrefix"] = json!(key_prefix);
        }
        let body = export_ok(
            &router,
            case,
            json!({"s3DestinationConfig": destination_config}),
        )
        .await;

        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {body}"))
            .to_string();
        // A `JobId` is an opaque string the service mints; this emulator hands out a
        // UUID-shaped one, which is what AWS's own job ids look like.
        assert_eq!(
            job_id.len(),
            36,
            "case {case:?}: expected a 36-character job id, got {job_id:?}"
        );
        assert!(
            job_id
                .char_indices()
                .all(|(at, character)| if matches!(at, 8 | 13 | 18 | 23) {
                    character == '-'
                } else {
                    character.is_ascii_hexdigit()
                }),
            "case {case:?}: expected a UUID-shaped job id, got {job_id:?}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every export job gets its own id, got {job_id} twice"
        );
        minted.push(job_id.clone());

        let stem = format!(
            "{expected_prefix}{account}/ComputeOptimizer/{RESOURCE_TYPE_LAMBDA_FUNCTION}/{job_id}"
        );
        assert_eq!(
            body["s3Destination"],
            json!({
                "bucket": BUCKET,
                "key": format!("{stem}.csv"),
                "metadataKey": format!("{stem}-metadata.json"),
            }),
            "case {case:?}: unexpected destination, got {body}"
        );

        // The job the call recorded names exactly the objects the call answered, and carries
        // the resource type whose recommendations were exported.
        let listed = describe_ok(&router, case, json!({})).await;
        assert_eq!(
            reported_ids(&listed),
            vec![job_id.clone()],
            "case {case:?}: the export records exactly one job, got {listed}"
        );
        let job = reported_job(case, &listed, &job_id);
        assert_eq!(
            job["destination"],
            json!({"s3": body["s3Destination"].clone()}),
            "case {case:?}: the reported destination must be the one the export answered, \
             got {job}"
        );
        assert_eq!(
            job["resourceType"].as_str(),
            Some(RESOURCE_TYPE_LAMBDA_FUNCTION),
            "case {case:?}: a Lambda function recommendations export is of \
             {RESOURCE_TYPE_LAMBDA_FUNCTION}, got {job}"
        );
        assert!(
            job["failureReason"].is_null(),
            "case {case:?}: a job that did not fail carries no failure reason, got {job}"
        );
    }
}

/// The job a Lambda function export records walks the lifecycle the clock owes: `Queued` while
/// the caller can still see it queued, then `InProgress`, then the terminal `Complete`, which
/// never moves again. Reading it never moves it.
#[tokio::test]
async fn the_recorded_job_status_walks_the_lifecycle() {
    let router = enrolled_emulator().await;
    let job_id = export_job_id(&router, "a job to follow").await;

    let created = reported_job(
        "straight after the export",
        &describe_ok(&router, "straight after the export", json!({})).await,
        &job_id,
    );
    assert_eq!(
        created["status"].as_str(),
        Some(JOB_STATUS_QUEUED),
        "a job the clock has not moved yet is Queued, got {created}"
    );
    let created_at = created["creationTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("a reported job carries a creation timestamp, got {created}"));
    assert_eq!(
        created["lastUpdatedTimestamp"].as_f64(),
        Some(created_at),
        "a job that has not changed yet was last updated when it was created, got {created}"
    );

    // (case, how long to wait before reading, the status expected then)
    let steps: Vec<(&str, Duration, &str)> = vec![
        (
            "one step in",
            EXPORT_JOB_STEP + SLACK,
            JOB_STATUS_IN_PROGRESS,
        ),
        ("two steps in", EXPORT_JOB_STEP, JOB_STATUS_COMPLETE),
        (
            "well past the last step",
            2 * EXPORT_JOB_STEP,
            JOB_STATUS_COMPLETE,
        ),
    ];
    for (case, wait, expected) in steps {
        tokio::time::sleep(wait).await;
        let job = reported_job(case, &describe_ok(&router, case, json!({})).await, &job_id);
        assert_eq!(
            job["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected}, got {job}"
        );
        assert_eq!(
            job["creationTimestamp"].as_f64(),
            Some(created_at),
            "case {case:?}: the creation timestamp never changes, got {job}"
        );
        // A read reports the status the clock has made due and writes nothing, so the same
        // read twice answers the same job.
        let again = reported_job(case, &describe_ok(&router, case, json!({})).await, &job_id);
        assert_eq!(
            job, again,
            "case {case:?}: a read must not change what a later caller sees"
        );
    }
}

/// "You can have only one Lambda function export job in progress per Amazon Web Services
/// Region": a second Lambda function export is refused while the first is running, admitted
/// once it has completed, and never blocked by a job of another Region — nor by a job of
/// another resource type, whose limit is its own.
#[tokio::test]
async fn only_one_lambda_export_job_runs_per_region_at_a_time() {
    let router = enrolled_emulator().await;
    let first = export_job_id(&router, "the first export").await;

    let (status, body) = call(
        &router,
        "ExportLambdaFunctionRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second export while one is in progress is refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("LimitExceededException"),
        "expected LimitExceededException, got {body}"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "after the refusal", json!({})).await),
        vec![first.clone()],
        "a refused export records no job"
    );

    // An export of another resource type counts against its OWN limit, so the in-progress
    // Lambda function job does not block it and its job does not block a later Lambda
    // function export.
    let (status, body) = call(
        &router,
        "ExportIdleRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an export of another resource type is not blocked by a Lambda function job, got \
         {status}: {body}"
    );
    let idle_job = body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("an accepted export answers a jobId, got {body}"))
        .to_string();
    let listed = describe_ok(&router, "both resource types", json!({})).await;
    assert_eq!(
        reported_job("the idle export", &listed, &idle_job)["resourceType"].as_str(),
        Some(RESOURCE_TYPE_IDLE),
        "an idle resource export is of {RESOURCE_TYPE_IDLE}, got {listed}"
    );

    // Another Region's endpoint has its own in-progress job: the limit is per Region.
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        "ExportLambdaFunctionRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an export at another Region's endpoint is not blocked, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "the home Region", json!({})).await),
        vec![first.clone(), idle_job.clone()],
        "a Region reports only its own export jobs"
    );

    // Once the first job reaches its terminal status it is no longer in progress, so the
    // next export is admitted.
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let second = export_job_id(&router, "after the first completed").await;
    assert_ne!(
        second, first,
        "every export job gets its own id, got {second} twice"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "every job", json!({})).await),
        vec![first, idle_job, second],
        "the reader reports the jobs in creation order"
    );
}

/// An export request outside the service's documented input rules is refused with HTTP 400
/// and records no job — one row per rule, each on a value the rule rejects — and a request
/// that carries every member at a value the model declares is accepted.
#[tokio::test]
async fn an_invalid_export_request_is_refused() {
    let router = enrolled_emulator().await;

    // (case, request, the wire error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            // "You must create the destination Amazon S3 bucket ... Compute Optimizer does
            // not create the S3 bucket for you."
            "a destination naming no bucket",
            json!({"s3DestinationConfig": {}}),
            "InvalidParameterValueException",
        ),
        (
            "a destination whose bucket is blank",
            json!({"s3DestinationConfig": {"bucket": "   "}}),
            "InvalidParameterValueException",
        ),
        (
            // "This parameter cannot be specified together with the include member accounts
            // parameter. The parameters are mutually exclusive."
            "accountIds together with includeMemberAccounts",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "accountIds": [ACCOUNT],
                "includeMemberAccounts": true,
            }),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "accountIds": ["12345"]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not digits at all",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "accountIds": ["abcdefghijkl"]}),
            "InvalidParameterValueException",
        ),
        (
            // The `Finding` filter selects on `LambdaFunctionRecommendationFinding`:
            // Optimized | NotOptimized | Unavailable.
            "a finding outside the LambdaFunctionRecommendationFinding value set",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": LAMBDA_FILTER_FINDING, "values": ["Enormous"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            // A finding of ANOTHER resource type's vocabulary is still not one of this
            // filter's values.
            "a finding of another resource type's finding vocabulary",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": LAMBDA_FILTER_FINDING, "values": ["Overprovisioned"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a reason code outside the LambdaFunctionRecommendationFindingReasonCode set",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": LAMBDA_FILTER_FINDING_REASON_CODE,
                             "values": ["CPUOverprovisioned"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"values": ["Optimized"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            // `FileFormat` is a modelled enum whose only value is Csv, so the request
            // boundary refuses anything else before the handler runs.
            "a file format the model does not declare",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "fileFormat": "Parquet"}),
            "InvalidParameterValueException",
        ),
        (
            // Likewise `ExportableLambdaFunctionField`.
            "a field to export the model does not declare",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "fieldsToExport": ["FunctionArn", "NotAField"],
            }),
            "InvalidParameterValueException",
        ),
        (
            // An `ExportableLambdaFunctionField` of another export's vocabulary is not one
            // of this export's fields.
            "a field to export belonging to another resource type's export",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "fieldsToExport": ["CurrentInstanceType"],
            }),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted_code) in rows {
        let (status, body) = call(
            &router,
            "ExportLambdaFunctionRecommendations",
            request.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains(wanted_code),
            "case {case:?}: expected {wanted_code}, got {body}"
        );
        assert!(
            body["jobId"].is_null(),
            "case {case:?}: a refused export answers no job id, got {body}"
        );
        assert_eq!(
            reported_ids(&describe_ok(&router, case, json!({})).await),
            Vec::<String>::new(),
            "case {case:?}: a refused export records no job"
        );
    }

    // Every field a valid request may carry together, accepted: the rows above refuse a
    // VALUE, never the member. The `fieldsToExport` values are
    // `ExportableLambdaFunctionField` ones and the filter values the
    // `LambdaFunctionRecommendationFinding` and
    // `LambdaFunctionRecommendationFindingReasonCode` sets the state module names.
    let body = export_ok(
        &router,
        "every member set to a value the model declares",
        json!({
            "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "quarterly"},
            "accountIds": [ACCOUNT],
            "includeMemberAccounts": false,
            "fileFormat": "Csv",
            "fieldsToExport": ["FunctionArn", "AccountId", "Finding", "FindingReasonCodes",
                               "CurrentConfigurationMemorySize",
                               "RecommendationOptionsConfigurationMemorySize", "Tags"],
            "filters": [{"name": LAMBDA_FILTER_FINDING,
                         "values": [LAMBDA_FINDING_UNAVAILABLE]},
                        {"name": LAMBDA_FILTER_FINDING_REASON_CODE,
                         "values": [LAMBDA_REASON_INCONCLUSIVE,
                                    METRIC_REASON_MEMORY_OVERPROVISIONED]}],
        }),
    )
    .await;
    assert!(
        body["jobId"].is_string(),
        "an accepted export answers a job id, got {body}"
    );
}

/// The enrollment gate: "The account is not opted in to Compute Optimizer" is what the export
/// answers while the account is not enrolled, before and after opting back out, and no job is
/// recorded either time.
#[tokio::test]
async fn an_export_is_refused_while_the_account_is_not_opted_in() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    let request = json!({"s3DestinationConfig": {"bucket": BUCKET}});

    for case in ["before enrolling", "after opting back out"] {
        let (status, body) = call(
            &router,
            "ExportLambdaFunctionRecommendations",
            request.clone(),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 while not opted in, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "case {case:?}: expected OptInRequiredException, got {body}"
        );

        if case == "before enrolling" {
            // Enrolling is what admits the export; the store the refused call left alone is
            // still empty, and the export then records its one job.
            opt_in(&router).await;
            assert_eq!(
                reported_ids(&describe_ok(&router, case, json!({})).await),
                Vec::<String>::new(),
                "case {case:?}: a refused export records no job"
            );
            let job_id = export_job_id(&router, case).await;
            assert_eq!(
                reported_ids(&describe_ok(&router, case, json!({})).await),
                vec![job_id],
                "case {case:?}: the export recorded its job once the account was enrolled"
            );

            let (status, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": "Inactive", "includeMemberAccounts": false}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: opting back out: got {status}: {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The CONTENT of the file this export writes
//
// The reader of the same analysis is the ORACLE for every cell, so no expectation below
// restates a figure this analysis happens to hold: the columns that name a member of a
// `LambdaFunctionRecommendation` are compared against what that reader reports for the same
// function version, and the few that name none — the configured timeout and the four cost
// columns — against the service that owns the function and against what the reader's own
// reported members bill at the public prices this service prices a function with.
// ---------------------------------------------------------------------------

/// The destination bucket the content fixtures export to, apart from the job fixtures above so
/// neither sees the other's objects.
const CONTENT_BUCKET: &str = "lambda-recommendation-export-files";

/// The export under test, and the reader of the same analysis.
const EXPORT: &str = "ExportLambdaFunctionRecommendations";
/// See [`EXPORT`].
const READER: &str = "GetLambdaFunctionRecommendations";

/// How a cell holding several values separates them in the export file, so one exportable field
/// is still one column. AWS publishes no layout for the file, so this is the emulator's own,
/// asserted here as what a client reading the file sees.
const LIST_SEPARATOR: &str = "|";

/// The column whose cell states an instant rather than text.
const TIMESTAMP_COLUMN: &str = "LastRefreshTimestamp";

/// The function this suite publishes a version of and re-configures, so the exported rows span
/// both forms a function version takes and two configurations of one function.
const VERSIONED_FUNCTION: &str = "invoice-renderer";

/// The `Timeout` the `$LATEST` version of that function is moved to, in seconds — inside
/// Lambda's own range for the member, and not the default a created function carries, so the
/// column cannot pass by stating that default.
const RECONFIGURED_TIMEOUT_SECS: i64 = 42;

/// The columns of the Lambda function field set that name no member of a
/// `LambdaFunctionRecommendation`: the timeout the version is configured with, and the cost of
/// the configuration in force and of the one its option would move it to. Each is asserted on
/// its own oracle rather than on the reader's answer.
const DERIVED_COLUMNS: &[&str] = &[
    "CurrentConfigurationTimeout",
    "CurrentCostTotal",
    "CurrentCostAverage",
    "RecommendationOptionsCostLow",
    "RecommendationOptionsCostHigh",
];

/// An emulator with the account's own analysed Lambda functions, the destination bucket
/// created, the account opted in and the analysis its enrollment starts already due — which is
/// what the reader reports over, so it is what the export file holds.
///
/// The analysis becomes readable because its stored instant is already past
/// ([`make_analysis_due`]), not because the suite waited for the clock to reach it.
async fn analysed_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    register_analysed_functions(&router, &engine).await;
    create_bucket(&router, CONTENT_BUCKET).await;
    opt_in(&router).await;
    make_analysis_due(&engine);
    (router, engine)
}

/// The same emulator, with a PUBLISHED version of one function beside the unpublished ones and
/// that function's `$LATEST` version re-configured: a function VERSION is the analysed
/// resource, so an exported file holds one row per version, each carrying the configuration
/// AWS Lambda holds for THAT version.
async fn versioned_emulator() -> MultiServiceRouter {
    let (router, engine) = analysed_emulator().await;
    let version = publish_function_version(&router, REGION, VERSIONED_FUNCTION).await;
    // The published version's own observed runs, so the analysis reaches a finding for it
    // rather than reporting it unavailable for want of any observation.
    publish_function_runs(
        &engine,
        VERSIONED_FUNCTION,
        &version,
        // The plain-text framing the runtime writes a run report in, which is the format this
        // function's logs are delivered in.
        "Text",
        &[ObservedRuns {
            days_ago: 1.0,
            invocations: 20_000.0,
            durations_ms: &[120.0, 260.0, 640.0],
            memory_used_mb: &[700.0, 880.0],
        }],
    );
    // The unpublished version is re-configured AFTER the version was published, so the two
    // versions of the one function are configured with different timeouts.
    let (status, body) = call_lambda(
        &router,
        REGION,
        "PUT",
        &format!("/2015-03-31/functions/{VERSIONED_FUNCTION}/configuration"),
        Some(json!({"Timeout": RECONFIGURED_TIMEOUT_SECS})),
    )
    .await;
    assert_eq!(
        status, 200,
        "re-configuring {VERSIONED_FUNCTION}: got {status}: {body}"
    );
    assert_eq!(
        body["Timeout"].as_i64(),
        Some(RECONFIGURED_TIMEOUT_SECS),
        "AWS Lambda must hold the timeout the update asked for, got {body}"
    );
    router
}

/// The recommendations the reader reports for `filters`, in the order it reports them.
async fn reader_recommendations(
    router: &MultiServiceRouter,
    case: &str,
    filters: &Value,
) -> Vec<Value> {
    let mut request = json!({"maxResults": 100});
    if filters
        .as_array()
        .is_some_and(|values| !values.is_empty())
    {
        request["filters"] = filters.clone();
    }
    let (status, body) = call(router, READER, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {request}: got {status}: {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the analysis fits one page, got {body}"
    );
    body["lambdaFunctionRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a list, got {body}"))
        .clone()
}

/// A string member of a reported object, or the empty cell where it reports none.
fn text(reported: &Value, member: &str) -> String {
    reported[member].as_str().unwrap_or_default().to_string()
}

/// A number a reported object carries, or `0` for one the protocol omitted because it equals
/// the member's modelled default.
fn number(reported: &Value, member: &str) -> f64 {
    reported[member].as_f64().unwrap_or(0.0)
}

/// A whole-number member as the export file states it.
fn whole_cell(reported: &Value, member: &str) -> String {
    reported[member].as_i64().unwrap_or(0).to_string()
}

/// A figure of a reported OBJECT as the export file states it — at the two decimal places every
/// figure this service reports is rounded to — and the empty cell where the object itself is
/// absent, which is what a column of a member the recommendation does not report holds.
fn figure_cell(parent: &Value, member: &str) -> String {
    if parent.is_object() {
        round_hundredths(number(parent, member)).to_string()
    } else {
        String::new()
    }
}

/// A list-valued member as one cell ([`LIST_SEPARATOR`]).
fn list_cell(reported: &Value) -> String {
    reported
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
                .join(LIST_SEPARATOR)
        })
        .unwrap_or_default()
}

/// The tags a recommendation reports as one cell: `key=value` pairs.
fn tags_cell(reported: &Value) -> String {
    reported
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    format!(
                        "{}={}",
                        tag["key"].as_str().unwrap_or_default(),
                        tag["value"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<String>>()
                .join(LIST_SEPARATOR)
        })
        .unwrap_or_default()
}

/// The metric `metrics` reports under `name` at `statistic`, as the export file states it, and
/// the empty cell for a metric it reports none of.
fn metric_cell(metrics: &Value, name: &str, statistic: &str) -> String {
    reported_metric(metrics, name, statistic)
        .map_or_else(String::new, |value| round_hundredths(value).to_string())
}

/// The value one reported metric carries, or `None` where no such metric is reported at all.
fn reported_metric(metrics: &Value, name: &str, statistic: &str) -> Option<f64> {
    metrics
        .as_array()?
        .iter()
        .find(|metric| {
            metric["name"].as_str() == Some(name) && metric["statistic"].as_str() == Some(statistic)
        })
        .map(|metric| number(metric, "value"))
}

/// The cell the export file must carry under `column` for the recommendation the READER
/// reports, or `None` for a column that names no member of one ([`DERIVED_COLUMNS`] and the
/// timestamp, which the caller compares as an instant).
///
/// Every value of `ExportableLambdaFunctionField` is covered: the members a
/// `LambdaFunctionRecommendation` reports, including those of its one memory size
/// recommendation option, whose columns are empty for a function the analysis reports no option
/// for.
fn expected_cell(column: &str, reported: &Value) -> Option<String> {
    let option = &reported["memorySizeRecommendationOptions"][0];
    let savings = &option["savingsOpportunity"];
    let after = &option["savingsOpportunityAfterDiscounts"];
    let preferences = &reported["effectiveRecommendationPreferences"];
    let projected = &option["projectedUtilizationMetrics"];
    let observed = &reported["utilizationMetrics"];
    let cell = match column {
        "AccountId" => text(reported, "accountId"),
        "FunctionArn" => text(reported, "functionArn"),
        "FunctionVersion" => text(reported, "functionVersion"),
        "Finding" => text(reported, "finding"),
        "FindingReasonCodes" => list_cell(&reported["findingReasonCodes"]),
        "CurrentPerformanceRisk" => text(reported, "currentPerformanceRisk"),
        "NumberOfInvocations" => whole_cell(reported, "numberOfInvocations"),
        "LookbackPeriodInDays" => round_hundredths(number(reported, "lookbackPeriodInDays")).to_string(),
        "CurrentConfigurationMemorySize" => whole_cell(reported, "currentMemorySize"),
        "UtilizationMetricsDurationAverage" => metric_cell(observed, "Duration", "Average"),
        "UtilizationMetricsDurationMaximum" => metric_cell(observed, "Duration", "Maximum"),
        "UtilizationMetricsMemoryAverage" => metric_cell(observed, "Memory", "Average"),
        "UtilizationMetricsMemoryMaximum" => metric_cell(observed, "Memory", "Maximum"),
        "RecommendationOptionsConfigurationMemorySize" => {
            if option.is_object() {
                whole_cell(option, "memorySize")
            } else {
                String::new()
            }
        }
        "RecommendationOptionsProjectedUtilizationMetricsDurationLowerBound" => {
            metric_cell(projected, "Duration", "LowerBound")
        }
        "RecommendationOptionsProjectedUtilizationMetricsDurationExpected" => {
            metric_cell(projected, "Duration", "Expected")
        }
        "RecommendationOptionsProjectedUtilizationMetricsDurationUpperBound" => {
            metric_cell(projected, "Duration", "UpperBound")
        }
        "RecommendationOptionsSavingsOpportunityPercentage" => {
            figure_cell(savings, "savingsOpportunityPercentage")
        }
        "RecommendationOptionsEstimatedMonthlySavingsCurrency" => {
            text(&savings["estimatedMonthlySavings"], "currency")
        }
        "RecommendationOptionsEstimatedMonthlySavingsValue" => {
            figure_cell(&savings["estimatedMonthlySavings"], "value")
        }
        "RecommendationOptionsSavingsOpportunityAfterDiscountsPercentage" => {
            figure_cell(after, "savingsOpportunityPercentage")
        }
        "RecommendationOptionsEstimatedMonthlySavingsCurrencyAfterDiscounts" => {
            text(&after["estimatedMonthlySavings"], "currency")
        }
        "RecommendationOptionsEstimatedMonthlySavingsValueAfterDiscounts" => {
            figure_cell(&after["estimatedMonthlySavings"], "value")
        }
        "EffectiveRecommendationPreferencesSavingsEstimationMode" => {
            text(&preferences["savingsEstimationMode"], "source")
        }
        "Tags" => tags_cell(&reported["tags"]),
        TIMESTAMP_COLUMN => return None,
        column if DERIVED_COLUMNS.contains(&column) => return None,
        column => panic!("column {column:?} is not covered by this expectation"),
    };
    Some(cell)
}

/// What `invocations` invocations of `duration_ms` at `memory_mb` bill in USD, at the public
/// AWS Lambda prices this service prices a function with ([`LAMBDA_PRICE_PER_GB_SECOND`] per
/// GB-second of billed duration and [`LAMBDA_PRICE_PER_REQUEST`] per request).
fn billed(invocations: f64, memory_mb: f64, duration_ms: f64) -> f64 {
    let gb_seconds =
        invocations * (duration_ms / MILLISECONDS_PER_SECOND) * (memory_mb / MB_PER_GB);
    gb_seconds * LAMBDA_PRICE_PER_GB_SECOND + invocations * LAMBDA_PRICE_PER_REQUEST
}

/// What one cost column must state for the recommendation the READER reports, computed from
/// that recommendation's OWN members — how often the function ran, the memory in force, the
/// mean invocation it observed, and the memory size and projected durations of its option — or
/// `None` where the recommendation carries no option, whose cost columns are then empty.
fn expected_cost(column: &str, reported: &Value) -> Option<f64> {
    let invocations = number(reported, "numberOfInvocations");
    let look_back_days = number(reported, "lookbackPeriodInDays");
    let option = &reported["memorySizeRecommendationOptions"][0];
    let option_cost = |statistic: &str| {
        billed(
            invocations,
            number(option, "memorySize"),
            reported_metric(&option["projectedUtilizationMetrics"], "Duration", statistic)
                .unwrap_or(0.0),
        )
    };
    let current = billed(
        invocations,
        number(reported, "currentMemorySize"),
        reported_metric(&reported["utilizationMetrics"], "Duration", "Average").unwrap_or(0.0),
    );
    match column {
        "CurrentCostTotal" => Some(current),
        "CurrentCostAverage" => Some(if look_back_days > 0.0 {
            current / look_back_days
        } else {
            0.0
        }),
        "RecommendationOptionsCostLow" => option.is_object().then(|| option_cost("LowerBound")),
        "RecommendationOptionsCostHigh" => option.is_object().then(|| option_cost("UpperBound")),
        column => panic!("column {column:?} is not a cost column"),
    }
}

/// The `Timeout` AWS Lambda — the service that OWNS the function — holds the version a
/// recommendation is about configured with, as the export file states it.
async fn lambda_timeout(router: &MultiServiceRouter, case: &str, reported: &Value) -> String {
    let arn = text(reported, "functionArn");
    let name = arn
        .rsplit(":function:")
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    let version = text(reported, "functionVersion");
    let uri = if version == LATEST_VERSION {
        format!("/2015-03-31/functions/{name}")
    } else {
        format!("/2015-03-31/functions/{name}:{version}")
    };
    let (status, body) = call_lambda(router, REGION, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AWS Lambda must hold {arn}: got {status}: {body}"
    );
    body["Configuration"]["Timeout"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: AWS Lambda states the timeout of {arn}, got {body}")
        })
        .to_string()
}

/// The instant one cell of the timestamp column states, as a count of whole milliseconds —
/// `None` for a cell that states no instant at all.
///
/// The file states the instant in RFC 3339 while a modelled timestamp crosses the wire as a
/// JSON NUMBER, so both sides are compared at the millisecond resolution an AWS JSON timestamp
/// states.
fn stated_millis(cell: &str) -> Option<i64> {
    let at = ::aws_smithy_types::DateTime::from_str(
        cell,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(millis_of(at))
}

/// One `DateTime` as a count of whole milliseconds since the epoch.
fn millis_of(at: ::aws_smithy_types::DateTime) -> i64 {
    at.secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(at.subsec_nanos() / 1_000_000))
}

/// The catalog above is the model's: every `ExportableLambdaFunctionField` has an expectation —
/// so a field the model adds fails here rather than going unasserted — and every column
/// asserted on its own oracle is one of those fields.
#[test]
fn the_expectation_covers_every_exportable_function_field() {
    for field in EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES {
        // `expected_cell` panics on a field it does not cover.
        let _ = expected_cell(field, &json!({}));
    }
    for column in DERIVED_COLUMNS {
        assert!(
            EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES.contains(column),
            "column {column:?} is not an ExportableLambdaFunctionField"
        );
    }
}

/// The export file holds the recommendations `GetLambdaFunctionRecommendations` reports for the
/// same analysis: one row per recommendation, in the order the reader reports them, every cell
/// the value that reader reports for the `ExportableLambdaFunctionField` naming the column —
/// and the header is exactly the fields the request asked for, or the whole modelled field set
/// when it asked for none. One row per field selection.
#[tokio::test]
async fn the_export_file_holds_what_the_function_reader_reports() {
    let whole_set: Vec<&str> = EXPORTABLE_LAMBDA_FUNCTION_FIELD_VALUES.to_vec();

    // (case, the `fieldsToExport` the request carries, the columns the file must then carry)
    type Selection = (&'static str, Vec<&'static str>, Vec<&'static str>);
    let selections: Vec<Selection> = vec![
        (
            "the version analysed, the function's own configuration and what was observed",
            vec![
                "AccountId",
                "FunctionArn",
                "FunctionVersion",
                "Finding",
                "FindingReasonCodes",
                "NumberOfInvocations",
                "CurrentConfigurationMemorySize",
                "CurrentConfigurationTimeout",
                "UtilizationMetricsDurationMaximum",
                "UtilizationMetricsMemoryMaximum",
                "CurrentCostTotal",
                "CurrentCostAverage",
                "Tags",
            ],
            vec![
                "AccountId",
                "FunctionArn",
                "FunctionVersion",
                "Finding",
                "FindingReasonCodes",
                "NumberOfInvocations",
                "CurrentConfigurationMemorySize",
                "CurrentConfigurationTimeout",
                "UtilizationMetricsDurationMaximum",
                "UtilizationMetricsMemoryMaximum",
                "CurrentCostTotal",
                "CurrentCostAverage",
                "Tags",
            ],
        ),
        (
            // A field asked for twice is one column, so neither the file nor the work of
            // building it grows with a list that repeats.
            "the option, its projections, its costs and its savings, one field asked for twice",
            vec![
                "FunctionArn",
                "RecommendationOptionsConfigurationMemorySize",
                "RecommendationOptionsProjectedUtilizationMetricsDurationLowerBound",
                "RecommendationOptionsProjectedUtilizationMetricsDurationExpected",
                "RecommendationOptionsProjectedUtilizationMetricsDurationUpperBound",
                "RecommendationOptionsCostLow",
                "RecommendationOptionsCostHigh",
                "RecommendationOptionsSavingsOpportunityPercentage",
                "RecommendationOptionsEstimatedMonthlySavingsCurrency",
                "RecommendationOptionsEstimatedMonthlySavingsValue",
                "FunctionArn",
            ],
            vec![
                "FunctionArn",
                "RecommendationOptionsConfigurationMemorySize",
                "RecommendationOptionsProjectedUtilizationMetricsDurationLowerBound",
                "RecommendationOptionsProjectedUtilizationMetricsDurationExpected",
                "RecommendationOptionsProjectedUtilizationMetricsDurationUpperBound",
                "RecommendationOptionsCostLow",
                "RecommendationOptionsCostHigh",
                "RecommendationOptionsSavingsOpportunityPercentage",
                "RecommendationOptionsEstimatedMonthlySavingsCurrency",
                "RecommendationOptionsEstimatedMonthlySavingsValue",
            ],
        ),
        (
            // A request that selects no field gets every field the model declares for the
            // resource type (`ExportableLambdaFunctionField`).
            "no field selection at all",
            Vec::new(),
            whole_set,
        ),
    ];

    for (case, fields, columns) in selections {
        // A fresh emulator per row: one Lambda function export job is in progress per Region at
        // a time, so each row gets its own store rather than waiting on the previous row's job.
        let router = versioned_emulator().await;
        let reported = reader_recommendations(&router, case, &json!([])).await;
        assert!(
            reported.len() > 1,
            "case {case:?}: the analysis holds several function recommendations"
        );

        let rows =
            exported_recommendation_rows(&router, case, EXPORT, CONTENT_BUCKET, &fields).await;
        let header = rows.first().cloned().unwrap_or_default();
        assert_eq!(
            header, columns,
            "case {case:?}: the header row is the fields the request asked for, got {rows:?}"
        );
        let data = rows.get(1..).unwrap_or_default();
        assert_eq!(
            data.len(),
            reported.len(),
            "case {case:?}: expected one row per reported recommendation ({}), got {rows:?}",
            reported.len()
        );

        for (row, recommendation) in data.iter().zip(&reported) {
            let function = text(recommendation, "functionArn");
            for column in &header {
                let held = cell(&header, row, column);
                if column == TIMESTAMP_COLUMN {
                    // "The timestamp of when the function recommendation was last generated":
                    // the instant the reader reports for the same recommendation.
                    let expected = recommendation["lastRefreshTimestamp"]
                        .as_f64()
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: the reader states when {function} was analysed, \
                                 got {recommendation}"
                            )
                        });
                    assert_eq!(
                        stated_millis(held),
                        Some(millis_of(::aws_smithy_types::DateTime::from_secs_f64(
                            expected
                        ))),
                        "case {case:?}: column {column} of {function} must state the instant the \
                         reader reports ({expected}), got {held:?} in {row:?}"
                    );
                    continue;
                }
                if let Some(expected) = expected_cell(column, recommendation) {
                    assert_eq!(
                        held, expected,
                        "case {case:?}: column {column} of {function} must hold what the reader \
                         reports ({expected:?}), got {row:?}"
                    );
                    continue;
                }
                if column == "CurrentConfigurationTimeout" {
                    let expected = lambda_timeout(&router, case, recommendation).await;
                    assert_eq!(
                        held, expected,
                        "case {case:?}: column {column} of {function} must state the timeout AWS \
                         Lambda holds that version configured with ({expected}), got {row:?}"
                    );
                    continue;
                }
                let Some(expected) = expected_cost(column, recommendation) else {
                    assert!(
                        held.is_empty(),
                        "case {case:?}: column {column} of {function} is empty for a \
                         recommendation the reader reports no option for, got {row:?}"
                    );
                    continue;
                };
                let actual: f64 = held.parse().unwrap_or_else(|_| {
                    panic!(
                        "case {case:?}: column {column} of {function} must state a cost, got \
                         {held:?} in {row:?}"
                    )
                });
                // The cell is that cost at the two decimal places every figure this service
                // reports is rounded to, so it is compared at that resolution.
                let slack = expected.abs() * 0.01 + 0.02;
                assert!(
                    (actual - expected).abs() <= slack,
                    "case {case:?}: column {column} of {function} must cost what the reader's \
                     own members bill at the published prices ({expected} +/- {slack}), got \
                     {actual}"
                );
            }
        }
    }
}

/// The exported rows are not a sheet of empty cells: every member the function reader reports
/// BEYOND the findings each analysed resource carries — the version analysed, how often it ran,
/// the memory it is configured with and for how long an invocation may run, the utilization
/// observed, the memory size its option would move it to with that option's projections and
/// costs, and the function's tags — is filled for at least one exported row, under the column
/// the field set names for it.
#[tokio::test]
async fn the_exported_rows_carry_the_function_members_the_reader_reports() {
    let case = "the filled columns";
    let router = versioned_emulator().await;
    let reported = reader_recommendations(&router, case, &json!([])).await;
    let rows = exported_recommendation_rows(&router, case, EXPORT, CONTENT_BUCKET, &[]).await;
    let header = rows.first().cloned().unwrap_or_default();
    let data = rows.get(1..).unwrap_or_default();
    assert_eq!(
        data.len(),
        reported.len(),
        "expected one row per reported recommendation ({}), got {rows:?}",
        reported.len()
    );

    // One column per member the Lambda function surface reports beyond the shared findings
    // columns, each the column the model's own field set declares for it.
    let must_be_filled = [
        "FunctionVersion",
        "NumberOfInvocations",
        "CurrentConfigurationMemorySize",
        "CurrentConfigurationTimeout",
        "UtilizationMetricsDurationAverage",
        "UtilizationMetricsDurationMaximum",
        "UtilizationMetricsMemoryAverage",
        "UtilizationMetricsMemoryMaximum",
        "CurrentCostTotal",
        "CurrentCostAverage",
        "RecommendationOptionsConfigurationMemorySize",
        "RecommendationOptionsCostLow",
        "RecommendationOptionsCostHigh",
        "RecommendationOptionsProjectedUtilizationMetricsDurationLowerBound",
        "RecommendationOptionsProjectedUtilizationMetricsDurationExpected",
        "RecommendationOptionsProjectedUtilizationMetricsDurationUpperBound",
        "RecommendationOptionsEstimatedMonthlySavingsValue",
        "Tags",
    ];
    for column in must_be_filled {
        let filled = data
            .iter()
            .filter(|row| !cell(&header, row, column).is_empty())
            .count();
        assert!(
            filled > 0,
            "column {column} must carry what the reader reports for at least one of the analysed \
             functions, got {rows:?}"
        );
    }

    // The two versions of the one function the suite publishes are two rows, each stating the
    // configuration AWS Lambda holds for THAT version: the version is what the row is about.
    let versions: Vec<(&str, &str)> = data
        .iter()
        .filter(|row| cell(&header, row, "FunctionArn").contains(VERSIONED_FUNCTION))
        .map(|row| {
            (
                cell(&header, row, "FunctionVersion"),
                cell(&header, row, "CurrentConfigurationTimeout"),
            )
        })
        .collect();
    assert!(
        versions.len() > 1,
        "the published version and the unpublished one are each a row of {VERSIONED_FUNCTION}, \
         got {versions:?} in {rows:?}"
    );
    assert!(
        versions
            .iter()
            .any(|(version, _)| *version == LATEST_VERSION)
            && versions.iter().any(|(version, _)| *version != LATEST_VERSION),
        "the rows span the unpublished version and a published one, got {versions:?}"
    );
    assert!(
        versions
            .iter()
            .any(|(_, timeout)| *timeout == RECONFIGURED_TIMEOUT_SECS.to_string())
            && versions
                .iter()
                .any(|(_, timeout)| *timeout != RECONFIGURED_TIMEOUT_SECS.to_string()),
        "each row states the timeout of its own version, got {versions:?}"
    );
}

/// "Functions with a finding of unavailable are not returned unless you specify the filter
/// parameter with a value of `Unavailable`": the export file holds exactly the recommendations
/// the reader answers the SAME filter list with — one row per filter list, spanning the two
/// filter names this surface declares, the finding only a function the analysis reached no
/// finding for carries, the reason codes of either finding, a list that selects every function
/// and one that selects none.
#[tokio::test]
async fn the_export_holds_the_recommendations_the_reader_answers_the_same_filters_with() {
    // (case, the `filters` both the reader and the export are given)
    let rows: Vec<(&str, Value)> = vec![
        ("no filter at all", json!([])),
        (
            "the finding of a function the analysis could not size",
            json!([{"name": LAMBDA_FILTER_FINDING, "values": [LAMBDA_FINDING_UNAVAILABLE]}]),
        ),
        (
            "a finding the analysis did reach",
            json!([{"name": LAMBDA_FILTER_FINDING, "values": [FINDING_NOT_OPTIMIZED]}]),
        ),
        (
            "every finding the model declares at once",
            json!([{"name": LAMBDA_FILTER_FINDING,
                    "values": [FINDING_OPTIMIZED, FINDING_NOT_OPTIMIZED,
                               LAMBDA_FINDING_UNAVAILABLE]}]),
        ),
        (
            "the reason codes behind an unavailable finding",
            json!([{"name": LAMBDA_FILTER_FINDING_REASON_CODE,
                    "values": [LAMBDA_REASON_INSUFFICIENT_DATA, LAMBDA_REASON_INCONCLUSIVE]}]),
        ),
        (
            "a reason code of a function the analysis sized",
            json!([{"name": LAMBDA_FILTER_FINDING_REASON_CODE,
                    "values": [METRIC_REASON_MEMORY_OVERPROVISIONED]}]),
        ),
        (
            // Two filters are ANDed, and an optimized function carries no reason code at all,
            // so this list selects nothing — for the export as for the reader.
            "two filters nothing satisfies at once",
            json!([{"name": LAMBDA_FILTER_FINDING, "values": [FINDING_OPTIMIZED]},
                   {"name": LAMBDA_FILTER_FINDING_REASON_CODE,
                    "values": [LAMBDA_REASON_INSUFFICIENT_DATA]}]),
        ),
    ];

    let fields = [
        "FunctionArn",
        "FunctionVersion",
        "Finding",
        "FindingReasonCodes",
        "CurrentConfigurationMemorySize",
        "RecommendationOptionsConfigurationMemorySize",
    ];

    let mut saw_unavailable = false;
    let mut saw_nothing = false;
    for (case, filters) in rows {
        // A fresh emulator per row, for the one-job-per-Region limit.
        let (router, _engine) = analysed_emulator().await;
        let reported = reader_recommendations(&router, case, &filters).await;
        let exported = exported_recommendation_rows_selecting(
            &router,
            case,
            EXPORT,
            CONTENT_BUCKET,
            &fields,
            &filters,
        )
        .await;
        let header = exported.first().cloned().unwrap_or_default();
        let data = exported.get(1..).unwrap_or_default();

        let selected: Vec<String> = reported
            .iter()
            .map(|recommendation| text(recommendation, "functionArn"))
            .collect();
        let held: Vec<String> = data
            .iter()
            .map(|row| cell(&header, row, "FunctionArn").to_string())
            .collect();
        assert_eq!(
            held, selected,
            "case {case:?}: the file holds the recommendations the reader answers {filters} with, \
             got {exported:?}"
        );

        for (row, recommendation) in data.iter().zip(&reported) {
            let function = text(recommendation, "functionArn");
            for (column, expected) in [
                ("FunctionVersion", text(recommendation, "functionVersion")),
                ("Finding", text(recommendation, "finding")),
                (
                    "FindingReasonCodes",
                    list_cell(&recommendation["findingReasonCodes"]),
                ),
                (
                    "CurrentConfigurationMemorySize",
                    whole_cell(recommendation, "currentMemorySize"),
                ),
            ] {
                assert_eq!(
                    cell(&header, row, column),
                    expected,
                    "case {case:?}: column {column} of {function} must hold what the reader \
                     reports ({expected:?}), got {row:?}"
                );
            }
            // The option's column is filled exactly where the reader reports an option: a
            // function "Compute Optimizer was unable to generate a recommendation" for has no
            // memory size to move to.
            let option = &recommendation["memorySizeRecommendationOptions"][0];
            assert_eq!(
                !cell(&header, row, "RecommendationOptionsConfigurationMemorySize").is_empty(),
                option.is_object(),
                "case {case:?}: the option column of {function} is filled exactly when the \
                 reader reports an option ({option}), got {row:?}"
            );
            saw_unavailable |= text(recommendation, "finding") == LAMBDA_FINDING_UNAVAILABLE;
        }
        saw_nothing |= data.is_empty();
    }
    assert!(
        saw_unavailable,
        "the table must span a filter list the reader answers with a function the analysis \
         reached no finding for, or the rule it is about is never exercised"
    );
    assert!(
        saw_nothing,
        "the table must span a filter list that selects no recommendation at all"
    );
}
