// SPDX-License-Identifier: CC0-1.0

//! `GetTopPathStatisticsByTraffic`, end to end over the wire.
//!
//! The operation "retrieves aggregated statistics about the top URI paths accessed
//! by bot traffic for a specified web ACL and time window", so the RULE these suites
//! assert is: the answer is an aggregation of the requests recorded for the web ACL
//! the ARN names — the requests inside the window that match the request's bot
//! filters and path prefix, grouped by path, ordered by request count, each entry
//! carrying its share of the matching total and at most
//! `NumberOfTopTrafficBotsPerPath` of the busiest bots that asked for it, cut into
//! pages of at most `Limit` entries — and the call REPORTS: it changes nothing a
//! later caller sees.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: both scopes, windows that cover all, part and none of the log, each bot
//! filter alone and in combination, grouping with and without a `UriPathPrefix`,
//! every page size, and each ARN failure.
//!
//! WAF appends to a web ACL's request log in its DATA plane, which this emulator
//! does not serve, so no operation writes that log: the aggregation cases seed it
//! through the state API ([`record_traffic`]) and then drive every assertion through
//! the router's front door, the same door an SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    BotRequestRecord, ComAmazonawsWafv2Awswaf20190729State, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, Waf};

/// The operation under test, named once.
const OPERATION: &str = "GetTopPathStatisticsByTraffic";

/// The instant every window and every recorded request below is expressed relative
/// to (epoch seconds for 2026-01-01T00:00:00Z), so the whole suite is deterministic
/// and never races a wall clock.
const BASE_SECS: i64 = 1_767_225_600;

/// A `TimeWindow` spanning `[BASE_SECS + from, BASE_SECS + to]`, in the
/// epoch-seconds form an awsJson1_1 body carries a timestamp in.
fn window(from: i64, to: i64) -> Value {
    json!({ "StartTime": BASE_SECS + from, "EndTime": BASE_SECS + to })
}

/// One recorded request, `at` seconds after [`BASE_SECS`].
fn record(
    at: i64,
    uri_path: &str,
    bot_name: &str,
    bot_category: &str,
    bot_organization: &str,
) -> BotRequestRecord {
    BotRequestRecord {
        observed_at_millis: (BASE_SECS + at) * 1_000,
        uri_path: uri_path.to_string(),
        bot_name: bot_name.to_string(),
        bot_category: bot_category.to_string(),
        bot_organization: bot_organization.to_string(),
    }
}

/// The traffic fixture the aggregation cases read: seven requests over three
/// top-level paths (`/api` four, `/docs` two, `/` one), sent by four bots in two
/// categories and four organizations, each at a known second inside the first
/// minute of the window.
///
/// The bot names, categories and organizations are the examples the model's own
/// filter docs give (`BotName` "gptbot or googlebot", `BotCategory` "ai …
/// search_engine", `BotOrganization` "openai or google").
fn traffic_fixture() -> Vec<BotRequestRecord> {
    vec![
        record(10, "/api/v1/users", "gptbot", "ai", "openai"),
        record(20, "/api/v2/orders", "gptbot", "ai", "openai"),
        record(30, "/api/v1/users", "claudebot", "ai", "anthropic"),
        record(40, "/api/v1/users", "googlebot", "search_engine", "google"),
        record(50, "/docs/intro", "gptbot", "ai", "openai"),
        record(60, "/docs/intro", "googlebot", "search_engine", "google"),
        record(70, "/", "bingbot", "search_engine", "microsoft"),
    ]
}

/// Append `records` to the request log of the web ACL `(scope, web_acl_id)`.
///
/// The one thing these suites cannot do over the wire: WAF's data plane is what
/// records a request against a web ACL, and this emulator serves only the WAFv2
/// control plane, so nothing in the API fills that log.
fn record_traffic(
    engine: &engine::InMemoryEngine,
    scope: &str,
    web_acl_id: &str,
    records: Vec<BotRequestRecord>,
) {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .bot_requests
        .entry((scope.to_string(), web_acl_id.to_string()))
        .or_default()
        .extend(records);
}

/// How many requests the log of `(scope, web_acl_id)` holds.
fn recorded_request_count(engine: &engine::InMemoryEngine, scope: &str, web_acl_id: &str) -> usize {
    engine
        .state::<ComAmazonawsWafv2Awswaf20190729State>()
        .lock()
        .bot_requests(scope, web_acl_id)
        .len()
}

/// The request body, carrying the four members the model marks `@required` beside
/// `WebAclArn`'s own.
fn request(arn: &str, scope: &str, time_window: Value, limit: i64, bots_per_path: i64) -> Value {
    json!({
        "WebAclArn": arn,
        "Scope": scope,
        "TimeWindow": time_window,
        "Limit": limit,
        "NumberOfTopTrafficBotsPerPath": bots_per_path,
    })
}

/// [`request`] with the members of `extra` merged in, for the optional filters.
fn with(mut body: Value, extra: &Value) -> Value {
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// The `(Path, RequestCount)` pairs of a `PathStatistics` list, in the order the
/// response carries them.
fn rows(statistics: &Value) -> Vec<(String, i64)> {
    statistics
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["Path"].as_str().unwrap_or_default().to_string(),
                        entry["RequestCount"].as_i64().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The bot names of one entry's `TopBots`, in the order the response carries them.
fn top_bot_names(entry: &Value) -> Vec<String> {
    entry["TopBots"]
        .as_array()
        .map(|bots| {
            bots.iter()
                .map(|bot| bot["BotName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every entry's `Percentage` must be its own `RequestCount` as a share of `whole`,
/// and the entries must be ordered by `RequestCount`, busiest first.
fn assert_shares_and_order(case: &str, statistics: &Value, whole: i64) {
    let entries = statistics.as_array().cloned().unwrap_or_default();
    let mut previous = i64::MAX;
    for entry in &entries {
        let count = entry["RequestCount"].as_i64().unwrap_or_default();
        let percentage = entry["Percentage"].as_f64().unwrap_or_default();
        let expected = if whole <= 0 {
            0.0
        } else {
            100.0 * count as f64 / whole as f64
        };
        assert!(
            (percentage - expected).abs() < 1e-9,
            "case {case:?}: entry {entry} must report {expected} percent of {whole}, \
             got {percentage}"
        );
        assert!(
            count <= previous,
            "case {case:?}: entries must be ordered by request count, got {statistics}"
        );
        previous = count;
    }
}

#[tokio::test]
async fn a_web_acl_that_evaluated_no_request_reports_zero_over_every_window() {
    let waf = Waf::new();
    // Both scopes, and a web ACL in each that has evaluated nothing: the answer is
    // the zeroed aggregation, not a refusal.
    for scope in [SCOPE_CLOUDFRONT, SCOPE_REGIONAL] {
        let name = format!("quiet-{}", scope.to_lowercase());
        let (_id, arn, _lock) = create_web_acl(&waf, scope, &name, json!([])).await;
        for (case, time_window) in [
            ("one hour forward", window(0, 3_600)),
            ("the three hours before", window(-10_800, 0)),
            ("one second", window(0, 1)),
        ] {
            let case = format!("{scope} / {case}");
            let answer = waf
                .ok(OPERATION, request(&arn, scope, time_window, 50, 5))
                .await;
            assert_eq!(
                answer["TotalRequestCount"],
                json!(0),
                "case {case:?}: a web ACL that evaluated nothing reports no requests, \
                 got {answer}"
            );
            assert_eq!(
                answer["PathStatistics"],
                json!([]),
                "case {case:?}: no requests means no path, got {answer}"
            );
            assert!(
                answer["NextMarker"].is_null(),
                "case {case:?}: an exhausted window carries no marker, got {answer}"
            );
            assert_eq!(
                answer["TopCategories"],
                json!([]),
                "case {case:?}: an unfiltered request populates TopCategories, and no \
                 request means no category, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn path_statistics_group_by_top_level_path_ordered_by_request_count() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "ordering-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    // The fixture's seven requests, grouped by their top-level path: `/api` holds
    // four of them, `/docs` two and `/` one.
    let expected = vec![
        ("/api".to_string(), 4_i64),
        ("/docs".to_string(), 2),
        ("/".to_string(), 1),
    ];
    let answer = waf
        .ok(
            OPERATION,
            request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
        )
        .await;
    assert_eq!(
        answer["TotalRequestCount"],
        json!(7),
        "case \"whole window\": every matching request is counted, got {answer}"
    );
    assert_eq!(
        rows(&answer["PathStatistics"]),
        expected,
        "case \"whole window\": expected {expected:?}, got {answer}"
    );
    assert_shares_and_order("whole window", &answer["PathStatistics"], 7);

    // `Source` names the filters the request applied, so an unfiltered request
    // carries none.
    for entry in answer["PathStatistics"].as_array().unwrap_or(&vec![]) {
        assert!(
            entry["Source"].is_null(),
            "case \"whole window\": an unfiltered entry carries no Source, got {entry}"
        );
    }

    // The busiest path's bots: `gptbot` sent two of its four requests, `claudebot`
    // and `googlebot` one each, so the first entry is `gptbot` and the tie behind it
    // resolves by name.
    let api = &answer["PathStatistics"][0];
    assert_eq!(
        top_bot_names(api),
        vec![
            "gptbot".to_string(),
            "claudebot".to_string(),
            "googlebot".to_string()
        ],
        "case \"whole window\": TopBots are ordered by request count, got {api}"
    );
    assert_shares_and_order("whole window / top bots", &api["TopBots"], 4);

    // "The maximum number of top bots to include in the statistics for each path":
    // every published value of `NumberOfTopTrafficBotsPerPath`'s range bounds the
    // list, and the bound keeps the busiest.
    for bots_per_path in 1..=10_i64 {
        let case = format!("{bots_per_path} bots per path");
        let answer = waf
            .ok(
                OPERATION,
                request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, bots_per_path),
            )
            .await;
        let bots = top_bot_names(&answer["PathStatistics"][0]);
        let expected_len = usize::try_from(bots_per_path).unwrap_or(0).min(3);
        assert_eq!(
            bots.len(),
            expected_len,
            "case {case:?}: expected {expected_len} bots, got {bots:?}"
        );
        assert_eq!(
            bots.first().map(String::as_str),
            Some("gptbot"),
            "case {case:?}: the busiest bot is kept first, got {bots:?}"
        );
    }
}

#[tokio::test]
async fn the_time_window_selects_the_requests_it_contains() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "window-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    // The fixture's requests sit at seconds 10, 20, 30, 40, 50, 60 and 70 of the
    // window, so a window is a filter over them: the whole minute, each half of it,
    // a gap that holds none, the single instant of one request, and an inverted
    // window, which holds no instant at all.
    for (case, time_window, expected_total, expected_rows) in [
        (
            "the whole first minute",
            window(0, 60),
            6_i64,
            vec![("/api".to_string(), 4_i64), ("/docs".to_string(), 2)],
        ),
        (
            "the first 45 seconds",
            window(0, 45),
            4,
            vec![("/api".to_string(), 4)],
        ),
        (
            "the second half",
            window(45, 120),
            3,
            vec![("/docs".to_string(), 2), ("/".to_string(), 1)],
        ),
        ("a gap with no request", window(100, 200), 0, vec![]),
        (
            "one instant",
            window(30, 30),
            1,
            vec![("/api".to_string(), 1)],
        ),
        ("an inverted window", window(3_600, 0), 0, vec![]),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                request(&arn, SCOPE_REGIONAL, time_window, 50, 10),
            )
            .await;
        assert_eq!(
            answer["TotalRequestCount"].as_i64(),
            Some(expected_total),
            "case {case:?}: expected {expected_total} requests, got {answer}"
        );
        assert_eq!(
            rows(&answer["PathStatistics"]),
            expected_rows,
            "case {case:?}: expected {expected_rows:?}, got {answer}"
        );
        assert_shares_and_order(case, &answer["PathStatistics"], expected_total);
    }
}

#[tokio::test]
async fn a_uri_path_prefix_reports_the_individual_uris_inside_it() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "prefix-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    // "If you specify /api, the response includes statistics for paths like
    // /api/v1/users and /api/v2/orders": inside a prefix the entries are the URIs
    // themselves, and only the requests inside the prefix are counted.
    for (case, prefix, expected_total, expected_rows) in [
        (
            "/api",
            "/api",
            4_i64,
            vec![
                ("/api/v1/users".to_string(), 3_i64),
                ("/api/v2/orders".to_string(), 1),
            ],
        ),
        (
            "/api/v1",
            "/api/v1",
            3,
            vec![("/api/v1/users".to_string(), 3)],
        ),
        (
            "/docs",
            "/docs",
            2,
            vec![("/docs/intro".to_string(), 2)],
        ),
        (
            "/",
            "/",
            7,
            vec![
                ("/api/v1/users".to_string(), 3),
                ("/".to_string(), 1),
                ("/api/v2/orders".to_string(), 1),
                ("/docs/intro".to_string(), 2),
            ],
        ),
        ("/nothing-here", "/nothing-here", 0, vec![]),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
                    &json!({ "UriPathPrefix": prefix }),
                ),
            )
            .await;
        assert_eq!(
            answer["TotalRequestCount"].as_i64(),
            Some(expected_total),
            "case {case:?}: expected {expected_total} requests inside the prefix, \
             got {answer}"
        );
        let reported = rows(&answer["PathStatistics"]);
        // The rows of the `/` case are written in path order above; the response
        // orders them by request count, which `assert_shares_and_order` checks.
        let mut sorted_reported = reported.clone();
        sorted_reported.sort();
        let mut sorted_expected = expected_rows.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_reported, sorted_expected,
            "case {case:?}: expected {sorted_expected:?}, got {answer}"
        );
        assert_shares_and_order(case, &answer["PathStatistics"], expected_total);
    }
}

#[tokio::test]
async fn a_bot_filter_selects_that_bot_s_requests_and_is_echoed_in_source() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "filtered-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    // Each filter the model declares, alone and combined, against the fixture: the
    // filter selects the requests whose own value matches it, and the entries report
    // it back in `Source`.
    for (case, filter, expected_total, expected_rows) in [
        (
            "BotCategory ai",
            json!({ "BotCategory": "ai" }),
            4_i64,
            vec![("/api".to_string(), 3_i64), ("/docs".to_string(), 1)],
        ),
        (
            "BotCategory search_engine",
            json!({ "BotCategory": "search_engine" }),
            3,
            vec![
                ("/".to_string(), 1),
                ("/api".to_string(), 1),
                ("/docs".to_string(), 1),
            ],
        ),
        (
            "BotOrganization openai",
            json!({ "BotOrganization": "openai" }),
            3,
            vec![("/api".to_string(), 2), ("/docs".to_string(), 1)],
        ),
        (
            "BotName googlebot",
            json!({ "BotName": "googlebot" }),
            2,
            vec![("/api".to_string(), 1), ("/docs".to_string(), 1)],
        ),
        (
            "BotCategory ai and BotName gptbot",
            json!({ "BotCategory": "ai", "BotName": "gptbot" }),
            3,
            vec![("/api".to_string(), 2), ("/docs".to_string(), 1)],
        ),
        (
            "BotName with no traffic",
            json!({ "BotName": "no-such-bot" }),
            0,
            vec![],
        ),
    ] {
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
                    &filter,
                ),
            )
            .await;
        assert_eq!(
            answer["TotalRequestCount"].as_i64(),
            Some(expected_total),
            "case {case:?}: expected {expected_total} matching requests, got {answer}"
        );
        let mut reported = rows(&answer["PathStatistics"]);
        reported.sort();
        let mut expected = expected_rows.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {answer}"
        );
        assert_shares_and_order(case, &answer["PathStatistics"], expected_total);
        for entry in answer["PathStatistics"].as_array().unwrap_or(&vec![]) {
            assert_eq!(
                entry["Source"],
                filter,
                "case {case:?}: Source reports the filters the request applied, \
                 got {entry}"
            );
        }
        // "This field is only populated when no bot filters are applied to the
        // request."
        assert!(
            answer["TopCategories"].is_null(),
            "case {case:?}: a filtered request reports no TopCategories, got {answer}"
        );
    }
}

#[tokio::test]
async fn top_categories_aggregate_the_window_per_bot_category() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "categories-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    let answer = waf
        .ok(
            OPERATION,
            request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
        )
        .await;
    // One entry per category seen in the window: `ai` sent four of the seven
    // requests and `search_engine` three, and each entry names its category in
    // `Source` and its busiest path in `Path`.
    let categories: Vec<(String, String, i64)> = answer["TopCategories"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Source"]["BotCategory"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                entry["Path"].as_str().unwrap_or_default().to_string(),
                entry["RequestCount"].as_i64().unwrap_or_default(),
            )
        })
        .collect();
    // `search_engine`'s three requests went to three different paths, one each, so
    // its busiest path is a tie that the path itself breaks — the same way two
    // equally busy paths are ordered above.
    let expected = vec![
        ("ai".to_string(), "/api".to_string(), 4_i64),
        ("search_engine".to_string(), "/".to_string(), 3),
    ];
    assert_eq!(
        categories, expected,
        "case \"unfiltered window\": expected {expected:?}, got {answer}"
    );
    assert_shares_and_order("unfiltered window / categories", &answer["TopCategories"], 7);
}

#[tokio::test]
async fn limit_pages_the_window_and_next_marker_resumes_after_the_last_path() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "paging-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());

    let whole_window = vec![
        ("/api".to_string(), 4_i64),
        ("/docs".to_string(), 2),
        ("/".to_string(), 1),
    ];
    // Every page size from one entry to one more than the window holds: the pages
    // walk the one order, each page holds at most `Limit` entries, the last page
    // carries no marker, and every page reports the window's own total.
    for page_size in 1..=4_i64 {
        let case = format!("page size {page_size}");
        let mut seen: Vec<(String, i64)> = Vec::new();
        let mut marker: Option<String> = None;
        for _ in 0..=whole_window.len() {
            let mut body = request(&arn, SCOPE_REGIONAL, window(0, 3_600), page_size, 10);
            if let Some(marker) = &marker {
                body = with(body, &json!({ "NextMarker": marker }));
            }
            let answer = waf.ok(OPERATION, body).await;
            assert_eq!(
                answer["TotalRequestCount"].as_i64(),
                Some(7),
                "case {case:?}: every page reports the window's total, got {answer}"
            );
            let page = rows(&answer["PathStatistics"]);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(0),
                "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
            );
            seen.extend(page);
            match answer["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, whole_window,
            "case {case:?}: the pages must walk the whole window in one order, got {seen:?}"
        );
    }

    // A marker naming no path in the window names no position to resume from.
    let answer = waf
        .ok(
            OPERATION,
            with(
                request(&arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
                &json!({ "NextMarker": "/not-a-reported-path" }),
            ),
        )
        .await;
    assert_eq!(
        answer["PathStatistics"],
        json!([]),
        "case \"unknown marker\": nothing follows a position the window does not hold, \
         got {answer}"
    );
}

#[tokio::test]
async fn a_bot_filtered_cloudfront_view_is_refused_as_a_pricing_plan_feature() {
    let (waf, engine) = Waf::sharing_engine();
    let (cloudfront_id, cloudfront_arn, _lock) =
        create_web_acl(&waf, SCOPE_CLOUDFRONT, "plan-acl", json!([])).await;
    let (regional_id, regional_arn, _lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "plan-acl", json!([])).await;
    record_traffic(&engine, SCOPE_CLOUDFRONT, &cloudfront_id, traffic_fixture());
    record_traffic(&engine, SCOPE_REGIONAL, &regional_id, traffic_fixture());

    // Each bot filter, alone and combined: for a CLOUDFRONT web ACL the bot-level
    // view is a CloudFront pricing-plan feature and is refused, while the same
    // filter over the same traffic in the REGIONAL namespace — which no CloudFront
    // pricing plan gates — is served.
    for (case, filter) in [
        ("BotCategory", json!({ "BotCategory": "ai" })),
        ("BotOrganization", json!({ "BotOrganization": "openai" })),
        ("BotName", json!({ "BotName": "gptbot" })),
        (
            "every filter at once",
            json!({ "BotCategory": "ai", "BotOrganization": "openai", "BotName": "gptbot" }),
        ),
    ] {
        waf.refused(
            OPERATION,
            with(
                request(&cloudfront_arn, SCOPE_CLOUDFRONT, window(0, 3_600), 50, 10),
                &filter,
            ),
            "WAFFeatureNotIncludedInPricingPlanException",
            case,
        )
        .await;
        let answer = waf
            .ok(
                OPERATION,
                with(
                    request(&regional_arn, SCOPE_REGIONAL, window(0, 3_600), 50, 10),
                    &filter,
                ),
            )
            .await;
        assert!(
            answer["TotalRequestCount"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: the REGIONAL namespace serves the filtered view, got {answer}"
        );
    }

    // The unfiltered view of the same CLOUDFRONT web ACL is served, and it is the
    // aggregation of that web ACL's own log.
    let answer = waf
        .ok(
            OPERATION,
            request(&cloudfront_arn, SCOPE_CLOUDFRONT, window(0, 3_600), 50, 10),
        )
        .await;
    assert_eq!(
        rows(&answer["PathStatistics"]),
        vec![
            ("/api".to_string(), 4_i64),
            ("/docs".to_string(), 2),
            ("/".to_string(), 1)
        ],
        "case \"unfiltered CLOUDFRONT\": the unfiltered view is served, got {answer}"
    );
}

/// An ARN WAF will not accept as a web ACL ARN of the namespace asked about is
/// refused; an ARN that is well formed, of that namespace, and names no web ACL in it
/// simply selects no recorded request, which is an empty result.
///
/// The refused rows differ in HOW the ARN is not one: no ARN at all, a wafv2 ARN of
/// another resource kind, and a web ACL ARN of the OTHER namespace — the last one
/// because a wafv2 ARN carries its namespace in its resource part, so an ARN of the
/// regional namespace is not an ARN this request can use when it asks about the
/// global one.
///
/// The second half is what separates a traffic QUERY from a resource read: "the
/// total number of requests that match the query criteria within the specified time
/// window" is 0 when nothing matches, and `TotalRequestCount` is `@default 0` with
/// `PathStatistics` `@required`, so a well-formed query always has an answer.
#[tokio::test]
async fn an_arn_waf_cannot_read_as_a_web_acl_arn_is_refused_and_one_naming_nothing_is_empty() {
    let waf = Waf::new();
    let (_id, regional_arn, _lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "addressed-acl", json!([])).await;
    let (_global_id, global_arn, _global_lock) =
        create_web_acl(&waf, SCOPE_CLOUDFRONT, "addressed-acl", json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_REGIONAL, "addressed-ip-set").await;
    // A well-formed web ACL ARN naming an id nothing was created under.
    let unknown_arn = format!("{regional_arn}-unknown");

    for (case, arn, scope) in [
        (
            "a value that is no ARN",
            "arn:aws:wafv2:not-an-arn-at-all".to_string(),
            SCOPE_REGIONAL,
        ),
        (
            "an ARN of another resource kind",
            ip_set_arn,
            SCOPE_REGIONAL,
        ),
        (
            "a REGIONAL web ACL asked for in the CLOUDFRONT namespace",
            regional_arn.clone(),
            SCOPE_CLOUDFRONT,
        ),
        (
            "a CLOUDFRONT web ACL asked for in the REGIONAL namespace",
            global_arn,
            SCOPE_REGIONAL,
        ),
    ] {
        let body = waf
            .refused(
                OPERATION,
                request(&arn, scope, window(0, 3_600), 50, 10),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            (body["Field"].as_str(), body["Parameter"].as_str()),
            (Some("RESOURCE_ARN"), Some(arn.as_str())),
            "case {case:?}: the refusal must name the ARN it rejected, got {body}"
        );
    }

    // No traffic was recorded for this one, and the ARN is of the namespace asked
    // about, so it answers the empty result rather than a refusal: it names no web
    // ACL at all.
    for (case, arn, scope) in [("a web ACL ARN naming nothing", unknown_arn, SCOPE_REGIONAL)] {
        let answer = waf
            .ok(OPERATION, request(&arn, scope, window(0, 3_600), 50, 10))
            .await;
        assert_eq!(
            (
                answer["PathStatistics"].as_array().map(Vec::len),
                answer["TotalRequestCount"].as_i64()
            ),
            (Some(0), Some(0)),
            "case {case:?}: expected an empty PathStatistics list and TotalRequestCount \
             0, got {answer}"
        );
    }
}

#[tokio::test]
async fn the_statistics_call_reports_and_changes_nothing_a_later_caller_sees() {
    let (waf, engine) = Waf::sharing_engine();
    let (id, arn, _lock) = create_web_acl(&waf, SCOPE_REGIONAL, "reporting-acl", json!([])).await;
    record_traffic(&engine, SCOPE_REGIONAL, &id, traffic_fixture());
    let before = recorded_request_count(&engine, SCOPE_REGIONAL, &id);

    // The same request, repeated: a read neither consumes the log nor changes what
    // the next caller is told.
    let body = request(&arn, SCOPE_REGIONAL, window(0, 3_600), 2, 3);
    let first = waf.ok(OPERATION, body.clone()).await;
    let second = waf.ok(OPERATION, body).await;
    assert_eq!(
        first, second,
        "case \"repeated read\": two identical reads must answer identically, got \
         {first} then {second}"
    );
    assert_eq!(
        recorded_request_count(&engine, SCOPE_REGIONAL, &id),
        before,
        "case \"repeated read\": a read must not touch the recorded requests"
    );
}
