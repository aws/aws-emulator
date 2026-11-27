Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6e2d7185f30747a2356e601c1d412ac8e0be11c4

# A host's two reported ENIs are ids minted from EC2's counter but never written to EC2's network-interface store, on a premise that store contradicts

_Filed: 2026-11-19T03:34:01Z_
_Operation: CreateEnvironmentHost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6e2d7185f30747a2356e601c1d412ac8e0be11c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`launch_host_instance` puts a real bare-metal instance in the account's EC2 store, but the two `eni-` ids it reports as the host's `networkInterfaces` are invented from EC2's id counter and inserted nowhere: no `NetworkInterface` record is created for either. The comment that justifies this states that EC2 has no network-interface store and that every `…NetworkInterface` operation of that service is a scaffold stub — both false in this repo today. The consequences are a response that names resources no caller can resolve, and two stores that disagree about one host:

* `DescribeNetworkInterfaces` with either reported id answers `InvalidNetworkInterfaceID.NotFound`, because the id was never stored;
* `DescribeInstances` on the host's own `ec2InstanceId` reports ONE interface — the primary ENI `insert_instance` mints — whose id is neither of the two Amazon EVS reported;
* the model sentence this crate quotes for the member ("Amazon EVS provisions two identically configured ENIs in the VMkernel management subnet during host creation") describes interfaces that exist in that subnet, and none do.

The same fabrication reaches `CreateEnvironment`, whose `add_requested_hosts` calls the same writer for each entry of `hosts`. EC2 publishes the writer this path needs — `insert_launched_instance(record, &[LaunchedInterface])`, the same body `insert_instance` delegates to with an empty slice — so the host's interfaces can be stored as the launch's own, exactly as the instance already is.

## Evidence

The fabrication and its stale premise, services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs:660-667:

    // The ENIs. EC2 holds no network interface store yet (every `…NetworkInterface`
    // operation is still a scaffold stub), so the ids are minted from EC2's OWN id
    // counter — `eni-` + 17 hex digits, the form `ec2::network_interface_id_is_well_formed`
    // reads — and are therefore unique among the account's EC2 ids, ready to be inserted
    // into that store on the day it lands.
    let network_interface_ids = (0..HOST_NETWORK_INTERFACES)
        .map(|_| store.next_id("eni"))
        .collect();

That day has landed. services/service-com-amazonaws-ec2-amazonec2/src/state/store.rs:2273:

    pub network_interfaces: BTreeMap<String, NetworkInterface>,

services/service-com-amazonaws-ec2-amazonec2/src/state/instances_and_filters.rs:154-163 (the very writer EVS calls):

    /// Store a launched instance, appending it to its reservation — and create the two
    /// resources the launch gives it: the EBS root volume
    /// ([`Self::mint_launched_root_volume`]) and the primary network interface
    /// ([`Self::mint_launched_primary_interface`]).
    ///
    /// For a launch that NAMED the interfaces of its instance, [`Self::insert_launched_instance`]
    /// is the same write with those interfaces instead of the single default one
    pub fn insert_instance(&mut self, record: InstanceRecord) {
        self.insert_launched_instance(record, &[]);

and instances_and_filters.rs:199: `self.mint_launched_primary_interface(&instance_id);`, instances_and_filters.rs:2393: `network_interfaces: Some(self.instance_network_interfaces(&instance.instance_id)),`.

A reported id resolves to nothing — services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs:290-292:

    let Some(interface) = store.network_interfaces.get(&id) else {
        return Err(crate::state::invalid_network_interface_id_not_found(&id));
    };

(no `…NetworkInterface` handler of EC2 is a stub except `delete_network_interface_permission.rs:4`.)

The promise the EVS crate makes for these members, services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1245-1251:

    /// `ec2InstanceId` and `networkInterfaces` are reported from the EC2 resources the
    /// host's creation launched ([`Self::ec2`]) — the instance id the account's own
    /// Amazon EC2 store minted, and the ENIs attached to it — so a caller can look the
    /// host's instance up in EC2 with the id reported here.

and the model sentence the crate quotes, src/state/ec2_resources.rs:444-450: "Amazon EVS provisions two identically configured ENIs in the VMkernel management subnet during host creation".

Test coverage: tests/all/host_ec2_instances.rs:357-374 asserts only the SHAPE of the two ids (`is_long_ec2_id(interface, "eni")`, distinct) and never resolves them in EC2, so no test pins the fabrication with a reason.

## Steps to Reproduce

1. CreateEnvironment with vcfVersion VCF-5.2.1 and wait for environmentState CREATED (or use SELF_DEPLOYED and add a host).
2. CreateEnvironmentHost with host.hostName esx-1; note the returned host's ec2InstanceId and the two networkInterfaces[].networkInterfaceId values.
3. Call EC2 DescribeNetworkInterfaces with NetworkInterfaceIds set to the first reported id: the answer is InvalidNetworkInterfaceID.NotFound (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs:290).
4. Call EC2 DescribeInstances with that ec2InstanceId: its NetworkInterfaces carries exactly one interface, whose id is neither of the two Amazon EVS reported.
