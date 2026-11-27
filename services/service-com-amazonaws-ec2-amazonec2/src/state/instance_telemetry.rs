// SPDX-License-Identifier: CC0-1.0

//! The `AWS/EC2` series a RUNNING instance publishes about ITSELF: the instance metric
//! families, the cadence the instance's monitoring state selects them at, and where they are
//! stored.
//!
//! # What the series are
//!
//! Amazon EC2 publishes each of these measurements of one instance as its own `AWS/EC2`
//! metric, dimensioned on exactly that instance ([`INSTANCE_ID_DIMENSION`]) and measured in
//! that metric's own unit ([`INSTANCE_METRICS`] is the catalog, with the unit of each entry).
//! Those series are what a CPU dashboard plots and what every instance alarm — including the
//! ones a scaling policy or an EC2 instance action is built from — is evaluated against, so an
//! instance whose metrics are never published has no utilisation to graph and no breach to
//! fire on.
//!
//! # The aggregate series beside them
//!
//! A measurement of one instance is also a contribution to the series of the instances it is
//! one OF, which is what the aggregate dimensions select: "`InstanceType`: This dimension
//! filters the data you request for all instances running with this specified instance type"
//! and "`ImageId`: This dimension filters the data you request for all instances running this
//! Amazon EC2 Amazon Machine Image (AMI)", both "available for instances with detailed
//! monitoring enabled" (Amazon EC2 User Guide, instance metric dimensions). So a guest
//! measurement of an instance with DETAILED monitoring is published a second and a third time,
//! into the series dimensioned on that instance's type ([`INSTANCE_TYPE_DIMENSION`]) and on
//! the image it was launched from ([`IMAGE_ID_DIMENSION`]) — one contribution per instance per
//! period, so the statistics a reader takes of an aggregate period are the statistics of the
//! instances that were measured in it: its `SampleCount` is how many of them there were, its
//! `Maximum` the largest reading any of them took. An instance with BASIC monitoring
//! contributes to neither, which is what makes those dimensions a thing detailed monitoring
//! buys, and a status check's verdict contributes to neither either: it is a verdict about one
//! named instance rather than a quantity measured on a guest.
//!
//! # When a sample happens
//!
//! At the cadence the instance's MONITORING STATE selects, which is the distinction between
//! basic and detailed monitoring: "Basic monitoring | Status check metrics are available in
//! 1-minute periods. All other metrics are available in 5-minute periods" against "Detailed
//! monitoring | You can get metrics in 1-minute periods" (Amazon EC2 User Guide, instance
//! monitoring). So
//!
//! * every metric of an instance with detailed monitoring is published once per
//!   [`DETAILED_MONITORING_PERIOD`];
//! * the status-check metrics of an instance with basic monitoring are published once per
//!   [`STATUS_CHECK_PERIOD`] all the same, because those are available in 1-minute periods
//!   under basic monitoring too;
//! * every other metric of an instance with basic monitoring is published once per
//!   [`BASIC_MONITORING_PERIOD`].
//!
//! A sample is TAKEN at the boundaries of that period — the instants of the WALL CLOCK that are
//! multiples of the period's own length, the grid CloudWatch itself aggregates a period on
//! ("CloudWatch aggregates data points based on the length of the period that you specify",
//! `GetMetricStatistics`) — and each reading measures the period that ENDED at the boundary it
//! was taken at ([`due_sample_instant`]). A datapoint carries the instant its reading was taken,
//! which is what makes the cadence observable: the boundaries of a 5-minute period are 5-minute
//! boundaries, so a datapoint stamped off that grid can only have been taken at the boundary of a
//! 1-minute period. One boundary yields one reading however often the sampler looks
//! ([`ComAmazonawsEc2Amazonec2State::instance_metric_periods`] remembers which instant each
//! series was last read at), so a period holds one reading of one instance rather than as many as
//! something happened to look.
//!
//! A reading is of the period BEHIND the instant it was taken at, so a reader sees a series that
//! stops at the last boundary the clock has passed and a 5-minute series lags further behind the
//! clock than a 1-minute one.
//!
//! Only a RUNNING instance publishes: these measurements are taken on a guest that is up, so
//! an instance that is pending, stopped or terminated has nothing measuring it and no datapoint
//! at the instants it was not running at — which is what tells an idle instance from one that
//! is not there. That is a fact of each DATAPOINT and not only of the series as a whole: a
//! reading is stamped with the instant it was taken, so an instance's series carries nothing
//! stamped before its guest came up and nothing stamped after it went away, and a client reading
//! the series of an instance FROM ITS LAUNCH sees the instance's first reading one period after
//! that launch rather than two. Publication resumes with the instance: one started again is
//! running again, so the next boundary it is running at carries its reading.
//!
//! Two things drive those samples:
//!
//! * [`publish_instance_metrics`], called from the one seam every request of this service
//!   passes through ([`crate::service`]), so what the request left the account's instances in
//!   is sampled as part of serving it;
//! * [`drive_instance_metric_clock`], the one bounded in-process task per store, which keeps
//!   sampling while the store holds a running instance. A client watching an instance's
//!   utilisation makes no further EC2 call before it looks for the next datapoint, so without
//!   the task a series would stop at the last request's own reading. It does not change WHEN a
//!   sample is due — [`due_sample_instant`] alone decides that — only how soon after a period's
//!   boundary the reading of that period is taken.
//!
//! # The agent's series travel the same way
//!
//! A guest may also hold the CLOUDWATCH AGENT ([`super::guest_cloudwatch_agent`]), which
//! publishes the series its own configuration declares — measurements of the machine no API of
//! this account can derive, and the StatsD values its listener received. Those are datapoints
//! of the same account's metric store, due at the agent's own interval rather than at the
//! instance's monitoring cadence, so they are collected and appended by the two seams above
//! ([`due_agent_samples`]) instead of by a second clock over the same store.
//!
//! # What one sample says
//!
//! Each metric is a measurement taken on the instance's GUEST, and this emulator runs no guest
//! workload at all — so in every period the quantity each of them measures is zero, which is
//! what each metric's own definition says for exactly that period: `CPUUtilization` is "the
//! percentage of allocated EC2 compute units that are currently in use on the instance" and
//! nothing is using any of them, `NetworkIn`/`NetworkOut` are "the number of bytes received
//! on"/"sent out on all network interfaces by the instance" in the period and no byte crossed
//! one, the disk metrics are the operations "completed" and the bytes "read from"/"written to"
//! the instance's volumes in the period and none completed, and `MetadataNoToken` is the number
//! of accesses to the metadata service "without a token", of which the guest made none. Any
//! other number would be a measurement of work nothing performed, which a client could not tell
//! from a measured one.
//!
//! The status-check metrics are NOT that: they report the verdict of a check this service
//! really performs on the instance ([`super::instance_status_checks`]), as the 0 or 1 the
//! metric is defined to carry ("0 (passed) or 1 (failed)"), and an instance whose checks have
//! not reached a verdict yet — a guest still initializing — has no verdict to publish and so no
//! datapoint in that period.
//!
//! Nothing here fails a request: the datapoints are published off the answer, so a CloudWatch
//! store this engine does not hold, or one at capacity, drops the sample and the request is
//! answered as it was.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::{
    instance_status_checks, service_state, ComAmazonawsEc2Amazonec2State, InstanceRecord,
    InstanceStateName, MONITORING_STATE_DISABLING, MONITORING_STATE_ENABLED,
    SUMMARY_STATUS_IMPAIRED, SUMMARY_STATUS_OK,
};

/// The namespace Amazon EC2 publishes an instance's own metrics under.
pub const NAMESPACE: &str = "AWS/EC2";

/// The dimension an instance's OWN series is dimensioned on: "`InstanceId`: This dimension
/// filters the data you request for only the identified instance" (Amazon EC2 User Guide,
/// instance metric dimensions).
pub const INSTANCE_ID_DIMENSION: &str = "InstanceId";

/// The dimension the series of every instance of ONE instance type is rolled up under:
/// "`InstanceType`: This dimension filters the data you request for all instances running
/// with this specified instance type ... available for instances with detailed monitoring
/// enabled" (Amazon EC2 User Guide, instance metric dimensions).
pub const INSTANCE_TYPE_DIMENSION: &str = "InstanceType";

/// The dimension the series of every instance launched from ONE image is rolled up under:
/// "`ImageId`: This dimension filters the data you request for all instances running this
/// Amazon EC2 Amazon Machine Image (AMI) ... available for instances with detailed monitoring
/// enabled" (Amazon EC2 User Guide, instance metric dimensions).
pub const IMAGE_ID_DIMENSION: &str = "ImageId";

/// The `StandardUnit` value a percentage is published in.
pub const PERCENT_UNIT: &str = "Percent";

/// The `StandardUnit` value a number of bytes is published in.
pub const BYTES_UNIT: &str = "Bytes";

/// The `StandardUnit` value a number of things is published in.
pub const COUNT_UNIT: &str = "Count";

/// Which CADENCE CLASS a metric belongs to: what decides how often it is published, and what
/// its reading is OF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricClass {
    /// A measurement of the guest, published once per period of the instance's monitoring
    /// state: [`BASIC_MONITORING_PERIOD`] under basic monitoring and
    /// [`DETAILED_MONITORING_PERIOD`] under detailed.
    Instance,
    /// The verdict of the status check `which`, "available in 1-minute periods" under basic
    /// monitoring as well as detailed, so always published once per [`STATUS_CHECK_PERIOD`].
    StatusCheck(StatusCheck),
}

/// Which check a status-check metric reports the verdict of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCheck {
    /// The host the instance runs on (`StatusCheckFailed_System`).
    System,
    /// The instance's own software and network stack (`StatusCheckFailed_Instance`).
    Instance,
    /// Both of them at once: "reports whether the instance has passed both the instance status
    /// check and the system status check" (`StatusCheckFailed`), so it fails when either half
    /// did and has a verdict only once both halves have one.
    Both,
}

/// One metric of the `AWS/EC2` instance metric families: its name, the unit it is measured in
/// and the cadence class it is published at.
#[derive(Debug, Clone, Copy)]
pub struct InstanceMetric {
    /// The `MetricName` the series is published under.
    pub name: &'static str,
    /// The `StandardUnit` its datapoints carry.
    pub unit: &'static str,
    /// Which cadence class it belongs to ([`MetricClass`]).
    pub class: MetricClass,
}

/// The metrics a running instance publishes, with the unit of each.
///
/// The catalog and every unit in it come from the metrics Amazon EC2 documents for one INSTANCE
/// (Amazon EC2 User Guide, "Instance metrics" and "Status check metrics"); they are documented
/// metrics rather than Smithy shapes, so there is no model member to read them from. Only the
/// metrics EVERY instance publishes are here: the CPU-credit metrics of a burstable instance
/// measure credits a guest earns and spends, of which this store observes neither, and
/// `StatusCheckFailed_AttachedEBS` reports an attached-volume status this service does not
/// measure at all ([`super::instance_status_checks`] reports none), so neither is claimed.
pub const INSTANCE_METRICS: &[InstanceMetric] = &[
    InstanceMetric {
        name: "CPUUtilization",
        unit: PERCENT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "NetworkIn",
        unit: BYTES_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "NetworkOut",
        unit: BYTES_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "NetworkPacketsIn",
        unit: COUNT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "NetworkPacketsOut",
        unit: COUNT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "DiskReadOps",
        unit: COUNT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "DiskWriteOps",
        unit: COUNT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "DiskReadBytes",
        unit: BYTES_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "DiskWriteBytes",
        unit: BYTES_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "MetadataNoToken",
        unit: COUNT_UNIT,
        class: MetricClass::Instance,
    },
    InstanceMetric {
        name: "StatusCheckFailed",
        unit: COUNT_UNIT,
        class: MetricClass::StatusCheck(StatusCheck::Both),
    },
    InstanceMetric {
        name: "StatusCheckFailed_Instance",
        unit: COUNT_UNIT,
        class: MetricClass::StatusCheck(StatusCheck::Instance),
    },
    InstanceMetric {
        name: "StatusCheckFailed_System",
        unit: COUNT_UNIT,
        class: MetricClass::StatusCheck(StatusCheck::System),
    },
];

/// The period an instance with DETAILED monitoring publishes every metric at: "Detailed
/// monitoring | You can get metrics in 1-minute periods".
pub const DETAILED_MONITORING_PERIOD: Duration = Duration::from_secs(60);

/// The period an instance with BASIC monitoring publishes its guest measurements at: "All other
/// metrics are available in 5-minute periods".
pub const BASIC_MONITORING_PERIOD: Duration = Duration::from_secs(300);

/// The period the status-check metrics are published at, whatever the monitoring state: "Status
/// check metrics are available in 1-minute periods".
pub const STATUS_CHECK_PERIOD: Duration = Duration::from_secs(60);

/// The `StorageResolution` these datapoints are published at, in seconds: the regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How often the clock looks at the store for a sample that has come due.
///
/// Not a publishing schedule — what is published is decided by [`due_sample_instant`] alone —
/// only how soon after a period's boundary the reading of that period is taken. Far shorter than
/// the shortest period above, so no period waits long for the reading it owes.
const SAMPLE_TICK: Duration = Duration::from_secs(5);

/// How long a running clock's claim on the store stands before another call may start one.
///
/// Several ticks long, so a clock that is merely busy keeps its claim.
const CLOCK_LEASE: Duration = Duration::from_secs(30);

/// How many times one clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start. Whatever the clock has
/// not reached when it is spent is sampled by the next clock — every request starts one if none
/// is running — or by the next request's own publish.
const MAX_CLOCK_WAKE_UPS: usize = 720;

/// Whether `monitoring_state` is DETAILED monitoring: the state an instance is in while every
/// one of its metrics is published at the 1-minute cadence.
///
/// `enabled` is detailed monitoring in force, and so is `disabling`: that is the transient state
/// of an `UnmonitorInstances` whose move to `disabled` the clock still owes, so detailed
/// monitoring has not been taken away yet. `pending` is the mirror of it on the way up — a
/// `MonitorInstances` whose move to `enabled` is still owed — which is not detailed monitoring
/// yet, so the instance is still publishing at the basic cadence.
pub fn is_detailed_monitoring(monitoring_state: &str) -> bool {
    monitoring_state == MONITORING_STATE_ENABLED || monitoring_state == MONITORING_STATE_DISABLING
}

/// The period `metric` is published at for an instance whose monitoring state is
/// `monitoring_state`.
pub fn publish_period(metric: &InstanceMetric, monitoring_state: &str) -> Duration {
    match metric.class {
        MetricClass::StatusCheck(_) => STATUS_CHECK_PERIOD,
        MetricClass::Instance if is_detailed_monitoring(monitoring_state) => {
            DETAILED_MONITORING_PERIOD
        }
        MetricClass::Instance => BASIC_MONITORING_PERIOD,
    }
}

/// The START of the `period`-long window `at` falls in, as epoch seconds.
///
/// The windows are anchored at multiples of the period, which is how CloudWatch aggregates one,
/// so this is the last BOUNDARY of that period the clock has passed — the instant the reading of
/// the period behind it was taken at ([`due_sample_instant`]).
pub fn period_start(at: SystemTime, period: Duration) -> i64 {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let length = period.as_secs().max(1);
    i64::try_from(seconds - seconds % length).unwrap_or_default()
}

/// The instant the reading DUE at `at` was taken: the most recent boundary of `period` the clock
/// has passed, as epoch seconds.
///
/// A sample is taken at a period's boundary and measures the period that ended there, so the
/// reading due at any instant is the one belonging to the last boundary behind it: the period the
/// clock is still inside has not been measured yet. That is both when the sample happened and the
/// time stamp its datapoint carries, which is why a reader sees a series that stops at the last
/// boundary the clock has passed, and why a 5-minute series lags further behind it than a
/// 1-minute one.
fn due_sample_instant(at: SystemTime, period: Duration) -> i64 {
    period_start(at, period)
}

/// One datapoint an instance owes: which series it belongs to, what it measured and the instant
/// it was read at.
#[derive(Debug, Clone)]
struct DueSample {
    /// The instance the series is dimensioned on.
    instance_id: String,
    /// The metric it is a reading of.
    metric: &'static InstanceMetric,
    /// The value that reading took.
    value: f64,
    /// The instant it was taken at, which is the time stamp it carries.
    taken_at_secs: i64,
    /// The AGGREGATE series this reading is also a contribution to, as `(dimension name,
    /// dimension value)` — empty for a reading that is published under the instance's own
    /// dimension alone ([`aggregate_dimensions`]).
    aggregates: Vec<(&'static str, String)>,
}

/// Publish every `AWS/EC2` datapoint the account's instances owe, and answer whether this store
/// holds an instance that is publishing at all.
///
/// Call it with NO store lock held: it takes this service's lock to read the instances and
/// CloudWatch's to append the datapoints, one after the other. The answer is what tells the
/// clock whether there is anything left to sample — a store with no running instance publishes
/// nothing until a request starts one.
pub fn publish_instance_metrics<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    publish_instance_metrics_at(engine, SystemTime::now())
}

/// The same publish as [`publish_instance_metrics`], for the instant `now`.
///
/// The instant is a parameter so that one definition serves the request seam, the clock and a
/// test that drives the cadence over a span of periods without waiting for them to pass; every
/// caller in `src/` passes the wall clock.
pub fn publish_instance_metrics_at<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let (samples, agent_samples, publishing) = {
        let mut store = state.lock();
        // Every move the clock already owed is materialised before the instances are read: an
        // instance whose launch has come due is RUNNING at this instant whether or not anybody
        // called, and so is the monitoring move that selects its cadence.
        store.materialize_due_transitions();
        let samples = due_samples(&mut store, now);
        // What the CloudWatch agent in each guest owes at this instant, which is due at its
        // own interval rather than at the instance's cadence ([`due_agent_samples`]), and the
        // datapoints the account's VOLUMES owe for the I/O their guests performed on them, which
        // are measurements of operations that already happened
        // ([`super::volume_telemetry::due_volume_samples`]).
        let mut agent_samples = due_agent_samples(&mut store, now);
        agent_samples.extend(super::volume_telemetry::due_volume_samples(&mut store, now));
        let publishing = store.instances.values().any(publishes_metrics);
        (samples, agent_samples, publishing)
    };
    if !samples.is_empty() {
        append(engine, &samples, now);
    }
    if !agent_samples.is_empty() {
        append_agent_samples(engine, &agent_samples, now);
    }
    publishing
}

/// The datapoints the CLOUDWATCH AGENT in each of this store's guests owes at `now`, recording
/// what it publishes as taken.
///
/// Only a RUNNING instance has a guest, and only a guest whose agent was started and configured
/// has an agent collecting at all ([`super::guest_cloudwatch_agent::due_samples`], which states
/// what one of those datapoints is). What each measurement reads is computed from the machine
/// — its memory and its root filesystem — so the series the agent publishes and what the
/// guest's own programs report about the same machine are one fact.
fn due_agent_samples(
    store: &mut ComAmazonawsEc2Amazonec2State,
    now: SystemTime,
) -> Vec<super::guest_cloudwatch_agent::AgentSample> {
    // What each running agent MEASURES, read while the whole store is to hand: the instance's
    // own record, its instance type's published memory and its root volume's size all have to
    // be read before the instance is taken for the collection itself.
    let measuring: Vec<(String, super::guest_cloudwatch_agent::GuestMeasurements)> = store
        .instances
        .values()
        .filter(|instance| {
            publishes_metrics(instance) && instance.guest_runtime.cloudwatch_agent.is_running()
        })
        .map(|instance| {
            (
                instance.instance_id.clone(),
                guest_measurements(store, instance),
            )
        })
        .collect();
    let mut samples: Vec<super::guest_cloudwatch_agent::AgentSample> = Vec::new();
    for (instance_id, measurements) in measuring {
        let Some(instance) = store.instances.get_mut(&instance_id) else {
            continue;
        };
        samples.extend(super::guest_cloudwatch_agent::due_samples(
            &mut instance.guest_runtime.cloudwatch_agent,
            &measurements,
            now,
        ));
    }
    samples
}

/// What the agent in the guest of `instance` can measure of that machine: its memory
/// ([`super::guest_memory`]) and its root filesystem ([`super::guest_filesystem`]), both read
/// off the stored instance.
fn guest_measurements(
    store: &ComAmazonawsEc2Amazonec2State,
    instance: &InstanceRecord,
) -> super::guest_cloudwatch_agent::GuestMeasurements {
    // What the machine's own storage accounts for: the files it holds and the space its
    // programs reserved, which is the ONE reading the guest's own `df` reports as well.
    let stored_bytes =
        super::guest_filesystem::storage_bytes(&instance.guest_files, &instance.guest_runtime);
    super::guest_cloudwatch_agent::GuestMeasurements {
        memory: super::guest_memory::guest_memory(
            super::instance_type_record(&instance.instance_type).map(|record| record.memory_mib),
        ),
        filesystem: super::guest_cloudwatch_agent::GuestFilesystem::of(
            store
                .launched_root_device(instance.image_id.as_deref())
                .and_then(|(_device, ebs)| ebs.volume_size),
            stored_bytes,
        ),
    }
}

/// Append the datapoints a guest's own activity owes to the CloudWatch metric store, and make the
/// alarms watching those series due for evaluation.
///
/// Two kinds travel this way, and both are measurements of things that already happened: what the
/// CloudWatch AGENT in a guest collected or received ([`super::guest_cloudwatch_agent`]) and the
/// I/O a guest performed on a VOLUME attached to it ([`super::volume_telemetry`]). Each carries the
/// namespace, the dimensions and the instant of its own measurement, so one append serves both.
///
/// The same store, through the same accessor, as the instance's own series
/// ([`append`]): a metric has one home, so these series and every reader of them cannot
/// drift. Each datapoint is ONE observation — a collection the agent took, a value its
/// listener received, or a transfer a guest performed — so a `Sum` over a period is what that
/// period's observations came to and an `Average` is their mean, exactly as a published datapoint
/// of the same series is.
fn append_agent_samples<Eng: Send + Sync + 'static>(
    engine: &Eng,
    samples: &[super::guest_cloudwatch_agent::AgentSample],
    at: SystemTime,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::state::WrittenSeries> = Vec::new();
    for sample in samples {
        #[allow(clippy::cast_precision_loss)]
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs: sample.at_secs as f64,
            sample_count: 1.0,
            sum: sample.value,
            minimum: sample.value,
            maximum: sample.value,
            unit: sample.unit.clone(),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        let key = cloudwatch::state::MetricKey {
            namespace: sample.namespace.clone(),
            metric_name: sample.metric_name.clone(),
            dimensions: sample.dimensions.clone(),
        };
        let unit = datum.unit().to_string();
        if store.append_datapoints(key.clone(), vec![datum]).is_err() {
            // The metric store holds no more series. Whatever request this reading came off was
            // already answered, so nothing is failed for it; the dropped reading is reported
            // where a developer can see it.
            ::tracing::warn!(
                namespace = %sample.namespace,
                metric = %sample.metric_name,
                "the CloudWatch metric store is at capacity, so this guest measurement was not \
                 published"
            );
            continue;
        }
        written.push(cloudwatch::state::WrittenSeries {
            key,
            units: vec![unit],
        });
    }
    store.make_alarms_due(&written, epoch_secs(at));
}

/// Start the one in-process task that keeps sampling this store's instances, and stands down
/// once none publishes or its wake-ups are spent.
///
/// At most one clock runs per store ([`CLOCK_LEASE`]), and with no async runtime to spawn onto
/// there is no clock — the series are then published by the next request alone, as
/// [`publish_instance_metrics`] always does.
pub fn drive_instance_metric_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !claim_clock(&mut state.lock(), SystemTime::now()) {
        return;
    }
    let engine = Arc::clone(engine);
    let handle = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            // Every wake-up waits, so a store with nothing to sample costs a yield rather than
            // a spin, and the runtime is handed back in between.
            ::tokio::time::sleep(SAMPLE_TICK).await;
            let publishing = publish_instance_metrics(&*engine);
            if !renew_clock(&*engine, publishing) {
                return;
            }
        }
        release_clock(&*engine);
    });
    // The handle is kept in the store, so this task can be stopped: the next claim aborts a
    // clock that outlived its lease ([`claim_clock`]).
    state.lock().instance_metric_clock = Some(handle);
}

/// Whether `instance` is publishing its own series at all: only a RUNNING instance is being
/// measured.
fn publishes_metrics(instance: &InstanceRecord) -> bool {
    instance.state == InstanceStateName::Running
}

/// The datapoints the instances of `store` owe at `now`, and the instants they were read at
/// recorded as taken — so each boundary of each series yields exactly one.
///
/// The bookkeeping of instances this store no longer holds goes with them, so a terminated
/// instance that has been reclaimed leaves nothing behind.
fn due_samples(store: &mut ComAmazonawsEc2Amazonec2State, now: SystemTime) -> Vec<DueSample> {
    let ComAmazonawsEc2Amazonec2State {
        instances,
        instance_metric_periods,
        ..
    } = store;
    instance_metric_periods.retain(|instance_id, _| instances.contains_key(instance_id));
    let mut samples: Vec<DueSample> = Vec::new();
    for instance in instances.values() {
        if !publishes_metrics(instance) {
            continue;
        }
        let published = instance_metric_periods
            .entry(instance.instance_id.clone())
            .or_default();
        for metric in INSTANCE_METRICS {
            let period = publish_period(metric, &instance.monitoring_state);
            let taken_at_secs = due_sample_instant(now, period);
            if published.get(metric.name) == Some(&taken_at_secs) {
                continue;
            }
            // The instant the sample was TAKEN, which is the instant its reading is a reading as
            // of: a guest that was not up then was measured by nothing, and the verdict a status
            // check publishes is the verdict it had reached by then.
            let taken_at = instant_at(taken_at_secs);
            if !was_measured_at(instance, taken_at) {
                continue;
            }
            let Some(value) = measurement(instance, metric, taken_at) else {
                continue;
            };
            published.insert(metric.name, taken_at_secs);
            samples.push(DueSample {
                instance_id: instance.instance_id.clone(),
                metric,
                value,
                taken_at_secs,
                aggregates: aggregate_dimensions(instance, metric),
            });
        }
    }
    samples
}

/// The AGGREGATE series one reading of `metric` on `instance` is also a contribution to, as
/// `(dimension name, dimension value)`.
///
/// The two aggregate dimensions are "available for instances with detailed monitoring enabled"
/// (Amazon EC2 User Guide, instance metric dimensions — quoted in the module docs), so an
/// instance publishing at the basic cadence contributes to neither: there is no roll-up of a
/// measurement that was not taken every minute. What is rolled up is a quantity measured on the
/// guest; a status check's verdict is about one named instance, so it is published under that
/// instance's own dimension alone.
///
/// A dimension whose value this store does not hold for the instance — an instance with no
/// image recorded for it — yields no series rather than one dimensioned on nothing.
fn aggregate_dimensions(
    instance: &InstanceRecord,
    metric: &InstanceMetric,
) -> Vec<(&'static str, String)> {
    if !matches!(metric.class, MetricClass::Instance)
        || !is_detailed_monitoring(&instance.monitoring_state)
    {
        return Vec::new();
    }
    [
        (INSTANCE_TYPE_DIMENSION, Some(instance.instance_type.clone())),
        (IMAGE_ID_DIMENSION, instance.image_id.clone()),
    ]
    .into_iter()
    .filter_map(|(dimension, value)| {
        let value = value?;
        (!value.is_empty()).then_some((dimension, value))
    })
    .collect()
}

/// Whether the guest of `instance` was up at `taken_at`, the instant a sample was taken.
///
/// A reading taken before this guest came up is a reading of nothing of this instance, so there
/// is none: an instance that has just launched has no history of instants it did not exist at,
/// and its series therefore carries nothing stamped before its launch. The instant measured
/// against is the boot the guest actually had ([`super::instance_boots`]), which is also what the
/// status checks are measured from.
fn was_measured_at(instance: &InstanceRecord, taken_at: SystemTime) -> bool {
    super::instance_boots(instance)
        .last()
        .is_some_and(|boot| boot.booted_at < taken_at)
}

/// The instant the epoch second `secs` names, or the epoch itself for a second no instant can be
/// built from — a seconds count beyond what the platform's clock can represent is no instant to
/// take a reading at rather than a reason to unwind the sampler.
fn instant_at(secs: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(u64::try_from(secs).unwrap_or_default()))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// What `metric` measures on `instance` as of `at`, or `None` when there is no measurement to
/// publish for it in this period.
///
/// Every guest measurement is zero for the reason the module docs give. A status-check metric
/// carries the verdict this service really reached for that check
/// ([`super::instance_status_checks`]), and a check that has reached no verdict yet — one whose
/// guest was still initializing — has nothing to publish for that period.
fn measurement(instance: &InstanceRecord, metric: &InstanceMetric, at: SystemTime) -> Option<f64> {
    let MetricClass::StatusCheck(which) = metric.class else {
        return Some(NOTHING_MEASURED);
    };
    let (system, instance_check) = instance_status_checks(instance, at);
    match which {
        StatusCheck::System => check_verdict(&system),
        StatusCheck::Instance => check_verdict(&instance_check),
        StatusCheck::Both => check_verdict(&system)
            .zip(check_verdict(&instance_check))
            .map(|(system, instance)| system.max(instance)),
    }
}

/// What one status check's summary publishes: 0 for a check that passed, 1 for one that failed,
/// and `None` for a summary that is no verdict at all — a check still initializing, or one that
/// does not apply.
fn check_verdict(summary: &crate::ops::types::InstanceStatusSummary) -> Option<f64> {
    match summary.status.as_deref()? {
        SUMMARY_STATUS_OK => Some(CHECK_PASSED),
        SUMMARY_STATUS_IMPAIRED => Some(CHECK_FAILED),
        _ => None,
    }
}

/// What every guest measurement of every instance this emulator holds measured: nothing, so
/// zero — see the module docs.
const NOTHING_MEASURED: f64 = 0.0;

/// What a status check that PASSED publishes: "0 (passed)" of "0 (passed) or 1 (failed)".
const CHECK_PASSED: f64 = 0.0;

/// What one that FAILED publishes: the "1 (failed)" of the same sentence.
const CHECK_FAILED: f64 = 1.0;

/// Append the samples to the CloudWatch metric store, and make the alarms watching those series
/// due for evaluation.
///
/// Each reading is appended to the instance's OWN series and to every aggregate series it is a
/// contribution to ([`DueSample::aggregates`]): CloudWatch aggregates the datapoints a period
/// holds, so one contribution per instance per period is what makes an aggregate period's
/// statistics the statistics of the instances measured in it.
///
/// The datapoints go into the CloudWatch emulator's OWN store — the same one `ListMetrics`,
/// `GetMetricStatistics` and the alarm evaluation read — so one metric has one home and the
/// publisher and its readers cannot drift. They never travel over `PutMetricData`, which refuses
/// a publish into a namespace the services own.
fn append<Eng: Send + Sync + 'static>(engine: &Eng, samples: &[DueSample], at: SystemTime) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::state::WrittenSeries> = Vec::new();
    for sample in samples {
        let datum = cloudwatch::state::StoredDatum {
            // The instant the reading was taken at: the reading is the reading of the period
            // behind that instant, so an `Average`, a `Minimum` and a `Maximum` over the window
            // holding it all answer from this one datapoint.
            #[allow(clippy::cast_precision_loss)]
            timestamp_secs: sample.taken_at_secs as f64,
            // One observation: the datapoint IS the reading taken of the instance.
            sample_count: 1.0,
            sum: sample.value,
            minimum: sample.value,
            maximum: sample.value,
            unit: Some(sample.metric.unit.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        let own = (INSTANCE_ID_DIMENSION, sample.instance_id.clone());
        for (dimension, value) in std::iter::once(own).chain(sample.aggregates.iter().cloned()) {
            let key = cloudwatch::state::MetricKey {
                namespace: NAMESPACE.to_string(),
                metric_name: sample.metric.name.to_string(),
                dimensions: [(dimension.to_string(), value)].into_iter().collect(),
            };
            if store
                .append_datapoints(key.clone(), vec![datum.clone()])
                .is_err()
            {
                // The metric store holds no more series. Whatever request this reading came off
                // was already answered, so nothing is failed for it; the dropped reading is
                // reported where a developer can see it.
                ::tracing::warn!(
                    instance = %sample.instance_id,
                    metric = %sample.metric.name,
                    dimension = %dimension,
                    "the CloudWatch metric store is at capacity, so this instance's measurement \
                     was not published"
                );
                continue;
            }
            written.push(cloudwatch::state::WrittenSeries {
                key,
                units: vec![sample.metric.unit.to_string()],
            });
        }
    }
    // An instance alarm is an alarm on one of these series, so the arriving datapoints are what
    // makes its next evaluation due — exactly as a published datapoint does.
    store.make_alarms_due(&written, epoch_secs(at));
}

/// `at` as the epoch seconds CloudWatch's own readers window by.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Take the clock claim for this store when no clock holds a live lease, reporting whether the
/// caller may start one.
///
/// The task of a clock whose lease has lapsed without being released is ABORTED as the claim is
/// taken: such a clock has either ended already (where the abort does nothing) or is stuck, and
/// leaving a stuck one running beside the new one would be two clocks over one store.
fn claim_clock(store: &mut ComAmazonawsEc2Amazonec2State, now: SystemTime) -> bool {
    if store
        .instance_metric_clock_until
        .is_some_and(|until| now < until)
    {
        return false;
    }
    if let Some(previous) = store.instance_metric_clock.take() {
        previous.abort();
    }
    store.instance_metric_clock_until = now.checked_add(CLOCK_LEASE);
    true
}

/// Renew the running clock's lease, reporting whether it is to keep sampling.
///
/// A store with no instance that publishes has nothing to sample, so the clock releases its
/// claim and stands down; the next request starts one again.
fn renew_clock<Eng: Send + Sync + 'static>(engine: &Eng, publishing: bool) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    if !publishing {
        release(&mut store);
        return false;
    }
    store.instance_metric_clock_until = SystemTime::now().checked_add(CLOCK_LEASE);
    true
}

/// Release the claim of a clock that has spent its wake-ups.
fn release_clock<Eng: Send + Sync + 'static>(engine: &Eng) {
    if let Some(state) = service_state(engine) {
        release(&mut state.lock());
    }
}

/// Give up the clock claim: no clock holds the lease, and none holds a task either.
fn release(store: &mut ComAmazonawsEc2Amazonec2State) {
    store.instance_metric_clock_until = None;
    store.instance_metric_clock = None;
}
