// SPDX-License-Identifier: CC0-1.0

//! LOG PUBLICATION, the CloudWatch Logs destination: what an application's
//! `monitoringConfiguration.cloudWatchLoggingConfiguration` makes a finished job run
//! deliver, end to end over HTTP across both services.
//!
//! RULE: a job run submitted under a monitoring configuration that ENABLES CloudWatch
//! logging delivers its driver log into the group that configuration names, in the stream
//! the run is keyed by, once the run reaches a state it never leaves — whatever terminal
//! state it reached — and a run whose configuration enables none, or asks only for an
//! artifact this service has none of, delivers nothing.
//!
//! The rows differ along what the rule generalises over: WHO configures the destination (the
//! application, the run's own `configurationOverrides`, both at once, neither), whether
//! logging is ENABLED at all, the GROUP (the one the configuration names, and the default for
//! one that names none), the STREAM PREFIX (named and absent), WHICH artifacts the `logTypes`
//! ask for (none at all, the driver's standard error, only its standard output, another
//! worker type), the ENGINE whose driver the artifact belongs to (Spark and Hive, which name
//! different worker types), and the TERMINAL STATE the run reached (the one the clock takes
//! it to, and the one a cancel takes it to).
//!
//! The log group name and the log types come from
//! `smithy_query(query="shape",shape="CloudWatchLoggingConfiguration")`:
//! `logTypes`' "Array Members Valid Values" are `STDOUT`, `STDERR`, `HIVE_LOG`, `TEZ_AM` and
//! `SYSTEM_LOGS` and its key values `SPARK_DRIVER`, `SPARK_EXECUTOR`, `HIVE_DRIVER` and
//! `TEZ_TASK`, and "If you don't specify any log types, driver STDOUT and STDERR logs will be
//! published to CloudWatch Logs by default". The stream keying is written out by hand here,
//! so a changed layout in `src/` fails this suite.
//!
//! CloudWatch Logs and Amazon S3 are mounted on the SAME engine this service runs on, exactly
//! as the assembled emulator mounts them, and every delivered record is read back through
//! CloudWatch Logs' own `GetLogEvents`: what is asserted is what that service persisted and
//! not a copy of it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService;

use super::application_lifecycle::{
    cancel_job_run, create_application, get_job_run, start_job_run, Answer, EXECUTION_ROLE,
};

/// The Region every request of this suite is signed for — the one the shared helpers in
/// [`super::application_lifecycle`] sign for, so the log group this service delivers to is
/// addressed in the same Region the test reads it in.
const REGION: &str = "eu-west-1";

/// The bucket the one row that configures BOTH destinations publishes its object into.
const BUCKET: &str = "emr-serverless-both-destinations";

/// The states a `JobRunState` names that a job run never leaves. Source:
/// `smithy_query(query="shape",shape="JobRunState")` — written out here rather than imported,
/// so a wrong set in `src/` cannot make this suite pass.
const TERMINAL_STATES: &[&str] = &["SUCCESS", "FAILED", "CANCELLED"];

/// The group a configuration that names none delivers to, as EMR Serverless' logging
/// documentation publishes it.
const DEFAULT_LOG_GROUP: &str = "/aws/emr-serverless";

/// The worker type a Spark application's driver runs as.
const SPARK_DRIVER: &str = "SPARK_DRIVER";

/// The same, for a Hive application's driver.
const HIVE_DRIVER: &str = "HIVE_DRIVER";

/// The log type of the artifact a finished run has: its driver's standard error.
const STDERR_LOG_TYPE: &str = "STDERR";

/// How long a row waits for a condition it polls for: the whole compressed run window, the
/// transitions around it and generous slack for a loaded machine.
const BUDGET: Duration = Duration::from_secs(60);

/// How often it looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One emulator carrying this service, CloudWatch Logs and Amazon S3 over ONE shared engine,
/// exactly as the production router builds them.
fn emulator_with_log_destinations() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(EmulatedService::new(engine));
    router
}

/// One CloudWatch Logs call (awsJson1_1), answered with the HTTP status and the JSON body.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One path-style Amazon S3 call, answered as the status and the body bytes.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: Bytes) -> (u16, Bytes) {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !body.is_empty() {
        builder = builder.header("content-type", "application/xml");
    }
    let request = builder.body(body).expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.into_body())
}

/// Create [`BUCKET`], so the Amazon S3 destination of the row that names one resolves.
async fn given_bucket(router: &MultiServiceRouter) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{BUCKET}"),
        Bytes::from(configuration),
    )
    .await;
    assert!(
        status == 200 || status == 409,
        "creating s3://{BUCKET} must succeed or report it is already there, got {status} with \
         {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// The keys of every object under `prefix` in [`BUCKET`], from `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, listing) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        Bytes::new(),
    )
    .await;
    let listing = String::from_utf8_lossy(&listing).to_string();
    assert_eq!(
        status, 200,
        "listing s3://{BUCKET}/{prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// `GetLogEvents` on one stream: the status it answered and the messages it reported, oldest
/// first.
///
/// The status is handed back rather than swallowed, so a case asserting that NOTHING was
/// delivered cannot pass on a call that merely failed.
async fn delivered_messages(
    router: &MultiServiceRouter,
    log_group: &str,
    log_stream: &str,
) -> (u16, Vec<String>) {
    let (status, body) = logs(
        router,
        "GetLogEvents",
        &json!({
            "logGroupName": log_group,
            "logStreamName": log_stream,
            "startFromHead": true
        }),
    )
    .await;
    let messages = body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (status, messages)
}

/// Poll `GetJobRun` until the run reports a state it never leaves, and answer that read.
///
/// The wait is on the CONDITION, not on a span: it looks again every [`POLL_INTERVAL`] until
/// the run is terminal or the generous [`BUDGET`] passes.
async fn poll_until_terminal(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    job_run_id: &str,
) -> Answer {
    let deadline = Instant::now() + BUDGET;
    loop {
        let answer = get_job_run(router, application_id, job_run_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetJobRun expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let state = answer
            .at("/jobRun/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if TERMINAL_STATES.contains(&state.as_str()) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job run must reach a terminal state, last saw {state:?}"
        );
        // The look-again yields to the runtime rather than blocking its thread: this suite's
        // own calls are the only work on it, and a blocked thread cannot serve them.
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetLogEvents` until the stream holds at least one record, and answer what it holds
/// then (or what the last read saw once the budget passes).
async fn poll_until_delivered(
    router: &MultiServiceRouter,
    log_group: &str,
    log_stream: &str,
) -> (u16, Vec<String>) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, messages) = delivered_messages(router, log_group, log_stream).await;
        if !messages.is_empty() || Instant::now() >= deadline {
            return (status, messages);
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `monitoringConfiguration` whose CloudWatch block is `configuration`.
fn cloud_watch(configuration: Value) -> Value {
    json!({ "cloudWatchLoggingConfiguration": configuration })
}

/// Where one row's artifact belongs: the stream the run's configuration points at.
///
/// Every row names one, including a row that must deliver NOTHING — that is where the suite
/// asserts nothing arrived, so a row pinning the `logTypes` filtering reads the very stream a
/// wrong filter would have written to rather than one nothing could ever create.
struct Destination {
    /// The group it is delivered to.
    log_group: &'static str,
    /// The `logStreamNamePrefix` the stream carries, with its separator — `""` for a
    /// configuration that names none.
    stream_prefix: &'static str,
    /// The worker-type segment of the stream.
    worker_type: &'static str,
}

/// One row: what the application and the run configure, and what that has to deliver.
struct Row {
    case: &'static str,
    /// `CreateApplicationRequest$type`.
    engine_type: &'static str,
    /// The application's `monitoringConfiguration`, or `None` for one that names none.
    application_monitoring: Option<Value>,
    /// The run's `configurationOverrides`, or `None` for a run that overrides nothing.
    overrides: Option<Value>,
    /// The `jobDriver` the run is submitted with.
    driver: Value,
    /// Whether the row cancels the run instead of letting the clock finish it.
    cancels: bool,
    /// The stream the row's configuration points at.
    destination: Destination,
    /// Whether the finished run has to deliver its driver log there.
    delivers: bool,
}

/// The `sparkSubmit` driver every Spark row submits.
fn spark_driver() -> Value {
    json!({ "sparkSubmit": { "entryPoint": "s3://scripts/ddl.py" } })
}

/// The `hive` driver every Hive row submits.
fn hive_driver() -> Value {
    json!({ "hive": { "query": "SELECT 1" } })
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the application enables logging and names no group",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({ "enabled": true }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "a named group",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logGroupName": "/emr-serverless/named"
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: "/emr-serverless/named",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "a named stream prefix",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logStreamNamePrefix": "runs/"
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "runs/",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "the driver's standard error asked for by name",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logTypes": { SPARK_DRIVER: [STDERR_LOG_TYPE] }
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "a Hive application, whose driver is a different worker type",
            engine_type: "HIVE",
            application_monitoring: Some(cloud_watch(json!({ "enabled": true }))),
            overrides: None,
            driver: hive_driver(),
            cancels: false,
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "",
                worker_type: HIVE_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "the run's own overrides, on an application configuring none",
            engine_type: "SPARK",
            application_monitoring: None,
            overrides: Some(json!({
                "monitoringConfiguration": cloud_watch(json!({
                    "enabled": true,
                    "logGroupName": "/emr-serverless/by-run"
                }))
            })),
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: "/emr-serverless/by-run",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "the run's overrides winning over the application's group",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logGroupName": "/emr-serverless/by-app"
            }))),
            overrides: Some(json!({
                "monitoringConfiguration": cloud_watch(json!({
                    "enabled": true,
                    "logGroupName": "/emr-serverless/overridden"
                }))
            })),
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: "/emr-serverless/overridden",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "a run the caller cancels, which still delivers its log",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logGroupName": "/emr-serverless/cancelled"
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: true,
            destination: Destination {
                log_group: "/emr-serverless/cancelled",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "both destinations configured at once",
            engine_type: "SPARK",
            application_monitoring: Some(json!({
                "s3MonitoringConfiguration": { "logUri": format!("s3://{BUCKET}/logs/") },
                "cloudWatchLoggingConfiguration": {
                    "enabled": true,
                    "logGroupName": "/emr-serverless/both"
                }
            })),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: "/emr-serverless/both",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: true,
        },
        Row {
            case: "logging not enabled, which delivers nothing",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({ "enabled": false }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            // A configuration carrying `enabled` off names no group of its own, so the
            // stream nothing may reach is the one in the default group.
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: false,
        },
        Row {
            case: "no monitoring configuration anywhere, which delivers nothing",
            engine_type: "SPARK",
            application_monitoring: None,
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            destination: Destination {
                log_group: DEFAULT_LOG_GROUP,
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: false,
        },
        Row {
            case: "only the driver's standard output asked for, which this run has none of",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logGroupName": "/emr-serverless/stdout-only",
                "logTypes": { SPARK_DRIVER: ["STDOUT"] }
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            // The group this row's configuration NAMES: the stream the driver's standard
            // error would have reached had the `logTypes` filter not held.
            destination: Destination {
                log_group: "/emr-serverless/stdout-only",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: false,
        },
        Row {
            case: "another worker type asked for, which nothing ran",
            engine_type: "SPARK",
            application_monitoring: Some(cloud_watch(json!({
                "enabled": true,
                "logGroupName": "/emr-serverless/executor-only",
                "logTypes": { "SPARK_EXECUTOR": [STDERR_LOG_TYPE] }
            }))),
            overrides: None,
            driver: spark_driver(),
            cancels: false,
            // See the row above: the group this configuration names, where the driver's
            // artifact must not appear.
            destination: Destination {
                log_group: "/emr-serverless/executor-only",
                stream_prefix: "",
                worker_type: SPARK_DRIVER,
            },
            delivers: false,
        },
    ]
}

/// What one row's run ended up as: the ids and the terminal read.
struct Finished {
    application_id: String,
    job_run_id: String,
    terminal: Answer,
}

/// Run one row: create the application, submit the run and take it to a terminal state.
async fn run_row(router: &MultiServiceRouter, row: &Row, index: usize) -> Finished {
    // `ClientToken` is bounded at 64 characters, so the row is named by its INDEX rather
    // than by its label; the label is what every assertion reports.
    let mut create = json!({
        "clientToken": format!("cw-app-{index}"),
        "releaseLabel": "emr-7.1.0",
        "type": row.engine_type,
    });
    if let Some(monitoring) = row.application_monitoring.clone() {
        create["monitoringConfiguration"] = monitoring;
    }
    let created = create_application(router, create).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateApplication expected 200, got {} with {:?}",
        row.case, created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut submit = json!({
        "clientToken": format!("cw-run-{index}"),
        "executionRoleArn": EXECUTION_ROLE,
        "jobDriver": row.driver.clone(),
    });
    if let Some(overrides) = row.overrides.clone() {
        submit["configurationOverrides"] = overrides;
    }
    let submitted = start_job_run(router, &application_id, submit).await;
    assert_eq!(
        submitted.status, 200,
        "case {:?}: StartJobRun expected 200, got {} with {:?}",
        row.case, submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    if row.cancels {
        let cancelled = cancel_job_run(router, &application_id, &job_run_id, "").await;
        assert_eq!(
            cancelled.status, 200,
            "case {:?}: CancelJobRun expected 200, got {} with {:?}",
            row.case, cancelled.status, cancelled.body
        );
    }

    let terminal = poll_until_terminal(router, row.case, &application_id, &job_run_id).await;
    Finished {
        application_id,
        job_run_id,
        terminal,
    }
}

/// RULE: a monitoring configuration that enables CloudWatch logging makes the finished run's
/// driver log EXIST in the group it names, in the stream the run is keyed by; a run whose
/// configuration enables none, or asks only for an artifact this service has none of, delivers
/// nothing.
#[tokio::test]
async fn a_finished_job_run_delivers_its_driver_log_to_the_configured_log_group() {
    let router = emulator_with_log_destinations();
    given_bucket(&router).await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let Finished {
            application_id,
            job_run_id,
            terminal,
        } = run_row(&router, &row, index).await;

        let destination = &row.destination;
        let stream = format!(
            "{}applications/{application_id}/jobs/{job_run_id}/{}/{STDERR_LOG_TYPE}",
            destination.stream_prefix, destination.worker_type
        );

        if !row.delivers {
            // A run that delivers nothing leaves the stream its own configuration points at
            // empty: the read either reports no such resource, or reports an empty one.
            let (status, messages) =
                delivered_messages(&router, destination.log_group, &stream).await;
            assert!(
                status != 200 || messages.is_empty(),
                "case {case:?}: nothing may be delivered to {}/{stream}, got {status} with \
                 {messages:?}",
                destination.log_group
            );
            continue;
        }

        let (status, messages) =
            poll_until_delivered(&router, destination.log_group, &stream).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLogEvents on {}/{stream} expected 200, got {status} with \
             {messages:?}",
            destination.log_group
        );
        assert!(
            !messages.is_empty(),
            "case {case:?}: the finished run must deliver its driver log to {}/{stream}, got \
             no record",
            destination.log_group
        );

        // The records ARE the run's own log: every line the driver log reports is one event,
        // and together they name the run, the identity it runs as and the terminal state it
        // reached.
        let delivered = messages.join("\n");
        let state = terminal
            .at("/jobRun/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let state_details = terminal
            .at("/jobRun/stateDetails")
            .as_str()
            .unwrap_or_default()
            .to_string();
        for needle in [
            application_id.as_str(),
            job_run_id.as_str(),
            EXECUTION_ROLE,
            state.as_str(),
            state_details.as_str(),
        ] {
            assert!(
                delivered.contains(needle),
                "case {case:?}: the delivered log must report {needle:?}, got {delivered:?}"
            );
        }
        for message in &messages {
            assert!(
                !message.trim().is_empty(),
                "case {case:?}: every delivered record must carry a line, got {messages:?}"
            );
        }

        // The row that configures BOTH destinations is delivered to both: the object the
        // Amazon S3 destination is owed is there beside the stream asserted above.
        if row.application_monitoring.as_ref().is_some_and(|monitoring| {
            monitoring.get("s3MonitoringConfiguration").is_some()
        }) {
            let prefix = format!("logs/applications/{application_id}/jobs/{job_run_id}/");
            let keys = keys_under(&router, &prefix).await;
            assert_eq!(
                keys,
                vec![format!("{prefix}{}/stderr.gz", destination.worker_type)],
                "case {case:?}: the same run must publish its object to the Amazon S3 \
                 destination too, got {keys:?}"
            );
        }
    }
}
