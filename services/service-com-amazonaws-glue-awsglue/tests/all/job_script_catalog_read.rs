// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a Glue job run's script COMPUTES over a Data Catalog table: the
//! frame it reads `from_catalog`, and the Lake Formation governance of that read.
//!
//! The rule under test: a run of a job is one execution of the job's own script, so a script
//! that reads a catalog table reads THE TABLE THIS EMULATOR HOLDS — the metadata the Data
//! Catalog answers and the objects the store really serves at the location that metadata
//! names — and the run comes to rest in what that execution came to: `SUCCEEDED` with no
//! `ErrorMessage` for a script that ran to its end, and the modelled failure state carrying
//! the SCRIPT's own error for one that raised. The table's metadata is itself governed: a
//! principal holding no Lake Formation permission on the table is REFUSED it, so the run of
//! a job whose role holds none fails carrying that refusal and the same run succeeds once the
//! grant exists — the one write that changes the outcome.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness; the scripts, the
//! tables' data and everything a script writes are real objects, staged and read back through
//! Amazon S3's own API. The due instant a run's terminal state falls at is moved THROUGH THE
//! STORE rather than slept for and every run is read back until it has come to rest
//! ([`crate::job_runs::run_at_rest`]), so no outcome here depends on how fast the host runs.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: the layout
//! the table declares its data in (delimited text with and without a header line, another
//! delimiter, JSON, a binary layout this emulator cannot resolve), how much data there is
//! (none, one row, several), whether the table is partitioned, and what Lake Formation holds
//! for the role the run is performed as.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_STATE_FAILED, JOB_RUN_STATE_SUCCEEDED,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, InlinePolicy,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::crawler_role_trust::{emulator_with_iam, glue_trust, seed_role};
use crate::job_script_execution::{create_bucket, object_body, stage_script, BUCKET, SCRIPTS};
use crate::job_runs::run_at_rest;
use crate::wire::{ok, ACCOUNT_ID, REGION};

/// The `GlueVersion` every job here runs under, and the Spark version AWS publishes for it:
/// "Glue version determines the versions of Apache Spark and Python that Glue supports".
const GLUE_VERSION: &str = "4.0";
/// See [`GLUE_VERSION`].
const SPARK_VERSION: &str = "3.3.0";

/// The Lake Formation group every IAM user and role belongs to, spelled as Lake Formation
/// spells it on the wire. Source: `DataLakeSettings$CreateDatabaseDefaultPermissions`.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The ETL script every row runs: the documented entry points of the Glue ETL library — a
/// `GlueContext` over a `SparkContext`, a `Job` it initialises and commits, and a frame read
/// `from_catalog` — and it writes what it COMPUTED over that frame to an object a test reads
/// back.
const READS_THE_CATALOG_TABLE: &str = r#"
import json
import sys

import boto3
from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(
    sys.argv, ["JOB_NAME", "database", "table", "bucket", "output"]
)
spark_context = SparkContext()
glue_context = GlueContext(spark_context)
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_catalog(
    database=args["database"], table_name=args["table"]
)
report = {
    "job": args["JOB_NAME"],
    "rows": frame.count(),
    "columns": [field.name for field in frame.schema().fields],
    "spark": glue_context.spark_session.version,
    "as_a_dataframe": frame.toDF().count(),
    # Asking for more partitions than this process would ever hold costs it nothing and
    # changes none of the rows: an action is over the elements across the partitions.
    "repartitioned": frame.toDF().rdd.repartition(1000000000).count(),
    "records": sorted(json.dumps(record, sort_keys=True) for record in frame.records),
}
boto3.client("s3").put_object(
    Bucket=args["bucket"],
    Key=args["output"],
    Body=json.dumps(report, sort_keys=True).encode("utf-8"),
)
job.commit()
"#;

/// An emulator with Glue, IAM, Amazon S3 and Lake Formation on one shared engine, plus that
/// engine.
///
/// IAM is mounted because a run is performed as the job's role, which is resolved against
/// this account's own IAM; Amazon S3 because the script, the table's data and the script's
/// output are objects of the store; Lake Formation because it OWNS the permissions a catalog
/// read is judged against. STS is mounted with it, because the session a run's calls are
/// signed as is minted by the token service and resolved back to the role through it.
fn emulator_with_catalog_and_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = emulator_with_iam();
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lakeformation_awslakeformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Seed the role a run is performed as: one Glue can assume, allowed every Amazon S3 call,
/// so what decides a catalog read is the Lake Formation state and nothing else.
fn job_role(engine: &Arc<engine::InMemoryEngine>, role_name: &str) -> String {
    let arn = seed_role(engine, role_name, Some(glue_trust()));
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let mut policies: BTreeMap<String, InlinePolicy> = BTreeMap::new();
    let policy_name = format!("{role_name}-s3");
    policies.insert(
        name_key(&policy_name),
        InlinePolicy {
            policy_name,
            policy_document: json!({
                "Version": "2012-10-17",
                "Statement": [{ "Effect": "Allow", "Action": ["s3:*"], "Resource": "*" }],
            })
            .to_string(),
        },
    );
    store
        .role_inline_policies
        .insert(name_key(role_name), policies);
    arn
}

/// One Lake Formation call over the wire, asserting it was served.
///
/// Lake Formation speaks `restJson1` (`POST /<Operation>`), and the router routes on the
/// SigV4 credential scope's service field — which is what makes this harness differ from the
/// Glue one only in the address and that header.
async fn lake_formation(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.{REGION}.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/\
                 lakeformation/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the Lake Formation request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: Lake Formation {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// Stage one object of a table's data.
async fn stage_data(router: &MultiServiceRouter, key: &str, body: &str) {
    stage_script(router, key, body).await;
}

/// The columns every row's table declares, in declaration order, with the Hive types the
/// catalog holds them under (`Column$Type`).
fn columns() -> Value {
    json!([
        { "Name": "id", "Type": "int" },
        { "Name": "city", "Type": "string" },
        { "Name": "amount", "Type": "double" },
    ])
}

/// Create the database and the table one row reads, at the location `prefix` names.
async fn create_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    prefix: &str,
    parameters: Value,
    serde_parameters: Value,
    partition_keys: Value,
) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    let mut input = json!({
        "Name": table,
        "Parameters": parameters,
        "StorageDescriptor": {
            "Location": format!("s3://{BUCKET}/{prefix}"),
            "Columns": columns(),
            "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
            "SerdeInfo": {
                "SerializationLibrary":
                    "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe",
                "Parameters": serde_parameters,
            },
        },
    });
    if partition_keys.as_array().is_some_and(|keys| !keys.is_empty()) {
        input["PartitionKeys"] = partition_keys;
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": input }),
    )
    .await;
}

/// Create the job whose script reads `database`.`table` and writes its report to `output`.
async fn create_job(
    router: &MultiServiceRouter,
    name: &str,
    role: &str,
    location: &str,
    database: &str,
    table: &str,
    output: &str,
) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": role,
            "GlueVersion": GLUE_VERSION,
            "WorkerType": "G.1X",
            "NumberOfWorkers": 2,
            "Command": {
                "Name": "glueetl",
                "PythonVersion": "3",
                "ScriptLocation": location,
            },
            "DefaultArguments": {
                "--database": database,
                "--table": table,
                "--bucket": BUCKET,
                "--output": output,
            },
        }),
    )
    .await;
}

/// Start a run of `job_name` and answer the `JobRun` reported once it has come to rest.
async fn run_to_rest(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    job_name: &str,
) -> Value {
    let started = ok(router, "StartJobRun", &json!({ "JobName": job_name })).await;
    let run_id = started["JobRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: StartJobRun must answer a JobRunId"))
        .to_string();
    run_at_rest(router, engine, case, job_name, &run_id).await
}

/// The report the script wrote, parsed.
async fn report(router: &MultiServiceRouter, case: &str, output: &str) -> Value {
    let written = object_body(router, output)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the script must have written {output:?}"));
    serde_json::from_str(&written)
        .unwrap_or_else(|_| panic!("case {case:?}: the report is JSON, got {written:?}"))
}

/// One row of the layout table: a table declaring its data one way, the objects behind it,
/// and what the script must compute over the frame it reads.
struct Layout {
    /// The case every assertion of the row reports.
    case: &'static str,
    /// What the row is named after in the catalog, in the store and in the job.
    name: &'static str,
    /// `Table$Parameters` the table carries.
    parameters: Value,
    /// `SerdeInfo$Parameters` its storage descriptor carries.
    serde_parameters: Value,
    /// The objects staged under the table's location: `(name within the prefix, body)`.
    objects: Vec<(&'static str, String)>,
    /// How many records the frame must then hold.
    rows: usize,
    /// The records the frame must hold, as the script renders each, or an empty list for a
    /// row that only counts.
    records: Vec<String>,
}

/// The rows: one per layout a table declares its data in, and one per amount of data.
fn layouts() -> Vec<Layout> {
    let csv_with_header = "id,city,amount\n1,Berlin,10.5\n2,Lisbon,20\n3,Oslo,0.25\n";
    let csv_headerless = "7,Rome,1.5\n";
    let tab_separated = "4\tSofia\t3\n5\tRiga\t4.5\n";
    let json_lines = "{\"id\": 8, \"city\": \"Porto\", \"amount\": 9.5}\n\
                      {\"id\": 9, \"city\": \"Vienna\", \"amount\": 2}\n";
    vec![
        Layout {
            case: "delimited text whose first line is a header the table skips",
            name: "csv-header",
            parameters: json!({ "classification": "csv", "skip.header.line.count": "1" }),
            serde_parameters: json!({ "field.delim": "," }),
            objects: vec![("part-0.csv", csv_with_header.to_string())],
            rows: 3,
            records: vec![
                "{\"amount\": 10.5, \"city\": \"Berlin\", \"id\": 1}".to_string(),
                "{\"amount\": 0.25, \"city\": \"Oslo\", \"id\": 3}".to_string(),
                "{\"amount\": 20.0, \"city\": \"Lisbon\", \"id\": 2}".to_string(),
            ],
        },
        Layout {
            case: "delimited text with no header line at all, holding one row",
            name: "csv-headerless",
            parameters: json!({ "classification": "csv" }),
            serde_parameters: json!({ "field.delim": "," }),
            objects: vec![("part-0.csv", csv_headerless.to_string())],
            rows: 1,
            records: vec!["{\"amount\": 1.5, \"city\": \"Rome\", \"id\": 7}".to_string()],
        },
        Layout {
            case: "text delimited by something other than a comma",
            name: "tab-separated",
            parameters: json!({ "classification": "csv" }),
            serde_parameters: json!({ "field.delim": "\t" }),
            objects: vec![("part-0.tsv", tab_separated.to_string())],
            rows: 2,
            records: vec![
                "{\"amount\": 3.0, \"city\": \"Sofia\", \"id\": 4}".to_string(),
                "{\"amount\": 4.5, \"city\": \"Riga\", \"id\": 5}".to_string(),
            ],
        },
        Layout {
            case: "one JSON document per line",
            name: "json-lines",
            parameters: json!({ "classification": "json" }),
            serde_parameters: json!({}),
            objects: vec![("part-0.json", json_lines.to_string())],
            rows: 2,
            records: vec![
                "{\"amount\": 9.5, \"city\": \"Porto\", \"id\": 8}".to_string(),
                "{\"amount\": 2.0, \"city\": \"Vienna\", \"id\": 9}".to_string(),
            ],
        },
        Layout {
            case: "a table whose location holds no data object at all",
            name: "no-data",
            parameters: json!({ "classification": "csv", "skip.header.line.count": "1" }),
            serde_parameters: json!({ "field.delim": "," }),
            objects: Vec::new(),
            rows: 0,
            records: Vec::new(),
        },
        Layout {
            case: "data spread over two objects, beside the marker a writer left",
            name: "two-objects",
            parameters: json!({ "classification": "csv" }),
            serde_parameters: json!({ "field.delim": "," }),
            objects: vec![
                ("part-0.csv", csv_headerless.to_string()),
                ("part-1.csv", "6,Athens,8\n".to_string()),
                ("_SUCCESS", String::new()),
            ],
            rows: 2,
            records: vec![
                "{\"amount\": 1.5, \"city\": \"Rome\", \"id\": 7}".to_string(),
                "{\"amount\": 8.0, \"city\": \"Athens\", \"id\": 6}".to_string(),
            ],
        },
    ]
}

/// A script's frame holds the rows the table's own layout declares, and the run comes to rest
/// in what that execution came to.
#[tokio::test]
async fn a_frame_read_from_the_catalog_holds_the_rows_the_tables_layout_declares() {
    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "catalog-read-role");

    for layout in layouts() {
        let case = layout.case;
        let database = format!("db_{}", layout.name.replace('-', "_"));
        let table = format!("t_{}", layout.name.replace('-', "_"));
        let prefix = format!("data/{}/", layout.name);
        for (within, body) in &layout.objects {
            stage_data(&router, &format!("{prefix}{within}"), body).await;
        }
        create_table(
            &router,
            &database,
            &table,
            &prefix,
            layout.parameters.clone(),
            layout.serde_parameters.clone(),
            json!([]),
        )
        .await;

        let job_name = format!("job-{}", layout.name);
        let output = format!("reports/{}.json", layout.name);
        let location = stage_script(
            &router,
            &format!("{SCRIPTS}{}.py", layout.name),
            READS_THE_CATALOG_TABLE,
        )
        .await;
        create_job(
            &router, &job_name, &role, &location, &database, &table, &output,
        )
        .await;

        let run = run_to_rest(&router, &engine, case, &job_name).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_SUCCEEDED),
            "case {case:?}: the script ran to its end, so the run succeeds, got {run}"
        );
        assert!(
            run.get("ErrorMessage").is_none(),
            "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {run}"
        );

        let read = report(&router, case, &output).await;
        assert_eq!(
            read.get("rows").and_then(Value::as_u64),
            Some(layout.rows as u64),
            "case {case:?}: expected {} records in the frame, got {read}",
            layout.rows
        );
        assert_eq!(
            read.get("as_a_dataframe").and_then(Value::as_u64),
            Some(layout.rows as u64),
            "case {case:?}: the same frame as a Spark dataset holds the same rows, got {read}"
        );
        assert_eq!(
            read.get("repartitioned").and_then(Value::as_u64),
            Some(layout.rows as u64),
            "case {case:?}: a dataset asked for more partitions than it has elements holds \
             the same rows, got {read}"
        );
        assert_eq!(
            read.get("columns"),
            Some(&json!(["id", "city", "amount"])),
            "case {case:?}: the frame's schema is the table's columns, got {read}"
        );
        assert_eq!(
            read.get("spark").and_then(Value::as_str),
            Some(SPARK_VERSION),
            "case {case:?}: a Glue {GLUE_VERSION} job runs on Spark {SPARK_VERSION}, got {read}"
        );
        assert_eq!(
            read.get("job").and_then(Value::as_str),
            Some(job_name.as_str()),
            "case {case:?}: the service supplies the job's own name to its script, got {read}"
        );
        if !layout.records.is_empty() {
            let mut expected = layout.records.clone();
            expected.sort();
            assert_eq!(
                read.get("records"),
                Some(&json!(expected)),
                "case {case:?}: expected the table's own rows, typed as it declares them, got \
                 {read}"
            );
        }
    }
}

/// A frame read from a PARTITIONED table holds every partition's rows, each carrying the
/// values of the partition it was read from.
#[tokio::test]
async fn a_frame_read_from_a_partitioned_table_holds_every_partitions_rows() {
    let case = "a table partitioned by day";
    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "partitioned-read-role");

    let database = "db_partitioned";
    let table = "t_partitioned";
    let prefix = "data/partitioned/";
    stage_data(
        &router,
        &format!("{prefix}dt=2026-01-01/part-0.csv"),
        "1,Berlin,10.5\n2,Lisbon,20\n",
    )
    .await;
    stage_data(
        &router,
        &format!("{prefix}dt=2026-01-02/part-0.csv"),
        "3,Oslo,0.25\n",
    )
    .await;
    create_table(
        &router,
        database,
        table,
        prefix,
        json!({ "classification": "csv" }),
        json!({ "field.delim": "," }),
        json!([{ "Name": "dt", "Type": "string" }]),
    )
    .await;
    for day in ["2026-01-01", "2026-01-02"] {
        ok(
            &router,
            "CreatePartition",
            &json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInput": {
                    "Values": [day],
                    "StorageDescriptor": {
                        "Location": format!("s3://{BUCKET}/{prefix}dt={day}/"),
                        "Columns": columns(),
                        "SerdeInfo": {
                            "SerializationLibrary":
                                "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe",
                            "Parameters": { "field.delim": "," },
                        },
                    },
                },
            }),
        )
        .await;
    }

    let output = "reports/partitioned.json";
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}partitioned.py"),
        READS_THE_CATALOG_TABLE,
    )
    .await;
    create_job(
        &router,
        "job-partitioned",
        &role,
        &location,
        database,
        table,
        output,
    )
    .await;

    let run = run_to_rest(&router, &engine, case, "job-partitioned").await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: the script ran to its end, so the run succeeds, got {run}"
    );
    let read = report(&router, case, output).await;
    assert_eq!(
        read.get("rows").and_then(Value::as_u64),
        Some(3),
        "case {case:?}: the frame holds both partitions' rows, got {read}"
    );
    assert_eq!(
        read.get("columns"),
        Some(&json!(["id", "city", "amount", "dt"])),
        "case {case:?}: a partitioned table's frame carries its partition keys too, got {read}"
    );
    let records = read
        .get("records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rendered: Vec<&str> = records.iter().filter_map(Value::as_str).collect();
    assert!(
        rendered
            .iter()
            .any(|record| record.contains("\"dt\": \"2026-01-01\"") && record.contains("Berlin"))
            && rendered
                .iter()
                .any(|record| record.contains("\"dt\": \"2026-01-02\"")
                    && record.contains("Oslo")),
        "case {case:?}: each record carries the values of its own partition, got {rendered:?}"
    );
}

/// A table whose data layout this emulator cannot resolve into rows fails the run carrying
/// the SCRIPT's own error naming that layout — nothing is reported as read.
#[tokio::test]
async fn a_table_in_a_layout_that_cannot_be_resolved_fails_the_run_naming_it() {
    let case = "a table whose data is Parquet";
    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "binary-layout-role");

    let prefix = "data/parquet/";
    stage_data(&router, &format!("{prefix}part-0.parquet"), "PAR1").await;
    create_table(
        &router,
        "db_parquet",
        "t_parquet",
        prefix,
        json!({ "classification": "parquet" }),
        json!({}),
        json!([]),
    )
    .await;
    let output = "reports/parquet.json";
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}parquet.py"),
        READS_THE_CATALOG_TABLE,
    )
    .await;
    create_job(
        &router,
        "job-parquet",
        &role,
        &location,
        "db_parquet",
        "t_parquet",
        output,
    )
    .await;

    let run = run_to_rest(&router, &engine, case, "job-parquet").await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_FAILED),
        "case {case:?}: the script raised, so the run fails, got {run}"
    );
    let message = run
        .get("ErrorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("parquet"),
        "case {case:?}: expected the script's own error naming the layout, got {message:?}"
    );
    assert!(
        object_body(&router, output).await.is_none(),
        "case {case:?}: nothing was read, so the script wrote no report"
    );
}

/// A frame WRITTEN out leaves real objects behind, and a sink told to update the catalog
/// registers the partitions it wrote.
#[tokio::test]
async fn a_frame_written_out_leaves_real_objects_and_registers_what_it_wrote() {
    /// The script: it reads one table, maps its fields, writes the frame to a path of its
    /// own, and writes it again through a sink that partitions the output and updates the
    /// catalog.
    const WRITES_THE_FRAME_OUT: &str = r#"
import sys

from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.transforms import ApplyMapping
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(
    sys.argv,
    [
        "JOB_NAME",
        "database",
        "table",
        "flat_path",
        "partitioned_path",
        "sink_table",
    ],
)
glue_context = GlueContext(SparkContext())
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_catalog(
    database=args["database"], table_name=args["table"]
)
# The source fields named in another case than the records carry them, which is how the
# library matches them unless a mapping asks for the name as written.
mapped = ApplyMapping.apply(
    frame=frame,
    mappings=[
        ("ID", "int", "id", "int"),
        ("CITY", "string", "city", "string"),
        ("Amount", "double", "amount", "double"),
    ],
)
glue_context.write_dynamic_frame.from_options(
    frame=mapped,
    connection_type="s3",
    connection_options={"path": args["flat_path"]},
    format="csv",
)
sink = glue_context.getSink(
    path=args["partitioned_path"],
    connection_type="s3",
    partitionKeys=["city"],
    enableUpdateCatalog=True,
)
sink.setCatalogInfo(
    catalogDatabase=args["database"], catalogTableName=args["sink_table"]
)
sink.setFormat("json")
sink.writeFrame(mapped)
job.commit()
"#;

    let case = "a frame written out";
    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "frame-write-role");

    let database = "db_written";
    let table = "t_source";
    let sink_table = "t_written";
    let prefix = "data/written/";
    stage_data(
        &router,
        &format!("{prefix}part-0.csv"),
        "1,Berlin,10.5\n2,Lisbon,20\n3,Berlin,1.5\n",
    )
    .await;
    create_table(
        &router,
        database,
        table,
        prefix,
        json!({ "classification": "csv" }),
        json!({ "field.delim": "," }),
        json!([]),
    )
    .await;
    // The table the sink writes into: partitioned by the key the sink partitions on, so the
    // partitions the write registers are partitions of a table the catalog holds.
    let partitioned_prefix = "written/partitioned/";
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": sink_table,
                "Parameters": { "classification": "json" },
                "PartitionKeys": [{ "Name": "city", "Type": "string" }],
                "StorageDescriptor": {
                    "Location": format!("s3://{BUCKET}/{partitioned_prefix}"),
                    "Columns": [
                        { "Name": "id", "Type": "int" },
                        { "Name": "amount", "Type": "double" },
                    ],
                },
            },
        }),
    )
    .await;

    let flat_prefix = "written/flat";
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}written.py"),
        WRITES_THE_FRAME_OUT,
    )
    .await;
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": "job-written",
            "Role": role,
            "GlueVersion": GLUE_VERSION,
            "WorkerType": "G.1X",
            "NumberOfWorkers": 2,
            "Command": {
                "Name": "glueetl",
                "PythonVersion": "3",
                "ScriptLocation": location,
            },
            "DefaultArguments": {
                "--database": database,
                "--table": table,
                "--sink_table": sink_table,
                "--flat_path": format!("s3://{BUCKET}/{flat_prefix}"),
                "--partitioned_path": format!("s3://{BUCKET}/{partitioned_prefix}"),
            },
        }),
    )
    .await;

    let run = run_to_rest(&router, &engine, case, "job-written").await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: the script ran to its end, so the run succeeds, got {run}"
    );

    // What the unpartitioned write left: one part file holding the frame's own records, as
    // the delimited text the write was asked for, beside the marker a finished write leaves.
    let written = object_body(&router, &format!("{flat_prefix}/part-00000.csv")).await;
    assert_eq!(
        written.as_deref(),
        Some("id,city,amount\n1,Berlin,10.5\n2,Lisbon,20.0\n3,Berlin,1.5\n"),
        "case {case:?}: expected the frame's own records as delimited text, got {written:?}"
    );
    assert_eq!(
        object_body(&router, &format!("{flat_prefix}/_SUCCESS")).await,
        Some(String::new()),
        "case {case:?}: a finished write leaves its marker beside the data"
    );

    // What the partitioned write left: one part file per value of the partition key, holding
    // the records of that partition and not the key itself.
    let berlin = object_body(
        &router,
        &format!("{partitioned_prefix}city=Berlin/part-00000.json"),
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the Berlin partition must have been written"));
    assert_eq!(
        berlin,
        "{\"id\": 1, \"amount\": 10.5}\n{\"id\": 3, \"amount\": 1.5}\n",
        "case {case:?}: expected the Berlin records, got {berlin:?}"
    );
    let lisbon = object_body(
        &router,
        &format!("{partitioned_prefix}city=Lisbon/part-00000.json"),
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the Lisbon partition must have been written"));
    assert_eq!(
        lisbon,
        "{\"id\": 2, \"amount\": 20.0}\n",
        "case {case:?}: expected the Lisbon record, got {lisbon:?}"
    );

    // And the catalog holds what the write registered: one partition per value written, at
    // the location its own objects are under.
    let listed = ok(
        &router,
        "GetPartitions",
        &json!({ "DatabaseName": database, "TableName": sink_table }),
    )
    .await;
    let mut registered: Vec<(String, String)> = listed["Partitions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: GetPartitions must answer a list, got {listed}"))
        .iter()
        .map(|partition| {
            (
                partition["Values"][0].as_str().unwrap_or_default().to_string(),
                partition["StorageDescriptor"]["Location"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    registered.sort();
    assert_eq!(
        registered,
        vec![
            (
                "Berlin".to_string(),
                format!("s3://{BUCKET}/{partitioned_prefix}city=Berlin")
            ),
            (
                "Lisbon".to_string(),
                format!("s3://{BUCKET}/{partitioned_prefix}city=Lisbon")
            ),
        ],
        "case {case:?}: expected the partitions the write registered, got {listed}"
    );
}

/// An option a read or a write is GIVEN that this library does not honour fails the run
/// naming it, rather than reading or writing as if the option had not been passed.
#[tokio::test]
async fn an_option_this_library_does_not_honour_fails_the_run_naming_it() {
    /// A read of the catalog table under an option the reader honours none of.
    const READS_THE_CATALOG_WITH_AN_OPTION: &str = r#"
import sys

import boto3
from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(sys.argv, ["JOB_NAME", "database", "table", "bucket", "output"])
glue_context = GlueContext(SparkContext())
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_catalog(
    database=args["database"],
    table_name=args["table"],
    additional_options={"useCatalogSchema": True},
)
boto3.client("s3").put_object(
    Bucket=args["bucket"], Key=args["output"], Body=str(frame.count()).encode("utf-8")
)
job.commit()
"#;

    /// A read of the objects under a path, PRE-FILTERED by a predicate the reader applies
    /// none of.
    const READS_A_PATH_WITH_A_PREDICATE: &str = r#"
import sys

import boto3
from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(sys.argv, ["JOB_NAME", "database", "table", "bucket", "output"])
glue_context = GlueContext(SparkContext())
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_options(
    connection_type="s3",
    connection_options={"paths": ["s3://%s/data/option/" % (args["bucket"],)]},
    format="csv",
    push_down_predicate="dt='2026-01-01'",
)
boto3.client("s3").put_object(
    Bucket=args["bucket"], Key=args["output"], Body=str(frame.count()).encode("utf-8")
)
job.commit()
"#;

    /// A write through a sink carrying an option the sink performs none of.
    const WRITES_WITH_AN_OPTION: &str = r#"
import sys

from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(sys.argv, ["JOB_NAME", "database", "table", "bucket", "output"])
glue_context = GlueContext(SparkContext())
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_catalog(
    database=args["database"], table_name=args["table"]
)
sink = glue_context.getSink(
    path="s3://%s/%s" % (args["bucket"], args["output"]),
    connection_type="s3",
    compression="gzip",
)
sink.setFormat("json")
sink.writeFrame(frame)
job.commit()
"#;

    // (case, the script, the option the run's error must name)
    let rows: [(&str, &str, &str); 3] = [
        (
            "a catalog read given an option the reader honours none of",
            READS_THE_CATALOG_WITH_AN_OPTION,
            "useCatalogSchema",
        ),
        (
            "a path read asked to pre-filter by a predicate",
            READS_A_PATH_WITH_A_PREDICATE,
            "dt='2026-01-01'",
        ),
        (
            "a write asked for a compression the sink does not apply",
            WRITES_WITH_AN_OPTION,
            "compression",
        ),
    ];

    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "read-option-role");
    let prefix = "data/option/";
    stage_data(&router, &format!("{prefix}part-0.csv"), "1,Berlin,10.5\n").await;
    create_table(
        &router,
        "db_option",
        "t_option",
        prefix,
        json!({ "classification": "csv" }),
        json!({ "field.delim": "," }),
        json!([]),
    )
    .await;

    for (index, (case, script, named)) in rows.into_iter().enumerate() {
        let job_name = format!("job-option-{index}");
        let output = format!("reports/option-{index}.json");
        let location = stage_script(
            &router,
            &format!("{SCRIPTS}option-{index}.py"),
            script,
        )
        .await;
        create_job(
            &router,
            &job_name,
            &role,
            &location,
            "db_option",
            "t_option",
            &output,
        )
        .await;

        let run = run_to_rest(&router, &engine, case, &job_name).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_FAILED),
            "case {case:?}: the call was refused, so the run fails, got {run}"
        );
        let message = run
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(named),
            "case {case:?}: expected the script's own error naming {named:?}, got {message:?}"
        );
        assert!(
            object_body(&router, &output).await.is_none(),
            "case {case:?}: nothing was performed, so the script wrote nothing"
        );
    }
}

/// `ResolveChoice` resolves the field its spec names — by casting it, or by projecting it to
/// one type — and a spec naming an action this library does not perform fails the run.
#[tokio::test]
async fn a_resolve_choice_casts_or_projects_the_field_it_names_and_refuses_what_it_cannot() {
    /// The script: it reads the table and resolves one field by the action its `--action`
    /// argument names, then writes what the resolved frame holds.
    const RESOLVES_A_CHOICE: &str = r#"
import json
import sys

import boto3
from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.transforms import ResolveChoice
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(
    sys.argv, ["JOB_NAME", "database", "table", "bucket", "output", "action"]
)
glue_context = GlueContext(SparkContext())
job = Job(glue_context)
job.init(args["JOB_NAME"], args)
frame = glue_context.create_dynamic_frame.from_catalog(
    database=args["database"], table_name=args["table"]
)
resolved = ResolveChoice.apply(frame=frame, specs=[("amount", args["action"])])
report = {
    "records": sorted(
        json.dumps(record, sort_keys=True) for record in resolved.records
    ),
    "schema": ["%s:%s" % (field.name, field.dataType) for field in resolved.schema().fields],
}
boto3.client("s3").put_object(
    Bucket=args["bucket"],
    Key=args["output"],
    Body=json.dumps(report, sort_keys=True).encode("utf-8"),
)
job.commit()
"#;

    /// What one row expects of the resolved frame: the records it holds and its schema, as
    /// the script renders each.
    type Resolved = (Value, Value);

    // (case, the action a spec names, the records the resolved frame must hold and its
    // schema — or `None` for an action this library does not perform, which fails the run)
    let rows: [(&str, &str, Option<Resolved>); 3] = [
        (
            "casting a field to another type",
            "cast:long",
            Some((
                json!([
                    "{\"amount\": 1, \"city\": \"Lisbon\", \"id\": 2}",
                    "{\"amount\": 10, \"city\": \"Berlin\", \"id\": 1}",
                ]),
                json!(["id:int", "city:string", "amount:long"]),
            )),
        ),
        (
            "projecting a field to a type none of its values is of",
            "project:string",
            Some((
                json!([
                    "{\"amount\": null, \"city\": \"Berlin\", \"id\": 1}",
                    "{\"amount\": null, \"city\": \"Lisbon\", \"id\": 2}",
                ]),
                json!(["id:int", "city:string", "amount:string"]),
            )),
        ),
        (
            "an action that restructures the frame, which this library does not perform",
            "make_cols",
            None,
        ),
    ];

    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role = job_role(&engine, "resolve-choice-role");
    let prefix = "data/resolve/";
    stage_data(
        &router,
        &format!("{prefix}part-0.csv"),
        "1,Berlin,10.5\n2,Lisbon,1.5\n",
    )
    .await;
    create_table(
        &router,
        "db_resolve",
        "t_resolve",
        prefix,
        json!({ "classification": "csv" }),
        json!({ "field.delim": "," }),
        json!([]),
    )
    .await;

    for (index, (case, action, expected)) in rows.into_iter().enumerate() {
        let job_name = format!("job-resolve-{index}");
        let output = format!("reports/resolve-{index}.json");
        let location = stage_script(
            &router,
            &format!("{SCRIPTS}resolve-{index}.py"),
            RESOLVES_A_CHOICE,
        )
        .await;
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": job_name,
                "Role": role,
                "GlueVersion": GLUE_VERSION,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 2,
                "Command": {
                    "Name": "glueetl",
                    "PythonVersion": "3",
                    "ScriptLocation": location,
                },
                "DefaultArguments": {
                    "--database": "db_resolve",
                    "--table": "t_resolve",
                    "--bucket": BUCKET,
                    "--output": output,
                    "--action": action,
                },
            }),
        )
        .await;

        let run = run_to_rest(&router, &engine, case, &job_name).await;
        match expected {
            Some((records, schema)) => {
                assert_eq!(
                    run.get("JobRunState").and_then(Value::as_str),
                    Some(JOB_RUN_STATE_SUCCEEDED),
                    "case {case:?}: the script ran to its end, so the run succeeds, got {run}"
                );
                let read = report(&router, case, &output).await;
                assert_eq!(
                    read.get("records"),
                    Some(&records),
                    "case {case:?}: expected the resolved records, got {read}"
                );
                assert_eq!(
                    read.get("schema"),
                    Some(&schema),
                    "case {case:?}: the resolved field carries the type it was resolved to, \
                     got {read}"
                );
            }
            None => {
                assert_eq!(
                    run.get("JobRunState").and_then(Value::as_str),
                    Some(JOB_RUN_STATE_FAILED),
                    "case {case:?}: the script raised, so the run fails, got {run}"
                );
                let message = run
                    .get("ErrorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    message.contains(action),
                    "case {case:?}: expected the script's own error naming the action, got \
                     {message:?}"
                );
                assert!(
                    object_body(&router, &output).await.is_none(),
                    "case {case:?}: the frame was not resolved, so nothing was written"
                );
            }
        }
    }
}

/// The one write that repairs the job: a run reading a GOVERNED table fails while the
/// job's role holds no Lake Formation permission on that table, and succeeds once the grant
/// exists.
#[tokio::test]
async fn a_governed_tables_frame_is_read_only_once_lake_formation_grants_it() {
    let case = "a Lake-Formation-governed table";
    let (router, engine) = emulator_with_catalog_and_s3();
    create_bucket(&router).await;
    let role_name = "governed-read-role";
    let role = job_role(&engine, role_name);
    let role_arn = format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}");
    assert_eq!(
        role, role_arn,
        "case {case:?}: the role a grant names is the one IAM holds"
    );

    let database = "db_governed";
    let table = "t_governed";
    let prefix = "data/governed/";
    stage_data(
        &router,
        &format!("{prefix}part-0.csv"),
        "id,city,amount\n1,Berlin,10.5\n2,Lisbon,20\n",
    )
    .await;
    create_table(
        &router,
        database,
        table,
        prefix,
        json!({ "classification": "csv", "skip.header.line.count": "1" }),
        json!({ "field.delim": "," }),
        json!([]),
    )
    .await;
    // The catalog is handed to Lake Formation: the grant the database and its table were
    // created carrying — `ALL` to the group every IAM principal belongs to — is revoked, so
    // what a principal holds is what Lake Formation granted it and nothing else.
    for resource in [
        json!({ "Database": { "Name": database } }),
        json!({ "Table": { "DatabaseName": database, "Name": table } }),
    ] {
        lake_formation(
            &router,
            case,
            "RevokePermissions",
            &json!({
                "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
                "Resource": resource,
                "Permissions": ["ALL"],
            }),
        )
        .await;
    }

    let output = "reports/governed.json";
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}governed.py"),
        READS_THE_CATALOG_TABLE,
    )
    .await;
    create_job(
        &router,
        "job-governed",
        &role,
        &location,
        database,
        table,
        output,
    )
    .await;

    let denied = run_to_rest(&router, &engine, case, "job-governed").await;
    assert_eq!(
        denied.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_FAILED),
        "case {case:?}: with no grant the catalog read is refused, so the run fails, got \
         {denied}"
    );
    let message = denied
        .get("ErrorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("Insufficient Lake Formation permission(s)") && message.contains(table),
        "case {case:?}: expected the Lake Formation refusal of the table, got {message:?}"
    );
    assert!(
        object_body(&router, output).await.is_none(),
        "case {case:?}: the read was refused, so the script wrote no report"
    );

    // The ONE write that repairs the job: the permissions a reader of the table needs,
    // granted to the role the run is performed as.
    lake_formation(
        &router,
        case,
        "GrantPermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": role_arn },
            "Resource": { "Table": { "DatabaseName": database, "Name": table } },
            "Permissions": ["SELECT", "DESCRIBE"],
        }),
    )
    .await;

    let granted = run_to_rest(&router, &engine, case, "job-governed").await;
    assert_eq!(
        granted.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: after the grant the same run reads the table and succeeds, got \
         {granted}"
    );
    assert!(
        granted.get("ErrorMessage").is_none(),
        "case {case:?}: a run that failed at nothing carries no ErrorMessage, got {granted}"
    );
    let read = report(&router, case, output).await;
    assert_eq!(
        read.get("rows").and_then(Value::as_u64),
        Some(2),
        "case {case:?}: the frame holds the governed table's own rows, got {read}"
    );
}
