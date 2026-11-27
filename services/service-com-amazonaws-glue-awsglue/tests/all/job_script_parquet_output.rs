// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the APACHE PARQUET output a Glue job run's write leaves in Amazon S3.
//!
//! The rule under test: a Spark job run that writes its dataset in the columnar format leaves
//! Apache Parquet objects — the dataset's own values in each file's pages, and the row count,
//! the schema and every column's bounds in its footer — laid out Hive-style under the
//! partition columns the write names, with the part file carrying the suffix of the codec its
//! pages are stored under. A dataset written that way reads back as the dataset that was
//! written, partition columns and all.
//!
//! So every assertion here judges the objects with a PARQUET READER (the `parquet` crate),
//! not with assumptions about their bytes: a body that merely looked like a Parquet file
//! would fail to open, and one whose footer did not state what it holds would fail to read.
//!
//! The rows span what the rule generalises over: HOW MANY partition columns the write names
//! (none, one, two of different types), WHICH codec its pages are stored under (the default
//! and none at all), and WHAT the dataset holds (whole numbers, text, fractions, booleans,
//! and a column holding a null).
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, the scripts and
//! the data they read are staged through Amazon S3's own `PutObject`, and each run is read
//! back until it has come to rest ([`crate::job_runs::run_at_rest`]) with the span the clock
//! owes it made due through the store rather than slept out — so no outcome here depends on
//! how fast the host runs.

use parquet::basic::Compression;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{JOB_RUN_STATE_FAILED, JOB_RUN_STATE_SUCCEEDED};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_script_execution::{
    create_bucket, emulator_with_s3, object_body, run_to_rest, stage_script, BUCKET, ROLE_ARN,
    SCRIPTS,
};
use crate::wire::ok;

/// A Spark script that reads its CSV source under a schema of its own and writes the dataset
/// out as Parquet — partitioned by the columns its `--PARTITION_BY` argument names, with its
/// pages stored under the codec `--COMPRESSION` names.
const WRITES_PARQUET: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession
from pyspark.sql.types import (
    BooleanType,
    DoubleType,
    LongType,
    StringType,
    StructField,
    StructType,
)

args = getResolvedOptions(
    sys.argv, ["SOURCE_PATH", "OUTPUT_PATH", "PARTITION_BY", "COMPRESSION"]
)
spark = SparkSession(SparkContext.getOrCreate())
schema = StructType(
    [
        StructField("id", LongType()),
        StructField("region", StringType()),
        StructField("year", LongType()),
        StructField("amount", DoubleType()),
        StructField("live", BooleanType()),
    ]
)
source = spark.read.schema(schema).option("header", "true").csv(args["SOURCE_PATH"])
writer = source.write.mode("overwrite").option("compression", args["COMPRESSION"])
keys = [key for key in args["PARTITION_BY"].split(",") if key]
if keys:
    writer = writer.partitionBy(*keys)
writer.parquet(args["OUTPUT_PATH"])
"#;

/// A script that writes its source out as Parquet, partitioned, and then READS that output
/// back and reports the rows and the schema it read — so what the write left is judged by a
/// read of it as well.
const ROUND_TRIPS_PARQUET: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH", "REPORT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
source = spark.read.option("header", "true").option("inferSchema", "true").csv(
    args["SOURCE_PATH"]
)
source.write.mode("overwrite").partitionBy("region").parquet(args["OUTPUT_PATH"])
read = spark.read.parquet(args["OUTPUT_PATH"])
lines = []
for row in read.collect():
    held = row.asDict()
    lines.append("|".join("%s=%s" % (name, held[name]) for name in sorted(held)))
lines.sort()
lines.append("columns=%s" % (",".join(sorted(read.columns)),))
lines.append("rows=%d" % (read.count(),))
spark.sparkContext.parallelize(lines).saveAsTextFile(args["REPORT_PATH"])
"#;

/// A Glue ETL script that writes its dynamic frame out through the documented sink, in the
/// columnar format, partitioned by one of its fields.
const SINK_WRITES_PARQUET: &str = r#"
import sys

from awsglue.context import GlueContext
from awsglue.job import Job
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
glueContext.write_dynamic_frame.from_options(
    frame=source,
    connection_type="s3",
    connection_options={"path": args["OUTPUT_PATH"], "partitionKeys": ["region"]},
    format="parquet",
    transformation_ctx="sink",
)
job.commit()
"#;

/// A script that computes over its source through the FUNCTION CATALOG — a projection built
/// with `concat`, `lit` and `cast`, a conditional, a filter, a grouping with three aggregates
/// and an ordering — writes what it computed out as Parquet, and reports what the catalog's
/// own functions answer for the arguments they were given.
const AGGREGATES_THROUGH_THE_FUNCTION_CATALOG: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession
from pyspark.sql import functions as F

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "OUTPUT_PATH", "REPORT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
source = (
    spark.read.schema("id bigint, region string, year bigint, amount double, live boolean")
    .option("header", "true")
    .csv(args["SOURCE_PATH"])
)
labelled = source.withColumn(
    "label", F.concat(F.col("region"), F.lit("-"), F.col("year").cast("string"))
).withColumn(
    "band", F.when(F.col("amount") > 8, F.lit("high")).otherwise(F.lit("low"))
)
grouped = labelled.filter(F.col("live")).groupBy("label").agg(
    F.sum("amount").alias("total"),
    F.count("*").alias("rows"),
    F.max("band").alias("band"),
)
grouped.orderBy(F.col("total")).write.mode("overwrite").parquet(args["OUTPUT_PATH"])

# What the catalog's own functions answer for the arguments they were given: the pattern and
# the limit of a split, the half-up rounding, a distinct count over one column and over two
# together, the first and last value of a column holding a null, and the three-valued logic.
nulls = spark.createDataFrame([(None,), (7.5,)], ["amount"])
checks = [
    "split=%s" % (labelled.select(F.split(F.lit("a1b22c"), "[0-9]+")).collect()[0][0],),
    "limit=%s" % (labelled.select(F.split(F.lit("a,b,c"), ",", 2)).collect()[0][0],),
    "round=%s" % (labelled.select(F.round(F.lit(2.5))).collect()[0][0],),
    "negative=%s" % (labelled.select(F.round(F.lit(-2.5))).collect()[0][0],),
    "regions=%s" % (labelled.agg(F.countDistinct("region")).collect()[0][0],),
    "tuples=%s" % (labelled.agg(F.countDistinct("region", "id")).collect()[0][0],),
    "first=%s" % (nulls.agg(F.first("amount")).collect()[0][0],),
    "first_ignore=%s" % (nulls.agg(F.first("amount", True)).collect()[0][0],),
    "last=%s" % (nulls.agg(F.last("amount")).collect()[0][0],),
    "and=%s" % (labelled.select(F.lit(None) & F.lit(True)).collect()[0][0],),
    "or=%s" % (labelled.select(F.lit(None) | F.lit(True)).collect()[0][0],),
]
spark.sparkContext.parallelize(checks).saveAsTextFile(args["REPORT_PATH"])
"#;

/// The source every row of the table reads: whole numbers, text, fractions and booleans.
const SOURCE: &str = "id,region,year,amount,live\n\
                      1,west,2024,10.5,true\n\
                      2,east,2025,3.25,false\n\
                      3,west,2024,7.75,true\n";

/// The source the aggregate case reads: five rows over three labels, one of which the
/// script's filter drops.
const SOURCE_FOR_AGGREGATES: &str = "id,region,year,amount,live\n\
                                     1,west,2024,10.5,true\n\
                                     2,east,2025,3.25,false\n\
                                     3,west,2024,7.75,true\n\
                                     4,east,2025,12.25,true\n\
                                     5,south,2023,1.5,true\n";

/// The same shape of source with one row holding NO amount, so a written column really
/// carries a null and its footer says how many.
const SOURCE_WITH_A_NULL: &str = "id,region,year,amount,live\n\
                                   1,west,2024,10.5,true\n\
                                   2,east,2025,,false\n";

/// A source whose partition column holds values the Hive layout may not spell in a path: a
/// path separator and the character its own grammar uses between a column and its value.
const SOURCE_WITH_AWKWARD_PARTITION_VALUES: &str = "id,region,year,amount,live\n\
                                                    1,a/b,2024,10.5,true\n\
                                                    2,x=y,2025,3.25,false\n";

/// The `Timeout` every job here carries, in minutes: the shortest the model admits, since the
/// span one script run may last is this emulator's own ceiling and far below it.
const TIMEOUT_MINUTES: i64 = 1;

/// The Parquet writer the footer of every object written here is stamped with: the
/// `parquet-mr` release the Apache Spark 3.3 that AWS Glue 4.0 ships depends on, which is the
/// writer that wrote the file.
const PARQUET_WRITER: &str = "parquet-mr version 1.12.2";

/// The `CreateJob` body of a Spark job whose script is at `location`.
fn job_body(name: &str, location: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": location, "PythonVersion": "3" },
        "GlueVersion": "4.0",
        "WorkerType": "G.1X",
        "NumberOfWorkers": 2,
        "Timeout": TIMEOUT_MINUTES,
    })
}

/// The bytes of one object, or `None` when the object store does not serve it.
pub(crate) async fn object_bytes(router: &MultiServiceRouter, key: &str) -> Option<Vec<u8>> {
    let object = crate::wire::s3_client(router)
        .get_object()
        .bucket(BUCKET)
        .key(key)
        .send()
        .await
        .ok()?;
    Some(object.body.collect().await.ok()?.into_bytes().to_vec())
}

/// Every object key the store holds under `prefix`, in the order the store lists them.
pub(crate) async fn keys_under(
    router: &MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Vec<String> {
    let listed = crate::wire::s3_client(router)
        .list_objects_v2()
        .bucket(BUCKET)
        .prefix(prefix)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the output must list, got {error:?}"));
    listed
        .contents()
        .iter()
        .map(|object| object.key().unwrap_or_default().to_string())
        .collect()
}

/// One listed part key with the per-WRITE parts of its name taken out.
///
/// A part file's basename is `part-<index>-<write-job uuid>-c<counter><suffix>`, and the UUID
/// and the counter belong to the write job that produced it, so a row names a part by its
/// index and the suffix of the format and codec it was written in. A name that is not that
/// template at all is answered as it stands, so an assertion over it reports what was really
/// listed.
fn named_by_index(within: &str) -> String {
    let (directory, basename) = match within.rsplit_once('/') {
        Some((at, name)) => (format!("{at}/"), name),
        None => (String::new(), within),
    };
    let held = (|| {
        let rest = basename.strip_prefix("part-")?;
        let index = rest
            .get(..5)
            .filter(|index| index.chars().all(|held| held.is_ascii_digit()))?;
        let rest = rest.get(5..)?.strip_prefix('-')?;
        let uuid = rest.get(..36)?;
        if uuid.split('-').map(str::len).collect::<Vec<_>>() != vec![8, 4, 4, 4, 12] {
            return None;
        }
        let rest = rest.get(36..)?.strip_prefix("-c")?;
        let _counter = rest
            .get(..3)
            .filter(|counter| counter.chars().all(|held| held.is_ascii_digit()))?;
        Some(format!("part-{index}{}", rest.get(3..).unwrap_or_default()))
    })();
    match held {
        Some(name) => format!("{directory}{name}"),
        None => within.to_string(),
    }
}

/// One value of a read row, rendered so a case's expectation can state it as text.
fn rendered(field: &Field) -> String {
    match field {
        Field::Null => "null".to_string(),
        Field::Bool(value) => value.to_string(),
        Field::Int(value) => value.to_string(),
        Field::Long(value) => value.to_string(),
        Field::Double(value) => value.to_string(),
        Field::Str(value) => value.clone(),
        other => other.to_string(),
    }
}

/// What one Parquet object holds, read with a Parquet reader: its rows, its columns' names
/// and physical types, the codec its pages are stored under, and what its footer states about
/// each column.
pub(crate) struct Held {
    /// Every row, each value rendered in column order.
    pub(crate) rows: Vec<Vec<String>>,
    /// The (column, physical type) pairs the footer's schema states, in column order.
    pub(crate) schema: Vec<(String, String)>,
    /// The codec the chunks' pages are stored under.
    pub(crate) compression: String,
    /// The row count the footer states, which a reader trusts before reading a page.
    pub(crate) stated_rows: i64,
    /// Per column: how many nulls the footer says it holds, and its stated bounds.
    pub(crate) statistics: Vec<(i64, String, String)>,
    /// The writer the footer is stamped with, which is the Parquet writer the environment
    /// that ran the script ships.
    pub(crate) created_by: String,
}

/// Read one written object as the Parquet file it has to be.
pub(crate) fn held_by(case: &str, key: &str, body: &[u8]) -> Held {
    let reader = SerializedFileReader::new(bytes::Bytes::from(body.to_vec()))
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {key} must be a Parquet file, got {error:?}")
        });
    let metadata = reader.metadata();
    let descriptor = metadata.file_metadata().schema_descr();
    let schema: Vec<(String, String)> = (0..descriptor.num_columns())
        .map(|index| {
            let column = descriptor.column(index);
            (
                column.name().to_string(),
                column.physical_type().to_string(),
            )
        })
        .collect();
    let group = metadata.row_group(0);
    let compression = format!("{:?}", group.column(0).compression());
    let statistics: Vec<(i64, String, String)> = (0..group.num_columns())
        .map(|index| {
            let column = group.column(index);
            let stated = column.statistics().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {key} must state statistics for {:?}",
                    column.column_path()
                )
            });
            let bound = |held: Option<&[u8]>| match (held, column.column_type()) {
                (None, _) => "none".to_string(),
                (Some(bytes), parquet::basic::Type::INT64) => {
                    i64::from_le_bytes(bytes.try_into().unwrap_or_default()).to_string()
                }
                (Some(bytes), parquet::basic::Type::DOUBLE) => {
                    f64::from_le_bytes(bytes.try_into().unwrap_or_default()).to_string()
                }
                (Some(bytes), parquet::basic::Type::BOOLEAN) => {
                    (bytes.first().copied().unwrap_or_default() != 0).to_string()
                }
                (Some(bytes), _) => String::from_utf8_lossy(bytes).to_string(),
            };
            (
                i64::try_from(stated.null_count_opt().unwrap_or_default()).unwrap_or_default(),
                bound(stated.min_bytes_opt()),
                bound(stated.max_bytes_opt()),
            )
        })
        .collect();
    let rows = reader
        .get_row_iter(None)
        .unwrap_or_else(|error| panic!("case {case:?}: {key} must be readable, got {error:?}"))
        .map(|row| {
            row.unwrap_or_else(|error| {
                panic!("case {case:?}: every row of {key} must be readable, got {error:?}")
            })
            .get_column_iter()
            .map(|(_name, field)| rendered(field))
            .collect()
        })
        .collect();
    Held {
        rows,
        schema,
        compression,
        stated_rows: metadata.file_metadata().num_rows(),
        statistics,
        created_by: metadata
            .file_metadata()
            .created_by()
            .unwrap_or_default()
            .to_string(),
    }
}

/// One row of the table: a write of a dataset, and the Parquet objects it must leave.
struct Row {
    /// What every assertion of the row reports.
    case: &'static str,
    /// The job's name, which is also the path segment its objects live under.
    label: &'static str,
    /// The CSV staged under the row's source prefix.
    source: &'static str,
    /// The columns the write partitions by, comma-separated; empty for a write that names
    /// none.
    partition_by: &'static str,
    /// The codec the write stores its pages under, as its `compression` option names it.
    compression: &'static str,
    /// The codec the written chunks must then report.
    stored: Compression,
    /// The part files the write must leave under the output prefix, relative to it — each
    /// named by its index and its suffix, with the UUID and the counter a write mints per
    /// write of its own left out ([`named_by_index`]).
    parts: &'static [&'static str],
    /// The rows each of those parts must hold, in part order.
    rows: &'static [&'static [&'static [&'static str]]],
    /// The (column, physical type) pairs each part's footer must state.
    schema: &'static [(&'static str, &'static str)],
    /// Per column of each part: the null count and the bounds its footer must state.
    statistics: &'static [&'static [(i64, &'static str, &'static str)]],
}

/// RULE: a Spark job run's columnar write leaves Apache Parquet objects whose pages hold the
/// dataset's own values and whose footer states its rows, its schema and each column's
/// bounds, under the Hive-style partition directories the write names.
#[tokio::test]
async fn a_columnar_write_leaves_parquet_objects_laid_out_as_the_write_names() {
    let rows = [
        Row {
            case: "a write that names no partition column",
            label: "parquet-flat",
            source: SOURCE,
            partition_by: "",
            compression: "snappy",
            stored: Compression::SNAPPY,
            parts: &["part-00000.snappy.parquet"],
            rows: &[&[
                &["1", "west", "2024", "10.5", "true"],
                &["2", "east", "2025", "3.25", "false"],
                &["3", "west", "2024", "7.75", "true"],
            ]],
            schema: &[
                ("id", "INT64"),
                ("region", "BYTE_ARRAY"),
                ("year", "INT64"),
                ("amount", "DOUBLE"),
                ("live", "BOOLEAN"),
            ],
            statistics: &[&[
                (0, "1", "3"),
                (0, "east", "west"),
                (0, "2024", "2025"),
                (0, "3.25", "10.5"),
                (0, "false", "true"),
            ]],
        },
        Row {
            case: "a write partitioned by one text column",
            label: "parquet-by-region",
            source: SOURCE,
            partition_by: "region",
            compression: "snappy",
            stored: Compression::SNAPPY,
            parts: &[
                "region=east/part-00000.snappy.parquet",
                "region=west/part-00000.snappy.parquet",
            ],
            rows: &[
                &[&["2", "2025", "3.25", "false"]],
                &[&["1", "2024", "10.5", "true"], &["3", "2024", "7.75", "true"]],
            ],
            schema: &[
                ("id", "INT64"),
                ("year", "INT64"),
                ("amount", "DOUBLE"),
                ("live", "BOOLEAN"),
            ],
            statistics: &[
                &[
                    (0, "2", "2"),
                    (0, "2025", "2025"),
                    (0, "3.25", "3.25"),
                    (0, "false", "false"),
                ],
                &[
                    (0, "1", "3"),
                    (0, "2024", "2024"),
                    (0, "7.75", "10.5"),
                    (0, "true", "true"),
                ],
            ],
        },
        Row {
            case: "a write partitioned by a text column and a whole-number one",
            label: "parquet-by-region-year",
            source: SOURCE,
            partition_by: "region,year",
            compression: "snappy",
            stored: Compression::SNAPPY,
            parts: &[
                "region=east/year=2025/part-00000.snappy.parquet",
                "region=west/year=2024/part-00000.snappy.parquet",
            ],
            rows: &[
                &[&["2", "3.25", "false"]],
                &[&["1", "10.5", "true"], &["3", "7.75", "true"]],
            ],
            schema: &[("id", "INT64"), ("amount", "DOUBLE"), ("live", "BOOLEAN")],
            statistics: &[
                &[(0, "2", "2"), (0, "3.25", "3.25"), (0, "false", "false")],
                &[(0, "1", "3"), (0, "7.75", "10.5"), (0, "true", "true")],
            ],
        },
        Row {
            case: "a write whose partition values hold characters a path may not carry",
            label: "parquet-awkward-values",
            source: SOURCE_WITH_AWKWARD_PARTITION_VALUES,
            partition_by: "region",
            compression: "snappy",
            stored: Compression::SNAPPY,
            // The value is escaped as the Hive layout spells it, so a cell holding a path
            // separator names a directory under the write's own path and nothing else.
            parts: &[
                "region=a%2Fb/part-00000.snappy.parquet",
                "region=x%3Dy/part-00000.snappy.parquet",
            ],
            rows: &[
                &[&["1", "2024", "10.5", "true"]],
                &[&["2", "2025", "3.25", "false"]],
            ],
            schema: &[
                ("id", "INT64"),
                ("year", "INT64"),
                ("amount", "DOUBLE"),
                ("live", "BOOLEAN"),
            ],
            statistics: &[
                &[
                    (0, "1", "1"),
                    (0, "2024", "2024"),
                    (0, "10.5", "10.5"),
                    (0, "true", "true"),
                ],
                &[
                    (0, "2", "2"),
                    (0, "2025", "2025"),
                    (0, "3.25", "3.25"),
                    (0, "false", "false"),
                ],
            ],
        },
        Row {
            case: "a write whose pages are stored under no codec at all",
            label: "parquet-uncompressed",
            source: SOURCE,
            partition_by: "",
            compression: "uncompressed",
            stored: Compression::UNCOMPRESSED,
            parts: &["part-00000.parquet"],
            rows: &[&[
                &["1", "west", "2024", "10.5", "true"],
                &["2", "east", "2025", "3.25", "false"],
                &["3", "west", "2024", "7.75", "true"],
            ]],
            schema: &[
                ("id", "INT64"),
                ("region", "BYTE_ARRAY"),
                ("year", "INT64"),
                ("amount", "DOUBLE"),
                ("live", "BOOLEAN"),
            ],
            statistics: &[&[
                (0, "1", "3"),
                (0, "east", "west"),
                (0, "2024", "2025"),
                (0, "3.25", "10.5"),
                (0, "false", "true"),
            ]],
        },
        Row {
            case: "a write of a dataset holding a null",
            label: "parquet-with-a-null",
            source: SOURCE_WITH_A_NULL,
            partition_by: "",
            compression: "snappy",
            stored: Compression::SNAPPY,
            parts: &["part-00000.snappy.parquet"],
            rows: &[&[
                &["1", "west", "2024", "10.5", "true"],
                &["2", "east", "2025", "null", "false"],
            ]],
            schema: &[
                ("id", "INT64"),
                ("region", "BYTE_ARRAY"),
                ("year", "INT64"),
                ("amount", "DOUBLE"),
                ("live", "BOOLEAN"),
            ],
            statistics: &[&[
                (0, "1", "2"),
                (0, "east", "west"),
                (0, "2024", "2025"),
                (1, "10.5", "10.5"),
                (0, "false", "true"),
            ]],
        },
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for row in &rows {
        let case = row.case;
        let label = row.label;
        stage_script(&router, &format!("data/{label}/input.csv"), row.source).await;
        let location = stage_script(
            &router,
            &format!("{SCRIPTS}{label}.py"),
            WRITES_PARQUET,
        )
        .await;
        ok(&router, "CreateJob", &job_body(label, &location)).await;

        let run = run_to_rest(
            &router,
            &engine,
            label,
            &json!({
                "--SOURCE_PATH": format!("s3://{BUCKET}/data/{label}/"),
                "--OUTPUT_PATH": format!("s3://{BUCKET}/output/{label}"),
                "--PARTITION_BY": row.partition_by,
                "--COMPRESSION": row.compression,
            }),
        )
        .await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_SUCCEEDED),
            "case {case:?}: expected the run to come to rest in {JOB_RUN_STATE_SUCCEEDED}, \
             got {run}"
        );

        let prefix = format!("output/{label}/");
        let listed = keys_under(&router, case, &prefix).await;
        let parts: Vec<String> = listed
            .iter()
            .map(|key| key[prefix.len()..].to_string())
            .collect();
        assert_eq!(
            parts.iter().map(|part| named_by_index(part)).collect::<Vec<_>>(),
            row.parts.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: expected the part files {:?} under {prefix} and nothing else, \
             got {listed:?}",
            row.parts
        );

        for (index, part) in parts.iter().enumerate() {
            let key = format!("{prefix}{part}");
            let body = object_bytes(&router, &key)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: {key} must be served"));
            let held = held_by(case, &key, &body);
            let expected: Vec<Vec<String>> = row.rows[index]
                .iter()
                .map(|values| values.iter().map(ToString::to_string).collect())
                .collect();
            assert_eq!(
                held.rows, expected,
                "case {case:?}: expected {key} to hold {expected:?}, got {:?}",
                held.rows
            );
            assert_eq!(
                held.schema,
                row.schema
                    .iter()
                    .map(|(name, kind)| ((*name).to_string(), (*kind).to_string()))
                    .collect::<Vec<_>>(),
                "case {case:?}: expected the footer of {key} to state the schema {:?}, got \
                 {:?}",
                row.schema,
                held.schema
            );
            assert_eq!(
                held.stated_rows,
                i64::try_from(expected.len()).unwrap_or_default(),
                "case {case:?}: expected the footer of {key} to state {} rows, got {}",
                expected.len(),
                held.stated_rows
            );
            assert_eq!(
                held.compression,
                format!("{:?}", row.stored),
                "case {case:?}: expected the pages of {key} stored as {:?}, got {}",
                row.stored,
                held.compression
            );
            let bounds: Vec<(i64, String, String)> = row.statistics[index]
                .iter()
                .map(|(nulls, smallest, largest)| {
                    (*nulls, (*smallest).to_string(), (*largest).to_string())
                })
                .collect();
            assert_eq!(
                held.statistics, bounds,
                "case {case:?}: expected the footer of {key} to state the nulls and bounds \
                 {bounds:?}, got {:?}",
                held.statistics
            );
            // WHO wrote it: the Parquet writer the Spark release this job's `GlueVersion`
            // ships, which is what the footer of a file that release wrote is stamped with.
            assert_eq!(
                held.created_by, PARQUET_WRITER,
                "case {case:?}: expected {key} stamped {PARQUET_WRITER:?}, got {:?}",
                held.created_by
            );
        }
    }
}

/// A script that only READS a Parquet path and reports how many rows it took in — so what a
/// read of a body that is not one comes to is readable from the run.
const READS_PARQUET: &str = r#"
import sys

from awsglue.utils import getResolvedOptions
from pyspark.context import SparkContext
from pyspark.sql import SparkSession

args = getResolvedOptions(sys.argv, ["SOURCE_PATH", "REPORT_PATH"])
spark = SparkSession(SparkContext.getOrCreate())
read = spark.read.parquet(args["SOURCE_PATH"])
spark.sparkContext.parallelize(["rows=%d" % (read.count(),)]).saveAsTextFile(
    args["REPORT_PATH"]
)
"#;

/// RULE, the other way round: a dataset written out as Parquet READS BACK as the dataset that
/// was written — the values out of the files' pages, and each partition column out of the
/// `<name>=<value>` directory the Hive layout holds it in.
#[tokio::test]
async fn a_parquet_write_reads_back_as_the_dataset_that_was_written() {
    let case = "a partitioned write read back through the Parquet reader";
    let label = "parquet-round-trip";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    stage_script(&router, &format!("data/{label}/input.csv"), SOURCE).await;
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}{label}.py"),
        ROUND_TRIPS_PARQUET,
    )
    .await;
    ok(&router, "CreateJob", &job_body(label, &location)).await;

    let run = run_to_rest(
        &router,
        &engine,
        label,
        &json!({
            "--SOURCE_PATH": format!("s3://{BUCKET}/data/{label}/"),
            "--OUTPUT_PATH": format!("s3://{BUCKET}/output/{label}"),
            "--REPORT_PATH": format!("s3://{BUCKET}/report/{label}"),
        }),
    )
    .await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
    );

    let report = object_body(&router, &format!("report/{label}/part-00000")).await;
    assert_eq!(
        report.as_deref(),
        Some(
            "amount=10.5|id=1|live=true|region=west|year=2024\n\
             amount=3.25|id=2|live=false|region=east|year=2025\n\
             amount=7.75|id=3|live=true|region=west|year=2024\n\
             columns=amount,id,live,region,year\n\
             rows=3\n"
        ),
        "case {case:?}: expected the read of the written output to report every row it was \
         written from, got {report:?}"
    );
}

/// RULE: the same columnar write through the AWS Glue ETL library's own documented sink — a
/// dynamic frame written `format="parquet"` with partition keys leaves the same Parquet
/// objects under the same Hive layout.
#[tokio::test]
async fn a_dynamic_frame_sink_writes_parquet_under_the_partition_keys_it_names() {
    let case = "a dynamic frame written out as Parquet, partitioned";
    let label = "parquet-sink";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    stage_script(&router, &format!("data/{label}/input.csv"), SOURCE).await;
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}{label}.py"),
        SINK_WRITES_PARQUET,
    )
    .await;
    ok(&router, "CreateJob", &job_body(label, &location)).await;

    let run = run_to_rest(
        &router,
        &engine,
        label,
        &json!({
            "--JOB_NAME": label,
            "--SOURCE_PATH": format!("s3://{BUCKET}/data/{label}/"),
            "--OUTPUT_PATH": format!("s3://{BUCKET}/output/{label}"),
        }),
    )
    .await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
    );

    let prefix = format!("output/{label}/");
    let listed = keys_under(&router, case, &prefix).await;
    let parts: Vec<String> = listed
        .iter()
        .map(|key| key[prefix.len()..].to_string())
        .collect();
    assert_eq!(
        parts,
        vec![
            "region=east/part-00000.snappy.parquet".to_string(),
            "region=west/part-00000.snappy.parquet".to_string(),
        ],
        "case {case:?}: expected one Parquet part per partition key value, got {listed:?}"
    );

    // The records a Glue dynamic frame read out of delimited text are the text's own values,
    // so the columns the sink writes carry them as text.
    for (part, expected) in [
        (
            "region=east/part-00000.snappy.parquet",
            vec![vec!["2", "2025", "3.25", "false"]],
        ),
        (
            "region=west/part-00000.snappy.parquet",
            vec![
                vec!["1", "2024", "10.5", "true"],
                vec!["3", "2024", "7.75", "true"],
            ],
        ),
    ] {
        let key = format!("{prefix}{part}");
        let body = object_bytes(&router, &key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: {key} must be served"));
        let held = held_by(case, &key, &body);
        let rows: Vec<Vec<String>> = expected
            .iter()
            .map(|values| values.iter().map(ToString::to_string).collect())
            .collect();
        assert_eq!(
            held.rows, rows,
            "case {case:?}: expected {key} to hold {rows:?}, got {:?}",
            held.rows
        );
        assert_eq!(
            held.schema
                .iter()
                .map(|(name, _kind)| name.clone())
                .collect::<Vec<_>>(),
            vec!["id", "year", "amount", "live"],
            "case {case:?}: expected the footer of {key} to state the fields the frame held \
             outside its partition keys, got {:?}",
            held.schema
        );
    }
}

/// RULE: what a run writes is what its script COMPUTED — a projection, a conditional, a
/// filter, a grouping and an ordering built from the function catalog every PySpark program
/// imports (`pyspark.sql.functions`), landed in the Parquet file's own pages and stated in
/// its footer.
#[tokio::test]
async fn an_aggregate_computed_through_the_function_catalog_is_what_the_run_writes() {
    let case = "a grouped aggregate written out as Parquet";
    let label = "parquet-aggregates";
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;
    stage_script(
        &router,
        &format!("data/{label}/input.csv"),
        SOURCE_FOR_AGGREGATES,
    )
    .await;
    let location = stage_script(
        &router,
        &format!("{SCRIPTS}{label}.py"),
        AGGREGATES_THROUGH_THE_FUNCTION_CATALOG,
    )
    .await;
    ok(&router, "CreateJob", &job_body(label, &location)).await;

    let run = run_to_rest(
        &router,
        &engine,
        label,
        &json!({
            "--SOURCE_PATH": format!("s3://{BUCKET}/data/{label}/"),
            "--OUTPUT_PATH": format!("s3://{BUCKET}/output/{label}"),
            "--REPORT_PATH": format!("s3://{BUCKET}/report/{label}"),
        }),
    )
    .await;
    assert_eq!(
        run.get("JobRunState").and_then(Value::as_str),
        Some(JOB_RUN_STATE_SUCCEEDED),
        "case {case:?}: expected {JOB_RUN_STATE_SUCCEEDED}, got {run}"
    );

    // What the catalog's functions answered for the arguments they were given: the pattern
    // and the limit a split was called with, the half-up rounding, a distinct count over one
    // column and over two together, the first and the last value of a column holding a null
    // (Spark's own default answers the null; `ignorenulls` passes over it), and the
    // three-valued logic a null operand gives.
    let report = object_body(&router, &format!("report/{label}/part-00000")).await;
    assert_eq!(
        report.as_deref(),
        Some(
            "split=['a', 'b', 'c']\n\
             limit=['a', 'b,c']\n\
             round=3.0\n\
             negative=-3.0\n\
             regions=3\n\
             tuples=5\n\
             first=None\n\
             first_ignore=7.5\n\
             last=7.5\n\
             and=None\n\
             or=True\n"
        ),
        "case {case:?}: expected the catalog's own answers, got {report:?}"
    );

    let prefix = format!("output/{label}/");
    let listed = keys_under(&router, case, &prefix).await;
    // The write left one part file, whose name carries the UUID of the write job that
    // produced it, so it is read out of the store's own listing.
    assert_eq!(
        listed
            .iter()
            .map(|key| named_by_index(&key[prefix.len()..]))
            .collect::<Vec<_>>(),
        vec!["part-00000.snappy.parquet".to_string()],
        "case {case:?}: expected one Parquet part file under {prefix}, got {listed:?}"
    );
    let key = listed[0].clone();
    let body = object_bytes(&router, &key)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: {key} must be served"));
    let held = held_by(case, &key, &body);
    // The row the filter dropped contributes to nothing: the totals are the sums of the
    // rows that were kept, the counts their number, and the order the ascending one the
    // script asked for.
    assert_eq!(
        held.rows,
        vec![
            vec!["south-2023", "1.5", "1", "low"],
            vec!["east-2025", "12.25", "1", "high"],
            vec!["west-2024", "18.25", "2", "low"],
        ],
        "case {case:?}: expected the aggregate the script computed, got {:?}",
        held.rows
    );
    assert_eq!(
        held.schema,
        vec![
            ("label".to_string(), "BYTE_ARRAY".to_string()),
            ("total".to_string(), "DOUBLE".to_string()),
            ("rows".to_string(), "INT64".to_string()),
            ("band".to_string(), "BYTE_ARRAY".to_string()),
        ],
        "case {case:?}: expected the footer to state the aggregate's own schema, got {:?}",
        held.schema
    );
    assert_eq!(
        held.statistics,
        vec![
            (0, "east-2025".to_string(), "west-2024".to_string()),
            (0, "1.5".to_string(), "18.25".to_string()),
            (0, "1".to_string(), "2".to_string()),
            (0, "high".to_string(), "low".to_string()),
        ],
        "case {case:?}: expected the footer to state the bounds of what was computed, got \
         {:?}",
        held.statistics
    );
}

/// RULE: a body at a Parquet path that is NOT a Parquet file this runtime reads fails the read
/// naming the object and what could not be read of it — it is never taken for data, and it
/// never costs the run more than the failure.
///
/// The rows span the ways a caller-supplied body can be one: bytes that are not a Parquet file
/// at all, a body shorter than the format's own frame, one whose stated footer is longer than
/// the file, and one whose footer is bytes that are no metadata. (The ceiling on how many
/// bytes one read takes in is a constant of the reader, far above anything a test stages.)
#[tokio::test]
async fn a_body_that_is_not_a_parquet_file_fails_the_read_naming_it() {
    // (case, the staged body, what the failure must carry beyond the object's own address)
    let rows: [(&str, Vec<u8>, Option<&str>); 4] = [
        (
            "bytes that are not a Parquet file at all",
            b"id,region\n1,west\n".to_vec(),
            Some("a file not framed by the Parquet magic bytes is not one"),
        ),
        (
            "a body shorter than the format's own frame",
            b"PAR1".to_vec(),
            Some("a file of 4 bytes is not a Parquet file"),
        ),
        (
            "a body whose stated footer is longer than the file",
            [b"PAR1".as_slice(), &[0xFF, 0xFF, 0x00, 0x00], b"PAR1".as_slice()].concat(),
            Some("the footer a Parquet file states is longer than the file"),
        ),
        (
            "a body whose footer is bytes that are no metadata",
            [
                b"PAR1".as_slice(),
                &[0x01, 0x02, 0x03, 0x04, 0x05],
                &[0x05, 0x00, 0x00, 0x00],
                b"PAR1".as_slice(),
            ]
            .concat(),
            None,
        ),
    ];

    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, (case, body, carries)) in rows.iter().enumerate() {
        let label = format!("parquet-unreadable-{index}");
        let key = format!("data/{label}/part-0.parquet");
        crate::wire::s3_client(&router)
            .put_object()
            .bucket(BUCKET)
            .key(&key)
            .body(aws_sdk_s3::primitives::ByteStream::from(body.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the body must be stored: {error:?}"));
        let location = stage_script(&router, &format!("{SCRIPTS}{label}.py"), READS_PARQUET).await;
        ok(&router, "CreateJob", &job_body(&label, &location)).await;

        let run = run_to_rest(
            &router,
            &engine,
            &label,
            &json!({
                "--SOURCE_PATH": format!("s3://{BUCKET}/data/{label}/"),
                "--REPORT_PATH": format!("s3://{BUCKET}/report/{label}"),
            }),
        )
        .await;
        assert_eq!(
            run.get("JobRunState").and_then(Value::as_str),
            Some(JOB_RUN_STATE_FAILED),
            "case {case:?}: expected the run to come to rest in {JOB_RUN_STATE_FAILED}, got \
             {run}"
        );
        let message = run["ErrorMessage"].as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(&format!("s3://{BUCKET}/{key}")),
            "case {case:?}: expected the failure to name the object, got {message:?}"
        );
        if let Some(held) = carries {
            assert!(
                message.contains(held),
                "case {case:?}: expected the failure to carry {held:?}, got {message:?}"
            );
        }
        let report = object_body(&router, &format!("report/{label}/part-00000")).await;
        assert_eq!(
            report, None,
            "case {case:?}: a read that failed took nothing in, so the script wrote no \
             report, got {report:?}"
        );
    }
}
