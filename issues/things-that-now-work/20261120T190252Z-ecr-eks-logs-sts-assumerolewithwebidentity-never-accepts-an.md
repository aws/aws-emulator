Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret
Issue-Key: sts|AssumeRoleWithWebIdentity|InvalidIdentityToken
Issue-Commit: e62e0ae23225a1c6c2c24fb08c0dcaf4852934da

# ecr__eks__logs: STS AssumeRoleWithWebIdentity never accepts an EKS-issued service-account token, so IRSA credentials can never be obtained (1 cause)

> _Note: this issue was identified against the code base at commit `e62e0ae23225a1c6c2c24fb08c0dcaf4852934da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleWithWebIdentity

## Description
Cause 1 of 1: STS AssumeRoleWithWebIdentity refuses a projected Kubernetes service-account token minted by the emulator's own EKS cluster, even after the cluster's OIDC issuer has been registered with IAM CreateOpenIDConnectProvider and the target role's trust policy federates that provider. The refusal persists for the whole exchange window, so an IRSA (IAM Roles for Service Accounts) identity can never be assumed.

AWS: AssumeRoleWithWebIdentity is the IRSA exchange. Given a RoleArn whose trust policy has Effect Allow, Principal.Federated = the OIDC provider ARN created for the cluster's identity.oidc.issuer, Action sts:AssumeRoleWithWebIdentity and Conditions on <issuer-host-path>:sub / <issuer-host-path>:aud that the presented token satisfies, AWS answers HTTP 200 with Credentials (AccessKeyId / SecretAccessKey / SessionToken), AssumedRoleUser, SubjectFromWebIdentityToken, Provider and Audience. Smithy (sts, AssumeRoleWithWebIdentity): required members RoleArn, RoleSessionName, WebIdentityToken (sensitive, @length 4..20000); output member Credentials; the only modeled refusals are ExpiredTokenException (400), IDPCommunicationErrorException (400 IDPCommunicationError), IDPRejectedClaimException (403 IDPRejectedClaim), InvalidIdentityTokenException (400 InvalidIdentityToken), MalformedPolicyDocumentException (400), PackedPolicyTooLargeException (400) and RegionDisabledException (403); the operation is optionalAuth (anonymous). None of those classes applies to a freshly minted, unexpired, correctly-audienced (aud = sts.amazonaws.com) token from a registered provider, so AWS does not refuse this call. AWS's propagation delay for a just-created OIDC provider or trust policy is seconds, which is why the check retries only those two transient classes.

Emulator: every attempt in the retry window is refused with one of the two transient-looking refusals and never succeeds, so no Credentials are ever returned. The refusals observed are: InvalidIdentityToken (HTTP 400) whose message contains AWS's fixed text "No OpenIDConnect provider found in your account for " followed by the issuer URL taken from the request's token (variable part = the cluster's identity.oidc.issuer value), i.e. the emulator does not join the token's issuer claim to the provider IAM just registered; and/or AccessDenied naming sts:AssumeRoleWithWebIdentity (fixed text "Not authorized to perform sts:AssumeRoleWithWebIdentity"; the variable parts in AWS's wording are the role ARN and the federated identity), i.e. the trust policy's Federated principal plus :sub/:aud conditions are not evaluated as satisfied. Because the call is never accepted, the check exhausts its window and reports: TimeoutError: STS accepting the web identity for arn:aws:iam::<account>:role/<irsa-role-name>: the run budget ended first; not graded (the role ARN is a per-run value, not part of the requirement).

Why: a client relying on AWS's behavior gets no IRSA credentials at all. Every workload pattern that authenticates a pod to AWS through its service account — the standard way an EKS pod reaches ECR, S3, DynamoDB or any other API — is impossible against the emulator: the AWS SDK's web-identity credential provider fails hard and the caller sees AccessDenied/InvalidIdentityToken instead of a session. Code that only exercises the happy path of 'annotate the SA with eks.amazonaws.com/role-arn and let the SDK exchange the projected token' cannot run.

Fix: the STS AssumeRoleWithWebIdentity handler and its web-identity verification path — it must (a) resolve the token's issuer claim against the OIDC providers registered through IAM CreateOpenIDConnectProvider (matching on the bare issuer host/path, scheme-insensitively, as the trust-policy condition keys are written) and accept tokens the emulator's own EKS TokenRequest endpoint minted for that cluster, and (b) evaluate the role's trust policy Federated principal together with the <issuer-host-path>:sub and <issuer-host-path>:aud conditions against the token's claims, then mint session credentials. Related but not the same defect: open issue 20261111T111216Z-cloudformation-eks-eks-publishes-an-oidc-issuer-url-on-a.md (EKS publishes an OIDC issuer URL on a real AWS host instead of an address the emulator serves) — that issue describes the issuer value, not STS's refusal to exchange a token for it; neither it nor 20261117T163844Z-eks-cluster-plane-responses-diverge-default-serviceipv4cidr-oidc.md covers this exchange.

Traces: ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret — the cluster reached ACTIVE, the nodegroup reached ACTIVE, IAM CreateOpenIDConnectProvider for identity.oidc.issuer succeeded, the ServiceAccount/Pod were created and a 'Pulled' event observed, a TokenRequest for audience sts.amazonaws.com returned a token, and only then did STS refuse the exchange for the whole window.

## Evidence
Expected (AWS): AssumeRoleWithWebIdentity(RoleArn=<IRSA role>, RoleSessionName=sim2real-irsa-pod, WebIdentityToken=<projected SA token, aud=sts.amazonaws.com>) -> HTTP 200 with Credentials{AccessKeyId, SecretAccessKey, SessionToken}, AssumedRoleUser, SubjectFromWebIdentityToken=system:serviceaccount:default:<sa-name>, Provider=<issuer host/path>, Audience=sts.amazonaws.com; those credentials then authorize ecr:DescribeImages via AmazonEC2ContainerRegistryReadOnly.

Actual (emulator): the exchange is never accepted. Each attempt is refused with either 400 InvalidIdentityToken, message "No OpenIDConnect provider found in your account for <issuer URL>" (fixed text up to the URL; URL is the cluster's identity.oidc.issuer), or AccessDenied whose message names sts:AssumeRoleWithWebIdentity ("Not authorized to perform sts:AssumeRoleWithWebIdentity"). The check's retry loop therefore never exits successfully and ends with:

  File "check_ecr__eks__logs_eks_irsa_role_pulls_ecr_image_without_imagepullsecret.py", line 214, in run
    creds = _assume_role_with_web_identity(
  File "...", line 297, in _assume_role_with_web_identity
    raise TimeoutError(
  TimeoutError: STS accepting the web identity for arn:aws:iam::000000000000:role/sim2real-irsa-podrole-<suffix>: the run budget ended first; not graded

(The account id, role-name suffix and session name above are per-run values; the requirement is the 200-with-Credentials outcome and the absence of either refusal.)

## Steps to Reproduce
1. ecr CreateRepository and push an image tag (used later to prove the credentials work).
2. iam CreateRole for the cluster (trusts eks.amazonaws.com, AmazonEKSClusterPolicy) and for the nodes (trusts ec2.amazonaws.com, AmazonEKSWorkerNodePolicy + AmazonEKS_CNI_Policy + AmazonEC2ContainerRegistryReadOnly).
3. eks CreateCluster, wait ACTIVE, read identity.oidc.issuer; eks CreateNodegroup, wait ACTIVE.
4. iam CreateOpenIDConnectProvider(Url=<identity.oidc.issuer>, ClientIDList=["sts.amazonaws.com"]).
5. iam CreateRole for the pod with trust: Principal.Federated=<that provider ARN>, Action sts:AssumeRoleWithWebIdentity, Condition StringEquals {"<issuer-host-path>:sub": "system:serviceaccount:default:<sa>", "<issuer-host-path>:aud": "sts.amazonaws.com"}; attach AmazonEC2ContainerRegistryReadOnly.
6. Against the served cluster Kubernetes API: create the ServiceAccount annotated eks.amazonaws.com/role-arn=<pod role ARN>, create a Pod using it, then POST its TokenRequest with audiences=["sts.amazonaws.com"], expirationSeconds=600 and take status.token.
7. Call sts AssumeRoleWithWebIdentity(RoleArn=<pod role ARN>, RoleSessionName="sim2real-irsa-pod", WebIdentityToken=<that token>). Observed: refused (400 InvalidIdentityToken "No OpenIDConnect provider found in your account for <issuer>" / AccessDenied naming sts:AssumeRoleWithWebIdentity) on every attempt; expected 200 with Credentials, which should then succeed at ecr DescribeImages(repositoryName=<repo>, imageIds=[{imageTag: <tag>}]) returning exactly one imageDetails entry.
