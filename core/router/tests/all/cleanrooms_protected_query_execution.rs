// SPDX-License-Identifier: CC0-1.0

//! A Clean Rooms protected query RUNS over the data behind the configured tables its
//! collaboration associates, and the rows it computes land in the object store — driven
//! across the services the question spans, mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! RULE: a protected query a membership submits advances from SUBMITTED to the outcome of
//! its own run — SUCCESS for a statement the engine answers, carrying
//! `statistics.totalDurationInMillis` and `result.output.s3.location`, the address under
//! the effective result bucket and key prefix that the result object was written at. The
//! rows it answers are the rows the Data Catalog tables behind its relations really hold,
//! read through the columns their analysis rules expose, so a statement that reads a
//! column no rule allows is refused rather than answered.
//!
//! Three crates have to agree for that to hold (Clean Rooms runs the statement, Glue holds
//! the catalog the relations resolve in, and Amazon S3 holds both the data under those
//! tables and the result object), so the suite lives here rather than in any of them.
//!
//! Nothing here asserts a value this emulator invented: the result location is rebuilt
//! from the ids the service minted and the configuration the calls carried, and the result
//! object is read back through Amazon S3's own `ListObjectsV2` and `GetObject` — so a
//! location Clean Rooms reports for an object the object store does not serve fails the
//! suite.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call here is signed for, which is also the routing key. Outside
/// `us-east-1`, so a bucket is created with the location constraint such a bucket carries.
const REGION: &str = "us-west-2";

/// The bucket the queried data lives in, and the one the results are written to: Clean
/// Rooms refuses a result location in the bucket holding the data it reads, so they are
/// two buckets here as well.
pub(crate) const DATA_BUCKET: &str = "cleanrooms-run-data";
pub(crate) const RESULTS_BUCKET: &str = "cleanrooms-run-results";

/// The Data Catalog database the two queried tables live in.
pub(crate) const DATABASE: &str = "cleanrooms_run_db";

/// The prefix the membership's default result configuration names, which the effective
/// prefix is the collaboration partition of.
pub(crate) const RESULT_PREFIX: &str = "results/";

/// The role a result configuration and an association name. Nothing here reads it: the
/// members are modelled as ARNs and echoed.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/clean-rooms";

/// The SerDe the two tables are registered with, and the separator its parameters name.
const LAZY_SIMPLE_SERDE: &str = "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe";

/// The XML namespace an Amazon S3 document carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// How long a poll waits for the run a query is owed.
///
/// Generous rather than tight: the completion is due a span after the start call, so this
/// deadline is only the point at which the run is declared never to have happened — a
/// loaded host must not be the thing that trips it.
pub(crate) const PATIENCE: Duration = Duration::from_secs(120);

/// How often that poll asks again.
pub(crate) const POLL: Duration = Duration::from_millis(25);

/// The `ProtectedQueryStatus` values a query is still being processed in.
pub(crate) const PENDING_STATUSES: [&str; 3] = ["SUBMITTED", "STARTED", "CANCELLING"];

/// Clean Rooms, Glue and Amazon S3 over ONE shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. Nothing is verified: this router runs with
/// request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One raw request to the emulator's Amazon S3 front door: path-style, so the bucket and
/// the key live in the URI and the body is the payload.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().to_vec())
}

/// One Glue call (`awsJson1_1` under the target prefix `AWSGlue`), requiring a 200.
pub(crate) async fn glue(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("glue.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization("glue"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: Glue {operation} must succeed, got {status} with {parsed}"
    );
    parsed
}

/// One Clean Rooms call (restJson1, URI-routed), as its status and body.
async fn cleanrooms_try(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(|body| body.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("cleanrooms.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("cleanrooms"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, requiring a 200.
pub(crate) async fn cleanrooms(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Value {
    let (status, parsed) = cleanrooms_try(router, method, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Clean Rooms {method} {path} must succeed, got {status} with {parsed}"
    );
    parsed
}

/// Create a bucket and put `body` at `key` in it.
pub(crate) async fn given_object(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
    body: &str,
) {
    let (status, answered) = s3(router, "PUT", &format!("/{bucket}/{key}"), body.as_bytes()).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the write of {key:?} must be accepted, got {status} with {}",
        String::from_utf8_lossy(&answered)
    );
}

/// Create `bucket`.
pub(crate) async fn given_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, answered) = s3(
        router,
        "PUT",
        &format!("/{bucket}"),
        configuration.as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the bucket {bucket:?} must be created, got {status} with {}",
        String::from_utf8_lossy(&answered)
    );
}

/// Register one Data Catalog table over the objects under `s3://DATA_BUCKET/<folder>/`,
/// declaring `columns` as `(name, Hive type)` pairs, comma-separated with one header line.
pub(crate) async fn given_table(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    folder: &str,
    columns: &[(&str, &str)],
) {
    let columns: Vec<Value> = columns
        .iter()
        .map(|(name, data_type)| json!({ "Name": name, "Type": data_type }))
        .collect();
    glue(
        router,
        case,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "Parameters": {
                    "EXTERNAL": "TRUE",
                    "classification": "csv",
                    "skip.header.line.count": "1",
                },
                "StorageDescriptor": {
                    "Location": format!("s3://{DATA_BUCKET}/{folder}/"),
                    "Columns": columns,
                    "SerdeInfo": {
                        "SerializationLibrary": LAZY_SIMPLE_SERDE,
                        "Parameters": { "field.delim": "," },
                    },
                },
            },
        }),
    )
    .await;
}

/// What one relation of the collaboration is made of: the catalog table behind it, the
/// alias a statement names it by, the columns the configured table allows, and the ones
/// its `list` analysis rule lets a query join on and list.
pub(crate) struct Exposed<'names> {
    pub(crate) table: &'names str,
    pub(crate) alias: &'names str,
    pub(crate) allowed_columns: &'names [&'names str],
    pub(crate) join_columns: &'names [&'names str],
    pub(crate) list_columns: &'names [&'names str],
}

/// Configure `exposed.table` for the collaboration, give it a `list` analysis rule over
/// its join and list columns, and associate it under its alias: the membership's own
/// association is what a statement names.
pub(crate) async fn given_association(
    router: &MultiServiceRouter,
    case: &str,
    membership_id: &str,
    exposed: &Exposed<'_>,
) {
    let Exposed {
        table,
        alias,
        allowed_columns,
        join_columns,
        list_columns,
    } = *exposed;
    let configured = cleanrooms(
        router,
        case,
        "POST",
        "/configuredTables",
        Some(&json!({
            "name": format!("{table}-configured"),
            "tableReference": { "glue": { "tableName": table, "databaseName": DATABASE } },
            "allowedColumns": allowed_columns,
            "analysisMethod": "DIRECT_QUERY",
        })),
    )
    .await;
    let configured_table_id = configured["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no configuredTable.id in {configured}"))
        .to_string();
    cleanrooms(
        router,
        case,
        "POST",
        &format!("/configuredTables/{configured_table_id}/analysisRule"),
        Some(&json!({
            "analysisRuleType": "LIST",
            "analysisRulePolicy": {
                "v1": { "list": { "joinColumns": join_columns, "listColumns": list_columns } },
            },
        })),
    )
    .await;
    cleanrooms(
        router,
        case,
        "POST",
        &format!("/memberships/{membership_id}/configuredTableAssociations"),
        Some(&json!({
            "configuredTableIdentifier": configured_table_id,
            "name": alias,
            "roleArn": ROLE_ARN,
        })),
    )
    .await;
}

/// A collaboration this account can query in, joined with a membership whose default
/// result configuration writes CSV under [`RESULT_PREFIX`]: `(membership id,
/// collaboration id)`.
pub(crate) async fn given_membership(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let collaboration = cleanrooms(
        router,
        case,
        "POST",
        "/collaborations",
        Some(&json!({
            "name": "protected-query-run",
            "creatorDisplayName": "creator",
            "creatorMemberAbilities": ["CAN_QUERY", "CAN_RECEIVE_RESULTS"],
            "members": [],
            "queryLogStatus": "ENABLED",
        })),
    )
    .await;
    let collaboration_id = collaboration["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no collaboration.id in {collaboration}"))
        .to_string();
    let membership = cleanrooms(
        router,
        case,
        "POST",
        "/memberships",
        Some(&json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
            "defaultResultConfiguration": {
                "outputConfiguration": {
                    "s3": {
                        "resultFormat": "CSV",
                        "bucket": RESULTS_BUCKET,
                        "keyPrefix": RESULT_PREFIX,
                    },
                },
                "roleArn": ROLE_ARN,
            },
        })),
    )
    .await;
    let membership_id = membership["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no membership.id in {membership}"))
        .to_string();
    (membership_id, collaboration_id)
}

/// Submit `sql` under the membership's own default result configuration and answer the
/// protected query once it is terminal.
pub(crate) async fn ran(
    router: &MultiServiceRouter,
    case: &str,
    membership_id: &str,
    sql: &str,
) -> Value {
    ran_with(router, case, membership_id, sql, None).await
}

/// Submit `sql` with `result_configuration` as the call's own
/// `StartProtectedQueryInput$resultConfiguration` (`None` leaves the membership's default
/// in effect) and answer the protected query once it is terminal. Waits for the CONDITION,
/// never for a fixed time.
pub(crate) async fn ran_with(
    router: &MultiServiceRouter,
    case: &str,
    membership_id: &str,
    sql: &str,
    result_configuration: Option<&Value>,
) -> Value {
    let mut body = json!({
        "type": "SQL",
        "sqlParameters": { "queryString": sql },
    });
    if let Some(configuration) = result_configuration {
        body["resultConfiguration"] = configuration.clone();
    }
    let started = cleanrooms(
        router,
        case,
        "POST",
        &format!("/memberships/{membership_id}/protectedQueries"),
        Some(&body),
    )
    .await;
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no protectedQuery.id in {started}"))
        .to_string();
    let deadline = Instant::now() + PATIENCE;
    loop {
        let read = cleanrooms(
            router,
            case,
            "GET",
            &format!("/memberships/{membership_id}/protectedQueries/{query_id}"),
            None,
        )
        .await;
        let query = read["protectedQuery"].clone();
        let status = query["status"].as_str().unwrap_or_default().to_string();
        if !PENDING_STATUSES.contains(&status.as_str()) {
            return query;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the query never reached a terminal status, last read {query}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Every key `bucket` holds under `prefix`, read through `ListObjectsV2`.
pub(crate) async fn listed_keys(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    prefix: &str,
) -> Vec<String> {
    let encoded = prefix.replace('=', "%3D");
    let (status, body) = s3(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={encoded}"),
        &[],
    )
    .await;
    let document = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: the listing of {prefix:?} must succeed, got {status} with {document}"
    );
    document
        .split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split_once("</Key>"))
        .map(|(key, _)| key.to_string())
        .collect()
}

/// The bytes `bucket` holds at `key`.
pub(crate) async fn object_bytes(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
) -> Vec<u8> {
    let (status, body) = s3(router, "GET", &format!("/{bucket}/{key}"), &[]).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the result object {key:?} must be served, got {status} with {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// Those bytes as text.
pub(crate) async fn object_body(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
) -> String {
    String::from_utf8_lossy(&object_bytes(router, case, bucket, key).await).to_string()
}

/// The whole round trip: the data is staged in the object store, the catalog tables over
/// it are registered, the collaboration exposes them through `list` analysis rules, and
/// the protected query over them SUCCEEDS with the joined rows written where it says.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_query_runs_over_its_configured_tables_and_writes_the_rows_it_computed() {
    let router = emulator();
    let case = "a DISTINCT join over two associated tables";

    given_bucket(&router, case, DATA_BUCKET).await;
    given_bucket(&router, case, RESULTS_BUCKET).await;
    given_object(
        &router,
        case,
        DATA_BUCKET,
        "rows/rows.csv",
        "id,value,joinkey\n1,10,A\n2,20,B\n3,30,C\n",
    )
    .await;
    given_object(
        &router,
        case,
        DATA_BUCKET,
        "labels/labels.csv",
        "joinkey,label\nA,alpha\nB,beta\nC,gamma\n",
    )
    .await;
    glue(
        &router,
        case,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(
        &router,
        case,
        "rows",
        "rows",
        &[("id", "int"), ("value", "int"), ("joinkey", "string")],
    )
    .await;
    given_table(
        &router,
        case,
        "labels",
        "labels",
        &[("joinkey", "string"), ("label", "string")],
    )
    .await;

    let (membership_id, collaboration_id) = given_membership(&router, case).await;
    given_association(
        &router,
        case,
        &membership_id,
        &Exposed {
            table: "rows",
            alias: "rows_assoc",
            allowed_columns: &["id", "value", "joinkey"],
            join_columns: &["joinkey"],
            list_columns: &["id", "value"],
        },
    )
    .await;
    given_association(
        &router,
        case,
        &membership_id,
        &Exposed {
            table: "labels",
            alias: "labels_assoc",
            allowed_columns: &["joinkey", "label"],
            join_columns: &["joinkey"],
            list_columns: &["label"],
        },
    )
    .await;

    let query = ran(
        &router,
        case,
        &membership_id,
        "SELECT DISTINCT rows_assoc.id, rows_assoc.value, labels_assoc.label \
         FROM rows_assoc JOIN labels_assoc ON rows_assoc.joinkey = labels_assoc.joinkey",
    )
    .await;

    assert_eq!(
        query["status"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: a statement that satisfies the analysis rules succeeds, got {query}"
    );
    assert!(
        query.get("error").is_none(),
        "case {case:?}: a successful query carries no error, got {query}"
    );
    let duration = query["statistics"]["totalDurationInMillis"].as_i64();
    assert!(
        duration.is_some_and(|millis| millis > 0),
        "case {case:?}: the completion carries a positive duration, got {query}"
    );

    // The location is the folder every object of THIS query's results is under: the
    // configured prefix, the collaboration partition, then the query's own id.
    let query_id = query["id"].as_str().expect("the protected query id");
    let folder = format!(
        "{}collaboration={collaboration_id}/{query_id}/",
        RESULT_PREFIX
    );
    assert_eq!(
        query["result"]["output"]["s3"]["location"].as_str(),
        Some(format!("s3://{RESULTS_BUCKET}/{folder}").as_str()),
        "case {case:?}: the result location is the folder the object was written at, got {query}"
    );
    assert_eq!(
        query["resultConfiguration"]["outputConfiguration"]["s3"]["keyPrefix"].as_str(),
        Some(format!("{}collaboration={collaboration_id}", RESULT_PREFIX).trim_end_matches('/')),
        "case {case:?}: the reported prefix is the one the results went under, got {query}"
    );

    // BACKED: the object store itself serves an object at that address, and its bytes are
    // the rows the data really holds, joined as the statement asked.
    let keys = listed_keys(&router, case, RESULTS_BUCKET, &folder).await;
    assert_eq!(
        keys.len(),
        1,
        "case {case:?}: the run wrote one result object under {folder:?}, got {keys:?}"
    );
    let body = object_body(&router, case, RESULTS_BUCKET, &keys[0]).await;
    let mut lines: Vec<&str> = body.lines().collect();
    let header = lines.first().copied().unwrap_or_default().to_string();
    lines.remove(0);
    lines.sort_unstable();
    assert_eq!(
        header, "id,value,label",
        "case {case:?}: the result names the columns the statement selected, got {body:?}"
    );
    assert_eq!(
        lines,
        vec!["1,10,alpha", "2,20,beta", "3,30,gamma"],
        "case {case:?}: the result holds the joined rows, got {body:?}"
    );
}

/// The columns a statement may read are the ones the analysis rules expose: a column the
/// configured table allows but no rule names is not readable, and neither is one the
/// table does not allow at all — so such a statement is REFUSED rather than answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_reading_a_column_no_analysis_rule_allows_is_refused() {
    let router = emulator();

    given_bucket(&router, "setup", DATA_BUCKET).await;
    given_bucket(&router, "setup", RESULTS_BUCKET).await;
    given_object(
        &router,
        "setup",
        DATA_BUCKET,
        "rows/rows.csv",
        "id,value,joinkey\n1,10,A\n2,20,B\n3,30,C\n",
    )
    .await;
    glue(
        &router,
        "setup",
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    given_table(
        &router,
        "setup",
        "rows",
        "rows",
        &[("id", "int"), ("value", "int"), ("joinkey", "string")],
    )
    .await;
    let (membership_id, _) = given_membership(&router, "setup").await;
    // The rule lists `id` and joins on `joinkey`; `value` is a column the configured
    // table allows and no rule exposes, and `secret` is one it does not allow at all.
    given_association(
        &router,
        "setup",
        &membership_id,
        &Exposed {
            table: "rows",
            alias: "rows_assoc",
            allowed_columns: &["id", "value", "joinkey"],
            join_columns: &["joinkey"],
            list_columns: &["id"],
        },
    )
    .await;

    // Each row differs in WHICH unreadable thing the statement names, and the last one is
    // the statement the rules do allow — so the refusals are not a service that refuses
    // everything.
    let cases: Vec<(&str, &str, bool)> = vec![
        (
            "a column the table allows and no analysis rule exposes",
            "SELECT value FROM rows_assoc",
            false,
        ),
        (
            "a column the configured table does not allow at all",
            "SELECT secret FROM rows_assoc",
            false,
        ),
        (
            "a relation the membership associates nothing under",
            "SELECT id FROM other_assoc",
            false,
        ),
        (
            "a statement the analysis rule allows",
            "SELECT DISTINCT id FROM rows_assoc",
            true,
        ),
    ];

    for (case, sql, succeeds) in cases {
        let query = ran(&router, case, &membership_id, sql).await;
        if succeeds {
            assert_eq!(
                query["status"].as_str(),
                Some("SUCCESS"),
                "case {case:?}: the statement the rules allow is answered, got {query}"
            );
            continue;
        }
        assert_eq!(
            query["status"].as_str(),
            Some("FAILED"),
            "case {case:?}: a statement reading what no rule exposes is refused, got {query}"
        );
        assert_eq!(
            query["error"]["code"].as_str(),
            Some(
                service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state::PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY
            ),
            "case {case:?}: unexpected failure code, got {query}"
        );
        // A statement no analysis rule allows is refused the same way however often it is
        // submitted, so the failure reports that running it again is not expected to
        // succeed.
        assert_eq!(
            query["error"]["retry"]["retryable"].as_bool(),
            Some(false),
            "case {case:?}: a statement the rules refuse is no more allowed on a retry, got \
             {query}"
        );
        assert!(
            query.get("result").is_none() && query.get("statistics").is_none(),
            "case {case:?}: a query that did not run reports no result and no statistics, got \
             {query}"
        );
    }
}
