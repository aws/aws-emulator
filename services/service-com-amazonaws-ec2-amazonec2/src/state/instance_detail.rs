// SPDX-License-Identifier: CC0-1.0

//! The FIXED detail every EC2 instance reports, and the settings a LAUNCH chose for it.
//!
//! `DescribeInstances` (and `RunInstances`, which answers with the same `Instance` shape)
//! reports a large set of members for every instance, not only the ones a request named:
//! the machine facts of the AMI it was launched from (`Instance$Architecture`,
//! `$RootDeviceName`, `$RootDeviceType`, `$VirtualizationType`, `$Hypervisor`,
//! `$EnaSupport`, `$BootMode`, `$PlatformDetails`, `$UsageOperation`), the CPU layout of
//! its instance type (`$CpuOptions`), the volumes it holds (`$BlockDeviceMappings`), the
//! place it runs in (`$Placement`) and the option sets a launch may configure
//! (`$MetadataOptions`, `$EnclaveOptions`, `$HibernationOptions`,
//! `$CapacityReservationSpecification`, `$MaintenanceOptions`,
//! `$NetworkPerformanceOptions`, `$PrivateDnsNameOptions`, `$Operator`).
//!
//! Every one of them is DERIVED here from what the store holds — the instance's own
//! record, the AMI, the instance type, the subnet and the volume store — so the instance
//! report, `DescribeImages`, `DescribeInstanceTypes`, `DescribeSubnets` and
//! `DescribeVolumes` cannot disagree about one instance, and so a member is never a
//! literal written into one response.
//!
//! The settings a launch CHOSE are the only part that cannot be re-derived, so they are
//! stored on the instance ([`InstanceDetail`], one field of
//! [`super::InstanceRecord`]) in the catalog form the response reports them in — never as
//! the request's own text.

use std::time::Instant;

use crate::ops::types::{
    CAPACITY_RESERVATION_PREFERENCE_VALUES, CapacityReservationSpecificationResponse, CpuOptions,
    EbsInstanceBlockDevice, EnclaveOptions, EnclaveOptionsRequest, HOSTNAME_TYPE_VALUES,
    HTTP_TOKENS_STATE_VALUES, HibernationOptions, INSTANCE_AUTO_RECOVERY_STATE_VALUES,
    INSTANCE_BANDWIDTH_WEIGHTING_VALUES, INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
    INSTANCE_METADATA_PROTOCOL_STATE_VALUES, INSTANCE_METADATA_TAGS_STATE_VALUES, Instance,
    InstanceBlockDeviceMapping, InstanceMaintenanceOptions, InstanceMaintenanceOptionsRequest,
    InstanceMetadataOptionsRequest, InstanceMetadataOptionsResponse,
    InstanceNetworkPerformanceOptions, InstanceNetworkPerformanceOptionsRequest, Placement,
    PrivateDnsNameOptionsRequest, PrivateDnsNameOptionsResponse, RequestLaunchTemplateData,
    SHUTDOWN_BEHAVIOR_VALUES, TENANCY_VALUES,
};
use crate::scaffolded::Scaffolded;

use super::{
    AccountInstanceMetadataDefaults, BOOT_MODE_UEFI_PREFERRED, ComAmazonawsEc2Amazonec2State,
    DEFAULT_DISABLE_API_STOP,
    DEFAULT_SHUTDOWN_BEHAVIOR, HOSTNAME_TYPE_IP_NAME,
    IMAGE_HYPERVISOR_XEN, IMAGE_ROOT_DEVICE_TYPE_EBS, IMAGE_VIRTUALIZATION_TYPE_HVM,
    IMDS_SUPPORT_V2, ImageSpec, InstanceRecord, InstanceStateName, MAX_METADATA_HOP_LIMIT,
    MIN_METADATA_HOP_LIMIT,
    TRANSITION_DELAY, catalog_member, due_instant, echoed_resource_id, image_creation_time,
    instance_type_record, invalid_parameter_value, region_of_zone_name, supported_boot_modes,
    unmanaged_operator,
    zone_id_for_zone_name,
};

/// `InstanceMetadataOptionsState`'s `applied`: "the metadata options have been
/// successfully applied on the instance"
/// (`InstanceMetadataOptionsResponse$State`).
pub const METADATA_OPTIONS_STATE_APPLIED: &str = "applied";

/// `InstanceMetadataOptionsState`'s `pending`: "the metadata options are being updated and
/// the instance is not ready to process metadata traffic with the new selection"
/// (`InstanceMetadataOptionsResponse$State`) — the state a launch's options are in until
/// the clock has carried them onto the instance.
pub const METADATA_OPTIONS_STATE_PENDING: &str = "pending";

/// `HttpTokensState`'s `optional`: "IMDSv2 is optional, which means that you can use either
/// IMDSv2 or IMDSv1" (`InstanceMetadataOptionsRequest$HttpTokens`).
pub const HTTP_TOKENS_OPTIONAL: &str = "optional";

/// `HttpTokensState`'s `required`: "IMDSv2 is required, which means that IMDSv1 is disabled,
/// and you must use IMDSv2" (`InstanceMetadataOptionsRequest$HttpTokens`) — the default for
/// an instance whose AMI declares `ImdsSupport` `v2.0` and whose account states no
/// preference, which is the rule [`InstanceDetail::launched`] applies.
pub const HTTP_TOKENS_REQUIRED: &str = "required";

/// The hop limit an instance launched from an IMDSv2-only AMI runs with: "instances
/// launched from this AMI will have `HttpTokens` automatically set to `required` and
/// `HttpPutResponseHopLimit` set to `2`" (`RegisterImageRequest$ImdsSupport`).
pub const IMDS_V2_METADATA_HOP_LIMIT: i32 = 2;

/// `InstanceMetadataEndpointState`'s `enabled`, the documented default: "Default:
/// `enabled`" (`InstanceMetadataOptionsRequest$HttpEndpoint`).
pub const METADATA_ENDPOINT_ENABLED: &str = "enabled";

/// `InstanceMetadataProtocolState`'s `disabled`, the documented default of the IPv6
/// endpoint: "Default: `disabled`" (`InstanceMetadataOptionsRequest$HttpProtocolIpv6`).
pub const METADATA_PROTOCOL_IPV6_DISABLED: &str = "disabled";

/// `InstanceMetadataTagsState`'s `disabled`, the documented default of tag access from the
/// metadata: "Default: `disabled`"
/// (`InstanceMetadataOptionsRequest$InstanceMetadataTags`).
pub const METADATA_TAGS_DISABLED: &str = "disabled";

/// The hop limit an instance whose launch named none runs with: one hop, the bottom of the
/// range the member publishes ("Possible values: Integers from 1 to 64",
/// `InstanceMetadataOptionsRequest$HttpPutResponseHopLimit`, carried here as
/// [`MIN_METADATA_HOP_LIMIT`]) — the most restrictive value, which is what AWS gives an
/// instance nothing configured a hop limit for.
pub const DEFAULT_METADATA_HOP_LIMIT: i32 = MIN_METADATA_HOP_LIMIT;

/// `Tenancy`'s `default`: shared hardware, where "an instance with a tenancy of `dedicated`
/// runs on single-tenant hardware" (`Placement$Tenancy`).
pub const TENANCY_DEFAULT: &str = "default";

/// `CapacityReservationPreference`'s `open`: "the instance can run in any `open` Capacity
/// Reservation that has matching attributes" — and the preference of a launch that named
/// none, "the instance's Capacity Reservation preference defaults to open"
/// (`RunInstancesRequest$CapacityReservationSpecification`).
pub const CAPACITY_RESERVATION_PREFERENCE_OPEN: &str = "open";

/// `InstanceAutoRecoveryState`'s `default`: Amazon EC2's own recovery behaviour, which is
/// what an instance whose launch asked for nothing else has
/// (`InstanceMaintenanceOptions$AutoRecovery`).
pub const AUTO_RECOVERY_DEFAULT: &str = "default";

/// `InstanceRebootMigrationState`'s `default`: "Amazon EC2 attempts to migrate the instance
/// to new hardware (reboot migration)" for a scheduled `system-reboot` event, which is the
/// behaviour of an instance nothing has disabled it for
/// (`InstanceMaintenanceOptions$RebootMigration`).
pub const REBOOT_MIGRATION_DEFAULT: &str = "default";

/// `InstanceBandwidthWeighting`'s `default`: the baseline bandwidth split an instance runs
/// with until a launch boosts one side of it
/// (`InstanceNetworkPerformanceOptions$BandwidthWeighting`).
pub const BANDWIDTH_WEIGHTING_DEFAULT: &str = "default";

/// `InstanceBootModeValues`'s `legacy-bios`: the mode an instance boots in when its AMI
/// does not require UEFI (`Instance$CurrentInstanceBootMode`).
pub const INSTANCE_BOOT_MODE_LEGACY_BIOS: &str = "legacy-bios";

/// `InstanceBootModeValues`'s `uefi`, which is also `BootModeValues`' `uefi` — an AMI that
/// names this boot mode boots the instance in it (`Instance$BootMode` /
/// `$CurrentInstanceBootMode`).
pub const INSTANCE_BOOT_MODE_UEFI: &str = "uefi";

/// The settings a LAUNCH chose for one instance, in the catalog form the response reports
/// them in.
///
/// Everything else an instance reports is re-derived from the store on every read (see the
/// module docs), so this holds only what no other record can answer: the option sets
/// `RunInstances` accepts. Each enum member is a `&'static str` taken from the generated
/// catalog of its own modelled enum (`super::catalog_member`), so a request's text is never
/// stored, and the three `private_dns_*` members are `Option`s because a launch that named
/// none leaves the instance following its SUBNET's own on-launch options
/// (`Subnet$PrivateDnsNameOptionsOnLaunch`) rather than a second copy of them.
#[derive(Debug, Clone)]
pub struct InstanceDetail {
    /// `InstanceMetadataOptionsResponse$HttpTokens`.
    pub http_tokens: &'static str,
    /// `InstanceMetadataOptionsResponse$HttpPutResponseHopLimit`.
    pub http_put_response_hop_limit: i32,
    /// `InstanceMetadataOptionsResponse$HttpEndpoint`.
    pub http_endpoint: &'static str,
    /// `InstanceMetadataOptionsResponse$HttpProtocolIpv6`.
    pub http_protocol_ipv6: &'static str,
    /// `InstanceMetadataOptionsResponse$InstanceMetadataTags`.
    pub instance_metadata_tags: &'static str,
    /// When the instance's metadata options reach `applied`: the options are `pending`
    /// until this instant, "being updated and the instance is not ready to process
    /// metadata traffic with the new selection"
    /// (`InstanceMetadataOptionsResponse$State`), so a caller that reads straight after the
    /// call that set them still sees that transient state. The due instant is stored;
    /// nothing sleeps for it.
    pub metadata_applied_at: Instant,
    /// `EnclaveOptions$Enabled`.
    pub enclave_enabled: bool,
    /// "Indicates whether stop protection is enabled for the instance"
    /// (`InstanceAttribute$DisableApiStop`), which a launch asks for through
    /// `RunInstancesRequest$DisableApiStop` — off for a launch that asks for nothing
    /// ([`DEFAULT_DISABLE_API_STOP`]).
    pub disable_api_stop: bool,
    /// "Indicates whether an instance stops or terminates when you initiate shutdown from
    /// the instance" (`InstanceAttribute$InstanceInitiatedShutdownBehavior`): the
    /// `ShutdownBehavior` member the launch chose, or [`DEFAULT_SHUTDOWN_BEHAVIOR`] when it
    /// chose none.
    pub instance_initiated_shutdown_behavior: &'static str,
    /// `CapacityReservationSpecificationResponse$CapacityReservationPreference`.
    pub capacity_reservation_preference: &'static str,
    /// `InstanceMaintenanceOptions$AutoRecovery`.
    pub auto_recovery: &'static str,
    /// `InstanceNetworkPerformanceOptions$BandwidthWeighting`.
    pub bandwidth_weighting: &'static str,
    /// `Placement$Tenancy`.
    pub tenancy: &'static str,
    /// The NAME of the placement group the launch put the instance in
    /// (`Placement$GroupName`), or `None` for an instance in none. Stored as the caller's
    /// own text because that is what the member is — a name the account chose — and charged
    /// against the store's byte budget by [`super::instance_payload_bytes`] like every other
    /// caller-chosen string of the record; the launch has already resolved it against the
    /// placement group store, so a name no group of the account has is never stored.
    pub placement_group_name: Option<String>,
    /// `PrivateDnsNameOptionsResponse$HostnameType`, or `None` to follow the subnet's.
    pub private_dns_hostname_type: Option<&'static str>,
    /// `PrivateDnsNameOptionsResponse$EnableResourceNameDnsARecord`, or `None` to follow
    /// the subnet's.
    pub private_dns_a_record: Option<bool>,
    /// `PrivateDnsNameOptionsResponse$EnableResourceNameDnsAAAARecord`, or `None` to follow
    /// the subnet's.
    pub private_dns_aaaa_record: Option<bool>,
}

impl Default for InstanceDetail {
    /// The detail of an instance whose launch configured none of it: every documented
    /// default, with the metadata options due to be applied on the same span as every other
    /// clock-owed move of this crate ([`TRANSITION_DELAY`]).
    fn default() -> Self {
        Self {
            http_tokens: HTTP_TOKENS_OPTIONAL,
            http_put_response_hop_limit: DEFAULT_METADATA_HOP_LIMIT,
            http_endpoint: METADATA_ENDPOINT_ENABLED,
            http_protocol_ipv6: METADATA_PROTOCOL_IPV6_DISABLED,
            instance_metadata_tags: METADATA_TAGS_DISABLED,
            metadata_applied_at: due_instant(TRANSITION_DELAY),
            enclave_enabled: false,
            disable_api_stop: DEFAULT_DISABLE_API_STOP,
            instance_initiated_shutdown_behavior: DEFAULT_SHUTDOWN_BEHAVIOR,
            capacity_reservation_preference: CAPACITY_RESERVATION_PREFERENCE_OPEN,
            auto_recovery: AUTO_RECOVERY_DEFAULT,
            bandwidth_weighting: BANDWIDTH_WEIGHTING_DEFAULT,
            tenancy: TENANCY_DEFAULT,
            placement_group_name: None,
            private_dns_hostname_type: None,
            private_dns_a_record: None,
            private_dns_aaaa_record: None,
        }
    }
}

impl InstanceDetail {
    /// The detail one LAUNCH chose: every option set the request named, resolved to a
    /// member of that member's own modelled catalog, with the ACCOUNT's IMDS defaults
    /// ([`super::AccountInstanceMetadataDefaults`]) and then the documented per-member
    /// defaults standing in for what the request left out.
    ///
    /// The order of precedence for the metadata options is the documented one: the
    /// request's own value, then the account-level default, then the member's documented
    /// default — and for `HttpTokens`, "if the value of `ImdsSupport` for the Amazon Machine
    /// Image (AMI) for your instance is `v2.0` and the account level default is set to
    /// `no-preference`, the default is `required`"
    /// (`InstanceMetadataOptionsRequest$HttpTokens`), which is why the AMI's own
    /// `ImdsSupport` is part of the question.
    ///
    /// A word outside its catalog, or a hop limit outside the published range, refuses the
    /// launch with `InvalidParameterValue` rather than being stored: the modelled enum
    /// constraint already refuses the former before a handler runs, so that refusal is
    /// reached only if the boundary ever stops covering a member.
    pub fn launched<E>(spec: InstanceDetailSpec<'_>) -> Result<Self, Scaffolded<E>> {
        let defaults = spec.account_metadata_defaults;
        let metadata = spec.metadata_options;
        // "If you set `ImdsSupport` to `v2.0` … instances launched from this AMI will have
        // `HttpTokens` automatically set to `required` and `HttpPutResponseHopLimit` set to
        // `2`" (`RegisterImageRequest$ImdsSupport`): one question about the AMI decides both
        // of those defaults below.
        let imds_v2_only = spec.image_imds_support == Some(IMDS_SUPPORT_V2);
        let requested =
            |pick: fn(&InstanceMetadataOptionsRequest) -> Option<&String>| -> Option<&str> {
                metadata.and_then(pick).map(String::as_str)
            };
        let http_tokens = requested_word(
            "HttpTokens",
            requested(|options| options.http_tokens.as_ref()),
            HTTP_TOKENS_STATE_VALUES,
        )?
        .or(defaults.http_tokens)
        .unwrap_or(if imds_v2_only {
            HTTP_TOKENS_REQUIRED
        } else {
            HTTP_TOKENS_OPTIONAL
        });
        let http_endpoint = requested_word(
            "HttpEndpoint",
            requested(|options| options.http_endpoint.as_ref()),
            INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
        )?
        .or(defaults.http_endpoint)
        .unwrap_or(METADATA_ENDPOINT_ENABLED);
        let http_protocol_ipv6 = requested_word(
            "HttpProtocolIpv6",
            requested(|options| options.http_protocol_ipv6.as_ref()),
            INSTANCE_METADATA_PROTOCOL_STATE_VALUES,
        )?
        .unwrap_or(METADATA_PROTOCOL_IPV6_DISABLED);
        let instance_metadata_tags = requested_word(
            "InstanceMetadataTags",
            requested(|options| options.instance_metadata_tags.as_ref()),
            INSTANCE_METADATA_TAGS_STATE_VALUES,
        )?
        .or(defaults.instance_metadata_tags)
        .unwrap_or(METADATA_TAGS_DISABLED);
        let http_put_response_hop_limit = match requested_metadata_hop_limit(
            metadata.and_then(|options| options.http_put_response_hop_limit),
            launch_hop_limit_out_of_range,
        )? {
            Some(hop_limit) => hop_limit,
            None => defaults
                .http_put_response_hop_limit
                .unwrap_or(if imds_v2_only {
                    IMDS_V2_METADATA_HOP_LIMIT
                } else {
                    DEFAULT_METADATA_HOP_LIMIT
                }),
        };
        Ok(Self {
            http_tokens,
            http_put_response_hop_limit,
            http_endpoint,
            http_protocol_ipv6,
            instance_metadata_tags,
            metadata_applied_at: due_instant(TRANSITION_DELAY),
            enclave_enabled: spec
                .enclave_options
                .and_then(|options| options.enabled)
                .unwrap_or(false),
            // "Indicates whether an instance is enabled for stop protection"
            // (`RunInstancesRequest$DisableApiStop`): a protection a launch turns ON, so a
            // launch that asks for nothing leaves the instance unprotected.
            disable_api_stop: spec.disable_api_stop.unwrap_or(DEFAULT_DISABLE_API_STOP),
            // "Indicates whether an instance stops or terminates when you initiate shutdown
            // from the instance … Default: `stop`"
            // (`RunInstancesRequest$InstanceInitiatedShutdownBehavior`), resolved to a member
            // of its own modelled catalog like every other word of this record.
            instance_initiated_shutdown_behavior: requested_word(
                "InstanceInitiatedShutdownBehavior",
                spec.instance_initiated_shutdown_behavior,
                SHUTDOWN_BEHAVIOR_VALUES,
            )?
            .unwrap_or(DEFAULT_SHUTDOWN_BEHAVIOR),
            capacity_reservation_preference: requested_word(
                "CapacityReservationPreference",
                spec.capacity_reservation_preference,
                CAPACITY_RESERVATION_PREFERENCE_VALUES,
            )?
            .unwrap_or(CAPACITY_RESERVATION_PREFERENCE_OPEN),
            auto_recovery: requested_word(
                "AutoRecovery",
                spec.maintenance_options
                    .and_then(|options| options.auto_recovery.as_deref()),
                INSTANCE_AUTO_RECOVERY_STATE_VALUES,
            )?
            .unwrap_or(AUTO_RECOVERY_DEFAULT),
            bandwidth_weighting: requested_word(
                "BandwidthWeighting",
                spec.network_performance_options
                    .and_then(|options| options.bandwidth_weighting.as_deref()),
                INSTANCE_BANDWIDTH_WEIGHTING_VALUES,
            )?
            .unwrap_or(BANDWIDTH_WEIGHTING_DEFAULT),
            tenancy: requested_word("Tenancy", spec.tenancy, TENANCY_VALUES)?
                .unwrap_or(TENANCY_DEFAULT),
            placement_group_name: spec.placement_group_name.map(str::to_string),
            private_dns_hostname_type: requested_word(
                "HostnameType",
                spec.private_dns_name_options
                    .and_then(|options| options.hostname_type.as_deref()),
                HOSTNAME_TYPE_VALUES,
            )?,
            private_dns_a_record: spec
                .private_dns_name_options
                .and_then(|options| options.enable_resource_name_dns_a_record),
            private_dns_aaaa_record: spec
                .private_dns_name_options
                .and_then(|options| options.enable_resource_name_dns_aaaa_record),
        })
    }

    /// The metadata options' state at `now`: `pending` until the move the clock owes them
    /// falls due, `applied` from that instant on.
    pub fn metadata_options_state_at(&self, now: Instant) -> &'static str {
        if now >= self.metadata_applied_at {
            METADATA_OPTIONS_STATE_APPLIED
        } else {
            METADATA_OPTIONS_STATE_PENDING
        }
    }

    /// These metadata options as the modelled response shape reports them at `now`.
    ///
    /// The ONE reading of the stored options, so the instance report and the answer a
    /// modify of them gives cannot disagree about one instance: both the `Instance` view
    /// ([`ComAmazonawsEc2Amazonec2State::report_instance_detail`]) and
    /// `ModifyInstanceMetadataOptions` report them from here.
    pub fn metadata_options_at(&self, now: Instant) -> InstanceMetadataOptionsResponse {
        InstanceMetadataOptionsResponse {
            state: Some(self.metadata_options_state_at(now).to_string()),
            http_tokens: Some(self.http_tokens.to_string()),
            http_put_response_hop_limit: Some(self.http_put_response_hop_limit),
            http_endpoint: Some(self.http_endpoint.to_string()),
            http_protocol_ipv6: Some(self.http_protocol_ipv6.to_string()),
            instance_metadata_tags: Some(self.instance_metadata_tags.to_string()),
        }
    }

    /// Start the move the clock owes a CHANGE of these options: they are `pending`, "being
    /// updated and the instance is not ready to process metadata traffic with the new
    /// selection" (`InstanceMetadataOptionsResponse$State`), until the span every
    /// clock-owed move of this crate takes ([`TRANSITION_DELAY`]) has passed, and `applied`
    /// from that instant on. The due instant is stored; nothing sleeps for it.
    pub fn metadata_options_changed(&mut self) {
        self.metadata_applied_at = due_instant(TRANSITION_DELAY);
    }
}

/// The `HttpPutResponseHopLimit` a request asks for, or `None` when it asked for none —
/// and the refusal `out_of_range` words for a hop count outside the range the member
/// publishes.
///
/// "Possible values: Integers from `1` to `64`"
/// (`InstanceMetadataOptionsRequest$HttpPutResponseHopLimit`, carried here as
/// [`MIN_METADATA_HOP_LIMIT`] and [`MAX_METADATA_HOP_LIMIT`]): the member targets a plain
/// `Integer` with no modelled `@range`, so this is the only place that range is enforced,
/// and it is shared by every operation that accepts the member — a launch
/// ([`InstanceDetail::launched`]) and a modify of one instance's options alike — so they
/// cannot state different limits.
///
/// The SENTENCE is the caller's, the way [`require_instance_record_worded`]'s is: EC2 does
/// not word this refusal the same way on every operation that carries the member, and which
/// wording an operation answers is a fact of that operation, recorded by its caller. The
/// RULE is one, so it lives here once.
pub fn requested_metadata_hop_limit<E>(
    requested: Option<i32>,
    out_of_range: impl Fn(i32) -> Scaffolded<E>,
) -> Result<Option<i32>, Scaffolded<E>> {
    match requested {
        Some(hop_limit)
            if !(MIN_METADATA_HOP_LIMIT..=MAX_METADATA_HOP_LIMIT).contains(&hop_limit) =>
        {
            Err(out_of_range(hop_limit))
        }
        requested => Ok(requested),
    }
}

/// The hop-limit refusal a LAUNCH answers: `InvalidParameterValue` in the wording EC2's
/// parameter layer gives a member's invalid value, naming the value and the range.
pub fn launch_hop_limit_out_of_range<E>(hop_limit: i32) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value ({hop_limit}) for parameter HttpPutResponseHopLimit is invalid. \
         Expecting {MIN_METADATA_HOP_LIMIT}-{MAX_METADATA_HOP_LIMIT}."
    ))
}

/// What one launch asks the instance detail to be, as the handler serving that launch reads
/// it off the request and the store.
///
/// A borrowed view rather than an owned record: every member is read once, resolved to a
/// catalog word by [`InstanceDetail::launched`], and never stored as the request's own text.
#[derive(Debug, Clone, Default)]
pub struct InstanceDetailSpec<'request> {
    /// `RunInstancesRequest$MetadataOptions`.
    pub metadata_options: Option<&'request InstanceMetadataOptionsRequest>,
    /// `RunInstancesRequest$EnclaveOptions`.
    pub enclave_options: Option<&'request EnclaveOptionsRequest>,
    /// `RunInstancesRequest$DisableApiStop`.
    pub disable_api_stop: Option<bool>,
    /// `RunInstancesRequest$InstanceInitiatedShutdownBehavior`, as the launch named it.
    pub instance_initiated_shutdown_behavior: Option<&'request str>,
    /// `CapacityReservationSpecification$CapacityReservationPreference`, as the launch
    /// named it.
    pub capacity_reservation_preference: Option<&'request str>,
    /// `RunInstancesRequest$MaintenanceOptions`.
    pub maintenance_options: Option<&'request InstanceMaintenanceOptionsRequest>,
    /// `RunInstancesRequest$NetworkPerformanceOptions`.
    pub network_performance_options: Option<&'request InstanceNetworkPerformanceOptionsRequest>,
    /// `RunInstancesRequest$PrivateDnsNameOptions`.
    pub private_dns_name_options: Option<&'request PrivateDnsNameOptionsRequest>,
    /// `Placement$Tenancy`, as the launch named it.
    pub tenancy: Option<&'request str>,
    /// `Placement$GroupName` — the placement group the launch named, already resolved
    /// against the placement group store by the handler that serves the launch.
    pub placement_group_name: Option<&'request str>,
    /// The account's own IMDS defaults, which stand in for every metadata option the
    /// request left out.
    pub account_metadata_defaults: AccountInstanceMetadataDefaults,
    /// `Image$ImdsSupport` of the AMI the instance is launched from, which decides the
    /// `HttpTokens` default where neither the request nor the account states one.
    pub image_imds_support: Option<&'request str>,
}

/// The member of `catalog` that `requested` names, or `None` when the request named
/// nothing — and `InvalidParameterValue` for a word the catalog does not hold, worded the
/// way EC2 words a member's invalid value, with the caller's own value bounded by the one
/// echo every refusal of this crate quotes a request value through
/// ([`echoed_resource_id`]).
///
/// Shared with the handlers that accept one of those words on a MODIFY of what a launch
/// chose, so a value outside a member's catalog is refused the same way wherever it
/// arrives.
pub fn requested_word<E>(
    member: &str,
    requested: Option<&str>,
    catalog: &'static [&'static str],
) -> Result<Option<&'static str>, Scaffolded<E>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    match catalog_member(catalog, requested) {
        Some(word) => Ok(Some(word)),
        None => Err(invalid_parameter_value(format!(
            "Value ({}) for parameter {member} is invalid. Valid values are {}",
            echoed_resource_id(requested),
            catalog.join(", ")
        ))),
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The detail an instance launched FROM a launch template version carries: every option
    /// set that version's launch data declares, resolved exactly as a launch naming the
    /// member itself resolves it.
    ///
    /// "The launch template contains the parameters to launch an instance"
    /// (`LaunchTemplateVersion$LaunchTemplateData`), so an instance launched from a version
    /// reports the option sets that version declares — its metadata options, enclave, stop
    /// protection, shutdown behaviour, Capacity Reservation preference, maintenance and
    /// network performance options, instance hostname options, tenancy and placement group.
    /// The version's own values stand where a launch would carry its own, and the account's
    /// IMDS defaults and the documented per-member defaults stand in for what the version
    /// leaves out ([`InstanceDetail::launched`]), which is the SAME resolution a launch that
    /// names a template gets — so an instance a consumer of the template mints (an Auto
    /// Scaling group's capacity change) and one `RunInstances` mints from the same version
    /// cannot report different options.
    ///
    /// `image_id` is the AMI the launch SETTLED on, which decides the `HttpTokens` default
    /// where neither the version nor the account states one: a consumer may launch the
    /// version with another image (a mixed instances policy's override names one), so the
    /// image is passed in rather than read off the version.
    ///
    /// A placement group the account holds none of is NOT carried: the instance would
    /// otherwise report a group no describe can report. Every word is resolved to a member of
    /// its own modelled catalog, and a version carrying one outside it — which the modelled
    /// enums keep out of the store — contributes none of them, the instance then reporting
    /// the documented defaults rather than the request's text.
    pub fn launch_template_instance_detail(
        &self,
        data: &RequestLaunchTemplateData,
        image_id: Option<&str>,
    ) -> InstanceDetail {
        // The template spells each option set in its own `LaunchTemplate*Request` shape;
        // the member a launch carries is the shape the resolution below reads, so each one
        // is read across member by member, exactly as `RunInstances` overlays a template
        // onto the launch it serves.
        let metadata_options =
            data.metadata_options
                .as_ref()
                .map(|options| InstanceMetadataOptionsRequest {
                    http_tokens: options.http_tokens.clone(),
                    http_put_response_hop_limit: options.http_put_response_hop_limit,
                    http_endpoint: options.http_endpoint.clone(),
                    http_protocol_ipv6: options.http_protocol_ipv6.clone(),
                    instance_metadata_tags: options.instance_metadata_tags.clone(),
                });
        let enclave_options = data
            .enclave_options
            .as_ref()
            .map(|options| EnclaveOptionsRequest {
                enabled: options.enabled,
            });
        let maintenance_options =
            data.maintenance_options
                .as_ref()
                .map(|options| InstanceMaintenanceOptionsRequest {
                    auto_recovery: options.auto_recovery.clone(),
                });
        let network_performance_options = data.network_performance_options.as_ref().map(|options| {
            InstanceNetworkPerformanceOptionsRequest {
                bandwidth_weighting: options.bandwidth_weighting.clone(),
            }
        });
        let private_dns_name_options =
            data.private_dns_name_options
                .as_ref()
                .map(|options| PrivateDnsNameOptionsRequest {
                    hostname_type: options.hostname_type.clone(),
                    enable_resource_name_dns_a_record: options.enable_resource_name_dns_a_record,
                    enable_resource_name_dns_aaaa_record: options
                        .enable_resource_name_dns_aaaa_record,
                });
        let image_imds_support = self.image_imds_support(image_id);
        let placement_group_name = data
            .placement
            .as_ref()
            .and_then(|placement| placement.group_name.as_deref())
            .filter(|group_name| !group_name.is_empty())
            .filter(|group_name| self.placement_group_id_for_name(group_name).is_some());
        InstanceDetail::launched::<()>(InstanceDetailSpec {
            metadata_options: metadata_options.as_ref(),
            enclave_options: enclave_options.as_ref(),
            disable_api_stop: data.disable_api_stop,
            instance_initiated_shutdown_behavior: data
                .instance_initiated_shutdown_behavior
                .as_deref(),
            capacity_reservation_preference: data
                .capacity_reservation_specification
                .as_ref()
                .and_then(|specification| specification.capacity_reservation_preference.as_deref()),
            maintenance_options: maintenance_options.as_ref(),
            network_performance_options: network_performance_options.as_ref(),
            private_dns_name_options: private_dns_name_options.as_ref(),
            tenancy: data
                .placement
                .as_ref()
                .and_then(|placement| placement.tenancy.as_deref()),
            placement_group_name,
            account_metadata_defaults: self.instance_metadata_defaults(),
            image_imds_support: image_imds_support.as_deref(),
        })
        .unwrap_or_default()
    }

    /// The block device mappings the instance `instance_id` holds: one entry per EBS volume
    /// of this account attached to it, at the device it is attached at, in device order.
    ///
    /// "Any block device mapping entries for the instance"
    /// (`Instance$BlockDeviceMappings`): read off the volume store — the one place an
    /// attachment is recorded, written by the launch that mints the root volume and by
    /// `AttachVolume` alike — so an instance report and `DescribeVolumes` cannot disagree
    /// about which volume an instance holds or what state its attachment is in. The state
    /// each entry reports is the one the clock has reached
    /// ([`super::VolumeRecord::attachments_to_instance_at`]), so an attachment still
    /// `attaching` is reported as such and reaches `attached` at its own due instant.
    ///
    /// The volumes of ONE instance are not indexed, so this is a pass over the volume
    /// store, which [`super::MAX_STORED_VOLUMES`] bounds; each volume is only asked for the
    /// attachments it holds to this instance, so nothing is cloned for a volume attached
    /// elsewhere. The PRODUCT a describe pays is bounded too, the instances one page
    /// reports being capped by [`super::MAX_PAGE_SIZE`].
    pub fn instance_block_device_mappings(
        &self,
        instance_id: &str,
        now: Instant,
    ) -> Vec<InstanceBlockDeviceMapping> {
        let mut mappings: Vec<InstanceBlockDeviceMapping> = self
            .volumes
            .values()
            .flat_map(|volume| volume.attachments_to_instance_at(instance_id, now))
            .map(|attachment| InstanceBlockDeviceMapping {
                device_name: attachment.device.clone(),
                ebs: Some(EbsInstanceBlockDevice {
                    attach_time: attachment.attach_time,
                    delete_on_termination: attachment.delete_on_termination,
                    status: attachment.state.clone(),
                    volume_id: attachment.volume_id.clone(),
                    // "The ARN of the Amazon Web Services-managed resource to which the
                    // volume is attached" and "the ID of the Amazon Web Services account
                    // that owns the volume … returned only for volumes that are attached to
                    // Amazon Web Services-managed resources": an instance's own volume is
                    // attached to no managed resource, so neither member applies.
                    associated_resource: None,
                    volume_owner_id: None,
                    operator: Some(unmanaged_operator()),
                    ebs_card_index: attachment.ebs_card_index,
                }),
            })
            .collect();
        mappings.sort_by(|left, right| left.device_name.cmp(&right.device_name));
        mappings
    }

    /// Report every FIXED member of `instance` onto `view` — see the module docs for the
    /// set and for where each one is read from.
    ///
    /// Called by [`ComAmazonawsEc2Amazonec2State::instance_view`], the ONE projection of a
    /// stored instance, so every operation that answers with an `Instance` answers with the
    /// same detail.
    pub(super) fn report_instance_detail(&self, instance: &InstanceRecord, view: &mut Instance) {
        let now = Instant::now();
        let detail = &instance.detail;
        // The machine facts of the AMI the instance was launched from. An AMI this account
        // cannot see publishes none to read, and the instance is EBS-backed and running all
        // the same, so it reports the facts of the EBS-backed Linux image this crate
        // publishes by default ([`ImageSpec::default`], the row every published catalog
        // image starts from) — the same fallback the launch's own root volume takes
        // (`super::launched_root_volumes`), rather than a second set of facts invented
        // here.
        let image = instance
            .image_id
            .as_deref()
            .and_then(|image_id| self.images.get(image_id));
        let defaults = ImageSpec::default();
        view.root_device_name = Some(
            image
                .and_then(|image| image.root_device_name.clone())
                .unwrap_or(defaults.root_device_name),
        );
        view.root_device_type = Some(
            image
                .and_then(|image| image.root_device_type.clone())
                .unwrap_or_else(|| IMAGE_ROOT_DEVICE_TYPE_EBS.to_string()),
        );
        view.virtualization_type = Some(
            image
                .and_then(|image| image.virtualization_type.clone())
                .unwrap_or_else(|| IMAGE_VIRTUALIZATION_TYPE_HVM.to_string()),
        );
        view.hypervisor = Some(
            image
                .and_then(|image| image.hypervisor.clone())
                .unwrap_or_else(|| IMAGE_HYPERVISOR_XEN.to_string()),
        );
        // "Indicates whether enhanced networking with ENA is enabled"
        // (`Instance$EnaSupport`): a fact of the AMI the instance was launched from, unless
        // a `ModifyInstanceAttribute` has written the INSTANCE's own `enaSupport`
        // ([`InstanceRecord::ena_support`]) — the same reading
        // [`ComAmazonawsEc2Amazonec2State::instance_sriov_net_support`] takes of the
        // attribute beside it.
        view.ena_support = Some(
            instance
                .ena_support
                .unwrap_or_else(|| image.and_then(|image| image.ena_support).unwrap_or(true)),
        );
        view.platform_details = Some(
            image
                .and_then(|image| image.platform_details.clone())
                .unwrap_or(defaults.platform_details),
        );
        view.usage_operation = Some(
            image
                .and_then(|image| image.usage_operation.clone())
                .unwrap_or(defaults.usage_operation),
        );
        // "The time that the usage operation was last updated"
        // (`Instance$UsageOperationUpdateTime`): the usage operation is the AMI's billing
        // attribution, so it was last updated when that AMI was registered — read off the
        // image's own `CreationDate` through the one conversion every report of that stored
        // string uses, never from this call's clock.
        view.usage_operation_update_time =
            image.and_then(|image| image_creation_time(image.creation_date.as_deref()));
        // "The platform. This value is `windows` for Windows instances; otherwise, it is
        // empty" (`Instance$Platform`), and the same reading for NitroTPM support: both are
        // facts of the AMI, reported only for an AMI that carries them.
        view.platform = image.and_then(|image| image.platform.clone());
        view.tpm_support = image.and_then(|image| image.tpm_support.clone());
        // "The boot mode that was specified by the AMI" (`Instance$BootMode`) — the mode
        // the image the instance was launched from declares, and, for an AMI this store
        // does not hold, the mode of the EBS-backed x86_64 HVM Linux image this crate
        // publishes by default ([`BOOT_MODE_UEFI_PREFERRED`]), the same fallback the
        // instance's other AMI-derived facts above take and the same reading
        // [`ComAmazonawsEc2Amazonec2State::instance_sriov_net_support`] takes.
        let boot_mode = match image {
            Some(image) => image.boot_mode.clone(),
            None => Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
        };
        // "The boot mode that is used to boot the instance at launch or start"
        // (`Instance$CurrentInstanceBootMode`): the mode the AMI REQUIRES where it requires
        // one, and — for an AMI that "supports both UEFI and Legacy BIOS"
        // (`uefi-preferred`) — the mode its INSTANCE TYPE offers, which is UEFI for a type
        // that supports it and the legacy BIOS for one that does not (the EC2 User Guide's
        // "Boot modes": Graviton types support UEFI only, the Nitro x86 types support both,
        // and a pre-Nitro type only the legacy BIOS). Read off the one derivation of an
        // instance type's supported boot modes this crate has
        // ([`supported_boot_modes`]), so the instance report and
        // `DescribeInstanceTypes`' own `SupportedBootModes` cannot disagree about one type.
        let type_supports_uefi = instance_type_record(&instance.instance_type).is_some_and(|record| {
            supported_boot_modes(&record.architecture, record.nitro)
                .iter()
                .any(|mode| mode == INSTANCE_BOOT_MODE_UEFI)
        });
        let boots_uefi = match boot_mode.as_deref() {
            Some(INSTANCE_BOOT_MODE_UEFI) => true,
            Some(BOOT_MODE_UEFI_PREFERRED) => type_supports_uefi,
            _ => false,
        };
        view.current_instance_boot_mode = Some(if boots_uefi {
            INSTANCE_BOOT_MODE_UEFI.to_string()
        } else {
            INSTANCE_BOOT_MODE_LEGACY_BIOS.to_string()
        });
        view.boot_mode = boot_mode;

        // "Any block device mapping entries for the instance"
        // (`Instance$BlockDeviceMappings`): the volumes the instance HOLDS, which a launch
        // still running holds none of — the root volume of a launch is attached as part of
        // that launch completing, so an instance still `pending` reports NO entry rather
        // than one for a volume it has not been given yet, and reports every entry once the
        // launch has carried it to `running`.
        view.block_device_mappings = Some(if instance.state == InstanceStateName::Pending {
            Vec::new()
        } else {
            self.instance_block_device_mappings(&instance.instance_id, now)
        });

        // "The CPU options for the instance" (`Instance$CpuOptions`): the cores and threads
        // per core of its own instance type, read off the one derivation of an instance
        // type's facts this crate has ([`instance_type_record`]), so the instance report and
        // `DescribeInstanceTypes` cannot disagree about one type. A type this emulator does
        // not offer has no CPU layout to report.
        view.cpu_options = instance_type_record(&instance.instance_type).map(|record| CpuOptions {
            core_count: Some(record.default_cores),
            threads_per_core: Some(record.default_threads_per_core),
            // AMD SEV-SNP and nested virtualization are enabled by a launch that asks for
            // them; nothing here asks, so neither is reported.
            amd_sev_snp: None,
            nested_virtualization: None,
        });

        // "The location where the instance launched" (`Instance$Placement`): its zone, that
        // zone's id, the tenancy of the hardware it runs on and the placement group it is
        // in — by name and by id, both read off the group the launch resolved, and the EMPTY
        // name AWS reports for an instance in no group.
        let placement_group = detail
            .placement_group_name
            .as_deref()
            .and_then(|group_name| self.placement_group_id_for_name(group_name));
        view.placement = Some(Placement {
            availability_zone: Some(instance.placement_az.clone()),
            availability_zone_id: zone_id_for_zone_name(&instance.placement_az),
            group_name: Some(detail.placement_group_name.clone().unwrap_or_default()),
            group_id: placement_group,
            tenancy: Some(detail.tenancy.to_string()),
            // A Dedicated Host, a partition and a host resource group are all things a
            // launch asks for; this one asked for none.
            affinity: None,
            host_id: None,
            host_resource_group_arn: None,
            partition_number: None,
            spread_domain: None,
        });

        // The option sets the launch chose (and a modify of them since), reported back
        // from the record that stored them.
        view.metadata_options = Some(detail.metadata_options_at(now));
        view.enclave_options = Some(EnclaveOptions {
            enabled: Some(detail.enclave_enabled),
        });
        view.hibernation_options = Some(HibernationOptions {
            configured: Some(instance.hibernation_configured),
        });
        view.capacity_reservation_specification = Some(CapacityReservationSpecificationResponse {
            capacity_reservation_preference: Some(
                detail.capacity_reservation_preference.to_string(),
            ),
            // "The action returns the `capacityReservationTarget` response element if the
            // instance explicitly targets a specific Capacity Reservation or Capacity
            // Reservation group" (`CapacityReservationSpecificationResponse`): this launch
            // targets none, so it reports the preference alone.
            capacity_reservation_target: None,
        });
        view.maintenance_options = Some(InstanceMaintenanceOptions {
            auto_recovery: Some(detail.auto_recovery.to_string()),
            // "This setting only applies to supported instances that have a scheduled
            // reboot event" (`InstanceMaintenanceOptions$RebootMigration`), and the setting
            // itself is a property of the instance either way: an instance nothing has
            // disabled migration for reports the `default` behaviour.
            reboot_migration: Some(REBOOT_MIGRATION_DEFAULT.to_string()),
        });
        view.network_performance_options = Some(InstanceNetworkPerformanceOptions {
            bandwidth_weighting: Some(detail.bandwidth_weighting.to_string()),
        });
        // "The options for the instance hostname" (`Instance$PrivateDnsNameOptions`): the
        // options the launch named, and for each one it did not, the on-launch option of the
        // SUBNET the instance runs in (`Subnet$PrivateDnsNameOptionsOnLaunch`) — read off
        // that subnet rather than copied onto the instance, so a caller that modifies the
        // subnet attribute and the instance report cannot disagree.
        let subnet_options = self
            .instance_subnet_id(instance)
            .and_then(|subnet_id| self.subnets.get(&subnet_id))
            .and_then(|subnet| subnet.private_dns_name_options_on_launch.clone());
        view.private_dns_name_options = Some(PrivateDnsNameOptionsResponse {
            hostname_type: detail
                .private_dns_hostname_type
                .map(str::to_string)
                .or_else(|| {
                    subnet_options
                        .as_ref()
                        .and_then(|options| options.hostname_type.clone())
                })
                .or_else(|| Some(HOSTNAME_TYPE_IP_NAME.to_string())),
            enable_resource_name_dns_a_record: detail
                .private_dns_a_record
                .or_else(|| {
                    subnet_options
                        .as_ref()
                        .and_then(|options| options.enable_resource_name_dns_a_record)
                })
                .or(Some(false)),
            enable_resource_name_dns_aaaa_record: detail
                .private_dns_aaaa_record
                .or_else(|| {
                    subnet_options
                        .as_ref()
                        .and_then(|options| options.enable_resource_name_dns_aaaa_record)
                })
                .or(Some(false)),
        });
        // "The service provider that manages the instance" (`Instance$Operator`): the
        // caller's own launch, so no service provider manages it — the one reading of that
        // answer this crate has.
        view.operator = Some(unmanaged_operator());
        // "The secondary interfaces for the instance" (`Instance$SecondaryInterfaces`): an
        // empty set for an instance that has none, which is every instance here — a
        // secondary interface is created by a launch that asks for one, and no launch path
        // here does. The instance's own interfaces are `Instance$NetworkInterfaces`,
        // projected from the interface store by
        // [`ComAmazonawsEc2Amazonec2State::instance_network_interfaces`].
        view.secondary_interfaces = Some(Vec::new());
        // "The public DNS name assigned to the instance … only available if you've enabled
        // DNS hostnames for your VPC" (`Instance$PublicDnsName`): derived from the public
        // address the instance holds, and the EMPTY name AWS reports for an instance that
        // holds none — the member is reported either way.
        view.public_dns_name = Some(
            instance
                .public_ip
                .as_deref()
                .map(|public_ip| {
                    super::public_dns_name(public_ip, region_of_zone_name(&instance.placement_az))
                })
                .unwrap_or_default(),
        );
    }
}
