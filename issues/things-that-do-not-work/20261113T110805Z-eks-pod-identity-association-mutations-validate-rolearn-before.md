Issue-Source: trace-sweep-divergence
Issue-Trace: eks-eks-errors-missing-cluster
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-eks-errors-missing-cluster eks-pod-identity-association-missing-cluster
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreatePodIdentityAssociation|4xx->4xx|$.message
Issue-Key: eks|CreatePodIdentityAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreatePodIdentityAssociation|4xx->4xx|status
Issue-Key: eks|UpdatePodIdentityAssociation|4xx->4xx|$.message
Issue-Key: eks|UpdatePodIdentityAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|UpdatePodIdentityAssociation|4xx->4xx|status
Issue-Commit: 95f1e080048228d599071943e64424bac0fa7206

# eks pod-identity association mutations validate roleArn before the cluster exists, answering 400 InvalidParameterException instead of EKS's 404 not-found

> _Note: this issue was identified against the code base at commit `95f1e080048228d599071943e64424bac0fa7206`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePodIdentityAssociation

## Description
Two net-new root causes share one shape: a pod-identity association mutation addressed at a cluster name that does not exist is refused for the ROLE it was given rather than for the MISSING CLUSTER. The emulator runs its roleArn trust-policy validation before it resolves the {clusterName} path label, so the caller is told its role trust policy is wrong (400 InvalidParameterException) when the real fault is that there is no such cluster (404 ResourceNotFoundException). The cluster name and role ARN quoted below are template slots filled from the request/state, never requirements.

Cause 1 of 2: CreatePodIdentityAssociation against a nonexistent cluster.
  AWS: Smithy: CreatePodIdentityAssociation, POST /clusters/{clusterName}/pod-identity-associations, clusterName is a required @httpLabel; declared errors include ResourceNotFoundException (httpError 404) and InvalidParameterException (httpError 400). The cluster referenced by the path label is resolved first, so a name that names no cluster in the account/region is refused 404 ResourceNotFoundException with x-amzn-errortype: ResourceNotFoundException and the message 'No cluster found for name: <clusterName>.' — fixed text 'No cluster found for name: ' plus a trailing period, the single variable part being the clusterName taken verbatim from the request path. The roleArn is never examined, so no trust-policy verdict is reachable on a cluster that does not exist.
  Emulator: HTTP 400 with x-amzn-errortype: InvalidParameterException and the message 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.', and the response body carries no clusterName field at all (the recorded AWS error body echoes clusterName: null for this error class, while the emulator populates it with the requested name on the 404-class siblings).
  Why: a client provisioning pod identity cannot tell a typo'd or not-yet-created cluster from a genuinely malformed IAM role. Retry/wait logic keyed on ResourceNotFoundException (wait for the cluster to appear, or create it) never fires; instead the caller is sent to rewrite a trust policy that is in fact fine, and a 400 is treated as a permanent request defect rather than a resolvable missing dependency. CloudFormation-style providers that map 404 to "dependency not ready" fail the stack outright.
  Fix: CreatePodIdentityAssociation's handler — resolve the {clusterName} path label against cluster state and emit the 404 ResourceNotFoundException BEFORE any roleArn/trust-policy validation runs; the role validation belongs after cluster resolution.
  Traces: eks-eks-errors-missing-cluster#task#3 (also eks-pod-identity-association-missing-cluster#task#1).

Cause 2 of 2: UpdatePodIdentityAssociation against a nonexistent cluster.
  AWS: Smithy: UpdatePodIdentityAssociation, POST /clusters/{clusterName}/pod-identity-associations/{associationId}, clusterName and associationId are required @httpLabels; declared errors include ResourceNotFoundException (httpError 404) and InvalidParameterException (httpError 400). The same ordering applies: an unknown clusterName is refused 404 ResourceNotFoundException with 'No cluster found for name: <clusterName>.' — same fixed text, clusterName from the request — without inspecting the supplied roleArn or the associationId.
  Emulator: HTTP 400 with x-amzn-errortype: InvalidParameterException and the same trust-policy sentence, 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'
  Why: an update path that is expected to surface 'the cluster is gone' instead reports a role-shape defect, so reconciliation loops that delete-and-recreate on 404, or that wait for a cluster to come back, are never entered; the operator is pointed at the wrong input.
  Fix: UpdatePodIdentityAssociation's handler — same ordering defect; resolve the cluster (and then the associationId) before validating roleArn.
  Traces: eks-eks-errors-missing-cluster#task#5 (also eks-pod-identity-association-missing-cluster#task#2).

Neither cause is covered by an existing issue. The open issue 20261104T105252Z-eks-10-net-new-root-causes-in-not.md covers pod-identity NOT-FOUND wording for an unknown associationId on an EXISTING cluster (Cause 6) and duplicate-association wording (Cause 7); 20261104T105252Z-eks-9-net-new-root-causes-in-parameter.md covers other parameter-validation ordering faults. The replay reports both clusters here as unowned: the defect is the missing-cluster precondition on the pod-identity association mutations.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (55 fixtures, 393 steps, 278 passed, 115 mismatched, score 70.7%).

Cause 1 — fixture eks-eks-errors-missing-cluster, step task#3, operation CreatePodIdentityAssociation (cluster cluster=2 steps):
  status: expected 404, got 400
  $.message: expected 'No cluster found for name: sim2real-probe-eks-pi-missing-cluster-b6aa9ab7.', got 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'
  $headers.x-amzn-errortype: expected ResourceNotFoundException-class (recorded as '<shaped-not-asserted>'), got 'InvalidParameterException'
  Same divergence at eks-pod-identity-association-missing-cluster#task#1.
  Fixed text of the AWS message: 'No cluster found for name: ' + '.'; variable part: the requested clusterName (here a per-run probe name, not a requirement).

Cause 2 — fixture eks-eks-errors-missing-cluster, step task#5, operation UpdatePodIdentityAssociation (cluster of 2 steps):
  status: expected 404, got 400
  $.message: expected 'No cluster found for name: sim2real-probe-eks-pi-missing-cluster-b6aa9ab7.', got 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'
  $headers.x-amzn-errortype: got 'InvalidParameterException'
  Same divergence at eks-pod-identity-association-missing-cluster#task#2.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreatePodIdentityAssociation (POST /clusters/{clusterName}/pod-identity-associations; clusterName/namespace/serviceAccount/roleArn required; errors InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceLimitExceededException 400, ResourceNotFoundException 404, ServerException 500); UpdatePodIdentityAssociation (POST /clusters/{clusterName}/pod-identity-associations/{associationId}; errors InvalidParameterException 400, InvalidRequestException 400, ResourceNotFoundException 404, ServerException 500).

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and re-run fixture eks-eks-errors-missing-cluster steps task#3 and task#5 (and fixture eks-pod-identity-association-missing-cluster steps task#1 and task#2).
3. Manually, against the emulator's EKS endpoint, with NO cluster of that name created:
   a. POST /clusters/<name-that-does-not-exist>/pod-identity-associations with a well-formed body (namespace, serviceAccount, roleArn pointing at any IAM role) -> expect HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'No cluster found for name: <name-that-does-not-exist>.'; observe HTTP 400 InvalidParameterException 'Trust policy of the role provided is invalid. Follow the official documentation to resolve the issue.'
   b. POST /clusters/<name-that-does-not-exist>/pod-identity-associations/<any-association-id> with a roleArn -> expect the same 404 ResourceNotFoundException 'No cluster found for name: <name-that-does-not-exist>.'; observe the same 400 trust-policy refusal.
4. Confirm the fix by repeating (a) and (b) with a roleArn whose trust policy IS valid and with one that is not: on a nonexistent cluster both must yield the identical 404 not-found refusal, because the role is never inspected.
