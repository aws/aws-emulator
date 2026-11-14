// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation-deployed `AWS::Glue::Crawler` DOES: the crawl a client starts on it,
//! and the Data Catalog tables that crawl publishes.
//!
//! The rule under test: **a crawler a stack reports as created is a crawler AWS Glue holds and
//! can RUN.** `StartCrawler` on the name the stack published answers 200 with an empty body,
//! the crawler leaves the state with no run in it, the objects under each of its Amazon S3
//! targets are walked, and every dataset found behind them is written into the crawler's
//! target database as a table located under that target's `s3://` prefix with the columns the
//! data carries. The clock owes the crawler its way back to `READY` one span later, which is
//! when the crawl's `LastCrawl.Status` and the table counts `GetCrawlerMetrics` reports become
//! readable. Its three modelled outcomes are the only ones it answers: an accepted start, the
//! `EntityNotFoundException` a name no crawler in the account has earns, and the
//! `CrawlerRunningException` a crawler whose crawl is in flight earns — never a status the
//! model does not declare for the operation.
//!
//! Four crates have to agree for that to hold (CloudFormation makes the create calls, IAM
//! answers for the crawl role, AWS Glue serves the crawl and the catalog, Amazon S3 holds the
//! data the crawl reads), so the suite lives here: a crate-local test can only ever see one
//! side of it. The whole emulator is assembled, exactly as the production entrypoint
//! assembles it.
//!
//! The deployed case — one stack declaring a database and a crawler over one `s3://<bucket>/`
//! prefix of comma-separated data — is ONE row of the table below; the rows differ along what
//! the rule generalises over: how many Amazon S3 targets the declaration names, whether a
//! target's path ends in the separator, the layout of the data behind it (comma-separated with
//! a header, comma-separated without one, one JSON document per line), whether the declaration
//! carries a `TablePrefix`, and how many datasets the crawl therefore publishes.
//!
//! Neither clock is waited for: the stack's due instants are pulled into the past
//! ([`crate::cloudformation_stack_clock::settle_the_stacks`]) and the crawl's is moved THROUGH
//! THE GLUE STORE ([`move_the_crawl_due_instant`]), so every transition here is the emulator's
//! own settling rather than a race against the host.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    LAST_CRAWL_STATUS_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_glue_jobs::{cloudformation, error_code, glue, TRUST_POLICY};
use crate::cloudformation_stack_clock::settle_the_stacks;
use crate::query_wire::{opt_element, outputs_of};

/// The Region every stack here is deployed in, and the Region the crawl reads its objects
/// through: a crawl walks the targets of the request that started it.
const REGION: &str = "us-east-1";

/// The logical ids every row's template gives the three resources it declares.
const ROLE_LOGICAL_ID: &str = "CrawlerRole";
const DATABASE_LOGICAL_ID: &str = "CatalogDatabase";
const CRAWLER_LOGICAL_ID: &str = "CatalogCrawler";

/// The output keys every row's template publishes — `Ref` of the crawler and of the database,
/// which is how a client reaches what the stack deployed at all.
const CRAWLER_OUTPUT: &str = "CatalogCrawlerName";
const DATABASE_OUTPUT: &str = "CatalogDatabaseName";

/// How many settling rounds a stack is given to reach a terminal status. Generous: each round
/// materialises one level of the declaration's dependency chain (the role, then the database,
/// then the crawler), and no round waits for the wall clock.
const SETTLING_ROUNDS: usize = 200;

/// How long the crawl's owed finish is polled for, and how often. Generous on purpose: the
/// condition polled for is the crawler's own state, never an elapsed time.
const CRAWL_POLL_ROUNDS: usize = 600;
const CRAWL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How far into the past an owed instant is pulled, so the next read finds it due however
/// coarse the host clock is.
const ALREADY_DUE: Duration = Duration::from_secs(1);

/// Far enough out that no owed finish falls due while a case asserts the transient state.
const NOT_YET_DUE: Duration = Duration::from_secs(3_600);

/// The `(Name, Type)` pairs of one `Column` list, in the order it holds them.
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

/// What one table the crawl must publish looks like.
struct ExpectedTable {
    /// The name it is written under: the dataset's own name behind the crawler's
    /// `TablePrefix`.
    name: &'static str,
    /// Its `StorageDescriptor$Location`: the `s3://` folder the dataset sits in.
    location: &'static str,
    /// The columns the data carries, as `(name, type)`.
    columns: &'static [(&'static str, &'static str)],
}

/// One row of the table: a stack declaring a crawler over staged Amazon S3 data, and what the
/// rule says the crawl it can then run must publish.
struct Deployment {
    case: &'static str,
    /// The bucket the row's objects live in (no underscore: `CreateBucket` refuses one).
    bucket: &'static str,
    /// The objects staged behind the targets, as `(key, body)`.
    objects: &'static [(&'static str, &'static str)],
    /// The `Targets` property the declaration carries.
    targets: fn() -> Value,
    /// The `TablePrefix` property it carries, if any.
    table_prefix: Option<&'static str>,
    /// The tables the crawl must publish, in name order.
    tables: &'static [ExpectedTable],
}

/// The rows: three stacks, each declaring a crawler over differently laid-out data.
fn deployments() -> Vec<Deployment> {
    vec![
        Deployment {
            case: "one target prefix of comma-separated data with a header",
            bucket: "cfn-crawl-sales",
            objects: &[("data/sample.csv", "id,name,value\n1,alpha,10\n2,beta,20\n")],
            targets: || json!({ "S3Targets": [{ "Path": "s3://cfn-crawl-sales/data/" }] }),
            table_prefix: None,
            tables: &[ExpectedTable {
                name: "data",
                location: "s3://cfn-crawl-sales/data/",
                columns: &[("id", "bigint"), ("name", "string"), ("value", "bigint")],
            }],
        },
        Deployment {
            case: "a target without a trailing separator, a table prefix, one JSON document \
                   per line",
            bucket: "cfn-crawl-events",
            objects: &[(
                "events/a.json",
                "{\"amount\": 1.5, \"id\": 3, \"ok\": true, \"who\": \"me\"}\n",
            )],
            targets: || json!({ "S3Targets": [{ "Path": "s3://cfn-crawl-events/events" }] }),
            table_prefix: Some("raw_"),
            tables: &[ExpectedTable {
                name: "raw_events",
                location: "s3://cfn-crawl-events/events/",
                columns: &[
                    ("amount", "double"),
                    ("id", "int"),
                    ("ok", "boolean"),
                    ("who", "string"),
                ],
            }],
        },
        Deployment {
            case: "two targets, one of them headerless, publish one table each",
            bucket: "cfn-crawl-multi",
            objects: &[
                ("orders/a.csv", "order_id,total\n1,2.5\n"),
                ("raw/rows.csv", "1,alpha\n2,beta\n"),
            ],
            targets: || {
                json!({
                    "S3Targets": [
                        { "Path": "s3://cfn-crawl-multi/orders/" },
                        { "Path": "s3://cfn-crawl-multi/raw/" },
                    ]
                })
            },
            table_prefix: None,
            tables: &[
                ExpectedTable {
                    name: "orders",
                    location: "s3://cfn-crawl-multi/orders/",
                    columns: &[("order_id", "bigint"), ("total", "double")],
                },
                ExpectedTable {
                    name: "raw",
                    location: "s3://cfn-crawl-multi/raw/",
                    columns: &[("col0", "bigint"), ("col1", "string")],
                },
            ],
        },
    ]
}

/// The emulator as it is ASSEMBLED, on one engine: the stack's calls, the crawl's reads and the
/// catalog it writes all land in the same store the production entrypoint serves from.
fn build_router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(Arc::clone(&engine));
    router.set_request_authentication(false);
    (router, engine)
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon S3 request against the Region's path-style endpoint, which is the endpoint a
/// crawl reads its targets through.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.as_bytes().to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// Create `bucket` and put every `(key, body)` into it, so the crawl has data to read.
async fn stage(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    objects: &[(&str, &str)],
) {
    let (status, body) = s3(router, "PUT", &format!("/{bucket}"), "").await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {status} with {body}"
    );
    for (key, content) in objects {
        let (status, body) = s3(router, "PUT", &format!("/{bucket}/{key}"), content).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: PutObject({key:?}) must succeed, got {status} with {body}"
        );
    }
}

/// The template one row deploys: the crawl role, the catalog database the crawler writes into,
/// and the crawler itself over the row's targets — each entity named by the stack, and both
/// names published as outputs.
fn template(row: &Deployment, database: &str, crawler: &str) -> String {
    let mut properties = serde_json::Map::new();
    properties.insert("Name".to_string(), json!(crawler));
    properties.insert(
        "Role".to_string(),
        json!({ "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] }),
    );
    properties.insert(
        "DatabaseName".to_string(),
        json!({ "Ref": DATABASE_LOGICAL_ID }),
    );
    properties.insert("Targets".to_string(), (row.targets)());
    if let Some(prefix) = row.table_prefix {
        properties.insert("TablePrefix".to_string(), json!(prefix));
    }

    let trust: Value = serde_json::from_str(TRUST_POLICY).expect("the trust policy is a document");
    json!({
        "Description": "a Glue Data Catalog database and a crawler over Amazon S3",
        "Resources": {
            ROLE_LOGICAL_ID: {
                "Type": "AWS::IAM::Role",
                "Properties": { "AssumeRolePolicyDocument": trust }
            },
            DATABASE_LOGICAL_ID: {
                "Type": "AWS::Glue::Database",
                "Properties": { "DatabaseInput": { "Name": database } }
            },
            CRAWLER_LOGICAL_ID: {
                "Type": "AWS::Glue::Crawler",
                "Properties": Value::Object(properties)
            }
        },
        "Outputs": {
            CRAWLER_OUTPUT: { "Value": { "Ref": CRAWLER_LOGICAL_ID } },
            DATABASE_OUTPUT: { "Value": { "Ref": DATABASE_LOGICAL_ID } }
        }
    })
    .to_string()
}

/// Deploy `body` as `stack_name` and answer the `DescribeStacks` XML of the stack once it has
/// reached a terminal status — which it reaches on its own clock, pulled into the past rather
/// than waited for.
async fn deploy(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack_name: &str,
    body: &str,
) -> String {
    let (status, answer) = cloudformation(
        router,
        REGION,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", body),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {answer}"
    );

    for _ in 0..SETTLING_ROUNDS {
        settle_the_stacks(router, engine, case).await;
        let (_, described) =
            cloudformation(router, REGION, "DescribeStacks", &[("StackName", stack_name)]).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED") {
            return described;
        }
    }
    panic!("case {case:?}: the stack never reached a terminal status");
}

/// Move the due instant of every crawl in flight to `at`, so a case can hold the transient
/// window open or make the owed finish due without waiting for it.
fn move_the_crawl_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for crawler in store.crawlers.values_mut() {
        if crawler.crawl_settles_at.is_some() {
            crawler.crawl_settles_at = Some(at);
        }
    }
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn crawler_of(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = glue(router, REGION, "GetCrawler", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCrawler({name:?}), got {status} with {body}"
    );
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetCrawler must report a Crawler, got {body}"))
}

/// The crawler once the finish the clock owed its crawl has been materialised: polled for the
/// CONDITION — its run state — rather than for any elapsed time.
async fn crawler_once_the_crawl_finished(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Value {
    for _ in 0..CRAWL_POLL_ROUNDS {
        let crawler = crawler_of(router, case, name).await;
        if crawler.get("State").and_then(Value::as_str) == Some(CRAWLER_STATE_READY) {
            return crawler;
        }
        tokio::time::sleep(CRAWL_POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the crawl of {name:?} never finished");
}

/// The tables `GetTables` reports for `database`, in name order.
async fn tables_of(router: &MultiServiceRouter, case: &str, database: &str) -> Vec<Value> {
    let (status, body) = glue(
        router,
        REGION,
        "GetTables",
        &json!({ "DatabaseName": database }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetTables({database:?}), got {status} with {body}"
    );
    let mut tables: Vec<Value> = body
        .get("TableList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    tables.sort_by_key(|table| {
        table
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    tables
}

/// The one `CrawlerMetrics` element `GetCrawlerMetrics` reports for `crawler`.
async fn metrics_of(router: &MultiServiceRouter, case: &str, crawler: &str) -> Value {
    let (status, body) = glue(
        router,
        REGION,
        "GetCrawlerMetrics",
        &json!({ "CrawlerNameList": [crawler] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCrawlerMetrics, got {status} with {body}"
    );
    let reported = body
        .get("CrawlerMetricsList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the one selected crawler has one metrics element, got {body}"
    );
    reported[0].clone()
}

/// A `CrawlerMetrics` member as the whole number it reports.
fn metric(metrics: &Value, member: &str) -> i64 {
    metrics.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// `StartCrawler` for `name`, and what it answered.
async fn start_crawler(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    glue(router, REGION, "StartCrawler", &json!({ "Name": name })).await
}

/// The `message` an awsJson error answer carries.
fn error_message(payload: &Value) -> String {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// What a stack-deployed crawler can be made to DO
// ---------------------------------------------------------------------------

/// A crawler a stack deployed crawls its Amazon S3 targets when it is started, and the
/// datasets it found are published into the database the declaration named.
#[tokio::test]
async fn a_stack_deployed_crawler_crawls_its_targets_and_publishes_its_tables() {
    for row in deployments() {
        let case = row.case;
        let (router, engine) = build_router_and_engine();
        stage(&router, case, row.bucket, row.objects).await;

        let database = format!("{}_db", row.bucket.replace('-', "_"));
        let crawler = format!("{}-crawler", row.bucket);
        let stack = format!("{}-stack", row.bucket);
        let body = template(&row, &database, &crawler);
        let described = deploy(&router, &engine, case, &stack, &body).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // The handles a client deploys FOR: the two names the stack publishes, which is what
        // every call below addresses — never a name this test chose for itself.
        let outputs = outputs_of(&described);
        let deployed_crawler = outputs
            .get(CRAWLER_OUTPUT)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack must publish {CRAWLER_OUTPUT}"));
        let deployed_database = outputs
            .get(DATABASE_OUTPUT)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack must publish {DATABASE_OUTPUT}"));
        assert_eq!(
            (deployed_crawler.as_str(), deployed_database.as_str()),
            (crawler.as_str(), database.as_str()),
            "case {case:?}: the published handles are the declared names, got {described}"
        );

        // Before the crawl: the deployed crawler is addressable, has no run in it and has
        // published nothing — so every table asserted below came from the crawl.
        let deployed = crawler_of(&router, case, &deployed_crawler).await;
        assert_eq!(
            deployed.get("DatabaseName").and_then(Value::as_str),
            Some(deployed_database.as_str()),
            "case {case:?}: the crawler writes into the declared database, got {deployed}"
        );
        assert_eq!(
            deployed.get("State").and_then(Value::as_str),
            Some(CRAWLER_STATE_READY),
            "case {case:?}: a crawler nobody has started has no run in it, got {deployed}"
        );
        assert!(
            deployed.get("LastCrawl").is_none(),
            "case {case:?}: a crawler that has never crawled reports no crawl, got {deployed}"
        );
        let (status, read_database) = glue(
            &router,
            REGION,
            "GetDatabase",
            &json!({ "Name": deployed_database }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the declared database answers, got {status} with {read_database}"
        );
        let before = tables_of(&router, case, &deployed_database).await;
        assert!(
            before.is_empty(),
            "case {case:?}: the deploy published no table of its own, got {before:?}"
        );

        // The start itself: an accepted crawl, answered with the operation's EMPTY output.
        let (status, answer) = start_crawler(&router, &deployed_crawler).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartCrawler, got {status} with {answer}"
        );
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: StartCrawler reports no member of its own, got {answer}"
        );

        // The transient window the clock owes the crawler, held open so what it refuses is
        // observable without waiting: a crawl is in progress, and another start of the SAME
        // crawler is the modelled conflict.
        move_the_crawl_due_instant(&engine, SystemTime::now() + NOT_YET_DUE);
        let running = crawler_of(&router, case, &deployed_crawler).await;
        assert_eq!(
            running.get("State").and_then(Value::as_str),
            Some(CRAWLER_STATE_RUNNING),
            "case {case:?}: the started crawl is in progress, got {running}"
        );
        let (status, refused) = start_crawler(&router, &deployed_crawler).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a crawl in progress refuses another start, got {status} with \
             {refused}"
        );
        assert!(
            error_code(&refused).ends_with("CrawlerRunningException"),
            "case {case:?}: expected CrawlerRunningException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            messages::crawler_already_started(&deployed_crawler),
            "case {case:?}: expected the modelled wording, got {refused}"
        );

        // The finish the clock owed is materialised, and what the crawl did becomes readable.
        move_the_crawl_due_instant(&engine, SystemTime::now() - ALREADY_DUE);
        let finished = crawler_once_the_crawl_finished(&router, case, &deployed_crawler).await;
        assert_eq!(
            finished.pointer("/LastCrawl/Status").and_then(Value::as_str),
            Some(LAST_CRAWL_STATUS_SUCCEEDED),
            "case {case:?}: the crawl read its targets, got {finished}"
        );
        assert!(
            finished.pointer("/LastCrawl/ErrorMessage").is_none(),
            "case {case:?}: a crawl that succeeded carries no error message, got {finished}"
        );

        // What the crawl PUBLISHED: one table per dataset behind the targets, each located
        // under the target's own prefix, with the columns the data carries.
        let published = tables_of(&router, case, &deployed_database).await;
        let names: Vec<&str> = published
            .iter()
            .map(|table| table.get("Name").and_then(Value::as_str).unwrap_or_default())
            .collect();
        let expected_names: Vec<&str> = row.tables.iter().map(|table| table.name).collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the tables {expected_names:?}, got {names:?}"
        );
        for (expected, table) in row.tables.iter().zip(published.iter()) {
            let location = table
                .pointer("/StorageDescriptor/Location")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                location, expected.location,
                "case {case:?}: expected {} located at {}, got {location:?}",
                expected.name, expected.location
            );
            assert!(
                location.starts_with(&format!("s3://{}/", row.bucket)),
                "case {case:?}: expected {} under the crawled bucket, got {location:?}",
                expected.name
            );
            let expected_columns: Vec<(String, String)> = expected
                .columns
                .iter()
                .map(|(name, declared)| ((*name).to_string(), (*declared).to_string()))
                .collect();
            let actual_columns = named_types(table.pointer("/StorageDescriptor/Columns"));
            assert_eq!(
                actual_columns, expected_columns,
                "case {case:?}: expected {} to carry {expected_columns:?}, got {actual_columns:?}",
                expected.name
            );
        }

        // The counters the metrics read reports are the ones the crawl that just finished
        // wrote: every table it published is a table it CREATED.
        let created = i64::try_from(row.tables.len()).expect("a handful of tables");
        let metrics = metrics_of(&router, case, &deployed_crawler).await;
        assert_eq!(
            metrics.get("CrawlerName").and_then(Value::as_str),
            Some(deployed_crawler.as_str()),
            "case {case:?}: the metrics are reported for the selected crawler, got {metrics}"
        );
        assert_eq!(
            (metric(&metrics, "TablesCreated"), metric(&metrics, "TablesUpdated")),
            (created, 0),
            "case {case:?}: expected the first crawl to create {created} table(s), got {metrics}"
        );

        // A SECOND crawl of the same data updates the tables the first one created rather
        // than creating them again, which is what the counters then report.
        let (status, answer) = start_crawler(&router, &deployed_crawler).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the second StartCrawler, got {status} with \
             {answer}"
        );
        move_the_crawl_due_instant(&engine, SystemTime::now() - ALREADY_DUE);
        let recrawled = crawler_once_the_crawl_finished(&router, case, &deployed_crawler).await;
        assert_eq!(
            recrawled.pointer("/LastCrawl/Status").and_then(Value::as_str),
            Some(LAST_CRAWL_STATUS_SUCCEEDED),
            "case {case:?}: the second crawl read its targets too, got {recrawled}"
        );
        let metrics = metrics_of(&router, case, &deployed_crawler).await;
        assert_eq!(
            (metric(&metrics, "TablesCreated"), metric(&metrics, "TablesUpdated")),
            (0, created),
            "case {case:?}: expected the recrawl to update {created} table(s), got {metrics}"
        );
        let after = tables_of(&router, case, &deployed_database).await;
        assert_eq!(
            after.len(),
            published.len(),
            "case {case:?}: the recrawl publishes no second copy of a dataset, got {after:?}"
        );
    }
}

/// A name no crawler in the account has is refused with the operation's OWN modelled miss —
/// the client error the model declares, not a status it does not.
///
/// The rows differ along the form of the name: the shortest `NameString` accepts, an ordinary
/// one, one carrying a space, and the longest it accepts (`@length(min: 1, max: 255)`, from
/// `smithy_query(query="operation",operation="StartCrawler")`).
#[tokio::test]
async fn starting_a_crawler_this_account_does_not_hold_is_the_modelled_miss() {
    let longest = "c".repeat(255);
    let rows: Vec<(&str, &str)> = vec![
        ("the shortest name", "c"),
        ("an ordinary name", "no-such-crawler"),
        ("a name carrying a space", "no such crawler"),
        ("the longest name", longest.as_str()),
    ];

    let (router, _engine) = build_router_and_engine();
    for (case, name) in rows {
        let (status, refused) = start_crawler(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from StartCrawler({name:?}), got {status} with \
             {refused}"
        );
        assert!(
            error_code(&refused).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            messages::crawler_not_found(name),
            "case {case:?}: expected the modelled wording, got {refused}"
        );
    }
}
