// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the data automation library INGESTION JOB:
//! `InvokeDataAutomationLibraryIngestionJob` mints one,
//! `GetDataAutomationLibraryIngestionJob` reports it and
//! `ListDataAutomationLibraryIngestionJobs` enumerates the jobs of one library.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch` with
//! a signed-looking request — so routing, HTTP binding, constraint validation, the handler
//! and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! RULE. An ingestion job is a resource OF the one library its `libraryArn` names: a
//! submission mints it under that library and answers its `jobArn`, a read of the
//! (library, job) pair reports it, and the library's listing enumerates exactly the jobs
//! minted under it — in this service's listing order (`creationTime`, then ARN), one page
//! at a time, with `maxResults` capping the page and a `nextToken` handed back only while
//! something is left after it. Each item carries what the modelled
//! `DataAutomationLibraryIngestionJobSummary` declares
//! (`smithy_query(query="shape",shape="DataAutomationLibraryIngestionJobSummary")`:
//! `jobArn`, `jobStatus`, `entityType`, `operationType`, `creationTime`,
//! `completionTime`), read back out of the record the submission persisted. A library this
//! store holds nothing under resolves to nothing, so every one of the three refuses it.
//!
//! The suites are TABLE-driven: the rows differ along the number of jobs stored, the page
//! size asked for, which library is addressed, the entity and operation types submitted
//! and the input source named — never over one recorded request.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at, timestamp_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A `libraryArn` that satisfies the modelled pattern and names no library of this store:
/// the resource id is 12..36 characters of `[a-zA-Z0-9-]`, so it reaches the handler and
/// is refused there rather than by the constraint walk.
const UNKNOWN_LIBRARY_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:data-automation-library/000000000000";

/// A `jobArn` that satisfies the modelled pattern and names no ingestion job of this
/// store, for the same reason as [`UNKNOWN_LIBRARY_ARN`].
const UNKNOWN_JOB_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:data-automation-library-ingestion-job/000000000000";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{REGION}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

fn emulator() -> MultiServiceRouter {
    emulator_with_store().0
}

/// The emulator under test, with a handle on the shared engine so a case can assert what
/// the store HOLDS and not only what it answers.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request and return its status and decoded JSON body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateDataAutomationLibrary`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-libraries/, code: 201}`. Returns the ARN.
async fn create_library(router: &MultiServiceRouter, name: &str) -> String {
    let (status, payload) = call(
        router,
        "PUT",
        "/data-automation-libraries/",
        &json!({ "libraryName": name }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected CreateDataAutomationLibrary 201, got {status} with {payload:?}"
    );
    string_at(&payload, "libraryArn", name)
}

/// The job collection's path for one library, as the modelled `@http` URIs bind it
/// (`/data-automation-libraries/{libraryArn}/library-ingestion-jobs/`).
fn jobs_uri(library_arn: &str) -> String {
    format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/",
        encode_label(library_arn)
    )
}

/// One ingestion job's path, as `GetDataAutomationLibraryIngestionJob` binds it.
fn job_uri(library_arn: &str, job_arn: &str) -> String {
    format!(
        "/data-automation-libraries/{}/library-ingestion-jobs/{}",
        encode_label(library_arn),
        encode_label(job_arn)
    )
}

/// An `inputConfiguration` naming an S3 object, the first of the two sources the modelled
/// `InputConfiguration` carries.
fn s3_input(key: &str) -> Value {
    json!({ "s3Object": { "s3Uri": format!("s3://bda-ingestion/{key}.json") } })
}

/// An `inputConfiguration` carrying the modelled `InlinePayload` union's upsert arm, with
/// one vocabulary entity (`smithy_query(query="shape",shape="UpsertEntityInfo")` carries
/// the single member `vocabulary`).
fn inline_upsert_input(entity_id: &str) -> Value {
    json!({
        "inlinePayload": {
            "upsertEntitiesInfo": [{
                "vocabulary": {
                    "entityId": entity_id,
                    "language": "EN",
                    "phrases": [{ "text": "mockingbird" }],
                },
            }],
        },
    })
}

/// An `inputConfiguration` carrying the union's delete arm.
fn inline_delete_input(entity_id: &str) -> Value {
    json!({
        "inlinePayload": { "deleteEntitiesInfo": { "entityIds": [entity_id] } },
    })
}

/// The `outputConfiguration` every submission carries (modelled required, one `s3Uri`).
fn output_configuration(key: &str) -> Value {
    json!({ "s3Uri": format!("s3://bda-ingestion/output/{key}") })
}

/// A submission body: the four required members, with the input source the case names.
fn submission(entity_type: &str, operation_type: &str, input: Value, key: &str) -> Value {
    json!({
        "entityType": entity_type,
        "operationType": operation_type,
        "inputConfiguration": input,
        "outputConfiguration": output_configuration(key),
    })
}

/// `InvokeDataAutomationLibraryIngestionJob`, per its modelled `@http`
/// `{method: PUT, uri: /data-automation-libraries/{libraryArn}/library-ingestion-jobs/,
/// code: 201}`.
async fn invoke_job(
    router: &MultiServiceRouter,
    library_arn: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "PUT", &jobs_uri(library_arn), body).await
}

/// A submission that must be accepted; returns the `jobArn` it answered.
async fn submit_job(
    router: &MultiServiceRouter,
    library_arn: &str,
    case: &str,
    body: &Value,
) -> String {
    let (status, payload) = invoke_job(router, library_arn, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected InvokeDataAutomationLibraryIngestionJob 201, got {status} \
         with {payload:?}"
    );
    string_at(&payload, "jobArn", case)
}

/// `ListDataAutomationLibraryIngestionJobs`, per its modelled `@http`
/// `{method: POST, uri: /data-automation-libraries/{libraryArn}/library-ingestion-jobs/}`.
async fn list_jobs(
    router: &MultiServiceRouter,
    library_arn: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "POST", &jobs_uri(library_arn), body).await
}

/// `GetDataAutomationLibraryIngestionJob`, per its modelled `@http`
/// `{method: POST, uri: …/library-ingestion-jobs/{jobArn}}`.
async fn get_job(router: &MultiServiceRouter, library_arn: &str, job_arn: &str) -> (u16, Value) {
    call(router, "POST", &job_uri(library_arn, job_arn), &json!({})).await
}

/// One row of the tagging table: the case, the tags the submission carries, and the
/// key/value pairs the minted job then holds.
type TaggingRow<'a> = (&'a str, Value, Vec<(&'a str, &'a str)>);

/// One row of the refusal table: the case, the status, error code and message AWS answers
/// for it, and the answer the call under test gave.
type RefusalRow<'a> = (&'a str, u16, &'a str, String, (u16, Value));

/// The `jobArn`s one page carries, in the order the page served them.
fn arns_of(page: &Value, case: &str) -> Vec<String> {
    page.get("jobs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no jobs in {page:?}"))
        .iter()
        .map(|item| string_at(item, "jobArn", case))
        .collect()
}

/// RULE: the listing enumerates exactly the jobs minted under the library addressed — so a
/// library nothing has been submitted against answers an empty page with no token, every
/// submission adds exactly one item, and a second library's jobs never appear in the
/// first's listing.
///
/// The rows differ along the COUNT submitted, which is what the enumeration generalises
/// over, and along WHICH library is addressed.
#[tokio::test]
async fn the_listing_reports_every_job_minted_under_that_library() {
    let router = emulator();
    let library = create_library(&router, "ingestion-listing-library").await;
    let other = create_library(&router, "ingestion-listing-other").await;

    let (status, empty) = list_jobs(&router, &library, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"a library with no job\": expected 200, got {status} with {empty:?}"
    );
    assert!(
        arns_of(&empty, "a library with no job").is_empty(),
        "case \"a library with no job\": expected no jobs, got {empty:?}"
    );
    assert!(
        empty.get("nextToken").is_none(),
        "case \"a library with no job\": a last page carries no token, got {empty:?}"
    );

    let mut submitted: Vec<String> = Vec::new();
    for index in 0..3 {
        let case = format!("{} submitted", index + 1);
        let body = submission(
            "VOCABULARY",
            "UPSERT",
            s3_input(&format!("listing-{index}")),
            &format!("listing-{index}"),
        );
        submitted.push(submit_job(&router, &library, &case, &body).await);

        let (status, page) = list_jobs(&router, &library, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {page:?}"
        );
        let listed = arns_of(&page, &case);
        assert_eq!(
            listed, submitted,
            "case {case:?}: expected the submitted jobs in creation order, got {listed:?}"
        );

        // The other library's listing is unaffected: a job belongs to one library.
        let (status, untouched) = list_jobs(&router, &other, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the other library, got {status} with \
             {untouched:?}"
        );
        assert!(
            arns_of(&untouched, &case).is_empty(),
            "case {case:?}: the other library must still hold no job, got {untouched:?}"
        );
    }

    // A job submitted against the OTHER library appears only in its own listing.
    let other_job = submit_job(
        &router,
        &other,
        "the other library",
        &submission("VOCABULARY", "UPSERT", s3_input("other"), "other"),
    )
    .await;
    let (_, other_page) = list_jobs(&router, &other, &json!({})).await;
    assert_eq!(
        arns_of(&other_page, "the other library"),
        vec![other_job.clone()],
        "case \"the other library\": expected only its own job, got {other_page:?}"
    );
    let (_, first_page) = list_jobs(&router, &library, &json!({})).await;
    assert_eq!(
        arns_of(&first_page, "the first library"),
        submitted,
        "case \"the first library\": a job of another library must not be listed here, got \
         {first_page:?}"
    );
}

/// RULE: `maxResults` caps the page and the `nextToken` resumes exactly after the last
/// item served, so walking the pages visits every job of the library once, in order,
/// whatever page size is asked for.
#[tokio::test]
async fn paging_walks_every_job_once_whatever_the_page_size() {
    let router = emulator();
    let library = create_library(&router, "ingestion-paging-library").await;
    let mut all: Vec<String> = Vec::new();
    for index in 0..5 {
        let case = format!("fixture {index}");
        all.push(
            submit_job(
                &router,
                &library,
                &case,
                &submission(
                    "VOCABULARY",
                    "UPSERT",
                    s3_input(&format!("paged-{index}")),
                    &format!("paged-{index}"),
                ),
            )
            .await,
        );
    }

    // (case, the page size asked for)
    let rows: [(&str, Option<i32>); 5] = [
        ("no page size, the server's own limit", None),
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("exactly the stored count", Some(5)),
        ("more than the stored count", Some(50)),
    ];
    for (case, max_results) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({});
            if let Some(max) = max_results {
                body["maxResults"] = json!(max);
            }
            if let Some(next) = &token {
                body["nextToken"] = json!(next);
            }
            let (status, page) = list_jobs(&router, &library, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {page:?}"
            );
            let listed = arns_of(&page, case);
            if let Some(max) = max_results {
                assert!(
                    listed.len() <= max as usize,
                    "case {case:?}: a page served {} items, over the {max} asked for",
                    listed.len()
                );
            }
            walked.extend(listed);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= all.len(),
                "case {case:?}: paging served {} items for {} stored",
                walked.len(),
                all.len()
            );
        }
        assert_eq!(
            walked, all,
            "case {case:?}: expected every submitted job once, in order, got {walked:?}"
        );
    }
}

/// RULE: the submission persists what it was handed, so a read of the (library, job) pair
/// reports exactly it and the listing's summary agrees with that read — whatever entity
/// type, operation type and input source the submission named.
///
/// The rows span the modelled enum sets of both members
/// (`smithy_query(shape="EntityType")` -> `["VOCABULARY"]`,
/// `smithy_query(shape="LibraryIngestionJobOperationType")` -> `["UPSERT", "DELETE"]`) and
/// both arms of the input union.
#[tokio::test]
async fn a_submitted_job_is_reported_as_it_was_submitted() {
    let router = emulator();
    let library = create_library(&router, "ingestion-roundtrip-library").await;

    // (case, entityType, operationType, the inputConfiguration)
    let rows: [(&str, &str, &str, Value); 4] = [
        (
            "a vocabulary upsert from an inline payload",
            "VOCABULARY",
            "UPSERT",
            inline_upsert_input("vocab-entity-1"),
        ),
        (
            "a vocabulary delete from an inline payload",
            "VOCABULARY",
            "DELETE",
            inline_delete_input("vocab-entity-1"),
        ),
        (
            "a vocabulary upsert from an S3 object",
            "VOCABULARY",
            "UPSERT",
            s3_input("phrases"),
        ),
        (
            "a vocabulary delete from an S3 object",
            "VOCABULARY",
            "DELETE",
            s3_input("phrases-removed"),
        ),
    ];
    for (case, entity_type, operation_type, input) in rows {
        let output = output_configuration(case.split(' ').next_back().unwrap_or("out"));
        let body = json!({
            "entityType": entity_type,
            "operationType": operation_type,
            "inputConfiguration": input,
            "outputConfiguration": output,
        });
        let job_arn = submit_job(&router, &library, case, &body).await;
        assert!(
            job_arn.contains(":data-automation-library-ingestion-job/"),
            "case {case:?}: the minted ARN must name this resource type, got {job_arn:?}"
        );

        let (status, read) = get_job(&router, &library, &job_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDataAutomationLibraryIngestionJob 200, got {status} \
             with {read:?}"
        );
        let job = read
            .get("job")
            .unwrap_or_else(|| panic!("case {case:?}: no job in {read:?}"));
        assert_eq!(
            string_at(job, "jobArn", case),
            job_arn,
            "case {case:?}: the read must answer under the ARN the submission minted"
        );
        assert_eq!(
            string_at(job, "entityType", case),
            entity_type,
            "case {case:?}: the read must report the submitted entityType, got {job:?}"
        );
        assert_eq!(
            string_at(job, "operationType", case),
            operation_type,
            "case {case:?}: the read must report the submitted operationType, got {job:?}"
        );
        assert_eq!(
            job.get("outputConfiguration"),
            Some(&output),
            "case {case:?}: the read must report the submitted outputConfiguration, got \
             {job:?}"
        );
        assert_eq!(
            string_at(job, "jobStatus", case),
            state::library_ingestion::INGESTION_JOB_IN_PROGRESS,
            "case {case:?}: a job read straight after its submission is still in flight, \
             got {job:?}"
        );

        // The listing's summary of this job agrees with the read, member for member.
        let (_, page) = list_jobs(&router, &library, &json!({})).await;
        let items = page
            .get("jobs")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no jobs in {page:?}"));
        let summary = items
            .iter()
            .find(|item| string_at(item, "jobArn", case) == job_arn)
            .unwrap_or_else(|| panic!("case {case:?}: {job_arn} is absent from {page:?}"));
        for member in ["jobStatus", "entityType", "operationType"] {
            assert_eq!(
                string_at(summary, member, case),
                string_at(job, member, case),
                "case {case:?}: the summary's {member} must match the read's, got {summary:?}"
            );
        }
        assert_eq!(
            timestamp_at(summary, "creationTime", case),
            timestamp_at(job, "creationTime", case),
            "case {case:?}: the summary's creationTime must match the read's, got {summary:?}"
        );
        assert!(
            summary.get("completionTime").is_none(),
            "case {case:?}: a job still in flight has no completionTime, got {summary:?}"
        );
    }
}

/// RULE: a job settles into its terminal status when the clock makes that transition due,
/// from which instant every read reports the same completion instant — and reporting it
/// writes nothing, so two reads at one stage agree and the listing agrees with the read.
#[tokio::test]
async fn a_job_completes_when_the_clock_makes_it_due() {
    let router = emulator();
    let library = create_library(&router, "ingestion-lifecycle-library").await;
    let job_arn = submit_job(
        &router,
        &library,
        "settling",
        &submission(
            "VOCABULARY",
            "UPSERT",
            inline_upsert_input("settling-entity"),
            "settling",
        ),
    )
    .await;

    // (case, the status due at that stage, whether a completion instant is reported). A
    // test may wait for a due instant; a handler may not. `tokio`'s timer is not in this
    // workspace's feature set, so the test thread parks instead.
    let legs = [
        (
            "before the transition is due",
            state::library_ingestion::INGESTION_JOB_IN_PROGRESS,
            false,
        ),
        (
            "after the transition is due",
            state::library_ingestion::INGESTION_JOB_COMPLETED,
            true,
        ),
    ];
    for (leg, (case, expected, completed)) in legs.into_iter().enumerate() {
        if leg > 0 {
            std::thread::sleep(
                state::library_ingestion::INGESTION_JOB_SPAN + Duration::from_millis(250),
            );
        }
        let mut reported: Vec<(i64, u32)> = Vec::new();
        for attempt in 1..=2 {
            let (status, read) = get_job(&router, &library, &job_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} read #{attempt}: expected 200, got {status} with {read:?}"
            );
            let job = read
                .get("job")
                .unwrap_or_else(|| panic!("case {case:?}: no job in {read:?}"));
            assert_eq!(
                string_at(job, "jobStatus", case),
                expected,
                "case {case:?} read #{attempt}: expected jobStatus {expected}, got {job:?}"
            );
            if completed {
                let completion = timestamp_at(job, "completionTime", case);
                assert!(
                    completion >= timestamp_at(job, "creationTime", case),
                    "case {case:?} read #{attempt}: the completion instant must not precede \
                     the creation instant, got {job:?}"
                );
                reported.push(completion);
            } else {
                assert!(
                    job.get("completionTime").is_none(),
                    "case {case:?} read #{attempt}: a job in flight reports no \
                     completionTime, got {job:?}"
                );
            }

            // The listing reports the same stage as the read.
            let (_, page) = list_jobs(&router, &library, &json!({})).await;
            let items = page
                .get("jobs")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no jobs in {page:?}"));
            assert_eq!(items.len(), 1, "case {case:?}: expected one job, got {page:?}");
            assert_eq!(
                string_at(&items[0], "jobStatus", case),
                expected,
                "case {case:?} read #{attempt}: the listing must report the same status as \
                 the read, got {page:?}"
            );
        }
        if completed {
            assert_eq!(
                reported.first(),
                reported.last(),
                "case {case:?}: two reads must report one completion instant, got {reported:?}"
            );
        }
    }
}

/// RULE: an `@idempotencyToken` replay answers about the job the first call accepted
/// rather than accepting a second one, so the library's listing still holds exactly one
/// job — while a submission under a fresh token mints its own.
#[tokio::test]
async fn an_idempotent_replay_accepts_no_second_job() {
    let router = emulator();
    let library = create_library(&router, "ingestion-idempotency-library").await;
    // `ClientToken` is modelled @length(33..256), pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$.
    let token = "ingestionjobclienttoken0000000001";
    let mut body = submission("VOCABULARY", "UPSERT", s3_input("idempotent"), "idempotent");
    body["clientToken"] = json!(token);

    let first = submit_job(&router, &library, "the first call", &body).await;
    let replay = submit_job(&router, &library, "the replay", &body).await;
    assert_eq!(
        replay, first,
        "case \"the replay\": expected the ARN the first call minted, got {replay:?}"
    );
    let (_, page) = list_jobs(&router, &library, &json!({})).await;
    assert_eq!(
        arns_of(&page, "the replay"),
        vec![first.clone()],
        "case \"the replay\": the library must still hold exactly one job, got {page:?}"
    );

    let mut fresh_body = body.clone();
    fresh_body["clientToken"] = json!("ingestionjobclienttoken0000000002");
    let second = submit_job(&router, &library, "a fresh token", &fresh_body).await;
    assert_ne!(
        second, first,
        "case \"a fresh token\": a submission under a fresh token mints its own job"
    );
    let (_, page) = list_jobs(&router, &library, &json!({})).await;
    assert_eq!(
        arns_of(&page, "a fresh token"),
        vec![first.clone(), second],
        "case \"a fresh token\": expected both jobs, got {page:?}"
    );

    // A token buys no success for a request this service would otherwise refuse: the
    // references the request names are resolved first, and a job of another library is no
    // replay of this submission.
    let other = create_library(&router, "ingestion-idempotency-other").await;
    let (status, refused) = invoke_job(&router, UNKNOWN_LIBRARY_ARN, &body).await;
    assert_eq!(
        status, 404,
        "case \"a replay against a library this store does not hold\": expected 404, got \
         {status} with {refused:?}"
    );
    let mut no_source = body.clone();
    no_source["inputConfiguration"] = json!({});
    let (status, refused) = invoke_job(&router, &library, &no_source).await;
    assert_eq!(
        status, 400,
        "case \"a replay whose inputConfiguration names no source\": expected 400, got \
         {status} with {refused:?}"
    );
    let under_other = submit_job(&router, &other, "the same token, another library", &body).await;
    assert_ne!(
        under_other, first,
        "case \"the same token, another library\": a job of another library is no replay"
    );
    let (_, page) = list_jobs(&router, &other, &json!({})).await;
    assert_eq!(
        arns_of(&page, "the same token, another library"),
        vec![under_other],
        "case \"the same token, another library\": expected its own single job, got {page:?}"
    );
    let (_, page) = list_jobs(&router, &library, &json!({})).await;
    assert_eq!(
        arns_of(&page, "after the refusals").len(),
        2,
        "case \"after the refusals\": the refused replays must leave no job behind, got \
         {page:?}"
    );
}

/// RULE: the tags a submission carries are held on the JOB the submission minted, under
/// the `jobArn` it answered, which is how the service's tagging surface addresses it — the
/// modelled `TaggableResourceArn` admits `data-automation-library-ingestion-job` beside
/// this service's other resource types
/// (`smithy_query(query="operation",operation="TagResource")`).
///
/// Asserted through the store's own tag lookup rather than over the wire, because the
/// three tagging paths are bound identically by the Bedrock control plane and the family
/// claim rule gives them to that plane (`src/service.rs`'s `can_handle`), so a router
/// holding only this service serves none of them.
///
/// The rows differ along the tag count a submission carries.
#[tokio::test]
async fn the_tags_a_submission_carries_are_held_on_the_job() {
    let (router, engine) = emulator_with_store();
    let library = create_library(&router, "ingestion-tagging-library").await;

    // (case, the tags the submission carries, the pairs the job then holds)
    let rows: Vec<TaggingRow<'_>> = vec![
        ("no tags at all", json!([]), Vec::new()),
        (
            "one tag",
            json!([{ "key": "team", "value": "data-eng" }]),
            vec![("team", "data-eng")],
        ),
        (
            "two tags",
            json!([
                { "key": "team", "value": "data-eng" },
                { "key": "env", "value": "test" },
            ]),
            vec![("team", "data-eng"), ("env", "test")],
        ),
    ];
    for (case, tags, expected) in rows {
        let mut body = submission(
            "VOCABULARY",
            "UPSERT",
            s3_input("tagged"),
            case.split(' ').next_back().unwrap_or("tagged"),
        );
        body["tags"] = tags;
        let job_arn = submit_job(&router, &library, case, &body).await;

        let want: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let store = engine.state::<ServiceState>();
        let held = store.lock().resource_tags(&job_arn).map(|tags| {
            tags.iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect::<Vec<(String, String)>>()
        });
        assert_eq!(
            held,
            Some(want),
            "case {case:?}: the tagging surface must resolve the minted job ARN and report \
             the submitted tags, got {held:?}"
        );

        // The same lookup the tagging CALLS write through resolves the job too, so a tag
        // set on it is held on the one list a read reports.
        let mut guard = store.lock();
        let writable = guard
            .resource_tags_mut(&job_arn)
            .map(|tags| tags.len())
            .unwrap_or_else(|| panic!("case {case:?}: {job_arn} resolves to no tag list"));
        assert_eq!(
            writable,
            expected.len(),
            "case {case:?}: expected {} tag(s) on the writable list, got {writable}",
            expected.len()
        );
    }
}

/// RULE: every one of the three operations resolves the resources the request names, and
/// refuses what resolves to nothing — the library with this service's fixed sentence for
/// an unresolved library, a job of another library (or of none) with the sentence for an
/// unresolved ingestion job; a `nextToken` this service did not issue and an
/// `inputConfiguration` naming no source are refused as validation failures.
///
/// The rows differ along WHICH reference fails to resolve and which operation is handed
/// it, so the rule is asserted across the whole surface rather than on one call.
#[tokio::test]
async fn what_resolves_to_nothing_is_refused() {
    let router = emulator();
    let library = create_library(&router, "ingestion-refusal-library").await;
    let other = create_library(&router, "ingestion-refusal-other").await;
    let job_of_other = submit_job(
        &router,
        &other,
        "fixture",
        &submission("VOCABULARY", "UPSERT", s3_input("refusal"), "refusal"),
    )
    .await;
    let body = submission("VOCABULARY", "UPSERT", s3_input("refused"), "refused");

    // (case, status, error code, message, the call)
    let library_missing = state::LIBRARY_ARN_NOT_FOUND_MESSAGE;
    let job_missing = state::library_ingestion::INGESTION_JOB_ARN_NOT_FOUND_MESSAGE;

    let rows: Vec<RefusalRow<'_>> = vec![
        (
            "a listing of a library this store does not hold",
            404,
            "ResourceNotFoundException",
            library_missing.to_string(),
            list_jobs(&router, UNKNOWN_LIBRARY_ARN, &json!({})).await,
        ),
        (
            "a read under a library this store does not hold",
            404,
            "ResourceNotFoundException",
            library_missing.to_string(),
            get_job(&router, UNKNOWN_LIBRARY_ARN, &job_of_other).await,
        ),
        (
            "a submission against a library this store does not hold",
            404,
            "ResourceNotFoundException",
            library_missing.to_string(),
            invoke_job(&router, UNKNOWN_LIBRARY_ARN, &body).await,
        ),
        (
            "a read of a job this store does not hold",
            404,
            "ResourceNotFoundException",
            job_missing.to_string(),
            get_job(&router, &library, UNKNOWN_JOB_ARN).await,
        ),
        (
            "a read of a job of another library",
            404,
            "ResourceNotFoundException",
            job_missing.to_string(),
            get_job(&router, &library, &job_of_other).await,
        ),
        (
            "a nextToken this service did not issue",
            400,
            "ValidationException",
            "nextToken is not a token this service issued".to_string(),
            list_jobs(&router, &library, &json!({ "nextToken": "not-a-token" })).await,
        ),
        (
            "a submission whose inputConfiguration names no source",
            400,
            "ValidationException",
            state::library_ingestion::MISSING_INGESTION_INPUT_MESSAGE.to_string(),
            invoke_job(
                &router,
                &library,
                &json!({
                    "entityType": "VOCABULARY",
                    "operationType": "UPSERT",
                    "inputConfiguration": {},
                    "outputConfiguration": output_configuration("no-source"),
                }),
            )
            .await,
        ),
    ];
    for (case, expected_status, expected_code, expected_message, (status, payload)) in rows {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {payload:?}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} with {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }

    // A refused submission left nothing behind: the library it named still lists only the
    // jobs that were accepted.
    let (_, page) = list_jobs(&router, &library, &json!({})).await;
    assert!(
        arns_of(&page, "after the refusals").is_empty(),
        "case \"after the refusals\": a refused submission must leave no job behind, got \
         {page:?}"
    );
}
