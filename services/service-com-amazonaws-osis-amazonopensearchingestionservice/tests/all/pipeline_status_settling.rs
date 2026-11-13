// SPDX-License-Identifier: CC0-1.0

//! Cover for the rule a client that polls a pipeline's status depends on: the transition a
//! lifecycle call arms (`CREATING` -> `ACTIVE`, `STOPPING` -> `STOPPED`, `STARTING` ->
//! `ACTIVE`, `UPDATING` -> back to the status the pipeline held) is due the instant that
//! call has answered, so the request that follows it is served the settled status and
//! every leg of the lifecycle is observable on the next read.
//!
//! Nothing here waits. No case sleeps, measures elapsed time or bounds anything by a
//! clock: each assertion is about the FIRST request that follows a lifecycle call, and the
//! polling cases count READS rather than seconds. That is exactly the property a polling
//! client needs — a leg that settles only after a wall-clock span costs such a client a
//! whole poll interval per leg, however short the span is.
//!
//! The rows of each table differ along what the rule generalises over: which lifecycle
//! call armed the transition, which status it settles into, which operation reports it
//! (`GetPipeline` or `ListPipelines`), and the pipeline's name shape and unit counts.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so nothing here depends on the fallback.
const REGION: &str = "eu-west-2";

/// How many reads a polling case will take before it gives up.
///
/// Generous: the rule under test is that ONE read is enough, so a case that needs more
/// than this is not slow, it is broken. Counting reads keeps the cases independent of how
/// fast the host runs — nothing here sleeps or looks at a clock.
const READ_BUDGET: usize = 8;

/// The statuses eligible for a start, as `StartPipeline`'s `ConflictException` enumerates
/// them (`smithy_query(query="shape", shape="PipelineStatus")` lists the full enum:
/// CREATING, ACTIVE, UPDATING, DELETING, CREATE_FAILED, UPDATE_FAILED, STARTING,
/// START_FAILED, STOPPING, STOPPED).
const START_ELIGIBLE: &str = "STOPPED, START_FAILED, CREATE_FAILED";

/// What one call answered: the HTTP status, the modelled error code (empty on success) and
/// the parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
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

    /// The error message the response carries, under either casing a protocol may render
    /// the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
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
        // The credential scope is how a handler learns the region its ARNs name.
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

/// A create request: `PipelineName` is `@length(3, 28)` / `^[a-z][a-z0-9\-]+$` and
/// `MinUnits` / `MaxUnits` are `@range(1,)` (smithy_query: operation CreatePipeline).
fn create_body(name: &str, min_units: i64, max_units: i64) -> Value {
    json!({
        "PipelineName": name,
        "MinUnits": min_units,
        "MaxUnits": max_units,
        "PipelineConfigurationBody": "version: \"2\"\nsettling-pipeline:\n  source:\n    http:\n",
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

async fn stop_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/stopPipeline/{name}"),
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

/// `UpdatePipeline` is `PUT /2022-01-01/osis/updatePipeline/{PipelineName}`: the name is an
/// `@httpLabel`, so the body carries only the members being changed.
async fn update_pipeline(router: &MultiServiceRouter, name: &str, patch: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/updatePipeline/{name}"),
        Some(patch),
    )
    .await
}

async fn list_pipelines(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/2022-01-01/osis/listPipelines", None).await
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

/// The status every pipeline a listing reports is in, keyed by pipeline name.
fn listed_statuses(listed: &Answer) -> BTreeMap<String, String> {
    listed.body["Pipelines"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            (
                summary["PipelineName"].as_str().unwrap_or_default().to_string(),
                summary["Status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn expect_ok(case: &str, step: &str, answer: Answer) -> Answer {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {step}, got {} / {:?}",
        answer.status, answer.body
    );
    answer
}

/// Read `GetPipeline` until the pipeline's status is one of `targets`, the way a client
/// polls, and answer with the status reached and how many reads it took.
///
/// The budget is a number of READS, not a duration: nothing here sleeps, so a slow host
/// makes the case slower, never different.
async fn poll_for_status(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    targets: &[&str],
) -> (String, usize) {
    let mut last = String::new();
    for reads in 1..=READ_BUDGET {
        let read = get_pipeline(router, name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPipeline while polling, got {} / {:?}",
            read.status, read.body
        );
        last = read.status_value();
        if targets.contains(&last.as_str()) {
            return (last, reads);
        }
    }
    panic!(
        "case {case:?}: the pipeline never reached any of {targets:?} in {READ_BUDGET} reads, \
         last status {last:?}"
    )
}

/// Which lifecycle call a row exercises, and what the pipeline has to be doing first.
#[derive(Debug, Clone, Copy)]
enum Leg {
    /// `CreatePipeline` on a name nothing holds.
    Create,
    /// `StopPipeline` on a running pipeline.
    Stop,
    /// `StartPipeline` on a stopped pipeline.
    Start,
    /// `UpdatePipeline` on a running pipeline.
    UpdateRunning,
    /// `UpdatePipeline` on a stopped pipeline.
    UpdateStopped,
}

impl Leg {
    fn call_name(self) -> &'static str {
        match self {
            Leg::Create => "CreatePipeline",
            Leg::Stop => "StopPipeline",
            Leg::Start => "StartPipeline",
            Leg::UpdateRunning | Leg::UpdateStopped => "UpdatePipeline",
        }
    }
}

/// Drive `name` to the point where `leg` has just been called, and answer with THAT call's
/// own response. Every step runs through the public API, and the status is read back
/// between steps the way a client does.
async fn drive_to_leg(
    router: &MultiServiceRouter,
    case: &str,
    leg: Leg,
    name: &str,
    min_units: i64,
    max_units: i64,
) -> Answer {
    let patch = json!({
        "PipelineConfigurationBody": "version: \"2\"\nsettling-pipeline:\n  source:\n    otel_trace_source:\n",
    });
    let created = expect_ok(
        case,
        "CreatePipeline",
        create_pipeline(router, create_body(name, min_units, max_units)).await,
    );
    if matches!(leg, Leg::Create) {
        return created;
    }
    let (running, _) = poll_for_status(router, case, name, &["ACTIVE"]).await;
    assert_eq!(
        running, "ACTIVE",
        "case {case:?}: the created pipeline must come up, got {running:?}"
    );
    if matches!(leg, Leg::UpdateRunning) {
        return expect_ok(case, "UpdatePipeline", update_pipeline(router, name, patch).await);
    }
    let stopped = expect_ok(case, "StopPipeline", stop_pipeline(router, name).await);
    if matches!(leg, Leg::Stop) {
        return stopped;
    }
    let (halted, _) = poll_for_status(router, case, name, &["STOPPED"]).await;
    assert_eq!(
        halted, "STOPPED",
        "case {case:?}: the stopped pipeline must settle, got {halted:?}"
    );
    match leg {
        Leg::UpdateStopped => {
            expect_ok(case, "UpdatePipeline", update_pipeline(router, name, patch).await)
        }
        _ => expect_ok(case, "StartPipeline", start_pipeline(router, name).await),
    }
}

/// Every lifecycle call answers with the transient status it is modelled to report, and
/// the FIRST `GetPipeline` that follows reports the settled one — with no wait.
#[tokio::test]
async fn every_lifecycle_leg_settles_by_the_first_status_report() {
    // Rows differ in the lifecycle call that armed the transition, in the status it
    // settles into, and in the pipeline's name shape and unit counts. The name lengths sit
    // on both published `@length(3, 28)` bounds.
    let cases = [
        ("a create", Leg::Create, "abc", 1_i64, 1_i64, "CREATING", "ACTIVE"),
        ("a stop", Leg::Stop, "settling-stop", 2, 4, "STOPPING", "STOPPED"),
        (
            "a start",
            Leg::Start,
            "a-settling-start-pipeline-28",
            3,
            3,
            "STARTING",
            "ACTIVE",
        ),
        (
            "an update of a running pipeline",
            Leg::UpdateRunning,
            "settling-update-running",
            1,
            9,
            "UPDATING",
            "ACTIVE",
        ),
        (
            "an update of a stopped pipeline",
            Leg::UpdateStopped,
            "settling-update-stopped",
            4,
            12,
            "UPDATING",
            "STOPPED",
        ),
    ];

    for (case, leg, name, min_units, max_units, transient, settled) in cases {
        let router = new_router();
        let armed = drive_to_leg(&router, case, leg, name, min_units, max_units).await;

        // The call that arms the transition reports the transient status, with the reason
        // that describes the move in flight.
        assert_eq!(
            armed.status_value(),
            transient,
            "case {case:?}: {} must answer with {transient:?}, got {:?}",
            leg.call_name(),
            armed.body
        );
        assert!(
            armed.member("StatusReason")["Description"].is_string(),
            "case {case:?}: a pipeline mid-change carries a StatusReason, got {:?}",
            armed.member("StatusReason")
        );

        // The very next status report — one read, no waiting — carries the settled status.
        let read = expect_ok(case, "GetPipeline", get_pipeline(&router, name).await);
        assert_eq!(
            read.status_value(),
            settled,
            "case {case:?}: the first report after {} must carry {settled:?}, got {:?}",
            leg.call_name(),
            read.body
        );
        assert_eq!(
            read.member("StatusReason"),
            &json!(null),
            "case {case:?}: a settled pipeline carries no StatusReason, got {:?}",
            read.member("StatusReason")
        );

        // The transition is materialised once: a further read reports the same status and
        // the same instant, so reading does not move the pipeline on again.
        let again = expect_ok(case, "GetPipeline", get_pipeline(&router, name).await);
        assert_eq!(
            again.status_value(),
            settled,
            "case {case:?}: a repeated report must stay at {settled:?}, got {:?}",
            again.body
        );
        assert_eq!(
            again.member("LastUpdatedAt"),
            read.member("LastUpdatedAt"),
            "case {case:?}: a repeated report must not restamp the pipeline, got {:?} \
             against {:?}",
            again.member("LastUpdatedAt"),
            read.member("LastUpdatedAt")
        );
        assert!(
            again.member("LastUpdatedAt").as_f64() >= again.member("CreatedAt").as_f64(),
            "case {case:?}: LastUpdatedAt must not precede CreatedAt, got {:?} / {:?}",
            again.member("LastUpdatedAt"),
            again.member("CreatedAt")
        );
    }
}

/// A client that drives the whole lifecycle — wait for ACTIVE, stop, wait for STOPPED,
/// start, wait for ACTIVE — completes it, and spends exactly one read on each leg.
#[tokio::test]
async fn a_polling_client_spends_one_read_on_each_lifecycle_leg() {
    // Rows differ in name shape and unit counts, so nothing under test is pinned to one
    // request's values.
    let cases = [
        ("shortest legal name", "pol", 1_i64, 1_i64),
        ("hyphenated name", "polling-client-1", 2, 6),
        ("longest legal name", "a-polling-lifecycle-cases-28", 3, 3),
    ];

    for (case, name, min_units, max_units) in cases {
        let router = new_router();
        expect_ok(
            case,
            "CreatePipeline",
            create_pipeline(&router, create_body(name, min_units, max_units)).await,
        );

        for (leg, targets) in [
            ("the create", &["ACTIVE", "CREATE_FAILED"][..]),
            ("the stop", &["STOPPED", "START_FAILED"][..]),
            ("the start", &["ACTIVE", "START_FAILED"][..]),
        ] {
            let (reached, reads) = poll_for_status(&router, case, name, targets).await;
            assert_eq!(
                reads, 1,
                "case {case:?}: {leg} must have settled by the first read, got {reached:?} \
                 after {reads} reads"
            );
            assert_eq!(
                reached,
                targets[0],
                "case {case:?}: {leg} must settle into {:?}, got {reached:?}",
                targets[0]
            );

            match leg {
                // A running pipeline is not eligible for starting, and the refusal states
                // the eligible set and the status the pipeline is in instead.
                "the create" => {
                    let refused = start_pipeline(&router, name).await;
                    assert_eq!(
                        refused.status, 409,
                        "case {case:?}: starting a running pipeline must be refused, got {} / {:?}",
                        refused.status, refused.body
                    );
                    assert_eq!(
                        refused.error_type, "ConflictException",
                        "case {case:?}: expected ConflictException, got {:?}",
                        refused.error_type
                    );
                    assert_eq!(
                        refused.message(),
                        format!(
                            "Only pipelines with one of the following statuses are eligible for \
                             starting: {START_ELIGIBLE}. The current status is ACTIVE."
                        ),
                        "case {case:?}: unexpected refusal wording, got {:?}",
                        refused.message()
                    );
                    // The refusal moved nothing.
                    let read = get_pipeline(&router, name).await;
                    assert_eq!(
                        read.status_value(),
                        "ACTIVE",
                        "case {case:?}: a refused start must leave the pipeline running, got {:?}",
                        read.body
                    );
                    let stopped = expect_ok(case, "StopPipeline", stop_pipeline(&router, name).await);
                    assert_eq!(
                        stopped.status_value(),
                        "STOPPING",
                        "case {case:?}: StopPipeline must answer with STOPPING, got {:?}",
                        stopped.body
                    );
                }
                "the stop" => {
                    let started =
                        expect_ok(case, "StartPipeline", start_pipeline(&router, name).await);
                    assert_eq!(
                        started.status_value(),
                        "STARTING",
                        "case {case:?}: StartPipeline must answer with STARTING, got {:?}",
                        started.body
                    );
                }
                _ => {}
            }
        }
    }
}

/// A lifecycle call that follows another one, with no read in between, meets the settled
/// status too: the transition is due to every request, not only to a report.
#[tokio::test]
async fn a_lifecycle_call_that_follows_another_meets_the_settled_status() {
    // Rows differ in the pair of calls chained back to back, in the transient status the
    // first one answers with and in the settled status the second one is served.
    let cases = [
        ("a stop straight after the create", "chain-stop", 1_i64, 1_i64, "stop"),
        ("a start straight after the stop", "chain-start", 2, 7, "start"),
        ("a stop straight after the start", "chain-restop", 3, 3, "restop"),
        ("a delete straight after the create", "chain-delete", 4, 9, "delete"),
    ];

    for (case, name, min_units, max_units, chain) in cases {
        let router = new_router();
        let created = expect_ok(
            case,
            "CreatePipeline",
            create_pipeline(&router, create_body(name, min_units, max_units)).await,
        );
        assert_eq!(
            created.status_value(),
            "CREATING",
            "case {case:?}: CreatePipeline must answer with CREATING, got {:?}",
            created.body
        );

        match chain {
            "delete" => {
                // The create's transition is due, so the pipeline is running and can be
                // deleted; the record is gone from the read that follows.
                expect_ok(case, "DeletePipeline", delete_pipeline(&router, name).await);
                let gone = get_pipeline(&router, name).await;
                assert_eq!(
                    gone.status, 404,
                    "case {case:?}: the deleted pipeline must be gone, got {} / {:?}",
                    gone.status, gone.body
                );
            }
            _ => {
                let stopped = expect_ok(case, "StopPipeline", stop_pipeline(&router, name).await);
                assert_eq!(
                    stopped.status_value(),
                    "STOPPING",
                    "case {case:?}: a stop of the running pipeline must answer with STOPPING, \
                     got {:?}",
                    stopped.body
                );
                if chain == "stop" {
                    continue;
                }
                // A start straight after the stop: the stop's transition is due, so the
                // pipeline is STOPPED and the start is eligible.
                let started =
                    expect_ok(case, "StartPipeline", start_pipeline(&router, name).await);
                assert_eq!(
                    started.status_value(),
                    "STARTING",
                    "case {case:?}: a start of the stopped pipeline must answer with STARTING, \
                     got {:?}",
                    started.body
                );
                if chain == "start" {
                    continue;
                }
                // And a stop straight after that start: the start's transition is due, so
                // the pipeline is running and stoppable again.
                let restopped =
                    expect_ok(case, "StopPipeline", stop_pipeline(&router, name).await);
                assert_eq!(
                    restopped.status_value(),
                    "STOPPING",
                    "case {case:?}: a stop of the started pipeline must answer with STOPPING, \
                     got {:?}",
                    restopped.body
                );
            }
        }
    }
}

/// `ListPipelines` reports every pipeline in its settled status too: the listing is a
/// status report, so a client polling it sees each leg complete on its next page.
#[tokio::test]
async fn a_listing_reports_every_pipeline_in_its_settled_status() {
    let router = new_router();
    // Pipelines differing in name shape and unit counts, created back to back with no read
    // in between, so one listing is the first report of all three.
    let names = [("listed-one", 1_i64, 1_i64), ("listed-two", 2, 5), ("l-3", 3, 3)];
    for (name, min_units, max_units) in names {
        expect_ok(
            "a listing of freshly created pipelines",
            "CreatePipeline",
            create_pipeline(&router, create_body(name, min_units, max_units)).await,
        );
    }

    let listed = expect_ok(
        "a listing of freshly created pipelines",
        "ListPipelines",
        list_pipelines(&router).await,
    );
    let statuses = listed_statuses(&listed);
    for (name, _, _) in names {
        assert_eq!(
            statuses.get(name).map(String::as_str),
            Some("ACTIVE"),
            "case \"a listing of freshly created pipelines\": {name} must be reported ACTIVE, \
             got {statuses:?}"
        );
    }

    // Stop one of them: its own response reports the transient status, and the next
    // listing reports the settled one — for that pipeline only.
    let stopping = expect_ok(
        "a listing after a stop",
        "StopPipeline",
        stop_pipeline(&router, "listed-two").await,
    );
    assert_eq!(
        stopping.status_value(),
        "STOPPING",
        "case \"a listing after a stop\": StopPipeline must answer with STOPPING, got {:?}",
        stopping.body
    );
    let listed = expect_ok(
        "a listing after a stop",
        "ListPipelines",
        list_pipelines(&router).await,
    );
    let statuses = listed_statuses(&listed);
    let expected: BTreeMap<String, String> = [
        ("listed-one", "ACTIVE"),
        ("listed-two", "STOPPED"),
        ("l-3", "ACTIVE"),
    ]
    .into_iter()
    .map(|(name, status)| (name.to_string(), status.to_string()))
    .collect();
    assert_eq!(
        statuses, expected,
        "case \"a listing after a stop\": expected {expected:?}, got {statuses:?}"
    );
}
