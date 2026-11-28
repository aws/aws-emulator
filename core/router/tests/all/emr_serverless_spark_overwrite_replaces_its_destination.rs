// SPDX-License-Identifier: CC0-1.0

//! What a Spark write's SAVE MODE makes of output that is ALREADY at the destination it writes,
//! driven end to end through an EMR Serverless job run on the assembled emulator.
//!
//! The RULE under test: **a write's save mode decides what an output already at the destination
//! means, and what is already there is the objects under that destination — whatever wrote them,
//! and whether or not anything registered a table over them. `overwrite` REPLACES them: the
//! objects the destination holds are deleted before the write commits its own, so a second
//! overwrite write of one output leaves exactly one copy of its rows; `append` leaves the new
//! parts beside the ones already there; `ignore` leaves the output as it stands; and the default
//! refuses the write. A `saveAsTable` resolves that against the TABLE: a table the catalog holds
//! is added to by an append and replaced by anything else, and a table the catalog holds none of
//! is created with this dataset as its data, so the location's own output is replaced.**
//!
//! That is why an overwrite job is idempotent: a run that failed after writing its files, and the
//! retry that then writes the same output again, leave ONE copy of the rows — which is what a
//! reader over the table the retry registered (Amazon Athena over the AWS Glue Data Catalog) then
//! counts and sums.
//!
//! Only a cross-service suite can pin this: EMR Serverless runs the program, IAM holds the roles
//! the two runs differ by, AWS Glue holds the catalog the table is registered in, Amazon S3 holds
//! the program, the input and every object the writes leave, CloudWatch Logs holds the driver log
//! each run's own report is read from, and Athena reads the table back over the same catalog and
//! the same objects. It therefore runs against the whole `build_router` service set, and every
//! assertion reads what one of those services persisted.
//!
//! The rows of the second suite differ along what the rule generalises over: the save mode
//! (`overwrite`, `append`, `ignore`, the explicit `errorifexists` and a write naming no mode at
//! all), the FORMAT written (Apache Parquet, delimited text, JSON documents) and the LAYOUT
//! (written flat, and partitioned by a column — including a partition the second write does not
//! write, which a static overwrite replaces along with the rest of the destination).

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for: the one whose `CreateBucket` needs no location
/// constraint in its body.
const REGION: &str = "us-east-1";

/// The bucket every program of this suite is fetched from, reads its input from and writes to.
const BUCKET: &str = "emr-serverless-overwrite-destination";

/// The service principal EMR Serverless assumes a job run's execution role as.
const EMR_SERVERLESS_PRINCIPAL: &str = "emr-serverless.amazonaws.com";

/// The Amazon EMR release the applications of this suite run on.
const RELEASE_LABEL: &str = "emr-7.9.0";

/// The `JobRunState` values a run never leaves. Source: the `JobRunState` enum (`SUBMITTED`,
/// `PENDING`, `SCHEDULED`, `RUNNING`, `SUCCESS`, `FAILED`, `CANCELLING`, `CANCELLED`, `QUEUED`).
const TERMINAL_STATES: [&str; 3] = ["SUCCESS", "FAILED", "CANCELLED"];

/// The worker type a Spark application's driver runs as, which is the key segment the run's log
/// stream is published under.
const SPARK_DRIVER: &str = "SPARK_DRIVER";

/// The log type the driver's standard error is delivered to CloudWatch Logs under. Source:
/// `CloudWatchLoggingConfiguration$logTypes`, whose valid values are `STDOUT`, `STDERR`,
/// `HIVE_LOG`, `TEZ_AM` and `SYSTEM_LOGS`.
const STDERR_LOG_TYPE: &str = "STDERR";

/// The CloudWatch Logs group EMR Serverless publishes a job run's logs to when the configuration
/// names none.
const DEFAULT_LOG_GROUP: &str = "/aws/emr-serverless";

/// How long a poll waits for work to reach the state it owes: the stages the clock owes a run,
/// the execution of its own program and the span the answer falls due after, with room to spare.
/// The wait is on the CONDITION, so a loaded host delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(180);

/// How long a poll sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The Glue catalog actions the Spark metastore client of a run that writes a partitioned table
/// makes. Source: the actions the Data Catalog publishes for those calls, which are what an EMR
/// Serverless execution role is scoped with.
const CATALOG_ACTIONS: [&str; 6] = [
    "glue:GetDatabase",
    "glue:GetTable",
    "glue:CreateTable",
    "glue:UpdateTable",
    "glue:BatchCreatePartition",
    "glue:GetPartitions",
];

/// The one catalog action the first run's role is missing, which is what fails that run AFTER its
/// program has written its output.
const WITHHELD_ACTION: &str = "glue:CreateTable";

/// The query states a query is still working in.
const PENDING_QUERY_STATES: [&str; 2] = ["QUEUED", "RUNNING"];

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

/// One `awsJson1_1` call: its status and body.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
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

/// One AWS Glue call.
async fn glue(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(router, "glue", "AWSGlue", operation, body).await
}

/// The same, for a call that must be served.
async fn glue_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = glue(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Glue {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon Athena call that must be served.
async fn athena_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = json_call(router, "athena", "AmazonAthena", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Athena {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One CloudWatch Logs call: its status and body.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    json_call(router, "logs", "Logs_20140328", operation, body).await
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

/// One path-style Amazon S3 request: its status and body.
async fn s3(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .body(Bytes::copy_from_slice(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

/// Hold this suite's bucket, which every object below lives in.
async fn given_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "setup: CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&body)
    );
}

/// Put `body` at `key`, which must be served.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, body: &[u8]) {
    let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: PutObject {key} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// The keys of every object under `prefix`, from `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let (status, body) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
    )
    .await;
    let listing = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: listing {prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// The part files under `prefix`: the objects a write leaves its rows in, as against the markers
/// an output committer leaves beside them.
async fn parts_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    keys_under(router, case, prefix)
        .await
        .into_iter()
        .filter(|key| {
            key.rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("part-"))
        })
        .collect()
}

/// One policy document in the form an `awsQuery` form body carries it.
fn form_encoded(document: &str) -> String {
    document
        .replace('%', "%25")
        .replace('"', "%22")
        .replace(' ', "%20")
        .replace('[', "%5B")
        .replace(']', "%5D")
        .replace('{', "%7B")
        .replace('}', "%7D")
        .replace('\n', "%0A")
        .replace('+', "%2B")
        .replace('&', "%26")
}

/// A trust policy admitting `principal` and nothing else.
fn trust_policy(principal: &str) -> String {
    form_encoded(&format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    ))
}

/// The identity policy a run's execution role carries: every object-store action a write of this
/// suite makes on its bucket — an overwrite DELETES what the destination holds, so the delete is
/// among them — and every catalog action the metastore client makes except `withheld`.
fn execution_policy(withheld: Option<&str>) -> String {
    let granted: Vec<String> = CATALOG_ACTIONS
        .iter()
        .filter(|action| Some(**action) != withheld)
        .map(|action| format!("\"{action}\""))
        .collect();
    form_encoded(&format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":[\"s3:GetObject\",\"s3:PutObject\",\
         \"s3:DeleteObject\",\"s3:ListBucket\"],\
         \"Resource\":[\"arn:aws:s3:::{BUCKET}\",\"arn:aws:s3:::{BUCKET}/*\"]}},\
         {{\"Effect\":\"Allow\",\"Action\":[{}],\"Resource\":\"*\"}}]}}",
        granted.join(",")
    ))
}

/// Create the role `name`, trusted by EMR Serverless and carrying [`execution_policy`], and
/// answer its ARN.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    withheld: Option<&str>,
) -> String {
    iam(
        router,
        case,
        &format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}&AssumeRolePolicyDocument={}",
            trust_policy(EMR_SERVERLESS_PRINCIPAL)
        ),
    )
    .await;
    iam(
        router,
        case,
        &format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}\
             &PolicyName=objects-and-catalog&PolicyDocument={}",
            execution_policy(withheld)
        ),
    )
    .await;
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// Create a SPARK application whose monitoring configuration publishes its runs' driver logs, and
/// answer its id.
async fn given_application(router: &MultiServiceRouter, case: &str, ordinal: &str) -> String {
    let answered = emr_serverless_ok(
        router,
        case,
        "POST",
        "/applications",
        &json!({
            "name": format!("overwrite-{ordinal}"),
            "clientToken": format!("application-{ordinal}"),
            "type": "SPARK",
            "releaseLabel": RELEASE_LABEL,
            "monitoringConfiguration": {
                "s3MonitoringConfiguration": { "logUri": format!("s3://{BUCKET}/logs/") },
                "cloudWatchLoggingConfiguration": { "enabled": true },
            },
        }),
    )
    .await;
    answered["applicationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateApplication must answer an id"))
        .to_string()
}

/// Submit `program_key` to `application_id` as `role`, and answer the run's id.
async fn started_run(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    token: &str,
    role: &str,
    program_key: &str,
    arguments: &[String],
) -> String {
    let started = emr_serverless_ok(
        router,
        case,
        "POST",
        &format!("/applications/{application_id}/jobruns"),
        &json!({
            "clientToken": token,
            "executionRoleArn": role,
            "jobDriver": {
                "sparkSubmit": {
                    "entryPoint": format!("s3://{BUCKET}/{program_key}"),
                    "entryPointArguments": arguments,
                },
            },
        }),
    )
    .await;
    started["jobRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: StartJobRun must answer a jobRunId"))
        .to_string()
}

/// Poll `GetJobRun` until the run reports a state it never leaves, and answer that read.
async fn settled_run(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    job_run_id: &str,
) -> Value {
    let path = format!("/applications/{application_id}/jobruns/{job_run_id}");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let answer = emr_serverless_ok(router, case, "GET", &path, &json!({})).await;
        let state = answer["jobRun"]["state"]
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
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll the run's CloudWatch Logs stream until its driver log is there, and answer that log as its
/// text: the events of the stream, one per line, which is the driver's own standard error.
async fn driver_log(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    job_run_id: &str,
) -> String {
    let stream = format!(
        "applications/{application_id}/jobs/{job_run_id}/{SPARK_DRIVER}/{STDERR_LOG_TYPE}"
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, answered) = logs(
            router,
            "GetLogEvents",
            &json!({
                "logGroupName": DEFAULT_LOG_GROUP,
                "logStreamName": stream,
                "startFromHead": true,
            }),
        )
        .await;
        let lines: Vec<String> = answered["events"]
            .as_array()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event["message"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if status == 200 && !lines.is_empty() {
            return lines.join("\n");
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run must deliver its driver log to {stream}, last read {status} \
             with {answered}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Run `sql` through Athena to completion and answer its data rows, each cell as its text.
async fn rows_of(
    router: &MultiServiceRouter,
    case: &str,
    database: &str,
    sql: &str,
) -> Vec<Vec<Option<String>>> {
    let started = athena_ok(
        router,
        case,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": database, "Catalog": "awsdatacatalog" },
            "ResultConfiguration": { "OutputLocation": format!("s3://{BUCKET}/athena/") },
        }),
    )
    .await;
    let id = started["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueryExecutionId, got {started}"))
        .to_string();
    let deadline = Instant::now() + DEADLINE;
    let status = loop {
        let reported = athena_ok(
            router,
            case,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        let status = reported["QueryExecution"]["Status"].clone();
        let state = status["State"].as_str().unwrap_or_default().to_string();
        if !PENDING_QUERY_STATES.contains(&state.as_str()) {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the query must reach a terminal state, last saw {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        status["State"].as_str(),
        Some("SUCCEEDED"),
        "case {case:?}: {sql} must succeed, got {status}"
    );
    let answered = athena_ok(
        router,
        case,
        "GetQueryResults",
        &json!({ "QueryExecutionId": id, "MaxResults": 1000 }),
    )
    .await;
    answered["ResultSet"]["Rows"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        // The first row of a DML result set's first page is the column labels.
        .skip(1)
        .map(|row| {
            row["Data"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|datum| {
                    datum["VarCharValue"]
                        .as_str()
                        .map(::std::string::ToString::to_string)
                })
                .collect()
        })
        .collect()
}

/// The program the first suite submits: it reads the seeded facts and writes them as a
/// PARTITIONED table of the catalog, with `mode("overwrite")` to a warehouse location it names
/// itself — the write a retry of a failed run makes again, unchanged.
const WAREHOUSE_PROGRAM: &str = r#"
import sys

from pyspark.sql import SparkSession

spark = (
    SparkSession.builder.appName("overwrite-warehouse").enableHiveSupport().getOrCreate()
)
source, location, name = sys.argv[1:4]
facts = (
    spark.read.option("header", "true")
    .schema("id string, dt string, amount bigint")
    .csv(source)
)
facts.write.mode("overwrite").format("parquet").partitionBy("dt").option(
    "path", location
).saveAsTable(name)
sys.stderr.write("driver: wrote %s\n" % (name,))
spark.stop()
"#;

/// How many rows the input holds, over how many `dt` values, and what their amounts sum to. The
/// values are the suite's own seed: what the rule says is that the table answers for them ONCE.
const SEEDED_ROWS: usize = 9;
const SEEDED_DAYS: usize = 3;
const SEEDED_TOTAL: i64 = 945;

/// The input the program reads: `SEEDED_ROWS` rows of `id,dt,amount` spread over `SEEDED_DAYS`
/// days, whose amounts sum to `SEEDED_TOTAL`.
fn facts_csv() -> String {
    let mut text = String::from("id,dt,amount\n");
    let mut total = 0_i64;
    for index in 0..SEEDED_ROWS {
        let amount = 101 + index as i64;
        total += amount;
        text.push_str(&format!(
            "id-{index},2026-04-{:02},{amount}\n",
            index % SEEDED_DAYS + 1
        ));
    }
    assert_eq!(
        total, SEEDED_TOTAL,
        "setup: the seeded amounts must sum to {SEEDED_TOTAL}, they sum to {total}"
    );
    text
}

/// RULE: an overwrite write replaces the output already at its destination, so a run that writes
/// the same output again — after an earlier run left its files there and then failed — leaves ONE
/// copy of the rows, and the table registered over that location counts and sums the input once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_overwrite_write_replaces_the_output_an_earlier_failed_run_left() {
    let case = "a retry of a run that failed after writing its output";
    let router = router();
    given_bucket(&router).await;

    let database = "overwrite_warehouse_db";
    let table = "facts";
    let source = "input/facts.csv";
    let program_key = "job/ddl.py";
    let location = format!("s3://{BUCKET}/warehouse/{table}");
    let prefix = format!("warehouse/{table}/");
    put_object(&router, case, source, facts_csv().as_bytes()).await;
    put_object(&router, case, program_key, WAREHOUSE_PROGRAM.as_bytes()).await;
    glue_ok(
        &router,
        case,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database, "LocationUri": format!("s3://{BUCKET}/warehouse") } }),
    )
    .await;

    // Two roles differing ONLY in the action the registration itself needs.
    let denied = given_role(&router, case, "OverwriteDeniedRole", Some(WITHHELD_ACTION)).await;
    let granted = given_role(&router, case, "OverwriteGrantedRole", None).await;
    let application_id = given_application(&router, case, "warehouse").await;
    let arguments = vec![
        format!("s3://{BUCKET}/{source}"),
        location.clone(),
        format!("{database}.{table}"),
    ];

    // The first run writes its output and is then refused the registration, so it ends FAILED and
    // the catalog holds no table — with its files left under the warehouse prefix.
    let first = started_run(
        &router,
        case,
        &application_id,
        "run-denied",
        &denied,
        program_key,
        &arguments,
    )
    .await;
    let answered = settled_run(&router, case, &application_id, &first).await;
    assert_eq!(
        answered["jobRun"]["state"].as_str(),
        Some("FAILED"),
        "case {case:?}: the run whose role may not register the table must fail, got {answered}"
    );
    let (status, refused) = glue(
        &router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": table }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the catalog must hold no table for a refused registration, got {status} \
         with {refused}"
    );
    let left = parts_under(&router, case, &prefix).await;
    assert_eq!(
        left.len(),
        SEEDED_DAYS,
        "case {case:?}: the failed run's own output must be under {prefix} before the retry, got \
         {left:?}"
    );

    // The retry — the same program and the same overwrite write, as a role that may register the
    // table — succeeds, and the table it registers is partitioned by the column the write named.
    let second = started_run(
        &router,
        case,
        &application_id,
        "run-granted",
        &granted,
        program_key,
        &arguments,
    )
    .await;
    let answered = settled_run(&router, case, &application_id, &second).await;
    let log = driver_log(&router, case, &application_id, &second).await;
    assert_eq!(
        answered["jobRun"]["state"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: the retry must succeed, got {answered} with the driver log {log:?}"
    );
    let registered = glue_ok(
        &router,
        case,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": table }),
    )
    .await;
    assert_eq!(
        registered["Table"]["PartitionKeys"],
        json!([{ "Name": "dt", "Type": "string" }]),
        "case {case:?}: expected the table to carry the write's partition key, got {registered}"
    );
    assert_eq!(
        registered["Table"]["StorageDescriptor"]["Location"].as_str(),
        Some(location.as_str()),
        "case {case:?}: expected the table to live at the location the write named, got \
         {registered}"
    );

    // The DATA under that location is the retry's alone: one part file per day, not two.
    let held = parts_under(&router, case, &prefix).await;
    assert_eq!(
        held.len(),
        SEEDED_DAYS,
        "case {case:?}: an overwrite write leaves one part file per partition it writes, got \
         {held:?}"
    );

    // ... which is what a reader over the table then counts and sums: the seeded input, once.
    let answered = rows_of(
        &router,
        case,
        database,
        &format!("SELECT count(*) AS c, sum(amount) AS s FROM \"{database}\".\"{table}\""),
    )
    .await;
    assert_eq!(
        answered,
        vec![vec![
            Some(SEEDED_ROWS.to_string()),
            Some(SEEDED_TOTAL.to_string()),
        ]],
        "case {case:?}: expected the table to answer for the seeded input once \
         ({SEEDED_ROWS}, {SEEDED_TOTAL}), got {answered:?}"
    );
}

/// The program the second suite submits: for each case it SEEDS the destination with one write and
/// then writes it again under the case's own save mode, reporting on its own standard error what
/// that second write did and how many rows the destination holds afterwards.
const SAVE_MODE_PROGRAM: &str = r#"
import json
import sys

from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("save-modes-on-a-path").getOrCreate()
root = sys.argv[1].rstrip("/")
cases = json.loads(sys.argv[2])


def frame(tag, count, groups):
    """A dataset of `count` rows tagged `tag`, spread over the groups `groups`."""
    rows = [(tag, index, groups[index % len(groups)]) for index in range(count)]
    return spark.createDataFrame(rows, "tag string, value int, g string").coalesce(1)


def write(held, case, mode, destination):
    writer = held.write.format(case["kind"])
    if mode:
        writer = writer.mode(mode)
    if case["partitioned"]:
        writer = writer.partitionBy("g")
    writer.save(destination)


for case in cases:
    destination = "%s/%s" % (root, case["label"])
    # The output the case's own write then finds already there, written by a first write of a
    # destination holding nothing — which every mode performs.
    write(frame("seed", case["seeded"], case["seed_groups"]), case, "", destination)
    outcome = "wrote"
    try:
        write(frame("again", case["wrote"], case["groups"]), case, case["mode"], destination)
    except BaseException as refused:
        # The class the write was refused by AND what it said, so a row asserts the refusal's
        # own wording rather than only that something raised.
        outcome = "refused %s: %s" % (type(refused).__name__, refused)
    read = spark.read.format(case["kind"]).load(destination)
    sys.stderr.write("driver: %s %s rows=%d\n" % (case["label"], outcome, read.count()))
spark.stop()
"#;

/// One row of the save-mode suite: how the destination is seeded, what the second write names and
/// holds, and what the destination must then be.
struct Row {
    /// The case, which is also the destination's own directory under the suite's root.
    label: &'static str,
    /// The format written.
    kind: &'static str,
    /// The save mode the second write names, or `""` for a write naming none.
    mode: &'static str,
    /// Whether both writes partition their output by the `g` column.
    partitioned: bool,
    /// How many rows the seeding write leaves, over which groups.
    seeded: usize,
    seed_groups: &'static [&'static str],
    /// How many rows the second write holds, over which groups.
    wrote: usize,
    groups: &'static [&'static str],
    /// Whether the second write is REFUSED by the mode it names, rather than performed.
    refused: bool,
    /// How many rows the destination holds afterwards, and how many part files.
    rows: usize,
    parts: usize,
    /// A path segment the destination must no longer hold afterwards, where the case has one.
    absent: Option<&'static str>,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "overwrite_parquet",
            kind: "parquet",
            mode: "overwrite",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 3,
            parts: 1,
            absent: None,
        },
        Row {
            label: "append_parquet",
            kind: "parquet",
            mode: "append",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 5,
            parts: 2,
            absent: None,
        },
        Row {
            label: "default_parquet",
            kind: "parquet",
            mode: "",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: true,
            rows: 2,
            parts: 1,
            absent: None,
        },
        Row {
            label: "errorifexists_parquet",
            kind: "parquet",
            mode: "errorifexists",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: true,
            rows: 2,
            parts: 1,
            absent: None,
        },
        Row {
            label: "ignore_parquet",
            kind: "parquet",
            mode: "ignore",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 2,
            parts: 1,
            absent: None,
        },
        Row {
            // A partitioned overwrite replaces the whole destination, the partitions this write
            // does not write among them: that is the default `spark.sql.sources
            // .partitionOverwriteMode`, `STATIC`.
            label: "overwrite_partitioned_parquet",
            kind: "parquet",
            mode: "overwrite",
            partitioned: true,
            seeded: 2,
            seed_groups: &["a", "b"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 3,
            parts: 1,
            absent: Some("g=b"),
        },
        Row {
            label: "overwrite_csv",
            kind: "csv",
            mode: "overwrite",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 3,
            parts: 1,
            absent: None,
        },
        Row {
            label: "overwrite_json",
            kind: "json",
            mode: "overwrite",
            partitioned: false,
            seeded: 2,
            seed_groups: &["a"],
            wrote: 3,
            groups: &["a"],
            refused: false,
            rows: 3,
            parts: 1,
            absent: None,
        },
    ]
}

/// RULE: a write's save mode decides what the output already at its destination means —
/// `overwrite` replaces it, `append` leaves this write's parts beside it, `ignore` leaves it as it
/// stands and the default refuses the write — whichever format is written and however the output
/// is laid out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_mode_decides_what_the_output_already_at_a_destination_means() {
    let case = "one destination per save mode, written twice";
    let router = router();
    given_bucket(&router).await;

    let program_key = "job/modes.py";
    put_object(&router, case, program_key, SAVE_MODE_PROGRAM.as_bytes()).await;
    let role = given_role(&router, case, "SaveModeOnAPathRole", None).await;
    let application_id = given_application(&router, case, "modes").await;
    let root = "modes";
    let rows = rows();
    let specification = Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "label": row.label,
                    "kind": row.kind,
                    "mode": row.mode,
                    "partitioned": row.partitioned,
                    "seeded": row.seeded,
                    "seed_groups": row.seed_groups,
                    "wrote": row.wrote,
                    "groups": row.groups,
                })
            })
            .collect(),
    );
    let job_run_id = started_run(
        &router,
        case,
        &application_id,
        "run-modes",
        &role,
        program_key,
        &[
            format!("s3://{BUCKET}/{root}"),
            specification.to_string(),
        ],
    )
    .await;
    let answered = settled_run(&router, case, &application_id, &job_run_id).await;
    let log = driver_log(&router, case, &application_id, &job_run_id).await;
    assert_eq!(
        answered["jobRun"]["state"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: the run must succeed, got {answered} with the driver log {log:?}"
    );

    for row in &rows {
        let label = row.label;
        // What the second write DID and what the destination then holds, read off the driver's own
        // report of its own read. A refused write is reported by the class that refused it AND the
        // sentence it carried, which names the destination the write was given.
        let outcome = if row.refused {
            format!(
                "refused AnalysisException: path s3://{BUCKET}/{root}/{label} already exists."
            )
        } else {
            "wrote".to_string()
        };
        let reported = format!("driver: {label} {outcome} rows={}", row.rows);
        assert!(
            log.contains(&reported),
            "case {case:?}/{label}: expected the driver log to carry {reported:?}, got {log:?}"
        );
        // ... and the OBJECTS under the destination, read through the object store's own listing.
        let prefix = format!("{root}/{label}/");
        let parts = parts_under(&router, case, &prefix).await;
        assert_eq!(
            parts.len(),
            row.parts,
            "case {case:?}/{label}: expected {} part file(s) under {prefix}, got {parts:?}",
            row.parts
        );
        if let Some(absent) = row.absent {
            assert!(
                !parts.iter().any(|key| key.contains(absent)),
                "case {case:?}/{label}: expected no {absent} output left under {prefix}, got \
                 {parts:?}"
            );
        }
    }
}
