// SPDX-License-Identifier: CC0-1.0

//! MODIFYING a capacity provider: which configuration an update may carry for the provider
//! it names, what each member of that configuration leaves in place, and the `updateStatus`
//! an accepted update is in flight under.
//!
//! An update carries "the parameters to update" — one of the two configurations a provider
//! is built on, never the capacity it is bound onto: the Auto Scaling group is not a member
//! of `AutoScalingGroupProviderUpdate` at all, so a provider stays over the group it was
//! created for. Within the configuration a member the request names REPLACES the stored
//! value and one it leaves out keeps it, since what the request carries is what it asks to
//! change.
//!
//! What the update leaves behind is the configuration IN FORCE for the provider, so every
//! member of it that documents a value Amazon ECS applies when nothing names it stands at
//! that value — each managed-scaling number ([`stored_managed_scaling`]), managed draining
//! on and managed termination protection off — exactly as a create's configuration is
//! stored, so a client reading it back is never told "no setting" where Amazon ECS has one.

use super::*;
use crate::ops::types::{
    AutoScalingGroupProviderUpdate, InstanceLaunchTemplate,
    UpdateManagedInstancesProviderConfiguration,
};

/// `CapacityProvider$updateStatus` while an accepted update is still being applied — the
/// `CapacityProviderUpdateStatus` value for it (`smithy_query` query="shape"
/// shape="CapacityProviderUpdateStatus"). The change is to the capacity behind the provider
/// ("These changes only apply to new Amazon ECS Managed Instances, or EC2 instances, not
/// existing ones"), so the update is reported in flight and
/// [`CAPACITY_PROVIDER_UPDATE_COMPLETE`] is owed by the clock.
pub const CAPACITY_PROVIDER_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// `CapacityProvider$updateStatus` once that update has been applied: the terminal status of
/// an updated provider, reported from the instant the change falls due.
pub const CAPACITY_PROVIDER_UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// `InvalidParameterException$message` for a request that names NEITHER of the two
/// configurations a capacity provider is built on. "You can create capacity providers for
/// Amazon ECS Managed Instances and EC2 instances": a provider is built on exactly one of
/// the two, each describing the whole of where its capacity comes from, so a request that
/// names neither describes no capacity — and asks for no change to one, where the request
/// updates a provider. Shared by every operation that takes the pair.
pub const CAPACITY_PROVIDER_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "either autoScalingGroupProvider or managedInstancesProvider must be specified";

/// `InvalidParameterException$message` for a request that names BOTH of them, which is the
/// other half of the same rule.
pub const CAPACITY_PROVIDER_CONFIGURATION_EXCLUSIVE_MESSAGE: &str =
    "autoScalingGroupProvider and managedInstancesProvider must not both be specified";

/// `InvalidParameterException$message` for an update whose configuration is not the one the
/// provider it names is built on. A provider binds onto exactly one class of capacity
/// (`CapacityProvider$type`), so the configuration of the other class describes nothing
/// about it: the sentence names the member each class is updated through and the remediation
/// clause every parameter refusal of this service closes on.
pub const CAPACITY_PROVIDER_CONFIGURATION_CLASS_MESSAGE: &str =
    "The specified configuration doesn't match the type of the specified capacity provider. \
     Specify autoScalingGroupProvider for an Auto Scaling group capacity provider or \
     managedInstancesProvider for a managed instances capacity provider and try again.";

/// `InvalidParameterException$message` for an update of a CLUSTER-SCOPED capacity provider
/// that does not name the cluster the provider is scoped to. The
/// `UpdateCapacityProvider$cluster` doc states the whole rule in one sentence: "Managed
/// instances capacity providers are cluster-scoped and can only be updated within their
/// associated cluster."
pub const CLUSTER_SCOPED_CAPACITY_PROVIDER_UPDATE_MESSAGE: &str =
    "Managed instances capacity providers are cluster-scoped and can only be updated within \
     their associated cluster.";

/// The sentence a request that does not name exactly one of the two capacity-provider
/// configurations is refused with, and `None` when it names one.
pub fn capacity_provider_configuration_violation(
    auto_scaling_group_provider: bool,
    managed_instances_provider: bool,
) -> Option<&'static str> {
    match (auto_scaling_group_provider, managed_instances_provider) {
        (false, false) => Some(CAPACITY_PROVIDER_CONFIGURATION_REQUIRED_MESSAGE),
        (true, true) => Some(CAPACITY_PROVIDER_CONFIGURATION_EXCLUSIVE_MESSAGE),
        _ => None,
    }
}

/// The `autoScalingGroupProvider` a provider is left bound onto by an update of it.
///
/// The group itself is carried over from the stored binding: the update has no member for it
/// ([`AutoScalingGroupProviderUpdate`] is `{managedScaling, managedTerminationProtection,
/// managedDraining}`), so an update changes how Amazon ECS drives the group and never which
/// group the provider is over. Each of the other members is replaced where the update names
/// it and kept where it does not, since what the request carries is what it asks to change,
/// and a `managedScaling` block the update carries is stored the way a create's is.
///
/// What the update leaves is the configuration IN FORCE for the provider, so a member
/// neither the update nor the stored configuration names stands at the value its own
/// documentation gives it — managed termination protection off
/// ([`MANAGED_TERMINATION_PROTECTION_DISABLED`]), managed draining on, and each
/// managed-scaling number at its own default — rather than at nothing at all: a client
/// reading the configuration back cannot be told "no setting" where Amazon ECS has one in
/// force.
pub fn updated_auto_scaling_group_provider(
    stored: &AutoScalingGroupProvider,
    update: &AutoScalingGroupProviderUpdate,
) -> AutoScalingGroupProvider {
    AutoScalingGroupProvider {
        auto_scaling_group_arn: stored.auto_scaling_group_arn.clone(),
        managed_scaling: match update.managed_scaling.as_ref() {
            Some(requested) => Some(stored_managed_scaling(Some(requested))),
            None => Some(stored_managed_scaling(stored.managed_scaling.as_ref())),
        },
        managed_termination_protection: update
            .managed_termination_protection
            .clone()
            .or_else(|| stored.managed_termination_protection.clone())
            .or_else(|| Some(MANAGED_TERMINATION_PROTECTION_DISABLED.to_string())),
        managed_draining: update
            .managed_draining
            .clone()
            .or_else(|| stored.managed_draining.clone())
            .or_else(|| Some(MANAGED_DRAINING_ENABLED.to_string())),
    }
}

/// The `managedInstancesProvider` an update leaves on the provider.
///
/// `infrastructureRoleArn` and `instanceLaunchTemplate` are required members of the update,
/// so both are written; within the launch template every member the update names replaces
/// the stored one and every member it leaves out keeps it, which is what "the updated launch
/// template configuration" means for a template the provider already has. The members the
/// update has NO member for — the capacity option its instances are launched under, whether
/// they run FIPS-validated modules and whether they have Nitro Sandbox enabled — are carried
/// over for the same reason the Auto Scaling group is: an update cannot change what it cannot
/// name.
///
/// `monitoring` is taken already resolved to the spelling the modelled value set lists
/// ([`managed_instances_monitoring_option`]), so the stored template reports a value inside
/// that set whichever spelling the update was written in.
pub fn updated_managed_instances_provider(
    stored: &ManagedInstancesProvider,
    update: &UpdateManagedInstancesProviderConfiguration,
    monitoring: Option<String>,
) -> ManagedInstancesProvider {
    let template = stored.instance_launch_template.clone().unwrap_or_default();
    let requested = &update.instance_launch_template;
    ManagedInstancesProvider {
        infrastructure_role_arn: Some(update.infrastructure_role_arn.clone()),
        instance_launch_template: Some(InstanceLaunchTemplate {
            ec2_instance_profile_arn: requested
                .ec2_instance_profile_arn
                .clone()
                .unwrap_or(template.ec2_instance_profile_arn),
            network_configuration: requested
                .network_configuration
                .clone()
                .unwrap_or(template.network_configuration),
            storage_configuration: requested
                .storage_configuration
                .clone()
                .or(template.storage_configuration),
            local_storage_configuration: requested
                .local_storage_configuration
                .clone()
                .or(template.local_storage_configuration),
            monitoring: monitoring.or(template.monitoring),
            capacity_option_type: template.capacity_option_type,
            instance_metadata_tags_propagation: requested
                .instance_metadata_tags_propagation
                .or(template.instance_metadata_tags_propagation),
            instance_requirements: requested
                .instance_requirements
                .clone()
                .or(template.instance_requirements),
            fips_enabled: template.fips_enabled,
            capacity_reservations: requested
                .capacity_reservations
                .clone()
                .or(template.capacity_reservations),
            nitro_enclaves_enabled: template.nitro_enclaves_enabled,
        }),
        propagate_tags: update
            .propagate_tags
            .clone()
            .or_else(|| stored.propagate_tags.clone()),
        infrastructure_optimization: update
            .infrastructure_optimization
            .clone()
            .or_else(|| stored.infrastructure_optimization.clone()),
        auto_repair_configuration: update
            .auto_repair_configuration
            .clone()
            .or_else(|| stored.auto_repair_configuration.clone()),
    }
}

impl CapacityProviderRecord {
    /// Record on this provider that an update of it was accepted: the new configuration is
    /// what every later read reports from this instant, while APPLYING it to the capacity
    /// behind the provider is the update left in flight
    /// ([`CAPACITY_PROVIDER_UPDATE_IN_PROGRESS`]) and the clock owes its completion
    /// [`TRANSITION_SPAN`] later.
    ///
    /// The provider's own `status` is untouched: the capacity it is bound onto is the
    /// capacity it had, so a provider that was usable stays usable across the update and one
    /// whose create Amazon ECS is still standing up stays under that create.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_status = Some(CAPACITY_PROVIDER_UPDATE_IN_PROGRESS.to_string());
        self.update_due_at = Some(now + TRANSITION_SPAN);
    }
}
