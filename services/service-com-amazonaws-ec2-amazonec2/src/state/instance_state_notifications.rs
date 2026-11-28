// SPDX-License-Identifier: CC0-1.0

//! The event every change of an INSTANCE's state is published as.
//!
//! Amazon EC2 is an event SOURCE, not only a control plane a client polls: "whenever the state
//! of an instance changes, Amazon EC2 sends an EC2 Instance State-change Notification event to
//! Amazon EventBridge" (the EC2 User Guide's instance state-change events). The event goes to
//! the account's default event bus ([`super::DEFAULT_EVENT_BUS`]) under the `source` this
//! service owns ([`super::EC2_EVENT_SOURCE`]) and the `detail-type` of an instance state
//! change ([`INSTANCE_STATE_CHANGE_DETAIL_TYPE`]), naming the instance in the envelope's
//! `resources` and carrying in the event `detail` the instance's id and the state it moved
//! INTO. So a client that drives automation off an instance's lifecycle — an alarm, a cleanup
//! job, a queue a rule delivers to — learns that an instance came up, stopped or was taken
//! back without polling EC2, and a client waiting for the notification is told instead of
//! hanging.
//!
//! WHICH moves are published: those of the doors that record them. A call that moves an
//! instance — a launch (`RunInstances`, and the capacity an EC2 Fleet places), a stop, a start
//! or a termination — publishes the state it moved the instance into at once, and the states
//! the CLOCK still owes that instance ([`super::InstanceRecord::owed_transitions`]) each when
//! that move falls due ([`InstanceStateNotification::publish_when_due`]) — a client watching
//! the bus makes no further EC2 call, so a move published only by the next reader would never
//! be published at all. A call that moved nothing (a repeated stop or terminate) announces
//! nothing: a door publishes the moves IT made.
//!
//! A move the instance never makes is never published either, and the question asked of the
//! store is whether that move HAPPENED — not where the instance is by the time the
//! publication is made. A state an instance ENTERED is a fact of its history: the clock
//! records every move it applies ([`super::AppliedInstanceMove`]) and the publication TAKES
//! that record, so a launch something terminated before it came up announces no `running`
//! (the clock never made that move), while the `stopped` of a stop is announced even where
//! the caller that was waiting for it started the instance again in the same instant the move
//! fell due. Which of the two it is cannot depend on who got there first: on AWS an instance
//! that entered a state announces it whatever it does next.
//!
//! The one instance move no door records here is the instance a Spot Instance request's own
//! FULFILMENT mints: nothing calls to start it — the clock fulfils a due request inside
//! whatever call happens to materialise it — so announcing it needs the store to record the
//! event rather than the door, which this module leaves to the Spot path's own work.
//!
//! The event's `time` is the instant the move falls DUE, not the instant the publication was
//! made, so a consumer reads when the instance moved.
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over the
//! framework's in-process callback, through the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the
//! instance was launched in. Publication is BEST EFFORT — an emulator with no EventBridge
//! mounted has nowhere to publish — so a failure is traced and the operation's own answer
//! stands.

use ::std::time::{Duration, Instant};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{
    ComAmazonawsEc2Amazonec2State, DEFAULT_EVENT_BUS, EC2_EVENT_SOURCE, InstanceStateName,
    epoch_millis_in, region_of_zone_name,
};

/// The `detail-type` every notification about one INSTANCE's state is published under — the
/// whole class of them, which is what a rule written for an instance's lifecycle matches on.
pub const INSTANCE_STATE_CHANGE_DETAIL_TYPE: &str = "EC2 Instance State-change Notification";

/// How many instance moves this process publishes AT ONCE.
///
/// Each publication waits out the span the store owes the move and then makes one in-process
/// call, so the work in flight grows with the rate instances are moved at unless it is capped.
/// The figure is well above the moves one caller's calls can have in flight within a single
/// transition span — a launch of the largest batch this service serves owes two each — and far
/// below the instances this store holds at all ([`super::MAX_STORED_INSTANCES`]), so an
/// ordinary client never meets it and a flood of calls cannot grow this process without limit.
pub const MAX_CONCURRENT_INSTANCE_PUBLICATIONS: usize = 512;

/// The slots those publications take, one each, held until the publication ends.
static PUBLICATION_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_INSTANCE_PUBLICATIONS);

/// How many of the moves its clock has applied this store remembers for the publications that
/// report them.
///
/// One entry is recorded per move the clock makes and taken again by the publication of that
/// move, which is made the moment the move falls due, so what is held at any instant is the
/// moves of the last transition span. The figure is far above the moves one caller's calls can
/// have in flight within such a span — a launch of the largest batch this service serves owes
/// two each — so an ordinary client never meets it, and a move nothing ever reports (a
/// publication this process could not spawn) is dropped once that many newer moves have been
/// recorded rather than kept for ever.
pub const MAX_RECORDED_INSTANCE_MOVES: usize = 4096;

/// One move this store's CLOCK has applied to an instance, waiting to be reported.
///
/// The record that a move HAPPENED: the instance it moved, the state it moved INTO and the
/// instant that move fell due, which is what tells one entry into a state from the next (a
/// stop and a later stop of the same instance each owe their own `stopped`). Written by
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] as it applies the move and
/// taken by
/// [`ComAmazonawsEc2Amazonec2State::take_applied_instance_move`], so the move is published
/// exactly once and the publication does not have to find the instance still in that state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedInstanceMove {
    /// The instance the clock moved.
    pub instance_id: String,
    /// The state it moved INTO.
    pub state: InstanceStateName,
    /// The instant that move fell due, which is the instant it was made at.
    pub due_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Record that the clock moved `instance_id` into `state`, a move that fell due at
    /// `due_at`.
    ///
    /// Called by [`Self::materialize_due_transitions`] for each move it applies — the ONE
    /// place an owed instance move is made — so every such move is recorded once, whichever
    /// call happened to carry the sweep. The log is bounded by
    /// [`MAX_RECORDED_INSTANCE_MOVES`]: the oldest entry goes when a new one would pass that,
    /// so a move nothing reports cannot grow this store without limit.
    pub fn record_applied_instance_move(
        &mut self,
        instance_id: &str,
        state: InstanceStateName,
        due_at: Instant,
    ) {
        while self.applied_instance_moves.len() >= MAX_RECORDED_INSTANCE_MOVES {
            self.applied_instance_moves.pop_front();
        }
        self.applied_instance_moves.push_back(AppliedInstanceMove {
            instance_id: instance_id.to_string(),
            state,
            due_at,
        });
    }

    /// Whether the clock moved `instance_id` into the state `state` names at `due_at`, TAKING
    /// that record.
    ///
    /// Taken rather than read, so the move it stands for is reported exactly once however many
    /// publications were started for it, and so the log holds only moves nothing has reported
    /// yet. `false` for a move this store never made — the one the instance was taken out from
    /// under — which is what withholds an event for a move that never happened.
    pub fn take_applied_instance_move(
        &mut self,
        instance_id: &str,
        state: &str,
        due_at: Instant,
    ) -> bool {
        let found = self.applied_instance_moves.iter().position(|applied| {
            applied.instance_id == instance_id
                && applied.state.as_str() == state
                && applied.due_at == due_at
        });
        match found {
            Some(index) => {
                self.applied_instance_moves.remove(index);
                true
            }
            None => false,
        }
    }
}

/// One instance state-change notification, rendered from the stored instance and waiting to be
/// published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceStateNotification {
    /// The Region the event is published in: the Region the instance runs in.
    pub region: String,
    /// The instance's ARN, which the envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    pub instance_arn: String,
    /// The instance that moved, as the `detail` names it.
    pub instance_id: String,
    /// The state it moved INTO, spelled as the `InstanceStateName` catalog spells it.
    pub state: String,
    /// WHEN that move falls due, in epoch milliseconds.
    pub at_millis: i64,
    /// How long the clock still owes before that instant.
    pub due_in: Duration,
    /// The instant the CLOCK owes this move at, for a move the clock still has to make.
    ///
    /// `None` for the state the instance is in already — the one the call that asked for this
    /// notification put it in while it held the store's guard, so that move is a fact and
    /// needs no further evidence. `Some` names the move in the clock's own record of what it
    /// applied ([`AppliedInstanceMove`]), which is what the publication asks for.
    pub due_at: Option<Instant>,
}

impl InstanceStateNotification {
    /// The event's `detail`: which instance, and the state it moved into.
    #[must_use]
    pub fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "instance-id": self.instance_id,
            "state": self.state,
        })
    }

    /// Publish this notification once the move it reports has FALLEN DUE, without the call
    /// that started the move waiting for it.
    ///
    /// An instance moves on the store's own clock, so the move this event reports is not a
    /// fact yet when the call is answered: publishing it then would tell a consumer that an
    /// instance was running while a read of it still said `pending`. The publication is
    /// therefore made from a task that resumes at the due instant — the one thing in this
    /// service that waits for a clock-owed instant, because no later call of this account can
    /// carry it: a consumer of the bus makes none. That task holds no lock across the
    /// publication, it makes exactly one publication and ends, and there is at most one of it
    /// per move this service recorded.
    ///
    /// `state` is this service's store, which the task re-reads at the due instant: the move
    /// is MATERIALISED there (it was owed at that instant whether or not anybody called) and
    /// the event is published only when the clock really made the move this event reports —
    /// asked of the store's own record of the moves it applied ([`AppliedInstanceMove`]),
    /// which is TAKEN by this publication. So an instance something moved out from under the
    /// move announces only the moves that happened, and a move that happened is announced even
    /// where the caller waiting for it moved the instance on in the same instant.
    ///
    /// The tasks in flight are CAPPED at [`MAX_CONCURRENT_INSTANCE_PUBLICATIONS`]: a slot is
    /// taken before one is spawned and held inside it until it ends, on every path, so the work
    /// and the in-process calls this makes cannot grow with the rate calls arrive at. A move
    /// that finds no slot is not published, and says so in the log, which is the same
    /// best-effort answer as an emulator with no EventBridge to publish to.
    ///
    /// Call it with NO store lock held.
    pub fn publish_when_due(
        self,
        state: ::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
        callback: EmbeddedEmulator,
    ) {
        let Ok(slot) = PUBLICATION_SLOTS.try_acquire() else {
            ::tracing::warn!(
                instance = %self.instance_id,
                "this process is already publishing {MAX_CONCURRENT_INSTANCE_PUBLICATIONS} \
                 instance moves, so this one is not published"
            );
            return;
        };
        ::tokio::spawn(async move {
            // Held for the whole publication and given back when this task ends, however it
            // ends, so the cap above counts what is really in flight.
            let _slot = slot;
            if !self.due_in.is_zero() {
                ::tokio::time::sleep(self.due_in).await;
            }
            if let Some(due_at) = self.due_at {
                if !clock_made_move(&state, &self.instance_id, &self.state, due_at) {
                    ::tracing::debug!(
                        instance = %self.instance_id,
                        state = %self.state,
                        "this instance did not move into the state this event would report, so \
                         nothing is published for it"
                    );
                    return;
                }
            }
            self.publish(&callback).await;
        });
    }

    /// Publish this notification onto the account's default event bus in the instance's
    /// Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller of
    /// the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &EmbeddedEmulator) {
        let client =
            ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EC2_EVENT_SOURCE)
            .detail_type(INSTANCE_STATE_CHANGE_DETAIL_TYPE)
            // The event's `time` is WHEN the move falls due, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If no
            // time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at_millis,
            ))
            .resources(&self.instance_arn)
            .detail(self.detail().to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    instance = %self.instance_id,
                    region = %self.region,
                    "an instance state-change notification was not accepted by the default \
                     event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    instance = %self.instance_id,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an instance state-change notification could not be published"
                );
            }
        }
    }
}

/// Whether the CLOCK moved the instance `instance_id` into the state `expected` names at
/// `due_at`, with everything the clock has already made due applied first.
///
/// A WRITE of exactly the kind a reporting operation makes: each move it materialises was owed
/// at this instant whether or not anybody called, so nothing a later caller would otherwise
/// have seen changes. The record of the move is taken with it, which is bookkeeping of what
/// has been reported and changes no answer this store gives a caller. Taken and given back
/// without awaiting anything, so no lock of this store is ever held across a call to another
/// service.
fn clock_made_move(
    state: &::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
    instance_id: &str,
    expected: &str,
    due_at: Instant,
) -> bool {
    let mut store = state.lock();
    store.materialize_due_transitions();
    store.take_applied_instance_move(instance_id, expected, due_at)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Every state-change event the instances `instance_ids` name owe as of `now`: the state
    /// each is in — the one the call that just moved it put it in — and each state the clock
    /// still owes it, at its own due instant.
    ///
    /// The ONE rendering of those events, so every door that moves an instance publishes the
    /// same event for the same move: a handler asks for them while it still holds this store's
    /// guard and publishes them once it has given the guard back
    /// ([`InstanceStateNotification::publish_when_due`]). An id this store holds no instance
    /// for contributes nothing.
    ///
    /// The Region each event is published in is the one the instance's own Availability Zone
    /// belongs to, read through the single reading of that ([`region_of_zone_name`]), so an
    /// instance and the events about it can never be placed in two Regions.
    #[must_use]
    pub fn instance_state_change_notifications(
        &self,
        instance_ids: &[String],
        now: Instant,
    ) -> Vec<InstanceStateNotification> {
        let mut notifications: Vec<InstanceStateNotification> = Vec::new();
        for instance_id in instance_ids {
            let Some(record) = self.instances.get(instance_id) else {
                continue;
            };
            let region = region_of_zone_name(&record.placement_az).to_string();
            let instance_arn =
                ::engine::emulator_arn("ec2", &region, &format!("instance/{instance_id}"));
            // The state it is in now — a move the calling door has just made, so it carries no
            // due instant to be judged by — and every state the clock owes it thereafter
            // ([`super::InstanceRecord::owed_transitions`], where the moves this store will
            // make are read), each at the instant that move falls due: the same instant the
            // clock records it under, so the events and the store cannot disagree about where
            // an instance is headed or about which move an event reports.
            let mut moves: Vec<(InstanceStateName, Option<Instant>)> = vec![(record.state, None)];
            moves.extend(
                record
                    .owed_transitions()
                    .into_iter()
                    .map(|(state, due_at)| (state, Some(due_at))),
            );
            for (state, due_at) in moves {
                let due_in =
                    due_at.map_or(Duration::ZERO, |at| at.saturating_duration_since(now));
                notifications.push(InstanceStateNotification {
                    region: region.clone(),
                    instance_arn: instance_arn.clone(),
                    instance_id: instance_id.clone(),
                    state: state.as_str().to_string(),
                    at_millis: epoch_millis_in(due_in),
                    due_in,
                    due_at,
                });
            }
        }
        notifications
    }
}

