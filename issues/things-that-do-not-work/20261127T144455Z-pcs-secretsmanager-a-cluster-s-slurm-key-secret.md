Issue-Severity: HIGH
Issue-Commit: ae4983b2ee9be0c2b746a2748bc3435fe07ec59f

# pcs__secretsmanager: a cluster's Slurm key secret holds a JSON document instead of the bare base64 key, so GetSecretValue's SecretString does not base64-decode

> _Note: this issue was identified against the code base at commit `ae4983b2ee9be0c2b746a2748bc3435fe07ec59f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
One net-new root cause (1 of 1), grounded in both failing pcs__secretsmanager checks.

Cause 1 of 1: the SecretString of the Secrets Manager secret PCS mints for a cluster's shared Slurm key (the value reported by GetCluster's slurmConfiguration.authKey.secretArn/secretVersion) is a JSON object wrapping the key, not the base64-encoded key itself. Failing checks: pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected, pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete.

AWS: the PCS Smithy model declares SlurmAuthKey{secretArn @required, secretVersion @required} — "The shared Slurm key for authentication, also known as the cluster secret" — and says nothing about the secret's payload, so the payload requirement is recorded/documented behavior only. AWS's own procedure for retrieving the cluster secret (https://docs.aws.amazon.com/pcs/latest/userguide/working-with_clusters_secrets_get.html) reads the key by piping the value straight through a base64 decoder: `aws secretsmanager get-secret-value --secret-id <authKey.secretArn> --version-id <authKey.secretVersion> --query 'SecretString' --output text | base64 -d > slurm.key`. That only works because the ENTIRE SecretString is the standard-alphabet base64 encoding of the shared key's raw bytes — no JSON envelope, no member name, no surrounding whitespace or quoting. The same holds for the JWT-key secret reported through slurmConfiguration.jwtAuth.jwtKey. Both reads here are 200 responses; nothing is refused, so no status or error code is at issue — the divergence is purely in the body of GetSecretValue.SecretString.

Emulator: the secret is created with a JSON document as its value, of the form {"slurm_key":"<base64>"} for the shared key (and {"jwt_key":"<base64>"} for the JWT key). GetSecretValue on the ARN and version the ACTIVE cluster reports therefore returns a SecretString that is not base64 at all, and a strict decode of it fails: `binascii.Error: Only base64 data is allowed` (the first check turns that into `AssertionError: the Slurm key is not a base64 string: Only base64 data is allowed`; the second raises the binascii.Error out of base64.b64decode(encoded, validate=True)). Everything around the payload is already right in the emulator — the ARN is a Secrets Manager secret in the caller's account and the cluster's Region, the reported version resolves and carries stages ["AWSCURRENT"], the name carries the reserved `pcs!` prefix and the cluster id, and OwningService is `pcs` — only the value's shape diverges.

Why: a client follows AWS's documented procedure and writes the decoded SecretString to the controller's slurm.key. Against the emulator it either errors out of the decode or, with a lenient decoder, writes garbage bytes derived from the JSON text, so every Slurm daemon provisioned against that key fails authentication. Any tooling that treats the cluster secret as opaque base64 key material (login-node bootstrap scripts, config management reading the cluster secret into munge/slurm key files) breaks on the emulator while working on AWS, and the divergence only surfaces at the point the key is used, not at the API call. It also masks the rest of the cluster-secret contract: the first failure aborts before the unknown-version refusal and the post-delete refusal assertions can be exercised at all.

Fix: in the PCS cluster-secret minting component — the code that builds the value passed to CreateSecret for a cluster's generated keys — the secret value must be the base64 encoding of the generated key bytes and nothing else; the JSON wrapping of the key under a document member should be dropped for both the shared Slurm key and the JWT key, so the value a cluster's reported secret holds is directly decodable. No change is needed in Secrets Manager itself: it stores and returns whatever value it is given.

Traces: the two failing checks named above; file/line citations for the minting path are in the Evidence section.

## Evidence
Check pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected:
  key = base64.b64decode(pinned.get("SecretString", ""), validate=True)
  -> AssertionError: the Slurm key is not a base64 string: Only base64 data is allowed
  (raised after GetSecretValue(SecretId=authKey.secretArn, VersionId=authKey.secretVersion) answered 200 with the ARN and version the cluster reported)

Check pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete:
  decoded = base64.b64decode(encoded, validate=True)
  -> binascii.Error: Only base64 data is allowed
  (raised after DescribeSecret confirmed Name begins with 'pcs!' and carries the cluster id, OwningService == 'pcs', VersionIdsToStages[authKey.secretVersion] == ['AWSCURRENT'], and GetSecretValue(VersionStage='AWSCURRENT') resolved to authKey.secretVersion)

Expected (AWS): GetSecretValue(SecretId=authKey.secretArn).SecretString is the standard-alphabet base64 encoding of the shared Slurm key's raw bytes and nothing else, so `base64.b64decode(SecretString, validate=True)` yields a non-empty byte string — exactly what AWS's documented `... --query 'SecretString' --output text | base64 -d > slurm.key` relies on.

Actual (emulator): SecretString is a JSON object carrying the base64 key under a member name — `{"slurm_key":"<base64>"}` for the shared key, `{"jwt_key":"<base64>"}` for the JWT key. Fixed text of the divergence is the JSON envelope and the member name; the base64 run inside it is per-run key material and is not part of the requirement.

Source of the envelope, in the checkout:
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:449-451 — `fn key_document(document_member, key) -> serde_json::json!({ document_member: key }).to_string()`
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:399 — `.secret_string(key_document(document_member, &key))` on the CreateSecret call
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:79,82 — the document members `slurm_key` / `jwt_key`
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:426-446 — `generated_key` already returns the base64 string that should be the whole value
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_cluster.rs:452-453 — CreateCluster mints the cluster's owed keys through `cluster_secrets::mint`

## Steps to Reproduce
1. Create an EC2 VPC, one subnet in an available zone, and a security group with a self-referencing ingress rule.
2. pcs CreateCluster(clusterName=<name>, scheduler={type: SLURM, version: 25.05}, size=SMALL, networking={subnetIds: [<subnet>], securityGroupIds: [<sg>]}).
3. Poll pcs GetCluster until status == ACTIVE and read slurmConfiguration.authKey.secretArn and .secretVersion.
4. secretsmanager GetSecretValue(SecretId=<secretArn>, VersionId=<secretVersion>).
5. Decode the returned SecretString strictly, e.g. python -c 'import base64,sys; base64.b64decode(sys.stdin.read().strip(), validate=True)', or AWS's documented `--query SecretString --output text | base64 -d`.
   Observed: the value is `{"slurm_key":"..."}` and the decode fails with "Only base64 data is allowed".
   Expected: the value decodes to the cluster's non-empty shared Slurm key bytes.
6. The same holds for slurmConfiguration.jwtAuth.jwtKey on a cluster whose Slurm REST API is enabled (document member `jwt_key`).
