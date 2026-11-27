// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rds-amazonrdsv19 service.
//!
//! The DB instance, DB snapshot, DB cluster, DB shard group, option group, DB subnet
//! group, cluster parameter group, DB parameter group, event subscription and global
//! cluster records the
//! implemented handlers read and write, plus the projection of a stored record onto the
//! modelled
//! `DBInstance` / `DBSnapshot` / `DBCluster` / `DBShardGroup` / `OptionGroup` /
//! `DBSubnetGroup` /
//! `DBClusterParameterGroup` / `DBParameterGroup` / `EventSubscription` /
//! `GlobalCluster` wire shapes. These live here rather than in one operation file because
//! every implemented operation shares them: `CreateDBInstance`, `DescribeDBInstances`,
//! `ModifyDBInstance`, `CreateDBSnapshot`, `DescribeDBSnapshots`, `DeleteDBInstance`,
//! `CreateDBCluster`, `DescribeDBClusters`,
//! `CreateDBShardGroup`, `DescribeDBShardGroups`,
//! `CreateOptionGroup`, `ModifyOptionGroup`, `DescribeOptionGroups`,
//! `DeleteOptionGroup`, `CreateDBSubnetGroup`, `DescribeDBSubnetGroups`, the six
//! cluster-parameter-group operations, `CreateDBParameterGroup`,
//! `DescribeDBParameterGroups`, `ModifyDBParameterGroup`, `DescribeDBParameters`,
//! `DeleteDBParameterGroup`,
//! `CreateEventSubscription`,
//! `DescribeEventSubscriptions`, `AddSourceIdentifierToSubscription`,
//! `CreateGlobalCluster`, `DescribeGlobalClusters`,
//! `CreateDBSecurityGroup` and `DescribeDBSecurityGroups` all
//! project the same records.
//!
//! RDS is a REGIONAL service, so every map is keyed by `(region, identifier)`: the
//! region a request is signed for scopes what it can see and create, one identifier
//! can name a different instance in each region, and a describe signed for one region
//! never reports another's.
//!
//! Keying by region is also what bounds the store and every scan, which matters
//! because the region comes from the request's own SigV4 credential scope and this
//! emulator authenticates nobody. Three bounds hold together: [`check_region`] admits
//! only a value of the published AWS region-id form (so a region key is at most 32
//! bytes), [`MAX_REGIONS`] caps how many distinct regions hold records at all, and the
//! per-region quotas cap the records in each. The store therefore holds at most
//! `MAX_REGIONS * DB_INSTANCE_QUOTA_PER_REGION` instances,
//! `MAX_REGIONS * DB_SNAPSHOT_QUOTA_PER_REGION` snapshots — each holding a copy of the
//! source instance's database, itself bounded by the page ceiling every database of this
//! service's data plane carries ([`data_plane::sql_engine`]) —
//! `MAX_REGIONS * DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION` DB cluster snapshots — each holding
//! a copy of the source cluster's database under that same page ceiling, which is what a
//! restore of one loads —
//! `MAX_REGIONS * DB_CLUSTER_QUOTA_PER_REGION` DB clusters — each carrying at most
//! [`MAX_CLUSTER_LIST_VALUES`] values in each of its stored string lists, at most
//! [`MAX_BACKTRACK_POINTS_PER_CLUSTER`] consistent points it can be backtracked to (each a
//! copy of its database under that same page ceiling) and at most
//! [`MAX_BACKTRACKS_PER_CLUSTER`] records of the backtracks performed on it —
//! `MAX_REGIONS * DB_SHARD_GROUP_QUOTA_PER_REGION` DB shard groups,
//! at most `MAX_REGIONS * DB_CAPACITY_RESERVATION_QUOTA_PER_REGION` DB capacity
//! reservations — whatever state each is in, and each holding capacity for at most that
//! region's DB instance allowance —
//! at most `MAX_REGIONS * RESERVED_DB_INSTANCE_RECORD_QUOTA_PER_REGION` purchased reserved
//! DB instances, which together reserve at most that region's
//! `RESERVED_DB_INSTANCE_QUOTA_PER_REGION` DB instances,
//! `MAX_REGIONS * OPTION_GROUP_QUOTA_PER_REGION` option groups and
//! `MAX_REGIONS * DB_SUBNET_GROUP_QUOTA_PER_REGION` DB subnet groups,
//! `MAX_REGIONS * DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION` cluster parameter groups
//! — each of those carrying at most [`MAX_PARAMETERS_PER_GROUP`] parameters of at most
//! [`MAX_FREE_FORM_LEN`] characters, a family string of at most [`MAX_FREE_FORM_LEN`]
//! characters and at most [`MAX_TAGS_PER_RESOURCE`] tags of at most [`MAX_TAG_KEY_LEN`] /
//! [`MAX_TAG_VALUE_LEN`] characters — at most
//! `MAX_REGIONS * DB_PARAMETER_GROUP_QUOTA_PER_REGION` DB parameter groups, each carrying
//! a family string and a description of at most [`MAX_FREE_FORM_LEN`] characters, at most
//! [`MAX_PARAMETERS_PER_GROUP`] parameters of at most [`MAX_FREE_FORM_LEN`] characters and
//! the same bounded tag list, at most
//! `MAX_REGIONS * CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION` custom engine versions,
//! at most `MAX_REGIONS * DB_SECURITY_GROUP_QUOTA_PER_REGION` DB security groups — each
//! carrying at most [`MAX_AUTHORIZATIONS_PER_DB_SECURITY_GROUP`] ingress rules and the
//! same bounded tag list —
//! and at most one CA certificate override per region,
//! and at most `MAX_REGIONS * MAX_EVENTS_PER_REGION` recorded lifecycle events —
//! the log `DescribeEvents` reports, a ring per region (see [`events`]) —
//! and at most [`CONFIGURATION_CHECKS`]`.len()` recommendations per DB instance or DB
//! cluster the configuration analysis found an issue in (see [`recommendations`]),
//! whatever sequence of requests arrives, and every
//! per-request scan reads one region's ordered range rather than the whole map.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AvailabilityZone, AvailableCapacityPerAvailabilityZone, BacktrackDBClusterOutput, Certificate,
    CertificateDetails,
    ClusterPendingModifiedValues,
    CreateDBClusterEndpointOutput, CreateDBShardGroupOutput,
    DBCapacityReservation, DBCapacityReservationCommitmentInfo,
    DBCluster, DBClusterAutomatedBackup, DBClusterMember, DBClusterOptionGroupStatus,
    DBClusterParameterGroup, DBClusterRole, DBClusterSnapshot, DBClusterSnapshotAttribute,
    DBClusterSnapshotAttributesResult, DBInstance, DBInstanceAutomatedBackup,
    DBInstanceAutomatedBackupsReplication, DBInstanceRole,
    DBInstanceStatusInfo,
    DBParameterGroup,
    DBParameterGroupStatus,
    DBSecurityGroup, DBSecurityGroupMembership, DBSnapshot, DBSnapshotAttribute,
    DBSnapshotAttributesResult, DBSubnetGroup,
    DomainMembership, EC2SecurityGroup, Endpoint, EventCategoriesMap, EventSubscription,
    GlobalCluster, GlobalClusterMember, IPRange, LimitlessDatabase, MasterUserSecret, Option2,
    OptionGroup,
    OptionGroupMembership,
    OptionSetting, Outpost, Parameter,
    PendingCloudwatchLogsExports, PendingModifiedValues, RestoreWindow,
    ScalingConfigurationInfo,
    ServerlessV2ScalingConfigurationInfo, SourceNotFoundFault, Subnet, Tag,
    VpcSecurityGroupMembership,
};

// The catalog of engine parameters each family publishes, and the rules over it, are
// SHARED with the Amazon DocumentDB and Amazon Neptune models of the same front door —
// this service serves their SDK clients' traffic too, since all three sign `rds` — so they
// live once in `service_common::rds_cluster_parameters` and are re-exported here: a handler
// reads them as `crate::state::<name>`. The family rule is this crate's own
// (`validate_parameter_group_family` in `state::parameter_groups`), because the families
// that exist are the ones this front door's engine-version catalogue publishes.
pub use service_common::rds_cluster_parameters::{
    change_is_pending, change_is_pending_for, default_apply_method, default_apply_method_for,
    minimum_engine_version_in_family, not_a_valid_identifier_message,
    parameter_cannot_be_modified_message, parameter_group_family_not_valid_message,
    parameter_has_pending_changes_message, parameter_not_found_message, parameter_source_filter,
    published_parameter,
    published_parameters, spec_in_catalog,
    validate_apply_method,
    validate_known_setting, validate_parameter_group_name,
    validate_parameter_name, validate_parameter_setting, validate_setting_in_catalog,
    ClusterParameterSpec,
    ParameterSourceFilter, APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT,
    APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC, DATA_TYPE_BOOLEAN, DOCDB_CLUSTER_PARAMETERS,
    DOCDB_PARAMETER_GROUP_FAMILIES,
    IMMEDIATE_APPLY_METHOD_FOR_A_STATIC_PARAMETER_MESSAGE,
    MAX_PARAMETERS_PER_MODIFY, MAX_PARAMETER_GROUP_NAME_LEN,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_USER,
};

pub mod option_catalog;

// The records of the resource families whose own fields, quotas and projections are large
// enough to crowd this module: each lives in its own file and is re-exported here, so a
// handler still reaches every one of them as `crate::state::<name>` and the store struct
// below stays the one place that says which maps exist.
// The IAM-role associations a DB instance and a DB cluster both carry, and the rules the
// four association operations share.
pub mod associated_roles;
// The EventBridge envelope each recorded lifecycle change is also PUBLISHED as: the
// account's default-bus view of the same records `DescribeEvents` reports.
pub mod bus_events;
pub mod blue_green;
pub mod custom_engine_versions;
pub mod db_proxies;
pub mod engine_log;
pub mod events;
pub mod export_tasks;
// What an export WRITES: the objects it records in the destination bucket, and the Parquet
// encoding of one exported table. Beside them, the preconditions one is accepted against:
// the role that writes the objects and the key they are encrypted under.
pub mod export_access;
pub mod export_objects;
pub mod export_parquet;
pub use export_access::*;
pub use export_objects::*;
pub mod integrations;
// What a restore FROM Amazon S3 is accepted against: the engine pair the migration reads,
// and the Percona XtraBackup backup the named bucket and prefix have to hold.
pub mod restore_from_s3;
pub use restore_from_s3::*;
pub mod tags;
pub mod tenant_databases;

// The same split, one concern per file, for the rest of this store's record families and
// for the reference data the handlers read beside them: the parameter-group records and the
// catalog rules over them, the event subscription record, the option-group records, the
// shard-group record, the published CA certificates, the configuration analysis behind
// `DescribeDBRecommendations`, and the event category / source-type catalog. Each is
// glob-re-exported, so every one of them is still reached as `crate::state::<name>`.
mod certificates;
// The CUSTOM endpoints of a DB cluster: their record, their projection, and the two
// endpoints every cluster publishes without a create.
mod cluster_endpoints;
// The endpoints of an RDS PROXY: the one every proxy publishes without a create, and the
// projection the proxy-endpoint report answers with.
mod proxy_endpoints;
mod default_catalog;
mod event_catalog;
mod recommendations;
mod event_subscriptions;
mod option_groups;
mod parameter_groups;
// WHICH names an engine has a setting for at all — the engine's configuration namespace,
// which is what decides whether a parameter group of one of its families HAS the parameter a
// request names. Its own file beside the parameter-group records, because it is reference
// data about the ENGINE rather than about any group.
mod engine_settings;
// The DB CAPACITY RESERVATION records, their published states and the projection that
// reports one: their own concern, because a reservation holds capacity for DB instances
// rather than being one.
mod capacity_reservations;
// RESERVED DB INSTANCES: the offering catalogue this service publishes, and the reservation
// a purchase of one persists. Its own concern beside the capacity reservations above,
// because a reserved DB instance is a BILLING commitment over a DB instance class rather
// than capacity held in an Availability Zone.
mod reserved_instances;
mod shard_groups;
mod snapshots;
// The tenant databases a DB snapshot CAPTURED, beside the snapshot records themselves:
// their own concern, because they are what `DescribeDBSnapshotTenantDatabases` reports.
mod snapshot_tenant_databases;
// What a MODIFICATION of a tenant database stages — the pending values it reports until the
// clock applies them — and the re-keying applying a rename takes.
mod tenant_database_modification;
// The Amazon SNS topic an event subscription delivers to, resolved against the service that
// owns topics.
mod sns_topics;

pub use associated_roles::*;
pub use certificates::*;
pub use cluster_endpoints::*;
pub use proxy_endpoints::*;
pub use default_catalog::*;
pub use event_catalog::*;
pub use recommendations::*;
pub use event_subscriptions::*;
pub use capacity_reservations::*;
pub use reserved_instances::*;
pub use engine_settings::*;
pub use option_groups::*;
pub use parameter_groups::*;
pub use shard_groups::*;
pub use snapshot_tenant_databases::*;
pub use tenant_database_modification::*;
pub use snapshots::*;
pub use sns_topics::*;

pub use blue_green::{
    blue_green_deployment_identifier_of, blue_green_deployment_not_found_message,
    check_blue_green_deployment_identifier, green_identifier,
    mint_blue_green_deployment_identifier, switched_over_blue_identifier,
    BlueGreenDeploymentRecord, BLUE_GREEN_DEPLOYMENT_QUOTA_PER_REGION,
    BLUE_GREEN_IDENTIFIER_PREFIX, BLUE_GREEN_STATUS_AVAILABLE, BLUE_GREEN_STATUS_DELETING,
    BLUE_GREEN_STATUS_PROVISIONING, BLUE_GREEN_STATUS_SWITCHOVER_COMPLETED,
    BLUE_GREEN_STATUS_SWITCHOVER_IN_PROGRESS,
};
pub use custom_engine_versions::{
    check_custom_engine, check_custom_engine_version_name,
    custom_db_engine_version_denied_message,
    custom_db_engine_version_key, custom_db_engine_version_resource_arn,
    custom_engine_version_is_modifiable, ec2_image_not_in_account_message,
    is_ami_backed_custom_engine, reports_engine_media_type,
    CustomDbEngineVersionRecord, CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION,
    CUSTOM_ENGINE_VERSION_ENGINES, CUSTOM_ENGINE_VERSION_STATUS_AVAILABLE,
    CUSTOM_ENGINE_VERSION_STATUS_CREATING, MEDIA_TYPE_CUSTOMER_PROVIDED,
};
pub use db_proxies::{
    check_target_group_name, check_user_auth_config, db_proxy_already_exists_message,
    db_proxy_not_found_message, db_proxy_state_name, db_proxy_unsupported_state_message,
    default_connection_pool_config, max_above_idle_message, merged_connection_pool_config,
    subnets_span_several_vpcs_message, unreadable_secret_description, user_auth_config_to_info,
    DbProxyRecord, DbProxyTargetGroupRecord,
    DbProxyTargetRecord, PoolConfigRejection, ProxyTargetKey, ProxyTargetRow, RegisteredDatabase,
    DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE, DEFAULT_AUTH_SCHEME_NONE,
    DEFAULT_CONNECTION_BORROW_TIMEOUT, DEFAULT_DB_PROXY_TARGET_GROUP,
    DEFAULT_IDLE_CLIENT_TIMEOUT, DEFAULT_MAX_CONNECTIONS_PERCENT,
    DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER, DEFAULT_TARGET_GROUP_RENAME_MESSAGE,
    DB_PROXY_ACTIVE_STATE_NAME,
    DB_PROXY_QUOTA_PER_REGION, DB_PROXY_STATUS_AVAILABLE, DB_PROXY_STATUS_CREATING,
    DB_PROXY_STATUS_DELETING, DB_PROXY_STATUS_MODIFYING, DB_PROXY_TARGET_QUOTA_PER_PROXY,
    ENGINE_FAMILY_MYSQL, ENGINE_FAMILY_POSTGRESQL, ENGINE_FAMILY_SQLSERVER,
    MAX_SESSION_PINNING_FILTERS,
    MAX_USER_AUTH_CONFIGS, MISSING_DB_PROXY_TARGETS_MESSAGE,
    MISSING_DB_PROXY_TARGET_IDENTIFIER_MESSAGE, NETWORK_TYPE_IPV4, TARGET_ROLE_READ_ONLY,
    TARGET_HEALTH_REASON_AUTH_FAILURE, TARGET_HEALTH_REASON_UNREACHABLE,
    TARGET_ROLE_READ_WRITE, TARGET_STATE_AVAILABLE, TARGET_STATE_REGISTERING,
    TARGET_STATE_UNAVAILABLE, TARGET_TYPE_RDS_INSTANCE, TARGET_TYPE_TRACKED_CLUSTER,
};
pub use bus_events::{
    detail_source_type, publish_recorded_events, published_detail_type, BusEvent,
    DEFAULT_EVENT_BUS, EVENT_SOURCE, MAX_PENDING_BUS_EVENTS,
};
pub use engine_log::InstanceLog;
pub use events::{
    check_event_source_type, EventLog, EventRecord, LifecycleEvent, DEFAULT_EVENT_WINDOW_MINUTES,
    DESCRIBE_EVENTS_SOURCE_TYPES, EVENT_RETENTION_SECONDS, MAX_EVENTS_PER_REGION,
};
pub use export_tasks::{
    copy_kms_key_not_accessible_message, cross_region_export_bucket_message,
    cross_region_export_source_message,
    export_key_grant_not_created_message, export_kms_key_not_accessible_message, export_part_id,
    export_role_missing_permissions_message, export_role_not_found_message, export_source_of,
    export_task_already_exists_message, export_task_not_cancelable_message,
    export_task_not_found_message, export_task_quota_message,
    invalid_export_only_message, invalid_export_source_arn_message,
    invalid_export_source_state_message, invalid_s3_bucket_message, kms_key_not_accessible_message,
    partial_export_warning, target_kms_key_not_accessible_message, ExportTaskRecord,
    COMPLETE_PERCENT_PROGRESS, EXPORT_SOURCE_DB_CLUSTER, EXPORT_SOURCE_DB_CLUSTER_SNAPSHOT,
    EXPORT_SOURCE_DB_SNAPSHOT, EXPORT_SOURCE_TYPE_CLUSTER, EXPORT_SOURCE_TYPE_SNAPSHOT,
    EXPORT_TASK_QUOTA_PER_REGION, EXPORT_TASK_STATUS_CANCELED, EXPORT_TASK_STATUS_CANCELING,
    EXPORT_TASK_STATUS_COMPLETE, EXPORT_TASK_STATUS_FAILED, EXPORT_TASK_STATUS_IN_PROGRESS,
    EXPORT_TASK_STATUS_STARTING, EXPORT_TASK_STATUS_VALUES, INITIAL_PERCENT_PROGRESS,
    MAX_EXPORT_TASK_IDENTIFIER_LEN, TERMINAL_STATUSES,
};
pub use integrations::{
    check_integration_target_arn, check_kms_key_id, integration_not_found_message,
    integration_state_message, mint_integration_id, IntegrationRecord,
    INTEGRATION_CONFLICT_OPERATION_MESSAGE, INTEGRATION_QUOTA_PER_REGION,
    INTEGRATION_STATUS_ACTIVE, INTEGRATION_STATUS_CREATING, INTEGRATION_STATUS_DELETING,
    INTEGRATION_STATUS_MODIFYING, INVALID_TARGET_TYPE_MESSAGE,
};
pub use tags::{
    added_key_count, check_tag_capacity, check_tags_to_write, merge_tags, TagTarget,
    TagTargetMiss, TaggedKind, RESERVED_TAG_PREFIXES, TAGGABLE_ARN_RESOURCE_TYPES,
};
pub use tenant_databases::{
    is_multi_tenant_engine, mint_tenant_database_resource_id, tenant_database_key,
    tenant_database_master_user_secret_name, tenant_operation_unsupported_message,
    TenantDatabaseRecord, MULTI_TENANT_ENGINE, TENANT_DATABASE_QUOTA_PER_INSTANCE,
    TENANT_DATABASE_STATUS_AVAILABLE, TENANT_DATABASE_STATUS_CREATING,
    TENANT_DATABASE_STATUS_DELETING,
};

// The DB instance DATA plane: the engine endpoint a client connects to and speaks the
// engine's own protocol on. Its own module because it is a whole side of this service
// (two wire protocols and a SQL engine), and only the record the store holds and the
// opener a create calls are reached from here.
pub mod data_plane;
// The DOCUMENT data plane of a DB instance whose engine is Amazon DocumentDB's: an AWS SDK
// DocumentDB client signs `rds`, so such an instance is created here, and what its endpoint
// publishes is the socket this emulator answers that engine's own wire on — served by the one
// implementation of that wire this tree has, which the DocumentDB control plane's crate owns.
pub mod document_plane;
pub use document_plane::{
    engine_serves_the_document_wire, forget_cluster_documents, open_cluster_document_plane,
    open_document_plane, published_address, published_cluster_host, record_master_credential,
    DocumentPlane,
};
// The subnets a request NAMES, resolved against the service that owns them: a subnet's
// Availability Zone is Amazon EC2's fact, and this is the one place this crate asks for it.
pub mod ec2_subnets;
// Whether the role a proxy reads its `Auth` secrets with may read them, asked of IAM: what
// decides whether a proxy target can authenticate to the database behind it.
pub mod proxy_secret_access;
pub use proxy_secret_access::{unreadable_auth_secret, PROXY_SECRET_READ_ACTION};
// The account's `default` DB subnet group: the group RDS provides with the account's
// default VPC, and the default-VPC network it reports, read from the owner of subnets.
pub mod default_subnet_group;
// The VPC security groups a request NAMES, resolved against the service that owns them:
// whether a group exists is Amazon EC2's fact, asked here and nowhere else.
mod ec2_security_groups;
// The Amazon Machine Image a request NAMES, resolved against the service that owns images:
// whether the account owns that AMI is Amazon EC2's fact, asked here and nowhere else.
pub mod ec2_images;
// The published parameter defaults Amazon RDS writes as a FORMULA over the instance's
// hardware, evaluated for one DB instance class: what the engine of that instance runs with.
pub mod parameter_formula;
mod instances;
mod clusters;
// The DNS host names a DB cluster publishes as its two endpoints, and the suffix they share.
mod cluster_endpoint_names;
// The AUTOMATED BACKUP of a DB instance: the one every instance that keeps continuous
// backups has, and the record a delete that asked to keep it leaves behind.
mod instance_automated_backups;
pub use instance_automated_backups::*;
// And the REPLICATION of those automated backups to another Amazon Web Services Region: the
// destination Region's own record of one, which the start enables and the stop ends.
mod instance_backup_replication;
pub use instance_backup_replication::*;
// The AUTOMATED DB snapshots those backups take of an instance, which `DescribeDBSnapshots`
// reports under the `automated` snapshot type.
mod automated_snapshots;
pub use automated_snapshots::*;
// The Aurora Serverless v2 CAPACITY a DB cluster runs on: the ACUs it holds, the auto-pause an
// idle one is owed, and the CloudWatch series both are published as.
pub mod serverless_capacity;
pub use serverless_capacity::ServerlessCapacityPublisher;
// The Aurora Serverless v1 CAPACITY a DB cluster in the `serverless` engine mode runs on: the
// ACUs it holds, the scaling a call owes it, and the values each Aurora engine publishes.
mod serverless_v1_capacity;
pub use serverless_v1_capacity::*;
// STORAGE AUTOSCALING of a DB instance: the raise of its `AllocatedStorage` toward
// `MaxAllocatedStorage` the storage its engine is using makes due.
pub mod storage_autoscaling;
pub use storage_autoscaling::StorageAutoscalingClock;
// The clock that materialises a lifecycle change this service answered BEFORE performing it,
// and publishes the event that change records.
pub mod lifecycle_event_clock;
pub use lifecycle_event_clock::{drive_due_lifecycle_events, LifecycleEventClock};
// The weekly maintenance window both resource kinds keep: the form it is STORED in, and
// when its next occurrence begins.
pub mod maintenance_window;
// What a modification the caller did not ask to apply immediately stages on a DB instance,
// and the window those settings are applied in.
mod instance_modification;
pub use instance_modification::*;
// Promoting a secondary DB cluster of a global database to be its primary: the one rule the
// failover and the switchover of a global database both perform.
mod global_cluster_promotion;
pub use global_cluster_promotion::*;
pub use maintenance_window::{stored_maintenance_window, window_is_open};
// The Data API HTTP endpoint of a DB cluster: the one write `EnableHttpEndpoint` and
// `DisableHttpEndpoint` share, and the preconditions both hold a request to.
mod data_api_endpoint;
// The audit policy state of a DB instance's database activity stream: the write
// `ModifyActivityStream` performs, and the preconditions it holds a request to.
mod activity_stream;
pub use activity_stream::*;
// The data a DB cluster holds through the Data API, which a snapshot of that cluster carries
// and a restore of that snapshot installs: the hand-off to the service that owns those
// databases.
mod data_api_cluster_data;
// And the tables such a copy contributes to a copy of the cluster's DATA: what an export of
// that cluster, or of a snapshot of it, reads beside the rows its wire endpoint wrote.
pub mod data_api_rows;
// The address a resource publishes when its data plane is an AWS API this emulator serves
// at its own endpoint (Amazon Neptune's graph API) rather than a native engine wire.
mod api_plane_address;
// What a graph DB cluster this front door provisioned runs, recorded where that cluster's
// own data plane reads it.
mod graph_cluster_directory;
pub use graph_cluster_directory::*;
// The Secrets Manager secret RDS manages for a resource's master user password: the
// record a resource stores for it, and the minting a request that turns
// `ManageMasterUserPassword` on depends on.
mod managed_master_secret;
// How that same secret is rotated when the rotation is asked of SECRETS MANAGER rather than
// of this service: the record this service writes for each secret it manages, naming the
// request that rotates that resource's master user password.
mod managed_secret_rotation;
// The KMS key an encrypted resource's storage is reported under: the one the request named,
// or the account's default key for this service where it named none.
mod storage_encryption_key;
// The identity a call RDS makes of its OWN presents to the service serving it: the one
// place this crate acts as its own service principal rather than as the caller.
mod service_principal_call;
// WHO OWNS that secret: it goes with the resource it was minted for, and a resource that
// merely inherits another's master credential owns none.
mod master_user_credentials;
mod published_values;
mod minted_values;
mod engine_defaults;
mod ca_certificates;
// The refusal wording and the shared request checks this service's handlers classify a
// refused call by: the sentences AWS words a refusal with, and the rules more than one
// operation holds a request to.
mod refusal_wording;
pub use refusal_wording::*;
// How a member that NAMES an existing DB cluster is read: the identifier the reference
// carries, and the refusal for a reference no DB cluster identifier could be.
mod cluster_lookup;
mod pagination;
pub use pagination::*;
// The GLOBAL DATABASE a DB cluster joins: its record, what it publishes, and the sentences
// the faults about it are worded as.
mod global_clusters;
pub use global_clusters::*;
pub use cluster_lookup::*;
pub use ca_certificates::*;
pub use engine_defaults::*;
pub use minted_values::*;
pub use published_values::*;
pub use cluster_endpoint_names::*;
pub use clusters::*;
pub use api_plane_address::*;
pub use data_api_endpoint::*;
pub use data_api_cluster_data::*;
pub use instances::*;
pub use managed_master_secret::*;
pub use managed_secret_rotation::*;
pub use storage_encryption_key::*;
pub use service_principal_call::*;
pub use master_user_credentials::*;
pub use data_plane::{
    captured_data_bytes, capture_cluster_data, capture_cluster_storage, capture_endpoint_storage,
    capture_instance_storage,
    cluster_endpoint::cluster_engine_wire,
    engine_wire, open_engine_endpoint,
    proxy_endpoint::{
        open_proxy_endpoint, proxy_engine_port, proxy_engine_wire, wire_engine_port,
        DbProxyEndpoint, ProxyEndpointPlan, PROXY_REQUIRES_TLS_MESSAGE,
    },
    rewind_cluster_storage, wire_session::SessionTarget,
    DbEngineEndpoint, EndpointRole, EngineEndpointPlan, EngineWire, ServedEndpoint,
};
pub use ec2_subnets::{invalid_subnets_message, subnet_zones, SubnetZones};
pub use default_subnet_group::{default_vpc_network, DefaultSubnetGroupNetwork};
pub use ec2_security_groups::{
    default_vpc_security_group, invalid_security_group_message, unresolved_vpc_security_groups,
};
pub use ec2_images::{image_ownership, ImageOwnership};

/// Per-service state for com-amazonaws-rds-amazonrdsv19. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRdsAmazonrdsv19State {
    /// DB instances by `(region, DBInstanceIdentifier)` — the identifier always stored
    /// lowercased, as the model documents. Ordered, so `DescribeDBInstances` pages
    /// deterministically within a region.
    pub db_instances: BTreeMap<RegionalKey, DbInstanceRecord>,
    /// DB snapshots by `(region, DBSnapshotIdentifier)`. Ordered, for the same reason.
    pub db_snapshots: BTreeMap<RegionalKey, DbSnapshotRecord>,
    /// The AUTOMATED snapshot each DB instance that keeps automated backups has taken, by
    /// `(region, DbiResourceId)` — the source instance's immutable id — holding the
    /// `DBSnapshotIdentifier` of its entry in [`Self::db_snapshots`].
    ///
    /// This is what makes the automated snapshot a change the clock owes ONCE rather than
    /// one every read could mint again: [`Self::settle_automated_db_snapshots`] is its only
    /// writer, it inserts for an instance that has none and removes the record and the
    /// snapshot with it when the instance stops keeping automated backups. One instance has
    /// one, so the map is bounded by [`DB_INSTANCE_QUOTA_PER_REGION`] per region and by
    /// [`MAX_REGIONS`] regions, like every other record here.
    pub automated_db_snapshots: BTreeMap<RegionalKey, String>,
    /// The RETAINED automated backups of DB instances that have been DELETED, by
    /// `(region, DbiResourceId)` — the id of the source instance, which "can't be changed
    /// and ... is unique to an Amazon Web Services Region", so one deleted instance leaves
    /// one record here. Ordered, so `DescribeDBInstanceAutomatedBackups` pages
    /// deterministically within a region.
    ///
    /// `DeleteDBInstance` with `DeleteAutomatedBackups` turned off inserts here — bounded
    /// by [`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`]
    /// regions — and `DeleteDBInstanceAutomatedBackup` is the reclamation path. The
    /// `active` backups of instances this store still holds are NOT here: they are
    /// projected from the instance records themselves
    /// ([`DbInstanceRecord::automated_backup_to_wire`]), so an instance and its automated
    /// backup cannot disagree.
    pub db_instance_automated_backups: BTreeMap<RegionalKey, RetainedInstanceBackupRecord>,
    /// The automated backups REPLICATED INTO a Region from a DB instance of another one, by
    /// `(destination region, DbiResourceId)` — the id of the source instance, so one source
    /// instance has one record per destination Region. Ordered, so
    /// `DescribeDBInstanceAutomatedBackups` pages deterministically within a region.
    ///
    /// `StartDBInstanceAutomatedBackupsReplication` inserts here — bounded by
    /// [`RETAINED_INSTANCE_BACKUP_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`]
    /// regions — and `StopDBInstanceAutomatedBackupsReplication` leaves the record behind as
    /// the `retained` backups the stopped replication had taken. The SOURCE instance's own
    /// half of the relationship is not stored: its automated backup reports the replications
    /// read off these records ([`Self::automated_backups_replications_of`]), so the two
    /// cannot disagree.
    pub db_instance_automated_backup_replications:
        BTreeMap<RegionalKey, ReplicatedInstanceBackupRecord>,
    /// DB CLUSTER snapshots by `(region, DBClusterSnapshotIdentifier)` — the identifier
    /// always stored lowercased, as `DBClusterSnapshotIdentifier` documents ("This value
    /// is stored as a lowercase string"). Ordered, so `DescribeDBClusterSnapshots` pages
    /// deterministically within a region.
    pub db_cluster_snapshots: BTreeMap<RegionalKey, DbClusterSnapshotRecord>,
    /// DB clusters by `(region, DBClusterIdentifier)` — the identifier always stored
    /// lowercased, as the model documents. Ordered, so `DescribeDBClusters` pages
    /// deterministically within a region.
    ///
    /// `CreateDBCluster` inserts here — bounded by [`DB_CLUSTER_QUOTA_PER_REGION`] per
    /// region and by [`MAX_REGIONS`] regions — and this is also what a request that
    /// REFERENCES a cluster (`CreateDBInstance$DBClusterIdentifier`,
    /// `CreateGlobalCluster$SourceDBClusterIdentifier`) is resolved against, so a
    /// reference can only name a cluster some create really stored.
    pub db_clusters: BTreeMap<RegionalKey, DbClusterRecord>,
    /// Cluster parameter groups by `(region, DBClusterParameterGroupName)` — the name
    /// always stored lowercased, as the model documents. Ordered, so
    /// `DescribeDBClusterParameterGroups` pages deterministically within a region.
    pub db_cluster_parameter_groups: BTreeMap<RegionalKey, DbClusterParameterGroupRecord>,
    /// DB (instance) parameter groups by `(region, DBParameterGroupName)` — the name always
    /// stored lowercased, as `CreateDBParameterGroupMessage$DBParameterGroupName` documents
    /// ("This value is stored as a lowercase string"). Ordered, so
    /// `DescribeDBParameterGroups` pages deterministically within a region.
    ///
    /// `CreateDBParameterGroup` inserts here — bounded by
    /// [`DB_PARAMETER_GROUP_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`] regions —
    /// and `DescribeDBParameterGroups` reports what is here. Separate from
    /// [`Self::db_cluster_parameter_groups`] because AWS keeps the two namespaces apart: a
    /// cluster parameter group and a DB parameter group of the same name are different
    /// resources with different ARNs.
    pub db_parameter_groups: BTreeMap<RegionalKey, DbParameterGroupRecord>,
    /// Event notification subscriptions by `(region, SubscriptionName)`. Ordered, so
    /// `DescribeEventSubscriptions` pages deterministically within a region.
    ///
    /// `CreateEventSubscription` inserts here — bounded by
    /// [`EVENT_SUBSCRIPTION_QUOTA_PER_REGION`] per region, and by [`MAX_REGIONS`] regions
    /// — and `DescribeEventSubscriptions` reports what is here.
    pub event_subscriptions: BTreeMap<RegionalKey, EventSubscriptionRecord>,
    /// Aurora global database clusters by `GlobalClusterIdentifier`.
    ///
    /// Keyed by the identifier ALONE, not by `(region, identifier)` like the maps above: a
    /// global cluster is an ACCOUNT-scoped resource whose ARN carries no region
    /// (`arn:aws:rds::<account>:global-cluster:<id>`) and spans the regions of its member
    /// clusters, so `DescribeGlobalClusters` reports it whatever region the request is
    /// signed for. `CreateGlobalCluster` inserts here, bounded by
    /// [`GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT`] — the account scope the map itself has.
    pub global_clusters: BTreeMap<String, GlobalClusterRecord>,
    /// Option groups by `(region, OptionGroupName)`. Ordered, so
    /// `DescribeOptionGroups` pages deterministically within a region.
    pub option_groups: BTreeMap<RegionalKey, OptionGroupRecord>,
    /// DB subnet groups by `(region, DBSubnetGroupName)` — the name always stored
    /// lowercased, as `CreateDBSubnetGroup` documents. These are what a request that
    /// NAMES a DB subnet group (`CreateDBInstance$DBSubnetGroupName`) is resolved
    /// against, so a name no group was created under is a not-found rather than a
    /// silently accepted reference.
    pub db_subnet_groups: BTreeMap<RegionalKey, DbSubnetGroupRecord>,
    /// What the owner of subnets answered about the DEFAULT VPC of a region, by region:
    /// the network the account's `default` DB subnet group covers
    /// ([`default_subnet_group`]). No request creates that group, so this is not a group
    /// record — it is the EC2 facts the derived group reports, kept so every read of it
    /// reports one answer. At most one entry per region, which [`MAX_REGIONS`] bounds.
    pub default_subnet_group_networks: BTreeMap<String, DefaultSubnetGroupNetwork>,
    /// DB security groups by `(region, DBSecurityGroupName)` — the name always stored
    /// lowercased, as `CreateDBSecurityGroup$DBSecurityGroupName` documents ("This value
    /// is stored as a lowercase string"). Ordered, so `DescribeDBSecurityGroups` pages
    /// deterministically within a region.
    ///
    /// `CreateDBSecurityGroup` inserts here — bounded by
    /// [`DB_SECURITY_GROUP_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`] regions,
    /// each group carrying at most [`MAX_AUTHORIZATIONS_PER_DB_SECURITY_GROUP`] ingress
    /// rules — and `DescribeDBSecurityGroups` reports what is here.
    pub db_security_groups: BTreeMap<RegionalKey, DbSecurityGroupRecord>,
    /// Blue/green deployments by `(region, BlueGreenDeploymentIdentifier)` — the
    /// identifier being the one this service MINTED, so a caller cannot choose a key.
    /// Ordered, so `DescribeBlueGreenDeployments` pages deterministically within a region.
    ///
    /// `CreateBlueGreenDeployment` inserts here, bounded by
    /// [`BLUE_GREEN_DEPLOYMENT_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`]
    /// regions.
    pub blue_green_deployments: BTreeMap<RegionalKey, BlueGreenDeploymentRecord>,
    /// Zero-ETL integrations by `(region, IntegrationName)`. Ordered, so
    /// `DescribeIntegrations` pages deterministically within a region.
    ///
    /// `CreateIntegration` inserts here, bounded by [`INTEGRATION_QUOTA_PER_REGION`] per
    /// region and by [`MAX_REGIONS`] regions.
    pub integrations: BTreeMap<RegionalKey, IntegrationRecord>,
    /// Tenant databases by `(region, <DBInstanceIdentifier>/<TenantDBName>)` — see
    /// [`tenant_database_key`] for why the second half pairs the two. Ordered, so one
    /// instance's tenant databases are an ordered range and
    /// `DescribeTenantDatabases` pages deterministically within it.
    ///
    /// `CreateTenantDatabase` inserts here, bounded by
    /// [`TENANT_DATABASE_QUOTA_PER_INSTANCE`] per DB instance, which the per-region
    /// instance quota in turn bounds.
    pub tenant_databases: BTreeMap<RegionalKey, TenantDatabaseRecord>,
    /// RDS proxies by `(region, DBProxyName)`. Ordered, so `DescribeDBProxies` pages
    /// deterministically within a region.
    ///
    /// `CreateDBProxy` inserts here — bounded by [`DB_PROXY_QUOTA_PER_REGION`] per region
    /// and by [`MAX_REGIONS`] regions — and each proxy carries at most
    /// [`DB_PROXY_TARGET_QUOTA_PER_PROXY`] target registrations, which
    /// `RegisterDBProxyTargets` adds and `DeregisterDBProxyTargets` removes.
    pub db_proxies: BTreeMap<RegionalKey, DbProxyRecord>,
    /// Snapshot and cluster export tasks by `(region, ExportTaskIdentifier)`. Ordered, so
    /// `DescribeExportTasks` pages deterministically within a region.
    ///
    /// `StartExportTask` — the operation that inserts here — is still scaffolded in this
    /// crate (its own stub issue), so nothing creates a task yet; `DescribeExportTasks`
    /// reports what is here and `CancelExportTask` moves a task that is here, both against
    /// the stored record rather than a canned answer.
    pub export_tasks: BTreeMap<RegionalKey, ExportTaskRecord>,
    /// The account's CA certificate OVERRIDE, by region: the `CertificateIdentifier`
    /// `ModifyCertificates` pinned as the certificate new DB instances are created with,
    /// for the regions where one is set.
    ///
    /// Keyed by region because that is the scope `ModifyCertificates` documents ("RDS has
    /// already moved to a new default CA certificate for the specified Amazon Web Services
    /// Region"), so an override set in one region leaves another region's default alone.
    /// An absent entry is the published system default ([`DEFAULT_CA_CERTIFICATE_IDENTIFIER`]),
    /// which is why this is an override map rather than a per-region copy of the default:
    /// at most ONE entry per region, and a region that holds no record at all cannot get
    /// one without passing [`ComAmazonawsRdsAmazonrdsv19State::has_room_for_region`], so
    /// the map is bounded by [`MAX_REGIONS`] like every other store here.
    pub certificate_overrides: BTreeMap<String, String>,
    /// Aurora Limitless Database DB shard groups by `(region, db_shard_group_key)` — the
    /// identifier folded by [`db_shard_group_key`], because
    /// `DescribeDBShardGroupsMessage$DBShardGroupIdentifier` "isn't case-sensitive", while
    /// the record keeps the identifier in the case the create named it. Ordered, so
    /// `DescribeDBShardGroups` pages deterministically within a region.
    ///
    /// `CreateDBShardGroup` inserts here — bounded by
    /// [`DB_SHARD_GROUP_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`] regions — and
    /// `DescribeDBShardGroups` reports what is here.
    pub db_shard_groups: BTreeMap<RegionalKey, DbShardGroupRecord>,
    /// DB capacity reservations by `(region, DBCapacityReservationIdentifier)` — the
    /// `dbcr-` identifier this service MINTED, so a caller cannot choose a key; the NAME it
    /// chose is resolved against the same region's range
    /// ([`Self::resolve_db_capacity_reservation`]). Ordered, so
    /// `DescribeDBCapacityReservations` pages deterministically within a region.
    ///
    /// `CreateDBCapacityReservation` inserts here, bounded by
    /// [`DB_CAPACITY_RESERVATION_QUOTA_PER_REGION`] records per region — a count over every
    /// record the region holds, whatever state each is in, so a reservation whose capacity
    /// has been released cannot be recycled into unbounded growth — and by [`MAX_REGIONS`]
    /// regions. A second, narrower gate caps what those records may RESERVE: a reservation
    /// holds capacity for `CapacityPerAvailabilityZone` DB instances in each of its zones,
    /// and a create is refused once the reservations of a region that still hold capacity
    /// would hold it for more than [`DB_INSTANCE_QUOTA_PER_REGION`] of them
    /// ([`Self::reserved_db_instances_in`]).
    pub db_capacity_reservations: BTreeMap<RegionalKey, DbCapacityReservationRecord>,
    /// The reservations `PurchaseReservedDBInstancesOffering` purchased, by
    /// `(region, ReservedDBInstanceId)` — the caller's own identifier for the reservation,
    /// or the one this service minted for a purchase that named none. Ordered, so
    /// `DescribeReservedDBInstances` pages deterministically within a region.
    ///
    /// Bounded by [`RESERVED_DB_INSTANCE_RECORD_QUOTA_PER_REGION`] records per region and by
    /// [`MAX_REGIONS`] regions, with a second, narrower gate on what those records may
    /// RESERVE: a purchase is refused once the reservations of a region that still hold
    /// capacity would reserve more than [`RESERVED_DB_INSTANCE_QUOTA_PER_REGION`] DB
    /// instances ([`Self::reserved_db_instance_capacity_in`]), the account allowance
    /// `DescribeAccountAttributes` publishes. The OFFERINGS a purchase resolves against are
    /// not here: they are published by the service rather than created by a caller, so they
    /// are reference data ([`published_reserved_offerings`]).
    pub reserved_db_instances: BTreeMap<RegionalKey, ReservedDbInstanceRecord>,
    /// Custom DB engine versions by `(region, custom_db_engine_version_key)` — the CEVs
    /// the account itself minted, as opposed to the engine versions this service
    /// publishes as reference data. Ordered, so `DescribeDBEngineVersions` reports and
    /// pages them deterministically within a region.
    ///
    /// `CreateCustomDBEngineVersion` inserts here, bounded by
    /// [`CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION`] per region and by [`MAX_REGIONS`]
    /// regions.
    pub custom_db_engine_versions: BTreeMap<RegionalKey, CustomDbEngineVersionRecord>,
    /// The lifecycle events this service has recorded, by region — what `DescribeEvents`
    /// reports.
    ///
    /// Keyed by region ALONE, because an event is not a resource a caller names: it is a
    /// record of a change this service made in that region, and the handlers that make
    /// those changes append here through [`Self::record_event`]. Bounded by
    /// [`MAX_EVENTS_PER_REGION`] records per region — the log is a ring — and by
    /// [`MAX_REGIONS`] regions, which [`Self::has_room_for_region`] counts this map into;
    /// see [`events`] for the published retention window a record also ages out of.
    pub events: BTreeMap<String, EventLog>,
    /// The recorded changes that have not been published onto the account's default event
    /// bus yet, oldest first.
    ///
    /// Every change [`Self::record_event`] writes is queued here as well, and the ONE seam
    /// every request of this front door passes through drains it after the response has been
    /// rendered (`crate::service::query_error_envelope`), so a change is published from
    /// outside the handler that made it and with this store released — see [`bus_events`].
    /// Bounded by [`MAX_PENDING_BUS_EVENTS`], so an emulator with nothing draining it holds
    /// a queue rather than growing one.
    pub pending_bus_events: Vec<BusEvent>,
    /// Monotonic counter behind every server-assigned id this service mints, so
    /// resource ids and endpoint hosts are unique and reproducible.
    pub mint_counter: u64,
    /// The recommendations this service's configuration analysis has raised, by
    /// `(region, recommendation_key)` — what `DescribeDBRecommendations` reports.
    ///
    /// Keyed by the resource and the check rather than by the minted recommendation id, so
    /// one check raises at most one record per resource; the record carries the id a caller
    /// names it by. [`Self::settle_db_recommendations`] is the only writer, and it raises a
    /// record only for a resource this store holds, so the map is bounded by the
    /// per-region instance and cluster quotas times [`CONFIGURATION_CHECKS`]`.len()`, over
    /// at most [`MAX_REGIONS`] regions.
    pub db_recommendations: BTreeMap<RegionalKey, DbRecommendationRecord>,
    /// The live engine endpoint of each DB instance that has one, by the same
    /// `(region, DBInstanceIdentifier)` key as [`Self::db_instances`].
    ///
    /// This is where the LISTENER lives, so it is also what holds its port open: an
    /// entry removed from here closes the port and severs its sessions (see
    /// [`data_plane`]). Bounded by the same per-region instance quota, because an entry
    /// exists only for an instance this store holds — `CreateDBInstance` inserts one for
    /// every engine whose wire this emulator speaks, and
    /// [`Self::settle_due_transitions`] drops it with the instance.
    pub db_engine_endpoints: BTreeMap<RegionalKey, DbEngineEndpoint>,
    /// The DOCUMENT-wire socket of each DB instance that has one, by `(region,
    /// DBInstanceIdentifier)` — the Amazon DocumentDB instances this door created
    /// ([`document_plane`]). Kept beside the instance records for the reason the engine
    /// endpoints above are: a record is a cloneable value and a live socket is not, and the
    /// handle held here is what keeps the port open for exactly as long as the instance.
    pub document_planes: BTreeMap<RegionalKey, DocumentPlane>,
    /// The live engine endpoint of each DB CLUSTER that has one, by the same
    /// `(region, DBClusterIdentifier)` key as [`Self::db_clusters`].
    ///
    /// The cluster's counterpart of [`Self::db_engine_endpoints`], and the same contract:
    /// this is where the LISTENER lives, so an entry removed from here closes the port and
    /// severs its sessions. Bounded by the same per-region cluster quota, because an entry
    /// exists only for a cluster this store holds — `CreateDBCluster` inserts one for every
    /// cluster whose engine speaks a wire this emulator serves
    /// ([`data_plane::cluster_endpoint`]), and [`Self::settle_due_transitions`] drops it
    /// with the cluster.
    pub db_cluster_engine_endpoints: BTreeMap<RegionalKey, DbEngineEndpoint>,
    /// The DOCUMENT-wire socket of each DB CLUSTER that has one, by the same
    /// `(region, DBClusterIdentifier)` key as [`Self::db_clusters`] — the Amazon DocumentDB
    /// clusters this door created ([`document_plane`]).
    ///
    /// The cluster's counterpart of [`Self::document_planes`], and the same contract: the
    /// handle held here is what keeps the port open for exactly as long as the cluster, and an
    /// entry removed from here closes it. Bounded by the per-region cluster quota, because an
    /// entry exists only for a cluster this store holds.
    pub cluster_document_planes: BTreeMap<RegionalKey, DocumentPlane>,
    /// The CUSTOM endpoints of DB clusters by `(region, db_cluster_endpoint_key)` — the
    /// identifier folded as `DBClusterEndpointIdentifier` documents ("this parameter is
    /// stored as a lowercase string"). Ordered, so `DescribeDBClusterEndpoints` pages
    /// deterministically within a region.
    ///
    /// `CreateDBClusterEndpoint` inserts here, bounded by
    /// [`DB_CLUSTER_ENDPOINT_QUOTA_PER_CLUSTER`] endpoints per cluster — an entry exists
    /// only for a cluster this store holds, so the per-region cluster quota and
    /// [`MAX_REGIONS`] bound the map in total — and
    /// [`Self::settle_due_transitions`] drops an endpoint with the cluster it belongs to.
    pub db_cluster_endpoints: BTreeMap<RegionalKey, DbClusterEndpointRecord>,
    /// The live LISTENER of each custom DB-cluster endpoint that has one, by the same
    /// `(region, db_cluster_endpoint_key)` key as [`Self::db_cluster_endpoints`].
    ///
    /// The custom endpoint's counterpart of [`Self::db_cluster_engine_endpoints`], and the
    /// same contract: this is where the listener lives, so an entry removed from here closes
    /// the port and severs its sessions. An entry exists only for an endpoint this store
    /// holds whose cluster speaks a wire this emulator serves
    /// ([`data_plane::cluster_endpoint`]), so the per-cluster endpoint allowance
    /// bounds it, and [`Self::settle_due_transitions`] drops it with the endpoint — and with
    /// the cluster the endpoint stands in front of.
    pub db_cluster_endpoint_planes: BTreeMap<RegionalKey, DbEngineEndpoint>,
    /// The live endpoint of each DB PROXY that has one, by the same
    /// `(region, DBProxyName)` key as [`Self::db_proxies`].
    ///
    /// The proxy's counterpart of [`Self::db_engine_endpoints`], and the same contract: this
    /// is where the LISTENER lives, so an entry removed from here closes the port and severs
    /// its sessions (see [`data_plane::proxy_endpoint`]). Bounded by the same per-region
    /// proxy quota, because an entry exists only for a proxy this store holds —
    /// `CreateDBProxy` inserts one for every proxy whose `EngineFamily` names a wire this
    /// emulator speaks, and [`Self::settle_due_transitions`] drops it with the proxy.
    pub db_proxy_endpoints: BTreeMap<RegionalKey, DbProxyEndpoint>,
    /// The endpoints a caller ADDED to a DB proxy, by `(region, DBProxyEndpointName)` — the
    /// name "must be unique for all DB proxy endpoints owned by your Amazon Web Services
    /// account in the specified Amazon Web Services Region"
    /// (`DBProxyEndpointAlreadyExistsFault`), which is exactly what this key enforces.
    /// Ordered, so `DescribeDBProxyEndpoints` pages deterministically within a region.
    ///
    /// `CreateDBProxyEndpoint` inserts here, bounded by
    /// [`DB_PROXY_ENDPOINT_QUOTA_PER_PROXY`] endpoints per proxy — an entry exists only for a
    /// proxy this store holds, so the per-region proxy quota and [`MAX_REGIONS`] bound the map
    /// in total — and [`Self::settle_due_transitions`] drops an endpoint with the proxy it
    /// belongs to.
    pub db_proxy_custom_endpoints: BTreeMap<RegionalKey, DbProxyCustomEndpointRecord>,
    /// The live LISTENER of each such endpoint that has one, by the same
    /// `(region, DBProxyEndpointName)` key as [`Self::db_proxy_custom_endpoints`].
    ///
    /// The added endpoint's counterpart of [`Self::db_proxy_endpoints`], and the same
    /// contract: this is where the listener lives, so an entry removed from here closes the
    /// port and severs its sessions (see [`data_plane::proxy_endpoint`]). An entry exists only
    /// for an endpoint this store holds whose proxy announced a wire this emulator speaks, so
    /// the per-proxy endpoint allowance bounds it, and [`Self::settle_due_transitions`] drops
    /// it with the endpoint — and with the proxy the endpoint fronts.
    pub db_proxy_custom_endpoint_planes: BTreeMap<RegionalKey, DbProxyEndpoint>,
    /// Where the Aurora Serverless v2 CAPACITY series of each such cluster has got to, how
    /// long each has been idle, and the one in-process clock that advances both
    /// ([`serverless_capacity`]).
    ///
    /// One entry per Serverless v2 cluster this store holds, so the per-region cluster quota
    /// and [`MAX_REGIONS`] bound it; an entry is dropped by the publisher's own pass once the
    /// cluster it belongs to is gone.
    pub serverless_capacity: ServerlessCapacityPublisher,
    /// How long each DB instance that keeps STORAGE AUTOSCALING on has been low on storage,
    /// and when its storage was last raised ([`storage_autoscaling`]), by
    /// `(region, DbiResourceId)` — the instance's immutable id, so a rename cannot orphan a
    /// clock or give an instance a second one.
    ///
    /// One entry per such instance, so the per-region DB instance quota and [`MAX_REGIONS`]
    /// bound it; [`Self::settle_storage_autoscaling`] drops the entries of the instances this
    /// store no longer holds.
    pub storage_autoscaling: BTreeMap<RegionalKey, StorageAutoscalingClock>,
    /// The one in-process clock per Region that materialises the lifecycle changes this
    /// service answered BEFORE performing them, and publishes the events they record
    /// ([`lifecycle_event_clock`]).
    ///
    /// At most one entry per Region, so [`MAX_REGIONS`] bounds it; an entry is taken by the
    /// clock it belongs to as soon as that Region owes no such change, and the task it holds
    /// is aborted when a later clock replaces it.
    pub lifecycle_event_clocks: BTreeMap<String, LifecycleEventClock>,
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The next value of the server-side id counter.
    pub fn next_mint(&mut self) -> u64 {
        self.mint_counter += 1;
        self.mint_counter
    }

    /// The `CertificateIdentifier` the account's override names in `region`, or `None`
    /// where no `ModifyCertificates` call set one.
    pub fn certificate_override(&self, region: &str) -> Option<&str> {
        self.certificate_overrides.get(region).map(String::as_str)
    }

    /// The CA certificate a DB instance created in `region` without a
    /// `CACertificateIdentifier` is issued under, which is also what
    /// `DescribeCertificates` reports as `DefaultCertificateForNewLaunches`: "either the
    /// root CA override set on your Amazon Web Services account or the system default CA
    /// for the Region if no override exists".
    pub fn default_ca_certificate(&self, region: &str) -> &str {
        self.certificate_override(region)
            .unwrap_or(DEFAULT_CA_CERTIFICATE_IDENTIFIER)
    }

    /// Materialise the status transitions the clock has already made due IN `region`.
    ///
    /// Call it at the top of every handler, reporting ones included: this writes only
    /// changes that were owed at this instant even if nobody had called, so a read
    /// cannot move a status that was not already due. It walks one region's ordered
    /// range — bounded by that region's quota — never the whole store, so the work one
    /// request does under the state lock does not grow with what other callers stored.
    pub fn settle_due_transitions(&mut self, region: &str) {
        let now = Instant::now();
        // A record whose pending change is its REMOVAL leaves the store here: the
        // keys are collected first because the range borrows the map mutably.
        let mut gone: Vec<RegionalKey> = Vec::new();
        // A rename the clock made due in the maintenance window moves the record to a new
        // key, so those keys are collected here too and the records moved below.
        let mut renamed: Vec<(RegionalKey, RegionalKey)> = Vec::new();
        // The lifecycle changes these settles COMPLETE — a restart, a stop, a start — each
        // with the instance it happened to and the instant the clock made it due. Collected
        // here because the region's event log is another field of this store, and recorded
        // below once the range no longer borrows the map.
        let mut completed: Vec<(String, String, LifecycleEvent, Instant)> = Vec::new();
        for (key, instance) in self
            .db_instances
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            // Read BEFORE the settle, which is what performs the change: afterwards the
            // instance reports the status it reached and owes nothing.
            let completing = instance.completing_lifecycle_event(now);
            if instance.settle(now) {
                gone.push(key.clone());
                continue;
            }
            if let Some((event, due_at)) = completing {
                completed.push((instance.identifier.clone(), instance.arn(), event, due_at));
            }
            if instance.identifier != key.1 {
                renamed.push((
                    key.clone(),
                    (region.to_string(), instance.identifier.clone()),
                ));
            }
        }
        // Each change a settle completed is a record of the region's event log — and so a
        // publication onto the account's default bus — dated when the clock made it DUE
        // rather than when this settle got round to it, because that is when it happened.
        for (identifier, arn, event, due_at) in completed {
            self.record_event_at(region, event, &identifier, arn, wire_timestamp_at(due_at, now));
        }
        for key in gone.drain(..) {
            self.db_instances.remove(&key);
            // The instance is gone from the service, so its engine endpoint goes with
            // it: dropping the record closes the port and severs every session on it.
            self.db_engine_endpoints.remove(&key);
            // And its DOCUMENT-wire socket, where it had one, for the same reason
            // ([`document_plane`]).
            self.document_planes.remove(&key);
            // And so do its tenant databases: a tenant database is a pluggable database
            // INSIDE a DB instance, so none of them outlives the instance that held it.
            self.drop_tenant_databases_of(&key);
        }
        for (from, to) in renamed {
            // An identifier another instance took while the rename was pending is not one
            // this one can be renamed to: the instance keeps the identifier it has rather
            // than displacing a record under a key it would share.
            if self.db_instances.contains_key(&to) {
                if let Some(record) = self.db_instances.get_mut(&from) {
                    record.identifier = from.1.clone();
                }
                continue;
            }
            if let Some(record) = self.db_instances.remove(&from) {
                self.db_instances.insert(to.clone(), record);
                // The instance's engine endpoint moves with it: the listener a client is
                // connected to stays the endpoint this instance publishes, whatever it is
                // now called.
                self.move_engine_endpoint(&from, &to);
                // The same for its document-wire socket: a rename moves the key the handle is
                // held under, so the port stays the endpoint this instance publishes.
                if let Some(plane) = self.document_planes.remove(&from) {
                    self.document_planes.insert(to.clone(), plane);
                }
                // Its tenant databases move with it too: each of them is addressed by the
                // instance that holds it, which is now called something else.
                self.move_tenant_databases_of(&from, &to);
            }
        }
        for (key, snapshot) in self
            .db_snapshots
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if snapshot.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_snapshots.remove(&key);
        }
        // The automated snapshots the instances of this region owe, now that their statuses
        // are the ones the clock has made due: an instance whose automated backup has
        // reached `active` has its first automated snapshot
        // ([`Self::settle_automated_db_snapshots`] says why that is owed at this instant).
        self.settle_automated_db_snapshots(region);
        // And the storage autoscaling they owe: an instance that has been low on storage for
        // the window has its `AllocatedStorage` raised, which is a change the CLOCK made due
        // ([`Self::settle_storage_autoscaling`]).
        self.settle_storage_autoscaling(region);
        let mut gone: Vec<RegionalKey> = Vec::new();
        for (key, snapshot) in self
            .db_cluster_snapshots
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if snapshot.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_cluster_snapshots.remove(&key);
        }
        for (key, cluster) in self
            .db_clusters
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if cluster.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_clusters.remove(&key);
            // The cluster is gone from the service, so its engine endpoint goes with it:
            // dropping the record closes the port and severs every session on it.
            self.db_cluster_engine_endpoints.remove(&key);
            // And its own DOCUMENT-wire socket, where it had one, for the same reason
            // ([`document_plane`]).
            self.cluster_document_planes.remove(&key);
            // And so do the custom endpoints that stood in front of it: a custom endpoint
            // "associated with an Amazon Aurora DB cluster" has nothing to point at once
            // that cluster is gone, which is why it is removed here rather than left
            // naming it — and the listener each of them published goes with it, which
            // closes its port and severs its sessions.
            let mut endpoints_gone: Vec<RegionalKey> = Vec::new();
            self.db_cluster_endpoints.retain(|endpoint_key, record| {
                let belongs = endpoint_key.0 == key.0 && record.db_cluster_identifier == key.1;
                if belongs {
                    endpoints_gone.push(endpoint_key.clone());
                }
                !belongs
            });
            for endpoint_key in endpoints_gone {
                self.db_cluster_endpoint_planes.remove(&endpoint_key);
            }
        }
        for (key, endpoint) in self
            .db_cluster_endpoints
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if endpoint.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_cluster_endpoints.remove(&key);
            // The endpoint is gone from the service, so the listener it published goes with
            // it: dropping the record closes the port and severs every session on it, after
            // which nothing answers at the address that endpoint published.
            self.db_cluster_endpoint_planes.remove(&key);
        }
        for (key, shard_group) in self
            .db_shard_groups
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if shard_group.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_shard_groups.remove(&key);
        }
        // A DB capacity reservation settles against two clocks: the state the create owed
        // it, and the expiry its own `EndDate` makes due. Neither removes the record, so
        // this walk collects no keys.
        let wall_clock = now_as_wire_timestamp();
        for (key, reservation) in self
            .db_capacity_reservations
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            reservation.settle(now, wall_clock);
        }
        for (key, deployment) in self
            .blue_green_deployments
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if deployment.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.blue_green_deployments.remove(&key);
        }
        for (key, integration) in self
            .integrations
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if integration.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.integrations.remove(&key);
        }
        // A tenant database settles in its own walk: applying a staged rename MOVES the
        // record, because the key it is stored under carries its name.
        self.settle_tenant_databases(now, region);
        for (key, proxy) in self
            .db_proxies
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if proxy.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_proxies.remove(&key);
            // The proxy's endpoint goes with it: dropping the record closes the port and
            // severs every session a client had open on it, so the address the proxy
            // published answers nothing once the proxy is gone.
            self.db_proxy_endpoints.remove(&key);
            // And so do the endpoints a caller ADDED to that proxy: an endpoint "associated
            // with a DB proxy" has no proxy to pool connections through once that proxy is
            // gone, which is why it is removed here rather than left naming it — and the
            // listener each of them published goes with it, which closes its port and severs
            // its sessions.
            let mut endpoints_gone: Vec<RegionalKey> = Vec::new();
            self.db_proxy_custom_endpoints.retain(|endpoint_key, record| {
                let belongs = endpoint_key.0 == key.0 && record.db_proxy_name == key.1;
                if belongs {
                    endpoints_gone.push(endpoint_key.clone());
                }
                !belongs
            });
            for endpoint_key in endpoints_gone {
                self.db_proxy_custom_endpoint_planes.remove(&endpoint_key);
            }
        }
        for (key, endpoint) in self
            .db_proxy_custom_endpoints
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if endpoint.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.db_proxy_custom_endpoints.remove(&key);
            // The endpoint is gone from the service, so the listener it published goes with
            // it: dropping the record closes the port and severs every session on it, after
            // which nothing answers at the address that endpoint published.
            self.db_proxy_custom_endpoint_planes.remove(&key);
        }
        for (key, export_task) in self
            .export_tasks
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if export_task.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone.drain(..) {
            self.export_tasks.remove(&key);
        }
        for (key, version) in self
            .custom_db_engine_versions
            .range_mut((region.to_string(), String::new())..)
        {
            if key.0 != region {
                break;
            }
            if version.settle(now) {
                gone.push(key.clone());
            }
        }
        for key in gone {
            self.custom_db_engine_versions.remove(&key);
        }
        // The automated backups replicated INTO this region owe the clock the one transition
        // a backup still waiting for its first replicated snapshot has: `creating` ->
        // `active`. Nothing removes one, so this walk answers no keys.
        self.settle_replicated_instance_backups(region, now);
        // A reserved DB instance settles against two clocks as well: the state the purchase
        // owed it, and the retirement its own term makes due
        // ([`Self::settle_reserved_db_instances`]).
        self.settle_reserved_db_instances(region, now);
        // An event subscription is in no transient status and owes the clock no
        // transition: it is a notification rule, so a create, a modify and a delete of one
        // are all in force by the time that call answers (see `EventSubscriptionRecord`).
        // A global cluster is ACCOUNT-scoped, so it sits in no region's range: the whole
        // map is walked, bounded by [`GLOBAL_CLUSTER_QUOTA_PER_ACCOUNT`], which is what
        // makes the transition a modify owes due whichever region the next request is
        // signed for.
        let mut gone_globally: Vec<String> = Vec::new();
        for (identifier, global) in self.global_clusters.iter_mut() {
            if global.settle(now) {
                gone_globally.push(identifier.clone());
            }
        }
        for identifier in gone_globally {
            self.global_clusters.remove(&identifier);
        }
        // An event past the published retention window is one `DescribeEvents` does not
        // report whether or not anybody calls, so dropping it here materialises an expiry
        // the clock already owed rather than changing what a caller can see.
        if let Some(log) = self.events.get_mut(region) {
            log.prune(now_as_wire_timestamp().as_secs_f64());
        }
        // The configuration analysis runs over the resources this region holds, now that
        // the statuses above are the ones the clock owes: see
        // [`Self::settle_db_recommendations`] for why what it writes was owed at this
        // instant too.
        self.settle_db_recommendations(region);
    }

    /// Record `event` for the resource `source_identifier` / `source_arn` in `region`: the
    /// log entry `DescribeEvents` then reports for the change the caller just made.
    ///
    /// Called by the handler that PERFORMED the change, with the store already locked, so
    /// a record exists only because the service did the thing it records. Dated NOW, which
    /// is when that handler made the change; a change the CLOCK completes later is recorded
    /// through [`Self::record_event_at`] with the instant it was made due instead.
    pub fn record_event(
        &mut self,
        region: &str,
        event: LifecycleEvent,
        source_identifier: &str,
        source_arn: String,
    ) {
        self.record_event_at(
            region,
            event,
            source_identifier,
            source_arn,
            now_as_wire_timestamp(),
        );
    }

    /// Take the publications queued by [`Self::record_event`], leaving the queue empty.
    ///
    /// Called by the one seam that publishes them, which holds this store only for the take
    /// itself ([`publish_recorded_events`]): the envelopes are owned values, so nothing of
    /// this store is borrowed across the publication.
    pub fn take_pending_bus_events(&mut self) -> Vec<BusEvent> {
        ::std::mem::take(&mut self.pending_bus_events)
    }

    /// The events recorded in `region`, oldest first — the order `DescribeEvents` reports
    /// and pages them in. A region nothing has happened in has none.
    pub fn events_in(&self, region: &str) -> &[EventRecord] {
        self.events
            .get(region)
            .map_or(&[], |log| log.entries.as_slice())
    }

    /// The custom engine versions stored in `region`, in `(engine, version)` key order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn custom_db_engine_versions_in(&self, region: &str) -> Vec<&CustomDbEngineVersionRecord> {
        range_in(&self.custom_db_engine_versions, region)
    }

    /// Whether `family` is the DB parameter group family of a CUSTOM engine version this
    /// account stored in `region`.
    ///
    /// A CEV is an engine version this front door publishes too — `DescribeDBEngineVersions`
    /// reports it beside the catalogue, carrying the family RDS writes for it — and a DB
    /// instance created on one takes the default parameter groups of THAT family. So a family
    /// a stored CEV names is a family that exists, which is the half of
    /// [`validate_parameter_group_family`]'s rule a pure function over the static catalogue
    /// cannot see: the create of a parameter group consults this before refusing a family.
    ///
    /// An ordered range over the region's own records, so the work is bounded by
    /// [`CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn publishes_custom_parameter_group_family(&self, region: &str, family: &str) -> bool {
        self.custom_db_engine_versions_in(region)
            .into_iter()
            .any(|cev| cev.parameter_group_family.eq_ignore_ascii_case(family))
    }

    /// The custom engine version `(engine, engine_version)` names in `region`, or `None`
    /// when the account created none of that name there.
    pub fn resolve_custom_db_engine_version(
        &self,
        region: &str,
        engine: &str,
        engine_version: &str,
    ) -> Option<&CustomDbEngineVersionRecord> {
        self.custom_db_engine_versions.get(&(
            region.to_string(),
            custom_db_engine_version_key(&engine.to_ascii_lowercase(), engine_version),
        ))
    }

    /// Whether anything in `region` is USING the custom engine version
    /// `(engine, engine_version)`: a DB instance running it, or a snapshot taken of one.
    ///
    /// What a CEV's availability status change is gated on — it "must not currently be in
    /// use" (`ModifyCustomDBEngineVersion$Status`) — which is what the modelled
    /// `InvalidCustomDBEngineVersionStateFault` reports. The uses answered from here are
    /// the two this service holds records of; the automated backups that member also names
    /// are not records this store carries, so nothing is claimed about them.
    pub fn custom_db_engine_version_in_use(
        &self,
        region: &str,
        engine: &str,
        engine_version: &str,
    ) -> bool {
        let runs_it = |record_engine: &str, record_version: Option<&str>| {
            record_engine.eq_ignore_ascii_case(engine)
                && record_version.is_some_and(|version| version == engine_version)
        };
        self.instances_in(region)
            .into_iter()
            .any(|instance| runs_it(&instance.engine, instance.engine_version.as_deref()))
            || self
                .snapshots_in(region)
                .into_iter()
                .any(|snapshot| runs_it(&snapshot.engine, snapshot.engine_version.as_deref()))
    }

    /// The custom engine version whose ARN is `arn`, in `region`, or `None` when no stored
    /// CEV carries it — what `CreateCustomDBEngineVersion$SourceCustomDbEngineVersionIdentifier`
    /// ("the ARN of a CEV to use as a source") is resolved against, so a source can only
    /// name a CEV some create really stored.
    pub fn resolve_custom_db_engine_version_arn(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&CustomDbEngineVersionRecord> {
        // Only an ARN can match, so a value of any other form does not pay for rendering
        // a region's stored ARNs.
        if !arn.starts_with("arn:") {
            return None;
        }
        self.custom_db_engine_versions_in(region)
            .into_iter()
            .find(|record| record.arn() == arn)
    }

    /// The export tasks stored in `region`, in identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// what one region holds however many regions the store holds.
    pub fn export_tasks_in(&self, region: &str) -> Vec<&ExportTaskRecord> {
        range_in(&self.export_tasks, region)
    }

    /// The export task `identifier` names in `region`, or `None` when nothing of that
    /// identifier exists — what a request that NAMES an export task is resolved against.
    ///
    /// `ExportTaskIdentifier` is "a unique identifier for the export task" and the model
    /// documents no case folding for it, so the lookup is exact, as the store is keyed.
    pub fn resolve_export_task(&self, region: &str, identifier: &str) -> Option<&ExportTaskRecord> {
        self.export_tasks
            .get(&(region.to_string(), identifier.to_string()))
    }

    /// The DB shard groups stored in `region`, in folded-identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`DB_SHARD_GROUP_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn shard_groups_in(&self, region: &str) -> Vec<&DbShardGroupRecord> {
        range_in(&self.db_shard_groups, region)
    }

    /// The DB shard group `named` resolves to in `region`, or `None` when nothing of that
    /// name exists — what a request that NAMES a DB shard group is resolved against.
    ///
    /// `DescribeDBShardGroupsMessage$DBShardGroupIdentifier` is "the user-supplied DB
    /// shard group identifier" and "this parameter isn't case-sensitive", so the lookup
    /// folds the way [`db_shard_group_key`] keys the store.
    pub fn resolve_db_shard_group(&self, region: &str, named: &str) -> Option<&DbShardGroupRecord> {
        self.db_shard_groups
            .get(&(region.to_string(), db_shard_group_key(named)))
    }

    /// The DB clusters stored in `region`, in identifier order.
    pub fn clusters_in(&self, region: &str) -> Vec<&DbClusterRecord> {
        range_in(&self.db_clusters, region)
    }

    /// The DB cluster `named` resolves to in `region`, or `None` when nothing of that name
    /// exists — what a request that NAMES a DB cluster is resolved against.
    ///
    /// `named` is "the user-supplied DB cluster identifier or the Amazon Resource Name
    /// (ARN) of the DB cluster", and "this parameter isn't case-sensitive": both forms are
    /// folded to the identifier the store is keyed by before the lookup, so the two name
    /// one and the same cluster.
    pub fn resolve_db_cluster(&self, region: &str, named: &str) -> Option<&DbClusterRecord> {
        let identifier = db_cluster_identifier_of(named).to_ascii_lowercase();
        self.db_clusters.get(&(region.to_string(), identifier))
    }

    /// The member DB instances of `record` — the instances that name this cluster — WRITER
    /// FIRST, then the readers earliest-created first.
    ///
    /// That is the order `DBClusterMembers` reports them in, and the one
    /// `FailoverDBCluster` reads to find the primary it is being asked to move, so both
    /// agree about which member is the writer by construction; see
    /// [`DbClusterRecord::writer_identifier`].
    pub fn cluster_members(&self, record: &DbClusterRecord) -> Vec<&DbInstanceRecord> {
        let mut members: Vec<&DbInstanceRecord> = self
            .instances_in(&record.region)
            .into_iter()
            .filter(|instance| {
                instance
                    .db_cluster_identifier
                    .as_ref()
                    .is_some_and(|named| named.eq_ignore_ascii_case(&record.identifier))
            })
            .collect();
        members.sort_by(|left, right| {
            // The writer first, then the rest earliest-created first. A cluster nobody has
            // failed over names no writer, so the order is purely by creation and the
            // first-created instance is the primary; after a failover the promoted member
            // sorts ahead of it, which is what moves `IsClusterWriter`.
            let writes = |instance: &DbInstanceRecord| {
                record
                    .writer_identifier
                    .as_ref()
                    .is_some_and(|named| named.eq_ignore_ascii_case(&instance.identifier))
            };
            writes(right)
                .cmp(&writes(left))
                .then_with(|| {
                    left.instance_create_time
                        .as_secs_f64()
                        .total_cmp(&right.instance_create_time.as_secs_f64())
                })
                .then_with(|| left.identifier.cmp(&right.identifier))
        });
        members
    }

    /// What a DB cluster reports about the records AROUND it — see [`ClusterMembership`].
    fn cluster_membership(&self, record: &DbClusterRecord) -> ClusterMembership {
        let members = self.cluster_members(record);
        ClusterMembership {
            // "Indicates whether the DB cluster has instances in multiple Availability
            // Zones" — read off the zones its members are actually in, so it cannot
            // disagree with them.
            multi_az: members
                .iter()
                .map(|instance| instance.availability_zone.as_str())
                .collect::<BTreeSet<&str>>()
                .len()
                > 1,
            members: members
                .iter()
                .enumerate()
                .map(|(position, instance)| DBClusterMember {
                    db_instance_identifier: Some(instance.identifier.clone()),
                    is_cluster_writer: Some(position == 0),
                    db_cluster_parameter_group_status: Some(MEMBERSHIP_IN_SYNC.to_string()),
                    // `PromotionTier` is "the order in which an Aurora Replica is promoted
                    // to the primary instance": a property of the MEMBER instance, read off
                    // that instance's own record, so this member and the `PromotionTier`
                    // the instance itself reports are one answer.
                    promotion_tier: instance.promotion_tier(),
                })
                .collect(),
            // "Contains one or more identifiers of the read replicas associated with this
            // DB cluster": the clusters in this region whose replication source is this
            // one, and the DB instances created as read replicas OF it
            // (`CreateDBInstanceReadReplica$SourceDBClusterIdentifier`, the Multi-AZ DB
            // cluster that "will act as the source for the read replica"). Both are read
            // from those records rather than copied onto this one, so a replica this store
            // no longer holds is not one the cluster reports.
            read_replica_identifiers: self
                .clusters_in(&record.region)
                .into_iter()
                .filter(|other| {
                    other
                        .replication_source_arn
                        .as_ref()
                        .is_some_and(|source| *source == record.arn())
                })
                .map(|other| other.identifier.clone())
                .chain(
                    self.instances_in(&record.region)
                        .into_iter()
                        .filter(|instance| {
                            instance
                                .read_replica_source_db_cluster_identifier
                                .as_ref()
                                .is_some_and(|source| {
                                    source.eq_ignore_ascii_case(&record.identifier)
                                })
                        })
                        .map(|instance| instance.identifier.clone()),
                )
                .collect(),
            // The global database cluster the cluster belongs to, read from that global
            // cluster's own record: the cluster stores only its identifier, and the
            // immutable id the envelope reports beside it is the one that record carries,
            // so the two cannot name different global clusters.
            global_cluster_resource_id: record
                .global_cluster_identifier
                .as_ref()
                .and_then(|identifier| self.global_clusters.get(identifier))
                .map(|global| global.resource_id.clone()),
            // "backing-up — the DB cluster is currently being backed up": a backup of this
            // cluster is in flight exactly while a snapshot OF it is still being taken, so
            // it is read off those snapshot records — as the members above are read off the
            // instances — and never copied onto the cluster.
            backing_up: self
                .cluster_snapshots_in(&record.region)
                .into_iter()
                .any(|snapshot| {
                    snapshot.db_cluster_identifier == record.identifier
                        && snapshot.status == SNAPSHOT_STATUS_CREATING
                }),
            // "The custom endpoints associated with the DB cluster": the endpoints
            // `CreateDBClusterEndpoint` stored for this cluster, read off those records and
            // reported as the addresses they publish — the same address each of them
            // reports as its own `Endpoint`, since both come from one projection.
            custom_endpoints: self
                .cluster_endpoints_of(&record.region, &record.identifier)
                .into_iter()
                .filter_map(|endpoint| self.cluster_endpoint_to_wire(endpoint).endpoint)
                .collect(),
            // "The details for Aurora Limitless Database": reported for a cluster that
            // operates as one, with the status read off the DB SHARD GROUPS of this cluster —
            // the compute the Limitless database runs on — as the members above are read off
            // the instances and the endpoints ([`DbClusterRecord::limitless_database`]).
            limitless_database: record.limitless_database(
                self.shard_groups_in(&record.region)
                    .into_iter()
                    .any(|shard_group| {
                        shard_group
                            .db_cluster_identifier
                            .eq_ignore_ascii_case(&record.identifier)
                            && shard_group.is_available()
                    }),
            ),
        }
    }

    /// Project a stored DB cluster onto the modelled `DBCluster` output shape, with the
    /// members and read replicas read from the records that carry them.
    pub fn cluster_to_wire(&self, record: &DbClusterRecord) -> DBCluster {
        record.to_wire(&self.cluster_membership(record))
    }

    /// Project a stored DB instance onto the modelled `DBInstance` output shape, with the
    /// nested `DBSubnetGroup` read from the DB subnet group record the instance's own
    /// `DBSubnetGroupName` resolves to ([`Self::db_subnet_group_of`]) and the DB CLUSTER it
    /// belongs to read from that cluster's own record ([`Self::db_cluster_of`]).
    ///
    /// The one way a handler projects an instance — the instance-level counterpart of
    /// [`Self::cluster_to_wire`] — so every operation that reports a DB instance reports
    /// the same group for it, and a group `ModifyDBSubnetGroup` changes is reported as it
    /// is now rather than as it was when the instance was created.
    pub fn instance_to_wire(&self, record: &DbInstanceRecord) -> DBInstance {
        record.to_wire(
            self.db_subnet_group_of(record).as_ref(),
            self.db_parameter_group_of(record).as_ref(),
            self.db_cluster_of(record).as_ref(),
        )
    }

    /// The DB cluster `record` is a member of, as this store holds it, or `None` for an
    /// instance that belongs to none and for a cluster this store no longer resolves.
    ///
    /// Taken by VALUE for the reason [`Self::db_subnet_group_of`] is. What a MEMBER of a
    /// cluster reports for the members its cluster owns — its storage, its master user —
    /// is read off this record, so the member and `DescribeDBClusters` cannot disagree
    /// about them.
    pub fn db_cluster_of(&self, record: &DbInstanceRecord) -> Option<DbClusterRecord> {
        self.resolve_db_cluster(&record.region, record.db_cluster_identifier.as_deref()?)
            .cloned()
    }

    /// The DB parameter group `record` names, as this store holds it, or `None` for an
    /// instance that names none and for one of the DEFAULT groups AWS provides, which no
    /// request creates and this store therefore has no record of (such a group sets no
    /// parameter of its own, so a membership in it is always in sync).
    ///
    /// Taken by VALUE for the reason [`Self::db_subnet_group_of`] is: a handler that holds the
    /// instance record mutably still has to report the membership it projects.
    pub fn db_parameter_group_of(
        &self,
        record: &DbInstanceRecord,
    ) -> Option<DbParameterGroupRecord> {
        let name = record.db_parameter_group_name.as_ref()?;
        self.db_parameter_groups
            .get(&(record.region.clone(), name.to_ascii_lowercase()))
            .cloned()
    }

    /// The DB subnet group `record` was created in, as this store holds it, or `None` for an
    /// instance that names none and for a name this store no longer resolves.
    ///
    /// Taken by VALUE so a handler that holds the instance record mutably can still read the
    /// group it reports: it is resolved before the record is taken, and
    /// [`Self::instance_to_wire`] is the same answer for a record held immutably.
    pub fn db_subnet_group_of(&self, record: &DbInstanceRecord) -> Option<DbSubnetGroupRecord> {
        self.resolve_db_subnet_group(&record.region, record.db_subnet_group_name.as_deref()?)
    }

    /// The DB instances stored in `region`, in identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`DB_INSTANCE_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn instances_in(&self, region: &str) -> Vec<&DbInstanceRecord> {
        range_in(&self.db_instances, region)
    }

    /// The DB snapshots stored in `region`, in identifier order.
    pub fn snapshots_in(&self, region: &str) -> Vec<&DbSnapshotRecord> {
        range_in(&self.db_snapshots, region)
    }

    /// The DB cluster snapshots stored in `region`, in identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by
    /// [`DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION`] however many regions the store holds.
    pub fn cluster_snapshots_in(&self, region: &str) -> Vec<&DbClusterSnapshotRecord> {
        range_in(&self.db_cluster_snapshots, region)
    }

    /// The key of the cluster snapshot `value` names IN `region`: either its
    /// (case-insensitive) identifier or its ARN, both of which
    /// `DBClusterSnapshotIdentifier` and `SourceDBClusterSnapshotIdentifier` accept.
    pub fn resolve_cluster_snapshot_key(&self, region: &str, value: &str) -> Option<String> {
        let lowered = value.to_ascii_lowercase();
        if self
            .db_cluster_snapshots
            .contains_key(&(region.to_string(), lowered.clone()))
        {
            return Some(lowered);
        }
        // Only an ARN can match the scan below, so a plain identifier that named nothing
        // does not pay for rendering a region's stored ARNs.
        if !value.starts_with("arn:") {
            return None;
        }
        self.cluster_snapshots_in(region)
            .into_iter()
            .find(|record| record.arn() == value)
            .map(|record| record.snapshot_identifier.clone())
    }

    /// The cluster parameter groups stored in `region`, in name order.
    pub fn cluster_parameter_groups_in(&self, region: &str) -> Vec<&DbClusterParameterGroupRecord> {
        range_in(&self.db_cluster_parameter_groups, region)
    }

    /// The cluster parameter group `named` resolves to in `region`, or `None` when nothing
    /// of that name exists.
    ///
    /// `named` is "the identifier or Amazon Resource Name (ARN) for the source DB cluster
    /// parameter group", so both forms resolve to the one record: the name folded the way
    /// the store keys it, or an ARN matched against the stored groups' own ARNs.
    pub fn resolve_cluster_parameter_group(
        &self,
        region: &str,
        named: &str,
    ) -> Option<&DbClusterParameterGroupRecord> {
        if let Some(stored) = self
            .db_cluster_parameter_groups
            .get(&(region.to_string(), named.to_ascii_lowercase()))
        {
            return Some(stored);
        }
        // Only an ARN can match the scan below, so a plain name that named nothing does
        // not pay for rendering a region's stored ARNs.
        if !named.starts_with("arn:") {
            return None;
        }
        self.cluster_parameter_groups_in(region)
            .into_iter()
            .find(|record| record.arn() == named)
    }

    /// Every DB CLUSTER parameter group a request signed for `region` can SEE, in name
    /// order: the groups a request created there plus the default group RDS provides for
    /// each engine-version family with a cluster form
    /// ([`default_db_cluster_parameter_groups`]).
    ///
    /// The cluster-level counterpart of [`Self::visible_db_parameter_groups`], and for the
    /// same reason: the real service's account has those groups from the moment it exists,
    /// and no request creates or changes one.
    pub fn visible_cluster_parameter_groups(
        &self,
        region: &str,
    ) -> Vec<DbClusterParameterGroupRecord> {
        let mut groups: Vec<DbClusterParameterGroupRecord> = self
            .cluster_parameter_groups_in(region)
            .into_iter()
            .cloned()
            .chain(default_db_cluster_parameter_groups(region))
            .collect();
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        groups
    }

    /// The DB CLUSTER parameter group `named` resolves to in `region` — a group a request
    /// created or one of RDS's own defaults — or `None` when nothing of that name exists;
    /// the cluster-level counterpart of [`Self::resolve_visible_db_parameter_group`].
    ///
    /// `DBClusterParameterGroupName` "is stored as a lowercase string", so the name is
    /// folded the way every create stores it.
    pub fn resolve_visible_cluster_parameter_group(
        &self,
        region: &str,
        named: &str,
    ) -> Option<DbClusterParameterGroupRecord> {
        let folded = named.to_ascii_lowercase();
        if let Some(stored) = self
            .db_cluster_parameter_groups
            .get(&(region.to_string(), folded.clone()))
        {
            return Some(stored.clone());
        }
        default_db_cluster_parameter_group(region, &folded)
    }

    /// The DB (instance) parameter groups stored in `region`, in name order.
    pub fn parameter_groups_in(&self, region: &str) -> Vec<&DbParameterGroupRecord> {
        range_in(&self.db_parameter_groups, region)
    }

    /// The DB (instance) parameter group `named` resolves to in `region`, or `None` when
    /// nothing of that name was created there; the instance-level sibling of
    /// [`Self::resolve_cluster_parameter_group`], resolving the same two forms — "the
    /// identifier or ARN for the source DB parameter group" — the same way.
    ///
    /// This resolves the groups a request CREATED. The groups RDS provides itself are
    /// reached through [`Self::resolve_visible_db_parameter_group`], because an operation
    /// that may not touch one of those has to tell them apart.
    pub fn resolve_db_parameter_group(
        &self,
        region: &str,
        named: &str,
    ) -> Option<&DbParameterGroupRecord> {
        if let Some(stored) = self
            .db_parameter_groups
            .get(&(region.to_string(), named.to_ascii_lowercase()))
        {
            return Some(stored);
        }
        // Only an ARN can match the scan below, so a plain name that named nothing does
        // not pay for rendering a region's stored ARNs.
        if !named.starts_with("arn:") {
            return None;
        }
        self.parameter_groups_in(region)
            .into_iter()
            .find(|record| record.arn() == named)
    }

    /// Every DB parameter group a request signed for `region` can SEE, in name order: the
    /// groups a request created there plus the default group RDS provides for each
    /// engine-version family ([`default_db_parameter_groups`]).
    ///
    /// The default groups are not records of this store, because no request creates them
    /// and none can change them; they are reported beside the account's own because the
    /// real service's account has them from the moment it exists. This is the
    /// parameter-group counterpart of [`Self::visible_db_subnet_groups`].
    pub fn visible_db_parameter_groups(&self, region: &str) -> Vec<DbParameterGroupRecord> {
        let mut groups: Vec<DbParameterGroupRecord> = self
            .parameter_groups_in(region)
            .into_iter()
            .cloned()
            .chain(default_db_parameter_groups(region))
            .collect();
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        groups
    }

    /// The DB parameter group `named` resolves to in `region` — a group a request created
    /// or one of RDS's own defaults — or `None` when nothing of that name exists.
    ///
    /// `DBParameterGroupName` "is stored as a lowercase string", so the name is folded the
    /// way every create stores it.
    pub fn resolve_visible_db_parameter_group(
        &self,
        region: &str,
        named: &str,
    ) -> Option<DbParameterGroupRecord> {
        let folded = named.to_ascii_lowercase();
        if let Some(stored) = self
            .db_parameter_groups
            .get(&(region.to_string(), folded.clone()))
        {
            return Some(stored.clone());
        }
        default_db_parameter_group(region, &folded)
    }

    /// The event notification subscriptions stored in `region`, in subscription-name
    /// order.
    pub fn event_subscriptions_in(&self, region: &str) -> Vec<&EventSubscriptionRecord> {
        range_in(&self.event_subscriptions, region)
    }

    /// The identifiers an event subscription's source of `source_type` may name in
    /// `region`, or `None` when this service holds no records of that source type at all.
    ///
    /// An event source identifier is bound to one resource type per source type ("If the
    /// source type is a DB instance, a `DBInstanceIdentifier` value must be supplied; if
    /// the source type is a DB parameter group, a `DBParameterGroupName` value must be
    /// supplied", and so on for each published type), so a source identifier is resolved
    /// against the stored resources of its subscription's source type and one that names
    /// none is the modelled `SourceNotFoundFault`.
    ///
    /// `None` is for a value whose resources this service holds no record of, so there is
    /// nothing to resolve against. Among the values [`EVENT_SUBSCRIPTION_SOURCE_TYPES`]
    /// carries, that is `db-security-group`, which no operation here creates, and
    /// `custom-engine-version`, which this store keys by engine AND version rather than by
    /// one identifier a source id could carry; an identifier of such a type is accepted
    /// rather than refused for a record this service could never have. A value outside that
    /// published list falls through here too, and never reaches this resolver:
    /// `check_event_subscription_source_type` refuses it before a subscription of that type
    /// can exist.
    pub fn event_source_identifiers_in(
        &self,
        region: &str,
        source_type: &str,
    ) -> Option<Vec<String>> {
        match source_type {
            "db-instance" => Some(
                self.instances_in(region)
                    .into_iter()
                    .map(|instance| instance.identifier.clone())
                    .collect(),
            ),
            "db-cluster" => Some(
                self.clusters_in(region)
                    .into_iter()
                    .map(|cluster| cluster.identifier.clone())
                    .collect(),
            ),
            "db-snapshot" => Some(
                self.snapshots_in(region)
                    .into_iter()
                    .map(|snapshot| snapshot.snapshot_identifier.clone())
                    .collect(),
            ),
            "db-cluster-snapshot" => Some(
                self.cluster_snapshots_in(region)
                    .into_iter()
                    .map(|snapshot| snapshot.snapshot_identifier.clone())
                    .collect(),
            ),
            "db-parameter-group" => Some(
                self.parameter_groups_in(region)
                    .into_iter()
                    .map(|group| group.name.clone())
                    .collect(),
            ),
            "db-proxy" => Some(
                self.db_proxies_in(region)
                    .into_iter()
                    .map(|proxy| proxy.name.clone())
                    .collect(),
            ),
            "zero-etl" => Some(
                self.integrations_in(region)
                    .into_iter()
                    .map(|integration| integration.name.clone())
                    .collect(),
            ),
            "blue-green-deployment" => Some(
                self.blue_green_deployments_in(region)
                    .into_iter()
                    .map(|deployment| deployment.identifier.clone())
                    .collect(),
            ),
            "db-shard-group" => Some(
                self.shard_groups_in(region)
                    .into_iter()
                    .map(|shard_group| shard_group.identifier.clone())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The option groups stored in `region`, in name order.
    pub fn option_groups_in(&self, region: &str) -> Vec<&OptionGroupRecord> {
        range_in(&self.option_groups, region)
    }

    /// The option group `named` resolves to in `region`, or `None` when nothing of that
    /// name exists there.
    ///
    /// A member that must "specify a valid option group" names it either by its
    /// `OptionGroupName` or by the `OptionGroupArn` this service reports for it, so both
    /// forms resolve to the one record. `OptionGroupName` carries no "isn't
    /// case-sensitive" note — the members that fold case say so explicitly — so the name
    /// is matched exactly as the store keys it.
    pub fn resolve_option_group(&self, region: &str, named: &str) -> Option<&OptionGroupRecord> {
        if let Some(stored) = self
            .option_groups
            .get(&(region.to_string(), named.to_string()))
        {
            return Some(stored);
        }
        // Only an ARN can match the scan below, so a plain name that named nothing does
        // not pay for rendering a region's stored ARNs.
        if !named.starts_with("arn:") {
            return None;
        }
        self.option_groups_in(region)
            .into_iter()
            .find(|record| record.arn() == named)
    }

    /// Every option group a request signed for `region` can SEE, in name order: the groups
    /// a request created there plus the default group RDS provides for each engine and
    /// major engine version ([`default_option_groups`]).
    ///
    /// The option-group counterpart of [`Self::visible_db_parameter_groups`], and for the
    /// same reason: the real service's account has those groups from the moment it exists,
    /// and no request creates, changes or deletes one.
    pub fn visible_option_groups(&self, region: &str) -> Vec<OptionGroupRecord> {
        let mut groups: Vec<OptionGroupRecord> = self
            .option_groups_in(region)
            .into_iter()
            .cloned()
            .chain(default_option_groups(region))
            .collect();
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        groups
    }

    /// The option group `named` resolves to in `region` — a group a request created or one
    /// of RDS's own defaults — or `None` when nothing of that name exists; the resolving
    /// sibling of [`Self::resolve_option_group`] that sees the AWS-managed groups too.
    pub fn resolve_visible_option_group(
        &self,
        region: &str,
        named: &str,
    ) -> Option<OptionGroupRecord> {
        if let Some(stored) = self.resolve_option_group(region, named) {
            return Some(stored.clone());
        }
        default_option_group(region, named)
    }

    /// The DB subnet groups stored in `region`, in name order.
    pub fn db_subnet_groups_in(&self, region: &str) -> Vec<&DbSubnetGroupRecord> {
        range_in(&self.db_subnet_groups, region)
    }

    /// The DB security groups stored in `region`, in name order.
    pub fn db_security_groups_in(&self, region: &str) -> Vec<&DbSecurityGroupRecord> {
        range_in(&self.db_security_groups, region)
    }

    /// The DB security group `name` resolves to in `region`, or `None` when nothing of
    /// that name is stored there.
    ///
    /// `DBSecurityGroupName` "is stored as a lowercase string", so a lookup folds the
    /// name the same way the create that stored it did and one spelling cannot hide a
    /// group another spelling created.
    pub fn resolve_db_security_group(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&DbSecurityGroupRecord> {
        self.db_security_groups
            .get(&(region.to_string(), name.to_ascii_lowercase()))
    }

    /// The blue/green deployments stored in `region`, in identifier order.
    pub fn blue_green_deployments_in(&self, region: &str) -> Vec<&BlueGreenDeploymentRecord> {
        range_in(&self.blue_green_deployments, region)
    }

    /// The blue/green deployment `named` resolves to in `region`, or `None` when nothing
    /// of that identifier exists. `named` is either the identifier or the deployment's
    /// ARN, and "this parameter isn't case-sensitive".
    pub fn resolve_blue_green_deployment(
        &self,
        region: &str,
        named: &str,
    ) -> Option<&BlueGreenDeploymentRecord> {
        let identifier = blue_green_deployment_identifier_of(named).to_ascii_lowercase();
        self.blue_green_deployments
            .get(&(region.to_string(), identifier))
    }

    /// Whether the database `arn` serves NO WRITES because of a blue/green deployment this
    /// store holds — which is what makes its endpoint refuse every statement that writes.
    ///
    /// One side of a deployment's pairing is replaying the other's writes rather than taking
    /// its own, and WHICH side that is follows production:
    ///
    /// * before the switchover it is the GREEN database. "The databases in the green
    ///   environment are read-only": the green environment is kept in sync with production by
    ///   replaying the blue environment's writes, so a client cannot write to it, exactly as
    ///   it cannot write to a read replica.
    /// * after the switchover it is the BLUE one. Production has moved to the green
    ///   environment, and the databases left behind in the blue environment are kept for
    ///   their data — renamed, retaining their automated backups, and taking no further
    ///   writes.
    ///
    /// The pairing is what says so, so a database stops being read-only when the deployment
    /// that paired it is no longer there — which is what a delete of the deployment leaves
    /// behind.
    pub fn serves_reads_only_in_a_blue_green_deployment(&self, region: &str, arn: &str) -> bool {
        self.blue_green_deployments_in(region)
            .into_iter()
            .any(|deployment| {
                let read_only = match deployment.switched_over {
                    false => &deployment.target_arn,
                    true => &deployment.source_arn,
                };
                read_only.eq_ignore_ascii_case(arn)
            })
    }

    /// The integrations stored in `region`, in name order.
    pub fn integrations_in(&self, region: &str) -> Vec<&IntegrationRecord> {
        range_in(&self.integrations, region)
    }

    /// The integration `named` resolves to in `region`, or `None` when nothing of that
    /// name exists.
    ///
    /// `DescribeIntegrations$IntegrationIdentifier` is "the unique identifier of the
    /// integration", which is either the name a create gave it or its ARN, so both forms
    /// are resolved — the ARN against the stored records' own ARNs, since the id inside it
    /// is the minted one.
    pub fn resolve_integration(&self, region: &str, named: &str) -> Option<&IntegrationRecord> {
        if let Some(stored) = self.integrations.get(&(region.to_string(), named.to_string())) {
            return Some(stored);
        }
        // Only an ARN can match the scan below, so a plain name that named nothing does
        // not pay for rendering a region's stored ARNs.
        if !named.starts_with("arn:") {
            return None;
        }
        self.integrations_in(region)
            .into_iter()
            .find(|record| record.arn() == named)
    }

    /// The tenant databases stored in `region` for the DB instance `db_instance_identifier`,
    /// in tenant-name order.
    ///
    /// An ordered range over the one instance's keys rather than a scan of the region, so
    /// the work is bounded by [`TENANT_DATABASE_QUOTA_PER_INSTANCE`].
    pub fn tenant_databases_of(
        &self,
        region: &str,
        db_instance_identifier: &str,
    ) -> Vec<&TenantDatabaseRecord> {
        let prefix = format!("{db_instance_identifier}/");
        self.tenant_databases
            .range((region.to_string(), prefix.clone())..)
            .take_while(|((stored, key), _)| stored == region && key.starts_with(&prefix))
            .map(|(_, record)| record)
            .collect()
    }

    /// The tenant databases stored in `region`, in instance-then-tenant-name order.
    pub fn tenant_databases_in(&self, region: &str) -> Vec<&TenantDatabaseRecord> {
        range_in(&self.tenant_databases, region)
    }

    /// The RDS proxies stored in `region`, in name order.
    pub fn db_proxies_in(&self, region: &str) -> Vec<&DbProxyRecord> {
        range_in(&self.db_proxies, region)
    }

    /// The endpoints callers ADDED to proxies of `region`, in endpoint-name order — the
    /// order `DescribeDBProxyEndpoints` reports and pages them in.
    pub fn proxy_custom_endpoints_in(&self, region: &str) -> Vec<&DbProxyCustomEndpointRecord> {
        range_in(&self.db_proxy_custom_endpoints, region)
    }

    /// The endpoints added to the proxy `proxy_name` in `region`, in the same order.
    ///
    /// An ordered range over the region's own endpoints, so the work is bounded by what that
    /// region may hold however many regions the store holds.
    pub fn proxy_custom_endpoints_of(
        &self,
        region: &str,
        proxy_name: &str,
    ) -> Vec<&DbProxyCustomEndpointRecord> {
        self.proxy_custom_endpoints_in(region)
            .into_iter()
            .filter(|endpoint| endpoint.db_proxy_name == proxy_name)
            .collect()
    }

    /// What the proxy target `target` reports about the database behind it, read from that
    /// database's own record in `region`, or `None` when that database is no longer
    /// stored.
    pub fn registered_database(
        &self,
        region: &str,
        target: &DbProxyTargetRecord,
    ) -> Option<RegisteredDatabase> {
        if target.is_cluster {
            let cluster = self.db_clusters.get(&(
                region.to_string(),
                target.identifier.clone(),
            ))?;
            return Some(RegisteredDatabase {
                arn: cluster.arn(),
                endpoint: cluster.endpoint(),
                // The address and port a client reaches the cluster on, as for an
                // instance below.
                port: cluster.published_port(),
            });
        }
        let instance = self
            .db_instances
            .get(&(region.to_string(), target.identifier.clone()))?;
        Some(RegisteredDatabase {
            arn: instance.arn(),
            endpoint: instance.endpoint_address(),
            // The address and port a client reaches the instance on, which is what a
            // proxy target reports: for an instance whose engine this emulator serves,
            // that is its engine listener's (`data_plane`), and for a member of a DB
            // cluster that manages its port, that cluster's
            // ([`DbInstanceRecord::endpoint_published_port`]).
            port: instance.endpoint_published_port(self.db_cluster_of(instance).as_ref()),
        })
    }

    /// What one registration contributes to a proxy's target report in `region`, each
    /// target keyed by the database it reports (which is the key the report pages on).
    ///
    /// A registered DB INSTANCE contributes exactly itself. A registered DB CLUSTER is
    /// TRACKED: the proxy reaches the cluster's DB instances through it, so the
    /// registration contributes the cluster's own target and one `RDS_INSTANCE` target per
    /// member instance — the writer first, as the cluster reports its members — computed
    /// from the cluster's CURRENT membership, so a member added or removed after the
    /// registration is reported exactly as the cluster reports it.
    /// `unreadable_secret` is the `Auth` secret the proxy may not read, as the CALLER
    /// established it against IAM for this report ([`proxy_secret_access`]): the health of
    /// every target of a proxy that cannot read its credentials is the same, because it is
    /// the proxy's login that fails and not one database.
    fn proxy_target_rows(
        &self,
        region: &str,
        target: &DbProxyTargetRecord,
        unreadable_secret: Option<&str>,
    ) -> Vec<ProxyTargetRow> {
        let now = Instant::now();
        let mut rows = vec![ProxyTargetRow::registration(
            target,
            self.registered_database(region, target),
            now,
            unreadable_secret,
        )];
        if target.is_cluster {
            if let Some(cluster) = self
                .db_clusters
                .get(&(region.to_string(), target.identifier.clone()))
            {
                let in_force = target.is_in_force(now);
                for (position, member) in self.cluster_members(cluster).into_iter().enumerate() {
                    rows.push(ProxyTargetRow::tracked_instance(
                        &target.identifier,
                        &member.identifier,
                        Some(RegisteredDatabase {
                            arn: member.arn(),
                            endpoint: member.endpoint_address(),
                            port: member.endpoint_published_port(Some(cluster)),
                        }),
                        // `cluster_members` reports the writer first, which is the member
                        // `DBClusterMembers` reports `IsClusterWriter` for.
                        position == 0,
                        unreadable_secret,
                        in_force,
                    ));
                }
            }
        }
        rows
    }

    /// The databases a proxy reaches through one registration: the DB instance or DB
    /// cluster the registration names and, for a tracked cluster, the member instances the
    /// proxy reaches through it — the database every target that registration contributes
    /// to the report reports, keyed as [`ProxyTargetKey`] documents.
    ///
    /// WHICH databases a registration reaches does not depend on their health, so this asks
    /// for the rows with nothing established about the proxy's credentials.
    pub fn proxy_reached_databases(
        &self,
        region: &str,
        registration: &DbProxyTargetRecord,
    ) -> Vec<ProxyTargetKey> {
        self.proxy_target_rows(region, registration, None)
            .iter()
            .map(ProxyTargetRow::database_key)
            .collect()
    }

    /// The targets a proxy's registrations contribute to its report in `region`, in
    /// registration order, each keyed by the database it reports.
    ///
    /// One database is ONE target, so a database that two registrations both reach is
    /// reported once, under the first registration that reaches it; the key is the
    /// database's, so a DB instance and a DB cluster of one and the same identifier stay
    /// two targets and each key a page can resume after stays unique.
    ///
    /// The walk is bounded by what the store already caps: a proxy holds at most
    /// [`DB_PROXY_TARGET_QUOTA_PER_PROXY`] registrations and a region at most
    /// [`DB_INSTANCE_QUOTA_PER_REGION`] DB instances, so neither the work nor the list a
    /// request can ask for grows without a bound.
    pub fn proxy_report_targets(
        &self,
        region: &str,
        registrations: &[DbProxyTargetRecord],
        unreadable_secret: Option<&str>,
    ) -> Vec<(String, crate::ops::types::DBProxyTarget)> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        registrations
            .iter()
            .flat_map(|target| self.proxy_target_rows(region, target, unreadable_secret))
            .map(|row| (row.page_key(), row))
            .filter(|(key, _)| seen.insert(key.clone()))
            .map(|(key, row)| (key, row.to_wire()))
            .collect()
    }

    /// The DB subnet group `name` resolves to in `region`, or `None` when nothing of
    /// that name exists — what a request that NAMES a DB subnet group is resolved
    /// against.
    ///
    /// Two things resolve: a group `CreateDBSubnetGroup` stored, and the `default`
    /// group the real service provides in every region alongside the account's default
    /// VPC. The default one is not stored because nobody creates it —
    /// `CreateDBSubnetGroup` refuses that name ("Must not be default") for exactly that
    /// reason — and `CreateDBInstance$PubliclyAccessible` documents a default that
    /// branches on it, so a request naming it is resolving a group that exists.
    pub fn resolve_db_subnet_group(
        &self,
        region: &str,
        name: &str,
    ) -> Option<DbSubnetGroupRecord> {
        if let Some(stored) = self
            .db_subnet_groups
            .get(&(region.to_string(), name.to_string()))
        {
            return Some(stored.clone());
        }
        (name == DEFAULT_DB_SUBNET_GROUP)
            .then(|| self.account_default_db_subnet_group(region))
    }

    /// Every DB subnet group a request signed for `region` can see, in name order: the
    /// groups stored there plus the account's `default` group.
    pub fn visible_db_subnet_groups(&self, region: &str) -> Vec<DbSubnetGroupRecord> {
        let mut groups: Vec<DbSubnetGroupRecord> = self
            .db_subnet_groups_in(region)
            .into_iter()
            .cloned()
            .collect();
        groups.push(self.account_default_db_subnet_group(region));
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        groups
    }

    /// Whether a record may be stored for `region`: either it already holds records, or
    /// the store has room for one more region ([`MAX_REGIONS`]).
    ///
    /// The count walks the keys of every map that opens a region bucket, which the
    /// bounds above already cap in total (and the override map holds at most one entry
    /// per region), so the work is bounded by that total rather than by anything a
    /// caller chooses. Every operation that would create a bucket for a region holding
    /// no record yet gates on this, so a caller cannot open an unbounded number of them
    /// by naming a fresh region on each request.
    pub fn has_room_for_region(&self, region: &str) -> bool {
        let known: BTreeSet<&str> = self
            .db_instances
            .keys()
            .chain(self.db_snapshots.keys())
            // A retained automated backup outlives the instance it was taken of, so its
            // region is counted here too rather than only while that instance is stored.
            .chain(self.db_instance_automated_backups.keys())
            // And so do the automated backups replicated INTO a region, which outlive both
            // the source instance and the replication that took them.
            .chain(self.db_instance_automated_backup_replications.keys())
            .chain(self.db_cluster_snapshots.keys())
            .chain(self.db_clusters.keys())
            .chain(self.db_cluster_endpoints.keys())
            .chain(self.db_cluster_parameter_groups.keys())
            .chain(self.db_parameter_groups.keys())
            .chain(self.event_subscriptions.keys())
            .chain(self.option_groups.keys())
            .chain(self.db_subnet_groups.keys())
            .chain(self.db_security_groups.keys())
            .chain(self.db_shard_groups.keys())
            .chain(self.db_capacity_reservations.keys())
            .chain(self.reserved_db_instances.keys())
            .chain(self.blue_green_deployments.keys())
            .chain(self.integrations.keys())
            .chain(self.tenant_databases.keys())
            .chain(self.db_proxies.keys())
            .chain(self.export_tasks.keys())
            .chain(self.custom_db_engine_versions.keys())
            // A raised recommendation is about a resource of the same region, so its
            // bucket is already counted; it is chained all the same, so no map of this
            // store can hold a region the cap does not see.
            .chain(self.db_recommendations.keys())
            .map(|(stored, _)| stored.as_str())
            .chain(self.certificate_overrides.keys().map(String::as_str))
            // The event log outlives the records it is about — a deleted instance's events
            // are still readable — so its regions are counted too, or a region whose
            // records were all deleted would keep a bucket nothing bounds.
            .chain(self.events.keys().map(String::as_str))
            // The network the owner of subnets answered for a region is cached per region
            // and outlives the records it was read for, so its keys are counted too: a
            // read that caches one must not be able to open a bucket the cap cannot see.
            .chain(
                self.default_subnet_group_networks
                    .keys()
                    .map(String::as_str),
            )
            .collect();
        known.contains(region) || known.len() < MAX_REGIONS
    }
}

/// The values of `map` whose key names `region`, in key order.
fn range_in<'a, T>(map: &'a BTreeMap<RegionalKey, T>, region: &str) -> Vec<&'a T> {
    map.range((region.to_string(), String::new())..)
        .take_while(|((stored, _), _)| stored == region)
        .map(|(_, record)| record)
        .collect()
}

/// What the clock owes a record once its due instant passes.
#[derive(Debug, Clone)]
pub enum PendingChange {
    /// The resource reaches this status (`creating` -> `available`, say).
    Status(String),
    /// The resource is gone from the service: the end of a delete that was already
    /// accepted, which "can't be canceled or reverted after it begins".
    Removal,
}

/// A change this service owes: what becomes of the resource, and when that is due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub change: PendingChange,
    pub due_at: Instant,
}

impl PendingStatus {
    /// A transition to `status`, due [`STATUS_TRANSITION_DELAY`] from now.
    pub fn in_a_moment(status: &str) -> Self {
        Self {
            change: PendingChange::Status(status.to_string()),
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }

    /// A transition to `status`, due [`STATUS_TRANSITION_DELAY`] after `previous` — the
    /// instant the transition BEFORE it in the same progression became due.
    ///
    /// What a record whose progression has more than one step owes the clock: the span of the
    /// second step runs from the moment the first was due, not from the moment somebody
    /// looked. So a caller that reads a resource long after it started moving finds the
    /// progression as far along as the clock has taken it, rather than one step per read.
    pub fn in_a_moment_after(previous: Instant, status: &str) -> Self {
        Self {
            change: PendingChange::Status(status.to_string()),
            due_at: previous + STATUS_TRANSITION_DELAY,
        }
    }

    /// The record's removal from the store, due [`STATUS_TRANSITION_DELAY`] after `previous`
    /// — the instant the transition before it became due, for the reason
    /// [`Self::in_a_moment_after`] gives: the second step of a progression is owed from the
    /// first step's due instant, not from the moment somebody looked.
    pub fn removal_in_a_moment_after(previous: Instant) -> Self {
        Self {
            change: PendingChange::Removal,
            due_at: previous + STATUS_TRANSITION_DELAY,
        }
    }

    /// The record's removal from the store, due [`STATUS_TRANSITION_DELAY`] from now.
    pub fn removal_in_a_moment() -> Self {
        Self {
            change: PendingChange::Removal,
            due_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }
}

/// The provisioning transition a NEW resource is stored with, and the instant it makes that
/// resource ready: `(pending_status, ready_at)`.
///
/// `source_ready_at` is the instant the resource this one is PROVISIONED FROM became ready —
/// the DB instance a snapshot is taken of, the snapshot a copy is made of, the DB cluster a
/// member joins — or `None` for a resource this service provisions from nothing (a new DB
/// instance, a new DB cluster).
///
/// The span is charged once per DEPENDENCY LEVEL rather than once per call: a resource that
/// waits on another one is ready [`STATUS_TRANSITION_DELAY`] after THAT one was, so a chain
/// of derived resources is ready one span per level after its root was created, however long
/// after each other the calls that asked for them arrive. That is what lets a client drive a
/// whole chain with a poll loop: by the time it has polled one level to its terminal status —
/// which costs it at least one poll of its own — the next level's span has already elapsed,
/// so a resource created from a settled source is reported ready by the first read after the
/// create rather than one poll interval later. A caller that creates the source and the
/// derivative back to back still sees the derivative in its transient status, and the state
/// errors that go with it, for the rest of that span.
pub fn provisioning_to(status: &str, source_ready_at: Option<Instant>) -> (PendingStatus, Instant) {
    let pending = match source_ready_at {
        Some(ready_at) => PendingStatus::in_a_moment_after(ready_at, status),
        None => PendingStatus::in_a_moment(status),
    };
    let ready_at = pending.due_at;
    (pending, ready_at)
}

// ---------------------------------------------------------------------------
// Cluster parameter groups
//
// A cluster parameter group is a named, family-scoped set of engine parameters that
// `CreateDBClusterParameterGroup` mints, `ModifyDBClusterParameterGroup` writes into,
// `ResetDBClusterParameterGroup` clears, the two describes report and
// `DeleteDBClusterParameterGroup` removes. The group carries the parameters a CALLER has
// set; the engine's own defaults belong to the FAMILY, which publishes them
// (`published_parameters`), and are not materialised into a group — which is why a freshly
// created group reports an EMPTY `Parameters` list.
//
// This is the control plane an AWS SDK Amazon RDS, Amazon DocumentDB or Amazon Neptune
// client reaches: all three sign `rds`, which resolves to this service (see
// `src/service.rs`), exactly as all three APIs are served on the RDS endpoint by AWS.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS bound: the longest caller-supplied free-form string
/// this service stores for a member whose length the API publishes no limit for (a
/// parameter group's `Description` or `DBParameterGroupFamily`, a parameter's name or
/// value). Nothing caller-sized reaches the store unchecked.
pub const MAX_FREE_FORM_LEN: usize = 512;

/// Refuse a caller-supplied free-form string longer than [`MAX_FREE_FORM_LEN`].
pub fn check_free_form(member: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length > MAX_FREE_FORM_LEN {
        return Err(format!(
            "{member} may be at most {MAX_FREE_FORM_LEN} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse an EMPTY value for a member the model marks required.
///
/// A required member carries a value, and the empty string is no value — so Amazon RDS
/// refuses such a request rather than storing a resource whose required member says
/// nothing. The one sentence it answers that with names the member, which is what the
/// caller has to fix, and holds for every required member a request can leave empty, so it
/// lives here next to the other free-form rules rather than in each create.
///
/// The test is emptiness, not blankness: a value made of whitespace is a value the caller
/// chose, and it is stored and reported back exactly as sent, like any other.
///
/// It is not the model's own missing-member refusal either: a member that is ABSENT is
/// refused at the boundary with the framework's text, and a member that is PRESENT and
/// empty is this one.
pub fn check_not_empty(member: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!(
            "The parameter {member} must be provided and must not be blank."
        ));
    }
    Ok(())
}

/// The message a `DBShardGroupNotFoundFault` carries where the call only REPORTS the DB
/// shard groups: "the specified DB shard group name wasn't found", in the
/// `<resource type> <name> not found.` wording this front door answers a named resource it
/// cannot resolve with.
///
/// `named` is the value the REQUEST carried, not the key the store folded it to, so a
/// caller matching the message against what it sent finds it there — the same reason
/// [`db_cluster_not_found_message`] names the request's spelling.
///
/// The calls that ACT on the shard group word the same fault differently, as Amazon RDS
/// does: see [`DB_SHARD_GROUP_DOES_NOT_EXIST_MESSAGE`] and
/// [`DB_SHARD_GROUP_NOT_ACCESSIBLE_MESSAGE`].
pub fn db_shard_group_not_found_message(named: &str) -> String {
    format!("DBShardGroup {named} not found.")
}

/// The sentence a `DBShardGroupNotFoundFault` carries where the call MODIFIES the shard
/// group's configuration.
///
/// It has NO variable parts: Amazon RDS interpolates neither the identifier the request
/// carried nor the account it was signed for — it says only that the pair named nothing —
/// so this is a constant rather than a `format!` over the request, and a caller matching
/// the sentence sees exactly what the real service sends.
pub const DB_SHARD_GROUP_DOES_NOT_EXIST_MESSAGE: &str =
    "The DB shard group with requested identifier in requested account doesn't exist.";

/// The sentence a `DBShardGroupNotFoundFault` carries where the call acts on the shard
/// group as a whole — the delete that removes it and the reboot that restarts it.
///
/// Amazon RDS words this one as a statement about reachability rather than about the
/// identifier, with no variable part and no terminal period, so it too is a constant.
pub const DB_SHARD_GROUP_NOT_ACCESSIBLE_MESSAGE: &str =
    "DB shard group not found or can not be accessed";

/// The sentence a `DBClusterNotFoundFault` carries where the DB cluster is a cluster the
/// request names for a DB SHARD GROUP to hang off.
///
/// Unlike the wordings above it names NO cluster: the Aurora Limitless Database calls
/// report only that the primary cluster the request named does not exist, so nothing from
/// the request is interpolated into it.
pub const DB_CLUSTER_DOES_NOT_EXIST_MESSAGE: &str = "The DB cluster doesn't exist.";

/// The message a `DBClusterNotFoundFault` carries where the call LOOKS a DB cluster UP for
/// its own sake — it reports the cluster, or resolves one it reads something else out of —
/// as the API that call belongs to words the fault.
///
/// A call that resolves the cluster as the SOURCE another resource stands on names it the
/// other way ([`db_cluster_cannot_be_accessed_message`]), and the calls of the BACKTRACK
/// api name it no way at all ([`BACKTRACK_DB_CLUSTER_NOT_FOUND_MESSAGE`]), so the wording
/// follows the API the call belongs to rather than whether it reads or writes.
///
/// `named` is the value the REQUEST carried, not the key the store folded it to: the
/// members that name a DB cluster accept "the user-supplied DB cluster identifier or the
/// Amazon Resource Name (ARN)", and the fault names the one the caller wrote, in the
/// spelling they wrote it in, so a caller matching the message against what it sent finds
/// it there.
pub fn db_cluster_not_found_message(named: &str) -> String {
    format!("DBCluster {named} not found.")
}

/// The sentence a `DBClusterNotFoundFault` carries where the call MODIFIES the DB
/// cluster's own configuration.
///
/// Like the shard group's modification wording
/// ([`DB_SHARD_GROUP_DOES_NOT_EXIST_MESSAGE`]) it has NO variable part: Amazon RDS reports
/// only that the cluster the request named is not there, and interpolates neither the
/// identifier the caller wrote nor the account the call was signed for. So it is a constant
/// rather than a `format!` over the request, which also keeps the sentence's cardinality at
/// one — a client grouping or alerting on this error class sees one text, not one per
/// identifier.
pub const MODIFY_DB_CLUSTER_NOT_FOUND_MESSAGE: &str = "DBCluster not found";

/// The message a `DBClusterNotFoundFault` carries where the call acts on the cluster itself
/// — the delete that removes it, and the failover that moves its primary — and where it
/// resolves the cluster as the SOURCE another resource stands on: the cluster a DB instance
/// joins, a global cluster or a point-in-time restore is built from, an export is taken of,
/// or a CUSTOM ENDPOINT hangs off, whether that call creates the dependent resource or only
/// reports it.
///
/// This front door words the one fault by the ROLE the cluster plays in the call, the same
/// way it words a DB subnet group's ([`db_subnet_group_not_found_message`] beside
/// [`db_subnet_group_does_not_exist_message`]), so the wording lives here rather than in
/// each handler. `named` is the value the request carried, as in
/// [`db_cluster_not_found_message`].
pub fn db_cluster_cannot_be_accessed_message(named: &str) -> String {
    format!("The source cluster could not be found or cannot be accessed: {named}")
}

/// The sentence a `DBClusterNotFoundFault` carries across the BACKTRACK api: the call that
/// performs a backtrack on the cluster and the one that reports the backtracks performed on
/// it.
///
/// That api words this fault as a statement about the lookup itself, with no variable part
/// at all — neither the identifier the caller wrote nor the account the call was signed for
/// — so it is a constant, exactly as the modification's wording
/// ([`MODIFY_DB_CLUSTER_NOT_FOUND_MESSAGE`]) is, and every call of the api answers the one
/// sentence rather than the per-identifier wording the other cluster readers use
/// ([`db_cluster_not_found_message`]).
pub const BACKTRACK_DB_CLUSTER_NOT_FOUND_MESSAGE: &str = "Could not find specified DB Cluster";

/// Another wording this front door gives a `DBClusterNotFoundFault`: the one the call that
/// takes a SNAPSHOT of the cluster answers, and the one the calls that manage the cluster's
/// IAM ROLE ASSOCIATIONS answer — the `<resource type> not found: <named>` form their DB
/// instance siblings use for the same association calls.
///
/// One fault, a wording per ROLE the cluster plays in the call (see
/// [`db_cluster_not_found_message`] for a call that REPORTS the cluster or stops it,
/// [`db_cluster_cannot_be_accessed_message`] for one that acts on the cluster itself, and
/// [`MODIFY_DB_CLUSTER_NOT_FOUND_MESSAGE`] for one that modifies its configuration), so
/// each operation answers with the one its own API uses rather than with a single house
/// style. `named` is the value the request carried, as in the other two that name one.
pub fn db_cluster_not_found_colon_message(named: &str) -> String {
    format!("DBCluster not found: {named}")
}

/// The message `ModifyDBCluster` answers an `InvalidDBClusterStateFault` with: the sentence
/// Amazon RDS words for a modification it will not take because the cluster is not in a state
/// the operation can be performed in — the case a second modification arriving while the
/// first is still applying lands in, and the one a call that arrives before the cluster has
/// finished coming up lands in.
///
/// It names the STATUS the cluster is in, which is what tells the caller whether waiting will
/// clear the refusal, and nothing else: RDS interpolates neither the cluster identifier the
/// request carried nor the modification it asked for. So the status, read from the stored
/// record, is the one variable part.
pub fn db_cluster_not_available_for_modification_message(status: &str) -> String {
    format!("DB cluster isn't available for modification with status {status}.")
}

/// The message `PromoteReadReplicaDBCluster` answers an `InvalidDBClusterStateFault` with for
/// a cluster that is no read replica at all: one created without a replication source, or one
/// an earlier promotion already made standalone.
///
/// Like the modification's sentence above it has NO variable parts — Amazon RDS states only
/// the precondition the call has, interpolating neither the identifier the request carried nor
/// the cluster's status — so it is a constant rather than a `format!` over the request, and a
/// caller matching on the text to tell this refusal from the status refusal of the same fault
/// sees exactly what the real service sends.
pub const PROMOTE_NOT_A_READ_REPLICA_MESSAGE: &str =
    "You cannot promote a DB cluster that is not a read replica.";

/// The sentence every `DBClusterAlreadyExistsFault` of this service carries: the answer to
/// a request that would put a second DB cluster under an identifier the region already
/// holds — the create that names a stored identifier, and the modification that renames a
/// cluster onto one.
///
/// Like its DB instance sibling ([`DB_INSTANCE_ALREADY_EXISTS_MESSAGE`]) it has NO variable
/// part — Amazon RDS states only that the cluster exists — and it carries no terminal
/// period. The capitalisation is the service's own.
pub const DB_CLUSTER_ALREADY_EXISTS_MESSAGE: &str = "DB Cluster already exists";

/// How many DB instances a DB cluster has to hold for a failover to be accepted.
///
/// A failover moves the writer role from one member of the cluster to another, so there has
/// to be a member to move it TO as well as one to move it from: a cluster holding fewer is
/// refused with [`failover_needs_two_instances_message`].
pub const MIN_FAILOVER_DB_CLUSTER_INSTANCES: usize = 2;

/// The message `FailoverDBCluster` answers an `InvalidDBClusterStateFault` with when the
/// cluster holds fewer than [`MIN_FAILOVER_DB_CLUSTER_INSTANCES`] DB instances.
///
/// Amazon RDS states the precondition as the COUNT the cluster has to reach, and names the
/// cluster straight after the colon with no space before it.
pub fn failover_needs_two_instances_message(identifier: &str) -> String {
    format!(
        "Database cluster:{identifier} should have at least two database instances for \
         failover."
    )
}

// The message an `InvalidDBParameterGroupStateFault` carries for a reset refused because a
// parameter of the group still carries a change that has not been applied is the shared
// `service_common::rds_cluster_parameters::parameter_has_pending_changes_message` — one
// sentence for every service view of this front door.

/// The name AWS gives the DB subnet group it creates alongside an account's default
/// VPC. `CreateDBSubnetGroup$DBSubnetGroupName` refuses it ("Must not be default")
/// because the service owns that name, and `CreateDBInstance$PubliclyAccessible`
/// documents a default that branches on a request naming it.
pub const DEFAULT_DB_SUBNET_GROUP: &str = "default";

/// The message a `CreateDBSubnetGroup` naming the one name this member reserves is refused
/// with ("Must not be default"): the group of that name is the one AWS provides alongside the
/// account's default VPC, so RDS states that the name is taken and what to do about it.
///
/// Amazon RDS words it as a statement about the name the request carried, which is the only
/// variable part — echoed as the request spelled it, the way the member's other refusal
/// echoes a value ([`check_db_subnet_group_name`]) — followed by the fixed instruction.
///
/// It lives here, beside the name the sentence is about, so the create that answers it and
/// any later caller word it the same way.
pub fn db_subnet_group_name_reserved_message(name: &str) -> String {
    format!("Subnet group name {name} is reserved. Please specify another name.")
}

/// The message the `DBSubnetGroupNotFoundFault` carries for `name` where the call LOOKS the
/// group UP in order to REPORT it (`DescribeDBSubnetGroups`, a tag read). The fault names
/// the group the request could not resolve, in the folded spelling the group would be
/// stored under.
///
/// Amazon RDS words this one fault by the CALL that answers it, so the report's wording
/// lives here, next to the wording the calls that ACT on a group answer
/// ([`db_subnet_group_does_not_exist_message`]), rather than in each handler.
///
/// The calls that PLACE A DB INSTANCE in the group answer a third wording of the same
/// fault ([`db_subnet_group_not_found_for_db_instance_message`]).
pub fn db_subnet_group_not_found_message(name: &str) -> String {
    format!("DB Subnet group '{name}' not found.")
}

/// The message the `DBSubnetGroupNotFoundFault` carries where the group is the one the call
/// PLACES A DB INSTANCE in: the create, the read replica built from another instance, and
/// the restore that brings an instance back from a snapshot all resolve
/// `DBSubnetGroupName` for the same purpose and answer the same sentence.
///
/// Amazon RDS words this one as a statement about the group, spelling the resource type the
/// way the modelled shape spells it — `DBSubnetGroup`, with no internal space — and quoting
/// the group in single quotes. It is a third wording of the one fault, beside the report's
/// ([`db_subnet_group_not_found_message`]) and the one the calls whose SUBJECT is the group
/// answer ([`db_subnet_group_does_not_exist_message`]), which is how this front door words a
/// fault by the ROLE the resource plays in the call.
///
/// Like both of those it names the group in the folded spelling it would be stored under
/// ("This value is stored as a lowercase string"), which is the spelling every operation
/// resolves the member in.
pub fn db_subnet_group_not_found_for_db_instance_message(name: &str) -> String {
    format!("DBSubnetGroup '{name}' not found.")
}

/// The message the `DBSubnetGroupNotFoundFault` carries where the group is the group the
/// call ACTS on: `DeleteDBSubnetGroup` and `ModifyDBSubnetGroup`, whose whole effect is on
/// that group, and `CreateDBCluster`, which places the cluster it creates in it. RDS words
/// the fault differently there than in the report ([`db_subnet_group_not_found_message`]),
/// so a caller can tell "there is no such group to report" from "there is no such group to
/// place anything in"; both still answer the one modelled 404 `DBSubnetGroupNotFoundFault`.
///
/// Like the report's wording it is parameterized by the requested name, in the folded
/// spelling the group would be stored under.
pub fn db_subnet_group_does_not_exist_message(name: &str) -> String {
    format!("DB subnet group '{name}' does not exist.")
}

/// The message the `DBSubnetGroupAlreadyExistsFault` carries for `name`: the fault reports
/// "`DBSubnetGroupName` is already used by an existing DB subnet group", and RDS words it
/// as a sentence about the group — the resource type, the requested name in quotes, then
/// "already exists." — the same shape as the two wordings above. The name is the only
/// variable part, quoted as the REQUEST spelled it: the sentence is about the name the
/// caller asked for, not about the folded key the collision was found under.
pub fn db_subnet_group_already_exists_message(name: &str) -> String {
    format!("The DB subnet group '{name}' already exists.")
}

/// The message the `InvalidDBSubnetGroupStateFault` carries where a DB subnet group cannot
/// be deleted because a database is still placed in it ("The specified database subnet
/// group must not be associated with any DB instances").
///
/// Amazon RDS words this one sentence from the request and from the blocking resource: the
/// group name the request asked to delete, quoted, the KIND of database that still holds it
/// ([`DbSubnetGroupHolder::noun`]) and that database's identifier. It lives here, beside the
/// not-found wordings, so the one operation that answers it and any later one word it the
/// same way.
///
/// The noun comes from the record the association was found on rather than being fixed:
/// the sentence tells the caller what has to be deleted before the group can be, so naming
/// a DB instance where a DB cluster is the holder would name the wrong resource.
pub fn db_subnet_group_in_use_message(
    name: &str,
    holder: DbSubnetGroupHolder,
    holder_identifier: &str,
) -> String {
    format!(
        "Cannot delete the subnet group '{name}' because at least one {}: \
         {holder_identifier} is still using it.",
        holder.noun()
    )
}

/// The KIND of database that can hold a DB subnet group, as the in-use refusal names it.
///
/// Both records of this front door that carry a `DBSubnetGroupName` place a database in the
/// group — a DB instance and a DB cluster — and the refusal
/// ([`db_subnet_group_in_use_message`]) names the one it found, so the two kinds are a type
/// rather than a string a handler spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSubnetGroupHolder {
    /// A DB instance created with `DBSubnetGroupName` set to the group.
    DbInstance,
    /// A DB cluster created with `DBSubnetGroupName` set to the group.
    DbCluster,
}

impl DbSubnetGroupHolder {
    /// How the in-use sentence names this kind of database.
    pub fn noun(self) -> &'static str {
        match self {
            Self::DbInstance => "database instance",
            Self::DbCluster => "database cluster",
        }
    }
}

/// A stored DB subnet group: the name, the description and the EC2 subnets the request
/// named.
///
/// It exists so a request that NAMES a DB subnet group can be resolved: an instance
/// created against a group that was never created is a reference to nothing, which the
/// real service refuses rather than accepts. The ARN is computed from the stored region
/// and name, like the other records', so it cannot drift from the record.
#[derive(Debug, Clone)]
pub struct DbSubnetGroupRecord {
    pub name: String,
    /// The region the group was created in; the scope of its ARN.
    pub region: String,
    pub description: String,
    /// The EC2 subnet ids the request named, in the order it named them.
    pub subnet_ids: Vec<String>,
    /// The Availability Zone of each of [`Self::subnet_ids`], as Amazon EC2 — the service
    /// that owns subnets — reported it when the group was created or last modified
    /// ([`crate::state::subnet_zones`]).
    ///
    /// A subnet EC2 answered for no zone (one the account does not have, or one asked about
    /// where EC2 is not mounted) is simply absent: the group then reports no zone for that
    /// subnet rather than one this service chose.
    pub subnet_zones: BTreeMap<String, String>,
    /// The VPC that OWNS [`Self::subnet_ids`], as Amazon EC2 reported it when the group was
    /// created or last modified: a subnet belongs to one VPC, and that VPC is the group's.
    ///
    /// `None` where the owner of subnets answered no VPC for them at all (a deployment EC2
    /// is not mounted in), in which case the group reports the VPC its region resolves into
    /// ([`region_vpc_id`]) rather than claiming a VPC EC2 never named.
    pub vpc_id: Option<String>,
    pub tags: Vec<Tag>,
}

impl DbSubnetGroupRecord {
    /// The account's `default` DB subnet group in `region`: the group the real service
    /// provides with the account's default VPC, which no request creates and which
    /// therefore is not stored.
    ///
    /// The group's NETWORK — the default VPC and its default subnets — belongs to the
    /// service that owns subnets, so this bare group names none, and
    /// [`ComAmazonawsRdsAmazonrdsv19State::account_default_db_subnet_group`] is what fills
    /// it in from what EC2 answered ([`default_subnet_group`]). Every read of the group
    /// goes through that store method; this constructor is the shape they share.
    pub fn account_default(region: &str) -> Self {
        Self {
            name: DEFAULT_DB_SUBNET_GROUP.to_string(),
            region: region.to_string(),
            description: DEFAULT_DB_SUBNET_GROUP.to_string(),
            subnet_ids: Vec::new(),
            subnet_zones: BTreeMap::new(),
            vpc_id: None,
            tags: Vec::new(),
        }
    }

    /// `arn:aws:rds:<region>:<account>:subgrp:<name>` — `subgrp` is the resource type
    /// RDS gives a DB subnet group in its ARN format.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("subgrp:{}", self.name),
        )
    }

    /// Project the stored record onto the modelled `DBSubnetGroup` output shape.
    pub fn to_wire(&self) -> DBSubnetGroup {
        DBSubnetGroup {
            db_subnet_group_name: Some(self.name.clone()),
            db_subnet_group_description: Some(self.description.clone()),
            subnet_group_status: Some("Complete".to_string()),
            // A group that names no subnet reports NO `Subnets` member rather than an
            // empty list: an empty list would claim the group covers nothing, which is
            // a different statement from "this service does not hold its subnets".
            subnets: (!self.subnet_ids.is_empty()).then(|| {
                self.subnet_ids
                    .iter()
                    .map(|id| Subnet {
                        subnet_identifier: Some(id.clone()),
                        subnet_status: Some("Active".to_string()),
                        // The Availability Zone a subnet lies in is an EC2 fact RDS reads
                        // off the subnet, so it is the zone the owner of subnets answered
                        // with for this very id ([`Self::subnet_zones`]) and never a value
                        // built here. A subnet EC2 answered for no zone reports NONE: the
                        // member is omitted rather than filled with a zone the subnet may
                        // not be in.
                        subnet_availability_zone: self.subnet_zones.get(id).map(|zone| {
                            AvailabilityZone {
                                name: Some(zone.clone()),
                            }
                        }),
                        // `SubnetOutpost` describes a subnet on an Outpost. Nothing here
                        // is on one, so the element is reported EMPTY — which says the
                        // subnet is not associated with an Outpost — rather than omitted.
                        subnet_outpost: Some(Outpost::default()),
                    })
                    .collect()
            }),
            db_subnet_group_arn: Some(self.arn()),
            // The VPC the group's subnets belong to, and the network protocols those
            // subnets carry: both read off the subnets, as RDS reads them. The VPC is the
            // one the owner of subnets answered with for this group's own subnets; where it
            // answered none, the VPC the region resolves into stands in.
            vpc_id: Some(
                self.vpc_id
                    .clone()
                    .unwrap_or_else(|| region_vpc_id(&self.region)),
            ),
            supported_network_types: Some(
                SUBNET_GROUP_NETWORK_TYPES
                    .iter()
                    .map(|network_type| (*network_type).to_string())
                    .collect(),
            ),
        }
    }
}

/// The message a `DBSecurityGroupNotFoundFault` carries for `name`: the fault reports
/// "`DBSecurityGroupName` doesn't refer to an existing DB security group", naming the
/// group the request could not resolve in the folded spelling it would be stored under.
///
/// Amazon RDS words this family's fault as a SENTENCE about the group — the resource type,
/// the name, then "not found." — rather than as the `<type> not found: <name>` form it uses
/// for a parameter group, so the name goes inside the sentence.
///
/// It lives here rather than in one handler because every operation that resolves a
/// `DBSecurityGroupName` answers the same one fault, and the wording is what a caller
/// reads to tell which group was missing.
pub fn db_security_group_not_found_message(name: &str) -> String {
    format!("DBSecurityGroup {name} not found.")
}

/// The message a `DBSecurityGroupNotFoundFault` carries where the DB security group is the
/// SUBJECT of the call and the call acts on the group itself — the delete that removes it,
/// and the two ingress calls that change the rules it holds.
///
/// This front door words the one fault by the ROLE the resource plays in the call, the same
/// way it words a DB cluster's ([`db_cluster_not_found_message`] beside
/// [`db_cluster_not_found_colon_message`]) and a DB subnet group's
/// ([`db_subnet_group_not_found_message`] beside
/// [`db_subnet_group_does_not_exist_message`]), so a caller can tell "there is no such
/// group to report" from "there is no such group to change"; both still answer the one
/// modelled 404. Like the lookup wording it names the group in the folded spelling the
/// group would be stored under.
pub fn db_security_group_not_found_colon_message(name: &str) -> String {
    format!("DBSecurityGroup not found: {name}")
}

/// The name AWS keeps for the DB security group of the account itself.
/// `DBSecurityGroupName` refuses it on every operation that states the constraint ("Must
/// not be `Default`", and "You can't delete the default DB security group"), compared after
/// the value is folded the way that member documents it is stored.
///
/// It lives here because the create and the delete refuse the same one name, so neither can
/// drift from the other.
pub const DEFAULT_DB_SECURITY_GROUP: &str = "default";

/// The message the `InvalidParameterValue` refusal carries for a create that asks for the
/// one name the service keeps for itself ([`DEFAULT_DB_SECURITY_GROUP`]).
///
/// Amazon RDS words it as a statement to the caller about what cannot be named, naming the
/// reserved name unquoted and nothing of the request: the request's own spelling folds to
/// that name, so there is nothing else to report.
pub fn db_security_group_reserved_name_message() -> String {
    format!("You cannot name a DB security group {DEFAULT_DB_SECURITY_GROUP}")
}

/// The longest `DBSecurityGroupName` this service stores, from the length the member's own
/// documentation states ("Must be 1 to 255 letters, numbers, or hyphens").
pub const MAX_DB_SECURITY_GROUP_NAME_LEN: usize = 255;

/// Refuse a `DBSecurityGroupName` this service cannot STORE — and nothing else.
///
/// The member is a plain string in this model: it carries no `@pattern` and no `@length`
/// (`smithy_query(query="operation",operation="CreateDBSecurityGroup")`), and a group whose
/// name breaks the alphabet-and-hyphen clauses of the identifier rule is created and
/// reported by the real service, so that rule — the one the DB instance, cluster, snapshot
/// and parameter-group names state ([`check_identifier`]) — is NOT this member's and must
/// not be borrowed for it. What a value entering the store is still checked for is what
/// every stored caller-sized value is: that the required member carries a value at all
/// ([`check_not_empty`]) and that it is no longer than the length its own documentation
/// states ([`MAX_DB_SECURITY_GROUP_NAME_LEN`]).
///
/// It lives here because every operation that takes a DB security group NAME — the create,
/// and the membership lists that name groups by it — applies the one rule.
pub fn check_db_security_group_name(member: &str, value: &str) -> Result<(), String> {
    check_not_empty(member, value)?;
    check_stored_text(member, value, MAX_DB_SECURITY_GROUP_NAME_LEN)
}

/// [`check_db_security_group_name`] over a LIST of DB security group names, bounded by the
/// same count cap as the other security group membership lists
/// ([`MAX_OPTION_SECURITY_GROUPS`]).
pub fn check_db_security_group_names(member: &str, values: &[String]) -> Result<(), String> {
    check_request_list_len(member, values.len(), MAX_OPTION_SECURITY_GROUPS)?;
    for value in values {
        check_db_security_group_name(member, value)?;
    }
    Ok(())
}

/// The message the `InvalidDBSecurityGroupStateFault` carries where a DB security group
/// cannot be deleted because a DB instance is still a member of it ("The specified DB
/// security group must not be associated with any DB instances").
///
/// Worded from the request and from the blocking resource — the group the request asked to
/// delete, in the folded spelling it is stored under, and the identifier of the DB instance
/// that still names it — like the sibling in-use wording of this service's other group
/// families. It lives here, beside the not-found wording, so the operation that answers it
/// and any later one word it the same way.
pub fn db_security_group_in_use_message(name: &str, holder_identifier: &str) -> String {
    format!(
        "The DB security group {name} can't be deleted because it is in use by DB instance \
         {holder_identifier}"
    )
}

/// The message the `DBSecurityGroupAlreadyExistsFault` carries for `name`: the fault
/// reports "A DB security group with the name specified in `DBSecurityGroupName` already
/// exists".
///
/// Amazon RDS words it as a sentence about the group — "A security group named", the
/// requested name unquoted, then "already exists" with no terminal period — so the name
/// sits inside the sentence rather than after a colon. The name is the only variable part,
/// in the folded spelling the group would be stored under, as that member documents
/// ("stored as a lowercase string").
pub fn db_security_group_already_exists_message(name: &str) -> String {
    format!("A security group named {name} already exists")
}

/// The refusal this API version answers a request that would MODIFY the ingress rules of a
/// VPC DB security group with.
///
/// Every DB security group of this account is VPC-backed: the ingress operations' own
/// documentation records that "EC2-Classic was retired on August 15, 2022", so there is no
/// other network a group could belong to, and the two ingress APIs this service publishes
/// are the EC2-Classic-era ones — they cannot modify a VPC group, and say which API
/// versions can. Fixed text: it names neither the group nor the rule, so there is nothing
/// of the request in it.
///
/// It lives here rather than in one handler because `AuthorizeDBSecurityGroupIngress` and
/// `RevokeDBSecurityGroupIngress` are the same refusal — both modify a group's rules — and
/// a caller branching on one must read the same answer from the other.
pub const VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE: &str =
    "VPC DB Security Groups cannot be modified with this API version.  Please use an API \
     version between 2012-01-15 and 2012-10-31 to modify this group.";

/// A stored DB security group: the name and description a create gave it, and the ingress
/// rules authorized on it.
///
/// `CreateDBSecurityGroup` inserts the record and `DescribeDBSecurityGroups` reports it,
/// both through [`Self::to_wire`], so one group reads the same way whichever call reports
/// it. The ARN is computed from the stored region and name, like the other records', so it
/// cannot drift from the record.
#[derive(Debug, Clone, Default)]
pub struct DbSecurityGroupRecord {
    /// `DBSecurityGroupName`, folded to lowercase — the record's key within its region.
    pub name: String,
    /// The region the group was created in; the scope of its ARN.
    pub region: String,
    /// `DBSecurityGroupDescription`.
    pub description: String,
    /// The CIDR ranges authorized on the group, in the order they were authorized.
    ///
    /// Written by the operations that authorize and revoke a CIDR rule, which are still
    /// scaffolded in this crate (each its own stub issue); a group a create just stored
    /// holds none, and [`Self::to_wire`] reports exactly what is here rather than a canned
    /// list.
    pub ip_ranges: Vec<IPRange>,
    /// The EC2 security groups authorized on the group, in the order they were
    /// authorized — written by the same operations as [`Self::ip_ranges`].
    pub ec2_security_groups: Vec<EC2SecurityGroup>,
    /// The tags the group carries: the ones its create assigned, plus whatever the tag
    /// operations have written since.
    pub tags: Vec<Tag>,
}

impl DbSecurityGroupRecord {
    /// `arn:aws:rds:<region>:<account>:secgrp:<name>` — `secgrp` is the resource type RDS
    /// gives a DB security group in its ARN format.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("secgrp:{}", self.name),
        )
    }

    /// How many ingress rules the group holds: both forms an authorization can take count
    /// toward the one published allowance,
    /// [`MAX_AUTHORIZATIONS_PER_DB_SECURITY_GROUP`].
    pub fn authorization_count(&self) -> usize {
        self.ip_ranges.len() + self.ec2_security_groups.len()
    }

    /// Project the stored record onto the modelled `DBSecurityGroup` output shape.
    pub fn to_wire(&self) -> DBSecurityGroup {
        DBSecurityGroup {
            // "Provides the Amazon Web Services ID of the owner of a specific DB security
            // group" — the account every resource of this emulator belongs to.
            owner_id: Some(emulator_server::emulator_account_id().to_string()),
            db_security_group_name: Some(self.name.clone()),
            db_security_group_description: Some(self.description.clone()),
            // "Provides the VpcId of the DB security group": every DB security group of
            // this account belongs to a VPC — "EC2-Classic was retired", which is the only
            // network a group could have belonged to otherwise — so the group reports the
            // VPC its region resolves into ([`region_vpc_id`]), the same VPC a DB subnet
            // group of that region reports, rather than nothing at all.
            vpc_id: Some(region_vpc_id(&self.region)),
            // Both rule lists are reported even when empty: a group really holds no
            // ingress rule until one is authorized, which is a different statement from
            // "this service does not know its rules".
            ec2_security_groups: Some(self.ec2_security_groups.clone()),
            ip_ranges: Some(self.ip_ranges.clone()),
            db_security_group_arn: Some(self.arn()),
        }
    }
}

/// The `DBClusterSnapshotAttributesResult` a snapshot's `restore` attribute projects onto.
///
/// `restore` is the one attribute AWS defines for a cluster snapshot, and it is always
/// reported: a snapshot nobody has been authorized for carries the attribute with an
/// empty value list, which is how a caller reads "shared with no account". Both
/// `DescribeDBClusterSnapshotAttributes` and `ModifyDBClusterSnapshotAttribute` answer
/// with it, so neither can report it differently from the other.
pub fn restore_attribute_to_wire(
    record: &DbClusterSnapshotRecord,
) -> DBClusterSnapshotAttributesResult {
    DBClusterSnapshotAttributesResult {
        db_cluster_snapshot_identifier: Some(record.snapshot_identifier.clone()),
        db_cluster_snapshot_attributes: Some(vec![DBClusterSnapshotAttribute {
            attribute_name: Some(SNAPSHOT_RESTORE_ATTRIBUTE.to_string()),
            attribute_values: Some(record.restore_attribute.iter().cloned().collect()),
        }]),
    }
}

/// The values a `restore` attribute request really authorizes: `values` less the account
/// that OWNS the snapshot.
///
/// `restore` is "the list of Amazon Web Services accounts that have permission to copy or
/// restore the manual DB cluster snapshot", and the way to grow it is "to share a manual DB
/// cluster snapshot with OTHER Amazon Web Services accounts … use the ValuesToAdd parameter
/// to add a list of IDs of the Amazon Web Services accounts that are authorized to restore
/// the manual DB cluster snapshot". An account already copies and restores its own
/// snapshots by owning them, so it is never a grantee of one: naming it is ACCEPTED — there
/// is nothing wrong with the request — and grants nothing, so the value is not stored and
/// the attribute every later read reports is the one the snapshot already had.
///
/// Every snapshot this emulator holds belongs to the one account it serves
/// ([`emulator_server::emulator_account_id`]), which is therefore the owner of the snapshot
/// a request addresses. One rule for both snapshot kinds, so the DB snapshot's attribute
/// call and the DB cluster snapshot's cannot drift apart.
pub fn snapshot_attribute_grantees(values: &[String]) -> impl Iterator<Item = &str> {
    let owner = emulator_server::emulator_account_id();
    values
        .iter()
        .map(String::as_str)
        .filter(move |value| *value != owner)
}

/// Refuse a `restore` attribute value that is neither an Amazon Web Services account id
/// nor the `all` that makes a snapshot public; see [`SNAPSHOT_ATTRIBUTE_ALL`] and
/// [`ACCOUNT_ID_DIGITS`] for the documentation this comes from.
pub fn check_snapshot_attribute_value(member: &str, value: &str) -> Result<(), String> {
    let well_formed = value == SNAPSHOT_ATTRIBUTE_ALL
        || (value.chars().count() == ACCOUNT_ID_DIGITS && value.chars().all(|c| c.is_ascii_digit()));
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "invalid value '{value}' for {member}: each value must be a \
         {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account id or \
         '{SNAPSHOT_ATTRIBUTE_ALL}'"
    ))
}

/// The `SnapshotType` values `DescribeDBClusterSnapshotsMessage$SnapshotType` lists:
/// "You can specify one of the following values: automated ... manual ... shared ...
/// public". A request naming anything else is refused rather than silently matching
/// nothing.
pub const SNAPSHOT_TYPES: [&str; 4] = ["automated", "manual", "shared", "public"];

/// Refuse a `SnapshotType` outside the documented list; see [`SNAPSHOT_TYPES`].
pub fn check_snapshot_type(value: &str) -> Result<(), String> {
    if SNAPSHOT_TYPES.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "invalid value '{value}' for SnapshotType: valid values are {}",
        SNAPSHOT_TYPES.join(", ")
    ))
}

/// Refuse a value that is not of the published EC2 subnet id form: `subnet-` followed
/// by 8 or 17 lowercase hexadecimal characters. A value that is not a subnet id at all
/// names no subnet, which is the modelled `InvalidSubnet`.
pub fn check_subnet_id(subnet_id: &str) -> Result<(), String> {
    let suffix = subnet_id.strip_prefix("subnet-").unwrap_or_default();
    if (suffix.len() == 8 || suffix.len() == 17)
        && suffix
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Ok(());
    }
    Err(format!(
        "'{subnet_id}' is not an EC2 subnet id (expected subnet- followed by 8 or 17 \
         hexadecimal characters)"
    ))
}

/// Why a `SubnetIds` list a request carries cannot make up a DB subnet group. Each
/// variant carries the message naming the constraint it broke; the caller maps it onto
/// the modelled fault of ITS operation, which is the one thing that differs between the
/// operations that take such a list.
#[derive(Debug, Clone)]
pub enum SubnetListRejection {
    /// More subnets than one group holds — `DBSubnetQuotaExceededFault`.
    TooManySubnets(String),
    /// A list that names no subnet at all, or a value that is not an EC2 subnet id —
    /// `InvalidSubnet`.
    NotASubnet(String),
    /// The named subnets cover fewer Availability Zones than a group must —
    /// `DBSubnetGroupDoesNotCoverEnoughAZs`.
    NotEnoughAvailabilityZones(String),
    /// An identifier the request named that cannot make up this group: one the account
    /// holds no subnet for, or one of a VPC other than the group's. Either way the VALUE
    /// the request carried is the thing that cannot be used, which is the query protocol's
    /// common `InvalidParameterValue`.
    UnusableSubnets(String),
    /// The named subnets are not all in one VPC, so they make up no DB subnet group —
    /// `InvalidSubnet`, whose documented condition this is.
    NotInOneVpc(String),
}

/// Refuse a `SubnetIds` list THE REQUEST ALONE settles cannot make up a DB subnet group: a
/// list that names no subnet at all ([`EMPTY_SUBNET_LIST_MESSAGE`]), one past the published
/// allowance for ONE group ([`MAX_SUBNETS_PER_DB_SUBNET_GROUP`]), and a value that is not of
/// the EC2 subnet id form ([`check_subnet_id`]).
///
/// Called BEFORE the subnets are resolved against the service that owns them
/// ([`subnet_zones`]), and that order is the point: these three rules need nothing but the
/// request, so a list that breaks one is refused without a single hop, and the number of
/// identifiers that reach the resolution is capped by [`MAX_SUBNETS_PER_DB_SUBNET_GROUP`]
/// rather than by what a caller chose to send. The coverage rule, which is a statement about
/// the subnets' zones, follows in [`check_db_subnet_group_zone_coverage`].
pub fn check_db_subnet_group_subnet_list(
    subnet_ids: &[String],
) -> Result<(), SubnetListRejection> {
    if subnet_ids.is_empty() {
        return Err(SubnetListRejection::NotASubnet(
            EMPTY_SUBNET_LIST_MESSAGE.to_string(),
        ));
    }
    if subnet_ids.len() > MAX_SUBNETS_PER_DB_SUBNET_GROUP {
        return Err(SubnetListRejection::TooManySubnets(format!(
            "the request names {} subnets: a DB subnet group holds at most {}",
            subnet_ids.len(),
            MAX_SUBNETS_PER_DB_SUBNET_GROUP
        )));
    }
    for subnet_id in subnet_ids {
        check_subnet_id(subnet_id).map_err(SubnetListRejection::NotASubnet)?;
    }
    Ok(())
}

/// Refuse a `SubnetIds` list naming identifiers the account holds no subnet for, whichever
/// operation carries it.
///
/// A subnet is Amazon EC2's resource, so whether one EXISTS is EC2's answer, read off the
/// records the owner of subnets returned for the request's own identifiers ([`subnet_zones`]):
/// an identifier it answered for no subnet names no subnet of the account, and the list
/// cannot make up a group however many Availability Zones it might otherwise have covered.
///
/// Comes BEFORE [`check_db_subnet_group_zone_coverage`], the way RDS resolves the identifiers
/// before it evaluates coverage: coverage is a statement about subnets, and an identifier
/// that names none contributes no zone to count.
///
/// A hop the owner of subnets never answered refuses nothing: the identifiers are then
/// unknown rather than absent (see [`SubnetZones::absent`]).
///
/// The refusal names the request's WHOLE list rather than the absent subset
/// ([`invalid_subnets_message`]): the sentence says that SOME of the subnets it was asked
/// about are invalid.
pub fn check_db_subnet_group_subnets_exist(
    subnet_ids: &[String],
    zones: &SubnetZones,
) -> Result<(), SubnetListRejection> {
    if zones.absent(subnet_ids).is_empty() {
        return Ok(());
    }
    Err(SubnetListRejection::UnusableSubnets(
        invalid_subnets_message(subnet_ids),
    ))
}

/// Refuse a `SubnetIds` list whose subnets do not all lie in ONE VPC, whichever operation
/// carries it: `InvalidSubnet` is the fault for exactly that — the model documents it as "The
/// requested subnet is invalid, or multiple subnets were requested that are not all in a
/// common VPC" — and a DB subnet group is a set of subnets of one VPC, which is the VPC the
/// group reports.
///
/// The refusal NAMES the two VPCs the list straddles
/// ([`subnets_in_different_vpcs_message`]), which is what a caller needs to tell WHICH of the
/// subnets it named is the one in the wrong place.
///
/// The VPC of each subnet is Amazon EC2's answer for the request's own identifiers
/// ([`subnet_zones`]), so a list asked about where the owner of subnets could not be reached
/// names no VPC at all and is refused on nothing.
pub fn check_db_subnet_group_common_vpc(
    subnet_ids: &[String],
    zones: &SubnetZones,
) -> Result<(), SubnetListRejection> {
    let Some((first_vpc_id, second_vpc_id)) = zones.straddled_vpcs(subnet_ids) else {
        return Ok(());
    };
    Err(SubnetListRejection::NotInOneVpc(
        subnets_in_different_vpcs_message(&first_vpc_id, &second_vpc_id),
    ))
}

/// What a `SubnetIds` list that does not lie in ONE VPC is refused with: Amazon RDS states
/// that a subnet is in a different VPC than another, naming the two VPCs — the one a subnet
/// of the list lies in, and the one it has to be in.
///
/// Which two those are follows from what the call knows: a list asked to make up a group of
/// its own names the VPCs it straddles, in the order its subnets introduce them
/// ([`SubnetZones::straddled_vpcs`]); a list asked to make up a group that already HAS a VPC
/// names the VPC of the subnet outside it and the group's own
/// ([`check_db_subnet_group_subnets_in_vpc`]). Either way both values come from the subnets
/// the request named, as the service that owns them reports their VPC; the text around them
/// is fixed.
#[must_use]
pub fn subnets_in_different_vpcs_message(first_vpc_id: &str, second_vpc_id: &str) -> String {
    format!("Subnet has different VPC Id: {first_vpc_id} than {second_vpc_id}")
}

/// Refuse a `SubnetIds` list naming subnets of a VPC OTHER than `vpc_id`, the VPC the DB
/// subnet group a modification addresses is in.
///
/// A DB subnet group belongs to the VPC its subnets are in, and a modification changes which
/// subnets OF THAT VPC it holds: an identifier naming a subnet of another VPC is not one this
/// group can be made of. The list then names subnets of two VPCs, which is what the refusal
/// states — the sentence naming the VPC the offending subnet lies in and the one the group is
/// in ([`subnets_in_different_vpcs_message`]), the same sentence a list that straddles VPCs
/// without a group to compare against is refused with.
pub fn check_db_subnet_group_subnets_in_vpc(
    subnet_ids: &[String],
    zones: &SubnetZones,
    vpc_id: &str,
) -> Result<(), SubnetListRejection> {
    // A subnet is outside the group's VPC only where EC2 NAMED a VPC for it — one it named
    // none for is no evidence of a subnet in the wrong place ([`SubnetZones::outside_vpc`])
    // — so the VPC the sentence names beside the group's own is read off the first such
    // subnet, and a list with none of them is not refused here at all.
    let Some(elsewhere) = zones
        .outside_vpc(subnet_ids, vpc_id)
        .iter()
        .find_map(|subnet_id| zones.vpc_of_subnet(subnet_id))
    else {
        return Ok(());
    };
    Err(SubnetListRejection::UnusableSubnets(
        subnets_in_different_vpcs_message(elsewhere, vpc_id),
    ))
}

/// Refuse a `SubnetIds` list whose subnets cover fewer Availability Zones than a DB subnet
/// group must ("DB subnet groups must contain at least one subnet in at least two AZs in the
/// Amazon Web Services Region"), whichever operation carries it.
///
/// A subnet lies in exactly ONE Availability Zone, and WHICH zone is Amazon EC2's fact: the
/// coverage is therefore counted over the zones `zones` carries for the request's own
/// subnets ([`subnet_zones`]), so two subnets of one zone cover one zone and two subnets of
/// two zones cover two. The refusal NAMES the coverage those subnets add up to — the same
/// zones the group's `Subnets` report — which is what RDS reports.
///
/// A subnet EC2 answered no zone for lies in a zone this service does not know, which is not
/// evidence that the list covers too few: it counts as a zone that may be any of them, so
/// nothing is refused on an assumed zone — and a deployment where the owner of subnets
/// cannot be asked at all refuses nothing on that silence.
///
/// Comes after [`check_db_subnet_group_subnet_list`], whose rules the list has to pass
/// first: coverage is a statement about subnets that are there.
pub fn check_db_subnet_group_zone_coverage(
    subnet_ids: &[String],
    zones: &SubnetZones,
) -> Result<(), SubnetListRejection> {
    let distinct: BTreeSet<&str> = subnet_ids.iter().map(String::as_str).collect();
    let covered = zones.covered(distinct.iter().copied());
    let unknown = zones.unresolved(distinct.iter().copied());
    if covered.len().saturating_add(unknown) < MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES {
        return Err(SubnetListRejection::NotEnoughAvailabilityZones(
            db_subnet_group_az_coverage_message(&covered),
        ));
    }
    Ok(())
}

/// The Availability Zones a DB subnet group must cover: "DB subnet groups must contain at
/// least one subnet in at least two AZs in the Amazon Web Services Region."
pub const MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES: usize = 2;

/// What a `SubnetIds` list with no entries is refused with: the list itself is the thing
/// that cannot be used, so Amazon RDS states that the ids are required and says nothing
/// about the Availability Zone coverage the named subnets add up to — a statement about
/// subnets, which this request carries none of.
pub const EMPTY_SUBNET_LIST_MESSAGE: &str = "Subnet IDs are required.";

/// The message the `DBSubnetGroupDoesNotCoverEnoughAZs` fault carries: RDS states the
/// requirement, then the coverage the request's own subnets add up to, then what to do
/// about it. The zones are named in order, and a request that names no subnet at all
/// covers none.
pub fn db_subnet_group_az_coverage_message(covered: &BTreeSet<String>) -> String {
    let coverage = covered
        .iter()
        .map(String::as_str)
        .collect::<Vec<&str>>()
        .join(", ");
    format!(
        "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. \
         Current AZ coverage: {coverage}. Add subnets to cover at least \
         {MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES} AZs."
    )
}

/// The region segment of an ARN (`arn:aws:<service>:<region>:<account>:<resource>`), or
/// `None` when `value` does not have that shape. EMPTY for a region-less ARN, which is how
/// a global resource's ARN reads.
///
/// `CopyDBClusterSnapshotMessage$SourceDBClusterSnapshotIdentifier`: "If the source
/// snapshot is in the same Amazon Web Services Region as the copy, specify a valid DB
/// snapshot identifier ... If the source snapshot is in a different Amazon Web Services
/// Region ... specify a valid DB cluster snapshot ARN." So the region a copy looked for
/// its source in is the ARN's own when it was given one, and the request's region
/// otherwise.
pub fn arn_region(value: &str) -> Option<&str> {
    let mut fields = value.split(':');
    let (Some("arn"), Some(_partition), Some(_service), Some(region)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    Some(region)
}

/// What an Amazon RDS resource ARN names: the region it is in, the resource TYPE and the
/// resource's own identifier.
#[derive(Debug, Clone, Copy)]
pub struct RdsResourceArn<'a> {
    /// The ARN's region field.
    pub region: &'a str,
    /// The ARN's resource type — `db` for a DB instance, `cluster` for a DB cluster.
    pub resource_type: &'a str,
    /// The resource's identifier.
    pub identifier: &'a str,
}

/// The RDS resource `value` names, or `None` when `value` is not an ARN of this service at
/// all.
///
/// Several members take "the Amazon Resource Name (ARN)" of a database rather than its
/// identifier — `CreateBlueGreenDeployment$Source` is a `DatabaseArn`,
/// `CreateIntegration$SourceArn` an RDS `db` or `cluster` ARN — and which resource TYPE
/// the ARN names decides which store the lookup goes to, and so which not-found fault a
/// request that resolves to nothing is answered with.
pub fn parse_rds_resource_arn(value: &str) -> Option<RdsResourceArn<'_>> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || !partition.starts_with("aws") || *service != ARN_NAMESPACE {
        return None;
    }
    if !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (resource_type, identifier) = resource.split_once(':')?;
    if resource_type.is_empty() || identifier.is_empty() {
        return None;
    }
    Some(RdsResourceArn {
        region,
        resource_type,
        identifier,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRdsAmazonrdsv19State>)
}

/// The SHARED data-source directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A monitoring service addresses a DB resource by the immutable resource id THIS
/// service minted for it (`DbiResourceId`), and the form of that id does not say which
/// service minted it — Amazon DocumentDB mints ids of the same form. So a minted id is
/// recorded here, in ONE directory under one shared type
/// ([`::service_common::db_data_sources::DataSourceDirectory`]), which is how
/// Performance Insights can decide whether its request's `ServiceType` is the service
/// that owns the resource the request names. The engine keys state by TYPE, so a copy
/// of its own inside [`ComAmazonawsRdsAmazonrdsv19State`] would be invisible to the
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

/// The account's MANAGEMENT-EVENT (audit) log in the SAME shared engine — the one store
/// CloudTrail's `LookupEvents` reads: call it with `&*engine`.
///
/// A SHARED type ([`::service_common::management_events::ManagementEventLog`]) and not a
/// field of [`ComAmazonawsRdsAmazonrdsv19State`], because the service that SERVES a call
/// is the one that knows the call happened while the service that REPORTS it is
/// CloudTrail; the engine keys state by TYPE, so a copy of its own here would be
/// invisible to the reader.
///
/// Answer `None` by leaving the call unrecorded: the record is the account's audit trail,
/// never part of this operation's own answer, so an engine carrying no log still serves
/// the call.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::ManagementEventLog,
            >,
        )
}

/// The account's SESSION directory in that same shared engine: the sessions STS minted,
/// each keyed by the access key id it was minted under, which is what lets a record of a
/// call made with one name the session it was made by.
///
/// A SHARED type, for the same reason as the log above: the service that MINTS a session
/// and the services that record the calls made WITH it are not the same service.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::SessionAuditDirectory,
            >,
        )
}

/// The region this request was signed for: every ARN and endpoint host this service
/// mints is scoped to it, and it is half of every stored record's key.
///
/// Falls back to `us-east-1`, the region a generated emulator defaults to, when the
/// request carries no SigV4 credential scope to read it from. A scope that names
/// something that is not an AWS region id is refused rather than stored: the value is
/// request data this emulator does not authenticate, and it keys the store.
pub fn request_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<String, String> {
    let region = emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string());
    check_region(&region)?;
    Ok(region)
}

/// The longest AWS region id this service accepts. The longest one AWS publishes is
/// 14 characters (`ap-southeast-4`); 32 leaves room for any it adds.
const MAX_REGION_LEN: usize = 32;

/// Refuse a credential-scope region that is not of the published AWS region-id form:
/// lowercase letters in two or more hyphen-separated groups, then a one- or
/// two-digit number — `us-east-1`, `eu-central-1`, `ap-southeast-4`, `us-gov-west-1`,
/// `cn-north-1`.
pub fn check_region(region: &str) -> Result<(), String> {
    let malformed = || {
        format!(
            "the credential scope names '{region}', which is not an AWS region id \
             (expected a form like us-east-1, at most {MAX_REGION_LEN} characters)"
        )
    };
    if region.is_empty() || region.len() > MAX_REGION_LEN {
        return Err(malformed());
    }
    let mut parts: Vec<&str> = region.split('-').collect();
    let number = parts.pop().ok_or_else(malformed)?;
    if parts.len() < 2
        || !(1..=2).contains(&number.len())
        || !number.bytes().all(|b| b.is_ascii_digit())
        || !parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase()))
    {
        return Err(malformed());
    }
    Ok(())
}

/// The 8-4-4-4-12 lowercase-hexadecimal token a server-minted resource id of this service
/// takes when the id AWS assigns is a UUID: `label` says which kind of resource it belongs
/// to, and `mint` is the store's one monotone counter, so two resources — of the same kind
/// or of different kinds — can never be minted the same token. It is drawn from the same
/// seed every other minted token of this service is ([`mint_seed`]), so it is unique to its
/// one resource rather than to that resource's position in a creation sequence. The digest
/// is the maintained `sha2` crate's, so nothing here is hand-rolled.
pub fn mint_uuid_token(label: &str, mint: u64) -> String {
    uuid_of_seed(&mint_seed(label, mint))
}

/// The 8-4-4-4-12 lowercase-hexadecimal UUID of `seed`, derived from its digest and from
/// nothing else — so a value minted for one seed is the same every time any read asks for
/// it, which is what lets a UUID be derived from a resource's own immutable id rather than
/// stored beside it. [`mint_uuid_token`] is this reading over the store's counter.
pub fn uuid_of_seed(seed: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let digest = Sha256::digest(seed);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
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

/// `arn:aws:rds:<region>:<account>:db:<identifier>`.
pub fn db_instance_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("db:{identifier}"))
}

/// `arn:aws:rds:<region>:<account>:cluster:<identifier>` — the ARN form RDS gives a DB
/// cluster, which is also what `DescribeDBClusters$DBClusterIdentifier` and the
/// `db-cluster-id` filter accept in place of the identifier.
pub fn db_cluster_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("cluster:{identifier}"))
}

/// The resource `named` names: `named` itself, or the RESOURCE part of it when it is an
/// ARN.
///
/// An RDS resource ARN carries the resource it names in its last colon-separated field
/// (`arn:<partition>:rds:<region>:<account>:db:<identifier>`,
/// `…:cluster:<identifier>`), so one fold serves every member that accepts "the
/// user-supplied identifier or the Amazon Resource Name (ARN)". A value that is an ARN of
/// something else keeps its last segment, which names no resource of the type the caller
/// looks it up in and so resolves to nothing.
///
/// The two per-resource names below read better at a call site, and keep the resource a
/// handler is folding visible; they are aliases of this one fold, so a divergence in
/// either ARN shape is a change to the alias rather than a silent change to both.
pub fn arn_resource_name_of(named: &str) -> &str {
    match named.strip_prefix("arn:") {
        None => named,
        Some(rest) => rest.rsplit(':').next().unwrap_or(named),
    }
}

/// The DB cluster identifier `named` carries ([`arn_resource_name_of`]).
///
/// Several members accept either form — "the user-supplied DB cluster identifier or the
/// Amazon Resource Name (ARN) of the DB cluster" — so both are folded to the identifier
/// the store is keyed by.
pub fn db_cluster_identifier_of(named: &str) -> &str {
    arn_resource_name_of(named)
}

/// The DB instance identifier `named` carries ([`arn_resource_name_of`]).
///
/// `DescribeValidDBInstanceModificationsMessage$DBInstanceIdentifier` is "The customer
/// identifier or the ARN of your DB instance", so both forms are folded to the identifier
/// the store is keyed by.
pub fn db_instance_identifier_of(named: &str) -> &str {
    arn_resource_name_of(named)
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
/// the framework's own success-path minter produces, so a client cannot tell the two
/// apart. Unique because a monotone counter goes into the digest; the digest is the
/// maintained `sha2` crate's, so nothing here is hand-rolled.
pub fn new_request_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// The monotone part of a request id, so two ids minted inside one clock tick still
    /// differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let digest = Sha256::digest(format!("request:{counter}:{nanos}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version (4) and variant (RFC 4122) nibbles, so the id is well formed and not
    // mistaken for a name-based or time-based one.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid_text(&bytes)
}

/// The sixteen bytes `bytes` as a UUID in the 8-4-4-4-12 form every one of them is written
/// in, or the empty string for a value that is not sixteen bytes.
///
/// The ONE place that layout is written, so a minted request id and a `uuid` value a client
/// bound to a statement (`data_plane::pg_binary`) cannot be spelled differently. The
/// hexadecimal is the maintained `hex` crate's, so nothing here is hand-rolled.
#[must_use]
pub fn uuid_text(bytes: &[u8]) -> String {
    if bytes.len() != 16 {
        return String::new();
    }
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

/// The current instant as the modelled timestamp shapes carry it.
pub fn now_as_wire_timestamp() -> ::aws_smithy_types::DateTime {
    wire_timestamp_of(SystemTime::now())
}

/// The instant a change the clock owed became DUE, as the modelled timestamp shapes carry it:
/// the wall-clock reading of `due_at`, taken as how long ago it passed.
///
/// What a record whose progression settled LATE reports for the moment it reached that state:
/// the state was reached when the clock made it due, not when somebody finally looked, so a
/// resource first read long after it finished reports the instant it finished.
pub fn wire_timestamp_at(due_at: Instant, now: Instant) -> ::aws_smithy_types::DateTime {
    let late = now.saturating_duration_since(due_at);
    wire_timestamp_of(SystemTime::now().checked_sub(late).unwrap_or_else(SystemTime::now))
}

/// `instant` as the modelled timestamp shapes carry it.
fn wire_timestamp_of(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let seconds = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
}

/// The Route 53 hosted zone id reported for the endpoints of one region under one control
/// plane's DNS domain.
///
/// There is ONE zone per region and endpoint domain, so this is a function of those two
/// alone: every cluster and every instance a region serves under one domain reports the
/// same id, which is the invariance the member promises ("The ID that Amazon Route 53
/// assigns when you create a hosted zone").
///
/// The published ids, and the form of one minted for a pair none is cited for, live in
/// `service_common::rds_family_hosted_zones` — the graph control plane's own front door
/// reports the member for the same engine, and one zone per region and domain holds however
/// a caller reached the control plane, so both doors read one table and one mint rather than
/// a copy each. Published ids are cited there.
pub fn hosted_zone_id(region: &str, endpoint_domain: &str) -> String {
    ::service_common::rds_family_hosted_zones::hosted_zone_id(region, endpoint_domain)
}

/// `width` characters drawn from the digits-and-uppercase alphabet AWS's server-assigned
/// ids are written in, derived from the SHA-256 digest of `seed`.
///
/// Deterministic in the seed and in nothing else, so a value minted for one seed is the
/// same every time any read asks for it — and the ONE mapping of a digest onto that
/// alphabet, `service_common::minted_base36`, which the hosted zone of a region is minted
/// through as well, so two services minting from one seed cannot answer different ids.
fn minted_base36(seed: &str, width: usize) -> String {
    ::service_common::minted_base36(seed, width)
}

/// `width` lowercase hexadecimal characters of the SHA-256 digest of `seed` — the alphabet
/// an EC2 resource id is written in; see [`minted_base36`] for what being derived from the
/// seed alone buys.
fn minted_hex(seed: &str, width: usize) -> String {
    use ::sha2::{Digest, Sha256};

    let mut encoded = ::hex::encode(Sha256::digest(seed.as_bytes()));
    encoded.truncate(width);
    encoded
}

/// The EC2 VPC the subnets of one region resolve into.
///
/// RDS reads a subnet's VPC and Availability Zone off EC2, and this service holds no EC2
/// resources of its own, so a subnet a request names is resolved into the ONE VPC of its
/// region — the account's default VPC, which is the VPC a DB subnet group that names no
/// other one belongs to. The id is minted from the region in EC2's published `vpc-` form
/// and is the same for every read of every group in that region, so a client that resolves
/// a subnet group's `VpcId` to decide where to place a cluster gets an answer that holds
/// still; reporting nothing says the group belongs to no VPC, which no DB subnet group
/// does.
pub fn region_vpc_id(region: &str) -> String {
    format!("vpc-{}", minted_hex(&format!("vpc/{region}"), EC2_ID_WIDTH))
}

/// The default security group of a region's VPC: the group every VPC carries, and the one
/// a cluster or instance whose request named no `VpcSecurityGroupIds` belongs to.
///
/// Source: `CreateDBClusterMessage$VpcSecurityGroupIds` / `DBInstance$VpcSecurityGroups` —
/// a resource created without any named security group is placed in its VPC's default
/// group, which is why that group is what `VpcSecurityGroups` reports rather than an empty
/// list. Minted from the VPC it belongs to, in EC2's published `sg-` form.
pub fn default_security_group_id(region: &str) -> String {
    format!(
        "sg-{}",
        minted_hex(&format!("default-security-group/{}", region_vpc_id(region)), EC2_ID_WIDTH)
    )
}

/// The number of hexadecimal characters in a current-generation EC2 resource id
/// (`vpc-0a1b2c3d4e5f60718`, `sg-0a1b2c3d4e5f60718`).
const EC2_ID_WIDTH: usize = 17;

/// The Availability Zones of `region`, in the order the region publishes them.
///
/// The zones a Region HAS: the names AWS publishes for that Region, read off the one table
/// this tree keeps them in (`::service_common::aws_availability_zones`), so a Region that
/// exposes four or six zones answers four or six and not a fixed three. That is what a
/// refusal names as the zones a DB subnet group does not cover YET — the zones a caller can
/// add a subnet in — and the zones a standby may be placed in.
///
/// How many of them ONE DB cluster is placed across is a different question
/// ([`cluster_placement_zones`]), and which of them a given SUBNET lies in is not one this
/// service answers at all: that is read off the subnet's own record in the service that owns
/// subnets ([`crate::state::subnet_zones`]).
pub fn region_availability_zones(region: &str) -> Vec<String> {
    ::service_common::aws_availability_zones::availability_zones(region)
}

/// How many Availability Zones a DB cluster whose request named none is placed across.
///
/// `CreateDBCluster` places "a writer and two reader DB instances … each in a different
/// Availability Zone", so a cluster spans THREE zones however many the Region exposes —
/// which is why the placement is a prefix of [`region_availability_zones`] and not the whole
/// list.
pub const CLUSTER_PLACEMENT_ZONE_COUNT: usize = 3;

/// The Availability Zones a DB cluster in `region` whose request named none is placed
/// across: the first [`CLUSTER_PLACEMENT_ZONE_COUNT`] zones the Region publishes.
pub fn cluster_placement_zones(region: &str) -> Vec<String> {
    let mut zones = region_availability_zones(region);
    zones.truncate(CLUSTER_PLACEMENT_ZONE_COUNT);
    zones
}

/// The network types a DB subnet group of this service supports.
///
/// Source: `DBSubnetGroup$SupportedNetworkTypes` — "Valid values: IPV4 | DUAL … A
/// DBSubnetGroup can support only the IPv4 protocol or the IPv4 and the IPv6 protocols
/// (DUAL)." A group supports `DUAL` only where its subnets carry an IPv6 CIDR, and the
/// subnets this service resolves carry none, so every group it holds supports IPv4.
pub const SUBNET_GROUP_NETWORK_TYPES: [&str; 1] = ["IPV4"];

/// The `VpcSecurityGroups` membership list a cluster or an instance in `region` reports for
/// the security groups `ids` names.
///
/// Each association is in place from the create onwards, so each reports the `active`
/// status; a request that named none gets the one group RDS puts the resource in, its VPC's
/// default security group ([`default_security_group_id`]) — the member reports the groups
/// the resource BELONGS to, and every resource belongs to at least one.
pub fn vpc_security_group_memberships(
    region: &str,
    ids: &[String],
) -> Vec<VpcSecurityGroupMembership> {
    let membership = |id: String| VpcSecurityGroupMembership {
        vpc_security_group_id: Some(id),
        status: Some(MEMBERSHIP_ACTIVE.to_string()),
    };
    if ids.is_empty() {
        return vec![membership(default_security_group_id(region))];
    }
    ids.iter().cloned().map(membership).collect()
}

/// The port an Amazon DocumentDB cluster and its instances accept connections on when the
/// request names none: 27017, the port that control plane's `CreateDBCluster` reports for
/// a cluster created without a `Port` of its own (the Amazon DocumentDB API documents it
/// as that member's default, and its own emulated front door
/// `services/service-com-amazonaws-docdb-amazonrdsv19/src/state.rs` states the same
/// value).
pub const DOCDB_DEFAULT_PORT: i32 = 27017;

/// The same for Amazon Neptune, from that control plane's own
/// `CreateDBClusterMessage$Port`: "Default: 8182"
/// (`smithy_query(query="doc",shape="CreateDBClusterMessage",member="Port",model_path="services/service-com-amazonaws-neptune-amazonrdsv19/smithy-model/com-amazonaws-neptune-amazonrdsv19.json")`).
pub const NEPTUNE_DEFAULT_PORT: i32 = 8182;

/// The DNS domain the endpoints of a resource running `engine` sit in.
///
/// An endpoint's domain belongs to the control plane that OWNS the engine rather than to
/// this front door: a cluster of one of [`SIBLING_CONTROL_PLANE_ENGINES`], and every
/// instance in it, is reached under that service's own domain — `docdb.amazonaws.com` for
/// Amazon DocumentDB, `neptune.amazonaws.com` for Amazon Neptune, both as the endpoint
/// form those services publish for their clusters — while everything this door creates for
/// its own engines is reached under `rds.amazonaws.com`. The label in front of the domain
/// is the same server-assigned one either way; only the domain follows the engine.
pub fn endpoint_domain(engine: &str) -> &'static str {
    if engine.eq_ignore_ascii_case(DOCDB_ENGINE) {
        DOCDB_ENDPOINT_DOMAIN
    } else if engine.eq_ignore_ascii_case(NEPTUNE_ENGINE) {
        NEPTUNE_ENDPOINT_DOMAIN
    } else {
        RDS_ENDPOINT_DOMAIN
    }
}

/// The DNS domain this front door's own endpoints sit in; see [`endpoint_domain`].
///
/// The three domains are stated ONCE, in the module both front doors read a Region's hosted
/// zone out of (`service_common::rds_family_hosted_zones`), because that table is keyed by
/// the domain: a second copy of one of these strings that drifted would stop matching the
/// published row for a Region.
pub const RDS_ENDPOINT_DOMAIN: &str =
    ::service_common::rds_family_hosted_zones::RDS_ENDPOINT_DOMAIN;
/// The DNS domain the Amazon DocumentDB control plane's endpoints sit in.
pub const DOCDB_ENDPOINT_DOMAIN: &str =
    ::service_common::rds_family_hosted_zones::DOCDB_ENDPOINT_DOMAIN;
/// The DNS domain the Amazon Neptune control plane's endpoints sit in.
pub const NEPTUNE_ENDPOINT_DOMAIN: &str =
    ::service_common::rds_family_hosted_zones::NEPTUNE_ENDPOINT_DOMAIN;


/// The daily backup window an instance gets when the request names none.
///
/// `PreferredBackupWindow`: "The default is a 30-minute window selected at random from
/// an 8-hour block of time for each Amazon Web Services Region." This emulator picks
/// ONE window rather than a random one, so two reads of an instance report the same
/// window; it keeps the documented `hh24:mi-hh24:mi` UTC format and the documented
/// 30-minute minimum.
pub const DEFAULT_PREFERRED_BACKUP_WINDOW: &str = "07:00-07:30";

/// The weekly maintenance window an instance gets when the request names none.
///
/// `PreferredMaintenanceWindow`: "The default is a 30-minute window selected at random
/// from an 8-hour block of time for each Amazon Web Services Region, occurring on a
/// random day of the week" — again one fixed window here, in the documented
/// `ddd:hh24:mi-ddd:hh24:mi` form, on one of the documented day values, at the
/// documented 30-minute minimum, and "not conflict[ing] with the preferred backup
/// window" ([`DEFAULT_PREFERRED_BACKUP_WINDOW`] ends half an hour before it starts).
pub const DEFAULT_PREFERRED_MAINTENANCE_WINDOW: &str = "mon:08:00-mon:08:30";

/// The Database Insights mode an instance is created in when the request names none:
/// the `standard` member of `DatabaseInsightsMode` (`standard | advanced`), advanced
/// being the mode a request asks for.
pub const DEFAULT_DATABASE_INSIGHTS_MODE: &str = "standard";

/// The network type an instance is created with when the request names none.
///
/// `NetworkType`: "Valid Values: `IPV4 | DUAL`" — a subnet group "can support only the
/// IPv4 protocol or the IPv4 and the IPv6 protocols (`DUAL`)", so IPv4 only unless the
/// request asks for both.
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The network types a resource of this service may be created with, as
/// `CreateDBClusterMessage$NetworkType` publishes them ("Valid Values: IPV4 | DUAL"), the
/// first being the default [`DEFAULT_NETWORK_TYPE`].
pub const NETWORK_TYPES: [&str; 2] = [DEFAULT_NETWORK_TYPE, "DUAL"];

/// How many entries one of the lists `CreateDBCluster` STORES may carry
/// (`AvailabilityZones`, `EnableCloudwatchLogsExports`).
///
/// Emulator resource guard, not an AWS quota: neither member publishes a bound and both
/// are stored, so the growth site carries a cap — the same reason [`MAX_FILTER_TERMS`]
/// caps a describe's filter terms. Generous next to the three Availability Zones an Aurora
/// cluster spans and the handful of log types each engine publishes.
pub const MAX_CLUSTER_LIST_ENTRIES: usize = 32;

/// Where automated backups and manual snapshots are stored when the request names no
/// `BackupTarget`: "Valid Values: local (Dedicated Local Zone) outposts (Amazon Web
/// Services Outposts) region (Amazon Web Services Region) Default: region".
pub const DEFAULT_BACKUP_TARGET: &str = "region";

/// The lifecycle type an instance is created with when the request names none:
/// `EngineLifecycleSupport` "Default: `open-source-rds-extended-support`".
pub const DEFAULT_ENGINE_LIFECYCLE_SUPPORT: &str = "open-source-rds-extended-support";

/// The Enhanced Monitoring interval an instance is created with when the request names
/// none: `MonitoringInterval` "Valid Values: 0 | 1 | 5 | 10 | 15 | 30 | 60. Default:
/// 0" — zero disables collection.
pub const DEFAULT_MONITORING_INTERVAL: i32 = 0;

/// The baseline storage throughput, in MiBps, of the `gp3` storage type — the only
/// type `StorageThroughput` "applies only to" ("General Purpose SSD gp3 storage:
/// baseline storage throughput of 125 MiBps", Amazon RDS storage types, Amazon RDS
/// User Guide).
pub const GP3_BASELINE_STORAGE_THROUGHPUT: i32 = 125;

/// The activity-stream status of a resource that holds no database activity stream: the
/// `stopped` member of `ActivityStreamStatus` (`stopped | starting | started |
/// stopping`). The status a DB cluster or DB instance no `StartActivityStream` started a
/// stream on reports, and the one a stream whose stop has come due leaves behind
/// ([`ActivityStream`]).
pub const ACTIVITY_STREAM_STOPPED: &str = "stopped";

/// `DocdbAnalyticsEnabled` for a cluster created without the Amazon DocumentDB analytics
/// capability: no member of any `CreateDBCluster` this front door serves turns it on, so a
/// cluster this service holds never has it.
pub const DEFAULT_DOCDB_ANALYTICS_ENABLED: bool = false;

/// `CustomDataGroupPresent` for a cluster created without a custom data group: no member of
/// any `CreateDBCluster` this front door serves supplies one.
pub const DEFAULT_CUSTOM_DATA_GROUP_PRESENT: bool = false;

/// `AutoMinorVersionUpgrade` for a cluster whose request named nothing, for the engines the
/// member applies to: `CreateDBClusterMessage$AutoMinorVersionUpgrade` — "By default, minor
/// engine upgrades are applied automatically", "Valid for Cluster Type: Aurora DB clusters
/// and Multi-AZ DB cluster".
///
/// Which engines take it is [`cluster_auto_minor_version_upgrade_default`]: a cluster of an
/// engine whose own control plane neither publishes the member nor applies minor upgrades on
/// its own is born with it OFF.
pub const DEFAULT_CLUSTER_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// The upgrade rollout order of an instance nothing configured one for:
/// `UpgradeRolloutOrder` "[second] - Default order for resources not specifically
/// configured". No operation this service implements configures it.
pub const DEFAULT_UPGRADE_ROLLOUT_ORDER: &str = "second";

/// The failover order of a DB instance whose request named none:
/// `CreateDBInstanceMessage$PromotionTier` — "the order of priority in which an Aurora
/// Replica is promoted to the primary instance after a failure of the existing primary
/// instance … Default: `1`".
pub const DEFAULT_PROMOTION_TIER: i32 = 1;

/// The status RDS reports for a group membership that is in effect — the
/// `ParameterApplyStatus` of an applied DB parameter group and the `Status` of an
/// applied option group.
const MEMBERSHIP_IN_SYNC: &str = "in-sync";

/// The `DBParameterGroupStatus$ParameterApplyStatus` of a DB parameter group whose change
/// this instance's engine has not come up again since.
///
/// `ModifyDBParameterGroup`: "changes to static parameters require a reboot ... before the
/// change can take effect" — and the status says which reboot a caller still owes, so a
/// client can tell a change that is in force from one that is waiting
/// ([`ComAmazonawsRdsAmazonrdsv19State::parameter_apply_status`]).
const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// The `DBParameterGroupStatus$ParameterApplyStatus` of a DB parameter group RDS is still
/// ATTACHING to the instance, which is what a modification that moves the instance to
/// another group starts ("the parameter group name itself is changed immediately" —
/// `ModifyDBInstance$DBParameterGroupName`).
///
/// Transient, like every other change this service owes: the attach is due
/// [`STATUS_TRANSITION_DELAY`] after the modification, after which the membership rests
/// where the values the newly associated group carries put it — [`MEMBERSHIP_IN_SYNC`] where
/// each of them is applied immediately, and [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] until a
/// reboot otherwise ([`DbInstanceRecord::parameter_apply_status`]).
const PARAMETER_APPLY_STATUS_APPLYING: &str = "applying";

/// The `OptionGroupMembership$Status` of an option group RDS has NOT applied to the
/// instance yet — one of the values that member publishes ("`in-sync`, `pending-apply`,
/// `pending-removal`, …").
///
/// The group is applied as part of provisioning the instance, so the attachment is
/// outstanding at the instant the call that starts it is accepted and is applied from the
/// next one ([`DbInstanceRecord::option_group_apply_status`]).
const OPTION_GROUP_STATUS_PENDING_APPLY: &str = "pending-apply";

/// The status RDS reports for a security-group membership that is in effect.
const MEMBERSHIP_ACTIVE: &str = "active";

/// The lowest and highest port `CreateDBInstanceMessage$Port` accepts
/// ("Valid Values: 1150-65535").
pub const PORT_RANGE: std::ops::RangeInclusive<i32> = 1150..=65535;

/// The lowest and highest `BackupRetentionPeriod`
/// ("Constraints: Must be a value from 0 to 35").
pub const BACKUP_RETENTION_RANGE: std::ops::RangeInclusive<i32> = 0..=35;

/// The lowest and highest `MaxRecords` a describe operation accepts
/// ("Constraints: Minimum 20, maximum 100").
pub const MAX_RECORDS_RANGE: std::ops::RangeInclusive<i32> = 20..=100;

/// The page size a describe operation uses when the request names none
/// (`MaxRecords` "Default: 100").
pub const DEFAULT_MAX_RECORDS: usize = 100;

/// Whether `engine` names an Aurora engine, which several documented defaults
/// branch on (`aurora-mysql`, `aurora-postgresql`).
pub fn is_aurora(engine: &str) -> bool {
    engine.to_ascii_lowercase().starts_with("aurora")
}

/// Whether an instance running `engine` is always a MEMBER of a DB cluster, so that its
/// master user credential is the CLUSTER's and the instance has none of its own.
///
/// These are the engines of the two sibling control planes this front door serves
/// ([`is_sibling_control_plane_engine`]), whose OWN `CreateDBInstance` APIs carry no usable
/// master-user member at all: `com.amazonaws.docdb#CreateDBInstanceMessage` declares
/// neither `MasterUsername` nor `MasterUserPassword`, and the Amazon Neptune model declares
/// both and documents each as "Not supported by Neptune"
/// (`smithy_query(query="doc",shape="CreateDBInstanceMessage",member="MasterUserPassword")`
/// on that model). An instance of such an engine exists only as a member of a cluster,
/// which is where its master user lives. Named for the rule it carries rather than for the
/// control plane, and resolved by the one predicate that holds the engine list.
///
/// The Aurora engines are NOT here even though their own instances take the member's value
/// no more ("This setting doesn't apply to Amazon Aurora DB instances"): this predicate
/// gates a REFUSAL, and nothing recorded says what the Aurora instance create answers.
pub fn is_cluster_member_engine(engine: &str) -> bool {
    is_sibling_control_plane_engine(engine)
}

/// The engine prefixes an instance-level start or stop does not apply to, from the
/// documentation `StartDBInstance` and `StopDBInstance` share: "This command doesn't
/// apply to RDS Custom, Aurora MySQL, and Aurora PostgreSQL. For Aurora DB clusters,
/// use StartDBCluster / StopDBCluster instead."
///
/// RDS Custom is every `custom-` engine of [`DB_INSTANCE_ENGINES`], and Aurora MySQL and
/// Aurora PostgreSQL are its two `aurora-` engines, so the rule is carried as the two
/// prefixes rather than as a list a further engine of either family would fall out of.
const ENGINE_PREFIXES_WITHOUT_AN_INSTANCE_START_OR_STOP: [&str; 2] = ["aurora-", "custom-"];

/// Whether an instance running `engine` can be started and stopped on its own, as
/// opposed to only through its DB cluster — see
/// [`ENGINE_PREFIXES_WITHOUT_AN_INSTANCE_START_OR_STOP`]. One predicate for both
/// operations, so the two cannot disagree about which engines they apply to.
pub fn has_instance_level_start_stop(engine: &str) -> bool {
    let engine = engine.to_ascii_lowercase();
    !ENGINE_PREFIXES_WITHOUT_AN_INSTANCE_START_OR_STOP
        .iter()
        .any(|prefix| engine.starts_with(prefix))
}

/// The documented default for `PubliclyAccessible` when the request omits it.
///
/// From `CreateDBInstanceMessage$PubliclyAccessible`: "If `DBSubnetGroup` isn't
/// specified, `PubliclyAccessible` defaults to `false` for Aurora instances and
/// `true` for non-Aurora instances. If `DBSubnetGroup` is specified,
/// `PubliclyAccessible` defaults to `false` unless the value of `DBSubnetGroup` is
/// `default`, in which case `PubliclyAccessible` defaults to `true`."
pub fn default_publicly_accessible(engine: &str, db_subnet_group_name: Option<&str>) -> bool {
    match db_subnet_group_name {
        None => !is_aurora(engine),
        Some(name) => name.eq_ignore_ascii_case("default"),
    }
}

/// The storage type a DB instance gets when the request names none.
///
/// `CreateDBInstanceMessage$StorageType` states "Default: `io1`, if the `Iops` parameter is
/// specified", and the recorded service provisions GENERAL PURPOSE SSD — `gp2`, the first
/// value the member's own list publishes ("Valid Values: `gp2 | gp3 | io1 | io2 |
/// standard`") — for a request that specifies neither, where that same sentence names `gp3`.
/// The recorded answer is what the service does, so it is what this emulator provisions:
/// such an instance reports `gp2` and, since `StorageThroughput` "applies only to the `gp3`
/// storage type", no separate throughput ([`default_storage_throughput`]).
pub fn default_storage_type(iops: Option<i32>) -> &'static str {
    if iops.is_some() {
        "io1"
    } else {
        "gp2"
    }
}

/// The storage throughput a DB instance is provisioned with when the request names
/// none.
///
/// `StorageThroughput` "applies only to the `gp3` storage type", whose baseline is
/// [`GP3_BASELINE_STORAGE_THROUGHPUT`]; an instance on any other storage type is
/// provisioned no separate throughput, which RDS reports as zero rather than as an
/// absent member.
pub fn default_storage_throughput(storage_type: &str) -> i32 {
    if storage_type.eq_ignore_ascii_case("gp3") {
        GP3_BASELINE_STORAGE_THROUGHPUT
    } else {
        0
    }
}

/// The license model a record OF A CLUSTER reports — its snapshots and its automated
/// backups — for a cluster running `engine`.
///
/// `LicenseModel` "doesn't apply to Amazon Aurora", whose engines are not offered under a
/// community licence at all: the engine IS the licensed product, so the cluster-level
/// records of an Aurora engine report the ENGINE'S OWN NAME as their license model, exactly
/// as Amazon Neptune's do ([`default_license_model`]). Every other engine's cluster reports
/// the licence that engine family publishes, which is the one an instance of it reports too.
pub fn cluster_license_model(engine: &str) -> Option<String> {
    if is_aurora(engine) {
        return Some(engine.to_ascii_lowercase());
    }
    default_license_model(engine).map(str::to_string)
}

/// The license model of an instance the request named none for, or `None` for an engine
/// whose license model this emulator cannot cite one for.
///
/// From the value list `CreateDBInstanceMessage$LicenseModel` publishes: "RDS for Db2 -
/// bring-your-own-license | marketplace-license" ("The default for RDS for Db2 is
/// bring-your-own-license"), "RDS for MariaDB - general-public-license", "RDS for
/// Microsoft SQL Server - license-included | bring-your-own-media", "RDS for MySQL -
/// general-public-license", "RDS for Oracle - bring-your-own-license |
/// license-included", "RDS for PostgreSQL - postgresql-license". Where the list offers
/// one value it IS the default; where it offers two, the default is the one that engine
/// can be created with unprepared — a Db2 or Oracle Enterprise Edition instance runs on
/// a license you bring, while SQL Server and Oracle SE2 include one. The member
/// "doesn't apply to Amazon Aurora", whose INSTANCES RDS reports with the license model
/// of the engine family they run (`aurora-mysql` the MySQL one, `aurora-postgresql` the
/// PostgreSQL one), so the match is on the engine family rather than on the exact
/// engine name; a record of an Aurora CLUSTER reports the engine's own name instead, which
/// is what [`cluster_license_model`] answers.
pub fn default_license_model(engine: &str) -> Option<&'static str> {
    let engine = engine.to_ascii_lowercase();
    if engine.contains("db2") {
        Some("bring-your-own-license")
    } else if engine.contains("sqlserver") || engine.contains("oracle-se") {
        Some("license-included")
    } else if engine.contains("oracle") {
        Some("bring-your-own-license")
    } else if engine.contains("postgres") {
        Some("postgresql-license")
    } else if engine.contains("mysql") || engine.contains("mariadb") {
        Some("general-public-license")
    } else if engine == NEPTUNE_ENGINE {
        // Amazon Neptune publishes no separate license: the engine IS the licensed
        // product, and its control plane reports the engine's own name as the license
        // model of a Neptune cluster's snapshots and instances.
        Some(NEPTUNE_ENGINE)
    } else {
        None
    }
}

/// The master user a control plane ASSIGNS a cluster of `engine`, for an engine whose
/// `CreateDBCluster` cannot be given one.
///
/// `MasterUsername` is documented "Not supported by Neptune"
/// (`smithy_query(query="doc",shape="CreateDBClusterMessage",member="MasterUsername",model_path="services/service-com-amazonaws-neptune-amazonrdsv19/smithy-model/com-amazonaws-neptune-amazonrdsv19.json")`),
/// so a Neptune cluster's master user is not a value the request carries — the control
/// plane assigns it, and `admin` is the name it assigns. Every other engine's cluster
/// reports the username its own request named and nothing where it named none.
pub fn assigned_master_username(engine: &str) -> Option<&'static str> {
    engine
        .eq_ignore_ascii_case(NEPTUNE_ENGINE)
        .then_some(NEPTUNE_ASSIGNED_MASTER_USERNAME)
}

/// The master user Amazon Neptune assigns a cluster; see [`assigned_master_username`].
pub const NEPTUNE_ASSIGNED_MASTER_USERNAME: &str = "admin";

/// The name of the default DB parameter group of one engine-version family.
///
/// `DBParameterGroupName`: "If you don't specify a value, then Amazon RDS uses the
/// default DB parameter group for the specified DB engine and version" — the group RDS
/// names [`DEFAULT_PARAMETER_GROUP_PREFIX`] followed by that version's
/// `DBParameterGroupFamily` (`default.mysql8.0`); the same naming its default DB CLUSTER
/// parameter group carries.
pub fn default_db_parameter_group_name(parameter_group_family: &str) -> String {
    format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{parameter_group_family}")
}

/// The name of the default option group of one engine and major engine version.
///
/// RDS creates a default option group per engine and major version and names it
/// `default:<engine>-<major engine version>`, writing the version's dots as hyphens
/// (`default:mysql-8-0`, `default:sqlserver-ee-15-00`) — the naming the "Working with
/// option groups" section of the Amazon RDS User Guide publishes.
pub fn default_option_group_name(engine: &str, major_engine_version: &str) -> String {
    format!(
        "default:{}-{}",
        engine.to_ascii_lowercase(),
        major_engine_version.replace('.', "-")
    )
}

/// How Amazon RDS WORDS the refusal of an identifier member's value: by the SUBJECT the
/// member carries, or by the sentence pair that states the rule.
///
/// The subject wording is `Invalid <subject>:  <value>` — it names what the parameter is
/// rather than the member, carries no reason, and echoes the value as the request spelled
/// it (the two spaces after the colon are part of it, not a typo). The other is
/// [`not_a_valid_identifier_message`], which names the member at fault, states the whole
/// rule and echoes nothing.
///
/// Neither is anything the model states: which one a member answers, and for which clause,
/// is the service's own and is recorded per member — so it is given here, once, rather than
/// decided in each handler.
enum IdentifierWording {
    /// This member is refused by its subject whichever clause the value broke.
    Subject(&'static str),
    /// This member is refused by its subject only where the value is no identifier TOKEN of
    /// it at all; a token of that shape which still breaks one of the two HYPHEN rules is
    /// refused with the sentence pair instead.
    SubjectForAValueThatIsNoToken(&'static str),
    /// This member is refused by its subject only where the value is drawn from the
    /// identifier ALPHABET ([`is_drawn_from_the_identifier_alphabet`]) — so a leading digit,
    /// a trailing hyphen, two consecutive hyphens and a value past the member's bound all
    /// answer the subject — while a value carrying a character that alphabet excludes is
    /// refused with the sentence pair instead.
    ///
    /// The split applies to a value a request OFFERS as an identifier to be stored
    /// ([`check_identifier`]), not to a REFERENCE being resolved
    /// ([`check_identifier_reference`]), which keeps the subject for every clause.
    SubjectForAValueFromTheAlphabet(&'static str),
    /// This member is refused with the sentence pair whichever clause the value broke.
    TheRule,
}

/// Which wording each identifier member of this front door carries.
fn identifier_wording(member: &str) -> IdentifierWording {
    match member {
        // The identifier of a DB instance, whether `CreateDBInstance` is naming the one to
        // create or `ModifyDBInstance` the one to rename to: RDS answers its subject for a
        // value that is no identifier token of the member at all, and states the rule for a
        // token of that shape which breaks one of the two hyphen rules.
        "DBInstanceIdentifier" | "NewDBInstanceIdentifier" => {
            IdentifierWording::SubjectForAValueThatIsNoToken("database identifier")
        }
        // The identifier of a DB cluster, which RDS words by its own subject rather than
        // by the DB instance's — the two refusals differ in that one word — for every clause
        // a value drawn from the identifier alphabet can break, including the ones the
        // instance path states the rule for. A value carrying a character the alphabet
        // excludes is the one clause it answers with the rule instead.
        "DBClusterIdentifier" | "NewDBClusterIdentifier" => {
            IdentifierWording::SubjectForAValueFromTheAlphabet("database cluster identifier")
        }
        // The name of a DB SUBNET group, which RDS words by its own subject — the one it
        // calls that member by, which names neither the resource type's `DB` nor the member
        // — for every clause of the name rule, the way a DB cluster's identifier is worded.
        "DBSubnetGroupName" => IdentifierWording::Subject("subnet group name"),
        // The name of an option group: on create, on modify, and the name a COPY of a
        // group is created under, which is an option group's name too — so a caller
        // reading the refusal cannot tell those paths apart by its wording.
        "OptionGroupName" | "TargetOptionGroupIdentifier" => {
            IdentifierWording::Subject("option group name")
        }
        // The name of a DB CLUSTER parameter group, which RDS words by its own subject
        // rather than by the sentence pair the DB parameter group's name answers — the two
        // members differ in that — for every clause of the name rule, and at the create and
        // at the name a COPY of a group is created under alike.
        "DBClusterParameterGroupName" | "TargetDBClusterParameterGroupIdentifier" => {
            IdentifierWording::Subject("cluster parameter group name")
        }
        // The identifier of a DB SNAPSHOT a copy names: the source it is taken of and the
        // name the copy is created under are both a snapshot's identifier, so the one
        // subject answers either.
        "SourceDBSnapshotIdentifier" | "TargetDBSnapshotIdentifier" => {
            IdentifierWording::Subject("snapshot identifier")
        }
        // The identifier of a GLOBAL database, whether `CreateGlobalCluster` is naming the
        // one to create or `ModifyGlobalCluster` the one to rename to: RDS answers the DB
        // instance's split here too — its subject for a value that is no identifier token
        // of the member at all, the rule for a token of that shape which breaks one of the
        // two hyphen rules — with the subject a global database carries.
        "GlobalClusterIdentifier" | "NewGlobalClusterIdentifier" => {
            IdentifierWording::SubjectForAValueThatIsNoToken("global cluster identifier")
        }
        // Every other identifier member — a DB parameter group's name, a subscription's, a
        // shard group's, a tenant database's — states the rule and names itself.
        _ => IdentifierWording::TheRule,
    }
}

/// The length bound a GLOBAL CLUSTER identifier is held to, at every member that names one.
///
/// A global database has ONE identifier rule, and the member that renames it is where the API
/// writes that rule down: `ModifyGlobalClusterMessage$NewGlobalClusterIdentifier` states "Must
/// contain from 1 to 63 letters, numbers, or hyphens. The first character must be a letter.
/// Can't end with a hyphen or contain two consecutive hyphens." The create's own member
/// documents only that the value "is stored as a lowercase string", and the model's `@length`
/// on it (1..255) is wider than the rule the API publishes — exactly as its `@pattern` is
/// wider than the alphabet an identifier may use — so both members are held to the one
/// published rule, and an identifier a create refuses cannot be reached by a rename either.
pub const MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN: usize = 63;

/// Refuse an identifier that breaks the constraints its member documents.
///
/// `DBInstanceIdentifier` and `DBSnapshotIdentifier` share one rule, differing only
/// in the length bound (63 and 255): "Must contain from 1 to N letters, numbers, or
/// hyphens. First character must be a letter. Can't end with a hyphen or contain two
/// consecutive hyphens."
///
/// Returns the message to refuse the request with: the MEMBER, and for the members that
/// split them the CLAUSE the value broke, settle which of the two wordings Amazon RDS
/// answers with (see [`IdentifierWording`]).
///
/// This is the gate for a value a request OFFERS as the identifier of a resource it creates,
/// copies, restores or renames. A REFERENCE being resolved — the identifier a request names
/// an EXISTING resource by — goes through [`check_identifier_reference`], which words every
/// clause by the member's subject.
///
/// The wording is the MEMBER's and the CLAUSE's, not the engine's: the same member is
/// refused the same way whichever control plane's engine the request named, which is what
/// the recorded answers of this one front door carry.
///
/// The CALLER answers this with `InvalidParameterValue`: a value outside the constraints
/// its own member publishes is exactly what that Query API common error reports, and the
/// error code is what a client branches on.
pub fn check_identifier(member: &str, value: &str, max_len: usize) -> Result<(), String> {
    if identifier_is_well_formed(value, max_len) {
        return Ok(());
    }
    Err(offered_identifier_refusal_message(member, value, max_len))
}

/// Refuse a REFERENCE that is no identifier of its member at all: the value a request names
/// an EXISTING resource by, as opposed to one it offers to be stored ([`check_identifier`]).
///
/// The grammar is the same walk ([`identifier_is_well_formed`]); what differs is the
/// WORDING, because a reference gate's recorded answers carry the member's subject for every
/// clause — a character the identifier alphabet excludes included — where the value a create
/// offers is held to the alphabet by the front door's own statement of the rule
/// ([`IdentifierWording::SubjectForAValueFromTheAlphabet`]).
pub fn check_identifier_reference(
    member: &str,
    value: &str,
    max_len: usize,
) -> Result<(), String> {
    if identifier_is_well_formed(value, max_len) {
        return Ok(());
    }
    Err(identifier_refusal_message(member, value, max_len))
}

/// The message a value a request OFFERS as an identifier is refused with: the member's own
/// wording ([`identifier_refusal_message`]), except where that member answers the sentence
/// pair for a value carrying a character the identifier alphabet excludes.
///
/// A member whose wording splits on the alphabet states the whole rule there — the sentence
/// that names the member and echoes nothing — because the alphabet is the one clause the
/// front door states rather than names a subject for.
fn offered_identifier_refusal_message(member: &str, value: &str, max_len: usize) -> String {
    match identifier_wording(member) {
        IdentifierWording::SubjectForAValueFromTheAlphabet(_)
            if !is_drawn_from_the_identifier_alphabet(value) =>
        {
            not_a_valid_identifier_message(member)
        }
        _ => identifier_refusal_message(member, value, max_len),
    }
}

/// The message a value outside its identifier member's rule is refused with: the wording
/// that member carries ([`identifier_wording`]), filled in from the member and the value.
///
/// Stated once here because the rule a value broke and the WORDING of the refusal are two
/// different things: every member whose value has to be an identifier of this front door —
/// whether its rule is the identifier walk ([`check_identifier`]) or the name rule a
/// parameter group's name follows ([`check_parameter_group_name`]) — words its refusal from
/// this one place, so no two of them can disagree about what a member is called.
fn identifier_refusal_message(member: &str, value: &str, max_len: usize) -> String {
    match identifier_wording(member) {
        IdentifierWording::Subject(subject)
        | IdentifierWording::SubjectForAValueFromTheAlphabet(subject) => {
            format!("Invalid {subject}:  {value}")
        }
        IdentifierWording::SubjectForAValueThatIsNoToken(subject)
            if !is_an_identifier_token(value, max_len) =>
        {
            format!("Invalid {subject}:  {value}")
        }
        _ => not_a_valid_identifier_message(member),
    }
}

/// Refuse a parameter group NAME outside the rule the API publishes for it, worded the way
/// this front door words that member.
///
/// The rule is the shared one every view of this front door holds a parameter group name to
/// ([`validate_parameter_group_name`]: "from 1 to 255 letters or numbers. The first
/// character must be a letter. Cannot end with a hyphen or contain two consecutive
/// hyphens"). What is this crate's own is the WORDING — a cluster parameter group's name is
/// refused by its SUBJECT, the other parameter-group members by the sentence pair that
/// states the rule — so the verdict comes from the shared rule and the sentence from
/// [`identifier_refusal_message`], the one place every identifier member of this service is
/// worded from.
pub fn check_parameter_group_name(member: &str, value: &str) -> Result<(), String> {
    if validate_parameter_group_name(member, value).is_ok() {
        return Ok(());
    }
    Err(identifier_refusal_message(
        member,
        value,
        MAX_PARAMETER_GROUP_NAME_LEN,
    ))
}

/// The longest `DBSubnetGroupName` this front door accepts, as that member documents it:
/// "Must contain no more than 255 letters, numbers, periods, underscores, spaces, or
/// hyphens."
pub const MAX_DB_SUBNET_GROUP_NAME_LEN: usize = 255;

/// Refuse a `DBSubnetGroupName` outside the constraints its member documents — "Must contain
/// no more than 255 letters, numbers, periods, underscores, spaces, or hyphens. ... First
/// character must be a letter."
///
/// The RULE is this member's own: its alphabet is WIDER than the one the identifier members
/// share ([`check_identifier`]), admitting periods, underscores and spaces as well. The
/// WORDING is not — it comes from [`identifier_refusal_message`], the one place every named
/// member of this front door is worded from, so this member cannot drift from its neighbours
/// into a sentence of the emulator's own.
///
/// The value is echoed as the REQUEST spelled it: the refusal is about the value the caller
/// sent, not about the folded spelling the group would have been stored under ("This value is
/// stored as a lowercase string"), and the rule itself does not look at case.
///
/// The caller answers this with `InvalidParameterValue`, the Query API common error a value
/// outside its member's published constraints is reported with.
pub fn check_db_subnet_group_name(value: &str) -> Result<(), String> {
    if db_subnet_group_name_is_well_formed(value) {
        return Ok(());
    }
    Err(identifier_refusal_message(
        "DBSubnetGroupName",
        value,
        MAX_DB_SUBNET_GROUP_NAME_LEN,
    ))
}

/// The constraint walk behind [`check_db_subnet_group_name`]: a value of 1 to
/// [`MAX_DB_SUBNET_GROUP_NAME_LEN`] characters, drawn from the letters, numbers, periods,
/// underscores, spaces and hyphens the member names, whose first character is a letter.
fn db_subnet_group_name_is_well_formed(value: &str) -> bool {
    let length = value.chars().count();
    (1..=MAX_DB_SUBNET_GROUP_NAME_LEN).contains(&length)
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-'))
}

/// Whether `value` is written as an identifier TOKEN at all: it begins with an ASCII letter,
/// carries only the letters, digits and hyphens an identifier is drawn from, and is no
/// longer than `max_len` characters — the clauses a value has to satisfy to be an identifier
/// of its member's shape AT ALL.
///
/// This is the clause whose refusal is worded by the member's subject rather than by the
/// sentence pair (see [`check_identifier`]). It says nothing about the two HYPHEN rules,
/// which a value of this shape can still break.
fn is_an_identifier_token(value: &str, max_len: usize) -> bool {
    value.starts_with(|c: char| c.is_ascii_alphabetic())
        && is_drawn_from_the_identifier_alphabet(value)
        && value.chars().count() <= max_len
}

/// Whether every character of `value` is one an identifier of this front door is WRITTEN IN:
/// the ASCII letters, the digits and the hyphen, the alphabet every identifier member states
/// ("must contain only ASCII letters, digits, and hyphens").
///
/// A value with no characters at all is drawn from no alphabet: there is nothing in it to be
/// inside or outside this one.
fn is_drawn_from_the_identifier_alphabet(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The constraint walk behind [`check_identifier`]: whether `value` satisfies every clause
/// the identifier members state — that it is an identifier token at all
/// ([`is_an_identifier_token`]: the leading letter, the alphabet and the member's own length
/// bound) and the two hyphen rules. WHICH clause failed is what settles the wording of the
/// refusal, which is why [`check_identifier`] asks the token clause again.
fn identifier_is_well_formed(value: &str, max_len: usize) -> bool {
    is_an_identifier_token(value, max_len) && !value.ends_with('-') && !value.contains("--")
}

/// Refuse a `MaxRecords` outside the range every describe operation documents.
///
/// The caller answers this with `InvalidParameterValue` — the Query API common error Amazon
/// RDS itself answers a `MaxRecords` outside the documented window with ("Constraints:
/// Minimum 20, maximum 100" is a constraint on the VALUE of a parameter) — rather than with
/// a synthetic `ValidationException`, because the error code is what a client branches on.
pub fn check_max_records(max_records: Option<i32>) -> Result<usize, String> {
    check_max_records_in(max_records, &MAX_RECORDS_RANGE)
}

/// Refuse a `MaxRecords` outside `range`, for the operations whose own member documents
/// a different bound than [`MAX_RECORDS_RANGE`] — `DescribeOrderableDBInstanceOptions`
/// documents "Minimum 20, maximum 1000". The default page size and the wording of the
/// refusal stay here, shared by every describe operation.
///
/// The message is built from the request and the member's own documented window: the value
/// rejected, the member it was sent for and the bounds it must lie between, in the wording
/// Amazon RDS states that refusal with, so a caller can fix the request from the text alone.
pub fn check_max_records_in(
    max_records: Option<i32>,
    range: &std::ops::RangeInclusive<i32>,
) -> Result<usize, String> {
    match max_records {
        None => Ok(DEFAULT_MAX_RECORDS),
        Some(value) if range.contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "Invalid value {value} for MaxRecords. Must be between {} and {}",
            range.start(),
            range.end()
        )),
    }
}

/// Refuse a describe request carrying more filter values than [`MAX_FILTER_TERMS`].
///
/// The scan is the product of the stored record count and the filter terms, and it
/// runs with this service's one state lock held; the record count is capped by the
/// per-region quotas, and this caps the other factor.
pub fn check_filter_terms(filters: &[crate::ops::types::Filter]) -> Result<(), String> {
    let terms: usize = filters.iter().map(|filter| filter.values.len()).sum();
    if terms > MAX_FILTER_TERMS {
        return Err(format!(
            "Filters carries {terms} values: this emulator serves at most {MAX_FILTER_TERMS}"
        ));
    }
    Ok(())
}

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator accepts in a
/// caller-sized list whose length the model publishes no `@length` for.
///
/// The per-entry work such a list drives runs with this service's one state lock held, so
/// the count is the caller's to choose and has to be capped. It is comfortably above the
/// largest useful request: `ValuesToAdd` / `ValuesToRemove` cannot usefully carry more than
/// [`MAX_SNAPSHOT_SHARED_ACCOUNTS`] account ids plus [`SNAPSHOT_ATTRIBUTE_ALL`]. The check
/// itself is [`check_request_list_len`], the one this store already states that rule with.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// How many security group memberships one option of an option group may carry.
///
/// `OptionConfiguration$DBSecurityGroupMemberships` and
/// `OptionConfiguration$VpcSecurityGroupMemberships` publish no bound, and their values
/// are STORED, so they are capped for the same reason as [`MAX_FILTER_TERMS`]: request
/// data that enters the store has to be bounded. Generous next to the five security
/// groups AWS lets one DB instance carry.
pub const MAX_OPTION_SECURITY_GROUPS: usize = 32;

/// How long a text value a `ModifyOptionGroup` request may store: the value of an
/// option setting, or an `OptionVersion`. Neither member publishes a bound and both are
/// stored, so this emulator caps them.
pub const MAX_OPTION_TEXT_LEN: usize = 1024;

/// Refuse stored request text longer than [`MAX_OPTION_TEXT_LEN`], naming the member
/// and the bound it broke.
pub fn check_option_text(member: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length > MAX_OPTION_TEXT_LEN {
        return Err(format!(
            "{member} carries {length} characters: this emulator stores at most \
             {MAX_OPTION_TEXT_LEN}"
        ));
    }
    Ok(())
}

/// Refuse a `Tags` list this emulator will not store, for a member that hands them over as
/// an `Option`: the rule is [`check_tags`]'s, stated once for this store.
///
/// `TagList` carries no `@length`, so the list a caller sends is the caller's to size; it
/// is stored on a record, so the count and each entry are bounded before any of it is
/// kept. `Err` carries the message, naming the member and the bound it broke.
pub fn check_tag_list(tags: Option<&Vec<Tag>>) -> Result<(), String> {
    match tags {
        None => Ok(()),
        Some(tags) => check_tags(tags),
    }
}

/// Refuse a security group membership list longer than [`MAX_OPTION_SECURITY_GROUPS`],
/// and any entry that is not of the form a VPC security group id takes.
///
/// A DB SECURITY GROUP name is not checked here: that member carries no identifier pattern
/// at all, so a list of group names goes through [`check_db_security_group_names`].
pub fn check_security_group_memberships(member: &str, values: &[String]) -> Result<(), String> {
    if values.len() > MAX_OPTION_SECURITY_GROUPS {
        return Err(format!(
            "{member} carries {} entries: this emulator stores at most \
             {MAX_OPTION_SECURITY_GROUPS}",
            values.len()
        ));
    }
    for value in values {
        // A VPC security group id is drawn from the letters/numbers/hyphens alphabet the
        // identifier members share.
        check_identifier(member, value, 255)?;
    }
    Ok(())
}

/// How many entries one `ModifyOptionGroup` request may name in `OptionsToInclude`,
/// `OptionsToRemove`, or in one option configuration's `OptionSettings`.
///
/// None of those members publishes a bound, and the work a request costs is the PRODUCT
/// of its option count and its settings count, so each factor is capped BEFORE any
/// per-entry work runs — the same reason [`MAX_FILTER_TERMS`] caps a describe's filter
/// terms. The bound is generous: the whole catalogue publishes fewer options than this
/// for every engine together, and no option publishes more settings than this, so a
/// request past it is asking for options and settings that do not exist.
pub const MAX_OPTIONS_PER_REQUEST: usize = 64;

/// Refuse an option or option-settings list longer than [`MAX_OPTIONS_PER_REQUEST`],
/// naming the member and the bound it broke.
pub fn check_option_list_len(member: &str, len: usize) -> Result<(), String> {
    if len > MAX_OPTIONS_PER_REQUEST {
        return Err(format!(
            "{member} carries {len} entries: this emulator serves at most \
             {MAX_OPTIONS_PER_REQUEST} per request"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The reference catalogs this front door reports
//
// `DescribeCertificates` and `DescribeEventCategories` report what the service
// PUBLISHES rather than what a caller created, so their source is a catalog rather than
// the store above. Each catalog below names where its values come from, and every
// handler projects them through one of the `to_wire` functions here, so no two handlers
// can disagree about a CA certificate or an event category. (The DB engine version
// catalogue is the same kind of reference data and lives in
// `crate::ops::describe_db_engine_versions`, which owns it and lends it to
// `DescribeOrderableDBInstanceOptions`.)
//
// This front door serves the RDS, Amazon DocumentDB and Amazon Neptune control planes —
// the three models that share its endpoint prefix, signing name, protocol and API
// version (see `src/service.rs`) — so the catalogs cover all three, as AWS's own `rds`
// endpoint does.
// ---------------------------------------------------------------------------

/// The longest `SubscriptionName` this service accepts: "Constraints: The name must be
/// less than 255 characters" (`CreateEventSubscriptionMessage$SubscriptionName`), which
/// the model itself does not express.
pub const MAX_SUBSCRIPTION_NAME_LEN: usize = 254;

/// How many `EventCategories` entries one request may carry.
///
/// The member publishes no bound and every entry is STORED, so it is capped for the same
/// reason as [`MAX_FILTER_TERMS`]: request data that enters the store has to be bounded.
/// Generous next to the fourteen categories the largest published source type has.
pub const MAX_EVENT_CATEGORIES_PER_REQUEST: usize = 32;

/// The longest `EventCategories` entry this service stores. The published categories are
/// short phrases — the longest, `security patching`, is 17 characters — and a source type
/// this emulator publishes no category map for cannot have its entries resolved, so their
/// length is what bounds them.
pub const MAX_EVENT_CATEGORY_LEN: usize = 64;

/// How many `SourceIds` entries one request may carry.
///
/// Each entry is resolved against the stored resources of its source type, so the scan is
/// the PRODUCT of this count and the per-region record quota, under this service's one
/// state lock: this caps the factor the caller chooses, exactly as [`check_filter_terms`]
/// does for a describe. Generous next to the forty DB instances a region may hold.
pub const MAX_SOURCE_IDS_PER_REQUEST: usize = 64;

/// The longest source identifier this service stores: the bound the identifier members
/// of this service publish for the resources a source identifier names
/// (`DBInstanceIdentifier`, `DBSnapshotIdentifier`, `DBClusterIdentifier` — 255).
pub const MAX_SOURCE_ID_LEN: usize = 255;

/// How many source identifiers one subscription may hold at all.
///
/// AWS publishes no per-subscription source-identifier quota, so this is THIS emulator's
/// store guard rather than a modelled limit, and the 400 a handler refuses a further add
/// with is this service's own refusal rather than a modelled fault.
///
/// `CreateEventSubscription` already bounds the list it stores at
/// [`MAX_SOURCE_IDS_PER_REQUEST`]; a subscription whose sources are added one call at a
/// time has to be bounded by the same count, or the stored list grows without bound for
/// the source types this service resolves nothing against.
pub const MAX_SOURCE_IDS_PER_SUBSCRIPTION: usize = MAX_SOURCE_IDS_PER_REQUEST;

/// How many tags one resource may carry: Amazon RDS's published tag quota ("Tags per
/// resource": 50). Tags are stored request data, so the bound is enforced rather than
/// documented.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key AWS accepts: "Tag keys ... maximum length: 128 Unicode characters"
/// (Tagging AWS resources).
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value AWS accepts: "maximum length: 256 Unicode characters".
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// Refuse a request list longer than `max`, naming the member, the count and the bound it
/// broke — for the members that publish no bound of their own and whose entries are
/// stored or scanned.
pub fn check_request_list_len(member: &str, len: usize, max: usize) -> Result<(), String> {
    if len > max {
        return Err(format!(
            "{member} carries {len} entries: this emulator serves at most {max}"
        ));
    }
    Ok(())
}

/// Refuse stored request text longer than `max`, naming the member and the bound it broke.
pub fn check_stored_text(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} carries {length} characters: this emulator stores at most {max}"
        ));
    }
    Ok(())
}

/// Refuse a `Tags` list this service will not store: more tags than one resource may
/// carry, or a key or value past AWS's published tag lengths. Every tag a create stores is
/// request data, so all three bounds are checked before the store is touched.
///
/// The rules `Tag$Key` documents for the key ITSELF — that a key is required, and that
/// neither key nor value may carry a reserved prefix — belong to a caller's own tag write
/// and live in [`tags::check_tags_to_write`]: the tags a create carries include the ones
/// another service of this emulator assigns to a resource it provisions, which AWS assigns
/// under exactly those reserved prefixes.
pub fn check_tags(tags: &[Tag]) -> Result<(), String> {
    check_request_list_len("Tags", tags.len(), MAX_TAGS_PER_RESOURCE)?;
    for tag in tags {
        if let Some(key) = &tag.key {
            check_stored_text("Tags.Key", key, MAX_TAG_KEY_LEN)?;
        }
        if let Some(value) = &tag.value {
            check_stored_text("Tags.Value", value, MAX_TAG_VALUE_LEN)?;
        }
    }
    Ok(())
}

/// Refuse a `SubscriptionName` outside the constraint its member documents, naming the
/// member and the bound it broke.
pub fn check_subscription_name(name: &str) -> Result<(), String> {
    let length = name.chars().count();
    if length == 0 || length > MAX_SUBSCRIPTION_NAME_LEN {
        return Err(format!(
            "SubscriptionName must be less than {} characters; got {length}",
            MAX_SUBSCRIPTION_NAME_LEN + 1
        ));
    }
    Ok(())
}

/// Refuse an `SnsTopicArn` that is not an Amazon SNS topic ARN:
/// `arn:<partition>:sns:<region>:<account>:<topic name>`, the form
/// "`SnsTopicArn` — The Amazon Resource Name (ARN) of the SNS topic created for event
/// notification" takes.
///
/// Only the FORM is checked here. Whether a topic of that ARN exists is Amazon SNS's state,
/// which is asked of that service itself ([`sns_topic_is_absent`], the modelled
/// `SNSTopicArnNotFoundFault`). A value that is not an SNS topic ARN names no topic whatever
/// SNS holds, which is what the modelled `SNSInvalidTopicFault` says.
pub fn check_sns_topic_arn(arn: &str) -> Result<(), String> {
    let malformed = || {
        format!(
            "invalid value {arn} for SnsTopicArn: expected an Amazon SNS topic ARN of the \
             form arn:aws:sns:<region>:<account>:<topic name>"
        )
    };
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, topic] = fields[..] else {
        return Err(malformed());
    };
    if prefix != "arn"
        || partition.is_empty()
        || service != "sns"
        || account.is_empty()
        || !account.bytes().all(|b| b.is_ascii_digit())
        || topic.is_empty()
    {
        return Err(malformed());
    }
    check_region(region).map_err(|_| malformed())?;
    Ok(())
}
