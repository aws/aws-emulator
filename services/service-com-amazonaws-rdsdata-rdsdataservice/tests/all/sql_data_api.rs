// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RDS Data API over the real wire: every call goes
//! through the router, so routing, restJson1 deserialization, the handler and
//! serialization are all exercised.
//!
//! The rules under test are stated once each, above the test that pins them, and
//! every table row is one input of the class the rule generalises over.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

pub(crate) const RESOURCE_ARN: &str = "arn:aws:rds:us-east-1:000000000000:cluster:demo";
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-east-1:000000000000:secret:demo";

/// One emulator with one Data API client pointed at it. The suites beside this
/// one drive the same client.
pub(crate) struct DataApi {
    router: MultiServiceRouter,
    /// The ONE engine the router hands every handler of this service, kept so a suite can
    /// reach this service's own store the way a handler does — which is how a suite drives
    /// the CLOCK a window is measured against instead of waiting for it ([`Self::age_transaction`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl DataApi {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Present the transaction `transaction_id` names as one OPENED `open_for` ago whose
    /// id was last used `idle_for` ago, by moving the instants its windows are measured
    /// from back by those amounts.
    ///
    /// The clock a suite drives: a window the service owes is reached by moving the
    /// transaction's own instants, so nothing waits for wall-clock time to pass. Pass
    /// `open_for` no smaller than `idle_for` — a transaction cannot have been used before
    /// it was opened.
    pub(crate) fn age_transaction(
        &self,
        transaction_id: &str,
        open_for: Duration,
        idle_for: Duration,
    ) {
        assert!(
            open_for >= idle_for,
            "case {transaction_id}: a transaction cannot have been used {idle_for:?} ago and \
             opened only {open_for:?} ago"
        );
        let store = service_com_amazonaws_rdsdata_rdsdataservice::state::service_state(
            &*self.engine,
        )
        .expect("this service's store is in the shared engine");
        let mut state = store.lock();
        let cluster = state
            .cluster_mut(RESOURCE_ARN)
            .expect("the cluster every call of this suite addresses");
        let transaction = cluster
            .transactions
            .get_mut(transaction_id)
            .unwrap_or_else(|| panic!("the cluster holds the transaction {transaction_id}"));
        let now = SystemTime::now();
        transaction.created_at = now - open_for;
        transaction.last_used_at = now - idle_for;
    }

    /// Send one request the way an SDK client does, and read the status and body.
    pub(crate) async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://rds-data.us-east-1.amazonaws.com{path}"))
            .header("host", "rds-data.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// `ExecuteStatement` with whatever members the case needs.
    pub(crate) async fn execute(&self, members: Value) -> (u16, Value) {
        self.post("/Execute", with_target(members)).await
    }

    /// `ExecuteStatement` for a statement that must succeed.
    pub(crate) async fn run(&self, members: Value) -> Value {
        let (status, body) = self.execute(members.clone()).await;
        assert_eq!(status, 200, "statement {members} failed with {body}");
        body
    }

    pub(crate) async fn batch(&self, members: Value) -> (u16, Value) {
        self.post("/BatchExecute", with_target(members)).await
    }

    async fn begin(&self, members: Value) -> (u16, Value) {
        self.post("/BeginTransaction", with_target(members)).await
    }

    /// Open a transaction that must succeed, and return its id.
    pub(crate) async fn begin_transaction(&self, database: Option<&str>) -> String {
        let mut members = json!({});
        if let Some(database) = database {
            members["database"] = json!(database);
        }
        let (status, body) = self.begin(members).await;
        assert_eq!(status, 200, "BeginTransaction failed with {body}");
        body["transactionId"]
            .as_str()
            .unwrap_or_else(|| panic!("BeginTransaction returned no transactionId: {body}"))
            .to_string()
    }

    /// Open a transaction on ANOTHER cluster of this same emulator, and return its id: the
    /// id is bound to the resource that opened it, which is what a call naming a different
    /// `resourceArn` is refused against.
    pub(crate) async fn begin_transaction_on(&self, resource_arn: &str) -> String {
        let (status, body) = self
            .post(
                "/BeginTransaction",
                json!({"resourceArn": resource_arn, "secretArn": SECRET_ARN}),
            )
            .await;
        assert_eq!(status, 200, "BeginTransaction failed with {body}");
        body["transactionId"]
            .as_str()
            .unwrap_or_else(|| panic!("BeginTransaction returned no transactionId: {body}"))
            .to_string()
    }

    pub(crate) async fn commit(&self, transaction_id: &str) -> (u16, Value) {
        self.post(
            "/CommitTransaction",
            with_target(json!({"transactionId": transaction_id})),
        )
        .await
    }

    pub(crate) async fn rollback(&self, transaction_id: &str) -> (u16, Value) {
        self.post(
            "/RollbackTransaction",
            with_target(json!({"transactionId": transaction_id})),
        )
        .await
    }
}

/// Add the cluster and secret every call carries.
fn with_target(mut members: Value) -> Value {
    let object = members
        .as_object_mut()
        .expect("a request body is a JSON object");
    object.insert("resourceArn".to_string(), json!(RESOURCE_ARN));
    object.insert("secretArn".to_string(), json!(SECRET_ARN));
    members
}

/// A number member, defaulting to the modelled default of 0 when the wire omits it.
pub(crate) fn number(body: &Value, member: &str) -> i64 {
    body.get(member).and_then(Value::as_i64).unwrap_or(0)
}

/// A list member, defaulting to empty when the wire omits it.
pub(crate) fn list(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The wire error code restJson1 carries in `__type`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// One named parameter.
pub(crate) fn parameter(name: &str, value: Value) -> Value {
    json!({"name": name, "value": value})
}

/// An id of the same CLASS as one this service issued, that no transaction was ever opened
/// under.
///
/// A transaction id carries the resource it belongs to, so an id a cluster could have
/// issued cannot be written by hand without an id it DID issue to take that binding from:
/// changing one digit of the issued id's head leaves the binding intact and names a
/// transaction nothing opened.
pub(crate) fn a_sibling_id(issued: &str) -> String {
    let (first, rest) = issued
        .split_at_checked(1)
        .unwrap_or_else(|| panic!("an issued transaction id is not empty: {issued:?}"));
    let other = if first == "0" { "1" } else { "0" };
    format!("{other}{rest}")
}

/// RULE: the five implemented operations compute their answers from one persisted
/// SQL database per (cluster, database name), and a transaction's writes reach
/// that database only when it commits.
///
/// This is the client script the issue describes, run end to end.
#[tokio::test]
async fn a_data_api_client_script_runs_end_to_end() {
    let api = DataApi::new();

    let transaction = api.begin_transaction(Some("demo")).await;
    assert!(
        !transaction.is_empty() && transaction.len() <= 192,
        "transactionId must be a non-empty id of at most 192 characters, got {transaction:?}"
    );

    let created = api
        .run(json!({
            "database": "demo",
            "transactionId": transaction,
            "sql": "CREATE TABLE pets (id INT, name VARCHAR(32))",
        }))
        .await;
    assert_eq!(
        number(&created, "numberOfRecordsUpdated"),
        0,
        "DDL updates no records, got {created}"
    );

    let pets = [(1, "Ada"), (2, "Grace"), (3, "Alan")];
    let (status, batch) = api
        .batch(json!({
            "database": "demo",
            "transactionId": transaction,
            "sql": "INSERT INTO pets (id, name) VALUES (:id, :name)",
            "parameterSets": pets.iter().map(|(id, name)| json!([
                parameter("id", json!({"longValue": id})),
                parameter("name", json!({"stringValue": name})),
            ])).collect::<Vec<_>>(),
        }))
        .await;
    assert_eq!(status, 200, "BatchExecuteStatement failed with {batch}");
    assert_eq!(
        list(&batch, "updateResults").len(),
        pets.len(),
        "one UpdateResult per parameter set, got {batch}"
    );

    // Nothing the transaction did is visible outside it yet — not even the table.
    let (status, outside) = api
        .execute(json!({"database": "demo", "sql": "SELECT id, name FROM pets"}))
        .await;
    assert_eq!(
        status, 400,
        "an uncommitted CREATE TABLE must not be visible outside its transaction, got {outside}"
    );
    assert_eq!(
        error_code(&outside),
        "DatabaseErrorException",
        "the engine's refusal is reported as DatabaseErrorException, got {outside}"
    );

    let (status, committed) = api.commit(&transaction).await;
    assert_eq!(status, 200, "CommitTransaction failed with {committed}");
    assert!(
        committed["transactionStatus"]
            .as_str()
            .is_some_and(|status| !status.is_empty()),
        "CommitTransaction must report a transactionStatus, got {committed}"
    );

    let selected = api
        .run(json!({
            "database": "demo",
            "sql": "SELECT id, name FROM pets",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        selected["records"],
        json!([
            [{"longValue": 1}, {"stringValue": "Ada"}],
            [{"longValue": 2}, {"stringValue": "Grace"}],
            [{"longValue": 3}, {"stringValue": "Alan"}],
        ]),
        "the committed rows come back in insertion order, got {selected}"
    );
    assert_eq!(
        number(&selected, "numberOfRecordsUpdated"),
        0,
        "a SELECT updates no records, got {selected}"
    );
    let metadata = list(&selected, "columnMetadata");
    assert_eq!(metadata.len(), 2, "two columns were selected, got {selected}");
    assert_eq!(metadata[0]["name"], json!("id"), "case id: got {metadata:?}");
    assert_eq!(
        metadata[0]["label"],
        json!("id"),
        "case id: got {metadata:?}"
    );
    assert_eq!(
        metadata[0]["typeName"],
        json!("int4"),
        "case id: got {metadata:?}"
    );
    assert_eq!(
        number(&metadata[1], "nullable"),
        1,
        "case name: a column without NOT NULL is nullable, got {metadata:?}"
    );
    assert_eq!(
        metadata[1]["typeName"],
        json!("varchar"),
        "case name: got {metadata:?}"
    );
    assert_eq!(
        number(&metadata[1], "precision"),
        32,
        "case name: VARCHAR(32)'s declared length is its precision, got {metadata:?}"
    );

    // A rolled-back DELETE leaves the committed rows alone.
    let second = api.begin_transaction(Some("demo")).await;
    assert_ne!(
        second, transaction,
        "each BeginTransaction mints its own id, got {second}"
    );
    let deleted = api
        .run(json!({
            "database": "demo",
            "transactionId": second,
            "sql": "DELETE FROM pets",
        }))
        .await;
    assert_eq!(
        number(&deleted, "numberOfRecordsUpdated"),
        3,
        "the DELETE removed the three stored rows, got {deleted}"
    );
    let (status, rolled_back) = api.rollback(&second).await;
    assert_eq!(status, 200, "RollbackTransaction failed with {rolled_back}");
    let after_rollback = api
        .run(json!({"database": "demo", "sql": "SELECT id, name FROM pets"}))
        .await;
    assert_eq!(
        list(&after_rollback, "records").len(),
        3,
        "a rolled-back DELETE leaves the committed rows, got {after_rollback}"
    );

    // The rolled-back transaction is gone, and so is the committed one.
    for (operation, finished) in [("commit", transaction.as_str()), ("rollback", second.as_str())] {
        let (status, body) = api.commit(finished).await;
        assert_eq!(
            status, 404,
            "case {operation}: a finished transaction id is not found, got {body}"
        );
        assert_eq!(
            error_code(&body),
            "TransactionNotFoundException",
            "case {operation}: got {body}"
        );
    }
}

/// RULE: the `Field` member a value comes back in is decided by the column's
/// PostgreSQL type and the storage class the engine holds the value in — `isNull`
/// for NULL, `booleanValue` for `bool`, `longValue` for an integer type,
/// `doubleValue` for a floating-point type, `stringValue` for a character type,
/// `blobValue` for `bytea` —
/// subject to `resultSetOptions`: the model documents `decimalReturnType=STRING`
/// as the default for a `DECIMAL` column and `DOUBLE_OR_LONG` as "a Long value if
/// its scale is 0, or a Double value otherwise", and `longReturnType=STRING` as
/// returning a `LONG` as a string.
///
/// The same table pins `columnMetadata`'s `typeName` (PostgreSQL's catalog name
/// for the type the column was declared with) and its `java.sql.Types` code.
#[tokio::test]
async fn the_field_member_follows_the_column_type_and_result_set_options() {
    struct Case {
        column: &'static str,
        bound: Value,
        options: Option<Value>,
        expected: Value,
        type_name: &'static str,
        jdbc_type: i64,
    }

    let cases = vec![
        Case {
            column: "value BOOLEAN",
            bound: json!({"booleanValue": true}),
            options: None,
            expected: json!({"booleanValue": true}),
            type_name: "bool",
            jdbc_type: -7,
        },
        Case {
            column: "value BOOLEAN",
            bound: json!({"booleanValue": false}),
            options: None,
            expected: json!({"booleanValue": false}),
            type_name: "bool",
            jdbc_type: -7,
        },
        Case {
            column: "value INT",
            bound: json!({"longValue": 42}),
            options: None,
            expected: json!({"longValue": 42}),
            type_name: "int4",
            jdbc_type: 4,
        },
        Case {
            column: "value SMALLINT",
            bound: json!({"longValue": -32_768}),
            options: None,
            expected: json!({"longValue": -32_768}),
            type_name: "int2",
            jdbc_type: 5,
        },
        Case {
            column: "value BIGINT",
            bound: json!({"longValue": -9_007_199_254_740_993_i64}),
            options: None,
            expected: json!({"longValue": -9_007_199_254_740_993_i64}),
            type_name: "int8",
            jdbc_type: -5,
        },
        Case {
            column: "value DOUBLE PRECISION",
            bound: json!({"doubleValue": 1.5}),
            options: None,
            expected: json!({"doubleValue": 1.5}),
            type_name: "float8",
            jdbc_type: 8,
        },
        Case {
            column: "value REAL",
            bound: json!({"doubleValue": -0.5}),
            options: None,
            expected: json!({"doubleValue": -0.5}),
            type_name: "float4",
            jdbc_type: 7,
        },
        Case {
            column: "value VARCHAR(16)",
            bound: json!({"stringValue": "hello"}),
            options: None,
            expected: json!({"stringValue": "hello"}),
            type_name: "varchar",
            jdbc_type: 12,
        },
        Case {
            column: "value TEXT",
            bound: json!({"stringValue": ""}),
            options: None,
            expected: json!({"stringValue": ""}),
            type_name: "text",
            jdbc_type: 12,
        },
        Case {
            // "AAAA" is the base64 of three zero bytes: restJson1 carries a blob
            // base64-encoded in both directions.
            column: "value BYTEA",
            bound: json!({"blobValue": "AAAA"}),
            options: None,
            expected: json!({"blobValue": "AAAA"}),
            type_name: "bytea",
            jdbc_type: -2,
        },
        Case {
            column: "value VARCHAR(16)",
            bound: json!({"isNull": true}),
            options: None,
            expected: json!({"isNull": true}),
            type_name: "varchar",
            jdbc_type: 12,
        },
        Case {
            column: "value DECIMAL(10,2)",
            bound: json!({"stringValue": "12.34"}),
            options: None,
            expected: json!({"stringValue": "12.34"}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        Case {
            column: "value DECIMAL(10,2)",
            bound: json!({"stringValue": "12.34"}),
            options: Some(json!({"decimalReturnType": "DOUBLE_OR_LONG"})),
            expected: json!({"doubleValue": 12.34}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        Case {
            column: "value DECIMAL(10,0)",
            bound: json!({"stringValue": "12"}),
            options: Some(json!({"decimalReturnType": "DOUBLE_OR_LONG"})),
            expected: json!({"longValue": 12}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        // The COLUMN's declared scale decides, not how the engine happens to hold
        // the value: an integer in a scaled DECIMAL is still "a Double value".
        Case {
            column: "value DECIMAL(10,2)",
            bound: json!({"longValue": 12}),
            options: Some(json!({"decimalReturnType": "DOUBLE_OR_LONG"})),
            expected: json!({"doubleValue": 12.0}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        Case {
            column: "value NUMERIC(8,3)",
            bound: json!({"doubleValue": 2.5}),
            options: None,
            expected: json!({"stringValue": "2.500"}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        // A `numeric` with no declared scale carries each value's own scale, so
        // the value keeps the digits it has.
        Case {
            column: "value NUMERIC",
            bound: json!({"doubleValue": 2.5}),
            options: None,
            expected: json!({"stringValue": "2.5"}),
            type_name: "numeric",
            jdbc_type: 2,
        },
        Case {
            column: "value INT",
            bound: json!({"longValue": 7}),
            options: Some(json!({"longReturnType": "STRING"})),
            expected: json!({"stringValue": "7"}),
            type_name: "int4",
            jdbc_type: 4,
        },
    ];

    for case in cases {
        let api = DataApi::new();
        let label = format!("{} <- {}", case.column, case.bound);
        api.run(json!({"sql": format!("CREATE TABLE probe ({})", case.column)}))
            .await;
        api.run(json!({
            // A value written as TEXT reaches a column of another type through a CAST
            // the statement writes (a `typeHint` is the other way of saying it); what
            // the case is about is the column's own type and the options, so every
            // case binds the same way.
            "sql": format!(
                "INSERT INTO probe (value) VALUES (CAST(:value AS {}))",
                case.column.strip_prefix("value ").unwrap_or(case.column)
            ),
            "parameters": [parameter("value", case.bound.clone())],
        }))
        .await;

        let mut select = json!({
            "sql": "SELECT value FROM probe",
            "includeResultMetadata": true,
        });
        if let Some(options) = case.options.clone() {
            select["resultSetOptions"] = options;
        }
        let selected = api.run(select).await;

        assert_eq!(
            selected["records"],
            json!([[case.expected.clone()]]),
            "case {label} (options {:?}): got {selected}",
            case.options
        );
        let metadata = list(&selected, "columnMetadata");
        assert_eq!(
            metadata.first().map(|column| column["typeName"].clone()),
            Some(json!(case.type_name)),
            "case {label}: got {selected}"
        );
        assert_eq!(
            metadata.first().map_or(0, |column| number(column, "type")),
            case.jdbc_type,
            "case {label}: got {selected}"
        );
    }
}

/// RULE: `numberOfRecordsUpdated` is the number of rows the engine actually
/// inserted, updated or deleted — 0 for DDL and for a statement that only reads —
/// and `BatchExecuteStatement` runs the statement once per parameter set, so N
/// sets of an INSERT leave N rows a later SELECT returns.
///
/// A call carrying NO parameter sets runs the statement once, which is the batch
/// rule's own row in `tests/all/engine_dialects.rs`.
#[tokio::test]
async fn row_counts_are_what_the_engine_changed() {
    for rows in [1_usize, 2, 5, 13] {
        let api = DataApi::new();
        api.run(json!({"sql": "CREATE TABLE items (id INT, label VARCHAR(8))"}))
            .await;

        let (status, batch) = api
            .batch(json!({
                "sql": "INSERT INTO items (id, label) VALUES (:id, :label)",
                "parameterSets": (0..rows).map(|index| json!([
                    parameter("id", json!({"longValue": index})),
                    parameter("label", json!({"stringValue": format!("row{index}")})),
                ])).collect::<Vec<_>>(),
            }))
            .await;
        assert_eq!(status, 200, "case {rows} rows: batch failed with {batch}");
        assert_eq!(
            list(&batch, "updateResults").len(),
            rows,
            "case {rows} rows: one UpdateResult per parameter set, got {batch}"
        );

        let selected = api
            .run(json!({"sql": "SELECT id, label FROM items"}))
            .await;
        assert_eq!(
            selected["records"],
            json!((0..rows)
                .map(|index| json!([
                    {"longValue": index},
                    {"stringValue": format!("row{index}")},
                ]))
                .collect::<Vec<_>>()),
            "case {rows} rows: every inserted row comes back, in order, got {selected}"
        );
        // A statement that RETURNED a result set changed no rows. The model makes
        // `numberOfRecordsUpdated` REQUIRED with a default of 0, so restJson1 always
        // puts it on the wire: "a SELECT does not set it" is observable as the
        // member carrying exactly its default, asserted here on the member itself
        // (not through `number`, which would also accept it missing).
        assert_eq!(
            selected["numberOfRecordsUpdated"],
            json!(0),
            "case {rows} rows: a SELECT changed no rows, got {selected}"
        );

        let updated = api
            .run(json!({"sql": "UPDATE items SET label = 'same'"}))
            .await;
        assert_eq!(
            number(&updated, "numberOfRecordsUpdated"),
            i64::try_from(rows).expect("the row count fits"),
            "case {rows} rows: the UPDATE touched every row, got {updated}"
        );

        let half = api
            .run(json!({"sql": "DELETE FROM items WHERE id % 2 = 0"}))
            .await;
        assert_eq!(
            number(&half, "numberOfRecordsUpdated"),
            i64::try_from(rows.div_ceil(2)).expect("the row count fits"),
            "case {rows} rows: the DELETE removed the even ids, got {half}"
        );
    }
}

/// RULE: `generatedFields` is EMPTY — the member's own documentation says "The
/// generatedFields data isn't supported by Aurora PostgreSQL. To get the values
/// of generated fields, use the RETURNING clause" — while the sequence a `serial`
/// column declares still assigns that column's value when an INSERT leaves it
/// out, so the key is what a RETURNING clause and a later read both report.
///
/// The rows differ along HOW the inserted key is asked for: not asked for at all,
/// through RETURNING, and through a later SELECT; and along WHICH operation
/// inserted, since `BatchExecuteStatement` carries the same member per parameter
/// set.
#[tokio::test]
async fn a_sequence_assigns_the_key_and_generated_fields_stays_empty() {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE notes (id serial PRIMARY KEY, body TEXT NOT NULL)",
    }))
    .await;

    for expected_key in 1..=3_i64 {
        let inserted = api
            .run(json!({
                "sql": "INSERT INTO notes (body) VALUES (:body)",
                "parameters": [parameter("body", json!({"stringValue": format!("note {expected_key}")}))],
            }))
            .await;
        assert_eq!(
            list(&inserted, "generatedFields"),
            Vec::<Value>::new(),
            "case key {expected_key}: generatedFields is not supported by this engine, \
             got {inserted}"
        );
        assert_eq!(
            number(&inserted, "numberOfRecordsUpdated"),
            1,
            "case key {expected_key}: one row was inserted, got {inserted}"
        );
    }

    // RETURNING is the documented way to read a generated field, so it carries
    // the key the sequence assigned.
    let returned = api
        .run(json!({
            "sql": "INSERT INTO notes (body) VALUES (:body) RETURNING id",
            "parameters": [parameter("body", json!({"stringValue": "note 4"}))],
        }))
        .await;
    assert_eq!(
        returned["records"],
        json!([[{"longValue": 4}]]),
        "case RETURNING: the sequence's next value is returned, got {returned}"
    );

    // And a batch, whose own `generatedFields` is empty per parameter set.
    let (status, batch) = api
        .batch(json!({
            "sql": "INSERT INTO notes (body) VALUES (:body)",
            "parameterSets": [
                [parameter("body", json!({"stringValue": "note 5"}))],
                [parameter("body", json!({"stringValue": "note 6"}))],
            ],
        }))
        .await;
    assert_eq!(status, 200, "BatchExecuteStatement failed with {batch}");
    for (index, result) in list(&batch, "updateResults").iter().enumerate() {
        assert_eq!(
            list(result, "generatedFields"),
            Vec::<Value>::new(),
            "case batch row {index}: generatedFields is not supported by this engine, \
             got {batch}"
        );
    }

    let selected = api
        .run(json!({
            "sql": "SELECT id, body FROM notes ORDER BY id",
            "includeResultMetadata": true,
        }))
        .await;
    assert_eq!(
        selected["records"],
        json!([
            [{"longValue": 1}, {"stringValue": "note 1"}],
            [{"longValue": 2}, {"stringValue": "note 2"}],
            [{"longValue": 3}, {"stringValue": "note 3"}],
            [{"longValue": 4}, {"stringValue": "note 4"}],
            [{"longValue": 5}, {"stringValue": "note 5"}],
            [{"longValue": 6}, {"stringValue": "note 6"}],
        ]),
        "every assigned key is the stored id, got {selected}"
    );
    let metadata = list(&selected, "columnMetadata");
    assert_eq!(
        metadata[0]["isAutoIncrement"],
        json!(true),
        "case id: a serial column's value is assigned by its sequence, got {metadata:?}"
    );
    assert_eq!(
        metadata[0]["typeName"],
        json!("serial"),
        "case id: got {metadata:?}"
    );
    assert_eq!(
        number(&metadata[0], "nullable"),
        0,
        "case id: a primary key is NOT NULL, got {metadata:?}"
    );
    assert_eq!(
        number(&metadata[1], "nullable"),
        0,
        "case body: a NOT NULL column is not nullable, got {metadata:?}"
    );

    // The NOT NULL declaration is the engine's, so it is enforced.
    let (status, refused) = api
        .execute(json!({
            "sql": "INSERT INTO notes (body) VALUES (:body)",
            "parameters": [parameter("body", json!({"isNull": true}))],
        }))
        .await;
    assert_eq!(
        status, 400,
        "a NULL in a NOT NULL column is refused, got {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "DatabaseErrorException",
        "the engine's constraint failure is the database's error, got {refused}"
    );
}

/// RULE: with `formatRecordsAs=JSON` the result set is returned as one JSON array
/// of objects in `formattedRecords`, and `records` and `columnMetadata` are blank;
/// with `NONE` (the default) it is returned in `records`.
#[tokio::test]
async fn the_json_record_format_returns_the_result_set_as_json() {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE readings (id INT, label VARCHAR(8), flag BOOLEAN, score DOUBLE PRECISION, note TEXT)",
    }))
    .await;
    let rows = [
        (1, "first", true, 1.5, Some("here")),
        (2, "second", false, -0.25, None),
    ];
    for (id, label, flag, score, note) in rows {
        api.run(json!({
            "sql": "INSERT INTO readings (id, label, flag, score, note) VALUES (:id, :label, :flag, :score, :note)",
            "parameters": [
                parameter("id", json!({"longValue": id})),
                parameter("label", json!({"stringValue": label})),
                parameter("flag", json!({"booleanValue": flag})),
                parameter("score", json!({"doubleValue": score})),
                parameter("note", match note {
                    Some(note) => json!({"stringValue": note}),
                    None => json!({"isNull": true}),
                }),
            ],
        }))
        .await;
    }

    let formatted = api
        .run(json!({
            "sql": "SELECT id, label, flag, score, note FROM readings",
            "includeResultMetadata": true,
            "formatRecordsAs": "JSON",
        }))
        .await;
    assert!(
        list(&formatted, "records").is_empty() && list(&formatted, "columnMetadata").is_empty(),
        "records and columnMetadata are blank in the JSON format, got {formatted}"
    );
    let records: Value = serde_json::from_str(
        formatted["formattedRecords"]
            .as_str()
            .unwrap_or_else(|| panic!("formattedRecords must be a string: {formatted}")),
    )
    .expect("formattedRecords parses as JSON");
    assert_eq!(
        records,
        json!([
            {"id": 1, "label": "first", "flag": true, "score": 1.5, "note": "here"},
            {"id": 2, "label": "second", "flag": false, "score": -0.25, "note": null},
        ]),
        "case JSON: got {formatted}"
    );

    let plain = api
        .run(json!({
            "sql": "SELECT id, label FROM readings",
            "formatRecordsAs": "NONE",
        }))
        .await;
    assert!(
        plain.get("formattedRecords").is_none(),
        "case NONE: formattedRecords is only present for the JSON format, got {plain}"
    );
    assert_eq!(
        list(&plain, "records").len(),
        rows.len(),
        "case NONE: the result set is returned in records, got {plain}"
    );

    // A binary column is rendered as the base64 text this protocol carries a
    // `blobValue` as, so the bytes a client base64-decodes are the bytes stored.
    api.run(json!({"sql": "CREATE TABLE blobs (payload BYTEA)"}))
        .await;
    api.run(json!({
        "sql": "INSERT INTO blobs (payload) VALUES (:payload)",
        "parameters": [parameter("payload", json!({"blobValue": "AAAA"}))],
    }))
    .await;
    let rendered = api
        .run(json!({
            "sql": "SELECT payload FROM blobs",
            "formatRecordsAs": "JSON",
        }))
        .await;
    assert_eq!(
        rendered["formattedRecords"],
        json!("[{\"payload\":\"AAAA\"}]"),
        "case JSON over a binary value: got {rendered}"
    );

    // Nor a number JSON cannot represent: a non-finite REAL the engine produced is
    // refused rather than reported as NULL, which would say the value is missing.
    let (status, not_a_number) = api
        .execute(json!({
            "sql": "SELECT 1e308 * 10 AS big",
            "formatRecordsAs": "JSON",
        }))
        .await;
    assert_eq!(
        status, 400,
        "case JSON over a non-finite number: got {status} {not_a_number}"
    );
    assert_eq!(
        error_code(&not_a_number),
        "UnsupportedResultException",
        "case JSON over a non-finite number: got {not_a_number}"
    );
}

/// RULE: a `transactionId` is refused by the CLASS of the id it is, on every operation
/// that takes one — an id the resource the request names could have issued, which no
/// transaction of that resource is RUNNING under, is HTTP 404
/// `TransactionNotFoundException`, while an id that could NOT have been issued for that
/// resource is an invalid argument, HTTP 400 `ValidationException` "Invalid transaction
/// ID". A statement the ENGINE refuses is HTTP 400 `DatabaseErrorException` while a fault
/// of the CALL — a parameter that cannot be bound, a member that names nothing — is HTTP
/// 400 `ValidationException`; and an operation nobody has implemented still answers HTTP
/// 501 rather than a hollow 200.
#[tokio::test]
async fn the_modelled_failures_are_what_a_client_sees() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE things (id INT)"})).await;

    // An id this resource DID issue and that no longer resolves: the transaction was
    // committed, so the id is one of this cluster's and answers to nothing.
    let ended = api.begin_transaction(None).await;
    let (status, body) = api.commit(&ended).await;
    assert_eq!(status, 200, "the transaction commits: {body}");
    let calls = vec![
        (
            "ExecuteStatement",
            api.execute(json!({"sql": "SELECT id FROM things", "transactionId": ended}))
                .await,
        ),
        (
            "BatchExecuteStatement",
            api.batch(json!({
                "sql": "INSERT INTO things (id) VALUES (:id)",
                "parameterSets": [[parameter("id", json!({"longValue": 1}))]],
                "transactionId": ended,
            }))
            .await,
        ),
        ("CommitTransaction", api.commit(&ended).await),
        ("RollbackTransaction", api.rollback(&ended).await),
    ];
    for (operation, (status, body)) in calls {
        assert_eq!(status, 404, "case {operation}: got {body}");
        assert_eq!(
            error_code(&body),
            "TransactionNotFoundException",
            "case {operation}: got {body}"
        );
    }

    // The ids that could NOT have been issued for this cluster: ids of this service's own
    // form whose resource binding is not this cluster's. The rows span that class: an id a
    // second cluster of this emulator really issued, one a third cluster in another Region
    // issued, and one of the same form whose binding belongs to no cluster at all. (A value
    // that is not of that form at all is the same class of refusal, which
    // `malformed_transaction_ids` pins over its own table.)
    const ANOTHER_CLUSTER_ARN: &str = "arn:aws:rds:us-east-1:000000000000:cluster:other";
    const FAR_CLUSTER_ARN: &str = "arn:aws:rds:us-west-2:000000000000:cluster:far";
    let another_clusters = api.begin_transaction_on(ANOTHER_CLUSTER_ARN).await;
    let far_clusters = api.begin_transaction_on(FAR_CLUSTER_ARN).await;
    // The same form, 48 hexadecimal digits, with a binding that is nobody's.
    let bound_to_nothing = "0".repeat(48);
    for (case, id) in [
        ("another cluster's transaction id", another_clusters.as_str()),
        (
            "a transaction id of a cluster in another Region",
            far_clusters.as_str(),
        ),
        (
            "an id of the same form bound to no cluster",
            bound_to_nothing.as_str(),
        ),
    ] {
        let refusals = vec![
            (
                "ExecuteStatement",
                api.execute(json!({"sql": "SELECT id FROM things", "transactionId": id}))
                    .await,
            ),
            (
                "BatchExecuteStatement",
                api.batch(json!({
                    "sql": "INSERT INTO things (id) VALUES (:id)",
                    "parameterSets": [[parameter("id", json!({"longValue": 1}))]],
                    "transactionId": id,
                }))
                .await,
            ),
            ("CommitTransaction", api.commit(id).await),
            ("RollbackTransaction", api.rollback(id).await),
        ];
        for (operation, (status, body)) in refusals {
            assert_eq!(status, 400, "case {case} on {operation}: got {body}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} on {operation}: got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some("Invalid transaction ID"),
                "case {case} on {operation}: got {body}"
            );
        }
    }
    // Nothing a refused call carried reached this cluster's data, and the transaction the
    // other cluster opened is still usable there.
    let rows = api
        .run(json!({"sql": "SELECT count(*) AS rows FROM things"}))
        .await;
    assert_eq!(
        rows["records"][0][0]["longValue"].as_i64(),
        Some(0),
        "a refused call must have written nothing, got {rows}"
    );
    let (status, body) = api
        .post(
            "/CommitTransaction",
            json!({
                "resourceArn": ANOTHER_CLUSTER_ARN,
                "secretArn": SECRET_ARN,
                "transactionId": another_clusters,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the other cluster's transaction must still be alive there: {body}"
    );

    let refusals = vec![
        (
            "a statement the engine cannot parse",
            json!({"sql": "SELEKT nothing"}),
            "DatabaseErrorException",
        ),
        (
            "a table that does not exist",
            json!({"sql": "SELECT id FROM absent"}),
            "DatabaseErrorException",
        ),
        (
            "a named parameter with no value supplied",
            json!({"sql": "SELECT id FROM things WHERE id = :id"}),
            "ValidationException",
        ),
        (
            "an array parameter, which the model says is not supported",
            json!({
                "sql": "INSERT INTO things (id) VALUES (:id)",
                "parameters": [parameter("id", json!({"arrayValue": {"longValues": [1, 2]}}))],
            }),
            "ValidationException",
        ),
        (
            "isNull set to false, which names no value",
            json!({
                "sql": "INSERT INTO things (id) VALUES (:id)",
                "parameters": [parameter("id", json!({"isNull": false}))],
            }),
            "ValidationException",
        ),
        (
            "more than one statement in one call",
            json!({"sql": "INSERT INTO things (id) VALUES (1); INSERT INTO things (id) VALUES (2)"}),
            "ValidationException",
        ),
    ];
    for (case, members, expected) in refusals {
        let (status, body) = api.execute(members).await;
        assert_eq!(status, 400, "case {case}: got {body}");
        assert_eq!(
            error_code(&body),
            expected,
            "case {case}: expected {expected}, got {body}"
        );
    }

    // A refused statement changed nothing.
    let remaining = api.run(json!({"sql": "SELECT id FROM things"})).await;
    assert!(
        list(&remaining, "records").is_empty(),
        "no refused statement may leave rows behind, got {remaining}"
    );

    // A value the model types as an ARN but that is not one names no cluster and
    // no secret, on every operation that takes the pair.
    let not_an_arn = "demo-cluster";
    let arn_cases = vec![
        (
            "ExecuteStatement with a resourceArn that is not an ARN",
            "/Execute",
            json!({"resourceArn": not_an_arn, "secretArn": SECRET_ARN, "sql": "SELECT 1"}),
        ),
        (
            "ExecuteStatement with a secretArn that is not an ARN",
            "/Execute",
            json!({"resourceArn": RESOURCE_ARN, "secretArn": not_an_arn, "sql": "SELECT 1"}),
        ),
        (
            "BeginTransaction with a resourceArn that is not an ARN",
            "/BeginTransaction",
            json!({"resourceArn": not_an_arn, "secretArn": SECRET_ARN}),
        ),
        (
            "CommitTransaction with a secretArn that is not an ARN",
            "/CommitTransaction",
            json!({"resourceArn": RESOURCE_ARN, "secretArn": not_an_arn, "transactionId": "any"}),
        ),
        (
            "RollbackTransaction with a resourceArn that is not an ARN",
            "/RollbackTransaction",
            json!({"resourceArn": not_an_arn, "secretArn": SECRET_ARN, "transactionId": "any"}),
        ),
        (
            "BatchExecuteStatement with a resourceArn that is not an ARN",
            "/BatchExecute",
            json!({"resourceArn": not_an_arn, "secretArn": SECRET_ARN, "sql": "SELECT 1"}),
        ),
    ];
    for (case, path, body) in arn_cases {
        let (status, answered) = api.post(path, body).await;
        assert_eq!(status, 400, "case {case}: got {answered}");
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: got {answered}"
        );
    }

    // ExecuteSql is deprecated and not implemented; it must say so, not answer 200.
    let (status, body) = api
        .post(
            "/ExecuteSql",
            json!({
                "dbClusterOrInstanceArn": RESOURCE_ARN,
                "awsSecretStoreArn": SECRET_ARN,
                "sqlStatements": "SELECT 1",
            }),
        )
        .await;
    assert_eq!(status, 501, "case ExecuteSql: got {body}");
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "case ExecuteSql: got {body}"
    );
}

/// RULE: caller SQL runs on a real engine, so every engine capability that could
/// reach outside this emulator's own state is taken away: no host path can be
/// opened, created or read, no pragma outside this database's schema may run, and
/// no action the confinement has not considered is allowed.
#[tokio::test]
async fn the_engine_cannot_reach_outside_the_emulators_own_state() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE secrets (id INT)"})).await;

    // Paths that must never come into existence. Nothing in this test writes to
    // disk; the assertion is that the engine did not either.
    let attached = std::env::temp_dir().join("rdsdata-emulator-attach-must-not-exist.db");
    let vacuumed = std::env::temp_dir().join("rdsdata-emulator-vacuum-must-not-exist.db");
    let escapes = vec![
        (
            "ATTACH of a host path",
            format!("ATTACH DATABASE '{}' AS leak", attached.display()),
        ),
        (
            "VACUUM INTO a host path",
            format!("VACUUM INTO '{}'", vacuumed.display()),
        ),
        (
            "DETACH",
            "DETACH DATABASE leak".to_string(),
        ),
        (
            "a pragma that sets a temp-file directory",
            format!("PRAGMA temp_store_directory = '{}'", std::env::temp_dir().display()),
        ),
        (
            "a pragma that switches journalling off",
            "PRAGMA journal_mode = OFF".to_string(),
        ),
        (
            "a pragma that makes the schema writable",
            "PRAGMA writable_schema = ON".to_string(),
        ),
        (
            "the extension loader",
            "SELECT load_extension('/tmp/anything.so')".to_string(),
        ),
        (
            "a virtual table over database pages",
            "CREATE VIRTUAL TABLE pages USING sqlite_dbpage".to_string(),
        ),
    ];
    for (case, sql) in escapes {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 400, "case {case}: expected a refusal, got {body}");
        assert_eq!(
            error_code(&body),
            "DatabaseErrorException",
            "case {case}: got {body}"
        );
    }
    for (case, path) in [("ATTACH", &attached), ("VACUUM INTO", &vacuumed)] {
        assert!(
            !path.exists(),
            "case {case}: the engine must not create a host file, but {} exists",
            path.display()
        );
    }

    // The data plane still works after every refusal.
    let selected = api.run(json!({"sql": "SELECT id FROM secrets"})).await;
    assert!(
        list(&selected, "records").is_empty(),
        "the database is still readable after the refusals, got {selected}"
    );
}

/// RULE: the functions and virtual-table modules caller SQL may reach are an
/// ALLOW-LIST, so one the data plane has not reviewed is refused even though the
/// engine defines it — being absent from a list of refusals is never permission.
///
/// The refused rows are functions and modules this build DOES define (so only the
/// authorizer can be refusing them), each reaching something outside the values in
/// the caller's own tables: code loading, a host file, the raw database pages, the
/// build's identity and its compile options. The allowed rows span the documented
/// families the allow-list names — core scalar, aggregate, date and time, math and
/// JSON — so the rule is not "deny everything".
#[tokio::test]
async fn the_functions_caller_sql_may_reach_are_allow_listed() {
    let api = DataApi::new();

    // Each case names a capability the engine defines and the data plane does not
    // allow. `sqlite_version`/`sqlite_source_id`/`sqlite_compileoption_*` are the
    // sharp ones: they parse and resolve, so a 400 can only come from the
    // authorizer refusing a function that is not on the allow-list.
    let refused = [
        ("the extension loader", "SELECT load_extension('/tmp/x.so')"),
        ("the engine's version", "SELECT sqlite_version()"),
        ("the engine's source id", "SELECT sqlite_source_id()"),
        (
            "the build's compile options",
            "SELECT sqlite_compileoption_get(0)",
        ),
        (
            "a compile option probe",
            "SELECT sqlite_compileoption_used('ENABLE_DBPAGE_VTAB')",
        ),
        ("a row's byte offset in the file", "SELECT sqlite_offset(1)"),
        (
            "the raw database pages, as a module",
            "CREATE VIRTUAL TABLE pages USING sqlite_dbpage",
        ),
        (
            "a pragma module the introspection list does not name",
            "SELECT * FROM pragma_database_list",
        ),
        (
            "another pragma module the introspection list does not name",
            "SELECT * FROM pragma_function_list",
        ),
    ];
    for (case, sql) in refused {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(
            status, 400,
            "case {case}: an unreviewed capability must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DatabaseErrorException",
            "case {case}: got {body}"
        );
    }

    // One row per documented family the allow-list draws from, so the engine is
    // still a usable SQL engine.
    let allowed = [
        ("a core scalar function", "SELECT abs(-7) AS v", json!(7)),
        (
            "a string function",
            "SELECT upper('ab') AS v",
            json!("AB"),
        ),
        (
            "an aggregate function",
            "SELECT count(*) AS v FROM (SELECT 1 UNION ALL SELECT 2)",
            json!(2),
        ),
        (
            "a date and time function",
            "SELECT strftime('%Y', '2020-02-03') AS v",
            json!("2020"),
        ),
        ("a math function", "SELECT round(2.4) AS v", json!(2.0)),
        (
            "a JSON function",
            "SELECT json_extract('{\"a\":\"b\"}', '$.a') AS v",
            json!("b"),
        ),
        (
            "the schema introspection module the data plane itself uses",
            "SELECT count(*) AS v FROM pragma_table_info('sqlite_master')",
            json!(5),
        ),
    ];
    for (case, sql, expected) in allowed {
        let body = api.run(json!({"sql": sql})).await;
        let records = list(&body, "records");
        let value = records
            .first()
            .and_then(|row| row.as_array())
            .and_then(|row| row.first())
            .and_then(|field| field.as_object())
            .and_then(|field| field.values().next().cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            value, expected,
            "case {case}: expected {expected}, got {body}"
        );
    }
}

/// RULE: one request's engine work is bounded on every axis — output size, wall
/// clock, statement complexity, value size, statements per call and stored objects
/// — and a request that passes a bound is refused with a modelled error instead of
/// consuming the emulator.
#[tokio::test]
async fn one_request_cannot_consume_the_emulator() {
    let api = DataApi::new();

    // OUTPUT: an endless result set is cut off at the documented 1 MB.
    let (status, too_large) = api
        .execute(json!({
            "sql": "WITH RECURSIVE forever(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM forever) \
                    SELECT x FROM forever",
        }))
        .await;
    assert_eq!(status, 400, "case endless result set: got {too_large}");
    assert_eq!(
        error_code(&too_large),
        "UnsupportedResultException",
        "case endless result set: got {too_large}"
    );

    // COMPLEXITY: an expression deeper than the engine's configured limit.
    let deep = format!("SELECT {}1{}", "abs(".repeat(400), ")".repeat(400));
    let (status, too_deep) = api.execute(json!({"sql": deep})).await;
    assert_eq!(status, 400, "case deep expression: got {too_deep}");
    assert_eq!(
        error_code(&too_deep),
        "DatabaseErrorException",
        "case deep expression: got {too_deep}"
    );

    // VALUE SIZE: one value larger than a whole response may be.
    let (status, too_big) = api
        .execute(json!({"sql": "SELECT zeroblob(4000000)"}))
        .await;
    assert_eq!(status, 400, "case oversized value: got {too_big}");
    assert_eq!(
        error_code(&too_big),
        "DatabaseErrorException",
        "case oversized value: got {too_big}"
    );

    // FORMATTED OUTPUT: the JSON body is charged for what it EMITS, not only for
    // the values the engine produced. The column label comes from the request and
    // repeats once per row, so a result set whose values fit the cap can still
    // describe a body far past it; the rows below differ in how the product is
    // reached — many rows with a long label, and fewer rows with a longer one.
    for (case, label_len, rows) in [("a long alias", 20_000, 2_000), ("a longer alias", 50_000, 100)]
    {
        let label = "a".repeat(label_len);
        let (status, amplified) = api
            .execute(json!({
                "sql": format!(
                    "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < {rows}) \
                     SELECT x AS \"{label}\" FROM c"
                ),
                "formatRecordsAs": "JSON",
            }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the formatted body must be refused at the response cap, got {status}"
        );
        assert_eq!(
            error_code(&amplified),
            "UnsupportedResultException",
            "case {case}: got {}",
            amplified["__type"]
        );
    }

    // And a formatted body that fits the cap is still returned, so the bound is a
    // bound rather than a refusal of the JSON format.
    let formatted = api
        .run(json!({
            "sql": "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 10) \
                    SELECT x AS \"value\" FROM c",
            "formatRecordsAs": "JSON",
        }))
        .await;
    assert_eq!(
        formatted["formattedRecords"].as_str().map(|records| records
            .matches("\"value\"")
            .count()),
        Some(10),
        "case a small formatted body: expected ten records, got {formatted}"
    );

    // OUTPUT IS A REQUEST'S BUDGET, NOT A STATEMENT'S: a batch whose every
    // statement returns a value that fits on its own is still refused once the
    // REQUEST's total passes the cap, so N parameter sets cannot occupy N times
    // the memory one statement may.
    let sets = vec![json!([parameter("size", json!({"longValue": 400_000}))]); 32];
    let (status, batched) = api
        .batch(json!({
            "sql": "SELECT zeroblob(:size)",
            "parameterSets": sets,
        }))
        .await;
    assert_eq!(
        status, 400,
        "case a batch past the request's output budget: got {status} {batched}"
    );
    // `BatchExecuteStatement` does not model `UnsupportedResultException` — it
    // returns no result set — so a result-shaped refusal of a statement the engine
    // RAN is reported under the database's own error class, and the message says
    // which bound was reached.
    assert_eq!(
        error_code(&batched),
        "DatabaseErrorException",
        "case a batch past the request's output budget: got {batched}"
    );

    // One statement returning that same 400 kB VALUE is served, so the bound is
    // the request's total and not a refusal of the value.
    let single = api
        .run(json!({
            "sql": "SELECT zeroblob(:size) AS payload",
            "parameters": [parameter("size", json!({"longValue": 400_000}))],
        }))
        .await;
    let returned = list(&single, "records");
    assert_eq!(
        returned
            .first()
            .and_then(|row| row.as_array())
            .and_then(|row| row.first())
            .and_then(|field| field.get("blobValue"))
            .and_then(Value::as_str)
            // base64 of 400_000 zero bytes, which is what the wire carries.
            .map(str::len),
        Some(400_000usize.div_ceil(3) * 4),
        "case one 400 kB value inside the budget: got {single}"
    );

    // STATEMENTS PER CALL: more parameter sets than one batch may run.
    let over_cap = vec![json!([parameter("id", json!({"longValue": 1}))]);
        service_com_amazonaws_rdsdata_rdsdataservice::state::MAX_PARAMETER_SETS + 1];
    api.run(json!({"sql": "CREATE TABLE many (id INT)"})).await;
    let (status, too_many) = api
        .batch(json!({
            "sql": "INSERT INTO many (id) VALUES (:id)",
            "parameterSets": over_cap,
        }))
        .await;
    assert_eq!(status, 400, "case too many parameter sets: got {too_many}");
    assert_eq!(
        error_code(&too_many),
        "ValidationException",
        "case too many parameter sets: got {too_many}"
    );

    // STORED OBJECTS: a stream of fresh cluster ARNs stops at the cap instead of
    // growing the store without limit. A fresh emulator, so the only clusters are
    // the ones this loop names. This cap is the EMULATOR's own room rather than a
    // property of the request, so it is the modelled `ServiceUnavailableError`
    // (503) and not a bad request, unlike the parameter-set cap above.
    let api = DataApi::new();
    let clusters = service_com_amazonaws_rdsdata_rdsdataservice::state::MAX_CLUSTERS;
    for index in 0..clusters {
        let (status, body) = api
            .post(
                "/Execute",
                json!({
                    "resourceArn": format!("arn:aws:rds:us-east-1:000000000000:cluster:c{index}"),
                    "secretArn": SECRET_ARN,
                    "sql": "CREATE TABLE t (id INT)",
                }),
            )
            .await;
        assert_eq!(status, 200, "case cluster {index}: got {body}");
    }
    let (status, refused) = api
        .post(
            "/Execute",
            json!({
                "resourceArn": format!("arn:aws:rds:us-east-1:000000000000:cluster:c{clusters}"),
                "secretArn": SECRET_ARN,
                "sql": "CREATE TABLE t (id INT)",
            }),
        )
        .await;
    assert_eq!(
        status, 503,
        "case one cluster past the cap: got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceUnavailableError",
        "case one cluster past the cap: got {refused}"
    );
}

/// RULE: a capped slot belongs to what a caller actually created. Naming a cluster
/// or a database is what brings it into being here, so an address a request only
/// read — or named in a statement the engine refused — holds no slot: the store
/// forgets a cluster whose databases carry no schema and which has no open
/// transaction, which no caller can tell from one that was never created.
///
/// The rows differ along WHAT only named the object: a read of an address never
/// written, and a statement the engine refused. Each fills the cap that many times
/// over and then asserts a real caller is still served — and the last case shows
/// the converse, that a database a caller DID create keeps its slot.
#[tokio::test]
async fn naming_a_cluster_or_database_does_not_spend_its_cap() {
    let clusters = service_com_amazonaws_rdsdata_rdsdataservice::state::MAX_CLUSTERS;
    let databases = service_com_amazonaws_rdsdata_rdsdataservice::state::MAX_DATABASES_PER_CLUSTER;

    for (case, sql, expected) in [
        ("a read of an address never written", "SELECT 1", 200),
        ("a statement the engine refused", "SELEKT 1", 400),
    ] {
        // Clusters: twice the cap's worth of fresh ARNs, none of which creates
        // anything.
        let api = DataApi::new();
        for index in 0..(clusters * 2) {
            let (status, body) = api
                .post(
                    "/Execute",
                    json!({
                        "resourceArn": format!("arn:aws:rds:us-east-1:000000000000:cluster:probe{index}"),
                        "secretArn": SECRET_ARN,
                        "sql": sql,
                    }),
                )
                .await;
            assert_eq!(
                status, expected,
                "case {case}: cluster probe {index} answered {status} {body}"
            );
        }
        let created = api.run(json!({"sql": "CREATE TABLE kept (id INT)"})).await;
        assert_eq!(
            number(&created, "numberOfRecordsUpdated"),
            0,
            "case {case}: a real caller must still be served after the probes, got {created}"
        );

        // Databases: the same, within one cluster.
        let api = DataApi::new();
        for index in 0..(databases * 2) {
            let (status, body) = api
                .execute(json!({"database": format!("probe{index}"), "sql": sql}))
                .await;
            assert_eq!(
                status, expected,
                "case {case}: database probe {index} answered {status} {body}"
            );
        }
        let created = api
            .run(json!({"database": "kept", "sql": "CREATE TABLE kept (id INT)"}))
            .await;
        assert_eq!(
            number(&created, "numberOfRecordsUpdated"),
            0,
            "case {case}: a real database must still be served after the probes, got {created}"
        );

        // What the caller created is still there, and still holds its slot.
        let read_back = api
            .run(json!({"database": "kept", "sql": "SELECT id FROM kept"}))
            .await;
        assert!(
            list(&read_back, "records").is_empty(),
            "case {case}: the created table must survive the sweep, got {read_back}"
        );
    }

    // A database that holds ONLY a TEMP object is not empty either: a TEMP table
    // lives as long as its connection, so across requests, and dropping the
    // database would make the caller's table vanish — which is exactly what an
    // unobservable sweep must never do.
    let api = DataApi::new();
    api.run(json!({"database": "scratch", "sql": "CREATE TEMP TABLE held (id INT)"}))
        .await;
    api.run(json!({"database": "scratch", "sql": "INSERT INTO held (id) VALUES (7)"}))
        .await;
    for index in 0..(databases * 2) {
        let (status, body) = api
            .execute(json!({"database": format!("probe{index}"), "sql": "SELECT 1"}))
            .await;
        assert_eq!(
            status, 200,
            "case a temp-only database: probe {index} answered {status} {body}"
        );
    }
    let held = api
        .run(json!({"database": "scratch", "sql": "SELECT id FROM held"}))
        .await;
    assert_eq!(
        held["records"],
        json!([[{"longValue": 7}]]),
        "case a temp-only database: a TEMP table must survive the sweep, got {held}"
    );
}

/// RULE: EVERY schema a caller's statement can write into is bounded by the same
/// page cap — the stored tables and the `temp` schema a TEMP object lives in — so
/// an object that outlives the statement which created it cannot grow the process,
/// and the engine answers the cap with a refusal instead of running on.
///
/// The rows differ along WHERE the fill lands: the stored schema, the `temp`
/// schema of the default database, and the `temp` schema of a named database
/// (a second connection, created on demand). A TEMP object is the case that needs
/// its own cap: it is not part of `main`, and it lives for as long as the stored
/// database does, so a per-statement time budget does not bound it.
#[tokio::test]
async fn every_schema_a_statement_writes_into_is_bounded() {
    // Each fill writes rows just under the engine's per-value cap, one per step of
    // an endless recursion, so nothing but a page cap can stop it. A capped schema
    // ends the statement with the engine's own "full" failure — a bad request —
    // while an UNCAPPED one would keep allocating until the statement deadline and
    // answer `StatementTimeoutException`, so the code and the elapsed time are what
    // tell the two apart.
    fn fill(table: &str) -> String {
        format!(
            "INSERT INTO {table} (payload) SELECT zeroblob(900000) FROM \
             (WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT x FROM n)"
        )
    }
    let cases = [
        (
            "the stored schema",
            "CREATE TABLE stored (payload BYTEA)",
            "stored",
            None,
        ),
        (
            "the temp schema of the default database",
            "CREATE TEMP TABLE scratch (payload BYTEA)",
            "scratch",
            None,
        ),
        (
            "the temp schema of a named database",
            "CREATE TEMP TABLE scratch (payload BYTEA)",
            "scratch",
            Some("shop"),
        ),
    ];
    for (case, create, table, database) in cases {
        let api = DataApi::new();
        let mut statement = json!({"sql": create});
        if let Some(database) = database {
            statement["database"] = json!(database);
        }
        api.run(statement).await;

        let mut filling = json!({"sql": fill(table)});
        if let Some(database) = database {
            filling["database"] = json!(database);
        }
        let started = std::time::Instant::now();
        let (status, refused) = api.execute(filling).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the schema's page cap to refuse the fill, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DatabaseErrorException",
            "case {case}: a cap reached is the engine refusing the statement, not a deadline, got {refused}"
        );
        assert!(
            started.elapsed() < service_com_amazonaws_rdsdata_rdsdataservice::state::STATEMENT_TIME_BUDGET,
            "case {case}: the cap must stop the fill before the statement budget does, took {:?}",
            started.elapsed()
        );

        // The database is still usable afterwards, and the refused statement left
        // nothing behind: the engine rolled it back.
        let mut counting = json!({"sql": format!("SELECT count(*) AS rows FROM {table}")});
        if let Some(database) = database {
            counting["database"] = json!(database);
        }
        let counted = api.run(counting).await;
        assert_eq!(
            counted["records"],
            json!([[{"longValue": 0}]]),
            "case {case}: the refused fill must leave no rows, got {counted}"
        );
    }
}

/// RULE: a statement that will not finish is aborted at its deadline and reported
/// as the modelled timeout, rather than occupying the emulator.
#[tokio::test]
async fn an_endless_statement_is_aborted_at_its_deadline() {
    let api = DataApi::new();
    let started = std::time::Instant::now();
    // Counting an endless recursion returns no row until it finishes, so only the
    // deadline can stop it.
    let (status, timed_out) = api
        .execute(json!({
            "sql": "WITH RECURSIVE forever(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM forever) \
                    SELECT count(*) FROM forever",
        }))
        .await;
    assert_eq!(status, 400, "case endless statement: got {timed_out}");
    assert_eq!(
        error_code(&timed_out),
        "StatementTimeoutException",
        "case endless statement: got {timed_out}"
    );
    assert!(
        started.elapsed() < service_com_amazonaws_rdsdata_rdsdataservice::state::REQUEST_TIME_BUDGET,
        "case endless statement: the statement budget must cut in before the request budget, took {:?}",
        started.elapsed()
    );
}

/// RULE: a statement is bounded at every point the engine can be stopped at, not
/// only between its instructions. A pattern comparison is ONE instruction, so the
/// pattern a statement carries is bounded by the engine's own pattern-length
/// limit (set in `src/state.rs`, far below the engine's default), and a statement
/// that outruns its deadline inside one operation is unwound from outside the
/// engine — so every call answers with a modelled response inside the request
/// budget, and the data plane is still usable afterwards.
///
/// The rows differ along the pattern: one a client writes, one past the limit, and
/// one whose wildcards a pattern matcher would backtrack over.
#[tokio::test]
async fn a_pattern_comparison_is_bounded_and_leaves_the_data_plane_usable() {
    const REQUEST_BUDGET: std::time::Duration =
        service_com_amazonaws_rdsdata_rdsdataservice::state::REQUEST_TIME_BUDGET;
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE haystack (straw TEXT)"}))
        .await;
    api.run(json!({
        "sql": "INSERT INTO haystack (straw) VALUES (:straw)",
        "parameters": [parameter("straw", json!({"stringValue": "a".repeat(4096)}))],
    }))
    .await;

    for (case, pattern, served) in [
        ("a pattern a client writes", "%aaa%".to_string(), true),
        (
            "a pattern of wildcards a matcher backtracks over",
            format!("{}b", "%a".repeat(32)),
            true,
        ),
        ("a pattern past the engine's limit", "%a".repeat(4096), false),
    ] {
        let started = std::time::Instant::now();
        let (status, body) = api
            .execute(json!({
                "sql": "SELECT count(*) AS found FROM haystack WHERE straw LIKE :pattern",
                "parameters": [parameter("pattern", json!({"stringValue": pattern}))],
            }))
            .await;
        assert_eq!(
            status,
            if served { 200 } else { 400 },
            "case {case}: got {status} {body}"
        );
        if !served {
            // This refusal is the EMULATOR's own bound surfacing, not a modelled
            // AWS failure: the real engine would run the comparison. What is
            // asserted here is that the bound holds and is reported as a refusal
            // of the request, rather than occupying the data plane.
            assert_eq!(
                error_code(&body),
                "DatabaseErrorException",
                "case {case}: a pattern the engine refuses is reported as the database's \
                 own error, got {body}"
            );
        }
        assert!(
            started.elapsed() < REQUEST_BUDGET,
            "case {case}: the call must answer inside the request budget, took {:?}",
            started.elapsed()
        );
    }

    // The data plane is still there afterwards: nothing was left holding it.
    let counted = api
        .run(json!({"sql": "SELECT count(*) AS rows FROM haystack"}))
        .await;
    assert_eq!(
        counted["records"],
        json!([[{"longValue": 1}]]),
        "case after the refused patterns: the row is still readable, got {counted}"
    );
}

/// RULE: EVERY call answers, with a modelled response, inside the budget one
/// request has — whatever else the data plane is doing. A statement that cannot
/// finish is aborted at its own deadline, a call that has to WAIT for a data
/// plane another call is holding is answered at the outer one, and a statement whose
/// own TEXT is what grows costs work linear in that text, so no request is
/// ever left open.
///
/// The rows differ along how many calls contend for the data plane at once: one
/// that only has to outrun its own deadline, and several that have to wait for
/// the ones in front of them as well — and then along what one call's own statement
/// asks for.
#[tokio::test]
async fn every_call_answers_inside_the_request_budget() {
    // Counting an endless recursion returns no row until it finishes, so nothing
    // but a deadline ends it, and it holds the data plane while it runs.
    const ENDLESS: &str = "WITH RECURSIVE forever(x) AS \
                           (SELECT 1 UNION ALL SELECT x + 1 FROM forever) \
                           SELECT count(*) FROM forever";
    const REQUEST_BUDGET: std::time::Duration =
        service_com_amazonaws_rdsdata_rdsdataservice::state::REQUEST_TIME_BUDGET;

    for contending in [1_usize, 3] {
        let api = Arc::new(DataApi::new());
        let started = std::time::Instant::now();
        let calls = (0..contending)
            .map(|_| {
                let api = Arc::clone(&api);
                tokio::spawn(async move { api.execute(json!({"sql": ENDLESS})).await })
            })
            .collect::<Vec<_>>();
        for call in calls {
            let (status, answered) = call
                .await
                .unwrap_or_else(|joined| panic!("case {contending} calls: {joined}"));
            assert_eq!(
                status, 400,
                "case {contending} calls: expected a modelled refusal, got {status} {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "StatementTimeoutException",
                "case {contending} calls: expected the modelled timeout, got {answered}"
            );
        }
        // Each call is bounded by the request budget, so the whole set is bounded
        // by the budget of the one that waited for all the others.
        let bound = REQUEST_BUDGET * u32::try_from(contending).expect("a small count");
        assert!(
            started.elapsed() < bound,
            "case {contending} calls: expected every call answered within {bound:?}, took {:?}",
            started.elapsed()
        );
    }

    // And a statement whose TEXT is what grows — as many placeholder tokens as the
    // level bound admits — is answered inside one request's budget too: what the data
    // plane does per placeholder costs one pass over the statement, not one per
    // placeholder. A statement past that bound is refused, as the suite that pins the
    // bound asserts.
    let api = DataApi::new();
    let placeholders = (0..400)
        .map(|index| format!(":p{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let parameters = (0..400)
        .map(|index| parameter(&format!("p{index}"), json!({"stringValue": "a"})))
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let (status, answered) = api
        .execute(json!({
            "sql": format!("SELECT {placeholders}"),
            "parameters": parameters,
        }))
        .await;
    assert_eq!(
        status, 200,
        "case four hundred placeholders: expected the statement to run, got {status} {answered}"
    );
    assert!(
        started.elapsed() < REQUEST_BUDGET,
        "case four hundred placeholders: expected an answer within {REQUEST_BUDGET:?}, took {:?}",
        started.elapsed()
    );
}

/// RULE: a `typeHint` on a `stringValue` names the type that string is SENT AS, so the
/// string has to be a value of that type — the documented form is accepted and stored,
/// and anything else is refused as a value the service CANNOT PARSE as that type,
/// naming the hint and the string, in the class a failure to read the caller's data is
/// — and a hint on any other `Field` member, where the member already names the type,
/// is ignored.
///
/// The formats come from the model: `DATE` is `YYYY-MM-DD`, `TIME` is
/// `HH:MM:SS[.FFF]`, `TIMESTAMP` is `YYYY-MM-DD HH:MM:SS[.FFF]`, and `DECIMAL`,
/// `JSON` and `UUID` are a decimal number, a JSON document and a UUID
/// (`smithy_query(query="shape", shape="SqlParameter")`, member `typeHint`;
/// `TypeHint` lists exactly those six values).
#[tokio::test]
async fn a_type_hint_decides_what_a_string_parameter_may_be() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE hinted (hint VARCHAR(16), value TEXT)"}))
        .await;

    // One row per hint the model declares: a value of the documented form, one that is
    // the right shape for some OTHER hint (so a hint that was ignored would let it
    // through), and the sentence the refusal of that string carries — the hint and the
    // string for a value the service cannot read as that type, and the PARSER's own
    // complaint for a document, which is what a document type's input error says.
    let cases = [
        (
            "DATE",
            "2024-02-29",
            "2024-02-30",
            "Cannot parse DATE parameter: \"2024-02-30\"".to_string(),
        ),
        (
            "TIME",
            "23:59:59.500",
            "24:00:00",
            "Cannot parse TIME parameter: \"24:00:00\"".to_string(),
        ),
        (
            "TIMESTAMP",
            "2024-01-31 00:00:00",
            "2024-01-31T00:00:00",
            "Cannot parse TIMESTAMP parameter: \"2024-01-31T00:00:00\"".to_string(),
        ),
        (
            "DECIMAL",
            "-12345678901234567890.25",
            "12.5e3",
            "Cannot parse DECIMAL parameter: \"12.5e3\"".to_string(),
        ),
        (
            "JSON",
            r#"{"a":[1,2,null]}"#,
            "{a:1}",
            "invalid input syntax for type json: key must be a string at line 1 column 2"
                .to_string(),
        ),
        (
            "UUID",
            "4bcd3f0e-7a25-4f8c-9c3a-1b2c3d4e5f60",
            "4bcd3f0e-7a25-4f8c-9c3a",
            "Cannot parse UUID parameter: \"4bcd3f0e-7a25-4f8c-9c3a\"".to_string(),
        ),
    ];
    for (hint, accepted, refused, expected_message) in cases {
        let inserted = api
            .run(json!({
                "sql": "INSERT INTO hinted (hint, value) VALUES (:hint, :value)",
                "parameters": [
                    parameter("hint", json!({"stringValue": hint})),
                    json!({"name": "value", "value": {"stringValue": accepted}, "typeHint": hint}),
                ],
            }))
            .await;
        assert_eq!(
            number(&inserted, "numberOfRecordsUpdated"),
            1,
            "case {hint} accepting {accepted:?}: expected 1 row inserted, got {inserted}"
        );

        let read_back = api
            .run(json!({
                "sql": "SELECT value FROM hinted WHERE hint = :hint",
                "parameters": [parameter("hint", json!({"stringValue": hint}))],
            }))
            .await;
        assert_eq!(
            read_back["records"],
            json!([[{"stringValue": accepted}]]),
            "case {hint}: the accepted value is stored as it was sent, got {read_back}"
        );

        let (status, body) = api
            .execute(json!({
                "sql": "INSERT INTO hinted (hint, value) VALUES (:hint, :value)",
                "parameters": [
                    parameter("hint", json!({"stringValue": hint})),
                    json!({"name": "value", "value": {"stringValue": refused}, "typeHint": hint}),
                ],
            }))
            .await;
        assert_eq!(
            status, 400,
            "case {hint} refusing {refused:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DatabaseErrorException",
            "case {hint} refusing {refused:?}: the value was SENT as that type, so\
             failing to read it is the data plane's error, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(expected_message),
            "case {hint} refusing {refused:?}: expected the sentence that names the \
             failure to read it, got {body}"
        );
    }

    // The edges of each documented format: the first and last value inside it,
    // and one just outside.
    let bounds = [
        ("DATE", "0001-01-01", 200),
        ("DATE", "9999-12-31", 200),
        ("DATE", "2023-02-29", 400),
        ("DATE", "2024-1-01", 400),
        ("TIME", "00:00:00", 200),
        ("TIME", "23:59:59", 200),
        ("TIME", "23:59:59.9", 200),
        ("TIME", "23:59:60", 400),
        ("TIMESTAMP", "2024-02-29 23:59:59.999", 200),
        ("TIMESTAMP", "2024-02-29 24:00:00", 400),
        ("DECIMAL", "0", 200),
        ("DECIMAL", "-0.0001", 200),
        ("DECIMAL", "1.", 400),
    ];
    for (hint, value, expected) in bounds {
        let (status, body) = api
            .execute(json!({
                "sql": "INSERT INTO hinted (hint, value) VALUES (:hint, :value)",
                "parameters": [
                    parameter("hint", json!({"stringValue": hint})),
                    json!({"name": "value", "value": {"stringValue": value}, "typeHint": hint}),
                ],
            }))
            .await;
        assert_eq!(
            status, expected,
            "case {hint} at {value:?}: expected HTTP {expected}, got {status} {body}"
        );
    }

    // A seventh hint is not a value `TypeHint` has, so it is refused rather than
    // dropped.
    let (status, body) = api
        .execute(json!({
            "sql": "INSERT INTO hinted (hint, value) VALUES (:hint, :value)",
            "parameters": [
                parameter("hint", json!({"stringValue": "unknown"})),
                json!({"name": "value", "value": {"stringValue": "x"}, "typeHint": "INTERVAL"}),
            ],
        }))
        .await;
    assert_eq!(
        status, 400,
        "case an unmodelled hint: expected HTTP 400, got {status} {body}"
    );

    // And a hint on a member that is not `stringValue` is ignored: the member already
    // names the type, so the value goes in unchanged — for a value of a type, and for a
    // NULL, which has no type to be sent as.
    for (case, value) in [
        ("a hint on longValue", json!({"longValue": 7})),
        ("a hint on a NULL", json!({"isNull": true})),
    ] {
        let inserted = api
            .run(json!({
                "sql": "INSERT INTO hinted (hint, value) VALUES (:hint, :value)",
                "parameters": [
                    parameter("hint", json!({"stringValue": case})),
                    json!({"name": "value", "value": value, "typeHint": "DATE"}),
                ],
            }))
            .await;
        assert_eq!(
            number(&inserted, "numberOfRecordsUpdated"),
            1,
            "case {case}: expected 1 row inserted, got {inserted}"
        );
    }
}

/// RULE: `columnMetadata` is returned for a result set ONLY when
/// `includeResultMetadata` is true; the member is modelled with a default of
/// false, so omitting it and sending it false must both leave the metadata out
/// while still returning the rows.
#[tokio::test]
async fn column_metadata_is_returned_only_when_the_caller_asks_for_it() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE sensors (id INT, label VARCHAR(16))"}))
        .await;
    let rows = [(1, "north"), (2, "south")];
    for (id, label) in rows {
        api.run(json!({
            "sql": "INSERT INTO sensors (id, label) VALUES (:id, :label)",
            "parameters": [
                parameter("id", json!({"longValue": id})),
                parameter("label", json!({"stringValue": label})),
            ],
        }))
        .await;
    }

    // One row per form the member can arrive in: absent, explicitly false, true.
    let cases = [
        ("omitted", json!({"sql": "SELECT id, label FROM sensors"}), 0),
        (
            "false",
            json!({"sql": "SELECT id, label FROM sensors", "includeResultMetadata": false}),
            0,
        ),
        (
            "true",
            json!({"sql": "SELECT id, label FROM sensors", "includeResultMetadata": true}),
            2,
        ),
    ];
    for (case, members, expected) in cases {
        let body = api.run(members).await;
        let metadata = list(&body, "columnMetadata");
        assert_eq!(
            metadata.len(),
            expected,
            "case includeResultMetadata={case}: expected {expected} columnMetadata \
             entries, got {metadata:?} in {body}"
        );
        assert_eq!(
            list(&body, "records").len(),
            rows.len(),
            "case includeResultMetadata={case}: the rows are returned either way, got {body}"
        );
    }
}

/// RULE: a transaction reads the committed rows it started from plus its own
/// writes, and nobody else sees those writes until it commits — checked from both
/// sides of the same statement sequence. What a transaction reads once ANOTHER
/// writer has committed is the rule `tests/all/read_committed_transactions.rs` pins.
#[tokio::test]
async fn a_transaction_sees_its_own_writes_and_no_one_else_sees_them() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE ledger (id INT, amount DECIMAL(12,2))"}))
        .await;
    api.run(json!({
        "sql": "INSERT INTO ledger (id, amount) VALUES (:id, :amount)",
        "parameters": [
            parameter("id", json!({"longValue": 1})),
            // An exact number is sent as one: the hint names the type the string is
            // sent as, which is what the column takes.
            json!({"name": "amount", "value": {"stringValue": "10.00"}, "typeHint": "DECIMAL"}),
        ],
    }))
    .await;

    let transaction = api.begin_transaction(None).await;

    // The transaction starts from the committed rows.
    let inside = api
        .run(json!({"sql": "SELECT id, amount FROM ledger", "transactionId": transaction}))
        .await;
    assert_eq!(
        inside["records"],
        json!([[{"longValue": 1}, {"stringValue": "10.00"}]]),
        "a transaction starts from what was committed, got {inside}"
    );

    api.run(json!({
        "sql": "INSERT INTO ledger (id, amount) VALUES (:id, :amount)",
        "transactionId": transaction,
        "parameters": [
            parameter("id", json!({"longValue": 2})),
            json!({"name": "amount", "value": {"stringValue": "-2.50"}, "typeHint": "DECIMAL"}),
        ],
    }))
    .await;

    let inside = api
        .run(json!({"sql": "SELECT id FROM ledger", "transactionId": transaction}))
        .await;
    assert_eq!(
        list(&inside, "records").len(),
        2,
        "the transaction sees its own write, got {inside}"
    );

    let outside = api.run(json!({"sql": "SELECT id FROM ledger"})).await;
    assert_eq!(
        list(&outside, "records").len(),
        1,
        "an uncommitted write is invisible outside the transaction, got {outside}"
    );

    let (status, body) = api.commit(&transaction).await;
    assert_eq!(status, 200, "CommitTransaction failed with {body}");

    let after = api
        .run(json!({"sql": "SELECT id, amount FROM ledger"}))
        .await;
    assert_eq!(
        after["records"],
        json!([
            [{"longValue": 1}, {"stringValue": "10.00"}],
            [{"longValue": 2}, {"stringValue": "-2.50"}],
        ]),
        "a committed write is visible to a later non-transactional read, got {after}"
    );
}
