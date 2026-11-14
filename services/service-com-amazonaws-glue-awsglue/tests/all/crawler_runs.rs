// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `StartCrawler` DOES: the crawl of a crawler's data stores, and
//! the Data Catalog tables it populates from what it found there.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the objects
//! a crawl reads are staged through Amazon S3's own `PutObject`, so what is asserted is HTTP
//! behaviour over state the data plane really holds.
//!
//! The rule under test: `StartCrawler` "starts a crawl using the specified crawler" — it
//! refuses a name no crawler in the account has (`EntityNotFoundException`) and a crawler
//! whose crawl is already in progress (`CrawlerRunningException`); otherwise the crawler
//! leaves the state with no run in it, the objects under each of its Amazon S3 targets are
//! walked, and each dataset found behind them is written into the crawler's target database
//! as one table whose columns are read off the data, whose partition keys are the folder
//! levels below the target (named off a Hive-style `key=value` segment where the path carries
//! one, by depth where it does not) and which carries one partition per distinct value of
//! those levels. The clock owes the crawler its way back to `READY` one span later, which is
//! when the crawl's `LastCrawl` and its table counts become readable.
//!
//! The recorded case — a crawler over one Amazon S3 prefix whose Hive-style folders are
//! detected as partitions — is ONE row of the table below; the rows differ along what the
//! rule generalises over: how many folder levels the objects sit at, whether those levels are
//! Hive-style, the layout of the data (comma-separated with and without a header, another
//! delimiter, one JSON document per line), whether the target path ends in the separator,
//! how many objects sit behind the dataset, and which of them the target's `Exclusions`
//! exclude.
//!
//! The due instant a crawl's finish falls at is moved THROUGH THE STORE rather than slept
//! for ([`set_due_instant`]), so every transition here is deterministic.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    crawl_target_unreadable, fold_name, ComAmazonawsGlueAwsglueState, CLASSIFICATION_CSV,
    CLASSIFICATION_JSON, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    CRAWL_WITHOUT_TARGET_DATABASE, DEPRECATED_BY_CRAWLER_PARAMETER, LAST_CRAWL_STATUS_FAILED,
    LAST_CRAWL_STATUS_SUCCEEDED, MAX_CRAWLED_EXCLUSIONS, UPDATED_BY_CRAWLER_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID};

/// The role every crawler here is created with.
pub(crate) const ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-crawl";

/// The database every crawl here writes its output into.
pub(crate) const DATABASE: &str = "crawled_db";

/// An emulator with Glue AND Amazon S3 mounted on one shared engine, plus the engine itself
/// so a test can move the due instant a crawl's finish falls at.
///
/// Amazon S3 is mounted because a crawl READS the objects under its targets: it reaches them
/// over the emulator's own in-process callback, so the objects a test stages with `PutObject`
/// are the objects the crawl classifies.
fn emulator_with_s3() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a crawl reads
/// through, so what a test stages here is what the crawl reads. The shared harness owns the
/// one client configuration ([`crate::wire::s3_client`]).
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    crate::wire::s3_client(router)
}

/// Create `bucket` and put every `(key, body)` into it.
pub(crate) async fn stage(router: &MultiServiceRouter, bucket: &str, objects: &[(&str, &str)]) {
    let client = s3(router);
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the bucket {bucket} must be created, got {error:?}"));
    put(router, bucket, objects).await;
}

/// Put further objects into a bucket that already exists.
async fn put(router: &MultiServiceRouter, bucket: &str, objects: &[(&str, &str)]) {
    let client = s3(router);
    for (key, body) in objects {
        client
            .put_object()
            .bucket(bucket)
            .key(*key)
            .body(aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("the object {key} must be stored, got {error:?}"));
    }
}

/// Create the database every crawl here writes into, tolerating one a previous row made.
pub(crate) async fn given_database(router: &MultiServiceRouter, name: &str) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
}

/// Create one crawler over the wire: the required members, the target `target` describes, and
/// whatever `extra` adds.
async fn given_crawler(router: &MultiServiceRouter, name: &str, target: Value, extra: Value) {
    given_crawler_over(router, name, json!([target]), extra).await;
}

/// The same, for a crawler carrying SEVERAL Amazon S3 targets: `targets` is the whole
/// `S3Targets` list.
pub(crate) async fn given_crawler_over(
    router: &MultiServiceRouter,
    name: &str,
    targets: Value,
    extra: Value,
) {
    let mut body = json!({
        "Name": name,
        "Role": ROLE,
        "DatabaseName": DATABASE,
        "Targets": { "S3Targets": targets },
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    ok(router, "CreateCrawler", &body).await;
}

/// Move the due instant of every crawl in flight to `at`, so a test can hold the transient
/// window open or make the owed finish due without sleeping.
pub(crate) fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for crawler in store.crawlers.values_mut() {
        if crawler.crawl_settles_at.is_some() {
            crawler.crawl_settles_at = Some(at);
        }
    }
}

/// An instant already past, so the next call finishes what the clock owed.
pub(crate) fn already_past() -> SystemTime {
    SystemTime::now() - Duration::from_secs(1)
}

/// An instant far enough out that no owed finish falls due during a test.
pub(crate) fn far_future() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// Start the crawl of `name` and make the finish the clock owes it due, so the next read
/// reports the crawl that finished.
async fn crawl(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>, name: &str) {
    let body = ok(router, "StartCrawler", &json!({ "Name": name })).await;
    assert_eq!(
        body,
        json!({}),
        "StartCrawler reports no member of its own, got {body}"
    );
    set_due_instant(engine, already_past());
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn crawler_of(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawler must report a Crawler, got {body}"))
}

/// The `Table` structure `GetTable` reports, or `None` when the catalog holds no such table.
async fn table_of(router: &MultiServiceRouter, database: &str, table: &str) -> Option<Value> {
    let (status, parsed) = call(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": table }),
    )
    .await;
    if status != 200 {
        return None;
    }
    parsed.get("Table").cloned()
}

/// The `(name, type)` of every column of one table's storage descriptor, in the order the
/// table declares them.
fn columns_of(table: &Value) -> Vec<(String, String)> {
    named_types(table.pointer("/StorageDescriptor/Columns"))
}

/// The `(name, type)` of every partition key of one table, in the order it declares them.
fn partition_keys_of(table: &Value) -> Vec<(String, String)> {
    named_types(table.get("PartitionKeys"))
}

/// The `(Name, Type)` pairs of a `Column` list, in the order it holds them.
fn named_types(columns: Option<&Value>) -> Vec<(String, String)> {
    columns
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .map(|column| {
                    (
                        column
                            .get("Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        column
                            .get("Type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One table parameter, or `None` when the table carries none of that name.
fn parameter_of(table: &Value, name: &str) -> Option<String> {
    table
        .pointer("/Parameters")
        .and_then(Value::as_object)
        .and_then(|parameters| parameters.get(name))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `Values` of every partition the catalog holds for one table, in the order it holds
/// them.
///
/// Read from the store rather than over the wire because the operation that PAGES over a
/// table's partitions (`GetPartitions`) is still a scaffold; every partition this answers is
/// also read back over the wire, one at a time, by [`served_partition`].
fn stored_partitions(
    engine: &Arc<engine::InMemoryEngine>,
    database: &str,
    table: &str,
) -> Vec<Vec<String>> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .database(ACCOUNT_ID, database)
        .and_then(|database| database.tables.get(&fold_name(table)))
        .map(|table| table.partitions.keys().cloned().collect())
        .unwrap_or_default()
}

/// The `Partition` structure `GetPartition` serves for one value list, or `None` when the
/// catalog holds no partition at that address.
async fn served_partition(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    values: &[String],
) -> Option<Value> {
    let (status, parsed) = call(
        router,
        "GetPartition",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionValues": values,
        }),
    )
    .await;
    if status != 200 {
        return None;
    }
    parsed.get("Partition").cloned()
}

/// One row of the discovery table: the objects staged, the target crawled, and the table the
/// crawl must write for them.
struct Discovery {
    /// What the row is about.
    case: &'static str,
    /// The bucket the row's objects live in (no underscore: `CreateBucket` refuses one).
    bucket: &'static str,
    /// The objects staged, as `(key, body)`.
    objects: Vec<(&'static str, &'static str)>,
    /// The `S3Target` the crawler carries.
    target: Value,
    /// The name of the table the crawl must write.
    table: &'static str,
    /// The columns that table must declare, as `(name, type)`.
    columns: Vec<(&'static str, &'static str)>,
    /// The partition keys it must declare, as `(name, type)`.
    partition_keys: Vec<(&'static str, &'static str)>,
    /// The partitions it must hold, as the value lists that address them.
    partitions: Vec<Vec<&'static str>>,
    /// What the data must be classified as.
    classification: &'static str,
}

// ---------------------------------------------------------------------------
// What a crawl discovers behind its targets
// ---------------------------------------------------------------------------

/// A crawl writes one table per dataset behind its targets, with the columns the data carries
/// and the partitions its folder levels hold.
///
/// The rows differ along what the rule generalises over: the number of folder levels below
/// the target (none, one, two), whether those levels are Hive-style `key=value` segments, the
/// layout of the data (comma-separated with a header, comma-separated without one, tab
/// separated, one JSON document per line), whether the target path ends in the separator, how
/// many objects sit behind the dataset, and which of them the target's `Exclusions` exclude.
/// The recorded case — a Hive-partitioned comma-separated prefix — is the first row.
#[tokio::test]
async fn a_crawl_writes_the_datasets_its_targets_hold() {
    let rows: Vec<Discovery> = vec![
        Discovery {
            case: "hive-style partitions, one level, csv with a header",
            bucket: "crawl-hive-one",
            objects: vec![
                (
                    "sales/year=2024/part-0.csv",
                    "order_id,amount\n1,10.5\n2,11.25\n",
                ),
                (
                    "sales/year=2025/part-0.csv",
                    "order_id,amount\n3,12.75\n",
                ),
            ],
            target: json!({ "Path": "s3://crawl-hive-one/sales/" }),
            table: "sales",
            columns: vec![("order_id", "bigint"), ("amount", "double")],
            partition_keys: vec![("year", "string")],
            partitions: vec![vec!["2024"], vec!["2025"]],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "hive-style partitions, two levels, target without a trailing separator",
            bucket: "crawl-hive-two",
            objects: vec![
                (
                    "events/year=2024/month=01/a.csv",
                    "id,name,active\n1,alpha,true\n",
                ),
                (
                    "events/year=2024/month=02/a.csv",
                    "id,name,active\n2,beta,false\n",
                ),
                (
                    "events/year=2025/month=01/a.csv",
                    "id,name,active\n3,gamma,true\n",
                ),
            ],
            target: json!({ "Path": "s3://crawl-hive-two/events" }),
            table: "events",
            columns: vec![("id", "bigint"), ("name", "string"), ("active", "boolean")],
            partition_keys: vec![("year", "string"), ("month", "string")],
            partitions: vec![
                vec!["2024", "01"],
                vec!["2024", "02"],
                vec!["2025", "01"],
            ],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "folder levels that are not hive-style are named by their depth",
            bucket: "crawl-plain-folders",
            objects: vec![
                ("logs/2024/part.csv", "line\nfirst\n"),
                ("logs/2025/part.csv", "line\nsecond\n"),
            ],
            target: json!({ "Path": "s3://crawl-plain-folders/logs/" }),
            table: "logs",
            columns: vec![("line", "string")],
            partition_keys: vec![("partition_0", "string")],
            partitions: vec![vec!["2024"], vec!["2025"]],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "objects directly under the target are one unpartitioned dataset",
            bucket: "crawl-flat",
            objects: vec![
                ("flat/one.csv", "id,label\n7,seven\n"),
                ("flat/two.csv", "id,label\n8,eight\n"),
            ],
            target: json!({ "Path": "s3://crawl-flat/flat/" }),
            table: "flat",
            columns: vec![("id", "bigint"), ("label", "string")],
            partition_keys: vec![],
            partitions: vec![],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "one JSON document per line, members as columns",
            bucket: "crawl-json",
            objects: vec![(
                "docs/region=eu/a.json",
                "{\"amount\": 1.5, \"id\": 3, \"ok\": true, \"tags\": [\"x\"], \"who\": \"me\"}\n",
            )],
            target: json!({ "Path": "s3://crawl-json/docs/" }),
            table: "docs",
            // The members are reported in the order the parsed document hands them over,
            // which is by name.
            columns: vec![
                ("amount", "double"),
                ("id", "int"),
                ("ok", "boolean"),
                ("tags", "array<string>"),
                ("who", "string"),
            ],
            partition_keys: vec![("region", "string")],
            partitions: vec![vec!["eu"]],
            classification: CLASSIFICATION_JSON,
        },
        Discovery {
            case: "a headerless delimited object names its columns by position",
            bucket: "crawl-headerless",
            objects: vec![(
                "raw/day=01/rows.csv",
                "1,alpha\n2,beta\n",
            )],
            target: json!({ "Path": "s3://crawl-headerless/raw/" }),
            table: "raw",
            columns: vec![("col0", "bigint"), ("col1", "string")],
            partition_keys: vec![("day", "string")],
            partitions: vec![vec!["01"]],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "another delimiter of the classifier's own set",
            bucket: "crawl-tabs",
            objects: vec![(
                "tsv/day=02/rows.tsv",
                "id\tlabel\n5\tfive\n",
            )],
            target: json!({ "Path": "s3://crawl-tabs/tsv/" }),
            table: "tsv",
            columns: vec![("id", "bigint"), ("label", "string")],
            partition_keys: vec![("day", "string")],
            partitions: vec![vec!["02"]],
            classification: CLASSIFICATION_CSV,
        },
        Discovery {
            case: "an excluded object's folder is no partition of the dataset",
            bucket: "crawl-exclusions",
            objects: vec![
                ("mixed/day=01/rows.csv", "id\n1\n"),
                ("mixed/day=02/rows.tmp", "id\n2\n"),
            ],
            target: json!({
                "Path": "s3://crawl-exclusions/mixed/",
                "Exclusions": ["**/*.tmp"],
            }),
            table: "mixed",
            columns: vec![("id", "bigint")],
            partition_keys: vec![("day", "string")],
            partitions: vec![vec!["01"]],
            classification: CLASSIFICATION_CSV,
        },
    ];

    for row in &rows {
        let (router, engine) = emulator_with_s3();
        let case = row.case;
        stage(&router, row.bucket, &row.objects).await;
        given_database(&router, DATABASE).await;
        given_crawler(&router, "crawler", row.target.clone(), json!({})).await;

        crawl(&router, &engine, "crawler").await;

        let crawler = crawler_of(&router, "crawler").await;
        assert_eq!(
            crawler.get("State").and_then(Value::as_str),
            Some(CRAWLER_STATE_READY),
            "case {case:?}: the finished crawl leaves the crawler ready, got {crawler}"
        );
        assert_eq!(
            crawler.pointer("/LastCrawl/Status").and_then(Value::as_str),
            Some(LAST_CRAWL_STATUS_SUCCEEDED),
            "case {case:?}: the crawl read its target, got {crawler}"
        );
        assert!(
            crawler.pointer("/LastCrawl/ErrorMessage").is_none(),
            "case {case:?}: a crawl that succeeded carries no error message, got {crawler}"
        );

        let table = table_of(&router, DATABASE, row.table)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the crawl must write the table {}", row.table));
        let expected_columns: Vec<(String, String)> = row
            .columns
            .iter()
            .map(|(name, declared)| ((*name).to_string(), (*declared).to_string()))
            .collect();
        let actual_columns = columns_of(&table);
        assert_eq!(
            actual_columns, expected_columns,
            "case {case:?}: expected the columns {expected_columns:?}, got {actual_columns:?}"
        );
        let expected_keys: Vec<(String, String)> = row
            .partition_keys
            .iter()
            .map(|(name, declared)| ((*name).to_string(), (*declared).to_string()))
            .collect();
        let actual_keys = partition_keys_of(&table);
        assert_eq!(
            actual_keys, expected_keys,
            "case {case:?}: expected the partition keys {expected_keys:?}, got {actual_keys:?}"
        );
        assert_eq!(
            parameter_of(&table, "classification").as_deref(),
            Some(row.classification),
            "case {case:?}: expected the data classified as {}, got {table}",
            row.classification
        );
        assert_eq!(
            parameter_of(&table, UPDATED_BY_CRAWLER_PARAMETER).as_deref(),
            Some("crawler"),
            "case {case:?}: the table records the crawler that wrote it, got {table}"
        );

        let expected_partitions: Vec<Vec<String>> = row
            .partitions
            .iter()
            .map(|values| values.iter().map(|value| (*value).to_string()).collect())
            .collect();
        let actual_partitions = stored_partitions(&engine, DATABASE, row.table);
        assert_eq!(
            actual_partitions, expected_partitions,
            "case {case:?}: expected the partitions {expected_partitions:?}, got \
             {actual_partitions:?}"
        );
        // Every discovered partition is really served, and its own location is the folder its
        // slice of the data sits in, under the table's location.
        let location = table
            .pointer("/StorageDescriptor/Location")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for values in &expected_partitions {
            let partition = served_partition(&router, DATABASE, row.table, values)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the partition {values:?} must be served")
                });
            let partition_location = partition
                .pointer("/StorageDescriptor/Location")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                partition_location.starts_with(&location) && partition_location != location,
                "case {case:?}: expected the partition {values:?} under {location}, got \
                 {partition_location}"
            );
            for value in values {
                assert!(
                    partition_location.contains(value.as_str()),
                    "case {case:?}: the partition {values:?} sits in the folder its values \
                     name, got {partition_location}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The crawl is in flight until the clock finishes it
// ---------------------------------------------------------------------------

/// A crawl is IN PROGRESS from the start until the instant the clock owed it its finish: the
/// crawler reports the running state and no report of this crawl, a second start is refused,
/// and the finish the clock owed is materialised by the next read.
#[tokio::test]
async fn a_crawl_is_in_flight_until_the_clock_finishes_it() {
    let (router, engine) = emulator_with_s3();
    stage(
        &router,
        "crawl-in-flight",
        &[("data/year=2024/a.csv", "id\n1\n")],
    )
    .await;
    given_database(&router, DATABASE).await;
    given_crawler(
        &router,
        "crawler",
        json!({ "Path": "s3://crawl-in-flight/data/" }),
        json!({}),
    )
    .await;

    ok(&router, "StartCrawler", &json!({ "Name": "crawler" })).await;
    // Held open, so the transient window is observable without sleeping for it.
    set_due_instant(&engine, far_future());

    let running = crawler_of(&router, "crawler").await;
    assert_eq!(
        running.get("State").and_then(Value::as_str),
        Some(CRAWLER_STATE_RUNNING),
        "case \"in flight\": the crawl is in progress, got {running}"
    );
    assert!(
        running.get("LastCrawl").is_none(),
        "case \"in flight\": a first crawl still running has produced no report, got {running}"
    );

    // "If the crawler is already running, returns a CrawlerRunningException."
    let (status, parsed) = call(&router, "StartCrawler", &json!({ "Name": "crawler" })).await;
    assert_eq!(
        status, 400,
        "case \"second start\": expected 400, got {status} with {parsed}"
    );
    assert!(
        error_code(&parsed).ends_with("CrawlerRunningException"),
        "case \"second start\": expected CrawlerRunningException, got {parsed}"
    );
    assert_eq!(
        error_message(&parsed),
        "Crawler with name crawler has already started",
        "case \"second start\": expected the refusal to name the crawler, got {parsed}"
    );

    // A write to the crawler's definition is refused for the same reason, and the delete too:
    // both are decided by the state this crawl put the crawler in.
    for operation in ["UpdateCrawler", "DeleteCrawler"] {
        let (status, parsed) = call(&router, operation, &json!({ "Name": "crawler" })).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: a crawl in progress refuses it, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("CrawlerRunningException"),
            "case {operation:?}: expected CrawlerRunningException, got {parsed}"
        );
    }

    // The finish the clock owed is materialised by the next read, whoever reads it.
    set_due_instant(&engine, already_past());
    let finished = crawler_of(&router, "crawler").await;
    assert_eq!(
        finished.get("State").and_then(Value::as_str),
        Some(CRAWLER_STATE_READY),
        "case \"finished\": the owed finish is materialised on read, got {finished}"
    );
    assert_eq!(
        finished.pointer("/LastCrawl/Status").and_then(Value::as_str),
        Some(LAST_CRAWL_STATUS_SUCCEEDED),
        "case \"finished\": the finished crawl reports its status, got {finished}"
    );
    // A second read of a finished crawl reports the same crawl: nothing about it is derived
    // from the instant somebody asked.
    let again = crawler_of(&router, "crawler").await;
    assert_eq!(
        again, finished,
        "case \"finished\": two reads of one finished crawl agree, got {again} and {finished}"
    );

    // The crawl that finished is the one the metrics report on: one table created, none
    // updated and none deleted, by the crawl that wrote it.
    let metrics = ok(
        &router,
        "GetCrawlerMetrics",
        &json!({ "CrawlerNameList": ["crawler"] }),
    )
    .await;
    let reported = metrics
        .pointer("/CrawlerMetricsList/0")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawlerMetrics must report the crawler, got {metrics}"));
    assert_eq!(
        reported.get("TablesCreated").and_then(Value::as_i64),
        Some(1),
        "case \"metrics\": the crawl created one table, got {reported}"
    );
    assert_eq!(
        reported.get("TablesUpdated").and_then(Value::as_i64).unwrap_or(0),
        0,
        "case \"metrics\": the crawl updated no table, got {reported}"
    );
    assert_eq!(
        reported.get("TablesDeleted").and_then(Value::as_i64).unwrap_or(0),
        0,
        "case \"metrics\": the crawl deleted no table, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// What a second crawl does to the tables the first one wrote
// ---------------------------------------------------------------------------

/// One row of the update-behaviour table: the policy the crawler states, and what a second
/// crawl of a dataset whose schema changed leaves behind.
struct SchemaChange {
    /// What the row is about.
    case: &'static str,
    /// The `SchemaChangePolicy` the crawler is created with, if any.
    policy: Value,
    /// The columns the stored table must declare after the second crawl.
    columns: Vec<(&'static str, &'static str)>,
    /// How many tables that crawl must report as updated.
    updates: i64,
}

/// A second crawl of the same crawler follows its `SchemaChangePolicy$UpdateBehavior`: the
/// default writes the schema it inferred over the stored table, and the logging behaviour
/// leaves the stored table exactly as the first crawl wrote it.
///
/// The rows are the modelled `UpdateBehavior` values, from
/// `smithy_query(query="shape",shape="UpdateBehavior")`.
#[tokio::test]
async fn a_second_crawl_follows_the_crawler_update_behaviour() {
    let rows: Vec<SchemaChange> = vec![
        SchemaChange {
            case: "no policy takes the update behaviour",
            policy: json!({}),
            columns: vec![("id", "bigint"), ("label", "string")],
            updates: 1,
        },
        SchemaChange {
            case: "UPDATE_IN_DATABASE writes the new schema",
            policy: json!({ "SchemaChangePolicy": { "UpdateBehavior": "UPDATE_IN_DATABASE" } }),
            columns: vec![("id", "bigint"), ("label", "string")],
            updates: 1,
        },
        SchemaChange {
            case: "LOG leaves the stored table alone",
            policy: json!({ "SchemaChangePolicy": { "UpdateBehavior": "LOG" } }),
            columns: vec![("id", "bigint")],
            updates: 0,
        },
    ];

    for row in &rows {
        let (case, policy, expected_columns, expected_updates) =
            (row.case, &row.policy, &row.columns, row.updates);
        let (router, engine) = emulator_with_s3();
        stage(&router, "crawl-rerun", &[("data/day=01/a.csv", "id\n1\n")]).await;
        given_database(&router, DATABASE).await;
        given_crawler(
            &router,
            "crawler",
            json!({ "Path": "s3://crawl-rerun/data/" }),
            policy.clone(),
        )
        .await;

        crawl(&router, &engine, "crawler").await;
        let first = table_of(&router, DATABASE, "data")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the first crawl writes the table"));
        assert_eq!(
            columns_of(&first),
            vec![("id".to_string(), "bigint".to_string())],
            "case {case:?}: the first crawl reads the columns the data carried, got {first}"
        );

        // The same dataset, now carrying a second column.
        put(
            &router,
            "crawl-rerun",
            &[("data/day=01/a.csv", "id,label\n1,alpha\n")],
        )
        .await;
        crawl(&router, &engine, "crawler").await;

        let second = table_of(&router, DATABASE, "data")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the table is still there"));
        let expected: Vec<(String, String)> = expected_columns
            .iter()
            .map(|(name, declared)| ((*name).to_string(), (*declared).to_string()))
            .collect();
        let actual = columns_of(&second);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the columns {expected:?}, got {actual:?}"
        );
        // One table, not two: a second crawl of the same dataset writes the same table.
        let tables = ok(
            &router,
            "GetTables",
            &json!({ "DatabaseName": DATABASE }),
        )
        .await;
        assert_eq!(
            crate::wire::table_names(&tables),
            vec!["data".to_string()],
            "case {case:?}: the second crawl writes no second table, got {tables}"
        );

        let metrics = ok(
            &router,
            "GetCrawlerMetrics",
            &json!({ "CrawlerNameList": ["crawler"] }),
        )
        .await;
        let updated = metrics
            .pointer("/CrawlerMetricsList/0/TablesUpdated")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert_eq!(
            updated, expected_updates,
            "case {case:?}: expected {expected_updates} table(s) updated, got {metrics}"
        );
    }
}

/// One row of the delete-behaviour table: the policy the crawler states, and what becomes of
/// a table it wrote whose data a later crawl no longer finds.
struct Abandoned {
    /// What the row is about.
    case: &'static str,
    /// The `SchemaChangePolicy` the crawler is created with, if any.
    policy: Value,
    /// Whether the abandoned table is still in the database afterwards.
    survives: bool,
    /// Whether it is marked as deprecated by this crawler.
    deprecated: bool,
    /// How many tables the crawl must report as deleted.
    deletes: i64,
}

/// A table THIS crawler wrote whose data a later crawl no longer finds follows the crawler's
/// `SchemaChangePolicy$DeleteBehavior`: the default marks it deprecated and keeps it, the
/// deleting behaviour removes it, and the logging behaviour leaves it untouched.
///
/// The rows are the modelled `DeleteBehavior` values, from
/// `smithy_query(query="shape",shape="DeleteBehavior")`.
#[tokio::test]
async fn a_crawl_follows_the_crawler_delete_behaviour_for_data_that_is_gone() {
    let rows: Vec<Abandoned> = vec![
        Abandoned {
            case: "no policy takes the deprecating behaviour",
            policy: json!({}),
            survives: true,
            deprecated: true,
            deletes: 0,
        },
        Abandoned {
            case: "DEPRECATE_IN_DATABASE keeps the table and marks it",
            policy: json!({ "SchemaChangePolicy": { "DeleteBehavior": "DEPRECATE_IN_DATABASE" } }),
            survives: true,
            deprecated: true,
            deletes: 0,
        },
        Abandoned {
            case: "DELETE_FROM_DATABASE removes the table",
            policy: json!({ "SchemaChangePolicy": { "DeleteBehavior": "DELETE_FROM_DATABASE" } }),
            survives: false,
            deprecated: false,
            deletes: 1,
        },
        Abandoned {
            case: "LOG leaves the table untouched",
            policy: json!({ "SchemaChangePolicy": { "DeleteBehavior": "LOG" } }),
            survives: true,
            deprecated: false,
            deletes: 0,
        },
    ];

    for row in &rows {
        let (case, policy, survives, deprecated, expected_deletes) = (
            row.case,
            &row.policy,
            row.survives,
            row.deprecated,
            row.deletes,
        );
        let (router, engine) = emulator_with_s3();
        stage(
            &router,
            "crawl-abandoned",
            &[
                ("first/day=01/a.csv", "id\n1\n"),
                ("second/day=01/a.csv", "id\n2\n"),
            ],
        )
        .await;
        given_database(&router, DATABASE).await;
        given_crawler(
            &router,
            "crawler",
            json!({ "Path": "s3://crawl-abandoned/first/" }),
            policy.clone(),
        )
        .await;
        crawl(&router, &engine, "crawler").await;
        assert!(
            table_of(&router, DATABASE, "first").await.is_some(),
            "case {case:?}: the first crawl writes the table it found"
        );

        // The crawler is pointed at another dataset, so the table it wrote before is one
        // whose data the next crawl does not find.
        ok(
            &router,
            "UpdateCrawler",
            &json!({
                "Name": "crawler",
                "Targets": { "S3Targets": [{ "Path": "s3://crawl-abandoned/second/" }] },
            }),
        )
        .await;
        crawl(&router, &engine, "crawler").await;

        let abandoned = table_of(&router, DATABASE, "first").await;
        assert_eq!(
            abandoned.is_some(),
            survives,
            "case {case:?}: expected the abandoned table to survive: {survives}, got \
             {abandoned:?}"
        );
        if let Some(table) = abandoned.as_ref() {
            let marked = parameter_of(table, DEPRECATED_BY_CRAWLER_PARAMETER);
            assert_eq!(
                marked.is_some(),
                deprecated,
                "case {case:?}: expected the table marked deprecated: {deprecated}, got {table}"
            );
        }
        assert!(
            table_of(&router, DATABASE, "second").await.is_some(),
            "case {case:?}: the second crawl writes the dataset it did find"
        );
        let metrics = ok(
            &router,
            "GetCrawlerMetrics",
            &json!({ "CrawlerNameList": ["crawler"] }),
        )
        .await;
        let deleted = metrics
            .pointer("/CrawlerMetricsList/0/TablesDeleted")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert_eq!(
            deleted, expected_deletes,
            "case {case:?}: expected {expected_deletes} table(s) deleted, got {metrics}"
        );
    }
}

// ---------------------------------------------------------------------------
// A crawl that cannot be carried out
// ---------------------------------------------------------------------------

/// A crawl whose output has nowhere to go, whose target holds nothing, or whose target
/// carries more exclusion patterns than one crawl matches against is reported for what it
/// was: the first and the last FAILED carrying the reason and writing no table, the middle one
/// SUCCEEDED having written none — a target that holds no dataset is not a failure.
#[tokio::test]
async fn a_crawl_reports_what_it_could_and_could_not_do() {
    let (router, engine) = emulator_with_s3();
    stage(&router, "crawl-outcomes", &[("full/day=01/a.csv", "id\n1\n")]).await;
    given_database(&router, DATABASE).await;

    // A crawler that names no database has nowhere to write the tables it finds.
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "no-database",
            "Role": ROLE,
            "Targets": { "S3Targets": [{ "Path": "s3://crawl-outcomes/full/" }] },
        }),
    )
    .await;
    crawl(&router, &engine, "no-database").await;
    let failed = crawler_of(&router, "no-database").await;
    assert_eq!(
        failed.pointer("/LastCrawl/Status").and_then(Value::as_str),
        Some(LAST_CRAWL_STATUS_FAILED),
        "case \"no database\": the crawl failed, got {failed}"
    );
    assert_eq!(
        failed
            .pointer("/LastCrawl/ErrorMessage")
            .and_then(Value::as_str),
        Some(CRAWL_WITHOUT_TARGET_DATABASE),
        "case \"no database\": the crawl reports why it failed, got {failed}"
    );
    assert!(
        table_of(&router, DATABASE, "full").await.is_none(),
        "case \"no database\": a crawl that failed writes no table"
    );

    // A target that holds no object at all: there is nothing there to describe, which is a
    // crawl that found nothing rather than one that failed.
    given_crawler(
        &router,
        "empty-target",
        json!({ "Path": "s3://crawl-outcomes/absent/" }),
        json!({}),
    )
    .await;
    crawl(&router, &engine, "empty-target").await;
    let empty = crawler_of(&router, "empty-target").await;
    assert_eq!(
        empty.pointer("/LastCrawl/Status").and_then(Value::as_str),
        Some(LAST_CRAWL_STATUS_SUCCEEDED),
        "case \"empty target\": a target holding nothing is no failure, got {empty}"
    );
    assert!(
        table_of(&router, DATABASE, "absent").await.is_none(),
        "case \"empty target\": a crawl that found nothing writes no table"
    );

    // A target carrying more exclusion patterns than one crawl matches its objects against is
    // data this crawl did not read, so the crawl fails saying so rather than reading the
    // target with some of its patterns dropped. The ceiling itself and one past it are both
    // exercised: the row above this one crawls a target with none, the discovery table crawls
    // one with a single pattern, and this is the first count past the bound.
    let too_many: Vec<String> = (0..=MAX_CRAWLED_EXCLUSIONS)
        .map(|index| format!("**/*.skip{index}"))
        .collect();
    given_crawler(
        &router,
        "too-many-exclusions",
        json!({ "Path": "s3://crawl-outcomes/full/", "Exclusions": too_many }),
        json!({}),
    )
    .await;
    crawl(&router, &engine, "too-many-exclusions").await;
    let unmatched = crawler_of(&router, "too-many-exclusions").await;
    assert_eq!(
        unmatched
            .pointer("/LastCrawl/Status")
            .and_then(Value::as_str),
        Some(LAST_CRAWL_STATUS_FAILED),
        "case \"past the exclusion ceiling\": the crawl did not read the target, got {unmatched}"
    );
    assert_eq!(
        unmatched
            .pointer("/LastCrawl/ErrorMessage")
            .and_then(Value::as_str),
        Some(crawl_target_unreadable("s3://crawl-outcomes/full/").as_str()),
        "case \"past the exclusion ceiling\": the crawl names the target it could not read, \
         got {unmatched}"
    );
    assert!(
        table_of(&router, DATABASE, "full").await.is_none(),
        "case \"past the exclusion ceiling\": a crawl that failed writes no table"
    );
}

/// A name no crawler in the account has is a miss, whatever its spelling: "a specified entity
/// does not exist", with the catalog's own wording for a crawler.
///
/// The rows differ along the spelling of the absent name: one never created, and the stored
/// crawler's name in another casing — a crawler is addressed by name exactly as it was
/// created, so another casing addresses no crawler.
#[tokio::test]
async fn starting_a_crawler_that_does_not_exist_is_a_miss() {
    let (router, _engine) = emulator_with_s3();
    given_database(&router, DATABASE).await;
    given_crawler(
        &router,
        "stored",
        json!({ "Path": "s3://crawl-miss/data/" }),
        json!({}),
    )
    .await;

    for (case, name) in [
        ("a name nothing created", "absent"),
        ("the stored name in another casing", "STORED"),
    ] {
        let (status, parsed) = call(&router, "StartCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {parsed}"
        );
        assert_eq!(
            error_message(&parsed),
            format!("Crawler entry with name {name} does not exist"),
            "case {case:?}: expected the miss to name the crawler, got {parsed}"
        );
    }
}
