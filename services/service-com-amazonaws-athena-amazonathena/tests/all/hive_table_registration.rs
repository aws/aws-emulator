// SPDX-License-Identifier: CC0-1.0

//! What the Data Catalog HOLDS for a table registered through Athena's Hive DDL, as Glue's
//! own reads report it.
//!
//! Two rules are under test here.
//!
//! * A table a Hive `CREATE EXTERNAL TABLE` registers is defined by a whole Hive table
//!   object, so every collection of that definition the statement declares nothing for is
//!   stored EMPTY rather than left unknown, and the Hive property that records when DDL
//!   last wrote the definition is stored with it. `Table$PartitionKeys` states the rule for
//!   the partition keys — "When you create a table used by Amazon Athena, and you do not
//!   specify any `partitionKeys`, you must at least set the value of `partitionKeys` to an
//!   empty list" — and the collections of `StorageDescriptor` (`BucketColumns`,
//!   `Parameters`, `SkewedInfo` and its own three members) and `Table$LastAccessTime` ("the
//!   last time that the table was accessed. This is usually taken from HDFS") go with it: a
//!   table nothing has read yet was accessed at the epoch. So a client that round-trips
//!   such a table reads `[]` and `{}` where there is nothing, never a null it cannot tell
//!   from "unknown".
//! * An external table's `LOCATION` is "the physical location of the table"
//!   (`StorageDescriptor$Location`): storage that already exists. A statement whose location
//!   names a bucket the object store does not hold registers nothing and settles FAILED with
//!   the category and error type Athena publishes for that class.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: whether the
//! statement declares partition keys, table properties, a row format or a storage format,
//! whether its location closes with a separator, and — for the location rule — whether the
//! bucket exists, whether the key looks like a folder or a file, and whether the location is
//! an object-store path at all.
//!
//! Every call goes over the wire the production front door uses: Athena and Glue through
//! `POST /` with `X-Amz-Target` and Amazon S3 through its own REST data plane, all on one
//! `MultiServiceRouter` over ONE shared engine, so what the DDL wrote is read back exactly
//! as a client reads it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::catalog_sql::{ddl, ERROR_CATEGORY_USER};
use service_com_amazonaws_athena_amazonathena::state::{
    QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use service_com_amazonaws_glue_awsglue::state as glue_state;
use smithy_server_runtime::MultiServiceRouter;

/// The name Athena reaches the account's AWS Glue Data Catalog by.
const CATALOG: &str = "AwsDataCatalog";

/// The bucket the tables of these suites keep their data in.
const BUCKET: &str = "athena-hive-registration";

/// The database the statements run in.
const DATABASE: &str = "hive_shop";

/// Where a query's results are written, which every submission has to name.
const OUTPUT_LOCATION: &str = "s3://athena-hive-registration/results/";

/// How long past [`QUERY_EXECUTION_COMPLETION`] a suite waits for a started statement to be
/// readable in its terminal state.
const SETTLE_GRACE: Duration = Duration::from_millis(400);

/// The `AthenaError$ErrorType` of the location class the absent-bucket refusal belongs to.
const ERROR_TYPE_LOCATION_NOT_FOUND: i64 = 1306;

/// Athena's wording for a `LOCATION` whose bucket the object store does not hold.
const BUCKET_ABSENT_MESSAGE: &str = "The bucket specified in the ddl statement does not exist";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
}

/// Athena, Glue and Amazon S3 mounted on ONE shared engine, the way the router mounts every
/// service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_athena_amazonathena::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_1 call to `service`, and its HTTP status and parsed body.
async fn call(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.us-east-1.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header("authorization", authorization(service))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Athena call, as a client makes it.
async fn athena(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "athena", "AmazonAthena", operation, body).await
}

/// One Glue call, as a client makes it.
async fn glue(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call(router, "glue", "AWSGlue", operation, body).await
}

/// Create the bucket the tables' data lives in.
async fn create_bucket(router: &MultiServiceRouter) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{BUCKET}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "setup: creating s3://{BUCKET} must succeed, got {} with {}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create the database the statements run in.
async fn create_database(router: &MultiServiceRouter) {
    let (status, created) = glue(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: creating the database must succeed, got {status} with {created:?}"
    );
}

/// Start one statement in the context database and return the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, sql: &str) -> String {
    let (status, answer) = athena(
        router,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": DATABASE, "Catalog": CATALOG },
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartQueryExecution({sql}) must be accepted, got {status} with {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {answer:?}"))
        .to_string()
}

/// Wait out the span a completion is owed after, once for a whole suite.
async fn settle() {
    ::tokio::time::sleep(QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).await;
}

/// The stored `QueryExecution` of `id`.
async fn execution(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = athena(
        router,
        "GetQueryExecution",
        &json!({ "QueryExecutionId": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueryExecution must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// The stored Glue table `DATABASE`.`table` as `GetTable` reports it, or `None` when the
/// catalog holds none.
async fn glue_table(router: &MultiServiceRouter, table: &str) -> Option<Value> {
    let (status, answer) = glue(
        router,
        "GetTable",
        &json!({ "DatabaseName": DATABASE, "Name": table }),
    )
    .await;
    (status == 200).then(|| answer["Table"].clone())
}

/// The entry for `table` in the listing `GetTables` answers for the database, or `None`
/// when the listing holds none.
async fn listed_table(router: &MultiServiceRouter, table: &str) -> Option<Value> {
    let (status, answer) = glue(
        router,
        "GetTables",
        &json!({ "DatabaseName": DATABASE, "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetTables must answer 200, got {status} with {answer:?}"
    );
    answer["TableList"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|listed| listed["Name"].as_str() == Some(table))
        .cloned()
}

/// Seconds from the epoch now, which bounds the DDL time a statement submitted after this
/// instant can have recorded.
fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One row of the registration table: a `CREATE EXTERNAL TABLE` and what the catalog must
/// then hold for the members the statement declares nothing of.
struct RegistrationCase {
    /// What this row is about.
    case: &'static str,
    /// The table the statement defines.
    table: &'static str,
    /// The statement submitted.
    sql: &'static str,
    /// The `StorageDescriptor.Location` the entry must carry.
    location: &'static str,
    /// The partition keys it must carry, as `(name, type)` — empty for a statement that
    /// declares none, which is stored as an EMPTY list rather than left unknown.
    partition_keys: Vec<(&'static str, &'static str)>,
}

/// A table registered through Hive DDL carries the Hive metastore's own defaults for
/// everything the statement declares nothing of, on every catalog read of it: `GetTables`
/// and `GetTable` report the same present-and-empty partition keys, bucket columns,
/// descriptor parameters and skew information, the epoch as the last access time, and the
/// Hive DDL-time property beside the properties the statement declared.
#[tokio::test]
async fn hive_ddl_registers_the_hive_compatibility_defaults_of_a_table() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router).await;

    // The rows differ along whether partition keys, table properties, a row format or a
    // storage format are declared, and whether the location closes with a separator.
    let cases: Vec<RegistrationCase> = vec![
        RegistrationCase {
            case: "a bare unpartitioned table whose location closes with a separator",
            table: "plain",
            sql: "CREATE EXTERNAL TABLE plain (id bigint) \
                  LOCATION 's3://athena-hive-registration/plain/'",
            location: "s3://athena-hive-registration/plain",
            partition_keys: vec![],
        },
        RegistrationCase {
            case: "a delimited table with properties, the recorded form",
            table: "items",
            sql: "CREATE EXTERNAL TABLE items (item_id string, name string, price int) \
                  ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' \
                  LOCATION 's3://athena-hive-registration/products/' \
                  TBLPROPERTIES ('skip.header.line.count'='1')",
            location: "s3://athena-hive-registration/products",
            partition_keys: vec![],
        },
        RegistrationCase {
            case: "a partitioned table, whose declared keys are stored as declared",
            table: "daily",
            sql: "CREATE EXTERNAL TABLE daily (total double) PARTITIONED BY (day string) \
                  LOCATION 's3://athena-hive-registration/daily'",
            location: "s3://athena-hive-registration/daily",
            partition_keys: vec![("day", "string")],
        },
        RegistrationCase {
            case: "a table in a columnar storage format",
            table: "columnar",
            sql: "CREATE EXTERNAL TABLE columnar (id bigint) STORED AS PARQUET \
                  LOCATION 's3://athena-hive-registration/columnar/'",
            location: "s3://athena-hive-registration/columnar",
            partition_keys: vec![],
        },
        RegistrationCase {
            case: "a table under IF NOT EXISTS whose name the statement qualifies",
            table: "logs",
            sql: "CREATE EXTERNAL TABLE IF NOT EXISTS hive_shop.logs (line string) \
                  STORED AS TEXTFILE LOCATION 's3://athena-hive-registration/logs/'",
            location: "s3://athena-hive-registration/logs",
            partition_keys: vec![],
        },
    ];

    let submitted_after = epoch_seconds();
    let mut started: Vec<(&RegistrationCase, String)> = Vec::new();
    for case in &cases {
        started.push((case, start(&router, case.case, case.sql).await));
    }
    settle().await;

    for (case, id) in started {
        let found = execution(&router, case.case, &id).await;
        assert_eq!(
            found["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {:?}: the registration must succeed, got {found:?}",
            case.case
        );
        let read = glue_table(&router, case.table).await.unwrap_or_else(|| {
            panic!(
                "case {:?}: the catalog must hold {DATABASE}.{} after the DDL",
                case.case, case.table
            )
        });
        let listed = listed_table(&router, case.table).await.unwrap_or_else(|| {
            panic!(
                "case {:?}: GetTables must list {DATABASE}.{}",
                case.case, case.table
            )
        });
        // One catalog, so the listing and the single-table read report the same entry:
        // whatever is asserted below holds for both.
        assert_eq!(
            listed, read,
            "case {:?}: GetTables and GetTable must report the same table",
            case.case
        );

        for table in [&read, &listed] {
            assert_eq!(
                table["TableType"].as_str(),
                Some(ddl::TABLE_TYPE_EXTERNAL),
                "case {:?}: unexpected table type in {table:?}",
                case.case
            );
            // The location is the table's data PREFIX, without the separator that closed
            // it — never an individual object key.
            assert_eq!(
                table["StorageDescriptor"]["Location"].as_str(),
                Some(case.location),
                "case {:?}: expected the data prefix {}, got {table:?}",
                case.case,
                case.location
            );
            assert_eq!(
                table["PartitionKeys"].as_array().map(|keys| keys
                    .iter()
                    .map(|key| (
                        key["Name"].as_str().unwrap_or_default().to_string(),
                        key["Type"].as_str().unwrap_or_default().to_string(),
                    ))
                    .collect::<Vec<_>>()),
                Some(
                    case.partition_keys
                        .iter()
                        .map(|(name, data_type)| ((*name).to_string(), (*data_type).to_string()))
                        .collect::<Vec<_>>()
                ),
                "case {:?}: the partition keys must be a definite list, got {table:?}",
                case.case
            );
            assert_eq!(
                table["LastAccessTime"].as_f64(),
                Some(0.0),
                "case {:?}: a table nothing has read was accessed at the epoch, got {table:?}",
                case.case
            );
            assert_eq!(
                table["StorageDescriptor"]["BucketColumns"].as_array(),
                Some(&Vec::new()),
                "case {:?}: an unbucketed table has no bucket columns, got {table:?}",
                case.case
            );
            assert_eq!(
                table["StorageDescriptor"]["SortColumns"].as_array(),
                Some(&Vec::new()),
                "case {:?}: an unbucketed table has no sort order, got {table:?}",
                case.case
            );
            assert_eq!(
                table["StorageDescriptor"]["Parameters"].as_object(),
                Some(&serde_json::Map::new()),
                "case {:?}: the descriptor's own properties are empty, got {table:?}",
                case.case
            );
            assert_eq!(
                table["StorageDescriptor"]["SkewedInfo"],
                json!({
                    "SkewedColumnNames": [],
                    "SkewedColumnValues": [],
                    "SkewedColumnValueLocationMaps": {},
                }),
                "case {:?}: no value is recorded as skewed, got {table:?}",
                case.case
            );
            // The property the statement declared is stored beside the ones the
            // registration materialises, not instead of them.
            assert_eq!(
                table["Parameters"][ddl::EXTERNAL_PROPERTY].as_str(),
                Some(ddl::EXTERNAL_PROPERTY_VALUE),
                "case {:?}: an external table is marked as one, got {table:?}",
                case.case
            );
            let ddl_time = table["Parameters"][glue_state::HIVE_LAST_DDL_TIME_PARAMETER]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the Hive DDL time must be stored, got {table:?}",
                        case.case
                    )
                });
            let seconds: u64 = ddl_time.parse().unwrap_or_else(|_| {
                panic!(
                    "case {:?}: the Hive DDL time is an instant in seconds, got {ddl_time:?}",
                    case.case
                )
            });
            assert!(
                seconds >= submitted_after,
                "case {:?}: the DDL time is the instant the definition was written, got \
                 {seconds} before {submitted_after}",
                case.case
            );
        }
    }
}

/// One row of the location table: a `LOCATION` a `CREATE EXTERNAL TABLE` declares, and
/// whether the object store holds the bucket it names.
struct BucketCase {
    /// What this row is about.
    case: &'static str,
    /// The table the statement defines.
    table: &'static str,
    /// The `LOCATION` it declares, as submitted.
    location: &'static str,
    /// Whether that location names a bucket the object store does not hold, which is what
    /// refuses the statement.
    bucket_absent: bool,
}

/// An external table is registered over storage that EXISTS: a statement whose `LOCATION`
/// names a bucket the object store does not hold settles FAILED with the user category and
/// the published location error type, says so in the words Athena uses, and leaves the
/// catalog without the table — while a location in a bucket that does exist, and one that is
/// no object-store path at all, register it.
#[tokio::test]
async fn an_external_table_location_has_to_name_a_bucket_that_exists() {
    let router = emulator();
    create_bucket(&router).await;
    create_database(&router).await;

    // The rows differ along whether the bucket exists, whether the key names a folder, a
    // file or the bucket root, and whether the location is an object-store path at all.
    let cases: Vec<BucketCase> = vec![
        BucketCase {
            case: "a prefix in a bucket the object store does not hold",
            table: "absent_prefix",
            location: "s3://athena-hive-absent/products/",
            bucket_absent: true,
        },
        BucketCase {
            case: "a key with no trailing separator in a bucket that is not there",
            table: "absent_key",
            location: "s3://athena-hive-absent/products",
            bucket_absent: true,
        },
        BucketCase {
            case: "the root of a bucket that is not there",
            table: "absent_root",
            location: "s3://athena-hive-absent/",
            bucket_absent: true,
        },
        BucketCase {
            case: "a file name in a bucket that is not there: the bucket is the refusal",
            table: "absent_file",
            location: "s3://athena-hive-absent/products/items.csv",
            bucket_absent: true,
        },
        BucketCase {
            case: "a prefix in the bucket the object store holds",
            table: "present_prefix",
            location: "s3://athena-hive-registration/products/",
            bucket_absent: false,
        },
        BucketCase {
            case: "the root of the bucket the object store holds",
            table: "present_root",
            location: "s3://athena-hive-registration/",
            bucket_absent: false,
        },
        BucketCase {
            case: "a location that is no object-store path at all",
            table: "other_store",
            location: "hdfs://cluster/warehouse/items",
            bucket_absent: false,
        },
    ];

    let mut started: Vec<(&BucketCase, String)> = Vec::new();
    for case in &cases {
        let sql = format!(
            "CREATE EXTERNAL TABLE {} (item_id string, name string, price int) \
             ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION '{}'",
            case.table, case.location
        );
        started.push((case, start(&router, case.case, &sql).await));
    }
    settle().await;

    for (case, id) in started {
        let found = execution(&router, case.case, &id).await;
        let state = found["Status"]["State"].as_str();
        let held = glue_table(&router, case.table).await;
        if case.bucket_absent {
            assert_eq!(
                state,
                Some(QUERY_STATE_FAILED),
                "case {:?}: a location whose bucket is absent registers nothing, got {found:?}",
                case.case
            );
            assert_eq!(
                found["Status"]["AthenaError"]["ErrorCategory"].as_i64(),
                Some(i64::from(ERROR_CATEGORY_USER)),
                "case {:?}: the caller's location is the cause, got {found:?}",
                case.case
            );
            assert_eq!(
                found["Status"]["AthenaError"]["ErrorType"].as_i64(),
                Some(ERROR_TYPE_LOCATION_NOT_FOUND),
                "case {:?}: unexpected error type in {found:?}",
                case.case
            );
            assert_eq!(
                found["Status"]["AthenaError"]["ErrorMessage"].as_str(),
                Some(BUCKET_ABSENT_MESSAGE),
                "case {:?}: unexpected message in {found:?}",
                case.case
            );
            assert_eq!(
                found["Status"]["StateChangeReason"].as_str(),
                Some(BUCKET_ABSENT_MESSAGE),
                "case {:?}: the reason states the failure, got {found:?}",
                case.case
            );
            assert!(
                held.is_none(),
                "case {:?}: the refused statement must leave no table, got {held:?}",
                case.case
            );
        } else {
            assert_eq!(
                state,
                Some(QUERY_STATE_SUCCEEDED),
                "case {:?}: a location over storage that exists registers the table, got \
                 {found:?}",
                case.case
            );
            let held = held.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the catalog must hold {DATABASE}.{} after the DDL",
                    case.case, case.table
                )
            });
            assert_eq!(
                held["TableType"].as_str(),
                Some(ddl::TABLE_TYPE_EXTERNAL),
                "case {:?}: unexpected table type in {held:?}",
                case.case
            );
        }
    }
}
