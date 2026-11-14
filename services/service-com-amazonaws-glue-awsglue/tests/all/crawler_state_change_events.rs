// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EventBridge event a Glue crawl publishes when it comes to rest.
//!
//! The rule under test: a crawl that comes to rest — the instant the clock owes the crawler
//! its way back to `READY` and its `LastCrawl` becomes readable — is published onto the
//! account's DEFAULT EventBridge bus as `source` `aws.glue` / `detail-type` `Glue Crawler
//! State Change`, whose `detail` carries that crawler's own `crawlerName`, the `accountId` it
//! is held in, the `state` the crawl came to rest in, the `message` the service has about it
//! and the tables it wrote (`tablesCreated`, `tablesUpdated`, `tablesDeleted`). EventBridge
//! then routes it, so a rule whose `EventPattern` matches delivers it to that rule's targets
//! and a rule that does not match receives nothing, while a target queue that has granted
//! EventBridge nothing receives nothing either. A crawl still in flight has come to rest at
//! nothing and publishes nothing, and a crawl already published is not published again.
//!
//! The recorded case — one crawler over one Amazon S3 prefix whose crawl succeeds, matched by
//! a rule on that source and detail-type and delivered to its target — is ONE row of the table
//! below; the rows differ along what the rule generalises over: the state the crawl comes to
//! rest in (it wrote what it found, or it could not read its target at all), how many datasets
//! it wrote, and which of the table counts carries the figure.
//!
//! EventBridge, SQS and Amazon S3 are mounted BESIDE Glue on the SAME engine, so the
//! publication travels the real in-process callback, through the real bus, to a real SQS
//! target: nothing here reads the publisher's own queue. The due instant a crawl's finish
//! falls at is moved THROUGH THE STORE rather than slept for
//! ([`crate::crawler_runs::set_due_instant`]), and the event is waited for as a CONDITION on a
//! generous deadline, so no case asserts on how fast the host runs.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::crawler_runs::{
    already_past, far_future, given_crawler_over, given_database, set_due_instant, stage, DATABASE,
};
use crate::job_run_state_change_events::{
    delivered_event, drain, given_queue, given_rule, grant_delivery,
};
use crate::wire::{ok, ACCOUNT_ID};

/// The `source` and `detail-type` Glue publishes a crawl's outcome under, and the two `state`
/// values this emulator's crawls come to rest in — spelled out here rather than imported, so a
/// wrong constant in the implementation cannot make a test pass.
const EVENT_SOURCE: &str = "aws.glue";
const CRAWLER_STATE_CHANGE: &str = "Glue Crawler State Change";
const STATE_SUCCEEDED: &str = "Succeeded";
const STATE_FAILED: &str = "Failed";

/// The sentence the event carries for a crawl that failed at nothing.
const SUCCEEDED_MESSAGE: &str = "Crawler Succeeded";

/// The rule that matches a Glue crawler event, and the rule that cannot.
const MATCHING_RULE: &str = "glue-crawler-state-change";
const OTHER_RULE: &str = "unrelated-crawler-events";

/// An emulator with Glue, Amazon S3, EventBridge AND SQS on one shared engine, plus that
/// engine.
///
/// Amazon S3 is mounted because a crawl READS the objects under its targets; EventBridge and
/// SQS because the publication goes onto the account's default bus and a rule on that bus
/// delivers it to a queue.
fn emulator_with_a_bus() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = crate::wire::emulator_on(Arc::clone(&engine));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One row: the data a crawler is pointed at, and the event its crawl must publish.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The bucket the row's objects live in (no underscore: `CreateBucket` refuses one), or
    /// `None` for a row whose target names a bucket that does not exist at all.
    bucket: Option<&'static str>,
    /// The objects staged in it, as `(key, body)`.
    objects: Vec<(&'static str, &'static str)>,
    /// The `S3Targets` list the crawler carries.
    targets: Value,
    /// The `state` the event must carry.
    state: &'static str,
    /// The tables the crawl must report created, updated and deleted.
    counts: (i64, i64, i64),
}

/// The rows: a crawl that wrote one dataset, one that wrote two, and one that could not read
/// its target at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one dataset of delimited text",
            bucket: Some("crawler-events-one"),
            objects: vec![("sales/day=01/part-0.csv", "order_id,amount\n1,10.5\n")],
            targets: json!([{ "Path": "s3://crawler-events-one/sales/" }]),
            state: STATE_SUCCEEDED,
            counts: (1, 0, 0),
        },
        Row {
            case: "two datasets, one per target",
            bucket: Some("crawler-events-two"),
            objects: vec![
                ("orders/day=01/part-0.csv", "order_id\n1\n"),
                ("items/day=01/part-0.csv", "item_id\n7\n"),
            ],
            targets: json!([
                { "Path": "s3://crawler-events-two/orders/" },
                { "Path": "s3://crawler-events-two/items/" },
            ]),
            state: STATE_SUCCEEDED,
            counts: (2, 0, 0),
        },
        Row {
            case: "a target whose bucket the object store does not serve",
            bucket: None,
            objects: Vec::new(),
            targets: json!([{ "Path": "s3://crawler-events-absent/sales/" }]),
            state: STATE_FAILED,
            counts: (0, 0, 0),
        },
    ]
}

/// The `CrawlerMetrics` structure `GetCrawlerMetrics` reports for `name`: the table counts the
/// event's own figures must agree with.
async fn metrics_of(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(
        router,
        "GetCrawlerMetrics",
        &json!({ "CrawlerNameList": [name] }),
    )
    .await;
    body.pointer("/CrawlerMetricsList/0")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawlerMetrics must report the crawler, got {body}"))
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn crawler_of(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawler must report a Crawler, got {body}"))
}

/// Assert the envelope and the `detail` of one published crawler state change.
fn assert_event(case: &str, event: &Value, crawler: &str, state: &str, message: &str, counts: (i64, i64, i64)) {
    assert_eq!(
        event["source"],
        json!(EVENT_SOURCE),
        "case {case:?}: expected source {EVENT_SOURCE:?}, got {event}"
    );
    assert_eq!(
        event["detail-type"],
        json!(CRAWLER_STATE_CHANGE),
        "case {case:?}: expected detail-type {CRAWLER_STATE_CHANGE:?}, got {event}"
    );
    let detail = &event["detail"];
    assert_eq!(
        detail["crawlerName"],
        json!(crawler),
        "case {case:?}: the detail must name the crawler, got {detail}"
    );
    assert_eq!(
        detail["accountId"],
        json!(ACCOUNT_ID),
        "case {case:?}: the detail must name the account, got {detail}"
    );
    assert_eq!(
        detail["state"],
        json!(state),
        "case {case:?}: expected state {state:?}, got {detail}"
    );
    assert_eq!(
        detail["message"],
        json!(message),
        "case {case:?}: expected the message {message:?}, got {detail}"
    );
    let (created, updated, deleted) = counts;
    assert_eq!(
        detail["tablesCreated"],
        json!(created.to_string()),
        "case {case:?}: expected {created} table(s) created, got {detail}"
    );
    assert_eq!(
        detail["tablesUpdated"],
        json!(updated.to_string()),
        "case {case:?}: expected {updated} table(s) updated, got {detail}"
    );
    assert_eq!(
        detail["tablesDeleted"],
        json!(deleted.to_string()),
        "case {case:?}: expected {deleted} table(s) deleted, got {detail}"
    );
}

/// Every crawl that comes to rest is published onto the default bus as a `Glue Crawler State
/// Change` event, delivered to the matching rule's granted target — and withheld from the
/// queue that granted EventBridge nothing and from the rule that does not match.
#[tokio::test]
async fn a_crawl_that_comes_to_rest_publishes_a_glue_crawler_state_change() {
    let (router, engine) = emulator_with_a_bus();
    given_database(&router, DATABASE).await;
    let (granted_url, granted_arn) = given_queue(&router, "glue-crawler-events").await;
    let (ungranted_url, ungranted_arn) =
        given_queue(&router, "glue-crawler-events-no-policy").await;
    let (unrelated_url, unrelated_arn) = given_queue(&router, "unrelated-crawler-events").await;
    grant_delivery(&router, &granted_url, &granted_arn).await;
    grant_delivery(&router, &unrelated_url, &unrelated_arn).await;
    given_rule(
        &router,
        MATCHING_RULE,
        json!({ "source": [EVENT_SOURCE], "detail-type": [CRAWLER_STATE_CHANGE] }),
        &[granted_arn, ungranted_arn],
    )
    .await;
    given_rule(
        &router,
        OTHER_RULE,
        json!({ "source": ["aws.ecs"], "detail-type": ["ECS Task State Change"] }),
        &[unrelated_arn],
    )
    .await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let crawler = format!("crawler-event-{index}");
        if let Some(bucket) = row.bucket {
            stage(&router, bucket, &row.objects).await;
        }
        given_crawler_over(&router, &crawler, row.targets.clone(), json!({})).await;

        // The crawl is held in flight first: a crawl that has come to rest at nothing
        // publishes nothing, whatever its data looked like.
        ok(&router, "StartCrawler", &json!({ "Name": crawler })).await;
        set_due_instant(&engine, far_future());
        let in_flight = drain(&router, &granted_url).await;
        assert!(
            in_flight.is_empty(),
            "case {case:?}: a crawl still in flight publishes nothing, got {in_flight:?}"
        );

        // The finish the clock owes is made due through the store, and the crawler read back:
        // the `LastCrawl` the read reports is the crawl the event must report.
        set_due_instant(&engine, already_past());
        let reported = crawler_of(&router, &crawler).await;
        assert_eq!(
            reported.pointer("/LastCrawl/Status").and_then(Value::as_str),
            Some(if row.state == STATE_SUCCEEDED {
                "SUCCEEDED"
            } else {
                "FAILED"
            }),
            "case {case:?}: the crawl must have come to rest, got {reported}"
        );
        // A crawl that failed states its reason, and the event's message is that same reason;
        // one that failed at nothing carries the service's account of what became of it.
        let message = reported
            .pointer("/LastCrawl/ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or(SUCCEEDED_MESSAGE)
            .to_string();

        // The clock pump publishes what the settle recorded, so the event is waited for
        // rather than expected at an instant.
        let event = delivered_event(&router, case, &granted_url).await;
        assert_event(case, &event, &crawler, row.state, &message, row.counts);

        // The figures the event carries are the ones a metrics read answers with: the event
        // reports the crawl this service holds and not a second copy of it.
        let metrics = metrics_of(&router, &crawler).await;
        let (created, updated, deleted) = row.counts;
        assert_eq!(
            (
                metrics.get("TablesCreated").and_then(Value::as_i64),
                metrics.get("TablesUpdated").and_then(Value::as_i64),
                metrics.get("TablesDeleted").and_then(Value::as_i64),
            ),
            (Some(created), Some(updated), Some(deleted)),
            "case {case:?}: the event's figures must be the crawler's own, got {metrics}"
        );

        // Read again: the crawl has already been published, so nothing is published twice.
        crawler_of(&router, &crawler).await;
        let again = drain(&router, &granted_url).await;
        assert!(
            again.is_empty(),
            "case {case:?}: a crawl already published is not published again, got {again:?}"
        );
    }

    // The negative half of the delivery contract: the sibling target that granted EventBridge
    // nothing received none of the events, and neither did the rule that does not match them.
    let withheld = drain(&router, &ungranted_url).await;
    assert!(
        withheld.is_empty(),
        "a target queue with no resource policy receives nothing, got {withheld:?}"
    );
    let unrelated = drain(&router, &unrelated_url).await;
    assert!(
        unrelated.is_empty(),
        "a rule that does not match receives nothing, got {unrelated:?}"
    );
}

/// Each crawl of one crawler publishes its OWN event: the second crawl of the same data
/// rewrites the table the first one created, so the figures move from created to updated.
#[tokio::test]
async fn each_crawl_of_one_crawler_publishes_its_own_event() {
    let (router, engine) = emulator_with_a_bus();
    given_database(&router, DATABASE).await;
    let (url, arn) = given_queue(&router, "glue-crawler-events-again").await;
    grant_delivery(&router, &url, &arn).await;
    given_rule(
        &router,
        MATCHING_RULE,
        json!({ "source": [EVENT_SOURCE], "detail-type": [CRAWLER_STATE_CHANGE] }),
        &[arn],
    )
    .await;
    stage(
        &router,
        "crawler-events-twice",
        &[("sales/day=01/part-0.csv", "order_id,amount\n1,10.5\n")],
    )
    .await;
    let crawler = "crawler-event-recrawled";
    given_crawler_over(
        &router,
        crawler,
        json!([{ "Path": "s3://crawler-events-twice/sales/" }]),
        json!({}),
    )
    .await;

    // The first crawl created the table; the second found it there and rewrote it.
    for (case, counts) in [
        ("the first crawl of the dataset", (1, 0, 0)),
        ("the second crawl of the same dataset", (0, 1, 0)),
    ] {
        ok(&router, "StartCrawler", &json!({ "Name": crawler })).await;
        set_due_instant(&engine, already_past());
        crawler_of(&router, crawler).await;
        let event = delivered_event(&router, case, &url).await;
        assert_event(
            case,
            &event,
            crawler,
            STATE_SUCCEEDED,
            SUCCEEDED_MESSAGE,
            counts,
        );
    }
}
