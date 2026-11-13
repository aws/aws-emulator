Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.cluster-kubernetes-api-server-serves-advertised-version eks.deploy-cluster-active-and-api-reachable eks.deploy-eks-cluster-cloudformation-active eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator eks.eks-delete-cluster-with-a-live-node-is-rejected eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod eks.k8s-configmap-mounted-value-readable-inside-the-pod eks.k8s-deployment-apply-runs-pods-on-real-nodes eks.k8s-deployment-scale-converges-to-the-requested-replicas eks.k8s-rbac-get-only-role-rejects-deleting-a-pod eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit eks.k8s-rolling-update-replaces-pods-without-dropping-the-service eks.k8s-service-clusterip-load-balances-across-pods eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports eks.eks-deployment-pod-is-reachable-through-a-cluster-service eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster eks.access-entry-takes-precedence-over-the-aws-auth-configmap eks.namespace-scoped-access-policy-confines-reads-to-its-namespace eks.eks-kubectl-namespace eks.irsa-cross-namespace-serviceaccount-token-rejected eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds eks.eks-view-access-policy-write-is-forbidden-by-the-api-server eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.cluster-kubernetes-api-server-serves-advertised-version|_assert_refused#20|AssertionError|assert status in (401, 403), (
Issue-Key: eks.deploy-cluster-active-and-api-reachable|run#17|AssertionError|assert anon_status == 401, (
Issue-Key: eks.deploy-eks-cluster-cloudformation-active|run#15|AssertionError|assert status == 401, f"unauthenticated GET /version returned HTTP {status}, expected 401"
Issue-Key: eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator|run#15|AssertionError|assert version["gitVersion"].startswith(f"v{_K8S_VERSION}."), (
Issue-Key: eks.eks-delete-cluster-with-a-live-node-is-rejected|_k8s_json#5|AssertionError|assert status in expect, (
Issue-Key: eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod|_k8s_json#9|AssertionError|assert status in expect, (
Issue-Key: eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni|_k8s_json#21|AssertionError|assert status in expect, (
Issue-Key: eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod|_k8s_json#12|AssertionError|assert status in expect, (
Issue-Key: eks.k8s-configmap-mounted-value-readable-inside-the-pod|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-deployment-apply-runs-pods-on-real-nodes|_sh#4|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-deployment-scale-converges-to-the-requested-replicas|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-rbac-get-only-role-rejects-deleting-a-pod|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-rolling-update-replaces-pods-without-dropping-the-service|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-service-clusterip-load-balances-across-pods|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports|_k8s_json#8|AssertionError|assert status in expect, (
Issue-Key: eks.eks-deployment-pod-is-reachable-through-a-cluster-service|_k8s_json#9|AssertionError|assert status in expect, (
Issue-Key: eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule|_k8s_json#20|AssertionError|assert status in expect, (
Issue-Key: eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster|_k8s_json#11|AssertionError|assert status in expect, (
Issue-Key: eks.access-entry-takes-precedence-over-the-aws-auth-configmap|_k8s_expect#13|AssertionError|assert status in expect, (
Issue-Key: eks.namespace-scoped-access-policy-confines-reads-to-its-namespace|_k8s_expect#24|AssertionError|assert status in expect, (
Issue-Key: eks.eks-kubectl-namespace|_wait_api_reachable#11|AssertionError|raise AssertionError(
Issue-Key: eks.irsa-cross-namespace-serviceaccount-token-rejected|_run#30|AssertionError|raise AssertionError(
Issue-Key: eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered|_run#29|AssertionError|raise AssertionError(
Issue-Key: eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service|_run#22|AssertionError|raise AssertionError(
Issue-Key: eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds|_run#17|AssertionError|raise AssertionError(
Issue-Key: eks.eks-view-access-policy-write-is-forbidden-by-the-api-server|run#21|AssertionError|assert write_status == 403, (
Issue-Key: eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied|_create_pod_once_identity_is_injected#25|KeyError|injected = {e["name"] for e in admitted["spec"]["containers"][0].get("env") or []}
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# eks: 10 net-new root causes in the served Kubernetes API — anonymous access is not refused, /version is not a k8s version object, nodes/namespaces/deployments/daemonsets/roles/SelfSubjectAccessReview are 404, kubectl create bodies are rejected, access policies are not enforced, Pod objects omit spec.containers

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
The Kubernetes API server the emulator serves for an EKS cluster diverges from a real cluster's API server in ten independent ways. Each cause below is a separate defect and must be closed separately; the failing check ids for each are listed with it.

Cause 1 of 10: an anonymous request to the cluster's Kubernetes API server is answered 200 instead of being refused.
AWS: recorded behavior only (the cluster's Kubernetes API server is not part of the EKS Smithy model; Smithy only models DescribeCluster -> Cluster.endpoint/certificateAuthority). A real EKS API server authenticates every request: a request carrying no Authorization header is answered HTTP 401 with a Kubernetes Status object whose kind is "Status", apiVersion "v1", status "Failure", reason "Unauthorized", message "Unauthorized" and code 401. 403 Forbidden is acceptable only where the request authenticates and RBAC then denies it; an unauthenticated caller never gets a 200.
Emulator: anonymous GET /version on the published cluster endpoint returns HTTP 200 and a body of the form {"major":"1","minor":"<minor>","gitVersion":"v<major>.<minor>","platform":"emulator"} — no authentication is required at all.
Why: a client relying on AWS's behavior treats a 200 from /version as proof that its IAM-derived bearer token was accepted, and treats a 401 as the signal to refresh the token. An always-open endpoint makes every authentication test pass vacuously, so IAM/access-entry authentication cannot be exercised and credential-expiry handling is never driven.
Fix: the authentication filter in front of the served cluster Kubernetes API listener (the EKS cluster data-plane handler), which must reject requests with no (or an unverifiable) bearer token before routing.
Traces: eks.cluster-kubernetes-api-server-serves-advertised-version (anonymous GET /version returned 200, expected one of 401/403), eks.deploy-cluster-active-and-api-reachable, eks.deploy-eks-cluster-cloudformation-active.

Cause 2 of 10: GET /version does not return a Kubernetes version object.
AWS: recorded behavior only. A real EKS API server answers /version with the Kubernetes version payload: gitVersion is a patch-level EKS build string of the form "v<major>.<minor>.<patch>-eks-<build>" (its fixed parts are the leading "v", the dots and the "-eks-" marker; the major/minor come from the cluster's version, the patch and build id come from the control plane), and the object also carries major, minor, gitCommit, gitTreeState, buildDate, goVersion, compiler and platform, where platform is an OS/arch pair such as "linux/amd64".
Emulator: the body is {"major":"1","minor":"36","gitVersion":"v1.36","platform":"emulator"} — gitVersion carries no patch component and no -eks- build suffix, platform is the literal "emulator", and gitCommit/gitTreeState/buildDate/goVersion/compiler are absent.
Why: any client that parses gitVersion to decide feature availability (kubectl's version skew check, Helm, controller-runtime) fails to parse a two-component version or mis-reads the cluster as a different release than DescribeCluster advertises.
Fix: the /version handler of the served cluster Kubernetes API (EKS cluster data-plane handler), which must derive a patch-level version from the cluster's advertised version.
Traces: eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator (gitVersion 'v1.33' is not a 1.33 patch release).

Cause 3 of 10: the core/v1 nodes collection is not served.
AWS: recorded behavior only. A real cluster serves GET /api/v1/nodes (with labelSelector support, e.g. eks.amazonaws.com/nodegroup=<nodegroup name>) returning a NodeList, and advertises "nodes" in the /api/v1 discovery document so `kubectl get nodes` resolves the resource type.
Emulator: GET /api/v1/nodes (with or without a labelSelector) is answered HTTP 404 with {"kind":"Status",...,"message":"the server could not find the requested resource: /api/v1/nodes","reason":"NotFound","code":404}, and discovery omits the type, so kubectl fails with `error: the server doesn't have a resource type "nodes"`.
Why: every client that schedules, drains, inspects capacity or verifies that a managed node group's nodes registered is broken; a 404 on a core collection is indistinguishable from "wrong API path" to a client.
Fix: the core/v1 resource registry and node collection handler of the served Kubernetes API, plus the /api/v1 discovery document it builds.
Traces: eks.eks-delete-cluster-with-a-live-node-is-rejected, eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod, eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni, eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod, eks.k8s-configmap-mounted-value-readable-inside-the-pod, eks.k8s-deployment-apply-runs-pods-on-real-nodes, eks.k8s-deployment-scale-converges-to-the-requested-replicas, eks.k8s-rbac-get-only-role-rejects-deleting-a-pod, eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit, eks.k8s-rolling-update-replaces-pods-without-dropping-the-service, eks.k8s-service-clusterip-load-balances-across-pods.

Cause 4 of 10: the apps/v1 group is not served (deployments and daemonsets).
AWS: recorded behavior only. A real cluster serves POST/GET /apis/apps/v1/namespaces/<namespace>/deployments and /apis/apps/v1/namespaces/<namespace>/daemonsets/<name> (201 on create, 200 on read), and the EKS-managed add-ons appear there (kube-system/daemonsets/kube-proxy, kube-system/deployments/ebs-csi-controller when the add-on is installed).
Emulator: those paths are answered HTTP 404 with reason "NotFound" and message "the server could not find the requested resource: <path>" — the apps/v1 group is absent, so no Deployment or DaemonSet can be created or read and installed add-ons have no in-cluster workload.
Why: a client cannot deploy a workload at all, and a client that verifies an add-on is running by reading its Deployment/DaemonSet concludes the add-on was never installed even though DescribeAddon reports it ACTIVE.
Fix: the apps/v1 group handlers and discovery of the served Kubernetes API, and the add-on installer that should materialise the managed workloads it reports.
Traces: eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports, eks.eks-deployment-pod-is-reachable-through-a-cluster-service, eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule, eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster.

Cause 5 of 10: the rbac.authorization.k8s.io/v1 group is not served.
AWS: recorded behavior only. A real cluster serves POST /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles (and rolebindings, clusterroles, clusterrolebindings), answering 201 Created with the stored object.
Emulator: POST /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles is answered HTTP 404 with reason "NotFound" and message "the server could not find the requested resource: /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles".
Why: RBAC cannot be configured, so no authorization behavior on the cluster can be set up or exercised; a client's bootstrap of its own least-privilege roles fails with a 404 that looks like a wrong API version.
Fix: the rbac.authorization.k8s.io/v1 group handlers and discovery of the served Kubernetes API.
Traces: eks.access-entry-takes-precedence-over-the-aws-auth-configmap.

Cause 6 of 10: the authorization.k8s.io/v1 SelfSubjectAccessReview endpoint is not served.
AWS: recorded behavior only. A real cluster serves POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews, answering 201 with the review object whose status.allowed states whether the calling identity may perform the requested verb on the requested resource/namespace.
Emulator: that path is answered HTTP 404 with reason "NotFound" and message "the server could not find the requested resource: /apis/authorization.k8s.io/v1/selfsubjectaccessreviews".
Why: `kubectl auth can-i` and every client that pre-flights its permissions get a 404 instead of an allow/deny decision, so namespace-scoped access-policy confinement cannot be observed.
Fix: the authorization.k8s.io/v1 subject-access-review handler of the served Kubernetes API, wired to the same authorizer that decides request admission.
Traces: eks.namespace-scoped-access-policy-confines-reads-to-its-namespace.

Cause 7 of 10: core/v1 namespaces are missing from API discovery.
AWS: recorded behavior only. A real cluster advertises "namespaces" in the /api/v1 discovery document, so `kubectl get namespaces` resolves and lists them.
Emulator: kubectl fails with `error: the server doesn't have a resource type "namespaces"` — the resource is not advertised by discovery.
Why: kubectl and every discovery-driven client cannot address namespaces at all, which blocks the first step of essentially any cluster interaction.
Fix: the discovery document builder and the core/v1 namespace collection handler of the served Kubernetes API.
Traces: eks.eks-kubectl-namespace.

Cause 8 of 10: a namespace-create request body sent by kubectl is rejected as not naming a Kubernetes resource.
AWS: recorded behavior only. A real cluster accepts `kubectl create namespace <name>` — a POST to /api/v1/namespaces whose JSON body is {"apiVersion":"v1","kind":"Namespace","metadata":{"name":"<name>",...}} — with 201 Created and the stored Namespace object.
Emulator: the request is refused with the Status message "The request is invalid: the request body is not a JSON object naming a Kubernetes resource" (kubectl exits rc=1), i.e. the body parser does not recognise the apiVersion/kind pair kubectl sends.
Why: no client that uses kubectl, client-go or `kubectl apply` can create a namespace, so every workflow that starts by creating its own namespace is unusable; the message also misreports a well-formed body as invalid.
Fix: the request-body deserializer/admission path of the served Kubernetes API create handler (apiVersion+kind resolution).
Traces: eks.irsa-cross-namespace-serviceaccount-token-rejected, eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered, eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service, eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds.

Cause 9 of 10: EKS access-policy authorization is not enforced on write requests.
AWS: recorded behavior only for the API-server side; AWS documents AmazonEKSViewPolicy as granting read-only access. A principal whose only association is AmazonEKSViewPolicy is refused a write: POST /api/v1/namespaces is answered HTTP 403 with a Status object whose reason is "Forbidden" and whose message has the fixed shape 'namespaces is forbidden: User "<username>" cannot create resource "namespaces" in API group "" at the cluster scope' (the username comes from the access entry, the resource/verb/group from the request).
Emulator: the same principal's create succeeds — HTTP 201 with the created Namespace object — so the associated access policy is ignored when authorizing requests.
Why: a client relying on AWS's behavior believes a view-only principal cannot mutate the cluster; against the emulator a read-only identity silently has write power, so least-privilege regressions pass.
Fix: the authorizer of the served Kubernetes API, which must map the authenticated principal to its associated EKS access policies (and their access scope) before admitting a write.
Traces: eks.eks-view-access-policy-write-is-forbidden-by-the-api-server.

Cause 10 of 10: a Pod object returned by the API server omits spec.containers.
AWS: recorded behavior only. A real cluster echoes back the full PodSpec on create and read: spec.containers is a required, non-empty list, each entry carrying at least name and image plus any env the admission webhooks injected.
Emulator: the returned Pod object has no spec.containers member at all, so a client indexing it raises (the check fails with KeyError: 'containers' on admitted["spec"]["containers"]).
Why: every client that reads back a pod — to confirm its image, to read injected environment (e.g. the pod-identity/IRSA credential variables), or to build a typed object — crashes or sees an empty workload, because the Kubernetes API contract guarantees spec.containers is present.
Fix: the Pod serializer/state of the served Kubernetes API (it must persist and echo the submitted PodSpec, including admission-injected env).
Traces: eks.eks-pod-identity-credentials-are-scoped-and-an-unallowed-bucket-is-denied.

Related open issues describe why the endpoint is unreachable in other runs (e.g. 20261109T160052Z-eks-3-net-new-root-causes-cluster-endpoint.md, 20261104T053901Z-eks-auth-the-whole-pod-identity-data-plane.md); here the endpoint WAS reached and answered, so none of them covers these ten causes.

## Evidence
Cause 1: anonymous GET /version -> HTTP 200 b'{"major":"1","minor":"36","gitVersion":"v1.36","platform":"emulator"}'; expected 401 (403 acceptable). Cause 2: gitVersion 'v1.33' is not a 1.33 patch release (expected v1.33.<patch>-eks-<build>). Cause 3: GET /api/v1/nodes -> 404 {"kind":"Status","message":"the server could not find the requested resource: /api/v1/nodes","reason":"NotFound","code":404}; kubectl: error: the server doesn't have a resource type "nodes". Cause 4: POST /apis/apps/v1/namespaces/<ns>/deployments -> 404 NotFound; GET /apis/apps/v1/namespaces/kube-system/daemonsets/kube-proxy -> 404; GET /apis/apps/v1/namespaces/kube-system/deployments/ebs-csi-controller -> 404. Cause 5: POST /apis/rbac.authorization.k8s.io/v1/namespaces/<ns>/roles -> 404 NotFound. Cause 6: POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews -> 404 NotFound. Cause 7: kubectl get namespaces -> rc=1 'the server doesn't have a resource type "namespaces"'. Cause 8: kubectl create namespace <name> -> rc=1 'The request is invalid: the request body is not a JSON object naming a Kubernetes resource'. Cause 9: principal holding only AmazonEKSViewPolicy POST /api/v1/namespaces -> 201 with the created Namespace; expected 403 Forbidden. Cause 10: created/read Pod object has no spec.containers -> KeyError: 'containers' (and kubernetes client ValueError: Invalid value for `containers`, must not be `None`).

## Steps to Reproduce
1. CreateCluster and wait for ACTIVE; take cluster.endpoint and certificateAuthority.data from DescribeCluster. 2. GET <endpoint>/version with no Authorization header -> observe 200 instead of 401, and the non-patch gitVersion/'emulator' platform (causes 1-2). 3. With an authenticated client: GET /api/v1/nodes; POST /apis/apps/v1/namespaces/<ns>/deployments; GET /apis/apps/v1/namespaces/kube-system/daemonsets/kube-proxy; POST /apis/rbac.authorization.k8s.io/v1/namespaces/<ns>/roles; POST /apis/authorization.k8s.io/v1/selfsubjectaccessreviews -> each 404 NotFound (causes 3-6). 4. Run `kubectl get namespaces` and `kubectl create namespace <name>` against the endpoint -> unknown resource type, then the invalid-body refusal (causes 7-8). 5. Create an access entry associated only with AmazonEKSViewPolicy, authenticate as that principal and POST /api/v1/namespaces -> 201 instead of 403 (cause 9). 6. Create a pod and read it back -> spec.containers absent (cause 10).
