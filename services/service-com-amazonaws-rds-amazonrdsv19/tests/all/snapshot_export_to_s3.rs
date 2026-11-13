// SPDX-License-Identifier: CC0-1.0

//! What a snapshot or cluster EXPORT to Amazon S3 leaves behind, and what it refuses.
//!
//! The rules this suite pins, each over a table spanning what it generalises over:
//!
//! * An export writes the SOURCE's own data: one Apache Parquet object per table of the
//!   database the source carries, under `<S3Prefix>/<ExportTaskIdentifier>/<database>/
//!   <schema>.<table>/`. Every source kind `SourceArn` may name carries that data — a DB
//!   snapshot and a DB cluster snapshot each hold the copy the engine took when the
//!   snapshot was taken, and a DB cluster holds its live cluster volume — so the kind of
//!   source decides nothing about whether the data is written.
//! * A completed export also writes its own SUMMARY object,
//!   `export_info_<ExportTaskIdentifier>.json`, describing the export as a whole: what it
//!   read, where it wrote, the status it ended in and the per-table row counts.
//! * A column the source declared an EXACT numeric type is exported as a Parquet DECIMAL
//!   carrying that column's precision and scale, so an exact value stays exact; the
//!   approximate and integer types keep their own Parquet types.
//! * Each refusal of an export carries the status, the code and the SENTENCE Amazon RDS
//!   words that error class in, with the request's own values in its variable parts.
//!
//! Amazon S3, IAM and KMS are mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, and every object asserted here is read back through Amazon
//! S3's own `ListObjectsV2` and `GetObject`, so what this suite finds is what that service
//! really holds.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parquet::basic::{ConvertedType, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::RowAccessor;
use serde_json::Value;
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_instances::{call, expect_error, expect_ok, text, REGION};
use crate::rds_s3_integration::{create_bucket, emulator, kms, put_object_policy, role, s3};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second Region, for the row that names a bucket outside the export's own.
const OTHER_REGION: &str = "eu-west-1";

/// The master user every source below is created with, and its password — test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The database the exported tables live in.
const DATABASE: &str = "appdb";

/// How long a poll loop here is given before it calls a transition unobservable: generous
/// next to the span this service owes one after, so a loaded machine cannot fail the suite.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// The gap between two polls of one loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Poll `read` until `settled` holds, answering the body it settled on; panics with the last
/// answer when [`POLL_DEADLINE`] passes first. The loop waits for the CONDITION, so a faster
/// or slower machine only changes how many times it looks.
async fn poll_until<Read, Fut>(case: &str, what: &str, read: Read, settled: impl Fn(&str) -> bool) -> String
where
    Read: Fn() -> Fut,
    Fut: std::future::Future<Output = (u16, String)>,
{
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = read().await;
        if status == 200 && settled(&body) {
            return body;
        }
        last = format!("HTTP {status}: {body}");
        sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {what} never settled in {POLL_DEADLINE:?}; last answer: {last}")
}

/// One table a source is seeded with: its name, the statement that creates it and the rows
/// inserted into it.
struct SeedTable {
    table: &'static str,
    create: &'static str,
    rows: &'static [&'static str],
}

/// The tables every source of the layout table carries: one holding rows and one holding
/// none, so the export is read over a table with data and a table without.
const SEEDED: &[SeedTable] = &[
    SeedTable {
        table: "ledger",
        create: "CREATE TABLE ledger (id INTEGER, label VARCHAR(20))",
        rows: &[
            "INSERT INTO ledger VALUES (1, 'north')",
            "INSERT INTO ledger VALUES (2, 'south')",
        ],
    },
    SeedTable {
        table: "audit",
        create: "CREATE TABLE audit (note TEXT)",
        rows: &[],
    },
];

/// The columns of the exact-numeric table: the declared type, and the Parquet type and
/// `(precision, scale)` the export has to carry it as.
///
/// The exact types are the ones the engines publish as synonyms — "DEC, NUMERIC, and FIXED
/// are synonyms for DECIMAL", and `numeric` is PostgreSQL's own spelling — with and without
/// declared arguments, at the published bound of a Parquet decimal (38 digits), one past it,
/// and with the approximate and integer types beside them as the controls.
struct NumericColumn {
    column: &'static str,
    declared: &'static str,
    /// The Parquet physical type the column is written as.
    physical: PhysicalType,
    /// The converted type a reader reads its bytes through.
    converted: ConvertedType,
    /// `(precision, scale)`, zero for a column that is no decimal.
    precision_and_scale: (i32, i32),
    /// The value the one seeded row holds in this column, as the source's own literal.
    literal: &'static str,
    /// The UNSCALED integer a decimal column must carry that value as.
    unscaled: i128,
}

const NUMERIC_COLUMNS: &[NumericColumn] = &[
    NumericColumn {
        column: "price",
        declared: "DECIMAL(10,2)",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
        precision_and_scale: (10, 2),
        literal: "10.57",
        unscaled: 1057,
    },
    NumericColumn {
        column: "score",
        declared: "NUMERIC(12,4)",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
        precision_and_scale: (12, 4),
        literal: "-3.1416",
        unscaled: -31_416,
    },
    NumericColumn {
        column: "quantity",
        declared: "DEC",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
        // No arguments: the engines' own default for one, `DECIMAL(10, 0)`.
        precision_and_scale: (10, 0),
        literal: "42",
        unscaled: 42,
    },
    NumericColumn {
        column: "widest",
        declared: "FIXED(38,10)",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
        // The published bound of a Parquet decimal: 38 digits. Its unscaled value here is
        // past what a 64-bit integer holds, so only the wide decimal carries it.
        precision_and_scale: (38, 10),
        literal: "1234567890.5",
        unscaled: 12_345_678_905_000_000_000,
    },
    NumericColumn {
        column: "duty",
        // An engine's own spelling of the same type, carrying an attribute beside the
        // arguments: the type and its digits are read whatever sits around them.
        declared: "UNSIGNED DECIMAL(8,3)",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
        precision_and_scale: (8, 3),
        literal: "4.125",
        unscaled: 4_125,
    },
    NumericColumn {
        column: "beyond",
        declared: "DECIMAL(39,2)",
        // One digit past that bound: the column is exported as its DIGITS, which hold every
        // one of them, rather than as a decimal that would drop some.
        physical: PhysicalType::BYTE_ARRAY,
        converted: ConvertedType::UTF8,
        precision_and_scale: (0, 0),
        literal: "'1.25'",
        unscaled: 0,
    },
    NumericColumn {
        column: "ratio",
        declared: "DOUBLE",
        physical: PhysicalType::DOUBLE,
        converted: ConvertedType::NONE,
        precision_and_scale: (0, 0),
        literal: "0.5",
        unscaled: 0,
    },
    NumericColumn {
        column: "counter",
        declared: "INTEGER",
        physical: PhysicalType::INT64,
        converted: ConvertedType::NONE,
        precision_and_scale: (0, 0),
        literal: "7",
        unscaled: 0,
    },
];

/// The destination an export writes to: the bucket, the key and the role, all held by the
/// services that own them.
struct Destination {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    bucket: String,
    role_arn: String,
    key_id: String,
}

/// Stand up one destination: a bucket of this Region, a KMS key and a role the export
/// principal may assume that is granted the object write.
async fn destination(bucket: &str, role_name: &str) -> Destination {
    let (router, engine) = emulator();
    create_bucket(&router, bucket, REGION).await;
    let key_id = kms(&router)
        .create_key()
        .description("the export's encryption key")
        .send()
        .await
        .expect("the emulated KMS creates the export key")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id");
    let role_arn = role(
        &router,
        role_name,
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(bucket)),
    )
    .await;
    Destination {
        router,
        engine,
        bucket: bucket.to_string(),
        role_arn,
        key_id,
    }
}

/// Seed `tables` into `database`, through the engine the source really runs on — so a
/// snapshot taken of it carries real rows.
fn seed(database: &st::data_plane::sql_engine::InstanceDatabase, tables: &[SeedTable]) {
    database
        .create_database(DATABASE)
        .expect("the source's database is created");
    for table in tables {
        database
            .execute(DATABASE, table.create, &[])
            .unwrap_or_else(|error| panic!("seeding {}: {}", table.table, error.message));
        for row in table.rows {
            database
                .execute(DATABASE, row, &[])
                .unwrap_or_else(|error| panic!("seeding {}: {}", table.table, error.message));
        }
    }
}

/// Create a DB instance, poll it to `available`, seed it and answer the engine database it
/// runs on.
async fn available_instance(destination: &Destination, case: &str, identifier: &str) {
    expect_ok(
        case,
        call(
            &destination.router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => identifier,
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
            ],
        )
        .await,
    );
    let request = params!["DBInstanceIdentifier" => identifier];
    poll_until(
        case,
        &format!("the DB instance {identifier} reaching available"),
        || call(&destination.router, "DescribeDBInstances", &request),
        |body| text(body, "DBInstanceStatus") == "available",
    )
    .await;
}

/// The same for a DB cluster.
async fn available_cluster(destination: &Destination, case: &str, identifier: &str) {
    expect_ok(
        case,
        call(
            &destination.router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => identifier,
                "Engine" => "aurora-mysql",
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
            ],
        )
        .await,
    );
    let request = params!["DBClusterIdentifier" => identifier];
    poll_until(
        case,
        &format!("the DB cluster {identifier} reaching available"),
        || call(&destination.router, "DescribeDBClusters", &request),
        |body| text(body, "Status") == "available",
    )
    .await;
}

/// The engine database a DB instance runs on.
fn instance_database(
    destination: &Destination,
    identifier: &str,
) -> Arc<st::data_plane::sql_engine::InstanceDatabase> {
    let state = st::service_state(&*destination.engine).expect("this service's store");
    let database = state
        .lock()
        .engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .map(st::DbEngineEndpoint::database);
    database.expect("a MySQL DB instance serves an engine endpoint")
}

/// The engine database a DB cluster's own volume holds.
fn cluster_database(
    destination: &Destination,
    identifier: &str,
) -> Arc<st::data_plane::sql_engine::InstanceDatabase> {
    let state = st::service_state(&*destination.engine).expect("this service's store");
    let database = state
        .lock()
        .cluster_engine_endpoint(&(REGION.to_string(), identifier.to_string()))
        .map(st::DbEngineEndpoint::database);
    database.expect("an Aurora MySQL DB cluster serves an engine endpoint")
}

/// Take a DB snapshot of `instance` and poll it to `available`, answering its ARN.
async fn available_snapshot(destination: &Destination, case: &str, instance: &str, snapshot: &str) -> String {
    expect_ok(
        case,
        call(
            &destination.router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => instance,
                "DBSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    let request = params!["DBSnapshotIdentifier" => snapshot];
    poll_until(
        case,
        &format!("the DB snapshot {snapshot} reaching available"),
        || call(&destination.router, "DescribeDBSnapshots", &request),
        |body| text(body, "Status") == "available",
    )
    .await;
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{snapshot}")
}

/// The same for a DB cluster snapshot.
async fn available_cluster_snapshot(
    destination: &Destination,
    case: &str,
    cluster: &str,
    snapshot: &str,
) -> String {
    expect_ok(
        case,
        call(
            &destination.router,
            "CreateDBClusterSnapshot",
            &params![
                "DBClusterIdentifier" => cluster,
                "DBClusterSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    let request = params!["DBClusterSnapshotIdentifier" => snapshot];
    poll_until(
        case,
        &format!("the DB cluster snapshot {snapshot} reaching available"),
        || call(&destination.router, "DescribeDBClusterSnapshots", &request),
        |body| text(body, "Status") == "available",
    )
    .await;
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:{snapshot}")
}

/// Start one export, as a client starts it.
async fn start_export(
    destination: &Destination,
    identifier: &str,
    source_arn: &str,
    prefix: Option<&str>,
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = params![
        "ExportTaskIdentifier" => identifier,
        "SourceArn" => source_arn,
        "S3BucketName" => destination.bucket,
        "IamRoleArn" => destination.role_arn,
        "KmsKeyId" => destination.key_id,
    ]
    .to_vec();
    if let Some(prefix) = prefix {
        request.push(("S3Prefix", prefix.to_string()));
    }
    call(&destination.router, "StartExportTask", &request).await
}

/// The keys the destination bucket holds under `prefix`, through Amazon S3's own listing.
async fn listed_keys(destination: &Destination, prefix: &str) -> Vec<String> {
    s3(&destination.router)
        .list_objects_v2()
        .bucket(&destination.bucket)
        .prefix(prefix)
        .send()
        .await
        .expect("Amazon S3 lists the destination bucket")
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The bytes Amazon S3 holds at `key`.
async fn object_bytes(destination: &Destination, key: &str) -> Vec<u8> {
    let answer = s3(&destination.router)
        .get_object()
        .bucket(&destination.bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("Amazon S3 holds the exported object {key}: {error}"));
    answer
        .body
        .collect()
        .await
        .expect("the exported object's bytes")
        .to_vec()
}

/// One row of the source-kind table: what the export reads and how its source is provisioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// `arn:aws:rds:…:snapshot:<id>` — a DB snapshot of a DB instance.
    DbSnapshot,
    /// `arn:aws:rds:…:cluster-snapshot:<id>` — a DB cluster snapshot.
    ClusterSnapshot,
    /// `arn:aws:rds:…:cluster:<id>` — the DB cluster itself.
    Cluster,
}

/// Rule: an export writes the SOURCE's own tables as Parquet objects under the export's
/// documented path, for every kind of source `SourceArn` may name — each of them carries the
/// data the export reads.
#[tokio::test]
async fn an_export_writes_a_parquet_object_for_every_table_of_every_source_kind() {
    const ROWS: &[(&str, SourceKind, Option<&str>)] = &[
        ("a DB snapshot, at the root of the bucket", SourceKind::DbSnapshot, None),
        ("a DB cluster snapshot, under a prefix", SourceKind::ClusterSnapshot, Some("nightly/exports")),
        ("a DB cluster, under a prefix", SourceKind::Cluster, Some("live")),
    ];

    for (index, (case, kind, prefix)) in ROWS.iter().enumerate() {
        let destination = destination(
            &format!("emu-layout-bucket-{index}"),
            &format!("emu-layout-role-{index}"),
        )
        .await;
        let source_arn = match kind {
            SourceKind::DbSnapshot => {
                let instance = "emu-layout-instance";
                available_instance(&destination, case, instance).await;
                seed(&instance_database(&destination, instance), SEEDED);
                available_snapshot(&destination, case, instance, "emu-layout-snapshot").await
            }
            SourceKind::ClusterSnapshot => {
                let cluster = "emu-layout-cluster-snap";
                available_cluster(&destination, case, cluster).await;
                seed(&cluster_database(&destination, cluster), SEEDED);
                available_cluster_snapshot(&destination, case, cluster, "emu-layout-cluster-snapshot")
                    .await
            }
            SourceKind::Cluster => {
                let cluster = "emu-layout-cluster";
                available_cluster(&destination, case, cluster).await;
                seed(&cluster_database(&destination, cluster), SEEDED);
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{cluster}")
            }
        };

        let identifier = format!("emu-layout-export-{index}");
        expect_ok(
            case,
            start_export(&destination, &identifier, &source_arn, *prefix).await,
        );
        let export_prefix = st::export_prefix(*prefix, &identifier);
        let keys = listed_keys(&destination, &export_prefix).await;
        for table in SEEDED {
            let table_prefix = format!("{export_prefix}{DATABASE}/{DATABASE}.{}/", table.table);
            let written: Vec<&String> = keys
                .iter()
                .filter(|key| key.starts_with(&table_prefix) && key.ends_with(".gz.parquet"))
                .collect();
            assert_eq!(
                written.len(),
                1,
                "case {case:?}: expected one Parquet object under {table_prefix}, got {keys:?}"
            );
            // And it is a real Parquet file carrying exactly the rows the source holds.
            let bytes = object_bytes(&destination, written[0]).await;
            let reader = SerializedFileReader::new(Bytes::from(bytes))
                .unwrap_or_else(|error| panic!("case {case:?}: {table_prefix} is Parquet: {error}"));
            let rows = reader.metadata().file_metadata().num_rows();
            assert_eq!(
                rows,
                table.rows.len() as i64,
                "case {case:?}: expected the source's own rows in {table_prefix}, got {rows}"
            );
        }
        // Beside the data, the two JSON files an export writes.
        let tables_info = st::tables_info_key(&export_prefix, &identifier, SEEDED.len());
        let export_info = st::export_info_key(&export_prefix, &identifier);
        for expected in [&tables_info, &export_info] {
            assert!(
                keys.contains(expected),
                "case {case:?}: expected the export's {expected} among {keys:?}"
            );
        }
    }
}

/// Rule: a completed export writes its own summary object beside the per-table files, and
/// that summary describes THIS export: what it read, where it wrote, the status it ended in
/// and the rows each table contributed.
///
/// The rows span a prefix given and omitted, and an export limited to one table, which is
/// what the summary's contents generalise over.
#[tokio::test]
async fn the_export_summary_object_describes_the_export_that_ran() {
    let destination = destination("emu-summary-bucket", "emu-summary-role").await;
    let instance = "emu-summary-instance";
    available_instance(&destination, "the export's source", instance).await;
    seed(&instance_database(&destination, instance), SEEDED);
    let source_arn =
        available_snapshot(&destination, "the export's source", instance, "emu-summary-snapshot")
            .await;

    const ROWS: &[(&str, Option<&str>, Option<&str>)] = &[
        ("the whole snapshot at the root of the bucket", None, None),
        ("the whole snapshot under a prefix", Some("exports/nightly"), None),
        ("one table of the snapshot", None, Some("appdb.ledger")),
    ];
    for (index, (case, prefix, export_only)) in ROWS.iter().enumerate() {
        let identifier = format!("emu-summary-export-{index}");
        let mut request: Vec<(&str, String)> = params![
            "ExportTaskIdentifier" => identifier,
            "SourceArn" => source_arn,
            "S3BucketName" => destination.bucket,
            "IamRoleArn" => destination.role_arn,
            "KmsKeyId" => destination.key_id,
        ]
        .to_vec();
        if let Some(prefix) = prefix {
            request.push(("S3Prefix", (*prefix).to_string()));
        }
        if let Some(entry) = export_only {
            request.push(("ExportOnly.member.1", (*entry).to_string()));
        }
        expect_ok(
            case,
            call(&destination.router, "StartExportTask", &request).await,
        );

        let export_prefix = st::export_prefix(*prefix, &identifier);
        let key = st::export_info_key(&export_prefix, &identifier);
        let keys = listed_keys(&destination, &export_prefix).await;
        assert!(
            keys.contains(&key),
            "case {case:?}: expected the export's summary at {key}, got {keys:?}"
        );
        let document: Value = serde_json::from_slice(&object_bytes(&destination, &key).await)
            .unwrap_or_else(|error| panic!("case {case:?}: the summary must parse: {error}"));
        for (member, expected) in [
            ("exportTaskIdentifier", Value::String(identifier.clone())),
            ("sourceArn", Value::String(source_arn.clone())),
            (
                "sourceType",
                Value::String(st::EXPORT_SOURCE_TYPE_SNAPSHOT.to_string()),
            ),
            (
                "s3Bucket",
                Value::String(destination.bucket.clone()),
            ),
            (
                "s3Prefix",
                Value::String(prefix.unwrap_or_default().to_string()),
            ),
            (
                "status",
                Value::String(st::EXPORT_TASK_STATUS_COMPLETE.to_string()),
            ),
        ] {
            assert_eq!(
                document.get(member),
                Some(&expected),
                "case {case:?}: expected {member} to be {expected}, got {document}"
            );
        }
        // One entry per table the export really wrote, each naming the rows it carried.
        let exported: Vec<&str> = match export_only {
            Some(entry) => vec![entry.rsplit('.').next().expect("a table name")],
            None => SEEDED.iter().map(|table| table.table).collect(),
        };
        let tables = document
            .get("tables")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the summary lists its tables, got {document}"));
        assert_eq!(
            tables.len(),
            exported.len(),
            "case {case:?}: expected one entry per exported table, got {document}"
        );
        for table in &exported {
            let entry = tables
                .iter()
                .find(|entry| entry.get("table") == Some(&Value::String((*table).to_string())))
                .unwrap_or_else(|| panic!("case {case:?}: expected {table} in {document}"));
            let seeded = SEEDED
                .iter()
                .find(|seeded| seeded.table == *table)
                .expect("a seeded table");
            assert_eq!(
                entry.get("rowsExported").and_then(Value::as_u64),
                Some(seeded.rows.len() as u64),
                "case {case:?}: expected the rows {table} holds, got {entry}"
            );
        }
    }
}

/// Rule: a column the source declared an EXACT numeric type is exported as a Parquet DECIMAL
/// of that column's precision and scale, and its values survive as the exact unscaled
/// integers that decimal carries; a column of an approximate or integer type keeps its own
/// Parquet type.
#[tokio::test]
async fn an_exact_numeric_column_is_exported_as_a_decimal_of_its_own_precision_and_scale() {
    let destination = destination("emu-decimal-bucket", "emu-decimal-role").await;
    let instance = "emu-decimal-instance";
    available_instance(&destination, "the decimal source", instance).await;
    {
        let database = instance_database(&destination, instance);
        database
            .create_database(DATABASE)
            .expect("the source's database is created");
        let columns: Vec<String> = NUMERIC_COLUMNS
            .iter()
            .map(|column| format!("{} {}", column.column, column.declared))
            .collect();
        database
            .execute(
                DATABASE,
                &format!("CREATE TABLE amounts ({})", columns.join(", ")),
                &[],
            )
            .unwrap_or_else(|error| panic!("creating amounts: {}", error.message));
        let literals: Vec<&str> = NUMERIC_COLUMNS.iter().map(|column| column.literal).collect();
        database
            .execute(
                DATABASE,
                &format!("INSERT INTO amounts VALUES ({})", literals.join(", ")),
                &[],
            )
            .unwrap_or_else(|error| panic!("seeding amounts: {}", error.message));
    }
    let source_arn = available_snapshot(
        &destination,
        "the decimal source",
        instance,
        "emu-decimal-snapshot",
    )
    .await;

    let identifier = "emu-decimal-export";
    expect_ok(
        "export the decimal table",
        start_export(&destination, identifier, &source_arn, None).await,
    );
    let export_prefix = st::export_prefix(None, identifier);
    let keys = listed_keys(&destination, &export_prefix).await;
    let key = keys
        .iter()
        .find(|key| key.contains(&format!("{DATABASE}.amounts/")))
        .unwrap_or_else(|| panic!("the amounts table was exported, got {keys:?}"))
        .clone();
    let bytes = object_bytes(&destination, &key).await;
    let reader = SerializedFileReader::new(Bytes::from(bytes))
        .unwrap_or_else(|error| panic!("the exported object is Parquet: {error}"));
    let schema = reader.metadata().file_metadata().schema_descr();
    let row = reader
        .get_row_iter(None)
        .expect("the exported rows read")
        .next()
        .expect("the one seeded row")
        .expect("the one seeded row reads");

    for (index, column) in NUMERIC_COLUMNS.iter().enumerate() {
        let case = column.column;
        let described = schema.column(index);
        assert_eq!(
            described.name(),
            column.column,
            "case {case:?}: expected the source's own column order, got {}",
            described.name()
        );
        assert_eq!(
            described.physical_type(),
            column.physical,
            "case {case:?}: expected the Parquet type {:?}, got {:?}",
            column.physical,
            described.physical_type()
        );
        assert_eq!(
            described.converted_type(),
            column.converted,
            "case {case:?}: expected the converted type {:?}, got {:?}",
            column.converted,
            described.converted_type()
        );
        if column.converted != ConvertedType::DECIMAL {
            continue;
        }
        assert_eq!(
            (described.type_precision(), described.type_scale()),
            column.precision_and_scale,
            "case {case:?}: expected the declared precision and scale {:?}, got ({}, {})",
            column.precision_and_scale,
            described.type_precision(),
            described.type_scale()
        );
        // The VALUE is the exact unscaled integer that decimal carries, so nothing of it was
        // rounded into a floating-point type.
        let decimal = row
            .get_decimal(index)
            .unwrap_or_else(|error| panic!("case {case:?}: the column holds a decimal: {error}"));
        let unscaled = bigdecimal::num_bigint::BigInt::from_signed_bytes_be(decimal.data());
        assert_eq!(
            unscaled.to_string(),
            column.unscaled.to_string(),
            "case {case:?}: expected the unscaled value {}, got {unscaled}",
            column.unscaled
        );
        assert_eq!(
            (decimal.precision(), decimal.scale()),
            column.precision_and_scale,
            "case {case:?}: expected the value's own precision and scale {:?}, got ({}, {})",
            column.precision_and_scale,
            decimal.precision(),
            decimal.scale()
        );
    }
}

/// One row of the refusal table: what the request names, and the status, code and sentence
/// the service answers it with.
struct RefusalRow {
    case: &'static str,
    /// The destination bucket the request names, defaulting to the fixture's own.
    bucket: Option<String>,
    /// The role ARN the request names, defaulting to the fixture's own.
    role_arn: Option<String>,
    /// The KMS key the request names, defaulting to the fixture's own.
    key_id: Option<String>,
    status: u16,
    code: &'static str,
    message: String,
}

/// Rule: every refusal of an export carries the status, the error code and the sentence
/// Amazon RDS words that error class in, with the request's own values — and only those —
/// in its variable parts.
///
/// The rows span the two conditions `InvalidS3BucketFault` covers (a bucket that does not
/// exist and one whose name is no bucket name), the bucket outside the export's Region that
/// is refused as the parameter value it is, the two conditions `IamRoleNotFound` covers (a
/// role IAM holds nothing under and one whose trust policy admits another principal) and the
/// two `KMSKeyNotAccessibleFault` covers (a key KMS holds nothing under and one whose own
/// policy authorizes the export role for nothing), each with its own bucket, role and key so
/// the variable parts differ from row to row.
#[tokio::test]
async fn each_export_refusal_carries_the_services_own_sentence() {
    let destination = destination("emu-refusal-bucket", "emu-refusal-role").await;
    let instance = "emu-refusal-instance";
    available_instance(&destination, "the refused exports' source", instance).await;
    seed(&instance_database(&destination, instance), SEEDED);
    let source_arn = available_snapshot(
        &destination,
        "the refused exports' source",
        instance,
        "emu-refusal-snapshot",
    )
    .await;

    // A bucket that exists, in another Region: a destination this export cannot write to.
    let elsewhere = "emu-refusal-elsewhere";
    create_bucket(&destination.router, elsewhere, OTHER_REGION).await;
    // A role IAM holds nothing under, and one it holds that trusts another service.
    let absent = format!("arn:aws:iam::{ACCOUNT}:role/emu-refusal-absent");
    let untrusting = role(
        &destination.router,
        "emu-refusal-untrusting",
        st::RDS_SERVICE_PRINCIPAL,
        Some(put_object_policy(&destination.bucket)),
    )
    .await;
    // A key whose own policy grants the export role nothing: in KMS no principal holds a
    // permission the key policy does not give it.
    let other_role = role(
        &destination.router,
        "emu-refusal-key-owner",
        st::EXPORT_SERVICE_PRINCIPAL,
        None,
    )
    .await;
    let closed_key = kms(&destination.router)
        .create_key()
        .policy(format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{other_role}"}},"Action":"kms:*","Resource":"*"}}]}}"#
        ))
        .bypass_policy_lockout_safety_check(true)
        .send()
        .await
        .expect("the emulated KMS creates the key with its own policy")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id");

    let rows = [
        RefusalRow {
            case: "a destination bucket Amazon S3 holds nothing under",
            bucket: Some("emu-refusal-no-such-bucket".to_string()),
            role_arn: None,
            key_id: None,
            status: 400,
            code: "InvalidS3BucketFault",
            message: "The S3 bucket emu-refusal-no-such-bucket isn't valid. Either it isn't \
                      located in the current AWS Region or it doesn't exist."
                .to_string(),
        },
        RefusalRow {
            case: "a destination bucket name that is no bucket name",
            bucket: Some("emu..refusal".to_string()),
            role_arn: None,
            key_id: None,
            status: 400,
            code: "InvalidS3BucketFault",
            message: "The S3 bucket emu..refusal isn't valid. Either it isn't located in the \
                      current AWS Region or it doesn't exist."
                .to_string(),
        },
        RefusalRow {
            case: "a destination bucket in another Region",
            bucket: Some(elsewhere.to_string()),
            role_arn: None,
            key_id: None,
            status: 400,
            code: "InvalidParameterValue",
            message: format!("The S3 bucket {elsewhere} isn't located in the current AWS Region."),
        },
        RefusalRow {
            case: "a role IAM holds nothing under",
            bucket: None,
            role_arn: Some(absent.clone()),
            key_id: None,
            status: 404,
            code: "IamRoleNotFound",
            message: format!(
                "The principal {} isn't allowed to assume the IAM role {absent} or the IAM \
                 role {absent} doesn't exist.",
                st::EXPORT_SERVICE_PRINCIPAL
            ),
        },
        RefusalRow {
            case: "a role whose trust policy admits another service principal",
            bucket: None,
            role_arn: Some(untrusting.clone()),
            key_id: None,
            status: 404,
            code: "IamRoleNotFound",
            message: format!(
                "The principal {} isn't allowed to assume the IAM role {untrusting} or the \
                 IAM role {untrusting} doesn't exist.",
                st::EXPORT_SERVICE_PRINCIPAL
            ),
        },
        RefusalRow {
            case: "a KMS key whose policy authorizes the export role for nothing",
            bucket: None,
            role_arn: None,
            key_id: Some(closed_key.clone()),
            status: 400,
            code: "KMSKeyNotAccessibleFault",
            message: format!(
                "The IAM role {} isn't authorized to call kms:CreateGrant on the KMS key \
                 {closed_key}.",
                destination.role_arn
            ),
        },
        RefusalRow {
            case: "a KMS key KMS holds nothing under",
            bucket: None,
            role_arn: None,
            key_id: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
            )),
            status: 400,
            code: "KMSKeyNotAccessibleFault",
            message: format!(
                "The KMS key arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-\
                 555555555555 doesn't exist or is disabled."
            ),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let body = expect_error(
            row.case,
            call(
                &destination.router,
                "StartExportTask",
                &params![
                    "ExportTaskIdentifier" => format!("emu-refusal-export-{index}"),
                    "SourceArn" => source_arn,
                    "S3BucketName" => row.bucket.clone().unwrap_or_else(|| destination.bucket.clone()),
                    "IamRoleArn" => row.role_arn.clone().unwrap_or_else(|| destination.role_arn.clone()),
                    "KmsKeyId" => row.key_id.clone().unwrap_or_else(|| destination.key_id.clone()),
                ],
            )
            .await,
            row.status,
            row.code,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, row.message,
            "case {:?}: expected the refusal sentence {:?}, got {message:?}",
            row.case, row.message
        );
    }

    // And nothing of a refused export was written or recorded.
    let keys = listed_keys(&destination, "").await;
    assert!(
        keys.is_empty(),
        "a refused export writes no object, got {keys:?}"
    );
}
