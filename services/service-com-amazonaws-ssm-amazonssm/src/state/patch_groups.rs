// SPDX-License-Identifier: CC0-1.0

//! Which patch baseline a PATCH GROUP is patched against: the registration
//! `RegisterPatchBaselineForPatchGroup` writes, the one
//! `GetPatchBaselineForPatchGroup` reports, the one
//! `DeregisterPatchBaselineForPatchGroup` takes away, and the ones `DescribePatchGroups`
//! lists.
//!
//! A patch group is not a resource of this service: it is a VALUE a managed node carries as
//! a tag ([`super::patch_runs::PATCH_GROUP_TAG_KEYS`]), and what a register persists is the
//! pairing between that value and a baseline — "Registers a patch baseline for a patch
//! group" (`RegisterPatchBaselineForPatchGroup`). So the store holds the pairing and nothing
//! else about a group: a group no register has named holds no entry, and one whose baseline
//! was deleted holds none again.
//!
//! The pairing is per OPERATING SYSTEM. `GetPatchBaselineForPatchGroup` takes an
//! `OperatingSystem` beside the group and reports one back
//! (`smithy_query(query="operation", operation="GetPatchBaselineForPatchGroup")`), so one
//! group can be patched against a baseline of each operating system — and registering a
//! SECOND baseline of an operating system a group already has one for is what
//! `AlreadyExistsException` is documented for: "Error returned if an attempt is made to
//! register a patch group with a patch baseline that is already registered with a different
//! patch baseline".
//!
//! What a read reports for a group no register has named is the DEFAULT baseline of the
//! operating system asked about ([`ComAmazonawsSsmAmazonssmState::default_patch_baseline`]):
//! the read "retrieves the patch baseline that should be used for the specified patch group"
//! and models no not-found error at all, and what should be used for a group nothing has
//! registered a baseline for is the default of its operating system — which is also the
//! baseline Patch Manager patches such a node against
//! ([`ComAmazonawsSsmAmazonssmState::patch_baseline_in_force`]).

use std::collections::BTreeMap;

use super::patch_baselines::{
    PatchBaselineIdentityRecord, ResolvedBaseline, FILTER_KEY_NAME_PREFIX,
    FILTER_KEY_OPERATING_SYSTEM,
};
use super::ComAmazonawsSsmAmazonssmState;
use crate::ops::types::PatchGroupPatchBaselineMapping;

/// How many patch-group registrations this store holds across every Region.
///
/// Emulator resource guard, NOT a published AWS quota: the Region a registration is recorded
/// under comes from the request's own credential scope (`super::request_region`) and the
/// patch group is a free-form value the request carries, so without a ceiling a caller could
/// grow the store without bound. A register that REPLACES an entry already held is always
/// accepted, so the guard can never wedge a group an account has already registered.
pub const MAX_PATCH_GROUP_REGISTRATIONS: usize = 1_000;

/// Why a register cannot be served, as the operation's own modelled refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationRefusal {
    /// The group already has a DIFFERENT baseline of that operating system, which is what
    /// `AlreadyExistsException` is documented for. Carries the baseline it is registered
    /// with, so the refusal can name it.
    AlreadyRegistered(String),
    /// This store can hold no further registration ([`MAX_PATCH_GROUP_REGISTRATIONS`]),
    /// which is what `ResourceLimitExceededException` reports.
    StoreFull,
}

/// How this service states that a patch group is already registered with another baseline.
///
/// `AlreadyExistsException`'s own documentation is "Error returned if an attempt is made to
/// register a patch group with a patch baseline that is already registered with a different
/// patch baseline", and the refusal NAMES both values the caller needs to act on: the group
/// it asked about and the baseline that group is registered with.
pub fn already_registered_message(patch_group: &str, baseline_id: &str) -> String {
    format!(
        "Patch group {patch_group} is already registered with patch baseline {baseline_id}"
    )
}

/// How this service states that it can hold no further patch-group registration.
///
/// `ResourceLimitExceededException`'s own documentation is "Error returned when the caller
/// has exceeded the default resource quotas", worded over the ceiling this store holds
/// ([`MAX_PATCH_GROUP_REGISTRATIONS`]) — an emulator guard rather than a published quota,
/// which is why the sentence names the count rather than quoting a published one.
pub fn registrations_store_full_message() -> String {
    format!(
        "This endpoint holds the {MAX_PATCH_GROUP_REGISTRATIONS} patch group registrations \
         it can hold"
    )
}

/// How this service states that a baseline cannot be deleted while a patch group is
/// registered with it.
///
/// `ResourceInUseException`'s own documentation is "Error returned if an attempt is made to
/// delete a patch baseline that is registered for a patch group", and the refusal names the
/// group that holds it so the caller knows which registration to take away first: the fixed
/// text states what the delete was refused for, and the group it names is the one the store
/// holds the registration under.
pub fn baseline_in_use_message(patch_group: &str) -> String {
    format!("PatchBaseline to be deleted is used for Patch Group: {patch_group}")
}

impl ComAmazonawsSsmAmazonssmState {
    /// The baseline `patch_group` is REGISTERED with for `operating_system` at `region`'s
    /// endpoint, or `None` when nothing has registered one there.
    ///
    /// A recorded registration is resolved against the baseline store rather than reported as
    /// stored, so a registration that no longer names a baseline this endpoint holds reports
    /// nothing instead of a baseline nothing holds.
    pub fn patch_group_registration(
        &self,
        region: &str,
        patch_group: &str,
        operating_system: &str,
    ) -> Option<ResolvedBaseline> {
        let key = (
            region.to_string(),
            patch_group.to_string(),
            operating_system.to_string(),
        );
        self.patch_group_baselines
            .get(&key)
            .and_then(|reference| self.resolve_patch_baseline(region, reference))
            .filter(|resolved| resolved.operating_system == operating_system)
    }

    /// The baseline a node of `patch_group` is PATCHED AGAINST at `region`'s endpoint: the
    /// one registered for the group, and otherwise the default baseline of
    /// `operating_system`.
    ///
    /// This is the one reading of "the patch baseline that should be used for the specified
    /// patch group" (`GetPatchBaselineForPatchGroup`), so what that read reports and what a
    /// patching operation runs against cannot disagree. A node carrying no patch group at all
    /// belongs to no group, and the baseline in force for it is the default of its operating
    /// system — which is what an empty `patch_group` resolves to here.
    pub fn patch_baseline_in_force(
        &self,
        region: &str,
        patch_group: &str,
        operating_system: &str,
    ) -> Option<ResolvedBaseline> {
        if !patch_group.is_empty() {
            if let Some(registered) =
                self.patch_group_registration(region, patch_group, operating_system)
            {
                return Some(registered);
            }
        }
        self.default_patch_baseline(region, operating_system)
    }

    /// Record `resolved` as the baseline `patch_group` is patched against at `region`'s
    /// endpoint — what `RegisterPatchBaselineForPatchGroup` persists, so a later read of the
    /// group reports it instead of the default of its operating system.
    ///
    /// The operating system the pairing is keyed by is the BASELINE's own: a baseline applies
    /// to one operating system ([`ResolvedBaseline::operating_system`]), so registering it
    /// says which operating system of the group it patches. Registering the baseline the
    /// group already has is the state the call asks for and is accepted; registering a
    /// DIFFERENT one for that operating system is refused
    /// ([`RegistrationRefusal::AlreadyRegistered`]).
    pub fn register_patch_baseline_for_patch_group(
        &mut self,
        region: &str,
        patch_group: &str,
        resolved: &ResolvedBaseline,
    ) -> Result<(), RegistrationRefusal> {
        let key = (
            region.to_string(),
            patch_group.to_string(),
            resolved.operating_system.clone(),
        );
        if let Some(held) = self.patch_group_baselines.get(&key) {
            let held = self
                .resolve_patch_baseline(region, held)
                .map(|resolved| resolved.baseline_id);
            if let Some(held) = held.filter(|held| held != &resolved.baseline_id) {
                return Err(RegistrationRefusal::AlreadyRegistered(held));
            }
        } else if self.patch_group_baselines.len() >= MAX_PATCH_GROUP_REGISTRATIONS {
            return Err(RegistrationRefusal::StoreFull);
        }
        self.patch_group_baselines
            .insert(key, resolved.reference.clone());
        Ok(())
    }

    /// Take away the registration between `patch_group` and the baseline `baseline_id` names
    /// at `region`'s endpoint, reporting whether one was held.
    ///
    /// What `DeregisterPatchBaselineForPatchGroup` persists: the pairing is gone, so a later
    /// read of the group reports the default of its operating system again and a patching
    /// operation on a node of that group runs against that default. A group registered with
    /// ANOTHER baseline keeps that registration — the call names the baseline to deregister —
    /// and the registrations of every other Region are left exactly as they were.
    pub fn deregister_patch_baseline_for_patch_group(
        &mut self,
        region: &str,
        patch_group: &str,
        baseline_id: &str,
    ) -> bool {
        let held: Vec<(String, String, String)> = self
            .patch_group_baselines
            .iter()
            .filter(|((held_region, held_group, _), reference)| {
                held_region == region
                    && held_group == patch_group
                    && self
                        .patch_baseline_query(region, reference)
                        .is_some_and(|named| named == baseline_id)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &held {
            self.patch_group_baselines.remove(key);
        }
        !held.is_empty()
    }

    /// The patch groups registered with the baseline `baseline_id` names at `region`'s
    /// endpoint, in ascending group order: "Patch groups included in the patch baseline"
    /// (`GetPatchBaseline$PatchGroups`) — a group is included in a baseline by registering it
    /// there, so this is the projection of the registrations that name this baseline.
    ///
    /// Without repeats: a group registered for two operating systems of one baseline is in
    /// that baseline once.
    pub fn patch_groups_of_patch_baseline(&self, region: &str, baseline_id: &str) -> Vec<String> {
        let mut groups: Vec<String> = self
            .patch_group_baselines
            .iter()
            .filter(|((held_region, _, _), reference)| {
                held_region == region
                    && self
                        .patch_baseline_query(region, reference)
                        .is_some_and(|named| named == baseline_id)
            })
            .map(|((_, group, _), _)| group.clone())
            .collect();
        groups.sort();
        groups.dedup();
        groups
    }

    /// Remove every registration of `region` that names the baseline `baseline_id`, as
    /// deleting that baseline does: nothing holds the baseline any more, so no group is
    /// patched against it and every group it had is patched against the default of its
    /// operating system again.
    pub fn drop_patch_group_registrations_of(&mut self, region: &str, baseline_id: &str) {
        let arn = super::patch_baselines::patch_baseline_arn(region, baseline_id);
        self.patch_group_baselines
            .retain(|(held_region, _, _), reference| {
                held_region != region
                    || (reference.as_str() != baseline_id && reference.as_str() != arn)
            });
    }

    /// Every registration reachable through `region`'s endpoint, as the mapping between a
    /// patch group and the baseline it is registered with — what `DescribePatchGroups`
    /// lists: "Lists all patch groups that have been registered with patch baselines".
    ///
    /// A READ: whatever a register persisted is what this projects, and nothing is written.
    /// The baseline side is the SAME basic information a baseline listing reports
    /// ([`ComAmazonawsSsmAmazonssmState::patch_baseline_identities`]), so the two reads
    /// cannot disagree about a baseline's name, operating system, description or whether it
    /// is the default of its operating system. A registration whose baseline this endpoint
    /// no longer holds reports nothing rather than a baseline nothing holds — the same
    /// reading [`Self::patch_group_registration`] takes.
    ///
    /// The order is total and taken from the registrations themselves — by patch group, with
    /// the baseline's id breaking a tie for a group registered with a baseline of each of
    /// several operating systems — so it is independent of the request, two reads report the
    /// same sequence, and a page resumes where the last one stopped.
    pub fn patch_group_mappings(&self, region: &str) -> Vec<PatchGroupMappingRecord> {
        let identities: BTreeMap<String, PatchBaselineIdentityRecord> = self
            .patch_baseline_identities(region)
            .into_iter()
            .map(|identity| (identity.baseline_id.clone(), identity))
            .collect();
        let mut mappings: Vec<PatchGroupMappingRecord> = self
            .patch_group_baselines
            .iter()
            .filter(|((held_region, _, _), _)| held_region == region)
            .filter_map(|((_, patch_group, _), reference)| {
                let resolved = self.resolve_patch_baseline(region, reference)?;
                let baseline = identities.get(&resolved.baseline_id)?.clone();
                Some(PatchGroupMappingRecord {
                    patch_group: patch_group.clone(),
                    baseline,
                })
            })
            .collect();
        mappings.sort_by(|left, right| {
            (&left.patch_group, &left.baseline.baseline_id)
                .cmp(&(&right.patch_group, &right.baseline.baseline_id))
        });
        mappings
    }
}

// ---------------------------------------------------------------------------
// Listing the registrations an endpoint holds
// ---------------------------------------------------------------------------

/// Every key a patch group listing filters on, in the order its own documentation lists
/// them: "Supported keys for DescribePatchGroups include the following: NAME_PREFIX Sample
/// values: AWS- | My-. OPERATING_SYSTEM Sample values: AMAZON_LINUX | SUSE | WINDOWS"
/// (`smithy_query(query="doc", shape="DescribePatchGroupsRequest", member="Filters")`).
///
/// Both name a property of the BASELINE a group is registered with — the sample values of
/// `NAME_PREFIX` are baseline names and `OPERATING_SYSTEM` is the operating system a
/// baseline applies to — so a filter narrows the listing to the registrations whose baseline
/// meets it ([`super::patch_baselines::PatchBaselineIdentityRecord::matches_filter`], the one
/// matcher a baseline listing uses). `OWNER` is a key of the baseline listing and NOT of
/// this one, so it is refused here.
pub const PATCH_GROUP_FILTER_KEYS: &[&str] =
    &[FILTER_KEY_NAME_PREFIX, FILTER_KEY_OPERATING_SYSTEM];

/// Whether `key` is a key a patch group listing filters on ([`PATCH_GROUP_FILTER_KEYS`]).
pub fn is_patch_group_filter_key(key: &str) -> bool {
    PATCH_GROUP_FILTER_KEYS.contains(&key)
}

/// What separates the two halves of the cursor a page of registrations resumes at.
///
/// Neither half can carry it: a patch group matches `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`
/// (`PatchGroupPatchBaselineMapping$PatchGroup`) and a baseline id `^[a-zA-Z0-9_\-:/]{20,128}$`
/// (`PatchBaselineIdentity$BaselineId`), so a cursor names exactly one registration.
const CURSOR_SEPARATOR: char = '|';

/// One registration an endpoint holds, as the mapping a listing reports: the patch group,
/// and the basic information of the baseline it is registered with.
#[derive(Debug, Clone)]
pub struct PatchGroupMappingRecord {
    /// "The name of the patch group registered with the patch baseline."
    pub patch_group: String,
    /// "The patch baseline the patch group is registered with", as the basic information
    /// every baseline read of this service reports about it.
    pub baseline: PatchBaselineIdentityRecord,
}

impl PatchGroupMappingRecord {
    /// The `NextToken` a page hands out to resume AT this registration.
    ///
    /// A listing walks `(patch group, baseline id)` pairs in ascending order, so the position
    /// it resumes at is that pair and not an offset into a list that may have changed between
    /// two pages.
    pub fn page_token(&self) -> String {
        format!(
            "{}{CURSOR_SEPARATOR}{}",
            self.patch_group, self.baseline.baseline_id
        )
    }

    /// This registration as the mapping a listing reports.
    pub fn to_mapping(&self) -> PatchGroupPatchBaselineMapping {
        PatchGroupPatchBaselineMapping {
            patch_group: Some(self.patch_group.clone()),
            baseline_identity: Some(self.baseline.to_identity()),
        }
    }
}
