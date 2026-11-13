// SPDX-License-Identifier: CC0-1.0

//! How the pipeline control plane WORDS a refusal, and when a start is a refusal
//! at all — driven end to end over HTTP.
//!
//! Three rules are under test, each over a table whose rows differ along what the
//! rule generalises over rather than repeating one request:
//!
//! 1. A refusal carries the wording the service uses for THAT operation and that
//!    error class, and its only variable parts are the ones the wording has: a
//!    name the request named, or a status read out of stored state. Text the
//!    service keeps fixed stays fixed — a submitted number is never interpolated
//!    into it, and an operation whose wording names no resource echoes none. The
//!    rows vary the operation, the error class, the pipeline name's length and
//!    shape, the submitted unit counts, and whether the pipeline was never
//!    created or created and then deleted.
//! 2. `StartPipeline` refuses a pipeline whose status is not one a start is
//!    eligible from, and the refusal enumerates the eligible statuses (compared
//!    as a SET, because the order carries no meaning) and states the status the
//!    pipeline is in. The rows vary the ineligible status: running, mid-create
//!    and mid-stop.
//! 3. A start of a pipeline that an accepted start already put on its way to
//!    running is accepted idempotently — every duplicate answers 200 with the
//!    same in-flight pipeline, and the resource makes exactly ONE transition.
//!    The rows vary the pipeline (name shape and unit counts) and fire the
//!    duplicates both concurrently and staggered inside the transition span.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is not the emulator default, so nothing under test can depend on
/// a default-region code path.
const REGION: &str = "eu-west-2";

/// The statuses a start is eligible from, written out here rather than read from
/// the service so the message is compared against the rule and not against the
/// code that renders it.
///
/// Source: the `PipelineStatus` values a pipeline rests in while it is NOT running
/// (`smithy_query(query="shape", shape="PipelineStatus")` lists CREATING, ACTIVE,
/// UPDATING, DELETING, CREATE_FAILED, UPDATE_FAILED, STARTING, START_FAILED,
/// STOPPING, STOPPED) — stopped, or left failed by a previous start or create.
const START_ELIGIBLE: [&str; 3] = ["STOPPED", "START_FAILED", "CREATE_FAILED"];

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol may
    /// render the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }

    /// One member of the returned `Pipeline`.
    fn member(&self, name: &str) -> &Value {
        &self.body["Pipeline"][name]
    }

    fn status_value(&self) -> String {
        self.member("Status")
            .as_str()
            .unwrap_or_else(|| panic!("expected a Status in {:?}", self.body))
            .to_string()
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised
    // elsewhere; these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

fn create_body(name: &str, min_units: i64, max_units: i64) -> Value {
    json!({
        "PipelineName": name,
        "MinUnits": min_units,
        "MaxUnits": max_units,
        "PipelineConfigurationBody": "version: \"2\"\n",
    })
}

async fn create_pipeline(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/2022-01-01/osis/createPipeline", Some(body)).await
}

async fn get_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipeline/{name}"),
        None,
    )
    .await
}

async fn start_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/startPipeline/{name}"),
        None,
    )
    .await
}

async fn stop_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/stopPipeline/{name}"),
        None,
    )
    .await
}

async fn delete_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2022-01-01/osis/deletePipeline/{name}"),
        None,
    )
    .await
}

/// Create a pipeline and read it back, so the caller starts from a running pipeline: the
/// `CREATING` -> `ACTIVE` transition the create armed is due as soon as the create has
/// answered.
async fn active_pipeline(router: &MultiServiceRouter, name: &str, min_units: i64, max_units: i64) {
    let created = create_pipeline(router, create_body(name, min_units, max_units)).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected the pipeline to be created, got {} / {:?}",
        created.status, created.body
    );
    let read = get_pipeline(router, name).await;
    assert_eq!(
        read.status_value(),
        "ACTIVE",
        "case {name:?}: expected the created pipeline to reach ACTIVE, got {:?}",
        read.body
    );
}

/// Stop a running pipeline and read it back, so the caller starts from a stopped one.
async fn stopped_pipeline(router: &MultiServiceRouter, name: &str, min_units: i64, max_units: i64) {
    active_pipeline(router, name, min_units, max_units).await;
    let stopped = stop_pipeline(router, name).await;
    assert_eq!(
        stopped.status, 200,
        "case {name:?}: expected the pipeline to be stopped, got {} / {:?}",
        stopped.status, stopped.body
    );
    let read = get_pipeline(router, name).await;
    assert_eq!(
        read.status_value(),
        "STOPPED",
        "case {name:?}: expected the stopped pipeline to reach STOPPED, got {:?}",
        read.body
    );
}

/// What one refusal row expects of the answer.
struct Refusal {
    case: &'static str,
    status: u16,
    error_type: &'static str,
    message: String,
    /// Text that must NOT appear in the message — a value the service keeps out
    /// of this wording (a submitted count, or a name it does not echo).
    absent: Vec<String>,
}

/// Every refusal carries its own operation's wording, with only the variable
/// parts that wording has.
#[tokio::test]
async fn a_refusal_carries_the_wording_of_the_operation_that_refused_it() {
    let router = new_router();
    let mut rows: Vec<(Answer, Refusal)> = Vec::new();

    // A duplicate create, over names at both published `PipelineName` length
    // bounds (`@length(3, 28)`) and one of mixed shape: the name is the single
    // variable part, quoted.
    for (case, name) in [
        ("shortest legal name", "abc"),
        ("longest legal name", "a-very-long-pipeline-name-28"),
        ("hyphen-and-digit name", "p-1-2-3"),
    ] {
        let first = create_pipeline(&router, create_body(name, 1, 1)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected the first create to succeed, got {} / {:?}",
            first.status, first.body
        );
        // A second create of the same name, with a different body and different
        // unit counts, so nothing but the name can decide the refusal.
        let again = create_pipeline(&router, create_body(name, 2, 4)).await;
        rows.push((
            again,
            Refusal {
                case,
                status: 409,
                error_type: "ResourceAlreadyExistsException",
                message: format!("Pipeline with given name \"{name}\" already exists"),
                absent: Vec::new(),
            },
        ));
    }

    // MinUnits above MaxUnits, over pairs that differ in magnitude and distance:
    // the wording is entirely fixed, so neither submitted count appears in it.
    for (min_units, max_units) in [(2_i64, 1_i64), (5, 1), (10, 9), (100, 3)] {
        let refused = create_pipeline(
            &router,
            create_body(&format!("sized-{min_units}-{max_units}"), min_units, max_units),
        )
        .await;
        rows.push((
            refused,
            Refusal {
                case: "min above max",
                status: 400,
                error_type: "ValidationException",
                message: "minUnits cannot be greater than maxUnits".to_string(),
                absent: vec![min_units.to_string(), max_units.to_string()],
            },
        ));
    }

    // A name-addressed start and stop that find nothing: the start's wording
    // names no pipeline, the stop's names the one the request named. Rows differ
    // in name shape and in whether the name was never used or created and then
    // deleted.
    for (case, name, create_first) in [
        ("never created", "absent-pipeline", false),
        ("shortest legal name", "xyz", false),
        ("created and then deleted", "deleted-pipeline", true),
    ] {
        if create_first {
            active_pipeline(&router, name, 1, 2).await;
            let deleted = delete_pipeline(&router, name).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected the pipeline to be deleted, got {} / {:?}",
                deleted.status, deleted.body
            );
        }
        rows.push((
            start_pipeline(&router, name).await,
            Refusal {
                case,
                status: 404,
                error_type: "ResourceNotFoundException",
                message: "No pipeline settings found".to_string(),
                absent: vec![name.to_string()],
            },
        ));
        rows.push((
            stop_pipeline(&router, name).await,
            Refusal {
                case,
                status: 404,
                error_type: "ResourceNotFoundException",
                message: format!(
                    "The pipeline {name} could not be stopped because it does not exist"
                ),
                absent: Vec::new(),
            },
        ));
    }

    for (answer, expected) in rows {
        let case = expected.case;
        assert_eq!(
            answer.status, expected.status,
            "case {case:?}: expected status {}, got {} / {:?}",
            expected.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, expected.error_type,
            "case {case:?}: expected {}, got {:?}",
            expected.error_type, answer.error_type
        );
        let message = answer.message();
        assert_eq!(
            message, expected.message,
            "case {case:?}: expected {:?}, got {message:?}",
            expected.message
        );
        for absent in &expected.absent {
            assert!(
                !message.contains(absent),
                "case {case:?}: {absent:?} is not part of this wording, got {message:?}"
            );
        }
    }
}

/// Split `Only pipelines with one of the following statuses are eligible for
/// starting: <statuses>. The current status is <status>.` into the eligible set
/// and the current status, or `None` when the message is not that wording.
fn parse_eligibility_refusal(message: &str) -> Option<(BTreeSet<String>, String)> {
    const HEAD: &str =
        "Only pipelines with one of the following statuses are eligible for starting: ";
    const MIDDLE: &str = ". The current status is ";
    let rest = message.strip_prefix(HEAD)?;
    let tail = rest.strip_suffix('.')?;
    let (statuses, current) = tail.split_once(MIDDLE)?;
    Some((
        statuses
            .split(", ")
            .map(|status| status.trim().to_string())
            .collect(),
        current.to_string(),
    ))
}

/// A start of a pipeline in a status that is not eligible is refused, and the
/// refusal states which statuses are and which one the pipeline is in.
#[tokio::test]
async fn starting_an_ineligible_pipeline_states_the_eligible_statuses_and_the_current_one() {
    let router = new_router();

    // Rows differ in how the pipeline reached the running status a start is refused from:
    // created, and stopped and started again. A pipeline mid-change is ineligible too and
    // the wording names the transient status, but the transition a lifecycle call arms is
    // due the instant that call answers
    // (`service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN`),
    // so only that call's own response carries a transient status and a later start always
    // meets a settled one.
    let running = "running-pipeline";
    active_pipeline(&router, running, 1, 1).await;

    let restarted = "restarted-pipeline";
    stopped_pipeline(&router, restarted, 2, 5).await;
    let started = start_pipeline(&router, restarted).await;
    assert_eq!(
        started.status, 200,
        "case \"restarted\": expected the start to succeed, got {} / {:?}",
        started.status, started.body
    );
    let read = get_pipeline(&router, restarted).await;
    assert_eq!(
        read.status_value(),
        "ACTIVE",
        "case \"restarted\": expected the restarted pipeline to be running, got {:?}",
        read.body
    );

    let expected_eligible: BTreeSet<String> =
        START_ELIGIBLE.iter().map(|status| status.to_string()).collect();

    for (case, name, current) in [
        ("a pipeline running since it was created", running, "ACTIVE"),
        ("a pipeline running again after a restart", restarted, "ACTIVE"),
    ] {
        let refused = start_pipeline(&router, name).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_type
        );
        let message = refused.message();
        let (eligible, reported) = parse_eligibility_refusal(&message).unwrap_or_else(|| {
            panic!("case {case:?}: expected the eligibility wording, got {message:?}")
        });
        // The order of the list carries no meaning, so it is compared as a set.
        assert_eq!(
            eligible, expected_eligible,
            "case {case:?}: expected the eligible statuses {expected_eligible:?}, got {eligible:?}"
        );
        assert_eq!(
            reported, current,
            "case {case:?}: expected the pipeline's current status {current:?}, got {reported:?}"
        );
        assert!(
            !message.contains(name),
            "case {case:?}: this wording names no pipeline, got {message:?}"
        );

        // The refusal changed nothing: the pipeline is in the status it was in.
        let read = get_pipeline(&router, name).await;
        assert_eq!(
            read.status_value(),
            current,
            "case {case:?}: a refused start must leave the status alone, got {:?}",
            read.body
        );
    }
}

/// A duplicate start of a pipeline that is already on its way to running is
/// accepted, and the pipeline transitions exactly once.
#[tokio::test]
async fn a_duplicate_start_is_accepted_and_the_pipeline_transitions_once() {
    // Rows differ in the pipeline's name shape and unit counts, so nothing under
    // test depends on one request's values.
    let cases = [
        ("shortest legal name", "sss", 1_i64, 1_i64),
        ("hyphenated name", "idempotent-start-1", 2, 5),
        ("longest legal name", "a-very-long-pipeline-name-28", 3, 3),
    ];

    for (case, name, min_units, max_units) in cases {
        let router = new_router();
        stopped_pipeline(&router, name, min_units, max_units).await;

        // Two starts in flight at once, then a third inside the same transition
        // span: every one of them is the same start.
        let (first, second) = tokio::join!(
            start_pipeline(&router, name),
            start_pipeline(&router, name)
        );
        let third = start_pipeline(&router, name).await;

        for (ordinal, answer) in [&first, &second, &third].into_iter().enumerate() {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: duplicate start {ordinal} must be accepted, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.member("PipelineName"),
                &json!(name),
                "case {case:?}: duplicate start {ordinal} must answer with the pipeline, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.status_value(),
                "STARTING",
                "case {case:?}: duplicate start {ordinal} must report the transition in flight, \
                 got {:?}",
                answer.body
            );
            // One transition means one write: every duplicate reports the instant
            // the accepted start stamped, not a new one.
            assert_eq!(
                answer.member("LastUpdatedAt"),
                first.member("LastUpdatedAt"),
                "case {case:?}: duplicate start {ordinal} must not restart the transition, \
                 got {:?} against {:?}",
                answer.member("LastUpdatedAt"),
                first.member("LastUpdatedAt")
            );
        }

        // The single transition the accepted start owes comes due, and the
        // pipeline is running.
        let read = get_pipeline(&router, name).await;
        assert_eq!(
            read.status_value(),
            "ACTIVE",
            "case {case:?}: the started pipeline must reach ACTIVE, got {:?}",
            read.body
        );
        assert_eq!(
            read.member("StatusReason"),
            &json!(null),
            "case {case:?}: a settled pipeline carries no status reason, got {:?}",
            read.member("StatusReason")
        );

        // And once it is running, a start is refused again: idempotence covers a
        // start already in flight, not a running pipeline.
        let refused = start_pipeline(&router, name).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: starting a running pipeline must be refused, got {} / {:?}",
            refused.status, refused.body
        );
    }
}
