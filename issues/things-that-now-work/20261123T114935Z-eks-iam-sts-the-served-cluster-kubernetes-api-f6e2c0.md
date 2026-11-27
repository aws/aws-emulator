Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
Issue-Key: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token|run#38|KeyError|assert user_info["uid"].startswith(f"aws-iam-authenticator:{account_id}:"), user_info
Issue-Commit: d4a73420c47c8214eb7a45d67f6bbcf3a13465a4

# eks__iam__sts: the served cluster Kubernetes API's SelfSubjectReview reports a userInfo with no uid, so the authenticated IAM principal's aws-iam-authenticator identity is unobservable

> _Note: this issue was identified against the code base at commit `d4a73420c47c8214eb7a45d67f6bbcf3a13465a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: a SelfSubjectReview against the cluster's served Kubernetes API answers 201 with a status.userInfo that omits the `uid` member entirely.
AWS: recorded behavior only — this is the Kubernetes API EKS serves for a cluster, not a Smithy-modeled EKS operation (the Smithy surface involved is eks#CreateCluster with accessConfig.authenticationMode='API' and bootstrapClusterCreatorAdminPermissions=true, plus sts#GetCallerIdentity for the caller's ARN/account). POST /apis/authentication.k8s.io/v1/selfsubjectreviews with body {"apiVersion":"authentication.k8s.io/v1","kind":"SelfSubjectReview"}, authenticated with an STS-presigned-URL bearer token for the cluster creator, is answered HTTP 201 whose status.userInfo describes the IAM principal in Kubernetes' own terms as aws-iam-authenticator maps it: `username` is the IAM principal ARN verbatim (the same string EKS reports as the access entry's `username`), `uid` is the fixed prefix `aws-iam-authenticator:` followed by the account id and the IAM principal id — i.e. `aws-iam-authenticator:<account-id>:<principal-id>`, where both variable parts come from the request's credentials and the account's own state — `groups` contains `system:authenticated`, and `extra` carries arn=[<caller arn>], canonicalArn=[<caller arn>], principalId=[<principal id>]. The uid is not an opaque blob: its last segment is exactly extra.principalId[0], so the two fields agree.
Emulator: the POST is answered 201 and status.userInfo.username is the caller ARN (that assertion passed), but the object has NO `uid` key at all — reading it raises KeyError: 'uid'. Everything downstream of the identity review in this scenario (the groups/extra assertions, the uid<->principalId agreement, and the namespace create/replay/read-back and the 401-for-unauthenticated legs that follow) is therefore unreachable.
Why: userInfo.uid is the identity a Kubernetes cluster uses to distinguish two principals that happen to share a username and to key audit and RBAC bindings; for EKS it is also the only place the IAM principal id surfaces through the Kubernetes boundary. A client that resolves "who am I in this cluster" through SelfSubjectReview (kubectl auth whoami, admission webhooks, audit correlation, any controller that logs or authorizes on the subject uid) gets a response with a required field absent — it either crashes on the missing key, as the check does, or silently treats the caller as having no identity, so the aws-iam-authenticator mapping from the IAM ARN to a Kubernetes subject can never be verified.
Fix: the served cluster Kubernetes API's authentication/identity component in the eks service crate — the SelfSubjectReview (authentication.k8s.io/v1) responder must project the authenticated IAM principal into a complete UserInfo: username = the principal ARN, uid = 'aws-iam-authenticator:<account-id>:<principal-id>' derived from the credentials that authenticated the request, groups including 'system:authenticated', and extra arn/canonicalArn/principalId, with uid's last segment equal to extra.principalId[0].
Related open issue, which does NOT carry this check's replay key and covers a different field: 20261117T115837Z-eks-iam-sts-the-served-cluster-kubernetes-api.md (created objects omit metadata.resourceVersion).
Traces: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. This failure is reported owned by no open issue and blocked by no unimplemented stub.

eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token [error]:
  File check_eks__iam__sts_eks_kubernetes_api_namespace_via_sts_bearer_token.py, line 177, in run
    assert user_info["uid"].startswith(f"aws-iam-authenticator:{account_id}:"), user_info
  KeyError: 'uid'

The preceding assertions passed, so: the cluster reached ACTIVE, the bootstrap access entry for the creating principal exists with type 'STANDARD' and username == the caller ARN, AmazonEKSClusterAdminPolicy is associated, the STS-presigned bearer token authenticated, and POST /apis/authentication.k8s.io/v1/selfsubjectreviews was answered 201 with status.userInfo.username == the caller ARN. Only status.userInfo.uid is absent.

Expected (AWS): 201 with status.userInfo = {username: '<caller arn>', uid: 'aws-iam-authenticator:<account-id>:<principal-id>', groups: [... 'system:authenticated' ...], extra: {arn: ['<caller arn>'], canonicalArn: ['<caller arn>'], principalId: ['<principal-id>']}} and uid == 'aws-iam-authenticator:' + account-id + ':' + extra.principalId[0]. The account id, principal id and ARN are values of the request's own credentials and the emulator's own state, not per-run noise outside its reach.

## Steps to Reproduce
1. sts.get_caller_identity() to capture the caller ARN and account id.
2. eks.create_cluster(... accessConfig={'authenticationMode':'API','bootstrapClusterCreatorAdminPermissions':True}, resourcesVpcConfig with the EKS subnets ...) and wait for status ACTIVE; confirm eks.list_access_entries / describe_access_entry report an entry for the caller ARN with type 'STANDARD' and username == the caller ARN.
3. Build an STS GetCallerIdentity presigned URL for the cluster (the aws-iam-authenticator token) and use it as the Bearer token against the cluster's published Kubernetes endpoint.
4. POST /apis/authentication.k8s.io/v1/selfsubjectreviews with {"apiVersion":"authentication.k8s.io/v1","kind":"SelfSubjectReview"}. Expect 201.
5. Read status.userInfo of the response body: 'uid' is absent (expected 'aws-iam-authenticator:<account-id>:<principal-id>'). Also check groups contains 'system:authenticated', extra.arn == [caller arn], extra.canonicalArn == [caller arn], and uid == 'aws-iam-authenticator:<account-id>:' + extra.principalId[0].
6. With the same token, POST /api/v1/namespaces for a fresh namespace (expect 201 with metadata.uid, labels['kubernetes.io/metadata.name'], spec.finalizers ['kubernetes'], status.phase 'Active'), replay it (expect 409 AlreadyExists) and confirm an unauthenticated GET /version is refused 401.
