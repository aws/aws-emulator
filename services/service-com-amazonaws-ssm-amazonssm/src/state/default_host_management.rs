// SPDX-License-Identifier: CC0-1.0

//! Default Host Management Configuration (DHMC): the IAM role an account registers for its
//! Amazon EC2 instances to be managed as managed nodes, and where that registration lives.
//!
//! DHMC is one account-level SETTING rather than a resource of its own: the permissions
//! `RegisterDefaultHostManagementConfiguration` documents are
//! `ssm:GetServiceSetting`/`ssm:UpdateServiceSetting`, and the ones
//! `DeregisterDefaultHostManagementConfiguration` documents are
//! `ssm:GetServiceSetting`/`ssm:ResetServiceSetting`, both over the
//! `/ssm/managed-instance/default-ec2-instance-management-role` setting
//! ([`ROLE_SETTING_ID`]). So a register is a write of that setting and a deregister a
//! restore of it, through the one store the three service-setting operations read and write
//! ([`super::service_settings`]) — which is why `GetServiceSetting` reports what a register
//! left and what a deregister took away.
//!
//! The value the setting holds is the role's NAME: "for
//! `/ssm/managed-instance/default-ec2-instance-management-role`, enter the name of an IAM
//! role" (`UpdateServiceSettingRequest$SettingValue`). A setting nobody has registered a
//! role for holds the sentinel for an unconfigured name
//! ([`super::service_settings::UNSET_VALUE`]), which is what tells a register whether it
//! would REPLACE a registration — the question `Overwrite` guards.
//!
//! WHICH ROLE is registered depends on the permission model
//! ([`PERMISSION_MODEL_SELF_MANAGED`], [`PERMISSION_MODEL_SERVICE_MANAGED`]): a self-managed
//! registration names its own role, which this service resolves against IAM exactly as it
//! resolves every other caller-supplied role ([`super::activation_role`]); a service-managed
//! one is the role Systems Manager keeps in the IAM reserved path for its own service
//! principal ([`service_managed_role_arn`]), which is a role IAM's own reads resolve rather
//! than one this service invents.

/// The `SettingId` a DHMC registration lives under, as `GetServiceSetting`,
/// `UpdateServiceSetting` and `ResetServiceSetting` name it — the one setting both DHMC
/// operations document their permissions over.
pub const ROLE_SETTING_ID: &str = "/ssm/managed-instance/default-ec2-instance-management-role";

/// The `DHMCPermissionModel` under which the CALLER provides the role: "SELF_MANAGED You
/// provide the role to register in `RoleArn`"
/// (`RegisterDefaultHostManagementConfigurationRequest$PermissionModel`).
pub const PERMISSION_MODEL_SELF_MANAGED: &str = "SELF_MANAGED";

/// The `DHMCPermissionModel` under which Systems Manager provides it: "SERVICE_MANAGED
/// Amazon Web Services Systems Manager creates and manages the role on your behalf." The
/// other value of the same enum.
pub const PERMISSION_MODEL_SERVICE_MANAGED: &str = "SERVICE_MANAGED";

/// How a `SELF_MANAGED` registration that names no role is refused: "if `PermissionModel` is
/// `SELF_MANAGED`, this value is required" (`…Request$RoleArn`). The member is optional in
/// the model because the other permission model must NOT carry it, so which of the two
/// requests is well formed is settled here.
pub const ROLE_ARN_REQUIRED: &str =
    "RoleArn is required when PermissionModel is SELF_MANAGED: it names the self-managed IAM \
     role to register for Default Host Management Configuration.";

/// How a `SERVICE_MANAGED` registration that names one is refused: "if `PermissionModel` is
/// `SERVICE_MANAGED`, this value must not be set" (the same member), because the role is the
/// one Systems Manager keeps rather than one the caller chooses.
pub const ROLE_ARN_NOT_ALLOWED: &str =
    "RoleArn must not be set when PermissionModel is SERVICE_MANAGED: Amazon Web Services \
     Systems Manager provides the role for Default Host Management Configuration.";

/// How a register that would REPLACE a registration without asking to is refused.
///
/// `Overwrite` "specifies whether to override the role that is currently registered for
/// Default Host Management Configuration. The default is false, which acts as a guardrail
/// against accidentally replacing an existing configuration. Set this to true to replace the
/// currently registered role" — so a register made for an account that already has one, with
/// the guardrail still up, writes nothing and says what to set.
///
/// The whole sentence is FIXED text for this condition: it interpolates nothing from the
/// request, so neither the registered role nor the one asked for appears in it.
pub const OVERWRITE_REQUIRED: &str =
    "A role is already registered for Default Host Management Configuration. Set Overwrite to \
     true to replace the currently registered role.";

/// The ARN of the role a `SERVICE_MANAGED` registration registers: "SERVICE_MANAGED The ARN
/// of the role that Amazon Web Services Systems Manager created in the IAM reserved path"
/// (`RegisterDefaultHostManagementConfigurationResult$RoleArn`).
///
/// The IAM reserved path is where a service's own roles live, and the role this service keeps
/// there is the one IAM publishes for its service principal — the SAME role this crate
/// already names for the other work Systems Manager does on an account's behalf
/// ([`super::maintenance_window_tasks::service_linked_role_arn`]), composed by IAM's own
/// builders so the ARN reported is the one IAM holds that role under and IAM's own reads
/// resolve it.
#[must_use]
pub fn service_managed_role_arn() -> String {
    super::maintenance_window_tasks::service_linked_role_arn()
}

/// The role NAME a DHMC registration records for `role_arn`: what the setting holds, because
/// "for `/ssm/managed-instance/default-ec2-instance-management-role`, enter the name of an
/// IAM role".
///
/// The ARN is read through the one reading of an IAM role ARN this tree has
/// (`::service_common::iam_role::role_name`), which answers the name under whatever path the
/// ARN carries; a value that names no role of this account is recorded as it stands, so
/// nothing of the request is dropped silently.
#[must_use]
pub fn registered_role_value(role_arn: &str) -> String {
    ::service_common::iam_role::role_name(role_arn)
        .unwrap_or(role_arn)
        .to_string()
}

/// Whether `setting_value` is a role a register already recorded, which is what `Overwrite`
/// guards: the setting holds the sentinel for an unconfigured name
/// ([`super::service_settings::UNSET_VALUE`]) until a register writes one.
#[must_use]
pub fn is_registered(setting_value: &str) -> bool {
    !setting_value.is_empty() && setting_value != super::service_settings::UNSET_VALUE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: what the setting records is the role's NAME, under whatever path the ARN
    /// carries, and a value that names no role of this account is recorded as it stands.
    #[test]
    fn a_registration_records_the_role_name() {
        let account = ::engine::emulator_account_id();
        // (case, the ARN a register was handed, the value the setting records)
        let rows: &[(&str, String, &str)] = &[
            (
                "a role at the account root",
                format!("arn:aws:iam::{account}:role/DefaultHostManagement"),
                "DefaultHostManagement",
            ),
            (
                "a role under a path",
                format!("arn:aws:iam::{account}:role/service-role/DefaultHostManagement"),
                "DefaultHostManagement",
            ),
            (
                "the role Systems Manager keeps in the reserved path",
                service_managed_role_arn(),
                "AWSServiceRoleForAmazonSSM",
            ),
            (
                "a value that names no role of this account",
                "arn:aws:iam::999999999999:role/Elsewhere".to_string(),
                "arn:aws:iam::999999999999:role/Elsewhere",
            ),
        ];
        for (case, role_arn, expected) in rows {
            let recorded = registered_role_value(role_arn);
            assert_eq!(
                &recorded, expected,
                "case {case:?}: expected the setting to record {expected:?}, got {recorded:?}"
            );
        }
    }

    /// RULE: only a setting holding a role NAME is a registration; the unconfigured sentinel
    /// is not one, so the guardrail has nothing to protect.
    #[test]
    fn only_a_recorded_role_is_a_registration() {
        // (case, the value the setting holds, whether a role is registered)
        let rows: &[(&str, &str, bool)] = &[
            (
                "the sentinel a setting naming nothing holds",
                super::super::service_settings::UNSET_VALUE,
                false,
            ),
            ("no value at all", "", false),
            ("a recorded role name", "DefaultHostManagement", true),
            (
                "the role Systems Manager keeps",
                "AWSServiceRoleForAmazonSSM",
                true,
            ),
        ];
        for (case, held, expected) in rows {
            assert_eq!(
                is_registered(held),
                *expected,
                "case {case:?}: expected is_registered({held:?}) to be {expected}"
            );
        }
    }
}
