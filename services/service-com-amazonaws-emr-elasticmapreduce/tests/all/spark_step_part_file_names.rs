// SPDX-License-Identifier: CC0-1.0

//! What a Spark step's DATASET WRITER names the part files it lands in Amazon S3.
//!
//! THE RULE: a write through Spark's dataset writer lands, under the output path — and under
//! one `<column>=<value>` directory level per column it partitions by — one part file per
//! partition of the dataset, whose basename is the template Spark's file output committer
//! leaves: the fixed `part-` prefix, the zero-padded index of the task that wrote it, the UUID
//! the write job was minted with, the fixed `-c` counter of that task, and the suffix of the
//! format the write named (`.json`, `.csv`, `.txt`, and a columnar part's codec suffix ahead
//! of `.parquet`). The parts of ONE write carry ONE write-job UUID and the parts of another
//! carry another, which is what attributes a file to the write that produced it; the suffix is
//! what a client selecting Spark output by its name finds the format it asked for by. The
//! marker a completed write leaves sits directly under the output path, and a partition
//! column is spelled by the directory rather than written into the part.
//!
//! The rows span what the rule generalises over: the FORMAT the write names (JSON, delimited
//! text, lines of text, the columnar format), the codec a columnar part's pages are stored
//! under, how many partitions the dataset holds — so how many parts one write leaves — and
//! whether the write partitions its output by a column, so whether its parts sit under
//! Hive-style directories.
//!
//! Amazon S3 is mounted beside Amazon EMR on one engine through this crate's own step harness
//! ([`super::spark_step_application`]): the application is a real object of the store, the step
//! is submitted with `RunJobFlow`, and every key asserted below is read back with
//! `ListObjectsV2` and `GetObject`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::spark_step_application::{
    drained, emulator, keys_under, launch, put_object, s3, BUCKET,
};

/// The key the application of this suite is written under.
const APPLICATION: &str = "applications/part_file_names.py";

/// The prefix every write of the application lands under.
const OUTPUT: &str = "out/names";

/// The marker a completed write leaves beside its parts, which the output committer of an
/// Amazon EMR cluster's own environment writes.
const SUCCESS_MARKER: &str = "_SUCCESS";

/// The application the step submits: one dataset, written SIX times — the aggregate of the
/// recorded shape partitioned by its key, the same dataset whole, over two partitions as
/// delimited text, as lines of text, and twice in the columnar format under two codecs.
///
/// Every write goes to its own path, so each one is a write job of its own and the names it
/// leaves are read back per path.
const PROGRAM: &str = r#"
import sys

from pyspark.sql import SparkSession
from pyspark.sql.functions import sum as summed

out = sys.argv[1].rstrip("/")
spark = SparkSession.builder.appName("part-file-names").getOrCreate()
frame = spark.createDataFrame(
    [("eng", 7), ("ops", 11), ("eng", 5)], "dept string, amount bigint"
)

aggregated = frame.groupBy("dept").agg(summed("amount").alias("total"))
aggregated.repartition(1).write.mode("overwrite").partitionBy("dept").json(
    "%s/json_partitioned" % out
)
frame.repartition(1).write.mode("overwrite").json("%s/json_whole" % out)
frame.repartition(2).write.mode("overwrite").csv("%s/csv_two_parts" % out)
spark.createDataFrame([("alpha",), ("beta",)], "line string").repartition(1).write.mode(
    "overwrite"
).text("%s/text_lines" % out)
frame.repartition(1).write.mode("overwrite").parquet("%s/parquet_snappy" % out)
frame.repartition(1).write.mode("overwrite").option("compression", "none").parquet(
    "%s/parquet_plain" % out
)
spark.stop()
"#;

/// How a part's bytes are read back.
#[derive(Clone, Copy, Debug)]
enum Holds {
    /// The text the part holds, compared as it stands.
    Text,
    /// The JSON documents its lines are, compared as documents: the writer renders a row's
    /// own fields, so what is asserted is the fields and their values rather than a spelling.
    Documents,
    /// The bytes are the columnar format's own, so what is asserted of them is that they are
    /// an Apache Parquet file — the format's magic at both ends of the file.
    ParquetBytes,
}

/// One row: a write of the application, and what it must leave behind.
struct Row {
    case: &'static str,
    /// The path segment under [`OUTPUT`] the write lands at.
    segment: &'static str,
    /// The suffix every part of the write must carry, which is the format it named.
    suffix: &'static str,
    /// How its parts are read back.
    holds: Holds,
    /// What each output DIRECTORY of the write holds: the directory relative to the write's
    /// own path — empty for a write that partitions by nothing — and one entry per part under
    /// it, in key order, carrying what that part holds. A columnar row's entries are read for
    /// their COUNT alone, since its bytes are the format's own.
    held: &'static [(&'static str, &'static [&'static str])],
}

/// Every row, in the order the application performs their writes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded shape: the aggregate partitioned by its key, where the partition
            // column is spelled by the directory and is NOT a field of the part.
            case: "an aggregate written as JSON, partitioned by its key",
            segment: "json_partitioned",
            suffix: ".json",
            holds: Holds::Documents,
            held: &[
                ("dept=eng", &[r#"{"total": 12}"#]),
                ("dept=ops", &[r#"{"total": 11}"#]),
            ],
        },
        Row {
            case: "the whole dataset written as JSON over one partition",
            segment: "json_whole",
            suffix: ".json",
            holds: Holds::Documents,
            held: &[(
                "",
                &["{\"dept\": \"eng\", \"amount\": 7}\n{\"dept\": \"ops\", \"amount\": \
                   11}\n{\"dept\": \"eng\", \"amount\": 5}\n"],
            )],
        },
        Row {
            case: "delimited text over two partitions, which leaves two parts",
            segment: "csv_two_parts",
            suffix: ".csv",
            holds: Holds::Text,
            held: &[("", &["eng,7\nops,11\n", "eng,5\n"])],
        },
        Row {
            case: "lines of text, whose parts carry the text source's own suffix",
            segment: "text_lines",
            suffix: ".txt",
            holds: Holds::Text,
            held: &[("", &["alpha\nbeta\n"])],
        },
        Row {
            case: "the columnar format under the codec a write names none for",
            segment: "parquet_snappy",
            suffix: ".snappy.parquet",
            holds: Holds::ParquetBytes,
            held: &[("", &[""])],
        },
        Row {
            case: "the columnar format with its pages under no codec at all",
            segment: "parquet_plain",
            suffix: ".parquet",
            holds: Holds::ParquetBytes,
            held: &[("", &[""])],
        },
    ]
}

/// The four parts of one part-file basename: the task index, the write-job UUID, the file
/// counter and the suffix — `None` when the name is not that template at all.
///
/// Written out here rather than imported, so a name the runtime builds by another template
/// cannot make this suite pass.
fn named(basename: &str) -> Option<(String, String, String, String)> {
    let rest = basename.strip_prefix("part-")?;
    let index = rest.get(..5)?;
    if !index.chars().all(|held| held.is_ascii_digit()) {
        return None;
    }
    let rest = rest.get(5..)?.strip_prefix('-')?;
    let uuid = rest.get(..36)?;
    let groups: Vec<&str> = uuid.split('-').collect();
    if groups.iter().map(|group| group.len()).collect::<Vec<_>>() != vec![8, 4, 4, 4, 12]
        || !groups
            .iter()
            .all(|group| group.chars().all(|held| held.is_ascii_hexdigit()))
    {
        return None;
    }
    let rest = rest.get(36..)?.strip_prefix("-c")?;
    let counter = rest.get(..3)?;
    if !counter.chars().all(|held| held.is_ascii_digit()) {
        return None;
    }
    let suffix = rest.get(3..)?;
    Some((
        index.to_string(),
        uuid.to_string(),
        counter.to_string(),
        suffix.to_string(),
    ))
}

/// The bytes at `key`, which must be served.
async fn body(router: &MultiServiceRouter, case: &str, key: &str) -> Vec<u8> {
    let (status, answered) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: GetObject on {key} must be served, got {status} with {:?}",
        String::from_utf8_lossy(&answered)
    );
    answered
}

/// One JSON document, parsed — a part's line is compared as the document it is.
fn document(case: &str, line: &str) -> Value {
    serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("case {case:?}: {line:?} must be JSON: {error}"))
}

/// Every write of one submitted application leaves its parts under the template Spark names a
/// part file by, and the directories and the marker its layout puts them in.
#[tokio::test]
async fn a_dataset_writes_parts_carry_spark_part_file_names() {
    let router = emulator();
    let rows = rows();

    let (status, answered) = s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert!(
        (200..300).contains(&status),
        "CreateBucket must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
    put_object(&router, "the application", APPLICATION, PROGRAM.as_bytes()).await;

    let cluster_id = launch(
        &router,
        "spark part file names",
        json!([{
            "Name": "write every format",
            "ActionOnFailure": "CONTINUE",
            "HadoopJarStep": {
                "Jar": "command-runner.jar",
                "Args": [
                    "spark-submit",
                    "--deploy-mode",
                    "client",
                    format!("s3://{BUCKET}/{APPLICATION}"),
                    format!("s3://{BUCKET}/{OUTPUT}"),
                ],
            },
        }]),
        true,
    )
    .await;
    let steps = drained("the step", &router, &cluster_id).await;
    let step = steps
        .first()
        .unwrap_or_else(|| panic!("expected the submitted step back, got {steps:?}"));
    assert_eq!(
        step["Status"]["State"].as_str(),
        Some("COMPLETED"),
        "an application that exited cleanly completes its step, got {step}"
    );

    // The UUID each write was minted with, so no two writes share one.
    let mut minted: Vec<(&str, String)> = Vec::new();

    for row in &rows {
        let case = row.case;
        let prefix = format!("{OUTPUT}/{}/", row.segment);
        let listed = keys_under(&router, case, &prefix).await;
        let within: Vec<String> = listed
            .iter()
            .map(|key| key[prefix.len().min(key.len())..].to_string())
            .collect();

        // The marker a completed write leaves sits directly under the write's own path.
        assert!(
            within.iter().any(|key| key == SUCCESS_MARKER),
            "case {case:?}: expected {SUCCESS_MARKER} directly under {prefix}, got {within:?}"
        );

        let parts: Vec<String> = within
            .iter()
            .filter(|key| *key != SUCCESS_MARKER)
            .cloned()
            .collect();
        let expected_count: usize = row.held.iter().map(|(_, held)| held.len()).sum();
        assert_eq!(
            parts.len(),
            expected_count,
            "case {case:?}: expected {expected_count} part file(s) under {prefix}, got \
             {within:?}"
        );

        let mut uuids: Vec<String> = Vec::new();
        for (directory, held) in row.held {
            for (index, expected) in held.iter().enumerate() {
                // WHERE the part sits: under the Hive-style directory of its partition
                // values, or directly under the write's own path when it partitions by
                // nothing.
                let under: Vec<&String> = parts
                    .iter()
                    .filter(|key| {
                        let at = key.rsplit_once('/').map(|(held, _)| held).unwrap_or("");
                        at == *directory
                    })
                    .collect();
                assert_eq!(
                    under.len(),
                    held.len(),
                    "case {case:?}: expected {} part file(s) under {prefix}{directory}, got \
                     {parts:?}",
                    held.len()
                );
                let key = under[index];
                let basename = key.rsplit('/').next().unwrap_or_default();
                let (task, uuid, counter, suffix) = named(basename).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {basename:?} is not a Spark part-file name \
                         (part-NNNNN-<uuid>-cNNN{})",
                        row.suffix
                    )
                });
                assert_eq!(
                    task,
                    format!("{index:05}"),
                    "case {case:?}: expected the task index of part {index} in {basename:?}, \
                     got {task:?}"
                );
                assert_eq!(
                    counter, "000",
                    "case {case:?}: a task that wrote one file names the first counter, got \
                     {counter:?} in {basename:?}"
                );
                assert_eq!(
                    suffix, row.suffix,
                    "case {case:?}: expected the suffix of the format the write named, got \
                     {suffix:?} in {basename:?}"
                );
                uuids.push(uuid);

                // WHAT the part holds: what the write rendered of the rows of its own
                // partition, with a partition column left to the directory.
                let bytes = body(&router, case, &format!("{prefix}{key}")).await;
                let text = String::from_utf8_lossy(&bytes).to_string();
                match row.holds {
                    Holds::Text => assert_eq!(
                        text, *expected,
                        "case {case:?}: expected {key} to hold {expected:?}, got {text:?}"
                    ),
                    Holds::Documents => {
                        let documents: Vec<Value> = text
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| document(case, line))
                            .collect();
                        let wanted: Vec<Value> = expected
                            .split('\n')
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| document(case, line))
                            .collect();
                        assert_eq!(
                            documents, wanted,
                            "case {case:?}: expected {key} to hold {wanted:?}, got {text:?}"
                        );
                    }
                    Holds::ParquetBytes => {
                        assert!(
                            bytes.starts_with(b"PAR1") && bytes.ends_with(b"PAR1"),
                            "case {case:?}: expected {key} to be an Apache Parquet file, got \
                             {} byte(s) beginning {:?}",
                            bytes.len(),
                            String::from_utf8_lossy(&bytes[..bytes.len().min(8)])
                        );
                    }
                }
            }
        }

        // ONE write is one write job: every part of it carries the same UUID.
        let first = uuids
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the write left no part file"));
        for uuid in &uuids {
            assert_eq!(
                *uuid, first,
                "case {case:?}: every part of one write carries that write's own UUID, got \
                 {uuids:?}"
            );
        }
        minted.push((case, first));
    }

    // ... and no two writes carry the same one, which is what attributes a file to the write
    // that produced it.
    for (index, (case, uuid)) in minted.iter().enumerate() {
        for (other, held) in &minted[index + 1..] {
            assert_ne!(
                uuid, held,
                "case {case:?}: expected a UUID of its own, got the one {other:?} carries \
                 ({uuid})"
            );
        }
    }
}
