// SPDX-License-Identifier: CC0-1.0

//! ONE DIRECTORY'S CERTIFICATE AUTHORITY ENROLLMENT POLICY: what `EnableCAEnrollmentPolicy`
//! records for a directory, what `DisableCAEnrollmentPolicy` withdraws, and what
//! `DescribeCAEnrollmentPolicy` reports about it.
//!
//! The policy "determines how client certificates are automatically enrolled and managed
//! through Amazon Web Services Private Certificate Authority"
//! (`DescribeCAEnrollmentPolicy`), and a directory has at most one — the enable takes the
//! `DirectoryId` and the connector it enrolls through and nothing that identifies a second
//! policy — so the record lives on the directory it belongs to
//! ([`super::Directory::ca_enrollment_policy`]) and deleting the directory takes it with it.
//!
//! The status is the SERVICE's rather than the caller's: neither write takes one and
//! neither answers one, so a toggle records the TRANSIENT status its own documentation
//! names ("IN_PROGRESS - The policy is being activated", "DISABLING - The policy is being
//! deactivated") and the clock settles it one [`TRANSITION_SPAN`] later, the same way every
//! other transition this service owes is settled ([`super::ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`]).
//! A directory no enable has ever named holds no record at all, which is what
//! [`CA_ENROLLMENT_POLICY_DISABLED`] — "the policy is inactive and automatic certificate
//! enrollment is not available" — reports for.
//!
//! # The connector an enable enrolls through
//!
//! Enrollment is performed BY a PCA connector, which is a resource of the AWS Private CA
//! Connector for Active Directory service and not of this one: "ensure that the PCA
//! connector is properly configured and accessible from the directory. The connector must
//! be in an active state" (`EnableCAEnrollmentPolicy`). So an enable resolves the
//! `PcaConnectorArn` in the store of the service that OWNS connectors
//! ([`connector_defect`]) and is refused when that service holds no such connector, holds
//! one that is not in service yet, or holds one created for another directory — a policy
//! this service reported `Success` for while nothing could enroll a certificate would be a
//! success backed by nothing.
//!
//! Each refusal's wording follows the class it belongs to: a value the caller must fix is
//! named in this service's own resolution sentence ([`no_such_pca_connector`]), while a
//! fact about the directory's own policy, which names no value to fix, is stated as the
//! model words that error shape.

use super::*;

use ::service_com_amazonaws_pcaconnectorad_pcaconnectorad::state as pca_connector;

/// The status a policy carries between `EnableCAEnrollmentPolicy` and
/// [`TRANSITION_SPAN`]: the `InProgress` value of the modelled `CaEnrollmentPolicyStatus`
/// enum (`smithy_query(query="shape",shape="CaEnrollmentPolicyStatus")`), documented as
/// "IN_PROGRESS - The policy is being activated".
pub const CA_ENROLLMENT_POLICY_IN_PROGRESS: &str = "InProgress";

/// The status a policy reaches once that transition has fallen due: the `Success` value of
/// the same modelled enum, "SUCCESS - The policy is active and automatic certificate
/// enrollment is operational".
pub const CA_ENROLLMENT_POLICY_SUCCESS: &str = "Success";

/// The status a policy carries between `DisableCAEnrollmentPolicy` and
/// [`TRANSITION_SPAN`]: the `Disabling` value of the same modelled enum, "DISABLING - The
/// policy is being deactivated".
pub const CA_ENROLLMENT_POLICY_DISABLING: &str = "Disabling";

/// The status a policy reaches once the disable has settled, and the status a directory no
/// enable has ever named reports: the `Disabled` value of the same modelled enum,
/// "DISABLED - The policy is inactive and automatic certificate enrollment is not
/// available".
pub const CA_ENROLLMENT_POLICY_DISABLED: &str = "Disabled";

/// The `EnableAlreadyInProgressException` diagnostic an enable is refused with while the
/// transition an earlier enable owes is still outstanding.
///
/// The sentence is the one the model states for that error shape
/// (`smithy_query(query="shape",shape="EnableAlreadyInProgressException")`): it is a fact
/// about the directory's own policy and carries no value of the request.
/// [`refusal`] appends the request id, once.
pub const CA_ENROLLMENT_ENABLE_IN_PROGRESS: &str =
    "An enable operation for CA enrollment policy is already in progress for this directory.";

/// The `DisableAlreadyInProgressException` diagnostic a disable is refused with while the
/// transition an earlier disable owes is still outstanding — the model's own sentence for
/// that error shape (`smithy_query(query="shape",shape="DisableAlreadyInProgressException")`).
pub const CA_ENROLLMENT_DISABLE_IN_PROGRESS: &str =
    "A disable operation for CA enrollment policy is already in progress for this directory.";

/// The `EntityAlreadyExistsException` diagnostic an enable is refused with when the
/// directory already holds a policy — the model's own sentence for that error shape
/// (`smithy_query(query="shape",shape="EntityAlreadyExistsException")`), which is the
/// service-wide wording for an entity a create cannot make a second of.
pub const CA_ENROLLMENT_POLICY_EXISTS: &str = "The specified entity already exists.";

/// The `EntityDoesNotExistException` diagnostic a disable is refused with when the
/// directory holds no policy to withdraw — the model's own sentence for that error shape
/// (`smithy_query(query="shape",shape="EntityDoesNotExistException")`).
pub const CA_ENROLLMENT_POLICY_ABSENT: &str = "The specified entity could not be found.";

/// The `EntityDoesNotExistException` diagnostic an enable naming a connector the service
/// that owns connectors does not hold is refused with.
///
/// In this service's own resolution wording — the entity, the quoted value the request
/// asked for, then "does not exist." ([`no_such_directory`], and the same shape every other
/// entity this service resolves is refused with) — because the caller fixes the request by
/// naming a connector that is there. [`refusal`] appends the request id, once.
#[must_use]
pub fn no_such_pca_connector(pca_connector_arn: &str) -> String {
    format!("PCA connector: '{pca_connector_arn}' does not exist.")
}

/// The `InvalidParameterException` diagnostic an enable naming a connector that is not
/// ready to enroll is refused with: the connector is there, but it is not in the state the
/// operation requires of it ("the connector must be in an active state").
#[must_use]
pub fn pca_connector_not_active(pca_connector_arn: &str) -> String {
    format!(
        "PCA connector: '{pca_connector_arn}' is not in an active state, so it cannot enroll \
         certificates for this directory."
    )
}

/// The `InvalidParameterException` diagnostic an enable naming a connector of ANOTHER
/// directory is refused with: a connector enrolls for the one directory it was created
/// for, so it is not accessible from this one.
#[must_use]
pub fn pca_connector_of_another_directory(pca_connector_arn: &str, directory_id: &str) -> String {
    format!(
        "PCA connector: '{pca_connector_arn}' was not created for directory {directory_id}, so \
         it cannot enroll certificates for it."
    )
}

/// Why the connector an enable names cannot enroll a directory's clients, or `None` when
/// nothing is wrong with it.
///
/// Decided in the store of the service that OWNS connectors, so a connector exists here
/// only if that service reports it from its own read — and the statuses the clock has
/// already made due are materialised there first, exactly as that service's own reads do,
/// so a connector the clock has brought into service is in service whichever plane asks.
///
/// An engine carrying no connector store holds no connector at all, which is
/// [`ConnectorDefect::Absent`]: there is no component that could be enrolling.
///
/// LOCK ORDER: the caller holds this service's own store first and this one second, which
/// is the order every cross-service reader in this tree keeps.
#[must_use]
pub fn connector_defect<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    pca_connector_arn: &str,
    now: SystemTime,
) -> Option<ConnectorDefect> {
    let Some(state) = pca_connector::service_state(engine) else {
        return Some(ConnectorDefect::Absent);
    };
    let mut store = state.lock();
    store.settle(now);
    let Some(connector) = store.connectors.get(pca_connector_arn) else {
        return Some(ConnectorDefect::Absent);
    };
    if connector.directory_id != directory_id {
        return Some(ConnectorDefect::OtherDirectory);
    }
    if connector.status != pca_connector::CONNECTOR_STATUS_ACTIVE {
        return Some(ConnectorDefect::NotActive);
    }
    None
}

/// What [`connector_defect`] found: which refusal the enable answers, and nothing about the
/// connector's internals — the diagnostic names the value the request carried, so no state
/// of another service's resource is disclosed beyond the fact that refused the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorDefect {
    /// The service that owns connectors holds none under that ARN.
    Absent,
    /// It holds one, but not in the active state the enable requires of it.
    NotActive,
    /// It holds one, created for another directory.
    OtherDirectory,
}

/// One directory's CA enrollment policy.
///
/// The connector is held as the enable passed it, because that is what
/// `DescribeCAEnrollmentPolicy$PcaConnectorArn` reports — "the ARN of the … connector that
/// is configured for automatic certificate enrollment in this directory" — and a later
/// enable replaces it. The status is never the caller's: see the module docs.
#[derive(Debug, Clone)]
pub struct CaEnrollmentPolicyRecord {
    /// The `PcaConnectorArn` the enable named, exactly as it arrived.
    pub pca_connector_arn: String,
    /// One of the modelled `CaEnrollmentPolicyStatus` values
    /// ([`crate::ops::types::CA_ENROLLMENT_POLICY_STATUS_VALUES`]).
    pub status: String,
    /// Why the policy is in that status, as
    /// `DescribeCAEnrollmentPolicy$CaEnrollmentPolicyStatusReason` reports it —
    /// "particularly useful when the policy is in an error or transitional state".
    ///
    /// `None` on every path this emulator takes today, which is why the member is
    /// structurally absent from the read: the statuses that carry a reason are the ones
    /// nothing here reaches — `Failed`, which an activation this emulator never fails
    /// would set, and `Impaired`, which the network between the directory and its
    /// connector would. The field is held rather than left out so whichever of those two
    /// transitions is emulated first fills it where the model says it belongs, instead of
    /// a reason being invented beside the status.
    pub status_reason: Option<String>,
    /// When the status transition the last toggle owes falls due; `None` once there is
    /// none left to make. See [`TRANSITION_SPAN`].
    pub status_due_at: Option<SystemTime>,
    /// When the policy was last modified, as
    /// `DescribeCAEnrollmentPolicy$LastUpdatedDateTime` reports it: the instant of the
    /// toggle, and then the instant the transition that toggle owed fell due.
    pub last_updated: SystemTime,
}

impl CaEnrollmentPolicyRecord {
    /// The record an `EnableCAEnrollmentPolicy` at `now` writes: the connector it named,
    /// the transient status the activation answers in, and the instant that status settles.
    #[must_use]
    pub fn enabling(pca_connector_arn: String, now: SystemTime) -> Self {
        Self {
            pca_connector_arn,
            status: CA_ENROLLMENT_POLICY_IN_PROGRESS.to_string(),
            status_reason: None,
            status_due_at: Some(now + TRANSITION_SPAN),
            last_updated: now,
        }
    }

    /// Record the `DisableCAEnrollmentPolicy` at `now`: the deactivation answers in its own
    /// transient status, which settles one span later the same way the activation's does.
    pub fn start_disabling(&mut self, now: SystemTime) {
        self.status = CA_ENROLLMENT_POLICY_DISABLING.to_string();
        self.status_due_at = Some(now + TRANSITION_SPAN);
        self.last_updated = now;
    }

    /// Materialise the transition this policy's last toggle owes, when the clock has
    /// already made it due at `now`: the transient activation settles on
    /// [`CA_ENROLLMENT_POLICY_SUCCESS`] and the transient deactivation on
    /// [`CA_ENROLLMENT_POLICY_DISABLED`]. Called only from
    /// [`super::ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`],
    /// which is where every clock-owed write of this service is made.
    pub fn advance_clock(&mut self, now: SystemTime) {
        let Some(due) = self.status_due_at else {
            return;
        };
        if now < due {
            return;
        }
        let settled = match self.status.as_str() {
            CA_ENROLLMENT_POLICY_IN_PROGRESS => CA_ENROLLMENT_POLICY_SUCCESS,
            CA_ENROLLMENT_POLICY_DISABLING => CA_ENROLLMENT_POLICY_DISABLED,
            // No other status carries a due instant, so there is nothing to settle: a
            // status this emulator does not move out of keeps whatever it holds.
            _ => return,
        };
        self.status = settled.to_string();
        self.last_updated = due;
        self.status_due_at = None;
    }

    /// Whether this policy is one an enable cannot make a second of: whether it is active
    /// or still on its way out of being active.
    ///
    /// A policy whose deactivation is still outstanding is STILL the directory's policy —
    /// the enrollment it configures is withdrawn only when the status settles — so an
    /// enable meets the same entity either way.
    #[must_use]
    pub fn in_force(&self) -> bool {
        self.status != CA_ENROLLMENT_POLICY_DISABLED
    }
}

impl super::Directory {
    /// The `CaEnrollmentPolicyStatus` this directory reports: the status of the policy an
    /// enable recorded, and [`CA_ENROLLMENT_POLICY_DISABLED`] for a directory that holds
    /// none — "the policy is inactive and automatic certificate enrollment is not
    /// available", which is exactly the position of a directory nothing has enrolled.
    ///
    /// THE one place that member is derived, so every operation that reports a directory's
    /// policy reports the same status.
    #[must_use]
    pub fn ca_enrollment_policy_status(&self) -> &str {
        self.ca_enrollment_policy
            .as_ref()
            .map_or(CA_ENROLLMENT_POLICY_DISABLED, |policy| &policy.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a toggle answers in its own TRANSIENT status and the clock settles it one span
    /// later — the activation on `Success`, the deactivation on `Disabled` — and nothing
    /// settles before that instant.
    #[test]
    fn each_toggle_settles_one_span_after_the_call() {
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // (case, the record the toggle wrote, the transient status, the settled status)
        let rows: Vec<(&str, CaEnrollmentPolicyRecord, &str, &str)> = vec![
            (
                "an activation",
                CaEnrollmentPolicyRecord::enabling("arn".to_string(), started),
                CA_ENROLLMENT_POLICY_IN_PROGRESS,
                CA_ENROLLMENT_POLICY_SUCCESS,
            ),
            (
                "a deactivation",
                {
                    let mut record =
                        CaEnrollmentPolicyRecord::enabling("arn".to_string(), started);
                    record.advance_clock(started + TRANSITION_SPAN);
                    record.start_disabling(started + TRANSITION_SPAN);
                    record
                },
                CA_ENROLLMENT_POLICY_DISABLING,
                CA_ENROLLMENT_POLICY_DISABLED,
            ),
        ];
        for (case, record, transient, settled) in rows {
            let toggled_at = record.last_updated;
            let mut record = record;
            assert_eq!(
                record.status, transient,
                "case {case:?}: expected the transient {transient}, got {:?}",
                record.status
            );
            // An instant before the due one changes nothing.
            record.advance_clock(toggled_at + TRANSITION_SPAN - Duration::from_millis(1));
            assert_eq!(
                record.status, transient,
                "case {case:?}: expected {transient} before the transition falls due, got \
                 {:?}",
                record.status
            );
            record.advance_clock(toggled_at + TRANSITION_SPAN);
            assert_eq!(
                record.status, settled,
                "case {case:?}: expected {settled} once it has, got {:?}",
                record.status
            );
            assert_eq!(
                record.last_updated,
                toggled_at + TRANSITION_SPAN,
                "case {case:?}: expected the policy last modified at the instant the \
                 transition fell due, got {:?}",
                record.last_updated
            );
            assert!(
                record.status_due_at.is_none(),
                "case {case:?}: expected no transition left to make, got {:?}",
                record.status_due_at
            );
            // Settling is idempotent: a later pass of the clock finds nothing to do.
            record.advance_clock(toggled_at + TRANSITION_SPAN + Duration::from_secs(60));
            assert_eq!(
                record.status, settled,
                "case {case:?}: expected a settled policy to stay {settled}, got {:?}",
                record.status
            );
        }
    }

    /// RULE: every status a toggle of this service records is one the model declares, and a
    /// policy counts as in force until it has settled on `Disabled`.
    #[test]
    fn the_statuses_recorded_are_modelled_and_only_disabled_is_not_in_force() {
        let statuses = [
            CA_ENROLLMENT_POLICY_IN_PROGRESS,
            CA_ENROLLMENT_POLICY_SUCCESS,
            CA_ENROLLMENT_POLICY_DISABLING,
            CA_ENROLLMENT_POLICY_DISABLED,
        ];
        for status in statuses {
            assert!(
                crate::ops::types::CA_ENROLLMENT_POLICY_STATUS_VALUES.contains(&status),
                "case {status:?}: expected a modelled CaEnrollmentPolicyStatus one of {:?}",
                crate::ops::types::CA_ENROLLMENT_POLICY_STATUS_VALUES
            );
            let record = CaEnrollmentPolicyRecord {
                pca_connector_arn: "arn".to_string(),
                status: status.to_string(),
                status_reason: None,
                status_due_at: None,
                last_updated: SystemTime::UNIX_EPOCH,
            };
            let in_force = record.in_force();
            assert_eq!(
                in_force,
                status != CA_ENROLLMENT_POLICY_DISABLED,
                "case {status:?}: expected in_force to be {}, got {in_force}",
                status != CA_ENROLLMENT_POLICY_DISABLED
            );
        }
    }
}
