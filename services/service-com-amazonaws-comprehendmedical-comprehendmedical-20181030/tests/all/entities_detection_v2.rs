// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the asynchronous entities-detection job lifecycle of Amazon
//! Comprehend Medical (start, describe, list, stop), plus the refusals this service
//! words itself and the 501 the synchronous `DetectEntitiesV2` inference still answers.
//!
//! `InferICD10CM` is implemented and has its own suite (`tests/all/icd10_cm_inference.rs`).
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise
//! routing, validation, the handler and serialization together.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Wait out a transition the clock owes a job.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature (see the root manifest), and each test awaits nothing
/// else while it waits, so blocking this test's own thread costs nothing.
fn wait(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// A router carrying only this service, over its own engine, so each test observes
/// exactly the jobs it created.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call `operation` with `body` and return the HTTP status and the parsed JSON answer.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Wall clock in epoch seconds, for the submit-time filter bounds.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A start call with everything the model requires, plus whatever `extra` adds.
fn start_body(extra: Vec<(&str, Value)>) -> Value {
    let mut body = json!({
        "InputDataConfig": {"S3Bucket": "in-bucket", "S3Key": "docs/"},
        "OutputDataConfig": {"S3Bucket": "out-bucket"},
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/CMRole",
        "LanguageCode": "en",
    });
    for (member, value) in extra {
        body[member] = value;
    }
    body
}

/// Start a job and return its `JobId`.
async fn start_job(router: &MultiServiceRouter, extra: Vec<(&str, Value)>) -> String {
    let (status, answer) = call(router, "StartEntitiesDetectionV2Job", &start_body(extra)).await;
    assert_eq!(status, 200, "start a job: expected 200, got {status} {answer}");
    answer["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("start a job: expected a JobId, got {answer}"))
        .to_string()
}

/// Describe `job_id` and return its `ComprehendMedicalAsyncJobProperties`.
async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Value {
    let (status, answer) = call(
        router,
        "DescribeEntitiesDetectionV2Job",
        &json!({"JobId": job_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "describe {job_id}: expected 200, got {status} {answer}"
    );
    answer["ComprehendMedicalAsyncJobProperties"].clone()
}

/// The synchronous detection calls that are machine-learning inference over clinical
/// text with no ontology to link against are performed by nothing in this emulator, so
/// each one answers the scaffold's 501 `NotImplemented` — never an empty or synthesised
/// 200 a client could not tell from a real detection.
///
/// The rows span the text-inference operations that are still scaffolds and both ends of
/// the accepted text length, so an operation that starts answering 200 without a real
/// inference component behind it fails here. `InferICD10CM`, `InferRxNorm` and
/// `InferSNOMEDCT` link against an ontology and `DetectPHI` places the written forms of
/// the identifier classes it reports; all four are implemented and asserted in their own
/// suites.
#[tokio::test]
async fn the_text_inference_operations_answer_not_implemented() {
    let router = router();
    let cases: Vec<(&str, Value)> = vec![
        (
            "DetectEntitiesV2",
            json!({"Text": "Patient takes 50 mg of Ibuprofen twice daily for headache."}),
        ),
        ("DetectEntitiesV2", json!({"Text": "a"})),
        ("DetectEntities", json!({"Text": "Reports fever."})),
    ];
    for (operation, body) in cases {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: an operation nothing computes must answer 501, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("NotImplemented"),
            "case {operation:?}: expected NotImplemented, got {answer}"
        );
        assert!(
            answer["Entities"].is_null(),
            "case {operation:?}: a stub must not answer with an entity list, got {answer}"
        );
    }
}

/// A value outside a modelled constraint is refused with this service's own client
/// error — before any handler runs, so a stubbed operation still refuses a bad request
/// with a 400 rather than its 501. Text past the length limit is a
/// `TextSizeLimitExceededException`, every other bad value an `InvalidRequestException`.
#[tokio::test]
async fn a_bad_value_is_refused_with_the_services_own_error() {
    let oversize = "a".repeat(20_001);
    let long_job_id = "j".repeat(33);
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "empty text",
            "DetectEntitiesV2",
            json!({"Text": ""}),
            "InvalidRequestException",
        ),
        (
            "text past the 20000 character limit",
            "DetectEntitiesV2",
            json!({"Text": oversize.clone()}),
            "TextSizeLimitExceededException",
        ),
        (
            "text past the limit of another text operation",
            "DetectPHI",
            json!({"Text": oversize}),
            "TextSizeLimitExceededException",
        ),
        (
            "absent required text",
            "DetectEntitiesV2",
            json!({}),
            "InvalidRequestException",
        ),
        (
            "job id past the 32 character limit",
            "DescribeEntitiesDetectionV2Job",
            json!({"JobId": long_job_id}),
            "InvalidRequestException",
        ),
        (
            "max results below the range minimum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 0}),
            "InvalidRequestException",
        ),
        (
            "max results above the range maximum",
            "ListEntitiesDetectionV2Jobs",
            json!({"MaxResults": 501}),
            "InvalidRequestException",
        ),
        (
            "a data access role arn outside the modelled pattern",
            "StartEntitiesDetectionV2Job",
            start_body(vec![("DataAccessRoleArn", json!("not-an-arn"))]),
            "InvalidRequestException",
        ),
        (
            "a language code the enum does not carry",
            "StartEntitiesDetectionV2Job",
            start_body(vec![("LanguageCode", json!("fr"))]),
            "InvalidRequestException",
        ),
        (
            "an absent required input configuration",
            "StartEntitiesDetectionV2Job",
            json!({
                "OutputDataConfig": {"S3Bucket": "out-bucket"},
                "DataAccessRoleArn": "arn:aws:iam::123456789012:role/CMRole",
                "LanguageCode": "en",
            }),
            "InvalidRequestException",
        ),
    ];

    let router = router();
    for (name, operation, body, expected) in cases {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some(expected),
            "case {name:?}: expected {expected}, got {answer}"
        );
    }
}

/// A started job is recorded: the describe reports exactly what the start call was
/// given, and repeating that same submission — one `ClientRequestToken` with the same
/// parameters — never starts a second job.
#[tokio::test]
async fn a_started_job_is_read_back_and_its_request_token_is_idempotent() {
    let router = router();
    let job_id = start_job(
        &router,
        vec![
            ("JobName", json!("demo-job")),
            ("ClientRequestToken", json!("tok-1")),
            ("KMSKey", json!("alias/cm")),
        ],
    )
    .await;
    assert_eq!(
        job_id.len(),
        32,
        "a minted JobId must fit the modelled 1..32 length, got {job_id:?}"
    );

    let repeated = start_job(
        &router,
        vec![
            ("JobName", json!("demo-job")),
            ("ClientRequestToken", json!("tok-1")),
            ("KMSKey", json!("alias/cm")),
        ],
    )
    .await;
    assert_eq!(
        repeated, job_id,
        "case {:?}: repeating one submission must name the first job, got {repeated:?}",
        "tok-1"
    );

    let properties = describe_job(&router, &job_id).await;
    for (member, expected) in [
        ("JobId", json!(job_id.clone())),
        ("JobName", json!("demo-job")),
        ("LanguageCode", json!("en")),
        (
            "DataAccessRoleArn",
            json!("arn:aws:iam::123456789012:role/CMRole"),
        ),
        ("KMSKey", json!("alias/cm")),
        (
            "InputDataConfig",
            json!({"S3Bucket": "in-bucket", "S3Key": "docs/"}),
        ),
        ("OutputDataConfig", json!({"S3Bucket": "out-bucket"})),
    ] {
        assert_eq!(
            properties[member], expected,
            "case {member:?}: describe must report what start was given, got {properties}"
        );
    }
    assert!(
        properties["SubmitTime"].as_f64().is_some_and(|at| at > 0.0),
        "describe must report a SubmitTime, got {properties}"
    );
    assert!(
        properties["ExpirationTime"].as_f64() > properties["SubmitTime"].as_f64(),
        "job metadata must expire after it was submitted, got {properties}"
    );
    assert_eq!(
        properties["ModelVersion"].as_str().map(str::is_empty),
        Some(false),
        "describe must report the model version, got {properties}"
    );

    // Only this one job exists, which is what idempotency means here.
    let (status, answer) = call(&router, "ListEntitiesDetectionV2Jobs", &json!({})).await;
    assert_eq!(status, 200, "list jobs: expected 200, got {status} {answer}");
    let listed = answer["ComprehendMedicalAsyncJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "two starts with one request token must leave one job, got {answer}"
    );
}

/// Two starts without a request token are two jobs, each with its own id.
#[tokio::test]
async fn starts_without_a_request_token_are_distinct_jobs() {
    let router = router();
    let first = start_job(&router, vec![("JobName", json!("same-name"))]).await;
    let second = start_job(&router, vec![("JobName", json!("same-name"))]).await;
    assert_ne!(
        first, second,
        "case {:?}: two starts with no token must mint two ids, got {first:?} twice",
        "same-name"
    );
    for job_id in [&first, &second] {
        let properties = describe_job(&router, job_id).await;
        assert_eq!(
            properties["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {job_id:?}: each job must be readable on its own id, got {properties}"
        );
    }
}

/// An unknown job is refused as a not-found resource with the service's refusal status,
/// on describe and on stop alike.
#[tokio::test]
async fn an_unknown_job_is_refused() {
    let router = router();
    for operation in [
        "DescribeEntitiesDetectionV2Job",
        "StopEntitiesDetectionV2Job",
    ] {
        let (status, answer) = call(&router, operation, &json!({"JobId": "doesnotexist"})).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {operation:?}: expected ResourceNotFoundException, got {answer}"
        );
    }
}

/// A started job moves out of `SUBMITTED` on its own, and ends in a terminal status
/// with an `EndTime` and a `Message` saying why — this emulator performs the medical
/// entities analysis of a document for no document, so no run is performed for a job of
/// this family and it ends in the modelled failure state naming the input nothing
/// analysed, rather than reporting a run nothing performed.
#[tokio::test]
async fn a_started_job_progresses_on_the_clock() {
    let router = router();
    let job_id = start_job(&router, vec![("JobName", json!("progress"))]).await;

    let fresh = describe_job(&router, &job_id).await;
    let status = fresh["JobStatus"].as_str().unwrap_or_default().to_string();
    assert!(
        ["SUBMITTED", "IN_PROGRESS"].contains(&status.as_str()),
        "case {job_id:?}: a job just started must be non-terminal, got {status:?}"
    );
    assert!(
        fresh["EndTime"].is_null(),
        "case {job_id:?}: a running job has no EndTime, got {fresh}"
    );

    wait(900);
    let settled = describe_job(&router, &job_id).await;
    assert_eq!(
        settled["JobStatus"].as_str(),
        Some("FAILED"),
        "case {job_id:?}: the job must reach its terminal status, got {settled}"
    );
    assert!(
        settled["EndTime"].as_f64() > settled["SubmitTime"].as_f64(),
        "case {job_id:?}: a finished job carries an EndTime after its SubmitTime, got {settled}"
    );
    assert!(
        settled["Message"]
            .as_str()
            .is_some_and(|message| message.contains("in-bucket")),
        "case {job_id:?}: a FAILED job must say why, naming the input it could not read, got {settled}"
    );
    assert!(
        settled["ManifestFilePath"].is_null(),
        "case {job_id:?}: no run wrote results, so no manifest is reported, got {settled}"
    );
}

/// Stopping a running job moves it to `STOP_REQUESTED`, and the clock then owes the
/// move to `STOPPED` with an `EndTime`. A later describe observes both.
#[tokio::test]
async fn stopping_a_job_is_observed_by_a_later_describe() {
    let router = router();
    let job_id = start_job(&router, vec![("JobName", json!("to-stop"))]).await;

    let (status, answer) = call(
        &router,
        "StopEntitiesDetectionV2Job",
        &json!({"JobId": job_id.clone()}),
    )
    .await;
    assert_eq!(status, 200, "stop a job: expected 200, got {status} {answer}");
    assert_eq!(
        answer["JobId"].as_str(),
        Some(job_id.as_str()),
        "stop a job: expected the JobId back, got {answer}"
    );

    let stopping = describe_job(&router, &job_id).await;
    assert_eq!(
        stopping["JobStatus"].as_str(),
        Some("STOP_REQUESTED"),
        "case {job_id:?}: a stop is requested before it takes effect, got {stopping}"
    );

    wait(600);
    let stopped = describe_job(&router, &job_id).await;
    assert_eq!(
        stopped["JobStatus"].as_str(),
        Some("STOPPED"),
        "case {job_id:?}: the requested stop must take effect, got {stopped}"
    );
    assert!(
        stopped["EndTime"].as_f64() > stopped["SubmitTime"].as_f64(),
        "case {job_id:?}: a stopped job carries an EndTime, got {stopped}"
    );

    // Stopping a job that already finished is refused, and leaves its terminal status
    // alone. The refusal itself is table-tested in `tests/all/refusal_messages.rs`.
    wait(50);
    let (status, answer) = call(
        &router,
        "StopEntitiesDetectionV2Job",
        &json!({"JobId": job_id.clone()}),
    )
    .await;
    assert_eq!(
        status, 400,
        "stopping a stopped job: expected 400, got {status} {answer}"
    );
    assert_eq!(
        answer["__type"].as_str(),
        Some("InvalidRequestException"),
        "stopping a stopped job: expected InvalidRequestException, got {answer}"
    );
    let still = describe_job(&router, &job_id).await;
    assert_eq!(
        still["JobStatus"].as_str(),
        Some("STOPPED"),
        "case {job_id:?}: a terminal status never changes again, got {still}"
    );
}

/// `ListEntitiesDetectionV2Jobs` pages through the jobs that were actually created, in
/// submit order, for any page size: every page carries at most `MaxResults` jobs, the
/// `NextToken` resumes exactly where the page stopped, and the last page carries none.
#[tokio::test]
async fn listing_jobs_pages_through_every_job_for_any_page_size() {
    let router = router();
    let mut created = Vec::new();
    for index in 0..5 {
        created.push(start_job(&router, vec![("JobName", json!(format!("job-{index}")))]).await);
    }

    for page_size in [1_usize, 2, 4, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let (status, answer) = call(&router, "ListEntitiesDetectionV2Jobs", &body).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status} {answer}"
            );
            let page = answer["ComprehendMedicalAsyncJobPropertiesList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must not exceed MaxResults, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|job| job["JobId"].as_str().unwrap_or_default().to_string()),
            );
            token = answer["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case page size {page_size}: expected every job in submit order, got {seen:?}"
        );
    }

    // A token this service never issued is refused, whether or not it happens to be
    // valid hex; a token is only a resume key when it decodes to a minted JobId's shape.
    for token in ["not-a-token", "61", "zz", "", &"ab".repeat(40)] {
        let (status, answer) = call(
            &router,
            "ListEntitiesDetectionV2Jobs",
            &json!({"NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token {token:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("InvalidRequestException"),
            "case token {token:?}: expected InvalidRequestException, got {answer}"
        );
    }
}

/// Every filter member narrows the list to the jobs that match it, and a filter that
/// matches nothing returns nothing.
#[tokio::test]
async fn listing_jobs_honours_every_filter_member() {
    let router = router();
    let alpha = start_job(&router, vec![("JobName", json!("demo-alpha"))]).await;
    wait(50);
    let midpoint = now_secs();
    wait(50);
    let beta = start_job(&router, vec![("JobName", json!("demo-beta"))]).await;

    // Both jobs reach their terminal status on the clock, so a status filter is stable.
    wait(900);

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "JobName names one job",
            json!({"Filter": {"JobName": "demo-beta"}}),
            vec![beta.clone()],
        ),
        (
            "JobName matches no job",
            json!({"Filter": {"JobName": "demo-absent"}}),
            vec![],
        ),
        (
            "JobStatus the jobs reached",
            json!({"Filter": {"JobStatus": "FAILED"}}),
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "JobStatus no job reached",
            json!({"Filter": {"JobStatus": "COMPLETED"}}),
            vec![],
        ),
        (
            "SubmitTimeBefore the midpoint",
            json!({"Filter": {"SubmitTimeBefore": midpoint}}),
            vec![alpha.clone()],
        ),
        (
            "SubmitTimeAfter the midpoint",
            json!({"Filter": {"SubmitTimeAfter": midpoint}}),
            vec![beta.clone()],
        ),
        (
            "every member at once",
            json!({"Filter": {
                "JobName": "demo-beta",
                "JobStatus": "FAILED",
                "SubmitTimeAfter": midpoint,
                "SubmitTimeBefore": now_secs() + 60.0,
            }}),
            vec![beta.clone()],
        ),
        (
            "members that cannot both hold",
            json!({"Filter": {"JobName": "demo-alpha", "SubmitTimeAfter": midpoint}}),
            vec![],
        ),
    ];

    for (name, body, expected) in cases {
        let (status, answer) = call(&router, "ListEntitiesDetectionV2Jobs", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} {answer}"
        );
        let listed: Vec<String> = answer["ComprehendMedicalAsyncJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|job| job["JobId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {name:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // The boundary instant itself: a bound equal to a job's own SubmitTime must not
    // lose that job from both halves of the split. Both bounds are inclusive, so the
    // job is returned by at least one of them whichever way the wire value rounds.
    let boundary = describe_job(&router, &alpha).await["SubmitTime"]
        .as_f64()
        .expect("a described job reports its SubmitTime");
    let mut found_at_boundary = Vec::new();
    for member in ["SubmitTimeBefore", "SubmitTimeAfter"] {
        let (status, answer) = call(
            &router,
            "ListEntitiesDetectionV2Jobs",
            &json!({"Filter": {member: boundary}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?} at the boundary: expected 200, got {status} {answer}"
        );
        if answer["ComprehendMedicalAsyncJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .any(|job| job["JobId"].as_str() == Some(alpha.as_str()))
        {
            found_at_boundary.push(member);
        }
    }
    assert!(
        !found_at_boundary.is_empty(),
        "case a bound at the job's own SubmitTime ({boundary}): the job must still be \
         reachable, got neither SubmitTimeBefore nor SubmitTimeAfter"
    );
}
