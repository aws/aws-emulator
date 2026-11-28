// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instances an Auto Scaling group's capacity changes launch and
//! terminate, in the account's own Amazon EC2 store.
//!
//! An instance id a group reports under `AutoScalingGroups[].Instances[].InstanceId`
//! names an instance OF THE ACCOUNT: a capacity change launches it from the group's
//! launch source, so the account's own `DescribeInstances` resolves it, and a scale-in
//! (or a forced delete of the group) terminates that same instance. This module is the
//! one place that launch and that termination happen: the instances live in Amazon
//! EC2's own store, reached through EC2's own accessor and writers, so one instance is
//! never described by two services differently and no id is minted that resolves
//! nowhere.
//!
//! What an instance runs comes from the group's launch source, resolved here: a launch
//! configuration the account holds, or the launch template the group (or its mixed
//! instances policy) names, read from Amazon EC2's own launch-template store. A group
//! whose `VPCZoneIdentifier` names subnets places each instance in one of them, so the
//! instance reports a `SubnetId` of the group and the Availability Zone that subnet is
//! in; a group that names only zones places into those zones.
//!
//! LOCKING. This service's own store is locked by the handler, and Amazon EC2's is
//! taken here for the one pass that launches or terminates and released when it ends;
//! nothing awaits while either is held. Amazon EC2 never reaches this service, so the
//! two locks are never taken in the other order and no cycle exists.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use ::service_com_amazonaws_ec2_amazonec2::ops::types::{
    EbsBlockDevice, IamInstanceProfile, IamInstanceProfileSpecification, RequestLaunchTemplateData,
};
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

pub use ::service_com_amazonaws_ec2_amazonec2::state::VolumeKeyRefusal;

use crate::ops::types::{LaunchTemplateSpecification, MixedInstancesPolicy, Tag};
use crate::state::{ec2_subnets, StoredInstance, StoredLaunchConfiguration};

/// The tag Amazon EC2 Auto Scaling itself puts on every instance it launches, naming
/// the group the instance belongs to — beside the group's own propagate-at-launch tags.
pub const GROUP_NAME_TAG: &str = "aws:autoscaling:groupName";

/// What a group asks Amazon EC2 to launch: how many instances, where they go and what
/// they run.
#[derive(Debug)]
pub struct LaunchRequest<'a> {
    /// The Region the group lives in, which is the Region its instances run in.
    pub region: &'a str,
    /// The group the instances belong to.
    pub group_name: &'a str,
    /// How many instances to launch.
    pub count: usize,
    /// The instant the launch was OWED: the instant of the call for a capacity change a
    /// caller asked for, and the instant the step fell due for one this service's clock
    /// owes. It is the instant Amazon EC2 dates each instance from and runs its own span to
    /// `running` from, so the group's record and EC2's agree on how old an instance is.
    pub launched_at: SystemTime,
    /// How many instances the group already holds, so the placements are used in turn
    /// ACROSS capacity changes instead of restarting at the first one every time.
    pub placed: usize,
    /// The group's Availability Zones.
    pub zones: &'a [String],
    /// The group's `VPCZoneIdentifier`, when it has one.
    pub vpc_zone_identifier: Option<&'a str>,
    /// The launch configuration the group launches from, when it launches from one.
    pub launch_configuration: Option<&'a StoredLaunchConfiguration>,
    /// The launch template the group launches from, when it names one.
    pub launch_template: Option<&'a LaunchTemplateSpecification>,
    /// The mixed instances policy the group launches from, when it has one.
    pub mixed_instances_policy: Option<&'a MixedInstancesPolicy>,
    /// The instance type the launch SOURCE states, where it states one: the type an override
    /// names, or the one its `InstanceRequirements` selected when the group was configured
    /// ([`super::StoredGroup::launch_instance_type`]). `None` leaves the type to the launch
    /// template, which is Amazon EC2's own record.
    pub selected_instance_type: Option<&'a str>,
    /// Whether that source chooses its instance type by ATTRIBUTES
    /// ([`super::instance_requirements::selects_by_requirements`]), so that a source with no
    /// `selected_instance_type` has nothing it may launch at all.
    pub selects_by_requirements: bool,
    /// The group's tags; the ones marked `PropagateAtLaunch` go on the instances.
    pub tags: &'a [Tag],
    /// The service-linked role the group makes the launch under, which is the principal the
    /// authorities over the launch's own references are weighed for — the KMS key the volumes
    /// it creates would be encrypted under ([`super::volume_encryption_keys`]).
    pub service_linked_role_arn: &'a str,
}

/// One instance Amazon EC2 launched: its id, where EC2 placed it and what it runs.
#[derive(Debug, Clone)]
pub struct LaunchedInstance {
    pub instance_id: String,
    pub availability_zone: String,
    pub instance_type: Option<String>,
    pub image_id: Option<String>,
    /// The launch-template specification this instance was ACTUALLY launched from, with
    /// the version Amazon EC2 resolved for the launch — the number, never the `$Latest` or
    /// `$Default` word a group may float on. `None` when the launch read no template of
    /// the account at all.
    pub launch_template: Option<LaunchTemplateSpecification>,
}

/// Where one instance of the group goes: an Availability Zone, and the subnet of that
/// zone the group named, when it named subnets.
#[derive(Debug, Clone)]
struct Placement {
    availability_zone: String,
    subnet_id: Option<String>,
    /// The subnet's own block, which the instance's primary private address comes out
    /// of; `None` for a group that named no subnet, whose instances are addressed from
    /// the default VPC's block the way any launch without a subnet is.
    cidr_block: Option<String>,
}

/// The launch parameters a group's launch source gives every instance it launches.
#[derive(Debug, Clone, Default)]
struct LaunchParameters {
    image_id: Option<String>,
    instance_type: Option<String>,
    key_name: Option<String>,
    /// The security groups the launch source names, as `(group id, group name)`: a
    /// launch source may name either, exactly as a launch does.
    security_groups: Vec<(Option<String>, Option<String>)>,
    user_data: Option<String>,
    ebs_optimized: bool,
    monitoring_enabled: bool,
    /// The option sets the launch source declares for every instance it launches — its
    /// metadata options, enclave, stop protection, shutdown behaviour, Capacity Reservation
    /// preference, maintenance and network performance options, instance hostname options,
    /// tenancy and placement group — resolved by Amazon EC2 itself from the launch-template
    /// version ([`ec2::ComAmazonawsEc2Amazonec2State::launch_template_instance_detail`]), so
    /// an instance this group launches reports what a launch naming that version reports.
    detail: ec2::InstanceDetail,
    /// `LaunchTemplateData$DisableApiTermination`, which the instance reports under
    /// `DescribeInstanceAttribute$DisableApiTermination`.
    disable_api_termination: bool,
    /// `LaunchTemplateHibernationOptionsRequest$Configured`, which the instance reports
    /// under `Instance$HibernationOptions`.
    hibernation_configured: bool,
    /// The credit option for CPU usage the launch source NAMES
    /// (`LaunchTemplateData$CreditSpecification`), or `None` where it names none — in which
    /// case the instance takes the account's default for its instance family, exactly as a
    /// launch naming the same version does. Held as the documented option Amazon EC2 resolved
    /// the word to ([`ec2::cpu_credits_value`]), never as the version's own text.
    cpu_credits_requested: Option<&'static str>,
    /// The EBS root block device every instance of this launch is created with: the AMI's
    /// own root device with the entry the launch source's `BlockDeviceMappings` carries for
    /// that device overlaid, resolved by Amazon EC2 itself
    /// ([`ec2::ComAmazonawsEc2Amazonec2State::launch_template_root_block_device`]) so the
    /// volume an instance of this group boots is the volume a launch naming the same version
    /// boots. `None` for a launch source that names no mapping to apply, whose instances are
    /// created with their AMI's own root device.
    root_block_device: Option<EbsBlockDevice>,
}

/// Launch `request.count` instances for the group and answer the ones Amazon EC2
/// stored, in launch order.
///
/// Answers FEWER than asked for — or none at all — when the account's Amazon EC2 store
/// cannot hold them (its instance count or its byte budget, or a subnet with no free
/// address left) or when the group has nowhere to place one. The caller gives the group
/// only the instances that come back, so a group never reports an id Amazon EC2 does
/// not hold.
pub fn launch<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Vec<LaunchedInstance> {
    if request.count == 0 {
        return Vec::new();
    }
    // Resolved BEFORE Amazon EC2's store is locked, because resolving it reads IAM's:
    // the two stores are never held at once, the order every consumer of a group's
    // launch templates takes ([`super::iam_instance_profiles`]).
    let iam_instance_profile = launched_instance_profile(engine, request);
    // And the AMI the launch source names THROUGH a Systems Manager parameter, resolved for
    // the same reason: resolving it reads the store of the service that owns parameters
    // ([`super::ssm_image_aliases`]).
    let resolved_image = resolved_image_alias(engine, request);
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is nowhere to launch an instance, so the
        // group is given none rather than an id nothing holds.
        return Vec::new();
    };
    let mut store = state.lock();
    let parameters = launch_parameters(&store, request, resolved_image.as_deref());
    let placements = placements(&store, request);
    if placements.is_empty() {
        return Vec::new();
    }
    // The launch-template version this launch READ, as Amazon EC2 resolved it: the group's
    // specification may float on `$Latest` or `$Default`, and the instance runs the one
    // version that word named at this instant.
    let launched_template =
        launched_specification(request).and_then(|specification| {
            resolved_specification(&store, specification)
        });

    // What the instance's tags are, in the order one source overrides another:
    //
    // * the instance tags the launch-template version this launch read declares, which every
    //   launch from that version applies, read through the service that OWNS launch templates
    //   ([`ec2::launch_template_instance_tags`]);
    // * the group's own tags marked `PropagateAtLaunch` — "Determines whether the tag is added
    //   to new instances as they are launched in the group" (`Tag$PropagateAtLaunch`) — which
    //   win over the template's for a key both spell, the group being the nearer source;
    // * the reserved tags no source spells: the group the instance belongs to
    //   ([`GROUP_NAME_TAG`]) and, for a launch from a template, the template and the resolved
    //   version the launch read ([`ec2::LAUNCH_TEMPLATE_ID_TAG`]).
    let template_data = launched_template
        .as_ref()
        .and_then(|specification| template_data(&store, specification));
    let mut tags: BTreeMap<String, String> = template_data
        .as_ref()
        .map(ec2::launch_template_instance_tags)
        .unwrap_or_default();
    tags.extend(
        request
            .tags
            .iter()
            .filter(|tag| super::propagates_at_launch(tag))
            .map(|tag| (tag.key.clone(), tag.value.clone().unwrap_or_default())),
    );
    tags.insert(GROUP_NAME_TAG.to_string(), request.group_name.to_string());
    // An instance launched FROM A LAUNCH TEMPLATE carries the two system tags Amazon EC2 adds
    // for one, naming the template and the version this launch read
    // ([`ec2::LAUNCH_TEMPLATE_ID_TAG`]) — the account's one record of which template version
    // produced the instance. A group that launches from a launch CONFIGURATION read no
    // template, so its instances carry neither.
    if let Some(specification) = launched_template.as_ref() {
        if let Some(launch_template_id) = specification
            .launch_template_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            tags.insert(
                ec2::LAUNCH_TEMPLATE_ID_TAG.to_string(),
                launch_template_id.to_string(),
            );
        }
        if let Some(version) = specification
            .version
            .as_deref()
            .filter(|version| !version.is_empty())
        {
            tags.insert(
                ec2::LAUNCH_TEMPLATE_VERSION_TAG.to_string(),
                version.to_string(),
            );
        }
    }

    // The tags the launch-template version names for the resources a launch from it CREATES
    // beside the instance — "the tags to apply to the resources that are created during
    // instance launch" (`RequestLaunchTemplateData$TagSpecifications`) — read for each
    // resource type through the service that owns launch templates
    // ([`ec2::launch_template_resource_tags`]), so a group's launch tags the volume and the
    // interface its instances get exactly as a caller's own `RunInstances` does. A group's
    // own `PropagateAtLaunch` tags are the INSTANCE's ("determines whether the tag is added
    // to new instances as they are launched in the group", `Tag$PropagateAtLaunch`), so they
    // are not among these.
    let launched_volume_tags = ec2::tag_list(
        &template_data
            .as_ref()
            .map(|data| ec2::launch_template_resource_tags(data, ec2::RESOURCE_TYPE_VOLUME))
            .unwrap_or_default(),
    );
    let launched_interface_tags = ec2::tag_list(
        &template_data
            .as_ref()
            .map(|data| {
                ec2::launch_template_resource_tags(data, ec2::RESOURCE_TYPE_NETWORK_INTERFACE)
            })
            .unwrap_or_default(),
    );

    let instance_type = parameters
        .instance_type
        .clone()
        .unwrap_or_else(|| ec2::DEFAULT_INSTANCE_TYPE.to_string());
    let monitoring_state = if parameters.monitoring_enabled {
        ec2::MONITORING_STATE_ENABLED
    } else {
        ec2::MONITORING_STATE_DISABLED
    };
    // The instant the launch was OWED, which is the instant the group's own record dates
    // the instance from: a capacity change a caller asked for is owed at the instant of that
    // call, and one this service's clock owes at the instant its step fell due. Amazon EC2
    // dates the instance from the same instant and runs its own span to `running` from it,
    // so one instance is never two ages and a step retired after the fact does not restart a
    // span that has already elapsed.
    let launch_time = request.launched_at;
    // The credit option every instance of this launch runs under, resolved once by Amazon
    // EC2's own reading of a launch: the option the launch source named, the account's default
    // for the instance type's family otherwise, and none at all for a type that is not a
    // burstable performance type.
    let cpu_credits = store.launched_cpu_credits(
        request.region,
        &instance_type,
        parameters.cpu_credits_requested,
    );
    let mut launched = Vec::new();
    for offset in 0..request.count {
        if store.instances.len() >= ec2::MAX_STORED_INSTANCES {
            break;
        }
        let placement = &placements[(request.placed + offset) % placements.len()];
        // An instance of a subnet is addressed out of that subnet's own block; one the
        // group placed by zone alone is addressed the way a launch that names no subnet
        // is.
        let private_ip = match placement.cidr_block.as_deref() {
            Some(cidr_block) => store.mint_private_ip_in_cidr(cidr_block),
            None => store.allocate_private_ip(),
        };
        let Some(private_ip) = private_ip else {
            break;
        };
        let (kernel_id, ramdisk_id) =
            store.image_kernel_and_ramdisk(parameters.image_id.as_deref());
        let instance_id = store.mint_instance_id();
        // Amazon EC2 Auto Scaling launches each instance on its own, so each lands in
        // its own reservation, the unit of state one launch creates.
        let reservation_id = store.mint_reservation_id();
        let record = ec2::InstanceRecord {
            instance_id: instance_id.clone(),
            reservation_id: reservation_id.clone(),
            image_id: parameters.image_id.clone(),
            instance_type: instance_type.clone(),
            key_name: parameters.key_name.clone(),
            subnet_id: placement.subnet_id.clone(),
            security_groups: parameters.security_groups.clone(),
            private_dns_name: ec2::private_dns_name(&private_ip, request.region),
            private_ip: private_ip.clone(),
            // A public address comes from a default-VPC subnet or an Elastic IP,
            // neither of which is modelled.
            public_ip: None,
            // …so Amazon maps no public address to it when it starts either.
            mapped_public_ipv4: false,
            // A launched instance is `pending` and becomes `running` when the group's own
            // launch transition is due, so a caller that describes it straight away still
            // sees `pending` — the same transient state, over the same span, that the group
            // reports it `Pending` for ([`crate::state::LAUNCH_TRANSITION`]). The two
            // services settle it together: a member the group reports in service is an
            // instance Amazon EC2 reports running.
            state: ec2::InstanceStateName::Pending,
            pending_transition: Some(ec2::PendingTransition::due_after(
                ec2::InstanceStateName::Running,
                launch_time,
                crate::state::LAUNCH_TRANSITION,
            )),
            state_transition_reason: None,
            state_reason: None,
            // Nobody has asked to reboot an instance the group has just launched, and
            // nobody has sent it a diagnostic interrupt either.
            queued_reboots: 0,
            last_reboot_queued_at: None,
            diagnostic_interrupts: 0,
            last_diagnostic_interrupt_at: None,
            // The guest's FIRST boot: its operating system comes up as the instance
            // finishes starting, the same instant the `pending` -> `running` move above
            // falls due.
            boots: Vec::new(),
            // The guest's own storage starts empty: what is on it is what the instance's
            // user-data program puts there, boot by boot.
            guest_files: ::std::collections::BTreeMap::new(),
            // And it is running nothing: what it runs is what that program installs and
            // starts, which the guest's own boot performs.
            guest_runtime: ec2::GuestRuntime::default(),
            boot_due: Some(launch_time + ec2::TRANSITION_DELAY),
            launch_time,
            // One instance per reservation, so it is that reservation's first.
            ami_launch_index: 0,
            user_data: parameters.user_data.clone(),
            placement_az: placement.availability_zone.clone(),
            monitoring_state: monitoring_state.to_string(),
            // The launch template says whether detailed monitoring is on from the start, so
            // there is no move for Amazon EC2's clock to make.
            monitoring_transition: None,
            // The option sets the group's launch source declares, resolved by Amazon EC2
            // from the launch-template version this launch reads
            // ([`launch_parameters`]); a source that declares none leaves every one of them
            // at its documented default.
            detail: parameters.detail.clone(),
            architecture: ec2::architecture_for(&instance_type),
            kernel_id,
            ramdisk_id,
            source_dest_check: ec2::DEFAULT_SOURCE_DEST_CHECK,
            ebs_optimized: parameters.ebs_optimized,
            // A launch sets no enhanced networking of the instance's own: the instance
            // reports the AMI's until `ModifyInstanceAttribute` gives it a value.
            sriov_net_support: None,
            ena_support: None,
            hibernation_configured: parameters.hibernation_configured,
            // The root volume the launch source's `BlockDeviceMappings` describe, as Amazon
            // EC2 resolved it against the AMI this launch settled on ([`launch_parameters`]):
            // every member of the mapping's EBS settings reaches the volume the store mints
            // for this instance, with the AMI's own answering the ones the source left out. A
            // source that names no mapping leaves this unset, so the instance's root device is
            // the one its AMI carries (Amazon EC2 applies the Region's EBS encryption default
            // to it where that is on).
            root_block_device: parameters.root_block_device.clone(),
            // A capacity change resolves the ROOT device of the version it launches from and
            // no other: the Amazon EBS devices a version names beside it are this launch
            // path's own gap, the same one that leaves a launch CONFIGURATION with no root
            // device of its own.
            data_block_devices: Vec::new(),
            disable_api_termination: parameters.disable_api_termination,
            tags: tags.clone(),
            // The tags the launch-template version names for the resources this launch
            // CREATES beside the instance, read once above.
            launched_volume_tags: launched_volume_tags.clone(),
            launched_interface_tags: launched_interface_tags.clone(),
            // No client token: this launch is not a caller's request to replay.
            client_token: None,
            // An Auto Scaling group's own capacity is on-demand here: no Spot Instance
            // request is modelled, so the instance fulfills none.
            spot_instance_request_id: None,
            cpu_credits,
        };
        // The store's byte budget is asked about the record itself, the same charge
        // storing it applies, so a run of capacity changes cannot grow this process's
        // memory past the bound Amazon EC2 keeps on it.
        if store.would_exceed_stored_bytes(ec2::instance_payload_bytes(&record)) {
            store.release_private_ip(&private_ip);
            break;
        }
        store.insert_reservation(ec2::ReservationRecord {
            reservation_id,
            owner_id: ::engine::emulator_account_id().to_string(),
            requester_id: None,
            instance_ids: Vec::new(),
        });
        store.insert_instance(record);
        // The interface Amazon EC2 gives the instance it just stored is attached to that
        // instance by this launch, not by a later call: NOTHING answers a caller the
        // attachment being made — the group launches the instance itself, and the first
        // thing a client can read of its interface is what `DescribeNetworkInterfaces`
        // reports once the group has launched it. So the attachment is completed here,
        // through the owning store's own seam, rather than left for the clock to finish
        // under a caller who was never shown the transient state.
        store.complete_launched_interface_attachments(&instance_id);
        // "An instance profile … is used to pass an IAM role to an EC2 instance": the
        // instance a group launches RUNS AS the instance profile its launch source names,
        // which is an association of the account's own — the record Amazon EC2 reports
        // under `Instance$IamInstanceProfile` and the only evidence any consumer of it
        // has that the instance has a role (the SSM Agent's credentials among them). It
        // is written through the owning store's own seam, for the same reason the
        // interface attachment above is: nothing answers a caller the association being
        // made, so the group completes it as it launches. A store with no room for it
        // records none rather than failing the launch.
        if let Some(profile) = iam_instance_profile.clone() {
            store.insert_iam_instance_profile_association(
                ec2::IamInstanceProfileAssociationSpec {
                    instance_id: instance_id.clone(),
                    iam_instance_profile: profile,
                },
            );
        }
        launched.push(LaunchedInstance {
            instance_id,
            availability_zone: placement.availability_zone.clone(),
            instance_type: Some(instance_type.clone()),
            image_id: parameters.image_id.clone(),
            launch_template: launched_template.clone(),
        });
    }
    launched
}

/// Stop the instances `instance_ids` names in the account's Amazon EC2 store, or start
/// them again — what a warm pool does to the instances it holds.
///
/// `at` is the instant the move was OWED, which every writer here is anchored at (see
/// [`terminate`]).
///
/// A warm pool keeps its instances in "the instance state to transition to after the
/// lifecycle actions are complete" (`WarmPoolConfiguration$PoolState`), and only
/// `Running` leaves them running: a `Stopped` or `Hibernated` pool holds instances
/// Amazon EC2 reports `stopped`, which is the whole point of the pool — a pre-initialized
/// instance that costs nothing to keep. So this is the same move Amazon EC2 makes for a
/// stop or a start of its own: the instance is `stopping` and reaches `stopped` on EC2's
/// own clock (or `pending` and then `running`), so a caller that reads it straight after
/// the pool settled still sees it on its way. An instance already in (or on its way to)
/// the asked-for state is left alone, one on its way to `terminated` is left alone too,
/// and an id the store holds nothing for changes nothing.
pub fn set_running<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
    running: bool,
    at: SystemTime,
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let now = at;
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        if instance.state.is_terminating() {
            continue;
        }
        match (running, instance.state) {
            (false, ec2::InstanceStateName::Pending | ec2::InstanceStateName::Running) => {
                instance.state = ec2::InstanceStateName::Stopping;
                instance.pending_transition = Some(ec2::PendingTransition::starting_at(
                    ec2::InstanceStateName::Stopped,
                    now,
                ));
                instance.state_transition_reason = Some(ec2::user_initiated_reason(now));
                instance.state_reason = Some((
                    ec2::INSTANCE_STATE_REASON_USER_INITIATED_SHUTDOWN.to_string(),
                    ec2::INSTANCE_STATE_REASON_MESSAGE_USER_INITIATED_SHUTDOWN.to_string(),
                ));
            }
            // An instance still on its way DOWN is brought back up from here too: the
            // pool owns where its instances are, so it re-targets a move still in flight
            // rather than waiting for it to land — otherwise a pool switched to `Running`
            // while a stop was in progress would leave the instance stopped.
            (true, ec2::InstanceStateName::Stopped | ec2::InstanceStateName::Stopping) => {
                instance.state = ec2::InstanceStateName::Pending;
                instance.pending_transition = Some(ec2::PendingTransition::starting_at(
                    ec2::InstanceStateName::Running,
                    now,
                ));
                // A stopped instance has no running operating system, so starting it
                // brings the guest up again, exactly as Amazon EC2's own start does.
                instance.boot_due = Some(now + ec2::TRANSITION_DELAY);
                // The instance is on its way up again, so the reason recorded for the
                // stop no longer describes its state.
                instance.state_transition_reason = None;
                instance.state_reason = None;
            }
            _ => {}
        }
    }
}

/// Terminate the instances `instance_ids` names in the account's Amazon EC2 store.
///
/// What Amazon EC2 does for a termination: the instance is `shutting-down` and reaches
/// `terminated` on EC2's own clock, so a caller that reads it straight after a scale-in
/// still sees it on its way out. An instance already on its way to (or at) `terminated`
/// is left alone — terminating is idempotent — and an id the store holds nothing for
/// changes nothing.
///
/// `at` is the instant the termination was OWED: the instant of the call for a termination
/// a caller asked for, and the instant the step FELL DUE for one this service's clock owes
/// (a scale-in, a replacement of an instance refresh, a health check's replacement). That
/// is the instant Amazon EC2's own span to `terminated` runs from, so a pass that retires a
/// step the clock made due earlier does not restart a span that has already elapsed — and
/// the work that owed the termination is never reported complete while the instance it
/// terminated is still on its way out ([`ec2::PendingTransition::starting_at`]).
pub fn terminate<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
    at: SystemTime,
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let now = at;
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        if instance.state.is_terminating() {
            continue;
        }
        instance.state = ec2::InstanceStateName::ShuttingDown;
        instance.pending_transition = Some(ec2::PendingTransition::starting_at(
            ec2::InstanceStateName::Terminated,
            now,
        ));
        instance.state_transition_reason = Some(ec2::user_initiated_reason(now));
        // A scaling activity a caller asked for shut the instance down, so the shutdown
        // is the client's — the reason Amazon EC2 records for a termination a caller
        // asked for, which is also what `state_transition_reason` above says.
        instance.state_reason = Some((
            TERMINATION_REASON_CODE.to_string(),
            format!("{TERMINATION_REASON_CODE}: {TERMINATION_REASON_MESSAGE}"),
        ));
    }
}

/// The `InstanceStateName` a RUNNING instance carries, as Amazon EC2 names it on the wire.
///
/// Read from Amazon EC2's own enum rather than written out, so a consumer that has to hold
/// an instance to that state ([`super::attached_instances`]) and Amazon EC2's own reports
/// can never disagree about how it is spelled.
#[must_use]
pub fn running_state() -> &'static str {
    ec2::InstanceStateName::Running.as_str()
}

/// Finish the START of the instances `instance_ids` names in the account's Amazon EC2
/// store: a start Amazon EC2's own clock has not retired by `at` is due at that instant and
/// retired here, so Amazon EC2 reports the instance `running`.
///
/// What a LAUNCH lifecycle hook's wait state means for the instance it holds: the work that
/// wait exists for runs ON a started instance — "create a launch template or launch
/// configuration with a user data script that runs while an instance is in a wait state due
/// to a lifecycle hook"
/// (`smithy_query(query="operation",operation="CompleteLifecycleAction")`) — so an instance a
/// hook has taken into that wait has finished starting, and Amazon EC2 reports it `running`
/// for as long as the hold lasts rather than still coming up. Without this, a group could
/// admit an instance to service (which is what resolving the hold does) while Amazon EC2 was
/// still starting it, and the two services would disagree about an instance the group has
/// already reported in service.
///
/// `at` is the instant the hold began, which is the instant the start COMPLETED: the span
/// Amazon EC2 runs to `running` was spent getting the instance to the point the hook holds it
/// at, so it is not restarted here. Anchored like every other writer in this module
/// ([`terminate`]) — a hold the clock took earlier leaves the move due at once, and one taken
/// at an instant still ahead leaves it due then.
///
/// The move is applied through Amazon EC2's OWN pass — the instance's pending transition is
/// made due and that pass retires it — so everything Amazon EC2 does when a start completes
/// is done once, by Amazon EC2. An instance it holds no record of, one already running, and
/// one on its way out are left exactly as they are.
pub fn finish_starting<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
    at: SystemTime,
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    // What is left of the span at this instant: nothing for an instant already past, which is
    // what leaves the move due at once.
    let remaining = at
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    let mut starting = false;
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        if instance.state != ec2::InstanceStateName::Pending {
            continue;
        }
        instance.pending_transition = Some(ec2::PendingTransition {
            to: ec2::InstanceStateName::Running,
            due_at: ::std::time::Instant::now() + remaining,
            // Coming into service is the ordinary end of a launch, so the move records no
            // state reason of its own.
            reason: None,
        });
        // The guest comes up with it: its first boot is what runs the user-data program the
        // wait state exists for, so it is owed at the same instant.
        instance.boot_due = Some(at);
        starting = true;
    }
    if starting {
        store.materialize_due_transitions();
    }
}

/// What the account's own Amazon EC2 store holds for one instance a request named: the
/// facts a consumer of an EXISTING instance needs, as Amazon EC2 holds them.
#[derive(Debug, Clone)]
pub struct HeldInstance {
    /// The Availability Zone Amazon EC2 placed the instance in.
    pub availability_zone: String,
    /// The `InstanceStateName` Amazon EC2 reports for it, as EC2 names it on the wire.
    pub state: String,
    /// What Amazon EC2 launched it as.
    pub instance_type: String,
    /// The AMI it was launched from, when EC2's record holds one.
    pub image_id: Option<String>,
    /// When Amazon EC2 launched it, which is how old the instance is.
    pub launched_at: SystemTime,
}

/// What the account's own Amazon EC2 store holds for each of `instance_ids`, keyed by
/// instance id — an id the store holds nothing for is simply absent from the answer.
///
/// The store is read ONCE for the whole batch, so the cost of one call does not grow with
/// the account's inventory, and Amazon EC2's own clock-owed moves are materialised through
/// EC2's own pass before it is read: an instance whose move to `running` has already fallen
/// due IS running whether or not anybody described it, so a consumer does not refuse an
/// instance that Amazon EC2's own reader reports as up. EC2's store lock is taken for that
/// one pass and released before this returns; the answer is empty on an engine carrying no
/// Amazon EC2 store at all.
pub fn held<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
) -> BTreeMap<String, HeldInstance> {
    if instance_ids.is_empty() {
        return BTreeMap::new();
    }
    let Some(state) = ec2::service_state(engine) else {
        return BTreeMap::new();
    };
    let mut store = state.lock();
    store.materialize_due_transitions();
    instance_ids
        .iter()
        .filter_map(|instance_id| {
            let record = store.instances.get(instance_id)?;
            Some((
                instance_id.clone(),
                HeldInstance {
                    availability_zone: record.placement_az.clone(),
                    state: record.state.as_str().to_string(),
                    instance_type: record.instance_type.clone(),
                    image_id: record.image_id.clone(),
                    launched_at: record.launch_time,
                },
            ))
        })
        .collect()
}

/// Which of `instance_ids` the account's own Amazon EC2 store NO LONGER RUNS: the ones
/// whose record is on its way to `terminated` or already there, and the ones Amazon EC2
/// holds no record of at all.
///
/// A consumer of an instance it did not launch itself — a group that holds one — reads
/// this to learn that the instance has gone away under it, however it went: terminated by
/// a caller of Amazon EC2's own API, or no longer held at all.
///
/// `None` on an engine carrying no Amazon EC2 store: whether an instance still runs is a
/// fact of the account's state, so with no state to read there is no evidence either way
/// and the caller leaves its instances exactly as they are. Amazon EC2's own clock-owed
/// moves are materialised through EC2's own pass first, for the reason [`held`] states.
/// EC2's store lock is taken for that one pass and released before this returns.
pub fn gone<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
) -> Option<Vec<String>> {
    let state = ec2::service_state(engine)?;
    if instance_ids.is_empty() {
        return Some(Vec::new());
    }
    let mut store = state.lock();
    store.materialize_due_transitions();
    Some(
        instance_ids
            .iter()
            .filter(|instance_id| {
                store
                    .instances
                    .get(instance_id.as_str())
                    .is_none_or(|record| record.state.is_terminating())
            })
            .cloned()
            .collect(),
    )
}

/// Record in the account's own Amazon EC2 store that the instances `instance_ids` names are
/// now instances of the group `group_name`: the tag Amazon EC2 Auto Scaling itself puts on
/// every instance of a group ([`GROUP_NAME_TAG`]).
///
/// An instance a capacity change launched is given that tag by the launch itself, so this is
/// the same fact for an instance the group was GIVEN rather than launched — an attach
/// ([`super::attached_instances`]) — and it is written where the instance lives, so the tag
/// a client reads off the instance comes from Amazon EC2's own record of it. An id the store
/// holds nothing for records nothing. EC2's store lock is taken for the one pass and
/// released before this returns.
pub fn record_group_membership<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
    group_name: &str,
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        instance
            .tags
            .insert(GROUP_NAME_TAG.to_string(), group_name.to_string());
    }
}

/// Record in the account's own Amazon EC2 store that the instances `instance_ids` names are
/// NO LONGER instances of the group `group_name`: the tag [`record_group_membership`] put on
/// them comes off.
///
/// The converse of that write, for the one transition that releases a member of a group
/// without terminating it — a detach ([`super::detached_instances`]): "after the instances
/// are detached, you can manage them independent of the Auto Scaling group"
/// (`DetachInstances`), and the tag Amazon EC2 Auto Scaling puts on every member of a group
/// is exactly what says an instance is not managed independently. An instance whose tag
/// names ANOTHER group is left alone — it is that group's member, not this one's — and so is
/// an id this store holds nothing for. EC2's store lock is taken for the one pass and
/// released before this returns.
pub fn clear_group_membership<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &[String],
    group_name: &str,
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        if instance.tags.get(GROUP_NAME_TAG).map(String::as_str) == Some(group_name) {
            instance.tags.remove(GROUP_NAME_TAG);
        }
    }
}

/// The subnet the account's Amazon EC2 store placed `instance_id` in, when it placed it
/// in one.
///
/// Where an instance IS is Amazon EC2's own fact — the placement [`launch`] above wrote —
/// so a consumer that has to report it (the lifecycle event a capacity change publishes)
/// reads it from there rather than guessing it from the group's `VPCZoneIdentifier`.
/// `None` for an instance EC2 holds no record of, for one it placed by zone alone, and
/// when this engine carries no Amazon EC2 store at all. EC2's store lock is taken for the
/// one lookup and released before this returns.
pub fn subnet_of<Eng: Send + Sync + 'static>(engine: &Eng, instance_id: &str) -> Option<String> {
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    store.instances.get(instance_id)?.subnet_id.clone()
}

/// The VPC each of `instance_ids` runs IN, keyed by instance id, as the account's own
/// Amazon EC2 store holds it: the VPC of the subnet the instance was placed in.
///
/// An instance the store holds no record of, one placed by Availability Zone alone (which
/// is in no subnet, so in no VPC this service can read), one whose subnet the store no
/// longer holds, and every instance on an engine carrying no Amazon EC2 store at all are
/// simply ABSENT from the answer: what a consumer does with a VPC it could not read is its
/// own rule, and nothing may be concluded from silence.
///
/// The store is read ONCE for the whole batch, so the cost does not grow with the account's
/// inventory; EC2's store lock is taken for that one pass and released before this returns.
pub fn vpcs_of<'a, Eng: Send + Sync + 'static, I>(
    engine: &Eng,
    instance_ids: I,
) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let ids: BTreeSet<&str> = instance_ids.into_iter().collect();
    if ids.is_empty() {
        return BTreeMap::new();
    }
    let Some(state) = ec2::service_state(engine) else {
        return BTreeMap::new();
    };
    let store = state.lock();
    ids.into_iter()
        .filter_map(|instance_id| {
            let subnet_id = store.instances.get(instance_id)?.subnet_id.as_deref()?;
            let vpc_id = store.subnets.get(subnet_id)?.vpc_id.clone()?;
            Some((instance_id.to_string(), vpc_id))
        })
        .collect()
}

/// The `StateReason` code of an instance terminated because a caller's capacity change
/// scaled the group in: the client asked for the shutdown, which is the code Amazon EC2
/// records for a termination a caller asked for.
const TERMINATION_REASON_CODE: &str = "Client.UserInitiatedShutdown";

/// The sentence that code carries.
const TERMINATION_REASON_MESSAGE: &str = "User initiated shutdown";

/// Where the group's instances go, in the order the group names them: one entry per
/// subnet of its `VPCZoneIdentifier` that the account holds in one of the group's zones,
/// or one per zone for a group that named no usable subnet.
///
/// A subnet named twice is one placement: naming it again does not place more instances
/// there.
fn placements(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    request: &LaunchRequest<'_>,
) -> Vec<Placement> {
    let mut placements: Vec<Placement> = Vec::new();
    for subnet_id in ec2_subnets::subnet_ids(request.vpc_zone_identifier.unwrap_or_default()) {
        let Some(subnet) = store.subnets.get(&subnet_id) else {
            continue;
        };
        let Some(zone) = subnet.availability_zone.clone() else {
            continue;
        };
        // The group's zones are what it places into: a subnet outside them is not one
        // of this group's placements, however the request named it.
        if !request.zones.is_empty() && !request.zones.contains(&zone) {
            continue;
        }
        if placements
            .iter()
            .any(|placement| placement.subnet_id.as_deref() == Some(subnet_id.as_str()))
        {
            continue;
        }
        placements.push(Placement {
            availability_zone: zone,
            subnet_id: Some(subnet_id),
            cidr_block: subnet.cidr_block.clone(),
        });
    }
    if placements.is_empty() {
        placements = request
            .zones
            .iter()
            .map(|zone| Placement {
                availability_zone: zone.clone(),
                subnet_id: None,
                cidr_block: None,
            })
            .collect();
    }
    placements
}

/// The launch parameters the group's launch source gives its instances.
///
/// A launch configuration the account holds is read from this service's own store by
/// the caller; a launch template is read from Amazon EC2's, which is where launch
/// templates live. A group whose launch source names a template the account does not
/// hold (or holds no such version of) contributes no parameters, so its instances run
/// what Amazon EC2 gives a launch that names none.
fn launch_parameters(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    request: &LaunchRequest<'_>,
    resolved_image: Option<&str>,
) -> LaunchParameters {
    if let Some(configuration) = request.launch_configuration {
        return LaunchParameters {
            image_id: Some(configuration.image_id.clone()),
            instance_type: Some(configuration.instance_type.clone()),
            key_name: configuration.key_name.clone(),
            security_groups: configuration
                .security_groups
                .iter()
                .map(|group| (Some(group.clone()), None))
                .collect(),
            user_data: configuration.user_data.clone(),
            ebs_optimized: configuration.ebs_optimized,
            monitoring_enabled: configuration.instance_monitoring_enabled,
            // A launch configuration carries option sets of its own, and this store holds
            // none to read them off: `CreateLaunchConfiguration` is refused
            // `UnsupportedOperation`, so no group launches from one
            // ([`StoredLaunchConfiguration`]). An instance of such a group would report
            // every option set at its documented default, which is what this leaves them at.
            detail: ec2::InstanceDetail::default(),
            disable_api_termination: false,
            hibernation_configured: false,
            // For the same reason: no group launches from a launch configuration, and a
            // launch configuration carries no credit specification either, so an instance of
            // one would take its instance family's default credit option.
            cpu_credits_requested: None,
            // For the same reason: no group launches from a launch configuration, and an
            // instance of one would be created with its AMI's own root device.
            root_block_device: None,
        };
    }

    // A mixed instances policy carries the group's launch template, and its overrides
    // name the instance types to launch; the first override is the one this takes, the
    // way an allocation strategy prefers the first type it is given.
    let policy_template = request
        .mixed_instances_policy
        .and_then(|policy| policy.launch_template.as_ref());
    let specification = launched_specification(request);
    let override_entry = policy_template
        .and_then(|template| template.overrides.as_ref())
        .and_then(|overrides| overrides.first());
    let data = specification.and_then(|specification| template_data(store, specification));

    let image_id = override_entry
        .and_then(|entry| entry.image_id.clone())
        .or_else(|| data.as_ref().and_then(|data| data.image_id.clone()));
    // "The Systems Manager parameter will resolve to an AMI ID on launch"
    // (`RequestLaunchTemplateData$ImageId`): a value naming a parameter is turned into the AMI
    // the parameter holds by the launch, so the instance this group places runs — and reports
    // — that AMI and never the reference the template stored. The resolution is the caller's,
    // made against the store of the service that owns parameters before Amazon EC2's was
    // locked ([`super::ssm_image_aliases::aliased_launch_image`]).
    let image_id = match (resolved_image, image_id) {
        (Some(resolved), Some(_)) => Some(resolved.to_string()),
        (_, image_id) => image_id,
    };
    // "The block device mapping" (`RequestLaunchTemplateData$BlockDeviceMappings`) a version
    // carries applies to every launch made from it, so the root volume an instance of this
    // group boots is the one that version describes: Amazon EC2's own reading of the
    // version's mappings against the AMI this launch settled on, which is the same reading a
    // launch naming the version goes through.
    let root_block_device = data
        .as_ref()
        .and_then(|data| {
            store.launch_template_root_block_device(data, image_id.as_deref(), request.region)
        })
        .map(|(_, ebs)| ebs);
    LaunchParameters {
        // The option sets the launch-template version declares, resolved by Amazon EC2
        // itself against the account's own IMDS defaults and the AMI this launch settled
        // on: "the launch template contains the parameters to launch an instance", so an
        // instance a capacity change launches from a version reports what a launch naming
        // that version reports. A group whose source names no template the account holds
        // reads no version, and its instances carry the documented defaults.
        detail: data
            .as_ref()
            .map(|data| store.launch_template_instance_detail(data, image_id.as_deref()))
            .unwrap_or_default(),
        disable_api_termination: data
            .as_ref()
            .and_then(|data| data.disable_api_termination)
            .unwrap_or(false),
        hibernation_configured: data
            .as_ref()
            .and_then(|data| data.hibernation_options.as_ref())
            .and_then(|options| options.configured)
            .unwrap_or(false),
        // "The credit option for CPU usage of the instance"
        // (`LaunchTemplateData$CreditSpecification`) the version declares, resolved to the
        // documented option by Amazon EC2's own reading of that word: a version naming none,
        // and one naming a word outside the documented pair, leave the instance on its
        // family's default.
        cpu_credits_requested: data
            .as_ref()
            .and_then(|data| data.credit_specification.as_ref())
            .and_then(|specification| ec2::cpu_credits_value(&specification.cpu_credits)),
        image_id,
        // The type the launch source states: the one an override names, or the one its
        // `InstanceRequirements` selected when the group was configured — resolved by the
        // caller through the one reading of an override's launch type this crate has
        // ([`super::instance_requirements`]), so the type launched here is the type the
        // group's capacity arithmetic counted it as. A launch source that states none leaves
        // the type to the launch-template version.
        instance_type: request
            .selected_instance_type
            .map(str::to_string)
            .or_else(|| data.as_ref().and_then(|data| data.instance_type.clone())),
        key_name: data.as_ref().and_then(|data| data.key_name.clone()),
        // The security groups the version declares, wherever it declares them: a version that
        // names its network interfaces names its groups on the primary one of them — "if you
        // specify a network interface, you must specify any security groups as part of the
        // network interface instead of using this parameter"
        // (`RequestLaunchTemplateData$SecurityGroupIds`) — and one that names no interface
        // names them on its own two members. Read through the ONE reading of that member pair
        // Amazon EC2 has ([`ec2::launch_template_security_groups`]), so the groups an instance
        // of a group comes up in are the groups a launch naming the same version comes up in,
        // instead of the VPC's default group a launch that named none would get.
        security_groups: data
            .as_ref()
            .map(ec2::launch_template_security_groups)
            .unwrap_or_default(),
        user_data: data.as_ref().and_then(|data| data.user_data.clone()),
        ebs_optimized: data
            .as_ref()
            .and_then(|data| data.ebs_optimized)
            .unwrap_or(false),
        monitoring_enabled: data
            .as_ref()
            .and_then(|data| data.monitoring.as_ref())
            .and_then(|monitoring| monitoring.enabled)
            .unwrap_or(false),
        root_block_device,
    }
}

/// The ONE launch-template specification a capacity change launches its instances from,
/// when the group's launch source names a template at all: the group's own
/// `LaunchTemplate`, or the launch template its mixed instances policy carries.
///
/// One reading, so the parameters an instance is launched with
/// ([`launch_parameters`]) and the IAM instance profile it runs as
/// ([`launched_instance_profile`]) come from the same launch-template version. The
/// per-override templates a policy may name are not it: an override contributes the
/// instance type (and the AMI) to launch, not the version this launch reads — the whole
/// set is [`specifications`], which is what a request's templates are VALIDATED over.
fn launched_specification<'a>(
    request: &'a LaunchRequest<'_>,
) -> Option<&'a LaunchTemplateSpecification> {
    request.launch_template.or_else(|| {
        request
            .mixed_instances_policy
            .and_then(|policy| policy.launch_template.as_ref())
            .and_then(|template| template.launch_template_specification.as_ref())
    })
}

/// The IAM instance profile every instance of this launch RUNS AS, as the association
/// Amazon EC2 records over an instance reports it, or `None` when the group's launch
/// source names none.
///
/// The reference the launch source carries is resolved against the account's own IAM
/// records through the ONE reading of that member Amazon EC2 has
/// ([`ec2::resolve_iam_instance_profile`]), so the profile an instance of a group runs as
/// is the one a launch naming it would run as — the ARN and the unique id IAM holds the
/// profile under, never values derived here.
///
/// Takes Amazon EC2's store lock for the launch-template read and IAM's for the
/// resolution, one after the other and neither while the other is held; this runs before
/// the launch locks EC2's store for its own pass.
fn launched_instance_profile<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<IamInstanceProfile> {
    let specification = named_instance_profile(engine, request)?;
    // Nothing the reading above can refuse reaches it: exactly one of the two spellings is
    // filled, so neither the both-given nor the neither-given refusal is reachable, and a
    // launch source carrying a reference that does not RESOLVE — an ill-formed ARN among
    // them — is refused when the group is created or updated, before any capacity change
    // launches from it ([`super::iam_instance_profiles::unresolvable_profile_error`]). What
    // is left is the answer that reading gives a profile IAM holds no record of: the
    // reference as the launch source spelled it, with no id.
    ec2::resolve_iam_instance_profile::<::std::convert::Infallible, Eng>(engine, &specification)
        .ok()
}

/// How the group's launch source spells the instance profile its instances run as, as the
/// specification Amazon EC2's own reading of that member takes.
///
/// A launch template carries the reference as an ARN or as a name
/// (`LaunchTemplateIamInstanceProfileSpecificationRequest`), and a launch configuration as
/// one value that is "the name or the Amazon Resource Name (ARN) of the instance profile"
/// (`LaunchConfiguration$IamInstanceProfile`) — read as an ARN exactly when the tree's one
/// IAM-ARN reading says it names an instance profile. The ARN wins where a template carries
/// both, the way the resolution every other consumer of these templates performs reads them
/// ([`super::iam_instance_profiles`]). An empty value names nothing.
fn named_instance_profile<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<IamInstanceProfileSpecification> {
    if let Some(configuration) = request.launch_configuration {
        return configuration
            .iam_instance_profile
            .as_deref()
            .and_then(spelled_profile_reference);
    }
    let profile = launch_template_data(engine, launched_specification(request)?)?
        .iam_instance_profile?;
    if let Some(arn) = profile.arn.filter(|arn| !arn.is_empty()) {
        return Some(IamInstanceProfileSpecification {
            arn: Some(arn),
            name: None,
        });
    }
    let name = profile.name.filter(|name| !name.is_empty())?;
    Some(IamInstanceProfileSpecification {
        arn: None,
        name: Some(name),
    })
}

/// The ONE value a launch configuration carries for the profile its instances run as, read
/// as the specification Amazon EC2's own reading of that member takes: "the name or the
/// Amazon Resource Name (ARN) of the instance profile"
/// (`LaunchConfiguration$IamInstanceProfile`) is one member holding either spelling, so
/// which one it is has to be read off the value.
///
/// It is an ARN exactly when the tree's one reading of an IAM ARN says it names an instance
/// profile ([`::service_common::iam_role::instance_profile_name`]) — so a value that is
/// ARN-shaped but names another vendor, another IAM entity type or another account is not
/// taken for one — and a name otherwise. An empty value names nothing.
fn spelled_profile_reference(reference: &str) -> Option<IamInstanceProfileSpecification> {
    if reference.is_empty() {
        return None;
    }
    if ::service_common::iam_role::instance_profile_name(reference).is_some() {
        return Some(IamInstanceProfileSpecification {
            arn: Some(reference.to_string()),
            name: None,
        });
    }
    Some(IamInstanceProfileSpecification {
        arn: None,
        name: Some(reference.to_string()),
    })
}

/// Every launch template a group's launch source names, in the order the request names
/// them: the group's own `LaunchTemplate` first, then the launch template of its mixed
/// instances policy and the template each of that policy's overrides names.
///
/// A mixed instances policy carries a launch template of its own, and "some instance types
/// might require a launch template with a different AMI", so an override may name a further
/// one (`LaunchTemplateOverrides$LaunchTemplateSpecification`) — each of them is a template
/// instances of the group are launched from, so every consumer of a group's templates reads
/// the same set from here: the IAM instance profile they name
/// ([`super::iam_instance_profiles`]), the security groups they name
/// ([`super::ec2_security_groups`]) and the halves this module completes
/// ([`complete_launch_template_specifications`], which walks the same set to WRITE them).
pub(super) fn specifications<'a>(
    launch_template: Option<&'a LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&'a MixedInstancesPolicy>,
) -> Vec<&'a LaunchTemplateSpecification> {
    let policy_template = mixed_instances_policy.and_then(|policy| policy.launch_template.as_ref());
    launch_template
        .into_iter()
        .chain(policy_template.and_then(|template| template.launch_template_specification.as_ref()))
        .chain(
            policy_template
                .and_then(|template| template.overrides.as_ref())
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.launch_template_specification.as_ref()),
        )
        .collect()
}

/// Complete every launch-template specification a group carries with the half the request did
/// not state: the `LaunchTemplateName` of a template named by id, and the
/// `LaunchTemplateId` of one named by name.
///
/// "Conditional: You must specify either a `LaunchTemplateId` or a `LaunchTemplateName`"
/// (`LaunchTemplateSpecification`), so a caller states ONE of them — and Amazon EC2 Auto
/// Scaling resolves the template when the group is created, which is why every read of a
/// group reports both: a client that created the group by id reads the template's name off
/// the group, and one that created it by name reads the id. The pair comes from the account's
/// own launch-template store, the same place a launch reads the template's parameters from
/// ([`template_data`]), so the name a group reports is the name EC2 holds that template
/// under.
///
/// A specification naming a template the account does not hold is left exactly as the request
/// stated it: there is no name or id to report for a template nothing holds, and none is
/// invented. EC2's store lock is taken for the one pass that completes them and released
/// before this returns, so no caller holds it.
pub fn complete_launch_template_specifications<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch_template: Option<&mut LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&mut MixedInstancesPolicy>,
) {
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let store = state.lock();
    // Every specification a group carries: its own, the one its mixed instances policy
    // names, and the one each of that policy's overrides may name instead — the same set
    // every reader of a group's templates resolves over ([`specifications`]), walked here
    // by mutable reference because this pass WRITES the halves back.
    let mut specifications: Vec<&mut LaunchTemplateSpecification> = Vec::new();
    if let Some(specification) = launch_template {
        specifications.push(specification);
    }
    if let Some(template) = mixed_instances_policy.and_then(|policy| policy.launch_template.as_mut())
    {
        if let Some(specification) = template.launch_template_specification.as_mut() {
            specifications.push(specification);
        }
        for entry in template.overrides.iter_mut().flatten() {
            if let Some(specification) = entry.launch_template_specification.as_mut() {
                specifications.push(specification);
            }
        }
    }
    for specification in specifications {
        complete_specification(&store, specification);
    }
}

/// Fill in the half of ONE specification the request did not state, from Amazon EC2's own
/// launch-template store.
fn complete_specification(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    specification: &mut LaunchTemplateSpecification,
) {
    match (
        specification.launch_template_id.clone(),
        specification.launch_template_name.clone(),
    ) {
        (Some(launch_template_id), None) => {
            if let Some(record) = store.launch_templates.get(&launch_template_id) {
                specification.launch_template_name =
                    Some(record.launch_template_name().to_string());
            }
        }
        (None, Some(launch_template_name)) => {
            specification.launch_template_id =
                store.launch_template_id_for_name(&launch_template_name);
        }
        // Both stated, or neither: there is nothing to resolve. A specification stating both
        // is refused by the owning service's own rules, not completed here.
        _ => {}
    }
}

/// The launch-template version `specification` names, read from the account's own Amazon
/// EC2 store — the launch data itself, for a caller that needs more of it than a launch
/// does.
///
/// `None` when the account holds no such template, or no such version of it, and when
/// this engine carries no Amazon EC2 store at all: there is then no launch data to read.
/// EC2's store lock is taken for the one lookup that answers this and released before the
/// answer is returned, so no caller holds it.
pub fn launch_template_data<Eng: Send + Sync + 'static>(
    engine: &Eng,
    specification: &LaunchTemplateSpecification,
) -> Option<RequestLaunchTemplateData> {
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    template_data(&store, specification)
}

/// The ids of the launch templates a group configured by `launch_template` and
/// `mixed_instances_policy` would launch from: the id Amazon EC2 holds each of them under,
/// which is how Amazon EC2 identifies a launch template.
///
/// The set is the one every consumer of a group's templates reads ([`specifications`]), each
/// resolved against the account's own Amazon EC2 store and listed once, in the order the
/// request names them: two specifications naming one template name one template. A
/// specification the account holds no template for contributes nothing — there is no template
/// of it to name — and neither does an engine carrying no Amazon EC2 store at all.
///
/// EC2's store lock is taken for this one walk and released before the answer is returned,
/// so no caller holds it; this reads only.
pub fn launch_template_ids<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch_template: Option<&LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> Vec<String> {
    let Some(state) = ec2::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let mut ids: Vec<String> = Vec::new();
    for specification in specifications(launch_template, mixed_instances_policy) {
        let Some((launch_template_id, _)) = resolved_version(&store, specification) else {
            continue;
        };
        if !ids.contains(&launch_template_id) {
            ids.push(launch_template_id);
        }
    }
    ids
}

/// The launch parameters of the one launch-template version a group's specification
/// names, from Amazon EC2's own launch-template store.
///
/// The template is named by id or by name, and the version by number or by one of the
/// two words the model documents: `$Default` (also what a specification carrying no
/// version means) and `$Latest`.
fn template_data(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    specification: &LaunchTemplateSpecification,
) -> Option<RequestLaunchTemplateData> {
    let (launch_template_id, version) = resolved_version(store, specification)?;
    let record = store.launch_templates.get(&launch_template_id)?;
    record
        .versions
        .get(&version)
        .map(|version| version.data.clone())
}

/// Which of a group's instances are NOT already running the configuration a refresh would
/// give them — the question `SkipMatching` asks of every instance of a group, answered
/// under ONE acquisition of Amazon EC2's store lock whatever the group holds.
///
/// An instance MATCHES when the launch-template version it was launched from resolves to the
/// version `target` resolves to: a specification is a WAY OF NAMING one version (the
/// template by id or by name, the version by number or by `$Latest`/`$Default`), so two
/// spellings of one version are one configuration and an instance running it needs no
/// replacing. The target is resolved ONCE here rather than per instance, and
/// `instance_types` — the types the target names explicitly, which come from the request —
/// is a SET, so what one instance costs does not grow with the request: no work here is the
/// product of the group's size and anything a caller sends.
///
/// A specification naming a template (or a version of one) the account does not hold
/// resolves to nothing, and is then compared exactly as it was written
/// ([`super::same_launch_template`]), which is also what every comparison falls back to on
/// an engine carrying no Amazon EC2 store at all.
pub fn instances_not_running<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    instances: impl Iterator<Item = &'a StoredInstance>,
    target: Option<&LaunchTemplateSpecification>,
    instance_types: &BTreeSet<String>,
) -> Vec<String> {
    let state = ec2::service_state(engine);
    let store = state.as_ref().map(|state| state.lock());
    let store = store.as_deref();
    let resolved_target = store
        .zip(target)
        .and_then(|(store, target)| resolved_version(store, target));
    instances
        .filter(|instance| {
            let matching = runs_target(
                store,
                resolved_target.as_ref(),
                instance.launch_template.as_ref(),
                target,
            ) && (instance_types.is_empty()
                || instance
                    .instance_type
                    .as_ref()
                    .is_some_and(|instance_type| instance_types.contains(instance_type)));
            !matching
        })
        .map(|instance| instance.instance_id.clone())
        .collect()
}

/// Whether an instance launched from `launched` is running the version `target` names,
/// `resolved_target` being that version where the store resolves it.
///
/// Both sides resolved, the versions decide; either side unresolvable, the specifications
/// are compared as they were written, since there is then no stored version to tell them
/// apart by.
fn runs_target(
    store: Option<&ec2::ComAmazonawsEc2Amazonec2State>,
    resolved_target: Option<&(String, i64)>,
    launched: Option<&LaunchTemplateSpecification>,
    target: Option<&LaunchTemplateSpecification>,
) -> bool {
    let (Some(launched), Some(target)) = (launched, target) else {
        return launched.is_none() && target.is_none();
    };
    let resolved_launched = store.and_then(|store| resolved_version(store, launched));
    match (resolved_launched.as_ref(), resolved_target) {
        (Some(launched_version), Some(target_version)) => launched_version == target_version,
        _ => super::same_launch_template(Some(launched), Some(target)),
    }
}

/// The `(launch template id, version number)` a specification resolves to against a
/// locked Amazon EC2 store — the one place this crate resolves a specification, shared by
/// the launch data a group launches from and by the matching [`instances_not_running`]
/// does.
///
/// The VERSION member itself is read by the service that OWNS launch templates
/// ([`ec2::resolve_launch_template_version`], the one reading of that member in the
/// emulator): an alias resolves to the number the template records for it and a number has
/// to be a version the template holds, so this crate and Amazon EC2's own operations can
/// never disagree about which version a word names. A specification carrying no version
/// means the template's default ([`ec2::LAUNCH_TEMPLATE_VERSION_DEFAULT`], from the
/// model's documentation of `LaunchTemplateSpecification$Version`: "The default value is
/// `$Default`"). EC2's refusal is discarded: a consumer of a template the account does not
/// hold, or of a version it does not hold, resolves to nothing rather than refusing — what
/// an unresolvable launch source does is the caller's own rule, not this lookup's.
fn resolved_version(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    specification: &LaunchTemplateSpecification,
) -> Option<(String, i64)> {
    let launch_template_id = match specification.launch_template_id.as_deref() {
        Some(launch_template_id) => launch_template_id.to_string(),
        None => store.launch_template_id_for_name(specification.launch_template_name.as_deref()?)?,
    };
    let record = store.launch_templates.get(&launch_template_id)?;
    let version = ec2::resolve_launch_template_version::<()>(
        record,
        specification
            .version
            .as_deref()
            .unwrap_or(ec2::LAUNCH_TEMPLATE_VERSION_DEFAULT),
    )
    .ok()?;
    Some((launch_template_id, version))
}

/// The specification an instance launched from `specification` REPORTS: the template's id
/// and name as Amazon EC2 holds them, and the version this launch resolved to, written as
/// the number it is.
///
/// A group's own specification may name the template by one half and float its version on
/// `$Latest` or `$Default`; the instance it launches ran ONE version of ONE template, so
/// that is what the instance reports. `None` when the account holds no such template, or
/// no such version of it: there is then no launch the group made from it to report.
fn resolved_specification(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    specification: &LaunchTemplateSpecification,
) -> Option<LaunchTemplateSpecification> {
    let (launch_template_id, version) = resolved_version(store, specification)?;
    let record = store.launch_templates.get(&launch_template_id)?;
    Some(LaunchTemplateSpecification {
        launch_template_id: Some(launch_template_id),
        launch_template_name: record.template.launch_template_name.clone(),
        version: Some(version.to_string()),
    })
}

/// Why the configuration this launch would run cannot produce an INSTANCE IN SERVICE, and
/// the terminal status code the activity of each unplaced unit ends in
/// ([`super::BlockedLaunch`]) — or `None` when the launch can be made.
///
/// A group launches from the launch-template version its specification names AT THE INSTANT
/// OF THE LAUNCH — a group floating on `$Latest` or `$Default` launches from whatever that
/// word then names — so a configuration the account's own stores cannot satisfy is not a
/// request to refuse but a LAUNCH that places nothing: Amazon EC2 Auto Scaling records the
/// reason as the terminal activity's `StatusMessage`
/// ([`super::AutoScalingState::record_failed_activity`]), which is where a client reads why
/// its group never reached its desired capacity.
///
/// Which terminal code that activity ends in follows the reason: a launch Amazon EC2 Auto
/// Scaling itself could not make the attempt for is an attempt that `Failed`, while one
/// Amazon EC2 would not carry out — the launch source the instance would run resolves to
/// nothing there, or the instance it launched would be taken back before it ever reached
/// service — is `Cancelled` — "the StatusCode value is Failed or Cancelled and the
/// StatusMessage element in the response indicates the cause of the failure"
/// (`DescribeScalingActivities`' own documentation).
///
/// What is resolved here is what this service, as the CONSUMER of the template, resolves
/// when it launches from it: the launch-template version the launch reads
/// ([`super::group_configuration::unresolved_launch_template_launch_failure`], which answers
/// Amazon EC2's own sentence for a reference that no longer resolves), the IAM instance
/// profile the instances would run as
/// ([`super::iam_instance_profiles`], whose header states why that reference is this
/// service's to resolve) and the KMS key the volumes they boot would be encrypted under
/// ([`super::volume_encryption_keys`]). What Amazon EC2 itself validates about the launch
/// PARAMETERS — an AMI and an instance type whose architectures disagree — stays with the
/// service that owns the images and the instance types, which is asked for its verdict rather
/// than second-guessed ([`architecture_mismatch_reason`]).
///
/// The sentence a reason of this crate's own carries ends the way every launch failure does
/// ([`super::LAUNCH_FAILED_SENTENCE`]); a reason another service worded is reported as that
/// service worded it. `None` also covers an engine carrying neither the Amazon EC2, the IAM
/// nor the KMS store: a failure has to be grounded in what the account's state answered,
/// never assumed.
pub fn launch_failure_reason<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<super::BlockedLaunch> {
    // The launch-template version this launch would READ, resolved against Amazon EC2 at the
    // instant of the launch. A launch template is Amazon EC2's resource and stays the client's
    // to delete after a group was configured against it, so a reference that no longer
    // resolves is a launch Amazon EC2 would not carry out: nothing is placed and the capacity
    // change ends CANCELLED, carrying Amazon EC2's own sentence for the reference
    // it could not resolve
    // ([`super::group_configuration::unresolved_launch_template_launch_failure`]). Asked
    // first, because nothing else of a launch source can be read from a version that is not
    // there.
    if let Some(reason) =
        super::group_configuration::unresolved_launch_template_launch_failure(
            engine,
            request.launch_template,
            request.mixed_instances_policy,
        )
    {
        return Some(super::BlockedLaunch::cancelled(reason));
    }
    // A launch source that chooses its instance type by ATTRIBUTES can launch only a type
    // those attributes admit: "if you specify `InstanceRequirements`, you can't specify
    // `InstanceType`" (`LaunchTemplateOverrides$InstanceRequirements`), so one whose
    // requirements no offered instance type satisfies has nothing to launch at all — and the
    // launch records that instead of placing an instance of the type its launch template
    // happens to name, which those requirements exclude.
    if request.selects_by_requirements && request.selected_instance_type.is_none() {
        return Some(super::BlockedLaunch::failed(
            unsatisfiable_requirements_message(),
        ));
    }
    // The AMI the launch source names THROUGH a Systems Manager parameter, resolved at the
    // instant of the launch by the service that OWNS parameters: "the Systems Manager
    // parameter will resolve to an AMI ID on launch"
    // (`RequestLaunchTemplateData$ImageId`), so a reference that store produces no value for
    // leaves the launch with no AMI to run at all — an attempt Amazon EC2 Auto Scaling cannot
    // make, which is a launch that FAILED, carrying Amazon EC2's own sentence for an alias it
    // could not resolve ([`super::ssm_image_aliases::invalid_aliases_launch_failure`]). Asked
    // before the architecture of that AMI is weighed, because there is no image to weigh.
    if let Some(reason) = unresolved_image_alias_reason(engine, request) {
        return Some(super::BlockedLaunch::failed(reason));
    }
    // What Amazon EC2 refuses about the LAUNCH PARAMETERS this group would send it: an AMI
    // built for a processor architecture the instance type does not run cannot boot on that
    // type, so Amazon EC2 carries the launch out for nobody — the attempt is one Amazon EC2
    // Auto Scaling could not make, which is a launch that FAILED, carrying Amazon EC2's own
    // sentence for the pair it would not launch. Asked before anything the ACCOUNT holds for
    // the launch is resolved (the instance profile and the volume key below), because the
    // parameters are refused without any of them being looked up.
    if let Some(reason) = architecture_mismatch_reason(engine, request) {
        return Some(super::BlockedLaunch::failed(format!(
            "{reason} {}",
            super::LAUNCH_FAILED_SENTENCE
        )));
    }
    // The IAM instance profile the instances would run as, resolved against the account's
    // own IAM records: an instance cannot be launched under a profile the account does not
    // hold. Nothing of this service is locked here, and resolving it reads Amazon EC2's
    // store and then IAM's, one after the other ([`super::iam_instance_profiles`]).
    if let Some(reason) = super::iam_instance_profiles::unresolved_profile_launch_failure(
        engine,
        request.launch_template,
        request.mixed_instances_policy,
    ) {
        return Some(super::BlockedLaunch::failed(reason));
    }
    // The KMS key the volumes those instances boot would be encrypted under, resolved
    // against the account's own KMS records: a key that produces no data key produces no
    // volume — whether the key's own state refuses it or the authorities in force do not
    // let the group's service-linked role use the key — so the instance Amazon EC2 launched
    // is taken back before it ever reaches service and the capacity change ends CANCELLED
    // ([`super::volume_encryption_keys`]). Amazon EC2's store is read, then IAM's, then
    // KMS's, one after the other, none while this service's own is held.
    //
    // Amazon EC2 ACCEPTS such a launch — nothing about the request is wrong — so the attempt
    // is one the launch MAKES and EC2 withdraws, which is why the reason carries that
    // service's own refusal for the caller to perform it with
    // ([`super::BlockedLaunch::cancelled_attempt`]).
    super::volume_encryption_keys::unusable_volume_key_launch_failure(
        engine,
        request.region,
        request.service_linked_role_arn,
        request.launch_template,
        request.mixed_instances_policy,
    )
    .map(|unusable| {
        super::BlockedLaunch::cancelled_attempt(unusable.status_message, unusable.refusal)
    })
}

/// The image reference the group's launch source carries, as the launch source itself states
/// it — the value an override names, or the launch-template version's own.
///
/// Read under a guard of Amazon EC2's store that is taken and GIVEN BACK here, because the
/// parameter such a value may name is resolved against ANOTHER service's store straight after
/// and no two of those stores are ever held at once. `None` for a launch source that names no
/// image, and on an engine carrying no Amazon EC2 store at all.
fn launch_source_image_id<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<String> {
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    launch_parameters(&store, request, None).image_id
}

/// The AMI the launch source's image reference resolves to, for a source that names its image
/// through a Systems Manager parameter — `None` for one that names an AMI directly, for a
/// reference that resolves to nothing, and on an engine with no store to resolve it against.
///
/// What [`launch`] launches with and what [`architecture_mismatch_reason`] weighs, so the AMI
/// a launch is judged on and the AMI it runs can never differ.
fn resolved_image_alias<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<String> {
    let image_id = launch_source_image_id(engine, request)?;
    match super::ssm_image_aliases::aliased_launch_image(engine, request.region, &image_id)? {
        super::ssm_image_aliases::AliasedLaunchImage::Value(value) => Some(value),
        super::ssm_image_aliases::AliasedLaunchImage::Unresolved => None,
    }
}

/// Why Amazon EC2 would not carry out this launch at all: the Systems Manager parameter the
/// launch source's `ImageId` names resolves to nothing, so there is no AMI to launch from.
///
/// `None` for a source that names an AMI directly, for a reference that resolves, and on an
/// engine carrying no store of the service that owns parameters: a failure is grounded in what
/// the account's state answered, never assumed.
fn unresolved_image_alias_reason<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<String> {
    let image_id = launch_source_image_id(engine, request)?;
    let reference = ec2::ssm_image_alias_parameter(&image_id)?;
    match super::ssm_image_aliases::aliased_launch_image(engine, request.region, &image_id)? {
        super::ssm_image_aliases::AliasedLaunchImage::Value(_) => None,
        super::ssm_image_aliases::AliasedLaunchImage::Unresolved => Some(
            super::ssm_image_aliases::invalid_aliases_launch_failure(reference),
        ),
    }
}

/// Why Amazon EC2 would not carry out this launch at all: the AMI the group's launch source
/// names is built for a processor architecture the instance type it names does not run.
///
/// The verdict is Amazon EC2's own — that service owns the images and the instance types, and
/// states it in its own words
/// ([`ec2::ComAmazonawsEc2Amazonec2State::launch_architecture_mismatch`]) — and the two values
/// it is asked about are the ones THIS launch would send it: the AMI and the instance type the
/// launch source resolves to, read through the one resolution [`launch`] itself launches with
/// ([`launch_parameters`]), so a launch that would be refused and a launch that is made can
/// never be judged on different parameters.
///
/// `None` for a launch source that names no AMI, and on an engine carrying no Amazon EC2 store
/// at all: there is then no image to weigh the type against.
fn architecture_mismatch_reason<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
) -> Option<String> {
    // Resolved before Amazon EC2's store is locked, for the reason [`launch`] resolves it
    // there: the AMI weighed against the instance type is the AMI the launch would really run,
    // which for a source naming a Systems Manager parameter is what that parameter holds.
    let resolved_image = resolved_image_alias(engine, request);
    let state = ec2::service_state(engine)?;
    let store = state.lock();
    let parameters = launch_parameters(&store, request, resolved_image.as_deref());
    let image_id = parameters.image_id.as_deref()?;
    // The type the launch would be made with, which for a source that names none is the type
    // Amazon EC2 gives a launch that names none — exactly what [`launch`] launches it as.
    let instance_type = parameters
        .instance_type
        .as_deref()
        .unwrap_or(ec2::DEFAULT_INSTANCE_TYPE);
    store.launch_architecture_mismatch(image_id, instance_type)
}

/// Make the launch Amazon EC2 ACCEPTS and take the instances it minted back, because a volume
/// each of them would boot cannot be created under the key it was to be encrypted under
/// (`refusal`) — and answer the ids of the attempts, in launch order.
///
/// What Amazon EC2 does for such a launch: the instances exist, in the account's own store,
/// where every instance of the account lives — an attempt a client can describe — and the
/// clock owes each of them the shutdown instead of `running`, carrying the state reason that
/// says why ([`ec2::ComAmazonawsEc2Amazonec2State::withdraw_launch_under_unusable_volume_key`],
/// the ONE writer of that outcome, so an attempt this service made and one a client's own
/// `RunInstances` made end the same way). The volumes that launch created go with them.
///
/// The attempts are NOT instances of the group: none of them ever reaches service, so the
/// group reports none of them and the capacity it was asked for stays owed. Answers nothing
/// for a launch of nothing, and nothing when the account's Amazon EC2 store could place
/// nothing.
pub fn withdraw_unusable_key_attempt<Eng: Send + Sync + 'static>(
    engine: &Eng,
    request: &LaunchRequest<'_>,
    refusal: &VolumeKeyRefusal,
) -> Vec<String> {
    let attempted: Vec<String> = launch(engine, request)
        .into_iter()
        .map(|instance| instance.instance_id)
        .collect();
    if attempted.is_empty() {
        return attempted;
    }
    let Some(state) = ec2::service_state(engine) else {
        // Unreachable: a launch with no Amazon EC2 store behind it mints nothing, so there is
        // nothing here to withdraw. Answered as the attempts that WERE made, so the ids this
        // function reports and the instances it minted can never disagree.
        return attempted;
    };
    let mut store = state.lock();
    for instance_id in &attempted {
        store.withdraw_launch_under_unusable_volume_key(instance_id, refusal);
    }
    attempted
}

/// What a launch whose source's `InstanceRequirements` admit no offered instance type
/// records.
///
/// Ends the way every launch failure of this crate does
/// ([`super::LAUNCH_FAILED_SENTENCE`]), and names no requirement of the request: what the
/// client can act on is that the attributes it stated match nothing it could be given.
#[must_use]
pub fn unsatisfiable_requirements_message() -> String {
    format!(
        "No instance type offered to this account matches the instance requirements of the Auto \
         Scaling group's launch source, so no instance was launched. {}",
        super::LAUNCH_FAILED_SENTENCE
    )
}

/// Whether the launch-template version `specification` names resolves its image through
/// a Systems Manager parameter rather than naming an AMI id.
///
/// The form such an `ImageId` carries is Amazon EC2's own
/// ([`ec2::ssm_image_alias_parameter`], the one reading of that member in this
/// emulator), so this service — the consumer of the template — asks EC2 what the value
/// means instead of reading it a second way. `false` for a template the account does not
/// hold: there is then no image to read.
pub fn resolves_image_through_ssm_parameter<Eng: Send + Sync + 'static>(
    engine: &Eng,
    specification: &LaunchTemplateSpecification,
) -> bool {
    launch_template_data(engine, specification)
        .and_then(|data| data.image_id)
        .as_deref()
        .and_then(ec2::ssm_image_alias_parameter)
        .is_some()
}

#[cfg(test)]
mod tests {
    //! The reading of the ONE member a launch configuration spells its instance profile in,
    //! driven on values rather than through a group: which spelling a value is cannot be
    //! observed on the wire for a launch source `CreateLaunchConfiguration` refuses, so it
    //! is held to the rule here.

    use super::*;

    /// A launch configuration's `IamInstanceProfile` is read as an ARN exactly when it
    /// names an instance profile of an account, and as a name otherwise.
    #[test]
    fn a_launch_configuration_profile_is_read_as_an_arn_or_as_a_name() {
        let account = ::engine::emulator_account_id();
        let held_arn = format!("arn:aws:iam::{account}:instance-profile/asg-profile");
        let under_path = format!("arn:aws:iam::{account}:instance-profile/service-role/asg-profile");
        let role_arn = format!("arn:aws:iam::{account}:role/asg-profile");
        // (case, the value the configuration carries, the ARN read off it, the name read
        // off it)
        let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            ("a plain profile name", "asg-profile", None, Some("asg-profile")),
            (
                "a name that happens to carry a slash",
                "team/asg-profile",
                None,
                Some("team/asg-profile"),
            ),
            ("the ARN of a profile", &held_arn, Some(&held_arn), None),
            (
                "the ARN of a profile under a path",
                &under_path,
                Some(&under_path),
                None,
            ),
            (
                "an ARN naming another IAM entity type",
                &role_arn,
                None,
                Some(&role_arn),
            ),
            (
                "a value that is not an ARN at all",
                "instance-profile/asg-profile",
                None,
                Some("instance-profile/asg-profile"),
            ),
        ];
        for (case, reference, arn, name) in cases {
            let read = spelled_profile_reference(reference)
                .unwrap_or_else(|| panic!("case {case:?}: expected {reference} to be read"));
            assert_eq!(
                (read.arn.as_deref(), read.name.as_deref()),
                (*arn, *name),
                "case {case:?}: expected ({arn:?}, {name:?}), got ({:?}, {:?})",
                read.arn,
                read.name
            );
        }
        assert!(
            spelled_profile_reference("").is_none(),
            "case \"an empty value\": expected it to name no instance profile"
        );
    }
}
