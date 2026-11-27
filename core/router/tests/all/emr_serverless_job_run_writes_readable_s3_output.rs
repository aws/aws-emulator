// SPDX-License-Identifier: CC0-1.0

//! An EMR Serverless job run really EXECUTES the Spark application its submit named, and the
//! objects that application writes are readable afterwards through Amazon S3's own `GetObject`.
//!
//! The RULE under test: **`SparkSubmit$entryPoint` is "The entry point for the Spark submit job
//! run" and a job run is "a unit of work … that you submit to an Amazon EMR Serverless
//! application" (`JobRun`), so a run EXECUTES that entry point with its
//! `entryPointArguments` — and the state the run comes to rest in is what that execution came
//! to: `SUCCESS` for an application that ran to its end, the modelled failure state for one
//! that raised or that nothing could run. The objects the application wrote are then real
//! objects of the account's object store, so `GetObject` on a key it wrote answers HTTP 200
//! with the object's own bytes and the object metadata in headers — `ETag`, `Last-Modified`,
//! `Accept-Ranges: bytes`, the `Content-Type` the write stored, the server-side-encryption
//! algorithm of the bucket's default encryption, and the checksum headers for a read that asks
//! for them — and answers 404 `NoSuchKey` only where no such key exists.**
//!
//! Only a cross-service suite can pin this: EMR Serverless runs the application, IAM holds the
//! execution role the run is performed as, and every object — the application's own source and
//! everything it writes — is a real object of Amazon S3's own store, put there with `PutObject`
//! and read back with `GetObject`, never through anything this suite handed EMR Serverless. It
//! therefore runs against the WHOLE `build_router` service set, as the binary serves it.
//!
//! The rows differ along what the rule generalises over: HOW the application writes (the AWS
//! SDK it finds on its path, Spark's own text writer), WHERE it writes (a key at the bucket's
//! root, a key under a nested prefix, a prefix it is given on its command line), WHAT it
//! computes (an aggregate over inputs of different sizes), whether it declares a media type for
//! what it wrote, and how a run FAILS (an application that raises, one that exits non-zero, one
//! whose entry point the store holds nothing at, and a driver no engine here runs).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for: the one whose `CreateBucket` needs no
/// location constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every application of this suite is fetched from and writes into.
const BUCKET: &str = "emr-serverless-job-run-output";

/// The execution role every submit names — "The execution role ARN for the job run"
/// (`StartJobRunRequest$executionRoleArn`).
const EXECUTION_ROLE: &str = "EMRServerlessJobRunRole";

/// The service principal EMR Serverless assumes a job run's execution role as.
const EMR_SERVERLESS_PRINCIPAL: &str = "emr-serverless.amazonaws.com";

/// The Amazon EMR release the application of this suite runs on.
const RELEASE_LABEL: &str = "emr-7.1.0";

/// The `JobRunState` values a run never leaves, spelled out here rather than imported from the
/// service crate, so a wrong catalog in `src/` cannot make this suite pass. Source: the
/// `JobRunState` enum (`SUBMITTED`, `PENDING`, `SCHEDULED`, `RUNNING`, `SUCCESS`, `FAILED`,
/// `CANCELLING`, `CANCELLED`, `QUEUED`).
const TERMINAL_STATES: [&str; 3] = ["SUCCESS", "FAILED", "CANCELLED"];

/// The `Content-Type` Amazon S3 reports for an object stored without one, as the Amazon S3 User
/// Guide gives it.
const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// The one value `Accept-Ranges` carries on an object read.
const ACCEPT_RANGES: &str = "bytes";

/// The server-side encryption a general purpose bucket's default encryption applies: "all
/// buckets have a default encryption configuration that uses server-side encryption with Amazon
/// S3 managed keys (SSE-S3)" (`GetBucketEncryption`), whose `ServerSideEncryption` value is
/// `AES256`.
const DEFAULT_SSE: &str = "AES256";

/// The checksum type a single write's checksum carries: "the checksum type is always
/// `FULL_OBJECT`" (`PutObjectOutput$ChecksumType`).
const FULL_OBJECT: &str = "FULL_OBJECT";

/// How long a poll waits for a job run to come to REST: the stages the clock owes it, the
/// execution of its own application and the span the state that execution answered falls due
/// after, with room to spare. The wait is on the CONDITION — it looks again until the run has
/// come to rest — so a loaded host delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: the router above runs with authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One EMR Serverless call (`restJson1`, REST over `/applications/...`): its status and body.
async fn emr_serverless(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("emr-serverless.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("emr-serverless"))
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same, for a call that must be served.
async fn emr_serverless_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = emr_serverless(router, method, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {method} {path}, got {status} with {parsed}"
    );
    parsed
}

/// One IAM `awsQuery` call, which must answer 200.
async fn iam(router: &MultiServiceRouter, case: &str, body: &str) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization("iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM, got {status} with {text}"
    );
}

/// What one Amazon S3 read answered: the status, the response headers by lower-case name, and
/// the body.
struct Read {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// One path-style Amazon S3 request, with the extra request headers `headers` on it.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Read {
    let host = format!("s3.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization("s3"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::copy_from_slice(body))
                .expect("the test request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    Read {
        status,
        headers,
        body: response.body().to_vec(),
    }
}

/// Put `body` at `key`, which must be served.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &[u8]) {
    let answered = s3(router, "PUT", &format!("/{BUCKET}/{key}"), &[], body).await;
    assert!(
        (200..300).contains(&answered.status),
        "case {case:?}: PutObject {key} must succeed, got {} {}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
}

/// `GetObject` on `key`, asking for the stored checksum as well — "To retrieve the checksum,
/// this mode must be enabled" (`GetObjectRequest$ChecksumMode`).
async fn get_object(router: &MultiServiceRouter, key: &str) -> Read {
    s3(
        router,
        "GET",
        &format!("/{BUCKET}/{key}"),
        &[("x-amz-checksum-mode", "ENABLED")],
        &[],
    )
    .await
}

/// The key of the one part file a dataset write left under the output directory `prefix`.
///
/// A part file is named after the write job that produced it — `part-NNNNN-<uuid>-cNNN` and
/// the suffix of the format the write named — so a row names the DIRECTORY and the key is read
/// out of the store's own listing.
async fn part_key(router: &MultiServiceRouter, case: &str, prefix: &str) -> String {
    let answered = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
        &[],
    )
    .await;
    let listing = String::from_utf8_lossy(&answered.body).to_string();
    assert_eq!(
        answered.status, 200,
        "case {case:?}: listing s3://{BUCKET}/{prefix} must succeed, got {} with {listing}",
        answered.status
    );
    let parts: Vec<String> = listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .filter(|key| {
            let basename = key.rsplit('/').next().unwrap_or_default();
            basename.starts_with("part-")
        })
        .collect();
    assert_eq!(
        parts.len(),
        1,
        "case {case:?}: expected one part file under {prefix}, got {listing}"
    );
    parts[0].clone()
}

/// A trust policy admitting `principal` and nothing else, in the form an `awsQuery` form body
/// carries it: its quotes and spaces percent-encoded.
fn trust_policy(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
    .replace('"', "%22")
    .replace(' ', "%20")
}

/// The identity policy the execution role carries: the object store actions a job run's own
/// reads and writes perform, on this suite's bucket and nothing else — the scope a client gives
/// an EMR Serverless execution role. In the form an `awsQuery` form body carries it.
fn execution_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[\"s3:GetObject\",\"s3:PutObject\",\"s3:ListBucket\"],\
         \"Resource\":[\"arn:aws:s3:::{BUCKET}\",\"arn:aws:s3:::{BUCKET}/*\"]}}]}}"
    )
    .replace('"', "%22")
    .replace(' ', "%20")
    .replace('[', "%5B")
    .replace(']', "%5D")
}

/// The bucket and the execution role every row shares, each created through the service that
/// owns it.
async fn seed(router: &MultiServiceRouter) {
    let case = "setup";
    let answered = s3(router, "PUT", &format!("/{BUCKET}"), &[], &[]).await;
    assert!(
        (200..300).contains(&answered.status),
        "case {case:?}: CreateBucket must succeed, got {} {}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    iam(
        router,
        case,
        &format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={EXECUTION_ROLE}\
             &AssumeRolePolicyDocument={}",
            trust_policy(EMR_SERVERLESS_PRINCIPAL)
        ),
    )
    .await;
    iam(
        router,
        case,
        &format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={EXECUTION_ROLE}\
             &PolicyName=job-run-output&PolicyDocument={}",
            execution_policy()
        ),
    )
    .await;
}

/// An application that computes an aggregate over its own data and writes the result as ONE
/// object, through the Hadoop filesystem Spark writes through.
const AGGREGATE_PROGRAM: &str = r#"
import sys

from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("sums").getOrCreate()
count = int(sys.argv[1])
destination = sys.argv[2]
totals = (
    spark.sparkContext.parallelize(range(count), 2)
    .map(lambda value: (value % 3, value))
    .reduceByKey(lambda left, right: left + right)
    .sortByKey()
    .collect()
)
text = "".join("%d,%d\n" % row for row in totals)
path = spark._jvm.org.apache.hadoop.fs.Path(destination)
out = path.getFileSystem(spark._jsc.hadoopConfiguration()).create(path, True)
out.write(bytearray(text.encode("utf-8")))
out.close()
spark.stop()
"#;

/// An application that writes through SPARK's own text writer, which lays its output out as one
/// part file per partition under the prefix it is given.
const TEXT_WRITER_PROGRAM: &str = r#"
import sys

from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("lines").getOrCreate()
count = int(sys.argv[1])
destination = sys.argv[2]
rows = ["row-%d" % index for index in range(count)]
spark.sparkContext.parallelize(rows, 1).saveAsTextFile(destination)
spark.stop()
"#;

/// An application that writes through the DATAFRAME writer, whose output is laid out the same
/// way.
const FRAME_WRITER_PROGRAM: &str = r#"
import sys

from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("frame").getOrCreate()
count = int(sys.argv[1])
destination = sys.argv[2]
rows = [(index, index * index) for index in range(count)]
frame = spark.createDataFrame(rows, ["value", "square"])
frame.coalesce(1).write.mode("overwrite").csv(destination)
spark.stop()
"#;

/// An application that RAISES, before writing anything.
const RAISING_PROGRAM: &str = r#"
from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("raises").getOrCreate()
values = spark.sparkContext.parallelize([1, 2, 3]).collect()
raise ValueError("the input partition is malformed")
"#;

/// An application that ENDS THE PROGRAM with a non-zero exit status, before writing anything.
const EXITING_PROGRAM: &str = r#"
import sys

sys.exit(3)
"#;

/// The CSV the aggregate program computes over `count` inputs, derived here the same way the
/// program derives it — so the assertion is on a COMPUTED result rather than on a value this
/// suite also wrote.
fn expected_aggregate(count: u32) -> String {
    let mut totals: BTreeMap<u32, u32> = BTreeMap::new();
    for index in 0..count {
        *totals.entry(index % 3).or_default() += index;
    }
    totals
        .iter()
        .map(|(group, total)| format!("{group},{total}\n"))
        .collect()
}

/// The lines the text-writer program writes, in the order it writes them.
fn expected_lines(count: u32) -> String {
    (0..count).map(|index| format!("row-{index}\n")).collect()
}

/// The rows the frame writer writes, as the CSV it renders them to.
fn expected_squares(count: u32) -> String {
    (0..count)
        .map(|index| format!("{index},{}\n", index * index))
        .collect()
}

/// What a row expects of its run.
enum Comes {
    /// `SUCCESS`, with the object at this key holding these bytes. A key ending in `/` names
    /// the output DIRECTORY of a dataset write instead, whose one part file is named after
    /// the write job that produced it ([`part_key`]).
    Success { key: String, body: String },
    /// The modelled failure state, whose `stateDetails` says this, and the key the store must
    /// then hold nothing at.
    Failed { says: &'static str, no_key: String },
}

/// One row: the application its submit names, the arguments it is given and what the run comes
/// to.
struct Row {
    case: &'static str,
    /// The application's own source, or `None` for a submit naming an object nothing ever put
    /// there.
    program: Option<&'static str>,
    /// `jobDriver`, as the submit carries it — a `sparkSubmit` unless the row names another
    /// driver.
    driver: Option<Value>,
    /// The arguments the Spark submit passes the application.
    arguments: Vec<String>,
    comes: Comes,
}

/// The key one row's application is put under.
fn program_key(ordinal: usize) -> String {
    format!("applications/{ordinal}/main.py")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an aggregate written as one object at the bucket's root",
            program: Some(AGGREGATE_PROGRAM),
            driver: None,
            arguments: vec!["12".to_string(), format!("s3://{BUCKET}/sums.csv")],
            comes: Comes::Success {
                key: "sums.csv".to_string(),
                body: expected_aggregate(12),
            },
        },
        Row {
            case: "a smaller aggregate written under a nested prefix the arguments name",
            program: Some(AGGREGATE_PROGRAM),
            driver: None,
            arguments: vec![
                "5".to_string(),
                format!("s3://{BUCKET}/out/run-2/sums.csv"),
            ],
            comes: Comes::Success {
                key: "out/run-2/sums.csv".to_string(),
                body: expected_aggregate(5),
            },
        },
        Row {
            case: "output written through Spark's own text writer, read back as its part file",
            program: Some(TEXT_WRITER_PROGRAM),
            driver: None,
            arguments: vec!["4".to_string(), format!("s3://{BUCKET}/out/run-3/lines")],
            comes: Comes::Success {
                key: "out/run-3/lines/part-00000".to_string(),
                body: expected_lines(4),
            },
        },
        Row {
            case: "output written through the dataframe writer, as the CSV it renders",
            program: Some(FRAME_WRITER_PROGRAM),
            driver: None,
            arguments: vec!["3".to_string(), format!("s3://{BUCKET}/out/run-4/squares")],
            comes: Comes::Success {
                key: "out/run-4/squares/".to_string(),
                body: expected_squares(3),
            },
        },
        Row {
            case: "an application that raises before writing anything",
            program: Some(RAISING_PROGRAM),
            driver: None,
            arguments: vec![
                "3".to_string(),
                format!("s3://{BUCKET}/out/run-5/sums.csv"),
            ],
            comes: Comes::Failed {
                says: "ValueError: the input partition is malformed",
                no_key: "out/run-5/sums.csv".to_string(),
            },
        },
        Row {
            case: "an application that ends the program with a non-zero exit status",
            program: Some(EXITING_PROGRAM),
            driver: None,
            arguments: Vec::new(),
            comes: Comes::Failed {
                // The STATUS the program exited with is what failed the run: "the main
                // function must exit with a zero exit code" is the rule a submitted program's
                // run ends by.
                says: "exited with status",
                no_key: "out/run-6/sums.csv".to_string(),
            },
        },
        Row {
            case: "a submit whose entry point the object store holds nothing at",
            program: None,
            driver: None,
            arguments: Vec::new(),
            comes: Comes::Failed {
                says: "unable to download the application",
                no_key: "out/run-7/sums.csv".to_string(),
            },
        },
        Row {
            case: "a Hive driver, which no engine of this emulator runs",
            program: None,
            driver: Some(json!({ "hive": { "query": "SELECT 1" } })),
            arguments: Vec::new(),
            comes: Comes::Failed {
                says: "executes no Hive query",
                no_key: "out/run-8/sums.csv".to_string(),
            },
        },
    ]
}

/// An application reaching `STARTED`, which is what a job run is submitted to.
async fn started_application(router: &MultiServiceRouter, case: &str, token: &str) -> String {
    let created = emr_serverless_ok(
        router,
        case,
        "POST",
        "/applications",
        &json!({
            "clientToken": token,
            "releaseLabel": RELEASE_LABEL,
            "type": "SPARK",
            "autoStartConfiguration": { "enabled": true },
            "autoStopConfiguration": { "enabled": false },
        }),
    )
    .await;
    let application_id = created["applicationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateApplication must answer an applicationId, got {created}")
        })
        .to_string();
    emr_serverless_ok(
        router,
        case,
        "POST",
        &format!("/applications/{application_id}/start"),
        &json!({}),
    )
    .await;
    application_id
}

/// `GetJobRun`, as the `JobRun` it answers.
async fn job_run(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    job_run_id: &str,
) -> Value {
    emr_serverless_ok(
        router,
        case,
        "GET",
        &format!("/applications/{application_id}/jobruns/{job_run_id}"),
        &json!({}),
    )
    .await["jobRun"]
        .clone()
}

/// Poll every submitted run until each has come to REST, and answer them in submit order.
///
/// The wait is on the CONDITION — it looks again until every run has come to rest — and ONE
/// deadline covers them all, because they are in flight together.
async fn rested(
    router: &MultiServiceRouter,
    cases: &[&str],
    submitted: &[(String, String)],
) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    let mut runs: Vec<Option<Value>> = vec![None; submitted.len()];
    loop {
        let mut pending: Vec<String> = Vec::new();
        for (index, (application_id, job_run_id)) in submitted.iter().enumerate() {
            if runs[index].is_some() {
                continue;
            }
            let run = job_run(router, cases[index], application_id, job_run_id).await;
            let state = run["state"].as_str().unwrap_or_default().to_string();
            if TERMINAL_STATES.contains(&state.as_str()) {
                runs[index] = Some(run);
            } else {
                pending.push(format!("{:?} is {state:?}", cases[index]));
            }
        }
        if pending.is_empty() {
            return runs.into_iter().flatten().collect();
        }
        assert!(
            Instant::now() < deadline,
            "these job runs never came to rest: {pending:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every row: the run EXECUTES the application its submit named, and what that application
/// wrote is readable afterwards through `GetObject` with the object's metadata in headers.
#[tokio::test]
async fn a_job_run_executes_its_application_and_its_output_is_readable() {
    let router = router();
    let rows = rows();

    seed(&router).await;

    // Every row's application is staged and every run submitted before any is polled, so the
    // runs are in flight together rather than one after another.
    let mut submitted: Vec<(String, String)> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        if let Some(program) = row.program {
            put_object(&router, row.case, &program_key(ordinal), program.as_bytes()).await;
        }
        let application_id =
            started_application(&router, row.case, &format!("app-{ordinal}")).await;
        let driver = row.driver.clone().unwrap_or_else(|| {
            json!({
                "sparkSubmit": {
                    "entryPoint": format!("s3://{BUCKET}/{}", program_key(ordinal)),
                    "entryPointArguments": row.arguments,
                },
            })
        });
        let answered = emr_serverless_ok(
            &router,
            row.case,
            "POST",
            &format!("/applications/{application_id}/jobruns"),
            &json!({
                "clientToken": format!("run-{ordinal}"),
                "executionRoleArn":
                    format!("arn:aws:iam::000000000000:role/{EXECUTION_ROLE}"),
                "jobDriver": driver,
            }),
        )
        .await;
        let job_run_id = answered["jobRunId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartJobRun must answer a jobRunId, got {answered}",
                    row.case
                )
            })
            .to_string();
        submitted.push((application_id, job_run_id));
    }

    let cases: Vec<&str> = rows.iter().map(|row| row.case).collect();
    let runs = rested(&router, &cases, &submitted).await;

    for (row, run) in rows.iter().zip(&runs) {
        let case = row.case;
        let state = run["state"].as_str().unwrap_or_default();
        let details = run["stateDetails"].as_str().unwrap_or_default().to_string();
        match &row.comes {
            Comes::Success { key, body } => {
                assert_eq!(
                    state, "SUCCESS",
                    "case {case:?}: an application that ran to its end leaves the run in \
                     SUCCESS, got {state:?} with {details:?}"
                );
                // THE RULE's own instant: the read that finds the run rested is already past
                // its application, so what that application wrote is readable NOW.
                let key = match key.strip_suffix('/') {
                    Some(prefix) => part_key(&router, case, prefix).await,
                    None => key.clone(),
                };
                let key = key.as_str();
                let answered = get_object(&router, key).await;
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: GetObject on the key the application wrote answers 200, \
                     got {} with {:?}",
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
                assert_eq!(
                    String::from_utf8_lossy(&answered.body),
                    body.as_str(),
                    "case {case:?}: expected the bytes the application computed, got {:?}",
                    String::from_utf8_lossy(&answered.body)
                );
                // ... and the object's metadata, in the headers an object read carries it in.
                for (header, expected) in [
                    ("accept-ranges", ACCEPT_RANGES),
                    ("content-type", DEFAULT_CONTENT_TYPE),
                    ("x-amz-server-side-encryption", DEFAULT_SSE),
                    ("x-amz-checksum-type", FULL_OBJECT),
                ] {
                    let actual = answered.headers.get(header).map(String::as_str);
                    assert_eq!(
                        actual,
                        Some(expected),
                        "case {case:?}: expected {header}: {expected:?}, got {actual:?} \
                         ({:?})",
                        answered.headers
                    );
                }
                for header in ["etag", "last-modified", "x-amz-checksum-crc64nvme"] {
                    let actual = answered.headers.get(header).map(String::as_str);
                    assert!(
                        actual.is_some_and(|value| !value.is_empty()),
                        "case {case:?}: expected {header} on the read, got {actual:?} ({:?})",
                        answered.headers
                    );
                }
                assert_eq!(
                    answered.headers.get("content-length").map(String::as_str),
                    Some(body.len().to_string().as_str()),
                    "case {case:?}: the read reports the length of the bytes it serves, got \
                     {:?}",
                    answered.headers.get("content-length")
                );
                // The run reports the execution it performed: when it moved to RUNNING, when
                // it was terminated, and how long it took.
                for member in ["startedAt", "endedAt"] {
                    assert!(
                        !run[member].is_null(),
                        "case {case:?}: a run that executed reports {member}, got {run}"
                    );
                }
                assert!(
                    run["totalExecutionDurationSeconds"]
                        .as_i64()
                        .is_some_and(|seconds| seconds >= 1),
                    "case {case:?}: a run that executed reports the seconds it took, got {run}"
                );
            }
            Comes::Failed { says, no_key } => {
                assert_eq!(
                    state, "FAILED",
                    "case {case:?}: a run whose work did not succeed ends FAILED, got \
                     {state:?} with {details:?}"
                );
                assert!(
                    details.contains(says),
                    "case {case:?}: expected the state details to say {says:?}, got \
                     {details:?}"
                );
                // Nothing was written, so a read of the output key is the 404 Amazon S3
                // answers for a key that is not there.
                let answered = get_object(&router, no_key).await;
                assert_eq!(
                    answered.status,
                    404,
                    "case {case:?}: GetObject on a key nothing wrote answers 404, got {} \
                     with {:?}",
                    answered.status,
                    String::from_utf8_lossy(&answered.body)
                );
                assert!(
                    String::from_utf8_lossy(&answered.body).contains("NoSuchKey"),
                    "case {case:?}: the 404 is the NoSuchKey document, got {:?}",
                    String::from_utf8_lossy(&answered.body)
                );
            }
        }
    }
}
