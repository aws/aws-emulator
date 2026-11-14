// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch series a provisioned cluster publishes about its own HEALTH, and the one
//! in-process clock that keeps publishing them.
//!
//! Amazon Redshift reports the health of every cluster it runs as the
//! [`HEALTH_STATUS_METRIC`] metric of the [`METRIC_NAMESPACE`] namespace: once a minute the
//! service connects to the cluster's database and runs a simple query, and publishes whether
//! that succeeded as a 0/1 flag — [`HEALTHY`] for a cluster whose database answered,
//! [`UNHEALTHY`] for one that did not. No API call of the client's reports that check, so the
//! series is the only way a client can watch a cluster's health: a dashboard, an alarm or a
//! runbook reading it against a cluster that publishes nothing cannot tell a healthy cluster
//! from one that does not exist.
//!
//! The same observation is published at TWO dimension sets, which are two independent series
//! because a metric's identity includes its dimension set:
//!
//! * cluster-wide, dimensioned by [`CLUSTER_IDENTIFIER_DIMENSION`] alone;
//! * per NODE, dimensioned by [`CLUSTER_IDENTIFIER_DIMENSION`] together with
//!   [`NODE_ID_DIMENSION`], whose value is the node's own ROLE as the dimension spells it
//!   ([`node_id`]) — so the single node of a `single-node` cluster publishes one node series
//!   and a `multi-node` cluster publishes one for its leader and one per compute node.
//!
//! Which node series exist therefore follows from the cluster's own topology
//! ([`super::cluster_nodes`], what `DescribeClusters` reports in `Cluster$ClusterNodes`) and
//! from nothing else: a dimension set no node of the cluster has is a series nobody published,
//! which is what a read of it finds.
//!
//! The datapoints are appended to the CloudWatch emulator's OWN metric store — the same store
//! `PutMetricData` writes and `ListMetrics` / `GetMetricStatistics` / `GetMetricData` read —
//! reached through the shared engine, so one metric has one home. They go into it directly
//! rather than through `PutMetricData`, which reserves the `AWS/` namespaces for the services
//! that own them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`).
//!
//! # The clock the series is published on
//!
//! A cluster publishes one datapoint per MINUTE ([`PUBLISH_INTERVAL`]), so a client watching a
//! freshly created cluster — or an alarm whose evaluation spans several periods — would have
//! minutes to sit through against an emulator. So the series is published on an ACCELERATED
//! schedule: its datapoints come due [`ACCELERATED_SPACING`] apart until the series reaches the
//! present and one publish interval apart from then on, while every datapoint keeps the
//! one-minute SPACING the real series has. A series therefore opens with [`RUN_UP_PERIODS`]
//! datapoints stamped BEFORE the instant the cluster became available, so a reader finds a
//! series that has been running.
//!
//! What makes that observable without any further call is [`drive_cluster_health_metrics`]: a
//! write that brings a cluster into being publishes whatever is already due and starts the one
//! bounded in-process task that waits for the next due instant. Nothing here is on a READ
//! path, and nothing it publishes depends on who called: the datapoints it materialises are
//! the ones the stored due instants have already made due.

use ::std::collections::{BTreeMap, HashMap, HashSet};
use ::std::sync::Arc;
use ::std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::{
    availability_status, ComAmazonawsRedshiftRedshiftserviceversion20121201State as RedshiftState,
    StoredCluster,
};

/// The namespace Amazon Redshift publishes the metrics of a provisioned cluster into.
pub const METRIC_NAMESPACE: &str = "AWS/Redshift";

/// The metric reporting whether one cluster — or one of its nodes — is healthy.
pub const HEALTH_STATUS_METRIC: &str = "HealthStatus";

/// The dimension naming the cluster an observation is about: its `ClusterIdentifier`.
pub const CLUSTER_IDENTIFIER_DIMENSION: &str = "ClusterIdentifier";

/// The dimension naming the NODE an observation is about, carried beside
/// [`CLUSTER_IDENTIFIER_DIMENSION`] on the per-node series.
pub const NODE_ID_DIMENSION: &str = "NodeID";

/// What the flag reads when the cluster's database answered the health query.
pub const HEALTHY: f64 = 1.0;

/// What it reads when the cluster's database did not: the cluster is not available for
/// queries, so the query a health check runs cannot succeed.
pub const UNHEALTHY: f64 = 0.0;

/// How far apart the datapoints of one series are STAMPED: Amazon Redshift checks a cluster's
/// health once a minute, which is also the finest period a regular-resolution metric can be
/// read or alarmed at.
pub const PUBLISH_INTERVAL: Duration = Duration::from_secs(60);

/// The `StorageResolution` these datapoints are stored at, in seconds: the regular resolution
/// a metric has unless its publisher asks for the high-resolution one ("This field is
/// optional, if you do not specify it the default of 60 is used",
/// `MetricDatum$StorageResolution`), which is the minute the health check runs per.
const STORAGE_RESOLUTION_SECS: i32 = 60;

/// How many datapoints a series OPENS with, stamped one [`PUBLISH_INTERVAL`] apart ahead of
/// the instant the cluster became available.
///
/// The emulator's own figure, not an AWS one: the real series has been running for as long as
/// the cluster has been up, so a reader — or an alarm created straight after — sees as many
/// periods of data as it asks for. Five periods is the run-up of the longest evaluation an
/// `EvaluationPeriods` of one to five asks for.
const RUN_UP_PERIODS: u32 = 5;

/// How far apart the datapoints of one series come DUE while the series is still catching up
/// with the present.
///
/// The emulator's own span: the run-up is published in a little over a second, so a caller
/// reading the series the instant the cluster became available still finds the empty series a
/// cluster that has published nothing yet has.
const ACCELERATED_SPACING: Duration = Duration::from_millis(250);

/// The shortest a clock wake-up waits, so a datapoint already past its due instant still
/// costs a yield rather than a CPU-bound spin.
const MIN_PUBLISH_WAIT: Duration = Duration::from_millis(50);

/// The longest a clock wake-up waits, so the clock re-reads the store — which a write may have
/// changed since — at least this often.
const MAX_PUBLISH_WAIT: Duration = Duration::from_secs(5);

/// How many times one clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and the next write
/// that brings a cluster into being starts a clock again if none is running.
const MAX_CLOCK_WAKE_UPS: usize = 2_000;

/// How many datapoints ONE pass publishes, over every series the store holds.
///
/// A bound on the work one wake-up does, not a quota: a cluster this pass has no room for
/// keeps the due instant it carries, so the next wake-up publishes its minute intact rather
/// than the pass doing unbounded work while a clock tick is being served. One cluster needs
/// its cluster-wide datapoint plus one per node, which
/// [`super::MAX_NODES_PER_CLUSTER`] bounds far below this cap, so no cluster can be too big
/// for a pass of its own.
const MAX_DATAPOINTS_PER_PASS: usize = 2_000;

/// How the [`NODE_ID_DIMENSION`] dimension spells the node whose `ClusterNode$NodeRole` is
/// `node_role`.
///
/// The roles a cluster reports are upper case (`SHARED`, `LEADER`, `COMPUTE-0`, …,
/// [`super::cluster_nodes`]) while the dimension names a node in title case with its position
/// kept as it stands (`Shared`, `Leader`, `Compute-0`): one rule over the whole role
/// vocabulary — the role word in title case, and the suffix after the first hyphen
/// verbatim — rather than a case per role, so a role this service grows later is spelled the
/// same way without another table to keep in step.
#[must_use]
pub fn node_id(node_role: &str) -> String {
    let (word, position) = match node_role.split_once('-') {
        Some((word, position)) => (word, Some(position)),
        None => (node_role, None),
    };
    let lowered = word.to_ascii_lowercase();
    let mut spelled = String::with_capacity(node_role.len());
    let mut characters = lowered.chars();
    if let Some(first) = characters.next() {
        spelled.extend(first.to_uppercase());
        spelled.push_str(characters.as_str());
    }
    if let Some(position) = position {
        spelled.push('-');
        spelled.push_str(position);
    }
    spelled
}

/// The cluster-wide series of the cluster `cluster_identifier`: the one dimensioned by its
/// identifier alone.
#[must_use]
pub fn cluster_series(cluster_identifier: &str) -> cloudwatch::MetricKey {
    series(cluster_identifier, None)
}

/// The series of ONE node of that cluster: the one dimensioned by its identifier together with
/// the node's [`NODE_ID_DIMENSION`].
#[must_use]
pub fn node_series(cluster_identifier: &str, node_id: &str) -> cloudwatch::MetricKey {
    series(cluster_identifier, Some(node_id))
}

/// The one place a published series is addressed, so the publish and every reader that looks
/// for what it stored name the same series.
fn series(cluster_identifier: &str, node_id: Option<&str>) -> cloudwatch::MetricKey {
    let mut dimensions: BTreeMap<String, String> = BTreeMap::new();
    dimensions.insert(
        CLUSTER_IDENTIFIER_DIMENSION.to_string(),
        cluster_identifier.to_string(),
    );
    if let Some(node_id) = node_id {
        dimensions.insert(NODE_ID_DIMENSION.to_string(), node_id.to_string());
    }
    cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: HEALTH_STATUS_METRIC.to_string(),
        dimensions,
    }
}

/// One series' place in its schedule: the stamp its next datapoint carries, and when that
/// datapoint comes due.
#[derive(Debug)]
struct SeriesClock {
    /// The time stamp the next datapoint carries. Advances by [`PUBLISH_INTERVAL`] per
    /// publish, so the series keeps the spacing the real one has.
    next_stamp: SystemTime,
    /// The instant the next datapoint comes due; see [`ACCELERATED_SPACING`].
    next_due_at: Instant,
}

/// Where every health series of this store has got to, and the one in-process clock that
/// advances them.
///
/// Held on the store so the schedule is the store's own: a series exists while the cluster it
/// belongs to does, and the clock is claimed once however many writes drive it.
#[derive(Debug, Default)]
pub struct HealthStatusPublisher {
    /// The schedule of each cluster being health-checked, keyed by `ClusterIdentifier`.
    series: HashMap<String, SeriesClock>,
    /// Whether a clock is running for this store.
    clock_running: bool,
    /// Which claim the running clock holds, so a clock that has stood down cannot release a
    /// claim a later one took.
    clock_generation: u64,
    /// The running clock's task, held so the task a request started stays stoppable.
    clock: Option<::tokio::task::JoinHandle<()>>,
}

/// Publish every health datapoint the clock has already made due, and start the one bounded
/// in-process task that waits for the next one.
///
/// Called by the writes that bring a cluster into being — never by a read: the datapoints this
/// materialises are the ones the stored due instants have already made due, so what a caller
/// sees does not depend on whether anybody called.
///
/// With no async runtime to spawn onto there is no clock, and what is due at the moment of the
/// write is then the whole series until the next write — the router always runs on one.
pub fn drive_cluster_health_metrics<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    publish_due_health_status(&**engine);
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_clock(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a clock that ran out of work
        // has already released its claim inside the one critical section that observed it.
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let read_at = ::service_common::clock::now(&*engine);
            let Some(wait) = next_wait_or_release(&mut state.lock(), generation, read_at) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps a due
            // datapoint from costing a CPU-bound spin, and it hands the runtime back between
            // wake-ups.
            ::tokio::time::sleep(wait).await;
            publish_due_health_status(&*engine);
        }
        if capped {
            // Only THIS clock's claim, so a clock started since keeps its own.
            release_clock(&mut state.lock(), generation);
        }
    });
    set_clock(&mut claimed.lock(), generation, handle);
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood down
/// installs nothing and aborts nothing.
fn set_clock(store: &mut RedshiftState, generation: u64, handle: ::tokio::task::JoinHandle<()>) {
    if store.health_metrics.clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.health_metrics.clock.replace(handle) {
        previous.abort();
    }
}

/// Claim the single publisher clock for this store: the claim's GENERATION when the caller may
/// start one, `None` when one already runs.
fn claim_clock(store: &mut RedshiftState) -> Option<u64> {
    if store.health_metrics.clock_running {
        return None;
    }
    store.health_metrics.clock_running = true;
    store.health_metrics.clock_generation = store.health_metrics.clock_generation.wrapping_add(1);
    Some(store.health_metrics.clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended on the
/// wake-up cap cannot clear a claim a clock started since is holding.
fn release_clock(store: &mut RedshiftState, generation: u64) {
    if store.health_metrics.clock_generation == generation {
        store.health_metrics.clock_running = false;
    }
}

/// How long the clock waits before its next pass, or `None` — having RELEASED its claim — when
/// the store holds nothing that is going to publish.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section, which is what
/// makes the hand-over race-free: a write holds this same lock to store the cluster BEFORE it
/// tries to claim the clock, so either this call sees that cluster and the running clock keeps
/// it, or the write's claim finds the flag cleared and starts a clock of its own.
fn next_wait_or_release(
    store: &mut RedshiftState,
    generation: u64,
    now: Instant,
) -> Option<Duration> {
    let due = store
        .health_metrics
        .series
        .values()
        .map(|series| series.next_due_at)
        .min();
    // A cluster the service is still bringing up publishes nothing yet, and its first
    // datapoint is owed the moment it becomes available — which the clock the store holds for
    // it already fixes.
    let coming_up = store
        .clusters
        .iter()
        .filter(|stored| !is_checked(stored))
        .map(|stored| stored.available_at)
        .min();
    match [due, coming_up].into_iter().flatten().min() {
        Some(at) => Some(
            at.saturating_duration_since(now)
                .clamp(MIN_PUBLISH_WAIT, MAX_PUBLISH_WAIT),
        ),
        None => {
            release_clock(store, generation);
            None
        }
    }
}

/// Publish the datapoint every series that is due is reported as.
///
/// Three steps, so no store guard is held over another service's lock and the two stores are
/// never locked at once:
///
/// 1. under THIS service's guard, materialise what the clock owes the clusters, open a series
///    for every cluster that is being health-checked and close the ones whose cluster is gone;
/// 2. still under that guard, take the datapoints the due series are published as;
/// 3. with the metric store's guard, append them and make the alarms over them due.
fn publish_due_health_status<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    // The pass is dated by the EMULATOR's clock, like every transition it
    // materialises: a clock that stands still owes no datapoint yet.
    let instant = ::service_common::clock::now(engine);
    let published = {
        let mut store = state.lock();
        // A cluster whose provisioning span has passed is available at this instant whether or
        // not anybody called, and an available cluster is what the service health-checks — so
        // the transitions the clock owes are materialised before the pass reads the statuses.
        store.settle(instant);
        sync_series(&mut store, now, instant);
        due_datapoints(&mut store, instant)
    };
    append_datapoints(engine, published, now);
}

/// Whether this cluster is one the service health-checks: it has a database to connect to,
/// which a cluster has from the moment it is first available.
///
/// A cluster still being provisioned has none yet, so it publishes nothing at all — which is
/// what makes a series open at the instant the cluster becomes available and not before.
fn is_checked(stored: &StoredCluster) -> bool {
    stored
        .cluster
        .cluster_status
        .as_deref()
        .is_some_and(|status| status.starts_with(super::STATUS_AVAILABLE))
}

/// What one health check of this cluster observes: [`HEALTHY`] while the cluster is available
/// for queries, so the query the check runs succeeds, and [`UNHEALTHY`] while it is not.
///
/// Read off the cluster's own `ClusterAvailabilityStatus` rule
/// ([`super::availability_status`]) — "Available - The cluster is available for queries" — so
/// the series and `DescribeClusters` cannot disagree about one cluster.
fn observed_health(stored: &StoredCluster) -> f64 {
    let available = stored
        .cluster
        .cluster_status
        .as_deref()
        .is_some_and(|status| availability_status(status) == super::AVAILABILITY_STATUS_AVAILABLE);
    if available {
        HEALTHY
    } else {
        UNHEALTHY
    }
}

/// Open a series for every cluster this store health-checks, and close the series of every
/// cluster it no longer holds.
///
/// A series opens with its run-up already behind it ([`RUN_UP_PERIODS`]) and its first
/// datapoint due at once, so the pass that finds the cluster available publishes the series'
/// first datapoints itself. A cluster whose series already exists keeps it, so a cluster that
/// goes through a change — and reports [`UNHEALTHY`] while it does — keeps ONE series across
/// it rather than restarting its run-up.
fn sync_series(store: &mut RedshiftState, now: SystemTime, instant: Instant) {
    let held: HashSet<String> = store
        .clusters
        .iter()
        .filter_map(|stored| stored.cluster.cluster_identifier.clone())
        .collect();
    store
        .health_metrics
        .series
        .retain(|identifier, _| held.contains(identifier));
    let run_up = PUBLISH_INTERVAL.saturating_mul(RUN_UP_PERIODS);
    let opening: Vec<String> = store
        .clusters
        .iter()
        .filter(|stored| is_checked(stored))
        .filter_map(|stored| stored.cluster.cluster_identifier.clone())
        .filter(|identifier| !store.health_metrics.series.contains_key(identifier))
        .collect();
    for identifier in opening {
        store.health_metrics.series.insert(
            identifier,
            SeriesClock {
                next_stamp: now.checked_sub(run_up).unwrap_or(now),
                next_due_at: instant,
            },
        );
    }
}

/// Take the datapoints every series that is due at `instant` is published as, and advance each
/// of those series to its next datapoint.
///
/// One observation per due series, published at BOTH the dimension sets it is reported at: the
/// cluster-wide one and one per node of the cluster's own topology.
///
/// The whole minute of ONE cluster is taken together or not at all: its cluster-wide datapoint
/// and its nodes' carry the same stamp, so a pass with no room for all of them leaves that
/// cluster's clock where it is — the minute is still due, and the next wake-up publishes it
/// whole, rather than the nodes it had no room for losing it ([`MAX_DATAPOINTS_PER_PASS`]).
fn due_datapoints(
    store: &mut RedshiftState,
    instant: Instant,
) -> Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> {
    let due: Vec<String> = store
        .health_metrics
        .series
        .iter()
        .filter(|(_, series)| instant >= series.next_due_at)
        .map(|(identifier, _)| identifier.clone())
        .collect();
    let mut published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> = Vec::new();
    for identifier in due {
        let Some(stored) = store
            .clusters
            .iter()
            .find(|stored| stored.cluster.cluster_identifier.as_deref() == Some(&identifier))
        else {
            continue;
        };
        let health = observed_health(stored);
        // The nodes the cluster itself reports, which is the topology the per-node series
        // belong to: one series per node, under the role that node is in.
        let nodes: Vec<String> = stored
            .cluster
            .cluster_nodes
            .iter()
            .flatten()
            .filter_map(|node| node.node_role.as_deref().map(node_id))
            .collect();
        // Room for this cluster's WHOLE minute, taken before its clock moves: a pass that
        // cannot finish this cluster publishes none of it and leaves the minute due.
        if published.len() + 1 + nodes.len() > MAX_DATAPOINTS_PER_PASS {
            return published;
        }
        let Some(series) = store.health_metrics.series.get_mut(&identifier) else {
            continue;
        };
        let stamp = advance(series, instant);
        let datum = |stamp: SystemTime| cloudwatch::StoredDatum {
            timestamp_secs: epoch_secs(stamp),
            // One observation: this check's own verdict on the cluster.
            sample_count: 1.0,
            sum: health,
            minimum: health,
            maximum: health,
            // The health check reports a 0/1 flag, which the `StandardUnit` vocabulary has no
            // member for, so the datapoint declares no unit — and the metric store reports a
            // datapoint published without one under the value the enumerated type has for a
            // measurement that has none.
            unit: None,
            storage_resolution: STORAGE_RESOLUTION_SECS,
        };
        published.push((cluster_series(&identifier), datum(stamp)));
        for node in nodes {
            published.push((node_series(&identifier, &node), datum(stamp)));
        }
    }
    published
}

/// Take the time stamp of the datapoint that is due and schedule the next one, `instant` being
/// what the store's clock read when this pass started.
fn advance(series: &mut SeriesClock, instant: Instant) -> SystemTime {
    let stamp = series.next_stamp;
    series.next_stamp = stamp.checked_add(PUBLISH_INTERVAL).unwrap_or(stamp);
    // While the series is still behind the present it is published at the accelerated spacing;
    // once it has caught up, the next datapoint is due when the clock reaches its stamp, which
    // is one publish interval apart. Never longer than one interval: that is the most the
    // schedule can owe, so a clock that has stepped backwards costs one slow publish rather
    // than an unbounded wait.
    let wait = series
        .next_stamp
        .duration_since(SystemTime::now())
        .unwrap_or(ACCELERATED_SPACING)
        .min(PUBLISH_INTERVAL);
    series.next_due_at = instant.checked_add(wait).unwrap_or(instant);
    stamp
}

/// Append the datapoints this pass published to the metric store, and make every alarm over
/// the series they belong to due for evaluation.
///
/// Nothing is published on an engine that holds no metric store to reach: a datapoint has one
/// home, and this path never makes a second one. An alarm is evaluated FROM these datapoints,
/// so storing a series it watches is what makes its evaluation due — an alarm cannot tell a
/// service's own metric from one a client published.
fn append_datapoints<Eng: Send + Sync + 'static>(
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
        let unit = datum.unit().to_string();
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The cluster itself is up either way, so
            // nothing is failed for it; the dropped observation is reported where a developer
            // can see it.
            ::tracing::warn!(
                metric = HEALTH_STATUS_METRIC,
                "the CloudWatch metric store is at capacity, so this health observation was dropped"
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

/// `at` as the epoch seconds a stored datapoint carries, which is the unit the metric store's
/// own readers window by. An instant before the epoch (which no clock reports) reads as the
/// epoch itself rather than failing the publish.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}
