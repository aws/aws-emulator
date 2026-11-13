// SPDX-License-Identifier: CC0-1.0

//! A domain's configuration beyond what the request carried: the storage performance
//! members the service refuses, the option blocks it fills in ITSELF for a request
//! that named none, and the members a descriptor of a stored domain derives at the
//! moment of the read.
//!
//! All three are one subject — what the SERVICE, rather than the caller, puts in a
//! domain's configuration — and they live here, beside the store rather than in it,
//! so that:
//!
//! * the defaults are written ONCE, into the stored record ([`apply_server_defaults`]),
//!   by the create and the update paths, and every read path reports them from that
//!   one record rather than filling them in again per operation; and
//! * the derived members are derived ONCE ([`reported_status`], [`reported_config`],
//!   [`reported_change_progress`]), so `CreateDomain`, `DescribeDomain`,
//!   `DescribeDomains`, `DeleteDomain`, `DescribeDomainConfig`, `UpdateDomainConfig`
//!   and `DescribeDomainChangeProgress` cannot report one stored domain differently.
//!
//! A DERIVED member is one no stored value can express, because it says where the
//! domain is at the instant of the read: whether the configuration change in flight
//! has begun being applied at all (a configuration cannot be applied to a cluster the
//! service has not provisioned yet), which properties that change is applying, and
//! which service software a cluster that does not exist yet runs — none.

use crate::ops::types::*;
use crate::state::{
    datetime, ConfigChange, Domain, AUTO_TUNE_STATE_DISABLED,
    OPT_ACCESS_POLICIES, OPT_ADVANCED_OPTIONS, OPT_ADVANCED_SECURITY_OPTIONS, OPT_AIML_OPTIONS,
    OPT_AUTOMATED_SNAPSHOT_PAUSE_OPTIONS, OPT_AUTO_TUNE_OPTIONS, OPT_CLUSTER_CONFIG,
    OPT_COGNITO_OPTIONS, OPT_DEPLOYMENT_STRATEGY_OPTIONS, OPT_DOMAIN_ENDPOINT_OPTIONS,
    OPT_EBS_OPTIONS, OPT_ENCRYPTION_AT_REST_OPTIONS, OPT_ENGINE_MODE, OPT_ENGINE_VERSION,
    OPT_IDENTITY_CENTER_OPTIONS, OPT_IP_ADDRESS_TYPE, OPT_LOG_PUBLISHING_OPTIONS,
    OPT_NODE_TO_NODE_ENCRYPTION_OPTIONS, OPT_OFF_PEAK_WINDOW_OPTIONS, OPT_SNAPSHOT_OPTIONS,
    OPT_SOFTWARE_UPDATE_OPTIONS, OPT_TAGS, OPT_USE_CASE, OPT_VPC_OPTIONS, VOLUME_TYPE_GP2,
    VOLUME_TYPE_GP3, VOLUME_TYPE_IO1,
};

use ::serde_json::Value;

/// `ConfigChangeStatus`'s `Pending` (one of
/// `crate::ops::types::CONFIG_CHANGE_STATUS_VALUES`): the change has been accepted and
/// the service has not begun applying it.
pub const CONFIG_CHANGE_PENDING: &str = "Pending";

/// `OverallChangeStatus`'s `PENDING` (one of
/// `crate::ops::types::OVERALL_CHANGE_STATUS_VALUES`): the same statement about the
/// change as a whole.
pub const CHANGE_STATUS_PENDING: &str = "PENDING";

// ---------------------------------------------------------------------------
// The configuration the service provisions a domain with
//
// `CreateDomain` names a handful of options and the service provisions the domain
// with a WHOLE configuration: every option block the request left out is set to
// the value the service defaults it to, and that value is part of the domain from
// the moment it is created — which is why it is written into the stored record
// here rather than invented by each operation that reports one.
// ---------------------------------------------------------------------------

/// `TLSSecurityPolicy`'s `Policy-Min-TLS-1-2-2019-07` (one of
/// `crate::ops::types::T_L_S_SECURITY_POLICY_VALUES`): the policy a domain's HTTPS
/// endpoint applies when the create named none.
pub const DEFAULT_TLS_SECURITY_POLICY: &str = "Policy-Min-TLS-1-2-2019-07";

/// `IPAddressType`'s `ipv4` (one of `crate::ops::types::I_P_ADDRESS_TYPE_VALUES`, the
/// other being `dualstack`): the address family a domain is reached over when the
/// create named none.
pub const IP_ADDRESS_TYPE_IPV4: &str = "ipv4";

/// `DeploymentStrategy`'s `CapacityOptimized` (one of
/// `crate::ops::types::DEPLOYMENT_STRATEGY_VALUES`): the strategy a domain is deployed
/// with when the create named none.
pub const DEFAULT_DEPLOYMENT_STRATEGY: &str = "CapacityOptimized";

/// `NaturalLanguageQueryGenerationDesiredState`'s `DISABLED`: the feature is off on a
/// domain that did not ask for it.
pub const NATURAL_LANGUAGE_QUERY_GENERATION_DISABLED: &str = "DISABLED";

/// `NaturalLanguageQueryGenerationCurrentState`'s `NOT_ENABLED` (one of
/// `crate::ops::types::NATURAL_LANGUAGE_QUERY_GENERATION_CURRENT_STATE_VALUES`): the
/// feature has never been enabled on the domain — a different statement from
/// "disabling it has finished".
pub const NATURAL_LANGUAGE_QUERY_GENERATION_NOT_ENABLED: &str = "NOT_ENABLED";

/// `PauseState`'s `Disabled` (one of `crate::ops::types::PAUSE_STATE_VALUES`): no
/// automated snapshot pause is in force on the domain.
pub const PAUSE_STATE_DISABLED: &str = "Disabled";

/// `RollbackOnDisable`'s `NO_ROLLBACK` (one of
/// `crate::ops::types::ROLLBACK_ON_DISABLE_VALUES`): "specify `NO_ROLLBACK` to retain
/// all prior Auto-Tune settings" — what disabling Auto-Tune on a domain nobody has
/// asked a rollback of does.
pub const ROLLBACK_ON_DISABLE_NO_ROLLBACK: &str = "NO_ROLLBACK";

/// The advanced option that decides whether the domain reports the engine's own
/// version in its root response, named as `AdvancedOptions` documents it. Off unless
/// the caller asks for it.
const ADVANCED_OPTION_OVERRIDE_MAIN_RESPONSE_VERSION: &str = "override_main_response_version";

/// The advanced option that decides whether "explicit references to indexes are
/// allowed inside the body of HTTP requests", which `AdvancedOptions` documents as
/// "Default is true".
const ADVANCED_OPTION_ALLOW_EXPLICIT_INDEX: &str = "rest.action.multi.allow_explicit_index";

/// The baseline IOPS a [`VOLUME_TYPE_GP3`] volume is provisioned with when the request
/// names none: the floor the instance-type limit catalog publishes for that volume
/// type as `MinimumIops` (`crate::state::instance_type_limits`).
const GP3_BASELINE_IOPS: i32 = 3_000;

/// The baseline throughput, in MiB/s, such a volume is provisioned with when the
/// request names none — likewise that volume type's published `MinimumThroughput`.
const GP3_BASELINE_THROUGHPUT_MIB: i32 = 125;

/// The hour of the UTC day a domain's off-peak window opens when the create named no
/// start time — the window `Domain::next_off_peak_window_start` reads for a domain that
/// configured no custom start.
///
/// The service assigns every new domain "a custom 10-hour, low-traffic window during
/// which OpenSearch Service can perform mandatory configuration changes", and the start
/// time it assigns is the SERVICE's, not the shape's: `WindowStartTime$Hours` carries a
/// modelled default of zero, which is what an absent member decodes as, while a domain
/// that named no window comes up with the window the service picked for it.
///
/// WHICH hour the real service picks is unpublished, so this is the emulator's OWN choice
/// of one fixed hour — the hour observed on a domain created without a window — and not a
/// documented value. What the caller can rely on is that the domain carries a window it
/// did not ask for and that every read reports the same one.
const DEFAULT_OFF_PEAK_WINDOW_HOUR: i64 = 5;

/// The hour of the UTC day the daily automated snapshot is taken at when the create
/// names none: `SnapshotOptions$AutomatedSnapshotStartHour` documents "Default is
/// `0` hours".
const DEFAULT_AUTOMATED_SNAPSHOT_START_HOUR: i32 = 0;

/// The volume types whose per-volume IOPS are PROVISIONED, so `EBSOptions$Iops` names
/// something for them: "applicable only for the `gp3` and provisioned IOPS EBS volume
/// types", the provisioned-IOPS one of `crate::ops::types::VOLUME_TYPE_VALUES` being
/// `io1`.
const PROVISIONED_IOPS_VOLUME_TYPES: [&str; 2] = [VOLUME_TYPE_GP3, VOLUME_TYPE_IO1];

/// Set `held` to `default` when the record holds no value for it, leaving a value the
/// request supplied exactly as it came.
fn default_to<T>(held: &mut Option<T>, default: T) {
    if held.is_none() {
        *held = Some(default);
    }
}

/// The option container `held` carries, minted empty when it carries none: a block the
/// service defaults is a block the domain HAS, so each of its members is defaulted
/// inside one container.
fn container<T: Default>(held: &mut Option<T>) -> &mut T {
    held.get_or_insert_with(T::default)
}

/// Fill in the value the service itself sets for every option block `domain` holds
/// none for, and for every member of a block the request filled only part of.
///
/// This is the configuration the service PROVISIONS the domain with: a create names a
/// few options and the domain comes up with a whole configuration, so a caller reading
/// any option block back — to decide whether encryption at rest is on, what TLS policy
/// applies, or what a volume's IOPS and throughput are — reads a value rather than
/// nothing. Called by the create path and by the update path, so a block an update
/// introduces (switching a volume to `gp3`, say) is completed the same way.
///
/// Every default is the one the service documents or is recorded answering with, and a
/// member the request DID carry is never overwritten.
pub fn apply_server_defaults(domain: &mut Domain) {
    // "Identity and Access Management (IAM) policy document specifying the access
    // policies for the new domain": a domain created without one has an empty policy,
    // which is the empty document rather than no value at all.
    default_to(&mut domain.access_policies, String::new());

    // The two advanced options the service sets on every domain, at the defaults
    // `AdvancedOptions` documents for them. "Note the use of a string rather than a
    // boolean": the map's values are the words `true` and `false`.
    for (option, default) in [
        (ADVANCED_OPTION_OVERRIDE_MAIN_RESPONSE_VERSION, false),
        (ADVANCED_OPTION_ALLOW_EXPLICIT_INDEX, true),
    ] {
        domain
            .advanced_options
            .entry(option.to_string())
            .or_insert_with(|| default.to_string());
    }

    // Every machine learning feature is off on a domain that did not ask for one, and
    // a feature that was never enabled reports that rather than a finished disabling.
    let aiml = container(&mut domain.aiml_options);
    let generation = container(&mut aiml.natural_language_query_generation_options);
    default_to(
        &mut generation.desired_state,
        NATURAL_LANGUAGE_QUERY_GENERATION_DISABLED.to_string(),
    );
    default_to(
        &mut generation.current_state,
        NATURAL_LANGUAGE_QUERY_GENERATION_NOT_ENABLED.to_string(),
    );
    default_to(&mut container(&mut aiml.s3_vectors_engine).enabled, false);
    default_to(
        &mut container(&mut aiml.serverless_vector_acceleration).enabled,
        false,
    );

    // Fine-grained access control is off, and so are the internal user database it
    // would be served by and the anonymous-auth migration period.
    let security = container(&mut domain.advanced_security_options);
    default_to(&mut security.enabled, false);
    default_to(&mut security.internal_user_database_enabled, false);
    default_to(&mut security.anonymous_auth_enabled, false);

    // Auto-Tune is `DISABLED` on a new domain, and would deploy its changes on a
    // maintenance schedule rather than in the domain's off-peak window.
    let auto_tune = container(&mut domain.auto_tune_options);
    default_to(
        &mut auto_tune.desired_state,
        AUTO_TUNE_STATE_DISABLED.to_string(),
    );
    default_to(&mut auto_tune.use_off_peak_window, false);

    // No automated snapshot pause is in force: "suspending snapshots reduces data
    // protection", so it is off until a caller asks for one.
    let pause = container(&mut domain.automated_snapshot_pause_options);
    default_to(&mut pause.state, PAUSE_STATE_DISABLED.to_string());

    // The cluster topology toggles: a domain sits in a single zone, with no dedicated
    // master node, no standby zone and neither of the two storage tiers below the hot
    // one, unless the request asked for them.
    let cluster = &mut domain.cluster_config;
    for toggle in [
        &mut cluster.dedicated_master_enabled,
        &mut cluster.zone_awareness_enabled,
        &mut cluster.warm_enabled,
        &mut cluster.multi_az_with_standby_enabled,
    ] {
        default_to(toggle, false);
    }
    default_to(
        &mut cluster.cold_storage_options,
        ColdStorageOptions { enabled: false },
    );

    // A `gp3` volume's performance is PROVISIONED, so such a volume carries the
    // baseline the service provisions it at when the request named neither figure. No
    // other volume type provisions either, and is left exactly as it came.
    if let Some(ebs) = domain.ebs_options.as_mut() {
        if ebs.volume_type.as_deref() == Some(VOLUME_TYPE_GP3) {
            default_to(&mut ebs.iops, GP3_BASELINE_IOPS);
            default_to(&mut ebs.throughput, GP3_BASELINE_THROUGHPUT_MIB);
        }
    }

    // Amazon Cognito authentication for OpenSearch Dashboards is off.
    default_to(&mut container(&mut domain.cognito_options).enabled, false);

    default_to(
        &mut domain.deployment_strategy_options,
        DeploymentStrategyOptions {
            deployment_strategy: DEFAULT_DEPLOYMENT_STRATEGY.to_string(),
        },
    );

    // The domain endpoint does not require HTTPS, carries no custom endpoint of its
    // own, and applies the TLS policy the service picks for a new domain.
    let endpoint_options = container(&mut domain.domain_endpoint_options);
    default_to(&mut endpoint_options.enforce_https, false);
    default_to(&mut endpoint_options.custom_endpoint_enabled, false);
    default_to(
        &mut endpoint_options.tls_security_policy,
        DEFAULT_TLS_SECURITY_POLICY.to_string(),
    );

    // Neither kind of encryption is on unless the create asked for it.
    default_to(
        &mut container(&mut domain.encryption_at_rest_options).enabled,
        false,
    );
    default_to(
        &mut container(&mut domain.node_to_node_encryption_options).enabled,
        false,
    );

    // IAM Identity Center is not integrated with the domain's user interface: the
    // domain HAS the settings container, and holds no setting in it until a caller
    // names the Identity Center instance to integrate with.
    container(&mut domain.identity_center_options);

    default_to(
        &mut domain.ip_address_type,
        IP_ADDRESS_TYPE_IPV4.to_string(),
    );

    // "All domains created after [February 16, 2023] have an off-peak window by
    // default", opening at the hour of the UTC day the service assigns one that named
    // no start time ([`DEFAULT_OFF_PEAK_WINDOW_HOUR`]) — the window
    // `Domain::next_off_peak_window_start` reads.
    let off_peak = container(&mut domain.off_peak_window_options);
    default_to(&mut off_peak.enabled, true);
    let window = container(&mut off_peak.off_peak_window);
    default_to(
        &mut window.window_start_time,
        WindowStartTime {
            hours: DEFAULT_OFF_PEAK_WINDOW_HOUR,
            minutes: 0,
        },
    );

    default_to(
        &mut container(&mut domain.snapshot_options).automated_snapshot_start_hour,
        DEFAULT_AUTOMATED_SNAPSHOT_START_HOUR,
    );

    // Service software is updated when a caller asks for it, not automatically; a
    // deployment that does run takes "the latest available service software", which is
    // what the service sets for a new domain.
    let software_update = container(&mut domain.software_update_options);
    default_to(&mut software_update.auto_software_update_enabled, false);
    default_to(
        &mut software_update.use_latest_service_software_for_blue_green,
        true,
    );
}

/// The access policy document as the service HOLDS it: the tokens the caller wrote, in
/// the order they were written, with the whitespace BETWEEN them — which denotes
/// nothing — dropped, so what a caller reads back is the service's own rendering of the
/// document it was handed rather than the layout it was typed in.
///
/// That rendering is the one every document this service holds goes through
/// (`crate::state::stored_policy_document` renders a data source's policy with it), and
/// no value inside the document is re-rendered from a parse: a number keeps the digits
/// it was written with and a string its exact content. A document that is not JSON
/// denotes no document to re-render, and is held exactly as it came.
pub fn stored_access_policies(document: Option<String>) -> Option<String> {
    let document = document?;
    if ::serde_json::from_str::<Value>(&document).is_err() {
        return Some(document);
    }
    Some(::service_common::json_document::minified(&document))
}

/// The refusal a request earns for naming an EBS performance member the volume type it
/// asks for does not provision, or `None` when every member it named applies to that
/// type.
///
/// `EBSOptions$Throughput` is "applicable only for the `gp3` volume type" and
/// `EBSOptions$Iops` "only for the `gp3` and provisioned IOPS EBS volume types": a
/// request setting one on another type asks for performance a volume of that type does
/// not have, so it is refused rather than stored — no domain is created carrying a
/// figure its own volume could not honour. A request naming no volume type gets the
/// general-purpose type the service defaults to, which is the type the figure is
/// judged against.
pub fn ebs_performance_refusal(ebs: Option<&EBSOptions>) -> Option<ValidationException> {
    let ebs = ebs?;
    let volume_type = ebs.volume_type.as_deref().unwrap_or(VOLUME_TYPE_GP2);
    if ebs.throughput.is_some() && volume_type != VOLUME_TYPE_GP3 {
        return Some(ValidationException {
            message: Some(format!(
                "Throughput is only valid when volume type is {}.",
                VOLUME_TYPE_GP3.to_uppercase()
            )),
        });
    }
    if ebs.iops.is_some() && !PROVISIONED_IOPS_VOLUME_TYPES.contains(&volume_type) {
        return Some(ValidationException {
            message: Some(format!(
                "IOPS is only valid when volume type is {VOLUME_TYPE_IO1} or {VOLUME_TYPE_GP3}."
            )),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// The derived members of a descriptor
//
// What a descriptor says about WHERE the domain is at the instant of the read:
// whether the change in flight has begun being applied, which properties it is
// applying, and what service software its cluster runs. None of them is a stored
// value, and all of them are derived HERE, so no two read paths can disagree.
// ---------------------------------------------------------------------------

/// This domain as `DomainStatus` — what `CreateDomain`, `DescribeDomain`,
/// `DescribeDomains` and `DeleteDomain` answer with: the stored record's own
/// projection, with the members only the instant of the read decides.
pub fn reported_status(domain: &Domain) -> DomainStatus {
    let mut status = domain.to_status();
    status.change_progress_details = reported_change_progress_details(domain);
    status.modifying_properties = Some(reported_modifying_properties(domain));
    status.service_software_options = Some(reported_service_software(domain));
    status
}

/// This domain as `DomainConfig` — what `DescribeDomainConfig` and
/// `UpdateDomainConfig` answer with, with the same members derived the same way.
pub fn reported_config(domain: &Domain) -> DomainConfig {
    let mut config = domain.to_config();
    config.change_progress_details = reported_change_progress_details(domain);
    config.modifying_properties = Some(reported_modifying_properties(domain));
    // Every domain has a VPC CONFIGURATION, empty for one reached on a public
    // endpoint: the container is reported with whatever the domain holds — empty
    // rather than absent — the same rule the log-publishing container follows, because
    // a caller diffing desired configuration against actual needs a container to read.
    if config.vpc_options.is_none() {
        config.vpc_options = domain
            .option_status(OPT_VPC_OPTIONS)
            .map(|status| VPCDerivedInfoStatus {
                options: VPCDerivedInfo::default(),
                status,
            });
    }
    // The Auto-Tune settings the SERVICE fills in, reported with the configuration
    // rather than stored on the domain: "specify `NO_ROLLBACK` to retain all prior
    // Auto-Tune settings" is what a domain nobody has asked a rollback of is under, and
    // `MaintenanceSchedules` is "a list of maintenance schedules" that a domain with no
    // schedule has none of — an empty list rather than nothing, so a caller reading the
    // Auto-Tune rollback and maintenance settings reads a value. The two are not part of
    // the domain's own record, so what a caller SET stays what the stored record holds
    // and what a configuration change reports it is applying.
    if let Some(auto_tune) = config.auto_tune_options.as_mut() {
        let options = container(&mut auto_tune.options);
        default_to(&mut options.maintenance_schedules, Vec::new());
        default_to(
            &mut options.rollback_on_disable,
            ROLLBACK_ON_DISABLE_NO_ROLLBACK.to_string(),
        );
    }
    config
}

/// Whether `domain` holds a SETTING IN FORCE for the option `option` names: a value
/// saying something is configured, rather than an option block that is empty or one
/// whose every setting is switched off.
///
/// What an option holds decides whether a change in flight is applying anything TO it
/// (`Domain::option_state`): a feature that is off, and a container the domain holds
/// nothing in, has nothing for the service to apply to a cluster, so such an option is
/// not reported as being modified — the log-publishing configuration of a domain that
/// publishes no log type and the VPC configuration of a domain reached on a public
/// endpoint are two of them. Read from the SAME projection of the stored record a
/// change reports its properties from ([`option_value`]), so the two cannot disagree
/// about what an option holds.
pub fn option_setting_in_force(domain: &Domain, option: &str) -> bool {
    option_value(domain, option).is_some_and(|value| setting_in_force(&value))
}

/// Whether `value` states a setting that is in force: a number, a non-empty text or a
/// toggle that is ON, or a container or list holding at least one of those. An empty
/// container, an empty text and a toggle that is off state that nothing is configured.
fn setting_in_force(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(enabled) => *enabled,
        Value::String(text) => !text.is_empty(),
        Value::Number(_) => true,
        Value::Array(elements) => elements.iter().any(setting_in_force),
        Value::Object(members) => members.values().any(setting_in_force),
    }
}

/// The `ChangeProgressStatusDetails` `DescribeDomainChangeProgress` answers with for
/// `change` on `domain`.
pub fn reported_change_progress(
    domain: &Domain,
    change: &ConfigChange,
) -> ChangeProgressStatusDetails {
    let mut progress = change.to_progress_status();
    if awaits_provisioning(domain, change) {
        progress.config_change_status = Some(CONFIG_CHANGE_PENDING.to_string());
        progress.status = Some(CHANGE_STATUS_PENDING.to_string());
    }
    // "The specific stages that the domain is going through to perform the
    // configuration change": the service publishes none for a change it has not begun
    // applying, which is an empty list rather than no list at all — a caller walking
    // the stages has a list to walk. "The total number of stages required for the
    // configuration change" is the number of stages that list carries, so the
    // denominator a caller renders progress against is the stages the change reports:
    // this emulator applies a change in ONE step and publishes no stage, so the list is
    // empty and the total is zero.
    progress.total_number_of_stages = 0;
    progress.change_progress_stages = Some(Vec::new());
    // Both lists name the same properties the same way; which of the two a property
    // sits in is `ConfigChange`'s own answer.
    if let Some(pending) = progress.pending_properties.take() {
        progress.pending_properties = Some(reported_properties(domain, &pending));
    }
    if let Some(completed) = progress.completed_properties.take() {
        progress.completed_properties = Some(reported_properties(domain, &completed));
    }
    progress
}

/// The `ChangeProgressDetails` every full descriptor of `domain` carries: the most
/// recent configuration change, in the status that change is really in.
fn reported_change_progress_details(domain: &Domain) -> Option<ChangeProgressDetails> {
    let change = domain.latest_change()?;
    let mut details = change.to_details();
    if awaits_provisioning(domain, change) {
        details.config_change_status = Some(CONFIG_CHANGE_PENDING.to_string());
    }
    Some(details)
}

/// Whether `change` is still WAITING to be applied rather than being applied: it is in
/// flight, and the cluster whose configuration it changes has not been provisioned
/// yet.
///
/// A configuration cannot be applied to a cluster that does not exist, so the change a
/// create starts stays `Pending` for as long as the domain is being created, while a
/// change on a provisioned cluster is `ApplyingChanges` for as long as it is in
/// flight.
fn awaits_provisioning(domain: &Domain, change: &ConfigChange) -> bool {
    change.in_progress() && !domain.cluster_is_up()
}

/// `DomainStatus$ModifyingProperties` / `DomainConfig$ModifyingProperties`:
/// "information about the domain properties that are currently being modified".
///
/// Those are the properties of the configuration change IN FLIGHT — the same leaf
/// properties that change reports as pending — each with the value it is being moved
/// to and the kind that value is. A domain with no change in flight is modifying
/// nothing, which is the empty list rather than no list: a caller enumerating what is
/// being modified has a list to enumerate.
///
/// `ActiveValue` is "the current value of the domain property that is being modified",
/// and a property the change in flight is SETTING — every property of a domain being
/// created — had no value before it: the member is reported as the empty value rather
/// than left out.
fn reported_modifying_properties(domain: &Domain) -> Vec<ModifyingProperties> {
    let Some(change) = domain.latest_change().filter(|change| change.in_progress()) else {
        return Vec::new();
    };
    let mut modifying: Vec<ModifyingProperties> = change_leaf_values(domain, &change.properties)
        .into_iter()
        .map(|(name, value)| {
            let pending = property_text(&value);
            ModifyingProperties {
                name: Some(name),
                active_value: Some(String::new()),
                value_type: Some(property_value_type(&pending).to_string()),
                pending_value: Some(pending),
            }
        })
        .collect();
    modifying.sort_by(|left, right| left.name.cmp(&right.name));
    modifying
}

/// The service software a descriptor of `domain` reports — and what
/// `StartServiceSoftwareUpdate` and `CancelServiceSoftwareUpdate` answer with, so the
/// record one of them moves and the record a later descriptor reports are one answer.
///
/// "The current service software version present on the domain" is the software the
/// domain's NODES run, and a domain whose cluster is not there has no node to run any:
/// the version is reported empty while the domain is still being provisioned and once it
/// is being torn down ([`Domain::cluster_exists`]), the same way the endpoint it serves
/// traffic on is withheld then. `NewVersion` is likewise the empty
/// version rather than no member when no release is available to move onto, and the
/// date past which the service would update the domain by itself is the epoch when no
/// such update is scheduled — the two members a caller reads to decide whether an
/// update is due.
pub fn reported_service_software(domain: &Domain) -> ServiceSoftwareOptions {
    let mut options = domain.service_software.to_options();
    if !domain.cluster_exists() {
        options.current_version = Some(String::new());
    }
    default_to(&mut options.new_version, String::new());
    default_to(
        &mut options.automated_update_date,
        datetime(::std::time::SystemTime::UNIX_EPOCH),
    );
    options
}

// ---------------------------------------------------------------------------
// The properties a configuration change applies
//
// A change is reported property by property: each LEAF option container being
// set, named as the configuration names it, beside the value it is being set to.
// Both halves come from the stored record, so what a change reports is what the
// domain holds.
// ---------------------------------------------------------------------------

/// What separates a property's name from the value it is being set to.
const PROPERTY_SEPARATOR: &str = " : ";

/// What separates a container's name from the name of a container nested in it.
const PROPERTY_PATH_SEPARATOR: &str = ".";

/// `PropertyValueType`'s `STRINGIFIED_JSON`: the value "contain[s] content in JSON
/// format".
const PROPERTY_VALUE_TYPE_STRINGIFIED_JSON: &str = "STRINGIFIED_JSON";

/// `PropertyValueType`'s `PLAIN_TEXT`: the value is a "direct value".
const PROPERTY_VALUE_TYPE_PLAIN_TEXT: &str = "PLAIN_TEXT";

/// The `options` a change is applying, as `ChangeProgressStatusDetails` reports them:
/// one entry per LEAF option container, naming it and the value the domain holds for
/// it.
///
/// Ordered by name, so two reads of one change report its properties in the same
/// order.
fn reported_properties(domain: &Domain, options: &[String]) -> Vec<String> {
    let mut properties: Vec<String> = change_leaf_values(domain, options)
        .into_iter()
        .map(|(name, value)| format!("{name}{PROPERTY_SEPARATOR}{}", property_text(&value)))
        .collect();
    properties.sort();
    properties
}

/// The leaf properties the `options` of a change amount to, each with the value the
/// domain holds for it: the ONE reading of a change's properties, so the list it
/// reports as pending and the properties it reports as modifying cannot name different
/// things.
fn change_leaf_values(domain: &Domain, options: &[String]) -> Vec<(String, Value)> {
    options
        .iter()
        .filter_map(|option| option_value(domain, option).map(|value| leaf_values(option, &value)))
        .flatten()
        .collect()
}

/// The text a property's value is reported as: a container or a list as the compact
/// JSON document it is, a direct value as itself, and a property the domain holds no
/// value for as the empty text — the same way `ActiveValue` reports a property that had
/// no value before the change.
fn property_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Which `PropertyValueType` the property reported as `text` carries, decided from that
/// text: `STRINGIFIED_JSON` where it is "content in JSON format, such as
/// {\"Enabled\":\"True\"}", and `PLAIN_TEXT` where it is a "direct value such as \"1\",
/// \"True\", or \"c5.large.search\"".
///
/// Decided from the text rather than from the member's modelled type, because the two
/// kinds the enum names are about the text a caller has to parse: a container and a list
/// render as a JSON document, and so does a DOCUMENT the caller submitted as a string —
/// an access policy is JSON content whoever typed it. A direct value, and a property the
/// domain holds no value for, is no JSON document and so plain text.
fn property_value_type(text: &str) -> &'static str {
    let json_content = ::serde_json::from_str::<Value>(text)
        .is_ok_and(|document| document.is_object() || document.is_array());
    if json_content {
        PROPERTY_VALUE_TYPE_STRINGIFIED_JSON
    } else {
        PROPERTY_VALUE_TYPE_PLAIN_TEXT
    }
}

/// The option blocks a change applies PROPERTY BY PROPERTY, each with the properties the
/// service names it by.
///
/// A change reports what it is applying at the grain the service applies it: most option
/// blocks are one property, set as a whole, while the blocks below are a GROUP of
/// settings the service applies one at a time — so a caller watching a cluster resize
/// finds `ClusterConfig.InstanceType` beside `ClusterConfig.InstanceCount` rather than
/// one entry for the whole cluster configuration.
///
/// The property names are the blocks' own modelled members
/// (`smithy_query(query="shape",shape="com.amazonaws.opensearch#ClusterConfig,com.amazonaws.opensearch#AdvancedSecurityOptions,com.amazonaws.opensearch#AIMLOptions")`),
/// less the members the service does not apply on their own — a zone-awareness
/// configuration and the per-node options travel with the cluster's topology, and
/// whether fine-grained access control is on travels with the settings it governs —
/// plus the cluster's warm-tier STORAGE and the block's master-user credentials, which
/// are properties the service applies and this model declares no reportable member for.
const LEAF_PROPERTY_BLOCKS: [(&str, &[&str]); 3] = [
    (
        OPT_AIML_OPTIONS,
        &[
            "NaturalLanguageQueryGenerationOptions",
            "S3VectorsEngine",
            "ServerlessVectorAcceleration",
        ],
    ),
    (
        OPT_ADVANCED_SECURITY_OPTIONS,
        &[
            "AnonymousAuthDisableDate",
            "AnonymousAuthEnabled",
            "IAMFederationOptions",
            "InternalUserDatabaseEnabled",
            "JWTOptions",
            "MasterUserOptions",
            "SAMLOptions",
        ],
    ),
    (
        OPT_CLUSTER_CONFIG,
        &[
            "ColdStorageOptions",
            "DedicatedMasterCount",
            "DedicatedMasterEnabled",
            "DedicatedMasterType",
            "InstanceCount",
            "InstanceType",
            "MultiAZWithStandbyEnabled",
            "WarmCount",
            "WarmEnabled",
            "WarmStorage",
            "WarmType",
            "ZoneAwarenessEnabled",
        ],
    ),
];

/// `value` split into the leaf properties the service names it by: a block it applies
/// property by property ([`LEAF_PROPERTY_BLOCKS`]) is reported property by property —
/// every property of the block, each with the value the domain holds for it — and every
/// other option is reported whole.
///
/// The service provisions a whole block, so every property of one HAS a value: a property
/// the configuration does not use (the warm instance type of a cluster with no warm tier)
/// is reported with the EMPTY value, carried here as JSON null and reported as the empty
/// text by [`property_text`].
fn leaf_values(option: &str, value: &Value) -> Vec<(String, Value)> {
    let Some((_, properties)) = LEAF_PROPERTY_BLOCKS
        .iter()
        .find(|(block, _)| *block == option)
    else {
        return vec![(option.to_string(), value.clone())];
    };
    properties
        .iter()
        .map(|property| {
            (
                format!("{option}{PROPERTY_PATH_SEPARATOR}{property}"),
                value.get(property).cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// A JSON object of the members a container holds, leaving out every member it holds
/// no value for — the document the service reports as that container's value.
///
/// Rendered in member-name order, which is the order `serde_json`'s object keeps its
/// keys in, so one container always renders the same way.
fn object(members: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name.to_string(), value)))
            .collect(),
    )
}

/// The value `domain` holds for the option `option` names, as the JSON document a
/// change reports it as — or `None` when the domain holds no value for it.
///
/// The member names are the MODELLED ones, so a property reported here names the same
/// member the configuration does.
fn option_value(domain: &Domain, option: &str) -> Option<Value> {
    let value = match option {
        OPT_ACCESS_POLICIES => Value::from(domain.access_policies.clone()?),
        OPT_ADVANCED_OPTIONS => object(
            domain
                .advanced_options
                .iter()
                .map(|(key, value)| (key.as_str(), Some(Value::from(value.clone()))))
                .collect(),
        ),
        OPT_ADVANCED_SECURITY_OPTIONS => {
            let options = domain.advanced_security_options.as_ref()?;
            // Every member the STORED shape declares. `MasterUserOptions` is not one of
            // them: it carries the master user's name and password, which the record
            // deliberately does not hold (`state::stored_advanced_security_options`), so
            // no change ever reports a credential a caller sent.
            object(vec![
                (
                    "AnonymousAuthDisableDate",
                    options
                        .anonymous_auth_disable_date
                        .map(|at| Value::from(at.secs())),
                ),
                (
                    "AnonymousAuthEnabled",
                    options.anonymous_auth_enabled.map(Value::from),
                ),
                ("Enabled", options.enabled.map(Value::from)),
                (
                    "IAMFederationOptions",
                    options.iam_federation_options.as_ref().map(|federation| {
                        object(vec![
                            ("Enabled", federation.enabled.map(Value::from)),
                            ("RolesKey", federation.roles_key.clone().map(Value::from)),
                            (
                                "SubjectKey",
                                federation.subject_key.clone().map(Value::from),
                            ),
                        ])
                    }),
                ),
                (
                    "InternalUserDatabaseEnabled",
                    options.internal_user_database_enabled.map(Value::from),
                ),
                (
                    "JWTOptions",
                    options.jwt_options.as_ref().map(|jwt| {
                        object(vec![
                            ("Enabled", jwt.enabled.map(Value::from)),
                            ("JwksUrl", jwt.jwks_url.clone().map(Value::from)),
                            ("PublicKey", jwt.public_key.clone().map(Value::from)),
                            ("RolesKey", jwt.roles_key.clone().map(Value::from)),
                            ("SubjectKey", jwt.subject_key.clone().map(Value::from)),
                        ])
                    }),
                ),
                (
                    "SAMLOptions",
                    options.saml_options.as_ref().map(|saml| {
                        object(vec![
                            ("Enabled", saml.enabled.map(Value::from)),
                            (
                                "Idp",
                                saml.idp.as_ref().map(|idp| {
                                    object(vec![
                                        ("EntityId", Some(Value::from(idp.entity_id.clone()))),
                                        (
                                            "MetadataContent",
                                            Some(Value::from(idp.metadata_content.clone())),
                                        ),
                                    ])
                                }),
                            ),
                            ("RolesKey", saml.roles_key.clone().map(Value::from)),
                            (
                                "SessionTimeoutMinutes",
                                saml.session_timeout_minutes.map(Value::from),
                            ),
                            ("SubjectKey", saml.subject_key.clone().map(Value::from)),
                        ])
                    }),
                ),
            ])
        }
        OPT_AIML_OPTIONS => {
            let options = domain.aiml_options.as_ref()?;
            object(vec![
                (
                    "NaturalLanguageQueryGenerationOptions",
                    options
                        .natural_language_query_generation_options
                        .as_ref()
                        .map(|generation| {
                            object(vec![
                                (
                                    "CurrentState",
                                    generation.current_state.clone().map(Value::from),
                                ),
                                (
                                    "DesiredState",
                                    generation.desired_state.clone().map(Value::from),
                                ),
                            ])
                        }),
                ),
                (
                    "S3VectorsEngine",
                    options
                        .s3_vectors_engine
                        .as_ref()
                        .map(|engine| object(vec![("Enabled", engine.enabled.map(Value::from))])),
                ),
                (
                    "ServerlessVectorAcceleration",
                    options
                        .serverless_vector_acceleration
                        .as_ref()
                        .map(|acceleration| {
                            object(vec![("Enabled", acceleration.enabled.map(Value::from))])
                        }),
                ),
            ])
        }
        OPT_AUTOMATED_SNAPSHOT_PAUSE_OPTIONS => {
            let options = domain.automated_snapshot_pause_options.as_ref()?;
            object(vec![
                ("Enabled", Some(Value::from(options.enabled))),
                (
                    "EndTime",
                    options.end_time.map(|at| Value::from(at.secs())),
                ),
                (
                    "StartTime",
                    options.start_time.map(|at| Value::from(at.secs())),
                ),
                ("State", options.state.clone().map(Value::from)),
            ])
        }
        OPT_AUTO_TUNE_OPTIONS => {
            let options = domain.auto_tune_options.as_ref()?;
            object(vec![
                (
                    "DesiredState",
                    options.desired_state.clone().map(Value::from),
                ),
                (
                    "MaintenanceSchedules",
                    options.maintenance_schedules.as_ref().map(|schedules| {
                        Value::Array(
                            schedules
                                .iter()
                                .map(|schedule| {
                                    object(vec![
                                        (
                                            "CronExpressionForRecurrence",
                                            schedule
                                                .cron_expression_for_recurrence
                                                .clone()
                                                .map(Value::from),
                                        ),
                                        (
                                            "Duration",
                                            schedule.duration.as_ref().map(|duration| {
                                                object(vec![
                                                    ("Unit", duration.unit.clone().map(Value::from)),
                                                    ("Value", duration.value.map(Value::from)),
                                                ])
                                            }),
                                        ),
                                        (
                                            "StartAt",
                                            schedule.start_at.map(|at| Value::from(at.secs())),
                                        ),
                                    ])
                                })
                                .collect(),
                        )
                    }),
                ),
                (
                    "RollbackOnDisable",
                    options.rollback_on_disable.clone().map(Value::from),
                ),
                (
                    "UseOffPeakWindow",
                    options.use_off_peak_window.map(Value::from),
                ),
            ])
        }
        OPT_CLUSTER_CONFIG => {
            let cluster = &domain.cluster_config;
            object(vec![
                (
                    "ColdStorageOptions",
                    cluster
                        .cold_storage_options
                        .as_ref()
                        .map(|cold| object(vec![("Enabled", Some(Value::from(cold.enabled)))])),
                ),                (
                    "DedicatedMasterCount",
                    cluster.dedicated_master_count.map(Value::from),
                ),
                (
                    "DedicatedMasterEnabled",
                    cluster.dedicated_master_enabled.map(Value::from),
                ),
                (
                    "DedicatedMasterType",
                    cluster.dedicated_master_type.clone().map(Value::from),
                ),
                ("InstanceCount", cluster.instance_count.map(Value::from)),
                (
                    "InstanceType",
                    cluster.instance_type.clone().map(Value::from),
                ),
                (
                    "MultiAZWithStandbyEnabled",
                    cluster.multi_az_with_standby_enabled.map(Value::from),
                ),
                ("WarmCount", cluster.warm_count.map(Value::from)),
                ("WarmEnabled", cluster.warm_enabled.map(Value::from)),
                ("WarmType", cluster.warm_type.clone().map(Value::from)),
                (
                    "ZoneAwarenessConfig",
                    cluster.zone_awareness_config.as_ref().map(|zones| {
                        object(vec![(
                            "AvailabilityZoneCount",
                            zones.availability_zone_count.map(Value::from),
                        )])
                    }),
                ),
                (
                    "ZoneAwarenessEnabled",
                    cluster.zone_awareness_enabled.map(Value::from),
                ),
            ])
        }
        OPT_COGNITO_OPTIONS => {
            let options = domain.cognito_options.as_ref()?;
            object(vec![
                ("Enabled", options.enabled.map(Value::from)),
                (
                    "IdentityPoolId",
                    options.identity_pool_id.clone().map(Value::from),
                ),
                ("RoleArn", options.role_arn.clone().map(Value::from)),
                ("UserPoolId", options.user_pool_id.clone().map(Value::from)),
            ])
        }
        OPT_DEPLOYMENT_STRATEGY_OPTIONS => {
            let options = domain.deployment_strategy_options.as_ref()?;
            object(vec![(
                "DeploymentStrategy",
                Some(Value::from(options.deployment_strategy.clone())),
            )])
        }
        OPT_DOMAIN_ENDPOINT_OPTIONS => {
            let options = domain.domain_endpoint_options.as_ref()?;
            object(vec![
                (
                    "CustomEndpoint",
                    options.custom_endpoint.clone().map(Value::from),
                ),
                (
                    "CustomEndpointCertificateArn",
                    options
                        .custom_endpoint_certificate_arn
                        .clone()
                        .map(Value::from),
                ),
                (
                    "CustomEndpointEnabled",
                    options.custom_endpoint_enabled.map(Value::from),
                ),
                ("EnforceHTTPS", options.enforce_https.map(Value::from)),
                (
                    "TLSSecurityPolicy",
                    options.tls_security_policy.clone().map(Value::from),
                ),
            ])
        }
        OPT_EBS_OPTIONS => {
            let options = domain.ebs_options.as_ref()?;
            object(vec![
                ("EBSEnabled", options.ebs_enabled.map(Value::from)),
                ("Iops", options.iops.map(Value::from)),
                ("Throughput", options.throughput.map(Value::from)),
                ("VolumeSize", options.volume_size.map(Value::from)),
                ("VolumeType", options.volume_type.clone().map(Value::from)),
            ])
        }
        OPT_ENCRYPTION_AT_REST_OPTIONS => {
            let options = domain.encryption_at_rest_options.as_ref()?;
            object(vec![
                ("Enabled", options.enabled.map(Value::from)),
                (
                    "EncryptionMode",
                    options.encryption_mode.clone().map(Value::from),
                ),
                ("KmsKeyId", options.kms_key_id.clone().map(Value::from)),
            ])
        }
        OPT_ENGINE_MODE => Value::from(domain.engine_mode.clone()),
        OPT_ENGINE_VERSION => Value::from(domain.engine_version.clone()),
        OPT_IDENTITY_CENTER_OPTIONS => {
            let options = domain.identity_center_options.as_ref()?;
            object(vec![
                (
                    "EnabledAPIAccess",
                    options.enabled_api_access.map(Value::from),
                ),
                (
                    "IdentityCenterApplicationARN",
                    options
                        .identity_center_application_arn
                        .clone()
                        .map(Value::from),
                ),
                (
                    "IdentityCenterInstanceARN",
                    options.identity_center_instance_arn.clone().map(Value::from),
                ),
                (
                    "IdentityCenterInstanceRegion",
                    options
                        .identity_center_instance_region
                        .clone()
                        .map(Value::from),
                ),
                (
                    "IdentityStoreId",
                    options.identity_store_id.clone().map(Value::from),
                ),
                ("RolesKey", options.roles_key.clone().map(Value::from)),
                ("SubjectKey", options.subject_key.clone().map(Value::from)),
            ])
        }
        OPT_IP_ADDRESS_TYPE => Value::from(domain.ip_address_type.clone()?),
        OPT_LOG_PUBLISHING_OPTIONS => object(
            domain
                .log_publishing_options
                .iter()
                .map(|(log_type, option)| {
                    (
                        log_type.as_str(),
                        Some(object(vec![
                            (
                                "CloudWatchLogsLogGroupArn",
                                option
                                    .cloud_watch_logs_log_group_arn
                                    .clone()
                                    .map(Value::from),
                            ),
                            ("Enabled", option.enabled.map(Value::from)),
                        ])),
                    )
                })
                .collect(),
        ),
        OPT_NODE_TO_NODE_ENCRYPTION_OPTIONS => {
            let options = domain.node_to_node_encryption_options.as_ref()?;
            object(vec![("Enabled", options.enabled.map(Value::from))])
        }
        OPT_OFF_PEAK_WINDOW_OPTIONS => {
            let options = domain.off_peak_window_options.as_ref()?;
            object(vec![
                ("Enabled", options.enabled.map(Value::from)),
                (
                    "OffPeakWindow",
                    options.off_peak_window.as_ref().map(|window| {
                        object(vec![(
                            "WindowStartTime",
                            window.window_start_time.as_ref().map(|start| {
                                object(vec![
                                    ("Hours", Some(Value::from(start.hours))),
                                    ("Minutes", Some(Value::from(start.minutes))),
                                ])
                            }),
                        )])
                    }),
                ),
            ])
        }
        OPT_SNAPSHOT_OPTIONS => {
            let options = domain.snapshot_options.as_ref()?;
            object(vec![(
                "AutomatedSnapshotStartHour",
                options.automated_snapshot_start_hour.map(Value::from),
            )])
        }
        OPT_SOFTWARE_UPDATE_OPTIONS => {
            let options = domain.software_update_options.as_ref()?;
            object(vec![
                (
                    "AutoSoftwareUpdateEnabled",
                    options.auto_software_update_enabled.map(Value::from),
                ),
                (
                    "UseLatestServiceSoftwareForBlueGreen",
                    options
                        .use_latest_service_software_for_blue_green
                        .map(Value::from),
                ),
            ])
        }
        OPT_USE_CASE => Value::from(domain.use_case.clone()),
        // The tag set is not an option container: the service applies it as the list of
        // `Tag` the create carried, which is the document a caller reads back, so it is
        // reported as that list — empty for a domain created with no tag.
        OPT_TAGS => Value::Array(
            domain
                .tags
                .iter()
                .map(|(key, value)| {
                    object(vec![
                        ("Key", Some(Value::from(key.clone()))),
                        ("Value", Some(Value::from(value.clone()))),
                    ])
                })
                .collect(),
        ),
        OPT_VPC_OPTIONS => {
            let options = domain.vpc_options.as_ref()?;
            object(vec![
                ("EgressEnabled", options.egress_enabled.map(Value::from)),
                (
                    "SecurityGroupIds",
                    options.security_group_ids.clone().map(Value::from),
                ),
                ("SubnetIds", options.subnet_ids.clone().map(Value::from)),
            ])
        }
        // Every option a domain's configuration carries is named above; a name that is
        // not one of them names no option there is a value to report for.
        _ => return None,
    };
    Some(value)
}
