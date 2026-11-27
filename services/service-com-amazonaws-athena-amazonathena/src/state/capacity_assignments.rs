// SPDX-License-Identifier: CC0-1.0

//! Which workgroups consume a capacity reservation's capacity.
//!
//! `CapacityAssignmentConfiguration` documents the arrangement: it "Assigns Athena
//! workgroups (and hence their queries) to capacity reservations. A capacity reservation can
//! have only one capacity assignment configuration, but the capacity assignment
//! configuration can be made up of multiple individual assignments." So the configuration is
//! not a resource of its own: it is one field of the reservation it configures
//! ([`CapacityReservationRecord::capacity_assignments`]), which is what makes "only one"
//! true by construction — `PutCapacityAssignmentConfiguration` ("If a capacity assignment
//! configuration already exists for the capacity reservation, replaces the existing capacity
//! assignment configuration") REPLACES that field, and the configuration leaves the account
//! with the reservation it belongs to.
//!
//! Both operations of the slice are answered from here, so the write and the read cannot
//! drift: [`ComAmazonawsAthenaAmazonathenaState::put_capacity_assignments`] stores what a put
//! carried and [`ComAmazonawsAthenaAmazonathenaState::capacity_assignment_configuration`]
//! projects that same field back.

use std::time::SystemTime;

use super::*;

use crate::ops::types::{CapacityAssignment, CapacityAssignmentConfiguration};

/// The message a put naming a workgroup this account does not hold is refused with.
///
/// AWS's own wording for that miss, and wholly fixed text: neither the assignment nor the
/// workgroup name the request carried is interpolated into it, so a put naming one unknown
/// workgroup and a put naming several read the same way.
///
/// It is also the FIRST condition a put reports: the workgroups the request assigns are
/// resolved before the reservation the put is addressed to
/// ([`ComAmazonawsAthenaAmazonathenaState::put_capacity_assignments`]).
pub const WORK_GROUPS_NOT_FOUND: &str = "One or more WorkGroups are not found.";

/// The message a put naming a workgroup ANOTHER reservation's capacity still serves is
/// refused with.
///
/// A workgroup's queries run on the capacity assigned to it, and one workgroup's capacity
/// cannot be served by two reservations at once — so a configuration naming a workgroup
/// another live reservation holds is refused, and AWS states that in wholly fixed text:
/// neither the workgroup, nor either reservation, nor the number of names in the way is
/// interpolated into it, so a put colliding on one workgroup and a put colliding on several
/// read the same way.
pub const WORK_GROUPS_ASSIGNED_TO_ANOTHER_RESERVATION: &str =
    "One or more workgroups are assigned to another active reservation. Update your \
     reservation assignments to continue.";

/// Every workgroup name `assignments` names, across all the assignments of one
/// configuration: a configuration "can be made up of multiple individual assignments", and
/// each assignment carries its own `WorkGroupNames` list, so the workgroups a configuration
/// assigns are the names of all of them together.
pub fn assigned_work_groups(
    assignments: &[CapacityAssignment],
) -> impl Iterator<Item = &str> + '_ {
    assignments
        .iter()
        .filter_map(|assignment| assignment.work_group_names.as_deref())
        .flatten()
        .map(String::as_str)
}

/// How many individual assignments and workgroup names together one capacity assignment
/// configuration holds in this emulator.
///
/// The EMULATOR's own bound, with no counterpart in the model: `CapacityAssignmentsList` and
/// `WorkGroupNamesList` carry no `@length`, and Athena publishes no limit on either. Every
/// name a configuration carries has to be a workgroup this account holds, and the account
/// holds at most [`MAX_WORK_GROUPS`] of them, so a configuration past this bound assigns no
/// workgroup a configuration inside it could not — it only repeats itself. It is what keeps
/// one put from growing the emulator's heap without limit; a configuration past it is refused
/// with the operation's own `InvalidRequestException`.
pub const MAX_CAPACITY_ASSIGNMENT_ENTRIES: usize = MAX_WORK_GROUPS;

/// The refusal message for a configuration past [`MAX_CAPACITY_ASSIGNMENT_ENTRIES`].
pub fn capacity_assignment_configuration_too_large() -> String {
    format!(
        "Cannot store this capacity assignment configuration: a configuration holds at most \
         {MAX_CAPACITY_ASSIGNMENT_ENTRIES} assignments and workgroup names together."
    )
}

/// Why `assignments` is not a capacity assignment configuration this service can store, if it
/// is not: it is larger than [`MAX_CAPACITY_ASSIGNMENT_ENTRIES`].
///
/// Measured on the request alone, so a put that is going to be refused for its size never
/// walks the registry and no other Athena request waits behind the work of measuring it.
pub fn capacity_assignments_error(assignments: &[CapacityAssignment]) -> Option<String> {
    let entries = assignments.len() + assigned_work_groups(assignments).count();
    (entries > MAX_CAPACITY_ASSIGNMENT_ENTRIES).then(capacity_assignment_configuration_too_large)
}

/// Whether `record` is a reservation whose capacity a workgroup assigned to it can still
/// run on at `now`.
///
/// Every status but the two a cancellation leaves behind: cancelling a reservation
/// "releases the capacity" it holds (`CancelCapacityReservation`), so the workgroups a
/// cancelled — or cancelling — reservation assigns are no longer served by it and the
/// capacity is free for another reservation to claim them. A reservation still allocating
/// the capacity it asked for holds it just as an allocated one does, which is why the
/// question is "does it still hold the capacity" rather than "is it allocated yet".
fn serves_its_work_groups(record: &CapacityReservationRecord, now: SystemTime) -> bool {
    !matches!(
        record.status_at(now),
        CAPACITY_RESERVATION_STATUS_CANCELLING | CAPACITY_RESERVATION_STATUS_CANCELLED
    )
}

impl CapacityReservationRecord {
    /// Make `assignments` this reservation's capacity assignment configuration, replacing
    /// whatever configuration it held. The ONE place that field is written.
    pub fn assign_capacity(&mut self, assignments: Vec<CapacityAssignment>) {
        self.capacity_assignments = assignments;
    }

    /// This reservation's capacity assignment configuration, as the modelled shape a read
    /// answers with.
    ///
    /// Every reservation the account holds has one, and a reservation no put has configured
    /// yet has it empty: the configuration belongs to the reservation rather than existing
    /// beside it, so there is no reservation for which the question has no answer.
    pub fn to_capacity_assignment_configuration(&self) -> CapacityAssignmentConfiguration {
        CapacityAssignmentConfiguration {
            capacity_reservation_name: Some(self.name.clone()),
            capacity_assignments: Some(self.capacity_assignments.clone()),
        }
    }
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// Store `assignments` as the capacity assignment configuration of the reservation
    /// `name`, replacing the one it held.
    ///
    /// `Err` carries the sentence saying why NOTHING was stored: an assignment names a
    /// workgroup this account does not hold, the account holds no such reservation, or an
    /// assignment names a workgroup another live reservation already serves. All three are
    /// measured before the configuration is touched, so a put that is refused leaves the
    /// reservation configured exactly as the last accepted put left it.
    ///
    /// Whether every assigned workgroup RESOLVES is asked first, before the reservation the
    /// put is addressed to: what the request carries is checked before what it is addressed
    /// to, so a put whose assignment names a workgroup this account does not hold reports
    /// that condition whatever the state of the named reservation — and a client reads the
    /// one remedy that is its own to take (correct or create the workgroups it is assigning)
    /// rather than one about a reservation.
    pub fn put_capacity_assignments(
        &mut self,
        name: &str,
        assignments: Vec<CapacityAssignment>,
        now: SystemTime,
    ) -> Result<(), String> {
        // A configuration assigns WORKGROUPS, so every name it carries must be one this
        // account holds — asked of the ONE place a workgroup name is resolved
        // ([`ComAmazonawsAthenaAmazonathenaState::holds_work_group`]), which is why the
        // account's default workgroup is assignable without a record of its own.
        if !assigned_work_groups(&assignments).all(|work_group| self.holds_work_group(work_group))
        {
            return Err(WORK_GROUPS_NOT_FOUND.to_string());
        }
        if self.capacity_reservations.reservation(name, now).is_none() {
            return Err(capacity_reservation_not_found(name));
        }
        // And a workgroup is served by at most ONE reservation: a name another reservation
        // that still holds its capacity assigns is capacity this reservation cannot also
        // claim. Measured over the reservations the account holds at `now`, so a reservation
        // whose cancellation has been asked for frees the workgroups it assigned and one
        // whose retention elapsed stands in nothing's way.
        if assigned_work_groups(&assignments)
            .any(|work_group| self.work_group_served_elsewhere(work_group, name, now))
        {
            return Err(WORK_GROUPS_ASSIGNED_TO_ANOTHER_RESERVATION.to_string());
        }
        if let Some(record) = self.capacity_reservations.reservation_mut(name, now) {
            record.assign_capacity(assignments);
        }
        Ok(())
    }

    /// Whether a reservation OTHER than `excluding` serves `work_group` at `now`: one the
    /// account holds, that still holds its capacity ([`serves_its_work_groups`]), whose own
    /// capacity assignment configuration names that workgroup.
    ///
    /// A pure read of the registry, asked of every reservation the account holds, so the
    /// answer is about the state the store is in rather than about the order puts arrived in.
    fn work_group_served_elsewhere(
        &self,
        work_group: &str,
        excluding: &str,
        now: SystemTime,
    ) -> bool {
        self.capacity_reservations
            .records_at(now)
            .filter(|record| record.name != excluding)
            .filter(|record| serves_its_work_groups(record, now))
            .any(|record| {
                assigned_work_groups(&record.capacity_assignments)
                    .any(|assigned| assigned == work_group)
            })
    }

    /// The capacity assignment configuration of the reservation `name`, or the sentence a
    /// reservation this account does not hold is reported with.
    ///
    /// A pure read: it projects the field a put wrote
    /// ([`CapacityReservationRecord::to_capacity_assignment_configuration`]) and resolves the
    /// reservation through the same filter every other read of one applies, so a reservation
    /// whose cancelled retention has elapsed is as absent here as it is to a read of the
    /// reservation itself.
    pub fn capacity_assignment_configuration(
        &self,
        name: &str,
        now: SystemTime,
    ) -> Result<CapacityAssignmentConfiguration, String> {
        self.capacity_reservations
            .reservation(name, now)
            .map(CapacityReservationRecord::to_capacity_assignment_configuration)
            .ok_or_else(|| capacity_reservation_not_found(name))
    }
}
