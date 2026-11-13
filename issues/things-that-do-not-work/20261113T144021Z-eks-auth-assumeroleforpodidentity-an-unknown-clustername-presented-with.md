Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks-auth.pod-identity-native-round-trip-success eks-auth.pod-identity-wrong-audience-token-rejected
Issue-Shape-Id: com.amazonaws.eksauth#EKSAuthFrontend
Issue-Key: eks-auth.pod-identity-native-round-trip-success|run#21|AssertionError|assert code == "ResourceNotFoundException", f"GOT={code!r}"
Issue-Key: eks-auth.pod-identity-wrong-audience-token-rejected|run#8|AssertionError|assert message == _WRONG_AUDIENCE_MESSAGE, (
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# eks-auth AssumeRoleForPodIdentity: an unknown clusterName presented with a genuine cluster-issued token is 400 InvalidTokenException instead of 404 ResourceNotFoundException, and the wrong-audience rejection is not worded as AWS words it (2 causes)

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleForPodIdentity

## Description
Two net-new root causes in service eks-auth, on the only operation it binds (AssumeRoleForPodIdentity, POST /clusters/{clusterName}/assume-role-for-pod-identity, restJson1). Both are on the operation's rejection path and both are reached only now that the pod-identity data plane works end to end (the positive round trip in check pod-identity-native-round-trip-success passes every assertion up to the one below).

RELATION TO EXISTING ISSUES: the open issue 20261107T170624Z-eks-auth-assumeroleforpodidentity-never-evaluates-the-caller-s.md covers the missing IAM authorization step only; the resolved issue 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md covers token-parse-before-cluster-lookup ordering for a token that is NOT genuine cluster key material. Neither covers either cause below (Cause 1 is specifically a GENUINE, cluster-issued token presented against a clusterName that names no cluster; Cause 2 is the wording of a claim-level token rejection), and the behavioral replay marked neither check as ALREADY FILED or OWNED.

Cause 1 of 2: a clusterName that names no cluster is never reported as not found — it is reported as a bad token header, so the modeled ResourceNotFoundException (404) is unreachable for a caller holding genuine key material.
AWS: Smithy (services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json) models on AssumeRoleForPodIdentity both ResourceNotFoundException (httpError 404, doc "The specified resource could not be found.") and InvalidTokenException (httpError 400, doc "The specified Kubernetes service account token is invalid."); clusterName is an @httpLabel, @required, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$ (so it is case-SENSITIVE: a case-flipped spelling of an existing cluster is a different, non-existent cluster name). Recorded behavior: when the presented token IS a service account token the cluster genuinely issued for audience pods.eks.amazonaws.com, and the same token is then replayed against a clusterName that resolves to no cluster, AWS answers the resource failure — HTTP 404, Error.Code = ResourceNotFoundException — rather than a token failure. In that message the fixed text is AWS's not-found sentence and the only variable part is the cluster name taken from the request URI; the requirements this check asserts are the status (404) and the code (ResourceNotFoundException).
Emulator: the request is answered HTTP 400 / InvalidTokenException. services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs resolves the cluster as `guard.cluster(&input.cluster_name).filter(|cluster| cluster.publishes_signing_key(&token_headers.key_id)).ok_or_else(|| malformed_token(TokenMalformation::InvalidHeader(KEY_ID_HEADER)))` — a single lookup that collapses "this cluster does not exist" and "this cluster publishes no such kid" into one answer, and the file's own module doc states the intent: "a cluster that is not registered publishes no keys ... and the modelled ResourceNotFoundException (404) is reachable by no request". So no request can ever obtain the modeled 404 for an unknown cluster, however good its token.
Why: a client cannot distinguish "I am pointed at the wrong/deleted cluster" (retry is pointless, 404, non-retryable resource error) from "my projected service account token is stale" (re-read the token file and retry, 400). The EKS Pod Identity Agent keys its remediation on exactly that distinction, and 404 vs 400 also changes SDK retry/error classification; an emulator-backed test that asserts a mis-addressed cluster is refused with ResourceNotFoundException can never pass.
Fix: the cluster-resolution step of the operation's handler (the `guard.cluster(...).filter(...)` lookup in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs) must split the two questions: resolve the clusterName label first and raise ResourceNotFoundException (404) when it names no cluster, and only then judge the token's kid against that cluster's published signing keys (InvalidTokenException, 400). The existing ordering guarantee for tokens that are not JWTs at all (the parse step ahead of any state read) stays as it is.
Traces: eks-auth.pod-identity-native-round-trip-success.

Cause 2 of 2: a cluster-issued token minted for the wrong audience is refused with an emulator-authored sentence instead of AWS's claim-specific aud message.
AWS: Smithy models InvalidTokenException (httpError 400, doc "The specified Kubernetes service account token is invalid."); the output member `audience` is documented as "The identity that is allowed to use the credentials. This value is always pods.eks.amazonaws.com." The message wording is recorded behavior only: a validly-signed token the cluster issued for audience sts.amazonaws.com is refused with HTTP 400 / InvalidTokenException and the message "The token included in the request has an invalid claim value for aud. Expected pods.eks.amazonaws.com." Its fixed text is "The token included in the request has an invalid claim value for <claim>. Expected <expected value>." — the claim name comes from the claim that failed (here aud) and the expected value is the pod-identity audience the service requires (pods.eks.amazonaws.com, constant per the model's own doc); nothing in it is derived from the rejected token, which is @sensitive.
Emulator: same status and code, but the message is "The service account token is invalid: it must be a token the cluster issued for the audience pods.eks.amazonaws.com." — one catch-all sentence for every cluster-side verdict, produced by `token_rejection(TokenRejection::Invalid)` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs. The verification path reports a single Invalid rejection, so a wrong `aud` is not distinguished from any other claim failure and no claim name appears in the message.
Why: an error message is part of the contract. Operators and the Pod Identity Agent's logs identify a misconfigured `audiences:` in a projected-token volume from AWS's "invalid claim value for aud" wording; a client (or support runbook, or log-scraping assertion) matching that text sees no match, and the generic sentence names no claim, so the actual misconfiguration is not diagnosable from the refusal.
Fix: the token-rejection wording of this operation — `token_rejection` / the `TokenRejection` enum in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs — must carry which claim the cluster's verification rejected and render AWS's claim-specific sentence for it, keeping the status 400 and the code InvalidTokenException and echoing nothing of the @sensitive token.
Traces: eks-auth.pod-identity-wrong-audience-token-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 20/22 passed (90.9%); eks-auth 19/21 (2 failed), eks__eks-auth 1/1. Neither failure is marked ALREADY FILED or OWNED, and both are the two causes above.

Cause 1 — [mismatch] eks-auth.pod-identity-native-round-trip-success:
  File "check_eks_auth_pod_identity_native_round_trip_success.py", line 88, in run
    assert code == "ResourceNotFoundException", f"GOT={code!r}"
  AssertionError: GOT='InvalidTokenException'
The check first completes the full positive round trip against the emulator (TokenRequest for a pod-bound service account token, AssumeRoleForPodIdentity returning subject/audience/podIdentityAssociation/assumedRoleUser/credentials, and a borrowed-credential sts:GetCallerIdentity whose Arn matches assumedRoleUser.arn) — all of those assertions passed. It then replays the SAME genuine token against `clusterName.swapcase()`, a pattern-valid name that resolves to no cluster.
Expected (AWS): HTTP 404, Error.Code = ResourceNotFoundException (fixed not-found sentence; only the cluster name is request-derived).
Actual (emulator): HTTP 400, Error.Code = InvalidTokenException.
Code grounding: services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs — module doc lines 20-26 ("a cluster that is not registered publishes no keys ... the modelled ResourceNotFoundException (404) is reachable by no request") and the lookup at lines 271-277 (`guard.cluster(&input.cluster_name).filter(|cluster| cluster.publishes_signing_key(&token_headers.key_id)).ok_or_else(|| ... malformed_token(TokenMalformation::InvalidHeader(KEY_ID_HEADER)))`). In the handler, ResourceNotFoundException is constructed only by `no_association()` (lines ~499-515), i.e. for a missing pod-identity association, never for an unknown cluster.

Cause 2 — [mismatch] eks-auth.pod-identity-wrong-audience-token-rejected:
  File "check_eks_auth_pod_identity_wrong_audience_token_rejected.py", line 40, in run
    assert message == _WRONG_AUDIENCE_MESSAGE
  AssertionError: EXPECTED_MESSAGE='The token included in the request has an invalid claim value for aud. Expected pods.eks.amazonaws.com.' GOT='The service account token is invalid: it must be a token the cluster issued for the audience pods.eks.amazonaws.com.'
The error CODE matched (InvalidTokenException, 400) and the check's positive control passed — the same pod, same identity, audience pods.eks.amazonaws.com, assumes the role successfully — so audience is the only difference between the accepted and the refused token, and only the message diverges.
Expected (AWS): HTTP 400 / InvalidTokenException, message "The token included in the request has an invalid claim value for aud. Expected pods.eks.amazonaws.com." (fixed text "The token included in the request has an invalid claim value for <claim>. Expected <expected value>."; <claim>=the failing claim, <expected value>=the required pod-identity audience).
Actual (emulator): HTTP 400 / InvalidTokenException, message "The service account token is invalid: it must be a token the cluster issued for the audience pods.eks.amazonaws.com."
Code grounding: services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs lines ~486-494 (`TokenRejection::Invalid => InvalidTokenException { message: "The service account token is invalid: it must be a token the cluster issued for the audience {PODS_AUDIENCE}." }`).

Model grounding (both causes): AssumeRoleForPodIdentity errors = AccessDeniedException(400), ExpiredTokenException(400), InternalServerException(500), InvalidParameterException(400), InvalidRequestException(400), InvalidTokenException(400), ResourceNotFoundException(404), ServiceUnavailableException(503), ThrottlingException(429); input clusterName @httpLabel @length 1..100 @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$, token @required @sensitive @pattern ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Create an EKS cluster, wait for ACTIVE, and create a pod identity association for (namespace=default, serviceAccount=default) pointing at an IAM role trusted by pods.eks.amazonaws.com.
3. Through the cluster's Kubernetes API, create a Pod bound to that service account and mint a projected service account token via the TokenRequest API with audiences=["pods.eks.amazonaws.com"] and boundObjectRef naming that Pod.
4. Confirm the positive path: eks-auth AssumeRoleForPodIdentity(clusterName=<cluster>, token=<that token>) returns 200 with subject/audience/podIdentityAssociation/assumedRoleUser/credentials, and sts:GetCallerIdentity with the returned credentials reports the same assumed-role ARN.
5. Cause 1: replay the SAME token with clusterName=<cluster>.swapcase() (a pattern-valid name that exists on no cluster).
   Observe: HTTP 400 / InvalidTokenException.
   AWS: HTTP 404 / ResourceNotFoundException — the unknown cluster name is reported as not found, not as a bad token header.
6. Cause 2: mint a second token for the same pod and service account with audiences=["sts.amazonaws.com"], and call AssumeRoleForPodIdentity(clusterName=<cluster>, token=<that token>).
   Observe: HTTP 400 / InvalidTokenException with message "The service account token is invalid: it must be a token the cluster issued for the audience pods.eks.amazonaws.com."
   AWS: HTTP 400 / InvalidTokenException with message "The token included in the request has an invalid claim value for aud. Expected pods.eks.amazonaws.com."
   Repeat the call: the identical rejection (code and message) must be returned again, with no caching or partial acceptance.
7. Re-replay eks-auth.pod-identity-native-round-trip-success and eks-auth.pod-identity-wrong-audience-token-rejected; both must pass before closing.
