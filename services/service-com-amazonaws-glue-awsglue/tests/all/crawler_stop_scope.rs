// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SCOPE of `StopCrawler`: which crawl a stop ends, what every
//! crawler reader then reports about it, and what a stopped crawl leaves behind.
//!
//! The rule under test: "if the specified crawler is running, stops the crawl" — a stop ends
//! the crawl of the ONE crawler it names and nothing else, that crawler leaves `RUNNING` for
//! `STOPPING` with the clock owing it its way back to `READY`, and the crawl it ended is
//! reported — by every surface that reports a crawl — with the modelled `LastCrawlStatus`
//! `CANCELLED` and the modelled `CrawlerHistoryState` `STOPPED`, keeping the catalog tables it
//! had already written (sources: `StopCrawler`'s own documentation, `Crawler$State`,
//! `LastCrawlInfo$Status`, `CrawlerHistory$State` and `CrawlerMetrics`). The crawler is then a
//! crawler at rest like any other: it can crawl and be stopped again.
//!
//! Where `tests/all/crawler_stop.rs` walks ONE crawler's states and the three refusal classes,
//! this suite is about the stop's reach: the rows below differ along which crawler of several
//! under way the stop names, which reader is asked about the stopped crawl, and whether a
//! cycle's crawl was stopped or left to finish.
//!
//! The due instant an owed finish falls at is moved THROUGH THE STORE rather than slept for
//! ([`crate::crawler_runs::set_due_instant`]), so every transition here is deterministic and
//! nothing waits on the wall clock. Calls go over the wire through the shared [`crate::wire`]
//! harness, and the objects the crawls read are staged through Amazon S3's own `PutObject`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::crawler_runs::{
    already_past, emulator_with_s3, far_future, given_crawler_over, given_database,
    set_due_instant, stage, DATABASE,
};
use crate::wire::{ok, table_names};

/// The bucket the crawled data sits in.
const BUCKET: &str = "glue-crawler-stop-scope-suite";

/// The modelled `CrawlerState` values this suite walks, written out here rather than read from
/// the crate's constants so the suite pins the wire values.
const READY: &str = "READY";
const RUNNING: &str = "RUNNING";
const STOPPING: &str = "STOPPING";

/// The modelled `LastCrawlStatus` values a crawl comes to rest with here.
const CANCELLED: &str = "CANCELLED";
const SUCCEEDED: &str = "SUCCEEDED";

/// The modelled `CrawlerHistoryState` values a listed crawl stands in: a crawl in flight, one
/// that was stopped, and one that ran to the end (`smithy_query shape=CrawlerHistoryState`).
const HISTORY_RUNNING: &str = "RUNNING";
const HISTORY_STOPPED: &str = "STOPPED";
const HISTORY_COMPLETED: &str = "COMPLETED";

/// The crawlers test rows put under way together, each over its own prefix so no two of them
/// write the same catalog table.
const CRAWLERS: [&str; 3] = ["crawler-a", "crawler-b", "crawler-c"];

/// The prefix, and so the table name, crawler `name` crawls.
fn prefix_of(name: &str) -> String {
    format!("rows_{}", name.rsplit('-').next().unwrap_or(name))
}

/// Stage one object behind each crawler's own prefix.
async fn stage_data(router: &MultiServiceRouter) {
    let objects: Vec<(String, String)> = CRAWLERS
        .iter()
        .map(|name| {
            (
                format!("{}/rows.csv", prefix_of(name)),
                "id,name\n1,ada\n".to_string(),
            )
        })
        .collect();
    let staged: Vec<(&str, &str)> = objects
        .iter()
        .map(|(key, body)| (key.as_str(), body.as_str()))
        .collect();
    stage(router, BUCKET, &staged).await;
}

/// Create one crawler over its own prefix in the shared database.
async fn given_crawler(router: &MultiServiceRouter, name: &str) {
    given_crawler_over(
        router,
        name,
        json!([{ "Path": format!("s3://{BUCKET}/{}/", prefix_of(name)) }]),
        json!({}),
    )
    .await;
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn reported(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetCrawler", &json!({ "Name": name })).await["Crawler"].clone()
}

/// The `Crawler` projection of `name` as EVERY crawler reader reports it, each tagged with the
/// operation that answered it: the single read, the listing, and the batch read. What they
/// report must be one record, not three.
async fn projections(router: &MultiServiceRouter, name: &str) -> Vec<(&'static str, Value)> {
    let single = reported(router, name).await;

    let listed = ok(router, "GetCrawlers", &json!({}))
        .await
        .get("Crawlers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetCrawlers must report a Crawlers list"))
        .iter()
        .find(|crawler| crawler.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawlers must report the crawler {name}"));

    let batched = ok(
        router,
        "BatchGetCrawlers",
        &json!({ "CrawlerNames": [name] }),
    )
    .await
    .pointer("/Crawlers/0")
    .cloned()
    .unwrap_or_else(|| panic!("BatchGetCrawlers must report the crawler {name}"));

    vec![
        ("GetCrawler", single),
        ("GetCrawlers", listed),
        ("BatchGetCrawlers", batched),
    ]
}

/// The `CrawlerHistoryState` of every crawl `ListCrawls` reports for `name`, newest first.
async fn listed_crawl_states(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    ok(router, "ListCrawls", &json!({ "CrawlerName": name }))
        .await
        .get("Crawls")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListCrawls must report a Crawls list for {name}"))
        .iter()
        .map(|crawl| {
            crawl
                .get("State")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every listed crawl carries a State, got {crawl}"))
                .to_string()
        })
        .collect()
}

/// The `CrawlerMetrics` `GetCrawlerMetrics` reports for `name`.
async fn metrics(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "GetCrawlerMetrics",
        &json!({ "CrawlerNameList": [name] }),
    )
    .await
    .pointer("/CrawlerMetricsList/0")
    .cloned()
    .unwrap_or_else(|| panic!("GetCrawlerMetrics must report metrics for {name}"))
}

/// A stop ends the crawl of the crawler it NAMES and no other: the crawlers left alone stay
/// under way and their crawls run to the end, while the one named comes to rest cancelled.
///
/// The rows differ along which of the crawls under way the stop names.
#[tokio::test]
async fn a_stop_ends_only_the_crawl_of_the_crawler_it_names() {
    for stopped in CRAWLERS {
        let (router, engine) = emulator_with_s3();
        stage_data(&router).await;
        given_database(&router, DATABASE).await;
        for name in CRAWLERS {
            given_crawler(&router, name).await;
        }
        for name in CRAWLERS {
            ok(&router, "StartCrawler", &json!({ "Name": name })).await;
        }
        // Hold every transient window open, so three crawls are under way when the stop lands.
        set_due_instant(&engine, far_future());

        ok(&router, "StopCrawler", &json!({ "Name": stopped })).await;
        // The stop owes this crawler its own way back: hold that window open too.
        set_due_instant(&engine, far_future());

        for name in CRAWLERS {
            let crawler = reported(&router, name).await;
            let expected = if name == stopped { STOPPING } else { RUNNING };
            assert_eq!(
                crawler["State"],
                json!(expected),
                "case {stopped:?}: expected {name} to be {expected}, got {crawler}"
            );
            let listed = listed_crawl_states(&router, name).await;
            assert_eq!(
                listed,
                vec![HISTORY_RUNNING.to_string()],
                "case {stopped:?}: {name} still has its crawl in flight, got {listed:?}"
            );
        }

        // Every owed finish falls due: each crawler is back at rest, reporting the outcome of
        // its own crawl.
        set_due_instant(&engine, already_past());
        for name in CRAWLERS {
            let crawler = reported(&router, name).await;
            assert_eq!(
                crawler["State"],
                json!(READY),
                "case {stopped:?}: {name} comes to rest READY, got {crawler}"
            );
            let (status, history) = if name == stopped {
                (CANCELLED, HISTORY_STOPPED)
            } else {
                (SUCCEEDED, HISTORY_COMPLETED)
            };
            assert_eq!(
                crawler["LastCrawl"]["Status"],
                json!(status),
                "case {stopped:?}: expected {name}'s crawl to be {status}, got {crawler}"
            );
            let listed = listed_crawl_states(&router, name).await;
            assert_eq!(
                listed,
                vec![history.to_string()],
                "case {stopped:?}: expected {name}'s listed crawl to be {history}, got {listed:?}"
            );
        }
    }
}

/// Every reader of a stopped crawler reports the SAME record: the crawler stopping while the
/// clock still owes it its finish, then at rest with the crawl it ended reported cancelled —
/// and reading it, however often, moves nothing.
///
/// The rows differ along which reader is asked (`GetCrawler`, `GetCrawlers`,
/// `BatchGetCrawlers`), and the listing and the metrics are read beside them, so no surface
/// reports a crawl the others do not.
#[tokio::test]
async fn every_crawler_reader_reports_one_stopped_crawl() {
    let (router, engine) = emulator_with_s3();
    stage_data(&router).await;
    given_database(&router, DATABASE).await;
    let name = CRAWLERS[0];
    given_crawler(&router, name).await;

    ok(&router, "StartCrawler", &json!({ "Name": name })).await;
    set_due_instant(&engine, far_future());
    ok(&router, "StopCrawler", &json!({ "Name": name })).await;
    set_due_instant(&engine, far_future());

    // Read after read, the crawler stays where the stop put it: a read reports, it does not
    // finish what the clock has not made due.
    for round in 1..=3 {
        for (reader, crawler) in projections(&router, name).await {
            assert_eq!(
                crawler["State"],
                json!(STOPPING),
                "round {round} case {reader:?}: expected {STOPPING}, got {crawler}"
            );
            assert!(
                crawler.get("LastCrawl").is_none(),
                "round {round} case {reader:?}: a crawl still coming to rest is reported by no \
                 LastCrawl, got {crawler}"
            );
        }
    }
    let listed = listed_crawl_states(&router, name).await;
    assert_eq!(
        listed,
        vec![HISTORY_RUNNING.to_string()],
        "the crawl asked to stop has not ended yet, got {listed:?}"
    );
    let in_flight = metrics(&router, name).await;
    assert_eq!(
        in_flight["StillEstimating"],
        json!(true),
        "a crawl still coming to rest is a crawl in progress, got {in_flight}"
    );

    // The finish the clock owed falls due.
    set_due_instant(&engine, already_past());
    for (reader, crawler) in projections(&router, name).await {
        assert_eq!(
            crawler["State"],
            json!(READY),
            "case {reader:?}: expected {READY}, got {crawler}"
        );
        assert_eq!(
            crawler["LastCrawl"]["Status"],
            json!(CANCELLED),
            "case {reader:?}: the crawl that was stopped is reported {CANCELLED}, got {crawler}"
        );
    }
    let listed = listed_crawl_states(&router, name).await;
    assert_eq!(
        listed,
        vec![HISTORY_STOPPED.to_string()],
        "the crawl that was stopped is listed {HISTORY_STOPPED}, got {listed:?}"
    );

    let at_rest = metrics(&router, name).await;
    assert_eq!(
        at_rest["StillEstimating"],
        json!(false),
        "a crawler back at rest has no crawl to estimate, got {at_rest}"
    );
    // A stop ENDS the crawl, it does not undo it: the table the crawl wrote is still in the
    // catalog, and the metrics count exactly the tables that are there.
    let tables = ok(&router, "GetTables", &json!({ "DatabaseName": DATABASE })).await;
    let written = table_names(&tables);
    assert_eq!(
        written,
        vec![prefix_of(name)],
        "the table the stopped crawl wrote survives the stop, got {written:?}"
    );
    assert_eq!(
        at_rest["TablesCreated"],
        json!(written.len()),
        "the metrics count the tables the stopped crawl wrote, got {at_rest}"
    );
}

/// A stopped crawler is a crawler at rest like any other: it crawls again, is stopped again,
/// and each crawl is recorded with the outcome it came to rest with — newest first.
///
/// The rows differ along whether the cycle's crawl was stopped or left to run to the end, so
/// the history has to distinguish the two.
#[tokio::test]
async fn a_stopped_crawler_crawls_and_stops_again() {
    let (router, engine) = emulator_with_s3();
    stage_data(&router).await;
    given_database(&router, DATABASE).await;
    let name = CRAWLERS[0];
    given_crawler(&router, name).await;

    // Whether each successive crawl of this one crawler is stopped.
    let cycles = [true, false, true, true];
    let mut expected: Vec<String> = Vec::new();
    for (index, stop) in cycles.iter().enumerate() {
        ok(&router, "StartCrawler", &json!({ "Name": name })).await;
        if *stop {
            // Hold the crawl under way open, so there is a crawl for the stop to end.
            set_due_instant(&engine, far_future());
            ok(&router, "StopCrawler", &json!({ "Name": name })).await;
        }
        set_due_instant(&engine, already_past());

        let crawler = reported(&router, name).await;
        assert_eq!(
            crawler["State"],
            json!(READY),
            "cycle {index}: the crawler comes back to rest, got {crawler}"
        );
        let status = if *stop { CANCELLED } else { SUCCEEDED };
        assert_eq!(
            crawler["LastCrawl"]["Status"],
            json!(status),
            "cycle {index}: expected the crawl to be {status}, got {crawler}"
        );

        expected.insert(
            0,
            if *stop { HISTORY_STOPPED } else { HISTORY_COMPLETED }.to_string(),
        );
        let listed = listed_crawl_states(&router, name).await;
        assert_eq!(
            listed, expected,
            "cycle {index}: expected the crawls {expected:?}, got {listed:?}"
        );

        // Neither a stop nor a restart writes a second table for the one dataset behind the
        // crawler's target.
        let tables = ok(&router, "GetTables", &json!({ "DatabaseName": DATABASE })).await;
        let written = table_names(&tables);
        assert_eq!(
            written,
            vec![prefix_of(name)],
            "cycle {index}: the dataset is one table however often it is crawled, got {written:?}"
        );
    }
}
