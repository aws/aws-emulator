// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshift-redshiftserviceversion20121201 service.
//!
//! One store per emulator, holding the provisioned-cluster control plane: the
//! clusters, the snapshots taken of them, the cluster parameter groups and
//! cluster subnet groups they are configured from, the HSM configurations that
//! tell a cluster where to store its database encryption keys, the usage limits
//! set on their features, the snapshot copy grants that let Redshift encrypt
//! copied snapshots in another region, the snapshot schedules a cluster's
//! backups are taken on (see [`StoredSnapshotSchedule`]),
//! the Query Editor (QEV2) and the Redshift
//! IAM Identity Center applications the account registered, and the namespace registrations
//! that expose a cluster's or a serverless namespace's data to a consumer
//! account through the Glue Data Catalog, and the event notification
//! subscriptions that say which Amazon Redshift events are published to an
//! Amazon SNS topic, and the authentication profiles that hold, under a name a
//! client names instead of repeating them, the connection settings it connects
//! with, the Redshift-managed VPC endpoints the account created on its clusters
//! and the endpoint authorizations that say which account may create one,
//! and the resource policies attached to the resources whose ARNs a
//! `PutResourcePolicy` named, and the Amazon Redshift Advisor recommendations
//! recorded against the account's clusters. Every collection keeps
//! INSERTION ORDER, so the `Marker`
//! cursor the paginated `Describe*` operations hand out is a stable, resumable
//! offset into the same sequence a later page walks.
//!
//! Besides the store this module owns the small rules every cluster operation
//! shares: the published engine-version catalog, the published parameter group
//! FAMILIES and the default group of each (see
//! [`published_parameter_group_families`]), the engine default PARAMETERS a
//! parameter group of such a family publishes (see
//! [`ENGINE_DEFAULT_PARAMETERS`]), the documented identifier syntax, the
//! documented `MaxRecords` window, the published maintenance-track catalog (see
//! [`published_maintenance_tracks`]), the published account attributes (see
//! [`published_account_attributes`]), the published event catalog a subscription
//! is scoped against (see [`published_event_catalog`]), every `Cluster` member this service DERIVES
//! from a cluster's own configuration (one rule, [`populate_derived_members`],
//! so the create, modify and read paths cannot report different member sets),
//! the connection endpoint a cluster serves on (one rule, [`cluster_endpoint`],
//! reported only once the cluster is serving),
//! and the clock-owed `creating` -> `available` transitions (see
//! [`PROVISIONING_SPAN`]) and the faster in-place `modifying` -> `available` one
//! an IAM role change owes (see [`IAM_ROLE_APPLY_SPAN`]).

use crate::ops::types::{
    AccountAttribute, AccountWithRestoreAccess, AquaConfiguration, Association,
    AttributeValueTarget,
    AuthenticationProfile, AuthorizeEndpointAccessOutput, AvailabilityZone, CertificateAssociation,
    Cluster, ClusterAssociatedToSchedule, ClusterDbRevision, ClusterIamRole,
    ClusterNode, ClusterParameterGroup, ClusterParameterGroupStatus, ClusterParameterStatus,
    ClusterSubnetGroup, ClusterVersion, CreateEndpointAccessOutput, CreateIntegrationOutput,
    CreateSnapshotScheduleOutput, CreateUsageLimitOutput, Endpoint,
    Event, EventCategoriesMap, EventInfoMap,
    EventSubscription, HsmClientCertificate, HsmConfiguration, InboundIntegration,
    MaintenanceTrack,
    NamespaceIdentifierUnion,
    NetworkInterface, Parameter, PartnerIntegrationInfo, PendingModifiedValues, Qev2IdcApplication,
    RecurringCharge,
    Recommendation,
    RedshiftIdcApplication, ReservedNode, ReservedNodeConfigurationOption,
    ReservedNodeExchangeStatus, ReservedNodeOffering, SecondaryClusterInfo, Snapshot,
    SnapshotCopyGrant, Subnet,
    SupportedOperation,
    SupportedPlatform, UpdateTarget,
    VpcEndpoint, VpcSecurityGroupMembership,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

/// The event a control-plane write raises, and the notification every
/// subscription scoped to it receives.
pub mod notifications;

/// Whether the serverless namespace a registration change names exists, resolved
/// against the control plane that owns it.
pub mod serverless;

/// Whether the zero-ETL source a `CreateIntegration` names exists, resolved
/// against the service that owns the table or bucket it names.
pub mod zero_etl_source;

/// The IAM Identity Center instance an application registration creates its
/// managed application in, resolved against the service that owns the account's
/// instances, and the denial a registration naming another instance carries.
pub mod identity_center;

/// The store's own operations, on the state struct this module declares.
mod store;
/// The wording of the resource-policy ARN resolution in
/// [`resolve_policy_resource`].
mod policy_resources;
/// The cluster record: its derived members, its IAM roles and its shape.
mod clusters;
/// The published event surface and the subscriptions scoped to it.
mod events;
/// The reserved-node offerings, their pricing and the exchange surface.
mod reserved_nodes;
/// The engine parameters a cluster parameter group holds.
mod parameters;
/// The resources a tag can be put on, and the tags they carry.
mod tags;
/// The members a cluster record reports that are COMPUTED rather than stored.
mod derived;
/// The syntax every name this service admits has to satisfy.
mod naming;
/// The subnets a cluster subnet group is made of, resolved against the service
/// that owns them.
mod ec2_subnets;
/// The audit-log delivery a cluster is configured with, and the Amazon S3 bucket
/// it writes to.
mod logging;
/// The resize a cluster's last shape change started, and how a read reports it.
mod resize;
/// The snapshot schedules an account created, and the invocation times computed
/// from their definitions.
mod snapshot_schedules;
/// The scheduled actions an account created: the record each one is, the
/// schedule expression it runs on and the invocation times computed from it.
mod scheduled_actions;
/// The lakehouse configuration a cluster carries, and the registration a
/// `ModifyLakehouseConfiguration` writes onto it.
mod lakehouse;
/// A cluster's DATA, which lives in the SQL engine the Redshift Data API runs
/// statements on: what a snapshot of a cluster copies and a restore writes back.
mod cluster_data;
/// The table restores a `RestoreTableFromClusterSnapshot` asked for, and the
/// status a read reports for each.
mod table_restores;
/// The datashares of this account, and the namespaces a datashare operation
/// names one of.
mod data_shares;
/// The node configurations `DescribeNodeConfigurationOptions` offers for an
/// action, computed from the published orderable catalog.
mod node_configurations;
/// The CloudWatch series a cluster publishes about its own health, and the
/// in-process clock that keeps publishing them. Re-exported below, so a handler
/// reaches it as `crate::state::…` like every other part of this store.
pub mod cluster_metrics;

pub use cluster_metrics::*;

pub use cluster_data::*;
pub use clusters::*;
pub use data_shares::*;
pub use table_restores::*;
pub use ec2_subnets::*;
pub use derived::*;
pub use events::*;
pub use logging::*;
pub use naming::*;
pub use node_configurations::*;
pub use parameters::*;
pub use policy_resources::*;
pub use lakehouse::*;
pub use reserved_nodes::*;
pub use resize::*;
pub use scheduled_actions::*;
pub use snapshot_schedules::*;
pub use tags::*;

/// How long after the call that starts it a provisioning transition falls due:
/// a cluster or a snapshot reported `creating` becomes `available` this long
/// after the `CreateCluster` / `CreateClusterSnapshot` / `DeleteCluster` call
/// that minted it.
///
/// The span is the emulator's own choice — AWS documents no provisioning
/// duration — and is deliberately non-zero, so a caller that acts immediately
/// after the starting call still observes the transient `creating` state. The
/// due instant is stored and nothing ever sleeps for it.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// How long after the call that asks for it an IN-PLACE configuration change
/// falls due: a cluster reported `modifying` because its IAM role set changed
/// becomes `available` again, and each association leaves `adding` / `removing`,
/// this long after the `ModifyClusterIamRoles` call that asked for it.
///
/// Much shorter than [`PROVISIONING_SPAN`], and for the same reason the two
/// spans exist at all: Amazon Redshift documents neither duration, so each is
/// this emulator's own compression of one — and the compression keeps their ORDER
/// and their ratio. Provisioning a new cluster is the slow one; applying a role
/// change to a cluster that is already running is the fast one, and it has to
/// land inside the gap between one call of a client's poll loop and the next. A
/// client reconciling a cluster's roles attaches one, polls until the association
/// is `in-sync`, and repeats that per role; if one change outlived a poll gap,
/// every change would cost that client a whole poll interval for work the service
/// has nothing left to do.
///
/// Non-zero, so the transient `modifying` status and the transient `ApplyStatus`
/// are reported by the call that asked for the change — which is where a client
/// reads them, and what makes the change observable at all. The due instant is
/// stored and nothing ever sleeps for it.
pub const IAM_ROLE_APPLY_SPAN: Duration = Duration::from_millis(1);

/// The default `DBName` when `CreateCluster` does not carry one.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage", member="DBName")`
/// — "Default: dev".
pub const DEFAULT_DB_NAME: &str = "dev";

/// The only Amazon Redshift engine version the service offers.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage", member="ClusterVersion")`
/// — "Only version 1.0 is currently available. Example: 1.0".
pub const DEFAULT_CLUSTER_VERSION: &str = "1.0";

/// `ClusterType` for a cluster of two or more compute nodes.
pub const CLUSTER_TYPE_MULTI_NODE: &str = "multi-node";
/// `ClusterType` for a cluster that is its own single node.
pub const CLUSTER_TYPE_SINGLE_NODE: &str = "single-node";

/// Every `ClusterType` this service offers, in the documented order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterType")` — "Valid Values: multi-node | single-node".
pub const ORDERABLE_CLUSTER_TYPES: [&str; 2] = [CLUSTER_TYPE_MULTI_NODE, CLUSTER_TYPE_SINGLE_NODE];

/// Every node type this service offers, in the documented order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NodeType")` — "Valid Values: dc2.large | dc2.8xlarge | rg.large |
/// rg.xlarge | rg.4xlarge | rg.12xlarge | ra3.large | ra3.xlplus | ra3.4xlarge |
/// ra3.16xlarge". The identical sentence appears on
/// `ModifyClusterMessage$NodeType`, so one list serves both.
pub const ORDERABLE_NODE_TYPES: [&str; 10] = [
    "dc2.large",
    "dc2.8xlarge",
    "rg.large",
    "rg.xlarge",
    "rg.4xlarge",
    "rg.12xlarge",
    "ra3.large",
    "ra3.xlplus",
    "ra3.4xlarge",
    "ra3.16xlarge",
];

/// The node-type family Amazon Redshift publishes as the PREVIOUS generation of
/// compute nodes.
///
/// A cluster that already runs one of these keeps running it — which is why they
/// stay in the `NodeType` valid-value list every operation naming a node type is
/// checked against ([`ORDERABLE_NODE_TYPES`]) — and the reserved-node exchange
/// surface targets them by name ("Returns an array of DC2 ReservedNodeOfferings
/// that matches the payment type, term, and usage price of the given DC1
/// reserved node", `smithy_query(query="doc",
/// operation="GetReservedNodeExchangeOfferings")`). What the family is NOT is a
/// shape a new cluster is ordered with, so the orderable catalog
/// ([`orderable_cluster_types`]) offers none of it.
pub const PREVIOUS_GENERATION_NODE_TYPE_PREFIX: &str = "dc2.";

/// Every node type a SINGLE-NODE cluster is orderable with, in
/// [`ORDERABLE_NODE_TYPES`] order.
///
/// Source: the orderable-shape catalog Amazon Redshift publishes (what
/// `DescribeOrderableClusterOptions` reports). A single-node cluster is the
/// whole cluster on ONE compute node, and the service offers that only on the
/// entry-size member of each current-generation family; every larger member is
/// offered as `multi-node` alone. The model bounds only the VALUE lists —
/// `ClusterType` is "multi-node | single-node" and `NumberOfNodes` "If you don't
/// specify this parameter, you get a single-node cluster"
/// (`smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NumberOfNodes")`) — and says nothing about which node type pairs with
/// which cluster type, so the pairing is published catalog data.
pub const SINGLE_NODE_NODE_TYPES: [&str; 3] = ["rg.large", "ra3.large", "ra3.xlplus"];

/// Every node type an ELASTIC resize can be performed on, which is the first
/// restriction `ResizeCluster` documents for the resize kind it performs by
/// default.
///
/// Source: `smithy_query(query="doc", operation="ResizeCluster")` — "Elastic
/// resize operations have the following restrictions: You can only resize
/// clusters of the following types: dc2.large dc2.8xlarge rg.large rg.xlarge
/// rg.4xlarge rg.12xlarge ra3.large ra3.xlplus ra3.4xlarge ra3.16xlarge".
///
/// That is the SAME list of ten node types [`ORDERABLE_NODE_TYPES`] carries, so
/// every cluster this service admits can take an elastic resize and the
/// restriction refuses none of them: the function below is what says so, and what
/// would start refusing if either published list ever changed.
pub fn supports_elastic_resize(node_type: &str) -> bool {
    ORDERABLE_NODE_TYPES.contains(&node_type)
}

/// Why an elastic resize of a cluster whose node type cannot take one is
/// refused, naming that node type.
///
/// The fixed text is the restriction the operation documents (see
/// [`supports_elastic_resize`]) and the variable part is the cluster's own node
/// type, which is what the caller has to decide from whether to ask for the
/// classic resize instead.
pub fn elastic_resize_node_type_message(node_type: &str) -> String {
    format!(
        "Elastic resize is not supported for clusters of node type {node_type}; retry the \
         request with Classic set to true."
    )
}

/// Why an elastic resize asking for a different node type is refused: "the type
/// of nodes that you add must match the node type for the cluster"
/// (`smithy_query(query="doc", operation="ResizeCluster")`), so changing it is an
/// option that resize kind does not support. Fixed text with no variable part —
/// the restriction is about the option, not about a value.
pub const ELASTIC_RESIZE_KEEPS_NODE_TYPE: &str =
    "An elastic resize cannot change the cluster's node type: the type of nodes that you add \
     must match the node type for the cluster.";

/// Why a request that names one half of a reserved-node exchange without the
/// other is refused.
///
/// The two members are one exchange — "the identifier of the reserved node" and
/// "the identifier of the target reserved node offering" — so neither on its own
/// describes a reservation to move onto an offering.
pub const RESERVED_NODE_EXCHANGE_PAIR_REQUIRED: &str =
    "ReservedNodeId and TargetReservedNodeOfferingId must be specified together: an \
     exchange moves one reservation onto one target offering.";

/// The `ClusterType`s one node type is ORDERABLE as, in the documented
/// [`ORDERABLE_CLUSTER_TYPES`] order, or an empty list for a node type a new
/// cluster cannot be ordered with at all.
///
/// The one rule the orderable catalog is built by: a current-generation node
/// type ([`PREVIOUS_GENERATION_NODE_TYPE_PREFIX`]) is orderable as `multi-node`,
/// and as `single-node` as well when the service offers a one-node cluster of
/// that shape ([`SINGLE_NODE_NODE_TYPES`]).
///
/// This governs what the catalog OFFERS, not what a write accepts: a cluster is
/// created, modified or restored onto any node type the model's valid-value list
/// publishes ([`ORDERABLE_NODE_TYPES`]), including the previous generation that
/// existing clusters run.
pub fn orderable_cluster_types(node_type: &str) -> Vec<&'static str> {
    if node_type.starts_with(PREVIOUS_GENERATION_NODE_TYPE_PREFIX) {
        return Vec::new();
    }
    ORDERABLE_CLUSTER_TYPES
        .into_iter()
        .filter(|cluster_type| {
            *cluster_type != CLUSTER_TYPE_SINGLE_NODE
                || SINGLE_NODE_NODE_TYPES.contains(&node_type)
        })
        .collect()
}

/// The port the Redshift database engine listens on when the request names none.
pub const DEFAULT_PORT: i32 = 5439;

/// The cluster parameter group family an engine version belongs to.
///
/// A Redshift family is named `redshift-<engine version>`, which is why the
/// default parameter group of engine version 1.0 is `default.redshift-1.0`
/// (`smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterParameterGroupName")` — "Default: The default Amazon Redshift
/// cluster parameter group"): the default group OF the `redshift-1.0` family.
pub fn parameter_group_family(cluster_version: &str) -> String {
    format!("redshift-{cluster_version}")
}

/// The DEFAULT cluster parameter group OF one family: `default.` followed by the
/// family name, so the `redshift-1.0` family's default group is
/// `default.redshift-1.0`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterParameterGroupName")` — "Default: The default Amazon Redshift
/// cluster parameter group".
pub fn family_default_parameter_group(family: &str) -> String {
    format!("default.{family}")
}

/// The default cluster parameter group of the family an engine version belongs
/// to, which is what `DescribeClusterVersions` pairs with that version.
pub fn default_parameter_group(cluster_version: &str) -> String {
    family_default_parameter_group(&parameter_group_family(cluster_version))
}

/// The cluster parameter group family whose DEFAULT group a cluster joins when
/// the request names none.
///
/// A cluster's parameter group family is a SERVICE-level default, not a value
/// derived from the engine version the cluster runs: Amazon Redshift reports a
/// freshly created provisioned cluster with `ClusterVersion` 1.0 and the
/// parameter group `default.redshift-2.0` on the same `Cluster`, so deriving the
/// family from the cluster's version would contradict the service. The model
/// bounds only what the family names may be —
/// `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupFamily")`: "The parameter group family names associated
/// with the default parameter groups provide you the valid values" — which is
/// exactly [`published_parameter_group_families`], the families this store seeds
/// a default group for.
pub const DEFAULT_PARAMETER_GROUP_FAMILY: &str = "redshift-2.0";

/// The parameter group a cluster joins when the request names none: the default
/// group of [`DEFAULT_PARAMETER_GROUP_FAMILY`].
pub fn cluster_default_parameter_group() -> String {
    family_default_parameter_group(DEFAULT_PARAMETER_GROUP_FAMILY)
}

/// Every cluster parameter group family this service publishes a DEFAULT group
/// for, in catalog order: the family of each published engine version, then the
/// family new clusters join when that is not already one of them.
///
/// This list IS the `ParameterGroupFamily` catalogue (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_group_families`]),
/// so a cluster can always look its own parameter group up and a
/// `CreateClusterParameterGroup` can always name the family a cluster reports.
pub fn published_parameter_group_families() -> Vec<String> {
    let mut families: Vec<String> = published_cluster_versions()
        .into_iter()
        .filter_map(|version| version.cluster_parameter_group_family)
        .collect();
    if !families
        .iter()
        .any(|family| family == DEFAULT_PARAMETER_GROUP_FAMILY)
    {
        families.push(DEFAULT_PARAMETER_GROUP_FAMILY.to_string());
    }
    families
}

/// The engine BUILD Amazon Redshift publishes for the engine version this
/// service offers.
///
/// Amazon Redshift reports the engine build of a version as that version's
/// `ClusterVersion$Description` (`smithy_query(query="shape",
/// shape="ClusterVersion")` — "The description of the cluster version"), and the
/// build it publishes for engine version 1.0 is this one. A build is catalog
/// data: the same for every account and stable across reads, never a per-run
/// value.
///
/// It is a DIFFERENT published value from the cluster release a maintenance
/// track applies ([`PUBLISHED_TRACK_RELEASES`]): the service publishes the
/// engine build of a VERSION and the release of a TRACK independently, so
/// neither is derived from the other.
pub const PUBLISHED_ENGINE_BUILD: &str = "1.0.49795";

/// Every engine version this service offers, paired with the engine BUILD that
/// version currently runs.
///
/// Source for the version list: `smithy_query(query="doc",
/// shape="CreateClusterMessage", member="ClusterVersion")` — "Only version 1.0
/// is currently available. Example: 1.0". Source for the build:
/// [`PUBLISHED_ENGINE_BUILD`].
pub const PUBLISHED_ENGINE_BUILDS: [(&str, &str); 1] =
    [(DEFAULT_CLUSTER_VERSION, PUBLISHED_ENGINE_BUILD)];

/// `ClusterVersion$Description` for an engine build: the Amazon Redshift engine
/// (ParAccel Analytic DataBase) build string, `PADB <build>`.
///
/// The member identifies WHICH ENGINE BUILD a version corresponds to, so it
/// carries the build rather than restating the version number the caller already
/// has in `ClusterVersion$ClusterVersion`.
pub fn engine_build_description(build: &str) -> String {
    format!("PADB {build}")
}

/// The Amazon Redshift engine versions this service offers, in the order
/// `DescribeClusterVersions` reports them.
///
/// Built from [`PUBLISHED_ENGINE_BUILDS`], so each entry carries the family
/// [`parameter_group_family`] gives its version and the build description
/// [`engine_build_description`] gives its build. Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]
/// when the store is created, so the catalog a read reports and the
/// [`DEFAULT_CLUSTER_VERSION`] a `CreateCluster` applies cannot disagree.
pub fn published_cluster_versions() -> Vec<ClusterVersion> {
    PUBLISHED_ENGINE_BUILDS
        .into_iter()
        .map(|(version, build)| ClusterVersion {
            cluster_version: Some(version.to_string()),
            cluster_parameter_group_family: Some(parameter_group_family(version)),
            description: Some(engine_build_description(build)),
        })
        .collect()
}

/// The DEFAULT cluster parameter group of every published engine version: the
/// groups a fresh account already has, before it creates one of its own.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupFamily")` — "For example, a valid family name is
/// \"redshift-1.0\"" and "The parameter group family names associated with the
/// default parameter groups provide you the valid values": the families these
/// groups carry ARE the catalogue a `CreateClusterParameterGroup` request is
/// checked against (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::parameter_group_families`]).
/// Derived from [`published_parameter_group_families`], so the families
/// `CreateClusterParameterGroup` accepts, the family a cluster's default group
/// belongs to and the groups `DescribeClusterParameterGroups` reports cannot
/// disagree.
pub fn default_parameter_groups() -> Vec<StoredParameterGroup> {
    published_parameter_group_families()
        .into_iter()
        .map(|family| StoredParameterGroup {
            group: ClusterParameterGroup {
                parameter_group_name: Some(family_default_parameter_group(&family)),
                parameter_group_family: Some(family.clone()),
                description: Some(default_parameter_group_description(&family)),
                tags: Some(Vec::new()),
            },
            default_group: true,
            // A default group reports its family's engine defaults and
            // nothing else: nobody has changed a value on it.
            parameter_values: BTreeMap::new(),
        })
        .collect()
}

/// The description Redshift gives a default cluster parameter group.
pub fn default_parameter_group_description(family: &str) -> String {
    format!("Default parameter group for {family}")
}

/// The maintenance track a cluster joins when the request names none.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="MaintenanceTrackName")` — "If you don't provide a maintenance track
/// name, the cluster is assigned to the current track."
pub const DEFAULT_MAINTENANCE_TRACK: &str = "current";

/// The maintenance track that applies the PREVIOUSLY certified maintenance
/// release.
///
/// Source: `smithy_query(query="shape", shape="MaintenanceTrack")` — "If the
/// value for MaintenanceTrack is current, the cluster is updated to the most
/// recently certified maintenance release. If the value is trailing, the cluster
/// is updated to the previously certified maintenance release."
pub const TRAILING_MAINTENANCE_TRACK: &str = "trailing";

/// Every maintenance track this service publishes, in the order
/// `DescribeClusterTracks` reports them.
///
/// Source: `smithy_query(query="shape", shape="MaintenanceTrack")` on
/// `MaintenanceTrackName` — "The name of the maintenance track. Possible values
/// are current and trailing." This list IS the catalogue every operation that
/// names a maintenance track resolves against, so a cluster can always look its
/// own track up in what `DescribeClusterTracks` reports. Read off
/// [`PUBLISHED_TRACK_RELEASES`], so the names the catalog publishes and the
/// releases they apply cannot disagree.
pub const PUBLISHED_MAINTENANCE_TRACKS: [&str; 2] = [
    PUBLISHED_TRACK_RELEASES[0].0,
    PUBLISHED_TRACK_RELEASES[1].0,
];

/// The operations a maintenance track you can switch to supports, in the
/// hyphenated form `SupportedOperation$OperationName` names an operation by.
///
/// Source: `smithy_query(query="shape", shape="SupportedOperation")` —
/// "Describes the operations that are allowed on a maintenance track" — read
/// against the operations whose input carries a `MaintenanceTrackName` and acts
/// on a cluster that already exists: `ModifyClusterMessage$MaintenanceTrackName`
/// ("The name for the maintenance track that you want to assign for the
/// cluster") and `RestoreFromClusterSnapshotMessage$MaintenanceTrackName` ("The
/// name of the maintenance track for the restored cluster"). `CreateCluster`
/// also carries the member but puts a NEW cluster on a track rather than
/// switching an existing one onto it, so it is not an operation a switch target
/// supports. The order is the one the service reports them in: the restore
/// first, then the modify.
pub const TRACK_SWITCH_OPERATIONS: [&str; 2] = ["restore-from-cluster-snapshot", "modify-cluster"];

/// The cluster RELEASE each maintenance track this service publishes applies,
/// in the order `DescribeClusterTracks` reports the tracks.
///
/// `MaintenanceTrack$DatabaseVersion` is "The version number for the cluster
/// release" (`smithy_query(query="shape", shape="MaintenanceTrack")`), and the
/// model publishes no release for either track, so this table IS the catalog the
/// service publishes for the tracks themselves: one patch release of the 1.0
/// cluster-release stream per track, carried PER TRACK rather than derived, for
/// two reasons.
///
/// First, the releases of two tracks are certified separately — `current` applies
/// "the most recently certified maintenance release" and `trailing` "the
/// previously certified" one — so a track's release identifies that track's
/// release and is not a number a reader may ORDER against another track's. What
/// the catalog has to keep is that the two tracks apply DIFFERENT releases, so a
/// client can tell them apart and diff them.
///
/// Second, the release a track applies is not the engine BUILD a version reports
/// as its `ClusterVersion$Description` ([`PUBLISHED_ENGINE_BUILD`]): the service
/// publishes those two values independently, and this emulator publishes each
/// from its own source rather than deriving one from the other.
///
/// A release moves with every certification, so the digits are release-dependent
/// catalog data — the same for every account and stable across reads, never a
/// per-run value.
pub const PUBLISHED_TRACK_RELEASES: [(&str, &str); 2] = [
    (DEFAULT_MAINTENANCE_TRACK, "1.0.434008"),
    (TRAILING_MAINTENANCE_TRACK, "1.0.436211"),
];

/// `MaintenanceTrack$DatabaseVersion` — "The version number for the cluster
/// release" — of one published track, or `None` for a name the catalog does not
/// publish.
///
/// Read off [`PUBLISHED_TRACK_RELEASES`], the one table every operation that
/// reports a track's release resolves against.
pub fn track_database_version(track: &str) -> Option<String> {
    PUBLISHED_TRACK_RELEASES
        .into_iter()
        .find(|(published, _)| *published == track)
        .map(|(_, release)| release.to_string())
}

/// The maintenance-track catalog, in the order `DescribeClusterTracks` reports
/// it.
///
/// Each track carries the release it applies ([`track_database_version`]) and,
/// as its `UpdateTargets`, every OTHER published track — "A maintenance track
/// that you can switch the current track to"
/// (`smithy_query(query="shape", shape="UpdateTarget")`) — with that target's
/// own release and the operations a switch onto it is performed by
/// ([`TRACK_SWITCH_OPERATIONS`]). Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::maintenance_tracks`]
/// when the store is created, so the catalog a read reports and the
/// [`DEFAULT_MAINTENANCE_TRACK`] a `CreateCluster` applies cannot disagree.
pub fn published_maintenance_tracks() -> Vec<MaintenanceTrack> {
    PUBLISHED_MAINTENANCE_TRACKS
        .into_iter()
        .map(|track| MaintenanceTrack {
            maintenance_track_name: Some(track.to_string()),
            database_version: track_database_version(track),
            update_targets: Some(
                PUBLISHED_MAINTENANCE_TRACKS
                    .into_iter()
                    .filter(|target| *target != track)
                    .map(|target| UpdateTarget {
                        maintenance_track_name: Some(target.to_string()),
                        database_version: track_database_version(target),
                        supported_operations: Some(
                            TRACK_SWITCH_OPERATIONS
                                .into_iter()
                                .map(|operation| SupportedOperation {
                                    operation_name: Some(operation.to_string()),
                                })
                                .collect(),
                        ),
                    })
                    .collect(),
            ),
        })
        .collect()
}

/// Which sentence an `InvalidClusterTrackFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`]: the fault's status (400) and
/// awsQuery code (`InvalidClusterTrack`) are what the model fixes, while the
/// fixed text around the requested name is an attribute of the OPERATION —
/// recorded refusals show the operations that would put a cluster ON the track
/// terminating the sentence, and the catalog read leaving it unterminated.
/// Naming each wording here keeps the choice at the handler that makes it, so no
/// operation can quietly inherit another one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidClusterTrackWording {
    /// `Invalid cluster track: <MaintenanceTrackName>.` — what the operations
    /// that would place a cluster on the track they name answer
    /// (`CreateCluster`, `ModifyCluster`, `RestoreFromClusterSnapshot`): the
    /// fixed prefix, the requested name, and a terminating period.
    TrackThenPeriod,
    /// `Invalid cluster track: <MaintenanceTrackName>` — what the catalog read
    /// `DescribeClusterTracks` answers: the same prefix and name, with no
    /// sentence terminator.
    TrackUnterminated,
}

/// The `InvalidClusterTrackFault` message `wording` words, naming the
/// `MaintenanceTrackName` the request supplied.
pub fn invalid_cluster_track_message(
    wording: InvalidClusterTrackWording,
    maintenance_track_name: &str,
) -> String {
    match wording {
        InvalidClusterTrackWording::TrackThenPeriod => {
            format!("Invalid cluster track: {maintenance_track_name}.")
        }
        InvalidClusterTrackWording::TrackUnterminated => {
            format!("Invalid cluster track: {maintenance_track_name}")
        }
    }
}

/// Refuse a `MaintenanceTrackName` the catalog does not publish, with the
/// message the modelled `InvalidClusterTrackFault` carries for the operation
/// that `wording` belongs to.
///
/// One rule for every operation that names a maintenance track, so a name
/// `DescribeClusterTracks` does not report cannot be accepted by a create, a
/// modify or a restore; only the sentence it is refused with is per-operation
/// ([`InvalidClusterTrackWording`]).
pub fn check_maintenance_track(
    wording: InvalidClusterTrackWording,
    name: &str,
) -> Result<(), String> {
    if PUBLISHED_MAINTENANCE_TRACKS.contains(&name) {
        return Ok(());
    }
    Err(invalid_cluster_track_message(wording, name))
}

/// The most days a cluster's maintenance may be deferred for — the
/// account-level allowance `DescribeAccountAttributes` publishes under
/// [`ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION`], and the bound a
/// `ModifyClusterMaintenance` deferral is held to.
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterMaintenanceMessage",
/// member="DeferMaintenanceDuration")` — "An integer indicating the duration of
/// the maintenance window in days. … The duration must be 60 days or less."
pub const MAX_DEFER_MAINTENANCE_DURATION: i32 = 60;

/// The end instant of a deferral that starts at `start` and lasts `days`.
///
/// `DeferMaintenanceDuration` is "the duration of the maintenance window in
/// days", and `DeferMaintenanceEndTime` is "a timestamp indicating end time for
/// the deferred maintenance window" — so a request that names a duration
/// describes the same window as one that names the end time it comes to, and the
/// stored record carries both however the request spelled it. A day is the same
/// count of seconds a day of snapshot retention is ([`SECONDS_PER_DAY`]).
pub fn defer_maintenance_end(
    start: ::aws_smithy_types::DateTime,
    days: i32,
) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(
        start
            .secs()
            .saturating_add(i64::from(days).saturating_mul(SECONDS_PER_DAY)),
    )
}

/// Why a deferral naming BOTH an end time and a duration is refused.
///
/// The rule is the two members' own documentation: "if you specify a duration,
/// you can't specify an end time" and "if you specify an end time, you can't
/// specify a duration" — each value is acceptable on its own and the pair is not,
/// which is the refusal class Amazon Redshift answers with.
pub const DEFER_MAINTENANCE_END_AND_DURATION: &str =
    "DeferMaintenanceEndTime and DeferMaintenanceDuration cannot both be specified: a \
     deferred maintenance window is described by one or the other.";

/// Why a deferral naming NEITHER an end time nor a duration is refused: the two
/// members are the two ways of saying how long the window lasts, so a request
/// carrying neither describes no window. See [`DEFER_MAINTENANCE_END_AND_DURATION`].
pub const DEFER_MAINTENANCE_END_OR_DURATION: &str =
    "Either DeferMaintenanceEndTime or DeferMaintenanceDuration must be specified to \
     describe how long maintenance is deferred for.";

/// Why a `DeferMaintenanceDuration` past the published allowance is refused,
/// naming the value the request carried and the bound.
///
/// The bound is documented ([`MAX_DEFER_MAINTENANCE_DURATION`]) and published as
/// an account attribute, so both appear in the sentence the caller has to fix the
/// request from.
pub fn defer_maintenance_duration_message(requested: i32) -> String {
    format!(
        "Invalid value for DeferMaintenanceDuration: {requested}. The duration must be \
         {MAX_DEFER_MAINTENANCE_DURATION} days or less."
    )
}

/// A fresh `DeferMaintenanceIdentifier` for a deferral whose request named none.
///
/// "A unique identifier for the deferred maintenance window"
/// (`smithy_query(query="doc", shape="ModifyClusterMaintenanceMessage",
/// member="DeferMaintenanceIdentifier")`): a request may bring its own, and one
/// that does not gets one minted by the service and STORED, so every later read
/// of that window reports the same identifier. The model states no form for it,
/// so the form is this emulator's: the prefix Amazon Redshift's own identifiers
/// carry, and then the per-call token every other minted identifier of this
/// service is drawn from ([`new_usage_limit_id`]).
pub fn new_defer_maintenance_identifier() -> String {
    format!("dfm-{}", new_usage_limit_id().replace('-', ""))
}

/// `AccountAttribute$AttributeName` of the deferred-maintenance allowance.
///
/// Amazon Redshift names an account attribute in lower-case hyphenated form,
/// after the member whose value it bounds
/// (`ModifyClusterMaintenanceMessage$DeferMaintenanceDuration`).
pub const ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION: &str = "max-defer-maintenance-duration";

/// One published account attribute: its name and the values it carries, each
/// wrapped in the modelled `AttributeValueTarget` an `AttributeValueList` holds
/// (`smithy_query(query="shape", shape="AttributeValueTarget")` — "Describes an
/// attribute value").
pub fn account_attribute<I>(name: &str, values: I) -> AccountAttribute
where
    I: IntoIterator<Item = String>,
{
    AccountAttribute {
        attribute_name: Some(name.to_string()),
        attribute_values: Some(
            values
                .into_iter()
                .map(|value| AttributeValueTarget {
                    attribute_value: Some(value),
                })
                .collect(),
        ),
    }
}

/// Every account attribute this service publishes, in the order
/// `DescribeAccountAttributes` reports them.
///
/// An entry is "a name value pair that describes an aspect of an account"
/// (`smithy_query(query="shape", shape="AccountAttribute")`) — a published
/// allowance of the ACCOUNT, not a record a caller creates — so the catalog
/// holds the allowances this service publishes for an account: the
/// deferred-maintenance maximum ([`MAX_DEFER_MAINTENANCE_DURATION`]), the
/// documented bound a `ModifyClusterMaintenance` deferral is held to once that
/// operation enforces one. Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::account_attributes`]
/// when the store is created, exactly like the engine-version and
/// maintenance-track catalogs, so the allowance a read reports and the bound
/// that enforcement reads are one constant.
pub fn published_account_attributes() -> Vec<AccountAttribute> {
    vec![account_attribute(
        ATTRIBUTE_MAX_DEFER_MAINTENANCE_DURATION,
        [MAX_DEFER_MAINTENANCE_DURATION.to_string()],
    )]
}

/// The default number of days automated snapshots are retained.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AutomatedSnapshotRetentionPeriod")` — "Default: 1 … Must be a value
/// from 0 to 35."
pub const DEFAULT_AUTOMATED_RETENTION: i32 = 1;

/// Inclusive bounds on `AutomatedSnapshotRetentionPeriod`, from the same doc
/// sentence as [`DEFAULT_AUTOMATED_RETENTION`].
pub const AUTOMATED_RETENTION_BOUNDS: (i32, i32) = (0, 35);

/// Inclusive bounds on a manual-snapshot retention period, which may also be
/// `-1` ("retained indefinitely").
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ManualSnapshotRetentionPeriod")` — "The value must be either -1 or an
/// integer between 1 and 3,653."
pub const MANUAL_RETENTION_BOUNDS: (i32, i32) = (1, 3653);

/// The sentinel manual retention period meaning "retained indefinitely".
pub const MANUAL_RETENTION_INDEFINITE: i32 = -1;

/// Whether a manual retention period is one this service admits: the
/// "indefinitely" sentinel, or a day count inside [`MANUAL_RETENTION_BOUNDS`].
///
/// Source: the doc sentence on [`MANUAL_RETENTION_BOUNDS`], which every member
/// carrying a manual retention period repeats — "The value must be either -1 or
/// an integer between 1 and 3,653."
pub fn manual_retention_admitted(period: i32) -> bool {
    period == MANUAL_RETENTION_INDEFINITE
        || (MANUAL_RETENTION_BOUNDS.0..=MANUAL_RETENTION_BOUNDS.1).contains(&period)
}

/// How many days automated snapshots copied to another Region are retained
/// there when the call that enables the copy names no retention period.
///
/// Source: `smithy_query(query="doc", shape="EnableSnapshotCopyMessage",
/// member="RetentionPeriod")` — "Default: 7."
pub const DEFAULT_SNAPSHOT_COPY_RETENTION: i32 = 7;

/// Inclusive bounds on the automated-snapshot retention period of a cross-Region
/// snapshot copy configuration, from the same doc sentence as
/// [`DEFAULT_SNAPSHOT_COPY_RETENTION`] — "Constraints: Must be at least 1 and no
/// more than 35."
///
/// Narrower than [`AUTOMATED_RETENTION_BOUNDS`], which admits 0 (a cluster that
/// takes no automated snapshots at all): a copy configuration is a standing
/// instruction to keep copies, so it keeps them for at least a day.
pub const SNAPSHOT_COPY_RETENTION_BOUNDS: (i32, i32) = (1, 35);

/// Whether the retention period of a cross-Region snapshot copy configuration is
/// one this service admits: a day count inside
/// [`SNAPSHOT_COPY_RETENTION_BOUNDS`].
pub fn snapshot_copy_retention_admitted(period: i32) -> bool {
    (SNAPSHOT_COPY_RETENTION_BOUNDS.0..=SNAPSHOT_COPY_RETENTION_BOUNDS.1).contains(&period)
}

/// The number of seconds in one day: a manual retention period is a count of
/// days, and every instant this service derives from one is measured in seconds.
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

/// When a manual retention period of `period` days set on a snapshot taken at
/// `created_at` ENDS, in epoch seconds — `None` for a snapshot retained
/// indefinitely, or one whose record carries no creation time.
///
/// The window is counted from the snapshot's own `SnapshotCreateTime`, "the time
/// (in UTC format) when Amazon Redshift began the snapshot"
/// (`smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`).
/// That is what makes `smithy_query(query="doc",
/// shape="ModifyClusterSnapshotMessage", member="ManualSnapshotRetentionPeriod")`
/// — "If the manual snapshot falls outside of the new retention period, you can
/// specify the force option to immediately delete the snapshot" — possible at
/// all: a window counted from the call that sets it could never already have
/// ended.
pub fn manual_retention_end(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
) -> Option<i64> {
    if period == MANUAL_RETENTION_INDEFINITE {
        return None;
    }
    created_at.map(|created| created.secs() + i64::from(period) * SECONDS_PER_DAY)
}

/// Whether the manual retention period `period` of a snapshot taken at
/// `created_at` has ALREADY passed at `now`.
///
/// This is the condition `smithy_query(query="doc",
/// shape="ModifyClusterSnapshotMessage", member="Force")` overrides — "a Boolean
/// option to override an exception if the retention period has already passed".
pub fn manual_retention_has_passed(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) -> bool {
    manual_retention_end(created_at, period).is_some_and(|end| end <= now.secs())
}

/// `Snapshot$ManualSnapshotRemainingDays`: "the number of days until a manual
/// snapshot will pass its retention period"
/// (`smithy_query(query="doc", shape="Snapshot",
/// member="ManualSnapshotRemainingDays")`).
///
/// Counted from `now` to the end of the window ([`manual_retention_end`]) and
/// rounded UP, so a snapshot taken today under a period of `n` days has `n` days
/// remaining and the count reaches zero only once the period has passed.
/// `None` for a snapshot retained indefinitely: it never passes a period.
pub fn manual_snapshot_remaining_days(
    created_at: Option<::aws_smithy_types::DateTime>,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) -> Option<i32> {
    let end = manual_retention_end(created_at, period)?;
    let remaining = end - now.secs();
    if remaining <= 0 {
        return Some(0);
    }
    let days = (remaining as u64).div_ceil(SECONDS_PER_DAY as u64);
    Some(i32::try_from(days).unwrap_or(i32::MAX))
}

/// Set a snapshot's manual retention period and every member it DERIVES.
///
/// One rule for the create, the modify and the read, so no two paths can report
/// different retention bookkeeping for the same period.
pub fn apply_manual_retention(
    snapshot: &mut Snapshot,
    period: i32,
    now: ::aws_smithy_types::DateTime,
) {
    snapshot.manual_snapshot_retention_period = Some(period);
    refresh_manual_retention(snapshot, now);
}

/// Recompute the members a snapshot DERIVES from the manual retention period it
/// already carries, as of `now`.
///
/// `SnapshotRetentionStartTime` is "a timestamp representing the start of the
/// retention period for the snapshot" (`smithy_query(query="doc",
/// shape="Snapshot", member="SnapshotRetentionStartTime")`), which is the instant
/// the window [`manual_retention_end`] measures starts at; a snapshot retained
/// indefinitely is under no retention period, so it reports neither member.
///
/// Called by [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]:
/// the remaining-day count falls as the clock advances whether or not anybody
/// calls, so recomputing it on a read shows nothing a later caller could not
/// already have observed.
pub fn refresh_manual_retention(snapshot: &mut Snapshot, now: ::aws_smithy_types::DateTime) {
    let period = snapshot
        .manual_snapshot_retention_period
        .unwrap_or(MANUAL_RETENTION_INDEFINITE);
    snapshot.snapshot_retention_start_time = if period == MANUAL_RETENTION_INDEFINITE {
        None
    } else {
        snapshot.snapshot_create_time
    };
    snapshot.manual_snapshot_remaining_days =
        manual_snapshot_remaining_days(snapshot.snapshot_create_time, period, now);
}

/// The documented `MaxRecords` window every paginated Redshift `Describe*`
/// operation shares: default 100, minimum 20, maximum 100.
///
/// Source: `smithy_query(query="doc", shape="DescribeClustersMessage",
/// member="MaxRecords")` and the identical sentence on
/// `DescribeClusterSnapshotsMessage$MaxRecords` and
/// `DescribeClusterVersionsMessage$MaxRecords` — "Default: 100 … Constraints:
/// minimum 20, maximum 100."
pub const MAX_RECORDS_DEFAULT: i32 = 100;
/// The documented minimum of the `MaxRecords` window, and the only bound of it
/// a request is refused for naming a value outside. See [`MAX_RECORDS_DEFAULT`]
/// and [`page_bounds_within`].
pub const MAX_RECORDS_MIN: i32 = 20;
/// The documented maximum of the `MaxRecords` window: the most records one page
/// carries, which is both the page a request that names no size is served and
/// the page a request that names MORE is served. NOT a refusal — a request
/// naming more is served, clamped to this value (see [`page_bounds_within`]).
/// See [`MAX_RECORDS_DEFAULT`].
pub const MAX_RECORDS_MAX: i32 = 100;

/// `ClusterIamRole$ApplyStatus` once the association is established: "the role is
/// available for use by the cluster".
///
/// Source: `smithy_query(query="doc", shape="ClusterIamRole",
/// member="ApplyStatus")`, which publishes exactly three statuses — this one,
/// [`IAM_ROLE_APPLY_STATUS_ADDING`] and [`IAM_ROLE_APPLY_STATUS_REMOVING`].
pub const IAM_ROLE_APPLY_STATUS_IN_SYNC: &str = "in-sync";
/// `ClusterIamRole$ApplyStatus` while the role "is in the process of being
/// associated with the cluster". See [`IAM_ROLE_APPLY_STATUS_IN_SYNC`].
pub const IAM_ROLE_APPLY_STATUS_ADDING: &str = "adding";
/// `ClusterIamRole$ApplyStatus` while the role "is in the process of being
/// disassociated" from the cluster. See [`IAM_ROLE_APPLY_STATUS_IN_SYNC`].
pub const IAM_ROLE_APPLY_STATUS_REMOVING: &str = "removing";

/// `ClusterStatus` while the cluster is being provisioned.
pub const STATUS_CREATING: &str = "creating";
/// `ClusterStatus` / snapshot `Status` once provisioning is done.
pub const STATUS_AVAILABLE: &str = "available";
/// `ClusterStatus` while the service is applying a change to a cluster that was
/// already provisioned: the cluster is only intermittently available for queries
/// until the change is applied (see [`availability_status`]).
pub const STATUS_MODIFYING: &str = "modifying";
/// `ClusterStatus` while the cluster is restarting, which is the momentary
/// outage a `RebootCluster` call starts. Like [`STATUS_MODIFYING`] it is work
/// the service is doing to an already provisioned cluster, so it ends at the
/// cluster's own due instant (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]).
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="ClusterStatus")`,
/// which publishes `rebooting` among the cluster states.
pub const STATUS_REBOOTING: &str = "rebooting";
/// `ClusterStatus` while the service is rotating a cluster's encryption keys,
/// which is what a `RotateEncryptionKey` call starts. Like [`STATUS_REBOOTING`]
/// it is work the service is doing to an already provisioned cluster, so it ends
/// at the cluster's own due instant (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]).
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="ClusterStatus")`,
/// which publishes `rotating-keys` among the cluster states.
pub const STATUS_ROTATING_KEYS: &str = "rotating-keys";

/// Why a `RotateEncryptionKey` against a cluster whose data is not encrypted is
/// refused, naming the cluster.
///
/// The rule is the operation's own documented constraint on the value it was
/// given: `smithy_query(query="doc", shape="RotateEncryptionKeyMessage",
/// member="ClusterIdentifier")` — "Constraints: Must be the name of valid cluster
/// that has encryption enabled" — so the fixed text states the constraint and the
/// cluster the request named is the variable part.
pub fn rotation_requires_encrypted_cluster_message(cluster_identifier: &str) -> String {
    format!(
        "Cluster {cluster_identifier} does not have encryption enabled; \
         ClusterIdentifier must be the name of a valid cluster that has encryption enabled."
    )
}
/// `ClusterStatus` a `DeleteCluster` response reports.
pub const STATUS_DELETING: &str = "deleting";
/// `ClusterStatus` of a cluster whose compute has been suspended, which is where
/// a `PauseCluster` leaves it once the pause falls due.
///
/// Source: `smithy_query(query="doc", shape="Cluster", member="ClusterStatus")`,
/// which publishes `paused` among the cluster states and publishes no state for
/// the pause being applied — so the applying state is the one the published
/// vocabulary already has for work the service is doing to a provisioned cluster
/// ([`STATUS_MODIFYING`]), and this is where it settles (see
/// [`StoredCluster::settles_to`]).
pub const STATUS_PAUSED: &str = "paused";

/// Which sentence a `ClusterNotFoundFault` is worded with.
///
/// The fault's status (404) and awsQuery code (`ClusterNotFound`) are what the
/// model fixes; the MESSAGE is an attribute of the OPERATION rather than of the
/// service. Most of the sentences below interpolate the `ClusterIdentifier` the
/// request named, while one leaves the refusal constant — which part is variable
/// is itself per-operation. Naming each wording here keeps the choice
/// at the handler that makes it, so no operation can quietly inherit another
/// one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterNotFoundWording {
    /// `Cluster not found: <ClusterIdentifier>` — what `RebootCluster`,
    /// `ModifyCluster`, `ModifyClusterIamRoles`, `EnableSnapshotCopy`,
    /// `DisableSnapshotCopy`, `ModifySnapshotCopyRetentionPeriod`,
    /// `CreateClusterSnapshot` and `CreateUsageLimit` answer, and the sentence
    /// the two families that dereference a cluster without describing it —
    /// partner integration (`AddPartner`, `DeletePartner`, `DescribePartners`,
    /// `UpdatePartnerStatus`) and custom domain association
    /// (`CreateCustomDomainAssociation`, `ModifyCustomDomainAssociation`,
    /// `DeleteCustomDomainAssociation`) — are recorded answering too.
    NotFoundThenIdentifier,
    /// `Cluster <ClusterIdentifier> not found.` — what `DescribeClusters` and
    /// `DeleteCluster` answer, and the sentence every other cluster-scoped
    /// operation carries until a recorded refusal gives it one of its own.
    IdentifierThenNotFound,
    /// `Cluster <ClusterIdentifier> can't be found.` — what the namespace
    /// registration pair, `RegisterNamespace` and `DeregisterNamespace`, answer.
    IdentifierCannotBeFound,
    /// `The cluster <ClusterIdentifier> was not found.` — what the reserved-node
    /// exchange operations answer (reached through
    /// [`exchange_cluster_not_found_message`]).
    TheClusterWasNotFound,
    /// `The specified cluster identifier can't be found.` — what the endpoint
    /// AUTHORIZATION operations (`AuthorizeEndpointAccess`,
    /// `RevokeEndpointAccess`, `DescribeEndpointAuthorization`) answer. The one
    /// wording with NO variable part: the refusal does not echo the identifier
    /// the request named, so it is passed one only to keep every caller of
    /// [`cluster_not_found_message`] uniform.
    SpecifiedIdentifierCannotBeFound,
}

/// Which sentence a `ClusterSnapshotNotFoundFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`], for the snapshot the request
/// named: the fault's status (404) and awsQuery code (`ClusterSnapshotNotFound`)
/// are what the model fixes, while the fixed text around the identifier is an
/// attribute of the OPERATION, and recorded refusals show both sentences below
/// in use. Naming each wording here keeps the choice at the handler that makes
/// it, so no operation can quietly inherit another one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterSnapshotNotFoundWording {
    /// `ClusterSnapshot not found: <SnapshotIdentifier>` — what the operations
    /// that ACT on the snapshot they name answer (`DeleteClusterSnapshot`,
    /// `ModifyClusterSnapshot`, `CopyClusterSnapshot`,
    /// `AuthorizeSnapshotAccess`, `RevokeSnapshotAccess`,
    /// `RestoreFromClusterSnapshot`), the same shape of sentence the operations
    /// that act on a named cluster answer for a cluster
    /// ([`ClusterNotFoundWording::NotFoundThenIdentifier`]) and the
    /// parameter-group operations answer for a group.
    NotFoundThenIdentifier,
    /// `Snapshot <SnapshotIdentifier> not found.` — what the snapshot READ,
    /// `DescribeClusterSnapshots`, answers: the fixed text is an attribute of
    /// the OPERATION, so a read and a write word the same fault differently for
    /// the same name.
    SnapshotThenNotFound,
}

/// The `ClusterSnapshotNotFoundFault` message `wording` words, naming the
/// `SnapshotIdentifier` the request asked about.
pub fn cluster_snapshot_not_found_message(
    wording: ClusterSnapshotNotFoundWording,
    snapshot_identifier: &str,
) -> String {
    match wording {
        ClusterSnapshotNotFoundWording::NotFoundThenIdentifier => {
            format!("ClusterSnapshot not found: {snapshot_identifier}")
        }
        ClusterSnapshotNotFoundWording::SnapshotThenNotFound => {
            format!("Snapshot {snapshot_identifier} not found.")
        }
    }
}

/// Why an operation that needs an AVAILABLE snapshot refuses one that is not,
/// naming the snapshot and the state it is in.
///
/// `smithy_query(query="shape", shape="InvalidClusterSnapshotStateFault")` — "The
/// specified cluster snapshot is not in the `available` state" — which is the
/// fixed part of the sentence; the snapshot the request named and the state it is
/// actually in are the variable parts a caller needs to decide whether to wait.
/// ONE function for every operation whose precondition is just that, the way
/// [`invalid_cluster_state_message`] serves every operation that needs a running
/// cluster: `DeleteClusterSnapshot` (through [`snapshot_delete_refusal`]),
/// `ModifyClusterSnapshot`, `BatchModifyClusterSnapshots`,
/// `CopyClusterSnapshot`, `AuthorizeSnapshotAccess`,
/// `RestoreFromClusterSnapshot`, `RestoreTableFromClusterSnapshot` and
/// `DescribeNodeConfigurationOptions` all word it through here, so the wording
/// cannot drift between them.
pub fn invalid_snapshot_state_message(snapshot_identifier: &str, status: &str) -> String {
    format!(
        "Cluster snapshot {snapshot_identifier} is in the {status} state, not {STATUS_AVAILABLE}."
    )
}

/// The `ClusterNotFoundFault` message `wording` words, naming the
/// `ClusterIdentifier` the request asked about.
pub fn cluster_not_found_message(
    wording: ClusterNotFoundWording,
    cluster_identifier: &str,
) -> String {
    match wording {
        ClusterNotFoundWording::NotFoundThenIdentifier => {
            format!("Cluster not found: {cluster_identifier}")
        }
        ClusterNotFoundWording::IdentifierThenNotFound => {
            format!("Cluster {cluster_identifier} not found.")
        }
        ClusterNotFoundWording::IdentifierCannotBeFound => {
            format!("Cluster {cluster_identifier} can't be found.")
        }
        ClusterNotFoundWording::TheClusterWasNotFound => {
            format!("The cluster {cluster_identifier} was not found.")
        }
        ClusterNotFoundWording::SpecifiedIdentifierCannotBeFound => {
            "The specified cluster identifier can't be found.".to_string()
        }
    }
}

/// What a `ClusterAlreadyExistsFault` says: the account already holds a cluster
/// under the `ClusterIdentifier` the request named, so nothing was created.
///
/// Amazon Redshift's wording for this fault carries NO variable part — it names
/// neither the identifier nor the operation — so one constant serves every path
/// that raises it (`CreateCluster`, the rename `ModifyCluster` performs and
/// `RestoreFromClusterSnapshot`), the way
/// [`ClusterNotFoundWording::SpecifiedIdentifierCannotBeFound`] carries none for
/// the not-found fault.
pub const CLUSTER_ALREADY_EXISTS: &str = "Cluster already exists";

/// Why a request that would take a `SnapshotIdentifier` the account already
/// holds a snapshot under is refused, naming that identifier.
///
/// `smithy_query(query="shape", shape="ClusterSnapshotAlreadyExistsFault")` —
/// "The value specified as a snapshot identifier is already used by an existing
/// snapshot": the identifier IS what the refusal is about, so unlike
/// [`CLUSTER_ALREADY_EXISTS`] it is interpolated. One sentence for every path
/// that mints a snapshot under a caller-chosen identifier
/// (`CreateClusterSnapshot`, `CopyClusterSnapshot` and the final snapshot
/// `DeleteCluster` takes), so the three cannot drift apart.
pub fn cluster_snapshot_already_exists_message(snapshot_identifier: &str) -> String {
    format!(
        "Cannot create the snapshot because a snapshot with the identifier {snapshot_identifier} already exists."
    )
}

/// The snapshot `Status` a snapshot this service no longer holds reports.
///
/// Source: `smithy_query(query="doc", shape="Snapshot", member="Status")` —
/// "DeleteClusterSnapshot returns status as \"deleted\"": the record a call
/// reports back after removing it from the store.
pub const STATUS_DELETED: &str = "deleted";

/// `RestoreStatus$Status` while the cluster a `RestoreFromClusterSnapshot`
/// minted is still being provisioned.
///
/// Source: `smithy_query(query="doc", shape="RestoreStatus", member="Status")` —
/// "Returns starting, restoring, completed, or failed."
pub const RESTORE_STATUS_RESTORING: &str = "restoring";

/// `RestoreStatus$Status` once the restored cluster is `available`. Applied by
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] with the
/// same clock-owed transition that makes the cluster available. See
/// [`RESTORE_STATUS_RESTORING`] for the published value list.
pub const RESTORE_STATUS_COMPLETED: &str = "completed";

/// The `SnapshotType` of every snapshot a caller asks for, whether through
/// `CreateClusterSnapshot` or as a `DeleteCluster` final snapshot.
///
/// Source: `smithy_query(query="doc", shape="Snapshot", member="SnapshotType")`
/// — "Snapshots created using CreateClusterSnapshot and CopyClusterSnapshot are
/// of type \"manual\"".
pub const SNAPSHOT_TYPE_MANUAL: &str = "manual";

/// The `SnapshotType` Redshift gives a snapshot it took on its own schedule.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterSnapshotsMessage",
/// member="SnapshotType")` — "Valid Values: automated | manual".
pub const SNAPSHOT_TYPE_AUTOMATED: &str = "automated";

/// How many snapshot identifiers ONE batch snapshot operation accepts before it
/// refuses the whole call with its own modelled limit fault.
///
/// Source: `smithy_query(query="shape",
/// shape="BatchModifyClusterSnapshotsLimitExceededFault")` — "The maximum number
/// for snapshot identifiers has been reached. The limit is 100."
pub const BATCH_SNAPSHOT_IDENTIFIER_LIMIT: usize = 100;

/// The `SnapshotErrorMessage$FailureCode` a batch snapshot operation reports for
/// an entry naming no snapshot this store holds.
///
/// `SnapshotErrorMessage$FailureCode` is "the failure code for the error"
/// (`smithy_query(query="shape", shape="SnapshotErrorMessage")`), and a batch
/// operation reports per entry the same condition the one-snapshot operation
/// raises as a fault — so the code is that fault's own wire code,
/// `smithy_query(query="shape", shape="ClusterSnapshotNotFoundFault")` →
/// `awsQueryError` code `ClusterSnapshotNotFound`.
pub const SNAPSHOT_FAILURE_NOT_FOUND: &str = "ClusterSnapshotNotFound";

/// The `SnapshotErrorMessage$FailureCode` for an entry naming a snapshot whose
/// state carries nothing the call can act on.
///
/// The same reasoning as [`SNAPSHOT_FAILURE_NOT_FOUND`]:
/// `smithy_query(query="shape", shape="InvalidClusterSnapshotStateFault")` →
/// `awsQueryError` code `InvalidClusterSnapshotState`.
pub const SNAPSHOT_FAILURE_INVALID_STATE: &str = "InvalidClusterSnapshotState";

/// The `SnapshotErrorMessage$FailureCode` for an entry whose snapshot would fall
/// outside the requested retention window.
///
/// The same reasoning as [`SNAPSHOT_FAILURE_NOT_FOUND`]:
/// `smithy_query(query="shape", shape="InvalidRetentionPeriodFault")` →
/// `awsQueryError` code `InvalidRetentionPeriodFault`.
pub const SNAPSHOT_FAILURE_INVALID_RETENTION_PERIOD: &str = "InvalidRetentionPeriodFault";

/// How many clusters the store holds before `CreateCluster` refuses with the
/// modelled `ClusterQuotaExceededFault`.
///
/// Amazon Redshift publishes an adjustable quota on clusters per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::clusters`]
/// growing without end: every record it holds is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// How many snapshots the store holds before `CreateClusterSnapshot` — and the
/// final snapshot `DeleteCluster` takes — refuses with the modelled
/// `ClusterSnapshotQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshots`], not a
/// claim about an account's published limit. It is larger than the cluster bound
/// because a cluster accumulates snapshots over its life.
pub const SNAPSHOT_QUOTA: usize = 10_000;

/// How many table restore requests the store KEEPS: a request past this count
/// drops the oldest record rather than growing the log
/// ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::record_table_restore`],
/// which the handler passes this bound to).
///
/// The same reasoning as [`SNAPSHOT_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::table_restores`],
/// not a claim about an account's published limit. It is a RING rather than a
/// refusal because nothing deletes a table restore request — `RestoreTableFromClusterSnapshot`
/// appends one per served call and the service publishes no operation that
/// removes one — so a bound that refused instead would make the operation stop
/// working once the log filled.
pub const TABLE_RESTORE_QUOTA: usize = 10_000;

/// How many table restores this emulator has IN FLIGHT before
/// `RestoreTableFromClusterSnapshot` refuses another with the modelled
/// `InProgressTableRestoreQuotaExceededFault`.
///
/// The fault is documented "You have exceeded the allowed number of table restore
/// requests. Wait for your current table restore requests to complete before
/// making a new request", and neither the model nor the shape docs state the
/// number — so this is the emulator's OWN bound on how many copies it will have
/// running at once rather than a claim about an account's published limit. It is
/// what bounds the engine work one caller can have under way;
/// [`TABLE_RESTORE_QUOTA`] bounds what the store RETAINS.
pub const IN_PROGRESS_TABLE_RESTORE_QUOTA: usize = 20;

/// How many accounts one snapshot may be shared with before
/// `AuthorizeSnapshotAccess` refuses with the modelled
/// `AuthorizationQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: neither the model nor the shape docs
/// state a number, so this is the emulator's OWN bound on one snapshot's
/// `Snapshot$AccountsWithRestoreAccess` list rather than a claim about any
/// account's published limit. It is what stops that list growing without end:
/// `RevokeSnapshotAccess` releases an entry.
pub const RESTORE_ACCOUNTS_PER_SNAPSHOT_QUOTA: usize = 20;

/// The identifier a request names to share a snapshot with Amazon Web Services
/// Support, rather than with an account of its own.
///
/// Source: `smithy_query(query="doc", shape="AuthorizeSnapshotAccessMessage",
/// member="AccountWithRestoreAccess")` — "To share a snapshot with Amazon Web
/// Services Support, specify amazon-redshift-support" — and
/// `smithy_query(query="doc", shape="AccountWithRestoreAccess",
/// member="AccountAlias")` — "For Amazon Web Services Support, the identifier is
/// amazon-redshift-support".
pub const SUPPORT_ACCOUNT_ALIAS: &str = "amazon-redshift-support";

/// How many digits an Amazon Web Services account identifier has, which is what
/// [`account_with_restore_access`] admits beside [`SUPPORT_ACCOUNT_ALIAS`].
///
/// Source: `engine::emulator_account_id()`, the account every ARN this service
/// mints is scoped to, is a 12-digit identifier — the one form an Amazon Web
/// Services account id takes.
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The `AccountWithRestoreAccess` record the account an
/// `AuthorizeSnapshotAccess` / `RevokeSnapshotAccess` request names authorizes,
/// or why the value names no account.
///
/// One rule for both operations, so an authorization cannot be stored under a
/// shape a revoke then fails to recognise: an account IDENTIFIER is reported as
/// `AccountId` ("The identifier of an Amazon Web Services account authorized to
/// restore a snapshot") and the documented support alias as `AccountAlias`
/// ("For Amazon Web Services Support, the identifier is
/// `amazon-redshift-support`") — this emulator knows no account number for
/// Amazon Web Services Support, so it invents none.
pub fn account_with_restore_access(value: &str) -> Result<AccountWithRestoreAccess, String> {
    if value == SUPPORT_ACCOUNT_ALIAS {
        return Ok(AccountWithRestoreAccess {
            account_id: None,
            account_alias: Some(value.to_string()),
        });
    }
    if value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(AccountWithRestoreAccess {
            account_id: Some(value.to_string()),
            account_alias: None,
        });
    }
    Err(format!(
        "Invalid value for AccountWithRestoreAccess: '{value}'. It must be a {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account identifier or '{SUPPORT_ACCOUNT_ALIAS}'."
    ))
}

/// Whether two authorization records name the SAME grantee — the account id
/// where one is held, the alias otherwise.
///
/// This is what makes authorizing an account twice an
/// `AuthorizationAlreadyExistsFault` and what a revoke matches the entry to
/// release by.
pub fn same_restore_account(
    left: &AccountWithRestoreAccess,
    right: &AccountWithRestoreAccess,
) -> bool {
    (left.account_id.is_some() && left.account_id == right.account_id)
        || (left.account_alias.is_some() && left.account_alias == right.account_alias)
}

/// How many cluster subnet groups the account's own groups may number before
/// `CreateClusterSubnetGroup` refuses with the modelled
/// `ClusterSubnetGroupQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: Amazon Redshift publishes an
/// adjustable quota on subnet groups per account per Region, and this emulator
/// models neither Service Quotas nor an account's adjustments, so this is the
/// emulator's OWN bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::subnet_groups`]
/// rather than a claim about any account's limit.
pub const SUBNET_GROUP_QUOTA: usize = 1_000;

/// How many subnets one cluster subnet group may hold before
/// `CreateClusterSubnetGroup` refuses with the modelled
/// `ClusterSubnetQuotaExceededFault`.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSubnetGroupMessage",
/// member="SubnetIds")` — "A maximum of 20 subnets can be modified in a single
/// request."
pub const SUBNETS_PER_SUBNET_GROUP_QUOTA: usize = 20;

/// `ClusterSubnetGroup$SubnetGroupStatus` for a group whose every subnet
/// resolved.
///
/// Source: `smithy_query(query="doc", shape="ClusterSubnetGroup",
/// member="SubnetGroupStatus")` — "Possible values are Complete, Incomplete and
/// Invalid." Every subnet this service admits is one it stored, so a stored group
/// is always `Complete`.
pub const SUBNET_GROUP_STATUS_COMPLETE: &str = "Complete";

/// `Subnet$SubnetStatus` for a subnet of a stored group.
pub const SUBNET_STATUS_ACTIVE: &str = "Active";

/// The `Cluster$IpAddressType` values a subnet group of this service supports.
///
/// Source: `smithy_query(query="doc", shape="ClusterSubnetGroup",
/// member="SupportedClusterIpAddressTypes")` — "Possible values are ipv4 and
/// dualstack" — with `Ipv6CidrBlockNotFoundFault`'s own "There are no subnets in
/// your VPC with associated IPv6 CIDR blocks. To use dual-stack mode, associate
/// an IPv6 CIDR block with each subnet in your VPC": this service models no IPv6
/// CIDR block on a subnet, so a stored group supports `ipv4` alone.
pub const SUPPORTED_CLUSTER_IP_ADDRESS_TYPES: [&str; 1] = ["ipv4"];

/// How many `SortingEntities` one `DescribeClusterSnapshots` request may carry.
///
/// `SnapshotAttributeToSortBy` declares three values, so a request naming more
/// entities than there are attributes can only repeat one; the cap keeps the
/// per-request sort work a small constant multiple of one pass over the store,
/// which matters because the sort runs while the shared state lock is held.
pub const MAX_SORTING_ENTITIES: usize = 3;

/// How many usage limits the store holds before `CreateUsageLimit` refuses with
/// the modelled `LimitExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`],
/// not a claim about an account's published limit. A usage limit's identity is
/// the id its create mints, so repeating one configuration appends another
/// record; this bound is what stops that collection growing without end. It is
/// set far above any workflow an emulator serves (every feature of every cluster
/// the store can hold, several times over), and every record it holds is
/// released by `DeleteUsageLimit`.
pub const USAGE_LIMIT_QUOTA: usize = 10_000;

/// How many Query Editor (QEV2) IAM Identity Center applications the store
/// holds before `CreateQev2IdcApplication` refuses with the modelled
/// `LimitExceededFault`.
///
/// The same reasoning as [`USAGE_LIMIT_QUOTA`]: the emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::qev2_idc_applications`],
/// not a claim about an account's published limit. An application's identity is
/// its name, and names are unlimited in number, so this bound is what stops the
/// collection growing without end; it is set far above any workflow an emulator
/// serves, and every record it holds is released by
/// `DeleteQev2IdcApplication`.
pub const QEV2_IDC_APPLICATION_QUOTA: usize = 10_000;

/// How many HSM configurations the store holds before `CreateHsmConfiguration`
/// refuses with the modelled `HsmConfigurationQuotaExceededFault`.
///
/// The same reasoning as [`QEV2_IDC_APPLICATION_QUOTA`]: the emulator's own
/// bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::hsm_configurations`],
/// not a claim about an account's published limit. An HSM configuration's
/// identity is the identifier its create names, and identifiers are unlimited in
/// number, so this bound is what stops the collection growing without end; it is
/// set far above any workflow an emulator serves, and every record it holds is
/// released by `DeleteHsmConfiguration`.
pub const HSM_CONFIGURATION_QUOTA: usize = 10_000;

/// How many snapshot copy grants the store holds before
/// `CreateSnapshotCopyGrant` refuses with the modelled
/// `SnapshotCopyGrantQuotaExceededFault`.
///
/// The same reasoning as [`HSM_CONFIGURATION_QUOTA`]: the emulator's own bound
/// on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_copy_grants`],
/// not a claim about an account's published limit. A grant's identity is the
/// name its create gives it, and names are unlimited in number, so this bound is
/// what stops the collection growing without end; it is set far above any
/// workflow an emulator serves, and every record it holds is released by
/// `DeleteSnapshotCopyGrant`.
pub const SNAPSHOT_COPY_GRANT_QUOTA: usize = 10_000;

/// Which sentence a `SnapshotCopyGrantNotFoundFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`]: the fault's status (400) and
/// awsQuery code (`SnapshotCopyGrantNotFoundFault`) are what the model fixes,
/// while the fixed text around the `SnapshotCopyGrantName` is an attribute of
/// the OPERATION.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotCopyGrantNotFoundWording {
    /// `Snapshot copy grant '<SnapshotCopyGrantName>' not found.` — what the
    /// operations that DEREFERENCE a grant answer: the read
    /// (`DescribeSnapshotCopyGrants`) and the cluster write configured from a
    /// grant (`EnableSnapshotCopy`).
    NameThenNotFound,
    /// `Snapshot copy grant not found: '<SnapshotCopyGrantName>'.` — what the
    /// operation whose TARGET is the grant itself, `DeleteSnapshotCopyGrant`,
    /// answers.
    NotFoundThenName,
}

/// The `SnapshotCopyGrantNotFoundFault` message `wording` words, naming the
/// grant the request asked about.
pub fn snapshot_copy_grant_not_found_message(
    wording: SnapshotCopyGrantNotFoundWording,
    name: &str,
) -> String {
    match wording {
        SnapshotCopyGrantNotFoundWording::NameThenNotFound => {
            format!("Snapshot copy grant '{name}' not found.")
        }
        SnapshotCopyGrantNotFoundWording::NotFoundThenName => {
            format!("Snapshot copy grant not found: '{name}'.")
        }
    }
}

/// Why a create under a name the account already holds a snapshot copy grant
/// under is refused, naming the grant the request asked for.
///
/// The NAME is what an account's grants are unique on, so the refusal names it —
/// single-quoted, as this family words an identifier it reports back inside a
/// sentence.
pub fn snapshot_copy_grant_already_exists_message(name: &str) -> String {
    format!(
        "Cannot create the snapshot copy grant because a grant with the identifier '{name}' already exists."
    )
}

/// How many event notification subscriptions the store holds before
/// `CreateEventSubscription` refuses with the modelled
/// `EventSubscriptionQuotaExceededFault`.
///
/// The same reasoning as [`SNAPSHOT_COPY_GRANT_QUOTA`]: the emulator's own bound
/// on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::event_subscriptions`],
/// not a claim about an account's published limit. A subscription's identity is
/// the name its create gives it, and names are unlimited in number, so this
/// bound is what stops the collection growing without end; it is set far above
/// any workflow an emulator serves, and every record it holds is released by
/// `DeleteEventSubscription`.
pub const EVENT_SUBSCRIPTION_QUOTA: usize = 10_000;

/// How many authentication profiles the store holds before
/// `CreateAuthenticationProfile` refuses with the modelled
/// `AuthenticationProfileQuotaExceededFault`.
///
/// The same reasoning as [`EVENT_SUBSCRIPTION_QUOTA`]: the emulator's own bound
/// on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::authentication_profiles`],
/// not a claim about an account's published limit — the modelled doc
/// (`smithy_query(query="doc", shape="CreateAuthenticationProfileMessage",
/// member="AuthenticationProfileContent")`) says the allowance "is determined by
/// a quota for your account", so there is no published number to enforce. A
/// profile's identity is the name its create gives it, and names are unlimited
/// in number, so this bound is what stops the collection growing without end; it
/// is set far above any workflow an emulator serves, and every record it holds is
/// released by `DeleteAuthenticationProfile`.
pub const AUTHENTICATION_PROFILE_QUOTA: usize = 10_000;

/// Why a request naming no authentication profile is refused.
///
/// Shared by every operation that looks one up — the read, the modify and the
/// delete — so a client sees one message for one condition. The sentence has NO
/// variable part: it names neither the profile the request asked about nor
/// anything else about the request, and tells the caller what to check instead.
pub const AUTHENTICATION_PROFILE_NOT_FOUND: &str =
    "The authentication profile does not exist. Check the name and try the operation again.";

/// Why a create under a name the account already holds a profile under is
/// refused, naming that profile.
///
/// The NAME is what an account's authentication profiles are unique on, so the
/// refusal names it: the caller has to pick another one.
pub fn authentication_profile_exists_message(name: &str) -> String {
    format!("Authentication profile already exists: {name}")
}

/// Why a write carrying a profile map this service cannot read is refused: the
/// document the request carried is not one profile map — a JSON object of
/// settings — so there is nothing to store under the profile.
///
/// Source: `smithy_query(query="doc", shape="CreateAuthenticationProfileMessage",
/// member="AuthenticationProfileContent")` — "The content of the authentication
/// profile in JSON format." The sentence has no variable part: it describes the
/// FORM a profile map must carry and how to supply it, not the content the
/// request sent.
pub const AUTHENTICATION_PROFILE_MAP_NOT_JSON: &str = "The profile map file doesn't contain valid content in JSON format. \
Edit the map file to add valid content in JSON format or specify another map file that contains valid content in JSON format. \
You can include a path to the file. Instead of a file, you can specify a JSON string.";

/// How long one authentication profile's content may be, in characters, before a
/// write refuses it with the modelled
/// `AuthenticationProfileQuotaExceededFault`.
///
/// The emulator's own bound, for the same reason as
/// [`AUTHENTICATION_PROFILE_QUOTA`] and set against the one JSON configuration
/// document this service does publish a maximum for: a workload management
/// configuration's 8000 characters (see [`parameter_value_max_len`]). The
/// modelled doc (`smithy_query(query="doc",
/// shape="CreateAuthenticationProfileMessage",
/// member="AuthenticationProfileContent")`) says the maximum length of the JSON
/// string "is determined by a quota for your account", so there is no published
/// number to enforce — and the content is held in the store for as long as the
/// profile is, so without a bound one request could size the store.
pub const MAX_AUTHENTICATION_PROFILE_CONTENT_LEN: usize = 8_000;

/// Why authentication profile content cannot be stored, classified so each
/// operation answers it with the fault its own model declares for that class.
///
/// One classification for the create and the modify path, so a profile can never
/// hold content the other would have refused.
#[derive(Debug)]
pub enum AuthenticationProfileContentRefusal {
    /// Content that is not one profile map — a JSON object of settings, the
    /// form the member is documented to carry — so this is
    /// `InvalidAuthenticationProfileRequestFault`.
    NotJson(String),
    /// Content longer than this service stores under one profile —
    /// `AuthenticationProfileQuotaExceededFault`, the fault the model declares
    /// for the SIZE of a profile as well as for the number of them.
    TooLong(String),
}

/// Whether authentication profile content is something this service can store.
///
/// The content is kept and reported back VERBATIM, so a write can only check its
/// SIZE and its FORM: it has to be at most
/// [`MAX_AUTHENTICATION_PROFILE_CONTENT_LEN`] characters, and it has to be one
/// profile map CARRYING CONTENT — a JSON object with at least one setting in
/// it. That is what the member is documented to hold, and what the service's own
/// refusal asks a caller to supply: text that is no JSON document, a document
/// that is not an object, and an object with nothing in it all leave the profile
/// with no settings to apply. The size is checked FIRST, so a request cannot
/// make the service parse more than it would ever store.
pub fn check_authentication_profile_content(
    content: &str,
) -> Result<(), AuthenticationProfileContentRefusal> {
    let length = content.chars().count();
    if length > MAX_AUTHENTICATION_PROFILE_CONTENT_LEN {
        return Err(AuthenticationProfileContentRefusal::TooLong(format!(
            "The content of the authentication profile is {length} characters long, which exceeds the maximum of {MAX_AUTHENTICATION_PROFILE_CONTENT_LEN} characters."
        )));
    }
    let carries_settings = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|document| match document {
            serde_json::Value::Object(settings) => Some(settings),
            _ => None,
        })
        .is_some_and(|settings| !settings.is_empty());
    if !carries_settings {
        return Err(AuthenticationProfileContentRefusal::NotJson(
            AUTHENTICATION_PROFILE_MAP_NOT_JSON.to_string(),
        ));
    }
    Ok(())
}

/// How long one resource policy document may be, in characters, before a write
/// refuses it as a document this service cannot hold.
///
/// The emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::resource_policies`],
/// not a claim about a published limit: the document is kept VERBATIM for as
/// long as the policy is attached, so without a bound one request could size the
/// store. Set at the size of the largest policy document AWS accepts anywhere
/// (an IAM resource-based policy's 20 480 characters), so no document the real
/// service would store is refused here.
pub const MAX_RESOURCE_POLICY_LEN: usize = 20_480;

/// How many resources this store holds a policy against before a write refuses
/// to attach another.
///
/// The emulator's own bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::resource_policies`],
/// with the same reasoning as [`NAMESPACE_REGISTRATION_QUOTA`] and the same
/// value: a policy holder is identified by the ARN a request carries rather than
/// by a record this store mints, so this bound is what stops the collection
/// growing without end. It is set far above any workflow an emulator serves, and
/// every entry it holds is released by `DeleteResourcePolicy`.
pub const RESOURCE_POLICY_QUOTA: usize = NAMESPACE_REGISTRATION_QUOTA;

/// What a write answers when the store already holds
/// [`RESOURCE_POLICY_QUOTA`] policies and the request would attach one more.
///
/// It says that the limit is this emulator's, so a caller does not read the
/// refusal as a defect in the document it sent.
pub fn resource_policy_quota_message() -> String {
    format!(
        "Emulator limit, not a defect in this request: this emulator holds at most {RESOURCE_POLICY_QUOTA} resource policies; delete one before attaching another."
    )
}

/// Why a policy document is not one this service can attach to a resource.
///
/// A resource policy is a JSON policy document (`smithy_query(query="doc",
/// shape="ResourcePolicy", member="Policy")` — "The content of a resource
/// policy"), so a write can check its SIZE and its FORM and nothing else: the
/// document is reported back exactly as it arrived.
pub fn check_resource_policy(policy: &str) -> Result<(), String> {
    // The size is checked FIRST, so a request cannot make the service parse
    // more than it would ever store.
    let length = policy.chars().count();
    if length > MAX_RESOURCE_POLICY_LEN {
        return Err(format!(
            "The resource policy is {length} characters long, which exceeds the maximum of {MAX_RESOURCE_POLICY_LEN} characters."
        ));
    }
    match serde_json::from_str::<serde_json::Value>(policy) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        _ => Err(RESOURCE_POLICY_NOT_A_POLICY_DOCUMENT.to_string()),
    }
}

/// Why a request carrying something that is not a JSON policy document is
/// refused.
///
/// The fixed sentence Amazon Redshift answers a policy it could not read with:
/// the document, not the request's other members, is what the refusal is about,
/// so it carries no variable part — and the text is the only thing that tells a
/// parse failure apart from the other causes of `InvalidPolicyFault`.
pub const RESOURCE_POLICY_NOT_A_POLICY_DOCUMENT: &str = "The resource policy can't be parsed.";

/// The services whose resources carry an Amazon Redshift resource policy: the
/// provisioned control plane and Amazon Redshift Serverless, whose namespaces
/// are the resources `PutResourcePolicy` / `GetResourcePolicy` /
/// `DeleteResourcePolicy` attach a policy to, read and detach.
pub const POLICY_RESOURCE_SERVICES: [&str; 2] = ["redshift", "redshift-serverless"];

/// Why the ARN a resource-policy operation carries names no resource this
/// operation works on.
///
/// One classification for the write, the read and the delete, so the three
/// cannot disagree about which ARN names a policy holder.
#[derive(Debug)]
pub enum PolicyResourceRefusal {
    /// The value is no ARN at all ([`is_arn`]), so it names no resource — the
    /// modelled `ResourceNotFoundFault`.
    NotAResource(String),
    /// The ARN names a resource of another service, which carries no Amazon
    /// Redshift resource policy — the modelled `UnsupportedOperationFault`,
    /// declared by all three resource-policy operations.
    UnsupportedResource(String),
    /// The ARN is this service's own and names a resource that does not exist,
    /// so there is no resource to attach a policy to, report one for or detach
    /// one from. The service models no error for it: this is the up-front
    /// parameter refusal, `InvalidParameterValue` (see
    /// [`resolve_policy_resource`]).
    NoSuchResource(String),
}

/// Whether `resource_arn` names a resource an Amazon Redshift resource policy
/// attaches to: an ARN ([`is_arn`]) of one of [`POLICY_RESOURCE_SERVICES`].
///
/// The ARN's SYNTAX and the service that owns the resource, which is as far as
/// the value itself goes; whether that resource EXISTS is resolved against the
/// control plane that owns it ([`resolve_policy_resource`]), which is what the
/// three resource-policy operations call.
pub fn check_policy_resource_arn(resource_arn: &str) -> Result<(), PolicyResourceRefusal> {
    if !is_arn(resource_arn) {
        return Err(PolicyResourceRefusal::NotAResource(
            resource_not_found_message(resource_arn),
        ));
    }
    let service = resource_arn.split(':').nth(2).unwrap_or_default();
    if !POLICY_RESOURCE_SERVICES.contains(&service) {
        return Err(PolicyResourceRefusal::UnsupportedResource(format!(
            "{UNSUPPORTED_POLICY_RESOURCE} {resource_arn}"
        )));
    }
    Ok(())
}

/// Why a request naming a resource of another service is refused.
///
/// Source for the sentence: `smithy_query(query="shape",
/// shape="UnsupportedOperationFault")` — "The requested operation isn't
/// supported" — followed by the ARN the request carried.
pub const UNSUPPORTED_POLICY_RESOURCE: &str =
    "The requested operation isn't supported for resource";

/// Resolve the `ResourceArn` of a resource-policy request: `Ok(())` when it
/// identifies a resource a policy attaches to, and the refusal the operation
/// answers otherwise (worded by [`no_policy_resource_message`]).
///
/// `PutResourcePolicy`, `GetResourcePolicy` and `DeleteResourcePolicy` all take
/// the ARN of the resource the policy is attached to, and a policy is attached
/// TO a resource: an ARN that identifies none is refused by all three, BEFORE
/// any policy state is read or written, so no document is ever attached to,
/// reported for or detached from a resource that is not there. One resolver for
/// the three, as [`check_policy_resource_arn`] is one classification of the
/// ARN's syntax and owning service.
///
/// Where the answer comes from depends on which control plane owns the resource,
/// and NEITHER path leaves this process:
///
/// * a provisioned resource is this store's own, so the answer is a lookup in
///   memory ([`service_state`] →
///   [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::holds_policy_resource`]);
/// * a Redshift Serverless namespace belongs to another control plane with its
///   own store, so that service is asked through [`serverless`], whose client
///   (`serverless::serverless_client`) is built on
///   `ctx.callback().http_client()` and `ctx.callback().aws_credentials_provider()`
///   — the same in-process callback seam `notifications::raise` publishes
///   through. No network client is configured on this path, so nothing here
///   opens a socket or leaves the host, and the caller's ARN never becomes an
///   outbound destination: it is matched against what that service reports.
///
/// Silence follows the rule the namespace registration already sets: a control
/// plane that answers it holds no such namespace settles the question and the
/// request is refused, while one that cannot be asked at all leaves it UNDECIDED
/// and the request is admitted rather than refused on the strength of a peer
/// that never answered.
///
/// The peer lookup is awaited BEFORE this store is locked, so no lock is held
/// across an in-process call.
pub async fn resolve_policy_resource<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    resource_arn: &str,
) -> Result<(), PolicyResourceRefusal> {
    // The ARN's syntax and the service that owns the resource first: a value
    // that is no ARN, or one of a service whose resources carry no Amazon
    // Redshift resource policy, is already answered.
    check_policy_resource_arn(resource_arn)?;

    if serverless::is_serverless_arn(resource_arn) {
        return match serverless::resolve_zero_etl_target(ctx, resource_arn).await {
            // The namespace is there. Whether compute is attached to it says
            // nothing about attaching a policy: the resource exists either way.
            Some(serverless::WarehouseTarget::Active | serverless::WarehouseTarget::NotActive) => {
                Ok(())
            }
            Some(serverless::WarehouseTarget::Absent) => Err(
                PolicyResourceRefusal::NoSuchResource(no_policy_resource_message(resource_arn)),
            ),
            Some(serverless::WarehouseTarget::Unresolvable(reason)) => {
                policy_resource_undecided(resource_arn, &reason);
                Ok(())
            }
            // An ARN of that service naming something other than a namespace:
            // this emulator has no way to ask about it, so the question is as
            // undecided as a peer that did not answer.
            None => {
                policy_resource_undecided(
                    resource_arn,
                    "the ARN names no namespace of the Redshift Serverless control plane",
                );
                Ok(())
            }
        };
    }

    let Some(state) = service_state(engine) else {
        // The store could not be reached, so nothing is known about the
        // resource: undecided, never a refusal of the caller's value.
        policy_resource_undecided(resource_arn, STORE_UNAVAILABLE);
        return Ok(());
    };
    let region = request_region(ctx);
    let held = {
        let store = state.lock();
        store.holds_policy_resource(resource_arn, &region)
    };
    if held {
        Ok(())
    } else {
        Err(PolicyResourceRefusal::NoSuchResource(
            no_policy_resource_message(resource_arn),
        ))
    }
}

/// What a resource-policy operation answers when the resource it names carries
/// no resource policy: there is no document to report, to replace or to detach.
///
/// One sentence for the read and the delete, so a client sees one message for
/// one condition, and it names nothing about the request: the condition is that
/// the POLICY does not exist, not that the resource does not.
pub const RESOURCE_POLICY_NOT_FOUND: &str = "The requested resource policy doesn’t exist.";

/// How many tags one resource may carry before a create that would exceed it
/// refuses with the modelled `TagLimitExceededFault`.
///
/// Source: `smithy_query(query="doc", operation="CreateTags")` — "A resource can
/// have up to 50 tags. If you try to create more than 50 tags for a resource,
/// you will receive an error and the attempt will fail."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `UsageLimit$Period` when `CreateUsageLimit` names none.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="Period")` — "The default is monthly."
pub const DEFAULT_USAGE_LIMIT_PERIOD: &str = "monthly";

/// `UsageLimit$BreachAction` when `CreateUsageLimit` names none.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="BreachAction")` — "The default is log."
pub const DEFAULT_USAGE_LIMIT_BREACH_ACTION: &str = "log";

/// The `LimitType` each `FeatureType` is limited by, which is a property of the
/// FEATURE rather than a choice: a feature metered in minutes takes `time`, one
/// metered in terabytes takes `data-scanned`.
///
/// Source: `smithy_query(query="doc", shape="CreateUsageLimitMessage",
/// member="LimitType")` — "If FeatureType is spectrum, then LimitType must be
/// data-scanned. If FeatureType is concurrency-scaling, then LimitType must be
/// time. If FeatureType is cross-region-datasharing, then LimitType must be
/// data-scanned. If FeatureType is extra-compute-for-automatic-optimization,
/// then LimitType must be time." The keys are every value of
/// `UsageLimitFeatureType`.
pub const USAGE_LIMIT_FEATURE_LIMIT_TYPES: [(&str, &str); 4] = [
    ("spectrum", "data-scanned"),
    ("concurrency-scaling", "time"),
    ("cross-region-datasharing", "data-scanned"),
    ("extra-compute-for-automatic-optimization", "time"),
];

/// The one `LimitType` this feature may be limited by, or `None` for a feature
/// the model does not declare.
pub fn usage_limit_type_of_feature(feature_type: &str) -> Option<&'static str> {
    USAGE_LIMIT_FEATURE_LIMIT_TYPES
        .iter()
        .find(|(feature, _)| *feature == feature_type)
        .map(|(_, limit_type)| *limit_type)
}

/// A fresh `UsageLimitId`.
///
/// A usage limit is "identified by the returned usage limit identifier"
/// (`smithy_query(query="doc", operation="CreateUsageLimit")`): the id is minted
/// by the service, carries nothing from the request, and is what every later
/// call names the limit by — so it is drawn per call rather than derived from
/// the limit's own values, and the minted id is STORED so every read of that
/// limit reports the same one.
///
/// Minted by the same generator this crate already uses for its other
/// wire-visible 8-4-4-4-12 lowercase-hex value, the awsQuery envelope's
/// `<RequestId>`: a per-process sequence number digested with the wall clock, so
/// two limits created in the same nanosecond still get different ids and no new
/// dependency is needed.
pub fn new_usage_limit_id() -> String {
    crate::service::query_error_envelope::new_request_id()
}

/// The `IdcOnboardStatus` of a registered Query Editor (QEV2) IAM Identity
/// Center application.
///
/// The model publishes no value list for the member (`smithy_query(query="doc",
/// shape="Qev2IdcApplication", member="IdcOnboardStatus")` — "The onboarding
/// status for the Amazon Redshift Query Editor (QEV2) IAM Identity Center
/// application"), so the single status this emulator reports is its own choice:
/// the application is registered by the time `CreateQev2IdcApplication`
/// returns — the call itself mints the managed application — so the status it
/// reports is terminal and no transition is owed on it.
pub const QEV2_IDC_ONBOARD_STATUS: &str = "Completed";

/// What every operation taking a `Qev2IdcApplicationArn` answers when the ARN
/// names no registered application: ONE fixed sentence naming the resource type.
///
/// The message carries nothing from the request, so the refusal never reflects
/// a caller's value back and two operations enforcing the same precondition
/// cannot report different wording. The modelled doc text
/// (`smithy_query(query="doc", shape="Qev2IdcApplicationNotExistsFault")`) is
/// the API reference's prose about the fault, not the sentence the service puts
/// on the wire, which is why it is not reused verbatim here.
pub const QEV2_IDC_APPLICATION_NOT_EXISTS: &str =
    "The Qev2IdcApplication you specified doesn't exist.";

/// What `ModifyQev2IdcApplication` answers when no application it may act on
/// matches the ARN — see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::modifiable_qev2_idc_application_position`].
///
/// A refusal states the precondition the operation enforces, and a modify's is
/// wider than a read's or a delete's: the application must exist AND have
/// finished onboarding, because a modify changes the IAM Identity Center managed
/// application that onboarding minted, while a read only reports the
/// registration and a delete only removes it. Like
/// [`QEV2_IDC_APPLICATION_NOT_EXISTS`] it carries nothing from the request.
pub const QEV2_IDC_APPLICATION_NOT_MODIFIABLE: &str =
    "The Qev2IdcApplication you specified doesn't exist or the status isn't Completed.";

/// The ARN of a Query Editor (QEV2) IAM Identity Center application:
/// `arn:aws:redshift:<region>:<account>:qev2idcapplication:<uuid>`.
///
/// Derived from the application's own name and Region, so the value is stable
/// across every read of the same application and is not a per-run value; it is
/// computed ONCE, by the create, and then persisted on the stored application.
///
/// An application deleted and registered again under the same name therefore
/// gets the same ARN, which is intentional: the ARN is content-addressed from
/// the resource's own identity, exactly as [`cluster_arn`] and
/// [`subnet_group_arn`] are, and a name is all an application's identity is.
pub fn qev2_idc_application_arn(application_name: &str, region: &str) -> String {
    let uuid = content_uuid(&format!("qev2idcapplication/{application_name}/{region}"));
    engine::emulator_arn("redshift", region, &format!("qev2idcapplication:{uuid}"))
}

/// The ARN of the IAM Identity Center MANAGED application a registered Query
/// Editor (QEV2) application is backed by:
/// `arn:aws:sso::<account>:application/<instance id>/apl-<token>`.
///
/// IAM Identity Center is a global service, so the ARN carries no Region. The
/// instance segment is the instance the request named (the last path segment of
/// its ARN, which is where an IAM Identity Center instance ARN carries its
/// `ssoins-…` id) and the application segment is derived from that instance and
/// the application's name, so the value is stable across reads rather than
/// per-run.
pub fn idc_managed_application_arn(idc_instance_arn: &str, application_name: &str) -> String {
    let instance = idc_instance_arn
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "ssoins-{}",
                short_digest(&format!("idc-instance/{idc_instance_arn}"), 16)
            )
        });
    let token = short_digest(
        &format!("idc-managed-application/{idc_instance_arn}/{application_name}"),
        16,
    );
    engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/apl-{token}"),
    )
}

/// How many Redshift IAM Identity Center applications the store holds before
/// `CreateRedshiftIdcApplication` refuses with the modelled
/// `RedshiftIdcApplicationQuotaExceededFault`.
///
/// The same reasoning as [`QEV2_IDC_APPLICATION_QUOTA`]: the emulator's own
/// bound on a collection whose records are identified by a NAME, of which there
/// are unboundedly many, so the bound is what keeps
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::redshift_idc_applications`]
/// finite. Amazon Redshift publishes no quota for the collection, and the
/// modelled fault is the service's own way of saying the bound was reached.
pub const REDSHIFT_IDC_APPLICATION_QUOTA: usize = 10_000;

/// The `IdcOnboardStatus` of a registered Redshift IAM Identity Center
/// application.
///
/// The model publishes no value list for the member (`smithy_query(query="doc",
/// shape="RedshiftIdcApplication", member="IdcOnboardStatus")` — "The onboarding
/// status for the Amazon Redshift IAM Identity Center application"), so the
/// single status this emulator reports is its own choice, for the same reason as
/// [`QEV2_IDC_ONBOARD_STATUS`]: the call itself mints the IAM Identity Center
/// managed application, so the application is registered by the time
/// `CreateRedshiftIdcApplication` returns and the status it reports is terminal.
pub const REDSHIFT_IDC_ONBOARD_STATUS: &str = "Completed";

/// What every operation taking a `RedshiftIdcApplicationArn` answers when the
/// ARN names no registered application: ONE sentence, naming the ARN the
/// request supplied.
///
/// One helper rather than a sentence per handler, so the create, the read, the
/// modify, the delete and the cluster operations that take the member cannot
/// report different wording for the same precondition.
pub fn redshift_idc_application_not_exists(application_arn: &str) -> String {
    format!("The RedshiftIdcApplication with the ARN {application_arn} wasn't found.")
}

/// What `CreateRedshiftIdcApplication` answers when the NAME the request
/// carries is one the account already has an application under: one sentence
/// naming that name and telling the caller what to do about it.
pub fn redshift_idc_application_already_exists(application_name: &str) -> String {
    format!(
        "You already have a RedshiftIdcApplication with the name {application_name}. Use a different name."
    )
}

/// What `CreateRedshiftIdcApplication` answers when the account already holds
/// as many applications as it may ([`REDSHIFT_IDC_APPLICATION_QUOTA`]).
pub fn redshift_idc_application_quota_exceeded() -> String {
    format!(
        "The request would exceed the allowed number of Redshift IAM Identity Center applications ({REDSHIFT_IDC_APPLICATION_QUOTA})."
    )
}

/// The ARN of a Redshift IAM Identity Center application:
/// `arn:aws:redshift:<region>:<account>:redshiftidcapplication:<uuid>`.
///
/// Derived from the application's own name and Region exactly as
/// [`qev2_idc_application_arn`] is, so the value is stable across every read of
/// the same application and is not a per-run value; it is computed ONCE, by the
/// create, and then persisted on the stored application.
pub fn redshift_idc_application_arn(application_name: &str, region: &str) -> String {
    let uuid = content_uuid(&format!("redshiftidcapplication/{application_name}/{region}"));
    engine::emulator_arn(
        "redshift",
        region,
        &format!("redshiftidcapplication:{uuid}"),
    )
}

/// The longest an ARN this service STORES may be.
///
/// An ARN-valued member the model constrains only by length (`IdcInstanceArn`
/// is modelled `len 0..2147483647`) would otherwise let one request store a
/// two-gigabyte string, so a value longer than an AWS ARN can be is refused
/// rather than persisted. 2048 is the published maximum length of an AWS ARN
/// (the limit AWS documents for an ARN-valued field, e.g. an IAM role ARN), so
/// no ARN the real service accepts is refused by this bound.
pub const MAX_ARN_LENGTH: usize = 2048;

/// Whether a value is an AWS ARN at all: `arn:`, a non-empty partition and
/// service, and a non-empty resource portion, within [`MAX_ARN_LENGTH`].
///
/// A general check on the FORM of an ARN, not on which resource it names: it
/// says nothing about the partition, Region, account or service a particular
/// member requires. It exists because a member the model constrains only by
/// length can still be documented as an ARN (`smithy_query(query="doc",
/// shape="CreateQev2IdcApplicationMessage", member="IdcInstanceArn")` — "The
/// Amazon Resource Name (ARN) of the IAM Identity Center instance"), so a value
/// that is no ARN names no resource and is refused before anything is stored.
pub fn is_arn(value: &str) -> bool {
    if value.len() > MAX_ARN_LENGTH {
        return false;
    }
    let mut segments = value.split(':');
    segments.next() == Some("arn")
        && segments.next().is_some_and(|partition| !partition.is_empty())
        && segments.next().is_some_and(|service| !service.is_empty())
        // region, account and the resource portion: an ARN has at least six
        // colon-separated segments, and the resource portion is never empty.
        && segments.next().is_some()
        && segments.next().is_some()
        && segments.next().is_some_and(|resource| !resource.is_empty())
}

/// What an operation answers when an ARN-valued member carries a value longer
/// than an ARN can be ([`MAX_ARN_LENGTH`]): the member, the length it carried
/// and the bound.
///
/// One helper rather than a sentence per handler, so two operations refusing
/// the same over-long value cannot report different wording.
pub fn arn_too_long_message(member: &str, length: usize) -> String {
    format!(
        "Invalid value for {member}: the value is {length} characters long; an Amazon Resource Name (ARN) is at most {MAX_ARN_LENGTH} characters."
    )
}

/// What an operation answers when an ARN-valued member carries a value that is
/// no ARN at all (see [`is_arn`]): the member and the value it carried.
///
/// A member documented as naming ONE kind of resource states that in its own
/// refusal instead (see `create_qev2_idc_application.rs`); this is the sentence
/// for a member documented only as an ARN.
pub fn not_an_arn_message(member: &str, value: &str) -> String {
    format!(
        "Invalid value for {member}: '{value}'. The value must be an Amazon Resource Name (ARN)."
    )
}

/// Check the trusted token issuer ARNs one request names, naming the member and
/// the restriction it breaks. An `Err` is an `InvalidParameterValue`.
///
/// `TrustedTokenIssuerArn` is modelled only by length, exactly like the
/// application's own ARN-valued members, so the same rule applies to it: a
/// value that is no ARN names no token issuer. One helper rather than a copy
/// per handler, so the create and the modify hold the member to the same rule.
pub fn check_authorized_token_issuers(
    issuers: &[crate::ops::types::AuthorizedTokenIssuer],
) -> Result<(), String> {
    for (position, issuer) in issuers.iter().enumerate() {
        let Some(issuer_arn) = issuer.trusted_token_issuer_arn.as_deref() else {
            continue;
        };
        let member = format!(
            "AuthorizedTokenIssuerList.member.{}.TrustedTokenIssuerArn",
            position + 1
        );
        if issuer_arn.len() > MAX_ARN_LENGTH {
            return Err(arn_too_long_message(&member, issuer_arn.len()));
        }
        if !is_arn(issuer_arn) {
            return Err(not_an_arn_message(&member, issuer_arn));
        }
    }
    Ok(())
}

/// What a handler answers when the shared store cannot be reached: the service's
/// own modelled "a dependent internal service is temporarily unavailable" fault.
pub const STORE_UNAVAILABLE: &str =
    "the Amazon Redshift control-plane store is temporarily unavailable";

/// How long after the call that asks for it a namespace REGISTRATION change
/// falls due.
///
/// Two things fall due at that instant, both started by the same call: a
/// provisioned cluster whose namespace registration changed is reported
/// [`STATUS_MODIFYING`] until then and [`STATUS_AVAILABLE`] from then on (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::begin_namespace_registration_change`]),
/// and a registration a `DeregisterNamespace` reported as leaving is GONE from
/// then on — the namespace is registered to that consumer no longer.
///
/// The span is the emulator's own choice, for the same reason and with the same
/// shape as [`PROVISIONING_SPAN`]: Amazon Redshift documents no duration for a
/// registration change, and the span is deliberately non-zero so a caller that
/// polls the cluster straight after the call still observes the change being
/// applied. The due instant is stored and nothing ever sleeps for it.
pub const NAMESPACE_REGISTRATION_SPAN: Duration = Duration::from_millis(500);

/// How many namespace registrations the store holds before
/// `RegisterNamespace` refuses to record another one.
///
/// The same reasoning as [`SNAPSHOT_COPY_GRANT_QUOTA`]: the emulator's own bound
/// on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::namespace_registrations`],
/// not a claim about an account's published limit. A registration's identity is
/// the (namespace, consumer account) pair the request carries, and this bound is
/// what stops the collection growing without end; it is set far above any
/// workflow an emulator serves, and every record it holds is released by
/// `DeregisterNamespace`.
pub const NAMESPACE_REGISTRATION_QUOTA: usize = 10_000;

/// The registration status reported for a namespace a `RegisterNamespace` call
/// has just registered.
///
/// Source: `smithy_query(query="shape", shape="NamespaceRegistrationStatus")` —
/// the enum carries exactly `Registering` and `Deregistering`.
pub const NAMESPACE_REGISTERING: &str = "Registering";

/// The registration status reported for a namespace a `DeregisterNamespace`
/// call has just deregistered; same source as [`NAMESPACE_REGISTERING`].
pub const NAMESPACE_DEREGISTERING: &str = "Deregistering";

/// Which namespace a registration request names: the namespace of a
/// provisioned cluster, or a serverless namespace reached through one of its
/// workgroups.
///
/// ONE resolution of the modelled union for both `RegisterNamespace` and
/// `DeregisterNamespace` (see [`registered_namespace`]), so a registration can
/// never be stored under a key the deregister then fails to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisteredNamespace {
    /// The namespace of the provisioned cluster with this `ClusterIdentifier`.
    Provisioned {
        /// The cluster whose namespace is registered.
        cluster_identifier: String,
    },
    /// The serverless namespace with this identifier, reached through the
    /// workgroup with this one.
    Serverless {
        /// The serverless namespace that is registered.
        namespace_identifier: String,
        /// The workgroup the request reaches that namespace through.
        workgroup_identifier: String,
    },
}

impl RegisteredNamespace {
    /// How a refusal or a message names this namespace.
    pub fn described(&self) -> String {
        match self {
            Self::Provisioned { cluster_identifier } => {
                format!("cluster {cluster_identifier}")
            }
            Self::Serverless {
                namespace_identifier,
                workgroup_identifier,
            } => format!(
                "serverless namespace {namespace_identifier} of workgroup {workgroup_identifier}"
            ),
        }
    }
}

/// The namespace a modelled `NamespaceIdentifierUnion` names, as this store
/// keys it.
pub fn registered_namespace(identifier: &NamespaceIdentifierUnion) -> RegisteredNamespace {
    match identifier {
        NamespaceIdentifierUnion::ProvisionedIdentifier(provisioned) => {
            RegisteredNamespace::Provisioned {
                cluster_identifier: provisioned.cluster_identifier.clone(),
            }
        }
        NamespaceIdentifierUnion::ServerlessIdentifier(serverless) => {
            RegisteredNamespace::Serverless {
                namespace_identifier: serverless.namespace_identifier.clone(),
                workgroup_identifier: serverless.workgroup_identifier.clone(),
            }
        }
    }
}

/// Why a namespace a registration request names can take no registration
/// change.
///
/// One rule for `RegisterNamespace` and `DeregisterNamespace` (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::registration_target`]);
/// each operation maps it to its OWN modelled error, because the two declare
/// the same three and must answer the same one for the same cause.
#[derive(Debug, Clone)]
pub enum NamespaceRefusal {
    /// No cluster this account holds carries the identifier the request names.
    ClusterNotFound(String),
    /// The cluster is not in the state a registration change is admitted in.
    InvalidClusterState(String),
    /// The request names no namespace that could exist.
    InvalidNamespace(String),
}

/// The consumer account id a registration request names, or why the value names
/// no account.
///
/// `ConsumerIdentifiers` is "an array containing the ID of the consumer account"
/// (`smithy_query(query="doc", shape="RegisterNamespaceInputMessage",
/// member="ConsumerIdentifiers")`) and the model constrains it only by length,
/// so the one form an account id takes — [`ACCOUNT_ID_DIGITS`] digits, as
/// [`account_with_restore_access`] also admits — is checked through
/// [`account_id_value`] rather than storing a registration for an account that
/// cannot exist.
pub fn consumer_account_id(value: &str) -> Result<String, String> {
    account_id_value("ConsumerIdentifiers", value)
}

/// A namespace registration as this service stores it: which namespace is
/// registered to which consumer account, and the registration status the call
/// that last changed it reported.
///
/// The `NamespaceRegistrationStatus` enum carries only the two transient values
/// (see [`NAMESPACE_REGISTERING`]), so a record that exists is a namespace
/// registered to that consumer and a record a deregister marked is one the
/// clock is about to remove (see [`StoredNamespaceRegistration::removed_at`]).
#[derive(Debug, Clone)]
pub struct StoredNamespaceRegistration {
    /// The namespace that is registered.
    pub namespace: RegisteredNamespace,
    /// The consumer account the namespace is registered to.
    pub consumer_identifier: String,
    /// [`NAMESPACE_REGISTERING`] or [`NAMESPACE_DEREGISTERING`].
    pub status: String,
    /// When the clock-owed REMOVAL of this record falls due, set by the
    /// deregister that asked for it and `None` while the namespace is
    /// registered. See [`NAMESPACE_REGISTRATION_SPAN`].
    pub removed_at: Option<Instant>,
}

/// A cluster as this service stores it: the modelled projection a response is
/// built from, plus what the model does not carry.
#[derive(Debug, Clone)]
pub struct StoredCluster {
    /// Exactly what `DescribeClusters` reports, so a read never recomputes it.
    pub cluster: Cluster,
    /// `single-node` or `multi-node`. The `Cluster` shape models no
    /// `ClusterType` member, but `ModifyCluster` accepts one, so the resolved
    /// value is kept here.
    pub cluster_type: String,
    /// The admin password, stored so a later `ModifyCluster` can replace it and
    /// NEVER placed on [`StoredCluster::cluster`] — the `Cluster` shape models
    /// no password member, so it can never reach the wire.
    pub master_user_password: Option<String>,
    /// When the clock-owed changes the last call against this cluster started
    /// fall due: the `creating` -> `available` transition, the admin password a
    /// create or a modify asked for, and the `adding` / `removing` IAM role
    /// associations a `ModifyClusterIamRoles` reported as in progress.
    pub available_at: Instant,
    /// The connection endpoint this cluster serves on once it is up: its
    /// address, its port and its VPC interface endpoint ([`cluster_endpoint`]).
    ///
    /// Decided when the cluster is allocated and kept here from then on, because
    /// a cluster that is still being provisioned HAS no published endpoint:
    /// `Cluster$Endpoint` is reported only once the cluster is serving, so
    /// [`StoredCluster::cluster`] carries it only from the moment this record's
    /// provisioning completed (see [`StoredCluster::publish_endpoint`]).
    pub endpoint: Endpoint,
    /// The users of this cluster's DATABASE, each with the database groups it
    /// belongs to.
    ///
    /// A database user is not a modelled member of any shape — it lives in the
    /// cluster's database rather than in the control plane — so it is held here
    /// and can never reach the wire. The admin user the create named is in it
    /// from the moment the cluster is allocated ([`initial_database_users`]),
    /// and `GetClusterCredentials` adds one when its `AutoCreate` asks for a
    /// user the database does not hold yet (see
    /// [`StoredCluster::create_database_user`]). Keyed by user name, so the
    /// database holds at most one user under a name however often credentials
    /// are issued for it.
    pub database_users: BTreeMap<String, BTreeSet<String>>,
    /// How this cluster's audit logging is configured: what `EnableLogging` put
    /// on it and `DescribeLoggingStatus` reports back.
    ///
    /// A cluster nobody enabled logging on carries the DEFAULT record, which is
    /// logging disabled — an account that has configured nothing still has a
    /// logging status to read (see [`ClusterLogging`]).
    pub logging: ClusterLogging,
    /// The LAST resize of this cluster, and `None` for a cluster whose shape has
    /// never been changed — which is what makes `DescribeResize` answer
    /// `ResizeNotFound` for it (see [`StoredResize`]).
    pub resize: Option<StoredResize>,
    /// Which `ClusterStatus` the clock-owed settle moves this cluster to when
    /// [`StoredCluster::available_at`] falls due, for the work that does NOT end
    /// with the cluster serving again.
    ///
    /// `None` — the usual case — is work after which the cluster is
    /// [`STATUS_AVAILABLE`]: provisioning it, applying a change to it, restarting
    /// it. A suspended cluster is the one exception the published status
    /// vocabulary carries ([`STATUS_PAUSED`]): the pause a `PauseCluster` starts
    /// is applied like any other change and the cluster is NOT available at the
    /// end of it, so the status it settles to is recorded here rather than
    /// assumed by
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`].
    pub settles_to: Option<String>,
    /// The IAM Identity Center application whose trusted identity propagation
    /// this cluster's lakehouse configuration is associated with, and `None` for
    /// a cluster no `ModifyLakehouseConfiguration` associated one with.
    ///
    /// Held here rather than on [`StoredCluster::cluster`] because the `Cluster`
    /// shape models no member for it — unlike the registration status and the
    /// catalog of the same configuration, which ARE `Cluster` members
    /// (`$LakehouseRegistrationStatus`, `$CatalogArn`) and so live there.
    pub lakehouse_idc_application_arn: Option<String>,
}

impl StoredCluster {
    /// Report this cluster's connection endpoint from now on: the cluster is up,
    /// so the address and port a client connects to are its to publish.
    ///
    /// Called by the transition that finishes provisioning
    /// ([`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]) and
    /// by nothing else: a cluster reported `creating` has no endpoint, which is
    /// what stops a client that polls for one from dialling a cluster that is
    /// not serving yet.
    pub fn publish_endpoint(&mut self) {
        self.cluster.endpoint = Some(self.endpoint.clone());
    }

    /// Re-report an endpoint that is ALREADY published, after a change moved it
    /// (a new port, or the new hostname a rename moves the cluster to).
    ///
    /// A cluster that has not finished provisioning still publishes none, so a
    /// change applied while it is coming up moves the endpoint it will serve on
    /// without making it visible early.
    pub fn refresh_endpoint(&mut self) {
        if self.cluster.endpoint.is_some() {
            self.publish_endpoint();
        }
    }

    /// Create the database user `db_user`, joined to `groups` beside
    /// [`PUBLIC_DATABASE_GROUP`], unless this cluster's database already holds a
    /// user under that name or holds as many users as it may.
    ///
    /// The rule is `GetClusterCredentials$AutoCreate`, "Create a database user
    /// with the name specified for the user named in DbUser IF ONE DOES NOT
    /// EXIST", together with `GetClusterCredentials$DbGroups`: the groups a
    /// request names are joined "for the current session, IN ADDITION TO any
    /// group memberships for an existing user", so a user the database already
    /// holds keeps exactly the memberships it has — a session's groups are the
    /// session's, not a change to the user — and a new one "is added only to
    /// PUBLIC" when the request names none.
    ///
    /// A created user outlives the call that created it and is released only
    /// with the cluster, so the count is bounded by
    /// [`DATABASE_USERS_PER_CLUSTER_QUOTA`]: a request that would take the
    /// database past it creates nothing and is told so.
    pub fn create_database_user(
        &mut self,
        db_user: &str,
        groups: &[String],
    ) -> DatabaseUserCreation {
        if self.database_users.contains_key(db_user) {
            return DatabaseUserCreation::AlreadyPresent;
        }
        if self.database_users.len() >= DATABASE_USERS_PER_CLUSTER_QUOTA {
            return DatabaseUserCreation::QuotaExceeded;
        }
        let mut joined = BTreeSet::from([PUBLIC_DATABASE_GROUP.to_string()]);
        joined.extend(groups.iter().cloned());
        self.database_users.insert(db_user.to_string(), joined);
        DatabaseUserCreation::Created
    }
}

/// What [`StoredCluster::create_database_user`] did with the name it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseUserCreation {
    /// The database held no user under that name, and now holds one.
    Created,
    /// The database already held a user under that name, which is left exactly
    /// as it stands.
    AlreadyPresent,
    /// The database holds as many users as it may
    /// ([`DATABASE_USERS_PER_CLUSTER_QUOTA`]), so no user was created.
    QuotaExceeded,
}

/// How many users one cluster's database holds before `GetClusterCredentials`
/// refuses to create another.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: neither the model nor the shape docs
/// state a number, so this is the emulator's OWN bound on
/// [`StoredCluster::database_users`] rather than a claim about any account's
/// published limit. It is what stops that map growing without end — a database
/// user outlives the call that created it and is released only when the cluster
/// is deleted — and it is far above any workflow an emulator serves.
pub const DATABASE_USERS_PER_CLUSTER_QUOTA: usize = 1_000;

/// Why a request whose `AutoCreate` would take a cluster's database past
/// [`DATABASE_USERS_PER_CLUSTER_QUOTA`] is refused, naming the cluster and the
/// bound.
pub fn database_user_quota_message(cluster_identifier: &str) -> String {
    format!(
        "Emulator limit, not a defect in this request: the database of cluster \
         {cluster_identifier} holds at most {DATABASE_USERS_PER_CLUSTER_QUOTA} users; \
         no further user can be created in it."
    )
}

/// How many database groups one session may join.
///
/// The same reasoning as [`DATABASE_USERS_PER_CLUSTER_QUOTA`]: the model bounds
/// `GetClusterCredentials$DbGroups` by nothing at all, so this is the
/// emulator's own bound on how many names one request may carry — and, because a
/// created user keeps the groups it joined, on how many a single call can place
/// in the store.
pub const MAX_DB_GROUPS_PER_SESSION: usize = 50;

/// Why a request naming more than [`MAX_DB_GROUPS_PER_SESSION`] database groups
/// is refused, naming the count it carried and the bound.
pub fn db_groups_limit_message(requested: usize) -> String {
    format!(
        "Emulator limit, not a defect in this request: a session may join at most \
         {MAX_DB_GROUPS_PER_SESSION} database groups; DbGroups named {requested}."
    )
}

/// The database group every user of a Redshift database belongs to.
///
/// Source: `smithy_query(query="doc", shape="GetClusterCredentialsMessage",
/// member="DbGroups")` — "If not specified, a new user is added only to
/// PUBLIC" — and the same member's `DbUser` sibling, "a new user is created
/// using the value for DbUser with PUBLIC permissions".
pub const PUBLIC_DATABASE_GROUP: &str = "PUBLIC";

/// The database users a cluster's database holds the moment the cluster is
/// allocated: the admin user the create named, in [`PUBLIC_DATABASE_GROUP`].
///
/// `MasterUsername` is "the user name associated with the admin user account for
/// the cluster", so that user EXISTS in the database from the start — which is
/// what makes `GetClusterCredentials$AutoCreate`'s "if one does not exist" mean
/// something for it. A cluster record carrying no admin user name (one seeded
/// straight into the store) holds no database user.
pub fn initial_database_users(master_username: Option<&str>) -> BTreeMap<String, BTreeSet<String>> {
    master_username
        .map(|name| {
            BTreeMap::from([(
                name.to_string(),
                BTreeSet::from([PUBLIC_DATABASE_GROUP.to_string()]),
            )])
        })
        .unwrap_or_default()
}

/// A cluster parameter group as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredParameterGroup {
    /// Exactly what `DescribeClusterParameterGroups` reports, so a read never
    /// recomputes it.
    pub group: ClusterParameterGroup,
    /// Whether Redshift itself owns the group (a DEFAULT parameter group, which
    /// every account has for each engine version) rather than the account having
    /// created it. A default group is outside [`PARAMETER_GROUP_QUOTA`] and its
    /// family is one of the valid `ParameterGroupFamily` values.
    pub default_group: bool,
    /// The parameter values a caller SET on this group, by parameter name.
    ///
    /// ONLY the overrides: a parameter the group never changed reports its
    /// family's engine default (see [`family_parameters`]), so a stored group is
    /// never a copy of the catalog and a group of a family whose catalog grows
    /// reports the new settings too.
    pub parameter_values: BTreeMap<String, String>,
}

impl StoredParameterGroup {
    /// The parameters this group reports, in the catalog's `ParameterName`
    /// order: its family's engine defaults, each carrying the value this group
    /// holds for it and `user` as its `Source` where the group changed it.
    pub fn parameters(&self) -> Vec<Parameter> {
        let minimum_engine_version = minimum_engine_version();
        family_parameters(self.group.parameter_group_family.as_deref().unwrap_or_default())
            .iter()
            .map(|parameter| {
                parameter.to_api(
                    self.parameter_values.get(parameter.name).map(String::as_str),
                    minimum_engine_version.as_deref(),
                )
            })
            .collect()
    }

    /// The catalog entry for the parameter `name` of this group's family, or
    /// `None` when the family publishes no such parameter — which is what makes
    /// a request naming one a request naming nothing this group holds.
    pub fn family_parameter(&self, name: &str) -> Option<&'static EngineParameter> {
        family_parameters(self.group.parameter_group_family.as_deref().unwrap_or_default())
            .iter()
            .find(|parameter| parameter.name == name)
    }
}

/// A snapshot as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredSnapshot {
    /// Exactly what `DescribeClusterSnapshots` reports.
    pub snapshot: Snapshot,
    /// When the `creating` -> `available` transition falls due.
    pub available_at: Instant,
    /// The cluster the snapshot was taken OF, as it stood at that instant —
    /// which is what `RestoreFromClusterSnapshot` rebuilds a cluster from.
    ///
    /// Captured when the snapshot is taken rather than read off the source
    /// cluster at restore time, because "a snapshot contains a copy of the
    /// cluster data as of this exact time"
    /// (`smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`):
    /// the source cluster may have been modified, renamed or deleted since, and
    /// a restore must still rebuild the configuration of that instant. It also
    /// carries what the `Snapshot` shape models no member for (the admin
    /// password, the maintenance window, the retention periods), so the restored
    /// cluster can use "the admin credentials the cluster had at the time the
    /// snapshot was taken".
    ///
    /// `None` for a snapshot record no `CreateClusterSnapshot` / `DeleteCluster`
    /// final snapshot minted (one seeded straight into the store): a restore
    /// from it rebuilds from the modelled [`StoredSnapshot::snapshot`] members
    /// and this service's documented defaults alone.
    pub source: Option<StoredCluster>,
    /// The cluster's DATABASES as they stood when the snapshot was taken —
    /// "a snapshot contains a copy of the cluster data as of this exact time"
    /// — so a `RestoreFromClusterSnapshot` serves the rows written before it was
    /// taken and none of those written after.
    ///
    /// Taken through the Data API's own seam ([`capture_cluster_data`]) and
    /// opaque here; held behind an `Arc` because a snapshot's data is also the
    /// data of every copy `CopyClusterSnapshot` makes of it, and because a
    /// restore reads it with no store lock held. EMPTY for a snapshot of a
    /// cluster no statement has run against, and for a record seeded straight
    /// into the store.
    pub data: ::std::sync::Arc<ClusterData>,
}

/// An HSM configuration as this service stores it: the modelled projection a
/// response is built from, plus the two request values the `HsmConfiguration`
/// shape models no member for.
#[derive(Debug, Clone)]
pub struct StoredHsmConfiguration {
    /// Exactly what `DescribeHsmConfigurations` reports, so a read never
    /// recomputes it.
    pub configuration: HsmConfiguration,
    /// The HSM's public certificate file, kept verbatim — a cluster configured
    /// with this HSM connects to the HSM with it.
    ///
    /// Held here and NEVER on [`StoredHsmConfiguration::configuration`]: the
    /// `HsmConfiguration` shape models no certificate member, so it can never
    /// reach the wire, exactly as [`StoredCluster::master_user_password`] holds
    /// a cluster's admin password.
    pub hsm_server_public_certificate: String,
    /// The password that unlocks the HSM partition, held for the same reason and
    /// in the same place as [`StoredHsmConfiguration::hsm_server_public_certificate`].
    pub hsm_partition_password: String,
}

/// A reserved node as this service stores it: the modelled projection a response
/// is built from, plus when the payment confirmation its purchase started falls
/// due.
#[derive(Debug, Clone)]
pub struct StoredReservedNode {
    /// Exactly what `DescribeReservedNodes` reports, so a read never recomputes
    /// it.
    pub node: ReservedNode,
    /// When the clock-owed [`RESERVED_NODE_STATE_PENDING_PAYMENT`] ->
    /// [`RESERVED_NODE_STATE_ACTIVE`] transition falls due. See
    /// [`RESERVED_NODE_PAYMENT_SPAN`].
    pub active_at: Instant,
}

/// A reserved-node exchange as this service stores it: the modelled projection
/// `DescribeReservedNodeExchangeStatus` reports, plus when the exchange the
/// accept started falls due.
#[derive(Debug, Clone)]
pub struct StoredReservedNodeExchange {
    /// Exactly what `DescribeReservedNodeExchangeStatus` reports, so a read
    /// never recomputes it.
    pub status: ReservedNodeExchangeStatus,
    /// When the clock-owed [`RESERVED_NODE_EXCHANGE_STATUS_REQUESTED`] ->
    /// [`RESERVED_NODE_EXCHANGE_STATUS_SUCCEEDED`] transition falls due. See
    /// [`RESERVED_NODE_EXCHANGE_SPAN`].
    pub completed_at: Instant,
}

/// Per-service state for com-amazonaws-redshift-redshiftserviceversion20121201. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    /// Every provisioned cluster, keyed by `ClusterIdentifier`, in creation order.
    pub clusters: Vec<StoredCluster>,
    /// Every cluster subnet group, keyed by `ClusterSubnetGroupName`, in creation
    /// order.
    ///
    /// The modelled shape IS the whole record — a subnet group carries no status
    /// the clock moves and nothing the `ClusterSubnetGroup` shape has no member
    /// for — so unlike the collections around it this one needs no `Stored*`
    /// wrapper.
    pub subnet_groups: Vec<ClusterSubnetGroup>,
    /// Every snapshot, keyed by `SnapshotIdentifier`, in creation order.
    pub snapshots: Vec<StoredSnapshot>,
    /// Every cluster parameter group, keyed by `ParameterGroupName`, in creation
    /// order — the default parameter groups first, then the ones the account
    /// created.
    pub parameter_groups: Vec<StoredParameterGroup>,
    /// Every usage limit, keyed by `UsageLimitId`, in creation order.
    ///
    /// The modelled `UsageLimit` shape (generated under the name
    /// [`CreateUsageLimitOutput`], because `CreateUsageLimit` returns it
    /// unwrapped) IS the whole record — a usage limit carries no status the clock
    /// moves and nothing the shape has no member for — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::subnet_groups`],
    /// this collection needs no `Stored*` wrapper.
    pub usage_limits: Vec<CreateUsageLimitOutput>,
    /// Every HSM configuration, keyed by `HsmConfigurationIdentifier`, in
    /// creation order.
    pub hsm_configurations: Vec<StoredHsmConfiguration>,
    /// Every snapshot copy grant, keyed by `SnapshotCopyGrantName`, in creation
    /// order.
    ///
    /// The modelled `SnapshotCopyGrant` shape IS the whole record — a grant
    /// carries no status the clock moves and nothing the shape has no member
    /// for — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`],
    /// this collection needs no `Stored*` wrapper.
    pub snapshot_copy_grants: Vec<SnapshotCopyGrant>,
    /// Every snapshot schedule, keyed by `ScheduleIdentifier`, in creation
    /// order.
    ///
    /// Unlike the collections around it this one holds a `Stored*` wrapper even
    /// though a schedule carries no status the clock moves: three members of the
    /// modelled `SnapshotSchedule` shape are DERIVED when the schedule is
    /// reported rather than stored, so the record holds only what a create gave
    /// it (see [`StoredSnapshotSchedule`]).
    pub snapshot_schedules: Vec<StoredSnapshotSchedule>,
    /// Every scheduled action, keyed by `ScheduledActionName`, in creation
    /// order.
    ///
    /// Like [`StoredSnapshotSchedule`] this collection holds a `Stored*` wrapper
    /// even though a scheduled action carries no status the clock moves: the
    /// modelled `ScheduledAction` shape's `$NextInvocations` is DERIVED when the
    /// action is reported rather than stored, so the record holds only what a
    /// create or a modify gave it (see [`StoredScheduledAction`]).
    pub scheduled_actions: Vec<StoredScheduledAction>,
    /// Every Query Editor (QEV2) IAM Identity Center application the account
    /// registered, keyed by `Qev2IdcApplicationArn`, in creation order.
    ///
    /// The modelled `Qev2IdcApplication` shape IS the whole record — the
    /// application carries no status the clock moves (see
    /// [`QEV2_IDC_ONBOARD_STATUS`]) and nothing the shape has no member for —
    /// so, like [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::usage_limits`],
    /// this collection needs no `Stored*` wrapper.
    pub qev2_idc_applications: Vec<Qev2IdcApplication>,
    /// Every Redshift IAM Identity Center application the account created, keyed
    /// by `RedshiftIdcApplicationArn`, in creation order.
    ///
    /// The modelled `RedshiftIdcApplication` shape IS the whole record — the
    /// application carries no status the clock moves (see
    /// [`REDSHIFT_IDC_ONBOARD_STATUS`]) and nothing the shape has no member
    /// for — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::qev2_idc_applications`],
    /// this collection needs no `Stored*` wrapper.
    pub redshift_idc_applications: Vec<RedshiftIdcApplication>,
    /// The engine versions this service offers, in catalog order. Unlike the
    /// collections above this one is SEEDED when the store is created — the
    /// versions exist before any cluster does, which is why
    /// `DescribeClusterVersions` can be called "even before creating any
    /// clusters" — and no operation adds to or removes from it.
    pub cluster_versions: Vec<ClusterVersion>,
    /// The maintenance tracks this service publishes, in catalog order. SEEDED
    /// when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]:
    /// the tracks a cluster can be assigned to exist before any cluster does,
    /// and no operation adds to or removes from the list.
    pub maintenance_tracks: Vec<MaintenanceTrack>,
    /// The account attributes this service publishes, in catalog order. SEEDED
    /// when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::maintenance_tracks`]:
    /// an attribute is an allowance the account already has before it creates
    /// anything, and no operation adds to or removes from the list.
    pub account_attributes: Vec<AccountAttribute>,
    /// The reserved node offerings this service publishes, in catalog order.
    /// SEEDED when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::account_attributes`]:
    /// an offering is purchasable before the account reserves anything, and no
    /// operation adds to or removes from the list (see
    /// [`published_reserved_node_offerings`]).
    pub reserved_node_offerings: Vec<ReservedNodeOffering>,
    /// The exchange-target reserved node offerings this service publishes, in
    /// catalog order. SEEDED when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::reserved_node_offerings`],
    /// and held apart from them because an exchange target is not a product a
    /// purchase buys: it is the offering an existing reservation may be moved
    /// onto (see [`published_exchange_reserved_node_offerings`]).
    pub exchange_reserved_node_offerings: Vec<ReservedNodeOffering>,
    /// Every reservation `PurchaseReservedNodeOffering` bought, keyed by
    /// `ReservedNodeId`, in purchase order — unlike the offering catalog above,
    /// which is seeded and never grows, this collection starts empty and a
    /// purchase is the only thing that adds to it.
    pub reserved_nodes: Vec<StoredReservedNode>,
    /// Every reserved-node exchange `AcceptReservedNodeExchange` asked for,
    /// keyed by `ReservedNodeExchangeRequestId`, in request order — like the
    /// reservations above, this collection starts empty and an accept is the
    /// only thing that adds to it.
    pub reserved_node_exchanges: Vec<StoredReservedNodeExchange>,
    /// Every namespace registration `RegisterNamespace` recorded, keyed by the
    /// (namespace, consumer account) pair it is a registration of, in
    /// registration order.
    pub namespace_registrations: Vec<StoredNamespaceRegistration>,
    /// Every event notification subscription, keyed by its name
    /// (`CustSubscriptionId`), in creation order.
    ///
    /// The modelled `EventSubscription` shape IS the whole record — a
    /// subscription's `Status` moves only when the Amazon SNS topic it names
    /// loses its permission or is deleted, neither of which is a transition the
    /// clock owes — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_copy_grants`],
    /// this collection needs no `Stored*` wrapper.
    pub event_subscriptions: Vec<EventSubscription>,
    /// The event catalog this service publishes, one entry per published source
    /// type, in catalog order. SEEDED when the store is created, exactly like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::cluster_versions`]:
    /// the events a subscription can be scoped to exist before anything raises
    /// one, and no operation adds to or removes from the list (see
    /// [`published_event_catalog`]).
    pub event_catalog: Vec<EventCategoriesMap>,
    /// Every event a completed control-plane write RAISED, in the order it
    /// happened — which is chronological, because an event is appended at the
    /// instant it is raised.
    ///
    /// Unlike
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::event_catalog`],
    /// which is the published description of the events this service CAN raise,
    /// this is the log of the ones that actually happened: each entry is one
    /// occurrence, carrying the source it happened to, the instant it happened
    /// at and the message that occurrence rendered. The log is what an event read
    /// reports, and an entry leaves it once [`EVENT_RETENTION`] has passed (see
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`]).
    pub events: Vec<Event>,
    /// Every authentication profile, keyed by `AuthenticationProfileName`, in
    /// creation order.
    ///
    /// The modelled `AuthenticationProfile` shape IS the whole record — a
    /// profile is its name and the content a write gave it, and carries no
    /// status the clock moves — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::event_subscriptions`],
    /// this collection needs no `Stored*` wrapper.
    pub authentication_profiles: Vec<AuthenticationProfile>,
    /// The resource policy attached to each resource that carries one, keyed by
    /// the ARN of the resource it is attached to.
    ///
    /// A resource carries at most ONE policy — `PutResourcePolicy` "updates the
    /// resource policy for a specified resource", so a second write to the same
    /// resource REPLACES the document it held rather than adding to it — and the
    /// document is held VERBATIM, because what a read reports is "the content of
    /// a resource policy": the characters the write sent, not a re-rendering of
    /// the document they parse as.
    pub resource_policies: BTreeMap<String, String>,
    /// Every partner integration, keyed by the (cluster, database, partner)
    /// triple it is an integration of, in the order the partners were added.
    pub partner_integrations: Vec<StoredPartnerIntegration>,
    /// Every HSM client certificate, keyed by
    /// `HsmClientCertificateIdentifier`, in creation order.
    ///
    /// The modelled `HsmClientCertificate` shape IS the whole record — the
    /// certificate is its identifier, the public key the service minted for it
    /// and its tags, and it carries no status the clock moves — so, like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_copy_grants`],
    /// this collection needs no `Stored*` wrapper.
    pub hsm_client_certificates: Vec<HsmClientCertificate>,
    /// Every zero-ETL integration, keyed by `IntegrationArn`, in creation order.
    pub integrations: Vec<StoredIntegration>,
    /// Every Redshift-managed VPC endpoint, keyed by `EndpointName`, in creation
    /// order.
    pub endpoint_accesses: Vec<StoredEndpointAccess>,
    /// Every endpoint authorization, keyed by the cluster and the grantee account
    /// it was granted for, in creation order.
    pub endpoint_authorizations: Vec<StoredEndpointAuthorization>,
    /// Every Amazon Redshift Advisor recommendation recorded against a cluster of
    /// this account, keyed by `Id`, in the order they were recorded.
    ///
    /// Unlike the published catalogs above, this collection is neither seeded nor
    /// grown by any operation: an Advisor finding comes from ANALYSING a
    /// cluster's workload — its query and table history — which this control
    /// plane holds none of, and the service publishes no API that records one. So
    /// a cluster no analysis has observed carries no recommendation, and a read
    /// reports for it the empty page the real service reports.
    ///
    /// The modelled `Recommendation` shape IS the whole record — a recommendation
    /// is an observation already made, and carries no status the clock moves — so,
    /// like
    /// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::snapshot_copy_grants`],
    /// this collection needs no `Stored*` wrapper.
    pub recommendations: Vec<Recommendation>,
    /// Every table restore `RestoreTableFromClusterSnapshot` asked for, keyed by
    /// `TableRestoreRequestId`, in request order (see [`StoredTableRestore`]).
    pub table_restores: Vec<StoredTableRestore>,
    /// Every datashare of this account, keyed by `DataShareArn`, in the order the
    /// store holds them.
    ///
    /// Neither seeded nor grown by any operation of this service: a datashare is
    /// created on the producer's DATABASE and Amazon Redshift publishes no API
    /// that creates one (see [`StoredDataShare`]), so an account that has created
    /// none holds none and every operation that acts on one answers
    /// `InvalidDataShareFault` for an ARN that resolves to nothing.
    pub data_shares: Vec<StoredDataShare>,
    /// Where the health series of each cluster has got to, and the one in-process
    /// clock that advances them: the schedule the AWS/Redshift `HealthStatus`
    /// metric of every cluster this store holds is published on (see
    /// [`cluster_metrics`]).
    ///
    /// Held here rather than per cluster record because the clock is the STORE's:
    /// it is claimed once however many writes drive it, and it publishes for
    /// every cluster the store holds.
    pub health_metrics: cluster_metrics::HealthStatusPublisher,
}

/// Where in the store one resource an ARN names sits: its collection and its
/// position in it.
///
/// One resolution serves both the read and the write path (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::resource_slot`]),
/// so a tag read and a tag write can never resolve the same ARN differently.
#[derive(Debug, Clone, Copy)]
enum ResourceSlot {
    Cluster(usize),
    Snapshot(usize),
    ParameterGroup(usize),
    SubnetGroup(usize),
    HsmConfiguration(usize),
}

/// A fresh store already holds what exists before any caller writes anything:
/// the published engine-version catalog and, for each of its versions, the
/// DEFAULT parameter group of that version's family.
///
/// Source: `smithy_query(query="doc", operation="DescribeClusterParameterGroups")`
/// — "Returns a list of Amazon Redshift parameter groups, including parameter
/// groups you created and the default parameter group": the default group exists
/// without anybody creating it, so it is part of an empty store rather than
/// something a read mints.
impl Default for ComAmazonawsRedshiftRedshiftserviceversion20121201State {
    fn default() -> Self {
        Self {
            clusters: Vec::new(),
            subnet_groups: Vec::new(),
            snapshots: Vec::new(),
            parameter_groups: default_parameter_groups(),
            usage_limits: Vec::new(),
            hsm_configurations: Vec::new(),
            snapshot_copy_grants: Vec::new(),
            snapshot_schedules: Vec::new(),
            scheduled_actions: Vec::new(),
            qev2_idc_applications: Vec::new(),
            redshift_idc_applications: Vec::new(),
            cluster_versions: published_cluster_versions(),
            maintenance_tracks: published_maintenance_tracks(),
            account_attributes: published_account_attributes(),
            reserved_node_offerings: published_reserved_node_offerings(),
            exchange_reserved_node_offerings: published_exchange_reserved_node_offerings(),
            reserved_nodes: Vec::new(),
            reserved_node_exchanges: Vec::new(),
            namespace_registrations: Vec::new(),
            event_subscriptions: Vec::new(),
            event_catalog: published_event_catalog(),
            events: Vec::new(),
            authentication_profiles: Vec::new(),
            resource_policies: BTreeMap::new(),
            partner_integrations: Vec::new(),
            hsm_client_certificates: Vec::new(),
            integrations: Vec::new(),
            endpoint_accesses: Vec::new(),
            endpoint_authorizations: Vec::new(),
            recommendations: Vec::new(),
            table_restores: Vec::new(),
            data_shares: Vec::new(),
            health_metrics: cluster_metrics::HealthStatusPublisher::default(),
        }
    }
}

/// One `TaggedResource` per tag of one resource: the tag, the resource's ARN and
/// the resource's type, which is what `DescribeTags` reports a tag as.
fn tagged_resource_entries<'a>(
    resource_type: &'static str,
    arn: String,
    tags: &'a [crate::ops::types::Tag],
) -> impl Iterator<Item = crate::ops::types::TaggedResource> + 'a {
    tags.iter()
        .map(move |tag| crate::ops::types::TaggedResource {
            tag: Some(tag.clone()),
            resource_name: Some(arn.clone()),
            resource_type: Some(resource_type.to_string()),
        })
}

/// The `ClusterAvailabilityStatus` of a cluster that can serve queries: "Available - The
/// cluster is available for queries"
/// (`smithy_query(query="doc", shape="Cluster", member="ClusterAvailabilityStatus")`).
///
/// Named here because it is what the health the service publishes about a cluster is read off
/// ([`cluster_metrics`]) as well as what [`availability_status`] reports, so the two cannot
/// disagree about which clusters are serving.
pub const AVAILABILITY_STATUS_AVAILABLE: &str = "Available";

/// `ClusterAvailabilityStatus` for a `ClusterStatus`.
///
/// The two vocabularies both come from the model.
/// `smithy_query(query="doc", shape="Cluster", member="ClusterAvailabilityStatus")`
/// declares exactly five values: "Available - The cluster is available for
/// queries", "Unavailable - The cluster is not available for queries",
/// "Maintenance - … intermittently available … due to maintenance activities",
/// "Modifying - … intermittently available for queries due to changes that
/// modify the cluster" and "Failed - The cluster failed and is not available for
/// queries". `smithy_query(query="doc", shape="Cluster", member="ClusterStatus")`
/// declares the twenty `ClusterStatus` values this maps from, so the mapping is a
/// rule over the whole published vocabulary rather than a case per status:
///
/// * a status that STARTS with `available` ("available", "available,
///   prep-for-resize", "available, resize-cleanup") is `Available`;
/// * a status in which the service is changing the cluster — bringing it up,
///   modifying, resizing, cancelling a resize, rebooting, renaming, rotating its
///   keys, updating its HSM settings or taking its final snapshot — is
///   `Modifying`: the cluster is only intermittently available while it happens;
/// * a status the cluster cannot serve queries from because it broke
///   (`hardware-failure`, every `incompatible-…`) is `Failed`;
/// * every other published status (`deleting`, `paused`, `storage-full`) is
///   `Unavailable`.
///
/// No `ClusterStatus` reports scheduled maintenance, so no status maps to
/// `Maintenance`.
pub fn availability_status(cluster_status: &str) -> &'static str {
    if cluster_status.starts_with(STATUS_AVAILABLE) {
        return AVAILABILITY_STATUS_AVAILABLE;
    }
    match cluster_status {
        STATUS_CREATING | STATUS_MODIFYING | "resizing" | "cancelling-resize" | "rebooting"
        | "renaming" | "rotating-keys" | "updating-hsm" | "final-snapshot" => "Modifying",
        "hardware-failure" => "Failed",
        other if other.starts_with("incompatible-") => "Failed",
        _ => "Unavailable",
    }
}

/// `Cluster$AvailabilityZoneRelocationStatus` for the `AvailabilityZoneRelocation`
/// a request carried (or did not).
///
/// Source: `smithy_query(query="doc", shape="Cluster",
/// member="AvailabilityZoneRelocationStatus")` — "Describes the status of the
/// Availability Zone relocation operation" — paired with
/// `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="AvailabilityZoneRelocation")`, "The option to enable relocation for an
/// Amazon Redshift cluster between Availability Zones after the cluster is
/// created": the member reports whether that option is on, so it is `enabled`
/// exactly when the request asked for relocation and `disabled` exactly when it
/// turned it off.
///
/// The model documents NO default for the option. Amazon Redshift reports
/// relocation `enabled` on a provisioned cluster whose request named the option
/// at all — see the recorded `DescribeClusters` response this emulator is
/// measured against — so [`RELOCATION_ENABLED_BY_DEFAULT`] is what a silent
/// request gets; a request that names the option is always honoured.
pub fn availability_zone_relocation_status(requested: Option<bool>) -> String {
    if requested.unwrap_or(RELOCATION_ENABLED_BY_DEFAULT) {
        "enabled"
    } else {
        "disabled"
    }
    .to_string()
}

/// Whether Availability Zone relocation is on for a cluster whose request did not
/// name the option. See [`availability_zone_relocation_status`].
pub const RELOCATION_ENABLED_BY_DEFAULT: bool = true;

/// What an Identity Center auth token request from a caller whose credentials
/// carry NO embedded IAM Identity Center identity is refused with.
///
/// Why such a request is refused at all: `smithy_query(query="doc",
/// operation="GetIdentityCenterAuthToken")` — "This API is exclusively for use
/// with Amazon Web Services IAM Identity Center enhanced credentials. If the
/// caller is not using enhanced credentials with embedded Amazon Web Services IAM
/// Identity Center identity, the API will return an error."
///
/// The SENTENCE is the one Amazon Redshift answers such a call with, as recorded
/// against AWS. It is fixed text end to end: the refusal is about the credential
/// the request was signed with, so nothing in it is interpolated from the
/// request.
pub const IDENTITY_CENTER_ENHANCED_CREDENTIALS_REQUIRED: &str =
    "Request rejected: Not an IDC user acting";

/// The opaque `Marker` cursor the paginated `Describe*` operations hand out for
/// the record at `offset` in the stable insertion order.
///
/// The offset is paired with a tag derived from it, so a cursor this service did
/// not produce is refused rather than served as a mid-page offset: the
/// representation is this module's business alone and a caller can only echo
/// back one it was given.
fn marker_for(offset: usize) -> String {
    format!("{offset}.{}", short_digest(&format!("marker/{offset}"), 8))
}

/// The offset a `Marker` names, or `None` when the cursor is not one
/// [`marker_for`] produced.
fn marker_offset(cursor: &str) -> Option<usize> {
    let (offset, tag) = cursor.rsplit_once('.')?;
    let offset: usize = offset.parse().ok()?;
    (tag == short_digest(&format!("marker/{offset}"), 8)).then_some(offset)
}

/// The `MaxRecords` window one paginated operation accepts: the page size it
/// uses when the request carries none, the smallest page a request may ask for,
/// and the largest page the published window names.
///
/// A window rather than three bare constants because the operations do NOT all
/// share one: most Redshift `Describe*` operations publish the 20..100 window of
/// [`MAX_RECORDS_WINDOW`], while `DescribeTags` publishes its own
/// ([`TAG_MAX_RECORDS_WINDOW`]).
///
/// Only [`MaxRecordsWindow::min`] is a refusal; [`MaxRecordsWindow::max`] caps
/// the page served. See [`page_bounds_within`].
#[derive(Debug, Clone, Copy)]
pub struct MaxRecordsWindow {
    /// The page size a request that names no `MaxRecords` gets.
    pub default: i32,
    /// The smallest `MaxRecords` a request may name.
    pub min: i32,
    /// The largest page the published window names, and the most records one
    /// page carries: the page a request that names no size is served, and the
    /// page a request naming MORE is clamped to (see [`page_bounds_within`]) —
    /// a value past it is not refused.
    pub max: i32,
}

/// The window [`page_bounds`] applies: the one every paginated Redshift
/// `Describe*` operation but `DescribeTags` publishes. See
/// [`MAX_RECORDS_DEFAULT`].
pub const MAX_RECORDS_WINDOW: MaxRecordsWindow = MaxRecordsWindow {
    default: MAX_RECORDS_DEFAULT,
    min: MAX_RECORDS_MIN,
    max: MAX_RECORDS_MAX,
};

/// `DescribeTags`'s own `MaxRecords` window.
///
/// Source: `smithy_query(query="doc", operation="DescribeTags")` — "The
/// MaxRecords parameter can be a range from 10 to 50 results to return in a
/// request." The sentence states no default, so the page size a request that
/// names no `MaxRecords` gets is this window's own MAXIMUM: the most the
/// operation publishes is also the most it serves unasked. Only the minimum is
/// a refusal, as on every other window, and the maximum caps the page served
/// ([`page_bounds_within`]).
pub const TAG_MAX_RECORDS_WINDOW: MaxRecordsWindow = MaxRecordsWindow {
    default: 50,
    min: 10,
    max: 50,
};

/// Resolve one page of `total` records from the request's `Marker` / `MaxRecords`,
/// under the `MaxRecords` window of the operation serving it.
///
/// Only the window's MINIMUM is a refusal: a `MaxRecords` below it is refused,
/// while one above the window's published maximum is SERVED — the published
/// window is the page size a paginating client is told to stay inside, not a
/// bound the service turns a larger request away at, so refusing a bigger page
/// would refuse a call the service serves.
///
/// The page it is served is the window's maximum: that value is the most records
/// one page carries, so a request for more is answered with a full page and a
/// `Marker` for the rest. Clamping rather than honouring the number keeps the
/// records one request materialises proportional to the window instead of to the
/// whole collection, which is what each caller's page is copied and serialised
/// from.
///
/// Returns the half-open range of the page and the `Marker` to hand back, which
/// is `Some` only when records remain after it.
pub fn page_bounds_within(
    window: MaxRecordsWindow,
    marker: Option<&str>,
    max_records: Option<i32>,
    total: usize,
) -> Result<(usize, usize, Option<String>), String> {
    let page_size = match max_records {
        None => window.default,
        Some(value) if value >= window.min => value.min(window.max),
        Some(value) => {
            return Err(format!(
                "Invalid value for MaxRecords: {value}. MaxRecords must be at least {}.",
                window.min
            ))
        }
    };
    let start = match marker {
        None => 0usize,
        Some(cursor) => match marker_offset(cursor) {
            Some(offset) if offset <= total => offset,
            _ => {
                return Err(format!(
                    "Invalid value for Marker: '{cursor}'. The marker must be one returned by a previous call."
                ))
            }
        },
    };
    let end = (start + page_size as usize).min(total);
    let next = (end < total).then(|| marker_for(end));
    Ok((start, end, next))
}

/// Resolve one page of `total` records under the window every paginated Redshift
/// `Describe*` operation but `DescribeTags` publishes ([`MAX_RECORDS_WINDOW`]).
pub fn page_bounds(
    marker: Option<&str>,
    max_records: Option<i32>,
    total: usize,
) -> Result<(usize, usize, Option<String>), String> {
    page_bounds_within(MAX_RECORDS_WINDOW, marker, max_records, total)
}

/// What a `DeleteCluster` that carries NEITHER half of the final-snapshot
/// decision is refused with: `SkipFinalClusterSnapshot` defaults to false, and a
/// delete that is not skipping the final snapshot has to name the snapshot to
/// take.
///
/// Fixed text with no variable part: the sentence is about the two members, not
/// about the cluster the request named.
pub const FINAL_SNAPSHOT_DECISION_REQUIRED: &str =
    "FinalClusterSnapshotIdentifier is required unless SkipFinalClusterSnapshot is specified.";

/// The fewest nodes a `multi-node` cluster may have.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterType")` — "multi-node, the NumberOfNodes parameter is
/// required" — with `NumberOfNodes`'s own "If you don't specify this parameter,
/// you get a single-node cluster": a cluster of more than one node is what the
/// multi-node shape IS, so two is its floor.
pub const MIN_MULTI_NODE_NODES: i32 = 2;

/// What a `NumberOfNodes` other than one asked for a `single-node` cluster is
/// refused with. The shape admits exactly one node, so no value of the request
/// names a count beside it.
pub const SINGLE_NODE_COUNT_REFUSAL: &str =
    "Number of nodes must be 1 or not be supplied for cluster type single-node.";

/// What a `NumberOfNodes` below [`MIN_MULTI_NODE_NODES`] asked for a
/// `multi-node` cluster is refused with — including a request that names no
/// count at all, which asks for the one-node shape.
pub const MULTI_NODE_COUNT_REFUSAL: &str =
    "Number of nodes for cluster type multi-node must be greater than or equal to 2.";

/// The days a weekly maintenance window may name, in week order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="PreferredMaintenanceWindow")` — "Valid Days: Mon | Tue | Wed | Thu |
/// Fri | Sat | Sun" — matched case-insensitively, which is how Redshift reports
/// a window back (`wed:04:00-wed:04:30`).
pub const MAINTENANCE_WINDOW_DAYS: [&str; 7] =
    ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// How long a weekly maintenance window must span, at the least.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="PreferredMaintenanceWindow")` — "Constraints: Minimum 30-minute
/// window."
pub const MIN_MAINTENANCE_WINDOW_MINUTES: i64 = 30;

/// The sentence Amazon Redshift refuses a window shorter than
/// [`MIN_MAINTENANCE_WINDOW_MINUTES`] with, including the unspaced
/// "30minutes" the service itself writes it as.
///
/// The number is the bound, so it is formatted from the constant above rather
/// than spelled out; the surrounding words are the service's own and are
/// reproduced as it words them, which is why nothing here inserts the space a
/// reader expects.
fn short_maintenance_window_message() -> String {
    format!("Maintenance window must be at least {MIN_MAINTENANCE_WINDOW_MINUTES}minutes.")
}

/// Minutes from the start of the week of one `ddd:hh24:mi` endpoint of a
/// maintenance window, or `None` when the text is not in that form.
///
/// Each clock field is exactly two ASCII digits, which is what `hh24` and `mi`
/// are: a signed or differently sized field (`+4`, `-1`, `4`, `004`) is not the
/// documented form and is no window at all.
fn maintenance_window_instant(endpoint: &str) -> Option<i64> {
    /// One two-digit clock field, or `None` for anything else.
    fn clock_field(field: &str) -> Option<i64> {
        if field.len() != 2 || !field.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        field.parse().ok()
    }

    let mut parts = endpoint.split(':');
    let day = parts.next()?.to_ascii_lowercase();
    let hour = clock_field(parts.next()?)?;
    let minute = clock_field(parts.next()?)?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let day = MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|candidate| *candidate == day)? as i64;
    Some(day * 24 * 60 + hour * 60 + minute)
}

/// Refuse a `PreferredMaintenanceWindow` outside the documented form, or one
/// that spans less than [`MIN_MAINTENANCE_WINDOW_MINUTES`].
///
/// One rule for every operation that takes a maintenance window, so a window a
/// create would refuse cannot be set by a modify or a restore. A window whose
/// end precedes its start wraps around the week (`sun:23:45-mon:00:30`), so its
/// span is measured modulo one week — which is also why a window whose two
/// endpoints are equal spans nothing and is refused.
pub fn check_maintenance_window(window: &str) -> Result<(), String> {
    const MINUTES_PER_WEEK: i64 = 7 * 24 * 60;
    let malformed = || {
        Err(format!(
            "Invalid value for PreferredMaintenanceWindow: '{window}'. The window must be in the format ddd:hh24:mi-ddd:hh24:mi, with each day one of {}.",
            MAINTENANCE_WINDOW_DAYS.join(" | ")
        ))
    };
    let Some((start, end)) = window.split_once('-') else {
        return malformed();
    };
    let (Some(start), Some(end)) = (
        maintenance_window_instant(start),
        maintenance_window_instant(end),
    ) else {
        return malformed();
    };
    if (end - start).rem_euclid(MINUTES_PER_WEEK) < MIN_MAINTENANCE_WINDOW_MINUTES {
        return Err(short_maintenance_window_message());
    }
    Ok(())
}

/// The ARN resource-type segment of a provisioned cluster.
pub const ARN_SEGMENT_CLUSTER: &str = "cluster";
/// The ARN resource-type segment of a cluster snapshot. Its resource id is
/// `<cluster>/<snapshot>` (see [`snapshot_arn`]).
pub const ARN_SEGMENT_SNAPSHOT: &str = "snapshot";
/// The ARN resource-type segment of a cluster parameter group.
pub const ARN_SEGMENT_PARAMETER_GROUP: &str = "parametergroup";
/// The ARN resource-type segment of a cluster subnet group.
pub const ARN_SEGMENT_SUBNET_GROUP: &str = "subnetgroup";
/// The ARN resource-type segment of an HSM configuration (the "HSM connection"
/// the API publishes).
pub const ARN_SEGMENT_HSM_CONFIGURATION: &str = "hsmconfiguration";

/// One instant as the ISO-8601 text a STRING-typed timestamp member carries
/// (`CreateCustomDomainAssociationResult$CustomDomainCertExpiryTime` is modelled
/// as a string, not a timestamp).
pub fn iso8601(instant: ::aws_smithy_types::DateTime) -> String {
    instant
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// How long the ACM certificate a custom domain association names is valid for
/// after the association is created.
///
/// `Cluster$CustomDomainCertificateExpiryDate` and
/// `CreateCustomDomainAssociationResult$CustomDomainCertExpiryTime` are the
/// expiry of the certificate the request named, which is a resource of AWS
/// Certificate Manager rather than of this service — so the emulator derives it
/// from the association's own creation instant and this span, the validity
/// period ACM issues a public certificate for (13 months, 395 days). Derived
/// rather than drawn, so every read of one association reports the same expiry.
pub const CUSTOM_DOMAIN_CERTIFICATE_VALIDITY: Duration =
    Duration::from_secs(395 * 24 * 60 * 60);

/// What every operation on a custom domain association answers when the name it
/// resolves names no association at all: the sentence Amazon Redshift words
/// `CustomDomainAssociationNotFoundFault` with, which states that nothing is
/// linked to the name the request supplied rather than naming a resource type.
///
/// The fixed prose is the requirement and the `CustomDomainName` is taken from
/// the request, so one condition reads the same way through every operation
/// that resolves an association by its domain name.
pub fn custom_domain_association_not_found_message(custom_domain_name: &str) -> String {
    format!(
        "There aren't any associations linked to the custom domain name {custom_domain_name}."
    )
}

/// The same refusal for a request that names the association it wants through
/// the CERTIFICATE instead of the domain name
/// (`DescribeCustomDomainAssociations$CustomDomainCertificateArn`): the same
/// sentence about the member the value arrived on, so no refusal interpolates a
/// certificate ARN into a sentence about a domain name.
pub fn custom_domain_certificate_association_not_found_message(
    custom_domain_certificate_arn: &str,
) -> String {
    format!(
        "There aren't any associations linked to the custom domain certificate \
         {custom_domain_certificate_arn}."
    )
}

/// What an operation on the custom domain association OF a cluster answers when
/// that cluster carries none.
pub fn cluster_custom_domain_not_found_message(cluster_identifier: &str) -> String {
    format!(
        "CustomDomainAssociation not found for cluster: {cluster_identifier}"
    )
}

/// How many partner integrations the store holds before `AddPartner` refuses.
///
/// The emulator's own bound on a collection keyed by names, of which there are
/// unboundedly many — the same reasoning as [`QEV2_IDC_APPLICATION_QUOTA`].
pub const PARTNER_INTEGRATION_QUOTA: usize = 10_000;

/// The `PartnerIntegrationStatus` a partner integration starts in.
///
/// The model publishes the value list (`smithy_query(query="shape",
/// shape="PartnerIntegrationStatus")` — `Active | Inactive | RuntimeFailure |
/// ConnectionFailure`) but documents no initial value. `UpdatePartnerStatus` is
/// documented as the call "a partner" makes to "update the status of a partner
/// integration", so an integration the ACCOUNT has just added, and that no
/// partner has reported on yet, is not active: it starts `Inactive` and the
/// partner's own update is what moves it.
pub const PARTNER_INTEGRATION_INITIAL_STATUS: &str = "Inactive";

/// Every `PartnerIntegrationStatus` the model publishes, in model order.
pub const PARTNER_INTEGRATION_STATUSES: [&str; 4] =
    ["Active", "Inactive", "RuntimeFailure", "ConnectionFailure"];

/// A partner integration as this service stores it: the modelled projection
/// `DescribePartners` reports, plus the cluster and account it belongs to, which
/// the `PartnerIntegrationInfo` shape has no member for.
#[derive(Debug, Clone)]
pub struct StoredPartnerIntegration {
    /// The cluster whose data the partner reaches.
    pub cluster_identifier: String,
    /// The account that owns that cluster, as the request named it.
    pub account_id: String,
    /// Exactly what `DescribePartners` reports.
    pub info: PartnerIntegrationInfo,
}

/// What every operation on a partner integration answers when the pair it
/// resolves names none.
pub fn partner_not_found_message(partner_name: &str, database_name: &str) -> String {
    format!(
        "Partner integration not found: partner {partner_name} on database {database_name}"
    )
}

/// How many HSM client certificates the store holds before
/// `CreateHsmClientCertificate` refuses with the modelled
/// `HsmClientCertificateQuotaExceededFault`.
///
/// The same reasoning as [`HSM_CONFIGURATION_QUOTA`]: the emulator's own bound
/// on a collection keyed by an identifier, and the modelled fault is the
/// service's own way of saying the bound was reached.
pub const HSM_CLIENT_CERTIFICATE_QUOTA: usize = 10_000;

/// The public key of an HSM client certificate, in the PEM text Redshift
/// reports.
///
/// Source: `smithy_query(query="doc", shape="HsmClientCertificate",
/// member="HsmClientCertificatePublicKey")` — "The public key that the Amazon
/// Redshift cluster will use to connect to the HSM. You must register the public
/// key in the HSM." The key is MINTED by the service — `CreateHsmClientCertificate`
/// takes no key material, only the identifier to mint one under — so here it is
/// DERIVED from that identifier: the same certificate always reports the same
/// key, so the blob a create answered with is the blob every later read of it
/// carries, and nothing is drawn per run.
pub fn hsm_client_certificate_public_key(identifier: &str) -> String {
    let body = short_digest(&format!("hsm-client-certificate/{identifier}"), 56);
    format!("-----BEGIN CERTIFICATE-----\n{body}\n-----END CERTIFICATE-----\n")
}

/// Which sentence an `HsmClientCertificateNotFoundFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`], and the same one the sibling
/// HSM configuration family answers with: the fault's status (400) and awsQuery
/// code (`HsmClientCertificateNotFoundFault`) are what the model fixes, while
/// the fixed text around the `HsmClientCertificateIdentifier` is an attribute of
/// the OPERATION. The resource is named as a reader would write it ("HSM client
/// certificate"), not by its shape name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HsmClientCertificateNotFoundWording {
    /// `HSM client certificate not found: <HsmClientCertificateIdentifier>` —
    /// what the operation whose TARGET is the certificate itself,
    /// `DeleteHsmClientCertificate`, answers.
    NotFoundThenIdentifier,
    /// `HSM client certificate '<HsmClientCertificateIdentifier>' not found.` —
    /// what the READ, `DescribeHsmClientCertificates`, answers when its
    /// identifier names no certificate: embedded, single-quoted and ending in a
    /// period.
    IdentifierThenNotFound,
}

/// The `HsmClientCertificateNotFoundFault` message `wording` words, naming the
/// identifier the request carried.
pub fn hsm_client_certificate_not_found_message(
    wording: HsmClientCertificateNotFoundWording,
    identifier: &str,
) -> String {
    match wording {
        HsmClientCertificateNotFoundWording::NotFoundThenIdentifier => {
            format!("HSM client certificate not found: {identifier}")
        }
        HsmClientCertificateNotFoundWording::IdentifierThenNotFound => {
            format!("HSM client certificate '{identifier}' not found.")
        }
    }
}

/// How many zero-ETL integrations the store holds before `CreateIntegration`
/// refuses with the modelled `IntegrationQuotaExceededFault`.
pub const INTEGRATION_QUOTA: usize = 10_000;

/// The `ZeroETLIntegrationStatus` an integration a create just recorded is in,
/// until [`PROVISIONING_SPAN`] has passed.
pub const INTEGRATION_STATUS_CREATING: &str = "creating";

/// The `ZeroETLIntegrationStatus` of an integration that is replicating.
pub const INTEGRATION_STATUS_ACTIVE: &str = "active";

/// The `ZeroETLIntegrationStatus` of an integration a modify is being applied
/// to, until [`PROVISIONING_SPAN`] has passed.
pub const INTEGRATION_STATUS_MODIFYING: &str = "modifying";

/// The `ZeroETLIntegrationStatus` of an integration whose deletion is under way.
pub const INTEGRATION_STATUS_DELETING: &str = "deleting";

/// The ARN of a zero-ETL integration:
/// `arn:aws:redshift:<region>:<account>:integration:<uuid>`.
///
/// Derived from the integration's own name and Region, so the value is stable
/// across every read of the same integration rather than per-run, exactly as
/// [`qev2_idc_application_arn`] is; it is computed ONCE, by the create, and then
/// persisted on the stored integration.
pub fn integration_arn(integration_name: &str, region: &str) -> String {
    let uuid = content_uuid(&format!("integration/{integration_name}/{region}"));
    engine::emulator_arn("redshift", region, &format!("integration:{uuid}"))
}

/// What every operation on an integration answers when the ARN it resolves
/// names none.
pub fn integration_not_found_message(integration_arn: &str) -> String {
    format!("Integration not found: {integration_arn}")
}

/// How Amazon Redshift states that the (source, target) pair a create names is
/// already replicating.
///
/// An integration replicates ONE source into ONE data warehouse, so the pair is
/// what an integration is unique on and what the sentence names. Fixed text bar
/// the two ARNs, which are the request's own `SourceArn` and `TargetArn`.
pub fn integration_pair_exists_message(source_arn: &str, target_arn: &str) -> String {
    format!(
        "An integration from source {source_arn} to target {target_arn} already exists. \
         Only one integration is allowed."
    )
}

/// What `CreateIntegration` answers when the data warehouse its `TargetArn`
/// names exists but is not active yet.
///
/// Entirely fixed text: the condition is about the target the request already
/// named, and the sentence tells the client what to do about it — wait, because
/// the condition is transient, which is what distinguishes this refusal from the
/// permanent "that target can't be found".
pub const INTEGRATION_TARGET_NOT_ACTIVE: &str = "Unable to perform create operation on the \
     specified zero-ETL integration because its target Redshift data warehouse is not active. \
     Wait until the data warehouse is active and try again.";

/// What `CreateIntegration` answers when its `TargetArn` names no data warehouse
/// at all — a permanent condition, naming the ARN the request carried.
pub fn integration_target_not_found_message(target_arn: &str) -> String {
    format!("Integration target not found: {target_arn}")
}

/// Why a `TargetArn` that names no namespace of either control plane is refused
/// by an operation that FILTERS on it rather than creating against it.
pub fn integration_namespace_not_found_message(target_arn: &str) -> String {
    format!("Namespace not found: {target_arn}")
}

/// A zero-ETL integration as this service stores it: the modelled projection
/// `DescribeIntegrations` reports, plus when the status transition the last call
/// against it started falls due.
#[derive(Debug, Clone)]
pub struct StoredIntegration {
    /// Exactly what `DescribeIntegrations` reports, so a read never recomputes
    /// it.
    pub integration: CreateIntegrationOutput,
    /// When the clock-owed [`INTEGRATION_STATUS_CREATING`] /
    /// [`INTEGRATION_STATUS_MODIFYING`] -> [`INTEGRATION_STATUS_ACTIVE`]
    /// transition falls due. See [`PROVISIONING_SPAN`].
    pub active_at: Instant,
}

impl StoredIntegration {
    /// The INBOUND projection of this integration: what
    /// `DescribeInboundIntegrations` reports for it, which is the same
    /// integration seen from the target that receives the data.
    pub fn inbound(&self) -> InboundIntegration {
        InboundIntegration {
            integration_arn: self.integration.integration_arn.clone(),
            source_arn: self.integration.source_arn.clone(),
            target_arn: self.integration.target_arn.clone(),
            status: self.integration.status.clone(),
            errors: self.integration.errors.clone(),
            create_time: self.integration.create_time,
        }
    }
}

/// The rule for a Redshift-managed VPC endpoint name (`EndpointName`).
///
/// Source: `smithy_query(query="doc", shape="CreateEndpointAccessMessage",
/// member="EndpointName")` — "An endpoint name must contain 1-30 characters.
/// Valid characters are A-Z, a-z, 0-9, and hyphen(-). The first character must be
/// a letter. The name can't contain two consecutive hyphens or end with a
/// hyphen." Uppercase is explicitly VALID here, and the doc states no stored
/// form, so the name is held exactly as the request spelled it (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::endpoint_access`]).
pub const ENDPOINT_NAME_RULE: IdentifierRule = IdentifierRule {
    member: "EndpointName",
    max_len: 30,
    lowercase_only: false,
};

/// The `EndpointAccess$EndpointStatus` of an endpoint a create just recorded,
/// until [`PROVISIONING_SPAN`] has passed.
///
/// `smithy_query(query="doc", shape="EndpointAccess", member="EndpointStatus")`
/// documents only "the status of the endpoint" and the model declares no enum for
/// it, so the three values this service reports are the cluster lifecycle's own
/// ([`STATUS_CREATING`], [`STATUS_AVAILABLE`]'s endpoint counterpart below, and
/// [`STATUS_DELETING`]): a Redshift-managed VPC endpoint is provisioned, serving,
/// or being torn down.
pub const ENDPOINT_STATUS_CREATING: &str = STATUS_CREATING;

/// The `EndpointAccess$EndpointStatus` of an endpoint that is serving
/// connections, which is the state a modify or a delete may be asked for in.
pub const ENDPOINT_STATUS_ACTIVE: &str = "active";

/// The `EndpointAccess$EndpointStatus` of an endpoint whose deletion is under
/// way: the record a `DeleteEndpointAccess` reported, until
/// [`PROVISIONING_SPAN`] has passed and
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] drops it.
pub const ENDPOINT_STATUS_DELETING: &str = STATUS_DELETING;

/// The `AuthorizationStatus` of an authorization that is in force.
///
/// Source: `smithy_query(query="shape", shape="AuthorizationStatus")` —
/// `Authorized | Revoking`.
pub const AUTHORIZATION_STATUS_AUTHORIZED: &str = "Authorized";

/// The `AuthorizationStatus` of an authorization a revoke has started on, until
/// [`PROVISIONING_SPAN`] has passed and
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::settle`] drops it.
pub const AUTHORIZATION_STATUS_REVOKING: &str = "Revoking";

/// How many Redshift-managed VPC endpoints one cluster may have before
/// `CreateEndpointAccess` refuses with the modelled
/// `EndpointsPerClusterLimitExceededFault`.
///
/// The same reasoning as [`USAGE_LIMIT_QUOTA`]: Amazon Redshift publishes an
/// adjustable quota on endpoints per cluster and this emulator models neither
/// Service Quotas nor an account's adjustments, so this is the emulator's OWN
/// bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::endpoint_accesses`]
/// rather than a claim about any account's limit. Every record it holds is
/// released by `DeleteEndpointAccess`.
pub const ENDPOINTS_PER_CLUSTER_QUOTA: usize = 1_000;

/// How many endpoint authorizations one cluster may carry before
/// `AuthorizeEndpointAccess` refuses with the modelled
/// `EndpointAuthorizationsPerClusterLimitExceededFault`.
///
/// The emulator's own bound, for the same reason as
/// [`ENDPOINTS_PER_CLUSTER_QUOTA`]; every record it holds is released by
/// `RevokeEndpointAccess`.
pub const ENDPOINT_AUTHORIZATIONS_PER_CLUSTER_QUOTA: usize = 1_000;

/// The DNS address a Redshift-managed VPC endpoint is reached at.
///
/// Source: `smithy_query(query="doc", shape="EndpointAccess", member="Address")`
/// — "The DNS address of the endpoint". Built in the same Redshift hostname form
/// as a cluster's own endpoint ([`endpoint_address`]) and derived from the
/// endpoint's own name and Region, so the same endpoint always reports the same
/// address and nothing here is drawn per run.
pub fn endpoint_access_address(endpoint_name: &str, region: &str) -> String {
    let token = short_digest(&format!("endpoint-access/{endpoint_name}/{region}"), 12);
    format!("{endpoint_name}.{token}.{region}.redshift.amazonaws.com")
}

/// The subnet of `group` that Amazon Redshift deploys the endpoint
/// `endpoint_name` into, or `None` when the group holds no subnet.
///
/// Source: `smithy_query(query="doc", shape="CreateEndpointAccessMessage",
/// member="SubnetGroupName")` — "The subnet group from which Amazon Redshift
/// chooses the subnet to deploy the endpoint." The service chooses, so the choice
/// is DERIVED from the endpoint's own name: the same endpoint always reports the
/// same subnet and zone, and two endpoints of one group can land in different
/// subnets.
pub fn endpoint_deployment_subnet(group: &ClusterSubnetGroup, endpoint_name: &str) -> Option<Subnet> {
    let subnets = group.subnets.as_deref().unwrap_or_default();
    if subnets.is_empty() {
        return None;
    }
    let index = digest_byte(&format!("endpoint-subnet/{endpoint_name}")) as usize % subnets.len();
    subnets.get(index).cloned()
}

/// The security-group memberships a Redshift-managed VPC endpoint reports.
///
/// Source: `smithy_query(query="doc", shape="EndpointAccess",
/// member="VpcSecurityGroups")` — "The security groups associated with the
/// endpoint". The ONE rule for them, so the create path and the modify path
/// cannot report a membership differently: an endpoint that named no group is in
/// its VPC's default one, exactly as a cluster that named none is
/// ([`default_vpc_security_group_id`]), so the list is never empty.
pub fn endpoint_security_groups(
    requested: Option<&[String]>,
    vpc_id: &str,
) -> Vec<VpcSecurityGroupMembership> {
    let named = requested.unwrap_or_default();
    let ids: Vec<String> = if named.is_empty() {
        vec![default_vpc_security_group_id(vpc_id)]
    } else {
        named.to_vec()
    };
    ids.into_iter()
        .map(|id| VpcSecurityGroupMembership {
            vpc_security_group_id: Some(id),
            status: Some(SECURITY_GROUP_STATUS_ACTIVE.to_string()),
        })
        .collect()
}

/// Which sentence an `EndpointNotFoundFault` is worded with.
///
/// The same split as [`ClusterNotFoundWording`]: the fault's status (404) and
/// awsQuery code (`EndpointNotFound`) are what the model fixes, while the fixed
/// text around the `EndpointName` is an attribute of the OPERATION, and recorded
/// refusals show both sentences below in use. Naming each wording here keeps the
/// choice at the handler that makes it, so no operation can quietly inherit
/// another one's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointNotFoundWording {
    /// `Endpoint <EndpointName> can't be found` — what the endpoint READ,
    /// `DescribeEndpointAccess`, answers. No trailing period.
    EndpointCannotBeFound,
    /// `The endpoint name <EndpointName> can't be found.` — what the operations
    /// that ACT on the endpoint they name answer (`DeleteEndpointAccess`,
    /// `ModifyEndpointAccess`): the same split a read and a write make for a
    /// snapshot ([`ClusterSnapshotNotFoundWording`]).
    TheEndpointNameCannotBeFound,
}

/// The `EndpointNotFoundFault` message `wording` words, naming the
/// `EndpointName` the request asked about.
pub fn endpoint_not_found_message(
    wording: EndpointNotFoundWording,
    endpoint_name: &str,
) -> String {
    match wording {
        EndpointNotFoundWording::EndpointCannotBeFound => {
            format!("Endpoint {endpoint_name} can't be found")
        }
        EndpointNotFoundWording::TheEndpointNameCannotBeFound => {
            format!("The endpoint name {endpoint_name} can't be found.")
        }
    }
}

/// What `ModifyUsageLimit` and `DeleteUsageLimit` answer when the
/// `UsageLimitId` they resolve names no stored limit.
///
/// The fault's status (404) and awsQuery code (`UsageLimitNotFound`) are what
/// the model fixes; this is the fixed text around the id, with no trailing
/// period.
pub fn usage_limit_not_found_message(usage_limit_id: &str) -> String {
    format!("Usage limit {usage_limit_id} is not found")
}

/// What `AuthorizeEndpointAccess` and `RevokeEndpointAccess` answer when the
/// `Account` the request names is the caller's own.
///
/// An endpoint authorization grants one account access to ANOTHER account's
/// cluster, so the grantee can never be the grantor: the guard is a judgement on
/// the request's own argument and is made before any name the request carries is
/// resolved. A FIXED sentence — the refusal names no account, because the only
/// account it could name is the caller's own.
pub const SELF_ACCOUNT_NOT_GRANTABLE: &str =
    "Specify an AWS account that is different from the current AWS account.";

/// What `CreateEndpointAccess` answers when the account already holds an
/// endpoint under the requested name.
pub fn endpoint_exists_message(endpoint_name: &str) -> String {
    format!("Endpoint {endpoint_name} already exists.")
}

/// What every operation on an endpoint authorization answers when the cluster and
/// account pair it resolves names none.
pub fn endpoint_authorization_not_found_message(
    cluster_identifier: &str,
    grantee: &str,
) -> String {
    format!(
        "Endpoint authorization for cluster {cluster_identifier} and account {grantee} not found."
    )
}

/// The Amazon Web Services account id `member` names, or why the value names no
/// account.
///
/// The one form an account id takes — [`ACCOUNT_ID_DIGITS`] digits, as
/// [`account_with_restore_access`] also admits — checked for whichever member
/// carries it, so a request naming an account that cannot exist is refused rather
/// than stored.
pub fn account_id_value(member: &str, value: &str) -> Result<String, String> {
    if value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(value.to_string());
    }
    Err(format!(
        "Invalid value for {member}: '{value}'. It must be a {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account identifier."
    ))
}

/// A Redshift-managed VPC endpoint as this service stores it: the modelled
/// projection every endpoint operation reports, plus the two clock-owed instants
/// the model carries no member for.
#[derive(Debug, Clone)]
pub struct StoredEndpointAccess {
    /// Exactly what `DescribeEndpointAccess` reports, so a read never recomputes
    /// it.
    pub endpoint: CreateEndpointAccessOutput,
    /// When the clock-owed [`ENDPOINT_STATUS_CREATING`] ->
    /// [`ENDPOINT_STATUS_ACTIVE`] transition falls due. See
    /// [`PROVISIONING_SPAN`].
    pub active_at: Instant,
    /// When the clock-owed REMOVAL of this record falls due, set by the
    /// `DeleteEndpointAccess` that asked for it and `None` while the endpoint
    /// exists. See [`PROVISIONING_SPAN`].
    pub removed_at: Option<Instant>,
}

impl StoredEndpointAccess {
    /// The VPC this endpoint has its interface in, as its own record carries it.
    pub fn vpc_id(&self) -> Option<&str> {
        self.endpoint
            .vpc_endpoint
            .as_ref()
            .and_then(|vpc_endpoint| vpc_endpoint.vpc_id.as_deref())
    }
}

/// An endpoint authorization as this service stores it: the record the
/// authorize minted, minus the members read off the state around it (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::endpoint_authorization_view`]),
/// plus when the revoke that started on it falls due.
#[derive(Debug, Clone)]
pub struct StoredEndpointAuthorization {
    /// The authorization itself: who granted it, to whom, on which cluster, when,
    /// which VPCs it allows and whether a revoke has started on it.
    pub authorization: AuthorizeEndpointAccessOutput,
    /// When the clock-owed REMOVAL of this record falls due, set by the
    /// `RevokeEndpointAccess` that asked for it and `None` while the
    /// authorization is in force. See [`PROVISIONING_SPAN`].
    pub removed_at: Option<Instant>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftRedshiftserviceversion20121201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftRedshiftserviceversion20121201State>)
}
