Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8e7edcdb3fa5431fa8bcd5fe3e300fe0352aedaa

# UpdateCluster drops clientToken: a retried update is applied a second time instead of replaying the first call's result

_Filed: 2026-11-19T14:02:13Z_
_Operation: UpdateCluster | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8e7edcdb3fa5431fa8bcd5fe3e300fe0352aedaa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateClusterRequest$clientToken` is an `@idempotencyToken` (the operation itself is `@idempotent`) whose documentation states that "the subsequent retries with the same client token return the result from the original successful request and they have no additional effect". The `UpdateCluster` handler never reads `input.client_token`: the token is neither consulted nor recorded, so a retry presenting the SAME token is executed as a fresh update. That retry HAS an additional effect — it re-applies every member the request carries, bumps `modifiedAt`, and moves the cluster's status from `ACTIVE` back to `UPDATING` with a fresh `PendingChange::Activate` — and if the retried body differs from the first (e.g. another `scheduler.version`), the second value is stored instead of the first call's result being replayed.

This is not a feature the crate lacks: the sibling update handler in this same crate implements exactly the replay, through the crate's own idempotency index (`ComAmazonawsPcsAwsparallelcomputingserviceState::remembered_create` / `remember_create`), and an integration test pins it for compute node groups ("a repeated `@idempotencyToken` returns the first call's result and has no further effect"). `UpdateCluster` is the one update path that skips the helper the crate already owns. (The already-open DeleteCluster/parameter-ignored issue covers the three DELETE operations, whose output is empty and whose retry merely earns a refusal; the update path is a different defect — the retry mutates stored state and reports a cluster that was ACTIVE as UPDATING again.)

## Evidence

Smithy (smithy_query doc, UpdateClusterRequest$clientToken): "<p>A unique, case-sensitive identifier that you provide to ensure the idempotency of the request. Idempotency ensures that an API request completes only once. With an idempotent request, if the original request completes successfully, the subsequent retries with the same client token return the result from the original successful request and they have no additional effect. ...</p>" (smithy_query operation=UpdateCluster: `"clientToken": {"target": "SBClientToken", "idempotencyToken": true, ...}`, `"idempotent": true`).

The handler never reads the member. The input type carries it — services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/types/update_cluster_types.rs:18: `pub client_token: Option<String>,` — while `grep -n client_token src/ops/update_cluster.rs` matches only the doc header (src/ops/update_cluster.rs:56). The whole write is unconditional on it — src/ops/update_cluster.rs:245-264:
```
        // A member the request carries is applied; one it omits leaves the stored value
        // alone. A request that asks for no change leaves the cluster where it is rather
        // than moving its status for nothing.
        let mut changed = false;
        if let Some(scheduler) = input.scheduler {
            cluster.scheduler.version = scheduler.version;
            changed = true;
        }
...
        if changed {
            // PCS reports a cluster as UPDATING while it applies the change; the service
            // owes it ACTIVE again from the next request on.
            cluster.status = crate::state::STATUS_UPDATING.to_string();
            cluster.modified_at = crate::state::now_date_time();
            cluster.pending = Some(crate::state::PendingChange::Activate);
        }
```

The sibling update handler in the SAME crate does consult and record the token — src/ops/update_compute_node_group.rs:349-359:
```
        // An `@idempotencyToken` retry returns the first call's result and has NO further
        // effect, so the stored record is answered without applying the change a second
        // time. The token is remembered with the group the first call changed, so the same
        // token naming another group is a conflict rather than a second update.
        if let Some(remembered) = state
            .remembered_create(Self::NAME, input.client_token.as_ref())
            .cloned()
        {
```
and src/ops/update_compute_node_group.rs:467-472: `state.remember_create(Self::NAME, input.client_token.as_ref(), Some(&cluster_id), &group_id,);`. The index itself is src/state.rs:744-748: "/// Remember what `operation` with `token` created, so a retry replays it. /// /// An existing entry is never overwritten — the token names the FIRST call's /// resource for as long as it is remembered".

Test coverage: tests/all/cluster_update_and_endpoints/compute_node_group_update.rs:748-751 pins the rule for the group path ("/// RULE: a repeated `@idempotencyToken` returns the first call's result and has no further /// effect" / `async fn a_repeated_client_token_replays_the_first_update()`), including lines 788-811 where a retry carrying a DIFFERENT `amiId` under the same token is replayed and "expected the retry to have no further effect". No test sends a `clientToken` to UpdateCluster — `grep -rn clientToken tests/` matches only that suite and the CREATE calls in tests/all/cluster_lifecycle.rs:793, 806, 819, 833, 911, 918 — so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateCluster(clusterName="tok-update", scheduler={type:SLURM,version:"24.11"}, size=SMALL, networking={subnetIds:[<a subnet EC2 holds>]}) -> 200 (CREATING).
2. GetCluster(clusterIdentifier="tok-update") -> 200, status ACTIVE (the settle pass materialised the create).
3. UpdateCluster(clusterIdentifier="tok-update", scheduler={version:"25.05"}, clientToken="update-token-0001") -> 200, cluster reported UPDATING, scheduler.version 25.05.
4. GetCluster -> 200, status ACTIVE, version 25.05.
5. Retry the byte-identical call: UpdateCluster(clusterIdentifier="tok-update", scheduler={version:"25.05"}, clientToken="update-token-0001").
6. Expected per the clientToken documentation: the first call's result is returned and the retry has no additional effect (the cluster stays ACTIVE with the modifiedAt the first update set). Observed: the update is applied again — the response reports status UPDATING and modifiedAt is bumped, and the next GetCluster shows the cluster transitioning ACTIVE again.
7. Variant proving the result is not replayed either: send the retry with the same token but scheduler={version:"25.11"} — the new version is stored, whereas the sibling UpdateComputeNodeGroup replays the first call's value for the same shape of retry (tests/all/cluster_update_and_endpoints/compute_node_group_update.rs:788-811).
