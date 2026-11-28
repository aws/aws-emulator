// SPDX-License-Identifier: CC0-1.0

//! Paging `GetAppMonitorData` while the buffer it reads keeps changing.
//!
//! The rule under test: a page of a paginated read resumes strictly after the KEY of the
//! last entry the previous page returned — here the rank the data plane appended that event
//! at — and is answered against the query its token was issued for, so each entry matching
//! that query is reported exactly once across its pages however the collection changes in
//! between, and a token presented with another query is refused rather than read as a
//! position in it.
//!
//! Each table below differs along what that rule generalises over: the page size, how many
//! events are delivered between two pages and whether the walk is still running when they
//! arrive, how many events the buffer's ceiling evicts between two pages, which component
//! of the query a resuming request changes, and where the window's end falls among the
//! events' instants (including an event stamped AHEAD of the server clock, which
//! `RumEvent$timestamp` — "The exact time that this event occurred", a `Timestamp` with no
//! range — admits).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_rum_rum::ops::types::RumEvent;
use service_com_amazonaws_rum_rum::state::rum_event::{event_document, Delivery};
use service_com_amazonaws_rum_rum::state::{StoredRumEvent, MAX_EVENTS_PER_APP_MONITOR};
use service_com_amazonaws_rum_rum::ComAmazonawsRumRumState;

use crate::support::{call, create, emulator, message, REGION};

/// The monitor every case queries.
const MONITOR: &str = "paging-monitor";

/// `2024-01-01T00:00:00Z` in seconds since the epoch: the instant the first seeded event of
/// a clock-free case carries, with the rest one second apart after it.
const BASE: i64 = 1_704_067_200;

/// A window end far past every seeded instant, so a case that is not about the window names
/// one bound explicitly and the clock plays no part in it.
const WINDOW_END: i64 = BASE + 86_400;

/// `PutRumEventsRequest$BatchId` every delivery carries: a UUID, as the member's pattern
/// asks.
const BATCH_ID: &str = "11111111-2222-4333-8444-555555555555";

/// `RumEvent$type` every seeded event carries.
const EVENT_TYPE: &str = "com.amazon.rum.page_view_event";

/// The most pages any walk below is allowed to take before it is declared non-terminating:
/// generously above the pages the largest case needs (eight matching events at one per
/// page).
const MAX_PAGES: usize = 64;

/// The `id` of the seeded event at `index`, matching `RumEvent$id`'s pattern
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
fn event_id(index: usize) -> String {
    format!("{index:08x}-1234-4234-8234-123456789012")
}

/// The browser the event at `index` reports, so a filtered case selects half the events:
/// the odd indices are one browser and the even ones another.
fn browser_of(index: usize) -> &'static str {
    if index % 2 == 1 {
        "Chrome"
    } else {
        "Firefox"
    }
}

/// One event as `PutRumEvents` carries it: `metadata` and `details` are
/// `@mediaType("application/json")` strings, so the documents travel serialized.
fn wire_event(index: usize, instant: i64) -> Value {
    json!({
        "id": event_id(index),
        "timestamp": instant,
        "type": EVENT_TYPE,
        "metadata": json!({"browserName": browser_of(index)}).to_string(),
        "details": json!({"pageTitle": "/home"}).to_string(),
    })
}

/// One event as the monitor's buffer holds it, for the one case that fills the buffer to its
/// ceiling (which no wire body is the right size to do).
///
/// The document goes in through the SAME projection the data-plane write stores it with
/// (`crate::state::rum_event::event_document`), so a seeded event is placed in a window,
/// reported and filtered exactly as a delivered one is.
fn stored_event(monitor_id: &str, index: usize, instant: i64) -> StoredRumEvent {
    let event = RumEvent {
        id: event_id(index),
        timestamp: DateTime::from_secs(instant),
        r#type: EVENT_TYPE.to_string(),
        metadata: Some(json!({"browserName": browser_of(index)}).to_string()),
        details: json!({"pageTitle": "/home"}).to_string(),
    };
    StoredRumEvent {
        event_json: event_document(
            &Delivery {
                app_monitor_id: monitor_id,
                app_monitor_name: MONITOR,
                app_monitor_version: Some("1.0.0"),
                batch_id: BATCH_ID,
                user_id: None,
                session_id: None,
                // This case seeds each event's own metadata, so the batch declares no
                // attributes for every event to share.
                shared_attributes: None,
            },
            &event,
        ),
    }
}

/// Now, in seconds since the epoch — the instant an omitted `TimeRange$Before` resolves to
/// ("If you omit this, the time range extends to the time that this operation is
/// performed"). The cases that are about the window place their events hours either side of
/// it, so which side an event falls on cannot depend on how fast the host runs.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock at or after the epoch")
            .as_secs(),
    )
    .expect("an instant an i64 can hold")
}

/// A query body for `after`, `before`, `filters` and `max_results`, with both window
/// bounds taken from the whole second a case seeds its events at ([`window_bound`]).
fn query(after: i64, before: Option<i64>, filters: Value, max_results: i64) -> Value {
    let mut time_range = json!({"After": window_bound(after)});
    if let Some(before) = before {
        time_range["Before"] = json!(window_bound(before));
    }
    json!({"TimeRange": time_range, "Filters": filters, "MaxResults": max_results})
}

/// How many milliseconds one second is: `TimeRange$After`/`$Before` bound an event in epoch
/// MILLISECONDS, while `RumEvent$timestamp` travels as the epoch seconds restJson1 carries
/// a `Timestamp` as, so a case seeds its events in seconds and asks for them in
/// milliseconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// One window bound, from the whole second a case places its events at.
fn window_bound(seconds: i64) -> i64 {
    seconds * MILLIS_PER_SECOND
}

/// One monitor with `events` delivered to it through the data plane, and the engine behind
/// it (the one case that fills the buffer to its ceiling seeds it through the store).
async fn seeded(case: &str, events: Vec<Value>) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine) = emulator();
    let monitor_id = create(
        &router,
        case,
        json!({"Name": MONITOR, "Domain": "paging.example.com"}),
    )
    .await;
    if !events.is_empty() {
        deliver(&router, case, &monitor_id, events).await;
    }
    (router, engine, monitor_id)
}

/// Deliver `events` to the monitor `monitor_id` names through `PutRumEvents`.
async fn deliver(router: &MultiServiceRouter, case: &str, monitor_id: &str, events: Vec<Value>) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(json!({
            "BatchId": BATCH_ID,
            "AppMonitorDetails": {"name": MONITOR, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {
                "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
            },
            "RumEvents": events,
        })),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// One page of a query: the `id` of each event it reported, in order, and its `NextToken`.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> (Vec<String>, Option<String>) {
    let reply = call(
        router,
        "POST",
        &format!("/appmonitor/{MONITOR}/data"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let reported = reply.json(case);
    let ids = reported["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Events array, got {reported}"))
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected each event to be a JSON string, got {entry}")
            });
            let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON document, got {text:?} ({error})")
            });
            document["event_id"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    (ids, reported["NextToken"].as_str().map(str::to_string))
}

/// One walk over a query's pages.
struct Walk<'a> {
    /// The request the FIRST page is asked with.
    first: Value,
    /// The request every continuation is asked with, carrying the token of the page before
    /// it. Usually the same request; a case about the window resumes with a body that
    /// leaves `TimeRange$Before` out.
    resumed: Value,
    /// Events delivered through the data plane after the first page — what makes the
    /// collection change mid-walk.
    delivered: Vec<Value>,
    /// The monitor those events are delivered to.
    monitor_id: &'a str,
}

/// Follow a walk's pages to the end, reporting every event id they carried in order.
///
/// Asserts on the way that no event is reported twice across the pages and that the walk
/// terminates.
async fn walk(router: &MultiServiceRouter, case: &str, plan: Walk<'_>) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let request = match &token {
            None => plan.first.clone(),
            Some(token) => {
                let mut resumed = plan.resumed.clone();
                resumed["NextToken"] = json!(token);
                resumed
            }
        };
        let (ids, next) = page(router, case, request).await;
        for id in &ids {
            assert!(
                !collected.contains(id),
                "case {case:?}: expected each matching event to be reported once, got {id} \
                 again after {collected:?} (this page: {ids:?})"
            );
        }
        collected.extend(ids);
        pages += 1;
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: paging did not terminate after {pages} pages, got {collected:?}"
        );
        if pages == 1 && !plan.delivered.is_empty() {
            deliver(router, case, plan.monitor_id, plan.delivered.clone()).await;
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    collected
}

/// The ids of the events at `indexes`.
fn ids_of(indexes: &[usize]) -> Vec<String> {
    indexes.iter().copied().map(event_id).collect()
}

/// The six events a clock-free case starts from, one second apart inside [`WINDOW_END`].
fn seed_events() -> Vec<Value> {
    (1..=6)
        .map(|index| wire_event(index, BASE + index as i64))
        .collect()
}

/// One row of the table below: a walk at one page size over a query, with events delivered
/// to the monitor after its first page.
struct Arriving {
    case: &'static str,
    /// `MaxResults`.
    page_size: i64,
    /// `Filters`.
    filters: Value,
    /// The events delivered after the first page, by seeded index.
    arriving: Vec<usize>,
    /// The events the pages must report between them, by seeded index, in order.
    expected: Vec<usize>,
}

#[tokio::test]
async fn paging_reports_each_matching_event_once_while_events_keep_arriving() {
    // Each row differs along the page size, how many events arrive between two pages,
    // whether the walk is still running when they do, and whether the query narrows by a
    // filter. The window is named explicitly, so these rows are about the resume point
    // alone.
    let table = vec![
        Arriving {
            case: "pages of one, nothing arriving",
            page_size: 1,
            filters: json!([]),
            arriving: vec![],
            expected: (1..=6).collect(),
        },
        Arriving {
            case: "pages of one, two events arriving after the first page",
            page_size: 1,
            filters: json!([]),
            arriving: vec![7, 8],
            expected: (1..=8).collect(),
        },
        Arriving {
            case: "pages of two, two events arriving after the first page",
            page_size: 2,
            filters: json!([]),
            arriving: vec![7, 8],
            expected: (1..=8).collect(),
        },
        Arriving {
            case: "pages of three, one event arriving after the first page",
            page_size: 3,
            filters: json!([]),
            arriving: vec![7],
            expected: (1..=7).collect(),
        },
        Arriving {
            case: "pages of five, two events arriving after the first page",
            page_size: 5,
            filters: json!([]),
            arriving: vec![7, 8],
            expected: (1..=8).collect(),
        },
        // One page holds the whole result set, so the walk is already over when the events
        // arrive: they belong to the next query, not to this one.
        Arriving {
            case: "one page holding everything, an event arriving after the walk",
            page_size: 6,
            filters: json!([]),
            arriving: vec![7],
            expected: (1..=6).collect(),
        },
        // Paging walks what the filter selected, and an arriving event joins that walk only
        // when it matches.
        Arriving {
            case: "pages of one, a filter the arriving events straddle",
            page_size: 1,
            filters: json!([{"Name": "Browser", "Values": ["Chrome"]}]),
            arriving: vec![7, 8],
            expected: vec![1, 3, 5, 7],
        },
        Arriving {
            case: "pages of two, a filter the arriving events straddle",
            page_size: 2,
            filters: json!([{"Name": "Browser", "Values": ["Firefox"]}]),
            arriving: vec![7, 8],
            expected: vec![2, 4, 6, 8],
        },
        // A query matching nothing has one empty page and no token, so there is nothing to
        // resume at all.
        Arriving {
            case: "a filter matching nothing, an event arriving after the first page",
            page_size: 1,
            filters: json!([{"Name": "Browser", "Values": ["Opera"]}]),
            arriving: vec![7],
            expected: vec![],
        },
    ];

    for row in table {
        let (router, _engine, monitor_id) = seeded(row.case, seed_events()).await;
        let request = query(0, Some(WINDOW_END), row.filters, row.page_size);
        let collected = walk(
            &router,
            row.case,
            Walk {
                first: request.clone(),
                resumed: request,
                delivered: row
                    .arriving
                    .iter()
                    .map(|index| wire_event(*index, BASE + *index as i64))
                    .collect(),
                monitor_id: &monitor_id,
            },
        )
        .await;
        let case = row.case;
        assert_eq!(
            collected,
            ids_of(&row.expected),
            "case {case:?}: expected the pages to report every matching event once, in the \
             order the data plane appended them, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_after_the_event_it_ended_on_when_the_buffer_evicts_its_oldest() {
    // The buffer keeps the last `MAX_EVENTS_PER_APP_MONITOR` events delivered and drops
    // the oldest past that, so events the pages before a resume were counted against can
    // leave the buffer between two pages. Each row differs along how many of them do.
    for evicted in [1_usize, 2, 10] {
        let case = format!("{evicted} events evicted between two pages");
        let (router, engine, monitor_id) = seeded(&case, Vec::new()).await;

        // The buffer at its ceiling, seeded through the store the data plane writes to:
        // nothing else fills it, and no wire body is the right size to.
        {
            let state = engine.state::<ComAmazonawsRumRumState>();
            let mut store = state.lock();
            let record = store
                .app_monitor_mut(REGION, MONITOR)
                .expect("the monitor the create stored");
            record.record_events(
                (0..MAX_EVENTS_PER_APP_MONITOR)
                    .map(|index| stored_event(&monitor_id, index, BASE)),
            );
        }

        let request = query(0, Some(WINDOW_END), json!([]), 1);
        let (first, token) = page(&router, &case, request.clone()).await;
        assert_eq!(
            first,
            ids_of(&[0]),
            "case {case:?}: expected the first page to report the oldest event held, got \
             {first:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken while the buffer holds more")
        });

        // The delivery that pushes the buffer past its ceiling: the oldest `evicted`
        // events, the first page's own event among them, leave the buffer.
        deliver(
            &router,
            &case,
            &monitor_id,
            (0..evicted)
                .map(|step| wire_event(MAX_EVENTS_PER_APP_MONITOR + step, BASE + 1))
                .collect(),
        )
        .await;

        let mut resumed = request.clone();
        resumed["NextToken"] = json!(token);
        let (second, next) = page(&router, &case, resumed).await;
        assert_eq!(
            second,
            ids_of(&[evicted]),
            "case {case:?}: expected the resumed page to report the oldest event the buffer \
             still holds — the one ranked straight after the first page's — rather than one \
             {evicted} further on, got {second:?}"
        );

        // And the walk goes on from there, still one event at a time, with nothing skipped.
        let token = next.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken while the buffer holds more")
        });
        let mut resumed = request;
        resumed["NextToken"] = json!(token);
        let (third, _) = page(&router, &case, resumed).await;
        assert_eq!(
            third,
            ids_of(&[evicted + 1]),
            "case {case:?}: expected the next page to report the event ranked after the \
             resumed one, got {third:?}"
        );
    }
}

#[tokio::test]
async fn a_continuation_is_answered_against_the_window_its_query_started_with() {
    // The instants the rows place either side of the window's end: hours away from the
    // clock on both sides, so which side an event falls on cannot depend on how fast the
    // host runs. The last event is stamped AHEAD of the server clock, which a sending
    // application's own clock running fast produces and `RumEvent$timestamp` bounds in no
    // way.
    let now = now_seconds();
    let instants: Vec<(usize, i64)> = vec![
        (1, now - 7_200),
        (2, now - 7_190),
        (3, now - 7_180),
        (4, now - 600),
        (5, now + 3_600),
    ];
    let events: Vec<Value> = instants
        .iter()
        .map(|(index, instant)| wire_event(*index, *instant))
        .collect();

    // Each row differs along where the window's end falls among those instants and along
    // the page size; every row resumes with a body that leaves `TimeRange$Before` out, so
    // the window a continuation is answered against is the one its query started with
    // rather than one resolved afresh.
    let table: Vec<(&str, Option<i64>, Vec<usize>)> = vec![
        ("a window the query named", Some(now - 3_600), vec![1, 2, 3]),
        ("a window ending at the instant the query was performed", None, vec![1, 2, 3, 4]),
    ];

    for (window, before, expected) in table {
        for page_size in [1_i64, 2] {
            let case = format!("{window}, pages of {page_size}");
            let (router, _engine, monitor_id) = seeded(&case, events.clone()).await;
            let collected = walk(
                &router,
                &case,
                Walk {
                    first: query(0, before, json!([]), page_size),
                    resumed: query(0, None, json!([]), page_size),
                    delivered: Vec::new(),
                    monitor_id: &monitor_id,
                },
            )
            .await;
            assert_eq!(
                collected,
                ids_of(&expected),
                "case {case:?}: expected every page to report the events of the window the \
                 query started with, got {collected:?}"
            );

            // The event stamped ahead of the clock is not lost: it is outside the window
            // this query was issued for, and a query whose window covers its instant
            // reports it.
            let (covered, _) = page(
                &router,
                &case,
                query(0, Some(now + 7_200), json!([]), 0),
            )
            .await;
            assert_eq!(
                covered,
                ids_of(&[1, 2, 3, 4, 5]),
                "case {case:?}: expected a query covering every instant to report every \
                 event, got {covered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_is_only_honoured_by_the_query_that_issued_it() {
    // The query every row below resumes: two filters over one dimension, so a row can
    // reorder them, repeat one of them or drop one.
    let issuing = |before: Option<i64>| {
        query(
            0,
            before,
            json!([
                {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                {"Name": "Browser", "Values": ["Edge"]},
            ]),
            1,
        )
    };

    // Rows that are the SAME query differently spelled, or that differ only in what a
    // token is not scoped to: each is honoured and reports the event ranked after the one
    // the first page ended on. The filters and their values are a disjunction, so their
    // order and their repetition change nothing about what the query selects.
    let honoured: Vec<(&str, Value)> = vec![
        ("the same query again", issuing(Some(WINDOW_END))),
        (
            "the same filters in the other order",
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Edge"]},
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                ]),
                1,
            ),
        ),
        (
            "the same values in the other order",
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Firefox", "Chrome"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        (
            "a repeated filter",
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        (
            "a repeated value",
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        // A page size is not part of the query a token is scoped to: it says how much of
        // the rest to return, not what the rest is.
        (
            "another page size",
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                3,
            ),
        ),
    ];

    for (case, resumed) in honoured {
        let (router, _engine, _id) = seeded(case, seed_events()).await;
        let (first, token) = page(&router, case, issuing(Some(WINDOW_END))).await;
        assert_eq!(
            first,
            ids_of(&[1]),
            "case {case:?}: expected the first page to report the oldest matching event, got \
             {first:?}"
        );
        let mut request = resumed;
        request["NextToken"] = json!(token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken while events remain")
        }));
        let (second, _) = page(&router, case, request).await;
        assert_eq!(
            second.first().map(String::as_str),
            Some(event_id(2).as_str()),
            "case {case:?}: expected the page to resume after the event the first page \
             ended on, got {second:?}"
        );
    }

    // Rows that are ANOTHER query: the token names a position in a sequence this request
    // does not ask for, so it is refused rather than read as a position in this one.
    let refused: Vec<(&str, Option<i64>, Value)> = vec![
        (
            "another window start",
            Some(WINDOW_END),
            query(
                BASE,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        (
            "another window end",
            Some(WINDOW_END),
            query(
                0,
                Some(WINDOW_END + 1),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        // The window the first page resolved for an omitted `Before` is the query's as
        // much as a named one is: a request naming a different end asks about other
        // instants.
        (
            "a window end named after one was left to the instant the query ran at",
            None,
            query(
                0,
                Some(BASE),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        (
            "a filter dropped",
            Some(WINDOW_END),
            query(
                0,
                Some(WINDOW_END),
                json!([{"Name": "Browser", "Values": ["Chrome", "Firefox"]}]),
                1,
            ),
        ),
        (
            "a value dropped",
            Some(WINDOW_END),
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
        (
            "a filter added",
            Some(WINDOW_END),
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Browser", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Browser", "Values": ["Edge"]},
                    {"Name": "Country", "Values": ["US"]},
                ]),
                1,
            ),
        ),
        (
            "no filters at all",
            Some(WINDOW_END),
            query(0, Some(WINDOW_END), json!([]), 1),
        ),
        (
            "the same values under another key",
            Some(WINDOW_END),
            query(
                0,
                Some(WINDOW_END),
                json!([
                    {"Name": "Invert", "Values": ["Chrome", "Firefox"]},
                    {"Name": "Invert", "Values": ["Edge"]},
                ]),
                1,
            ),
        ),
    ];

    for (case, before, resumed) in refused {
        let (router, _engine, _id) = seeded(case, seed_events()).await;
        let (_, token) = page(&router, case, issuing(before)).await;
        let token = token
            .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken while events remain"));
        let mut request = resumed;
        request["NextToken"] = json!(token);
        let reply = call(
            &router,
            "POST",
            &format!("/appmonitor/{MONITOR}/data"),
            Some(request),
        )
        .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected a token presented with another query to be refused, got \
             {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected the modelled validation error, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let reported = reply.json(case);
        let text = message(&reported).unwrap_or_default();
        assert!(
            text.contains(&token) && text.contains(MONITOR),
            "case {case:?}: expected the refusal to name the token and the app monitor, got \
             {text:?}"
        );
    }
}
