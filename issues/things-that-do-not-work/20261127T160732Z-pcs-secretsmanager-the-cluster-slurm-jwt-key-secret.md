Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete
Issue-Key: pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected|run#19|AssertionError|raise AssertionError(f"the Slurm key is not a base64 string: {exc}") from exc
Issue-Key: pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete|run#37|Error|decoded = base64.b64decode(encoded, validate=True)
Issue-Commit: a069a9a0ca52a06ec1b3b6edb1946a214c0d1090

# pcs__secretsmanager: the cluster Slurm/JWT key secret is minted with a JSON envelope, so GetSecretValue's SecretString is not base64-decodable key material

> _Note: this issue was identified against the code base at commit `a069a9a0ca52a06ec1b3b6edb1946a214c0d1090`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
One net-new root cause (Cause 1 of 1), clustered from both failing pcs__secretsmanager checks. NOTE: open issue 20261127T144455Z-pcs-secretsmanager-a-cluster-s-slurm-key-secret.md describes the same envelope and was filed against commit ae4983b2ee9be0c2b746a2748bc3435fe07ec59f; the replay report does not mark either check as ALREADY FILED or OWNED by it, and the defect is still present in the current checkout, so it is filed here with the failing check ids attached as the replay key. A fixer should treat the two as one work item.

Cause 1 of 1: the value PCS stores in the Secrets Manager secret it mints for a cluster's generated keys is a JSON document wrapping the key, not the bare base64 key. Failing checks: pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected, pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete.

AWS: Smithy (pcs) models SlurmAuthKey{secretArn @required: String, secretVersion @required: String}, documented "The shared Slurm key for authentication, also known as the cluster secret"; the model says nothing about the secret's payload, so the payload shape is recorded/documented behavior only. AWS's documented retrieval procedure (working-with_clusters_secrets_get.html) pipes the value straight through a decoder: `aws secretsmanager get-secret-value --secret-id <authKey.secretArn> --version-id <authKey.secretVersion> --query 'SecretString' --output text | base64 -d > slurm.key`. That works only because the ENTIRE SecretString is the standard-alphabet base64 encoding of the shared key's raw bytes — no JSON object, no member name, no quoting or surrounding whitespace. The same holds for the JWT key reported through slurmConfiguration.jwtAuth.jwtKey. Both reads in these checks are 200 responses; no status or error code is at issue, the divergence is purely the body of GetSecretValue.SecretString.

Emulator: GetSecretValue on the ARN and version the ACTIVE cluster reports answers 200 with a SecretString of the form {"slurm_key":"<base64>"} (and {"jwt_key":"<base64>"} for the JWT key). A strict decode of that value fails: `binascii.Error: Only base64 data is allowed` — the first check surfaces it as `AssertionError: the Slurm key is not a base64 string: Only base64 data is allowed`, the second raises the binascii.Error out of base64.b64decode(encoded, validate=True). Everything else about the secret is already correct in the emulator: the ARN is a secretsmanager secret in the caller's account and the cluster's Region, the reported version resolves and carries VersionStages ['AWSCURRENT'], the name carries the reserved `pcs!` prefix plus the cluster id, and OwningService is `pcs`. Only the stored value's shape diverges. The fixed text of the divergence is the JSON envelope and its member name (`slurm_key` / `jwt_key`); the base64 run inside it is per-run key material and is not part of the requirement.

Why: a client that follows AWS's documented procedure writes the decoded SecretString to the controller's slurm.key. Against the emulator the decode either errors out or, with a lenient decoder, yields garbage bytes derived from the JSON text, so every Slurm/munge daemon provisioned from that key fails authentication — and the breakage surfaces only where the key is used, not at the API call. It also masks the rest of the cluster-secret contract: the first check aborts before it can exercise the unknown-version refusal, and the second before the post-delete ResourceNotFoundException refusal, so neither of those assertions is reached at all.

Fix: in the PCS cluster-secret minting component — the code that builds the value handed to CreateSecret for a cluster's generated keys — the secret value must be the base64 encoding of the generated key bytes and nothing else; drop the JSON wrapping for both the shared Slurm key and the JWT key. No change is needed in Secrets Manager: it stores and returns whatever value it is given.

Traces: the two failing checks named above; file/line citations are under Evidence.

## Evidence
Check pcs__secretsmanager.cluster-auth-key-reads-at-its-reported-version-and-an-unknown-version-is-rejected (mismatch):
  key = base64.b64decode(pinned.get("SecretString", ""), validate=True)
  -> AssertionError: the Slurm key is not a base64 string: Only base64 data is allowed
  raised after GetSecretValue(SecretId=authKey.secretArn, VersionId=authKey.secretVersion) answered 200 with the same ARN and VersionId the cluster reported.

Check pcs__secretsmanager.slurm-auth-key-secret-read-refused-after-cluster-delete (error):
  decoded = base64.b64decode(encoded, validate=True)
  -> binascii.Error: Only base64 data is allowed
  raised after DescribeSecret confirmed Name starts with 'pcs!' and carries the cluster id, OwningService == 'pcs', VersionIdsToStages[authKey.secretVersion] == ['AWSCURRENT'], and GetSecretValue(VersionStage='AWSCURRENT') resolved to authKey.secretVersion.

Expected (AWS): GetSecretValue(SecretId=authKey.secretArn[, VersionId=authKey.secretVersion]).SecretString is the standard-alphabet base64 encoding of the shared key's raw bytes and nothing else, so base64.b64decode(SecretString, validate=True) yields a non-empty byte string.

Actual (emulator): SecretString is a JSON object carrying the base64 key under a member name: {"slurm_key":"<base64>"} for the shared Slurm key, {"jwt_key":"<base64>"} for the JWT key.

Source of the envelope in the current checkout (unchanged from the earlier filing):
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:448-451 — fn key_document(document_member, key) -> serde_json::json!({ document_member: key }).to_string()
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:399 — .secret_string(key_document(document_member, &key)) on the CreateSecret call
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:426-446 — generated_key() already returns the base64 string that should be the whole value
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/state/cluster_secrets.rs:79,82 — the document members slurm_key / jwt_key
  services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_cluster.rs:452-453 — CreateCluster mints the owed keys via cluster_secrets::mint

Smithy cited: pcs SlurmAuthKey{secretArn @required, secretVersion @required}; doc "The shared Slurm key for authentication, also known as the cluster secret" (payload shape is documented/recorded behavior only).

## Steps to Reproduce
1. ec2: CreateVpc, CreateSubnet in an available availability zone, CreateSecurityGroup with a self-referencing ingress rule.
2. pcs CreateCluster(clusterName=<name>, scheduler={type: SLURM, version: 25.05}, size=SMALL, networking={subnetIds: [<subnet>], securityGroupIds: [<sg>]}).
3. Poll pcs GetCluster until status == ACTIVE; read slurmConfiguration.authKey.secretArn and .secretVersion.
4. secretsmanager GetSecretValue(SecretId=<secretArn>, VersionId=<secretVersion>).
5. Decode the returned SecretString strictly: python -c 'import base64,sys; base64.b64decode(sys.stdin.read().strip(), validate=True)' (or AWS's documented `--query SecretString --output text | base64 -d`).
   Observed: the value is {"slurm_key":"..."} and the decode fails with "Only base64 data is allowed".
   Expected: the value decodes to the cluster's non-empty shared Slurm key bytes.
6. The same applies to slurmConfiguration.jwtAuth.jwtKey on a cluster with the Slurm REST API enabled (member jwt_key).
