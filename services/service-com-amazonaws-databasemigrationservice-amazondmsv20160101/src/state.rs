// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-databasemigrationservice-amazondmsv20160101 service.
//!
//! This module owns the replication-instance, endpoint, replication-task and
//! replication-subnet-group records the implemented operations read and write, the
//! resource-id minting the DMS ARNs are built from, the identifier rules the member docs
//! publish, the single place where a clock-owed status transition is materialised
//! ([`settle`]), and the readings of the facts DMS resolves against ANOTHER service
//! rather than holding itself: the account's subnets and Availability Zones, which are
//! Amazon EC2's ([`placed_subnets`], [`region_availability_zones`]).
//!
//! [`settle`]: ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State::settle

/// The `Filters` guard every DMS `Describe*` that takes a `FilterList` shares: which
/// filter names a report accepts, and the one refusal an unrecognized name is answered
/// with.
pub mod filters;

/// Folding the engine settings a `ModifyEndpoint` request supplied onto the ones an
/// endpoint already holds, which is what `ExactSettings` selects between.
pub mod endpoint_settings;

/// The RUN of a replication task: the full load it performs between the endpoints' own
/// data planes, the per-task and per-table accounting that load records, and the stop a
/// task with no phase after it arrives at.
pub mod task_run;

/// The task-settings document a replication task runs with: the service defaults, and a
/// caller's own settings resolved against them.
pub mod task_settings;

pub use filters::{check_filter_names, unrecognized_filter_name_message};
pub use task_run::{advance_task_runs, TaskRun, FULL_LOAD_ONLY_FINISHED, STOPPED_NORMALLY};
pub use task_settings::{
    default_task_settings, resolve_task_settings, validation_enabled, LOG_COMPONENTS,
    LOG_SEVERITY_DEFAULT,
};

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AvailabilityZone, DmsTransferSettings, DocDbSettings, DynamoDbSettings, ElasticsearchSettings,
    Endpoint, GcpMySQLSettings, IBMDb2Settings, KafkaSettings, KerberosAuthenticationSettings,
    KinesisSettings, MicrosoftSQLServerSettings, MongoDbSettings, MySQLSettings, NeptuneSettings,
    OracleSettings, PostgreSQLSettings, RedisSettings, RedshiftSettings, ReplicationInstance,
    ReplicationPendingModifiedValues, ReplicationSubnetGroup, ReplicationTask, S3Settings, Subnet,
    SybaseSettings, Tag, TimestreamSettings, VpcSecurityGroupMembership,
};

/// A replication instance `CreateReplicationInstance` has accepted but not finished
/// provisioning. One of the values the `ReplicationInstance$ReplicationInstanceStatus`
/// doc publishes.
pub const INSTANCE_CREATING: &str = "creating";
/// A replication instance that finished provisioning.
pub const INSTANCE_AVAILABLE: &str = "available";
/// The status a replication instance `DeleteReplicationInstance` has accepted carries
/// while it is on its way out of the account: the record is owed removal
/// ([`Owed::Removal`]), so a caller that reads straight after the delete still sees the
/// instance leaving. One of the values the `ReplicationInstance$ReplicationInstanceStatus`
/// doc publishes.
pub const INSTANCE_DELETING: &str = "deleting";

/// The status a replication instance carries while a setting whose change needs an
/// outage is being applied to it, which is what `ModifyReplicationInstance` accepts with
/// `ApplyImmediately`. The record is owed [`INSTANCE_AVAILABLE`] again
/// ([`Owed::ToStatus`]), so a caller that reads straight after the modify still sees the
/// instance working on it. One of the values the
/// `ReplicationInstance$ReplicationInstanceStatus` doc publishes.
pub const INSTANCE_MODIFYING: &str = "modifying";

/// The status an endpoint carries once it is usable. DMS creates an endpoint ready —
/// it provisions nothing — so this is the status every create reports.
pub const ENDPOINT_ACTIVE: &str = "active";

/// The status an endpoint `DeleteEndpoint` has accepted carries while it is on its way
/// out of the account: the record is owed removal ([`Owed::Removal`]), so a caller that
/// reads straight after the delete still sees the endpoint leaving.
pub const ENDPOINT_DELETING: &str = "deleting";

/// A task `CreateReplicationTask` has accepted but not finished creating: "the task
/// is being created in response to running the CreateReplicationTask operation"
/// (`ReplicationTask$Status` doc).
pub const TASK_CREATING: &str = "creating";
/// A created task that has never run, and can be started.
pub const TASK_READY: &str = "ready";
/// A task `StartReplicationTask` has accepted.
pub const TASK_STARTING: &str = "starting";
/// A task that is migrating data.
pub const TASK_RUNNING: &str = "running";
/// A task `StopReplicationTask` has accepted: "the task is preparing to stop in
/// response to running the StopReplicationTask operation" (`ReplicationTask$Status`
/// doc). The record is owed [`TASK_STOPPED`] ([`Owed::ToStatus`]), so a caller that
/// reads straight after the stop still sees the task coming off its run.
pub const TASK_STOPPING: &str = "stopping";
/// A task that was stopped, and so can be started again.
pub const TASK_STOPPED: &str = "stopped";
/// A task `DeleteReplicationTask` has accepted: "the task is being deleted in
/// response to running the DeleteReplicationTask operation" (`ReplicationTask$Status`
/// doc).
pub const TASK_DELETING: &str = "deleting";

/// How long after the call that starts it a status transition this emulator owes
/// becomes due (`creating` -> `available` for an instance, `creating` -> `ready` and
/// `starting` -> `running` for a task, `deleting` -> gone). A chosen span, not an
/// AWS-documented duration: it only has to be non-zero, so a caller that acts
/// straight after a create, start or delete still observes the transient status and
/// the state errors that go with it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(400);

/// The region used to build ARNs and name Availability Zones when a request carries
/// no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The storage a replication instance gets when the request names no
/// `AllocatedStorage`. The emulator's own default: the model publishes no default for
/// the member.
pub const DEFAULT_ALLOCATED_STORAGE: i32 = 50;

/// The engine version a replication instance gets when the request names none.
/// `CreateReplicationInstance`'s `EngineVersion` doc says "If an engine version
/// number is not specified when a replication instance is created, the default is the
/// latest engine version available", and the model publishes no version catalog, so
/// this is the version THIS emulator presents as its latest.
pub const DEFAULT_ENGINE_VERSION: &str = "3.5.4";

/// The SSL mode an endpoint gets when the request names none: the `Endpoint$SslMode`
/// doc says "The default value is none."
pub const DEFAULT_SSL_MODE: &str = "none";

/// How an engine settings structure reports authentication when the request named no
/// method.
///
/// DMS reports `password` there — authentication by the endpoint's own user name and
/// password — for an endpoint created without one, and `password` is the value each
/// engine's authentication-method enum models for exactly that
/// (`MySQLAuthenticationMethod`, `PostgreSQLAuthenticationMethod`,
/// `OracleAuthenticationMethod` and `SqlServerAuthenticationMethod`); the other values
/// each name an alternative the caller has to ask for (`iam`, `kerberos`).
pub const DEFAULT_AUTHENTICATION_METHOD: &str = "password";

/// Whether an endpoint a caller created is read-only. `Endpoint$IsReadOnly`: "When set
/// to true, this endpoint is managed by DMS as part of a zero-ETL integration and
/// cannot be modified or deleted directly" — which a caller-created endpoint is not.
pub const DEFAULT_IS_READ_ONLY: bool = false;

/// The expanded engine name each `EngineName` value carries, which is what
/// `Endpoint$EngineDisplayName` reports: "The expanded name for the engine name. For
/// example, if the EngineName parameter is \"aurora\", this value would be \"Amazon
/// Aurora MySQL\"". The keys are the values `CreateEndpoint$EngineName` publishes.
const ENGINE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("aurora", "Amazon Aurora MySQL"),
    ("aurora-serverless", "Amazon Aurora MySQL Serverless"),
    ("aurora-postgresql", "Amazon Aurora PostgreSQL"),
    ("aurora-postgresql-serverless", "Amazon Aurora PostgreSQL Serverless"),
    ("azuredb", "Microsoft Azure SQL Database"),
    ("azure-sql-managed-instance", "Microsoft Azure SQL Managed Instance"),
    ("babelfish", "Babelfish"),
    ("db2", "IBM Db2 LUW"),
    ("db2-zos", "IBM Db2 for z/OS"),
    ("dms-transfer", "DMS Transfer"),
    ("docdb", "Amazon DocumentDB"),
    ("documentdb", "Amazon DocumentDB"),
    ("dynamodb", "Amazon DynamoDB"),
    ("elasticsearch", "Amazon OpenSearch Service"),
    ("gcp-mysql", "Google Cloud SQL for MySQL"),
    ("kafka", "Apache Kafka"),
    ("kinesis", "Amazon Kinesis Data Streams"),
    ("mariadb", "MariaDB"),
    ("mongodb", "MongoDB"),
    ("mysql", "MySQL"),
    ("neptune", "Amazon Neptune"),
    ("opensearch", "Amazon OpenSearch Service"),
    ("oracle", "Oracle"),
    ("postgres", "PostgreSQL"),
    ("redis", "Redis"),
    ("redshift", "Amazon Redshift"),
    ("redshift-serverless", "Amazon Redshift Serverless"),
    ("s3", "Amazon S3"),
    ("sqlserver", "Microsoft SQL Server"),
    ("sybase", "SAP ASE"),
    ("timestream", "Amazon Timestream"),
];

/// The expanded name for `engine_name`.
///
/// An engine name outside the published catalog is reported as given rather than
/// guessed at: the member still carries the engine the endpoint runs. Refusing an
/// engine name the catalog does not list belongs to the create path's own validation
/// rule; once that lands, this fallback is unreachable for a stored endpoint.
pub fn engine_display_name(engine_name: &str) -> String {
    ENGINE_DISPLAY_NAMES
        .iter()
        .find(|(name, _)| *name == engine_name)
        .map(|(_, display)| (*display).to_string())
        .unwrap_or_else(|| engine_name.to_string())
}

/// The key an endpoint's connection parameters are encrypted with when the request
/// named none: "If you don't specify a value for the KmsKeyId parameter, then DMS uses
/// your default encryption key. KMS creates the default encryption key for your Amazon
/// Web Services account. Your Amazon Web Services account has a different default
/// encryption key for each Amazon Web Services Region" (`Endpoint$KmsKeyId`).
///
/// Derived from the region alone, so it is one key per region, stable for the life of
/// every endpoint that reports it.
pub fn default_kms_key_arn(region: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"dms-default-encryption-key");
    hasher.update(region.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(&digest[..16]);
    let key_id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    emulator_server::emulator_arn("kms", region, &format!("key/{key_id}"))
}

/// Whether a replication instance the request says nothing about is publicly
/// accessible: the `PubliclyAccessible` doc says "The default value is true."
pub const DEFAULT_PUBLICLY_ACCESSIBLE: bool = true;

/// Whether minor engine upgrades are applied automatically when the request says
/// nothing: the `AutoMinorVersionUpgrade` doc says "This parameter defaults to true."
pub const DEFAULT_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// The maintenance window a replication instance gets when the request names none.
/// The doc says "A 30-minute window selected at random from an 8-hour block of time
/// per Amazon Web Services Region"; this emulator picks one deterministically instead,
/// in the documented `ddd:hh24:mi-ddd:hh24:mi` format.
pub const DEFAULT_MAINTENANCE_WINDOW: &str = "sun:03:00-sun:03:30";

/// The status a VPC security group carries once it is attached to an instance.
pub const VPC_SECURITY_GROUP_ACTIVE: &str = "active";

/// The status a replication subnet group carries once every subnet in it is usable,
/// which is what `ReplicationSubnetGroup$SubnetGroupStatus` reports. The model
/// publishes no value list for it, so this is the value THIS emulator reports for a
/// group that is ready.
pub const SUBNET_GROUP_COMPLETE: &str = "Complete";

/// The network type a resource placed in a subnet group of IPv4 subnets gets:
/// "A value of IPV4 represents an instance profile with IPv4 network type and only
/// supports IPv4 addressing" (`CreateInstanceProfile$NetworkType`, the one member of this
/// model that publishes the value list; `ReplicationInstance$NetworkType` and
/// `ReplicationSubnetGroup$SupportedNetworkTypes` are plain strings over the same set).
pub const NETWORK_TYPE_IPV4: &str = "IPV4";

/// The network type a resource placed in a subnet group whose subnets carry IPv4 AND IPv6
/// gets: "A value of DUAL represents an instance profile with dual network type that
/// supports IPv4 and IPv6 addressing" (same member).
pub const NETWORK_TYPE_DUAL: &str = "DUAL";

/// The status one subnet of a replication subnet group carries
/// (`Subnet$SubnetStatus`). As with the group status, the model publishes no value
/// list, so this is the value this emulator reports for a usable subnet.
pub const SUBNET_ACTIVE: &str = "Active";

/// How many Availability Zones the subnets of a replication subnet group have to cover.
///
/// `CreateReplicationSubnetGroup`'s own doc: "The VPC needs to have at least one subnet
/// in at least two availability zones in the Amazon Web Services Region, otherwise the
/// service will throw a ReplicationSubnetGroupDoesNotCoverEnoughAZs exception."
/// `SubnetIds` is a plain list in the model, so this is a rule the handler enforces
/// rather than a modelled constraint.
pub const MIN_SUBNET_GROUP_AVAILABILITY_ZONES: usize = 2;

/// The principal DMS assumes a caller-supplied service access role as, which is what a
/// role's trust policy has to admit for DMS to be able to use it
/// (`CreateEndpoint$ServiceAccessRoleArn`: "the service access role that you want to use
/// to create the endpoint. The role must allow the iam:PassRole action").
pub const DMS_SERVICE_PRINCIPAL: &str = "dms.amazonaws.com";

/// The replication instance classes DMS offers, as `(family, sizes)`.
///
/// `ReplicationInstanceClass` is a plain string in the model (length 0..30, no
/// `@enum`), and the catalog lives in the page the member doc links instead —
/// "Selecting the right DMS replication instance for your migration" in the DMS user
/// guide, whose instance-class table this mirrors. `CreateReplicationInstance`'s own
/// `ReplicationInstanceClass` doc names `dms.c4.large` as an example, which is the
/// `c4` row below.
///
/// A class names a family and a size: `dms.<family>.<size>`.
const REPLICATION_INSTANCE_CLASS_FAMILIES: &[(&str, &[&str])] = &[
    ("t2", &["micro", "small", "medium", "large"]),
    ("t3", &["micro", "small", "medium", "large"]),
    ("c4", &["large", "xlarge", "2xlarge", "4xlarge"]),
    (
        "c5",
        &[
            "large", "xlarge", "2xlarge", "4xlarge", "9xlarge", "12xlarge", "18xlarge", "24xlarge",
        ],
    ),
    (
        "c6i",
        &[
            "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
            "32xlarge",
        ],
    ),
    ("r4", &["large", "xlarge", "2xlarge", "4xlarge", "8xlarge"]),
    (
        "r5",
        &[
            "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
        ],
    ),
    (
        "r6i",
        &[
            "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge", "24xlarge",
            "32xlarge",
        ],
    ),
];

/// Whether `class` names a replication instance class DMS offers.
///
/// The value is `dms.<family>.<size>` with both parts drawn from
/// [`REPLICATION_INSTANCE_CLASS_FAMILIES`]; anything else — another prefix, a family
/// DMS does not run, or a size that family is not offered in — names no class.
pub fn is_known_replication_instance_class(class: &str) -> bool {
    let Some(rest) = class.strip_prefix("dms.") else {
        return false;
    };
    let Some((family, size)) = rest.split_once('.') else {
        return false;
    };
    REPLICATION_INSTANCE_CLASS_FAMILIES
        .iter()
        .any(|(known_family, sizes)| *known_family == family && sizes.contains(&size))
}

/// Smallest `MaxRecords` a paginated DMS describe accepts, per the member doc:
/// "Constraints: Minimum 20, maximum 100."
pub const MAX_RECORDS_MIN: i32 = 20;
/// Largest `MaxRecords` a paginated DMS describe accepts ("maximum 100").
pub const MAX_RECORDS_MAX: i32 = 100;
/// The page size a describe uses when the request names no `MaxRecords`:
/// "Default: 100".
pub const DEFAULT_MAX_RECORDS: i32 = 100;

/// The most characters a `ResourceIdentifier` may carry: "The value for this
/// parameter can have up to 31 characters."
pub const RESOURCE_IDENTIFIER_MAX_LEN: usize = 31;

/// The most characters a `ReplicationInstanceIdentifier` may carry: "Must contain
/// 1-63 alphanumeric characters or hyphens."
pub const INSTANCE_IDENTIFIER_MAX_LEN: usize = 63;

/// The most characters a `ReplicationTaskIdentifier` may carry: "Must contain 1-255
/// alphanumeric characters or hyphens."
pub const TASK_IDENTIFIER_MAX_LEN: usize = 255;

/// The most characters a `ReplicationSubnetGroupIdentifier` may carry: "Must contain no
/// more than 255 alphanumeric characters, periods, underscores, or hyphens."
pub const SUBNET_GROUP_IDENTIFIER_MAX_LEN: usize = 255;

/// The one `ReplicationSubnetGroupIdentifier` the member doc reserves: "Must not be
/// \"default\"."
pub const RESERVED_SUBNET_GROUP_IDENTIFIER: &str = "default";

/// How many characters a minted resource id (the last ARN field) carries. Inside the
/// 31 characters a caller-supplied `ResourceIdentifier` may have, so a minted id and
/// a supplied one are interchangeable.
const MINTED_RESOURCE_ID_LEN: usize = 26;

/// The alphabet a minted resource id is drawn from: uppercase ASCII letters and
/// digits, which is what the `ResourceIdentifier` doc allows ("only ASCII letters,
/// digits, and hyphen").
const RESOURCE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The most replication instances this emulator's store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// `CreateReplicationInstance` is the only thing that adds one, so the store needs a
/// maximum entry count: this is the emulator's OWN ceiling, not a modelled or
/// published DMS quota.
pub const MAX_REPLICATION_INSTANCES: usize = 100;
/// The most endpoints this emulator's store holds — again the emulator's own ceiling.
pub const MAX_ENDPOINTS: usize = 1000;
/// The most replication tasks this emulator's store holds — the emulator's own
/// ceiling.
pub const MAX_REPLICATION_TASKS: usize = 1000;
/// The most replication subnet groups this emulator's store holds — the emulator's own
/// ceiling, for the same reason.
pub const MAX_REPLICATION_SUBNET_GROUPS: usize = 100;

/// The most subnets ONE `CreateReplicationSubnetGroup` may name.
///
/// CHOSEN as a process-safety backstop, not a published DMS quota (`SubnetIdentifierList`
/// declares no `length`): the list comes from the request and every id in it is resolved
/// against EC2, so without a ceiling one create would size that work. It is applied
/// before any id is looked up, and sits well above the handful of Availability Zones a
/// Region has.
pub const MAX_SUBNETS_PER_GROUP: usize = 20;

/// What the clock owes a record once [`Pending::due`] passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owed {
    /// Move the record to this status.
    ToStatus(&'static str),
    /// Drop the record (a deleted task leaves the store).
    Removal,
}

/// A transition this emulator owes a record, and the instant it comes due. The
/// instant is stored, never slept on.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    /// When the transition is due.
    pub due: SystemTime,
    /// What is owed.
    pub owed: Owed,
}

impl Pending {
    /// A transition due [`TRANSITION_DELAY`] after `now`.
    pub fn after(now: SystemTime, owed: Owed) -> Self {
        Self {
            due: now + TRANSITION_DELAY,
            owed,
        }
    }
}

/// One replication subnet group: the subnets a replication instance can be placed in.
///
/// `CreateReplicationSubnetGroup` is the operation that persists one and
/// `DeleteReplicationSubnetGroup` the one that drops it; the record and the store live
/// here so every operation that RESOLVES a `ReplicationSubnetGroupIdentifier` reads the
/// one place groups are held.
#[derive(Debug, Clone, Default)]
pub struct ReplicationSubnetGroupRecord {
    /// The group's ARN, `arn:aws:dms:<region>:<account>:subgrp:<identifier>` — the
    /// handle a tag attached to the group reports (`Tag$ResourceArn`) and the key a
    /// paged scan of the groups resumes against, derived from the Region the create was
    /// signed for and the identifier the group is held under, and stored so it is the
    /// same value on every later reading. The modelled `ReplicationSubnetGroup` shape
    /// declares no ARN member, so no reading of a group reports it as a field of its
    /// own.
    pub arn: String,
    /// The caller's identifier, stored lowercased: "This value is stored as a
    /// lowercase string" (`CreateReplicationSubnetGroup$ReplicationSubnetGroupIdentifier`).
    pub identifier: String,
    /// The description the caller gave the group.
    pub description: String,
    /// The VPC the group's subnets belong to, as EC2 reports it for them.
    pub vpc_id: String,
    /// The group's subnets, each with the Availability Zone it sits in.
    pub subnets: Vec<SubnetRecord>,
    /// The group's tags, as the create supplied them.
    pub tags: Vec<Tag>,
}

/// One subnet of a [`ReplicationSubnetGroupRecord`].
#[derive(Debug, Clone, Default)]
pub struct SubnetRecord {
    /// The subnet's own identifier.
    pub identifier: String,
    /// The Availability Zone the subnet sits in.
    pub availability_zone: String,
    /// Whether EC2 reports an IPv4 CIDR block for the subnet.
    pub ipv4: bool,
    /// Whether EC2 reports an IPv6 CIDR block the subnet still holds.
    pub ipv6: bool,
}

impl ReplicationSubnetGroupRecord {
    /// The IP addressing protocols this group's subnets support, as
    /// `ReplicationSubnetGroup$SupportedNetworkTypes` reports them.
    ///
    /// The member doc: "The IP addressing protocol supported by the subnet group. This
    /// is used by a replication instance with values such as IPv4 only or Dual-stack
    /// that supports both IPv4 and IPv6 addressing. IPv6 only is not yet supported."
    /// The values are the ones the `NetworkType` doc publishes for a resource placed in
    /// a group ([`NETWORK_TYPE_IPV4`], [`NETWORK_TYPE_DUAL`]), and they follow from the
    /// address families EC2 reports for the group's OWN subnets: a replication instance
    /// gets an address in every one of them, so a protocol is supported only when every
    /// subnet carries it. `IPV6` is never reported, because IPv6 only "is not yet
    /// supported" — a group of subnets carrying no IPv4 range therefore supports no
    /// protocol DMS offers.
    pub fn supported_network_types(&self) -> Vec<String> {
        let mut supported = Vec::new();
        if self.subnets.is_empty() || !self.subnets.iter().all(|subnet| subnet.ipv4) {
            return supported;
        }
        supported.push(NETWORK_TYPE_IPV4.to_string());
        if self.subnets.iter().all(|subnet| subnet.ipv6) {
            supported.push(NETWORK_TYPE_DUAL.to_string());
        }
        supported
    }

    /// This record as the modelled `ReplicationSubnetGroup` shape: every member that
    /// shape declares, and nothing else.
    pub fn to_shape(&self) -> ReplicationSubnetGroup {
        ReplicationSubnetGroup {
            replication_subnet_group_identifier: Some(self.identifier.clone()),
            replication_subnet_group_description: Some(self.description.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            subnet_group_status: Some(SUBNET_GROUP_COMPLETE.to_string()),
            subnets: Some(
                self.subnets
                    .iter()
                    .map(|subnet| Subnet {
                        subnet_identifier: Some(subnet.identifier.clone()),
                        subnet_availability_zone: Some(AvailabilityZone {
                            name: Some(subnet.availability_zone.clone()),
                        }),
                        subnet_status: Some(SUBNET_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            // The IP addressing protocols a group supports follow from the address
            // families EC2 reports for its subnets.
            supported_network_types: Some(self.supported_network_types()),
            // A group a caller created is its own, not one DMS manages for a zero-ETL
            // integration, so it is not read-only.
            is_read_only: Some(DEFAULT_IS_READ_ONLY),
        }
    }
}

/// One replication instance.
#[derive(Debug, Clone)]
pub struct ReplicationInstanceRecord {
    /// The instance's ARN, `arn:aws:dms:<region>:<account>:rep:<resource id>`.
    pub arn: String,
    /// The caller's identifier, stored lowercased as the member doc requires.
    pub identifier: String,
    /// The compute and memory capacity the caller asked for.
    pub instance_class: String,
    /// The instance's status, one of the `ReplicationInstanceStatus` values.
    pub status: String,
    /// Storage allocated to the instance, in gigabytes.
    pub allocated_storage: i32,
    /// When `CreateReplicationInstance` minted it.
    pub create_time: SystemTime,
    /// The Availability Zone the instance runs in.
    pub availability_zone: String,
    /// The standby's Availability Zone, for a Multi-AZ instance only.
    pub secondary_availability_zone: Option<String>,
    /// The instance's engine version.
    pub engine_version: String,
    /// Whether the instance is a Multi-AZ deployment.
    pub multi_az: bool,
    /// Whether the instance has a public IP address.
    pub publicly_accessible: bool,
    /// Whether minor engine upgrades are applied automatically.
    pub auto_minor_version_upgrade: bool,
    /// The weekly maintenance window.
    pub preferred_maintenance_window: String,
    /// The KMS key the caller named, if any.
    pub kms_key_id: Option<String>,
    /// The custom DNS name servers the caller named, if any.
    pub dns_name_servers: Option<String>,
    /// The IP address protocol the caller named, if any.
    pub network_type: Option<String>,
    /// The VPC security groups the caller named.
    pub vpc_security_group_ids: Vec<String>,
    /// The identifier of the replication subnet group the caller placed the instance
    /// in, if any. Resolved against [`ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State::replication_subnet_groups`]
    /// when the instance is reported, so a group that has left the store is not
    /// reported as though it were still there.
    pub replication_subnet_group: Option<String>,
    /// The kerberos authentication settings the caller attached to the instance, if any.
    pub kerberos_authentication_settings: Option<KerberosAuthenticationSettings>,
    /// The settings `ModifyReplicationInstance` accepted for the instance but has not
    /// applied to it: a change that needs an outage waits for the maintenance window
    /// unless the modify asked for it to be applied immediately, so the instance keeps
    /// running with the values above until then and reports the waiting ones here.
    pub pending_modified_values: Option<ReplicationPendingModifiedValues>,
    /// The instance's tags.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this instance, if any.
    pub pending: Option<Pending>,
    /// Creation order, so a describe scans instances oldest first.
    pub sequence: u64,
}

impl ReplicationInstanceRecord {
    /// This record as the modelled `ReplicationInstance` shape, with its subnet group
    /// resolved against `subnet_groups`.
    pub fn to_shape(
        &self,
        subnet_groups: &BTreeMap<String, ReplicationSubnetGroupRecord>,
    ) -> ReplicationInstance {
        ReplicationInstance {
            replication_instance_identifier: Some(self.identifier.clone()),
            replication_instance_class: Some(self.instance_class.clone()),
            replication_instance_status: Some(self.status.clone()),
            allocated_storage: self.allocated_storage,
            instance_create_time: Some(tstamp(self.create_time)),
            vpc_security_groups: Some(
                self.vpc_security_group_ids
                    .iter()
                    .map(|id| VpcSecurityGroupMembership {
                        vpc_security_group_id: Some(id.clone()),
                        status: Some(VPC_SECURITY_GROUP_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            availability_zone: Some(self.availability_zone.clone()),
            // "The subnet group for the replication instance"
            // (`ReplicationInstance$ReplicationSubnetGroup`): the group the create
            // resolved, read back from the store rather than copied into the record.
            replication_subnet_group: self
                .replication_subnet_group
                .as_ref()
                .and_then(|identifier| subnet_groups.get(identifier))
                .map(ReplicationSubnetGroupRecord::to_shape),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            // The modifications DMS has accepted for the instance but not applied yet,
            // which only `ModifyReplicationInstance` leaves behind.
            pending_modified_values: self.pending_modified_values.clone(),
            multi_az: self.multi_az,
            engine_version: Some(self.engine_version.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            kms_key_id: self.kms_key_id.clone(),
            replication_instance_arn: Some(self.arn.clone()),
            // The emulator models no EC2, so an instance has no addresses to report;
            // both scalar members are deprecated in the model besides.
            replication_instance_public_ip_address: None,
            replication_instance_private_ip_address: None,
            replication_instance_public_ip_addresses: None,
            replication_instance_private_ip_addresses: None,
            replication_instance_ipv6_addresses: None,
            publicly_accessible: self.publicly_accessible,
            secondary_availability_zone: self.secondary_availability_zone.clone(),
            // Only an instance in the Free DMS program has an expiry.
            free_until: None,
            dns_name_servers: self.dns_name_servers.clone(),
            network_type: self.network_type.clone(),
            // Kerberos settings are only attached by a request that carries them.
            kerberos_authentication_settings: self.kerberos_authentication_settings.clone(),
        }
    }
}

/// The engine-specific settings structures an endpoint holds, as the request that
/// created it supplied them.
///
/// One field per settings member `CreateEndpoint` models; `EngineName` selects the one
/// an `Endpoint` reports. Build it through [`EngineSettings::sanitized`], which is what
/// keeps a secret out of the store — and so out of every response.
#[derive(Debug, Clone, Default)]
pub struct EngineSettings {
    /// Settings for an Amazon DynamoDB target.
    pub dynamo_db: Option<DynamoDbSettings>,
    /// Settings for an Amazon S3 target.
    pub s3: Option<S3Settings>,
    /// Settings for a DMS Transfer source.
    pub dms_transfer: Option<DmsTransferSettings>,
    /// Settings for a MongoDB source.
    pub mongo_db: Option<MongoDbSettings>,
    /// Settings for an Amazon Kinesis Data Streams target.
    pub kinesis: Option<KinesisSettings>,
    /// Settings for an Apache Kafka target.
    pub kafka: Option<KafkaSettings>,
    /// Settings for an OpenSearch target.
    pub elasticsearch: Option<ElasticsearchSettings>,
    /// Settings for an Amazon Neptune target.
    pub neptune: Option<NeptuneSettings>,
    /// Settings for an Amazon Redshift endpoint.
    pub redshift: Option<RedshiftSettings>,
    /// Settings for a PostgreSQL endpoint.
    pub postgre_sql: Option<PostgreSQLSettings>,
    /// Settings for a MySQL endpoint.
    pub my_sql: Option<MySQLSettings>,
    /// Settings for an Oracle endpoint.
    pub oracle: Option<OracleSettings>,
    /// Settings for a SAP ASE endpoint.
    pub sybase: Option<SybaseSettings>,
    /// Settings for a Microsoft SQL Server endpoint.
    pub microsoft_sql_server: Option<MicrosoftSQLServerSettings>,
    /// Settings for an IBM Db2 endpoint.
    pub ibm_db2: Option<IBMDb2Settings>,
    /// Settings for a DocumentDB endpoint.
    pub doc_db: Option<DocDbSettings>,
    /// Settings for a Redis target.
    pub redis: Option<RedisSettings>,
    /// Settings for a GCP MySQL source.
    pub gcp_my_sql: Option<GcpMySQLSettings>,
    /// Settings for an Amazon Timestream target.
    pub timestream: Option<TimestreamSettings>,
}

/// Build [`EngineSettings`] from the per-engine settings members of an endpoint request.
///
/// `CreateEndpoint` and `ModifyEndpoint` model the same settings members, so both read them
/// through this ONE list: a structure the model adds later reaches both paths by being named
/// here. The two requests are distinct generated types with the same member names, which is
/// why this is a macro over the binding rather than a function over a trait; it moves each
/// member out of that binding, exactly as writing the literal in the handler would.
macro_rules! engine_settings_from_input {
    ($input:ident) => {
        $crate::state::EngineSettings {
            dynamo_db: $input.dynamo_db_settings,
            s3: $input.s3_settings,
            dms_transfer: $input.dms_transfer_settings,
            mongo_db: $input.mongo_db_settings,
            kinesis: $input.kinesis_settings,
            kafka: $input.kafka_settings,
            elasticsearch: $input.elasticsearch_settings,
            neptune: $input.neptune_settings,
            redshift: $input.redshift_settings,
            postgre_sql: $input.postgre_sql_settings,
            my_sql: $input.my_sql_settings,
            oracle: $input.oracle_settings,
            sybase: $input.sybase_settings,
            microsoft_sql_server: $input.microsoft_sql_server_settings,
            ibm_db2: $input.ibm_db2_settings,
            doc_db: $input.doc_db_settings,
            redis: $input.redis_settings,
            gcp_my_sql: $input.gcp_my_sql_settings,
            timestream: $input.timestream_settings,
        }
    };
}

pub(crate) use engine_settings_from_input;

/// Clear each named member of an optional settings structure.
macro_rules! clear_members {
    ($holder:expr, $($member:ident),+ $(,)?) => {
        if let Some(settings) = $holder.as_mut() {
            $(settings.$member = None;)+
        }
    };
}

impl EngineSettings {
    /// These settings with every secret member dropped.
    ///
    /// A password a request carried is persisted nowhere, so no response can echo one
    /// back and nothing in the store holds a credential for a real database.
    pub fn sanitized(mut self) -> Self {
        clear_members!(self.my_sql, password);
        clear_members!(self.postgre_sql, password);
        clear_members!(self.oracle, password, asm_password, security_db_encryption);
        clear_members!(self.microsoft_sql_server, password);
        clear_members!(self.sybase, password);
        clear_members!(self.ibm_db2, password);
        clear_members!(self.mongo_db, password);
        clear_members!(self.doc_db, password);
        clear_members!(self.redshift, password);
        clear_members!(self.gcp_my_sql, password);
        clear_members!(self.redis, auth_password);
        clear_members!(self.kafka, sasl_password, ssl_client_key_password);
        self
    }
}

/// Fill the connection parameters an engine settings structure shares with the
/// endpoint's own members.
///
/// What the settings structure itself carries wins, and the endpoint's top-level
/// member fills what the structure left unset: an `Endpoint` reports the connection
/// parameters the request supplied, wherever it supplied them.
macro_rules! with_connection_parameters {
    ($record:expr, $settings:expr) => {{
        let record = $record;
        let mut settings = $settings;
        if settings.server_name.is_none() {
            settings.server_name = record.server_name.clone();
        }
        if settings.port.is_none() {
            settings.port = record.port;
        }
        if settings.database_name.is_none() {
            settings.database_name = record.database_name.clone();
        }
        if settings.username.is_none() {
            settings.username = record.username.clone();
        }
        settings
    }};
}

/// Report [`DEFAULT_AUTHENTICATION_METHOD`] for a settings structure whose engine
/// models an authentication method the request did not name.
macro_rules! with_default_authentication_method {
    ($settings:expr) => {{
        let mut settings = $settings;
        if settings.authentication_method.is_none() {
            settings.authentication_method = Some(DEFAULT_AUTHENTICATION_METHOD.to_string());
        }
        settings
    }};
}

/// One endpoint.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    /// The endpoint's ARN, `arn:aws:dms:<region>:<account>:endpoint:<resource id>`.
    pub arn: String,
    /// The caller's identifier.
    pub identifier: String,
    /// `source` or `target`, as `ReplicationEndpointTypeValue` models them for a
    /// request.
    pub endpoint_type: String,
    /// The database engine name the caller named.
    pub engine_name: String,
    /// The endpoint's status.
    pub status: String,
    /// The user name used to connect, if the caller named one.
    pub username: Option<String>,
    /// The server at the endpoint, if the caller named one.
    pub server_name: Option<String>,
    /// The port used to reach the endpoint, if the caller named one.
    pub port: Option<i32>,
    /// The database at the endpoint, if the caller named one.
    pub database_name: Option<String>,
    /// Extra connection attributes, if the caller named any.
    pub extra_connection_attributes: Option<String>,
    /// The SSL mode used to connect.
    pub ssl_mode: String,
    /// The key the endpoint's connection parameters are encrypted with: the one the
    /// caller named, or the region's default DMS key ([`default_kms_key_arn`]).
    pub kms_key_id: String,
    /// The certificate used for the SSL connection, if the caller named one.
    pub certificate_arn: Option<String>,
    /// The role DMS assumes to reach the endpoint, if the caller named one.
    pub service_access_role_arn: Option<String>,
    /// The external table definition, if the caller named one.
    pub external_table_definition: Option<String>,
    /// Whether DMS manages this endpoint as part of a zero-ETL integration, which
    /// makes it read-only.
    pub is_read_only: bool,
    /// The engine-specific settings the endpoint carries.
    pub settings: EngineSettings,
    /// The endpoint's tags.
    pub tags: Vec<Tag>,
    /// The transition the clock owes this endpoint, if any: `DeleteEndpoint` owes a
    /// deleting endpoint its removal.
    pub pending: Option<Pending>,
    /// Creation order, so a describe scans endpoints oldest first.
    pub sequence: u64,
}

impl EndpointRecord {
    /// This record as the modelled `Endpoint` shape — the ONE projection every
    /// operation that answers with an endpoint goes through, so the create path and
    /// the describe path report the same member set for the same endpoint.
    pub fn to_shape(&self) -> Endpoint {
        let mut endpoint = Endpoint {
            endpoint_identifier: Some(self.identifier.clone()),
            // A response carries the endpoint type uppercased; the lowercase
            // `source`/`target` of `ReplicationEndpointTypeValue` are the request form.
            endpoint_type: Some(self.endpoint_type.to_ascii_uppercase()),
            engine_name: Some(self.engine_name.clone()),
            engine_display_name: Some(engine_display_name(&self.engine_name)),
            username: self.username.clone(),
            server_name: self.server_name.clone(),
            port: self.port,
            database_name: self.database_name.clone(),
            extra_connection_attributes: self.extra_connection_attributes.clone(),
            status: Some(self.status.clone()),
            kms_key_id: Some(self.kms_key_id.clone()),
            endpoint_arn: Some(self.arn.clone()),
            certificate_arn: self.certificate_arn.clone(),
            ssl_mode: Some(self.ssl_mode.clone()),
            service_access_role_arn: self.service_access_role_arn.clone(),
            external_table_definition: self.external_table_definition.clone(),
            // A cross-account external id is minted by a cross-account create, which
            // this emulator does not model.
            external_id: None,
            is_read_only: Some(self.is_read_only),
            // A Lakehouse endpoint is one DMS creates itself for a zero-ETL
            // integration, so nothing here holds its settings.
            lakehouse_settings: None,
            ..Endpoint::default()
        };
        self.project_engine_settings(&mut endpoint);
        endpoint
    }

    /// Report the single engine settings structure this endpoint's `EngineName`
    /// selects, leaving every structure belonging to another engine absent.
    ///
    /// A structure whose members are all optional is reported for its engine even when
    /// the request carried none of it, because the endpoint's own connection
    /// parameters and the engine's documented defaults populate it. A structure the
    /// model gives required members (`ElasticsearchSettings`, `NeptuneSettings`,
    /// `RedisSettings`, `TimestreamSettings`, `DynamoDbSettings`) is reported only as
    /// the request supplied it, since nothing here can invent a required value.
    fn project_engine_settings(&self, endpoint: &mut Endpoint) {
        let settings = &self.settings;
        match self.engine_name.as_str() {
            "mysql" | "mariadb" | "aurora" | "aurora-serverless" => {
                endpoint.my_sql_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(self, settings.my_sql.clone().unwrap_or_default())
                ));
            }
            "postgres" | "aurora-postgresql" | "aurora-postgresql-serverless" | "babelfish" => {
                endpoint.postgre_sql_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(
                        self,
                        settings.postgre_sql.clone().unwrap_or_default()
                    )
                ));
            }
            "oracle" => {
                endpoint.oracle_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(self, settings.oracle.clone().unwrap_or_default())
                ));
            }
            "sqlserver" | "azuredb" | "azure-sql-managed-instance" => {
                endpoint.microsoft_sql_server_settings = Some(with_default_authentication_method!(
                    with_connection_parameters!(
                        self,
                        settings.microsoft_sql_server.clone().unwrap_or_default()
                    )
                ));
            }
            "sybase" => {
                endpoint.sybase_settings = Some(with_connection_parameters!(
                    self,
                    settings.sybase.clone().unwrap_or_default()
                ));
            }
            "db2" | "db2-zos" => {
                endpoint.ibm_db2_settings = Some(with_connection_parameters!(
                    self,
                    settings.ibm_db2.clone().unwrap_or_default()
                ));
            }
            "mongodb" => {
                endpoint.mongo_db_settings = Some(with_connection_parameters!(
                    self,
                    settings.mongo_db.clone().unwrap_or_default()
                ));
            }
            "docdb" | "documentdb" => {
                endpoint.doc_db_settings = Some(with_connection_parameters!(
                    self,
                    settings.doc_db.clone().unwrap_or_default()
                ));
            }
            "redshift" | "redshift-serverless" => {
                endpoint.redshift_settings = Some(with_connection_parameters!(
                    self,
                    settings.redshift.clone().unwrap_or_default()
                ));
            }
            "gcp-mysql" => {
                endpoint.gcp_my_sql_settings = Some(with_connection_parameters!(
                    self,
                    settings.gcp_my_sql.clone().unwrap_or_default()
                ));
            }
            "s3" => endpoint.s3_settings = settings.s3.clone(),
            "dms-transfer" => endpoint.dms_transfer_settings = settings.dms_transfer.clone(),
            "kinesis" => endpoint.kinesis_settings = settings.kinesis.clone(),
            "kafka" => endpoint.kafka_settings = settings.kafka.clone(),
            "dynamodb" => endpoint.dynamo_db_settings = settings.dynamo_db.clone(),
            "opensearch" | "elasticsearch" => {
                endpoint.elasticsearch_settings = settings.elasticsearch.clone();
            }
            "neptune" => endpoint.neptune_settings = settings.neptune.clone(),
            "redis" => endpoint.redis_settings = settings.redis.clone(),
            "timestream" => endpoint.timestream_settings = settings.timestream.clone(),
            // An engine name the model's catalog does not list selects no settings
            // structure, so reporting none is what keeps a response from carrying one
            // belonging to another engine.
            _ => {}
        }
    }
}

/// One replication task.
#[derive(Debug, Clone)]
pub struct ReplicationTaskRecord {
    /// The task's ARN, `arn:aws:dms:<region>:<account>:task:<resource id>`.
    pub arn: String,
    /// The caller's identifier.
    pub identifier: String,
    /// The ARN of the `source` endpoint the task reads.
    pub source_endpoint_arn: String,
    /// The ARN of the `target` endpoint the task writes.
    pub target_endpoint_arn: String,
    /// The ARN of the replication instance the task runs on.
    pub replication_instance_arn: String,
    /// `full-load`, `cdc` or `full-load-and-cdc`, as `MigrationTypeValue` models them.
    pub migration_type: String,
    /// The table mappings the caller supplied.
    pub table_mappings: String,
    /// The settings the task runs with: the caller's `ReplicationTaskSettings` resolved
    /// against the service defaults when the task was created
    /// ([`resolve_task_settings`]), which is the document every operation that answers
    /// with this task reports.
    pub replication_task_settings: Option<String>,
    /// Supplemental task data the caller supplied, if any.
    pub task_data: Option<String>,
    /// The task's status, one of the `ReplicationTask$Status` values.
    pub status: String,
    /// Why the task stopped, once it has.
    pub stop_reason: Option<String>,
    /// The last failure the task reported, if any.
    pub last_failure_message: Option<String>,
    /// When `CreateReplicationTask` minted it.
    pub creation_date: SystemTime,
    /// When `StartReplicationTask` last started it.
    pub start_date: Option<SystemTime>,
    /// Where a CDC operation starts, if the caller named a position.
    pub cdc_start_position: Option<String>,
    /// Where a CDC operation stops, if the caller named a position.
    pub cdc_stop_position: Option<String>,
    /// The task's tags.
    pub tags: Vec<Tag>,
    /// The run `StartReplicationTask` started, if the task has ever been started: what
    /// its full load moved, and what the clock still owes it
    /// ([`task_run`]).
    pub run: Option<TaskRun>,
    /// The transition the clock owes this task, if any.
    pub pending: Option<Pending>,
    /// Creation order, so a describe scans tasks oldest first.
    pub sequence: u64,
}

impl ReplicationTaskRecord {
    /// This record as the modelled `ReplicationTask` shape.
    ///
    /// `without_settings` drops `ReplicationTaskSettings`, which is what
    /// `DescribeReplicationTasks`' `WithoutSettings` member asks for.
    pub fn to_shape(&self, without_settings: bool) -> ReplicationTask {
        ReplicationTask {
            replication_task_identifier: Some(self.identifier.clone()),
            source_endpoint_arn: Some(self.source_endpoint_arn.clone()),
            target_endpoint_arn: Some(self.target_endpoint_arn.clone()),
            replication_instance_arn: Some(self.replication_instance_arn.clone()),
            migration_type: Some(self.migration_type.clone()),
            table_mappings: Some(self.table_mappings.clone()),
            replication_task_settings: if without_settings {
                None
            } else {
                self.replication_task_settings.clone()
            },
            status: Some(self.status.clone()),
            last_failure_message: self.last_failure_message.clone(),
            stop_reason: self.stop_reason.clone(),
            replication_task_creation_date: Some(tstamp(self.creation_date)),
            replication_task_start_date: self.start_date.map(tstamp),
            cdc_start_position: self.cdc_start_position.clone(),
            cdc_stop_position: self.cdc_stop_position.clone(),
            // A recovery checkpoint is produced by a CDC operation that has read
            // changes; nothing in this emulator reads a source database, so there is
            // no checkpoint to report.
            recovery_checkpoint: None,
            replication_task_arn: Some(self.arn.clone()),
            // The statistics of the run this task is on, or has finished: the full load's
            // progress, what it loaded and the instants it was started and stopped at
            // ([`ReplicationTaskRecord::stats`]). A task that has never run reports none.
            replication_task_stats: self.stats(),
            task_data: self.task_data.clone(),
            // Only `MoveReplicationTask` sets the instance a task is moving to.
            target_replication_instance_arn: None,
        }
    }
}

/// Per-service state for com-amazonaws-databasemigrationservice-amazondmsv20160101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State {
    /// Every replication instance this emulator holds, keyed by ARN.
    pub replication_instances: BTreeMap<String, ReplicationInstanceRecord>,
    /// `ReplicationInstanceIdentifier` -> ARN, so a duplicate create is refused.
    pub instance_arn_by_identifier: BTreeMap<String, String>,
    /// Every replication subnet group this emulator holds, keyed by its (lowercased)
    /// identifier, which is how `CreateReplicationSubnetGroup` stores one and how
    /// `CreateReplicationInstance` and `DeleteReplicationSubnetGroup` resolve one.
    pub replication_subnet_groups: BTreeMap<String, ReplicationSubnetGroupRecord>,
    /// Every endpoint this emulator holds, keyed by ARN.
    pub endpoints: BTreeMap<String, EndpointRecord>,
    /// `EndpointIdentifier` -> ARN, so a duplicate create is refused.
    pub endpoint_arn_by_identifier: BTreeMap<String, String>,
    /// Every replication task this emulator holds, keyed by ARN.
    pub replication_tasks: BTreeMap<String, ReplicationTaskRecord>,
    /// `ReplicationTaskIdentifier` -> ARN, so a duplicate create is refused.
    pub task_arn_by_identifier: BTreeMap<String, String>,
    /// Bumped once per created resource, so a describe can scan in creation order.
    pub next_sequence: u64,
    /// Bumped once per minted resource id, so two ids minted in the same instant
    /// differ.
    pub id_counter: u64,
}

impl ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State {
    /// Materialise every status transition the clock has already made due.
    ///
    /// Called at the top of every handler, including the reporting ones: the change
    /// was owed at `now` whether or not anybody called, so applying it here is what
    /// makes a read and a later read agree.
    pub fn settle(&mut self, now: SystemTime) {
        // A replication instance whose `deleting` span has elapsed leaves the store, and
        // the `ReplicationInstanceIdentifier` it held becomes free for a new instance: an
        // identifier is unique only among the instances the account HOLDS.
        let removed: Vec<(String, String)> = self
            .replication_instances
            .values()
            .filter(|instance| {
                instance
                    .pending
                    .is_some_and(|pending| now >= pending.due && pending.owed == Owed::Removal)
            })
            .map(|instance| (instance.arn.clone(), instance.identifier.clone()))
            .collect();
        for (arn, identifier) in removed {
            self.replication_instances.remove(&arn);
            // Only while the identifier still points at the instance that is leaving: a
            // create that has already taken the identifier owns the entry.
            if self
                .instance_arn_by_identifier
                .get(&identifier)
                .is_some_and(|held| *held == arn)
            {
                self.instance_arn_by_identifier.remove(&identifier);
            }
        }

        for instance in self.replication_instances.values_mut() {
            if let Some(pending) = instance.pending {
                if now >= pending.due {
                    instance.pending = None;
                    if let Owed::ToStatus(next) = pending.owed {
                        instance.status = next.to_string();
                    }
                }
            }
        }

        // An endpoint whose `deleting` span has elapsed leaves the store, and the
        // `EndpointIdentifier` it held becomes free for a new endpoint: an identifier is
        // unique only among the endpoints the account HOLDS.
        let removed: Vec<(String, String)> = self
            .endpoints
            .values()
            .filter(|endpoint| {
                endpoint
                    .pending
                    .is_some_and(|pending| now >= pending.due && pending.owed == Owed::Removal)
            })
            .map(|endpoint| (endpoint.arn.clone(), endpoint.identifier.clone()))
            .collect();
        for (arn, identifier) in removed {
            self.endpoints.remove(&arn);
            // Only while the identifier still points at the endpoint that is leaving: a
            // create that has already taken the identifier owns the entry.
            if self
                .endpoint_arn_by_identifier
                .get(&identifier)
                .is_some_and(|held| *held == arn)
            {
                self.endpoint_arn_by_identifier.remove(&identifier);
            }
        }

        for endpoint in self.endpoints.values_mut() {
            if let Some(pending) = endpoint.pending {
                if now >= pending.due {
                    endpoint.pending = None;
                    if let Owed::ToStatus(next) = pending.owed {
                        endpoint.status = next.to_string();
                    }
                }
            }
        }

        // A task whose `deleting` span has elapsed leaves the store, and its
        // identifier becomes free again.
        let removed: Vec<(String, String)> = self
            .replication_tasks
            .values()
            .filter(|task| {
                task.pending
                    .is_some_and(|pending| now >= pending.due && pending.owed == Owed::Removal)
            })
            .map(|task| (task.arn.clone(), task.identifier.clone()))
            .collect();
        for (arn, identifier) in removed {
            self.replication_tasks.remove(&arn);
            self.task_arn_by_identifier.remove(&identifier);
        }

        for task in self.replication_tasks.values_mut() {
            if let Some(pending) = task.pending {
                if now >= pending.due {
                    task.pending = None;
                    if let Owed::ToStatus(next) = pending.owed {
                        task.status = next.to_string();
                    }
                }
            }
            // A started task is owed more than its next status: the run it is on records
            // the time it has been running, and a full load whose finish the clock has
            // passed is finished — which, for a task with no phase after it, is the stop
            // it arrives at on its own.
            task.settle_run(now);
        }
    }

    /// The next creation sequence number.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.next_sequence
    }

    /// The replication instances this emulator holds, oldest first.
    pub fn ordered_instances(&self) -> Vec<&ReplicationInstanceRecord> {
        let mut records: Vec<&ReplicationInstanceRecord> =
            self.replication_instances.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// The endpoints this emulator holds, oldest first.
    pub fn ordered_endpoints(&self) -> Vec<&EndpointRecord> {
        let mut records: Vec<&EndpointRecord> = self.endpoints.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// The replication subnet groups this emulator holds, in identifier order.
    ///
    /// A group is held under its identifier ("This value is stored as a lowercase
    /// string"), which is also the only handle an operation names one by, so the key
    /// order is a stable order for reporting them: two readings of an unchanged account
    /// list the same groups in the same order, which is what a paged describe resumes
    /// against.
    pub fn ordered_subnet_groups(&self) -> Vec<&ReplicationSubnetGroupRecord> {
        self.replication_subnet_groups.values().collect()
    }

    /// The replication tasks this emulator holds, oldest first.
    pub fn ordered_tasks(&self) -> Vec<&ReplicationTaskRecord> {
        let mut records: Vec<&ReplicationTaskRecord> = self.replication_tasks.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }

    /// 32 bytes derived from `domain`, a per-store counter and the wall clock, so
    /// every minted resource id is distinct from every other.
    fn digest(&mut self, domain: &str) -> [u8; 32] {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        hasher.update(self.id_counter.to_be_bytes());
        hasher.update(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes(),
        );
        hasher.finalize().into()
    }

    /// A fresh resource id for the last field of a DMS ARN: uppercase letters and
    /// digits, beginning with a letter, inside the 31 characters the
    /// `ResourceIdentifier` doc allows.
    pub fn mint_resource_id(&mut self, domain: &str) -> String {
        let digest = self.digest(domain);
        let mut id: String = digest
            .iter()
            .take(MINTED_RESOURCE_ID_LEN)
            .map(|byte| char::from(RESOURCE_ID_ALPHABET[usize::from(*byte) % RESOURCE_ID_ALPHABET.len()]))
            .collect();
        // "can only begin with a letter": fold a leading digit onto a letter rather
        // than redrawing, so the id stays a pure function of the digest.
        if id.starts_with(|c: char| c.is_ascii_digit()) {
            let rest = id.split_off(1);
            id = format!("A{rest}");
        }
        id
    }
}

/// A stored instant as the modelled `TStamp`, at the precision this service reports a
/// timestamp at.
///
/// Every date a DMS response carries travels as epoch seconds with MILLISECOND precision —
/// the resolution an AWS timestamp has on this wire — so an instant read off the host clock
/// is truncated to it HERE, in the one place the records are projected through, rather than
/// reported with the clock's own sub-millisecond digits.
pub fn tstamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_millis(
        i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatabasemigrationserviceAmazondmsv20160101State>)
}

/// The region a request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A DMS ARN for `resource_type` (`rep`, `endpoint`, `task`) and `resource_id`.
///
/// Built through `emulator_server::emulator_arn`, which is the single source of the
/// canonical account id every emitted ARN carries.
pub fn dms_arn(region: &str, resource_type: &str, resource_id: &str) -> String {
    emulator_server::emulator_arn("dms", region, &format!("{resource_type}:{resource_id}"))
}

/// The Availability Zone a replication instance is placed in when the request names
/// none: the request's region plus a zone letter.
///
/// The doc says the default is "a random, system-chosen Availability Zone in the
/// endpoint's Amazon Web Services Region, for example: us-east-1d"; the emulator
/// models no EC2, so it picks one deterministically in the same form.
pub fn availability_zone_for(region: &str, index: usize) -> String {
    let letter = char::from(b'a' + (index % 26) as u8);
    format!("{region}{letter}")
}

/// Why an identifier was refused, as a sentence that names the member and the
/// constraint it broke.
///
/// The constraints are the ones the `ReplicationInstanceIdentifier`,
/// `EndpointIdentifier` and `ReplicationTaskIdentifier` docs publish, and the ones the
/// `ResourceIdentifier` doc publishes for the ARN suffix: ASCII letters, digits and
/// hyphens only, a letter first, no trailing hyphen, no two consecutive hyphens, and
/// (where the doc gives one) a maximum length.
pub fn identifier_refusal(member: &str, value: &str, max_len: Option<usize>) -> Option<String> {
    if value.is_empty() {
        return Some(format!("{member} must not be empty"));
    }
    if let Some(max_len) = max_len {
        if value.chars().count() > max_len {
            return Some(format!(
                "{member} must contain at most {max_len} characters, and {value} has {}",
                value.chars().count()
            ));
        }
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Some(format!(
            "{member} must contain only ASCII letters, digits and hyphens: {value}"
        ));
    }
    if !value.as_bytes()[0].is_ascii_alphabetic() {
        return Some(format!(
            "{member} must begin with a letter: {value}"
        ));
    }
    if value.ends_with('-') {
        return Some(format!("{member} must not end with a hyphen: {value}"));
    }
    if value.contains("--") {
        return Some(format!(
            "{member} must not contain two consecutive hyphens: {value}"
        ));
    }
    None
}

/// The request's tags, each carrying the ARN of the resource they were attached to,
/// which is what `Tag$ResourceArn` reports.
pub fn tags_for(arn: &str, tags: Option<Vec<Tag>>) -> Vec<Tag> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| Tag {
            resource_arn: Some(arn.to_string()),
            ..tag
        })
        .collect()
}

/// A `Marker` that resumes a paged describe at `arn`.
///
/// Hex-encoded rather than returned raw so a client cannot read a resource ARN out of
/// a pagination token, and so a marker this emulator did not mint is refused rather
/// than silently treated as an ARN.
pub fn encode_marker(arn: &str) -> String {
    hex::encode(arn)
}

/// The ARN a `Marker` resumes at, or `None` when the marker was not minted by
/// [`encode_marker`].
pub fn decode_marker(marker: &str) -> Option<String> {
    let bytes = hex::decode(marker).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    (!arn.is_empty()).then_some(arn)
}

/// The page size a paginated describe uses for `max_records`, or the reason the
/// request is refused.
///
/// The member doc publishes both the default and the range: "Default: 100" and
/// "Constraints: Minimum 20, maximum 100", which the model's plain `IntegerOptional`
/// does not express.
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    page_size_in_range(max_records, MAX_RECORDS_MIN, MAX_RECORDS_MAX)
}

/// The same reading of `MaxRecords` for an operation whose own member doc publishes a
/// DIFFERENT range — `DescribeTableStatistics` accepts up to 500 of them — so both
/// operations refuse a value outside their range with one sentence rather than two
/// readings of the same member.
pub fn page_size_in_range(
    max_records: Option<i32>,
    minimum: i32,
    maximum: i32,
) -> Result<usize, String> {
    let requested = max_records.unwrap_or(DEFAULT_MAX_RECORDS);
    let refusal =
        || format!("MaxRecords {requested} is outside the supported range {minimum}-{maximum}");
    if !(minimum..=maximum).contains(&requested) {
        return Err(refusal());
    }
    usize::try_from(requested).map_err(|_| refusal())
}

/// The span of `records` one page covers when resuming at `marker`, plus the marker
/// that resumes after it.
///
/// `key` names a record's pagination key (its ARN). The marker is "resume AT this
/// key", and the final page carries none, which is what the `@paginated` contract
/// means by an absent `outputToken`.
pub fn paginate<T>(
    records: &[T],
    marker: Option<&String>,
    size: usize,
    key: impl Fn(&T) -> &str,
) -> Result<(::std::ops::Range<usize>, Option<String>), String> {
    let start = match marker {
        Some(marker) => {
            let resume_at = decode_marker(marker)
                .ok_or_else(|| "Marker was not produced by a previous response".to_string())?;
            records
                .iter()
                .position(|record| key(record) == resume_at)
                .ok_or_else(|| {
                    "Marker resumes at a resource that is no longer described".to_string()
                })?
        }
        None => 0,
    };

    let end = records.len().min(start.saturating_add(size));
    let next = (end < records.len()).then(|| encode_marker(key(&records[end])));
    Ok((start..end, next))
}

/// Why a `ReplicationSubnetGroupIdentifier` was refused, as a sentence naming the
/// constraint it broke, or `None` when it is one DMS accepts.
///
/// The constraints are the ones the member doc publishes: "Must contain no more than 255
/// alphanumeric characters, periods, underscores, or hyphens. Must not be \"default\"" —
/// a different alphabet from the one [`identifier_refusal`] holds the other identifiers
/// to, so the two readings stay apart. The comparison against the reserved name is made
/// on the folded value, since the identifier "is stored as a lowercase string".
pub fn subnet_group_identifier_refusal(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some("ReplicationSubnetGroupIdentifier must not be empty".to_string());
    }
    if value.chars().count() > SUBNET_GROUP_IDENTIFIER_MAX_LEN {
        return Some(format!(
            "ReplicationSubnetGroupIdentifier must contain at most \
             {SUBNET_GROUP_IDENTIFIER_MAX_LEN} characters, and {value} has {}",
            value.chars().count()
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.')
    {
        return Some(format!(
            "ReplicationSubnetGroupIdentifier must contain only alphanumeric characters, \
             periods, underscores and hyphens: {value}"
        ));
    }
    if value.to_lowercase() == RESERVED_SUBNET_GROUP_IDENTIFIER {
        return Some(format!(
            "ReplicationSubnetGroupIdentifier must not be \"{RESERVED_SUBNET_GROUP_IDENTIFIER}\""
        ));
    }
    None
}

/// DMS's sentence for a `ReplicationSubnetGroupIdentifier` that names no group the
/// account holds, naming the value as the CALLER sent it.
///
/// One wording for the one fact, so the operations that resolve a group identifier —
/// `CreateReplicationInstance`, which refuses the parameter, and
/// `DeleteReplicationSubnetGroup`, which answers the modelled `ResourceNotFoundFault` —
/// report it identically.
pub fn subnet_group_absent_message(identifier: &str) -> String {
    format!("Replication Subnet Group '{identifier}' does not exist.")
}

/// DMS's sentence for an `EndpointIdentifier` another endpoint of the account already
/// holds, naming the identifier the CALLER sent, double-quoted.
///
/// The existing endpoint's ARN has no place in the message text — it travels in the
/// modelled `ResourceAlreadyExistsFault$resourceArn` member instead — and one wording
/// serves both operations that take an identifier, `CreateEndpoint` and `ModifyEndpoint`.
pub fn endpoint_identifier_in_use_message(identifier: &str) -> String {
    format!(r#"ReplicationEndpoint "{identifier}" already in use."#)
}

/// What DMS answers a `ReplicationInstanceIdentifier` an instance of the account already
/// holds.
///
/// An identifier names ONE replication instance of the account, so the refusal reports
/// the identifier the caller sent and the instance that holds it. It serves both
/// operations that take one, `CreateReplicationInstance` and
/// `ModifyReplicationInstance`, so a duplicate and a rename onto a taken identifier are
/// reported identically.
pub fn instance_identifier_in_use_message(identifier: &str, existing_arn: &str) -> String {
    format!("replication instance {identifier} already exists as {existing_arn}")
}

/// DMS's sentence for a service access role it cannot use, naming the role ARN the
/// CALLER sent.
///
/// The fixed text is DMS's own for the `AccessDeniedFault` class ("DMS was denied access
/// to the endpoint. Check that the role is correctly configured", as the modelled shape
/// documents it); the ARN is the request's.
pub fn role_not_configured_message(role_arn: &str) -> String {
    format!("The IAM Role {role_arn} is not configured properly.AccessDenied")
}

/// Every service access role ARN an endpoint request carries: the endpoint's own
/// `ServiceAccessRoleArn` and the per-engine settings structures' copies of it.
///
/// One list for the one question "which roles does this endpoint ask DMS to assume",
/// because the member is modelled on the endpoint AND on each engine settings structure
/// that needs a role of its own (`DynamoDbSettings`, `S3Settings`, `DmsTransferSettings`,
/// `KinesisSettings`, `ElasticsearchSettings`, `NeptuneSettings`, `RedshiftSettings`,
/// `PostgreSQLSettings`, `MySQLSettings`), and each copy is documented the same way:
/// "The role must allow the iam:PassRole action".
///
/// Order follows the request: the endpoint's own member first, then the settings
/// structures, so a refusal names the first role the request gave that DMS cannot use.
pub fn service_access_role_arns(
    endpoint_role_arn: Option<&str>,
    settings: &EngineSettings,
) -> Vec<String> {
    let per_engine: [Option<&str>; 9] = [
        settings
            .dynamo_db
            .as_ref()
            .map(|it| it.service_access_role_arn.as_str()),
        settings
            .s3
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .dms_transfer
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .kinesis
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .elasticsearch
            .as_ref()
            .map(|it| it.service_access_role_arn.as_str()),
        settings
            .neptune
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .redshift
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .postgre_sql
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
        settings
            .my_sql
            .as_ref()
            .and_then(|it| it.service_access_role_arn.as_deref()),
    ];
    ::std::iter::once(endpoint_role_arn)
        .chain(per_engine)
        .flatten()
        .filter(|arn| !arn.is_empty())
        .map(::std::string::ToString::to_string)
        .collect()
}

/// What EC2 answered about the subnets a request named.
#[derive(Debug, Clone)]
pub enum PlacedSubnets {
    /// EC2 reported every id, and they all belong to one VPC: that VPC, and each subnet
    /// with the Availability Zone it sits in, in the order the request named them.
    InOneVpc {
        /// The VPC every named subnet belongs to.
        vpc_id: String,
        /// The named subnets, request order.
        subnets: Vec<SubnetRecord>,
    },
    /// EC2 answered that this value names no subnet of the account.
    Absent(String),
    /// EC2 reported the subnets, and they are spread over more than one VPC — so there is
    /// no single `VpcId` a group of them could be reported under.
    AcrossVpcs(Vec<String>),
    /// Nothing was learned: there is no EC2 in this emulator to ask, or its answer could
    /// not be read.
    Unresolved,
}

/// The subnets `subnet_ids` names, resolved against the account's EC2 network.
///
/// A subnet is Amazon EC2's resource and not DMS's, so what a request names is resolved
/// against EC2, reached the way any emulated service reaches another: the AWS SDK driven
/// over the emulator's OWN in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
/// service crate and nothing here opens a socket.
///
/// Each id is asked about on its own, so the answer says WHICH value named no subnet.
/// The list is bounded by the caller before it gets here ([`MAX_SUBNETS_PER_GROUP`]).
pub async fn placed_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> PlacedSubnets {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let Some(ec2) = ec2_client(ctx, region) else {
        return PlacedSubnets::Unresolved;
    };
    let mut subnets: Vec<SubnetRecord> = Vec::with_capacity(subnet_ids.len());
    let mut vpc_ids: Vec<String> = Vec::new();
    for subnet_id in subnet_ids {
        let answer = ec2
            .describe_subnets()
            .subnet_ids(subnet_id.clone())
            .send()
            .await;
        let reported = match answer {
            Ok(output) => output
                .subnets()
                .iter()
                .find(|subnet| subnet.subnet_id() == Some(subnet_id.as_str()))
                .cloned(),
            Err(error) => {
                let code = error
                    .as_service_error()
                    .and_then(ProvideErrorMetadata::code)
                    .unwrap_or_default()
                    .to_string();
                // `InvalidSubnetID.NotFound` for an id no subnet carries,
                // `InvalidSubnetID.Malformed` for a value that is not a subnet id at
                // all: both say the value names no subnet of the account.
                if code.starts_with(SUBNET_ID_REFUSAL_PREFIX) {
                    return PlacedSubnets::Absent(subnet_id.clone());
                }
                // Any other answer is this emulator failing to ASK rather than EC2
                // reporting a subnet the account does not have, so nothing is concluded
                // from it — and it is worth a warning, because an emulator with no EC2
                // mounted cannot place a subnet group at all.
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a request named could not be resolved against EC2"
                );
                return PlacedSubnets::Unresolved;
            }
        };
        // EC2 reports one subnet per id it resolved, so an id it reported nothing for
        // names no subnet of the account.
        let Some(reported) = reported else {
            return PlacedSubnets::Absent(subnet_id.clone());
        };
        let (Some(vpc_id), Some(availability_zone)) =
            (reported.vpc_id(), reported.availability_zone())
        else {
            // A subnet EC2 reported without a VPC or a zone is an answer this cannot
            // read, not a subnet the account lacks.
            ::tracing::warn!(
                subnet = %subnet_id,
                "EC2 reported a subnet with no VPC or no Availability Zone"
            );
            return PlacedSubnets::Unresolved;
        };
        if !vpc_ids.iter().any(|known| known == vpc_id) {
            vpc_ids.push(vpc_id.to_string());
        }
        subnets.push(SubnetRecord {
            identifier: subnet_id.clone(),
            availability_zone: availability_zone.to_string(),
            ipv4: reported
                .cidr_block()
                .is_some_and(|block| !block.is_empty()),
            ipv6: reported
                .ipv6_cidr_block_association_set()
                .iter()
                .filter(|association| {
                    // An association that is on its way out (`disassociating`,
                    // `disassociated`) or that never took (`failing`, `failed`) does not
                    // give the subnet a range; one with no state reported is read as
                    // holding its block.
                    association
                        .ipv6_cidr_block_state()
                        .and_then(|state| state.state())
                        .is_none_or(|state| {
                            matches!(
                                state,
                                ::aws_sdk_ec2::types::SubnetCidrBlockStateCode::Associating
                                    | ::aws_sdk_ec2::types::SubnetCidrBlockStateCode::Associated
                            )
                        })
                })
                .any(|association| {
                    association
                        .ipv6_cidr_block()
                        .is_some_and(|block| !block.is_empty())
                }),
        });
    }
    match vpc_ids.len() {
        1 => PlacedSubnets::InOneVpc {
            vpc_id: vpc_ids.into_iter().next().unwrap_or_default(),
            subnets,
        },
        0 => PlacedSubnets::Unresolved,
        _ => PlacedSubnets::AcrossVpcs(vpc_ids),
    }
}

/// The Availability Zones of `region`, as EC2 reports them, or an empty list when there
/// is no EC2 to ask.
///
/// The zones of a Region are a fact of the account's network rather than of DMS, so they
/// are read from the service that owns them, over the same in-process callback
/// [`placed_subnets`] uses.
pub async fn region_availability_zones(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<String> {
    let Some(ec2) = ec2_client(ctx, region) else {
        return Vec::new();
    };
    match ec2.describe_availability_zones().send().await {
        Ok(output) => output
            .availability_zones()
            .iter()
            .filter_map(|zone| zone.zone_name().map(::std::string::ToString::to_string))
            .collect(),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Region's Availability Zones could not be read from EC2"
            );
            Vec::new()
        }
    }
}

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` for an id no subnet carries, `InvalidSubnetID.Malformed`
/// for a value that is not a subnet id at all.
const SUBNET_ID_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 client this service resolves a caller-named subnet through, addressed to
/// `region`.
///
/// The transport is the emulator's own in-process callback, so there is nothing to retry,
/// time out or watch for a stalled stream — and each of those would need a sleep
/// implementation this client is not given. The same goes for caching the identity: the
/// credentials the callback hands over are used directly.
///
/// `None` when the request carries no Region LABEL: the Region a request is signed for is
/// caller-supplied text and here it would end up in an endpoint URL's authority, so it is
/// read as a label (`service_common::arn::region_label`, this tree's one statement of
/// that grammar) rather than interpolated as given.
fn ec2_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let Some(label) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return None;
    };
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(label.to_string()))
        .endpoint_url(format!("https://ec2.{label}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}
