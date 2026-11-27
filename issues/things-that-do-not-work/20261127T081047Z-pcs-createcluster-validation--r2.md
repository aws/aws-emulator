Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c3d790cdd40dba9d9a42d3f60dddb8b4dc07ea62

# CreateCluster accepts and persists more than one networking.subnetId, although the member documents "PCS currently supports only 1 subnet in this list"

_Filed: 2026-11-27T08:10:47Z_
_Operation: CreateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `c3d790cdd40dba9d9a42d3f60dddb8b4dc07ea62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`NetworkingRequest$subnetIds` carries an explicit note in its own documentation: "PCS currently supports only 1 subnet in this list." The handler never applies that upper bound. The only ceiling it puts on the member is the emulator's generic stored-list ceiling of 200 entries, and the state module's own comment about this member considers only its LOWER bound (the modelled `@length(min: 1)` the boundary enforces), never the documented maximum of one.

So a CreateCluster naming two, three or two hundred control-plane subnets is served, the whole list is persisted in the cluster record, and every later GetCluster/ListClusters reports a cluster whose control plane is spread over subnets the service documents as unsupported. Downstream logic then treats that as a first-class case: `ec2_subnets::placement_violation` builds a SET of cluster VPCs out of the control-plane subnets (src/state/ec2_subnets.rs:139), so a multi-subnet, multi-VPC cluster silently widens the VPC a compute node group may be placed in.

DISCLOSURE for the fixer: one existing row pins today's acceptance — tests/all/subnet_placement.rs:191-195 asserts a two-subnet create answers 200 — and tests/all/subnet_placement/cluster_vpc_placement.rs:479 builds a deliberately two-subnet cluster for the VPC-span case. Neither states a reason bearing on the COUNT: the rules those suites declare are about resolving EVERY id against EC2 (existence), and the file header says the rows merely "span what the rules generalise over: how many subnets a request names". The documented one-subnet maximum is addressed nowhere in src/.

## Evidence

SMITHY — the member's own documentation states the rule and the number. `smithy_query(query="shape", shape="NetworkingRequest")`, member `subnetIds`:
> "The list of subnet IDs where PCS creates an Elastic Network Interface (ENI) to enable communication between managed controllers and PCS resources. Subnet IDs have the form `subnet-0123456789abcdef0`. Subnets can't be in Outposts, Wavelength or an Amazon Web Services Local Zone. **PCS currently supports only 1 subnet in this list.**"

The output side words it in the singular too — `smithy_query(query="shape", shape="Networking")`, member `subnetIds`: "The ID of the subnet where PCS creates an Elastic Network Interface (ENI) ...".

HANDLER — the only bound applied to that member is the generic 200-entry stored-list ceiling; src/ops/create_cluster.rs:233-241:
```
        let stored_lists: [(&str, usize); 5] = [
            (
                "networking.subnetIds",
                input.networking.subnet_ids.as_ref().map_or(0, Vec::len),
            ),
```
checked at src/ops/create_cluster.rs:265-272 through src/state.rs:1606-1608:
```
pub fn stored_list_violation(entries: usize) -> Option<String> {
    (entries > MAX_STORED_LIST_ENTRIES).then(|| length_at_most_constraint(MAX_STORED_LIST_ENTRIES))
}
```
with src/state.rs:249: `pub const MAX_STORED_LIST_ENTRIES: usize = 200;`

and the whole list is then persisted — src/ops/create_cluster.rs:406: `networking: input.networking.into(),` (src/state.rs:2150-2157 copies `subnet_ids` through unchanged).

CRATE SIDE — the one comment that discusses this member's bounds considers only the minimum, and concludes nothing is owed here; src/state.rs:1613-1618:
```
/// A STORED group's subnet list is `SubnetIdList`, modelled `@length(min: 1)`
/// (`ComputeNodeGroup$subnetIds`), while the request members that place a group target the
/// unbounded `StringList`: this is the lower bound every path that WRITES that list holds it
/// to, stated once so a create and an update cannot disagree about it. The cluster's own
/// `NetworkingRequest$subnetIds` needs no entry here — it targets `SubnetIdList` itself, so
/// the boundary refuses an empty list before a handler runs.
```
No `@length(max:)` exists on `SubnetIdList` (`smithy_query(query="shape", shape="SubnetIdList")` reports `length: {min: 1}` only), so the boundary cannot refuse a longer list and the documented maximum is enforced nowhere.

DOWNSTREAM — the multi-subnet cluster is treated as a real shape, widening a group's allowed VPCs; src/state/ec2_subnets.rs:139-147:
```
    let cluster_vpcs: HashSet<&str> = control_plane.values().filter_map(Option::as_deref).collect();
    if cluster_vpcs.is_empty() {
        return None;
    }
    let outside = placed
        .values()
        .filter_map(Option::as_deref)
        .any(|vpc| !cluster_vpcs.contains(vpc));
```

TEST COVERAGE that pins today's acceptance (and would need updating) — tests/all/subnet_placement.rs:191-195:
```
        (
            "two subnets EC2 holds",
            vec![held_one.clone(), held_two.clone()],
            true,
        ),
```
under a header whose stated rules are about existence, not count (tests/all/subnet_placement.rs:7-14, 21-23), and tests/all/subnet_placement/cluster_vpc_placement.rs:479: `&create_cluster("span-host", "24.11", &control_plane),` with a two-subnet control plane.

## Steps to Reproduce

1. Mount PCS beside EC2 on one engine (the harness tests/all/subnet_placement.rs builds) and create one VPC with two subnets, subnet-A and subnet-B.
2. Call CreateCluster(clusterName="two-subnets", scheduler={type:SLURM,version:"24.11"}, size=SMALL, networking={subnetIds:[subnet-A, subnet-B]}).
3. Observed: HTTP 200, and cluster.networking.subnetIds carries BOTH ids; GetCluster reports the same two ids on every later read.
4. Expected per the member's own documentation ("PCS currently supports only 1 subnet in this list"): the create is refused on networking.subnetIds and nothing is persisted (ListClusters reports no such cluster).
5. To see the downstream effect, create the two subnets in DIFFERENT VPCs instead, create the cluster over both, then CreateComputeNodeGroup with subnetIds=[subnet-B]: the group is placed because the cluster's VPC set now holds both VPCs (ec2_subnets.rs:139-147), where a one-subnet cluster would have bounded it to one VPC.
