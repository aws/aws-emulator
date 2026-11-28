Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b325123fedfa77f0bbc54662ef6abf5ee95872ba

# A vpcId whose subnets are not all in one Availability Zone is accepted, although the crate treats that requirement as a premise it never checks

_Filed: 2026-11-28T16:08:54Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `b325123fedfa77f0bbc54662ef6abf5ee95872ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironmentRequest$vpcId` documents three requirements on the VPC a request names. The handler enforces two of them — the VPC must exist and must not already carry another deployed environment (`vpc_is_in_use`) — but never enforces the single-Availability-Zone requirement: the only network precondition, `ec2_resources::network_failures`, asks the account's EC2 store whether the VPC exists and whether the service access subnet belongs to it, and nothing else. A `CreateEnvironment` naming a VPC whose subnets sit in two Availability Zones is therefore answered 200 and the environment is minted, although the member doc states Amazon EVS requires the opposite and `ValidationException` is the one client-input error this operation models.

The crate does not merely miss the rule, it RELIES on it: `ec2_resources::environment_zone` quotes that very sentence as its justification for taking the service access subnet's zone as "the single Availability Zone every resource of the environment is placed in", and `StoredVlan::availability_zone` is documented as "the single zone every subnet of the environment's VPC is in". So for a multi-AZ VPC every `Vlan$availabilityZone` this service reports — and the zone `launch_host_instance` places each host's EC2 bare metal instance in — states a fact about the caller's VPC that the account's own EC2 store contradicts.

The store needed to decide it is already read by this same handler under the same lock: `vlan_placement_failures` walks `store.subnets` filtered to the request's VPC, and `environment_zone` reads `subnet.availability_zone` off such a record.

## Evidence

Smithy (`smithy_query(query="doc", shape="CreateEnvironmentRequest", member="vpcId")`, mirrored in services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:303-310):
  "A unique ID for the VPC that the environment is deployed inside." / "Amazon EVS requires that all VPC subnets exist in a single Availability Zone in a Region where the service is available." / "The VPC cannot be used with any other deployed Amazon EVS environment."

The handler enforces the third sentence and not the second — src/ops/create_environment.rs:629-635:
```
        if state.vpc_is_in_use(&region, &input.vpc_id) {
            failures.push(crate::state::field_failure(
                "vpcId",
                "is already used by another deployed Amazon EVS environment, which Amazon EVS \
                 does not support",
```

The only network precondition, src/state/ec2_resources.rs:139-160 (`network_failures`): existence and VPC membership only, no zone is read:
```
    if !store.vpcs.contains_key(vpc_id) {
        return vec![invalid_vpc_clause(vpc_id)];
    }
    let subnet_in_vpc = store
        .subnets
        .get(service_access_subnet_id)
        .is_some_and(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id));
```

The premise the crate takes on trust — src/state/ec2_resources.rs:1229-1237 (`environment_zone`):
```
/// The single Availability Zone every resource of the environment is placed in — its zone
/// name and that zone's id.
///
/// "Amazon EVS requires that all VPC subnets exist in a single Availability Zone in a
/// Region where the service is available" (`vpcId` doc), and the service access subnet
/// is one of those subnets, so ITS zone is the environment's zone and the VLAN subnets
/// are provisioned there.
```
and src/state.rs:1501-1503:
```
    /// The Availability Zone of that subnet — the single zone every subnet of the
    /// environment's VPC is in.
```

The facts are already in hand on this path — src/state/ec2_resources.rs:1208-1218 walks the VPC's own subnets, and :1252 reads a subnet's `availability_zone`:
```
        let conflicting = store
            .subnets
            .iter()
            .filter(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id))
```
```
            let zone_name = subnet.availability_zone.clone()?;
```

Test coverage: tests/all/create_environment_network_membership.rs and tests/all/vlan_subnet_propagation.rs build every VPC with its subnets in ONE zone (`availability_zone: ZONE`, vlan_subnet_propagation.rs:530 asserts each VLAN subnet carries `row.zone`), so no test exercises — or pins with a stated reason — a VPC whose subnets span two zones.

## Steps to Reproduce

1. Against the EC2 emulator, create a VPC (e.g. 10.0.0.0/16) in us-east-1.
2. CreateSubnet twice in that VPC: one subnet in us-east-1a (the service access subnet) and one in us-east-1b, so the VPC's subnets are NOT all in one Availability Zone.
3. Call CreateEnvironment with that vpcId, the us-east-1a subnet as serviceAccessSubnetId, termsAccepted true, a valid vcfVersion and ten non-overlapping initialVlans blocks of the VPC.
4. Observe HTTP 200 with an environment in CREATING: the request naming a VPC the member doc says Amazon EVS requires to be single-AZ is accepted, with no ValidationException clause naming vpcId.
5. After the create settles, call ListEnvironmentVlans: every VLAN reports availabilityZone us-east-1a — the value the crate documents as "the single zone every subnet of the environment's VPC is in" — while ec2 DescribeSubnets still reports a subnet of the same VPC in us-east-1b.
