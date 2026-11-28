Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: a4af9501c1d5c983daa3ec1227964a73edd2f5b5

# CreateComputeNodeGroup's idempotency replay ignores the computeNodeGroupName the request named, so a reused clientToken answers 200 with a different group and creates nothing

_Filed: 2026-11-23T23:16:42Z_
_Operation: CreateComputeNodeGroup | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `a4af9501c1d5c983daa3ec1227964a73edd2f5b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

On the `@idempotencyToken` replay path, `CreateComputeNodeGroup` compares only the CLUSTER the retry names against the remembered entry and then returns the first call's group. The `@required` `computeNodeGroupName` the second request carried is never read again on that path: the name-conflict check and the insert that would store it are both after the `return`. A second `CreateComputeNodeGroup` in the SAME cluster that reuses a token but names a different group is therefore answered HTTP 200 with the OTHER group's `name`/`id`/`arn`, nothing is persisted for the name the caller asked for, and no refusal tells the caller so. A client that derives its `clientToken` from a stack/template value rather than minting a fresh one per call (the common way this happens) believes it created `beta` while holding a body describing `alpha`; `GetComputeNodeGroup("beta")` answers not-found and `ListComputeNodeGroups` never reports it.

The crate's own comment on that block states the rule it is enforcing — "a retry naming a DIFFERENT cluster cannot slip past the replay and create a second group: it conflicts" — and the machinery for stating that refusal (`ConflictException` carrying "clientToken was already used to create compute node group …") is already in the handler. A retry naming a different GROUP in the same cluster slips past exactly that guard and is answered as if it had worked.

## Evidence

Smithy, `CreateComputeNodeGroupRequest$clientToken` (smithy_query doc): "Idempotency ensures that an API request completes only once. With an idempotent request, if the original request completes successfully, the subsequent retries with the same client token return the result from the original successful request and they have no additional effect." — and `CreateComputeNodeGroupRequest$computeNodeGroupName` is `@required`: "A name to identify the cluster. Example: MyCluster".

The replay, comparing the cluster only — services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_compute_node_group.rs:368-400:
```
        // An `@idempotencyToken` retry returns the first call's result and has no
        // further effect. The token is remembered with the cluster the first call
        // created the group in, so a retry naming a DIFFERENT cluster cannot slip past
        // the replay and create a second group: it conflicts.
        if let Some(remembered) = state
            .remembered_create(Self::NAME, input.client_token.as_ref())
            .cloned()
        {
            let first = (remembered.cluster_id.as_deref() == Some(cluster_id.as_str()))
                .then(|| {
                    state
                        .clusters
                        .get(&cluster_id)
                        .and_then(|cluster| cluster.compute_node_groups.get(&remembered.resource_id))
                })
                .flatten();
            let Some(first) = first else {
                return Err(CreateComputeNodeGroupErrorModeled::ConflictException(
                    ConflictException {
                        message: format!(
                            "clientToken was already used to create compute node group {}",
                            remembered.resource_id
                        ),
...
            return Ok(CreateComputeNodeGroupOutput {
                compute_node_group: Some(first.to_api()),
            });
        }
```
The name the request carried is used only BELOW that `return`: the duplicate-name conflict at create_compute_node_group.rs:420-436 (`.find(|group| group.name == input.compute_node_group_name)`) and the stored record at create_compute_node_group.rs:467 (`name: input.compute_node_group_name,`).

What the remembered entry holds — src/state.rs:835-851 `remember_create(... resource_name: None ...)` and src/state.rs:685-696 `RememberedCreate { cluster_id, resource_id, resource_name }`: nothing on the create path records or re-checks the requested name.

Test coverage: the only idempotency rows for this operation replay the request VERBATIM — tests/all/cluster_lifecycle.rs:824-836 (`first_group` re-sent unchanged, asserting the first call's id comes back). No test re-uses a token under a different `computeNodeGroupName`, so nothing pins the current answer.

## Steps to Reproduce

1. CreateCluster(clusterName="tok-cluster", scheduler={type:"SLURM",version:"24.11"}, size="SMALL", networking={subnetIds:[<a resolvable subnet>]}) -> 200; the next request observes it ACTIVE.
2. CreateComputeNodeGroup(clusterIdentifier="tok-cluster", computeNodeGroupName="alpha", <valid subnetIds/customLaunchTemplate/iamInstanceProfileArn/scalingConfiguration/instanceConfigs>, clientToken="group-token-0001") -> 200, computeNodeGroup.name="alpha", id=pcs_X.
3. CreateComputeNodeGroup(clusterIdentifier="tok-cluster", computeNodeGroupName="beta", same remaining members, clientToken="group-token-0001") — same cluster, same token, DIFFERENT name.
4. Observed: HTTP 200 whose body is computeNodeGroup.name="alpha" with id pcs_X. Nothing for "beta" is persisted and no refusal is returned.
5. GetComputeNodeGroup(clusterIdentifier="tok-cluster", computeNodeGroupIdentifier="beta") -> ResourceNotFoundException; ListComputeNodeGroups reports one group, "alpha".
