Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# CreateCluster drops clientToken, so a retried create under the same idempotency token answers ConflictException instead of the cluster it already minted

_Filed: 2026-11-27T21:22:29Z_
_Operation: CreateCluster | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCluster` is modelled `@idempotent` and its `clientToken` member carries `@idempotencyToken`, the trait an SDK fills in and REUSES verbatim on an automatic retry. The handler never reads `input.client_token`: it is not consulted before minting, it is not stored on `ClusterRecord`, and nothing in the crate records tokens that have already been served.

The consequence is not a silently dropped label but a changed outcome. The handler mints a fresh `cluster_id`/`cluster_arn` on every call and then refuses a `clusterName` the store already holds with `ConflictException`. So a client whose first `CreateCluster` succeeded but whose response was lost, and which retries with the SAME `clientToken` the SDK generated, is answered HTTP 409 `ConflictException` ("A cluster already exists with given name") rather than the cluster that token already created. The identical omission exists on `UpdateCluster`, whose `clientToken` member is likewise `@idempotencyToken` and likewise never read — a fix belongs in both handlers.

## Evidence

The model declares the trait pair — services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/OPERATIONS.md:44-45:

    - **Traits:** @idempotent
    - **Input:** `clusterName` (string, required), ... `kmsKeyId` (string), `clientToken` (string, @idempotencyToken), ...

and the handler's own module doc repeats it — src/ops/create_cluster.rs:62:

    //! * `clientToken` (String, @idempotencyToken) — The client token for the elastic cluster.

The handler builds the persisted record from every other input member and never names `client_token` — src/ops/create_cluster.rs:300-330 populates `cluster_name`, `admin_user_name`, `admin_user_password`, `auth_type`, `shard_capacity`, `shard_count`, `shard_instance_count`, `vpc_security_group_ids`, `subnet_ids`, `preferred_maintenance_window`, `preferred_backup_window`, `backup_retention_period`, `kms_key_id`, `shards`, `tags`, `pending_maintenance_actions` — and `grep -rn 'client_token' src/` matches ONLY the generated input structs:

    src/ops/types/create_cluster_types.rs:26:    pub client_token: Option<String>,
    src/ops/types/update_cluster_types.rs:24:    pub client_token: Option<String>,

What a retry therefore meets instead — src/ops/create_cluster.rs:293-298:

            if let Some(existing_arn) = guard.cluster_names.get(&cluster_name) {
                return Err(CreateClusterErrorModeled::ConflictException(
                    state::name_conflict(state::ResourceKind::Cluster, existing_arn),
                )
                .into());
            }

Test coverage: tests/all/duplicate_name_conflict_wording.rs and tests/all/cluster_lifecycle.rs exercise the duplicate-name conflict, but no test in tests/all sends a `clientToken` at all (`grep -rn 'clientToken' tests/` returns nothing), so the retry case is unexercised.

## Steps to Reproduce

1. POST /cluster with {"clusterName":"demo","authType":"PLAIN_TEXT","adminUserName":"admin","adminUserPassword":"Secret123!","shardCapacity":2,"shardCount":1,"clientToken":"11111111-1111-1111-1111-111111111111"} -> 200, note cluster.clusterArn.
2. Repeat the IDENTICAL request, same clientToken (what an SDK retry sends).
3. Expected for an @idempotent operation whose clientToken is @idempotencyToken: the same cluster the first call minted. Actual: HTTP 409 ConflictException "A cluster already exists with given name" from src/ops/create_cluster.rs:293-298, because the token was never recorded.
4. Same check against PUT /cluster/{clusterArn} with a repeated clientToken: the member is read nowhere in src/ops/update_cluster.rs either.
