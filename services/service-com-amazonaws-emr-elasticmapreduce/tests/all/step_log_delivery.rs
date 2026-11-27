// SPDX-License-Identifier: CC0-1.0

//! The log directory a finished step leaves in Amazon S3, end to end over the wire.
//!
//! THE RULE: "LogUri — the location in Amazon S3 to write the log files of the job flow"
//! (`RunJobFlow$LogUri` doc). Naming that location makes the files EXIST there: a step that has
//! reached a terminal `StepState` on a cluster launched with a `LogUri` has a log directory of
//! its own, `<LogUri>/<cluster-id>/steps/<step-id>/`, holding exactly the three gzipped files a
//! step's run produces — the controller's own log, and the two console streams of the work —
//! and each file carries that run's own output: the status it exited with, what it printed, and
//! the driver's traceback where it raised one. A cluster launched with no `LogUri` has nowhere
//! to write them, so nothing is written anywhere.
//!
//! The rows span what the rule generalises over: how the work ENDED (a clean exit, a driver
//! that raises, a driver that exits with a status of its own), which stream it wrote on, and
//! whether a destination was configured at all.
//!
//! Amazon S3 is mounted BESIDE Amazon EMR on one engine, every application is written through
//! S3's own `CreateBucket`/`PutObject`, and every file is read back through `ListObjectsV2` and
//! `GetObject` — so the delivered objects are objects a client's own reads find, and the text
//! asserted on is what came out of the gzip stream the service wrote.

use std::collections::BTreeSet;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    CONTROLLER_LOG_OBJECT, STDERR_LOG_OBJECT, STDOUT_LOG_OBJECT, STEP_LOG_OBJECTS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region both services are addressed in: the one whose `CreateBucket` needs no location
/// constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every application, every output and every log file of this suite lives in.
pub(crate) const BUCKET: &str = "emr-step-logs";

/// The key prefix the cluster's `LogUri` names inside it.
const LOG_PREFIX: &str = "logs";

/// The roles a launch names.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// How long a poll waits for a condition: both provisioning phases, the steps' own spans and
/// their application runs, with room to spare on a loaded host.
pub(crate) const DEADLINE: Duration = Duration::from_secs(90);

/// How long a poll sleeps between looks.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How many further reads a case takes after its step has ENDED: a delivery is attempted by
/// whatever reports the step, so what a read reports is checked THROUGH the window those
/// attempts are made in rather than sampled once.
const READS_PAST_THE_END: usize = 20;

/// The states a step never leaves (`com.amazonaws.emr#StepState`).
pub(crate) const TERMINAL: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// The marker a row's application writes, which no other text of this suite carries: finding it
/// in a file is what shows the file holds THAT run's output.
pub(crate) const MARKER: &str = "sim-to-real-step-log-marker";

/// What a Python driver's traceback opens with, which is what a client greps a failed step's
/// standard error for.
const TRACEBACK_HEADER: &str = "Traceback (most recent call last):";

/// Amazon EMR and Amazon S3 on one engine, the shape the router mounts them in.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One `awsJson1_1` Amazon EMR call that must be served.
pub(crate) async fn emr_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let host = format!("elasticmapreduce.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261104/{REGION}/\
                 elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One path-style Amazon S3 request, answered as `(status, body)`.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261104/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    (parts.status.as_u16(), answered.to_vec())
}

/// Put `body` at `key`, which must be served.
pub(crate) async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &[u8]) {
    let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: PutObject {key} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// The keys of every object under `prefix`, from `ListObjectsV2`.
pub(crate) async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, listing) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
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

/// The TEXT of the gzipped object at `key`, which must be a gzip stream a reader can
/// decompress.
async fn gunzipped(router: &MultiServiceRouter, case: &str, key: &str) -> String {
    let (status, body) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: GetObject {key} must succeed, got {status}"
    );
    assert_eq!(
        body.get(..2),
        Some([0x1f, 0x8b].as_slice()),
        "case {case:?}: {key} must be a gzip stream, got {:?}",
        body.get(..2)
    );
    let mut text = String::new();
    ::flate2::read::GzDecoder::new(body.as_slice())
        .read_to_string(&mut text)
        .unwrap_or_else(|error| panic!("case {case:?}: {key} is no gzip stream: {error}"));
    text
}

/// One row: the application a step submits, and what its log files have to say.
struct Case {
    label: &'static str,
    source: String,
    /// The state the step must reach.
    state: &'static str,
    /// Fragments the step's standard-error file must carry.
    stderr_holds: Vec<String>,
    /// Fragments the step's standard-output file must carry.
    stdout_holds: Vec<String>,
    /// The exit status the controller log must report.
    exit_status: i32,
    /// Whether any of the step's files may carry a Python traceback.
    traceback: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a driver that prints and exits cleanly",
            source: format!(
                "import sys\n\
                 from pyspark import SparkContext\n\
                 total = SparkContext.getOrCreate().parallelize([1, 2, 3]).sum()\n\
                 print(\"{MARKER}-ok %d\" % total)\n"
            ),
            state: "COMPLETED",
            stderr_holds: Vec::new(),
            stdout_holds: vec![format!("{MARKER}-ok 6")],
            exit_status: 0,
            traceback: false,
        },
        Case {
            label: "a driver that raises after printing",
            source: format!(
                "import sys\n\
                 from pyspark import SparkContext\n\
                 SparkContext.getOrCreate().parallelize([1]).collect()\n\
                 print(\"{MARKER}-before\")\n\
                 raise RuntimeError(\"{MARKER}-boom\")\n"
            ),
            state: "FAILED",
            stderr_holds: vec![
                TRACEBACK_HEADER.to_string(),
                format!("RuntimeError: {MARKER}-boom"),
            ],
            stdout_holds: vec![format!("{MARKER}-before")],
            exit_status: 1,
            traceback: true,
        },
        Case {
            label: "a driver that exits with a status of its own",
            source: format!(
                "import sys\n\
                 sys.stderr.write(\"{MARKER}-giving-up\\n\")\n\
                 sys.exit(3)\n"
            ),
            state: "FAILED",
            stderr_holds: vec![format!("{MARKER}-giving-up")],
            stdout_holds: Vec::new(),
            exit_status: 3,
            traceback: false,
        },
    ]
}

/// The key one row's application is written under.
fn application_key(ordinal: usize) -> String {
    format!("applications/step-{ordinal}.py")
}

/// The `StepConfig` one row submits.
fn step_config(case: &Case, ordinal: usize) -> Value {
    json!({
        "Name": case.label,
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": {
            "Jar": "command-runner.jar",
            "Args": [
                "spark-submit",
                "--deploy-mode",
                "client",
                format!("s3://{BUCKET}/{}", application_key(ordinal)),
            ],
        },
    })
}

/// A cluster that stays up, launched with `steps` and the `LogUri` the row asks for.
pub(crate) async fn launch(
    router: &MultiServiceRouter,
    name: &str,
    steps: Value,
    log_uri: Option<&str>,
) -> String {
    let mut request = json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Applications": [{ "Name": "Spark" }],
        "Steps": steps,
        "StepConcurrencyLevel": 1,
        "Instances": {
            "InstanceCount": 2,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    });
    if let Some(log_uri) = log_uri {
        request["LogUri"] = json!(log_uri);
    }
    let launched = emr_ok(router, "RunJobFlow", request).await;
    launched["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// Poll `ListSteps` until every step of `cluster_id` is terminal, answering them in submission
/// order. The wait is on the CONDITION: it looks again until the steps have settled or the
/// generous [`DEADLINE`] passes.
async fn drained(case: &str, router: &MultiServiceRouter, cluster_id: &str) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let listed = emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        let mut steps: Vec<Value> = listed["Steps"].as_array().cloned().unwrap_or_default();
        steps.reverse();
        let states: Vec<String> = steps
            .iter()
            .map(|step| step["Status"]["State"].as_str().unwrap_or_default().to_string())
            .collect();
        if !states.is_empty() && states.iter().all(|state| TERMINAL.contains(&state.as_str())) {
            return steps;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected every step to reach a terminal state, got {states:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `ListObjectsV2` until `prefix` holds every file a step's log directory holds, answering
/// the keys it holds then.
async fn delivered(case: &str, router: &MultiServiceRouter, cluster_id: &str, prefix: &str) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let keys = keys_under(router, prefix).await;
        if keys.len() >= STEP_LOG_OBJECTS.len() {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the step's log files under {prefix}, got {keys:?}"
        );
        // A client learns the files are there by reading the step, which is what the agent's
        // delivery is owed by; the listing is then what reports them.
        emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `DescribeStep` until the step `step_id` reports the log file its push wrote, answering
/// the path it reports. The wait is on the CONDITION: a step holds that path from the push that
/// delivered its files, so the read looks again until it is there or the generous [`DEADLINE`]
/// passes.
async fn reported_log_file(
    case: &str,
    router: &MultiServiceRouter,
    cluster_id: &str,
    step_id: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = emr_ok(
            router,
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": step_id }),
        )
        .await;
        if let Some(path) = described["Step"]["Status"]["FailureDetails"]["LogFile"].as_str() {
            return path.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the failed step to report its log file, got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every row: the step's own log directory holds exactly the three gzipped files, and each
/// carries that run's own output.
#[tokio::test]
async fn a_finished_step_leaves_its_log_directory_in_amazon_s3() {
    let router = emulator();
    let cases = cases();

    let (status, answered) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
    for (ordinal, case) in cases.iter().enumerate() {
        put_object(
            &router,
            case.label,
            &application_key(ordinal),
            case.source.as_bytes(),
        )
        .await;
    }

    // One cluster carrying every row's step, run strictly in order.
    let steps: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(ordinal, case)| step_config(case, ordinal))
        .collect();
    let cluster_id = launch(
        &router,
        "step log delivery",
        json!(steps),
        Some(&format!("s3://{BUCKET}/{LOG_PREFIX}/")),
    )
    .await;

    let settled = drained("the queue", &router, &cluster_id).await;
    assert_eq!(
        settled.len(),
        cases.len(),
        "expected one step per row back, got {settled:?}"
    );

    for (case, step) in cases.iter().zip(&settled) {
        let label = case.label;
        let step_id = step["Id"].as_str().unwrap_or_default();
        assert_eq!(
            step["Status"]["State"].as_str(),
            Some(case.state),
            "case {label:?}: expected the step to end {}, got {step}",
            case.state
        );

        let prefix = format!("{LOG_PREFIX}/{cluster_id}/{}/{step_id}", "steps");
        let keys = delivered(label, &router, &cluster_id, &prefix).await;
        let expected: BTreeSet<String> = STEP_LOG_OBJECTS
            .iter()
            .map(|name| format!("{prefix}/{name}"))
            .collect();
        let held: BTreeSet<String> = keys.iter().cloned().collect();
        assert_eq!(
            held, expected,
            "case {label:?}: expected exactly {expected:?} under {prefix}, got {held:?}"
        );

        let controller = gunzipped(&router, label, &format!("{prefix}/{CONTROLLER_LOG_OBJECT}")).await;
        let stderr = gunzipped(&router, label, &format!("{prefix}/{STDERR_LOG_OBJECT}")).await;
        let stdout = gunzipped(&router, label, &format!("{prefix}/{STDOUT_LOG_OBJECT}")).await;

        // The controller log reports the status the work exited with, in the step controller's
        // own words.
        let exit = format!("exitCode {}", case.exit_status);
        assert!(
            controller.contains(&exit),
            "case {label:?}: expected {exit:?} in the controller log, got {controller:?}"
        );
        assert!(
            controller.contains(step_id),
            "case {label:?}: expected the controller log to name the step, got {controller:?}"
        );
        let returned = format!("Command exiting with ret '{}'", case.exit_status);
        assert!(
            stderr.contains(&returned),
            "case {label:?}: expected {returned:?} in the standard-error file, got {stderr:?}"
        );
        for fragment in &case.stderr_holds {
            assert!(
                stderr.contains(fragment.as_str()),
                "case {label:?}: expected {fragment:?} in the standard-error file, got {stderr:?}"
            );
        }
        for fragment in &case.stdout_holds {
            assert!(
                stdout.contains(fragment.as_str()),
                "case {label:?}: expected {fragment:?} in the standard-output file, got {stdout:?}"
            );
        }
        // The two streams are kept APART: what the driver printed is not in its error file,
        // and a traceback appears only for a run that raised one — in neither file of a run
        // that did not.
        for fragment in &case.stdout_holds {
            assert!(
                !stderr.contains(fragment.as_str()),
                "case {label:?}: the standard-output text {fragment:?} belongs in the output \
                 file alone, got {stderr:?}"
            );
        }
        let traced = [&controller, &stderr, &stdout]
            .iter()
            .any(|text| text.contains(TRACEBACK_HEADER));
        assert_eq!(
            traced, case.traceback,
            "case {label:?}: expected a traceback in the files: {}, got {}; stderr {stderr:?}",
            case.traceback, traced
        );

        // A failed step reports where its own log file is, and that is where it really is. The
        // path belongs to the files, so it is reported from the push that WROTE them: the read
        // looks again until the step names it.
        if case.state == "FAILED" {
            let log_file = reported_log_file(label, &router, &cluster_id, step_id).await;
            assert_eq!(
                log_file,
                format!("s3://{BUCKET}/{prefix}/{STDERR_LOG_OBJECT}"),
                "case {label:?}: expected the step to report its delivered log file, got \
                 {log_file:?}"
            );
        }
    }
}

/// A cluster launched with NO `LogUri` has nowhere to write a step's files, so none is written:
/// the step still runs and still ends, and the bucket holds nothing but what this suite put
/// there.
#[tokio::test]
async fn a_cluster_with_no_log_destination_writes_no_log_files() {
    let router = emulator();
    let (status, _) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!((200..300).contains(&status), "CreateBucket must succeed");
    let key = "applications/no-destination.py";
    put_object(
        &router,
        "no destination",
        key,
        b"from pyspark import SparkContext\nSparkContext.getOrCreate().parallelize([1]).sum()\n",
    )
    .await;

    let cluster_id = launch(
        &router,
        "no log destination",
        json!([{
            "Name": "no log destination",
            "ActionOnFailure": "CONTINUE",
            "HadoopJarStep": {
                "Jar": "command-runner.jar",
                "Args": ["spark-submit", format!("s3://{BUCKET}/{key}")],
            },
        }]),
        None,
    )
    .await;
    let settled = drained("no destination", &router, &cluster_id).await;
    assert_eq!(
        settled.first().and_then(|step| step["Status"]["State"].as_str()),
        Some("COMPLETED"),
        "a step whose application exits cleanly completes, got {settled:?}"
    );
    let keys = keys_under(&router, "").await;
    assert_eq!(
        keys,
        vec![key.to_string()],
        "a cluster that named no LogUri writes no log file, got {keys:?}"
    );
    // Nor does the step claim a log file it has none of.
    let log_file = &settled[0]["Status"]["FailureDetails"];
    assert!(
        log_file.is_null(),
        "a completed step reports no failure, got {log_file}"
    );
}

/// A cluster whose `LogUri` names a bucket the object store does not hold has nowhere to write
/// the files either: the step still ends, and it reports no log file, because none exists.
#[tokio::test]
async fn a_destination_that_does_not_resolve_leaves_no_log_file_reported() {
    let router = emulator();
    let (status, _) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!((200..300).contains(&status), "CreateBucket must succeed");
    let key = "applications/absent-destination.py";
    put_object(
        &router,
        "an absent destination",
        key,
        format!("raise RuntimeError(\"{MARKER}-absent\")\n").as_bytes(),
    )
    .await;

    let cluster_id = launch(
        &router,
        "an absent destination",
        json!([{
            "Name": "an absent destination",
            "ActionOnFailure": "CONTINUE",
            "HadoopJarStep": {
                "Jar": "command-runner.jar",
                "Args": ["spark-submit", format!("s3://{BUCKET}/{key}")],
            },
        }]),
        // No bucket of this name was ever created, so no object can land there.
        Some("s3://emr-step-logs-no-such-bucket/logs/"),
    )
    .await;
    let settled = drained("an absent destination", &router, &cluster_id).await;
    let step = settled
        .first()
        .expect("the submitted step comes back")
        .clone();
    assert_eq!(
        step["Status"]["State"].as_str(),
        Some("FAILED"),
        "an application that raises fails its step, got {step}"
    );
    assert!(
        step["Status"]["FailureDetails"]["LogFile"].is_null(),
        "a step whose log files could not be written reports no log file, got {step}"
    );
    // And it reports none at ANY read, not only at the one this case happened to take: the
    // delivery is attempted whenever the step is reported, so the step is read through the
    // window those attempts are made in rather than sampled once.
    for _ in 0..READS_PAST_THE_END {
        let again = emr_ok(&router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        let step = again["Steps"][0].clone();
        assert!(
            step["Status"]["FailureDetails"]["LogFile"].is_null(),
            "a step whose log files could not be written reports no log file, got {step}"
        );
    }
}
