Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected
Issue-Key: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery|_create_aws_logging_configmap#22|ApiException|core_v1.create_namespaced_config_map("aws-observability", body)
Issue-Key: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch|_create_aws_logging_configmap#20|ApiException|core_v1.create_namespaced_config_map("aws-observability", body)
Issue-Key: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image|_create_pod#23|ValueError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret|_create_pod#37|ValueError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image|_create_pod#24|ValueError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected|_create_pod#26|ValueError|core_v1.create_namespaced_pod(namespace, body)
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# ecr__eks__logs: 2 net-new root causes in the served Kubernetes API — creating a ConfigMap is answered 405 MethodNotAllowed, and Pod objects come back with no spec.containers

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Two independent defects in the Kubernetes API the emulator serves for an EKS cluster, both reached on a cluster that answered (so neither is an unreachable-endpoint case). Each cause below is a separate defect and must be closed separately.

Cause 1 of 2: POST of a ConfigMap into an existing namespace is refused 405 MethodNotAllowed.
AWS: recorded behavior only (the cluster's Kubernetes API is not in the EKS Smithy model). A real cluster serves POST /api/v1/namespaces/<namespace>/configmaps and answers 201 Created with the stored ConfigMap; the Fargate logging contract is exactly this — a ConfigMap named aws-logging created in the aws-observability namespace. A 405 is returned only for a verb a collection genuinely does not support, and the message's fixed shape is 'the server does not allow <VERB> on <path>'.
Emulator: the create is answered HTTP 405 with {"kind":"Status","apiVersion":"v1","status":"Failure","message":"the server does not allow POST on /api/v1/namespaces/<namespace>/configmaps","reason":"MethodNotAllowed","code":405} — the configmaps collection is registered for reads only, so no ConfigMap can ever be created.
Why: a client relying on AWS's behavior cannot configure anything that is delivered through a ConfigMap (Fargate log routing, aws-auth, application configuration); a 405 tells it the verb is unsupported by the resource, which is a permanent, non-retryable answer, so it gives up rather than retrying or falling back.
Fix: the core/v1 ConfigMap collection handler of the served cluster Kubernetes API — it must accept POST (create) and persist the object, not only GET.
Traces: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery, ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch.

Cause 2 of 2: a Pod object returned by the API server has no spec.containers.
AWS: recorded behavior only. The Kubernetes API contract makes PodSpec.containers a required, non-empty list; a real cluster echoes it back on create and on read, each entry carrying at least name and image (plus imagePullSecrets, env and the service-account projections as submitted or injected).
Emulator: the Pod object returned for a created/read pod omits spec.containers entirely. A typed client therefore fails to construct the object at all — kubernetes.client raises ValueError: Invalid value for `containers`, must not be `None` while deserializing V1PodSpec — and an untyped client sees a pod with no workload.
Why: every client that reads a pod back to learn its image, its pull secrets, its injected credential environment or its status crashes or mis-reports an empty pod; image-pull behavior (private ECR pulls through a node role, a Fargate pod execution role or IRSA) cannot be observed at all because the container list that names the image is missing.
Fix: the Pod serializer/state of the served cluster Kubernetes API (persist and echo the submitted PodSpec, including admission-injected members).
Traces: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image, ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret, ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image, ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected.

Open issues 20261104T060239Z-ecr-eks-logs-a-cluster-s-certificateauthority-data.md, 20261106T070547Z-ecr-eks-logs-a-fargate-profile-cluster-s.md and 20261109T141609Z-ecr-eks-logs-a-cluster-s-published-api.md concern a cluster endpoint/CA that cannot be reached or loaded; in these checks the endpoint WAS reached and answered 405/served a Pod object, so neither cause is covered by them.

## Evidence
Cause 1: kubernetes.client.exceptions.ApiException: (405) Reason: Method Not Allowed, body {"kind":"Status","apiVersion":"v1","metadata":{},"status":"Failure","message":"the server does not allow POST on /api/v1/namespaces/aws-observability/configmaps","reason":"MethodNotAllowed","details":null,"code":405}; expected 201 Created with the stored ConfigMap. Cause 2: ValueError: Invalid value for `containers`, must not be `None` raised in v1_pod_spec.py while deserializing the Pod the emulator returned — spec.containers absent where the Kubernetes API requires a non-empty list.

## Steps to Reproduce
Cause 1: create a cluster (Fargate profile variant), reach its API endpoint with an authenticated kubernetes client, create namespace aws-observability, then POST a ConfigMap (e.g. aws-logging) into it -> 405 MethodNotAllowed instead of 201. Cause 2: on a reachable cluster, create a pod referencing a private ECR image and read the pod back (GET /api/v1/namespaces/<ns>/pods/<name>) -> the returned object has no spec.containers, so a typed client raises.
