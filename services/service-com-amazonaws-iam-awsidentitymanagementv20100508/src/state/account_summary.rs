// SPDX-License-Identifier: CC0-1.0

//! The account-level counters and quotas `GetAccountSummary` reports, and the account's
//! global-endpoint token version, which is one of them.
//!
//! One concern in one place: every entry of the reported `SummaryMap` is either a COUNT
//! aggregated from the account's own collections or a QUOTA this crate already states for
//! the operation that enforces it, so the summary cannot drift from what the rest of the
//! service does — a quota reported here and refused elsewhere is the same constant, and a
//! count reported here is the same collection a list operation pages over.
//!
//! The keys are exactly the values `summaryKeyType` declares
//! (`smithy_query(query="shape", shape="summaryKeyType")`); the map's values are integers
//! (`summaryValueType`), so a yes/no fact about the account is reported as `1` or `0`.

#[allow(unused_imports)]
use super::*;

/// The `summaryKeyType` keys this emulator reports, spelled as the model declares them.
///
/// Written out as constants rather than inline strings so the aggregation below and any
/// reader of it name one spelling: a key spelled differently would be a key no client
/// looks up.
pub mod keys {
    /// The number of IAM users in the account.
    pub const USERS: &str = "Users";
    /// The maximum number of users allowed in the account.
    pub const USERS_QUOTA: &str = "UsersQuota";
    /// The number of IAM groups in the account.
    pub const GROUPS: &str = "Groups";
    /// The maximum number of groups allowed in the account.
    pub const GROUPS_QUOTA: &str = "GroupsQuota";
    /// The number of IAM roles in the account.
    pub const ROLES: &str = "Roles";
    /// The maximum number of roles allowed in the account.
    pub const ROLES_QUOTA: &str = "RolesQuota";
    /// The number of server certificates in the account.
    pub const SERVER_CERTIFICATES: &str = "ServerCertificates";
    /// The maximum number of server certificates allowed in the account.
    pub const SERVER_CERTIFICATES_QUOTA: &str = "ServerCertificatesQuota";
    /// The number of instance profiles in the account.
    pub const INSTANCE_PROFILES: &str = "InstanceProfiles";
    /// The maximum number of instance profiles allowed in the account.
    pub const INSTANCE_PROFILES_QUOTA: &str = "InstanceProfilesQuota";
    /// The number of customer managed policies in the account.
    pub const POLICIES: &str = "Policies";
    /// The maximum number of customer managed policies allowed in the account.
    pub const POLICIES_QUOTA: &str = "PoliciesQuota";
    /// The number of identity providers (SAML and OIDC) in the account.
    pub const PROVIDERS: &str = "Providers";
    /// The number of MFA devices, both assigned and unassigned.
    pub const MFA_DEVICES: &str = "MFADevices";
    /// The number of MFA devices that have been assigned to an IAM user.
    pub const MFA_DEVICES_IN_USE: &str = "MFADevicesInUse";
    /// Whether MFA is enabled for the account's root user.
    pub const ACCOUNT_MFA_ENABLED: &str = "AccountMFAEnabled";
    /// Whether the account's root user has access keys.
    pub const ACCOUNT_ACCESS_KEYS_PRESENT: &str = "AccountAccessKeysPresent";
    /// Whether the account's root user has a password.
    pub const ACCOUNT_PASSWORD_PRESENT: &str = "AccountPasswordPresent";
    /// Whether the account's root user has signing certificates.
    pub const ACCOUNT_SIGNING_CERTIFICATES_PRESENT: &str = "AccountSigningCertificatesPresent";
    /// The maximum size of an inline policy embedded in a user.
    pub const USER_POLICY_SIZE_QUOTA: &str = "UserPolicySizeQuota";
    /// The maximum size of an inline policy embedded in a group.
    pub const GROUP_POLICY_SIZE_QUOTA: &str = "GroupPolicySizeQuota";
    /// The maximum size of an inline policy embedded in a role.
    pub const ROLE_POLICY_SIZE_QUOTA: &str = "RolePolicySizeQuota";
    /// The maximum size of a role's trust policy.
    pub const ASSUME_ROLE_POLICY_SIZE_QUOTA: &str = "AssumeRolePolicySizeQuota";
    /// The maximum size of a managed policy's document.
    pub const POLICY_SIZE_QUOTA: &str = "PolicySizeQuota";
    /// The maximum number of groups one user may belong to.
    pub const GROUPS_PER_USER_QUOTA: &str = "GroupsPerUserQuota";
    /// The maximum number of signing certificates one user may hold.
    pub const SIGNING_CERTIFICATES_PER_USER_QUOTA: &str = "SigningCertificatesPerUserQuota";
    /// The maximum number of access keys one user may hold.
    pub const ACCESS_KEYS_PER_USER_QUOTA: &str = "AccessKeysPerUserQuota";
    /// The maximum number of managed policies attachable to one group.
    pub const ATTACHED_POLICIES_PER_GROUP_QUOTA: &str = "AttachedPoliciesPerGroupQuota";
    /// The maximum number of managed policies attachable to one role.
    pub const ATTACHED_POLICIES_PER_ROLE_QUOTA: &str = "AttachedPoliciesPerRoleQuota";
    /// The maximum number of managed policies attachable to one user.
    pub const ATTACHED_POLICIES_PER_USER_QUOTA: &str = "AttachedPoliciesPerUserQuota";
    /// The number of managed policy versions the account's policies hold.
    pub const POLICY_VERSIONS_IN_USE: &str = "PolicyVersionsInUse";
    /// The maximum number of managed policy versions the account may hold.
    pub const POLICY_VERSIONS_IN_USE_QUOTA: &str = "PolicyVersionsInUseQuota";
    /// The maximum number of versions one managed policy may hold.
    pub const VERSIONS_PER_POLICY_QUOTA: &str = "VersionsPerPolicyQuota";
    /// The version of the global endpoint token the account is set to.
    pub const GLOBAL_ENDPOINT_TOKEN_VERSION: &str = "GlobalEndpointTokenVersion";
}

/// The maximum size, in characters, of a role's trust policy — the quota the summary
/// reports as `AssumeRolePolicySizeQuota`.
///
/// AWS's "IAM and STS quotas" page gives "Role trust policy length: 2,048 characters", the
/// same measure the inline-policy quotas beside it use
/// ([`MAX_USER_INLINE_POLICY_CHARACTERS`]). Nothing in this crate refuses a trust policy on
/// its length yet, so this constant is reported and not yet enforced — which is why it is
/// stated here, beside the summary that reports it, rather than claimed by a handler.
pub const MAX_ASSUME_ROLE_POLICY_CHARACTERS: usize = 2_048;

/// The maximum number of managed policy versions one account may hold in use — the quota
/// the summary reports as `PolicyVersionsInUseQuota`.
///
/// AWS's "IAM and STS quotas" page gives "Policy versions in use: 10,000".
pub const MAX_POLICY_VERSIONS_IN_USE: usize = 10_000;

/// The two values `globalEndpointTokenVersion` declares
/// (`smithy_query(query="shape", shape="globalEndpointTokenVersion")`), each with the
/// number `GetAccountSummary` reports it as — the summary's values are integers, so the
/// version is reported as `1` or `2` while `SetSecurityTokenServicePreferences` names it
/// as the token version `v1Token`/`v2Token`.
pub const GLOBAL_ENDPOINT_TOKEN_VERSIONS: &[(&str, i32)] = &[("v1Token", 1), ("v2Token", 2)];

/// The token version an account is on before anything sets one: version 1.
///
/// "Version 1 tokens are valid only in Amazon Web Services Regions that are available by
/// default", which is the behaviour of the global endpoint an account has never changed the
/// preference of — `SetSecurityTokenServicePreferences` is the operation that moves it, so
/// an account that has never called it reports the version its tokens are already issued
/// under.
pub const DEFAULT_GLOBAL_ENDPOINT_TOKEN_VERSION: i32 = 1;

/// The number `GetAccountSummary` reports for the token version `version` names, or `None`
/// for a value the enum does not declare.
pub fn global_endpoint_token_version(version: &str) -> Option<i32> {
    GLOBAL_ENDPOINT_TOKEN_VERSIONS
        .iter()
        .find(|(declared, _)| *declared == version)
        .map(|(_, reported)| *reported)
}

/// How a yes/no fact about the account is reported in a map whose values are integers.
fn reported_flag(holds: bool) -> i32 {
    i32::from(holds)
}

/// A count or a quota as `summaryValueType` carries it. A collection longer than an `i32`
/// is reported at that ceiling rather than wrapping, which no account this emulator serves
/// can reach (every collection is bounded by a quota far below it).
fn reported_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// "A set of key–value pairs containing information about IAM entity usage and IAM
    /// quotas" (`GetAccountSummaryResponse$SummaryMap`): every count read off the account's
    /// own collections, and every quota the constant the operation enforcing it uses.
    ///
    /// A READ: nothing here writes, so reporting the summary changes nothing a later caller
    /// sees.
    pub fn account_summary(&self) -> BTreeMap<String, i32> {
        // The MFA devices the account holds are the virtual devices it created and the FIDO
        // security keys registered to it; a device is "in use" once a user holds it, which
        // for a virtual device is the user the enable bound it to.
        let mfa_devices = self.virtual_mfa_devices.len() + self.fido_security_keys.len();
        let mfa_devices_in_use = self
            .virtual_mfa_devices
            .values()
            .filter(|device| device.user_key.is_some())
            .count()
            + self.fido_security_keys.len();
        // The versions the account's own managed policies hold: an AWS-managed policy is
        // not the account's, so its versions are not in the account's use.
        let policy_versions: usize = self
            .policies
            .values()
            .map(|policy| policy.versions.len())
            .sum();
        [
            (keys::USERS, reported_count(self.users.len())),
            (keys::USERS_QUOTA, reported_count(MAX_USERS_PER_ACCOUNT)),
            (keys::GROUPS, reported_count(self.groups.len())),
            (keys::GROUPS_QUOTA, reported_count(MAX_GROUPS_PER_ACCOUNT)),
            (keys::ROLES, reported_count(self.roles.len())),
            (keys::ROLES_QUOTA, reported_count(MAX_ROLES_PER_ACCOUNT)),
            (
                keys::SERVER_CERTIFICATES,
                reported_count(self.server_certificates.len()),
            ),
            (
                keys::SERVER_CERTIFICATES_QUOTA,
                reported_count(MAX_SERVER_CERTIFICATES_PER_ACCOUNT),
            ),
            (
                keys::INSTANCE_PROFILES,
                reported_count(self.instance_profiles.len()),
            ),
            (
                keys::INSTANCE_PROFILES_QUOTA,
                reported_count(MAX_INSTANCE_PROFILES_PER_ACCOUNT),
            ),
            (keys::POLICIES, reported_count(self.policies.len())),
            (
                keys::POLICIES_QUOTA,
                reported_count(MAX_POLICIES_PER_ACCOUNT),
            ),
            (
                keys::PROVIDERS,
                reported_count(self.saml_providers.len() + self.open_id_connect_providers.len()),
            ),
            (keys::MFA_DEVICES, reported_count(mfa_devices)),
            (keys::MFA_DEVICES_IN_USE, reported_count(mfa_devices_in_use)),
            // The account's ROOT user: this emulator holds no root-user credentials — no
            // operation of this API creates any, and the account's own long-term key
            // belongs to an IAM user — so the three facts about them are reported as the
            // state the account is in, which is "none".
            (keys::ACCOUNT_MFA_ENABLED, reported_flag(false)),
            (keys::ACCOUNT_ACCESS_KEYS_PRESENT, reported_flag(false)),
            (keys::ACCOUNT_PASSWORD_PRESENT, reported_flag(false)),
            (
                keys::ACCOUNT_SIGNING_CERTIFICATES_PRESENT,
                reported_flag(false),
            ),
            (
                keys::USER_POLICY_SIZE_QUOTA,
                reported_count(MAX_USER_INLINE_POLICY_CHARACTERS),
            ),
            (
                keys::GROUP_POLICY_SIZE_QUOTA,
                reported_count(MAX_GROUP_INLINE_POLICY_CHARACTERS),
            ),
            (
                keys::ROLE_POLICY_SIZE_QUOTA,
                reported_count(MAX_ROLE_INLINE_POLICY_CHARACTERS),
            ),
            (
                keys::ASSUME_ROLE_POLICY_SIZE_QUOTA,
                reported_count(MAX_ASSUME_ROLE_POLICY_CHARACTERS),
            ),
            (
                keys::POLICY_SIZE_QUOTA,
                reported_count(MAX_MANAGED_POLICY_CHARACTERS),
            ),
            (
                keys::GROUPS_PER_USER_QUOTA,
                reported_count(MAX_GROUPS_PER_USER),
            ),
            (
                keys::SIGNING_CERTIFICATES_PER_USER_QUOTA,
                reported_count(MAX_SIGNING_CERTIFICATES_PER_USER),
            ),
            (
                keys::ACCESS_KEYS_PER_USER_QUOTA,
                reported_count(MAX_ACCESS_KEYS_PER_USER),
            ),
            (
                keys::ATTACHED_POLICIES_PER_GROUP_QUOTA,
                reported_count(MAX_MANAGED_POLICIES_PER_GROUP),
            ),
            (
                keys::ATTACHED_POLICIES_PER_ROLE_QUOTA,
                reported_count(MAX_MANAGED_POLICIES_PER_ROLE),
            ),
            (
                keys::ATTACHED_POLICIES_PER_USER_QUOTA,
                reported_count(MAX_MANAGED_POLICIES_PER_USER),
            ),
            (keys::POLICY_VERSIONS_IN_USE, reported_count(policy_versions)),
            (
                keys::POLICY_VERSIONS_IN_USE_QUOTA,
                reported_count(MAX_POLICY_VERSIONS_IN_USE),
            ),
            (
                keys::VERSIONS_PER_POLICY_QUOTA,
                reported_count(MAX_POLICY_VERSIONS),
            ),
            (
                keys::GLOBAL_ENDPOINT_TOKEN_VERSION,
                self.global_endpoint_token_version
                    .unwrap_or(DEFAULT_GLOBAL_ENDPOINT_TOKEN_VERSION),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
    }
}
