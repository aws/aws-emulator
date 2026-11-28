// SPDX-License-Identifier: CC0-1.0

//! The restore slice of this service's state: how a DB backup becomes a DB resource
//! again.
//!
//! A backup holds the whole resource it was taken from — its configuration and, for a DB
//! instance, the InfluxDB data its own plane served
//! ([`super::DbBackupSnapshot`]) — so restoring one BUILDS a resource out of those
//! captured values. Nothing is recomputed from the backup's own identity and nothing is
//! invented: every member of the restored record comes from the snapshot, from the restore
//! request where it overrode one, or from the restored resource's own new identity (its
//! id, ARN, endpoint and secret).
//!
//! Two choices decide what a restore produces, and both are the request's:
//!
//! * the DEPLOYMENT TYPE — the one the request asks for, else the one the backup captured
//!   — decides which FAMILY the restored resource belongs to, because the modelled
//!   `ResourceDeploymentType` spans both families: the read-replicas type is the cluster
//!   deployment type, the other two are the DB-instance ones;
//! * the RESTORE MODE decides whether the resource is a new one or the one that already
//!   holds the name: `NEW_RESOURCE` (the default) mints a resource, and
//!   `REPLACE_EXISTING` restores into the resource the name resolves to, which keeps its
//!   id, its ARN and the address a client is already dialling it at.
//!
//! Either way the restored resource is RESTORING when the call answers and the clock
//! settles it, exactly as a create does: a restore is work the service does, not an
//! instant event.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    ConflictException, DbBackupConfiguration, LogDeliveryConfiguration, MaintenanceSchedule,
    ResourceNotFoundException, ServiceQuotaExceededException, ValidationException,
};

use super::influx_data_plane::TimeSeriesSnapshot;
use super::{
    availability_zones, cluster_nodes, configuration_node_groups, db_cluster_arn,
    db_cluster_endpoints, db_instance_arn, db_instance_endpoint, deployment_node_groups,
    influx_auth_parameters_secret_arn, marketplace_offering_for_deployment_type,
    name_already_exists, parameter_cluster_configuration, service_managed_db_parameter_group,
    ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState, DbBackup, DbCluster, DbInstance,
    NamedResource, ARN_SEGMENT_DB_CLUSTER, ARN_SEGMENT_DB_INSTANCE,
    CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS, CLUSTER_STATUS_RESTORING, CLUSTER_TRANSITION_SPAN,
    DB_CLUSTER_QUOTA, DB_INSTANCE_QUOTA, DEPLOYMENT_TYPE_SINGLE_AZ, FAILOVER_MODE_AUTOMATIC,
    NAMED_DB_CLUSTER, NAMED_DB_INSTANCE, RESOURCE_TYPE_DB_CLUSTER, STATUS_RESTORING,
    TRANSITION_SPAN, VALIDATION_EXCEPTION_REASON_OTHER,
};

/// The two modes a restore runs in, the members of the modelled `RestoreMode` enum
/// (`crate::ops::types::RESTORE_MODE_VALUES`). `NEW_RESOURCE` is the default, as
/// `restoreMode` documents ("Valid values are NEW_RESOURCE (default) and
/// REPLACE_EXISTING").
pub const RESTORE_MODE_NEW_RESOURCE: &str = "NEW_RESOURCE";
/// See [`RESTORE_MODE_NEW_RESOURCE`].
pub const RESTORE_MODE_REPLACE_EXISTING: &str = "REPLACE_EXISTING";

/// The status a restore reports for itself: the only member of the modelled
/// `RestoreStatus` enum (`crate::ops::types::RESTORE_STATUS_VALUES`).
pub const RESTORE_STATUS_RESTORING: &str = "RESTORING";

/// What a restore request asks to CHANGE about the resource its backup captured.
///
/// Every member is optional because the backup carries one of its own: "If not specified,
/// the restored resource uses the same security groups as the backup" is the rule for all
/// of them, so an absent member leaves the captured value standing.
#[derive(Debug, Clone, Default)]
pub struct RestoreOverrides {
    pub deployment_type: Option<String>,
    pub vpc_subnet_ids: Option<Vec<String>>,
    pub vpc_security_group_ids: Option<Vec<String>>,
    pub publicly_accessible: Option<bool>,
    pub log_delivery_configuration: Option<LogDeliveryConfiguration>,
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    pub port: Option<i32>,
    pub network_type: Option<String>,
    pub db_backup_configurations: Option<Vec<DbBackupConfiguration>>,
    pub kms_key_id: Option<String>,
    /// The tags to associate with the restored resource. Empty where the request named
    /// none, in which case a resource restored in place keeps the tags it has.
    pub tags: BTreeMap<String, String>,
}

/// What a restore produced: the resource it restored into and what the operation's output
/// reports about it.
#[derive(Debug, Clone)]
pub struct Restored {
    /// The id of the restored resource — a newly minted one, or the id of the resource the
    /// restore replaced.
    pub resource_id: String,
    /// Which family that resource belongs to, in the spelling
    /// [`restored_resource_type`] gives it.
    pub resource_type: &'static str,
    /// The engine it runs, the one the backup captured.
    pub engine_type: String,
    /// The deployment type it was restored with.
    pub deployment_type: String,
    /// The InfluxDB data the restored DB instance's own plane must serve, when the backup
    /// captured any and the restored resource is a DB instance. The caller publishes it
    /// once the store's lock is released, because opening a socket is IO.
    pub data: Option<TimeSeriesSnapshot>,
}

/// Why a restore was refused, in the modelled terms the operation answers with.
#[derive(Debug)]
pub enum RestoreRefusal {
    NotFound(ResourceNotFoundException),
    Conflict(ConflictException),
    QuotaExceeded(ServiceQuotaExceededException),
    Invalid(ValidationException),
}

/// The refusal a restore into an EXISTING resource gets when no resource of that family
/// holds the name it asked for.
///
/// `name` is documented "If restoring to an existing resource, the name must match the
/// existing resource name", so with that restore mode the name is a lookup and a name
/// nothing holds resolves to no resource to restore into. The sentence is this service's
/// own already-exists clause vocabulary ([`name_already_exists`]) in the negative: it
/// names the resource, the name the request asked for and the customer the records belong
/// to, and the `resourceId` and `resourceType` the model marks required carry the name
/// asked for and the family looked in.
#[must_use]
pub fn named_resource_not_found(
    resource: NamedResource,
    requested_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "The {} with name [{requested_name}], customer [{}] does not exist",
            resource.spelling,
            ::engine::emulator_account_id(),
        ),
        resource_id: requested_name.to_string(),
        resource_type: resource.resource_type.to_string(),
    }
}

/// The deployment type a restore of `backup` gives the restored resource: the one the
/// request asked for, else the one the backup captured, else the default of the family the
/// backup was taken from.
///
/// A backup captures the source's deployment type, and a source that never named one (a
/// DB instance created without it) is a standalone single-AZ resource, which is what a
/// restore of it produces.
#[must_use]
pub fn restored_deployment_type(backup: &DbBackup, requested: Option<&str>) -> String {
    if let Some(requested) = requested {
        return requested.to_string();
    }
    if let Some(captured) = backup.snapshot.deployment_type.clone() {
        return captured;
    }
    if backup.snapshot.resource_type == RESOURCE_TYPE_DB_CLUSTER {
        CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS.to_string()
    } else {
        DEPLOYMENT_TYPE_SINGLE_AZ.to_string()
    }
}

/// Whether a resource of `deployment_type` is a DB CLUSTER.
///
/// The modelled `ResourceDeploymentType` spans both families: its read-replicas member is
/// the sole member of `ClusterDeploymentType`, so a resource of that type is a DB cluster,
/// and its other two members are the `DeploymentType` of a DB instance.
#[must_use]
pub fn deployment_type_is_cluster(deployment_type: &str) -> bool {
    deployment_type == CLUSTER_DEPLOYMENT_TYPE_READ_REPLICAS
}

/// How a restore NAMES the family of the resource it produced: this service's own name for
/// that family — the segment every ARN of it carries
/// ([`super::ARN_SEGMENT_DB_INSTANCE`], [`super::ARN_SEGMENT_DB_CLUSTER`]), which is also
/// the name its IAM resources are declared under — and so what
/// `RestoreFromDbBackupOutput$resourceType` reports.
#[must_use]
pub fn restored_resource_type(deployment_type: &str) -> &'static str {
    if deployment_type_is_cluster(deployment_type) {
        ARN_SEGMENT_DB_CLUSTER
    } else {
        ARN_SEGMENT_DB_INSTANCE
    }
}

impl RestoreOverrides {
    /// The tag set a restored record carries: the request's, else the ones the resource
    /// being restored into already holds.
    fn tags_for(&self, replaced: Option<&BTreeMap<String, String>>) -> BTreeMap<String, String> {
        if self.tags.is_empty() {
            return replaced.cloned().unwrap_or_default();
        }
        self.tags.clone()
    }
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Restore the backup `db_backup_id` into a resource named `name` in `region`, as
    /// `restore_mode` asks and with `overrides` applied over everything the backup
    /// captured.
    ///
    /// The whole decision and the whole write, under the caller's one lock: the restored
    /// resource (and, for a cluster, every node it provisions) is in the store by the time
    /// this answers, or nothing is.
    pub fn restore_from_db_backup(
        &mut self,
        region: &str,
        name: &str,
        db_backup_id: &str,
        restore_mode: Option<&str>,
        overrides: &RestoreOverrides,
        now: SystemTime,
    ) -> Result<Restored, RestoreRefusal> {
        // The backup is the whole source of the restored resource, so a backup nothing
        // resolves leaves nothing to restore from and nothing stands in for it.
        let Some(backup) = self.db_backups.get(db_backup_id).cloned() else {
            return Err(RestoreRefusal::NotFound(super::db_backup_not_found(
                db_backup_id,
            )));
        };

        let deployment_type = restored_deployment_type(&backup, overrides.deployment_type.as_deref());

        // An availability setting sold as an AWS Marketplace offering is provisioned only
        // for an account that holds an active subscription to it, whichever call asks for
        // that deployment type. Checked before anything is minted or written, so a refused
        // restore persists nothing.
        if let Some(offering) = marketplace_offering_for_deployment_type(&deployment_type)
            .filter(|offering| !self.holds_marketplace_subscription(*offering))
        {
            return Err(RestoreRefusal::Invalid(ValidationException {
                message: offering.missing_subscription_message.to_string(),
                // The request is well formed and it is the account's subscription that is
                // missing, so no field of it failed validation: the reason is the modelled
                // catch-all.
                reason: VALIDATION_EXCEPTION_REASON_OTHER.to_string(),
            }));
        }

        let replace_existing = restore_mode == Some(RESTORE_MODE_REPLACE_EXISTING);
        if deployment_type_is_cluster(&deployment_type) {
            self.restore_into_db_cluster(region, name, &backup, overrides, &deployment_type, replace_existing, now)
        } else {
            self.restore_into_db_instance(region, name, &backup, overrides, &deployment_type, replace_existing, now)
        }
    }

    /// Restore `backup` into the DB instance `name`: the one that holds the name when the
    /// restore replaces it, else a newly minted one.
    #[allow(clippy::too_many_arguments)]
    fn restore_into_db_instance(
        &mut self,
        region: &str,
        name: &str,
        backup: &DbBackup,
        overrides: &RestoreOverrides,
        deployment_type: &str,
        replace_existing: bool,
        now: SystemTime,
    ) -> Result<Restored, RestoreRefusal> {
        let held = self.db_instance_id_for_name(region, name).cloned();
        let record = if replace_existing {
            // The name is a lookup in this mode: it must match the resource being restored
            // into, so a name no live instance holds resolves to nothing to restore.
            let Some(id) = held else {
                return Err(RestoreRefusal::NotFound(named_resource_not_found(
                    NAMED_DB_INSTANCE,
                    name,
                )));
            };
            let replaced = self.db_instances.get(&id).cloned();
            restored_db_instance(
                region,
                &id,
                name,
                backup,
                overrides,
                deployment_type,
                now,
                replaced.as_ref(),
            )
        } else {
            // A new resource takes the name, and DB instance names are unique per customer
            // and per Region, so a name a live instance holds conflicts exactly as it does
            // for a create.
            if let Some(existing_id) = held {
                return Err(RestoreRefusal::Conflict(name_already_exists(
                    NAMED_DB_INSTANCE,
                    &existing_id,
                    name,
                )));
            }
            // The store is bounded: past the emulator's own cardinality bound a restore
            // that mints a resource is refused with the modelled quota error instead of
            // growing the state without end.
            if self.db_instances.len() >= DB_INSTANCE_QUOTA {
                return Err(RestoreRefusal::QuotaExceeded(db_instance_quota_exceeded()));
            }
            let id = self.mint_db_instance_id(name, region);
            restored_db_instance(
                region, &id, name, backup, overrides, deployment_type, now, None,
            )
        };

        let restored = Restored {
            resource_id: record.id.clone(),
            resource_type: restored_resource_type(deployment_type),
            engine_type: backup.snapshot.engine_type.clone(),
            deployment_type: deployment_type.to_string(),
            data: backup.snapshot.data.clone(),
        };
        self.claim_db_instance_name(region, name, record.id.clone());
        self.db_instances.insert(record.id.clone(), record);
        Ok(restored)
    }

    /// Restore `backup` into the DB cluster `name`, provisioning the nodes it is composed
    /// of: the cluster that holds the name when the restore replaces it, else a newly
    /// minted one.
    #[allow(clippy::too_many_arguments)]
    fn restore_into_db_cluster(
        &mut self,
        region: &str,
        name: &str,
        backup: &DbBackup,
        overrides: &RestoreOverrides,
        deployment_type: &str,
        replace_existing: bool,
        now: SystemTime,
    ) -> Result<Restored, RestoreRefusal> {
        // The topology is the one the backup captured — a cluster's backup carries it —
        // and otherwise the node composition the backup captured, or the one the DB
        // parameter group applied to the source configures, expressed as node groups
        // running the captured instance type: exactly how a create derives one.
        let node_groups = match backup.snapshot.deployment_configuration.as_ref() {
            Some(captured) => deployment_node_groups(captured),
            None => {
                let configuration = match backup.snapshot.cluster_configuration.clone() {
                    Some(captured) => captured,
                    None => {
                        parameter_cluster_configuration(self.captured_parameters(backup).as_ref())
                    }
                };
                configuration_node_groups(&configuration, &backup.snapshot.db_instance_type)
            }
        };

        let held = self.db_cluster_id_for_name(region, name).cloned();
        let replaced = match (replace_existing, held) {
            (true, None) => {
                return Err(RestoreRefusal::NotFound(named_resource_not_found(
                    NAMED_DB_CLUSTER,
                    name,
                )))
            }
            (true, Some(id)) => self.db_clusters.get(&id).cloned(),
            (false, Some(existing_id)) => {
                return Err(RestoreRefusal::Conflict(name_already_exists(
                    NAMED_DB_CLUSTER,
                    &existing_id,
                    name,
                )))
            }
            (false, None) => {
                if self.db_clusters.len() >= DB_CLUSTER_QUOTA {
                    return Err(RestoreRefusal::QuotaExceeded(ServiceQuotaExceededException {
                        message: format!(
                            "the maximum of {DB_CLUSTER_QUOTA} DB clusters this emulator \
                             stores has been reached"
                        ),
                    }));
                }
                None
            }
        };

        let id = match replaced.as_ref() {
            Some(record) => record.id.clone(),
            None => self.mint_db_cluster_id(name, region),
        };
        let mut cluster = restored_db_cluster(
            region,
            &id,
            name,
            backup,
            overrides,
            deployment_type,
            node_groups,
            now,
            replaced.as_ref(),
        );

        // Every node of the restored cluster is a DB instance of this store, named after
        // the cluster exactly as a create names one. The nodes a replaced cluster already
        // has are the ones restored into, so their ids and names stand; a cluster restored
        // as a new resource provisions a node set of its own.
        let nodes = cluster_nodes(name, &id, &cluster.node_groups);
        let reused: Vec<String> = replaced
            .as_ref()
            .map(|record| record.db_instance_ids.clone())
            .unwrap_or_default();
        if reused.is_empty() {
            // A cluster's nodes are DB instances in the same bounded store, so the same
            // bound refuses a cluster the store has no room for.
            if self.db_instances.len() + nodes.len() > DB_INSTANCE_QUOTA {
                return Err(RestoreRefusal::QuotaExceeded(db_instance_quota_exceeded()));
            }
            // A node's name is a DB instance name, unique per customer and per Region too.
            // Checked for all of them BEFORE anything is stored.
            for node in &nodes {
                if let Some(existing_id) = self.db_instance_id_for_name(region, &node.name) {
                    let existing_id = existing_id.clone();
                    return Err(RestoreRefusal::Conflict(name_already_exists(
                        NAMED_DB_INSTANCE,
                        &existing_id,
                        &node.name,
                    )));
                }
            }
        }

        for (position, node) in nodes.iter().enumerate() {
            let node_id = match reused.get(position) {
                Some(existing) => existing.clone(),
                None => self.mint_db_instance_id(&node.name, region),
            };            // The node's every member follows the RESTORED cluster, so one place derives a
            // node's configuration whether a create or a restore provisioned it.
            let mut record = cluster.node_instance(node, &node_id, now);
            if let Some(existing) = self.db_instances.get(&node_id) {
                // A node restored in place keeps the name it published and the address a
                // client dials it at.
                record.name = existing.name.clone();
                record.arn = existing.arn.clone();
                record.endpoint = existing.endpoint.clone();
                record.served_endpoint = existing.served_endpoint.clone();
                record.served_port = existing.served_port;
                record.created_at = existing.created_at;
            }
            record.status = STATUS_RESTORING.to_string();
            record.settles_at = Some(now + TRANSITION_SPAN);
            self.claim_db_instance_name(region, &record.name, node_id.clone());
            if !cluster.db_instance_ids.contains(&node_id) {
                cluster.db_instance_ids.push(node_id.clone());
            }
            self.db_instances.insert(node_id, record);
        }

        // A composition the restore narrowed leaves the replaced cluster with nodes it no
        // longer has: those DB instances are released with their names and their sockets,
        // so nothing is left in the store that the restored cluster does not report.
        for orphan in reused.iter().skip(nodes.len()) {
            if let Some(record) = self.db_instances.remove(orphan) {
                self.release_db_instance_name(&record.region, &record.name);
            }
            self.db_instance_planes.remove(orphan);
            cluster.db_instance_ids.retain(|held| held != orphan);
        }

        let restored = Restored {
            resource_id: id.clone(),
            resource_type: restored_resource_type(deployment_type),
            engine_type: cluster.engine_type.clone(),
            deployment_type: deployment_type.to_string(),
            // A cluster is reached through the cluster's own endpoints, which no socket of
            // this emulator serves, so there is no plane for the captured data to live in.
            data: None,
        };
        self.claim_db_cluster_name(region, name, id.clone());
        self.db_clusters.insert(id, cluster);
        Ok(restored)
    }

    /// The parameters of the DB parameter group the backup captured, as this account and
    /// Region resolves it now: one of the service-managed catalog's groups, or one this
    /// account created. `None` when the backup captured no group, and when the group it
    /// captured is no longer there to read.
    fn captured_parameters(&self, backup: &DbBackup) -> Option<crate::ops::types::Parameters> {
        let identifier = backup.snapshot.db_parameter_group_id.as_deref()?;
        if let Some(group) = service_managed_db_parameter_group(identifier) {
            return Some(group.parameters.clone());
        }
        self.db_parameter_groups
            .get(identifier)
            .map(|group| group.parameters.clone())
    }
}

/// The refusal a call that would mint a DB instance past the emulator's own bound on the
/// store gets — the same bound a create is refused by.
fn db_instance_quota_exceeded() -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the maximum of {DB_INSTANCE_QUOTA} DB instances for this account and Region \
             has been reached"
        ),
    }
}

/// The DB instance record a restore builds: every member from what the backup captured,
/// unless the request overrode it or the resource being restored into owns it.
///
/// `replaced` is the record a `REPLACE_EXISTING` restore restores INTO: it keeps its id,
/// its ARN, the endpoint and port a client is already dialling, its secret and its
/// creation time, and takes the backup's configuration for everything else. A restore that
/// mints a resource passes `None`, and the new resource gets an identity of its own.
#[allow(clippy::too_many_arguments)]
fn restored_db_instance(
    region: &str,
    id: &str,
    name: &str,
    backup: &DbBackup,
    overrides: &RestoreOverrides,
    deployment_type: &str,
    now: SystemTime,
    replaced: Option<&DbInstance>,
) -> DbInstance {
    let snapshot = &backup.snapshot;
    let (availability_zone, secondary_availability_zone) =
        availability_zones(region, Some(deployment_type));
    // The initial InfluxDB admin user, organization and bucket of the restored instance are
    // the ones the captured plane held: the restore request carries no password, so the
    // resource it produces authenticates the backup's admin user.
    let credential = snapshot.data.as_ref().map(TimeSeriesSnapshot::credential);
    DbInstance {
        id: id.to_string(),
        name: name.to_string(),
        arn: replaced
            .map(|record| record.arn.clone())
            .unwrap_or_else(|| db_instance_arn(region, id)),
        region: region.to_string(),
        // Restoring is work the service does, so the record is RESTORING when the call
        // answers and the clock settles it to AVAILABLE one transition span later.
        status: STATUS_RESTORING.to_string(),
        settles_at: Some(now + TRANSITION_SPAN),
        endpoint: replaced
            .map(|record| record.endpoint.clone())
            .unwrap_or_else(|| db_instance_endpoint(name, id, region)),
        // A resource restored in place keeps the socket it publishes; a minted one has no
        // address until the caller opens one for it.
        served_endpoint: replaced.and_then(|record| record.served_endpoint.clone()),
        served_port: replaced.and_then(|record| record.served_port),
        port: overrides.port.unwrap_or(snapshot.port),
        network_type: overrides
            .network_type
            .clone()
            .or_else(|| snapshot.network_type.clone()),
        db_instance_type: snapshot.db_instance_type.clone(),
        db_storage_type: snapshot.db_storage_type.clone(),
        allocated_storage: snapshot.allocated_storage,
        deployment_type: Some(deployment_type.to_string()),
        vpc_subnet_ids: overrides
            .vpc_subnet_ids
            .clone()
            .unwrap_or_else(|| snapshot.vpc_subnet_ids.clone()),
        vpc_security_group_ids: overrides
            .vpc_security_group_ids
            .clone()
            .unwrap_or_else(|| snapshot.vpc_security_group_ids.clone()),
        publicly_accessible: overrides.publicly_accessible.or(snapshot.publicly_accessible),
        db_parameter_group_identifier: snapshot.db_parameter_group_id.clone(),
        availability_zone: Some(availability_zone),
        secondary_availability_zone,
        log_delivery_configuration: overrides
            .log_delivery_configuration
            .clone()
            .or_else(|| snapshot.log_delivery_configuration.clone()),
        maintenance_schedule: overrides
            .maintenance_schedule
            .clone()
            .or_else(|| snapshot.maintenance_schedule.clone()),
        // A backup reports no automated-backup configuration of the resource it captured,
        // so the restored resource's are the ones the restore asked for.
        db_backup_configurations: overrides.db_backup_configurations.clone(),
        influx_auth_parameters_secret_arn: replaced
            .map(|record| record.influx_auth_parameters_secret_arn.clone())
            .unwrap_or_else(|| influx_auth_parameters_secret_arn(region, id)),
        kms_key_id: overrides
            .kms_key_id
            .clone()
            .or_else(|| snapshot.kms_key_id.clone()),
        username: credential.map(|credential| credential.username.clone()),
        organization: credential.map(|credential| credential.organization.clone()),
        bucket: credential.map(|credential| credential.bucket.clone()),
        tags: overrides.tags_for(replaced.map(|record| &record.tags)),
        // A restore into a resource that is a cluster's node leaves it that node; a
        // standalone resource belongs to no cluster.
        cluster: replaced.and_then(|record| record.cluster.clone()),
        created_at: replaced.map_or(now, |record| record.created_at),
    }
}

/// The DB cluster record a restore builds, under the same rule as [`restored_db_instance`]:
/// the captured configuration, the request's overrides, and the identity of the resource
/// being restored into where there is one.
///
/// Its `db_instance_ids` are filled in by the caller as the nodes it provisions are stored.
#[allow(clippy::too_many_arguments)]
fn restored_db_cluster(
    region: &str,
    id: &str,
    name: &str,
    backup: &DbBackup,
    overrides: &RestoreOverrides,
    deployment_type: &str,
    node_groups: Vec<crate::ops::types::NodeGroup>,
    now: SystemTime,
    replaced: Option<&DbCluster>,
) -> DbCluster {
    let snapshot = &backup.snapshot;
    let minted = db_cluster_endpoints(name, id, region);
    let (endpoint, reader_endpoint) = replaced
        .map(|record| (record.endpoint.clone(), record.reader_endpoint.clone()))
        .unwrap_or(minted);
    // As for a restored DB instance: the initial InfluxDB admin user, organization and
    // bucket are the captured plane's, because the restore request carries no password.
    let credential = snapshot.data.as_ref().map(TimeSeriesSnapshot::credential);
    DbCluster {
        id: id.to_string(),
        name: name.to_string(),
        arn: replaced
            .map(|record| record.arn.clone())
            .unwrap_or_else(|| db_cluster_arn(region, id)),
        region: region.to_string(),
        // A cluster is composite: it is restored only once the nodes it provisions are, so
        // it owes one span for them and one more for itself.
        status: CLUSTER_STATUS_RESTORING.to_string(),
        settles_at: Some(now + CLUSTER_TRANSITION_SPAN),
        endpoint,
        reader_endpoint,
        port: overrides.port.unwrap_or(snapshot.port),
        deployment_type: deployment_type.to_string(),
        db_instance_type: snapshot.db_instance_type.clone(),
        network_type: overrides
            .network_type
            .clone()
            .or_else(|| snapshot.network_type.clone()),
        db_storage_type: snapshot.db_storage_type.clone(),
        allocated_storage: snapshot.allocated_storage,
        engine_type: snapshot.engine_type.clone(),
        publicly_accessible: overrides.publicly_accessible.or(snapshot.publicly_accessible),
        db_parameter_group_identifier: snapshot.db_parameter_group_id.clone(),
        effective_db_parameter_group_identifier: snapshot.db_parameter_group_id.clone(),
        log_delivery_configuration: overrides
            .log_delivery_configuration
            .clone()
            .or_else(|| snapshot.log_delivery_configuration.clone()),
        maintenance_schedule: overrides
            .maintenance_schedule
            .clone()
            .or_else(|| snapshot.maintenance_schedule.clone()),
        influx_auth_parameters_secret_arn: replaced
            .map(|record| record.influx_auth_parameters_secret_arn.clone())
            .unwrap_or_else(|| influx_auth_parameters_secret_arn(region, id)),
        vpc_subnet_ids: overrides
            .vpc_subnet_ids
            .clone()
            .unwrap_or_else(|| snapshot.vpc_subnet_ids.clone()),
        vpc_security_group_ids: overrides
            .vpc_security_group_ids
            .clone()
            .unwrap_or_else(|| snapshot.vpc_security_group_ids.clone()),
        // A backup of a DB cluster captures its failover mode; one of a DB instance has
        // none to capture, so a cluster restored from it recovers from a primary-node
        // failure itself, exactly as a create that named no mode leaves it.
        failover_mode: snapshot
            .failover_mode
            .clone()
            .unwrap_or_else(|| FAILOVER_MODE_AUTOMATIC.to_string()),
        node_groups,
        db_backup_configurations: overrides.db_backup_configurations.clone(),
        kms_key_id: overrides
            .kms_key_id
            .clone()
            .or_else(|| snapshot.kms_key_id.clone()),
        username: replaced
            .and_then(|record| record.username.clone())
            .or_else(|| credential.map(|credential| credential.username.clone())),
        organization: replaced
            .and_then(|record| record.organization.clone())
            .or_else(|| credential.map(|credential| credential.organization.clone())),
        bucket: replaced
            .and_then(|record| record.bucket.clone())
            .or_else(|| credential.map(|credential| credential.bucket.clone())),
        tags: overrides.tags_for(replaced.map(|record| &record.tags)),
        db_instance_ids: Vec::new(),
        created_at: replaced.map_or(now, |record| record.created_at),
    }
}
