// SPDX-License-Identifier: CC0-1.0

//! What a statement whose TYPES do not work out does.
//!
//! Two rules, both of them about a value's type rather than about its bytes:
//!
//! * an arithmetic operator is not defined between a NUMBER and CHARACTER data, so a
//!   statement that applies one across those two families FAILS when it is analysed, naming
//!   the operator, the two types and where the statement wrote it — it is never answered by
//!   reading one operand as the other;
//! * a CAST is CHECKED: a value that is not a value of the type the cast names fails the
//!   query, naming the value and that type — never a zero, and never a number truncated to
//!   fit.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! operator, which side each family sits on, the type the cast names, each type's published
//! width and one value past it, and the statements that must still SUCCEED.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    QUERY_STATE_FAILED, QUERY_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The bucket the table below keeps its data in.
const BUCKET: &str = "athena-typed-expressions";

/// Where the queries write their results.
const OUTPUT_LOCATION: &str = "s3://athena-typed-expressions/results/";

/// The database the table below lives in.
const DATABASE: &str = "typed_expressions";

/// The table every statement here reads.
const TABLE: &str = "readings";

/// Its one object: `id,name,amount,digits`.
const DATA: &str = "1,alpha,10.5,42\n2,beta,20.25,7\n";

/// How long a poll waits for a started query to settle. Generous: what is asserted is the
/// state the execution reaches, never how fast the host reached it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long one poll sleeps before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// An arithmetic operator between a number and character data fails the query, in Athena's
/// own words about the two types.
#[tokio::test]
async fn arithmetic_across_a_number_and_character_data_fails_the_query() {
    let router = emulator();
    seed(&router).await;

    // (case, the statement, the reason it must fail with)
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a number column plus a character column",
            format!("SELECT id + name AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:11: '+' cannot be applied to integer, varchar",
        ),
        (
            "the two the other way round",
            format!("SELECT name + id AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:13: '+' cannot be applied to varchar, integer",
        ),
        (
            "a subtraction",
            format!("SELECT id - name AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:11: '-' cannot be applied to integer, varchar",
        ),
        (
            "a multiplication of an approximate number",
            format!("SELECT amount * name AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:15: '*' cannot be applied to double, varchar",
        ),
        (
            "a division",
            format!("SELECT id / name AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:11: '/' cannot be applied to integer, varchar",
        ),
        (
            "a remainder",
            format!("SELECT id % name AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:11: '%' cannot be applied to integer, varchar",
        ),
        (
            "a character column and a number the statement wrote",
            format!("SELECT name + 1 AS bad FROM {TABLE}"),
            "TYPE_MISMATCH: line 1:13: '+' cannot be applied to varchar, integer",
        ),
        (
            "the operator in a predicate rather than in the select list",
            format!("SELECT id FROM {TABLE} WHERE id + name > 2"),
            "TYPE_MISMATCH: line 1:34: '+' cannot be applied to integer, varchar",
        ),
    ];

    // Started first, all of them, so the span a completion is owed after the submit is
    // waited out once rather than once per row.
    let mut started: Vec<(&str, &String, &str, String)> = Vec::new();
    for (case, sql, expected) in &cases {
        started.push((case, sql, expected, start_query(&router, case, sql).await));
    }
    for (case, sql, expected, id) in &started {
        let status = settled_status(&router, case, id).await;
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: {sql} must fail, got {status}"
        );
        assert_eq!(
            status["StateChangeReason"].as_str(),
            Some(*expected),
            "case {case:?}: expected the reason {expected:?}, got {status}"
        );
    }

    // And the statements whose types DO work out are answered: one number family with
    // another, a character operator over character data, and a number with a literal.
    let answered: Vec<(&str, String, &str)> = vec![
        (
            "two numbers of different widths",
            format!("SELECT id + amount AS total FROM {TABLE} WHERE id = 1"),
            "11.5",
        ),
        (
            "a number and a number the statement wrote",
            format!("SELECT id + 1 AS next FROM {TABLE} WHERE id = 1"),
            "2",
        ),
        (
            "character data joined to character data",
            format!("SELECT name || 'x' AS joined FROM {TABLE} WHERE id = 1"),
            "alphax",
        ),
    ];
    let mut computed: Vec<(&str, &String, &str, String)> = Vec::new();
    for (case, sql, expected) in &answered {
        computed.push((case, sql, expected, start_query(&router, case, sql).await));
    }
    for (case, sql, expected, id) in &computed {
        let rows = rows_of(&router, case, id).await;
        assert_eq!(
            rows,
            vec![vec![Some((*expected).to_string())]],
            "case {case:?}: {sql} must answer {expected:?}, got {rows:?}"
        );
    }
}

/// A cast whose value is not a value of the type it names fails the query, naming the value
/// and the type.
#[tokio::test]
async fn a_cast_of_a_value_the_type_does_not_hold_fails_the_query() {
    let router = emulator();
    seed(&router).await;

    // (case, the statement, the reason it must fail with). The rows differ along the type
    // the cast names, whether the value is text that names no number or a number too wide
    // for the type, and where the value comes from.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a character column that names no number, cast to a 32-bit number",
            format!("SELECT CAST(name AS integer) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast 'alpha' to INT",
        ),
        (
            "the same value cast to a 64-bit number",
            format!("SELECT CAST(name AS bigint) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast 'alpha' to BIGINT",
        ),
        (
            "the same value cast to an approximate number",
            format!("SELECT CAST(name AS double) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast 'alpha' to DOUBLE",
        ),
        (
            "the same value cast to a boolean",
            format!("SELECT CAST(name AS boolean) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast 'alpha' to BOOLEAN",
        ),
        (
            "a number written out that the 32-bit type does not hold",
            format!("SELECT CAST('2147483648' AS integer) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast '2147483648' to INT",
        ),
        (
            "a number with a point, cast to a whole number",
            format!("SELECT CAST('1.5' AS integer) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast '1.5' to INT",
        ),
        (
            "a character column the 16-bit type does not hold",
            format!("SELECT CAST('40000' AS smallint) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast '40000' to SMALLINT",
        ),
        (
            // Not text: a value the engine already holds AS a number, which its own cast
            // would narrow into the type instead of refusing.
            "a whole number the 32-bit type does not hold",
            format!("SELECT CAST(9999999999 AS integer) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast '9999999999' to INT",
        ),
        (
            "an approximate number the 8-bit type does not hold",
            format!("SELECT CAST(amount * 100 AS tinyint) AS bad FROM {TABLE} WHERE id = 1"),
            "INVALID_CAST_ARGUMENT: Cannot cast '1050' to TINYINT",
        ),
    ];

    // Started first, all of them; see the suite above.
    let mut started: Vec<(&str, &String, &str, String)> = Vec::new();
    for (case, sql, expected) in &cases {
        started.push((case, sql, expected, start_query(&router, case, sql).await));
    }
    for (case, sql, expected, id) in &started {
        let status = settled_status(&router, case, id).await;
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_FAILED),
            "case {case:?}: {sql} must fail, got {status}"
        );
        assert_eq!(
            status["StateChangeReason"].as_str(),
            Some(*expected),
            "case {case:?}: expected the reason {expected:?}, got {status}"
        );
        // The failure carries the class it belongs to: the caller's own statement is the
        // cause, and the type Athena publishes for a failure the evaluation raises.
        assert_eq!(
            (
                status["AthenaError"]["ErrorCategory"].as_i64(),
                status["AthenaError"]["ErrorType"].as_i64(),
            ),
            (Some(2), Some(1100)),
            "case {case:?}: expected the user category and the evaluation type, got {status}"
        );
    }

    // And a cast the type DOES hold is performed: a column whose text names a number, a
    // value inside the type's width, and a cast to a character type, which holds every
    // value.
    let answered: Vec<(&str, String, &str)> = vec![
        (
            "a character column whose text names a number",
            format!("SELECT CAST(digits AS integer) AS read FROM {TABLE} WHERE id = 1"),
            "42",
        ),
        (
            "the largest value the 32-bit type holds",
            format!("SELECT CAST('2147483647' AS integer) AS read FROM {TABLE} WHERE id = 1"),
            "2147483647",
        ),
        (
            "a number cast to a character type",
            format!("SELECT CAST(id AS varchar) AS read FROM {TABLE} WHERE id = 1"),
            "1",
        ),
        (
            "a word the boolean type holds",
            format!("SELECT CAST('true' AS boolean) AS read FROM {TABLE} WHERE id = 1"),
            "true",
        ),
    ];
    let mut computed: Vec<(&str, &String, &str, String)> = Vec::new();
    for (case, sql, expected) in &answered {
        computed.push((case, sql, expected, start_query(&router, case, sql).await));
    }
    for (case, sql, expected, id) in &computed {
        let rows = rows_of(&router, case, id).await;
        assert_eq!(
            rows,
            vec![vec![Some((*expected).to_string())]],
            "case {case:?}: {sql} must answer {expected:?}, got {rows:?}"
        );
    }
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

/// The bucket, the object and the catalog table every statement here reads — each created
/// over the wire of the service that owns it.
async fn seed(router: &MultiServiceRouter) {
    s3_put(router, &format!("/{BUCKET}"), "").await;
    s3_put(router, &format!("/{BUCKET}/{TABLE}/rows.csv"), DATA).await;
    for sql in [
        format!("CREATE DATABASE {DATABASE}"),
        format!(
            "CREATE EXTERNAL TABLE {DATABASE}.{TABLE} \
             (id int, name string, amount double, digits string) ROW FORMAT DELIMITED \
             FIELDS TERMINATED BY ',' LOCATION 's3://{BUCKET}/{TABLE}/'"
        ),
    ] {
        let status = settled_status_of(router, "setup", &sql).await;
        assert_eq!(
            status["State"].as_str(),
            Some(QUERY_STATE_SUCCEEDED),
            "setup: {sql} must succeed, got {status}"
        );
    }
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

/// One Athena call that must succeed.
async fn athena_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, "athena", "AmazonAthena", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Athena {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// Write one object at `path`, which for a bare bucket name creates the bucket.
async fn s3_put(router: &MultiServiceRouter, path: &str, body: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        response.status().is_success(),
        "writing s3://{path} must succeed, got {}",
        response.status()
    );
}

/// Start `sql` and answer the `Status` it settles in, polled until it is terminal — the
/// condition is what is waited on, never a fixed time.
/// Start `sql` and answer the `QueryExecutionId` it minted.
///
/// A suite STARTS every statement it is going to assert on before it waits on any of them:
/// a completion is owed a span after the submit, so waiting per statement would pay that
/// span once per row of the table.
async fn start_query(router: &MultiServiceRouter, case: &str, sql: &str) -> String {
    let answer = athena_ok(
        router,
        case,
        "StartQueryExecution",
        &json!({
            "QueryString": sql,
            "QueryExecutionContext": { "Database": DATABASE },
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a query execution id, got {answer}"))
        .to_string()
}

/// The `Status` the execution `id` settles in, polled until it is terminal — the condition
/// is what is waited on, never a fixed time.
async fn settled_status(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let started = Instant::now();
    loop {
        let body = athena_ok(
            router,
            case,
            "GetQueryExecution",
            &json!({ "QueryExecutionId": id }),
        )
        .await;
        let reported = body["QueryExecution"]["Status"].clone();
        let state = reported["State"].as_str().unwrap_or_default().to_string();
        if state == QUERY_STATE_SUCCEEDED || state == QUERY_STATE_FAILED {
            let mut reported = reported;
            reported["QueryExecutionId"] = json!(id);
            return reported;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: the execution never reached a terminal state, last saw {reported}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Start `sql` and answer the `Status` it settles in, for a statement nothing else waits
/// beside.
async fn settled_status_of(router: &MultiServiceRouter, case: &str, sql: &str) -> Value {
    let id = start_query(router, case, sql).await;
    settled_status(router, case, &id).await
}

/// Require the execution `id` SUCCEEDED, and answer its data rows — the label row a DML
/// result set opens with dropped, and `None` for a datum carrying no value.
async fn rows_of(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
) -> Vec<Vec<Option<String>>> {
    let status = settled_status(router, case, id).await;
    assert_eq!(
        status["State"].as_str(),
        Some(QUERY_STATE_SUCCEEDED),
        "case {case:?}: the statement must succeed, got {status}"
    );
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
                .map(|datum| datum["VarCharValue"].as_str().map(str::to_string))
                .collect()
        })
        .collect()
}
