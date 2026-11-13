Issue-Source: trace-sweep-divergence
Issue-Trace: sts-assume-role-with-saml-invalid-assertion
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-assume-role-with-saml-invalid-assertion
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|AssumeRoleWithSAML|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 6c6b3e93b46253302160cd3817152950aa37dc77

# sts AssumeRoleWithSAML missing-provider refusal drops the relayed AWSOpenIdDiscoveryService envelope from its Error.Message

> _Note: this issue was identified against the code base at commit `6c6b3e93b46253302160cd3817152950aa37dc77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleWithSAML

## Description
AssumeRoleWithSAML, called with a PrincipalArn that names a SAML provider that does not exist in the account, is refused with the correct HTTP status and the correct error code, but the emulator serves only the bare human sentence as Error.Message and omits the relayed downstream-service envelope that AWS appends to this particular refusal. 1 cause, on service sts.

Cause 1 of 1: the missing-SAML-provider refusal's Error.Message is truncated to its leading sentence — the trailing '(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: ...)' envelope is not emitted.

AWS: recorded behavior only for the message text — Smithy (com.amazonaws.sts#AWSSecurityTokenServiceV20110615, AssumeRoleWithSAML) models the refusal shapes (InvalidIdentityTokenException awsQueryError code 'InvalidIdentityToken' / httpError 400, IDPRejectedClaimException 'IDPRejectedClaim' / 403, ExpiredTokenException 400, MalformedPolicyDocument 400, PackedPolicyTooLarge 400, RegionDisabledException 403) and declares PrincipalArn as a required arnType (@length 20-2048), but it states no message text. The recording shows that STS does not author this message itself: it validates the SAML provider by calling an internal discovery service and RELAYS that service's SDK error envelope verbatim inside its own Error.Message. The recorded response is HTTP 400 and its Error.Message is the leading sentence "Specified provider doesn't exist" followed by a space and a parenthesized envelope of six semicolon-separated 'Key: Value' fields in a fixed order: 'Service', 'Status Code', 'Error Code', 'Request ID', 'Proxy'. Which parts are fixed text and which are values: the sentence "Specified provider doesn't exist", the parenthesis/semicolon/'Key: ' punctuation, and the field names are FIXED; the field values are the relayed call's own — 'Service' is the relayed service's name (the recording shows the SAML/OIDC provider-discovery service), 'Status Code' is that call's HTTP status, 'Error Code' is that call's exception name (the recording shows the SAML-manifest-not-found exception), 'Request ID' is the relayed call's request id and is PER-RUN, and 'Proxy' is 'null' when no proxy was used. The requirement is the sentence plus a populated envelope of those five fields in that order; the specific request id in the recording is a per-run value and is NOT part of the contract.

Emulator: the response status (400) and the error code both match, but Error.Message is exactly "Specified provider doesn't exist" with no trailing space and no parenthesized envelope at all — the five relayed fields are absent, not merely different.

Why: AWS SDK and CLI users (and log scrapers, retry classifiers and support tooling) parse that parenthesized envelope to learn WHICH downstream call failed and with WHICH exception — here, that the failure is a SAML-manifest lookup miss rather than a rejected assertion, a malformed policy or an expired token. Against the emulator all of that attribution is gone, so a client that branches on the relayed 'Error Code' field, or an assertion that the message carries the envelope, sees a message shape AWS never serves and either mis-routes the failure or fails outright. A test asserting the full AWS message for this refusal cannot pass.

Fix: the sts crate's AssumeRoleWithSAML handler and the error serializer it uses for the provider-lookup failure — compose Error.Message as the fixed sentence plus the relayed envelope (Service / Status Code / Error Code / Request ID / Proxy, in that order, with a per-request request id) instead of emitting the bare sentence.

Traces: sts-assume-role-with-saml-invalid-assertion#task#1.

Not covered by any open issue: the open sts issues ('STS vended credentials and policy-size members diverge...' 20261029T191504Z, 'sts: 4 net-new behavioral root causes...' 20261113T093925Z, 'sts GetWebIdentityToken serves 501 NotImplemented...' 20261113T034059Z, 'sts: a GetFederationToken session-policy refusal is attributed...' 20261113T073604Z) concern credential vending, GetFederationToken session policies, GetWebIdentityToken and MFA/aws:userid behavior; none carries this replay key and none touches the AssumeRoleWithSAML provider-lookup message.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, score_pct=65.9).

Cluster service=sts|AssumeRoleWithSAML|status=400->400|keys=$.ErrorResponse.Error.Message, fixture sts-assume-role-with-saml-invalid-assertion step task#1, verdict=mismatch, affected_count=1, owned_by empty (not covered by any filed issue), stub_path empty (operation is implemented).

Expected (recorded) HTTP 400, $.ErrorResponse.Error.Message:
  "Specified provider doesn't exist (Service: AWSOpenIdDiscoveryService; Status Code: 400; Error Code: AuthSamlManifestNotFoundException; Request ID: 6ef5f51d-669e-468b-a923-f1d1f4ac260d; Proxy: null)"

Actual (emulator) HTTP 400, $.ErrorResponse.Error.Message:
  "Specified provider doesn't exist"

Status (400 -> 400) and $.ErrorResponse.Error.Code agree; the ONLY divergence is the dropped envelope. In the expected message the fixed text is the sentence "Specified provider doesn't exist", the ' (' / '; ' / ': ' punctuation and the field names 'Service', 'Status Code', 'Error Code', 'Request ID', 'Proxy'; the values are relayed from the provider-discovery call ('AWSOpenIdDiscoveryService', '400', 'AuthSamlManifestNotFoundException', 'null'). The request id '6ef5f51d-669e-468b-a923-f1d1f4ac260d' is a per-run value and is not asserted as a literal — the defect is that the whole envelope, including the field names and the other four values, is missing.

Model evidence: smithy_query(operation="AssumeRoleWithSAML") on services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json — PrincipalArn required (arnType, @length 20-2048); modeled errors InvalidIdentityTokenException (400, awsQueryError InvalidIdentityToken), IDPRejectedClaimException (403, IDPRejectedClaim), ExpiredTokenException (400), MalformedPolicyDocumentException (400), PackedPolicyTooLargeException (400), RegionDisabledException (403). No message text is modeled, hence 'recorded behavior only' for the wording.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the sts divergence clusters.
3. Cause 1: replay fixture sts-assume-role-with-saml-invalid-assertion step task#1 — call AssumeRoleWithSAML with a PrincipalArn naming a SAML provider that does not exist in the account (plus any RoleArn and SAMLAssertion satisfying the modeled length constraints).
4. Read $.ErrorResponse.Error.Message on the 400 response. The emulator returns the bare sentence "Specified provider doesn't exist"; AWS returns that sentence followed by a space and the relayed envelope '(Service: <discovery service>; Status Code: <relayed status>; Error Code: <relayed exception>; Request ID: <per-run id>; Proxy: null)'.
5. The step must pass — the message must carry the sentence plus all five envelope fields in order, with the request id generated per request — before this issue is closed.
