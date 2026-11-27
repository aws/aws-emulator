// SPDX-License-Identifier: CC0-1.0

//! A step that submits a Spark application RUNS it, end to end over the wire.
//!
//! THE RULE: "Amazon EMR executes each step in the order listed. For a step to be considered
//! complete, the main function must exit with a zero exit code and all Hadoop jobs started
//! while the step was running must have completed and run successfully."
//! (`AddJobFlowSteps` doc.) So the state a step ends in is the outcome of RUNNING what it
//! submitted, and the effects of that run are real: a step whose `command-runner.jar`
//! command is a `spark-submit` of a Python application in Amazon S3 runs that application,
//! reaches `COMPLETED` when it exits cleanly and `FAILED` carrying the application's own
//! error when it raises, and the objects the application writes are objects a later
//! `GetObject` reads. A step whose submission this emulator holds no engine or no bytes for
//! keeps the not-executed failure that says so, rather than reporting an outcome for work
//! nothing performed.
//!
//! The rows span what the rule generalises over: which Spark surface the application
//! computes through (the dataset's own transformations and actions, the Hadoop filesystem
//! the JVM gateway hands out, a dataset of rows and its writer, a read of a stored object,
//! the version of the engine the cluster's release ships), how many partitions the work is
//! sliced into, whether the application exits cleanly or raises, whether it reaches for a
//! surface this emulator has none of, whether its file exists at all, and whether the step
//! submits a Python application in the first place.
//!
//! Amazon S3 is mounted BESIDE Amazon EMR on one engine, and every application is written
//! through S3's own `CreateBucket`/`PutObject` and every output read back through
//! `GetObject` — so the code that runs is the code the bucket holds, and the output is a
//! real object rather than anything this suite handed Amazon EMR.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    APPLICATION_FAILED_REASON, APPLICATION_UNREADABLE_REASON, CLUSTER_STEPS_COMPLETED_REASON,
    CLUSTER_STEPS_COMPLETED_WITH_ERRORS_REASON, STEP_NOT_EXECUTED_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region both services are addressed in: the one whose `CreateBucket` needs no
/// location constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every application and every output of this suite lives in.
///
/// `pub(crate)` so a sibling suite drives the same Amazon S3 and Amazon EMR pair through this
/// harness rather than standing up a second one.
pub(crate) const BUCKET: &str = "emr-spark-applications";

/// The roles a launch names.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// How long a poll waits for a step to reach a terminal state: both provisioning phases, the
/// step's own span and one application run, with room to spare.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The states a step never leaves (`com.amazonaws.emr#StepState`).
const TERMINAL: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// The states a cluster never leaves (`com.amazonaws.emr#ClusterState`).
const TERMINAL_CLUSTER: [&str; 2] = ["TERMINATED", "TERMINATED_WITH_ERRORS"];

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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/s3/\
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
pub(crate) async fn put_object(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    body: &[u8],
) {
    let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: PutObject {key} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// The object at `key`, or `None` when the store holds none.
pub(crate) async fn object(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let (status, answered) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    (200..300)
        .contains(&status)
        .then(|| String::from_utf8_lossy(&answered).to_string())
}

/// The keys of every object under `prefix`, from `ListObjectsV2`.
pub(crate) async fn keys_under(
    router: &MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Vec<String> {
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
        "case {case:?}: listing s3://{BUCKET}/{prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// The one part file a dataset write left under the output directory `prefix`, as its body.
///
/// A part file's name carries the UUID the write job was minted with, so it is not a name a
/// row can spell: the part is the one object under the directory whose name is not a
/// committer's marker, and its name is asserted to be the `part-` prefixed one a write leaves.
async fn part_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Option<String> {
    let listed = keys_under(router, case, prefix).await;
    let parts: Vec<String> = listed
        .iter()
        .filter(|key| {
            let basename = key.rsplit('/').next().unwrap_or_default();
            !basename.starts_with('_') && !basename.starts_with('.')
        })
        .cloned()
        .collect();
    assert_eq!(
        parts.len(),
        1,
        "case {case:?}: expected one part file under {prefix}, got {listed:?}"
    );
    let key = &parts[0];
    let basename = key.rsplit('/').next().unwrap_or_default();
    assert!(
        basename.starts_with("part-"),
        "case {case:?}: expected a part file under {prefix}, got {basename:?}"
    );
    object(router, key).await
}

/// What a row expects its step to end as.
enum Ends {
    /// The application exited cleanly: the step completed, and these `(key, content)` pairs
    /// are the objects it really wrote. A key ending in `/` names the output DIRECTORY of a
    /// dataset write instead, whose one part file carries that content — such a part is named
    /// after the write job that produced it, so no row can spell its key.
    Completed(Vec<(String, String)>),
    /// The application failed: the step's `FailureDetails` carry this reason and every one
    /// of these fragments in its message.
    Failed(&'static str, Vec<String>),
}

/// One row: the application this step submits, and the end it must reach.
struct Case {
    label: &'static str,
    /// The application's source, written into the bucket under the row's own key — `None`
    /// for a row whose application file is never put there.
    source: Option<String>,
    /// The `HadoopJarStep` the step submits, built from the row's own application key.
    submits_jar: bool,
    /// The arguments the submit passes the application, after its path.
    arguments: Vec<String>,
    ends: Ends,
}

/// The sum of every value in `1..=100` whose remainder modulo three is `remainder`, computed
/// here so the expected output is this suite's own arithmetic rather than the emulator's.
fn modular_sum(remainder: u32) -> u32 {
    (1..=100u32).filter(|value| value % 3 == remainder).sum()
}

fn cases() -> Vec<Case> {
    let expected_sums = format!(
        "0,{}\n1,{}\n2,{}\n",
        modular_sum(0),
        modular_sum(1),
        modular_sum(2)
    );
    vec![
        Case {
            label: "a keyed reduction written through the Hadoop filesystem",
            source: Some(
                r#"
import sys
from pyspark.sql import SparkSession

spark = SparkSession.builder.getOrCreate()
rows = (
    spark.sparkContext.parallelize(range(1, 101), 2)
    .map(lambda value: (value % 3, value))
    .reduceByKey(lambda left, right: left + right)
    .sortByKey()
    .collect()
)
text = "".join("%d,%d\n" % row for row in rows)
path = spark._jvm.org.apache.hadoop.fs.Path(sys.argv[1])
out = path.getFileSystem(spark._jsc.hadoopConfiguration()).create(path, True)
out.write(bytearray(text.encode("utf-8")))
out.close()
spark.stop()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: vec![format!("s3://{BUCKET}/out/sums.csv")],
            ends: Ends::Completed(vec![("out/sums.csv".to_string(), expected_sums)]),
        },
        Case {
            label: "a filtered dataset saved as text, one part file per partition",
            source: Some(
                r#"
import sys
from pyspark import SparkContext

sc = SparkContext.getOrCreate()
(
    sc.parallelize(range(1, 11), 3)
    .filter(lambda value: value % 2 == 0)
    .map(lambda value: value * value)
    .saveAsTextFile(sys.argv[1])
)
sc.stop()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: vec![format!("s3://{BUCKET}/out/squares")],
            ends: Ends::Completed(vec![
                ("out/squares/part-00000".to_string(), "4\n16\n".to_string()),
                ("out/squares/part-00001".to_string(), "36\n".to_string()),
                (
                    "out/squares/part-00002".to_string(),
                    "64\n100\n".to_string(),
                ),
                ("out/squares/_SUCCESS".to_string(), String::new()),
            ]),
        },
        Case {
            label: "a read of a stored object, counted and written as rows",
            source: Some(
                r#"
import sys
from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("counted").getOrCreate()
lines = spark.sparkContext.textFile(sys.argv[1])
total = lines.map(lambda line: len(line)).sum()
frame = spark.createDataFrame([(lines.count(), total)], ["lines", "characters"])
frame.write.mode("overwrite").csv(sys.argv[2])
spark.stop()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: vec![
                format!("s3://{BUCKET}/in/words.txt"),
                format!("s3://{BUCKET}/out/counted"),
            ],
            // "alpha\nbeta\ngamma\n" is three lines of 5, 4 and 5 characters.
            ends: Ends::Completed(vec![
                ("out/counted/".to_string(), "3,14\n".to_string()),
                ("out/counted/_SUCCESS".to_string(), String::new()),
            ]),
        },
        Case {
            label: "an application that writes the Spark version its cluster runs",
            source: Some(
                r#"
import sys
from pyspark.sql import SparkSession

spark = SparkSession.builder.getOrCreate()
assert spark.version == spark.sparkContext.getConf().get("spark.version", spark.version)
path = spark._jvm.org.apache.hadoop.fs.Path(sys.argv[1])
out = path.getFileSystem(spark._jsc.hadoopConfiguration()).create(path, True)
out.write(spark.version)
out.close()
spark.stop()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: vec![format!("s3://{BUCKET}/out/version.txt")],
            // The version the cluster's own release ships: every launch of this suite is
            // `emr-7.9.0`, and the release catalog publishes Spark 3.5.5 for that release
            // (asserted against `DescribeReleaseLabel` in `cluster_read_facts.rs`).
            ends: Ends::Completed(vec![("out/version.txt".to_string(), "3.5.5".to_string())]),
        },
        Case {
            label: "an application that reads its dataset back through the forms a frame's own \
                    columns take",
            source: Some(
                r#"
import sys
from pyspark.sql import SparkSession

spark = SparkSession.builder.getOrCreate()
frame = spark.createDataFrame(
    [(index, "n%d" % index, index * 1.5) for index in range(10)],
    "id bigint, name string, amount double",
)
frame.write.mode("overwrite").parquet(sys.argv[1])
# A read naming no format reads the session's default source format.
read = spark.read.load(sys.argv[1])
# A column by attribute, a column by subscript, a filter spelled as SQL text, and the
# rows as the JSON documents `toJSON` answers.
probed = read.filter(read.id == 7).collect()[0]
subscript = read.filter(read["name"] == "n3").collect()[0]
counted = read.filter("amount > 12").count()
documents = read.filter(read.id == 0).toJSON().collect()
text = "%s|%s|%d|%s" % (probed["name"], subscript["id"], counted, documents[0])
path = spark._jvm.org.apache.hadoop.fs.Path(sys.argv[2])
out = path.getFileSystem(spark._jsc.hadoopConfiguration()).create(path, True)
out.write(bytearray(text.encode("utf-8")))
out.close()
spark.stop()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: vec![
                format!("s3://{BUCKET}/out/columns"),
                format!("s3://{BUCKET}/out/columns.txt"),
            ],
            // The row the attribute form probed, the id the subscript form probed, how many
            // rows the SQL-text filter kept (only `9 * 1.5`), and the first row as JSON —
            // this suite's own arithmetic over the ten rows the program names.
            ends: Ends::Completed(vec![(
                "out/columns.txt".to_string(),
                "n7|3|1|{\"id\": 0, \"name\": \"n0\", \"amount\": 0.0}".to_string(),
            )]),
        },
        Case {
            label: "an application that raises",
            source: Some(
                r#"
from pyspark.sql import SparkSession

spark = SparkSession.builder.getOrCreate()
values = spark.sparkContext.parallelize([1, 2, 3]).collect()
raise RuntimeError("the job gave up on %d values" % len(values))
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: Vec::new(),
            ends: Ends::Failed(
                APPLICATION_FAILED_REASON,
                vec![
                    "RuntimeError".to_string(),
                    "the job gave up on 3 values".to_string(),
                ],
            ),
        },
        Case {
            label: "an application reaching for a Spark surface this emulator has none of",
            source: Some(
                r#"
from pyspark.sql import SparkSession

spark = SparkSession.builder.getOrCreate()
spark.readStream.format("kafka").load()
"#
                .to_string(),
            ),
            submits_jar: false,
            arguments: Vec::new(),
            ends: Ends::Failed(APPLICATION_FAILED_REASON, vec!["AttributeError".to_string()]),
        },
        Case {
            label: "an application file the bucket holds nothing under",
            source: None,
            submits_jar: false,
            arguments: Vec::new(),
            ends: Ends::Failed(
                APPLICATION_UNREADABLE_REASON,
                vec!["Unable to download the application".to_string()],
            ),
        },
        Case {
            label: "a step submitting a jar, which no engine here runs",
            source: None,
            submits_jar: true,
            arguments: Vec::new(),
            ends: Ends::Failed(STEP_NOT_EXECUTED_REASON, Vec::new()),
        },
    ]
}

/// The key one row's application is written under.
fn application_key(ordinal: usize) -> String {
    format!("applications/job-{ordinal}.py")
}

/// The `StepConfig` one row submits.
fn step_config(case: &Case, ordinal: usize) -> Value {
    let mut args: Vec<String> = vec!["spark-submit".to_string(), "--deploy-mode".to_string(), "cluster".to_string()];
    if case.submits_jar {
        args.push(format!("s3://{BUCKET}/applications/job-{ordinal}.jar"));
    } else {
        args.push(format!("s3://{BUCKET}/{}", application_key(ordinal)));
    }
    args.extend(case.arguments.iter().cloned());
    json!({
        "Name": case.label,
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": { "Jar": "command-runner.jar", "Args": args },
    })
}

/// A cluster that stays up, launched with `steps`.
pub(crate) async fn launch(
    router: &MultiServiceRouter,
    name: &str,
    steps: Value,
    kept_alive: bool,
) -> String {
    let launched = emr_ok(
        router,
        "RunJobFlow",
        json!({
            "Name": name,
            "ReleaseLabel": "emr-7.9.0",
            "JobFlowRole": INSTANCE_PROFILE,
            "ServiceRole": SERVICE_ROLE,
            "Applications": [{ "Name": "Spark" }],
            "Steps": steps,
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": kept_alive,
            },
        }),
    )
    .await;
    launched["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// Poll `ListSteps` until every step of `cluster_id` is terminal, answering them in
/// submission order.
pub(crate) async fn drained(
    case: &str,
    router: &MultiServiceRouter,
    cluster_id: &str,
) -> Vec<Value> {
    let started = Instant::now();
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
            started.elapsed() < DEADLINE,
            "case {case:?}: expected every step to reach a terminal state, got {states:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `DescribeCluster` until the cluster is terminal, answering it as last read.
async fn stopped(case: &str, router: &MultiServiceRouter, cluster_id: &str) -> Value {
    let started = Instant::now();
    loop {
        let cluster = emr_ok(router, "DescribeCluster", json!({ "ClusterId": cluster_id })).await
            ["Cluster"]
            .clone();
        let state = cluster["Status"]["State"].as_str().unwrap_or_default().to_string();
        if TERMINAL_CLUSTER.contains(&state.as_str()) {
            return cluster;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: expected the cluster to stop, got {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every row: the step ends in what running its application answered, and the objects that
/// application wrote are objects the store really holds.
#[tokio::test]
async fn a_submitted_spark_application_runs_and_decides_its_step() {
    let router = emulator();
    let cases = cases();

    // The bucket, the applications and the one input object a row reads.
    let (status, answered) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
    put_object(&router, "input", "in/words.txt", b"alpha\nbeta\ngamma\n").await;
    for (ordinal, case) in cases.iter().enumerate() {
        if let Some(source) = case.source.as_ref() {
            put_object(
                &router,
                case.label,
                &application_key(ordinal),
                source.as_bytes(),
            )
            .await;
        }
    }

    // One cluster per row, all launched before any is waited on, each kept alive so its
    // step's own end is what is read rather than the cluster's shutdown.
    let mut launched: Vec<String> = Vec::new();
    for (ordinal, case) in cases.iter().enumerate() {
        launched.push(
            launch(
                &router,
                case.label,
                json!([step_config(case, ordinal)]),
                true,
            )
            .await,
        );
    }

    for (case, cluster_id) in cases.iter().zip(&launched) {
        let label = case.label;
        let steps = drained(label, &router, cluster_id).await;
        let step = steps
            .first()
            .unwrap_or_else(|| panic!("case {label:?}: expected the submitted step back"));
        let state = step["Status"]["State"].as_str().unwrap_or_default();
        let timeline = &step["Status"]["Timeline"];
        match &case.ends {
            Ends::Completed(objects) => {
                assert_eq!(
                    state, "COMPLETED",
                    "case {label:?}: an application that exited cleanly completes its step, \
                     got {step}"
                );
                assert!(
                    step["Status"]["FailureDetails"].is_null(),
                    "case {label:?}: a completed step reports no failure, got {step}"
                );
                for (key, expected) in objects {
                    let held = match key.strip_suffix('/') {
                        Some(prefix) => part_under(&router, label, prefix).await,
                        None => object(&router, key).await,
                    };
                    assert_eq!(
                        held.as_deref(),
                        Some(expected.as_str()),
                        "case {label:?}: expected {key} to hold {expected:?}, got {held:?}"
                    );
                }
            }
            Ends::Failed(reason, fragments) => {
                assert_eq!(
                    state, "FAILED",
                    "case {label:?}: expected the modelled failure state, got {step}"
                );
                let reported = step["Status"]["FailureDetails"]["Reason"]
                    .as_str()
                    .unwrap_or_default();
                assert_eq!(
                    reported, *reason,
                    "case {label:?}: wrong failure reason, got {step}"
                );
                let message = step["Status"]["FailureDetails"]["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                for fragment in fragments {
                    assert!(
                        message.contains(fragment.as_str()),
                        "case {label:?}: expected {fragment:?} in the failure message, \
                         got {message:?}"
                    );
                }
            }
        }
        // Either way the step reports its own timeline: it was queued, started and ended.
        for member in ["CreationDateTime", "StartDateTime", "EndDateTime"] {
            assert!(
                timeline[member].as_f64().is_some(),
                "case {label:?}: a step that ran reports {member}, got {step}"
            );
        }
        assert!(
            timeline["StartDateTime"].as_f64() <= timeline["EndDateTime"].as_f64(),
            "case {label:?}: a step ends at or after it started, got {step}"
        );

        // `DescribeStep` reports the same step as the listing: one record, two readers.
        let described = emr_ok(
            &router,
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": step["Id"] }),
        )
        .await["Step"]
            .clone();
        assert_eq!(
            described["Status"]["State"].as_str(),
            Some(state),
            "case {label:?}: DescribeStep reports the same state, got {described}"
        );
        assert_eq!(
            described["Status"]["FailureDetails"], step["Status"]["FailureDetails"],
            "case {label:?}: DescribeStep reports the same failure, got {described}"
        );
    }
}

/// A cluster that does not stay up stops once its queue has run, and the reason it reports is
/// Amazon EMR's own sentence for that shutdown — which says whether the steps succeeded.
#[tokio::test]
async fn an_auto_terminating_cluster_reports_how_its_steps_ended() {
    let router = emulator();
    let (status, _) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!((200..300).contains(&status), "CreateBucket must succeed");

    // Two rows: a step whose application exits cleanly, and one whose application raises.
    // Both drain the queue, so both clusters stop — with the sentence that says which.
    let rows: [(&str, &str, &str); 2] = [
        (
            "every step completed",
            "clean.py",
            "from pyspark import SparkContext\n\
             SparkContext.getOrCreate().parallelize([1, 2, 3]).sum()\n",
        ),
        (
            "a step that failed",
            "broken.py",
            "raise ValueError(\"nothing to compute\")\n",
        ),
    ];
    let expected = [
        CLUSTER_STEPS_COMPLETED_REASON,
        CLUSTER_STEPS_COMPLETED_WITH_ERRORS_REASON,
    ];

    let mut launched: Vec<String> = Vec::new();
    for (label, key, source) in &rows {
        put_object(&router, label, key, source.as_bytes()).await;
        launched.push(
            launch(
                &router,
                label,
                json!([{
                    "Name": label,
                    "ActionOnFailure": "CONTINUE",
                    "HadoopJarStep": {
                        "Jar": "command-runner.jar",
                        "Args": ["spark-submit", format!("s3://{BUCKET}/{key}")],
                    },
                }]),
                false,
            )
            .await,
        );
    }

    for ((label, _, _), (cluster_id, message)) in
        rows.iter().zip(launched.iter().zip(expected.iter()))
    {
        let cluster = stopped(label, &router, cluster_id).await;
        assert_eq!(
            cluster["Status"]["State"].as_str(),
            Some("TERMINATED"),
            "case {label:?}: a cluster that ran its queue stops, got {cluster}"
        );
        assert_eq!(
            cluster["Status"]["StateChangeReason"]["Code"].as_str(),
            Some("ALL_STEPS_COMPLETED"),
            "case {label:?}: the shutdown names why it happened, got {cluster}"
        );
        assert_eq!(
            cluster["Status"]["StateChangeReason"]["Message"].as_str(),
            Some(*message),
            "case {label:?}: expected the fixed sentence {message:?}, got {cluster}"
        );
    }
}
