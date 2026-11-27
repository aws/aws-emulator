// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an APACHE SPARK job run READS: the objects its source path
//! really holds, read as the role the job runs as.
//!
//! The rule under test: a job run executes its script in the environment that kind of job
//! has — the Spark engine and the AWS Glue ETL library on the import path — so what the run
//! reports is what that execution came to: `SUCCEEDED` with the objects the script's writes
//! really left behind, computed from the objects its reads really found, and the modelled
//! failure state carrying the engine's OWN error for a script that raised. Every read and
//! write the script makes is signed as the job's `Role`, so one the object store refuses that
//! role is refused; a SOURCE the role may not read resolves no file at that path, which is
//! the analysis failure Spark raises for a source that is not there, and a refused WRITE
//! reports the store's own refusal naming the object it was making.
//!
//! The rows span what the rule generalises over: WHICH engine surface the script computes
//! through (the dataset reader, the resilient distributed dataset, the AWS Glue dynamic
//! frame), HOW MUCH its source holds (one object, several objects, three rows, five rows — so
//! the value the run writes is the one its own data produces and not a constant) and WHAT the
//! role is allowed (the source read, the output write, a deny over another prefix, a source
//! that is not there).
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, the scripts and
//! the data they read are staged through Amazon S3's own `PutObject`, and the due instant a
//! run's terminal state falls at is moved THROUGH THE STORE rather than slept for, with every
//! run read back until it has come to rest ([`crate::job_runs::run_at_rest`]) — so no outcome
//! here depends on how fast the host runs.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{JOB_RUN_STATE_FAILED, JOB_RUN_STATE_SUCCEEDED};

use crate::crawler_role_trust::{glue_trust, seed_role};
use crate::job_run_execution_role::{emulator_with_iam_and_s3, grant_role, statement};
use crate::job_runs::run_at_rest;
use crate::job_script_execution::{
    create_bucket, emulator_with_s3, object_body, run_to_rest, stage_script, BUCKET, ROLE_ARN,
    SCRIPTS,
};
use crate::wire::ok;

/// A script that reads a CSV source through the dataset reader and writes the number of rows
/// it really held — the one value that cannot be produced without reading the source.
const COUNTS_ITS_CSV_SOURCE: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
source = spark.read.option("header", "true").csv(args["SOURCE_PATH"])
spark.sparkContext.parallelize([str(source.count())]).saveAsTextFile(args["OUTPUT_PATH"])
"#;

/// The same count, written through the DATASET writer out of a dataset coalesced to one
/// partition — the other way a Spark program lands one part file.
const COUNTS_THROUGH_THE_DATASET_WRITER: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
source = spark.read.option("header", "true").csv(args["SOURCE_PATH"])
counted = spark.createDataFrame([(str(source.count()),)], ["rows"])
counted.coalesce(1).write.mode("overwrite").csv(args["OUTPUT_PATH"])
"#;

/// A script that reads a TEXT source — every object under the path — and writes two values
/// computed from the lines it found.
const MEASURES_ITS_TEXT_SOURCE: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH"])
sc = SparkContext.getOrCreate()
lines = sc.textFile(args["SOURCE_PATH"])
longest = lines.map(len).max()
sc.parallelize(["%d lines, longest %d" % (lines.count(), longest)]).saveAsTextFile(
    args["OUTPUT_PATH"]
)
"#;

/// A Glue ETL script: the documented entry points of a Glue PySpark job — `GlueContext`, the
/// dynamic frame, a named transform and `Job` — reading a CSV source and writing JSON.
const GLUE_ETL_SCRIPT: &str = r#"
import sys

from awsglue.context import GlueContext
from awsglue.job import Job
from awsglue.transforms import ApplyMapping
from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext

args = getResolvedOptions(sys.argv, ["JOB_NAME", "SOURCE_PATH", "OUTPUT_PATH"])
glueContext = GlueContext(SparkContext.getOrCreate())
job = Job(glueContext)
job.init(args["JOB_NAME"], args)
source = glueContext.create_dynamic_frame.from_options(
    connection_type="s3",
    connection_options={"paths": [args["SOURCE_PATH"]]},
    format="csv",
    format_options={"withHeader": True},
)
mapped = ApplyMapping.apply(
    frame=source,
    mappings=[("id", "string", "id", "int"), ("name", "string", "who", "string")],
    transformation_ctx="mapped",
)
glueContext.write_dynamic_frame.from_options(
    frame=mapped,
    connection_type="s3",
    connection_options={"path": args["OUTPUT_PATH"]},
    format="json",
    transformation_ctx="sink",
)
job.commit()
"#;

/// A script that reaches the AWS Glue ETL library and the Spark session a Glue job script
/// resolves its own imports against, and computes over a dataset of its own: what the engine
/// being on the import path IS, with no source in the way.
const BUILDS_ITS_OWN_DATASET: &str = r#"
from awsglue.context import GlueContext
from awsglue.dynamicframe import DynamicFrame
from awsglue.job import Job
from awsglue.transforms import SelectFields
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

session = SparkSession(SparkContext.getOrCreate())
frame = session.createDataFrame([(1, "ada"), (2, "bo")], ["id", "name"])
if frame.count() != 2:
    raise ValueError("the dataset did not hold the rows it was built from")
if frame.select("name").collect()[1]["name"] != "bo":
    raise ValueError("the dataset did not hold the values it was built from")
glueContext = GlueContext(session.sparkContext)
job = Job(glueContext)
job.init("own-dataset", {})
kept = SelectFields.apply(
    frame=DynamicFrame.fromDF(frame, glueContext, "own"), paths=["name"]
)
if kept.toDF().columns != ["name"]:
    raise ValueError("the frame did not keep the field the transform names")
job.commit()
"#;

/// A script that reads a CSV source through the dataset reader with an explicit schema and
/// a separator of its own, and writes what it read back out.
const READS_ITS_SOURCE_WITH_A_SCHEMA: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
source = spark.read.schema(["region", "amount"]).option("sep", ";").csv(
    args["SOURCE_PATH"]
)
lines = [
    "%s=%s" % (row["region"], row["amount"]) for row in source.collect()
]
spark.sparkContext.parallelize(lines).saveAsTextFile(args["OUTPUT_PATH"])
"#;

/// The `Timeout` every job here carries, in minutes: the shortest the model admits, since the
/// span one script run may last is this emulator's own ceiling and far below it.
const TIMEOUT_MINUTES: i64 = 1;

/// The `CreateJob` body of a job whose script is at `location` and whose command is `command`.
fn job_body(name: &str, role: &str, command: &str, location: &str) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": role,
        "Command": { "Name": command, "ScriptLocation": location, "PythonVersion": "3" },
        "Timeout": TIMEOUT_MINUTES,
    });
    if command == "pythonshell" {
        // "The allowed values are 0.0625 or 1 DPU" for a Python shell job
        // (`CreateJob$MaxCapacity`).
        body["MaxCapacity"] = json!(0.0625);
    } else {
        body["GlueVersion"] = json!("4.0");
        body["WorkerType"] = json!("G.1X");
        body["NumberOfWorkers"] = json!(2);
    }
    body
}

/// Where one case's source objects live.
fn source_prefix(case: &str) -> String {
    format!("s3://{BUCKET}/data/{case}/")
}

/// Where one case's output objects live.
fn output_prefix(case: &str) -> String {
    format!("s3://{BUCKET}/output/{case}")
}

/// The body of the one object a run left under `output/<case>` that is not a committer's
/// marker — the part file its write landed — or `None` where the run left none at all.
///
/// A part file written through the DATASET writer is named after the write job that produced
/// it (`part-NNNNN-<uuid>-cNNN` and the suffix of the format), so the key is read out of the
/// store's own listing rather than spelled here.
async fn written_part(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> Option<String> {
    let found = output_objects(router, case).await;
    let parts: Vec<&(String, String)> = found
        .iter()
        .filter(|(key, _body)| {
            let basename = key.rsplit('/').next().unwrap_or_default();
            !basename.starts_with('_') && !basename.starts_with('.')
        })
        .collect();
    assert!(
        parts.len() <= 1,
        "case {case:?}: expected at most one part file under output/{case}/, got {found:?}"
    );
    let (key, body) = parts.first()?;
    let basename = key.rsplit('/').next().unwrap_or_default();
    assert!(
        basename.starts_with("part-"),
        "case {case:?}: expected a part file under output/{case}/, got {basename:?}"
    );
    Some(body.clone())
}

/// Every object one case's sink left under its output prefix, as (key, body) pairs — the
/// objects a reader of that output directory finds, whatever the sink named its parts.
async fn output_objects(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> Vec<(String, String)> {
    let client = crate::wire::s3_client(router);
    let listed = client
        .list_objects_v2()
        .bucket(BUCKET)
        .prefix(format!("output/{case}/"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the output must list, got {error:?}"));
    let mut found = Vec::new();
    for object in listed.contents() {
        let key = object.key().unwrap_or_default().to_string();
        let body = object_body(router, &key).await.unwrap_or_default();
        found.push((key, body));
    }
    found
}

/// One row of the engine table: a script, the objects its source path holds, and the body the
/// part file it writes must carry.
struct Row {
    /// What every assertion of the row reports.
    case: &'static str,
    /// The path segment this row's objects live under, which is also its job's name.
    label: &'static str,
    /// The script the job's run executes.
    script: &'static str,
    /// The objects staged under the row's source prefix, as (name, body).
    sources: Vec<(&'static str, &'static str)>,
    /// The body the one part file under the row's output prefix must carry.
    expected: &'static str,
}

/// A Spark job run's output is COMPUTED from the objects its source path holds: the same
/// script over different data writes different values, and a source spread over several
/// objects is read in full.
#[tokio::test]
async fn a_spark_job_runs_output_is_computed_from_the_objects_its_source_holds() {
    let rows = vec![
        Row {
            case: "a dataset read of a three-row CSV source",
            label: "csv-three-rows",
            script: COUNTS_ITS_CSV_SOURCE,
            sources: vec![("input.csv", "id,name\n1,ada\n2,bo\n3,cy\n")],
            expected: "3\n",
        },
        Row {
            case: "the same script over a five-row source",
            label: "csv-five-rows",
            script: COUNTS_ITS_CSV_SOURCE,
            sources: vec![("input.csv", "id,name\n1,a\n2,b\n3,c\n4,d\n5,e\n")],
            expected: "5\n",
        },
        Row {
            case: "a text source spread over two objects",
            label: "text-two-objects",
            script: MEASURES_ITS_TEXT_SOURCE,
            sources: vec![("part-a.txt", "alpha\nbe\n"), ("part-b.txt", "gamma12\n")],
            expected: "3 lines, longest 7\n",
        },
        Row {
            case: "a semicolon-separated source read under a schema of its own",
            label: "csv-schema",
            script: READS_ITS_SOURCE_WITH_A_SCHEMA,
            sources: vec![("input.csv", "west;3\neast;4\n")],
            expected: "west=3\neast=4\n",
        },
        Row {
            // The dataset WRITER's own path, over a dataset coalesced to one partition: a
            // write leaves one part file per partition, so this lands the same one object.
            case: "the count written through the dataset writer",
            label: "csv-coalesced",
            script: COUNTS_THROUGH_THE_DATASET_WRITER,
            sources: vec![("input.csv", "id,name\n1,ada\n2,bo\n3,cy\n")],
            expected: "3\n",
        },
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows {
        let case = row.case;
        let label = row.label;
        for (object, body) in &row.sources {
            stage_script(&router, &format!("data/{label}/{object}"), body).await;
        }
        let location = stage_script(&router, &format!("{SCRIPTS}{label}.py"), row.script).await;
        ok(
            &router,
            "CreateJob",
            &job_body(label, ROLE_ARN, "glueetl", &location),
        )
        .await;

        let run = run_to_rest(
            &router,
            &engine,
            label,
            &json!({
                "--SOURCE_PATH": source_prefix(label),
                "--OUTPUT_PATH": output_prefix(label),
            }),
        )
        .await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_SUCCEEDED),
            "case {case:?}: expected the run to come to rest in {JOB_RUN_STATE_SUCCEEDED}, \
             got {run}"
        );
        let written = written_part(&router, label).await;
        assert_eq!(
            written.as_deref(),
            Some(row.expected),
            "case {case:?}: expected the part file to carry {:?}, got {written:?}",
            row.expected
        );
        // A Glue job's own environment leaves NO success marker: what a run leaves under its
        // output path is the part files its write produced and nothing beside them, because
        // that environment's output committer does not mark a completed job.
        let marker = object_body(&router, &format!("output/{label}/_SUCCESS")).await;
        assert_eq!(
            marker, None,
            "case {case:?}: expected no marker object beside the output, got {marker:?}"
        );
    }
}

/// A Glue ETL script reaches the AWS Glue library every Glue Spark job environment ships: the
/// run executes it and the records it writes are the source's records under the names and
/// types its mapping states.
#[tokio::test]
async fn a_glue_etl_script_reads_and_writes_through_its_dynamic_frame() {
    let case = "glue-etl";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    stage_script(
        &router,
        &format!("data/{case}/input.csv"),
        "id,name\n1,ada\n2,bo\n",
    )
    .await;
    let location = stage_script(&router, &format!("{SCRIPTS}{case}.py"), GLUE_ETL_SCRIPT).await;
    ok(
        &router,
        "CreateJob",
        &job_body(case, ROLE_ARN, "glueetl", &location),
    )
    .await;

    let run = run_to_rest(
        &router,
        &engine,
        case,
        &json!({
            "--JOB_NAME": case,
            "--SOURCE_PATH": source_prefix(case),
            "--OUTPUT_PATH": output_prefix(case),
        }),
    )
    .await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: expected the run to come to rest in {JOB_RUN_STATE_SUCCEEDED}, got {run}"
    );

    // What the sink left under the output prefix: the records themselves, whatever the part
    // files are named, beside the marker a reader of that directory looks for.
    let written = output_objects(&router, case).await;
    let body: String = written
        .iter()
        .filter(|(key, _body)| !key.ends_with("_SUCCESS"))
        .map(|(_key, body)| body.clone())
        .collect();
    assert!(
        !body.trim().is_empty(),
        "case {case:?}: expected the sink's own objects under the output prefix, got {written:?}"
    );
    let records: Vec<Value> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("case {case:?}: {line:?} must be JSON: {error}"))
        })
        .collect();
    assert_eq!(
        records,
        vec![
            json!({ "id": 1, "who": "ada" }),
            json!({ "id": 2, "who": "bo" }),
        ],
        "case {case:?}: expected the source's records under the mapping's names and types, \
         got {body:?}"
    );
}

/// What the role of one row is allowed, and what the run of a script reading and writing
/// under it must come to.
struct RoleRow {
    /// What every assertion of the row reports, and the path segment its objects live under.
    case: &'static str,
    /// The inline policy statements in force on the row's role, as (effect, actions,
    /// resource) — the resource spelling `BUCKET` for this suite's own bucket.
    grants: Vec<(&'static str, &'static str, &'static str)>,
    /// Whether the source objects are staged at all.
    stages_source: bool,
    /// Whether the run must succeed and leave its output.
    succeeds: bool,
    /// What the failed run's `ErrorMessage` must carry, each of them.
    carries: Vec<&'static str>,
    /// Whether the path the refusal names is the row's OUTPUT prefix rather than its source.
    names_the_output: bool,
}

/// Every read and write a Spark script makes is signed as the job's role, so one the object
/// store refuses is refused — and the run reports it, naming the path, with no output left
/// behind. A SOURCE the role may not read is reported the way a source that is not there is:
/// the read resolves no file at that path either way. A refused WRITE names the object it was
/// making, so it reports the store's own refusal.
#[tokio::test]
async fn a_run_reports_the_refusal_of_a_source_or_output_its_role_may_not_reach() {
    let rows = [
        RoleRow {
            case: "allowed-both",
            // "s3:ListBucket" is what a read of a source DIRECTORY needs beside the object
            // reads: the objects under the prefix have to be listed before they can be read.
            grants: vec![
                ("Allow", "s3:GetObject", "*"),
                ("Allow", "s3:PutObject", "*"),
                ("Allow", "s3:ListBucket", "*"),
            ],
            stages_source: true,
            succeeds: true,
            carries: Vec::new(),
            names_the_output: false,
        },
        RoleRow {
            case: "denied-the-source",
            grants: vec![
                ("Allow", "s3:*", "*"),
                ("Deny", "s3:GetObject", "arn:aws:s3:::BUCKET/data/*"),
            ],
            stages_source: true,
            succeeds: false,
            // The read's own resolution of the source path is what the deny withholds, and a
            // path it resolved no file at is the analysis failure Spark raises for one that
            // is not there — the same report as the `a-source-that-is-not-there` row below.
            carries: vec!["Path does not exist"],
            names_the_output: false,
        },
        RoleRow {
            case: "denied-another-prefix",
            grants: vec![
                ("Allow", "s3:*", "*"),
                ("Deny", "s3:GetObject", "arn:aws:s3:::BUCKET/elsewhere/*"),
            ],
            stages_source: true,
            succeeds: true,
            carries: Vec::new(),
            names_the_output: false,
        },
        RoleRow {
            case: "denied-the-output",
            grants: vec![
                ("Allow", "s3:*", "*"),
                ("Deny", "s3:PutObject", "arn:aws:s3:::BUCKET/output/*"),
            ],
            stages_source: true,
            succeeds: false,
            carries: vec!["AccessDeniedException", "Access Denied", "403"],
            names_the_output: true,
        },
        RoleRow {
            case: "a-source-that-is-not-there",
            grants: vec![("Allow", "s3:*", "*")],
            stages_source: false,
            succeeds: false,
            carries: vec!["Path does not exist"],
            names_the_output: false,
        },
    ];


    let (router, engine) = emulator_with_iam_and_s3();
    create_bucket(&router).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let role_name = format!("spark-job-role-{index}");
        let role = seed_role(&engine, &role_name, Some(glue_trust()));
        let documents: Vec<Value> = row
            .grants
            .iter()
            .map(|&(effect, action, resource)| {
                // The script object itself stays readable for every row: what a row varies is
                // the SOURCE and the OUTPUT, not whether the script can be fetched, and a
                // deny spelled over the data or output prefix leaves the scripts prefix
                // alone.
                statement(effect, &[action], &resource.replace("BUCKET", BUCKET))
            })
            .collect();
        grant_role(&engine, &role_name, &documents, &[]);

        if row.stages_source {
            stage_script(
                &router,
                &format!("data/{case}/input.csv"),
                "id,name\n1,ada\n2,bo\n3,cy\n",
            )
            .await;
        }
        let name = format!("spark-role-{case}");
        let location = stage_script(&router, &format!("{SCRIPTS}{name}.py"), COUNTS_ITS_CSV_SOURCE)
            .await;
        ok(
            &router,
            "CreateJob",
            &job_body(&name, &role, "glueetl", &location),
        )
        .await;

        let started = ok(
            &router,
            "StartJobRun",
            &json!({
                "JobName": name,
                "Arguments": {
                    "--SOURCE_PATH": source_prefix(case),
                    "--OUTPUT_PATH": output_prefix(case),
                },
            }),
        )
        .await;
        let run_id = started["JobRunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: StartJobRun must answer a JobRunId"))
            .to_string();
        let run = run_at_rest(&router, &engine, case, &name, &run_id).await;
        let message = run["ErrorMessage"].as_str().unwrap_or_default().to_string();
        let written = written_part(&router, case).await;

        if row.succeeds {
            assert_eq!(
                run.get("JobRunState").and_then(Value::as_str),
                Some(JOB_RUN_STATE_SUCCEEDED),
                "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
            );
            assert_eq!(
                written.as_deref(),
                Some("3\n"),
                "case {case:?}: expected the count of the source's rows, got {written:?}"
            );
            continue;
        }
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_FAILED),
            "case {case:?}: expected {JOB_RUN_STATE_FAILED}, got {run}"
        );
        for held in &row.carries {
            assert!(
                message.contains(held),
                "case {case:?}: expected an ErrorMessage carrying {held:?}, got {message:?}"
            );
        }
        // The path the run could not reach is named, which is the one variable part of that
        // refusal.
        let named = if row.names_the_output {
            output_prefix(case)
        } else {
            source_prefix(case)
        };
        assert!(
            message.contains(&named),
            "case {case:?}: expected the ErrorMessage to name {named:?}, got {message:?}"
        );
        assert!(
            written.is_none(),
            "case {case:?}: a run that was refused wrote no output, got {written:?}"
        );
    }
}

/// The AWS Glue ETL library and the Spark session under it are on a job script's import path
/// whichever command the job carries, and a script that reaches them computes through them.
#[tokio::test]
async fn a_job_script_finds_the_glue_etl_library_and_the_spark_session() {
    // (case, the job's JobCommand$Name)
    let rows: [(&str, &str); 3] = [
        ("an Apache Spark ETL job", "glueetl"),
        ("an Apache Spark streaming job", "gluestreaming"),
        ("a Python shell job", "pythonshell"),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (case, command) in rows {
        let name = format!("spark-engine-{command}");
        let location =
            stage_script(&router, &format!("{SCRIPTS}{name}.py"), BUILDS_ITS_OWN_DATASET).await;
        ok(
            &router,
            "CreateJob",
            &job_body(&name, ROLE_ARN, command, &location),
        )
        .await;

        let run = run_to_rest(&router, &engine, &name, &json!({})).await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_SUCCEEDED),
            "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
        );
    }
}
