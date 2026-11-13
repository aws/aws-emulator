// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-groundstation-groundstation service.
//!
//! Every resource this service hands out lives here: configs keyed by their
//! `(configType, configId)` pair (the two labels `GET /config/{configType}/{configId}`
//! carries), mission profiles, contacts, ephemerides and AWS Ground Station Agents keyed
//! by their uuid, plus one ARN index so a `trackingConfigArn` / `missionProfileArn`
//! reference resolves without scanning a map.
//!
//! Beside them are the facts about the account and the ground station network that AWS
//! establishes OUTSIDE this API, which the model therefore only reports: the uplink
//! approval, the reserved-minutes plan, the on-boarded satellites and the antennas AWS has
//! taken out of service for maintenance ([`MaintenanceReservation`]). Each is read from an
//! operator setting when this store is created.
//!
//! The helpers beside the struct are the pieces every handler of this service needs —
//! the request's region, the union-member-to-`ConfigCapabilityType` mapping, the
//! capability's user-facing spelling, the `ConfigArn` reader, uuid minting and the
//! opaque pagination token codec — so they are written once here rather than in each
//! `src/ops/<operation>.rs`.
//!
//! Error TEXT, on the other hand, is per operation: this service answers a
//! `ResourceNotFoundException` with the text of the operation that raised it, so each
//! `src/ops/<operation>.rs` carries its own (see [`MISSION_PROFILE_NOT_FOUND`] for the
//! one literal two operations genuinely share).

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AgentDetails, AggregateStatus, AntennaDemodDecodeDetails, AzElSegments, ComponentStatusData,
    ConfigDetails, ConfigTypeData, ConnectionDetails, ContactVersion, AzElSegmentsData,
    DataflowDetail, Destination,
    DiscoveryData, DownlinkDataflowDetails, Eirp, Elevation, EndpointDetails, EphemerisData,
    EphemerisMetaData, Frequency,
    FrequencyBandwidth,
    InvalidParameterException, KmsKey, ProgramTrackSettings, RangedConnectionDetails,
    ResourceNotFoundException, S3Object, S3RecordingDetails,
    Source, SpectrumConfig,
    TrackingOverrides, UplinkDataflowDetails,
};
mod ephemerides;
mod contacts;
pub use contacts::*;
pub use ephemerides::*;

/// Region used in minted ARNs when the request carries no usable SigV4 credential
/// scope (an unsigned request still routes by endpoint host, so this is reachable).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, per `OPERATIONS.md`.
pub const ARN_SERVICE: &str = "groundstation";

/// How long after the call that starts it a transition this service owes the clock falls
/// due, for every such transition this emulator chooses the span of (the ones below).
///
/// AWS documents no duration for any of them — the real service takes minutes to place a
/// stream's network interfaces or to validate an ephemeris — so the span is the
/// emulator's own, and it is a FIXED one: a caller does not have to guess a different
/// wait per resource. It is deliberately never zero, and deliberately longer than the
/// handful of requests a caller makes next, so the transient status and the state errors
/// that go with it are observable rather than a race against the caller's own next call.
/// Nothing sleeps for it: the due instant is stored, and a later reporting call
/// materialises the move once it has passed.
const CLOCK_OWED_TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// `ReserveContact` admits a contact in `SCHEDULING`; the scheduler moves it to
/// `SCHEDULED` on its own. CHOSEN span (AWS documents no duration for it): the span this
/// emulator gives every transition it owes the clock, which is deliberately never zero
/// and long enough that a caller that goes on working against the service after
/// reserving — reserving another contact, reading the first one back — still observes the
/// transient status and the errors it carries.
pub const SCHEDULING_SETTLES_AFTER: Duration = CLOCK_OWED_TRANSITION_SPAN;

// `CreateDataflowEndpointGroup` is deliberately absent from the spans above: it hands back
// a group whose dataflow endpoints are already attached. The recorded group lifecycle reads
// a group back after creating it and is answered `endpoint.status` `created` with
// `healthStatus` `HEALTHY` and `healthReasons` `[HEALTHY]`, so that resource is returned
// READY and owes the clock no move. (An AWS Ground Station Agent endpoint carries no
// `status` of its own; the health it reports is that of the agent registered for its group,
// derived on every read by `apply_agent_endpoint_health`.)

/// `UpdateContact` appends a contact version in `UPDATING` ("the version is being applied
/// to the contact" — `smithy_query(query="shape", shape="VersionStatus")`); the service
/// applies it and it becomes `ACTIVE` on its own. CHOSEN span (AWS documents no duration
/// for it): the span this emulator gives every transition it owes the clock, so a caller
/// that goes on working after the update still observes the version being applied and the
/// configuration still in force.
pub const VERSION_ACTIVATES_AFTER: Duration = CLOCK_OWED_TRANSITION_SPAN;

/// `CancelContact` on a contact whose pass is already under way STOPS it: the contact
/// reports `CANCELLING` while the stop is being carried out and reaches `CANCELLED` on its
/// own. CHOSEN span (AWS documents no duration for it): the span this emulator gives every
/// transition it owes the clock, so a caller that goes on working after the cancel still
/// observes the release under way.
pub const CANCELLATION_SETTLES_AFTER: Duration = CLOCK_OWED_TRANSITION_SPAN;

/// `CreateEphemeris` admits an ephemeris in `VALIDATING`; the service validates the
/// supplied data and the ephemeris reaches `ENABLED`, `DISABLED` or `INVALID` on its own.
/// CHOSEN span (AWS documents no duration for it — the real service takes up to minutes to
/// validate an ephemeris): the span this emulator gives every transition it owes the
/// clock, so a caller that goes on working after the create — creating the rest of a set
/// of ephemerides, then reading the first one back — still observes `VALIDATING`.
pub const EPHEMERIS_VALIDATION_SETTLES_AFTER: Duration = CLOCK_OWED_TRANSITION_SPAN;

/// Which `ConfigCapabilityType` a `ConfigTypeData` union member selects.
///
/// The config's type is NOT a request field: it is implied by whichever member of the
/// union the caller set, and it is what both the `configArn`'s `config/<type>/<id>`
/// segment and the `{configType}` label of `GetConfig` carry. The eight pairs are the
/// model's own: `smithy_query(query="shape", shape="ConfigCapabilityType")` lists the
/// enum values and `ConfigTypeData` the members.
pub fn config_type_of(config_data: &ConfigTypeData) -> &'static str {
    match config_data {
        ConfigTypeData::antennaDownlinkConfig(_) => "antenna-downlink",
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(_) => "antenna-downlink-demod-decode",
        ConfigTypeData::antennaUplinkConfig(_) => "antenna-uplink",
        ConfigTypeData::dataflowEndpointConfig(_) => "dataflow-endpoint",
        ConfigTypeData::s3RecordingConfig(_) => "s3-recording",
        ConfigTypeData::telemetrySinkConfig(_) => "telemetry-sink",
        ConfigTypeData::trackingConfig(_) => "tracking",
        ConfigTypeData::uplinkEchoConfig(_) => "uplink-echo",
    }
}

/// The error code this service names when its request validator refuses a request that
/// failed the model's own rules: a missing required member, or a value outside a
/// declared bound. Recorded against AWS, which answers HTTP 400 with
/// `x-amzn-errortype: BadRequestException` — NOT the `ValidationException` the framework
/// defaults to, which this service's model does not declare for any operation
/// (`smithy_query(query="service_info")` lists its common errors).
pub const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";

/// The message that goes with [`BAD_REQUEST_ERROR_CODE`]: one fixed sentence for the
/// whole class, not a rendering of the constraint that failed.
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The frequency bands an uplink `Config` may be tuned to, in MHz, as closed ranges.
///
/// Source: the model's own documentation of `UplinkSpectrumConfig$centerFrequency` —
/// "Center frequency of an uplink spectral Config. Valid values are between 2025 to
/// 2120 MHz" (`smithy_query(query="doc", shape="UplinkSpectrumConfig",
/// member="centerFrequency")`). A frequency outside them is not one the service will
/// radiate on, so a `Config` naming it is refused rather than stored. It is the band
/// the SERVICE operates; which part of it a given account may transmit on is
/// [`ComAmazonawsGroundstationGroundstationState::approved_uplink_bands_mhz`].
pub const UPLINK_CENTER_FREQUENCY_BANDS_MHZ: &[(f64, f64)] = &[(2025.0, 2120.0)];

/// The frequency bands a downlink `Config` may be tuned to, in MHz.
///
/// Source: `SpectrumConfig$centerFrequency`'s documentation — "Valid values are between
/// 2200 to 2300 MHz and 7750 to 8400 MHz for downlink and 2025 to 2120 MHz for uplink"
/// (`smithy_query(query="doc", shape="SpectrumConfig", member="centerFrequency")`).
/// `SpectrumConfig` is the DOWNLINK spectrum shape — the uplink one is
/// `UplinkSpectrumConfig` above — so the two downlink bands are the ones that apply.
pub const DOWNLINK_CENTER_FREQUENCY_BANDS_MHZ: &[(f64, f64)] = &[(2200.0, 2300.0), (7750.0, 8400.0)];

/// The bandwidth an `antennaDownlinkConfig` may ask for, in MHz.
///
/// Source: `FrequencyBandwidth$value`'s documentation — "For AntennaDownlinkconfig,
/// valid values are between 10 kHz to 54 MHz" (`smithy_query(query="doc",
/// shape="FrequencyBandwidth", member="value")`), with the lower bound converted to the
/// MHz every bound here is expressed in.
pub const ANTENNA_DOWNLINK_BANDWIDTH_MHZ: (f64, f64) = (0.01, 54.0);

/// The bandwidth an `antennaDownlinkDemodDecodeConfig` may ask for, in MHz.
///
/// Source: the same `FrequencyBandwidth$value` documentation — "For
/// AntennaDownlinkDemodDecodeconfig, valid values are between 125 kHz to 650 MHz".
pub const DEMOD_DECODE_BANDWIDTH_MHZ: (f64, f64) = (0.125, 650.0);

/// The target EIRP an `antennaUplinkConfig` may ask for, in dBW.
///
/// Source: `Eirp$value`'s documentation — "Value of an EIRP. Valid values are between
/// 20.0 to 50.0 dBW" (`smithy_query(query="doc", shape="Eirp", member="value")`).
pub const TARGET_EIRP_DBW: (f64, f64) = (20.0, 50.0);

/// What the service answers for an uplink center frequency this account may not
/// transmit on: its fixed refusal text for this one condition, as recorded against AWS.
/// The model documents the error (`InvalidParameterException`, HTTP 431) but not the
/// sentence, so the status and the error code are the contract and this text is the
/// wording that goes with them.
pub const UNAPPROVED_UPLINK_FREQUENCY: &str =
    "The specified Uplink Frequency has not been approved.";

/// The values `FrequencyUnits` and `BandwidthUnits` accept (`smithy_query(query="shape",
/// shape="FrequencyUnits")` — `GHz`, `MHz`, `kHz`).
///
/// A token outside them never reaches a handler: it fails the model's own value set, so
/// the boundary validator refuses the request with this service's 400
/// `BadRequestException` (pinned by
/// `tests/all/request_validation.rs::a_unit_outside_the_modelled_enum_is_refused_before_the_handler`).
/// That is why [`in_megahertz`] needs no refusal of its own — a unit it cannot name is a
/// value in no band, which is how the checks below treat it.
pub const FREQUENCY_UNITS: &[&str] = &["GHz", "MHz", "kHz"];

/// The one value `EirpUnits` accepts (`smithy_query(query="shape", shape="EirpUnits")`),
/// refused outside it the same way as [`FREQUENCY_UNITS`].
pub const EIRP_UNITS: &[&str] = &["dBW"];

/// How far outside a documented bound a value may land and still be inside it: one
/// millihertz, expressed in MHz.
///
/// A bound is a quantity, not a bit pattern, and the same quantity written in another
/// unit converts through a binary multiply — `0.054 GHz` is the 54 MHz ceiling but
/// lands an ulp past it. The tolerance is far below `kHz`, the finest unit the model
/// offers, so it cannot widen a bound into a value a caller could use.
const BOUND_TOLERANCE_MHZ: f64 = 1e-9;

/// `value` expressed in MHz, from one of the [`FREQUENCY_UNITS`]; `None` for any other
/// unit, which the boundary validator has already refused (see [`FREQUENCY_UNITS`]).
fn in_megahertz(value: f64, units: &str) -> Option<f64> {
    match units {
        "GHz" => Some(value * 1_000.0),
        "MHz" => Some(value),
        "kHz" => Some(value / 1_000.0),
        _ => None,
    }
}

/// `value` as the request wrote it, unit and all: `2024.9 MHz`.
///
/// The refusal quotes the request rather than the converted number, so a caller reading
/// it sees the value it sent beside the bound it missed.
fn as_written(value: f64, units: &str) -> String {
    format!("{value} {units}")
}

/// The modelled `InvalidParameterException` (HTTP 431) naming the member at fault.
fn invalid_parameter(parameter_name: &str, message: String) -> InvalidParameterException {
    InvalidParameterException {
        message: Some(message),
        parameter_name: Some(parameter_name.to_string()),
    }
}

/// `bands` as a sentence fragment: `2200 to 2300 MHz or 7750 to 8400 MHz`.
fn render_bands(bands: &[(f64, f64)]) -> String {
    bands
        .iter()
        .map(|(low, high)| format!("{low} to {high} MHz"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Whether `megahertz` falls inside one of `bands`, each a closed range.
fn in_any_band(megahertz: f64, bands: &[(f64, f64)]) -> bool {
    bands.iter().any(|(low, high)| {
        megahertz >= low - BOUND_TOLERANCE_MHZ && megahertz <= high + BOUND_TOLERANCE_MHZ
    })
}

/// Check a center frequency against the bands its link direction allows, wording the
/// refusal from the request and the documented bound: the member, the value as written,
/// and the range it fell outside.
fn check_center_frequency(
    path: &str,
    frequency: &Frequency,
    bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    if in_megahertz(frequency.value, &frequency.units)
        .is_some_and(|megahertz| in_any_band(megahertz, bands))
    {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        format!(
            "Value '{}' at '{path}' is outside the valid range of {}.",
            as_written(frequency.value, &frequency.units),
            render_bands(bands)
        ),
    ))
}

/// Check a bandwidth against the closed range its config type allows.
fn check_bandwidth(
    path: &str,
    bandwidth: &FrequencyBandwidth,
    bound: (f64, f64),
) -> Result<(), InvalidParameterException> {
    if in_megahertz(bandwidth.value, &bandwidth.units)
        .is_some_and(|megahertz| in_any_band(megahertz, &[bound]))
    {
        return Ok(());
    }
    let (low, high) = bound;
    Err(invalid_parameter(
        path,
        format!(
            "Value '{}' at '{path}' is outside the valid range of {low} to {high} MHz.",
            as_written(bandwidth.value, &bandwidth.units)
        ),
    ))
}

/// Check a downlink spectrum: its center frequency against the downlink bands, its
/// bandwidth against the range the owning config type allows.
fn check_downlink_spectrum(
    path: &str,
    spectrum: &SpectrumConfig,
    bandwidth: (f64, f64),
) -> Result<(), InvalidParameterException> {
    check_center_frequency(
        &format!("{path}.centerFrequency"),
        &spectrum.center_frequency,
        DOWNLINK_CENTER_FREQUENCY_BANDS_MHZ,
    )?;
    check_bandwidth(&format!("{path}.bandwidth"), &spectrum.bandwidth, bandwidth)
}

/// Check an uplink center frequency against what this account may transmit on: inside
/// the band the service operates, AND inside one of the bands the account is approved
/// for. Both are the same refusal to a caller — the frequency is not one this account
/// may uplink on — so both answer the service's one fixed sentence for it.
fn check_uplink_center_frequency(
    path: &str,
    frequency: &Frequency,
    approved_bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    let megahertz = in_megahertz(frequency.value, &frequency.units);
    if megahertz.is_some_and(|megahertz| {
        in_any_band(megahertz, UPLINK_CENTER_FREQUENCY_BANDS_MHZ)
            && in_any_band(megahertz, approved_bands)
    }) {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        UNAPPROVED_UPLINK_FREQUENCY.to_string(),
    ))
}

/// Check a target EIRP against [`TARGET_EIRP_DBW`]. A unit outside [`EIRP_UNITS`] is a
/// value in no range, for the reason given on that constant.
fn check_target_eirp(path: &str, eirp: &Eirp) -> Result<(), InvalidParameterException> {
    let (low, high) = TARGET_EIRP_DBW;
    if EIRP_UNITS.contains(&eirp.units.as_str()) && eirp.value >= low && eirp.value <= high {
        return Ok(());
    }
    Err(invalid_parameter(
        &format!("{path}.value"),
        format!(
            "Value '{}' at '{path}.value' is outside the valid range of \
             {low} to {high} dBW.",
            as_written(eirp.value, &eirp.units)
        ),
    ))
}

/// Refuse a `ConfigTypeData` the service would not accept: a numeric member outside the
/// range the model documents for it, or an uplink frequency `approved_uplink_bands` does
/// not cover. The refusal is the service's modelled `InvalidParameterException` (HTTP
/// 431), naming the member at fault.
///
/// Every bound is the model's own, cited on its constant above, and the value is
/// compared after conversion from whichever `FrequencyUnits` the caller wrote it in —
/// `2.05 GHz`, `2050 MHz` and `2050000 kHz` are the same frequency and are judged alike.
/// The union members with no documented numeric range (`trackingConfig`,
/// `dataflowEndpointConfig`, `uplinkEchoConfig`, `s3RecordingConfig`,
/// `telemetrySinkConfig`) carry no bound for this to check: what they carry are
/// references, resolved elsewhere — `uplinkEchoConfig`'s against this service's own
/// store, `s3RecordingConfig`'s bucket by [`check_recording_bucket`].
fn validate_config_data(
    config_data: &ConfigTypeData,
    approved_uplink_bands: &[(f64, f64)],
) -> Result<(), InvalidParameterException> {
    match config_data {
        ConfigTypeData::antennaUplinkConfig(uplink) => {
            check_uplink_center_frequency(
                "configData.antennaUplinkConfig.spectrumConfig.centerFrequency",
                &uplink.spectrum_config.center_frequency,
                approved_uplink_bands,
            )?;
            check_target_eirp(
                "configData.antennaUplinkConfig.targetEirp",
                &uplink.target_eirp,
            )
        }
        ConfigTypeData::antennaDownlinkConfig(downlink) => check_downlink_spectrum(
            "configData.antennaDownlinkConfig.spectrumConfig",
            &downlink.spectrum_config,
            ANTENNA_DOWNLINK_BANDWIDTH_MHZ,
        ),
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(downlink) => check_downlink_spectrum(
            "configData.antennaDownlinkDemodDecodeConfig.spectrumConfig",
            &downlink.spectrum_config,
            DEMOD_DECODE_BANDWIDTH_MHZ,
        ),
        ConfigTypeData::trackingConfig(_)
        | ConfigTypeData::dataflowEndpointConfig(_)
        | ConfigTypeData::uplinkEchoConfig(_)
        | ConfigTypeData::s3RecordingConfig(_)
        | ConfigTypeData::telemetrySinkConfig(_) => Ok(()),
    }
}

/// The value the model gives `SpectrumConfig$polarization` when a request omits it.
///
/// Source: `smithy_query(query="shape", shape="SpectrumConfig")` — the member targets
/// `Polarization` (values `RIGHT_HAND`, `LEFT_HAND`, `NONE`) and carries
/// `default: "NONE"`, with `addedDefault` and `clientOptional`.
pub const SPECTRUM_CONFIG_POLARIZATION_DEFAULT: &str = "NONE";

/// Materialise the member defaults the model declares, on a `configData` about to be
/// stored.
///
/// A member the model gives a `default` is one the SERVICE always has a value for: a
/// request that leaves it out is stored carrying that default, so every later read
/// answers the member populated and a client that round-trips a resource back through a
/// create or an update loses nothing. An explicitly supplied value is never touched.
///
/// The one member with a modelled default anywhere in a `Config`'s shape closure is
/// `SpectrumConfig$polarization` (`smithy_query(query="closure",
/// operation="CreateConfig")` — no other member of the closure carries `default`), and
/// it is applied wherever that shape is reached: `antennaDownlinkConfig.spectrumConfig`
/// and `antennaDownlinkDemodDecodeConfig.spectrumConfig`. The spectrum shape reached
/// through `antennaUplinkConfig` is `UplinkSpectrumConfig`, whose own `polarization`
/// carries NO default, so an omitted uplink polarization stays absent — the default
/// materialised here is the model's, not this service's guess.
pub fn apply_modelled_defaults(config_data: &mut ConfigTypeData) {
    let spectrum: Option<&mut SpectrumConfig> = match config_data {
        ConfigTypeData::antennaDownlinkConfig(downlink) => Some(&mut downlink.spectrum_config),
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(downlink) => {
            Some(&mut downlink.spectrum_config)
        }
        ConfigTypeData::antennaUplinkConfig(_)
        | ConfigTypeData::trackingConfig(_)
        | ConfigTypeData::dataflowEndpointConfig(_)
        | ConfigTypeData::uplinkEchoConfig(_)
        | ConfigTypeData::s3RecordingConfig(_)
        | ConfigTypeData::telemetrySinkConfig(_) => None,
    };
    if let Some(spectrum) = spectrum {
        if spectrum.polarization.is_none() {
            spectrum.polarization = Some(SPECTRUM_CONFIG_POLARIZATION_DEFAULT.to_string());
        }
    }
}

/// The bucket name an S3 BUCKET ARN names, or `None` when the string is not one.
///
/// An S3 bucket ARN is `arn:<partition>:s3:::<bucket>`: the service segment is `s3` and
/// the region and account segments are empty, because a bucket name is global rather
/// than regional or per account (the model's `BucketArn` carries an `arnReference` to
/// `AWS::S3::Bucket` and no pattern, so the shape of the ARN is checked here). The
/// bucket is the first path segment of the resource part, so an ARN that carries a key
/// or a prefix after it still names its bucket — and an ARN of any OTHER service names
/// no bucket at all, however its resource part is spelled.
pub fn bucket_name_of(bucket_arn: &str) -> Option<&str> {
    let mut segments = bucket_arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let _partition = segments.next()?;
    if segments.next()? != "s3" {
        return None;
    }
    // Region and account: empty on a bucket ARN. An `arn:aws:s3:us-east-1:123…:…` is
    // some other S3 resource (an access point, say), not a bucket.
    if !segments.next()?.is_empty() || !segments.next()?.is_empty() {
        return None;
    }
    let resource = segments.next()?;
    let name = resource.split('/').next().unwrap_or(resource);
    (!name.is_empty()).then_some(name)
}

/// What `s3RecordingConfig`'s bucket resolution answers when the bucket cannot be
/// resolved: the service's SINGLE fixed refusal text for that condition, whatever made
/// the resolution fail — a `bucketArn` that names no bucket and a bucket the account does
/// not hold are the same answer, because both are the bucket's location failing to
/// resolve. Entirely fixed text: it interpolates no bucket, ARN, account or region. As
/// with every other refusal here the status (431) and the error code
/// (`InvalidParameterException`) are the contract, and this is the wording that goes
/// with them.
pub const UNRESOLVABLE_RECORDING_BUCKET: &str =
    "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure \
     that Ground Station has s3:GetBucketLocation permissions on the bucket.";

/// Refuse a `configData` whose recording bucket cannot be resolved — and only then.
///
/// An `s3RecordingConfig` names the bucket to record to by ARN, and Ground Station
/// resolves that bucket — it has to know where the bucket is before it can record to it —
/// before it will store the config: a bucket S3 reports this account does not hold, and a
/// `bucketArn` that names no bucket at all, are refused with the modelled
/// `InvalidParameterException` (HTTP 431) instead of being stored, so a caller never holds
/// a recording config that cannot record. Both failures are ONE refusal, carrying
/// [`UNRESOLVABLE_RECORDING_BUCKET`]: what the caller is told is that the bucket's
/// location could not be obtained, not which way the lookup came back empty. A bucket that
/// RESOLVES is accepted.
///
/// S3 is reached the way the framework documents a handler reaching another emulated
/// service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client
/// over the in-process callback's HTTP client and credentials, so this crate names no
/// other service crate and the bucket is resolved against the emulator's OWN S3 state.
/// The question is put with `HeadBucket`, the operation S3 documents both for existence
/// ("You can use this operation to determine if a bucket exists") and for a bucket's
/// location ("calls to the HeadBucket operation always return the bucket's location
/// through an HTTP response header […] we recommend using the HeadBucket operation for
/// bucket Region discovery"), so one call answers the resolution this refusal is about.
///
/// `None` means the create or update may proceed. That INCLUDES every answer that is not
/// S3 reporting the bucket absent — a router with no S3 mounted, or any other failure to
/// ask: nothing is then known about the bucket, and a refusal has to be grounded in what
/// S3 answered rather than assumed (worth a warning, because that is a deployment where
/// this gate does not run). Every other `ConfigTypeData` member names no bucket and is
/// nothing for this to resolve.
pub async fn unresolvable_recording_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config_data: &ConfigTypeData,
) -> Option<InvalidParameterException> {
    let ConfigTypeData::s3RecordingConfig(recording) = config_data else {
        return None;
    };
    let refusal = || {
        invalid_parameter(
            "configData.s3RecordingConfig.bucketArn",
            UNRESOLVABLE_RECORDING_BUCKET.to_string(),
        )
    };
    // A value that is not an S3 bucket ARN names no bucket, and that is settled without
    // asking S3: there is nothing to ask about. An ARN of any other service names no
    // bucket either, however its resource part is spelled (see [`bucket_name_of`]).
    let Some(bucket) = bucket_name_of(&recording.bucket_arn) else {
        return Some(refusal());
    };
    let answer = s3_client(ctx, region, ctx.callback().aws_credentials_provider())
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket, so the config can record to it.
        Ok(_) => None,
        // S3 says this account holds no such bucket, so its location cannot be obtained
        // either: the refusal is grounded.
        Err(error) if bucket_reported_absent(&error) => Some(refusal()),
        Err(error) => {
            ::tracing::warn!(
                bucket_arn = %recording.bucket_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the recording config's bucket could not be resolved against S3"
            );
            None
        }
    }
}

/// The S3 client this service reaches object storage through: the AWS SDK driven over the
/// emulator's own in-process transport, signing in the name of `credentials`.
///
/// Whose name that is differs by caller, which is why it is a parameter. A bucket this
/// service RESOLVES is read in the emulator's own name
/// ([`unresolvable_recording_bucket`]), while an upload URL this service hands a caller is
/// signed with the credential minted for that one upload
/// ([`presign_agent_task_response_upload`]), so the URL carries that credential's id and
/// nothing else.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    credentials: ::aws_sdk_s3::config::SharedCredentialsProvider,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(credentials)
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in
        // the path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are
        // handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not
/// reaching S3 at all.
fn bucket_reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}

/// How long an upload URL `GetAgentTaskResponseUrl` hands out stays signed for.
///
/// CHOSEN span: AWS documents no validity for the agent task response URL (the model says
/// only that it is "a presigned URL for uploading agent task response logs" —
/// `smithy_query(query="doc", shape="GetAgentTaskResponseUrlResponse",
/// member="presignedLogUrl")`), so the emulator signs it for the quarter of an hour an
/// agent needs to finish a pass and hand its log over. It is also how long the credential
/// that signed it lives: the credential is withdrawn when the URL it signed stops being
/// accepted.
pub const AGENT_TASK_RESPONSE_URL_VALIDITY: Duration = Duration::from_secs(15 * 60);

/// The credential's name in the upload URL's provider, for diagnostics only.
const AGENT_LOG_UPLOAD_SIGNER: &str = "GroundStationAgentTaskResponse";

/// The one region an Amazon S3 `CreateBucket` names by naming nothing: a request with no
/// `LocationConstraint` places the bucket in `us-east-1`, and S3 refuses that constraint
/// spelled out.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// The HTTP status Amazon S3 answers a `CreateBucket` for a bucket that is already there
/// with (`BucketAlreadyOwnedByYou` / `BucketAlreadyExists`).
const BUCKET_EXISTS_STATUS: u16 = 409;

/// The Amazon S3 bucket AWS Ground Station takes an account's agent task response logs in,
/// in `region`: one bucket per account per region, so the name is DERIVED from the account
/// and the region rather than generated and stored.
///
/// The name and the layout under it are this emulator's own — AWS documents only that the
/// upload URL is presigned, never where the object lands — but the bucket has to BE
/// somewhere a client can reach, so it is a bucket of this emulator's own object storage.
pub fn agent_log_bucket_name(region: &str) -> String {
    format!(
        "{ARN_SERVICE}-agent-logs-{region}-{}",
        ::engine::emulator_account_id()
    )
}

/// The Amazon S3 key one agent task's response log is uploaded at: the agent that ran the
/// task, then the task, so two tasks of one agent and one task of two agents each upload
/// to their own object and no upload overwrites another's log.
pub fn agent_task_response_log_key(agent_id: &str, task_id: &str) -> String {
    format!("agent/{agent_id}/task/{task_id}/agent-task-response.log")
}

/// One upload URL an agent holds, for one of its tasks: the URL itself, the credential it
/// is signed with and the instant that signature stops being accepted.
///
/// Held so that the URL reported for a task is reported again while it is still valid — a
/// reader hands every caller the SAME upload rather than a fresh one per call — and so the
/// credential behind a URL whose validity has run out is withdrawn instead of accumulating
/// in the credential store. A URL that is still valid is never withdrawn: an agent holding
/// one goes on uploading with it whatever else is read.
#[derive(Debug, Clone)]
pub struct AgentLogUpload {
    pub presigned_log_url: String,
    pub credential_id: String,
    pub expires_at: SystemTime,
}

impl AgentLogUpload {
    /// Whether this URL is still signed for at `now`.
    pub fn is_live(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }
}

/// Withdraw `credential_id` from the emulator's credential store: the credential that
/// signed an upload URL whose validity has run out, so a key that can no longer sign
/// anything does not stay in the store.
pub fn withdraw_credential(ctx: &smithy_server_runtime::OperationContext<'_>, credential_id: &str) {
    if let Err(why) = ctx.credential_store().delete(credential_id) {
        ::tracing::warn!(
            reason = ?why,
            "the credential that signed an agent task response upload URL could not be \
             withdrawn",
        );
    }
}

/// Sign the upload of `agent_id`'s response log for `task_id`, and answer the URL with the
/// credential it is signed with — `None` when nothing could be signed.
///
/// The URL is a presigned Amazon S3 `PutObject` of the object [`agent_task_response_log_key`]
/// names in the bucket [`agent_log_bucket_name`] names, so an agent that PUTs its log to
/// the URL stores it in the emulator's OWN object storage and a URL this service reports
/// names a destination that accepts the upload. Object storage is reached the way the
/// framework documents a handler reaching another emulated service
/// ([`smithy_server_runtime::OperationContext::callback`]), so the bucket is created in
/// this same emulator and nothing leaves the process.
///
/// It is signed with a credential the emulator's credential store MINTS for it, never with
/// the emulator's own: a presigned URL carries the signer's key id, and this one carries a
/// credential that can sign exactly the one upload the URL names, for
/// [`AGENT_TASK_RESPONSE_URL_VALIDITY`].
pub async fn presign_agent_task_response_upload(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    agent_id: &str,
    task_id: &str,
) -> Option<AgentLogUpload> {
    use ::emulator_server::aws_credentials::{AWS_ACCESS_KEY, SECRET_ACCESS_KEY};

    let bucket = agent_log_bucket_name(region);
    let key = agent_task_response_log_key(agent_id, task_id);
    // The destination the URL names has to be there before the URL is handed out,
    // otherwise the upload it authorises has nowhere to land and the caller would hold a
    // URL nothing serves. One bucket per account per region, so this is a create only the
    // first time; an object storage that does not hold the bucket afterwards is answered
    // as the failed dependency it is.
    if !create_agent_log_bucket(ctx, region, &bucket).await {
        return None;
    }

    let credentials = ctx.credential_store();
    let minted = credentials.create(&AWS_ACCESS_KEY);
    let signed = match minted.secret(SECRET_ACCESS_KEY) {
        Some(secret) => presigned_upload(ctx, region, &bucket, &key, &minted.id, secret).await,
        // A credential without its secret cannot sign anything.
        None => None,
    };
    match signed {
        Some(presigned_log_url) => Some(AgentLogUpload {
            presigned_log_url,
            credential_id: minted.id,
            expires_at: SystemTime::now() + AGENT_TASK_RESPONSE_URL_VALIDITY,
        }),
        // A credential that signed no URL is of no use to anyone, so it does not stay in
        // the store either.
        None => {
            let _ = credentials.delete(&minted.id);
            None
        }
    }
}

/// The presigned `PutObject` URL of `bucket`/`key`, signed in the name of the minted
/// credential, or `None` when the URL could not be signed.
async fn presigned_upload(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Option<String> {
    let expires = match ::aws_sdk_s3::presigning::PresigningConfig::expires_in(
        AGENT_TASK_RESPONSE_URL_VALIDITY,
    ) {
        Ok(expires) => expires,
        Err(failure) => {
            ::tracing::warn!(
                detail = %failure,
                "the agent task response URL's validity is not a presignable span"
            );
            return None;
        }
    };
    // The client sends no request — presigning only signs — but it is given the same
    // in-process transport anyway, so the URL it signs addresses the endpoint the
    // emulator's own object storage answers on.
    let signer = ::aws_sdk_s3::config::SharedCredentialsProvider::new(
        ::aws_sdk_s3::config::Credentials::new(
            access_key_id,
            secret_access_key,
            None,
            None,
            AGENT_LOG_UPLOAD_SIGNER,
        ),
    );
    match s3_client(ctx, region, signer)
        .put_object()
        .bucket(bucket)
        .key(key)
        .presigned(expires)
        .await
    {
        Ok(request) => Some(request.uri().to_string()),
        Err(failure) => {
            ::tracing::warn!(
                bucket,
                key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the agent task response log's upload URL could not be signed",
            );
            None
        }
    }
}

/// Create the bucket agent task response logs are uploaded to, unless object storage
/// already holds it, and answer whether object storage holds it now.
///
/// A bucket that was already there is what this call is for and counts as held. Every
/// OTHER answer — an emulator assembled without object storage, a transport failure, a
/// refused create — means nothing would take the upload a URL signed now would authorise,
/// so it is logged and reported as not held, and the read answers with the modelled
/// dependency failure rather than with a URL nothing serves.
async fn create_agent_log_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    let mut create = s3_client(ctx, region, ctx.callback().aws_credentials_provider())
        .create_bucket()
        .bucket(bucket);
    // One bucket per region, so it is placed in the request's own region — which on
    // `CreateBucket` is said with the location constraint rather than with the signing
    // region. See [`UNCONSTRAINED_BUCKET_REGION`] for the one region said by saying
    // nothing.
    if region != UNCONSTRAINED_BUCKET_REGION {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let Err(failure) = create.send().await else {
        return true;
    };
    let status = failure
        .raw_response()
        .map(|response| response.status().as_u16());
    if status == Some(BUCKET_EXISTS_STATUS) {
        return true;
    }
    ::tracing::warn!(
        bucket,
        region,
        status = ?status,
        detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
        "object storage did not stand up the bucket agent task response logs are uploaded \
         to, so no upload URL could be handed out",
    );
    false
}

/// The capability's name as this service spells it in user-facing text: the
/// UpperCamelCase form of a `ConfigCapabilityType` token.
///
/// That form is not a second naming scheme — it is exactly the name of the shape the
/// matching `ConfigTypeData` member targets, minus its `Config` suffix:
/// `antenna-uplink` -> `AntennaUplinkConfig` -> `AntennaUplink`, `s3-recording` ->
/// `S3RecordingConfig` -> `S3Recording`, `antenna-downlink-demod-decode` ->
/// `AntennaDownlinkDemodDecodeConfig` -> `AntennaDownlinkDemodDecode`. Source:
/// `smithy_query(query="shape", shape="ConfigTypeData,ConfigCapabilityType")` lists the
/// eight member targets beside the eight tokens, and the two agree segment for segment —
/// so the spelling is DERIVED from the token rather than tabulated here, and holds for
/// every capability type including any the model gains later.
pub fn capability_display_name(config_type: &str) -> String {
    config_type
        .split('-')
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The `(configType, configId)` pair a `ConfigArn` addresses, or `None` when `arn` is
/// not one of this service's config ARNs.
///
/// The modelled `ConfigArn` pattern ends in `config/<type>/<uuid>(/<suffix>)?` (it is on
/// `configArn` in `smithy_query(query="operation", operation="GetConfig")`), so the two
/// segments after `:config/` are the very pair [`ComAmazonawsGroundstationGroundstationState::configs`]
/// is keyed on — which is what lets an ARN-valued reference be reported by the id and
/// type it names rather than echoed back whole.
pub fn config_arn_parts(arn: &str) -> Option<(String, String)> {
    let (_, resource) = arn.split_once(":config/")?;
    let mut segments = resource.split('/');
    let config_type = segments.next()?;
    let config_id = segments.next()?;
    (!config_type.is_empty() && !config_id.is_empty())
        .then(|| (config_type.to_string(), config_id.to_string()))
}

/// The NODE of the config `arn` addresses, when the ARN names one: the optional segment
/// the modelled `ConfigArn` pattern allows after the config's uuid (`config/<type>/<uuid>`
/// followed by `(/.{1,256})?` — the pattern on `GetConfig`'s `configArn`).
///
/// A config can carry several outputs, and that trailing segment is how a reference picks
/// ONE of them: it is what an `antenna-downlink-demod-decode` edge endpoint names when it
/// is wired to a particular output node of that config, which is the `outputNode` a
/// contact's dataflow details report for it.
pub fn config_arn_node(arn: &str) -> Option<String> {
    let (_, resource) = arn.split_once(":config/")?;
    let mut segments = resource.splitn(3, '/');
    segments.next()?;
    segments.next()?;
    let node = segments.next()?;
    (!node.is_empty()).then(|| node.to_string())
}

/// Whether the `ConfigArn` `reference` addresses the config whose own ARN is
/// `config_arn`: that ARN itself, or that ARN followed by one of the config's NODES.
///
/// The modelled `ConfigArn` pattern ends in `config/<type>/<uuid>(/.{1,256})?`, so a
/// reference may address either the config as a whole or one named output of it (see
/// [`config_arn_node`]) — both are references to that one config, which is what makes
/// them resolve to it and what makes it in use.
pub fn addresses_config(reference: &str, config_arn: &str) -> bool {
    reference == config_arn
        || reference
            .strip_prefix(config_arn)
            .is_some_and(|node| node.starts_with('/'))
}

/// What this service answers when a mission profile a request names does not exist: a
/// fixed literal that discloses neither the id nor the ARN that was asked for.
///
/// It lives here rather than in one handler because BOTH operations that resolve a
/// mission profile answer it — `GetMissionProfile`, which resolves one by
/// `{missionProfileId}`, and `ReserveContact`, which resolves one by `missionProfileArn`
/// — so it is one text of the service, not one text per operation. The DELETE of a
/// profile answers its own, id-bearing text instead
/// ([`mission_profile_delete_not_found_message`]).
pub const MISSION_PROFILE_NOT_FOUND: &str = "MissionProfile not found";

/// The message `DeleteMissionProfile` carries when `{missionProfileId}` names no profile:
/// the fixed words `Mission Profile ` … ` not found` around the one variable part, the
/// `{missionProfileId}` the request path carried.
///
/// A delete reports WHICH profile it could not find, because a caller deleting several at
/// once attributes the failure by that id; the reads of a profile answer the bare
/// [`MISSION_PROFILE_NOT_FOUND`] literal and `UpdateMissionProfile` answers its own
/// account-qualified text ([`mission_profile_mutation_not_found_message`]), so no one of
/// the three is derived from another.
pub fn mission_profile_delete_not_found_message(mission_profile_id: &str) -> String {
    format!("Mission Profile {mission_profile_id} not found")
}

/// The region to mint this request's ARNs in: the SigV4 credential scope's region,
/// which is the region the caller addressed, falling back to [`DEFAULT_REGION`].
///
/// A scope region that could not appear in an ARN (the model's `ConfigArn` pattern
/// allows `[-a-z0-9]{1,50}` there) falls back too, so a minted ARN always satisfies the
/// modelled pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is spellable in the region segment of this service's ARNs
/// (`[-a-z0-9]{1,50}`, from the `ConfigArn`/`MissionProfileArn` patterns).
fn is_arn_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 50
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// How many ids this process has minted: the part of an id that makes it UNIQUE.
///
/// A counter cannot repeat within a process, so no two ids this service hands out can
/// collide however fast they are minted — which a clock reading alone cannot promise.
static IDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A per-process random value, the part of an id that makes it UNGUESSABLE.
///
/// `RandomState` seeds each of its hashers with keys the standard library draws from the
/// OS once per process, so hashing a fixed value through one yields a value that differs
/// between runs and is not derivable from the request.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(&RandomState::new());
        ::std::hash::Hash::hash(&"com.amazonaws.groundstation", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// A fresh lowercase, hyphenated uuid — the `Uuid` shape every id member of this
/// service targets (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`),
/// in the version-4 layout (version nibble `4`, variant bits `10`).
///
/// The 16 bytes are a SHA-256 digest — from the maintained `sha2` crate, not written
/// here — of this process's random seed, the number of ids minted before this one and
/// the current clock reading, rendered by `hex`. The counter is what makes an id unique
/// and the seed is what makes it unguessable; digesting them is what spreads both across
/// all 16 bytes, so an id reveals neither.
///
/// It does NOT use the `uuid` crate, which would otherwise be the obvious choice: taking
/// a new dependency rewrites the workspace `Cargo.lock`, the one file every concurrently
/// landing change in this repository also rewrites, and this change has to merge beside
/// them. `sha2` and `hex` are already in this crate's dependency closure, so minting the
/// id from them leaves the lockfile untouched.
pub fn new_uuid() -> String {
    use ::sha2::Digest as _;

    let minted = IDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let mut bytes = <[u8; 16]>::try_from(&digest.finalize()[..16]).expect("16 of 32 digest bytes");

    // The version-4 layout: `4` in the high nibble of byte 6, `10` in the top two bits
    // of byte 8. Everything else stays as digested.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `now` as a Smithy timestamp.
pub fn now_timestamp() -> DateTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    DateTime::from_secs_f64(secs)
}

/// `timestamp`, shifted by `seconds` (negative shifts earlier).
pub fn shift_seconds(timestamp: &DateTime, seconds: i64) -> DateTime {
    DateTime::from_secs_f64(timestamp.as_secs_f64() + seconds as f64)
}

/// The scope a listing's pagination tokens are issued under: the listing's own name,
/// plus a fingerprint of the filters the sequence it pages was cut by when it has any.
///
/// A page boundary is only meaningful against the sequence it was taken from, so a token
/// carries its scope: a token from another listing, or from the same listing under
/// different filters, is then refused rather than silently resuming inside a different
/// sequence (skipping or repeating records). The fingerprint is a non-cryptographic hash
/// — it only has to change when the filters do.
///
/// `DefaultHasher`'s output is NOT stable across process runs or std versions, which is
/// fine and deliberate: a token only ever addresses a boundary in this process's
/// in-memory store, so it is not meant to survive a restart (after one there is no
/// sequence left to resume), and a token from an earlier run is refused rather than
/// resumed.
pub fn page_scope(listing: &str, filters: &[String]) -> String {
    if filters.is_empty() {
        return listing.to_string();
    }
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    // Hashed as ONE string joined on US (0x1f), a byte none of this service's filter
    // values can contain (every one is an ARN, a ground-station name, an enum value or a
    // timestamp), so no two different filter sets can run together into the same input.
    ::std::hash::Hash::hash(&filters.join("\u{1f}"), &mut hasher);
    format!("{listing}.{:016x}", ::std::hash::Hasher::finish(&hasher))
}

/// One field of a [`PageCursor`]: the value one of the members a listing is ORDERED by
/// carries, held so that comparing two cursors field by field puts them in exactly the
/// order the listing holds the items they were read from.
///
/// The three forms are ordered as declared — an absent optional member before every
/// present one, as `Option` orders `None` before `Some` — so a listing ordered on an
/// optional member yields cursors ordered the same way. Within one listing a given
/// position carries one form (plus [`CursorField::Absent`] where the member is optional),
/// because it is read off the same member of every item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CursorField {
    /// An optional order member that carries nothing.
    Absent,
    /// A number, ordered as numbers are. An instant is one of these
    /// ([`CursorField::instant`]).
    Number(i64),
    /// Text, ordered as `str` orders text — the order a `sort` of the same values gives.
    Text(String),
}

impl CursorField {
    /// The field for the text `value`.
    pub fn text(value: &str) -> Self {
        Self::Text(value.to_string())
    }

    /// The field for an optional text member: [`CursorField::Absent`] when it carries
    /// nothing.
    pub fn optional_text(value: Option<&str>) -> Self {
        value.map_or(Self::Absent, Self::text)
    }

    /// The field for an optional number member: [`CursorField::Absent`] when it carries
    /// nothing.
    pub fn optional_number(value: Option<i32>) -> Self {
        value.map_or(Self::Absent, |number| Self::Number(i64::from(number)))
    }

    /// The field for the instant `value`.
    ///
    /// Held as the integer that orders as `f64::total_cmp` orders the seconds the instant
    /// carries — its bit pattern, with the sign folded in — so no rounding can put two
    /// instants in an order the listing does not hold them in.
    pub fn instant(value: DateTime) -> Self {
        let bits = value.as_secs_f64().to_bits() as i64;
        Self::Number(bits ^ (((bits >> 63) as u64) >> 1) as i64)
    }

    /// The field for an optional instant member: [`CursorField::Absent`] when it carries
    /// nothing.
    pub fn optional_instant(value: Option<DateTime>) -> Self {
        value.map_or(Self::Absent, Self::instant)
    }

    /// This field inside a token: a one-letter tag naming the form, then the value in
    /// characters the model's `PaginationToken` pattern admits (text is hex, which a
    /// ground-station name's spaces and colons are not).
    fn render(&self) -> String {
        match self {
            Self::Absent => "a".to_string(),
            Self::Number(number) => format!("n{number}"),
            Self::Text(text) => format!("t{}", hex::encode(text)),
        }
    }

    /// The field [`CursorField::render`] wrote, or `None` when `part` is not one.
    fn parse(part: &str) -> Option<Self> {
        let mut characters = part.chars();
        let tag = characters.next()?;
        let value = characters.as_str();
        match tag {
            'a' if value.is_empty() => Some(Self::Absent),
            'n' => value.parse().ok().map(Self::Number),
            't' => hex::decode(value)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(Self::Text),
            _ => None,
        }
    }
}

/// Which form ONE position of a listing's cursor carries — the shape of the member the
/// listing takes that part of its order on, as it writes it.
///
/// One field list of a listing has a fixed arity and a fixed form per position, because
/// every field is read off the same member of every item it covers: a required text member
/// yields [`CursorFieldShape::text`], an optional one [`CursorFieldShape::optional_text`]
/// (text, or [`CursorField::Absent`] where the member carries nothing), and the number and
/// instant members the matching constructors below. A listing whose order is composed of
/// PARTS that take their key off different members writes one field list per part, each
/// naming its own part with a [`CursorFieldShape::marker`], and names them all to
/// [`decode_page_token`].
///
/// [`decode_page_token`] reads these to tell a token the listing ISSUED from one that merely
/// starts with its scope: a field list of another arity, or another form at some position,
/// is not a boundary in the listing's order and is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorFieldShape {
    /// Whether the position carries [`CursorField::Absent`] — only where the member it is
    /// read off is optional.
    absent: bool,
    /// Whether the position carries [`CursorField::Number`].
    number: bool,
    /// The ONE number this position carries, where the listing writes a fixed value there
    /// rather than a member's: see [`CursorFieldShape::marker`]. `None` where any number
    /// the member can hold belongs here.
    marker: Option<i64>,
    /// Whether the position carries [`CursorField::Text`].
    text: bool,
}

impl CursorFieldShape {
    /// The shape of a position read off a REQUIRED text member
    /// ([`CursorField::text`]).
    pub const fn text() -> Self {
        Self {
            absent: false,
            number: false,
            marker: None,
            text: true,
        }
    }

    /// The shape of a position read off an OPTIONAL text member
    /// ([`CursorField::optional_text`]).
    pub const fn optional_text() -> Self {
        Self {
            absent: true,
            number: false,
            marker: None,
            text: true,
        }
    }

    /// The shape of a position read off a REQUIRED number member
    /// ([`CursorField::Number`]), an instant among them ([`CursorField::instant`], which
    /// holds the instant as the integer that orders as the instant does).
    pub const fn number() -> Self {
        Self {
            absent: false,
            number: true,
            marker: None,
            text: false,
        }
    }

    /// The shape of a position read off an OPTIONAL number or instant member
    /// ([`CursorField::optional_number`], [`CursorField::optional_instant`]).
    pub const fn optional_number() -> Self {
        Self {
            absent: true,
            number: true,
            marker: None,
            text: false,
        }
    }

    /// The shape of a position carrying the fixed number `value`: the marker a listing
    /// whose order is COMPOSED of parts writes to say which part an item belongs to.
    ///
    /// A part's field list admits only ITS marker, so the key of one part cannot be read
    /// behind the marker of another — a field list no part writes, and no position in the
    /// listing's order.
    pub const fn marker(value: i64) -> Self {
        Self {
            absent: false,
            number: true,
            marker: Some(value),
            text: false,
        }
    }

    /// Whether this position of the listing's cursor carries `field`.
    fn admits(&self, field: &CursorField) -> bool {
        match field {
            CursorField::Absent => self.absent,
            // A marked position carries ITS number and no other.
            CursorField::Number(number) => {
                self.number && self.marker.is_none_or(|marker| marker == *number)
            }
            CursorField::Text(_) => self.text,
        }
    }
}

/// Where a page of a listing ENDED: the ordering key of the last item it returned, which
/// is what its `nextToken` carries so the next page resumes after that item.
///
/// A position in the sequence rather than a COUNT of items: every listing of this service
/// recomputes its sequence from the live store on every call (and an availability search
/// computes it from the clock too), so between two pages an item can leave the sequence —
/// a contact reserved over an offered window, a config deleted, an ephemeris expired —
/// and everything behind it then sits one place earlier. A count would address a
/// different item after that and the items it passed over would be returned by no page of
/// the walk; a key addresses the boundary itself, so the next page is the items ordered
/// after it whatever has since left or joined.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageCursor(Vec<CursorField>);

impl PageCursor {
    /// The cursor for an item a listing orders by `fields`, most significant first.
    pub fn new(fields: Vec<CursorField>) -> Self {
        Self(fields)
    }

    /// The cursor for an item a listing orders by one text member — an id, for most of
    /// them.
    pub fn text(value: &str) -> Self {
        Self::new(vec![CursorField::text(value)])
    }

    /// The cursor for an item a listing orders by one optional text member.
    pub fn optional_text(value: Option<&str>) -> Self {
        Self::new(vec![CursorField::optional_text(value)])
    }
}

/// A pagination token for "resume `scope` after the item `cursor` names".
///
/// Spelled `<scope>.<field>.<field>…`: every character is inside the model's
/// `PaginationToken` pattern (`^[A-Za-z0-9-/+_.=]+$`) and needs no escaping in the query
/// string `ListMissionProfiles` binds its `nextToken` to, and the shortest form clears
/// the shape's 3-character minimum.
pub fn encode_page_token(scope: &str, cursor: &PageCursor) -> String {
    let fields: Vec<String> = cursor.0.iter().map(CursorField::render).collect();
    format!("{scope}.{}", fields.join("."))
}

/// The cursor a token issued by [`encode_page_token`] for `scope` carries, or `None` when
/// the token was not issued by this service for that scope — including a token whose field
/// list is not one the listing under `scope` ever writes.
///
/// `cursors` is every field list that listing WRITES — one per part of its order, each
/// position by position as the listing writes it (its [`CursorFieldShape`] for the member
/// the order is taken on there); a listing ordered on one set of members names exactly one.
/// A token is a page boundary only in the order the listing holds its items in, and a field
/// list of another LENGTH, or carrying another FORM at some position, names no place in that
/// order: [`CursorField`]'s ordering would still compare it against every item's cursor — an
/// `Absent` before all of them, a `Number` before every `Text`, a `Text` after every number
/// — so such a token, honoured, would answer a page the sequence never had a boundary for
/// (its first page, or none of it at all) instead of being refused. So the decoded list must
/// match ONE of `cursors` whole — its arity, every field's form, and the marker naming the
/// part a field list belongs to ([`CursorFieldShape::marker`]), so one part's key cannot be
/// read behind another's marker either — and only a field list this listing issues decodes.
pub fn decode_page_token(
    scope: &str,
    cursors: &[&[CursorFieldShape]],
    token: &str,
) -> Option<PageCursor> {
    let fields = token.strip_prefix(scope)?.strip_prefix('.')?;
    let cursor: Vec<CursorField> = fields
        .split('.')
        .map(CursorField::parse)
        .collect::<Option<Vec<CursorField>>>()?;
    let issuable = cursors.iter().any(|written| {
        written.len() == cursor.len()
            && written
                .iter()
                .zip(cursor.iter())
                .all(|(position, field)| position.admits(field))
    });
    issuable.then(|| PageCursor::new(cursor))
}

/// Largest page any listing returns, and the ceiling on a caller's `maxResults`: the
/// modelled maximum of `PaginationMaxResults` (`smithy_query(query="shape",
/// shape="PaginationMaxResults")` — range 1..100). A request that names no page size
/// gets this bounded page and a `nextToken`, rather than the whole store in one answer.
pub const MAX_PAGE_SIZE: usize = 100;

/// Largest number of records this emulator keeps in any one of its stores.
///
/// NOT an AWS quota — AWS Ground Station's own limits are per-account and adjustable.
/// It is a bound on the shared in-process store, so a caller cannot grow the emulator's
/// memory without limit; a create that would pass it is refused with the service's
/// modelled `ResourceLimitExceededException` (HTTP 429, "Account limits for this
/// resource have been exceeded"), which is what the service answers when a create runs
/// past an account limit.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// One page of `items`, plus the token that resumes AFTER it.
///
/// `items` is the listing's whole sequence in its own order and `key_of` reads an item's
/// place in that order ([`PageCursor`]), which the caller keeps ASCENDING and distinct
/// over the sequence: the page is the first [`page_size`] items ordered after the cursor
/// `after` carries, so an item that left the sequence since the previous page cannot hide
/// the items behind it from the walk, and the token this hands back names the last item
/// of the page rather than a count of the ones before it.
///
/// The page is at most [`MAX_PAGE_SIZE`] items whatever the caller asked for, so no
/// single request can draw the whole store out in one answer.
pub fn paginate<T: Clone>(
    scope: &str,
    items: &[T],
    max_results: Option<i32>,
    after: Option<&PageCursor>,
    key_of: impl Fn(&T) -> PageCursor,
) -> (Vec<T>, Option<String>) {
    let start = match after {
        None => 0,
        // Everything up to and including the item the previous page ended on is behind the
        // boundary; the sequence is ascending, so that prefix is found by bisection.
        Some(after) => items.partition_point(|item| key_of(item) <= *after),
    };
    let limit = page_size(max_results);
    let end = start.saturating_add(limit).min(items.len());
    let page = items[start..end].to_vec();
    // `end` is past `start` whenever anything remains, so the page has a last item to name.
    let next = (end < items.len()).then(|| encode_page_token(scope, &key_of(&items[end - 1])));
    (page, next)
}

/// How many items one page holds for a caller's `maxResults`: what it asked for, bounded by
/// [`MAX_PAGE_SIZE`], and that bound when it asked for nothing.
///
/// [`paginate`] reads this to cut the page; a listing that COMPUTES its items reads it to
/// know how many it can be asked for at most, so it materialises no more than the page it
/// is answering can carry.
pub fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        Some(max) if max > 0 => (max as usize).min(MAX_PAGE_SIZE),
        _ => MAX_PAGE_SIZE,
    }
}

/// A response list member whose modelled shape admits NO empty list, the way a response of
/// this service carries it: the entries when it has any, and ABSENT when it has none.
///
/// A list shape with a non-zero minimum length forbids the empty list, so a member
/// targeting one reports "nothing to report" by being left out: serializing `[]` there
/// would answer with a value outside the member's own modelled constraint. This is the
/// reading this service's reads already take for a `len 1..` member they have nothing for,
/// and it is the opposite of how a member whose shape admits zero entries answers — that
/// one stays present and empty, as [`response_tags`] keeps a tag map present.
///
/// This is where a member of this service whose list shape carries a non-zero minimum
/// COLLECTS its entries, so the reading is written once rather than per operation. A
/// member that can never have an entry to collect states it directly instead —
/// `DescribeEphemeris$errorReasons`, which this emulator's validation records no detail
/// for, is reported absent at its own site.
pub fn non_empty_list<T>(entries: Vec<T>) -> Option<Vec<T>> {
    (!entries.is_empty()).then_some(entries)
}

/// Whether a listing's list-valued filter member, which NAMED the values `named`, admits
/// an item carrying `value`.
///
/// A filter that names NO values narrows nothing. Every list-valued filter member this
/// service has is modelled with a minimum length of zero — `ListContacts$statusList` and
/// `ListEphemerides$statusList` are `len 0..500` and
/// `ListGroundStationReservations$reservationTypes` is unbounded below
/// (`smithy_query(query="operation", operation="ListContacts,ListEphemerides,\
/// ListGroundStationReservations")`) — and each is documented as the values to RETURN, so
/// an empty list is a request that names nothing to narrow by rather than a request for
/// nothing; an absent optional one is the same request, written by leaving the member out.
/// Stated once here so the same request shape means the same thing in every listing of
/// this service rather than one thing per handler.
pub fn list_filter_admits(named: &[String], value: &str) -> bool {
    named.is_empty() || named.iter().any(|wanted| wanted == value)
}

/// The two members a window-filtered listing is asked for its window in, spelled once so
/// the refusal below names and quotes the same members every listing binds.
const SEARCH_WINDOW_START_PARAMETER: &str = "startTime";

/// See [`SEARCH_WINDOW_START_PARAMETER`].
const SEARCH_WINDOW_END_PARAMETER: &str = "endTime";

/// Refuse a search window that does not run forwards, with the modelled
/// `InvalidParameterException` (HTTP 431) naming [`SEARCH_WINDOW_END_PARAMETER`].
///
/// The window a listing searches runs forwards: a request whose `endTime` precedes its
/// `startTime` describes no interval at all, so it is refused rather than answered with
/// the empty page the window test would produce. A window naming the same instant twice
/// still describes an interval — the CLOSED reading [`windows_overlap`] applies — so it is
/// answered.
///
/// Every window-filtered listing of this service searches its window the same way, so
/// they read ONE rule here and map it into their own error enum, the way they already do
/// for [`validate_tags`]: a change to the comparison, or to the member the refusal names,
/// is then made once rather than once per listing.
pub fn validate_search_window(
    start_time: DateTime,
    end_time: DateTime,
) -> Result<(), InvalidParameterException> {
    if end_time.as_secs_f64() < start_time.as_secs_f64() {
        return Err(invalid_parameter(
            SEARCH_WINDOW_END_PARAMETER,
            format!(
                "{SEARCH_WINDOW_END_PARAMETER} must not precede {SEARCH_WINDOW_START_PARAMETER}"
            ),
        ));
    }
    Ok(())
}

/// What an ARN this service minted points at, so an ARN-valued reference resolves in
/// one lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A config, by the `(configType, configId)` pair its map is keyed on.
    Config {
        config_type: String,
        config_id: String,
    },
    /// A mission profile, by `missionProfileId`.
    MissionProfile { mission_profile_id: String },
    /// A contact, by `contactId`.
    Contact { contact_id: String },
    /// A dataflow endpoint group, by `dataflowEndpointGroupId`.
    DataflowEndpointGroup { dataflow_endpoint_group_id: String },
    /// An AWS Ground Station Agent, by `agentId`.
    Agent { agent_id: String },
    /// An ephemeris, by `ephemerisId`.
    Ephemeris { ephemeris_id: String },
}

/// A config as `CreateConfig` stored it.
#[derive(Debug, Clone)]
pub struct Config {
    pub config_id: String,
    pub config_arn: String,
    pub name: String,
    pub config_type: String,
    pub config_data: ConfigTypeData,
    pub tags: HashMap<String, String>,
}

impl Config {
    /// Whether this config references the config `config_arn` names, through ANY member of
    /// its `configData` that carries a `ConfigArn`.
    ///
    /// One member of the union does: an `uplinkEchoConfig` echoes the `antenna-uplink`
    /// config its `antennaUplinkConfigArn` names (`smithy_query(query="shape",
    /// shape="ConfigTypeData,UplinkEchoConfig")` — the only `ConfigArn`-targeted member of
    /// the eight capability bodies). It is matched with [`addresses_config`], like every
    /// other inbound reference, so a reference naming one NODE of a config is as much a
    /// reference to it as one naming the config whole.
    ///
    /// This is what makes the referenced config "in use" for [`DeleteConfig`]'s referential
    /// integrity check, beside the mission profile's references
    /// ([`MissionProfile::references_config`]).
    ///
    /// [`DeleteConfig`]: crate::ops::delete_config
    pub fn references_config(&self, config_arn: &str) -> bool {
        match &self.config_data {
            ConfigTypeData::uplinkEchoConfig(echo) => {
                addresses_config(&echo.antenna_uplink_config_arn, config_arn)
            }
            ConfigTypeData::antennaDownlinkConfig(_)
            | ConfigTypeData::antennaDownlinkDemodDecodeConfig(_)
            | ConfigTypeData::antennaUplinkConfig(_)
            | ConfigTypeData::dataflowEndpointConfig(_)
            | ConfigTypeData::s3RecordingConfig(_)
            | ConfigTypeData::telemetrySinkConfig(_)
            | ConfigTypeData::trackingConfig(_) => false,
        }
    }
}

/// A mission profile as `CreateMissionProfile` stored it.
#[derive(Debug, Clone)]
pub struct MissionProfile {
    pub mission_profile_id: String,
    pub mission_profile_arn: String,
    pub name: String,
    pub region: String,
    pub contact_pre_pass_duration_seconds: Option<i32>,
    pub contact_post_pass_duration_seconds: Option<i32>,
    pub minimum_viable_contact_duration_seconds: i32,
    pub dataflow_edges: Vec<Vec<String>>,
    pub tracking_config_arn: String,
    pub telemetry_sink_config_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub streams_kms_key: Option<KmsKey>,
    pub streams_kms_role: Option<String>,
}

impl MissionProfile {
    /// Whether this profile references the config `config_arn` names, through ANY member
    /// that carries a `ConfigArn`: its tracking config, its telemetry sink config, or
    /// either end of any of its dataflow edges.
    ///
    /// This is what makes the config "in use" for [`DeleteConfig`]'s referential
    /// integrity check, so the question is asked of the whole profile rather than of one
    /// member.
    ///
    /// [`DeleteConfig`]: crate::ops::delete_config
    pub fn references_config(&self, config_arn: &str) -> bool {
        // Each reference is matched with [`addresses_config`], so an edge wired to one
        // NODE of a config is as much a reference to it as one naming the config whole.
        addresses_config(&self.tracking_config_arn, config_arn)
            || self
                .telemetry_sink_config_arn
                .as_deref()
                .is_some_and(|arn| addresses_config(arn, config_arn))
            || self.dataflow_edges.iter().any(|edge| {
                edge.iter()
                    .any(|arn| addresses_config(arn, config_arn))
            })
    }
}

/// `tags` the way a response of this service carries them: the resource's tag map,
/// SERIALIZED even when the resource carries no tag — `{}` then, never an omitted
/// member.
///
/// A `TagsMap`-targeting response member of this service reports how a resource is
/// tagged, and "no tags" is a map with no entry, not the absence of an answer: a client
/// that diffs or iterates tags reads a map in both cases. Every read that reports tags
/// goes through here, so the whole service answers the same way.
pub fn response_tags(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// The request member every tag map of this service arrives in, and the member a refusal
/// of one names — `tags` on each operation that takes a `TagsMap`
/// (`smithy_query(query="operation", operation="CreateConfig,CreateMissionProfile,\
/// CreateDataflowEndpointGroup,ReserveContact")`).
pub const TAGS_PARAMETER: &str = "tags";

/// The tag-key prefix AWS reserves for itself: a caller may neither set nor remove a key
/// that starts with it, on any AWS resource.
///
/// Source: the AWS tagging rules every taggable service shares ("the `aws:` prefix is
/// reserved for AWS use … you can't edit or delete a tag with the `aws:` prefix"). The
/// model cannot express it — `TagsMap` is a bare `map<String,String>` with no `length`
/// or `pattern` trait (`smithy_query(query="shape", shape="TagsMap")`) — so the rule
/// lives here, where every create of this service reaches it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, matched in
/// ANY letter case: the namespace is AWS's whichever way a caller spells it, so
/// `AWS:owner` is as reserved as `aws:owner`. The same reading the other services of
/// this emulator enforce, so one tag map is judged alike wherever it is sent.
pub fn is_reserved_tag_key(key: &str) -> bool {
    // `str::get` answers `None` rather than panicking when the key's first four bytes
    // are not a character boundary, so a key that cannot carry the prefix is simply
    // not one.
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The inclusive length bounds of a tag KEY, in characters, from the same AWS tagging
/// rules: a key is 1 to 128 characters.
pub const TAG_KEY_CHARS: (usize, usize) = (1, 128);

/// The largest tag VALUE, in characters, from the same rules. A value may be empty, so
/// only the ceiling is a bound.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The most tags one resource may carry, from the same rules.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Refuse a `tags` map AWS's platform-wide tagging rules do not allow, with the modelled
/// `InvalidParameterException` (HTTP 431) naming the [`TAGS_PARAMETER`] member.
///
/// `TagsMap` carries no constraint in the model, so the boundary validator lets every map
/// through and this is the only gate: a tag map is checked HERE, before any id, ARN or
/// record is minted, so a caller never holds a resource AWS would have refused to tag
/// that way. Every create of this service that accepts tags calls it, as does
/// `TagResource` (which additionally judges the MERGED map against the per-resource
/// bound — see [`validate_tag_merge`]), so one illegal map is refused identically
/// whichever resource and whichever operation it was sent to.
///
/// The conditions, each from a constant above: more than [`MAX_TAGS_PER_RESOURCE`]
/// entries (a property of the map as a whole, so it is judged first — the request is
/// refused however legal each entry is), a key in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace in any letter case ([`is_reserved_tag_key`]),
/// a key outside [`TAG_KEY_CHARS`], or a value
/// longer than [`MAX_TAG_VALUE_CHARS`]. Lengths are counted in CHARACTERS rather than
/// bytes, because that is the unit the rules are written in, and the entries are walked
/// in key order so the same bad map is always refused for the same reason.
///
/// The status (431) and the error code are the contract; the text names the member and
/// the constraint it failed, built from the request.
pub fn validate_tags(
    tags: Option<&HashMap<String, String>>,
) -> Result<(), InvalidParameterException> {
    let Some(tags) = tags else {
        return Ok(());
    };

    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(invalid_parameter(
            TAGS_PARAMETER,
            format!(
                "Value at '{TAGS_PARAMETER}' carries {} tags; a resource may carry at most \
                 {MAX_TAGS_PER_RESOURCE} tags.",
                tags.len()
            ),
        ));
    }

    let (min_key, max_key) = TAG_KEY_CHARS;
    let mut entries: Vec<(&String, &String)> = tags.iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    for (key, value) in entries {
        if is_reserved_tag_key(key) {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAGS_PARAMETER}' uses the reserved \
                     '{RESERVED_TAG_KEY_PREFIX}' prefix, which only AWS may set."
                ),
            ));
        }
        let key_chars = key.chars().count();
        if key_chars < min_key || key_chars > max_key {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAGS_PARAMETER}' is {key_chars} characters long; a \
                     tag key must be {min_key} to {max_key} characters."
                ),
            ));
        }
        let value_chars = value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(invalid_parameter(
                TAGS_PARAMETER,
                format!(
                    "Tag value at '{TAGS_PARAMETER}.{key}' is {value_chars} characters long; a \
                     tag value must be at most {MAX_TAG_VALUE_CHARS} characters."
                ),
            ));
        }
    }

    Ok(())
}

/// Refuse a tag ASSIGNMENT whose merge would leave the resource carrying more than
/// [`MAX_TAGS_PER_RESOURCE`] tags, with the modelled `InvalidParameterException`
/// (HTTP 431) naming the [`TAGS_PARAMETER`] member.
///
/// [`validate_tags`] judges the map a request CARRIES; this judges the map the resource
/// would be left with, which is the quantity the rule is actually about ("the most tags
/// one resource may carry"): assigning tags merges them into what the resource already
/// holds, so a legal request map can still take the resource past the bound. A key the
/// resource already holds is an OVERWRITE and adds nothing to the count, so only the
/// genuinely new keys are counted.
///
/// Checked before anything is written, so a refused assignment leaves the resource
/// exactly as it was rather than half-tagged.
pub fn validate_tag_merge(
    stored: &HashMap<String, String>,
    assigned: &HashMap<String, String>,
) -> Result<(), InvalidParameterException> {
    let added = assigned
        .keys()
        .filter(|key| !stored.contains_key(*key))
        .count();
    let merged = stored.len() + added;
    if merged > MAX_TAGS_PER_RESOURCE {
        return Err(invalid_parameter(
            TAGS_PARAMETER,
            format!(
                "Value at '{TAGS_PARAMETER}' would leave the resource carrying {merged} tags; a \
                 resource may carry at most {MAX_TAGS_PER_RESOURCE} tags."
            ),
        ));
    }
    Ok(())
}

/// The request member a tag REMOVAL names its keys in — `tagKeys` on `UntagResource`
/// (`smithy_query(query="operation", operation="UntagResource")`), the removal's
/// counterpart to [`TAGS_PARAMETER`].
pub const TAG_KEYS_PARAMETER: &str = "tagKeys";

/// Refuse a tag-key list that names a key AWS's platform-wide tagging rules do not let a
/// caller hold, with the modelled `InvalidParameterException` (HTTP 431) naming the
/// [`TAG_KEYS_PARAMETER`] member.
///
/// The REMOVAL counterpart of [`validate_tags`]: one set of rules decides which keys are
/// a caller's, so a key a caller may not set is a key it may not remove either — a key in
/// the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace in any letter case
/// ([`is_reserved_tag_key`]), which names a tag AWS itself maintains, or a key past
/// [`TAG_KEY_CHARS`]'s ceiling, which no tag of a resource can be spelled with. The floor
/// on a key and the length of the list are MODELLED (`smithy_query(query="shape",
/// shape="TagKeys")` — a `0..500` list of `length min 1` members), so the boundary
/// validator has already refused an empty key and an over-long list before this runs.
///
/// Judged from the request alone, before any ARN is resolved and before anything is
/// removed, so one illegal key costs the whole call and every tag stays as it was —
/// including the keys of the same request that ARE the caller's to remove. Keys are
/// walked in the order the request wrote them, so the same list is always refused for the
/// same reason.
///
/// The status (431) and the error code are the contract; the text names the member and the
/// constraint it failed, built from the request.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), InvalidParameterException> {
    let (_, max_key) = TAG_KEY_CHARS;
    for key in keys {
        if is_reserved_tag_key(key) {
            return Err(invalid_parameter(
                TAG_KEYS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAG_KEYS_PARAMETER}' uses the reserved \
                     '{RESERVED_TAG_KEY_PREFIX}' prefix, whose tags only AWS may change."
                ),
            ));
        }
        let key_chars = key.chars().count();
        if key_chars > max_key {
            return Err(invalid_parameter(
                TAG_KEYS_PARAMETER,
                format!(
                    "Tag key '{key}' at '{TAG_KEYS_PARAMETER}' is {key_chars} characters long; a \
                     tag key must be at most {max_key} characters."
                ),
            ));
        }
    }

    Ok(())
}

/// The message every operation addressed at a config by its `(configType, configId)`
/// pair carries when that pair names no config — built from the two `@httpLabel`
/// members of the request, id before type, which is the rule the recorded corpus shows
/// for this service's config reads.
///
/// It lives here rather than in one handler because the two operations that LOOK UP a
/// config by that pair answer it — `GetConfig` and `DeleteConfig`. The operation that
/// WRITES one does not: see [`config_mutation_not_found_message`].
pub fn config_not_found_message(config_type: &str, config_id: &str) -> String {
    format!("No config found with id {config_id} and type {config_type}")
}

/// The message `UpdateConfig` carries when the pair it addressed names no config.
///
/// The MUTATION path composes its own text, from the same two `@httpLabel` members plus
/// the account the write was scoped to: `No config found with ID <configId> and type
/// <configType> for customer <accountId>` — capital `ID` and an account-qualified tail,
/// where the read path ([`config_not_found_message`]) names only the pair. The
/// composition rule is the requirement; the two are separate functions because the read
/// and the write genuinely answer different texts, so neither can be derived from the
/// other.
pub fn config_mutation_not_found_message(config_type: &str, config_id: &str) -> String {
    format!(
        "No config found with ID {config_id} and type {config_type} for customer {}",
        ::engine::emulator_account_id()
    )
}

/// The message `UpdateMissionProfile` carries when `{missionProfileId}` names no
/// profile: the id the request addressed plus the account the write was scoped to, where
/// the read path answers this service's bare [`MISSION_PROFILE_NOT_FOUND`] literal.
pub fn mission_profile_mutation_not_found_message(mission_profile_id: &str) -> String {
    format!(
        "No mission profile found with ID {mission_profile_id} for customer {}",
        ::engine::emulator_account_id()
    )
}

/// The message `UpdateContact` carries when `{contactId}` names no contact: the fixed
/// text `Contact not found: ` followed by the one variable part, the `{contactId}` the
/// request path carried.
///
/// The id is the ONLY thing interpolated — nothing of the environment (the account the
/// write was scoped to) belongs in it, so the text reads the same wherever this emulator
/// runs. `DescribeContact` answers a bare literal for the same missing contact and
/// `CancelContact` quotes the id in a sentence of its own: each operation carries the text
/// of its own error class.
pub fn contact_update_not_found_message(contact_id: &str) -> String {
    format!("Contact not found: {contact_id}")
}

/// What `DeleteConfig` answers while a live mission profile still references the config:
/// the refusal names the referencing mission-profile ARNs, which is what tells a caller
/// what to delete first.
///
/// The status (431) and the error code (`InvalidParameterException`) are the contract;
/// this is the wording that goes with them, and the ARNs in it are the ones the store
/// holds rather than anything echoed from the request.
pub fn config_in_use_message(mission_profile_arns: &[String]) -> String {
    resource_in_use_text("Config", "Mission Profiles", mission_profile_arns)
}

/// What `DeleteConfig` answers while another config this account still holds references
/// the config — an `uplinkEchoConfig` echoing the `antenna-uplink` config the delete
/// addressed: the refusal names the referencing config ARNs, which is what tells a caller
/// what to delete first.
///
/// Same status (431), error code (`InvalidParameterException`) and sentence as the
/// mission-profile refusal above; only the kind of resource that holds the reference
/// differs, so the two are composed from one template rather than written twice.
pub fn config_in_use_by_configs_message(config_arns: &[String]) -> String {
    resource_in_use_text("Config", "Configs", config_arns)
}

/// The in-use refusal a delete of this service answers: the resource that cannot be
/// deleted, the plural name of the kind of thing that holds it, and the holders the store
/// holds.
///
/// ONE template for every such refusal ([`config_in_use_message`],
/// [`config_in_use_by_configs_message`], [`mission_profile_in_use_message`],
/// [`dataflow_endpoint_group_in_use_message`] and [`ephemeris_in_use_message`]), so the
/// sentence cannot drift between the deletes that answer it; only what is held and what
/// holds it differs.
fn resource_in_use_text(resource: &str, holder_kind_plural: &str, holders: &[String]) -> String {
    resource_in_use_text_for(resource, "deleted", holder_kind_plural, holders)
}

/// The same sentence as [`resource_in_use_text`], for the write `action` the hold refuses:
/// a resource in use "cannot be modified or deleted" (`smithy_query(query="doc",
/// shape="ResourceInUseException")`), so a MODIFY of one is refused in the same words as
/// its delete and only the action named differs.
fn resource_in_use_text_for(
    resource: &str,
    action: &str,
    holder_kind_plural: &str,
    holders: &[String],
) -> String {
    format!(
        "{resource} cannot be {action}; it is currently being used by the following \
         {holder_kind_plural}: {}",
        holders.join(", ")
    )
}

/// What `DeleteMissionProfile` answers while a contact still to be flown names the
/// profile: the refusal names the contacts that hold it, which is what tells a caller
/// what has to be flown or released first.
///
/// The status (431) and the error code (`InvalidParameterException`) are the contract —
/// this operation models no `ResourceInUseException` (`smithy_query(query="operation",
/// operation="DeleteMissionProfile")`), so the in-use refusal is the bad-parameter one
/// the sibling config delete already answers. This is the wording that goes with it, and
/// the ids in it are the ones the store holds rather than anything echoed from the
/// request — the same composition as [`config_in_use_message`] and
/// [`ephemeris_in_use_message`].
pub fn mission_profile_in_use_message(contact_ids: &[String]) -> String {
    resource_in_use_text("Mission Profile", "contacts", contact_ids)
}

/// What `DeleteDataflowEndpointGroup` answers while a contact still to be flown is wired to
/// an endpoint the group holds: the refusal names the contacts that hold it, which is what
/// tells a caller what has to be flown or released first.
///
/// The status (431) and the error code (`InvalidParameterException`) are the contract — this
/// operation models no `ResourceInUseException` (`smithy_query(query="operation",
/// operation="DeleteDataflowEndpointGroup")`), so the in-use refusal is the bad-parameter
/// one its sibling deletes already answer — and this is the wording that goes with it, from
/// the one template every in-use refusal of this service is composed from
/// ([`resource_in_use_text`]). The ids in it are the ones the store holds rather than
/// anything echoed from the request.
pub fn dataflow_endpoint_group_in_use_message(contact_ids: &[String]) -> String {
    resource_in_use_text("Dataflow Endpoint Group", "contacts", contact_ids)
}

/// Why an ARN-valued reference that admits exactly ONE config capability type cannot be
/// used, when it cannot.
///
/// The two are distinct conditions and this service answers them differently: see
/// [`ComAmazonawsGroundstationGroundstationState::typed_reference_fault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedReferenceFault {
    /// The ARN resolves to no config this account holds: there is nothing to reference.
    Absent,
    /// It resolves to a config that EXISTS, under another capability type.
    WrongType,
}

/// What this service answers for such a reference that resolved to NO config: the
/// capability the member required, spelled as the model spells that capability's shape
/// ([`capability_display_name`]), and the ARN the request gave.
pub fn typed_reference_absent_message(required_type: &str, arn: &str) -> String {
    format!(
        "Unable to find {} Config with ARN: {arn}",
        capability_display_name(required_type)
    )
}

/// What this service answers for such a reference that resolved to a config of ANOTHER
/// capability type: the ARN named a config that exists, so nothing is missing — what is
/// wrong is the type it points at, and the text states the type the member requires as
/// the `ConfigCapabilityType` token a `ConfigArn`'s type segment carries.
pub fn typed_reference_wrong_type_message(required_type: &str) -> String {
    wrong_config_type_message(&[required_type])
}

/// The one config capability type an `uplinkEchoConfig`'s `antennaUplinkConfigArn` may
/// name — the member's own name and the `antenna-uplink` segment of a `ConfigArn` are what
/// say so (`smithy_query(query="shape", shape="UplinkEchoConfig,ConfigCapabilityType")`).
pub const UPLINK_ECHO_REFERENCED_TYPE: &str = "antenna-uplink";

/// The member a refusal of an `uplinkEchoConfig`'s reference names, shared by the create
/// and the update so both answer one parameter name (as [`DATAFLOW_EDGES_PARAMETER`] is one
/// name for a mission profile's graph).
pub const UPLINK_ECHO_REFERENCE_PARAMETER: &str =
    "configData.uplinkEchoConfig.antennaUplinkConfigArn";

/// Why the config an `uplinkEchoConfig` echoes cannot be referenced, as the modelled
/// refusal to answer with.
///
/// The two faults are different conditions and this service answers them differently, the
/// same way it answers them for a program track's ephemeris reference
/// ([`ProgramTrackFault`]) — see
/// [`ComAmazonawsGroundstationGroundstationState::uplink_echo_reference_fault`], which is
/// where the whole rule lives, so the create and the update of a config cannot drift apart
/// on it.
#[derive(Debug, Clone)]
pub enum UplinkEchoReferenceFault {
    /// The `antennaUplinkConfigArn` names no config this account holds: there is nothing to
    /// echo, so the resource is MISSING (`ResourceNotFoundException`, HTTP 434).
    Absent(ResourceNotFoundException),
    /// It names a config that EXISTS, of another capability type than
    /// [`UPLINK_ECHO_REFERENCED_TYPE`]: a BAD PARAMETER (`InvalidParameterException`,
    /// HTTP 431), since the reference resolved and no amount of waiting changes the type of
    /// the config it points at.
    WrongType(InvalidParameterException),
}

/// `types` as the sentence fragment a refusal names them in, each quoted as the
/// `ConfigCapabilityType` token it is: `"antenna-uplink"`, or `"dataflow-endpoint" or
/// "s3-recording"`, or `"a", "b" or "c"`.
fn quoted_config_types(types: &[&str]) -> String {
    let quoted: Vec<String> = types.iter().map(|token| format!("\"{token}\"")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} or {last}", leading.join(", ")),
    }
}

/// What this service answers for a config reference that resolved to a config whose
/// capability type the POSITION it was named in does not admit: the reference resolved,
/// so nothing is missing — what is wrong is the type it points at, and the text states
/// the capability types that position does admit.
///
/// One text for the whole class: a position admitting exactly one type
/// ([`typed_reference_wrong_type_message`], the recorded wording for
/// `uplinkEchoConfig$antennaUplinkConfigArn`) and one admitting several (either end of a
/// mission profile's dataflow edge) differ only in how many types the sentence lists.
pub fn wrong_config_type_message(admissible: &[&str]) -> String {
    format!(
        "Invalid ARN provided. Config Type must be of type {}.",
        quoted_config_types(admissible)
    )
}

/// The member a refusal of a mission profile's dataflow graph names, shared by the create
/// and the update so both answer one parameter name.
pub const DATAFLOW_EDGES_PARAMETER: &str = "dataflowEdges";

/// The member a refusal of a config's endpoint selection names: the part of the
/// `configData` a write of a config carries the selection in
/// (`smithy_query(query="shape", shape="ConfigTypeData,DataflowEndpointConfig")`), named
/// the way a refusal of the other `configData` member that carries a cross-resource
/// reference is ([`UPLINK_ECHO_REFERENCE_PARAMETER`]).
pub const DATAFLOW_ENDPOINT_NAME_PARAMETER: &str =
    "configData.dataflowEndpointConfig.dataflowEndpointName";

/// `names` as the sentence fragment a refusal lists them in, each quoted the way this
/// service already quotes an endpoint name: `"alpha" and "beta"`, or `"alpha", "beta" and
/// "gamma"`.
fn quoted_endpoint_names(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} and {last}", leading.join(", ")),
    }
}

/// What this service answers when the dataflow graph of a mission profile selects
/// endpoints no ONE dataflow endpoint group holds: the names at fault, and the rule they
/// break.
///
/// "When a contact uses multiple `DataflowEndpointConfig` objects, each `Config` must
/// match a `DataflowEndpoint` in the same group" (`smithy_query(query="operation",
/// operation="CreateDataflowEndpointGroup")`), so a graph whose endpoint selections cannot
/// all be served by one group describes a contact that cannot be flown.
pub fn dataflow_endpoints_of_different_groups_message(names: &[&str]) -> String {
    format!(
        "{} are endpoints of different dataflow endpoint groups; when a contact uses \
         multiple dataflowEndpointConfig objects, each Config must match a DataflowEndpoint \
         in the same group",
        quoted_endpoint_names(names)
    )
}

/// The ordered `(source, destination)` capability pairs a dataflow edge may join.
///
/// A dataflow edge is one dataflow of a contact, so its two ends are the capability that
/// ORIGINATES the data and the capability that RECEIVES it: data an antenna downlinks
/// (`antenna-downlink`, or `antenna-downlink-demod-decode` when the service demodulates
/// and decodes it first) is carried to a dataflow endpoint of the customer's VPC or
/// written to their bucket; data the customer hands to a dataflow endpoint is radiated by
/// the antenna (`antenna-uplink`); and the echo of an uplink (`uplink-echo`) is carried
/// back to a dataflow endpoint. The capability types that take part are the model's own:
/// `Source$configDetails` is "Additional details for a Config, if type is
/// dataflow-endpoint or antenna-downlink-demod-decode", `Destination$configDetails` the
/// same for a destination, and `ConfigDetails` carries `s3RecordingDetails`, "Details for
/// an S3 recording Config in a contact" (`smithy_query(query="shape",
/// shape="Source,Destination,ConfigDetails")`).
///
/// The two capability types that appear in NO pair are the two a mission profile wires up
/// through a member of its own rather than through the graph — `trackingConfigArn` is "ARN
/// of a tracking Config" and `telemetrySinkConfigArn` that of a telemetry sink
/// (`smithy_query(query="operation", operation="CreateMissionProfile")`) — so neither is a
/// node of a dataflow.
pub const DATAFLOW_EDGE_CAPABILITY_PAIRS: &[(&str, &str)] = &[
    ("antenna-downlink", "dataflow-endpoint"),
    ("antenna-downlink", "s3-recording"),
    ("antenna-downlink-demod-decode", "dataflow-endpoint"),
    ("antenna-downlink-demod-decode", "s3-recording"),
    ("dataflow-endpoint", "antenna-uplink"),
    ("uplink-echo", "dataflow-endpoint"),
];

/// The capability types a dataflow edge may name as its SOURCE: every distinct first
/// element of [`DATAFLOW_EDGE_CAPABILITY_PAIRS`], in the order the table lists them, so
/// the set is derived from the table rather than tabulated twice.
pub fn dataflow_edge_source_types() -> Vec<&'static str> {
    let mut types: Vec<&'static str> = Vec::new();
    for (source, _) in DATAFLOW_EDGE_CAPABILITY_PAIRS {
        if !types.contains(source) {
            types.push(source);
        }
    }
    types
}

/// The capability types a dataflow edge whose source is `source_type` may name as its
/// DESTINATION; empty when no dataflow starts at that capability.
pub fn dataflow_edge_destination_types(source_type: &str) -> Vec<&'static str> {
    DATAFLOW_EDGE_CAPABILITY_PAIRS
        .iter()
        .filter(|(source, _)| *source == source_type)
        .map(|(_, destination)| *destination)
        .collect()
}

/// The message an operation that resolves a config BY ARN — a mission profile's
/// `trackingConfigArn`, `telemetrySinkConfigArn` or a `dataflowEdges` endpoint — carries
/// when that reference resolves to nothing.
///
/// It identifies the config the lookup looked FOR: its id, the config type it searched
/// under, and the account the search was scoped to, rather than echoing the ARN back.
/// The id and the fallback type come from the ARN the request gave
/// ([`config_arn_parts`]); `required_type` is the type the member itself requires, which
/// is `None` for a dataflow edge, whose endpoints may be of any type. An ARN that is not
/// one of this service's config ARNs names no id to report, so the ARN is all there is
/// to say.
///
/// Shared by every operation that takes such a reference, so a create and an update of
/// the same profile answer one text.
pub fn config_reference_not_found_message(arn: &str, required_type: Option<&str>) -> String {
    let from_arn = config_arn_parts(arn);
    let searched_type = required_type
        .map(str::to_string)
        .or_else(|| from_arn.as_ref().map(|(config_type, _)| config_type.clone()));
    match (&from_arn, &searched_type) {
        (Some((_, config_id)), Some(config_type)) => format!(
            "No config found with ID {config_id} and config type: {config_type} for customer {}",
            ::engine::emulator_account_id()
        ),
        _ => format!("Unable to find Config with ARN: {arn}"),
    }
}

/// What this service answers when a dataflow endpoint group a request names does not
/// exist: the service's own fixed sentence for that error class, which names the kind of
/// resource and ends the sentence, and discloses neither the id nor the ARN that was asked
/// for.
///
/// It lives here rather than in one handler because every operation addressed at a group
/// by its `{dataflowEndpointGroupId}` label answers it — `GetDataflowEndpointGroup` and
/// `DeleteDataflowEndpointGroup` — so it is one text of the service: one resource, one
/// error class, one sentence. The read's wording is the recorded one; the delete reads it
/// from here rather than carrying a sentence of its own.
pub const DATAFLOW_ENDPOINT_GROUP_NOT_FOUND: &str = "The specified group does not exist.";

/// Which kind of endpoint an `EndpointDetails` describes.
///
/// The operation's own documentation partitions the endpoints exactly this way — "All
/// dataflow endpoints within a single dataflow endpoint group must be of the same type.
/// You cannot mix AWS Ground Station Agent endpoints with Dataflow endpoints in the same
/// group" (`smithy_query(query="operation", operation="CreateDataflowEndpointGroup")`) —
/// so the kind is what a group's endpoints have to agree on, and it is also what decides
/// which members the service derives for an endpoint rather than reads from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// The `endpoint` member: a `DataflowEndpoint` Ground Station itself connects the
    /// stream to, inside the customer's VPC.
    Dataflow,
    /// One of the three AWS Ground Station Agent members: the stream is carried by an
    /// agent the customer runs on an instance of their own.
    Agent,
}

impl EndpointKind {
    /// How a refusal names this kind, so a mixed group's message says what disagrees.
    pub fn description(self) -> &'static str {
        match self {
            Self::Dataflow => "a dataflow endpoint",
            Self::Agent => "an AWS Ground Station Agent endpoint",
        }
    }

    /// Whether an endpoint of this kind can be attached only with the `securityDetails`
    /// triple, i.e. whether a request that omits it describes nothing the service can
    /// create.
    ///
    /// `EndpointDetails` leaves `securityDetails` optional (`smithy_query(query="shape",
    /// shape="EndpointDetails")`), and what it carries is "a list of subnets, a list of
    /// security groups and a role to connect streams to instances"
    /// (`smithy_query(query="doc", shape="SecurityDetails")`) — the elastic network
    /// interfaces GROUND STATION itself places to deliver a [`Self::Dataflow`]
    /// endpoint's stream. A [`Self::Agent`] endpoint's stream is carried by the agent the
    /// customer runs on their own instance, so its shape marks everything it needs
    /// `@required` of itself (`name`, `egressAddress`, `ingressAddress` —
    /// `smithy_query(query="shape", shape="AwsGroundStationAgentEndpoint")`) and the VPC
    /// placement does not apply to it; one is accepted with or without the member.
    pub fn requires_security_details(self) -> bool {
        match self {
            Self::Dataflow => true,
            Self::Agent => false,
        }
    }
}

/// The endpoint members `details` sets, by the modelled member name and the kind of
/// endpoint each one describes, in the order `EndpointDetails` declares them
/// (`smithy_query(query="shape", shape="EndpointDetails")`).
///
/// An `EndpointDetails` describes ONE endpoint, so exactly one of these members belongs
/// to a well-formed request; this is what lets `CreateDataflowEndpointGroup` refuse an
/// item that names none or several, and what the read path classifies a stored endpoint
/// with.
pub fn endpoint_members(details: &EndpointDetails) -> Vec<(&'static str, EndpointKind)> {
    let mut members = Vec::new();
    if details.endpoint.is_some() {
        members.push(("endpoint", EndpointKind::Dataflow));
    }
    if details.aws_ground_station_agent_endpoint.is_some() {
        members.push(("awsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    if details.uplink_aws_ground_station_agent_endpoint.is_some() {
        members.push(("uplinkAwsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    if details.downlink_aws_ground_station_agent_endpoint.is_some() {
        members.push(("downlinkAwsGroundStationAgentEndpoint", EndpointKind::Agent));
    }
    members
}

/// The NAME of the endpoint `details` describes, whichever member describes it.
///
/// Every endpoint member of `EndpointDetails` carries a name of its own (`name` on
/// `DataflowEndpoint` and on each of the three agent-endpoint shapes —
/// `smithy_query(query="shape", shape="EndpointDetails")`), and that name is what a
/// `dataflowEndpointConfig` references an endpoint of this account by
/// (`dataflowEndpointName`), so one reader answers it for all four.
pub fn endpoint_name(details: &EndpointDetails) -> Option<&str> {
    if let Some(endpoint) = details.endpoint.as_ref() {
        return endpoint.name.as_deref();
    }
    if let Some(endpoint) = details.aws_ground_station_agent_endpoint.as_ref() {
        return Some(endpoint.name.as_str());
    }
    if let Some(endpoint) = details.uplink_aws_ground_station_agent_endpoint.as_ref() {
        return Some(endpoint.name.as_str());
    }
    details
        .downlink_aws_ground_station_agent_endpoint
        .as_ref()
        .map(|endpoint| endpoint.name.as_str())
}

/// Refuse an endpoint of a group that carries no name, or one an endpoint already accepted
/// into the SAME group took.
///
/// "The `name` field in each endpoint is used in your mission profile
/// `DataflowEndpointConfig` to specify which endpoints to use during a contact. When a
/// contact uses multiple `DataflowEndpointConfig` objects, each `Config` must match a
/// `DataflowEndpoint` in the same group" (`smithy_query(query="operation",
/// operation="CreateDataflowEndpointGroup")`). That match is by name, so an endpoint
/// carrying none, and two endpoints of one group carrying the same one, describe nothing a
/// `dataflowEndpointConfig` can single out (see
/// [`ComAmazonawsGroundstationGroundstationState::dataflow_endpoint`]).
///
/// `path` is how a refusal spells this item (`endpointDetails[0]`, `endpoints[0]`) and
/// `member` the endpoint member that describes it, so the refusal names the exact member at
/// fault; `taken` maps each name already accepted to the `path` of the item that took it,
/// and this call adds the accepted name to it. Written here rather than in a handler
/// because every create of a dataflow endpoint group holds its endpoints to the same rule,
/// whichever member its request carries them in.
pub fn claim_endpoint_name<'a>(
    path: &str,
    member: &str,
    details: &'a EndpointDetails,
    taken: &mut HashMap<&'a str, String>,
) -> Result<(), InvalidParameterException> {
    let Some(name) = endpoint_name(details) else {
        return Err(invalid_parameter(
            &format!("{path}.{member}.name"),
            format!(
                "{path} carries no endpoint name; a mission profile's dataflowEndpointConfig \
                 selects an endpoint of a group by its name, so every endpoint in a dataflow \
                 endpoint group must have one"
            ),
        ));
    };
    if let Some(first) = taken.insert(name, path.to_string()) {
        return Err(invalid_parameter(
            &format!("{path}.{member}.name"),
            format!(
                "{path} and {first} are both named {name:?}; a mission profile's \
                 dataflowEndpointConfig selects an endpoint of a group by its name, so two \
                 endpoints of one dataflow endpoint group cannot share one"
            ),
        ));
    }
    Ok(())
}

/// The ports a socket address of an endpoint can name.
///
/// A dataflow endpoint's address is the TCP/UDP socket Ground Station connects a contact's
/// stream to, so the only values it can carry are the transport's own port numbers: 1
/// through 65535, port 0 being the "any port" placeholder a listener asks the kernel to
/// choose and never an address a stream can be sent to. `SocketAddress$port` and the two
/// bounds of `IntegerRange` are plain modelled `Integer`s with no `range` of their own
/// (`smithy_query(query="shape", shape="SocketAddress")`, `smithy_query(query="shape",
/// shape="IntegerRange")`), so nothing upstream of a handler bounds them.
pub const PORT_MIN: i32 = 1;
/// See [`PORT_MIN`].
pub const PORT_MAX: i32 = 65535;

/// The transmission units a dataflow endpoint's MTU can be sized in bytes.
///
/// Every `mtu` of this service's endpoint shapes is the "Maximum transmission unit (MTU)
/// size in bytes of a dataflow endpoint", one documented member of one documented range:
/// the model states that range as 1400..1500 on `DataflowEndpoint$mtu` and on
/// `RangedConnectionDetails$mtu` (`smithy_query(query="shape", shape="DataflowEndpoint")`,
/// `smithy_query(query="shape", shape="RangedConnectionDetails")`), so a value outside it
/// is not a size the service will frame a stream at. `ConnectionDetails$mtu` — the egress
/// half of every agent connection pair — carries the same sentence and no `range` of its
/// own (`smithy_query(query="shape", shape="ConnectionDetails")`), so nothing upstream of
/// a handler bounds it and the handler holds it to the same bound as its siblings.
pub const MTU_MIN: i32 = 1400;
/// See [`MTU_MIN`].
pub const MTU_MAX: i32 = 1500;

/// Refuse an endpoint any of whose addresses is not an address a stream can be sent to: a
/// host part that is not the IPv4 address its shape says it is, a port no socket can be
/// addressed by, or an MTU outside the bytes a dataflow endpoint's MTU can take.
///
/// Walks every address-, port- and mtu-valued member reachable from one `EndpointDetails`,
/// whichever endpoint member describes it: a `DataflowEndpoint`'s own `address`, an agent
/// endpoint's `egressAddress`/`ingressAddress`, and the connection pair an uplink or
/// downlink agent endpoint's `dataflowDetails` carries (`smithy_query(query="closure",
/// operation="CreateDataflowEndpointGroup")`). `path` is how a refusal spells the item, and
/// each member is appended to it, so the message names the exact address at fault.
///
/// Written here rather than in a handler because every create of a dataflow endpoint group
/// holds its endpoints' addresses to the same rule.
pub fn check_endpoint_addresses(
    path: &str,
    details: &EndpointDetails,
) -> Result<(), InvalidParameterException> {
    if let Some(endpoint) = details.endpoint.as_ref() {
        if let Some(address) = endpoint.address.as_ref() {
            check_port(&format!("{path}.endpoint.address.port"), address.port)?;
        }
    }
    if let Some(agent) = details.aws_ground_station_agent_endpoint.as_ref() {
        let agent_path = format!("{path}.awsGroundStationAgentEndpoint");
        check_connection(&format!("{agent_path}.egressAddress"), &agent.egress_address)?;
        check_ranged_connection(
            &format!("{agent_path}.ingressAddress"),
            &agent.ingress_address,
        )?;
    }
    if let Some(uplink) = details.uplink_aws_ground_station_agent_endpoint.as_ref() {
        let UplinkDataflowDetails::agentConnectionDetails(connection) = &uplink.dataflow_details;
        let connection_path = format!(
            "{path}.uplinkAwsGroundStationAgentEndpoint.dataflowDetails.agentConnectionDetails"
        );
        check_connection(
            &format!("{connection_path}.ingressAddressAndPort"),
            &connection.ingress_address_and_port,
        )?;
        check_ranged_connection(
            &format!("{connection_path}.agentIpAndPortAddress"),
            &connection.agent_ip_and_port_address,
        )?;
    }
    if let Some(downlink) = details.downlink_aws_ground_station_agent_endpoint.as_ref() {
        let DownlinkDataflowDetails::agentConnectionDetails(connection) = &downlink.dataflow_details;
        let connection_path = format!(
            "{path}.downlinkAwsGroundStationAgentEndpoint.dataflowDetails.agentConnectionDetails"
        );
        check_connection(
            &format!("{connection_path}.egressAddressAndPort"),
            &connection.egress_address_and_port,
        )?;
        check_ranged_connection(
            &format!("{connection_path}.agentIpAndPortAddress"),
            &connection.agent_ip_and_port_address,
        )?;
    }
    Ok(())
}

/// Refuse a `ConnectionDetails` whose socket address names an unusable port, or whose
/// `mtu` is sized outside [`MTU_MIN`]..=[`MTU_MAX`] — the bound nothing upstream of a
/// handler applies to this half of a connection pair.
fn check_connection(
    path: &str,
    connection: &ConnectionDetails,
) -> Result<(), InvalidParameterException> {
    check_port(
        &format!("{path}.socketAddress.port"),
        connection.socket_address.port,
    )?;
    check_mtu(&format!("{path}.mtu"), connection.mtu)
}

/// Refuse a `RangedConnectionDetails` whose socket address is not an address at all: a
/// `name` that is not an IPv4 address, a port-range bound outside the ports a socket can be
/// addressed by, or a range that runs backwards and so holds no port at all.
fn check_ranged_connection(
    path: &str,
    connection: &RangedConnectionDetails,
) -> Result<(), InvalidParameterException> {
    check_ipv4_address(
        &format!("{path}.socketAddress.name"),
        &connection.socket_address.name,
    )?;
    let range_path = format!("{path}.socketAddress.portRange");
    let range = &connection.socket_address.port_range;
    check_port(&format!("{range_path}.minimum"), range.minimum)?;
    check_port(&format!("{range_path}.maximum"), range.maximum)?;
    if range.minimum > range.maximum {
        return Err(invalid_parameter(
            &range_path,
            format!(
                "{range_path} has minimum {} above maximum {}; a port range's minimum must \
                 not exceed its maximum",
                range.minimum, range.maximum
            ),
        ));
    }
    Ok(())
}

/// Refuse one value held to the model's `IpV4Address` that is not an IPv4 address, naming
/// where it was carried.
///
/// `RangedSocketAddress$name` is an `IpV4Address` (`smithy_query(query="shape",
/// shape="RangedSocketAddress")`), and that shape's pattern only holds a value to four
/// dot-separated runs of up to three digits, so a value whose octets run past 255 clears
/// the boundary validator and is still not an address a contact's stream can be carried
/// over. The sibling member of the same socket address — `SocketAddress$name` — is a plain
/// modelled `String` ("Name of a socket address"), not an `IpV4Address`, so a name carried
/// there is held to no address syntax.
fn check_ipv4_address(path: &str, value: &str) -> Result<(), InvalidParameterException> {
    if is_ipv4_address(value) {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        format!(
            "{value} is not an IPv4 address; each of the four octets of an IPv4 address is \
             between 0 and 255"
        ),
    ))
}

/// Whether `value` is a dotted-quad IPv4 address.
///
/// The model's `IpV4Address` pattern already held the value to four dot-separated runs of
/// one to three digits, so what is left is whether those digits name an address at all,
/// which `std::net::Ipv4Addr`'s own parser answers. Held here because every member this
/// service types as an `IpV4Address` — an agent's discovery addresses and the ingress
/// socket address of an agent-carried dataflow endpoint — is parsed the same way.
pub fn is_ipv4_address(value: &str) -> bool {
    value.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// Refuse one port value outside [`PORT_MIN`]..=[`PORT_MAX`], naming where it was carried.
fn check_port(path: &str, port: i32) -> Result<(), InvalidParameterException> {
    if (PORT_MIN..=PORT_MAX).contains(&port) {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        format!(
            "{path} is {port}; a dataflow endpoint's port must be between {PORT_MIN} and \
             {PORT_MAX}"
        ),
    ))
}

/// Refuse an MTU sized outside [`MTU_MIN`]..=[`MTU_MAX`], naming where it was carried. An
/// absent `mtu` asks for no particular size and is accepted; the member is optional.
fn check_mtu(path: &str, mtu: Option<i32>) -> Result<(), InvalidParameterException> {
    let Some(mtu) = mtu else {
        return Ok(());
    };
    if (MTU_MIN..=MTU_MAX).contains(&mtu) {
        return Ok(());
    }
    Err(invalid_parameter(
        path,
        format!("Value '{mtu}' at '{path}' is outside the valid range of {MTU_MIN} to {MTU_MAX}."),
    ))
}

/// The `EndpointStatus` a dataflow endpoint carries once the service has attached it,
/// which is how `CreateDataflowEndpointGroup` hands it back (`smithy_query(query="shape",
/// shape="EndpointStatus")` — `created`, `creating`, `deleted`, `deleting`, `failed`; the
/// whole list is `crate::ops::types::ENDPOINT_STATUS_VALUES`).
pub const ENDPOINT_CREATED: &str = "created";

/// The `CapabilityHealth` values an endpoint's `healthStatus` reports
/// (`smithy_query(query="shape", shape="CapabilityHealth")`).
pub const HEALTH_HEALTHY: &str = "HEALTHY";
/// See [`HEALTH_HEALTHY`].
pub const HEALTH_UNHEALTHY: &str = "UNHEALTHY";

/// The `CapabilityHealthReason` an endpoint still being attached reports, the one a
/// finished endpoint reports, and the one an AWS Ground Station Agent endpoint no agent
/// has registered against reports (`smithy_query(query="shape",
/// shape="CapabilityHealthReason")` — `NO_REGISTERED_AGENT`, `INVALID_IP_OWNERSHIP`,
/// `NOT_AUTHORIZED_TO_CREATE_SLR`, `UNVERIFIED_IP_OWNERSHIP`, `INITIALIZING_DATAPLANE`,
/// `DATAPLANE_FAILURE`, `HEALTHY`).
pub const REASON_INITIALIZING_DATAPLANE: &str = "INITIALIZING_DATAPLANE";
/// See [`REASON_INITIALIZING_DATAPLANE`].
pub const REASON_HEALTHY: &str = "HEALTHY";
/// See [`REASON_INITIALIZING_DATAPLANE`].
pub const REASON_NO_REGISTERED_AGENT: &str = "NO_REGISTERED_AGENT";
/// See [`REASON_INITIALIZING_DATAPLANE`].
pub const REASON_DATAPLANE_FAILURE: &str = "DATAPLANE_FAILURE";

/// The `AgentStatus` an AWS Ground Station Agent endpoint carries while no agent has
/// registered itself for that endpoint (`smithy_query(query="shape",
/// shape="AgentStatus")` — `SUCCESS`, `FAILED`, `ACTIVE`, `INACTIVE`).
///
/// It is also the aggregate status a freshly registered [`Agent`] carries: the status of
/// an agent is what the agent itself reports through `UpdateAgentStatus`, so until it has
/// reported one the service holds it inactive rather than claiming a status nothing
/// observed.
pub const AGENT_INACTIVE: &str = "INACTIVE";

/// The other `AgentStatus` values an agent reports of itself through `UpdateAgentStatus`
/// (`smithy_query(query="shape", shape="AgentStatus")` — `SUCCESS`, `FAILED`, `ACTIVE`,
/// `INACTIVE`): the two that say its components are running, and the one that says they
/// failed.
pub const AGENT_ACTIVE: &str = "ACTIVE";
/// See [`AGENT_ACTIVE`].
pub const AGENT_SUCCESS: &str = "SUCCESS";
/// See [`AGENT_ACTIVE`].
pub const AGENT_FAILED: &str = "FAILED";

/// The `AuditResults` an AWS Ground Station Agent endpoint reports once an agent carries
/// it (`smithy_query(query="shape", shape="AuditResults")` — `HEALTHY`, `UNHEALTHY`).
pub const AUDIT_HEALTHY: &str = "HEALTHY";
/// See [`AUDIT_HEALTHY`].
pub const AUDIT_UNHEALTHY: &str = "UNHEALTHY";

/// Stamp on `details` the members an AWS Ground Station Agent endpoint's own AGENT decides
/// rather than the request: `agentStatus`, `auditResults`, `healthStatus` and
/// `healthReasons`. `carrier` is the registered agent that carries the endpoint's stream,
/// `None` when no registered agent carries it.
///
/// `details` that describes no agent endpoint is left exactly as it is, so one reader can
/// walk every endpoint of a group whatever kind each one is.
///
/// An agent endpoint's stream is carried by an agent the customer registers with
/// `RegisterAgent` ([`ComAmazonawsGroundstationGroundstationState::agent_for_capability`]),
/// and the endpoint reports the agent that carries it:
///
/// * until one has registered for it there is no agent to report, so the endpoint reports
///   `NO_REGISTERED_AGENT`, an `INACTIVE` agent and no audit result, none having run;
/// * once one has, `agentStatus` is "the status of AgentEndpoint"
///   (`smithy_query(query="doc", shape="AwsGroundStationAgentEndpoint",
///   member="agentStatus")`), which is the status that agent last reported of itself
///   through `UpdateAgentStatus`, and the endpoint's health is what that status implies: an
///   agent whose components are running is the endpoint's stream flowing, an agent that
///   reported a failure is a failed dataplane, and an agent that has reported nothing yet
///   is a dataplane still coming up — the same reason this service reports for a dataflow
///   endpoint it has not finished attaching.
pub fn apply_agent_endpoint_health(details: &mut EndpointDetails, carrier: Option<&Agent>) {
    if !endpoint_members(details)
        .iter()
        .any(|(_, kind)| *kind == EndpointKind::Agent)
    {
        return;
    }

    let (agent_status, audit_results, health_status, health_reason) = match carrier {
        None => (
            AGENT_INACTIVE,
            None,
            HEALTH_UNHEALTHY,
            REASON_NO_REGISTERED_AGENT,
        ),
        Some(agent) => match agent.aggregate_status.status.as_str() {
            reported @ (AGENT_ACTIVE | AGENT_SUCCESS) => (
                reported,
                Some(AUDIT_HEALTHY),
                HEALTH_HEALTHY,
                REASON_HEALTHY,
            ),
            reported @ AGENT_FAILED => (
                reported,
                Some(AUDIT_UNHEALTHY),
                HEALTH_UNHEALTHY,
                REASON_DATAPLANE_FAILURE,
            ),
            reported => (
                reported,
                None,
                HEALTH_UNHEALTHY,
                REASON_INITIALIZING_DATAPLANE,
            ),
        },
    };

    // The three agent members `EndpointDetails` declares each carry their own `agentStatus`
    // and `auditResults` (`smithy_query(query="shape", shape="EndpointDetails")`), and an
    // item sets exactly one of them ([`endpoint_members`]), so all three are stamped from
    // the one answer rather than each kind deciding its own.
    let agent_status = Some(agent_status.to_string());
    let audit_results = audit_results.map(str::to_string);
    if let Some(endpoint) = details.aws_ground_station_agent_endpoint.as_mut() {
        endpoint.agent_status = agent_status.clone();
        endpoint.audit_results = audit_results.clone();
    }
    if let Some(endpoint) = details.uplink_aws_ground_station_agent_endpoint.as_mut() {
        endpoint.agent_status = agent_status.clone();
        endpoint.audit_results = audit_results.clone();
    }
    if let Some(endpoint) = details.downlink_aws_ground_station_agent_endpoint.as_mut() {
        endpoint.agent_status = agent_status;
        endpoint.audit_results = audit_results;
    }

    details.health_status = Some(health_status.to_string());
    details.health_reasons = Some(vec![health_reason.to_string()]);
}

/// A dataflow endpoint group as `CreateDataflowEndpointGroup` stored it.
#[derive(Debug, Clone)]
pub struct DataflowEndpointGroup {
    pub dataflow_endpoint_group_id: String,
    pub dataflow_endpoint_group_arn: String,
    /// The endpoints of the group, as the request described them plus the members the
    /// SERVICE owns: each endpoint's `status`, and the `healthStatus`/`healthReasons`
    /// the model documents as "ignored when calling `CreateDataflowEndpointGroup`"
    /// (`smithy_query(query="doc", shape="EndpointDetails", member="healthStatus")`).
    pub endpoint_details: Vec<EndpointDetails>,
    pub contact_pre_pass_duration_seconds: Option<i32>,
    pub contact_post_pass_duration_seconds: Option<i32>,
    pub tags: HashMap<String, String>,
}

/// An AWS Ground Station Agent as `RegisterAgent` registered it, with the status its
/// last `UpdateAgentStatus` report carried.
///
/// The model makes `Agent` a resource of this service with the lifecycle
/// `RegisterAgent` (create), `GetAgentConfiguration` (read) and `UpdateAgentStatus`
/// (update), identified by `agentId` and ARN-addressed as `agent/{agentId}`
/// (`smithy_query(query="shape", shape="Agent")`), so one record per agent carries
/// everything those three operations read and write.
#[derive(Debug, Clone)]
pub struct Agent {
    pub agent_id: String,
    pub agent_arn: String,
    /// What the agent said about itself when it registered: its version, the EC2
    /// instance it runs on and the versions of its components.
    pub agent_details: AgentDetails,
    /// The addresses and capability ARNs the agent registered with — "data for
    /// associating an agent with the capabilities it is managing".
    pub discovery_data: DiscoveryData,
    pub tags: HashMap<String, String>,
    /// The aggregate status the agent last reported, [`AGENT_INACTIVE`] until it has
    /// reported one.
    pub aggregate_status: AggregateStatus,
    /// The per-component statuses the agent last reported: the whole list of the latest
    /// report, since each report carries the agent's components as they stand.
    pub component_statuses: Vec<ComponentStatusData>,
    /// The agent task the last report was about; `None` before the first report.
    pub task_id: Option<String>,
    /// The upload URLs this agent holds, keyed by the agent task each takes the response
    /// log of (`GetAgentTaskResponseUrl`).
    ///
    /// No modelled read reports them: they are held so that the URL handed out for a task
    /// is handed out again while it is still valid, so an agent uploading with one is not
    /// handed a second. The map is bounded by the validity of its own entries — each is
    /// released, and the credential behind it withdrawn, once the clock has run out on it
    /// ([`Agent::take_expired_log_uploads`], called by every read of an upload URL) — so it
    /// holds only what is still signed for.
    pub log_uploads: HashMap<String, AgentLogUpload>,
    /// The tasking document `GetAgentConfiguration` hands the agent.
    ///
    /// `None` until the service has tasked the agent: the document describes the
    /// contact the agent is to carry, and no operation this model declares produces one
    /// (`smithy_query(query="digest")` carries no operation that tasks an agent), so an
    /// agent that nothing has tasked is reported with no document rather than with an
    /// invented one.
    pub tasking_document: Option<String>,
    /// When the agent registered.
    pub registered: DateTime,
    /// When the agent last reported its status, equal to [`Agent::registered`] until it
    /// has reported once.
    pub last_updated: DateTime,
}

impl Agent {
    /// Drop every upload URL of this agent whose validity the clock has run out on, and
    /// answer the credentials behind them, for [`withdraw_credential`].
    ///
    /// This materialises the expiry that was due at this instant whether or not anybody
    /// called: a presigned URL stops being accepted when its validity runs out, and the
    /// credential that signed it can then sign nothing. A URL that is STILL signed for is
    /// left exactly as it is, whichever task is being read about, so a caller holding one
    /// goes on uploading with it.
    pub fn take_expired_log_uploads(&mut self, now: SystemTime) -> Vec<String> {
        let mut expired = Vec::new();
        self.log_uploads.retain(|_, upload| {
            let live = upload.is_live(now);
            if !live {
                expired.push(upload.credential_id.clone());
            }
            live
        });
        expired
    }
}

/// What this service answers when an `{agentId}` label names no registered agent.
///
/// Shared by the two operations that address an agent by that label —
/// `GetAgentConfiguration` and `UpdateAgentStatus` — so a read and an update of an
/// agent that is not there answer one text.
pub fn agent_not_found_message(agent_id: &str) -> String {
    format!("No agent found with id {agent_id}")
}

/// What an operation addressed at a resource by its `resourceArn` answers when that ARN
/// names no resource of this account.
///
/// FIXED text: the whole message, carrying NOTHING from the request. The ARN is taken only
/// to decide that nothing matched it, which the caller already knows it sent, so a request
/// about a resource that was deleted, about one this service never minted, and about
/// another service's resource are all answered with this one sentence. It is the wording
/// the model gives the shape itself — "Resource was not found."
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`) — without the
/// trailing stop, as the recorded refusal of a tagging call against a deleted resource's
/// ARN states it.
///
/// Shared by every tag operation, so a read, an assignment and a removal of the tags of a
/// resource that is not there answer one text.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error CODE this service's ARN-addressed tagging operations name a resource they
/// cannot find by.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` each declare the modelled
/// `ResourceNotFoundException` (`smithy_query(query="operation", operation="TagResource")`),
/// and that shape's payload and status are what the answer carries — but the code this
/// service's tagging plane puts on the wire (`x-amzn-errortype`, and the body's `__type`)
/// is the shorter `NotFoundException`, which the model declares as no shape at all. The
/// code is what an SDK resolves an error from and what a client branches on, so it is the
/// recorded one. It belongs to the ARN-addressed tagging operations only: every operation
/// of this service addressed by an id or a label answers the modelled code.
pub const TAGGING_NOT_FOUND_ERROR_CODE: &str = "NotFoundException";

/// The HTTP status of that refusal: the `@httpError` of the shape whose payload it carries,
/// `ResourceNotFoundException` (`smithy_query(query="operation",
/// operation="TagResource")`), so the code on the wire is the tagging plane's and the status
/// stays the model's.
pub const TAGGED_RESOURCE_NOT_FOUND_STATUS: u16 = 434;

/// The body of that refusal: the modelled shape, carrying the one fixed sentence
/// ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`]) and nothing else — the shape declares no member
/// beside `message`.
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TAGGED_RESOURCE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The error channel of this service's ARN-addressed tagging operations: either one of the
/// operation's own modelled errors, rendered exactly as the model declares it, or the
/// tagging plane's own not-found refusal.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` are one plane, so they cannot
/// word one condition three ways: an ARN resolving to nothing is
/// [`TAGGING_NOT_FOUND_ERROR_CODE`] carrying `ResourceNotFoundException`'s payload at
/// [`TAGGED_RESOURCE_NOT_FOUND_STATUS`], whichever of the three was called. Generic over the
/// operation's modelled enum, so the mapping is written once here instead of once per
/// operation — and so every other refusal those handlers make (an invalid tag map, an
/// unavailable store) keeps the code, the status and the body its own model gives it.
#[derive(Debug)]
pub enum TaggingError<E> {
    /// One of the operation's modelled errors.
    Modeled(E),
    /// "That ARN names no resource of this account" — [`tagged_resource_not_found`]'s body
    /// under the tagging plane's own code.
    NotFound(ResourceNotFoundException),
}

impl<E> TaggingError<E> {
    /// The not-found refusal for a `resourceArn` naming no resource this service holds.
    pub fn not_found() -> Self {
        Self::NotFound(tagged_resource_not_found())
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for TaggingError<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Modeled(e) => write!(f, "{e}"),
            Self::NotFound(_) => {
                write!(
                    f,
                    "{TAGGING_NOT_FOUND_ERROR_CODE}: {TAGGED_RESOURCE_NOT_FOUND_MESSAGE}"
                )
            }
        }
    }
}

/// `source()` forwards to the modelled error, so its own `source()` survives the wrapper.
impl<E: ::std::error::Error + 'static> ::std::error::Error for TaggingError<E> {
    fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Modeled(e) => e.source(),
            Self::NotFound(_) => None,
        }
    }
}

/// Every method delegates a modelled failure to the modelled error, so wrapping the channel
/// changes NOTHING about how those are rendered; only the plane's own refusal is answered
/// here, and only its CODE departs from the shape it carries.
impl<E: ::smithy_server_runtime::OperationError> ::smithy_server_runtime::OperationError
    for TaggingError<E>
{
    fn as_serializable(&self) -> &dyn ::aws_smithy_schema::serde::SerializableStruct {
        match self {
            Self::Modeled(e) => e.as_serializable(),
            Self::NotFound(e) => e,
        }
    }

    fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Modeled(e) => e.variant_schema(),
            Self::NotFound(_) => &crate::ops::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::Modeled(e) => e.shape_name(),
            Self::NotFound(_) => TAGGING_NOT_FOUND_ERROR_CODE,
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Modeled(e) => e.http_status(),
            Self::NotFound(_) => TAGGED_RESOURCE_NOT_FOUND_STATUS,
        }
    }

    fn query_error_code(&self) -> Option<&'static str> {
        match self {
            // restJson1 is this service's only protocol, and the shape carries no
            // `awsQueryError` trait to honour in any case.
            Self::Modeled(e) => e.query_error_code(),
            Self::NotFound(_) => None,
        }
    }
}

/// The `ReservationType` of the block of antenna time one of this account's contacts
/// holds at a ground station.
///
/// This and [`RESERVATION_TYPE_MAINTENANCE`] are the model's own enum values
/// (`smithy_query(query="shape", shape="ReservationType")` — `MAINTENANCE` and
/// `CONTACT`), which is both what `ListGroundStationReservations` reports as an item's
/// `reservationType` and what its `reservationTypes` filters on.
pub const RESERVATION_TYPE_CONTACT: &str = "CONTACT";

/// The `ReservationType` of a block of antenna time AWS holds for its own maintenance of
/// a ground station's antenna. See [`RESERVATION_TYPE_CONTACT`].
pub const RESERVATION_TYPE_MAINTENANCE: &str = "MAINTENANCE";

/// The `MaintenanceType` values a maintenance reservation can carry: the model's own enum
/// (`smithy_query(query="shape", shape="MaintenanceType")` — `PLANNED` and `UNPLANNED`).
pub const MAINTENANCE_TYPES: &[&str] = &["PLANNED", "UNPLANNED"];

/// The name of the `number`th antenna of a ground station, counted from one.
///
/// CHOSEN spelling: the model constrains an `AntennaName` only to 4..256 characters of
/// `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="AntennaName")`), documents it
/// as no more than "Name of the antenna", and no request of this service names one — AWS
/// places a pass on an antenna itself. So the name is this emulator's, spelled once here
/// so the antenna a reservation reports and the antenna [`MAINTENANCE_ENV`] takes out of
/// service read the same way. The ground station is reported beside it (a
/// `GroundStationReservationListItem` carries both), so the name does not repeat it.
pub fn antenna_name(number: usize) -> String {
    format!("Antenna {number}")
}

/// Whether two windows, each `(start, end)` in epoch seconds, share any instant.
///
/// CLOSED at both ends, which is how a listing's window filter asks the question — a
/// reservation is in the window when its own window overlaps it — so one that starts
/// exactly as the window ends is in it. The SAME reading serves the antenna placement, so
/// two passes that merely touch are placed on different antennas; the two call sites
/// ([`ComAmazonawsGroundstationGroundstationState::antennas_held`] and the listings) read
/// one rule, and a half-open reading would have to replace it in both rather than in one.
pub fn windows_overlap(first: (f64, f64), second: (f64, f64)) -> bool {
    first.1 >= second.0 && first.0 <= second.1
}

/// A ground station antenna AWS has taken out of service for a span, as
/// [`MAINTENANCE_ENV`] described it — the `MAINTENANCE` half of what
/// `ListGroundStationReservations` reports.
///
/// Nothing in THIS API creates one: the model declares no operation that schedules
/// maintenance and none that reports it other than `ListGroundStationReservations` itself
/// (`smithy_query(query="digest")`), because AWS maintains its own antennas. So a
/// maintenance window is minted when this service's store is created, from the operator
/// setting, and the API reports it — and, like a contact's reservation, it holds the
/// antenna, so no pass is placed on an antenna while it is under maintenance.
#[derive(Debug, Clone)]
pub struct MaintenanceReservation {
    /// The ground station whose antenna is out of service, a `GroundStationName`.
    pub ground_station: String,
    /// The antenna that is out of service, an `AntennaName` ([`antenna_name`]).
    pub antenna_name: String,
    /// Whether this maintenance was planned, a `MaintenanceType` ([`MAINTENANCE_TYPES`]).
    pub maintenance_type: String,
    /// The first instant the antenna is out of service.
    pub start_time: DateTime,
    /// The instant it is back in service.
    pub end_time: DateTime,
}

/// A satellite this account is on-boarded to, as [`SATELLITES_ENV`] described it.
///
/// Nothing in THIS API creates one: the model declares no operation that onboards a
/// satellite — `ListSatellites` and `GetSatellite` only report them
/// (`smithy_query(query="digest")`) — because AWS onboards a spacecraft to an account
/// outside the API, once its licensing and configuration have been reviewed. So a
/// satellite record is minted when this service's store is created, from the operator
/// setting, and the API reports it.
#[derive(Debug, Clone)]
pub struct Satellite {
    /// The `Uuid` this onboarding is addressed by, minted by [`new_uuid`] when the store
    /// is created and stable for the life of that store.
    pub satellite_id: String,
    /// This satellite's ARN. Its REGION segment is empty: the modelled `satelliteArn`
    /// pattern makes the region optional
    /// (`^arn:aws:groundstation:([-a-z0-9]{1,50})?:[0-9]{12}:satellite/…`,
    /// `smithy_query(query="shape", shape="satelliteArn")`) where every other ARN of this
    /// service requires one, because an on-boarded satellite belongs to the account
    /// rather than to one region.
    pub satellite_arn: String,
    /// The spacecraft's NORAD catalogue number ([`NORAD_SATELLITE_ID_RANGE`]).
    pub norad_satellite_id: i32,
    /// The ground stations this satellite is on-boarded to, each a `GroundStationName`.
    pub ground_stations: Vec<String>,
    /// The ephemeris currently used to compute this satellite's trajectory, once one is
    /// known for it — what `GetSatellite` and `ListSatellites` report as
    /// `currentEphemeris`.
    ///
    /// Materialised by
    /// [`ComAmazonawsGroundstationGroundstationState::refresh_current_ephemerides`] from
    /// the ephemerides this account holds for the satellite: the one in force supersedes
    /// whatever default ephemeris the onboarding carried, and the default comes back when
    /// the account holds none in force. An onboarding carries none, because a default
    /// comes from outside this API (AWS computes one from Space Track) and this emulator
    /// has no such source, so a satellite knows no ephemeris until one is stored for it.
    pub current_ephemeris: Option<EphemerisMetaData>,
}

/// The range a `noradSatelliteID` may hold: `smithy_query(query="shape",
/// shape="noradSatelliteID")` — 0..339999, inclusive of both bounds.
pub const NORAD_SATELLITE_ID_RANGE: ::std::ops::RangeInclusive<i32> = 0..=339_999;

/// Whether `value` is inside [`NORAD_SATELLITE_ID_RANGE`], so the bound is stated once
/// and read the same way everywhere.
pub fn is_norad_satellite_id(value: i32) -> bool {
    NORAD_SATELLITE_ID_RANGE.contains(&value)
}

/// Most ground stations one satellite can be on-boarded to: the modelled maximum length
/// of `GroundStationIdList` (`smithy_query(query="shape", shape="GroundStationIdList")` —
/// len 0..500), which is what `GetSatellite`/`ListSatellites` report.
pub const MAX_GROUND_STATIONS_PER_SATELLITE: usize = 500;

/// Whether `byte` is inside the character set a `GroundStationName` and an `AntennaName`
/// are spelled from: `[ a-zA-Z0-9-._:=]`, which both shapes' patterns name
/// (`smithy_query(query="shape", shape="GroundStationName,AntennaName")`).
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'.' | b'_' | b':' | b'=')
}

/// Whether `name` is spellable as a `GroundStationName`: length 4..97 over the character
/// set `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape", shape="GroundStationName")`).
pub fn is_ground_station_name(name: &str) -> bool {
    (4..=97).contains(&name.len()) && name.bytes().all(is_name_byte)
}

/// Whether `name` is spellable as an `AntennaName`: length 4..256 over the same character
/// set as a `GroundStationName` (`smithy_query(query="shape", shape="AntennaName")`).
pub fn is_antenna_name(name: &str) -> bool {
    (4..=256).contains(&name.len()) && name.bytes().all(is_name_byte)
}

/// Most antennas one ground station can report: the modelled maximum length of
/// `AntennaList` (`smithy_query(query="shape", shape="AntennaList")` — len 0..100), which
/// is the list `ListAntennas` answers from.
pub const MAX_ANTENNAS_PER_GROUND_STATION: usize = 100;

/// A ground station serving this account, and the antennas at it.
///
/// Nothing in THIS API builds one: the model declares no operation that creates a ground
/// station or an antenna — the listings only report them (`smithy_query(query="digest")`)
/// — because AWS builds and operates the antennas itself and an account reaches whichever
/// ones it has been granted. So a ground station record is written when this service's
/// store is created, from the operator setting, and the API reports it.
#[derive(Debug, Clone)]
pub struct GroundStation {
    /// Its `GroundStationName`: both the `groundStationId` an operation addresses it by
    /// and the `groundStationName` a listing reports, which are ONE name in this API —
    /// the two members target the same shape (`smithy_query(query="shape",
    /// shape="GroundStationData")`).
    pub ground_station_name: String,
    /// The antennas at it, each an `AntennaName`, held in name order and bounded by
    /// [`MAX_ANTENNAS_PER_GROUND_STATION`]. A ground station whose antennas this account
    /// has not been told about has none here, which is a ground station with nothing to
    /// list rather than an unknown one.
    pub antennas: Vec<String>,
}

/// What this service answers when a `satelliteId` names no satellite this account is
/// on-boarded to.
///
/// Shared by every operation that takes one, so a create that references an unknown
/// satellite and a listing filtered on one answer the same text.
pub fn satellite_not_found_message(satellite_id: &str) -> String {
    format!("No satellite found with id {satellite_id}")
}

/// The `satelliteId` a `satelliteArn` addresses, or `None` when `arn` names no satellite
/// resource.
///
/// The modelled `satelliteArn` pattern ends in `satellite/<uuid>`
/// (`smithy_query(query="shape", shape="satelliteArn")`), and that uuid is the very key
/// [`ComAmazonawsGroundstationGroundstationState::satellites`] is keyed on — which is what
/// lets an ARN-valued satellite reference be reported by the id it names rather than
/// echoed back whole, exactly as a config reference is ([`config_arn_parts`]).
pub fn satellite_arn_id(arn: &str) -> Option<String> {
    let (_, satellite_id) = arn.split_once(":satellite/")?;
    (!satellite_id.is_empty()).then(|| satellite_id.to_string())
}

/// The message an operation that resolves a satellite BY ARN — the `satelliteArn` a
/// contact version is flown against — carries when that reference resolves to nothing.
///
/// It identifies the satellite the lookup looked FOR, by the id the ARN names
/// ([`satellite_arn_id`]), under the text this service already answers for a `satelliteId`
/// naming no onboarding of this account ([`satellite_not_found_message`]): the two ways a
/// request references a satellite then answer one text. An ARN that names no satellite id
/// has none to report, so the ARN is all there is to say — the shape
/// [`config_reference_not_found_message`] takes for the same case.
pub fn satellite_reference_not_found_message(arn: &str) -> String {
    match satellite_arn_id(arn) {
        Some(satellite_id) => satellite_not_found_message(&satellite_id),
        None => format!("Unable to find Satellite with ARN: {arn}"),
    }
}

/// Per-service state for com-amazonaws-groundstation-groundstation. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsGroundstationGroundstationState {
    /// Configs, keyed by the `(configType, configId)` pair `GetConfig` addresses.
    pub configs: HashMap<(String, String), Config>,
    /// Mission profiles, keyed by `missionProfileId`.
    pub mission_profiles: HashMap<String, MissionProfile>,
    /// Contacts, keyed by `contactId`.
    pub contacts: HashMap<String, Contact>,
    /// Dataflow endpoint groups, keyed by `dataflowEndpointGroupId`.
    pub dataflow_endpoint_groups: HashMap<String, DataflowEndpointGroup>,
    /// Registered AWS Ground Station Agents, keyed by `agentId`.
    pub agents: HashMap<String, Agent>,
    /// Ephemerides, keyed by `ephemerisId` — what `ListEphemerides` reports.
    pub ephemerides: HashMap<String, Ephemeris>,
    /// Every ARN this service minted, and what it points at.
    pub arn_index: HashMap<String, ResourceRef>,
    /// The uplink frequency bands, in MHz, this account has been approved to transmit
    /// on — the account-level authorisation `CreateConfig` checks an
    /// `antennaUplinkConfig`'s center frequency against.
    ///
    /// EMPTY unless [`APPROVED_UPLINK_BANDS_ENV`] grants a band, and that is this
    /// account's real authorisation rather than a gap in the emulator: AWS Ground
    /// Station radiates only on frequencies an account has been approved for, that
    /// approval is granted outside the API (by AWS, after spectrum coordination), and
    /// this service's model carries no operation that grants one and no response member
    /// that reports one (`smithy_query(query="digest")`). So an account starts with no
    /// uplink approval, exactly as a new AWS account does, and every
    /// `antennaUplinkConfig` is refused with [`UNAPPROVED_UPLINK_FREQUENCY`] until a
    /// band lands here — from the environment variable at start-up, or from the grant
    /// the day the API models one. It is a field rather than a constant because the
    /// authorisation is per account.
    pub approved_uplink_bands_mhz: Vec<(f64, f64)>,
    /// The reserved-minutes pricing plan this account holds — what `GetMinuteUsage`
    /// reports as `isReservedMinutesCustomer` and `totalReservedMinuteAllocation`, and
    /// what it measures `estimatedMinutesRemaining` against.
    ///
    /// EMPTY unless [`RESERVED_MINUTES_ENV`] grants an allocation, and that is this
    /// account's real pricing plan rather than a gap in the emulator: an account signs up
    /// for reserved minutes outside this API (AWS sells the commitment; the model carries
    /// no operation that signs an account up and none that reports the signup other than
    /// `GetMinuteUsage` itself — `smithy_query(query="digest")`). So an account starts on
    /// on-demand pricing, exactly as a new AWS account does, and `GetMinuteUsage` answers
    /// `isReservedMinutesCustomer: false` with a zero allocation until a plan lands here.
    pub reserved_minutes: ReservedMinutesPlan,
    /// The satellites this account is on-boarded to, keyed by `satelliteId` — what
    /// `ListSatellites` reports.
    ///
    /// EMPTY unless [`SATELLITES_ENV`] onboards one, and that is this account's real
    /// onboarding rather than a gap in the emulator: AWS onboards a spacecraft to an
    /// account outside this API (after reviewing its licensing and configuration), and
    /// the model carries no operation that onboards one and no response member that
    /// reports one other than the two satellite readers themselves
    /// (`smithy_query(query="digest")`). So an account starts on-boarded to no satellite,
    /// exactly as a new AWS account does, and `ListSatellites` answers an empty list until
    /// an onboarding lands here.
    pub satellites: HashMap<String, Satellite>,
    /// The ground station antennas AWS has taken out of service for maintenance — the
    /// `MAINTENANCE` reservations `ListGroundStationReservations` reports, and blocks of
    /// antenna time no pass is placed on.
    ///
    /// EMPTY unless [`MAINTENANCE_ENV`] schedules one, and that is the real state of the
    /// ground station network rather than a gap in the emulator: AWS maintains its own
    /// antennas, and the model carries no operation that schedules maintenance and none
    /// that reports it other than `ListGroundStationReservations` itself
    /// (`smithy_query(query="digest")`). So a ground station starts with nothing out of
    /// service and the listing reports only this account's own contact reservations until
    /// a maintenance window lands here.
    pub maintenance_reservations: Vec<MaintenanceReservation>,
    /// The ground stations this account is served by, keyed by their `GroundStationName`,
    /// and the antennas at each — what `ListAntennas` reports.
    ///
    /// EMPTY unless [`GROUND_STATIONS_ENV`] describes one, and that is this account's real
    /// reach rather than a gap in the emulator: AWS builds and operates the antennas and
    /// grants an account access to them outside this API, and the model carries no
    /// operation that builds a ground station or an antenna and no response member that
    /// reports one other than the listings themselves (`smithy_query(query="digest")`). So
    /// an account starts knowing only the ground stations its satellite onboardings name
    /// ([`Self::knows_ground_station`]), and none of them lists an antenna until an
    /// inventory lands here.
    pub ground_stations: HashMap<String, GroundStation>,
}

/// The reserved-minutes pricing plan an account holds: which calendar months it covers
/// and how many minutes it allocates in each.
///
/// The allocation is PER MONTH because `GetMinuteUsage` reports both the signup and the
/// allocation "specific to the month being requested"
/// (`smithy_query(query="operation", operation="GetMinuteUsage")`), so a plan that
/// started, changed size or lapsed partway through a year answers differently month by
/// month.
#[derive(Debug, Clone, Default)]
pub struct ReservedMinutesPlan {
    /// Minutes allocated in every month the plan covers, when it is a standing monthly
    /// commitment; `None` when the account has no standing commitment at all.
    pub monthly_allocation: Option<i32>,
    /// The allocation of one calendar month, keyed `(year, month)`. An entry here
    /// OVERRIDES [`monthly_allocation`](Self::monthly_allocation) for that month, which
    /// is how a plan of a different size — or one covering a single month — is expressed.
    pub by_month: HashMap<(i32, i32), i32>,
}

impl ReservedMinutesPlan {
    /// The minutes this plan allocates for `(year, month)`, or `None` when the plan does
    /// not cover that month — which is exactly what makes the account a reserved-minutes
    /// customer *for the month being requested*.
    pub fn allocation_for(&self, year: i32, month: i32) -> Option<i32> {
        self.by_month
            .get(&(year, month))
            .copied()
            .or(self.monthly_allocation)
    }
}

/// The environment variable an operator uses to put THIS emulated account on the reserved
/// minutes pricing plan, since the API models no operation that signs one up: a
/// comma-separated list of entries, each either a bare minute count (a standing monthly
/// commitment, `GROUNDSTATION_RESERVED_MINUTES="500"`) or `<year>-<month>=<minutes>` for
/// one calendar month (`"500,2026-02=750"` commits 500 minutes a month and 750 in
/// February 2026). Read once when this service's store is created. Unset means on-demand
/// pricing, which is what a new AWS account has.
pub const RESERVED_MINUTES_ENV: &str = "GROUNDSTATION_RESERVED_MINUTES";

/// The plan `setting` grants. An entry that is not a non-negative minute count, optionally
/// keyed by a `<year>-<month>` inside the calendar's 1–12 months, is skipped rather than
/// guessed at, so a typo commits nothing instead of committing something unintended.
pub fn parse_reserved_minutes(setting: &str) -> ReservedMinutesPlan {
    let mut plan = ReservedMinutesPlan::default();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            // A bare count: the standing monthly commitment.
            None => {
                if let Ok(minutes) = entry.parse::<i32>() {
                    if minutes >= 0 {
                        plan.monthly_allocation = Some(minutes);
                    }
                }
            }
            // `<year>-<month>=<minutes>`: one month's own allocation.
            Some((month_key, minutes)) => {
                let Some((year, month)) = month_key.trim().split_once('-') else {
                    continue;
                };
                let (Ok(year), Ok(month), Ok(minutes)) = (
                    year.trim().parse::<i32>(),
                    month.trim().parse::<i32>(),
                    minutes.trim().parse::<i32>(),
                ) else {
                    continue;
                };
                if (1..=12).contains(&month) && minutes >= 0 {
                    plan.by_month.insert((year, month), minutes);
                }
            }
        }
    }
    plan
}

/// The environment variable an operator uses to grant THIS emulated account its uplink
/// approval, since the API models no operation that grants one: a comma-separated list
/// of `<low>-<high>` bands in MHz, read once when this service's store is created
/// (`GROUNDSTATION_APPROVED_UPLINK_BANDS_MHZ="2025-2120"` approves the whole band the
/// service operates). Unset means no approval, which is what a new AWS account has and
/// what AWS answers for.
pub const APPROVED_UPLINK_BANDS_ENV: &str = "GROUNDSTATION_APPROVED_UPLINK_BANDS_MHZ";

/// The bands `setting` grants: `<low>-<high>` pairs in MHz, comma-separated. An entry
/// that is not a pair of numbers low-to-high is skipped rather than guessed at, so a
/// typo grants nothing instead of granting something unintended.
pub fn parse_approved_uplink_bands(setting: &str) -> Vec<(f64, f64)> {
    setting
        .split(',')
        .filter_map(|entry| {
            let (low, high) = entry.trim().split_once('-')?;
            let low: f64 = low.trim().parse().ok()?;
            let high: f64 = high.trim().parse().ok()?;
            (low <= high).then_some((low, high))
        })
        .collect()
}

/// The environment variable an operator uses to onboard a satellite to THIS emulated
/// account, since the API models no operation that onboards one: a comma-separated list of
/// entries, each either a bare NORAD catalogue number (`25994`) or
/// `<norad>=<ground station>+<ground station>…` naming the ground stations the satellite is
/// on-boarded to (`25994=Ohio 1+Oregon 1`). Read once when this service's store is created.
/// Unset means no satellite, which is what a new AWS account is on-boarded to.
pub const SATELLITES_ENV: &str = "GROUNDSTATION_SATELLITES";

/// The satellites `setting` onboards, keyed by the `satelliteId` each is minted with.
///
/// `,` separates entries and `+` separates one entry's ground stations because neither is
/// spellable in a `GroundStationName`, so no name can be split in two; the first `=` of an
/// entry separates the NORAD number from the names, which may contain further `=`.
///
/// An entry whose NORAD number is not an integer inside [`NORAD_SATELLITE_ID_RANGE`] is
/// skipped, as is one repeating a NORAD number already on-boarded (one spacecraft is one
/// satellite) and any ground station not spellable as a `GroundStationName`, so a typo
/// onboards nothing instead of something unintended. The list is bounded by
/// [`MAX_RECORDS_PER_STORE`] and each satellite's ground stations by
/// [`MAX_GROUND_STATIONS_PER_SATELLITE`].
pub fn parse_satellites(setting: &str) -> HashMap<String, Satellite> {
    let mut satellites: HashMap<String, Satellite> = HashMap::new();
    let mut onboarded: Vec<i32> = Vec::new();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || satellites.len() >= MAX_RECORDS_PER_STORE {
            continue;
        }
        let (norad, names) = match entry.split_once('=') {
            None => (entry, ""),
            Some((norad, names)) => (norad.trim(), names),
        };
        let Ok(norad_satellite_id) = norad.parse::<i32>() else {
            continue;
        };
        if !is_norad_satellite_id(norad_satellite_id) || onboarded.contains(&norad_satellite_id) {
            continue;
        }
        let ground_stations: Vec<String> = names
            .split('+')
            .map(str::trim)
            .filter(|name| is_ground_station_name(name))
            .map(str::to_string)
            .take(MAX_GROUND_STATIONS_PER_SATELLITE)
            .collect();
        let satellite_id = new_uuid();
        let satellite_arn =
            ::engine::emulator_arn(ARN_SERVICE, "", &format!("satellite/{satellite_id}"));
        onboarded.push(norad_satellite_id);
        satellites.insert(
            satellite_id.clone(),
            Satellite {
                satellite_id,
                satellite_arn,
                norad_satellite_id,
                ground_stations,
                current_ephemeris: None,
            },
        );
    }
    satellites
}

/// The environment variable an operator uses to take a ground station's antenna out of
/// service for maintenance, since the API models no operation that schedules one: a
/// comma-separated list of entries, each
/// `<ground station>/<antenna>/<maintenance type>/<start>/<end>` with the two instants in
/// epoch seconds (`"Ohio 1/Antenna 1/PLANNED/1763000000/1763003600"`). Read once when this
/// service's store is created. Unset means nothing is out of service, which is what a
/// ground station network normally is.
///
/// `,` separates entries and `/` the fields of one, because neither is spellable in a
/// `GroundStationName` or an `AntennaName`, so no name can be split in two.
pub const MAINTENANCE_ENV: &str = "GROUNDSTATION_MAINTENANCE";

/// The maintenance windows `setting` schedules, in the order it lists them.
///
/// An entry that is not five `/`-separated fields — a ground station and an antenna
/// spellable as their shapes require, a `MaintenanceType` of [`MAINTENANCE_TYPES`], and a
/// window of two epoch-seconds instants running forwards — is skipped rather than guessed
/// at, so a typo takes nothing out of service instead of something unintended. The list is
/// bounded by [`MAX_RECORDS_PER_STORE`].
pub fn parse_maintenance(setting: &str) -> Vec<MaintenanceReservation> {
    let mut windows: Vec<MaintenanceReservation> = Vec::new();
    for entry in setting.split(',') {
        if windows.len() >= MAX_RECORDS_PER_STORE {
            break;
        }
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let fields: Vec<&str> = entry.split('/').map(str::trim).collect();
        let [ground_station, antenna, maintenance_type, start, end] = fields[..] else {
            continue;
        };
        if !is_ground_station_name(ground_station)
            || !is_antenna_name(antenna)
            || !MAINTENANCE_TYPES.contains(&maintenance_type)
        {
            continue;
        }
        let (Ok(start), Ok(end)) = (start.parse::<i64>(), end.parse::<i64>()) else {
            continue;
        };
        if end <= start {
            continue;
        }
        windows.push(MaintenanceReservation {
            ground_station: ground_station.to_string(),
            antenna_name: antenna.to_string(),
            maintenance_type: maintenance_type.to_string(),
            start_time: DateTime::from_secs(start),
            end_time: DateTime::from_secs(end),
        });
    }
    windows
}

/// What this service answers when a `groundStationId` names no ground station this
/// account is served by: a fixed sentence with no variable part, which deliberately
/// reports the name as invalid, unknown OR inaccessible rather than asserting it does not
/// exist, and echoes nothing of the request back.
///
/// Shared by every operation whose subject is a ground station addressed by name
/// ([`ComAmazonawsGroundstationGroundstationState::knows_ground_station`]), so each of
/// them answers one text of the service. The operations that resolve a ground station
/// model no not-found error for it (`ListAntennas` models only
/// `InvalidParameterException` — `smithy_query(query="operation",
/// operation="ListAntennas")`), so an unresolvable name is a bad parameter by
/// construction, which is what this sentence says.
pub const GROUND_STATION_INVALID: &str =
    "Invalid request: Ground station is invalid, does not exist, or you do not have access to it";

/// The environment variable an operator uses to describe the ground stations THIS
/// emulated account is served by and the antennas at each, since the API models no
/// operation that builds one: a comma-separated list of entries, each either a bare
/// `GroundStationName` (a ground station whose antennas this account has not been told
/// about) or `<ground station>=<antenna>+<antenna>…` naming the antennas at it
/// (`Ohio 1=Antenna 1+Antenna 2,Oregon 1`). Read once when this service's store is
/// created. Unset means no inventory, in which case the only ground stations this account
/// knows are the ones its satellite onboardings name, and none of them lists an antenna.
pub const GROUND_STATIONS_ENV: &str = "GROUNDSTATION_GROUND_STATIONS";

/// The ground stations `setting` describes, keyed by the `GroundStationName` each is
/// addressed by.
///
/// `,` separates entries and `+` separates one entry's antennas because neither is
/// spellable in a `GroundStationName` or an `AntennaName`, so no name can be split in
/// two; the first `=` of an entry separates the ground station from its antennas, which
/// may contain further `=`.
///
/// An entry whose ground station is not spellable as a `GroundStationName` is skipped, as
/// is one repeating a ground station already described (one ground station is one entry)
/// and any antenna not spellable as an `AntennaName` or repeating one already at that
/// ground station, so a typo describes nothing instead of something unintended. The
/// inventory is bounded by [`MAX_RECORDS_PER_STORE`] and each ground station's antennas by
/// [`MAX_ANTENNAS_PER_GROUND_STATION`].
pub fn parse_ground_stations(setting: &str) -> HashMap<String, GroundStation> {
    let mut inventory: HashMap<String, GroundStation> = HashMap::new();
    for entry in setting.split(',') {
        let entry = entry.trim();
        if entry.is_empty() || inventory.len() >= MAX_RECORDS_PER_STORE {
            continue;
        }
        let (name, antennas) = match entry.split_once('=') {
            None => (entry, ""),
            Some((name, antennas)) => (name.trim(), antennas),
        };
        if !is_ground_station_name(name) || inventory.contains_key(name) {
            continue;
        }
        let mut at_station: Vec<String> = Vec::new();
        for antenna in antennas.split('+').map(str::trim) {
            if !is_antenna_name(antenna)
                || at_station.iter().any(|held| held == antenna)
                || at_station.len() >= MAX_ANTENNAS_PER_GROUND_STATION
            {
                continue;
            }
            at_station.push(antenna.to_string());
        }
        // Held in NAME order, so the order a listing reports them in is a property of the
        // store rather than of each read. The bound above keeps the first names the
        // setting named; the sort only arranges what it kept.
        at_station.sort();
        inventory.insert(
            name.to_string(),
            GroundStation {
                ground_station_name: name.to_string(),
                antennas: at_station,
            },
        );
    }
    inventory
}

/// The value `variable` holds, parsed by `parse`, or nothing when it is not set.
fn from_env<T: Default>(variable: &str, parse: impl Fn(&str) -> T) -> T {
    ::std::env::var(variable)
        .map(|setting| parse(&setting))
        .unwrap_or_default()
}

impl Default for ComAmazonawsGroundstationGroundstationState {
    /// An account with no resources, the uplink approval [`APPROVED_UPLINK_BANDS_ENV`]
    /// grants it, the reserved minutes [`RESERVED_MINUTES_ENV`] commits, the satellites
    /// [`SATELLITES_ENV`] onboards, the ground station maintenance [`MAINTENANCE_ENV`]
    /// schedules and the ground stations [`GROUND_STATIONS_ENV`] describes (none of them,
    /// unless they are set).
    fn default() -> Self {
        Self {
            configs: HashMap::new(),
            mission_profiles: HashMap::new(),
            contacts: HashMap::new(),
            dataflow_endpoint_groups: HashMap::new(),
            agents: HashMap::new(),
            ephemerides: HashMap::new(),
            arn_index: HashMap::new(),
            approved_uplink_bands_mhz: from_env(
                APPROVED_UPLINK_BANDS_ENV,
                parse_approved_uplink_bands,
            ),
            reserved_minutes: from_env(RESERVED_MINUTES_ENV, parse_reserved_minutes),
            satellites: from_env(SATELLITES_ENV, parse_satellites),
            maintenance_reservations: from_env(MAINTENANCE_ENV, parse_maintenance),
            ground_stations: from_env(GROUND_STATIONS_ENV, parse_ground_stations),
        }
    }
}

/// What one end of a dataflow edge reports, before it is placed in the `Source` or the
/// `Destination` the edge wired it to — the two shapes carry the same three facts about
/// the config, under a region member each names differently.
struct DataflowEnd {
    config_type: String,
    config_id: String,
    config_details: Option<ConfigDetails>,
    region: String,
}

/// One dataflow edge of `contact`'s pass as a `DataflowDetail`: its first `ConfigArn` is
/// the dataflow's SOURCE and its second the DESTINATION (a `DataflowEdge` is a list of
/// exactly two — `smithy_query(query="shape", shape="DataflowEdge")`).
///
/// Each end is described by the parameters THIS contact's pass is flown with
/// ([`ComAmazonawsGroundstationGroundstationState::contact_config_data`]), so a config
/// updated after the contact was scheduled does not move what it reports.
///
/// `errorMessage` carries nothing: no dataflow has run in this emulator, so none has
/// failed.
fn dataflow_detail(
    state: &ComAmazonawsGroundstationGroundstationState,
    contact: &Contact,
    edge: &[String],
) -> DataflowDetail {
    let source = edge
        .first()
        .and_then(|arn| dataflow_end(state, contact, arn))
        .map(|end| Source {
            config_type: Some(end.config_type),
            config_id: Some(end.config_id),
            config_details: end.config_details,
            dataflow_source_region: Some(end.region),
        });
    let destination = edge
        .get(1)
        .and_then(|arn| dataflow_end(state, contact, arn))
        .map(|end| Destination {
            config_type: Some(end.config_type),
            config_id: Some(end.config_id),
            config_details: end.config_details,
            dataflow_destination_region: Some(end.region),
        });
    DataflowDetail {
        source,
        destination,
        error_message: None,
    }
}

/// The end of a dataflow the `ConfigArn` `arn` names, or `None` when `arn` addresses no
/// config at all.
///
/// The type and the id are the ARN's own ([`config_arn_parts`]), so an edge is reported by
/// the config it names rather than echoed back whole. The details are built from the
/// parameters `contact`'s pass is flown with
/// ([`ComAmazonawsGroundstationGroundstationState::contact_config_data`]) — the config's
/// own until an update of it replaced them — for the
/// three capability types `ConfigDetails` has a member for — "additional details for a
/// `Config`, if type is `dataflow-endpoint` or `antenna-downlink-demod-decode`", plus
/// `s3RecordingDetails` (`smithy_query(query="shape", shape="ConfigDetails")`) — and absent
/// for every other type, which that union can say nothing about:
///
/// * `dataflow-endpoint`: the endpoint of this account the config names, as the dataflow
///   endpoint group that holds it stored it; absent when no group holds one by that name,
///   since there is then no endpoint to describe.
/// * `antenna-downlink-demod-decode`: the output node the EDGE is wired to, which is the
///   node segment of the ARN it named ([`config_arn_node`]); absent when the edge named the
///   config as a whole.
/// * `s3-recording`: the bucket the config records to, and the key template it records
///   under — the config's own `prefix`, which is the template the keys of that recording
///   are built from.
///
/// The region is where that end of the dataflow sits: a `dataflow-endpoint` config may
/// place its endpoint in a region of its own (`dataflowEndpointRegion`), and every other
/// end is at the region the contact itself was reserved in.
fn dataflow_end(
    state: &ComAmazonawsGroundstationGroundstationState,
    contact: &Contact,
    arn: &str,
) -> Option<DataflowEnd> {
    let (config_type, config_id) = config_arn_parts(arn)?;
    let contact_region = contact.region.as_str();
    let config_data = state.contact_config_data(contact, arn);

    let config_details = match config_data {
        Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => state
            .dataflow_endpoint(&endpoint_config.dataflow_endpoint_name)
            .map(ConfigDetails::endpointDetails),
        Some(ConfigTypeData::antennaDownlinkDemodDecodeConfig(_)) => {
            config_arn_node(arn).map(|output_node| {
                ConfigDetails::antennaDemodDecodeDetails(AntennaDemodDecodeDetails {
                    output_node: Some(output_node),
                })
            })
        }
        Some(ConfigTypeData::s3RecordingConfig(recording)) => {
            Some(ConfigDetails::s3RecordingDetails(S3RecordingDetails {
                bucket_arn: Some(recording.bucket_arn.clone()),
                key_template: recording.prefix.clone(),
            }))
        }
        _ => None,
    };

    let region = match config_data {
        Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => endpoint_config
            .dataflow_endpoint_region
            .clone()
            .unwrap_or_else(|| contact_region.to_string()),
        _ => contact_region.to_string(),
    };

    Some(DataflowEnd {
        config_type,
        config_id,
        config_details,
        region,
    })
}

/// The endpoint names the dataflow graph `edges` selects, in the order the graph names them
/// and without repeats, where `config_data` is what resolves each end's `ConfigArn` to the
/// parameters it is read with.
///
/// An edge ends at the config its `ConfigArn` names and a `dataflowEndpointConfig` selects
/// an endpoint of this account by name (`dataflowEndpointName`), so those names are what a
/// contact flown through this graph is wired to. An end whose ARN resolves to no parameters
/// contributes nothing.
///
/// The ONE reader of a graph's endpoint selections, so the question asked of a graph being
/// WRITTEN ([`ComAmazonawsGroundstationGroundstationState::dataflow_endpoint_names_wired_by`])
/// and the one asked of a graph a contact is flown through
/// ([`ComAmazonawsGroundstationGroundstationState::contact_wired_dataflow_endpoint_names`])
/// differ only in which parameters each resolves an end with.
fn endpoint_names_selected_by<'a>(
    edges: &[Vec<String>],
    mut config_data: impl FnMut(&str) -> Option<&'a ConfigTypeData>,
) -> Vec<&'a str> {
    let mut names: Vec<&str> = Vec::new();
    for arn in edges.iter().flatten() {
        if let Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) = config_data(arn) {
            let name = endpoint_config.dataflow_endpoint_name.as_str();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

impl ComAmazonawsGroundstationGroundstationState {
    /// Refuse a `ConfigTypeData` this account's authorisation or the model's own
    /// documented ranges do not allow, with the modelled `InvalidParameterException`
    /// (HTTP 431). See [`validate_config_data`] for the bounds.
    ///
    /// The one part of a `configData` this cannot judge from the store is an
    /// `s3RecordingConfig`'s bucket, which lives in another service:
    /// [`unresolvable_recording_bucket`] resolves it against S3 before the handler takes
    /// this lock.
    pub fn validate_config_data(
        &self,
        config_data: &ConfigTypeData,
    ) -> Result<(), InvalidParameterException> {
        validate_config_data(config_data, &self.approved_uplink_bands_mhz)
    }

    /// Every ground station this account reaches, as the store records it and with
    /// repeats: its own inventory ([`GROUND_STATIONS_ENV`]) and the stations the
    /// satellites it is on-boarded to are on-boarded to — the two places a ground station
    /// this account reaches is recorded. Written once here so "the ground stations of this
    /// account" is one rule, whether a caller asks whether one exists or asks for all of
    /// them.
    fn ground_station_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.ground_stations.keys().map(String::as_str).chain(
            self.satellites
                .values()
                .flat_map(|satellite| satellite.ground_stations.iter().map(String::as_str)),
        )
    }

    /// Whether this account knows a ground station called `name`. A name neither place
    /// records is a ground station this account cannot address.
    pub fn knows_ground_station(&self, name: &str) -> bool {
        self.ground_station_names().any(|station| station == name)
    }

    /// The ground stations this account is served by, each a `GroundStationName`, in name
    /// order and without repeats — so one station recorded in both places, or named twice
    /// in one, is one ground station, and a page boundary is stable whatever order the
    /// store holds them in. Empty for an account told about none.
    pub fn known_ground_stations(&self) -> Vec<String> {
        let mut names: Vec<String> = self.ground_station_names().map(str::to_string).collect();
        names.sort();
        names.dedup();
        names
    }

    /// The ground stations the satellite `satellite_id` is on-boarded to, in the same
    /// order and with the same de-duplication as [`Self::known_ground_stations`]; empty
    /// when this account holds no onboarding under that id.
    pub fn ground_stations_of_satellite(&self, satellite_id: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .satellites
            .get(satellite_id)
            .map(|satellite| satellite.ground_stations.clone())
            .unwrap_or_default();
        names.sort();
        names.dedup();
        names
    }

    /// The antennas at the ground station `name`, each an `AntennaName`, in the order the
    /// inventory holds them — by name, which [`parse_ground_stations`] is what arranges —
    /// so a page boundary is stable whatever order the setting named them in; empty when
    /// that ground station has none and when this account knows no station by that name.
    pub fn antennas_at(&self, name: &str) -> Vec<String> {
        self.ground_stations
            .get(name)
            .map(|station| station.antennas.clone())
            .unwrap_or_default()
    }

    /// The config `arn` names, if it names one.
    ///
    /// An ARN naming one NODE of a config resolves to that config too: the index holds
    /// the ARN of each config this service minted, and the modelled `ConfigArn` pattern
    /// lets a reference append a node to it ([`addresses_config`]), so such a reference is
    /// resolved through the config it addresses rather than looked up as an ARN of its
    /// own.
    pub fn config_by_arn(&self, arn: &str) -> Option<&Config> {
        match self.arn_index.get(arn) {
            Some(ResourceRef::Config {
                config_type,
                config_id,
            }) => self.configs.get(&(config_type.clone(), config_id.clone())),
            Some(_) => None,
            None => {
                let (config_type, config_id) = config_arn_parts(arn)?;
                let config = self.configs.get(&(config_type, config_id))?;
                addresses_config(arn, &config.config_arn).then_some(config)
            }
        }
    }

    /// Whether `arn` resolves to a config this service still holds, of `required_type`
    /// when the member that carries the reference is for one config type only.
    ///
    /// Shared by every operation that takes a config reference, so a reference to a
    /// config that was never created and one to a config that has since been DELETED
    /// are refused the same way.
    pub fn resolves_config(&self, arn: &str, required_type: Option<&str>) -> bool {
        self.config_by_arn(arn)
            .is_some_and(|config| required_type.is_none_or(|wanted| config.config_type == wanted))
    }

    /// Why `arn` cannot be used where only a config of `required_type` may be named, or
    /// `None` when it names one.
    ///
    /// The two faults are different conditions and this service answers them
    /// differently: an ARN resolving to no config of this account is a MISSING resource
    /// (`ResourceNotFoundException`, HTTP 434), while one resolving to a config that
    /// exists under another capability type is a BAD PARAMETER
    /// (`InvalidParameterException`, HTTP 431) — the reference resolved, and what is
    /// wrong with it is the type it points at, which no amount of waiting will change.
    /// See [`typed_reference_absent_message`] and
    /// [`typed_reference_wrong_type_message`] for the two texts.
    ///
    /// Used by every reference member whose resolution is type-checked against the ARN it
    /// was given — `uplinkEchoConfig$antennaUplinkConfigArn`, whose refusal quotes that
    /// ARN. A mission profile's config references are resolved differently: their
    /// recorded text names the id from the ARN under the type the MEMBER requires ("No
    /// config found with ID … and config type: … for customer …"), i.e. the lookup is by
    /// that pair, so a config of another type is genuinely nothing found there — see
    /// [`config_reference_not_found_message`] and [`resolves_config`].
    ///
    /// [`resolves_config`]: Self::resolves_config
    pub fn typed_reference_fault(
        &self,
        arn: &str,
        required_type: &str,
    ) -> Option<TypedReferenceFault> {
        match self.config_by_arn(arn) {
            None => Some(TypedReferenceFault::Absent),
            Some(config) if config.config_type != required_type => {
                Some(TypedReferenceFault::WrongType)
            }
            Some(_) => None,
        }
    }

    /// Why the config an `uplinkEchoConfig` in `config_data` echoes cannot be referenced,
    /// as the modelled refusal to answer with; `None` when the reference names a config of
    /// [`UPLINK_ECHO_REFERENCED_TYPE`] this account holds, and when the `configData`
    /// carries no `uplinkEchoConfig` at all.
    ///
    /// An `uplinkEchoConfig` echoes a specific antenna uplink config, named by ARN: like
    /// every ARN-valued reference it must resolve to a config that exists, and the config it
    /// names must be of the type the member is for. The two ways that fails are different
    /// answers ([`UplinkEchoReferenceFault`]): nothing there is a missing resource, while a
    /// config of another type is a bad parameter.
    ///
    /// Written here rather than in a handler because every write of a config holds an
    /// `uplinkEchoConfig` it stores to the same reference rule — the whole rule, which
    /// capability the reference admits and which fault answers which error included — so a
    /// create and an update of one echo config cannot answer differently. Each caller lifts
    /// the fault into its own error enum, the way both writers of a contact version lift
    /// [`Self::program_track_fault`].
    pub fn uplink_echo_reference_fault(
        &self,
        config_data: &ConfigTypeData,
    ) -> Option<UplinkEchoReferenceFault> {
        let ConfigTypeData::uplinkEchoConfig(echo) = config_data else {
            return None;
        };
        let arn = &echo.antenna_uplink_config_arn;
        match self.typed_reference_fault(arn, UPLINK_ECHO_REFERENCED_TYPE)? {
            TypedReferenceFault::Absent => Some(UplinkEchoReferenceFault::Absent(
                ResourceNotFoundException {
                    // The not-found text names the capability the reference required,
                    // spelled as the model spells that capability's shape, and the ARN the
                    // request gave — not the wire enum token, which is an on-the-wire
                    // spelling rather than a user-facing one.
                    message: Some(typed_reference_absent_message(
                        UPLINK_ECHO_REFERENCED_TYPE,
                        arn,
                    )),
                },
            )),
            TypedReferenceFault::WrongType => Some(UplinkEchoReferenceFault::WrongType(
                InvalidParameterException {
                    message: Some(typed_reference_wrong_type_message(
                        UPLINK_ECHO_REFERENCED_TYPE,
                    )),
                    parameter_name: Some(UPLINK_ECHO_REFERENCE_PARAMETER.to_string()),
                },
            )),
        }
    }

    /// Why the program track `overrides` name cannot be flown, as the modelled refusal to
    /// answer with; `None` when the ephemeris it names is one this account holds in the
    /// form the member requires, when the overrides name no program track, and when the
    /// write names no overrides at all.
    ///
    /// A program track points the antenna from an ephemeris of THIS account, so the
    /// reference is resolved before a contact version carrying it is stored — which is
    /// what every other reference this service's writes take is held to: a config ARN
    /// ([`Self::typed_reference_fault`], [`Self::resolves_config`]) and an ephemeris's own
    /// `satelliteId`. An id this account holds no ephemeris under references nothing, and
    /// one naming an ephemeris of another `EphemerisType` than the member requires
    /// ([`program_track_reference`]) points at a trajectory of the wrong form.
    ///
    /// Shared by both writers of a contact version, so a contact cannot be left holding a
    /// program track the other writer would have refused.
    pub fn program_track_fault(
        &self,
        overrides: Option<&TrackingOverrides>,
    ) -> Option<ProgramTrackFault> {
        let reference = program_track_reference(overrides?)?;
        match self.ephemerides.get(reference.ephemeris_id) {
            None => Some(ProgramTrackFault::Absent(ResourceNotFoundException {
                message: Some(EPHEMERIS_NOT_FOUND.to_string()),
            })),
            Some(ephemeris) if ephemeris.ephemeris_type != reference.required_type => {
                Some(ProgramTrackFault::WrongType(InvalidParameterException {
                    message: Some(wrong_ephemeris_type_message(reference.required_type)),
                    parameter_name: Some(reference.parameter_name),
                }))
            }
            Some(_) => None,
        }
    }

    /// Why the two configs an `edge` of a mission profile's `dataflowEdges` names cannot
    /// form a dataflow, as the modelled `InvalidParameterException` (HTTP 431) to answer
    /// with; `None` when they can.
    ///
    /// An edge is one dataflow of a contact, so its ends are type-checked and not merely
    /// resolved: the source's capability must be one that originates a dataflow and the
    /// destination's one that receives data from THAT source
    /// ([`DATAFLOW_EDGE_CAPABILITY_PAIRS`]). The refusal names whichever end is at fault —
    /// the source when its capability starts no dataflow at all, otherwise the destination
    /// — and the capability types that end does admit, in the wording this service already
    /// uses for a config reference pointing at a config of a type its position does not
    /// take ([`wrong_config_type_message`]).
    ///
    /// An end whose ARN resolves to no config is NOT this fault: that is a missing
    /// resource, answered by [`resolves_config`] before an edge reaches here.
    ///
    /// Shared by the create and the update, so a profile cannot be left holding a graph a
    /// create would have refused.
    ///
    /// [`resolves_config`]: Self::resolves_config
    pub fn dataflow_edge_fault(&self, edge: &[String]) -> Option<InvalidParameterException> {
        // The model makes an edge a 2..2 list (`smithy_query(query="shape",
        // shape="DataflowEdge")`), so the boundary validator has already refused any other
        // length and there is no pair here to judge.
        let [source, destination] = edge else {
            return None;
        };
        let source_type = &self.config_by_arn(source)?.config_type;
        let destination_type = &self.config_by_arn(destination)?.config_type;

        let admissible_destinations = dataflow_edge_destination_types(source_type);
        if admissible_destinations.contains(&destination_type.as_str()) {
            return None;
        }
        let admissible = if admissible_destinations.is_empty() {
            dataflow_edge_source_types()
        } else {
            admissible_destinations
        };
        Some(invalid_parameter(
            DATAFLOW_EDGES_PARAMETER,
            wrong_config_type_message(&admissible),
        ))
    }

    /// The ARNs of the mission profiles that reference the config `config_arn` names, in
    /// a stable order; empty when nothing references it.
    ///
    /// Every member of a mission profile that carries a `ConfigArn` is an inbound
    /// reference — `trackingConfigArn`, `telemetrySinkConfigArn` and both ends of every
    /// entry of `dataflowEdges` (`smithy_query(query="shape",
    /// shape="CreateMissionProfileRequest")`) — so a config is in use when ANY of them
    /// names it, not only the tracking one. Sorted, so a refusal listing them reads the
    /// same on every call whatever order the map iterates in.
    pub fn mission_profiles_referencing(&self, config_arn: &str) -> Vec<String> {
        let mut referencing: Vec<String> = self
            .mission_profiles
            .values()
            .filter(|profile| profile.references_config(config_arn))
            .map(|profile| profile.mission_profile_arn.clone())
            .collect();
        referencing.sort();
        referencing
    }

    /// The ARNs of the configs that reference the config `config_arn` names, in a stable
    /// order; empty when no config references it.
    ///
    /// A config's own `configData` can carry a `ConfigArn`
    /// ([`Config::references_config`]), so a config is an inbound reference holder exactly
    /// as a mission profile is ([`Self::mission_profiles_referencing`]) and makes the
    /// config it names in use. The config `config_arn` names is never counted as a holder
    /// of itself. Sorted, so a refusal listing them reads the same on every call whatever
    /// order the map iterates in.
    pub fn configs_referencing(&self, config_arn: &str) -> Vec<String> {
        let mut referencing: Vec<String> = self
            .configs
            .values()
            .filter(|config| {
                config.config_arn != config_arn && config.references_config(config_arn)
            })
            .map(|config| config.config_arn.clone())
            .collect();
        referencing.sort();
        referencing
    }

    /// The ids of the contacts still to be flown that name the mission profile
    /// `mission_profile_arn` names, in a stable order; empty when nothing holds it.
    ///
    /// A contact carries the ARN of the profile it was reserved against
    /// ([`Contact::mission_profile_arn`]) and reads the wiring of its pass back out of
    /// that profile until an update of it moves away from that wiring
    /// ([`Self::contact_dataflow_edges`]), so a contact holds its profile for
    /// as long as it is NOT terminal ([`Contact::is_non_terminal`]) — once its pass is
    /// over or its reservation released, nothing of it is still to be flown and it no
    /// longer depends on the profile. Sorted, so a refusal listing them reads the same on
    /// every call whatever order the map iterates in.
    pub fn contacts_using_mission_profile(&self, mission_profile_arn: &str) -> Vec<String> {
        let mut holding: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| {
                contact.is_non_terminal() && contact.mission_profile_arn == mission_profile_arn
            })
            .map(|contact| contact.contact_id.clone())
            .collect();
        holding.sort();
        holding
    }

    /// Remove the config the `(configType, configId)` pair addresses, along with the ARN
    /// index entry that resolved to it, and answer it; `None` when the pair addresses no
    /// config.
    ///
    /// Dropping the index entry is what makes the deletion complete: an ARN-valued
    /// reference to a deleted config then resolves to nothing, exactly as a reference to
    /// one that never existed does.
    pub fn remove_config(&mut self, config_type: &str, config_id: &str) -> Option<Config> {
        let removed = self
            .configs
            .remove(&(config_type.to_string(), config_id.to_string()))?;
        self.arn_index.remove(&removed.config_arn);
        Some(removed)
    }

    /// Remove the mission profile `mission_profile_id` addresses, along with its ARN
    /// index entry, and answer it; `None` when no profile has that id.
    pub fn remove_mission_profile(&mut self, mission_profile_id: &str) -> Option<MissionProfile> {
        let removed = self.mission_profiles.remove(mission_profile_id)?;
        self.arn_index.remove(&removed.mission_profile_arn);
        Some(removed)
    }

    /// The satellite `arn` names, if this account is on-boarded to one.
    ///
    /// Resolved by the ARN each onboarding CARRIES, which is the key every rule of this
    /// service that couples a satellite to something else is written against — the pointing
    /// data an availability search reads ([`Self::pointing_passes`]) and the ephemeris a
    /// contact holds through its satellite ([`Self::contacts_using_ephemeris`]) both select
    /// on `satellite_arn` — so a reference that resolves here resolves for them too.
    /// Satellites are not minted by this API, so they carry no [`Self::arn_index`] entry
    /// and the lookup is over the onboardings themselves.
    ///
    /// [`Self::pointing_passes`]: Self::pointing_passes
    pub fn satellite_by_arn(&self, arn: &str) -> Option<&Satellite> {
        self.satellites
            .values()
            .find(|satellite| satellite.satellite_arn == arn)
    }

    /// Why the satellite a write's `satelliteArn` names cannot be flown, as the modelled
    /// `ResourceNotFoundException` (HTTP 434) to answer with; `None` when it names a
    /// satellite this account is on-boarded to and when the write names no satellite at
    /// all (the member is optional on both writers — `smithy_query(query="operation",
    /// operation="ReserveContact,UpdateContact")`).
    ///
    /// A contact is flown against a satellite of THIS account, so the reference is resolved
    /// before a contact version carrying it is stored — which is what every other reference
    /// this service's writes take is held to: a config ARN ([`Self::resolves_config`]), an
    /// ephemeris id ([`Self::program_track_fault`]) and an ephemeris's own `satelliteId`.
    /// An ARN no onboarding of this account carries references nothing.
    ///
    /// Shared by both writers of a contact version, so a contact cannot be left flown
    /// against a satellite the other writer would have refused.
    ///
    /// [`Self::resolves_config`]: Self::resolves_config
    pub fn satellite_reference_fault(
        &self,
        satellite_arn: Option<&str>,
    ) -> Option<ResourceNotFoundException> {
        let arn = satellite_arn?;
        self.satellite_by_arn(arn)
            .is_none()
            .then(|| ResourceNotFoundException {
                message: Some(satellite_reference_not_found_message(arn)),
            })
    }

    /// The mission profile `arn` names, if it names one.
    pub fn mission_profile_by_arn(&self, arn: &str) -> Option<&MissionProfile> {
        match self.arn_index.get(arn)? {
            ResourceRef::MissionProfile { mission_profile_id } => {
                self.mission_profiles.get(mission_profile_id)
            }
            _ => None,
        }
    }

    /// The dataflow endpoint of this account named `name`, as a read of the group that
    /// holds it reports it ([`Self::reported_endpoint`]); `None` when no group of this
    /// account holds one by that name.
    ///
    /// A `dataflowEndpointConfig` names an endpoint by `dataflowEndpointName` rather than
    /// by the group it belongs to, so the lookup is over every group's endpoints
    /// ([`endpoint_name`] reads the name whichever member describes the endpoint). Groups
    /// are taken in id order, so two endpoints sharing a name resolve to the same one on
    /// every call whatever order the map iterates in.
    pub fn dataflow_endpoint(&self, name: &str) -> Option<EndpointDetails> {
        let mut group_ids: Vec<&String> = self.dataflow_endpoint_groups.keys().collect();
        group_ids.sort();
        group_ids
            .into_iter()
            .filter_map(|id| self.dataflow_endpoint_groups.get(id))
            .find_map(|group| {
                group
                    .endpoint_details
                    .iter()
                    .find(|details| endpoint_name(details) == Some(name))
                    .map(|details| self.reported_endpoint(details, group))
            })
    }

    /// The ids of the dataflow endpoint groups of this account that hold an endpoint named
    /// `name`, in id order; empty when no group of this account holds one by that name.
    ///
    /// An endpoint name is unique only WITHIN one group ([`claim_endpoint_name`]), so a
    /// name can be held by several groups, and which groups hold it is what decides
    /// whether a graph selecting several names can be served by one group
    /// ([`Self::dataflow_endpoint_group_fault`]). Sorted, so the answer is the same on
    /// every call whatever order the map iterates in.
    pub fn dataflow_endpoint_group_ids_holding(&self, name: &str) -> Vec<&str> {
        let mut ids: Vec<&str> = self
            .dataflow_endpoint_groups
            .iter()
            .filter(|(_, group)| {
                group
                    .endpoint_details
                    .iter()
                    .any(|details| endpoint_name(details) == Some(name))
            })
            .map(|(id, _)| id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The endpoint names the dataflow graph `edges` selects, in the order the graph names
    /// them and without repeats; empty when it selects none.
    ///
    /// An edge ends at the config its `ConfigArn` names and a `dataflowEndpointConfig`
    /// selects an endpoint of this account by name (`dataflowEndpointName`), so those names
    /// are what a contact flown through this graph is wired to. An end whose ARN resolves
    /// to no config contributes nothing: that is a missing resource, answered by
    /// [`Self::resolves_config`].
    ///
    /// Written against a graph rather than a stored profile, so the writes that take one
    /// ([`Self::dataflow_endpoint_group_fault`]) and the reads of one that was stored
    /// ([`Self::contact_wired_dataflow_endpoint_names`]) resolve it the same way. The
    /// selections are read from the configs as they stand NOW, which is what a write that
    /// is being judged is judged against; a read of one CONTACT's graph resolves them
    /// through the parameters that contact's pass is flown with instead
    /// ([`Self::contact_wired_dataflow_endpoint_names`]).
    pub fn dataflow_endpoint_names_wired_by<'a>(&'a self, edges: &[Vec<String>]) -> Vec<&'a str> {
        endpoint_names_selected_by(edges, |arn| {
            self.config_by_arn(arn).map(|config| &config.config_data)
        })
    }

    /// Why the dataflow graph `edges` cannot be flown as ONE contact, as the modelled
    /// `InvalidParameterException` (HTTP 431) to answer with; `None` when it can.
    ///
    /// "When a contact uses multiple `DataflowEndpointConfig` objects, each `Config` must
    /// match a `DataflowEndpoint` in the same group" (`smithy_query(query="operation",
    /// operation="CreateDataflowEndpointGroup")`), so the endpoints a graph selects
    /// ([`Self::dataflow_endpoint_names_wired_by`]) have to be servable by a SINGLE group:
    /// a graph for which no one group of this account holds every name it selects
    /// describes a contact the service could not run, and is refused rather than stored.
    /// The refusal names the selections that cannot share a group
    /// ([`dataflow_endpoints_of_different_groups_message`]).
    ///
    /// A name no group of this account holds yet constrains nothing — an endpoint group
    /// may be created after the config that selects its endpoint, so such a selection is
    /// not yet in any group and says nothing about which group the contact runs through.
    /// A name several groups hold ([`Self::dataflow_endpoint_group_ids_holding`]) narrows
    /// the groups that could serve the graph without picking one, so a graph stays
    /// acceptable while ANY group holds every name it selects.
    ///
    /// Shared by the create and the update, so a profile cannot be left holding a graph a
    /// create would have refused.
    pub fn dataflow_endpoint_group_fault(
        &self,
        edges: &[Vec<String>],
    ) -> Option<InvalidParameterException> {
        self.endpoint_group_fault_over(
            DATAFLOW_EDGES_PARAMETER,
            &self.dataflow_endpoint_names_wired_by(edges),
        )
    }

    /// Why the endpoint selections `names`, in the order the graph that selects them names
    /// them, cannot all be served by ONE dataflow endpoint group, as the modelled
    /// `InvalidParameterException` (HTTP 431) naming `parameter` to answer with; `None`
    /// when one group can serve them.
    ///
    /// The whole of the group rule, asked of a SELECTION SET rather than of one graph, so
    /// every write judged by it — a graph a mission profile write carries
    /// ([`Self::dataflow_endpoint_group_fault`]) and the graphs a config write would leave
    /// the profiles that reference that config holding
    /// ([`Self::config_data_change_group_fault`]) — reaches the same verdict and the same
    /// sentence, and only the member each refusal names differs.
    fn endpoint_group_fault_over(
        &self,
        parameter: &str,
        names: &[&str],
    ) -> Option<InvalidParameterException> {
        // The groups that hold every selection judged so far, and those selections.
        let mut candidates: Vec<&str> = Vec::new();
        let mut constrained: Vec<&str> = Vec::new();
        for name in names.iter().copied() {
            let holders = self.dataflow_endpoint_group_ids_holding(name);
            if holders.is_empty() {
                continue;
            }
            if constrained.is_empty() {
                candidates = holders;
                constrained.push(name);
                continue;
            }
            let shared: Vec<&str> = candidates
                .iter()
                .copied()
                .filter(|id| holders.contains(id))
                .collect();
            if shared.is_empty() {
                constrained.push(name);
                return Some(invalid_parameter(
                    parameter,
                    dataflow_endpoints_of_different_groups_message(&constrained),
                ));
            }
            candidates = shared;
            constrained.push(name);
        }
        None
    }

    /// Why the config `config_arn` names cannot be re-parameterised to `replacement`, as
    /// the modelled `InvalidParameterException` (HTTP 431) to answer with; `None` when it
    /// can.
    ///
    /// A `dataflowEndpointConfig`'s `dataflowEndpointName` is what a mission profile's
    /// graph selects its endpoints THROUGH ([`Self::dataflow_endpoint_names_wired_by`]), so
    /// replacing a stored config's parameters re-aims every graph that already ends at that
    /// config. The group rule is a property of the graph a profile HOLDS, not of the call
    /// that stored it ([`Self::dataflow_endpoint_group_fault`]): a write that would leave a
    /// stored profile holding a graph no one dataflow endpoint group can serve is refused
    /// just as the write of that graph would have been, so no sequence of accepted calls
    /// reaches a state the service would not have let a caller build directly. This is the
    /// same reason a config a profile still references is not DELETED out from under it
    /// ([`Self::mission_profiles_referencing`]).
    ///
    /// Every profile that references the config is judged, in ARN order, and each with the
    /// replacement standing in for that config wherever its graph reaches it — by the
    /// config's own ARN or by one of its NODES ([`addresses_config`]) — so the verdict is
    /// the one the profiles would be read with once the write landed.
    pub fn config_data_change_group_fault(
        &self,
        config_arn: &str,
        replacement: &ConfigTypeData,
    ) -> Option<InvalidParameterException> {
        for profile_arn in self.mission_profiles_referencing(config_arn) {
            let Some(profile) = self.mission_profile_by_arn(&profile_arn) else {
                continue;
            };
            let names = endpoint_names_selected_by(&profile.dataflow_edges, |arn| {
                match self.config_by_arn(arn) {
                    Some(config) if config.config_arn == config_arn => Some(replacement),
                    resolved => resolved.map(|config| &config.config_data),
                }
            });
            if let Some(refusal) =
                self.endpoint_group_fault_over(DATAFLOW_ENDPOINT_NAME_PARAMETER, &names)
            {
                return Some(refusal);
            }
        }
        None
    }

    /// The endpoints of `group` as a read of it REPORTS them: what the create stored, with
    /// every member the endpoint's own agent decides taken from that agent
    /// ([`apply_agent_endpoint_health`]).
    ///
    /// Derived on read rather than copied into the group, so one record of an agent answers
    /// every endpoint it carries: an agent registers for its capabilities and then reports
    /// its status against the agent resource, and both are visible in the endpoints that
    /// agent carries without either call having to walk the groups.
    ///
    /// Shared by every read that reports an endpoint, so two readers of one stored endpoint
    /// cannot describe its health differently.
    pub fn reported_endpoint_details(&self, group: &DataflowEndpointGroup) -> Vec<EndpointDetails> {
        group
            .endpoint_details
            .iter()
            .map(|details| self.reported_endpoint(details, group))
            .collect()
    }

    /// One endpoint of `group` as a read of it reports it. See
    /// [`Self::reported_endpoint_details`].
    pub fn reported_endpoint(
        &self,
        details: &EndpointDetails,
        group: &DataflowEndpointGroup,
    ) -> EndpointDetails {
        let mut reported = details.clone();
        apply_agent_endpoint_health(
            &mut reported,
            self.agent_for_capability(&group.dataflow_endpoint_group_arn),
        );
        reported
    }

    /// The registered agent that carries the capability `capability_arn` addresses, when
    /// one has registered for it.
    ///
    /// A registration carries "data for associating an agent with the capabilities it is
    /// managing" (`smithy_query(query="shape", shape="DiscoveryData")`), whose
    /// `capabilityArns` is the only thing in it that names what the agent manages, and the
    /// capability an agent-carried endpoint belongs to is the dataflow endpoint group that
    /// holds it — addressed, like every other resource of this service, by the ARN its
    /// create minted. So an agent carries a group's agent endpoints exactly when its
    /// registration named that group's ARN.
    ///
    /// Two agents may name one capability — an agent registering for a capability another
    /// already named takes it over, the way a customer replacing the instance their agent
    /// runs on does — so the LATEST registration carries it, ties broken by agent id, which
    /// is what makes the answer the same on every call whatever order the map iterates in.
    pub fn agent_for_capability(&self, capability_arn: &str) -> Option<&Agent> {
        let mut carrying: Vec<&Agent> = self
            .agents
            .values()
            .filter(|agent| {
                agent
                    .discovery_data
                    .capability_arns
                    .iter()
                    .any(|arn| arn == capability_arn)
            })
            .collect();
        carrying.sort_by(|left, right| {
            left.registered
                .as_secs_f64()
                .total_cmp(&right.registered.as_secs_f64())
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        carrying.pop()
    }

    /// The dataflow graph `contact`'s pass is flown through, or `None` when nothing of its
    /// wiring is known any more.
    ///
    /// The wiring a contact is flown with is the one it was RESERVED with: the edges held
    /// on the contact once an update of its profile moved away from them
    /// ([`Contact::reserved_dataflow_edges`]), and otherwise the edges the profile it names
    /// still holds — one record of each edge for as long as the two agree about it. A
    /// contact that holds no wiring of its own and whose profile has since been DELETED (a
    /// terminal contact no longer holds its profile) knows nothing of what it was wired to,
    /// which is what `None` says.
    ///
    /// The ONE answer to "what is this contact wired to", so a read of its dataflows
    /// ([`Self::contact_dataflow_list`]) and the question of what resources its pass still
    /// depends on ([`Self::contact_wired_dataflow_endpoint_names`]) cannot disagree.
    pub fn contact_dataflow_edges<'a>(
        &'a self,
        contact: &'a Contact,
    ) -> Option<&'a [Vec<String>]> {
        if let Some(edges) = contact.reserved_dataflow_edges.as_deref() {
            return Some(edges);
        }
        Some(
            self.mission_profile_by_arn(&contact.mission_profile_arn)?
                .dataflow_edges
                .as_slice(),
        )
    }

    /// The dataflows of `contact`'s pass, as a `DataflowList`: one entry per dataflow edge
    /// of the graph it is flown through ([`Self::contact_dataflow_edges`]), each end
    /// reported by the config that edge wired it to and placed in the contact's own region
    /// unless the config names a region of its own ([`dataflow_detail`]). `None` when that
    /// graph is no longer known at all.
    ///
    /// Shared by every read that reports a contact's dataflows, so two readers of one
    /// stored wiring cannot describe it differently.
    pub fn contact_dataflow_list(&self, contact: &Contact) -> Option<Vec<DataflowDetail>> {
        let edges = self.contact_dataflow_edges(contact)?;
        Some(
            edges
                .iter()
                .map(|edge| dataflow_detail(self, contact, edge))
                .collect(),
        )
    }

    /// The parameters `contact`'s pass is flown with at the end of a dataflow the
    /// `ConfigArn` `arn` names, or `None` when nothing is known about that end's config.
    ///
    /// A config's parameters reach a contact when the contact is scheduled, and an update
    /// of the config does not reach a contact already scheduled with it, so the answer is
    /// the `configData` the contact holds for that config once an update replaced it
    /// ([`Contact::reserved_config_data`]) and otherwise the config's own — one record of
    /// those parameters for as long as the two agree about them, exactly as
    /// [`Self::contact_dataflow_edges`] keeps one record of each edge.
    ///
    /// The ONE answer to "with what parameters is this contact flown through that config",
    /// so a read of its dataflows ([`dataflow_end`]) and the question of what resources its
    /// pass still depends on ([`Self::contact_wired_dataflow_endpoint_names`]) cannot
    /// disagree. An ARN naming one NODE of a config is the same config's parameters, since
    /// the pair the frozen copy is keyed on is the ARN's own ([`config_arn_parts`]).
    pub fn contact_config_data<'a>(
        &'a self,
        contact: &'a Contact,
        arn: &str,
    ) -> Option<&'a ConfigTypeData> {
        if let Some(key) = config_arn_parts(arn) {
            if let Some(frozen) = contact.reserved_config_data.get(&key) {
                return Some(frozen);
            }
        }
        self.config_by_arn(arn).map(|config| &config.config_data)
    }

    /// Keep the dataflow wiring of every contact an update of the mission profile
    /// `mission_profile_id` must not reach, to be called BEFORE that update replaces it.
    ///
    /// "Updating a mission profile will not update the execution parameters for existing
    /// future contacts" (`smithy_query(query="doc", shape="UpdateMissionProfile")`), and
    /// the dataflows of a pass are such a parameter: a contact that still holds its profile
    /// ([`Contact::is_non_terminal`] — the same contacts that keep the profile from being
    /// deleted, [`Self::contacts_using_mission_profile`]) therefore goes on being flown
    /// through the graph it was reserved with. So the graph the profile holds NOW is copied
    /// onto each of those contacts first, and the update then moves the profile alone.
    ///
    /// A contact that already holds a graph of its own keeps it: that one is older, so it
    /// is the wiring that contact was reserved with, and a second update cannot rewire it
    /// either. A contact ALREADY terminal when the update lands has no pass left to fly and
    /// keeps nothing, so what it reports stays the profile's and a profile deleted out from
    /// under it still leaves it reporting no wiring at all.
    ///
    /// A contact that goes terminal LATER keeps the graph it was flown with, which is the
    /// record of that pass: becoming terminal gives nothing back, exactly as the pass
    /// stages do not give back the antenna time a contact has spent
    /// ([`Contact::antenna_time_spent`]). So two terminal contacts of one profile can
    /// report different wirings — the one each was flown through — and only a contact
    /// nothing ever rewired has no wiring of its own to report once its profile is gone.
    pub fn freeze_contact_dataflow_edges(&mut self, mission_profile_id: &str) {
        let Some(profile) = self.mission_profiles.get(mission_profile_id) else {
            return;
        };
        let mission_profile_arn = profile.mission_profile_arn.clone();
        let edges = profile.dataflow_edges.clone();
        for contact in self.contacts.values_mut() {
            if contact.mission_profile_arn == mission_profile_arn
                && contact.is_non_terminal()
                && contact.reserved_dataflow_edges.is_none()
            {
                contact.reserved_dataflow_edges = Some(edges.clone());
            }
        }
    }

    /// Keep the parameters of the config `(config_type, config_id)` addresses on every
    /// contact an update of that config must not reach, to be called BEFORE that update
    /// replaces them.
    ///
    /// "Updating a `Config` will not update the execution parameters for existing future
    /// contacts scheduled with this `Config`" (`smithy_query(query="doc",
    /// shape="UpdateConfig")`), and a config's `configData` IS the execution parameters of
    /// every dataflow end its ARN is wired to — the endpoint a `dataflowEndpointConfig`
    /// selects and the region it sits in, the bucket and key template an `s3RecordingConfig`
    /// records under ([`dataflow_end`]). So a contact still to be flown
    /// ([`Contact::is_non_terminal`]) that is scheduled with this config — one whose own
    /// wiring ([`Self::contact_dataflow_edges`]) names it, whether as a whole or by one of
    /// its nodes ([`addresses_config`]) — keeps the parameters it was scheduled with, and
    /// the update then moves the config alone.
    ///
    /// A contact that already holds parameters of its own for this config keeps them: those
    /// are older, so they are the ones that contact was scheduled with, and a second update
    /// cannot reach it either. A contact ALREADY terminal when the update lands has no pass
    /// left to fly and keeps nothing, so what it reports stays the config's; a contact that
    /// goes terminal LATER keeps what it was flown with, which is the record of that pass —
    /// the same division [`Self::freeze_contact_dataflow_edges`] makes.
    pub fn freeze_contact_config_data(&mut self, config_type: &str, config_id: &str) {
        let key = (config_type.to_string(), config_id.to_string());
        let Some(config) = self.configs.get(&key) else {
            return;
        };
        let config_arn = config.config_arn.clone();
        let config_data = config.config_data.clone();
        // Which contacts reach the config is asked of the store as a whole (a contact's
        // wiring can be its profile's), so the holders are collected first and the copies
        // written afterwards.
        let holders: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| {
                contact.is_non_terminal()
                    && !contact.reserved_config_data.contains_key(&key)
                    && self
                        .contact_dataflow_edges(contact)
                        .is_some_and(|edges| {
                            edges
                                .iter()
                                .flatten()
                                .any(|arn| addresses_config(arn, &config_arn))
                        })
            })
            .map(|contact| contact.contact_id.clone())
            .collect();
        for contact_id in holders {
            if let Some(contact) = self.contacts.get_mut(&contact_id) {
                contact
                    .reserved_config_data
                    .insert(key.clone(), config_data.clone());
            }
        }
    }

    /// The tags of the resource `arn` names, when this service holds one.
    ///
    /// EVERY resource this service hands out is taggable — a config, a mission profile, a
    /// contact, a dataflow endpoint group, an agent and an ephemeris each carry their own
    /// `tags` map, which is what their creates store and their reads report — so the
    /// lookup is by ARN through the one
    /// [`ComAmazonawsGroundstationGroundstationState::arn_index`] rather than per resource
    /// kind: that index is what makes an ARN-addressed operation resolve without knowing
    /// which map the resource lives in. An ARN this service never minted, or one whose
    /// resource has since been DELETED (a delete drops the index entry), resolves to
    /// nothing.
    ///
    /// This is the READ half, for the operation that reports a resource's tags;
    /// [`Self::tags_mut`] is the write half.
    pub fn tags_of(&self, arn: &str) -> Option<&HashMap<String, String>> {
        match self.arn_index.get(arn)? {
            ResourceRef::Config {
                config_type,
                config_id,
            } => self
                .configs
                .get(&(config_type.clone(), config_id.clone()))
                .map(|config| &config.tags),
            ResourceRef::MissionProfile { mission_profile_id } => self
                .mission_profiles
                .get(mission_profile_id)
                .map(|profile| &profile.tags),
            ResourceRef::Contact { contact_id } => {
                self.contacts.get(contact_id).map(|contact| &contact.tags)
            }
            ResourceRef::DataflowEndpointGroup {
                dataflow_endpoint_group_id,
            } => self
                .dataflow_endpoint_groups
                .get(dataflow_endpoint_group_id)
                .map(|group| &group.tags),
            ResourceRef::Agent { agent_id } => self.agents.get(agent_id).map(|agent| &agent.tags),
            ResourceRef::Ephemeris { ephemeris_id } => self
                .ephemerides
                .get(ephemeris_id)
                .map(|ephemeris| &ephemeris.tags),
        }
    }

    /// The tags of the resource `arn` names, for WRITING, when this service holds one.
    ///
    /// The write half of [`Self::tags_of`]: the same ARN resolution, handing back the
    /// stored map itself so a tag write is a read-modify-write of the ONE map the
    /// resource's reads report, under the single lock the caller already holds. Two
    /// writers that each add a key therefore both persist, whatever order they arrive in.
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        // Resolved to an owned reference first: the index borrow ends here, so the
        // resource's own map can be borrowed mutably below.
        match self.arn_index.get(arn)?.clone() {
            ResourceRef::Config {
                config_type,
                config_id,
            } => self
                .configs
                .get_mut(&(config_type, config_id))
                .map(|config| &mut config.tags),
            ResourceRef::MissionProfile { mission_profile_id } => self
                .mission_profiles
                .get_mut(&mission_profile_id)
                .map(|profile| &mut profile.tags),
            ResourceRef::Contact { contact_id } => self
                .contacts
                .get_mut(&contact_id)
                .map(|contact| &mut contact.tags),
            ResourceRef::DataflowEndpointGroup {
                dataflow_endpoint_group_id,
            } => self
                .dataflow_endpoint_groups
                .get_mut(&dataflow_endpoint_group_id)
                .map(|group| &mut group.tags),
            ResourceRef::Agent { agent_id } => self
                .agents
                .get_mut(&agent_id)
                .map(|agent| &mut agent.tags),
            ResourceRef::Ephemeris { ephemeris_id } => self
                .ephemerides
                .get_mut(&ephemeris_id)
                .map(|ephemeris| &mut ephemeris.tags),
        }
    }

    /// The names of the dataflow endpoints `contact`'s pass is wired to, in a stable order
    /// and without repeats; empty when its dataflows are wired to no endpoint, and when
    /// nothing of its wiring is known any more.
    ///
    /// A dataflow edge ends at the config its `ConfigArn` names, and a
    /// `dataflowEndpointConfig` selects an endpoint of this account by name
    /// (`dataflowEndpointName`), so those names are what the pass is wired to. The graph is
    /// the contact's own ([`Self::contact_dataflow_edges`]) and each end is resolved with
    /// the parameters that contact's pass is flown with
    /// ([`Self::contact_config_data`]) — the same graph and the same parameters a read of
    /// the contact reports the endpoint behind each edge end of ([`dataflow_end`]), so the
    /// in-use question and the read cannot disagree about what a pass is wired to. Its
    /// endpoint selections are resolved by [`endpoint_names_selected_by`], the
    /// one reader of a graph's selections, and the names are then sorted so a refusal
    /// listing them reads the same on every call.
    pub fn contact_wired_dataflow_endpoint_names<'a>(
        &'a self,
        contact: &'a Contact,
    ) -> Vec<&'a str> {
        let Some(edges) = self.contact_dataflow_edges(contact) else {
            return Vec::new();
        };
        let mut names =
            endpoint_names_selected_by(edges, |arn| self.contact_config_data(contact, arn));
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The ids of the contacts still to be flown whose dataflows are wired to an endpoint
    /// the dataflow endpoint group `dataflow_endpoint_group_id` holds, in a stable order;
    /// empty when nothing holds it, and when no group has that id.
    ///
    /// A contact is wired to the endpoints its own dataflow edges select by name
    /// ([`Self::contact_wired_dataflow_endpoint_names`]), and an endpoint carries that name whichever
    /// member of its group describes it ([`endpoint_name`]), so a contact holds the group
    /// holding any endpoint its wiring names. It holds it for as long as it is NOT terminal
    /// ([`Contact::is_non_terminal`]) — once its pass is over or its reservation released,
    /// nothing of it is still to be flown, so what it was wired to is no longer a live
    /// dependency. Sorted, so a refusal listing them reads the same on every call whatever
    /// order the map iterates in.
    ///
    /// An endpoint name is unique only WITHIN a group ([`claim_endpoint_name`]), so two
    /// groups may each hold one by the same name; a read of a contact resolves such a name
    /// to one of them ([`Self::dataflow_endpoint`]) while the account holds both, so a
    /// contact wired to that name holds EITHER group — a delete does not decide for the
    /// caller which of the two their pass was meant to run through.
    pub fn contacts_using_dataflow_endpoint_group(
        &self,
        dataflow_endpoint_group_id: &str,
    ) -> Vec<String> {
        let Some(group) = self.dataflow_endpoint_groups.get(dataflow_endpoint_group_id) else {
            return Vec::new();
        };
        let held: Vec<&str> = group
            .endpoint_details
            .iter()
            .filter_map(endpoint_name)
            .collect();
        let mut holding: Vec<String> = self
            .contacts
            .values()
            .filter(|contact| {
                contact.is_non_terminal()
                    && self
                        .contact_wired_dataflow_endpoint_names(contact)
                        .iter()
                        .any(|wired| held.contains(wired))
            })
            .map(|contact| contact.contact_id.clone())
            .collect();
        holding.sort();
        holding
    }

    /// Remove the dataflow endpoint group `dataflow_endpoint_group_id` addresses, along
    /// with its ARN index entry, and answer it; `None` when no group has that id.
    pub fn remove_dataflow_endpoint_group(
        &mut self,
        dataflow_endpoint_group_id: &str,
    ) -> Option<DataflowEndpointGroup> {
        let removed = self
            .dataflow_endpoint_groups
            .remove(dataflow_endpoint_group_id)?;
        self.arn_index.remove(&removed.dataflow_endpoint_group_arn);
        Some(removed)
    }

    /// Materialise every `SCHEDULING` -> `SCHEDULED` move, every `CANCELLING` ->
    /// `CANCELLED` move, every `UPDATING` -> `ACTIVE` contact-version move and every step
    /// of the pass lifecycle ([`CONTACT_PASS_LIFECYCLE`]) the clock has already made due.
    /// A reporting operation calls this before reading: the moves were owed at this
    /// instant whether or not anybody called, so applying them changes nothing a later
    /// caller could otherwise have seen.
    ///
    /// A contact's own status moves come FIRST and the version move last, so the version
    /// move is judged against the status the clock has already carried the contact to: a
    /// contact with no pass left to fly takes no version change
    /// ([`Contact::apply_due_versions`]).
    pub fn settle_due_contacts(&mut self) {
        let now = SystemTime::now();
        let now_seconds = now
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64());
        for contact in self.contacts.values_mut() {
            if contact
                .cancellation_settles_at
                .is_some_and(|due| due <= now)
            {
                if contact.contact_status == CONTACT_CANCELLING {
                    contact.contact_status = CONTACT_CANCELLED.to_string();
                    contact.last_updated = now_timestamp();
                }
                contact.cancellation_settles_at = None;
            }
            if contact.scheduling_settles_at.is_some_and(|due| due <= now) {
                if contact.contact_status == CONTACT_SCHEDULING {
                    contact.contact_status = CONTACT_SCHEDULED.to_string();
                    contact.last_updated = now_timestamp();
                }
                contact.scheduling_settles_at = None;
            }
            // And then the pass itself: the stage of its window the clock has reached.
            // Last of the contact's own moves, so a contact the scheduler has just
            // confirmed is carried on to the stage its own timestamps owe it in the same
            // settle.
            if contact.advance_pass_stage(now_seconds) {
                contact.last_updated = now_timestamp();
            }
            // The version move comes after every status move, so it is judged against the
            // status the clock has ALREADY carried the contact to: a contact the pass
            // lifecycle or a release has just ended takes no version change
            // ([`Contact::apply_due_versions`]).
            contact.apply_due_versions();
        }
    }

    /// The antennas at `ground_station` that something already holds for any instant of
    /// `window` (`(start, end)` in epoch seconds): the ones a maintenance window AWS
    /// scheduled has taken out of service, and the ones a contact of this account whose
    /// reservation still holds its antenna ([`Contact::reserves_antenna`]) is placed on.
    ///
    /// A released reservation holds nothing, so the antenna it was on is free again. Read
    /// in ONE pass over each store, so placing a pass does not rescan them per candidate
    /// antenna.
    pub fn antennas_held(&self, ground_station: &str, window: (f64, f64)) -> HashSet<&str> {
        let under_maintenance = self
            .maintenance_reservations
            .iter()
            .filter(|maintenance| maintenance.ground_station == ground_station)
            .filter(|maintenance| {
                windows_overlap(
                    (
                        maintenance.start_time.as_secs_f64(),
                        maintenance.end_time.as_secs_f64(),
                    ),
                    window,
                )
            })
            .map(|maintenance| maintenance.antenna_name.as_str());
        let reserved = self
            .contacts
            .values()
            .filter(|contact| contact.ground_station == ground_station)
            .filter(|contact| contact.reserves_antenna())
            .filter(|contact| {
                windows_overlap(
                    (
                        contact.start_time.as_secs_f64(),
                        contact.end_time.as_secs_f64(),
                    ),
                    window,
                )
            })
            .map(|contact| contact.antenna_name.as_str());
        under_maintenance.chain(reserved).collect()
    }

    /// The antennas a pass at `ground_station` can be placed on, in the order the
    /// placement takes them: the antennas this account has been told are AT that ground
    /// station ([`Self::antennas_at`], in the name order the inventory holds them) when it
    /// records any, and otherwise the one antenna this emulator numbers for a ground
    /// station it was told nothing about ([`antenna_name`]).
    ///
    /// A ground station's antennas are FINITE — AWS builds a fixed number of them at a
    /// site — so this list is also what bounds how much antenna time can be held there at
    /// once. CHOSEN count for a ground station with no recorded inventory: ONE antenna, the
    /// smallest a ground station can be operated with, so the one-pass-per-antenna rule
    /// still bounds it rather than that station taking unboundedly many overlapping passes;
    /// an operator that wants more says so with [`GROUND_STATIONS_ENV`].
    pub fn placement_antennas(&self, ground_station: &str) -> Vec<String> {
        let recorded = self.antennas_at(ground_station);
        if recorded.is_empty() {
            vec![antenna_name(1)]
        } else {
            recorded
        }
    }

    /// The antenna at `ground_station` a pass over `window` is placed on: the first of
    /// [`Self::placement_antennas`] nothing holds for any instant of it, or `None` when
    /// every one of them is held — the ground station has no antenna time left over that
    /// window, which is what a reservation of it is refused on.
    ///
    /// An antenna serves ONE pass at a time, so two reservations whose windows overlap at
    /// a ground station are on different antennas, and a reservation that overlaps none
    /// takes the first antenna. Overlap is [`windows_overlap`], which is inclusive at both
    /// ends, so two passes that merely TOUCH — one ending exactly as the next starts — are
    /// placed on different antennas too; the band between a pass and the next on one
    /// antenna is this emulator's reading, not a documented one, and it is the same reading
    /// the listing's window filter uses.
    ///
    /// CHOSEN placement: AWS assigns the antenna itself and documents no rule for which,
    /// and no request of this service names one, so the emulator places a pass on the first
    /// free antenna. A maintenance window on an antenna the placement never names
    /// ([`MAINTENANCE_ENV`] accepts any `AntennaName`) is an antenna no pass is placed on,
    /// so it holds nothing from a contact — it is still reported as the reservation it is.
    pub fn free_antenna(&self, ground_station: &str, window: (f64, f64)) -> Option<String> {
        let held = self.antennas_held(ground_station, window);
        self.placement_antennas(ground_station)
            .into_iter()
            .find(|antenna| !held.contains(antenna.as_str()))
    }

}

/// The azimuth-elevation segments `ephemeris` supplies INLINE for `ground_station`, or
/// `None` when it supplies none for that station — because it is of another type, was
/// supplied for another station, or holds its data in an Amazon S3 object this store cannot
/// read. See [`ComAmazonawsGroundstationGroundstationState::pointing_passes`].
fn az_el_segments_for<'a>(
    ephemeris: &'a Ephemeris,
    ground_station: &str,
) -> Option<&'a AzElSegments> {
    let EphemerisData::azEl(az_el) = &ephemeris.supplied_data else {
        return None;
    };
    if az_el.ground_station != ground_station {
        return None;
    }
    inline_az_el_segments(&ephemeris.supplied_data)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGroundstationGroundstationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGroundstationGroundstationState>)
}
