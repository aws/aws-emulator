// SPDX-License-Identifier: CC0-1.0

//! A submitted Spark application's own SESSION runs statements, end to end over the wire.
//!
//! THE RULE every row below asserts: a Spark session resolves the table names a statement it is
//! given carries — in the text of `spark.sql` or in the name `spark.table` takes — in the
//! session's catalog, which on a cluster whose configuration selects the AWS Glue Data Catalog
//! as its Hive metastore is that catalog; it reads the table's data from the location the
//! catalog records for it and answers the dataset the statement computed over that data. A name
//! the catalog holds nothing under fails the statement in the driver with Spark's own analysis
//! failure, which fails the step carrying that error, and a statement of a class nothing here
//! runs fails it with the failure of the engine saying so — never an empty answer, which would
//! say the statement ran and found nothing.
//!
//! The rows differ along what the rule generalises over: the statement form (an aggregate, a
//! projection with a predicate, a whole-table read through `table`, a statement whose effect is
//! a write), how the name is qualified (`<database>.<table>`, bare after the session's current
//! database is set), and whether the catalog holds the name at all.
//!
//! Every row drives the real HTTP path through one emulator carrying Amazon EMR, the Glue Data
//! Catalog and Amazon S3 over one engine: the catalog is written through
//! `CreateDatabase`/`CreateTable`, the table's data and each application through Amazon S3's own
//! `PutObject`, each answer read back through `GetObject`, and a step only ever read through
//! `DescribeStep`. Nothing reaches into any service's state struct.

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    APPLICATION_FAILED_REASON, RESOLVABLE_WORK,
};

use crate::glue_metastore_steps::Pairing;
use crate::job_flow_lifecycle::at;
use crate::spark_step_application::s3;

/// The Glue database these rows' table lives in, and the table itself.
const DATABASE: &str = "spark_session_db";
const TABLE: &str = "takings";

/// A table name the catalog holds nothing under.
const ABSENT_TABLE: &str = "takings_absent";

/// The bucket the table's data, the applications and their answers live in.
const BUCKET: &str = "spark-session-sql";

/// The prefix the table's location names, and the object its rows live in.
const DATA_PREFIX: &str = "takings";
const DATA_OBJECT: &str = "takings/rows.csv";

/// The rows that object holds: the table declares `id bigint, region string, amount double`,
/// and the object is comma-separated data, which is what the table's `classification` records.
const DATA_ROWS: &str = "1,west,10.5\n2,east,4.25\n3,west,1.25\n";

/// What the test's OWN arithmetic says the aggregate over those rows is, so the expected answer
/// is this suite's rather than the emulator's.
fn expected_total() -> f64 {
    10.5 + 4.25 + 1.25
}

/// What a row expects its step to end as.
enum Ends {
    /// The application ran to its end: the step completed, and this is the answer it wrote.
    Completed(String),
    /// The statement was refused: the step failed with this reason, and its message carries
    /// every one of these fragments.
    Failed(&'static str, Vec<String>),
}

/// One row: the body its application runs, and the end its step must reach.
struct Case {
    label: &'static str,
    /// Python that binds `frame` to the dataset the statement answered.
    body: String,
    ends: Ends,
}

/// The application a row submits: it runs the row's own body and writes the dataset that body
/// bound, row per line and value per comma, through the Hadoop filesystem — so what the answer
/// holds is what the engine really computed, read back as a stored object.
fn application(body: &str) -> String {
    format!(
        r#"
import sys
from pyspark.sql import SparkSession

spark = SparkSession.builder.appName("session statements").enableHiveSupport().getOrCreate()
{body}
lines = []
for row in frame.collect():
    lines.append(",".join("" if value is None else str(value) for value in row))
text = "\n".join(lines)
path = spark._jvm.org.apache.hadoop.fs.Path(sys.argv[1])
out = path.getFileSystem(spark._jsc.hadoopConfiguration()).create(path, True)
out.write(bytearray(text.encode("utf-8")))
out.close()
spark.stop()
"#
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an aggregate over the registered table",
            body: format!(
                "frame = spark.sql(\"SELECT COUNT(*) AS held, SUM(amount) AS total FROM \
                 {DATABASE}.{TABLE}\")"
            ),
            // The total is written the way the program writes a number it was answered: a
            // float carries its fractional part, which is what `str` of one gives.
            ends: Ends::Completed(format!("3,{:?}", expected_total())),
        },
        Case {
            label: "a projection with a predicate over it",
            body: format!(
                "frame = spark.sql(\"SELECT id FROM {DATABASE}.{TABLE} WHERE region = 'west' \
                 ORDER BY id\")"
            ),
            ends: Ends::Completed("1\n3".to_string()),
        },
        Case {
            label: "the whole table read by name and ordered by the dataset itself",
            body: format!("frame = spark.table(\"{DATABASE}.{TABLE}\").orderBy(\"id\")"),
            ends: Ends::Completed("1,west,10.5\n2,east,4.25\n3,west,1.25".to_string()),
        },
        Case {
            label: "a bare name, resolved in the database the session was set to",
            body: format!(
                "spark.catalog.setCurrentDatabase(\"{DATABASE}\")\n\
                 frame = spark.sql(\"SELECT COUNT(*) AS held FROM {TABLE}\")"
            ),
            ends: Ends::Completed("3".to_string()),
        },
        Case {
            label: "a statement over a name the catalog holds nothing under",
            body: format!(
                "frame = spark.sql(\"SELECT COUNT(*) FROM {DATABASE}.{ABSENT_TABLE}\")"
            ),
            ends: Ends::Failed(
                APPLICATION_FAILED_REASON,
                vec![
                    "AnalysisException".to_string(),
                    format!("Table or view not found: {DATABASE}.{ABSENT_TABLE}"),
                ],
            ),
        },
        Case {
            label: "a statement whose effect is a write nothing here applies",
            body: format!(
                "frame = spark.sql(\"CREATE TABLE {DATABASE}.copied AS SELECT id FROM \
                 {DATABASE}.{TABLE}\")"
            ),
            ends: Ends::Failed(
                APPLICATION_FAILED_REASON,
                vec!["SparkException".to_string(), RESOLVABLE_WORK.to_string()],
            ),
        },
    ]
}

/// The key one row's application is stored under.
fn application_key(ordinal: usize) -> String {
    format!("applications/session-{ordinal}.py")
}

/// The key one row's answer is written to.
fn answer_key(ordinal: usize) -> String {
    format!("answers/session-{ordinal}.csv")
}

/// Every row: the statement its session submits is resolved in the cluster's Glue Data Catalog
/// metastore and computed over the data that catalog's table points at, and the step ends in
/// what that run answered.
#[tokio::test]
async fn a_sessions_statements_are_resolved_in_the_clusters_metastore() {
    let pairing = Pairing::new();
    let cases = cases();

    // The bucket, the table's own data and one application per row.
    for (case, uri, body) in [
        ("CreateBucket", format!("/{BUCKET}"), String::new()),
        (
            "PutObject",
            format!("/{BUCKET}/{DATA_OBJECT}"),
            DATA_ROWS.to_string(),
        ),
    ] {
        let (status, answered) = s3(pairing.router(), "PUT", &uri, body.as_bytes()).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: must succeed, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }
    for (ordinal, case) in cases.iter().enumerate() {
        let source = application(&case.body);
        let (status, answered) = s3(
            pairing.router(),
            "PUT",
            &format!("/{BUCKET}/{}", application_key(ordinal)),
            source.as_bytes(),
        )
        .await;
        assert!(
            (200..300).contains(&status),
            "case {:?}: the application must be stored, got {status} {}",
            case.label,
            String::from_utf8_lossy(&answered)
        );
    }

    // The catalog the sessions resolve their names in: one database with one table over the
    // location the data was just put at.
    pairing
        .glue(
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": DATABASE } }),
        )
        .await;
    pairing
        .glue(
            "CreateTable",
            json!({
                "DatabaseName": DATABASE,
                "TableInput": {
                    "Name": TABLE,
                    "Parameters": { "classification": "csv" },
                    "StorageDescriptor": {
                        "Location": format!("s3://{BUCKET}/{DATA_PREFIX}/"),
                        "Columns": [
                            { "Name": "id", "Type": "bigint" },
                            { "Name": "region", "Type": "string" },
                            { "Name": "amount", "Type": "double" },
                        ],
                    },
                },
            }),
        )
        .await;

    // One Glue-metastore cluster, with every row's step submitted at once so the suite waits
    // through one cluster bring-up rather than one per row.
    let cluster_id = pairing
        .glue_metastore_cluster("session statements", cases.len() as i32)
        .await;
    let steps: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(ordinal, case)| {
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
                        format!("s3://{BUCKET}/{}", answer_key(ordinal)),
                    ],
                },
            })
        })
        .collect();
    let step_ids = pairing.add_steps(&cluster_id, steps).await;

    for (ordinal, case) in cases.iter().enumerate() {
        let step = pairing
            .await_terminal_step(case.label, &cluster_id, &step_ids[ordinal])
            .await;
        let state = at(&step, &["Status", "State"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        let failure = at(&step, &["Status", "FailureDetails"]).clone();
        match &case.ends {
            Ends::Completed(expected) => {
                assert_eq!(
                    state, "COMPLETED",
                    "case {:?}: expected COMPLETED, got {state:?} with {failure}",
                    case.label
                );
                let (status, answered) = s3(
                    pairing.router(),
                    "GET",
                    &format!("/{BUCKET}/{}", answer_key(ordinal)),
                    &[],
                )
                .await;
                assert!(
                    (200..300).contains(&status),
                    "case {:?}: the answer the application wrote must be stored, got {status}",
                    case.label
                );
                let written = String::from_utf8_lossy(&answered).to_string();
                assert_eq!(
                    &written, expected,
                    "case {:?}: expected the answer {expected:?}, got {written:?}",
                    case.label
                );
            }
            Ends::Failed(reason, fragments) => {
                assert_eq!(
                    state, "FAILED",
                    "case {:?}: expected FAILED, got {state:?}",
                    case.label
                );
                let answered_reason = failure["Reason"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    &answered_reason, reason,
                    "case {:?}: expected the reason {reason:?}, got {answered_reason:?}",
                    case.label
                );
                let message = failure["Message"].as_str().unwrap_or_default().to_string();
                for fragment in fragments {
                    assert!(
                        message.contains(fragment.as_str()),
                        "case {:?}: expected the message to carry {fragment:?}, got {message:?}",
                        case.label
                    );
                }
            }
        }
    }
}
