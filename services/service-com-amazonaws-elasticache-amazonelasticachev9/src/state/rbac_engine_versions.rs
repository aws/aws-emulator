// SPDX-License-Identifier: CC0-1.0

//! Which engine version an RBAC record needs: the floor a `User` or a
//! `UserGroup` reports through `MinimumEngineVersion`.
//!
//! RBAC is an engine feature, and each engine offers it from a version of its
//! own: `CreateUser` and `CreateUserGroup` are documented "For Valkey engine
//! version 7.2 onwards and Redis OSS 6.0 to 7.1", so a record listed for an
//! engine cannot be used on a version that engine published before the feature
//! its configuration needs existed. The floor is therefore resolved against the
//! engine-version catalog this service publishes
//! ([`CACHE_ENGINE_VERSIONS`]) rather than stated as one number per feature.

use super::*;

/// The oldest version `engine` publishes that is at least `feature_floor` — the
/// `MinimumEngineVersion` a record listed for `engine` whose configuration needs
/// the feature `feature_floor` arrived in reports.
///
/// The feature floors ([`USER_MIN_ENGINE_VERSION`],
/// [`USER_MIN_ENGINE_VERSION_IAM`]) are stated on the Redis OSS version line,
/// which Valkey's own numbering continues, so one comparison covers both
/// engines: an engine whose oldest published version is already past a floor
/// reports that version (Valkey's first ElastiCache line, 7.2, is past every
/// RBAC floor), and one that published the floor itself reports the floor.
///
/// The versions compare as version numbers rather than as text — see
/// [`version_sort_key`]. An engine that publishes nothing at or above the floor
/// does not offer the feature at all, and there is no published version to name
/// for it, so the floor itself is reported; RBAC is refused for such an engine
/// before a record is ever stored (see [`normalize_access_control_engine`]).
pub fn rbac_minimum_engine_version(engine: &str, feature_floor: &'static str) -> &'static str {
    let floor = version_sort_key(feature_floor);
    CACHE_ENGINE_VERSIONS
        .iter()
        .filter(|row| row.engine == engine && version_sort_key(row.engine_version) >= floor)
        .min_by_key(|row| version_sort_key(row.engine_version))
        .map_or(feature_floor, |row| row.engine_version)
}
