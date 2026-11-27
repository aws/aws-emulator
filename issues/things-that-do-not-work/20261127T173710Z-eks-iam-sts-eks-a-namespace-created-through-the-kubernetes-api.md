Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
Issue-Key: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token|run#52|KeyError|assert created["metadata"]["labels"]["kubernetes.io/metadata.name"] == namespace, created
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# eks: a Namespace created through the Kubernetes API carries no kubernetes.io/metadata.name label

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
=== Cause 1 of 1: the cluster's Kubernetes API does not add the NamespaceDefaultLabelName label to a created Namespace ===
AWS: recorded behavior only (no Smithy member carries this — it is Kubernetes object state served by the EKS cluster endpoint EKS publishes as `Cluster.Endpoint`, reached with an `sts`-derived bearer token). Every Kubernetes cluster at the versions EKS serves runs the `NamespaceDefaultLabelName` admission plugin, so the object a `POST /api/v1/namespaces` returns always carries `metadata.labels["kubernetes.io/metadata.name"]` whose value is the namespace's own `metadata.name`. The label is added by the API server, not by the client, and the recording shows it present in the create response.
Emulator: the created object's `metadata.labels` has no `kubernetes.io/metadata.name` key at all, so reading it raises `KeyError: 'kubernetes.io/metadata.name'`.
Why: this label is the standard way a client selects a namespace by name — `NetworkPolicy.namespaceSelector`, `PodAffinity` namespace selectors, admission policies and Helm charts all match on `kubernetes.io/metadata.name`. Against the emulator every such selector matches nothing, so policies that should isolate or target a namespace silently apply to nobody.
Fix: the EKS cluster's Kubernetes API object handler for Namespace creation — add the `kubernetes.io/metadata.name` label (value = the namespace name) to the stored and returned object, as the admission plugin does.
Traces: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token.

## Evidence
From one replay of the behavioral suite (service heading `### eks__iam__sts`, 1 of 2 checks failed; the other is owned by an already-filed issue).

[error] eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
  File "check_eks__iam__sts_eks_kubernetes_api_namespace_via_sts_bearer_token.py", line 217, in run
    assert created["metadata"]["labels"]["kubernetes.io/metadata.name"] == namespace, created
  KeyError: 'kubernetes.io/metadata.name'

Expected: the create response's `metadata.labels` contains `kubernetes.io/metadata.name` equal to the namespace's `metadata.name` (the namespace name itself is generated per run; the requirement is label-present and label-value == name).
Observed: the key is absent from `metadata.labels`.

## Steps to Reproduce
1. CreateCluster in EKS and wait for ACTIVE; read `Cluster.Endpoint` and `Cluster.CertificateAuthority.Data`.
2. Mint an EKS bearer token from STS (the `k8s-aws-v1.` presigned GetCallerIdentity form) for a principal mapped to cluster-admin.
3. `POST <endpoint>/api/v1/namespaces` with body `{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"<ns>"}}` and that bearer token.
4. In the response body, read `metadata.labels["kubernetes.io/metadata.name"]` — it must equal `<ns>`. Observed: the key does not exist. The same holds for a subsequent `GET /api/v1/namespaces/<ns>`.
