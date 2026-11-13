// SPDX-License-Identifier: CC0-1.0

//! The Prometheus data plane a workspace announces: the endpoint `DescribeWorkspace`
//! reports, and the remote_write / instant-query API served beneath it.
//!
//! The rule under test: a resource's announced endpoint names the address the REQUEST
//! reached the service on, and the API announced there is served — a remote-write request
//! writes samples to that workspace and an instant query evaluates the caller's own PromQL
//! against them. Each suite below asserts that over a TABLE of inputs: several hosts for
//! the announcement, several metric names, label sets, values and both of the URL
//! compositions a Prometheus client builds for the write, and a catalog of PromQL
//! expressions (selector, every parameterless aggregation operator, both grouping
//! modifiers, a scalar) for the read.
//!
//! The write body is built here the way a remote_write sender builds it: a
//! `prometheus.WriteRequest` protobuf document compressed with snappy's block format —
//! encoded through the same message definitions the service decodes it with
//! (`state::remote_write`), so the test exercises the wire format rather than an internal
//! shortcut.

use bytes::Bytes;
use prost::Message;
use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::workspace_lifecycle::{
    announced_prometheus_endpoint, text, wait_for_due_transition, Amp,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    promql::{MAX_MATCHERS, MAX_NESTING_DEPTH, MAX_QUERY_BYTES},
    remote_write::{WireLabel, WireSample, WireTimeSeries, WriteRequest},
    unix_millis, DEFAULT_RETENTION_PERIOD_IN_DAYS, METRIC_NAME_LABEL,
};

/// The Region every request of this suite is signed for, unless a case names another.
const REGION: &str = "us-west-2";

/// One label set and the samples written to it, as a case states them.
///
/// `pub(crate)`, with [`request`], [`remote_write_body`] and [`remote_write_headers`],
/// because `crate::announced_endpoint_composition` drives the same data plane over the
/// same wire format: a second copy of the request builder or of the remote-write encoder
/// would be a second set of rules about how a test reaches that API.
pub(crate) type SeriesCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [(i64, f64)]);

/// One round-trip case: its name, the metric name written, that series' labels and the
/// value.
type RoundTripCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], f64);

/// One element of an instant vector as a case compares it: the labels and the value.
type VectorElement = (BTreeMap<String, String>, f64);

/// One PromQL case: its name, the expression, and the elements the answer must carry.
type QueryCase<'a> = (&'a str, &'a str, Vec<VectorElement>);

/// One write-path refusal case: its name, the method, the path, the body, and the status
/// and reason the plane must answer.
type WriteRefusalCase<'a> = (&'a str, &'a str, String, Vec<u8>, u16, &'a str);

/// One request to this emulator, addressed to `host` and signed for `region` — the two
/// facts the announced endpoint and the Region scoping are read from.
pub(crate) fn request(
    method: &str,
    host: &str,
    region: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> http::Request<Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/aps/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder
        .body(Bytes::from(body))
        .expect("a well-formed test request")
}

/// A remote-write body, built as a sender builds it: the protobuf `WriteRequest` of the
/// stated series, compressed with snappy's block format.
pub(crate) fn remote_write_body(series: &[SeriesCase<'_>]) -> Vec<u8> {
    let request = WriteRequest {
        timeseries: series
            .iter()
            .map(|(metric, labels, samples)| WireTimeSeries {
                labels: std::iter::once(WireLabel {
                    name: METRIC_NAME_LABEL.to_string(),
                    value: (*metric).to_string(),
                })
                .filter(|label| !label.value.is_empty())
                .chain(labels.iter().map(|(name, value)| WireLabel {
                    name: (*name).to_string(),
                    value: (*value).to_string(),
                }))
                .collect(),
                samples: samples
                    .iter()
                    .map(|(timestamp, value)| WireSample {
                        value: *value,
                        timestamp: *timestamp,
                    })
                    .collect(),
            })
            .collect(),
    };
    snappy(&request.encode_to_vec())
}

/// Snappy's block format, which is what `Content-Encoding: snappy` names.
fn snappy(raw: &[u8]) -> Vec<u8> {
    snap::raw::Encoder::new()
        .compress_vec(raw)
        .expect("snappy compresses any input")
}

/// The headers a remote_write sender announces its body with.
pub(crate) fn remote_write_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("content-encoding", "snappy"),
        ("content-type", "application/x-protobuf"),
        ("x-prometheus-remote-write-version", "0.1.0"),
    ]
}

/// `POST <endpoint>api/v1/remote_write` — the target a Prometheus client composes by
/// joining the ANNOUNCED endpoint (which already ends in the version prefix) with the
/// API's own version-prefixed path.
fn joined_write_path(workspace_id: &str) -> String {
    format!("/workspaces/{workspace_id}/api/v1/api/v1/remote_write")
}

/// `POST <endpoint>remote_write` — the same endpoint, composed by appending the API's name
/// to the announced endpoint.
fn direct_write_path(workspace_id: &str) -> String {
    format!("/workspaces/{workspace_id}/api/v1/remote_write")
}

/// The instant-query path of a workspace's announced endpoint.
fn query_path(workspace_id: &str, query: &str) -> String {
    format!(
        "/workspaces/{workspace_id}/api/v1/query?query={}",
        crate::workspace_lifecycle::encode_label(query)
    )
}

/// Now, in the milliseconds a sample's timestamp is carried in.
fn now_ms() -> i64 {
    unix_millis(SystemTime::now())
}

/// Write `series` to the data-plane `path` and answer the status and the body the plane
/// returned.
async fn write(
    amp: &Amp,
    region: &str,
    path: &str,
    series: &[SeriesCase<'_>],
) -> (u16, String) {
    let body = remote_write_body(series);
    let (status, _headers, answered) = amp
        .send(request(
            "POST",
            &host_of(region),
            region,
            path,
            &remote_write_headers(),
            body,
        ))
        .await;
    (status, String::from_utf8_lossy(&answered).trim().to_string())
}

/// Read `query` back from `workspace_id`'s announced endpoint: the status and the
/// Prometheus API document the plane answered.
async fn query(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    query: &str,
) -> (u16, serde_json::Value) {
    let (status, _headers, answered) = amp
        .send(request(
            "GET",
            &host_of(region),
            region,
            &query_path(workspace_id, query),
            &[],
            Vec::new(),
        ))
        .await;
    let document = serde_json::from_slice(&answered).unwrap_or_else(|error| {
        panic!(
            "the query answer must be JSON ({error}): {:?}",
            String::from_utf8_lossy(&answered)
        )
    });
    (status, document)
}

/// The host a request of this suite is addressed to, which is the regional endpoint the
/// rest of this target addresses.
fn host_of(region: &str) -> String {
    format!("aps.{region}.amazonaws.com")
}

/// `query` sent as a `POST` form body, which is how a client sends an expression too long
/// for a URI.
async fn post_query(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    expression: &str,
) -> (u16, serde_json::Value) {
    let (status, _headers, answered) = amp
        .send(request(
            "POST",
            &host_of(region),
            region,
            &format!("/workspaces/{workspace_id}/api/v1/query"),
            &[("content-type", "application/x-www-form-urlencoded")],
            format!(
                "query={}",
                crate::workspace_lifecycle::encode_label(expression)
            )
            .into_bytes(),
        ))
        .await;
    let document = serde_json::from_slice(&answered).unwrap_or_else(|error| {
        panic!(
            "the query answer must be JSON ({error}): {:?}",
            String::from_utf8_lossy(&answered)
        )
    });
    (status, document)
}

/// The elements of an instant-vector answer as a case compares them: each series' labels
/// and its value, in the one order [`sorted`] puts them in, so the order the plane
/// happened to walk the store in never decides whether a case passes.
fn vector_of(document: &serde_json::Value) -> Vec<VectorElement> {
    let elements: Vec<VectorElement> = document["data"]["result"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|element| {
            let labels = element["metric"]
                .as_object()
                .map(|map| {
                    map.iter()
                        .map(|(name, value)| (name.clone(), text(value)))
                        .collect()
                })
                .unwrap_or_default();
            let value = text(&element["value"][1])
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("a sample value must be a number: {element:?}"));
            (labels, value)
        })
        .collect();
    sorted(elements)
}

/// One stable order for the elements of an instant vector, applied to what a case expects
/// and to what the plane answered alike.
fn sorted(mut elements: Vec<VectorElement>) -> Vec<VectorElement> {
    elements.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.partial_cmp(&right.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    elements
}

/// The label set a case expects on one element of the answer.
fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// `DescribeWorkspace` announces the endpoint the REQUEST reached the service on, keeping
/// the documented `/workspaces/<workspaceId>/api/v1/` path — so a client that follows the
/// announced URL reaches this emulator rather than another address.
#[tokio::test]
async fn the_announced_endpoint_names_the_host_the_request_reached() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "announced").await;

    // One row per shape of endpoint a caller can reach this emulator on: the loopback
    // address and port the emulator serves, a host carrying no port, this service's
    // regional endpoint, and a host that is not this service's at all.
    for host in [
        "127.0.0.1:9297",
        "localhost",
        "aps.us-west-2.amazonaws.com",
        "aps-workspaces.us-west-2.amazonaws.com",
        "emulator.internal:8443",
    ] {
        let (status, _headers, answered) = amp
            .send(request(
                "GET",
                host,
                REGION,
                &format!("/workspaces/{workspace_id}"),
                &[("content-type", "application/json")],
                Vec::new(),
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {host:?}: the workspace must be described, got {:?}",
            String::from_utf8_lossy(&answered)
        );
        let described: serde_json::Value =
            serde_json::from_slice(&answered).expect("the describe answer is JSON");
        assert_eq!(
            text(&described["workspace"]["prometheusEndpoint"]),
            format!("https://{host}/workspaces/{workspace_id}/api/v1/"),
            "case {host:?}: the announced endpoint must name the host this request \
             reached the service on, got {described:?}"
        );
    }
}

/// A sample written to the announced endpoint is read back from it, whichever of the two
/// paths a client composes — and whatever the metric name, the labels and the value.
#[tokio::test]
async fn a_written_sample_is_read_back_from_the_announced_endpoint() {
    let amp = Amp::new();

    // (case, metric name, labels, value)
    let rows: &[RoundTripCase<'_>] = &[
        ("a plain name and one label", "up", &[("job", "prometheus")], 42.0),
        (
            "a name carrying colons and no label",
            "http:requests:rate5m",
            &[],
            0.5,
        ),
        (
            "a negative value and two labels",
            "queue_depth",
            &[("queue", "ingest"), ("az", "usw2-az1")],
            -17.25,
        ),
        (
            "a leading underscore and a large value",
            "_internal_batches",
            &[("instance", "10.0.0.1:9090")],
            1_000_000.0,
        ),
    ];

    for (case, metric, series_labels, value) in rows {
        let workspace_id = amp.active_workspace(REGION, "round-trip").await;
        let announced = {
            let described = amp.describe(REGION, &workspace_id).await;
            text(&described.body["workspace"]["prometheusEndpoint"])
        };
        assert_eq!(
            announced,
            announced_prometheus_endpoint(REGION, &workspace_id),
            "case {case:?}: the announced endpoint must be the one this request reached"
        );

        for (composition, path) in [
            ("joined with the API's version-prefixed path", joined_write_path(&workspace_id)),
            ("the API's name appended", direct_write_path(&workspace_id)),
        ] {
            let written_at = now_ms();
            let (status, body) = write(
                &amp,
                REGION,
                &path,
                &[(metric, series_labels, &[(written_at, *value)])],
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?} / {composition:?}: the write must be accepted, got {body:?}"
            );

            let (status, document) = query(&amp, REGION, &workspace_id, metric).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {composition:?}: the query must answer 200, got {document:?}"
            );
            assert_eq!(
                text(&document["status"]),
                "success",
                "case {case:?} / {composition:?}: the answer must be a success, got {document:?}"
            );
            assert_eq!(
                text(&document["data"]["resultType"]),
                "vector",
                "case {case:?} / {composition:?}: an instant query answers a vector, got \
                 {document:?}"
            );
            let mut expected_labels = labels(series_labels);
            expected_labels.insert(METRIC_NAME_LABEL.to_string(), (*metric).to_string());
            assert_eq!(
                vector_of(&document),
                vec![(expected_labels, *value)],
                "case {case:?} / {composition:?}: the query must report the sample that was \
                 written, got {document:?}"
            );
            let reported_at = document["data"]["result"][0]["value"][0]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: a timestamp is a number: {document:?}"));
            assert!(
                (reported_at - written_at as f64 / 1_000.0).abs() < 0.001,
                "case {case:?} / {composition:?}: the sample must be reported at the instant \
                 it was written ({written_at} ms), got {reported_at}"
            );
        }
    }
}

/// The instant query evaluates the PromQL the request carries: a selector with each
/// matcher operator, every parameterless aggregation operator with and without a grouping
/// modifier, and a scalar.
///
/// The operator catalog is the one `RandomCutForestConfiguration$query` publishes for this
/// service ("Supported PromQL aggregation operators: `avg`, `count`, `group`, `max`,
/// `min`, `quantile`, `stddev`, `stdvar`, and `sum`"), less `quantile`, which takes a
/// parameter and is covered by the refusal suite below.
#[tokio::test]
async fn the_instant_query_evaluates_the_expression_it_carries() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "promql").await;
    let at = now_ms();
    let seed: &[SeriesCase<'_>] = &[
        ("up", &[("job", "api"), ("instance", "one")], &[(at, 1.0)]),
        ("up", &[("job", "api"), ("instance", "two")], &[(at, 3.0)]),
        ("up", &[("job", "db"), ("instance", "three")], &[(at, 8.0)]),
        ("requests", &[("job", "api")], &[(at, 5.0)]),
    ];
    let (status, body) = write(
        &amp,
        REGION,
        &direct_write_path(&workspace_id),
        seed,
    )
    .await;
    assert_eq!(status, 204, "the seeding write must be accepted, got {body:?}");

    let up = |job: &str, instance: &str| {
        labels(&[
            (METRIC_NAME_LABEL, "up"),
            ("job", job),
            ("instance", instance),
        ])
    };

    // (case, expression, the elements the answer must carry)
    let rows: Vec<QueryCase<'_>> = vec![
        (
            "a bare metric name selects every series of it",
            "up",
            vec![
                (up("api", "one"), 1.0),
                (up("api", "two"), 3.0),
                (up("db", "three"), 8.0),
            ],
        ),
        (
            "an equality matcher",
            "up{job=\"api\"}",
            vec![(up("api", "one"), 1.0), (up("api", "two"), 3.0)],
        ),
        (
            "an inequality matcher",
            "up{job!=\"api\"}",
            vec![(up("db", "three"), 8.0)],
        ),
        (
            "a regular-expression matcher",
            "up{instance=~\"on.|two\"}",
            vec![(up("api", "one"), 1.0), (up("api", "two"), 3.0)],
        ),
        (
            "a negated regular-expression matcher",
            "up{instance!~\"on.|two\"}",
            vec![(up("db", "three"), 8.0)],
        ),
        (
            "a matcher alone selects across metric names",
            "{job=\"db\"}",
            vec![(up("db", "three"), 8.0)],
        ),
        (
            "a metric nothing was written to selects nothing",
            "disk_free",
            Vec::new(),
        ),
        ("sum over everything", "sum(up)", vec![(labels(&[]), 12.0)]),
        (
            "sum grouped by a label",
            "sum by (job) (up)",
            vec![(labels(&[("job", "api")]), 4.0), (labels(&[("job", "db")]), 8.0)],
        ),
        (
            "sum grouped without a label",
            "sum without (instance) (up)",
            vec![(labels(&[("job", "api")]), 4.0), (labels(&[("job", "db")]), 8.0)],
        ),
        ("count over everything", "count(up)", vec![(labels(&[]), 3.0)]),
        ("min over everything", "min(up)", vec![(labels(&[]), 1.0)]),
        ("max over everything", "max(up)", vec![(labels(&[]), 8.0)]),
        ("avg over everything", "avg(up)", vec![(labels(&[]), 4.0)]),
        ("group over everything", "group(up)", vec![(labels(&[]), 1.0)]),
        (
            "stdvar over everything",
            "stdvar(up)",
            // The population variance of 1, 3 and 8 around their mean of 4.
            vec![(labels(&[]), 26.0 / 3.0)],
        ),
        (
            "stddev over everything",
            "stddev(up)",
            vec![(labels(&[]), (26.0f64 / 3.0).sqrt())],
        ),
        (
            "an aggregation of a selected subset",
            "sum(up{job=\"api\"})",
            vec![(labels(&[]), 4.0)],
        ),
        (
            "another metric's series are not part of it",
            "requests",
            vec![(labels(&[(METRIC_NAME_LABEL, "requests"), ("job", "api")]), 5.0)],
        ),
    ];

    for (case, expression, expected) in rows {
        let expected = sorted(expected);
        let (status, document) = query(&amp, REGION, &workspace_id, expression).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {expression:?} must be evaluated, got {document:?}"
        );
        let answered = vector_of(&document);
        assert_eq!(
            answered.len(),
            expected.len(),
            "case {case:?}: {expression:?} must answer {} element(s), got {document:?}",
            expected.len()
        );
        for (index, ((answered_labels, answered_value), (labels, value))) in
            answered.iter().zip(expected.iter()).enumerate()
        {
            assert_eq!(
                answered_labels, labels,
                "case {case:?}: element {index} of {expression:?} must carry {labels:?}, got \
                 {document:?}"
            );
            assert!(
                (answered_value - value).abs() < 1e-9,
                "case {case:?}: element {index} of {expression:?} must be {value}, got \
                 {answered_value}"
            );
        }
    }

    // A number literal is PromQL's scalar, and the API answers it as one.
    let (status, document) = query(&amp, REGION, &workspace_id, "42.5").await;
    assert_eq!(status, 200, "a scalar must be evaluated, got {document:?}");
    assert_eq!(
        text(&document["data"]["resultType"]),
        "scalar",
        "a number literal answers a scalar, got {document:?}"
    );
    assert_eq!(
        text(&document["data"]["result"][1]),
        "42.5",
        "the scalar must carry the literal's value, got {document:?}"
    );
}

/// What the data plane refuses, and with which status: one row per rule, each asserted
/// against the endpoint the workspace announces.
#[tokio::test]
async fn the_data_plane_refuses_what_it_cannot_serve() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "refusals").await;
    let at = now_ms();

    // The write path. (case, method, path, headers, body, expected status, expected reason)
    let unparseable = snappy(&[0xff, 0xff, 0xff, 0xff]);
    let write_rows: Vec<WriteRefusalCase<'_>> = vec![
        (
            "a method the endpoint does not serve",
            "GET",
            direct_write_path(&workspace_id),
            Vec::new(),
            405,
            "this endpoint of the Prometheus API serves another method",
        ),
        (
            "a body that is not a snappy block",
            "POST",
            direct_write_path(&workspace_id),
            b"not snappy at all".to_vec(),
            400,
            "snappy decode error",
        ),
        (
            "a snappy body that is not a WriteRequest",
            "POST",
            direct_write_path(&workspace_id),
            unparseable,
            400,
            "unable to decode the remote write request",
        ),
        (
            "a series carrying no metric name",
            "POST",
            direct_write_path(&workspace_id),
            remote_write_body(&[("", &[("job", "api")], &[(at, 1.0)])]),
            400,
            "invalid metric name",
        ),
        (
            "a series carrying a metric name the data model does not allow",
            "POST",
            direct_write_path(&workspace_id),
            remote_write_body(&[("not a name", &[], &[(at, 1.0)])]),
            400,
            "invalid metric name",
        ),
        (
            "a series carrying a label name the data model does not allow",
            "POST",
            direct_write_path(&workspace_id),
            remote_write_body(&[("up", &[("not-a-label", "api")], &[(at, 1.0)])]),
            400,
            "invalid label name",
        ),
        (
            "a write to a workspace this account does not hold",
            "POST",
            direct_write_path("ws-0000000000000000"),
            remote_write_body(&[("up", &[], &[(at, 1.0)])]),
            404,
            "the Prometheus endpoint of this workspace is not available",
        ),
    ];

    for (case, method, path, body, status, reason) in write_rows {
        let (answered, answered_body) = {
            let (answered, _headers, bytes) = amp
                .send(request(
                    method,
                    &host_of(REGION),
                    REGION,
                    &path,
                    &remote_write_headers(),
                    body,
                ))
                .await;
            (answered, String::from_utf8_lossy(&bytes).trim().to_string())
        };
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} {answered_body:?}"
        );
        assert_eq!(
            answered_body, reason,
            "case {case:?}: expected the refusal to say {reason:?}, got {answered_body:?}"
        );
    }

    // The query path. (case, expression, expected status, expected errorType)
    let query_rows: &[(&str, &str, u16, &str)] = &[
        ("an expression that is not PromQL", "up{", 400, "bad_data"),
        ("an empty expression", "", 400, "bad_data"),
        ("a range selector", "up[5m]", 501, "not_implemented"),
        ("a function call", "rate(up[5m])", 501, "not_implemented"),
        ("a binary operation", "up + 1", 501, "not_implemented"),
        (
            "a parameterised aggregation",
            "quantile(0.9, up)",
            501,
            "not_implemented",
        ),
        ("an offset modifier", "up offset 5m", 501, "not_implemented"),
    ];
    for (case, expression, status, error_type) in query_rows {
        let (answered, document) = query(&amp, REGION, &workspace_id, expression).await;
        assert_eq!(
            answered, *status,
            "case {case:?}: expected HTTP {status}, got {document:?}"
        );
        assert_eq!(
            text(&document["status"]),
            "error",
            "case {case:?}: the answer must be an error, got {document:?}"
        );
        assert_eq!(
            text(&document["errorType"]),
            *error_type,
            "case {case:?}: expected errorType {error_type:?}, got {document:?}"
        );
    }

    // An expression past the length this plane parses, and an evaluation instant that is
    // not a timestamp: both are the caller's to fix, so both are the query API's
    // `bad_data`. The long expression travels in a form body, which is the only way a
    // client sends one that long at all.
    let too_long = format!("up{{job=\"{}\"}}", "x".repeat(64 << 10));
    let (status, _headers, answered) = amp
        .send(request(
            "POST",
            &host_of(REGION),
            REGION,
            &format!("/workspaces/{workspace_id}/api/v1/query"),
            &[("content-type", "application/x-www-form-urlencoded")],
            format!("query={}", crate::workspace_lifecycle::encode_label(&too_long)).into_bytes(),
        ))
        .await;
    let document: serde_json::Value =
        serde_json::from_slice(&answered).expect("a JSON answer");
    assert_eq!(
        status, 400,
        "an expression past the parsed length must be refused, got {document:?}"
    );
    assert_eq!(
        text(&document["errorType"]),
        "bad_data",
        "the refusal must be a bad-data, got {document:?}"
    );

    // An expression past a bound on its own size or complexity is refused as `bad_data`
    // too — and the emulator ANSWERS, which is itself part of what these rows assert: a
    // tree as deep as these name is built and dropped by the parser with the call stack,
    // so a process that survived the row is the evidence that the size bound holds.
    //
    // Every way PromQL nests is covered, because only the bracket forms are visible to a
    // bracket counter: brackets, an operator chain, a chain of a binary operator over
    // selectors, and repeated unary operators. The bounds come from `state::promql`'s
    // MAX_QUERY_BYTES, MAX_NESTING_DEPTH and MAX_MATCHERS.
    //
    // `MAX_NESTING_DEPTH` is the depth of the TREE, and a parenthesis is one level of it,
    // so this many parentheses is one level past the bound.
    let deep = format!(
        "{}up{}",
        "(".repeat(MAX_NESTING_DEPTH),
        ")".repeat(MAX_NESTING_DEPTH)
    );
    // Past MAX_QUERY_BYTES, whichever way it nests: refused on the size, before a parser
    // is handed it.
    let very_deep = format!("{}up{}", "(".repeat(30_000), ")".repeat(30_000));
    let very_long_chain = "1+".repeat(30_000) + "1";
    // Inside MAX_QUERY_BYTES and nesting through OPERATORS rather than brackets: each
    // names a tree thousands of levels deep, which the size bound is what keeps
    // parseable, and the depth bound is what refuses.
    let operator_chain = "1+".repeat(MAX_QUERY_BYTES / 4) + "1";
    let selector_chain = "up*".repeat(MAX_QUERY_BYTES / 8) + "up";
    // A unary chain over a SELECTOR: a chain over a number literal is folded by the
    // parser into that literal, so it names no deep tree at all.
    let unary_chain = "- ".repeat(MAX_QUERY_BYTES / 8) + "up";
    let matchers: Vec<String> = (0..=MAX_MATCHERS)
        .map(|index| format!("l{index}=~\"v{index}.*\""))
        .collect();
    let wide = format!("up{{{}}}", matchers.join(","));
    for (case, expression, status, error_type) in [
        ("one level past the nesting bound", &deep, 400, "bad_data"),
        ("brackets past the size bound", &very_deep, 400, "bad_data"),
        ("an operator chain past the size bound", &very_long_chain, 400, "bad_data"),
        ("an operator chain inside the size bound", &operator_chain, 400, "bad_data"),
        ("a chain of selectors", &selector_chain, 400, "bad_data"),
        ("repeated unary operators", &unary_chain, 400, "bad_data"),
        ("one matcher past the matcher bound", &wide, 400, "bad_data"),
    ] {
        let (answered, document) = post_query(&amp, REGION, &workspace_id, expression).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {document:?}"
        );
        assert_eq!(
            text(&document["errorType"]),
            error_type,
            "case {case:?}: expected errorType {error_type:?}, got {document:?}"
        );
    }
    // And the expressions just inside both bounds are evaluated, so neither bound is a
    // refusal of ordinary PromQL.
    let at_the_bound = format!(
        "{}up{}",
        "(".repeat(MAX_NESTING_DEPTH - 1),
        ")".repeat(MAX_NESTING_DEPTH - 1)
    );
    let at_the_matcher_bound = format!("up{{{}}}", matchers[..MAX_MATCHERS - 1].join(","));
    for (case, expression) in [
        ("the deepest nesting evaluated", &at_the_bound),
        ("the widest selector evaluated", &at_the_matcher_bound),
    ] {
        let (status, document) = post_query(&amp, REGION, &workspace_id, expression).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an expression inside the bounds must be evaluated, got \
             {document:?}"
        );
    }


    for stated in ["yesterday", "", "9e99999", "2026-13-45T99:99:99Z"] {
        let uri = format!(
            "/workspaces/{workspace_id}/api/v1/query?query=up&time={}",
            crate::workspace_lifecycle::encode_label(stated)
        );
        let (status, _headers, answered) = amp
            .send(request(
                "GET",
                &host_of(REGION),
                REGION,
                &uri,
                &[],
                Vec::new(),
            ))
            .await;
        let document: serde_json::Value = serde_json::from_slice(&answered)
            .unwrap_or_else(|error| panic!("case {stated:?}: a JSON answer ({error})"));
        // An empty `time` is no value at all, so the query is evaluated at the request's
        // own instant; anything else that is not a timestamp is refused.
        let expected = if stated.is_empty() { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {stated:?}: expected HTTP {expected}, got {document:?}"
        );
    }


    let (status, document) = query(&amp, "eu-north-1", &workspace_id, "up").await;
    assert_eq!(
        status, 404,
        "a workspace of another Region must not be served, got {document:?}"
    );
    assert_eq!(
        text(&document["errorType"]),
        "not_found",
        "the refusal must be a not-found, got {document:?}"
    );
}

/// A sample older than the newest one of its series is refused while the workspace's
/// configured out-of-order window is zero, accepted once the window covers it — and a
/// refused write leaves the workspace holding exactly what it held.
#[tokio::test]
async fn an_out_of_order_sample_is_judged_against_the_configured_window() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "out-of-order").await;
    let at = now_ms();
    let path = direct_write_path(&workspace_id);

    let (status, body) = write(
        &amp,
        REGION,
        &path,
        &[("up", &[("job", "api")], &[(at, 7.0)])],
    )
    .await;
    assert_eq!(status, 204, "the first write must be accepted, got {body:?}");

    // The default window is zero seconds, so a sample behind the newest one is refused.
    let older = at - 60_000;
    let (status, body) = write(
        &amp,
        REGION,
        &path,
        &[("up", &[("job", "api")], &[(older, 1.0)])],
    )
    .await;
    assert_eq!(
        status, 400,
        "an out-of-order sample must be refused while the window is zero, got {body:?}"
    );
    assert_eq!(
        body, "out of order sample",
        "the refusal must say what was wrong, got {body:?}"
    );
    let (_status, document) = query(&amp, REGION, &workspace_id, "up").await;
    assert_eq!(
        vector_of(&document),
        vec![(labels(&[(METRIC_NAME_LABEL, "up"), ("job", "api")]), 7.0)],
        "a refused write must leave the workspace holding what it held, got {document:?}"
    );

    // Configure a window that covers it, and the same sample is written.
    let configured = amp
        .call(
            "PATCH",
            &format!("/workspaces/{workspace_id}/configuration"),
            REGION,
            &serde_json::json!({
                "outOfOrderTimeWindowInSeconds": 600,
                // `UpdateWorkspaceConfiguration` takes at least one of
                // `limitsPerLabelSet` or `retentionPeriodInDays`, so the request states
                // the retention the workspace already had.
                "retentionPeriodInDays": DEFAULT_RETENTION_PERIOD_IN_DAYS,
            })
            .to_string(),
        )
        .await;
    assert!(
        (200..300).contains(&configured.status),
        "the configuration update must be accepted, got {configured:?}"
    );
    wait_for_due_transition();

    let (status, body) = write(
        &amp,
        REGION,
        &path,
        &[("up", &[("job", "api")], &[(older, 1.0)])],
    )
    .await;
    assert_eq!(
        status, 204,
        "a sample inside the configured window must be accepted, got {body:?}"
    );
    // The newest sample is still the one a query reports: the older one sits behind it.
    let (_status, document) = query(&amp, REGION, &workspace_id, "up").await;
    assert_eq!(
        vector_of(&document),
        vec![(labels(&[(METRIC_NAME_LABEL, "up"), ("job", "api")]), 7.0)],
        "the instant query reports the newest sample of the series, got {document:?}"
    );
    // And the older one is reported when the query is evaluated at its own instant.
    let (status, document) = {
        let uri = format!(
            "/workspaces/{workspace_id}/api/v1/query?query=up&time={}",
            older as f64 / 1_000.0
        );
        let (status, _headers, answered) = amp
            .send(request(
                "GET",
                &host_of(REGION),
                REGION,
                &uri,
                &[],
                Vec::new(),
            ))
            .await;
        (
            status,
            serde_json::from_slice::<serde_json::Value>(&answered).expect("a JSON answer"),
        )
    };
    assert_eq!(status, 200, "the query must be evaluated, got {document:?}");
    assert_eq!(
        vector_of(&document),
        vec![(labels(&[(METRIC_NAME_LABEL, "up"), ("job", "api")]), 1.0)],
        "at the older sample's instant the query reports that sample, got {document:?}"
    );
}

/// A workspace serves its Prometheus endpoint only while it is usable: not before the
/// clock has made it `ACTIVE`, and not once it has been deleted — whose removal takes the
/// workspace's series with it.
#[tokio::test]
async fn the_endpoint_is_served_only_while_the_workspace_is_usable() {
    let amp = Amp::new();
    let created = amp
        .call(
            "POST",
            "/workspaces",
            REGION,
            &serde_json::json!({ "alias": "not-yet" }).to_string(),
        )
        .await;
    assert_eq!(created.status, 202, "the create must be accepted: {created:?}");
    let workspace_id = text(&created.body["workspaceId"]);

    // Still CREATING: "Workspace has been created and is usable" is what ACTIVE means, so
    // the endpoint is not there yet.
    let at = now_ms();
    let (status, body) = write(
        &amp,
        REGION,
        &direct_write_path(&workspace_id),
        &[("up", &[], &[(at, 1.0)])],
    )
    .await;
    assert_eq!(
        status, 404,
        "a workspace that is not usable yet serves no endpoint, got {body:?}"
    );

    wait_for_due_transition();
    let (status, body) = write(
        &amp,
        REGION,
        &direct_write_path(&workspace_id),
        &[("up", &[], &[(now_ms(), 1.0)])],
    )
    .await;
    assert_eq!(status, 204, "an active workspace serves it, got {body:?}");

    let deleted = amp.delete(REGION, &workspace_id, None).await;
    assert_eq!(deleted.status, 202, "the delete must be accepted: {deleted:?}");
    wait_for_due_transition();

    let (status, document) = query(&amp, REGION, &workspace_id, "up").await;
    assert_eq!(
        status, 404,
        "a deleted workspace serves no endpoint, got {document:?}"
    );
    // The samples went with the workspace: a workspace created afterwards holds none of
    // them, whatever it is called.
    let replacement = amp.active_workspace(REGION, "not-yet").await;
    let (status, document) = query(&amp, REGION, &replacement, "up").await;
    assert_eq!(status, 200, "the new workspace serves its endpoint, got {document:?}");
    assert_eq!(
        vector_of(&document),
        Vec::new(),
        "a new workspace holds no series of the deleted one, got {document:?}"
    );
}

/// The samples the plane reports are the samples that were written: a value that is not
/// finite, and one written through `POST` with the expression in the form body, are both
/// read back as PromQL's own rendering of them.
#[tokio::test]
async fn the_query_answers_a_post_and_renders_every_value_the_format_carries() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "renderings").await;
    let at = now_ms();

    // (case, the value written, the string the API renders it as)
    let rows: &[(&str, f64, &str)] = &[
        ("a whole number", 3.0, "3"),
        ("a fraction", 0.125, "0.125"),
        ("a negative fraction", -2.5, "-2.5"),
        ("positive infinity", f64::INFINITY, "+Inf"),
        ("negative infinity", f64::NEG_INFINITY, "-Inf"),
        ("not a number", f64::NAN, "NaN"),
    ];
    for (index, (case, value, rendered)) in rows.iter().enumerate() {
        let metric = format!("rendered_{index}");
        let (status, body) = write(
            &amp,
            REGION,
            &direct_write_path(&workspace_id),
            &[(&metric, &[], &[(at, *value)])],
        )
        .await;
        assert_eq!(status, 204, "case {case:?}: the write must be accepted, got {body:?}");

        // The expression travels in the form body of a `POST`, which is how a client
        // sends a long query.
        let (status, _headers, answered) = amp
            .send(request(
                "POST",
                &host_of(REGION),
                REGION,
                &format!("/workspaces/{workspace_id}/api/v1/query"),
                &[("content-type", "application/x-www-form-urlencoded")],
                format!("query={metric}").into_bytes(),
            ))
            .await;
        let document: serde_json::Value =
            serde_json::from_slice(&answered).expect("a JSON answer");
        assert_eq!(
            status, 200,
            "case {case:?}: a POSTed query must be evaluated, got {document:?}"
        );
        assert_eq!(
            text(&document["data"]["result"][0]["value"][1]),
            *rendered,
            "case {case:?}: the value must be rendered as {rendered:?}, got {document:?}"
        );
    }
}
