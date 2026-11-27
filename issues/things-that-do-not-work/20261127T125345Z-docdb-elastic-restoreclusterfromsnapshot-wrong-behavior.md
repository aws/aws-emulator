Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# RestoreClusterFromSnapshot asks the cluster-count guard under a lock it drops before inserting, so the guard is bypassable

_Filed: 2026-11-27T12:53:45Z_
_Operation: RestoreClusterFromSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RestoreClusterFromSnapshot calls `state::ensure_cluster_capacity(guard.clusters.len())` inside a FIRST `store.lock()` block, releases that lock to open the data-plane socket, then takes a SECOND lock under which it re-judges only the cluster NAME before inserting the new record. Nothing re-asks the capacity guard under the lock that performs the insert, so two restores that interleave between the two blocks both see `clusters.len() == MAX_CLUSTERS - 1`, both pass the guard, and both insert — leaving the store holding more clusters than the guard allows and more open sockets than `ClusterDataPlanes` documents it bounds.

This is inconsistent two ways inside this crate:

* the sibling mint path, `CreateCluster`, asks the SAME helper inside the one guard that also inserts, so for it the check and the act are atomic;
* this handler's own comment states that a value read before the lock was released has to be judged again after it is re-acquired — and then re-judges only the name, not the count.

The defect is the check-then-act window, not the value of the cap: the fix is to ask `ensure_cluster_capacity` under the same guard that inserts (as `CreateCluster` already does), which leaves every response and every existing test unchanged for the sequential case the suite covers.

## Evidence

RestoreClusterFromSnapshot — capacity asked in the FIRST guard block, which ends before the socket is opened:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/restore_cluster_from_snapshot.rs:217-255`
```
        let (source, admin_user_name, snapshot_kms_key_id, ...) = {
            let mut guard = store.lock();
            ...
            // The store lives for the life of the process and is reclaimed only by an
            // explicit delete, so the record count is capped.
            state::ensure_cluster_capacity(guard.clusters.len())
                .map_err(RestoreClusterFromSnapshotErrorModeled::ServiceQuotaExceededException)?;
            ...
            name_is_free(&guard, &cluster_name, &input.cluster_name)?;
            inherited
        };
```

The lock is then dropped, the socket is opened (`restore_cluster_from_snapshot.rs:263-271`), and a SECOND guard does the insert — re-judging the name only:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/restore_cluster_from_snapshot.rs:287-336`
```
        let cluster = {
            let mut guard = store.lock();
            // The lock was released while the socket was opened, so the name is judged again:
            // another call may have taken it in between, and exactly one of the two restores
            // may hold it.
            name_is_free(&guard, &cluster_name, &input.cluster_name)?;
            ...
            guard.clusters.insert(cluster_arn.clone(), record);
```

The sibling mint path asks the same guard INSIDE the guard that inserts:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/create_cluster.rs:287-339`
```
        let cluster = {
            let mut guard = store.lock();
            // The store lives for the life of the process and is reclaimed only by an
            // explicit delete, so the record count is capped.
            state::ensure_cluster_capacity(guard.clusters.len())
                .map_err(CreateClusterErrorModeled::ServiceQuotaExceededException)?;
            ...
            guard.clusters.insert(cluster_arn.clone(), record);
```

The guard the two paths share states that it is what bounds the store:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs:1213-1215`
```
/// Refuse a create that would take the cluster count past [`MAX_CLUSTERS`].
pub fn ensure_cluster_capacity(current: usize) -> Result<(), ServiceQuotaExceededException> {
    if current < MAX_CLUSTERS {
```

Test coverage: `tests/all/cluster_lifecycle.rs:1144-1179` (`the_record_count_is_capped_and_a_delete_frees_a_slot`) drives the cap through `POST /cluster` SEQUENTIALLY only; no suite drives the cap through `POST /cluster-snapshot/{snapshotArn}/restore`, and `tests/all/concurrent_cluster_modification.rs` pins single-writer semantics for `UpdateCluster` only (its header: "an elastic cluster takes one modification at a time"), so nothing pins the restore path's check-then-act window.

## Steps to Reproduce

1. Create MAX_CLUSTERS - 1 (99) clusters and one snapshot of one of them.
2. Issue two concurrent POST /cluster-snapshot/{snapshotArn}/restore calls asking for two DIFFERENT cluster names, timed so both enter the handler before either reaches its second `store.lock()`.
3. Both calls read `guard.clusters.len() == 99` in their first guard block (restore_cluster_from_snapshot.rs:246) and pass `ensure_cluster_capacity`.
4. Both open a plane, re-acquire the lock, pass `name_is_free` (different names) and insert.
5. Observe GET /clusters now pages 101 clusters — one past the guard — where CreateCluster under the same race refuses the second with ServiceQuotaExceededException because its check is inside the inserting guard.
