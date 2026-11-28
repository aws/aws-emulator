// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instances a cluster runs on: what each one is, where it came from, and
//! the lifecycle the clock moves it through.
//!
//! One concern of this service's state. "Represents an Amazon EC2 instance provisioned as
//! part of cluster" (`Instance` doc, Smithy model) says what one of these is: an instance of
//! Amazon EC2, not a resource of this service. So a cluster's capacity is not minted here —
//! Amazon EMR ASKS the service that owns instances to launch it, over the in-process
//! callback every emulated service reaches another through
//! ([`smithy_server_runtime::OperationContext::callback`]), and keeps the ids Amazon EC2
//! minted. This crate names no other service's crate, no socket is opened and nothing
//! outside this process is reachable; the endpoint URL the client is built with is
//! ADDRESSING for that transport, whose only variable part is the Region the request was
//! signed for, and the credentials are the emulator's own rather than the caller's.
//!
//! What a cluster therefore reports as `Instance$Ec2InstanceId` is an instance Amazon EC2
//! itself returns from `DescribeInstances`, with the private address and private DNS name
//! Amazon EC2 gave it; a cluster whose launch Amazon EC2 refused, and one launched in an
//! emulator with no Amazon EC2 mounted, hold the instances Amazon EC2 really answered with
//! and nothing else — this service reports no instance that does not exist.
//!
//! The lifecycle is this service's own: an instance Amazon EMR has just launched is
//! `PROVISIONING` and the clock owes it the move to `RUNNING` ([`super::TRANSITION_SPAN`]),
//! and an instance of a cluster that has terminated is `TERMINATED` — the same spans and
//! the same reasons its instance group reaches ([`super::InstanceGroupRecord`]).
//!
//! `Instance$Market` is this service's own fact too, and the one member of the record that is
//! not read off Amazon EC2's answer. "The instance purchasing option. Valid values are
//! `ON_DEMAND` or `SPOT`" is the option the COLLECTION provisions its instances under
//! (`InstanceGroup$Market`, `InstanceTypeConfig`'s bid members), so it is reported from the
//! collection's own configuration: this emulator's Amazon EC2 models no Spot market — a
//! launch carries no `InstanceMarketOptions` because nothing there would act on one — so
//! asking it for the market would answer about a market it does not have, where the
//! collection's configuration is what a client asked for and what every other read of that
//! collection reports.

use super::*;

use crate::ops::types::{Instance, InstanceStateChangeReason, InstanceStatus, InstanceTimeline};

/// The prefix of every instance id Amazon EMR mints for an instance of a cluster
/// (`Instance$Id`, "the unique identifier for the instance in Amazon EMR").
pub const CLUSTER_INSTANCE_ID_PREFIX: &str = "ci-";

/// `com.amazonaws.emr#InstanceState`, for the states this emulator sets
/// (`smithy_query(query="shape", shape="InstanceState")`).
pub mod instance_state {
    /// The instance is acquiring its Amazon EC2 resources and is not up yet.
    pub const PROVISIONING: &str = "PROVISIONING";
    /// The instance is up and part of the cluster.
    pub const RUNNING: &str = "RUNNING";
    /// The instance is gone, because its cluster is.
    pub const TERMINATED: &str = "TERMINATED";
}

/// `com.amazonaws.emr#InstanceStateChangeReasonCode`, for the codes this emulator sets.
pub mod instance_state_change_reason_code {
    pub const CLUSTER_TERMINATED: &str = "CLUSTER_TERMINATED";
}

/// How many instances ONE cluster holds records for.
///
/// "Provides information for all active Amazon EC2 instances and Amazon EC2 instances
/// terminated in the last 30 days, up to a maximum of 2,000" (`ListInstances` doc, Smithy
/// model) — the ceiling the service itself publishes for what it reports of a cluster's
/// instances, so it is also as many as this store keeps for one: every instance a collection
/// asks for is one record on the cluster, and a terminated cluster keeps its records, so
/// nothing removes one and this is what keeps the resident state finite.
pub const MAX_INSTANCES_PER_CLUSTER: usize = 2_000;

/// `ListInstances` page size.
///
/// CHOSEN, not documented, for the same reason [`super::LIST_INSTANCE_GROUPS_PAGE_SIZE`] is:
/// the operation paginates on `Marker` but AWS publishes no page size for it, and it is
/// deliberately below [`MAX_INSTANCES_PER_CLUSTER`], so a cluster can hold more instances
/// than one page reports and the pagination contract — a `Marker` that resumes the listing
/// where the page stopped — is reachable rather than dead.
pub const LIST_INSTANCES_PAGE_SIZE: usize = 50;

/// One persisted instance of a cluster.
///
/// [`ClusterInstanceRecord::to_instance`] projects it onto the modelled `Instance` shape, so
/// `ListInstances` reports what was persisted rather than anything built from its own input.
#[derive(Debug, Clone)]
pub struct ClusterInstanceRecord {
    /// `Instance$Id`: the identifier Amazon EMR minted for this instance.
    pub id: String,
    /// `Instance$Ec2InstanceId`: the instance Amazon EC2 minted for it, which is what makes
    /// this record a reference to an instance that exists.
    pub ec2_instance_id: String,
    /// The instance group this instance belongs to, for a cluster whose capacity is stated
    /// as groups, and `None` for one stated as fleets.
    pub instance_group_id: Option<String>,
    /// The instance fleet it belongs to instead, by the same rule.
    pub instance_fleet_id: Option<String>,
    /// `Instance$InstanceType`: the Amazon EC2 instance type it was launched as.
    pub instance_type: String,
    /// `Instance$Market`: the purchasing option the collection provisions it under.
    pub market: String,
    /// The private address Amazon EC2 gave it, and the Amazon-provided private hostname of
    /// that address — both read off Amazon EC2's own answer, so neither is invented here. A
    /// launch Amazon EC2 reported without one leaves the member absent.
    pub private_ip_address: Option<String>,
    pub private_dns_name: Option<String>,
    /// `InstanceTimeline$CreationDateTime`.
    pub creation_date_time: SystemTime,
    /// When the `PROVISIONING` -> `RUNNING` transition the clock owes comes due, and `None`
    /// once it has been materialised.
    pub ready_due_at: Option<SystemTime>,
    /// `InstanceTimeline$ReadyDateTime`: the instant that move came DUE, once it has.
    pub ready_date_time: Option<SystemTime>,
    /// `InstanceTimeline$EndDateTime`: when the instance was terminated.
    pub end_date_time: Option<SystemTime>,
    /// The state the record is in, moved only by [`Self::advance_to`] and [`Self::end_at`].
    pub state: String,
    /// Why it is in that state, for the one state something moved it to.
    pub state_change_reason: Option<StateChangeReasonRecord>,
    /// The Amazon EBS volumes this instance was given, one per block device its collection
    /// asks every one of its instances for ([`super::InstanceEbsVolume`]) — each a volume
    /// Amazon EC2 itself minted, which is what makes `Instance$EbsVolumes` a list of volumes
    /// that exist.
    pub ebs_volumes: Vec<InstanceEbsVolume>,
    /// Whether the log file this NODE is owed under its cluster's `LogUri` has been written
    /// ([`super::node_logs`]), which is what keeps one node's file written once however many
    /// callers report its cluster afterwards.
    pub node_log_delivered: bool,
}

impl ClusterInstanceRecord {
    /// Persist one instance Amazon EC2 has just launched for a collection of this cluster.
    ///
    /// An instance Amazon EMR has just launched is `PROVISIONING` ("the instance is
    /// provisioning" — `InstanceState`), so a caller that lists straight after the launch
    /// sees it coming up, and the clock owes it `RUNNING`.
    pub fn launched(id: String, launched: LaunchedInstance, now: SystemTime) -> Self {
        Self {
            id,
            ec2_instance_id: launched.ec2_instance_id,
            instance_group_id: launched.instance_group_id,
            instance_fleet_id: launched.instance_fleet_id,
            instance_type: launched.instance_type,
            market: launched.market,
            private_ip_address: launched.private_ip_address,
            private_dns_name: launched.private_dns_name,
            creation_date_time: now,
            ready_due_at: Some(now + TRANSITION_SPAN),
            ready_date_time: None,
            end_date_time: None,
            state: instance_state::PROVISIONING.to_string(),
            state_change_reason: None,
            ebs_volumes: launched.ebs_volumes,
            node_log_delivered: false,
        }
    }

    /// Whether this instance is one the cluster still holds: an instance that has not been
    /// terminated, which is what makes it part of its collection's capacity.
    pub fn is_live(&self) -> bool {
        self.state != instance_state::TERMINATED
    }

    /// Release this instance at `at`, because the collection it belongs to was resized below
    /// the capacity it was holding.
    ///
    /// "TERMINATED — The instance was terminated" (`InstanceState`), which is the same end
    /// [`Self::end_at`] records for an instance that went with its cluster. NO state change
    /// reason is reported beside it: `InstanceStateChangeReasonCode` names no reason for a
    /// resize, and a code this service cannot ground is not one it reports.
    pub fn release_at(&mut self, at: SystemTime) {
        if self.state == instance_state::TERMINATED {
            return;
        }
        self.state = instance_state::TERMINATED.to_string();
        self.ready_due_at = None;
        self.end_date_time = Some(at);
    }

    /// Materialise the transition the clock already made due, and nothing else: an instance
    /// that was `PROVISIONING` is `RUNNING` once its span has passed, which was owed at that
    /// instant whether or not anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.state != instance_state::PROVISIONING {
            return;
        }
        if let Some(due) = self.ready_due_at {
            if now >= due {
                self.state = instance_state::RUNNING.to_string();
                self.ready_date_time = Some(due);
                self.ready_due_at = None;
            }
        }
    }

    /// End this instance at `at`, because its cluster reached `TERMINATED`.
    ///
    /// "TERMINATED — The instance was terminated" (`InstanceState`), and the modelled reason
    /// for an instance that went with its cluster is `CLUSTER_TERMINATED` — the same end its
    /// instance group reaches ([`InstanceGroupRecord::end_at`]).
    pub fn end_at(&mut self, at: SystemTime) {
        if self.state == instance_state::TERMINATED {
            return;
        }
        self.state = instance_state::TERMINATED.to_string();
        self.state_change_reason = Some(StateChangeReasonRecord {
            code: instance_state_change_reason_code::CLUSTER_TERMINATED.to_string(),
            message: Some("The cluster was terminated.".to_string()),
        });
        self.ready_due_at = None;
        self.end_date_time = Some(at);
    }

    /// The wire `InstanceStatus` for this record.
    pub fn to_status(&self) -> InstanceStatus {
        InstanceStatus {
            state: Some(self.state.clone()),
            state_change_reason: self.state_change_reason.as_ref().map(|reason| {
                InstanceStateChangeReason {
                    code: Some(reason.code.clone()),
                    message: reason.message.clone(),
                }
            }),
            timeline: Some(InstanceTimeline {
                creation_date_time: Some(timestamp(self.creation_date_time)),
                ready_date_time: self.ready_date_time.map(timestamp),
                end_date_time: self.end_date_time.map(timestamp),
            }),
        }
    }

    /// The wire `Instance` for this record — what `ListInstances` reports.
    ///
    /// No public address is reported for an instance of this emulator: no address of this
    /// process is reachable from outside it, so none is announced as one — the same reading
    /// a cluster's own master node address is given ([`super::master_node`]).
    ///
    /// "The list of Amazon EBS volumes that are attached to this instance"
    /// (`Instance$EbsVolumes` doc) is one entry per volume this instance was given
    /// ([`super::InstanceEbsVolume`]), and the member is left absent for an instance whose
    /// collection asks for no Amazon EBS volume at all — the same reading
    /// `InstanceGroup$EbsBlockDevices` is given for a collection that configured none.
    pub fn to_instance(&self) -> Instance {
        Instance {
            id: Some(self.id.clone()),
            ec2_instance_id: Some(self.ec2_instance_id.clone()),
            public_dns_name: None,
            public_ip_address: None,
            private_dns_name: self.private_dns_name.clone(),
            private_ip_address: self.private_ip_address.clone(),
            status: Some(self.to_status()),
            instance_group_id: self.instance_group_id.clone(),
            instance_fleet_id: self.instance_fleet_id.clone(),
            market: Some(self.market.clone()),
            instance_type: Some(self.instance_type.clone()),
            ebs_volumes: optional_list(
                &self
                    .ebs_volumes
                    .iter()
                    .map(InstanceEbsVolume::to_ebs_volume)
                    .collect::<Vec<_>>(),
            ),
        }
    }
}

/// What one collection of a cluster asks Amazon EC2 for: so many instances of one type,
/// provisioned under one market, for the group or fleet that asked.
#[derive(Debug, Clone)]
pub struct InstanceLaunch {
    pub instance_group_id: Option<String>,
    pub instance_fleet_id: Option<String>,
    /// The role the collection fills in the cluster — `MASTER`, `CORE` or `TASK`
    /// (`InstanceRoleType`, `InstanceFleetType`) — which is what decides the placement group
    /// its instances are launched into ([`LaunchPlacement::group_strategy_for`]).
    pub role: String,
    pub instance_type: String,
    pub market: String,
    pub count: i32,
    /// The Amazon EBS block devices the collection maps onto EVERY one of its instances
    /// ([`super::collection_ebs_block_devices`]): "the block device mapping, which defines
    /// the EBS volumes … to attach to the instance at launch"
    /// (`RunInstancesRequest$BlockDeviceMappings` doc, Amazon EC2 model) is what the launch
    /// states them as, so each instance holds the volumes its collection asked for from the
    /// moment Amazon EC2 launched it.
    pub ebs_devices: Vec<EbsBlockDevice>,
    /// "Indicates whether an Amazon EBS volume is EBS-optimized"
    /// (`EbsConfiguration$EbsOptimized` doc) as the collection configured it, and `None` for
    /// one that configured nothing — which leaves the launch stating none.
    pub ebs_optimized: Option<bool>,
}

/// One instance Amazon EC2 launched, as this service keeps it.
#[derive(Debug, Clone)]
pub struct LaunchedInstance {
    pub ec2_instance_id: String,
    pub instance_group_id: Option<String>,
    pub instance_fleet_id: Option<String>,
    pub instance_type: String,
    pub market: String,
    pub private_ip_address: Option<String>,
    pub private_dns_name: Option<String>,
    /// The Availability Zone Amazon EC2 launched it in, read off its own answer — which is
    /// where the Amazon EBS volumes this instance is given have to be created, since "the
    /// volume and instance must be within the same Availability Zone"
    /// (`AttachVolumeRequest$VolumeId` doc, Amazon EC2 model).
    pub availability_zone: Option<String>,
    /// The Amazon EBS volumes Amazon EC2 created for this instance at the devices its
    /// collection mapped, read off the service that owns volumes as part of the launch
    /// ([`launch_ebs_volumes`]) — which is what `Instance$EbsVolumes` reports.
    pub ebs_volumes: Vec<InstanceEbsVolume>,
}

/// The instances the instance GROUP `group` asks for: "the target number of instances for
/// the instance group" (`InstanceGroupConfig$InstanceCount`) of its one instance type,
/// under the market it provisions.
pub fn group_launch(group: &InstanceGroupRecord) -> Option<InstanceLaunch> {
    let count = group.config.instance_count;
    (count > 0).then(|| InstanceLaunch {
        instance_group_id: Some(group.id.clone()),
        instance_fleet_id: None,
        role: group.config.instance_role.clone(),
        instance_type: group.config.instance_type.clone(),
        market: group.market().to_string(),
        count,
        // The devices every instance of this group is attached, by the one rule both
        // collection types name their devices by
        // ([`super::collection_ebs_block_devices`]): the layout the group configured, or
        // the default layout of its instance type when it configured none.
        ebs_devices: group_ebs_block_devices(&group.config),
        ebs_optimized: group
            .config
            .ebs_configuration
            .as_ref()
            .and_then(|configuration| configuration.ebs_optimized),
    })
}

/// The units ONE instance the instance type configuration `config` provisions supplies toward
/// a fleet's target capacities.
///
/// "The number of units that a provisioned instance of this type provides toward fulfilling
/// the target capacities" (`InstanceTypeConfig$WeightedCapacity` doc, Smithy model). A
/// configuration that states no weight, and a collection that states no configuration at all,
/// supplies one unit per instance ([`DEFAULT_WEIGHTED_CAPACITY`]); a weight of zero would
/// fulfil no capacity however many instances were provisioned, so one is the floor.
///
/// The ONE reading of that weight, so the instances a target needs
/// ([`fleet_launches`]) and the units a managed scaling activity adds for one instance
/// ([`super::managed_scaling`]) cannot disagree about one fleet.
pub fn provisioned_weighted_capacity(config: Option<&InstanceTypeConfig>) -> i32 {
    config
        .and_then(|config| config.weighted_capacity)
        .unwrap_or(DEFAULT_WEIGHTED_CAPACITY)
        .max(1)
}

/// The instances the instance FLEET `fleet` asks for, one launch per market it targets.
///
/// A fleet states its capacity in UNITS, not instances: "the target capacity of On-Demand
/// units for the instance fleet, which determines how many On-Demand Instances to
/// provision" (`InstanceFleet$TargetOnDemandCapacity`), and each instance type it may
/// provision supplies `WeightedCapacity` of them. Amazon EMR chooses which types to
/// provision by fit, and there is no capacity here to rank types by fit — so the choice is
/// the FIRST instance type the configuration lists, exactly as the choice of ONE placement
/// out of a list of candidates is ([`super::launched_in`]), and the fleet provisions as many
/// of that type as it takes to reach the target: the units are covered, so a partially
/// covering last instance still counts.
pub fn fleet_launches(fleet: &InstanceFleetRecord) -> Vec<InstanceLaunch> {
    let configs = fleet.config.instance_type_configs.as_deref().unwrap_or(&[]);
    let Some(chosen) = configs.first() else {
        return Vec::new();
    };
    let weight = provisioned_weighted_capacity(Some(chosen));
    let instances_for = |target: Option<i32>| -> i32 {
        let units = target.unwrap_or(0).max(0);
        // Units are covered, so the division rounds UP: a target the chosen type cannot
        // divide exactly still gets an instance for the remainder. The divisor is the units
        // ONE instance supplies, which is never zero ([`provisioned_weighted_capacity`]), and
        // a division by one that was is answered with no instance rather than performed.
        units
            .saturating_add(weight.saturating_sub(1))
            .checked_div(weight)
            .unwrap_or_default()
    };
    [
        (
            market_type::ON_DEMAND,
            instances_for(fleet.config.target_on_demand_capacity),
        ),
        (
            market_type::SPOT,
            instances_for(fleet.config.target_spot_capacity),
        ),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(market, count)| InstanceLaunch {
        instance_group_id: None,
        instance_fleet_id: Some(fleet.id.clone()),
        role: fleet.config.instance_fleet_type.clone(),
        instance_type: chosen.instance_type.clone(),
        market: market.to_string(),
        count,
        // The devices the CHOSEN instance type maps onto each of its instances, by the one
        // rule both collection types name their devices by
        // ([`super::fleet_ebs_block_devices`]).
        ebs_devices: fleet_ebs_block_devices(chosen),
        ebs_optimized: chosen
            .ebs_configuration
            .as_ref()
            .and_then(|configuration| configuration.ebs_optimized),
    })
    .collect()
}

/// Every launch one cluster's collections ask for, in collection order: the groups' and
/// then the fleets', within the room a cluster with no instances yet has
/// ([`MAX_INSTANCES_PER_CLUSTER`]).
pub fn planned_launches(
    groups: &[InstanceGroupRecord],
    fleets: &[InstanceFleetRecord],
) -> Vec<InstanceLaunch> {
    let planned: Vec<InstanceLaunch> = groups
        .iter()
        .filter_map(group_launch)
        .chain(fleets.iter().flat_map(fleet_launches))
        .collect();
    launches_within(planned, MAX_INSTANCES_PER_CLUSTER)
}

/// `launches` cut to the `room` instances the cluster has left
/// ([`MAX_INSTANCES_PER_CLUSTER`] less the records it already holds).
///
/// The bound is what keeps one cluster's records finite AND what keeps the launch and the
/// records the same set: every path that asks Amazon EC2 for capacity cuts its plan here
/// FIRST, so Amazon EC2 is never asked for an instance this service could not then report —
/// an instance launched and not recorded would be one no `ListInstances` names and no
/// `TerminateJobFlows` releases.
pub fn launches_within(launches: Vec<InstanceLaunch>, room: usize) -> Vec<InstanceLaunch> {
    let mut remaining = i32::try_from(room).unwrap_or(i32::MAX);
    let mut bounded: Vec<InstanceLaunch> = Vec::new();
    for mut launch in launches {
        if remaining <= 0 {
            break;
        }
        launch.count = launch.count.min(remaining);
        remaining -= launch.count;
        bounded.push(launch);
    }
    bounded
}

/// WHERE and AS WHAT a cluster's instances are launched: the facts of the cluster that every
/// one of its launches carries, read off its record so a launch made when the cluster is
/// created and one made when capacity is added cannot differ.
#[derive(Debug, Clone)]
pub struct LaunchPlacement {
    /// The subnet the cluster runs in, where its launch named one.
    pub subnet_id: Option<String>,
    /// The Availability Zone it runs in, resolved when it was launched.
    pub availability_zone: Option<String>,
    /// "The name of the Amazon EC2 key pair that can be used to connect to the master node"
    /// (`JobFlowInstancesConfig$Ec2KeyName`).
    pub key_name: Option<String>,
    /// "An IAM role for an Amazon EMR cluster. The Amazon EC2 instances of the cluster assume
    /// this role" (`RunJobFlow$JobFlowRole`) — so the instances are launched with it.
    pub instance_profile: Option<String>,
    /// "The custom AMI ID to use for the provisioned instance group"
    /// (`InstanceGroupConfig$CustomAmiId`) as the cluster states it
    /// (`RunJobFlow$CustomAmiId`), and `None` for a cluster that named none.
    pub custom_ami_id: Option<String>,
    /// "The size, in GiB, of the Amazon EBS root device volume of the Linux AMI that is used
    /// for each Amazon EC2 instance" (`RunJobFlow$EbsRootVolumeSize` doc), beside the IOPS
    /// and the throughput of that same device (`$EbsRootVolumeIops`,
    /// `$EbsRootVolumeThroughput`) — what the launch asked for the ROOT device of every one
    /// of the cluster's instances, as against the devices a collection's `EbsConfiguration`
    /// asks for. `None` leaves that device exactly as the image carries it.
    pub ebs_root_volume_size: Option<i32>,
    pub ebs_root_volume_iops: Option<i32>,
    pub ebs_root_volume_throughput: Option<i32>,
    /// The cluster the instances belong to, which is what the Amazon EC2 placement group
    /// they are launched into is named after ([`Self::placement_group_name_for`]).
    pub cluster_id: String,
    /// The Amazon EC2 placement group strategy each instance ROLE of this cluster is placed
    /// under, as `(role, strategy)`: "Amazon EC2 Placement Group strategy associated with
    /// instance role" (`PlacementGroupConfig$PlacementStrategy`), for every role this
    /// cluster really places ([`Self::effective_placement_groups`]).
    pub placement_groups: Vec<(String, String)>,
    /// The cluster's own tags, which it propagates to its instances: "a list of tags to
    /// associate with a cluster and propagate to Amazon EC2 instances" (`RunJobFlow$Tags`).
    /// A key in the reserved namespace is dropped, because what this service may set under
    /// that prefix are the tags it manages itself ([`CLUSTER_ID_TAG`],
    /// [`INSTANCE_ROLE_TAG`]) and not a key its caller supplied.
    pub tags: Vec<(String, String)>,
}

/// The service-managed tag naming the cluster an Amazon EC2 resource was created for.
///
/// "Amazon EMR automatically tags each Amazon EC2 instance it creates with
/// `aws:elasticmapreduce:job-flow-id` and `aws:elasticmapreduce:instance-group-role`"
/// (Amazon EMR Management Guide, "Tagging Amazon EMR clusters"), and the resources Amazon
/// EMR creates for a cluster carry the cluster's identity the same way — which is how a
/// client finds the Amazon EC2 resources of one cluster.
pub(super) const CLUSTER_ID_TAG: &str = "aws:elasticmapreduce:job-flow-id";

/// The service-managed tag naming the role an instance fills in its cluster, by the same
/// rule: `MASTER`, `CORE` or `TASK`.
const INSTANCE_ROLE_TAG: &str = "aws:elasticmapreduce:instance-group-role";

/// The prefix AWS reserves for the tags its own services set, which is why a cluster's own
/// tags are filtered through it before they are propagated: the keys this service sets under
/// it are its own.
const RESERVED_TAG_PREFIX: &str = "aws:";

/// The placement strategy Amazon EMR applies to the primary instance group of a cluster
/// with more than one primary node, when its launch configured none of its own.
///
/// "When you launch a cluster with multiple primary nodes, Amazon EMR automatically applies
/// the `SPREAD` placement group strategy to the primary instance group" (Amazon EMR
/// Management Guide, "Placement groups") — so a highly available cluster's primaries are on
/// separate underlying hardware whether or not the launch asked, which is the point of
/// running more than one.
const AUTOMATIC_PRIMARY_PLACEMENT_STRATEGY: &str = "SPREAD";

/// The `PlacementGroupStrategy` value that asks for NO placement group at all, which is why
/// a configuration naming it places nothing ([`super::created_placement_groups`] already
/// drops those entries before a cluster is stored).
const NO_PLACEMENT_STRATEGY: &str = "NONE";

impl LaunchPlacement {
    /// The placement of the cluster `cluster`.
    pub fn of(cluster: &ClusterRecord) -> Self {
        Self {
            subnet_id: cluster.instances.ec2_subnet_id.clone(),
            availability_zone: cluster.instances.ec2_availability_zone.clone(),
            key_name: cluster.instances.ec2_key_name.clone(),
            instance_profile: cluster.job_flow_role.clone(),
            custom_ami_id: cluster.custom_ami_id.clone(),
            ebs_root_volume_size: cluster.ebs_root_volume_size,
            ebs_root_volume_iops: cluster.ebs_root_volume_iops,
            ebs_root_volume_throughput: cluster.ebs_root_volume_throughput,
            cluster_id: cluster.id.clone(),
            placement_groups: Self::effective_placement_groups(cluster),
            tags: cluster
                .tags
                .iter()
                .filter_map(|tag| Some((tag.key.clone()?, tag.value.clone().unwrap_or_default())))
                .filter(|(key, _)| !key.starts_with(RESERVED_TAG_PREFIX))
                .collect(),
        }
    }

    /// The Amazon EC2 placement group strategy each role of `cluster` is really placed
    /// under: the ones its launch configured ([`PlacementGroupConfig`]), plus the one Amazon
    /// EMR applies to the primary instance group of a cluster with more than one primary
    /// node when the launch configured none for it
    /// ([`AUTOMATIC_PRIMARY_PLACEMENT_STRATEGY`]).
    ///
    /// A configuration is a strategy PER ROLE — "role of the instance in the cluster"
    /// (`PlacementGroupConfig$InstanceRole`) — so one role is placed under one strategy and
    /// the first configuration for a role is the one that holds.
    fn effective_placement_groups(cluster: &ClusterRecord) -> Vec<(String, String)> {
        let mut placed: Vec<(String, String)> = Vec::new();
        for config in &cluster.placement_groups {
            let role = config.instance_role.clone();
            let Some(strategy) = config
                .placement_strategy
                .clone()
                .filter(|strategy| !strategy.eq_ignore_ascii_case(NO_PLACEMENT_STRATEGY))
            else {
                continue;
            };
            if placed.iter().any(|(held, _)| *held == role) {
                continue;
            }
            placed.push((role, strategy));
        }
        let primaries = cluster
            .groups
            .iter()
            .filter(|group| group.config.instance_role == instance_group_type::MASTER)
            .fold(0i64, |total, group| {
                total.saturating_add(i64::from(group.config.instance_count.max(0)))
            })
            .saturating_add(
                cluster
                    .fleets
                    .iter()
                    .filter(|fleet| fleet.config.instance_fleet_type == instance_group_type::MASTER)
                    .fold(0i64, |total, fleet| {
                        total
                            .saturating_add(i64::from(
                                fleet.config.target_on_demand_capacity.unwrap_or(0).max(0),
                            ))
                            .saturating_add(i64::from(
                                fleet.config.target_spot_capacity.unwrap_or(0).max(0),
                            ))
                    }),
            );
        if primaries >= MIN_HIGHLY_AVAILABLE_PRIMARY_NODES
            && !placed
                .iter()
                .any(|(role, _)| role == instance_group_type::MASTER)
        {
            placed.push((
                instance_group_type::MASTER.to_string(),
                AUTOMATIC_PRIMARY_PLACEMENT_STRATEGY.to_string(),
            ));
        }
        placed
    }

    /// The strategy the instances filling `role` are placed under, or `None` when this
    /// cluster places that role in no placement group.
    pub fn group_strategy_for(&self, role: &str) -> Option<&str> {
        self.placement_groups
            .iter()
            .find(|(placed, _)| placed == role)
            .map(|(_, strategy)| strategy.as_str())
    }

    /// The name of the Amazon EC2 placement group this cluster's `role` instances are
    /// launched into.
    ///
    /// CHOSEN, not published: AWS documents that Amazon EMR creates the placement group for a
    /// role but not what it calls it. The name is derived from the cluster and the role, so
    /// one cluster's primaries share exactly one group, no two clusters share a group, and
    /// the name is the same every time it is derived.
    pub fn placement_group_name_for(&self, role: &str) -> String {
        format!("EMR_{}_{role}", self.cluster_id)
    }
}

/// The image a cluster's instances are launched from when its launch named no custom AMI, per
/// CPU ARCHITECTURE of the instance type the collection asks for.
///
/// "Amazon EMR releases are based on Amazon Linux" (Amazon EMR Management Guide, "Amazon
/// Linux release versions"), and the Amazon EMR AMI itself is not an image any account can
/// see as its own. What a cluster's instances run here is therefore the Amazon Linux image
/// the account CAN see, named through the public Systems Manager parameter that resolves to
/// its latest build — "the Systems Manager parameter will resolve to an AMI ID on launch"
/// (`RunInstancesRequest$ImageId`) — so the launch names no AMI id this service made up and
/// the instance reports the AMI the account's own catalogue holds.
///
/// There is one parameter per architecture because an image holds the machine code of ONE
/// architecture and "you must specify an instance type and an AMI that have matching
/// architectures" (Amazon EC2 User Guide): a cluster asking for Graviton instance types
/// (`AARCH64`, [`instance_type_architecture`]) therefore boots the `arm64` image and every
/// other the `x86_64` one, so a collection of either kind really launches. The two names are
/// the published Amazon Linux 2023 parameter paths, which differ only in that suffix.
const CLUSTER_IMAGE_X86_64: &str =
    "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64";
const CLUSTER_IMAGE_ARM64: &str =
    "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64";

/// The image the instances of a collection asking for `instance_type` are launched from.
///
/// A type this service's catalog does not carry is launched from the `x86_64` image, which is
/// the architecture of all but the Graviton families: nothing is known about such a type, and
/// the launch states the image every other collection states rather than none at all. That it
/// was not known is traced, because a type that is in fact of the other architecture is a
/// launch Amazon EC2 then refuses for the mismatch.
fn cluster_image_for(instance_type: &str) -> &'static str {
    match instance_type_architecture(instance_type) {
        Some(architecture::AARCH64) => CLUSTER_IMAGE_ARM64,
        Some(_) => CLUSTER_IMAGE_X86_64,
        None => {
            ::tracing::debug!(
                instance_type = ?instance_type,
                "this collection's instance type is not in the catalog, so its nodes are \
                 launched from the x86-64 image"
            );
            CLUSTER_IMAGE_X86_64
        }
    }
}

/// The device a cluster's instances hold their ROOT volume at.
///
/// "Amazon EMR releases are based on Amazon Linux" (Amazon EMR Management Guide, "Amazon
/// Linux release versions") and the root device of an Amazon Linux AMI is `/dev/xvda` (Amazon
/// EC2 User Guide, "Device names on Linux instances" — "the root device name for an instance
/// launched from a paravirtual or HVM Amazon Linux AMI"), so it is the device a cluster's
/// launch states what it asks of its root volume at. A cluster launched from a custom AMI
/// whose root device is some other name leaves that mapping naming a device the image does not
/// carry, which is the same thing the launch would state to Amazon EC2 directly.
const CLUSTER_ROOT_DEVICE_NAME: &str = "/dev/xvda";

/// Ask Amazon EC2 to delete the placement groups `group_names` names, because the cluster
/// they were created for is shutting down.
///
/// The mirror of the group an instance launch creates: Amazon EMR created these groups for
/// the cluster, so the shutdown that releases the cluster's instances releases them too —
/// nothing of the cluster is left behind in Amazon EC2. A refusal, and a call that reached no
/// Amazon EC2, are recorded and otherwise left alone.
pub async fn release_cluster_placement_groups(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_names: &[String],
) {
    if group_names.is_empty() {
        return;
    }
    let client = studio_network::ec2_client(ctx, region);
    for group_name in group_names {
        let answer = client
            .delete_placement_group()
            .group_name(group_name)
            .send()
            .await;
        if let Err(error) = answer {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                group_name = %group_name,
                "Amazon EC2 did not delete this cluster's placement group"
            );
        }
    }
}

/// Ask Amazon EC2 to terminate the instances `ec2_instance_ids` names, because the cluster
/// they were launched for is shutting down.
///
/// The mirror of [`launch_cluster_instances`]: Amazon EMR launched these instances in the
/// account, so it is Amazon EMR that releases them, and the instances a terminated cluster
/// reports as `TERMINATED` are instances Amazon EC2 has itself been told to terminate. A
/// refusal, and a call that reached no Amazon EC2, leave the records as they are: the
/// cluster's own lifecycle is this service's to move and does not depend on the answer.
///
/// Nothing here holds this service's store, for the reason [`launch_cluster_instances`]
/// states.
pub async fn terminate_cluster_instances(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ec2_instance_ids: &[String],
) {
    if ec2_instance_ids.is_empty() {
        return;
    }
    let answer = studio_network::ec2_client(ctx, region)
        .terminate_instances()
        .set_instance_ids(Some(ec2_instance_ids.to_vec()))
        .send()
        .await;
    if let Err(error) = answer {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "Amazon EC2 did not terminate this cluster's instances"
        );
    }
}

/// The Amazon EC2 placement group the instances of one collection are launched into, as
/// `(name, strategy)`, or `None` when the cluster places that collection's role in none.
impl InstanceLaunch {
    fn placement_group(&self, placement: &LaunchPlacement) -> Option<(String, String)> {
        let strategy = placement.group_strategy_for(&self.role)?;
        Some((
            placement.placement_group_name_for(&self.role),
            strategy.to_string(),
        ))
    }
}

/// Make sure Amazon EC2 holds the placement group `name` under `strategy`, answering the
/// group's own id when a launch may name it.
///
/// Creating it is idempotent for this service's purpose: the group is named after the cluster
/// and the role, so the only call that creates it is the first launch of that role, and a
/// repeat — a second collection filling the same role — is answered by Amazon EC2 with the
/// group already existing, in which case the group it already holds under that name is the
/// one this wanted. The strategy Amazon EMR states is the Amazon EC2 strategy of the same
/// name, in the spelling Amazon EC2 takes (`PlacementStrategy`: `cluster`, `partition`,
/// `spread`).
async fn ensure_placement_group(
    client: &::aws_sdk_ec2::Client,
    name: &str,
    strategy: &str,
) -> Option<String> {
    let answer = client
        .create_placement_group()
        .group_name(name)
        .strategy(::aws_sdk_ec2::types::PlacementStrategy::from(
            strategy.to_ascii_lowercase().as_str(),
        ))
        .send()
        .await;
    match answer {
        Ok(created) => created
            .placement_group()
            .and_then(|group| group.group_id())
            .map(str::to_string),
        Err(error) => {
            // A group that is already there is the state this asks for, so the launch may
            // name it; anything else leaves the role unplaced.
            let already_held = error
                .as_service_error()
                .and_then(::aws_sdk_ec2::error::ProvideErrorMetadata::code)
                .is_some_and(|code| code == PLACEMENT_GROUP_ALREADY_HELD);
            if !already_held {
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    group_name = name,
                    "Amazon EC2 did not create this cluster's placement group"
                );
                return None;
            }
            client
                .describe_placement_groups()
                .group_names(name)
                .send()
                .await
                .ok()?
                .placement_groups()
                .iter()
                .find(|group| group.group_name() == Some(name))
                .and_then(|group| group.group_id())
                .map(str::to_string)
        }
    }
}

/// The Amazon EC2 error code for a placement group the account already holds under that
/// name, which for a group named after a cluster and a role is the group this launch wanted.
const PLACEMENT_GROUP_ALREADY_HELD: &str = "InvalidPlacementGroup.Duplicate";

/// Tag the Amazon EC2 resources `resource_ids` names with `tags`, which is how every resource
/// Amazon EMR creates for a cluster carries that cluster's identity.
///
/// The tags include this service's own reserved-namespace keys ([`CLUSTER_ID_TAG`],
/// [`INSTANCE_ROLE_TAG`]): they are Amazon EMR's to set on what it provisions, and they reach
/// Amazon EC2 on the hop Amazon EMR makes rather than on anything a caller sent. A refusal is
/// recorded and otherwise left alone — the resources exist and the cluster reports them; only
/// the tags are missing.
pub(super) async fn tag_cluster_resources(
    client: &::aws_sdk_ec2::Client,
    resource_ids: &[String],
    tags: &[(String, String)],
) {
    if resource_ids.is_empty() || tags.is_empty() {
        return;
    }
    let tags: Vec<::aws_sdk_ec2::types::Tag> = tags
        .iter()
        .map(|(key, value)| {
            ::aws_sdk_ec2::types::Tag::builder()
                .key(key)
                .value(value)
                .build()
        })
        .collect();
    let answer = client
        .create_tags()
        .set_resources(Some(resource_ids.to_vec()))
        .set_tags(Some(tags))
        .send()
        .await;
    if let Err(error) = answer {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "Amazon EC2 did not tag this cluster's resources"
        );
    }
}

/// Ask Amazon EC2 for the instances `launches` describe, and answer the ones it launched.
///
/// One `RunInstances` per launch, so each collection's instances carry that collection's own
/// type and market. A launch Amazon EC2 refused — and a call that reached no Amazon EC2 at
/// all — contributes NO instances: this service reports the capacity Amazon EC2 really
/// provisioned, so a cluster whose instances could not be launched reports none rather than
/// records of instances that do not exist.
///
/// Nothing here holds this service's store: the whole plan is passed by value and awaited
/// with no guard held, so a launch cannot be ordered behind a lock.
pub async fn launch_cluster_instances(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    placement: &LaunchPlacement,
    launches: &[InstanceLaunch],
) -> Vec<LaunchedInstance> {
    let client = studio_network::ec2_client(ctx, region);
    let mut launched: Vec<LaunchedInstance> = Vec::new();
    for launch in launches {
        // WHICH image this collection's instances boot: the custom AMI the launch named for
        // the whole cluster, or the Amazon Linux image of the architecture THIS collection's
        // instance type runs ([`cluster_image_for`]).
        let image_id = placement
            .custom_ami_id
            .clone()
            .unwrap_or_else(|| cluster_image_for(&launch.instance_type).to_string());
        // WHERE this collection's instances are placed, settled before they are launched: a
        // role this cluster places is placed in a placement group OF this cluster, which is
        // Amazon EC2's resource and so is created there ([`ensure_placement_group`]) before a
        // launch names it. A group Amazon EC2 would not create places nothing, and the
        // instances are launched without one rather than not launched at all. The group is a
        // resource Amazon EMR created for the cluster, so it carries the cluster's identity.
        let mut group_name = None;
        if let Some((name, strategy)) = launch.placement_group(placement) {
            if let Some(group_id) = ensure_placement_group(&client, &name, &strategy).await {
                tag_cluster_resources(
                    &client,
                    &[group_id],
                    &[(CLUSTER_ID_TAG.to_string(), placement.cluster_id.clone())],
                )
                .await;
                group_name = Some(name);
            }
        }
        let mut request = client
            .run_instances()
            .image_id(&image_id)
            .instance_type(::aws_sdk_ec2::types::InstanceType::from(
                launch.instance_type.as_str(),
            ))
            .min_count(launch.count)
            .max_count(launch.count);
        if let Some(subnet_id) = placement.subnet_id.as_deref() {
            request = request.subnet_id(subnet_id);
        }
        // The placement this launch states: the zone the cluster runs in, where it was not
        // named through a subnet, and the placement group this role is placed in. One
        // `Placement` carries both, so it is built once from what the launch has.
        if placement.subnet_id.is_none() || group_name.is_some() {
            let mut stated = ::aws_sdk_ec2::types::Placement::builder();
            if placement.subnet_id.is_none() {
                stated = stated.set_availability_zone(placement.availability_zone.clone());
            }
            stated = stated.set_group_name(group_name.clone());
            request = request.placement(stated.build());
        }
        if let Some(key_name) = placement.key_name.as_deref() {
            request = request.key_name(key_name);
        }
        if let Some(profile) = placement.instance_profile.as_deref() {
            request = request.iam_instance_profile(
                ::aws_sdk_ec2::types::IamInstanceProfileSpecification::builder()
                    .name(profile)
                    .build(),
            );
        }
        // The ROOT device of this cluster's instances, where the launch asked for one of its
        // own: "the size, in GiB, of the Amazon EBS root device volume of the Linux AMI that
        // is used for each Amazon EC2 instance" (`RunJobFlow$EbsRootVolumeSize` doc), with
        // the IOPS and throughput of that same device. It is stated as the launch's mapping
        // for the root device of the Amazon Linux image a cluster's instances run
        // ([`CLUSTER_ROOT_DEVICE_NAME`]), which is how a launch states what it wants of a
        // device the image already carries; the members the launch does not state are left as
        // the image carries them.
        if placement.ebs_root_volume_size.is_some()
            || placement.ebs_root_volume_iops.is_some()
            || placement.ebs_root_volume_throughput.is_some()
        {
            request = request.block_device_mappings(
                ::aws_sdk_ec2::types::BlockDeviceMapping::builder()
                    .device_name(CLUSTER_ROOT_DEVICE_NAME)
                    .ebs(
                        ::aws_sdk_ec2::types::EbsBlockDevice::builder()
                            .set_volume_size(placement.ebs_root_volume_size)
                            .set_iops(placement.ebs_root_volume_iops)
                            .set_throughput(placement.ebs_root_volume_throughput)
                            .build(),
                    )
                    .build(),
            );
        }
        // The STORAGE this collection asks every one of its instances for: "the block device
        // mapping, which defines the EBS volumes … to attach to the instance at launch"
        // (`RunInstancesRequest$BlockDeviceMappings` doc, Amazon EC2 model) — so the volumes
        // an `EbsConfiguration` asks for are created by the launch itself and the instance
        // holds each one at the device Amazon EMR maps it to, rather than being attached
        // afterwards. Each device states exactly what its `VolumeSpecification` asks for:
        // "EBS volume specifications such as volume type, IOPS, size (GiB) and throughput
        // (MiB/s) that are requested for the EBS volume attached to an Amazon EC2 instance in
        // the cluster" (`VolumeSpecification` doc). They go with the instance, the way the
        // cluster's own storage does.
        for device in &launch.ebs_devices {
            let (Some(device_name), Some(specification)) =
                (device.device.as_deref(), device.volume_specification.as_ref())
            else {
                continue;
            };
            request = request.block_device_mappings(
                ::aws_sdk_ec2::types::BlockDeviceMapping::builder()
                    .device_name(device_name)
                    .ebs(
                        ::aws_sdk_ec2::types::EbsBlockDevice::builder()
                            .volume_size(specification.size_in_gb)
                            .volume_type(::aws_sdk_ec2::types::VolumeType::from(
                                specification.volume_type.as_str(),
                            ))
                            .set_iops(specification.iops)
                            .set_throughput(specification.throughput)
                            .delete_on_termination(true)
                            .build(),
                    )
                    .build(),
            );
        }
        if let Some(ebs_optimized) = launch.ebs_optimized {
            request = request.ebs_optimized(ebs_optimized);
        }
        let answer = match request.send().await {
            Ok(answer) => answer,
            Err(error) => {
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    instance_type = %launch.instance_type,
                    "Amazon EC2 launched no instances for this collection"
                );
                continue;
            }
        };
        let mut minted: Vec<String> = Vec::new();
        let mut from_this_launch: Vec<LaunchedInstance> = Vec::new();
        for instance in answer.instances() {
            let Some(ec2_instance_id) = instance.instance_id() else {
                continue;
            };
            minted.push(ec2_instance_id.to_string());
            from_this_launch.push(LaunchedInstance {
                ec2_instance_id: ec2_instance_id.to_string(),
                instance_group_id: launch.instance_group_id.clone(),
                instance_fleet_id: launch.instance_fleet_id.clone(),
                // The type and the market are the COLLECTION's: the launch asked for that
                // type, and the market is the purchasing option this service provisions the
                // collection under.
                instance_type: instance
                    .instance_type()
                    .map_or_else(|| launch.instance_type.clone(), |kind| kind.as_str().to_string()),
                market: launch.market.clone(),
                private_ip_address: instance.private_ip_address().map(str::to_string),
                private_dns_name: instance.private_dns_name().map(str::to_string),
                availability_zone: instance
                    .placement()
                    .and_then(|placement| placement.availability_zone())
                    .map(str::to_string),
                // Filled in from Amazon EC2's own answer below.
                ebs_volumes: Vec::new(),
            });
        }
        // WHICH volume Amazon EC2 created for each device this launch mapped, asked of the
        // service that owns volumes right where the launch that asked for them was made (see
        // [`launch_ebs_volumes`]). The ids it answers are what `Instance$EbsVolumes` reports,
        // so no volume is reported that Amazon EC2 does not hold.
        let volumes = launch_ebs_volumes(&client, &minted, &launch.ebs_devices).await;
        let mut volume_ids: Vec<String> = Vec::new();
        for instance in &mut from_this_launch {
            instance.ebs_volumes = volumes
                .get(&instance.ec2_instance_id)
                .cloned()
                .unwrap_or_default();
            volume_ids.extend(instance.ebs_volumes.iter().map(|volume| volume.volume_id.clone()));
        }
        launched.extend(from_this_launch);
        // The instances Amazon EMR just launched — and the volumes the launch created for
        // them — carry the cluster they belong to and the role they fill, plus the cluster's
        // own tags it propagates to them.
        let mut instance_tags = vec![
            (CLUSTER_ID_TAG.to_string(), placement.cluster_id.clone()),
            (INSTANCE_ROLE_TAG.to_string(), launch.role.clone()),
        ];
        instance_tags.extend(placement.tags.iter().cloned());
        let mut tagged = minted;
        tagged.extend(volume_ids);
        tag_cluster_resources(&client, &tagged, &instance_tags).await;
    }
    launched
}

/// The Amazon EBS volumes Amazon EC2 created for the instances `ec2_instance_ids` at the
/// devices `devices` maps, as `instance id -> volumes in device order`.
///
/// The launch asked Amazon EC2 for one volume per mapped device, so this asks Amazon EC2 which
/// volume it created for each: ONE read of the volumes attached to those instances —
/// "attachment.instance-id - The ID of the instance the volume is attached to"
/// (`DescribeVolumes$Filters` doc, Amazon EC2 model) — filtered to the devices the collection
/// mapped, so the cluster's own root volume, which no `EbsConfiguration` asked for, is not
/// among them. A device Amazon EC2 reports no volume at contributes NONE, which is what keeps
/// every `VolumeId` this service reports a volume `DescribeVolumes` resolves; so does a launch
/// that mapped no device at all, which is not asked about.
///
/// `client` is the client the launch itself was made with: the in-process callback every
/// emulated service reaches another through, which this module's header states the reading of
/// — no socket is opened, nothing outside this process is reachable, and the credentials are
/// the emulator's own.
async fn launch_ebs_volumes(
    client: &::aws_sdk_ec2::Client,
    ec2_instance_ids: &[String],
    devices: &[EbsBlockDevice],
) -> ::std::collections::BTreeMap<String, Vec<InstanceEbsVolume>> {
    let mut held: ::std::collections::BTreeMap<String, Vec<InstanceEbsVolume>> =
        ::std::collections::BTreeMap::new();
    let mapped: Vec<&str> = devices
        .iter()
        .filter_map(|device| device.device.as_deref())
        .collect();
    if mapped.is_empty() || ec2_instance_ids.is_empty() {
        return held;
    }
    let answer = client
        .describe_volumes()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name(VOLUME_ATTACHMENT_INSTANCE_FILTER)
                .set_values(Some(ec2_instance_ids.to_vec()))
                .build(),
        )
        .send()
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Amazon EC2 reported no Amazon EBS volumes for this collection's instances"
            );
            return held;
        }
    };
    for ec2_instance_id in ec2_instance_ids {
        // One entry per device the collection mapped, in the collection's own device order,
        // which is the order the devices are exposed on the node.
        let volumes: Vec<InstanceEbsVolume> = mapped
            .iter()
            .filter_map(|device_name| {
                let volume_id = answer.volumes().iter().find_map(|volume| {
                    let attached_here = volume.attachments().iter().any(|attachment| {
                        attachment.instance_id() == Some(ec2_instance_id.as_str())
                            && attachment.device() == Some(*device_name)
                    });
                    attached_here.then(|| volume.volume_id()).flatten()
                })?;
                Some(InstanceEbsVolume {
                    device: (*device_name).to_string(),
                    volume_id: volume_id.to_string(),
                })
            })
            .collect();
        if !volumes.is_empty() {
            held.insert(ec2_instance_id.clone(), volumes);
        }
    }
    held
}

/// The Amazon EC2 volume filter naming the instance a volume is attached to:
/// "attachment.instance-id - The ID of the instance the volume is attached to"
/// (`DescribeVolumes$Filters` doc, Amazon EC2 model).
const VOLUME_ATTACHMENT_INSTANCE_FILTER: &str = "attachment.instance-id";
