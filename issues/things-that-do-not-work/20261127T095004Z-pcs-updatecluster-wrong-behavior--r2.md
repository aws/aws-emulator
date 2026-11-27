Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# UpdateCluster never clears slurmConfiguration.jwtAuth when the update turns the Slurm REST API off, so the cluster reports a JWT key for a daemon it no longer runs

_Filed: 2026-11-27T09:50:04Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An `UpdateCluster` that sets `slurmConfiguration.slurmRest.mode` to `NONE` on a cluster whose REST API was on leaves the stored `slurmConfiguration.jwtAuth.jwtKey` in place. `crate::state::apply_slurm_update` merges the request into the stored configuration and never touches `jwt_auth` (it only ever gets written by `cluster_secrets::record`, and only ever cleared by `cluster_secrets::take`, which `DeleteCluster` calls), so after the update the record carries `slurmRest.mode = NONE` AND a `jwtAuth.jwtKey`.

The next request settles the cluster back to `ACTIVE`, and `ClusterRecord::settle` recomputes `endpoints` from `scheduler_endpoints()`, which drops the `SLURMRESTD` endpoint because `slurm_rest_enabled()` is now false. From that point on `GetCluster`/`CreateCluster`'s own record answers both ways at once: no `SLURMRESTD` endpoint (the daemon is off) and a `jwtAuth.jwtKey` (the key that daemon verifies its JSON web tokens with). That is exactly the disagreement `ClusterRecord::slurm_rest_enabled`'s own doc comment says cannot happen, and it contradicts the rule this crate's own secrets suite states for the reported member.

The mirror direction is implemented: the handler recomputes `cluster_secrets::owed(cluster)` after applying the change and mints the JWT key when an update turns the REST API ON (and the suite covers that case, `an_update_that_enables_the_slurm_rest_api_adds_the_jwt_key`). Only the OFF direction has no counterpart: nothing takes the member back off the record.

## Evidence

CRATE SIDE A — the rule the crate states for this member, in the suite that owns it (tests/all/cluster_lifecycle/slurm_cluster_secrets.rs:14-16):
```
//! * a cluster whose Slurm REST API is ENABLED (`slurmRest.mode` `STANDARD`) also reports
//!   `slurmConfiguration.jwtAuth.jwtKey`, the key that daemon verifies its JSON web tokens
//!   with, in a second such secret; one whose REST API is off reports no `jwtAuth` at all;
```
and the state module's own statement that the two reported facts cannot diverge (src/state.rs:634-646):
```
/// Whether this cluster's Slurm REST API is ON: "A value of `STANDARD` means the Slurm
/// REST API is enabled" (`SlurmRest$mode`, whose documented default is
/// [`MODE_NONE`]).
///
/// The ONE answer to that question, so the endpoint the daemon is published on
/// ([`Self::scheduler_endpoints`]) and the JWT key it verifies tokens with
/// ([`cluster_secrets::owed`]) cannot disagree about whether the cluster runs it.
pub fn slurm_rest_enabled(&self) -> bool {
```
plus `Owed::jwt_key` (src/state/cluster_secrets.rs:96-100): "The JWT key, which a cluster whose Slurm REST API is enabled has ... the REST API is on exactly when the cluster's `slurmRest.mode` is `STANDARD`".

CRATE SIDE B — the update path that breaks it. The merge never touches `jwt_auth` (src/state.rs:1845-1897, body shown around `slurm_rest`):
```
pub fn apply_slurm_update(
    stored: Option<ClusterSlurmConfiguration>,
    update: UpdateClusterSlurmConfigurationRequest,
) -> ClusterSlurmConfiguration {
    let mut merged = stored.unwrap_or_default();
...
    if let Some(slurm_rest) = update.slurm_rest {
        let previous = merged.slurm_rest.take();
        merged.slurm_rest = Some(SlurmRest {
            // "The default value for mode is NONE" (`SlurmRestRequest$mode`).
            mode: slurm_rest
                .mode
                .or_else(|| previous.map(|previous| previous.mode))
                .unwrap_or_else(|| MODE_NONE.to_string()),
        });
    }
```
and the handler asks only what the cluster is now OWED, never what it must give back (src/ops/update_cluster.rs:350-367):
```
        if let Some(slurm) = input.slurm_configuration {
            let stored = cluster.slurm_configuration.take();
            cluster.slurm_configuration = Some(crate::state::apply_slurm_update(stored, slurm));
            changed = true;
        }
...
        // The Slurm keys this cluster is owed once the change has been applied: a cluster
        // whose Slurm REST API this update turned ON is owed the JWT key that daemon
        // verifies its tokens with, and one already holding a key is owed nothing
        let owed_keys = crate::state::cluster_secrets::owed(cluster);
```

CRATE SIDE C — the endpoint half DOES follow the switch, which is what makes the record self-contradictory (src/state.rs:598-603 and 619-631):
```
        if !removable && self.status == STATUS_ACTIVE {
            self.endpoints = self.scheduler_endpoints();
        }
```
```
            (ENDPOINT_SLURMRESTD, SLURMRESTD_PORT, self.slurm_rest_enabled()),
```
and the only place the member is ever cleared is the DELETE path (src/state/cluster_secrets.rs:223-235):
```
/// The members go with the secrets: a cluster whose secrets have been released reports no key
/// at all, rather than an ARN naming a secret that is gone.
pub fn take(record: &mut ClusterRecord) -> Vec<String> {
```

TEST COVERAGE — the suite asserts `jwtAuth.is_null()` only for CREATE rows whose request named `mode: NONE` (tests/all/cluster_lifecycle/slurm_cluster_secrets.rs:413-419, rows at :335-337), and the only update case it drives is the ON direction (`an_update_that_enables_the_slurm_rest_api_adds_the_jwt_key`, :456). No test turns the REST API OFF with `UpdateCluster`, which is why the gap survives.

## Steps to Reproduce

1. Mount PCS beside Secrets Manager on the emulator so the Slurm key secrets are really created.
2. CreateCluster(clusterName="rest-cluster", size="SMALL", scheduler=..., networking=..., slurmConfiguration={"slurmRest":{"mode":"STANDARD"}}).
3. GetCluster(clusterIdentifier="rest-cluster") — the cluster is ACTIVE, reports slurmConfiguration.jwtAuth.jwtKey, and its endpoints include the SLURMRESTD endpoint.
4. UpdateCluster(clusterIdentifier="rest-cluster", slurmConfiguration={"slurmRest":{"mode":"NONE"}}) — answers 200 with status UPDATING.
5. GetCluster(clusterIdentifier="rest-cluster") again (this request settles the cluster back to ACTIVE).
6. Observe the contradiction in one body: slurmConfiguration.slurmRest.mode == "NONE" and no SLURMRESTD entry in endpoints, yet slurmConfiguration.jwtAuth.jwtKey is still reported.
7. Expected, per the crate's own rule ("one whose REST API is off reports no `jwtAuth` at all", tests/all/cluster_lifecycle/slurm_cluster_secrets.rs:16, and "the endpoint the daemon is published on and the JWT key it verifies tokens with cannot disagree about whether the cluster runs it", src/state.rs:638-640): the jwtAuth member is gone from the record once the update turns the REST API off.
