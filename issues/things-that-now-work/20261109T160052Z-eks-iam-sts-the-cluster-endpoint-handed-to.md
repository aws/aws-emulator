Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
Issue-Key: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token|run#13|AssertionError|assert not (p := ctx.published_url_mismatch(cluster["endpoint"])), p
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# eks__iam__sts: the cluster endpoint handed to an STS-bearer-token Kubernetes client is a real eks.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: Cluster.endpoint is announced as an AWS-owned Kubernetes API hostname, so the whole STS-bearer-token Kubernetes API flow (SelfSubjectReview, namespace create/replay, the unauthenticated 401 control) can never be addressed.
AWS: Smithy: Cluster.endpoint — "The endpoint for your Kubernetes API server." On AWS the announced host serves the cluster's Kubernetes API, and a client presenting an STS-derived bearer token reaches it there. Recorded behavior: the check sends its requests to exactly the endpoint the API published and gets Kubernetes responses (201 on namespace create, 409 on replay, 401 unauthenticated).
Emulator: DescribeCluster publishes https://<32-hex-id>.<3-hex>.<region>.eks.amazonaws.com. The harness rejects the announced address: "the emulator published https://<ID>.<xxx>.us-west-2.eks.amazonaws.com: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". This is the emulator's own egress refusal — it announced an address other than the one it is reached on.
Why: a client that trusts the API's own endpoint value is pointed at AWS production (or at nothing), so no token-authenticated Kubernetes call can be made against the emulated cluster and the authorization behavior behind the endpoint is unobservable.
Fix: the eks DescribeCluster/CreateCluster response projection for Cluster.endpoint (cluster serializer/state) must publish an address on a listener the emulator serves, and that listener must route the cluster's Kubernetes API paths.
Traces: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token. The same root cause under other service headings is already filed (20261109T141609Z-ecr-eks-logs-a-cluster-s-published-api.md, 20261109T131727Z-eks-auth-the-pod-identity-data-plane-is.md) and a separate eks-heading issue is filed in this same batch; none of them carries this check. Open issue 20261104T043843Z-eks-iam-sts-eks-createcluster-with-bootstrapclustercreatoradminpermissions-... is a different cause (missing creator access entry).

## Evidence
Check eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token:
AssertionError: the emulator published https://D4D87A31584445844C37874123E163A2.d4d.us-west-2.eks.amazonaws.com: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:45903/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
Expected: cluster['endpoint'] is a URL on one of the emulator's own hosts that serves the Kubernetes API (so GET /version, POST /api/v1/namespaces etc. can be made with the STS bearer token).
Actual: an https://<id>.<region>.eks.amazonaws.com URL. The hex id, region and the harness port are per-run values; the defect is that the host is not one the emulator serves.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Replay eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token: create an EKS cluster, wait for ACTIVE, then read cluster['endpoint'] from DescribeCluster and check it against the emulator's own hosts (ctx.published_url_mismatch) before issuing the SelfSubjectReview / namespace requests with an STS bearer token.
