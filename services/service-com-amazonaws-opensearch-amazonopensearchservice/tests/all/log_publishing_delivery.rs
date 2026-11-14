// SPDX-License-Identifier: CC0-1.0

//! What a domain's enabled `LogPublishingOptions` DELIVER into the CloudWatch Logs log
//! groups they name, driven over the real wire path: the domain and its log publishing are
//! created through this service's own `CreateDomain`, the work is done through signed
//! requests to the endpoint `DescribeDomain` published, and the records are read back
//! through the CloudWatch Logs service's OWN `FilterLogEvents` and `DescribeLogStreams` on
//! the same shared engine — so what is asserted is what a client of the log group sees.
//!
//! The rule under test, asserted over a table rather than one call: a domain publishes into
//! the log group an ENABLED `LogPublishingOption` names the records that log type is the log
//! OF — a slow-log record for a read or a write whose own duration reaches the threshold the
//! INDEX's settings state for that log type, and the cluster's application-log line for a
//! change to the indexes it holds — and nothing else; a log type the domain does not publish
//! and a threshold the operation does not reach leave the group untouched.
//!
//! The rows differ along what the rule generalises over: which log type carries the work,
//! which operation does it, which threshold setting and which LEVEL of it is set, the
//! threshold's value and the unit it is written in (including the two forms that disable a
//! level), and whether the work reaches it.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    baseline_create, log_group_arn, Emulator, DEFAULT_ACCESS_KEY_ID, PREFIX, SETTLE_WAIT,
};
use super::domain_rest_data_plane::{endpoint_of, rest};

/// The log group the domain publishes its INDEX_SLOW_LOGS to.
const INDEX_SLOW_GROUP: &str = "/aws/opensearch/domains/index-slow";

/// The log group it publishes its SEARCH_SLOW_LOGS to.
const SEARCH_SLOW_GROUP: &str = "/aws/opensearch/domains/search-slow";

/// The log group it publishes its ES_APPLICATION_LOGS to.
const APPLICATION_GROUP: &str = "/aws/opensearch/domains/application";

/// A group nothing publishes to, so a row can show that a log type the domain does not
/// publish delivers nowhere.
const UNPUBLISHED_GROUP: &str = "/aws/opensearch/domains/unpublished";

/// The index every row reads and writes.
const INDEX: &str = "logged";

/// The indexing slow-log threshold setting, as the engine names it below `index.`.
const INDEX_THRESHOLD: &str = "index.indexing.slowlog.threshold.index";

/// The search slow-log threshold setting for the query phase.
const SEARCH_THRESHOLD: &str = "index.search.slowlog.threshold.query";

/// The messages `FilterLogEvents` reports for the whole of `group`, in the order it pages
/// them.
///
/// `pub(crate)` because a sibling suite reads a published record the same way rather than
/// growing a second copy of this.
pub(crate) async fn events(emulator: &Emulator, group: &str) -> Vec<String> {
    let answer = emulator
        .logs_call("FilterLogEvents", json!({ "logGroupName": group }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case FilterLogEvents({group:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    event["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The stream names `DescribeLogStreams` reports for `group`.
async fn streams(emulator: &Emulator, group: &str) -> Vec<String> {
    let answer = emulator
        .logs_call("DescribeLogStreams", json!({ "logGroupName": group }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeLogStreams({group:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["logStreams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .map(|stream| {
                    stream["logStreamName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One emulator with the four groups above created through CloudWatch Logs itself and a
/// domain publishing three of the log types, each to its own group, with one index created
/// on it.
async fn with_publishing(domain: &str) -> (Emulator, String) {
    let emulator = Emulator::with_log_groups();
    for group in [
        INDEX_SLOW_GROUP,
        SEARCH_SLOW_GROUP,
        APPLICATION_GROUP,
        UNPUBLISHED_GROUP,
    ] {
        emulator.created_log_group(group).await;
    }
    let mut body = baseline_create(domain);
    // The data-plane work below is signed by this emulator's own credential, so the
    // domain's access policy has to admit it: the baseline body carries a policy with no
    // statement at all, which admits nothing.
    body["AccessPolicies"] = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Principal": { "AWS": "*" }, "Action": "es:ESHttp*", "Resource": "*" }],
    })
    .to_string()
    .into();
    body["LogPublishingOptions"] = json!({
        "INDEX_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(INDEX_SLOW_GROUP),
        },
        "SEARCH_SLOW_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(SEARCH_SLOW_GROUP),
        },
        "ES_APPLICATION_LOGS": {
            "Enabled": true,
            "CloudWatchLogsLogGroupArn": log_group_arn(APPLICATION_GROUP),
        },
        // A DISABLED option publishes nothing, so the group it names stays empty however
        // much work the domain does.
        "AUDIT_LOGS": {
            "Enabled": false,
            "CloudWatchLogsLogGroupArn": log_group_arn(UNPUBLISHED_GROUP),
        },
    });
    let created = emulator.create(body).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({domain:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, domain);
    (emulator, host)
}

/// Create `INDEX` on the domain at `host`.
async fn created_index(emulator: &Emulator, host: &str) {
    let created = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "PUT",
        &format!("/{INDEX}"),
        Some(json!({ "settings": { "number_of_shards": 2, "number_of_replicas": 1 } })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case PUT /{INDEX}: expected 200, got {} with {:?}",
        created.status, created.body
    );
}

/// Write the settings `body` on `INDEX`, asserting the update was taken.
async fn given_settings(emulator: &Emulator, host: &str, case: &str, body: Value) {
    let answer = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "PUT",
        &format!("/{INDEX}/_settings"),
        Some(body),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the settings update to be taken, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn each_log_type_delivers_its_own_records_to_the_group_its_option_names() {
    let domain = "logs-delivery";
    let (emulator, host) = with_publishing(domain).await;

    // An index the cluster did not hold before is a change to the cluster's own state,
    // which its APPLICATION log states — and is no slow-log record, so the slow-log
    // groups stay empty.
    created_index(&emulator, &host).await;
    let application = events(&emulator, APPLICATION_GROUP).await;
    assert!(
        application
            .iter()
            .any(|message| message.contains("creating index") && message.contains(INDEX)),
        "case \"an index created\": expected the application log to state the create, got \
         {application:?}"
    );
    for group in [INDEX_SLOW_GROUP, SEARCH_SLOW_GROUP, UNPUBLISHED_GROUP] {
        assert!(
            events(&emulator, group).await.is_empty(),
            "case \"an index created\": expected {group} to carry no record, got {:?}",
            events(&emulator, group).await
        );
    }

    // With both thresholds set to a duration every operation reaches, the write goes to
    // the indexing group and the search goes to the search group — each to the group its
    // own log type names and to no other.
    given_settings(
        &emulator,
        &host,
        "thresholds every operation reaches",
        json!({
            format!("{INDEX_THRESHOLD}.warn"): "0ms",
            format!("{SEARCH_THRESHOLD}.warn"): "0ms",
        }),
    )
    .await;

    let written = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        &format!("/{INDEX}/_doc/first"),
        Some(json!({ "title": "a slow document" })),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "case \"a document written\": expected 201, got {} with {:?}",
        written.status, written.body
    );
    let indexing = events(&emulator, INDEX_SLOW_GROUP).await;
    assert!(
        indexing.iter().any(|message| {
            message.contains("index.indexing.slowlog.index")
                && message.contains(INDEX)
                && message.contains("id[first]")
                && message.contains("took_millis[")
        }),
        "case \"a document written\": expected the indexing slow log to state the write, got \
         {indexing:?}"
    );
    assert!(
        events(&emulator, SEARCH_SLOW_GROUP).await.is_empty(),
        "case \"a document written\": expected the search group to carry no record of a write"
    );

    let searched = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        &format!("/{INDEX}/_search"),
        Some(json!({ "query": { "match_all": {} } })),
    )
    .await;
    assert_eq!(
        searched.status, 200,
        "case \"a search\": expected 200, got {} with {:?}",
        searched.status, searched.body
    );
    let search = events(&emulator, SEARCH_SLOW_GROUP).await;
    assert!(
        search.iter().any(|message| {
            message.contains("index.search.slowlog.query")
                && message.contains(INDEX)
                && message.contains("took_millis[")
        }),
        "case \"a search\": expected the search slow log to state the read, got {search:?}"
    );
    assert_eq!(
        indexing.len(),
        events(&emulator, INDEX_SLOW_GROUP).await.len(),
        "case \"a search\": expected the indexing group to carry no record of a search"
    );
    // A disabled option delivers nothing, whatever work the domain does.
    assert!(
        events(&emulator, UNPUBLISHED_GROUP).await.is_empty(),
        "case \"a disabled option\": expected its group to stay empty, got {:?}",
        events(&emulator, UNPUBLISHED_GROUP).await
    );

    // A record comes from a NODE of the domain's cluster, so the stream it is written to
    // is one the control plane reports as a node of that domain.
    tokio::time::sleep(SETTLE_WAIT).await;
    let nodes = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/nodes"),
            None,
        )
        .await;
    let reported: Vec<String> = nodes.body["DomainNodesStatusList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|node| node["NodeId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    for group in [INDEX_SLOW_GROUP, SEARCH_SLOW_GROUP, APPLICATION_GROUP] {
        for stream in streams(&emulator, group).await {
            assert!(
                reported.contains(&stream),
                "case \"the stream a record is written to\": expected {stream:?} in {group} to \
                 be a node of the domain, got {reported:?}"
            );
        }
    }
}

/// One threshold row: the value the level carries, the level it is set on, and whether a
/// write of that index is recorded with it in force.
struct Threshold {
    case: &'static str,
    /// The level the row sets, and the value it sets it to.
    level: &'static str,
    value: Value,
    /// Whether the write that follows produces a record.
    records: bool,
}

/// The thresholds a row may set, so each row turns the levels it does not name OFF rather
/// than inheriting the previous row's.
const LEVELS: [&str; 4] = ["warn", "info", "debug", "trace"];

#[tokio::test]
async fn a_slow_log_record_is_published_only_for_work_that_reaches_the_index_threshold() {
    let domain = "logs-thresholds";
    let (emulator, host) = with_publishing(domain).await;
    created_index(&emulator, &host).await;

    // The values span the engine's time-value units and both forms that turn a level off
    // (its documented `-1` default, and no value at all), plus a duration no in-memory
    // write reaches.
    let rows: &[Threshold] = &[
        Threshold {
            case: "zero milliseconds on warn",
            level: "warn",
            value: json!("0ms"),
            records: true,
        },
        Threshold {
            case: "zero seconds on warn",
            level: "warn",
            value: json!("0s"),
            records: true,
        },
        Threshold {
            case: "zero microseconds on info",
            level: "info",
            value: json!("0micros"),
            records: true,
        },
        Threshold {
            case: "a bare zero, which the engine reads as milliseconds, on trace",
            level: "trace",
            value: json!("0"),
            records: true,
        },
        Threshold {
            case: "the engine's own disabled default",
            level: "warn",
            value: json!("-1"),
            records: false,
        },
        Threshold {
            case: "a negative duration",
            level: "warn",
            value: json!("-1s"),
            records: false,
        },
        Threshold {
            case: "a duration no write reaches",
            level: "warn",
            value: json!("1h"),
            records: false,
        },
        Threshold {
            case: "thirty seconds",
            level: "warn",
            value: json!("30s"),
            records: false,
        },
        Threshold {
            case: "no threshold at all",
            level: "warn",
            value: Value::Null,
            records: false,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        // Every level the row does not name is reset, so what decides the outcome is the
        // one value under test.
        let mut body = ::serde_json::Map::new();
        for level in LEVELS {
            let value = if level == row.level {
                row.value.clone()
            } else {
                Value::Null
            };
            body.insert(format!("{INDEX_THRESHOLD}.{level}"), value);
        }
        given_settings(&emulator, &host, row.case, Value::Object(body)).await;

        let before = events(&emulator, INDEX_SLOW_GROUP).await.len();
        let written = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/{INDEX}/_doc/{position}"),
            Some(json!({ "title": "a document" })),
        )
        .await;
        assert_eq!(
            written.status, 201,
            "case {}: expected the write to be taken, got {} with {:?}",
            row.case, written.status, written.body
        );
        let after = events(&emulator, INDEX_SLOW_GROUP).await;
        let produced = after.len() - before;
        assert_eq!(
            produced,
            usize::from(row.records),
            "case {}: expected {} record(s) from the write, got {:?}",
            row.case,
            usize::from(row.records),
            &after[before..]
        );
        // A record is stated at the level whose threshold the work reached.
        if row.records {
            let stated = after.last().cloned().unwrap_or_default();
            assert!(
                stated.contains(&row.level.to_uppercase()),
                "case {}: expected the record to be stated at {}, got {stated:?}",
                row.case,
                row.level.to_uppercase()
            );
        }
    }
}
