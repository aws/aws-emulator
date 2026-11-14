// SPDX-License-Identifier: CC0-1.0

//! The data plane of a Redshift cluster: `COPY` from Amazon S3, `UNLOAD` to it,
//! and a Spectrum external schema whose tables are read out of it.
//!
//! Every suite here drives the emulator over the wire, with the services a
//! statement really reaches mounted beside the Data API on ONE engine — Amazon
//! Redshift for the cluster, Amazon S3 for the objects, IAM for the role a
//! statement names and AWS Glue for the Data Catalog an external schema is
//! bound to — so what is asserted is what a client sees: an object staged with
//! `PutObject` is what a `COPY` loads, and an object an `UNLOAD` wrote is what
//! `GetObject` serves.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. a `COPY` loads the rows the objects at its source carry, in whichever
//!    format and with whichever options the statement names;
//! 2. a row the target's columns cannot take is REJECTED and recorded in
//!    `STL_LOAD_ERRORS`, and a load that rejects more rows than `MAXERROR`
//!    allows fails and loads nothing;
//! 3. a load or an export whose IAM role may not read or write the objects
//!    fails with Amazon S3's own refusal, raised inside the cluster's request;
//! 4. an `UNLOAD` writes the query's rows as objects and, with `MANIFEST`, a
//!    manifest that lists them, and refuses a destination that is not empty
//!    unless `ALLOWOVERWRITE` says otherwise;
//! 5. a Spectrum external table serves the rows the objects at its catalog
//!    location carry, without any of them being loaded into the database.
//!
//! This module also holds the HARNESS the parameter suites share
//! ([`Harness`]), so every suite that needs a cluster with its data plane
//! mounted drives one harness rather than a copy of it each.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService;

/// The Region every call this harness makes is signed for, so the cluster, the
/// bucket, the role and the catalog are all in one Region.
pub const REGION: &str = "us-east-1";

/// The database every statement here runs in: the one a cluster is created
/// with when its create names none.
pub const DATABASE: &str = "dev";

/// The database user every statement here runs as.
pub const DB_USER: &str = "awsuser";

/// The wait every submit asks for: the modelled ceiling, which outlasts the
/// engine's own execution bound, so a statement's recorded outcome is the
/// engine's verdict and never this request's patience on a loaded host.
pub const WAIT_SECONDS: i32 = 30;

/// How long a poll for a clock-owed transition waits before giving up —
/// generous, because what ends it is the CONDITION and not the clock.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll sleeps between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One emulator carrying the Data API, Amazon Redshift, Amazon S3, IAM and AWS
/// Glue over ONE shared engine, the way the router mounts them in production.
pub struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    /// A fresh emulator with every service a cluster's data plane reaches.
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// Call one Data API operation and answer its status and body.
    pub async fn data(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.json("redshift-data", "RedshiftData", operation, body)
            .await
    }

    /// Call one Data API operation and require a 200.
    pub async fn data_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.data(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed for {body}, got {status} {response}"
        );
        response
    }

    /// Call one awsJson 1.1 operation of the service whose signing name is
    /// `signing_name`, under the target prefix `prefix`.
    pub async fn json(
        &self,
        signing_name: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header("authorization", authorization(signing_name))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Call one AWS Glue operation and require a 200.
    pub async fn glue_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.json("glue", "AWSGlue", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed for {body}, got {status} {response}"
        );
        response
    }

    /// Call one awsQuery operation — Amazon Redshift's protocol and IAM's — and
    /// answer its status and body text.
    pub async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        parameters: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in parameters {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization(signing_name))
            .body(bytes::Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).into_owned();
        (status, text)
    }

    /// Call one Amazon Redshift operation and require a 200.
    pub async fn redshift_ok(&self, action: &str, parameters: &[(&str, &str)]) -> String {
        let (status, body) = self.query("redshift", "2012-12-01", action, parameters).await;
        assert_eq!(
            status, 200,
            "{action} must succeed for {parameters:?}, got {status} {body}"
        );
        body
    }

    /// Call one IAM operation and require a 200.
    pub async fn iam_ok(&self, action: &str, parameters: &[(&str, &str)]) -> String {
        let (status, body) = self.query("iam", "2010-05-08", action, parameters).await;
        assert_eq!(
            status, 200,
            "{action} must succeed for {parameters:?}, got {status} {body}"
        );
        body
    }

    /// Make one Amazon S3 REST request, path-style, and answer its status and
    /// body bytes.
    pub async fn s3(&self, method: &str, path: &str, body: Vec<u8>) -> (u16, Vec<u8>) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::from(body))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.body().to_vec())
    }

    /// Create a bucket through Amazon S3's own operation.
    pub async fn create_bucket(&self, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), Vec::new()).await;
        assert_eq!(
            status, 200,
            "CreateBucket {bucket}: got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Stage one object through Amazon S3's own `PutObject`.
    pub async fn put_object(&self, bucket: &str, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), body.as_bytes().to_vec())
            .await;
        assert_eq!(
            status, 200,
            "PutObject {bucket}/{key}: got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// Read one object back through Amazon S3's own `GetObject`.
    pub async fn get_object(&self, bucket: &str, key: &str) -> Option<String> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), Vec::new()).await;
        (status == 200).then(|| String::from_utf8_lossy(&body).into_owned())
    }

    /// Create a provisioned cluster and wait until it is available, so a
    /// statement submitted afterwards runs on an endpoint that exists.
    pub async fn create_cluster(&self, identifier: &str) {
        self.redshift_ok(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("DBName", DATABASE),
            ],
        )
        .await;
        self.await_cluster_available(identifier, "created").await;
    }

    /// Wait until `identifier` reports itself available — the CONDITION, polled
    /// until it holds, never a sleep for a fixed time.
    pub async fn await_cluster_available(&self, identifier: &str, case: &str) {
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let body = self
                .redshift_ok(
                    "DescribeClusters",
                    &[("ClusterIdentifier", identifier)],
                )
                .await;
            if body.contains("<ClusterStatus>available</ClusterStatus>") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: the cluster {identifier} never became available; last answer: \
                 {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Submit one statement against `cluster` and answer the record the service
    /// reports for it, already in its terminal state.
    pub async fn run(&self, cluster: &str, sql: &str) -> Value {
        let submitted = self
            .data_ok(
                "ExecuteStatement",
                &json!({
                    "ClusterIdentifier": cluster,
                    "Database": DATABASE,
                    "DbUser": DB_USER,
                    "Sql": sql,
                    "WaitTimeSeconds": WAIT_SECONDS,
                }),
            )
            .await;
        self.data_ok("DescribeStatement", &json!({"Id": submitted["Id"]}))
            .await
    }

    /// Submit a statement that must FINISH, and answer the record of it.
    pub async fn run_ok(&self, case: &str, cluster: &str, sql: &str) -> Value {
        let described = self.run(cluster, sql).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {case}: {sql} must finish, got {described}"
        );
        described
    }

    /// Submit a statement that must FAIL, and answer the error it reports.
    pub async fn run_failing(&self, case: &str, cluster: &str, sql: &str) -> String {
        let described = self.run(cluster, sql).await;
        assert_eq!(
            described["Status"], "FAILED",
            "case {case}: {sql} must fail, got {described}"
        );
        described["Error"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a failed statement reports why: {described}"))
            .to_string()
    }

    /// Run `sql` and answer the rows it produced, each row a list of the
    /// `Field` documents the Data API reports.
    pub async fn query_rows(&self, case: &str, cluster: &str, sql: &str) -> Vec<Vec<Value>> {
        let described = self.run_ok(case, cluster, sql).await;
        let fetched = self
            .data_ok("GetStatementResult", &json!({"Id": described["Id"]}))
            .await;
        fetched["Records"]
            .as_array()
            .map(|records| {
                records
                    .iter()
                    .map(|record| record.as_array().cloned().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run `sql` and answer the one cell of its one row, as text.
    pub async fn one_value(&self, case: &str, cluster: &str, sql: &str) -> String {
        let rows = self.query_rows(case, cluster, sql).await;
        let cell = rows
            .first()
            .and_then(|row| row.first())
            .cloned()
            .unwrap_or(Value::Null);
        field_text(&cell)
    }
}

/// The text a `Field` document carries, whichever member it arrived in.
pub fn field_text(field: &Value) -> String {
    for member in [
        "stringValue",
        "longValue",
        "doubleValue",
        "booleanValue",
        "blobValue",
    ] {
        if let Some(value) = field.get(member) {
            return match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
        }
    }
    if field.get("isNull").is_some() {
        return String::new();
    }
    field.to_string()
}

/// The SigV4 credential scope every call carries: signatures are not checked
/// here, but the scope is the router's primary routing signal, so it has to
/// name the service the way an SDK would.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIAEMULATORTEST/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=00"
    )
}

/// Percent-encode one form value, escaping everything outside the RFC 3986
/// unreserved characters — the set a form-encoded awsQuery value is written
/// under.
fn form_encode(value: &str) -> String {
    /// Everything that is not an RFC 3986 unreserved character
    /// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`), which is the set a value is
    /// escaped outside of.
    const RESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    ::percent_encoding::utf8_percent_encode(value, RESERVED).to_string()
}

/// A policy document granting `actions` on `resources`.
pub fn allow_policy(actions: &[&str], resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resources}],
    })
    .to_string()
}

/// A policy document denying `actions` on `resources`.
pub fn deny_policy(actions: &[&str], resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Action": actions, "Resource": resources}],
    })
    .to_string()
}

/// The trust policy a Redshift cluster's role carries.
pub fn redshift_trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "redshift.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create a role whose inline policy is `policy`, and answer its ARN.
pub async fn create_role(harness: &Harness, name: &str, policy: Option<&str>) -> String {
    harness
        .iam_ok(
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", &redshift_trust_policy()),
            ],
        )
        .await;
    if let Some(document) = policy {
        harness
            .iam_ok(
                "PutRolePolicy",
                &[
                    ("RoleName", name),
                    ("PolicyName", "data-plane"),
                    ("PolicyDocument", document),
                ],
            )
            .await;
    }
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// A `CREATE TABLE` of this dialect creates the table it declares, whatever
/// STORAGE attributes it carries: how each column is compressed, how the rows
/// are distributed, in which order they are stored and whether a snapshot
/// includes them are directions about physical layout, and the table a client
/// then reads holds what the declaration says it holds.
///
/// The rows differ in WHICH attributes the statement carries and where —
/// per-column, per-table, with and without a parenthesised key list — so the
/// rule is asserted over the class of declarations rather than over one of
/// them.
#[tokio::test]
async fn a_create_table_carrying_storage_attributes_creates_the_table() {
    let harness = Harness::new();
    let cluster = "storage-attributes";
    harness.create_cluster(cluster).await;

    let rows = [
        (
            "a column's compression and the table's distribution key",
            "CREATE TABLE stored_1 (id INTEGER NOT NULL ENCODE az64 DISTKEY, name \
             VARCHAR(16) ENCODE lzo)",
        ),
        (
            "a distribution style and a compound sort key",
            "CREATE TABLE stored_2 (id INTEGER, name VARCHAR(16)) DISTSTYLE KEY DISTKEY (id) \
             COMPOUND SORTKEY (id, name)",
        ),
        (
            "an interleaved sort key",
            "CREATE TABLE stored_3 (id INTEGER, name VARCHAR(16)) INTERLEAVED SORTKEY (id, name)",
        ),
        (
            "a column marked as the sort key, and a table excluded from snapshots",
            "CREATE TABLE stored_4 (id INTEGER SORTKEY, name VARCHAR(16)) BACKUP NO \
             DISTSTYLE EVEN",
        ),
        (
            "the sort key the cluster chooses itself",
            "CREATE TABLE stored_5 (id INTEGER, name VARCHAR(16)) DISTSTYLE AUTO SORTKEY AUTO",
        ),
        (
            "a temporary table carrying attributes of its own",
            "CREATE TEMPORARY TABLE stored_6 (id INTEGER ENCODE RAW) DISTSTYLE ALL",
        ),
    ];

    for (case, sql) in rows {
        harness.run_ok(case, cluster, sql).await;
    }

    // Each table holds what its declaration said it holds, which is what a
    // client writing to it and reading it back sees.
    harness
        .run_ok(
            "the declared columns",
            cluster,
            "INSERT INTO stored_2 (id, name) VALUES (1, 'one')",
        )
        .await;
    let read_back = harness
        .one_value(
            "the declared columns",
            cluster,
            "SELECT name FROM stored_2 WHERE id = 1",
        )
        .await;
    assert_eq!(
        read_back, "one",
        "a table declared with storage attributes holds the rows written to it"
    );
}

/// A statement of this vocabulary that is TRUNCATED, malformed, or names an
/// option this engine does not carry out is not half-read: it fails, and it
/// loads, exports, binds and sets nothing.
///
/// The rows differ in WHERE the statement stops being one — a missing source, an
/// unclosed query, an unbalanced key list, a value that is no delimiter, an
/// option whose work this engine cannot do, a statement longer than the engine
/// reads — so what is asserted is that the reader hands such a text on rather
/// than guessing at what it meant.
#[tokio::test]
async fn a_statement_this_engine_cannot_read_fails_and_does_nothing() {
    let harness = Harness::new();
    let cluster = "malformed-statements";
    harness.create_cluster(cluster).await;
    harness.create_bucket("partial").await;
    harness.put_object("partial", "rows.csv", "1,one\n").await;
    harness
        .run_ok(
            "the table these statements name",
            cluster,
            "CREATE TABLE partial (id INTEGER, name VARCHAR(8))",
        )
        .await;
    let role = format!(
        "arn:aws:iam::{}:role/partial-role",
        engine::emulator_account_id()
    );

    let rows = [
        ("a load that names no source at all", "COPY partial FROM".to_string()),
        (
            "a load whose source is not an object address",
            format!("COPY partial FROM 'not-an-address' IAM_ROLE '{role}' CSV"),
        ),
        (
            "a load naming a compression this engine does not read",
            format!("COPY partial FROM 's3://partial/rows.csv' IAM_ROLE '{role}' CSV GZIP"),
        ),
        (
            "a load whose delimiter is not one character",
            format!(
                "COPY partial FROM 's3://partial/rows.csv' IAM_ROLE '{role}' DELIMITER 'xx'"
            ),
        ),
        (
            "an export whose query is never closed",
            format!("UNLOAD ('SELECT id FROM partial' TO 's3://partial/out/' IAM_ROLE '{role}'"),
        ),
        (
            "an export that names no destination",
            "UNLOAD ('SELECT id FROM partial')".to_string(),
        ),
        (
            "an external schema that names no database",
            format!("CREATE EXTERNAL SCHEMA half FROM DATA CATALOG IAM_ROLE '{role}'"),
        ),
        (
            "an external table that names no location",
            "CREATE EXTERNAL TABLE half.rows (id INT) STORED AS TEXTFILE".to_string(),
        ),
        (
            "a parameter set to nothing",
            "SET query_group TO".to_string(),
        ),
        ("a read of nothing", "SHOW".to_string()),
        (
            "a table whose key list is never closed",
            "CREATE TABLE unbalanced (id INTEGER) SORTKEY (id".to_string(),
        ),
        (
            "a statement longer than the engine reads",
            format!("SELECT '{}'", "x".repeat(2 * 1024 * 1024)),
        ),
    ];

    for (case, sql) in &rows {
        let described = harness.run(cluster, sql).await;
        assert_eq!(
            described["Status"], "FAILED",
            "case {case:?}: a statement this engine cannot read must fail, got {described}"
        );
        assert!(
            described["Error"].as_str().is_some_and(|error| !error.is_empty()),
            "case {case:?}: a failed statement must say why, got {described}"
        );
    }

    // And nothing any of them named happened: no row was loaded, no object was
    // written, no schema was bound and no parameter was set.
    let loaded = harness
        .one_value("nothing was loaded", cluster, "SELECT count(*) FROM partial")
        .await;
    assert_eq!(loaded, "0", "no row may be loaded by a statement that failed");
    assert!(
        harness.get_object("partial", "out/0000_part_00").await.is_none(),
        "no object may be written by an export that failed"
    );
    let label = harness
        .one_value("nothing was set", cluster, "SHOW query_group")
        .await;
    assert_eq!(
        label, "default",
        "no parameter may be set by a statement that failed"
    );
}

/// A `COPY` loads the rows the objects at its source carry, in whichever form
/// the statement names them.
///
/// The rows differ in the FORM of the source — the default pipe-delimited text,/// `CSV` with a quoted field, an explicit `DELIMITER`, a header line skipped
/// with `IGNOREHEADER`, one JSON document per line, several objects under one
/// key prefix, and a `MANIFEST` that lists the objects — so what is asserted is
/// the rule over the class of sources a load reads, not one recorded source.
#[tokio::test]
async fn a_copy_loads_the_rows_the_source_objects_carry() {
    let harness = Harness::new();
    let cluster = "copy-formats";
    harness.create_cluster(cluster).await;
    harness.create_bucket("loads").await;
    let role = create_role(
        &harness,
        "copy-formats-role",
        Some(&allow_policy(
            &["s3:GetObject", "s3:ListBucket"],
            &["arn:aws:s3:::loads", "arn:aws:s3:::loads/*"],
        )),
    )
    .await;

    /// One load: the objects staged for it, the options the statement names and
    /// the rows the table must hold afterwards.
    struct Row {
        case: &'static str,
        objects: &'static [(&'static str, &'static str)],
        source: &'static str,
        options: &'static str,
        expected: &'static [(i64, &'static str)],
    }

    let rows = [
        Row {
            case: "the default pipe-delimited text",
            objects: &[("plain/rows.txt", "1|one\n2|two\n")],
            source: "plain/rows.txt",
            options: "",
            expected: &[(1, "one"), (2, "two")],
        },
        Row {
            case: "CSV, with a quoted field carrying the delimiter",
            objects: &[("csv/rows.csv", "1,\"one,first\"\n2,two\n")],
            source: "csv/rows.csv",
            options: "CSV",
            expected: &[(1, "one,first"), (2, "two")],
        },
        Row {
            case: "an explicit delimiter",
            objects: &[("tab/rows.txt", "1\tone\n2\ttwo\n")],
            source: "tab/rows.txt",
            options: "DELIMITER '\t'",
            expected: &[(1, "one"), (2, "two")],
        },
        Row {
            case: "a header line the load is told to skip",
            objects: &[("header/rows.csv", "id,name\n1,one\n2,two\n")],
            source: "header/rows.csv",
            options: "CSV IGNOREHEADER 1",
            expected: &[(1, "one"), (2, "two")],
        },
        Row {
            case: "one JSON document per line, mapped by column name",
            objects: &[(
                "json/rows.json",
                "{\"name\":\"one\",\"id\":1}\n{\"id\":2,\"name\":\"two\"}\n",
            )],
            source: "json/rows.json",
            options: "JSON 'auto'",
            expected: &[(1, "one"), (2, "two")],
        },
        Row {
            case: "every object under one key prefix",
            objects: &[
                ("prefix/part-1.csv", "1,one\n"),
                ("prefix/part-2.csv", "2,two\n"),
            ],
            source: "prefix/",
            options: "CSV",
            expected: &[(1, "one"), (2, "two")],
        },
        Row {
            case: "a manifest that lists the objects to load",
            objects: &[
                ("manifest/part-1.csv", "1,one\n"),
                ("manifest/part-2.csv", "2,two\n"),
                (
                    "manifest/list.json",
                    "{\"entries\":[{\"url\":\"s3://loads/manifest/part-1.csv\",\"mandatory\":true},\
                     {\"url\":\"s3://loads/manifest/part-2.csv\"}]}",
                ),
            ],
            source: "manifest/list.json",
            options: "CSV MANIFEST",
            expected: &[(1, "one"), (2, "two")],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let table = format!("loaded_{index}");
        for (key, body) in row.objects {
            harness.put_object("loads", key, body).await;
        }
        harness
            .run_ok(
                row.case,
                cluster,
                &format!("CREATE TABLE {table} (id INTEGER, name VARCHAR(32))"),
            )
            .await;
        harness
            .run_ok(
                row.case,
                cluster,
                &format!(
                    "COPY {table} (id, name) FROM 's3://loads/{}' IAM_ROLE '{role}' {}",
                    row.source, row.options
                ),
            )
            .await;
        let loaded = harness
            .query_rows(
                row.case,
                cluster,
                &format!("SELECT id, name FROM {table} ORDER BY id"),
            )
            .await;
        let actual: Vec<(String, String)> = loaded
            .iter()
            .map(|cells| {
                (
                    field_text(cells.first().unwrap_or(&Value::Null)),
                    field_text(cells.get(1).unwrap_or(&Value::Null)),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = row
            .expected
            .iter()
            .map(|(id, name)| (id.to_string(), (*name).to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {:?}: the load must put these rows in the table, got {actual:?}",
            row.case
        );
    }
}

/// A row the target's columns cannot take is REJECTED, recorded in
/// `STL_LOAD_ERRORS` with the published code and reason for what was wrong with
/// it, and — with `MAXERROR` at its default — fails the whole load, which then
/// loads nothing.
///
/// The rows differ in the TYPE that rejected the value and in what was wrong
/// with it, so the rule is asserted over the class of load errors.
#[tokio::test]
async fn a_rejected_row_fails_the_load_and_is_recorded_in_the_load_errors() {
    let harness = Harness::new();
    let cluster = "copy-rejections";
    harness.create_cluster(cluster).await;
    harness.create_bucket("rejects").await;
    let role = create_role(
        &harness,
        "copy-rejections-role",
        Some(&allow_policy(
            &["s3:GetObject", "s3:ListBucket"],
            &["arn:aws:s3:::rejects", "arn:aws:s3:::rejects/*"],
        )),
    )
    .await;

    /// One rejected load: the column type the value went into, the object's
    /// rows, and what the load error must report.
    struct Row {
        case: &'static str,
        declared: &'static str,
        body: &'static str,
        column: &'static str,
        column_type: &'static str,
        value: &'static str,
        code: i64,
        line: i64,
        /// The reason the engine words from the value it stopped on, where it
        /// words one.
        reason: Option<&'static str>,
    }

    let rows = [
        Row {
            case: "a word where an integer column expects digits",
            declared: "id INTEGER, amt INTEGER",
            body: "1,7\n2,seven\n",
            column: "amt",
            column_type: "int4",
            value: "seven",
            code: 1207,
            line: 2,
            reason: Some("Invalid digit, Value 's', Pos 0, Type: Integer"),
        },
        Row {
            case: "a word where a decimal column expects a number",
            declared: "id INTEGER, amt DECIMAL(10,2)",
            body: "1,1.50\n2,not-a-number\n",
            column: "amt",
            column_type: "numeric",
            value: "not-a-number",
            code: 1207,
            line: 2,
            // The value carries an exponent marker, so the parse stops in the
            // exponent rather than on the first character that is not a digit.
            reason: Some("Invalid exponent, Value 'r', Pos 11, Type: Decimal"),
        },
        Row {
            case: "a number a bigint column cannot read, reported for its own width",
            declared: "id INTEGER, amt BIGINT",
            body: "1,7\n2,12x34\n",
            column: "amt",
            column_type: "int8",
            value: "12x34",
            code: 1207,
            line: 2,
            reason: Some("Invalid digit, Value 'x', Pos 2, Type: Long"),
        },
        Row {
            case: "a value a boolean column cannot read",
            declared: "id INTEGER, flag BOOLEAN",
            body: "1,true\n2,perhaps\n",
            column: "flag",
            column_type: "bool",
            value: "perhaps",
            code: 1210,
            line: 2,
            reason: None,
        },
        Row {
            case: "a date that is not one",
            declared: "id INTEGER, day DATE",
            body: "1,2026-01-02\n2,yesterday\n",
            column: "day",
            column_type: "date",
            value: "yesterday",
            code: 1205,
            line: 2,
            reason: None,
        },
        Row {
            case: "a character value longer than the column declares",
            declared: "id INTEGER, name VARCHAR(4)",
            body: "1,abcd\n2,abcdefgh\n",
            column: "name",
            column_type: "varchar",
            value: "abcdefgh",
            code: 1204,
            line: 2,
            reason: None,
        },
        Row {
            case: "a record with fewer fields than the table has columns",
            declared: "id INTEGER, name VARCHAR(8)",
            body: "1,one\n2\n",
            column: "",
            column_type: "",
            value: "2",
            code: 1202,
            line: 2,
            reason: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let table = format!("rejected_{index}");
        let key = format!("rows-{index}.csv");
        harness.put_object("rejects", &key, row.body).await;
        harness
            .run_ok(
                row.case,
                cluster,
                &format!("CREATE TABLE {table} ({})", row.declared),
            )
            .await;
        let error = harness
            .run_failing(
                row.case,
                cluster,
                &format!("COPY {table} FROM 's3://rejects/{key}' IAM_ROLE '{role}' CSV"),
            )
            .await;
        assert_eq!(
            error,
            format!(
                "ERROR: Load into table '{table}' failed.  Check 'stl_load_errors' system table \
                 for details."
            ),
            "case {:?}: a failed load points the client at the load errors",
            row.case
        );
        let count = harness
            .one_value(
                row.case,
                cluster,
                &format!("SELECT count(*) FROM {table}"),
            )
            .await;
        assert_eq!(
            count, "0",
            "case {:?}: a load that failed loads none of its rows",
            row.case
        );
        let recorded = harness
            .query_rows(
                row.case,
                cluster,
                &format!(
                    "SELECT colname, type, raw_field_value, err_code, line_number, \
                     btrim(err_reason) FROM stl_load_errors WHERE filename LIKE '%{key}%' ORDER \
                     BY line_number"
                ),
            )
            .await;
        let first = recorded
            .first()
            .unwrap_or_else(|| panic!("case {:?}: the rejected row must be recorded", row.case));
        let cells: Vec<String> = first.iter().map(field_text).collect();
        assert_eq!(
            cells[..5],
            [
                row.column.to_string(),
                row.column_type.to_string(),
                row.value.to_string(),
                row.code.to_string(),
                row.line.to_string(),
            ],
            "case {:?}: the load error must name the column, its type, the value, the published \
             code and the line",
            row.case
        );
        if let Some(reason) = row.reason {
            assert_eq!(
                cells.get(5).map(String::as_str),
                Some(reason),
                "case {:?}: the load error must carry the engine's own reason",
                row.case
            );
        }
    }
}

/// `MAXERROR` is how many rows a load may reject and still succeed: up to that
/// many are skipped and recorded, and the rows that parsed are loaded.
#[tokio::test]
async fn maxerror_lets_a_load_skip_the_rows_it_rejects() {
    let harness = Harness::new();
    let cluster = "copy-maxerror";
    harness.create_cluster(cluster).await;
    harness.create_bucket("tolerant").await;
    let role = create_role(
        &harness,
        "copy-maxerror-role",
        Some(&allow_policy(
            &["s3:GetObject", "s3:ListBucket"],
            &["arn:aws:s3:::tolerant", "arn:aws:s3:::tolerant/*"],
        )),
    )
    .await;
    for (index, (case, maxerror, loaded)) in [
        ("one rejection allowed", "MAXERROR 1", "2"),
        ("more rejections allowed than there are", "MAXERROR 5", "2"),
    ]
    .into_iter()
    .enumerate()
    {
        let table = format!("tolerant_{index}");
        let key = format!("rows-{index}.csv");
        harness
            .put_object("tolerant", &key, "1,10\n2,ten\n3,30\n")
            .await;
        harness
            .run_ok(
                case,
                cluster,
                &format!("CREATE TABLE {table} (id INTEGER, amt INTEGER)"),
            )
            .await;
        harness
            .run_ok(
                case,
                cluster,
                &format!(
                    "COPY {table} FROM 's3://tolerant/{key}' IAM_ROLE '{role}' CSV {maxerror}"
                ),
            )
            .await;
        let count = harness
            .one_value(case, cluster, &format!("SELECT count(*) FROM {table}"))
            .await;
        assert_eq!(
            count, loaded,
            "case {case:?}: the rows that parsed must be loaded"
        );
        let rejected = harness
            .one_value(
                case,
                cluster,
                &format!(
                    "SELECT count(*) FROM stl_load_errors WHERE raw_field_value = 'ten' AND \
                     filename LIKE '%{key}%'"
                ),
            )
            .await;
        assert_eq!(
            rejected, "1",
            "case {case:?}: the row that was skipped must be recorded"
        );
    }
}

/// What the IAM role a load names may do decides whether the load happens at
/// all: a role whose documents do not allow the read fails with Amazon S3's own
/// refusal — naming the assumed-role session the cluster made the call as — and
/// leaves the table empty.
///
/// The rows differ in what the role's documents say: nothing at all, an explicit
/// deny, a grant, and a role IAM holds no record of (which establishes nothing,
/// so the read is made).
#[tokio::test]
async fn the_role_a_load_names_decides_whether_the_objects_may_be_read() {
    let harness = Harness::new();
    let cluster = "copy-authorization";
    harness.create_cluster(cluster).await;
    harness.create_bucket("guarded").await;
    harness.put_object("guarded", "rows.csv", "1,one\n").await;

    let granted = create_role(
        &harness,
        "granted-role",
        Some(&allow_policy(
            &["s3:GetObject", "s3:ListBucket"],
            &["arn:aws:s3:::guarded", "arn:aws:s3:::guarded/*"],
        )),
    )
    .await;
    let silent = create_role(&harness, "silent-role", None).await;
    let denied = create_role(
        &harness,
        "denied-role",
        Some(&deny_policy(&["s3:GetObject"], &["arn:aws:s3:::guarded/*"])),
    )
    .await;
    let unheld = format!(
        "arn:aws:iam::{}:role/never-created",
        engine::emulator_account_id()
    );

    for (index, (case, role, loads)) in [
        ("a role whose policy grants the read", granted.as_str(), true),
        ("a role carrying no policy at all", silent.as_str(), false),
        ("a role whose policy denies the read", denied.as_str(), false),
        (
            "a role IAM holds no record of, which establishes nothing",
            unheld.as_str(),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let table = format!("guarded_{index}");
        harness
            .run_ok(
                case,
                cluster,
                &format!("CREATE TABLE {table} (id INTEGER, name VARCHAR(8))"),
            )
            .await;
        let sql =
            format!("COPY {table} FROM 's3://guarded/rows.csv' IAM_ROLE '{role}' CSV");
        if loads {
            harness.run_ok(case, cluster, &sql).await;
            let count = harness
                .one_value(case, cluster, &format!("SELECT count(*) FROM {table}"))
                .await;
            assert_eq!(count, "1", "case {case:?}: the load must read the object");
            continue;
        }
        let error = harness.run_failing(case, cluster, &sql).await;
        let session = role.rsplit('/').next().unwrap_or_default();
        assert!(
            error.contains("S3ServiceException")
                && error.contains(&format!(
                    "assumed-role/{session}/RedshiftIamRoleSession"
                ))
                && error.contains("is not authorized to perform: s3:"),
            "case {case:?}: the failure must be S3's own refusal of the cluster's session, got \
             {error:?}"
        );
        let count = harness
            .one_value(case, cluster, &format!("SELECT count(*) FROM {table}"))
            .await;
        assert_eq!(
            count, "0",
            "case {case:?}: a load that was refused leaves the table empty"
        );
    }
}

/// An `UNLOAD` writes the query's rows into Amazon S3 as objects a reader can
/// fetch, and `MANIFEST` writes the document that lists them.
///
/// The rows differ in the options the export names — the delimiter, a header
/// line, quoted fields, a manifest — so what is asserted is that the objects
/// carry the query's rows in the form the statement asked for.
#[tokio::test]
async fn unload_writes_the_rows_as_objects_and_lists_them_in_its_manifest() {
    let harness = Harness::new();
    let cluster = "unload-forms";
    harness.create_cluster(cluster).await;
    harness.create_bucket("exports").await;
    let role = create_role(
        &harness,
        "unload-role",
        Some(&allow_policy(
            &["s3:PutObject", "s3:ListBucket"],
            &["arn:aws:s3:::exports", "arn:aws:s3:::exports/*"],
        )),
    )
    .await;
    harness
        .run_ok("fixture", cluster, "CREATE TABLE exported (id INTEGER, name VARCHAR(16))")
        .await;
    harness
        .run_ok(
            "fixture",
            cluster,
            "INSERT INTO exported VALUES (1, 'one'), (2, 'two')",
        )
        .await;

    /// One export: the options it names and the body its data object must
    /// carry.
    struct Row {
        case: &'static str,
        prefix: &'static str,
        options: &'static str,
        body: &'static str,
        manifest: bool,
    }

    let rows = [
        Row {
            case: "the default pipe delimiter",
            prefix: "plain/",
            options: "",
            body: "1|one\n2|two\n",
            manifest: false,
        },
        Row {
            case: "a named delimiter and a manifest",
            prefix: "piped/",
            options: "MANIFEST DELIMITER ',' ALLOWOVERWRITE",
            body: "1,one\n2,two\n",
            manifest: true,
        },
        Row {
            case: "a header line of column names",
            prefix: "headed/",
            options: "DELIMITER ',' HEADER",
            body: "id,name\n1,one\n2,two\n",
            manifest: false,
        },
        Row {
            case: "every field quoted",
            prefix: "quoted/",
            options: "DELIMITER ',' ADDQUOTES",
            body: "\"1\",\"one\"\n\"2\",\"two\"\n",
            manifest: false,
        },
    ];

    for row in &rows {
        harness
            .run_ok(
                row.case,
                cluster,
                &format!(
                    "UNLOAD ('SELECT id, name FROM exported ORDER BY id') TO \
                     's3://exports/{}' IAM_ROLE '{role}' {}",
                    row.prefix, row.options
                ),
            )
            .await;
        let data = harness
            .get_object("exports", &format!("{}0000_part_00", row.prefix))
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the export must write an object a reader can fetch",
                    row.case
                )
            });
        assert_eq!(
            data, row.body,
            "case {:?}: the written object must carry the query's rows",
            row.case
        );
        let manifest = harness
            .get_object("exports", &format!("{}manifest", row.prefix))
            .await;
        assert_eq!(
            manifest.is_some(),
            row.manifest,
            "case {:?}: a manifest is written exactly when the statement asks for one",
            row.case
        );
        if let Some(manifest) = manifest {
            let document: Value = serde_json::from_str(&manifest).unwrap_or(Value::Null);
            let url = document["entries"][0]["url"].as_str().unwrap_or_default();
            let length = document["entries"][0]["meta"]["content_length"]
                .as_u64()
                .unwrap_or_default();
            assert_eq!(
                url,
                format!("s3://exports/{}0000_part_00", row.prefix),
                "case {:?}: the manifest must name the object that was written: {manifest}",
                row.case
            );
            assert_eq!(
                length,
                row.body.len() as u64,
                "case {:?}: the manifest must report the object's byte count",
                row.case
            );
        }
    }

    // "By default, UNLOAD fails if it finds files that it would possibly
    // overwrite", and `ALLOWOVERWRITE` is what lets it.
    let refused = harness
        .run_failing(
            "a destination that already holds objects",
            cluster,
            "UNLOAD ('SELECT id FROM exported') TO 's3://exports/plain/' IAM_ROLE 'arn:aws:iam::\
             000000000000:role/unload-role'",
        )
        .await;
    assert!(
        refused.contains("not empty") && refused.contains("ALLOWOVERWRITE"),
        "an export over a destination that is not empty must say so, got {refused:?}"
    );
    harness
        .run_ok(
            "the same destination with ALLOWOVERWRITE",
            cluster,
            &format!(
                "UNLOAD ('SELECT id, name FROM exported ORDER BY id') TO 's3://exports/plain/' \
                 IAM_ROLE '{role}' ALLOWOVERWRITE"
            ),
        )
        .await;
}

/// A Spectrum external table serves the rows the objects at its catalog
/// location carry, and loads none of them into the database.
///
/// The rows differ in the SHAPE of the query over the external table — every
/// row, a projection, a filter, an aggregate, a join against a local table — so
/// what is asserted is that the external table is a relation of the database's
/// SQL and not one answer to one query.
#[tokio::test]
async fn a_spectrum_external_table_serves_the_objects_rows_without_loading_them() {
    let harness = Harness::new();
    let cluster = "spectrum";
    harness.create_cluster(cluster).await;
    harness.create_bucket("lake").await;
    harness
        .put_object("lake", "sales/part-1.csv", "1,north,10\n2,south,20\n")
        .await;
    harness
        .put_object("lake", "sales/part-2.csv", "3,north,30\n")
        .await;
    let role = create_role(
        &harness,
        "spectrum-role",
        Some(&allow_policy(
            &["s3:GetObject", "s3:ListBucket"],
            &["arn:aws:s3:::lake", "arn:aws:s3:::lake/*"],
        )),
    )
    .await;

    // The external schema is bound to a database of the Data Catalog, and the
    // statement is asked to create that database: Glue's own read of it is what
    // shows it really exists.
    harness
        .run_ok(
            "an external schema bound to a catalog database",
            cluster,
            &format!(
                "CREATE EXTERNAL SCHEMA spectrum_schema FROM DATA CATALOG DATABASE 'lake_db' \
                 IAM_ROLE '{role}' CREATE EXTERNAL DATABASE IF NOT EXISTS"
            ),
        )
        .await;
    let described = harness
        .glue_ok("GetDatabase", &json!({"Name": "lake_db"}))
        .await;
    assert_eq!(
        described["Database"]["Name"], "lake_db",
        "the statement must create the external database where the catalog holds it: {described}"
    );

    harness
        .run_ok(
            "an external table over the objects in Amazon S3",
            cluster,
            "CREATE EXTERNAL TABLE spectrum_schema.sales (id INT, region VARCHAR(16), amount INT) \
             ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' STORED AS TEXTFILE LOCATION \
             's3://lake/sales/'",
        )
        .await;
    let catalogued = harness
        .glue_ok(
            "GetTable",
            &json!({"DatabaseName": "lake_db", "Name": "sales"}),
        )
        .await;
    assert_eq!(
        catalogued["Table"]["StorageDescriptor"]["Location"], "s3://lake/sales/",
        "the external table's definition must live in the catalog: {catalogued}"
    );

    for (case, sql, expected) in [
        (
            "every row of the external table",
            "SELECT count(*) FROM spectrum_schema.sales",
            "3",
        ),
        (
            "an aggregate over a column the catalog types as a number",
            "SELECT sum(amount) FROM spectrum_schema.sales",
            "60",
        ),
        (
            "a filter on a character column",
            "SELECT count(*) FROM spectrum_schema.sales WHERE region = 'north'",
            "2",
        ),
        (
            "a projection of one row",
            "SELECT region FROM spectrum_schema.sales WHERE id = 2",
            "south",
        ),
        (
            "a join against a table of the database itself",
            "SELECT count(*) FROM spectrum_schema.sales s JOIN local_regions r ON r.name = \
             s.region",
            "2",
        ),
    ] {
        if case.starts_with("a join") {
            harness
                .run_ok(
                    case,
                    cluster,
                    "CREATE TABLE local_regions (name VARCHAR(16))",
                )
                .await;
            harness
                .run_ok(case, cluster, "INSERT INTO local_regions VALUES ('north')")
                .await;
        }
        let answered = harness.one_value(case, cluster, sql).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: an external table answers from the objects at its location"
        );
    }

    // The rows were never loaded: the database's own catalog holds only the
    // tables a caller created in it, so a read of the external table through
    // the database's catalog finds nothing there.
    let (status, body) = harness
        .data(
            "DescribeTable",
            &json!({
                "ClusterIdentifier": cluster,
                "Database": DATABASE,
                "DbUser": DB_USER,
                "Table": "sales",
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "an external table is no table of the database itself, got {status} {body}"
    );

    // And the catalog view reports the external table, as a Spectrum client
    // reads it.
    let listed = harness
        .query_rows(
            "the external catalog view",
            cluster,
            "SELECT schemaname, tablename, location FROM svv_external_tables ORDER BY tablename",
        )
        .await;
    let cells: Vec<Vec<String>> = listed
        .iter()
        .map(|row| row.iter().map(field_text).collect())
        .collect();
    assert_eq!(
        cells,
        vec![vec![
            "spectrum_schema".to_string(),
            "sales".to_string(),
            "s3://lake/sales/".to_string(),
        ]],
        "the external catalog view must report the bound schema's tables"
    );
}
