// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE and the in-process clock every `AWS/RDS` series this service publishes shares:
//! how a series is addressed, how far apart its datapoints are stamped, where one datapoint is
//! appended to, and the one bounded task that waits for the next due instant.
//!
//! Amazon RDS reports its resources through CloudWatch: no API operation of this service answers
//! how many connections a DB instance is serving or how many capacity units a cluster is running
//! on, so the ONE place a client can watch either is the series Amazon RDS publishes for it. Each
//! of those series is published the same way, and this module is that way — the publishers above
//! it ([`super::instance_metrics`], [`super::serverless_capacity`]) decide only WHICH series
//! exist and what one observation of each reads.
//!
//! Three things are shared here:
//!
//! * the ADDRESS of a series ([`series`]): a metric's identity includes its dimension set, so the
//!   publish and every reader that looks for what it stored name the same key.
//! * the SCHEDULE of one series ([`SeriesClock`]): a datapoint every [`PUBLISH_INTERVAL`], which
//!   is the finest period a regular-resolution metric is read and alarmed at, with
//!   [`RUN_UP_PERIODS`] datapoints already behind a series that opens — so a reader, or an alarm
//!   created straight after the resource, finds a series that has been running.
//! * the CLOCK ([`drive`]): the writes that bring such a resource into being publish whatever is
//!   already due and start the one bounded in-process task that waits for the next due instant.
//!   Nothing here is on a READ path, and nothing it publishes depends on who called: every
//!   datapoint it materialises is one the stored due instants had already made due.
//!
//! The datapoints are appended to the CloudWatch emulator's OWN metric store — the same store
//! `PutMetricData` writes and `GetMetricStatistics` reads — reached through the shared engine, so
//! one metric has one home. They go into it directly rather than through `PutMetricData`, which
//! reserves the `AWS/` namespaces for the services that own them ("you should not specify a
//! namespace that begins with `AWS/`", `PutMetricData$Namespace`).
//!
//! Every instant here is the EMULATOR's clock ([`::service_common::clock`]), which is the process
//! clock unless an operator stops it: a datapoint that is due only after a span has passed is due
//! when that clock says so.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::ComAmazonawsRdsAmazonrdsv19State as RdsState;

/// The namespace Amazon RDS publishes the metrics of its DB resources into.
pub const METRIC_NAMESPACE: &str = "AWS/RDS";

/// The dimension naming the DB cluster an observation is about: its `DBClusterIdentifier`.
pub const DB_CLUSTER_IDENTIFIER_DIMENSION: &str = "DBClusterIdentifier";

/// The dimension naming the DB INSTANCE an observation is about: its `DBInstanceIdentifier`.
pub const DB_INSTANCE_IDENTIFIER_DIMENSION: &str = "DBInstanceIdentifier";

/// How far apart the datapoints of one series are STAMPED: the minute a regular-resolution
/// metric is read and alarmed at.
pub const PUBLISH_INTERVAL: Duration = Duration::from_secs(60);

/// The `StorageResolution` these datapoints are stored at, in seconds: the regular resolution a
/// metric has unless its publisher asks for the high-resolution one ("This field is optional, if
/// you do not specify it the default of 60 is used", `MetricDatum$StorageResolution`).
const STORAGE_RESOLUTION_SECS: i32 = 60;

/// How many datapoints a series OPENS with, stamped one [`PUBLISH_INTERVAL`] apart ahead of the
/// instant the resource became available.
///
/// The emulator's own figure, not an AWS one: the real series has been running for as long as the
/// resource has, so a reader — or an alarm created straight after — sees as many periods of data
/// as it asks for. Five periods is the run-up of the longest evaluation an `EvaluationPeriods` of
/// one to five asks for.
const RUN_UP_PERIODS: u32 = 5;

/// How far apart the datapoints of one series come DUE while the series is still catching up with
/// the present.
///
/// The emulator's own span: the run-up is published in a little over a second, so a caller
/// reading the series the instant the resource became available still finds the empty series a
/// resource that has published nothing yet has.
const ACCELERATED_SPACING: Duration = Duration::from_millis(250);

/// The shortest a clock wake-up waits, so a datapoint already past its due instant still costs a
/// yield rather than a CPU-bound spin.
const MIN_PUBLISH_WAIT: Duration = Duration::from_millis(50);

/// The longest a clock wake-up waits, so the clock re-reads the store — which a write, a session
/// or an advance of the emulator's clock may have changed since — at least this often.
const MAX_PUBLISH_WAIT: Duration = Duration::from_secs(5);

/// How many times one clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and the next write that
/// brings such a resource into being starts a clock again if none is running.
const MAX_CLOCK_WAKE_UPS: usize = 2_000;

/// How many datapoints ONE pass of one publisher appends, over every series it holds.
///
/// A bound on the work one wake-up does, not a quota: a resource this pass has no room for keeps
/// the due instant it carries, so the next wake-up publishes its minute intact.
pub const MAX_DATAPOINTS_PER_PASS: usize = 2_000;

/// The series of `metric_name` reported at the one dimension `dimension`=`value`.
///
/// The one place a published series is addressed, so the publish and every reader that looks for
/// what it stored name the same series.
#[must_use]
pub fn series(metric_name: &str, dimension: &str, value: &str) -> cloudwatch::MetricKey {
    let mut dimensions: BTreeMap<String, String> = BTreeMap::new();
    dimensions.insert(dimension.to_string(), value.to_string());
    cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions,
    }
}

/// Every Region this store keeps DB resource records for.
///
/// What a publisher walks: the records of one Region are an ordered range its own quota bounds, so
/// a pass costs the resources the store holds rather than a scan of the whole map.
#[must_use]
pub fn regions(store: &RdsState) -> Vec<String> {
    store
        .db_instances
        .keys()
        .chain(store.db_clusters.keys())
        .map(|(region, _)| region.clone())
        .collect::<HashSet<String>>()
        .into_iter()
        .collect()
}

/// One series' place in its schedule: the stamp its next datapoint carries, and when that
/// datapoint comes due.
#[derive(Debug)]
pub struct SeriesClock {
    /// The time stamp the next datapoint carries. Advances by [`PUBLISH_INTERVAL`] per publish,
    /// so the series keeps the spacing the real one has.
    next_stamp: SystemTime,
    /// The instant the next datapoint comes due, on the emulator's clock.
    next_due_at: Instant,
}

impl SeriesClock {
    /// The schedule a series that OPENS now carries: its run-up already behind it
    /// ([`RUN_UP_PERIODS`]) and its first datapoint due at once, so the pass that finds the
    /// resource available publishes the series' first datapoints itself.
    #[must_use]
    pub fn opening(now: SystemTime, instant: Instant) -> Self {
        let run_up = PUBLISH_INTERVAL.saturating_mul(RUN_UP_PERIODS);
        Self {
            next_stamp: now.checked_sub(run_up).unwrap_or(now),
            next_due_at: instant,
        }
    }

    /// The instant this series' next datapoint comes due.
    #[must_use]
    pub fn next_due_at(&self) -> Instant {
        self.next_due_at
    }

    /// Whether this series' next datapoint is due at `instant`.
    #[must_use]
    pub fn is_due(&self, instant: Instant) -> bool {
        instant >= self.next_due_at
    }

    /// Take the time stamp of the datapoint that is due and schedule the next one, `instant`
    /// being what the emulator's clock read when this pass started.
    ///
    /// While the series is still behind the present it is published at the accelerated spacing;
    /// once it has caught up, the next datapoint is due one publish interval later, which is the
    /// spacing the real series has.
    pub fn advance(&mut self, instant: Instant) -> SystemTime {
        let stamp = self.next_stamp;
        self.next_stamp = stamp.checked_add(PUBLISH_INTERVAL).unwrap_or(stamp);
        let wait = self
            .next_stamp
            .duration_since(SystemTime::now())
            .unwrap_or(ACCELERATED_SPACING)
            .min(PUBLISH_INTERVAL);
        self.next_due_at = instant.checked_add(wait).unwrap_or(instant);
        stamp
    }
}

/// The ONE datapoint an observation of a series is published as: a single measurement of `value`
/// stamped at `stamp`.
///
/// `unit` is the `StandardUnit` the measurement is reported in, or `None` for a quantity that is
/// no member of that vocabulary — the metric store reports a datapoint published without one
/// under the value the enumerated type has for a measurement that has none.
#[must_use]
pub fn observation(stamp: SystemTime, value: f64, unit: Option<String>) -> cloudwatch::StoredDatum {
    cloudwatch::StoredDatum {
        timestamp_secs: epoch_secs(stamp),
        sample_count: 1.0,
        sum: value,
        minimum: value,
        maximum: value,
        unit,
        storage_resolution: STORAGE_RESOLUTION_SECS,
    }
}

/// The single in-process clock of ONE publisher: which claim is running, and the task that holds
/// it.
#[derive(Debug, Default)]
pub struct PublisherClock {
    /// Whether a clock is running for this publisher.
    running: bool,
    /// Which claim the running clock holds, so a clock that has stood down cannot release a claim
    /// a later one took.
    generation: u64,
    /// The running clock's task, held so the task a request started stays stoppable.
    task: Option<::tokio::task::JoinHandle<()>>,
}

/// Where one publisher's clock lives on the store, so the claim is the store's own.
type ClockOf = fn(&mut RdsState) -> &mut PublisherClock;

/// Publish every datapoint one publisher's clock has already made due, and start the one bounded
/// in-process task that waits for the next one.
///
/// Called by the writes that bring such a resource into being — never by a read: the datapoints
/// this materialises are the ones the stored due instants have already made due, so what a caller
/// sees does not depend on whether anybody called.
///
/// With no async runtime to spawn onto there is no clock, and what is due at the moment of the
/// write is then the whole series until the next write — the router always runs on one.
///
/// * `clock_of` reaches this publisher's claim on the store.
/// * `publish_due` appends whatever that publisher owes at the instant it is called.
/// * `next_due_at` answers when that publisher next owes anything, as the emulator's clock stands
///   at the instant it is passed, or `None` when it owes nothing at all and the clock may stand
///   down.
pub fn drive<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    clock_of: ClockOf,
    publish_due: fn(&Eng),
    next_due_at: fn(&RdsState, Instant) -> Option<Instant>,
) {
    publish_due(&**engine);
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim(clock_of(&mut state.lock())) else {
        return;
    };
    let engine = Arc::downgrade(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            // The emulator this clock publishes for, for as long as it is still there: the handle
            // is WEAK and is upgraded per wake-up, so a clock cannot keep an emulator nobody else
            // holds — its listeners, its engines and its stores — alive, and it stands down with
            // it.
            let Some(alive) = engine.upgrade() else {
                break;
            };
            let read_at = ::service_common::clock::now(&*alive);
            let wait = {
                // Observing "nothing is owed" and standing down happen in ONE critical section,
                // which is what makes the hand-over race-free: a write holds this same lock to
                // store the resource BEFORE it tries to claim the clock, so either this read sees
                // that resource and the running clock keeps it, or the write's claim finds the
                // flag cleared and starts a clock of its own.
                let mut store = state.lock();
                match next_due_at(&store, read_at) {
                    Some(at) => at
                        .saturating_duration_since(read_at)
                        .clamp(MIN_PUBLISH_WAIT, MAX_PUBLISH_WAIT),
                    None => {
                        release(clock_of(&mut store), generation);
                        // Nothing is going to publish, and the claim is already released.
                        return;
                    }
                }
            };
            drop(alive);
            // Every wake-up waits, even for an instant already past: that is what keeps a due
            // datapoint from costing a CPU-bound spin, and it hands the runtime back between
            // wake-ups.
            ::tokio::time::sleep(wait).await;
            let Some(alive) = engine.upgrade() else {
                break;
            };
            publish_due(&*alive);
        }
        // Only THIS clock's claim, so a clock started since keeps its own.
        release(clock_of(&mut state.lock()), generation);
    });
    install(clock_of(&mut claimed.lock()), generation, handle);
}

/// Claim one publisher's clock: the claim's GENERATION when the caller may start one, `None` when
/// one already runs.
fn claim(clock: &mut PublisherClock) -> Option<u64> {
    if clock.running {
        return None;
    }
    clock.running = true;
    clock.generation = clock.generation.wrapping_add(1);
    Some(clock.generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended on the wake-up
/// cap cannot clear a claim a clock started since is holding.
fn release(clock: &mut PublisherClock, generation: u64) {
    if clock.generation == generation {
        clock.running = false;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood down
/// installs nothing and aborts nothing.
fn install(clock: &mut PublisherClock, generation: u64, handle: ::tokio::task::JoinHandle<()>) {
    if clock.generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = clock.task.replace(handle) {
        previous.abort();
    }
}

/// Append the datapoints one pass published to the metric store, and make every alarm over the
/// series they belong to due for evaluation.
///
/// Nothing is published on an engine that holds no metric store to reach: a datapoint has one
/// home, and this path never makes a second one. An alarm is evaluated FROM these datapoints, so
/// storing a series it watches is what makes its evaluation due — an alarm cannot tell a
/// service's own metric from one a client published.
pub fn append<Eng: Send + Sync + 'static>(
    engine: &Eng,
    published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)>,
    now: SystemTime,
) {
    if published.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::WrittenSeries> = Vec::with_capacity(published.len());
    for (key, datum) in published {
        let watched = key.clone();
        let metric = key.metric_name.clone();
        let unit = datum.unit().to_string();
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The resource itself is up either way, so
            // nothing is failed for it; the dropped observation is reported where a developer can
            // see it.
            ::tracing::warn!(
                metric = %metric,
                "the CloudWatch metric store is at capacity, so this observation was dropped"
            );
            continue;
        }
        written.push(cloudwatch::WrittenSeries {
            key: watched,
            units: vec![unit],
        });
    }
    store.make_alarms_due(&written, epoch_secs(now));
}

/// `at` as the epoch seconds a stored datapoint carries, which is the unit the metric store's own
/// readers window by. An instant before the epoch (which no clock reports) reads as the epoch
/// itself rather than failing the publish.
#[must_use]
pub fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}
