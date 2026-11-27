// SPDX-License-Identifier: CC0-1.0

//! The groups another service MANAGES, and whether a read reports them and the resources
//! that hang off them.
//!
//! # The rule this module implements
//!
//! An Auto Scaling group may name the entity that manages it: `Operator` "describes the
//! entity that manages an Auto Scaling group"
//! (`smithy_query(query="shape",shape="Operator")`), whose `Principal` is "the service
//! principal that is authorized to manage the Auto Scaling group"
//! (`smithy_query(query="doc",shape="Operator",member="Principal")`). A group created with
//! one is a MANAGED Auto Scaling group — a group another service maintains on the account's
//! behalf — and a group created without one is the account's own.
//!
//! Every read of this service that documents `IncludeManagedResources` states the same rule
//! about it: "Specifies whether to include managed Auto Scaling groups in the response. When
//! set to `true` (default), the response includes managed Auto Scaling groups"
//! (`smithy_query(query="doc",shape="AutoScalingGroupNamesType",member="IncludeManagedResources")`,
//! and the reads of a group's instances, tags, scaling policies, scheduled actions, scaling
//! activities and notification configurations each say it of what they report). So the
//! member is read the same way wherever it appears — absent is `true` — and the value
//! `false` leaves out every item whose group an operator manages, whatever else the request
//! asked to be narrowed by. It only narrows a read: nothing it says changes what the account
//! holds.

use super::{AutoScalingState, StoredGroup};

/// `IncludeManagedResources` as every read of this service reads it when the request leaves
/// it out: "When set to `true` (default), the response includes managed Auto Scaling
/// groups", so a request that states nothing is served the managed resources too.
pub const DEFAULT_INCLUDE_MANAGED_RESOURCES: bool = true;

/// Whether a read reports managed resources at all, from the `IncludeManagedResources` the
/// request carried.
///
/// THE ONE reading of that member, so the seven reads that document it cannot disagree about
/// what an absent one means.
#[must_use]
pub fn includes_managed(requested: Option<bool>) -> bool {
    requested.unwrap_or(DEFAULT_INCLUDE_MANAGED_RESOURCES)
}

impl StoredGroup {
    /// Whether this is a MANAGED Auto Scaling group: one an operator service principal
    /// manages, rather than one the account maintains itself.
    #[must_use]
    pub fn is_managed(&self) -> bool {
        self.operator.is_some()
    }

    /// Whether a read reports this group, and the resources that belong to it, given what
    /// its `IncludeManagedResources` asked for.
    ///
    /// A group the account maintains itself is reported by every read; a managed one only by
    /// a read that asked for managed resources.
    #[must_use]
    pub fn is_reported(&self, includes_managed: bool) -> bool {
        includes_managed || !self.is_managed()
    }
}

impl AutoScalingState {
    /// Whether a read reports the resources recorded against the group `group_name`, given
    /// what its `IncludeManagedResources` asked for — for the reads whose items name their
    /// group rather than being held inside it.
    ///
    /// A name no group of the account answers to names no managed group: the scaling
    /// activities of a group the account has since deleted are recorded against a name
    /// nothing manages any more, so they are reported whatever the member asked for, exactly
    /// as they are reported for a group that never named an operator.
    #[must_use]
    pub fn reports_group_resources(&self, group_name: &str, includes_managed: bool) -> bool {
        self.groups
            .get(group_name)
            .is_none_or(|group| group.is_reported(includes_managed))
    }
}
