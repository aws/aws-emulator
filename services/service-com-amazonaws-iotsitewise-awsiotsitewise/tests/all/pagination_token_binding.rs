// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` of a time-ranged read belongs to the REQUEST that minted it.
//!
//! The rule these suites pin: a paginated read honours a `nextToken` only against a
//! request asking for the same enumeration — the same stream or source, the same
//! range, the same filters and the same ordering, and for a BATCH read the same list
//! of entries — and refuses a token minted by any other request with the modelled
//! `InvalidRequestException` (HTTP 400), serving no page at all. A resume point on
//! its own (an instant, a window start, a position in a list) names a position in
//! every enumeration that happens to hold it, and the readings of several
//! properties are written at the SAME timestamps, so without that binding a token
//! would silently serve a slice of another enumeration and drop everything before the
//! point it resumed at.
//!
//! What does NOT change the enumeration does not break a token: the page size, and
//! naming the same stream or source by its other identifying form.
//!
//! The rows span the readers of this service that page by the instant they stopped
//! at — `GetAssetPropertyValueHistory`, `GetAssetPropertyAggregates` and
//! `GetCaptureData` — and the BATCH readers that page by a POSITION in the entry
//! list of the request (`BatchGetAssetPropertyAggregates`,
//! `BatchGetAssetPropertyValueHistory`), and, for each, the request members the rule
//! generalises over.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, …) is the one the
//! asset suite already has; this suite reuses it rather than growing a second.

use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, CaptureSegment};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router_with_engine,
};

/// The epoch this suite's history and aggregate tables measure their seconds from.
///
/// `BatchPutAssetPropertyValue` ingests only TQVs inside `[now - 7 days, now + 10
/// minutes]`, so the shared `super::support` anchoring maps these small seconds into
/// the accepted window at the wire boundary.
const ROW_EPOCH: i64 = 0;

/// A table second as the absolute `timeInSeconds` the wire carries.
fn anchored(seconds: i64) -> i64 {
    super::support::anchored(ROW_EPOCH, seconds)
}

/// The inverse of [`anchored`].
fn relative(seconds: i64) -> i64 {
    super::support::relative(ROW_EPOCH, seconds)
}

/// A table second as the RFC 3339 form an `@httpQuery` timestamp travels as:
/// `startDate` and `endDate` carry no `@timestampFormat`, so they use Smithy's
/// default `date-time`.
fn query_date(seconds: i64) -> String {
    DateTime::from_secs(anchored(seconds))
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// The modelled `Quality` values a filter may name. Source:
/// `smithy_query(query="shape", shape="Quality")` — GOOD | BAD | UNCERTAIN.
const QUALITIES: [&str; 3] = ["GOOD", "BAD", "UNCERTAIN"];

/// An asset carrying two DOUBLE measurement properties; answers
/// `(assetId, firstPropertyId, secondPropertyId)`.
async fn asset_with_two_properties(
    router: &MultiServiceRouter,
    name: &str,
    first: &str,
    second: &str,
) -> (String, String, String) {
    let model_id = create_model(
        router,
        &format!("{name}Model"),
        json!([
            {"name": first, "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": second, "dataType": "DOUBLE", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(router, name, &model_id).await;
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset({name}): got {status} {asset}");
    let first_id = property_id(&asset["assetProperties"], first);
    let second_id = property_id(&asset["assetProperties"], second);
    (asset_id, first_id, second_id)
}

/// Write `samples` — `(table second, quality)`, a `None` quality meaning the write
/// carries none — to every `(assetId, propertyId)` in `streams`, so the SAME instants
/// land in each of them, which is what a single `BatchPutAssetPropertyValue` call
/// does.
async fn write_to_each(
    router: &MultiServiceRouter,
    streams: &[(&str, &str)],
    samples: &[(i64, Option<&str>)],
) {
    let entries: Vec<Value> = streams
        .iter()
        .enumerate()
        .map(|(at, (asset_id, property))| {
            let values: Vec<Value> = samples
                .iter()
                .map(|(seconds, quality)| {
                    let mut sample = json!({
                        "value": {"doubleValue": *seconds as f64 / 10.0},
                        "timestamp": {"timeInSeconds": anchored(*seconds), "offsetInNanos": 0},
                    });
                    if let Some(quality) = quality {
                        sample["quality"] = json!(quality);
                    }
                    sample
                })
                .collect();
            json!({
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": values,
            })
        })
        .collect();
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "writing samples: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "writing samples: every entry must be accepted, got {body}"
    );
}

/// `GET /properties/history?<query>`.
async fn history(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/properties/history?{query}"), None).await
}

/// The table seconds of the samples a history answer reports, in the order reported.
fn reported_seconds(case: &str, body: &Value) -> Vec<i64> {
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {body}"))
        .iter()
        .map(|sample| {
            relative(
                sample["timestamp"]["timeInSeconds"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case:?}: a sample with no timestamp: {body}")),
            )
        })
        .collect()
}

/// Mint a first-page `nextToken` from the history read `query` names, asserting the
/// page it answers is the one sample the query asked for.
async fn mint_history_token(router: &MultiServiceRouter, case: &str, query: &str) -> String {
    let (status, body) = history(router, &format!("{query}&maxResults=1")).await;
    assert_eq!(status, 200, "minting {case:?}: got {status} {body}");
    assert_eq!(
        reported_seconds(case, &body).len(),
        1,
        "minting {case:?}: expected a one-sample page, got {body}"
    );
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting {case:?}: expected a nextToken, got {body}"))
        .to_string()
}

/// Assert `body`/`status` is the modelled refusal of a token this request never
/// issued, carrying no page at all.
fn assert_token_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a token minted by another request, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body}"
    );
    assert!(
        body.get("assetPropertyValueHistory").is_none()
            && body.get("aggregatedValues").is_none()
            && body.get("data").is_none(),
        "case {case:?}: a refused token must serve no page, got {body}"
    );
}

#[tokio::test]
async fn a_history_token_is_honoured_only_by_the_request_that_minted_it() {
    let (router, engine) = router_with_engine();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Turbine1", "Speed", "Torque").await;
    let (other_asset, other_speed, _) =
        asset_with_two_properties(&router, "Turbine2", "Speed", "Torque").await;

    // One batch, the same instants in every stream — the shape the issue's own
    // reproduction takes, and the ordinary shape of industrial ingest.
    let written: [(i64, Option<&str>); 5] = [
        (100, None),
        (200, None),
        (250, Some("BAD")),
        (300, None),
        (400, None),
    ];
    write_to_each(
        &router,
        &[
            (&asset_id, &speed),
            (&asset_id, &torque),
            (&other_asset, &other_speed),
        ],
        &written,
    )
    .await;

    // The alias form names the same stream as the ids do. The operations that bind an
    // alias are still 501 scaffolds, so the index is seeded in the one shared store
    // the handlers read.
    let alias = "/company/windfarm/3/turbine/1/speed";
    service_state(&*engine)
        .expect("the service store")
        .lock()
        .property_alias_index
        .insert(alias.to_string(), (asset_id.clone(), speed.clone()));

    let minted = format!("assetId={asset_id}&propertyId={speed}");
    let token = mint_history_token(&router, "the plain read", &minted).await;

    // Rows that ask for the SAME enumeration: the token resumes at the sample after
    // the first page, whatever page size or identifying form the request uses.
    let honoured: Vec<(&str, String)> = vec![
        ("the request that minted it", format!("{minted}&maxResults=1")),
        ("a wider page", format!("{minted}&maxResults=4")),
        ("no page size at all", minted.clone()),
        (
            "the ordering named explicitly, as the default",
            format!("{minted}&timeOrdering=ASCENDING&maxResults=1"),
        ),
        ("the same stream by its alias", format!("propertyAlias={alias}")),
    ];
    for (case, query) in honoured {
        let (status, body) = history(&router, &format!("{query}&nextToken={token}")).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported = reported_seconds(case, &body);
        assert_eq!(
            reported.first().copied(),
            Some(200),
            "case {case:?}: expected the page to resume at 200, got {reported:?}"
        );
    }

    // Rows that ask for ANOTHER enumeration. Each one holds a sample at the resumed
    // instant, so each would be served from that offset — dropping every earlier
    // sample of it — were the token not bound to the request that minted it.
    let refused: Vec<(&str, String)> = vec![
        (
            "another property of the same asset",
            format!("assetId={asset_id}&propertyId={torque}"),
        ),
        (
            "the same property name on another asset",
            format!("assetId={other_asset}&propertyId={other_speed}"),
        ),
        (
            "the ordering flipped",
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "a quality filter the minting read did not carry",
            format!("{minted}&qualities=GOOD"),
        ),
        (
            "a start bound the minting read did not carry",
            format!("{minted}&startDate={}", query_date(150)),
        ),
        (
            "an end bound the minting read did not carry",
            format!("{minted}&endDate={}", query_date(300)),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = history(&router, &format!("{query}&nextToken={token}")).await;
        assert_token_refused(case, status, &body);
    }

    // The same binding the other way round: a token minted by a request that names a
    // filter, an ordering or a window is no token of the plain read either. Rows span
    // the whole `Quality` catalog, including the values the issue never cites.
    let mut variants: Vec<(String, String)> = vec![
        (
            "minted DESCENDING".to_string(),
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "minted over a narrower window".to_string(),
            format!(
                "{minted}&startDate={}&endDate={}",
                query_date(99),
                query_date(300)
            ),
        ),
    ];
    for quality in QUALITIES {
        // Every quality has at least two samples to page through: BAD has the one
        // written as BAD plus — for GOOD and UNCERTAIN — none, so only the qualities
        // the writes cover can mint a token.
        variants.push((
            format!("minted qualities={quality}"),
            format!("{minted}&qualities={quality}"),
        ));
    }
    for (case, query) in variants {
        let (status, body) = history(&router, &format!("{query}&maxResults=1")).await;
        assert_eq!(status, 200, "minting {case:?}: got {status} {body}");
        let Some(variant_token) = body["nextToken"].as_str() else {
            // A variant whose enumeration holds one sample or none issues no token,
            // so there is nothing to replay for it.
            continue;
        };
        let (status, replayed) =
            history(&router, &format!("{minted}&nextToken={variant_token}")).await;
        assert_token_refused(&case, status, &replayed);
        // ... and it still resumes its OWN enumeration.
        let (status, resumed) =
            history(&router, &format!("{query}&nextToken={variant_token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: its own request must still resume it, got {status} {resumed}"
        );
        assert!(
            !reported_seconds(&case, &resumed).is_empty(),
            "case {case:?}: its own request must be answered a page, got {resumed}"
        );
    }
}

/// `GET /properties/aggregates?...` for one property over `(start, end]` at
/// `resolution`.
fn aggregates_query(
    asset_id: &str,
    property: &str,
    resolution: &str,
    start: i64,
    end: i64,
) -> String {
    format!(
        "assetId={asset_id}&propertyId={property}&resolution={resolution}\
         &aggregateTypes=COUNT&startDate={}&endDate={}",
        query_date(start),
        query_date(end)
    )
}

/// The epoch seconds of the windows an aggregate answer reports, in order.
///
/// Absolute, not table seconds: the window grid is aligned to the Unix epoch, so
/// where a window starts is decided by that grid and not by this suite's own epoch.
fn reported_windows(case: &str, body: &Value) -> Vec<i64> {
    body["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
        .iter()
        .map(|aggregate| {
            aggregate["timestamp"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: an aggregate with no timestamp"))
                as i64
        })
        .collect()
}

/// `GET /properties/aggregates?...` for the stream `stream` spells — either
/// `assetId=…&propertyId=…` or `propertyAlias=…` — over `(start, end]` at
/// `resolution`, asking for `types`.
///
/// The aggregate SELECTION is a parameter here where [`aggregates_query`] fixes it
/// at one function: `aggregateTypes` decides what each window reports and therefore
/// which windows are reported at all, so it is part of what a token is bound to. A
/// list bound to the query string travels as its name repeated once per entry.
fn aggregates_selection(
    stream: &str,
    resolution: &str,
    start: i64,
    end: i64,
    types: &[&str],
) -> String {
    let selection: String = types
        .iter()
        .map(|aggregate| format!("&aggregateTypes={aggregate}"))
        .collect();
    format!(
        "{stream}&resolution={resolution}&startDate={}&endDate={}{selection}",
        query_date(start),
        query_date(end)
    )
}

#[tokio::test]
async fn an_aggregates_token_is_bound_to_the_selection_and_the_stream_it_names() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Press1", "Speed", "Torque").await;
    let (other_asset, other_speed, _) =
        asset_with_two_properties(&router, "Press2", "Speed", "Torque").await;

    // The same instants in every stream, in three distinct `1m` windows, plus one BAD
    // reading so a quality-filtered enumeration of the same stream exists too. The
    // window grid is aligned to the epoch, so each of these streams enumerates exactly
    // the same window starts — which is what puts a foreign token's resume point in
    // every one of them.
    let written: [(i64, Option<&str>); 4] =
        [(0, None), (120, None), (180, Some("BAD")), (240, None)];
    write_to_each(
        &router,
        &[
            (&asset_id, &speed),
            (&asset_id, &torque),
            (&other_asset, &other_speed),
        ],
        &written,
    )
    .await;

    // An alias is another SPELLING of one stream, not another stream: bound to this
    // property through `UpdateAssetProperty`, it asks for the same enumeration the
    // ids do.
    let alias = "/company/press/1/speed";
    let (status, bound) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{speed}"),
        Some(json!({"propertyAlias": alias})),
    )
    .await;
    assert_eq!(status, 200, "binding the alias: got {status} {bound}");

    let ids = format!("assetId={asset_id}&propertyId={speed}");
    let minted = aggregates_selection(&ids, "1m", -60, 300, &["COUNT", "AVERAGE"]);

    // What the whole enumeration is, so a resumed page can be checked against the
    // window that really follows the first one.
    let (status, whole) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the unpaginated read: got {status} {whole}");
    let windows = reported_windows("the unpaginated read", &whole);
    assert_eq!(
        windows.len(),
        3,
        "the unpaginated read: expected the three windows the GOOD readings fall in, \
         got {windows:?}"
    );

    let (status, first) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}&maxResults=1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "minting the token: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting the token: expected a nextToken, got {first}"))
        .to_string();

    // Rows asking for the SAME enumeration, spelled differently: the token resumes at
    // the window after the first page, whatever page size the request names, however
    // it names the stream, and whether it restates a default or leaves it out.
    let honoured: Vec<(&str, String)> = vec![
        ("the request that minted it", format!("{minted}&maxResults=1")),
        ("a wider page", format!("{minted}&maxResults=2")),
        ("no page size at all", minted.clone()),
        (
            "the ordering named explicitly, as the default",
            format!("{minted}&timeOrdering=ASCENDING"),
        ),
        (
            "the quality named explicitly, as the default",
            format!("{minted}&qualities=GOOD"),
        ),
        (
            "the selection repeating a function it already names",
            aggregates_selection(&ids, "1m", -60, 300, &["COUNT", "AVERAGE", "COUNT"]),
        ),
        (
            "the same stream by its alias",
            aggregates_selection(
                &format!("propertyAlias={alias}"),
                "1m",
                -60,
                300,
                &["COUNT", "AVERAGE"],
            ),
        ),
    ];
    for (case, query) in honoured {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/aggregates?{query}&nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let resumed = reported_windows(case, &body);
        assert_eq!(
            resumed.first().copied(),
            windows.get(1).copied(),
            "case {case:?}: expected the page to resume at the second window {:?}, \
             got {resumed:?}",
            windows.get(1)
        );
    }

    // Rows asking for ANOTHER enumeration. Every one of them holds a window at the
    // resume point the token carries, so each would be served from that offset —
    // dropping every earlier window of it — were the token not bound to the request
    // that minted it. The selection rows span a narrower, a wider and a differently
    // named one; the resolution rows every other value the modelled pattern
    // `^1m|15m|1h|1d$` admits.
    let refused: Vec<(&str, String)> = vec![
        (
            "a selection naming fewer functions",
            aggregates_selection(&ids, "1m", -60, 300, &["COUNT"]),
        ),
        (
            "a selection naming more functions",
            aggregates_selection(&ids, "1m", -60, 300, &["COUNT", "AVERAGE", "SUM"]),
        ),
        (
            "a selection naming another function",
            aggregates_selection(&ids, "1m", -60, 300, &["COUNT", "SUM"]),
        ),
        (
            "another property of the same asset",
            aggregates_selection(
                &format!("assetId={asset_id}&propertyId={torque}"),
                "1m",
                -60,
                300,
                &["COUNT", "AVERAGE"],
            ),
        ),
        (
            "the same property name on another asset",
            aggregates_selection(
                &format!("assetId={other_asset}&propertyId={other_speed}"),
                "1m",
                -60,
                300,
                &["COUNT", "AVERAGE"],
            ),
        ),
        (
            "the resolution 15m",
            aggregates_selection(&ids, "15m", -60, 300, &["COUNT", "AVERAGE"]),
        ),
        (
            "the resolution 1h",
            aggregates_selection(&ids, "1h", -60, 300, &["COUNT", "AVERAGE"]),
        ),
        (
            "the resolution 1d",
            aggregates_selection(&ids, "1d", -60, 300, &["COUNT", "AVERAGE"]),
        ),
        (
            "the ordering flipped",
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "a quality filter the minting read did not carry",
            format!("{minted}&qualities=BAD"),
        ),
        (
            "a start bound the minting read did not carry",
            aggregates_selection(&ids, "1m", -120, 300, &["COUNT", "AVERAGE"]),
        ),
        (
            "an end bound the minting read did not carry",
            aggregates_selection(&ids, "1m", -60, 360, &["COUNT", "AVERAGE"]),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/aggregates?{query}&nextToken={token}"),
            None,
        )
        .await;
        assert_token_refused(case, status, &body);
    }

    // A refusal costs the enumeration it was replayed against nothing: walked from
    // its own start it still reports every window, the ones BEFORE the resume point
    // that foreign token carried included.
    let other = aggregates_selection(
        &format!("assetId={asset_id}&propertyId={torque}"),
        "1m",
        -60,
        300,
        &["COUNT", "AVERAGE"],
    );
    let (status, body) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{other}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the other enumeration: got {status} {body}");
    let expected = reported_windows("the other enumeration", &body);
    let mut walked: Vec<i64> = Vec::new();
    let mut resume: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match resume.as_deref() {
            Some(resume) => format!(
                "/properties/aggregates?{other}&maxResults=1&nextToken={resume}"
            ),
            None => format!("/properties/aggregates?{other}&maxResults=1"),
        };
        let (status, page) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "walking the other enumeration: got {status} {page}"
        );
        walked.extend(reported_windows("walking the other enumeration", &page));
        resume = page["nextToken"].as_str().map(str::to_string);
        pages += 1;
        assert!(
            pages <= expected.len() + 1,
            "walking the other enumeration: the walk must terminate, got {walked:?}"
        );
        if resume.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, expected,
        "walking the other enumeration: expected {expected:?}, got {walked:?}"
    );
}

#[tokio::test]
async fn an_aggregates_token_is_honoured_only_by_the_request_that_minted_it() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Mixer1", "Speed", "Torque").await;

    // Readings in three distinct `1m` windows, written to both properties at the same
    // instants: the window grid is aligned to the epoch, so the two enumerations carry
    // exactly the same window starts.
    let written: [(i64, Option<&str>); 3] = [(0, None), (120, None), (240, None)];
    write_to_each(
        &router,
        &[(&asset_id, &speed), (&asset_id, &torque)],
        &written,
    )
    .await;

    let minted = aggregates_query(&asset_id, &speed, "1m", -60, 300);

    // What the whole enumeration is, so the resumed page can be checked against the
    // window that really follows the first one.
    let (status, whole) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the unpaginated read: got {status} {whole}");
    let windows = reported_windows("the unpaginated read", &whole);
    assert!(
        windows.len() >= 3,
        "the unpaginated read: expected at least three windows, got {windows:?}"
    );

    let (status, first) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}&maxResults=1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "minting an aggregates token: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting an aggregates token: expected one, got {first}"))
        .to_string();

    // The request that minted it resumes at the next window; a request naming another
    // enumeration is refused, even though the window start it carries exists there.
    let (status, resumed) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minted}&nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "the minting request: got {status} {resumed}");
    assert_eq!(
        reported_windows("the minting request", &resumed)
            .first()
            .copied(),
        windows.get(1).copied(),
        "the minting request: expected the page to resume at the second window, got {resumed}"
    );

    let refused: Vec<(&str, String)> = vec![
        (
            "another property of the same asset",
            aggregates_query(&asset_id, &torque, "1m", -60, 300),
        ),
        (
            "the ordering flipped",
            format!("{minted}&timeOrdering=DESCENDING"),
        ),
        (
            "a quality filter the minting read did not carry",
            format!("{minted}&qualities=BAD"),
        ),
        (
            "a wider range",
            aggregates_query(&asset_id, &speed, "1m", -120, 300),
        ),
        (
            "a coarser resolution",
            aggregates_query(&asset_id, &speed, "1h", -60, 300),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/aggregates?{query}&nextToken={token}"),
            None,
        )
        .await;
        assert_token_refused(case, status, &body);
    }
}

/// The workspace this suite's capture sources live in.
const WORKSPACE: &str = "token-binding-floor";

/// The `VideoDataType` of the seeded media. Source:
/// `smithy_query(query="shape", shape="VideoDataType")` — the enum holds exactly this
/// one value.
const CAPTURE_DATA_TYPE: &str = "VIDEO-MP4";

/// A modelled `TimeInNanos`.
fn time_in_nanos(seconds: i64) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: Some(0),
    }
}

#[tokio::test]
async fn a_capture_data_token_is_honoured_only_by_the_request_that_minted_it() {
    let (router, engine) = router_with_engine();
    super::support::ensure_workspace(&router, WORKSPACE).await;

    // Two capture sources of ONE workspace whose segments start at the same instants —
    // two cameras of one line, started together.
    let first_source = "4c1d6f28-7a93-4b55-9e01-2d8f6c3a5b74";
    let second_source = "9b2e7a31-5c48-4f60-8d13-7e9a4c2b6f05";
    let first_alias = "/company/line/1/camera/a";
    let starts = [1_700_000_100_i64, 1_700_000_200, 1_700_000_300];
    {
        let state = service_state(&*engine).expect("the service store");
        let mut store = state.lock();
        for (source, alias) in [(first_source, Some(first_alias)), (second_source, None)] {
            for (at, start) in starts.iter().enumerate() {
                let stored = store.put_capture_segment(
                    WORKSPACE,
                    source,
                    alias,
                    CAPTURE_DATA_TYPE,
                    CaptureSegment {
                        start_time: time_in_nanos(*start),
                        end_time: time_in_nanos(start + 10),
                        data: vec![at as u8 + 1; 4],
                    },
                    SystemTime::now(),
                );
                assert_eq!(
                    stored,
                    Ok(()),
                    "seeding {source} at {start}: the producer entry point refused it"
                );
            }
        }
    }

    let uri = format!("/workspaces/{WORKSPACE}/get-capture-data");
    let window = |start: i64, end: i64| {
        json!({
            "startTime": {"timeInSeconds": start, "offsetInNanos": 0},
            "endTime": {"timeInSeconds": end, "offsetInNanos": 0},
        })
    };
    let request = |source: Option<&str>, alias: Option<&str>, start: i64, end: i64| {
        let mut body = window(start, end);
        if let Some(source) = source {
            body["timeSeriesId"] = json!(source);
        }
        if let Some(alias) = alias {
            body["propertyAlias"] = json!(alias);
        }
        body
    };

    let minting = request(Some(first_source), None, 1_700_000_000, 1_700_000_400);
    let (status, first) = call(&router, "POST", &uri, Some(minting.clone())).await;
    assert_eq!(status, 200, "minting a capture token: got {status} {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting a capture token: expected one, got {first}"))
        .to_string();

    // The requests that ask for the SAME enumeration — including the alias form, which
    // names the same source — resume at the second segment.
    for (case, mut body) in [
        ("the request that minted it", minting.clone()),
        (
            "the same source by its alias",
            request(None, Some(first_alias), 1_700_000_000, 1_700_000_400),
        ),
    ] {
        body["nextToken"] = json!(token);
        let (status, resumed) = call(&router, "POST", &uri, Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {resumed}");
        assert_eq!(
            resumed["startTime"]["timeInSeconds"].as_i64(),
            Some(starts[1]),
            "case {case:?}: expected the chunk starting at {}, got {resumed}",
            starts[1]
        );
    }

    // Another source of the same workspace holds a segment starting at the resumed
    // instant, and a narrower window selects a different run of this source's
    // segments: neither is the enumeration the token was minted in.
    for (case, mut body) in [
        (
            "another source of the same workspace",
            request(Some(second_source), None, 1_700_000_000, 1_700_000_400),
        ),
        (
            "a narrower window",
            request(Some(first_source), None, 1_700_000_150, 1_700_000_400),
        ),
    ] {
        body["nextToken"] = json!(token);
        let (status, refused) = call(&router, "POST", &uri, Some(body)).await;
        assert_token_refused(case, status, &refused);
    }
}

// ---------------------------------------------------------------------------
// The BATCH readers, which page by a POSITION in the entry list of the request.
//
// A position is meaningful only in the list it is a position in: honoured against
// another list of at least that length, it reports that list's leading entries as
// "completely processed by previous paginated requests" and drops their data from
// this page and from every later one. So the token is bound to the entry list that
// minted it, exactly as the readers above bind theirs to the enumeration they page.
// ---------------------------------------------------------------------------

/// Where `BatchGetAssetPropertyAggregates` is bound: `POST
/// /properties/batch/aggregates`, from `smithy_query(query="operation",
/// operation="BatchGetAssetPropertyAggregates")`.
const BATCH_AGGREGATES: &str = "/properties/batch/aggregates";

/// Where `BatchGetAssetPropertyValueHistory` is bound: `POST
/// /properties/batch/history`.
const BATCH_HISTORY: &str = "/properties/batch/history";

/// The member a `BatchGetAssetPropertyAggregates` success entry carries its data
/// points in.
const AGGREGATED_VALUES: &str = "aggregatedValues";

/// The member a `BatchGetAssetPropertyValueHistory` success entry carries its data
/// points in.
const VALUE_HISTORY: &str = "assetPropertyValueHistory";

/// The table seconds this suite's batch tables write a sample at: far enough apart
/// that each lands in its own `1m` window, whatever the anchor.
const BATCH_SAMPLES: [(i64, Option<&str>); 3] = [(0, None), (120, None), (240, None)];

/// One `BatchGetAssetPropertyAggregates` entry over the window the batch tables
/// write in.
fn aggregates_entry(entry_id: &str, asset_id: &str, property: &str) -> Value {
    json!({
        "entryId": entry_id,
        "assetId": asset_id,
        "propertyId": property,
        "aggregateTypes": ["COUNT"],
        "resolution": "1m",
        "startDate": anchored(-60),
        "endDate": anchored(300),
    })
}

/// One `BatchGetAssetPropertyValueHistory` entry over the same window.
fn history_entry(entry_id: &str, asset_id: &str, property: &str) -> Value {
    json!({
        "entryId": entry_id,
        "assetId": asset_id,
        "propertyId": property,
        "startDate": anchored(-60),
        "endDate": anchored(300),
    })
}

/// The window start one aggregate reports, in epoch seconds (the grid is aligned to
/// the Unix epoch, not to this suite's own anchor).
fn aggregate_window(point: &Value) -> i64 {
    point["timestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("an aggregate carries its window start: {point}")) as i64
}

/// The instant one reported sample carries, as a table second.
fn sample_instant(point: &Value) -> i64 {
    relative(
        point["timestamp"]["timeInSeconds"]
            .as_i64()
            .unwrap_or_else(|| panic!("a sample carries its timestamp: {point}")),
    )
}

/// Every `(entryId, data point instant)` a batch page reports, in the order it
/// reports them.
fn batch_points(
    case: &str,
    body: &Value,
    member: &str,
    instant: fn(&Value) -> i64,
) -> Vec<(String, i64)> {
    body["successEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no successEntries in {body}"))
        .iter()
        .flat_map(|success| {
            let entry_id = success["entryId"].as_str().unwrap_or_default().to_string();
            success[member]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |point| (entry_id.clone(), instant(&point)))
        })
        .collect()
}

/// `entries` with the `member` of the entry at `at` replaced by `value`.
fn with_member(entries: &[Value], at: usize, member: &str, value: Value) -> Vec<Value> {
    let mut changed = entries.to_vec();
    changed[at][member] = value;
    changed
}

/// Drive one batch reader against the rule: a `nextToken` resumes the entry list
/// that minted it — whatever page size the next request asks for — and is refused,
/// with no page and no entry reported as already processed, against any other list.
async fn assert_a_batch_token_binds_to_its_entry_list(
    router: &MultiServiceRouter,
    reader: &str,
    uri: &str,
    member: &'static str,
    instant: fn(&Value) -> i64,
    entries: &[Value],
    other_lists: Vec<(String, Vec<Value>)>,
) {
    // The unpaginated answer is the oracle for what this entry list enumerates.
    let (status, whole) = call(
        router,
        "POST",
        uri,
        Some(json!({"entries": entries.to_vec()})),
    )
    .await;
    assert_eq!(status, 200, "case {reader:?}: got {status} {whole}");
    let points = batch_points(reader, &whole, member, instant);
    assert!(
        points.len() >= 4,
        "case {reader:?}: the table must enumerate several data points per entry, got {points:?}"
    );

    // One data point per page, so the first page mints a token resuming INSIDE the
    // first entry.
    let (status, first) = call(
        router,
        "POST",
        uri,
        Some(json!({"entries": entries.to_vec(), "maxResults": 1})),
    )
    .await;
    assert_eq!(status, 200, "case {reader:?} first page: got {status} {first}");
    assert_eq!(
        batch_points(reader, &first, member, instant),
        points[..1].to_vec(),
        "case {reader:?} first page: expected the first data point only, got {first}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {reader:?} first page: expected a nextToken, got {first}"))
        .to_string();

    // Honoured by the list that minted it, whatever the page size: the page SIZE
    // decides where a page ends, not which entries are enumerated. WHICH data
    // points a resumed page carries depends on that size — one `maxResults` budget
    // is spread across the entries — but it always carries something new, and
    // nothing the first page already reported.
    for size in [None, Some(1), Some(3), Some(99)] {
        let case = format!("{reader}: resumed with maxResults={size:?}");
        let mut request = json!({"entries": entries.to_vec(), "nextToken": token});
        if let Some(size) = size {
            request["maxResults"] = json!(size);
        }
        let (status, resumed) = call(router, "POST", uri, Some(request)).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {resumed}");
        let reported = batch_points(&case, &resumed, member, instant);
        assert!(
            !reported.is_empty(),
            "case {case:?}: the list that minted the token is served from it, got {resumed}"
        );
        assert!(
            !reported.contains(&points[0]),
            "case {case:?}: a resumed page never repeats what the first page carried, \
             got {reported:?}"
        );
        assert!(
            reported.iter().all(|point| points.contains(point)),
            "case {case:?}: every data point of a resumed page is one this entry list \
             enumerates, got {reported:?}"
        );
    }

    // ... and the whole walk of that list still reports every data point of every
    // entry exactly once, in that entry's own order.
    let mut collected: Vec<(String, i64)> = Vec::new();
    let mut walking: Option<String> = None;
    for page in 0..points.len() + 2 {
        let case = format!("{reader}: walking page {page}");
        let mut request = json!({"entries": entries.to_vec(), "maxResults": 1});
        if let Some(token) = &walking {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call(router, "POST", uri, Some(request)).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        collected.extend(batch_points(&case, &body, member, instant));
        walking = body["nextToken"].as_str().map(str::to_string);
        if walking.is_none() {
            break;
        }
    }
    assert!(
        walking.is_none(),
        "case {reader:?}: the walk of one entry list must terminate"
    );
    for entry in entries {
        let entry_id = entry["entryId"].as_str().unwrap_or_default();
        let walked: Vec<i64> = collected
            .iter()
            .filter(|(answered, _)| answered == entry_id)
            .map(|(_, at)| *at)
            .collect();
        let expected: Vec<i64> = points
            .iter()
            .filter(|(answered, _)| answered == entry_id)
            .map(|(_, at)| *at)
            .collect();
        assert_eq!(
            walked, expected,
            "case {reader:?}: paging one entry list must deliver entry {entry_id}'s own \
             run exactly once, in order"
        );
    }
    assert_eq!(
        collected.len(),
        points.len(),
        "case {reader:?}: the walk reports exactly the data points the entry list \
         enumerates, got {collected:?}"
    );

    // Every other entry list is long enough to hold the position the token carries,
    // and every stream it names holds a data point at that position, so each one
    // would be served from it — dropping the entries before it — were the token not
    // bound to the list that minted it.
    for (row, list) in other_lists {
        let case = format!("{reader}: token replayed against {row}");
        let (status, refused) = call(
            router,
            "POST",
            uri,
            Some(json!({"entries": list, "nextToken": token})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a token minted against another entry list, \
             got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused}"
        );
        assert!(
            refused.get("successEntries").is_none() && refused.get("skippedEntries").is_none(),
            "case {case:?}: a refused token must serve no page and report no entry as already \
             processed, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_batch_token_is_honoured_only_by_the_entry_list_that_minted_it() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Fan1", "Speed", "Torque").await;
    let (other_asset, other_speed, other_torque) =
        asset_with_two_properties(&router, "Fan2", "Speed", "Torque").await;

    // The same instants in every stream, so every entry list below enumerates the
    // same number of data points at the same positions: nothing but the binding can
    // tell one list's token from another's.
    write_to_each(
        &router,
        &[
            (&asset_id, &speed),
            (&asset_id, &torque),
            (&other_asset, &other_speed),
            (&other_asset, &other_torque),
        ],
        &BATCH_SAMPLES,
    )
    .await;

    // Rows differ along what the rule generalises over: the ORDER of the entries,
    // the entry ids, the streams they name, the length of the list, and the members
    // that decide what one entry enumerates (its window, its ordering, its quality
    // filter and — for the aggregate reader — its resolution and aggregate types).
    for (reader, uri, member, instant, build) in [
        (
            "BatchGetAssetPropertyAggregates",
            BATCH_AGGREGATES,
            AGGREGATED_VALUES,
            aggregate_window as fn(&Value) -> i64,
            aggregates_entry as fn(&str, &str, &str) -> Value,
        ),
        (
            "BatchGetAssetPropertyValueHistory",
            BATCH_HISTORY,
            VALUE_HISTORY,
            sample_instant as fn(&Value) -> i64,
            history_entry as fn(&str, &str, &str) -> Value,
        ),
    ] {
        let entries = vec![
            build("first", &asset_id, &speed),
            build("second", &asset_id, &torque),
        ];
        let mut reversed = entries.clone();
        reversed.reverse();
        let mut other_lists: Vec<(String, Vec<Value>)> = vec![
            ("the same entries in the other order".to_string(), reversed),
            (
                "the same streams under other entry ids".to_string(),
                with_member(&entries, 0, "entryId", json!("renamed")),
            ),
            (
                "an entry naming another property of the same asset".to_string(),
                with_member(&entries, 0, "propertyId", json!(torque)),
            ),
            (
                "a completely different list of entries".to_string(),
                vec![
                    build("third", &other_asset, &other_speed),
                    build("fourth", &other_asset, &other_torque),
                ],
            ),
            (
                "a shorter list".to_string(),
                entries[..1].to_vec(),
            ),
            (
                "a longer list".to_string(),
                entries
                    .iter()
                    .cloned()
                    .chain([build("third", &other_asset, &other_speed)])
                    .collect(),
            ),
            (
                "an entry over a narrower window".to_string(),
                with_member(&entries, 0, "startDate", json!(anchored(60))),
            ),
            (
                "an entry with the ordering flipped".to_string(),
                with_member(&entries, 1, "timeOrdering", json!("DESCENDING")),
            ),
            (
                "an entry carrying a quality filter".to_string(),
                with_member(&entries, 1, "qualities", json!(["GOOD"])),
            ),
        ];
        if uri == BATCH_AGGREGATES {
            // Members only the aggregate entry carries, each of which changes how
            // many data points its entry enumerates or what they reduce to.
            other_lists.push((
                "an entry at a coarser resolution".to_string(),
                with_member(&entries, 0, "resolution", json!("1h")),
            ));
            other_lists.push((
                "an entry asking for another aggregate type".to_string(),
                with_member(&entries, 0, "aggregateTypes", json!(["SUM"])),
            ));
        }

        assert_a_batch_token_binds_to_its_entry_list(
            &router, reader, uri, member, instant, &entries, other_lists,
        )
        .await;
    }
}

#[tokio::test]
async fn a_batch_token_is_not_a_token_of_the_other_batch_reader() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Hoist1", "Speed", "Torque").await;
    write_to_each(
        &router,
        &[(&asset_id, &speed), (&asset_id, &torque)],
        &BATCH_SAMPLES,
    )
    .await;

    // The two readers page the same two streams over the same window, so a position
    // in one enumeration exists in the other. A token carries the OPERATION it was
    // sent to beside the entry list, so it is only a token of that operation.
    let aggregates = vec![
        aggregates_entry("first", &asset_id, &speed),
        aggregates_entry("second", &asset_id, &torque),
    ];
    let history = vec![
        history_entry("first", &asset_id, &speed),
        history_entry("second", &asset_id, &torque),
    ];
    for (case, minting_uri, minting_entries, replayed_uri, replayed_entries) in [
        (
            "an aggregates token replayed against the history reader",
            BATCH_AGGREGATES,
            &aggregates,
            BATCH_HISTORY,
            &history,
        ),
        (
            "a history token replayed against the aggregates reader",
            BATCH_HISTORY,
            &history,
            BATCH_AGGREGATES,
            &aggregates,
        ),
    ] {
        let (status, first) = call(
            &router,
            "POST",
            minting_uri,
            Some(json!({"entries": minting_entries.clone(), "maxResults": 1})),
        )
        .await;
        assert_eq!(status, 200, "minting {case:?}: got {status} {first}");
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("minting {case:?}: expected a nextToken, got {first}"))
            .to_string();

        let (status, refused) = call(
            &router,
            "POST",
            replayed_uri,
            Some(json!({"entries": replayed_entries.clone(), "nextToken": token})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// HOW a refused `nextToken` is worded.
//
// A refusal carries one of two fixed texts, and which one says what the caller
// should do about it: text this service never issued as a token cannot be resumed
// at all, while a token of its own replayed with other query filtering parameters
// means the walk was restarted under another query. Neither text carries any part
// of the request — not the token, not a count of entries — so a client keying on
// the message reads the same sentence whatever it sent.
// ---------------------------------------------------------------------------

/// What a time-series read answers for text it never issued as a token. Recorded
/// behaviour for the wording; the error is the modelled `InvalidRequestException`
/// (400).
const UNISSUED_MESSAGE: &str = "Invalid next token provided";

/// What a time-series read answers for a token it DID issue, replayed with other
/// query filtering parameters.
const FILTER_MISMATCH_MESSAGE: &str =
    "The provided next token does not match query filtering parameters";

/// What `ExecuteQuery` answers for either — the query reader words its own pagination
/// refusal.
const QUERY_TOKEN_MESSAGE: &str = "Invalid pagination token encountered";

/// Where `BatchGetAssetPropertyValue` is bound: `POST /properties/batch/latest`.
const BATCH_LATEST: &str = "/properties/batch/latest";

/// The `InterpolationType` the interpolated read's rows below ask for. Source:
/// `smithy_query(query="doc", shape="GetInterpolatedAssetPropertyValuesRequest",
/// member="type")` — LINEAR_INTERPOLATION | LOCF_INTERPOLATION.
const LINEAR: &str = "LINEAR_INTERPOLATION";

/// Assert a refusal is 400 `InvalidRequestException` carrying exactly `message`, and
/// that the message is that fixed text alone: it never echoes `token`.
fn assert_refusal_message(case: &str, status: u16, body: &Value, message: &str, token: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a token this read cannot resolve, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "case {case:?}: expected the message {message:?}, got {body}"
    );
    assert!(
        !body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(token),
        "case {case:?}: the message is fixed text and never echoes the token {token:?}, \
         got {body}"
    );
}

#[tokio::test]
async fn text_this_service_never_issued_as_a_token_is_refused_in_one_fixed_wording() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Valve1", "Speed", "Torque").await;
    write_to_each(
        &router,
        &[(&asset_id, &speed), (&asset_id, &torque)],
        &[(0, None), (120, None), (240, None)],
    )
    .await;

    // Rows are tokens no read of this service ever issued, each inside `NextToken`'s
    // modelled `^[A-Za-z0-9+/=]+$` pattern so it reaches the handler's own check
    // rather than the runtime's constraint check: base64 text of several lengths,
    // padded and unpadded, and hex that spells no resume point of any enumeration.
    let forged = [
        "c2ltMnJlYWxuZXZlcmlzc3VlZA==",
        "AAAA",
        "ZZZ=",
        "0123456789abcdef",
    ];
    let history_stream = format!("assetId={asset_id}&propertyId={speed}");
    let interpolated = format!(
        "assetId={asset_id}&propertyId={speed}&startTimeInSeconds={}\
         &endTimeInSeconds={}&quality=GOOD&intervalInSeconds=60&type={LINEAR}",
        anchored(0),
        anchored(240)
    );
    let batch_history = vec![history_entry("one", &asset_id, &speed)];
    let batch_aggregates = vec![aggregates_entry("one", &asset_id, &speed)];
    let batch_latest = vec![json!({
        "entryId": "one", "assetId": asset_id, "propertyId": speed
    })];

    for token in forged {
        // The single-property reads, each driven over its own binding.
        let (status, body) = history(
            &router,
            &format!("{history_stream}&nextToken={token}"),
        )
        .await;
        assert_refusal_message(
            &format!("GetAssetPropertyValueHistory/{token}"),
            status,
            &body,
            UNISSUED_MESSAGE,
            token,
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!(
                "/properties/aggregates?{}&nextToken={token}",
                aggregates_query(&asset_id, &speed, "1m", -60, 300)
            ),
            None,
        )
        .await;
        assert_refusal_message(
            &format!("GetAssetPropertyAggregates/{token}"),
            status,
            &body,
            UNISSUED_MESSAGE,
            token,
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/interpolated?{interpolated}&nextToken={token}"),
            None,
        )
        .await;
        assert_refusal_message(
            &format!("GetInterpolatedAssetPropertyValues/{token}"),
            status,
            &body,
            UNISSUED_MESSAGE,
            token,
        );

        // ... and the batch reads, whose tokens are positions in an entry list.
        for (reader, uri, entries) in [
            (
                "BatchGetAssetPropertyValue",
                BATCH_LATEST,
                batch_latest.clone(),
            ),
            (
                "BatchGetAssetPropertyValueHistory",
                BATCH_HISTORY,
                batch_history.clone(),
            ),
            (
                "BatchGetAssetPropertyAggregates",
                BATCH_AGGREGATES,
                batch_aggregates.clone(),
            ),
        ] {
            let (status, body) = call(
                &router,
                "POST",
                uri,
                Some(json!({"entries": entries, "nextToken": token})),
            )
            .await;
            assert_refusal_message(
                &format!("{reader}/{token}"),
                status,
                &body,
                UNISSUED_MESSAGE,
                token,
            );
        }

        // The query reader words it its own way, for the same token.
        let (status, body) = call(
            &router,
            "POST",
            "/queries/execution",
            Some(json!({
                "queryStatement": "SELECT property_id FROM asset_property",
                "nextToken": token,
            })),
        )
        .await;
        assert_refusal_message(
            &format!("ExecuteQuery/{token}"),
            status,
            &body,
            QUERY_TOKEN_MESSAGE,
            token,
        );
    }
}

#[tokio::test]
async fn a_token_replayed_with_other_query_filtering_parameters_says_so() {
    let router = crate::asset_lifecycle::router();
    let (asset_id, speed, torque) =
        asset_with_two_properties(&router, "Valve2", "Speed", "Torque").await;
    write_to_each(
        &router,
        &[(&asset_id, &speed), (&asset_id, &torque)],
        &[(0, None), (120, None), (240, None)],
    )
    .await;

    // Rows are the ways one request can differ from the request that minted the
    // token while still enumerating something: another property, the opposite
    // ordering, and a window the minting read did not carry. Each is a token this
    // service DID issue, so each earns the filter-mismatch wording rather than the
    // unissued one.
    let minting = format!("assetId={asset_id}&propertyId={speed}");
    let token = mint_history_token(&router, "the plain read", &minting).await;
    for (case, query) in [
        (
            "another property of the same asset",
            format!("assetId={asset_id}&propertyId={torque}"),
        ),
        (
            "the ordering flipped",
            format!("{minting}&timeOrdering=DESCENDING"),
        ),
        (
            "a start bound the minting read did not carry",
            format!("{minting}&startDate={}", query_date(60)),
        ),
    ] {
        let (status, body) = history(&router, &format!("{query}&nextToken={token}")).await;
        assert_refusal_message(
            &format!("GetAssetPropertyValueHistory: {case}"),
            status,
            &body,
            FILTER_MISMATCH_MESSAGE,
            &token,
        );
    }

    // The aggregate reader binds its tokens the same way, so it refuses one of its
    // own the same way.
    let minting = aggregates_query(&asset_id, &speed, "1m", -60, 300);
    let (status, first) = call(
        &router,
        "GET",
        &format!("/properties/aggregates?{minting}&maxResults=1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "minting an aggregates token: got {status} {first}");
    let aggregates_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting an aggregates token: got {first}"))
        .to_string();
    let (status, body) = call(
        &router,
        "GET",
        &format!(
            "/properties/aggregates?{}&nextToken={aggregates_token}",
            aggregates_query(&asset_id, &torque, "1m", -60, 300)
        ),
        None,
    )
    .await;
    assert_refusal_message(
        "GetAssetPropertyAggregates: another property of the same asset",
        status,
        &body,
        FILTER_MISMATCH_MESSAGE,
        &aggregates_token,
    );

    // A batch token is a position in the entry list it was minted for, so another
    // entry list is another set of query filtering parameters.
    let minting_entries = vec![history_entry("one", &asset_id, &speed)];
    let (status, first) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({"entries": minting_entries, "maxResults": 1})),
    )
    .await;
    assert_eq!(status, 200, "minting a batch token: got {status} {first}");
    let batch_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting a batch token: got {first}"))
        .to_string();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_HISTORY,
        Some(json!({
            "entries": [history_entry("one", &asset_id, &torque)],
            "nextToken": batch_token,
        })),
    )
    .await;
    assert_refusal_message(
        "BatchGetAssetPropertyValueHistory: an entry naming another property",
        status,
        &body,
        FILTER_MISMATCH_MESSAGE,
        &batch_token,
    );

    // And a query token belongs to its statement: replayed with another one it is
    // refused in the query reader's own wording.
    let (status, first) = call(
        &router,
        "POST",
        "/queries/execution",
        Some(json!({
            "queryStatement": "SELECT property_id FROM asset_property",
            "maxResults": 1,
        })),
    )
    .await;
    assert_eq!(status, 200, "minting a query token: got {status} {first}");
    let query_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting a query token: got {first}"))
        .to_string();
    let (status, body) = call(
        &router,
        "POST",
        "/queries/execution",
        Some(json!({
            "queryStatement": "SELECT asset_id FROM asset_property",
            "nextToken": query_token,
        })),
    )
    .await;
    assert_refusal_message(
        "ExecuteQuery: a token issued for another statement",
        status,
        &body,
        QUERY_TOKEN_MESSAGE,
        &query_token,
    );
}
