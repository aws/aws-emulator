// SPDX-License-Identifier: CC0-1.0

//! The Neptune Serverless PLATFORM VERSIONS this control plane publishes.
//!
//! `DescribeServerlessV2PlatformVersions` reports the platform versions Neptune Serverless
//! runs on, and `DBCluster$ServerlessV2PlatformVersion` names the one a cluster runs on.
//! Both answer out of the ONE catalogue below, so the version a cluster reports is always a
//! version the report publishes. It is reference data: no operation of this model takes a
//! platform version as an input, so nothing a caller sends chooses one and no path writes
//! one.

use super::NEPTUNE_ENGINE;
use crate::ops::types::ServerlessV2PlatformVersionInfo;

/// The product whose Serverless platform these versions belong to, as the description of a
/// published version names it.
const NEPTUNE_PRODUCT: &str = "Amazon Neptune";

/// `ServerlessV2PlatformVersionInfo$Status` for a platform version that is in use. The
/// member states both statuses: "enabled - The platform version is in use" and "disabled -
/// The platform version is not in use".
pub const PLATFORM_VERSION_ENABLED: &str = "enabled";

/// `ServerlessV2PlatformVersionInfo$Status` for a platform version that is no longer in
/// use — the rows `DescribeServerlessV2PlatformVersionsMessage$IncludeAll` asks for as well
/// ("whether to also include platform versions which are no longer in use"); see
/// [`PLATFORM_VERSION_ENABLED`].
pub const PLATFORM_VERSION_DISABLED: &str = "disabled";

/// Records the platform-version report includes when the request names no `MaxRecords`.
/// Source: `DescribeServerlessV2PlatformVersionsMessage$MaxRecords` — "Default: 20".
pub const DEFAULT_PLATFORM_VERSION_MAX_RECORDS: i32 = 20;

/// The inclusive bounds this report's `MaxRecords` must fall in — its own, which are wider
/// than the record reports'.
/// Source: `DescribeServerlessV2PlatformVersionsMessage$MaxRecords` — "Constraints: Minimum
/// 1, maximum 200".
pub const PLATFORM_VERSION_MAX_RECORDS_BOUNDS: (i32, i32) = (1, 200);

/// One Neptune Serverless platform version this service publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerlessPlatformVersion {
    /// `ServerlessV2PlatformVersionInfo$ServerlessV2PlatformVersion`, a whole number
    /// (`DescribeServerlessV2PlatformVersionsMessage$ServerlessV2PlatformVersion`:
    /// "Example: 3").
    pub version: &'static str,
    /// Whether this version is still in use, which is the `Status` it reports; see
    /// [`PLATFORM_VERSION_ENABLED`].
    pub in_use: bool,
    /// Whether this is the engine's default platform version, "the version used for new DB
    /// clusters" (`ServerlessV2PlatformVersionInfo$IsDefault`).
    pub is_default: bool,
}

/// The Serverless platform versions this service publishes, OLDEST FIRST — the catalogue's
/// own order, which is the order the report publishes its records in.
///
/// The platform is SERVICE-MANAGED: a version is a whole number, and the number the model's
/// own example names ([`ServerlessPlatformVersion::version`]: "Example: 3") is the one this
/// front door serves now — so it is the version the catalogue designates the default, the
/// one every Serverless cluster it holds runs on, and the lower numbers it counted up from
/// are the versions it has already moved off, published as no longer in use.
pub const SERVERLESS_V2_PLATFORM_VERSIONS: &[ServerlessPlatformVersion] = &[
    platform_version("1", false, false),
    platform_version("2", false, false),
    platform_version("3", true, true),
];

/// One catalogue row, so the table above reads as a table. Columns, in order: the platform
/// version, whether it is still in use, whether it is the engine's default.
const fn platform_version(
    version: &'static str,
    in_use: bool,
    is_default: bool,
) -> ServerlessPlatformVersion {
    ServerlessPlatformVersion {
        version,
        in_use,
        is_default,
    }
}

/// Every published Serverless platform version, oldest first; see
/// [`SERVERLESS_V2_PLATFORM_VERSIONS`].
pub fn serverless_v2_platform_versions() -> impl Iterator<Item = &'static ServerlessPlatformVersion>
{
    SERVERLESS_V2_PLATFORM_VERSIONS.iter()
}

/// The platform version this catalogue designates the engine's default — "the version used
/// for new DB clusters" — or `None` when it designates none.
///
/// It is the one accessor for "which platform does a Serverless cluster run on": the
/// cluster envelope ([`super::cluster_view`]) and the platform-version report answer from
/// here, so neither can name a version the other does not publish.
pub fn default_serverless_v2_platform_version() -> Option<&'static ServerlessPlatformVersion> {
    SERVERLESS_V2_PLATFORM_VERSIONS
        .iter()
        .find(|row| row.is_default)
}

/// `ServerlessV2PlatformVersionInfo$ServerlessV2PlatformVersionDescription` for `version`:
/// "the description of the serverless platform", which names the product's own Serverless
/// platform and the version it describes.
pub fn serverless_platform_version_description(version: &str) -> String {
    format!("{NEPTUNE_PRODUCT} Serverless platform version {version}")
}

/// One published platform version as the modelled `ServerlessV2PlatformVersionInfo`.
///
/// Every member is the ROW's own or derived from it: the description from the version
/// ([`serverless_platform_version_description`]) and the status from whether the version is
/// still in use ([`PLATFORM_VERSION_ENABLED`]).
///
/// `ServerlessV2FeaturesSupport` reports "any Neptune Serverless properties or limits that
/// DIFFER between serverless platform versions"; no limit this emulator publishes differs
/// between the versions of this catalogue, so the member is left absent rather than
/// invented.
pub fn serverless_platform_version_to_shape(
    row: &ServerlessPlatformVersion,
) -> ServerlessV2PlatformVersionInfo {
    ServerlessV2PlatformVersionInfo {
        serverless_v2_platform_version: Some(row.version.to_string()),
        serverless_v2_platform_version_description: Some(serverless_platform_version_description(
            row.version,
        )),
        engine: Some(NEPTUNE_ENGINE.to_string()),
        serverless_v2_features_support: None,
        status: Some(
            match row.in_use {
                true => PLATFORM_VERSION_ENABLED,
                false => PLATFORM_VERSION_DISABLED,
            }
            .to_string(),
        ),
        is_default: Some(row.is_default),
    }
}

/// Apply the platform-version report's own documented `MaxRecords` default and bounds, as a
/// page size; see [`PLATFORM_VERSION_MAX_RECORDS_BOUNDS`].
pub fn resolve_platform_version_max_records(requested: Option<i32>) -> Result<usize, String> {
    let (low, high) = PLATFORM_VERSION_MAX_RECORDS_BOUNDS;
    let value = match requested {
        None => DEFAULT_PLATFORM_VERSION_MAX_RECORDS,
        Some(value) if (low..=high).contains(&value) => value,
        Some(value) => {
            return Err(format!(
                "Invalid value {value} for MaxRecords: must be between {low} and {high}"
            ))
        }
    };
    Ok(value as usize)
}

/// One page of a PUBLISHED CATALOGUE, plus the `Marker` a follow-up call resumes from.
///
/// A catalogue is published in an order of its own — a platform version oldest first —
/// which is not the order of any value in the record, so the token handed back names the
/// POSITION the next page starts at rather than a value out of the record. It is
/// zero-padded so positions compare the way [`super::page`] compares its keys, which is
/// where the paging itself happens: one reading of `Marker` for every report of this
/// service.
pub fn page_catalog<T: Clone>(
    items: &[T],
    marker: Option<&str>,
    max_records: usize,
) -> (Vec<T>, Option<String>) {
    let keys: Vec<String> = (0..items.len()).map(catalog_page_key).collect();
    let (page_keys, next) = super::page(&keys, marker, max_records);
    let page = page_keys
        .iter()
        .filter_map(|key| key.parse::<usize>().ok())
        .filter_map(|position| items.get(position).cloned())
        .collect();
    (page, next)
}

/// The pagination token of the record at `position` in a published catalogue: the position
/// zero-padded to a fixed width, so the tokens of a catalogue order lexically the way their
/// positions order numerically.
fn catalog_page_key(position: usize) -> String {
    format!("{position:09}")
}
