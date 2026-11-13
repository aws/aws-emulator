// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetMetricWidgetImage`: the PNG it answers with is a graph of the
//! datapoints `PutMetricData` stored, drawn at the size the widget definition names, over the
//! window and granularity it names.
//!
//! Every suite here drives the service over the wire it speaks (`awsJson1_0`, `POST /` with
//! `X-Amz-Target`), decodes the base64 blob a client receives and decodes the PNG inside it,
//! so an answer that is not an image computed from the shared store cannot pass.
//!
//! The image's PIXELS are what the rules are asserted over, never a recorded byte string: an
//! image is a function of the store and the definition, so the tables state what must make two
//! images differ (a datapoint, a window, a value, a size, a view, a theme) and what must leave
//! them identical (the same definition over the same store, and a definition whose expression
//! names the series it would have drawn anyway).

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, error_message, now_secs, ok};

/// The namespace every series in this suite is published under.
const NAMESPACE: &str = "MyApp/WidgetImage";

/// The metric most rows graph.
const METRIC: &str = "Latency";

/// The granularity the graphs here are drawn at — the default a definition naming no `period`
/// takes, so the rows that leave it out and the rows that name it lay the same windows.
const PERIOD: f64 = 300.0;

/// The first eight bytes of every PNG (the signature in the PNG specification).
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The shape a refusal of this service names in the body of a JSON answer: the
/// request-validation class, whose legacy query code (`ValidationError`) travels in the
/// `x-amzn-query-error` header instead (`query_compatible_error_envelope`).
const REFUSAL_CODE: &str = "ValidationException";

/// A period-aligned instant an hour before now: recent enough to sit inside CloudWatch's own
/// retention at this granularity, and aligned so the windows a row places datapoints in do not
/// depend on where in a period the suite happens to run.
fn base_time() -> f64 {
    (now_secs() / PERIOD).floor() * PERIOD - 12.0 * PERIOD
}

/// `secs` as the absolute ISO 8601 instant a definition's `start`/`end` carries.
fn absolute(secs: f64) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let millis = (secs * 1_000.0).round() as i64;
    aws_smithy_types::DateTime::from_millis(millis)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant inside the timestamp type's range")
}

/// Publish `(offset in seconds from `base`, value)` into the `Service=api` series of `metric`.
async fn publish(
    router: &MultiServiceRouter,
    metric: &str,
    base: f64,
    points: &[(f64, f64)],
) {
    let data: Vec<Value> = points
        .iter()
        .map(|(offset, value)| {
            json!({
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "Value": value,
                "Unit": "Milliseconds",
                "Timestamp": base + offset,
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

/// One `metrics` entry naming the `Service=api` series of `metric`, with `options` merged into
/// its options object.
fn entry(metric: &str, options: Value) -> Value {
    let mut entry = json!([NAMESPACE, metric, "Service", "api"]);
    let listed = entry.as_array_mut().expect("the entry is an array");
    listed.push(options);
    entry
}

/// A definition over one metric, with `members` merged into it.
fn widget_over(metric: &str, members: &Value) -> Value {
    let mut widget = json!({"metrics": [entry(metric, json!({}))]});
    merge(&mut widget, members);
    widget
}

/// Merge the members of `patch` into `into`, so a row states only what it changes.
fn merge(into: &mut Value, patch: &Value) {
    let (Some(into), Some(patch)) = (into.as_object_mut(), patch.as_object()) else {
        return;
    };
    for (member, value) in patch {
        into.insert(member.clone(), value.clone());
    }
}

/// Render `widget` and return the PNG bytes a client receives.
async fn rendered(router: &MultiServiceRouter, case: &str, widget: &Value) -> Vec<u8> {
    let body = ok(
        router,
        "GetMetricWidgetImage",
        &json!({"MetricWidget": widget.to_string()}),
    )
    .await;
    let encoded = body
        .get("MetricWidgetImage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no MetricWidgetImage: {body}"));
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: MetricWidgetImage must be base64-encoded, got {error}")
        })
}

/// The image's dimensions and its RGB pixels, decoded with the maintained PNG decoder.
fn decoded(case: &str, image: &[u8]) -> (u32, u32, Vec<u8>) {
    assert_eq!(
        image.get(..PNG_MAGIC.len()),
        Some(&PNG_MAGIC[..]),
        "case {case:?}: expected the PNG signature, got {:?}",
        image.get(..PNG_MAGIC.len())
    );
    let mut reader = png::Decoder::new(image)
        .read_info()
        .unwrap_or_else(|error| panic!("case {case:?}: the image must decode as a PNG, got {error}"));
    let mut pixels = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .unwrap_or_else(|error| panic!("case {case:?}: the image must carry a frame, got {error}"));
    assert_eq!(
        info.color_type,
        png::ColorType::Rgb,
        "case {case:?}: expected an RGB image, got {:?}",
        info.color_type
    );
    pixels.truncate(info.buffer_size());
    (info.width, info.height, pixels)
}

/// How many pixels of `pixels` carry exactly `color`.
fn painted(pixels: &[u8], color: [u8; 3]) -> usize {
    pixels
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|pixel| **pixel == color)
        .count()
}

/// Every definition is drawn at the size it names, and a definition naming none is drawn at
/// the documented default.
///
/// The rows differ along what the size rule generalises over: a definition naming neither
/// side, one naming both, the smallest graph there is, and the largest side this emulator
/// renders in each direction.
#[tokio::test]
async fn a_definition_is_rendered_at_the_size_it_names() {
    let router = build_router();
    let base = base_time();
    publish(&router, METRIC, base, &[(0.0, 10.0), (300.0, 20.0)]).await;

    // (the case, the size members the definition carries, the pixels the image must be)
    // "width — The width of the graph in pixels. The default is 600." / "height — … The
    // default is 395." (the metric widget structure reference), and 2000 is the largest side
    // this emulator renders.
    let rows: [(&str, Value, (u32, u32)); 5] = [
        ("no size named", json!({}), (600, 395)),
        ("both sides named", json!({"width": 320, "height": 240}), (320, 240)),
        ("the smallest graph", json!({"width": 1, "height": 1}), (1, 1)),
        ("the widest graph", json!({"width": 2000, "height": 20}), (2000, 20)),
        ("the tallest graph", json!({"width": 20, "height": 2000}), (20, 2000)),
    ];

    for (case, members, expected) in rows {
        let widget = widget_over(METRIC, &members);
        let image = rendered(&router, case, &widget).await;
        let (width, height, _) = decoded(case, &image);
        assert_eq!(
            (width, height),
            expected,
            "case {case:?}: expected a {expected:?} image, got {:?}",
            (width, height)
        );
    }
}

/// The graph is of the datapoints the store holds: two stores that differ in their datapoints
/// render different images, and the same store renders the same image.
///
/// The rows differ along what the datapoints can differ in — how many there are, which windows
/// they fall in, and which values they take — and every row's image must be distinct from
/// every other's, because each row's graph is of its own data. The row with no datapoint at
/// all is one of them: an empty store draws the empty graph rather than another row's.
///
/// The value axis is FIXED for every row (`yAxis.left`), so a row that differs only in the
/// value its datapoint took is a different graph rather than the same shape placed on the
/// axis the data itself chose.
#[tokio::test]
async fn the_graph_is_drawn_from_the_datapoints_the_store_holds() {
    // (the case, the datapoints published as `(offset from the base, value)`)
    let rows: [(&str, &[(f64, f64)]); 6] = [
        ("no datapoint at all", &[]),
        ("one datapoint", &[(150.0, 40.0)]),
        ("one datapoint of another value", &[(150.0, 90.0)]),
        ("one datapoint in another window", &[(1_650.0, 40.0)]),
        ("three rising datapoints", &[(150.0, 10.0), (450.0, 40.0), (750.0, 90.0)]),
        ("three falling datapoints", &[(150.0, 90.0), (450.0, 40.0), (750.0, 10.0)]),
    ];

    let base = base_time();
    // One absolute window and one fixed axis for every row, so the graphs differ only in the
    // data they are of.
    let window = json!({
        "start": absolute(base),
        "end": absolute(base + 12.0 * PERIOD),
        "width": 300,
        "height": 200,
        "yAxis": {"left": {"min": 0, "max": 100}},
    });

    let mut images: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for (case, points) in rows {
        let router = build_router();
        if !points.is_empty() {
            publish(&router, METRIC, base, points).await;
        }
        let widget = widget_over(METRIC, &window);
        let image = rendered(&router, case, &widget).await;
        let again = rendered(&router, case, &widget).await;
        assert_eq!(
            image, again,
            "case {case:?}: the same definition over the same store must render the same \
             bytes, got {} then {} bytes",
            image.len(),
            again.len()
        );
        for (other, drawn) in &images {
            assert_ne!(
                &image, drawn,
                "case {case:?}: a graph of these datapoints must differ from the graph of \
                 {other:?}, got the same {} bytes",
                image.len()
            );
        }
        images.insert(case, image);
    }
}

/// Only the datapoints INSIDE the graph's window are drawn: a window that does not reach a
/// datapoint renders exactly the image an empty store renders, and one that reaches it does
/// not.
///
/// The rows differ along where the window sits relative to the one published datapoint — over
/// it, before it, after it, and over it from the relative form a definition states a window in
/// (`start: -PT3H`) — which is the class the window rule generalises over.
#[tokio::test]
async fn only_the_datapoints_inside_the_window_are_graphed() {
    let base = base_time();
    let published = base + 150.0;

    let with_data = build_router();
    publish(&with_data, METRIC, base, &[(150.0, 42.0)]).await;
    let empty = build_router();

    // (the case, the window members, whether the window reaches the datapoint)
    let rows: [(&str, Value, bool); 4] = [
        (
            "a window over the datapoint",
            json!({"start": absolute(published - 600.0), "end": absolute(published + 600.0)}),
            true,
        ),
        (
            "a window that ends before the datapoint",
            json!({"start": absolute(published - 3_600.0), "end": absolute(published - 1_200.0)}),
            false,
        ),
        (
            "a window that starts after the datapoint",
            json!({"start": absolute(published + 1_200.0), "end": absolute(published + 3_600.0)}),
            false,
        ),
        (
            "the relative window of the last three hours",
            json!({"start": "-PT3H", "end": "P0D"}),
            true,
        ),
    ];

    for (case, window, reaches) in rows {
        let mut members = window;
        merge(&mut members, &json!({"width": 300, "height": 200}));
        let widget = widget_over(METRIC, &members);
        let drawn = rendered(&with_data, case, &widget).await;
        let nothing = rendered(&empty, case, &widget).await;
        assert_eq!(
            drawn == nothing,
            !reaches,
            "case {case:?}: expected the graph of the published datapoint to {} the graph of \
             an empty store",
            if reaches { "differ from" } else { "equal" }
        );
    }
}

/// A series is drawn in the colour its entry names, and the plot the definition's `view` asks
/// for and the theme it names change the image.
///
/// The rows differ along the members that decide how the same resolved series is DRAWN, which
/// is the class this rule generalises over: the colour of the series, the plot, and the theme
/// the graph sits on.
#[tokio::test]
async fn how_the_series_is_drawn_follows_the_definition() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        METRIC,
        base,
        &[(150.0, 10.0), (450.0, 40.0), (750.0, 90.0)],
    )
    .await;

    let window = json!({
        "start": absolute(base),
        "end": absolute(base + 12.0 * PERIOD),
        "width": 300,
        "height": 200,
    });

    // "color — The six-digit HTML hex color code to be used for this metric" (the metric
    // widget structure reference): a colour no theme of this renderer draws in, so every
    // pixel carrying it was drawn for this series.
    let colored = {
        let mut widget = json!({"metrics": [entry(METRIC, json!({"color": "#123456"}))]});
        merge(&mut widget, &window);
        widget
    };
    let image = rendered(&router, "a named colour", &colored).await;
    let (_, _, pixels) = decoded("a named colour", &image);
    let count = painted(&pixels, [0x12, 0x34, 0x56]);
    assert!(
        count > 0,
        "case {:?}: the series must be drawn in the colour its entry names, got {count} pixels \
         of it",
        "a named colour"
    );

    // (the case, the members that change how the graph is drawn)
    let rows: [(&str, Value); 7] = [
        ("the default plot on the default theme", json!({})),
        ("the column plot", json!({"view": "bar"})),
        ("the dark theme", json!({"theme": "dark"})),
        ("another colour", json!({"metrics": [entry(METRIC, json!({"color": "#654321"}))]})),
        // "annotations — … horizontal is an array of horizontal annotations, each with a
        // value" (the metric widget structure reference): a line at a value the axis spans.
        (
            "a horizontal annotation inside the axis",
            json!({"annotations": {"horizontal": [{"value": 50}]}}),
        ),
        (
            "a horizontal annotation at another value",
            json!({"annotations": {"horizontal": [{"value": 20}]}}),
        ),
        // "stat — The statistic to use for this metric": the same datapoints folded by
        // another statistic over another granularity are another series, so another graph.
        (
            "another statistic over another granularity",
            json!({
                "metrics": [entry(METRIC, json!({"stat": "Maximum", "period": 900}))],
            }),
        ),
    ];

    let mut images: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for (case, members) in rows {
        let mut widget = widget_over(METRIC, &window);
        merge(&mut widget, &members);
        let image = rendered(&router, case, &widget).await;
        for (other, drawn) in &images {
            assert_ne!(
                &image, drawn,
                "case {case:?}: this graph must differ from {other:?}, got the same {} bytes",
                image.len()
            );
        }
        images.insert(case, image);
    }
}

/// An entry carrying an expression is EVALUATED over the entries it refers to: an expression
/// that names a series draws the graph that series draws, and one that transforms it does not.
///
/// The value axis is fixed by `yAxis.left`, so the rows are compared on the values the series
/// takes rather than on the shape an auto-scaled axis would make identical.
#[tokio::test]
async fn an_expression_entry_is_evaluated_over_the_series_it_refers_to() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        METRIC,
        base,
        &[(150.0, 20.0), (450.0, 20.0), (750.0, 20.0)],
    )
    .await;

    let frame = json!({
        "start": absolute(base),
        "end": absolute(base + 12.0 * PERIOD),
        "width": 300,
        "height": 200,
        "yAxis": {"left": {"min": 0, "max": 100}},
    });
    let drawn_in = json!({"color": "#1f77b4", "id": "m1"});

    /// The definition for a row: the entries it graphs, inside the shared frame.
    fn definition(entries: Value, frame: &Value) -> Value {
        let mut widget = json!({"metrics": entries});
        merge(&mut widget, frame);
        widget
    }

    let metric_only = definition(json!([entry(METRIC, drawn_in.clone())]), &frame);
    let through_expression = definition(
        json!([
            entry(METRIC, json!({"id": "m1", "visible": false})),
            [{"expression": "m1", "id": "e1", "color": "#1f77b4"}],
        ]),
        &frame,
    );
    let transformed = definition(
        json!([
            entry(METRIC, json!({"id": "m1", "visible": false})),
            [{"expression": "m1 * 2", "id": "e1", "color": "#1f77b4"}],
        ]),
        &frame,
    );

    let plain = rendered(&router, "the metric itself", &metric_only).await;
    let referred = rendered(&router, "an expression naming it", &through_expression).await;
    let doubled = rendered(&router, "an expression doubling it", &transformed).await;

    assert_eq!(
        plain,
        referred,
        "case {:?}: an expression that names the series must draw the graph the series draws, \
         got {} and {} bytes",
        "an expression naming it",
        plain.len(),
        referred.len()
    );
    assert_ne!(
        plain,
        doubled,
        "case {:?}: an expression that doubles the series must draw a different graph on a \
         fixed axis, got the same {} bytes",
        "an expression doubling it",
        plain.len()
    );
}

/// Every `view` the metric widget structure documents renders the graph, and the plot the
/// renderer draws follows it: a column plot for `bar`, and the time-series line for the views
/// whose single-figure layout this renderer does not lay out.
///
/// The rows are the documented set (`view — The default is timeSeries. Valid values:
/// timeSeries | singleValue | gauge | bar | pie`), so the DEVIATION is recorded as well as the
/// rule: `singleValue`, `gauge` and `pie` are served with the graph of the same resolved
/// series rather than refused, and only `bar` draws its own plot.
#[tokio::test]
async fn every_documented_view_renders_the_graph_of_the_same_series() {
    let router = build_router();
    let base = base_time();
    publish(
        &router,
        METRIC,
        base,
        &[(150.0, 10.0), (450.0, 40.0), (750.0, 90.0)],
    )
    .await;

    let window = json!({
        "start": absolute(base),
        "end": absolute(base + 12.0 * PERIOD),
        "width": 300,
        "height": 200,
    });

    let line = {
        let widget = widget_over(METRIC, &window);
        rendered(&router, "the default view", &widget).await
    };

    // (the case, the `view` the definition names, whether it draws its own plot)
    let rows: [(&str, &str, bool); 5] = [
        ("the time-series view", "timeSeries", false),
        ("the column view", "bar", true),
        ("the single-value view", "singleValue", false),
        ("the gauge view", "gauge", false),
        ("the pie view", "pie", false),
    ];

    for (case, view, own_plot) in rows {
        let mut widget = widget_over(METRIC, &window);
        merge(&mut widget, &json!({"view": view}));
        let image = rendered(&router, case, &widget).await;
        let (width, height, _) = decoded(case, &image);
        assert_eq!(
            (width, height),
            (300, 200),
            "case {case:?}: every documented view renders a 300x200 graph, got {:?}",
            (width, height)
        );
        assert_eq!(
            image != line,
            own_plot,
            "case {case:?}: expected this view to {} the time-series plot",
            if own_plot {
                "draw a plot of its own rather than"
            } else {
                "draw"
            }
        );
    }
}

/// A definition this service cannot render is REFUSED, in CloudWatch's own refusal code and
/// naming the member of the definition that has to change.
///
/// The rows span the classes a definition can be wrong in: a payload that is not a widget
/// document, a payload past the published size, a document naming no metric or more metrics
/// than one graph holds, a size, a granularity, a window, an entry and an image format outside
/// the values their members take.
#[tokio::test]
async fn a_definition_this_service_cannot_render_is_refused() {
    let router = build_router();

    // "Up to 100 KB uncompressed payload." (`GetMetricWidgetImage`) — a document past it,
    // built by padding a member the renderer ignores rather than by repeating a metric.
    let oversized = {
        let mut widget = widget_over(METRIC, &json!({}));
        merge(&mut widget, &json!({"title": "t".repeat(100 * 1024)}));
        widget.to_string()
    };
    // "As many as 100 metrics in the graph." (`GetMetricWidgetImage`) — one entry past it.
    let too_many_metrics = json!({
        "metrics": (1..=101)
            .map(|index| entry(&format!("{METRIC}{index}"), json!({})))
            .collect::<Vec<Value>>(),
    })
    .to_string();

    // (the case, the request body, the message the refusal must carry)
    let rows: [(&str, Value, &str); 14] = [
        (
            "a payload that is not JSON",
            json!({"MetricWidget": "not a document"}),
            "The field MetricWidget must be a valid JSON object",
        ),
        (
            "a payload larger than one call carries",
            json!({"MetricWidget": oversized}),
            "The field MetricWidget must not be larger than 102400 bytes",
        ),
        (
            "more metrics than one graph holds",
            json!({"MetricWidget": too_many_metrics}),
            "The collection MetricWidget.metrics must not have a size greater than 100.",
        ),
        (
            "a payload that is a JSON array",
            json!({"MetricWidget": "[]"}),
            "The field MetricWidget must be a valid JSON object",
        ),
        (
            "a document naming no metrics",
            json!({"MetricWidget": "{}"}),
            "The parameter MetricWidget.metrics is required.",
        ),
        (
            "a document naming an empty metrics array",
            json!({"MetricWidget": json!({"metrics": []}).to_string()}),
            "The parameter MetricWidget.metrics is required.",
        ),
        (
            "a width of no pixels",
            json!({"MetricWidget": widget_over(METRIC, &json!({"width": 0})).to_string()}),
            "The value 0 for parameter MetricWidget.width is invalid.",
        ),
        (
            "a graph wider than this emulator renders",
            json!({"MetricWidget": widget_over(METRIC, &json!({"width": 5000})).to_string()}),
            "The value 5000 for parameter MetricWidget.width is invalid.",
        ),
        (
            "a view no metric widget has",
            json!({"MetricWidget": widget_over(METRIC, &json!({"view": "donut"})).to_string()}),
            "The parameter MetricWidget.view must be a value in the set [timeSeries, \
             singleValue, gauge, bar, pie].",
        ),
        (
            "a granularity no metric is readable at",
            json!({"MetricWidget": widget_over(METRIC, &json!({"period": 7})).to_string()}),
            "The parameter Period must be a value in the set [1, 5, 10, 20, 30].",
        ),
        (
            "a window that ends where it starts",
            json!({"MetricWidget": widget_over(
                METRIC,
                &json!({"start": "-PT1H", "end": "-PT1H"}),
            ).to_string()}),
            "The parameter MetricWidget.end must name an instant later than \
             MetricWidget.start.",
        ),
        (
            "a horizontal annotation naming no value",
            json!({"MetricWidget": json!({
                "metrics": [entry(METRIC, json!({}))],
                "annotations": {"horizontal": [{"label": "ceiling"}]},
            }).to_string()}),
            "The parameter MetricWidget.annotations.horizontal.member.1.value is required.",
        ),
        (
            "an entry that is not an array",
            json!({"MetricWidget": json!({"metrics": ["AWS/EC2"]}).to_string()}),
            "The value \"AWS/EC2\" for parameter MetricWidget.metrics.member.1 is invalid.",
        ),
        (
            "an image format no graph is rendered in",
            json!({
                "MetricWidget": widget_over(METRIC, &json!({})).to_string(),
                "OutputFormat": "jpeg",
            }),
            "The parameter OutputFormat must be a value in the set [png, image/png].",
        ),
    ];

    for (case, body, expected) in rows {
        let (status, answered) = call(&router, "GetMetricWidgetImage", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {answered}"
        );
        assert_eq!(
            error_code(&answered),
            REFUSAL_CODE,
            "case {case:?}: expected the {REFUSAL_CODE} code, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            error_message(&answered)
        );
    }
}

/// Both documented `OutputFormat` values render the graph, and so does a request naming none.
///
/// "Only PNG images are supported. The default is png." — the two values name the same image,
/// so every row answers the same PNG of the same store.
#[tokio::test]
async fn every_documented_output_format_renders_the_png() {
    let router = build_router();
    let base = base_time();
    publish(&router, METRIC, base, &[(150.0, 25.0)]).await;
    let widget = widget_over(
        METRIC,
        &json!({
            "start": absolute(base),
            "end": absolute(base + 12.0 * PERIOD),
            "width": 300,
            "height": 200,
        }),
    );

    let rows: [(&str, Option<&str>); 3] = [
        ("no format named", None),
        ("the default format", Some("png")),
        ("the custom-request format", Some("image/png")),
    ];

    let mut first: Option<Vec<u8>> = None;
    for (case, format) in rows {
        let mut body = json!({"MetricWidget": widget.to_string()});
        if let Some(format) = format {
            merge(&mut body, &json!({"OutputFormat": format}));
        }
        let answered = ok(&router, "GetMetricWidgetImage", &body).await;
        let encoded = answered
            .get("MetricWidgetImage")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no MetricWidgetImage in {answered}"));
        let image = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap_or_else(|error| panic!("case {case:?}: expected base64, got {error}"));
        let (width, height, _) = decoded(case, &image);
        assert_eq!(
            (width, height),
            (300, 200),
            "case {case:?}: expected a 300x200 image, got {:?}",
            (width, height)
        );
        match &first {
            None => first = Some(image),
            Some(expected) => assert_eq!(
                &image, expected,
                "case {case:?}: every format renders the same PNG of the same store, got {} \
                 bytes against {} bytes",
                image.len(),
                expected.len()
            ),
        }
    }
}

/// Rendering a graph changes nothing a later caller sees: the series a read reports before the
/// graphs and after them are the same.
///
/// The rows are the reads a caller would notice a write through — the metrics the store lists
/// and the datapoints it reports — asserted across several renders, including ones that would
/// have had to write to answer (a window reaching past the data, and an expression over it).
#[tokio::test]
async fn rendering_a_graph_writes_nothing() {
    let router = build_router();
    let base = base_time();
    publish(&router, METRIC, base, &[(150.0, 10.0), (450.0, 40.0)]).await;

    let listed = ok(
        &router,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE}),
    )
    .await;
    let statistics = json!({
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "StartTime": base,
        "EndTime": base + 12.0 * PERIOD,
        "Period": PERIOD,
        "Statistics": ["Sum", "Average", "SampleCount"],
    });
    let before = ok(&router, "GetMetricStatistics", &statistics).await;

    for (case, members) in [
        ("the default window", json!({})),
        (
            "an absolute window",
            json!({"start": absolute(base), "end": absolute(base + 12.0 * PERIOD)}),
        ),
        (
            "a window reaching past the data",
            json!({"start": "-PT12H", "end": "P0D"}),
        ),
    ] {
        let mut widget = widget_over(METRIC, &members);
        merge(&mut widget, &json!({"width": 200, "height": 150}));
        rendered(&router, case, &widget).await;
    }

    let after = ok(&router, "GetMetricStatistics", &statistics).await;
    assert_eq!(
        before, after,
        "the datapoints a read reports must be unchanged by rendering a graph of them, got \
         {before} then {after}"
    );
    let listed_after = ok(
        &router,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE}),
    )
    .await;
    assert_eq!(
        listed, listed_after,
        "the metrics the store lists must be unchanged by rendering a graph of them, got \
         {listed} then {listed_after}"
    );
}
