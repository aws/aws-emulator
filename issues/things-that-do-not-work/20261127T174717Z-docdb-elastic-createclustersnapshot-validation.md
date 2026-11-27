Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aff844956fc3a746e6f61af85746cbd7333fe795

# CreateClusterSnapshot is the only cluster-acting operation with no settle and no cluster-status gate, so a STOPPED cluster can be snapshotted

_Filed: 2026-11-27T17:47:17Z_
_Operation: CreateClusterSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `aff844956fc3a746e6f61af85746cbd7333fe795`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other operation in this crate that ACTS on a stored cluster first materialises the transition the clock owes the record (`ClusterRecord::settle_unless_in_flight`) and then asks a status gate before writing anything: `UpdateCluster` -> `ensure_modifiable`, `StopCluster` -> `ensure_available`, `StartCluster` -> `ensure_startable`, `DeleteCluster` -> `ensure_deletable`, and even `ApplyPendingMaintenanceAction` settles. `CreateClusterSnapshot` reads the cluster with a plain `guard.clusters.get(..)`, settles nothing and gates on nothing, so it mints a snapshot of a cluster in ANY status.

Concretely: a cluster that has been stopped cannot be updated, started-over, or deleted (`DeleteCluster` answers `Can't delete the cluster because it is in STOPPED state.`), yet `CreateClusterSnapshot` of that same stopped cluster answers 200 and persists a snapshot. A snapshot taken while an `UpdateCluster` change is still in flight is likewise accepted, and because the handler never settles, the cluster record is left in its transient status by a call that acted on it — the one thing `settle_unless_in_flight` exists to prevent for an acting call.

## Evidence

The handler reads the cluster and goes straight to minting — src/ops/create_cluster_snapshot.rs:172-190:
```
        let mut guard = store.lock();
        // The cluster being snapshotted is resolved FIRST, so a request naming a
        // cluster that does not exist is a 404 rather than a quota or name conflict.
        let Some(cluster) = guard.clusters.get(&input.cluster_arn) else {
            return Err(CreateClusterSnapshotErrorModeled::ResourceNotFoundException(
```
(no `settle_unless_in_flight`, no `ensure_*` anywhere in the file).

The siblings all do both. src/ops/stop_cluster.rs:165-170:
```
        record.settle_unless_in_flight();
        state::ensure_available(record, "stopped")
            .map_err(StopClusterErrorModeled::ValidationException)?;
```
src/ops/update_cluster.rs:239-245:
```
        record.settle_unless_in_flight();
        state::ensure_modifiable(record).map_err(UpdateClusterErrorModeled::ConflictException)?;
```
src/ops/delete_cluster.rs:172-176:
```
        record.settle_unless_in_flight();
        state::ensure_deletable(record).map_err(DeleteClusterErrorModeled::ValidationException)?;
```
src/ops/apply_pending_maintenance_action.rs:279: `record.settle_unless_in_flight();`

The state module states that an ACTING call is the one that must settle — src/state.rs:279-286:
```
    /// Materialise the transition for a call that ACTS on this cluster, once the change
    /// it carries is no longer in flight.
    ...
    pub fn settle_unless_in_flight(&mut self) {
```

This crate's own test states the rule the handler does not implement — tests/all/pending_maintenance_reads.rs:343-345:
```
            // A snapshot can only be taken of a settled cluster, and a restore reads
            // the snapshot's record of it.
            settled_cluster(router, &source_arn).await;
```

Test coverage: no suite snapshots a cluster that is NOT owed `ACTIVE` — tests/all/cluster_lifecycle.rs:1647-1653 and 1721-1727 settle the cluster first, and tests/all/cluster_snapshot_listing.rs:215-223 snapshots clusters whose mint transition is already due (`mint_in_flight_until()` is `Instant::now()`, src/state.rs:554-556), so adding the settle+gate leaves every existing test green.

## Steps to Reproduce

1. POST /cluster (valid minimal body) -> note `clusterArn`.
2. GET /cluster/{clusterArn} once so the create settles to ACTIVE.
3. POST /cluster/{clusterArn}/stop -> 200, status STOPPING; GET /cluster/{clusterArn} -> STOPPED.
4. DELETE /cluster/{clusterArn} -> 400 `Can't delete the cluster because it is in STOPPED state.`; PUT /cluster/{clusterArn} -> 409 `Can't perform operation on cluster as it's in use` / state refusal — the cluster is gated for every acting operation.
5. POST /cluster-snapshot with {"clusterArn": <arn>, "snapshotName": "snap-of-stopped"} -> 200: a snapshot of the stopped cluster is minted, and the cluster record is left un-settled by the call that acted on it.
