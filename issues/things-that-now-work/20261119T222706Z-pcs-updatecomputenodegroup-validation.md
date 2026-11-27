Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dcd6c0968bb95ff573300c824ad4ea7810662c7a

# UpdateComputeNodeGroup accepts a subnetIds change into a VPC other than the cluster's

_Filed: 2026-11-19T22:27:06Z_
_Operation: UpdateComputeNodeGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `dcd6c0968bb95ff573300c824ad4ea7810662c7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateComputeNodeGroupRequest$subnetIds` documents the placement rule: "The subnets must be in the same VPC as the cluster." The handler resolves the request's subnets against EC2 for EXISTENCE only — `crate::state::ec2_subnets::violation` compares the ids EC2 reported with the ids the request named and reads nothing else off the answer (`Subnet::vpc_id` is never read anywhere in this crate) — and then overwrites the stored list. A compute node group whose instances could never reach the cluster controller is therefore stored and reported, rather than refused with the modelled `ValidationException` (whose own documentation lists "There are networking related errors, such as network validation failure" among its examples).

Everything the comparison needs is already on this path: the handler already makes the EC2 hop with exactly these ids (so each reported subnet's VPC is in the answer the emulator receives), and the cluster's own control-plane subnets are in state on the record this update resolves (`ClusterRecord::networking`). Only the comparison is missing.

Note on scope: the corresponding gap on the CREATE path is already filed (CreateComputeNodeGroup/validation), but this handler was still a 501 scaffold when that issue was written (its stub issue is only now fixed), so the update path — a second, independent writer of the same stored list — is covered by no issue. Both writers go through the same gate, so the fix belongs with it.

## Evidence

Smithy (smithy_query query="doc" shape="UpdateComputeNodeGroupRequest" member="subnetIds"): "<p>The list of subnet IDs where the compute node group provisions instances. The subnets must be in the same VPC as the cluster.</p>"

The only networking gate on the update path — services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/update_compute_node_group.rs:289-297:
```
        if let Some(subnet_ids) = input.subnet_ids.as_ref() {
            if let Some(refusal) =
                crate::state::ec2_subnets::violation(ctx, &region, subnet_ids).await
            {
                return Err(
                    UpdateComputeNodeGroupErrorModeled::ValidationException(refusal).into(),
                );
            }
        }
```
That gate decides on ids alone — src/state/ec2_subnets.rs:116-126:
```
            // EC2 reports one subnet per id it resolved, so an id it reported nothing for
            // names no subnet of the account — the same thing the refusal below says.
            let reported: Vec<&str> = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::subnet_id)
                .collect();
            if subnet_ids.iter().all(|id| reported.contains(&id.as_str())) {
                return None;
            }
```
and the handler then overwrites the stored placement without consulting the cluster — update_compute_node_group.rs:421-424:
```
        if let Some(subnet_ids) = input.subnet_ids {
            group.subnet_ids = subnet_ids;
            changed = true;
        }
```
The cluster's own placement that it would have to be compared against is on the record this very handler resolved — src/state.rs:267-268:
```
    /// The control-plane networking the request asked for.
    pub networking: Networking,
```

Test coverage: the compute-node-group update suite never mounts EC2 and never places an update in a second VPC — every `subnetIds` row is a synthetic id list (tests/all/cluster_update_and_endpoints/compute_node_group_update.rs:162-174: `json!({"subnetIds": [subnet(1), subnet(2), subnet(3)]})` etc.), and the only refusal rows are the empty list and the stored-list ceiling (same file, lines 612-618). Nothing pins the current acceptance.

## Steps to Reproduce

1. Boot the emulator with EC2 mounted (as tests/all/subnet_placement.rs does).
2. EC2: CreateVpc 10.42.0.0/16 -> vpc-A, CreateSubnet(vpc-A, 10.42.1.0/24) -> subnet-A; CreateVpc 10.99.0.0/16 -> vpc-B, CreateSubnet(vpc-B, 10.99.1.0/24) -> subnet-B.
3. PCS CreateCluster(clusterName="group-host", scheduler={type:SLURM,version:"24.11"}, size=SMALL, networking={subnetIds:[subnet-A]}) -> 200.
4. PCS CreateComputeNodeGroup(clusterIdentifier="group-host", computeNodeGroupName="nodes", subnetIds=[subnet-A], customLaunchTemplate={id:"lt-0123456789abcdef0",version:"1"}, iamInstanceProfileArn="arn:aws:iam::<account>:instance-profile/AWSPCS-demo", scalingConfiguration={minInstanceCount:0,maxInstanceCount:4}, instanceConfigs=[{instanceType:"c6i.large"}]) -> 200; one more request settles it ACTIVE.
5. PCS UpdateComputeNodeGroup(clusterIdentifier="group-host", computeNodeGroupIdentifier="nodes", subnetIds=[subnet-B]).
6. Expected per the member documentation: refused with ValidationException because subnet-B is not in the cluster's VPC. Observed: HTTP 200, and GetComputeNodeGroup reports subnetIds=[subnet-B] on a group whose cluster lives in vpc-A.
