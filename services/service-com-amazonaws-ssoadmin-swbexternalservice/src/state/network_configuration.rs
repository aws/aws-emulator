// SPDX-License-Identifier: CC0-1.0

//! The NETWORK ACCESS CONTROLS an instance is configured with: the record
//! `UpdateInstance` writes and `DescribeInstance` reports.
//!
//! An instance starts with NONE — `DescribeInstance$NetworkConfigurationDetails` is
//! "Present only after a network configuration has been set on the instance" — so the
//! record is held as an `Option` on the instance and the member is absent until a caller
//! sets one, rather than reported as a configuration nobody asked for.

use crate::ops::types::NetworkConfigurationInfo;

/// The network access controls configured on one instance.
///
/// The lists are held MATERIALISED (empty rather than absent) because the configuration
/// is replaced whole: `UpdateInstance$NetworkConfiguration` "replaces the stored network
/// configuration in its entirety. Any list that you omit from the object, or include as an
/// empty list, is cleared" — so an omitted list and an empty one are the same stored
/// value, and a later read cannot tell a caller it kept a restriction it cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfigurationRecord {
    /// `NetworkConfigurationInfo$VpceAccessRequired` — whether access "must originate
    /// from a VPC endpoint". `false` is the value the member documents as enforcing no
    /// network access control at all, which is why it clears the three lists
    /// ([`Self::from_shape`]).
    pub vpce_access_required: bool,
    /// `NetworkConfigurationInfo$RestrictSourceVpcs` — the VPCs whose endpoints may
    /// reach the instance. Empty means the documented default: "access is allowed from
    /// any VPC in your organization or Amazon Web Services account".
    pub restrict_source_vpcs: Vec<String>,
    /// `NetworkConfigurationInfo$AllowSourceIps` — the CIDR blocks allowed "in addition
    /// to the VPCs allowed by RestrictSourceVpcs". Empty means no additional range.
    pub allow_source_ips: Vec<String>,
    /// `NetworkConfigurationInfo$ExcludeApplicationArns` — the applications exempted
    /// from the controls. Empty means "the controls apply to all applications".
    pub exclude_application_arns: Vec<String>,
}

impl NetworkConfigurationRecord {
    /// The record the modeled `NetworkConfigurationInfo` of an accepted `UpdateInstance`
    /// describes.
    ///
    /// Two documented rules are applied as the record is built, so no later read has to
    /// know them:
    ///
    /// * an omitted list is a CLEARED list ("Any list that you omit from the object, or
    ///   include as an empty list, is cleared");
    /// * `VpceAccessRequired` `false` clears all three — "When this value is false, no
    ///   network access control is enforced and the values of RestrictSourceVpcs,
    ///   AllowSourceIps, and ExcludeApplicationArns are cleared".
    pub fn from_shape(configuration: &NetworkConfigurationInfo) -> Self {
        if !configuration.vpce_access_required {
            return Self::default();
        }
        Self {
            vpce_access_required: true,
            restrict_source_vpcs: configuration.restrict_source_vpcs.clone().unwrap_or_default(),
            allow_source_ips: configuration.allow_source_ips.clone().unwrap_or_default(),
            exclude_application_arns: configuration
                .exclude_application_arns
                .clone()
                .unwrap_or_default(),
        }
    }

    /// This record as the modeled `NetworkConfigurationInfo` shape.
    ///
    /// A cleared list is reported ABSENT rather than as an empty list: that is the state
    /// an omitted list leaves behind, so a caller reads back the configuration it stated
    /// and not a list it never sent.
    pub fn to_shape(&self) -> NetworkConfigurationInfo {
        NetworkConfigurationInfo {
            vpce_access_required: self.vpce_access_required,
            restrict_source_vpcs: reported(&self.restrict_source_vpcs),
            allow_source_ips: reported(&self.allow_source_ips),
            exclude_application_arns: reported(&self.exclude_application_arns),
        }
    }
}

/// One stored control list as it is reported: absent when it is cleared.
fn reported(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}
