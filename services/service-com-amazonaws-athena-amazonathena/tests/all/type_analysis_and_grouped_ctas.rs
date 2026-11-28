// SPDX-License-Identifier: CC0-1.0

//! What Athena's query ANALYSIS refuses before a row is read, and what its
//! `CREATE TABLE … WITH (…) AS SELECT … GROUP BY … HAVING …` materialises.
//!
//! Two rules, both reached through `StartQueryExecution`/`GetQueryExecution` over an external
//! table whose data is a delimited object in the object store:
//!
//! * an operator the engine has no signature for its operand types ABORTS the execution: it
//!   settles in the terminal failed state carrying the failure envelope — the reason in the
//!   engine's own wording for that condition (the quoted operator, the clause, and the
//!   operand types in the order they were written), the user category, no retry and an
//!   integer error type — and is never answered by reading one operand as the other;
//! * a table defined from a GROUPED query runs that query like any other read and then
//!   materialises its rows: the destination is registered in the data catalog over the
//!   location the statement's properties name, the objects land there, and a later read of
//!   the destination answers exactly the groups the `HAVING` kept, with the aggregate each
//!   group computes.
//!
//! Each suite asserts its rule over a TABLE of inputs rather than one statement: the
//! analysis rows differ along the operator, which side each type family sits on, whether an
//! operand is a column or a literal the statement wrote, where in the statement the operator
//! sits and which line it sits on; the materialising rows differ along the aggregate, the
//! file format written, how the source relation is named, and how many groups the `HAVING`
//! keeps. Every aggregate asserted is computed here from the seeded rows, so what the
//! destination answers is checked against an independent count rather than against itself.
//!
//! Athena, Glue and Amazon S3 are mounted on ONE shared engine and driven over the wire the
//! production front door uses, so what is asserted is what an SDK client observes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::catalog_sql::{
    ERROR_TYPE_ENTITY_NOT_FOUND, ERROR_TYPE_GENERIC_USER,
};
use service_com_amazonaws_athena_amazonathena::state::{
    QUERY_EXECUTION_COMPLETION, QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The bucket every table and every result object here lives in.
const BUCKET: &str = "athena-type-analysis";

/// Where every query writes its results.
const RESULTS: &str = "s3://athena-type-analysis/results/";

/// The database every relation here lives in.
const DATABASE: &str = "analysis_lake";

/// The table the analysis suites read: `id int, name string, amount double, qty bigint`.
const READINGS: &str = "readings";

/// Its one object, as the delimited text it is stored as.
const READINGS_CSV: &str = "1,alpha,10.5,42\n2,beta,20.25,7\n";

/// The table the materialising suite reads: `id int, category string, amount bigint`.
const SOURCE: &str = "src";

/// The rows [`SOURCE`] holds — three categories, one of them carrying two rows — which are
/// also the oracle every aggregate below is computed from: `(id, category, amount)`.
const SOURCE_ROWS: &[(i64, &str, i64)] = &[
    (1, "north", 10),
    (2, "south", 5),
    (3, "north", 7),
    (4, "east", 20),
];

/// The `AthenaError$ErrorCategory` of a failure the caller's own statement is the cause of,
/// per the member's documented list (1 - System, 2 - User, 3 - Other).
const CATEGORY_USER: i64 = 2;

/// The least value `AthenaError$ErrorType` is modelled with (`@range` 0-9999).
const ERROR_TYPE_MIN: i64 = 0;

/// The greatest value that member is modelled with; see [`ERROR_TYPE_MIN`].
const ERROR_TYPE_MAX: i64 = 9999;

/// How long past [`QUERY_EXECUTION_COMPLETION`] a poll waits for a started query to be
/// readable in its terminal state. Generous: what is asserted is the state the execution
/// reaches, never how fast the host reached it.
const SETTLE_GRACE: Duration = Duration::from_secs(60);

/// How long one poll sleeps before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// RULE: an arithmetic operator the engine has no signature for its operand types fails the
/// execution when the statement is analysed, in the engine's own wording for that condition
/// — the quoted operator, `cannot be applied to`, and the operand types in the order they
/// were written, located at the line and column the statement wrote the operator at.
#[tokio::test]
async fn an_operator_with_no_signature_for_its_operands_fails_in_athenas_wording() {
    let router = emulator();
    seed(&router).await;

    // (case, the statement, the operator, the type of its left operand, of its right)
    let cases: Vec<(&str, String, &str, &str, &str)> = vec![
        (
            "a 32-bit number column and a character column",
            format!("SELECT id + name AS bad FROM \"{DATABASE}\".\"{READINGS}\""),
            "+",
            "integer",
            "varchar",
        ),
        (
            "the two families the other way round, subtracted",
            format!("SELECT name - id AS bad FROM \"{DATABASE}\".\"{READINGS}\""),
            "-",
            "varchar",
            "integer",
        ),
        (
            "an approximate number multiplied by character data",
            format!("SELECT amount * name AS bad FROM {READINGS}"),
            "*",
            "double",
            "varchar",
        ),
        (
            "a 64-bit number divided by character data",
            format!("SELECT qty / name AS bad FROM {READINGS}"),
            "/",
            "bigint",
            "varchar",
        ),
        (
            "the remainder operator",
            format!("SELECT id % name AS bad FROM {READINGS}"),
            "%",
            "integer",
            "varchar",
        ),
        (
            "a number the statement wrote and a character column",
            format!("SELECT 1 + name AS bad FROM {READINGS}"),
            "+",
            "integer",
            "varchar",
        ),
        (
            // A character literal is typed by its own length, which is what the operand type
            // then names.
            "a character literal and a 64-bit number column",
            format!("SELECT 'abc' + qty AS bad FROM {READINGS}"),
            "+",
            "varchar(3)",
            "bigint",
        ),
        (
            "the operator in a predicate rather than the projection",
            format!("SELECT id FROM {READINGS} WHERE amount - name > 1"),
            "-",
            "double",
            "varchar",
        ),
        (
            "the operator inside a function argument",
            format!("SELECT max(id + name) AS bad FROM {READINGS}"),
            "+",
            "integer",
            "varchar",
        ),
        (
            "the operator on the second line of the statement",
            format!("SELECT id,\n       id + name AS bad FROM {READINGS}"),
            "+",
            "integer",
            "varchar",
        ),
    ];

    // Every statement is started before any of them is read back, so the span a completion is
    // owed after the submit elapses once for the whole table instead of once per row.
    let mut started: Vec<(&str, &String, String, String)> = Vec::new();
    for (case, sql, operator, left, right) in &cases {
        let expected = type_mismatch_reason(sql, operator, left, right);
        started.push((case, sql, expected, start(&router, case, sql).await));
    }

    for (case, sql, expected, id) in &started {
        let status = settled(&router, case, id).await["Status"].clone();
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: {sql} must reach {QUERY_STATE_FAILED}, got {status}"
        );
        assert_eq!(
            status["StateChangeReason"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the reason {expected:?}, got {status}"
        );
        assert_failure_envelope(case, &status, expected, ERROR_TYPE_GENERIC_USER);
    }

    // And a statement whose operand types DO have such an operator is answered: two number
    // families together, and a character operator over character data.
    let answered: Vec<(&str, String, &str)> = vec![
        (
            "a 32-bit and a 64-bit number",
            format!("SELECT id + qty AS total FROM {READINGS} WHERE id = 1"),
            "43",
        ),
        (
            "a whole and an approximate number",
            format!("SELECT id + amount AS total FROM {READINGS} WHERE id = 1"),
            "11.5",
        ),
        (
            "character data joined to character data",
            format!("SELECT name || 'x' AS joined FROM {READINGS} WHERE id = 1"),
            "alphax",
        ),
    ];
    let mut computed: Vec<(&str, &String, &str, String)> = Vec::new();
    for (case, sql, expected) in &answered {
        computed.push((case, sql, expected, start(&router, case, sql).await));
    }
    for (case, sql, expected, id) in &computed {
        let status = settled(&router, case, id).await["Status"].clone();
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {case:?}: {sql} must reach {QUERY_STATE_SUCCEEDED}, got {status}"
        );
        let rows = rows_of(&router, case, id).await;
        assert_eq!(
            rows,
            vec![vec![Some((*expected).to_string())]],
            "case {case:?}: {sql} must answer {expected:?}, got {rows:?}"
        );
    }
}

/// RULE: each class of defect a statement can carry is reported as its OWN class — the class
/// name the message opens with — so a client can tell a name it got wrong from a type it got
/// wrong, each beside the same failure envelope.
#[tokio::test]
async fn each_class_of_statement_defect_reports_its_own_class() {
    let router = emulator();
    seed(&router).await;

    // (case, the statement, the class its message must open with, what it must name, the
    // error type Athena publishes for that class)
    let cases: Vec<(&str, String, &str, String, i32)> = vec![
        (
            "a type no such operator is defined between",
            format!("SELECT id + name AS bad FROM \"{DATABASE}\".\"{READINGS}\""),
            "TYPE_MISMATCH",
            "'+'".to_string(),
            ERROR_TYPE_GENERIC_USER,
        ),
        (
            "a relation the catalog does not hold",
            format!("SELECT id FROM \"{DATABASE}\".\"no_such_table\""),
            "TABLE_NOT_FOUND",
            "no_such_table".to_string(),
            ERROR_TYPE_ENTITY_NOT_FOUND,
        ),
        (
            "a column no relation of the statement carries",
            format!("SELECT no_such_column FROM \"{DATABASE}\".\"{READINGS}\""),
            "COLUMN_NOT_FOUND",
            "no_such_column".to_string(),
            ERROR_TYPE_GENERIC_USER,
        ),
    ];

    let mut started: Vec<(&str, &str, &String, i32, String)> = Vec::new();
    for (case, sql, class, named, error_type) in &cases {
        started.push((case, class, named, *error_type, start(&router, case, sql).await));
    }

    let mut classes: Vec<String> = Vec::new();
    for (case, class, named, error_type, id) in &started {
        let status = settled(&router, case, id).await["Status"].clone();
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: the statement must reach {QUERY_STATE_FAILED}, got {status}"
        );
        let reason = status["StateChangeReason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.starts_with(&format!("{class}: ")),
            "case {case:?}: expected a message of the {class} class, got {reason:?}"
        );
        assert!(
            reason.contains(named.as_str()),
            "case {case:?}: expected the message to name {named}, got {reason:?}"
        );
        assert_failure_envelope(case, &status, &reason, *error_type);
        classes.push((*class).to_string());
    }
    let distinct: ::std::collections::BTreeSet<&String> = classes.iter().collect();
    assert_eq!(
        distinct.len(),
        classes.len(),
        "the classes must be distinguishable from one another, got {classes:?}"
    );
}

/// One row of the materialising suite: the statement it writes, and what it must leave.
struct Grouped {
    /// What the row is, which is also the destination table and the prefix its data lands
    /// under.
    case: &'static str,
    /// The file format its `format` property declares.
    format: &'static str,
    /// The aggregate the query computes per group, as the statement writes it.
    aggregate: &'static str,
    /// The same aggregate over one group's amounts, computed here.
    oracle: fn(&[i64]) -> i64,
    /// The bound its `HAVING` keeps a group above, or `None` for a statement with no
    /// `HAVING` at all.
    having: Option<i64>,
    /// How the SOURCE relation is written: a two-part quoted name, or the bare name the
    /// submission's own database resolves.
    relation: Relation,
}

/// How a statement writes the relation it reads.
#[derive(Clone, Copy)]
enum Relation {
    /// `"<database>"."<table>"` — two quoted parts, which name the table of that database.
    Qualified,
    /// `<table>` — resolved against the database the submission names.
    Bare,
}

/// RULE: `CREATE TABLE … WITH (…) AS SELECT … GROUP BY … HAVING …` runs its query and
/// materialises the rows it produced — registering the destination in the data catalog over
/// the location its `external_location` property names and writing its objects there — so a
/// later read of the destination answers exactly the groups the `HAVING` kept, each with the
/// aggregate its own rows compute.
#[tokio::test]
async fn a_grouped_table_defined_from_a_query_is_materialised_and_read_back() {
    let router = emulator();
    seed(&router).await;

    let cases: Vec<Grouped> = vec![
        Grouped {
            case: "sum_above_a_bound",
            format: "TEXTFILE",
            aggregate: "sum(amount)",
            oracle: |amounts| amounts.iter().sum(),
            having: Some(10),
            relation: Relation::Qualified,
        },
        Grouped {
            case: "count_above_a_bound",
            format: "PARQUET",
            aggregate: "count(amount)",
            oracle: |amounts| i64::try_from(amounts.len()).unwrap_or(i64::MAX),
            having: Some(1),
            relation: Relation::Qualified,
        },
        Grouped {
            case: "min_of_every_group",
            format: "TEXTFILE",
            aggregate: "min(amount)",
            oracle: |amounts| amounts.iter().copied().min().unwrap_or_default(),
            having: None,
            relation: Relation::Bare,
        },
        Grouped {
            case: "max_above_a_bound_every_group_clears",
            format: "JSON",
            aggregate: "max(amount)",
            oracle: |amounts| amounts.iter().copied().max().unwrap_or_default(),
            having: Some(0),
            relation: Relation::Qualified,
        },
    ];

    // Every destination is written before any is read back; see the suite above.
    let mut written: Vec<(&Grouped, String)> = Vec::new();
    for case in &cases {
        let sql = materialising_statement(case);
        written.push((case, start(&router, case.case, &sql).await));
    }

    for (case, id) in &written {
        let execution = settled(&router, case.case, id).await;
        assert_eq!(
            execution["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {:?}: {} must reach {QUERY_STATE_SUCCEEDED}, got {}",
            case.case,
            materialising_statement(case),
            execution["Status"]
        );

        // The destination is registered over the location the statement named, and its
        // objects are there.
        let location = format!("s3://{BUCKET}/{}/", case.case);
        let registered = glue_table(&router, case.case).await;
        // Compared without the trailing separator: a prefix denotes the same folder whether
        // or not the catalog entry records the separator it was written with.
        let recorded = registered["Table"]["StorageDescriptor"]["Location"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        assert_eq!(
            recorded,
            location.trim_end_matches('/'),
            "case {:?}: expected the table registered over {location}, got {registered}",
            case.case
        );
        let keys = listed_keys(&router, case.case, &format!("{}/", case.case)).await;
        assert!(
            !keys.is_empty(),
            "case {:?}: the rows must be written under {location}",
            case.case
        );
    }

    // What the destination answers, against the groups computed here from the seeded rows.
    let mut reads: Vec<(&Grouped, String)> = Vec::new();
    for (case, _) in &written {
        let sql = format!(
            "SELECT category, total FROM \"{DATABASE}\".\"{}\" ORDER BY category",
            case.case
        );
        reads.push((case, start(&router, case.case, &sql).await));
    }

    for (case, id) in &reads {
        let execution = settled(&router, case.case, id).await;
        assert_eq!(
            execution["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "case {:?}: reading the destination must succeed, got {}",
            case.case,
            execution["Status"]
        );
        let expected = expected_groups(case);
        let answered = rows_of(&router, case.case, id).await;
        assert_eq!(
            answered, expected,
            "case {:?}: expected the groups {expected:?}, got {answered:?}",
            case.case
        );
    }
}

/// The statement one materialising row submits.
fn materialising_statement(case: &Grouped) -> String {
    let relation = match case.relation {
        Relation::Qualified => format!("\"{DATABASE}\".\"{SOURCE}\""),
        Relation::Bare => SOURCE.to_string(),
    };
    let having = case
        .having
        .map(|bound| format!(" HAVING {} > {bound}", case.aggregate))
        .unwrap_or_default();
    format!(
        "CREATE TABLE \"{DATABASE}\".\"{}\" WITH (format = '{}', \
         external_location = 's3://{BUCKET}/{}/') AS SELECT category, {} AS total FROM \
         {relation} GROUP BY category{having}",
        case.case, case.format, case.case, case.aggregate
    )
}

/// The groups one materialising row must answer with, computed here from [`SOURCE_ROWS`] and
/// ordered by category the way the read orders them.
fn expected_groups(case: &Grouped) -> Vec<Vec<Option<String>>> {
    let mut grouped: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (_, category, amount) in SOURCE_ROWS {
        grouped.entry(category).or_default().push(*amount);
    }
    grouped
        .iter()
        .map(|(category, amounts)| (*category, (case.oracle)(amounts)))
        .filter(|(_, computed)| case.having.is_none_or(|bound| *computed > bound))
        .map(|(category, computed)| vec![Some(category.to_string()), Some(computed.to_string())])
        .collect()
}

/// The reason a statement whose operand types have no such operator must carry.
///
/// The operator is located INDEPENDENTLY of the service: the line and column are counted off
/// the submitted text itself, which is where the statement wrote the operator.
fn type_mismatch_reason(sql: &str, operator: &str, left: &str, right: &str) -> String {
    let (line, column) = located(sql, operator);
    format!("TYPE_MISMATCH: line {line}:{column}: '{operator}' cannot be applied to {left}, {right}")
}

/// Where `operator` is written in `sql`, as a one-based line and column.
///
/// The operator is located as a SPACE-DELIMITED token, which is how every statement below
/// writes it: a character of a literal or of a name — the `/` of an object address, the `-`
/// of a date, the `%` of a pattern — is not one, so the locator stays the operator's own
/// however a row is written.
fn located(sql: &str, operator: &str) -> (usize, usize) {
    let token = format!(" {operator} ");
    let at = sql
        .find(token.as_str())
        .unwrap_or_else(|| panic!("the statement {sql:?} must write the operator {operator:?}"))
        // The operator itself is the character after the space the token opens with.
        + 1;
    let (mut line, mut column) = (1usize, 1usize);
    for (index, character) in sql.char_indices() {
        if index == at {
            return (line, column);
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    panic!("the statement {sql:?} must write the operator {operator:?}");
}

/// Assert the envelope every failed execution carries: the caller's own statement is the
/// cause, the failure is terminal rather than worth retrying, the error type Athena
/// publishes for that failure's own class — an integer inside the modelled range — and the
/// error message is the reason itself.
fn assert_failure_envelope(case: &str, status: &Value, reason: &str, published: i32) {
    let error = status["AthenaError"].clone();
    assert_eq!(
        error["ErrorCategory"].as_i64(),
        Some(CATEGORY_USER),
        "case {case:?}: expected the user category, got {error}"
    );
    assert_eq!(
        error["Retryable"].as_bool(),
        Some(false),
        "case {case:?}: a failed execution is terminal, so it is not retryable, got {error}"
    );
    assert!(
        (ERROR_TYPE_MIN..=ERROR_TYPE_MAX).contains(&i64::from(published)),
        "case {case:?}: the type published for this class must be inside the modelled \
         range {ERROR_TYPE_MIN}..={ERROR_TYPE_MAX}, got {published}"
    );
    assert_eq!(
        error["ErrorType"].as_i64(),
        Some(i64::from(published)),
        "case {case:?}: expected the error type {published} this class publishes, got {error}"
    );
    assert_eq!(
        error["ErrorMessage"].as_str(),
        Some(reason),
        "case {case:?}: expected the published message {reason:?}, got {error}"
    );
}

/// Athena, Glue and Amazon S3 mounted on ONE shared engine, the way the router mounts every
/// service: the catalog Glue holds is the catalog Athena writes, and the objects S3 holds are
/// the data Athena writes and reads.
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

/// The bucket, the objects and the catalog the suites start from — each created the way a
/// client creates it: the objects over Amazon S3's wire, the database and the tables through
/// Athena's own DDL.
async fn seed(router: &MultiServiceRouter) {
    let (status, answer) = s3(router, "PUT", &format!("/{BUCKET}"), String::new()).await;
    assert!(
        (200..300).contains(&status),
        "setup: creating s3://{BUCKET} must succeed, got {status} with {answer}"
    );
    put_object(router, &format!("{READINGS}/rows.csv"), READINGS_CSV.to_string()).await;
    put_object(router, &format!("{SOURCE}/rows.csv"), source_csv()).await;

    for sql in [
        format!("CREATE DATABASE {DATABASE}"),
        format!(
            "CREATE EXTERNAL TABLE `{DATABASE}`.`{READINGS}` \
             (`id` int, `name` string, `amount` double, `qty` bigint) ROW FORMAT DELIMITED \
             FIELDS TERMINATED BY ',' STORED AS TEXTFILE LOCATION 's3://{BUCKET}/{READINGS}/'"
        ),
        format!(
            "CREATE EXTERNAL TABLE `{DATABASE}`.`{SOURCE}` \
             (`id` int, `category` string, `amount` bigint) ROW FORMAT DELIMITED \
             FIELDS TERMINATED BY ',' STORED AS TEXTFILE LOCATION 's3://{BUCKET}/{SOURCE}/'"
        ),
    ] {
        let id = start(router, "setup", &sql).await;
        let execution = settled(router, "setup", &id).await;
        assert_eq!(
            execution["Status"]["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "setup: {sql} must succeed, got {}",
            execution["Status"]
        );
    }
}

/// [`SOURCE_ROWS`] as the delimited text the table's object holds.
fn source_csv() -> String {
    SOURCE_ROWS
        .iter()
        .map(|(id, category, amount)| format!("{id},{category},{amount}\n"))
        .collect()
}

/// A SigV4 `Authorization` header naming `service` in its credential scope — the routing key
/// the router claims a request by. Nothing is verified: this harness turns request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=not-verified"
    )
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
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
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

/// One Athena call that must answer 200.
async fn athena_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, "athena", "AmazonAthena", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Athena {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// The Glue record of the table `name` in [`DATABASE`], which must be there.
async fn glue_table(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, answer) = call(
        router,
        "glue",
        "AWSGlue",
        "GetTable",
        &json!({ "DatabaseName": DATABASE, "Name": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the catalog must hold the table, got {status} with {answer}"
    );
    answer
}

/// One path-style request to the emulator's Amazon S3 front door, as text.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Write one object into [`BUCKET`].
async fn put_object(router: &MultiServiceRouter, key: &str, body: String) {
    let (status, answer) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        (200..300).contains(&status),
        "setup: writing s3://{BUCKET}/{key} must succeed, got {status} with {answer}"
    );
}

/// The keys `ListObjectsV2` lists under `prefix`, sorted.
async fn listed_keys(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let (status, body) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        String::new(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: listing s3://{BUCKET}/{prefix} must answer 200, got {status} with {body}"
    );
    let mut keys: Vec<String> = body
        .split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split_once("</Key>"))
        .map(|(key, _)| key.to_string())
        .collect();
    keys.sort();
    keys
}

/// Start `sql` and answer the `QueryExecutionId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, sql: &str) -> String {
    let started = athena_ok(
        router,
        case,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": DATABASE, "Catalog": "awsdatacatalog" },
            "ResultConfiguration": { "OutputLocation": RESULTS },
        }),
    )
    .await;
    started["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an execution id, got {started}"))
        .to_string()
}

/// Poll `id` until it is terminal and answer its `QueryExecution`.
///
/// The CONDITION is what is waited on: every look asks the service which state the execution
/// is in, and the deadline is far above the span a completion is due after.
async fn settled(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let attempts = ((QUERY_EXECUTION_COMPLETION + SETTLE_GRACE).as_millis()
        / POLL_INTERVAL.as_millis())
        + 2;
    for _ in 0..attempts {
        let body = athena_ok(
            router,
            case,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        let execution = body["QueryExecution"].clone();
        let state = execution["Status"]["State"].as_str().unwrap_or_default();
        if state == QUERY_STATE_SUCCEEDED || state == QUERY_STATE_FAILED {
            return execution;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the execution {id} never reached a terminal state");
}

/// The data rows `id` answers with, each cell `None` for a NULL — the label row a result set
/// opens with dropped.
async fn rows_of(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<Vec<Option<String>>> {
    let body = athena_ok(
        router,
        case,
        "GetQueryResults",
        &json!({ "QueryExecutionId": id, "MaxResults": 100 }),
    )
    .await;
    body["ResultSet"]["Rows"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .skip(1)
        .map(|row| {
            row["Data"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|datum| {
                    datum["VarCharValue"]
                        .as_str()
                        .map(::std::string::ToString::to_string)
                })
                .collect()
        })
        .collect()
}
