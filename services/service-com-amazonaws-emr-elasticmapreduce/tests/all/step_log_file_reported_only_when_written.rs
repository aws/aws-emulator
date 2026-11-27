// SPDX-License-Identifier: CC0-1.0

//! The log file a step REPORTS, against what the object store really holds, at every instant of
//! the step's life.
//!
//! THE RULE: "The path to the log file where the step failure root cause was originally
//! recorded." (`FailureDetails$LogFile` doc.) A path is reported only once that file IS there:
//! writing a step's log directory into Amazon S3 takes time and the calls that report the step
//! do not wait for it, so a read taken while those files are in flight — and every read of a
//! step whose push never landed — reports no log file at all. Whenever a read DOES report one,
//! the object is one a client's own `GetObject` finds.
//!
//! The rows span what the rule generalises over: whether the destination the launch named
//! RESOLVES at all, and the form of the prefix inside it (a prefix, and the bucket's root).
//! Every read of every row is checked against the store, and the reads are taken all through
//! the step's life — before it ends, as it ends and after — so a path reported for a file that
//! is still being written, or for one no write ever landed, is caught by the read reporting it.
//!
//! Amazon S3 is mounted BESIDE Amazon EMR on one engine, so the objects asserted on are the
//! objects a client's own `ListObjectsV2` and `GetObject` find. The wiring, the one `awsJson1_1`
//! call and the object-store requests are [`crate::step_log_delivery`]'s, so the suites of this
//! crate cannot disagree about how a cluster is launched or how an object is written.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    service_state, STDERR_LOG_OBJECT, STEP_LOG_OBJECTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::step_log_delivery::{
    emr_ok, put_object, s3, BUCKET, DEADLINE, MARKER, POLL_INTERVAL, TERMINAL,
};

/// A second bucket of the object store, so one row delivers at a bucket's ROOT without its
/// log files landing beside an application.
const ROOT_BUCKET: &str = "emr-step-log-paths-root";

/// A bucket name the object store holds NO bucket under: nothing this suite creates, so no
/// object can ever land there.
const ABSENT_BUCKET: &str = "emr-step-log-paths-absent";

/// A bucket that does not exist when a step ends and is created AFTERWARDS, so the files that
/// could not land once are pushed again when the destination resolves.
const LATER_BUCKET: &str = "emr-step-log-paths-later";

/// The roles a launch names.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// How many further reads a row takes after the step has ENDED, so the window a delivery is
/// made in is read through rather than read once.
const READS_AFTER_THE_END: usize = 20;

/// The key segment one cluster's step directories live under.
const STEPS_SEGMENT: &str = "steps";

/// Amazon EMR and Amazon S3 on one engine, with the ENGINE they are mounted on kept beside the
/// router: the engine is where this service's own store lives, which is what lets a case hold a
/// delivery in flight. [`crate::step_log_delivery::emulator`] is the same wiring for a case that
/// needs the router alone.
fn emulator_on_one_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Create `bucket`, which must be served.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let (status, answered) = s3(router, "PUT", &format!("/{bucket}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket {bucket} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// The keys of every object of `bucket` under `prefix`, from `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, bucket: &str, prefix: &str) -> Vec<String> {
    let (status, listing) = s3(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={prefix}"),
        &[],
    )
    .await;
    let listing = String::from_utf8_lossy(&listing).to_string();
    assert_eq!(
        status, 200,
        "listing s3://{bucket}/{prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// One row: the destination a launch names, and whether a step's files can land there.
struct Row {
    case: &'static str,
    /// The bucket the `LogUri` names.
    bucket: &'static str,
    /// The key prefix it names inside that bucket — empty for the bucket's root.
    prefix: &'static str,
    /// Whether the object store holds that bucket when the step ENDS, which is whether the push
    /// it is owed then can land.
    resolves: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a destination that resolves, under a prefix",
            bucket: BUCKET,
            prefix: "logs",
            resolves: true,
        },
        Row {
            case: "a destination that resolves, at the bucket's root",
            bucket: ROOT_BUCKET,
            prefix: "",
            resolves: true,
        },
        Row {
            case: "a destination that does not resolve",
            bucket: ABSENT_BUCKET,
            prefix: "logs",
            resolves: false,
        },
    ]
}

impl Row {
    /// The `LogUri` this row's launch names.
    fn log_uri(&self) -> String {
        if self.prefix.is_empty() {
            format!("s3://{}", self.bucket)
        } else {
            format!("s3://{}/{}/", self.bucket, self.prefix)
        }
    }

    /// The key this row's step keeps its log directory under.
    fn step_prefix(&self, cluster_id: &str, step_id: &str) -> String {
        if self.prefix.is_empty() {
            format!("{cluster_id}/{STEPS_SEGMENT}/{step_id}")
        } else {
            format!("{}/{cluster_id}/{STEPS_SEGMENT}/{step_id}", self.prefix)
        }
    }
}

/// A cluster that stays up, carrying one step whose application RAISES — so the step fails and
/// a `FailureDetails` is what reports its log file.
async fn launch(router: &MultiServiceRouter, row: &Row, application_key: &str) -> String {
    let launched = emr_ok(
        router,
        "RunJobFlow",
        json!({
            "Name": row.case,
            "ReleaseLabel": "emr-6.15.0",
            "LogUri": row.log_uri(),
            "JobFlowRole": INSTANCE_PROFILE,
            "ServiceRole": SERVICE_ROLE,
            "Applications": [{ "Name": "Spark" }],
            "Steps": [{
                "Name": row.case,
                "ActionOnFailure": "CONTINUE",
                "HadoopJarStep": {
                    "Jar": "command-runner.jar",
                    "Args": ["spark-submit", format!("s3://{BUCKET}/{application_key}")],
                },
            }],
            "StepConcurrencyLevel": 1,
            "Instances": {
                "InstanceCount": 2,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            },
        }),
    )
    .await;
    launched["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// The id of the one step a row's cluster carries, polled for because the queue is reported by
/// the cluster rather than by the launch. The wait is on the CONDITION.
async fn step_id_of(router: &MultiServiceRouter, case: &str, cluster_id: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let listed = emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
        if let Some(id) = listed["Steps"][0]["Id"].as_str() {
            return id.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the submitted step to be listed, got {listed}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// THE CHECK every read of this suite is put through: whatever a read reports as the step's log
/// file is an object the store HOLDS, under that step's own directory. Answers whether a path
/// was reported at all.
async fn reported_path_resolves(
    router: &MultiServiceRouter,
    row: &Row,
    cluster_id: &str,
    step_id: &str,
    read_by: &str,
    step: &Value,
) -> bool {
    let case = row.case;
    let Some(reported) = step["Status"]["FailureDetails"]["LogFile"].as_str() else {
        return false;
    };
    let expected = format!(
        "s3://{}/{}/{STDERR_LOG_OBJECT}",
        row.bucket,
        row.step_prefix(cluster_id, step_id)
    );
    assert_eq!(
        reported, expected,
        "case {case:?}: {read_by} must report the step's own log file, got {reported:?}"
    );
    let (bucket, key) = reported
        .strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .unwrap_or_else(|| panic!("case {case:?}: {reported:?} is no s3:// URI"));
    let (status, body) = s3(router, "GET", &format!("/{bucket}/{key}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: {read_by} reported {reported:?}, which GetObject must find, got {status} \
         {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        body.get(..2),
        Some([0x1f, 0x8b].as_slice()),
        "case {case:?}: {reported:?} must be the gzip stream the push wrote, got {:?}",
        body.get(..2)
    );
    true
}

/// Poll until the step has ENDED, putting every read of the way there through the check. The
/// wait is on the CONDITION: it looks again until the step is terminal or the generous
/// [`DEADLINE`] passes.
async fn ended_step(
    router: &MultiServiceRouter,
    row: &Row,
    cluster_id: &str,
    step_id: &str,
) -> Value {
    let case = row.case;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (step, _) = read_both_ways(router, row, cluster_id, step_id).await;
        let state = step["Status"]["State"].as_str().unwrap_or_default().to_string();
        if TERMINAL.contains(&state.as_str()) {
            return step;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the step to reach a terminal state, got {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the step through `DescribeStep` and through `ListSteps`, putting BOTH reports through
/// the check. Answers the `DescribeStep` projection and whether a log file was reported.
async fn read_both_ways(
    router: &MultiServiceRouter,
    row: &Row,
    cluster_id: &str,
    step_id: &str,
) -> (Value, bool) {
    let listed = emr_ok(router, "ListSteps", json!({ "ClusterId": cluster_id })).await;
    let summary = listed["Steps"][0].clone();
    let listed_path =
        reported_path_resolves(router, row, cluster_id, step_id, "ListSteps", &summary).await;
    let described = emr_ok(
        router,
        "DescribeStep",
        json!({ "ClusterId": cluster_id, "StepId": step_id }),
    )
    .await;
    let step = described["Step"].clone();
    let described_path =
        reported_path_resolves(router, row, cluster_id, step_id, "DescribeStep", &step).await;
    (step, listed_path || described_path)
}

/// Every row: no read ever reports a log file the object store does not hold, and a row whose
/// destination resolves ends up reporting one — the file the push really wrote.
#[tokio::test]
async fn a_reported_step_log_file_is_one_the_object_store_holds() {
    let router = crate::step_log_delivery::emulator();
    create_bucket(&router, BUCKET).await;
    create_bucket(&router, ROOT_BUCKET).await;

    for (ordinal, row) in rows().iter().enumerate() {
        let case = row.case;
        let application_key = format!("applications/raising-{ordinal}.py");
        put_object(&router, case, &application_key, &raising_application(ordinal)).await;

        let cluster_id = launch(&router, row, &application_key).await;
        let step_id = step_id_of(&router, case, &cluster_id).await;

        // The reads run all through the step's life and on past its end, so the window a
        // delivery is made in is read THROUGH: a path reported for a file still in flight, or
        // for one no write landed, is caught by the read that reports it.
        let deadline = Instant::now() + DEADLINE;
        let mut after_the_end = 0usize;
        let mut ever_reported = false;
        let ended = loop {
            let (step, reported) = read_both_ways(&router, row, &cluster_id, &step_id).await;
            ever_reported |= reported;
            let state = step["Status"]["State"].as_str().unwrap_or_default().to_string();
            if TERMINAL.contains(&state.as_str()) {
                after_the_end += 1;
                // A destination that resolves owes the path too: the files land, and from then
                // on the step reports them.
                if after_the_end > READS_AFTER_THE_END && (!row.resolves || ever_reported) {
                    break step;
                }
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the step to end and (resolving: {}) to report its log \
                 file, saw state {state:?} after {after_the_end} reads past the end",
                row.resolves
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };

        assert_eq!(
            ended["Status"]["State"].as_str(),
            Some("FAILED"),
            "case {case:?}: an application that raises fails its step, got {ended}"
        );
        let reported = ended["Status"]["FailureDetails"]["LogFile"].clone();
        if row.resolves {
            // The path the step reports, and the directory the push wrote, are the same place.
            let prefix = row.step_prefix(&cluster_id, &step_id);
            let held: BTreeSet<String> = keys_under(&router, row.bucket, &prefix)
                .await
                .into_iter()
                .collect();
            let expected: BTreeSet<String> = STEP_LOG_OBJECTS
                .iter()
                .map(|name| format!("{prefix}/{name}"))
                .collect();
            assert_eq!(
                held, expected,
                "case {case:?}: expected exactly {expected:?} under {prefix}, got {held:?}"
            );
            assert_eq!(
                reported.as_str(),
                Some(format!("s3://{}/{prefix}/{STDERR_LOG_OBJECT}", row.bucket).as_str()),
                "case {case:?}: expected the step to report its delivered log file, got {ended}"
            );
        } else {
            assert!(
                reported.is_null(),
                "case {case:?}: a step whose log files could not be written reports no log file, \
                 got {ended}"
            );
            assert!(
                !ever_reported,
                "case {case:?}: no read may report a log file for a destination that holds none"
            );
            // Nor did a file of this cluster land in a bucket the store DOES hold: a push that
            // could not reach its destination wrote nothing anywhere.
            for bucket in [BUCKET, ROOT_BUCKET] {
                let keys = keys_under(&router, bucket, "").await;
                assert!(
                    keys.iter().all(|key| !key.contains(&cluster_id)),
                    "case {case:?}: a push that did not land wrote nothing, got {keys:?} in \
                     {bucket}"
                );
            }
        }
    }
}

/// The application every single-step case below submits: one that RAISES, so the step fails and
/// a `FailureDetails` is what reports its log file.
fn raising_application(ordinal: usize) -> Vec<u8> {
    format!(
        "import sys\n\
         from pyspark import SparkContext\n\
         SparkContext.getOrCreate().parallelize([1]).collect()\n\
         raise RuntimeError(\"{MARKER}-{ordinal}\")\n"
    )
    .into_bytes()
}

/// The files the ended step of `cluster_id` is owed, TAKEN the way a push takes them: holding
/// them is a delivery in flight, which is the window a read must report nothing in. The wait is
/// on the CONDITION, because a push of this service's own may be holding them when the case
/// asks.
async fn claimed_step_files(
    engine: &engine::InMemoryEngine,
    case: &str,
    cluster_id: &str,
) -> Vec<service_com_amazonaws_emr_elasticmapreduce::state::PendingStepLogs> {
    let state = service_state(engine).expect("this service's store is mounted on the engine");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let owed = state.lock().take_due_step_logs(cluster_id);
        if !owed.is_empty() {
            return owed;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the ended step to be owed its log directory"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A delivery in FLIGHT — the files taken, nothing written yet — is reported by no read: the
/// path belongs to the files, not to the attempt, so it appears only once they have landed.
#[tokio::test]
async fn a_delivery_still_in_flight_is_reported_by_no_read() {
    let (router, engine) = emulator_on_one_engine();
    create_bucket(&router, BUCKET).await;
    let row = Row {
        case: "a delivery in flight",
        bucket: ABSENT_BUCKET,
        prefix: "logs",
        resolves: false,
    };
    let case = row.case;
    let application_key = "applications/in-flight.py";
    put_object(&router, case, application_key, &raising_application(10)).await;

    let cluster_id = launch(&router, &row, application_key).await;
    let step_id = step_id_of(&router, case, &cluster_id).await;
    let ended = ended_step(&router, &row, &cluster_id, &step_id).await;
    assert_eq!(
        ended["Status"]["State"].as_str(),
        Some("FAILED"),
        "case {case:?}: an application that raises fails its step, got {ended}"
    );

    // The files are TAKEN here, which is exactly what a push does before it writes them: from
    // now until the write lands, this step's directory exists nowhere but in flight.
    let owed = claimed_step_files(&engine, case, &cluster_id).await;
    assert_eq!(
        owed.len(),
        1,
        "case {case:?}: the ended step is owed one log directory, got {owed:?}"
    );
    assert_eq!(
        owed[0].objects.len(),
        STEP_LOG_OBJECTS.len(),
        "case {case:?}: a step's directory holds every file of a run, got {owed:?}"
    );
    for _ in 0..READS_AFTER_THE_END {
        let (step, reported) = read_both_ways(&router, &row, &cluster_id, &step_id).await;
        assert!(
            !reported,
            "case {case:?}: a delivery still in flight is reported by no read, got {step}"
        );
    }

    // The write did not go through, so the directory is given back: the step is owed its files
    // again, and still reports none.
    service_state(&*engine)
        .expect("this service's store is mounted on the engine")
        .lock()
        .forget_delivered_step_logs(&cluster_id, std::slice::from_ref(&step_id));
    for _ in 0..READS_AFTER_THE_END {
        let (step, reported) = read_both_ways(&router, &row, &cluster_id, &step_id).await;
        assert!(
            !reported,
            "case {case:?}: a push that did not land leaves no path to report, got {step}"
        );
    }
    let owed_again = claimed_step_files(&engine, case, &cluster_id).await;
    assert_eq!(
        owed_again.len(),
        1,
        "case {case:?}: a push that did not land leaves the step owed its files, got \
         {owed_again:?}"
    );
}

/// A destination that did not resolve when the step ended leaves the step OWED its files: once
/// the object store holds the bucket, a later push lands them and the step reports the path —
/// which is then a path a read of the store finds.
#[tokio::test]
async fn files_a_destination_could_not_take_land_once_it_resolves() {
    let router = crate::step_log_delivery::emulator();
    create_bucket(&router, BUCKET).await;
    let row = Row {
        case: "a destination that resolves later",
        bucket: LATER_BUCKET,
        prefix: "logs",
        // What the destination comes to be: no bucket of this name exists when the step ends,
        // and the case creates it afterwards.
        resolves: true,
    };
    let case = row.case;
    let application_key = "applications/later-destination.py";
    put_object(&router, case, application_key, &raising_application(11)).await;

    let cluster_id = launch(&router, &row, application_key).await;
    let step_id = step_id_of(&router, case, &cluster_id).await;
    let ended = ended_step(&router, &row, &cluster_id, &step_id).await;
    assert_eq!(
        ended["Status"]["State"].as_str(),
        Some("FAILED"),
        "case {case:?}: an application that raises fails its step, got {ended}"
    );
    for _ in 0..READS_AFTER_THE_END {
        let (step, reported) = read_both_ways(&router, &row, &cluster_id, &step_id).await;
        assert!(
            !reported,
            "case {case:?}: no read reports a path while the destination holds nothing, got {step}"
        );
    }

    // The destination appears. The files were never written, so the step is still owed them,
    // and the next push lands them.
    create_bucket(&router, LATER_BUCKET).await;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (step, reported) = read_both_ways(&router, &row, &cluster_id, &step_id).await;
        if reported {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the files to land once the destination resolved, got {step}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let prefix = row.step_prefix(&cluster_id, &step_id);
    let held: BTreeSet<String> = keys_under(&router, LATER_BUCKET, &prefix)
        .await
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = STEP_LOG_OBJECTS
        .iter()
        .map(|name| format!("{prefix}/{name}"))
        .collect();
    assert_eq!(
        held, expected,
        "case {case:?}: expected exactly {expected:?} under {prefix}, got {held:?}"
    );
}
