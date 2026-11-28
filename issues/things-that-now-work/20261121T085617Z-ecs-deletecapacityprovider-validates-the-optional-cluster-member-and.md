Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.capacity-provider-ops-on-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.capacity-provider-ops-on-missing-cluster-not-found|run#25|AssertionError|assert code is None, f"DeleteCapacityProvider: expected success, got {code}"
Issue-Commit: 0c46b3cc7da45ad21000fa4a6694a9d98cd58e91

# ECS DeleteCapacityProvider validates the optional `cluster` member and refuses ClusterNotFoundException instead of resolving the provider by name alone

> _Note: this issue was identified against the code base at commit `0c46b3cc7da45ad21000fa4a6694a9d98cd58e91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCapacityProvider

## Description
Cause 1 of 1: DeleteCapacityProvider treats the optional `cluster` input member as a resource that must exist, and refuses the whole call when it does not resolve, instead of locating the capacity provider by its own name/ARN.

AWS: Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json) models DeleteCapacityProviderRequest with `capacityProvider` (String, required, resourceIdentifier CapacityProviderName) and `cluster` (String, optional — "The name of the cluster that contains the capacity provider to delete. Managed instances capacity providers are cluster-scoped and can only be deleted from their associated cluster."). The model does list ClusterNotFoundException among the operation's errors, so the model alone does not settle when it is raised — the rest is recorded behavior only: in the recording, an existing capacity provider (a Managed Instances provider created in a real cluster) is deleted with `cluster` set to a name that exists in no cluster of the account, and AWS ANSWERS the call successfully (HTTP 200, `capacityProvider` in the body). AWS resolves the provider by its unique name/ARN; the unresolvable `cluster` value does not make the delete a refusal. The same recording shows the cluster-scoping rule IS enforced by the sibling operations: CreateCapacityProvider, DescribeCapacityProviders, UpdateCapacityProvider and PutClusterCapacityProviders against the same nonexistent cluster each return 400 ClusterNotFoundException with the message `Cluster not found.` (fixed text, no variable parts) — and the emulator matches AWS on all four. DeleteCapacityProvider is the one that does not validate it.

Emulator: the call is refused with HTTP 400 ClusterNotFoundException (message `Cluster not found.`, the same fixed text the emulator already uses for the sibling operations; its variable part is only the clause the emulator substitutes for a deleted-but-still-present cluster). The handler checks the request's cluster reference against the active-cluster map and returns before the provider is ever looked up, so no provider is retired and no `capacityProvider` view is returned.

Why: a client that deletes a capacity provider while passing a cluster reference that is stale, deleted or simply not the provider's scope gets a hard 400 where AWS deletes the provider. Teardown code and CDK/CloudFormation-style cleanup paths that pass `cluster` opportunistically therefore leave the provider ACTIVE forever, and callers branching on ClusterNotFoundException conclude the cluster is missing when the real resource they named — the provider — exists and was deletable. It also masks the distinct refusals AWS reserves for this operation (ClientException for a provider name that resolves to nothing, InvalidParameterException for a cluster-scoped provider addressed through the wrong cluster).

Fix: the DeleteCapacityProvider handler's input validation — the cluster-existence gate must not pre-empt provider resolution for this operation; resolve `capacityProvider` by name/ARN first and let an unresolvable `cluster` value not turn the delete into a ClusterNotFoundException refusal (sibling operations that DO validate the cluster are already correct and must keep their behavior).

Traces: ecs.capacity-provider-ops-on-missing-cluster-not-found (the same check's Create/Describe/Update/PutClusterCapacityProviders assertions pass, including the exact `Cluster not found.` wording; only the final DeleteCapacityProvider assertion fails).

## Evidence
Failing check: ecs.capacity-provider-ops-on-missing-cluster-not-found [mismatch]

    File "check_ecs_capacity_provider_ops_on_missing_cluster_not_found.py", line 271, in run
      assert code is None, f"DeleteCapacityProvider: expected success, got {code}"
    AssertionError: DeleteCapacityProvider: expected success, got ClusterNotFoundException

Expected (AWS, recorded): DeleteCapacityProvider(capacityProvider=<existing provider name>, cluster=<name of no existing cluster>) -> HTTP 200 with `capacityProvider` in the body; no error code raised.
Actual (emulator): HTTP 400, Error.Code = ClusterNotFoundException, Error.Message = "Cluster not found."; the provider is left untouched.

Same check, same emulator run, the four sibling calls against the same nonexistent cluster all match AWS (400 / ClusterNotFoundException / "Cluster not found."): CreateCapacityProvider, DescribeCapacityProviders, UpdateCapacityProvider, PutClusterCapacityProviders. The divergence is confined to DeleteCapacityProvider.

## Steps to Reproduce
1. ecs.CreateCluster with any fresh cluster name.
2. Create a capacity provider in that cluster (the check uses a Managed Instances provider, created with ecs.CreateCapacityProvider(name=<cp>, cluster=<cluster>, managedInstancesProvider=...)) and wait for its status to settle to a terminal value (ACTIVE or INACTIVE).
3. Call ecs.DeleteCapacityProvider(capacityProvider=<cp>, cluster="no-such-cluster-<random>") — a cluster name that exists in no cluster of the account.
4. AWS: the call returns HTTP 200 and the deleted provider in `capacityProvider`. Emulator: HTTP 400 ClusterNotFoundException "Cluster not found." and the provider is not retired.

Replay: run_behavioral_suite and re-run check ecs.capacity-provider-ops-on-missing-cluster-not-found.
