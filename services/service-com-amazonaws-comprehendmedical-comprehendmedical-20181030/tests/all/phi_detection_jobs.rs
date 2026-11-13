// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the asynchronous PHI-detection job family: the jobs
//! `StartPHIDetectionJob` records and the pages `ListPHIDetectionJobs` reports.
//!
//! The rule under test is that a list answers with the jobs this family actually holds —
//! every member the caller set on the filter narrows them, the order is the one the
//! filter's own documentation states, a page carries at most `MaxResults` of them and the
//! `NextToken` resumes exactly where the page stopped. Each table therefore spans a CLASS
//! of input: every page size from one job to more than exist, every filter member and
//! combinations of them, and both ends of each modelled bound with one value past it.
//!
//! Every request goes over the wire the service actually speaks (awsJson1_1, `POST /`
//! with `X-Amz-Target: ComprehendMedical_20181030.<Operation>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, so these tests exercise routing,
//! validation, the handler and serialization together.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// Wait out a transition the clock owes a job.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature, and each test awaits nothing else while it waits.
fn wait(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

/// Wall clock in epoch seconds, for the submit-time filter bounds.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

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

/// Start a job of `operation`'s family and return its `JobId`.
async fn start_job(
    router: &MultiServiceRouter,
    operation: &str,
    extra: Vec<(&str, Value)>,
) -> String {
    let (status, answer) = call(router, operation, &start_body(extra)).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: starting a job must answer 200, got {status} {answer}"
    );
    answer["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {operation:?}: expected a JobId, got {answer}"))
        .to_string()
}

/// List PHI jobs with `body`, asserting the call succeeded.
async fn list_phi_jobs(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, answer) = call(router, "ListPHIDetectionJobs", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {answer}"
    );
    answer
}

/// The `JobId` of every job of a list answer, in the order the answer reported them.
fn listed_ids(answer: &Value) -> Vec<String> {
    answer["ComprehendMedicalAsyncJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| job["JobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A started PHI job is reported by the list with exactly what the start call was given,
/// and repeating that same submission — one `ClientRequestToken` with the same
/// parameters — never starts a second job.
#[tokio::test]
async fn a_started_phi_job_is_listed_with_what_start_was_given() {
    let router = router();
    let job_id = start_job(
        &router,
        "StartPHIDetectionJob",
        vec![
            ("JobName", json!("phi-demo")),
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
        "StartPHIDetectionJob",
        vec![
            ("JobName", json!("phi-demo")),
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

    let answer = list_phi_jobs(&router, "every job of the family", &json!({})).await;
    assert_eq!(
        listed_ids(&answer),
        vec![job_id.clone()],
        "two starts of one submission must leave one job, got {answer}"
    );
    let properties = answer["ComprehendMedicalAsyncJobPropertiesList"][0].clone();
    for (member, expected) in [
        ("JobId", json!(job_id.clone())),
        ("JobName", json!("phi-demo")),
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
            "case {member:?}: the list must report what start was given, got {properties}"
        );
    }
    assert!(
        properties["SubmitTime"].as_f64().is_some_and(|at| at > 0.0),
        "a listed job carries the time it was submitted, got {properties}"
    );
    assert!(
        properties["ExpirationTime"].as_f64() > properties["SubmitTime"].as_f64(),
        "job metadata must expire after it was submitted, got {properties}"
    );
    assert_eq!(
        properties["ModelVersion"].as_str().map(str::is_empty),
        Some(false),
        "a listed job reports the model version, got {properties}"
    );
    assert!(
        answer["NextToken"].is_null(),
        "a page that ends the list carries no NextToken, got {answer}"
    );
}

/// `ListPHIDetectionJobs` pages through the jobs that were actually created, in submit
/// order, for any page size: every page carries at most `MaxResults` jobs, the
/// `NextToken` resumes exactly where the page stopped, and the last page carries none.
#[tokio::test]
async fn listing_phi_jobs_pages_through_every_job_for_any_page_size() {
    let router = router();
    let mut created = Vec::new();
    for index in 0..5 {
        created.push(
            start_job(
                &router,
                "StartPHIDetectionJob",
                vec![("JobName", json!(format!("phi-{index}")))],
            )
            .await,
        );
    }

    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(ref next) = token {
                body["NextToken"] = json!(next);
            }
            let case = format!("page size {page_size}");
            let answer = list_phi_jobs(&router, &case, &body).await;
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed MaxResults, got {}",
                page.len()
            );
            seen.extend(page);
            token = answer["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
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
    for token in ["not-a-token", "61", "zz", &"ab".repeat(40)] {
        let (status, answer) = call(
            &router,
            "ListPHIDetectionJobs",
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

/// Every filter member narrows the list to the jobs that match it, a filter that matches
/// nothing returns nothing, and the order is the one the filter documents: a list bounded
/// only by `SubmitTimeAfter` is newest first, every other list oldest first.
#[tokio::test]
async fn listing_phi_jobs_honours_every_filter_member_and_its_documented_order() {
    let router = router();
    let before_any = now_secs();
    let alpha = start_job(
        &router,
        "StartPHIDetectionJob",
        vec![("JobName", json!("phi-alpha"))],
    )
    .await;
    wait(50);
    let midpoint = now_secs();
    wait(50);
    let beta = start_job(
        &router,
        "StartPHIDetectionJob",
        vec![("JobName", json!("phi-beta"))],
    )
    .await;

    // Both jobs reach their terminal status on the clock, so a status filter is stable.
    wait(900);
    let later = now_secs();

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "JobName names one job",
            json!({"Filter": {"JobName": "phi-beta"}}),
            vec![beta.clone()],
        ),
        (
            "JobName matches no job",
            json!({"Filter": {"JobName": "phi-absent"}}),
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
            "SubmitTimeBefore both jobs: oldest first",
            json!({"Filter": {"SubmitTimeBefore": later}}),
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "SubmitTimeAfter both jobs: newest first",
            json!({"Filter": {"SubmitTimeAfter": before_any}}),
            vec![beta.clone(), alpha.clone()],
        ),
        (
            "both time bounds: oldest first",
            json!({"Filter": {"SubmitTimeAfter": before_any, "SubmitTimeBefore": later}}),
            vec![alpha.clone(), beta.clone()],
        ),
        (
            "every member at once",
            json!({"Filter": {
                "JobName": "phi-beta",
                "JobStatus": "FAILED",
                "SubmitTimeAfter": midpoint,
                "SubmitTimeBefore": later,
            }}),
            vec![beta.clone()],
        ),
        (
            "members that cannot both hold",
            json!({"Filter": {"JobName": "phi-alpha", "SubmitTimeAfter": midpoint}}),
            vec![],
        ),
    ];

    for (name, body, expected) in cases {
        let answer = list_phi_jobs(&router, name, &body).await;
        let listed = listed_ids(&answer);
        assert_eq!(
            listed, expected,
            "case {name:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Paging follows the order it reports: a newest-first list resumed by its own
    // NextToken continues at the next OLDER job, and never repeats one.
    let newest_first = list_phi_jobs(
        &router,
        "the first newest-first page",
        &json!({"MaxResults": 1, "Filter": {"SubmitTimeAfter": before_any}}),
    )
    .await;
    assert_eq!(
        listed_ids(&newest_first),
        vec![beta.clone()],
        "case a newest-first page: expected the newest job, got {newest_first}"
    );
    let token = newest_first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with a job after it carries a NextToken, got {newest_first}"))
        .to_string();
    let resumed = list_phi_jobs(
        &router,
        "the resumed newest-first page",
        &json!({"MaxResults": 1, "NextToken": token, "Filter": {"SubmitTimeAfter": before_any}}),
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        vec![alpha.clone()],
        "case a resumed newest-first page: expected the older job, got {resumed}"
    );
}

/// Each job family holds its own jobs: a PHI job is never reported by another family's
/// list, and a `ClientRequestToken` is idempotent only within the family that issued the
/// job it created.
#[tokio::test]
async fn each_job_family_lists_only_its_own_jobs() {
    let router = router();
    let phi = start_job(
        &router,
        "StartPHIDetectionJob",
        vec![("JobName", json!("shared")), ("ClientRequestToken", json!("tok"))],
    )
    .await;
    let entities = start_job(
        &router,
        "StartEntitiesDetectionV2Job",
        vec![("JobName", json!("shared")), ("ClientRequestToken", json!("tok"))],
    )
    .await;
    assert_ne!(
        phi, entities,
        "case {:?}: one token per family, so two families mint two jobs, got {phi:?} twice",
        "tok"
    );

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("ListPHIDetectionJobs", vec![phi.clone()]),
        ("ListEntitiesDetectionV2Jobs", vec![entities.clone()]),
    ];
    for (operation, expected) in cases {
        let (status, answer) = call(&router, operation, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected 200, got {status} {answer}"
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
            "case {operation:?}: a list reports only its own family, got {listed:?}"
        );
    }
}

/// A list request outside a modelled bound is refused with this service's own client
/// error, and both ends of each bound are served.
///
/// The bounds are the model's own: `MaxResults` has `range 1..500` and the filter's
/// `JobName` `length 1..256` with `pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`
/// (`smithy_query(query="operation",operation="ListPHIDetectionJobs")` and
/// `smithy_query(query="shape",shape="ComprehendMedicalAsyncJobFilter")`).
#[tokio::test]
async fn a_list_request_outside_a_modelled_bound_is_refused() {
    let router = router();
    let longest_name = "a".repeat(256);
    let too_long_name = "a".repeat(257);
    // (case, request, the status expected)
    let cases: Vec<(&str, Value, u16)> = vec![
        ("MaxResults at the range minimum", json!({"MaxResults": 1}), 200),
        ("MaxResults at the range maximum", json!({"MaxResults": 500}), 200),
        ("MaxResults one below the minimum", json!({"MaxResults": 0}), 400),
        ("MaxResults one above the maximum", json!({"MaxResults": 501}), 400),
        (
            "a filter JobName at the length minimum",
            json!({"Filter": {"JobName": "a"}}),
            200,
        ),
        (
            "a filter JobName at the length maximum",
            json!({"Filter": {"JobName": longest_name}}),
            200,
        ),
        (
            "a filter JobName one past the length maximum",
            json!({"Filter": {"JobName": too_long_name}}),
            400,
        ),
        (
            "an empty filter JobName, below the length minimum",
            json!({"Filter": {"JobName": ""}}),
            400,
        ),
        (
            "a filter JobName outside the modelled pattern",
            json!({"Filter": {"JobName": "not#a#valid#name!"}}),
            400,
        ),
        (
            "a filter JobName the pattern allows",
            json!({"Filter": {"JobName": "job.name-1_2:3/4=5+6%7@8"}}),
            200,
        ),
        (
            "a filter JobStatus the enum does not carry",
            json!({"Filter": {"JobStatus": "ALMOST_DONE"}}),
            400,
        ),
    ];

    for (name, body, expected_status) in cases {
        let (status, answer) = call(&router, "ListPHIDetectionJobs", &body).await;
        assert_eq!(
            status, expected_status,
            "case {name:?}: expected {expected_status}, got {status} {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer["__type"].as_str(),
                Some("InvalidRequestException"),
                "case {name:?}: expected InvalidRequestException, got {answer}"
            );
        } else {
            assert!(
                answer["ComprehendMedicalAsyncJobPropertiesList"].is_array(),
                "case {name:?}: an accepted request lists the family's jobs, got {answer}"
            );
        }
    }
}
