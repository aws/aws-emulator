// SPDX-License-Identifier: CC0-1.0

//! Amazon Personalize dataset EXPORT jobs, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with three services mounted on
//! ONE shared engine — the Personalize control plane (awsJson1_1, `POST /` with
//! `X-Amz-Target: AmazonPersonalize.<Operation>`), Amazon Personalize Events (restJson1,
//! `POST /items`, `/users`, `/events`) and Amazon S3 (where an export lands) — so what is
//! asserted is what a signing AWS client would see, the cross-service read and the
//! object actually written included.
//!
//! The rule under test: a dataset export job writes the records of the dataset its
//! `datasetArn` names to the Amazon S3 folder its `jobOutput` names, selecting them by
//! `ingestionMode` — the rows a dataset import job read in (`BULK`), the records the
//! ingestion data plane put in (`PUT`, the default), or both (`ALL`) — and reports the
//! documented `CREATE PENDING > CREATE IN_PROGRESS > ACTIVE -or- CREATE FAILED`
//! progression through `DescribeDatasetExportJob`. A dataset holding none of the
//! requested kind is refused with `InvalidInputException`, so nothing is ever reported
//! exported that was not written.
//!
//! The suites are TABLES: the rows vary what the rule generalises over (the dataset type,
//! the ingestion mode including its default, which half of the dataset holds data, the
//! record count and the destination folder's form), and the recorded case the issue cited
//! — `PutItems` followed by a `PUT` export read back — is one row among them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState as EventsState;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for, and every minted ARN is scoped to.
const REGION: &str = "us-east-1";
/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The bucket an export is written to.
const BUCKET: &str = "personalize-exports";
/// A bucket nothing creates, so a write to it cannot succeed.
const MISSING_BUCKET: &str = "personalize-exports-absent";

/// The statuses a job reports before its work is complete. Source: the model's own
/// documentation — "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE -or- CREATE FAILED".
const TRANSIENT_STATUSES: &[&str] = &["CREATE PENDING", "CREATE IN_PROGRESS"];
/// How long a case waits for a job to reach a terminal status before calling it stuck.
/// Far above the emulator's own create span, so a loaded machine does not fail the suite.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(30);

/// The one status an `awsJson1_1` service puts on a client error, whatever `@httpError`
/// the error shape declares: the shape in `__type` is what classifies it (the rule
/// `client_error_status.rs` states for this service).
const CLIENT_ERROR_STATUS: u16 = 400;

/// One emulator: the Personalize control plane, the Personalize Events ingestion plane
/// and S3, all on ONE shared engine — which is what the assembled router does.
struct Exporter {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Exporter {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are verified by the emulator, never by a service; this suite is
        // about behaviour, so it signs with a fixed scope and verifies nothing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_personalize_amazonpersonalize::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// One control-plane call (`AmazonPersonalize.<Operation>` on `POST /`).
    async fn control(&self, operation: &str, body: &Value) -> (u16, Value) {
        let host = format!("personalize.{REGION}.amazonaws.com");
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AmazonPersonalize.{operation}"))
            .header("authorization", sigv4("personalize"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), parse(response.body()))
    }

    /// A control-plane call that must succeed.
    async fn control_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.control(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed}"
        );
        parsed
    }

    /// One ingestion call (restJson1 `POST <route>` on the personalize-events endpoint).
    async fn ingest(&self, route: &str, body: &Value, case: &str) {
        let host = format!("personalize-events.{REGION}.amazonaws.com");
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{route}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", sigv4("personalize"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "case {case:?}: expected {route} to answer 200, got {status} {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// One path-style request to the emulator's S3 front door: S3 is a REST service, so
    /// the bucket and key live in the URI and the body is the object's bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Bytes) {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", sigv4("s3"))
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// Create [`BUCKET`], which an export is written into.
    ///
    /// The create names NO `LocationConstraint`: the unspecified constraint stands for
    /// us-east-1, which is the Region this suite signs for.
    async fn create_bucket(&self) {
        let (created, _) = self.s3("PUT", &format!("/{BUCKET}"), &[]).await;
        assert!(
            created == 200 || created == 409,
            "CreateBucket {BUCKET}: expected 200 or 409, got {created}"
        );
    }

    /// Upload one object.
    async fn upload(&self, key: &str, body: &str) {
        let (put, _) = self
            .s3("PUT", &format!("/{BUCKET}/{key}"), body.as_bytes())
            .await;
        assert_eq!(put, 200, "PutObject {key}: expected 200, got {put}");
    }

    /// The text of the object at `key`, which must exist.
    async fn object(&self, key: &str, case: &str) -> String {
        let (status, body) = self.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the export at {key} to be readable, got {status}"
        );
        String::from_utf8(body.to_vec()).expect("the export is UTF-8 text")
    }

    /// Create a dataset group and wait for it to become `ACTIVE`.
    async fn active_group(&self, case: &str, suffix: &str) -> String {
        let arn = self
            .control_ok(
                "CreateDatasetGroup",
                &json!({"name": format!("group-{suffix}")}),
                case,
            )
            .await["datasetGroupArn"]
            .as_str()
            .expect("a datasetGroupArn")
            .to_string();
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        loop {
            let group = self
                .control_ok(
                    "DescribeDatasetGroup",
                    &json!({"datasetGroupArn": arn}),
                    case,
                )
                .await["datasetGroup"]
                .clone();
            if group["status"].as_str() == Some("ACTIVE") {
                return arn;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the dataset group never became ACTIVE: {group}"
            );
            ::tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Create a schema and a dataset of `dataset_type` in `group`, and return its ARN.
    async fn dataset(
        &self,
        case: &str,
        suffix: &str,
        group: &str,
        dataset_type: &str,
        columns: &[&str],
    ) -> String {
        let schema = self
            .control_ok(
                "CreateSchema",
                &json!({
                    "name": format!("{dataset_type}-{suffix}"),
                    "schema": avro_schema(dataset_type, columns),
                }),
                case,
            )
            .await["schemaArn"]
            .as_str()
            .expect("a schemaArn")
            .to_string();
        self.control_ok(
            "CreateDataset",
            &json!({
                "name": format!("{dataset_type}-{suffix}"),
                "datasetGroupArn": group,
                "datasetType": dataset_type,
                "schemaArn": schema,
            }),
            case,
        )
        .await["datasetArn"]
            .as_str()
            .expect("a datasetArn")
            .to_string()
    }

    /// Import `csv` into `dataset` in bulk, which is what makes its rows `BULK` data.
    async fn import(&self, case: &str, suffix: &str, dataset: &str, csv: &str) {
        let key = format!("{suffix}/bulk.csv");
        self.upload(&key, csv).await;
        self.control_ok(
            "CreateDatasetImportJob",
            &json!({
                "jobName": format!("import-{suffix}"),
                "datasetArn": dataset,
                "dataSource": {"dataLocation": format!("s3://{BUCKET}/{key}")},
                "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/PersonalizeImport"),
            }),
            case,
        )
        .await;
    }

    /// Submit one dataset export job.
    async fn export(
        &self,
        job_name: &str,
        dataset: &str,
        path: &str,
        ingestion_mode: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({
            "jobName": job_name,
            "datasetArn": dataset,
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/PersonalizeExport"),
            "jobOutput": {"s3DataDestination": {"path": path}},
        });
        if let Some(mode) = ingestion_mode {
            body["ingestionMode"] = json!(mode);
        }
        self.control("CreateDatasetExportJob", &body).await
    }

    /// Submit one dataset export job that must be accepted, and return its ARN.
    async fn export_ok(
        &self,
        case: &str,
        job_name: &str,
        dataset: &str,
        path: &str,
        ingestion_mode: Option<&str>,
    ) -> String {
        let (status, body) = self.export(job_name, dataset, path, ingestion_mode).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDatasetExportJob to answer 200, got {status} {body}"
        );
        body["datasetExportJobArn"]
            .as_str()
            .expect("a datasetExportJobArn")
            .to_string()
    }

    /// Poll `DescribeDatasetExportJob` until the job leaves the transient statuses, and
    /// return it.
    async fn await_terminal(&self, case: &str, arn: &str) -> Value {
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        loop {
            let job = self
                .control_ok(
                    "DescribeDatasetExportJob",
                    &json!({"datasetExportJobArn": arn}),
                    case,
                )
                .await["datasetExportJob"]
                .clone();
            let reported = job["status"].as_str().unwrap_or("").to_string();
            if !TRANSIENT_STATUSES.contains(&reported.as_str()) {
                assert!(
                    !reported.is_empty(),
                    "case {case:?}: expected the export job to report a status, got {job}"
                );
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the export job never left {reported:?}"
            );
            ::tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Register an event tracker for `group`, as `CreateEventTracker` in the control
    /// plane will once it is implemented (it is still a scaffold stub with its own
    /// issue): a tracking id is minted there, never by the ingestion plane, so a suite
    /// that needs one seeds the same registry that create would.
    fn seed_tracker(&self, tracking_id: &str, group: &str) {
        self.engine
            .state::<EventsState>()
            .lock()
            .register_event_tracker(tracking_id, Some(group.to_string()))
            .expect("the ingestion store accepts the tracker");
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the
/// routing signal the router reads first.
fn sigv4(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// A response body as JSON, or `Null` for an empty one.
fn parse(body: &Bytes) -> Value {
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or(Value::Null)
    }
}

/// The wire error code of a failure response, with any namespace stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// An Avro record schema for `dataset_type` declaring `columns`, in the Avro JSON format
/// `CreateSchema` documents.
fn avro_schema(dataset_type: &str, columns: &[&str]) -> String {
    let fields: Vec<Value> = columns
        .iter()
        .map(|column| json!({"name": column, "type": "string"}))
        .collect();
    json!({
        "type": "record",
        "name": dataset_type,
        "namespace": "com.amazonaws.personalize.schema",
        "fields": fields,
        "version": "1.0",
    })
    .to_string()
}

/// The exported file, read as the CSV it is: its header row and one map per record,
/// keyed by the column names that header names.
///
/// Parsed by the `csv` crate rather than split by hand, so a quoted value is read the way
/// it was written.
fn exported_rows(body: &str) -> (Vec<String>, Vec<BTreeMap<String, String>>) {
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(body.as_bytes());
    let header: Vec<String> = reader
        .headers()
        .expect("the export carries a header row")
        .iter()
        .map(str::to_string)
        .collect();
    let rows = reader
        .records()
        .map(|record| {
            let record = record.expect("a readable export record");
            header
                .iter()
                .cloned()
                .zip(record.iter().map(str::to_string))
                .filter(|(_, value)| !value.is_empty())
                .collect()
        })
        .collect();
    (header, rows)
}

/// What the ingestion data plane put into a dataset before it was exported.
///
/// Each metadata variant carries the name its `properties` member writes the one property
/// under, so a case can PUT a property in a different case from the one the dataset's
/// schema declares it in.
#[derive(Debug, Clone, Copy)]
enum Ingested {
    /// Nothing was put in: the dataset holds only what was imported in bulk.
    Nothing,
    /// `PutItems` records, as `(itemId, value)` pairs.
    Items {
        property: &'static str,
        records: &'static [(&'static str, &'static str)],
    },
    /// `PutUsers` records, as `(userId, value)` pairs.
    Users {
        property: &'static str,
        records: &'static [(&'static str, &'static str)],
    },
    /// `PutEvents` item interactions, as `(userId, itemId)` pairs.
    Events(&'static [(&'static str, &'static str)]),
}

/// One row of the export table.
struct ExportCase {
    case: &'static str,
    dataset_type: &'static str,
    /// The columns the dataset's schema declares. The first is the one a record of this
    /// type is keyed by, which is what the expectations below name a record with.
    columns: &'static [&'static str],
    id_column: &'static str,
    /// The CSV imported in bulk, when the case has bulk data.
    bulk: Option<&'static str>,
    ingested: Ingested,
    /// The `ingestionMode` the export asks for; `None` exercises the documented default.
    ingestion_mode: Option<&'static str>,
    /// The destination folder, below `s3://<bucket>/`, with and without a trailing
    /// delimiter.
    folder: &'static str,
    /// The ids the export must carry, and the ones it must not.
    expected: &'static [&'static str],
    unexpected: &'static [&'static str],
    /// `(id, column, value)` triples the export must report, so a property that was PUT
    /// or imported is read back with its value and not merely its id.
    expected_values: &'static [(&'static str, &'static str, &'static str)],
}

/// The bulk CSV of two Items records, in the comma-delimited form an import reads.
const BULK_ITEMS: &str = "item_id,genre\nbulk-item-1,history\nbulk-item-2,sci-fi\n";
/// The bulk CSV of three Users records.
const BULK_USERS: &str = "user_id,age\nbulk-user-1,41\nbulk-user-2,52\nbulk-user-3,63\n";
/// The same three Users records, imported under a header spelled the way Amazon
/// Personalize's own documented schemas spell their fields.
const BULK_USERS_UPPER: &str =
    "USER_ID,AGE\nbulk-user-1,41\nbulk-user-2,52\nbulk-user-3,63\n";

/// The table: one row per (dataset type, ingestion mode, which half holds data, schema
/// field spelling) the rule generalises over. The recorded case the issue cited —
/// `PutItems` then a `PUT` export read back by its `ITEM_ID` — is the row named for it.
const EXPORT_CASES: &[ExportCase] = &[
    ExportCase {
        case: "items, PUT by default, read back with its properties",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        id_column: "item_id",
        bulk: None,
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama"), ("put-item-2", "comedy")],
        },
        ingestion_mode: None,
        folder: "exports/items-default/",
        expected: &["put-item-1", "put-item-2"],
        unexpected: &[],
        expected_values: &[
            ("put-item-1", "genre", "drama"),
            ("put-item-2", "genre", "comedy"),
        ],
    },
    ExportCase {
        case: "items under an upper-case schema are exported under its spellings",
        dataset_type: "Items",
        columns: &["ITEM_ID", "GENRE"],
        id_column: "ITEM_ID",
        bulk: None,
        ingested: Ingested::Items {
            property: "GENRE",
            records: &[("put-item-1", "drama")],
        },
        ingestion_mode: Some("PUT"),
        folder: "exports/items-upper/",
        expected: &["put-item-1"],
        unexpected: &[],
        expected_values: &[("put-item-1", "GENRE", "drama")],
    },
    ExportCase {
        case: "a property PUT in another case is exported under the schema's spelling",
        dataset_type: "Items",
        columns: &["ITEM_ID", "GENRE"],
        id_column: "ITEM_ID",
        bulk: None,
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama"), ("put-item-2", "comedy")],
        },
        ingestion_mode: Some("PUT"),
        folder: "exports/items-mixed-case/",
        expected: &["put-item-1", "put-item-2"],
        unexpected: &[],
        expected_values: &[
            ("put-item-1", "GENRE", "drama"),
            ("put-item-2", "GENRE", "comedy"),
        ],
    },
    ExportCase {
        case: "a property named like the id column cannot replace the record's own id",
        dataset_type: "Items",
        columns: &["ITEM_ID", "GENRE"],
        id_column: "ITEM_ID",
        bulk: None,
        ingested: Ingested::Items {
            // What identifies a record comes from the record, so a property a caller
            // happens to name the same as the id column is dropped rather than exported.
            property: "ITEM_ID",
            records: &[("put-item-1", "not-the-id")],
        },
        ingestion_mode: Some("PUT"),
        folder: "exports/items-id-property/",
        expected: &["put-item-1"],
        unexpected: &["not-the-id"],
        expected_values: &[],
    },
    ExportCase {
        case: "items, PUT named explicitly, exports only the ingested records",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        id_column: "item_id",
        bulk: Some(BULK_ITEMS),
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama")],
        },
        ingestion_mode: Some("PUT"),
        folder: "exports/items-put/",
        expected: &["put-item-1"],
        unexpected: &["bulk-item-1", "bulk-item-2"],
        expected_values: &[("put-item-1", "genre", "drama")],
    },
    ExportCase {
        case: "items, BULK exports only the imported rows",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        id_column: "item_id",
        bulk: Some(BULK_ITEMS),
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama")],
        },
        ingestion_mode: Some("BULK"),
        // A folder named without a trailing delimiter is the same folder.
        folder: "exports/items-bulk",
        expected: &["bulk-item-1", "bulk-item-2"],
        unexpected: &["put-item-1"],
        expected_values: &[
            ("bulk-item-1", "genre", "history"),
            ("bulk-item-2", "genre", "sci-fi"),
        ],
    },
    ExportCase {
        case: "items, ALL exports both halves of the dataset",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        id_column: "item_id",
        bulk: Some(BULK_ITEMS),
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama")],
        },
        ingestion_mode: Some("ALL"),
        folder: "exports/items-all/",
        expected: &["bulk-item-1", "bulk-item-2", "put-item-1"],
        unexpected: &[],
        expected_values: &[
            ("bulk-item-1", "genre", "history"),
            ("put-item-1", "genre", "drama"),
        ],
    },
    ExportCase {
        case: "users, PUT exports the ingested users",
        dataset_type: "Users",
        columns: &["user_id", "age"],
        id_column: "user_id",
        bulk: None,
        ingested: Ingested::Users {
            property: "age",
            records: &[("put-user-1", "29"), ("put-user-2", "33")],
        },
        ingestion_mode: Some("PUT"),
        folder: "exports/users-put/",
        expected: &["put-user-1", "put-user-2"],
        unexpected: &[],
        expected_values: &[("put-user-2", "age", "33")],
    },
    ExportCase {
        case: "users, ALL over an upper-case schema folds both halves into one column set",
        dataset_type: "Users",
        columns: &["USER_ID", "AGE"],
        id_column: "USER_ID",
        bulk: Some(BULK_USERS_UPPER),
        ingested: Ingested::Users {
            property: "AGE",
            records: &[("put-user-1", "29")],
        },
        ingestion_mode: Some("ALL"),
        folder: "exports/users-all-upper/",
        expected: &["bulk-user-1", "bulk-user-2", "bulk-user-3", "put-user-1"],
        unexpected: &[],
        expected_values: &[
            ("bulk-user-3", "AGE", "63"),
            ("put-user-1", "AGE", "29"),
        ],
    },
    ExportCase {
        case: "users, ALL with three imported rows and nothing ingested",
        dataset_type: "Users",
        columns: &["user_id", "age"],
        id_column: "user_id",
        bulk: Some(BULK_USERS),
        ingested: Ingested::Nothing,
        ingestion_mode: Some("ALL"),
        folder: "exports/users-all/",
        expected: &["bulk-user-1", "bulk-user-2", "bulk-user-3"],
        unexpected: &[],
        expected_values: &[("bulk-user-3", "age", "63")],
    },
    ExportCase {
        case: "interactions, PUT exports the events a tracker of the group ingested",
        dataset_type: "Interactions",
        columns: &["USER_ID", "ITEM_ID", "TIMESTAMP"],
        id_column: "USER_ID",
        bulk: None,
        ingested: Ingested::Events(&[("put-user-1", "put-item-1"), ("put-user-2", "put-item-2")]),
        ingestion_mode: Some("PUT"),
        folder: "exports/interactions-put/",
        expected: &["put-user-1", "put-user-2"],
        unexpected: &[],
        expected_values: &[
            ("put-user-1", "ITEM_ID", "put-item-1"),
            ("put-user-2", "ITEM_ID", "put-item-2"),
            // The instant the caller reported, in the Unix time the schema's TIMESTAMP
            // field carries.
            ("put-user-1", "TIMESTAMP", "1764000000"),
        ],
    },
];

/// The client-side instant every ingested event reports, as the Unix time
/// `Event$sentAt` carries.
const SENT_AT: f64 = 1_764_000_000.0;

/// Put `ingested` into `dataset` through the ingestion data plane, over the wire.
async fn ingest(emulator: &Exporter, case: &ExportCase, dataset: &str, group: &str) {
    match case.ingested {
        Ingested::Nothing => {}
        Ingested::Items { property, records } => {
            let items: Vec<Value> = records
                .iter()
                .map(|(id, value)| {
                    json!({
                        "itemId": id,
                        "properties": json!({property: value}).to_string(),
                    })
                })
                .collect();
            emulator
                .ingest(
                    "/items",
                    &json!({"datasetArn": dataset, "items": items}),
                    case.case,
                )
                .await;
        }
        Ingested::Users { property, records } => {
            let users: Vec<Value> = records
                .iter()
                .map(|(id, value)| {
                    json!({
                        "userId": id,
                        "properties": json!({property: value}).to_string(),
                    })
                })
                .collect();
            emulator
                .ingest(
                    "/users",
                    &json!({"datasetArn": dataset, "users": users}),
                    case.case,
                )
                .await;
        }
        Ingested::Events(events) => {
            let tracking_id = format!("tracker-{}", short(case.case));
            emulator.seed_tracker(&tracking_id, group);
            for (index, (user, item)) in events.iter().enumerate() {
                emulator
                    .ingest(
                        "/events",
                        &json!({
                            "trackingId": tracking_id,
                            "sessionId": format!("session-{index}"),
                            "userId": user,
                            "eventList": [{
                                "eventType": "click",
                                "sentAt": SENT_AT,
                                "itemId": item,
                            }],
                        }),
                        case.case,
                    )
                    .await;
            }
        }
    }
}

/// A short, stable suffix for one case's resource names.
fn short(case: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(case.as_bytes());
    hex::encode(&digest[..4])
}

#[tokio::test]
async fn an_export_job_writes_the_records_the_ingestion_mode_names() {
    for case in EXPORT_CASES {
        let emulator = Exporter::new();
        emulator.create_bucket().await;
        let suffix = short(case.case);
        let group = emulator.active_group(case.case, &suffix).await;
        let dataset = emulator
            .dataset(
                case.case,
                &suffix,
                &group,
                case.dataset_type,
                case.columns,
            )
            .await;
        if let Some(bulk) = case.bulk {
            emulator.import(case.case, &suffix, &dataset, bulk).await;
        }
        ingest(&emulator, case, &dataset, &group).await;

        let job_name = format!("export-{suffix}");
        let path = format!("s3://{BUCKET}/{}", case.folder);
        let arn = emulator
            .export_ok(case.case, &job_name, &dataset, &path, case.ingestion_mode)
            .await;
        let job = emulator.await_terminal(case.case, &arn).await;
        assert_eq!(
            job["status"].as_str().unwrap_or(""),
            "ACTIVE",
            "case {:?}: expected the export job to reach ACTIVE, got {job}",
            case.case
        );

        // The job's output is an object under the folder the request named, named for
        // the job, so two jobs writing to one folder do not overwrite each other.
        let folder = case.folder.trim_end_matches('/');
        let body = emulator
            .object(&format!("{folder}/{job_name}.csv"), case.case)
            .await;
        let (header, rows) = exported_rows(&body);
        let ids: Vec<String> = rows
            .iter()
            .filter_map(|row| row.get(case.id_column).cloned())
            .collect();
        for expected in case.expected {
            assert!(
                ids.iter().any(|id| id == expected),
                "case {:?}: expected the export to carry {expected:?} in {:?}, got {ids:?} \
                 (header {header:?})",
                case.case,
                case.id_column
            );
        }
        for unexpected in case.unexpected {
            assert!(
                !ids.iter().any(|id| id == unexpected),
                "case {:?}: expected the export NOT to carry {unexpected:?}, got {ids:?}",
                case.case
            );
        }
        assert_eq!(
            ids.len(),
            case.expected.len(),
            "case {:?}: expected exactly {} exported records, got {ids:?}",
            case.case,
            case.expected.len()
        );
        for (id, column, value) in case.expected_values {
            let row = rows
                .iter()
                .find(|row| row.get(case.id_column).map(String::as_str) == Some(id))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected an exported record {id:?}, got {rows:?}",
                        case.case
                    )
                });
            assert_eq!(
                row.get(*column).map(String::as_str),
                Some(*value),
                "case {:?}: expected {id:?} to carry {column}={value:?}, got {row:?}",
                case.case
            );
        }
        // Every column of the header is one the records carry; nothing is invented.
        assert!(
            header.contains(&case.id_column.to_string()),
            "case {:?}: expected the header to name {:?}, got {header:?}",
            case.case,
            case.id_column
        );
    }
}

/// One row of the refusal table: a dataset holding no data of the mode asked for.
struct NoDataCase {
    case: &'static str,
    dataset_type: &'static str,
    columns: &'static [&'static str],
    bulk: Option<&'static str>,
    ingested: Ingested,
    ingestion_mode: Option<&'static str>,
    /// The mode the message names, which is the one the export resolved to.
    reported_mode: &'static str,
}

/// The modes a dataset can hold no data for. Source: `IngestionMode`'s own values
/// (`smithy_query(query="shape", shape="IngestionMode")`: BULK | PUT | ALL), each asked
/// of a dataset holding the other half or nothing at all.
const NO_DATA_CASES: &[NoDataCase] = &[
    NoDataCase {
        case: "an empty dataset, PUT by default",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        bulk: None,
        ingested: Ingested::Nothing,
        ingestion_mode: None,
        reported_mode: "PUT",
    },
    NoDataCase {
        case: "a dataset with only ingested records, asked for BULK",
        dataset_type: "Items",
        columns: &["item_id", "genre"],
        bulk: None,
        ingested: Ingested::Items {
            property: "genre",
            records: &[("put-item-1", "drama")],
        },
        ingestion_mode: Some("BULK"),
        reported_mode: "BULK",
    },
    NoDataCase {
        case: "a dataset with only imported rows, asked for PUT",
        dataset_type: "Users",
        columns: &["user_id", "age"],
        bulk: Some(BULK_USERS),
        ingested: Ingested::Nothing,
        ingestion_mode: Some("PUT"),
        reported_mode: "PUT",
    },
    NoDataCase {
        case: "an empty dataset, asked for ALL",
        dataset_type: "Users",
        columns: &["user_id", "age"],
        bulk: None,
        ingested: Ingested::Nothing,
        ingestion_mode: Some("ALL"),
        reported_mode: "ALL",
    },
];

#[tokio::test]
async fn an_export_of_data_a_dataset_does_not_hold_is_refused_as_an_input_fault() {
    for case in NO_DATA_CASES {
        let emulator = Exporter::new();
        emulator.create_bucket().await;
        let suffix = short(case.case);
        let group = emulator.active_group(case.case, &suffix).await;
        let dataset = emulator
            .dataset(
                case.case,
                &suffix,
                &group,
                case.dataset_type,
                case.columns,
            )
            .await;
        if let Some(bulk) = case.bulk {
            emulator.import(case.case, &suffix, &dataset, bulk).await;
        }
        // The ingestion half of the table reuses the export table's own ingester.
        let ingesting = ExportCase {
            case: case.case,
            dataset_type: case.dataset_type,
            columns: case.columns,
            id_column: case.columns[0],
            bulk: case.bulk,
            ingested: case.ingested,
            ingestion_mode: case.ingestion_mode,
            folder: "unused/",
            expected: &[],
            unexpected: &[],
            expected_values: &[],
        };
        ingest(&emulator, &ingesting, &dataset, &group).await;

        let (status, body) = emulator
            .export(
                &format!("export-{suffix}"),
                &dataset,
                &format!("s3://{BUCKET}/exports/{suffix}/"),
                case.ingestion_mode,
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected CreateDatasetExportJob to answer 400, got {status} {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {:?}: expected InvalidInputException, got {body}",
            case.case
        );
        assert_eq!(
            error_message(&body),
            format!(
                "No data found in datasets for ingestionMode: {}",
                case.reported_mode
            ),
            "case {:?}: unexpected refusal wording",
            case.case
        );
        // Nothing was exported, so no job exists to poll. This service is `awsJson1_1`,
        // whose one client-error status is 400 (see `client_error_status.rs`), so the
        // error SHAPE is what says "not found".
        let (described, described_body) = emulator
            .control(
                "DescribeDatasetExportJob",
                &json!({"datasetExportJobArn": format!(
                    "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-export-job/export-{suffix}"
                )}),
            )
            .await;
        assert_eq!(
            described, CLIENT_ERROR_STATUS,
            "case {:?}: expected no export job to have been recorded, got {described} \
             {described_body}",
            case.case
        );
        assert_eq!(
            error_code(&described_body),
            "ResourceNotFoundException",
            "case {:?}: expected no export job to have been recorded, got {described_body}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_export_job_reports_the_documented_progression_and_echoes_its_request() {
    let case = "progression";
    let emulator = Exporter::new();
    emulator.create_bucket().await;
    let suffix = short(case);
    let group = emulator.active_group(case, &suffix).await;
    let dataset = emulator
        .dataset(case, &suffix, &group, "Items", &["item_id", "genre"])
        .await;
    emulator
        .ingest(
            "/items",
            &json!({
                "datasetArn": dataset,
                "items": [{"itemId": "put-item-1", "properties": "{\"genre\": \"drama\"}"}],
            }),
            case,
        )
        .await;

    let job_name = format!("export-{suffix}");
    let path = format!("s3://{BUCKET}/exports/{suffix}/");
    let arn = emulator
        .export_ok(case, &job_name, &dataset, &path, Some("PUT"))
        .await;
    assert_eq!(
        arn,
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-export-job/{job_name}"),
        "the ARN is minted from the job's own name"
    );

    // The first read happens before the clock owes the job its terminal status, so it
    // reports one of the documented transient states.
    let first = emulator
        .control_ok(
            "DescribeDatasetExportJob",
            &json!({"datasetExportJobArn": arn}),
            case,
        )
        .await["datasetExportJob"]
        .clone();
    let reported = first["status"].as_str().unwrap_or("").to_string();
    assert!(
        TRANSIENT_STATUSES.contains(&reported.as_str()) || reported == "ACTIVE",
        "case {case:?}: expected a documented status, got {first}"
    );

    let job = emulator.await_terminal(case, &arn).await;
    assert_eq!(
        job["status"].as_str().unwrap_or(""),
        "ACTIVE",
        "case {case:?}: expected the export job to reach ACTIVE, got {job}"
    );
    assert_eq!(job["jobName"].as_str(), Some(job_name.as_str()));
    assert_eq!(job["datasetArn"].as_str(), Some(dataset.as_str()));
    assert_eq!(job["ingestionMode"].as_str(), Some("PUT"));
    assert_eq!(
        job["jobOutput"]["s3DataDestination"]["path"].as_str(),
        Some(path.as_str()),
        "the destination is echoed as the request named it: {job}"
    );
    assert!(
        job["failureReason"].is_null(),
        "case {case:?}: a job that succeeded carries no failureReason: {job}"
    );
    assert!(
        job["creationDateTime"].is_number() && job["lastUpdatedDateTime"].is_number(),
        "case {case:?}: expected the modelled timestamps, got {job}"
    );

    // A second submission of the same job name collides on the ARN minted from it.
    let (again, again_body) = emulator
        .export(&job_name, &dataset, &path, Some("PUT")).await;
    assert_eq!(
        again, CLIENT_ERROR_STATUS,
        "case {case:?}: expected a repeated job name to be refused, got {again} {again_body}"
    );
    assert_eq!(
        error_code(&again_body),
        "ResourceAlreadyExistsException",
        "case {case:?}: got {again_body}"
    );

    // An ARN naming no export job at all.
    let unknown =
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-export-job/export-nothing");
    let (status, body) = emulator
        .control(
            "DescribeDatasetExportJob",
            &json!({"datasetExportJobArn": unknown}),
        )
        .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case {case:?}: expected an unknown export job to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Resource Arn {unknown} does not exist."),
        "case {case:?}: unexpected not-found wording"
    );

    // A dataset ARN naming no dataset.
    let missing_dataset =
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/absent/ITEMS");
    let (status, body) = emulator
        .export(
            &format!("export-absent-{suffix}"),
            &missing_dataset,
            &path,
            Some("PUT"),
        )
        .await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case {case:?}: expected an unknown dataset to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The given dataset does not exist: {missing_dataset}"),
        "case {case:?}: unexpected not-found wording"
    );
}

/// The destination forms an export cannot be written to. Source: `S3DataConfig$path`'s
/// own pattern `^(s3|http|https)://.+$` — only the first names storage this emulator
/// serves.
const UNWRITABLE_PATHS: &[&str] = &[
    "https://example.invalid/exports/",
    "http://example.invalid/exports/",
    "s3:///exports/",
];

#[tokio::test]
async fn an_export_that_cannot_be_written_is_refused_or_reported_failed() {
    let case = "unwritable";
    let emulator = Exporter::new();
    emulator.create_bucket().await;
    let suffix = short(case);
    let group = emulator.active_group(case, &suffix).await;
    let dataset = emulator
        .dataset(case, &suffix, &group, "Items", &["item_id", "genre"])
        .await;
    emulator
        .ingest(
            "/items",
            &json!({
                "datasetArn": dataset,
                "items": [{"itemId": "put-item-1"}],
            }),
            case,
        )
        .await;

    // A path naming no Amazon S3 bucket is an INPUT fault: the submission is refused and
    // no job is recorded.
    for (index, path) in UNWRITABLE_PATHS.iter().enumerate() {
        let (status, body) = emulator
            .export(
                &format!("export-unwritable-{index}"),
                &dataset,
                path,
                Some("PUT"),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {path:?}: expected CreateDatasetExportJob to answer 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {path:?}: got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The job output path {path} names no Amazon S3 bucket; Amazon Personalize \
                 exports data to Amazon S3."
            ),
            "case {path:?}: unexpected refusal wording"
        );
    }

    // A bucket that does not exist IS writable storage, so the job is accepted and then
    // reports the modelled failure with the reason — never a success nothing wrote.
    let job_name = format!("export-missing-bucket-{suffix}");
    let arn = emulator
        .export_ok(
            case,
            &job_name,
            &dataset,
            &format!("s3://{MISSING_BUCKET}/exports/"),
            Some("PUT"),
        )
        .await;
    let job = emulator.await_terminal(case, &arn).await;
    assert_eq!(
        job["status"].as_str().unwrap_or(""),
        "CREATE FAILED",
        "case {case:?}: expected the export to fail, got {job}"
    );
    let reason = job["failureReason"].as_str().unwrap_or_default();
    assert!(
        reason.contains(MISSING_BUCKET),
        "case {case:?}: expected the failure reason to name the destination, got {job}"
    );
}
