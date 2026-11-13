// SPDX-License-Identifier: CC0-1.0

//! What the account's own Amazon EC2 store says about the resources an Amazon EVS
//! request names.
//!
//! An Amazon EVS environment is deployed INTO the caller's existing network: `vpcId` is
//! "the VPC associated with the environment" and `serviceAccessSubnetId` "the subnet
//! that is used to establish connectivity between the Amazon EVS control plane and
//! VPC", so both name resources the account already holds. Whether it holds them is not
//! something this service can answer from its own records — it is a fact about EC2 — so
//! the question is put to EC2's OWN store accessor in the shared engine, exactly as
//! CloudWatch RUM's authorization gate reads the IAM store rather than keeping a second
//! copy of what IAM knows. Nothing is cached: a VPC created a moment ago is usable at
//! once, and one that has been deleted stops being usable at once.
//!
//! Every other member of such a request that names an EC2 resource of the caller's
//! network is resolved in that same store, and against the same VPC: the security groups
//! that "allow traffic between the Amazon EVS control plane and your VPC"
//! ([`security_group_failures`]), the network access control list a VLAN subnet uses
//! ([`network_acl_failures`]) and the blocks the VLAN subnets are provisioned from
//! ([`vlan_placement_failures`]). The route server peers the environment's route server
//! endpoints peer with are resolved there too, by id and not against a VPC, because a peer
//! is identified account-wide ([`route_server_peer_failures`]).
//!
//! A host is placed on EC2 resources of the caller's account the same way: a
//! `placementGroupId` is "the unique ID of the placement group where the host is placed"
//! and a `dedicatedHostId` "the unique ID of the Amazon EC2 Dedicated Host", and Amazon
//! EVS creates neither of them, so whether the account holds them is EC2's fact to state
//! and is asked of the same store ([`host_placement_failures`]).
//!
//! A store this emulator cannot read at all — an engine that is not the in-memory one —
//! is EVIDENCE OF NOTHING and so refuses nothing: every function here answers `None`
//! for that case, and the callers treat `None` as "no failure to report".
//!
//! The same store is also WRITTEN here, for the resources Amazon EVS creates in the
//! account on the caller's behalf:
//!
//! * an Amazon EVS host IS an EC2 bare metal instance (`Host` doc), so
//!   [`launch_host_instance`] puts that instance in EC2's own store through EC2's own
//!   state API, and the host reports the id EC2 minted for it;
//!   [`terminate_host_instance`] ends it again when the host is deleted from its
//!   environment, so no instance of a host no environment holds is left running;
//! * Amazon EVS "provisions the VLAN subnets" of an environment inside the VPC the
//!   request names, and "environment deletion also deletes the associated Amazon EVS
//!   VLAN subnets" (`CreateEnvironment` / `DeleteEnvironment` docs) — those subnets are
//!   EC2 subnets of the caller's account, so they are created in and removed from EC2's
//!   own store through EC2's own writers ([`provision_vlan_subnets`],
//!   [`release_vlan_subnets`]) rather than existing only inside this service's records:
//!   an id no other service holds would name nothing.
//!
//! * an Elastic IP address `AssociateEipToVlan` associates with a public HCX VLAN stays
//!   ALLOCATED to the caller, but the ASSOCIATION is an EC2 fact — `allocationId` is an
//!   EC2 allocation and the `eipassoc-` id the operation reports is an EC2
//!   `AssociationId` — so it is written onto that allocation's own `Address` record
//!   ([`associate_elastic_ip`]) and cleared from it again ([`release_elastic_ips`]):
//!   `DescribeAddresses` then reports the address a VLAN is using as associated, and one
//!   allocation cannot be associated twice over.
//!
//! Nothing else is written: the VPC and the service access subnet an environment is
//! deployed into belong to the caller and are only read, and no Elastic IP address is
//! ever allocated or released here — only associated and disassociated.
//!
//! Nothing in this module does arithmetic on an address or a prefix length of its own:
//! every judgement about a CIDR block a caller supplied is EC2's own published helper
//! (`canonical_subnet_ipv4_cidr`, `ipv4_cidr_block_contains`,
//! `ipv4_cidr_blocks_overlap`), so one reading of a block serves both services, and a
//! block those cannot read is skipped rather than guessed at. There is no indexing or
//! slicing of a caller-supplied string here, and no `unwrap`, `expect` or `panic!`: a
//! request cannot panic a handler through this module.
//!
//! LOCKING. Each function takes EC2's store lock for the shortest span that answers it,
//! does a bounded amount of work under it (at most one pass over the VPC's subnets, or
//! the ten subnets of one environment), and never awaits while holding it. The lock is
//! `engine::ServiceState::lock`, which recovers from poisoning rather than propagating a
//! panic, so a panicking request cannot wedge either store. The callers hold this
//! service's own lock across these calls, so the order is always EVS-then-EC2; EC2 never
//! reaches this service, so no cycle exists.

use std::time::Instant;

use crate::ops::types::ValidationExceptionField;

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The account's EC2 store in the shared engine, through EC2's own accessor.
fn ec2_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ec2::ComAmazonawsEc2Amazonec2State>> {
    ec2::service_state(engine)
}

/// The clause `CreateEnvironment` states for a `vpcId` the account holds no VPC for.
///
/// The sentence names the id the request gave and nothing from the store, and it is the
/// wording Amazon EVS states this refusal with: a request-derived validation clause that
/// names the member's value and what to check about it.
pub fn invalid_vpc_clause(vpc_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        "vpcId",
        format!(
            "The VPC id {vpc_id} is not valid or does not exist. Please verify the VPC id \
             exists and is valid."
        ),
    )
}

/// The clause `CreateEnvironment` states for a `serviceAccessSubnetId` that is not a
/// subnet of the VPC the request named — because the account holds no such subnet, or
/// because the subnet it holds belongs to another VPC.
///
/// Stated in the same form as [`invalid_vpc_clause`], because it is the same kind of
/// failure: a member naming a network resource the environment cannot be deployed into.
pub fn invalid_subnet_clause(subnet_id: &str, vpc_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        "serviceAccessSubnetId",
        format!(
            "The service access subnet id {subnet_id} is not valid or does not exist in VPC \
             {vpc_id}. Please verify the subnet id exists in the VPC and is valid."
        ),
    )
}

/// Every failure of the preconditions that the network a `CreateEnvironment` request
/// names exists: the VPC, and the service access subnet INSIDE that VPC.
///
/// The subnet is only judged once the VPC resolves — a subnet is identified within the
/// VPC that holds it, so an unknown VPC leaves nothing to judge it against, and the
/// refusal then names only the VPC. An unreadable EC2 store reports no failure at all.
pub fn network_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    service_access_subnet_id: &str,
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    if !store.vpcs.contains_key(vpc_id) {
        return vec![invalid_vpc_clause(vpc_id)];
    }
    let subnet_in_vpc = store
        .subnets
        .get(service_access_subnet_id)
        .is_some_and(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id));
    if subnet_in_vpc {
        Vec::new()
    } else {
        vec![invalid_subnet_clause(service_access_subnet_id, vpc_id)]
    }
}

/// The clause stated for a `securityGroups` entry that is not a security group of the VPC
/// the request named — because the account holds no group with that id, or because the
/// group it holds belongs to another VPC.
///
/// `member` is the PATH of the entry that named it, so the clause points at the element
/// the caller wrote. Stated in the same form as [`invalid_subnet_clause`], because it is
/// the same kind of failure: a member naming a network resource of the VPC the
/// environment is deployed into.
pub fn invalid_security_group_clause(
    member: &str,
    group_id: &str,
    vpc_id: &str,
) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The security group id {group_id} given for {member} is not valid or does not \
             exist in VPC {vpc_id}. Please verify the security group id exists in the VPC \
             and is valid."
        ),
    )
}

/// One security group a request named, and the path of the member that named it, so a
/// refusal names the element the caller wrote.
#[derive(Debug, Clone, Copy)]
pub struct SecurityGroupReference<'a> {
    /// The path of the member this id came from.
    pub member: &'a str,
    /// The `GroupId` the request supplied.
    pub group_id: &'a str,
}

/// Every failure of the precondition that each security group a request names is a
/// security group OF the VPC the environment is deployed into.
///
/// The groups "allow traffic between the Amazon EVS control plane and your VPC for Amazon
/// EVS service access" (`ServiceAccessSecurityGroups` doc) and Amazon EVS creates none of
/// them, so each id names a group the account already holds; a security group is scoped
/// to one VPC (`SecurityGroup$VpcId`), so a group of some other VPC cannot carry that
/// VPC's traffic. The same rule [`network_failures`] applies to `serviceAccessSubnetId`,
/// asked of the same store.
///
/// An unknown VPC leaves nothing to judge the groups against, so nothing is reported —
/// [`invalid_vpc_clause`] is the clause for that — and so does an unreadable EC2 store.
pub fn security_group_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    groups: &[SecurityGroupReference<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    if !store.vpcs.contains_key(vpc_id) {
        return Vec::new();
    }
    groups
        .iter()
        .filter(|group| {
            store
                .security_groups
                .get(group.group_id)
                .is_none_or(|held| held.vpc_id.as_deref() != Some(vpc_id))
        })
        .map(|group| invalid_security_group_clause(group.member, group.group_id, vpc_id))
        .collect()
}

/// The clause stated for a network ACL id that is not a network access control list of the
/// VPC the request named — because the account holds no list with that id, or because the
/// list it holds belongs to another VPC.
///
/// `member` is the PATH of the member that named it, so the clause points at what the
/// caller wrote. Stated in the same form as [`invalid_security_group_clause`], because it is
/// the same kind of failure: a member naming a network resource of the VPC the environment
/// is deployed into.
pub fn invalid_network_acl_clause(
    member: &str,
    network_acl_id: &str,
    vpc_id: &str,
) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The network ACL id {network_acl_id} given for {member} is not valid or does \
             not exist in VPC {vpc_id}. Please verify the network ACL id exists in the VPC \
             and is valid."
        ),
    )
}

/// One network access control list a request named, and the path of the member that named
/// it, so a refusal names what the caller wrote.
#[derive(Debug, Clone, Copy)]
pub struct NetworkAclReference<'a> {
    /// The path of the member this id came from.
    pub member: &'a str,
    /// The `NetworkAclId` the request supplied.
    pub network_acl_id: &'a str,
}

/// Every failure of the precondition that each network access control list a request names
/// is a list OF the VPC the environment is deployed into.
///
/// A network ACL is an Amazon EC2 resource of the caller's own account that Amazon EVS does
/// not create — every VPC comes with its default one and `CreateNetworkAcl` mints the rest —
/// so an id that names no list of the account names nothing for a VLAN subnet to use,
/// exactly as a `securityGroups` entry names no group ([`security_group_failures`]) and
/// `vpcId` no VPC ([`network_failures`]). A network ACL belongs to ONE VPC
/// (`NetworkAcl$VpcId`) and controls traffic of that VPC's subnets, so a list of some other
/// VPC cannot be used by a VLAN subnet provisioned in this one — the same scope the security
/// groups are judged at.
///
/// An unknown VPC leaves nothing to judge the lists against, so nothing is reported —
/// [`invalid_vpc_clause`] is the clause for that — and so does an unreadable EC2 store.
pub fn network_acl_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    network_acls: &[NetworkAclReference<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    if !store.vpcs.contains_key(vpc_id) {
        return Vec::new();
    }
    network_acls
        .iter()
        .filter(|acl| {
            store
                .network_acls
                .get(acl.network_acl_id)
                .is_none_or(|held| held.vpc_id.as_deref() != Some(vpc_id))
        })
        .map(|acl| invalid_network_acl_clause(acl.member, acl.network_acl_id, vpc_id))
        .collect()
}

/// The clause stated for a `placementGroupId` the account's own Amazon EC2 store holds no
/// placement group for.
///
/// `member` is the PATH of the member that named it, so one clause serves every operation
/// that accepts a host: `CreateEnvironmentHost`'s single `host` and `CreateEnvironment`'s
/// `hosts` entries. Stated in the same form as [`invalid_vpc_clause`], because it is the
/// same kind of failure: a member naming an EC2 resource the launch cannot be placed in.
pub fn invalid_placement_group_clause(
    member: &str,
    placement_group_id: &str,
) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The placement group id {placement_group_id} is not valid or does not exist. \
             Please verify the placement group id exists and is valid."
        ),
    )
}

/// The clause stated for a `dedicatedHostId` the account's own Amazon EC2 store holds no
/// Dedicated Host for. See [`invalid_placement_group_clause`] for the `member` path.
pub fn invalid_dedicated_host_clause(
    member: &str,
    dedicated_host_id: &str,
) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The Dedicated Host id {dedicated_host_id} is not valid or does not exist. \
             Please verify the Dedicated Host id exists and is valid."
        ),
    )
}

/// The EC2 placement references ONE requested host names, and the path of the member that
/// named it, so a refusal names the member the caller wrote.
///
/// `member_prefix` is that member's path without the leaf — `host` for
/// `CreateEnvironmentHost`, `hosts[<index>]` for a `CreateEnvironment` entry — so the two
/// operations state one rule with the paths their own inputs have.
#[derive(Debug, Clone, Copy)]
pub struct HostPlacementReferences<'a> {
    /// The path of the `HostInfoForCreate` member these references came from.
    pub member_prefix: &'a str,
    /// The `placementGroupId` the host named, when it named one.
    pub placement_group_id: Option<&'a str>,
    /// The `dedicatedHostId` the host named, when it named one.
    pub dedicated_host_id: Option<&'a str>,
}

/// Every failure of the precondition that the EC2 resources a requested host is PLACED on
/// exist in the account's own Amazon EC2 store: the placement group it names, and the
/// Dedicated Host it names.
///
/// An Amazon EVS host IS an EC2 bare metal instance (`Host` doc), so a host that names a
/// placement group ("The unique ID of the placement group where the host is placed",
/// `HostInfoForCreate$placementGroupId`) or a Dedicated Host ("The unique ID of the Amazon
/// EC2 Dedicated Host") names a resource the account has to hold already — Amazon EVS
/// creates neither — exactly as `vpcId` and `serviceAccessSubnetId` do for an environment
/// ([`network_failures`]). Both stores are read through EC2's own accessor, under one lock
/// for the whole batch, so the four hosts of a `CreateEnvironment` cost one acquisition.
///
/// An unreadable EC2 store reports no failure at all, the "evidence of nothing" answer
/// every function here gives for it.
pub fn host_placement_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    hosts: &[HostPlacementReferences<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let mut failures = Vec::new();
    for host in hosts {
        if let Some(placement_group_id) = host
            .placement_group_id
            .filter(|id| !store.placement_groups.contains_key(*id))
        {
            failures.push(invalid_placement_group_clause(
                &format!("{}.placementGroupId", host.member_prefix),
                placement_group_id,
            ));
        }
        if let Some(dedicated_host_id) = host
            .dedicated_host_id
            .filter(|id| !store.dedicated_hosts.contains_key(*id))
        {
            failures.push(invalid_dedicated_host_clause(
                &format!("{}.dedicatedHostId", host.member_prefix),
                dedicated_host_id,
            ));
        }
    }
    failures
}

/// The clause stated for a route server peer id the account's own Amazon EC2 store holds
/// no route server peer for.
///
/// `member` is the PATH of the entry that named it, so the clause points at the element the
/// caller wrote. Stated in the same form as [`invalid_placement_group_clause`], because it
/// is the same kind of failure: a member naming an Amazon EC2 resource of the caller's
/// account that the account does not hold.
pub fn invalid_route_server_peer_clause(member: &str, peer_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The route server peer id {peer_id} given for {member} is not valid or does not \
             exist. Please verify the route server peer id exists and is valid."
        ),
    )
}

/// One route server peer a request named, and the path of the member that named it, so a
/// refusal names the element the caller wrote.
#[derive(Debug, Clone, Copy)]
pub struct RouteServerPeerReference<'a> {
    /// The path of the member this id came from.
    pub member: &'a str,
    /// The `RouteServerPeerId` the request supplied.
    pub peer_id: &'a str,
}

/// Every failure of the precondition that each route server peer a request names is a peer
/// the account's own Amazon EC2 store holds.
///
/// A route server peer is an EC2 resource of the caller's account — `RouteServerPeerId` is
/// minted by `CreateRouteServerPeer` and Amazon EVS creates none — so an id that names no
/// peer of the account names nothing to peer with, exactly as `vpcId` names no VPC to
/// deploy into ([`network_failures`]) and a `placementGroupId` no group to place a host in
/// ([`host_placement_failures`]). The question asked is the one EC2 itself answers for such
/// an id: whether the account holds a peer record of it, whatever state that peer is in.
///
/// An unreadable EC2 store reports no failure at all, the "evidence of nothing" answer
/// every function here gives for it.
pub fn route_server_peer_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    peers: &[RouteServerPeerReference<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    peers
        .iter()
        .filter(|peer| !store.route_server_peers.contains_key(peer.peer_id))
        .map(|peer| invalid_route_server_peer_clause(peer.member, peer.peer_id))
        .collect()
}

/// How many elastic network interfaces Amazon EVS attaches to a host.
///
/// "Amazon EVS provisions two identically configured ENIs in the VMkernel management
/// subnet during host creation. One ENI is active, and the other is in standby mode for
/// automatic switchover during a failure scenario" (`NetworkInterface` shape doc), which
/// is also the `@length(max: 2)` the model puts on `Host$networkInterfaces`.
pub const HOST_NETWORK_INTERFACES: usize = 2;

/// The EC2 resources Amazon EVS created for one ESX host: the bare metal instance that
/// IS the host, and the elastic network interfaces attached to it.
///
/// The two always travel together, because one launch produces both — which is why a
/// host either reports `ec2InstanceId` AND `networkInterfaces` or reports neither.
#[derive(Debug, Clone)]
pub struct HostEc2Resources {
    /// The `InstanceId` of the EC2 instance the host runs on, as EC2's own store minted
    /// it: the id `DescribeInstances` answers for.
    pub instance_id: String,
    /// The `NetworkInterfaceId` of each ENI attached to the host, in attachment order.
    pub network_interface_ids: Vec<String>,
}

/// What a host launch needs to know about the host and the environment it joins.
///
/// Every field is a fact the request or the stored environment already carries: nothing
/// here is invented, so the instance EC2 ends up holding describes the same host Amazon
/// EVS reports.
#[derive(Debug, Clone, Copy)]
pub struct HostLaunch<'a> {
    /// The Region the environment lives in — the Region the request was addressed to.
    pub region: &'a str,
    /// The EC2 bare metal instance type the request asked for.
    pub instance_type: &'a str,
    /// The SSH key pair the request named.
    pub key_name: &'a str,
    /// The host's management IP address, taken out of the `vmkManagement` VLAN subnet:
    /// the instance's primary private IPv4 address, because that address IS the host's.
    pub management_ip: &'a str,
    /// The `vmkManagement` VLAN subnet the host's ENIs live in.
    pub management_subnet_id: &'a str,
    /// The environment's service access subnet, read for the Availability Zone the
    /// environment is deployed in.
    pub service_access_subnet_id: &'a str,
}

/// The physical processor cores one host running on `instance_type` has — the cores a
/// VCF license has to cover for that host, which is what the `KEY_COVERAGE` check
/// ("Verifies that the VCF license key allocates sufficient vCPU cores for all deployed
/// hosts") counts.
///
/// Amazon EC2's OWN fact about the instance type, read through its instance-type record
/// (`VCpuInfo$DefaultCores`, what `DescribeInstanceTypes` reports): an Amazon EVS host
/// IS an EC2 bare metal instance, so its core count is EC2's to state and never a number
/// of this service's own. A fact about the TYPE, so it reads no store and takes no lock.
///
/// `None` for an instance type EC2 holds no record for, because the cores of a host
/// running on it are then not known at all.
pub fn instance_type_cores(instance_type: &str) -> Option<u64> {
    let record = ec2::instance_type_record(instance_type)?;
    u64::try_from(record.default_cores).ok()
}

/// Launch the EC2 bare metal instance an ESX host IS, and attach its ENIs.
///
/// "An ESX host that runs on an Amazon EC2 bare metal instance" (`Host` doc): the host
/// is not an Amazon EVS record that merely mentions an instance, it is that instance, so
/// adding a host to an environment has to put a real instance in the ACCOUNT's own EC2
/// store — the one `DescribeInstances` answers from — and report its id. The instance is
/// written through EC2's own state API ([`ec2::ComAmazonawsEc2Amazonec2State::insert_instance`]),
/// so it is indistinguishable from one `RunInstances` launched: `pending` with the move
/// to `running` the clock owes it, its primary private address the host's own management
/// address, and its subnet the VLAN subnet that address came out of.
///
/// `None` — no instance launched, so the host reports neither `ec2InstanceId` nor
/// `networkInterfaces` rather than naming resources that do not exist — when the EC2
/// store cannot be read at all (the same "evidence of nothing" answer every function in
/// this module gives for that case), or when that store is at a ceiling of its own.
///
/// The caller holds this service's own store while this runs. That nesting is safe in
/// one direction only, and this is it: Amazon EC2's handlers never read Amazon EVS
/// state, so EVS -> EC2 is the only order either lock is ever taken in.
pub fn launch_host_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: HostLaunch<'_>,
) -> Option<HostEc2Resources> {
    let state = ec2_state(engine)?;
    let mut store = state.lock();

    // The Availability Zone the environment is deployed in: the zone of the service
    // access subnet the caller gave it, which is a fact about that EC2 subnet rather
    // than a choice. An account whose store no longer holds that subnet falls back to
    // the Region's first zone, the way EC2's own placement does — and a Region with no
    // zone at all leaves nothing to place an instance in, so nothing is launched: a
    // Region name is not a zone name, and storing one would report a zone no
    // `DescribeAvailabilityZones` lists.
    let availability_zone = store
        .subnets
        .get(launch.service_access_subnet_id)
        .and_then(|subnet| subnet.availability_zone.clone())
        .or_else(|| {
            ec2::availability_zones_in_region(launch.region)
                .first()
                .map(|zone| zone.zone_name.clone())
        })?;

    // The EC2 store's own ceilings, asked BEFORE anything is minted: this writer is
    // bound by them exactly as EC2's own launch paths are.
    let payload_bytes = launch.instance_type.len()
        + launch.key_name.len()
        + launch.management_subnet_id.len()
        + availability_zone.len();
    if store.instances.len() >= ec2::MAX_STORED_INSTANCES
        || store.would_exceed_stored_bytes(payload_bytes)
    {
        return None;
    }

    // One host is one instance, so it is its own reservation, the way a Spot Instance
    // launch is.
    let reservation_id = store.mint_reservation_id();
    store.insert_reservation(ec2::ReservationRecord {
        reservation_id: reservation_id.clone(),
        owner_id: ::engine::emulator_account_id().to_string(),
        requester_id: None,
        instance_ids: Vec::new(),
    });
    let instance_id = store.mint_instance_id();
    store.insert_instance(ec2::InstanceRecord {
        instance_id: instance_id.clone(),
        reservation_id,
        // The ESX image Amazon EVS boots a host from is published by Amazon EVS, and
        // this emulator's AMI catalogue holds no such image: an `ami-` id invented here
        // would name an image `DescribeImages` cannot report, so none is claimed.
        image_id: None,
        instance_type: launch.instance_type.to_string(),
        key_name: Some(launch.key_name.to_string()),
        // The host's ENIs live in the VMkernel management subnet (`NetworkInterface`
        // doc), which is the subnet its management address came out of.
        subnet_id: Some(launch.management_subnet_id.to_string()),
        // The environment's service access security groups guard the Amazon EVS control
        // plane's access to the VPC, not the host, so the launch names no group.
        security_groups: Vec::new(),
        private_dns_name: ec2::private_dns_name(launch.management_ip, launch.region),
        private_ip: launch.management_ip.to_string(),
        // A host is reached over the VLAN subnets, never over a public address.
        public_ip: None,
        // Just launched: `pending`, with the move to `running` the clock owes it on
        // EC2's own span — the host's own CREATING -> CREATED runs on Amazon EVS's.
        state: ec2::InstanceStateName::Pending,
        pending_transition: Some(ec2::PendingTransition::starting_now(
            ec2::InstanceStateName::Running,
        )),
        state_transition_reason: None,
        state_reason: None,
        // Nobody has asked to reboot a host that has just been launched, and nobody has
        // sent it a diagnostic interrupt either.
        queued_reboots: 0,
        last_reboot_queued_at: None,
        diagnostic_interrupts: 0,
        last_diagnostic_interrupt_at: None,
        // The host's FIRST boot: its operating system comes up as the instance finishes
        // starting, the same instant the `pending` -> `running` move above falls due.
        boots: Vec::new(),
        // The guest's own storage starts empty: what is on it is what the instance's
        // user-data program puts there, boot by boot.
        guest_files: ::std::collections::BTreeMap::new(),
        boot_due: Some(::std::time::SystemTime::now() + ec2::TRANSITION_DELAY),
        launch_time: ::std::time::SystemTime::now(),
        ami_launch_index: 0,
        // "EC2 instances created through Amazon EVS do not support associating an IAM
        // instance profile" (`instanceType` doc) and nothing passes user data to an ESX
        // host either.
        user_data: None,
        placement_az: availability_zone,
        monitoring_state: ec2::MONITORING_STATE_DISABLED.to_string(),
        // Detailed monitoring is off from the start, so there is no move for EC2's clock
        // to make; only `MonitorInstances` records one.
        monitoring_transition: None,
        // Amazon EVS configures none of the option sets an EC2 instance reports (its
        // metadata options, enclave, tenancy and the rest), so every one of them is at its
        // documented default.
        detail: ec2::InstanceDetail::default(),
        architecture: ec2::architecture_for(launch.instance_type),
        // An ESX host boots the image Amazon EVS publishes, not a kernel or RAM disk
        // image a launch names, so it carries neither; source/destination checking
        // starts at the default every EC2 instance starts at.
        kernel_id: None,
        ramdisk_id: None,
        source_dest_check: ec2::DEFAULT_SOURCE_DEST_CHECK,
        ebs_optimized: false,
        hibernation_configured: false,
        disable_api_termination: false,
        // The environment's tags tag the ENVIRONMENT; nothing tags the instance.
        tags: ::std::collections::BTreeMap::new(),
        client_token: None,
        // Not a Spot Instance: an ESX host is launched on demand.
        spot_instance_request_id: None,
    });
    // The ENIs. EC2 holds no network interface store yet (every `…NetworkInterface`
    // operation is still a scaffold stub), so the ids are minted from EC2's OWN id
    // counter — `eni-` + 17 hex digits, the form `ec2::network_interface_id_is_well_formed`
    // reads — and are therefore unique among the account's EC2 ids, ready to be inserted
    // into that store on the day it lands.
    let network_interface_ids = (0..HOST_NETWORK_INTERFACES)
        .map(|_| store.next_id("eni"))
        .collect();
    Some(HostEc2Resources {
        instance_id,
        network_interface_ids,
    })
}

/// Terminate the EC2 bare metal instance an ESX host IS, in the account's own Amazon EC2
/// store, and report whether this call moved one.
///
/// The counterpart of [`launch_host_instance`], and owed for the same reason: a host IS
/// an EC2 instance ("An ESX host that runs on an Amazon EC2 bare metal instance", `Host`
/// doc), so a host that is deleted from its environment cannot leave that instance
/// running in the account — `DescribeInstances` would report an instance of a host no
/// Amazon EVS environment holds, and the account would keep paying for it. The change is
/// written through EC2's own record exactly as that service's own termination writes it:
/// `shutting-down` with the move to `terminated` the clock owes it, and the terminated
/// record kept, so the instance is indistinguishable from one `TerminateInstances`
/// stopped.
///
/// `false` when the EC2 store cannot be read at all, when it holds no instance of that
/// id, or when the instance is already on its way to `terminated` — nothing to do in any
/// of the three.
///
/// The caller holds this service's own store while this runs, the one safe nesting order
/// (EVS -> EC2) every writer here takes.
pub fn terminate_host_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_id: &str,
) -> bool {
    let Some(state) = ec2_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    let Some(instance) = store.instances.get_mut(instance_id) else {
        return false;
    };
    if instance.state.is_terminating() {
        return false;
    }
    instance.state = ec2::InstanceStateName::ShuttingDown;
    instance.pending_transition = Some(ec2::PendingTransition::starting_now(
        ec2::InstanceStateName::Terminated,
    ));
    // The caller asked for the host to go, so the shutdown is the client's, which is the
    // reason EC2 records for a termination a caller asked for.
    instance.state_transition_reason = Some(ec2::user_initiated_reason(
        ::std::time::SystemTime::now(),
    ));
    instance.state_reason = Some((
        "Client.UserInitiatedShutdown".to_string(),
        "Client.UserInitiatedShutdown: User initiated shutdown".to_string(),
    ));
    true
}

/// What came of asking Amazon EC2 to record the association of an Elastic IP allocation
/// with an Amazon EVS VLAN subnet.
///
/// An `eipassoc-` id is an EC2 identifier and the association is a change to the EC2
/// `Address` record — the one `DescribeAddresses` answers from — so EC2 is where the
/// association is made and where the id comes from, never this service.
#[derive(Debug, Clone)]
pub enum ElasticIpAssociation {
    /// EC2 recorded the association on the allocation's `Address`: the `AssociationId`
    /// it minted, and the public address it holds for the allocation.
    Associated {
        /// The `eipassoc-` id EC2's own id minter gave the association.
        association_id: String,
        /// The Elastic IP address EC2 holds for the allocation, when it holds one.
        ip_address: Option<String>,
    },
    /// The allocation is ALREADY associated — with another VLAN, or with any other EC2
    /// resource — so nothing was written: one Elastic IP address carries one association
    /// at a time (`Resource.AlreadyAssociated`).
    AlreadyAssociated {
        /// The association the allocation already carries.
        association_id: String,
        /// The instance it is associated with, when it is associated with one.
        instance_id: Option<String>,
    },
    /// The account's EC2 store holds no allocation of that id, so there was no `Address`
    /// record to associate: the member names an EC2 resource of the caller's account the
    /// account does not hold, which [`invalid_allocation_clause`] is the refusal for.
    NoSuchAllocation,
    /// There is no readable EC2 store on this engine, so nothing was written and nothing
    /// is known: the same "evidence of nothing" answer every other function here gives.
    NoEc2Store,
}

/// The clause stated for an `allocationId` the account's own Amazon EC2 store holds no
/// Elastic IP allocation for.
///
/// Stated in the same form as [`invalid_vpc_clause`], because it is the same kind of
/// failure: a member naming an Amazon EC2 resource of the caller's account that the
/// account does not hold, so the request cannot be carried out on it. The sentence is
/// built from the request — it names the id the caller gave and nothing from the store.
pub fn invalid_allocation_clause(allocation_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        "allocationId",
        format!(
            "The Elastic IP address allocation id {allocation_id} is not valid or does not \
             exist. Please verify the allocation id exists and is valid."
        ),
    )
}

/// Record on the account's own Amazon EC2 store that `allocation_id` is now associated
/// with an Amazon EVS VLAN subnet, and answer the association EC2 made.
///
/// `AssociateEipToVlan` consumes an EC2 resource: `allocationId` is "the Elastic IP
/// address allocation ID" of the caller's account and the `eipassoc-` id the operation
/// reports is an EC2 `AssociationId`, so the association has to live on the EC2 `Address`
/// record rather than only inside this service — otherwise `DescribeAddresses` reports an
/// address as unassociated while a VLAN is using it, and the same allocation could be
/// associated twice over. An allocation EC2 already holds an association for is NOT
/// re-associated: it is reported back as [`ElasticIpAssociation::AlreadyAssociated`] so
/// the caller can refuse the request, which is EC2's own single-use rule for an Elastic IP
/// address (`Resource.AlreadyAssociated`).
///
/// `networkInterfaceId`, `networkInterfaceOwnerId` and `privateIpAddress` stay ABSENT on
/// the record: they name the elastic network interface in the VLAN subnet the address is
/// bound to, and this emulator mints no interface for the HCX appliance, so an id here
/// would name a resource no operation could resolve — exactly the reading EC2's own
/// `AssociateAddress` takes of those members.
///
/// The caller holds this service's own store while this runs, which is the one safe
/// nesting order (EVS -> EC2), the same as [`launch_host_instance`].
pub fn associate_elastic_ip<Eng: Send + Sync + 'static>(
    engine: &Eng,
    allocation_id: &str,
) -> ElasticIpAssociation {
    let Some(state) = ec2_state(engine) else {
        return ElasticIpAssociation::NoEc2Store;
    };
    let mut store = state.lock();
    let Some(held) = store.addresses.get(allocation_id) else {
        return ElasticIpAssociation::NoSuchAllocation;
    };
    if let Some(association_id) = held.association_id.clone() {
        return ElasticIpAssociation::AlreadyAssociated {
            association_id,
            instance_id: held.instance_id.clone(),
        };
    }
    // The id comes from EC2's OWN monotonic id source, so no two associations of the
    // account share one whether EC2 or Amazon EVS made them. `eipassoc-` + 17 hex digits
    // is 26 characters, which is also the `@length(max: 26)` this service's own
    // `AssociationId` carries.
    let association_id = store.next_id(ec2::ELASTIC_IP_ASSOCIATION_ID_PREFIX);
    let Some(address) = store.addresses.get_mut(allocation_id) else {
        return ElasticIpAssociation::NoSuchAllocation;
    };
    address.association_id = Some(association_id.clone());
    ElasticIpAssociation::Associated {
        association_id,
        ip_address: address.public_ip.clone(),
    }
}

/// End the EC2 associations `associations` names — each `(allocation id, association
/// id)` — on the account's own Amazon EC2 store, and report how many were ended.
///
/// The inverse of [`associate_elastic_ip`], and the counterpart of
/// [`release_vlan_subnets`]: an Elastic IP address a VLAN no longer uses goes back to
/// being an unassociated allocation of the account, "disassociated … but remains
/// allocated to your account" (EC2's `AssociateAddress` doc on the same transition), so
/// it can be associated again. Only an address whose `AssociationId` is still the one
/// this service recorded is touched, so a later association made elsewhere for the same
/// allocation is never cleared by a stale record of ours.
pub fn release_elastic_ips<Eng: Send + Sync + 'static>(
    engine: &Eng,
    associations: &[(String, String)],
) -> usize {
    if associations.is_empty() {
        return 0;
    }
    let Some(state) = ec2_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    let mut released = 0;
    for (allocation_id, association_id) in associations {
        let ours = store
            .addresses
            .get(allocation_id)
            .and_then(|address| address.association_id.as_deref())
            == Some(association_id.as_str());
        if ours {
            // EC2's own primitive for ending an association, so an address released by
            // Amazon EVS is left in exactly the state `DisassociateAddress` leaves one in.
            store.detach_elastic_ip(allocation_id);
            released += 1;
        }
    }
    released
}

/// The `Subnet$Type` Amazon EC2 reports for a subnet Amazon EVS provisioned:
/// "Indicates if this is a subnet used with Amazon Elastic VMware Service (EVS).
/// Possible values are `Elastic VMware Service` or no value" (`Subnet$Type` doc, EC2's
/// own model). It is what marks a subnet of the account's EC2 store as one of an
/// environment's VLAN subnets, so [`release_vlan_subnets`] removes only the subnets
/// this service provisioned and never one the caller created.
pub const EVS_SUBNET_TYPE: &str = "Elastic VMware Service";

/// One VLAN subnet of an environment, as the request described it: the `initialVlans`
/// member it came from (so a refusal names the member the caller sent) and the CIDR
/// block supplied for it.
#[derive(Debug, Clone, Copy)]
pub struct VlanSubnetRequest<'a> {
    /// The path of the `initialVlans` member this VLAN subnet came from.
    pub member: &'a str,
    /// The CIDR block the request supplied for it.
    pub cidr: &'a str,
}

/// One VLAN subnet as Amazon EC2 provisioned it: the id EC2 minted for it, the block it
/// stored, and the Availability Zone the subnet is in.
#[derive(Debug, Clone)]
pub struct ProvisionedVlanSubnet {
    /// The `SubnetId` EC2's own id minter gave the subnet.
    pub subnet_id: String,
    /// The IPv4 CIDR block EC2 stored for the subnet — the requested block in the
    /// canonical form EC2 holds one in, so the VLAN and the subnet report ONE block.
    pub cidr_block: String,
    /// The zone name of the subnet (`us-west-2a`), as EC2 holds it.
    pub availability_zone: String,
}

/// What came of asking Amazon EC2 to provision an environment's VLAN subnets.
#[derive(Debug)]
pub enum VlanSubnetProvisioning {
    /// EC2 created one subnet per requested VLAN, in the order they were asked for.
    Provisioned(Vec<ProvisionedVlanSubnet>),
    /// There is no readable EC2 store on this engine, so nothing was created and
    /// nothing is known: the caller falls back to ids of its own, exactly as the
    /// read-side functions above treat an unreadable store as evidence of nothing.
    NoEc2Store,
    /// No Availability Zone could be named for the environment's subnets, so NOTHING
    /// was created: a subnet of no zone is a record EC2 itself never produces, and the
    /// caller answers the server error EC2's own `CreateSubnet` answers for a Region
    /// with no zone rather than storing one.
    ZoneUnknown,
    /// The account's EC2 store cannot hold that many more subnets, so NOTHING was
    /// created: the environment's VLAN subnets are provisioned together or not at all.
    SubnetLimitExceeded {
        /// How many subnets the store already holds.
        held: usize,
        /// The ceiling it holds them under (EC2's own `MAX_SUBNETS`).
        limit: usize,
    },
}

/// The IPv4 CIDR blocks associated with one VPC: its primary block and every block a
/// later association added, which together are the address space a subnet of that VPC
/// has to come out of.
fn vpc_ipv4_blocks(vpc: &ec2_types::Vpc) -> Vec<String> {
    let mut blocks: Vec<String> = vpc.cidr_block.clone().into_iter().collect();
    for association in vpc.cidr_block_association_set.iter().flatten() {
        if let Some(block) = association.cidr_block.clone() {
            if !blocks.contains(&block) {
                blocks.push(block);
            }
        }
    }
    blocks
}

/// Every failure of the preconditions an environment's VLAN subnet blocks must meet
/// AGAINST THE VPC they are provisioned inside: each block is a block of the VPC's own
/// address space, and overlaps no subnet that VPC already holds ("Amazon EVS VLAN
/// subnet CIDR blocks must not overlap with other subnets in the VPC", `InitialVlans`
/// doc).
///
/// This is the same overlap rule `CreateEnvironment` applies among an environment's own
/// ten blocks, at the scope the doc states it: the whole VPC. It has to hold, because
/// each block becomes a real EC2 subnet of that VPC, and EC2 holds no two overlapping
/// subnets in one VPC. It is also the same IMPLEMENTATION of that rule: the type
/// `CreateEnvironment` judges the environment's own blocks with
/// ([`crate::state::Ipv4Cidr`]) reads a block through these same published helpers, so
/// one block cannot be read one way here and another way there.
///
/// An unknown VPC leaves nothing to judge the blocks against, so nothing is reported —
/// [`invalid_vpc_clause`] is the clause for that — and so does an unreadable EC2 store.
pub fn vlan_placement_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    planned: &[VlanSubnetRequest<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let Some(vpc) = store.vpcs.get(vpc_id) else {
        return Vec::new();
    };
    let vpc_blocks = vpc_ipv4_blocks(vpc);
    let mut failures = Vec::new();
    for vlan in planned {
        // A block the subnet rules cannot read or size is already refused by the
        // operation's own CIDR clauses, so it is not judged a second time here.
        let Ok(canonical) = ec2::canonical_subnet_ipv4_cidr(vlan.cidr) else {
            continue;
        };
        if !vpc_blocks
            .iter()
            .any(|block| ec2::ipv4_cidr_block_contains(block, &canonical))
        {
            failures.push(super::field_failure(
                vlan.member,
                &format!(
                    "must be a block of the CIDR of VPC {vpc_id} ({blocks}), because \
                     Amazon EVS provisions the VLAN subnet inside that VPC",
                    blocks = vpc_blocks.join(", "),
                ),
            ));
            continue;
        }
        let conflicting = store
            .subnets
            .iter()
            .filter(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id))
            .find(|(_, subnet)| {
                subnet
                    .cidr_block
                    .as_deref()
                    .is_some_and(|held| ec2::ipv4_cidr_blocks_overlap(held, &canonical))
            })
            .map(|(subnet_id, _)| subnet_id.clone());
        if let Some(conflicting) = conflicting {
            failures.push(super::field_failure(
                vlan.member,
                &format!("overlaps the CIDR block of subnet {conflicting} in VPC {vpc_id}"),
            ));
        }
    }
    failures
}

/// The single Availability Zone every subnet of the environment's VPC is in — its zone
/// name and that zone's id.
///
/// "Amazon EVS requires that all VPC subnets exist in a single Availability Zone in a
/// Region where the service is available" (`vpcId` doc), and the service access subnet
/// is one of those subnets, so ITS zone is the environment's zone and the VLAN subnets
/// are provisioned there. A service access subnet the store holds without a zone leaves
/// the Region's first zone, which is the zone `CreateSubnet` itself picks for a request
/// that names none; `None` when neither names one, which is no zone to put a subnet in.
fn environment_zone(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    region: &str,
    service_access_subnet_id: &str,
) -> Option<(String, String)> {
    let held = store
        .subnets
        .get(service_access_subnet_id)
        .and_then(|subnet| {
            let zone_name = subnet.availability_zone.clone()?;
            let zone_id = subnet
                .availability_zone_id
                .clone()
                .or_else(|| ec2::zone_id_for_zone_name(&zone_name))
                .unwrap_or_default();
            Some((zone_name, zone_id))
        });
    held.or_else(|| {
        ec2::availability_zones_in_region(region)
            .into_iter()
            .next()
            .map(|zone| (zone.zone_name, zone.zone_id))
    })
    .filter(|(zone_name, _)| !zone_name.is_empty())
}

/// How many subnets the account's EC2 store holds and the ceiling it holds them under,
/// when it cannot take `count` more — `None` when there is room (or no readable store,
/// which refuses nothing).
///
/// A READ, so a create can refuse on the EC2 subnet ceiling BEFORE it mints anything of
/// its own: [`provision_vlan_subnets`] enforces the same ceiling and writes nothing when
/// it is reached, but by then the create has already minted an environment id.
pub fn subnet_room_shortfall<Eng: Send + Sync + 'static>(
    engine: &Eng,
    count: usize,
) -> Option<(usize, usize)> {
    let state = ec2_state(engine)?;
    let held = state.lock().subnets.len();
    (held + count > ec2::MAX_SUBNETS).then_some((held, ec2::MAX_SUBNETS))
}

/// Provision one EC2 subnet per requested VLAN in the account's own Amazon EC2 store,
/// inside `vpc_id`, and report what EC2 made of each.
///
/// "When you specify SELF_DEPLOYED for vcfVersion, Amazon EVS provisions only the VLAN
/// subnets" and provisions them in every other mode too, so these are real subnets of
/// the caller's VPC: EC2's own [`ec2::ComAmazonawsEc2Amazonec2State::insert_subnet`]
/// mints each id, ARN and available-address count, so `DescribeSubnets` reports a VLAN
/// subnet exactly as it reports one `CreateSubnet` created, and each carries the
/// [`EVS_SUBNET_TYPE`] that marks it as an Amazon EVS subnet.
///
/// Each subnet starts `pending` and the clock owes it `available` one EC2
/// `TRANSITION_DELAY` later, the same span on the same monotonic clock as a subnet
/// `CreateSubnet` created — EC2 owns its own resource's lifecycle, so the span is EC2's
/// and not this service's.
///
/// NOTHING is written when the store cannot hold them all, or when no Availability Zone
/// can be named for them: an environment's VLAN subnets are provisioned together or not
/// at all, and never as a subnet of no zone.
pub fn provision_vlan_subnets<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    vpc_id: &str,
    service_access_subnet_id: &str,
    planned: &[VlanSubnetRequest<'_>],
) -> VlanSubnetProvisioning {
    let Some(state) = ec2_state(engine) else {
        return VlanSubnetProvisioning::NoEc2Store;
    };
    let mut store = state.lock();
    let held = store.subnets.len();
    if held + planned.len() > ec2::MAX_SUBNETS {
        return VlanSubnetProvisioning::SubnetLimitExceeded {
            held,
            limit: ec2::MAX_SUBNETS,
        };
    }
    let Some((zone_name, zone_id)) = environment_zone(&store, region, service_access_subnet_id)
    else {
        return VlanSubnetProvisioning::ZoneUnknown;
    };
    let available_at = Instant::now() + ec2::TRANSITION_DELAY;
    let mut provisioned = Vec::with_capacity(planned.len());
    for vlan in planned {
        // "We modify the specified CIDR block to its canonical form"
        // (`CreateSubnetRequest$CidrBlock`): the subnet is stored the way EC2 stores
        // one. A block EC2 cannot canonicalise cannot reach here — the operation's own
        // CIDR clauses refuse it — so the request's own text is the honest fallback.
        let cidr_block = ec2::canonical_subnet_ipv4_cidr(vlan.cidr)
            .unwrap_or_else(|_| vlan.cidr.to_string());
        let subnet_id = store.insert_subnet(ec2::SubnetSpec {
            vpc_id: vpc_id.to_string(),
            cidr_block,
            availability_zone: zone_name.clone(),
            availability_zone_id: zone_id.clone(),
            region: region.to_string(),
            state: ec2::SUBNET_STATE_PENDING.to_string(),
            // Only a default VPC's own subnets are the default for their zone, and a
            // VLAN subnet does not auto-assign a public IPv4 address: a public VLAN is
            // reached through the Elastic IP addresses `AssociateEipToVlan` associates.
            default_for_az: false,
            map_public_ip_on_launch: false,
            // Tags "don't propagate to any other cluster or Amazon Web Services
            // resources" (`CreateEnvironmentRequest$tags` doc), so the VLAN subnets
            // carry none of the environment's.
            tags: Vec::new(),
        });
        if let Some(subnet) = store.subnets.get_mut(&subnet_id) {
            subnet.r#type = Some(EVS_SUBNET_TYPE.to_string());
        }
        store
            .subnet_available_at
            .insert(subnet_id.clone(), available_at);
        let cidr_block = store
            .subnets
            .get(&subnet_id)
            .and_then(|subnet| subnet.cidr_block.clone())
            .unwrap_or_else(|| vlan.cidr.to_string());
        provisioned.push(ProvisionedVlanSubnet {
            subnet_id,
            cidr_block,
            availability_zone: zone_name.clone(),
        });
    }
    VlanSubnetProvisioning::Provisioned(provisioned)
}

/// Remove the VLAN subnets `subnet_ids` names from the account's own Amazon EC2 store,
/// and report how many were removed.
///
/// "Environment deletion also deletes the associated Amazon EVS VLAN subnets"
/// (`DeleteEnvironment` doc): the subnets this service provisioned go away with the
/// environment, so the account is not left holding a subnet of an environment that no
/// longer exists — and the VPC's address space is free for the next environment.
///
/// Only a subnet EC2 reports as an Amazon EVS subnet ([`EVS_SUBNET_TYPE`]) is removed,
/// so an id that has come to name some other subnet of the account cannot delete it.
pub fn release_vlan_subnets<Eng: Send + Sync + 'static>(
    engine: &Eng,
    subnet_ids: &[String],
) -> usize {
    let Some(state) = ec2_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    let mut removed = 0;
    for subnet_id in subnet_ids {
        let is_evs_subnet = store
            .subnets
            .get(subnet_id)
            .and_then(|subnet| subnet.r#type.as_deref())
            == Some(EVS_SUBNET_TYPE);
        if is_evs_subnet && store.remove_subnet(subnet_id).is_some() {
            removed += 1;
        }
    }
    removed
}
