Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid
Issue-Key: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid|_k8s_expect#10|AssertionError|assert status in expect, (
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# eks__logs: the cluster's Kubernetes API does not serve the rbac.authorization.k8s.io/v1 group, so a Role cannot be created and no audit-log record is ever produced

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: the rbac.authorization.k8s.io/v1 group is not served by the cluster Kubernetes API, so the audit-log behavior behind it is unreachable.
AWS: recorded behavior only (the cluster's Kubernetes API server is not part of the EKS Smithy model; Smithy only models DescribeCluster -> Cluster.endpoint/certificateAuthority and the logging configuration). A real EKS cluster serves POST /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles and answers 201 Created with the stored Role, and — with the 'audit' control-plane log type enabled — writes one audit record per request into /aws/eks/<clusterName>/cluster naming the authenticated username that the access entry maps the IAM principal to, for both the allowed and the forbidden request.
Emulator: the Role create is answered HTTP 404 with {"kind":"Status","apiVersion":"v1","status":"Failure","message":"the server could not find the requested resource: /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles","reason":"NotFound","code":404} — the RBAC API group is absent from the server's routing and discovery, so neither the allow nor the forbid request can be set up and no audit record is emitted.
Why: a client relying on AWS's behavior cannot create the RBAC objects that decide authorization, and gets a 404 that reads as 'wrong apiVersion' rather than 'group not served'; downstream, nothing lands in the cluster audit log, so an operator auditing who did what sees an empty log group on a cluster whose DescribeCluster reports audit logging enabled.
Fix: the rbac.authorization.k8s.io/v1 group handlers (routing + discovery) of the served cluster Kubernetes API, and the audit-log emitter that must record each API-server request under /aws/eks/<clusterName>/cluster with the access-entry username.
Traces: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid.
Open issues 20261104T105252Z-eks-logs-the-cluster-s-certificateauthority-data-is.md and 20261110T203834Z-eks-logs-the-cluster-api-server-endpoint-is.md cover a CA/endpoint that could not be reached; here the endpoint WAS reached and answered 404 on the RBAC path, so neither covers this cause.

## Evidence
AssertionError: POST /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles returned 404, expected one of (200, 201): {'kind': 'Status', 'apiVersion': 'v1', 'metadata': {}, 'status': 'Failure', 'message': 'the server could not find the requested resource: /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles', 'reason': 'NotFound', 'details': None, 'code': 404}. Expected: 201 Created with the stored Role, then one audit record in /aws/eks/<clusterName>/cluster for the allowed and for the forbidden request, each naming the access entry's username.

## Steps to Reproduce
1. CreateCluster with control-plane logging type 'audit' enabled and an access entry for an IAM principal; wait for ACTIVE. 2. Reach cluster.endpoint with an authenticated Kubernetes client. 3. POST a Role to /apis/rbac.authorization.k8s.io/v1/namespaces/<namespace>/roles -> 404 NotFound instead of 201. 4. (Blocked) issue an allowed and a forbidden request and read /aws/eks/<clusterName>/cluster for the two audit records naming the access-entry username.
