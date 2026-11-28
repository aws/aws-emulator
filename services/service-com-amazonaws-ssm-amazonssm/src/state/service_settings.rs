// SPDX-License-Identifier: CC0-1.0

//! The account's SERVICE SETTINGS: the account-level settings the Systems Manager
//! service teams provision, which `GetServiceSetting` reports, `UpdateServiceSetting`
//! overwrites and `ResetServiceSetting` restores to the provisioned value.
//!
//! "Services map a SettingId object to a setting value. Amazon Web Services services
//! teams define the default value for a SettingId. You can't create a new SettingId, but
//! you can overwrite the default value" — so the set of settings an account holds is a
//! CATALOGUE ([`SETTINGS`]) rather than something a caller creates, and a `SettingId`
//! outside it is reported as a setting no service team has provisioned. Each entry
//! carries the value the service team provisioned and, where the API documents them, the
//! values that setting accepts ([`SettingDefinition`]).
//!
//! Only a setting an account has MODIFIED is stored ([`ServiceSettingRecord`]): a setting
//! nobody has touched is reported from the catalogue, at the provisioned value and the
//! `Default` status, and the modification that an update or a reset makes is what puts a
//! record in the store. A reset is therefore recorded too — rather than the erasure of a
//! record — so no reader sees a setting's `LastModifiedDate` move backwards, but what it
//! records is the setting's restored state and not a customer overwrite
//! ([`restoration`]): `LastModifiedUser` "is populated only if the setting value was
//! overwritten", so a setting back at its provisioned value names the service team that
//! provisioned it ([`PROVISIONING_USER`]) as its last modifier.
//!
//! A setting is reached through one Region's endpoint and its ARN names that Region
//! ([`ServiceSettingRecord::to_service_setting`]), so the store is keyed by
//! `(region, setting id)` the way the rest of this service's state is.
//!
//! What a setting's value makes other operations DO is each of those operations' own
//! behaviour: this slice records the account's choice and reports it, and nothing here
//! changes what a parameter write or a node activation does.

use std::time::SystemTime;

use crate::ops::types::ServiceSetting;

/// The resource portion a service-setting ARN carries before the setting's id, e.g.
/// `arn:aws:ssm:us-east-1:111122223333:servicesetting/ssm/parameter-store/high-throughput-enabled`
/// (`UpdateServiceSettingRequest$SettingId` quotes that ARN). A setting id begins with
/// its own separator, so the prefix carries none.
pub const ARN_SETTING_RESOURCE_PREFIX: &str = "servicesetting";

/// `ServiceSetting$Status` for a setting at the value its service team provisioned:
/// "Default: The current setting uses a default value provisioned by the Amazon Web
/// Services service team."
pub const STATUS_DEFAULT: &str = "Default";

/// `ServiceSetting$Status` for a setting an account has overwritten: "Customized: The
/// current setting use a custom value specified by the customer."
pub const STATUS_CUSTOMIZED: &str = "Customized";

/// `ServiceSetting$Status` for a setting whose change has been asked for but is not
/// effective yet: "PendingUpdate: The current setting uses a default or custom value, but
/// a setting change request is pending approval."
pub const STATUS_PENDING_UPDATE: &str = "PendingUpdate";

/// Who last modified a setting nobody has modified: the service team that provisioned
/// it, which is what `LastModifiedUser` names for a setting still at its default.
pub const PROVISIONING_USER: &str = "System";

/// The value a setting whose value is a NAME reports while the account has named
/// nothing: the sentinel Systems Manager reports for an unconfigured setting of that
/// kind (the Default Host Management Configuration role is reported this way until a
/// role is named).
pub const UNSET_VALUE: &str = "$None";

/// One setting a Systems Manager service team provisions: the `SettingId` it is reached
/// by, the value the team provisioned, and the values the API documents it accepting.
#[derive(Debug, Clone, Copy)]
pub struct SettingDefinition {
    /// The `SettingId`, as `GetServiceSetting` reports it — the documented path form,
    /// which is also what the setting's ARN carries after its resource prefix.
    pub id: &'static str,
    /// The value the service team provisioned, which is what a setting at the `Default`
    /// status reports and what a reset restores.
    pub default_value: &'static str,
    /// The values `UpdateServiceSettingRequest$SettingValue` documents for this setting,
    /// or EMPTY for a setting whose value is free text (the name of a log group, the name
    /// of an IAM role) and whose catalogue of values the API therefore does not state.
    pub values: &'static [&'static str],
    /// Whether moving this setting OFF the account's own value needs the service team's
    /// approval before it takes effect, so a reset of it registers a change request
    /// instead of restoring the provisioned value at once ([`restoration`]) — the setting
    /// keeps the value the account set and reports [`STATUS_PENDING_UPDATE`], "a setting
    /// change request is pending approval".
    ///
    /// Which settings are reached that way is not in the model and not in the API
    /// documentation: it is what Systems Manager is observed to answer for them.
    pub reset_awaits_approval: bool,
}

/// The settings Systems Manager provisions, in the order
/// `UpdateServiceSettingRequest$SettingId` lists them — the one catalogue
/// `GetServiceSetting`, `UpdateServiceSetting` and `ResetServiceSetting` resolve a
/// `SettingId` through
/// (`smithy_query(query="doc",shape="com.amazonaws.ssm#UpdateServiceSettingRequest",member="SettingId")`).
///
/// Each `values` entry is the value list that operation's `SettingValue` documents for
/// that setting
/// (`smithy_query(query="doc",shape="com.amazonaws.ssm#UpdateServiceSettingRequest",member="SettingValue")`);
/// a setting the doc states no values for takes free text.
///
/// Each `default_value` is the value the service team provisions, which for a setting that
/// governs what an account may do is the one leaving that as the team set it up, and for a
/// feature an account opts into is the one that leaves it off — "if an Amazon Web Services service
/// charges money to the account based on feature or service usage, then the Amazon Web
/// Services service team might create a default setting of "false". This means the user
/// can't use this feature unless they change the setting to "true" and intentionally opt
/// in" (`ResetServiceSetting`'s own documentation) — and for a setting that names a
/// resource is [`UNSET_VALUE`], because the account has named none.
pub const SETTINGS: &[SettingDefinition] = &[
    SettingDefinition {
        id: "/ssm/appmanager/appmanager-enabled",
        default_value: "False",
        values: &["True", "False"],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/automation/customer-script-log-destination",
        default_value: UNSET_VALUE,
        values: &["CloudWatch"],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/automation/customer-script-log-group-name",
        default_value: UNSET_VALUE,
        values: &[],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/automation/enable-adaptive-concurrency",
        default_value: "false",
        values: &[],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        // The console's public-sharing permission is provisioned ALLOWING the sharing it
        // governs: the setting is what an account turns sharing off with, so the value a
        // reset restores is the permissive one. Neither the model nor the API
        // documentation states a default for it; this is what Systems Manager is observed
        // to report for an account that has not changed it.
        id: "/ssm/documents/console/public-sharing-permission",
        default_value: "Enable",
        values: &["Enable", "Disable"],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/managed-instance/activation-tier",
        default_value: "standard",
        values: &["standard", "advanced"],
        reset_awaits_approval: true,
    },
    SettingDefinition {
        id: super::default_host_management::ROLE_SETTING_ID,
        default_value: UNSET_VALUE,
        values: &[],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/opsinsights/opscenter",
        default_value: "Disabled",
        values: &["Enabled", "Disabled"],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/parameter-store/default-parameter-tier",
        default_value: super::TIER_STANDARD,
        values: &["Standard", "Advanced", "Intelligent-Tiering"],
        reset_awaits_approval: false,
    },
    SettingDefinition {
        id: "/ssm/parameter-store/high-throughput-enabled",
        default_value: "false",
        values: &["true", "false"],
        reset_awaits_approval: false,
    },
];

/// The setting `id` names, or `None` when no service team provisions a setting of that
/// id — which is what `ServiceSettingNotFound` reports.
#[must_use]
pub fn definition(id: &str) -> Option<&'static SettingDefinition> {
    SETTINGS.iter().find(|setting| setting.id == id)
}

/// The setting a request's `SettingId` names, by either of its documented spellings, or
/// `None` when no service team provisions it, which is what `ServiceSettingNotFound`
/// reports ([`NOT_FOUND_MESSAGE`]).
#[must_use]
pub fn resolve(setting_id: &str) -> Option<&'static SettingDefinition> {
    definition(parse_setting_id(setting_id)?)
}

/// What `ServiceSettingNotFound` reports for a `SettingId` no service team provisions:
/// NOTHING. The refusal carries no message at all — the error code alone states that the
/// setting is not one Systems Manager holds — so the three operations that resolve a
/// `SettingId` (`GetServiceSetting`, `UpdateServiceSetting`, `ResetServiceSetting`) leave
/// the member unset rather than shipping the error shape's documentation as text the
/// service never sends.
pub const NOT_FOUND_MESSAGE: Option<String> = None;

/// What `InternalServerError` reports when this service's store cannot be reached, so a
/// request that can read nothing is answered as the server-side failure it is rather than
/// with an invented setting.
pub const STORE_UNAVAILABLE: &str = "the service setting store is unavailable";

/// The principal an OVERWRITE records as its `LastModifiedUser`: the IAM user this
/// emulator's published long-term credential speaks for, which is the same principal STS
/// reports as the caller of an operation
/// (`::service_common::caller_principal::ambient_user_arn`) — so a client that reads its
/// own identity recognises the modifier of a setting it changed.
#[must_use]
pub fn modifying_user() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// How many Regions' service settings this store holds.
///
/// Emulator resource guard, NOT a published AWS quota: a setting is stored under the
/// Region of the request's own credential scope ([`super::request_region`]), which the
/// caller chooses, so without a total the fixed catalogue would be one catalogue per
/// caller-chosen partition of the key space and the store could grow without bound. More
/// Regions than AWS itself has, so no account reaching a Region that exists can trip it.
pub const MAX_SERVICE_SETTING_REGIONS: usize = 64;

/// How many settings this store holds in TOTAL: the catalogue, once per Region
/// [`MAX_SERVICE_SETTING_REGIONS`] allows.
pub const MAX_SERVICE_SETTINGS_TOTAL: usize = MAX_SERVICE_SETTING_REGIONS * SETTINGS.len();

/// What a modification is refused with when it would record one setting past
/// [`MAX_SERVICE_SETTINGS_TOTAL`] — the emulator's own guard, named as the emulator's
/// rather than as an AWS quota, and reported the way this crate's other total guards are.
#[must_use]
pub fn store_full_message() -> String {
    format!(
        "This emulator records at most {MAX_SERVICE_SETTINGS_TOTAL} service settings across every \
         Region"
    )
}

/// The setting id `value` names, or `None` when it is an ARN that names no `ssm` service
/// setting.
///
/// A `SettingId` is "the Amazon Resource Name (ARN) of the service setting", documented
/// with both spellings — the ARN
/// `arn:aws:ssm:us-east-1:111122223333:servicesetting/ssm/parameter-store/high-throughput-enabled`
/// and the bare id `/ssm/parameter-store/high-throughput-enabled` — so either resolves to
/// the one setting, through the one parse this service reads a name-or-ARN member with
/// ([`super::parse_resource_query`]).
///
/// Only the setting id is taken from the value: WHICH account's setting a call reads or
/// modifies is decided by the endpoint the request was signed for ([`super::request_region`])
/// and by the account this emulator serves, exactly as it is for a call that names the bare
/// id, so the Region and account segments of an ARN spelling select nothing.
#[must_use]
pub fn parse_setting_id(value: &str) -> Option<&str> {
    Some(super::parse_resource_query(value, ARN_SETTING_RESOURCE_PREFIX)?.name)
}

/// The ARN of the setting `id` names at `region`'s endpoint.
#[must_use]
pub fn setting_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        region,
        &format!("{ARN_SETTING_RESOURCE_PREFIX}{id}"),
    )
}

/// Whether `value` is one of the values `setting` accepts, or the reason it is not.
///
/// A setting the API documents a value list for accepts those values and nothing else.
/// The comparison ignores letter case, because a value list is a set of values and not a
/// set of spellings — the documented lists themselves spell one setting's booleans
/// `True`/`False` and another's `true`/`false`. What the account asked for is what gets
/// stored, so a read reports the value as its writer spelled it.
pub fn value_rejection(setting: &SettingDefinition, value: &str) -> Option<String> {
    if setting.values.is_empty()
        || setting
            .values
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(value))
    {
        return None;
    }
    Some(format!(
        "SettingValue '{value}' isn't one of the values the setting {} accepts: {}",
        setting.id,
        setting.values.join(", ")
    ))
}

/// A setting an account has MODIFIED: the value it holds now and the modification that
/// put it there.
///
/// A setting nobody has modified has no record — it is reported from the catalogue
/// instead ([`super::ComAmazonawsSsmAmazonssmState::service_setting`]) — so a record always
/// carries a real modification: the instant it was made and the principal that made it.
#[derive(Debug, Clone)]
pub struct ServiceSettingRecord {
    /// The value the setting holds now: what an update stored, or the provisioned value a
    /// reset restored.
    pub value: String,
    /// `Default` once a reset has restored the provisioned value, `Customized` while the
    /// account's own value stands, `PendingUpdate` while a change of it awaits approval.
    pub status: String,
    /// When the modification this record holds was made.
    pub last_modified_date: SystemTime,
    /// The principal that made it, as `LastModifiedUser` names them: the account's own
    /// principal while its value stands, and the provisioning service team once the
    /// setting is back at the value that team provisioned.
    pub last_modified_user: String,
}

/// What a RESET of `setting` leaves recorded, for the reading `current` the setting holds
/// now and the instant `at` the reset was asked for — or `None` when the setting is
/// already at the value its service team provisioned, so the reset restores nothing and
/// records nothing.
///
/// Restoring the provisioned value takes the setting back OUT of the account's ownership:
/// `ServiceSetting$LastModifiedUser` is "populated only if the setting value was
/// overwritten", so the restored setting names the team that provisioned it
/// ([`PROVISIONING_USER`]) rather than whoever asked for the restore, at the `Default`
/// status.
///
/// A setting whose move off the account's value needs that team's approval
/// ([`SettingDefinition::reset_awaits_approval`]) is not restored by the call: the value
/// the account set still stands and the setting reports [`STATUS_PENDING_UPDATE`] until
/// the change is approved — which no API operation does — so the modification it reports
/// stays the overwrite that is still in force, `at` included: the setting's value was not
/// modified, only a change of it asked for, and an account that wants the provisioned
/// value back without waiting writes it ([`super::ComAmazonawsSsmAmazonssmState::record_service_setting`]).
///
/// Whether the setting already holds the provisioned value ignores letter case, for the
/// same reason [`value_rejection`] does: a value list is a set of values, not of
/// spellings.
#[must_use]
pub fn restoration(
    setting: &SettingDefinition,
    current: &ServiceSettingRecord,
    at: SystemTime,
) -> Option<ServiceSettingRecord> {
    let holds_provisioned = current.value.eq_ignore_ascii_case(setting.default_value);
    if holds_provisioned && current.status == STATUS_DEFAULT {
        return None;
    }
    if setting.reset_awaits_approval && !holds_provisioned {
        return Some(ServiceSettingRecord {
            status: STATUS_PENDING_UPDATE.to_string(),
            ..current.clone()
        });
    }
    Some(ServiceSettingRecord {
        value: setting.default_value.to_string(),
        status: STATUS_DEFAULT.to_string(),
        last_modified_date: at,
        last_modified_user: PROVISIONING_USER.to_string(),
    })
}

impl ServiceSettingRecord {
    /// The modelled `ServiceSetting` for the setting `id` names at `region`'s endpoint:
    /// the stored value, the stored modification, and the ARN that id has there.
    #[must_use]
    pub fn to_service_setting(&self, region: &str, id: &str) -> ServiceSetting {
        ServiceSetting {
            setting_id: Some(id.to_string()),
            setting_value: Some(self.value.clone()),
            last_modified_date: Some(crate::state::wire_instant(self.last_modified_date)),
            last_modified_user: Some(self.last_modified_user.clone()),
            arn: Some(setting_arn(region, id)),
            status: Some(self.status.clone()),
        }
    }
}
