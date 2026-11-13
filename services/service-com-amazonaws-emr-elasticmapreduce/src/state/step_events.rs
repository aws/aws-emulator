// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a STEP moves through its states.
//!
//! # The rule this module implements
//!
//! Amazon EMR is an event SOURCE, not only a control plane a client polls: every state a
//! step MOVES INTO is published onto the account's default event bus
//! ([`DEFAULT_EVENT_BUS`]) under the `source` this service owns ([`EVENT_SOURCE`]) and the
//! `detail-type` of a step's state change ([`STEP_STATE_CHANGE_DETAIL_TYPE`]), carrying in
//! the event's `detail` the step the service holds at that instant — its id, its name, the
//! cluster it was submitted to, the state it has reached, how that state rates
//! ([`state_severity`]), what its cluster does when it fails and the sentence Amazon EMR
//! words the change in ([`StepStateChange::message`]). A rule on that bus whose `EventPattern` matches then delivers it to that rule's
//! targets, which is how a client reacts to a step — routing the failed ones to a queue,
//! alarming on them, reaping them — without calling Amazon EMR again, and a rule patterned
//! on another state receives nothing.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a step moves: the stored due instants do, through the
//! cluster's own step runner ([`ClusterRecord::advance_to`]) and the runs that answer for a
//! step's submission. This module only COMPARES the state the store now holds against the
//! one that was last published ([`StepRecord::published_state`]), so a transition is
//! published exactly once however it was materialised — the clock, a cancel, the shutdown of
//! its cluster, or the answer of the work it submitted — and no writer has to remember to
//! announce itself. A state nothing moved publishes nothing, so two calls at the same
//! instant publish the same single event.
//!
//! WHICH call publishes: the operation that MATERIALISED the transition, once it has
//! released this service's store ([`publish_step_state_changes`]) — the same seam a step's
//! own run is performed on. The event's `time` is the instant the step reached that state,
//! not the instant the publication happened to be made, so an event reports the timeline a
//! read of the same step reports.
//!
//! Publication is made with NO store lock held: EventBridge is reached the way every
//! emulated service reaches another, through the framework's in-process callback
//! (`service_common::eventbridge_client`), so this crate names no other service's crate. It
//! is best effort — an emulator with no EventBridge mounted has nowhere to publish — so a
//! failure is logged and the operation's own answer stands; the transitions of a publication
//! that failed are not consumed, so the next call that reports the cluster publishes the
//! state each of those steps is in ([`ComAmazonawsEmrElasticmapreduceState::forget_published_steps`]).

use ::std::time::UNIX_EPOCH;

use super::*;

/// The `source` of every event Amazon EMR publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's own namespace is the one its ARNs carry ([`ARN_VENDOR`] is the ARN
/// vendor; the source names the service, `emr`).
pub const EVENT_SOURCE: &str = "aws.emr";

/// The `detail-type` of the event published when a STEP changes state.
pub const STEP_STATE_CHANGE_DETAIL_TYPE: &str = "EMR Step Status Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// `detail.severity` of a state that is the ordinary course of a cluster's work: a step that
/// is queued, running, or has completed.
pub const SEVERITY_INFO: &str = "INFO";

/// `detail.severity` of a state in which the step did not fail but did not run to its end
/// either: a step that was cancelled, or cut short by the shutdown of its cluster.
pub const SEVERITY_WARN: &str = "WARN";

/// `detail.severity` of the state a step FAILED in.
pub const SEVERITY_ERROR: &str = "ERROR";

/// How the state a step reached RATES, in the severity vocabulary Amazon EMR publishes its
/// events with (`INFO`, `WARN`, `ERROR`) — which is a member of the event rather than of the
/// step, so it is derived from the state and stored nowhere.
///
/// Recorded behavior for the three values: a step reaching a state of the ordinary course —
/// queued, running, completed — is [`SEVERITY_INFO`]; the one that FAILED is the error
/// ([`SEVERITY_ERROR`]); and a step that was cancelled or cut short did not fail but did not
/// finish either ([`SEVERITY_WARN`]).
#[must_use]
pub fn state_severity(state: &str) -> &'static str {
    match state {
        step_state::FAILED => SEVERITY_ERROR,
        step_state::CANCEL_PENDING | step_state::CANCELLED | step_state::INTERRUPTED => {
            SEVERITY_WARN
        }
        _ => SEVERITY_INFO,
    }
}

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a call that materialised more transitions than that
/// publishes them in several requests rather than one the service would refuse.
///
/// `pub(super)` for the reason [`epoch_millis`] is.
pub(super) const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many step transitions ONE call records and publishes.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: each published event fans out to the targets
/// of every rule that matches it, so the work one request can set off is bounded here. A
/// transition past it stays unpublished and is published by the next call that reports the
/// cluster, so nothing is lost. It is above the step queue a cluster can run at once
/// (`MAX_STEP_CONCURRENCY_LEVEL`) by a wide margin, so no realistic call reaches it.
const MAX_RECORDED_CHANGES: usize = 64;

/// The form the minute a transition happened in is written in inside an event's message,
/// as Amazon EMR words one: the calendar date and the minute, in UTC.
const MESSAGE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// One step transition this service materialised, waiting to be published.
///
/// Every field is read off the step record the service's own reads are projected from, so
/// the event reports the step a `DescribeStep` of the same instant reports and never a
/// second, invented copy of it.
#[derive(Debug, Clone)]
pub struct StepStateChange {
    /// The cluster the step was submitted to.
    pub cluster_id: String,
    /// "The name of the cluster." (`Cluster$Name` doc.) The event's own message names the
    /// cluster by it, beside its id.
    pub cluster_name: String,
    /// The step that moved.
    pub step_id: String,
    /// "The name of the cluster step." (`Step$Name` doc.)
    pub name: String,
    /// The `StepState` the step moved INTO.
    pub state: String,
    /// "The action to take when the cluster step fails." (`Step$ActionOnFailure` doc.) A
    /// step that named none states none.
    pub action_on_failure: Option<String>,
    /// WHEN the step reached that state — the instant its own timeline records, not the
    /// instant this event happened to be published.
    pub at: SystemTime,
}

impl StepStateChange {
    /// The event `detail` this transition is reported in.
    ///
    /// Public because it IS the contract: the members a rule's `EventPattern` filters on,
    /// which is what a suite of this crate holds the published event to.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        let mut detail = ::serde_json::Map::new();
        detail.insert(
            "severity".to_string(),
            ::serde_json::Value::String(state_severity(&self.state).to_string()),
        );
        detail.insert(
            "stepId".to_string(),
            ::serde_json::Value::String(self.step_id.clone()),
        );
        detail.insert(
            "name".to_string(),
            ::serde_json::Value::String(self.name.clone()),
        );
        detail.insert(
            "clusterId".to_string(),
            ::serde_json::Value::String(self.cluster_id.clone()),
        );
        detail.insert(
            "state".to_string(),
            ::serde_json::Value::String(self.state.clone()),
        );
        if let Some(action) = &self.action_on_failure {
            detail.insert(
                "actionOnFailure".to_string(),
                ::serde_json::Value::String(action.clone()),
            );
        }
        detail.insert(
            "message".to_string(),
            ::serde_json::Value::String(self.message()),
        );
        ::serde_json::Value::Object(detail)
    }

    /// The event's `message`: the sentence Amazon EMR words a step's state change in.
    ///
    /// Recorded behavior for the FORM, whose every variable part is read off the step and
    /// its cluster: the step's id and name, the cluster's id and name, what became of the
    /// step and the minute that happened in, in UTC — "Step `<step id>` (`<step name>`) in
    /// Amazon EMR cluster `<cluster id>` (`<cluster name>`) failed at
    /// `<YYYY-MM-DD HH:MM>` UTC." The phrase in the middle is the one for the state the step
    /// reached ([`state_phrase`]).
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "Step {} ({}) in Amazon EMR cluster {} ({}) {} at {} UTC.",
            self.step_id,
            self.name,
            self.cluster_id,
            self.cluster_name,
            state_phrase(&self.state),
            ::chrono::DateTime::<::chrono::Utc>::from(self.at).format(MESSAGE_TIME_FORMAT),
        )
    }
}

/// What became of a step that reached `state`, as the event's message says it
/// ([`StepStateChange::message`]).
///
/// One phrase per `StepState` the model declares: a step is queued until its cluster starts
/// it, runs, and then either completed, failed, was cancelled — on the caller's request,
/// which it is first asked for — or was cut short by the shutdown of its cluster.
fn state_phrase(state: &str) -> &'static str {
    match state {
        step_state::PENDING => "is pending",
        step_state::RUNNING => "is running",
        step_state::COMPLETED => "completed",
        step_state::FAILED => "failed",
        step_state::CANCEL_PENDING => "is being cancelled",
        step_state::CANCELLED => "was cancelled",
        step_state::INTERRUPTED => "was interrupted",
        // Every state the model declares is named above; a state this service does not set
        // is still reported as the change it is rather than as one of them.
        _ => "changed state",
    }
}

impl StepRecord {
    /// The instant this step reached the state it is in.
    ///
    /// Read off the step's own timeline, which is what a reader reports: a step that has
    /// ENDED reached its state when it ended, one that is running when it started, and one
    /// that is still queued when it was created. The instant is never `now`, so the event
    /// and the step's `StepTimeline` agree however late the publication is made.
    fn state_reached_at(&self) -> SystemTime {
        if self.is_terminal() {
            return self
                .end_date_time
                .or(self.start_date_time)
                .unwrap_or(self.creation_date_time);
        }
        if self.state == step_state::RUNNING {
            return self.start_date_time.unwrap_or(self.creation_date_time);
        }
        self.creation_date_time
    }
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// The transitions of the cluster `cluster_id`'s steps that have not been published yet,
    /// recorded as published.
    ///
    /// Called with the store held, right after the transitions were materialised, and
    /// answers nothing for a cluster this account does not hold. At most
    /// [`MAX_RECORDED_CHANGES`] are taken; the rest keep the state they were last published
    /// in, so the next call that reports the cluster records them. A transition recorded here
    /// whose publication then FAILS is given back ([`Self::forget_published_steps`]), so a
    /// transition is published once but never consumed unpublished.
    pub fn record_step_state_changes(&mut self, cluster_id: &str) -> Vec<StepStateChange> {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        // The cluster's own name is read once, before its queue is walked: the event names
        // the cluster by it beside its id.
        let cluster_name = cluster.name.clone();
        for step in &mut cluster.steps {
            if changes.len() >= MAX_RECORDED_CHANGES {
                break;
            }
            if step.published_state.as_deref() == Some(step.state.as_str()) {
                continue;
            }
            changes.push(StepStateChange {
                cluster_id: cluster_id.to_string(),
                cluster_name: cluster_name.clone(),
                step_id: step.id.clone(),
                name: step.name.clone(),
                state: step.state.clone(),
                action_on_failure: step.action_on_failure.clone(),
                at: step.state_reached_at(),
            });
            step.published_state = Some(step.state.clone());
        }
        changes
    }

    /// Give back the record of publication for the steps `step_ids` of the cluster
    /// `cluster_id`: their transitions were recorded as published and the publication did not
    /// go through, so the state each of them is in is published by the next call that reports
    /// the cluster rather than being lost.
    pub fn forget_published_steps(&mut self, cluster_id: &str, step_ids: &[String]) {
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return;
        };
        for step in &mut cluster.steps {
            if step_ids.contains(&step.id) {
                step.published_state = None;
            }
        }
    }
}

/// Publish every step transition of the cluster `cluster_id` this service has materialised
/// and not published yet.
///
/// Called by the operations that materialise a step's transitions, AFTER they have released
/// this service's store: a publication reaches EventBridge's own handler, which matches the
/// event against the account's rules and delivers it to their targets — and a target may
/// call back into this service, so holding this store across it would deadlock the very call
/// the event set off.
pub async fn publish_step_state_changes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_id: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let changes = {
        let mut store = state.lock();
        store.record_step_state_changes(cluster_id)
    };
    let unpublished = publish(ctx.callback(), region, &changes).await;
    if !unpublished.is_empty() {
        let mut store = state.lock();
        store.forget_published_steps(cluster_id, &unpublished);
    }
}

/// Publish `changes` onto the account's default EventBridge bus, and answer the steps whose
/// transition could NOT be published.
///
/// Publication is best effort — an emulator with no EventBridge mounted has nowhere to
/// publish — so a failure is logged and the operation's own answer stands; what it is not is
/// silent loss, which is why the steps of a request that failed are answered and their
/// record of publication given back.
pub async fn publish(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    changes: &[StepStateChange],
) -> Vec<String> {
    if changes.is_empty() {
        return Vec::new();
    }
    let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
    let mut unpublished: Vec<String> = Vec::new();
    for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(STEP_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the step reached the state:
                // `PutEventsRequestEntry$Time` is "the time stamp of the event, per
                // RFC3339. If no time stamp is provided, the time stamp of the PutEvents
                // call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(change.at),
                ))
                .detail(change.detail().to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Amazon EMR step state change could not be published to EventBridge"
            );
            unpublished.extend(window.iter().map(|change| change.step_id.clone()));
        }
    }
    unpublished
}

/// `at` as the epoch milliseconds the event's `time` carries.
///
/// `pub(super)` so the sibling publisher of this service's OTHER event class
/// ([`super::cluster_events`]) renders the same member the same way.
pub(super) fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}
