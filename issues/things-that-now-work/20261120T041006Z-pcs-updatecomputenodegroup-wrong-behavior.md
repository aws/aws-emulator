Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 85b68b7a028471b2503d25a82f72e45745cd6af4

# UpdateComputeNodeGroup stores scaleDownIdleTimeInSeconds = -1 as the override instead of removing it

_Filed: 2026-11-20T04:10:06Z_
_Operation: UpdateComputeNodeGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `85b68b7a028471b2503d25a82f72e45745cd6af4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateComputeNodeGroupSlurmConfigurationRequest$scaleDownIdleTimeInSeconds` documents a SENTINEL value: "A value of -1 removes the override and applies the cluster-level setting to this compute node group." The member is the per-group override of the cluster-level `scaleDownIdleTimeInSeconds`, and the same documentation states how "no override" is expressed: "If not specified, the cluster-level setting applies."

The handler treats -1 as an ordinary value to SET. `UpdateComputeNodeGroup` merges the requested Slurm options through `crate::state::apply_compute_node_group_slurm_update`, whose only rule is "a member the request carries replaces the stored one": a request carrying -1 therefore writes `Some(-1)` into the stored record, and `ComputeNodeGroupRecord::to_api` reports it back. So an update whose documented effect is to REMOVE the override leaves the group carrying an override — and one of minus one second — and `GetComputeNodeGroup` cannot be distinguished from a group that was genuinely configured with -1. The override the caller asked to drop survives every later read, and a subsequent update that omits the member keeps it (the merge leaves an omitted member alone), so there is no call that undoes it.

The create path writes the same member the same way (`impl From<ComputeNodeGroupSlurmConfigurationRequest> for ComputeNodeGroupSlurmConfiguration`, whose request member carries the identical sentence), so a `CreateComputeNodeGroup` carrying -1 also persists and reports -1 instead of a group with no override; the fix belongs in the one place both paths write the member. Nothing in the crate states the -1 rule or justifies ignoring it: the only mention of the value is a test constant that treats it purely as the low end of the member's `@range`.

## Evidence

Smithy — the member's own documentation (`smithy_query(query="shape", shape="UpdateComputeNodeGroupSlurmConfigurationRequest")`, member `scaleDownIdleTimeInSeconds`):
"<p>The time (in seconds) before an idle node is scaled down. If not specified, the cluster-level setting applies. This overrides the cluster-level <code>scaleDownIdleTimeInSeconds</code> setting. A value of <code>-1</code> removes the override and applies the cluster-level setting to this compute node group. Requires Slurm version 25.11 or later.</p>"
(The same sentence is on `ComputeNodeGroupSlurmConfigurationRequest$scaleDownIdleTimeInSeconds` and on the output shape `ComputeNodeGroupSlurmConfiguration$scaleDownIdleTimeInSeconds`.)

The handler path — services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/update_compute_node_group.rs:445-450:
```
        if let Some(slurm) = input.slurm_configuration {
            let stored = group.slurm_configuration.take();
            group.slurm_configuration =
                Some(crate::state::apply_compute_node_group_slurm_update(stored, slurm));
            changed = true;
        }
```
The merge it calls, which stores the sentinel as a value — src/state.rs:1468-1478:
```
/// The stored Slurm configuration an `UpdateComputeNodeGroup` leaves behind: a member the
/// request carries replaces the stored one, a member it omits leaves the stored one alone —
/// the compute-node-group counterpart of [`apply_slurm_update`].
pub fn apply_compute_node_group_slurm_update(
    stored: Option<ComputeNodeGroupSlurmConfiguration>,
    update: UpdateComputeNodeGroupSlurmConfigurationRequest,
) -> ComputeNodeGroupSlurmConfiguration {
    let mut merged = stored.unwrap_or_default();
    if let Some(seconds) = update.scale_down_idle_time_in_seconds {
        merged.scale_down_idle_time_in_seconds = Some(seconds);
    }
```
There is no branch for -1 anywhere: `grep -n 'scale_down_idle' src/` reports only src/state.rs:1430-1431 (the cluster merge, whose member is `@range(min: 1)` and has no sentinel), src/state.rs:1476-1477 (above), src/state.rs:1681 and src/state.rs:1708 (the two `From` conversions that copy the member verbatim: `scale_down_idle_time_in_seconds: request.scale_down_idle_time_in_seconds,`).

What the stored record reports — src/state.rs:598 (`ComputeNodeGroupRecord::to_api`): `slurm_configuration: self.slurm_configuration.clone(),` so the -1 travels straight back out on `GetComputeNodeGroup`/`ListComputeNodeGroups` reads of the group.

Test coverage: the current echo-back IS pinned, but only as a range bound, not as the sentinel — tests/all/cluster_update_and_endpoints/compute_node_group_update.rs:44-48:
```
/// The published bounds of `scaleDownIdleTimeInSeconds`
/// (`@range(min: -1, max: 10000000)` on
/// `UpdateComputeNodeGroupSlurmConfigurationRequest$scaleDownIdleTimeInSeconds`), and a
/// value between them.
const SCALE_DOWN_SECONDS: &[i64] = &[-1, 900, 10_000_000];
```
and compute_node_group_update.rs:223-230, which asserts the stored group reports back whatever was sent, -1 included:
```
    for seconds in SCALE_DOWN_SECONDS {
        slurm["scaleDownIdleTimeInSeconds"] = json!(seconds);
        rows.push((
            format!("slurmConfiguration-scaleDownIdleTimeInSeconds-{seconds}"),
            json!({"slurmConfiguration": {"scaleDownIdleTimeInSeconds": seconds}}),
            json!({"slurmConfiguration": slurm.clone()}),
        ));
    }
```
The stated reason for that row is the published `@range` minimum — not the documented meaning of -1 — so the -1 row's expectation has to change with the fix (the 900 and 10_000_000 rows, and every other row of that test, stay as they are).

## Steps to Reproduce

1. CreateCluster(clusterName="idle-host", scheduler={type:"SLURM", version:"25.11"}, size="SMALL", networking={subnetIds:[<a subnet EC2 resolves>]}) -> 200; the next request reports the cluster ACTIVE.
2. CreateComputeNodeGroup(clusterIdentifier="idle-host", computeNodeGroupName="nodes", subnetIds=[<same subnet>], customLaunchTemplate={id:"lt-0123456789abcdef0", version:"1"}, iamInstanceProfileArn="arn:aws:iam::<account>:instance-profile/AWSPCS-demo", scalingConfiguration={minInstanceCount:0, maxInstanceCount:4}, instanceConfigs=[{instanceType:"c6i.large"}], slurmConfiguration={scaleDownIdleTimeInSeconds:900}) -> 200.
3. GetComputeNodeGroup(clusterIdentifier="idle-host", computeNodeGroupIdentifier="nodes") -> 200 with computeNodeGroup.slurmConfiguration.scaleDownIdleTimeInSeconds = 900 (the per-group override is in place).
4. UpdateComputeNodeGroup(clusterIdentifier="idle-host", computeNodeGroupIdentifier="nodes", slurmConfiguration={scaleDownIdleTimeInSeconds:-1}) -> 200.
5. GetComputeNodeGroup again. Observed: computeNodeGroup.slurmConfiguration.scaleDownIdleTimeInSeconds = -1 — the override is still there, now holding the sentinel. Expected per the member's own documentation ("A value of -1 removes the override and applies the cluster-level setting to this compute node group", with "If not specified, the cluster-level setting applies"): the group reports NO scaleDownIdleTimeInSeconds at all.
6. Same with a create: CreateComputeNodeGroup(..., slurmConfiguration={scaleDownIdleTimeInSeconds:-1}) is persisted and read back as -1 instead of as a group with no override.
