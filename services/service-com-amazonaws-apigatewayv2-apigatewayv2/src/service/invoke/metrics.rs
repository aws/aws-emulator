// SPDX-License-Identifier: CC0-1.0

//! WHERE this data plane's own datapoints land: the CloudWatch metric store, and the alarm
//! wake-up each datapoint written there owes.
//!
//! [`super::request_metrics`] decides WHAT one answered request observes and under which
//! dimensions; this module is the one write that stores those observations. It is a concern of
//! its own because storing a datapoint owes more than the storing: an alarm is evaluated FROM
//! the datapoints of the series it watches, so the write that stores one is what makes that
//! alarm's evaluation due — the same thing an ingested `PutMetricData` datapoint owes the
//! alarms over the series it lands in, which is why a service's own series cannot skip it. An
//! alarm over a stage's traffic would otherwise be graded only by the store's own scheduled
//! sweep: the transition the traffic decided would arrive later than the traffic that decided
//! it, and a client watching an api's error series could not tell an alarm that is late from a
//! pipeline that never ran at all.
//!
//! The store is the CloudWatch emulator's OWN metric store, reached through the shared engine,
//! so what is written here is the very data `ListMetrics`, `GetMetricStatistics` and the alarm
//! evaluation read, and a publisher cannot disagree with its readers.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// One datapoint to store: the series it belongs to, and the observation itself.
pub(super) type Datapoint = (cloudwatch::state::MetricKey, cloudwatch::state::StoredDatum);

/// Store `data`, and make every alarm watching a series it touched due for evaluation.
///
/// `api_id` names the api the datapoints are of, for the one thing that can go wrong here — a
/// store at capacity, which is reported where a developer can see it and nowhere else. Nothing
/// about publishing a metric may fail the request the stage has already answered, and an engine
/// carrying no CloudWatch at all has no store to publish into.
pub(super) fn stored<Eng: Send + Sync + 'static>(engine: &Eng, api_id: &str, data: Vec<Datapoint>) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let at_secs = cloudwatch::state::now_secs();
    let mut written: Vec<cloudwatch::state::WrittenSeries> = Vec::with_capacity(data.len());
    let mut store = state.lock();
    for (key, datum) in data {
        let units = vec![datum.unit().to_string()];
        if store.append_datapoints(key.clone(), vec![datum]).is_err() {
            ::tracing::warn!(
                %api_id,
                metric = %key.metric_name,
                "the CloudWatch metric store is at capacity, so this request was not counted"
            );
            continue;
        }
        written.push(cloudwatch::state::WrittenSeries { key, units });
    }
    store.make_alarms_due(&written, at_secs);
}
