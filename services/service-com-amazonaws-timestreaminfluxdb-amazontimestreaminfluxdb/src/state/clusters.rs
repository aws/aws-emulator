// SPDX-License-Identifier: CC0-1.0

//! The DB-cluster slice of this service's state.
//!
//! A DB cluster is a COMPOSITE resource: it provisions the DB instances its nodes are,
//! and those instances live in the same store every DB-instance operation reads
//! ([`super::DbInstance`]), so a node a cluster reports is a node the DB-instance
//! read paths return. This module owns the persisted cluster record plus the helpers a
//! cluster handler shares: minting the service-generated `id`, formatting the `arn` and
//! the two endpoints, deriving the engine and the default port from the DB parameter
//! group the create named, resolving the cluster's COMPUTE TOPOLOGY — the node groups the
//! create defined, else the ones the group's deprecated node-count parameters describe —
//! building each node's DB instance record, and settling the clock-owed statuses of a
//! cluster and its nodes.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ClusterConfiguration, DbBackupConfiguration, DbClusterSummary, DeploymentConfiguration,
    GetDbClusterOutput, InfluxDBv3EnterpriseDeploymentConfiguration, LogDeliveryConfiguration,
    MaintenanceSchedule, NodeGroup, Parameters, ValidationException,
};

use super::{
    availability_zones, backup_configurations_output, db_instance_arn, db_instance_endpoint,
    ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState, DbInstance, DbInstanceClusterRole,
    ARN_NAMESPACE, STATUS_CREATING, TRANSITION_SPAN,
    VALIDATION_EXCEPTION_REASON_FIELD_VALIDATION_FAILED,
};

/// `ClusterStatus` values this service's cluster records use, each one a member of
/// `crate::ops::types::CLUSTER_STATUS_VALUES` (the modelled `ClusterStatus` enum).
pub const CLUSTER_STATUS_CREATING: &str = "CREATING";
/// See [`CLUSTER_STATUS_CREATING`].
pub const CLUSTER_STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`CLUSTER_STATUS_CREATING`].
pub const CLUSTER_STATUS_RESTORING: &str = "RESTORING";

/// The cluster type a create that names none gets: the only member of the modelled
/// `ClusterDeploymentType` enum (`crate::ops::types::CLUSTER_DEPLOYMENT_TYPE_VALUES`).
pub const CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS: &str = "MULTI_NODE_READ_REPLICAS";

/// The failure-recovery behaviour a cluster gets when the create names none: recovery is
/// automatic unless the caller turns it off, the two members of the modelled
/// `FailoverMode` enum (`crate::ops::types::FAILOVER_MODE_VALUES`).
pub const FAILOVER_MODE_AUTOMATIC: &str = "AUTOMATIC";

/// The deployment type of a DB instance a cluster provisions: each node is deployed on
/// its own, a member of the modelled `DeploymentType` enum
/// (`crate::ops::types::DEPLOYMENT_TYPE_VALUES`) — what spreads a cluster's work over
/// several Availability Zones is its node set, not a standby beside one node.
pub const DEPLOYMENT_TYPE_SINGLE_AZ: &str = "SINGLE_AZ";

/// The `resourceType` this service reports on `ConflictException` /
/// `ResourceNotFoundException` for a DB cluster — a member of
/// `crate::ops::types::RESOURCE_TYPE_VALUES` (the modelled `ResourceType` enum).
pub const RESOURCE_TYPE_DB_CLUSTER: &str = "DB_CLUSTER";

/// The engines a DB parameter group's parameters configure, each one a member of
/// `crate::ops::types::ENGINE_TYPE_VALUES` (the modelled `EngineType` enum).
pub const ENGINE_TYPE_INFLUXDB_V2: &str = "INFLUXDB_V2";
/// See [`ENGINE_TYPE_INFLUXDB_V2`].
pub const ENGINE_TYPE_INFLUXDB_V3_CORE: &str = "INFLUXDB_V3_CORE";
/// See [`ENGINE_TYPE_INFLUXDB_V2`].
pub const ENGINE_TYPE_INFLUXDB_V3_ENTERPRISE: &str = "INFLUXDB_V3_ENTERPRISE";

/// The roles a cluster's node takes, each one a member of
/// `crate::ops::types::INSTANCE_MODE_VALUES` (the modelled `InstanceMode` enum).
pub const INSTANCE_MODE_INGEST: &str = "INGEST";
/// See [`INSTANCE_MODE_INGEST`].
pub const INSTANCE_MODE_QUERY: &str = "QUERY";
/// See [`INSTANCE_MODE_INGEST`].
pub const INSTANCE_MODE_COMPACT: &str = "COMPACT";

/// The port InfluxDB accepts connections on when the create names none, per engine, as
/// `CreateDbCluster`'s `port` documents ("Default: 8086 for InfluxDB v2, 8181 for
/// InfluxDB v3").
pub const DEFAULT_PORT_INFLUXDB_V2: i32 = 8086;
/// See [`DEFAULT_PORT_INFLUXDB_V2`].
pub const DEFAULT_PORT_INFLUXDB_V3: i32 = 8181;

/// The storage each of a cluster's nodes gets when the create names no
/// `allocatedStorage`: the floor of that member's modelled range (20..15360 GiB), which
/// is the least storage the service accepts for a node.
pub const DEFAULT_ALLOCATED_STORAGE: i32 = 20;

/// How long a DB cluster stays in a transient status before the clock settles it.
///
/// A cluster is composite: it is ready only once the DB instances it provisions are, so
/// it owes one [`TRANSITION_SPAN`] for its nodes and one more for itself — a caller that
/// reads straight after its nodes settled still sees the cluster coming up.
///
/// Derived from the one span this service settles a DB instance over rather than restating
/// it: one span per level of the resources a cluster depends on.
pub const CLUSTER_TRANSITION_SPAN: Duration = TRANSITION_SPAN.saturating_mul(2);

/// How many DB clusters the store holds before `CreateDbCluster` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Like [`super::DB_INSTANCE_QUOTA`], this is the emulator's OWN bound on the state
/// rather than a claim about any account's adjustable Service Quotas limit: it is what
/// stops the store (and the cluster name index beside it) growing without end, and it
/// sits far above any workflow an emulator serves. A cluster also consumes the DB
/// instance quota, one for each node it provisions.
pub const DB_CLUSTER_QUOTA: usize = 1_000;

/// The number of nodes that both ingest and query a cluster can have: the range the
/// model gives `InfluxDBv3EnterpriseParameters$ingestQueryInstances`, the member
/// `ClusterConfiguration$ingestQueryInstances` reports (1..4).
const INGEST_QUERY_NODE_RANGE: (i32, i32) = (1, 4);

/// The number of query-only nodes a cluster can have: the range the model gives
/// `InfluxDBv3EnterpriseParameters$queryOnlyInstances` (0..10). A group that configures
/// that family is taken at its word, including a configuration with no query-only node
/// at all.
const QUERY_ONLY_NODE_RANGE: (i32, i32) = (0, 10);

/// The nodes that both ingest and query a group of the InfluxDB v3 Enterprise family
/// configures when it sets none: the `default` the model gives
/// `InfluxDBv3EnterpriseParameters$ingestQueryInstances` (1).
const MODELLED_DEFAULT_INGEST_QUERY_NODES: i32 = 1;

/// The query-only nodes such a group configures when it sets none: the `default` the
/// model gives `InfluxDBv3EnterpriseParameters$queryOnlyInstances` (0).
const MODELLED_DEFAULT_QUERY_ONLY_NODES: i32 = 0;

/// Whether such a group puts compaction on a standalone node when it sets nothing: the
/// `default` the model gives `InfluxDBv3EnterpriseParameters$dedicatedCompactor` (false).
const MODELLED_DEFAULT_DEDICATED_COMPACTOR: bool = false;

/// The query-only nodes a read-replicas cluster gets when the group applied to it
/// carries no node parameters: one read replica beside the node that ingests, the
/// smallest composition the deployment type describes.
const DEFAULT_QUERY_ONLY_NODES: i32 = 1;

/// How many nodes one node group holds: the range the model gives `NodeGroup$count`
/// (1..13).
const NODE_GROUP_COUNT_RANGE: (i32, i32) = (1, 13);

/// How many node groups one topology holds: the maximum of the modelled `NodeGroupList`
/// length (1..10).
const NODE_GROUP_LIST_MAX: usize = 10;

/// The longest a DB instance name may be: the maximum of the modelled `DbInstanceName`
/// length (3..40), which the service-generated name of a cluster's node also obeys.
const DB_INSTANCE_NAME_MAX_LEN: usize = 40;

/// What the reader endpoint of a cluster carries beside the writer endpoint's host, so
/// the two addresses are distinct.
const CLUSTER_READER_LABEL: &str = "ro";

/// One persisted Timestream for InfluxDB DB cluster: the single source of truth every
/// DB-cluster operation reads and writes.
#[derive(Debug, Clone)]
pub struct DbCluster {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region this cluster was created in; its ARN, endpoints and nodes are all
    /// scoped to it, so a later operation derives them from here rather than from the
    /// calling request.
    pub region: String,
    pub status: String,
    pub endpoint: String,
    pub reader_endpoint: String,
    pub port: i32,
    pub deployment_type: String,
    /// The one instance type this cluster reports for itself: the create's own
    /// `dbInstanceType` where it named one, and otherwise the type its node groups run
    /// (see [`node_groups_instance_type`]) — a topology sizes each group on its own, so
    /// the cluster-level member is that projection of it. Every NODE runs its own group's
    /// type, which is what [`DbCluster::node_instance`] gives it.
    pub db_instance_type: String,
    pub network_type: Option<String>,
    pub db_storage_type: Option<String>,
    pub allocated_storage: i32,
    /// The engine this cluster runs, derived from the parameters of the DB parameter
    /// group applied to it (see [`engine_type`]).
    pub engine_type: String,
    pub publicly_accessible: Option<bool>,
    pub db_parameter_group_identifier: Option<String>,
    /// The group actually applied. This emulator applies no service-managed optimized
    /// default, so it matches `db_parameter_group_identifier`, as
    /// `effectiveDbParameterGroupIdentifier` documents for that case ("When no
    /// service-managed DB parameter group is applied, this value matches
    /// dbParameterGroupIdentifier").
    pub effective_db_parameter_group_identifier: Option<String>,
    pub log_delivery_configuration: Option<LogDeliveryConfiguration>,
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    pub influx_auth_parameters_secret_arn: String,
    pub vpc_subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub failover_mode: String,
    /// The compute topology this cluster was provisioned with: the node groups the create
    /// defined, else the ones the node-count parameters of the applied DB parameter group
    /// describe (see [`deployment_node_groups`] and [`configuration_node_groups`]). The
    /// nodes in [`DbCluster::db_instance_ids`] are what it describes, one per node of
    /// every group, so it is never empty.
    pub node_groups: Vec<NodeGroup>,
    pub db_backup_configurations: Option<Vec<DbBackupConfiguration>>,
    pub kms_key_id: Option<String>,
    /// Stored because `CreateDbCluster` accepts them for the initial InfluxDB admin
    /// user; they are not members of any DB-cluster output shape, so nothing echoes them
    /// back. The `password` is deliberately NOT kept: it is `@sensitive`, no output shape
    /// carries it, and the record only needs the secret ARN that stands for it.
    pub username: Option<String>,
    pub organization: Option<String>,
    pub bucket: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The ids of the DB instances this cluster provisioned, in creation order. They are
    /// the cluster's nodes, and they are stored in
    /// [`ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState::db_instances`]
    /// like any other DB instance.
    pub db_instance_ids: Vec<String>,
    pub created_at: SystemTime,
    /// When the clock owes this record its settle to `AVAILABLE`; `None` once settled.
    pub settles_at: Option<SystemTime>,
}

impl DbCluster {
    /// Materialise the status change the clock already made due.
    ///
    /// A reporting operation calls this before answering: the transition was owed at
    /// `settles_at` whether or not anybody called, so applying it on read is not a
    /// read-time mutation of the resource's own lifecycle.
    pub fn settle(&mut self, now: SystemTime) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = CLUSTER_STATUS_AVAILABLE.to_string();
            self.settles_at = None;
        }
    }

    /// The compute topology of this cluster as `deploymentConfiguration` reports it: the
    /// node groups it actually provisioned, under the union member that expresses a
    /// topology.
    ///
    /// Every cluster of this service is composed of node groups, whichever way its create
    /// described them, and the modelled `DeploymentConfiguration` union declares one
    /// member per engine topology this API expresses — today exactly one
    /// (`smithy_query(query="shape", shape="DeploymentConfiguration")`), the member whose
    /// node groups are the shape every topology here is expressed in — so the topology is
    /// reported under it, as the member documents ("The returned topology reflects the
    /// cluster's actual provisioned nodes").
    pub fn deployment_configuration(&self) -> Option<DeploymentConfiguration> {
        (!self.node_groups.is_empty()).then(|| {
            DeploymentConfiguration::InfluxDBv3Enterprise(
                InfluxDBv3EnterpriseDeploymentConfiguration {
                    node_groups: self.node_groups.clone(),
                },
            )
        })
    }

    /// The same topology as the deprecated `clusterConfiguration` reports it: the node
    /// counts [`node_groups_cluster_configuration`] reads off this cluster's node groups.
    pub fn cluster_configuration(&self) -> ClusterConfiguration {
        node_groups_cluster_configuration(&self.node_groups)
    }

    /// Project this stored record into `GetDbCluster`'s output shape. Every member comes
    /// from the record; nothing is recomputed from the request.
    pub fn to_get_output(&self) -> GetDbClusterOutput {
        GetDbClusterOutput {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
            reader_endpoint: Some(self.reader_endpoint.clone()),
            port: Some(self.port),
            deployment_type: Some(self.deployment_type.clone()),
            db_instance_type: Some(self.db_instance_type.clone()),
            network_type: self.network_type.clone(),
            db_storage_type: self.db_storage_type.clone(),
            allocated_storage: Some(self.allocated_storage),
            engine_type: Some(self.engine_type.clone()),
            publicly_accessible: self.publicly_accessible,
            db_parameter_group_identifier: self.db_parameter_group_identifier.clone(),
            effective_db_parameter_group_identifier: self
                .effective_db_parameter_group_identifier
                .clone(),
            log_delivery_configuration: self.log_delivery_configuration.clone(),
            maintenance_schedule: self.maintenance_schedule.clone(),
            // No maintenance has run and none is scheduled by this slice.
            last_maintenance_time: None,
            next_maintenance_time: None,
            influx_auth_parameters_secret_arn: Some(
                self.influx_auth_parameters_secret_arn.clone(),
            ),
            vpc_subnet_ids: Some(self.vpc_subnet_ids.clone()),
            vpc_security_group_ids: Some(self.vpc_security_group_ids.clone()),
            failover_mode: Some(self.failover_mode.clone()),
            cluster_configuration: Some(self.cluster_configuration()),
            deployment_configuration: self.deployment_configuration(),
            db_backup_configurations: backup_configurations_output(&self.db_backup_configurations),
            kms_key_id: self.kms_key_id.clone(),
        }
    }

    /// Project this stored record into the `DbClusterSummary` the list operation reports.
    ///
    /// Every member of that shape is a member of this record; the summary is the subset
    /// of [`Self::to_get_output`], so one cluster never reports two different values for
    /// the same member.
    pub fn to_summary(&self) -> DbClusterSummary {
        DbClusterSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
            reader_endpoint: Some(self.reader_endpoint.clone()),
            port: Some(self.port),
            deployment_type: Some(self.deployment_type.clone()),
            db_instance_type: Some(self.db_instance_type.clone()),
            network_type: self.network_type.clone(),
            db_storage_type: self.db_storage_type.clone(),
            allocated_storage: Some(self.allocated_storage),
            engine_type: Some(self.engine_type.clone()),
        }
    }

    /// The DB instance record one of this cluster's nodes is.
    ///
    /// Every member comes from the cluster or from the NODE GROUP the node belongs to: a
    /// node runs its group's instance type and the cluster's storage, network type and
    /// VPC placement, on the cluster's port, under the parameter group applied to the
    /// cluster and with the cluster's initial InfluxDB credentials. It is provisioning, so
    /// the clock owes it the settle to `AVAILABLE` every new DB instance gets, and the
    /// cluster settles only after that.
    pub fn node_instance(
        &self,
        node: &ClusterNode,
        instance_id: &str,
        now: SystemTime,
    ) -> DbInstance {
        let (availability_zone, secondary_availability_zone) =
            availability_zones(&self.region, Some(DEPLOYMENT_TYPE_SINGLE_AZ));
        DbInstance {
            id: instance_id.to_string(),
            name: node.name.clone(),
            arn: db_instance_arn(&self.region, instance_id),
            region: self.region.clone(),
            status: STATUS_CREATING.to_string(),
            endpoint: db_instance_endpoint(&node.name, instance_id, &self.region),
            port: self.port,
            // A node of a DB cluster is reached through the CLUSTER's own endpoints, which
            // this emulator serves no socket for, so a node publishes no socket of its own
            // and keeps the host Timestream for InfluxDB's own zone would carry.
            served_endpoint: None,
            served_port: None,
            network_type: self.network_type.clone(),
            db_instance_type: node.instance_type.clone(),
            db_storage_type: self.db_storage_type.clone(),
            allocated_storage: self.allocated_storage,
            deployment_type: Some(DEPLOYMENT_TYPE_SINGLE_AZ.to_string()),
            vpc_subnet_ids: self.vpc_subnet_ids.clone(),
            vpc_security_group_ids: self.vpc_security_group_ids.clone(),
            publicly_accessible: self.publicly_accessible,
            db_parameter_group_identifier: self.effective_db_parameter_group_identifier.clone(),
            availability_zone: Some(availability_zone),
            secondary_availability_zone,
            log_delivery_configuration: self.log_delivery_configuration.clone(),
            maintenance_schedule: self.maintenance_schedule.clone(),
            db_backup_configurations: self.db_backup_configurations.clone(),
            influx_auth_parameters_secret_arn: self.influx_auth_parameters_secret_arn.clone(),
            kms_key_id: self.kms_key_id.clone(),
            username: self.username.clone(),
            organization: self.organization.clone(),
            bucket: self.bucket.clone(),
            // The create request's tags are attached to the cluster, the resource the
            // caller named; nothing in the model attaches them to its nodes as well.
            tags: BTreeMap::new(),
            created_at: now,
            settles_at: Some(now + TRANSITION_SPAN),
            cluster: Some(DbInstanceClusterRole {
                db_cluster_id: self.id.clone(),
                instance_modes: node.instance_modes.clone(),
            }),
        }
    }
}

/// One node of a DB cluster: the service-generated name of the DB instance the cluster
/// provisions for it, the roles that instance takes in the cluster, and the instance type
/// it runs.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub name: String,
    /// The roles this node takes, the first of them the role `instanceMode` reports.
    /// Never empty.
    pub instance_modes: Vec<String>,
    /// The Timestream for InfluxDB instance type this node runs: the one its node group
    /// names, since `NodeGroup$instanceType` is what sizes the nodes of a group.
    pub instance_type: String,
}

/// The nodes a cluster named `cluster_name` with id `cluster_id` whose topology is
/// `node_groups` provisions, in creation order: every node of the first group, then of
/// the next, so a node's ordinal follows the order the topology lists its groups in.
///
/// One node per unit of `NodeGroup$count`, each taking that group's roles and instance
/// type: the DB instances a cluster is composed of ARE its node groups' nodes.
pub fn cluster_nodes(
    cluster_name: &str,
    cluster_id: &str,
    node_groups: &[NodeGroup],
) -> Vec<ClusterNode> {
    node_groups
        .iter()
        .flat_map(|group| {
            (0..group.count.max(0)).map(|_| (group.instance_modes.clone(), group.instance_type.clone()))
        })
        .enumerate()
        .map(|(position, (instance_modes, instance_type))| ClusterNode {
            name: node_name(cluster_name, cluster_id, position + 1),
            instance_modes,
            instance_type,
        })
        .collect()
}

/// The service-generated name of a cluster's node: the cluster's own name, the cluster's
/// id and the node's ordinal, as `DbInstanceForClusterSummary$name` documents ("A
/// service-generated name for the DB instance based on the customer-supplied name for the
/// DB cluster").
///
/// The id is in the name because two different cluster names can share the prefix that
/// survives the trim below, and a node name is a DB instance name, which is unique per
/// Region: without it one cluster's nodes could take the names another cluster's nodes
/// need. Kept inside the modelled `DbInstanceName` length by trimming the cluster-name
/// prefix, and never left ending in the separator, so the generated name satisfies that
/// member's pattern as well.
fn node_name(cluster_name: &str, cluster_id: &str, ordinal: usize) -> String {
    let suffix = format!("-{cluster_id}-{ordinal}");
    let room = DB_INSTANCE_NAME_MAX_LEN.saturating_sub(suffix.len());
    let prefix: String = cluster_name.chars().take(room).collect();
    format!("{}{suffix}", prefix.trim_end_matches('-'))
}

/// The node composition of a read-replicas cluster whose applied DB parameter group
/// configures `parameters`, as the DEPRECATED `ClusterConfiguration` expresses it.
///
/// `ClusterConfiguration`'s three members are exactly the three node parameters the
/// InfluxDB v3 Enterprise family carries (`ingestQueryInstances`, `queryOnlyInstances`,
/// `dedicatedCompactor`), so a cluster whose parameter group configures that family is
/// composed from them — each one defaulted to the `default` the model gives that member
/// and clamped into its range. Any other family carries no node parameters, so such a
/// cluster — and one whose create named no group at all — gets the smallest read-replicas
/// composition: one node that ingests and queries, one that only queries, and no
/// standalone compactor.
pub fn parameter_cluster_configuration(parameters: Option<&Parameters>) -> ClusterConfiguration {
    let (ingest_query, query_only, dedicated_compactor) = match parameters {
        Some(Parameters::InfluxDBv3Enterprise(enterprise)) => (
            enterprise
                .ingest_query_instances
                .unwrap_or(MODELLED_DEFAULT_INGEST_QUERY_NODES),
            enterprise
                .query_only_instances
                .unwrap_or(MODELLED_DEFAULT_QUERY_ONLY_NODES),
            enterprise
                .dedicated_compactor
                .unwrap_or(MODELLED_DEFAULT_DEDICATED_COMPACTOR),
        ),
        _ => (
            INGEST_QUERY_NODE_RANGE.0,
            DEFAULT_QUERY_ONLY_NODES,
            MODELLED_DEFAULT_DEDICATED_COMPACTOR,
        ),
    };
    ClusterConfiguration {
        ingest_query_instances: Some(
            ingest_query.clamp(INGEST_QUERY_NODE_RANGE.0, INGEST_QUERY_NODE_RANGE.1),
        ),
        query_only_instances: Some(
            query_only.clamp(QUERY_ONLY_NODE_RANGE.0, QUERY_ONLY_NODE_RANGE.1),
        ),
        dedicated_compactor: Some(dedicated_compactor),
    }
}

/// The node groups a `deploymentConfiguration` defines, the topology a request that
/// carries one asks for.
///
/// It is the SOURCE OF TRUTH for the cluster's topology, as the member documents ("A
/// deployment configuration becomes the source of truth for the cluster's topology.
/// Timestream for InfluxDB then ignores any node-count parameters in the associated DB
/// parameter group"), so nothing here consults the applied parameter group.
///
/// The topology is held to the size the model gives it — at most `NodeGroupList`'s ten
/// groups, each of at most `NodeGroup$count`'s thirteen nodes. The framework's boundary
/// walk refuses a request outside those declared bounds before this runs, so that is
/// unreachable for a request that crossed the wire; the bound here keeps the node set ONE
/// call can provision inside the model for an in-crate caller too (a restore rebuilding a
/// captured topology).
pub fn deployment_node_groups(configuration: &DeploymentConfiguration) -> Vec<NodeGroup> {
    let groups = match configuration {
        DeploymentConfiguration::InfluxDBv3Enterprise(enterprise) => &enterprise.node_groups,
    };
    groups
        .iter()
        .take(NODE_GROUP_LIST_MAX)
        .map(|group| NodeGroup {
            count: group
                .count
                .clamp(NODE_GROUP_COUNT_RANGE.0, NODE_GROUP_COUNT_RANGE.1),
            ..group.clone()
        })
        .collect()
}

/// The node groups a cluster composed as the deprecated `configuration` says is
/// provisioned with, every node of them running `db_instance_type`: one group of the
/// nodes that both ingest and query, one of the query-only read replicas where the
/// composition asks for any, and one of the standalone compactor where it asks for one.
///
/// This is the deprecated node-count view expressed as the topology that replaces it, so
/// one cluster has ONE stored topology whichever way its create described it.
pub fn configuration_node_groups(
    configuration: &ClusterConfiguration,
    db_instance_type: &str,
) -> Vec<NodeGroup> {
    let group = |modes: &[&str], count: i32| NodeGroup {
        instance_modes: modes.iter().map(|mode| (*mode).to_string()).collect(),
        instance_type: db_instance_type.to_string(),
        count,
    };
    let ingest_query = configuration
        .ingest_query_instances
        .unwrap_or(MODELLED_DEFAULT_INGEST_QUERY_NODES)
        // Held to the maxima the model gives those two members, so the node set one
        // request provisions is bounded here as well as where a composition is derived.
        .min(INGEST_QUERY_NODE_RANGE.1);
    let query_only = configuration
        .query_only_instances
        .unwrap_or(MODELLED_DEFAULT_QUERY_ONLY_NODES)
        .min(QUERY_ONLY_NODE_RANGE.1);
    let mut node_groups = Vec::new();
    if ingest_query > 0 {
        node_groups.push(group(
            &[INSTANCE_MODE_INGEST, INSTANCE_MODE_QUERY],
            ingest_query,
        ));
    }
    if query_only > 0 {
        node_groups.push(group(&[INSTANCE_MODE_QUERY], query_only));
    }
    if configuration.dedicated_compactor.unwrap_or_default() {
        node_groups.push(group(&[INSTANCE_MODE_COMPACT], 1));
    }
    node_groups
}

/// The node groups a cluster whose applied DB parameter group configures `parameters` is
/// provisioned with when its create defined no topology of its own: the deprecated
/// node-count composition those parameters describe, as node groups running
/// `db_instance_type`.
pub fn parameter_node_groups(
    parameters: Option<&Parameters>,
    db_instance_type: &str,
) -> Vec<NodeGroup> {
    configuration_node_groups(
        &parameter_cluster_configuration(parameters),
        db_instance_type,
    )
}

/// The deprecated `ClusterConfiguration` view of a topology: how many of its nodes both
/// ingest and query, how many only query, and whether compaction has a standalone node.
///
/// Those three members are the whole of that shape, so a role combination they cannot
/// express — a node that only ingests, or one that runs the processing engine — is
/// counted by none of them: that is what a view the topology outgrew can report. The
/// topology itself is reported in full by `deploymentConfiguration`.
///
/// The counts are the topology's own, NOT clamped: `ClusterConfiguration`'s three members
/// each target the plain `Integer` and declare no `@range`
/// (`smithy_query(query="shape", shape="ClusterConfiguration")`), unlike the deprecated
/// node-count PARAMETERS [`parameter_cluster_configuration`] reads, which do declare one
/// and are held to it there. So a topology with no node that both ingests and queries
/// reports zero of them, and one with thirteen read replicas reports thirteen: the number
/// of nodes the cluster actually has, which is what this shape reports about it.
pub fn node_groups_cluster_configuration(node_groups: &[NodeGroup]) -> ClusterConfiguration {
    let nodes_with = |wanted: &[&str], without: &[&str]| -> i32 {
        node_groups
            .iter()
            .filter(|group| {
                wanted
                    .iter()
                    .all(|mode| group.instance_modes.iter().any(|held| held == mode))
                    && !without
                        .iter()
                        .any(|mode| group.instance_modes.iter().any(|held| held == mode))
            })
            .map(|group| group.count.max(0))
            .sum()
    };
    ClusterConfiguration {
        ingest_query_instances: Some(nodes_with(
            &[INSTANCE_MODE_INGEST, INSTANCE_MODE_QUERY],
            &[],
        )),
        query_only_instances: Some(nodes_with(
            &[INSTANCE_MODE_QUERY],
            &[INSTANCE_MODE_INGEST],
        )),
        dedicated_compactor: Some(node_groups.iter().any(|group| {
            group.instance_modes.len() == 1 && group.instance_modes[0] == INSTANCE_MODE_COMPACT
        })),
    }
}

/// The one instance type a cluster whose topology is `node_groups` reports as its own
/// `dbInstanceType`: the type every group runs where they all run the same one, and
/// otherwise the type of the first group whose nodes INGEST — the cluster's write path.
///
/// A cluster reports a single `dbInstanceType` while a topology sizes each node group on
/// its own (`CreateDbClusterInput$dbInstanceType`: "Optional when you define the cluster
/// topology with deploymentConfiguration, because the instance type is then specified per
/// node group"), so the cluster-level member is this projection of the topology. WHICH
/// type a topology running several projects to is this emulator's own choice — neither the
/// model nor the documentation states one — so it takes the write path's, the type the
/// nodes that ingest run.
/// `None` only for an empty topology, which no cluster has: the modelled `nodeGroups` is
/// required and holds 1..10 groups.
pub fn node_groups_instance_type(node_groups: &[NodeGroup]) -> Option<String> {
    let first = node_groups.first()?;
    if node_groups
        .iter()
        .all(|group| group.instance_type == first.instance_type)
    {
        return Some(first.instance_type.clone());
    }
    node_groups
        .iter()
        .find(|group| {
            group
                .instance_modes
                .iter()
                .any(|mode| mode == INSTANCE_MODE_INGEST)
        })
        .map(|group| group.instance_type.clone())
        .or_else(|| Some(first.instance_type.clone()))
}

/// The refusal a `CreateDbCluster` gets that names neither the instance type its nodes
/// run nor a topology that names one per node group.
///
/// `dbInstanceType` is modelled optional, but only conditionally so, as it documents
/// ("Optional when you define the cluster topology with deploymentConfiguration, because
/// the instance type is then specified per node group") — so the framework's boundary
/// walk admits a request that sizes nothing, and the rule is stated here. A field of the
/// request is what is missing, so the `reason` the model marks required is the
/// field-validation member rather than the catch-all. The RULE is the model's; the
/// sentence that states it is this emulator's own, restating the condition the member
/// documents rather than reproducing a wording observed on the wire.
#[must_use]
pub fn db_instance_type_required() -> ValidationException {
    ValidationException {
        message: "One of dbInstanceType or deploymentConfiguration must be specified. The \
                  instance type is specified per node group when a deployment \
                  configuration defines the cluster topology"
            .to_string(),
        reason: VALIDATION_EXCEPTION_REASON_FIELD_VALIDATION_FAILED.to_string(),
    }
}

/// The engine a DB parameter group's `parameters` configure: one member of the modelled
/// `EngineType` enum per member of the modelled `Parameters` union.
///
/// A create that named no group runs the default engine, InfluxDB v2 — the family
/// [`super::default_db_parameter_group_parameters`] stands for.
pub fn engine_type(parameters: Option<&Parameters>) -> &'static str {
    match parameters {
        None | Some(Parameters::InfluxDBv2(_)) => ENGINE_TYPE_INFLUXDB_V2,
        Some(Parameters::InfluxDBv3Core(_)) => ENGINE_TYPE_INFLUXDB_V3_CORE,
        Some(Parameters::InfluxDBv3Enterprise(_)) => ENGINE_TYPE_INFLUXDB_V3_ENTERPRISE,
    }
}

/// The port InfluxDB accepts connections on when the create names none: per the `port`
/// member's documented default, 8086 on the v2 engine and 8181 on the v3 engines.
pub fn default_port(engine_type: &str) -> i32 {
    if engine_type == ENGINE_TYPE_INFLUXDB_V2 {
        DEFAULT_PORT_INFLUXDB_V2
    } else {
        DEFAULT_PORT_INFLUXDB_V3
    }
}

/// The ARN of a DB cluster:
/// `arn:aws:timestream-influxdb:<region>:<account>:db-cluster/<id>`, one of the four
/// resource segments the modelled `Arn` pattern allows.
pub fn db_cluster_arn(region: &str, id: &str) -> String {
    super::family_arn(super::ARN_SEGMENT_DB_CLUSTER, region, id)
}

/// The two endpoints a cluster is reached on: the one that serves writes and reads, and
/// the reader endpoint that serves read-only traffic.
///
/// Both carry the cluster name as their prefix, as `CreateDbCluster`'s `name` documents
/// ("This name will also be a prefix included in the endpoint"), then the cluster id so
/// two clusters that reused a name across Regions never share an endpoint.
pub fn db_cluster_endpoints(name: &str, id: &str, region: &str) -> (String, String) {
    (
        format!("{name}-{id}.{ARN_NAMESPACE}.{region}.on.aws"),
        format!("{name}-{id}-{CLUSTER_READER_LABEL}.{ARN_NAMESPACE}.{region}.on.aws"),
    )
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Mint the next service-generated DB cluster id: 10 lowercase hex characters, which
    /// satisfies the modelled `DbClusterId` pattern `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_cluster_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_cluster_seq += 1;
            let seq = self.db_cluster_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-cluster|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_clusters.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the live DB cluster holding `name` in `region`, if any.
    pub fn db_cluster_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.db_cluster_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_cluster_name(&mut self, region: &str, name: &str, id: String) {
        self.db_cluster_names
            .insert((region.to_string(), name.to_string()), id);
    }

    /// Materialise the status changes the clock already made due for the cluster `id`
    /// and the DB instances it provisioned.
    ///
    /// The nodes settle first and the cluster after them, which is the order their due
    /// instants were recorded in: a composite resource is ready only once what it
    /// provisions is.
    pub fn settle_db_cluster(&mut self, id: &str, now: SystemTime) {
        let Some(node_ids) = self.db_clusters.get(id).map(|c| c.db_instance_ids.clone()) else {
            return;
        };
        for node_id in node_ids {
            if let Some(node) = self.db_instances.get_mut(&node_id) {
                node.settle(now);
            }
        }
        if let Some(cluster) = self.db_clusters.get_mut(id) {
            cluster.settle(now);
        }
    }
}
