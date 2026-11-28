// SPDX-License-Identifier: CC0-1.0

//! The Spot fleet request store: the request record, its fulfilled capacity and the bytes it
//! retains.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The prefix of every Spot Fleet request id, which an `sfr-` id carries a UUID after the
/// way an EC2 Fleet id does ([`FLEET_ID_PREFIX`]).
pub const SPOT_FLEET_REQUEST_ID_PREFIX: &str = "sfr-";

/// The `submitted` member of the modelled `BatchState` catalog
/// ([`crate::ops::types::BATCH_STATE_VALUES`]): where a Spot Fleet request AWS has just
/// accepted sits, before it is `active`.
pub const SPOT_FLEET_STATE_SUBMITTED: &str = "submitted";

/// The `active` member of [`crate::ops::types::BATCH_STATE_VALUES`]: a Spot Fleet request
/// that is placing and holding capacity.
pub const SPOT_FLEET_STATE_ACTIVE: &str = "active";

/// The `modifying` member of [`crate::ops::types::BATCH_STATE_VALUES`]: a Spot Fleet request
/// whose target capacity is being changed.
pub const SPOT_FLEET_STATE_MODIFYING: &str = "modifying";

/// The `cancelled_running` member of [`crate::ops::types::BATCH_STATE_VALUES`]: "the Spot
/// Fleet request enters the `cancelled_running` state and the instances continue to run until
/// they are interrupted or you terminate them manually" (`CancelSpotFleetRequests`).
pub const SPOT_FLEET_STATE_CANCELLED_RUNNING: &str = "cancelled_running";

/// The `cancelled_terminating` member of [`crate::ops::types::BATCH_STATE_VALUES`]: "if you
/// choose to terminate the instances, the Spot Fleet request enters the
/// `cancelled_terminating` state" (`CancelSpotFleetRequests`).
pub const SPOT_FLEET_STATE_CANCELLED_TERMINATING: &str = "cancelled_terminating";

/// The `cancelled` member of [`crate::ops::types::BATCH_STATE_VALUES`]: where a cancelled
/// Spot Fleet request that has no running Spot Instance left ends up.
pub const SPOT_FLEET_STATE_CANCELLED: &str = "cancelled";

/// The `BatchState` members a Spot Fleet request can be CANCELLED from: one that is holding
/// capacity, one that has just been accepted, one whose capacity is being changed — and one
/// whose cancellation LEFT ITS INSTANCES RUNNING.
///
/// A cancellation is admissible exactly while something is left to cancel. The first three
/// states are a fleet that is still placing or holding capacity. The fourth is the second step
/// of the cancellation AWS documents: "otherwise, the Spot Fleet request enters the
/// `cancelled_running` state and the instances continue to run until they are interrupted or
/// you terminate them manually" (`CancelSpotFleetRequests`) — the instances are still there, so
/// a further cancel with `TerminateInstances` set is what takes them down and moves the fleet
/// on, and refusing it would leave that documented two-step cancel unusable.
///
/// Every other member is a fleet whose cancellation has been carried out
/// ([`SPOT_FLEET_STATE_CANCELLED_TERMINATING`], [`SPOT_FLEET_STATE_CANCELLED`]) or that failed,
/// so there is nothing left to ask for and
/// [`CANCEL_BATCH_FLEET_REQUEST_NOT_IN_CANCELLABLE_STATE`] is reserved for those.
pub const SPOT_FLEET_CANCELLABLE_STATES: &[&str] = &[
    SPOT_FLEET_STATE_SUBMITTED,
    SPOT_FLEET_STATE_ACTIVE,
    SPOT_FLEET_STATE_MODIFYING,
    SPOT_FLEET_STATE_CANCELLED_RUNNING,
];

/// The most Spot Fleet requests ONE `CancelSpotFleetRequests` call may name: "you can delete
/// up to 100 fleets in a single request. If you exceed the specified number, no fleets are
/// deleted" (`CancelSpotFleetRequests`), which is also the constraint
/// `CancelSpotFleetRequestsRequest$SpotFleetRequestIds` states ("you can specify up to 100
/// IDs in a single request").
pub const MAX_SPOT_FLEET_REQUESTS_PER_CANCEL: usize = 100;

/// Whether a Spot Fleet request in `state` can be cancelled
/// ([`SPOT_FLEET_CANCELLABLE_STATES`]).
pub fn spot_fleet_request_state_is_cancellable(state: &str) -> bool {
    SPOT_FLEET_CANCELLABLE_STATES.contains(&state)
}

/// The `fleetRequestIdDoesNotExist` member of
/// [`crate::ops::types::CANCEL_BATCH_ERROR_CODE_VALUES`]: the per-fleet code
/// `CancelSpotFleetRequests` reports for an id this account holds no Spot Fleet request for.
pub const CANCEL_BATCH_FLEET_REQUEST_ID_DOES_NOT_EXIST: &str = "fleetRequestIdDoesNotExist";

/// The `fleetRequestIdMalformed` member of
/// [`crate::ops::types::CANCEL_BATCH_ERROR_CODE_VALUES`]: the per-fleet code for something
/// that is not a Spot Fleet request id at all.
pub const CANCEL_BATCH_FLEET_REQUEST_ID_MALFORMED: &str = "fleetRequestIdMalformed";

/// The `fleetRequestNotInCancellableState` member of
/// [`crate::ops::types::CANCEL_BATCH_ERROR_CODE_VALUES`]: the per-fleet code for a fleet in a
/// `BatchState` it cannot be cancelled from ([`SPOT_FLEET_CANCELLABLE_STATES`]).
pub const CANCEL_BATCH_FLEET_REQUEST_NOT_IN_CANCELLABLE_STATE: &str =
    "fleetRequestNotInCancellableState";

/// The `unexpectedError` member of [`crate::ops::types::CANCEL_BATCH_ERROR_CODE_VALUES`]: the
/// per-fleet code for a fleet the cancellation did not carry out for a reason that is none of
/// the others, so no id a call named is ever left out of both of its answer's sets.
pub const CANCEL_BATCH_UNEXPECTED_ERROR: &str = "unexpectedError";

/// The message the per-fleet [`CANCEL_BATCH_UNEXPECTED_ERROR`] carries.
pub fn spot_fleet_request_unexpected_error_message(request_id: &str) -> String {
    format!("An unexpected error occurred cancelling the spot fleet request '{request_id}'")
}

/// The `pending_fulfillment` member of the modelled `ActivityStatus` catalog
/// ([`crate::ops::types::ACTIVITY_STATUS_VALUES`]): "After all requests are placed, the
/// status is `pending_fulfillment`" (`SpotFleetRequestConfig$ActivityStatus`).
pub const SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT: &str = "pending_fulfillment";

/// The `fulfilled` member of [`crate::ops::types::ACTIVITY_STATUS_VALUES`]: "If the size of
/// the fleet is equal to or greater than its target capacity, the status is `fulfilled`"
/// (`SpotFleetRequestConfig$ActivityStatus`).
pub const SPOT_FLEET_ACTIVITY_FULFILLED: &str = "fulfilled";

/// The `pending_termination` member of [`crate::ops::types::ACTIVITY_STATUS_VALUES`]: "if the
/// size of the fleet is decreased, the status is `pending_termination` while Spot Instances
/// are terminating" (`SpotFleetRequestConfig$ActivityStatus`).
pub const SPOT_FLEET_ACTIVITY_PENDING_TERMINATION: &str = "pending_termination";

/// The `request` member of [`FLEET_TYPE_VALUES`]: a Spot Fleet request that "only places the
/// required requests" and does not replenish them
/// (`SpotFleetRequestConfigData$Type`).
pub const FLEET_TYPE_REQUEST: &str = "request";

/// The resource type a `RequestSpotFleet` tag specification may name: "The value for
/// `ResourceType` must be `spot-fleet-request`, otherwise the Spot Fleet request fails"
/// (`SpotFleetRequestConfigData$TagSpecifications`), a member of the modelled `ResourceType`
/// catalog.
pub const RESOURCE_TYPE_SPOT_FLEET_REQUEST: &str = "spot-fleet-request";

/// The `lowestPrice` member of the modelled `AllocationStrategy` catalog
/// ([`crate::ops::types::ALLOCATION_STRATEGY_VALUES`]), which is also the member of
/// `OnDemandAllocationStrategy` both strategies default to: "if you do not specify a value,
/// Spot Fleet defaults to `lowestPrice`" (`SpotFleetRequestConfigData$OnDemandAllocationStrategy`).
pub const SPOT_FLEET_ALLOCATION_STRATEGY_LOWEST_PRICE: &str = "lowestPrice";

/// The `diversified` member of [`crate::ops::types::ALLOCATION_STRATEGY_VALUES`]: the
/// strategy that spreads a fleet's target capacity over its pools rather than placing it in
/// one ("you can specify that the Spot Fleet distribute the target capacity across the Spot
/// pools included in its launch specifications", `RequestSpotFleet`).
pub const SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED: &str = "diversified";

/// The most Spot pools one Spot Fleet request may describe — launch specifications, or
/// launch template overrides.
///
/// Neither list carries a modelled `@length`, so this is what bounds the per-pool work one
/// call asks for: an AMI, a subnet, a security-group and a launch-template resolution each.
pub const MAX_SPOT_FLEET_POOLS: usize = 100;

/// How many Spot Fleet requests this store holds, on the ceiling every store of this module
/// that mints its own ids is held to ([`MAX_STORED_RECORDS_PER_STORE`]).
pub const MAX_SPOT_FLEET_REQUESTS: usize = MAX_STORED_RECORDS_PER_STORE;

/// The most Spot Instances ONE Spot Fleet request places in this emulator.
///
/// `TargetCapacity` is a plain modelled `Integer` with no `@range`, and a fleet places one
/// Spot Instance request per instance its capacity takes, so this is what bounds the work
/// and the memory one call can ask for. A fleet whose target capacity needs more than this
/// places this many and keeps the activity status
/// [`SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT`], which is the status AWS reports for a fleet
/// whose size is below its target — never a capacity it did not place.
pub const MAX_SPOT_FLEET_SPOT_INSTANCES: usize = MAX_INSTANCES_PER_RUN as usize;

/// `InvalidSpotFleetRequestId.Malformed` (HTTP 400): the request named something that is not
/// a Spot Fleet request id at all, so there is nothing to look up.
pub const INVALID_SPOT_FLEET_REQUEST_ID_MALFORMED: &str = "InvalidSpotFleetRequestId.Malformed";

/// `InvalidSpotFleetRequestId.NotFound` (HTTP 400): the request named a Spot Fleet request id
/// this account holds no Spot Fleet request for.
pub const INVALID_SPOT_FLEET_REQUEST_ID_NOT_FOUND: &str = "InvalidSpotFleetRequestId.NotFound";

/// Whether `id` is a well-formed Spot Fleet request id: [`SPOT_FLEET_REQUEST_ID_PREFIX`] and
/// the UUID [`uuid_suffixed_id_is_well_formed`] reads, the form AWS issues one in
/// (`sfr-73fbd2ce-aa30-494c-8788-1cee4ffcbc91`) and the form
/// [`ComAmazonawsEc2Amazonec2State::mint_spot_fleet_request_id`] mints.
pub fn spot_fleet_request_id_is_well_formed(id: &str) -> bool {
    uuid_suffixed_id_is_well_formed(id, SPOT_FLEET_REQUEST_ID_PREFIX)
}

/// The message EC2 refuses something that is not a Spot Fleet request id at all with, and the
/// one the per-fleet [`CANCEL_BATCH_FLEET_REQUEST_ID_MALFORMED`] carries: one definition, so a
/// request-level refusal and a per-fleet error cannot word it two ways.
pub fn spot_fleet_request_id_malformed_message(request_id: &str) -> String {
    invalid_id_message(request_id)
}

/// The message the per-fleet [`CANCEL_BATCH_FLEET_REQUEST_ID_DOES_NOT_EXIST`] carries: AWS's
/// own sentence for a cancellation that could not find the fleet it was asked to cancel.
///
/// FIXED text, with no variable part: which fleet the entry is about is carried by the entry's
/// own `SpotFleetRequestId` member, so the sentence does not name it. A REQUEST-level refusal
/// words the same fact differently, because it names every id the request could not resolve at
/// once — see [`spot_fleet_request_ids_not_found_message`].
pub const SPOT_FLEET_REQUEST_CANCELLATION_NOT_FOUND_MESSAGE: &str =
    "Spot Fleet Request cancellation failed. Spot Fleet Request not found.";

/// The message EC2 refuses a REQUEST that named Spot Fleet request ids this account holds no
/// fleet for with: one sentence naming every unresolved id, in the order the request gave
/// them, so an operation that takes one id and one that takes a list word it the same way.
///
/// Recorded in the wire corpus for a `DescribeSpotFleetInstances` naming a Spot Fleet request
/// id of no fleet, which AWS answers
/// `InvalidSpotFleetRequestId.NotFound` with. Each id is named through
/// [`echoed_resource_id`], the one bound this crate puts on a refusal that quotes a value the
/// caller chose.
pub fn spot_fleet_request_ids_not_found_message(request_ids: &[&str]) -> String {
    let named: Vec<String> = request_ids
        .iter()
        .map(|request_id| echoed_resource_id(request_id))
        .collect();
    format!(
        "The SpotFleetRequestId(s) {} do not exist.",
        named.join(", ")
    )
}

/// The message the per-fleet [`CANCEL_BATCH_FLEET_REQUEST_NOT_IN_CANCELLABLE_STATE`] carries:
/// one sentence, with the fleet from the request and the `BatchState` from the store.
pub fn spot_fleet_request_not_cancellable_message(request_id: &str, state: &str) -> String {
    format!("The spot fleet request '{request_id}' is in the state '{state}' and cannot be canceled")
}

/// Refuse something that is not a Spot Fleet request id at all: HTTP 400
/// [`INVALID_SPOT_FLEET_REQUEST_ID_MALFORMED`], answered BEFORE any lookup so a caller can
/// tell a typo from a fleet the account does not hold.
pub fn invalid_spot_fleet_request_id_malformed<E>(request_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SPOT_FLEET_REQUEST_ID_MALFORMED,
        400,
        spot_fleet_request_id_malformed_message(request_id),
    )
}

/// Refuse a request that named well-formed Spot Fleet request ids this account holds no fleet
/// for: HTTP 400 [`INVALID_SPOT_FLEET_REQUEST_ID_NOT_FOUND`], naming every one of them
/// ([`spot_fleet_request_ids_not_found_message`]).
pub fn invalid_spot_fleet_request_ids_not_found<E>(request_ids: &[&str]) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SPOT_FLEET_REQUEST_ID_NOT_FOUND,
        400,
        spot_fleet_request_ids_not_found_message(request_ids),
    )
}

/// Refuse a well-formed Spot Fleet request id this account holds no fleet for: the
/// request-level refusal [`invalid_spot_fleet_request_ids_not_found`] makes, for an operation
/// that names ONE fleet.
pub fn invalid_spot_fleet_request_id_not_found<E>(request_id: &str) -> Scaffolded<E> {
    invalid_spot_fleet_request_ids_not_found(&[request_id])
}

/// Refuse `request_id` unless it is a well-formed Spot Fleet request id, BEFORE any lookup:
/// the question "is this a Spot Fleet request id at all" is asked ahead of "does this account
/// hold it", so "not an id of this kind" is never reported as "no such fleet".
///
/// A handler calls this to refuse a malformed id WITHOUT taking the state lock;
/// [`require_known_spot_fleet_request`] asks it again, so the ordering holds even for a caller
/// that goes straight to the lookup.
pub fn require_well_formed_spot_fleet_request_id<E>(
    request_id: &str,
) -> Result<(), Scaffolded<E>> {
    if spot_fleet_request_id_is_well_formed(request_id) {
        return Ok(());
    }
    Err(invalid_spot_fleet_request_id_malformed(request_id))
}

/// The Spot Fleet request `request_id` names, refusing a malformed id first and an id this
/// account holds no fleet for second — the order every named-resource lookup of this module
/// establishes.
pub fn require_known_spot_fleet_request<'a, E>(
    store: &'a ComAmazonawsEc2Amazonec2State,
    request_id: &str,
) -> Result<&'a SpotFleetRequestRecord, Scaffolded<E>> {
    require_well_formed_spot_fleet_request_id(request_id)?;
    store
        .spot_fleet_requests
        .get(request_id)
        .ok_or_else(|| invalid_spot_fleet_request_id_not_found(request_id))
}

/// The move the clock owes one Spot Fleet request: a [`ClockTransition`] to the `BatchState`
/// member it becomes, as a `&'static str` so only a word from this module's own catalog
/// constants can be stored — never a request's text.
pub type SpotFleetRequestTransition = ClockTransition<&'static str>;

/// One Spot Instance request a Spot Fleet placed to cover part of its target capacity.
///
/// The request ITSELF is a record of
/// [`ComAmazonawsEc2Amazonec2State::spot_instance_requests`], named here by id: the fleet
/// keeps only what is true of the request *as the fleet's* — how many capacity units the
/// instance it launches provides — so the state the fleet derives from a request is always
/// the one the Spot Instance request store holds.
#[derive(Debug, Clone)]
pub struct SpotFleetPlacement {
    /// The `SpotInstanceRequestId`, a key of
    /// [`ComAmazonawsEc2Amazonec2State::spot_instance_requests`].
    pub spot_instance_request_id: String,
    /// "The number of units provided by the specified instance type"
    /// (`SpotFleetLaunchSpecification$WeightedCapacity`) the launched instance provides.
    pub weighted_capacity: f64,
}

/// One stored Spot Fleet request: the modelled [`SpotFleetRequestConfig`]
/// `DescribeSpotFleetRequests` reports, plus the two facts that shape carries no member for.
///
/// `RequestSpotFleet` answers the id of the view stored here and `DescribeSpotFleetRequests`
/// reports the view itself, so the two cannot describe one fleet differently.
#[derive(Debug, Clone)]
pub struct SpotFleetRequestRecord {
    /// The fleet as the model shapes it, carrying the state reached so far and the capacity
    /// its placements have fulfilled.
    pub view: SpotFleetRequestConfig,
    /// The move the clock owes this fleet, while one is owed.
    pub pending: Option<SpotFleetRequestTransition>,
    /// The `EventSubType` the move the clock owes RECORDS, when the state word it enters is
    /// not the sub type AWS publishes for the change — a modification completing is
    /// `modify_succeeded` while the state it leaves the fleet in is `active`
    /// ([`EVENT_SUB_TYPE_MODIFY_SUCCEEDED`]). `None` for every move whose sub type IS the
    /// state word, which is what the `fleetRequestChange` list of
    /// `EventInformation$EventSubType` names for them.
    pub pending_event_sub_type: Option<&'static str>,
    /// The Spot Instance requests this fleet placed, in placement order.
    pub placements: Vec<SpotFleetPlacement>,
    /// The Spot pools this fleet places its Spot capacity in, as the write that stored the
    /// fleet resolved them from its launch specifications or its launch template
    /// configurations — with no count of their own, which is a fact of one placement round
    /// rather than of the pool.
    ///
    /// Kept because a fleet whose target capacity is INCREASED places the additional capacity
    /// in the same pools ("the Spot Fleet launches the additional Spot Instances according to
    /// the allocation strategy for the Spot Fleet request", `ModifySpotFleetRequest`), so the
    /// launch a later placement makes is the one the fleet was described with rather than one
    /// resolved a second way.
    pub pools: Vec<SpotFleetPoolPlacement>,
    /// The `SpotInstanceType` every Spot Instance request of this fleet is placed as, out of
    /// [`SPOT_INSTANCE_TYPES`] — a fact of the fleet's own `Type`, kept so a later placement
    /// places the same kind of request as the first.
    pub spot_request_type: &'static str,
    /// The interruption behaviour every Spot Instance request of this fleet carries, out of
    /// [`crate::ops::types::INSTANCE_INTERRUPTION_BEHAVIOR_VALUES`], kept for the same reason.
    pub interruption_behavior: &'static str,
    /// The events this fleet has been through, in the order they were recorded — what
    /// `DescribeSpotFleetRequestHistory` reports. Each event ([`SpotFleetHistoryEvent`]) is
    /// recorded at the instant it happened, by the writer that made it happen, so a report of
    /// the history derives nothing and writes nothing.
    pub history: Vec<SpotFleetHistoryEvent>,
    /// The Region the request was signed for, which the instances its placements launch need
    /// for their private DNS names. Not a member of any modelled Spot Fleet shape.
    pub region: String,
}

impl SpotFleetRequestRecord {
    /// This fleet's own `SpotFleetRequestId`, which is also the key the store holds it under.
    pub fn spot_fleet_request_id(&self) -> &str {
        self.view.spot_fleet_request_id.as_deref().unwrap_or_default()
    }

    /// This fleet's `BatchState` as it was last reported.
    pub fn state(&self) -> &str {
        self.view
            .spot_fleet_request_state
            .as_deref()
            .unwrap_or_default()
    }

    /// The fleet's configuration, as the request gave it and this module normalised it.
    pub fn config(&self) -> Option<&SpotFleetRequestConfigData> {
        self.view.spot_fleet_request_config.as_ref()
    }

    /// "The number of units to request for the Spot Fleet"
    /// (`SpotFleetRequestConfigData$TargetCapacity`).
    pub fn target_capacity(&self) -> f64 {
        self.config()
            .map_or(0.0, |config| f64::from(config.target_capacity))
    }

    /// Apply what the clock owes this fleet: the state move that has fallen due, and the
    /// capacity its placements have fulfilled by now.
    ///
    /// Both are owed to the CLOCK rather than to a caller — the move was due at this instant
    /// whether or not anybody called, and the fulfilled capacity is derived from the Spot
    /// Instance requests the fleet placed — so a reporting operation may apply them.
    ///
    /// `running_instances` is whether any Spot Instance this fleet launched is still held by
    /// the instance store ([`spot_fleet_has_running_instances`]), which is what a CANCELLED
    /// fleet's completion waits for: a cancelled fleet is `cancelled` once it has no running
    /// Spot Instance left, so its move is due to the clock AND to those instances, while every
    /// other move is due to the clock alone.
    pub(super) fn apply_due_move(&mut self, now: Instant, fulfilled: f64, running_instances: bool) {
        let due = self
            .pending
            .as_ref()
            .filter(|pending| now >= pending.due_at)
            .filter(|pending| pending.to != SPOT_FLEET_STATE_CANCELLED || !running_instances)
            .map(|pending| (pending.to, pending.at));
        if let Some((to, at)) = due {
            self.view.spot_fleet_request_state = Some(to.to_string());
            self.pending = None;
            // "A change in the status or configuration of the Spot Fleet request" is an event
            // of the fleet's history, recorded at the instant the change FELL DUE rather than
            // the instant somebody read it — the state changed when the move fell due. The sub
            // type is the state word for every move that publishes one, and the word carried
            // beside the move for the one that does not ([`Self::pending_event_sub_type`]).
            let event_sub_type = self.pending_event_sub_type.take().unwrap_or(to);
            self.history
                .push(SpotFleetHistoryEvent::fleet_request_change(
                    event_sub_type,
                    at,
                ));
        }
        let target = self.target_capacity();
        let on_demand = self
            .config()
            .and_then(|config| config.on_demand_fulfilled_capacity)
            .unwrap_or_default();
        if let Some(config) = self.view.spot_fleet_request_config.as_mut() {
            config.fulfilled_capacity = Some(fulfilled);
        }
        // "If the size of the fleet is decreased, the status is `pending_termination` while
        // Spot Instances are terminating" — which is where a fleet cancelled WITH its
        // instances sits until they are gone.
        let terminating = self.state() == SPOT_FLEET_STATE_CANCELLED_TERMINATING;
        // "If the size of the fleet is equal to or greater than its target capacity, the
        // status is `fulfilled`" — and until then the requests are placed and waiting, which
        // is `pending_fulfillment`.
        self.view.activity_status = Some(
            if terminating && running_instances {
                SPOT_FLEET_ACTIVITY_PENDING_TERMINATION
            } else if fulfilled + on_demand >= target {
                SPOT_FLEET_ACTIVITY_FULFILLED
            } else {
                SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT
            }
            .to_string(),
        );
    }
}

/// The capacity one Spot Fleet's `placements` have FULFILLED: the weighted capacity of every
/// placement whose Spot Instance request launched an instance the account still RUNS.
///
/// The ONE definition of "`FulfilledCapacity`": "The number of units fulfilled by this
/// request compared to the set target capacity. You cannot set this value"
/// (`SpotFleetRequestConfigData$FulfilledCapacity`) — so it is derived from the instances the
/// fleet's requests actually launched, never taken from a request and never counted for a
/// request that has launched nothing.
///
/// Those instances are exactly the ones the fleet REPORTS ([`spot_fleet_active_instances`]):
/// an instance that has reached `terminated`, and one the store no longer holds at all,
/// belongs to no fleet, so it fulfils no capacity either — which is what makes a fleet whose
/// capacity was terminated report the capacity it is left with rather than the one it once
/// had.
pub fn spot_fleet_fulfilled_capacity(
    placements: &[SpotFleetPlacement],
    requests: &BTreeMap<String, SpotInstanceRequestRecord>,
    instances: &HashMap<String, InstanceRecord>,
) -> f64 {
    placements
        .iter()
        .filter(|placement| {
            requests
                .get(&placement.spot_instance_request_id)
                .and_then(|record| record.view.instance_id.as_deref())
                .and_then(|instance_id| instances.get(instance_id))
                .is_some_and(|instance| instance.state != InstanceStateName::Terminated)
        })
        .map(|placement| placement.weighted_capacity)
        // Folded from a POSITIVE zero rather than summed: `Sum` for `f64` folds from a
        // negative zero, so a fleet that has launched nothing would report a capacity of
        // `-0.0` — not a value a capacity takes, nor one AWS writes.
        .fold(0.0, |fulfilled, capacity| fulfilled + capacity)
}

/// Every Spot Instance one Spot Fleet's `placements` launched, in placement order: the
/// `InstanceId` each placed Spot Instance request reports, for the requests that have one.
///
/// The ONE way the instances of a fleet are reached, so the cancellation that terminates them
/// and the derivation that waits for them read one set.
pub fn spot_fleet_instance_ids(
    placements: &[SpotFleetPlacement],
    requests: &BTreeMap<String, SpotInstanceRequestRecord>,
) -> Vec<String> {
    placements
        .iter()
        .filter_map(|placement| {
            requests
                .get(&placement.spot_instance_request_id)
                .and_then(|record| record.view.instance_id.clone())
        })
        .collect()
}

/// Whether any Spot Instance one Spot Fleet's `placements` launched is STILL RUNNING: an
/// instance this account's instance store holds that has not reached `terminated`.
///
/// What a cancelled fleet's completion waits for — a cancelled Spot Fleet request is
/// `cancelled` once it has no running Spot Instance — and what tells a fleet whose instances
/// are terminating from one whose instances are gone.
pub fn spot_fleet_has_running_instances(
    placements: &[SpotFleetPlacement],
    requests: &BTreeMap<String, SpotInstanceRequestRecord>,
    instances: &HashMap<String, InstanceRecord>,
) -> bool {
    spot_fleet_instance_ids(placements, requests)
        .iter()
        .any(|instance_id| {
            instances
                .get(instance_id)
                .is_some_and(|instance| instance.state != InstanceStateName::Terminated)
        })
}

/// The instances of one Spot Fleet request, as the modelled
/// [`ActiveInstance`](crate::ops::types::ActiveInstance) shape.
///
/// "Describes the running instances for the specified Spot Fleet"
/// (`DescribeSpotFleetInstances`): the instances the fleet is running for the account, which
/// are the ones its placements launched and the instance store still holds un-terminated —
/// the SAME set whose capacity the fleet reports as fulfilled
/// ([`spot_fleet_fulfilled_capacity`]), so a fleet that reports its target capacity fulfilled
/// reports the instances that fulfil it. An instance the account has terminated, and one the
/// store no longer holds at all, is not one of them.
///
/// Reached through the Spot Instance requests the fleet placed ([`spot_fleet_instance_ids`] is
/// the same walk for the ids alone) and projected from the INSTANCE store by the one
/// projection both kinds of fleet report through ([`active_instances`]), so the type reported
/// for an instance here is the one `DescribeInstances` reports for it. The caller sees them in
/// placement order; a report that pages puts them in id order itself.
pub fn spot_fleet_active_instances<'a>(
    store: &'a ComAmazonawsEc2Amazonec2State,
    record: &'a SpotFleetRequestRecord,
) -> Vec<crate::ops::types::ActiveInstance> {
    active_instances(
        store,
        record.placements.iter().filter_map(|placement| {
            let instance_id = store
                .spot_instance_requests
                .get(&placement.spot_instance_request_id)?
                .view
                .instance_id
                .as_deref()?;
            Some((
                instance_id,
                Some(placement.spot_instance_request_id.as_str()),
            ))
        }),
    )
}

/// How many Spot Instances each pool of a Spot Fleet places to cover `spot_target` capacity
/// units, given each pool's weighted capacity, never more than `cap` instances in total.
///
/// "By default, the Spot Fleet requests Spot Instances in the Spot Instance pool where the
/// price per unit is the lowest", and "alternatively, you can specify that the Spot Fleet
/// distribute the target capacity across the Spot pools included in its launch
/// specifications" (`RequestSpotFleet`) — so a `diversified` fleet splits the target evenly
/// across its pools and every other strategy places it all on one pool. This emulator holds
/// no Spot price history, so the lowest-priced pool cannot be told from the others: the one
/// pool is the FIRST the request gave, which is the order every other member of the
/// specification is read in.
///
/// How many instances a pool's share takes is that share divided by what one instance of the
/// pool provides, rounded UP, because an instance is not launched in
/// fractions — the same arithmetic a Capacity Reservation Fleet's instance count is made by.
/// `cap` is what bounds the whole computation: a target capacity the model leaves unbounded
/// can ask for more instances than this store will ever place, and the pools then place `cap`
/// of them between them.
pub fn spot_fleet_placement_counts(
    weights: &[f64],
    spot_target: f64,
    diversified: bool,
    cap: usize,
) -> Vec<usize> {
    let mut counts = vec![0usize; weights.len()];
    if weights.is_empty() || !spot_target.is_finite() || spot_target <= 0.0 || cap == 0 {
        return counts;
    }
    let pools = if diversified { weights.len() } else { 1 };
    // `pools` is at least 1 here, so the share is finite and positive.
    let share = spot_target / pools as f64;
    let mut placed = 0usize;
    for (count, weight) in counts.iter_mut().zip(weights).take(pools) {
        let weight = if weight.is_finite() && *weight > 0.0 {
            *weight
        } else {
            DEFAULT_FLEET_INSTANCE_WEIGHT
        };
        let needed = (share / weight).ceil();
        let needed = if !needed.is_finite() || needed <= 0.0 {
            0
        } else if needed >= cap as f64 {
            cap
        } else {
            // Inside `cap` and integral after `ceil`, so this conversion is exact.
            needed as usize
        };
        *count = needed.min(cap.saturating_sub(placed));
        placed = placed.saturating_add(*count);
    }
    counts
}

/// One pool of a Spot Fleet as a writer has already resolved it: the launch the fleet places
/// in that pool, what one of its instances provides, and how many of them to place.
#[derive(Debug, Clone)]
pub struct SpotFleetPoolPlacement {
    /// The launch every Spot Instance request of this pool carries, as the modelled
    /// [`LaunchSpecification`] a Spot Instance request reports — resolved from the fleet's
    /// own launch specification or from the launch template it names, so the store holds one
    /// shape however the fleet described the launch.
    pub launch: LaunchSpecification,
    /// "The number of units provided by the specified instance type"
    /// (`SpotFleetLaunchSpecification$WeightedCapacity`), defaulted to
    /// [`DEFAULT_FLEET_INSTANCE_WEIGHT`].
    pub weighted_capacity: f64,
    /// "The maximum price per unit hour that you are willing to pay for a Spot Instance" of
    /// this pool, or the fleet's own maximum when the pool named none.
    pub spot_price: Option<String>,
    /// How many Spot Instance requests this pool places
    /// ([`spot_fleet_placement_counts`]).
    pub count: usize,
}

/// What a writer must say to store a Spot Fleet request; everything else about the record —
/// the fleet's id, the state and activity status it starts in, the Spot Instance requests it
/// places and the bytes it charges — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_spot_fleet_request`].
#[derive(Debug, Clone)]
pub struct SpotFleetRequestSpec {
    /// The Region the request was signed for.
    pub region: String,
    /// The configuration as the request gave it, with the documented defaults applied and
    /// the members a caller cannot set ([`SpotFleetRequestConfigData::fulfilled_capacity`])
    /// left for the store to derive.
    pub config: SpotFleetRequestConfigData,
    /// The tags the request's `TagSpecifications` assign to the fleet.
    pub tags: Vec<Tag>,
    /// The pools the fleet places its Spot capacity in.
    pub pools: Vec<SpotFleetPoolPlacement>,
    /// The `SpotInstanceType` every Spot Instance request of this fleet is placed as, out of
    /// [`SPOT_INSTANCE_TYPES`]: a fleet that maintains its capacity places requests that
    /// outlive their instances.
    pub spot_request_type: &'static str,
    /// The interruption behaviour every Spot Instance request of this fleet carries, out of
    /// [`crate::ops::types::INSTANCE_INTERRUPTION_BEHAVIOR_VALUES`].
    pub interruption_behavior: &'static str,
}

/// The caller-chosen bytes one stored Spot Fleet request holds: the strings of its
/// configuration and its tags.
///
/// The ONE definition of that charge: `RequestSpotFleet` asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_spot_fleet_request`] charges exactly it. The Spot
/// Instance requests the fleet places charge their own bytes through
/// [`spot_instance_request_payload_bytes`], the one definition of THAT charge, so no path
/// stores caller-chosen bytes uncharged.
pub fn spot_fleet_request_payload_bytes(
    config: &SpotFleetRequestConfigData,
    tags: &[Tag],
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let launch_bytes: usize = config
        .launch_specifications
        .iter()
        .flatten()
        .map(|launch| {
            text(&launch.image_id)
                .saturating_add(text(&launch.instance_type))
                .saturating_add(text(&launch.key_name))
                .saturating_add(text(&launch.subnet_id))
                .saturating_add(text(&launch.user_data))
                .saturating_add(text(&launch.kernel_id))
                .saturating_add(text(&launch.ramdisk_id))
                .saturating_add(text(&launch.addressing_type))
                .saturating_add(text(&launch.spot_price))
                .saturating_add(
                    launch
                        .placement
                        .as_ref()
                        .map_or(0, |placement| text(&placement.availability_zone)),
                )
                .saturating_add(
                    launch
                        .security_groups
                        .iter()
                        .flatten()
                        .map(|group| text(&group.group_id).saturating_add(text(&group.group_name)))
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    let template_bytes: usize = config
        .launch_template_configs
        .iter()
        .flatten()
        .map(|entry| {
            let specification = entry.launch_template_specification.as_ref().map_or(0, |spec| {
                text(&spec.launch_template_id)
                    .saturating_add(text(&spec.launch_template_name))
                    .saturating_add(text(&spec.version))
            });
            let overrides = entry
                .overrides
                .iter()
                .flatten()
                .map(|over| {
                    text(&over.instance_type)
                        .saturating_add(text(&over.subnet_id))
                        .saturating_add(text(&over.availability_zone))
                        .saturating_add(text(&over.availability_zone_id))
                        .saturating_add(text(&over.spot_price))
                })
                .fold(0usize, usize::saturating_add);
            specification.saturating_add(overrides)
        })
        .fold(0usize, usize::saturating_add);
    config
        .iam_fleet_role
        .len()
        .saturating_add(text(&config.client_token))
        .saturating_add(text(&config.context))
        .saturating_add(text(&config.spot_price))
        .saturating_add(text(&config.spot_max_total_price))
        .saturating_add(text(&config.on_demand_max_total_price))
        .saturating_add(launch_bytes)
        .saturating_add(template_bytes)
        .saturating_add(tag_list_bytes(tags))
}

impl ComAmazonawsEc2Amazonec2State {
    /// A fresh, unused `SpotFleetRequestId`.
    ///
    /// The form [`spot_fleet_request_id_is_well_formed`] reads, minted by the one helper that
    /// renders this store's monotonic counter into a UUID
    /// ([`Self::mint_uuid_suffixed_id`]), so the id is unique without being drawn from a
    /// request and is never the null UUID.
    pub fn mint_spot_fleet_request_id(&mut self) -> String {
        self.mint_uuid_suffixed_id(SPOT_FLEET_REQUEST_ID_PREFIX)
    }

    /// Whether another Spot Fleet request fits inside the store's ceiling
    /// ([`MAX_SPOT_FLEET_REQUESTS`]).
    ///
    /// The same contract as [`Self::has_capacity_reservation_fleet_capacity`]: a writer asks
    /// first and refuses instead of growing the store past its ceiling.
    pub fn has_spot_fleet_request_capacity(&self) -> bool {
        self.spot_fleet_requests.len() < MAX_SPOT_FLEET_REQUESTS
    }

    /// Store one Spot Fleet request, placing the Spot Instance requests its target capacity
    /// takes, and answer it as it was stored.
    ///
    /// The one way a Spot Fleet request is stored, so the id it carries, the state and
    /// activity status it starts in, the requests it places and the bytes it charges cannot
    /// be built two ways.
    ///
    /// The CALL places the requests — "the Spot Fleet only places the required requests"
    /// (`SpotFleetRequestConfigData$Type`) — so they are facts of this write; what the fleet
    /// still owes the clock is its own STATE word (`submitted` -> `active`) and the capacity
    /// those requests fulfil, which falls due when the requests themselves launch their
    /// instances. Every placed request is an ordinary Spot Instance request of this account,
    /// fulfilled by the one clock-owed move every Spot Instance request has, so a fleet
    /// cannot report an instance the instance store does not hold.
    pub fn insert_spot_fleet_request(&mut self, spec: SpotFleetRequestSpec) -> SpotFleetRequestConfig {
        let fleet_id = self.mint_spot_fleet_request_id();
        let created_at = SystemTime::now();
        let placements = self.place_spot_fleet_capacity(&spec);
        let mut config = spec.config;
        // "The number of units fulfilled by this request compared to the set target
        // capacity. You cannot set this value" (`$FulfilledCapacity`): both are derived from
        // the placements, so whatever the request carried is replaced here. A fleet stored
        // through this writer holds no On-Demand capacity — `RequestSpotFleet` refuses a
        // request that asks for any, because an On-Demand Instance is launched by the fleet
        // directly rather than placed through a Spot Instance request — so none is reported
        // fulfilled either.
        config.fulfilled_capacity = Some(0.0);
        config.on_demand_fulfilled_capacity = Some(0.0);
        // The tags are reported on the fleet itself (`SpotFleetRequestConfig$Tags`), so the
        // specification that carried them is not echoed back as part of the configuration.
        config.tag_specifications = None;
        self.add_stored_bytes(
            spot_fleet_request_payload_bytes(&config, &spec.tags)
                .saturating_add(STORED_RECORD_OVERHEAD_BYTES),
        );
        let view = SpotFleetRequestConfig {
            // The requests have been placed by this call, so the fleet is waiting for them
            // to be fulfilled; the derivation below is what moves it on.
            activity_status: Some(SPOT_FLEET_ACTIVITY_PENDING_FULFILLMENT.to_string()),
            create_time: Some(system_time_to_date_time(created_at)),
            spot_fleet_request_config: Some(config),
            spot_fleet_request_id: Some(fleet_id.clone()),
            spot_fleet_request_state: Some(SPOT_FLEET_STATE_SUBMITTED.to_string()),
            tags: Some(spec.tags),
        };
        self.spot_fleet_requests.insert(
            fleet_id.clone(),
            SpotFleetRequestRecord {
                view,
                pending: Some(SpotFleetRequestTransition::starting_now(
                    SPOT_FLEET_STATE_ACTIVE,
                )),
                // `active` IS the `fleetRequestChange` sub type AWS publishes for that move,
                // so the move carries no sub type of its own.
                pending_event_sub_type: None,
                placements,
                // The pools with no count of their own: how many each placed is a fact of this
                // placement round, while the pools themselves are where the fleet places
                // capacity for as long as it holds.
                pools: spec
                    .pools
                    .iter()
                    .map(|pool| SpotFleetPoolPlacement { count: 0, ..pool.clone() })
                    .collect(),
                spot_request_type: spec.spot_request_type,
                interruption_behavior: spec.interruption_behavior,
                // The first event of every fleet's history: "`submitted` - the EC2 Fleet or
                // Spot Fleet request is being evaluated and Amazon EC2 is preparing to launch
                // the target number of instances" (`EventInformation$EventSubType`), which is
                // the state this write puts the fleet in, at the instant it was created.
                history: vec![SpotFleetHistoryEvent::fleet_request_change(
                    SPOT_FLEET_STATE_SUBMITTED,
                    created_at,
                )],
                region: spec.region,
            },
        );
        // Read back through the one derivation of the fulfilled capacity and the activity
        // status, so the create answers exactly what a describe in the same instant reports.
        self.materialize_due_spot_fleet_requests(Instant::now());
        self.spot_fleet_requests
            .get(&fleet_id)
            .map(|record| record.view.clone())
            .unwrap_or_default()
    }

    /// Place the Spot Instance requests one Spot Fleet's Spot capacity takes, and answer what
    /// was placed.
    ///
    /// "Amazon EC2 calculates the difference between the total capacity and On-Demand
    /// capacity, and launches the difference as Spot capacity" (`RequestSpotFleet`), so the
    /// capacity placed here is the fleet's target less its On-Demand target. Each request is
    /// stored through [`Self::insert_spot_instance_request`], the one writer of a Spot
    /// Instance request, so each is fulfilled into an instance by the clock exactly as a
    /// request a caller placed itself would be.
    pub(super) fn place_spot_fleet_capacity(&mut self, spec: &SpotFleetRequestSpec) -> Vec<SpotFleetPlacement> {
        let created_at = SystemTime::now();
        // "The end date and time of the request … After the end date and time, no new Spot
        // Instance requests are placed or able to fulfill the request. If no value is
        // specified, the Spot Fleet request remains until you cancel it"
        // (`SpotFleetRequestConfigData$ValidUntil`): a fleet with no end date places requests
        // that do not expire on their own, so their validity runs to this module's clock
        // horizon rather than to the seven days a bare `RequestSpotInstances` call gets.
        let valid_until = spec.config.valid_until;
        let valid_until_at = valid_until.map_or_else(
            || due_instant(MAX_CLOCK_HORIZON),
            |at| due_instant(duration_until(date_time_to_system_time(at))),
        );
        // "By default, Amazon EC2 starts fulfilling the request immediately"
        // (`$ValidFrom`), and a start date in the future is when the fulfillment falls due
        // instead — never sooner than the span this crate gives every clock-owed move, so a
        // caller reading the fleet straight back still sees it waiting.
        let until_fulfilled = spec
            .config
            .valid_from
            .map(date_time_to_system_time)
            .map_or(TRANSITION_DELAY, duration_until)
            .max(TRANSITION_DELAY);
        let mut placements: Vec<SpotFleetPlacement> = Vec::new();
        for pool in &spec.pools {
            let product_description = self
                .spot_product_description(pool.launch.image_id.as_deref())
                .to_string();
            for _ in 0..pool.count {
                let view = SpotInstanceRequest {
                    spot_instance_request_id: None,
                    state: Some(SPOT_REQUEST_STATE_OPEN.to_string()),
                    status: Some(SpotInstanceStatus {
                        code: Some(SPOT_STATUS_PENDING_EVALUATION.to_string()),
                        message: Some(SPOT_STATUS_MESSAGE_PENDING_EVALUATION.to_string()),
                        update_time: Some(system_time_to_date_time(created_at)),
                    }),
                    create_time: Some(system_time_to_date_time(created_at)),
                    r#type: Some(spec.spot_request_type.to_string()),
                    instance_interruption_behavior: Some(spec.interruption_behavior.to_string()),
                    valid_from: spec.config.valid_from,
                    valid_until,
                    // A fleet's requests are launched and terminated as the fleet decides,
                    // not as a launch group: neither member is a fleet's to set.
                    launch_group: None,
                    availability_zone_group: None,
                    spot_price: pool.spot_price.clone(),
                    product_description: Some(product_description.clone()),
                    launch_specification: Some(pool.launch.clone()),
                    // The fleet's tags tag the FLEET; nothing tags the request it placed.
                    tags: Some(Vec::new()),
                    instance_id: None,
                    launched_availability_zone: None,
                    launched_availability_zone_id: None,
                    // "Deprecated" (`SpotInstanceRequest$BlockDurationMinutes` and
                    // `ActualBlockHourlyPrice`): Spot blocks are not offered any more.
                    block_duration_minutes: None,
                    actual_block_hourly_price: None,
                    fault: None,
                };
                let spot_instance_request_id = self.insert_spot_instance_request(
                    view,
                    Some(SpotTransition::due_in(until_fulfilled)),
                    valid_until_at,
                    &spec.region,
                    None,
                    // A Spot Fleet's pools are read out of launch specifications this path
                    // takes no instance profile from, so the instances they place run as none.
                    None,
                );
                placements.push(SpotFleetPlacement {
                    spot_instance_request_id,
                    weighted_capacity: pool.weighted_capacity,
                });
            }
        }
        placements
    }

    /// Cancel the Spot Fleet request `fleet_id` names, and answer the `BatchState` it was in
    /// and the one it is in now — `None` when this store holds no such fleet or it is in a
    /// state it cannot be cancelled from ([`spot_fleet_request_state_is_cancellable`]).
    ///
    /// The one way a Spot Fleet request is cancelled, so what a cancellation does cannot be
    /// built two ways:
    ///
    /// * "after you cancel a Spot Fleet request, the Spot Fleet launches no new instances"
    ///   (`CancelSpotFleetRequests`) — so every Spot Instance request the fleet placed is
    ///   cancelled and owes no fulfillment, which is what stops the next clock pass launching
    ///   an instance for it;
    /// * "if you choose to terminate the instances, the Spot Fleet request enters the
    ///   `cancelled_terminating` state. Otherwise, the Spot Fleet request enters the
    ///   `cancelled_running` state and the instances continue to run until they are
    ///   interrupted or you terminate them manually" — so `terminate_instances` decides both
    ///   the state the fleet enters and whether the instances it launched are terminated;
    /// * a cancelled fleet reaches `cancelled` once it has no running Spot Instance left,
    ///   which is owed to the clock and to those instances
    ///   ([`SpotFleetRequestRecord::apply_due_move`]);
    /// * a cancellation that asks for nothing the fleet has not already done CHANGES NOTHING,
    ///   and so records nothing: a fleet already in `cancelled_running` has already stopped
    ///   placing capacity and already left its instances running, so there is no "change in the
    ///   status or configuration of the Spot Fleet request" to make an event of
    ///   ([`crate::state::EVENT_TYPE_FLEET_REQUEST_CHANGE`]). The call is still accepted — it
    ///   asked for a state the fleet is in — and it reports that state as both the previous and
    ///   the current one. That is what keeps one fleet's history bounded by the states it
    ///   ENTERS however often a caller repeats the call.
    pub fn cancel_spot_fleet_request(
        &mut self,
        fleet_id: &str,
        terminate_instances: bool,
    ) -> Option<(String, String)> {
        let record = self.spot_fleet_requests.get(fleet_id)?;
        let previous_state = record.state().to_string();
        if !spot_fleet_request_state_is_cancellable(&previous_state) {
            return None;
        }
        // The repeat that moves nothing: the fleet is in the very state this call would put it
        // in, so nothing is written — no event, no move the clock owes, and no second
        // cancellation of the Spot Instance requests it placed.
        if previous_state == SPOT_FLEET_STATE_CANCELLED_RUNNING && !terminate_instances {
            return Some((previous_state.clone(), previous_state));
        }
        let placements = record.placements.clone();

        let at = SystemTime::now();
        let current_state = if terminate_instances {
            SPOT_FLEET_STATE_CANCELLED_TERMINATING
        } else {
            SPOT_FLEET_STATE_CANCELLED_RUNNING
        };
        // The cancellation is itself "a change in the status or configuration of the Spot Fleet
        // request", so it is one event of the fleet's history, at the instant the cancellation
        // was asked for — recorded BEFORE the instances it terminates record theirs, because
        // the request was cancelled first and they are terminated because it was.
        self.record_spot_fleet_history_event(
            fleet_id,
            SpotFleetHistoryEvent::fleet_request_change(current_state, at),
        );
        for placement in &placements {
            // Each request the fleet placed is cancelled through the ONE writer of a Spot
            // Instance request's cancellation, so a request cancelled with its fleet and one
            // cancelled on its own end the same way — and the fleet places no new instance
            // for a request it has cancelled.
            self.cancel_spot_instance_request(
                &placement.spot_instance_request_id,
                terminate_instances,
                at,
            );
        }
        if terminate_instances {
            // "Terminating an instance is permanent and irreversible": the instances the fleet
            // launched, read through the ONE way a fleet's instances are reached.
            for instance_id in spot_fleet_instance_ids(&placements, &self.spot_instance_requests) {
                self.begin_instance_termination(&instance_id, at);
            }
        }

        let record = self.spot_fleet_requests.get_mut(fleet_id)?;
        record.view.spot_fleet_request_state = Some(current_state.to_string());
        // The `active` the clock owed a `submitted` fleet is not owed a cancelled one; what it
        // owes now is the `cancelled` the fleet reaches when its instances are gone.
        record.pending = Some(SpotFleetRequestTransition::starting_now(
            SPOT_FLEET_STATE_CANCELLED,
        ));
        // `cancelled` IS the sub type that move records, so a modification's
        // `modify_succeeded` is not left owed on a fleet that is now being cancelled.
        record.pending_event_sub_type = None;
        // Read the activity status back through the one derivation of it, so the cancellation
        // reports what a describe in the same instant reports.
        self.materialize_due_spot_fleet_requests(Instant::now());
        let current_state = self
            .spot_fleet_requests
            .get(fleet_id)
            .map_or_else(|| current_state.to_string(), |record| record.state().to_string());
        Some((previous_state, current_state))
    }

    /// Apply what the clock owes every stored Spot Fleet request, as of `now`.
    ///
    /// Called by [`Self::materialize_due_transitions`] AFTER the Spot Instance requests have
    /// been moved, which is the one place every owed move is applied from: a fleet's
    /// fulfilled capacity is derived from the requests it placed, so those have to be in the
    /// state the caller is about to be shown before a fleet reads them.
    pub(super) fn materialize_due_spot_fleet_requests(&mut self, now: Instant) {
        if self.spot_fleet_requests.is_empty() {
            return;
        }
        // One immutable pass to derive each fleet's fulfilled capacity and whether any
        // instance it launched is still running, then one mutable pass to write it: neither
        // can be read off the request and instance stores while that fleet is being mutated.
        let derived: Vec<(String, f64, bool)> = self
            .spot_fleet_requests
            .iter()
            .map(|(fleet_id, record)| {
                (
                    fleet_id.clone(),
                    spot_fleet_fulfilled_capacity(
                        &record.placements,
                        &self.spot_instance_requests,
                        &self.instances,
                    ),
                    spot_fleet_has_running_instances(
                        &record.placements,
                        &self.spot_instance_requests,
                        &self.instances,
                    ),
                )
            })
            .collect();
        for (fleet_id, fulfilled, running_instances) in derived {
            if let Some(record) = self.spot_fleet_requests.get_mut(&fleet_id) {
                record.apply_due_move(now, fulfilled, running_instances);
            }
        }
    }
}
