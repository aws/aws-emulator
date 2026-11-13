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

use ::service_com_amazonaws_ec2_amazonec2::ops::types::RequestLaunchTemplateData;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

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
    /// The group's tags; the ones marked `PropagateAtLaunch` go on the instances.
    pub tags: &'a [Tag],
}

/// One instance Amazon EC2 launched: its id, where EC2 placed it and what it runs.
#[derive(Debug, Clone)]
pub struct LaunchedInstance {
    pub instance_id: String,
    pub availability_zone: String,
    pub instance_type: Option<String>,
    pub image_id: Option<String>,
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
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is nowhere to launch an instance, so the
        // group is given none rather than an id nothing holds.
        return Vec::new();
    };
    let mut store = state.lock();
    let parameters = launch_parameters(&store, request);
    let placements = placements(&store, request);
    if placements.is_empty() {
        return Vec::new();
    }

    let mut tags: BTreeMap<String, String> = request
        .tags
        .iter()
        .filter(|tag| super::propagates_at_launch(tag))
        .map(|tag| (tag.key.clone(), tag.value.clone().unwrap_or_default()))
        .collect();
    tags.insert(GROUP_NAME_TAG.to_string(), request.group_name.to_string());

    let instance_type = parameters
        .instance_type
        .clone()
        .unwrap_or_else(|| ec2::DEFAULT_INSTANCE_TYPE.to_string());
    let monitoring_state = if parameters.monitoring_enabled {
        ec2::MONITORING_STATE_ENABLED
    } else {
        ec2::MONITORING_STATE_DISABLED
    };
    let launch_time = SystemTime::now();
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
            // A launched instance is `pending` and becomes `running` on Amazon EC2's own
            // clock, so a caller that describes it straight away still sees `pending` —
            // the same transient state the group reports it `Pending` for.
            state: ec2::InstanceStateName::Pending,
            pending_transition: Some(ec2::PendingTransition::starting_now(
                ec2::InstanceStateName::Running,
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
            // A launch template here configures none of the option sets an instance
            // reports (its metadata options, enclave, tenancy and the rest), so every one
            // of them is at its documented default.
            detail: ec2::InstanceDetail::default(),
            architecture: ec2::architecture_for(&instance_type),
            kernel_id,
            ramdisk_id,
            source_dest_check: ec2::DEFAULT_SOURCE_DEST_CHECK,
            ebs_optimized: parameters.ebs_optimized,
            hibernation_configured: false,
            disable_api_termination: false,
            tags: tags.clone(),
            // No client token: this launch is not a caller's request to replay.
            client_token: None,
            // An Auto Scaling group's own capacity is on-demand here: no Spot Instance
            // request is modelled, so the instance fulfills none.
            spot_instance_request_id: None,
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
        launched.push(LaunchedInstance {
            instance_id,
            availability_zone: placement.availability_zone.clone(),
            instance_type: Some(instance_type.clone()),
            image_id: parameters.image_id.clone(),
        });
    }
    launched
}

/// Stop the instances `instance_ids` names in the account's Amazon EC2 store, or start
/// them again — what a warm pool does to the instances it holds.
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
) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let now = SystemTime::now();
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
                instance.pending_transition = Some(ec2::PendingTransition::starting_now(
                    ec2::InstanceStateName::Stopped,
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
                instance.pending_transition = Some(ec2::PendingTransition::starting_now(
                    ec2::InstanceStateName::Running,
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
pub fn terminate<Eng: Send + Sync + 'static>(engine: &Eng, instance_ids: &[String]) {
    if instance_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let now = SystemTime::now();
    for instance_id in instance_ids {
        let Some(instance) = store.instances.get_mut(instance_id) else {
            continue;
        };
        if instance.state.is_terminating() {
            continue;
        }
        instance.state = ec2::InstanceStateName::ShuttingDown;
        instance.pending_transition = Some(ec2::PendingTransition::starting_now(
            ec2::InstanceStateName::Terminated,
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
        let Some(subnet) = store.subnets.get(subnet_id) else {
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
            .any(|placement| placement.subnet_id.as_deref() == Some(subnet_id))
        {
            continue;
        }
        placements.push(Placement {
            availability_zone: zone,
            subnet_id: Some(subnet_id.to_string()),
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
        };
    }

    // A mixed instances policy carries the group's launch template, and its overrides
    // name the instance types to launch; the first override is the one this takes, the
    // way an allocation strategy prefers the first type it is given.
    let policy_template = request
        .mixed_instances_policy
        .and_then(|policy| policy.launch_template.as_ref());
    let specification = request.launch_template.or_else(|| {
        policy_template.and_then(|template| template.launch_template_specification.as_ref())
    });
    let override_entry = policy_template
        .and_then(|template| template.overrides.as_ref())
        .and_then(|overrides| overrides.first());
    let data = specification.and_then(|specification| template_data(store, specification));

    LaunchParameters {
        image_id: override_entry
            .and_then(|entry| entry.image_id.clone())
            .or_else(|| data.as_ref().and_then(|data| data.image_id.clone())),
        instance_type: override_entry
            .and_then(|entry| entry.instance_type.clone())
            .or_else(|| data.as_ref().and_then(|data| data.instance_type.clone())),
        key_name: data.as_ref().and_then(|data| data.key_name.clone()),
        security_groups: data
            .as_ref()
            .map(|data| {
                data.security_group_ids
                    .iter()
                    .flatten()
                    .map(|group_id| (Some(group_id.clone()), None))
                    .chain(
                        data.security_groups
                            .iter()
                            .flatten()
                            .map(|group_name| (None, Some(group_name.clone()))),
                    )
                    .collect()
            })
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
    }
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
    // names, and the one each of that policy's overrides may name instead — the same set the
    // IAM instance profile of a group is resolved over
    // ([`super::iam_instance_profiles::specifications`]).
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
