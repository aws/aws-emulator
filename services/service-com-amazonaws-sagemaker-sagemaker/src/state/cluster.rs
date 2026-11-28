// SPDX-License-Identifier: CC0-1.0

//! The HyperPod cluster slice of this service's state.
//!
//! One file for the whole slice: the [`Cluster`] record `CreateCluster` writes with its
//! quota, the instant its clock-owed transitions are due from ([`transition_owed_at`])
//! and its id minting, and what a reader needs on top of it:
//!
//! * `ComAmazonawsSagemakerSagemakerState::cluster_key`, the one lookup that resolves
//!   the `ClusterNameOrArn` every cluster operation takes to the store key, so no
//!   handler re-implements it;
//! * the `*Details` / `*Output` projections `DescribeCluster` returns, built from the
//!   instance-group SPECIFICATIONS the create stored and the cluster's current status —
//!   so one stored record answers both the "what you asked for" and the "what is
//!   realised" halves of each detail shape, and a later reader cannot disagree with an
//!   earlier one.
//!
//! The module sits under `src/state/` because it holds and projects state, and is
//! declared in `src/state.rs` beside the other state slices of this service.
//!
//! No PROJECTION here writes: the `Creating` -> `InService` transition is
//! [`Cluster::settle_status`]'s, materialising only what the clock already owed.

use super::*;
use crate::state::{ComAmazonawsSagemakerSagemakerState, STATUS_IN_SERVICE};
use std::time::SystemTime;
use crate::ops::types::{
    ClusterAutoPatchConfigDetails, ClusterAutoScalingConfigOutput, ClusterInstanceGroupDetails,
    ClusterInstanceRequirementDetails, ClusterKubernetesConfigDetails,
    ClusterNetworkInterfaceDetails, ClusterPatchScheduleDetails,
    ClusterRestrictedInstanceGroupDetails, ClusterRestrictedInstanceGroupsConfigOutput,
    ClusterSharedEnvironmentConfigDetails, ClusterSlurmConfigDetails, EnvironmentConfigDetails,
};

/// `value`, but only once the cluster has reached `InService`.
///
/// Every `Current*` member of a `*Details` shape reports what is REALISED, against the
/// `Desired*` member that reports what the request declared ("you describe what you want
/// ... and the cluster works to reconcile the actual state with the declared state", per
/// `ClusterInstanceGroupSpecification$KubernetesConfig`). Nothing is realised while the
/// cluster is still provisioning, so every such member is absent until then.
fn realised<T>(in_service: bool, value: Option<T>) -> Option<T> {
    if in_service { value } else { None }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The store key of the cluster a `ClusterNameOrArn` names, or `None` when no
    /// cluster answers to it.
    ///
    /// Every cluster operation takes `ClusterNameOrArn` ("the string name or the Amazon
    /// Resource Name (ARN)"), so both forms resolve to the one record here rather than
    /// in each handler. A `ClusterName` cannot contain `:`, so the two forms never
    /// collide. The ARN form is matched in FULL rather than through
    /// `crate::state::name_from_arn_or_name`, because a cluster ARN's last segment is
    /// the minted id and not the store key — so an ARN naming another region, account or
    /// partition does not resolve here.
    ///
    /// A cluster whose deletion the clock already completed resolves to nothing, as it
    /// does from every other cluster lookup: past that instant the cluster is gone
    /// whether or not the record has been released yet ([`Cluster::is_released`]).
    pub fn cluster_key(&self, name_or_arn: &str) -> Option<String> {
        let now = SystemTime::now();
        self.clusters
            .get(name_or_arn)
            .or_else(|| {
                self.clusters
                    .values()
                    .find(|cluster| cluster.arn == name_or_arn)
            })
            .filter(|cluster| !cluster.is_released(now))
            .map(|cluster| cluster.name.clone())
    }
}

impl Cluster {
    /// Whether the cluster has finished provisioning, i.e. its instances are realised.
    pub fn is_in_service(&self) -> bool {
        self.status == STATUS_IN_SERVICE
    }

    /// The stored instance-group specifications as the `ClusterInstanceGroupDetails`
    /// every reader returns, in the order the request listed them.
    pub fn instance_group_details(&self) -> Vec<ClusterInstanceGroupDetails> {
        let in_service = self.is_in_service();
        self.instance_groups
            .iter()
            .map(|spec| ClusterInstanceGroupDetails {
                // The instances exist only once provisioning is done; the target is
                // what the request asked for, from the moment it was accepted. A count
                // realises as 0 rather than through `realised()`: `CurrentCount` is "the
                // number of instances that are currently in the instance group", and a
                // provisioning group genuinely has none, where an absent member would
                // read as "unknown".
                current_count: Some(if in_service { spec.instance_count } else { 0 }),
                target_count: Some(spec.instance_count),
                min_count: spec.min_instance_count,
                instance_group_name: Some(spec.instance_group_name.clone()),
                instance_type: spec.instance_type.clone(),
                instance_requirements: spec.instance_requirements.as_ref().map(|requirements| {
                    ClusterInstanceRequirementDetails {
                        current_instance_types: realised(
                            in_service,
                            Some(requirements.instance_types.clone()),
                        ),
                        desired_instance_types: Some(requirements.instance_types.clone()),
                    }
                }),
                // Reported only for a group whose instance types the service itself
                // chose per instance; this emulator provisions exactly what was asked
                // for, so there is no second breakdown to report.
                instance_type_details: None,
                life_cycle_config: spec.life_cycle_config.clone(),
                execution_role: Some(spec.execution_role.clone()),
                threads_per_core: spec.threads_per_core,
                instance_storage_configs: spec.instance_storage_configs.clone(),
                on_start_deep_health_checks: spec.on_start_deep_health_checks.clone(),
                // A group follows its cluster: `InstanceGroupStatus` spells both
                // members exactly as `ClusterStatus` does.
                status: Some(self.status.clone()),
                training_plan_arn: spec.training_plan_arn.clone(),
                // This emulator models no training plan, so a group that names one has
                // no plan status to report.
                training_plan_status: None,
                override_vpc_config: spec.override_vpc_config.clone(),
                scheduled_update_config: spec.scheduled_update_config.clone(),
                auto_patch_config: spec.auto_patch_config.as_ref().map(|config| {
                    let schedule =
                        config
                            .patch_schedule
                            .as_ref()
                            .map(|schedule| ClusterPatchScheduleDetails {
                                next_patch_date: schedule.next_patch_date,
                            });
                    ClusterAutoPatchConfigDetails {
                        patching_strategy: Some(config.patching_strategy.clone()),
                        current_patch_schedule: realised(in_service, schedule.clone()),
                        desired_patch_schedule: schedule,
                        deployment_config: config.deployment_config.clone(),
                    }
                }),
                current_image_id: realised(in_service, spec.image_id.clone()),
                desired_image_id: spec.image_id.clone(),
                current_image_release_version: realised(
                    in_service,
                    spec.image_release_version.clone(),
                ),
                desired_image_release_version: spec.image_release_version.clone(),
                // Nothing in this emulator publishes a newer AMI than the one a group
                // was created with, so there is no version comparison to report.
                image_version_status: None,
                // Set only while an operation (a scaling one, say) is in flight on the
                // group; nothing here runs one.
                active_operations: None,
                kubernetes_config: spec.kubernetes_config.as_ref().map(|config| {
                    ClusterKubernetesConfigDetails {
                        current_labels: realised(in_service, config.labels.clone()),
                        desired_labels: config.labels.clone(),
                        current_taints: realised(in_service, config.taints.clone()),
                        desired_taints: config.taints.clone(),
                    }
                }),
                capacity_requirements: spec.capacity_requirements.clone(),
                // Both belong to a software update; this cluster has never had one
                // requested, so neither has a value yet.
                target_state_count: None,
                software_update_status: None,
                active_software_update_config: None,
                slurm_config: spec
                    .slurm_config
                    .as_ref()
                    .map(|config| ClusterSlurmConfigDetails {
                        node_type: config.node_type.clone(),
                        partition_names: config.partition_names.clone(),
                    }),
                network_interface: spec.network_interface.as_ref().map(|interface| {
                    ClusterNetworkInterfaceDetails {
                        interface_type: interface.interface_type.clone(),
                    }
                }),
            })
            .collect()
    }

    /// The stored restricted instance groups as the details shape readers return.
    pub fn restricted_instance_group_details(&self) -> Vec<ClusterRestrictedInstanceGroupDetails> {
        let in_service = self.is_in_service();
        self.restricted_instance_groups
            .iter()
            .map(|spec| ClusterRestrictedInstanceGroupDetails {
                // Counts realise as 0 while provisioning, for the reason given on
                // `Cluster::instance_group_details`.
                current_count: Some(if in_service { spec.instance_count } else { 0 }),
                target_count: Some(spec.instance_count),
                instance_group_name: Some(spec.instance_group_name.clone()),
                instance_type: Some(spec.instance_type.clone()),
                execution_role: Some(spec.execution_role.clone()),
                threads_per_core: spec.threads_per_core,
                instance_storage_configs: spec.instance_storage_configs.clone(),
                on_start_deep_health_checks: spec.on_start_deep_health_checks.clone(),
                status: Some(self.status.clone()),
                training_plan_arn: spec.training_plan_arn.clone(),
                training_plan_status: None,
                override_vpc_config: spec.override_vpc_config.clone(),
                scheduled_update_config: spec.scheduled_update_config.clone(),
                environment_config: spec.environment_config.as_ref().map(|config| {
                    EnvironmentConfigDetails {
                        f_sx_lustre_config: config.f_sx_lustre_config.clone(),
                        // The RIG's output path is chosen by the service when the
                        // environment is materialised; this emulator runs no
                        // environment, so it reports none.
                        s3_output_path: None,
                    }
                }),
            })
            .collect()
    }

    /// The stored restricted-instance-group configuration as the output shape readers
    /// return, split into the declared and the realised halves.
    pub fn restricted_instance_groups_config_output(
        &self,
    ) -> Option<ClusterRestrictedInstanceGroupsConfigOutput> {
        let in_service = self.is_in_service();
        self.restricted_instance_groups_config
            .as_ref()
            .map(|config| ClusterRestrictedInstanceGroupsConfigOutput {
                shared_environment_config: ClusterSharedEnvironmentConfigDetails {
                    current_f_sx_lustre_config: realised(
                        in_service,
                        Some(config.shared_environment_config.f_sx_lustre_config.clone()),
                    ),
                    desired_f_sx_lustre_config: Some(
                        config.shared_environment_config.f_sx_lustre_config.clone(),
                    ),
                    current_f_sx_lustre_deletion_policy: realised(
                        in_service,
                        Some(
                            config
                                .shared_environment_config
                                .f_sx_lustre_deletion_policy
                                .clone(),
                        ),
                    ),
                    desired_f_sx_lustre_deletion_policy: Some(
                        config
                            .shared_environment_config
                            .f_sx_lustre_deletion_policy
                            .clone(),
                    ),
                },
            })
    }

    /// The stored autoscaling configuration as the output shape readers return. Its
    /// `Status` follows the cluster: `ClusterAutoScalingStatus` spells `Creating` and
    /// `InService` exactly as `ClusterStatus` does, and the autoscaler comes up with
    /// the cluster it scales.
    pub fn auto_scaling_output(&self) -> Option<ClusterAutoScalingConfigOutput> {
        self.auto_scaling
            .as_ref()
            .map(|config| ClusterAutoScalingConfigOutput {
                mode: config.mode.clone(),
                auto_scaler_type: config.auto_scaler_type.clone(),
                status: self.status.clone(),
                // Nothing in this emulator fails an autoscaler, so there is no failure
                // to report; the cluster's own `FailureMessage` is absent for the same
                // reason.
                failure_message: None,
            })
    }
}

/// The instant a transition this service's clock owes is due from, where the call that STARTS
/// it has already decided its outcome: the instant that call answers.
///
/// The HyperPod cluster lifecycle a client drives is three such transitions, and each is
/// DECIDED by the call that starts it: `CreateCluster` admits the cluster, so its
/// `Creating` -> `InService` (with the instances its groups declare) is settled;
/// `BatchAddClusterNodes` admits the nodes, so each added node's `Pending` -> `Running`
/// (with the instance id that comes with it) is settled; `DeleteCluster` admits the
/// teardown, so the cluster's removal is settled. An Inference Recommender job's single move
/// out of `PENDING` is one too, for the same reason:
/// `CreateInferenceRecommendationsJob` performs the job's own benchmark and records what it
/// measured before it answers ([`crate::state::recommendation_job_settles_at`]). Nothing is
/// left to compute
/// afterwards, so the emulator interposes no span of its own between deciding the outcome and
/// letting a reader observe it: the transition is owed from the instant that call answers, and
/// the FIRST request after it reports the settled resource. A client driving the resource
/// with the documented poll-until-settled loop therefore observes each transition on its
/// next probe instead of paying one whole polling interval — an interval it sizes against
/// the many minutes real provisioning takes — so a workflow that chains several
/// of these waits converges as fast as the emulator can answer instead of outliving its
/// own deadline.
///
/// The transient state stays the answer of the call that starts it: `CreateCluster`
/// stores `Creating`, `BatchAddClusterNodes` reports every node it added `Pending`, and
/// `DeleteCluster` leaves the cluster `Deleting`. The move out of it is materialised by
/// the settle pass of the next request ([`Cluster::settle_status`],
/// [`crate::state::ClusterNode::settle_status`]), which writes only what a stored instant
/// already in the past had made due — so nothing sleeps, no reader decides anything, and
/// a transition owed during one request is never materialised by that same request,
/// because every handler settles what it touches BEFORE it owes anything of its own.
///
/// What that costs, stated plainly: since the owed instant is this instant and not a later
/// one, a cluster's `Creating` and `Deleting`, an added node's `Pending` and a recommendation
/// job's `PENDING` are reported
/// only by the call that stores them, so no SEQUENCE of requests observes them — a reader
/// always answers the settled resource. That is the only way to spend none of a client's
/// polling interval: a transition still owed when the next probe arrives costs that probe
/// its whole interval however small the remaining span is, and bringing one forward
/// BECAUSE something read it would make a reporting call decide a status, which is the one
/// thing it may never do. This function is therefore deliberately the IDENTITY and hides
/// no arithmetic; it exists so the call sites name this rule rather than restate it,
/// and so changing when such a transition falls due is one edit. A test that
/// needs one of those statuses sets the stored instant itself.
///
/// A resource whose work is NOT over when the accepting call answers keeps a span of its own:
/// a node OPERATION — a reboot, a replacement, a volume attachment — has
/// [`crate::state::TRANSIENT_STATE_SETTLES_AFTER`], and a training job's run is reported stage
/// by stage ([`crate::state::TRAINING_JOB_PHASE_SPAN`]), each of which says why.
#[inline]
pub fn transition_owed_at(now: SystemTime) -> SystemTime {
    now
}

/// `ClusterStatus`'s terminal failure member. Nothing in this emulator fails a cluster,
/// so no record carries it — it is named because a delete is admitted for a cluster in
/// it as well as for one `InService` ([`Cluster::accepts_deletion`]), and a handler must
/// not compare a bare string literal.
pub const CLUSTER_STATUS_FAILED: &str = "Failed";

/// See [`MODEL_QUOTA`]; bounds the cluster store, and is released by `DeleteCluster`.
pub const CLUSTER_QUOTA: usize = 1_000;

/// How every node operation states that the cluster it was asked to change cannot take
/// the change: the cluster's ARN and the fact that it is on its way out.
///
/// Stated once here so `BatchAddClusterNodes`, `BatchDeleteClusterNodes`,
/// `BatchReplaceClusterNodes` and `AttachClusterNodeVolume` cannot word the one fact
/// differently. The condition is a cluster whose TERMINATION has begun
/// ([`Cluster::accepts_node_changes`]) — the status itself is not interpolated, because
/// that is the one status the sentence is about. None of these operations models an error
/// for it — each models `ResourceNotFound`, which a cluster a describe still reports is
/// not — so they answer the 400 `ValidationException` the refusal channel provides.
pub fn cluster_rejects_node_change(arn: &str) -> String {
    format!("Cluster {arn} is in a termination state that cannot be modified.")
}

/// How a node-STORAGE operation states that the cluster it was asked to act on cannot take a
/// volume change, because it is not orchestrated by Amazon EKS: both of them document "Your
/// cluster must use EKS as the orchestration and be in the `InService` state" on `ClusterArn`,
/// and a HyperPod cluster that is not EKS-orchestrated is one the Slurm workload manager runs.
///
/// RECORDED wording, entirely fixed text with no variable part: the refusal is about the
/// CLASS of cluster rather than about any one cluster or request member, so nothing is
/// interpolated into it. Stated once here so `AttachClusterNodeVolume` and
/// `DetachClusterNodeVolume` cannot word the one precondition differently. Neither models an
/// error for it — each models `ResourceNotFound`, which a cluster a describe still reports is
/// not — so the refusal travels as the 400 `ValidationException` the refusal channel
/// provides, like the `InService` half beside it.
pub const VOLUME_OPERATIONS_NOT_SUPPORTED: &str =
    "Volume operations are not supported for Slurm clusters";

/// Why the `requested` instances a cluster's groups ask for are more than SageMaker
/// provisions for ONE cluster, or `None` when the cluster fits.
///
/// Asked by the CREATE, where the cluster holds no node yet, so the utilisation the
/// sentence states is zero; a later addition to a cluster that already holds nodes is
/// weighed against the same bound by `BatchAddClusterNodes`, which states the cluster it
/// is about.
///
/// A created cluster's groups are brought up to the instance counts they declare
/// ([`Cluster::provision_group_instances`]), and the nodes of one cluster are bounded by
/// [`CLUSTER_NODE_QUOTA`] — the same bound `BatchAddClusterNodes` holds a later addition
/// to. So a create asking for more is refused with the modelled `ResourceLimitExceeded`
/// rather than accepted and silently provisioned short: the counts a reader reports and
/// the nodes a cluster holds cannot disagree.
///
/// The sentence is the one every other bound this emulator owns states (see
/// [`MODEL_QUOTA`]): the bound, what is already held and what the request asks on top.
pub fn cluster_node_limit_refusal(requested: i64) -> Option<String> {
    let limit = i64::try_from(CLUSTER_NODE_QUOTA).unwrap_or(i64::MAX);
    (requested > limit).then(|| {
        format!(
            "The service limit for nodes in a HyperPod cluster is {limit}, with current \
             utilization of 0 and a request delta of {requested}."
        )
    })
}

/// The node provisioning mode a HyperPod cluster runs in when the create named none.
///
/// `DescribeCluster` reports "the mode used for provisioning nodes in the cluster"
/// (`DescribeClusterResponse$NodeProvisioningMode`) for every cluster, not only for one
/// whose create spelled the member out: a cluster always provisions its nodes in SOME
/// mode. The modelled `ClusterNodeProvisioningMode` enum carries exactly ONE member, so
/// that member is the mode the service reports, and it is read from the generated value
/// slice rather than transcribed, so this stays in step with the model if the enum grows.
pub fn default_node_provisioning_mode() -> Option<String> {
    crate::ops::types::CLUSTER_NODE_PROVISIONING_MODE_VALUES
        .first()
        .map(|mode| (*mode).to_string())
}

/// `ClusterSortBy`'s documented default ("The field by which to sort results. The
/// default value is `CREATION_TIME`." — `ListClusters`' member doc). `ListClusters` has
/// its own default and its own enum tokens, which is why it does not share
/// [`DEFAULT_SORT_BY`].
pub const CLUSTER_SORT_BY_CREATION_TIME: &str = "CREATION_TIME";
/// `ClusterSortBy`'s other member.
pub const CLUSTER_SORT_BY_NAME: &str = "NAME";
/// `SortOrder`'s documented default for `ListClusters` ("The sort order for results. The
/// default value is `Ascending`.") — the opposite of [`DEFAULT_SORT_ORDER`], which is
/// what `ListModels` documents. The descending token is [`ORDER_DESCENDING`].
pub const CLUSTER_DEFAULT_SORT_ORDER: &str = ORDER_ASCENDING;

/// A SageMaker HyperPod cluster created by `CreateCluster`, as `ListClusters`
/// summarises it and `DescribeCluster` replays it.
///
/// The whole creation spec is stored, not only the members `ListClusters` reports: the
/// record is the resource, so the readers of the rest (`DescribeCluster`,
/// `ListClusterNodes`) read it from here rather than re-deriving it.
///
/// `in_service_at` is the instant the clock owes the `Creating` -> `InService`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it — which is the request that follows the create, since
/// the transition is owed from the instant the create answers
/// ([`transition_owed_at`]).
#[derive(Debug, Clone)]
pub struct Cluster {
    pub name: String,
    /// The 12-character id the `ClusterArn` pattern carries
    /// (`…:cluster/[a-z0-9]{12}`), minted once at create time.
    pub id: String,
    pub arn: String,
    pub status: String,
    pub instance_groups: Vec<crate::ops::types::ClusterInstanceGroupSpecification>,
    pub restricted_instance_groups:
        Vec<crate::ops::types::ClusterRestrictedInstanceGroupSpecification>,
    pub restricted_instance_groups_config:
        Option<crate::ops::types::ClusterRestrictedInstanceGroupsConfig>,
    pub vpc_config: Option<VpcConfig>,
    pub orchestrator: Option<crate::ops::types::ClusterOrchestrator>,
    pub node_recovery: Option<String>,
    pub tiered_storage_config: Option<crate::ops::types::ClusterTieredStorageConfig>,
    pub node_provisioning_mode: Option<String>,
    pub cluster_role: Option<String>,
    pub auto_scaling: Option<crate::ops::types::ClusterAutoScalingConfig>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes this cluster's deletion, set by
    /// [`Cluster::begin_deletion`]; `None` while no `DeleteCluster` has asked for one.
    /// Past it the cluster is gone for every caller ([`Cluster::is_released`]), whether
    /// or not a sweep has released the record yet.
    pub removal_due_at: Option<SystemTime>,
    /// The cluster's nodes, keyed by `NodeLogicalId` — the instances its groups were
    /// created asking for plus the ones `BatchAddClusterNodes` added, which
    /// `ListClusterNodes` reports, `DescribeClusterNode` replays and
    /// `AttachClusterNodeVolume` attaches storage inside. A cluster still `Creating`
    /// holds none: its groups' instances are minted when it reaches `InService`, by
    /// [`Cluster::settle_status`].
    pub nodes: BTreeMap<String, ClusterNode>,
    /// The Availability Zones each of this cluster's instance groups launches its nodes
    /// in, keyed by the group's name.
    ///
    /// Resolved from the subnets the groups run under when the cluster was created
    /// ([`crate::state::cluster_group_placements`]) and stored here, so every node of a
    /// group takes its placement from the one resolution and each reader of that node —
    /// `DescribeClusterNode`'s `Placement` and the `AvailabilityZones` of a
    /// `BatchAddClusterNodes` entry — reports the same zone.
    pub group_placements: BTreeMap<String, Vec<ClusterZone>>,
    /// What a `BatchAddClusterNodes` `ClientToken` replays: the outcome of the first call
    /// carrying that token. Keyed by the token.
    pub node_additions: BTreeMap<String, NodeAdditionRecord>,
}

impl Cluster {
    /// Whether a delete may begin on this cluster: its status is a settled one.
    ///
    /// A cluster the service is already moving — provisioning it, updating it, rolling
    /// an update back, or tearing it down — cannot take a second change at the same
    /// time, which is the conflict `DeleteCluster` models `ConflictException` for. The
    /// settled statuses are `InService` and [`CLUSTER_STATUS_FAILED`].
    pub fn accepts_deletion(&self) -> bool {
        self.status == STATUS_IN_SERVICE || self.status == CLUSTER_STATUS_FAILED
    }

    /// Start the deletion `DeleteCluster` asked for: the cluster reports `Deleting`, and
    /// the clock owes its release from the instant that call answers
    /// ([`transition_owed_at`]).
    ///
    /// The record is NOT dropped here: SageMaker answers a cluster in `ClusterStatus`'s
    /// `Deleting` until the teardown finishes, so that status is what this call leaves
    /// behind, and the removal is the clock's — stored as an instant and never slept on.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.removal_due_at = Some(transition_owed_at(now));
    }

    /// Whether a `DeleteCluster` has asked for this cluster's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether this cluster takes a change to its NODES.
    ///
    /// A cluster whose deletion has begun does not: its instances are going away with it,
    /// so adding, deleting, replacing or attaching storage to one is work the service
    /// cannot do, and the node operations refuse it with
    /// [`cluster_rejects_node_change`] rather than reporting a change they did not make.
    /// Every other status does — the nodes a cluster holds are reported from the instant
    /// it reaches `InService`, and a request against a cluster that is still provisioning
    /// is answered against the state a describe reports at the same instant.
    pub fn accepts_node_changes(&self) -> bool {
        !self.is_deleting()
    }

    /// Whether the deletion the clock owed has completed, i.e. the cluster is gone.
    ///
    /// A lookup answers `None` for such a cluster and a listing leaves it out, so no
    /// caller sees a cluster whose teardown was already due — the record itself is
    /// dropped by [`ComAmazonawsSagemakerSagemakerState::release_deleted_clusters`],
    /// which frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the `Creating` -> `InService` transition the clock already owes.
    ///
    /// The one write a reporting operation may make: the transition was due at
    /// `in_service_at` whether or not anybody listed the cluster, so the observer makes
    /// no difference to the result. Answers whether it happened on this call.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.in_service_at {
            self.status = STATUS_IN_SERVICE.to_string();
            self.provision_group_instances(now);
            return true;
        }
        false
    }

    /// Bring every instance group up to the number of instances it declares, minting the
    /// ones it is short of as `Running` nodes of that group.
    ///
    /// `InstanceCount` is "the number of instances to add to the instance group", so a
    /// group that declares N holds N of them once the cluster is provisioned — the same N
    /// that [`Cluster::instance_group_details`] reports as realised in `CurrentCount`, so
    /// the cluster's counts and its node list cannot disagree. They come up WITH the
    /// cluster, which is why this runs at the instant it reaches `InService` and never
    /// before: the create itself stores no node, and the first request after it reports
    /// the instances the groups asked for.
    ///
    /// A group is short by however many nodes it does not already hold, so the nodes a
    /// call added to it (which raised its count by the same amount) are not minted twice.
    /// The whole cluster stays inside [`CLUSTER_NODE_QUOTA`], the bound every other node
    /// path honours.
    pub(super) fn provision_group_instances(&mut self, now: SystemTime) {
        // The groups' settings are read off first, so the nodes can be written while
        // iterating them. A restricted group's instances are instances of this cluster
        // too, and `ListClusterNodes` reports "the list of instances ... in a SageMaker
        // HyperPod cluster".
        let declared: Vec<(String, i32, Option<String>, Option<i32>)> = self
            .instance_groups
            .iter()
            .map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    instance_group_types(group).first().cloned(),
                    group.threads_per_core,
                )
            })
            .chain(self.restricted_instance_groups.iter().map(|group| {
                (
                    group.instance_group_name.clone(),
                    group.instance_count,
                    Some(group.instance_type.clone()),
                    group.threads_per_core,
                )
            }))
            .collect();

        for (group_name, instance_count, instance_type, threads_per_core) in declared {
            let held = self
                .nodes
                .values()
                .filter(|node| node.instance_group_name == group_name)
                .count();
            let declared_count = usize::try_from(instance_count).unwrap_or_default();
            for ordinal in held..declared_count {
                if self.nodes.len() >= CLUSTER_NODE_QUOTA {
                    return;
                }
                // Where this instance is launched: the zone of the subnet its group runs
                // under, resolved when the cluster was created.
                let placement = self.node_placement(&group_name, ordinal);
                let logical_id = mint_node_logical_id();
                self.nodes.insert(
                    logical_id.clone(),
                    ClusterNode {
                        logical_id: logical_id.clone(),
                        // An instance that is up has its id, the same one the
                        // `Pending` -> `Running` transition mints for an added node.
                        instance_id: Some(mint_instance_id()),
                        instance_group_name: group_name.clone(),
                        instance_type: instance_type.clone(),
                        status: NODE_STATUS_RUNNING.to_string(),
                        availability_zone: placement.as_ref().map(|zone| zone.name.clone()),
                        availability_zone_id: placement.and_then(|zone| zone.id),
                        threads_per_core,
                        launch_time: now,
                        running_at: now,
                        // Nothing has asked for this node's hardware to be replaced.
                        replacement_due_at: None,
                        // Nothing has asked for this node to be rebooted.
                        reboot_due_at: None,
                        volume_attachments: BTreeMap::new(),
                    },
                );
            }
        }
    }

    /// Whether this cluster uses Amazon EKS as its ORCHESTRATION, i.e. its orchestrator is
    /// an Amazon EKS cluster.
    ///
    /// `Orchestrator` carries "exactly one orchestrator configuration: either `Eks` or
    /// `Slurm`" (`CreateCluster$Orchestrator`), so a cluster is EKS-orchestrated exactly
    /// when the configuration it was created with is the EKS one — a cluster orchestrated
    /// by Slurm is not, and neither is one that names no orchestrator at all, since
    /// nothing then names an EKS cluster for its nodes to be managed by.
    ///
    /// The storage operations of a node are the ones this decides: both
    /// `AttachClusterNodeVolume$ClusterArn` and `DetachClusterNodeVolume$ClusterArn`
    /// document "Your cluster must use EKS as the orchestration and be in the `InService`
    /// state", so the one predicate answers for both.
    pub fn is_eks_orchestrated(&self) -> bool {
        self.orchestrator
            .as_ref()
            .is_some_and(|orchestrator| orchestrator.eks.is_some())
    }

    /// The zone the node with ordinal `ordinal` among the nodes of the instance group
    /// `group_name` is launched in, or `None` when this cluster resolved no placement for
    /// that group at all.
    ///
    /// The group's resolved zones are taken IN TURN, so a group placed across the subnets
    /// of several zones spreads its instances over them instead of stacking every one in
    /// the first: a group's nodes are launched into "the availability zones" its
    /// configuration puts it in, not into one of them.
    pub fn node_placement(&self, group_name: &str, ordinal: usize) -> Option<ClusterZone> {
        let zones = self.group_placements.get(group_name)?;
        if zones.is_empty() {
            return None;
        }
        zones.get(ordinal % zones.len()).cloned()
    }

    /// How many nodes of the instance group `group_name` this cluster already holds, which
    /// is the ordinal the NEXT node of that group takes ([`Cluster::node_placement`]).
    pub fn nodes_in_group(&self, group_name: &str) -> usize {
        self.nodes
            .values()
            .filter(|node| node.instance_group_name == group_name)
            .count()
    }

    /// The zone of `group_name` whose NAME is `zone_name`, so a node an add request placed
    /// in a zone it named by name still reports the ID the owning service paired with it.
    pub fn group_zone_named(&self, group_name: &str, zone_name: &str) -> Option<ClusterZone> {
        self.group_placements
            .get(group_name)?
            .iter()
            .find(|zone| zone.name == zone_name)
            .cloned()
    }

    /// The instances this cluster CURRENTLY holds, the utilisation its account's instance
    /// quota is measured against ([`ComAmazonawsSagemakerSagemakerState::cluster_instances_held`]).
    ///
    /// The nodes the cluster holds once it is provisioned, and — for one still coming up,
    /// which holds none yet — the instances its groups have already committed: both are
    /// capacity the account has taken, and the two counts agree for every cluster past
    /// provisioning, because a group's count moves with the nodes added to or removed from
    /// it.
    pub fn instances_held(&self) -> i64 {
        let provisioned = i64::try_from(self.nodes.len()).unwrap_or(i64::MAX);
        let committed: i64 =
            cluster_instance_demand(&self.instance_groups, &self.restricted_instance_groups)
                .values()
                .sum();
        provisioned.max(committed)
    }

    /// The training plans this cluster's capacity is reserved against, in first-seen
    /// order with duplicates dropped.
    ///
    /// `ClusterSummary::TrainingPlanArns` is "a list of ARNs of the training plans
    /// associated with this cluster", and a cluster is associated with a plan through
    /// the `TrainingPlanArn` of an instance group it was created with — so the list is
    /// computed from the stored groups rather than stored a second time.
    pub fn training_plan_arns(&self) -> Vec<String> {
        let mut arns: Vec<String> = Vec::new();
        let from_groups = self
            .instance_groups
            .iter()
            .map(|group| group.training_plan_arn.as_ref())
            .chain(
                self.restricted_instance_groups
                    .iter()
                    .map(|group| group.training_plan_arn.as_ref()),
            );
        for arn in from_groups.flatten() {
            if !arns.iter().any(|seen| seen == arn) {
                arns.push(arn.clone());
            }
        }
        arns
    }

    /// Whether the node `logical_id` names runs in an instance group this cluster declared
    /// as its Slurm CONTROLLER, i.e. a node the Slurm workload manager runs on.
    ///
    /// Such a node cannot be taken out from under the workload manager: both
    /// `BatchDeleteClusterNodes$NodeIds` ("you cannot remove instances that are configured
    /// as Slurm controller nodes") and `BatchReplaceClusterNodes$NodeIds` ("you cannot
    /// replace instances that are configured as Slurm controller nodes") state it, so the
    /// membership is read off the group's own `SlurmConfig$NodeType` here, once, for both.
    ///
    /// `false` for a node of a group that declares no `SlurmConfig` — an EKS-orchestrated
    /// cluster has no Slurm controller at all — and for an identifier this cluster holds no
    /// node for, which its own not-found answer reports.
    pub fn is_slurm_controller_node(&self, logical_id: &str) -> bool {
        let Some(node) = self.nodes.get(logical_id) else {
            return false;
        };
        self.instance_groups
            .iter()
            .filter(|group| group.instance_group_name == node.instance_group_name)
            .any(|group| {
                group
                    .slurm_config
                    .as_ref()
                    .is_some_and(|slurm| slurm.node_type == SLURM_NODE_TYPE_CONTROLLER)
            })
    }
}

/// The `ClusterSlurmNodeType` member naming the instance group the Slurm workload manager
/// itself runs in. Named once here so the two node operations that refuse a change to such
/// a node do not compare a bare string literal.
pub const SLURM_NODE_TYPE_CONTROLLER: &str = "Controller";

/// How `BatchDeleteClusterNodes` states that an identifier names a Slurm controller node,
/// in the model's own words for the constraint
/// (`BatchDeleteClusterNodes$NodeIds`: "For SageMaker HyperPod clusters using the Slurm
/// workload manager, you cannot remove instances that are configured as Slurm controller
/// nodes.").
pub const SLURM_CONTROLLER_NOT_REMOVABLE: &str =
    "You cannot remove instances that are configured as Slurm controller nodes.";

/// How `BatchReplaceClusterNodes` states the same constraint, in that operation's own
/// words for it (`BatchReplaceClusterNodes$NodeIds`: "you cannot replace instances that
/// are configured as Slurm controller nodes").
pub const SLURM_CONTROLLER_NOT_REPLACEABLE: &str =
    "You cannot replace instances that are configured as Slurm controller nodes.";

/// How `BatchDeleteClusterNodes` refuses a request NONE of whose identifiers names a
/// deletable node: the sentence names the three conditions an identifier has to satisfy,
/// which are exactly the three per-node failures the call can report — the node exists, it
/// is not one of the Slurm controller group's, and its state admits deletion.
///
/// Entirely fixed text: the request's own identifiers are not named, because the refusal
/// is about the request as a whole rather than about any one of them.
pub const ALL_NODES_FAILED_VALIDATION: &str =
    "All provided nodes failed validation. Please ensure the nodes exist, are not part of \
     the controller group for the Slurm cluster, and are in a removable state.";

/// How a batch node operation words an identifier that names no node of the cluster, in
/// the terms the failure list carrying it documents its subject in: the instance-id list
/// speaks of the INSTANCE the identifier named and the logical-id list of the NODE, as the
/// per-list error shapes of `BatchRebootClusterNodes` and `BatchReplaceClusterNodes` do.
///
/// Fixed text with no variable part: the entry it sits in already carries the identifier it
/// is about. Stated once here so the operations reporting this one condition cannot word it
/// differently.
pub const INSTANCE_NOT_IN_CLUSTER: &str = "The instance does not exist in the specified cluster.";
/// See [`INSTANCE_NOT_IN_CLUSTER`].
pub const NODE_NOT_IN_CLUSTER: &str = "The node does not exist in the specified cluster.";

/// How a batch node operation words a specification that names an instance group the cluster
/// does not hold — the per-entry sentence that travels beside
/// `BatchAddClusterNodesErrorCode`'s `InstanceGroupNotFound`.
///
/// The group is named AS THE REQUEST NAMED IT, in parentheses, and the rest is fixed text:
/// the entry is about one group of the request, so the sentence points the caller at its own
/// input rather than at the cluster. Stated once here so every operation reporting this one
/// condition words it the same way.
pub fn instance_group_not_found(instance_group_name: &str) -> String {
    format!(
        "Instance group provided as part of the request ({instance_group_name}) does not \
         exist. Please check the input and try again."
    )
}

/// How a batch node operation refuses a request NONE of whose identifiers names a node of
/// the cluster at all — a client working from a stale node list, say.
///
/// The per-node failure lists report the nodes a call could not act on ALONGSIDE the ones
/// it could; a request that names no node the cluster holds asked for nothing such a call
/// can do, so it is refused outright instead of answering a 200 whose every entry is a
/// failure. Entirely fixed text: the request named nothing for the sentence to be about.
/// Stated once here so the operations that refuse this one shape cannot word it
/// differently.
pub const ALL_NODES_MISSING: &str =
    "All provided nodes do not exist. Please check the input and try again.";

/// How many instances of ONE ML instance type an account may run in its HyperPod
/// clusters, for a type whose published default is not zero.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — as with
/// [`MODEL_QUOTA`] and the hosting bound [`DEFAULT_ENDPOINT_USAGE_QUOTA`] — this is the
/// emulator's OWN bound rather than a claim about any account's limit for a particular
/// type: far above any cluster an emulator serves, and what stops one request asking for
/// an unbounded number of instances. The accelerated generations
/// ([`has_zero_default_instance_quota`]) carry AWS's published ZERO default instead, so a
/// cluster on one is refused until the account is granted capacity.
pub const DEFAULT_CLUSTER_USAGE_QUOTA: i64 = 1_000;

/// The published name of the quota that bounds how many instances of ONE ML instance type
/// an account may run in its HyperPod clusters, as a refusal states it.
///
/// Service Quotas names the entry `<instance type> for cluster usage` — the same shape as
/// the endpoint-usage entry [`endpoint_usage_quota_name`] words — so the instance type is
/// part of the quota's NAME rather than prose around it: a client reads which type it has
/// no capacity for straight off the message.
#[must_use]
pub fn cluster_usage_quota_name(instance_type: &str) -> String {
    format!("{instance_type} for cluster usage")
}

/// The account's quota, in instances, for running `instance_type` in a HyperPod cluster.
#[must_use]
pub fn cluster_usage_quota(instance_type: &str) -> i64 {
    if has_zero_default_instance_quota(instance_type) {
        return 0;
    }
    DEFAULT_CLUSTER_USAGE_QUOTA
}

/// The instance type a group that names none is counted under, so a group's instances are
/// still measured against a quota entry.
///
/// A group may name neither an `InstanceType` nor `InstanceRequirements`
/// ([`instance_group_types`] answers an empty list for one), and a quota is per type.
pub const UNNAMED_CLUSTER_INSTANCE_TYPE: &str = "ml.unspecified";

/// How many instances of each ML instance type a cluster's groups ask for: one entry per
/// type, the counts of every group naming it added up.
///
/// A group is counted under the FIRST type it admits, because that is the type its nodes
/// come up as ([`Cluster::provision_group_instances`]); a restricted group names its type
/// outright.
#[must_use]
pub fn cluster_instance_demand(
    instance_groups: &[crate::ops::types::ClusterInstanceGroupSpecification],
    restricted_instance_groups: &[crate::ops::types::ClusterRestrictedInstanceGroupSpecification],
) -> BTreeMap<String, i64> {
    let mut demand: BTreeMap<String, i64> = BTreeMap::new();
    for group in instance_groups {
        let instance_type = instance_group_types(group)
            .first()
            .cloned()
            .unwrap_or_else(|| UNNAMED_CLUSTER_INSTANCE_TYPE.to_string());
        *demand.entry(instance_type).or_default() += i64::from(group.instance_count);
    }
    for group in restricted_instance_groups {
        *demand.entry(group.instance_type.clone()).or_default() += i64::from(group.instance_count);
    }
    demand
}

impl ComAmazonawsSagemakerSagemakerState {
    /// How many instances of `instance_type` the account's HyperPod clusters are already
    /// using — the utilisation its cluster quota is measured against.
    ///
    /// Read off the stored clusters' own groups, so a cluster the clock has released uses
    /// nothing, exactly as a deleted endpoint does for the hosting quota.
    pub fn cluster_instances_in_use(&self, instance_type: &str) -> i64 {
        self.clusters
            .values()
            .filter_map(|cluster| {
                cluster_instance_demand(&cluster.instance_groups, &cluster.restricted_instance_groups)
                    .remove(instance_type)
            })
            .sum()
    }

    /// Why this account cannot run `demand` on top of what its clusters already hold, or
    /// `None` when every instance type it names is within quota.
    ///
    /// One instance type past its quota is enough to refuse the whole create, and the one
    /// reported is the first in type order, so the same request is always refused the same
    /// way. The sentence is [`account_quota_exceeded`]'s, counted in [`INSTANCES_UNIT`] —
    /// the same reading the hosting quota takes
    /// ([`ComAmazonawsSagemakerSagemakerState::hosting_quota_exceeded`]).
    pub fn cluster_quota_exceeded(&self, demand: &BTreeMap<String, i64>) -> Option<String> {
        demand.iter().find_map(|(instance_type, delta)| {
            let limit = cluster_usage_quota(instance_type);
            let in_use = self.cluster_instances_in_use(instance_type);
            (in_use + delta > limit).then(|| {
                account_quota_exceeded(
                    &cluster_usage_quota_name(instance_type),
                    INSTANCES_UNIT,
                    limit,
                    in_use,
                    *delta,
                )
            })
        })
    }

    /// How many instances the account's LIVE HyperPod clusters currently hold — the
    /// utilisation [`CLUSTER_INSTANCES_QUOTA`] is measured against.
    ///
    /// Counted over every cluster the account still has: one whose teardown has begun, one
    /// that failed, and one the clock has already released hold no capacity any more, so
    /// each is left out and the instances it held are available to the account again. What
    /// each remaining cluster contributes is what it holds ([`Cluster::instances_held`]).
    pub fn cluster_instances_held(&self, now: SystemTime) -> i64 {
        self.clusters
            .values()
            .filter(|cluster| {
                !cluster.is_released(now)
                    && !cluster.is_deleting()
                    && cluster.status != CLUSTER_STATUS_FAILED
            })
            .map(Cluster::instances_held)
            .sum()
    }

    /// Why the account cannot put `delta` more instances into its HyperPod clusters, or
    /// `None` when the whole ask fits inside `limit`.
    ///
    /// `limit` is the value the account's own instances-across-clusters quota is in force at
    /// ([`cluster_instance_entitlement`]), never a number this service decides. The bound is
    /// the ACCOUNT's, not one cluster's: it is measured against what every live cluster of
    /// the account already holds, so the answer to a scale-out does not depend on which
    /// cluster asks. The sentence is [`account_quota_exceeded`]'s, naming the published
    /// quota and counted in [`INSTANCES_UNIT`], like every other published account bound
    /// this service evaluates.
    pub fn cluster_instance_quota_exceeded(
        &self,
        now: SystemTime,
        delta: i64,
        limit: i64,
    ) -> Option<String> {
        let in_use = self.cluster_instances_held(now);
        (in_use + delta > limit).then(|| {
            account_quota_exceeded(
                CLUSTER_INSTANCES_QUOTA_NAME,
                INSTANCES_UNIT,
                limit,
                in_use,
                delta,
            )
        })
    }
}

/// The instance types an instance group admits: the one its specification names, or the
/// list its `InstanceRequirements` names.
///
/// The model makes the two mutually exclusive ("The `InstanceType` and
/// `InstanceRequirements` properties are mutually exclusive" —
/// `ClusterInstanceRequirements`' own doc), and a group may name neither, which is why
/// this answers a list that can be empty: a group that names no type admits any.
pub fn instance_group_types(
    group: &crate::ops::types::ClusterInstanceGroupSpecification,
) -> Vec<String> {
    if let Some(instance_type) = &group.instance_type {
        return vec![instance_type.clone()];
    }
    group
        .instance_requirements
        .as_ref()
        .map(|requirements| requirements.instance_types.clone())
        .unwrap_or_default()
}

/// Mint the id a `ClusterArn` carries: the 12 lowercase hex digits
/// [`mint_short_id`] produces, which is exactly the `[a-z0-9]{12}` the
/// modelled `ClusterArn` pattern admits.
///
/// One minter for the service: the entropy, its bounds and the reasons it is not a
/// secret are stated once, on [`mint_opaque_resource_id`]. `CreateCluster` still
/// re-mints on a collision rather than assuming uniqueness.
pub fn mint_cluster_id() -> String {
    mint_short_id()
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another cluster fits, i.e. the store is below [`CLUSTER_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn clusters_below_quota(&self) -> bool {
        self.clusters.len() < CLUSTER_QUOTA
    }

    /// Drop every cluster whose deletion the clock already completed, answering their
    /// ARNs in store order.
    ///
    /// Materialises only what was already due: the release was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a
    /// cluster ([`Cluster::is_released`]), so this changes nothing a later caller sees —
    /// it frees the name and the quota slot the gone cluster still held.
    pub fn release_deleted_clusters(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .clusters
            .values()
            .filter(|cluster| cluster.is_released(now))
            .map(|cluster| (cluster.name.clone(), cluster.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.clusters.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }

    /// The cluster whose `ClusterArn` is `arn`, if this account holds one.
    ///
    /// [`Self::clusters`] is keyed by `ClusterName` — the identifier `CreateCluster`
    /// takes — while another resource that BELONGS to a cluster names it by ARN
    /// (`CreateClusterSchedulerConfig$ClusterArn`), so the lookup is by the stored
    /// `Cluster::arn`. One place for it, so every operation resolving a `ClusterArn`
    /// resolves it against the same records `CreateCluster` writes and `ListClusters`
    /// reports, rather than against a second copy of the cluster.
    pub fn cluster_by_arn(&self, arn: &str) -> Option<&Cluster> {
        self.clusters
            .values()
            .find(|cluster| cluster.arn == arn)
            .filter(|cluster| !cluster.is_released(SystemTime::now()))
    }
}
