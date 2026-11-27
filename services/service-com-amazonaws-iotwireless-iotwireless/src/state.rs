// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotwireless-iotwireless service.
//!
//! Every implemented operation computes its answer from the records held here; the
//! handlers reach this store through [`service_state`] and nothing else.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    ApplicationConfig, ConflictException, ConnectionStatusEventConfiguration,
    DeviceRegistrationStateEventConfiguration, FPorts, JoinEventConfiguration, LoRaWANDevice,
    LoRaWANDeviceProfile, LoRaWANFuotaTask, LoRaWANGateway, LoRaWANGetServiceProfileInfo,
    LoRaWANConnectionStatusEventNotificationConfigurations,
    LoRaWANJoinEventNotificationConfigurations, LoRaWANMulticast, LoRaWANServiceProfile,
    MessageDeliveryStatusEventConfiguration, Positioning, ProximityEventConfiguration,
    ResourceNotFoundException, SidewalkDevice, SidewalkEventNotificationConfigurations,
    SidewalkPositioning, Tag, TraceContent, UpdateWirelessGatewayTaskCreate, ValidationException,
};

/// The ARN service namespace (`arn:aws:iotwireless:...`) and the SigV4 signing name
/// this service publishes.
pub const ARN_SERVICE: &str = "iotwireless";

/// The region an ARN carries when a request names none (unsigned, or signed without a
/// parseable credential scope). `us-east-1` is the default every generated emulator
/// presents.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request: the SigV4 credential scope's
/// region, falling back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:iotwireless:<region>:<account>:Destination/<name>`.
pub fn destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("Destination/{name}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessDevice/<id>`.
pub fn wireless_device_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("WirelessDevice/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:DeviceProfile/<id>`.
pub fn device_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("DeviceProfile/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:ServiceProfile/<id>`.
pub fn service_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("ServiceProfile/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:MulticastGroup/<id>`.
pub fn multicast_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("MulticastGroup/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:FuotaTask/<id>`.
pub fn fuota_task_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("FuotaTask/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessGateway/<id>`.
pub fn wireless_gateway_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("WirelessGateway/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessGatewayTaskDefinition/<id>`.
pub fn wireless_gateway_task_definition_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("WirelessGatewayTaskDefinition/{id}"),
    )
}

/// `arn:aws:iotwireless:<region>:<account>:NetworkAnalyzerConfiguration/<name>`.
pub fn network_analyzer_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("NetworkAnalyzerConfiguration/{name}"),
    )
}

/// `ExpressionType`'s modelled values, as the model-generated
/// [`crate::ops::types::EXPRESSION_TYPE_VALUES`] lists them.
pub use crate::ops::types::EXPRESSION_TYPE_VALUES as EXPRESSION_TYPES;

/// `WirelessDeviceType`'s modelled values, as the model-generated
/// [`crate::ops::types::WIRELESS_DEVICE_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_DEVICE_TYPE_VALUES as WIRELESS_DEVICE_TYPES;

/// The identifier kinds `GetWirelessDevice` accepts, as the model-generated
/// [`crate::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES as WIRELESS_DEVICE_ID_TYPES;

/// The identifier kinds `GetWirelessGateway` accepts, as the model-generated
/// [`crate::ops::types::WIRELESS_GATEWAY_ID_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_GATEWAY_ID_TYPE_VALUES as WIRELESS_GATEWAY_ID_TYPES;

/// `PositioningConfigStatus`'s modelled values, as the model-generated
/// [`crate::ops::types::POSITIONING_CONFIG_STATUS_VALUES`] lists them.
pub use crate::ops::types::POSITIONING_CONFIG_STATUS_VALUES as POSITIONING_CONFIG_STATUSES;

/// The `PositioningConfigStatus` a wireless device carries when nothing has named one.
///
/// `Positioning` is the integration status of the Device Location feature, one of the two
/// values `PositioningConfigStatus` lists, so every device has one: a device whose create
/// asked for no integration has the feature not integrated, and every reader reports that
/// rather than leaving the member out.
pub const DEFAULT_POSITIONING_CONFIG_STATUS: &str = "Disabled";

/// `DeviceProfileType`'s modelled values, as the model-generated
/// [`crate::ops::types::DEVICE_PROFILE_TYPE_VALUES`] lists them.
pub use crate::ops::types::DEVICE_PROFILE_TYPE_VALUES as DEVICE_PROFILE_TYPES;

/// The characters a resource name may not carry, as every `Name` member's model doc
/// states it ("The following special characters aren't accepted: <>^#~$").
pub const FORBIDDEN_NAME_CHARS: &[char] = &['<', '>', '^', '#', '~', '$'];

/// The first character of `name` this service refuses, or `None` when every character
/// is accepted.
pub fn forbidden_name_char(name: &str) -> Option<char> {
    name.chars().find(|c| FORBIDDEN_NAME_CHARS.contains(c))
}

/// The marker every `NextToken` this service mints carries, so a well-formed string a
/// client made up cannot pass as a resume point.
const PAGE_TOKEN_MARKER: &str = "iotwireless/v1/";

/// Encode a list's resume point — the key of the LAST item the page returned — as the
/// opaque `NextToken` a client echoes back.
///
/// Hex rather than the raw key so the token stays opaque: a client cannot read a
/// resource name out of it, and it survives a URI query parameter unescaped. The
/// encoded payload carries [`PAGE_TOKEN_MARKER`], which [`decode_page_token`] requires,
/// so only a token this service issued resumes a list.
pub fn encode_page_token(last_key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_MARKER}{last_key}").as_bytes())
}

/// The resume point a `NextToken` names, or `None` when the token is not one this
/// service minted (the caller gets a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_MARKER)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

/// One page of a list, and the `NextToken` that resumes after it.
///
/// `records` walks the store's values in the stable key order its `BTreeMap` holds them
/// in, already narrowed to whatever the request filtered on; `key` names the record's
/// store key (what a resume point is), and `project` is the list's own view of a record.
/// `max_results` is the modelled page size, where 0 is "no caller-supplied page size"
/// and the whole remaining list is returned.
///
/// One function, so every paginated list in this service resumes, truncates and mints
/// its token the same way: one more than the page size is taken, so whether a further
/// page exists is known without materialising the whole remaining list, and the token
/// resumes AFTER the last key the page returned — neither a repeat nor an omission.
pub fn page_of<'a, T: 'a, V>(
    records: impl Iterator<Item = &'a T>,
    resume_after: Option<&str>,
    max_results: i32,
    key: impl Fn(&T) -> &str,
    project: impl Fn(&T) -> V,
) -> (Vec<V>, Option<String>) {
    let limit = (max_results > 0).then_some(max_results as usize);
    let mut page: Vec<(String, V)> = records
        .filter(|record| resume_after.is_none_or(|last| key(record) > last))
        .take(limit.map_or(usize::MAX, |limit| limit.saturating_add(1)))
        .map(|record| (key(record).to_string(), project(record)))
        .collect();

    let mut next_token = None;
    if let Some(limit) = limit {
        if page.len() > limit {
            page.truncate(limit);
            next_token = page.last().map(|(key, _)| encode_page_token(key));
        }
    }
    (page.into_iter().map(|(_, value)| value).collect(), next_token)
}

/// A destination: the rule or MQTT topic a device's uplinks are routed to.
#[derive(Debug, Clone)]
pub struct Destination {
    pub name: String,
    pub arn: String,
    pub expression: String,
    /// `ExpressionType`: `RuleName` | `MqttTopic`.
    pub expression_type: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub tags: Vec<Tag>,
}

/// The sentence this service's 404 carries when the destination a request addresses is
/// not one the account holds: fixed text, with no part of it taken from the request or
/// from the store, so the refusal discloses nothing about which names exist.
pub const DESTINATION_NOT_FOUND_MESSAGE: &str = "Destination doesn't exist";

/// The `ResourceNotFoundException` body for a destination the account does not hold.
///
/// One function, so every operation that addresses a destination by name refuses a
/// missing one with the SAME body: [`DESTINATION_NOT_FOUND_MESSAGE`] and the two
/// optional members the model merely permits (`ResourceId`, `ResourceType`) left unset,
/// which is how this service answers the destination lookup.
pub fn destination_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(DESTINATION_NOT_FOUND_MESSAGE.to_string()),
        resource_id: None,
        resource_type: None,
    }
}

/// A device profile: the radio parameters a wireless device of one technology shares.
///
/// `profile_type` is what `DeviceProfileType` names, derived from which technology
/// member the create carried; `SidewalkCreateDeviceProfile` models no members, so a
/// Sidewalk profile's only stored settings are that it IS a Sidewalk profile.
#[derive(Debug, Clone)]
pub struct DeviceProfile {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// `DeviceProfileType`: `LoRaWAN` | `Sidewalk`.
    pub profile_type: String,
    pub lorawan: Option<LoRaWANDeviceProfile>,
    pub tags: Vec<Tag>,
}

/// The `RfRegion` a device profile carries when its create named none: the frequency
/// band this service defaults a LoRaWAN device profile to, one of the modelled
/// `SupportedRfRegion` values.
pub const DEFAULT_DEVICE_PROFILE_RF_REGION: &str = "US915";

/// The `RxDelay1` a device profile carries when its create named none: the LoRaWAN
/// RECEIVE_DELAY1 of one second, the delay between the end of an uplink and the opening
/// of the first downlink window.
pub const DEFAULT_RX_DELAY1: i32 = 1;

/// The `RxDrOffset1` a device profile carries when its create named none: the first
/// downlink window uses the uplink data rate unshifted.
pub const DEFAULT_RX_DR_OFFSET1: i32 = 0;

/// The `MaxDutyCycle` a device profile carries when its create named none.
pub const DEFAULT_MAX_DUTY_CYCLE: i32 = 10;

/// Whether a device profile supports over-the-air activation when its create named no
/// `SupportsJoin`: it does, which is how a device joins without pre-provisioned session
/// keys.
pub const DEFAULT_SUPPORTS_JOIN: bool = true;

/// The settings a created device profile carries: the ones the request named, kept
/// exactly as submitted, and this service's default for each radio parameter it left
/// out.
///
/// One function, so the stored resource — and therefore every reader of it — carries the
/// same settings whether or not the create spelled them out. The members with no default
/// here (`MacVersion`, `RegParamsRevision`, `MaxEirp`, the class B/C and ping-slot
/// members) are reported only when the create named them: the service fills in the
/// parameters that describe the frequency plan and the downlink windows, not the ones
/// that describe the device's own capabilities.
///
/// A radio parameter the plan of ONE band decides — the second receive window's data
/// rate and frequency, and the join channels — is taken from the band the profile
/// resolves to ([`region_device_profile_plan`]), never from a fixed set: filling a
/// profile in one band with another band's channel would report a channel no device in
/// it listens on. A band this service holds no plan for leaves those three unset for the
/// same reason, while the parameters no band decides are still filled in.
pub fn device_profile_settings(requested: &LoRaWANDeviceProfile) -> LoRaWANDeviceProfile {
    // The band is resolved first, because the plan the rest comes from hangs off it.
    let rf_region = requested
        .rf_region
        .clone()
        .unwrap_or_else(|| DEFAULT_DEVICE_PROFILE_RF_REGION.to_string());
    let plan = region_device_profile_plan(&rf_region);
    LoRaWANDeviceProfile {
        rx_delay1: Some(requested.rx_delay1.unwrap_or(DEFAULT_RX_DELAY1)),
        rx_dr_offset1: Some(requested.rx_dr_offset1.unwrap_or(DEFAULT_RX_DR_OFFSET1)),
        max_duty_cycle: Some(requested.max_duty_cycle.unwrap_or(DEFAULT_MAX_DUTY_CYCLE)),
        supports_join: Some(requested.supports_join.unwrap_or(DEFAULT_SUPPORTS_JOIN)),
        rx_data_rate2: requested
            .rx_data_rate2
            .or(plan.map(|plan| plan.rx_data_rate2)),
        rx_freq2: requested.rx_freq2.or(plan.map(|plan| plan.rx_freq2)),
        factory_preset_freqs_list: requested.factory_preset_freqs_list.clone().or_else(|| {
            plan.map(|plan| plan.join_channels.to_vec())
                .filter(|channels| !channels.is_empty())
        }),
        rf_region: Some(rf_region),
        ..requested.clone()
    }
}

/// The sentence this service's 409 carries when a create presents a `DevEui` another
/// device in the account already holds together with join key material that DIFFERS from
/// that device's: a second, distinct device colliding on one identifier.
///
/// FIXED text, with no variable part at all: it states that a device with the submitted
/// `DevEui` is already registered, and so names neither the identifier the request sent
/// (which the caller already holds) nor the device that holds it (which the caller may not
/// be able to read). A client tells this conflict from the idempotency-token conflict the
/// same operation raises with the same error code by matching the text.
pub const DUPLICATE_DEV_EUI_MESSAGE: &str = "Device with given DevEui already exists";

/// The sentence the same 409 carries when the create presents the SAME join key material as
/// the device that already holds the `DevEui`: the registration the request asks for has
/// already been made, so the refusal reports the identifier it was claimed under rather than
/// describing a collision between two devices.
///
/// The only variable part is the `DevEui` the request submitted — a value the caller sent, so
/// reporting it discloses nothing. Everything else is fixed text, the two-space gap before
/// `with` included: that is how the service words this sentence.
pub fn claimed_dev_eui_message(dev_eui: &str) -> String {
    format!("DevEui {dev_eui}  with same key has been claimed")
}

/// The LoRaWAN join key material a device configuration presents, as one comparable value.
///
/// It is the key and address members of every provisioning block the configuration carries —
/// `OtaaV1_1` (AppKey/NwkKey/JoinEui), `OtaaV1_0_x` (AppKey/AppEui/JoinEui/GenAppKey),
/// `AbpV1_1` and `AbpV1_0_x` (DevAddr and their session keys) — and nothing else: the frame
/// counter an ABP session resumes from moves over a device's life and is not key material, and
/// neither are the profile references, the `FPorts` assignment or the `DevEui` the material is
/// claimed under. Source: `smithy_query(query="shape",
/// shape="LoRaWANDevice,OtaaV1_1,OtaaV1_0_x,AbpV1_1,AbpV1_0_x,SessionKeysAbpV1_1,SessionKeysAbpV1_0_x")`.
///
/// Every member is hexadecimal (the model's patterns are `^[a-fA-F0-9]{n}$`), and hexadecimal
/// is case-insensitive, so the same key written in either case is the same key. A
/// configuration carrying no provisioning block presents no key material, and two such
/// configurations present the same (empty) material.
pub fn lorawan_key_material(lorawan: Option<&LoRaWANDevice>) -> String {
    let otaa_v1_1 = lorawan.and_then(|lorawan| lorawan.otaa_v1_1.as_ref());
    let otaa_v1_0_x = lorawan.and_then(|lorawan| lorawan.otaa_v1_0_x.as_ref());
    let abp_v1_1 = lorawan.and_then(|lorawan| lorawan.abp_v1_1.as_ref());
    let abp_v1_0_x = lorawan.and_then(|lorawan| lorawan.abp_v1_0_x.as_ref());
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        otaa_v1_1.map(|otaa| (&otaa.app_key, &otaa.nwk_key, &otaa.join_eui)),
        otaa_v1_0_x
            .map(|otaa| (&otaa.app_key, &otaa.app_eui, &otaa.join_eui, &otaa.gen_app_key)),
        abp_v1_1.map(|abp| &abp.dev_addr),
        abp_v1_1
            .and_then(|abp| abp.session_keys.as_ref())
            .map(|keys| (
                &keys.f_nwk_s_int_key,
                &keys.s_nwk_s_int_key,
                &keys.nwk_s_enc_key,
                &keys.app_s_key
            )),
        abp_v1_0_x.map(|abp| &abp.dev_addr),
        abp_v1_0_x
            .and_then(|abp| abp.session_keys.as_ref())
            .map(|keys| (&keys.nwk_s_key, &keys.app_s_key)),
    )
    .to_ascii_lowercase()
}

/// The `ConflictException` body for a `DevEui` already registered in the account, for the
/// collision the request actually is: a re-presentation of the registered device, when the
/// request's join key material is the material the holder was registered with
/// ([`claimed_dev_eui_message`]), and a different device claiming a taken identifier when the
/// two differ ([`DUPLICATE_DEV_EUI_MESSAGE`]).
///
/// One function, so every operation that claims a `DevEui` refuses a duplicate with the same
/// body. Both sentences leave the two optional members the model merely permits (`ResourceId`,
/// `ResourceType`) unset, which keeps the owning device's identity out of a refusal served to
/// a caller that cannot read it.
pub fn dev_eui_conflict(
    dev_eui: &str,
    requested: Option<&LoRaWANDevice>,
    holder: Option<&LoRaWANDevice>,
) -> ConflictException {
    let message = if lorawan_key_material(requested) == lorawan_key_material(holder) {
        claimed_dev_eui_message(dev_eui)
    } else {
        DUPLICATE_DEV_EUI_MESSAGE.to_string()
    };
    ConflictException {
        message: Some(message),
        resource_id: None,
        resource_type: None,
    }
}

/// The LoRaWAN specification version the 1.1 provisioning members (`OtaaV1_1`,
/// `AbpV1_1`) carry key material for.
/// Source: `LoRaWANDevice`'s members, `smithy_query(query="shape",
/// shape="LoRaWANDevice")` ("OTAA device object for v1.1", "ABP device object for
/// create APIs for v1.1").
pub const LORAWAN_VERSION_1_1: &str = "1.1";

/// The LoRaWAN specification version the 1.0.x provisioning members (`OtaaV1_0_x`,
/// `AbpV1_0_x`) carry key material for, at the granularity provisioning is defined at:
/// 1.0.x key material is the same across the 1.0 revisions.
/// Source: the same `LoRaWANDevice` members ("for v1.0.x").
pub const LORAWAN_VERSION_1_0: &str = "1.0";

/// The specification version family a `MacVersion` names: its first two dot-separated
/// components, since a device profile's `MacVersion` is a revision of a family
/// (`1.0.3` is a revision of `1.0`) and provisioning is version-specific only down to
/// the family.
///
/// `MacVersion` is a free-form string in the model (`length 0..64`), so an
/// unrecognisable value is its own family and matches only itself.
pub fn mac_version_family(mac_version: &str) -> &str {
    match mac_version.match_indices('.').nth(1) {
        Some((second_dot, _)) => &mac_version[..second_dot],
        None => mac_version,
    }
}

/// Every LoRaWAN specification version the provisioning members of `lorawan` carry key
/// material for, in the order the model declares them.
pub fn provisioning_versions(lorawan: &LoRaWANDevice) -> Vec<&'static str> {
    let mut versions = Vec::new();
    if lorawan.otaa_v1_1.is_some() || lorawan.abp_v1_1.is_some() {
        versions.push(LORAWAN_VERSION_1_1);
    }
    if lorawan.otaa_v1_0_x.is_some() || lorawan.abp_v1_0_x.is_some() {
        versions.push(LORAWAN_VERSION_1_0);
    }
    versions
}

/// The referenced profile's `MacVersion` when it cannot provision the key material
/// `lorawan` carries, and `None` when the two agree.
///
/// A device profile declares the MAC version the devices it provisions run, and each
/// provisioning member a create carries declares the version ITS key material is for,
/// so EVERY member the create carries must name the profile's version family — a create
/// carrying material for two families cannot agree with a profile that declares one. A
/// profile that declares no `MacVersion` (absent, or the empty string the model's
/// `length 0..64` permits), and a create that carries no provisioning member, constrain
/// nothing.
pub fn mismatched_mac_version(lorawan: &LoRaWANDevice, profile: &DeviceProfile) -> Option<String> {
    let mac_version = profile
        .lorawan
        .as_ref()?
        .mac_version
        .as_deref()
        .filter(|declared| !declared.is_empty())?;
    let family = mac_version_family(mac_version);
    provisioning_versions(lorawan)
        .into_iter()
        .any(|requested| requested != family)
        .then(|| mac_version.to_string())
}

/// The `ValidationException` body for key material the referenced device profile's
/// `MacVersion` cannot provision: fixed text whose only variable part is that stored
/// `MacVersion`.
pub fn mac_version_mismatch(mac_version: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Validation error: the input profile version is not match macVersion defines in \
             deviceProfile: {mac_version}"
        )),
    }
}

/// A service profile: the LoRaWAN network-server settings a set of wireless devices
/// shares.
///
/// `lorawan` holds the settings as the network server carries them — the create's own
/// members where the request named them, and the default the service applies for every
/// other member of the reported shape — so every reader projects one stored record
/// rather than re-deriving a default of its own. It is the REPORTED shape
/// (`LoRaWANGetServiceProfileInfo`), a superset of what a create can set, because the
/// network server adds its rate-policing, ADR and status-request configuration to every
/// profile it provisions; it is not optional for the same reason — every service profile
/// is provisioned there, whether or not its create named a `LoRaWAN` block.
#[derive(Debug, Clone)]
pub struct ServiceProfile {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub lorawan: LoRaWANGetServiceProfileInfo,
    pub tags: Vec<Tag>,
}

/// The `TxPowerIndexMin` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.TxPowerIndexMin`'s model doc ("Default: 0").
pub const DEFAULT_TX_POWER_INDEX_MIN: i32 = 0;

/// The `TxPowerIndexMax` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.TxPowerIndexMax`'s model doc ("Default: 15").
pub const DEFAULT_TX_POWER_INDEX_MAX: i32 = 15;

/// The `NbTransMin` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.NbTransMin`'s model doc ("Default: 0").
pub const DEFAULT_NB_TRANS_MIN: i32 = 0;

/// The `NbTransMax` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.NbTransMax`'s model doc ("Default: 3").
pub const DEFAULT_NB_TRANS_MAX: i32 = 3;

/// The `DrMin` a service profile carries when its create named none.
/// Source: `LoRaWANGetServiceProfileInfo.DrMin`'s modelled `@default` (0), which is the
/// value the reported shape carries for a member nothing set.
pub const DEFAULT_DR_MIN: i32 = 0;

/// The `DrMax` a service profile carries when its create named none.
/// Source: `LoRaWANGetServiceProfileInfo.DrMax`'s modelled `@default` (0).
pub const DEFAULT_DR_MAX: i32 = 0;

/// The `UlRate` the network server provisions a service profile with (messages per
/// hour), one of the account's service-profile defaults: a server-side constant the
/// create cannot set and no reader may report as unset.
pub const DEFAULT_UL_RATE: i32 = 60;

/// The `UlBucketSize` the network server provisions a service profile with.
pub const DEFAULT_UL_BUCKET_SIZE: i32 = 4096;

/// The `DlRate` the network server provisions a service profile with.
pub const DEFAULT_DL_RATE: i32 = 60;

/// The `DlBucketSize` the network server provisions a service profile with.
pub const DEFAULT_DL_BUCKET_SIZE: i32 = 4096;

/// The `DevStatusReqFreq` the network server provisions a service profile with (device
/// status requests per day).
pub const DEFAULT_DEV_STATUS_REQ_FREQ: i32 = 24;

/// The `MinGwDiversity` the network server provisions a service profile with — the
/// bottom of the member's published range (`@range` 1..100).
pub const DEFAULT_MIN_GW_DIVERSITY: i32 = 1;

/// The `TargetPer` the network server provisions a service profile with: the packet
/// error rate ADR aims at, as a percentage inside the member's published range
/// (`@range` 0..100).
pub const DEFAULT_TARGET_PER: i32 = 5;

/// The `AdrAlgorithm` the network server runs for a provisioned service profile.
///
/// `AdrAlgorithm` and [`DEFAULT_UPLINK_HISTORY_LEN`]'s member are reported members the
/// shipped service-model snapshot does not declare on `LoRaWANGetServiceProfileInfo`,
/// so that one shape is declared against this crate's own small model extension
/// instead; [`crate::ops::types::service_profile_report_types`] says why and what a
/// later refresh of the snapshot should do with it.
pub const DEFAULT_ADR_ALGORITHM: &str = "ORIGINAL";

/// The `UplinkHistoryLen` the network server's adaptive data rate algorithm keeps for a
/// provisioned service profile.
pub const DEFAULT_UPLINK_HISTORY_LEN: i32 = 20;

/// The settings a created service profile carries, as the reported shape carries them:
/// the ones the request named, and the default the service applies for every member it
/// could not or did not name.
///
/// One function, so the stored resource — and therefore every reader of it — carries
/// the same settings whether or not the create spelled them out, and the network-server
/// half of the profile (rate policing, device-status requests, gateway diversity, the
/// ADR packet-error target) is provisioned once, where the resource is minted. A create
/// that named no `LoRaWAN` block at all (`None`) is a create that named none of its
/// members, so it carries the same provisioned settings as one that sent an empty block.
pub fn service_profile_settings(
    requested: Option<&LoRaWANServiceProfile>,
) -> LoRaWANGetServiceProfileInfo {
    let requested = requested.cloned().unwrap_or_default();
    LoRaWANGetServiceProfileInfo {
        // The members `CreateServiceProfile`'s own shape carries, as the request named
        // them, with each member's published default for the ones it omitted.
        add_gw_metadata: requested.add_gw_metadata,
        dr_min: requested.dr_min.unwrap_or(DEFAULT_DR_MIN),
        dr_max: requested.dr_max.unwrap_or(DEFAULT_DR_MAX),
        pr_allowed: requested.pr_allowed,
        ra_allowed: requested.ra_allowed,
        tx_power_index_min: Some(
            requested
                .tx_power_index_min
                .unwrap_or(DEFAULT_TX_POWER_INDEX_MIN),
        ),
        tx_power_index_max: Some(
            requested
                .tx_power_index_max
                .unwrap_or(DEFAULT_TX_POWER_INDEX_MAX),
        ),
        nb_trans_min: Some(requested.nb_trans_min.unwrap_or(DEFAULT_NB_TRANS_MIN)),
        nb_trans_max: Some(requested.nb_trans_max.unwrap_or(DEFAULT_NB_TRANS_MAX)),
        // The network-server half, which nothing in this service's model can set: the
        // provisioned profile carries the service's default for each one.
        ul_rate: Some(DEFAULT_UL_RATE),
        ul_bucket_size: Some(DEFAULT_UL_BUCKET_SIZE),
        dl_rate: Some(DEFAULT_DL_RATE),
        dl_bucket_size: Some(DEFAULT_DL_BUCKET_SIZE),
        dev_status_req_freq: Some(DEFAULT_DEV_STATUS_REQ_FREQ),
        min_gw_diversity: Some(DEFAULT_MIN_GW_DIVERSITY),
        target_per: DEFAULT_TARGET_PER,
        adr_algorithm: Some(DEFAULT_ADR_ALGORITHM.to_string()),
        uplink_history_len: Some(DEFAULT_UPLINK_HISTORY_LEN),
        // Everything left is a member the network server provisions no value for — the
        // two rate policies, the channel mask — or a boolean flag whose modelled default
        // is `false`, which is what `Default` carries.
        ..LoRaWANGetServiceProfileInfo::default()
    }
}

/// The `ValidationException` body for a create whose data-rate window is inverted, or
/// `None` when it is not.
///
/// The window is `DrMin..DrMax` with the modelled default standing in for either member
/// the request omitted, and the service refuses a window whose bottom is above its top
/// before provisioning anything; the message's only variable parts are those two
/// effective values.
pub fn inverted_data_rate_window(requested: &LoRaWANServiceProfile) -> Option<ValidationException> {
    let dr_min = requested.dr_min.unwrap_or(DEFAULT_DR_MIN);
    let dr_max = requested.dr_max.unwrap_or(DEFAULT_DR_MAX);
    (dr_min > dr_max).then(|| ValidationException {
        message: Some(format!(
            "DrMin {dr_min} should not be larger than DrMax {dr_max}"
        )),
    })
}

/// The `EventNotificationTopicStatus` an event topic carries until an update enables it.
///
/// One of the modelled values [`crate::ops::types::EVENT_NOTIFICATION_TOPIC_STATUS_VALUES`]
/// lists: a resource publishes to no event topic nobody asked for, so every topic the
/// addressed resource publishes reports this status until
/// `UpdateResourceEventConfiguration` sets it.
pub const EVENT_TOPIC_DISABLED: &str = "Disabled";

/// One resource's event configuration: which of its event topics are enabled, held as
/// the modelled sections `UpdateResourceEventConfiguration` writes and
/// `GetResourceEventConfiguration` reports.
///
/// A section stays `None` until an update names it, and [`Self::overlay`] is the one
/// merge both sides go through — the writer takes the request's statuses into the stored
/// record, the reader takes the stored record onto
/// [`EventConfigurationResource::disabled_event_configuration`] — so a reader reports
/// exactly the statuses the writes left and a default for nothing else.
#[derive(Debug, Clone, Default)]
pub struct ResourceEventConfiguration {
    pub device_registration_state: Option<DeviceRegistrationStateEventConfiguration>,
    pub proximity: Option<ProximityEventConfiguration>,
    pub join: Option<JoinEventConfiguration>,
    pub connection_status: Option<ConnectionStatusEventConfiguration>,
    pub message_delivery_status: Option<MessageDeliveryStatusEventConfiguration>,
}

/// Take `requested`'s status for one event topic, leaving the stored one as it was when
/// the request named none.
fn merge_topic(stored: &mut Option<String>, requested: &Option<String>) {
    if let Some(status) = requested {
        *stored = Some(status.clone());
    }
}

/// Member-wise merge for the `Sidewalk` member the Sidewalk event sections carry.
fn merge_sidewalk_topics(
    stored: &mut Option<SidewalkEventNotificationConfigurations>,
    requested: &Option<SidewalkEventNotificationConfigurations>,
) {
    if let Some(requested) = requested {
        let target = stored.get_or_insert_with(SidewalkEventNotificationConfigurations::default);
        merge_topic(&mut target.amazon_id_event_topic, &requested.amazon_id_event_topic);
    }
}

/// Member-wise merge for each event section whose shape is `{ Sidewalk: {
/// AmazonIdEventTopic }, WirelessDeviceIdEventTopic }` — `DeviceRegistrationState`,
/// `Proximity` and `MessageDeliveryStatus`, which are three distinct modelled structures
/// with the same members, so one merge rule serves all three.
macro_rules! merge_sidewalk_section {
    ($name:ident, $section:ty) => {
        fn $name(stored: &mut Option<$section>, requested: &Option<$section>) {
            if let Some(requested) = requested {
                let target = stored.get_or_insert_with(<$section>::default);
                merge_sidewalk_topics(&mut target.sidewalk, &requested.sidewalk);
                merge_topic(
                    &mut target.wireless_device_id_event_topic,
                    &requested.wireless_device_id_event_topic,
                );
            }
        }
    };
}

merge_sidewalk_section!(
    merge_device_registration_state,
    DeviceRegistrationStateEventConfiguration
);
merge_sidewalk_section!(merge_proximity, ProximityEventConfiguration);
merge_sidewalk_section!(
    merge_message_delivery_status,
    MessageDeliveryStatusEventConfiguration
);

/// Member-wise merge for the `Join` section, whose technology member is LoRaWAN's
/// `DevEuiEventTopic`.
fn merge_join(stored: &mut Option<JoinEventConfiguration>, requested: &Option<JoinEventConfiguration>) {
    if let Some(requested) = requested {
        let target = stored.get_or_insert_with(JoinEventConfiguration::default);
        if let Some(requested_lorawan) = &requested.lo_ra_wan {
            let lorawan = target
                .lo_ra_wan
                .get_or_insert_with(LoRaWANJoinEventNotificationConfigurations::default);
            merge_topic(
                &mut lorawan.dev_eui_event_topic,
                &requested_lorawan.dev_eui_event_topic,
            );
        }
        merge_topic(
            &mut target.wireless_device_id_event_topic,
            &requested.wireless_device_id_event_topic,
        );
    }
}

/// Member-wise merge for the `ConnectionStatus` section, whose technology member is
/// LoRaWAN's `GatewayEuiEventTopic`.
fn merge_connection_status(
    stored: &mut Option<ConnectionStatusEventConfiguration>,
    requested: &Option<ConnectionStatusEventConfiguration>,
) {
    if let Some(requested) = requested {
        let target = stored.get_or_insert_with(ConnectionStatusEventConfiguration::default);
        if let Some(requested_lorawan) = &requested.lo_ra_wan {
            let lorawan = target
                .lo_ra_wan
                .get_or_insert_with(LoRaWANConnectionStatusEventNotificationConfigurations::default);
            merge_topic(
                &mut lorawan.gateway_eui_event_topic,
                &requested_lorawan.gateway_eui_event_topic,
            );
        }
        merge_topic(
            &mut target.wireless_gateway_id_event_topic,
            &requested.wireless_gateway_id_event_topic,
        );
    }
}

impl ResourceEventConfiguration {
    /// Take every event-topic status `requested` carries into this record, leaving a
    /// topic it does not name with the status it already had.
    ///
    /// An update names the topics it changes, so merging member by member is what lets a
    /// caller enable one topic without resetting the rest.
    pub fn overlay(&mut self, requested: &ResourceEventConfiguration) {
        merge_device_registration_state(
            &mut self.device_registration_state,
            &requested.device_registration_state,
        );
        merge_proximity(&mut self.proximity, &requested.proximity);
        merge_join(&mut self.join, &requested.join);
        merge_connection_status(&mut self.connection_status, &requested.connection_status);
        merge_message_delivery_status(
            &mut self.message_delivery_status,
            &requested.message_delivery_status,
        );
    }
}

/// What an event-configuration `IdentifierType`/`Identifier` pair addresses once it is
/// resolved against the store, and therefore which event topics that resource publishes.
///
/// Each modelled section names the kind of resource it belongs to through its own
/// resource-id topic member: `DeviceRegistrationState`, `Proximity`, `Join` and
/// `MessageDeliveryStatus` carry a `WirelessDeviceIdEventTopic`, `ConnectionStatus` a
/// `WirelessGatewayIdEventTopic`. So the sections a resource reports follow from what
/// the resource IS, not from which of its identifiers the request used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventConfigurationResource {
    /// A wireless device: the device-registration-state, proximity, join and
    /// message-delivery-status event topics.
    WirelessDevice,
    /// A wireless gateway: the connection-status event topics.
    WirelessGateway,
}

impl EventConfigurationResource {
    /// This resource's event configuration before any update has touched it: every event
    /// topic it publishes at [`EVENT_TOPIC_DISABLED`], and no section at all for a topic
    /// it does not publish.
    pub fn disabled_event_configuration(self) -> ResourceEventConfiguration {
        let disabled = || Some(EVENT_TOPIC_DISABLED.to_string());
        let sidewalk = || {
            Some(SidewalkEventNotificationConfigurations {
                amazon_id_event_topic: disabled(),
            })
        };
        match self {
            Self::WirelessDevice => ResourceEventConfiguration {
                device_registration_state: Some(DeviceRegistrationStateEventConfiguration {
                    sidewalk: sidewalk(),
                    wireless_device_id_event_topic: disabled(),
                }),
                proximity: Some(ProximityEventConfiguration {
                    sidewalk: sidewalk(),
                    wireless_device_id_event_topic: disabled(),
                }),
                join: Some(JoinEventConfiguration {
                    lo_ra_wan: Some(LoRaWANJoinEventNotificationConfigurations {
                        dev_eui_event_topic: disabled(),
                    }),
                    wireless_device_id_event_topic: disabled(),
                }),
                message_delivery_status: Some(MessageDeliveryStatusEventConfiguration {
                    sidewalk: sidewalk(),
                    wireless_device_id_event_topic: disabled(),
                }),
                connection_status: None,
            },
            Self::WirelessGateway => ResourceEventConfiguration {
                connection_status: Some(ConnectionStatusEventConfiguration {
                    lo_ra_wan: Some(LoRaWANConnectionStatusEventNotificationConfigurations {
                        gateway_eui_event_topic: disabled(),
                    }),
                    wireless_gateway_id_event_topic: disabled(),
                }),
                ..ResourceEventConfiguration::default()
            },
        }
    }
}

/// The `ResourceNotFoundException` body for an event-configuration identifier the account
/// holds no resource for, worded like the refusal the addressed resource's own reader
/// gives that identifier.
///
/// `PartnerAccountId` resolves to a partner account, and nothing associates one yet
/// (`AssociateAwsAccountWithPartnerAccount` is still scaffolded), so the store holds
/// none and every partner-account identifier is refused here.
pub fn event_configuration_not_found(
    identifier_type: &str,
    identifier: &str,
) -> ResourceNotFoundException {
    let (resource_type, noun) = match identifier_type {
        "WirelessDeviceId" | "DevEui" => ("WirelessDevice", "wireless device"),
        "WirelessGatewayId" | "GatewayEui" => ("WirelessGateway", "wireless gateway"),
        "PartnerAccountId" => ("PartnerAccount", "partner account"),
        // An identifier kind a later model adds: refused by that kind's own name,
        // rather than claimed to have addressed one of the kinds above.
        other => (other, "resource"),
    };
    ResourceNotFoundException {
        message: Some(format!("No {noun} with {identifier_type} {identifier}")),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The `ValidationException` body for a `Positioning` value outside the modelled enum,
/// and `None` for one [`POSITIONING_CONFIG_STATUSES`] lists.
///
/// One function, so every operation that takes a Device Location integration status —
/// the device's create and its update — refuses an unmodelled value with the same body.
pub fn unmodelled_positioning(positioning: &str) -> Option<ValidationException> {
    (!POSITIONING_CONFIG_STATUSES.contains(&positioning)).then(|| ValidationException {
        message: Some(format!(
            "Positioning must be one of {POSITIONING_CONFIG_STATUSES:?}, got {positioning:?}"
        )),
    })
}

/// Every `DestinationName` a wireless-device request references, in the order a refusal
/// should report them: the device's own destination, the one a Sidewalk positioning
/// configuration routes fixes to, and the one each LoRaWAN application `FPort` routes
/// frames to.
///
/// They are all the same kind of reference to the same resource, so one function collects
/// them for every operation that makes them — the create and the update, whose own input
/// shapes differ — and each is resolved against the destination store rather than only
/// the top-level member.
pub fn referenced_destination_names<'a>(
    own: Option<&'a str>,
    sidewalk_positioning: Option<&'a SidewalkPositioning>,
    applications: Option<&'a [ApplicationConfig]>,
) -> Vec<&'a str> {
    let mut names = Vec::new();
    if let Some(name) = own {
        names.push(name);
    }
    if let Some(name) =
        sidewalk_positioning.and_then(|positioning| positioning.destination_name.as_deref())
    {
        names.push(name);
    }
    for application in applications.unwrap_or_default() {
        if let Some(name) = application.destination_name.as_deref() {
            names.push(name);
        }
    }
    names
}

/// The `FPort` the service assigns a LoRaWAN device's positioning clock-synchronisation
/// service when the create named no assignment for it.
///
/// Source: the assignment the service answers a `GetWirelessDevice` with for a LoRaWAN
/// device whose create named no `FPorts` at all — the request carries no port, the
/// response carries this one, so the value is the service's and not the request's. It is
/// the same port [`DEFAULT_POSITIONING_STREAM_FPORT`] carries: the clock-synchronisation
/// and stream services share one port, each frame naming its own application package
/// inside the payload, while the solver below gets a port of its own. Inside `FPort`'s
/// published `range min 1 max 223`, like every value these members may take
/// (`smithy_query(query="shape", shape="FPorts,Positioning,ApplicationConfig")`).
pub const DEFAULT_POSITIONING_CLOCK_SYNC_FPORT: i32 = 199;

/// The `FPort` the service assigns a LoRaWAN device's GNSS positioning service when the
/// create named no assignment for it, from the same source and inside the same range.
pub const DEFAULT_POSITIONING_GNSS_FPORT: i32 = 198;

/// The `FPort` the service assigns a LoRaWAN device's positioning stream service when the
/// create named no assignment for it, from the same source and inside the same range.
pub const DEFAULT_POSITIONING_STREAM_FPORT: i32 = 199;

/// The `FPort` of the application entry the service assigns a LoRaWAN device when the
/// create named no application assignment, from the same source and inside the same
/// range: the port the positioning payloads the solver below decodes arrive on.
pub const DEFAULT_APPLICATION_FPORT: i32 = 199;

/// The `ApplicationConfigType` of that application entry: the only value
/// `ApplicationConfigType` models
/// ([`crate::ops::types::APPLICATION_CONFIG_TYPE_VALUES`]), the geolocation solver the
/// positioning payloads are decoded by.
pub const DEFAULT_APPLICATION_TYPE: &str = "SemtechGeolocation";

/// The `FPorts` assignment the service provisions a LoRaWAN device with when the create
/// named none of its own.
///
/// `FPorts` is the ports the device's application packages use, and the packages a device
/// runs use ports whether or not the create picked them, so a create that asks for no
/// assignment is provisioned with the service's: the positioning services' ports and the
/// geolocation application the positioning payloads are decoded by. A create that names
/// an `FPorts` block of its own is stored exactly as submitted — the assignment is then
/// the caller's, and no member is added to it — so this is the whole assignment or none of
/// it. Two members no assignment of the service's own fills in stay unset either way:
/// the FUOTA and multicast ports, which the sessions a caller sets up decide, and the
/// top-level clock-sync port.
///
/// The assigned application entry names no `DestinationName`: that member names the
/// position-data destination whose AWS IoT rule processes the device's position data, and
/// an assignment the service made for a create that asked for none has no such rule to
/// name, so the device's own destination carries the frames.
pub fn default_f_ports() -> FPorts {
    FPorts {
        positioning: Some(Positioning {
            clock_sync: Some(DEFAULT_POSITIONING_CLOCK_SYNC_FPORT),
            gnss: Some(DEFAULT_POSITIONING_GNSS_FPORT),
            stream: Some(DEFAULT_POSITIONING_STREAM_FPORT),
        }),
        applications: Some(vec![ApplicationConfig {
            f_port: Some(DEFAULT_APPLICATION_FPORT),
            r#type: Some(DEFAULT_APPLICATION_TYPE.to_string()),
            destination_name: None,
        }]),
        fuota: None,
        multicast: None,
        clock_sync: None,
    }
}

/// The LoRaWAN configuration a created device carries: the members the create named,
/// kept exactly as submitted, and [`default_f_ports`] when it named no `FPorts` at all.
///
/// One function, so the stored record — and therefore every reader of it — carries an
/// assignment whether or not the create spelled one out.
pub fn device_lorawan_settings(requested: &LoRaWANDevice) -> LoRaWANDevice {
    LoRaWANDevice {
        f_ports: Some(requested.f_ports.clone().unwrap_or_else(default_f_ports)),
        ..requested.clone()
    }
}

/// A wireless device registered against a destination.
#[derive(Debug, Clone)]
pub struct WirelessDevice {
    pub id: String,
    pub arn: String,
    /// `WirelessDeviceType`: `LoRaWAN` | `Sidewalk`.
    pub device_type: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub destination_name: String,
    pub lorawan: Option<LoRaWANDevice>,
    pub sidewalk: Option<SidewalkDevice>,
    /// `PositioningConfigStatus`: `Enabled` | `Disabled`. Every device carries one — the
    /// status its create or a later update named, or
    /// [`DEFAULT_POSITIONING_CONFIG_STATUS`] — so every reader reports it.
    pub positioning: String,
    /// Set by `AssociateWirelessDeviceWithThing`, which is still scaffolded, so a
    /// device created today carries none.
    pub thing_name: Option<String>,
    pub thing_arn: Option<String>,
    /// The FUOTA task this device belongs to, set by `AssociateWirelessDeviceWithFuotaTask`
    /// and cleared by `DisassociateWirelessDeviceFromFuotaTask`, so a device created
    /// today belongs to none. `ListWirelessDevices?fuotaTaskId=` selects on it, and a
    /// device that carries one reports a `FuotaDeviceStatus`.
    pub fuota_task_id: Option<String>,
    /// The multicast group this device belongs to, set by
    /// `AssociateWirelessDeviceWithMulticastGroup` and cleared by
    /// `DisassociateWirelessDeviceFromMulticastGroup`.
    /// `ListWirelessDevices?multicastGroupId=` selects on it, and
    /// `GetMulticastGroup` counts the devices that name a group.
    pub multicast_group_id: Option<String>,
    /// Which of this device's event topics are enabled, written by
    /// `UpdateResourceEventConfiguration` and reported by
    /// `GetResourceEventConfiguration`. A created device has configured none, so every
    /// section is unset until an update names it.
    pub event_configuration: ResourceEventConfiguration,
    pub tags: Vec<Tag>,
    /// When the device was registered. Reported by `GetWirelessDeviceStatistics` and
    /// the import-task operations, which are still scaffolded, so no implemented
    /// operation reads it yet.
    pub created_at: SystemTime,
    /// Set when an uplink is received; the uplink path is not emulated, so this stays
    /// `None` rather than reporting traffic that never happened.
    pub last_uplink_received_at: Option<String>,
}

/// A wireless gateway: the LoRaWAN radio configuration of one provisioned gateway.
#[derive(Debug, Clone)]
pub struct WirelessGateway {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The gateway configuration the create carried, reported back by the gateway's
    /// reader. `CreateWirelessGateway` marks `LoRaWAN` required, so every stored gateway
    /// has one.
    pub lorawan: LoRaWANGateway,
    /// Set by `AssociateWirelessGatewayWithThing`, which is still scaffolded, so a
    /// gateway created today carries none.
    pub thing_name: Option<String>,
    pub thing_arn: Option<String>,
    /// Which of this gateway's event topics are enabled, written by
    /// `UpdateResourceEventConfiguration` and reported by
    /// `GetResourceEventConfiguration`, like a device's.
    pub event_configuration: ResourceEventConfiguration,
    pub tags: Vec<Tag>,
}

/// The LoRaWAN configuration a stored gateway carries: the members the request named,
/// kept exactly as submitted, and an empty `Beaconing` container when the request named
/// none at all.
///
/// `Beaconing` is the block of beacon parameters a LoRaWAN gateway transmits with, and
/// the service reports the container for every LoRaWAN gateway — its own members unset
/// while nothing has configured them — so a client can tell "no beacon parameters" from
/// "this field is not reported". One function, so the stored record, and therefore every
/// reader of it, carries the container whether or not the request spelled one out.
pub fn gateway_lorawan_settings(requested: &LoRaWANGateway) -> LoRaWANGateway {
    LoRaWANGateway {
        beaconing: Some(requested.beaconing.clone().unwrap_or_default()),
        ..requested.clone()
    }
}

/// A wireless gateway task definition: the firmware update one gateway task applies.
///
/// `update` is the settings the create carried, reported back unchanged by the
/// definition's reader; `auto_create_tasks` is the modelled flag that says whether the
/// service creates a task for every gateway on the named current version by itself,
/// rather than only when `CreateWirelessGatewayTask` asks for one.
#[derive(Debug, Clone)]
pub struct WirelessGatewayTaskDefinition {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub auto_create_tasks: bool,
    pub update: Option<UpdateWirelessGatewayTaskCreate>,
    pub tags: Vec<Tag>,
}

/// The `ResourceNotFoundException` body for a resource kind this service addresses by an
/// `Id` the account does not hold: the sentence the service words for this error class,
/// with the resource kind and the requested id interpolated, and both optional members
/// carrying those same two values.
///
/// One function, so every resource whose refusal takes this form words it identically;
/// the kinds whose refusal is a different fixed sentence (a wireless gateway, a
/// destination, a wireless device) keep their own.
pub fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Cannot complete the step because resource is not found, with resource type \
             {resource_type}, resource Id {resource_id}"
        )),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The `ValidationException` body for a request that left out a member this service
/// requires, whatever the model marks `@required`: the sentence the service words for
/// this error class, whose only variable part is the name of the omitted member as the
/// model spells it.
///
/// One function, so every operation that requires a member the model leaves optional
/// refuses its absence with the same sentence.
pub fn missing_parameter(member: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("Parameter {member} is required.")),
    }
}

/// The `ResourceNotFoundException` body for a wireless gateway task definition the
/// account does not hold.
///
/// One function, so every operation that addresses a task definition by `Id` — its
/// reader, and the task operations that reference one — refuses a missing one with the
/// same body.
pub fn wireless_gateway_task_definition_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found("WirelessGatewayTaskDefinition", id)
}

/// The `ResourceNotFoundException` body for a device profile the account does not hold.
///
/// One function, so every operation that addresses a device profile by `Id` — its
/// reader and its delete — refuses a missing one with the same body, worded as
/// [`resource_not_found`] words this error class.
pub fn device_profile_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found("DeviceProfile", id)
}

/// A wireless gateway task: the task definition one wireless gateway was asked to apply.
///
/// `GetWirelessGatewayTask` and `DeleteWirelessGatewayTask` address a task by the
/// gateway's `WirelessGatewayId` alone, so a gateway holds at most ONE task; the store
/// keys them by that id for exactly that reason.
#[derive(Debug, Clone)]
pub struct WirelessGatewayTask {
    /// The gateway the task applies to; also this record's key in the store.
    pub wireless_gateway_id: String,
    /// The task definition the task applies, which the create resolved in the store.
    pub wireless_gateway_task_definition_id: String,
    /// `WirelessGatewayTaskStatus`, as [`WIRELESS_GATEWAY_TASK_STATUS_PENDING`]
    /// documents the value a created task carries.
    pub status: String,
    /// When the task was created, reported as `GetWirelessGatewayTask`'s `TaskCreatedAt`.
    pub created_at: SystemTime,
}

/// The `WirelessGatewayTaskStatus` a created gateway task carries: the gateway has not
/// picked the firmware update up yet.
///
/// One of the modelled values
/// [`crate::ops::types::WIRELESS_GATEWAY_TASK_STATUS_VALUES`] lists. The gateway side of
/// the firmware transfer is a physical radio this emulator does not run, so nothing
/// moves the task off this status: a later value would be progress no engine made.
pub const WIRELESS_GATEWAY_TASK_STATUS_PENDING: &str = "PENDING";

/// The `ResourceNotFoundException` body for a wireless gateway the account does not
/// hold.
///
/// Unlike the resource kinds [`resource_not_found`] serves, this refusal is one wholly
/// fixed sentence that interpolates nothing and leaves both optional members unset. One
/// function, so every operation that addresses a gateway — the task operations among
/// them — refuses a missing one with the same body.
pub fn wireless_gateway_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Wireless Gateway doesn't exist".to_string()),
        resource_id: None,
        resource_type: None,
    }
}

/// A stored instant as the `ISODateTimeString` this service's string date-time members
/// carry (`GetWirelessGatewayTask`'s `TaskCreatedAt` among them).
///
/// `aws_smithy_types` formats it, so the emulator hand-writes no calendar arithmetic;
/// `Format::DateTime` is the RFC-3339 form the modelled `ISODateTimeString` pattern
/// admits. `None` only for an instant that format cannot represent.
pub fn iso_date_time(at: SystemTime) -> Option<String> {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// A multicast group: the LoRaWAN downlink settings a set of wireless devices shares.
///
/// The CURRENT membership lives on the devices (`WirelessDevice::multicast_group_id`),
/// so `NumberOfDevicesInGroup` is DERIVED from the device store rather than kept here as
/// a second copy that could disagree with it. What this record does hold is the
/// membership the group was ASKED for ([`Self::requested_devices`]), which the current
/// membership cannot answer because a device that left the group was still requested to
/// join it.
#[derive(Debug, Clone)]
pub struct MulticastGroup {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The LoRaWAN settings the create carried, reported back by the group's reader and
    /// patched member by member by the group's update.
    pub lorawan: LoRaWANMulticast,
    pub tags: Vec<Tag>,
    /// `MulticastGroupStatus`, as [`MULTICAST_GROUP_STATUS_PENDING`] documents the value
    /// a created group carries. Service-assigned lifecycle state, never a request value:
    /// the create and the update take no `Status` member.
    pub status: String,
    /// When the group was created, reported as `GetMulticastGroup`'s `CreatedAt`.
    pub created_at: SystemTime,
    /// The devices whose association with this group has been REQUESTED, counted as
    /// `GetMulticastGroup`'s `LoRaWAN.NumberOfDevicesRequested`.
    ///
    /// An association request records the device here; disassociating it does NOT take it
    /// out, because the request still happened — only the device's own membership is
    /// released, so `NumberOfDevicesInGroup` falls while the number requested stands. A
    /// device that is deleted is pruned from here by the state's
    /// `forget_multicast_group_requests_for`, so nothing counts a device the store no
    /// longer holds, and the set stays bounded by the device store.
    pub requested_devices: BTreeSet<String>,
}

/// The `MulticastGroupStatus` a created multicast group carries: the group exists, and
/// no multicast session has been scheduled for it.
///
/// `MulticastGroupStatus` is a plain string in the model
/// (`smithy_query(query="shape", shape="MulticastGroupStatus")` lists no enum values),
/// so the token is this service's own name for that initial lifecycle state. A group
/// leaves it only when a session is scheduled on it, which the session operations do;
/// they are still scaffolded, so a stored group keeps reporting this and nothing invents
/// progress no session made.
pub const MULTICAST_GROUP_STATUS_PENDING: &str = "Pending";

/// The downlink channel each LoRaWAN regional plan reserves for class B/C downlinks, as
/// `(SupportedRfRegion, DlDr, DlFreq in Hz)` — the plan's second receive window, which
/// is the channel a multicast downlink goes out on.
///
/// One row per value of `SupportedRfRegion`
/// (`smithy_query(query="shape", shape="SupportedRfRegion")`), so every region a group
/// can name has a plan here and none is served by a special case; the const assertion
/// below fails the build if the model ever adds a region this table does not answer for.
///
/// Sources, row by row:
///
/// * each frequency is the plan's published class B/C downlink (RX2) channel — 869.525
///   MHz for EU868, 434.665 MHz for EU433, 505.3 MHz for CN470, 786 MHz for CN779,
///   869.1 MHz for RU864, 921.9 MHz for KR920, 866.55 MHz for IN865, 923.3 MHz for the
///   two 500 kHz plans (US915, AU915), and 923.2 MHz for AS923-1 shifted by that plan's
///   published per-group offsets for AS923-2/3/4 (-1.80, -6.60 and -5.90 MHz);
/// * each data rate is the one that channel is used at in its plan: the lowest data rate
///   (0) in the 125 kHz plans, 2 in the AS923 plans and in IN865, and 9 in the two 500
///   kHz plans, whose downlink-only data rates start above the uplink ones.
///
/// Every row is inside the published ranges of `DlDr` (0..15) and `DlFreq`
/// (100000000..1000000000), which the model declares.
const REGION_DOWNLINK_DEFAULTS: &[(&str, i32, i32)] = &[
    ("EU868", 0, 869_525_000),
    ("US915", 9, 923_300_000),
    ("AU915", 9, 923_300_000),
    ("AS923-1", 2, 923_200_000),
    ("AS923-2", 2, 921_400_000),
    ("AS923-3", 2, 916_600_000),
    ("AS923-4", 2, 917_300_000),
    ("EU433", 0, 434_665_000),
    ("CN470", 0, 505_300_000),
    ("CN779", 0, 786_000_000),
    ("RU864", 0, 869_100_000),
    ("KR920", 0, 921_900_000),
    ("IN865", 2, 866_550_000),
];

/// The table above answers for the WHOLE modelled catalog: one row per
/// `SupportedRfRegion`, checked at compile time against the model-generated value list so
/// a region a later model adds cannot silently fall through to no downlink channel.
const _: () = assert!(
    REGION_DOWNLINK_DEFAULTS.len() == crate::ops::types::SUPPORTED_RF_REGION_VALUES.len()
);

/// Complete a multicast group's `DefaultSessionParameters` the way the service does when
/// a write leaves them out: each member the request did not supply takes the value of
/// the group's RF region's downlink channel, so a stored group always carries the whole
/// downlink session plan a multicast session needs and its reader never answers a null
/// data rate or frequency.
///
/// A member the request DID supply is left exactly as submitted. A group that names no
/// RF region gets nothing: there is no regional plan to take a legal downlink channel
/// from, and a frequency invented without one would be a channel no device listens on.
pub fn complete_default_session_parameters(lorawan: &mut LoRaWANMulticast) {
    let Some((_, dl_dr, dl_freq)) = REGION_DOWNLINK_DEFAULTS
        .iter()
        .find(|(region, _, _)| Some(*region) == lorawan.rf_region.as_deref())
    else {
        return;
    };
    let parameters = lorawan.default_session_parameters.get_or_insert_default();
    parameters.dl_dr = parameters.dl_dr.or(Some(*dl_dr));
    parameters.dl_freq = parameters.dl_freq.or(Some(*dl_freq));
}

/// How many hertz one unit of a device profile's frequency members is: their modelled
/// ranges (`RxFreq2` and `PresetFreq`, 1000000..16700000) count in hundreds of hertz,
/// while [`REGION_DOWNLINK_DEFAULTS`] above — a multicast group's `DlFreq`, modelled
/// 100000000..1000000000 — counts in hertz, so the one published channel serves both
/// members through this conversion rather than being written down twice.
const HERTZ_PER_DEVICE_PROFILE_FREQUENCY: i32 = 100;

/// The part of each LoRaWAN regional plan a DEVICE PROFILE reports, as
/// `(SupportedRfRegion, second receive window's data rate, default join channels)` with
/// every frequency in the hundreds-of-hertz unit the device-profile members use.
///
/// One row per value of `SupportedRfRegion`
/// (`smithy_query(query="shape", shape="SupportedRfRegion")`), like
/// [`REGION_DOWNLINK_DEFAULTS`], so every band a profile can name is answered from the
/// catalog and none is served by a special case; the const assertion below fails the
/// build if the model adds a band this table does not answer for.
///
/// Sources, column by column:
///
/// * the data rate is the one each plan publishes for its second receive window: the
///   lowest data rate (0) in the 125 kHz plans, 2 in the AS923 plans and in IN865, and 8
///   — the first downlink-only data rate — in the two 500 kHz plans (US915, AU915).
///   `REGION_DOWNLINK_DEFAULTS` carries a different value for those two because it
///   answers for a class B/C multicast session rather than a device's RX2 window;
/// * the channels are the plan's published default (join) channels: 868.1/868.3/868.5
///   MHz for EU868, 923.2/923.4 MHz for AS923-1 shifted by that plan's published
///   per-group offsets for AS923-2/3/4 (-1.80, -6.60 and -5.90 MHz), 433.175/433.375/
///   433.575 MHz for EU433, 779.5/779.7/779.9 MHz for CN779, 868.9/869.1 MHz for RU864,
///   922.1/922.3/922.5 MHz for KR920, 865.0625/865.4025/865.985 MHz for IN865, and for
///   the two 500 kHz plans the eight 125 kHz channels of the sub-band this service
///   operates them on plus that sub-band's 500 kHz channel (903.9–905.3 MHz and 904.6
///   MHz for US915; 916.8–918.2 MHz and 917.5 MHz for AU915).
/// * CN470 has NO row of channels: its published default channel set is far longer than
///   the twenty entries `FactoryPresetFreqsList` models (`@length 0..20`), so a profile
///   in that band reports no channel list rather than a truncated plan no device
///   follows.
///
/// Every data rate is inside `RxDataRate2`'s published range (0..15) and every channel
/// inside `PresetFreq`'s (1000000..16700000).
const REGION_DEVICE_PROFILE_PLANS: &[(&str, i32, &[i32])] = &[
    ("EU868", 0, &[8_681_000, 8_683_000, 8_685_000]),
    (
        "US915",
        8,
        &[
            9_039_000, 9_041_000, 9_043_000, 9_045_000, 9_047_000, 9_049_000, 9_051_000,
            9_053_000, 9_046_000,
        ],
    ),
    (
        "AU915",
        8,
        &[
            9_168_000, 9_170_000, 9_172_000, 9_174_000, 9_176_000, 9_178_000, 9_180_000,
            9_182_000, 9_175_000,
        ],
    ),
    ("AS923-1", 2, &[9_232_000, 9_234_000]),
    ("AS923-2", 2, &[9_214_000, 9_216_000]),
    ("AS923-3", 2, &[9_166_000, 9_168_000]),
    ("AS923-4", 2, &[9_173_000, 9_175_000]),
    ("EU433", 0, &[4_331_750, 4_333_750, 4_335_750]),
    ("CN470", 0, &[]),
    ("CN779", 0, &[7_795_000, 7_797_000, 7_799_000]),
    ("RU864", 0, &[8_689_000, 8_691_000]),
    ("KR920", 0, &[9_221_000, 9_223_000, 9_225_000]),
    ("IN865", 2, &[8_650_625, 8_654_025, 8_659_850]),
];

/// The table above answers for the WHOLE modelled catalog, checked at compile time
/// against the model-generated value list so a band a later model adds cannot silently
/// fall through to no plan.
const _: () = assert!(
    REGION_DEVICE_PROFILE_PLANS.len() == crate::ops::types::SUPPORTED_RF_REGION_VALUES.len()
);

/// What one band's regional plan decides about a device profile: the second receive
/// window's data rate and frequency, and the band's default join channels.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfilePlan {
    /// `RxDataRate2`: the data rate the second receive window uses in this band.
    pub rx_data_rate2: i32,
    /// `RxFreq2`: that window's frequency, in the hundreds-of-hertz unit the member is
    /// modelled in.
    pub rx_freq2: i32,
    /// `FactoryPresetFreqsList`: the band's default join channels, in the same unit.
    /// EMPTY for a band whose published set does not fit the modelled list.
    pub join_channels: &'static [i32],
}

/// The plan `rf_region` names, or `None` for a band this service holds no plan for —
/// `RfRegion` is a free-form string in the model (`length 0..64`), so a profile may name
/// one the regional catalog does not list, and inventing a channel for it would report a
/// frequency no device listens on.
pub fn region_device_profile_plan(rf_region: &str) -> Option<DeviceProfilePlan> {
    let (_, rx_data_rate2, join_channels) = REGION_DEVICE_PROFILE_PLANS
        .iter()
        .find(|(region, _, _)| *region == rf_region)?;
    let (_, _, rx2_hertz) = REGION_DOWNLINK_DEFAULTS
        .iter()
        .find(|(region, _, _)| *region == rf_region)?;
    Some(DeviceProfilePlan {
        rx_data_rate2: *rx_data_rate2,
        rx_freq2: rx2_hertz / HERTZ_PER_DEVICE_PROFILE_FREQUENCY,
        join_channels,
    })
}

/// A FUOTA task: the firmware image a set of wireless devices is updated with, and the
/// fragmentation settings the update is sent down with.
///
/// The membership itself lives on the devices (`WirelessDevice::fuota_task_id`), like a
/// multicast group's, so nothing here is a second copy of it.
#[derive(Debug, Clone)]
pub struct FuotaTask {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The LoRaWAN settings the create carried (`RfRegion`), reported back by the
    /// task's reader.
    pub lorawan: Option<LoRaWANFuotaTask>,
    /// The S3 URI of the firmware image. `CreateFuotaTask` marks it required, so every
    /// stored task has one.
    pub firmware_update_image: String,
    /// The role the firmware update is applied under. Required, like the image.
    pub firmware_update_role: String,
    /// The task's EFFECTIVE fragmentation settings: the values the create carried, or
    /// the service defaults ([`DEFAULT_FUOTA_REDUNDANCY_PERCENT`],
    /// [`DEFAULT_FUOTA_FRAGMENT_SIZE_BYTES`], [`DEFAULT_FUOTA_FRAGMENT_INTERVAL_MS`])
    /// it applied for the ones the create omitted. Not optional, because a stored task
    /// always has a transfer plan to report.
    pub redundancy_percent: i32,
    pub fragment_size_bytes: i32,
    pub fragment_interval_ms: i32,
    /// The base64 file descriptor the create carried, reported back unchanged.
    pub descriptor: Option<String>,
    pub tags: Vec<Tag>,
    /// `FuotaTaskStatus`, as [`FUOTA_TASK_STATUS_PENDING`] documents the value a
    /// created task carries.
    pub status: String,
    /// When the task was created, reported as `GetFuotaTask`'s `CreatedAt`.
    pub created_at: SystemTime,
    /// When the FUOTA session started, reported as `GetFuotaTask`'s `LoRaWAN.StartTime`.
    /// A session starts only on `StartFuotaTask`, which is still scaffolded, so a task
    /// created today has none.
    pub start_time: Option<SystemTime>,
}

/// The `FuotaTaskStatus` a created FUOTA task carries: nothing has asked for its
/// session yet, so it is waiting to be started.
///
/// One of the modelled values [`crate::ops::types::FUOTA_TASK_STATUS_VALUES`] lists;
/// the task leaves it only when an operation moves it (the session operations are still
/// scaffolded).
pub const FUOTA_TASK_STATUS_PENDING: &str = "Pending";

/// The `RedundancyPercent` a FUOTA task is created with when the request names none:
/// half the fragments of the image are added as redundant ones, which is the value the
/// service is observed to report for a task whose create pinned none. One of the three
/// fragmentation defaults the service applies at create time, so a task that pinned
/// none still reports a complete transfer plan. Inside the member's published
/// `range 0..100`.
pub const DEFAULT_FUOTA_REDUNDANCY_PERCENT: i32 = 50;

/// The `FragmentSizeBytes` a FUOTA task is created with when the request names none:
/// the fragment size the service is observed to report for a task whose create pinned
/// none. Inside the published `range min 1`, like every value this member may take.
pub const DEFAULT_FUOTA_FRAGMENT_SIZE_BYTES: i32 = 50;

/// The `FragmentIntervalMS` a FUOTA task is created with when the request names none:
/// one second between fragments, which is the granularity the member's own
/// documentation gives it ("rounded to the nearest second").
pub const DEFAULT_FUOTA_FRAGMENT_INTERVAL_MS: i32 = 1_000;

/// The `FuotaDeviceStatus` a wireless device carries the moment it joins a FUOTA task:
/// it is a member, and no FUOTA session has sent it a fragment yet.
///
/// One of the modelled values [`crate::ops::types::FUOTA_DEVICE_STATUS_VALUES`] lists;
/// the device leaves it only once a session reports progress for it.
pub const FUOTA_DEVICE_STATUS_INITIAL: &str = "Initial";

/// The sentence this service's 404 carries when a wireless device a request addresses is
/// not one the account holds: fixed text, with no part of it taken from the request, so a
/// client can match it whichever identifier kind failed to resolve.
pub const WIRELESS_DEVICE_NOT_FOUND_MESSAGE: &str = "WirelessDevice record not found";

/// The `ResourceType` a wireless device's refusals name.
pub const WIRELESS_DEVICE_RESOURCE_TYPE: &str = "WirelessDevice";

/// The `ResourceNotFoundException` body for a wireless device the account does not hold,
/// addressed by `identifier`.
///
/// The sentence is [`WIRELESS_DEVICE_NOT_FOUND_MESSAGE`], one wholly fixed text, while
/// the two optional members report the identifier the request addressed the device by and
/// the kind of resource it addressed. One function, so every operation that addresses a
/// wireless device — its reader, its update, its delete, and the association operations
/// that resolve one — refuses a missing one with the same body.
pub fn wireless_device_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(WIRELESS_DEVICE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(WIRELESS_DEVICE_RESOURCE_TYPE.to_string()),
    }
}

/// The `ResourceNotFoundException` body for a FUOTA task the account does not hold,
/// worded by [`resource_not_found`] — the sentence this service refuses a missing
/// resource of this kind with, carrying the resource type and the requested `Id`.
///
/// One function, so every operation that addresses a FUOTA task by `Id` refuses a
/// missing one with the same body.
pub fn fuota_task_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found(FUOTA_TASK_RESOURCE_TYPE, id)
}

/// The `ResourceType` a FUOTA task's refusals name.
pub const FUOTA_TASK_RESOURCE_TYPE: &str = "FuotaTask";

/// The `ResourceNotFoundException` body for a multicast group the account does not hold.
///
/// One function, so every operation that addresses a multicast group by `Id` — its
/// reader, and the association operations that resolve one — refuses a missing one with
/// the same body.
pub fn multicast_group_not_found(id: &str) -> ResourceNotFoundException {
    resource_not_found("MulticastGroup", id)
}

/// A network analyzer configuration: the trace settings, and the wireless devices,
/// wireless gateways and multicast groups whose frames the configuration tracks.
///
/// Keyed by `Name` — the `@httpLabel` every reader, updater and delete addresses it by.
/// The three tracked-resource lists hold the ids in the order they were first added, so
/// a reader reports them in the order the caller built them up in, and each id appears
/// at most once however many times it is added.
///
/// `description` and `trace_content` are NOT optional, because a stored configuration is
/// fully specified: the create settles every trace setting the request left out at that
/// setting's default ([`stored_trace_content`]) and an unset description at the empty
/// string, so every later read reports all of them and a get/modify/update round trip
/// cannot lose a setting the caller never saw.
#[derive(Debug, Clone, Default)]
pub struct NetworkAnalyzerConfiguration {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub trace_content: TraceContent,
    /// `WirelessDeviceId`s this configuration tracks.
    pub wireless_devices: Vec<String>,
    /// `WirelessGatewayId`s this configuration tracks.
    pub wireless_gateways: Vec<String>,
    /// `MulticastGroupId`s this configuration tracks.
    pub multicast_groups: Vec<String>,
    pub tags: Vec<Tag>,
}

/// The setting a trace-content member carries when the request that stored the
/// configuration left it unset: tracing OFF.
///
/// It is the one value all three members share:
/// `smithy_query(query="shape", shape="TraceContent")` targets `WirelessDeviceFrameInfo`
/// (ENABLED | DISABLED), `LogLevel` (INFO | ERROR | DISABLED) and `MulticastFrameInfo`
/// (ENABLED | DISABLED), so a setting nobody asked for traces nothing.
pub const TRACE_SETTING_DISABLED: &str = "DISABLED";

/// The trace content a configuration STORES for the `TraceContent` a create carried:
/// every member the request set, and [`TRACE_SETTING_DISABLED`] for every member it left
/// unset — including the case of a request that carried no `TraceContent` at all.
///
/// A stored configuration therefore always carries all three settings, so its reader
/// reports a fully specified trace content and a get/modify/update round trip cannot drop
/// a setting the caller never saw.
pub fn stored_trace_content(requested: Option<&TraceContent>) -> TraceContent {
    let requested = requested.cloned().unwrap_or_default();
    let settled = |member: Option<String>| Some(member.unwrap_or_else(default_trace_setting));
    TraceContent {
        wireless_device_frame_info: settled(requested.wireless_device_frame_info),
        log_level: settled(requested.log_level),
        multicast_frame_info: settled(requested.multicast_frame_info),
    }
}

/// [`TRACE_SETTING_DISABLED`] as an owned value.
fn default_trace_setting() -> String {
    TRACE_SETTING_DISABLED.to_string()
}

/// The `ResourceNotFoundException` body for a network analyzer configuration the account
/// does not hold.
///
/// One function, so every operation that addresses a configuration by `Name` — its
/// reader, its updater and its delete — refuses a missing one with the same body: the
/// sentence this service words this error class with around the `ConfigurationName` the
/// request addressed, which is its only variable part, and the two optional members the
/// model merely permits (`ResourceId`, `ResourceType`) left unset, which is how this
/// service answers the configuration lookup.
pub fn network_analyzer_configuration_not_found(name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Configuration {name} is not exist")),
        resource_id: None,
        resource_type: None,
    }
}

/// The most wireless devices one network analyzer configuration tracks.
///
/// The ceiling the model publishes for the member that carries them:
/// `smithy_query(query="shape", shape="WirelessDeviceList")` is `length 0..250`, and
/// `GetNetworkAnalyzerConfiguration` reports the tracked devices through that very list
/// — so a stored set past this could not be reported.
pub const MAX_TRACKED_WIRELESS_DEVICES: usize = 250;

/// The most multicast groups one network analyzer configuration tracks, for the same
/// reason as [`MAX_TRACKED_WIRELESS_DEVICES`]:
/// `smithy_query(query="shape", shape="NetworkAnalyzerMulticastGroupList")` is
/// `length 0..10`.
pub const MAX_TRACKED_MULTICAST_GROUPS: usize = 10;

/// The most wireless gateways one network analyzer configuration tracks.
///
/// `smithy_query(query="shape", shape="WirelessGatewayList")` publishes no length for
/// the member that carries them, so the bound is the one the store itself implies: a
/// configuration tracks gateways this account holds, and the account holds at most
/// [`MAX_WIRELESS_GATEWAYS`] of them.
pub const MAX_TRACKED_WIRELESS_GATEWAYS: usize = MAX_WIRELESS_GATEWAYS;

/// Add every id in `additions` the list does not already carry, keeping the order ids
/// were first added in.
///
/// One function, so the create (which starts from an empty list) and the update (which
/// adds to a stored one) build a tracked-resource list the same way, and neither can
/// store an id twice.
pub fn add_tracked_ids(list: &mut Vec<String>, additions: &[String]) {
    for id in additions {
        if !list.iter().any(|held| held == id) {
            list.push(id.clone());
        }
    }
}

/// Drop every id in `removals` from the list, leaving the rest in order.
///
/// Removing an id a configuration does not track is a no-op: the request asks for a
/// state the configuration is already in.
pub fn remove_tracked_ids(list: &mut Vec<String>, removals: &[String]) {
    list.retain(|held| !removals.iter().any(|id| id == held));
}

/// The `ValidationException` for a tracked-resource list that would pass the ceiling the
/// model publishes for the member reporting it, or `None` when it fits.
pub fn tracked_list_over_limit(
    member: &str,
    len: usize,
    max: usize,
) -> Option<ValidationException> {
    (len > max).then(|| ValidationException {
        message: Some(format!(
            "a network analyzer configuration tracks at most {max} entries in {member}; \
             this request would track {len}"
        )),
    })
}

/// What an `@idempotencyToken` create remembers: which resource the token minted, and
/// a fingerprint of the parameters it was minted with.
///
/// `CreateDestination`/`CreateWirelessDevice` document that replaying a token with the
/// SAME parameters completes successfully, while replaying it with DIFFERENT parameters
/// is an HTTP 409 conflict — which needs both halves.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The key of the resource the first call created (a destination name, a device id).
    pub resource_key: String,
    /// Fingerprint of the create parameters, the token excluded.
    pub fingerprint: String,
}

/// A tag set as one comparable, order-independent sequence of `(key, value)` pairs.
///
/// Every `@idempotencyToken` create fingerprints its parameters to decide whether a
/// replayed token carries "the same parameters", and a tag set is unordered on the wire,
/// so two requests that differ only in tag order carry the same parameters. One
/// function, so every create orders them the same way.
pub fn sorted_tag_pairs(tags: &[Tag]) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs
}

/// The most destinations this emulator stores.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop cannot
/// grow the emulator's heap without bound. Neither create operation models a quota
/// error (`smithy_query(query="operation", operation="CreateDestination")` lists
/// AccessDenied / Conflict / InternalServer / ResourceNotFound / Throttling /
/// Validation), so a full store refuses with `ValidationException`.
pub const MAX_DESTINATIONS: usize = 100;

/// The most wireless devices this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`].
pub const MAX_WIRELESS_DEVICES: usize = 250;

/// The most device profiles this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateDeviceProfile` models no quota error either
/// (`smithy_query(query="operation", operation="CreateDeviceProfile")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_DEVICE_PROFILES: usize = 100;

/// The most service profiles this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateServiceProfile` models no quota error either
/// (`smithy_query(query="operation", operation="CreateServiceProfile")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_SERVICE_PROFILES: usize = 100;

/// The most multicast groups this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateMulticastGroup` models no quota error either
/// (`smithy_query(query="operation", operation="CreateMulticastGroup")` lists
/// AccessDenied / Conflict / InternalServer / ResourceNotFound / Throttling /
/// Validation), so a full store refuses with `ValidationException`.
pub const MAX_MULTICAST_GROUPS: usize = 100;

/// The most FUOTA tasks this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateFuotaTask` models no quota error either
/// (`smithy_query(query="operation", operation="CreateFuotaTask")` lists AccessDenied /
/// Conflict / InternalServer / ResourceNotFound / Throttling / Validation), so a full
/// store refuses with `ValidationException`.
pub const MAX_FUOTA_TASKS: usize = 100;

/// The most `ClientRequestToken` entries kept for `CreateFuotaTask` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_FUOTA_TASK_TOKENS: usize = MAX_FUOTA_TASKS;

/// The most wireless gateways this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateWirelessGateway` models no quota error either
/// (`smithy_query(query="operation", operation="CreateWirelessGateway")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_WIRELESS_GATEWAYS: usize = 100;

/// The most wireless gateway task definitions this emulator stores. A ceiling, for the
/// same reason as [`MAX_DESTINATIONS`]; `CreateWirelessGatewayTaskDefinition` models no
/// quota error either (`smithy_query(query="operation",
/// operation="CreateWirelessGatewayTaskDefinition")` lists AccessDenied / Conflict /
/// InternalServer / ResourceNotFound / Throttling / Validation), so a full store refuses
/// with `ValidationException`.
pub const MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS: usize = 100;

/// The most `ClientRequestToken` entries kept for
/// `CreateWirelessGatewayTaskDefinition` idempotency. Bounded for the same reason as
/// [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS: usize =
    MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS;

/// The most `ClientRequestToken` entries kept for `CreateDestination` idempotency.
///
/// At most one token is recorded per stored destination and a token is dropped with the
/// destination it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the resources they name.
pub const MAX_DESTINATION_TOKENS: usize = MAX_DESTINATIONS;

/// The most `ClientRequestToken` entries kept for `CreateWirelessDevice` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_DEVICE_TOKENS: usize = MAX_WIRELESS_DEVICES;

/// The most `ClientRequestToken` entries kept for `CreateDeviceProfile` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_DEVICE_PROFILE_TOKENS: usize = MAX_DEVICE_PROFILES;

/// The most `ClientRequestToken` entries kept for `CreateServiceProfile` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_SERVICE_PROFILE_TOKENS: usize = MAX_SERVICE_PROFILES;

/// The most `ClientRequestToken` entries kept for `CreateMulticastGroup` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_MULTICAST_GROUP_TOKENS: usize = MAX_MULTICAST_GROUPS;

/// The most `ClientRequestToken` entries kept for `CreateWirelessGateway` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_GATEWAY_TOKENS: usize = MAX_WIRELESS_GATEWAYS;

/// The most network analyzer configurations this emulator stores. A ceiling, for the
/// same reason as [`MAX_DESTINATIONS`]; `CreateNetworkAnalyzerConfiguration` models no
/// quota error either (`smithy_query(query="operation",
/// operation="CreateNetworkAnalyzerConfiguration")` lists AccessDenied / Conflict /
/// InternalServer / ResourceNotFound / Throttling / Validation), so a full store refuses
/// with `ValidationException`.
pub const MAX_NETWORK_ANALYZER_CONFIGURATIONS: usize = 100;

/// The most `ClientRequestToken` entries kept for
/// `CreateNetworkAnalyzerConfiguration` idempotency. Bounded for the same reason as
/// [`MAX_DESTINATION_TOKENS`].
pub const MAX_NETWORK_ANALYZER_CONFIGURATION_TOKENS: usize = MAX_NETWORK_ANALYZER_CONFIGURATIONS;

/// The resource kinds a log-level override addresses.
///
/// `ResourceType` is an unconstrained string in the model
/// (`smithy_query(query="shape", shape="ResourceType")`), so this set comes from the
/// member's own documentation — "The type of resource, which can be WirelessDevice,
/// WirelessGateway, or FuotaTask" — and is listed in that order.
pub const LOG_LEVEL_RESOURCE_TYPES: &[&str] = &["WirelessDevice", "WirelessGateway", "FuotaTask"];

/// The most log-level overrides one account holds, as `PutResourceLogLevel`'s own
/// documentation states the limit: "A limit of 200 log level override can be set per
/// account."
pub const MAX_RESOURCE_LOG_LEVEL_OVERRIDES: usize = 200;

/// The `ValidationException` body for a `ResourceType` outside
/// [`LOG_LEVEL_RESOURCE_TYPES`].
///
/// The model leaves `ResourceType` unconstrained, so the boundary validator cannot
/// refuse a value outside the documented set and every log-level operation refuses it
/// here, through this one function, with the sentence template this service words a
/// refused member value with.
pub fn unsupported_log_level_resource_type(resource_type: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "1 validation error detected: Value '{resource_type}' at 'resourceType' failed to \
             satisfy constraint: Member must satisfy enum value set: [{}]",
            LOG_LEVEL_RESOURCE_TYPES.join(", ")
        )),
    }
}

/// The `ResourceNotFoundException` body for the resource a log-level override would
/// apply to, when the account holds no such resource.
///
/// Each resource kind keeps the refusal its own operations already word, so a log-level
/// call refuses a missing wireless device, wireless gateway or FUOTA task exactly as the
/// operations that address it directly do.
pub fn log_level_resource_not_found(
    resource_type: &str,
    resource_identifier: &str,
) -> ResourceNotFoundException {
    match resource_type {
        "WirelessDevice" => wireless_device_not_found(resource_identifier),
        "WirelessGateway" => wireless_gateway_not_found(),
        "FuotaTask" => fuota_task_not_found(resource_identifier),
        other => resource_not_found(other, resource_identifier),
    }
}

/// Per-service state for com-amazonaws-iotwireless-iotwireless. A handler reaches it through [`service_state`].
///
/// The resource maps are `BTreeMap`s so every list enumerates in one stable, sorted key
/// order — which is what makes a pagination token that names the last key returned
/// resume exactly where the previous page stopped.
///
/// Every store here is BOUNDED: the resource maps by [`MAX_DESTINATIONS`] /
/// [`MAX_WIRELESS_DEVICES`] / [`MAX_DEVICE_PROFILES`] / [`MAX_SERVICE_PROFILES`] /
/// [`MAX_MULTICAST_GROUPS`] / [`MAX_FUOTA_TASKS`] /
/// [`MAX_WIRELESS_GATEWAYS`] / [`MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS`] /
/// [`MAX_NETWORK_ANALYZER_CONFIGURATIONS`] (the
/// creates refuse once full), `dev_eui_index` by one entry per stored device,
/// `gateway_eui_index` and `wireless_gateway_tasks` by one entry per stored gateway,
/// and the
/// token maps by [`MAX_DESTINATION_TOKENS`] / [`MAX_WIRELESS_DEVICE_TOKENS`] /
/// [`MAX_DEVICE_PROFILE_TOKENS`] / [`MAX_SERVICE_PROFILE_TOKENS`] /
/// [`MAX_MULTICAST_GROUP_TOKENS`] / [`MAX_FUOTA_TASK_TOKENS`] /
/// [`MAX_WIRELESS_GATEWAY_TOKENS`] /
/// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`] /
/// [`MAX_NETWORK_ANALYZER_CONFIGURATION_TOKENS`] with oldest-entry
/// eviction. The token maps are private so every write goes through
/// [`Self::record_destination_token`] / [`Self::record_wireless_device_token`] /
/// [`Self::record_device_profile_token`] / [`Self::record_service_profile_token`] /
/// [`Self::record_multicast_group_token`] / [`Self::record_fuota_task_token`] /
/// [`Self::record_wireless_gateway_token`] /
/// [`Self::record_wireless_gateway_task_definition_token`] /
/// [`Self::record_network_analyzer_configuration_token`] and
/// cannot bypass the cap. The log-level override map is bounded by
/// [`MAX_RESOURCE_LOG_LEVEL_OVERRIDES`] and private for the same reason.
#[derive(Debug, Default)]
pub struct ComAmazonawsIotwirelessIotwirelessState {
    /// Destinations by `Name`. Bounded by [`MAX_DESTINATIONS`].
    pub destinations: BTreeMap<String, Destination>,
    /// Wireless devices by `Id`. Bounded by [`MAX_WIRELESS_DEVICES`].
    pub wireless_devices: BTreeMap<String, WirelessDevice>,
    /// Device profiles by `Id`. Bounded by [`MAX_DEVICE_PROFILES`].
    pub device_profiles: BTreeMap<String, DeviceProfile>,
    /// Service profiles by `Id`. Bounded by [`MAX_SERVICE_PROFILES`].
    pub service_profiles: BTreeMap<String, ServiceProfile>,
    /// Multicast groups by `Id`. Bounded by [`MAX_MULTICAST_GROUPS`].
    pub multicast_groups: BTreeMap<String, MulticastGroup>,
    /// FUOTA tasks by `Id`. Bounded by [`MAX_FUOTA_TASKS`].
    pub fuota_tasks: BTreeMap<String, FuotaTask>,
    /// Wireless gateways by `Id`. Bounded by [`MAX_WIRELESS_GATEWAYS`].
    pub wireless_gateways: BTreeMap<String, WirelessGateway>,
    /// Wireless gateway task definitions by `Id`. Bounded by
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS`].
    pub wireless_gateway_task_definitions: BTreeMap<String, WirelessGatewayTaskDefinition>,
    /// The task each wireless gateway is running, by `WirelessGatewayId`. A gateway holds
    /// at most one task and the create resolves the gateway first, so this map is bounded
    /// by [`MAX_WIRELESS_GATEWAYS`] — one entry per stored gateway — rather than by a
    /// ceiling of its own.
    pub wireless_gateway_tasks: BTreeMap<String, WirelessGatewayTask>,
    /// `GatewayEui` -> gateway id, so `GetWirelessGateway?identifierType=GatewayEui`
    /// resolves without scanning and a duplicate `GatewayEui` is refusable. At most one
    /// entry per stored gateway.
    pub gateway_eui_index: BTreeMap<String, String>,
    /// `DevEui` -> device id, so `GetWirelessDevice?identifierType=DevEui` resolves
    /// without scanning and a duplicate `DevEui` is refusable. At most one entry per
    /// stored device, and the entry is released when the device is deleted.
    pub dev_eui_index: BTreeMap<String, String>,
    /// `CreateDestination` client request tokens, bounded by
    /// [`MAX_DESTINATION_TOKENS`]. Private: write through
    /// [`Self::record_destination_token`], read through [`Self::destination_token`].
    destination_tokens: HashMap<String, IdempotentCreate>,
    /// The order [`Self::destination_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    destination_token_order: VecDeque<String>,
    /// `CreateWirelessDevice` client request tokens, bounded by
    /// [`MAX_WIRELESS_DEVICE_TOKENS`]. Private, like the destination tokens.
    wireless_device_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_device_tokens`].
    wireless_device_token_order: VecDeque<String>,
    /// `CreateDeviceProfile` client request tokens, bounded by
    /// [`MAX_DEVICE_PROFILE_TOKENS`]. Private, like the destination tokens.
    device_profile_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::device_profile_tokens`].
    device_profile_token_order: VecDeque<String>,
    /// `CreateServiceProfile` client request tokens, bounded by
    /// [`MAX_SERVICE_PROFILE_TOKENS`]. Private, like the destination tokens.
    service_profile_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::service_profile_tokens`].
    service_profile_token_order: VecDeque<String>,
    /// `CreateMulticastGroup` client request tokens, bounded by
    /// [`MAX_MULTICAST_GROUP_TOKENS`]. Private, like the destination tokens.
    multicast_group_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::multicast_group_tokens`].
    multicast_group_token_order: VecDeque<String>,
    /// `CreateFuotaTask` client request tokens, bounded by [`MAX_FUOTA_TASK_TOKENS`].
    /// Private, like the destination tokens.
    fuota_task_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::fuota_task_tokens`].
    fuota_task_token_order: VecDeque<String>,
    /// `CreateWirelessGateway` client request tokens, bounded by
    /// [`MAX_WIRELESS_GATEWAY_TOKENS`]. Private, like the destination tokens.
    wireless_gateway_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_gateway_tokens`].
    wireless_gateway_token_order: VecDeque<String>,
    /// `CreateWirelessGatewayTaskDefinition` client request tokens, bounded by
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`]. Private, like the destination
    /// tokens.
    wireless_gateway_task_definition_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_gateway_task_definition_tokens`].
    wireless_gateway_task_definition_token_order: VecDeque<String>,
    /// Network analyzer configurations by `Name`. Bounded by
    /// [`MAX_NETWORK_ANALYZER_CONFIGURATIONS`].
    pub network_analyzer_configurations: BTreeMap<String, NetworkAnalyzerConfiguration>,
    /// `CreateNetworkAnalyzerConfiguration` client request tokens, bounded by
    /// [`MAX_NETWORK_ANALYZER_CONFIGURATION_TOKENS`]. Private, like the destination
    /// tokens.
    network_analyzer_configuration_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::network_analyzer_configuration_tokens`].
    network_analyzer_configuration_token_order: VecDeque<String>,
    /// The log-level override each resource carries: `ResourceType` -> that type's
    /// `ResourceIdentifier` -> `LogLevel`. Nested so a lookup resolves a pair without
    /// building a key, and bounded by [`MAX_RESOURCE_LOG_LEVEL_OVERRIDES`] across every
    /// type.
    ///
    /// Private: read through [`Self::resource_log_level`] and write through
    /// [`Self::set_resource_log_level`] / [`Self::clear_resource_log_level`], so nothing
    /// bypasses that ceiling or leaves an emptied type behind.
    resource_log_levels: BTreeMap<String, BTreeMap<String, String>>,
}

/// Record `token` in `tokens`, evicting the oldest recorded token first if that would
/// push the map past `cap`.
///
/// Shared by every token map so the caps are enforced by one piece of code: the
/// order queue and the map are only ever written together, here.
///
/// The cap cannot bind in practice, which is why evicting the oldest entry is a safe
/// last resort rather than a behaviour change: at most one token is recorded per stored
/// resource, a resource's tokens are dropped with it where its delete is implemented
/// (`forget_*_tokens_for`, which the destination, wireless-device, device-profile,
/// service-profile, multicast-group, FUOTA-task, wireless-gateway,
/// wireless-gateway-task-definition and network-analyzer-configuration deletes call),
/// and each
/// token cap equals its resource ceiling — so the map can only reach `cap` when every
/// entry in it names a resource the store accepted and the resource store is itself
/// full, which the create already refuses before it gets here. Eviction exists so the
/// bound holds whatever a future caller does; if it ever did evict a live resource's
/// token, the consequence is only that a replay of that token is treated as a fresh
/// create (a 409 for a destination whose name is taken, a second device otherwise).
fn record_token(
    tokens: &mut HashMap<String, IdempotentCreate>,
    order: &mut VecDeque<String>,
    cap: usize,
    token: String,
    record: IdempotentCreate,
) {
    // Only a NEW token grows the map, so re-recording an existing one must not evict a
    // live entry — least of all the token being re-recorded.
    if !tokens.contains_key(&token) {
        while tokens.len() >= cap {
            let Some(oldest) = order.pop_front() else { break };
            tokens.remove(&oldest);
        }
    }
    if tokens.insert(token.clone(), record).is_none() {
        order.push_back(token);
    }
}

/// Drop every token in `tokens` that minted `resource_key`.
fn forget_tokens_for(
    tokens: &mut HashMap<String, IdempotentCreate>,
    order: &mut VecDeque<String>,
    resource_key: &str,
) {
    tokens.retain(|_, record| record.resource_key != resource_key);
    let live = &*tokens;
    order.retain(|token| live.contains_key(token));
}

/// A fresh v4 uuid `existing` does not already hold.
///
/// Every resource id this service mints (`WirelessDeviceId`, `DeviceProfileId`) is a
/// uuid, so they all come from the `uuid` crate's v4 generator through here; the loop
/// re-draws rather than ever handing back an id already in the store.
fn mint_unused_uuid<T>(existing: &BTreeMap<String, T>) -> String {
    loop {
        let id = ::uuid::Uuid::new_v4().to_string();
        if !existing.contains_key(&id) {
            return id;
        }
    }
}

impl ComAmazonawsIotwirelessIotwirelessState {
    /// Mint a fresh, unused `WirelessDeviceId`.
    pub fn mint_wireless_device_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_devices)
    }

    /// Mint a fresh, unused `DeviceProfileId`.
    pub fn mint_device_profile_id(&mut self) -> String {
        mint_unused_uuid(&self.device_profiles)
    }

    /// Mint a fresh, unused `ServiceProfileId`.
    pub fn mint_service_profile_id(&mut self) -> String {
        mint_unused_uuid(&self.service_profiles)
    }

    /// What a `CreateServiceProfile` `ClientRequestToken` already minted, if anything.
    pub fn service_profile_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.service_profile_tokens.get(token)
    }

    /// Record `token` as having minted the service profile named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_SERVICE_PROFILE_TOKENS`].
    pub fn record_service_profile_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.service_profile_tokens,
            &mut self.service_profile_token_order,
            MAX_SERVICE_PROFILE_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted service profile `id`, so a token never
    /// outlives the resource it names.
    pub fn forget_service_profile_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.service_profile_tokens,
            &mut self.service_profile_token_order,
            id,
        );
    }

    /// Mint a fresh, unused `MulticastGroupId`.
    pub fn mint_multicast_group_id(&mut self) -> String {
        mint_unused_uuid(&self.multicast_groups)
    }

    /// Mint a fresh, unused `FuotaTaskId`.
    pub fn mint_fuota_task_id(&mut self) -> String {
        mint_unused_uuid(&self.fuota_tasks)
    }

    /// What a `CreateFuotaTask` `ClientRequestToken` already minted, if anything.
    pub fn fuota_task_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.fuota_task_tokens.get(token)
    }

    /// Record `token` as having minted the FUOTA task named by `record`, evicting the
    /// oldest token first if that would push the map past [`MAX_FUOTA_TASK_TOKENS`].
    pub fn record_fuota_task_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.fuota_task_tokens,
            &mut self.fuota_task_token_order,
            MAX_FUOTA_TASK_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted FUOTA task `id`, so a token never
    /// outlives the resource it names and a create/delete loop cannot grow the token
    /// store.
    pub fn forget_fuota_task_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.fuota_task_tokens,
            &mut self.fuota_task_token_order,
            id,
        );
    }

    /// Release every wireless device's membership of FUOTA task `id`, so no device
    /// reports belonging to a task the store no longer holds.
    pub fn release_fuota_task_members(&mut self, id: &str) {
        for device in self.wireless_devices.values_mut() {
            if device.fuota_task_id.as_deref() == Some(id) {
                device.fuota_task_id = None;
            }
        }
    }

    /// Mint a fresh, unused `WirelessGatewayId`.
    pub fn mint_wireless_gateway_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_gateways)
    }

    /// What a `CreateWirelessGateway` `ClientRequestToken` already minted, if anything.
    pub fn wireless_gateway_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.wireless_gateway_tokens.get(token)
    }

    /// Record `token` as having minted the gateway named by `record`, evicting the oldest
    /// token first if that would push the map past [`MAX_WIRELESS_GATEWAY_TOKENS`].
    pub fn record_wireless_gateway_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.wireless_gateway_tokens,
            &mut self.wireless_gateway_token_order,
            MAX_WIRELESS_GATEWAY_TOKENS,
            token,
            record,
        );
    }

    /// Drop every `CreateWirelessGateway` token that minted gateway `id`, so a token
    /// never outlives the resource it names.
    pub fn forget_wireless_gateway_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.wireless_gateway_tokens,
            &mut self.wireless_gateway_token_order,
            id,
        );
    }

    /// Remove wireless gateway `id` and everything the store held only because the
    /// gateway did: the `GatewayEui` that addressed it, the firmware task it was running,
    /// the network analyzer configurations that tracked it, and the idempotency tokens
    /// that minted it.
    ///
    /// `None` for a gateway the store does not hold, which is what lets the delete refuse
    /// an unknown `Id` from the same call that removes a known one. The `GatewayEui` goes
    /// because the identifier is free for another gateway to claim once this one is gone;
    /// the task and the tracked ids go because neither may name a gateway the account no
    /// longer holds.
    pub fn remove_wireless_gateway(&mut self, id: &str) -> Option<WirelessGateway> {
        let removed = self.wireless_gateways.remove(id)?;
        if let Some(gateway_eui) = removed.lorawan.gateway_eui.as_deref() {
            if self.gateway_eui_index.get(gateway_eui).map(String::as_str) == Some(id) {
                self.gateway_eui_index.remove(gateway_eui);
            }
        }
        self.wireless_gateway_tasks.remove(id);
        for configuration in self.network_analyzer_configurations.values_mut() {
            configuration.wireless_gateways.retain(|tracked| tracked != id);
        }
        self.forget_wireless_gateway_tokens_for(id);
        Some(removed)
    }

    /// Mint a fresh, unused `WirelessGatewayTaskDefinitionId`.
    pub fn mint_wireless_gateway_task_definition_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_gateway_task_definitions)
    }

    /// What a `CreateWirelessGatewayTaskDefinition` `ClientRequestToken` already minted,
    /// if anything.
    pub fn wireless_gateway_task_definition_token(
        &self,
        token: &str,
    ) -> Option<&IdempotentCreate> {
        self.wireless_gateway_task_definition_tokens.get(token)
    }

    /// Record `token` as having minted the task definition named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`].
    pub fn record_wireless_gateway_task_definition_token(
        &mut self,
        token: String,
        record: IdempotentCreate,
    ) {
        record_token(
            &mut self.wireless_gateway_task_definition_tokens,
            &mut self.wireless_gateway_task_definition_token_order,
            MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS,
            token,
            record,
        );
    }

    /// Drop every `CreateWirelessGatewayTaskDefinition` token that minted task definition
    /// `id`, so a token never outlives the resource it names.
    pub fn forget_wireless_gateway_task_definition_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.wireless_gateway_task_definition_tokens,
            &mut self.wireless_gateway_task_definition_token_order,
            id,
        );
    }

    /// How many stored wireless devices belong to multicast group `id`.
    ///
    /// The membership is held by the devices, so this counts them rather than reading a
    /// tally: every operation that reports a group's size answers from the same place
    /// the associate/disassociate operations write.
    pub fn devices_in_multicast_group(&self, id: &str) -> usize {
        self.wireless_devices
            .values()
            .filter(|device| device.multicast_group_id.as_deref() == Some(id))
            .count()
    }

    /// Record that `device_id`'s association with multicast group `group_id` has been
    /// requested, so the group keeps counting it as requested after the device leaves.
    ///
    /// A no-op for a group the store does not hold: an association request resolves both
    /// ends before it is recorded.
    pub fn record_multicast_group_request(&mut self, group_id: &str, device_id: &str) {
        if let Some(group) = self.multicast_groups.get_mut(group_id) {
            group.requested_devices.insert(device_id.to_string());
        }
    }

    /// Drop `device_id` from every group's requested set, so a deleted device is counted
    /// by no group and the sets stay bounded by the device store.
    pub fn forget_multicast_group_requests_for(&mut self, device_id: &str) {
        for group in self.multicast_groups.values_mut() {
            group.requested_devices.remove(device_id);
        }
    }

    /// Take multicast group `id` out of the store, with everything that only existed
    /// because the group did: the memberships the devices held in it, and the idempotency
    /// tokens that minted it.
    ///
    /// `None` for a group the store does not hold, which is what lets the delete refuse an
    /// unknown `Id` from the same call that removes a known one. The device memberships go
    /// because a device cannot belong to a group the account no longer holds — a stale
    /// `multicast_group_id` would otherwise keep answering
    /// `ListWirelessDevices?multicastGroupId=` and counting towards a group re-created
    /// under the same id.
    pub fn remove_multicast_group(&mut self, id: &str) -> Option<MulticastGroup> {
        let removed = self.multicast_groups.remove(id)?;
        for device in self.wireless_devices.values_mut() {
            if device.multicast_group_id.as_deref() == Some(id) {
                device.multicast_group_id = None;
            }
        }
        self.forget_multicast_group_tokens_for(id);
        Some(removed)
    }

    /// Drop every idempotency token that minted multicast group `id`, so a token never
    /// outlives the resource it names.
    pub fn forget_multicast_group_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.multicast_group_tokens,
            &mut self.multicast_group_token_order,
            id,
        );
    }

    /// What a `CreateMulticastGroup` `ClientRequestToken` already minted, if anything.
    pub fn multicast_group_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.multicast_group_tokens.get(token)
    }

    /// Record `token` as having minted the multicast group named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_MULTICAST_GROUP_TOKENS`].
    pub fn record_multicast_group_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.multicast_group_tokens,
            &mut self.multicast_group_token_order,
            MAX_MULTICAST_GROUP_TOKENS,
            token,
            record,
        );
    }

    /// What a `CreateDestination` `ClientRequestToken` already minted, if anything.
    pub fn destination_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.destination_tokens.get(token)
    }

    /// Record `token` as having minted the destination named by `record`, evicting the
    /// oldest token first if that would push the map past [`MAX_DESTINATION_TOKENS`].
    pub fn record_destination_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.destination_tokens,
            &mut self.destination_token_order,
            MAX_DESTINATION_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted destination `name`, so a token never
    /// outlives the resource it names.
    pub fn forget_destination_tokens_for(&mut self, name: &str) {
        forget_tokens_for(
            &mut self.destination_tokens,
            &mut self.destination_token_order,
            name,
        );
    }

    /// What a `CreateWirelessDevice` `ClientRequestToken` already minted, if anything.
    pub fn wireless_device_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.wireless_device_tokens.get(token)
    }

    /// Record `token` as having minted the device named by `record`, evicting the oldest
    /// token first if that would push the map past [`MAX_WIRELESS_DEVICE_TOKENS`].
    pub fn record_wireless_device_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.wireless_device_tokens,
            &mut self.wireless_device_token_order,
            MAX_WIRELESS_DEVICE_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted device `id`, so a token never outlives
    /// the resource it names.
    pub fn forget_wireless_device_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.wireless_device_tokens,
            &mut self.wireless_device_token_order,
            id,
        );
    }

    /// What a `CreateDeviceProfile` `ClientRequestToken` already minted, if anything.
    pub fn device_profile_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.device_profile_tokens.get(token)
    }

    /// Record `token` as having minted the device profile named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_DEVICE_PROFILE_TOKENS`].
    pub fn record_device_profile_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.device_profile_tokens,
            &mut self.device_profile_token_order,
            MAX_DEVICE_PROFILE_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted device profile `id`, so a token never
    /// outlives the resource it names.
    pub fn forget_device_profile_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.device_profile_tokens,
            &mut self.device_profile_token_order,
            id,
        );
    }

    /// The device a `GetWirelessDevice` identifier resolves to, or `None`.
    ///
    /// `identifier_type` is the modelled `WirelessDeviceIdType`; an unknown value
    /// resolves to nothing (the boundary validator already refuses one the enum does
    /// not list).
    pub fn find_device(&self, identifier_type: &str, identifier: &str) -> Option<&WirelessDevice> {
        match identifier_type {
            "WirelessDeviceId" => self.wireless_devices.get(identifier),
            "DevEui" => self
                .dev_eui_index
                .get(identifier)
                .and_then(|id| self.wireless_devices.get(id)),
            "ThingName" => self
                .wireless_devices
                .values()
                .find(|d| d.thing_name.as_deref() == Some(identifier)),
            "SidewalkManufacturingSn" => self.wireless_devices.values().find(|d| {
                d.sidewalk
                    .as_ref()
                    .and_then(|s| s.sidewalk_manufacturing_sn.as_deref())
                    == Some(identifier)
            }),
            _ => None,
        }
    }

    /// What a `CreateNetworkAnalyzerConfiguration` `ClientRequestToken` already minted,
    /// if anything.
    pub fn network_analyzer_configuration_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.network_analyzer_configuration_tokens.get(token)
    }

    /// Record `token` as having minted the configuration named by `record`, evicting the
    /// oldest token first if that would push the map past
    /// [`MAX_NETWORK_ANALYZER_CONFIGURATION_TOKENS`].
    pub fn record_network_analyzer_configuration_token(
        &mut self,
        token: String,
        record: IdempotentCreate,
    ) {
        record_token(
            &mut self.network_analyzer_configuration_tokens,
            &mut self.network_analyzer_configuration_token_order,
            MAX_NETWORK_ANALYZER_CONFIGURATION_TOKENS,
            token,
            record,
        );
    }

    /// Take network analyzer configuration `name` out of the store, together with the
    /// idempotency tokens that minted it.
    ///
    /// `None` for a name the store does not hold, which is what lets the delete refuse an
    /// unknown `ConfigurationName` from the same call that removes a known one. The tokens
    /// go because a token must not outlive the resource it names, and the name is free for
    /// another configuration to claim once this one is gone.
    pub fn remove_network_analyzer_configuration(
        &mut self,
        name: &str,
    ) -> Option<NetworkAnalyzerConfiguration> {
        let removed = self.network_analyzer_configurations.remove(name)?;
        self.forget_network_analyzer_configuration_tokens_for(name);
        Some(removed)
    }

    /// Drop every idempotency token that minted network analyzer configuration `name`, so
    /// a token never outlives the resource it names.
    pub fn forget_network_analyzer_configuration_tokens_for(&mut self, name: &str) {
        forget_tokens_for(
            &mut self.network_analyzer_configuration_tokens,
            &mut self.network_analyzer_configuration_token_order,
            name,
        );
    }

    /// The modelled 404 for the first id in these tracked-resource lists that names no
    /// stored resource, or `None` when every one of them resolves.
    ///
    /// A network analyzer configuration tracks OTHER resources by id, so a configuration
    /// may only name resources this account holds: one function, so the create and the
    /// update resolve a tracked id against the same state and refuse a missing one with
    /// the same body as its own reader does.
    pub fn unresolved_tracked_resource(
        &self,
        wireless_devices: &[String],
        wireless_gateways: &[String],
        multicast_groups: &[String],
    ) -> Option<ResourceNotFoundException> {
        if let Some(id) = wireless_devices
            .iter()
            .find(|id| !self.wireless_devices.contains_key(id.as_str()))
        {
            return Some(wireless_device_not_found(id));
        }
        if wireless_gateways
            .iter()
            .any(|id| !self.wireless_gateways.contains_key(id.as_str()))
        {
            // Every operation that addresses a gateway refuses a missing one with this
            // one fixed sentence, so a tracked gateway that does not resolve is refused
            // with it too rather than with a second wording.
            return Some(wireless_gateway_not_found());
        }
        if let Some(id) = multicast_groups
            .iter()
            .find(|id| !self.multicast_groups.contains_key(id.as_str()))
        {
            return Some(multicast_group_not_found(id));
        }
        None
    }

    /// The gateway a `GetWirelessGateway` identifier resolves to, or `None`.
    ///
    /// `identifier_type` is the modelled `WirelessGatewayIdType`; an unknown value
    /// resolves to nothing (the boundary validator already refuses one the enum does
    /// not list).
    pub fn find_gateway(
        &self,
        identifier_type: &str,
        identifier: &str,
    ) -> Option<&WirelessGateway> {
        match identifier_type {
            "WirelessGatewayId" => self.wireless_gateways.get(identifier),
            "GatewayEui" => self
                .gateway_eui_index
                .get(identifier)
                .and_then(|id| self.wireless_gateways.get(id)),
            "ThingName" => self
                .wireless_gateways
                .values()
                .find(|gateway| gateway.thing_name.as_deref() == Some(identifier)),
            _ => None,
        }
    }

    /// Whether the account holds the resource a log-level override would apply to.
    ///
    /// The override is an attribute OF a resource, so the three kinds
    /// [`LOG_LEVEL_RESOURCE_TYPES`] names are resolved against the stores that hold
    /// them; a type outside that set resolves to nothing (every log-level operation
    /// refuses one before it gets here).
    pub fn holds_log_level_resource(&self, resource_type: &str, resource_identifier: &str) -> bool {
        match resource_type {
            "WirelessDevice" => self.wireless_devices.contains_key(resource_identifier),
            "WirelessGateway" => self.wireless_gateways.contains_key(resource_identifier),
            "FuotaTask" => self.fuota_tasks.contains_key(resource_identifier),
            _ => false,
        }
    }

    /// The `LogLevel` override held for `resource_type`/`resource_identifier`, or `None`
    /// when that pair carries none.
    pub fn resource_log_level(
        &self,
        resource_type: &str,
        resource_identifier: &str,
    ) -> Option<&str> {
        self.resource_log_levels
            .get(resource_type)
            .and_then(|by_identifier| by_identifier.get(resource_identifier))
            .map(String::as_str)
    }

    /// How many log-level overrides the account holds, across every resource type.
    pub fn resource_log_level_count(&self) -> usize {
        self.resource_log_levels
            .values()
            .map(BTreeMap::len)
            .sum::<usize>()
    }

    /// Set `resource_type`/`resource_identifier`'s override to `log_level`, answering
    /// `false` — storing nothing — when a NEW override would push the account past
    /// [`MAX_RESOURCE_LOG_LEVEL_OVERRIDES`].
    ///
    /// Replacing an existing override is always accepted: it adds no entry, so the
    /// ceiling cannot bind on it.
    pub fn set_resource_log_level(
        &mut self,
        resource_type: &str,
        resource_identifier: &str,
        log_level: &str,
    ) -> bool {
        let fresh = self
            .resource_log_level(resource_type, resource_identifier)
            .is_none();
        if fresh && self.resource_log_level_count() >= MAX_RESOURCE_LOG_LEVEL_OVERRIDES {
            return false;
        }
        self.resource_log_levels
            .entry(resource_type.to_string())
            .or_default()
            .insert(resource_identifier.to_string(), log_level.to_string());
        true
    }

    /// Drop `resource_type`/`resource_identifier`'s override, answering whether one was
    /// held. A resource type left with no overrides is pruned, so the outer map holds
    /// no empty entries.
    pub fn clear_resource_log_level(
        &mut self,
        resource_type: &str,
        resource_identifier: &str,
    ) -> bool {
        let Some(by_identifier) = self.resource_log_levels.get_mut(resource_type) else {
            return false;
        };
        let removed = by_identifier.remove(resource_identifier).is_some();
        if by_identifier.is_empty() {
            self.resource_log_levels.remove(resource_type);
        }
        removed
    }

    /// The resource an event-configuration identifier addresses and the event
    /// configuration that resource holds, or `None` when the account holds no such
    /// resource.
    ///
    /// The configuration belongs to the RESOURCE, not to the identifier the request used
    /// to address it, so a device configured by its `DevEui` reports the same statuses
    /// when it is read by its `WirelessDeviceId`.
    pub fn find_event_configuration(
        &self,
        identifier_type: &str,
        identifier: &str,
    ) -> Option<(EventConfigurationResource, &ResourceEventConfiguration)> {
        match identifier_type {
            "WirelessDeviceId" | "DevEui" => {
                let device = self.find_device(identifier_type, identifier)?;
                Some((
                    EventConfigurationResource::WirelessDevice,
                    &device.event_configuration,
                ))
            }
            "WirelessGatewayId" | "GatewayEui" => {
                let gateway = self.find_gateway(identifier_type, identifier)?;
                Some((
                    EventConfigurationResource::WirelessGateway,
                    &gateway.event_configuration,
                ))
            }
            _ => None,
        }
    }

    /// The same lookup as [`Self::find_event_configuration`], for the writer.
    pub fn find_event_configuration_mut(
        &mut self,
        identifier_type: &str,
        identifier: &str,
    ) -> Option<(EventConfigurationResource, &mut ResourceEventConfiguration)> {
        match identifier_type {
            "WirelessDeviceId" | "DevEui" => {
                let id = self.find_device(identifier_type, identifier)?.id.clone();
                let device = self.wireless_devices.get_mut(&id)?;
                Some((
                    EventConfigurationResource::WirelessDevice,
                    &mut device.event_configuration,
                ))
            }
            "WirelessGatewayId" | "GatewayEui" => {
                let id = self.find_gateway(identifier_type, identifier)?.id.clone();
                let gateway = self.wireless_gateways.get_mut(&id)?;
                Some((
                    EventConfigurationResource::WirelessGateway,
                    &mut gateway.event_configuration,
                ))
            }
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotwirelessIotwirelessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotwirelessIotwirelessState>)
}
