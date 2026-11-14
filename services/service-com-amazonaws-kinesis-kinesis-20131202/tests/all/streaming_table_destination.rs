// SPDX-License-Identifier: CC0-1.0

//! The destination STREAMING TABLE a channel delivers to, end to end over the wire, with
//! Kinesis and S3 Tables mounted on ONE engine — the shape `core/router/src/lib.rs` mounts
//! them in — so the table a channel's create provisions is a record S3 Tables' own handlers
//! serve.
//!
//! RULE: a `CreateChannel` whose destination is a streaming table provisions that table in S3
//! Tables as it stores the channel — "Amazon Kinesis Data Streams creates this table in the
//! specified table bucket" (`S3TablesConfiguration$TableName`) — in the table bucket and
//! namespace the configuration names, under the name it names, reported by S3 Tables as a
//! table a service manages (`GetTable$managedByService` = this service's principal,
//! `GetTable$type` = `aws`), so `GetTable` and `ListTables` resolve it exactly as they resolve
//! a table a caller created. Provisioning never CLAIMS a table it did not create: a
//! destination naming a table bucket or namespace this account and region do not hold has
//! nowhere for its table and gets none, and a name a caller's own table holds is left to that
//! caller — a table name is unique inside its namespace.
//!
//! The recorded flow this was derived from creates ONE channel with ONE streaming table
//! configuration and then reads that table back, which is one row of the table below. The
//! rows differ along what the rule generalises over: the form and length of the destination
//! names (short, underscored, at the modelled `@length` bound of `S3TablesTableName` and
//! `S3TablesNamespace`), the compression the destination asks for, the record format the
//! source stream carries, and which bucket and namespace of the account the destination
//! names.
//!
//! Every Kinesis case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>`) and every S3 Tables case its restJson1 one, through
//! `MultiServiceRouter::dispatch`, so the statuses, bodies and modelled errors asserted here
//! are the ones a client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    managing_service_principal, CHANNEL_ACTIVE, STREAM_ACTIVE, STREAM_MODE_ON_DEMAND,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{dead_letter_queue, role_arn, schema_arn, stream_configuration};
use crate::stream_tags::{poll_until, stream_arn, REGION};

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `type` S3 Tables reports for "a table Amazon Web Services created on the account's
/// behalf" (`smithy_query(query="shape",shape="TableType")` -> `aws | customer`).
const TABLE_TYPE_AWS: &str = "aws";

/// The `@length` maximum of a destination table's name and of its namespace
/// (`smithy_query(query="shape",shape="S3TablesConfiguration")` -> `TableName` and
/// `Namespace`, both len 1..255, pattern `^[0-9a-z_]+$`).
const DESTINATION_NAME_MAX_LENGTH: usize = 255;

/// Kinesis and S3 Tables over ONE engine, so the table bucket a destination names and the
/// table the create provisions in it are the records S3 Tables' own handlers hold.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated; the
        // router still routes by the SigV4 credential scope, which is where the request's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_s3tables_s3tablebuckets::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One Kinesis call, signed for [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kinesis.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        self.dispatch(request).await
    }

    /// One Kinesis call that must be answered 200.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body {response}"
        );
        response
    }

    /// One S3 Tables call, signed for [`REGION`].
    async fn tables(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3tables.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3tables/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(payload))
            .expect("build the request");
        self.dispatch(request).await
    }

    /// One S3 Tables call that must be answered 200.
    async fn tables_ok(&self, case: &str, method: &str, path: &str, body: Option<&Value>) -> Value {
        let (status, response) = self.tables(method, path, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {method} {path} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// A table bucket of this account and region named `name`, as its ARN.
    async fn table_bucket(&self, case: &str, name: &str) -> String {
        let created = self
            .tables_ok(case, "PUT", "/buckets", Some(&json!({ "name": name })))
            .await;
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: CreateTableBucket answers the arn, got {created}"))
            .to_string()
    }

    /// A namespace named `namespace` inside the table bucket `bucket_arn`.
    async fn namespace(&self, case: &str, bucket_arn: &str, namespace: &str) {
        self.tables_ok(
            case,
            "PUT",
            &format!("/namespaces/{}", encode(bucket_arn)),
            Some(&json!({ "namespace": [namespace] })),
        )
        .await;
    }

    /// `GetTable` for one table, addressed by its bucket, namespace and name.
    async fn get_table(
        &self,
        bucket_arn: &str,
        namespace: &str,
        name: &str,
    ) -> (u16, Value) {
        self.tables(
            "GET",
            &format!(
                "/get-table?tableBucketARN={}&namespace={}&name={}",
                encode(bucket_arn),
                encode(namespace),
                encode(name)
            ),
            None,
        )
        .await
    }

    /// The `tables` member of `ListTables` for one bucket and namespace.
    async fn list_tables(&self, case: &str, bucket_arn: &str, namespace: &str) -> Vec<Value> {
        let listed = self
            .tables_ok(
                case,
                "GET",
                &format!(
                    "/tables/{}?namespace={}",
                    encode(bucket_arn),
                    encode(namespace)
                ),
                None,
            )
            .await;
        listed["tables"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: ListTables answers a tables list, got {listed}"))
    }

    /// An `ACTIVE` on-demand stream named `name` — the only capacity mode a channel supports
    /// — as its ARN.
    async fn on_demand_stream(&self, case: &str, name: &str) -> String {
        self.ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
        poll_until(case, &format!("the stream {name} is ready to serve"), || async {
            let (status, response) = self
                .call("DescribeStreamSummary", &json!({ "StreamName": name }))
                .await;
            status == 200
                && response["StreamDescriptionSummary"]["StreamStatus"].as_str()
                    == Some(STREAM_ACTIVE)
        })
        .await;
        stream_arn(REGION, name)
    }
}

/// An `@httpLabel` is ONE path segment and a query value ends at the next `&`, so an ARN's
/// `:` and `/` have to travel percent-encoded or the request addresses a different route.
fn encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('&', "%26")
        .replace('=', "%3D")
}

/// `name` repeated to `length` characters inside the `^[0-9a-z_]+$` pattern both destination
/// names carry, for the rows that sit on a modelled bound.
fn name_of_length(seed: &str, length: usize) -> String {
    seed.chars().cycle().take(length).collect()
}

/// One streaming table destination, with the dead-letter queue the model makes required.
fn tables_destination(bucket_arn: &str, namespace: &str, table: &str, compression: &str) -> Value {
    json!({
        "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "tables/"),
        "S3TablesConfigurationList": [{
            "TableBucketARN": bucket_arn,
            "Namespace": namespace,
            "TableName": table,
            "CompressionType": compression,
            "PartitionSpec": {
                "PartitionFields": [{ "Transform": "TIME_HOUR", "SourceName": "written_at" }],
            },
        }],
    })
}

/// The `CreateChannel` request body for a channel delivering to `destination`.
fn create_channel_body(name: &str, stream_arn: &str, format: &str, destination: Value) -> Value {
    json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(
            stream_arn,
            format,
            Some(schema_arn("records", "iceberg")),
        )],
        "S3TablesDestinationConfiguration": destination,
    })
}

/// One row of the provisioning table: a channel whose destination names `bucket`, `namespace`
/// and `table`.
struct Row {
    case: &'static str,
    bucket: &'static str,
    namespace: String,
    table: String,
    compression: &'static str,
    record_format: &'static str,
}

/// A create whose destination is a streaming table creates that table in S3 Tables, under the
/// destination's own bucket, namespace and name, reported as managed by this service — and
/// `ListTables` on that bucket and namespace reports exactly it.
#[tokio::test]
async fn a_streaming_table_destination_creates_the_table_it_names() {
    let emulator = Emulator::new();
    let rows = [
        Row {
            case: "short names",
            bucket: "ch-tables-short",
            namespace: "events".to_string(),
            table: "records".to_string(),
            compression: "ZSTD",
            record_format: "GSR_JSON",
        },
        Row {
            case: "underscored names",
            bucket: "ch-tables-underscored",
            namespace: "raw_events_2026".to_string(),
            table: "clickstream_v2".to_string(),
            compression: "SNAPPY",
            record_format: "JSON",
        },
        Row {
            case: "names at the modelled length bound",
            bucket: "ch-tables-bound",
            namespace: name_of_length("ns_0", DESTINATION_NAME_MAX_LENGTH),
            table: name_of_length("t_9", DESTINATION_NAME_MAX_LENGTH),
            compression: "NONE",
            record_format: "GSR_JSON",
        },
        Row {
            case: "names at the modelled length minimum",
            bucket: "ch-tables-minimum",
            namespace: "n".to_string(),
            table: "t".to_string(),
            compression: "ZSTD",
            record_format: "JSON",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let bucket_arn = emulator.table_bucket(case, row.bucket).await;
        emulator.namespace(case, &bucket_arn, &row.namespace).await;
        let source = emulator
            .on_demand_stream(case, &format!("tables-source-{index}"))
            .await;

        // Nothing holds the destination table before the channel is created, so what a read
        // resolves afterwards is what the create provisioned.
        let (before, body) = emulator
            .get_table(&bucket_arn, &row.namespace, &row.table)
            .await;
        assert_eq!(
            before, 404,
            "case {case}: the destination table must not exist before the create, got {before} \
             with body {body}"
        );

        let channel_name = format!("tables-channel-{index}");
        let created = emulator
            .ok(
                case,
                "CreateChannel",
                &create_channel_body(
                    &channel_name,
                    &source,
                    row.record_format,
                    tables_destination(&bucket_arn, &row.namespace, &row.table, row.compression),
                ),
            )
            .await;
        let channel_arn = created["ChannelDescription"]["ChannelARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the create answers a ChannelARN, got {created}"))
            .to_string();
        poll_until(
            case,
            &format!("the channel {channel_arn} reports {CHANNEL_ACTIVE}"),
            || async {
                let (status, response) = emulator
                    .call("DescribeChannel", &json!({ "ChannelARN": channel_arn }))
                    .await;
                status == 200
                    && response["ChannelDescription"]["ChannelStatus"].as_str()
                        == Some(CHANNEL_ACTIVE)
            },
        )
        .await;

        let (status, table) = emulator
            .get_table(&bucket_arn, &row.namespace, &row.table)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: the destination table must be readable after the create, got {status} \
             with body {table}"
        );
        assert_eq!(
            table["name"].as_str(),
            Some(row.table.as_str()),
            "case {case}: expected the table name {:?}, got {:?}",
            row.table,
            table["name"]
        );
        assert_eq!(
            table["namespace"],
            json!([row.namespace]),
            "case {case}: expected the namespace {:?}, got {:?}",
            json!([row.namespace]),
            table["namespace"]
        );
        assert_eq!(
            table["managedByService"].as_str(),
            Some(managing_service_principal().as_str()),
            "case {case}: expected managedByService {:?}, got {:?}",
            managing_service_principal(),
            table["managedByService"]
        );
        assert_eq!(
            table["type"].as_str(),
            Some(TABLE_TYPE_AWS),
            "case {case}: expected the table type {TABLE_TYPE_AWS:?}, got {:?}",
            table["type"]
        );

        let listed = emulator.list_tables(case, &bucket_arn, &row.namespace).await;
        let names: Vec<Option<&str>> = listed
            .iter()
            .map(|summary| summary["name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![Some(row.table.as_str())],
            "case {case}: expected the namespace to hold exactly the destination table, got \
             {names:?}"
        );
        assert_eq!(
            listed[0]["managedByService"].as_str(),
            Some(managing_service_principal().as_str()),
            "case {case}: expected the listed table to report managedByService {:?}, got {:?}",
            managing_service_principal(),
            listed[0]["managedByService"]
        );
    }
}

/// A table a CALLER created through S3 Tables is managed by no service, so the member that
/// reports a managing service tells the two apart inside one namespace.
#[tokio::test]
async fn a_caller_created_table_beside_it_reports_no_managing_service() {
    let case = "a customer table beside a channel's own";
    let emulator = Emulator::new();
    let bucket_arn = emulator.table_bucket(case, "ch-tables-mixed").await;
    emulator.namespace(case, &bucket_arn, "events").await;
    emulator
        .tables_ok(
            case,
            "PUT",
            &format!(
                "/tables/{}/{}",
                encode(&bucket_arn),
                encode("events")
            ),
            Some(&json!({ "name": "own_table", "format": "ICEBERG" })),
        )
        .await;
    let source = emulator.on_demand_stream(case, "mixed-source").await;
    emulator
        .ok(
            case,
            "CreateChannel",
            &create_channel_body(
                "mixed-channel",
                &source,
                "GSR_JSON",
                tables_destination(&bucket_arn, "events", "channel_table", "ZSTD"),
            ),
        )
        .await;

    for (table, expected) in [
        ("channel_table", Some(managing_service_principal())),
        ("own_table", None),
    ] {
        let (status, body) = emulator.get_table(&bucket_arn, "events", table).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {table} to be readable, got {status} with body {body}"
        );
        let reported = body["managedByService"].as_str().map(str::to_string);
        assert_eq!(
            reported, expected,
            "case {case}: expected {table} to report managedByService {expected:?}, got \
             {reported:?}"
        );
    }
    let listed = emulator.list_tables(case, &bucket_arn, "events").await;
    let reported: Vec<(String, Option<String>)> = listed
        .iter()
        .map(|summary| {
            (
                summary["name"].as_str().unwrap_or_default().to_string(),
                summary["managedByService"].as_str().map(str::to_string),
            )
        })
        .collect();
    let expected: Vec<(String, Option<String>)> = vec![
        ("channel_table".to_string(), Some(managing_service_principal())),
        ("own_table".to_string(), None),
    ];
    assert_eq!(
        reported, expected,
        "case {case}: expected the listing to report {expected:?}, got {reported:?}"
    );
}

/// One row of the table of destinations S3 Tables cannot take a table for: what of the
/// destination exists before the channel is created.
struct Unprovisionable {
    case: &'static str,
    /// Whether the table bucket is created at all.
    create_bucket: bool,
    /// Whether the destination namespace is created inside it.
    create_namespace: bool,
}

/// A destination S3 Tables has nowhere to put its table in leaves that destination WITHOUT a
/// table — and the emulator claims none: every read of it answers what S3 Tables actually
/// holds, which is nothing.
#[tokio::test]
async fn a_destination_with_nowhere_for_its_table_is_never_claimed_to_have_one() {
    let rows = [
        Unprovisionable {
            case: "a table bucket this account does not hold",
            create_bucket: false,
            create_namespace: false,
        },
        Unprovisionable {
            case: "a namespace the table bucket does not hold",
            create_bucket: true,
            create_namespace: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let bucket = format!("ch-tables-absent-{index}");
        let bucket_arn = if row.create_bucket {
            emulator.table_bucket(case, &bucket).await
        } else {
            format!(
                "arn:aws:s3tables:{REGION}:{}:bucket/{bucket}",
                engine::EMULATOR_ACCOUNT_ID
            )
        };
        if row.create_namespace {
            emulator.namespace(case, &bucket_arn, "events").await;
        }
        let source = emulator
            .on_demand_stream(case, &format!("absent-source-{index}"))
            .await;

        emulator
            .ok(
                case,
                "CreateChannel",
                &create_channel_body(
                    &format!("absent-destination-channel-{index}"),
                    &source,
                    "GSR_JSON",
                    tables_destination(&bucket_arn, "events", "records", "ZSTD"),
                ),
            )
            .await;

        let (status, body) = emulator.get_table(&bucket_arn, "events", "records").await;
        assert_eq!(
            status, 404,
            "case {case}: expected the destination table to be reported as absent, got {status} \
             with body {body}"
        );
    }
}

/// A table a CALLER created is never taken over: a destination naming it leaves the caller's
/// record exactly as it was, still managed by no service.
#[tokio::test]
async fn a_caller_created_table_is_not_taken_over_by_a_destination_of_its_name() {
    let case = "a destination naming a caller's own table";
    let emulator = Emulator::new();
    let bucket_arn = emulator.table_bucket(case, "ch-tables-held").await;
    emulator.namespace(case, &bucket_arn, "events").await;
    let created = emulator
        .tables_ok(
            case,
            "PUT",
            &format!("/tables/{}/{}", encode(&bucket_arn), encode("events")),
            Some(&json!({ "name": "records", "format": "ICEBERG" })),
        )
        .await;
    let held_arn = created["tableARN"].as_str().unwrap_or_default().to_string();
    let source = emulator.on_demand_stream(case, "held-source").await;

    emulator
        .ok(
            case,
            "CreateChannel",
            &create_channel_body(
                "held-destination-channel",
                &source,
                "GSR_JSON",
                tables_destination(&bucket_arn, "events", "records", "ZSTD"),
            ),
        )
        .await;

    let (status, table) = emulator.get_table(&bucket_arn, "events", "records").await;
    assert_eq!(
        status, 200,
        "case {case}: the caller's own table must still be readable, got {status} with body \
         {table}"
    );
    assert_eq!(
        table["tableARN"].as_str(),
        Some(held_arn.as_str()),
        "case {case}: expected the caller's table {held_arn:?} to be untouched, got {:?}",
        table["tableARN"]
    );
    assert_eq!(
        table["managedByService"].as_str(),
        None,
        "case {case}: a caller's own table is managed by no service, got {:?}",
        table["managedByService"]
    );
    let listed = emulator.list_tables(case, &bucket_arn, "events").await;
    assert_eq!(
        listed.len(),
        1,
        "case {case}: expected the namespace to still hold exactly the caller's table, got \
         {listed:?}"
    );
}

/// Provisioning is idempotent for the tables this service manages: a second channel delivering
/// to the same destination reaches the same one table rather than a refusal or a second
/// record.
#[tokio::test]
async fn a_second_channel_to_one_destination_reaches_the_same_table() {
    let case = "two channels, one destination table";
    let emulator = Emulator::new();
    let bucket_arn = emulator.table_bucket(case, "ch-tables-shared").await;
    emulator.namespace(case, &bucket_arn, "events").await;
    let source = emulator.on_demand_stream(case, "shared-source").await;

    let mut arns = Vec::new();
    for channel in ["first-channel", "second-channel"] {
        emulator
            .ok(
                case,
                "CreateChannel",
                &create_channel_body(
                    channel,
                    &source,
                    "GSR_JSON",
                    tables_destination(&bucket_arn, "events", "records", "ZSTD"),
                ),
            )
            .await;
        let (status, table) = emulator.get_table(&bucket_arn, "events", "records").await;
        assert_eq!(
            status, 200,
            "case {case}: the destination table must be readable after {channel}, got {status} \
             with body {table}"
        );
        arns.push(table["tableARN"].as_str().map(str::to_string));
    }
    assert_eq!(
        arns[0], arns[1],
        "case {case}: expected both creates to reach one table, got {:?} then {:?}",
        arns[0], arns[1]
    );
    let listed = emulator.list_tables(case, &bucket_arn, "events").await;
    assert_eq!(
        listed.len(),
        1,
        "case {case}: expected the namespace to hold exactly one table, got {listed:?}"
    );
}
