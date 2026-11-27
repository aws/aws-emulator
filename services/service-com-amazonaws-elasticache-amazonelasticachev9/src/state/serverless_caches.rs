// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `ServerlessCache$Status`'s values this slice writes and reads: the lifecycle
/// the member describes ("The current status of the serverless cache"), spelled
/// the way the API puts it on the wire.
///
/// The member is a plain `String` rather than an enum, so the spelling is not
/// modelled, and the wire value a client compares against is LOWER case — the
/// same spelling every other resource of this service reports its lifecycle in
/// ([`STATUS_CREATING`] and its siblings, which the serverless SNAPSHOT already
/// uses). The member's prose lists the values shouted; the wire does not, and a
/// client waiting for `available` is waiting for the wire value.
pub const SERVERLESS_STATUS_CREATING: &str = STATUS_CREATING;
/// See [`SERVERLESS_STATUS_CREATING`]. What a cache answers while ElastiCache
/// works a `ModifyServerlessCache` through
/// ([`ServerlessCacheRecord::begin_modification`]).
pub const SERVERLESS_STATUS_MODIFYING: &str = STATUS_MODIFYING;
/// See [`SERVERLESS_STATUS_CREATING`].
pub const SERVERLESS_STATUS_AVAILABLE: &str = STATUS_AVAILABLE;
/// See [`SERVERLESS_STATUS_CREATING`]. What a cache answers between the
/// `DeleteServerlessCache` that started its deletion and the removal the clock
/// owes it ([`ServerlessCacheRecord::pending_deletion`]).
pub const SERVERLESS_STATUS_DELETING: &str = STATUS_DELETING;

/// How ElastiCache refuses a `CreateServerlessCache` whose name is one the
/// account already holds: `ServerlessCacheAlreadyExistsFault` states only that a
/// cache of that name is already there, naming neither the name the request used
/// nor the cache it collided with — the whole sentence is fixed text.
pub const SERVERLESS_CACHE_ALREADY_EXISTS_MESSAGE: &str = "Serverless Cache already exists";

/// How long a serverless cache stays `CREATING` (or `MODIFYING`) before the
/// clock owes it `AVAILABLE`, and how long one stays `DELETING` before the clock
/// owes its removal — the same span a cache cluster's transitions take; see
/// [`CLUSTER_TRANSITION`] for why it is a chosen, non-zero span.
pub const SERVERLESS_CACHE_TRANSITION: Duration = CLUSTER_TRANSITION;

/// How many serverless caches the store holds before a create is refused with
/// `ServerlessCacheQuotaForCustomerExceededFault`. The emulator's OWN bound on
/// the store, for the reason [`CLUSTER_QUOTA`] gives.
pub const SERVERLESS_CACHE_QUOTA: usize = 150;

/// How many serverless cache snapshots the store holds before a create is
/// refused with `ServerlessCacheSnapshotQuotaExceededFault`. The emulator's OWN
/// bound on the store, for the reason [`CLUSTER_QUOTA`] gives.
pub const SERVERLESS_CACHE_SNAPSHOT_QUOTA: usize = 300;

/// What `ServerlessCacheSnapshot$SnapshotType` reports for a snapshot a
/// `CreateServerlessCacheSnapshot` took.
///
/// The member is "the type of snapshot of serverless cache" and carries no
/// modelled enum, and a snapshot a caller asked for by name is a manual one —
/// the word `Snapshot$SnapshotSource` reports a manually created snapshot under
/// as well ([`SNAPSHOT_SOURCE_MANUAL`]). No automatic snapshot is ever stored,
/// for the reason that constant gives: ElastiCache would take one during the
/// source cache's `DailySnapshotTime`, a day after the cache was created at the
/// earliest.
pub const SERVERLESS_SNAPSHOT_TYPE_MANUAL: &str = SNAPSHOT_SOURCE_MANUAL;

/// The other `ServerlessCacheSnapshot$SnapshotType` ElastiCache reports, for a
/// snapshot it took itself during a cache's `DailySnapshotTime`; see
/// [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`] for why none is ever stored here.
pub const SERVERLESS_SNAPSHOT_TYPE_AUTOMATED: &str = SNAPSHOT_SOURCE_AUTOMATED;

/// The stored `SnapshotType` a `DescribeServerlessCacheSnapshots$SnapshotType`
/// value selects, or the reason it names no type of snapshot.
///
/// The member is "the type of snapshot that is being described" and carries no
/// modelled enum, so the vocabulary is the one the service reports a snapshot's
/// own type under ([`SERVERLESS_SNAPSHOT_TYPE_AUTOMATED`] /
/// [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`]) and nothing else: a value outside it
/// describes no type of snapshot and is a bad parameter value rather than a
/// filter that selects nothing. That is what makes "read a snapshot's
/// `SnapshotType`, then list only the snapshots of that type" a round trip.
/// Matched ignoring ASCII case, as this service's other unconstrained filter
/// vocabularies are (see [`snapshot_source_filter`]): a type is a vocabulary
/// word, not an identity.
pub fn serverless_snapshot_type_filter(value: &str) -> Result<&'static str, String> {
    for reported in [
        SERVERLESS_SNAPSHOT_TYPE_AUTOMATED,
        SERVERLESS_SNAPSHOT_TYPE_MANUAL,
    ] {
        if value.eq_ignore_ascii_case(reported) {
            return Ok(reported);
        }
    }
    Err(format!(
        "Invalid SnapshotType Specified. If specified, should be either \
         '{SERVERLESS_SNAPSHOT_TYPE_AUTOMATED}' or '{SERVERLESS_SNAPSHOT_TYPE_MANUAL}'"
    ))
}

/// The page a `DescribeServerlessCacheSnapshots` serves when the request names
/// no `MaxResults`, and the largest page it serves at all: "The default is 50.
/// The Validation Constraints are a maximum of 50."
/// (`smithy_query(query="doc", shape="DescribeServerlessCacheSnapshotsRequest",
/// member="MaxResults")`).
pub const SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT: i32 = 50;
/// See [`SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT`].
pub const SERVERLESS_SNAPSHOT_MAX_RESULTS_MAX: i32 = 50;

/// The largest `SnapshotRetentionLimit` a serverless cache may carry, as
/// `CreateServerlessCache$SnapshotRetentionLimit` documents it: "The maximum
/// value allowed is 35 days."
pub const MAX_SNAPSHOT_RETENTION_LIMIT: i32 = 35;

/// The `DailySnapshotTime` a serverless cache is created with when the request
/// names none.
///
/// A cache that configures no snapshots still carries a time of day for them —
/// the recorded create answers with one — and ElastiCache assigns it: "If you do
/// not specify this parameter, ElastiCache automatically chooses an appropriate
/// time range" (`SnapshotWindow`). The emulator assigns one FIXED time instead
/// of inventing a per-cache random one, the same way
/// [`DEFAULT_MAINTENANCE_WINDOW`] is chosen, and this is the hour that member's
/// own example (`05:00-09:00`) starts at. It is stored on the record the create
/// writes, so a client reading `ServerlessCache$DailySnapshotTime` back gets
/// what the create answered with; a time the request names itself is its own
/// wall clock's ([`is_daily_snapshot_time`]).
pub const DEFAULT_DAILY_SNAPSHOT_TIME: &str = "05:00";

/// The bounds a `DataStorage` limit must be within, and the value that removes
/// the limit instead.
///
/// The member carries no modelled `@range`; the recorded refusal this repo
/// holds for an out-of-range limit
/// (`issues/.../20261022T193451Z-elasticache-nine-operation-families-answer-notimplemented-instead-of.md`,
/// recorded example "Invalid value for DataStorage limit. Must be between 1 and
/// 5000. You can set the value to zero to remove the limit.") is where these
/// come from.
pub const DATA_STORAGE_MIN_GB: i32 = 1;
/// See [`DATA_STORAGE_MIN_GB`].
pub const DATA_STORAGE_MAX_GB: i32 = 5000;
/// The `CacheUsageLimits` bound value that removes a limit rather than setting
/// one; see [`DATA_STORAGE_MIN_GB`].
pub const USAGE_LIMIT_REMOVED: i32 = 0;

/// What a request naming a `UserGroupId` for a Memcached serverless cache is
/// refused with: a user group is "available for Valkey and Redis OSS only"
/// (`CreateServerlessCache$UserGroupId`), so the combination is incompatible
/// rather than a missing group. One sentence for every handler that raises it.
pub const USER_GROUP_ENGINE_UNSUPPORTED: &str =
    "UserGroupId is only valid for a serverless cache running valkey or redis, not for memcached.";

/// The longest `Description` a serverless cache may carry, as
/// `CreateServerlessCache$Description` documents it: "The maximum length is 255
/// characters." The member carries no modelled `@length`, so the bound is
/// enforced in the handler.
pub const MAX_SERVERLESS_DESCRIPTION_LENGTH: usize = 255;

/// What ElastiCache reports for a resource whose description is BLANK.
///
/// The API normalises a blank description to a single space, whichever resource
/// carries it: a recorded `CreateServerlessCache` that names no `Description` is
/// answered with `" "` (the member documents "The default is NULL, i.e. if no
/// description is provided then an empty string will be returned"), and a
/// recorded `CreateCacheParameterGroup` that sends `Description=''` is reported
/// by `DescribeCacheParameterGroups` as `" "` too. The normalised value is
/// STORED, so a create response and a later describe cannot disagree.
pub const BLANK_DESCRIPTION: &str = " ";

/// See [`BLANK_DESCRIPTION`]: what a serverless cache created without a
/// description reports.
pub const DEFAULT_SERVERLESS_DESCRIPTION: &str = BLANK_DESCRIPTION;

/// The description a resource carries for the value `requested` named: the value
/// itself, byte for byte, or [`BLANK_DESCRIPTION`] when it is blank.
///
/// One helper, so every resource whose description a request SETS — the cache
/// parameter group, the cache subnet group, the replication group and the Global
/// datastore — normalises it the one way the service does, and a client that
/// round-trips descriptions reads back a value the service would answer.
pub fn stored_description(requested: &str) -> String {
    if requested.trim().is_empty() {
        BLANK_DESCRIPTION.to_string()
    } else {
        requested.to_string()
    }
}

/// `ConnectionType`'s values (`smithy_query(query="shape",
/// shape="ConnectionType")`: `vpc | public`), and the default
/// `CreateServerlessCache$ConnectionType` documents: "If not specified,
/// defaults to vpc."
pub const CONNECTION_TYPE_VPC: &str = "vpc";
/// See [`CONNECTION_TYPE_VPC`].
pub const CONNECTION_TYPE_PUBLIC: &str = "public";

/// The engine, and the lowest major version of it, that a `public`
/// `ConnectionType` needs: "Setting this to public requires Valkey 9 or above"
/// (`CreateServerlessCache$ConnectionType`).
pub const PUBLIC_CONNECTION_ENGINE: &str = "valkey";
/// See [`PUBLIC_CONNECTION_ENGINE`].
pub const PUBLIC_CONNECTION_MIN_MAJOR: u32 = 9;

/// The one `IntegrationType` this API offers (`smithy_query(query="shape",
/// shape="IntegrationType")`: `dynamodb`) — "ElastiCache supports DynamoDB
/// integration" (`Integration`). A value outside the enum never reaches a
/// handler: the framework refuses it while the request is validated, the same
/// way it refuses a `ConnectionType` or a `NetworkType` outside theirs.
pub const INTEGRATION_TYPE_DYNAMODB: &str = "dynamodb";

/// `IntegrationStatus`'s values this slice writes and reads (the enum's are
/// `enabled | disabled | enabling | disabling | failed`, from
/// `smithy_query(query="shape", shape="IntegrationStatus")`): an integration the
/// create asked for is brought up while the cache itself is still being created,
/// and reports `enabled` — "the integration is enabled" — once the clock has
/// carried that through.
pub const INTEGRATION_STATUS_ENABLING: &str = "enabling";
/// See [`INTEGRATION_STATUS_ENABLING`].
pub const INTEGRATION_STATUS_ENABLED: &str = "enabled";

/// The engine, and the lowest major version of it, an integration needs:
/// "DynamoDB integration requires the Valkey engine with a major engine version
/// of 9 or above" (`CreateServerlessCache$Integrations`).
pub const INTEGRATION_ENGINE: &str = "valkey";
/// See [`INTEGRATION_ENGINE`].
pub const INTEGRATION_MIN_MAJOR: u32 = 9;

/// What a request asking for an integration without saying which is refused
/// with: `IntegrationRequest$IntegrationType` is "the integration type to
/// enable", so an entry naming none asks for no integration at all, which is a
/// value that member cannot take.
pub const INTEGRATION_TYPE_REQUIRED_MESSAGE: &str =
    "Invalid value for Integrations: an integration must name the IntegrationType to enable.";

/// How a request is refused when the cache would run an `Engine` /
/// `MajorEngineVersion` pair that cannot carry `integration_type`, or `None`
/// when the pair can carry it.
///
/// "DynamoDB integration requires the Valkey engine with a major engine version
/// of 9 or above" (`CreateServerlessCache$Integrations`), so the requirement is
/// the engine and version the cache ends up on rather than anything about the
/// request that asked for it — which is why one statement serves the create that
/// enables an integration and the modify that moves a cache carrying one onto
/// another engine. Worded the way this service words the other capability a pair
/// either has or does not (a `public` `ConnectionType`, see
/// [`PUBLIC_CONNECTION_ENGINE`]): the requirement, then the pair this request
/// would leave the cache on.
pub fn integration_engine_refusal(
    integration_type: &str,
    engine: &str,
    major_engine_version: &str,
) -> Option<String> {
    let major = major_version_number(major_engine_version).unwrap_or(0);
    if engine == INTEGRATION_ENGINE && major >= INTEGRATION_MIN_MAJOR {
        return None;
    }
    Some(format!(
        "Integration {integration_type} requires {INTEGRATION_ENGINE} {INTEGRATION_MIN_MAJOR} or \
         above, not {engine} {major_engine_version}."
    ))
}

/// The `NetworkType` a serverless cache is created with when the request names
/// none: "If not specified, defaults to ipv4, unless all provided subnets are
/// IPv6-only, in which case it defaults to ipv6"
/// (`CreateServerlessCache$NetworkType`). The emulator models no EC2 subnets to
/// read an address family from, so the documented default applies.
pub const DEFAULT_NETWORK_TYPE: &str = "ipv4";

/// `StorageEncryptionType`'s values this slice uses, as
/// `ServerlessCache$StorageEncryptionType` documents them: "Serverless caches
/// are always encrypted at rest. The value is `sse-elasticache` if an
/// ElastiCache service-managed key is used, or `sse-kms` if a customer-managed
/// KMS key is used."
pub const STORAGE_ENCRYPTION_SSE_ELASTICACHE: &str = "sse-elasticache";
/// See [`STORAGE_ENCRYPTION_SSE_ELASTICACHE`].
pub const STORAGE_ENCRYPTION_SSE_KMS: &str = "sse-kms";

/// The Region in which a serverless cache that names no `SubnetIds` is given
/// two default subnets, and the number of default subnets everywhere else, as
/// `ServerlessCache$SubnetIds` documents it: "If no subnet IDs are given and
/// your VPC is in us-west-1, then ElastiCache will select 2 default subnets
/// across AZs in your VPC. For all other Regions, if no subnet IDs are given
/// then ElastiCache will select 3 default subnets across AZs in your default
/// VPC."
pub const TWO_DEFAULT_SUBNET_REGION: &str = "us-west-1";
/// See [`TWO_DEFAULT_SUBNET_REGION`].
pub const DEFAULT_SUBNETS_IN_TWO_SUBNET_REGION: usize = 2;
/// See [`TWO_DEFAULT_SUBNET_REGION`].
pub const DEFAULT_SUBNETS: usize = 3;

/// The page a `DescribeServerlessCaches` serves when the request names no
/// `MaxResults`: "The maximum number of records in the response. … The default
/// is 50." (`smithy_query(query="doc", shape="DescribeServerlessCachesRequest",
/// member="MaxResults")`).
pub const SERVERLESS_MAX_RESULTS_DEFAULT: i32 = 50;

/// The ARN of a serverless cache:
/// `arn:aws:elasticache:<region>:<account>:serverlesscache:<name>`.
pub fn serverless_cache_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("serverlesscache:{name}"))
}

/// The ARN of a serverless cache snapshot:
/// `arn:aws:elasticache:<region>:<account>:serverlesscachesnapshot:<name>`.
pub fn serverless_cache_snapshot_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("serverlesscachesnapshot:{name}"),
    )
}

/// How ElastiCache words `ServerlessCacheSnapshotNotFoundFault` where an
/// operation ACTS on a named snapshot ("this serverless cache snapshot could
/// not be found or does not exist"): the sentence says only that no snapshot is
/// held under the name the request specified, naming neither that name nor
/// anything else about the request — the whole sentence is fixed text, like
/// [`SERVERLESS_CACHE_ALREADY_EXISTS_MESSAGE`].
///
/// One statement, so every operation that acts on a serverless cache snapshot
/// by name — `ExportServerlessCacheSnapshot` and
/// `DeleteServerlessCacheSnapshot` today — words that fault the same
/// way. The READ words it differently, as
/// [`serverless_cache_snapshot_not_described_message`] states, and a COPY whose
/// SOURCE is missing differently again, as
/// [`COPY_SOURCE_SERVERLESS_CACHE_SNAPSHOT_NOT_FOUND_MESSAGE`] states.
pub const SERVERLESS_CACHE_SNAPSHOT_NOT_FOUND_MESSAGE: &str =
    "Serverless cache snapshot with specified name does not exists.";

/// How ElastiCache words the same fault for a `CopyServerlessCacheSnapshot`
/// whose `SourceServerlessCacheSnapshotName` names no snapshot this account
/// holds: `Source serverless cache snapshot does not exist.` — the missing
/// resource is named as the SOURCE of the copy, and nothing of the request is
/// interpolated.
///
/// A copy resolves TWO snapshot names, so the fault says which of them could not
/// be resolved rather than restating the class: the same split this service
/// already makes for the other snapshot it copies, where a taken DESTINATION
/// name is its own sentence ([`COPY_DESTINATION_SNAPSHOT_ALREADY_EXISTS_MESSAGE`])
/// beside the one a snapshot being TAKEN is refused with. The two are observable
/// apart, so one sentence cannot serve both surfaces.
pub const COPY_SOURCE_SERVERLESS_CACHE_SNAPSHOT_NOT_FOUND_MESSAGE: &str =
    "Source serverless cache snapshot does not exist.";

/// How `DescribeServerlessCacheSnapshots` words the same fault: a READ names
/// the snapshot it was asked for, as the fixed text "Serverless Cache Snapshot
/// with name " + the requested name + " not found".
///
/// Each surface words a not-found the way the service words it THERE — the same
/// split this service already makes for a missing cache cluster, where the
/// describe names the resource as the shape spells it and the snapshot creator
/// answers prose ([`cache_cluster_not_found_message`] /
/// [`snapshot_source_cluster_not_found_message`]) — so the sentence a read
/// answers with is stated here rather than shared with the acting operations.
/// The name is taken from the REQUEST, which is the only place it exists: the
/// store holds no snapshot under it.
pub fn serverless_cache_snapshot_not_described_message(requested: &str) -> String {
    format!("Serverless Cache Snapshot with name {requested} not found")
}

/// How many default subnets a serverless cache created in `region` without
/// `SubnetIds` is given; see [`TWO_DEFAULT_SUBNET_REGION`].
pub fn default_subnet_count(region: &str) -> usize {
    if region == TWO_DEFAULT_SUBNET_REGION {
        DEFAULT_SUBNETS_IN_TWO_SUBNET_REGION
    } else {
        DEFAULT_SUBNETS
    }
}

/// The major engine version a serverless cache is created with when the request
/// names none: the major line of the engine's current version, which is the one
/// statement this emulator makes about what that version is (see
/// [`default_engine_version`]).
///
/// A line is the engine's version up to the component the parameter-group family
/// is named for — `<major>` for Valkey and Redis OSS, `<major>.<minor>` for
/// Memcached — exactly as [`default_parameter_group_name`] splits it.
pub fn default_major_engine_version(engine: &str) -> String {
    let version = default_engine_version(engine);
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or_default();
    if engine == "memcached" {
        format!("{major}.{}", parts.next().unwrap_or("0"))
    } else {
        major.to_string()
    }
}

/// The full engine version a serverless cache compatible with major version
/// `major` runs, reported as `ServerlessCache$FullEngineVersion` ("The name and
/// version number of the engine the serverless cache is compatible with").
///
/// `DescribeCacheEngineVersions` is still a scaffolded 501, so the emulator's
/// own statement of the current version of each engine
/// ([`default_engine_version`]) is what a request for that engine's current
/// major line resolves to; any other line resolves to its first release.
pub fn serverless_full_engine_version(engine: &str, major: &str) -> String {
    let current = default_engine_version(engine);
    if current == major || current.starts_with(&format!("{major}.")) {
        current.to_string()
    } else {
        format!("{major}.0")
    }
}

/// True when `value` has the form of an engine version: dot-separated groups of
/// digits (`7`, `1.6`, `8.0`).
///
/// `MajorEngineVersion` carries no modelled constraint, so the boundary
/// validator passes any string and a value that cannot name a version is
/// refused here.
pub fn is_version_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The leading (major) component of a version label, or `None` when `value` is
/// not one.
pub fn major_version_number(value: &str) -> Option<u32> {
    value.split('.').next()?.parse().ok()
}

/// True when `value` is a daily snapshot time: `HH:MM` in UTC, the format
/// `DailySnapshotTime` takes ("The daily time that snapshots will be created
/// from the new serverless cache", the same `HH:MM` wall-clock form
/// `SnapshotWindow`'s "Example: 05:00-09:00" is built from).
pub fn is_daily_snapshot_time(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    let two_digits = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit());
    if !two_digits(hours) || !two_digits(minutes) {
        return false;
    }
    matches!(hours.parse::<u32>(), Ok(h) if h < 24)
        && matches!(minutes.parse::<u32>(), Ok(m) if m < 60)
}

/// Why a serverless cache cannot carry the `Description` a request named, or
/// the description it carries for that value.
///
/// The member carries no modelled `@length`, so the documented bound
/// ([`MAX_SERVERLESS_DESCRIPTION_LENGTH`]) is enforced here and a longer value
/// is refused rather than stored truncated; a blank one is the empty
/// description the API reports as [`DEFAULT_SERVERLESS_DESCRIPTION`]. One
/// implementation, so a created and a modified cache describe the same values.
pub fn serverless_description(requested: &str) -> Result<String, String> {
    let length = requested.chars().count();
    if length > MAX_SERVERLESS_DESCRIPTION_LENGTH {
        return Err(format!(
            "Invalid value for Description: the maximum length is \
             {MAX_SERVERLESS_DESCRIPTION_LENGTH} characters, got {length}."
        ));
    }
    Ok(stored_description(requested))
}

/// Why a serverless cache cannot carry the `SnapshotRetentionLimit` a request
/// named, or `None` when it may.
///
/// "The maximum value allowed is 35 days" ([`MAX_SNAPSHOT_RETENTION_LIMIT`]),
/// and a negative retention retains nothing: a limit outside that range is
/// refused rather than clamped. The sentence states the bound the service
/// publishes, and its one variable part is the rejected value from the request.
pub fn snapshot_retention_limit_refusal(limit: i32) -> Option<String> {
    (!(0..=MAX_SNAPSHOT_RETENTION_LIMIT).contains(&limit)).then(|| {
        format!(
            "Invalid snapshot retention limit: {limit}. Snapshot retention limit must be between \
             0 and {MAX_SNAPSHOT_RETENTION_LIMIT}."
        )
    })
}

/// Why a serverless cache cannot carry the `DailySnapshotTime` a request named,
/// or `None` when it may; see [`is_daily_snapshot_time`].
pub fn daily_snapshot_time_refusal(requested: &str) -> Option<String> {
    (!is_daily_snapshot_time(requested)).then(|| {
        format!(
            "Invalid value {requested} for DailySnapshotTime: must be a daily time in the format \
             HH:MM (UTC)."
        )
    })
}

/// Why the value a request named cannot be read as a `MajorEngineVersion` AT
/// ALL, or `None` when it names a version; see [`is_version_label`].
///
/// A value that names no version is a bad value of that one member. A value that
/// IS a version but one the engine does not run is a different refusal — see
/// [`unsupported_major_engine_version`].
pub fn major_engine_version_refusal(engine: &str, requested: &str) -> Option<String> {
    (!is_version_label(requested)).then(|| {
        format!(
            "Invalid value {requested} for MajorEngineVersion: must be a version such as {}.",
            default_major_engine_version(engine)
        )
    })
}

/// Why a serverless cache cannot run the major engine version `requested` of
/// `engine`, or `None` when that engine offers it.
///
/// WHICH versions an engine offers is the published engine-version catalog's
/// answer ([`version_is_offered`], the same catalog
/// `DescribeCacheEngineVersions` reports and a cluster's `EngineVersion` is held
/// against): `MajorEngineVersion` carries no modelled enum or pattern, so the set
/// of versions is enforced server-side, and a request naming a major line the
/// catalog publishes no version of asks for an `Engine` and a
/// `MajorEngineVersion` that do not go together rather than for a malformed
/// value. That is why the refusal is the INCOMPATIBLE-COMBINATION one ("Two or
/// more incompatible parameters were specified") and why its sentence names the
/// engine's support for the version rather than the member.
///
/// The one variable part of the sentence is the version the request named; the
/// rest is fixed text. One statement, so the create and the modify of a
/// serverless cache refuse the same versions with the same words.
pub fn unsupported_major_engine_version(engine: &str, requested: &str) -> Option<String> {
    (!version_is_offered(engine, requested)).then(|| {
        format!("Specified engine does not support major engine version ({requested}).")
    })
}

/// The `CacheUsageLimits` a cache carries after a request that named
/// `requested`, over the `stored` limits it carries now.
///
/// A limit is a BOUND rather than a document: each bound the request names
/// replaces the stored one, and a bound it does not name keeps the value the
/// cache is running — which is why the API publishes [`USAGE_LIMIT_REMOVED`] as
/// the way to remove a bound rather than treating an omitted member as a
/// removal. The result is what [`validate_cache_usage_limits`] is checked
/// against, so a request that lifts one bound past the other's stored value is
/// refused on the limits the cache would END UP with.
pub fn merge_cache_usage_limits(
    stored: Option<&CacheUsageLimits>,
    requested: &CacheUsageLimits,
) -> CacheUsageLimits {
    let stored_storage = stored.and_then(|limits| limits.data_storage.as_ref());
    let stored_ecpu = stored.and_then(|limits| limits.ecpu_per_second.as_ref());
    CacheUsageLimits {
        data_storage: match (requested.data_storage.as_ref(), stored_storage) {
            (Some(wanted), stored) => Some(DataStorage {
                minimum: wanted.minimum.or_else(|| stored.and_then(|s| s.minimum)),
                maximum: wanted.maximum.or_else(|| stored.and_then(|s| s.maximum)),
                // `DataStorage$Unit` is required and has one value (`GB`), so a
                // named member always carries it; an empty one keeps the unit
                // the cache is measured in.
                unit: if wanted.unit.is_empty() {
                    stored.map(|s| s.unit.clone()).unwrap_or_default()
                } else {
                    wanted.unit.clone()
                },
            }),
            (None, stored) => stored.cloned(),
        },
        ecpu_per_second: match (requested.ecpu_per_second.as_ref(), stored_ecpu) {
            (Some(wanted), stored) => Some(ECPUPerSecond {
                minimum: wanted.minimum.or_else(|| stored.and_then(|s| s.minimum)),
                maximum: wanted.maximum.or_else(|| stored.and_then(|s| s.maximum)),
            }),
            (None, stored) => stored.cloned(),
        },
    }
}

/// One `CacheUsageLimits` member as [`validate_cache_usage_limits`] checks it:
/// its name, its `Minimum`, its `Maximum`, and the upper bound its values must
/// be within (`None` where the API publishes none).
pub(super) type UsageLimitBounds = (&'static str, Option<i32>, Option<i32>, Option<i32>);

/// Why a request's `CacheUsageLimits` cannot be stored, as the message the
/// refusal carries, or `Ok(())` when every limit is one this store may hold.
///
/// `DataStorage` / `ECPUPerSecond` carry no modelled `@range`, so the boundary
/// validator passes any integer and the bounds are enforced here: a
/// `DataStorage` bound must be within [`DATA_STORAGE_MIN_GB`] ..=
/// [`DATA_STORAGE_MAX_GB`], an `ECPUPerSecond` bound must be positive (the API
/// publishes no upper bound for it, so none is enforced), and either member
/// takes [`USAGE_LIMIT_REMOVED`] to remove the limit instead of setting one. A
/// `Minimum` above its own `Maximum` is refused for both. Every accepted value
/// is persisted EXACTLY as sent — the stored limits are what a later read
/// reports.
pub fn validate_cache_usage_limits(limits: &CacheUsageLimits) -> Result<(), String> {
    let mut members: Vec<UsageLimitBounds> = Vec::new();
    if let Some(storage) = limits.data_storage.as_ref() {
        members.push((
            "DataStorage",
            storage.minimum,
            storage.maximum,
            Some(DATA_STORAGE_MAX_GB),
        ));
    }
    if let Some(ecpu) = limits.ecpu_per_second.as_ref() {
        members.push(("ECPUPerSecond", ecpu.minimum, ecpu.maximum, None));
    }
    for (member, minimum, maximum, upper) in members {
        for value in [minimum, maximum].into_iter().flatten() {
            if value == USAGE_LIMIT_REMOVED {
                continue;
            }
            let within = value >= DATA_STORAGE_MIN_GB && upper.is_none_or(|upper| value <= upper);
            if !within {
                // The template states the RULE the value broke — the member, its
                // bounds and the value that removes the limit — and carries
                // nothing of the value that broke it.
                return Err(match upper {
                    Some(upper) => format!(
                        "Invalid value for {member} limit. Must be between \
                         {DATA_STORAGE_MIN_GB} and {upper}. You can set the value to zero to \
                         remove the limit."
                    ),
                    None => format!(
                        "Invalid value for {member} limit. Must be at least \
                         {DATA_STORAGE_MIN_GB}. You can set the value to zero to remove the \
                         limit."
                    ),
                });
            }
        }
        // A removed bound bounds nothing, so it cannot be the inverted one.
        if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
            if minimum != USAGE_LIMIT_REMOVED
                && maximum != USAGE_LIMIT_REMOVED
                && minimum > maximum
            {
                return Err(format!(
                    "Invalid values for {member}. Minimum must not be greater than Maximum."
                ));
            }
        }
    }
    Ok(())
}

/// One serverless cache, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ServerlessCacheRecord {
    /// The (lowercased) `ServerlessCacheName`, which "is stored as a lowercase
    /// string" and is the resource's identity in this API.
    pub name: String,
    pub region: String,
    /// What `ServerlessCache$Description` reports; see
    /// [`DEFAULT_SERVERLESS_DESCRIPTION`].
    pub description: String,
    pub engine: String,
    pub major_engine_version: String,
    pub full_engine_version: String,
    /// One of `ServerlessCache$Status`'s values.
    pub status: String,
    /// The status change the clock owes this cache, if any.
    pub pending_status: Option<PendingTransition>,
    /// The instant the clock owes this cache's REMOVAL from the store, set by
    /// the `DeleteServerlessCache` that put it in `DELETING`. A deletion is not
    /// instant at AWS either — the call answers with the cache reporting
    /// `DELETING` — so the record outlives the call by
    /// [`SERVERLESS_CACHE_TRANSITION`] and is swept by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_caches`].
    pub pending_deletion: Option<SystemTime>,
    pub created_at: SystemTime,
    /// The usage limits as the request set them, persisted unchanged.
    pub cache_usage_limits: Option<CacheUsageLimits>,
    pub kms_key_id: Option<String>,
    /// `sse-kms` for a customer-managed key, else `sse-elasticache`; see
    /// [`STORAGE_ENCRYPTION_SSE_ELASTICACHE`].
    pub storage_encryption_type: String,
    /// The VPC security groups that guard the cache's endpoint: the ones the
    /// request named, or — "if no other information is given this will be the
    /// VPC's Default Security Group that is associated with the cluster VPC
    /// end-point" (`CreateServerlessCache$SecurityGroupIds`) — the default group
    /// of the VPC the cache is placed in, as EC2 reports it.
    pub security_group_ids: Vec<String>,
    /// The subnets the cache's VPC endpoint is deployed in: the ones the request
    /// named, or the defaults ElastiCache selects; see [`DEFAULT_SUBNETS`].
    pub subnet_ids: Vec<String>,
    pub user_group_id: Option<String>,
    /// The cache's snapshot posture, as the create stored it: the retention the
    /// request named or [`DEFAULT_SNAPSHOT_RETENTION_LIMIT`].
    pub snapshot_retention_limit: Option<i32>,
    /// See [`snapshot_retention_limit`](Self::snapshot_retention_limit): the
    /// daily time the request named, or [`DEFAULT_DAILY_SNAPSHOT_TIME`].
    pub daily_snapshot_time: Option<String>,
    pub network_type: String,
    pub connection_type: String,
    pub endpoint_address: String,
    pub reader_endpoint_address: String,
    pub endpoint_port: i32,
    /// The port the cache's READER endpoint answers on, which is its own socket's
    /// ([`crate::state::serve_serverless_cache`]) and the engine's client port for a cache
    /// this emulator serves no data plane for.
    pub reader_endpoint_port: i32,
    pub arn: String,
    /// The tags `CreateServerlessCache` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// The open data migration INTO this cache, which is the one copy
    /// `ServerlessCache$Migration` reports and every migration operation reads;
    /// see [`MigrationRecord`]. `None` until a migration is opened on it.
    pub migration: Option<MigrationRecord>,
    /// The integrations configured on this cache, which is the one copy
    /// `ServerlessCache$Integrations` ("configured integrations and their
    /// status") reports; see [`IntegrationRecord`]. Empty for a cache created
    /// without any: "if not specified, no integrations are enabled"
    /// (`CreateServerlessCache$Integrations`).
    pub integrations: Vec<IntegrationRecord>,
}

/// One integration configured on a serverless cache, as this emulator persists
/// it: "an integration configured on the serverless cache, and its current
/// status" (`Integration`).
#[derive(Debug, Clone)]
pub struct IntegrationRecord {
    /// The `IntegrationType` this integration is of
    /// ([`INTEGRATION_TYPE_DYNAMODB`]). It is the integration's identity on the
    /// cache — one status per type — so a request naming a type twice configures
    /// the ONE integration of that type.
    pub integration_type: String,
    /// One of `IntegrationStatus`'s values; see
    /// [`INTEGRATION_STATUS_ENABLING`].
    pub status: String,
    /// The status change the clock owes this integration, if any.
    pub pending_status: Option<PendingTransition>,
}

impl IntegrationRecord {
    /// The integration a `CreateServerlessCache` asked to enable.
    ///
    /// Enabling one is work ElastiCache carries out after the call answers, so
    /// the call that asked for it answers [`INTEGRATION_STATUS_ENABLING`] and the
    /// clock owes it [`INTEGRATION_STATUS_ENABLED`] once
    /// [`SERVERLESS_CACHE_TRANSITION`] — the span the cache's own creation takes
    /// — has passed.
    pub fn enabling(integration_type: String, now: SystemTime) -> Self {
        Self {
            integration_type,
            status: INTEGRATION_STATUS_ENABLING.to_string(),
            pending_status: Some(PendingTransition {
                due: now + SERVERLESS_CACHE_TRANSITION,
                status: INTEGRATION_STATUS_ENABLED.to_string(),
            }),
        }
    }

    /// Materialise the status change the clock owes this integration, if `now`
    /// is past its due instant; see [`ServerlessCacheRecord::settle`], which
    /// drives it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending_status = None;
    }

    /// This record as the modelled `Integration`.
    pub fn to_modeled(&self) -> Integration {
        Integration {
            integration_type: Some(self.integration_type.clone()),
            status: Some(self.status.clone()),
        }
    }
}

impl ServerlessCacheRecord {
    /// The endpoint address clients write to, and the one they read from.
    ///
    /// A serverless cache is reached through one DNS name per role rather than
    /// per node, so the name is the cache, this cache's stored DNS token, the
    /// `serverless` label, the Region and the service's DNS suffix — the same
    /// suffix [`CacheClusterRecord::configuration_endpoint_address`] builds. The
    /// reader endpoint is the same name with the `-ro` (read-only) suffix.
    pub(super) fn endpoint_addresses(name: &str, dns_token: &str, region: &str) -> (String, String) {
        (
            format!("{name}-{dns_token}.serverless.{region}.cache.amazonaws.com"),
            format!("{name}-{dns_token}-ro.serverless.{region}.cache.amazonaws.com"),
        )
    }

    /// Materialise the status change the clock owes this cache, if `now` is past
    /// its due instant, and hand back the change it made; see
    /// [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        // An integration's own clock-owed change: the `enabled` ElastiCache owes
        // one it was asked to enable ([`IntegrationRecord::enabling`]). It is not
        // the cache's status, so it falls due whether or not the cache itself is
        // owed a transition — and it records no event, because an integration is
        // not one of the sources this service reports events under (`SourceType`
        // has no member for one).
        for integration in &mut self.integrations {
            integration.settle(now);
        }
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// Put this cache into `MODIFYING` and record the `AVAILABLE` the clock now
    /// owes it, [`SERVERLESS_CACHE_TRANSITION`] from `now`.
    ///
    /// Scaling a cache's capacity, and moving it onto another engine or engine
    /// version, is work ElastiCache carries out after the call answers, so the
    /// call that asks for it answers with the cache `MODIFYING` and the clock owes
    /// it `AVAILABLE` from that call.
    pub fn begin_modification(&mut self, now: SystemTime) {
        self.status = SERVERLESS_STATUS_MODIFYING.to_string();
        self.pending_status = Some(PendingTransition {
            due: now + SERVERLESS_CACHE_TRANSITION,
            status: SERVERLESS_STATUS_AVAILABLE.to_string(),
        });
    }

    /// Put this cache into `DELETING` and record the removal the clock now owes
    /// it, [`SERVERLESS_CACHE_TRANSITION`] from `now`.
    ///
    /// The status change a delete makes is immediate (the call answers with the
    /// cache reporting `DELETING`), so it is NOT a pending transition; what the
    /// clock owes is the record's disappearance, which
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_caches`]
    /// materialises. Any status change that was still pending is dropped: a
    /// cache on its way out is not owed `AVAILABLE` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = SERVERLESS_STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_deletion = Some(now + SERVERLESS_CACHE_TRANSITION);
    }

    /// This record as the modelled `ServerlessCache`.
    pub fn to_modeled(&self) -> ServerlessCache {
        ServerlessCache {
            serverless_cache_name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            major_engine_version: Some(self.major_engine_version.clone()),
            full_engine_version: Some(self.full_engine_version.clone()),
            cache_usage_limits: self.cache_usage_limits.clone(),
            kms_key_id: self.kms_key_id.clone(),
            storage_encryption_type: Some(self.storage_encryption_type.clone()),
            security_group_ids: (!self.security_group_ids.is_empty())
                .then(|| self.security_group_ids.clone()),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            reader_endpoint: Some(Endpoint {
                address: Some(self.reader_endpoint_address.clone()),
                port: Some(self.reader_endpoint_port),
            }),
            arn: Some(self.arn.clone()),
            user_group_id: self.user_group_id.clone(),
            subnet_ids: Some(self.subnet_ids.clone()),
            snapshot_retention_limit: self.snapshot_retention_limit,
            daily_snapshot_time: self.daily_snapshot_time.clone(),
            network_type: Some(self.network_type.clone()),
            connection_type: Some(self.connection_type.clone()),
            // "Provides a summary of the migration status for the serverless
            // cache": the open migration this cache carries, if one was opened
            // on it. Its endpoint is "the standard endpoint of the serverless
            // cache", which is the one this record already holds.
            migration: self.migration.as_ref().map(|migration| {
                migration.to_modeled(Endpoint {
                    address: Some(self.endpoint_address.clone()),
                    port: Some(self.endpoint_port),
                })
            }),
            // "Configured integrations and their status": the integrations this
            // cache carries, each with the status it has reached. A cache
            // carrying none reports no list at all, the way it reports no
            // migration — "if not specified, no integrations are enabled".
            integrations: (!self.integrations.is_empty()).then(|| {
                self.integrations
                    .iter()
                    .map(IntegrationRecord::to_modeled)
                    .collect()
            }),
        }
    }
}

/// One serverless cache snapshot, as this emulator persists it: the source
/// cache's configuration as it stood when the snapshot was taken, plus the
/// snapshot's own identity and status.
///
/// "This API creates a copy of an entire ServerlessCache at a specific moment in
/// time", so the configuration below is COPIED out of the source cache record
/// when the snapshot is taken: a later `ModifyServerlessCache` cannot change
/// what a snapshot already taken reports, and a `DeleteServerlessCache` cannot
/// take it away with the cache.
#[derive(Debug, Clone)]
pub struct ServerlessCacheSnapshotRecord {
    /// The snapshot's identity, lowercased —
    /// `CreateServerlessCacheSnapshot$ServerlessCacheSnapshotName` "is stored as
    /// a lowercase string".
    pub name: String,
    pub arn: String,
    /// "The ID of the Amazon Web Services Key Management Service (KMS) key of a
    /// serverless cache snapshot", resolved to the key's own ARN when the
    /// request named one and otherwise the source cache's key.
    pub kms_key_id: Option<String>,
    /// [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`].
    pub snapshot_type: String,
    /// The snapshot's current status, spelled as [`STATUS_CREATING`] /
    /// [`STATUS_AVAILABLE`] do.
    pub status: String,
    /// The status change the clock owes this snapshot, if any.
    pub pending_status: Option<PendingTransition>,
    /// The instant the clock owes this snapshot's REMOVAL from the store, set by
    /// the `DeleteServerlessCacheSnapshot` that put it into [`STATUS_DELETING`].
    /// A deletion is not instant: the call answers with the snapshot reporting
    /// `deleting`, so the record outlives the call by [`SNAPSHOT_TRANSITION`] and
    /// is swept by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_cache_snapshots`] —
    /// the same posture [`SnapshotRecord::pending_removal`] holds for a cluster's
    /// snapshot. `None` for a snapshot nothing has deleted.
    pub pending_removal: Option<SystemTime>,
    /// "The date and time that the source serverless cache's metadata and cache
    /// data set was obtained for the snapshot."
    pub created_at: SystemTime,
    /// The source cache's name, engine and major engine version, which together
    /// are the `ServerlessCacheConfiguration` the snapshot reports.
    pub serverless_cache_name: String,
    pub engine: String,
    pub major_engine_version: String,
    /// The source cache's DATA SET as it stood when the snapshot was taken — the
    /// "cache data set [that] was obtained for the snapshot"
    /// (`ServerlessCacheSnapshot$CreateTime`), which is what makes this a copy of
    /// the cache rather than a note that one was taken.
    ///
    /// It is the snapshot's OWN copy ([`copied_keyspace`]), so a later write to
    /// the source cache does not reach it and a `DeleteServerlessCache` cannot
    /// take it away with the cache — which is what lets a cache created from this
    /// snapshot answer what was stored before it, and what
    /// `ServerlessCacheSnapshot$BytesUsedForCache` reports the size of.
    pub data: SharedKeyspace,
    /// The tags the create was given. The ONE copy of this snapshot's tags; see
    /// [`SnapshotRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl ServerlessCacheSnapshotRecord {
    /// A snapshot of `cache` as it stands now.
    ///
    /// A created snapshot is NOT ready: the call that took it answers
    /// [`STATUS_CREATING`], and the clock owes it [`STATUS_AVAILABLE`] once
    /// [`SNAPSHOT_TRANSITION`] — the span of that call — has passed.
    pub fn from_cache(cache: &ServerlessCacheRecord, new: NewServerlessCacheSnapshot) -> Self {
        Self {
            arn: serverless_cache_snapshot_arn(&cache.region, &new.name),
            name: new.name,
            // "The ID of the KMS key used to encrypt the snapshot. Default:
            // NULL": a snapshot of a cache encrypted with a customer managed key
            // keeps that key when the request names none, the same way
            // [`SnapshotRecord::copy_to`] keeps the source snapshot's.
            kms_key_id: new.kms_key_id.or_else(|| cache.kms_key_id.clone()),
            snapshot_type: SERVERLESS_SNAPSHOT_TYPE_MANUAL.to_string(),
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            // Nothing is owed the removal of a snapshot just taken.
            pending_removal: None,
            created_at: new.now,
            serverless_cache_name: cache.name.clone(),
            engine: cache.engine.clone(),
            major_engine_version: cache.major_engine_version.clone(),
            data: new.data,
            tags: new.tags,
        }
    }

    /// A copy of this snapshot stored under another name.
    ///
    /// A copy is "a copy of an existing serverless cache's snapshot", so
    /// everything that describes WHAT was captured — the source cache's name,
    /// engine and major engine version, the instant the data set was obtained at
    /// and the data set itself — is carried over unchanged rather than re-read
    /// from the live cache or re-stamped with the copy's own clock: copying does
    /// not retake the snapshot. Only the copy's own identity (name and ARN), its
    /// fresh status and the members the copy request itself carries (KMS key,
    /// tags) differ — the same split [`SnapshotRecord::copy_to`] makes for the
    /// other kind of snapshot this service copies.
    pub fn copy_to(&self, new: ServerlessCacheSnapshotCopy) -> ServerlessCacheSnapshotRecord {
        ServerlessCacheSnapshotRecord {
            arn: serverless_cache_snapshot_arn(&new.region, &new.target_name),
            name: new.target_name,
            // "The identifier of the KMS key used to encrypt the target
            // snapshot": the copy keeps the source's key when the request names
            // none, the way a snapshot taken of a cache keeps the cache's.
            kms_key_id: new.kms_key_id.or_else(|| self.kms_key_id.clone()),
            // A copy a caller asked for by name is that caller's own manual
            // snapshot, whatever the source's type was.
            snapshot_type: SERVERLESS_SNAPSHOT_TYPE_MANUAL.to_string(),
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            // The copy is its own resource: whatever the source's deletion
            // owes, nothing is owed on the record stored under the new name.
            pending_removal: None,
            // `Tags` are "a list of tags to be added to the target snapshot
            // resource" — the copy's own, not the source's.
            tags: new.tags,
            // The copy holds the data set the SOURCE captured, as a copy of its
            // own: two snapshots are two resources, so neither one's data set is
            // shared with the other.
            data: copied_keyspace(&self.data, new.now),
            ..self.clone()
        }
    }

    /// Materialise the status change the clock owes this snapshot, if `now` is
    /// past its due instant, and hand back the change it made. The one write a
    /// reporting operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// Put this snapshot into [`STATUS_DELETING`] and let the clock owe its
    /// removal [`SNAPSHOT_TRANSITION`] later.
    ///
    /// `DeleteServerlessCacheSnapshot` "deletes an existing serverless cache
    /// snapshot" and answers WITH "the snapshot to be deleted"
    /// (`DeleteServerlessCacheSnapshotResponse$ServerlessCacheSnapshot`), so the
    /// record is still there when the call answers, reporting `deleting`, and the
    /// removal is what the clock materialises
    /// ([`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_cache_snapshots`]) —
    /// the same way [`SnapshotRecord::begin_deletion`] leaves a cluster's
    /// snapshot. Any status change that was still pending is dropped: a snapshot
    /// on its way out is not owed `available` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_removal = Some(now + SNAPSHOT_TRANSITION);
    }

    /// This record as the modelled `ServerlessCacheSnapshot`.
    pub fn to_modeled(&self) -> ServerlessCacheSnapshot {
        ServerlessCacheSnapshot {
            serverless_cache_snapshot_name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            kms_key_id: self.kms_key_id.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            status: Some(self.status.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // "The time that the serverless cache snapshot will expire": only a
            // snapshot ElastiCache took automatically is kept for a retention
            // limit and so has an instant it expires at, and none is ever stored
            // (see [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`]).
            expiry_time: None,
            // "The total size of a serverless cache snapshot, in bytes": the
            // bytes the data set this snapshot captured holds, read off that
            // copy rather than invented. A snapshot of an empty cache reports
            // `0`, which is its size.
            bytes_used_for_cache: Some(lock_keyspace(&self.data).used_bytes().to_string()),
            serverless_cache_configuration: Some(ServerlessCacheConfiguration {
                serverless_cache_name: Some(self.serverless_cache_name.clone()),
                engine: Some(self.engine.clone()),
                major_engine_version: Some(self.major_engine_version.clone()),
            }),
        }
    }
}

/// The validated values a new serverless cache snapshot is built from; see
/// [`ServerlessCacheSnapshotRecord::from_cache`].
#[derive(Debug, Clone)]
pub struct NewServerlessCacheSnapshot {
    /// The stored (lowercased) `ServerlessCacheSnapshotName`.
    pub name: String,
    pub kms_key_id: Option<String>,
    /// The source cache's data set, captured at this instant
    /// ([`captured_serverless_cache_data`]).
    pub data: SharedKeyspace,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// The values a copy of a serverless cache snapshot is stored under; see
/// [`ServerlessCacheSnapshotRecord::copy_to`].
#[derive(Debug, Clone)]
pub struct ServerlessCacheSnapshotCopy {
    /// The stored (lowercased) `TargetServerlessCacheSnapshotName`.
    pub target_name: String,
    /// The Region the copy is minted in, which its ARN names.
    pub region: String,
    /// The `KmsKeyId` the copy request named, resolved to the key's own ARN.
    pub kms_key_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// Why a serverless cache snapshot cannot be stored under a name, with the
/// sentence it is refused in; see
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::serverless_cache_snapshot_refusal`].
///
/// The kind decides which modelled failure the MINTING operation lifts it into:
/// `CreateServerlessCacheSnapshot` models a fault for each, while
/// `DeleteServerlessCache` models only the already-exists one, so its
/// `FinalSnapshotName` reports the bound as the value it cannot accept.
#[derive(Debug, Clone)]
pub enum ServerlessSnapshotRefusal {
    /// A name another snapshot already holds — "must be unique for the customer
    /// account".
    AlreadyExists(String),
    /// The store is at [`SERVERLESS_CACHE_SNAPSHOT_QUOTA`].
    QuotaExceeded(String),
}
