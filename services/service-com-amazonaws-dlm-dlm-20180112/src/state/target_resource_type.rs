// SPDX-License-Identifier: CC0-1.0

//! Which configuration members a policy may state for what it TARGETS.
//!
//! `PolicyDetails$ResourceTypes` is "the target resource type for snapshot and
//! AMI lifecycle policies. Use `VOLUME` to create snapshots of individual
//! volumes or use `INSTANCE` to create multi-volume snapshots from the volumes
//! for an instance", and `Parameters` states what follows from it: "The set of
//! valid parameters depends on the combination of policy type and target
//! resource type" (`smithy_query(query="shape", shape="PolicyDetails")` and
//! `smithy_query(query="shape", shape="Parameters")`).
//!
//! Several configuration members are documented for policies that target ONE of
//! those two resource types only, so a request stating one for a policy that
//! targets the other states a configuration the service cannot apply — the
//! "invalid parameters" the write operations model `InvalidRequestException`
//! for. It is refused before anything is stored, so no stored policy carries a
//! member the resource type it targets admits nowhere and no read reports one.
//!
//! The members, each with the target its own documentation admits
//! (`smithy_query(query="shape", shape="Schedule")`,
//! `smithy_query(query="shape", shape="CreateRule")` and
//! `smithy_query(query="shape", shape="Parameters")`):
//!
//! * `Schedule$VariableTags` — "[AMI policies and snapshot policies that target
//!   instances only] ... Variable tags are only valid for EBS Snapshot
//!   Management – Instance policies", so the policies that take it target
//!   instances;
//! * `CreateRule$Scripts` — "[Custom snapshot policies that target instances
//!   only] Specifies pre and/or post scripts for a snapshot lifecycle policy
//!   that targets instances", stated inside a schedule's `CreateRule`;
//! * `Schedule$ArchiveRule` — "[Custom snapshot policies that target volumes
//!   only] The snapshot archiving rule for the schedule";
//! * `Parameters$ExcludeBootVolume` — "[Custom snapshot policies that target
//!   instances only] Indicates whether to exclude the root volume from
//!   multi-volume snapshot sets";
//! * `Parameters$ExcludeDataVolumeTags` — "[Custom snapshot policies that target
//!   instances only] The tags used to identify data (non-root) volumes to
//!   exclude from multi-volume snapshot sets".
//!
//! Those are every member a CUSTOM policy's configuration carries whose own
//! documentation names one of the two targets: the shapes it is built from
//! (`PolicyDetails`, `Schedule`, the `CreateRule` and the `Parameters` a
//! schedule's configuration nests, and the rules each of those nests in turn)
//! carry no other such sentence, so an entry joins the list only when the model
//! documents another member for one target.
//!
//! What those sentences say about the policy TYPE that takes a member is a
//! different member of the configuration (`PolicyDetails$PolicyType`) and is not
//! judged here: this module judges the target resource type alone, which is the
//! fact the refusal names.
//!
//! A configuration that states no target has none to judge a member against — an
//! event-based policy configures an `EventSource` and `Actions` instead, and a
//! default policy states `PolicyDetails$ResourceType` and carries no schedules
//! or parameters at all ([`custom_policy_only_violation`]) — so nothing is
//! refused for one.

use super::*;
use crate::ops::types::Parameters;

/// The `ResourceTypeValues` member a policy targets to "create snapshots of
/// individual volumes" (`smithy_query(query="shape",
/// shape="ResourceTypeValues")`, whose members are `VOLUME` and `INSTANCE`).
///
/// It is the target of a CUSTOM policy, stated as
/// `PolicyDetails$ResourceTypes`; the identically spelled
/// [`VOLUME_DEFAULT_POLICY_TYPE`] states which kind of DEFAULT policy a request
/// asks for, which is a different member saying a different thing.
const VOLUME_TARGET: &str = "VOLUME";

/// The `ResourceTypeValues` member a policy targets to "create multi-volume
/// snapshots from the volumes for an instance".
const INSTANCE_TARGET: &str = "INSTANCE";

/// One configuration member whose documentation admits one target resource type.
struct TargetRestrictedMember {
    /// The member's own wire name, which is how the refusal names it.
    name: &'static str,
    /// The `ResourceTypeValues` member its documentation admits.
    admitted_target: &'static str,
    /// Whether a configuration states it, wherever the configuration carries it.
    stated: fn(&PolicyDetails) -> bool,
}

impl TargetRestrictedMember {
    /// Whether a policy targeting `targets` may state this member: EVERY target
    /// it states has to be the one this member's documentation admits, so a
    /// configuration is judged on everything it really targets rather than on
    /// one of several types it names.
    fn admits(&self, targets: &[String]) -> bool {
        targets
            .iter()
            .all(|target| target == self.admitted_target)
    }
}

/// Every configuration member documented for one target resource type only, in
/// the order the configuration declares them: the `Schedules` members first
/// (`PolicyDetails` declares `Schedules` before `Parameters`), each in the order
/// its own shape declares it — which is the order the refusal names them in.
const TARGET_RESTRICTED_MEMBERS: [TargetRestrictedMember; 5] = [
    TargetRestrictedMember {
        name: "VariableTags",
        admitted_target: INSTANCE_TARGET,
        stated: variable_tags_stated,
    },
    TargetRestrictedMember {
        name: "Scripts",
        admitted_target: INSTANCE_TARGET,
        stated: scripts_stated,
    },
    TargetRestrictedMember {
        name: "ArchiveRule",
        admitted_target: VOLUME_TARGET,
        stated: archive_rule_stated,
    },
    TargetRestrictedMember {
        name: "ExcludeBootVolume",
        admitted_target: INSTANCE_TARGET,
        stated: exclude_boot_volume_stated,
    },
    TargetRestrictedMember {
        name: "ExcludeDataVolumeTags",
        admitted_target: INSTANCE_TARGET,
        stated: exclude_data_volume_tags_stated,
    },
];

/// True when any of the configuration's schedules states `VariableTags`.
///
/// Judged on EVERY element of `Schedules` (`@length(min: 1, max: 4)`), as every
/// other schedule rule is, so a policy is refused for whichever of its schedules
/// carries the member.
fn variable_tags_stated(details: &PolicyDetails) -> bool {
    schedules(details).any(|schedule| schedule.variable_tags.is_some())
}

/// True when any of the configuration's schedules states `Scripts` on its
/// creation rule.
///
/// Judged on every element of `Schedules`, as the other schedule members are,
/// and on the `CreateRule` each one carries, which is the structure that
/// declares the member.
fn scripts_stated(details: &PolicyDetails) -> bool {
    schedules(details).any(|schedule| {
        schedule
            .create_rule
            .as_ref()
            .is_some_and(|create_rule| create_rule.scripts.is_some())
    })
}

/// True when any of the configuration's schedules states an `ArchiveRule`.
fn archive_rule_stated(details: &PolicyDetails) -> bool {
    schedules(details).any(|schedule| schedule.archive_rule.is_some())
}

/// True when the configuration's `Parameters` states `ExcludeBootVolume`.
fn exclude_boot_volume_stated(details: &PolicyDetails) -> bool {
    parameters(details).is_some_and(|parameters| parameters.exclude_boot_volume.is_some())
}

/// True when the configuration's `Parameters` states `ExcludeDataVolumeTags`.
fn exclude_data_volume_tags_stated(details: &PolicyDetails) -> bool {
    parameters(details).is_some_and(|parameters| parameters.exclude_data_volume_tags.is_some())
}

/// The schedules a configuration carries, none for one that states no
/// `Schedules`.
fn schedules(details: &PolicyDetails) -> impl Iterator<Item = &Schedule> {
    details.schedules.iter().flatten()
}

/// The optional parameters a configuration carries.
fn parameters(details: &PolicyDetails) -> Option<&Parameters> {
    details.parameters.as_ref()
}

/// Why Amazon Data Lifecycle Manager refuses the configuration a request states
/// for what the policy TARGETS, or `None` when every member it states is one the
/// target resource type admits.
///
/// The members judged are the ones listed in this module's documentation, and
/// they are judged against the target the configuration itself states: a request
/// that states a member documented for the other target is refused naming the
/// target it stated and every member refused for it, and nothing else of the
/// request.
///
/// Both write paths run it before the first write, so neither an accepted create
/// nor an accepted update can leave a policy carrying such a member.
pub fn target_resource_type_violation(
    details: Option<&PolicyDetails>,
) -> Option<InvalidRequestException> {
    let details = details?;
    let targets = details.resource_types.as_deref()?;
    if targets.is_empty() {
        return None;
    }
    let refused: Vec<String> = TARGET_RESTRICTED_MEMBERS
        .iter()
        .filter(|member| (member.stated)(details) && !member.admits(targets))
        .map(|member| member.name.to_string())
        .collect();
    (!refused.is_empty()).then(|| parameters_invalid_for_target(targets, &refused))
}

/// How Amazon Data Lifecycle Manager states the refusal of parameters that the
/// resource type a policy targets does not admit: the fixed text below, the
/// target the request stated wrapped in braces, and the members refused.
///
/// The message is built from the request — the target is the one its
/// `PolicyDetails$ResourceTypes` states and the list holds the members it
/// actually stated — so it names the fields the caller has to correct.
fn parameters_invalid_for_target(
    targets: &[String],
    parameters: &[String],
) -> InvalidRequestException {
    invalid_request(format!(
        "The following parameter(s) are invalid for Resource Type {{{}}}: {}",
        targets.join(", "),
        parameters.join(", ")
    ))
}
