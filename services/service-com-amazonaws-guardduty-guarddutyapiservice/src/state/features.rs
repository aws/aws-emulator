// SPDX-License-Identifier: CC0-1.0

//! Detector features and the data sources behind them: the defaults a detector
//! comes up with, the refusals a feature configuration is checked against, and
//! the projection every feature and data-source read reports.

use super::*;

/// Refuse a feature list that asks for Runtime Monitoring and EKS Runtime
/// Monitoring at once, with the message the refusal carries.
///
/// "You can add only one of these two features because Runtime Monitoring already
/// includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`, and the
/// same sentence on `CreateDetector`), so the rule holds for a DETECTOR's feature
/// list and for an ORGANIZATION's alike. The two carry different status
/// vocabularies, so each caller supplies `asked_for` — whether its list asks for
/// the feature it names — and the rule and its message live here once.
pub fn refuse_runtime_monitoring_overlap(asked_for: impl Fn(&str) -> bool) -> Result<(), String> {
    if asked_for(FEATURE_EKS_RUNTIME_MONITORING) && asked_for(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
        ));
    }
    Ok(())
}

/// The feature set `CreateDetector` gives a detector when the request names no
/// features.
///
/// Every configurable feature is enabled except the [`OPT_IN_FEATURES`]: "When you
/// don't specify any features, with an exception to RUNTIME_MONITORING, all the
/// optional features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). The three
/// [`FOUNDATIONAL_FEATURES`] are not optional and are always enabled.
///
/// The records are created in [`reported_features`] order, which is the order a
/// read reports them in.
pub fn default_features(now: i64) -> Vec<FeatureRecord> {
    reported_features()
        .into_iter()
        .map(|name| {
            let status = if OPT_IN_FEATURES.contains(&name) {
                DISABLED
            } else {
                ENABLED
            };
            FeatureRecord {
                name: name.to_string(),
                status: status.to_string(),
                updated_at_millis: now,
                additional_configuration: Vec::new(),
                // A feature this account starts with ENABLED starts its free trial
                // at the same instant; one it starts with DISABLED has not started
                // its trial yet.
                free_trial_started_at_millis: (status == ENABLED).then_some(now),
            }
        })
        .collect()
}

/// The feature set a create gives a detector, resolved against the `Features` list
/// the create itself carries.
///
/// [`default_features`] is what a detector created without a `Features` list gets.
/// A create that ENABLES one of the two mutually exclusive Runtime Monitoring
/// features and leaves the other one to the default is accepted — only asking for
/// both is "an error" — so the default the SERVICE picks for the other one yields to
/// what the request asked for and starts [`DISABLED`], and its free trial has
/// therefore not begun either. That way the set a create stores never carries the
/// combination the service "can add only one of"
/// (`smithy_query(query="doc",shape="DetectorFeatureConfiguration")`).
pub fn default_features_for(
    now: i64,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Vec<FeatureRecord> {
    let mut records = default_features(now);
    // The LAST entry naming a feature is the one that lands, as applying the list
    // writes the entries in order, and an entry that states no status enables the
    // feature ([`apply_feature_configurations`]).
    let requested = |wanted: &str| -> Option<bool> {
        Some(
            features?
                .iter()
                .rfind(|feature| feature.name.as_deref() == Some(wanted))?
                .status
                .as_deref()
                != Some(DISABLED),
        )
    };
    for (asked_for, other) in [
        (FEATURE_RUNTIME_MONITORING, FEATURE_EKS_RUNTIME_MONITORING),
        (FEATURE_EKS_RUNTIME_MONITORING, FEATURE_RUNTIME_MONITORING),
    ] {
        // Only where the request says nothing about the other one: a request that
        // names it states its status itself, and a request that enables both is
        // refused before anything is stored.
        if requested(asked_for) != Some(true) || requested(other).is_some() {
            continue;
        }
        if let Some(record) = records.iter_mut().find(|record| record.name == other) {
            record.status = DISABLED.to_string();
            record.free_trial_started_at_millis = None;
        }
    }
    records
}

/// Refuse a `Features` list whose own entries this service cannot accept, with the
/// message the refusal carries.
///
/// Two rules, both from `CreateDetector`'s own documentation: a feature name must be
/// a `DetectorFeature` member, and — since each entry configures one member of that
/// enum — a list may carry no more entries than the enum has members
/// ([`validate_feature_list_length`]). Both are judged on the REQUEST alone, so they
/// are answered whatever the store holds; the rule that spans the list and the
/// configuration it is applied over is [`validate_feature_exclusion`].
pub fn validate_feature_configurations(
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_feature_list_length(&DETECTOR_FEATURES, features.len(), FEATURES_MEMBER)?;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        validate_feature_entry(&DETECTOR_FEATURES, name, feature.status.as_deref())?;
        if let Some(additional) = feature.additional_configuration.as_ref() {
            validate_feature_list_length(
                DETECTOR_ADDITIONAL_CONFIGURATIONS,
                additional.len(),
                ADDITIONAL_CONFIGURATION_MEMBER,
            )?;
        }
        for additional in feature.additional_configuration.iter().flatten() {
            let Some(additional_name) = additional.name.as_deref() else {
                continue;
            };
            validate_additional_configuration_entry(
                DETECTOR_ADDITIONAL_CONFIGURATIONS,
                additional_name,
                additional.status.as_deref(),
            )?;
        }
    }
    Ok(())
}

/// Refuse a member-account `Features` list whose own entries this service cannot
/// accept, with the message the refusal carries.
///
/// The member side of [`validate_feature_configurations`], and the same two rules:
/// `MemberFeaturesConfiguration.Name` targets `OrgFeature`
/// (`smithy_query(shape="MemberFeaturesConfiguration")`), so the accepted catalog
/// is [`MEMBER_FEATURES`] rather than [`DETECTOR_FEATURES`]. An
/// `AdditionalConfiguration` entry is checked the same way against the
/// `OrgFeatureAdditionalConfiguration` members it may name.
pub fn validate_member_feature_configurations(
    features: Option<&Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_feature_list_length(MEMBER_FEATURES, features.len(), FEATURES_MEMBER)?;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        validate_feature_entry(MEMBER_FEATURES, name, feature.status.as_deref())?;
        if let Some(additional) = feature.additional_configuration.as_ref() {
            validate_feature_list_length(
                MEMBER_ADDITIONAL_CONFIGURATIONS,
                additional.len(),
                ADDITIONAL_CONFIGURATION_MEMBER,
            )?;
        }
        for additional in feature.additional_configuration.iter().flatten() {
            let Some(additional_name) = additional.name.as_deref() else {
                continue;
            };
            validate_additional_configuration_entry(
                MEMBER_ADDITIONAL_CONFIGURATIONS,
                additional_name,
                additional.status.as_deref(),
            )?;
        }
    }
    Ok(())
}

/// Refuse a `Features` list that would leave the detector it is applied to with
/// both Runtime Monitoring features enabled, with the message the refusal carries.
///
/// `stored` is the configuration the list is applied over — the detector's own
/// switches — and is EMPTY where no configuration exists yet, a create resolving
/// its default set against the request afterwards ([`default_features_for`]).
pub fn validate_feature_exclusion(
    stored: &[FeatureRecord],
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_runtime_monitoring_pair(
        stored,
        features
            .iter()
            .map(|f| (f.name.as_deref(), f.status.as_deref())),
    )
}

/// Refuse a member-account `Features` list that would leave the member account's
/// configuration with both Runtime Monitoring features enabled: the member side of
/// [`validate_feature_exclusion`], which `UpdateMemberDetectors` documents the same
/// exclusion for.
pub fn validate_member_feature_exclusion(
    stored: &[FeatureRecord],
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    validate_runtime_monitoring_pair(
        stored,
        features
            .iter()
            .map(|f| (f.name.as_deref(), f.status.as_deref())),
    )
}

/// Refuse a feature list longer than the enum `catalog` its entries name: each
/// entry configures one member of that enum, so a list with more entries than the
/// enum has members necessarily names one twice, and the request is refused rather
/// than stored. `member` is the wire name of the list the refusal is about.
///
/// This is also what keeps a request from growing the store (or the work one call
/// does) by the length of a list the model declares no `@length` for: the stored
/// configuration is keyed by name, so it can never be longer than the catalog, and
/// neither can the list a request is allowed to send.
pub(super) fn validate_feature_list_length(
    catalog: &[&str],
    length: usize,
    member: &str,
) -> Result<(), String> {
    if length > catalog.len() {
        return Err(format!(
            "{member}: at most {} entries may be given, one for each supported value, but {length} were given.",
            catalog.len()
        ));
    }
    Ok(())
}

/// The wire name of the per-feature additional-configuration list both the
/// detector and the member feature shapes carry
/// (`smithy_query(shape="MemberFeaturesConfiguration")`).
pub(super) const ADDITIONAL_CONFIGURATION_MEMBER: &str = "features.additionalConfiguration";

/// The wire name of the feature list itself.
pub(super) const FEATURES_MEMBER: &str = "features";

/// Refuse one `(name, status)` entry of a feature list: the name must be a member
/// of the enum `catalog` the request's member targets, and the status, where
/// supplied, a `FeatureStatus` member.
pub(super) fn validate_feature_entry(
    catalog: &[&str],
    name: &str,
    status: Option<&str>,
) -> Result<(), String> {
    if !catalog.contains(&name) {
        return Err(format!(
            "Features: '{name}' is not a supported detector feature. Supported features are {}.",
            catalog.join(", ")
        ));
    }
    if let Some(status) = status {
        if status != ENABLED && status != DISABLED {
            return Err(format!(
                "Features: '{status}' is not a supported status for feature '{name}'. Supported values are {ENABLED}, {DISABLED}."
            ));
        }
    }
    Ok(())
}

/// Refuse one `(name, status)` entry of a feature's `AdditionalConfiguration` list:
/// the name must be a member of the enum `catalog` the entry's `Name` targets, and
/// the status, where supplied, a `FeatureStatus` member
/// ([`validate_feature_entry`]'s rule for the status, which the two lists share).
///
/// A name outside the catalog names a configuration NO feature supports, which is
/// what GuardDuty's own sentence for that refusal states
/// ([`ADDITIONAL_CONFIGURATION_UNSUPPORTED_MESSAGE`]) — a different class from a
/// feature name outside `DetectorFeature`, which names a feature instead. The
/// request boundary refuses such a value first, since every one of these names is
/// a modelled enum; this is the same rule stated where a handler can reach it, so
/// the two cannot answer one request differently.
pub(super) fn validate_additional_configuration_entry(
    catalog: &[&str],
    name: &str,
    status: Option<&str>,
) -> Result<(), String> {
    if !catalog.contains(&name) {
        return Err(ADDITIONAL_CONFIGURATION_UNSUPPORTED_MESSAGE.to_string());
    }
    // The name is in the catalog, so only the status rule is left to state.
    validate_feature_entry(catalog, name, status)
}

/// Refuse a feature list that would ADD EKS Runtime Monitoring to a configuration
/// that has Runtime Monitoring, or the other way round: "Specifying both EKS Runtime
/// Monitoring (EKS_RUNTIME_MONITORING) and Runtime Monitoring (RUNTIME_MONITORING)
/// will cause an error. You can add only one of these two features because Runtime
/// Monitoring already includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="UpdateMemberDetectors")`, which
/// `CreateDetector` and `UpdateDetector` document identically).
///
/// A `Features` list is a PARTIAL update — a feature it does not name keeps the
/// status it already had ([`apply_feature_configurations`]) — so the exclusion is
/// judged on the status each of the two ENDS UP with once the list has been applied,
/// not on what the request happens to mention: the list's own value where it names
/// the feature (the LAST entry naming it, which is the one the applier leaves
/// behind, and an entry that states no status enables it), and the status `stored`
/// already holds otherwise. This is the reading
/// [`validate_org_feature_configurations`] gives an organization's list, over the
/// same primitive.
///
/// It is the list's own ADDITION that is refused: a list that enables neither of the
/// two configures neither, so it is accepted whatever the stored configuration
/// holds.
pub(super) fn validate_runtime_monitoring_pair<'a>(
    stored: &[FeatureRecord],
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)> + Clone,
) -> Result<(), String> {
    let requested = |wanted: &str| -> Option<bool> {
        entries
            .clone()
            .filter(|(name, _)| *name == Some(wanted))
            .map(|(_, status)| status != Some(DISABLED))
            .last()
    };
    if requested(FEATURE_EKS_RUNTIME_MONITORING) != Some(true)
        && requested(FEATURE_RUNTIME_MONITORING) != Some(true)
    {
        return Ok(());
    }
    refuse_runtime_monitoring_overlap(|wanted| {
        requested(wanted).unwrap_or_else(|| feature_status_in(stored, wanted) == ENABLED)
    })
}

/// Refuse a `DestinationType` this service does not support, returning the AWS
/// service a destination of that type is an ARN of.
///
/// The accepted set is the `DestinationType` enum itself
/// (`smithy_query(shape="DestinationType")`), checked here rather than at the
/// boundary because this service validates its enum-valued members in the
/// handler, the way `Features` is checked above.
pub fn validate_destination_type(destination_type: &str) -> Result<&'static str, String> {
    DESTINATION_SERVICES
        .iter()
        .find(|(name, _)| *name == destination_type)
        .map(|(_, service)| *service)
        .ok_or_else(|| {
            format!(
                "{DESTINATION_TYPE_MEMBER}: '{destination_type}' is not a supported destination type. Supported types are {}.",
                DESTINATION_TYPES.join(", ")
            )
        })
}

/// The message GuardDuty refuses a `kmsKeyArn` it cannot use with: AWS's own
/// published wording for that parameter, naming the member in the words AWS uses
/// for it rather than the request's dotted wire path, and interpolating nothing.
///
/// It answers EVERY value the service cannot encrypt published findings with — one
/// the create left out, an empty one, and one that is not a KMS key ARN — because
/// each of them is the parameter carrying no value the service can use: the caller
/// is told the value is not valid rather than that it is missing, so it looks at
/// the key it supplied instead of supplying it again.
pub const KMS_KEY_NOT_VALID_MESSAGE: &str =
    "The request failed because the value specified for the kmsKeyArn parameter is not valid.";

/// Refuse a `DestinationProperties` this service cannot accept, returning the
/// destination and KMS key ARNs it validated, or the message the refusal carries.
///
/// `CreatePublishingDestination` takes "the properties of the publishing
/// destination, including the ARNs for the destination and the KMS key used for
/// encryption"
/// (`smithy_query(query="doc",shape="CreatePublishingDestinationRequest",member="DestinationProperties")`),
/// so both ARNs are required; each must be a well-formed ARN of the service its
/// own member names ("The ARN of the resource to publish to", "The ARN of the KMS
/// key to use for encryption" — `smithy_query(shape="DestinationProperties")`),
/// and `ExpectedBucketOwner`, where supplied, must be "The 12-digit Amazon Web
/// Services account ID of the account that owns the Amazon S3 bucket".
///
/// `destination_service` is the one [`validate_destination_type`] resolved, so
/// the destination ARN is checked against the service its TYPE names rather than
/// against a hard-coded one.
pub fn validate_destination_properties(
    destination_service: &str,
    properties: &crate::ops::types::DestinationProperties,
) -> Result<(String, String), String> {
    let destination_arn = properties
        .destination_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| missing_parameter_message(DESTINATION_ARN_MEMBER))?
        .to_string();
    // A `kmsKeyArn` the create cannot encrypt with is reported as the parameter's
    // VALUE not being valid, whether the request left it out or supplied something
    // unusable ([`KMS_KEY_NOT_VALID_MESSAGE`]).
    let kms_key_arn = properties
        .kms_key_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| KMS_KEY_NOT_VALID_MESSAGE.to_string())?
        .to_string();
    // Both members the create cannot do without are present, so what is left is
    // the FORM of each value supplied — the same rule, for the same members, that
    // an UPDATE applies to the members it carries. Presence is checked for BOTH
    // members before either form is: a request that leaves a required member out
    // has not said what to publish to at all, so the missing member is the one the
    // refusal names even when another member it did supply is also malformed.
    validate_destination_property_forms(destination_service, properties)?;
    Ok((destination_arn, kms_key_arn))
}

/// Refuse a `DestinationProperties` member whose supplied VALUE is not a value of
/// that member, leaving the members the request omits alone.
///
/// Every member of `DestinationProperties` is optional
/// (`smithy_query(shape="DestinationProperties")`), so this is the half of the
/// check that does not depend on whether the operation requires a member:
/// [`validate_destination_properties`] requires the two ARNs first and then calls
/// this, while an update of an existing destination carries only the members it
/// changes and calls it directly.
///
/// `destination_service` is the AWS service the destination's own
/// `DestinationType` names ([`validate_destination_type`]), so the destination ARN
/// is checked against that service rather than against a hard-coded one.
pub fn validate_destination_property_forms(
    destination_service: &str,
    properties: &crate::ops::types::DestinationProperties,
) -> Result<(), String> {
    if let Some(destination_arn) = properties.destination_arn.as_deref() {
        if !is_arn_for(destination_service, destination_arn) {
            return Err(invalid_parameter_message(DESTINATION_ARN_MEMBER));
        }
    }
    if let Some(kms_key_arn) = properties.kms_key_arn.as_deref() {
        if !is_arn_for(KMS_SERVICE, kms_key_arn) {
            // The same sentence an absent key gets: what the caller is told is
            // that the value this parameter carries is not one the service can
            // encrypt with ([`KMS_KEY_NOT_VALID_MESSAGE`]).
            return Err(KMS_KEY_NOT_VALID_MESSAGE.to_string());
        }
    }
    if let Some(owner) = properties.expected_bucket_owner.as_deref() {
        // "The 12-digit Amazon Web Services account ID of the account that owns the
        // Amazon S3 bucket"
        // (`smithy_query(query="doc",shape="DestinationProperties",member="ExpectedBucketOwner")`),
        // which is the account-id form [`is_account_id`] reads — the one reader
        // every account-id-valued member on this service is held to. The model's
        // own `len 12..12` refuses another length at the boundary; the length is
        // part of that reader too, so this helper states the whole rule.
        if !is_account_id(owner) {
            return Err(invalid_parameter_message(EXPECTED_BUCKET_OWNER_MEMBER));
        }
    }
    Ok(())
}

/// The AWS service each `DestinationType` member publishes to: an `S3`
/// destination is an Amazon S3 bucket ARN
/// (`smithy_query(shape="DestinationType")`, whose only member is `S3`).
pub(super) const DESTINATION_SERVICES: [(&str, &str); 1] = [(DESTINATION_TYPE_S3, S3_SERVICE)];

/// The service namespace of an Amazon S3 ARN.
pub(super) const S3_SERVICE: &str = "s3";

/// The service namespace of a KMS key ARN.
pub(super) const KMS_SERVICE: &str = "kms";

/// Write a `Features` list onto `detector`. A feature the list does not name keeps
/// the status it already had.
///
/// Answers whether the list configured any switch at all: an absent member, an
/// empty list, and a list whose entries name no feature each write nothing, which
/// is what lets a caller tell a request that configured the record from one that
/// asked for no change.
pub fn apply_feature_configurations(
    detector: &mut Detector,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
    now: i64,
) -> bool {
    let Some(features) = features else {
        return false;
    };
    let mut configured = false;
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        configured = true;
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        detector.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            merge_additional_configuration_in(
                &mut detector.features,
                name,
                additional
                    .iter()
                    .map(|cfg| (cfg.name.as_deref(), cfg.status.as_deref())),
                now,
            );
        }
    }
    configured
}

/// Write a member-account `Features` list onto `member`'s stored configuration,
/// the member side of [`apply_feature_configurations`]. A feature the list does
/// not name keeps the status it already had, so one request configures only the
/// switches it names.
///
/// Private because a member's switches are the ORGANIZATION's where its
/// configuration auto-enables them: [`apply_member_configuration`] is the one
/// entry point a handler writes them through, so no per-account write can bypass
/// that rule.
pub(super) fn apply_member_feature_configurations(
    member: &mut MemberRecord,
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
    now: i64,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        member.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            merge_additional_configuration_in(
                &mut member.features,
                name,
                additional
                    .iter()
                    .map(|cfg| (cfg.name.as_deref(), cfg.status.as_deref())),
                now,
            );
        }
    }
}

/// Which of the organization's accounts one application of its auto-enable
/// settings covers — the ONE thing [`AUTO_ENABLE_NEW`] and [`AUTO_ENABLE_ALL`]
/// differ in.
///
/// `NEW` auto-enables a switch "when a new account joins the organization", while
/// `ALL` auto-enables it for "all accounts in the organization ... includes NEW
/// accounts that join the organization and accounts that may have been suspended or
/// removed from the organization in GuardDuty"
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`,
/// and the same sentences for the auto-enablement of GuardDuty itself). So which
/// statuses an application honours depends on whether the account it writes is one
/// the settings are being applied TO by a request that names it, or one the
/// administrator was already holding when the settings themselves changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoEnableScope {
    /// The accounts a request names, which GuardDuty applies the settings to
    /// "irrespective of the accounts being new or existing members"
    /// (`smithy_query(query="doc",shape="CreateMembers")`): every auto-enablement
    /// covers them, `NEW` included.
    RequestedAccounts,
    /// The member accounts the administrator ALREADY holds, when the organization's
    /// own configuration is what is being written: only `ALL` covers them, because
    /// `NEW` is about the accounts that join from then on.
    AccountsAlreadyHeld,
}

impl AutoEnableScope {
    /// Does an `OrgFeatureStatus` auto-enable its switch for an account in this
    /// scope? `AutoEnableMembers` carries the same three members
    /// ([`AUTO_ENABLE_NONE`]), so the reading serves both enums.
    fn covers(self, auto_enable: &str) -> bool {
        match self {
            Self::RequestedAccounts => org_status_flag(auto_enable),
            Self::AccountsAlreadyHeld => auto_enable == AUTO_ENABLE_ALL,
        }
    }
}

/// Write the organization's auto-enable settings onto one member account's stored
/// configuration, for an account the settings are being applied to by a request
/// that names it ([`AutoEnableScope::RequestedAccounts`]) — see
/// [`apply_organization_auto_enable_for`] for the rule and for the other scope.
pub fn apply_organization_auto_enable(
    member: &mut MemberRecord,
    organization: &OrganizationConfiguration,
    now: i64,
) -> bool {
    apply_organization_auto_enable_for(
        member,
        organization,
        now,
        AutoEnableScope::RequestedAccounts,
    )
}

/// Write the organization's auto-enable settings onto one member account's stored
/// configuration, for the accounts `scope` covers.
///
/// A feature the organization auto-enables for an account in `scope` — any
/// `OrgFeatureStatus` [`AutoEnableScope::covers`] — is ENABLED for the account,
/// together with every additional configuration that feature auto-enables for the
/// same accounts. A feature the organization auto-enables for no account in `scope`
/// keeps whatever the member's configuration already said: auto-enabled for no
/// account at all, "the administrator must manage the feature for each account
/// individually"
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`),
/// and auto-enabled only for the accounts that JOIN, an account already held is not
/// one of them. So nothing here switches a feature OFF — which is also what "when
/// you update the auto-enable setting from `ALL` or `NEW` to `NONE`, this action
/// doesn't disable the corresponding option for your existing accounts"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnableOrganizationMembers")`)
/// says.
///
/// Only the features a member request can configure ([`MEMBER_FEATURES`]) can be
/// auto-enabled, which is exactly the set an organization configuration holds
/// ([`ORG_FEATURES`]): the foundational sources are on for every account anyway and
/// no organization setting names them.
///
/// Answers whether this call wrote any switch at all — re-applying the status a
/// switch already carries writes nothing ([`set_feature_in`]) — so a caller can
/// tell a write that landed from one that left the record exactly as it was.
pub fn apply_organization_auto_enable_for(
    member: &mut MemberRecord,
    organization: &OrganizationConfiguration,
    now: i64,
    scope: AutoEnableScope,
) -> bool {
    // The configuration as it stood before, so "did anything move" is answered on
    // the stored switches themselves rather than on what the settings named.
    let stored = member.features.clone();
    for name in MEMBER_FEATURES {
        if !scope.covers(organization.feature_auto_enable(name)) {
            continue;
        }
        member.set_feature(name, ENABLED, now);
        let auto_enabled: Vec<&str> = organization
            .features
            .iter()
            .filter(|feature| feature.name == *name)
            .flat_map(|feature| feature.additional_configuration.iter())
            .filter(|additional| scope.covers(&additional.auto_enable))
            .map(|additional| additional.name.as_str())
            .collect();
        for additional in auto_enabled {
            set_additional_configuration_status_in(
                &mut member.features,
                name,
                additional,
                ENABLED,
                now,
            );
        }
    }
    member.features != stored
}

/// Write one member account's REQUESTED configuration onto its stored record under
/// the organization's auto-enable settings.
///
/// A switch the organization auto-enables — a feature, or one of a feature's
/// additional configurations, whose `OrgFeatureStatus` is anything other than
/// [`AUTO_ENABLE_NONE`] ([`org_status_flag`]) — belongs to the ORGANIZATION: the
/// request leaves it exactly where it stood, its instant included, and
/// [`apply_organization_auto_enable`] then puts the auto-enablement over it, so it
/// ends ENABLED whatever the request asked for. A switch auto-enabled for no account
/// is one "the administrator must manage ... for each account individually"
/// (`smithy_query(query="doc",shape="OrganizationFeatureConfiguration",member="AutoEnable")`,
/// and the same sentence on `OrganizationAdditionalConfiguration`), so it takes
/// exactly what the request said.
///
/// So a member's stored configuration agrees with what
/// `DescribeOrganizationConfiguration` reports for the same organization HOWEVER it
/// was written — the invariant `CreateMembers` states and maintains for the accounts
/// it records, here for the per-account write of the same stored switches.
///
/// Answers whether this call wrote any switch at all, as
/// [`apply_organization_auto_enable`] does: a request that names no configuration,
/// and one that re-states the switches the account already carries, each leave the
/// record exactly as it was.
pub fn apply_member_configuration(
    member: &mut MemberRecord,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    features: Option<
        &Vec<crate::ops::types::update_member_detectors_types::MemberFeaturesConfiguration>,
    >,
    organization: &OrganizationConfiguration,
    now: i64,
) -> bool {
    // The configuration as it stood BEFORE the request, so the switches the
    // organization owns can be put back exactly once the request has passed over
    // them.
    let stored = member.features.clone();
    // The deprecated `DataSources` view is applied first so an explicit `Features`
    // entry for the same switch wins — the order `CreateDetector` and
    // `UpdateDetector` apply the two views in.
    apply_member_data_sources(member, data_sources, now);
    apply_member_feature_configurations(member, features, now);
    restore_organization_managed_switches(&mut member.features, &stored, organization);
    // Reported from the stored switches, not from this call's return: the
    // auto-enablement is written over a configuration the request may itself have
    // moved, so the one comparison that answers "did this request write" is the
    // whole configuration against the one it found.
    apply_organization_auto_enable(member, organization, now);
    member.features != stored
}

/// Put every switch the organization manages back the way `stored` held it, after a
/// per-account write has passed over the whole configuration.
///
/// Restoring rather than filtering the request is what keeps a switch's instant
/// saying when it last CHANGED ([`set_feature_in`]): a per-account write of a switch
/// the organization owns moves nothing at all.
pub(super) fn restore_organization_managed_switches(
    features: &mut [FeatureRecord],
    stored: &[FeatureRecord],
    organization: &OrganizationConfiguration,
) {
    for name in MEMBER_FEATURES {
        if !org_status_flag(organization.feature_auto_enable(name)) {
            continue;
        }
        let (Some(record), Some(was)) = (
            features.iter_mut().find(|record| record.name == *name),
            stored.iter().find(|record| record.name == *name),
        ) else {
            continue;
        };
        record.status = was.status.clone();
        record.updated_at_millis = was.updated_at_millis;
        record.free_trial_started_at_millis = was.free_trial_started_at_millis;
    }
    // Each additional configuration is a switch of its own, with its own
    // auto-enablement, so the organization owns them one at a time: one it
    // auto-enables goes back even though the feature that carries it is managed per
    // account, and one auto-enabled for nobody keeps what the request wrote even
    // though its feature is not.
    for feature in &organization.features {
        for additional in &feature.additional_configuration {
            if !org_status_flag(&additional.auto_enable) {
                continue;
            }
            let (Some(record), Some(was)) = (
                features.iter_mut().find(|record| record.name == feature.name),
                stored
                    .iter()
                    .find(|record| record.name == feature.name)
                    .and_then(|record| {
                        record
                            .additional_configuration
                            .iter()
                            .find(|configured| configured.name == additional.name)
                    }),
            ) else {
                // Nothing to put back: the member did not carry this configuration
                // before the write, so the auto-enablement is what records it, at
                // the instant it does.
                continue;
            };
            restore_additional_configuration_in(record, was);
        }
    }
}

/// Put one additional configuration back into a stored feature record, re-inserting
/// it at its place in the stored order — by NAME, the order
/// [`set_additional_configuration_status_in`] keeps the set in — when the write that
/// passed over the feature dropped it.
pub(super) fn restore_additional_configuration_in(record: &mut FeatureRecord, was: &AdditionalConfigRecord) {
    if let Some(existing) = record
        .additional_configuration
        .iter_mut()
        .find(|configured| configured.name == was.name)
    {
        *existing = was.clone();
        return;
    }
    let at = record
        .additional_configuration
        .partition_point(|configured| configured.name < was.name);
    record.additional_configuration.insert(at, was.clone());
}

/// Set one feature's status in a stored feature list, recording `now` as its
/// `UpdatedAt`, and adding the feature when the list does not carry it yet.
///
/// The one place a feature switch is written, so a detector's configuration and a
/// member's follow the same rule: re-applying the status a feature already has
/// changes nothing, `UpdatedAt` included, which is what makes the instant say when
/// the switch last CHANGED rather than when it was last written.
pub fn set_feature_in(features: &mut Vec<FeatureRecord>, name: &str, status: &str, now: i64) {
    if let Some(existing) = features.iter_mut().find(|f| f.name == name) {
        if existing.status != status {
            existing.status = status.to_string();
            existing.updated_at_millis = now;
        }
        // Enabling a feature for the first time starts its free trial, and only the
        // first time: a feature whose trial has already begun keeps the instant it
        // began at, however often the switch moves afterwards.
        if status == ENABLED && existing.free_trial_started_at_millis.is_none() {
            existing.free_trial_started_at_millis = Some(now);
        }
    } else {
        features.push(FeatureRecord {
            name: name.to_string(),
            status: status.to_string(),
            updated_at_millis: now,
            additional_configuration: Vec::new(),
            free_trial_started_at_millis: (status == ENABLED).then_some(now),
        });
    }
}

/// Write the `(name, status)` entries a request gave for ONE feature into that
/// feature's stored additional-configuration set, keyed by NAME.
///
/// Each entry configures one member of the additional-configuration enum, so the
/// request's list states something about the members it NAMES and nothing about the
/// rest — the way a `Features` list states something only about the features it
/// names ([`apply_feature_configurations`]). So each named configuration is written
/// over the stored one and every configuration the list leaves out keeps the status
/// it already had, rather than the list replacing the feature's whole stored set.
///
/// Keying by name is also what makes a repeated name ONE configuration whose last
/// entry wins — the entries are written in order — and what keeps the stored set no
/// longer than the enum's own member list however long the request's list is. An
/// entry naming nothing configures nothing and is dropped.
pub(super) fn merge_additional_configuration_in<'a>(
    features: &mut [FeatureRecord],
    feature: &str,
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
    now: i64,
) {
    for (name, status) in entries {
        let Some(name) = name else { continue };
        set_additional_configuration_status_in(
            features,
            feature,
            name,
            status.unwrap_or(ENABLED),
            now,
        );
    }
}

/// Set ONE of a feature's additional configurations in a stored feature list,
/// leaving that feature's other additional configurations where they are — the
/// single switch [`merge_additional_configuration_in`] writes a request's entries
/// through, and the one place an additional configuration is written at all.
///
/// Follows [`set_feature_in`]'s rule for the instant: re-applying the status a
/// configuration already has changes nothing, `UpdatedAt` included. A configuration
/// the feature does not carry yet is inserted at its place in the stored order,
/// which is by NAME, so one read reports the set in a total order however it was
/// written.
pub(super) fn set_additional_configuration_status_in(
    features: &mut [FeatureRecord],
    feature: &str,
    name: &str,
    status: &str,
    now: i64,
) {
    let Some(record) = features.iter_mut().find(|f| f.name == feature) else {
        return;
    };
    if let Some(existing) = record
        .additional_configuration
        .iter_mut()
        .find(|configured| configured.name == name)
    {
        if existing.status != status {
            existing.status = status.to_string();
            existing.updated_at_millis = now;
        }
        return;
    }
    let at = record
        .additional_configuration
        .partition_point(|configured| configured.name.as_str() < name);
    record.additional_configuration.insert(
        at,
        AdditionalConfigRecord {
            name: name.to_string(),
            status: status.to_string(),
            updated_at_millis: now,
        },
    );
}

/// The status a stored feature list holds for one feature, or [`DISABLED`] where
/// the list does not carry it at all.
///
/// [`Detector::feature_status`] over a feature list held anywhere, so a detector's
/// own switches and a member account's are read the same way.
pub fn feature_status_in<'a>(features: &'a [FeatureRecord], name: &str) -> &'a str {
    features
        .iter()
        .find(|feature| feature.name == name)
        .map_or(DISABLED, |feature| feature.status.as_str())
}

/// What one data source reports, read off the feature that governs it: that
/// feature's own status and the instant it last changed, in epoch milliseconds.
///
/// `fallback_millis` is the instant reported for a feature the list does not carry
/// at all, which reports [`DISABLED`].
pub fn feature_data_source(
    features: &[FeatureRecord],
    feature: &str,
    fallback_millis: i64,
) -> (String, i64) {
    features.iter().find(|f| f.name == feature).map_or_else(
        || (DISABLED.to_string(), fallback_millis),
        |f| (f.status.clone(), f.updated_at_millis),
    )
}

/// How long GuardDuty's free trial of one feature runs, in days.
///
/// GuardDuty's own documentation of the trial: an account enabling GuardDuty, or
/// enabling one of its protection plans, gets a 30-day free trial of it. The model
/// carries no duration — `FreeTrialDaysRemaining` is a plain integer
/// (`smithy_query(shape="DataSourceFreeTrial")`) — so the length is stated here
/// once, and everything a read reports about the trial is derived from it.
pub const FREE_TRIAL_DAYS: i32 = 30;

/// Milliseconds in one day: the unit a duration this service counts in DAYS is
/// measured against, the stored instants being epoch milliseconds.
pub const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// The whole days left in one feature's free trial at `now` — what
/// `FreeTrialDaysRemaining` reports for it.
///
/// The trial runs [`FREE_TRIAL_DAYS`] days from the instant it began
/// ([`FeatureRecord::free_trial_started_at_millis`]), so each whole day that has
/// elapsed since then consumes one: a feature whose trial has not begun has the
/// whole of it left, and one whose window has run out has none. The count follows
/// the CLOCK and not the current switch, because the window is a span of time and
/// not a measure of use: a feature switched off part-way through its trial has the
/// same days left as one left on.
///
/// One reader for the detector's own features and for a member account's, so two
/// reports of one account's trial cannot disagree.
pub fn free_trial_days_remaining(features: &[FeatureRecord], feature: &str, now: i64) -> i32 {
    let Some(started) = features
        .iter()
        .find(|f| f.name == feature)
        .and_then(|f| f.free_trial_started_at_millis)
    else {
        return FREE_TRIAL_DAYS;
    };
    let elapsed_days = i32::try_from(now.saturating_sub(started) / DAY_MILLIS).unwrap_or(i32::MAX);
    FREE_TRIAL_DAYS
        .saturating_sub(elapsed_days)
        .clamp(0, FREE_TRIAL_DAYS)
}

/// Write the deprecated `DataSources` view onto `detector`.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="CreateDetector")`), so each data source switch is
/// stored as the feature that governs it — one source of truth, read back through
/// either view.
pub fn apply_data_sources(
    detector: &mut Detector,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) -> bool {
    apply_data_sources_to(&mut detector.features, data_sources, now)
}

/// Write the deprecated `DataSources` view onto `member`'s stored configuration,
/// the member side of [`apply_data_sources`]. Private for the reason
/// [`apply_member_feature_configurations`] is.
pub(super) fn apply_member_data_sources(
    member: &mut MemberRecord,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) -> bool {
    apply_data_sources_to(&mut member.features, data_sources, now)
}

/// Write the deprecated `DataSources` view onto a stored feature list: the switch
/// each data source names is the feature that governs it, so the detector view and
/// the member view cannot disagree about which switch a node writes.
///
/// Answers whether the view configured any switch, the same reading
/// [`apply_feature_configurations`] gives its own list: a view that names no data
/// source writes nothing.
pub(super) fn apply_data_sources_to(
    features: &mut Vec<FeatureRecord>,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) -> bool {
    let Some(data_sources) = data_sources else {
        return false;
    };
    let mut configured = false;
    let status = |enable: bool| if enable { ENABLED } else { DISABLED };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        set_feature_in(features, FEATURE_S3_DATA_EVENTS, status(s3.enable), now);
        configured = true;
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        set_feature_in(
            features,
            FEATURE_EKS_AUDIT_LOGS,
            status(kubernetes.audit_logs.enable),
            now,
        );
        configured = true;
    }
    if let Some(enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|m| m.scan_ec2_instance_with_findings.as_ref())
        .and_then(|s| s.ebs_volumes)
    {
        set_feature_in(features, FEATURE_EBS_MALWARE_PROTECTION, status(enable), now);
        configured = true;
    }
    configured
}

/// Every feature a member-account request may configure: the `OrgFeature` members
/// `MemberFeaturesConfiguration.Name` targets
/// (`smithy_query(shape="MemberFeaturesConfiguration")`), taken from the crate's
/// MODEL-DERIVED list so a member the model gains cannot be missed here.
///
/// A narrower catalog than [`DETECTOR_FEATURES`]: the features an administrator
/// configures for a member account are the organisation-level ones.
pub const MEMBER_FEATURES: &[&str] = crate::ops::types::shared_types::ORG_FEATURE_VALUES;

/// Every additional configuration a member-account request may name: the
/// `OrgFeatureAdditionalConfiguration` members
/// `MemberAdditionalConfiguration.Name` targets
/// (`smithy_query(shape="MemberAdditionalConfiguration")`).
pub const MEMBER_ADDITIONAL_CONFIGURATIONS: &[&str] =
    crate::ops::types::shared_types::ORG_FEATURE_ADDITIONAL_CONFIGURATION_VALUES;

/// Every additional configuration a DETECTOR request may name: the
/// `FeatureAdditionalConfiguration` members
/// `DetectorAdditionalConfiguration.Name` targets
/// (`smithy_query(shape="DetectorAdditionalConfiguration")`).
pub const DETECTOR_ADDITIONAL_CONFIGURATIONS: &[&str] =
    crate::ops::types::shared_types::FEATURE_ADDITIONAL_CONFIGURATION_VALUES;

/// Every feature a MEMBER's configuration reports, in the order it reports them:
/// the [`MEMBER_FEATURES`] in [`DETECTOR_FEATURES`] (model) order, so one member
/// read orders its features the way a detector read orders its own.
///
/// The list is a total order over a SET, so a client may index it positionally.
pub fn member_reported_features() -> Vec<&'static str> {
    DETECTOR_FEATURES
        .iter()
        .copied()
        .filter(|name| MEMBER_FEATURES.contains(name))
        .collect()
}

/// The deprecated `DataSources` view of one stored feature list.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="GetDetector")`), so every node here is read back off
/// the feature that governs it: the same stored switch, projected under its other
/// name. A node reports exactly the members the model declares for it
/// (`smithy_query(shape="DataSourceConfigurationsResult")`) — each data source's
/// `Status`, and nothing else; the instant a switch last changed is reported by the
/// `Features` view, which is the view that models it.
///
/// One projection for both readers — `GetDetector` for a detector and
/// `GetMemberDetectors` for a member account — so two reads of one switch cannot
/// disagree. `malware_protection_service_role` is the Malware Protection
/// service-linked role to report, which is `None` for a role this emulator does
/// not hold (the member account's own), and the member is optional, so it is
/// omitted rather than invented.
pub fn data_source_configurations_view(
    features: &[FeatureRecord],
    malware_protection_service_role: Option<&str>,
) -> crate::ops::types::shared_types::DataSourceConfigurationsResult {
    use crate::ops::types::shared_types::*;

    let reported = |feature: &str| feature_status_in(features, feature).to_string();
    DataSourceConfigurationsResult {
        cloud_trail: CloudTrailConfigurationResult {
            status: reported(FEATURE_CLOUD_TRAIL),
        },
        dns_logs: DNSLogsConfigurationResult {
            status: reported(FEATURE_DNS_LOGS),
        },
        flow_logs: FlowLogsConfigurationResult {
            status: reported(FEATURE_FLOW_LOGS),
        },
        s3_logs: S3LogsConfigurationResult {
            status: reported(FEATURE_S3_DATA_EVENTS),
        },
        kubernetes: Some(KubernetesConfigurationResult {
            audit_logs: KubernetesAuditLogsConfigurationResult {
                status: reported(FEATURE_EKS_AUDIT_LOGS),
            },
        }),
        malware_protection: Some(MalwareProtectionConfigurationResult {
            scan_ec2_instance_with_findings: Some(ScanEc2InstanceWithFindingsResult {
                ebs_volumes: Some(EbsVolumesResult {
                    status: Some(reported(FEATURE_EBS_MALWARE_PROTECTION)),
                    // Nothing stopped the source from being enabled, so there is no
                    // reason to report.
                    reason: None,
                }),
            }),
            service_role: malware_protection_service_role.map(str::to_string),
        }),
    }
}

/// The modelled `MemberDataSourceConfiguration` a read reports for one stored
/// member account: its configuration under both the `Features` and the deprecated
/// `DataSources` view, the same two views `GetDetector` reports for a detector.
///
/// The projection lives here, beside the record, because it is what a member
/// configuration READ answers with — `GetMemberDetectors` today, and any later
/// reader of the same facts.
pub fn member_data_source_configuration_view(
    member: &MemberRecord,
) -> crate::ops::types::get_member_detectors_types::MemberDataSourceConfiguration {
    use crate::ops::types::get_member_detectors_types::*;

    MemberDataSourceConfiguration {
        account_id: member.account_id.clone(),
        data_sources: data_source_configurations_view(
            &member.features,
            // The Malware Protection service-linked role of a member account lives
            // in THAT account, which this emulator does not hold, and the member is
            // optional: omitted rather than invented.
            None,
        ),
        features: Some(
            member_reported_features()
                .into_iter()
                .map(|name| {
                    let record = member.features.iter().find(|f| f.name == name);
                    MemberFeaturesConfigurationResult {
                        name: Some(name.to_string()),
                        status: Some(
                            record
                                .map_or(DISABLED, |f| f.status.as_str())
                                .to_string(),
                        ),
                        updated_at: Some(::aws_smithy_types::DateTime::from_millis(
                            record.map_or(member.updated_at_millis, |f| f.updated_at_millis),
                        )),
                        additional_configuration: record
                            .filter(|f| !f.additional_configuration.is_empty())
                            .map(|f| {
                                f.additional_configuration
                                    .iter()
                                    .map(|cfg| MemberAdditionalConfigurationResult {
                                        name: Some(cfg.name.clone()),
                                        status: Some(cfg.status.clone()),
                                        updated_at: Some(
                                            ::aws_smithy_types::DateTime::from_millis(
                                                cfg.updated_at_millis,
                                            ),
                                        ),
                                        // Nothing in this emulator configures a
                                        // member through a GuardDuty policy, so the
                                        // optional member is absent.
                                        managed_by: None,
                                    })
                                    .collect()
                            }),
                        managed_by: None,
                    }
                })
                .collect(),
        ),
    }
}
