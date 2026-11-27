// SPDX-License-Identifier: CC0-1.0

//! The instance-level `AWS/RDS` series a DB instance publishes while it is serving: the
//! CONNECTIONS it is holding, reported where a dashboard, an alarm or a client watching load
//! reads them.
//!
//! No API operation of this service reports how many client connections a DB instance is serving,
//! so the ONE place a client can watch that number is the
//! [`DATABASE_CONNECTIONS_METRIC`] metric of the [`METRIC_NAMESPACE`] namespace, dimensioned by
//! the instance's [`DB_INSTANCE_IDENTIFIER_DIMENSION`] — "the number of client network
//! connections to the database instance", which Amazon RDS publishes for every DB instance it is
//! running. Against an instance that publishes nothing an alarm over that series never leaves
//! `INSUFFICIENT_DATA`, so no alarm action of it is ever fired and a client keyed on connection
//! load — autoscaling, paging, a runbook — silently never triggers.
//!
//! # What one observation reads
//!
//! The SESSIONS this emulator is really serving for the instance: the ones open on its own engine
//! endpoint ([`super::data_plane::pg_sessions::SessionRegistry`]), counted at the instant the
//! datapoint is stamped. So the series is a function of the connections the emulator holds, never
//! of who called the control plane — a session opened against the instance raises the next
//! datapoint, and one that closes lowers it. An instance this emulator serves no engine wire for
//! holds no session at all, and its series reads zero: the count of connections it is serving,
//! which is what a reader of the series is asking for.
//!
//! Only an instance that is AVAILABLE ([`super::DB_INSTANCE_STATUS_AVAILABLE`]) publishes: one
//! still being created, one stopped and one on its way out is not running a database to hold
//! connections, and the real series has no datapoint for the span it was not running in.
//!
//! The schedule, the stamping and the one bounded in-process clock are the ones every series of
//! this namespace shares ([`super::metric_publisher`]): a datapoint a minute, which is the finest
//! period a regular-resolution metric is read and alarmed at, published into the CloudWatch
//! emulator's own metric store. [`drive_instance_metrics`] is what makes that observable without
//! any further call: the writes that bring a DB instance into being publish whatever is already
//! due and start that clock. Nothing here is on a READ path — every datapoint it materialises is
//! one the stored due instants had already made due.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::metric_publisher::{self, PublisherClock, SeriesClock, MAX_DATAPOINTS_PER_PASS};
use super::{ComAmazonawsRdsAmazonrdsv19State as RdsState, RegionalKey};

pub use super::metric_publisher::{
    DB_INSTANCE_IDENTIFIER_DIMENSION, METRIC_NAMESPACE, PUBLISH_INTERVAL,
};

/// The metric reporting how many client connections a DB instance is serving: "the number of
/// client network connections to the database instance".
pub const DATABASE_CONNECTIONS_METRIC: &str = "DatabaseConnections";

/// The `StandardUnit` a connection count is reported in: a plain number of connections, which is
/// `Count` in that vocabulary (`smithy_query(query="shape",shape="StandardUnit")`).
pub const DATABASE_CONNECTIONS_UNIT: &str = "Count";

/// The connections series of the DB instance `identifier`.
#[must_use]
pub fn database_connections_series(identifier: &str) -> cloudwatch::MetricKey {
    metric_publisher::series(
        DATABASE_CONNECTIONS_METRIC,
        DB_INSTANCE_IDENTIFIER_DIMENSION,
        identifier,
    )
}

/// Where every instance-level series of this store has got to, and the one in-process clock that
/// advances them.
///
/// Held on the store so the schedule is the store's own: a series exists while the instance it
/// belongs to does, and the clock is claimed once however many writes drive it.
#[derive(Debug, Default)]
pub struct InstanceMetricPublisher {
    /// The schedule of each DB instance being published, by its region and `DbiResourceId` — the
    /// instance's immutable id, so a rename cannot orphan a series or give an instance a second
    /// one.
    series: HashMap<RegionalKey, SeriesClock>,
    /// The one in-process clock that advances this publisher's series.
    clock: PublisherClock,
}

/// Publish every instance-level datapoint the clock has already made due, and start the one
/// bounded in-process task that waits for the next one.
///
/// Called by the writes that bring a DB instance into being or back into service — never by a
/// read: the datapoints this materialises are the ones the stored due instants have already made
/// due, so what a caller sees does not depend on whether anybody called.
///
/// With no async runtime to spawn onto there is no clock, and what is due at the moment of the
/// write is then the whole series until the next write — the router always runs on one.
pub fn drive_instance_metrics<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    metric_publisher::drive(
        engine,
        |store| &mut store.instance_metrics.clock,
        publish_due_instance_metrics::<Eng>,
        next_instance_metric_due_at,
    );
}

/// When this publisher next owes a datapoint, as the emulator's clock stands at `now`, or `None`
/// when the store holds no DB instance at all and its clock may stand down.
fn next_instance_metric_due_at(store: &RdsState, now: Instant) -> Option<Instant> {
    let due = store
        .instance_metrics
        .series
        .values()
        .map(SeriesClock::next_due_at)
        .min();
    // An instance still being brought up publishes nothing yet, and its first datapoint is owed
    // the moment it becomes available — which the instant the store holds for it already fixes.
    // Only an instant still AHEAD of the clock is one: an instance that is not publishing and
    // whose bring-up is long past is one no transition is going to make available, so it owes
    // nothing and must not hold the clock at its shortest wait.
    let coming_up = instance_connections(store)
        .into_iter()
        .filter(|instance| !instance.publishing && instance.ready_at > now)
        .map(|instance| instance.ready_at)
        .min();
    [due, coming_up].into_iter().flatten().min()
}

/// What one pass needs to know about one DB instance, read off the store before anything is
/// written: no observation depends on the order the pass walks the store in.
#[derive(Debug)]
struct InstanceConnections {
    /// The instance's series key: its region and its immutable `DbiResourceId`.
    key: RegionalKey,
    /// The `DBInstanceIdentifier` the observation is dimensioned by, which is the identifier the
    /// instance is published under NOW — a renamed instance publishes under its new name.
    identifier: String,
    /// Whether this instance is running a database, and so publishing a series at all.
    publishing: bool,
    /// When the instance becomes available, for one that is not publishing yet.
    ready_at: Instant,
    /// How many client sessions it is serving.
    connections: f64,
}

/// Every DB instance this store holds, with what one connection observation of it needs.
///
/// Walks the instances of each region this store keeps records for, which the region's own
/// instance quota bounds, and reads nothing but the records and the live session registries.
fn instance_connections(store: &RdsState) -> Vec<InstanceConnections> {
    let mut instances: Vec<InstanceConnections> = Vec::new();
    for region in metric_publisher::regions(store) {
        for instance in store.instances_in(&region) {
            let open = store
                .engine_endpoint(&(instance.region.clone(), instance.identifier.clone()))
                .map_or(0, |endpoint| endpoint.sessions().open_count());
            instances.push(InstanceConnections {
                key: (region.clone(), instance.dbi_resource_id.clone()),
                identifier: instance.identifier.clone(),
                publishing: instance.status == super::DB_INSTANCE_STATUS_AVAILABLE,
                ready_at: instance.ready_at,
                #[allow(clippy::cast_precision_loss)]
                connections: open as f64,
            });
        }
    }
    instances
}

/// Publish the datapoint every instance-level series that is due is reported as.
///
/// Three steps, so no store guard is held over another service's lock and the two stores are
/// never locked at once:
///
/// 1. under THIS service's guard, materialise the transitions the clock owes, so the statuses the
///    pass reads are the ones that are already due;
/// 2. still under that guard, take the datapoints the due series are published as;
/// 3. with the metric store's guard, append them and make the alarms over them due.
fn publish_due_instance_metrics<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    // The pass is dated by the EMULATOR's clock, like every transition it materialises: a clock
    // that stands still owes no datapoint yet.
    let instant = ::service_common::clock::now(engine);
    let published = {
        let mut store = state.lock();
        for region in metric_publisher::regions(&store) {
            store.settle_due_transitions(&region);
        }
        let instances = instance_connections(&store);
        sync_series(&mut store, &instances, now, instant);
        due_datapoints(&mut store, &instances, instant)
    };
    metric_publisher::append(engine, published, now);
}

/// Open a series for every DB instance that is publishing one, and close the series of every
/// instance this store no longer holds or no longer publishes for.
///
/// A series opens with its run-up already behind it and its first datapoint due at once
/// ([`SeriesClock::opening`]), so the pass that finds the instance available publishes the series'
/// first datapoints itself. An instance whose series already exists keeps it, so one that is
/// stopped and started again keeps ONE series across both rather than restarting its run-up.
fn sync_series(
    store: &mut RdsState,
    instances: &[InstanceConnections],
    now: SystemTime,
    instant: Instant,
) {
    let publishing: HashSet<RegionalKey> = instances
        .iter()
        .filter(|instance| instance.publishing)
        .map(|instance| instance.key.clone())
        .collect();
    store
        .instance_metrics
        .series
        .retain(|key, _| publishing.contains(key));
    for key in publishing {
        store
            .instance_metrics
            .series
            .entry(key)
            .or_insert_with(|| SeriesClock::opening(now, instant));
    }
}

/// Take the datapoints every series that is due at `instant` is published as, and advance each of
/// those series to its next datapoint.
///
/// One observation per due instance. A pass with no room left leaves the clocks of the instances
/// it did not reach where they are, so the next wake-up publishes their minutes intact
/// ([`MAX_DATAPOINTS_PER_PASS`]).
fn due_datapoints(
    store: &mut RdsState,
    instances: &[InstanceConnections],
    instant: Instant,
) -> Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> {
    let mut published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> = Vec::new();
    for instance in instances {
        if published.len() >= MAX_DATAPOINTS_PER_PASS {
            return published;
        }
        let Some(series) = store
            .instance_metrics
            .series
            .get_mut(&instance.key)
            .filter(|series| series.is_due(instant))
        else {
            continue;
        };
        let stamp = series.advance(instant);
        published.push((
            database_connections_series(&instance.identifier),
            metric_publisher::observation(
                stamp,
                instance.connections,
                Some(DATABASE_CONNECTIONS_UNIT.to_string()),
            ),
        ));
    }
    published
}
