// SPDX-License-Identifier: CC0-1.0

//! The `AWS/ApiGateway` metrics a STAGE of this service's data planes publishes about the
//! traffic it served.
//!
//! A call a stage answered leaves no response a later caller can read: nothing in the control
//! plane reports that a client connected, that a route was dispatched or that an integration
//! failed. The only way a client learns any of it — a dashboard, a CloudWatch alarm, an
//! on-call rule — is the series API Gateway publishes about the stage itself.
//!
//! THIS module owns what both data planes share: the namespace, the dimension names, the
//! `DetailedMetricsEnabled` reading that decides whether a route has series of its own, and
//! the ONE write every datapoint of this service goes through ([`stored`]), so the namespace a
//! series lands in, the resolution it is stored at and the alarm wake-up it owes cannot differ
//! between them. What each plane counts is its own: the per-REQUEST series of an HTTP api's
//! stage are [`crate::service::invoke::request_metrics`], and the per-CALL series of a
//! WEBSOCKET api's stage — connections, frames, backend latency and the error classes — are
//! published from this module's [`StageMetrics`] by the socket that carries them
//! ([`crate::service::websocket::transport`]).
//!
//! WHICH dimension set each measurement lands under is part of the contract:
//!
//! * the STAGE-WIDE series, dimensioned on [`API_ID_DIMENSION`] and [`STAGE_DIMENSION`], is
//!   published for every measurement — it is the total across the stage's routes;
//! * the PER-ROUTE series, which adds [`ROUTE_DIMENSION`], is published beside it only for a
//!   route whose serving stage turns `DetailedMetricsEnabled` on
//!   ([`publishes_per_route`]): "Enables detailed metrics" is a per-route setting a stage
//!   carries in its `DefaultRouteSettings` or in the `RouteSettings` of one route
//!   (`RouteSettings$DetailedMetricsEnabled`), so a stage that turns it off publishes the
//!   stage-wide series alone. (An HTTP api's routes are named by the two halves of their
//!   `RouteKey` instead, which is why that plane states its own route dimensions.)
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own them
//! ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It goes
//! into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read, so one
//! metric and its reader cannot disagree.
//!
//! No count is kept here. Each observation is a datapoint of what ONE call did, so the total
//! a client reads is the `Sum` CloudWatch computes over a period and the number of calls is
//! its `SampleCount`.

use std::time::Duration;

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use crate::state::Stage;

/// The CloudWatch store an API Gateway stage's own metrics live in: the CloudWatch service's
/// OWN state struct, reached through the SAME shared engine this service's store lives in.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine, or `None` when the engine carries no
/// CloudWatch — a deployment with nothing to publish to.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The namespace API Gateway publishes a stage's own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/ApiGateway";

/// The dimension that names the api every one of these series is of.
pub const API_ID_DIMENSION: &str = "ApiId";

/// The dimension that names the stage every one of these series is of.
pub const STAGE_DIMENSION: &str = "Stage";

/// The dimension that names the route a per-route series is of.
pub const ROUTE_DIMENSION: &str = "Route";

/// The messages sent to the `$connect` route's INTEGRATION: one per connection request that
/// reached it.
pub const CONNECT_COUNT_METRIC: &str = "ConnectCount";

/// The messages the stage carried, in either direction: each frame a client sent it and each
/// frame it sent a client.
pub const MESSAGE_COUNT_METRIC: &str = "MessageCount";

/// How long one integration took to answer, in milliseconds — one sample per integration
/// performed, so the number of backend calls is the series' `SampleCount`.
pub const INTEGRATION_LATENCY_METRIC: &str = "IntegrationLatency";

/// The calls the stage answered the client with a client-error status: a handshake refused
/// `4xx`, or a frame the api routed nowhere.
pub const CLIENT_ERROR_METRIC: &str = "ClientError";

/// The calls whose route could not be executed to a successful end: an authorizer or
/// integration that failed, and a connection request an integration refused.
pub const EXECUTION_ERROR_METRIC: &str = "ExecutionError";

/// The calls whose integration produced no reply the gateway could use.
pub const INTEGRATION_ERROR_METRIC: &str = "IntegrationError";

/// The `StandardUnit` a bare count of things carries: `Count`.
pub const COUNT_UNIT: &str = "Count";

/// The `StandardUnit` a measured duration carries: `Milliseconds`, the unit API Gateway's own
/// latency series are expressed in.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Whether `stage` publishes the PER-ROUTE series of `route_key` beside its stage-wide one.
///
/// `RouteSettings$DetailedMetricsEnabled` "specifies whether detailed metrics are enabled",
/// and a stage carries route settings twice over: the `RouteSettings` of one route, which
/// govern that route, and the `DefaultRouteSettings` every route it does not name is served
/// under. So the route's own setting decides when it has one, and the stage's default decides
/// otherwise.
pub fn publishes_per_route(stage: &Stage, route_key: &str) -> bool {
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

/// What ONE call of a stage ended in, as the three error counters report it: a call that
/// failed none of these ways carries `false` for each, which is published as an explicit zero
/// ([`StageMetrics::served`]).
///
/// * `client_error` — the call was answered with a client-error status: a handshake refused
///   `4xx`, or a frame the api routes nowhere.
/// * `execution_error` — the route's execution did not reach a successful end: an authorizer
///   or an integration that failed, and a connection request an integration refused.
/// * `integration_error` — the integration produced no reply the gateway could use.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CallOutcome {
    pub client_error: bool,
    pub execution_error: bool,
    pub integration_error: bool,
}

/// The publisher of one STAGE's own metrics: the store the datapoints go to, and the api and
/// stage every series it writes is dimensioned on.
///
/// It holds no counts of its own — every observation is a datapoint in the CloudWatch store,
/// never a second copy kept here.
#[derive(Clone)]
pub struct StageMetrics {
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
    /// The `ApiId` every series this publisher writes is dimensioned on.
    api_id: String,
    /// The `StageName` every series this publisher writes is dimensioned on.
    stage_name: String,
}

impl ::std::fmt::Debug for StageMetrics {
    /// Which stage this publisher is of and whether it has a store to append to, which is all
    /// of it that is not that store (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("StageMetrics")
            .field("api_id", &self.api_id)
            .field("stage_name", &self.stage_name)
            .field("published", &self.store.is_some())
            .finish()
    }
}

impl StageMetrics {
    /// The publisher appending the series of `stage_name` of `api_id` to `store`.
    pub fn new(
        store: Option<::engine::ServiceState<CloudWatchState>>,
        api_id: &str,
        stage_name: &str,
    ) -> Self {
        Self {
            store,
            api_id: api_id.to_string(),
            stage_name: stage_name.to_string(),
        }
    }

    /// Publish that a message was sent to the `$connect` route's INTEGRATION: `ConnectCount`
    /// is "the number of messages sent to the `$connect` route integration", so it counts the
    /// connection requests that reached that integration — a handshake refused before it,
    /// and an api that fronts no `$connect` integration at all, send it nothing.
    ///
    /// The connection request is a message of the stage either way, which is published for it
    /// by [`Self::message`] as for every other call.
    ///
    /// `per_route` is the route key when the serving stage publishes its per-route series
    /// too, and `None` otherwise ([`publishes_per_route`]) — the same for every method here.
    pub fn connect_dispatched(&self, per_route: Option<&str>) {
        self.count(CONNECT_COUNT_METRIC, per_route);
    }

    /// Publish one message the stage carried, in either direction.
    pub fn message(&self, per_route: Option<&str>) {
        self.count(MESSAGE_COUNT_METRIC, per_route);
    }

    /// Publish how long ONE integration of the stage took to answer.
    pub fn integration_latency(&self, per_route: Option<&str>, elapsed: Duration) {
        self.publish(
            INTEGRATION_LATENCY_METRIC,
            per_route,
            elapsed.as_secs_f64() * 1_000.0,
            MILLISECONDS_UNIT,
        );
    }

    /// Publish what ONE call of the stage ended in: an observation of EACH error counter,
    /// carrying 1 for the class that call earned and 0 for the classes it did not.
    ///
    /// A zero is as much an observation as a one, and the reason the counters are published
    /// per call rather than only when something fails: a client reading `ClientError` over a
    /// window of healthy traffic reads an explicit zero, which is what tells "nothing failed"
    /// from "this stage is publishing nothing at all" — a gap no dashboard and no alarm can
    /// act on. It is also what makes the error counters' `SampleCount` the number of calls the
    /// stage served.
    pub fn served(&self, per_route: Option<&str>, outcome: CallOutcome) {
        for (metric, occurred) in [
            (CLIENT_ERROR_METRIC, outcome.client_error),
            (EXECUTION_ERROR_METRIC, outcome.execution_error),
            (INTEGRATION_ERROR_METRIC, outcome.integration_error),
        ] {
            self.publish(metric, per_route, f64::from(u8::from(occurred)), COUNT_UNIT);
        }
    }

    /// Publish ONE occurrence of a counted event.
    fn count(&self, metric: &str, per_route: Option<&str>) {
        self.publish(metric, per_route, 1.0, COUNT_UNIT);
    }

    /// Append one observation to the stage-wide series, and to the per-route series beside it
    /// when the serving stage publishes one — as ONE write, so both land at the same instant.
    fn publish(&self, metric: &str, per_route: Option<&str>, value: f64, unit: &str) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut data = vec![datapoint(
            metric,
            stage_dimensions(&self.api_id, &self.stage_name),
            value,
            unit,
        )];
        if let Some(route_key) = per_route {
            data.push(datapoint(
                metric,
                route_dimensions(&self.api_id, &self.stage_name, route_key),
                value,
                unit,
            ));
        }
        store_datapoints(store, &self.api_id, data);
    }
}

/// One datapoint to store: the series it belongs to, and the observation itself.
pub type Datapoint = (cloudwatch::MetricKey, cloudwatch::StoredDatum);

/// ONE observation of an `AWS/ApiGateway` series, ready to store: `value` in `unit`, of the
/// metric `metric` under `dimensions`, stamped now and at the resolution every datapoint of
/// this service is published at.
///
/// The ONE place the namespace, the time stamp and the storage resolution of this service's own
/// datapoints are decided, so the two data planes cannot publish into different series or at
/// different granularities.
pub fn datapoint(
    metric: &str,
    dimensions: cloudwatch::DimensionSet,
    value: f64,
    unit: &str,
) -> Datapoint {
    let key = cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions,
    };
    let datum = cloudwatch::StoredDatum {
        timestamp_secs: cloudwatch::now_secs(),
        sample_count: 1.0,
        sum: value,
        minimum: value,
        maximum: value,
        unit: Some(unit.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    };
    (key, datum)
}

/// Store `data` in the CloudWatch metric store of the SAME shared engine, and make every alarm
/// watching a series it touched due for evaluation.
///
/// An engine carrying no CloudWatch at all has no store to publish into, and nothing about
/// publishing a metric may fail the call the stage has already answered.
pub fn stored<Eng: Send + Sync + 'static>(engine: &Eng, api_id: &str, data: Vec<Datapoint>) {
    let Some(state) = cloudwatch_state(engine) else {
        return;
    };
    store_datapoints(&state, api_id, data);
}

/// Store `data`, and make every alarm watching a series it touched due for evaluation.
///
/// The ONE write this service's own datapoints go through, shared by both data planes (an HTTP
/// api's per-request series go through it too,
/// [`crate::service::invoke::request_metrics`]), because storing a datapoint owes more than
/// the storing: an alarm is evaluated FROM the datapoints of the series it watches, so the
/// write that stores one is what makes that alarm's evaluation due — the same thing an
/// ingested `PutMetricData` datapoint owes the alarms over the series it lands in, which is why
/// a service's own series cannot skip it. An alarm over a stage's traffic would otherwise be
/// graded only by the store's own scheduled sweep: the transition the traffic decided would
/// arrive later than the traffic that decided it, and a client watching an api's error series
/// could not tell an alarm that is late from a pipeline that never ran at all.
///
/// `api_id` names the api the datapoints are of, for the one thing that can go wrong here — a
/// store at capacity, which is reported where a developer can see it and nowhere else.
pub fn store_datapoints(
    store: &::engine::ServiceState<CloudWatchState>,
    api_id: &str,
    data: Vec<Datapoint>,
) {
    let at_secs = cloudwatch::now_secs();
    let mut written: Vec<cloudwatch::WrittenSeries> = Vec::with_capacity(data.len());
    let mut store = store.lock();
    for (key, datum) in data {
        let units = vec![datum.unit().to_string()];
        if store.append_datapoints(key.clone(), vec![datum]).is_err() {
            ::tracing::warn!(
                %api_id,
                metric = %key.metric_name,
                "the CloudWatch metric store is at capacity, so this call was not counted"
            );
            continue;
        }
        written.push(cloudwatch::WrittenSeries { key, units });
    }
    store.make_alarms_due(&written, at_secs);
}

/// The dimension set of a series about a STAGE as a whole: the api and the stage, which
/// together identify it.
pub fn stage_dimensions(api_id: &str, stage_name: &str) -> cloudwatch::DimensionSet {
    [
        (API_ID_DIMENSION.to_string(), api_id.to_string()),
        (STAGE_DIMENSION.to_string(), stage_name.to_string()),
    ]
    .into_iter()
    .collect()
}

/// The dimension set of a series about ONE ROUTE of a stage: the stage's own dimensions and
/// the route key beside them.
pub fn route_dimensions(
    api_id: &str,
    stage_name: &str,
    route_key: &str,
) -> cloudwatch::DimensionSet {
    let mut dimensions = stage_dimensions(api_id, stage_name);
    dimensions.insert(ROUTE_DIMENSION.to_string(), route_key.to_string());
    dimensions
}
