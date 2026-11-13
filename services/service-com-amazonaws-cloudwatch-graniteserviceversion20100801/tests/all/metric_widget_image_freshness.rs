// SPDX-License-Identifier: CC0-1.0

//! The freshness of the graph `GetMetricWidgetImage` answers with: the image is a function of
//! the widget definition and of the datapoints inside the window it names, and of nothing else.
//!
//! So a repeat call over a FIXED absolute window answers the same bytes while the store holds
//! the same series — a client may cache it — and a datapoint landing inside that window makes
//! the next call answer different bytes. The rule is asserted over the VALUES as well as over
//! the shape: a graph carries the axes its values and instants are read against, so two stores
//! whose series differ only in the values they took are different images, including the pairs a
//! plot normalised to its own data would draw identically (one point of any value, a series
//! shifted by a constant, and one scaled by a factor).
//!
//! Every row drives the service over the wire it speaks (`awsJson1_0`, `POST /` with
//! `X-Amz-Target`) and decodes the base64 blob a client receives, so nothing but the bytes a
//! caller gets is asserted.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok};

/// The namespace every series in this suite is published under.
const NAMESPACE: &str = "MyApp/WidgetFreshness";

/// The metric every row graphs.
const METRIC: &str = "Latency";

/// The granularity the graphs here are drawn at — the default a definition naming no `period`
/// takes ("period — The default is 300.", the metric widget structure reference).
const PERIOD: f64 = 300.0;

/// The first eight bytes of every PNG (the signature in the PNG specification).
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// A period-aligned instant two hours before now: recent enough to sit inside CloudWatch's
/// retention at this granularity, and aligned so the windows a row places datapoints in do not
/// depend on where in a period the suite happens to run.
fn base_time() -> f64 {
    (now_secs() / PERIOD).floor() * PERIOD - 24.0 * PERIOD
}

/// `secs` as the absolute ISO 8601 instant a definition's `start`/`end` carries.
fn absolute(secs: f64) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let millis = (secs * 1_000.0).round() as i64;
    aws_smithy_types::DateTime::from_millis(millis)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant inside the timestamp type's range")
}

/// Publish `(epoch seconds, value)` datapoints into the `Service=api` series of [`METRIC`].
async fn publish(router: &MultiServiceRouter, points: &[(f64, f64)]) {
    let data: Vec<Value> = points
        .iter()
        .map(|(at, value)| {
            json!({
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "Value": value,
                "Unit": "Milliseconds",
                "Timestamp": at,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// A definition graphing [`METRIC`] with `members` merged into it.
fn widget(members: &Value) -> Value {
    let mut widget = json!({
        "metrics": [[NAMESPACE, METRIC, "Service", "api"]],
        "width": 400,
        "height": 240,
    });
    let (Some(into), Some(patch)) = (widget.as_object_mut(), members.as_object()) else {
        return widget;
    };
    for (member, value) in patch {
        into.insert(member.clone(), value.clone());
    }
    widget
}

/// The window members a definition carries for the absolute `[start, end)` of `spans` periods
/// from `start`.
fn absolute_window(start: f64, spans: f64) -> Value {
    json!({"start": absolute(start), "end": absolute(start + spans * PERIOD)})
}

/// Render `definition` and return the PNG bytes a client receives.
async fn rendered(router: &MultiServiceRouter, case: &str, definition: &Value) -> Vec<u8> {
    let body = ok(
        router,
        "GetMetricWidgetImage",
        &json!({"MetricWidget": definition.to_string()}),
    )
    .await;
    let encoded = body
        .get("MetricWidgetImage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no MetricWidgetImage: {body}"));
    let image = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: MetricWidgetImage must be base64-encoded, got {error}")
        });
    assert_eq!(
        image.get(..PNG_MAGIC.len()),
        Some(&PNG_MAGIC[..]),
        "case {case:?}: expected the PNG signature, got {:?}",
        image.get(..PNG_MAGIC.len())
    );
    image
}

/// Render `definition` twice and return the bytes, requiring the two calls to agree: the same
/// definition over the same store is the same graph, which is what makes an answer cacheable.
async fn rendered_stably(router: &MultiServiceRouter, case: &str, definition: &Value) -> Vec<u8> {
    let image = rendered(router, case, definition).await;
    let again = rendered(router, case, definition).await;
    assert_eq!(
        image,
        again,
        "case {case:?}: a repeat call over the same window and store must answer the same \
         bytes, got {} then {} bytes",
        image.len(),
        again.len()
    );
    image
}

/// Over a FIXED absolute window the answer changes exactly when the series inside that window
/// changes.
///
/// The rows differ along where the new datapoint lands relative to the graph's window and
/// relative to the datapoints already in it — the class the freshness rule generalises over:
/// the window the first point is in, another window inside the graph's range, a value that
/// leaves the folded series as it was, and the two sides of the window the graph is of.
#[tokio::test]
async fn a_fixed_window_answers_the_same_bytes_until_its_series_changes() {
    let router = build_router();
    let base = base_time();
    let definition = widget(&absolute_window(base, 4.0));

    publish(&router, &[(base + 30.0, 10.0)]).await;
    let mut current = rendered_stably(&router, "the published series", &definition).await;

    // (the case, the datapoint published, whether the graph must change)
    let rows: [(&str, (f64, f64), bool); 5] = [
        (
            "another value in the window the series is already in",
            (base + 60.0, 90.0),
            true,
        ),
        (
            "a datapoint in another window inside the graph's range",
            (base + 2.0 * PERIOD + 30.0, 70.0),
            true,
        ),
        (
            "a datapoint that leaves the folded series as it was",
            (base + 90.0, 50.0),
            false,
        ),
        (
            "a datapoint after the window the graph is of",
            (base + 10.0 * PERIOD, 42.0),
            false,
        ),
        (
            "a datapoint before the window the graph is of",
            (base - 10.0 * PERIOD, 42.0),
            false,
        ),
    ];

    for (case, point, must_change) in rows {
        publish(&router, &[point]).await;
        let fresher = rendered_stably(&router, case, &definition).await;
        assert_eq!(
            fresher != current,
            must_change,
            "case {case:?}: expected this datapoint to {} the graph, got {} bytes then {} bytes",
            if must_change {
                "re-render"
            } else {
                "leave unchanged"
            },
            current.len(),
            fresher.len()
        );
        current = fresher;
    }
}

/// The graph carries the VALUES its series took, not only their shape: two stores whose series
/// differ in any value render different images.
///
/// The rows are the pairs a plot normalised to the data it drew would otherwise draw
/// identically — a single point of any value, a flat series at any level, a series shifted by a
/// constant and one scaled by a factor — plus the signs and magnitudes a value takes.
#[tokio::test]
async fn the_graph_carries_the_values_its_series_took() {
    let base = base_time();
    let definition = widget(&absolute_window(base, 4.0));

    // (the case, the values published one per window of the graph's range)
    let rows: [(&str, &[f64]); 9] = [
        ("one datapoint", &[10.0]),
        ("one datapoint of a larger value", &[900.0]),
        ("one datapoint of a negative value", &[-10.0]),
        ("one datapoint of a fractional value", &[10.5]),
        ("a rising series", &[10.0, 20.0]),
        ("the same series shifted up", &[15.0, 25.0]),
        ("the same series scaled up", &[20.0, 40.0]),
        ("a flat series", &[20.0, 20.0]),
        ("a flat series at another level", &[50.0, 50.0]),
    ];

    let mut images: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for (case, values) in rows {
        let router = build_router();
        let points: Vec<(f64, f64)> = values
            .iter()
            .enumerate()
            .map(|(window, value)| {
                #[allow(clippy::cast_precision_loss)]
                let offset = window as f64 * PERIOD + 30.0;
                (base + offset, *value)
            })
            .collect();
        publish(&router, &points).await;
        let image = rendered_stably(&router, case, &definition).await;
        for (other, drawn) in &images {
            assert_ne!(
                &image, drawn,
                "case {case:?}: a graph of these values must differ from the graph of \
                 {other:?}, got the same {} bytes",
                image.len()
            );
        }
        images.insert(case, image);
    }
}

/// The graph carries the WINDOW it is of: two renders of the same store over windows that fall
/// on different instants are different images.
///
/// The rows differ along what an absolute window can differ in — where it starts, how far it
/// reaches, and the day it falls on — over one store, so nothing but the window the graph
/// covers can make the bytes differ.
#[tokio::test]
async fn the_graph_carries_the_window_it_is_of() {
    let router = build_router();
    let base = base_time();
    publish(&router, &[(base + 30.0, 10.0)]).await;

    // (the case, the window the definition names)
    let rows: [(&str, Value); 4] = [
        ("the window the series is in", absolute_window(base, 4.0)),
        (
            "a window starting one period later",
            absolute_window(base + PERIOD, 4.0),
        ),
        ("a wider window", absolute_window(base, 8.0)),
        (
            "a window a day earlier",
            absolute_window(base - 24.0 * 60.0 * 60.0, 4.0),
        ),
    ];

    let mut images: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for (case, window) in rows {
        let image = rendered_stably(&router, case, &widget(&window)).await;
        for (other, drawn) in &images {
            assert_ne!(
                &image, drawn,
                "case {case:?}: a graph over this window must differ from the graph over \
                 {other:?}, got the same {} bytes",
                image.len()
            );
        }
        images.insert(case, image);
    }
}

/// A window that includes `now` re-renders as a datapoint lands in it, the same way a fixed
/// window does: the graph is of the series the store holds when the call is answered.
///
/// The window is the relative form a definition states one in (`start: -PT1H`, `end: PT0H`), so
/// the rule is asserted over the form a live dashboard widget uses as well as over the absolute
/// one.
#[tokio::test]
async fn a_window_that_includes_now_re_renders_as_a_datapoint_lands() {
    let router = build_router();
    let definition = widget(&json!({"start": "-PT1H", "end": "PT0H"}));

    let before = rendered(&router, "a live window over an empty store", &definition).await;
    // Inside the last hour and inside a window that has closed, so the publish and the render
    // are of the same window wherever in a period the suite runs.
    let published = (now_secs() / PERIOD).floor() * PERIOD - 2.0 * PERIOD + 30.0;
    publish(&router, &[(published, 42.0)]).await;
    let after = rendered(&router, "a live window over the published series", &definition).await;
    assert_ne!(
        before,
        after,
        "case {:?}: a window that includes now must re-render once a datapoint lands in it, \
         got the same {} bytes",
        "a live window over the published series",
        before.len()
    );
}
