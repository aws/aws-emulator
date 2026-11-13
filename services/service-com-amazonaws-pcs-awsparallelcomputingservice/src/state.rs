// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcs-awsparallelcomputingservice service.
//!
//! PCS is a control plane over three nested resources: a cluster, and the compute node
//! groups and queues that live inside one. Every operation computes its answer from the
//! records held here — a create mints an id and an ARN and stores the record, a read
//! looks it up, a delete takes it out.
//!
//! Besides the records, this module owns the three things more than one handler needs,
//! so no two operations can disagree about them:
//!
//! * [`ComAmazonawsPcsAwsparallelcomputingserviceState::settle`] — the lifecycle
//!   transitions the CLOCK owes. A created resource is reported `CREATING` and becomes
//!   `ACTIVE` once [`TRANSITION_SPAN`] has elapsed; a deleted cluster is reported
//!   `DELETING` and then disappears. The due instant is stored and checked on the next
//!   call, so nothing ever sleeps.
//! * the identifier rules: minting a `pcs_...` id, building an ARN, resolving a
//!   `clusterIdentifier`/`queueIdentifier`/`computeNodeGroupIdentifier` by name OR id,
//!   and refusing a resource name the boundary validator cannot refuse
//!   ([`resource_name_violation`]).
//! * paginating a creation-ordered list behind an opaque `nextToken`
//!   ([`paginate`]).
//!
//! # Every store here is bounded
//!
//! Nothing a caller can repeat grows this state without a ceiling:
//!
//! * record COUNT — [`MAX_CLUSTERS`], [`MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER`] and
//!   [`MAX_QUEUES_PER_CLUSTER`] are checked before the insert in each create, which
//!   refuses the overflowing call with the modelled `ServiceQuotaExceededException`
//!   ([`quota_exceeded`]).
//! * record SIZE — the caller-sized lists a create persists are capped at
//!   [`MAX_STORED_LIST_ENTRIES`] ([`stored_list_violation`]); the key and value LENGTHS
//!   of a tag need no check here because `TagKey` is modelled `@length(..128)` and
//!   `TagValue` `@length(..256)`, both enforced at the boundary before a handler runs.
//! * tag COUNT — a resource's stored tag set is capped at [`MAX_TAGS_PER_RESOURCE`]
//!   ([`tag_quota_exceeded`]) on every path that writes tags, a create carrying `tags`
//!   as much as a later `TagResource`; `RequestTagMap`'s modelled
//!   `@length(min: 1, max: 200)` bounds one request only.
//! * the idempotency index — [`MAX_REMEMBERED_CLIENT_TOKENS`] entries, evicted
//!   oldest-first, and every entry pointing into a cluster is dropped when that
//!   cluster is removed ([`ComAmazonawsPcsAwsparallelcomputingserviceState::settle`]),
//!   so a create/delete loop cannot grow it.
//! * response size — [`page_size`] clamps a caller's `maxResults` to the modelled
//!   `1..=100` before [`paginate`] truncates.

/// The EC2 subnets a request places a resource in, resolved against the service that owns
/// subnets.
pub mod ec2_subnets;

/// The IAM instance profile a compute node group's instances assume, resolved against the
/// service that owns instance profiles.
pub mod instance_profile;

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Accounting, AccountingRequest, Cluster, ClusterSlurmConfiguration,
    ClusterSlurmConfigurationRequest, ClusterSummary, ComputeNodeGroup,
    ComputeNodeGroupConfiguration, ComputeNodeGroupSlurmConfiguration,
    ComputeNodeGroupSlurmConfigurationRequest, ComputeNodeGroupSummary, ConflictException,
    CustomLaunchTemplate, Endpoint, ErrorInfo, InstanceConfig, Networking, NetworkingRequest,
    NodeLifecycleActions, NodeLifecycleActionsRequest, Queue, QueueSlurmConfiguration,
    QueueSlurmConfigurationRequest, QueueSummary, ResourceNotFoundException, ScalingConfiguration,
    ScalingConfigurationRequest, Scheduler, SchedulerRequest, ServiceQuotaExceededException,
    SlurmRest, SlurmRestRequest, SpotOptions, UpdateClusterSlurmConfigurationRequest,
    ValidationException,
    ValidationExceptionField,
};

/// The `ClusterStatus`/`QueueStatus`/`ComputeNodeGroupStatus` member a just-created
/// resource carries (every one of the three enums declares it; see
/// `CLUSTER_STATUS_VALUES` in `crate::ops::types`).
pub const STATUS_CREATING: &str = "CREATING";

/// The status a resource reaches once its creation has come due.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The status a cluster carries between `DeleteCluster` and its removal.
pub const STATUS_DELETING: &str = "DELETING";

/// The status a compute node group carries once its delete has come due, before the
/// record is forgotten.
///
/// `ComputeNodeGroupStatus` is the ONLY one of the service's three status enums that
/// declares a `DELETED` member (`COMPUTE_NODE_GROUP_STATUS_VALUES` in
/// `crate::ops::types`, against `CLUSTER_STATUS_VALUES` and `QUEUE_STATUS_VALUES`), so a
/// deleted compute node group is a thing the API can report and a deleted cluster or queue
/// is not: the group is reported `DELETED` for one span and then forgotten, while a queue
/// and a cluster go straight from `DELETING` to gone.
pub const STATUS_DELETED: &str = "DELETED";

/// The status a cluster carries while an `UpdateCluster` is being applied (a
/// `ClusterStatus` member), until the clock owes it [`STATUS_ACTIVE`] again.
pub const STATUS_UPDATING: &str = "UPDATING";

/// The cluster statuses an update can start from: "You can only update clusters that are
/// in ACTIVE, UPDATE_FAILED, or SUSPENDED state" (`UpdateCluster`'s own documentation).
pub const UPDATABLE_CLUSTER_STATUSES: &[&str] = &[STATUS_ACTIVE, "UPDATE_FAILED", "SUSPENDED"];

/// How a scheduler release is LABELLED: the `major.minor` of a Slurm release, optionally
/// with its patch component — `SchedulerRequest$version` is "the version of the specified
/// scheduling software", pointing at the Slurm versions PCS publishes, and every one of
/// them is labelled that way. The components below are what a label may be made of; the
/// SET of releases the service can provision is not stated anywhere in the model, and the
/// lists its documentation prints move as releases ship, so no such set is a gate here.
const SCHEDULER_VERSION_COMPONENTS: std::ops::RangeInclusive<usize> = 2..=3;

/// The most digits one component of a scheduler release label carries. With
/// [`SCHEDULER_VERSION_COMPONENTS`] this bounds the length of the version a create
/// PERSISTS, which the model — where `version` is a plain `String` with no `@length` —
/// does not.
const SCHEDULER_VERSION_COMPONENT_DIGITS: usize = 4;

/// The `AccountingMode`/`SlurmRestMode` member that turns an optional Slurm daemon on
/// ("A value of STANDARD means Slurm accounting is enabled" / "... the Slurm REST API is
/// enabled"). Both enums declare exactly `STANDARD` and `NONE`.
pub const MODE_STANDARD: &str = "STANDARD";

/// The `AccountingMode`/`SlurmRestMode` member that leaves an optional Slurm daemon off,
/// which is also what both members default to when a request names neither.
pub const MODE_NONE: &str = "NONE";

/// The `EndpointType` member of the Slurm controller endpoint every provisioned cluster
/// publishes (see `ENDPOINT_TYPE_VALUES` in `crate::ops::types`).
pub const ENDPOINT_SLURMCTLD: &str = "SLURMCTLD";

/// The `EndpointType` member of the Slurm accounting daemon's endpoint.
pub const ENDPOINT_SLURMDBD: &str = "SLURMDBD";

/// The `EndpointType` member of the Slurm REST API's endpoint.
pub const ENDPOINT_SLURMRESTD: &str = "SLURMRESTD";

/// The port `slurmctld` listens on — Slurm's own default `SlurmctldPort` — rendered as
/// the `String` the modelled `Endpoint$port` declares.
pub const SLURMCTLD_PORT: &str = "6817";

/// The port `slurmdbd` listens on — Slurm's own default `DbdPort`.
pub const SLURMDBD_PORT: &str = "6819";

/// The port `slurmrestd` listens on — Slurm's own default REST API port.
pub const SLURMRESTD_PORT: &str = "6820";

/// The `NetworkType` member a cluster whose control plane is addressed over IPv6 carries.
pub const NETWORK_TYPE_IPV6: &str = "IPV6";

/// How long after the call that starts it a PCS lifecycle transition the clock owes
/// comes due: a created resource leaving `CREATING`, a deleted cluster leaving
/// `DELETING` and disappearing. PCS itself takes minutes to provision a cluster; the
/// emulator keeps the transient state observable to the caller that acts straight after
/// the call, and never sleeps for it — the due instant is stored and read on the next
/// call.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The `maxResults` a list operation serves when the request names none: the
/// `@default(10)` the model declares on `MaxResults`.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The region an ARN carries when the request carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The prefix PCS reserves for the ids it generates (`pcs_...`); a caller-chosen
/// resource name can never take it.
pub const GENERATED_ID_PREFIX: &str = "pcs_";

/// The most clusters this emulator holds. PCS caps clusters per account per Region with
/// an account-adjustable service quota and answers `ServiceQuotaExceededException` once
/// it is reached ("The max number of clusters or queues has been reached for the
/// account"); the NUMBER here is the emulator's own ceiling, not a published AWS
/// default, and exists so a repeated create cannot grow this process without bound.
pub const MAX_CLUSTERS: usize = 50;

/// The most compute node groups one cluster holds, refused the same way ("The max number
/// of compute node groups has been reached for the associated cluster"). The number is
/// the emulator's ceiling.
pub const MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER: usize = 20;

/// The most queues one cluster holds, refused the same way. The number is the emulator's
/// ceiling.
pub const MAX_QUEUES_PER_CLUSTER: usize = 50;

/// The most entries a caller-sized list a create PERSISTS may carry (`subnetIds`,
/// `instanceConfigs`, `computeNodeGroupConfigurations`, and the Slurm custom-setting
/// lists). The model bounds none of them, so without this one request could size one
/// record arbitrarily; the ceiling matches the 200 the model allows for `RequestTagMap`,
/// the one caller-sized collection it does bound.
pub const MAX_STORED_LIST_ENTRIES: usize = 200;

/// The most tags one PCS resource holds, matching AWS's resource-tagging limit of 50
/// tags per resource. Checked on every path that writes tags — the `tags` a create
/// carries, and the set a `TagResource` would LEAVE BEHIND, so repeated tag writes cannot
/// grow a record past it either — and the overflowing call is refused with the modelled
/// `ServiceQuotaExceededException` ([`tag_quota_exceeded`]) and persists nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most `@idempotencyToken` values remembered at once. Beyond this the oldest entry
/// is evicted, so a caller minting fresh tokens cannot grow the index without bound; a
/// retry whose token has been evicted is served as a fresh create, which is what a
/// caller retrying long after the fact would get from the real service too.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 1024;

/// What the clock owes a resource once its pending transition comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingChange {
    /// `CREATING` -> [`STATUS_ACTIVE`].
    Activate,
    /// `DELETING` -> [`STATUS_DELETED`], and then owed [`PendingChange::Remove`] (see
    /// [`STATUS_DELETED`] for why only a compute node group takes this step).
    MarkDeleted,
    /// `DELETING` -> gone from state.
    Remove,
}

/// A transition the clock owes a resource, and the instant it comes due.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    /// The instant from which the change is owed. Never read by sleeping on it.
    pub due_at: Instant,
    /// The change the clock owes.
    pub change: PendingChange,
}

impl Pending {
    /// The change, owed [`TRANSITION_SPAN`] after now.
    pub fn in_a_span(change: PendingChange) -> Self {
        Self {
            due_at: Instant::now() + TRANSITION_SPAN,
            change,
        }
    }

    /// Whether `now` has reached the due instant.
    fn is_due(self, now: Instant) -> bool {
        now >= self.due_at
    }
}

/// A persisted cluster: everything `GetCluster`, `ListClusters` and the child creates
/// read.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    /// Creation order, which is the order a list walks and what a `nextToken` resumes
    /// from.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `clusterName`.
    pub name: String,
    /// The ARN built by [`cluster_arn`].
    pub arn: String,
    /// A `ClusterStatus` member.
    pub status: String,
    /// When the record was created; never changes afterwards.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The scheduler the request asked for.
    pub scheduler: Scheduler,
    /// A `Size` member.
    pub size: String,
    /// The control-plane networking the request asked for.
    pub networking: Networking,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<ClusterSlurmConfiguration>,
    /// Scheduler endpoints, published once the cluster has finished provisioning (see
    /// [`ClusterRecord::scheduler_endpoints`]). Empty while it is still `CREATING`.
    pub endpoints: Vec<Endpoint>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried, keyed for a later `TagResource`.
    pub tags: HashMap<String, String>,
    /// The cluster's compute node groups, by generated id.
    pub compute_node_groups: HashMap<String, ComputeNodeGroupRecord>,
    /// The cluster's queues, by generated id.
    pub queues: HashMap<String, QueueRecord>,
    /// The transition the clock owes this cluster, if any.
    pub pending: Option<Pending>,
}

/// A persisted compute node group, stored under its parent cluster.
#[derive(Debug, Clone)]
pub struct ComputeNodeGroupRecord {
    /// Creation order within the cluster.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `computeNodeGroupName`.
    pub name: String,
    /// The ARN built by [`compute_node_group_arn`].
    pub arn: String,
    /// The id of the cluster this group belongs to.
    pub cluster_id: String,
    /// A `ComputeNodeGroupStatus` member.
    pub status: String,
    /// When the record was created.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The AMI the request named, if any.
    pub ami_id: Option<String>,
    /// The subnets instances are provisioned in.
    pub subnet_ids: Vec<String>,
    /// A `PurchaseOption` member, when the request named one.
    pub purchase_option: Option<String>,
    /// The launch template the request named.
    pub custom_launch_template: CustomLaunchTemplate,
    /// The instance profile the request named.
    pub iam_instance_profile_arn: String,
    /// The scaling bounds the request asked for.
    pub scaling_configuration: ScalingConfiguration,
    /// The instance types the request listed.
    pub instance_configs: Vec<InstanceConfig>,
    /// Spot options, when the request carried any.
    pub spot_options: Option<SpotOptions>,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<ComputeNodeGroupSlurmConfiguration>,
    /// Lifecycle actions, when the request carried any.
    pub node_lifecycle_actions: Option<NodeLifecycleActions>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried.
    pub tags: HashMap<String, String>,
    /// The transition the clock owes this group, if any.
    pub pending: Option<Pending>,
}

/// A persisted queue, stored under its parent cluster.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    /// Creation order within the cluster.
    pub seq: u64,
    /// The generated `pcs_...` id.
    pub id: String,
    /// The caller-chosen `queueName`.
    pub name: String,
    /// The ARN built by [`queue_arn`].
    pub arn: String,
    /// The id of the cluster this queue belongs to.
    pub cluster_id: String,
    /// A `QueueStatus` member.
    pub status: String,
    /// When the record was created.
    pub created_at: DateTime,
    /// When the record last changed.
    pub modified_at: DateTime,
    /// The compute node groups the queue assigns jobs to.
    pub compute_node_group_configurations: Vec<ComputeNodeGroupConfiguration>,
    /// Slurm options, when the request carried any.
    pub slurm_configuration: Option<QueueSlurmConfiguration>,
    /// Provisioning errors. Empty on the path the emulator implements.
    pub error_info: Vec<ErrorInfo>,
    /// The tags the create request carried.
    pub tags: HashMap<String, String>,
    /// The transition the clock owes this queue, if any.
    pub pending: Option<Pending>,
}

impl ClusterRecord {
    /// The modelled `Cluster`, read off the stored record.
    pub fn to_api(&self) -> Cluster {
        Cluster {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            scheduler: self.scheduler.clone(),
            size: self.size.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            networking: self.networking.clone(),
            endpoints: non_empty(&self.endpoints),
            error_info: non_empty(&self.error_info),
        }
    }

    /// The modelled `ClusterSummary` `ListClusters` returns.
    pub fn to_summary(&self) -> ClusterSummary {
        ClusterSummary {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
        }
    }

    /// The compute node group `identifier` names — its generated id or its name.
    pub fn compute_node_group(&self, identifier: &str) -> Option<&ComputeNodeGroupRecord> {
        self.compute_node_groups.get(identifier).or_else(|| {
            self.compute_node_groups
                .values()
                .find(|group| group.name == identifier)
        })
    }

    /// The queue `identifier` names — its generated id or its name.
    pub fn queue(&self, identifier: &str) -> Option<&QueueRecord> {
        self.queues
            .get(identifier)
            .or_else(|| self.queues.values().find(|queue| queue.name == identifier))
    }

    /// The ids of this cluster's queues that STILL assign jobs to the compute node group
    /// `group_id`, in creation order.
    ///
    /// A queue's `computeNodeGroupConfigurations` name a group the way the request wrote
    /// it — by its generated id or by its name, the two forms
    /// `ComputeNodeGroupIdentifier` allows — so each configuration is resolved through
    /// [`Self::compute_node_group`] before it is compared, rather than matched as text.
    /// That makes this the ONE answer to "is this group still in use", which the delete
    /// of a group and the delete of a queue cannot disagree about.
    ///
    /// A queue that is itself already being deleted is NOT counted: the dependency that
    /// blocks a delete is an `ACTIVE` dependent ("A cluster to delete is in an unstable
    /// state. For example, because it still has `ACTIVE` node groups or queues", the
    /// model's own `ConflictException` example), so a caller deleting a hierarchy in order
    /// is not made to wait out each child's removal before starting the next.
    pub fn queues_assigning_to(&self, group_id: &str) -> Vec<String> {
        let mut referencing: Vec<(u64, String)> = self
            .queues
            .values()
            .filter(|queue| !queue.is_being_deleted())
            .filter(|queue| {
                queue
                    .compute_node_group_configurations
                    .iter()
                    .filter_map(|configuration| configuration.compute_node_group_id.as_deref())
                    .any(|named| {
                        self.compute_node_group(named)
                            .is_some_and(|group| group.id == group_id)
                    })
            })
            .map(|queue| (queue.seq, queue.id.clone()))
            .collect();
        referencing.sort_by_key(|(seq, _)| *seq);
        referencing.into_iter().map(|(_, id)| id).collect()
    }

    /// How many of this cluster's children still block its own delete: every queue and
    /// compute node group that is not already being deleted (see
    /// [`Self::queues_assigning_to`] for why a child on its way out does not).
    pub fn blocking_children(&self) -> usize {
        self.queues
            .values()
            .filter(|queue| !queue.is_being_deleted())
            .count()
            + self
                .compute_node_groups
                .values()
                .filter(|group| !group.is_being_deleted())
                .count()
    }

    /// Apply every transition the clock owes this cluster and its children at `now`.
    /// `true` means the cluster itself is now owed removal.
    fn settle(&mut self, now: Instant) -> bool {
        self.compute_node_groups
            .retain(|_, group| !group.settle(now));
        self.queues.retain(|_, queue| !queue.settle(now));
        let removable =
            apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now);
        // A cluster that has finished provisioning publishes the scheduler endpoints its
        // own configuration enables. Derived from the stored record, so recomputing it
        // is idempotent: a read can never change what a later caller sees.
        if !removable && self.status == STATUS_ACTIVE {
            self.endpoints = self.scheduler_endpoints();
        }
        removable
    }

    /// The scheduler endpoints this cluster publishes.
    ///
    /// The Slurm controller is how a client reaches the scheduler at all, so a
    /// provisioned cluster always publishes a [`ENDPOINT_SLURMCTLD`] endpoint; the
    /// optional daemons are published exactly when the cluster's Slurm configuration
    /// turns them on — accounting ([`ENDPOINT_SLURMDBD`]) and the Slurm REST API
    /// ([`ENDPOINT_SLURMRESTD`]), each `STANDARD` rather than `NONE`.
    fn scheduler_endpoints(&self) -> Vec<Endpoint> {
        let slurm = self.slurm_configuration.as_ref();
        let accounting = slurm
            .and_then(|slurm| slurm.accounting.as_ref())
            .is_some_and(|accounting| accounting.mode == MODE_STANDARD);
        let slurm_rest = slurm
            .and_then(|slurm| slurm.slurm_rest.as_ref())
            .is_some_and(|slurm_rest| slurm_rest.mode == MODE_STANDARD);
        [
            (ENDPOINT_SLURMCTLD, SLURMCTLD_PORT, true),
            (ENDPOINT_SLURMDBD, SLURMDBD_PORT, accounting),
            (ENDPOINT_SLURMRESTD, SLURMRESTD_PORT, slurm_rest),
        ]
        .into_iter()
        .filter(|(_, _, published)| *published)
        .map(|(endpoint_type, port, _)| self.endpoint(endpoint_type, port))
        .collect()
    }

    /// One endpoint of this cluster's control plane, on the address family its
    /// `networking.networkType` asks for.
    fn endpoint(&self, endpoint_type: &str, port: &str) -> Endpoint {
        let ipv6 = self.networking.network_type.as_deref() == Some(NETWORK_TYPE_IPV6);
        Endpoint {
            r#type: endpoint_type.to_string(),
            // "For clusters configured to use IPv6, this is an empty string"
            // (`Endpoint$privateIpAddress`).
            private_ip_address: if ipv6 {
                String::new()
            } else {
                control_plane_ipv4(self.seq)
            },
            // PCS runs the cluster controller in a service-owned account and reaches it
            // over the caller's subnets, so no public address is reported.
            public_ip_address: None,
            ipv6_address: ipv6.then(|| control_plane_ipv6(self.seq)),
            port: port.to_string(),
        }
    }
}

impl ComputeNodeGroupRecord {
    /// The modelled `ComputeNodeGroup`, read off the stored record.
    pub fn to_api(&self) -> ComputeNodeGroup {
        ComputeNodeGroup {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
            ami_id: self.ami_id.clone(),
            subnet_ids: self.subnet_ids.clone(),
            purchase_option: self.purchase_option.clone(),
            custom_launch_template: self.custom_launch_template.clone(),
            iam_instance_profile_arn: self.iam_instance_profile_arn.clone(),
            scaling_configuration: self.scaling_configuration.clone(),
            instance_configs: self.instance_configs.clone(),
            spot_options: self.spot_options.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            node_lifecycle_actions: self.node_lifecycle_actions.clone(),
            error_info: non_empty(&self.error_info),
        }
    }

    /// Apply the transition the clock owes this group at `now`; `true` means removal.
    fn settle(&mut self, now: Instant) -> bool {
        apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now)
    }

    /// Whether this group is already on its way out, so it blocks no delete of the
    /// cluster that holds it.
    pub fn is_being_deleted(&self) -> bool {
        self.status == STATUS_DELETING || self.status == STATUS_DELETED
    }

    /// The modelled `ComputeNodeGroupSummary` `ListComputeNodeGroups` returns.
    pub fn to_summary(&self) -> ComputeNodeGroupSummary {
        ComputeNodeGroupSummary {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
        }
    }
}

impl QueueRecord {
    /// The modelled `Queue`, read off the stored record.
    pub fn to_api(&self) -> Queue {
        Queue {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
            compute_node_group_configurations: self.compute_node_group_configurations.clone(),
            slurm_configuration: self.slurm_configuration.clone(),
            error_info: non_empty(&self.error_info),
        }
    }

    /// The modelled `QueueSummary` `ListQueues` returns.
    pub fn to_summary(&self) -> QueueSummary {
        QueueSummary {
            name: self.name.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            cluster_id: self.cluster_id.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            status: self.status.clone(),
        }
    }

    /// Apply the transition the clock owes this queue at `now`; `true` means removal.
    fn settle(&mut self, now: Instant) -> bool {
        apply_pending(&mut self.pending, &mut self.status, &mut self.modified_at, now)
    }

    /// Whether this queue is already on its way out, so it blocks neither the delete of
    /// a compute node group it assigns jobs to nor the delete of its cluster.
    pub fn is_being_deleted(&self) -> bool {
        self.status == STATUS_DELETING
    }
}

/// What a previous create with an `@idempotencyToken` produced, so a retry can be
/// answered with it rather than creating a second resource.
#[derive(Debug, Clone)]
pub struct RememberedCreate {
    /// The cluster the resource was created in; `None` for a cluster itself.
    pub cluster_id: Option<String>,
    /// The id the first call created.
    pub resource_id: String,
}

/// Per-service state for com-amazonaws-pcs-awsparallelcomputingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPcsAwsparallelcomputingserviceState {
    /// Every cluster, by its generated id. Compute node groups and queues are nested
    /// inside their cluster, so deleting a cluster cannot orphan one. At most
    /// [`MAX_CLUSTERS`] entries; each cluster holds at most
    /// [`MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER`] groups and [`MAX_QUEUES_PER_CLUSTER`]
    /// queues, and every caller-sized collection inside a record is bounded (see the
    /// module docs).
    pub clusters: HashMap<String, ClusterRecord>,
    /// `clusterName` -> cluster id, so a `clusterIdentifier` resolves by name or by id
    /// and a duplicate name is a conflict. One entry per cluster, removed with it.
    pub cluster_ids_by_name: HashMap<String, String>,
    /// Source of both the generated ids and the creation-order sequence a list walks.
    pub next_seq: u64,
    /// `<operation>:<clientToken>` -> what the first call created, so repeating an
    /// `@idempotencyToken` create returns that call's result instead of creating a
    /// second resource. Bounded at [`MAX_REMEMBERED_CLIENT_TOKENS`] by the FIFO order
    /// below, and pruned when the cluster an entry points into is removed.
    pub client_tokens: HashMap<String, RememberedCreate>,
    /// The keys of [`Self::client_tokens`] in insertion order, so the oldest entry can
    /// be evicted once the index is full. Holds exactly the map's keys.
    pub client_token_order: VecDeque<String>,
}

impl ComAmazonawsPcsAwsparallelcomputingserviceState {
    /// Allocate the next creation sequence and the `pcs_...` id that goes with it.
    pub fn mint_id(&mut self) -> (u64, String) {
        self.next_seq += 1;
        let seq = self.next_seq;
        (seq, format!("{GENERATED_ID_PREFIX}{:016x}", scramble(seq)))
    }

    /// The cluster `identifier` names — its generated id or its `clusterName`, the two
    /// forms `ClusterIdentifier`'s pattern allows.
    pub fn cluster(&self, identifier: &str) -> Option<&ClusterRecord> {
        let id = self.cluster_id_for(identifier)?;
        self.clusters.get(&id)
    }

    /// [`Self::cluster`], mutably.
    pub fn cluster_mut(&mut self, identifier: &str) -> Option<&mut ClusterRecord> {
        let id = self.cluster_id_for(identifier)?;
        self.clusters.get_mut(&id)
    }

    /// The tags of the resource `arn` names — a cluster, or a compute node group or a
    /// queue inside one — or `None` when no stored resource carries that ARN.
    ///
    /// Tags live on the record they belong to (see [`ClusterRecord::tags`]), so a create
    /// that carried tags, a later tag write and a tag read all see one set. The lookup is
    /// DIRECT: the ARN names the cluster it belongs to and the resource id at its tail
    /// ([`arn_cluster_id`], [`arn_resource`]), so neither the cluster map nor a cluster's
    /// children are scanned. The record found that way still has to carry exactly the ARN
    /// the request named, so a resource is reachable by exactly the ARN the service
    /// handed out for it and not by one that merely ends in the same id.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&HashMap<String, String>> {
        let (kind, resource_id) = arn_resource(arn);
        let cluster = self.clusters.get(arn_cluster_id(arn)?)?;
        if kind == CLUSTER {
            return (cluster.arn == arn).then_some(&cluster.tags);
        }
        if kind == QUEUE {
            let queue = cluster.queues.get(&resource_id)?;
            return (queue.arn == arn).then_some(&queue.tags);
        }
        if kind == COMPUTE_NODE_GROUP {
            let group = cluster.compute_node_groups.get(&resource_id)?;
            return (group.arn == arn).then_some(&group.tags);
        }
        None
    }

    /// [`Self::tags_by_arn`], mutably: the one write path the tagging operations take,
    /// so no two of them can disagree about where a resource's tags live.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        let (kind, resource_id) = arn_resource(arn);
        let cluster = self.clusters.get_mut(arn_cluster_id(arn)?)?;
        if kind == CLUSTER {
            return (cluster.arn == arn).then_some(&mut cluster.tags);
        }
        if kind == QUEUE {
            let queue = cluster.queues.get_mut(&resource_id)?;
            return (queue.arn == arn).then_some(&mut queue.tags);
        }
        if kind == COMPUTE_NODE_GROUP {
            let group = cluster.compute_node_groups.get_mut(&resource_id)?;
            return (group.arn == arn).then_some(&mut group.tags);
        }
        None
    }

    /// The id of the cluster `identifier` names.
    pub fn cluster_id_for(&self, identifier: &str) -> Option<String> {
        if self.clusters.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        self.cluster_ids_by_name.get(identifier).cloned()
    }

    /// What a previous call with this `@idempotencyToken` created, if it is still
    /// remembered.
    pub fn remembered_create(
        &self,
        operation: &str,
        token: Option<&String>,
    ) -> Option<&RememberedCreate> {
        self.client_tokens.get(&client_token_key(operation, token?))
    }

    /// Remember what `operation` with `token` created, so a retry replays it.
    ///
    /// An existing entry is never overwritten — the token names the FIRST call's
    /// resource for as long as it is remembered — and the index is kept at
    /// [`MAX_REMEMBERED_CLIENT_TOKENS`] entries by evicting the oldest.
    pub fn remember_create(
        &mut self,
        operation: &str,
        token: Option<&String>,
        cluster_id: Option<&str>,
        resource_id: &str,
    ) {
        let Some(token) = token else { return };
        let key = client_token_key(operation, token);
        if self.client_tokens.contains_key(&key) {
            return;
        }
        self.client_tokens.insert(
            key.clone(),
            RememberedCreate {
                cluster_id: cluster_id.map(str::to_string),
                resource_id: resource_id.to_string(),
            },
        );
        self.client_token_order.push_back(key);
        while self.client_token_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop every remembered create that points at `cluster_id` or at a resource inside
    /// it, so removing a cluster releases its share of the index.
    fn forget_creates_for_cluster(&mut self, cluster_id: &str) {
        let Self {
            client_tokens,
            client_token_order,
            ..
        } = self;
        client_tokens.retain(|_, remembered| {
            remembered.resource_id != cluster_id
                && remembered.cluster_id.as_deref() != Some(cluster_id)
        });
        client_token_order.retain(|key| client_tokens.contains_key(key));
    }

    /// Apply every lifecycle transition the clock owes at this instant. Called at the
    /// top of every handler, including the reporting ones: it only materialises a change
    /// that was already due, so a read never moves a status the clock had not reached.
    pub fn settle(&mut self) {
        let now = Instant::now();
        let removable: Vec<String> = self
            .clusters
            .iter_mut()
            .filter_map(|(id, cluster)| cluster.settle(now).then(|| id.clone()))
            .collect();
        for id in removable {
            if let Some(cluster) = self.clusters.remove(&id) {
                if self.cluster_ids_by_name.get(&cluster.name) == Some(&cluster.id) {
                    self.cluster_ids_by_name.remove(&cluster.name);
                }
            }
            self.forget_creates_for_cluster(&id);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcsAwsparallelcomputingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcsAwsparallelcomputingserviceState>)
}

/// The current wall clock, as the `date-time` `Timestamp` the model declares.
pub fn now_date_time() -> DateTime {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// `None` for an empty list, so an absent member stays absent on the wire.
fn non_empty<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// The private IPv4 address the cluster created `seq`th reports its control plane on.
///
/// PCS puts the cluster controller's elastic network interface in one of the caller's
/// subnets, so the real address is whichever one that VPC assigns. The emulator
/// provisions no interface, so it reports an address from the RFC 1918 `10.0.0.0/16`
/// block derived from the creation sequence: distinct per cluster, stable across reads,
/// and never a host or broadcast byte.
fn control_plane_ipv4(seq: u64) -> String {
    let host = seq % 254 + 1;
    let subnet = (seq / 254) % 256;
    format!("10.0.{subnet}.{host}")
}

/// The IPv6 address the cluster created `seq`th reports its control plane on, for a
/// cluster whose `networkType` is IPv6. From the `fd00::/8` unique-local range, for the
/// same reason [`control_plane_ipv4`] uses RFC 1918 space.
fn control_plane_ipv6(seq: u64) -> String {
    format!("fd00::{seq:x}")
}

/// Apply the transition the clock owes one record at `now`; `true` means the record is
/// owed removal and its owner should drop it.
///
/// Shared by all three record types so a new [`PendingChange`] variant cannot end up
/// handled three different ways.
fn apply_pending(
    pending: &mut Option<Pending>,
    status: &mut String,
    modified_at: &mut DateTime,
    now: Instant,
) -> bool {
    match *pending {
        Some(owed) if owed.is_due(now) => match owed.change {
            PendingChange::Activate => {
                *status = STATUS_ACTIVE.to_string();
                *modified_at = now_date_time();
                *pending = None;
                false
            }
            PendingChange::MarkDeleted => {
                *status = STATUS_DELETED.to_string();
                *modified_at = now_date_time();
                *pending = Some(Pending::in_a_span(PendingChange::Remove));
                false
            }
            PendingChange::Remove => true,
        },
        _ => false,
    }
}

/// The key an idempotency token is remembered under, scoped by operation so two
/// operations cannot collide on one token.
fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// SplitMix64's finaliser. A BIJECTION on `u64` — every step (an odd multiply, an
/// xor-shift) is invertible — so distinct creation sequences always produce distinct
/// ids. That makes the generated id opaque, the way PCS's own ids are, without a random
/// source or a collision check.
///
/// Deliberately a permutation of the creation index rather than a hash or a UUID: the
/// uniqueness has to be guaranteed, not merely probable, and `pcs_[a-zA-Z0-9]+` leaves
/// no room for a 36-character UUID. A hash crate would give neither property.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The region the request was signed for. PCS resources are regional, so every ARN the
/// service mints carries it; a request that carries no SigV4 credential scope gets
/// [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>`.
pub fn cluster_arn(region: &str, cluster_id: &str) -> String {
    ::engine::emulator_arn("pcs", region, &format!("cluster/{cluster_id}"))
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>/computenodegroup/<group id>`.
pub fn compute_node_group_arn(region: &str, cluster_id: &str, group_id: &str) -> String {
    ::engine::emulator_arn(
        "pcs",
        region,
        &format!("cluster/{cluster_id}/computenodegroup/{group_id}"),
    )
}

/// `arn:aws:pcs:<region>:<account>:cluster/<cluster id>/queue/<queue id>`.
pub fn queue_arn(region: &str, cluster_id: &str, queue_id: &str) -> String {
    ::engine::emulator_arn(
        "pcs",
        region,
        &format!("cluster/{cluster_id}/queue/{queue_id}"),
    )
}

/// The `@pattern` `clusterName`, `computeNodeGroupName` and `queueName` all carry
/// (`ClusterName`, `ComputeNodeGroupName` and `QueueName` in this service's model): a
/// leading ASCII letter, then only ASCII letters, digits and hyphens, which also rules out
/// the `pcs_` prefix the service reserves for the ids it generates, because `_` is not in
/// the class.
pub const RESOURCE_NAME_PATTERN: &str = "^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$";

/// The modelled `ValidationException` the service refuses this resource name with, or
/// `None` when it accepts it.
///
/// The name has to match [`RESOURCE_NAME_PATTERN`], and the refusal is stated the way
/// every other modelled-constraint violation of this service is
/// ([`constraint_violations`]), because that is what failed: a member constraint the model
/// declares.
///
/// The rule is enforced HERE rather than at the boundary because that regex carries a
/// negative lookahead, which the boundary validator's regex engine cannot compile, and
/// a pattern it cannot compile is treated as matching (see
/// `aws_smithy_schema_server::constraints::pattern_matches`). The `@length` bounds on
/// the same members compile and are enforced at the boundary, so they are not repeated.
pub fn resource_name_violation(member: &str, value: &str) -> Option<ValidationException> {
    let mut chars = value.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest: Vec<char> = chars.collect();
    // `[A-Za-z0-9-]+` is one-or-more, so the remainder must be non-empty as well as
    // inside the class; the `@length(min: 3)` the boundary enforces makes that
    // unreachable through the wire, but the helper stands on its own.
    let rest_is_allowed = !rest.is_empty()
        && rest
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == '-');
    if starts_with_letter && rest_is_allowed {
        return None;
    }
    Some(field_validation(
        member,
        pattern_constraint(RESOURCE_NAME_PATTERN),
    ))
}

/// The constraint clause stated for a value outside its member's `@pattern`.
///
/// The expression is quoted as the service PUBLISHES it rather than as the model writes
/// it: the `^`/`$` pair a model writes around a full-match expression is that wrapping, so
/// it is dropped through the tree's one reading of it
/// (`service_common::validation::unanchored`), and a client that lifts the expression out
/// of the message gets the one the service advertises.
pub fn pattern_constraint(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        ::service_common::validation::unanchored(pattern)
    )
}

/// The constraint clause stated for a collection longer than the most entries its member
/// admits.
pub fn length_at_most_constraint(entries: usize) -> String {
    format!("Member must have length less than or equal to {entries}")
}

/// The modelled `ValidationException` for ONE member whose value failed `constraint`:
/// [`constraint_violations`] over a single violation.
pub fn field_validation(member: &str, constraint: String) -> ValidationException {
    constraint_violations(&[(member, constraint)])
}

/// The modelled `ValidationException` (HTTP 400) for the member constraints one request
/// violated, each given as `(member path, the constraint that member's value failed)`.
///
/// The message is built from the REQUEST and the model, in the framing a constraint
/// violation is reported in: how many violations were detected, then one clause per
/// violation naming the member path and the constraint it failed, joined by `; `, so a
/// client that parses the sentence can count the violations and extract the offending
/// member. The modelled members carry the same detail structurally — reason
/// `fieldValidationFailed` (a `ValidationExceptionReason` member) and one `fieldList`
/// entry per violation — so a client can fix the request without parsing it at all.
pub fn constraint_violations(violations: &[(&str, String)]) -> ValidationException {
    let clauses: Vec<String> = violations
        .iter()
        .map(|(member, constraint)| {
            format!("Value at '{member}' failed to satisfy constraint: {constraint}")
        })
        .collect();
    ValidationException {
        message: format!(
            "{} validation error(s) detected: {}",
            clauses.len(),
            clauses.join("; ")
        ),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|(member, constraint)| ValidationExceptionField {
                    name: (*member).to_string(),
                    message: constraint.clone(),
                })
                .collect(),
        ),
    }
}

/// One of the resource kinds this service refuses a request about: the `resourceType`
/// member its errors carry, the noun the service's own error sentences name it by, and how
/// it names the kind of IDENTIFIER a request supplies for such a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceKind {
    /// The `resourceType` member of `ConflictException`, and the segment that names this
    /// kind of resource in a PCS ARN ([`arn_resource`]).
    pub resource_type: &'static str,
    /// How an error sentence names this kind of resource.
    pub noun: &'static str,
    /// How the service names the kind of identifier a request supplies for this resource,
    /// which is the `resourceType` an identifier-keyed not-found reports ([`not_found`]).
    pub identifier_label: &'static str,
}

/// The cluster kind.
pub const CLUSTER: ResourceKind = ResourceKind {
    resource_type: "cluster",
    noun: "cluster",
    identifier_label: "Cluster ID",
};

/// The queue kind.
pub const QUEUE: ResourceKind = ResourceKind {
    resource_type: "queue",
    noun: "queue",
    identifier_label: "Queue ID",
};

/// The compute node group kind.
pub const COMPUTE_NODE_GROUP: ResourceKind = ResourceKind {
    resource_type: "computenodegroup",
    noun: "compute node group",
    identifier_label: "Compute Node Group ID",
};

/// The kind an ARN names when its resource part is none of the three above: the service
/// defines no fourth type, so nothing about such an ARN claims a type PCS does not have
/// ([`arn_resource`]).
pub const UNNAMED_RESOURCE: ResourceKind = ResourceKind {
    resource_type: "resource",
    noun: "resource",
    identifier_label: "Resource ID",
};

/// The modelled `ResourceNotFoundException` (HTTP 404) for a resource a request named by
/// IDENTIFIER — a `clusterIdentifier`, `queueIdentifier` or `computeNodeGroupIdentifier` —
/// that the service cannot find.
///
/// The message is the service's own FIXED sentence for that kind of resource: it names
/// the kind and nothing else, so the identifier the caller supplied is not echoed back in
/// it. The modelled members are where the request's own values belong: `resourceId`
/// carries the identifier as supplied, and `resourceType` the kind of identifier that did
/// not resolve ([`ResourceKind::identifier_label`]).
pub fn not_found(kind: ResourceKind, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("AWS PCS couldn't find the {} you specified.", kind.noun),
        resource_id: identifier.to_string(),
        resource_type: kind.identifier_label.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for an ARN — the key the tagging
/// operations take — that names no resource this service holds.
///
/// The ARN-keyed path states the service's own fixed sentence for a resource that cannot
/// be found and names NEITHER the resource nor its kind: the ARN the caller sent carries
/// both already, so the refusal adds nothing by repeating them, where the
/// identifier-keyed path ([`not_found`]) reports both because an identifier carries
/// neither. The two members are modelled `@required`, so they travel empty rather than
/// absent.
pub fn arn_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "Resource cannot be found.".to_string(),
        resource_id: String::new(),
        resource_type: String::new(),
    }
}

/// The modelled `ConflictException` (HTTP 409) for an operation that needs a cluster in
/// [`STATUS_ACTIVE`] while the cluster is in another status.
///
/// The message is the service's own fixed sentence for the conflict (the model lists it
/// among `ConflictException`'s examples); the cluster is named by `resourceId`.
pub fn cluster_not_active(cluster_id: &str) -> ConflictException {
    ConflictException {
        message: "A cluster isn't in ACTIVE status.".to_string(),
        resource_id: cluster_id.to_string(),
        resource_type: CLUSTER.resource_type.to_string(),
    }
}

/// The modelled `ConflictException` (HTTP 409) for an operation on a resource of `kind`
/// that is not in [`STATUS_ACTIVE`] — the child-resource counterpart of
/// [`cluster_not_active`], worded the same way the service words that one and naming the
/// resource by `resourceId` rather than in the sentence.
pub fn not_active(kind: ResourceKind, resource_id: &str) -> ConflictException {
    if kind == CLUSTER {
        return cluster_not_active(resource_id);
    }
    ConflictException {
        message: format!("A {} isn't in {STATUS_ACTIVE} status.", kind.noun),
        resource_id: resource_id.to_string(),
        resource_type: kind.resource_type.to_string(),
    }
}

/// The modelled `ConflictException` (HTTP 409) for deleting a compute node group a queue
/// of its cluster still assigns jobs to.
///
/// "You must delete all queues associated with the compute node group first"
/// (`DeleteComputeNodeGroup`'s own documentation), which is the unstable state the model
/// names among `ConflictException`'s examples. The sentence is the service's own FIXED
/// text for that conflict and echoes no caller-supplied value: the group is named by
/// `resourceId`, the way every other refusal of this service names its resource.
pub fn compute_node_group_still_referenced(group_id: &str) -> ConflictException {
    ConflictException {
        message: "AWS PCS can't delete the compute node group you specified because the \
                  cluster has associated queues."
            .to_string(),
        resource_id: group_id.to_string(),
        resource_type: COMPUTE_NODE_GROUP.resource_type.to_string(),
    }
}

/// The modelled `ConflictException` (HTTP 409) for deleting a cluster that still holds a
/// queue or a compute node group.
///
/// "You must delete all queues and compute node groups associated with the cluster before
/// you can delete the cluster" (`DeleteCluster`'s own documentation) — the "unstable
/// state" the model names among `ConflictException`'s examples. The service's own FIXED
/// sentence for it, echoing no caller-supplied value: the cluster is named by
/// `resourceId`, as in [`compute_node_group_still_referenced`].
pub fn cluster_still_has_children(cluster_id: &str) -> ConflictException {
    ConflictException {
        message: "AWS PCS can't delete the cluster you specified because it has associated \
                  queues or compute node groups."
            .to_string(),
        resource_id: cluster_id.to_string(),
        resource_type: CLUSTER.resource_type.to_string(),
    }
}

/// The constraint clause a caller-sized list a create would PERSIST failed, or `None`
/// when the service accepts it.
///
/// The model bounds none of these lists, so this is the ceiling that keeps one request
/// from sizing one stored record arbitrarily — see [`MAX_STORED_LIST_ENTRIES`]. The clause
/// is the one every length ceiling of this service is stated in
/// ([`length_at_most_constraint`]); the caller hands it to [`field_validation`] with the
/// member it bounds.
pub fn stored_list_violation(entries: usize) -> Option<String> {
    (entries > MAX_STORED_LIST_ENTRIES).then(|| length_at_most_constraint(MAX_STORED_LIST_ENTRIES))
}

/// The modelled `ValidationException` (HTTP 400) `CreateCluster` refuses a scheduler
/// version with, or `None` when the service carries on with it.
///
/// THE RULE both paths share: `version` is a plain `String` in the model — no enum, no
/// `@pattern`, no `@length` — so the only thing that holds across releases is that the value
/// LABELS a scheduler release ([`names_a_scheduler_release`]). The "Valid Values" its
/// documentation prints are prose that moves as Slurm releases ship and are deliberately NOT
/// a gate: a request naming a release outside the list of the day is one the service carries
/// on with, and refusing it here would turn a working create into a terminal refusal. So the
/// check is a SHAPE check and nothing more, never the set of releases the service can
/// provision, which no part of the model states.
///
/// THE WORDING is this path's own: the create path names the scheduler type and the version
/// the request asked for, both taken from the request, while the update path answers a fixed
/// sentence naming neither ([`update_scheduler_version_violation`]) — each path has its own
/// wording, which is why these are two functions over one rule.
pub fn create_scheduler_version_violation(
    scheduler_type: &str,
    version: &str,
) -> Option<ValidationException> {
    if names_a_scheduler_release(version) {
        return None;
    }
    Some(service_validation(format!(
        "Scheduler version not found: type={scheduler_type}, version={version}"
    )))
}

/// The modelled `ValidationException` (HTTP 400) `UpdateCluster` refuses a scheduler version
/// with, or `None` when the service carries on with it.
///
/// The same rule as [`create_scheduler_version_violation`] — the value must label a scheduler
/// release, and the printed "Valid Values" are not a gate — answered in the update path's own
/// fixed sentence, which names no type and no version.
pub fn update_scheduler_version_violation(version: &str) -> Option<ValidationException> {
    if names_a_scheduler_release(version) {
        return None;
    }
    Some(service_validation(
        "AWS PCS doesn't support the scheduler version you provided.".to_string(),
    ))
}

/// Whether `version` labels a scheduler release: [`SCHEDULER_VERSION_COMPONENTS`]
/// dot-separated components, each one to [`SCHEDULER_VERSION_COMPONENT_DIGITS`] ASCII
/// digits.
fn names_a_scheduler_release(version: &str) -> bool {
    let components: Vec<&str> = version.split('.').collect();
    SCHEDULER_VERSION_COMPONENTS.contains(&components.len())
        && components.iter().all(|component| {
            (1..=SCHEDULER_VERSION_COMPONENT_DIGITS).contains(&component.len())
                && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// The modelled `ValidationException` for a request the SERVICE refuses by a rule the
/// model does not express as a member constraint: reason `other` (a
/// `ValidationExceptionReason` member) and no `fieldList`, which is for the
/// member-by-member refusals [`field_validation`] builds.
fn service_validation(message: String) -> ValidationException {
    ValidationException {
        message,
        reason: "other".to_string(),
        field_list: None,
    }
}

/// The stored Slurm configuration an `UpdateCluster` leaves behind: a member the request
/// carries replaces the stored one, a member it omits leaves the stored one alone.
pub fn apply_slurm_update(
    stored: Option<ClusterSlurmConfiguration>,
    update: UpdateClusterSlurmConfigurationRequest,
) -> ClusterSlurmConfiguration {
    let mut merged = stored.unwrap_or_default();
    if let Some(seconds) = update.scale_down_idle_time_in_seconds {
        merged.scale_down_idle_time_in_seconds = Some(seconds);
    }
    if let Some(settings) = update.slurm_custom_settings {
        merged.slurm_custom_settings = Some(settings);
    }
    if let Some(settings) = update.slurmdbd_custom_settings {
        merged.slurmdbd_custom_settings = Some(settings);
    }
    if let Some(settings) = update.cgroup_custom_settings {
        merged.cgroup_custom_settings = Some(settings);
    }
    if let Some(accounting) = update.accounting {
        let previous = merged.accounting.take();
        merged.accounting = Some(Accounting {
            default_purge_time_in_days: accounting
                .default_purge_time_in_days
                .or_else(|| previous.as_ref().and_then(|a| a.default_purge_time_in_days)),
            // "The default value for mode is NONE" (`AccountingRequest$mode`).
            mode: accounting
                .mode
                .or_else(|| previous.map(|previous| previous.mode))
                .unwrap_or_else(|| MODE_NONE.to_string()),
        });
    }
    if let Some(slurm_rest) = update.slurm_rest {
        let previous = merged.slurm_rest.take();
        merged.slurm_rest = Some(SlurmRest {
            // "The default value for mode is NONE" (`SlurmRestRequest$mode`).
            mode: slurm_rest
                .mode
                .or_else(|| previous.map(|previous| previous.mode))
                .unwrap_or_else(|| MODE_NONE.to_string()),
        });
    }
    merged
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would take
/// a store past its ceiling. `quota_code` is left absent rather than invented: the
/// emulator stands in for the quota, it does not reproduce Service Quotas' codes.
pub fn quota_exceeded(resource_type: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "the maximum number of {resource_type} resources ({limit}) has been reached"
        ),
        service_code: "pcs".to_string(),
        resource_id: None,
        resource_type: Some(resource_type.to_string()),
        quota_code: None,
    }
}

/// The resource kind a PCS ARN names, and the resource id at its tail.
///
/// A PCS ARN's resource part is `cluster/<id>`, `cluster/<id>/computenodegroup/<id>` or
/// `cluster/<id>/queue/<id>` (see [`cluster_arn`], [`compute_node_group_arn`] and
/// [`queue_arn`]), so the kind is the segment ahead of the trailing id. Read from the
/// REQUEST, not from state, so an ARN that names nothing still reports what it named.
pub fn arn_resource(arn: &str) -> (ResourceKind, String) {
    let resource_part = arn.splitn(6, ':').nth(5).unwrap_or(arn);
    let mut segments = resource_part.rsplit('/');
    let resource_id = segments.next().unwrap_or_default();
    let kind = match segments.next().unwrap_or_default() {
        segment if segment == CLUSTER.resource_type => CLUSTER,
        segment if segment == QUEUE.resource_type => QUEUE,
        segment if segment == COMPUTE_NODE_GROUP.resource_type => COMPUTE_NODE_GROUP,
        _ => UNNAMED_RESOURCE,
    };
    (kind, resource_id.to_string())
}

/// The id of the cluster a PCS ARN belongs to: the segment after `cluster/` in its
/// resource part, which every PCS ARN carries because a compute node group and a queue
/// are both nested in a cluster ([`compute_node_group_arn`], [`queue_arn`]). `None` when
/// the resource part does not start with the `cluster` segment, which no ARN the service
/// mints can.
///
/// What makes an ARN-keyed lookup direct: the cluster the ARN names is the entry of
/// [`ComAmazonawsPcsAwsparallelcomputingserviceState::clusters`] under this id.
pub fn arn_cluster_id(arn: &str) -> Option<&str> {
    let resource_part = arn.splitn(6, ':').nth(5)?;
    let mut segments = resource_part.split('/');
    if segments.next()? != CLUSTER.resource_type {
        return None;
    }
    segments.next()
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a tag set past
/// [`MAX_TAGS_PER_RESOURCE`] — the ceiling every path that writes tags checks, a create
/// carrying `tags` as much as a later `TagResource`.
///
/// AWS states the resulting-set rule and the ceiling, and names neither the request's
/// keys nor the resource, so the only variable part of the sentence is the ceiling
/// itself. `quota_code` is left absent rather than invented: the emulator stands in for
/// the quota, it does not reproduce Service Quotas' codes.
pub fn tag_quota_exceeded(kind: ResourceKind, resource_id: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags."
        ),
        service_code: "pcs".to_string(),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(kind.resource_type.to_string()),
        quota_code: None,
    }
}

/// How many tags a resource holding `stored` would hold once `requested` is merged in: a
/// key the resource already carries is EDITED, not added, so only new keys grow the set.
pub fn tag_count_after_merge(
    stored: &HashMap<String, String>,
    requested: &HashMap<String, String>,
) -> usize {
    stored.len()
        + requested
            .keys()
            .filter(|key| !stored.contains_key(*key))
            .count()
}

/// The page size a list serves for this `maxResults`.
///
/// `MaxResults` is `@default(10)` with `@range(min: 1, max: 100)`: the boundary refuses
/// anything outside the range, and a request that omits the member asks for the default,
/// which is also what a zero (an absent member that did not pick the default up) means.
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(size @ 1..=100) => size,
        _ => DEFAULT_PAGE_SIZE,
    }
}

/// The listing kind [`ListClusters`](crate::ops::list_clusters)' page tokens carry.
pub const PAGE_KIND_CLUSTERS: &str = "clusters";

/// The listing kind [`ListQueues`](crate::ops::list_queues)' page tokens carry, for one
/// cluster. Scoped by cluster id, so a token issued for one cluster's queues cannot
/// resume another cluster's.
pub fn page_kind_queues(cluster_id: &str) -> String {
    format!("queues:{cluster_id}")
}

/// The listing kind
/// [`ListComputeNodeGroups`](crate::ops::list_compute_node_groups)' page tokens carry, for
/// one cluster — scoped the same way [`page_kind_queues`] is, so no token resumes another
/// cluster's groups or another listing of the same cluster.
pub fn page_kind_compute_node_groups(cluster_id: &str) -> String {
    format!("computenodegroups:{cluster_id}")
}

/// The creation sequence a `nextToken` for the listing `kind` resumes after, or `None`
/// when the token is not one THAT listing issued.
///
/// The kind is part of the token because the creation sequence is service-wide: a bare
/// sequence would decode in every listing, so a token `ListClusters` handed out would
/// silently resume a `ListQueues` page (and one cluster's token another cluster's
/// queues) instead of being refused.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded.strip_prefix(&format!("{kind}#"))?.parse().ok()
}

/// The opaque `nextToken` that resumes a listing of `kind` after sequence `seq`.
fn encode_page_token(kind: &str, seq: u64) -> String {
    hex::encode(format!("{kind}#{seq}"))
}

/// One page of a creation-ordered list.
///
/// `kind` is the listing the token belongs to (see [`decode_page_token`]), `rows` every
/// candidate paired with its creation sequence, `after` the sequence the previous page
/// ended on, and `size` the page size. The returned token resumes after the last row of
/// this page and is absent on the final page.
pub fn paginate<T>(
    kind: &str,
    mut rows: Vec<(u64, T)>,
    after: Option<u64>,
    size: usize,
) -> (Vec<T>, Option<String>) {
    rows.sort_by_key(|(seq, _)| *seq);
    let mut remaining = rows
        .into_iter()
        .filter(|(seq, _)| after.is_none_or(|last| *seq > last));
    let page: Vec<(u64, T)> = remaining.by_ref().take(size).collect();
    let token = match (remaining.next(), page.last()) {
        (Some(_), Some((seq, _))) => Some(encode_page_token(kind, *seq)),
        _ => None,
    };
    (page.into_iter().map(|(_, row)| row).collect(), token)
}

impl From<SchedulerRequest> for Scheduler {
    fn from(request: SchedulerRequest) -> Self {
        Self {
            r#type: request.r#type,
            version: request.version,
        }
    }
}

impl From<NetworkingRequest> for Networking {
    fn from(request: NetworkingRequest) -> Self {
        Self {
            subnet_ids: request.subnet_ids,
            security_group_ids: request.security_group_ids,
            network_type: request.network_type,
        }
    }
}

impl From<AccountingRequest> for Accounting {
    fn from(request: AccountingRequest) -> Self {
        Self {
            default_purge_time_in_days: request.default_purge_time_in_days,
            mode: request.mode,
        }
    }
}

impl From<SlurmRestRequest> for SlurmRest {
    fn from(request: SlurmRestRequest) -> Self {
        Self { mode: request.mode }
    }
}

impl From<ClusterSlurmConfigurationRequest> for ClusterSlurmConfiguration {
    fn from(request: ClusterSlurmConfigurationRequest) -> Self {
        Self {
            scale_down_idle_time_in_seconds: request.scale_down_idle_time_in_seconds,
            slurm_custom_settings: request.slurm_custom_settings,
            slurmdbd_custom_settings: request.slurmdbd_custom_settings,
            cgroup_custom_settings: request.cgroup_custom_settings,
            // PCS reports the Secrets Manager secret it generated for the Slurm auth key
            // and for JWT auth. The emulator generates no secret, so it reports neither
            // rather than inventing an ARN.
            auth_key: None,
            jwt_auth: None,
            accounting: request.accounting.map(Accounting::from),
            slurm_rest: request.slurm_rest.map(SlurmRest::from),
        }
    }
}

impl From<ScalingConfigurationRequest> for ScalingConfiguration {
    fn from(request: ScalingConfigurationRequest) -> Self {
        Self {
            min_instance_count: request.min_instance_count,
            max_instance_count: request.max_instance_count,
        }
    }
}

impl From<ComputeNodeGroupSlurmConfigurationRequest> for ComputeNodeGroupSlurmConfiguration {
    fn from(request: ComputeNodeGroupSlurmConfigurationRequest) -> Self {
        Self {
            scale_down_idle_time_in_seconds: request.scale_down_idle_time_in_seconds,
            slurm_custom_settings: request.slurm_custom_settings,
            gres_custom_settings: request.gres_custom_settings,
        }
    }
}

impl From<NodeLifecycleActionsRequest> for NodeLifecycleActions {
    fn from(request: NodeLifecycleActionsRequest) -> Self {
        Self {
            stages: request.stages,
            script_caching_policy: request.script_caching_policy,
        }
    }
}

impl From<QueueSlurmConfigurationRequest> for QueueSlurmConfiguration {
    fn from(request: QueueSlurmConfigurationRequest) -> Self {
        Self {
            slurm_custom_settings: request.slurm_custom_settings,
        }
    }
}
