// SPDX-License-Identifier: CC0-1.0

//! The CAPACITY an Aurora Serverless v2 DB cluster is running on: the Aurora capacity units
//! (ACUs) it holds, the AUTO-PAUSE the clock owes one that has been idle, and the CloudWatch
//! series both are reported as.
//!
//! An Aurora Serverless v2 cluster has no instance class to read its compute off: its capacity
//! is a number that moves while the cluster runs, between the `MinCapacity` and `MaxCapacity`
//! its `ServerlessV2ScalingConfiguration` names. No API operation of this service reports that
//! number, so the ONE place a client can watch it is the [`SERVERLESS_DATABASE_CAPACITY_METRIC`]
//! metric of the [`METRIC_NAMESPACE`] namespace: a dashboard, an alarm or a client that waits
//! for a cluster to scale down reads that series, and against a cluster that publishes nothing
//! it cannot tell a running cluster from a paused one — or from one that does not exist.
//!
//! The observation is published at the two dimension sets the series is reported at, which are
//! independent series because a metric's identity includes its dimension set:
//!
//! * the cluster, dimensioned by [`DB_CLUSTER_IDENTIFIER_DIMENSION`] alone; and
//! * each of its Serverless v2 MEMBER instances ([`super::SERVERLESS_INSTANCE_CLASS`]),
//!   dimensioned by [`DB_INSTANCE_IDENTIFIER_DIMENSION`] — the capacity member's own documented
//!   subject is "a DB instance in an Aurora Serverless v2 cluster", so a cluster's members each
//!   publish the capacity they hold.
//!
//! # What one observation reads
//!
//! `MinCapacity` is "the minimum number of Aurora capacity units (ACUs) for a DB instance in an
//! Aurora Serverless v2 cluster … For Aurora versions that support the Aurora Serverless v2
//! auto-pause feature, the smallest value that you can use is 0. For versions that don't
//! support Aurora Serverless v2 auto-pause, the smallest value that you can use is 0.5", and
//! `SecondsUntilAutoPause` is "the number of seconds an Aurora Serverless v2 DB instance must be
//! idle before Aurora attempts to automatically pause it … This property is only shown when the
//! minimum capacity for the cluster is set to 0 ACUs". So:
//!
//! * a cluster that is RUNNING holds its configured minimum, and never less than the smallest
//!   capacity a running Serverless v2 instance has ([`MIN_RUNNING_CAPACITY`]) — a cluster
//!   configured down to 0 ACUs still runs on half an ACU until it pauses;
//! * a cluster whose minimum is 0 and that has been IDLE for its `SecondsUntilAutoPause` is
//!   PAUSED and holds [`PAUSED_CAPACITY`] — nothing is running, which is what scale-to-zero
//!   means;
//! * a cluster with an open session is not idle, so it cannot pause however long it has been
//!   connected for, and the next session on a paused cluster resumes it.
//!
//! Idleness is read from the SESSIONS the cluster's own engine endpoints and its members' hold
//! ([`super::data_plane::pg_sessions::SessionRegistry`]) — the connections this emulator is
//! really serving — so neither the pause nor the resume is a function of who called the control
//! plane. The data a paused cluster holds is untouched by the pause: the engine's databases stay
//! exactly as they were, which is what makes a resumed cluster still hold every row committed
//! before it paused.
//!
//! # The clock the series is published on
//!
//! The metric is published once a MINUTE ([`PUBLISH_INTERVAL`]) on the schedule and the one
//! bounded in-process clock every series of this namespace shares ([`super::metric_publisher`]),
//! which is also where a datapoint is appended to the metric store CloudWatch itself serves. What
//! makes that observable without any further call is [`drive_serverless_capacity`]: the writes
//! that bring a Serverless v2 cluster into being publish whatever is already due and start that
//! clock. Nothing here is on a READ path, and nothing it publishes depends on who called: every
//! datapoint it materialises is one the stored due instants had already made due, and so is the
//! pause, which is owed at the instant the idle period runs out whether or not anybody asks.
//!
//! Every instant here is the EMULATOR's clock ([`::service_common::clock`]), which is the
//! process clock unless an operator stops it: a pause that is due only after an idle period has
//! passed is due when that clock says so.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::metric_publisher::{self, PublisherClock, SeriesClock, MAX_DATAPOINTS_PER_PASS};
use super::{ComAmazonawsRdsAmazonrdsv19State as RdsState, RegionalKey};

pub use super::metric_publisher::{
    DB_CLUSTER_IDENTIFIER_DIMENSION, DB_INSTANCE_IDENTIFIER_DIMENSION, METRIC_NAMESPACE,
    PUBLISH_INTERVAL,
};

/// The metric reporting the current capacity of an Aurora Serverless v2 DB cluster, in Aurora
/// capacity units.
pub const SERVERLESS_DATABASE_CAPACITY_METRIC: &str = "ServerlessDatabaseCapacity";

/// What the capacity reads while the cluster is paused: no capacity at all, which is what
/// scaling to zero leaves running.
pub const PAUSED_CAPACITY: f64 = 0.0;

/// The smallest capacity a RUNNING Aurora Serverless v2 instance holds, in ACUs: half an ACU,
/// the smallest value `MinCapacity` takes for a version without the auto-pause feature ("For
/// versions that don't support Aurora Serverless v2 auto-pause, the smallest value that you can
/// use is 0.5"). A cluster configured down to 0 ACUs runs on this until it pauses — 0 ACUs is
/// the PAUSED state, not a running capacity.
pub const MIN_RUNNING_CAPACITY: f64 = 0.5;

/// The cluster-wide series of the DB cluster `identifier`.
#[must_use]
pub fn cluster_series(identifier: &str) -> cloudwatch::MetricKey {
    series(DB_CLUSTER_IDENTIFIER_DIMENSION, identifier)
}

/// The series of ONE Serverless v2 member instance of such a cluster.
#[must_use]
pub fn instance_series(identifier: &str) -> cloudwatch::MetricKey {
    series(DB_INSTANCE_IDENTIFIER_DIMENSION, identifier)
}

/// The one place a published series is addressed, so the publish and every reader that looks for
/// what it stored name the same series.
fn series(dimension: &str, value: &str) -> cloudwatch::MetricKey {
    metric_publisher::series(SERVERLESS_DATABASE_CAPACITY_METRIC, dimension, value)
}

/// How long one cluster has been idle, and whether that has paused it.
#[derive(Debug)]
struct IdleClock {
    /// When the cluster last had no session open at all, or `None` while one is open.
    idle_since: Option<Instant>,
    /// Whether the cluster is paused now.
    paused: bool,
}

/// Where every capacity series of this store has got to, how long each of its clusters has been
/// idle, and the one in-process clock that advances them.
///
/// Held on the store so the schedule is the store's own: a series exists while the cluster it
/// belongs to does, and the clock is claimed once however many writes drive it.
#[derive(Debug, Default)]
pub struct ServerlessCapacityPublisher {
    /// The schedule of each Serverless v2 cluster being published, by its key in the store.
    series: HashMap<RegionalKey, SeriesClock>,
    /// How long each of those clusters has been idle, by the same key.
    idle: HashMap<RegionalKey, IdleClock>,
    /// The one in-process clock that advances this publisher's series.
    clock: PublisherClock,
}

impl ServerlessCapacityPublisher {
    /// The capacity the cluster `key` is reported at now, or `None` for a cluster this store
    /// publishes no capacity series for.
    ///
    /// For a reader inside this crate — a test, or an operation that reports the capacity a
    /// cluster holds — so there is one answer rather than a second reading of the same state.
    #[must_use]
    pub fn reported_capacity(&self, key: &RegionalKey, configured_minimum: f64) -> Option<f64> {
        let idle = self.idle.get(key)?;
        Some(match idle.paused {
            true => PAUSED_CAPACITY,
            false => configured_minimum.max(MIN_RUNNING_CAPACITY),
        })
    }
}

/// Publish every capacity datapoint the clock has already made due, and start the one bounded
/// in-process task that waits for the next one.
///
/// Called by the writes that bring a Serverless v2 cluster into being or change its scaling
/// configuration — never by a read: the datapoints this materialises are the ones the stored due
/// instants have already made due, so what a caller sees does not depend on whether anybody
/// called.
///
/// With no async runtime to spawn onto there is no clock, and what is due at the moment of the
/// write is then the whole series until the next write — the router always runs on one.
pub fn drive_serverless_capacity<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    metric_publisher::drive(
        engine,
        |store| &mut store.serverless_capacity.clock,
        publish_due_capacity::<Eng>,
        next_capacity_due_at,
    );
}

/// When this publisher next owes a datapoint, as the emulator's clock stands at `now`, or `None`
/// when the store holds no Serverless v2 cluster at all and its clock may stand down.
fn next_capacity_due_at(store: &RdsState, now: Instant) -> Option<Instant> {
    let due = store
        .serverless_capacity
        .series
        .values()
        .map(SeriesClock::next_due_at)
        .min();
    // A cluster still being brought up publishes nothing yet, and its first datapoint is owed the
    // moment it becomes available — which the instant the store holds for it already fixes. Only
    // an instant still AHEAD of the clock is one: a cluster that is not publishing and whose
    // bring-up is long past is one no transition is going to make available, so it owes nothing
    // and must not hold the clock at its shortest wait.
    let coming_up = serverless_clusters(store)
        .into_iter()
        .filter(|cluster| !cluster.publishing && cluster.ready_at > now)
        .map(|cluster| cluster.ready_at)
        .min();
    [due, coming_up].into_iter().flatten().min()
}

/// What one pass needs to know about one Serverless v2 cluster, read off the store before
/// anything is written: no observation depends on the order the pass walks the store in.
#[derive(Debug)]
struct ClusterCapacity {
    /// The cluster's key in the store.
    key: RegionalKey,
    /// Whether this cluster is far enough along to publish a series at all.
    publishing: bool,
    /// When the cluster becomes available, for one that is not publishing yet.
    ready_at: Instant,
    /// `MinCapacity`, the capacity the cluster holds while it runs.
    min_capacity: f64,
    /// `SecondsUntilAutoPause`, the idle period that pauses a cluster whose minimum is 0.
    seconds_until_auto_pause: Duration,
    /// Whether the cluster can pause at all: "This property is only shown when the minimum
    /// capacity for the cluster is set to 0 ACUs."
    can_pause: bool,
    /// Whether any session is open on the cluster's own endpoints or on one of its members'.
    connected: bool,
    /// The `DBInstanceIdentifier` of each Serverless v2 member, which publishes a series of its
    /// own.
    serverless_members: Vec<String>,
}

/// Every Serverless v2 DB cluster this store holds, with what one capacity observation of it
/// needs.
///
/// Walks the clusters of each region this store keeps records for, which the region's own cluster
/// quota bounds, and reads nothing but the records and the live session registries.
fn serverless_clusters(store: &RdsState) -> Vec<ClusterCapacity> {
    let mut clusters: Vec<ClusterCapacity> = Vec::new();
    for region in metric_publisher::regions(store) {
        for cluster in store.clusters_in(&region) {
            let Some(scaling) = &cluster.serverless_v2_scaling else {
                continue;
            };
            let key: RegionalKey = (cluster.region.clone(), cluster.identifier.clone());
            let min_capacity = scaling.min_capacity.unwrap_or(MIN_RUNNING_CAPACITY);
            let members = store.cluster_members(cluster);
            // Every session this emulator is serving for the cluster: the ones on the cluster's
            // own writer and reader endpoints, and the ones on each member instance's own
            // endpoint. Any of them is a connection the cluster is serving, so it is not idle —
            // counted through the one traversal every reader of that number shares
            // ([`super::instance_connections`]).
            let connected = super::instance_connections::cluster_open_sessions(store, cluster) > 0;
            clusters.push(ClusterCapacity {
                publishing: cluster.status == super::DB_CLUSTER_STATUS_AVAILABLE,
                ready_at: cluster.ready_at,
                min_capacity,
                seconds_until_auto_pause: Duration::from_secs(
                    u64::try_from(scaling.seconds_until_auto_pause.unwrap_or(
                        *super::SECONDS_UNTIL_AUTO_PAUSE_RANGE.start(),
                    ))
                    .unwrap_or_default(),
                ),
                // Auto-pause is the feature a cluster asks for by naming a minimum of 0 ACUs.
                can_pause: min_capacity <= PAUSED_CAPACITY,
                connected,
                serverless_members: members
                    .iter()
                    .filter(|member| {
                        member.db_instance_class == super::SERVERLESS_INSTANCE_CLASS
                    })
                    .map(|member| member.identifier.clone())
                    .collect(),
                key,
            });
        }
    }
    clusters
}

/// Publish the datapoint every capacity series that is due is reported as.
///
/// Three steps, so no store guard is held over another service's lock and the two stores are
/// never locked at once:
///
/// 1. under THIS service's guard, materialise the transitions the clock owes, so the statuses the
///    pass reads are the ones that are already due;
/// 2. still under that guard, move each cluster's idle clock on and take the datapoints the due
///    series are published as;
/// 3. with the metric store's guard, append them and make the alarms over them due.
fn publish_due_capacity<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    // The pass is dated by the EMULATOR's clock, like every transition it materialises: a clock
    // that stands still owes no datapoint and no pause yet.
    let instant = ::service_common::clock::now(engine);
    let published = {
        let mut store = state.lock();
        for region in metric_publisher::regions(&store) {
            store.settle_due_transitions(&region);
        }
        let clusters = serverless_clusters(&store);
        settle_idle_clocks(&mut store, &clusters, instant);
        sync_series(&mut store, &clusters, now, instant);
        due_datapoints(&mut store, &clusters, instant)
    };
    metric_publisher::append(engine, published, now);
}

/// Move each cluster's idle clock on, and close the clocks of the clusters this store no longer
/// holds.
///
/// A cluster with a session open is not idle, so its idle clock is cleared and a paused cluster
/// RESUMES — which is what the next login does to a paused Serverless v2 cluster. A cluster with
/// none has been idle since the first pass that found it so, and it is PAUSED once that has been
/// longer than its `SecondsUntilAutoPause`: a change the clock made due, which nobody has to ask
/// for.
fn settle_idle_clocks(store: &mut RdsState, clusters: &[ClusterCapacity], instant: Instant) {
    let held: HashSet<RegionalKey> = clusters
        .iter()
        .map(|cluster| cluster.key.clone())
        .collect();
    store
        .serverless_capacity
        .idle
        .retain(|key, _| held.contains(key));
    for cluster in clusters {
        let clock = store
            .serverless_capacity
            .idle
            .entry(cluster.key.clone())
            .or_insert(IdleClock {
                idle_since: Some(instant),
                paused: false,
            });
        if cluster.connected {
            clock.idle_since = None;
            clock.paused = false;
            continue;
        }
        let since = *clock.idle_since.get_or_insert(instant);
        clock.paused = cluster.can_pause
            && instant.saturating_duration_since(since) >= cluster.seconds_until_auto_pause;
    }
}

/// Open a series for every Serverless v2 cluster that is publishing one, and close the series of
/// every cluster this store no longer holds or no longer publishes for.
///
/// A series opens with its run-up already behind it and its first datapoint due at once
/// ([`SeriesClock::opening`]), so the pass that finds the cluster available publishes the series'
/// first datapoints itself. A cluster whose series already exists keeps it, so one that pauses
/// and resumes keeps ONE series across both rather than restarting its run-up.
fn sync_series(
    store: &mut RdsState,
    clusters: &[ClusterCapacity],
    now: SystemTime,
    instant: Instant,
) {
    let publishing: HashSet<RegionalKey> = clusters
        .iter()
        .filter(|cluster| cluster.publishing)
        .map(|cluster| cluster.key.clone())
        .collect();
    store
        .serverless_capacity
        .series
        .retain(|key, _| publishing.contains(key));
    for key in publishing {
        store
            .serverless_capacity
            .series
            .entry(key)
            .or_insert_with(|| SeriesClock::opening(now, instant));
    }
}

/// Take the datapoints every series that is due at `instant` is published as, and advance each of
/// those series to its next datapoint.
///
/// One observation per due cluster, published at BOTH the dimension sets it is reported at: the
/// cluster's own and one per Serverless v2 member. The whole minute of ONE cluster is taken
/// together or not at all, so a pass with no room for all of it leaves that cluster's clock where
/// it is and the next wake-up publishes the minute whole ([`MAX_DATAPOINTS_PER_PASS`]).
fn due_datapoints(
    store: &mut RdsState,
    clusters: &[ClusterCapacity],
    instant: Instant,
) -> Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> {
    let mut published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> = Vec::new();
    for cluster in clusters {
        let due = store
            .serverless_capacity
            .series
            .get(&cluster.key)
            .is_some_and(|series| series.is_due(instant));
        if !due {
            continue;
        }
        let capacity = store
            .serverless_capacity
            .reported_capacity(&cluster.key, cluster.min_capacity)
            .unwrap_or_else(|| cluster.min_capacity.max(MIN_RUNNING_CAPACITY));
        // Room for this cluster's WHOLE minute, taken before its clock moves.
        if published.len() + 1 + cluster.serverless_members.len() > MAX_DATAPOINTS_PER_PASS {
            return published;
        }
        let Some(series) = store.serverless_capacity.series.get_mut(&cluster.key) else {
            continue;
        };
        let stamp = series.advance(instant);
        // One observation: the capacity the cluster holds at this minute. An Aurora capacity unit
        // is no member of the `StandardUnit` vocabulary, so the datapoint declares no unit.
        let datum = || metric_publisher::observation(stamp, capacity, None);
        published.push((cluster_series(&cluster.key.1), datum()));
        for member in &cluster.serverless_members {
            published.push((instance_series(member), datum()));
        }
    }
    published
}

