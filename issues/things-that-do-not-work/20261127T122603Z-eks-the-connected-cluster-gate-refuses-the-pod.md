Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.connected-cluster-gate-covers-list-reads-and-a-missing-cluster-fails-differently
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.connected-cluster-gate-covers-list-reads-and-a-missing-cluster-fails-differently|expect_error#7|AssertionError|assert got_code == code, f"expected {code}, got {got_code}: {got_message!r}"
Issue-Commit: 50a80390bb1e2ddab0fc937c65e59ad0dcd543f4

# eks: the connected-cluster gate refuses the pod-identity operations with ClientException instead of InvalidRequestException

> _Note: this issue was identified against the code base at commit `50a80390bb1e2ddab0fc937c65e59ad0dcd543f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePodIdentityAssociation

## Description
ONE net-new root cause: the gate that refuses unsupported operations on a CONNECTED (RegisterCluster'd) cluster raises a single error shape for every operation it guards, so the pod-identity operations get the access-policy operations' error code.

Cause 1 of 1: a connected cluster's refusal of CreatePodIdentityAssociation / ListPodIdentityAssociations carries Error.Code "ClientException" where AWS uses "InvalidRequestException".
  AWS:      Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): CreatePodIdentityAssociation (POST /clusters/{clusterName}/pod-identity-associations) and ListPodIdentityAssociations (GET /clusters/{clusterName}/pod-identity-associations) model errors InvalidParameterException, InvalidRequestException, ResourceNotFoundException and ServerException (Create also ResourceInUseException, ResourceLimitExceededException). Neither models ClientException at all. InvalidRequestException is documented "The request is invalid given the state of the cluster" — which is exactly what a connected cluster is — and is an @error("client") with httpError 400. The recording shows the refusal of these two operations on a connected cluster is HTTP 400 with Error.Code "InvalidRequestException" and the message "This operation is not supported for connected clusters" (that sentence is entirely fixed text — it names no resource and carries no per-run value). The SIBLING access-entry operations AssociateAccessPolicy and ListAssociatedAccessPolicies are refused on the same cluster with Error.Code "ClientException" and the SAME message, so the two families legitimately differ in code and agree in message; the per-operation code is the requirement.
  Emulator: all four refusals come back with Error.Code "ClientException" and the message "This operation is not supported for connected clusters". The two access-policy legs therefore pass and the two pod-identity legs diverge: the check asserts InvalidRequestException and gets ClientException with the correct message (`AssertionError: expected InvalidRequestException, got ClientException: 'This operation is not supported for connected clusters'`). The message, the HTTP status (400, shared by both shapes) and the effect-absence (the connector role survives the refusal) are all correct; only the error code is wrong.
  Why:      a client distinguishes these cases by modeled error shape, not by message text. A generated SDK client that catches InvalidRequestException around CreatePodIdentityAssociation/ListPodIdentityAssociations — the only shape the model lets it catch for this state — never catches the emulator's ClientException, so the refusal escapes as an unmodeled/unknown error and whatever recovery or retry classification the client attached to InvalidRequestException never runs. Conversely a client switching on ClientException sees the pod-identity family behave like the access-policy family, so code written against the emulator breaks on AWS.
  Fix:      the connected-cluster guard shared by the EKS pod-identity and access-entry handlers: it must emit the error shape the GUARDED OPERATION models for this state — InvalidRequestException for CreatePodIdentityAssociation and ListPodIdentityAssociations, ClientException for AssociateAccessPolicy and ListAssociatedAccessPolicies — rather than one shape for all of them. The message and the 400 status are already right and must not change.
  Traces:   eks.connected-cluster-gate-covers-list-reads-and-a-missing-cluster-fails-differently.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service heading `### eks`).

Failing check eks.connected-cluster-gate-covers-list-reads-and-a-missing-cluster-fails-differently, in its expect_error helper:
  AssertionError: expected InvalidRequestException, got ClientException: 'This operation is not supported for connected clusters'

Expected (AWS): HTTP 400 with Error.Code "InvalidRequestException" and Error.Message "This operation is not supported for connected clusters" for CreatePodIdentityAssociation and ListPodIdentityAssociations on a connected cluster.
Actual (emulator): HTTP 400 with Error.Code "ClientException" and that same message.

The check's own contract states the split: "A connected cluster refuses pod-identity (InvalidRequestException) and access-policy (ClientException) writes and List reads, keeping its connector role, while the same AssociateAccessPolicy on an unregistered cluster is ResourceNotFoundException." The AssociateAccessPolicy/ListAssociatedAccessPolicies legs (ClientException) and the ghost-cluster leg (ResourceNotFoundException, "The requested resource does not exist.") pass, and the DescribeCluster effect-absence assertion on connectorConfig.roleArn passes — so the registration, the message and the status are all correct and the single divergence is the error code of the pod-identity family. Cluster, role and service-account names in the check are per-run values and are not part of the requirement.

Smithy consulted: CreatePodIdentityAssociation and ListPodIdentityAssociations error lists (InvalidRequestException present, ClientException absent); ClientException and InvalidRequestException are both @error("client") httpError 400.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create an IAM role trusted by the EKS connector (sts:AssumeRole for the SSM service principal) and call EKS RegisterCluster(name=<c>, connectorConfig={roleArn:<role>, provider:"EKS_ANYWHERE"}), retrying while it answers ResourcePropagationDelayException or InvalidRequestException "Nonexistent role or missing ssm service principal in trust policy".
3. Call CreatePodIdentityAssociation(clusterName=<c>, namespace="default", serviceAccount=<sa>, roleArn=<role>) -> observe HTTP 400 Error.Code "ClientException"; expect Error.Code "InvalidRequestException" with Error.Message "This operation is not supported for connected clusters".
4. Call ListPodIdentityAssociations(clusterName=<c>) -> same divergence.
5. For contrast, call AssociateAccessPolicy(clusterName=<c>, principalArn=<role>, policyArn="arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy", accessScope={type:"cluster"}) and ListAssociatedAccessPolicies(clusterName=<c>, principalArn=<role>) -> ClientException, which is correct and must stay.
6. Replay eks.connected-cluster-gate-covers-list-reads-and-a-missing-cluster-fails-differently.
