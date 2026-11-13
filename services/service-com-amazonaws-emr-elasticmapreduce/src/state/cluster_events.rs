// SPDX-License-Identifier: CC0-1.0

//! Announcing a cluster's state changes on the account's default event bus.
//!
//! One concern, split out of [`super`]: the cluster lifecycle itself is the
//! record's ([`ClusterRecord::advance_to`]), and this module is what makes each
//! state the cluster ENTERS observable to a client that is not calling Amazon EMR
//! at all. Amazon EMR announces a cluster's lifecycle on the account's default
//! event bus — the events reference publishes `source` [`EVENT_SOURCE`] with
//! `detail-type` [`CLUSTER_STATE_CHANGE_DETAIL_TYPE`], whose `detail` names the
//! cluster (`clusterId`, `name`), the `state` it has just entered, that state's
//! `stateChangeReason` and the `severity` of the transition — so an EventBridge
//! rule whose pattern matches those fields delivers to its own targets.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **what moved** — one comparison of every cluster's reported state against
//!    the state last announced for it
//!    ([`ComAmazonawsEmrElasticmapreduceState::record_cluster_state_changes`]), so
//!    no writer can forget to announce a transition and none is announced twice:
//!    a transition becomes an event HERE, whoever materialised it.
//! 2. **when it moved** — the instant the cluster's own timeline stamps on that
//!    state ([`state_entered_at`]), not the instant somebody looked, so the
//!    event's `time` is a fact of the cluster.
//! 3. **the publication** — the tree's one in-process EventBridge client
//!    ([`::service_common::eventbridge_client`]) driven over the operation
//!    callback, so nothing here opens a socket or leaves this process. The
//!    `source`, the bus, the severity vocabulary, the `PutEvents` entry bound and
//!    the event's `time` are the SIBLING publisher's
//!    ([`super::step_events`], which publishes the states a STEP moves into):
//!    this module adds the cluster's own `detail-type` and `detail` and nothing
//!    else, so the two event classes this service publishes cannot disagree about
//!    the envelope they share.
//! 4. **the clock** — one bounded pump ([`drive_cluster_clock`]) that waits for
//!    the next instant this store owes a cluster transition, materialises what is
//!    then due and publishes it, so a client that only listens on the bus still
//!    sees the cluster finish. It does not change WHEN a transition happens: it
//!    applies only what the stored due instant has already made due, which is
//!    what every reporting operation applies too.
//!
//! The store's guard is never held across the publication: the queue is drained
//! under it and the `PutEvents` runs with it released.

use std::time::{Duration, SystemTime};

use super::*;

/// The `detail-type` of the event published when a CLUSTER changes state.
///
/// The `source` every event of this service carries, the bus it goes onto, the
/// severity vocabulary and the `PutEvents` entry bound are the sibling
/// publisher's ([`super::step_events::EVENT_SOURCE`],
/// [`super::step_events::DEFAULT_EVENT_BUS`], [`super::step_events::SEVERITY_INFO`],
/// [`super::step_events::SEVERITY_ERROR`]).
pub const CLUSTER_STATE_CHANGE_DETAIL_TYPE: &str = "EMR Cluster State Change";

/// How many queued changes one drain publishes, so no single pass is unbounded.
const MAX_PUBLISHED_PER_DRAIN: usize = 64;

/// How many changes may wait for publication before the oldest is dropped: a
/// backstop on this process's memory, far above the handful of transitions the
/// bounded cluster map can owe at once.
const MAX_PENDING_EVENTS: usize = 1_024;

/// The shortest one wake-up of the clock pump waits, so an instant already past
/// cannot cost a CPU-bound spin.
const MIN_CLOCK_WAIT: Duration = Duration::from_millis(5);

/// The longest one wake-up waits before it looks at the store again: a backstop,
/// so a due instant far in the future cannot park the pump indefinitely.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down. A backstop, not a
/// budget: a cluster owes a handful of transitions and the cluster map is bounded
/// ([`MAX_CLUSTERS`]), so a pump normally stands down for want of work.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// One cluster state change waiting to be published, rendered against the record
/// as it stood when the change was observed.
#[derive(Debug, Clone)]
pub struct ClusterStateChange {
    /// The cluster that moved.
    pub cluster_id: String,
    /// WHEN it moved — the instant the cluster's own timeline stamps on the state
    /// it moved into, not the instant some later call materialised or published
    /// it.
    pub at: SystemTime,
    /// The rendered `detail` of the event.
    pub detail: ::serde_json::Value,
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// Queue a state-change event for every cluster whose reported state differs
    /// from the one last announced for it, as of `now`.
    ///
    /// The comparison is against what was ANNOUNCED rather than against who wrote
    /// the state, so a transition the clock materialised on a read is announced
    /// exactly like one an operation wrote, and announcing it twice is impossible.
    /// A cluster this store no longer holds cannot move again, so its entry goes
    /// with it. The changes are queued oldest transition first.
    ///
    /// The state last announced is held HERE rather than on the record (where the
    /// sibling publisher keeps a step's, [`StepRecord::published_state`]) because
    /// the clock pump compares the whole cluster map at once rather than the
    /// queue of one named cluster: one map walk answers for every cluster this
    /// store holds, and an entry is dropped with the cluster it is about.
    pub fn record_cluster_state_changes(&mut self, now: SystemTime) {
        let clusters = &self.clusters;
        self.announced_cluster_states
            .retain(|cluster_id, _| clusters.contains_key(cluster_id));
        let mut observed: Vec<(String, ClusterStateChange)> = Vec::new();
        for cluster in self.clusters.values() {
            if self.announced_cluster_states.get(&cluster.id) == Some(&cluster.status.state) {
                continue;
            }
            let at = state_entered_at(cluster).unwrap_or(now);
            observed.push((
                cluster.id.clone(),
                ClusterStateChange {
                    cluster_id: cluster.id.clone(),
                    at,
                    detail: cluster_state_change_detail(cluster),
                },
            ));
        }
        observed.sort_by_key(|(_, change)| change.at);
        for (cluster_id, change) in observed {
            let state = self
                .clusters
                .get(&cluster_id)
                .map(|cluster| cluster.status.state.clone())
                .unwrap_or_default();
            self.announced_cluster_states.insert(cluster_id, state);
            while self.pending_cluster_events.len() >= MAX_PENDING_EVENTS {
                self.pending_cluster_events.remove(0);
                ::tracing::warn!(
                    "the queue of Amazon EMR cluster state changes waiting to be published to \
                     EventBridge is full at {MAX_PENDING_EVENTS}; the oldest change was dropped"
                );
            }
            self.pending_cluster_events.push(change);
        }
    }

    /// Take at most `limit` of the changes not yet published, oldest first.
    ///
    /// Taking them out under the lock is what stops two concurrent callers from
    /// publishing one change twice.
    fn take_cluster_state_changes(&mut self, limit: usize) -> Vec<ClusterStateChange> {
        let taking = limit.min(self.pending_cluster_events.len());
        self.pending_cluster_events.drain(..taking).collect()
    }

    /// The earliest instant at which this store owes a CLUSTER transition, if any
    /// — the instant the clock pump waits for.
    ///
    /// Every instant a cluster's own lifecycle is waiting on: the two phases of
    /// coming up, the shutdown a cluster that does not stay up is owed once its
    /// queue has run ([`ClusterRecord::auto_terminate_due_at`]), and the end of a
    /// shutdown under way.
    pub fn next_cluster_due_at(&self) -> Option<SystemTime> {
        self.clusters
            .values()
            .flat_map(|cluster| {
                [
                    cluster.status.bootstrapping_due_at,
                    cluster.status.ready_due_at,
                    cluster.auto_terminate_due_at(),
                    cluster.status.terminated_due_at,
                ]
            })
            .flatten()
            .min()
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim
    /// — when this store owes nothing more.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical
    /// section, which is what makes the hand-over race-free: a write stores its
    /// cluster (and so its due instant) under this same lock BEFORE it tries to
    /// claim the pump, so either this call sees that due instant and the running
    /// pump keeps it, or the write's claim finds the flag already cleared and
    /// starts a pump of its own. A change recorded but not yet published keeps the
    /// pump awake too.
    fn next_cluster_due_or_release_pump(
        &mut self,
        now: SystemTime,
        generation: u64,
    ) -> Option<SystemTime> {
        if !self.pending_cluster_events.is_empty() {
            return Some(now);
        }
        match self.next_cluster_due_at() {
            Some(due) => Some(due),
            None => {
                self.release_cluster_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the
    /// caller may start one, `None` when one is already running.
    fn start_cluster_clock_pump(&mut self) -> Option<u64> {
        if self.cluster_clock_pump_running {
            return None;
        }
        self.cluster_clock_pump_running = true;
        self.cluster_clock_pump_generation = self.cluster_clock_pump_generation.wrapping_add(1);
        Some(self.cluster_clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one, so a pump that
    /// ended on the wake-up cap cannot clear a claim a later pump is holding.
    fn release_cluster_clock_pump(&mut self, generation: u64) {
        if self.cluster_clock_pump_generation == generation {
            self.cluster_clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces, so no
    /// earlier pump task runs beside the one that now holds the claim.
    fn set_cluster_clock_pump(&mut self, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.cluster_clock_pump.replace(handle) {
            previous.abort();
        }
    }
}

/// The instant this cluster's own timeline stamps on the state it now reports, or
/// `None` when it stamps none.
///
/// `ClusterTimeline` is where a cluster records its lifecycle — "the creation date
/// and time of the cluster", "the date and time when the cluster was ready to run
/// steps", "the date and time when the cluster was terminated" — so the event's
/// `time` is read off the record rather than taken from whoever drained the queue.
fn state_entered_at(cluster: &ClusterRecord) -> Option<SystemTime> {
    match cluster.status.state.as_str() {
        cluster_state::STARTING => Some(cluster.status.creation_date_time),
        cluster_state::RUNNING | cluster_state::WAITING => cluster.status.ready_date_time,
        cluster_state::TERMINATED | cluster_state::TERMINATED_WITH_ERRORS => {
            cluster.status.end_date_time
        }
        _ => None,
    }
}

/// The `detail` of the event announcing the state `cluster` has just entered.
///
/// Every member is a fact of the record: the cluster's id and name, the state it
/// entered, the reason that state carries (as the JSON document the member holds a
/// rendering of, which is how the bus carries it) and how the transition RATES in
/// the severity vocabulary this service publishes its events with — `ERROR` for
/// the state that reports a cluster that ended with errors, and `INFO` for every
/// other state, none of which reports a failure. Nothing is synthesised: a state
/// the cluster carries no reason for carries none here either.
fn cluster_state_change_detail(cluster: &ClusterRecord) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "severity".to_string(),
        ::serde_json::Value::from(
            if cluster.status.state == cluster_state::TERMINATED_WITH_ERRORS {
                SEVERITY_ERROR
            } else {
                SEVERITY_INFO
            },
        ),
    );
    detail.insert(
        "clusterId".to_string(),
        ::serde_json::Value::from(cluster.id.clone()),
    );
    detail.insert(
        "name".to_string(),
        ::serde_json::Value::from(cluster.name.clone()),
    );
    detail.insert(
        "state".to_string(),
        ::serde_json::Value::from(cluster.status.state.clone()),
    );
    if let Some(reason) = cluster.status.state_change_reason.as_ref() {
        let mut rendered = ::serde_json::Map::new();
        rendered.insert(
            "code".to_string(),
            ::serde_json::Value::from(reason.code.clone()),
        );
        if let Some(message) = reason.message.as_ref() {
            rendered.insert(
                "message".to_string(),
                ::serde_json::Value::from(message.clone()),
            );
        }
        detail.insert(
            "stateChangeReason".to_string(),
            ::serde_json::Value::from(::serde_json::Value::Object(rendered).to_string()),
        );
    }
    ::serde_json::Value::Object(detail)
}

/// Announce every cluster transition this store has to announce, and hand the rest
/// of the lifecycle to the clock pump.
///
/// Called by an operation that reported or wrote a cluster, AFTER it has released
/// the store's guard: this takes that guard itself to compare the cluster map
/// against what was last announced, so an operation still holding it must drop it
/// first. A call that moved nothing records nothing, so it costs one comparison of
/// the cluster map.
///
/// The publication itself is the pump's ([`drive_cluster_clock`]), which is what
/// lets an operation answer its caller without waiting on another service: the
/// event leaves one pump tick later, and each transition the clock still owes the
/// cluster leaves as it falls due.
pub fn settle_cluster_events<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) {
    if let Some(state) = service_state(&**engine) {
        state.lock().record_cluster_state_changes(SystemTime::now());
    }
    drive_cluster_clock(ctx, engine, region);
}

/// Publish every queued cluster state change, and leave the queue empty.
///
/// Called with NO guard held: each drain records and takes its changes out in one
/// critical section, so nothing is published twice, and the in-process `PutEvents`
/// runs outside it. The queue is drained until it is EMPTY — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]), and the loop is what guarantees the tail of a
/// burst goes out with it.
async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    loop {
        let changes = {
            let mut store = state.lock();
            store.record_cluster_state_changes(SystemTime::now());
            store.take_cluster_state_changes(MAX_PUBLISHED_PER_DRAIN)
        };
        if changes.is_empty() {
            return;
        }
        publish_batch(&client, &changes).await;
    }
}

/// Publish one drain, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, changes: &[ClusterStateChange]) {
    for window in changes.chunks(super::step_events::MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(CLUSTER_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the cluster moved, not when the
                // queue was drained: `PutEventsRequestEntry$Time` is "the time
                // stamp of the event, per RFC3339. If no time stamp is provided,
                // the time stamp of the PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    super::step_events::epoch_millis(change.at),
                ))
                .detail(change.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Amazon EMR cluster state change could not be published to EventBridge"
            );
        }
    }
}

/// Publish the cluster state changes the CLOCK owes, as they fall due, with no
/// further call from anybody.
///
/// Amazon EMR announces a cluster as it moves, which is the whole point of the
/// event path: a client that reacts to a cluster through EventBridge makes no
/// further Amazon EMR call, so a transition published only by the next reader
/// would never be published at all. This starts ONE in-process task that waits for
/// the earliest due instant the store holds, materialises what is then due
/// ([`ComAmazonawsEmrElasticmapreduceState::advance_all`], the same call every
/// reporting operation makes) and publishes what that produced, until nothing is
/// owed. A change already queued is itself work, so a pump started for one wakes
/// at once and publishes it.
///
/// It does not change WHEN a transition happens: `advance_all` applies only what
/// the stored due instant has already made due, so a caller reading before that
/// instant still sees the transient state, and two reads at the same instant still
/// answer identically. The task waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it exits as soon as the store owes
/// nothing. With no async runtime to spawn onto there is nothing to publish from,
/// so the transitions are materialised by the next reporting operation and
/// announced once a runtime is there.
pub fn drive_cluster_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_cluster_clock_pump() else {
        return;
    };
    let callback = ctx.callback().clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that
        // ran out of work has already released its claim inside the one critical
        // section that observed it (`next_cluster_due_or_release_pump`).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state.lock().next_cluster_due_or_release_pump(now, generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what
            // keeps the backstop above from ever costing a CPU-bound spin, and it
            // hands the runtime back between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            state.lock().advance_all(SystemTime::now());
            publish_pending(&callback, &*engine, &region).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its
            // own.
            state.lock().release_cluster_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely
    // forgotten.
    pumped.lock().set_cluster_clock_pump(handle);
}
