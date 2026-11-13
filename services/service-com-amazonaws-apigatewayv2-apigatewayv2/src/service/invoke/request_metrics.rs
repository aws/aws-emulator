// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series API Gateway publishes about the requests an HTTP api's stage
//! ANSWERED.
//!
//! WHAT IS COUNTED. Every request a deployed stage answered is one observation of
//! [`COUNT_METRIC`], and one observation of each error counter — [`CLIENT_ERROR_METRIC`] for
//! the 4xx class and [`SERVER_ERROR_METRIC`] for the 5xx class (RFC 9110 §15.5, §15.6) — which
//! is [`ONE_REQUEST`] for a request whose status is in that class and [`NO_REQUEST`] for one
//! that is not. Both counters are published for EVERY request, which is what makes the `Sum`
//! over a span how many requests of that class the span held and the `Average` the share of
//! them. A request the gateway itself refused is a request the stage answered, so a route the
//! stage's `RouteSettings` throttled, one an authorizer refused and a path no route matched are
//! counted the same way as one a backend served — the status alone decides which counter it
//! lands in. Beside the counters go the two measurements of the same request: how long
//! answering it took ([`LATENCY_METRIC`]) and how many payload bytes it carried in and out
//! ([`DATA_PROCESSED_METRIC`]).
//!
//! WHICH DIMENSIONS. The stage-level series first: [`API_ID_DIMENSION`] alone, so a reader
//! filtering on the api sees its whole traffic, and [`API_ID_DIMENSION`] with
//! [`STAGE_DIMENSION`], so one stage's traffic is separable from another's. Then — and only
//! for a route whose serving stage asks for it with `RouteSettings$DetailedMetricsEnabled`
//! ("specifies whether detailed metrics are enabled") — the same observations again under the
//! route's own dimensions, [`METHOD_DIMENSION`] and [`RESOURCE_DIMENSION`] naming the two
//! halves of its `RouteKey` ([`route_dimensions`]). The opt-in is the caller's instruction
//! about the metrics and not about the serving, so a stage that asks for nothing still serves
//! its traffic and still publishes the stage-level series.
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same store
//! `ListMetrics`, `GetMetricStatistics` and the alarm evaluation read — reached through the
//! shared engine, so one metric has one home and the publisher and its readers cannot drift.
//! They never travel over `PutMetricData`, which refuses a publish into a namespace the
//! services own ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricDataInput$Namespace`).

use std::time::Duration;

use super::{RtRequest, RtResponse};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace API Gateway reports what the apis it serves did with the requests they
/// answered in: the ONE namespace of the service, which an HTTP api's series share with every
/// other kind of api AWS's API Gateway serves. The api's own identifier is what separates
/// them, which is why [`API_ID_DIMENSION`] is part of every series below.
pub const NAMESPACE: &str = "AWS/ApiGateway";

/// The dimension naming the api whose traffic a datapoint is about — `Api$ApiId`, "the API
/// identifier".
pub const API_ID_DIMENSION: &str = "ApiId";

/// The dimension naming the stage that answered it — `Stage$StageName`.
pub const STAGE_DIMENSION: &str = "Stage";

/// The dimension naming the verb half of the answering route's `RouteKey`.
pub const METHOD_DIMENSION: &str = "Method";

/// The dimension naming the path half of it.
pub const RESOURCE_DIMENSION: &str = "Resource";

/// How many requests the series' subject answered.
pub const COUNT_METRIC: &str = "Count";

/// How many of them were answered with a client-error status.
pub const CLIENT_ERROR_METRIC: &str = "4xx";

/// How many of them were answered with a server-error status.
pub const SERVER_ERROR_METRIC: &str = "5xx";

/// How long answering one request took, end to end.
pub const LATENCY_METRIC: &str = "Latency";

/// How many payload bytes answering it carried, in and out.
pub const DATA_PROCESSED_METRIC: &str = "DataProcessed";

/// The unit a counter of requests is published in
/// (`smithy_query(query="shape",shape="StandardUnit")`).
const COUNT_UNIT: &str = "Count";

/// The unit a latency is published in, which is the resolution the access-log record of the
/// same request reports it at.
const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The unit a payload size is published in.
const BYTES_UNIT: &str = "Bytes";

/// The lowest status of the client-error class, and the lowest of the server-error class
/// (RFC 9110 §15.5, §15.6) — the two classes the error counters count. A class holds
/// [`STATUSES_PER_CLASS`] statuses.
const FIRST_CLIENT_ERROR_STATUS: u16 = 400;
const FIRST_SERVER_ERROR_STATUS: u16 = 500;
const STATUSES_PER_CLASS: u16 = 100;

/// What ONE request contributes to a counter it belongs to, and what it contributes to one it
/// does not: a counter counts requests, so each one is a single observation, and a request
/// outside a counter's class is observed as none of them rather than left unpublished — which
/// is what lets the `Average` of an error counter be the share of the traffic in that class.
const ONE_REQUEST: f64 = 1.0;
const NO_REQUEST: f64 = 0.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Seconds hold a thousand milliseconds: a latency measured as a duration is reported in the
/// unit [`MILLISECONDS_UNIT`] names, with the sub-millisecond part kept.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// Count the request this stage has just answered into every series it belongs to.
///
/// Called from the seams that serve this data plane with the ANSWER in hand: which counter a
/// request lands in and how long answering it took are facts of the answer, so they cannot be
/// known before it.
///
/// Nothing here can fail the request: the stage has already answered, so an observation that
/// could not be stored is reported where a developer can see it and nothing else.
pub(super) fn published<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    response: &RtResponse,
    latency: Duration,
) {
    let Some(planned) = planned(engine, req, response, latency) else {
        return;
    };
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let timestamp_secs = cloudwatch::state::now_secs();
    let mut store = state.lock();
    for dimensions in &planned.series {
        for observation in &planned.observations {
            let key = cloudwatch::state::MetricKey {
                namespace: NAMESPACE.to_string(),
                metric_name: observation.metric_name.to_string(),
                dimensions: dimensions.clone(),
            };
            let datum = cloudwatch::state::StoredDatum {
                timestamp_secs,
                sample_count: 1.0,
                sum: observation.value,
                minimum: observation.value,
                maximum: observation.value,
                unit: Some(observation.unit.to_string()),
                storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
            };
            if store.append_datapoints(key, vec![datum]).is_err() {
                ::tracing::warn!(
                    api_id = %planned.api_id,
                    metric = %observation.metric_name,
                    "the CloudWatch metric store is at capacity, so this request was not \
                     counted"
                );
            }
        }
    }
}

/// One observation of one metric: its value and the unit that value is in.
#[derive(Debug, Clone, PartialEq)]
struct Observation {
    metric_name: &'static str,
    value: f64,
    unit: &'static str,
}

/// Everything to publish for one answered request: which series it belongs to and what each
/// of them observes.
#[derive(Debug)]
struct Planned {
    /// The api the request was answered for, for the log line above.
    api_id: String,
    /// The dimension sets this request is counted under.
    series: Vec<cloudwatch::state::DimensionSet>,
    /// The observations each of those series takes of it.
    observations: Vec<Observation>,
}

/// The series this request belongs to, read under ONE shared read of this service's store,
/// which is RELEASED before the datapoints are stored: the metric store belongs to another
/// service, and no lock of this one is held while that one is written.
///
/// `None` when the request reached no api of this service at all — an endpoint that resolves
/// to nothing is traffic of no api, so there is no series to count it in.
fn planned<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    response: &RtResponse,
    latency: Duration,
) -> Option<Planned> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    let resolved = super::resolve(&store, &super::Addressed::of_request(req))?;
    let api = resolved.api;
    let mut series = vec![api_series(&api.api_id)];
    if let Some((stage, route_path)) = super::stage_of(api, resolved.invoke_path) {
        let mut stage_series = api_series(&api.api_id);
        stage_series.insert(STAGE_DIMENSION.to_string(), stage.stage_name.clone());
        series.push(stage_series.clone());
        // WHICH route answered the request, where one did: the route's own series exist only
        // for a route the serving stage asks for them for.
        let answering = stage
            .deployment_id
            .as_deref()
            .and_then(|deployment_id| api.deployments.get(deployment_id))
            .and_then(|deployment| {
                super::matching_route(&deployment.configuration.routes, req.method(), &route_path)
            });
        if let Some(route) = answering.filter(|route| detailed(stage, &route.route_key)) {
            let (method, resource) = route_dimensions(&route.route_key);
            let mut route_series = stage_series;
            route_series.insert(METHOD_DIMENSION.to_string(), method);
            route_series.insert(RESOURCE_DIMENSION.to_string(), resource);
            series.push(route_series);
        }
    }
    Some(Planned {
        api_id: api.api_id.clone(),
        series,
        observations: observations(
            response.status().as_u16(),
            latency,
            request_payload_bytes(req).saturating_add(response_payload_bytes(response)),
        ),
    })
}

/// The dimension set that names the api alone, which every other set of these series extends.
fn api_series(api_id: &str) -> cloudwatch::state::DimensionSet {
    [(API_ID_DIMENSION.to_string(), api_id.to_string())]
        .into_iter()
        .collect()
}

/// Whether `stage` asks for the per-route series of the route `route_key` names.
///
/// `RouteSettings$DetailedMetricsEnabled` is read off the settings the stage states for THAT
/// route key, and otherwise off its `DefaultRouteSettings` — "the default route settings for
/// the stage", which is what governs a route the stage states nothing of its own for. The
/// fallback is per MEMBER rather than per settings block: a route whose own settings state
/// only a throttle states nothing about its metrics, so the stage's default is what decides
/// them. A stage that states neither asks for no route's series, which is the setting a
/// created stage carries.
fn detailed(stage: &crate::state::Stage, route_key: &str) -> bool {
    stage
        .route_settings
        .get(route_key)
        .and_then(|settings| settings.detailed_metrics_enabled)
        .or_else(|| {
            stage
                .default_route_settings
                .as_ref()
                .and_then(|settings| settings.detailed_metrics_enabled)
        })
        .unwrap_or(false)
}

/// The `Method` and `Resource` dimensions of ONE route's own series, read off its `RouteKey`.
///
/// A route key is the verb and the path it answers for, written as one value — `GET /orders`,
/// or `ANY /orders` for a key that answers every verb — so the two dimensions are the two
/// halves of it. The `$default` route names neither half: it is "the default route" that
/// answers whatever no other route of the api matches, every verb included, so the key itself
/// is the resource it stands for and the verb it answers is the one API Gateway's own key
/// grammar spells as every verb ([`super::ANY_METHOD`]).
fn route_dimensions(route_key: &str) -> (String, String) {
    match route_key.split_once(' ') {
        Some((method, path)) => (method.trim().to_string(), path.trim().to_string()),
        None => (super::ANY_METHOD.to_string(), route_key.to_string()),
    }
}

/// What the series take of a request answered with `status`, in `latency`, carrying `bytes` of
/// payload in and out.
fn observations(status: u16, latency: Duration, bytes: usize) -> Vec<Observation> {
    let in_class = |first: u16| {
        if status >= first && status < first.saturating_add(STATUSES_PER_CLASS) {
            ONE_REQUEST
        } else {
            NO_REQUEST
        }
    };
    vec![
        Observation {
            metric_name: COUNT_METRIC,
            value: ONE_REQUEST,
            unit: COUNT_UNIT,
        },
        Observation {
            metric_name: CLIENT_ERROR_METRIC,
            value: in_class(FIRST_CLIENT_ERROR_STATUS),
            unit: COUNT_UNIT,
        },
        Observation {
            metric_name: SERVER_ERROR_METRIC,
            value: in_class(FIRST_SERVER_ERROR_STATUS),
            unit: COUNT_UNIT,
        },
        Observation {
            metric_name: LATENCY_METRIC,
            value: latency.as_secs_f64() * MILLIS_PER_SECOND,
            unit: MILLISECONDS_UNIT,
        },
        Observation {
            metric_name: DATA_PROCESSED_METRIC,
            value: bytes as f64,
            unit: BYTES_UNIT,
        },
    ]
}

/// How many payload bytes the REQUEST carried, as the gateway measured them — the ONE reading
/// of a request's size this data plane has ([`super::payload_bytes`]), which the payload quota
/// refuses a request by. A request whose size nothing read carries no measurable payload, so
/// it contributes none.
fn request_payload_bytes(req: &RtRequest) -> usize {
    super::payload_bytes(req).unwrap_or_default()
}

/// How many payload bytes the ANSWER carried, which this endpoint renders in memory.
fn response_payload_bytes(response: &RtResponse) -> usize {
    response.body().bytes().map(<[u8]>::len).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    //! Which counters one answered request lands in, and which route a key names. The
    //! published series themselves are pinned end to end over the wire in
    //! `tests/all/http_api_request_metrics.rs`.

    use super::*;

    /// The value `metric` was observed at, out of what one answered request contributes.
    fn observed(status: u16, metric: &str) -> f64 {
        observations(status, Duration::from_millis(7), 11)
            .into_iter()
            .find(|observation| observation.metric_name == metric)
            .map(|observation| observation.value)
            .unwrap_or_else(|| panic!("no {metric} observation at all"))
    }

    #[test]
    fn every_answered_request_is_counted_and_its_class_decides_the_error_counters() {
        // Rows span the status classes an invoke is answered with: the successes a backend
        // produced, the redirect an upstream the caller reaches is sent as, each refusal this
        // data plane words itself, and the status a backend that produced nothing earns.
        let cases: [(u16, f64, f64); 9] = [
            (200, NO_REQUEST, NO_REQUEST),
            (201, NO_REQUEST, NO_REQUEST),
            (307, NO_REQUEST, NO_REQUEST),
            (400, ONE_REQUEST, NO_REQUEST),
            (403, ONE_REQUEST, NO_REQUEST),
            (404, ONE_REQUEST, NO_REQUEST),
            (429, ONE_REQUEST, NO_REQUEST),
            (500, NO_REQUEST, ONE_REQUEST),
            (503, NO_REQUEST, ONE_REQUEST),
        ];
        for (status, client, server) in cases {
            let counted = observed(status, COUNT_METRIC);
            assert_eq!(
                counted, ONE_REQUEST,
                "case {status}: expected one request counted, got {counted}"
            );
            let actual = observed(status, CLIENT_ERROR_METRIC);
            assert_eq!(
                actual, client,
                "case {status}: expected the {CLIENT_ERROR_METRIC} counter at {client}, got \
                 {actual}"
            );
            let actual = observed(status, SERVER_ERROR_METRIC);
            assert_eq!(
                actual, server,
                "case {status}: expected the {SERVER_ERROR_METRIC} counter at {server}, got \
                 {actual}"
            );
        }
    }

    #[test]
    fn a_routes_own_series_name_the_two_halves_of_its_route_key() {
        // Rows span the route-key forms an HTTP api admits: a verb and a path, the
        // verb-independent catch-all, a path carrying a variable and the greedy remainder, and
        // the default route, which names no halves at all.
        let cases: [(&str, &str, &str); 5] = [
            ("GET /orders", "GET", "/orders"),
            ("POST /orders/{id}", "POST", "/orders/{id}"),
            ("ANY /{proxy+}", "ANY", "/{proxy+}"),
            ("DELETE /", "DELETE", "/"),
            ("$default", super::super::ANY_METHOD, "$default"),
        ];
        for (route_key, method, resource) in cases {
            let actual = route_dimensions(route_key);
            assert_eq!(
                actual,
                (method.to_string(), resource.to_string()),
                "case {route_key:?}: expected the dimensions ({method:?}, {resource:?}), got \
                 {actual:?}"
            );
        }
    }
}
