// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb service.
//!
//! This module owns the persisted DB-instance records plus the small helpers every
//! DB-instance handler shares: minting the service-generated `id`, formatting the `arn`
//! and `endpoint`, settling the clock-owed status transition, and projecting a stored
//! record into each operation's own output shape. The handlers in `src/ops/` hold no
//! state of their own — they read and write the records here.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConflictException, DbBackupConfiguration, DbBackupConfigurationOutput, DbInstanceSummary,
    DbParameterGroupSummary, InfluxDBv2Parameters, InfluxDBv3CoreParameters,
    InfluxDBv3EnterpriseParameters, LogDeliveryConfiguration, MaintenanceSchedule, Parameters,
    ValidationException,
};

/// The DB-cluster records and the helpers the cluster operations share. A cluster
/// provisions the DB instances its nodes are, so its slice of the state lives beside
/// this one and reaches the same [`DbInstance`] records.
mod clusters;

/// The account-level AWS Marketplace subscriptions some deployment types are gated
/// behind, and the refusal a create gets without one.
mod marketplace;

/// How an `Arn` resolves to one of the records above, and the tag set that record
/// carries — the slice the three tagging operations share.
mod tags;

pub use clusters::*;
pub use marketplace::*;
pub use tags::*;

/// `Status` values this service's records use, each one a member of
/// `crate::ops::types::STATUS_VALUES` (the modelled `Status` enum).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// See [`STATUS_CREATING`].
pub const STATUS_REBOOTING: &str = "REBOOTING";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING_INSTANCE_TYPE: &str = "UPDATING_INSTANCE_TYPE";
/// See [`STATUS_CREATING`].
pub const STATUS_UPDATING_DEPLOYMENT_TYPE: &str = "UPDATING_DEPLOYMENT_TYPE";

/// The `resourceType` this service reports on `ConflictException` /
/// `ResourceNotFoundException` for a DB instance — a member of
/// `crate::ops::types::RESOURCE_TYPE_VALUES` (the modelled `ResourceType` enum).
pub const RESOURCE_TYPE_DB_INSTANCE: &str = "DB_INSTANCE";

/// The `resourceType` this service reports on `ConflictException` /
/// `ResourceNotFoundException` for a DB parameter group.
///
/// Those exceptions model `resourceType` as a plain `String`, NOT as the `ResourceType`
/// enum (`smithy_query(query="shape", shape="ConflictException,ResourceType")`), so the
/// parameter-group resource type is reportable even though the enum lists only the two
/// compute resources.
pub const RESOURCE_TYPE_DB_PARAMETER_GROUP: &str = "DB_PARAMETER_GROUP";

/// A resource family this service holds a unique-name index on, in the two spellings a
/// refusal about its name needs.
#[derive(Debug, Clone, Copy)]
pub struct NamedResource {
    /// The `resourceType` the refusal reports.
    pub resource_type: &'static str,
    /// How the refusal's sentence NAMES the resource: the shape name this model gives it
    /// (`smithy_query(query="digest")` — `DbInstance`, `DbCluster`, `DbParameterGroup`),
    /// which is how this service spells the resource in prose.
    pub spelling: &'static str,
}

/// See [`NamedResource`]: the DB instance family.
pub const NAMED_DB_INSTANCE: NamedResource = NamedResource {
    resource_type: RESOURCE_TYPE_DB_INSTANCE,
    spelling: "DbInstance",
};

/// See [`NamedResource`]: the DB parameter group family.
pub const NAMED_DB_PARAMETER_GROUP: NamedResource = NamedResource {
    resource_type: RESOURCE_TYPE_DB_PARAMETER_GROUP,
    spelling: "DbParameterGroup",
};

/// See [`NamedResource`]: the DB cluster family. Its `resourceType` is declared beside the
/// cluster records themselves (`clusters::RESOURCE_TYPE_DB_CLUSTER`), and is reached here
/// through this module's re-export of them, so all three named families read in one place.
pub const NAMED_DB_CLUSTER: NamedResource = NamedResource {
    resource_type: RESOURCE_TYPE_DB_CLUSTER,
    spelling: "DbCluster",
};

/// The refusal a create gets when the name it asks for is already taken: this service's
/// already-exists sentence, naming the resource, the id of the record that HOLDS the name,
/// the name the request asked for and the customer the records belong to — beside the
/// `resourceId` and `resourceType` the model marks required on `ConflictException`.
///
/// A client reconciling create-if-absent logic reads the existing record's id out of this
/// refusal to adopt that record instead of failing, so the id is part of the sentence and
/// not only of the `resourceId` member. Every value in it comes from the request and from
/// this emulator's state.
#[must_use]
pub fn name_already_exists(
    resource: NamedResource,
    existing_id: &str,
    requested_name: &str,
) -> ConflictException {
    ConflictException {
        message: format!(
            "The {} with id [{existing_id}], name [{requested_name}], customer [{}] \
             already exists",
            resource.spelling,
            ::engine::emulator_account_id(),
        ),
        resource_id: existing_id.to_string(),
        resource_type: resource.resource_type.to_string(),
    }
}

/// The refusal a lifecycle action gets while the resource it names is still busy with a
/// change of its own: the request is well formed, but the resource is not in the status
/// the action can start from.
///
/// Names the resource, its id, the status it is actually in and the status the action
/// needs — all four read out of this emulator's own state, nothing of them interpolated
/// from a recording.
#[must_use]
pub fn resource_not_in_status(
    resource: NamedResource,
    id: &str,
    current_status: &str,
    required_status: &str,
) -> ValidationException {
    ValidationException {
        message: format!(
            "The {} with id [{id}] is in status [{current_status}] and must be in status \
             [{required_status}] for the requested operation",
            resource.spelling,
        ),
        // The request itself is valid and no field of it failed validation — the
        // resource's status is what refuses it — so the reason is the modelled catch-all.
        reason: VALIDATION_EXCEPTION_REASON_OTHER.to_string(),
    }
}

/// The `deploymentType` that places a standby in a second Availability Zone, from the
/// modelled `DeploymentType` enum (`crate::ops::types::DEPLOYMENT_TYPE_VALUES`).
pub const DEPLOYMENT_TYPE_WITH_MULTIAZ_STANDBY: &str = "WITH_MULTIAZ_STANDBY";

/// The region a request's ARNs are scoped to when its SigV4 credential scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `aws.api#service$arnNamespace` / sigv4 signing name, used for both the
/// ARN service segment and the endpoint suffix.
pub const ARN_NAMESPACE: &str = "timestream-influxdb";

/// How long a DB instance stays in a transient status before the clock settles it to
/// `AVAILABLE`.
///
/// NOT an AWS-documented duration (a real instance takes minutes): a chosen span. It is
/// non-zero, so the transient status a lifecycle call announces is the record's real
/// status at that instant and the state errors keyed to it (a restart of an instance that
/// is not running, say) are reachable for a caller that acts on the answer it just got.
/// It is also SHORTER than the round trip a client needs to issue its next read, which is
/// what keeps a polling client live: the settle is already due when that read arrives, so
/// a workflow that provisions, waits, deletes and re-provisions does not spend one of the
/// client's probe intervals on every transition it waits through. A span of the order of a
/// client's probe interval instead of its round trip is what makes such a workflow cost
/// more wall time than the workflow is given.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(25);

/// The default `maxResults` for the paginated list operations: the maximum the model's
/// `MaxResults` range (1..100) allows, so an unbounded request pages at the modelled cap.
pub const MAX_RESULTS_DEFAULT: i32 = 100;

/// How many DB instances the store holds before `CreateDbInstance` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amazon Timestream for InfluxDB publishes an adjustable quota on DB instances per
/// account per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a claim
/// about any account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is released by
/// `DeleteDbInstance`, which also forgets that instance's name, so the name index can
/// never outgrow this bound either. It also caps the per-request work `ListDbInstances`
/// does when it settles the clock-owed statuses.
pub const DB_INSTANCE_QUOTA: usize = 1_000;

/// How many DB parameter groups the store holds before `CreateDbParameterGroup` refuses
/// with the modelled `ServiceQuotaExceededException`.
///
/// Like [`DB_INSTANCE_QUOTA`], this is the emulator's OWN bound on the state rather than
/// a claim about any account's adjustable Service Quotas limit: it is what stops the
/// store (and the parameter-group name index beside it) growing without end, and it sits
/// far above any workflow an emulator serves.
pub const DB_PARAMETER_GROUP_QUOTA: usize = 1_000;

/// One persisted Timestream for InfluxDB DB instance: the single source of truth every
/// DB-instance operation reads and writes.
#[derive(Debug, Clone)]
pub struct DbInstance {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region this instance was created in; its ARN, endpoint and Availability
    /// Zones are all scoped to it, so a later operation derives them from here rather
    /// than from the calling request.
    pub region: String,
    pub status: String,
    pub endpoint: String,
    pub port: i32,
    pub network_type: Option<String>,
    pub db_instance_type: String,
    pub db_storage_type: Option<String>,
    pub allocated_storage: i32,
    pub deployment_type: Option<String>,
    pub vpc_subnet_ids: Vec<String>,
    pub vpc_security_group_ids: Vec<String>,
    pub publicly_accessible: Option<bool>,
    pub db_parameter_group_identifier: Option<String>,
    pub availability_zone: Option<String>,
    pub secondary_availability_zone: Option<String>,
    pub log_delivery_configuration: Option<LogDeliveryConfiguration>,
    pub maintenance_schedule: Option<MaintenanceSchedule>,
    pub db_backup_configurations: Option<Vec<DbBackupConfiguration>>,
    pub influx_auth_parameters_secret_arn: String,
    pub kms_key_id: Option<String>,
    /// Stored because `CreateDbInstance` accepts them for the initial InfluxDB admin
    /// user; they are not members of any DB-instance output shape, so nothing echoes
    /// them back. The `password` is deliberately NOT kept: it is `@sensitive`, no output
    /// shape carries it, and the record only needs the secret ARN that stands for it.
    pub username: Option<String>,
    pub organization: Option<String>,
    pub bucket: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// What this instance is inside the DB cluster that provisioned it, or `None` for a
    /// standalone instance that belongs to no cluster.
    pub cluster: Option<DbInstanceClusterRole>,
    pub created_at: SystemTime,
    /// When the clock owes this record its settle to `AVAILABLE`; `None` once settled.
    pub settles_at: Option<SystemTime>,
}

/// A DB instance's place in the DB cluster that provisioned it: which cluster, and the
/// roles the instance takes in it.
#[derive(Debug, Clone)]
pub struct DbInstanceClusterRole {
    pub db_cluster_id: String,
    /// The roles this instance takes, each one a member of
    /// `crate::ops::types::INSTANCE_MODE_VALUES` (the modelled `InstanceMode` enum).
    /// Never empty.
    pub instance_modes: Vec<String>,
}

impl DbInstanceClusterRole {
    /// The single role `instanceMode` reports: the instance's first role, where
    /// `instanceModes` reports every one of them.
    pub fn instance_mode(&self) -> Option<String> {
        self.instance_modes.first().cloned()
    }
}

impl DbInstance {
    /// Materialise the status change the clock already made due.
    ///
    /// A reporting operation calls this before answering: the transition was owed at
    /// `settles_at` whether or not anybody called, so applying it on read is not a
    /// read-time mutation of the resource's own lifecycle.
    pub fn settle(&mut self, now: SystemTime) {
        if self.settles_at.is_some_and(|due| now >= due) {
            self.status = STATUS_AVAILABLE.to_string();
            self.settles_at = None;
        }
    }

    /// Put the record into `status` and make the settle back to `AVAILABLE` due one
    /// [`TRANSITION_SPAN`] from `now`.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        self.status = status.to_string();
        self.settles_at = Some(now + TRANSITION_SPAN);
    }

    /// Whether this record is settled and idle: in `AVAILABLE`, the only status a
    /// lifecycle action that restarts the instance can start from. Call [`Self::settle`]
    /// first, so a transition the clock already owed is not read as still running.
    pub fn is_available(&self) -> bool {
        self.status == STATUS_AVAILABLE
    }

    /// The list-shaped projection of this record.
    pub fn to_summary(&self) -> DbInstanceSummary {
        DbInstanceSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
            port: Some(self.port),
            network_type: self.network_type.clone(),
            db_instance_type: Some(self.db_instance_type.clone()),
            db_storage_type: self.db_storage_type.clone(),
            allocated_storage: Some(self.allocated_storage),
            deployment_type: self.deployment_type.clone(),
        }
    }

    /// The backup configurations as the output list shape.
    ///
    /// `nextAutomatedBackupTime` stays absent: scheduling automated backups belongs to
    /// the backup operations, which are not implemented, so this emulator reports no
    /// time for a backup nothing has scheduled.
    fn backup_configurations_output(&self) -> Option<Vec<DbBackupConfigurationOutput>> {
        backup_configurations_output(&self.db_backup_configurations)
    }
}

/// The stored backup configurations of a resource as the output list shape.
///
/// `nextAutomatedBackupTime` stays absent: scheduling automated backups belongs to the
/// backup operations, which are not implemented, so this emulator reports no time for a
/// backup nothing has scheduled.
pub fn backup_configurations_output(
    configurations: &Option<Vec<DbBackupConfiguration>>,
) -> Option<Vec<DbBackupConfigurationOutput>> {
    configurations.as_ref().map(|configs| {
        configs
            .iter()
            .map(|config| DbBackupConfigurationOutput {
                r#type: config.r#type.clone(),
                retention_days: config.retention_days,
                enabled: config.enabled,
                custom_schedule: config.custom_schedule.clone(),
                next_automated_backup_time: None,
            })
            .collect()
    })
}

/// The four DB-instance operations whose output shapes are the same full record.
///
/// They are four distinct generated types with identical members, so one macro writes
/// one projection per shape instead of four copies that could drift apart.
macro_rules! db_instance_record_projections {
    ($($method:ident -> $out:path),* $(,)?) => {
        impl DbInstance {
            $(
                /// Project this stored record into the operation's output shape. Every
                /// member comes from the record; nothing is recomputed from the request.
                pub fn $method(&self) -> $out {
                    $out {
                        id: self.id.clone(),
                        name: self.name.clone(),
                        arn: self.arn.clone(),
                        status: Some(self.status.clone()),
                        endpoint: Some(self.endpoint.clone()),
                        port: Some(self.port),
                        network_type: self.network_type.clone(),
                        db_instance_type: Some(self.db_instance_type.clone()),
                        db_storage_type: self.db_storage_type.clone(),
                        allocated_storage: Some(self.allocated_storage),
                        deployment_type: self.deployment_type.clone(),
                        vpc_subnet_ids: self.vpc_subnet_ids.clone(),
                        publicly_accessible: self.publicly_accessible,
                        vpc_security_group_ids: Some(self.vpc_security_group_ids.clone()),
                        db_parameter_group_identifier: self
                            .db_parameter_group_identifier
                            .clone(),
                        availability_zone: self.availability_zone.clone(),
                        secondary_availability_zone: self.secondary_availability_zone.clone(),
                        log_delivery_configuration: self.log_delivery_configuration.clone(),
                        influx_auth_parameters_secret_arn: Some(
                            self.influx_auth_parameters_secret_arn.clone(),
                        ),
                        // An instance a DB cluster provisioned reports that cluster and
                        // the roles it takes in it; a standalone instance belongs to no
                        // cluster, so it reports neither.
                        db_cluster_id: self
                            .cluster
                            .as_ref()
                            .map(|cluster| cluster.db_cluster_id.clone()),
                        instance_mode: self
                            .cluster
                            .as_ref()
                            .and_then(DbInstanceClusterRole::instance_mode),
                        instance_modes: self
                            .cluster
                            .as_ref()
                            .map(|cluster| cluster.instance_modes.clone()),
                        maintenance_schedule: self.maintenance_schedule.clone(),
                        // No maintenance has run and none is scheduled by this slice.
                        last_maintenance_time: None,
                        next_maintenance_time: None,
                        db_backup_configurations: self.backup_configurations_output(),
                        kms_key_id: self.kms_key_id.clone(),
                    }
                }
            )*
        }
    };
}

db_instance_record_projections! {
    to_create_output -> crate::ops::types::CreateDbInstanceOutput,
    to_get_output -> crate::ops::types::GetDbInstanceOutput,
    to_update_output -> crate::ops::types::UpdateDbInstanceOutput,
    to_delete_output -> crate::ops::types::DeleteDbInstanceOutput,
    to_reboot_output -> crate::ops::types::RebootDbInstanceOutput,
}

/// One persisted Timestream for InfluxDB DB parameter group: the single source of truth
/// every DB-parameter-group operation reads and writes.
///
/// A parameter group has no lifecycle status in its model (no `status` member on any of
/// its shapes), so it is ready the moment it is created and owes the clock no transition.
#[derive(Debug, Clone)]
pub struct DbParameterGroup {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The region the group was created in; its ARN is scoped to it, and the name
    /// uniqueness rule is per region, so later operations read the region from here.
    pub region: String,
    pub description: Option<String>,
    /// The engine parameter family this group configures, and the tunables overridden
    /// inside it. NOT optional: a group always belongs to exactly one family, so every
    /// shape that models `parameters` reports it (see
    /// [`default_db_parameter_group_parameters`] for the group whose create named none).
    pub parameters: Parameters,
    /// The tags the create request attached. No parameter-group output shape carries
    /// them, so nothing echoes them back; they are kept for the tagging operations.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
}

/// The DB-parameter-group operations whose output shape is the same full record.
///
/// They are distinct generated types with identical members, so one macro writes one
/// projection per shape instead of copies that could drift apart.
macro_rules! db_parameter_group_record_projections {
    ($($method:ident -> $out:path),* $(,)?) => {
        impl DbParameterGroup {
            $(
                /// Project this stored record into the operation's output shape. Every
                /// member comes from the record; nothing is recomputed from the request.
                pub fn $method(&self) -> $out {
                    $out {
                        id: self.id.clone(),
                        name: self.name.clone(),
                        arn: self.arn.clone(),
                        description: self.description.clone(),
                        parameters: Some(self.parameters.clone()),
                    }
                }
            )*
        }
    };
}

db_parameter_group_record_projections! {
    to_create_output -> crate::ops::types::CreateDbParameterGroupOutput,
    to_get_output -> crate::ops::types::GetDbParameterGroupOutput,
}

impl DbParameterGroup {
    /// The list-shaped projection of this record.
    ///
    /// `DbParameterGroupSummary` models only `id`, `name`, `arn` and `description`
    /// (`smithy_query(query="shape", shape="DbParameterGroupSummary")`), so a summary
    /// carries no `parameters`; every member it does carry comes from the stored record.
    pub fn to_summary(&self) -> DbParameterGroupSummary {
        DbParameterGroupSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            description: self.description.clone(),
        }
    }
}

/// The `db.influx.` prefix every DB instance type carries
/// (`smithy_query(query="shape", shape="DbInstanceType")`).
const DB_INSTANCE_TYPE_PREFIX: &str = "db.influx.";

/// The fixed text of the description a per-instance-size service-managed default group
/// carries. The variable part after it is the group's own name.
const SERVICE_MANAGED_DEFAULT_DESCRIPTION_PREFIX: &str = "Service-managed parameter group: ";

/// Every DB instance type, in the order the model declares them — ascending size.
///
/// The values of the modelled `DbInstanceType` enum
/// (`smithy_query(query="shape", shape="DbInstanceType")`). The generated
/// `crate::ops::types::DB_INSTANCE_TYPE_VALUES` holds the same nine values sorted
/// lexicographically, which is not the order a size-ordered catalog is reported in, so
/// the declaration order is kept here.
pub const DB_INSTANCE_TYPES_BY_SIZE: &[&str] = &[
    "db.influx.medium",
    "db.influx.large",
    "db.influx.xlarge",
    "db.influx.2xlarge",
    "db.influx.4xlarge",
    "db.influx.8xlarge",
    "db.influx.12xlarge",
    "db.influx.16xlarge",
    "db.influx.24xlarge",
];

/// The size token a service-managed default group's name carries for a DB instance type:
/// the part of the type after the `db.influx.` prefix with its first character upper-cased
/// (`db.influx.medium` -> `Medium`, `db.influx.2xlarge` -> `2xlarge`).
fn instance_size_token(db_instance_type: &str) -> String {
    let size = db_instance_type
        .strip_prefix(DB_INSTANCE_TYPE_PREFIX)
        .unwrap_or(db_instance_type);
    let mut characters = size.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// A v3 engine parameter family a service-managed default group exists for: the v3
/// members of the modelled `Parameters` union
/// (`smithy_query(query="shape", shape="Parameters")`).
#[derive(Debug, Clone, Copy)]
enum V3Family {
    Core,
    Enterprise,
}

impl V3Family {
    /// The token this family contributes to a service-managed group's name.
    fn name_token(self) -> &'static str {
        match self {
            V3Family::Core => "Core",
            V3Family::Enterprise => "Enterprise",
        }
    }

    /// The engine parameter family a group of this family configures, with no tunable
    /// overridden.
    fn parameters(self) -> Parameters {
        match self {
            V3Family::Core => Parameters::InfluxDBv3Core(InfluxDBv3CoreParameters::default()),
            V3Family::Enterprise => {
                Parameters::InfluxDBv3Enterprise(InfluxDBv3EnterpriseParameters::default())
            }
        }
    }
}

/// One entry of the service-managed catalog.
///
/// A catalog entry is NOT an account-scoped resource: its `id` IS its `name` and its `arn`
/// is empty, which is how the service reports a group nobody created. `region` is empty for
/// the same reason — the catalog is the same in every Region — and `created_at` is the
/// epoch, a value no parameter-group output shape carries.
fn service_managed_group(
    name: &str,
    description: String,
    parameters: Parameters,
) -> DbParameterGroup {
    DbParameterGroup {
        id: name.to_string(),
        name: name.to_string(),
        arn: String::new(),
        region: String::new(),
        description: Some(description),
        parameters,
        tags: BTreeMap::new(),
        created_at: SystemTime::UNIX_EPOCH,
    }
}

/// The service-managed (default) DB parameter groups every account and Region has without
/// anyone creating them, in the order the service reports them.
///
/// First the engine groups a create names to start a v3 engine, then one per-instance-size
/// default for each v3 engine parameter family of the modelled `Parameters` union
/// (`smithy_query(query="shape", shape="Parameters")`), the sizes in
/// [`DB_INSTANCE_TYPES_BY_SIZE`] order.
///
/// They live here rather than in [`ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState::db_parameter_groups`]
/// because no operation creates, changes or deletes one and they are identical in every
/// Region and account: they are the service's own catalog, not this account's records, so
/// they consume none of the per-account quota and the name index stays about customer
/// groups only.
pub fn service_managed_db_parameter_groups() -> &'static [DbParameterGroup] {
    static CATALOG: ::std::sync::OnceLock<Vec<DbParameterGroup>> = ::std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut catalog = vec![
            service_managed_group(
                "InfluxDBV3Core",
                "Service Parameter group to create InfluxDB v3 core engine".to_string(),
                Parameters::InfluxDBv3Core(InfluxDBv3CoreParameters::default()),
            ),
            service_managed_group(
                "InfluxDBV3Enterprise",
                "Service Parameter group to create InfluxDB v3 enterprise engine".to_string(),
                Parameters::InfluxDBv3Enterprise(InfluxDBv3EnterpriseParameters::default()),
            ),
            service_managed_group(
                "InfluxDBV3Enterprise1Node",
                "Service Parameter group to create InfluxDB v3 enterprise engine with 1 node"
                    .to_string(),
                Parameters::InfluxDBv3Enterprise(InfluxDBv3EnterpriseParameters {
                    // This group's own description is "with 1 node": a single instance
                    // taking both the ingest and the query role.
                    ingest_query_instances: 1,
                    ..InfluxDBv3EnterpriseParameters::default()
                }),
            ),
        ];

        for family in [V3Family::Core, V3Family::Enterprise] {
            for db_instance_type in DB_INSTANCE_TYPES_BY_SIZE {
                let name = format!(
                    "InfluxDBV3{}{}Default",
                    family.name_token(),
                    instance_size_token(db_instance_type)
                );
                let description = format!("{SERVICE_MANAGED_DEFAULT_DESCRIPTION_PREFIX}{name}");
                catalog.push(service_managed_group(&name, description, family.parameters()));
            }
        }
        catalog
    })
}

/// Where `identifier` sits in the service-managed catalog, if it names one of its groups.
pub fn service_managed_db_parameter_group_index(identifier: &str) -> Option<usize> {
    service_managed_db_parameter_groups()
        .iter()
        .position(|group| group.id == identifier)
}

/// The service-managed DB parameter group `identifier` names, if any. A catalog entry's
/// `id` is its `name`, so either resolves it.
pub fn service_managed_db_parameter_group(identifier: &str) -> Option<&'static DbParameterGroup> {
    service_managed_db_parameter_group_index(identifier)
        .map(|position| &service_managed_db_parameter_groups()[position])
}

/// Where a DB parameter group sits in the order `ListDbParameterGroups` reports: the
/// service-managed catalog first, in catalog order, then the customer-created groups by
/// their minted id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DbParameterGroupListKey {
    /// A service-managed group, at this position in the catalog.
    ServiceManaged(usize),
    /// A customer-created group, by its minted id.
    Customer(String),
}

/// The list position an `id` stands for — also what a pagination token resolves to, so a
/// page resumes strictly after the group the previous page ended on even if that group has
/// since been deleted or was never stored at all.
pub fn db_parameter_group_list_key(id: &str) -> DbParameterGroupListKey {
    match service_managed_db_parameter_group_index(id) {
        Some(position) => DbParameterGroupListKey::ServiceManaged(position),
        None => DbParameterGroupListKey::Customer(id.to_string()),
    }
}

/// Per-service state for com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Live DB instances by their service-generated `id`. Ordered, so `ListDbInstances`
    /// pages deterministically.
    pub db_instances: BTreeMap<String, DbInstance>,
    /// `(region, name)` -> `id`: the uniqueness index behind `ConflictException` on a
    /// duplicate name. The key carries the region because the rule the model documents is
    /// "DB instance names must be unique per customer and per region", so the same name
    /// in two regions is two instances.
    pub db_instance_names: BTreeMap<(String, String), String>,
    /// Monotonic sequence mixed into every minted `id`, so two instances created with
    /// the same name in one process never collide.
    pub db_instance_seq: u64,
    /// Live DB parameter groups by their service-generated `id`. Ordered, so
    /// `ListDbParameterGroups` pages deterministically.
    pub db_parameter_groups: BTreeMap<String, DbParameterGroup>,
    /// `(region, name)` -> `id`: the uniqueness index behind `ConflictException` on a
    /// duplicate parameter-group name. The key carries the region because the rule the
    /// model documents is "The name must be unique per customer and per region".
    pub db_parameter_group_names: BTreeMap<(String, String), String>,
    /// Monotonic sequence mixed into every minted parameter-group `id`.
    pub db_parameter_group_seq: u64,
    /// Live DB clusters by their service-generated `id`. Ordered, so `ListDbClusters`
    /// pages deterministically.
    pub db_clusters: BTreeMap<String, DbCluster>,
    /// `(region, name)` -> `id`: the uniqueness index behind `ConflictException` on a
    /// duplicate cluster name. The key carries the region because the rule the model
    /// documents is "DB cluster names must be unique per customer and per region", so
    /// the same name in two regions is two clusters.
    pub db_cluster_names: BTreeMap<(String, String), String>,
    /// Monotonic sequence mixed into every minted cluster `id`.
    pub db_cluster_seq: u64,
    /// The AWS Marketplace offerings this account holds an ACTIVE subscription to, by
    /// [`MarketplaceOffering::key`]. EMPTY by default, because nothing in this API
    /// subscribes an account: a buyer subscribes in AWS Marketplace, out of band of
    /// every operation here, which is what
    /// [`subscribe_account_to_marketplace_offering`] stands in for.
    pub marketplace_subscriptions: BTreeSet<String>,
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// Mint the next service-generated DB instance id: 10 lowercase hex characters, which
    /// satisfies the modelled `DbInstanceId` pattern `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_instance_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_instance_seq += 1;
            let seq = self.db_instance_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-instance|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_instances.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the live DB instance holding `name` in `region`, if any.
    pub fn db_instance_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        self.db_instance_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_instance_name(&mut self, region: &str, name: &str, id: String) {
        self.db_instance_names
            .insert((region.to_string(), name.to_string()), id);
    }

    /// Release the name a deleted record held, so the name can be reused.
    pub fn release_db_instance_name(&mut self, region: &str, name: &str) {
        self.db_instance_names
            .remove(&(region.to_string(), name.to_string()));
    }

    /// Mint the next service-generated DB parameter group id: 10 lowercase hex
    /// characters, which satisfies the modelled `DbParameterGroupId` pattern
    /// `^[a-zA-Z0-9]+$` and length 3..64.
    ///
    /// Opaque to the caller but deterministic for a given (name, region, sequence), and
    /// unique because the monotonic sequence advances until the id is unused.
    pub fn mint_db_parameter_group_id(&mut self, name: &str, region: &str) -> String {
        loop {
            self.db_parameter_group_seq += 1;
            let seq = self.db_parameter_group_seq;
            let digest = <sha2::Sha256 as sha2::Digest>::digest(
                format!("db-parameter-group|{region}|{name}|{seq}").as_bytes(),
            );
            let id = hex::encode(digest)[..10].to_string();
            if !self.db_parameter_groups.contains_key(&id) {
                return id;
            }
        }
    }

    /// The id of the DB parameter group holding `name` in `region`, if any.
    ///
    /// A service-managed group exists in every Region without anyone creating it, so its
    /// name is taken in every Region too — the "unique per customer and per region" rule
    /// covers the service's own catalog as well as this account's records.
    pub fn db_parameter_group_id_for_name(&self, region: &str, name: &str) -> Option<&String> {
        if let Some(group) = service_managed_db_parameter_group(name) {
            return Some(&group.id);
        }
        self.db_parameter_group_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every DB parameter group this account and Region has, each paired with its list
    /// position, in the stable order `ListDbParameterGroups` reports them: the
    /// service-managed catalog first, then the customer-created groups by minted id.
    pub fn db_parameter_groups_in_list_order(
        &self,
    ) -> Vec<(DbParameterGroupListKey, DbParameterGroupSummary)> {
        service_managed_db_parameter_groups()
            .iter()
            .enumerate()
            .map(|(position, group)| {
                (
                    DbParameterGroupListKey::ServiceManaged(position),
                    group.to_summary(),
                )
            })
            .chain(self.db_parameter_groups.iter().map(|(id, record)| {
                (
                    DbParameterGroupListKey::Customer(id.clone()),
                    record.to_summary(),
                )
            }))
            .collect()
    }

    /// Claim `name` in `region` for `id`.
    pub fn claim_db_parameter_group_name(&mut self, region: &str, name: &str, id: String) {
        self.db_parameter_group_names
            .insert((region.to_string(), name.to_string()), id);
    }
}

/// The region a request is scoped to: the region of its SigV4 credential scope, else
/// [`DEFAULT_REGION`].
///
/// Constrained to the characters the modelled `Arn` pattern allows in its region segment
/// (`[a-z0-9\-]+`), so an ARN built from it is always a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The four resource segments the modelled `Arn` pattern allows
/// (`smithy_query(query="shape", shape="Arn")`): every ARN this service spells names one
/// of these families.
pub const ARN_SEGMENT_DB_INSTANCE: &str = "db-instance";
/// See [`ARN_SEGMENT_DB_INSTANCE`].
pub const ARN_SEGMENT_DB_CLUSTER: &str = "db-cluster";
/// See [`ARN_SEGMENT_DB_INSTANCE`].
pub const ARN_SEGMENT_DB_PARAMETER_GROUP: &str = "db-parameter-group";
/// See [`ARN_SEGMENT_DB_INSTANCE`].
pub const ARN_SEGMENT_DB_BACKUP: &str = "db-backup";

/// The ARN of one resource of `segment`'s family in `region` — the ONE place this service
/// spells an ARN of its own, so every family's ARN has the same shape.
pub fn family_arn(segment: &str, region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{segment}/{id}"))
}

/// The ARN of a DB instance: `arn:aws:timestream-influxdb:<region>:<account>:db-instance/<id>`.
pub fn db_instance_arn(region: &str, id: &str) -> String {
    family_arn(ARN_SEGMENT_DB_INSTANCE, region, id)
}

/// The engine parameter family a DB parameter group configures when its create request
/// selects none.
///
/// `Parameters` is the union of the engine parameter families a group can configure
/// (`smithy_query(query="shape", shape="Parameters")`), and a group configures exactly
/// one of them — so the member is reported for every group, whether or not the request
/// chose a family. Absent a choice the group gets the family of the default engine,
/// InfluxDB v2, with no tunable overridden: every member of `InfluxDBv2Parameters` is
/// optional, so that variant's value object carries none of them.
pub fn default_db_parameter_group_parameters() -> Parameters {
    Parameters::InfluxDBv2(InfluxDBv2Parameters::default())
}

/// The ARN of a DB parameter group:
/// `arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/<id>`, one of the
/// four resource segments the modelled `Arn` pattern allows.
pub fn db_parameter_group_arn(region: &str, id: &str) -> String {
    family_arn(ARN_SEGMENT_DB_PARAMETER_GROUP, region, id)
}

/// The endpoint a client connects to InfluxDB on.
///
/// The DB instance name is the prefix, as `CreateDbInstance`'s `name` documents ("This
/// name will also be a prefix included in the endpoint"), followed by the instance id so
/// two instances that reused a name across regions never share an endpoint.
pub fn db_instance_endpoint(name: &str, id: &str, region: &str) -> String {
    format!("{name}-{id}.{ARN_NAMESPACE}.{region}.on.aws")
}

/// The ARN of the Secrets Manager secret holding the initial InfluxDB authorization
/// parameters for a DB instance, as `influxAuthParametersSecretArn` documents.
pub fn influx_auth_parameters_secret_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:{ARN_NAMESPACE}-{id}"),
    )
}

/// The Availability Zone a DB instance resides in, and the standby's zone.
///
/// A second zone is reported only for the deployment type that places a standby in one,
/// as `secondaryAvailabilityZone` documents ("when deploying with a MultiAZ standby
/// instance").
pub fn availability_zones(region: &str, deployment_type: Option<&str>) -> (String, Option<String>) {
    let primary = format!("{region}a");
    let secondary = (deployment_type == Some(DEPLOYMENT_TYPE_WITH_MULTIAZ_STANDBY))
        .then(|| format!("{region}b"));
    (primary, secondary)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState>)
}
