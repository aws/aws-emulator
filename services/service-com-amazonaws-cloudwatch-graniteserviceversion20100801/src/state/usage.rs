// SPDX-License-Identifier: CC0-1.0

//! The usage telemetry CloudWatch publishes about its OWN API: one `CallCount` observation
//! per served call, in the reserved `AWS/Usage` namespace.
//!
//! This is the service metering itself, not a caller publishing a metric: `PutMetricData`
//! refuses the reserved namespace (`crate::ops::put_metric_data`), so the only thing that can
//! put a series there is the service, and what it puts there is one datapoint of value 1 for
//! each call it served — the series every API-usage report and request-rate alarm is read
//! from. A call the service REFUSED was still served, so it is counted too: the count is of
//! requests the API answered, whatever it answered.
//!
//! The series' identity is the usage dimension set — `Service`, `Class`, `Type` and
//! `Resource` — carrying this service and the operation the request named, so a reader
//! selects one operation's calls by naming its operation in `Resource`, and an operation
//! nothing called in a window has no datapoint in it rather than a zero.
//!
//! A datapoint stored here is a metric update of this account like any other, so a running
//! metric stream whose filters select it exports it ([`super::metric_stream_export`]): a stream
//! streams "metrics from all metric namespaces in the account", so the namespace being one no
//! caller may publish into does not keep its metrics out of a stream.

use super::{
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, MetricKey,
};

/// The namespace an Amazon Web Services service publishes its own usage metrics in, which no
/// caller may publish into ([`crate::ops::put_metric_data`] refuses the reserved prefix).
pub const USAGE_NAMESPACE: &str = "AWS/Usage";

/// The usage metric that counts API requests: one observation of 1 per served call, so a
/// `Sum` over a window is how many calls that window held.
pub const CALL_COUNT: &str = "CallCount";

/// The `Service` dimension of every usage series this service publishes: its own name, as the
/// model gives it (`sdkId`/`cloudFormationName` `CloudWatch` on the service shape).
pub const USAGE_SERVICE: &str = "CloudWatch";

/// The `Class` dimension of an API call count: a call belongs to no usage class of its own,
/// stated as the value the metric system has for "none" rather than left off the series —
/// the dimension set is part of a metric's identity, so a reader has to be able to name it.
pub const USAGE_CLASS: &str = "None";

/// The `Type` dimension of an API call count: the usage counted is API requests.
pub const USAGE_TYPE: &str = "API";

/// The dimension naming which service's usage the series is about.
const SERVICE_DIMENSION: &str = "Service";

/// The dimension naming the usage class within that service.
const CLASS_DIMENSION: &str = "Class";

/// The dimension naming what kind of usage is counted.
const TYPE_DIMENSION: &str = "Type";

/// The dimension naming the thing used — for an API call count, the operation called.
const RESOURCE_DIMENSION: &str = "Resource";

/// The series one operation's call count lives in: the usage dimension set, with `operation`
/// as the resource used.
///
/// Derived from the operation name alone, so the call this service counted and the series a
/// reader asks for by that operation name are the same series.
pub fn call_count_key(operation: &str) -> MetricKey {
    MetricKey {
        namespace: USAGE_NAMESPACE.to_string(),
        metric_name: CALL_COUNT.to_string(),
        dimensions: [
            (SERVICE_DIMENSION, USAGE_SERVICE),
            (CLASS_DIMENSION, USAGE_CLASS),
            (TYPE_DIMENSION, USAGE_TYPE),
            (RESOURCE_DIMENSION, operation),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect(),
    }
}

impl State {
    /// Count one served call of `operation` into this service's usage telemetry, stamped
    /// `now_secs`.
    ///
    /// One observation of 1, published with no unit of its own — a call count is a bare
    /// number, which is what the unit value for "no unit" ([`super::UNIT_NONE`]) states — so
    /// a `Sum` over a window is the number of calls in it and an `Average` is 1.
    ///
    /// Stored through the seam every count this service publishes about itself goes through
    /// ([`State::append_self_metered_count`]), which is also what makes the evaluation of an
    /// alarm watching this series due and what drops the count rather than failing the call it
    /// is about when the store is full. The series itself grows no further than any other:
    /// there is one per operation this service models, and each is held to the store's own
    /// per-series datapoint cap, oldest first ([`State::append_datapoints`]).
    pub fn count_api_call(&mut self, operation: &str, now_secs: f64) {
        self.append_self_metered_count(call_count_key(operation), 1.0, now_secs);
    }
}

/// Count one served call of `operation` into the usage telemetry held in `engine`'s store for
/// this service, and export the metric update that count IS.
///
/// The seam the request path meters through ([`crate::service`]): it takes the store's lock
/// for the append alone and answers nothing, because the count is published off the response —
/// a call is answered whether or not its own usage datapoint could be stored.
///
/// A stream "can automatically stream CloudWatch metrics to Amazon Web Services destinations"
/// and the stream a caller creates streams "metrics from all metric namespaces in the account"
/// — every namespace, or every one its filters leave in (`PutMetricStream`) — so this series is
/// streamed like any other metric of the account: a datapoint stored HERE is a metric update,
/// and a running stream selecting it is owed its record
/// ([`super::metric_stream_export::export_published`], the one export path a caller's publish
/// takes too). The export runs with the store released and reads what the store already holds,
/// so metering a call writes nothing a later caller would not have seen — and a store that holds
/// no stream at all is not asked to plan one.
///
/// What a metered call can cost is bounded before it is spent: this seam publishes ONE series
/// over ONE period, so the planning scan is a single pass over that series' datapoints, itself
/// capped at [`super::MAX_DATAPOINTS_PER_SERIES`] and taken once for the request however many
/// streams select it; the records it can owe are capped at
/// [`super::metric_stream_export::MAX_RECORDS_PER_REQUEST`], which is what bounds the in-process
/// deliveries one metered call makes; and nothing is held while they are made.
pub async fn count_api_call<Eng: Send + Sync + 'static>(
    engine: &Eng,
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = super::now_secs();
    // The count, and whether anything could stream it, under ONE hold of the store: a store
    // holding no metric stream owes no record, so the call this meters costs the datapoint and
    // nothing else — no series is built and no export is planned for it.
    let streamed = {
        let mut store = state.lock();
        store.count_api_call(operation, now);
        !store.metric_streams.is_empty()
    };
    if !streamed {
        return;
    }
    // Which series and period this count publishes into, by the derivation an ingested batch's
    // is read with ([`super::metric_stream_export::published_at`]), so one observation owes
    // exactly the record a publish of the same datapoint would.
    let published =
        super::metric_stream_export::published_at(call_count_key(operation), now);
    // BOXED: this seam is awaited on the answer path of every operation of this service
    // ([`crate::service`]), and the export's own future is far larger than the count's. Holding
    // it inline would grow the future of every served call by that much — including the calls
    // one emulated service makes to another inside a handler, where those futures nest — so the
    // export lives on the heap and the metering future stays small.
    Box::pin(super::metric_stream_export::export_published(
        engine,
        ctx,
        &super::request_region(ctx.parts()),
        &published,
    ))
    .await;
}
