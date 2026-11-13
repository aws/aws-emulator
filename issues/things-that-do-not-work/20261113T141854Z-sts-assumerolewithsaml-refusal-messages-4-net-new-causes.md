Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-with-saml-non-base64-assertion-rejected sts.assume-role-with-saml-forged-assertion-rejected sts.assume-role-with-saml-idp-rejected-claim-denied sts.assume-role-with-saml-provider-deleted-blocked
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-with-saml-non-base64-assertion-rejected|run#11|AssertionError|assert (code, _stable(msg)) == ("InvalidIdentityToken", _NON_BASE64_MESSAGE), (
Issue-Key: sts.assume-role-with-saml-forged-assertion-rejected|run#12|AssertionError|assert _stable(refusal) == ("InvalidIdentityToken", _UNSIGNED_MESSAGE), (
Issue-Key: sts.assume-role-with-saml-idp-rejected-claim-denied|run#11|AssertionError|assert refusal == ("InvalidIdentityToken", _WRONG_ROLE_MESSAGE), f"got {refusal!r}"
Issue-Key: sts.assume-role-with-saml-provider-deleted-blocked|run#34|AssertionError|assert (code, stable) == ("InvalidIdentityToken", _DELETED_PROVIDER_MESSAGE), (
Issue-Commit: 5164801c346b7b34f471999eb845176f7f045b56

# sts AssumeRoleWithSAML refusal messages: 4 net-new causes — the relayed AWSOpenIdDiscoveryService envelope is dropped from InvalidIdentityToken messages, and two refusal sentences are emulator-authored wording

> _Note: this issue was identified against the code base at commit `5164801c346b7b34f471999eb845176f7f045b56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleWithSAML

## Description
AssumeRoleWithSAML refuses every malformed/untrusted SAML assertion with the right error class (InvalidIdentityToken) and the right HTTP status (400), but the Error.Message it serves is not the message AWS serves for any of four distinct refusal classes. In two classes the fixed sentence is correct and the trailing relayed envelope is missing; in two more the sentence itself is emulator-authored wording. 4 causes, all on service sts, all in the same handler/error-serializer path.

Smithy evidence used for all four causes: smithy_query(operation="AssumeRoleWithSAML") on services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json — RoleArn, PrincipalArn (arnType @length 20-2048) and SAMLAssertion (SAMLAssertionType @length 4-100000, @sensitive) are required; modeled errors are InvalidIdentityTokenException (httpError 400, awsQueryError code InvalidIdentityToken), IDPRejectedClaimException (403, IDPRejectedClaim), ExpiredTokenException (400), MalformedPolicyDocumentException (400), PackedPolicyTooLargeException (400), RegionDisabledException (403). The model states NO message text for any of them, so each cause's wording below is 'recorded behavior only' — taken from what the recorded AWS response carries, with its variable parts named.

The envelope AWS appends in causes 1, 2 and 4 is a single trailing ' (' + five semicolon-separated 'Key: Value' fields + ')' in fixed order: Service, Status Code, Error Code, Request ID, Proxy. FIXED text: the punctuation and the five field names. VALUES: 'Service' is the relayed internal discovery service (recorded as AWSOpenIdDiscoveryService), 'Status Code' that relayed call's HTTP status (recorded 400), 'Error Code' that call's exception name, 'Request ID' a PER-RUN uuid (never a literal requirement), 'Proxy' is 'null' when no proxy was used.

Cause 1 of 4: a SAMLAssertion that is not valid base64 is refused with the right sentence but no relayed envelope.
AWS: recorded behavior only — HTTP 400, Error.Code 'InvalidIdentityToken', Error.Message = the fixed sentence "Invalid base64 SAMLResponse" followed by the envelope with Error Code 'AuthSamlInvalidSamlResponseException' and a per-run Request ID.
Emulator: HTTP 400 and code 'InvalidIdentityToken' agree; Error.Message is exactly "Invalid base64 SAMLResponse" — the whole parenthesized envelope (field names and all five values) is absent, not merely different.
Why: SDK/CLI users, log scrapers and support tooling parse that envelope to learn WHICH downstream validation rejected the assertion (a decode failure vs a signature failure vs a manifest miss). Against the emulator all that attribution is gone and any client or test asserting the AWS message shape fails.
Fix: the sts AssumeRoleWithSAML handler's assertion-decode validation and the error serializer that composes Error.Message for it.
Traces: sts.assume-role-with-saml-non-base64-assertion-rejected.

Cause 2 of 4: an assertion whose <ds:Signature> is removed entirely is refused with an emulator-authored sentence and no relayed envelope.
AWS: recorded behavior only — HTTP 400, Error.Code 'InvalidIdentityToken', Error.Message = the fixed sentence "Response (with optional signature) must contain an assertion with a mandatory signature" followed by the envelope with Error Code 'AuthSamlInvalidSamlResponseException' and a per-run Request ID. The refusal is stable: an identical retry of the same unsigned assertion yields the same code and message.
Emulator: HTTP 400, code 'InvalidIdentityToken', Error.Message "Response is not signed" — a different fixed sentence AND no envelope.
Why: a client cannot distinguish 'the assertion carries no mandatory signature' from any other token rejection, and the documented AWS sentence a test or a diagnostic matches on never appears.
Fix: the sts AssumeRoleWithSAML handler's assertion-signature validation and its message composition (same serializer as cause 1).
Traces: sts.assume-role-with-saml-forged-assertion-rejected.

Cause 3 of 4: a correctly signed assertion whose Role claim names a role other than the requested RoleArn is refused with an emulator-authored sentence.
AWS: recorded behavior only — HTTP 400 with Error.Code 'InvalidIdentityToken' (NOT IDPRejectedClaim) and Error.Message exactly "SAML Assertion doesn't contain the requested Role and Metadata in the attributes". This message is entirely fixed text — it carries no envelope and no request-derived value — and it is served on every retry, while the same assertion carrying the matching Role claim succeeds.
Emulator: HTTP 400 and code 'InvalidIdentityToken' agree; Error.Message is "Specified role could not be found in the SAML assertion", wording AWS never serves.
Why: callers keying on the message to tell a role/attribute-mapping mistake in the IdP from a signature or trust failure are misled, and the fixed AWS sentence is unmatchable.
Fix: the sts AssumeRoleWithSAML handler's Role/RoleSessionName claim-matching validation message.
Traces: sts.assume-role-with-saml-idp-rejected-claim-denied.

Cause 4 of 4: once the IAM SAML provider named by PrincipalArn is deleted, a previously accepted signed assertion is refused with the right sentence but no relayed envelope.
AWS: recorded behavior only — HTTP 400, Error.Code 'InvalidIdentityToken', Error.Message = the fixed sentence "Specified provider doesn't exist" followed by the envelope with Error Code 'AuthSamlManifestNotFoundException' and a per-run Request ID.
Emulator: HTTP 400, code 'InvalidIdentityToken', Error.Message is exactly "Specified provider doesn't exist" with no trailing envelope.
Why: the failure can no longer be attributed to a SAML-manifest lookup miss rather than a rejected assertion.
Fix: same handler/serializer path as causes 1 and 2, on the provider-lookup failure branch.
Traces: sts.assume-role-with-saml-provider-deleted-blocked. Cross-reference: open issue 20261113T125549Z-sts-assumerolewithsaml-missing-provider-refusal-drops-the-relayed.md describes this same dropped envelope from the WIRE corpus (replay key sts|AssumeRoleWithSAML|4xx->4xx|$.ErrorResponse.Error.Message); it does not carry this behavioral check id, and it covers only this one refusal class — causes 1, 2 and 3 above are not in it. Other open sts issues (20261025T222710Z expired-assertion ExpiredTokenException body, 20261029T184359Z IAM SAML/OIDC provider metadata members) are different causes.

## Evidence
Behavioral suite replay against the booted emulator (126/239 passed; emulator serving, bootstrap_signal false). None of these four checks is marked ALREADY FILED or OWNED in the report; they are listed under '### sts'.

Cause 1 — sts.assume-role-with-saml-non-base64-assertion-rejected:
  AssertionError: got 'InvalidIdentityToken': 'Invalid base64 SAMLResponse'
  Expected code/message: ('InvalidIdentityToken', "Invalid base64 SAMLResponse (Service: AWSOpenIdDiscoveryService; Status Code: 400; Error Code: AuthSamlInvalidSamlResponseException; Request ID: <request-id>; Proxy: null)") — the check itself normalizes the per-run uuid to <request-id>, so only the envelope's presence and its other four values are asserted.

Cause 2 — sts.assume-role-with-saml-forged-assertion-rejected:
  AssertionError: unexpected refusal for an unsigned SAML assertion: ('InvalidIdentityToken', 'Response is not signed')
  Expected: ('InvalidIdentityToken', "Response (with optional signature) must contain an assertion with a mandatory signature (Service: AWSOpenIdDiscoveryService; Status Code: 400; Error Code: AuthSamlInvalidSamlResponseException; Request ID: <request-id>; Proxy: null)"). The positive control (same claims, correctly signed) succeeded before and after, so only the refusal message diverges.

Cause 3 — sts.assume-role-with-saml-idp-rejected-claim-denied:
  AssertionError: got ('InvalidIdentityToken', 'Specified role could not be found in the SAML assertion')
  Expected: ('InvalidIdentityToken', "SAML Assertion doesn't contain the requested Role and Metadata in the attributes") — all fixed text, no per-run part.

Cause 4 — sts.assume-role-with-saml-provider-deleted-blocked:
  AssertionError: expected the specific 'provider doesn't exist' rejection, got 'InvalidIdentityToken' ("Specified provider doesn't exist")
  Expected: ('InvalidIdentityToken', "Specified provider doesn't exist (Service: AWSOpenIdDiscoveryService; Status Code: 400; Error Code: AuthSamlManifestNotFoundException; Request ID: <request-id>; Proxy: null)").

In every cause the HTTP status (400) and Error.Code agree with AWS; the divergence is confined to Error.Message.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the booted emulator.
2. Cause 1: create an IAM SAML provider and a role trusting it, then call AssumeRoleWithSAML with a SAMLAssertion that is not valid base64 (length within the modeled 4-100000) and read Error.Code/Error.Message off the 400 — replay check sts.assume-role-with-saml-non-base64-assertion-rejected.
3. Cause 2: build a signed SAMLResponse for that provider/role (positive control must succeed), then re-send the identical response with its <ds:Signature> element removed, twice, and read Error.Message — replay check sts.assume-role-with-saml-forged-assertion-rejected.
4. Cause 3: send a correctly signed assertion whose Role attribute names a different role than the requested RoleArn and read Error.Code/Error.Message — replay check sts.assume-role-with-saml-idp-rejected-claim-denied.
5. Cause 4: assume successfully with a signed assertion, delete the IAM SAML provider named by PrincipalArn, replay the same assertion and read Error.Message — replay check sts.assume-role-with-saml-provider-deleted-blocked.
6. All four checks must pass — each refusal must carry AWS's fixed sentence and, for causes 1, 2 and 4, the trailing '(Service: ...; Status Code: ...; Error Code: ...; Request ID: <per-request id>; Proxy: null)' envelope with a request id generated per request — before this issue is closed.
