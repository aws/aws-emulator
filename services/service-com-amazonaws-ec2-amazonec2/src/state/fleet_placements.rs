// SPDX-License-Identifier: CC0-1.0

//! How an EC2 Fleet's capacity becomes instances: the words a fleet's capacity is described
//! with, the pools it places that capacity in, and the one launch of an instance from a
//! modelled launch specification that a fleet and a Spot Instance request both go through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.
//!
//! The launch is in ONE place ([`ComAmazonawsEc2Amazonec2State::launch_specified_instance`])
//! because two things in this API place instances from a launch specification — a Spot
//! Instance request the clock has fulfilled, and an EC2 Fleet placing its target capacity —
//! and an instance either places has to be the same kind of instance: one reservation of its
//! own, `pending` with the clock owing it `running`, with the type, AMI, subnet, zone,
//! kernel, RAM disk and monitoring state the specification named. Nothing here reports a
//! fleet: what a fleet reports about its instances is read back out of the INSTANCE store
//! ([`fleet_instance_groups`]), so a fleet cannot name an instance the store does not hold.

use super::*;
use crate::ops::types::{RunInstancesMonitoringEnabled, SpotPlacement};

/// `InstanceLifecycle`'s `on-demand` member: an instance the fleet launched directly rather
/// than through a Spot Instance request
/// ([`crate::ops::types::INSTANCE_LIFECYCLE_VALUES`]).
pub const INSTANCE_LIFECYCLE_ON_DEMAND: &str = "on-demand";

/// `DefaultTargetCapacityType`'s `spot` member — the capacity type a fleet's unassigned
/// target capacity is placed as: EC2 "calculates the difference between the total capacity
/// and On-Demand capacity, and launches the difference as Spot capacity"
/// (`RequestSpotFleet`), the same rule an EC2 Fleet's total, On-Demand and Spot target
/// capacities stand in. The generated catalog is
/// [`crate::ops::types::DEFAULT_TARGET_CAPACITY_TYPE_VALUES`].
pub const DEFAULT_TARGET_CAPACITY_TYPE_SPOT: &str = "spot";

/// `DefaultTargetCapacityType`'s `on-demand` member.
pub const DEFAULT_TARGET_CAPACITY_TYPE_ON_DEMAND: &str = "on-demand";

/// `DefaultTargetCapacityType`'s `capacity-block` member: capacity drawn from a Capacity
/// Block the launch template names, which is not a capacity source this store's launch path
/// places.
pub const DEFAULT_TARGET_CAPACITY_TYPE_CAPACITY_BLOCK: &str = "capacity-block";

/// `DefaultTargetCapacityType`'s `reserved-capacity` member: capacity drawn from a
/// Capacity Reservation named by `ReservedCapacityOptions`, which is not a capacity source
/// this store's launch path places either.
pub const DEFAULT_TARGET_CAPACITY_TYPE_RESERVED_CAPACITY: &str = "reserved-capacity";

/// `TargetCapacityUnitType`'s `units` member — "Default: `units` (the number of instances)"
/// (`TargetCapacitySpecificationRequest$TargetCapacityUnitType`). The generated catalog is
/// [`crate::ops::types::TARGET_CAPACITY_UNIT_TYPE_VALUES`].
pub const TARGET_CAPACITY_UNIT_TYPE_UNITS: &str = "units";

/// `SpotAllocationStrategy`'s `diversified` member: the fleet spreads its Spot capacity
/// across every pool its launch template configurations describe, rather than drawing it
/// from one. The generated catalog is
/// [`crate::ops::types::SPOT_ALLOCATION_STRATEGY_VALUES`].
pub const SPOT_ALLOCATION_STRATEGY_DIVERSIFIED: &str = "diversified";

/// `SpotAllocationStrategy`'s `lowest-price` member, which is also what a fleet that names
/// no Spot allocation strategy gets.
pub const SPOT_ALLOCATION_STRATEGY_LOWEST_PRICE: &str = "lowest-price";

/// `FleetOnDemandAllocationStrategy`'s `lowest-price` member — "`lowest-price` - EC2 Fleet
/// uses price to determine the order, launching the lowest price first … Default:
/// `lowest-price`" (`OnDemandOptionsRequest$AllocationStrategy`).
pub const FLEET_ON_DEMAND_ALLOCATION_STRATEGY_LOWEST_PRICE: &str = "lowest-price";

/// `FleetOnDemandAllocationStrategy`'s `prioritized` member — "EC2 Fleet uses the priority
/// that you assigned to each launch template override, launching the highest priority
/// first" (`OnDemandOptionsRequest$AllocationStrategy`).
pub const FLEET_ON_DEMAND_ALLOCATION_STRATEGY_PRIORITIZED: &str = "prioritized";

/// The priority a launch template override that names none is read with: "If no number is
/// set, the launch template override has the lowest priority"
/// (`FleetLaunchTemplateOverridesRequest$Priority`), and a lower number is a higher
/// priority — so the lowest priority there is, is the largest number an override may carry.
pub const LOWEST_FLEET_OVERRIDE_PRIORITY: f64 = f64::MAX;

/// The most instances one EC2 Fleet places in this emulator.
///
/// Neither `TotalTargetCapacity` nor the weighted capacity of a pool carries a modelled
/// bound, so a fleet's target capacity can ask for more instances than any store will hold:
/// a fleet places at most this many and reports the rest of its target as not yet
/// fulfilled, which is what EC2 reports for a fleet below its target. The same ceiling one
/// `RunInstances` call has ([`MAX_INSTANCES_PER_RUN`]), because a fleet's placement is the
/// same launch.
pub const MAX_FLEET_INSTANCES: usize = MAX_INSTANCES_PER_RUN as usize;

/// The most pools one `CreateFleet` request may describe: "For fleets of type `request` and
/// `maintain`, a maximum of 300 items is allowed across all launch templates"
/// (`FleetLaunchTemplateConfigRequest$Overrides`).
pub const MAX_FLEET_POOLS: usize = 300;

/// The largest minimum target capacity a fleet's Spot or On-Demand options may ask for:
/// "Constraints: Maximum value of `1000`" (`SpotOptionsRequest$MinTargetCapacity`, and the
/// same sentence on `OnDemandOptionsRequest$MinTargetCapacity`).
pub const MAX_FLEET_MIN_TARGET_CAPACITY: i32 = 1000;

/// One pool of an EC2 Fleet as the FLEET reports it: the launch template and the overrides
/// the pool's instances were launched from, and whether they are Spot or On-Demand
/// instances.
///
/// Held once per pool on the fleet record ([`FleetRecord::pools`]) and named by index from
/// each instance ([`FleetInstanceRecord::pool`]), because that is the shape every report of
/// a fleet's instances takes: "the launch templates and overrides that were used for
/// launching the instances" beside "the IDs of the instances"
/// (`CreateFleetInstance$LaunchTemplateAndOverrides` / `$InstanceIds`).
#[derive(Debug, Clone)]
pub struct FleetPoolView {
    /// `CreateFleetInstance$LaunchTemplateAndOverrides` — "the values that you specify in
    /// the Overrides replace the values in the launch template".
    pub launch_template_and_overrides: Option<crate::ops::types::LaunchTemplateAndOverridesResponse>,
    /// `CreateFleetInstance$Lifecycle`: [`INSTANCE_LIFECYCLE_SPOT`] or
    /// [`INSTANCE_LIFECYCLE_ON_DEMAND`], the member of the modelled `InstanceLifecycle`
    /// catalog this pool's capacity type names.
    pub lifecycle: String,
    /// How many instances the fleet asked this pool for, which is the pool's share of the
    /// target capacity ([`fleet_placement_counts`]).
    ///
    /// Stored beside what the pool launched so both reports of a fleet can tell the
    /// instances it placed from the ones it could not: "information about the instances that
    /// could not be launched by the fleet" (`CreateFleetResult$Errors`) is exactly this
    /// number less the instances of the pool the instance store holds
    /// ([`ComAmazonawsEc2Amazonec2State::fleet_launch_errors`]).
    pub planned: usize,
}

impl FleetPoolView {
    /// The caller-chosen bytes this pool holds: the launch template it names, the version
    /// and the override values a request chose. Charged by [`fleet_payload_bytes`], the one
    /// definition of a fleet's charge.
    pub fn payload_bytes(&self) -> usize {
        let text = |value: Option<&String>| value.map_or(0, String::len);
        let Some(held) = self.launch_template_and_overrides.as_ref() else {
            return 0;
        };
        let specification = held
            .launch_template_specification
            .as_ref()
            .map_or(0, |specification| {
                text(specification.launch_template_id.as_ref())
                    .saturating_add(text(specification.launch_template_name.as_ref()))
                    .saturating_add(text(specification.version.as_ref()))
            });
        let overrides = held.overrides.as_ref().map_or(0, |overridden| {
            text(overridden.instance_type.as_ref())
                .saturating_add(text(overridden.availability_zone.as_ref()))
                .saturating_add(text(overridden.availability_zone_id.as_ref()))
                .saturating_add(text(overridden.subnet_id.as_ref()))
                .saturating_add(text(overridden.max_price.as_ref()))
                .saturating_add(text(overridden.image_id.as_ref()))
        });
        specification.saturating_add(overrides)
    }
}

/// One pool of an EC2 Fleet as the handler serving a create has already resolved it: the
/// launch its instances are placed from, what one of them provides, how it is ordered
/// against the other pools, and how many instances it places.
#[derive(Debug, Clone)]
pub struct FleetPoolPlacement {
    /// The launch every instance of this pool is placed from, as the modelled
    /// [`LaunchSpecification`] a Spot Instance request also carries — resolved from the
    /// launch template version the pool names and the overrides that narrow it, so the
    /// store holds one shape however a fleet described the launch.
    pub launch: LaunchSpecification,
    /// "The number of units provided by the specified instance type … If this value is not
    /// specified, the default is 1" (`FleetLaunchTemplateOverridesRequest$WeightedCapacity`,
    /// carried here as [`DEFAULT_FLEET_INSTANCE_WEIGHT`]).
    pub weighted_capacity: f64,
    /// "The priority for the launch template override. The highest priority is launched
    /// first … The lower the number, the higher the priority" (`$Priority`), or
    /// [`LOWEST_FLEET_OVERRIDE_PRIORITY`] for a pool that names none.
    pub priority: f64,
    /// "The maximum price per unit hour that you are willing to pay for a Spot Instance"
    /// (`$MaxPrice`), or `None` for a pool that names none — which pays the current Spot
    /// price.
    pub max_price: Option<String>,
    /// What the fleet reports about this pool, stored with it.
    pub view: FleetPoolView,
    /// How many instances this pool places ([`fleet_placement_counts`]).
    pub count: usize,
}

impl FleetPoolPlacement {
    /// Whether this pool's instances are Spot Instances, which are placed through a Spot
    /// Instance request of this account rather than launched directly.
    pub fn is_spot(&self) -> bool {
        self.view.lifecycle == INSTANCE_LIFECYCLE_SPOT
    }
}

/// What one EC2 Fleet's placement is made of, for
/// [`ComAmazonawsEc2Amazonec2State::place_fleet_instances`].
#[derive(Debug, Clone)]
pub struct FleetPlacementSpec<'a> {
    /// The pools to place, in the order the request described them, each carrying the number
    /// of instances it places.
    pub pools: &'a [FleetPoolPlacement],
    /// The Region of the request's credential scope: the Region the instances are placed in,
    /// and the one a pool's zone was resolved against.
    pub region: &'a str,
    /// `SpotInstanceRequest$Type` for the requests a Spot pool places:
    /// [`SPOT_INSTANCE_TYPE_PERSISTENT`] for a fleet that maintains its capacity,
    /// [`SPOT_INSTANCE_TYPE_ONE_TIME`] for one that does not.
    pub spot_request_type: &'a str,
    /// `SpotInstanceRequest$InstanceInterruptionBehavior` for those requests.
    pub interruption_behavior: &'a str,
    /// The fleet's `ValidUntil`, after which "no new EC2 Fleet requests are placed or able
    /// to fulfill the request" (`CreateFleetRequest$ValidUntil`) — carried onto every Spot
    /// request the fleet places, so a request of a fleet whose window has closed expires
    /// exactly as one a caller placed itself does.
    pub valid_until: Option<SystemTime>,
    /// The tags the fleet's `instance` tag specifications assign to every instance it
    /// launches.
    pub instance_tags: &'a BTreeMap<String, String>,
}

/// How many instances each pool places to cover `target` capacity units, given each pool's
/// weighted capacity, never more than `cap` instances in total.
///
/// "If the target capacity divided by this value is not a whole number, Amazon EC2 rounds
/// the number of instances to the next whole number"
/// (`FleetLaunchTemplateOverridesRequest$WeightedCapacity`) — so a pool's share is divided
/// by what one of its instances provides and rounded UP. `spread` says whether the target is
/// spread across every pool (`diversified` Spot capacity) or drawn from the first pool the
/// request gave, which is the order every other member of a launch template configuration is
/// read in: this emulator holds no price history, so a "lowest price" pool cannot be told
/// from the others.
///
/// The arithmetic is [`spot_fleet_placement_counts`], the one allocation of a target capacity
/// across weighted pools this crate has, so an EC2 Fleet and a Spot Fleet request cannot
/// divide one target two ways.
pub fn fleet_placement_counts(
    weights: &[f64],
    target: f64,
    spread: bool,
    cap: usize,
) -> Vec<usize> {
    spot_fleet_placement_counts(weights, target, spread, cap)
}

/// The values a launch template override replaces in the template version it narrows.
///
/// "Any parameters that you specify override the same parameters in the launch template"
/// (`FleetLaunchTemplateConfigRequest$Overrides`), so these are the members the OVERRIDE
/// decides and everything else about the launch comes from the template version.
#[derive(Debug, Clone, Default)]
pub struct LaunchOverrides<'a> {
    /// "The instance type" (`FleetLaunchTemplateOverridesRequest$InstanceType`).
    pub instance_type: Option<&'a str>,
    /// "The IDs of the subnets in which to launch the instances" (`$SubnetId`).
    pub subnet_id: Option<&'a str>,
    /// "The Availability Zone in which to launch the instances" (`$AvailabilityZone`), as
    /// the Region's own zone name.
    pub availability_zone: Option<&'a str>,
    /// "The name of the key pair to use for the instances" (`$KeyName`).
    pub key_name: Option<&'a str>,
    /// "The ID of the AMI" (`$ImageId`), already resolved against the template's own.
    pub image_id: Option<&'a str>,
}

/// The launch one pool places, from the launch template version `data` and the values `over`
/// replaces in it, as the modelled [`LaunchSpecification`] every placement of this crate
/// takes.
///
/// The members this emulator's launch path models are carried onto the instance; the ones it
/// does not — block device mappings, network interface specifications and an IAM instance
/// profile — are left off rather than copied onto an instance nothing would apply them to.
/// Nothing a caller set is lost by that: a fleet that names one of those override members is
/// refused rather than answered with an instance that does not carry it.
pub fn launch_from_template_data(
    data: &RequestLaunchTemplateData,
    over: &LaunchOverrides<'_>,
) -> LaunchSpecification {
    let zone = over.availability_zone.map(str::to_string).or_else(|| {
        data.placement
            .as_ref()
            .and_then(|placement| placement.availability_zone.clone())
    });
    let groups: Vec<GroupIdentifier> = data
        .security_group_ids
        .iter()
        .flatten()
        .map(|group_id| GroupIdentifier {
            group_id: Some(group_id.clone()),
            group_name: None,
        })
        .chain(
            data.security_groups
                .iter()
                .flatten()
                .map(|group_name| GroupIdentifier {
                    group_id: None,
                    group_name: Some(group_name.clone()),
                }),
        )
        .collect();
    LaunchSpecification {
        image_id: over
            .image_id
            .map(str::to_string)
            .or_else(|| data.image_id.clone()),
        instance_type: over
            .instance_type
            .map(str::to_string)
            .or_else(|| data.instance_type.clone()),
        key_name: over
            .key_name
            .map(str::to_string)
            .or_else(|| data.key_name.clone()),
        subnet_id: over.subnet_id.map(str::to_string),
        user_data: data.user_data.clone(),
        addressing_type: None,
        kernel_id: data.kernel_id.clone(),
        ramdisk_id: data.ram_disk_id.clone(),
        ebs_optimized: Some(data.ebs_optimized.unwrap_or(false)),
        monitoring: data
            .monitoring
            .as_ref()
            .map(|monitoring| RunInstancesMonitoringEnabled {
                enabled: monitoring.enabled.unwrap_or(false),
            }),
        placement: zone.map(|zone| SpotPlacement {
            availability_zone: Some(zone),
            group_name: data
                .placement
                .as_ref()
                .and_then(|placement| placement.group_name.clone()),
            tenancy: None,
            availability_zone_id: None,
        }),
        security_groups: Some(groups),
        block_device_mappings: None,
        iam_instance_profile: None,
        network_interfaces: None,
    }
}

/// What one launch of an instance from a modelled [`LaunchSpecification`] decides, for
/// [`ComAmazonawsEc2Amazonec2State::launch_specified_instance`].
#[derive(Debug, Clone)]
pub struct SpecifiedLaunch<'a> {
    /// The launch itself: the AMI, the instance type, the subnet, the zone, the key pair,
    /// the user data, the security groups and the monitoring state of the instance.
    pub launch: &'a LaunchSpecification,
    /// The Region the instance is placed in, which decides the zone a launch that names
    /// none lands in and the private DNS name the instance gets.
    pub region: &'a str,
    /// The wall-clock instant the instance was launched at — the launch time it reports and
    /// the instant its guest's first boot is counted from.
    pub at: SystemTime,
    /// The Spot Instance request the instance was placed through, or `None` for an
    /// On-Demand launch.
    pub spot_instance_request_id: Option<&'a str>,
    /// The tags the launch assigns to the instance.
    pub tags: BTreeMap<String, String>,
}

/// One instance a specified launch placed.
#[derive(Debug, Clone)]
pub struct LaunchedInstance {
    /// The `InstanceId` the store minted.
    pub instance_id: String,
    /// The Availability Zone the instance was placed in.
    pub availability_zone: String,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Launch the one instance `spec` describes, and answer what was launched — or `None`
    /// when this store has no room for it: no instance slot
    /// ([`MAX_STORED_INSTANCES`]) or no private address left, and NOTHING was written.
    ///
    /// The ONE launch of an instance from a launch specification, so a Spot Instance request
    /// the clock has fulfilled and an EC2 Fleet placing its capacity cannot produce two
    /// different kinds of instance. Every launch parameter comes off the specification, so
    /// the instance is the one the caller described, and the instance is `pending` with the
    /// clock owing it `running` — the same move every launch of this crate owes.
    ///
    /// The instance's own byte charge is made by [`Self::insert_instance`]; the caller has
    /// already cleared the store's byte ceiling for the record it asked for.
    pub fn launch_specified_instance(
        &mut self,
        spec: SpecifiedLaunch<'_>,
    ) -> Option<LaunchedInstance> {
        let launch = spec.launch;
        let at = spec.at;
        // "The Availability Zone in which to launch the instances" when the launch named
        // one; otherwise the first zone the Region has, which is where a launch that names
        // no zone and no subnet lands.
        let availability_zone = launch
            .placement
            .as_ref()
            .and_then(|placement| placement.availability_zone.clone())
            .or_else(|| {
                availability_zones_in_region(spec.region)
                    .first()
                    .map(|zone| zone.zone_name.clone())
            })
            .unwrap_or_else(|| spec.region.to_string());

        if self.instances.len() >= MAX_STORED_INSTANCES {
            return None;
        }
        let private_ip = self.allocate_private_ip()?;

        // An instance a request or a fleet places is its own reservation: one instance
        // launched by one placement.
        let reservation_id = self.mint_reservation_id();
        self.insert_reservation(ReservationRecord {
            reservation_id: reservation_id.clone(),
            owner_id: ::engine::emulator_account_id().to_string(),
            requester_id: None,
            instance_ids: Vec::new(),
        });
        let instance_id = self.mint_instance_id();
        let (image_kernel_id, image_ramdisk_id) =
            self.image_kernel_and_ramdisk(launch.image_id.as_deref());
        let instance_type = launch
            .instance_type
            .clone()
            .unwrap_or_else(|| DEFAULT_INSTANCE_TYPE.to_string());
        self.insert_instance(InstanceRecord {
            instance_id: instance_id.clone(),
            reservation_id,
            image_id: launch.image_id.clone(),
            instance_type: instance_type.clone(),
            key_name: launch.key_name.clone(),
            subnet_id: launch.subnet_id.clone(),
            security_groups: launch
                .security_groups
                .iter()
                .flatten()
                .map(|group| (group.group_id.clone(), group.group_name.clone()))
                .collect(),
            private_dns_name: private_dns_name(&private_ip, spec.region),
            private_ip,
            // A public address comes from a default-VPC subnet or an Elastic IP, neither of
            // which this slice models — the same answer `RunInstances` gives.
            public_ip: None,
            // An instance AWS has just launched is `pending`; the clock owes it `running`,
            // on the same span as every other transition of this crate.
            state: InstanceStateName::Pending,
            pending_transition: Some(PendingTransition::starting_now(InstanceStateName::Running)),
            state_transition_reason: None,
            state_reason: None,
            // Nobody has asked to reboot an instance that has just been launched, and
            // nobody has interrupted it either.
            queued_reboots: 0,
            last_reboot_queued_at: None,
            diagnostic_interrupts: 0,
            last_diagnostic_interrupt_at: None,
            // The guest's FIRST boot: the operating system comes up as the instance finishes
            // starting, the same instant the `pending` -> `running` move above falls due.
            boots: Vec::new(),
            // The guest's own storage starts empty: what is on it is what the instance's
            // user-data program puts there, boot by boot.
            guest_files: ::std::collections::BTreeMap::new(),
            boot_due: Some(at + TRANSITION_DELAY),
            // The instance was launched when the placement fell due, not when somebody next
            // asked about it.
            launch_time: at,
            ami_launch_index: 0,
            user_data: launch.user_data.clone(),
            placement_az: availability_zone.clone(),
            monitoring_state: if launch
                .monitoring
                .as_ref()
                .is_some_and(|monitoring| monitoring.enabled)
            {
                MONITORING_STATE_ENABLED.to_string()
            } else {
                MONITORING_STATE_DISABLED.to_string()
            },
            // The launch specification says whether monitoring is on from the start, so
            // there is no move for the clock to make.
            monitoring_transition: None,
            // A launch specification configures none of the option sets an instance
            // reports, so every one of them is at its documented default.
            detail: InstanceDetail::default(),
            architecture: architecture_for(&instance_type),
            // The launch specification's kernel and RAM disk, falling back to the ones the
            // AMI carries — the same resolution `RunInstances` makes.
            kernel_id: launch.kernel_id.clone().or(image_kernel_id),
            ramdisk_id: launch.ramdisk_id.clone().or(image_ramdisk_id),
            // Nothing in a launch specification sets source/destination checking, so the
            // instance starts at the documented default.
            source_dest_check: DEFAULT_SOURCE_DEST_CHECK,
            ebs_optimized: launch.ebs_optimized.unwrap_or(false),
            // A launch specification sets no enhanced networking of the instance's own, so
            // it reports the AMI's until `ModifyInstanceAttribute` gives it a value.
            sriov_net_support: None,
            ena_support: None,
            // Hibernation is `InstanceInterruptionBehavior`'s business for a Spot Instance,
            // and a launch specification carries no hibernation member.
            hibernation_configured: false,
            // A launch specification's block device mappings are refused by the paths that
            // read one ("`RequestSpotInstances` with a block device mapping"), so no launch
            // reaching here overrides its AMI's root device.
            root_block_device: None,
            disable_api_termination: false,
            tags: spec.tags,
            client_token: None,
            spot_instance_request_id: spec.spot_instance_request_id.map(str::to_string),
        });
        Some(LaunchedInstance {
            instance_id,
            availability_zone,
        })
    }

    /// Place the instances every pool of one EC2 Fleet asks for, and answer the ones that
    /// were placed, in pool order.
    ///
    /// The CALL places them: "instances are launched immediately if there is available
    /// capacity" (`CreateFleet`), so the fleet's instances are facts of this write and the
    /// only thing left for the clock is each instance's own `pending` -> `running` move. A
    /// Spot pool's instances are placed through a Spot Instance request of this account —
    /// "the ID of the Spot Instance request" is what a fleet reports for one
    /// (`ActiveInstance$SpotInstanceRequestId`) — minted and fulfilled through the one
    /// writer of a Spot Instance request, so a fleet never names a request this store does
    /// not hold. An On-Demand pool's instances are launched directly, through the one launch
    /// both paths share ([`Self::launch_specified_instance`]).
    ///
    /// A pool this store has no room to place an instance for stops there: the fleet reports
    /// the capacity it DID place and its target as not yet fulfilled, which is what EC2
    /// reports for a fleet short of its target — never capacity nothing was launched for.
    pub fn place_fleet_instances(&mut self, spec: FleetPlacementSpec<'_>) -> Vec<FleetInstanceRecord> {
        let mut placed: Vec<FleetInstanceRecord> = Vec::new();
        for (index, pool) in spec.pools.iter().enumerate() {
            for _ in 0..pool.count {
                let instance = if pool.is_spot() {
                    self.place_fleet_spot_instance(&spec, pool, index)
                } else {
                    self.launch_specified_instance(SpecifiedLaunch {
                        launch: &pool.launch,
                        region: spec.region,
                        at: SystemTime::now(),
                        spot_instance_request_id: None,
                        tags: spec.instance_tags.clone(),
                    })
                    .map(|launched| FleetInstanceRecord {
                        instance_id: launched.instance_id,
                        spot_instance_request_id: None,
                        pool: index,
                    })
                };
                match instance {
                    Some(instance) => placed.push(instance),
                    // No room for another instance in this store: the pools that follow have
                    // none either, so the fleet places what it has and reports the rest of
                    // its target as unfulfilled.
                    None => return placed,
                }
            }
        }
        placed
    }

    /// Place one Spot Instance of a fleet: mint the Spot Instance request the fleet places
    /// it through, fulfil it at once, and answer the instance that fulfilment launched.
    ///
    /// The request is an ordinary Spot Instance request of this account, stored and fulfilled
    /// through the one writer and the one fulfilment every Spot Instance request goes
    /// through, so `DescribeSpotInstanceRequests` reports it exactly as it reports a request
    /// a caller placed, and the instance it launched is an instance of the instance store. A
    /// request this store could not launch for stays `open` carrying
    /// [`SPOT_STATUS_CAPACITY_NOT_AVAILABLE`] — the answer the real service gives for a
    /// request it cannot place — and the fleet counts no instance for it.
    fn place_fleet_spot_instance(
        &mut self,
        spec: &FleetPlacementSpec<'_>,
        pool: &FleetPoolPlacement,
        index: usize,
    ) -> Option<FleetInstanceRecord> {
        if !self.has_spot_instance_request_capacity(1) {
            return None;
        }
        let created_at = SystemTime::now();
        let valid_until = spec.valid_until.map(system_time_to_date_time);
        // "If no value is specified, the request remains until you cancel it"
        // (`CreateFleetRequest$ValidUntil`): a fleet with no end date places requests that do
        // not expire on their own, so their validity runs to this module's clock horizon.
        let valid_until_at = spec
            .valid_until
            .map_or_else(|| due_instant(MAX_CLOCK_HORIZON), |at| {
                due_instant(duration_until(at))
            });
        let product_description = self
            .spot_product_description(pool.launch.image_id.as_deref())
            .to_string();
        let view = SpotInstanceRequest {
            spot_instance_request_id: None,
            state: Some(SPOT_REQUEST_STATE_OPEN.to_string()),
            status: Some(SpotInstanceStatus {
                code: Some(SPOT_STATUS_PENDING_EVALUATION.to_string()),
                message: Some(SPOT_STATUS_MESSAGE_PENDING_EVALUATION.to_string()),
                update_time: Some(system_time_to_date_time(created_at)),
            }),
            create_time: Some(system_time_to_date_time(created_at)),
            r#type: Some(spec.spot_request_type.to_string()),
            instance_interruption_behavior: Some(spec.interruption_behavior.to_string()),
            valid_from: None,
            valid_until,
            // A fleet's requests are launched and terminated as the fleet decides, not as a
            // launch group: neither member is a fleet's to set.
            launch_group: None,
            availability_zone_group: None,
            spot_price: pool.max_price.clone(),
            product_description: Some(product_description),
            launch_specification: Some(pool.launch.clone()),
            // The fleet's `fleet` tags tag the FLEET and its `instance` tags the instances;
            // nothing tags the request the fleet placed.
            tags: Some(Vec::new()),
            instance_id: None,
            launched_availability_zone: None,
            launched_availability_zone_id: None,
            // "Deprecated" (`SpotInstanceRequest$BlockDurationMinutes` and
            // `ActualBlockHourlyPrice`): Spot blocks are not offered any more.
            block_duration_minutes: None,
            actual_block_hourly_price: None,
            fault: None,
        };
        let request_id = self.insert_spot_instance_request(
            view,
            Some(SpotTransition::due_in(Duration::ZERO)),
            valid_until_at,
            spec.region,
            None,
        );
        // The fleet's capacity is placed BY THIS CALL, so the request it placed is fulfilled
        // here rather than left for the clock: the instance the fleet reports has to exist
        // the instant the fleet does.
        self.fulfill_spot_instance_request(&request_id, spec.instance_tags.clone());
        let instance_id = self
            .spot_instance_requests
            .get(&request_id)?
            .view
            .instance_id
            .clone()?;
        Some(FleetInstanceRecord {
            instance_id,
            spot_instance_request_id: Some(request_id),
            pool: index,
        })
    }

    /// Every group of instances one fleet placed, as every report of a fleet's instances
    /// describes them: the pool they came from, what they are, and their ids.
    ///
    /// Projected from the INSTANCE store rather than from anything the fleet record copies,
    /// so the type, the zone and the subnet a fleet reports for an instance are the ones
    /// `DescribeInstances` reports for it, and an instance the store no longer holds is not
    /// reported at all. The groups are the fleet's POOLS — "the launch templates and
    /// overrides that were used for launching the instances" beside "the IDs of the
    /// instances" (`CreateFleetInstance`) — in pool order, so one entry carries every
    /// instance of one pool.
    pub fn fleet_instance_groups(&self, record: &FleetRecord) -> Vec<FleetInstanceGroup> {
        let mut groups: Vec<FleetInstanceGroup> = Vec::new();
        for launched in &record.instances {
            let Some(instance) = self.instances.get(&launched.instance_id) else {
                continue;
            };
            let pool = record.pools.get(launched.pool);
            if let Some(group) = groups
                .iter_mut()
                .find(|group| group.pool_index == launched.pool)
            {
                group.instance_ids.push(instance.instance_id.clone());
                continue;
            }
            groups.push(FleetInstanceGroup {
                pool_index: launched.pool,
                launch_template_and_overrides: pool
                    .and_then(|pool| pool.launch_template_and_overrides.clone()),
                // A pool's lifecycle is what the pool was placed as; for a fleet stored with
                // no pools at all, the Spot request the instance was placed through is what
                // says whether it is a Spot Instance.
                lifecycle: Some(pool.map_or_else(
                    || {
                        if launched.spot_instance_request_id.is_some() {
                            INSTANCE_LIFECYCLE_SPOT.to_string()
                        } else {
                            INSTANCE_LIFECYCLE_ON_DEMAND.to_string()
                        }
                    },
                    |pool| pool.lifecycle.clone(),
                )),
                instance_type: Some(instance.instance_type.clone()),
                // "The value is `windows` for Windows instances in an EC2 Fleet. Otherwise,
                // the value is blank" (`CreateFleetInstance$Platform`), read off the AMI the
                // instance was launched from through the one question this crate asks about
                // an image's platform.
                platform: self
                    .fleet_instance_platform(instance.image_id.as_deref())
                    .map(str::to_string),
                availability_zone: Some(instance.placement_az.clone()),
                subnet_id: instance.subnet_id.clone(),
                instance_ids: vec![instance.instance_id.clone()],
            });
        }
        groups
    }

    /// The launches one fleet could not make, as every report of a fleet's launch errors
    /// describes them.
    ///
    /// A pool the fleet asked for more instances than the instance store now holds for it is
    /// capacity this service could not provide, reported with EC2's own code for exactly
    /// that ([`INSUFFICIENT_INSTANCE_CAPACITY`]). Derived from the SAME two facts a report of
    /// the fleet's instances is — what each pool was asked for
    /// ([`FleetPoolView::planned`]) and which of its instances the store holds — so a create
    /// and a later describe of one fleet cannot disagree about which launches failed.
    pub fn fleet_launch_errors(&self, record: &FleetRecord) -> Vec<FleetLaunchError> {
        record
            .pools
            .iter()
            .enumerate()
            .filter_map(|(index, pool)| {
                let placed = record
                    .instances
                    .iter()
                    .filter(|launched| {
                        launched.pool == index && self.instances.contains_key(&launched.instance_id)
                    })
                    .count();
                (placed < pool.planned).then(|| FleetLaunchError {
                    launch_template_and_overrides: pool.launch_template_and_overrides.clone(),
                    lifecycle: Some(pool.lifecycle.clone()),
                    error_code: Some(INSUFFICIENT_INSTANCE_CAPACITY.to_string()),
                    error_message: Some(INSUFFICIENT_INSTANCE_CAPACITY_MESSAGE.to_string()),
                })
            })
            .collect()
    }

    /// The `PlatformValues` member a fleet reports for an instance launched from `image_id`:
    /// "the value is `windows` for Windows instances in an EC2 Fleet. Otherwise, the value is
    /// blank" (`CreateFleetInstance$Platform`).
    ///
    /// Read off the AMI's own `Image$Platform`, the one place this crate records that an
    /// image is a Windows one, so a fleet's report of an instance's platform cannot disagree
    /// with `DescribeImages`.
    pub fn fleet_instance_platform(&self, image_id: Option<&str>) -> Option<&'static str> {
        let windows = image_id
            .and_then(|image_id| self.images.get(image_id))
            .is_some_and(|image| {
                image
                    .platform
                    .as_deref()
                    .is_some_and(|platform| platform.eq_ignore_ascii_case(IMAGE_PLATFORM_WINDOWS))
            });
        windows.then_some(IMAGE_PLATFORM_WINDOWS)
    }
}

/// EC2's error code for capacity it cannot provide: "there is not enough capacity to fulfill
/// your instance request". A fleet reports it per pool it could not place every instance of
/// — "the error code that indicates why the instance could not be launched"
/// (`CreateFleetError$ErrorCode`).
pub const INSUFFICIENT_INSTANCE_CAPACITY: &str = "InsufficientInstanceCapacity";

/// The message EC2 words [`INSUFFICIENT_INSTANCE_CAPACITY`] with. Fixed text: the code says
/// everything there is to say about a launch no capacity was available for, and the pool it
/// happened in is named by the entry's own `LaunchTemplateAndOverrides`.
pub const INSUFFICIENT_INSTANCE_CAPACITY_MESSAGE: &str =
    "There is not enough capacity to fulfill your instance request. Reduce the number of \
     instances in your request, or wait for additional capacity to become available.";

/// One launch an EC2 Fleet could not make: the pool it was to be made in, and why.
///
/// The neutral projection both reports of a fleet's launch errors are built from — the
/// `CreateFleetError` entries a create answers and the `DescribeFleetError` entries a
/// describe answers are the same facts under two shape names — so a fleet cannot report its
/// errors two ways.
#[derive(Debug, Clone)]
pub struct FleetLaunchError {
    /// `CreateFleetError$LaunchTemplateAndOverrides`.
    pub launch_template_and_overrides: Option<crate::ops::types::LaunchTemplateAndOverridesResponse>,
    /// `CreateFleetError$Lifecycle`.
    pub lifecycle: Option<String>,
    /// `CreateFleetError$ErrorCode`.
    pub error_code: Option<String>,
    /// `CreateFleetError$ErrorMessage`.
    pub error_message: Option<String>,
}

/// One group of instances an EC2 Fleet placed: every instance of one of its pools, with what
/// the fleet reports about them.
///
/// The neutral projection both reports of a fleet's instances are built from — the
/// `CreateFleetInstance` entries a create answers and the `DescribeFleetsInstances` entries a
/// describe answers are the same facts under two shape names — so a fleet cannot report its
/// instances two ways.
#[derive(Debug, Clone)]
pub struct FleetInstanceGroup {
    /// Which pool of the fleet placed these instances ([`FleetRecord::pools`]).
    pub pool_index: usize,
    /// `CreateFleetInstance$LaunchTemplateAndOverrides`.
    pub launch_template_and_overrides: Option<crate::ops::types::LaunchTemplateAndOverridesResponse>,
    /// `CreateFleetInstance$Lifecycle`.
    pub lifecycle: Option<String>,
    /// `CreateFleetInstance$InstanceType`, as the instance store holds it.
    pub instance_type: Option<String>,
    /// `CreateFleetInstance$Platform`.
    pub platform: Option<String>,
    /// `CreateFleetInstance$AvailabilityZone`, as the instance store holds it.
    pub availability_zone: Option<String>,
    /// `CreateFleetInstance$SubnetId`, as the instance store holds it.
    pub subnet_id: Option<String>,
    /// `CreateFleetInstance$InstanceIds` — "the IDs of the instances".
    pub instance_ids: Vec<String>,
}
