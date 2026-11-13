Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.nodegroup-full-lifecycle eks.list-associated-access-policies-invalid-scope-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.nodegroup-full-lifecycle|run#25|AssertionError|assert dup_msg == f"NodeGroup already exists with name {nodegroup} and cluster name {cluster}", dup_msg
Issue-Key: eks.list-associated-access-policies-invalid-scope-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# eks: 2 net-new root causes in refusal wording — the duplicate-nodegroup ResourceInUseException sentence inserts colons, and ListAssociatedAccessPolicies answers a corrupt nextToken with ListClusters' sentence

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNodegroup

## Description
Two independent refusal-wording defects. Each cause below is a separate defect and must be closed separately.

Cause 1 of 2: CreateNodegroup's duplicate-nodegroup refusal inserts colons that AWS does not use.
AWS: Smithy: CreateNodegroup declares ResourceInUseException (errorKind client, httpError 409) alongside ClientException/InvalidParameterException/InvalidRequestException. For a nodegroup name that already exists on the cluster, AWS's message is 'NodeGroup already exists with name <nodegroupName> and cluster name <clusterName>' — the fixed text is 'NodeGroup already exists with name ' / ' and cluster name ', with no colons; the two variable parts are the request's nodegroupName and clusterName.
Emulator: the same call is refused with 'NodeGroup already exists with name: <nodegroupName> and cluster name: <clusterName>' — a colon is inserted after each 'name' label, so the fixed text does not match AWS.
Why: a client (or an operator runbook) that matches the message text to distinguish a benign re-create from another in-use condition does not match, and the mismatched sentence leaks into user-facing error reporting.
Fix: the CreateNodegroup handler's duplicate/already-exists refusal message (its validation layer), not the error class — the class and 409 status are already right.
Traces: eks.nodegroup-full-lifecycle. Note: open issue 20261104T105252Z-eks-10-net-new-root-causes-in-not.md owns the same wording seen from eks.nodegroup-duplicate-create-rejected; this check is not covered by it and is reported here so the fix is re-verified against this trace too.

Cause 2 of 2: ListAssociatedAccessPolicies answers an unparsable nextToken with the wrong sentence.
AWS: Smithy: ListAssociatedAccessPolicies(clusterName, principalArn, maxResults 1..100, nextToken) declares InvalidRequestException (400), ResourceNotFoundException (404) and ServerException (500); the pagination-token refusal AWS actually returns for this operation is a 400 whose message is the fixed sentence 'The specified nextToken is invalid: unrecognized format.' (no variable parts).
Emulator: the same corrupted token is refused with 'nextToken is not valid or expired.' — that is the sentence AWS uses for ListClusters, applied here by a shared pagination helper.
Why: the two sentences are different requirements on different operations; a client that keys its retry/reset logic on the message (or an operator reading it) is told the token expired when AWS said its format was unrecognized. See open issue 20261113T004144Z-eks-listclusters-invalid-nexttoken-refusal-is-worded-the.md, which records the SAME two sentences swapped on ListClusters — the emulator appears to have one global token message, while AWS words it per operation.
Fix: the ListAssociatedAccessPolicies nextToken validation (the shared pagination-token validator must carry a per-operation message rather than one sentence for all paginated EKS operations).
Traces: eks.list-associated-access-policies-invalid-scope-rejected.

## Evidence
Cause 1: expected message 'NodeGroup already exists with name <ng> and cluster name <cluster>'; emulator returned 'NodeGroup already exists with name: <ng> and cluster name: <cluster>' (check eks.nodegroup-full-lifecycle assertion: AssertionError: NodeGroup already exists with name: ng-..., and cluster name: sim2real-behav-eks-ng-...). Cause 2: AssertionError: expected message 'The specified nextToken is invalid: unrecognized format.', got 'nextToken is not valid or expired.' on ListAssociatedAccessPolicies with a corrupted nextToken.

## Steps to Reproduce
Cause 1: CreateCluster -> wait ACTIVE -> CreateNodegroup(name N) -> CreateNodegroup(name N) again; read the ResourceInUseException message and compare the fixed text (no colons after 'name'). Cause 2: create a cluster, an access entry and associate a policy, then call ListAssociatedAccessPolicies(clusterName, principalArn, nextToken='<corrupted token>') and read the 400 message.
