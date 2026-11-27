Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e02cdb40ab740aea2961dab64f98621a48c50c29

# A cluster's slurmConfiguration never reports authKey or jwtAuth, although the crate's own comment says PCS reports both

_Filed: 2026-11-21T01:27:58Z_
_Operation: GetCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e02cdb40ab740aea2961dab64f98621a48c50c29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every cluster read answers `slurmConfiguration` straight off the stored record, and the stored record's `auth_key` and `jwt_auth` are hard-coded to `None` at the one place a request's `ClusterSlurmConfigurationRequest` is turned into the stored `ClusterSlurmConfiguration`. So `GetCluster` (and the create/update responses built from the same record) never carry `slurmConfiguration.authKey` or `slurmConfiguration.jwtAuth` — the modelled members that hand a client the Secrets Manager secret ARN/version of the Slurm cluster secret and of the JWT key. A client that provisions anything that has to talk to the cluster's Slurm controller (a login node, a Slurm client outside the compute node groups, or any tooling that reads the cluster secret) has no value to read: the member is absent from every response, for every cluster, whatever its Slurm configuration says.

This is not a gap the crate is silent about: the comment sitting on top of the two `None`s states the rule it is breaking ("PCS reports the Secrets Manager secret it generated for the Slurm auth key and for JWT auth") and then admits the emulator reports neither, with the stated reason that it would otherwise have to invent an ARN. That reason is answerable with mechanisms this crate already has: PCS already resolves facts it does not own against other emulated services over the in-process callback (EC2 for subnets, IAM for the instance profile, STS for the caller), and the tree holds a Secrets Manager emulator (`services/service-com-amazonaws-secretsmanager-secretsmanager`), so the secret can be a real secret of this emulator rather than a fabricated ARN.

The write site is `ClusterRecord`'s conversion in `src/state.rs` (reached from CreateCluster/UpdateCluster); the read where the hole is observable is GetCluster, which is filed here.

## Evidence

The crate comment that states the rule and admits the gap — services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state.rs:1685-1689 (inside `impl From<ClusterSlurmConfigurationRequest> for ClusterSlurmConfiguration`):
```
            // PCS reports the Secrets Manager secret it generated for the Slurm auth key
            // and for JWT auth. The emulator generates no secret, so it reports neither
            // rather than inventing an ARN.
            auth_key: None,
            jwt_auth: None,
```
The fields exist on the stored/response type and are therefore silently absent on the wire — src/ops/types/shared_types.rs:81-82:
```
    pub auth_key: Option<SlurmAuthKey>,
    pub jwt_auth: Option<JwtAuth>,
```
GetCluster answers the stored record unchanged — src/ops/get_cluster.rs:181-183:
```
        Ok(GetClusterOutput {
            cluster: Some(cluster.to_api()),
        })
```
and `ClusterRecord::to_api` copies the stored Slurm configuration verbatim — src/state.rs:376: `slurm_configuration: self.slurm_configuration.clone(),` (so the `None`s above are what every reader sees; nothing else ever writes those two fields: `grep -rn "auth_key\|jwt_auth" src/` matches only state.rs:1688-1689 and shared_types.rs:81-82).

The members the model declares for exactly this (smithy_query query="shape", shape="ClusterSlurmConfiguration,SlurmAuthKey,JwtKey"):
- `ClusterSlurmConfiguration$authKey` -> `SlurmAuthKey`: "The shared Slurm key for authentication, also known as the cluster secret."
- `SlurmAuthKey$secretArn` (required): "The Amazon Resource Name (ARN) of the shared Slurm key."; `SlurmAuthKey$secretVersion` (required): "The version of the shared Slurm key."
- `JwtKey$secretArn` (required): "The Amazon Resource Name (ARN) of the Amazon Web Services Secrets Manager secret containing the JWT key."

That the "we would have to invent an ARN" reason is already answerable in this crate — src/state/instance_profile.rs:10-15: "IAM is reached the way any emulated service reaches another, over the in-process callback through the tree's one construction of that hop (`service_common::iam_client`), so this crate names no other service's crate and nothing leaves the process." and src/state/ec2_subnets.rs:13-15 says the same for EC2; a Secrets Manager emulator exists at services/service-com-amazonaws-secretsmanager-secretsmanager/Cargo.toml.

Test coverage: none pins the current absence — `grep -rn "authKey\|auth_key\|jwtAuth\|jwt_auth" tests/` returns no matches, and the cluster suites (tests/all/cluster_lifecycle.rs, tests/all/cluster_update_and_endpoints.rs) assert on `endpoints`, `networking`, `scheduler`, `size` and the Slurm settings lists only.

## Steps to Reproduce

1. CreateCluster(clusterName="secret-cluster", scheduler={type:"SLURM", version:"24.11"}, size="SMALL", networking={subnetIds:[<an existing subnet>]}, slurmConfiguration={"scaleDownIdleTimeInSeconds": 600, "slurmRest": {"mode": "STANDARD"}}) -> 200.
2. Issue one more request so the cluster settles to ACTIVE (the crate materialises owed transitions at the top of the next request).
3. GetCluster(clusterIdentifier="secret-cluster") -> 200.
4. Observed: `cluster.slurmConfiguration` carries `scaleDownIdleTimeInSeconds` and `slurmRest`, but no `authKey` member and no `jwtAuth` member at all — there is no secret ARN or version for the Slurm cluster secret anywhere in the response, and no later call can produce one.
5. Expected per the crate's own comment at src/state.rs:1685-1687 ("PCS reports the Secrets Manager secret it generated for the Slurm auth key and for JWT auth"): the cluster reports `slurmConfiguration.authKey.secretArn`/`.secretVersion` (and `jwtAuth.jwtKey` for a JWT-enabled cluster) naming a secret this emulator actually holds, mintable over the same in-process hop this crate already uses for EC2, IAM and STS.
