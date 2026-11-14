// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CreateJob`: a job TARGETS devices, and targeting a
//! device is what creates that device's job execution.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /jobs/{jobId}` signed for the `iot` signing name — and then reads
//! the shared job-execution store the IoT Jobs data plane serves, which is where the
//! executions a job creates live (`service_common::iot_job_executions`). The data
//! plane's own view of those executions is covered across the two services in
//! `core/router/tests/all/iot_job_execution_reaches_the_device.rs`.
//!
//! The rule under test: `CreateJob` registers the job under the `jobId` the request
//! names, mints its `jobArn`, and creates ONE `QUEUED` execution per targeted thing —
//! carrying the job's document and its `timeoutConfig` — while a request it refuses
//! (a target that names no device, a target ARN of another resource type, no document
//! at all, or a `jobId` already in use) writes nothing.
//!
//! Each table spans the class the rule generalises over — how many targets a job
//! carries and in what form, the form and size of the document, which optional
//! members the request carries, and the region it is signed for — rather than one
//! recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::MAX_JOB_TARGETS;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{JobExecutionStore, MAX_THING_NAME_LEN, STATUS_QUEUED};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }
}

/// One stored job execution, reduced to what a reader of the store sees.
#[derive(Debug, PartialEq, Eq)]
struct StoredExecution {
    job_id: String,
    status: String,
    version_number: i64,
    execution_number: i64,
    document: Option<String>,
    in_progress_timeout_minutes: Option<i64>,
    started: bool,
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it.
/// The engine handle is kept so a case can read the SHARED job-execution store the
/// control plane writes into.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(&json!({})), REGION)
            .await
    }

    async fn create_job(&self, job_id: &str, request: &Value) -> Reply {
        self.create_job_in(job_id, request, REGION).await
    }

    async fn create_job_in(&self, job_id: &str, request: &Value, region: &str) -> Reply {
        self.call("PUT", &format!("/jobs/{job_id}"), Some(request), region)
            .await
    }

    async fn delete_job(&self, job_id: &str, query: &str) -> Reply {
        self.call("DELETE", &format!("/jobs/{job_id}{query}"), None, REGION)
            .await
    }

    /// What the SHARED store holds for `thing_name`, in enqueue order.
    fn executions_for(&self, thing_name: &str) -> Vec<StoredExecution> {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .executions_for(thing_name)
            .iter()
            .map(|record| StoredExecution {
                job_id: record.job_id.clone(),
                status: record.status.clone(),
                version_number: record.version_number,
                execution_number: record.execution_number,
                document: record.job_document.clone(),
                in_progress_timeout_minutes: record.in_progress_timeout_minutes,
                started: record.started_at.is_some(),
            })
            .collect()
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    job_id: &'static str,
    /// The things the job targets, each as a `thing/…` ARN; a name repeated here is
    /// ONE device targeted twice.
    targets: &'static [&'static str],
    /// The `document` member, when the row carries one.
    document: Option<String>,
    /// The `description` member, when the row carries one.
    description: Option<&'static str>,
    /// `timeoutConfig.inProgressTimeoutInMinutes`, when the row carries one.
    in_progress_timeout_minutes: Option<i64>,
    /// `targetSelection`, when the row carries one.
    target_selection: Option<&'static str>,
    /// The region the row is signed for; its `jobArn` must be scoped to it.
    region: &'static str,
}

/// RULE: a job's targets ARE its executions — every distinct thing a `CreateJob`
/// targets holds exactly one `QUEUED` execution of that job afterwards, at version 1
/// and execution number 1, carrying the job's document and in-progress timeout.
#[tokio::test]
async fn every_targeted_thing_gets_one_queued_execution_of_the_job() {
    let emulator = Emulator::new();

    let long_document = format!(r#"{{"operation":"install","payload":"{}"}}"#, "x".repeat(4_000));
    let cases: Vec<CreateCase> = vec![
        CreateCase {
            case: "one target, minimal request",
            job_id: "job-minimal",
            targets: &["laser-a"],
            document: Some(r#"{"operation":"reboot"}"#.to_string()),
            description: None,
            in_progress_timeout_minutes: None,
            target_selection: None,
            region: REGION,
        },
        CreateCase {
            case: "two targets, every optional member",
            job_id: "job-two-targets",
            targets: &["laser-b", "laser-c"],
            document: Some(r#"{"operation":"install","version":"2"}"#.to_string()),
            description: Some("install version 2"),
            in_progress_timeout_minutes: Some(30),
            target_selection: Some("SNAPSHOT"),
            region: REGION,
        },
        CreateCase {
            case: "one device targeted twice is one execution",
            job_id: "job-duplicate-target",
            targets: &["laser-d", "laser-d"],
            document: Some(r#"{"operation":"reboot"}"#.to_string()),
            description: None,
            in_progress_timeout_minutes: Some(1),
            target_selection: Some("CONTINUOUS"),
            region: REGION,
        },
        CreateCase {
            case: "an empty document is a document",
            job_id: "job-empty-document",
            targets: &["laser-e"],
            document: Some(String::new()),
            description: None,
            in_progress_timeout_minutes: None,
            target_selection: None,
            region: REGION,
        },
        CreateCase {
            case: "a document near the modelled maximum",
            job_id: "job-long-document",
            targets: &["laser-f"],
            document: Some(long_document.clone()),
            description: None,
            in_progress_timeout_minutes: None,
            target_selection: None,
            region: REGION,
        },
        CreateCase {
            case: "three targets, signed for another region",
            job_id: "job-another-region",
            targets: &["laser-g", "laser-h", "laser-i"],
            document: Some(r#"{"operation":"rotate-certificate"}"#.to_string()),
            description: None,
            in_progress_timeout_minutes: None,
            target_selection: None,
            region: "eu-west-1",
        },
    ];

    for case in &cases {
        for thing_name in case.targets {
            let created = emulator.create_thing(thing_name).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the target device must be registrable, got {} {:?}",
                case.case, created.status, created.body
            );
        }

        let mut request = json!({
            "targets": case.targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
        });
        if let Some(document) = &case.document {
            request["document"] = json!(document);
        }
        if let Some(description) = case.description {
            request["description"] = json!(description);
        }
        if let Some(minutes) = case.in_progress_timeout_minutes {
            request["timeoutConfig"] = json!({"inProgressTimeoutInMinutes": minutes});
        }
        if let Some(selection) = case.target_selection {
            request["targetSelection"] = json!(selection);
        }

        let reply = emulator
            .create_job_in(case.job_id, &request, case.region)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );
        assert_eq!(
            reply.text("jobId").as_deref(),
            Some(case.job_id),
            "case {:?}: expected the jobId it was created under, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("jobArn"),
            Some(format!(
                "arn:aws:iot:{}:{ACCOUNT_ID}:job/{}",
                case.region, case.job_id
            )),
            "case {:?}: expected the job ARN scoped to the signed region, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("description").as_deref(),
            case.description,
            "case {:?}: expected the description it was created with, got {:?}",
            case.case,
            reply.body
        );

        // One execution per DISTINCT target, each QUEUED and never started.
        let mut distinct: Vec<&str> = Vec::new();
        for thing_name in case.targets {
            if !distinct.contains(thing_name) {
                distinct.push(thing_name);
            }
        }
        for thing_name in &distinct {
            let held = emulator.executions_for(thing_name);
            assert_eq!(
                held,
                vec![StoredExecution {
                    job_id: case.job_id.to_string(),
                    status: STATUS_QUEUED.to_string(),
                    version_number: 1,
                    execution_number: 1,
                    document: case.document.clone(),
                    in_progress_timeout_minutes: case.in_progress_timeout_minutes,
                    started: false,
                }],
                "case {:?}: expected exactly one queued execution for {thing_name}, got {held:?}",
                case.case
            );
        }
    }

    // A registered device no job targeted holds nothing: an execution exists because
    // a job named the device, never because the device exists.
    let created = emulator.create_thing("laser-untargeted").await;
    assert_eq!(
        created.status, 200,
        "an untargeted device must still be registrable, got {} {:?}",
        created.status, created.body
    );
    let held = emulator.executions_for("laser-untargeted");
    assert!(
        held.is_empty(),
        "case untargeted device: expected no executions, got {held:?}"
    );
}

/// One row of the refusal table: a `CreateJob` request this operation must refuse,
/// and the refusal a client sees.
struct RefusalCase {
    case: &'static str,
    /// The `targets` the request carries.
    targets: Vec<String>,
    /// The `document` member, when the row carries one.
    document: Option<Value>,
    status: u16,
    code: &'static str,
    /// A fragment the refusal's message must carry, so the message is shown to be
    /// built from the request rather than being one fixed template.
    message_carries: &'static str,
}

/// RULE: a `CreateJob` this operation refuses writes NOTHING — no job and no
/// execution for the targets that did resolve. A target that cannot be RESOLVED to
/// devices is refused; one that merely names a device the registry does not hold is
/// not (an execution is keyed by the thing name).
#[tokio::test]
async fn a_refused_create_job_writes_neither_the_job_nor_an_execution() {
    let emulator = Emulator::new();
    for thing_name in ["laser-known", "laser-second"] {
        let created = emulator.create_thing(thing_name).await;
        assert_eq!(
            created.status, 200,
            "the probe device must be registrable, got {} {:?}",
            created.status, created.body
        );
    }

    let document = json!(r#"{"operation":"reboot"}"#);
    let refused: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a thing group the registry does not hold",
            targets: vec![format!(
                "arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/fleet-1"
            )],
            document: Some(document.clone()),
            status: 404,
            code: "ResourceNotFoundException",
            message_carries: "fleet-1",
        },
        RefusalCase {
            case: "an ARN of another resource type",
            targets: vec![format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:job/other-job")],
            document: Some(document.clone()),
            status: 400,
            code: "InvalidRequestException",
            // The message names the resource type it rejected, which is built from
            // the ARN the request supplied.
            message_carries: "job",
        },
        RefusalCase {
            case: "an ARN of another service",
            targets: vec![format!("arn:aws:s3:{REGION}:{ACCOUNT_ID}:thing/laser-known")],
            document: Some(document.clone()),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "not an IoT thing or thing group ARN",
        },
        RefusalCase {
            case: "a target that is not an ARN at all",
            targets: vec!["laser-known".to_string()],
            document: Some(document.clone()),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "not an IoT thing or thing group ARN",
        },
        RefusalCase {
            case: "a resolvable target beside an unresolvable one",
            targets: vec![
                thing_arn("laser-known"),
                format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/fleet-2"),
            ],
            document: Some(document.clone()),
            status: 404,
            code: "ResourceNotFoundException",
            message_carries: "fleet-2",
        },
        RefusalCase {
            case: "neither document nor documentSource",
            targets: vec![thing_arn("laser-known")],
            document: None,
            status: 400,
            code: "InvalidRequestException",
            message_carries: "document",
        },
        RefusalCase {
            case: "a thing ARN whose name is not a ThingName",
            targets: vec![thing_arn("laser/known")],
            document: Some(document.clone()),
            status: 400,
            code: "InvalidRequestException",
            // The refusal names the unaddressable NAME and its constraint, not the
            // ARN format: the ARN itself is a thing ARN.
            message_carries: "is not a thing name a job may target",
        },
        RefusalCase {
            case: "a thing ARN whose name is one character past the maximum",
            targets: vec![thing_arn(&"n".repeat(MAX_THING_NAME_LEN + 1))],
            document: Some(document.clone()),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "is not a thing name a job may target",
        },
        RefusalCase {
            case: "one target past the per-job maximum",
            targets: (0..=MAX_JOB_TARGETS)
                .map(|index| thing_arn(&format!("laser-fanout-{index}")))
                .collect(),
            document: Some(document.clone()),
            status: 410,
            code: "LimitExceededException",
            message_carries: "targets",
        },
    ];

    for (index, row) in refused.iter().enumerate() {
        let RefusalCase {
            case,
            targets,
            document,
            status,
            code,
            message_carries,
        } = row;
        let job_id = format!("job-refused-{index}");
        let mut request = json!({"targets": targets});
        if let Some(document) = document {
            request["document"] = document.clone();
        }
        let reply = emulator.create_job(&job_id, &request).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(*code),
            "case {case:?}: expected {code}, got {:?} {:?}",
            reply.error_code,
            reply.body
        );
        assert!(
            reply.message().contains(message_carries),
            "case {case:?}: expected a message carrying {message_carries:?}, got {:?}",
            reply.body
        );

        // Nothing was written: neither an execution for a target that did resolve,
        // nor the job itself — so the id is still free.
        for thing_name in ["laser-known", "laser-second"] {
            let held = emulator.executions_for(thing_name);
            assert!(
                held.is_empty(),
                "case {case:?}: a refused create must leave {thing_name} with no execution, \
                 got {held:?}"
            );
        }
        let retried = emulator
            .create_job(
                &job_id,
                &json!({"targets": [thing_arn("laser-known")], "document": document.clone().unwrap_or(json!("{}"))}),
            )
            .await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: the refused job id must still be free, got {} {:?}",
            retried.status, retried.body
        );
        // Clear the execution that retry created, so the next row starts clean.
        emulator
            .engine
            .state::<JobExecutionStore>()
            .lock()
            .remove_execution("laser-known", &job_id, 1);
    }
}

/// RULE: `jobId` "must be unique for your account" — a second create under a live
/// job's id is refused 409 `ResourceAlreadyExistsException` and creates no second
/// execution.
#[tokio::test]
async fn a_second_job_under_a_live_id_is_a_conflict() {
    let emulator = Emulator::new();
    emulator.create_thing("laser-one").await;
    emulator.create_thing("laser-two").await;

    let first = emulator
        .create_job(
            "job-unique",
            &json!({"targets": [thing_arn("laser-one")], "document": "{}"}),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {:?}",
        first.status, first.body
    );

    // (case, targets, document) — a conflict whatever the second request asks for.
    for (case, targets, document) in [
        ("the same request again", vec!["laser-one"], "{}"),
        ("another target", vec!["laser-two"], "{}"),
        ("another document", vec!["laser-one"], r#"{"operation":"x"}"#),
    ] {
        let reply = emulator
            .create_job(
                "job-unique",
                &json!({
                    "targets": targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
                    "document": document,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException, got {:?}",
            reply.body
        );
        assert_eq!(
            emulator.executions_for("laser-two").len(),
            0,
            "case {case:?}: the refused create must target nothing new, got {:?}",
            emulator.executions_for("laser-two")
        );
        assert_eq!(
            emulator.executions_for("laser-one").len(),
            1,
            "case {case:?}: the first execution must be the only one, got {:?}",
            emulator.executions_for("laser-one")
        );
    }
}

/// RULE: a thing target names a DEVICE BY NAME, so a job may target a thing the
/// registry does not hold: the execution is created under that name and the data
/// plane answers that name's queue reads from it.
#[tokio::test]
async fn a_job_may_target_a_device_the_registry_does_not_hold() {
    let emulator = Emulator::new();
    let registered = "laser-registered";
    let created = emulator.create_thing(registered).await;
    assert_eq!(
        created.status, 200,
        "the registered device must be registrable, got {} {:?}",
        created.status, created.body
    );

    // (case, thing name, registered)
    for (case, thing_name, is_registered) in [
        ("a registered device", registered, true),
        ("a device that was never registered", "laser-unregistered", false),
        ("a name carrying the colon its pattern allows", "laser:probe-2", false),
    ] {
        let job_id = format!("job-permissive-{}", thing_name.replace(':', "-"));
        let reply = emulator
            .create_job(
                &job_id,
                &json!({"targets": [thing_arn(thing_name)], "document": "{}"}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 whether or not the device is registered \
             (registered={is_registered}), got {} {:?}",
            reply.status, reply.body
        );
        let held = emulator.executions_for(thing_name);
        assert_eq!(
            held.iter().map(|e| e.job_id.as_str()).collect::<Vec<&str>>(),
            vec![job_id.as_str()],
            "case {case:?}: expected the job queued for {thing_name}, got {held:?}"
        );
    }
}

/// RULE: deleting a job deletes "its related job executions" — after the delete the
/// device has nothing queued and no record of the job remains, so the id is free
/// again. A job with an unfinished execution is refused 409 without `force`.
#[tokio::test]
async fn deleting_a_job_deletes_its_executions() {
    let emulator = Emulator::new();
    for thing_name in ["laser-del-1", "laser-del-2"] {
        emulator.create_thing(thing_name).await;
    }

    // (case, the query string the delete carries, expected status)
    //
    // Every row creates the job afresh, targets both devices, and then deletes it;
    // the rows differ in whether the executions are still pending and in whether the
    // delete is forced.
    let rows: [(&str, bool, &str, u16); 4] = [
        ("queued executions, no force", true, "", 409),
        ("queued executions, forced", true, "?force=true", 200),
        ("no executions left, no force", false, "", 200),
        ("no executions left, forced", false, "?force=true", 200),
    ];

    for (case, keep_pending, query, expected) in rows {
        let job_id = format!("job-delete-{}", query.len() + usize::from(keep_pending));
        let created = emulator
            .create_job(
                &job_id,
                &json!({
                    "targets": [thing_arn("laser-del-1"), thing_arn("laser-del-2")],
                    "document": r#"{"operation":"reboot"}"#,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the job must be creatable, got {} {:?}",
            created.status, created.body
        );
        if !keep_pending {
            // Finish both executions, so nothing of the job is still running.
            let store = emulator.engine.state::<JobExecutionStore>();
            let mut store = store.lock();
            for thing_name in ["laser-del-1", "laser-del-2"] {
                if let Some(index) = store.execution_index(thing_name, &job_id, None) {
                    if let Some(record) = store.record_mut(thing_name, index) {
                        record.status = "SUCCEEDED".to_string();
                    }
                }
            }
        }

        let reply = emulator.delete_job(&job_id, query).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected {expected}, got {} {:?}",
            reply.status, reply.body
        );

        if expected == 409 {
            assert_eq!(
                reply.error_code.as_deref(),
                Some("InvalidStateTransitionException"),
                "case {case:?}: expected InvalidStateTransitionException, got {:?}",
                reply.body
            );
            // The refused delete kept both executions.
            for thing_name in ["laser-del-1", "laser-del-2"] {
                assert_eq!(
                    emulator.executions_for(thing_name).len(),
                    1,
                    "case {case:?}: the refused delete must keep {thing_name}'s execution"
                );
            }
            // Force it, so the next row starts from an empty store.
            let forced = emulator.delete_job(&job_id, "?force=true").await;
            assert_eq!(
                forced.status, 200,
                "case {case:?}: the forced delete must succeed, got {} {:?}",
                forced.status, forced.body
            );
        }

        for thing_name in ["laser-del-1", "laser-del-2"] {
            let held = emulator.executions_for(thing_name);
            assert!(
                held.is_empty(),
                "case {case:?}: the deleted job must leave {thing_name} with no execution, \
                 got {held:?}"
            );
        }
        // The job itself is gone: the id is free, and deleting it again is a 404.
        let again = emulator.delete_job(&job_id, "?force=true").await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting a job twice is not found, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            again.body
        );
    }
}

/// RULE: one `CreateJob` may name at most `MAX_JOB_TARGETS` devices — at the bound it
/// is accepted and every one of them holds its execution, one past it is refused 410
/// `LimitExceededException` (asserted in the refusal table) — and a member this
/// emulator cannot honour is refused rather than answered with a wrong outcome:
/// `schedulingConfig` names a job that queues nothing until its window opens, so it
/// is the modelled 400 and writes nothing.
#[tokio::test]
async fn the_per_job_fan_out_is_bounded_and_an_unschedulable_job_is_refused() {
    let emulator = Emulator::new();

    // At the bound: accepted, and every target holds exactly its own execution.
    let targets: Vec<String> = (0..MAX_JOB_TARGETS)
        .map(|index| thing_arn(&format!("laser-bound-{index}")))
        .collect();
    let reply = emulator
        .create_job(
            "job-at-the-bound",
            &json!({"targets": targets, "document": "{}"}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case at the target bound: expected 200, got {} {:?}",
        reply.status, reply.body
    );
    for index in 0..MAX_JOB_TARGETS {
        let thing_name = format!("laser-bound-{index}");
        let held = emulator.executions_for(&thing_name);
        assert_eq!(
            held.len(),
            1,
            "case at the target bound: expected one execution for {thing_name}, got {held:?}"
        );
    }

    // A member this handler cannot honour: refused, and nothing is written.
    for (case, scheduling) in [
        (
            "an end-behaviour-only schedule",
            json!({"endBehavior": "STOP_ROLLOUT"}),
        ),
        (
            "a future start time",
            json!({"startTime": "2030-01-01T00:00:00Z", "endBehavior": "CANCEL"}),
        ),
    ] {
        let reply = emulator
            .create_job(
                "job-scheduled",
                &json!({
                    "targets": [thing_arn("laser-scheduled")],
                    "document": "{}",
                    "schedulingConfig": scheduling,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {:?}",
            reply.body
        );
        assert!(
            reply.message().contains("schedulingConfig"),
            "case {case:?}: expected the message to name the member it refused, got {:?}",
            reply.body
        );
        let held = emulator.executions_for("laser-scheduled");
        assert!(
            held.is_empty(),
            "case {case:?}: a refused create must queue nothing, got {held:?}"
        );
    }
}

/// RULE: a job created from `documentSource` is accepted, and its executions carry NO
/// document — the S3 object that member names is outside this emulator, so nothing is
/// reported for it rather than an invented document.
#[tokio::test]
async fn a_job_created_from_a_document_source_carries_no_document() {
    let emulator = Emulator::new();
    emulator.create_thing("laser-source").await;

    for (case, source) in [
        (
            "an s3 object url",
            "https://s3.us-east-1.amazonaws.com/example-firmware/device-firmware.1.0",
        ),
        ("an s3 link", "s3://example-firmware/device-firmware.2.0"),
    ] {
        let job_id = format!("job-source-{}", source.len());
        let reply = emulator
            .create_job(
                &job_id,
                &json!({
                    "targets": [thing_arn("laser-source")],
                    "documentSource": source,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        let held = emulator.executions_for("laser-source");
        let created = held
            .iter()
            .find(|execution| execution.job_id == job_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected an execution, got {held:?}"));
        assert_eq!(
            created.document, None,
            "case {case:?}: expected no job document, got {created:?}"
        );
        assert_eq!(
            created.status,
            STATUS_QUEUED.to_string(),
            "case {case:?}: expected a queued execution, got {created:?}"
        );
    }
}
