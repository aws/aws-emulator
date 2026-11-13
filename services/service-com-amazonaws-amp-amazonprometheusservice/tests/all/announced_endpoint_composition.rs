// SPDX-License-Identifier: CC0-1.0

//! The endpoint a workspace ANNOUNCES is the endpoint its Prometheus API is SERVED on.
//!
//! The rule under test: a URL a client composes from the endpoint a resource announces —
//! the authority it names, joined with a path of the API served there — reaches that API
//! on this emulator, however the client joins the two. A Prometheus client never writes
//! the target out: it reads `WorkspaceDescription$prometheusEndpoint`, which already ends
//! in the API's version prefix, and appends the API's own path to it — so the announcement
//! and the routing have to agree about one path space, or every data-plane client of a
//! workspace is left addressing a route nothing serves.
//!
//! So every URL here is DERIVED from the announcement rather than written out, and the
//! suite spans the table the rule generalises over: the address the announcement was taken
//! over (the announced authority is the one the request reached, so each row announces a
//! different URL), the API path the client appends (with and without the version prefix the
//! announced URL already carries), and whether the client trims the announced trailing
//! slash before joining. Each cell writes a sample through the URL ONE join composes and
//! reads it back through the URL ANOTHER composes, so a pair of joins that addressed two
//! different endpoints — or an endpoint holding its own samples — fails the row.
//!
//! The samples themselves, the PromQL the query leg evaluates and what the plane refuses
//! are the subject of `crate::prometheus_data_plane`, whose request builder and
//! remote-write encoder this suite reuses.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::prometheus_data_plane::{remote_write_body, remote_write_headers, request};
use crate::workspace_lifecycle::{encode_label, text, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    unix_millis, METRIC_NAME_LABEL, PROMETHEUS_API_VERSION,
};

/// The Region every request of this suite is signed for.
const REGION: &str = "us-west-2";

/// The write path of the remote-write protocol, which is the name a sender appends to the
/// endpoint it was handed.
const REMOTE_WRITE: &str = "remote_write";

/// The instant-query path of the Prometheus HTTP API, the name a PromQL caller appends to
/// the endpoint it was handed.
const QUERY: &str = "query";

/// The label every series carries beside the metric name, so a row proves the labels of
/// the series it wrote come back too.
const JOB_LABEL: (&str, &str) = ("job", "composition");

/// One way a client joins the endpoint it was handed with a path of the API served there:
/// the announced PATH and the API path in, the request path out.
type Join = fn(&str, &str) -> String;

/// The announced URL carries the API's version prefix already, and a client that composes
/// its target from the API's own version-prefixed path appends that prefix again: both
/// name the same endpoint of the same API.
fn versioned_on_trimmed(announced_path: &str, api_path: &str) -> String {
    format!(
        "{}/{PROMETHEUS_API_VERSION}/{api_path}",
        announced_path.trim_end_matches('/')
    )
}

/// The same composition from a client that joins onto the announced trailing slash rather
/// than trimming it.
fn versioned_on_slash(announced_path: &str, api_path: &str) -> String {
    format!("{announced_path}{PROMETHEUS_API_VERSION}/{api_path}")
}

/// A client that reads the announced URL as already version-prefixed and appends the
/// endpoint's name alone.
fn named_on_trimmed(announced_path: &str, api_path: &str) -> String {
    format!("{}/{api_path}", announced_path.trim_end_matches('/'))
}

/// The same, joined onto the announced trailing slash.
fn named_on_slash(announced_path: &str, api_path: &str) -> String {
    format!("{announced_path}{api_path}")
}

/// The two joins that compose DIFFERENT paths, each with the name a failing case is
/// reported under. Whether the client trims the announced trailing slash cannot change the
/// target — the announced path ends in exactly one slash — and that is asserted once, by
/// [`a_trimmed_and_a_kept_slash_compose_the_same_target`], rather than by driving every
/// cell twice.
fn joins() -> Vec<(&'static str, Join)> {
    vec![
        (
            "the version-prefixed API path appended",
            versioned_on_trimmed as Join,
        ),
        ("the endpoint name alone appended", named_on_trimmed as Join),
    ]
}

/// The values a row writes, cycled over the cases: a whole number, a negative fraction, a
/// large magnitude and zero, so no row passes on a value the plane could have invented.
const VALUES: &[f64] = &[42.0, -0.5, 1_000_000.0, 0.0];

/// The endpoint `DescribeWorkspace` announces to a request addressed to `host`.
async fn announced_endpoint(amp: &Amp, host: &str, workspace_id: &str) -> String {
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
        status,
        200,
        "host {host:?}: the workspace must be described, got {:?}",
        String::from_utf8_lossy(&answered)
    );
    let described: serde_json::Value = serde_json::from_slice(&answered).unwrap_or_else(|error| {
        panic!(
            "host {host:?}: the describe answer must be JSON ({error}): {:?}",
            String::from_utf8_lossy(&answered)
        )
    });
    let announced = text(&described["workspace"]["prometheusEndpoint"]);
    assert!(
        !announced.is_empty(),
        "host {host:?}: the workspace must announce a Prometheus endpoint, got {described:?}"
    );
    announced
}

/// The announced URL split the way a client reads it: the authority a request to it is
/// addressed to, and the path beneath that authority. Parsed with the URI type the request
/// builder itself takes, so this suite states no URL rule of its own.
fn authority_and_path(host: &str, announced: &str) -> (String, String) {
    let uri: http::Uri = announced.parse().unwrap_or_else(|error| {
        panic!("host {host:?}: the announced endpoint must be a URL ({error}): {announced:?}")
    });
    let authority = uri
        .authority()
        .unwrap_or_else(|| {
            panic!("host {host:?}: the announced endpoint must name an authority: {announced:?}")
        })
        .to_string();
    (authority, uri.path().to_string())
}

/// The labels a case expects on the one element of the answer: the metric name the write
/// carried, under the label the format reserves for it, and the series' own label.
fn expected_labels(metric: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (METRIC_NAME_LABEL.to_string(), metric.to_string()),
        (JOB_LABEL.0.to_string(), JOB_LABEL.1.to_string()),
    ])
}

/// A sample written through a URL composed from the announced endpoint is read back
/// through a URL composed from that same announcement — whichever address the
/// announcement was taken over, and whichever of the joins a client builds either URL
/// with.
#[tokio::test]
async fn a_url_composed_from_the_announced_endpoint_reaches_the_served_api() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "announced-composition").await;
    let joins = joins();
    let mut case_index = 0_usize;

    // One row per address a caller can reach this emulator on: the loopback address and
    // port it serves, a host carrying no port, and this API's own regional endpoint.
    for host in [
        "127.0.0.1:9297",
        "localhost",
        "aps-workspaces.us-west-2.amazonaws.com",
    ] {
        let announced = announced_endpoint(&amp, host, &workspace_id).await;
        let (authority, announced_path) = authority_and_path(host, &announced);
        assert_eq!(
            authority, host,
            "host {host:?}: the announced endpoint must name the address this request \
             reached the service on, got {announced:?}"
        );

        for (write_join, compose_write) in &joins {
            for (read_join, compose_read) in &joins {
                let case = format!("{host} / wrote by {write_join} / read by {read_join}");
                // One series per cell, so a cell reports the sample IT wrote.
                let metric = format!("composed_round_trip_{case_index}");
                let value = VALUES[case_index % VALUES.len()];
                case_index += 1;
                let written_at = unix_millis(SystemTime::now());
                let labels = [JOB_LABEL];
                let samples = [(written_at, value)];
                let body = remote_write_body(&[(metric.as_str(), &labels, &samples)]);

                let write_path = compose_write(&announced_path, REMOTE_WRITE);
                let (status, _headers, answered) = amp
                    .send(request(
                        "POST",
                        &authority,
                        REGION,
                        &write_path,
                        &remote_write_headers(),
                        body,
                    ))
                    .await;
                assert!(
                    [200, 202, 204].contains(&status),
                    "case {case:?}: a remote-write request to {write_path:?} must be \
                     accepted, got HTTP {status} {:?}",
                    String::from_utf8_lossy(&answered)
                );

                let read_path = compose_read(&announced_path, QUERY);
                let (status, _headers, answered) = amp
                    .send(request(
                        "GET",
                        &authority,
                        REGION,
                        &format!("{read_path}?query={}", encode_label(&metric)),
                        &[],
                        Vec::new(),
                    ))
                    .await;
                assert_eq!(
                    status,
                    200,
                    "case {case:?}: an instant query to {read_path:?} must answer 200, got \
                     HTTP {status} {:?}",
                    String::from_utf8_lossy(&answered)
                );
                let document: serde_json::Value = serde_json::from_slice(&answered)
                    .unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: the query answer must be JSON ({error}): {:?}",
                            String::from_utf8_lossy(&answered)
                        )
                    });
                assert_eq!(
                    text(&document["status"]),
                    "success",
                    "case {case:?}: the answer must be a success, got {document:?}"
                );
                assert_eq!(
                    text(&document["data"]["resultType"]),
                    "vector",
                    "case {case:?}: an instant query answers a vector, got {document:?}"
                );
                let elements = document["data"]["result"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: the result is a list: {document:?}"));
                assert_eq!(
                    elements.len(),
                    1,
                    "case {case:?}: the selector names the one series this cell wrote, got \
                     {document:?}"
                );
                let reported_labels: BTreeMap<String, String> = elements[0]["metric"]
                    .as_object()
                    .map(|map| {
                        map.iter()
                            .map(|(name, value)| (name.clone(), text(value)))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported_labels,
                    expected_labels(&metric),
                    "case {case:?}: the element must carry the labels the write carried, got \
                     {document:?}"
                );
                let reported_value = text(&elements[0]["value"][1])
                    .parse::<f64>()
                    .unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: a sample value is a number ({error}): {document:?}"
                        )
                    });
                assert_eq!(
                    reported_value, value,
                    "case {case:?}: the query must report the value that was written \
                     ({value}), got {reported_value}"
                );
                let reported_at = elements[0]["value"][0].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: a sample timestamp is a number: {document:?}")
                });
                assert!(
                    (reported_at - written_at as f64 / 1_000.0).abs() < 0.001,
                    "case {case:?}: the sample must be reported at the instant it was \
                     written ({written_at} ms), got {reported_at}"
                );
            }
        }
    }
}

/// Trimming the announced trailing slash before joining, or joining onto it, composes the
/// SAME target — which is why the round-trip table above drives each composition once.
/// The announcement ends in exactly one slash, so the two client habits cannot address
/// different endpoints; this holds for every API path a client appends.
#[tokio::test]
async fn a_trimmed_and_a_kept_slash_compose_the_same_target() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "announced-slash").await;
    let announced = announced_endpoint(&amp, "localhost", &workspace_id).await;
    let (_, announced_path) = authority_and_path("localhost", &announced);
    assert!(
        announced_path.ends_with('/') && !announced_path.ends_with("//"),
        "the announced path must end in exactly one slash, got {announced_path:?}"
    );

    for api_path in [REMOTE_WRITE, QUERY] {
        for (case, trimmed, kept) in [
            (
                "the version-prefixed API path",
                versioned_on_trimmed as Join,
                versioned_on_slash as Join,
            ),
            (
                "the endpoint name alone",
                named_on_trimmed as Join,
                named_on_slash as Join,
            ),
        ] {
            let left = trimmed(&announced_path, api_path);
            let right = kept(&announced_path, api_path);
            assert_eq!(
                left, right,
                "case {case:?} / {api_path:?}: trimming the announced slash and joining onto \
                 it must compose the same target, got {left:?} and {right:?}"
            );
        }
    }
}
