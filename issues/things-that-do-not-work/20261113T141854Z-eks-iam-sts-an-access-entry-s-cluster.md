Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
Issue-Key: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token|run#28|AssertionError|assert admin[0]["accessScope"].get("namespaces") == [], admin
Issue-Commit: 5164801c346b7b34f471999eb845176f7f045b56

# eks__iam__sts: an access entry's cluster-scoped accessScope omits the namespaces member, so ListAssociatedAccessPolicies returns {type: cluster} with no namespaces list

> _Note: this issue was identified against the code base at commit `5164801c346b7b34f471999eb845176f7f045b56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAssociatedAccessPolicies

## Description
Cause 1 of 1: the AssociatedAccessPolicy.accessScope the emulator serves for a cluster-scoped policy association carries only the 'type' member; the 'namespaces' list AWS always serializes (empty for a cluster-scoped association) is absent from the response.

AWS: Smithy (com.amazonaws.eks, services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) models AccessScope with members 'type' (AccessScopeType enum, "The scope type of an access policy") and 'namespaces' (StringList, "A Kubernetes namespace that an access policy is scoped to. A value is required if you specified namespace for Type"), and AssociatedAccessPolicy as {policyArn, accessScope, associatedAt, modifiedAt}. For the message text / member presence this is recorded behavior only: the recording shows that for the cluster-creator admin association (policyArn .../AmazonEKSClusterAdminPolicy created by CreateCluster with bootstrapClusterCreatorAdminPermissions=true, accessConfig authenticationMode API_AND_CONFIG_MAP), AWS returns accessScope with BOTH members — type 'cluster' AND namespaces as a present, empty list. The requirement is that 'namespaces' is always present in a served AccessScope: an empty list when the scope type is cluster, and the scoped namespace names when the type is namespace. The policy ARN, the timestamps and the cluster name in the recording are per-run values and are not part of the requirement.

Emulator: ListAssociatedAccessPolicies for the creator access entry returns [{'policyArn': 'arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy', 'accessScope': {'type': 'cluster'}, 'associatedAt': ..., 'modifiedAt': ...}] — accessScope.namespaces is missing entirely, so a client reading it gets None rather than [].

Why: a client that reads accessScope['namespaces'] to decide whether an association is cluster-wide or namespace-scoped (SDK consumers get a typed list on AWS and a missing key here) raises a KeyError / dereferences null, or mis-classifies the scope. Any code that iterates the namespaces of every returned association breaks on the emulator while working against AWS.

Fix: the eks ListAssociatedAccessPolicies (and AssociateAccessPolicy) response serializer / access-entry state projection for AccessScope — always emit the 'namespaces' list, empty when the scope type is cluster.

Traces: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token (assertion at line 140 of the check: accessScope.get('namespaces') == []). Cross-reference: open issue 20261109T160052Z-eks-iam-sts-the-cluster-endpoint-handed-to.md carries this same check id but a different replay key and a different cause (Cluster.endpoint published on a real eks.amazonaws.com host, asserted at run#13); the check now progresses past that point and fails on the accessScope projection, which no open eks issue describes. The other open eks issues listed for access entries (20261104T105252Z-eks-10-net-new-root-causes-in-not.md) concern not-found/already-in-use refusal wording, not the served AccessScope shape.

## Evidence
Behavioral suite replay against the booted emulator (126/239 passed; emulator serving). Check listed under '### eks__iam__sts', not marked ALREADY FILED or OWNED.

Check eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token, line 140:
  assert admin[0]["accessScope"].get("namespaces") == [], admin
  AssertionError: [{'policyArn': 'arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy', 'accessScope': {'type': 'cluster'}, 'associatedAt': datetime.datetime(2026, 11, 13, 14, 6, 31, 743212, tzinfo=tzlocal()), 'modifiedAt': datetime.datetime(2026, 11, 13, 14, 6, 31, 743212, tzinfo=tzlocal())}]

Expected: accessScope == {'type': 'cluster', 'namespaces': []} — 'namespaces' present and empty for a cluster-scoped association.
Actual: accessScope == {'type': 'cluster'} — the member is absent. The policyArn is the AWS-managed cluster-admin policy ARN (fixed) and the two timestamps are per-run values; the defect is solely the missing namespaces list.

Model evidence: smithy_query(query="shape", shape="AccessScope,AssociatedAccessPolicy") on services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — AccessScope.namespaces targets StringList.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the booted emulator.
2. Replay check eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token: create an EKS cluster with accessConfig {authenticationMode: API_AND_CONFIG_MAP, bootstrapClusterCreatorAdminPermissions: true} and wait for it to become ACTIVE.
3. Call ListAccessEntries / ListAssociatedAccessPolicies for the cluster-creator principal's access entry and read the AmazonEKSClusterAdminPolicy association's accessScope.
4. The emulator returns {'type': 'cluster'}; AWS returns {'type': 'cluster', 'namespaces': []}. The check must pass (accessScope['namespaces'] == []) before this issue is closed.
