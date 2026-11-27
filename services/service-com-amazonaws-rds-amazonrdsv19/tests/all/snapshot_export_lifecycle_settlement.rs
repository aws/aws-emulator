// SPDX-License-Identifier: CC0-1.0

//! The RDS-to-S3 analytics path driven the way a CLIENT drives it: provision a DB instance,
//! fill a table over the engine's own wire, snapshot it, export the snapshot — and reach a
//! verdict on every stage with REPORTING calls alone.
//!
//! The rules this suite pins:
//!
//! * Every stage of that path that is reported in a NON-TERMINAL status reaches its terminal
//!   one on the CLOCK alone, so a client whose only calls are `DescribeDBInstances`,
//!   `DescribeDBSnapshots` and `DescribeExportTasks` — which is all a waiter or a provisioning
//!   script makes — observes `DBInstanceStatus` `available` with the instance's `Endpoint`, the
//!   snapshot's `Status` `available`, and the export task's `Status` settled on `COMPLETE` with
//!   `PercentProgress` 100 and a `TaskEndTime`, in bounded time and without any mutating call
//!   of its own.
//! * A settled export's objects are the SOURCE's own data, under the destination the request
//!   named: one Apache Parquet object per table of the source, below
//!   `<S3Prefix>/<ExportTaskIdentifier>/`, and the exported schema of a table a client declared
//!   over the engine's wire carries that table's columns — every column, by name, in the order
//!   it was declared, each typed for the kind of data its declared type holds.
//! * A `StartExportTask` whose `KmsKeyId` names a key the export may not perform
//!   `kms:CreateGrant` / `kms:DescribeKey` on is refused SYNCHRONOUSLY with the modelled
//!   `KMSKeyNotAccessibleFault` (HTTP 400), records no export task and writes no object — and
//!   the same snapshot exported to the same destination under a permitted key then settles on
//!   `COMPLETE` with its objects present, so a caller can branch on either outcome.
//!
//! The rows span what the rules generalise over: the forms `S3Prefix` takes (omitted, one
//! segment, several segments), the declared column types a driver writes, and the two spellings
//! `KmsKeyId` accepts for the unusable key.
//!
//! Amazon S3, IAM and KMS are mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, so the destination bucket, the export role and the keys are
//! held by the services that own them and every object asserted here is read back through
//! Amazon S3's own `ListObjectsV2` and `GetObject`. Every wait is a poll loop over a reporting
//! call ([`crate::lifecycle_poll_loops::poll_until`]), so what ends a loop is only a transition
//! this service's clock already owed and no assertion here is on how fast the host runs.

use bytes::Bytes;
use parquet::basic::{ConvertedType, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_engine_endpoint::{mysql_login, MySqlSession, Transport};
use crate::db_instances::{call, expect_error, expect_ok, optional_text, subtrees, text, REGION};
use crate::lifecycle_poll_loops::poll_until;
use crate::rds_s3_integration::{create_bucket, emulator, export_role_policy, kms, role, s3};

/// The canonical account id every generated emulator presents, which the key policy below is
/// written against.
const ACCOUNT: &str = "000000000000";

/// The master user every instance here is created with, and its password — test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The database the source instance is created with, which the client's table lives in.
const DATABASE: &str = "appdb";

/// The suffix every data object of an export carries: one GZIP-compressed Parquet part file
/// per table.
const PARQUET_SUFFIX: &str = ".gz.parquet";

/// The statuses an export task has STOPPED in, taken from the store's own list so this suite
/// and the handler cannot drift on what "stopped" means.
const TERMINAL: [&str; 3] = st::TERMINAL_STATUSES;

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The table the client declares over the engine's wire, and whose exported schema is read
/// back below.
const SOURCE_TABLE: &str = "ledger_entries";

/// One column of that table: how the client declared it, and the Parquet type an export of it
/// carries.
struct Column {
    name: &'static str,
    /// The type the client declares the column with, as a driver writes it.
    declared: &'static str,
    /// The Parquet physical type and converted (logical) type the exported column has to
    /// carry for the kind of data `declared` holds.
    physical: PhysicalType,
    converted: ConvertedType,
}

/// The columns of the source table, spanning the kinds of data a client's column holds, with
/// the Parquet type each kind is carried as.
///
/// Source of the type column: the Apache Parquet logical-type specification's mapping for each
/// kind of data — an EXACT numeric is a `DECIMAL` annotation over a fixed-length byte array,
/// which is the only Parquet type that keeps a declared precision and scale; an INSTANT is a
/// 64-bit `TIMESTAMP` in microseconds; an integer is a 64-bit integer, wide enough for every
/// value a declared integer column holds; an approximate numeric is a double; and character
/// data is a `UTF8`-annotated byte array. The rows deliberately reach past the types the
/// report cited (`BIGINT`, `TEXT`, `DOUBLE`), because the rule is about the kind of data a
/// column holds and not about one declaration.
const COLUMNS: &[Column] = &[
    Column {
        name: "entry_id",
        declared: "INT",
        physical: PhysicalType::INT64,
        converted: ConvertedType::NONE,
    },
    Column {
        name: "account_id",
        declared: "BIGINT",
        physical: PhysicalType::INT64,
        converted: ConvertedType::NONE,
    },
    Column {
        name: "label",
        declared: "VARCHAR(64)",
        physical: PhysicalType::BYTE_ARRAY,
        converted: ConvertedType::UTF8,
    },
    Column {
        name: "notes",
        declared: "TEXT",
        physical: PhysicalType::BYTE_ARRAY,
        converted: ConvertedType::UTF8,
    },
    Column {
        name: "amount",
        declared: "DECIMAL(12,4)",
        physical: PhysicalType::FIXED_LEN_BYTE_ARRAY,
        converted: ConvertedType::DECIMAL,
    },
    Column {
        name: "rate",
        declared: "DOUBLE",
        physical: PhysicalType::DOUBLE,
        converted: ConvertedType::NONE,
    },
    Column {
        name: "booked_at",
        declared: "DATETIME",
        physical: PhysicalType::INT64,
        converted: ConvertedType::TIMESTAMP_MICROS,
    },
];

/// The rows the client writes into that table — one value per column, in the declared order.
const SOURCE_ROWS: [&str; 2] = [
    "(1, 9000000001, 'opening', 'the first entry', 10.2500, 1.5, '2026-01-01 00:00:00')",
    "(2, 9000000002, 'closing', 'the second entry', 3.5000, 2.25, '2026-01-02 00:00:00')",
];

/// A key policy that grants the account root everything — the default key policy's own
/// statement, which delegates to IAM — and then DENIES every principal the two KMS operations
/// an export performs on the key it encrypts under ("the caller of this operation must be
/// authorized to run the following operations ... kms:CreateGrant kms:DescribeKey", `KmsKeyId`).
/// An explicit deny in any policy overrides any allow, so the deny decides for the export's
/// role as for anybody else.
fn key_policy_denying_the_export_operations() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{ACCOUNT}:root"}},"Action":"kms:*","Resource":"*"}},{{"Effect":"Deny","Principal":{{"AWS":"*"}},"Action":["kms:CreateGrant","kms:DescribeKey"],"Resource":"*"}}]}}"#
    )
}

/// Everything the analytics path needs, each resource held by the service that owns it.
struct Fixture {
    router: MultiServiceRouter,
    bucket: String,
    role_arn: String,
    /// A key the export may encrypt under: its key id and its key ARN.
    usable_key: (String, String),
    /// A key whose own policy withholds the export's KMS operations, in the same two
    /// spellings.
    denied_key: (String, String),
    /// The ARN of the snapshot every export below reads, as `DescribeDBSnapshots` reported it.
    source_arn: String,
}

/// Run `sql` in the client's session, failing the case when the engine refuses it.
async fn run(session: &mut MySqlSession, sql: &str) {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("{sql:?} must run, got {refusal:?}"));
}

/// Drive the whole source path with reporting calls alone: create the DB instance, poll it to
/// the status a client connects in, declare and fill the table over the engine's wire, snapshot
/// it and poll the snapshot to the status an export reads. Stands the destination up beside it.
async fn fixture(slug: &str) -> Fixture {
    let (router, _engine) = emulator();
    let instance = format!("emu-{slug}-instance");
    let snapshot = format!("emu-{slug}-snapshot");

    expect_ok(
        slug,
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => instance,
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
                "DBName" => DATABASE,
            ],
        )
        .await,
    );
    // Stage one of the path, waited out the way a client waits it out: `DescribeDBInstances`
    // and nothing else, until the instance reports the status a client may connect in.
    let describe_instance = params!["DBInstanceIdentifier" => instance];
    let (available, _polls) = poll_until(
        slug,
        "the source DB instance reaching the status a client connects in",
        || call(&router, "DescribeDBInstances", &describe_instance),
        |status, body| {
            status == 200 && text(body, "DBInstanceStatus") == st::DB_INSTANCE_STATUS_AVAILABLE
        },
    )
    .await;
    let endpoint = subtrees(&available, "Endpoint")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("case {slug:?}: an available DB instance publishes an Endpoint, got {available}")
        });
    let address = text(&endpoint, "Address");
    let port: u16 = text(&endpoint, "Port")
        .parse()
        .unwrap_or_else(|_| panic!("case {slug:?}: the Endpoint publishes a port, got {endpoint}"));

    // The table a client declares and fills on that endpoint, over the engine's own wire.
    let mut session = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| panic!("case {slug:?}: the master user is admitted, got {refusal:?}"));
    let declared: Vec<String> = COLUMNS
        .iter()
        .map(|column| format!("{} {}", column.name, column.declared))
        .collect();
    run(
        &mut session,
        &format!("CREATE TABLE {SOURCE_TABLE} ({})", declared.join(", ")),
    )
    .await;
    for values in SOURCE_ROWS {
        run(
            &mut session,
            &format!("INSERT INTO {SOURCE_TABLE} VALUES {values}"),
        )
        .await;
    }

    // Stage two: the snapshot of that instance, polled to the status an export reads.
    expect_ok(
        slug,
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => instance,
                "DBSnapshotIdentifier" => snapshot,
            ],
        )
        .await,
    );
    let describe_snapshot = params!["DBSnapshotIdentifier" => snapshot];
    let (settled, _polls) = poll_until(
        slug,
        "the source DB snapshot reaching the status an export reads",
        || call(&router, "DescribeDBSnapshots", &describe_snapshot),
        |status, body| status == 200 && text(body, "Status") == st::SNAPSHOT_STATUS_AVAILABLE,
    )
    .await;
    let source_arn = text(&settled, "DBSnapshotArn");
    assert!(
        !source_arn.is_empty(),
        "case {slug:?}: an available snapshot reports its ARN, got {settled}"
    );

    // The destination: the bucket, the export role, and the two keys.
    let bucket = format!("emu-{slug}-bucket");
    create_bucket(&router, &bucket, REGION).await;
    let role_arn = role(
        &router,
        &format!("emu-{slug}-role"),
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(export_role_policy(&bucket)),
    )
    .await;
    let usable = kms(&router)
        .create_key()
        .description("the key the export encrypts under")
        .send()
        .await
        .expect("the emulated KMS creates the usable key")
        .key_metadata
        .expect("the created key reports its metadata");
    let denied = kms(&router)
        .create_key()
        .description("the key the export may not use")
        .policy(key_policy_denying_the_export_operations())
        // A policy that names a principal other than this account's own calls for the lockout
        // check to be bypassed, exactly as it does against the real service.
        .bypass_policy_lockout_safety_check(true)
        .send()
        .await
        .expect("the emulated KMS creates the denied key")
        .key_metadata
        .expect("the created key reports its metadata");

    Fixture {
        router,
        bucket,
        role_arn,
        usable_key: (
            usable.key_id().to_string(),
            usable.arn().expect("the usable key's ARN").to_string(),
        ),
        denied_key: (
            denied.key_id().to_string(),
            denied.arn().expect("the denied key's ARN").to_string(),
        ),
        source_arn,
    }
}

impl Fixture {
    /// One `StartExportTask` of the fixture's snapshot, naming the key reference and the
    /// `S3Prefix` a row carries.
    async fn start_export(
        &self,
        identifier: &str,
        key_reference: &str,
        prefix: Option<&str>,
    ) -> (u16, String) {
        let mut request: Vec<(&str, String)> = params![
            "ExportTaskIdentifier" => identifier,
            "SourceArn" => self.source_arn,
            "S3BucketName" => self.bucket,
            "IamRoleArn" => self.role_arn,
            "KmsKeyId" => key_reference,
        ]
        .to_vec();
        if let Some(prefix) = prefix {
            request.push(("S3Prefix", prefix.to_string()));
        }
        call(&self.router, "StartExportTask", &request).await
    }

    /// Poll `DescribeExportTasks` — a REPORTING call and nothing else — until the task it names
    /// has stopped, and answer the report it settled on.
    async fn settled_task(&self, case: &str, identifier: &str) -> String {
        let request = params!["ExportTaskIdentifier" => identifier];
        let (body, _polls) = poll_until(
            case,
            &format!("the export task {identifier} reaching a terminal status"),
            || call(&self.router, "DescribeExportTasks", &request),
            |status, body| status == 200 && TERMINAL.contains(&text(body, "Status").as_str()),
        )
        .await;
        body
    }

    /// The `ExportTaskIdentifier`s this Region's `DescribeExportTasks` reports.
    async fn recorded_tasks(&self) -> Vec<String> {
        let described = expect_ok(
            "the recorded export tasks",
            call(
                &self.router,
                "DescribeExportTasks",
                &params!["MaxRecords" => 20],
            )
            .await,
        );
        subtrees(&described, "ExportTask")
            .iter()
            .map(|task| text(task, "ExportTaskIdentifier"))
            .collect()
    }

    /// The keys Amazon S3 holds in the destination bucket under `prefix`, through that
    /// service's own listing.
    async fn listed_keys(&self, prefix: &str) -> Vec<String> {
        s3(&self.router)
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await
            .expect("Amazon S3 lists the destination bucket")
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect()
    }

    /// The bytes Amazon S3 holds under one key.
    async fn object_bytes(&self, key: &str) -> Vec<u8> {
        s3(&self.router)
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("Amazon S3 holds the exported object {key}: {error}"))
            .body
            .collect()
            .await
            .expect("the exported object's bytes are read")
            .to_vec()
    }
}

/// One row of the settlement table: where the request asks the export to be written, and where
/// its objects therefore land.
struct PrefixRow {
    case: &'static str,
    identifier: &'static str,
    /// `S3Prefix`, given or omitted.
    prefix: Option<&'static str>,
    /// Where every object of the export has to sit: `<S3Prefix>/<ExportTaskIdentifier>/`, and
    /// `<ExportTaskIdentifier>/` for a request that named no prefix — written out here rather
    /// than computed, so the assertion is not the code under test restated.
    under: &'static str,
}

const PREFIX_ROWS: &[PrefixRow] = &[
    PrefixRow {
        case: "no prefix at all",
        identifier: "emu-settles-rooted",
        prefix: None,
        under: "emu-settles-rooted/",
    },
    PrefixRow {
        case: "a prefix of one segment",
        identifier: "emu-settles-one-segment",
        prefix: Some("exports"),
        under: "exports/emu-settles-one-segment/",
    },
    PrefixRow {
        case: "a prefix of several segments",
        identifier: "emu-settles-nested",
        prefix: Some("exports/nightly/2026"),
        under: "exports/nightly/2026/emu-settles-nested/",
    },
];

/// Assert the exported Parquet object of the client's table, under `under`, carries that
/// table's own columns: every one of them, by name, in the order they were declared, each
/// typed for the kind of data its declaration holds.
async fn assert_exported_schema(fixture: &Fixture, case: &str, under: &str) {
    let table_prefix = format!("{under}{DATABASE}/{DATABASE}.{SOURCE_TABLE}/");
    let keys = fixture.listed_keys(&table_prefix).await;
    let key = keys
        .iter()
        .find(|key| key.ends_with(PARQUET_SUFFIX))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Parquet object under {table_prefix}, got {keys:?}")
        })
        .clone();

    let bytes = fixture.object_bytes(&key).await;
    let reader = SerializedFileReader::new(Bytes::from(bytes))
        .unwrap_or_else(|error| panic!("case {case:?}: the object {key} is Parquet: {error}"));
    let schema = reader.metadata().file_metadata().schema_descr();

    let exported: Vec<String> = (0..schema.num_columns())
        .map(|index| schema.column(index).name().to_string())
        .collect();
    let declared: Vec<String> = COLUMNS
        .iter()
        .map(|column| column.name.to_string())
        .collect();
    assert_eq!(
        exported, declared,
        "case {case:?}: expected the source table's columns in the order declared, got \
         {exported:?}"
    );

    for (index, column) in COLUMNS.iter().enumerate() {
        let descriptor = schema.column(index);
        let found = (descriptor.physical_type(), descriptor.converted_type());
        assert_eq!(
            found,
            (column.physical, column.converted),
            "case {case:?}: expected the column {} declared {} to be exported as {} with \
             converted type {}, got {} with converted type {}",
            column.name,
            column.declared,
            column.physical,
            column.converted,
            found.0,
            found.1
        );
    }

    let rows = reader
        .metadata()
        .file_metadata()
        .num_rows();
    assert_eq!(
        rows,
        SOURCE_ROWS.len() as i64,
        "case {case:?}: expected the {} rows the source holds in {key}, got {rows}",
        SOURCE_ROWS.len()
    );
}

/// Rule: every non-terminal stage of the snapshot-export path settles on the CLOCK alone, so a
/// client that only REPORTS sees the export task reach `COMPLETE` with all of its progress and
/// an end time, and the objects it left are the source's own tables under the destination the
/// request named — with the exported schema of a table the client declared over the engine's
/// wire carrying that table's columns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_snapshot_export_lifecycle_settles_through_reporting_calls_alone() {
    let fixture = fixture("settles").await;

    for row in PREFIX_ROWS {
        let started = expect_ok(
            row.case,
            fixture
                .start_export(row.identifier, &fixture.usable_key.0, row.prefix)
                .await,
        );
        // The values `ExportTask$Status` and `ExportTask$PercentProgress` publish are AWS's, so
        // the accepted task's own are written out here rather than read from this crate.
        let accepted = text(&started, "Status");
        assert_eq!(
            accepted, "STARTING",
            "case {:?}: expected an accepted export to report \"STARTING\", got {accepted:?} in \
             {started}",
            row.case
        );

        let settled = fixture.settled_task(row.case, row.identifier).await;
        let status = text(&settled, "Status");
        assert_eq!(
            status, "COMPLETE",
            "case {:?}: expected the export to settle on \"COMPLETE\", got {status:?} in \
             {settled}",
            row.case
        );
        let progress = text(&settled, "PercentProgress");
        assert_eq!(
            progress, "100",
            "case {:?}: expected PercentProgress 100 at COMPLETE, got {progress:?}",
            row.case
        );
        let ended = optional_text(&settled, "TaskEndTime");
        assert!(
            ended.is_some(),
            "case {:?}: a task that has stopped reports when it stopped, got {settled}",
            row.case
        );

        // Where the export's objects landed: under the destination the request named, and
        // nowhere else.
        let under = row.under;
        let keys = fixture.listed_keys(under).await;
        assert!(
            !keys.is_empty(),
            "case {:?}: expected the export's objects under {under:?}, got {keys:?}",
            row.case
        );
        assert!(
            keys.iter().all(|key| key.starts_with(under)),
            "case {:?}: every object of the export sits under {under:?}, got {keys:?}",
            row.case
        );

        assert_exported_schema(&fixture, row.case, under).await;
    }
}

/// One row of the unusable-key table: the spelling `KmsKeyId` carries for the denied key.
struct DeniedKeyRow {
    case: &'static str,
    identifier: &'static str,
    /// Whether the request spells the key as its ARN rather than as its bare key id.
    by_arn: bool,
}

const DENIED_KEY_ROWS: &[DeniedKeyRow] = &[
    DeniedKeyRow {
        case: "the denied key by key id",
        identifier: "emu-denied-by-id",
        by_arn: false,
    },
    DeniedKeyRow {
        case: "the denied key by key ARN",
        identifier: "emu-denied-by-arn",
        by_arn: true,
    },
];

/// Rule: a `KmsKeyId` whose key withholds the KMS operations an export performs on it is
/// refused synchronously with the modelled `KMSKeyNotAccessibleFault` (HTTP 400) and records
/// nothing and writes nothing; the same snapshot exported to the same destination under a
/// permitted key then settles on `COMPLETE` with its objects present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_key_export_is_refused_and_the_permitted_key_export_of_the_same_snapshot_settles()
{
    let fixture = fixture("denied").await;
    // The one destination prefix both the refused attempts and the control export name, so the
    // refusal is shown to leave untouched the very place the permitted export then fills.
    const PREFIX: &str = "exports/attempted";

    for row in DENIED_KEY_ROWS {
        let key_reference = if row.by_arn {
            &fixture.denied_key.1
        } else {
            &fixture.denied_key.0
        };
        let refused = expect_error(
            row.case,
            fixture
                .start_export(row.identifier, key_reference, Some(PREFIX))
                .await,
            400,
            "KMSKeyNotAccessibleFault",
        );
        let message = text(&refused, "Message");
        let expected = format!("The KMS key {key_reference} doesn't exist or is disabled.");
        assert_eq!(
            message, expected,
            "case {:?}: expected the refusal sentence {expected:?}, got {message:?}",
            row.case
        );
    }

    let recorded = fixture.recorded_tasks().await;
    assert!(
        recorded.is_empty(),
        "a refused export records no export task, got {recorded:?}"
    );
    let written = fixture.listed_keys("").await;
    assert!(
        written.is_empty(),
        "a refused export writes no object, got {written:?}"
    );

    // And the control the client runs next: the same snapshot, the same destination, a key the
    // export may use — which settles, with its objects where the request asked for them.
    const CONTROL: &str = "emu-denied-control";
    let control_under = format!("{PREFIX}/{CONTROL}/");
    expect_ok(
        "the permitted-key control export",
        fixture
            .start_export(CONTROL, &fixture.usable_key.1, Some(PREFIX))
            .await,
    );
    let settled = fixture
        .settled_task("the permitted-key control export", CONTROL)
        .await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, "COMPLETE",
        "case \"the permitted-key control export\": expected \"COMPLETE\", got {status:?} in \
         {settled}"
    );
    let keys = fixture.listed_keys(&control_under).await;
    assert!(
        !keys.is_empty(),
        "case \"the permitted-key control export\": expected the export's objects under \
         {control_under:?}, got {keys:?}"
    );
    assert_exported_schema(
        &fixture,
        "the permitted-key control export",
        &control_under,
    )
    .await;
}
