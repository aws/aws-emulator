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

/// How ElastiCache words `ServerlessCacheSnapshotNotFoundFault` ("this
/// serverless cache snapshot could not be found or does not exist"): the
/// sentence says only that no snapshot is held under the name the request
/// specified, naming neither that name nor anything else about the request —
/// the whole sentence is fixed text, like
/// [`SERVERLESS_CACHE_ALREADY_EXISTS_MESSAGE`].
///
/// One statement, so every operation that resolves a serverless cache snapshot
/// by name — `DescribeServerlessCacheSnapshots` and
/// `ExportServerlessCacheSnapshot` — words that fault the same way.
pub const SERVERLESS_CACHE_SNAPSHOT_NOT_FOUND_MESSAGE: &str =
    "Serverless cache snapshot with specified name does not exists.";

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
/// refused rather than clamped.
pub fn snapshot_retention_limit_refusal(limit: i32) -> Option<String> {
    (!(0..=MAX_SNAPSHOT_RETENTION_LIMIT).contains(&limit)).then(|| {
        format!(
            "Invalid value {limit} for SnapshotRetentionLimit: must be at least 0 and no more \
             than {MAX_SNAPSHOT_RETENTION_LIMIT} days."
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

/// Why a serverless cache running `engine` cannot carry the
/// `MajorEngineVersion` a request named, or `None` when it may; see
/// [`is_version_label`].
pub fn major_engine_version_refusal(engine: &str, requested: &str) -> Option<String> {
    (!is_version_label(requested)).then(|| {
        format!(
            "Invalid value {requested} for MajorEngineVersion: must be a version such as {}.",
            default_major_engine_version(engine)
        )
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
    pub security_group_ids: Vec<String>,
    /// The subnets the cache's VPC endpoint is deployed in: the ones the request
    /// named, or the defaults ElastiCache selects; see [`DEFAULT_SUBNETS`].
    pub subnet_ids: Vec<String>,
    pub user_group_id: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub daily_snapshot_time: Option<String>,
    pub network_type: String,
    pub connection_type: String,
    pub endpoint_address: String,
    pub reader_endpoint_address: String,
    pub endpoint_port: i32,
    pub arn: String,
    /// The tags `CreateServerlessCache` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// The open data migration INTO this cache, which is the one copy
    /// `ServerlessCache$Migration` reports and every migration operation reads;
    /// see [`MigrationRecord`]. `None` until a migration is opened on it.
    pub migration: Option<MigrationRecord>,
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
    /// cache is `MODIFYING` from here and a caller reading it straight after
    /// still sees that status and the state errors that go with it.
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
                port: Some(self.endpoint_port),
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
    /// "The date and time that the source serverless cache's metadata and cache
    /// data set was obtained for the snapshot."
    pub created_at: SystemTime,
    /// The source cache's name, engine and major engine version, which together
    /// are the `ServerlessCacheConfiguration` the snapshot reports.
    pub serverless_cache_name: String,
    pub engine: String,
    pub major_engine_version: String,
    /// The tags the create was given. The ONE copy of this snapshot's tags; see
    /// [`SnapshotRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl ServerlessCacheSnapshotRecord {
    /// A snapshot of `cache` as it stands now.
    ///
    /// A created snapshot is NOT ready: it answers [`STATUS_CREATING`], and the
    /// clock owes it [`STATUS_AVAILABLE`] once [`SNAPSHOT_TRANSITION`] has
    /// passed, so a caller reading it straight after the call still sees the
    /// transient status.
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
            created_at: new.now,
            serverless_cache_name: cache.name.clone(),
            engine: cache.engine.clone(),
            major_engine_version: cache.major_engine_version.clone(),
            tags: new.tags,
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
            // "The total size of a serverless cache snapshot, in bytes": this
            // emulator serves no cache data plane, so it holds no data set whose
            // size it could report rather than invent — the same reason
            // [`NodeSnapshotRecord::to_modeled`] reports no cache size.
            bytes_used_for_cache: None,
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
