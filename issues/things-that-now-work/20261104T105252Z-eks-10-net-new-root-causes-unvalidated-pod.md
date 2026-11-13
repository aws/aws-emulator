Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.create-pod-identity-association-untrusted-role-rejected eks.pod-identity-association-untrusted-role-rejected eks.pod-identity-association-cross-account-role-rejected eks.fargate-profile-public-subnet-rejected eks.ekscc-list-access-entries-on-config-map-cluster-rejected eks.pod-identity-association-tags-empty-vs-absent-preserved eks.pod-identity-association-tags-unicode-round-trip eks.pod-identity-tags-unicode-round-trip eks.nodegroup-full-lifecycle eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod eks.access-entry-ec2-linux-defaults-idempotent-create-and-pagination eks.access-entry-takes-precedence-over-the-aws-auth-configmap eks.cluster-kubernetes-api-server-serves-advertised-version eks.deploy-cluster-active-and-api-reachable eks.e2e-access-entry-kubectl-authorization-flow eks.eks-access-entry-deletion-leaves-the-principal-unauthorized eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports eks.eks-delete-cluster-with-a-live-node-is-rejected eks.eks-deployment-pod-is-reachable-through-a-cluster-service eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied eks.eks-oidc-discovery-and-jwks-verify-a-projected-token-offline eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni eks.eks-view-access-policy-write-is-forbidden-by-the-api-server eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator eks.irsa-cross-namespace-serviceaccount-token-rejected eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered eks.k8s-configmap-mounted-value-readable-inside-the-pod eks.k8s-deployment-apply-runs-pods-on-real-nodes eks.k8s-deployment-scale-converges-to-the-requested-replicas eks.k8s-rbac-get-only-role-rejects-deleting-a-pod eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit eks.k8s-rolling-update-replaces-pods-without-dropping-the-service eks.k8s-service-clusterip-load-balances-across-pods eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds eks.namespace-scoped-access-policy-confines-reads-to-its-namespace eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod eks.eks-kubectl-namespace eks.capability-configuration-blob-round-trip eks.capability-create-malformed-input-rejected eks.capability-lifecycle-transitions eks.capability-malformed-name-rejected eks.capability-server-side-managed-status-transition eks.capability-update-requires-existing-ordering eks.create-capability-argocd-config-on-ack-type-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.create-pod-identity-association-untrusted-role-rejected|_error#12|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: eks.pod-identity-association-untrusted-role-rejected|_client_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: eks.pod-identity-association-cross-account-role-rejected|_error_code#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: eks.fargate-profile-public-subnet-rejected|_error_detail#13|AssertionError|raise AssertionError("expected the call to fail, but it succeeded")
Issue-Key: eks.ekscc-list-access-entries-on-config-map-cluster-rejected|_expect_error#6|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: eks|TagResource|NotFoundException
Issue-Key: eks.nodegroup-full-lifecycle|run#20|KeyError|assert active["resources"]["autoScalingGroups"], active["resources"]
Issue-Key: eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod|run#5|KeyError|asgs = [g["name"] for g in ng["resources"]["autoScalingGroups"]]
Issue-Key: eks.access-entry-ec2-linux-defaults-idempotent-create-and-pagination|run#10|AssertionError|assert created["kubernetesGroups"] == ["system:nodes"], (
Issue-Key: eks.access-entry-takes-precedence-over-the-aws-auth-configmap|_k8s#12|SSLError|ssl_ctx = ssl.create_default_context(
Issue-Key: eks.cluster-kubernetes-api-server-serves-advertised-version|_https_get#19|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.deploy-cluster-active-and-api-reachable|_http_get_k8s_version#13|SSLError|ssl_ctx = ssl.create_default_context(cadata=ca_pem)
Issue-Key: eks.e2e-access-entry-kubectl-authorization-flow|_k8s#15|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-access-entry-deletion-leaves-the-principal-unauthorized|_k8s#14|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports|_k8s#6|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-delete-cluster-with-a-live-node-is-rejected|_k8s#5|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-deployment-pod-is-reachable-through-a-cluster-service|_k8s#7|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule|_k8s#20|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied|_k8s#30|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-oidc-discovery-and-jwks-verify-a-projected-token-offline|_k8s#15|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster|_k8s#11|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni|_k8s#21|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.eks-view-access-policy-write-is-forbidden-by-the-api-server|_k8s#15|SSLError|ssl_ctx.load_verify_locations(cadata=ca_pem)
Issue-Key: eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator|_run#15|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {' '.join(cmd[:4])} exited {proc.returncode}: "
Issue-Key: eks.irsa-cross-namespace-serviceaccount-token-rejected|_run#30|AssertionError|raise AssertionError(
Issue-Key: eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered|_run#29|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-configmap-mounted-value-readable-inside-the-pod|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-deployment-apply-runs-pods-on-real-nodes|_sh#4|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-deployment-scale-converges-to-the-requested-replicas|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-rbac-get-only-role-rejects-deleting-a-pod|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-rolling-update-replaces-pods-without-dropping-the-service|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.k8s-service-clusterip-load-balances-across-pods|_sh#3|AssertionError|raise AssertionError(
Issue-Key: eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service|_run#22|AssertionError|raise AssertionError(
Issue-Key: eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds|_run#17|AssertionError|raise AssertionError(
Issue-Key: eks.namespace-scoped-access-policy-confines-reads-to-its-namespace|_k8s#5|SSLError|ssl_ctx = ssl.create_default_context(cadata=ca_pem)
Issue-Key: eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod|_k8s#12|SSLError|ssl_ctx = ssl.create_default_context(cadata=ca_pem)
Issue-Key: eks.eks-kubectl-namespace|_wait_api_reachable#11|AssertionError|raise AssertionError(
Issue-Key: eks|CreateCapability|InvalidParameterException
Issue-Key: eks.capability-create-malformed-input-rejected|_retry_on_client_error#10|Timeout|time.sleep(interval_s)
Issue-Key: eks.capability-lifecycle-transitions|_create_capability_with_retry#6|Timeout|time.sleep(delay_s)
Issue-Key: eks.capability-malformed-name-rejected|_create_capability_with_retry#7|Timeout|time.sleep(5)
Issue-Key: eks.capability-server-side-managed-status-transition|_create_capability_with_retry#11|Timeout|time.sleep(interval_s)
Issue-Key: eks.capability-update-requires-existing-ordering|_retry_on_trust_policy_lag#14|Timeout|time.sleep(interval_s)
Issue-Key: eks.create-capability-argocd-config-on-ack-type-rejected|_create_capability_with_trust_retry#8|Timeout|time.sleep(delay_s)
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks: 10 net-new root causes — unvalidated pod-identity roles, TagResource cannot see EKS resources, missing response members, an unusable certificateAuthority and CreateCapability refusing valid roles

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Causes 1-7 are implemented (pod-identity role trust and the cross-account pass-role on
create and update, the private-subnet rule for a Fargate profile, the authentication-mode
gate on all eight access-entry operations, the tag plane over every resource this service
mints an ARN for, a managed node group's real Auto Scaling group and launch template, and
the server-derived Kubernetes groups of a node access entry). Causes 8 and 10 were already
resolved before this fix: a cluster issues a real X.509 certificate authority, and a
capability role is accepted when its trust policy names any principal Amazon EKS publishes
for one.

CAUSE 9 is NOT resolved here and is not this issue's to close: every check of it — and
every check that was filed under cause 8 — now stops before any Kubernetes request is
made, because the endpoint a cluster PUBLISHES is an `*.eks.amazonaws.com` host rather
than an address of this emulator, so no client can reach anything served behind it. That
defect is owned by `20261110T195824Z-eks-auth-cluster-endpoint-is-published-as-a.md`,
`20261109T141609Z-ecr-eks-logs-a-cluster-s-published-api.md` and
`20261112T072951Z-eks-2-net-new-root-causes-cluster-endpoint.md`; the Kubernetes API
surface this cause asks for can only be observed once one of them lands.

## Operation
CreatePodIdentityAssociation

## Description
Ten net-new root causes spanning missing refusals, missing response state and one defect that makes the entire Kubernetes data plane unreachable. Names, ids and ARNs quoted below are template slots from the request or the emulator's state.

Cause 1 of 10: CreatePodIdentityAssociation accepts a roleArn whose trust policy does not permit pods.eks.amazonaws.com.
  AWS: Smithy: CreatePodIdentityAssociation errors include InvalidParameterException 400. A role that does not trust pods.eks.amazonaws.com (e.g. one trusting only ec2.amazonaws.com) is refused InvalidParameterException with 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.' — the whole sentence is fixed text — and no association is left behind.
  Emulator: HTTP 200, returning an association bound to the untrusted role.
  Why: pod identity depends on that trust relationship; the emulator records an association that can never issue credentials, so the failure surfaces only at runtime (or only on AWS).
  Fix: CreatePodIdentityAssociation's role validation — read the role's trust policy and require the pods.eks service principal.
  Traces: eks.create-pod-identity-association-untrusted-role-rejected, eks.pod-identity-association-untrusted-role-rejected.

Cause 2 of 10: pod-identity associations accept a roleArn from another account.
  AWS: a roleArn in a different account is refused AccessDeniedException with 'Cross-account pass role is not allowed.' — the whole sentence is fixed text (the check asserts exactly this for the same cross-account role on UpdatePodIdentityAssociation, and requires the create to be refused too).
  Emulator: HTTP 200, returning an association whose roleArn is in account 111111111111 while the cluster is in account 000000000000.
  Why: the emulator permits a cross-account pass-role AWS forbids, so an IAM boundary that exists in production is invisible here.
  Fix: the shared role-validation path of Create/UpdatePodIdentityAssociation — reject a roleArn whose account differs from the caller's.
  Traces: eks.pod-identity-association-cross-account-role-rejected.

Cause 3 of 10: CreateFargateProfile accepts a public subnet.
  AWS: a Fargate profile may only use private subnets; a public one is refused InvalidParameterException with 'Subnet <subnetId> provided in Fargate Profile is not a private subnet' — fixed text 'Subnet ' / ' provided in Fargate Profile is not a private subnet' with the offending subnet id as the variable — and the profile does not exist afterwards.
  Emulator: the call succeeds.
  Why: Fargate pods cannot run in a public subnet; accepting the profile defers a hard AWS failure to deploy time.
  Fix: CreateFargateProfile's subnet validation.
  Traces: eks.fargate-profile-public-subnet-rejected.

Cause 4 of 10: ListAccessEntries succeeds on a CONFIG_MAP-only cluster.
  AWS: when the cluster's accessConfig.authenticationMode is CONFIG_MAP the access-entry APIs are refused InvalidRequestException with "The cluster's authentication mode must be set to one of [API, API_AND_CONFIG_MAP] to perform this operation." — the whole sentence is fixed text. The same call on an API_AND_CONFIG_MAP cluster succeeds, so the refusal follows the stored authentication mode.
  Emulator: the call succeeds on a CONFIG_MAP cluster.
  Why: a caller cannot discover that the cluster is still aws-auth-based; it reads an empty access-entry list and concludes the API mode is active.
  Fix: the access-entry operations' authentication-mode guard.
  Traces: eks.ekscc-list-access-entries-on-config-map-cluster-rejected.

Cause 5 of 10: TagResource does not recognise a pod-identity association ARN.
  AWS: Smithy: TagResource (POST /tags/{resourceArn}) declares NotFoundException 404 for an unknown ARN; a pod-identity association ARN returned by CreatePodIdentityAssociation IS a taggable EKS resource, so TagResource succeeds (HTTP 200, empty body) and the tags appear on the association and in ListTagsForResource.
  Emulator: NotFoundException 404 'Resource was not found' for the association ARN it had just minted.
  Why: tagging a pod-identity association is impossible, so tag-driven cost allocation and tag-based access control cannot be exercised at all; the 404 also contradicts the ARN the emulator itself returned.
  Fix: TagResource/UntagResource ARN resolution — include pod-identity association ARNs in the resource lookup, and keep the tags on the association record.
  Traces: eks.pod-identity-association-tags-empty-vs-absent-preserved, eks.pod-identity-association-tags-unicode-round-trip, eks.pod-identity-tags-unicode-round-trip.

Cause 6 of 10: DescribeNodegroup omits the `resources` member.
  AWS: Smithy: Nodegroup.resources -> NodegroupResources {autoScalingGroups: AutoScalingGroupList, remoteAccessSecurityGroup: String}, documented as 'The resources associated with the node group, such as Auto Scaling groups and security groups for remote access.' A managed nodegroup that has reached ACTIVE reports it, naming the Auto Scaling group that backs the nodegroup.
  Emulator: the nodegroup object has no 'resources' key at all, so a client indexing it raises KeyError.
  Why: the Auto Scaling group name is the only documented way to reach the nodegroup's underlying capacity; without it any client that scales or inspects nodes through ASG fails outright rather than degrading.
  Fix: the Nodegroup serializer/state — populate resources.autoScalingGroups (and remoteAccessSecurityGroup when remote access is configured).
  Traces: eks.nodegroup-full-lifecycle, eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod.

Cause 7 of 10: an EC2_LINUX access entry gets no server-derived kubernetesGroups.
  AWS: Smithy: AccessEntry.kubernetesGroups -> StringList. For the server-managed type EC2_LINUX, EKS derives the groups itself and the created entry reports kubernetesGroups ['system:nodes'] even though the request sends none.
  Emulator: the created entry reports kubernetesGroups [].
  Why: the node-authorization grant the type exists to confer is absent, so a caller inspecting the entry cannot tell that nodes are authorized, and anything that reconciles groups will try to add them (which AWS refuses for this type).
  Fix: CreateAccessEntry's type handling — derive kubernetesGroups for server-managed types.
  Traces: eks.access-entry-ec2-linux-defaults-idempotent-create-and-pagination.

Cause 8 of 10: cluster.certificateAuthority.data is a placeholder, not a certificate, so no TLS client can reach the cluster.
  AWS: Smithy: Certificate.data is documented as 'The Base64-encoded certificate data required to communicate with your cluster. Add this to the certificate-authority-data section of the kubeconfig file for your cluster.' Base64-decoding it yields a PEM X.509 certificate that verifies the endpoint the cluster advertises.
  Emulator: the field decodes to the literal text 'EMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this is not a ...', which is not a PEM block. Every TLS client fails identically: python ssl raises 'ssl.SSLError: no start line: cadata does not contain a certificate', and kubectl exits 1 with 'error: unable to load root certificates: unable to parse bytes as PEM block'.
  Why: the kubeconfig EKS's own output builds is unusable, so the entire Kubernetes data plane — kubectl, access-entry authorization, pod/service/CoreDNS behavior, IRSA token verification, add-on image checks — cannot be reached by any client, and 28 behavioral checks fail before they can assert anything.
  Fix: the cluster's certificateAuthority state/serializer must emit the Base64-encoded PEM certificate of the CA that signs the served Kubernetes endpoint. Open issues 20261104T060239Z-ecr-eks-logs-a-cluster-s-certificateauthority-data.md and 20261104T053901Z-eks-auth-the-whole-pod-identity-data-plane.md describe the same placeholder under the ecr__eks__logs and eks-auth headings; all 28 checks below are reported unowned under the eks heading.
  Traces: eks.access-entry-takes-precedence-over-the-aws-auth-configmap, eks.cluster-kubernetes-api-server-serves-advertised-version, eks.deploy-cluster-active-and-api-reachable, eks.e2e-access-entry-kubectl-authorization-flow, eks.eks-access-entry-deletion-leaves-the-principal-unauthorized, eks.eks-coredns-resolves-a-service-to-the-clusterip-the-api-reports, eks.eks-delete-cluster-with-a-live-node-is-rejected, eks.eks-deployment-pod-is-reachable-through-a-cluster-service, eks.eks-ebs-csi-volume-data-survives-a-pod-reschedule, eks.eks-irsa-projected-token-assumed-inside-the-pod-and-sub-condition-denied, eks.eks-oidc-discovery-and-jwks-verify-a-projected-token-offline, eks.eks-pinned-addon-version-is-the-image-running-in-the-cluster, eks.eks-pod-ip-is-a-secondary-address-on-a-real-vpc-eni, eks.eks-view-access-policy-write-is-forbidden-by-the-api-server, eks.ekscc-active-cluster-serves-kubernetes-api-at-pinned-version-to-its-creator, eks.irsa-cross-namespace-serviceaccount-token-rejected, eks.irsa-web-identity-rejected-when-the-oidc-provider-is-deregistered, eks.k8s-configmap-mounted-value-readable-inside-the-pod, eks.k8s-deployment-apply-runs-pods-on-real-nodes, eks.k8s-deployment-scale-converges-to-the-requested-replicas, eks.k8s-rbac-get-only-role-rejects-deleting-a-pod, eks.k8s-resourcequota-rejects-a-pod-over-the-namespace-limit, eks.k8s-rolling-update-replaces-pods-without-dropping-the-service, eks.k8s-service-clusterip-load-balances-across-pods, eks.kubectl-applied-deployment-pod-runs-and-serves-its-configmap-through-a-service, eks.kubectl-denied-verb-returns-the-exact-forbidden-message-while-a-permitted-verb-succeeds, eks.namespace-scoped-access-policy-confines-reads-to-its-namespace, eks.nodegroup-taint-effect-is-translated-and-repels-an-untolerating-pod.

Cause 9 of 10: the advertised cluster endpoint serves no Kubernetes API discovery.
  AWS: the endpoint a cluster advertises answers the Kubernetes discovery endpoints (/api, /apis), so `kubectl get <resource> -n <namespace>` resolves the API group list and then the resource.
  Emulator: reached without TLS verification, the endpoint answers the discovery request as not-found: kubectl reports 'E... memcache.go:381] "Couldn't get current server API group list" err="the server could not find the requested resource"' and then 'Error from server (NotFound): the server could not find the requested resource'. This is distinct from Cause 8: here the connection succeeded and the server answered.
  Why: even a client that ignores the CA problem cannot use the cluster, because no Kubernetes resource can be discovered or addressed.
  Fix: the served Kubernetes API surface for a cluster endpoint — serve the discovery documents and the namespaced resource routes.
  Traces: eks.eks-kubectl-namespace.

Cause 10 of 10: CreateCapability refuses a role that exists and trusts EKS.
  AWS: CreateCapability with a roleArn that exists and whose trust policy names the EKS service principal is accepted; the capability is created and progresses through its lifecycle. A roleArn refusal is reserved for a role that is absent or does not trust EKS.
  Emulator: InvalidParameterException 'roleArn Nonexistent role or missing eks service principal in trust policy: <roleArn>' for a role the harness had just created (and confirmed readable through IAM) with the EKS trust policy attached.
  Why: the whole capability surface is unreachable — every capability check either fails on this refusal or spins in the role-propagation retry the refusal triggers until the suite's 60s budget expires, so create/update/delete ordering, status transitions and the configuration blob can never be exercised.
  Fix: CreateCapability's roleArn validation — resolve the role through the emulator's IAM state and accept a trust policy naming the EKS service principal.
  Traces: eks.capability-configuration-blob-round-trip (direct refusal); eks.capability-create-malformed-input-rejected, eks.capability-lifecycle-transitions, eks.capability-malformed-name-rejected, eks.capability-server-side-managed-status-transition, eks.capability-update-requires-existing-ordering, eks.create-capability-argocd-config-on-ack-type-rejected (same call, retried until timeout).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service eks 142/306 passed).

Cause 1: eks.pod-identity-association-untrusted-role-rejected — 'expected a rejection, but the call succeeded and returned {... HTTPStatusCode: 200, association: {... roleArn: arn:aws:iam::000000000000:role/sim2real-eks-untrusted-<id>}}'; the check asserts ('InvalidParameterException', 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'). The role it passes trusts only ec2.amazonaws.com; its positive control trusts pods.eks.amazonaws.com.
Cause 2: eks.pod-identity-association-cross-account-role-rejected — 'expected a ClientError, but the call SUCCEEDED and returned {... roleArn: arn:aws:iam::111111111111:role/no-trust-<id>}'; the check asserts AccessDeniedException 'Cross-account pass role is not allowed.'
Cause 3: eks.fargate-profile-public-subnet-rejected — 'expected the call to fail, but it succeeded'; required InvalidParameterException 'Subnet <subnetId> provided in Fargate Profile is not a private subnet'.
Cause 4: eks.ekscc-list-access-entries-on-config-map-cluster-rejected — 'the call SUCCEEDED; real AWS rejects it'; required InvalidRequestException "The cluster's authentication mode must be set to one of [API, API_AND_CONFIG_MAP] to perform this operation."
Cause 5: eks.pod-identity-association-tags-unicode-round-trip / -tags-empty-vs-absent-preserved / eks.pod-identity-tags-unicode-round-trip — 'botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the TagResource operation: Resource was not found'.
Cause 6: eks.nodegroup-full-lifecycle and eks.eks-managed-nodegroup-node-joins-and-runs-a-targeted-pod — "KeyError: 'resources'" while indexing the DescribeNodegroup nodegroup object.
Cause 7: eks.access-entry-ec2-linux-defaults-idempotent-create-and-pagination — "EC2_LINUX access entries must get the server-derived group ['system:nodes']; got []".
Cause 8: 28 checks fail with one of two messages — 'ssl.SSLError: no start line: cadata does not contain a certificate (_ssl.c:3977)' after ssl_ctx.load_verify_locations(cadata=ca_pem), or 'DATAPLANE_UNIMPLEMENTED: ... error: unable to load root certificates: unable to parse bytes as PEM block'. The base64 in the emulator's DescribeCluster response decodes to '-----BEGIN CERTIFICATE-----\nEMULATOR PLACEHOLDER: this emulator runs no certificate authority, so this is not a ...'.
Cause 9: eks.eks-kubectl-namespace — 'E1104 ... memcache.go:381] "Couldn\'t get current server API group list" err="the server could not find the requested resource"' / 'Error from server (NotFound): the server could not find the requested resource'.
Cause 10: eks.capability-configuration-blob-round-trip — 'create_capability never succeeded after IAM propagation retries: An error occurred (InvalidParameterException) when calling the CreateCapability operation: roleArn Nonexistent role or missing eks service principal in trust policy: arn:aws:iam::000000000000:role/sim2real-caprole-<id>'; the six sibling capability checks report 'Check timed out after 60.0s' on the same create-with-retry path.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreatePodIdentityAssociation errors InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceLimitExceededException 400, ResourceNotFoundException 404; TagResource POST /tags/{resourceArn}, errors BadRequestException 400, NotFoundException 404; Nodegroup.resources -> NodegroupResources {autoScalingGroups, remoteAccessSecurityGroup}; AccessEntry.kubernetesGroups -> StringList; Certificate.data ('Base64-encoded certificate data required to communicate with your cluster').

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the check ids in trace_ids.
3. Manually, against the emulator's EKS endpoint:
   a. Create an IAM role trusting only ec2.amazonaws.com and call CreatePodIdentityAssociation with it -> expect 400 InvalidParameterException 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'; repeat with a roleArn in another account -> expect AccessDeniedException 'Cross-account pass role is not allowed.'
   b. CreateFargateProfile with a public subnet -> expect InvalidParameterException 'Subnet <subnetId> provided in Fargate Profile is not a private subnet'.
   c. Create a cluster with accessConfig.authenticationMode CONFIG_MAP and call ListAccessEntries -> expect InvalidRequestException "The cluster's authentication mode must be set to one of [API, API_AND_CONFIG_MAP] to perform this operation."
   d. CreatePodIdentityAssociation, then TagResource with the returned associationArn -> expect 200 and the tags readable through ListTagsForResource/DescribePodIdentityAssociation.
   e. CreateNodegroup, wait for ACTIVE, DescribeNodegroup -> expect a 'resources' member naming the backing Auto Scaling group.
   f. CreateAccessEntry(type='EC2_LINUX') with no kubernetesGroups -> expect kubernetesGroups ['system:nodes'].
   g. DescribeCluster and base64-decode certificateAuthority.data -> expect a PEM X.509 certificate that verifies the advertised endpoint; build a kubeconfig from it and run 'kubectl get ns' -> expect success, and 'kubectl get pods -n <ns>' to resolve the API group list.
   h. Create an IAM role trusting the EKS service principal and call CreateCapability with it -> expect the capability to be created, not InvalidParameterException 'roleArn Nonexistent role or missing eks service principal in trust policy: <arn>'.
