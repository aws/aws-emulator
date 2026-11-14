// SPDX-License-Identifier: CC0-1.0

//! The capacity-block, Elastic IP and transit-gateway route-table stores, with the fast-launch
//! and PTR-record facts those addresses carry.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Capacity Reservations fit inside the store's ceiling
    /// ([`MAX_STORED_CAPACITY_RESERVATIONS`]).
    ///
    /// A writer must ask this BEFORE inserting and refuse when it answers `false`, the way
    /// [`Self::has_capacity_block_capacity`] gates its own store.
    pub fn has_capacity_reservation_capacity(&self, count: usize) -> bool {
        self.capacity_reservations.len().saturating_add(count)
            <= MAX_STORED_CAPACITY_RESERVATIONS
    }

    /// Store one Capacity Reservation and answer the `CapacityReservationId` it was given,
    /// or `None` when the store's own ceilings refuse it.
    ///
    /// The one place a Capacity Reservation enters this store, so every writer of one — and
    /// every reader that reports what this wrote — agrees on the id form, the stored shape
    /// and which members a reservation carries. BOTH ceilings are asked HERE, before the
    /// first write and before an id is minted — the record count
    /// ([`MAX_STORED_CAPACITY_RESERVATIONS`]) and the caller-chosen bytes
    /// ([`MAX_STORED_BYTES`]) — exactly as [`Self::insert_capacity_block`] asks its own, so
    /// no path can get past them by forgetting to: nothing in this emulator reclaims a
    /// stored reservation, so an unbounded write path would grow this process without
    /// limit. A caller must map `None` onto the quota error its operation answers. The
    /// bytes the record retains are charged here, and the clock-owed move is recorded
    /// rather than slept for.
    pub fn insert_capacity_reservation(
        &mut self,
        spec: CapacityReservationSpec,
    ) -> Option<String> {
        // The charge of the record this spec becomes, computed from the spec so it can be
        // asked BEFORE anything is written or an id is minted. It is exactly what
        // [`capacity_reservation_payload_bytes`] counts on the stored view below — the tags
        // and the instance type — which is the ONE definition of this store's charge.
        let charge = tag_list_bytes(&spec.tags).saturating_add(spec.instance_type.len());
        if !self.has_capacity_reservation_capacity(1) || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let capacity_reservation_id = self.next_id(CAPACITY_RESERVATION_ID_PREFIX);
        let view = CapacityReservation {
            capacity_reservation_id: Some(capacity_reservation_id.clone()),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            capacity_reservation_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("capacity-reservation/{capacity_reservation_id}"),
            )),
            availability_zone_id: Some(spec.availability_zone_id),
            instance_type: Some(spec.instance_type),
            instance_platform: Some(spec.instance_platform),
            availability_zone: Some(spec.availability_zone),
            tenancy: Some(spec.tenancy),
            total_instance_count: Some(spec.instance_count),
            // Nothing has launched into a reservation this call mints, so all of its
            // capacity is "the number of instances that can be launched in the Capacity
            // Reservation" (`CapacityReservation$AvailableInstanceCount`).
            available_instance_count: Some(spec.instance_count),
            ebs_optimized: Some(spec.ebs_optimized),
            // "Deprecated" (`CapacityReservation$EphemeralStorage`), and reported as the
            // request asked for it rather than dropped.
            ephemeral_storage: Some(spec.ephemeral_storage),
            state: Some(spec.state.to_string()),
            // "The date and time the Capacity Reservation was started." For capacity for
            // immediate use that is when it was created; a future-dated reservation starts
            // on the date the request asked for.
            start_date: Some(system_time_to_date_time(
                spec.start_date.unwrap_or(spec.created_at),
            )),
            end_date: spec.end_date.map(system_time_to_date_time),
            // "You must provide an EndDate value if EndDateType is limited. Omit EndDate if
            // EndDateType is unlimited" (`CreateCapacityReservationRequest$EndDate`), so the
            // end type is a fact of whether the reservation has an end date.
            end_date_type: Some(
                if spec.end_date.is_some() {
                    END_DATE_TYPE_LIMITED
                } else {
                    END_DATE_TYPE_UNLIMITED
                }
                .to_string(),
            ),
            instance_match_criteria: Some(spec.instance_match_criteria),
            create_date: Some(system_time_to_date_time(spec.created_at)),
            tags: Some(spec.tags),
            // This emulator has no Outposts, no Capacity Reservation Fleets and no cluster
            // placement groups to create a reservation in, so a reservation belongs to
            // none of them.
            outpost_arn: None,
            capacity_reservation_fleet_id: None,
            placement_group_arn: None,
            // "Information about instance capacity usage": nothing in this emulator
            // launches an instance INTO a Capacity Reservation, so no capacity of one is
            // allocated to a running instance.
            capacity_allocations: Some(Vec::new()),
            reservation_type: Some(CAPACITY_RESERVATION_TYPE_DEFAULT.to_string()),
            // "The ID of the Amazon Web Services account to which billing of the unused
            // capacity of the Capacity Reservation is assigned"
            // (`CapacityReservation$UnusedReservationBillingOwnerId`): the owner's own
            // account until `AssociateCapacityReservationBillingOwner` assigns it to
            // another one, so every reservation reports the account that is billed for the
            // capacity nothing has launched into.
            unused_reservation_billing_owner_id: Some(
                ::engine::emulator_account_id().to_string(),
            ),
            // "Information about the commitment for the Capacity Reservation"
            // (`CapacityReservation$CommitmentInfo`) and "The delivery method"
            // (`$DeliveryPreference`): members of a future-dated reservation only, so a
            // reservation created for immediate use carries neither.
            commitment_info: spec.commitment.map(|commitment| {
                CapacityReservationCommitmentInfo {
                    // "The instance capacity that you committed to when you requested the
                    // future-dated Capacity Reservation"
                    // (`CapacityReservationCommitmentInfo$CommittedInstanceCount`), which is
                    // the capacity the request reserved.
                    committed_instance_count: Some(spec.instance_count),
                    // "The date and time at which the commitment duration expires"
                    // (`$CommitmentEndDate`), computed by the writer from the start date.
                    commitment_end_date: Some(system_time_to_date_time(commitment.end_date)),
                    commitment_duration: Some(commitment.duration_seconds),
                }
            }),
            delivery_preference: spec.delivery_preference,
            capacity_block_id: None,
            interruptible: Some(spec.interruption_info.is_some()),
            // Filled in by the allocation path on the SOURCE reservation, which is the
            // reservation an allocation is made FROM; see
            // `ops/create_interruptible_capacity_reservation_allocation.rs`.
            interruptible_capacity_allocation: None,
            interruption_info: spec.interruption_info,
            // "This field is not returned if the Capacity Reservation has never been
            // modified" (`CapacityReservation$AdjustmentStatus`).
            adjustment_status: None,
            adjustment_details: None,
            // "This value doesn't change when you push out the start date"
            // (`CapacityReservation$OriginalStartDate`): the start date it was created with,
            // which for a future-dated reservation is the one the request asked for.
            original_start_date: Some(system_time_to_date_time(
                spec.start_date.unwrap_or(spec.created_at),
            )),
            zero_size_preference: spec.zero_size_preference.map(str::to_string),
            // "Only supported for UltraServers" (`CapacityReservation$LaunchStatus`), and
            // this reservation is of plain instances.
            launch_status: None,
        };
        self.add_stored_bytes(capacity_reservation_payload_bytes(&view));
        self.capacity_reservations.insert(
            capacity_reservation_id.clone(),
            CapacityReservationRecord {
                view,
                pending: spec.pending,
            },
        );
        Some(capacity_reservation_id)
    }

    /// Cancel the Capacity Reservation `capacity_reservation_id` names, and answer whether
    /// the store held one.
    ///
    /// "Cancels the specified Capacity Reservation, releases the reserved capacity, and
    /// changes the Capacity Reservation's state to `cancelled`"
    /// (`CancelCapacityReservation`): the ONE place that transition is written, so the
    /// operation that cancels and every reader of a cancelled reservation agree on what it
    /// holds. The reserved capacity stays the reservation's own count — "the total number of
    /// instances for which the Capacity Reservation reserves capacity"
    /// (`CapacityReservation$TotalInstanceCount`) is what was reserved, whatever state the
    /// reservation ends in.
    ///
    /// An interruptible allocation ties two reservations together and cannot outlive either
    /// of them, so the pair is released here too
    /// ([`Self::release_interruptible_allocation`]): cancelling an interruptible reservation
    /// releases the allocation recorded on its source, and cancelling a source that carries
    /// one cancels the interruptible reservation that allocation created. Either way the
    /// allocated capacity is the source's again and the allocation reaches its terminal
    /// status on the clock.
    pub fn cancel_capacity_reservation(&mut self, capacity_reservation_id: &str) -> bool {
        let Some(record) = self.capacity_reservations.get(capacity_reservation_id) else {
            return false;
        };
        let allocated_from = record
            .view
            .interruption_info
            .as_ref()
            .and_then(|info| info.source_capacity_reservation_id.clone());
        let carries_allocation = record.view.interruptible_capacity_allocation.is_some();
        if let Some(record) = self.capacity_reservations.get_mut(capacity_reservation_id) {
            record.view.state = Some(CAPACITY_RESERVATION_STATE_CANCELLED.to_string());
            record.pending = None;
        }
        if let Some(source_id) = allocated_from {
            self.release_interruptible_allocation(&source_id);
        }
        if carries_allocation {
            self.release_interruptible_allocation(capacity_reservation_id);
        }
        true
    }

    /// Release the interruptible allocation recorded on the source Capacity Reservation
    /// `source_id`: the allocated capacity goes back to the source, the interruptible
    /// reservation that allocation created is cancelled, and the allocation starts the move
    /// to its terminal status the clock owes it.
    ///
    /// "A value of `default` cancels the interruptible Capacity Reservation and returns the
    /// capacity to your source Capacity Reservation"
    /// (`InterruptibleCapacityAllocation$ZeroSizePreference`) — returning the capacity is
    /// what a cancellation of the allocation does, so the counts the allocation drew out of
    /// the source ([`crate::ops::create_interruptible_capacity_reservation_allocation`]) are
    /// added back to it here.
    ///
    /// Silent for a reservation that carries no allocation, and idempotent for one whose
    /// allocation is already on its way out: an allocation is released once however many
    /// times either side of it is cancelled.
    ///
    /// The move is recorded in the record's one `pending` slot, which also carries the
    /// reservation's OWN state moves ([`CapacityReservationMove::Active`] /
    /// [`CapacityReservationMove::Scheduled`], read by
    /// [`CapacityReservationRecord::state_at`]). That is sound because capacity can only be
    /// allocated out of an `active` reservation with no end date
    /// (`CreateInterruptibleCapacityReservationAllocation`), and a reservation already in
    /// `active` is owed no state move — so a source carrying an allocation never has one of
    /// those pending for this to overwrite.
    pub(super) fn release_interruptible_allocation(&mut self, source_id: &str) {
        let interruptible_id = {
            let Some(record) = self.capacity_reservations.get_mut(source_id) else {
                return;
            };
            let Some(allocation) = record.view.interruptible_capacity_allocation.as_mut() else {
                return;
            };
            if matches!(
                allocation.status.as_deref(),
                Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELING)
                    | Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED)
            ) {
                return;
            }
            let returned = allocation.target_instance_count.unwrap_or_default();
            let interruptible_id = allocation.interruptible_capacity_reservation_id.clone();
            allocation.status = Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELING.to_string());
            record.view.total_instance_count = Some(
                record
                    .view
                    .total_instance_count
                    .unwrap_or_default()
                    .saturating_add(returned),
            );
            record.view.available_instance_count = Some(
                record
                    .view
                    .available_instance_count
                    .unwrap_or_default()
                    .saturating_add(returned),
            );
            // The terminal status is owed to the clock rather than written now, so a caller
            // polling for it sees the `canceling` the model carries for the capacity still
            // moving — and sees `canceled` from the instant it falls due, whether or not
            // anybody calls again.
            record.pending = Some(CapacityReservationTransition::starting_now(
                CapacityReservationMove::AllocationCanceled,
            ));
            interruptible_id
        };
        if let Some(interruptible) = interruptible_id
            .as_deref()
            .and_then(|id| self.capacity_reservations.get_mut(id))
        {
            interruptible.view.state = Some(CAPACITY_RESERVATION_STATE_CANCELLED.to_string());
            interruptible.pending = None;
        }
    }

    /// Move the interruptible allocation recorded on the source Capacity Reservation
    /// `source_id` to `target_instance_count` instances, and answer whether it was applied.
    ///
    /// "Modifies the number of instances allocated to an interruptible reservation, allowing
    /// you to add more capacity or reclaim capacity to your source Capacity Reservation"
    /// (`UpdateInterruptibleCapacityReservationAllocation`): the difference between the
    /// target the allocation had and the one it is given MOVES between the two reservations —
    /// out of the source to add capacity, back into it to reclaim — so the pair always
    /// accounts for the capacity that was reserved. The caller must have checked that the
    /// source has the capacity an increase asks for.
    ///
    /// The allocation is left `updating` — "updating during reclamation, active when
    /// complete" (`InterruptibleCapacityAllocation$Status`) — and the clock owes it `active`
    /// with the new target allocated, whether or not anybody calls again.
    ///
    /// `false`, with nothing written, for a reservation the store does not hold or one that
    /// carries no allocation. The `pending` slot it writes carries the allocation's move on
    /// the terms [`Self::release_interruptible_allocation`] states.
    pub fn resize_interruptible_allocation(
        &mut self,
        source_id: &str,
        target_instance_count: i32,
    ) -> bool {
        // What leaves the source — positive to allocate more, negative to reclaim — and the
        // reservation it moves to, taken in one borrow of the source's record so the two
        // halves of the move cannot disagree about the number.
        let moved_to = {
            let Some(record) = self.capacity_reservations.get_mut(source_id) else {
                return false;
            };
            let Some(allocation) = record.view.interruptible_capacity_allocation.as_mut() else {
                return false;
            };
            let moved = target_instance_count
                .saturating_sub(allocation.target_instance_count.unwrap_or_default());
            allocation.target_instance_count = Some(target_instance_count);
            allocation.status = Some(INTERRUPTIBLE_ALLOCATION_STATUS_UPDATING.to_string());
            let interruptible_id = allocation.interruptible_capacity_reservation_id.clone();
            record.view.total_instance_count = Some(
                record
                    .view
                    .total_instance_count
                    .unwrap_or_default()
                    .saturating_sub(moved),
            );
            record.view.available_instance_count = Some(
                record
                    .view
                    .available_instance_count
                    .unwrap_or_default()
                    .saturating_sub(moved),
            );
            record.pending = Some(CapacityReservationTransition::starting_now(
                CapacityReservationMove::AllocationActive,
            ));
            interruptible_id.map(|id| (id, moved))
        };
        // The allocation's own half of the move. An allocation naming no reservation has no
        // other half to move — the source's side is applied either way, which is what makes
        // this answer `true`.
        let Some((interruptible_id, moved)) = moved_to else {
            return true;
        };
        if let Some(interruptible) = self.capacity_reservations.get_mut(&interruptible_id) {
            interruptible.view.total_instance_count = Some(
                interruptible
                    .view
                    .total_instance_count
                    .unwrap_or_default()
                    .saturating_add(moved),
            );
            interruptible.view.available_instance_count = Some(
                interruptible
                    .view
                    .available_instance_count
                    .unwrap_or_default()
                    .saturating_add(moved),
            );
        }
        true
    }

    /// Store the pair one Capacity Block purchase buys — the block, and the Capacity
    /// Reservation that holds its capacity — and answer
    /// `(capacity_block_id, capacity_reservation_id)`, or `None` with NOTHING written when
    /// either store's own ceiling refuses it.
    ///
    /// The two are ONE purchase, not two resources created separately: the block's
    /// `CapacityReservationIds` names the reservation and the reservation's
    /// `CapacityBlockId` names the block, so this is the one place that link is made and
    /// neither side can name an id the other half never got. Each half still enters through
    /// its own insert ([`Self::insert_capacity_block`],
    /// [`Self::insert_capacity_reservation`]), so a purchased record carries the same id
    /// form, stored shape and byte charge as one any other writer of that store makes. Both
    /// count ceilings are asked for the PAIR before either half is written and a half-written
    /// pair is taken back out, so a refused purchase stores neither record and charges
    /// nothing.
    pub fn insert_purchased_capacity_block(
        &mut self,
        spec: CapacityBlockPurchaseSpec,
    ) -> Option<(String, String)> {
        if !self.has_capacity_block_capacity(1) || !self.has_capacity_reservation_capacity(1) {
            return None;
        }
        let capacity_reservation_id = self.insert_capacity_reservation(spec.reservation)?;
        let mut block = spec.block;
        block.capacity_reservation_ids = vec![capacity_reservation_id.clone()];
        let Some(capacity_block_id) = self.insert_capacity_block(block) else {
            // The block half was refused, so there is no purchase at all: the reservation
            // half comes back out and gives back exactly the bytes
            // [`Self::insert_capacity_reservation`] charged for it.
            if let Some(record) = self.capacity_reservations.remove(&capacity_reservation_id) {
                self.release_stored_bytes(capacity_reservation_payload_bytes(&record.view));
            }
            return None;
        };
        // The reservation's side of the link, which can only be written once the block it
        // belongs to has an id. "The type of Capacity Reservation"
        // (`CapacityReservation$ReservationType`) follows from it: a reservation a Capacity
        // Block holds its capacity as is of that type rather than the default one.
        if let Some(record) = self.capacity_reservations.get_mut(&capacity_reservation_id) {
            record.view.capacity_block_id = Some(capacity_block_id.clone());
            record.view.reservation_type =
                Some(CAPACITY_RESERVATION_TYPE_CAPACITY_BLOCK.to_string());
        }
        Some((capacity_block_id, capacity_reservation_id))
    }
}

/// What one Capacity Block purchase stores: the block it buys, and the Capacity Reservation
/// that holds the block's capacity — the pair
/// [`ComAmazonawsEc2Amazonec2State::insert_purchased_capacity_block`] writes.
///
/// Neither half names the other: the block's `CapacityReservationIds` and the reservation's
/// `CapacityBlockId` are ids this store mints, so the insert fills them in rather than a
/// writer naming an id that does not exist yet.
#[derive(Debug, Clone)]
pub struct CapacityBlockPurchaseSpec {
    /// The Capacity Block the purchase buys, with its `capacity_reservation_ids` left to the
    /// insert.
    pub block: CapacityBlockSpec,
    /// The Capacity Reservation that holds its capacity.
    pub reservation: CapacityReservationSpec,
}

/// Refuse a request that named a Capacity Reservation id no reservation of the account
/// has: HTTP 400 `InvalidCapacityReservationId.NotFound`.
///
/// The not-found half of the pair [`require_known_capacity_reservation`] answers: a
/// well-formed id ([`capacity_reservation_id_is_well_formed`]) this store holds no
/// reservation under. A value OUTSIDE that form is answered
/// [`invalid_capacity_reservation_id_malformed`] instead, before any lookup.
pub fn invalid_capacity_reservation_id_not_found<E>(
    capacity_reservation_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND,
        400,
        format!("The capacity-reservation ID '{capacity_reservation_id}' does not exist"),
    )
}

/// The code EC2 answers for a value that is not a Capacity Reservation id at all, so there
/// is nothing to look it up as, in the `Invalid<Resource>Id.Malformed` family this service
/// refuses an unissuable id of any other kind with
/// ([`INVALID_LAUNCH_TEMPLATE_ID_MALFORMED`], [`INVALID_SECONDARY_NETWORK_ID_MALFORMED`]).
pub const INVALID_CAPACITY_RESERVATION_ID_MALFORMED: &str =
    "InvalidCapacityReservationId.Malformed";

/// Whether `id` is a well-formed Capacity Reservation id — one EC2 could have ISSUED.
///
/// The model publishes no pattern for `CapacityReservationId`
/// (`smithy_query(query="shape",shape="CapacityReservationId")`), so the form is the one
/// EC2 issues: the LONG resource-id reading ([`issued_long_id_is_well_formed`]) under this
/// resource's own [`CAPACITY_RESERVATION_ID_PREFIX`] — seventeen lowercase hexadecimal
/// digits whose leading digit is `0` and which are not all `0`, which is also what
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints. The same reading an instance id and a
/// Capacity Reservation Fleet id get, and for the same reason: EC2 answers
/// `Invalid<Kind>.Malformed` for an id of this kind that merely LOOKS hexadecimal, rather
/// than reporting it not found, so a caller can tell a typo from a reservation its account
/// does not hold — a recorded `MoveCapacityReservationInstances` call naming a `cr-` id
/// whose leading digit is above zero is answered
/// [`INVALID_CAPACITY_RESERVATION_ID_MALFORMED`], where a well-formed id nothing was
/// reserved under is answered [`INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND`].
pub fn capacity_reservation_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed(CAPACITY_RESERVATION_ID_PREFIX, id)
}

/// Refuse a value that is not a Capacity Reservation id at all: HTTP 400
/// [`INVALID_CAPACITY_RESERVATION_ID_MALFORMED`], worded the way AWS words a malformed id
/// of this resource kind and built from the value the request sent.
///
/// This is the sentence the reservation's WRITE and usage validators answer, recorded off the
/// 400s behind `issues/.../20261023T102725Z-ec2-getcapacityreservationusage-stub.md`,
/// `...-ec2-movecapacityreservationinstances-stub.md` and
/// `...-ec2-acceptcapacityreservationbillingownership-stub.md` — `ID`, and no terminating
/// period. EC2's validators do NOT all word this id the same way, exactly as they do not all
/// AGREE about it (see [`require_capacity_reservation_by_id_shape`] beside
/// [`require_resolvable_capacity_reservation`]), so an operation whose own recorded sentence
/// differs keeps that sentence where it resolves its id rather than rewording this one.
pub fn invalid_capacity_reservation_id_malformed<E>(
    capacity_reservation_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_ID_MALFORMED,
        400,
        format!("Capacity Reservation ID '{capacity_reservation_id}' is malformed"),
    )
}

/// Refuse a value outside the Capacity Reservation id form before any lookup, for a caller
/// that checks an id it does not need the record behind — a destination it is about to
/// resolve separately, or an id named beside another.
pub fn require_well_formed_capacity_reservation_id<E>(
    capacity_reservation_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if capacity_reservation_id_is_well_formed(capacity_reservation_id) {
        return Ok(());
    }
    Err(invalid_capacity_reservation_id_malformed(
        capacity_reservation_id,
    ))
}

/// The record of the Capacity Reservation a request names IN THE REGION it was signed for,
/// read by the SHAPE every EC2 resource id takes, or the refusal for an id it cannot resolve.
///
/// Reads the id by SHAPE, through [`require_capacity_reservation_by_id_shape`], rather than
/// by the stricter issued-id reading [`require_known_capacity_reservation`] applies, so a
/// value of that shape the account holds no reservation under — the all-zero placeholder
/// included — is reported NOT FOUND rather than malformed. That is the reading of the
/// operations recorded answering it: a `DescribeCapacityBlockExtensionOfferings` and a
/// `CreateCapacityReservationCancellationQuote` naming the all-zero `cr-` id are each
/// answered [`INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND`], so they share this reader rather
/// than each resolving its id its own way.
///
/// Region-scoped like [`require_capacity_reservation_in_region`], and for the same reason: a
/// reservation reserves capacity in ONE Availability Zone, so one of another Region is not a
/// reservation a request signed for this Region can name, and is reported not found rather
/// than resolved across Regions.
pub fn require_capacity_reservation_in_region_by_id_shape<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    zones: &[AvailabilityZoneRecord],
    capacity_reservation_id: &str,
) -> Result<&'store CapacityReservationRecord, crate::scaffolded::Scaffolded<E>> {
    let record = require_capacity_reservation_by_id_shape(store, capacity_reservation_id)?;
    if !capacity_reservation_is_in_region(&record.view, zones) {
        return Err(invalid_capacity_reservation_id_not_found(
            capacity_reservation_id,
        ));
    }
    Ok(record)
}

/// The record of the Capacity Reservation `capacity_reservation_id` names, read by the SHAPE
/// every EC2 resource id takes ([`prefixed_id_is_well_formed`]) rather than by the stricter
/// issued-id reading [`require_known_capacity_reservation`] applies: a value of that shape
/// the account holds no reservation under — the all-zero placeholder included — is reported
/// [`INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND`], and only a value outside the shape is
/// refused [`INVALID_CAPACITY_RESERVATION_ID_MALFORMED`], before any lookup.
///
/// Which of the two readings an operation applies is a fact of that OPERATION, recorded on
/// both sides, so the choice is made where the operation resolves its id rather than here:
/// a recorded `DescribeCapacityBlockExtensionOfferings` and the recorded interruptible
/// allocation pair (`CreateInterruptibleCapacityReservationAllocation`,
/// `UpdateInterruptibleCapacityReservationAllocation`) each report the all-zero `cr-` id
/// NOT FOUND and so read ids here, where a recorded `MoveCapacityReservationInstances`,
/// `CancelCapacityReservation` and `GetCapacityReservationUsage` refuse an id EC2 could not
/// have issued as malformed and so read ids through
/// [`require_known_capacity_reservation`].
pub fn require_capacity_reservation_by_id_shape<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    capacity_reservation_id: &str,
) -> Result<&'store CapacityReservationRecord, crate::scaffolded::Scaffolded<E>> {
    if !prefixed_id_is_well_formed(CAPACITY_RESERVATION_ID_PREFIX, capacity_reservation_id) {
        return Err(invalid_capacity_reservation_id_malformed(
            capacity_reservation_id,
        ));
    }
    store
        .capacity_reservations
        .get(capacity_reservation_id)
        .ok_or_else(|| invalid_capacity_reservation_id_not_found(capacity_reservation_id))
}

/// The record of the Capacity Reservation `capacity_reservation_id` names, for an operation
/// whose validator answers [`INVALID_CAPACITY_RESERVATION_ID_MALFORMED`] for EVERY id it
/// cannot resolve.
///
/// EC2's per-operation validators do NOT agree about a Capacity Reservation id they cannot
/// resolve, and this is the strict reading: a well-formed id the account holds no reservation
/// under is refused as malformed — to this validator an id it never issued THIS account is
/// not an id — rather than reported not found. The recorded contract of
/// `GetCapacityReservationUsage`, `GetGroupsForCapacityReservation` and
/// `MoveCapacityReservationInstances` carries that answer, where
/// `DescribeCapacityReservations` reports such an id
/// [`INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND`] and the interruptible allocation pair reads
/// ids through [`require_capacity_reservation_by_id_shape`]. So the reading belongs to the
/// OPERATION, and the three that take this one share it here rather than each keeping a copy.
pub fn require_resolvable_capacity_reservation<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    capacity_reservation_id: &str,
) -> Result<&'store CapacityReservationRecord, crate::scaffolded::Scaffolded<E>> {
    require_well_formed_capacity_reservation_id(capacity_reservation_id)?;
    store
        .capacity_reservations
        .get(capacity_reservation_id)
        .ok_or_else(|| invalid_capacity_reservation_id_malformed(capacity_reservation_id))
}

/// The record of the Capacity Reservation `capacity_reservation_id` names, or the refusal
/// for an id this service cannot resolve: a malformed one before an unknown one, the way
/// the service orders the two.
pub fn require_known_capacity_reservation<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    capacity_reservation_id: &str,
) -> Result<&'store CapacityReservationRecord, crate::scaffolded::Scaffolded<E>> {
    require_well_formed_capacity_reservation_id(capacity_reservation_id)?;
    store
        .capacity_reservations
        .get(capacity_reservation_id)
        .ok_or_else(|| invalid_capacity_reservation_id_not_found(capacity_reservation_id))
}

// =============================================================================
// Capacity Reservation cancellation quotes
// =============================================================================
//
// See the module docs for why a quote is stored as the modelled shape plus the one move the
// clock owes it. The words below are members of the model's own enums and the id form this
// service issues, named here so the create and the describe cannot spell one value two
// ways.

/// The id prefix a Capacity Reservation cancellation quote's id carries, in the
/// `<prefix>-<hex>` form every EC2 resource id takes (see
/// [`ComAmazonawsEc2Amazonec2State::next_id`]).
///
/// The model publishes no pattern for `CapacityReservationCancellationQuoteId`
/// (`smithy_query(query="shape",shape="CapacityReservationCancellationQuoteId")`), so the
/// prefix is this service's own for the resource kind, kept here as the one place it is
/// spelled.
pub const CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX: &str = "crcq";

/// The `ResourceType` a cancellation quote tag specification may name: the
/// `capacity-reservation-cancellation-quote` member of the modelled `ResourceType` enum
/// (`smithy_query(query="shape",shape="ResourceType")`), which is the resource
/// `CreateCapacityReservationCancellationQuote`'s "tags to apply to the cancellation quote"
/// go on.
pub const RESOURCE_TYPE_CAPACITY_RESERVATION_CANCELLATION_QUOTE: &str =
    "capacity-reservation-cancellation-quote";

/// The state a quote is generated in: the `pending` member of the modelled
/// `CapacityReservationCancellationQuoteState` enum
/// (`smithy_query(query="shape",shape="CapacityReservationCancellationQuoteState")`), the one
/// word that enum carries for a quote that has been asked for and is not usable yet.
pub const CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING: &str = "pending";

/// That enum's `active` member — the state a quote has to be in to be used: "The cancellation
/// quote must be in an active state" (`CancelCapacityReservationRequest$QuoteId`).
pub const CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE: &str = "active";

/// That enum's `expired` member, where a quote sits once the wall clock has passed the
/// `ExpirationTime` it was generated with
/// ([`CapacityReservationCancellationQuoteRecord::quote_state_at`]).
pub const CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED: &str = "expired";

/// How long a generated quote stays usable: "Cancellation quotes are valid for 24 hours"
/// (`CreateCapacityReservationCancellationQuote`), which is [`ONE_DAY`].
pub const CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY: Duration = ONE_DAY;

/// The cancellation charge a quote quotes for: the only member of the modelled
/// `ApplyCancellationCharges` enum
/// (`smithy_query(query="shape",shape="ApplyCancellationCharges")`), which is also the value
/// `CancelCapacityReservation` takes a quote with — "pass the quote ID with
/// ApplyCancellationCharges set to commitment-wind-down".
pub const APPLY_CANCELLATION_CHARGES_COMMITMENT_WIND_DOWN: &str = "commitment-wind-down";

/// How many cancellation quotes this emulator stores before a writer must refuse.
///
/// CHOSEN, like [`MAX_STORED_CAPACITY_RESERVATIONS`] and for the same reason: nothing in this
/// emulator reclaims a stored quote, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is no bound on the caller-chosen bytes each record holds, so those
/// are charged against [`MAX_STORED_BYTES`] too
/// ([`capacity_reservation_cancellation_quote_payload_bytes`]).
pub const MAX_STORED_CAPACITY_RESERVATION_CANCELLATION_QUOTES: usize = 4096;

/// The caller-chosen bytes one stored quote holds: its tags. Everything else on the record —
/// the ids, the instants, the counts, the state words — is derived by this module or bounded
/// by the id form, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`capacity_reservation_payload_bytes`] is for
/// its own store:
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation_cancellation_quote`] charges
/// exactly it, and a removal path must give back the same number.
pub fn capacity_reservation_cancellation_quote_payload_bytes(
    quote: &CapacityReservationCancellationQuote,
) -> usize {
    tag_list_bytes(quote.tags.as_deref().unwrap_or_default())
}

/// What the clock owes one stored cancellation quote next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityReservationCancellationQuoteMove {
    /// `pending` -> `active`: the quote is generated and usable.
    Active,
}

/// The move the clock owes one cancellation quote: a [`ClockTransition`] to the next
/// [`CapacityReservationCancellationQuoteMove`].
pub type CapacityReservationCancellationQuoteTransition =
    ClockTransition<CapacityReservationCancellationQuoteMove>;

/// One stored Capacity Reservation cancellation quote: the modelled
/// [`CapacityReservationCancellationQuote`] every operation that reports one answers with,
/// plus the move the clock owes it.
///
/// The view is the modelled shape itself rather than a parallel record, so every member a
/// writer sets round-trips through a reader without a field-by-field copy that can silently
/// drop one — the same arrangement [`CapacityReservationRecord`] uses.
#[derive(Debug, Clone)]
pub struct CapacityReservationCancellationQuoteRecord {
    /// The quote as the model shapes it.
    pub view: CapacityReservationCancellationQuote,
    /// The move the clock owes this quote, while one is owed.
    pub pending: Option<CapacityReservationCancellationQuoteTransition>,
}

impl CapacityReservationCancellationQuoteRecord {
    /// The quote's `CapacityReservationCancellationQuoteState` as it was last WRITTEN.
    ///
    /// [`Self::quote_state_at`] is what a reader reports: both of a quote's moves are facts
    /// of where the clock stands rather than something a writer stored.
    pub fn quote_state(&self) -> &str {
        self.view.quote_state.as_deref().unwrap_or_default()
    }

    /// The quote's state at `now`.
    ///
    /// Two facts of the clock are derived here rather than stored, so a read reports what was
    /// already due without writing anything:
    ///
    /// * The end of the wait for the quote to be generated: it is answered `pending` and the
    ///   move to `active` fell due at the wall-clock instant its
    ///   [`CapacityReservationCancellationQuoteTransition`] names ([`ClockTransition::at`]),
    ///   whether or not anybody called again.
    /// * The end of its validity: "Cancellation quotes are valid for 24 hours"
    ///   (`CreateCapacityReservationCancellationQuote`), so a quote is `expired` from the
    ///   `ExpirationTime` it carries on — which outranks the move above, a quote never
    ///   becoming usable after it has stopped being valid.
    pub fn quote_state_at(&self, now: SystemTime) -> &str {
        let now_key = date_time_key(system_time_to_date_time(now));
        if self
            .view
            .expiration_time
            .map(date_time_key)
            .is_some_and(|expires| now_key >= expires)
        {
            return CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_EXPIRED;
        }
        let stored = self.quote_state();
        let generated = stored == CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING
            && self.pending.as_ref().is_some_and(|pending| {
                matches!(
                    pending.to,
                    CapacityReservationCancellationQuoteMove::Active
                ) && now >= pending.at
            });
        if generated {
            return CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_ACTIVE;
        }
        stored
    }

    /// This quote as of `now`: the stored view with the state the clock has reached. A read
    /// observes the generation and the expiry the clock already owed without writing anything.
    pub fn view_at(&self, now: SystemTime) -> CapacityReservationCancellationQuote {
        let mut view = self.view.clone();
        view.quote_state = Some(self.quote_state_at(now).to_string());
        view
    }
}

/// What one stored cancellation quote is created with, for
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation_cancellation_quote`].
///
/// The instant is a [`SystemTime`] because the writer computes it from the clock it already
/// holds; the store converts it once. Everything the service itself owns — the id, the instant
/// the quote stops being valid and the state it is generated in — is derived by the insert
/// rather than named here.
#[derive(Debug, Clone)]
pub struct CapacityReservationCancellationQuoteSpec {
    /// "The ID of the Capacity Reservation associated with the cancellation quote"
    /// (`CapacityReservationCancellationQuote$CapacityReservationId`).
    pub capacity_reservation_id: String,
    /// When the quote was generated, which is what its validity is measured from.
    pub created_at: SystemTime,
    /// "The current configuration of the Capacity Reservation" as of that instant.
    pub current_configuration: CapacityReservationConfiguration,
    /// "The cancellation terms associated with the quote, including the fee type and charge
    /// details" (`$CancellationTerms`).
    pub cancellation_terms: Vec<CancellationTerms>,
    /// "The tags assigned to the cancellation quote" (`$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` cancellation quotes fit inside the store's ceiling
    /// ([`MAX_STORED_CAPACITY_RESERVATION_CANCELLATION_QUOTES`]).
    ///
    /// A writer must ask this BEFORE inserting and refuse when it answers `false`, the way
    /// [`Self::has_capacity_reservation_capacity`] gates its own store.
    pub fn has_capacity_reservation_cancellation_quote_capacity(&self, count: usize) -> bool {
        self.capacity_reservation_cancellation_quotes
            .len()
            .saturating_add(count)
            <= MAX_STORED_CAPACITY_RESERVATION_CANCELLATION_QUOTES
    }

    /// Store one cancellation quote and answer the `CapacityReservationCancellationQuoteId` it
    /// was given, or `None` when the store's own ceilings refuse it.
    ///
    /// The one place a quote enters this store, so every writer of one — and every reader that
    /// reports what this wrote — agrees on the id form, the stored shape and which members a
    /// quote carries. BOTH ceilings are asked HERE, before the first write and before an id is
    /// minted — the record count
    /// ([`MAX_STORED_CAPACITY_RESERVATION_CANCELLATION_QUOTES`]) and the caller-chosen bytes
    /// ([`MAX_STORED_BYTES`]) — exactly as [`Self::insert_capacity_reservation`] asks its own:
    /// nothing here reclaims a stored quote, so an unbounded write path would grow this
    /// process without limit. A caller must map `None` onto the quota error its operation
    /// answers. The bytes the record retains are charged here, and the clock-owed move is
    /// recorded rather than slept for.
    pub fn insert_capacity_reservation_cancellation_quote(
        &mut self,
        spec: CapacityReservationCancellationQuoteSpec,
    ) -> Option<String> {
        // The charge of the record this spec becomes, computed from the spec so it can be
        // asked BEFORE anything is written or an id is minted. It is exactly what
        // [`capacity_reservation_cancellation_quote_payload_bytes`] counts on the stored view
        // below, which is the ONE definition of this store's charge.
        let charge = tag_list_bytes(&spec.tags);
        if !self.has_capacity_reservation_cancellation_quote_capacity(1)
            || self.would_exceed_stored_bytes(charge)
        {
            return None;
        }
        let quote_id = self.next_id(CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX);
        let view = CapacityReservationCancellationQuote {
            capacity_reservation_cancellation_quote_id: Some(quote_id.clone()),
            capacity_reservation_id: Some(spec.capacity_reservation_id),
            // "The date and time at which the cancellation quote was created" ($CreateTime).
            create_time: Some(system_time_to_date_time(spec.created_at)),
            // "The date and time at which the cancellation quote expires" ($ExpirationTime),
            // which is its documented validity after that instant. Checked arithmetic: the
            // span is this module's own constant, so it cannot overflow in practice, and an
            // instant the clock type could not hold is reported as no expiry at all rather
            // than panicked on.
            expiration_time: spec
                .created_at
                .checked_add(CAPACITY_RESERVATION_CANCELLATION_QUOTE_VALIDITY)
                .map(system_time_to_date_time),
            // Generated `pending`, with `active` owed one TRANSITION_DELAY later.
            quote_state: Some(
                CAPACITY_RESERVATION_CANCELLATION_QUOTE_STATE_PENDING.to_string(),
            ),
            current_configuration: Some(spec.current_configuration),
            cancellation_terms: Some(spec.cancellation_terms),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(capacity_reservation_cancellation_quote_payload_bytes(&view));
        self.capacity_reservation_cancellation_quotes.insert(
            quote_id.clone(),
            CapacityReservationCancellationQuoteRecord {
                view,
                pending: Some(CapacityReservationCancellationQuoteTransition::starting_now(
                    CapacityReservationCancellationQuoteMove::Active,
                )),
            },
        );
        Some(quote_id)
    }
}

/// The code this service answers for a cancellation quote id the account holds no quote for,
/// in the `Invalid<Resource>Id.NotFound` family every other id of this service is refused
/// with ([`INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND`]).
pub const INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_NOT_FOUND: &str =
    "InvalidCapacityReservationCancellationQuoteId.NotFound";

/// The code for a value that is not a cancellation quote id at all, so there is nothing to
/// look it up as, in the `Invalid<Resource>Id.Malformed` family this service refuses an
/// unissuable id of any other kind with
/// ([`INVALID_CAPACITY_RESERVATION_ID_MALFORMED`]).
pub const INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_MALFORMED: &str =
    "InvalidCapacityReservationCancellationQuoteId.Malformed";

/// Whether `id` is a well-formed cancellation quote id — one this service could have ISSUED.
///
/// The model publishes no pattern for `CapacityReservationCancellationQuoteId`, so the form is
/// the one this service issues: the LONG resource-id reading
/// ([`issued_long_id_is_well_formed`]) under this resource's own
/// [`CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX`], which is what
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints — the same reading a Capacity Reservation
/// id gets ([`capacity_reservation_id_is_well_formed`]), so a caller can tell a typo from a
/// quote its account does not hold.
pub fn capacity_reservation_cancellation_quote_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed(CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_PREFIX, id)
}

/// Refuse a value that is not a cancellation quote id at all: HTTP 400
/// [`INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_MALFORMED`], worded the way AWS words
/// a malformed id of this resource kind and built from the value the request sent.
///
/// The resource is labelled `Cancellation Quote Id` — the quote's own two words, not the
/// reservation's, and `Id` rather than `ID` — and the sentence terminates in a period, the
/// way the Capacity Block offering's does
/// ([`invalid_capacity_block_offering_id_malformed`]). Recorded off the 400 a describe of a
/// quote id outside the issued form is answered with, which is the only authority for it:
/// EC2 models no error shapes, and its validators word this family's sentences differently
/// per resource (compare [`invalid_capacity_reservation_id_malformed`]).
///
/// The echo is cut to length by [`echoed_resource_id`], the ceiling this crate holds every
/// repeated-back id to, so a caller-sized value cannot grow the refusal it earned.
pub fn invalid_capacity_reservation_cancellation_quote_id_malformed<E>(
    quote_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_MALFORMED,
        400,
        format!(
            "Cancellation Quote Id '{echoed}' is malformed.",
            echoed = echoed_resource_id(quote_id),
        ),
    )
}

/// Refuse a well-formed cancellation quote id this store holds no quote under: HTTP 400
/// [`INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_NOT_FOUND`]. A value OUTSIDE the id
/// form is answered [`invalid_capacity_reservation_cancellation_quote_id_malformed`] instead,
/// before any lookup.
pub fn invalid_capacity_reservation_cancellation_quote_id_not_found<E>(
    quote_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_CANCELLATION_QUOTE_ID_NOT_FOUND,
        400,
        format!(
            "The capacity-reservation-cancellation-quote ID '{quote_id}' does not exist"
        ),
    )
}

/// Refuse a cancellation quote for a Capacity Reservation that has NO commitment terms in
/// force: HTTP 400 `InvalidParameterValue`.
///
/// "Generates a cancellation quote for a future-dated Capacity Reservation that is within its
/// commitment duration" (`CreateCapacityReservationCancellationQuote`), and a cancellation
/// costs nothing to quote when the reservation "is outside the commitment duration or the
/// commitment duration has elapsed" (`CancelCapacityReservation`). So a reservation that
/// committed to nothing and one whose commitment the clock has passed are the ONE
/// precondition — no commitment terms are in force — and both are refused here, in the
/// parameter-value class, rather than as a state the reservation is in.
///
/// AWS words this refusal as one FIXED sentence that interpolates nothing: the id the request
/// named is the value the caller already sent, and the state the reservation is in is read
/// back through a describe. Hence a constructor with no parameters, and the ONE place this
/// crate words it.
pub fn capacity_reservation_needs_no_cancellation_quote<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::invalid_parameter_value(
        "The Capacity Reservation contains no commitment terms. Quote is not needed for \
         cancellation.",
    )
}

/// A timestamp attribute as an EC2 filter value compares against it: the wire rendering of
/// the instant, which is what a caller was shown. A member that is not set carries no value
/// for the filter to match, like any other absent attribute.
///
/// The timestamp companion of [`boolean_filter_values`] and [`integer_filter_values`], beside
/// them so every describe renders one the same way.
pub fn timestamp_filter_values(at: Option<::aws_smithy_types::DateTime>) -> Vec<Cow<'static, str>> {
    at.and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
        .map(Cow::Owned)
        .into_iter()
        .collect()
}

/// One move of capacity between two Capacity Reservations, as
/// `MoveCapacityReservationInstances` made it.
///
/// Held against the `ClientToken` the move was asked under
/// ([`ComAmazonawsEc2Amazonec2State::capacity_reservation_move_client_tokens`]) so a repeat
/// of the same request reports the same move instead of moving the capacity twice, and a
/// DIFFERENT request under a token already in use is told so rather than served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityReservationInstanceMove {
    /// "The ID of the Capacity Reservation from which you want to move capacity"
    /// (`MoveCapacityReservationInstancesRequest$SourceCapacityReservationId`).
    pub source_capacity_reservation_id: String,
    /// "The ID of the Capacity Reservation that you want to move capacity into"
    /// (`$DestinationCapacityReservationId`).
    pub destination_capacity_reservation_id: String,
    /// "The number of instances that you want to move from the source Capacity Reservation"
    /// (`$InstanceCount`).
    pub instance_count: i32,
}

// =============================================================================
// Capacity Block offerings
// =============================================================================

/// The currency a Capacity Block offering is priced in: the only member of the modelled
/// `CurrencyCodeValues` enum (`smithy_query(query="shape",shape="CurrencyCodeValues")`), so
/// it is the one value `CapacityBlockOffering$CurrencyCode` can carry.
pub const CAPACITY_BLOCK_OFFERING_CURRENCY_CODE: &str = "USD";

/// "The tenancy of the Capacity Block" (`CapacityBlockOffering$Tenancy`): the `default`
/// member of the modelled `CapacityReservationTenancy` enum
/// (`smithy_query(query="shape",shape="CapacityReservationTenancy")`). The other member,
/// `dedicated`, is single-tenant hardware a caller asks for when it creates a Capacity
/// Reservation, and nothing in a Capacity Block offering search asks for it.
pub const CAPACITY_BLOCK_OFFERING_TENANCY: &str = "default";

/// The id prefix a Capacity Block offering id carries, in the `<prefix>-<payload>` form
/// every EC2 resource id takes; `cbo` is the prefix AWS gives an offering id of this
/// operation, beside [`CAPACITY_BLOCK_ID_PREFIX`] for the block a purchase mints.
pub const CAPACITY_BLOCK_OFFERING_ID_PREFIX: &str = "cbo";

/// The marker an offering id carries ahead of its checksum and terms, so an id a caller
/// assembled is detectable rather than read as terms — the same arrangement
/// [`PAGE_TOKEN_VERSION`] gives a pagination token.
pub(super) const CAPACITY_BLOCK_OFFERING_ID_VERSION: &str = "ec2-cbo-1";

/// What one reserved unit of a Capacity Block — one instance, or one EC2 UltraServer —
/// costs for one hour, in cents, as [`capacity_block_upfront_fee`] prices an offering.
///
/// CHOSEN, like [`MAX_INSTANCES_PER_RUN`]: "The total price to be paid up front"
/// (`CapacityBlockOffering$UpfrontFee`) is a function of AWS's price list, which neither the
/// model nor a request carries, so this emulator prices every reserved unit the same rather
/// than inventing a per-instance-type price list. What a caller can rely on is the RULE —
/// the fee is the reserved units times the reserved hours times one rate — not the rate.
pub const CAPACITY_BLOCK_FEE_CENTS_PER_UNIT_HOUR: u64 = 1_000;

/// "The total price to be paid up front" for `reserved_units` units held for
/// `duration_hours` hours, as `CapacityBlockOffering$UpfrontFee` carries it: a decimal
/// amount in [`CAPACITY_BLOCK_OFFERING_CURRENCY_CODE`], with the two fractional digits a
/// currency amount is written with.
///
/// The ONE definition of that charge, so every operation that reports an offering prices
/// one window the same way.
pub fn capacity_block_upfront_fee(reserved_units: u64, duration_hours: u64) -> String {
    let cents = reserved_units
        .saturating_mul(duration_hours)
        .saturating_mul(CAPACITY_BLOCK_FEE_CENTS_PER_UNIT_HOUR);
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// What one Capacity Block offering reserves: the terms a purchase of it would turn into a
/// stored Capacity Block.
///
/// An offering is not a stored resource — see the module docs — so the terms travel INSIDE
/// the offering id ([`encode_capacity_block_offering_id`]) rather than in a map keyed by it.
/// That is what lets `DescribeCapacityBlockOfferings` report offerings without writing
/// anything, while a purchase can still resolve the id a caller hands back to the window,
/// zone and counts the offering was reported with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityBlockOfferingTerms {
    /// "The instance type of the Capacity Block offering"
    /// (`CapacityBlockOffering$InstanceType`), or `None` for an offering of EC2
    /// UltraServers, which names an UltraServer type instead.
    pub instance_type: Option<String>,
    /// "The number of instances in the Capacity Block offering"
    /// (`CapacityBlockOffering$InstanceCount`).
    pub instance_count: Option<i32>,
    /// "The EC2 UltraServer type of the Capacity Block offering"
    /// (`CapacityBlockOffering$UltraserverType`), or `None` for an offering of plain
    /// instances.
    pub ultraserver_type: Option<String>,
    /// "The number of EC2 UltraServers in the offering"
    /// (`CapacityBlockOffering$UltraserverCount`).
    pub ultraserver_count: Option<i32>,
    /// The Availability Zone the reserved capacity sits in.
    pub availability_zone: String,
    /// That zone's id. The modelled `CapacityBlockOffering` carries no member for it, but
    /// the [`CapacityBlock`] a purchase mints does, so the offering's terms keep it.
    pub availability_zone_id: String,
    /// When the reserved window opens, in seconds since the Unix epoch.
    pub start_date_secs: i64,
    /// When it closes, in seconds since the Unix epoch.
    pub end_date_secs: i64,
}

/// The `CapacityBlockOfferingId` for `terms`: `cbo-<hex>` over the terms, this service's
/// offering-id marker and a checksum ([`encode_derived_id`] is that construction, shared with
/// every other derived id this service reports).
pub fn encode_capacity_block_offering_id(terms: &CapacityBlockOfferingTerms) -> String {
    encode_derived_id(
        CAPACITY_BLOCK_OFFERING_ID_PREFIX,
        CAPACITY_BLOCK_OFFERING_ID_VERSION,
        &capacity_block_offering_payload(terms),
    )
}

/// The terms an id minted by [`encode_capacity_block_offering_id`] carries; `None` for
/// anything else — an id that does not carry this service's prefix or marker, is not hex,
/// whose checksum does not match its terms, or whose terms are not the fields an offering
/// has.
pub fn decode_capacity_block_offering_id(offering_id: &str) -> Option<CapacityBlockOfferingTerms> {
    let fields = decode_derived_id(
        CAPACITY_BLOCK_OFFERING_ID_PREFIX,
        CAPACITY_BLOCK_OFFERING_ID_VERSION,
        offering_id,
        CAPACITY_BLOCK_OFFERING_FIELDS,
    )?;
    let terms = CapacityBlockOfferingTerms {
        start_date_secs: fields[0].parse().ok()?,
        end_date_secs: fields[1].parse().ok()?,
        instance_count: optional_count_field(&fields[2])?,
        ultraserver_count: optional_count_field(&fields[3])?,
        availability_zone: hex_field(&fields[4])?,
        availability_zone_id: hex_field(&fields[5])?,
        instance_type: optional_hex_field(&fields[6])?,
        ultraserver_type: optional_hex_field(&fields[7])?,
    };
    Some(terms)
}

/// The code EC2 answers for a value that is not a Capacity Block offering id at all.
pub const INVALID_CAPACITY_BLOCK_OFFERING_ID_MALFORMED: &str =
    "InvalidCapacityBlockOfferingIdMalformed";

/// Refuse a request that named something that is not a Capacity Block offering id: HTTP 400
/// [`INVALID_CAPACITY_BLOCK_OFFERING_ID_MALFORMED`].
///
/// Every offering id this service reports carries its own terms, this service's marker and a
/// checksum over both ([`encode_capacity_block_offering_id`]), so a value
/// [`decode_capacity_block_offering_id`] cannot read is not an id this service ever reported
/// an offering under — there is no offering for it to name, which is what "malformed" says.
/// There is no second, not-found half to tell it from: an offering is a search result rather
/// than a stored resource (see the module docs), so an id that decodes names the offering it
/// carries and one that does not names nothing.
pub fn invalid_capacity_block_offering_id_malformed<E>(
    capacity_block_offering_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_BLOCK_OFFERING_ID_MALFORMED,
        400,
        format!("Capacity Block Offering Id '{capacity_block_offering_id}' is malformed."),
    )
}

/// How many fields a Capacity Block offering id's payload carries.
pub(super) const CAPACITY_BLOCK_OFFERING_FIELDS: usize = 8;

/// The terms of one Capacity Block offering as the fields its id carries them as: the
/// instants as decimal, the counts and the text through the shared field writers.
pub(super) fn capacity_block_offering_payload(terms: &CapacityBlockOfferingTerms) -> Vec<String> {
    vec![
        terms.start_date_secs.to_string(),
        terms.end_date_secs.to_string(),
        derived_id_count_field(terms.instance_count),
        derived_id_count_field(terms.ultraserver_count),
        derived_id_text_field(Some(terms.availability_zone.as_str())),
        derived_id_text_field(Some(terms.availability_zone_id.as_str())),
        derived_id_text_field(terms.instance_type.as_deref()),
        derived_id_text_field(terms.ultraserver_type.as_deref()),
    ]
}

// -----------------------------------------------------------------------------
// The id a DERIVED resource is reported under
// -----------------------------------------------------------------------------

/// What separates the fields inside a derived id's payload. Every text field is hex-encoded
/// inside the payload ([`derived_id_text_field`]), so no field can carry the separator
/// whatever text it holds — see [`decode_derived_id`].
pub(super) const DERIVED_ID_FIELD_SEPARATOR: char = '|';

/// The id a derived resource is reported under: `<prefix>-<hex>` over `fields`, `marker` and
/// a checksum.
///
/// The ONE construction of such an id, for every kind of resource this service DERIVES rather
/// than stores — a Capacity Block offering, a Reserved Instances offering — so a change to the
/// layout or the checksum cannot drift between two kinds. Deriving the id FROM the fields is
/// what makes it stable — two searches that find the same thing report the same id, so a read
/// changes nothing a later caller sees — and resolvable, since [`decode_derived_id`] reads the
/// fields back out without a store. The checksum is the same non-secret one a pagination token
/// carries ([`page_token_checksum`]), for the same reason: it tells an id this service minted
/// from one a caller assembled, nothing more. `marker` says which kind of resource the fields
/// describe, so an id of one kind is never read as another's.
pub(super) fn encode_derived_id(prefix: &str, marker: &str, fields: &[String]) -> String {
    let payload = fields.join(&DERIVED_ID_FIELD_SEPARATOR.to_string());
    let checksum = page_token_checksum(&payload);
    let body = format!("{marker}:{checksum:016x}:{payload}");
    format!("{prefix}-{}", ::hex::encode(body.as_bytes()))
}

/// The `fields` an id minted by [`encode_derived_id`] carries; `None` for anything else — an
/// id that does not carry `prefix` or `marker`, is not hex, whose checksum does not match its
/// payload, or that does not carry exactly `fields` fields.
pub(super) fn decode_derived_id(
    prefix: &str,
    marker: &str,
    id: &str,
    fields: usize,
) -> Option<Vec<String>> {
    let hexed = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))?;
    let decoded = String::from_utf8(::hex::decode(hexed).ok()?).ok()?;
    let marked = decoded
        .strip_prefix(marker)
        .and_then(|rest| rest.strip_prefix(':'))?;
    let (checksum, payload) = marked.split_once(':')?;
    if checksum != format!("{:016x}", page_token_checksum(payload)) {
        return None;
    }
    // Every text field is itself hex, so no byte a request chose can carry the separator and
    // shift the fields the service derived; the field count is therefore exact.
    let carried: Vec<String> = payload
        .split(DERIVED_ID_FIELD_SEPARATOR)
        .map(str::to_string)
        .collect();
    (carried.len() == fields).then_some(carried)
}

/// One text field of a derived id, as the hex its payload carries it as, so the field is
/// separator-free whatever the text holds. An absent member is the empty field.
pub(super) fn derived_id_text_field(value: Option<&str>) -> String {
    ::hex::encode(value.unwrap_or_default().as_bytes())
}

/// One count field of a derived id, as decimal. An absent member is the empty field.
pub(super) fn derived_id_count_field(value: Option<i32>) -> String {
    value.map(|count| count.to_string()).unwrap_or_default()
}

/// One count field read back out of a derived id: an empty field is a member the resource does
/// not carry, anything that is not a number at all makes the whole id unreadable.
pub(super) fn optional_count_field(field: &str) -> Option<Option<i32>> {
    if field.is_empty() {
        return Some(None);
    }
    field.parse().ok().map(Some)
}

/// One text field read back out of a derived id: the text its hex carries. A field that is not
/// hex, or not UTF-8 under it, makes the whole id unreadable.
pub(super) fn hex_field(field: &str) -> Option<String> {
    String::from_utf8(::hex::decode(field).ok()?).ok()
}

/// One optional text field read back out of a derived id: an empty field is a member the
/// resource does not carry. The outer `None` is an unreadable field, the inner one an absent
/// member.
pub(super) fn optional_hex_field(field: &str) -> Option<Option<String>> {
    if field.is_empty() {
        return Some(None);
    }
    hex_field(field).map(Some)
}

// =============================================================================
// Windows fast launch
// =============================================================================

/// `FastLaunchStateCode`'s `enabling`: Amazon EC2 "launches an instance and runs through
/// Sysprep steps … then it creates a set of reserved snapshots" (`EnableFastLaunch`), and
/// the AMI cannot launch from them yet
/// (`smithy_query(query="shape",shape="FastLaunchStateCode")`).
pub const FAST_LAUNCH_STATE_ENABLING: &str = "enabling";

/// `FastLaunchStateCode`'s `enabled`: the pre-provisioned snapshots are in place.
pub const FAST_LAUNCH_STATE_ENABLED: &str = "enabled";

/// `FastLaunchStateCode`'s `disabling`: a caller has asked to "discontinue Windows fast
/// launch for a Windows AMI, and clean up existing pre-provisioned snapshots"
/// (`DisableFastLaunch`), and that cleanup is not finished.
pub const FAST_LAUNCH_STATE_DISABLING: &str = "disabling";

/// The one member of `FastLaunchResourceType`, and "the default value" when a request
/// names none (`EnableFastLaunchRequest$ResourceType`).
pub const FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT: &str = "snapshot";

/// "Value must be `6` or greater" (`EnableFastLaunchRequest$MaxParallelLaunches`).
///
/// Also what a request that names no maximum gets: every operation that reports a
/// configuration reports a `MaxParallelLaunches`, and the floor the member documents is the
/// only value the service has been told anything about.
pub const MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES: i32 = 6;

/// The smallest `TargetResourceCount` a configuration may ask for: "the number of
/// pre-provisioned snapshots to keep on hand for a Windows fast launch enabled AMI"
/// (`FastLaunchSnapshotConfigurationRequest$TargetResourceCount`) is a count of snapshots
/// kept on hand, so a configuration that keeps any keeps at least one. The model declares
/// no `@range` for the member, so a handler is the only place the floor can be enforced.
pub const MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT: i32 = 1;

/// The code EC2 answers when the resource a request names exists but is not in a state
/// that admits the request.
pub const INCORRECT_STATE: &str = "IncorrectState";

/// The code EC2 answers when a request is invalid in a way no member-specific code of its
/// own covers.
pub const INVALID_REQUEST: &str = "InvalidRequest";

/// The code EC2 answers for a launch template version the named template does not hold.
pub const INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND: &str =
    "InvalidLaunchTemplateId.VersionNotFound";

/// The `Version` value that names a launch template's latest version: "valid values are
/// $Latest, $Default, and numbers"
/// (`DescribeLaunchTemplateVersionsRequest$Versions`).
pub const LAUNCH_TEMPLATE_VERSION_LATEST: &str = "$Latest";

/// The `Version` value that names a launch template's default version, from the same
/// sentence as [`LAUNCH_TEMPLATE_VERSION_LATEST`].
pub const LAUNCH_TEMPLATE_VERSION_DEFAULT: &str = "$Default";

/// Refuse a request the service answers no member-specific code for: HTTP 400
/// `InvalidRequest`.
pub fn invalid_request<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_REQUEST, 400, message)
}

/// Refuse a request whose resource exists but is not in a state that admits it: HTTP 400
/// `IncorrectState`.
pub fn incorrect_state<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INCORRECT_STATE, 400, message)
}

/// Refuse a request whose resource exists but is not in a state that admits it, with
/// EC2's own wording for the code: the KIND of resource, the id the request named and the
/// state that resource is in, and nothing more — `Volume 'vol-…' is in the 'available'
/// state.`
///
/// The message is built from the request and the store, so it is the RULE that is fixed
/// here and not any one message: `<Kind> '<id>' is in the '<state>' state.` It says what
/// the caller has to change (the state of that resource) and nothing about which operation
/// asked, which is why every operation refusing SOLELY for the state of the resource it
/// names shares it. An operation whose refusal also names which states DO admit the
/// request, or whose resource is named by more than one id, keeps its own wording rather
/// than dropping what that message carries.
pub fn incorrect_state_of<E>(kind: &str, id: &str, state: &str) -> Scaffolded<E> {
    incorrect_state(format!("{kind} '{id}' is in the '{state}' state."))
}

/// What the clock owes a stored Windows fast launch configuration next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLaunchMove {
    /// `enabling` -> `enabled`: the reserved snapshots an enable started are in place.
    Enabled,
    /// `disabling` -> reported no longer: the cleanup a disable started is finished, so
    /// the image is not a fast launch enabled image any more.
    Discontinued,
}

/// A move the clock owes one stored resource, with the instants it falls due on.
///
/// ONE definition for every resource kind whose next state is owed to the clock rather than
/// to a caller: the `T` is what that kind becomes ([`FastLaunchMove`] for a Windows fast
/// launch configuration, a `&'static str` catalog word for a VPC BPA exclusion). An
/// instance's [`PendingTransition`] is deliberately NOT one of these — it carries no
/// wall-clock instant, an instance reporting no transition time.
#[derive(Debug, Clone)]
pub struct ClockTransition<T> {
    /// What the resource becomes.
    pub to: T,
    /// The instant from which the move is due. Monotonic ([`Instant`]), because it is only
    /// ever compared against the clock and never reported, so a wall-clock adjustment
    /// cannot move a resource back to the state it is leaving.
    pub due_at: Instant,
    /// The wall-clock instant the move falls due, which the resource reports as the time it
    /// reached the new state once the move has been applied — the state changed when the
    /// move fell due, not when somebody next asked about it.
    pub at: SystemTime,
}

impl<T> ClockTransition<T> {
    /// A move to `to`, due [`TRANSITION_DELAY`] from now: the span this crate gives every
    /// clock-owed transition, recorded rather than slept for.
    pub fn starting_now(to: T) -> Self {
        Self::due_in(to, TRANSITION_DELAY)
    }

    /// A move to `to`, due `delay` from now.
    ///
    /// One span ([`TRANSITION_DELAY`]) is what a resource whose readiness depends on
    /// nothing else takes; a resource that provisions OTHERS reaches its own state only
    /// once those are ready, which is one further span per level it depends on — so the
    /// delay is a multiple of that one span rather than a number of its own.
    pub fn due_in(to: T, delay: Duration) -> Self {
        Self {
            to,
            due_at: Instant::now() + delay,
            at: SystemTime::now() + delay,
        }
    }
}

// ---------------------------------------------------------------------------
// BYOIP address ranges (`ByoipCidr`), part 2: what a provision is held to, and
// the stored record. Continues the `part 1` banner above.
// ---------------------------------------------------------------------------

/// The most specific IPv4 prefix a BYOIP range may have: "The most specific IPv4 prefix
/// that you can specify is /24" (`ProvisionByoipCidrRequest$Cidr`).
pub const MOST_SPECIFIC_BYOIP_IPV4_PREFIX: u8 = 24;

/// The most specific IPv6 prefix a publicly advertisable BYOIP range may have: "The most
/// specific IPv6 address range that you can bring is /48 for CIDRs that are publicly
/// advertisable" (`ProvisionByoipCidrRequest$Cidr`).
pub const MOST_SPECIFIC_BYOIP_IPV6_PREFIX: u8 = 48;

/// The most specific IPv6 prefix a range that is NOT publicly advertisable may have: "…
/// and /56 for CIDRs that are not publicly advertisable"
/// (`ProvisionByoipCidrRequest$Cidr`).
pub const MOST_SPECIFIC_BYOIP_IPV6_PREFIX_NOT_PUBLICLY_ADVERTISABLE: u8 = 56;

/// "(IPv6 only) Indicate whether the address range will be publicly advertised to the
/// internet. Default: true" (`ProvisionByoipCidrRequest$PubliclyAdvertisable`).
pub const DEFAULT_PUBLICLY_ADVERTISABLE: bool = true;

/// The Local Zone network border groups a BYOIPv4 range may be provisioned to: "You can
/// provision BYOIP address ranges to and advertise them in the following Local Zone
/// network border groups: us-east-1-dfw-2 us-west-2-lax-1 us-west-2-phx-2"
/// (`ProvisionByoipCidrRequest$NetworkBorderGroup`).
pub const LOCAL_ZONE_NETWORK_BORDER_GROUPS: [&str; 3] =
    ["us-east-1-dfw-2", "us-west-2-lax-1", "us-west-2-phx-2"];

/// May a range of `family` be provisioned to the network border group `group` from
/// `region`?
///
/// The Region a request was signed for is always a network border group of its own — it is
/// the group a request that names none gets — and beside it a BYOIPv4 range may name one of
/// the Region's own Local Zone groups ([`LOCAL_ZONE_NETWORK_BORDER_GROUPS`], each of which
/// starts with its parent Region's name). "You cannot provision or advertise BYOIPv6
/// address ranges in Local Zones at this time"
/// (`ProvisionByoipCidrRequest$NetworkBorderGroup`), so for IPv6 only the Region itself
/// holds.
pub fn byoip_network_border_group_allowed(
    group: &str,
    region: &str,
    family: AddressFamily,
) -> bool {
    group == region
        || (family == AddressFamily::Ipv4
            && group.starts_with(region)
            && LOCAL_ZONE_NETWORK_BORDER_GROUPS.contains(&group))
}

/// The error code a provision past the store's record ceiling carries, in EC2's own
/// `<Resource>LimitExceeded` naming — the same convention as [`VPC_LIMIT_EXCEEDED`],
/// [`SUBNET_LIMIT_EXCEEDED`] and [`VOLUME_LIMIT_EXCEEDED`], each of which is the code the
/// real service answers that resource's quota with.
pub const BYOIP_CIDR_LIMIT_EXCEEDED: &str = "ByoipCidrLimitExceeded";

/// Refuse a provision that would take the store past the BYOIP record ceiling
/// ([`ComAmazonawsEc2Amazonec2State::has_byoip_cidr_capacity`]): HTTP 400
/// [`BYOIP_CIDR_LIMIT_EXCEEDED`].
pub fn byoip_cidr_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        BYOIP_CIDR_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of BYOIP address ranges has been reached \
             ({MAX_STORED_RECORDS_PER_STORE})"
        ),
    )
}

/// Why a requested BYOIP address range cannot be provisioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoipCidrProblem {
    /// The value is not a CIDR block of either address family.
    Malformed,
    /// The block parses, but its prefix is more specific than the most specific one the
    /// family (and, for IPv6, the advertisability) admits — the carried bound.
    PrefixLength(u8),
    /// The block parses, but it is not public address space, so it cannot be an address
    /// range brought to AWS: `ProvisionByoipCidrRequest$Cidr` is "The public IPv4 or IPv6
    /// address range".
    NotPublic,
}

/// One readable BYOIP address range: the block in canonical form, and its family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByoipCidrBlock {
    /// The block with its host bits cleared, the form this crate stores every CIDR in.
    pub cidr: String,
    /// Which address space it names, which decides the pool's resource type and the
    /// prefix bound it was held to.
    pub family: AddressFamily,
}

/// A requested BYOIP address range, read the one way this crate reads a CIDR block and
/// held to what `ProvisionByoipCidrRequest$Cidr` publishes for one: a PUBLIC IPv4 or IPv6
/// range, no more specific than /24 for IPv4, /48 for a publicly advertisable IPv6 range
/// and /56 for one that is not.
///
/// The canonical form is the one the store keys its ranges by ([`canonical_byoip_cidr`]);
/// what this adds is the published bounds a PROVISION is held to, which a range already in
/// the store is not re-checked against.
pub fn provisionable_byoip_cidr(
    value: &str,
    publicly_advertisable: bool,
) -> Result<ByoipCidrBlock, ByoipCidrProblem> {
    let family = AddressFamily::of_cidr(value);
    let canonical = canonical_byoip_cidr(value).map_err(|_| ByoipCidrProblem::Malformed)?;
    let (prefix_length, most_specific) = match family {
        AddressFamily::Ipv4 => {
            let (_, prefix_length) =
                split_ipv4_cidr(&canonical).ok_or(ByoipCidrProblem::Malformed)?;
            (prefix_length, MOST_SPECIFIC_BYOIP_IPV4_PREFIX)
        }
        AddressFamily::Ipv6 => {
            let (_, prefix_length) =
                split_ipv6_cidr(&canonical).ok_or(ByoipCidrProblem::Malformed)?;
            let most_specific = if publicly_advertisable {
                MOST_SPECIFIC_BYOIP_IPV6_PREFIX
            } else {
                MOST_SPECIFIC_BYOIP_IPV6_PREFIX_NOT_PUBLICLY_ADVERTISABLE
            };
            (prefix_length, most_specific)
        }
    };
    if prefix_length > most_specific {
        return Err(ByoipCidrProblem::PrefixLength(most_specific));
    }
    if !cidr_block_is_public(&canonical) {
        return Err(ByoipCidrProblem::NotPublic);
    }
    Ok(ByoipCidrBlock {
        cidr: canonical,
        family,
    })
}

/// Every IPv4 block the IANA IPv4 Special-Purpose Address Registry (RFC 6890) marks as
/// NOT globally reachable: this host, private use, shared address space, loopback,
/// link local, protocol assignments, the three documentation blocks, the AS112 and AMT
/// assignments, benchmarking, 6to4 relay anycast, multicast and reserved space.
///
/// An address range brought to AWS is "The public IPv4 or IPv6 address range"
/// (`ProvisionByoipCidrRequest$Cidr`) — a range the account holds a registry allocation and
/// an RPKI ROA for — so a block that meets any of these cannot be one.
pub(super) const NON_PUBLIC_IPV4_BLOCKS: [&str; 18] = [
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.0.2.0/24",
    "192.31.196.0/24",
    "192.52.193.0/24",
    "192.88.99.0/24",
    "192.168.0.0/16",
    "192.175.48.0/24",
    "198.18.0.0/15",
    "198.51.100.0/24",
    "203.0.113.0/24",
    "224.0.0.0/4",
    "240.0.0.0/4",
];

/// Every IPv6 block the IANA IPv6 Special-Purpose Address Registry marks as not globally
/// reachable while sitting INSIDE global unicast space: Teredo, benchmarking, ORCHIDv2 and
/// the two documentation blocks, plus 6to4. Everything outside global unicast is excluded
/// by [`GLOBAL_UNICAST_IPV6_BLOCK`] instead.
pub(super) const NON_PUBLIC_IPV6_BLOCKS: [&str; 6] = [
    "2001::/32",
    "2001:2::/48",
    "2001:20::/28",
    "2001:db8::/32",
    "2002::/16",
    "3fff::/20",
];

/// The one global unicast IPv6 block: IANA's IPv6 Global Unicast Address Assignments give
/// `2000::/3` as the only space handed to the registries, so a BYOIPv6 range has to be
/// inside it.
pub(super) const GLOBAL_UNICAST_IPV6_BLOCK: &str = "2000::/3";

/// Is every address of `block` public address space — space a registry allocates, as
/// opposed to the special-purpose blocks no one can bring to AWS?
///
/// A block that is only PARTLY public is not: a caller cannot hold an allocation for a
/// range that reaches into reserved space, which is why this asks about overlap rather
/// than about the block's base address.
pub fn cidr_block_is_public(block: &str) -> bool {
    match AddressFamily::of_cidr(block) {
        AddressFamily::Ipv4 => NON_PUBLIC_IPV4_BLOCKS
            .iter()
            .all(|reserved| !ipv4_cidr_blocks_overlap(block, reserved)),
        AddressFamily::Ipv6 => {
            ipv6_cidr_block_contains(GLOBAL_UNICAST_IPV6_BLOCK, block)
                && NON_PUBLIC_IPV6_BLOCKS
                    .iter()
                    .all(|reserved| !ipv6_cidr_blocks_overlap(block, reserved))
        }
    }
}

/// `block` as its base address and prefix length, for a block already in canonical form —
/// the IPv6 counterpart of [`split_ipv4_cidr`].
pub(super) fn split_ipv6_cidr(block: &str) -> Option<(u128, u8)> {
    let (address, prefix_length) = block.split_once('/')?;
    let base = u128::from(address.parse::<Ipv6Addr>().ok()?);
    let prefix_length: u8 = prefix_length.parse().ok()?;
    (prefix_length <= 128).then_some((base, prefix_length))
}

/// The first and last address of `block`, or `None` when it is not an IPv6 CIDR block.
pub(super) fn ipv6_cidr_bounds(block: &str) -> Option<(u128, u128)> {
    let (base, prefix_length) = split_ipv6_cidr(block)?;
    let size = 1u128
        .checked_shl(u32::from(128 - prefix_length))
        .unwrap_or_default();
    // A /0 spans the whole space, whose size does not fit a u128; its last address is the
    // largest one there is.
    let last = match size {
        0 => u128::MAX,
        size => base.checked_add(size - 1)?,
    };
    Some((base, last))
}

/// Do `left` and `right` share any IPv6 address? The IPv6 counterpart of
/// [`ipv4_cidr_blocks_overlap`], on the same terms: a block this emulator cannot read
/// overlaps nothing.
pub fn ipv6_cidr_blocks_overlap(left: &str, right: &str) -> bool {
    match (ipv6_cidr_bounds(left), ipv6_cidr_bounds(right)) {
        (Some((left_first, left_last)), Some((right_first, right_last))) => {
            left_first <= right_last && right_first <= left_last
        }
        _ => false,
    }
}

/// Is every address of `inner` an address of `outer`? The IPv6 counterpart of
/// [`ipv4_cidr_block_contains`].
pub fn ipv6_cidr_block_contains(outer: &str, inner: &str) -> bool {
    match (ipv6_cidr_bounds(outer), ipv6_cidr_bounds(inner)) {
        (Some((outer_first, outer_last)), Some((inner_first, inner_last))) => {
            outer_first <= inner_first && inner_last <= outer_last
        }
        _ => false,
    }
}

/// Do `left` and `right` share an address, whatever family they are?
///
/// Two blocks of DIFFERENT families share nothing — an IPv4 address is not an IPv6 one —
/// so the question is only ever asked within one family, and this is the one reader that
/// picks which. It is what "The address range cannot overlap with another address range
/// that you've brought to this or another Region" (`ProvisionByoipCidrRequest$Cidr`) asks.
pub fn cidr_blocks_overlap(left: &str, right: &str) -> bool {
    match (AddressFamily::of_cidr(left), AddressFamily::of_cidr(right)) {
        (AddressFamily::Ipv4, AddressFamily::Ipv4) => ipv4_cidr_blocks_overlap(left, right),
        (AddressFamily::Ipv6, AddressFamily::Ipv6) => ipv6_cidr_blocks_overlap(left, right),
        _ => false,
    }
}

/// What a provision stores: everything about one BYOIP range that comes from the request,
/// read and validated by `ProvisionByoipCidr` before the store is touched.
#[derive(Debug, Clone)]
pub struct ByoipCidrSpec {
    /// The range, canonical, as [`provisionable_byoip_cidr`] read it.
    pub block: ByoipCidrBlock,
    /// "A description for the address range and the address pool"
    /// (`ProvisionByoipCidrRequest$Description`).
    pub description: Option<String>,
    /// The network border group the range is advertised from: the one the request named,
    /// or the Region it was signed for.
    pub network_border_group: String,
    /// Whether the range "will be publicly advertised to the internet"
    /// (`ProvisionByoipCidrRequest$PubliclyAdvertisable`).
    pub publicly_advertisable: bool,
    /// "The tags to apply to the address pool"
    /// (`ProvisionByoipCidrRequest$PoolTagSpecifications`), already held to the bounds every
    /// create holds its tags to. They go on the address pool the provision creates, which is
    /// the only resource this action tags.
    pub pool_tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// The BYOIP range the store holds that shares an address with `cidr`, if any.
    ///
    /// "The address range cannot overlap with another address range that you've brought to
    /// this or another Region" (`ProvisionByoipCidrRequest$Cidr`) — asked over the whole
    /// store, so a range that merely CONTAINS or sits inside a provisioned one is caught
    /// as well as an exact repeat.
    pub fn byoip_cidr_overlapping(&self, cidr: &str) -> Option<&str> {
        self.byoip_cidrs
            .keys()
            .find(|held| cidr_blocks_overlap(cidr, held))
            .map(String::as_str)
    }

    /// Store `spec` as a newly provisioned BYOIP range and answer it as the provision
    /// reports it — `pending-provision`, the state the clock owes `provisioned`
    /// [`TRANSITION_DELAY`] from now ([`ByoipCidrRecord::pending`]).
    ///
    /// The pool the range gets ("creates a corresponding address pool",
    /// `ProvisionByoipCidr`) is minted here rather than taken from the request, so no caller
    /// can choose a pool id. For an IPv4 range that pool is a resource this store HOLDS — a
    /// [`PublicIpv4PoolRecord`] carrying the range and the request's pool tags, which
    /// `DescribePublicIpv4Pools` reports and `DeprovisionPublicIpv4PoolCidr` takes the range
    /// back out of; for an IPv6 range it is an [`Ipv6PoolRecord`] under an `ipv6pool-ec2-…`
    /// id, carrying the same facts, which `DescribeIpv6Pools` reports.
    ///
    /// The record goes in through [`Self::insert_byoip_cidr`], the one write path, which
    /// charges the bytes it holds and REFUSES a record that does not fit the store's ceilings
    /// — so `None`, with nothing stored, rather than a range reported to a caller that the
    /// store does not hold.
    pub fn provision_byoip_cidr(&mut self, spec: ByoipCidrSpec) -> Option<ByoipCidr> {
        let pool_id = match spec.block.family {
            AddressFamily::Ipv4 => self.insert_public_ipv4_pool(PublicIpv4PoolSpec {
                // "A description for the address range AND THE ADDRESS POOL"
                // (`ProvisionByoipCidrRequest$Description`): the one value describes both.
                description: spec.description.clone(),
                network_border_group: spec.network_border_group.clone(),
                tags: spec.pool_tags.clone(),
                cidrs: vec![spec.block.cidr.clone()],
            })?,
            AddressFamily::Ipv6 => self.insert_ipv6_pool(Ipv6PoolSpec {
                // The one description describes both, exactly as it does for the pool of
                // the other family above.
                description: spec.description.clone(),
                tags: spec.pool_tags.clone(),
                cidrs: vec![spec.block.cidr.clone()],
            })?,
        };
        let becomes = match (spec.block.family, spec.publicly_advertisable) {
            (AddressFamily::Ipv6, false) => BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
            _ => BYOIP_CIDR_STATE_PROVISIONED,
        };
        let record = ByoipCidrRecord {
            byoip_cidr: ByoipCidr {
                cidr: Some(spec.block.cidr.clone()),
                description: spec.description,
                // "The BYOIP CIDR associations with ASNs" (`ByoipCidr$AsnAssociations`):
                // none until `AssociateIpamByoasn` makes one.
                asn_associations: Some(Vec::new()),
                // "Upon success, contains the ID of the address pool"
                // (`ByoipCidr$StatusMessage`) — the provision has not succeeded yet, so
                // there is nothing to report until the clock says it has.
                status_message: None,
                state: Some(BYOIP_CIDR_STATE_PENDING_PROVISION.to_string()),
                network_border_group: Some(spec.network_border_group),
                // `ByoipCidr$AdvertisementType` is how the range IS advertised, and this
                // one is not advertised yet — `AdvertiseByoipCidr` is what starts that.
                advertisement_type: None,
                pool_id: Some(pool_id.clone()),
                // "The ID of the IPAM pool associated with the CIDR"
                // (`ByoipCidr$IpamPoolId`): a range provisioned through this operation is
                // not in an IPAM pool; `MoveByoipCidrToIpam` is what puts one there.
                ipam_pool_id: None,
            },
            released_at: None,
            // What the clock now owes this range, recorded rather than slept for: "the
            // address range is not ready to use until its status changes from
            // pending-provision to provisioned" (`ProvisionByoipCidr`), one
            // [`TRANSITION_DELAY`] from now, off the same monotonic clock as every other
            // move this store owes. WHAT it becomes is decided by the REQUEST — an IPv6
            // range the caller asked not to have advertised lands in
            // `provisioned-not-publicly-advertisable` — so it is recorded here rather than
            // guessed at the due instant.
            pending: Some(PendingStateMove::starting_now(becomes)),
        };
        let reported = record.byoip_cidr.clone();
        if !self.insert_byoip_cidr(record) {
            // A range the store would not hold is not reported, so the pool created a moment
            // ago for it goes back too: a pool whose range was never provisioned is no pool
            // of this account, and its bytes return to the budget. The id names a pool of
            // ONE of the two families, so at most one of these removals finds anything.
            self.remove_public_ipv4_pool(&pool_id);
            self.remove_ipv6_pool(&pool_id);
            return None;
        }
        Some(reported)
    }
}

/// The move the clock owes one Windows fast launch configuration: a [`ClockTransition`] to
/// the next [`FastLaunchMove`], reported as the configuration's `StateTransitionTime`.
pub type FastLaunchTransition = ClockTransition<FastLaunchMove>;

/// One image's Windows fast launch configuration: the modelled
/// [`DescribeFastLaunchImagesSuccessItem`] every operation that reports one answers with,
/// plus the move the clock owes it.
///
/// `EnableFastLaunch`, `DisableFastLaunch` and `DescribeFastLaunchImages` answer with the
/// same member set, so the one stored item is what all three report and they cannot
/// describe one configuration differently.
#[derive(Debug, Clone)]
pub struct FastLaunchRecord {
    /// The configuration as the model shapes it, carrying the state reached so far.
    pub view: DescribeFastLaunchImagesSuccessItem,
    /// The move the clock owes this configuration, while one is owed.
    pub pending: Option<FastLaunchTransition>,
}

impl FastLaunchRecord {
    /// The configuration's `FastLaunchStateCode` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// This configuration as `EnableFastLaunch` reports it.
    pub fn enable_response(&self) -> EnableFastLaunchOutput {
        let view = self.view.clone();
        EnableFastLaunchOutput {
            image_id: view.image_id,
            resource_type: view.resource_type,
            snapshot_configuration: view.snapshot_configuration,
            launch_template: view.launch_template,
            max_parallel_launches: view.max_parallel_launches,
            owner_id: view.owner_id,
            state: view.state,
            state_transition_reason: view.state_transition_reason,
            state_transition_time: view.state_transition_time,
        }
    }

    /// This configuration as `DisableFastLaunch` reports it.
    pub fn disable_response(&self) -> DisableFastLaunchOutput {
        let view = self.view.clone();
        DisableFastLaunchOutput {
            image_id: view.image_id,
            resource_type: view.resource_type,
            snapshot_configuration: view.snapshot_configuration,
            launch_template: view.launch_template,
            max_parallel_launches: view.max_parallel_launches,
            owner_id: view.owner_id,
            state: view.state,
            state_transition_reason: view.state_transition_reason,
            state_transition_time: view.state_transition_time,
        }
    }
}

/// Can the calling account see `image` at all?
///
/// "The images available to you include public images, private images that you own, and
/// private images owned by other Amazon Web Services accounts for which you have explicit
/// launch permissions" (`DescribeImages`). Nothing in this slice grants an explicit launch
/// permission — `ModifyImageAttribute` is still a scaffold — so the images available are
/// the public ones and the account's own, and a private image of another account is
/// invisible however a request asks for it.
pub fn image_is_visible(image: &Image, account: &str) -> bool {
    image.public.unwrap_or(false) || image.owner_id.as_deref() == Some(account)
}

/// Can the calling account see the image `image_id` at all — does the store hold an image
/// under that id that [`image_is_visible`] admits?
pub fn account_can_see_image(store: &ComAmazonawsEc2Amazonec2State, image_id: &str) -> bool {
    let account = ::engine::emulator_account_id();
    store
        .images
        .get(image_id)
        .is_some_and(|image| image_is_visible(image, account))
}

/// Refuse an image id a request named that the calling account cannot resolve: something
/// that is not an AMI id at all is malformed, and a well-formed id the account can see no
/// image for does not exist — the order EC2 answers the two in.
pub fn require_visible_image<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !image_id_is_well_formed(image_id) {
        return Err(invalid_ami_id_malformed(image_id));
    }
    if !account_can_see_image(store, image_id) {
        return Err(invalid_ami_id_not_found(image_id));
    }
    Ok(())
}

/// Refuse an AMI id an operation with EC2's STRICT image-id validator cannot resolve: HTTP
/// 400 [`INVALID_AMI_ID_MALFORMED`], `The image ID '<id>' is malformed`.
///
/// EC2's per-operation validators do NOT agree about an id they cannot resolve. The strict
/// ones answer this one code and wording for ANY value that does not name an image this
/// account can see — including a value in the long `ami-` id form that names none — where
/// the describes of the account's images draw the distinction the shared
/// [`require_visible_image`] keeps: [`invalid_ami_id_malformed`] for a value that is no AMI
/// id at all and [`invalid_ami_id_not_found`] for a well-formed id no visible image has.
/// Two recorded units pin the strict answer, `ec2-image-store-restore-error-paths#task#1` for
/// `CreateStoreImageTask` and the recorded `DescribeImageAttribute` refusal, so the wording
/// lives here rather than beside either caller. The message's one variable part is the id the
/// request sent. `GetImageAncestry` words its MALFORMED refusal from here too while keeping the
/// `.NotFound` of a well-formed id no visible image has, so a caller of it still tells the two
/// apart.
pub fn unresolvable_image_id<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_ID_MALFORMED,
        400,
        format!("The image ID '{image_id}' is malformed"),
    )
}

/// Resolve the image an operation with EC2's strict image-id validator names, or refuse the
/// id with [`unresolvable_image_id`].
///
/// The images an account may name are the ones it can see at all, read through the crate's
/// one definition of that question ([`image_is_visible`]), so a strict operation and the
/// describes agree about which AMIs exist for a caller. The resolved image is handed back,
/// so a caller that needs a fact of it reads it once rather than looking the same id up
/// again.
pub fn require_resolvable_image<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<&'store Image, Scaffolded<E>> {
    store
        .images
        .get(image_id)
        .filter(|image| image_is_visible(image, ::engine::emulator_account_id()))
        .ok_or_else(|| unresolvable_image_id(image_id))
}

/// Refuse an image id a Windows fast launch change names that cannot carry one.
///
/// Either the account cannot resolve it ([`require_visible_image`]), or the image is not a
/// Windows AMI: Windows fast launch is a setting of "Windows AMIs that you own or that have
/// been shared with you" (`EnableFastLaunch`), and "this value is set to `windows` for
/// Windows AMIs; otherwise, it is blank" (`Image$Platform`) is what says which an image is.
/// The images this account may change are therefore its own and the public ones — the same
/// set [`image_is_visible`] answers, no image here carrying an explicit launch permission.
pub fn require_windows_image<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_visible_image(store, image_id)?;
    let is_windows = store
        .images
        .get(image_id)
        .and_then(|image| image.platform.as_deref())
        == Some(IMAGE_PLATFORM_WINDOWS);
    if !is_windows {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{image_id}' for imageId. \
             Windows fast launch is supported for Windows AMIs only"
        )));
    }
    Ok(())
}

/// Which VERSION of a launch template a request names: a number, or one of the two aliases.
///
/// The REQUEST-ONLY half of [`resolve_launch_template_version`]: "Valid values are $Latest,
/// $Default, and numbers" (`DescribeLaunchTemplateVersionsRequest$Versions`) is a rule about
/// the WORD the request sent, which asks nothing of the store — so an operation that has to
/// refuse an unreadable one before it answers a `DryRun` can read the word here and resolve it
/// against the template later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTemplateVersionRequest {
    /// `$Latest`: whichever version the template most recently gained.
    Latest,
    /// `$Default`: the version the template records as its default.
    Default,
    /// A version number, which the template must hold.
    Numbered(i64),
}

/// The version a request named, or the refusal for a word that is neither an alias nor a
/// number.
///
/// The ONE reading of that member, so every operation that takes a launch template version
/// refuses the same word the same way.
pub fn launch_template_version_request<E>(
    version: &str,
) -> Result<LaunchTemplateVersionRequest, Scaffolded<E>> {
    match version {
        LAUNCH_TEMPLATE_VERSION_LATEST => Ok(LaunchTemplateVersionRequest::Latest),
        LAUNCH_TEMPLATE_VERSION_DEFAULT => Ok(LaunchTemplateVersionRequest::Default),
        other => match other.parse::<i64>() {
            Ok(number) => Ok(LaunchTemplateVersionRequest::Numbered(number)),
            Err(_) => Err(invalid_parameter_value(format!(
                "Invalid value '{version}' for version. Valid values are \
                 {LAUNCH_TEMPLATE_VERSION_LATEST}, {LAUNCH_TEMPLATE_VERSION_DEFAULT}, \
                 and version numbers"
            ))),
        },
    }
}

/// Resolve the launch template version a request names against the template that holds it.
///
/// The word itself is read first ([`launch_template_version_request`]): an alias resolves to
/// the number the template records for it, and a number has to be a version the template
/// holds — a version that does not exist is `InvalidLaunchTemplateId.VersionNotFound`.
pub fn resolve_launch_template_version<E>(
    record: &LaunchTemplateRecord,
    version: &str,
) -> Result<i64, Scaffolded<E>> {
    let numbered = match launch_template_version_request(version)? {
        LaunchTemplateVersionRequest::Latest => record.template.latest_version_number,
        LaunchTemplateVersionRequest::Default => record.template.default_version_number,
        LaunchTemplateVersionRequest::Numbered(number) => Some(number),
    };
    numbered
        .filter(|number| record.versions.contains_key(number))
        .ok_or_else(|| {
            Scaffolded::service_error(
                INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND,
                400,
                format!(
                    "Launch template version '{version}' does not exist for launch \
                     template '{}'",
                    record.launch_template_id()
                ),
            )
        })
}

/// Refuse a launch that named a version the launch template does not hold, in the wording an
/// operation that LAUNCHES from the template answers: HTTP 400
/// [`INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND`], `Could not find the specified version
/// <version> for the launch template with ID <id>.`
///
/// A SECOND wording of that code beside the one [`resolve_launch_template_version`] carries,
/// and a named constructor rather than a literal at a call site for the reason
/// [`crate::state::specified_launch_template_id_not_found`] is one: EC2's per-operation
/// validators do not agree about a version they cannot resolve, and WHICH wording an
/// operation answers is a fact of that operation, recorded by its caller — the recorded
/// `RunInstances` naming a version its launch template never minted pins this sentence. The
/// two variable parts are the version the request named and the template's id, each cut to
/// length by [`echoed_resource_id`] the way every refusal of this crate bounds a value it
/// repeats back.
pub fn specified_launch_template_version_not_found<E>(
    launch_template_id: &str,
    version: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND,
        400,
        format!(
            "Could not find the specified version {} for the launch template with ID {}.",
            echoed_resource_id(version),
            echoed_resource_id(launch_template_id)
        ),
    )
}

// =============================================================================
// Elastic IP addresses
// =============================================================================

/// The one network an Elastic IP address is allocated into today: `DomainType`'s `vpc`
/// ("The network (`vpc`)" — `AllocateAddressRequest$Domain`, and the same sentence on
/// `Address$Domain`).
pub const ADDRESS_DOMAIN_VPC: &str = "vpc";

/// `DomainType`'s other member, the EC2-Classic network
/// (`smithy_query(query="shape",shape="DomainType")`). The model documents only `vpc` as
/// the network an address is allocated into, so this value names the network a request
/// can no longer ask for.
pub const ADDRESS_DOMAIN_STANDARD: &str = "standard";

/// The one resource type `AllocateAddress`'s `TagSpecifications` can name: "The tags to
/// assign to the Elastic IP address" (`AllocateAddressRequest$TagSpecifications`), which
/// is the `elastic-ip` member of `ResourceType`.
pub const ADDRESS_TAGGABLE_RESOURCE: &str = "elastic-ip";

/// The address pool an Amazon-provided Elastic IP address comes from: "Amazon's pool of
/// IPv4 addresses" (`AllocateAddress`). An address out of that pool carries no
/// `PublicIpv4Pool` id — the member is "The ID of an address pool that you own" — so this
/// is the value the pool members report as, and the describe filter compares against
/// nothing for such an address.
pub const AMAZON_ADDRESS_POOL: &str = "amazon";

/// How many Elastic IP addresses this emulator stores before `AllocateAddress` is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored address, so the store needs a ceiling that is not the process's
/// memory. It is well above the published per-Region Elastic IP quota (5 by default,
/// which is adjustable), so a test that allocates more than a real account's default
/// quota is not refused for a reason this emulator cannot have.
pub const MAX_STORED_ADDRESSES: usize = 4096;

/// How many candidate addresses [`ComAmazonawsEc2Amazonec2State::mint_elastic_ip`] probes
/// before giving up, so the search cannot spin when the block it draws from is full.
pub const MAX_ELASTIC_IP_PROBES: u32 = 4096;

/// `AddressLimitExceeded` (HTTP 400): the allocation would take the account past the
/// number of Elastic IP addresses it may hold.
pub const ADDRESS_LIMIT_EXCEEDED: &str = "AddressLimitExceeded";

/// `InvalidAddress.NotFound` (HTTP 400): the request named an Elastic IP address this
/// account holds no allocation for.
pub const INVALID_ADDRESS_NOT_FOUND: &str = "InvalidAddress.NotFound";

/// `InvalidAllocationID.NotFound` (HTTP 400): the request named an allocation id this
/// account holds no address for.
pub const INVALID_ALLOCATION_ID_NOT_FOUND: &str = "InvalidAllocationID.NotFound";

/// `InvalidAllocationID.Malformed` (HTTP 400): the request named something that is not an
/// allocation id at all, so there is nothing to look up.
pub const INVALID_ALLOCATION_ID_MALFORMED: &str = "InvalidAllocationID.Malformed";

/// `InvalidElasticIpID.NotFound` (HTTP 400): the request named an Elastic IP address id this
/// account holds no address for.
///
/// The SAME failure [`INVALID_ALLOCATION_ID_NOT_FOUND`] names, under the other of the two
/// codes EC2 publishes for it — the one that names the Elastic IP address id rather than the
/// allocation id. EC2's per-operation validators do not agree about which they answer, the
/// way they do not agree about an unresolvable instance id
/// ([`crate::scaffolded::Scaffolded::instance_id_is_malformed`]), so WHICH one an operation
/// answers is a fact of that operation, recorded by its caller:
/// `ModifyAddressAttribute` is recorded answering THIS one for a well-formed allocation id the
/// account holds no address for, while the release, the transfer and the attribute RESET of an
/// address keep [`INVALID_ALLOCATION_ID_NOT_FOUND`]. It is therefore not generalised to them.
pub const INVALID_ELASTIC_IP_ID_NOT_FOUND: &str = "InvalidElasticIpID.NotFound";

/// `Resource.AlreadyAssociated` (HTTP 400): the request asked for an association the
/// resource already has with something else, and did not allow that one to be replaced
/// ("you can specify false to ensure the operation fails if the Elastic IP address is
/// already associated with another resource" — `AssociateAddressRequest$AllowReassociation`).
pub const RESOURCE_ALREADY_ASSOCIATED: &str = "Resource.AlreadyAssociated";

/// `InvalidNetworkInterfaceID.NotFound` (HTTP 400): the request named a network interface
/// id this account holds no network interface for.
pub const INVALID_NETWORK_INTERFACE_ID_NOT_FOUND: &str = "InvalidNetworkInterfaceID.NotFound";

/// `InvalidNetworkInterfaceId.Malformed` (HTTP 400): the request named something that is
/// not a network interface id at all, so there is nothing to look up.
///
/// The casing is EC2's own and differs from [`INVALID_NETWORK_INTERFACE_ID_NOT_FOUND`]'s:
/// the syntax refusal spells the id `Id` while the lookup refusal spells it `ID`, which is
/// how EC2's own error-code list spells the two.
pub const INVALID_NETWORK_INTERFACE_ID_MALFORMED: &str = "InvalidNetworkInterfaceId.Malformed";

/// The id prefix of the association between an Elastic IP address and a resource, which
/// is what `ElasticIpAssociationId` names and `AssociateAddress` mints.
pub const ELASTIC_IP_ASSOCIATION_ID_PREFIX: &str = "eipassoc";

impl ComAmazonawsEc2Amazonec2State {
    /// The next unused Elastic IP address, or `None` when the search is exhausted.
    ///
    /// Drawn from `54.0.0.0/8`, a block Amazon advertises EC2 public addresses from, which
    /// is what "Amazon's pool of IPv4 addresses" (`AllocateAddress`) is here. The host
    /// part comes from the store's own monotonic counter and never from the request — a
    /// caller "cannot specify" which Amazon-pool address it gets — and the last octet skips
    /// `0` and `255`, which no interface is given. An address some allocation still holds
    /// is skipped, and the search probes at most [`MAX_ELASTIC_IP_PROBES`] candidates so it
    /// cannot spin.
    pub fn mint_elastic_ip(&mut self) -> Option<String> {
        for _ in 0..MAX_ELASTIC_IP_PROBES {
            self.next_elastic_ip = self.next_elastic_ip.wrapping_add(1);
            let host = self.next_elastic_ip;
            let candidate = format!(
                "54.{}.{}.{}",
                (host / (254 * 256)) % 256,
                (host / 254) % 256,
                1 + host % 254
            );
            if self.allocation_id_for_public_ip(&candidate).is_none() {
                return Some(candidate);
            }
        }
        None
    }

    /// The allocation id of the stored address whose `PublicIp` is `public_ip`, or `None`
    /// when the account holds no such address.
    ///
    /// A scan of the address store rather than a second map keyed by the address, so the
    /// public IP of an allocation is read off the one record that holds it; the store is
    /// bounded by [`MAX_STORED_ADDRESSES`], the way
    /// [`ComAmazonawsEc2Amazonec2State::launch_template_id_for_name`] scans the template
    /// store.
    pub fn allocation_id_for_public_ip(&self, public_ip: &str) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| address.public_ip.as_deref() == Some(public_ip))
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// The allocation id of the address currently associated with `instance_id`, or `None`
    /// when no address of the account is.
    ///
    /// Scanned off the address records themselves, like
    /// [`Self::allocation_id_for_public_ip`] and for the same reason: `Address$InstanceId`
    /// is where an association is stored, so there is no second copy of it to fall out of
    /// step. An instance holds at most one Elastic IP address — "if you associate an
    /// Elastic IP address with an instance that has an existing Elastic IP address, the
    /// existing address is disassociated from the instance" (`AssociateAddress`) — so the
    /// first match is the only one.
    pub fn allocation_id_associated_with_instance(&self, instance_id: &str) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| address.instance_id.as_deref() == Some(instance_id))
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// End `allocation_id`'s association, leaving the address ALLOCATED to the account:
    /// "the existing address is disassociated from the instance, but remains allocated to
    /// your account" (`AssociateAddress`).
    ///
    /// Shared because every path that ends an association does exactly this — the remap
    /// `AssociateAddress` performs, and the ones `DisassociateAddress` and `ReleaseAddress`
    /// perform. The INTERFACE the association was recorded on gives it up too
    /// ([`Self::clear_interface_elastic_ip_association`]), and so does the instance that
    /// holds that interface, which stops reporting the public address it carried through it
    /// (`Instance$PublicIpAddress`) — so no reader of the three stores can see an
    /// association the others no longer have. Storing nothing new, it makes no byte charge.
    ///
    /// A no-op for an allocation id the account holds no address for, so a caller does not
    /// have to look one up first.
    pub fn detach_elastic_ip(&mut self, allocation_id: &str) {
        let Some(address) = self.addresses.get_mut(allocation_id) else {
            return;
        };
        let public_ip = address.public_ip.clone();
        let was_on = address.instance_id.take();
        let was_on_interface = address.network_interface_id.take();
        address.association_id = None;
        address.network_interface_owner_id = None;
        address.private_ip_address = None;
        if let Some(network_interface_id) = was_on_interface {
            self.clear_interface_elastic_ip_association(&network_interface_id, allocation_id);
        }
        let Some(instance) = was_on.and_then(|instance_id| self.instances.get_mut(&instance_id))
        else {
            return;
        };
        // Only the address this association carried: an instance reporting some other
        // public address keeps it.
        if instance.public_ip == public_ip {
            instance.public_ip = None;
        }
    }

    /// Store a newly allocated Elastic IP address, charging the caller-chosen bytes it
    /// holds against the store's byte budget so no path can store them uncharged.
    pub fn insert_address(&mut self, address: Address) -> String {
        let allocation_id = address.allocation_id.clone().unwrap_or_default();
        self.add_stored_bytes(address_payload_bytes(&address));
        self.addresses.insert(allocation_id.clone(), address);
        allocation_id
    }

    /// Release the Elastic IP address `allocation_id`: the account no longer holds it, so
    /// the record leaves the store and is answered as it last was, or `None` when the
    /// account holds no address under that id.
    ///
    /// "After releasing an Elastic IP address, it is released to the IP address pool"
    /// (`ReleaseAddress`): the allocation is the account's claim on the address, so nothing
    /// of it may outlive the release — the association it may still hold is ended first
    /// ([`Self::detach_elastic_ip`], which also stops the instance reporting the address),
    /// and the transfer offer the account may have made of it goes too, because an offer of
    /// an address the account does not hold is an offer of nothing. The bytes the record
    /// held go back to the store's budget on exactly the terms
    /// [`address_payload_bytes`] took them on.
    pub fn remove_address(&mut self, allocation_id: &str) -> Option<Address> {
        if !self.addresses.contains_key(allocation_id) {
            return None;
        }
        self.detach_elastic_ip(allocation_id);
        let address = self.addresses.remove(allocation_id)?;
        self.address_transfers.remove(allocation_id);
        // The reverse DNS record of an address the account no longer holds is the record of
        // nothing, and an update owed on it can no longer come due, so both go with the
        // allocation rather than outliving it.
        self.address_ptr_records.remove(allocation_id);
        self.release_stored_bytes(address_payload_bytes(&address));
        Some(address)
    }

    /// Whether another transit gateway fits inside the store's ceiling
    /// ([`MAX_TRANSIT_GATEWAYS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`transit_gateway_limit_exceeded`] when it answers `false`.
    pub fn has_transit_gateway_capacity(&self) -> bool {
        self.transit_gateways.len() < MAX_TRANSIT_GATEWAYS
    }

    /// Insert a transit gateway in `pending`, with the default route table a create owes
    /// it, and answer the `TransitGatewayId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id, the ARN built from it, the owning account, the creation time, the
    /// state and the default route table's id — so two writers cannot report the same
    /// gateway differently. The option values in `spec.options` are the ones the request
    /// resolved ([`transit_gateway_options`]); the two route-table ids are filled in here
    /// because only this function knows the table it created.
    ///
    /// "When you create a transit gateway, we create a default transit gateway route table
    /// and use it as the default association route table and the default propagation route
    /// table … If you disable automatic route propagation, we do not create a default
    /// transit gateway route table" (`CreateTransitGateway`), and "Either
    /// `defaultRouteTableAssociation` or `defaultRouteTablePropagation` must be set to
    /// `enable` for Amazon Web Services Transit Gateway to create the default transit
    /// gateway route table" (`TransitGatewayOptions$DefaultRouteTableAssociation`) — so the
    /// table is created exactly when at least one of the two is enabled, and each id is
    /// reported only for the option that is.
    ///
    /// Like [`Self::insert_vpc`], the caller-chosen bytes of the record are charged against
    /// the store's byte budget here, so a writer cannot store them uncharged, and the
    /// clock-owed `available` move is recorded rather than slept for.
    pub fn insert_transit_gateway(&mut self, spec: TransitGatewaySpec) -> String {
        let transit_gateway_id = self.next_id("tgw");
        let creation_time = system_time_to_date_time(spec.created_at);
        let mut options = spec.options;
        let associates = options.default_route_table_association.as_deref() == Some(OPTION_ENABLE);
        let propagates = options.default_route_table_propagation.as_deref() == Some(OPTION_ENABLE);
        if associates || propagates {
            if let Some(route_table_id) = self
                .insert_default_transit_gateway_route_table(&transit_gateway_id, spec.created_at)
            {
                if let Some(route_table) =
                    self.transit_gateway_route_tables.get_mut(&route_table_id)
                {
                    route_table.default_association_route_table = Some(associates);
                    route_table.default_propagation_route_table = Some(propagates);
                }
                if associates {
                    options.association_default_route_table_id = Some(route_table_id.clone());
                }
                if propagates {
                    options.propagation_default_route_table_id = Some(route_table_id);
                }
            }
        }
        let transit_gateway = TransitGateway {
            transit_gateway_id: Some(transit_gateway_id.clone()),
            transit_gateway_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("transit-gateway/{transit_gateway_id}"),
            )),
            state: Some(TRANSIT_GATEWAY_STATE_PENDING.to_string()),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            description: spec.description,
            creation_time: Some(creation_time),
            options: Some(options),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(transit_gateway_payload_bytes(&transit_gateway));
        self.transit_gateways
            .insert(transit_gateway_id.clone(), transit_gateway);
        // What the clock now owes this gateway: `available`, one TRANSITION_DELAY from
        // now — the same span, off the same monotonic clock, as a VPC's pending move.
        self.transit_gateway_available_at
            .insert(transit_gateway_id.clone(), Instant::now() + TRANSITION_DELAY);
        transit_gateway_id
    }

    /// Mint the DEFAULT transit gateway route table of `transit_gateway_id`, created at
    /// `at`, and answer its id — or `None`, having written NOTHING, when the store's route
    /// table ceiling ([`Self::has_transit_gateway_route_table_capacity`]) refuses it, exactly
    /// as [`Self::insert_transit_gateway_route_table`] refuses an additional table. Nothing in
    /// this emulator reclaims a stored table, so a write path that did not ask would let a
    /// caller grow this process without limit.
    ///
    /// The ONE place that table is built, for the create that owes a fresh gateway one and
    /// for the modification that owes one to a gateway whose default role has just been
    /// enabled. It starts `pending`, because it comes up WITH its gateway rather than on an
    /// entry of its own ([`Self::transit_gateway_route_table_available_at`] holds only the
    /// ADDITIONAL tables `CreateTransitGatewayRouteTable` makes), and it is created holding
    /// neither default role: which roles it holds is read off the gateway's options by the
    /// caller, which is the one place those roles are decided.
    ///
    /// The table is recorded as the gateway's default one
    /// ([`Self::transit_gateway_default_route_table`]), so a later modification gives an
    /// enabled role THIS table back instead of minting another.
    pub(super) fn insert_default_transit_gateway_route_table(
        &mut self,
        transit_gateway_id: &str,
        at: SystemTime,
    ) -> Option<String> {
        if !self.has_transit_gateway_route_table_capacity() {
            return None;
        }
        let route_table_id = self.next_id(TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX);
        let route_table = TransitGatewayRouteTable {
            transit_gateway_route_table_id: Some(route_table_id.clone()),
            transit_gateway_id: Some(transit_gateway_id.to_string()),
            state: Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING.to_string()),
            default_association_route_table: Some(false),
            default_propagation_route_table: Some(false),
            creation_time: Some(system_time_to_date_time(at)),
            // Nothing tags a default route table: a tag specification tags the gateway.
            tags: Some(Vec::new()),
        };
        self.add_stored_bytes(transit_gateway_route_table_payload_bytes(&route_table));
        self.transit_gateway_route_tables
            .insert(route_table_id.clone(), route_table);
        self.transit_gateway_default_route_table
            .insert(transit_gateway_id.to_string(), route_table_id.clone());
        Some(route_table_id)
    }

    /// Apply `modification` to the transit gateway it names, as of `at`, and answer the
    /// gateway as it NOW stands — `None` when this store holds no gateway under that id.
    ///
    /// The one way a stored transit gateway is modified, so what `ModifyTransitGateway`
    /// answers and what a later `DescribeTransitGateways` reports cannot differ: what it
    /// hands back is the stored record itself. Which members the call changes is
    /// [`TransitGatewayModification::applied_to`]'s one definition; what only this function
    /// can decide is done here:
    ///
    /// * the gateway is left in the TRANSIENT [`TRANSIT_GATEWAY_STATE_MODIFYING`] state with
    ///   the clock owing it `available` [`TRANSITION_DELAY`] later, exactly as a create
    ///   leaves a fresh gateway `pending` — the span is never zero, so a caller acting
    ///   straight after this call still sees the modification being carried out;
    /// * a default role that is enabled and names no route table gets one: the table the
    ///   gateway already holds in either role, or a freshly minted default table
    ///   ([`needs_default_transit_gateway_route_table`]) when it holds none — so one gateway
    ///   keeps ONE default table however its roles are moved between the two options;
    /// * a default role that is DISABLED names no route table at all, the same reading of
    ///   "each id is reported only for the option that is enabled" that
    ///   [`Self::insert_transit_gateway`] applies;
    /// * every route table of the gateway is left reporting the default roles the gateway's
    ///   options now name it for, so the gateway's options and its tables cannot disagree
    ///   about which table is default.
    ///
    /// The caller-chosen bytes are re-charged across the write — the record this replaces is
    /// refunded and the record it becomes is charged — so a caller cannot grow the store
    /// past [`MAX_STORED_BYTES`] by modifying one gateway repeatedly; a handler asks
    /// [`Self::would_exceed_stored_bytes_after_refund`] about that net change first.
    pub fn modify_transit_gateway(
        &mut self,
        modification: &TransitGatewayModification,
        at: SystemTime,
    ) -> Option<TransitGateway> {
        let held = self
            .transit_gateways
            .get(&modification.transit_gateway_id)?;
        let refund = transit_gateway_payload_bytes(held);
        let mut view = modification.applied_to(held);
        view.state = Some(TRANSIT_GATEWAY_STATE_MODIFYING.to_string());

        let mut options = view.options.take().unwrap_or_default();
        let associates = options.default_route_table_association.as_deref() == Some(OPTION_ENABLE);
        let propagates = options.default_route_table_propagation.as_deref() == Some(OPTION_ENABLE);
        // ONE table serves both default roles, so the table the gateway already holds is read
        // — and whether a fresh one is needed decided — BEFORE a disabled role's id is
        // cleared: a call that moves the default role from one option to the other, or that
        // disables a role and later enables it again, then reuses that table rather than
        // minting a second one and leaving the first a record nothing names. Which table is
        // the gateway's DEFAULT one outlives the roles being disabled
        // ([`Self::transit_gateway_default_route_table`]), where the option ids below do not.
        let held_default_route_table_id = options
            .association_default_route_table_id
            .clone()
            .or_else(|| options.propagation_default_route_table_id.clone())
            .or_else(|| {
                self.transit_gateway_default_route_table
                    .get(&modification.transit_gateway_id)
                    .filter(|route_table_id| {
                        self.transit_gateway_route_tables
                            .contains_key(route_table_id.as_str())
                    })
                    .cloned()
            });
        let mints_a_default_route_table = needs_default_transit_gateway_route_table(Some(&options))
            && held_default_route_table_id.is_none();
        if !associates {
            options.association_default_route_table_id = None;
        }
        if !propagates {
            options.propagation_default_route_table_id = None;
        }
        let default_route_table_id = if mints_a_default_route_table {
            self.insert_default_transit_gateway_route_table(&modification.transit_gateway_id, at)
        } else {
            held_default_route_table_id
        };
        if let Some(route_table_id) = default_route_table_id {
            if associates && options.association_default_route_table_id.is_none() {
                options.association_default_route_table_id = Some(route_table_id.clone());
            }
            if propagates && options.propagation_default_route_table_id.is_none() {
                options.propagation_default_route_table_id = Some(route_table_id);
            }
        }
        for (route_table_id, route_table) in &mut self.transit_gateway_route_tables {
            if route_table.transit_gateway_id.as_deref()
                != Some(modification.transit_gateway_id.as_str())
            {
                continue;
            }
            route_table.default_association_route_table = Some(
                options.association_default_route_table_id.as_deref()
                    == Some(route_table_id.as_str()),
            );
            route_table.default_propagation_route_table = Some(
                options.propagation_default_route_table_id.as_deref()
                    == Some(route_table_id.as_str()),
            );
        }
        view.options = Some(options);

        let charge = transit_gateway_payload_bytes(&view);
        self.transit_gateways
            .insert(modification.transit_gateway_id.clone(), view.clone());
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        // What the clock now owes this gateway: `available`, one TRANSITION_DELAY from now —
        // the same span, off the same monotonic clock, as a create's `pending` move.
        self.transit_gateway_available_at.insert(
            modification.transit_gateway_id.clone(),
            Instant::now() + TRANSITION_DELAY,
        );
        Some(view)
    }

    /// The attachments the transit gateway `transit_gateway_id` still holds, by the attachment
    /// id each is held under — what has to be taken away before the gateway itself can be
    /// deleted.
    ///
    /// An attachment whose owner REJECTED it is attached to nothing, so it is not one of them;
    /// every other stored attachment names this gateway for as long as it is stored, so it
    /// holds the gateway exactly as a stored network interface holds its subnet.
    pub fn transit_gateway_attachments_of<'a>(&'a self, transit_gateway_id: &str) -> Vec<&'a str> {
        self.transit_gateway_attachments
            .iter()
            .filter(|(_, record)| {
                record.view.transit_gateway_id() == Some(transit_gateway_id)
                    && record.view.state() != Some(record.view.states().rejected)
            })
            .map(|(attachment_id, _)| attachment_id.as_str())
            .collect()
    }

    /// Start deleting the transit gateway `transit_gateway_id` names, and answer it as the
    /// store then holds it — `None` when the account holds no such gateway.
    ///
    /// "Deletes the specified transit gateway" is work AWS reports as under way: the
    /// `TransitGatewayState` catalog carries `deleting` and `deleted`, so the gateway is left
    /// in the TRANSIENT state with the clock owing it the REMOVAL one [`TRANSITION_DELAY`]
    /// later ([`Self::transit_gateway_delete_at`], applied by
    /// [`Self::materialize_due_transitions`]) — the same arrangement, on the same clock, as the
    /// `pending` a create left it in, and the one
    /// [`Self::begin_ipam_deletion`] gives a deleted IPAM. So a caller that reads straight back
    /// still sees the delete under way and a later one finds the gateway gone, which is what
    /// `DescribeTransitGateways` no longer carrying it means.
    ///
    /// Any move the clock still owed the gateway is superseded: one on its way out is not going
    /// to land in the state a create or a modification was moving it to.
    pub fn begin_transit_gateway_deletion(
        &mut self,
        transit_gateway_id: &str,
    ) -> Option<TransitGateway> {
        let transit_gateway = self.transit_gateways.get_mut(transit_gateway_id)?;
        transit_gateway.state = Some(TRANSIT_GATEWAY_STATE_DELETING.to_string());
        let view = transit_gateway.clone();
        self.transit_gateway_available_at.remove(transit_gateway_id);
        self.transit_gateway_delete_at.insert(
            transit_gateway_id.to_string(),
            Instant::now() + TRANSITION_DELAY,
        );
        Some(view)
    }

    /// Apply the removal the clock owes a deleted transit gateway, as of `now`.
    ///
    /// A gateway a `DeleteTransitGateway` left `deleting` was GONE at the instant that call
    /// recorded for it, with the route tables that came up with it, whether or not anybody
    /// called again — so the removal is applied from [`Self::materialize_due_transitions`], its
    /// one caller, before anything reads the store.
    pub(super) fn materialize_due_transit_gateway_deletions(&mut self, now: Instant) {
        let due: Vec<String> = self
            .transit_gateway_delete_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(transit_gateway_id, _)| transit_gateway_id.clone())
            .collect();
        for transit_gateway_id in due {
            self.transit_gateway_delete_at.remove(&transit_gateway_id);
            self.remove_transit_gateway(&transit_gateway_id);
        }
    }

    /// Remove the transit gateway `transit_gateway_id` names with the route tables that came
    /// up with it, and answer it as the store held it — `None` when the account holds no such
    /// gateway.
    ///
    /// "When you delete a transit gateway, we delete its default route table": a route table
    /// of the gateway cannot outlive it, so it goes here rather than being left reporting a
    /// `TransitGatewayId` nothing resolves — and with each table go the associations and
    /// propagations that named it. The caller-chosen bytes every removed record held go back
    /// to the store's budget on exactly the terms the create charged them
    /// ([`transit_gateway_payload_bytes`], [`transit_gateway_route_table_payload_bytes`]), and
    /// so does the `available` move the clock still owed either.
    pub fn remove_transit_gateway(&mut self, transit_gateway_id: &str) -> Option<TransitGateway> {
        let transit_gateway = self.transit_gateways.remove(transit_gateway_id)?;
        self.release_stored_bytes(transit_gateway_payload_bytes(&transit_gateway));
        self.transit_gateway_available_at.remove(transit_gateway_id);
        self.transit_gateway_delete_at.remove(transit_gateway_id);
        // Which table was this gateway's default one is a fact about a gateway that is gone,
        // so the entry goes with it rather than outliving every gateway the store ever held.
        self.transit_gateway_default_route_table
            .remove(transit_gateway_id);
        let route_table_ids: Vec<String> = self
            .transit_gateway_route_tables
            .iter()
            .filter(|(_, route_table)| {
                route_table.transit_gateway_id.as_deref() == Some(transit_gateway_id)
            })
            .map(|(route_table_id, _)| route_table_id.clone())
            .collect();
        for route_table_id in route_table_ids {
            if let Some(route_table) = self.transit_gateway_route_tables.remove(&route_table_id) {
                self.release_stored_bytes(transit_gateway_route_table_payload_bytes(&route_table));
            }
            self.transit_gateway_route_table_available_at
                .remove(&route_table_id);
            // The static routes the table held: a route is a resource OF its table, so it
            // goes with the table rather than being left in a table nothing resolves.
            self.remove_transit_gateway_routes_of(&route_table_id);
            self.transit_gateway_associations
                .retain(|(held, _), _| *held != route_table_id);
            self.transit_gateway_propagations
                .retain(|(held, _), _| *held != route_table_id);
            self.transit_gateway_prefix_list_references
                .retain(|(held, _), _| *held != route_table_id);
        }
        Some(transit_gateway)
    }

    /// The attachments of `transit_gateway_id` whose KIND has to be deleted before the
    /// gateway's Amazon-side ASN can be modified, by the attachment id each is held under.
    ///
    /// "The modify ASN operation is not allowed on a transit gateway if it has the following
    /// attachments: Dynamic VPN, Static VPN, Direct Connect Gateway, Connect, VPN
    /// Concentrator, Client VPN. You must first delete all transit gateway attachments
    /// configured prior to modifying the ASN on the transit gateway"
    /// (`ModifyTransitGatewayOptions$AmazonSideAsn`) — a LIST of kinds, so the kinds not on
    /// it (a VPC attachment, a transit gateway peering) do not block the modification. An
    /// attachment whose owner REJECTED it is configured on nothing, so it does not block one
    /// either.
    pub fn transit_gateway_attachments_blocking_asn_change<'a>(
        &'a self,
        transit_gateway_id: &str,
    ) -> Vec<&'a str> {
        self.transit_gateway_attachments
            .iter()
            .filter(|(_, record)| {
                record.view.transit_gateway_id() == Some(transit_gateway_id)
                    && matches!(
                        record.view.resource_type(),
                        TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT
                            | TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN
                    )
                    && record.view.state() != Some(record.view.states().rejected)
            })
            .map(|(attachment_id, _)| attachment_id.as_str())
            .collect()
    }

    /// Whether another transit gateway route table fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_transit_gateway_capacity`]: a create asks first and
    /// refuses when it answers `false`. The ceiling is this PROCESS's record ceiling, over
    /// every route table the store holds, and not AWS's per-transit-gateway route table
    /// quota — nothing here models that quota.
    pub fn has_transit_gateway_route_table_capacity(&self) -> bool {
        self.transit_gateway_route_tables.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Insert an ADDITIONAL route table of `transit_gateway_id` in `pending`, and answer it
    /// as stored — or `None` when the store is full.
    ///
    /// "You can use CreateTransitGatewayRouteTable to create additional transit gateway
    /// route tables" (`CreateTransitGateway`): an additional table is neither of the
    /// gateway's default tables, so both `DefaultAssociationRouteTable` and
    /// `DefaultPropagationRouteTable` are reported `false` — only the table
    /// [`Self::insert_transit_gateway`] creates is a default one, and only
    /// `CreateTransitGateway`'s options decide which roles it holds.
    ///
    /// Everything the service owns is derived here rather than taken from the caller — the
    /// id, the state and the clock-owed `available` move — so no two writers can report one
    /// table differently. Like every other insert, the caller-chosen bytes of the record are
    /// charged against the store's byte budget here, so a writer cannot store them
    /// uncharged.
    pub fn insert_transit_gateway_route_table(
        &mut self,
        transit_gateway_id: &str,
        created_at: SystemTime,
        tags: Vec<Tag>,
    ) -> Option<TransitGatewayRouteTable> {
        if !self.has_transit_gateway_route_table_capacity() {
            return None;
        }
        let route_table_id = self.next_id(TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX);
        let route_table = TransitGatewayRouteTable {
            transit_gateway_route_table_id: Some(route_table_id.clone()),
            transit_gateway_id: Some(transit_gateway_id.to_string()),
            // The TRANSIENT state of a create, with the clock owing it `available`: the
            // span is never zero, so a caller acting straight after this call still sees
            // the table pending.
            state: Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING.to_string()),
            default_association_route_table: Some(false),
            default_propagation_route_table: Some(false),
            creation_time: Some(system_time_to_date_time(created_at)),
            tags: Some(tags),
        };
        self.add_stored_bytes(transit_gateway_route_table_payload_bytes(&route_table));
        self.transit_gateway_route_table_available_at
            .insert(route_table_id.clone(), Instant::now() + TRANSITION_DELAY);
        self.transit_gateway_route_tables
            .insert(route_table_id, route_table.clone());
        Some(route_table)
    }
}

/// The bytes one stored transit gateway route table costs: the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, plus the caller-chosen bytes
/// of its tags. Everything else on the record — the two ids, the state, the default-role
/// flags and the creation time — is derived by this module, so it is fixed-size; the
/// overhead is what keeps a table nobody tagged from being free against
/// [`MAX_STORED_BYTES`], the default table a gateway comes with included.
///
/// The ONE definition of that charge: `CreateTransitGatewayRouteTable` asks it before the
/// write and [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_route_table`] charges
/// exactly it, so a removal path must give back the same number.
pub fn transit_gateway_route_table_payload_bytes(route_table: &TransitGatewayRouteTable) -> usize {
    STORED_RECORD_OVERHEAD_BYTES.saturating_add(tag_list_bytes(
        route_table.tags.as_deref().unwrap_or_default(),
    ))
}

/// The caller-chosen bytes one stored Elastic IP address holds: its tags. Everything else
/// on the record — the allocation id, the address, the domain, the network border group —
/// is derived by this module or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `AllocateAddress` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn address_payload_bytes(address: &Address) -> usize {
    tag_list_bytes(address.tags.as_deref().unwrap_or_default())
}

/// Whether `id` is a well-formed Elastic IP allocation id.
///
/// An allocation id is `eipalloc-` followed by 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form, as the model's own id patterns read. EC2 answers
/// `InvalidAllocationID.Malformed` for anything else, BEFORE it looks the id up.
pub fn allocation_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("eipalloc-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `candidate` is a dotted-quad IPv4 address, the form
/// `AllocateAddressRequest$Address` and `DescribeAddressesRequest$PublicIps` take.
///
/// `Ipv4Addr`'s own parser accepts exactly four decimal octets, so this is the one place
/// the form is decided: the crate's CIDR helpers are about blocks, not single addresses.
pub fn is_ipv4_address(candidate: &str) -> bool {
    candidate.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// Whether `candidate` is an IPv6 address, the other form an option documented as taking
/// "four IPv4 addresses and four IPv6 addresses"
/// ([`DHCP_OPTION_NTP_SERVERS`]) accepts.
///
/// `Ipv6Addr`'s own parser decides the form, the way [`is_ipv4_address`] leaves the dotted
/// quad to `Ipv4Addr`.
pub fn is_ipv6_address(candidate: &str) -> bool {
    candidate.parse::<Ipv6Addr>().is_ok()
}

/// Refuse an allocation id the request named that this service cannot resolve: a
/// malformed one before an unknown one, the way the service orders the two.
///
/// Answers the stored address, so a caller that resolves an id reads the record it resolved
/// to rather than looking it up a second time.
pub fn require_known_address<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    allocation_id: &str,
) -> Result<&'store Address, crate::scaffolded::Scaffolded<E>> {
    if !allocation_id_is_well_formed(allocation_id) {
        return Err(invalid_allocation_id_malformed(allocation_id));
    }
    store
        .addresses
        .get(allocation_id)
        .ok_or_else(|| invalid_allocation_id_not_found(allocation_id))
}

/// Refuse a request that named something that is not an allocation id: HTTP 400
/// `InvalidAllocationID.Malformed`.
pub fn invalid_allocation_id_malformed<E>(allocation_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ALLOCATION_ID_MALFORMED,
        400,
        invalid_id_message(allocation_id),
    )
}

/// Why a well-formed allocation id resolves to no address of the account.
///
/// ONE wording, so the two codes EC2 publishes for that failure
/// ([`invalid_allocation_id_not_found`] and [`INVALID_ELASTIC_IP_ID_NOT_FOUND`], whichever an
/// operation answers) say the same thing about the same id.
pub fn allocation_id_not_found_message(allocation_id: &str) -> String {
    format!("The allocation ID '{allocation_id}' does not exist")
}

/// Refuse a request that named an allocation id no address of the account has: HTTP 400
/// `InvalidAllocationID.NotFound`.
pub fn invalid_allocation_id_not_found<E>(allocation_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ALLOCATION_ID_NOT_FOUND,
        400,
        allocation_id_not_found_message(allocation_id),
    )
}

/// Refuse a request that named an allocation id no address of the account has, under the
/// other of the two codes EC2 publishes for that failure: HTTP 400
/// [`INVALID_ELASTIC_IP_ID_NOT_FOUND`].
///
/// A second named constructor rather than a literal at the call site, for the reason
/// [`invalid_allocation_id_not_found`] is one and the reason given on the code itself: which
/// of the two an operation answers is a fact of that operation. The sentence is the one
/// recorded for that code, naming the id the REQUEST sent as the `elasticIp` id the code is
/// about — EC2's own spelling of the resource there, which is not how the sentence beside it
/// names an allocation id.
pub fn invalid_elastic_ip_id_not_found<E>(allocation_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ELASTIC_IP_ID_NOT_FOUND,
        400,
        format!("The elasticIp ID '{allocation_id}' does not exist"),
    )
}

/// Refuse a request that named an Elastic IP address this account holds no allocation
/// for: HTTP 400 `InvalidAddress.NotFound`.
///
/// ONE sentence for that code, whichever operation answers it — `The Elastic IP address
/// '<address>' does not exist or is not available.`, naming the address from the REQUEST — so a
/// recovery, an association, a release and a describe all read the same way; the refusal says
/// nothing about WHICH of the two it is, because the account is told neither.
pub fn invalid_address_not_found<E>(public_ip: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ADDRESS_NOT_FOUND,
        400,
        format!("The Elastic IP address '{public_ip}' does not exist or is not available."),
    )
}

// =============================================================================
// The `domain-name` attribute of an Elastic IP address
// =============================================================================

/// The one attribute of an Elastic IP address a caller can set and reset: the single
/// member of the modelled `AddressAttributeName` enum
/// (`smithy_query(query="shape",shape="AddressAttributeName")`), which names the address's
/// reverse DNS (PTR) record.
pub const ADDRESS_ATTRIBUTE_DOMAIN_NAME: &str = "domain-name";

/// The `PtrUpdateStatus$Status` of a PTR record update that has been accepted and not
/// applied yet.
///
/// "The updated PTR record for the IP address" (`AddressAttribute$PtrRecordUpdate`) is
/// reported only while an update is outstanding, so this is the status every such report
/// carries.
pub const PTR_UPDATE_STATUS_PENDING: &str = "PENDING";

/// `PendingRequestAlreadyExists` (HTTP 400): the address's PTR record already carries an
/// update that has not been applied, so a second one cannot be started on it.
pub const PENDING_REQUEST_ALREADY_EXISTS: &str = "PendingRequestAlreadyExists";

/// What the `domain-name` attribute of one Elastic IP address is, and what the clock still
/// owes it.
///
/// The modelled [`Address`] shape carries no member for the reverse DNS record, so this
/// record holds it beside the account's address rather than on it; it is projected into the
/// modelled `AddressAttribute` shape by
/// [`ComAmazonawsEc2Amazonec2State::address_attribute_view`].
#[derive(Debug, Clone, Default)]
pub struct AddressPtrRecord {
    /// The PTR record in effect for the address, which is what
    /// `AddressAttribute$PtrRecord` reports. `None` until an update that names one has been
    /// applied, and again once one that takes it away has been.
    pub ptr_record: Option<String>,
    /// The update started on that record and not applied yet, which is what
    /// `AddressAttribute$PtrRecordUpdate` reports.
    pub pending_update: Option<PendingPtrRecordUpdate>,
}

/// A PTR record update the clock has not finished yet: what it leaves in effect, and when it
/// falls due.
#[derive(Debug, Clone)]
pub struct PendingPtrRecordUpdate {
    /// The PTR record this update leaves in effect. `None` is a RESET, which takes the
    /// record away instead of naming a new one — "Resets the attribute of the specified IP
    /// address" (`ResetAddressAttribute`) — so the value a report carries for it is absent
    /// too.
    pub value: Option<String>,
    /// When the update falls due, on the same monotonic clock and the same
    /// [`TRANSITION_DELAY`] as every other move this store records rather than sleeps for.
    pub due_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether `allocation_id`'s PTR record already carries an update the clock has not
    /// applied.
    ///
    /// The one question both writers of the attribute ask before starting one, because the
    /// real service admits a single outstanding request per record and refuses a second
    /// with [`pending_request_already_exists`].
    pub fn address_ptr_update_is_pending(&self, allocation_id: &str) -> bool {
        self.address_ptr_records
            .get(allocation_id)
            .is_some_and(|record| record.pending_update.is_some())
    }

    /// Start an update of `allocation_id`'s PTR record to `value` — `None` to take the
    /// record away, which is what a reset asks for — and answer the attribute as it now
    /// reads.
    ///
    /// The value becomes the address's record AT ONCE and the update is reported as
    /// OUTSTANDING beside it until it falls due: `AddressAttribute$PtrRecord` is the record
    /// the attribute now carries, while "the updated PTR record for the IP address"
    /// (`$PtrRecordUpdate`) is the change itself, which a modify's recorded answer reports
    /// `PENDING` beside the record it has just set. It comes due one [`TRANSITION_DELAY`]
    /// later through [`Self::apply_due_ptr_record_updates`] — recorded rather than slept for,
    /// like every other clock-owed move of this store — and until then no second update can
    /// be started on the record ([`Self::address_ptr_update_is_pending`]). The caller has
    /// already resolved `allocation_id` to an address of the account and asked that question.
    ///
    /// No byte charge is made for the entry: it is keyed by an id of [`Self::addresses`],
    /// so there can be no more of them than there are stored addresses, and the one writer
    /// that names a value bounds it before it gets here, the way every other caller-supplied
    /// value of this store is bounded.
    pub fn start_address_ptr_record_update(
        &mut self,
        allocation_id: &str,
        value: Option<String>,
    ) -> Option<AddressAttribute> {
        let entry = self
            .address_ptr_records
            .entry(allocation_id.to_string())
            .or_default();
        entry.ptr_record = value.clone();
        entry.pending_update = Some(PendingPtrRecordUpdate {
            value,
            due_at: Instant::now() + TRANSITION_DELAY,
        });
        self.address_attribute_view(allocation_id)
    }

    /// Apply every PTR record update the clock has taken past its due instant.
    ///
    /// Reached through [`Self::materialize_due_transitions`], so a reporting operation may
    /// apply it: each update was already owed at `now` whether or not anybody called, and
    /// applying it is what stops the change being reported as outstanding. The record it
    /// names is written again here rather than only where the update is started, so one
    /// definition of what an applied update leaves in effect serves both ends. An entry left
    /// with neither a record nor an owed update is dropped, an address with no reverse DNS
    /// record being one this store holds nothing about.
    pub fn apply_due_ptr_record_updates(&mut self, now: Instant) {
        self.address_ptr_records.retain(|_allocation_id, record| {
            let due = record
                .pending_update
                .as_ref()
                .is_some_and(|pending| now >= pending.due_at);
            if due {
                record.ptr_record = record
                    .pending_update
                    .take()
                    .and_then(|pending| pending.value);
            }
            record.ptr_record.is_some() || record.pending_update.is_some()
        });
    }

    /// The `domain-name` attribute of `allocation_id` as the modelled `AddressAttribute`
    /// shape, or `None` when the account holds no address under that id.
    ///
    /// The ONE projection of an address and its [`AddressPtrRecord`] into that shape, so
    /// the operations that set, reset and report the attribute cannot describe one address
    /// differently. An address with no PTR record and no update owed on it reports neither
    /// member — the shape's own answer for an address nothing has given a reverse DNS
    /// record.
    pub fn address_attribute_view(&self, allocation_id: &str) -> Option<AddressAttribute> {
        let address = self.addresses.get(allocation_id)?;
        let record = self.address_ptr_records.get(allocation_id);
        Some(AddressAttribute {
            public_ip: address.public_ip.clone(),
            allocation_id: address.allocation_id.clone(),
            ptr_record: record.and_then(|record| record.ptr_record.clone()),
            ptr_record_update: record
                .and_then(|record| record.pending_update.as_ref())
                .map(|pending| PtrUpdateStatus {
                    value: pending.value.clone(),
                    status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
                    // "The reason for the PTR record update"
                    // (`PtrUpdateStatus$Reason`): an accepted update has none to give.
                    reason: None,
                }),
        })
    }
}

/// Refuse a request to change a PTR record that already has an outstanding update: HTTP
/// 400 `PendingRequestAlreadyExists`.
///
/// The message is the sentence the real service answers there, which carries no variable
/// part: the request named the record, and the refusal is about the one update already
/// outstanding on it.
pub fn pending_request_already_exists<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        PENDING_REQUEST_ALREADY_EXISTS,
        400,
        "A pending request for this record already exists.",
    )
}

/// Whether `id` is a well-formed network interface id.
///
/// `eni-` followed by 8 or 17 lowercase hexadecimal digits — the short and the long EC2
/// resource id form, read the way [`allocation_id_is_well_formed`] reads an allocation id.
/// The all-zero suffix is WELL FORMED here, unlike an instance id's: AWS answers
/// `eni-00000000000000000` with `InvalidNetworkInterfaceID.NotFound` rather than
/// `.Malformed`, which is the counter-example [`instance_id_is_well_formed`] records.
pub fn network_interface_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("eni-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a request that named something that is not a network interface id: HTTP 400
/// `InvalidNetworkInterfaceId.Malformed`.
///
/// The message is this RESOURCE's own sentence — `The interface ID '<id>' is malformed`,
/// naming the interface the way [`invalid_network_interface_id_not_found`] beside it names one
/// and carrying the id the REQUEST sent, bounded by [`echoed_resource_id`] — rather than the
/// resource-agnostic [`invalid_id_message`] template. An id of a resource BESIDE the interface
/// keeps that template: a malformed network interface ATTACHMENT id is refused
/// `Invalid id: "<value>"`, a different id space from this one.
pub fn invalid_network_interface_id_malformed<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        400,
        format!(
            "The interface ID '{}' is malformed",
            echoed_resource_id(network_interface_id)
        ),
    )
}

/// Refuse a request that named a network interface id no interface of the account has:
/// HTTP 400 `InvalidNetworkInterfaceID.NotFound`.
pub fn invalid_network_interface_id_not_found<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        400,
        format!("The networkInterface ID '{network_interface_id}' does not exist"),
    )
}
