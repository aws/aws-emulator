// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-docdb-amazonrdsv19 service.
//!
//! This is the Amazon DocumentDB control plane the implemented operations read and
//! write: the cluster records, the instance records that are a cluster's members, and
//! the final cluster snapshots `DeleteDBCluster` takes. It also holds the pieces more
//! than one handler needs — the documented input rules (`validate_*`), the documented
//! defaults, and the record-to-wire-shape projections — so no two handlers can disagree
//! about them.
//!
//! Every constant that encodes an AWS-published value names the model member whose
//! documentation states it; read it with
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-docdb-amazonrdsv19/smithy-model/com-amazonaws-docdb-amazonrdsv19.json")
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The subnets a `SubnetIds` list names, resolved against the service that OWNS subnets.
pub mod ec2_subnets;

/// The connection address an instance announces for a client to dial.
pub mod announced_address;

/// The data plane behind that address: the socket a client of a cluster or of one of its
/// instances connects to, and the documents the cluster holds.
pub mod cluster_data_plane;

pub use announced_address::announced_instance_host;

pub use cluster_data_plane::{
    endpoints_are_encrypted, engine_speaks_the_document_wire, serve_cluster, serve_instance,
    ClusterDocuments, ServedDataPlanes, ServedEndpoint, SharedVolume,
};

pub use ec2_subnets::{
    az_coverage_message, group_network_types, invalid_subnets_message, resolve_subnets,
    zone_coverage_shortfall, ResolvedSubnet, ResolvedSubnets, MIN_SUBNET_GROUP_AVAILABILITY_ZONES,
    NETWORK_TYPE_DUAL, NETWORK_TYPE_IPV4, SUBNETS_NOT_IN_ONE_VPC_MESSAGE,
};

use crate::ops::types::{
    AvailabilityZone, Certificate, DBCluster, DBClusterMember, DBClusterParameterGroup,
    DBClusterSnapshot, DBClusterSnapshotAttribute, DBClusterSnapshotAttributesResult,
    DBEngineVersion, DBInstance, DBSubnetGroup, Endpoint, EventCategoriesMap,
    OrderableDBInstanceOption, Parameter, PendingModifiedValues,
    ServerlessV2ScalingConfigurationInfo, Subnet, Tag, UpgradeTarget, VpcSecurityGroupMembership,
};

/// The ARN namespace and sigv4 signing name DocumentDB shares with RDS, so a cluster ARN
/// reads `arn:aws:rds:<region>:<account>:cluster:<id>` (`OPERATIONS.md`, "Wire protocol").
pub const ARN_NAMESPACE: &str = "rds";

/// `CreateDBClusterMessage$Engine`: "Valid values: docdb".
pub const DOCDB_ENGINE: &str = "docdb";

/// `CreateDBClusterMessage$EngineVersion`: "The --engine-version will default to the
/// latest major engine version." Read from the shared engine catalog of the front door this
/// service is a view of ([`::service_common::docdb_engine_catalog::DEFAULT_ENGINE_VERSION`]),
/// so a cluster created without a version runs the release that door DESIGNATES as the
/// engine's default — whichever view the create arrived at, and whether or not the
/// designated line publishes a later patch
/// ([`::service_common::docdb_engine_catalog::VersionDefault`]).
pub const DEFAULT_ENGINE_VERSION: &str = ::service_common::docdb_engine_catalog::DEFAULT_ENGINE_VERSION;

/// The port DocumentDB listens on when `Port` is absent (the engine's default port, and
/// the one `aws docdb create-db-cluster` reports back for a cluster created without it).
pub const DEFAULT_PORT: i32 = 27017;

/// `CreateDBClusterMessage$BackupRetentionPeriod`: "Default: 1 Constraints: Must be a
/// value from 1 to 35."
pub const DEFAULT_BACKUP_RETENTION_PERIOD: i32 = 1;
/// See [`DEFAULT_BACKUP_RETENTION_PERIOD`].
pub const MIN_BACKUP_RETENTION_PERIOD: i32 = 1;
/// See [`DEFAULT_BACKUP_RETENTION_PERIOD`].
pub const MAX_BACKUP_RETENTION_PERIOD: i32 = 35;

/// `DescribeDBClustersMessage$MaxRecords` / `DescribeDBInstancesMessage$MaxRecords`:
/// "Default: 100 Constraints: Minimum 20, maximum 100."
pub const DEFAULT_MAX_RECORDS: i32 = 100;
/// See [`DEFAULT_MAX_RECORDS`].
pub const MIN_MAX_RECORDS: i32 = 20;
/// See [`DEFAULT_MAX_RECORDS`].
pub const MAX_MAX_RECORDS: i32 = 100;

/// `CreateDBInstanceMessage$PromotionTier`: "Default: 1 Valid values: 0-15".
pub const DEFAULT_PROMOTION_TIER: i32 = 1;
/// See [`DEFAULT_PROMOTION_TIER`].
pub const MIN_PROMOTION_TIER: i32 = 0;
/// See [`DEFAULT_PROMOTION_TIER`].
pub const MAX_PROMOTION_TIER: i32 = 15;

/// `CreateDBClusterMessage$DBClusterIdentifier`, `CreateDBInstanceMessage$DBInstanceIdentifier`
/// and `ModifyDBClusterMessage$NewDBClusterIdentifier` all state the same rule: "Must
/// contain from 1 to 63 letters, numbers, or hyphens. The first character must be a
/// letter. Cannot end with a hyphen or contain two consecutive hyphens."
pub const MAX_IDENTIFIER_LEN: usize = 63;

/// `DeleteDBClusterMessage$FinalDBSnapshotIdentifier` states the same rule with a longer
/// bound: "Must be from 1 to 255 letters, numbers, or hyphens. The first character must
/// be a letter. Cannot end with a hyphen or contain two consecutive hyphens."
pub const MAX_SNAPSHOT_IDENTIFIER_LEN: usize = 255;

/// What a `DeleteDBCluster` that names NEITHER of its two final-snapshot members is refused
/// with.
///
/// `SkipFinalSnapshot`: "If false is specified, a cluster snapshot is created before the DB
/// cluster is deleted … If SkipFinalSnapshot is false, you must specify a
/// FinalDBSnapshotIdentifier parameter. Default: false" — so exactly one of the two has to
/// be given, which is what the sentence states: an imperative naming the member the request
/// has to carry and the member that waives it, with no terminal period. ENTIRELY fixed text,
/// so no identifier, value or limit of the request or of the store is substituted into it,
/// and the sibling view of this same front door
/// (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/published_values.rs`) words
/// the cluster-level delete with the same sentence.
pub const FINAL_SNAPSHOT_MEMBER_REQUIRED: &str =
    "Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot";

/// `CreateDBClusterMessage$MasterUsername`: "Must be from 1 to 63 letters or numbers. The
/// first character must be a letter."
pub const MAX_MASTER_USERNAME_LEN: usize = 63;

/// `CreateDBClusterMessage$MasterUserPassword`: "Must contain from 8 to 100 characters."
pub const MIN_MASTER_USER_PASSWORD_LEN: usize = 8;
/// See [`MIN_MASTER_USER_PASSWORD_LEN`].
pub const MAX_MASTER_USER_PASSWORD_LEN: usize = 100;

/// `CreateDBClusterMessage$MasterUserPassword`: the password "can contain any printable
/// ASCII character except forward slash (/), double quote (\"), or the \"at\" symbol (@)".
pub const MASTER_USER_PASSWORD_FORBIDDEN: [char; 3] = ['/', '"', '@'];

/// `CreateDBClusterMessage$StorageType`: "Valid values for storage type - standard |
/// iopt1 Default value is standard ... The storage type isn't returned when you set it to
/// standard."
pub const DEFAULT_STORAGE_TYPE: &str = "standard";

/// The status a resource carries until the clock makes it available.
pub const STATUS_CREATING: &str = "creating";
/// The status of a cluster or instance that finished provisioning.
pub const STATUS_AVAILABLE: &str = "available";
/// The status returned for a resource the caller just deleted.
pub const STATUS_DELETING: &str = "deleting";

/// `DBClusterMember$DBClusterParameterGroupStatus` for a member whose parameter group is
/// the one its cluster carries.
pub const PARAMETER_GROUP_IN_SYNC: &str = "in-sync";

/// `DBCluster$AutoMinorVersionUpgrade` for a cluster created without any
/// minor-version-upgrade input: neither `CreateDBClusterMessage` nor
/// `ModifyDBClusterMessage` models such a member, and the member was added to the
/// `DBCluster` shape from the recorded responses, where automatic minor version upgrade is
/// on for a cluster nothing asked to pin.
pub const DEFAULT_CLUSTER_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// `DBCluster$DBActivityStreamStatus` for a cluster that has never started a database
/// activity stream: the steady state AWS reports, and the only one this service can reach
/// (the model binds no activity-stream operation to start or stop one).
pub const ACTIVITY_STREAM_STOPPED: &str = "stopped";

/// `DBCluster$DocdbAnalyticsEnabled` for a cluster created without the Amazon DocumentDB
/// analytics capability: no input member in this model turns it on.
pub const DEFAULT_DOCDB_ANALYTICS_ENABLED: bool = false;

/// `DBCluster$CustomDataGroupPresent` for a cluster created without a custom data group:
/// no input member in this model supplies one.
pub const DEFAULT_CUSTOM_DATA_GROUP_PRESENT: bool = false;

/// `DBCluster$VPCNetworkingEnabled` for every cluster this service can create: an Amazon
/// DocumentDB cluster is reachable only from inside a virtual private cloud, whose
/// protocols its subnet group fixes (`DBCluster$NetworkType`: "The network type is
/// determined by the `DBSubnetGroup` specified for the cluster"), so VPC networking is on
/// for every one of them and no input member turns it off.
pub const DEFAULT_VPC_NETWORKING_ENABLED: bool = true;

/// `DBCluster$InternetAccessGatewayEnabled` for a cluster created without an internet
/// access gateway: no input member in this model attaches one.
pub const DEFAULT_INTERNET_ACCESS_GATEWAY_ENABLED: bool = false;

/// `DBInstance$PubliclyAccessible` for every instance of this engine:
/// "Not supported. Amazon DocumentDB does not currently support public endpoints. The
/// value of `PubliclyAccessible` is always `false`."
pub const INSTANCE_PUBLICLY_ACCESSIBLE: bool = false;

/// `DBInstance$AllocatedStorage`, in gibibytes: an Amazon DocumentDB instance stores its
/// data on the storage its CLUSTER manages ("Amazon DocumentDB manages the storage of your
/// cluster automatically", Amazon DocumentDB Developer Guide), so no storage is allocated
/// to the instance itself and it reports the single-unit size AWS reports for
/// cluster-managed storage.
pub const INSTANCE_ALLOCATED_STORAGE: i32 = 1;

/// `DBInstance$StorageThroughput` for an instance of this engine: throughput is a property
/// of provisioned instance storage, which cluster-managed storage
/// ([`INSTANCE_ALLOCATED_STORAGE`]) provisions none of.
pub const INSTANCE_STORAGE_THROUGHPUT: i32 = 0;

/// `DBInstance$DbInstancePort` for an instance of this engine: the instance is reached at
/// its cluster's port, which `DBInstance$Endpoint$Port` and `DBCluster$Port` report, so it
/// carries no port of its own.
pub const INSTANCE_PORT_UNSET: i32 = 0;

/// `DBInstance$HttpEndpointEnabled` for an instance of this engine: the model binds no
/// operation and no input member that enables an HTTP endpoint on an Amazon DocumentDB
/// resource, so none of them has one.
pub const INSTANCE_HTTP_ENDPOINT_ENABLED: bool = false;

/// Seconds in the day `BackupRetentionPeriod` is counted in
/// (`CreateDBClusterMessage$BackupRetentionPeriod`: "The number of days for which
/// automated backups are retained"), so the retained window can be measured back from
/// now.
const SECONDS_PER_DAY: f64 = 24.0 * 60.0 * 60.0;

/// How many characters follow the leading `Z` of a Route 53 hosted zone id, matching the
/// fourteen-character ids AWS publishes for its endpoint hosted zones.
const HOSTED_ZONE_ID_LEN: usize = 13;

/// `VpcSecurityGroupMembership$Status` for a security group attached to a live resource.
pub const SECURITY_GROUP_ACTIVE: &str = "active";

/// `DBClusterSnapshot$SnapshotType` for the final snapshot `DeleteDBCluster` takes: AWS
/// classes a snapshot the caller asked for as `manual`.
pub const SNAPSHOT_TYPE_MANUAL: &str = "manual";

/// The status a cluster snapshot carries while `CopyDBClusterSnapshot` is still copying
/// it. `CopyDBClusterSnapshot`'s own documentation names it: "To cancel the copy operation
/// after it is in progress, delete the target cluster snapshot ... while that cluster
/// snapshot is in the copying status."
pub const SNAPSHOT_STATUS_COPYING: &str = "copying";

/// The `SnapshotType` values `DescribeDBClusterSnapshotsMessage$SnapshotType` lists:
/// "You can specify one of the following values: automated ... manual ... shared ...
/// public". A request naming anything else is refused rather than silently matching
/// nothing.
pub const SNAPSHOT_TYPES: [&str; 4] = ["automated", "manual", "shared", "public"];

/// The one cluster-snapshot attribute AWS defines, as
/// `DBClusterSnapshotAttribute$AttributeName` states: "The attribute named `restore`
/// refers to the list of Amazon Web Services accounts that have permission to copy or
/// restore the manual cluster snapshot."
pub const SNAPSHOT_RESTORE_ATTRIBUTE: &str = "restore";

/// The `restore` value that makes a manual snapshot public:
/// `DBClusterSnapshotAttribute$AttributeValues` — "If a value of `all` is in the list,
/// then the manual cluster snapshot is public and available for any Amazon Web Services
/// account to copy or restore."
pub const SNAPSHOT_ATTRIBUTE_ALL: &str = "all";

/// The digits in an Amazon Web Services account id, the other value
/// `ModifyDBClusterSnapshotAttributeMessage$ValuesToAdd` accepts ("set this list to
/// include one or more Amazon Web Services account IDs").
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// How many accounts one manual snapshot may be authorized for: AWS's published limit on
/// the accounts a manual snapshot can be shared with (20). Reaching it is the modelled
/// `SharedSnapshotQuotaExceededFault` that `ModifyDBClusterSnapshotAttribute` declares.
pub const MAX_SNAPSHOT_SHARED_ACCOUNTS: usize = 20;

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The engine versions the `docdb` engine offers, oldest first.
///
/// They are the rows of the shared engine catalog of the one front door this service is a
/// view of ([`::service_common::docdb_engine_catalog::ENGINE_VERSIONS`]) rather than a list
/// of this view's own: the Amazon RDS view of that door reports the same versions for this
/// engine, and this is the catalogue [`resolve_engine_version`] answers from — a request
/// naming a version that is not in it names a version the engine cannot run.
pub fn engine_versions() -> Vec<&'static str> {
    ::service_common::docdb_engine_catalog::engine_versions()
        .map(|published| published.version)
        .collect()
}

/// `CreateDBSubnetGroupMessage$DBSubnetGroupName`: "Must contain no more than 255 letters,
/// numbers, periods, underscores, spaces, or hyphens."
pub const MAX_SUBNET_GROUP_NAME_LEN: usize = 255;

/// `CreateDBSubnetGroupMessage$DBSubnetGroupName` and
/// `DeleteDBSubnetGroupMessage$DBSubnetGroupName` both state it: "Must not be default".
pub const RESERVED_SUBNET_GROUP_NAME: &str = "default";

// "subnet groups must contain at least one subnet in at least two Availability Zones in the
// Amazon Web Services Region" is a statement about the ZONES the request's subnets lie in,
// so it lives beside the resolution that learns them:
// `ec2_subnets::MIN_SUBNET_GROUP_AVAILABILITY_ZONES` states the requirement and
// `ec2_subnets::zone_coverage_shortfall` applies it.

/// `DBSubnetGroup$SubnetGroupStatus` for a group whose subnets are all usable.
pub const SUBNET_GROUP_COMPLETE: &str = "Complete";

/// `Subnet$SubnetStatus` for a subnet that is in service.
pub const SUBNET_ACTIVE: &str = "Active";

/// The default-parameter-group name prefix AWS uses: a default cluster parameter group is
/// named `default.<family>` (`default.docdb5.0`), one per engine-version family, and exists
/// in every account and Region without anybody creating it.
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete,
// so each growth site carries a cap and nothing caller-sized reaches the store
// unchecked. The caps are Amazon DocumentDB's own published default quotas ("Amazon
// DocumentDB quotas and limits" in the Developer Guide) where it publishes one; where
// it publishes none the constant says so and the value is this emulator's resource
// guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_DB_CLUSTERS clusters, at most
// MAX_DB_INSTANCES instances (and MAX_DB_INSTANCES_PER_CLUSTER of them in any one
// cluster), at most MAX_DB_CLUSTER_SNAPSHOTS final snapshots, and each record's
// caller-sized lists capped at MAX_COLLECTION_ENTRIES entries (MAX_TAGS tags) of at
// most MAX_STRING_CHARS characters.
// ---------------------------------------------------------------------------

/// Amazon DocumentDB's published default quota "Clusters: 40" per Region. An over-quota
/// create is refused with the modelled `DBClusterQuotaExceededFault` rather than stored.
pub const MAX_DB_CLUSTERS: usize = 40;

/// Amazon DocumentDB's published quota "Instances per cluster: 16".
pub const MAX_DB_INSTANCES_PER_CLUSTER: usize = 16;

/// Amazon DocumentDB's published default quota "Instances: 40" per Region.
pub const MAX_DB_INSTANCES: usize = 40;

/// Amazon DocumentDB's published default quota "Manual cluster snapshots: 100" per
/// Region. Both sites that write a snapshot — the final snapshot `DeleteDBCluster` takes
/// and the copy `CopyDBClusterSnapshot` makes — refuse an over-quota create with the
/// modelled `SnapshotQuotaExceededFault` rather than growing the store without limit;
/// `DeleteDBClusterSnapshot` is the reclamation path.
pub const MAX_DB_CLUSTER_SNAPSHOTS: usize = 100;

/// Amazon DocumentDB's published default quota "Subnet groups: 50" per Region. An
/// over-quota create is refused with the modelled `DBSubnetGroupQuotaExceededFault` rather
/// than stored.
pub const MAX_DB_SUBNET_GROUPS: usize = 50;

/// Amazon DocumentDB's published default quota "Subnets per subnet group: 20". A create
/// naming more is refused with the modelled `DBSubnetQuotaExceededFault`.
pub const MAX_SUBNETS_PER_SUBNET_GROUP: usize = 20;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit: a tag key is at most 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit: a tag value is at most 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most cluster parameter groups this
/// emulator will store. Amazon DocumentDB publishes no quota for them, so an over-cap
/// create is refused with the modelled `DBParameterGroupQuotaExceededFault` rather than
/// stored.
///
/// The number of parameters inside one group is bounded separately, by
/// [`MAX_COLLECTION_ENTRIES`]; the number a single request may set is
/// `service_common::rds_cluster_parameters::MAX_PARAMETERS_PER_MODIFY`, re-exported below,
/// which `ModifyDBClusterParameterGroup`'s own documentation states ("A maximum of 20
/// parameters can be modified in a single request").
pub const MAX_DB_CLUSTER_PARAMETER_GROUPS: usize = 100;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will store
/// for a caller-sized list whose length the service publishes no limit for.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// emulator will store for a member whose length the service publishes no limit for.
pub const MAX_STRING_CHARS: usize = 512;

/// Refuse a caller-sized collection whose entry count would grow the store past `max`.
///
/// `Err` carries the message, naming the member and the bound it broke, for the caller
/// to fix the request from.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a `Tags` list this emulator will not store: too many tags, or a key or value
/// past AWS's tagging limits.
pub fn bound_tags(tags: Option<&Vec<Tag>>) -> Result<(), String> {
    let Some(tags) = tags else {
        return Ok(());
    };
    bound_entries("Tags", tags.len(), MAX_TAGS)?;
    for tag in tags {
        if let Some(key) = &tag.key {
            bound_chars("Tags.Key", key, MAX_TAG_KEY_CHARS)?;
        }
        if let Some(value) = &tag.value {
            bound_chars("Tags.Value", value, MAX_TAG_VALUE_CHARS)?;
        }
    }
    Ok(())
}

/// Refuse a caller-sized string list this emulator will not store: too many entries, or
/// an entry past [`MAX_STRING_CHARS`].
pub fn bound_string_list(member: &str, values: Option<&Vec<String>>) -> Result<(), String> {
    let Some(values) = values else {
        return Ok(());
    };
    bound_entries(member, values.len(), MAX_COLLECTION_ENTRIES)?;
    for value in values {
        bound_chars(member, value, MAX_STRING_CHARS)?;
    }
    Ok(())
}

/// Refuse an optional free-form string past [`MAX_STRING_CHARS`].
pub fn bound_optional_string(member: &str, value: Option<&String>) -> Result<(), String> {
    match value {
        Some(value) => bound_chars(member, value, MAX_STRING_CHARS),
        None => Ok(()),
    }
}

/// How long after the create call the `creating` -> `available` transition is owed.
///
/// AWS takes minutes; this emulator owes it inside the window the project fixes for a
/// clock-driven transition, so a caller that acts immediately after the create still
/// observes `creating` and the state errors that go with it. Nothing ever sleeps: the due
/// instant is stored on the record and [`ComAmazonawsDocdbAmazonrdsv19State::settle_clock`]
/// materialises it on the next call that reports the resource.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(300);

/// Alphabet for a minted resource-id token: the uppercase letters and digits AWS uses in
/// a `DbClusterResourceId` / `DbiResourceId`, as a 32-symbol set so the token is a plain
/// base-32 rendering of the monotonic counter.
const TOKEN_ALPHABET: [u8; 32] = *b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Token length, matching the fixed-width tokens AWS mints.
const TOKEN_LEN: usize = 12;

/// One instance's membership of a cluster.
#[derive(Debug, Clone)]
pub struct ClusterMember {
    /// The member instance's identifier, the key into [`ComAmazonawsDocdbAmazonrdsv19State::instances`].
    pub db_instance_identifier: String,
    /// Whether this member is the cluster's writer (its primary).
    pub is_cluster_writer: bool,
    /// Failover promotion order; see [`DEFAULT_PROMOTION_TIER`].
    pub promotion_tier: i32,
}

/// A persisted Amazon DocumentDB cluster.
///
/// The identifier is stored lowercased (`DBClusterIdentifier`: "This parameter is stored
/// as a lowercase string"), and is also this record's key in
/// [`ComAmazonawsDocdbAmazonrdsv19State::clusters`]. The ARN, and the endpoint host names of
/// a cluster this emulator serves no data plane for, are DERIVED from the identifier rather
/// than stored, because renaming a cluster through `ModifyDBCluster` moves them; the address a
/// SERVED cluster publishes is its socket's and is stored, because a rename does not move a
/// socket.
#[derive(Debug, Clone)]
pub struct DbClusterRecord {
    /// The lowercased `DBClusterIdentifier`.
    pub identifier: String,
    /// `DbClusterResourceId`, minted once and stable across a rename.
    pub resource_id: String,
    /// The region whose credential scope the create was signed with; the ARN and the
    /// endpoint host names sit in it.
    pub region: String,
    /// The DNS suffix shared by this cluster's endpoints and its instances' endpoints:
    /// `<token>.<region>.docdb.amazonaws.com`.
    pub endpoint_suffix: String,
    /// The address this cluster's OWN socket answers on, published as `Endpoint` and
    /// `ReaderEndpoint` where this emulator serves the cluster's data plane
    /// ([`cluster_data_plane::serve_cluster`]). `None` for a cluster it serves none for, which
    /// publishes the names Amazon DocumentDB's own zone would carry instead.
    pub served_endpoint: Option<ServedEndpoint>,
    /// `Status`.
    pub status: String,
    /// When the `creating` -> `available` transition falls due; `None` once settled.
    pub available_at: Option<Instant>,
    /// `Engine`.
    pub engine: String,
    /// `EngineVersion`.
    pub engine_version: String,
    /// `Port`.
    pub port: i32,
    /// `MasterUsername`.
    pub master_username: Option<String>,
    /// `ClusterCreateTime`.
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    /// `BackupRetentionPeriod`.
    pub backup_retention_period: i32,
    /// `PreferredBackupWindow`.
    pub preferred_backup_window: String,
    /// `PreferredMaintenanceWindow`.
    pub preferred_maintenance_window: String,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `KmsKeyId`.
    pub kms_key_id: Option<String>,
    /// `DeletionProtection`.
    pub deletion_protection: bool,
    /// `AvailabilityZones`.
    pub availability_zones: Vec<String>,
    /// The ids behind `VpcSecurityGroups`.
    pub vpc_security_group_ids: Vec<String>,
    /// `DBSubnetGroup`.
    pub db_subnet_group: Option<String>,
    /// `DBClusterParameterGroup`.
    pub db_cluster_parameter_group: Option<String>,
    /// `EnabledCloudwatchLogsExports`.
    pub enabled_cloudwatch_logs_exports: Vec<String>,
    /// `StorageType`, kept only when it is not the default; see [`DEFAULT_STORAGE_TYPE`].
    pub storage_type: Option<String>,
    /// `NetworkType`.
    pub network_type: Option<String>,
    /// `CopyTagsToSnapshot`.
    pub copy_tags_to_snapshot: bool,
    /// `ServerlessV2ScalingConfiguration` as (min capacity, max capacity).
    pub serverless_v2_scaling: Option<(f64, f64)>,
    /// `AutoMinorVersionUpgrade`; see [`DEFAULT_CLUSTER_AUTO_MINOR_VERSION_UPGRADE`].
    pub auto_minor_version_upgrade: bool,
    /// `DBActivityStreamStatus`; see [`ACTIVITY_STREAM_STOPPED`].
    pub activity_stream_status: String,
    /// `DocdbAnalyticsEnabled`; see [`DEFAULT_DOCDB_ANALYTICS_ENABLED`].
    pub docdb_analytics_enabled: bool,
    /// `CustomDataGroupPresent`; see [`DEFAULT_CUSTOM_DATA_GROUP_PRESENT`].
    pub custom_data_group_present: bool,
    /// `VPCNetworkingEnabled`; see [`DEFAULT_VPC_NETWORKING_ENABLED`].
    pub vpc_networking_enabled: bool,
    /// `InternetAccessGatewayEnabled`; see [`DEFAULT_INTERNET_ACCESS_GATEWAY_ENABLED`].
    pub internet_access_gateway_enabled: bool,
    /// `HostedZoneId`: the Route 53 hosted zone this cluster's endpoints live in, minted
    /// once from the cluster's Region; see [`hosted_zone_id`].
    pub hosted_zone_id: String,
    /// `DBClusterMembers`, in the order the instances joined.
    pub members: Vec<ClusterMember>,
    /// The cluster's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbClusterRecord {
    /// `DBClusterArn`, derived so a rename moves it.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster:{}", self.identifier),
        )
    }

    /// `Endpoint`: the cluster (writer) endpoint.
    ///
    /// The HOST this emulator answers the cluster's data plane at where it serves one, and the
    /// name the cluster's own control plane would give it where it serves none: an endpoint is
    /// an address the service ANSWERS on, and a name in Amazon DocumentDB's own DNS zone
    /// resolves to nothing on the machine this emulator runs on.
    ///
    /// A HOST and nothing else: `DBCluster` gives the cluster one `Port` member for both of its
    /// endpoints ([`Self::published_port`]), so neither carries a port of its own.
    pub fn endpoint(&self) -> String {
        match &self.served_endpoint {
            Some(served) => served.host.clone(),
            None => format!("{}.cluster-{}", self.identifier, self.endpoint_suffix),
        }
    }

    /// `ReaderEndpoint`: the load-balanced reader endpoint.
    ///
    /// The address this emulator answers the cluster's data plane at for a cluster it serves
    /// one for, exactly as [`Self::endpoint`] publishes it: the read path and the write path of
    /// one cluster are two endpoints in front of ONE plane, and the host a client dials to
    /// reach that plane is the host this emulator answers it on. A cluster this emulator serves
    /// no plane for keeps the name its own control plane would give it, under the published
    /// `cluster-ro-` label, so the two names differ in that label and in nothing else.
    pub fn reader_endpoint(&self) -> String {
        match &self.served_endpoint {
            Some(served) => served.host.clone(),
            None => format!("{}.cluster-ro-{}", self.identifier, self.endpoint_suffix),
        }
    }

    /// `Port`: "the port that the database engine is listening on" — the port this cluster was
    /// created with, or the one a modify moved it to.
    ///
    /// The configured port rather than the one the core assigned the cluster's socket: it is
    /// the one port the model gives the cluster's two endpoint members, so it is what a client
    /// completes the published host with, and the address it composes that way is recorded as
    /// one this emulator answers the cluster at
    /// ([`cluster_data_plane::open_endpoint_data_plane`]).
    pub fn published_port(&self) -> i32 {
        self.port
    }

    /// Materialise the `creating` -> `available` transition if the clock already owes it.
    fn settle(&mut self, now: Instant) {
        if self.available_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.available_at = None;
        }
    }
}

/// A persisted Amazon DocumentDB instance: one member of exactly one cluster.
///
/// Like a cluster, the identifier is stored lowercased and is this record's key. Values a
/// member inherits from its cluster (port, backup settings, encryption, security groups)
/// are NOT copied here: they are read from the cluster when the instance is reported, so
/// a `ModifyDBCluster` is visible on the members too.
#[derive(Debug, Clone)]
pub struct DbInstanceRecord {
    /// The lowercased `DBInstanceIdentifier`.
    pub identifier: String,
    /// `DbiResourceId`.
    pub dbi_resource_id: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// `DBInstanceClass`.
    pub instance_class: String,
    /// `Engine`.
    pub engine: String,
    /// `DBInstanceStatus`.
    pub status: String,
    /// When the `creating` -> `available` transition falls due; `None` once settled.
    pub available_at: Option<Instant>,
    /// The key of the cluster this instance belongs to.
    pub cluster_identifier: String,
    /// `AvailabilityZone`.
    pub availability_zone: String,
    /// `PromotionTier`.
    pub promotion_tier: i32,
    /// `Endpoint$Address`, resolved when the instance was created; see
    /// [`announced_address`].
    pub endpoint_address: String,
    /// The address this instance's own socket answers on, published as `Endpoint$Address` and
    /// `Endpoint$Port` where this emulator serves the instance's data plane
    /// ([`cluster_data_plane`]). `None` for an instance it serves none for, which publishes
    /// [`Self::endpoint_address`] and its cluster's port instead.
    pub served_endpoint: Option<ServedEndpoint>,
    /// `InstanceCreateTime`.
    pub instance_create_time: ::aws_smithy_types::DateTime,
    /// `PreferredMaintenanceWindow`.
    pub preferred_maintenance_window: String,
    /// `AutoMinorVersionUpgrade`.
    pub auto_minor_version_upgrade: bool,
    /// `CopyTagsToSnapshot`.
    pub copy_tags_to_snapshot: bool,
    /// `PerformanceInsightsEnabled`.
    pub performance_insights_enabled: bool,
    /// `PerformanceInsightsKMSKeyId`.
    pub performance_insights_kms_key_id: Option<String>,
    /// `CACertificateIdentifier`.
    pub ca_certificate_identifier: Option<String>,
    /// The instance's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbInstanceRecord {
    /// `DBInstanceArn`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("db:{}", self.identifier),
        )
    }

    /// `Endpoint$Address`: the host a client connects to.
    ///
    /// The address the instance's own socket answers on where this emulator serves its data
    /// plane, and the name its control plane publishes where it serves none — an endpoint is
    /// an address the service answers on, so a served instance publishes the address a client
    /// of this emulator reaches it at.
    pub fn endpoint_address(&self) -> String {
        match &self.served_endpoint {
            Some(served) => served.host.clone(),
            None => self.endpoint_address.clone(),
        }
    }

    /// `Endpoint$Port`: the port the database engine is listening on.
    ///
    /// The port the core assigned this instance's own socket where this emulator serves its
    /// data plane — the pair is ONE address a client dials, so the port is the socket's as the
    /// host is — and the port its cluster publishes for the engine where it serves none.
    pub fn endpoint_port(&self, cluster_port: i32) -> i32 {
        match &self.served_endpoint {
            Some(served) => served.port,
            None => cluster_port,
        }
    }

    /// Materialise the `creating` -> `available` transition if the clock already owes it.
    fn settle(&mut self, now: Instant) {
        if self.available_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.available_at = None;
        }
    }
}

/// A persisted cluster snapshot.
///
/// `DeleteDBCluster` writes one when the caller does not skip the final snapshot, and
/// `CopyDBClusterSnapshot` writes a copy of one, so a snapshot the API says exists really
/// is in state. `DescribeDBClusterSnapshots`, `DeleteDBClusterSnapshot`,
/// `DescribeDBClusterSnapshotAttributes` and `ModifyDBClusterSnapshotAttribute` read and
/// write exactly these records.
#[derive(Debug, Clone)]
pub struct DbClusterSnapshotRecord {
    /// `DBClusterSnapshotIdentifier`, lowercased, and this record's key. The member's own
    /// documentation does not repeat the "stored as a lowercase string" sentence the
    /// cluster and instance identifiers carry, but keying it the same way is what makes
    /// the duplicate check case-insensitive, as it is for every other identifier here.
    pub identifier: String,
    /// `DBClusterIdentifier` of the cluster the snapshot was taken from.
    pub cluster_identifier: String,
    /// The region the snapshot's ARN sits in.
    pub region: String,
    /// `SnapshotCreateTime`.
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// `ClusterCreateTime`, carried over from the cluster.
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    /// `Engine`.
    pub engine: String,
    /// `EngineVersion`.
    pub engine_version: String,
    /// `MasterUsername`.
    pub master_username: Option<String>,
    /// `Port`.
    pub port: i32,
    /// `AvailabilityZones`.
    pub availability_zones: Vec<String>,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `KmsKeyId`.
    pub kms_key_id: Option<String>,
    /// `StorageType`.
    pub storage_type: Option<String>,
    /// `SnapshotType`; see [`SNAPSHOT_TYPE_MANUAL`].
    pub snapshot_type: String,
    /// `Status`: [`STATUS_CREATING`] for a final snapshot being taken,
    /// [`SNAPSHOT_STATUS_COPYING`] for a copy still being made, [`STATUS_AVAILABLE`] once
    /// the clock has made it due.
    pub status: String,
    /// When the transition to [`STATUS_AVAILABLE`] falls due; `None` once settled.
    pub available_at: Option<Instant>,
    /// `SourceDBClusterSnapshotArn`: the snapshot this one was copied from, absent for a
    /// snapshot taken from a cluster.
    pub source_snapshot_arn: Option<String>,
    /// The accounts the `restore` attribute authorizes, which
    /// `ModifyDBClusterSnapshotAttribute` adds to and removes from and
    /// `DescribeDBClusterSnapshotAttributes` reports; see [`SNAPSHOT_RESTORE_ATTRIBUTE`].
    /// A set, so adding a value already present changes nothing and the report is ordered.
    pub restore_attribute: BTreeSet<String>,
    /// The snapshot's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbClusterSnapshotRecord {
    /// `DBClusterSnapshotArn`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-snapshot:{}", self.identifier),
        )
    }

    /// Whether this snapshot is public — its `restore` attribute authorizes every
    /// account; see [`SNAPSHOT_ATTRIBUTE_ALL`].
    pub fn is_public(&self) -> bool {
        self.restore_attribute.contains(SNAPSHOT_ATTRIBUTE_ALL)
    }

    /// Materialise the transition to `available` if the clock already owes it.
    fn settle(&mut self, now: Instant) {
        if self.available_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.available_at = None;
        }
    }

    /// This record's `restore` attribute as the modeled `DBClusterSnapshotAttributesResult`.
    ///
    /// ONE projection, so `DescribeDBClusterSnapshotAttributes` and
    /// `ModifyDBClusterSnapshotAttribute` cannot report the same record differently. The
    /// attribute is ALWAYS reported: a snapshot nobody has been authorized for carries it
    /// with an empty value list, which is how a caller reads "shared with no account".
    pub fn restore_attribute_to_shape(&self) -> DBClusterSnapshotAttributesResult {
        DBClusterSnapshotAttributesResult {
            db_cluster_snapshot_identifier: Some(self.identifier.clone()),
            db_cluster_snapshot_attributes: Some(vec![DBClusterSnapshotAttribute {
                attribute_name: Some(SNAPSHOT_RESTORE_ATTRIBUTE.to_string()),
                attribute_values: Some(self.restore_attribute.iter().cloned().collect()),
            }]),
        }
    }

    /// This record as the modeled `DBClusterSnapshot`.
    pub fn to_shape(&self) -> DBClusterSnapshot {
        DBClusterSnapshot {
            availability_zones: Some(self.availability_zones.clone()),
            db_cluster_snapshot_identifier: Some(self.identifier.clone()),
            db_cluster_identifier: Some(self.cluster_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            status: Some(self.status.clone()),
            port: Some(self.port),
            vpc_id: None,
            cluster_create_time: Some(self.cluster_create_time),
            master_username: self.master_username.clone(),
            engine_version: Some(self.engine_version.clone()),
            snapshot_type: Some(self.snapshot_type.clone()),
            // The progress of the copy this record stands for: complete once the snapshot
            // is available, and not yet begun while it is still being taken.
            percent_progress: Some(if self.status == STATUS_AVAILABLE { 100 } else { 0 }),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_cluster_snapshot_arn: Some(self.arn()),
            source_db_cluster_snapshot_arn: self.source_snapshot_arn.clone(),
            storage_type: self.storage_type.clone(),
        }
    }
}

/// A persisted subnet group: the set of VPC subnets a cluster's instances may be placed
/// in. Created by `CreateDBSubnetGroup`, reported by `DescribeDBSubnetGroups`, named by
/// `CreateDBCluster`'s `DBSubnetGroupName`, and reclaimed by `DeleteDBSubnetGroup`.
///
/// The name is stored lowercased (`DBSubnetGroupName`: "This value is stored as a lowercase
/// string") and is this record's key in
/// [`ComAmazonawsDocdbAmazonrdsv19State::subnet_groups`]. The ARN is derived from the name
/// rather than stored.
#[derive(Debug, Clone)]
pub struct DbSubnetGroupRecord {
    /// The lowercased `DBSubnetGroupName`.
    pub name: String,
    /// `DBSubnetGroupDescription`.
    pub description: String,
    /// The region whose credential scope the create was signed with; the ARN sits in it.
    pub region: String,
    /// The `SubnetIds` the caller named, in request order.
    pub subnet_ids: Vec<String>,
    /// The Availability Zone of each of [`Self::subnet_ids`], as the service that OWNS
    /// subnets answered when the group was written ([`ec2_subnets`]). A subnet EC2 named no
    /// zone for has no entry, and the group reports no zone for it rather than one chosen
    /// here.
    pub subnet_zones: BTreeMap<String, String>,
    /// `DBSubnetGroup$VpcId`: the VPC EC2 reports owns every one of this group's subnets.
    /// Absent where it answered for none of them.
    pub vpc_id: Option<String>,
    /// `DBSubnetGroup$SupportedNetworkTypes`, from the address families EC2 reports this
    /// group's subnets are addressed in; see [`ec2_subnets::group_network_types`].
    pub supported_network_types: Vec<String>,
    /// The group's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbSubnetGroupRecord {
    /// `DBSubnetGroupArn`, derived from the name.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("subgrp:{}", self.name),
        )
    }

    /// This record as the modeled `DBSubnetGroup`.
    ///
    /// The placement members — `VpcId`, `SupportedNetworkTypes` and each subnet's
    /// `SubnetAvailabilityZone` — are EC2's facts about the subnets this group holds, read
    /// off its records when the group was created or modified ([`ec2_subnets`]) and kept on
    /// the record, so the create, the modify and every later read project the ONE resolved
    /// placement. A fact EC2 answered none of is reported as an absent member rather than as
    /// a value invented here.
    pub fn to_shape(&self) -> DBSubnetGroup {
        DBSubnetGroup {
            db_subnet_group_name: Some(self.name.clone()),
            db_subnet_group_description: Some(self.description.clone()),
            vpc_id: self.vpc_id.clone(),
            subnet_group_status: Some(SUBNET_GROUP_COMPLETE.to_string()),
            subnets: Some(
                self.subnet_ids
                    .iter()
                    .map(|id| Subnet {
                        subnet_identifier: Some(id.clone()),
                        subnet_availability_zone: self.subnet_zones.get(id).map(|zone| {
                            AvailabilityZone {
                                name: Some(zone.clone()),
                            }
                        }),
                        subnet_status: Some(SUBNET_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            db_subnet_group_arn: Some(self.arn()),
            supported_network_types: (!self.supported_network_types.is_empty())
                .then(|| self.supported_network_types.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-docdb-amazonrdsv19. A handler reaches it through [`service_state`].
///
/// Every map is ordered, so a describe and the pagination marker it hands back are stable
/// across calls.
#[derive(Debug, Default)]
pub struct ComAmazonawsDocdbAmazonrdsv19State {
    /// Clusters, keyed by lowercased `DBClusterIdentifier`.
    pub clusters: BTreeMap<String, DbClusterRecord>,
    /// Instances, keyed by lowercased `DBInstanceIdentifier`.
    pub instances: BTreeMap<String, DbInstanceRecord>,
    /// Final cluster snapshots, keyed by `DBClusterSnapshotIdentifier`.
    pub snapshots: BTreeMap<String, DbClusterSnapshotRecord>,
    /// Cluster parameter groups, keyed by their lowercased
    /// `DBClusterParameterGroupName`.
    pub cluster_parameter_groups: BTreeMap<String, DbClusterParameterGroupRecord>,
    /// Subnet groups, keyed by lowercased `DBSubnetGroupName`.
    pub subnet_groups: BTreeMap<String, DbSubnetGroupRecord>,
    /// The sockets this emulator has open for its instances, keyed by the instance they
    /// serve. Kept beside the records because a live socket is not a cloneable value; see
    /// [`cluster_data_plane`].
    pub instance_data_planes: ServedDataPlanes,
    /// The sockets this emulator has open for its clusters' OWN endpoints, keyed by the
    /// cluster's immutable `DbClusterResourceId` — which a rename does not move — for the same
    /// reason; see [`cluster_data_plane::serve_cluster`].
    pub cluster_data_planes: ServedDataPlanes,
    /// Monotonic counter behind every minted resource id, so ids are deterministic.
    pub next_resource_id: u64,
}

impl ComAmazonawsDocdbAmazonrdsv19State {
    /// Take the next value of the monotonic resource-id counter.
    pub fn next_resource_ordinal(&mut self) -> u64 {
        self.next_resource_id += 1;
        self.next_resource_id
    }

    /// Materialise every `creating` -> `available` transition the clock already owes.
    ///
    /// A reporting operation calls this: the transition was due at the same instant
    /// whether or not anybody asked, so settling it on read changes nothing a later
    /// caller would not have seen anyway.
    pub fn settle_clock(&mut self) {
        let now = Instant::now();
        for cluster in self.clusters.values_mut() {
            cluster.settle(now);
        }
        for instance in self.instances.values_mut() {
            instance.settle(now);
        }
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
    }

    /// The key of the cluster `value` names: either its (case-insensitive) identifier or
    /// its ARN, both of which `DBClusterIdentifier` and the `db-cluster-id` filter accept.
    pub fn resolve_cluster_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.clusters.contains_key(&lowered) {
            return Some(lowered);
        }
        // Only an ARN can match the scan below, so a plain identifier that named nothing
        // does not pay for rendering every stored record's ARN.
        if !value.starts_with("arn:") {
            return None;
        }
        self.clusters
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.identifier.clone())
    }

    /// The key of the cluster snapshot `value` names: either its (case-insensitive)
    /// identifier or its ARN, both of which `DBClusterSnapshotIdentifier` and
    /// `SourceDBClusterSnapshotIdentifier` accept ("you can specify ... a valid cluster
    /// snapshot ARN").
    pub fn resolve_snapshot_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.snapshots.contains_key(&lowered) {
            return Some(lowered);
        }
        // See [`Self::resolve_cluster_key`]: only an ARN can match the scan.
        if !value.starts_with("arn:") {
            return None;
        }
        self.snapshots
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.identifier.clone())
    }

    /// The key of the instance `value` names: either its identifier or its ARN.
    pub fn resolve_instance_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.instances.contains_key(&lowered) {
            return Some(lowered);
        }
        // See [`Self::resolve_cluster_key`]: only an ARN can match the scan.
        if !value.starts_with("arn:") {
            return None;
        }
        self.instances
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.identifier.clone())
    }

    /// The key of the subnet group `value` names: either its (case-insensitive) name or its
    /// ARN, the two forms every `DBSubnetGroupName` member accepts.
    pub fn resolve_subnet_group_key(&self, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self.subnet_groups.contains_key(&lowered) {
            return Some(lowered);
        }
        // See [`Self::resolve_cluster_key`]: only an ARN can match the scan.
        if !value.starts_with("arn:") {
            return None;
        }
        self.subnet_groups
            .values()
            .find(|record| record.arn() == value)
            .map(|record| record.name.clone())
    }

    /// The identifiers of the clusters that are placed in the subnet group keyed `key`.
    ///
    /// `DeleteDBSubnetGroup`: "The specified database subnet group must not be associated
    /// with any DB instances", so a group with users is refused rather than deleted.
    pub fn subnet_group_users(&self, key: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| {
                cluster
                    .db_subnet_group
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(key))
            })
            .map(|cluster| cluster.identifier.clone())
            .collect()
    }

    /// The identifiers of the clusters that carry the cluster parameter group `name`, in
    /// identifier order. A group one of them carries is in use, so it cannot be deleted.
    ///
    /// `DBClusterParameterGroupName` is stored lowercased on the group, and a cluster's
    /// reference is matched case-insensitively against it for the same reason.
    pub fn clusters_using_parameter_group(&self, name: &str) -> Vec<String> {
        self.clusters
            .values()
            .filter(|cluster| {
                cluster
                    .db_cluster_parameter_group
                    .as_deref()
                    .is_some_and(|carried| carried.eq_ignore_ascii_case(name))
            })
            .map(|cluster| cluster.identifier.clone())
            .collect()
    }

    /// Give the cluster a writer again after one was removed: the surviving member with
    /// the lowest `PromotionTier` is promoted, which is the order `PromotionTier`
    /// documents ("the order in which an Amazon DocumentDB replica is promoted to the
    /// primary instance after a failure of the existing primary instance").
    pub fn reelect_writer(&mut self, cluster_key: &str) {
        let Some(cluster) = self.clusters.get_mut(cluster_key) else {
            return;
        };
        if cluster.members.iter().any(|member| member.is_cluster_writer) {
            return;
        }
        let winner = cluster
            .members
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.promotion_tier
                    .cmp(&right.promotion_tier)
                    .then_with(|| left.db_instance_identifier.cmp(&right.db_instance_identifier))
            })
            .map(|(index, _)| index);
        if let Some(index) = winner {
            cluster.members[index].is_cluster_writer = true;
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDocdbAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDocdbAmazonrdsv19State>)
}

/// The SHARED data-source directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A monitoring service addresses a DB resource by the immutable resource id THIS
/// service minted for it (`DbClusterResourceId`, `DbiResourceId`), and the form of
/// that id does not say which service minted it — Amazon RDS mints ids of the same
/// forms. So a minted id is recorded here, in ONE directory under one shared type
/// ([`::service_common::db_data_sources::DataSourceDirectory`]), which is how
/// Performance Insights can decide whether its request's `ServiceType` is the service
/// that owns the resource the request names. The engine keys state by TYPE, so a copy
/// of its own inside [`ComAmazonawsDocdbAmazonrdsv19State`] would be invisible to the
/// readers.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and
/// no reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to another
/// service, never part of this operation's own answer.
pub fn data_source_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::db_data_sources::DataSourceDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::db_data_sources::DataSourceDirectory,
            >,
        )
}

/// The region the caller signed this request for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the modeled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The id one response reports as its own: the value of the `<RequestId>` element an
/// awsQuery `<ErrorResponse>` envelope carries, the counterpart of the
/// `ResponseMetadata/RequestId` the framework's success renderer writes.
///
/// A correlation id, not a security token, and per-response rather than per-resource: two
/// responses must be distinguishable, which is what a client logs and an SDK's retry and
/// telemetry layers read. It needs no state, so it is a free function rather than a method
/// on the store — a request refused before any store is reached still answers with one.
///
/// The form is the 8-4-4-4-12 version-4 UUID every AWS request id carries, the same shape
/// the framework's own success-path minter produces, so a client cannot tell the two apart.
/// Unique because a monotone counter goes into the digest; the digest is the maintained
/// `sha2` crate's, so nothing here is hand-rolled.
pub fn new_request_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// The monotone part of a request id, so two ids minted inside one clock tick still
    /// differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let digest = Sha256::digest(format!("request:{counter}:{nanos}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version (4) and variant (RFC 4122) nibbles, so the id is well formed and not
    // mistaken for a name-based or time-based one.
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

/// The fixed-width token behind the `n`th minted resource id, as a base-32 rendering of
/// `n` over [`TOKEN_ALPHABET`]. Deterministic, so two runs of the same call sequence mint
/// the same ids.
pub fn resource_token(ordinal: u64) -> String {
    let mut digits = [TOKEN_ALPHABET[0]; TOKEN_LEN];
    let mut remaining = ordinal;
    for slot in digits.iter_mut().rev() {
        *slot = TOKEN_ALPHABET[(remaining % TOKEN_ALPHABET.len() as u64) as usize];
        remaining /= TOKEN_ALPHABET.len() as u64;
    }
    digits.iter().map(|byte| char::from(*byte)).collect()
}

/// How many Availability Zones of its Region a DocumentDB cluster spreads its storage
/// over: three copies in three zones, whatever the Region's own zone count is ("Amazon
/// DocumentDB stores six copies of your data across three Availability Zones").
pub const CLUSTER_AVAILABILITY_ZONES: usize = 3;

/// The Availability Zones a cluster lands in when `AvailabilityZones` is absent: the first
/// [`CLUSTER_AVAILABILITY_ZONES`] zones of the request's Region
/// ([`::service_common::aws_availability_zones`]). A cluster spans three zones even in a
/// Region that exposes more, which is why this is a prefix of the Region's zones and not the
/// whole list an orderable instance option reports.
pub fn default_availability_zones(region: &str) -> Vec<String> {
    let mut zones = ::service_common::aws_availability_zones::availability_zones(region);
    zones.truncate(CLUSTER_AVAILABILITY_ZONES);
    zones
}

/// The `PreferredBackupWindow` a cluster gets when the caller names none.
///
/// `CreateDBClusterMessage$PreferredBackupWindow`: "The default is a 30-minute window
/// selected at random from an 8-hour block of time for each Amazon Web Services Region."
/// The emulator picks it from the cluster's ordinal instead of at random, so it is stable
/// for the life of the cluster, and keeps the documented `hh24:mi-hh24:mi` shape.
pub fn default_backup_window(ordinal: u64) -> String {
    let hour = (ordinal % 8) as u32;
    format!("{hour:02}:00-{hour:02}:30")
}

/// The `PreferredMaintenanceWindow` a cluster or instance gets when the caller names none.
///
/// `CreateDBClusterMessage$PreferredMaintenanceWindow`: "The default is a 30-minute window
/// selected at random from an 8-hour block of time for each Amazon Web Services Region,
/// occurring on a random day of the week ... Format: ddd:hh24:mi-ddd:hh24:mi". It must
/// "not conflict with the preferred backup window", so the hour is taken from the block
/// after [`default_backup_window`]'s.
pub fn default_maintenance_window(ordinal: u64) -> String {
    // The weekdays `PreferredMaintenanceWindow` lists: "Valid days: Mon, Tue, Wed, Thu,
    // Fri, Sat, Sun".
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let day = DAYS[(ordinal % DAYS.len() as u64) as usize];
    let hour = (ordinal % 8) as u32 + 8;
    format!("{day}:{hour:02}:00-{day}:{hour:02}:30")
}

/// Refuse a value that breaks the identifier rule every DocumentDB identifier shares; see
/// [`MAX_IDENTIFIER_LEN`] for the documentation it comes from.
pub fn validate_identifier(member: &str, value: &str) -> Result<(), String> {
    validate_identifier_within(member, value, MAX_IDENTIFIER_LEN)
}

/// The identifier rule with an explicit length bound, for the members whose own
/// documentation states a different one (see [`MAX_SNAPSHOT_IDENTIFIER_LEN`]).
///
/// The refusal is [`not_a_valid_identifier_message`]: the ONE sentence pair this front door
/// words every identifier refusal with — it names the member at fault, states the whole
/// rule and echoes nothing of the rejected value.
pub fn validate_identifier_within(member: &str, value: &str, max_len: usize) -> Result<(), String> {
    let well_formed = !value.is_empty()
        && value.chars().count() <= max_len
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
    if well_formed {
        return Ok(());
    }
    Err(not_a_valid_identifier_message(member))
}

/// The region an ARN names — its fourth `:`-separated field — or `None` for a value that
/// is not an ARN.
///
/// `CopyDBClusterSnapshotMessage$SourceDBClusterSnapshotIdentifier`: "If the source
/// cluster snapshot is in the same Region as the copy, specify a valid snapshot
/// identifier. If the source cluster snapshot is in a different Region ... specify the
/// snapshot ARN." So the region a copy looked for its source in is the ARN's own when it
/// was given one, and the request's region otherwise.
pub fn arn_region(value: &str) -> Option<String> {
    let mut fields = value.split(':');
    let (Some("arn"), Some(_partition), Some(_service), Some(region)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    (!region.is_empty()).then(|| region.to_string())
}

/// Refuse a `restore` attribute value that is neither an Amazon Web Services account id
/// nor the `all` that makes a snapshot public; see [`SNAPSHOT_ATTRIBUTE_ALL`] and
/// [`ACCOUNT_ID_DIGITS`] for the documentation this comes from.
pub fn validate_snapshot_attribute_value(member: &str, value: &str) -> Result<(), String> {
    let well_formed = value == SNAPSHOT_ATTRIBUTE_ALL
        || (value.chars().count() == ACCOUNT_ID_DIGITS && value.chars().all(|c| c.is_ascii_digit()));
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: each value must be a \
         {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account id or \
         '{SNAPSHOT_ATTRIBUTE_ALL}'"
    ))
}

/// Refuse a member whose value is outside the list that member's own documentation
/// publishes, naming the member, the value and the values it admits.
///
/// Several members of this model are plain strings whose documentation states "Valid values:
/// …", so the boundary validator cannot refuse one and each handler has to — and every one of
/// them refuses it in this ONE sentence, so a client reads the same wording whichever member it
/// got wrong.
pub fn validate_value_from_list(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: valid values are {}",
        allowed.join(", ")
    ))
}

/// Refuse a `SnapshotType` outside the documented list; see [`SNAPSHOT_TYPES`].
pub fn validate_snapshot_type(value: &str) -> Result<(), String> {
    validate_value_from_list("SnapshotType", value, &SNAPSHOT_TYPES)
}

/// Refuse an `Engine` that is not the one Amazon DocumentDB runs; see [`DOCDB_ENGINE`].
pub fn validate_engine(value: &str) -> Result<(), String> {
    if value.eq_ignore_ascii_case(DOCDB_ENGINE) {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for Engine: the only valid value is '{DOCDB_ENGINE}'"
    ))
}

/// Refuse a subnet-group name outside its documented rule; see
/// [`MAX_SUBNET_GROUP_NAME_LEN`] and [`RESERVED_SUBNET_GROUP_NAME`].
pub fn validate_subnet_group_name(member: &str, value: &str) -> Result<(), String> {
    let allowed = |c: char| {
        c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == ' ' || c == '-'
    };
    let well_formed = !value.is_empty()
        && value.chars().count() <= MAX_SUBNET_GROUP_NAME_LEN
        && value.chars().all(allowed)
        && !value.eq_ignore_ascii_case(RESERVED_SUBNET_GROUP_NAME);
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: it must contain no more than \
         {MAX_SUBNET_GROUP_NAME_LEN} letters, numbers, periods, underscores, spaces, or \
         hyphens, and must not be '{RESERVED_SUBNET_GROUP_NAME}'"
    ))
}

/// The published engine version a requested `EngineVersion` names, or `None` when the
/// engine offers none.
///
/// A request may name a version as precisely as it likes: an exact published version
/// (`5.0.0`) or a leading component of one at a dot boundary (`5.0`, `5`), which is how a
/// caller asks for a LINE rather than a release and how `EngineVersion`'s own documentation
/// talks about "the intended major engine version". A line resolves to the release the
/// shared catalog DESIGNATES as that line's default
/// ([`::service_common::docdb_engine_catalog::line_default`]) — the same release the other
/// view of this front door provisions for a request naming only the line — and not to the
/// newest release the line publishes, which can be a patch AWS has designated nothing about.
/// Anything else names a version the engine cannot run.
pub fn resolve_engine_version(requested: &str) -> Option<&'static str> {
    use ::service_common::docdb_engine_catalog as catalog;

    if let Some(exact) = catalog::engine_version(requested) {
        return Some(exact.version);
    }
    if let Some(designated) = catalog::line_default(requested) {
        return Some(designated.version);
    }
    // A component SHORTER than a line (`5`) names every line that begins with it at a dot
    // boundary; the newest such line's designated default is the release it asks for.
    catalog::major_lines()
        .into_iter()
        .rev()
        .filter(|line| {
            line.strip_prefix(requested)
                .is_some_and(|rest| rest.starts_with('.'))
        })
        .find_map(catalog::line_default)
        .map(|designated| designated.version)
}

/// The `DBParameterGroupFamily` of an engine version: the engine name followed by the
/// version's `major.minor` line (`5.0.0` -> `docdb5.0`), which is the form
/// `DBClusterParameterGroup$DBParameterGroupFamily` reports.
pub fn parameter_group_family(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or(version);
    match parts.next() {
        Some(minor) => format!("{DOCDB_ENGINE}{major}.{minor}"),
        None => format!("{DOCDB_ENGINE}{major}"),
    }
}

/// The cluster parameter group every account and Region already has for `family`, as AWS
/// names it: `default.<family>`; see [`DEFAULT_PARAMETER_GROUP_PREFIX`].
pub fn default_parameter_group_name(family: &str) -> String {
    format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}")
}

/// The default cluster parameter groups the engine's published versions imply: one per
/// engine-version family, ordered by name, each sitting in `region`.
///
/// These are not caller-created records and so are not in the store: AWS provides one
/// default cluster parameter group per family in every account and Region, and
/// `DescribeDBClusterParameterGroups` reports them with no create having happened. A
/// caller-created group (`CreateDBClusterParameterGroup`, still scaffolded) will be a
/// stored record read alongside these.
pub fn default_parameter_groups(region: &str) -> Vec<DBClusterParameterGroup> {
    let mut families: Vec<String> = engine_versions()
        .iter()
        .map(|version| parameter_group_family(version))
        .collect();
    families.sort();
    families.dedup();
    families
        .iter()
        .map(|family| {
            let name = default_parameter_group_name(family);
            DBClusterParameterGroup {
                db_cluster_parameter_group_arn: Some(::engine::emulator_arn(
                    ARN_NAMESPACE,
                    region,
                    &format!("cluster-pg:{name}"),
                )),
                db_cluster_parameter_group_name: Some(name),
                db_parameter_group_family: Some(family.clone()),
                description: Some(format!("Default cluster parameter group for {family}")),
            }
        })
        .collect()
}

/// The cluster parameter group `name` names among the engine's DEFAULTS, or `None`.
///
/// Matching is case-insensitive on the name, as every parameter-group name member is.
/// A caller-CREATED group is a stored record, which [`resolve_parameter_group_in`] reads
/// alongside these; reach for that one wherever the store is in hand.
pub fn resolve_parameter_group(name: &str, region: &str) -> Option<DBClusterParameterGroup> {
    default_parameter_groups(region).into_iter().find(|group| {
        group
            .db_cluster_parameter_group_name
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(name))
    })
}

/// The cluster parameter group `name` names, whether a caller CREATED it
/// (`CreateDBClusterParameterGroup` stored it) or the engine provides it as the default of
/// a family, or `None` when there is no such group.
///
/// One resolver over both sets, so every operation that names a group by name — a cluster
/// create or modify, a describe — agrees about which groups exist.
pub fn resolve_parameter_group_in(
    store: &ComAmazonawsDocdbAmazonrdsv19State,
    name: &str,
    region: &str,
) -> Option<DBClusterParameterGroup> {
    store
        .cluster_parameter_groups
        .get(&name.to_ascii_lowercase())
        .map(DbClusterParameterGroupRecord::to_shape)
        .or_else(|| resolve_parameter_group(name, region))
}

/// Whether `family` is a parameter-group family THIS model publishes (`docdb5.0`).
///
/// A family belongs to one engine, so a request naming another engine's family
/// (`aurora-mysql8.0`) is not this model's to answer — see [`serves_parameter_group_name`],
/// which the shared `rds` front door reads for the same decision.
pub fn serves_parameter_group_family(family: &str) -> bool {
    engine_versions()
        .iter()
        .any(|version| parameter_group_family(version).eq_ignore_ascii_case(family))
}

/// Whether THIS model can answer for the cluster parameter group `name` at all.
///
/// A default group's name names its family (`default.docdb5.0`), and a family belongs to one
/// engine, so a `default.<family>` name is answerable only by the model that publishes that
/// family — this one answers for the DocumentDB families and for no other engine's. Any
/// other name is a caller-created group, and this service's store is the only cluster
/// parameter-group store in this emulator, so a name it does not hold is a group that does
/// not exist whichever engine asked.
///
/// The shared `rds` front door reads this to decide which co-located model serves a
/// parameter-group request (see
/// `services/service-com-amazonaws-rds-amazonrdsv19/src/service.rs`), so a caller on another
/// engine naming ITS family is not answered from this engine's catalogue.
pub fn serves_parameter_group_name(name: &str) -> bool {
    match name.strip_prefix(DEFAULT_PARAMETER_GROUP_PREFIX) {
        Some(family) => serves_parameter_group_family(family),
        None => true,
    }
}

/// Refuse a `MasterUsername` outside its documented rule; see [`MAX_MASTER_USERNAME_LEN`].
pub fn validate_master_username(value: &str) -> Result<(), String> {
    let well_formed = !value.is_empty()
        && value.chars().count() <= MAX_MASTER_USERNAME_LEN
        && value.chars().all(|c| c.is_ascii_alphanumeric())
        && value.starts_with(|c: char| c.is_ascii_alphabetic());
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for MasterUsername: it must be from 1 to \
         {MAX_MASTER_USERNAME_LEN} letters or numbers and the first character must be a letter"
    ))
}

/// Refuse a `MasterUserPassword` outside its documented rule; see
/// [`MIN_MASTER_USER_PASSWORD_LEN`] and [`MASTER_USER_PASSWORD_FORBIDDEN`]. The value
/// itself is never quoted back, and this service stores no form of it from which it could be
/// recovered: what a create keeps is the SCRAM credential a client's login is checked against
/// ([`cluster_data_plane::auth::ScramCredentials`]).
pub fn validate_master_user_password(value: &str) -> Result<(), String> {
    let length = value.chars().count();
    let printable_ascii = value.chars().all(|c| c.is_ascii() && !c.is_ascii_control());
    let well_formed = (MIN_MASTER_USER_PASSWORD_LEN..=MAX_MASTER_USER_PASSWORD_LEN)
        .contains(&length)
        && printable_ascii
        && !value.contains(|c: char| MASTER_USER_PASSWORD_FORBIDDEN.contains(&c));
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value for MasterUserPassword: it must contain from \
         {MIN_MASTER_USER_PASSWORD_LEN} to {MAX_MASTER_USER_PASSWORD_LEN} printable ASCII \
         characters and cannot contain '/', '\"' or '@'"
    ))
}

/// Refuse a `BackupRetentionPeriod` outside its documented range; see
/// [`MIN_BACKUP_RETENTION_PERIOD`].
pub fn validate_backup_retention_period(value: i32) -> Result<(), String> {
    if (MIN_BACKUP_RETENTION_PERIOD..=MAX_BACKUP_RETENTION_PERIOD).contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for BackupRetentionPeriod: it must be a value from \
         {MIN_BACKUP_RETENTION_PERIOD} to {MAX_BACKUP_RETENTION_PERIOD}"
    ))
}

/// Refuse a `PromotionTier` outside its documented range; see [`MIN_PROMOTION_TIER`].
pub fn validate_promotion_tier(value: i32) -> Result<(), String> {
    if (MIN_PROMOTION_TIER..=MAX_PROMOTION_TIER).contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for PromotionTier: valid values are \
         {MIN_PROMOTION_TIER}-{MAX_PROMOTION_TIER}"
    ))
}

/// The page size a describe must use: the caller's `MaxRecords` once it is inside the
/// documented range, else [`DEFAULT_MAX_RECORDS`]; see [`MIN_MAX_RECORDS`].
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    let Some(value) = max_records else {
        return Ok(DEFAULT_MAX_RECORDS as usize);
    };
    if (MIN_MAX_RECORDS..=MAX_MAX_RECORDS).contains(&value) {
        return Ok(value as usize);
    }
    Err(format!(
        "Invalid value {value} for MaxRecords: minimum {MIN_MAX_RECORDS}, maximum \
         {MAX_MAX_RECORDS}"
    ))
}

/// One page of an ordered key list, plus the `Marker` to report.
///
/// The `Marker` doc is what fixes the semantics: "An optional pagination token provided
/// by a previous request. If this parameter is specified, the response includes only
/// records BEYOND the marker" — so the token handed back is the LAST key of this page and
/// the next page starts strictly after it. The last page carries none, which is the round
/// trip `@paginated(inputToken: Marker, outputToken: Marker)` describes.
///
/// A marker that names no key in `keys` is taken as a position rather than refused: the
/// key it named can legitimately have been deleted between two pages, and the real
/// service answers the records beyond it rather than failing a caller that paged
/// correctly.
pub fn paginate(
    keys: &[String],
    marker: Option<&str>,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    let start = match marker {
        Some(from) => keys
            .iter()
            .position(|key| key.as_str() > from)
            .unwrap_or(keys.len()),
        None => 0,
    };
    let end = start.saturating_add(page_size).min(keys.len());
    let page = keys[start..end].to_vec();
    let next = (end < keys.len())
        .then(|| page.last().cloned())
        .flatten();
    (page, next)
}

/// One page of a PUBLISHED CATALOGUE, plus the `Marker` to report.
///
/// A catalogue is published in an order of its own — the strongest certificate authority
/// first, the engine's versions oldest first — which is not the order of any value in the
/// record, so the token this hands back names the POSITION the next page starts after
/// rather than a value out of the record. It is zero-padded so positions order the way
/// [`paginate`] compares its keys, and it is the same round trip `Marker` documents: "the
/// response includes only records beyond the marker".
pub fn paginate_catalog<T: Clone>(
    items: &[T],
    marker: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let keys: Vec<String> = (0..items.len()).map(catalog_page_key).collect();
    let (page_keys, next) = paginate(&keys, marker, page_size);
    let page = page_keys
        .iter()
        .filter_map(|key| key.parse::<usize>().ok())
        .filter_map(|position| items.get(position).cloned())
        .collect();
    (page, next)
}

/// The pagination token of the record at `position` in a published catalogue; see
/// [`paginate_catalog`].
fn catalog_page_key(position: usize) -> String {
    format!("{position:08}")
}

/// The major-version component of an engine version: everything before the first `.`, or
/// the whole string when it carries none. `ModifyDBCluster`'s `AllowMajorVersionUpgrade`
/// is what makes this a rule the service enforces.
pub fn engine_major_version(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// The `Tags` of a create request as the (key, value) pairs state keeps.
pub fn tag_pairs(tags: Option<Vec<Tag>>) -> Vec<(String, String)> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| {
            (
                tag.key.unwrap_or_default(),
                tag.value.unwrap_or_default(),
            )
        })
        .collect()
}

/// The Route 53 hosted zone the Region's Amazon DocumentDB endpoints live in. One zone per
/// Region, so every cluster in a Region reports the same id — as AWS does.
///
/// The id is DERIVED from the Region rather than looked up: AWS's per-Region zone ids are
/// published in its endpoint documentation but are in no part of this service's Smithy
/// model, and what the member promises (`DBCluster$HostedZoneId`, `Endpoint$HostedZoneId`:
/// "the ID that Amazon Route 53 assigns when you create a hosted zone") is a stable,
/// `Z`-prefixed id that does not change between reads. The mixing is the tree's one
/// derived-id mixer ([`::service_common::stable_hex_id`]), uppercased and cut to a leading
/// `Z` plus [`HOSTED_ZONE_ID_LEN`] characters — uppercase hexadecimal, a subset of the
/// uppercase alphanumerics AWS's own zone ids use. A client that compares the id against
/// AWS's published one for a Region sees a different label; it keeps the format, the
/// per-Region invariance and the stability the member promises.
pub fn hosted_zone_id(region: &str) -> String {
    let mixed: String = ::service_common::stable_hex_id(&["docdb-hosted-zone", region])
        .to_uppercase()
        .chars()
        .take(HOSTED_ZONE_ID_LEN)
        .collect();
    format!("Z{mixed}")
}

/// A cluster's point-in-time-restore window as (`EarliestRestorableTime`,
/// `LatestRestorableTime`), or `None` for a cluster that has no window yet.
///
/// The window is what `RestoreDBClusterToPointInTime` can restore inside:
/// `RestoreDBClusterToPointInTimeMessage$RestoreToTime` "must be before the latest
/// restorable time", and `UseLatestRestorableTime` restores to its upper end. Continuous
/// backup of a cluster only begins once the cluster has finished provisioning, so a
/// cluster still [`STATUS_CREATING`] reports no window at all, and a cluster whose
/// automated backups are off (a zero `BackupRetentionPeriod`) retains nothing to restore
/// from. Otherwise the window runs from the start of the retained backups —
/// `BackupRetentionPeriod` days back, and never before the cluster itself existed — up to
/// the instant it is read at, continuous backup covering everything up to now.
///
/// `now` is passed in so one call reports one consistent window on both ends.
pub fn cluster_restore_window(
    record: &DbClusterRecord,
    now: ::aws_smithy_types::DateTime,
) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
    if record.status == STATUS_CREATING || record.backup_retention_period <= 0 {
        return None;
    }
    let retained_from =
        now.as_secs_f64() - f64::from(record.backup_retention_period) * SECONDS_PER_DAY;
    let earliest = retained_from.max(record.cluster_create_time.as_secs_f64());
    Some((::aws_smithy_types::DateTime::from_secs_f64(earliest), now))
}

/// A cluster record as the modeled `DBCluster`.
///
/// `MultiAZ` is computed from the member instances' Availability Zones, and the endpoints
/// and ARN from the identifier, so nothing here is a value the handler was handed. The
/// point-in-time-restore window is computed the same way, from the record's creation
/// instant and retention period; see [`cluster_restore_window`].
pub fn cluster_to_shape(
    record: &DbClusterRecord,
    instances: &BTreeMap<String, DbInstanceRecord>,
) -> DBCluster {
    let zones_in_use: BTreeSet<&str> = record
        .members
        .iter()
        .filter_map(|member| instances.get(&member.db_instance_identifier))
        .map(|instance| instance.availability_zone.as_str())
        .collect();
    let restore_window = cluster_restore_window(record, now());
    DBCluster {
        // The members AWS's cluster envelope always carries, each read from the record so
        // one projection reports what every path stored; see
        // [`DEFAULT_CLUSTER_AUTO_MINOR_VERSION_UPGRADE`] and its neighbours. The
        // activity-stream status is carried under both of the names the envelope reports
        // it under, out of the one stored status.
        activity_stream_status: Some(record.activity_stream_status.clone()),
        auto_minor_version_upgrade: Some(record.auto_minor_version_upgrade),
        db_activity_stream_status: Some(record.activity_stream_status.clone()),
        docdb_analytics_enabled: Some(record.docdb_analytics_enabled),
        custom_data_group_present: Some(record.custom_data_group_present),
        vpc_networking_enabled: Some(record.vpc_networking_enabled),
        internet_access_gateway_enabled: Some(record.internet_access_gateway_enabled),
        availability_zones: Some(record.availability_zones.clone()),
        backup_retention_period: Some(record.backup_retention_period),
        db_cluster_identifier: Some(record.identifier.clone()),
        db_cluster_parameter_group: record.db_cluster_parameter_group.clone(),
        db_subnet_group: record.db_subnet_group.clone(),
        status: Some(record.status.clone()),
        percent_progress: None,
        earliest_restorable_time: restore_window.map(|(earliest, _)| earliest),
        endpoint: Some(record.endpoint()),
        reader_endpoint: Some(record.reader_endpoint()),
        multi_az: Some(zones_in_use.len() > 1),
        engine: Some(record.engine.clone()),
        engine_version: Some(record.engine_version.clone()),
        latest_restorable_time: restore_window.map(|(_, latest)| latest),
        port: Some(record.published_port()),
        master_username: record.master_username.clone(),
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
                    db_cluster_parameter_group_status: Some(PARAMETER_GROUP_IN_SYNC.to_string()),
                    promotion_tier: Some(member.promotion_tier),
                })
                .collect(),
        ),
        vpc_security_groups: Some(
            record
                .vpc_security_group_ids
                .iter()
                .map(|id| VpcSecurityGroupMembership {
                    vpc_security_group_id: Some(id.clone()),
                    status: Some(SECURITY_GROUP_ACTIVE.to_string()),
                })
                .collect(),
        ),
        hosted_zone_id: Some(record.hosted_zone_id.clone()),
        storage_encrypted: Some(record.storage_encrypted),
        kms_key_id: record.kms_key_id.clone(),
        db_cluster_resource_id: Some(record.resource_id.clone()),
        db_cluster_arn: Some(record.arn()),
        associated_roles: Some(Vec::new()),
        clone_group_id: None,
        cluster_create_time: Some(record.cluster_create_time),
        enabled_cloudwatch_logs_exports: (!record.enabled_cloudwatch_logs_exports.is_empty())
            .then(|| record.enabled_cloudwatch_logs_exports.clone()),
        deletion_protection: Some(record.deletion_protection),
        io_optimized_next_allowed_modification_time: None,
        storage_type: record.storage_type.clone(),
        serverless_v2_scaling_configuration: record.serverless_v2_scaling.map(|(min, max)| {
            ServerlessV2ScalingConfigurationInfo {
                min_capacity: Some(min),
                max_capacity: Some(max),
            }
        }),
        master_user_secret: None,
        network_type: record.network_type.clone(),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
    }
}

/// An instance record as the modeled `DBInstance`.
///
/// `cluster` is the instance's cluster, when it is still in state: the members that a
/// cluster owns (port, backup settings, encryption, security groups, engine version, the
/// master user name, the storage its data sits on, whether it runs in a virtual private
/// cloud, the Route 53 hosted zone its endpoints live in, and the point-in-time-restore
/// window its continuous backup covers) are read from it rather than copied at create
/// time, so a later `ModifyDBCluster` shows up on its instances too.
///
/// The members this engine fixes for every instance — it supports no public endpoint, no
/// HTTP endpoint, no instance-level port and no instance-level storage — are projected
/// from the constants that state each rule ([`INSTANCE_PUBLICLY_ACCESSIBLE`] and its
/// neighbours), so no path can report an instance as reachable from the internet or as
/// carrying provisioned storage of its own.
pub fn instance_to_shape(
    record: &DbInstanceRecord,
    cluster: Option<&DbClusterRecord>,
) -> DBInstance {
    DBInstance {
        db_instance_identifier: Some(record.identifier.clone()),
        db_instance_class: Some(record.instance_class.clone()),
        engine: Some(record.engine.clone()),
        db_instance_status: Some(record.status.clone()),
        // The storage the instance's data sits on is its CLUSTER's, so the type is the
        // cluster's own and the instance has neither a size nor a throughput of its own.
        allocated_storage: Some(INSTANCE_ALLOCATED_STORAGE),
        storage_type: Some(
            cluster
                .and_then(|owner| owner.storage_type.clone())
                .unwrap_or_else(|| DEFAULT_STORAGE_TYPE.to_string()),
        ),
        storage_throughput: Some(INSTANCE_STORAGE_THROUGHPUT),
        // The master user is the CLUSTER's: `CreateDBInstanceMessage` models no master-user
        // member at all, so an instance reports the user its cluster was created with.
        master_username: cluster.and_then(|owner| owner.master_username.clone()),
        license_model: Some(DOCDB_LICENSE_MODEL.to_string()),
        db_instance_port: Some(INSTANCE_PORT_UNSET),
        vpc_networking_enabled: Some(
            cluster.map_or(DEFAULT_VPC_NETWORKING_ENABLED, |owner| {
                owner.vpc_networking_enabled
            }),
        ),
        http_endpoint_enabled: Some(INSTANCE_HTTP_ENDPOINT_ENABLED),
        endpoint: Some(Endpoint {
            address: Some(record.endpoint_address()),
            port: Some(record.endpoint_port(cluster.map_or(DEFAULT_PORT, |owner| owner.port))),
            hosted_zone_id: cluster.map(|owner| owner.hosted_zone_id.clone()),
        }),
        instance_create_time: Some(record.instance_create_time),
        preferred_backup_window: cluster.map(|owner| owner.preferred_backup_window.clone()),
        backup_retention_period: cluster.map(|owner| owner.backup_retention_period),
        vpc_security_groups: cluster.map(|owner| {
            owner
                .vpc_security_group_ids
                .iter()
                .map(|id| VpcSecurityGroupMembership {
                    vpc_security_group_id: Some(id.clone()),
                    status: Some(SECURITY_GROUP_ACTIVE.to_string()),
                })
                .collect()
        }),
        availability_zone: Some(record.availability_zone.clone()),
        db_subnet_group: None,
        preferred_maintenance_window: Some(record.preferred_maintenance_window.clone()),
        // The changes to this instance that are still pending. The element is reported on
        // every instance, carrying a subelement per pending change and none at all for an
        // instance with nothing pending, which is how a caller reads "settled".
        pending_modified_values: Some(PendingModifiedValues::default()),
        latest_restorable_time: cluster
            .and_then(|owner| cluster_restore_window(owner, now()))
            .map(|(_, latest)| latest),
        engine_version: cluster.map(|owner| owner.engine_version.clone()),
        auto_minor_version_upgrade: Some(record.auto_minor_version_upgrade),
        publicly_accessible: Some(INSTANCE_PUBLICLY_ACCESSIBLE),
        status_infos: None,
        db_cluster_identifier: Some(record.cluster_identifier.clone()),
        storage_encrypted: cluster.map(|owner| owner.storage_encrypted),
        kms_key_id: cluster.and_then(|owner| owner.kms_key_id.clone()),
        dbi_resource_id: Some(record.dbi_resource_id.clone()),
        ca_certificate_identifier: record.ca_certificate_identifier.clone(),
        copy_tags_to_snapshot: Some(record.copy_tags_to_snapshot),
        promotion_tier: Some(record.promotion_tier),
        db_instance_arn: Some(record.arn()),
        enabled_cloudwatch_logs_exports: cluster.and_then(|owner| {
            (!owner.enabled_cloudwatch_logs_exports.is_empty())
                .then(|| owner.enabled_cloudwatch_logs_exports.clone())
        }),
        certificate_details: None,
        performance_insights_enabled: Some(record.performance_insights_enabled),
        performance_insights_kms_key_id: record.performance_insights_kms_key_id.clone(),
    }
}

// ---------------------------------------------------------------------------
// Cluster parameter groups
//
// A cluster parameter group is a named, family-scoped set of engine parameters that
// `CreateDBClusterParameterGroup` mints, `ModifyDBClusterParameterGroup` writes into,
// `ResetDBClusterParameterGroup` clears, the two describes report and
// `DeleteDBClusterParameterGroup` removes. The group carries the parameters a CALLER has
// set; the engine's own defaults are published by the family (see
// [`DOCDB_CLUSTER_PARAMETERS`]) and are not materialised into a group, which is why a
// freshly created group reports an EMPTY `Parameters` list.
// ---------------------------------------------------------------------------

// The catalog of engine parameters each family publishes, and the rules over it, are
// SHARED with the Amazon RDS and Amazon Neptune models of the same front door (an AWS SDK
// DocumentDB client signs `rds`, so its traffic is served there), so they live once in
// `service_common::rds_cluster_parameters` and are re-exported here: a handler reads them
// as `crate::state::<name>` exactly as before.
pub use service_common::rds_cluster_parameters::{
    change_is_pending, default_apply_method, engine_version_precedes,
    family_earliest_engine_version, minimum_engine_version_in_family,
    not_a_valid_identifier_message, parameter_has_pending_changes_message,
    parameter_source_filter, published_parameter,
    published_parameters, validate_parameter_group_family, validate_parameter_group_name,
    validate_parameter_name, validate_parameter_setting, ClusterParameterSpec,
    ParameterSourceFilter, APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT,
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, DOCDB_CLUSTER_PARAMETERS,
    DOCDB_PARAMETER_GROUP_FAMILIES, MAX_PARAMETERS_PER_MODIFY, MAX_PARAMETER_GROUP_NAME_LEN,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER,
};

/// One parameter value a caller set on a group.
#[derive(Debug, Clone)]
pub struct ParameterSetting {
    /// `Parameter$ParameterValue`, as the caller set it.
    pub value: String,
    /// `Parameter$ApplyMethod`, as the caller asked for it (see
    /// [`APPLY_METHOD_PENDING_REBOOT`] for what an absent one becomes).
    pub apply_method: String,
}

/// A persisted cluster parameter group.
///
/// The name is stored lowercased and is this record's key:
/// `CreateDBClusterParameterGroupMessage$DBClusterParameterGroupName` states "This value is
/// stored as a lowercase string."
#[derive(Debug, Clone)]
pub struct DbClusterParameterGroupRecord {
    /// The lowercased `DBClusterParameterGroupName`.
    pub name: String,
    /// `DBParameterGroupFamily`, as the caller named it.
    pub family: String,
    /// `Description`.
    pub description: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The parameters a caller has set, by `ParameterName`.
    pub parameters: BTreeMap<String, ParameterSetting>,
    /// The group's tags, as (key, value).
    pub tags: Vec<(String, String)>,
}

impl DbClusterParameterGroupRecord {
    /// `DBClusterParameterGroupArn`. The resource type is `cluster-pg`, the one AWS uses
    /// for a cluster parameter group under the `rds` ARN namespace this service shares.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-pg:{}", self.name),
        )
    }

    /// This record as the modelled `DBClusterParameterGroup`.
    pub fn to_shape(&self) -> DBClusterParameterGroup {
        DBClusterParameterGroup {
            db_cluster_parameter_group_name: Some(self.name.clone()),
            db_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            db_cluster_parameter_group_arn: Some(self.arn()),
        }
    }

    /// Whether the change to `name` is still waiting on a reboot of the cluster's
    /// instances; see `service_common::rds_cluster_parameters::change_is_pending` for the
    /// documented rule.
    pub fn parameter_is_pending(&self, name: &str) -> bool {
        let Some(setting) = self.parameters.get(name) else {
            return false;
        };
        change_is_pending(&self.family, name, &setting.apply_method)
    }

    /// The names of the parameter changes this group is still waiting on a reboot for, in
    /// name order; see [`Self::parameter_is_pending`].
    pub fn pending_parameters(&self) -> Vec<String> {
        self.parameters
            .keys()
            .filter(|name| self.parameter_is_pending(name))
            .cloned()
            .collect()
    }

    /// The parameter `name` as the modelled `Parameter`: the value and apply method the
    /// caller set, plus the metadata the family publishes for it when it publishes any.
    pub fn parameter_to_shape(&self, name: &str) -> Parameter {
        let setting = self.parameters.get(name);
        let spec = published_parameter(&self.family, name);
        Parameter {
            parameter_name: Some(name.to_string()),
            parameter_value: setting.map(|setting| setting.value.clone()),
            description: spec.map(|spec| spec.description.to_string()),
            source: Some(PARAMETER_SOURCE_USER.to_string()),
            apply_type: spec.map(|spec| spec.apply_type.to_string()),
            data_type: spec.map(|spec| spec.data_type.to_string()),
            allowed_values: spec.map(|spec| spec.allowed_values.to_string()),
            is_modifiable: spec.map(|spec| spec.is_modifiable),
            // The earliest version the parameter can apply at inside THIS group's family,
            // which is what the member answers about; see
            // `service_common::rds_cluster_parameters::minimum_engine_version_in_family`.
            minimum_engine_version: spec
                .map(|spec| minimum_engine_version_in_family(&self.family, spec)),
            apply_method: setting.map(|setting| setting.apply_method.clone()),
        }
    }
}

/// A parameter the engine publishes for `family`, as the modelled `Parameter` with its
/// default value and `Source` [`PARAMETER_SOURCE_ENGINE_DEFAULT`]: what a describe reports
/// when the caller asks for the engine's own parameters rather than the group's.
///
/// `family` is carried because `MinimumEngineVersion` is answered about the family the
/// group belongs to and not about the engine as a whole; see
/// [`minimum_engine_version_in_family`].
pub fn engine_default_parameter_to_shape(family: &str, spec: &ClusterParameterSpec) -> Parameter {
    Parameter {
        parameter_name: Some(spec.name.to_string()),
        parameter_value: Some(spec.default_value.to_string()),
        description: Some(spec.description.to_string()),
        source: Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string()),
        apply_type: Some(spec.apply_type.to_string()),
        data_type: Some(spec.data_type.to_string()),
        allowed_values: Some(spec.allowed_values.to_string()),
        is_modifiable: Some(spec.is_modifiable),
        minimum_engine_version: Some(minimum_engine_version_in_family(family, spec)),
        apply_method: None,
    }
}

// ---------------------------------------------------------------------------
// Published reference catalogs
//
// The read-only tables the catalog operations report: the certificate authorities
// `DescribeCertificates` publishes, the engine versions `DescribeDBEngineVersions`
// publishes, the event source types `DescribeEventCategories` publishes and the instance
// classes `DescribeOrderableDBInstanceOptions` offers. They are reference data — the same
// for every caller, and no request changes any of them — so they live here beside the
// records rather than inside one handler, and each report is PROJECTED from one of them:
// the certificate identifiers an engine version offers are read from the one certificate
// catalogue, and an option is offered for an engine version the engine-version report
// publishes, so no two reports can disagree about the same fact.
// ---------------------------------------------------------------------------

/// `Certificate$CertificateType` — "The type of the certificate. Example: CA". Every
/// authority this service publishes is a certificate authority.
pub const CA_CERTIFICATE_TYPE: &str = "CA";

/// The public-key algorithm a published certificate authority signs with.
///
/// It is a property of the authority that a second report reads: a DocumentDB cluster's
/// endpoint serves an RSA server certificate, so the authorities an engine version can be
/// launched under ([`supported_ca_certificate_identifiers`]) are the RSA ones, longest key
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateKey {
    /// An ECDSA authority, over a curve of this many bits.
    EllipticCurve(u32),
    /// An RSA authority, with a modulus of this many bits.
    Rsa(u32),
}

/// One certificate authority this service publishes for a cluster's TLS endpoint.
///
/// Source: the certificate authorities AWS publishes for this front door ("Updating your
/// Amazon DocumentDB TLS certificates", Amazon DocumentDB Developer Guide, and the
/// certificate-authority table of "Using SSL/TLS to encrypt a connection to a DB
/// instance"), each row carrying the `notBefore`/`notAfter` instants and the SHA-1
/// fingerprint of the certificate that row NAMES. A validity endpoint is the
/// certificate's own instant, to the second: the window belongs to the certificate
/// material, so it is never a calendar day with the clock zeroed, and two authorities
/// signed at different moments report different windows.
#[derive(Debug, Clone, Copy)]
pub struct CaCertificate {
    /// `Certificate$CertificateIdentifier` — "The unique key that identifies a
    /// certificate."
    pub identifier: &'static str,
    /// `Certificate$ValidFrom` — "The starting date-time from which the certificate is
    /// valid", as an ISO-8601 instant.
    pub valid_from: &'static str,
    /// `Certificate$ValidTill` — "The date-time after which the certificate is no longer
    /// valid", as an ISO-8601 instant.
    pub valid_till: &'static str,
    /// `Certificate$Thumbprint` — "The thumbprint of the certificate", the fingerprint of
    /// the published authority's certificate. EMPTY for an authority whose fingerprint
    /// this emulator does not hold: a fingerprint is a digest OF certificate bytes, so one
    /// that is not published is reported as no `Thumbprint` member at all rather than as
    /// an invented digest.
    pub thumbprint: &'static str,
    /// The key this authority signs with, which its identifier itself names
    /// (`rds-ca-rsa2048-g1`, `rds-ca-ecc384-g1`).
    pub key: CertificateKey,
}

/// Every certificate authority `DescribeCertificates` reports, in the order this front
/// door publishes them: the strongest authority first — every elliptic-curve authority
/// before every RSA one, and within a kind the longest key first.
///
/// The rows are the authorities the service PROVIDES, each carrying its own certificate's
/// `notBefore`/`notAfter` and SHA-1 fingerprint. An authority whose certificate has
/// expired is no longer provided for a cluster's TLS endpoint and so is not published at
/// all — the retired 2019 authority, whose certificate expired in August 2024, is not a
/// row here; an authority AWS publishes or retires is a row added or removed.
pub const CA_CERTIFICATES: &[CaCertificate] = &[
    CaCertificate {
        identifier: "rds-ca-ecc384-g1",
        valid_from: "2021-05-24T22:06:59Z",
        valid_till: "2121-05-24T23:06:59Z",
        thumbprint: "2ee3dcc06e50192559b13929e73484354f23387d",
        key: CertificateKey::EllipticCurve(384),
    },
    CaCertificate {
        identifier: "rds-ca-rsa4096-g1",
        valid_from: "2021-05-24T22:03:20Z",
        valid_till: "2121-05-24T23:03:20Z",
        thumbprint: "19da4f2af579a8ae1f6a0fa77aa5befd874b4cab",
        key: CertificateKey::Rsa(4096),
    },
    CaCertificate {
        identifier: "rds-ca-rsa2048-g1",
        valid_from: "2021-05-24T21:59:00Z",
        valid_till: "2061-05-24T22:59:00Z",
        thumbprint: "7c40cb42714b6fdb2b296f9bbd0e8bb364436a76",
        key: CertificateKey::Rsa(2048),
    },
];

impl CaCertificate {
    /// This catalogue entry as the modelled `Certificate`, with its ARN scoped to the
    /// region the request was signed for.
    pub fn to_shape(&self, region: &str) -> Certificate {
        Certificate {
            certificate_identifier: Some(self.identifier.to_string()),
            certificate_type: Some(CA_CERTIFICATE_TYPE.to_string()),
            // A fingerprint the catalogue does not carry is reported as an ABSENT member,
            // never as an empty string: a client pinning a thumbprint has to be able to
            // tell "not published" from a digest.
            thumbprint: (!self.thumbprint.is_empty()).then(|| self.thumbprint.to_string()),
            valid_from: Some(catalog_timestamp(self.valid_from)),
            valid_till: Some(catalog_timestamp(self.valid_till)),
            certificate_arn: Some(ca_certificate_arn(region, self.identifier)),
        }
    }

    /// Whether this authority is still valid at `instant`, which is what decides whether a
    /// cluster can still be launched under it.
    pub fn is_valid_at(&self, instant: &::aws_smithy_types::DateTime) -> bool {
        catalog_timestamp(self.valid_till).secs() > instant.secs()
    }

    /// The order an engine version publishes its supported authorities in: every
    /// elliptic-curve authority before every RSA one, and within a kind the longest key
    /// first, which is the order [`CA_CERTIFICATES`] itself is held in.
    pub fn publication_rank(&self) -> (u8, std::cmp::Reverse<u32>) {
        match self.key {
            CertificateKey::EllipticCurve(bits) => (0, std::cmp::Reverse(bits)),
            CertificateKey::Rsa(bits) => (1, std::cmp::Reverse(bits)),
        }
    }
}

/// The published authority `identifier` names, or `None` when none carries it.
///
/// `DescribeCertificatesMessage$CertificateIdentifier`: "This parameter is not case
/// sensitive", so the match ignores case.
pub fn ca_certificate(identifier: &str) -> Option<&'static CaCertificate> {
    CA_CERTIFICATES
        .iter()
        .find(|certificate| certificate.identifier.eq_ignore_ascii_case(identifier))
}

/// The ARN of a published CA certificate, in the form `Certificate$CertificateArn`
/// documents: "Example: `arn:aws:rds:us-east-1::cert:rds-ca-2019`".
///
/// The ACCOUNT field is empty, unlike every other ARN this service builds: a certificate
/// authority is published BY the service rather than created by an account, so there is no
/// owning account to name. It is still region-scoped, because the authority a cluster is
/// launched under is reported per Region.
pub fn ca_certificate_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::cert:{identifier}")
}

/// One of the catalogues' fixed ISO-8601 instants as the modelled timestamp type.
///
/// The input is a literal in a catalogue above, never caller data, so a value that is not
/// an instant is a typo in that catalogue: it panics at the first read rather than
/// silently dropping a timestamp member from a response.
pub fn catalog_timestamp(iso: &str) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_str(iso, ::aws_smithy_types::date_time::Format::DateTime)
        .expect("every catalogue instant in this service is an ISO-8601 literal")
}

/// `DBEngineVersion$SupportedCACertificateIdentifiers` — "A list of the supported CA
/// certificate identifiers."
///
/// Read from the ONE certificate catalogue this service reports through
/// `DescribeCertificates` ([`CA_CERTIFICATES`]), so the two reports cannot name different
/// authorities, narrowed twice: an authority whose `ValidTill` has passed is not one a
/// cluster can be launched under, and a DocumentDB cluster endpoint serves an RSA server
/// certificate, so the elliptic-curve authority this door also publishes is not offered to
/// this engine. The order is the one an engine version publishes its authorities in
/// ([`CaCertificate::publication_rank`]).
pub fn supported_ca_certificate_identifiers() -> Vec<String> {
    let now = now();
    let mut offered: Vec<&CaCertificate> = CA_CERTIFICATES
        .iter()
        .filter(|certificate| {
            certificate.is_valid_at(&now) && matches!(certificate.key, CertificateKey::Rsa(_))
        })
        .collect();
    offered.sort_by_key(|certificate| certificate.publication_rank());
    offered
        .into_iter()
        .map(|certificate| certificate.identifier.to_string())
        .collect()
}

/// `DBEngineVersion$DBEngineDescription` — "The description of the database engine", which
/// is the product's own full name.
pub const DOCDB_ENGINE_DESCRIPTION: &str = "Amazon DocumentDB (with MongoDB compatibility)";

/// How a version description names the product: `DBEngineVersionDescription` states the
/// product and the version it describes (`Amazon DocumentDB 5.0.0`).
pub const DOCDB_PRODUCT: &str = "Amazon DocumentDB";

/// The CloudWatch log types a cluster can export, each with the cluster parameter that
/// enables it, and the log types of one version: both are the shared catalog's
/// ([`::service_common::docdb_engine_catalog::EXPORTABLE_LOG_TYPES`],
/// [`::service_common::docdb_engine_catalog::exportable_log_types`]), so this view and the
/// Amazon RDS view of the same front door cannot report different log exports for one
/// version.
pub use ::service_common::docdb_engine_catalog::EXPORTABLE_LOG_TYPES;

/// `DBEngineVersion$ExportableLogTypes` for `version`; see [`EXPORTABLE_LOG_TYPES`].
pub fn exportable_log_types(version: &str) -> Vec<String> {
    ::service_common::docdb_engine_catalog::exportable_log_types(version)
        .into_iter()
        .map(|log_type| log_type.to_string())
        .collect()
}

/// The versions `version` can be upgraded to, as `DBEngineVersion$ValidUpgradeTarget`
/// reports them ("A list of engine versions that this database engine version can be
/// upgraded to").
///
/// Which versions those are is the shared front door's rule, read from the ONE catalog both
/// of its views answer this report from
/// ([`::service_common::docdb_engine_catalog::upgrade_targets`]): a later minor release of
/// the line the version runs, and a release of a later major line this engine offers an
/// in-place major version upgrade to.
///
/// Every member of a target is the TARGET's own, because a caller decides whether to take an
/// upgrade by what it would be running afterwards. `AutoUpgrade` is whether the target is
/// applied to a resource with `AutoMinorVersionUpgrade` set, so it is reported for a MINOR
/// move within one major line and never for a major one; `SupportsSnapshotUpgrade` is
/// whether a snapshot can be upgraded straight to the target, which is what "Upgrading a DB
/// snapshot engine version" documents for a MAJOR version upgrade.
pub fn valid_upgrade_targets(version: &str) -> Vec<UpgradeTarget> {
    let from_major = ::service_common::docdb_engine_catalog::engine_version(version)
        .map(|row| row.major)
        .unwrap_or_default();
    ::service_common::docdb_engine_catalog::upgrade_targets(version)
        .into_iter()
        .map(|target| {
            let major_upgrade = target.major != from_major;
            UpgradeTarget {
                engine: Some(DOCDB_ENGINE.to_string()),
                engine_version: Some(target.version.to_string()),
                major_engine_version: Some(target.major.to_string()),
                minor_engine_version: Some(target.minor.to_string()),
                description: Some(engine_version_description(target.version)),
                // `AutoUpgrade` is whether the target is applied to a resource with
                // `AutoMinorVersionUpgrade` set, which moves a cluster onto the DEFAULT
                // release of the line it already runs — so it is reported for a target
                // inside the version's own line that is that line's default, and for no
                // major-version target, which is never taken automatically.
                auto_upgrade: Some(
                    !major_upgrade
                        && ::service_common::docdb_engine_catalog::is_line_default(
                            target.version,
                        ),
                ),
                is_major_version_upgrade: Some(major_upgrade),
                supports_snapshot_upgrade: Some(major_upgrade),
                supports_data_groups: Some(
                    ::service_common::docdb_engine_catalog::VERSION_CAPABILITIES.data_groups,
                ),
            }
        })
        .collect()
}

/// `DBEngineVersion$DBEngineVersionDescription` for `version`.
///
/// The DocumentDB control plane publishes no per-version description of its own: every
/// version of this engine, and every upgrade target onto one, is described by the product's
/// own full name, which is the description the shared front door reports for this engine
/// ([`::service_common::docdb_engine_catalog::ENGINE_DESCRIPTION`]).
pub fn engine_version_description(version: &str) -> String {
    let _ = version;
    ::service_common::docdb_engine_catalog::ENGINE_DESCRIPTION.to_string()
}

/// One published engine version as the modelled `DBEngineVersion`.
///
/// Every member is DERIVED: the version split and the parameter-group family from the shared
/// engine catalog of the front door this service is a view of
/// ([`::service_common::docdb_engine_catalog`]), the exportable log types from the parameter
/// catalogue ([`exportable_log_types`]), the supported authorities from the certificate
/// catalogue ([`supported_ca_certificate_identifiers`]) and the upgrade targets from the
/// published version list ([`valid_upgrade_targets`]).
///
/// The capability members are the ones that front door reports on EVERY engine version,
/// whichever of its views answers — this engine publishes none of them but the certificate
/// rotation ([`::service_common::docdb_engine_catalog::VERSION_CAPABILITIES`]) — so a client
/// that gates on one reads the same answer from this view as from the Amazon RDS view of the
/// same door, rather than reading it as absent.
///
/// `ServerlessV2FeaturesSupport` reports "any Amazon DocumentDB Serverless properties or
/// limits that differ between Amazon DocumentDB engine versions"; this emulator publishes
/// no serverless capacity limits, so the member is left absent rather than invented.
pub fn engine_version_to_shape(version: &str) -> DBEngineVersion {
    let exportable = exportable_log_types(version);
    let capabilities = ::service_common::docdb_engine_catalog::VERSION_CAPABILITIES;
    let published = ::service_common::docdb_engine_catalog::engine_version(version);
    DBEngineVersion {
        engine: Some(DOCDB_ENGINE.to_string()),
        engine_version: Some(version.to_string()),
        major_engine_version: published.map(|row| row.major.to_string()),
        minor_engine_version: published.map(|row| row.minor.to_string()),
        db_parameter_group_family: Some(parameter_group_family(version)),
        db_engine_description: Some(DOCDB_ENGINE_DESCRIPTION.to_string()),
        db_engine_version_description: Some(engine_version_description(version)),
        valid_upgrade_target: Some(valid_upgrade_targets(version)),
        supports_log_exports_to_cloudwatch_logs: Some(!exportable.is_empty()),
        exportable_log_types: Some(exportable),
        supported_ca_certificate_identifiers: Some(supported_ca_certificate_identifiers()),
        supports_certificate_rotation_without_restart: Some(
            capabilities.certificate_rotation_without_restart,
        ),
        supports_read_replica: Some(capabilities.read_replica),
        supports_cluster: Some(capabilities.cluster),
        supports_global_databases: Some(capabilities.global_databases),
        supports_rds_proxy: Some(capabilities.rds_proxy),
        supports_parallel_query: Some(capabilities.parallel_query),
        supports_babelfish: Some(capabilities.babelfish),
        supports_limitless_database: Some(capabilities.limitless_database),
        supports_data_groups: Some(capabilities.data_groups),
        serverless_v2_features_support: None,
    }
}

/// The event source types this service publishes a category map for, each with the event
/// categories of that source type.
///
/// It is the SHARED catalog of the one `rds` front door this service is a view of
/// ([`::service_common::rds_event_categories::EVENT_CATEGORIES`]), re-exported here rather
/// than restated: Amazon DocumentDB, Amazon RDS and Amazon Neptune are three views of that
/// one door, `DescribeEventCategories` answers all three from the one published catalog,
/// and a view that held its own copy published rows — and an order — the front door does
/// not. The rows are published in the catalog's own order and each row's categories in
/// theirs, neither of them sorted.
///
/// It is NARROWER than the set of source types that front door ACCEPTS, which
/// [`invalid_event_source_type_message`] names: a map is published only for a source type
/// whose categories this emulator can cite.
pub use ::service_common::rds_event_categories::{event_categories_of, EVENT_CATEGORIES};

/// One source type's categories as the modelled `EventCategoriesMap`.
pub fn event_categories_to_shape(source_type: &str, categories: &[&str]) -> EventCategoriesMap {
    EventCategoriesMap {
        source_type: Some(source_type.to_string()),
        event_categories: Some(
            categories
                .iter()
                .map(|category| (*category).to_string())
                .collect(),
        ),
    }
}

/// The message the refusal of a `SourceType` that names no published event source type
/// carries.
///
/// The set of source types the front door this service shares accepts is wider than the
/// ones this report publishes a category map for, and the refusal names the whole of it, so
/// it is the shared `service_common::rds_event_source_types` list — one set for every
/// service view of that one front door — that this sentence comes from.
pub fn invalid_event_source_type_message() -> String {
    ::service_common::rds_event_source_types::invalid_event_source_type_message()
}

/// The message a report refuses a `Filters` entry whose `Name` it does not support with.
///
/// ONE sentence: the fixed text, then the name the request carried, and nothing after it —
/// no enumeration of the names the report does support, and no mention of the operation.
/// The only variable part is `name`, read straight off the request's own `Filter$Name`, so a
/// caller validating its filter names against the refusal reads back the value it sent, and
/// nothing in the sentence advertises which filters this emulator happens to implement.
///
/// One wording for every report of this service view that checks its filter names — the ones
/// that publish several names and the ones that publish a single name alike, since what is
/// refused is the same thing in both cases — so none of them can drift from the others.
pub fn unrecognized_filter_name_message(name: &str) -> String {
    format!("Unrecognized filter name: {name}")
}

/// The instance families a cluster member can be ordered in, and the sizes each is
/// published in.
///
/// It is the SHARED catalog of this engine's own
/// ([`::service_common::docdb_engine_catalog::INSTANCE_FAMILIES`]), re-exported here rather
/// than restated: the classes a cluster member of this engine can be ordered in are the
/// ENGINE's, and both views of the one `rds` front door that report them — this one and the
/// Amazon RDS view — read them there, so no client can read a different orderable class list
/// depending on how it addressed the door.
pub use ::service_common::docdb_engine_catalog::INSTANCE_FAMILIES as DOCDB_INSTANCE_FAMILIES;

/// Every `DBInstanceClass` this service offers, in the order
/// [`DOCDB_INSTANCE_FAMILIES`] publishes them.
pub fn instance_classes() -> Vec<String> {
    ::service_common::docdb_engine_catalog::instance_classes()
}

/// The storage types a cluster member of this engine can be ordered with, in the order the
/// value list publishes them — the shared catalog's own
/// ([`::service_common::docdb_engine_catalog::STORAGE_TYPES`]), so this view and the Amazon
/// RDS view of the same front door offer the same types for this engine. The first of them
/// is the type a create without a `StorageType` gets ([`DEFAULT_STORAGE_TYPE`]).
pub use ::service_common::docdb_engine_catalog::STORAGE_TYPES;

/// `OrderableDBInstanceOption$LicenseModel` and `DBInstance$LicenseModel` for this engine.
///
/// The licence setting does not apply to Amazon DocumentDB — the model declares no
/// `LicenseModel` on `CreateDBInstance` at all — and an engine AWS licenses itself carries
/// `na`. Read from the shared engine catalogue
/// ([`::service_common::docdb_engine_catalog::LICENSE_MODEL`]), so this view and the Amazon
/// RDS view of the same front door report the one value for this engine.
pub const DOCDB_LICENSE_MODEL: &str = ::service_common::docdb_engine_catalog::LICENSE_MODEL;

/// One (engine version, instance class, storage type) row as the modelled
/// `OrderableDBInstanceOption`.
///
/// The zones are the ones the REGION exposes
/// ([`::service_common::aws_availability_zones`]) — an option is offered in every zone of
/// the Region, which is a longer list than the three a single cluster spreads itself over
/// ([`default_availability_zones`]) — so a caller that places an instance in a zone this
/// report names is not refused a zone the Region in fact offers. `Vpc` is true because every
/// endpoint this emulator serves is reached inside a VPC.
///
/// `StorageType` is the ROW's own: a DocumentDB cluster — not its instance — owns the
/// storage, so the option reports no size and no provisioned IOPS, but the type is one of the
/// values a create picks between
/// ([`::service_common::docdb_engine_catalog::STORAGE_TYPES`]) and each of them is its own
/// option.
///
/// The capability members are the ones the shared front door reports on EVERY orderable
/// option, answered for this engine from the one catalog both of its views read
/// ([`::service_common::docdb_engine_catalog::OPTION_CAPABILITIES`]): this is the call a
/// client makes before a create to decide what it may order, so a capability its cluster
/// member does not have is reported false rather than left absent, and one it does have is
/// reported rather than hidden.
pub fn orderable_option_to_shape(
    version: &str,
    instance_class: &str,
    storage_type: &str,
    region: &str,
) -> OrderableDBInstanceOption {
    let capabilities = ::service_common::docdb_engine_catalog::OPTION_CAPABILITIES;
    OrderableDBInstanceOption {
        engine: Some(DOCDB_ENGINE.to_string()),
        engine_version: Some(version.to_string()),
        db_instance_class: Some(instance_class.to_string()),
        license_model: Some(DOCDB_LICENSE_MODEL.to_string()),
        availability_zones: Some(
            ::service_common::aws_availability_zones::availability_zones(region)
                .into_iter()
                .map(|zone| AvailabilityZone { name: Some(zone) })
                .collect(),
        ),
        vpc: Some(true),
        storage_type: Some(storage_type.to_string()),
        multi_az_capable: Some(capabilities.multi_az_capable),
        read_replica_capable: Some(capabilities.read_replica_capable),
        supports_storage_encryption: Some(capabilities.storage_encryption),
        supports_iops: Some(capabilities.iops),
        supports_storage_throughput: Some(capabilities.storage_throughput),
        supports_enhanced_monitoring: Some(capabilities.enhanced_monitoring),
        supports_iam_database_authentication: Some(capabilities.iam_database_authentication),
        supports_performance_insights: Some(capabilities.performance_insights),
        outpost_capable: Some(capabilities.outpost_capable),
        supports_clusters: Some(capabilities.clusters),
        supports_global_databases: Some(capabilities.global_databases),
        supports_http_endpoint: Some(capabilities.http_endpoint),
        supports_optimized_reads: Some(capabilities.optimized_reads),
        supports_optimized_writes: Some(capabilities.optimized_writes),
        supports_kerberos_authentication: Some(capabilities.kerberos_authentication),
        supports_dedicated_log_volume: Some(capabilities.dedicated_log_volume),
        supports_additional_storage_volumes: Some(capabilities.additional_storage_volumes),
        requires_custom_processor_features: Some(
            capabilities.requires_custom_processor_features,
        ),
        supported_engine_modes: Some(strings(
            ::service_common::docdb_engine_catalog::ENGINE_MODES,
        )),
        supported_activity_stream_modes: Some(strings(
            ::service_common::docdb_engine_catalog::ACTIVITY_STREAM_MODES,
        )),
        supported_network_types: Some(strings(
            ::service_common::docdb_engine_catalog::NETWORK_TYPES,
        )),
    }
}

/// A published list of literals as the owned strings a wire shape's list member takes.
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}
