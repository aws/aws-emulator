// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptune-amazonrdsv19 service.
//!
//! This module owns the service's PERSISTED records (a DB cluster, a DB instance and a
//! cluster's membership list) plus the pure functions that turn one of those records
//! into the modelled wire shape (`DBCluster` / `DBInstance`). Every handler under
//! `src/ops/` reads and writes state through [`service_state`] and renders its response
//! with [`cluster_view`] / [`instance_view`], so a `Describe*` answers from exactly the
//! bytes a `Create*` stored.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    DBCluster, DBClusterMember, DBClusterRole, DBInstance, DBParameterGroupStatus, Endpoint,
    ServerlessV2ScalingConfigurationInfo, VpcSecurityGroupMembership,
};

/// The port Neptune's instances accept connections on when the request names none.
/// Source: `CreateDBClusterMessage$Port` — "Default: 8182".
pub const DEFAULT_PORT: i32 = 8182;

/// The engine the service's model accepts.
/// Source: `CreateDBClusterMessage$Engine` — "Valid Values: neptune".
pub const NEPTUNE_ENGINE: &str = "neptune";

/// The engine version a cluster is created with when the request names none. The model
/// declares no default, so the emulator picks one Neptune release and PERSISTS it; every
/// later read reports the stored value.
pub const DEFAULT_ENGINE_VERSION: &str = "1.3.2.1";

/// Days of automated backups retained when the request names none.
/// Source: `CreateDBClusterMessage$BackupRetentionPeriod` — "Default: 1".
pub const DEFAULT_BACKUP_RETENTION_PERIOD: i32 = 1;

/// The inclusive bounds the backup retention period must fall in.
/// Source: `CreateDBClusterMessage$BackupRetentionPeriod` — "Must be a value from 1 to 35".
pub const BACKUP_RETENTION_BOUNDS: (i32, i32) = (1, 35);

/// Page size used when a `Describe*` request names no `MaxRecords`.
/// Source: `DescribeDBClustersMessage$MaxRecords` — "Default: 100".
pub const DEFAULT_MAX_RECORDS: i32 = 100;

/// The inclusive bounds `MaxRecords` must fall in.
/// Source: `DescribeDBClustersMessage$MaxRecords` — "Constraints: Minimum 20, maximum 100".
pub const MAX_RECORDS_BOUNDS: (i32, i32) = (20, 100);

/// The longest a DB cluster / DB instance identifier may be.
/// Source: `CreateDBClusterMessage$DBClusterIdentifier` — "Must contain from 1 to 63
/// letters, numbers, or hyphens".
pub const IDENTIFIER_MAX_LEN: usize = 63;

/// Storage, in GiB, reported for a Neptune cluster: Neptune storage grows automatically
/// and the API reports the minimum unit rather than a provisioned size.
pub const CLUSTER_ALLOCATED_STORAGE: i32 = 1;

/// The network type a cluster gets when the request names none.
/// Source: `CreateDBClusterMessage$NetworkType` — "IPV4 – ( the default )".
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The storage type whose name the API deliberately omits from a response.
/// Source: `CreateDBClusterMessage$StorageType` — "standard – ( the default ) ... When set
/// to standard, the storage type is not returned in the response."
pub const STANDARD_STORAGE_TYPE: &str = "standard";

/// The region an ARN or endpoint uses when the request carries no SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Status of a resource the service is still provisioning.
pub const STATUS_CREATING: &str = "creating";
/// Status of a provisioned, usable resource.
pub const STATUS_AVAILABLE: &str = "available";
/// Status reported for a resource whose deletion the caller just asked for.
pub const STATUS_DELETING: &str = "deleting";

/// How long after a create the clock owes the `creating` -> `available` transition.
///
/// Real Neptune takes minutes; the emulator compresses that to a span short enough for a
/// test and long enough that a caller acting straight after the create still observes the
/// transient `creating` status. Nothing sleeps for it: the due instant is stored on the
/// record and [`ComAmazonawsNeptuneAmazonrdsv19State::settle`] materialises the
/// transition the next time any caller touches the state.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// The most DB clusters this store holds. Bounds the memory one emulator process can be
/// made to hold through repeated `CreateDBCluster` calls with fresh identifiers, and with
/// it the per-request cost of [`ComAmazonawsNeptuneAmazonrdsv19State::settle`]'s scan.
/// `CreateDBCluster` refuses past it with the modelled `DBClusterQuotaExceededFault`
/// rather than growing without limit.
pub const MAX_STORED_CLUSTERS: usize = 1_000;

/// The most DB instances this store holds, bounding instance memory the same way.
/// `CreateDBInstance` refuses past it with the modelled `InstanceQuotaExceededFault`.
pub const MAX_STORED_INSTANCES: usize = 5_000;

/// The most instances one cluster holds. Real Neptune allows a writer plus up to 15 read
/// replicas, so this is the cluster's own documented shape rather than a chosen number,
/// and it bounds the `members` vector a single cluster's record carries.
///
/// `CreateDBInstance` refuses past it with `InstanceQuotaExceededFault` (HTTP 400), which
/// is a SUBSTITUTION: in real RDS that fault is the account-level instance quota, and the
/// per-cluster replica limit is reported as a parameter error. This operation models no
/// replica-limit fault, so the nearest modelled quota fault carries it; the status is the
/// same and the message names the cluster and the limit.
pub const MAX_MEMBERS_PER_CLUSTER: usize = 16;

/// Status of an IAM role association the service is still putting in place.
/// Source: `DBClusterRole$Status` — "PENDING - the IAM role ARN is being associated with the
/// DB cluster."
pub const ROLE_STATUS_PENDING: &str = "PENDING";

/// Status of an IAM role association that holds.
/// Source: `DBClusterRole$Status` — "ACTIVE - the IAM role ARN is associated with the DB
/// cluster and can be used to access other Amazon services on your behalf."
pub const ROLE_STATUS_ACTIVE: &str = "ACTIVE";

/// How many IAM roles one DB cluster may have associated with it at once: the "IAM roles per
/// DB cluster" quota AWS publishes for this engine family, which is what the modelled
/// `DBClusterRoleQuotaExceededFault` reports having reached — and what bounds the
/// associations one cluster's record carries.
pub const MAX_ASSOCIATED_ROLES: usize = 5;

/// How long after an association the clock owes the `PENDING` -> `ACTIVE` transition.
///
/// The association is not in force the instant the call returns — `PENDING` is "the IAM role
/// ARN is being associated with the DB cluster" — so the span is never zero: a caller reading
/// the cluster straight after the call still observes it. Nothing sleeps for it: the due
/// instant is stored on the association and
/// [`ComAmazonawsNeptuneAmazonrdsv19State::settle`] materialises the transition the next time
/// any caller touches the state.
pub const ROLE_ASSOCIATION_SPAN: Duration = Duration::from_secs(3);

/// One IAM role associated with a DB cluster, as `DBCluster$AssociatedRoles` reports it.
#[derive(Debug, Clone)]
pub struct ClusterRole {
    pub role_arn: String,
    pub feature_name: Option<String>,
    pub status: String,
    /// When `status` is `PENDING`, the instant the clock owes `ACTIVE`.
    pub active_at: Option<SystemTime>,
}

/// One instance's membership of a DB cluster, as `DBCluster$DBClusterMembers` reports it.
#[derive(Debug, Clone)]
pub struct ClusterMember {
    pub db_instance_identifier: String,
    pub is_cluster_writer: bool,
    pub promotion_tier: i32,
    pub db_cluster_parameter_group_status: String,
}

/// A persisted Neptune DB cluster. Every member of the modelled `DBCluster` shape that
/// this service can answer is computed from exactly these fields.
#[derive(Debug, Clone)]
pub struct DbClusterRecord {
    pub identifier: String,
    pub arn: String,
    pub resource_id: String,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    /// When `status` is `creating`, the instant the clock owes `available`.
    pub available_at: Option<SystemTime>,
    /// The DNS label AWS mints per cluster and shares between the cluster endpoints and
    /// the endpoints of every instance in the cluster.
    pub endpoint_token: String,
    pub endpoint: String,
    pub reader_endpoint: String,
    pub hosted_zone_id: String,
    pub port: i32,
    pub availability_zones: Vec<String>,
    pub db_subnet_group: Option<String>,
    pub db_cluster_parameter_group: String,
    pub backup_retention_period: i32,
    pub preferred_backup_window: String,
    pub preferred_maintenance_window: String,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub deletion_protection: bool,
    pub copy_tags_to_snapshot: bool,
    pub iam_database_authentication_enabled: bool,
    pub enabled_cloudwatch_logs_exports: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub serverless_v2_scaling_configuration: Option<(Option<f64>, Option<f64>)>,
    pub members: Vec<ClusterMember>,
    /// The IAM roles associated with this cluster, in the order they were associated.
    pub associated_roles: Vec<ClusterRole>,
    pub tags: Vec<(String, String)>,
    pub cluster_create_time: SystemTime,
    pub allocated_storage: i32,
    pub storage_type: Option<String>,
    pub network_type: Option<String>,
}

/// A persisted Neptune DB instance, always a member of one cluster.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    pub identifier: String,
    pub arn: String,
    pub dbi_resource_id: String,
    pub instance_class: String,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    /// When `status` is `creating`, the instant the clock owes `available`.
    pub available_at: Option<SystemTime>,
    pub cluster_identifier: String,
    pub endpoint_address: String,
    pub endpoint_port: i32,
    pub hosted_zone_id: String,
    pub availability_zone: String,
    pub publicly_accessible: bool,
    pub promotion_tier: i32,
    pub db_parameter_group: String,
    pub preferred_maintenance_window: String,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    pub multi_az: bool,
    pub monitoring_interval: i32,
    pub vpc_security_group_ids: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub instance_create_time: SystemTime,
    pub deletion_protection: bool,
}

/// Per-service state for com-amazonaws-neptune-amazonrdsv19. A handler reaches it through [`service_state`].
///
/// The `Describe*` operations need no pagination table: their `Marker` is the identifier
/// of the next record, and both maps are sorted by identifier, so a page resumes with a
/// range scan and a read never writes.
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptuneAmazonrdsv19State {
    /// Keyed by `DBClusterIdentifier` (already lowercased), sorted for stable paging.
    pub clusters: BTreeMap<String, DbClusterRecord>,
    /// Keyed by `DBInstanceIdentifier` (already lowercased), sorted for stable paging.
    pub instances: BTreeMap<String, DbInstanceRecord>,
}

impl ComAmazonawsNeptuneAmazonrdsv19State {
    /// Materialise every status transition the clock already owes.
    ///
    /// This is the ONLY write a reporting operation makes: a resource created more than
    /// [`PROVISIONING_SPAN`] ago was owed `available` at that instant whether or not
    /// anybody called, so applying it here cannot change what a later caller sees.
    pub fn settle(&mut self) {
        let now = SystemTime::now();
        for cluster in self.clusters.values_mut() {
            if cluster.available_at.is_some_and(|due| now >= due) {
                cluster.status = STATUS_AVAILABLE.to_string();
                cluster.available_at = None;
            }
            // An association the service was still putting in place was owed `ACTIVE` at its
            // own due instant, whoever called next.
            for role in &mut cluster.associated_roles {
                if role.active_at.is_some_and(|due| now >= due) {
                    role.status = ROLE_STATUS_ACTIVE.to_string();
                    role.active_at = None;
                }
            }
        }
        for instance in self.instances.values_mut() {
            if instance.available_at.is_some_and(|due| now >= due) {
                instance.status = STATUS_AVAILABLE.to_string();
                instance.available_at = None;
            }
        }
    }

    /// Whether this store can hold another cluster, i.e. is below
    /// [`MAX_STORED_CLUSTERS`].
    pub fn accepts_another_cluster(&self) -> bool {
        self.clusters.len() < MAX_STORED_CLUSTERS
    }

    /// Whether this store can hold another instance, i.e. is below
    /// [`MAX_STORED_INSTANCES`].
    pub fn accepts_another_instance(&self) -> bool {
        self.instances.len() < MAX_STORED_INSTANCES
    }

    /// The cluster `reference` names, by identifier or by ARN (both are accepted wherever
    /// the API takes a `DBClusterIdentifier`).
    pub fn find_cluster(&self, reference: &str) -> Option<&DbClusterRecord> {
        let key = reference.to_ascii_lowercase();
        self.clusters
            .get(&key)
            .or_else(|| self.clusters.values().find(|c| c.arn == reference))
    }

    /// The identifier of the cluster `reference` names, by identifier or by ARN.
    pub fn resolve_cluster_key(&self, reference: &str) -> Option<String> {
        self.find_cluster(reference).map(|c| c.identifier.clone())
    }

    /// The instance `reference` names, by identifier or by ARN.
    pub fn find_instance(&self, reference: &str) -> Option<&DbInstanceRecord> {
        let key = reference.to_ascii_lowercase();
        self.instances
            .get(&key)
            .or_else(|| self.instances.values().find(|i| i.arn == reference))
    }

    /// The identifier of the instance `reference` names, by identifier or by ARN.
    pub fn resolve_instance_key(&self, reference: &str) -> Option<String> {
        self.find_instance(reference).map(|i| i.identifier.clone())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNeptuneAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNeptuneAmazonrdsv19State>)
}

// ---------------------------------------------------------------------------
// Request-derived values: the region, identifier validation, minted ids.
// ---------------------------------------------------------------------------

/// The region every ARN and endpoint this service mints uses: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Lowercase `value` after checking the identifier grammar the API documents, or describe
/// the violation so the caller can fix the request.
///
/// Source (identical for `DBClusterIdentifier` and `DBInstanceIdentifier`): "This
/// parameter is stored as a lowercase string. Constraints: Must contain from 1 to 63
/// letters, numbers, or hyphens. First character must be a letter. Cannot end with a
/// hyphen or contain two consecutive hyphens."
pub fn normalize_identifier(member: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err(format!(
            "Invalid value for {member}: must contain from 1 to {IDENTIFIER_MAX_LEN} letters, numbers, or hyphens"
        ));
    }
    // The character grammar is checked BEFORE the length, so an input that violates both
    // is refused for the grammar — and the length is counted in characters, not bytes, so
    // a multi-byte input is never mis-reported as too long.
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "Invalid value for {member}: must contain only letters, numbers, or hyphens"
        ));
    }
    if value.chars().count() > IDENTIFIER_MAX_LEN {
        return Err(format!(
            "Invalid value for {member}: must contain from 1 to {IDENTIFIER_MAX_LEN} letters, numbers, or hyphens"
        ));
    }
    if !value.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err(format!(
            "Invalid value for {member}: first character must be a letter"
        ));
    }
    if value.ends_with('-') {
        return Err(format!("Invalid value for {member}: cannot end with a hyphen"));
    }
    if value.contains("--") {
        return Err(format!(
            "Invalid value for {member}: cannot contain two consecutive hyphens"
        ));
    }
    Ok(value.to_ascii_lowercase())
}

/// Refuse an engine the model does not list.
/// Source: `CreateDBClusterMessage$Engine` — "Valid Values: neptune".
pub fn validate_engine(engine: &str) -> Result<String, String> {
    if engine.eq_ignore_ascii_case(NEPTUNE_ENGINE) {
        Ok(NEPTUNE_ENGINE.to_string())
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: valid values are {NEPTUNE_ENGINE}"
        ))
    }
}

/// Apply `BackupRetentionPeriod`'s documented default and bounds.
pub fn resolve_backup_retention_period(requested: Option<i32>) -> Result<i32, String> {
    let (low, high) = BACKUP_RETENTION_BOUNDS;
    match requested {
        None => Ok(DEFAULT_BACKUP_RETENTION_PERIOD),
        Some(days) if (low..=high).contains(&days) => Ok(days),
        Some(days) => Err(format!(
            "Invalid value {days} for BackupRetentionPeriod: must be a value from {low} to {high}"
        )),
    }
}

/// Apply `MaxRecords`'s documented default and bounds, as a page size.
pub fn resolve_max_records(requested: Option<i32>) -> Result<usize, String> {
    let (low, high) = MAX_RECORDS_BOUNDS;
    let value = match requested {
        None => DEFAULT_MAX_RECORDS,
        Some(value) if (low..=high).contains(&value) => value,
        Some(value) => {
            return Err(format!(
                "Invalid value {value} for MaxRecords: must be between {low} and {high}"
            ))
        }
    };
    Ok(value as usize)
}

/// The filter names `DescribeDBClusters` and `DescribeDBInstances` document.
pub const SUPPORTED_DESCRIBE_FILTERS: [&str; 2] = ["db-cluster-id", "engine"];

/// The characters a minted resource id uses: RFC 4648 base32, which is what AWS's
/// `DbClusterResourceId` / `DbiResourceId` tokens are drawn from.
const RESOURCE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// The characters a minted DNS label uses: a DNS label is case-insensitive and AWS emits
/// endpoint shard labels in lowercase.
const DNS_LABEL_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The SHA-256 digest of `seed`, the single source of every value this service mints.
fn mint_digest(seed: &str) -> [u8; 32] {
    Sha256::digest(seed.as_bytes()).into()
}

/// `len` characters drawn from `alphabet` by a SHA-256 digest of `seed`.
///
/// Deterministic in `seed`, which always includes the creation instant in nanoseconds, so
/// two resources that reuse one identifier across a delete still get distinct tokens. The
/// result is STORED on the record at create time; nothing re-derives it on a read.
fn mint_token(seed: &str, alphabet: &[u8], len: usize) -> String {
    let mut out = String::with_capacity(len);
    let mut digest = mint_digest(seed);
    let mut taken = 0usize;
    while out.len() < len {
        if taken == digest.len() {
            digest = Sha256::digest(digest).into();
            taken = 0;
        }
        let byte = digest[taken];
        taken += 1;
        out.push(char::from(alphabet[usize::from(byte) % alphabet.len()]));
    }
    out
}

/// Nanoseconds since the Unix epoch, the uniqueness term in every minted token's seed.
fn epoch_nanos(at: SystemTime) -> u128 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// The seed every per-resource choice is derived from: the identifier plus the creation
/// instant, so a choice is stable for the record that stores it and distinct from a later
/// resource that reuses the identifier. The ONE definition both creates call.
pub fn record_seed(identifier: &str, created: SystemTime) -> String {
    format!("{identifier}/{}", epoch_nanos(created))
}

/// A cluster's `DbClusterResourceId`, e.g. `cluster-ABCDEFGHIJKLMNOPQRSTUVWXYZ`.
pub fn mint_cluster_resource_id(identifier: &str, created: SystemTime) -> String {
    let seed = format!("cluster-resource/{identifier}/{}", epoch_nanos(created));
    format!("cluster-{}", mint_token(&seed, RESOURCE_ID_ALPHABET, 26))
}

/// An instance's `DbiResourceId`, e.g. `db-ABCDEFGHIJKLMNOPQRSTUVWXYZ`.
pub fn mint_instance_resource_id(identifier: &str, created: SystemTime) -> String {
    let seed = format!("db-resource/{identifier}/{}", epoch_nanos(created));
    format!("db-{}", mint_token(&seed, RESOURCE_ID_ALPHABET, 26))
}

/// The per-cluster DNS shard label shared by the cluster endpoints and by the endpoint of
/// every instance in the cluster.
pub fn mint_endpoint_token(identifier: &str, created: SystemTime) -> String {
    let seed = format!("endpoint/{identifier}/{}", epoch_nanos(created));
    mint_token(&seed, DNS_LABEL_ALPHABET, 12)
}

/// The Route 53 hosted zone the region's RDS endpoints live in. One zone per region, so
/// every cluster in a region reports the same id — as AWS does.
///
/// The value is MINTED from the region rather than looked up: AWS's per-region zone ids
/// are published in the Route 53 / RDS documentation but are not in this service's Smithy
/// model, and the member's contract (`DBCluster$HostedZoneId`, `Endpoint$HostedZoneId`:
/// "the ID that Amazon Route 53 assigns when you create a hosted zone") is a stable,
/// `Z`-prefixed, per-region id. A client comparing it against AWS's own published id for
/// a region will see a different label; it keeps the format, the per-region invariance and
/// the stability the member promises.
pub fn hosted_zone_id(region: &str) -> String {
    format!(
        "Z{}",
        mint_token(&format!("hosted-zone/{region}"), RESOURCE_ID_ALPHABET, 13)
    )
}

/// The parameter-group family a Neptune engine version belongs to: the version's major
/// and minor components, e.g. `1.3.2.1` -> `neptune1.3`.
pub fn parameter_group_family(engine_version: &str) -> String {
    let mut parts = engine_version.split('.');
    let major = parts.next().unwrap_or("1");
    let minor = parts.next().unwrap_or("0");
    format!("{NEPTUNE_ENGINE}{major}.{minor}")
}

/// The name of the default parameter group for an engine version, which is what a cluster
/// or instance created without `DBClusterParameterGroupName` / `DBParameterGroupName`
/// gets: "If this argument is omitted, the default is used."
pub fn default_parameter_group(engine_version: &str) -> String {
    format!("default.{}", parameter_group_family(engine_version))
}

/// The three Availability Zones a region's cluster spans when the request names none.
pub fn default_availability_zones(region: &str) -> Vec<String> {
    ['a', 'b', 'c']
        .into_iter()
        .map(|suffix| format!("{region}{suffix}"))
        .collect()
}

/// The daily 30-minute automated-backup window, picked once per cluster and persisted.
///
/// Source: `CreateDBClusterMessage$PreferredBackupWindow` — "The default is a 30-minute
/// window selected at random from an 8-hour block of time for each Amazon Region." The
/// choice is derived from the cluster's own seed so it is stable, and then stored.
pub fn default_backup_window(seed: &str) -> String {
    // 16 half-hour slots inside an 8-hour block.
    let minutes_from_midnight =
        usize::from(mint_digest(&format!("backup-window/{seed}"))[0] % 16) * 30;
    let end = minutes_from_midnight + 30;
    format!(
        "{:02}:{:02}-{:02}:{:02}",
        minutes_from_midnight / 60,
        minutes_from_midnight % 60,
        end / 60,
        end % 60
    )
}

/// The weekly 30-minute maintenance window, picked once per resource and persisted.
///
/// Source: `CreateDBClusterMessage$PreferredMaintenanceWindow` — "Format:
/// ddd:hh24:mi-ddd:hh24:mi ... selected at random from an 8-hour block of time for each
/// Amazon Region, occurring on a random day of the week. Valid Days: Mon, Tue, Wed, Thu,
/// Fri, Sat, Sun."
pub fn default_maintenance_window(seed: &str) -> String {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let digest = mint_digest(&format!("maintenance-window/{seed}"));
    let day = DAYS[usize::from(digest[0]) % DAYS.len()];
    let minutes_from_midnight = usize::from(digest[1] % 16) * 30;
    let end = minutes_from_midnight + 30;
    format!(
        "{day}:{:02}:{:02}-{day}:{:02}:{:02}",
        minutes_from_midnight / 60,
        minutes_from_midnight % 60,
        end / 60,
        end % 60
    )
}

// ---------------------------------------------------------------------------
// Record -> wire shape.
// ---------------------------------------------------------------------------

fn security_group_memberships(ids: &[String]) -> Option<Vec<VpcSecurityGroupMembership>> {
    Some(
        ids.iter()
            .map(|id| VpcSecurityGroupMembership {
                vpc_security_group_id: Some(id.clone()),
                status: Some("active".to_string()),
            })
            .collect(),
    )
}

/// Render a persisted cluster as the modelled `DBCluster`, overriding only the reported
/// `status` (a delete reports `deleting` for the record it just removed).
pub fn cluster_view(record: &DbClusterRecord, status: &str) -> DBCluster {
    DBCluster {
        allocated_storage: Some(record.allocated_storage),
        availability_zones: Some(record.availability_zones.clone()),
        backup_retention_period: Some(record.backup_retention_period),
        // Neptune does not support a character set or a database name, and it has no
        // master user: the model documents all three as "Not supported by Neptune".
        character_set_name: None,
        database_name: None,
        master_username: None,
        db_cluster_identifier: Some(record.identifier.clone()),
        db_cluster_parameter_group: Some(record.db_cluster_parameter_group.clone()),
        db_subnet_group: record.db_subnet_group.clone(),
        status: Some(status.to_string()),
        percent_progress: None,
        earliest_restorable_time: None,
        endpoint: Some(record.endpoint.clone()),
        reader_endpoint: Some(record.reader_endpoint.clone()),
        // A cluster is multi-AZ once it has more than one instance to spread over them.
        multi_az: Some(record.members.len() > 1),
        engine: Some(record.engine.clone()),
        engine_version: Some(record.engine_version.clone()),
        latest_restorable_time: None,
        port: Some(record.port),
        db_cluster_option_group_memberships: Some(Vec::new()),
        preferred_backup_window: Some(record.preferred_backup_window.clone()),
        preferred_maintenance_window: Some(record.preferred_maintenance_window.clone()),
        replication_source_identifier: None,
        read_replica_identifiers: Some(Vec::new()),
        db_cluster_members: Some(
            record
                .members
                .iter()
                .map(|member| DBClusterMember {
                    db_instance_identifier: Some(member.db_instance_identifier.clone()),
                    is_cluster_writer: Some(member.is_cluster_writer),
                    db_cluster_parameter_group_status: Some(
                        member.db_cluster_parameter_group_status.clone(),
                    ),
                    promotion_tier: Some(member.promotion_tier),
                })
                .collect(),
        ),
        vpc_security_groups: security_group_memberships(&record.vpc_security_group_ids),
        hosted_zone_id: Some(record.hosted_zone_id.clone()),
        storage_encrypted: Some(record.storage_encrypted),
        kms_key_id: record.kms_key_id.clone(),
        db_cluster_resource_id: Some(record.resource_id.clone()),
        db_cluster_arn: Some(record.arn.clone()),
        associated_roles: Some(
            record
                .associated_roles
                .iter()
                .map(|role| DBClusterRole {
                    role_arn: Some(role.role_arn.clone()),
                    status: Some(role.status.clone()),
                    feature_name: role.feature_name.clone(),
                })
                .collect(),
        ),
        iam_database_authentication_enabled: Some(record.iam_database_authentication_enabled),
        clone_group_id: None,
        cluster_create_time: Some(::aws_smithy_types::DateTime::from(
            record.cluster_create_time,
        )),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
        enabled_cloudwatch_logs_exports: Some(record.enabled_cloudwatch_logs_exports.clone()),
        pending_modified_values: None,
        deletion_protection: Some(record.deletion_protection),
        cross_account_clone: Some(false),
        automatic_restart_time: None,
        serverless_v2_scaling_configuration: record.serverless_v2_scaling_configuration.map(
            |(min_capacity, max_capacity)| ServerlessV2ScalingConfigurationInfo {
                min_capacity,
                max_capacity,
            },
        ),
        global_cluster_identifier: None,
        io_optimized_next_allowed_modification_time: None,
        storage_type: record.storage_type.clone(),
        network_type: record.network_type.clone(),
    }
}

/// Render a persisted instance as the modelled `DBInstance`, overriding only the reported
/// `DBInstanceStatus`.
///
/// `cluster` is the instance's owning cluster, when it is still persisted: a cluster
/// instance inherits its storage, backup and encryption settings from the cluster, so
/// those response members are computed from the cluster's record rather than duplicated
/// on the instance's.
pub fn instance_view(
    record: &DbInstanceRecord,
    cluster: Option<&DbClusterRecord>,
    status: &str,
) -> DBInstance {
    DBInstance {
        db_instance_identifier: Some(record.identifier.clone()),
        db_instance_class: Some(record.instance_class.clone()),
        engine: Some(record.engine.clone()),
        db_instance_status: Some(status.to_string()),
        // Neptune has no master user and no database name on an instance.
        master_username: None,
        db_name: None,
        endpoint: Some(Endpoint {
            address: Some(record.endpoint_address.clone()),
            port: Some(record.endpoint_port),
            hosted_zone_id: Some(record.hosted_zone_id.clone()),
        }),
        allocated_storage: Some(cluster.map_or(CLUSTER_ALLOCATED_STORAGE, |c| c.allocated_storage)),
        instance_create_time: Some(::aws_smithy_types::DateTime::from(
            record.instance_create_time,
        )),
        preferred_backup_window: cluster.map(|c| c.preferred_backup_window.clone()),
        backup_retention_period: cluster.map(|c| c.backup_retention_period),
        db_security_groups: Some(Vec::new()),
        vpc_security_groups: security_group_memberships(&record.vpc_security_group_ids),
        db_parameter_groups: Some(vec![DBParameterGroupStatus {
            db_parameter_group_name: Some(record.db_parameter_group.clone()),
            parameter_apply_status: Some("in-sync".to_string()),
        }]),
        availability_zone: Some(record.availability_zone.clone()),
        // DB subnet groups have their own operations, which this slice leaves
        // unimplemented, so there is no subnet-group record to report here.
        db_subnet_group: None,
        preferred_maintenance_window: Some(record.preferred_maintenance_window.clone()),
        pending_modified_values: None,
        latest_restorable_time: None,
        multi_az: Some(record.multi_az),
        engine_version: Some(record.engine_version.clone()),
        auto_minor_version_upgrade: Some(record.auto_minor_version_upgrade),
        read_replica_source_db_instance_identifier: None,
        read_replica_db_instance_identifiers: Some(Vec::new()),
        read_replica_db_cluster_identifiers: Some(Vec::new()),
        license_model: None,
        iops: None,
        option_group_memberships: Some(Vec::new()),
        character_set_name: None,
        secondary_availability_zone: None,
        publicly_accessible: Some(record.publicly_accessible),
        status_infos: Some(Vec::new()),
        storage_type: cluster.and_then(|c| c.storage_type.clone()),
        tde_credential_arn: None,
        db_instance_port: Some(record.endpoint_port),
        db_cluster_identifier: Some(record.cluster_identifier.clone()),
        storage_encrypted: Some(cluster.is_some_and(|c| c.storage_encrypted)),
        kms_key_id: cluster.and_then(|c| c.kms_key_id.clone()),
        dbi_resource_id: Some(record.dbi_resource_id.clone()),
        ca_certificate_identifier: None,
        domain_memberships: Some(Vec::new()),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
        monitoring_interval: Some(record.monitoring_interval),
        enhanced_monitoring_resource_arn: None,
        monitoring_role_arn: None,
        promotion_tier: Some(record.promotion_tier),
        db_instance_arn: Some(record.arn.clone()),
        timezone: None,
        iam_database_authentication_enabled: Some(
            cluster.is_some_and(|c| c.iam_database_authentication_enabled),
        ),
        performance_insights_enabled: Some(false),
        performance_insights_kms_key_id: None,
        enabled_cloudwatch_logs_exports: cluster
            .map(|c| c.enabled_cloudwatch_logs_exports.clone()),
        deletion_protection: Some(record.deletion_protection),
        network_type: cluster.and_then(|c| c.network_type.clone()),
    }
}

// ---------------------------------------------------------------------------
// Paging and filtering, shared by the Describe* operations.
// ---------------------------------------------------------------------------

/// One page of `keys` starting at `marker`, plus the `Marker` a follow-up call resumes
/// from.
///
/// The marker IS the identifier the next page starts at: both state maps are sorted by
/// identifier, so resuming is a range scan and the operation stores nothing. `None` back
/// means the page reached the end of the list.
pub fn page(keys: &[String], marker: Option<&str>, max_records: usize) -> (Vec<String>, Option<String>) {
    let start = match marker {
        Some(from) => keys.partition_point(|key| key.as_str() < from),
        None => 0,
    };
    let end = start.saturating_add(max_records).min(keys.len());
    let next = keys.get(end).cloned();
    (keys[start.min(keys.len())..end].to_vec(), next)
}

/// Whether `(engine, cluster)` satisfies the documented `Describe*` filter
/// `(name, values)`.
///
/// The filter NAME is validated once, by the caller, against
/// [`SUPPORTED_DESCRIBE_FILTERS`] before any record is read; an unsupported name is
/// refused there, so it can never be read here as "nothing matched".
pub fn filter_matches(
    name: &str,
    values: &[String],
    engine: &str,
    cluster: Option<&DbClusterRecord>,
) -> bool {
    match name {
        "engine" => values.iter().any(|value| value == engine),
        "db-cluster-id" => cluster.is_some_and(|c| {
            values
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&c.identifier) || *value == c.arn)
        }),
        // Unreachable for a validated name; a name the caller did not validate selects
        // nothing rather than silently matching everything.
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The wordings the IAM-role-association calls answer their modelled faults with.
// ---------------------------------------------------------------------------

/// Whether `roles` already carries an association of `role_arn`, for any feature or none.
///
/// A DB cluster's associations are keyed by the ROLE: the already-exists fault reports that
/// the role ARN is already associated with the cluster and names no feature at all, so one
/// role is associated with one cluster once.
pub fn holds_role(roles: &[ClusterRole], role_arn: &str) -> bool {
    roles.iter().any(|role| role.role_arn == role_arn)
}

/// The `DBClusterNotFoundFault` wording the calls that manage a cluster's IAM ROLE
/// ASSOCIATIONS answer with: the `<resource type> not found: <named>` form, which is the one
/// this API family uses for those calls rather than the sentence a call that REPORTS or
/// DELETES the cluster answers with.
///
/// `named` is the value the REQUEST carried rather than the key the store folded it to, so a
/// caller matching the sentence against what it sent finds it there.
pub fn db_cluster_not_found_colon_message(named: &str) -> String {
    format!("DBCluster not found: {named}")
}

/// The `InvalidDBClusterStateFault` wording a change to a cluster's configuration answers
/// when the cluster is not in a state that change can be made in — a cluster still coming up,
/// or one on its way out.
///
/// It names the STATUS the cluster is in, which is what tells the caller whether waiting will
/// clear the refusal, and nothing else.
pub fn cluster_not_available_for_modification_message(status: &str) -> String {
    format!("DB cluster isn't available for modification with status {status}.")
}

/// The `DBClusterRoleAlreadyExistsFault` wording: it names the role ARN the request carried
/// and the cluster it is already associated with, and tells the caller which value to look at.
pub fn cluster_role_already_associated_message(role_arn: &str, named: &str) -> String {
    format!(
        "Role ARN {role_arn} is already associated with DB Cluster: {named}. Verify your role \
         ARN and try again."
    )
}

/// The `DBClusterRoleQuotaExceededFault` wording: the cluster already carries every IAM role
/// it may.
pub fn cluster_role_quota_message(identifier: &str) -> String {
    format!(
        "The DB cluster {identifier} already has {MAX_ASSOCIATED_ROLES} IAM roles associated \
         with it, which is the maximum."
    )
}

/// The refusal message for a `Describe*` filter name the API does not support.
pub fn unsupported_filter_message(name: &str) -> String {
    format!(
        "Unrecognized filter name: {name}. Supported filters are {}",
        SUPPORTED_DESCRIBE_FILTERS.join(", ")
    )
}
