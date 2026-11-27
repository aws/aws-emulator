// SPDX-License-Identifier: CC0-1.0

//! `InstanceLaunchTemplate`: the launch template a Managed Instances capacity provider is
//! created with.
//!
//! This is the ONE `InstanceLaunchTemplate` type of this crate — the shape
//! `CreateCapacityProvider` parses the template through and `DescribeCapacityProviders`
//! reports it through.
//!
//! It is declared here rather than beside the other shared shapes because of
//! `skip_validation = ["monitoring"]`, for the reason the update-side shape
//! (`InstanceLaunchTemplateUpdate` in `update_capacity_provider_types`) carries it: that
//! member's own documentation publishes the two monitoring categories in lower case, and
//! Amazon ECS serves a request that spells the option that way, so the modelled value set
//! cannot be enforced as case-sensitive equality at the boundary. The set is still enforced,
//! by the handler that stores the template (`crate::state::managed_instances_monitoring_option`),
//! which also stores the option in the spelling the set lists.
//!
//! Everything else about the shape is generated from the published model snapshot, exactly
//! as it is for every modelled shape: the member names, the shape id and the traits all come
//! from that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.ecs#InstanceLaunchTemplate",
    skip_validation = ["monitoring"]
)]
#[derive(Debug, Clone, Default)]
pub struct InstanceLaunchTemplate {
    pub ec2_instance_profile_arn: String,
    pub network_configuration: ManagedInstancesNetworkConfiguration,
    pub storage_configuration: Option<ManagedInstancesStorageConfiguration>,
    pub local_storage_configuration: Option<ManagedInstancesLocalStorageConfiguration>,
    pub monitoring: Option<String>,
    pub capacity_option_type: Option<String>,
    pub instance_metadata_tags_propagation: Option<bool>,
    pub instance_requirements: Option<InstanceRequirementsRequest>,
    pub fips_enabled: Option<bool>,
    pub capacity_reservations: Option<CapacityReservationRequest>,
    pub nitro_enclaves_enabled: Option<bool>,
}
