Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-with-saml-expired-token-rejected
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-with-saml-expired-token-rejected|_try_assume_saml#8|ResponseParserError|return sts_client.assume_role_with_saml(
Issue-Commit: 8bf24a185681652e6c0ae734129bf03e6b79d2f1

# STS AssumeRoleWithSAML: an expired SAML assertion is answered with a JSON UnknownService body instead of ExpiredTokenException "Response has expired" — the anonymous awsQuery request is never routed to STS

> _Note: this issue was identified against the code base at commit `8bf24a185681652e6c0ae734129bf03e6b79d2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleWithSAML

## Description
ONE net-new root cause in the STS service (service token `sts`).

Cause 1 of 1: an anonymous (unsigned) AssumeRoleWithSAML request is not attributed to STS by the request router, so the expired-assertion refusal path is unreachable; the caller gets a JSON `UnknownService` body on an awsQuery operation and its XML error parser crashes.

Note on adjacency: the open issue 20261025T022928Z-sts-assumerolewithsaml-is-unroutable-unknownservice-assumerole-parses-the.md describes the same routing defect, but it does NOT carry this check — neither its Issue-Trace-Ids nor any of its Issue-Key lines mention `sts.assume-role-with-saml-expired-token-rejected`, and the suite did not mark this check OWNED. It is filed here so this check is replayed; a fixer should close it together with that issue's cause 1. It is not covered by any resolved issue.

AWS: Smithy (com.amazonaws.sts#AWSSecurityTokenServiceV20110615) models AssumeRoleWithSAML with `auth: []` and `optionalAuth: true` — it is an ANONYMOUS operation, so a conforming client sends it with NO Authorization header (therefore no SigV4 credential scope), and the awsQuery protocol sends no X-Amz-Target. Input: RoleArn (required, arnType, @length 20..2048), PrincipalArn (required, arnType, @length 20..2048), SAMLAssertion (required, SAMLAssertionType, @length 4..100000, @sensitive), plus optional Policy/PolicyArns/DurationSeconds. Modeled errors, each serialized as an awsQuery XML ErrorResponse: ExpiredTokenException (awsQueryError code `ExpiredTokenException`, HTTP 400), IDPRejectedClaimException (`IDPRejectedClaim`, 403), InvalidIdentityTokenException (`InvalidIdentityToken`, 400), MalformedPolicyDocumentException (`MalformedPolicyDocument`, 400), PackedPolicyTooLargeException (`PackedPolicyTooLarge`, 400), RegionDisabledException (`RegionDisabledException`, 403). On success it returns Credentials, AssumedRoleUser, Subject, SubjectType, Issuer, Audience, NameQualifier.
For the specific condition this check exercises — a correctly RSA-signed assertion from a registered SAML provider whose Conditions/SubjectConfirmationData `NotOnOrAfter` is already in the past, with an IssueInstant still inside the 5-minute redemption window — AWS refuses with error code `ExpiredTokenException` at HTTP 400 and the message `Response has expired`. That message is entirely FIXED text for this error class: it carries no value taken from the request or from service state (recorded behavior; the rule that produces it — an assertion whose validity window has passed — is stated by the model's ExpiredTokenException and by the check, so the wording is the requirement for this refusal and not one run's value). The same request repeated must be refused identically (the refusal is derived from the assertion's own timestamps, so it is stable and not consumed).
The positive control AWS serves on the same trust: an otherwise-identical, currently-valid, correctly-signed assertion returns HTTP 200 with Credentials and `Subject` equal to the assertion's NameID (the session name).

Emulator: the unsigned awsQuery POST is not resolved to any service. The emulator answers a JSON body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — the wrong protocol for STS entirely — so botocore never constructs an error object and dies in its XML parser: `botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed`. Neither the expired-assertion refusal nor the valid-assertion positive control is ever reached: no status code, no error code and no message of the modeled contract is produced.

Why: a client relying on AWS's behavior cannot distinguish an expired SAML assertion from any other failure, because no classifiable response is returned at all — botocore raises a transport/parse exception rather than a ClientError with Code `ExpiredTokenException`. Enterprise SAML sign-in code that re-drives the IdP on `ExpiredTokenException` (the normal, expected path when a browser POSTs a stale SAMLResponse) will instead surface an unparseable-response crash, and a negative test asserting that a stale assertion is rejected can never pass or meaningfully fail. Because the operation is anonymous by design, this is not a credentials problem the caller can work around.

Why it is the emulator's: the request reaches the emulator and the emulator answers it; it simply refuses to attribute an unsigned request to STS.

Fix: the request-routing / service-resolution layer must resolve an anonymous awsQuery request to STS (e.g. from the `Action` form parameter plus the endpoint it is reached on) BEFORE authentication, instead of requiring a SigV4 credential scope, X-Amz-Target or Host header. Then the AssumeRoleWithSAML handler's assertion validation must evaluate the assertion's Conditions/SubjectConfirmationData validity window and raise ExpiredTokenException, and the service's awsQuery XML error serializer must render it as an ErrorResponse with code `ExpiredTokenException` at HTTP 400 and the message above; the success path must populate Subject from the assertion NameID.

Traces: sts.assume-role-with-saml-expired-token-rejected.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator (62/172 passed). `sts` is bootstrapped and answering (59 of 139 sts checks pass), so this is a fidelity finding and not a bootstrap gap.

Failing check `sts.assume-role-with-saml-expired-token-rejected` [error]:
```
File "parsers.py", line 600, in _do_error_parse
  root = self._parse_xml_string_to_dom(xml_contents)
File "parsers.py", line 550, in _parse_xml_string_to_dom
  raise ResponseParserError(
botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
```

Expected (AWS), for the expired assertion: an awsQuery XML ErrorResponse, HTTP 400, error Code `ExpiredTokenException`, Message `Response has expired` — the message is fixed text with no request- or state-derived parts — and the identical response on an immediate retry of the same assertion (the check asserts the refusal twice, via `_error_stable`, as its effect-absent proof).
Expected (AWS), for the positive control the check runs first: HTTP 200 with `Credentials.AccessKeyId` non-empty and `Subject` equal to the assertion's session name.
Actual (emulator): HTTP body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` in JSON, for which botocore's awsQuery error parser raises ResponseParserError; no error code, no status from the modeled contract, and no credentials on the control path.

The check mints a real RSA keypair and X.509 certificate with `cryptography`, registers a SAML provider in IAM, hand-builds the SAMLResponse already in Exclusive-C14N form and RSA-PKCS1v15/SHA-256-signs it, so the assertion presented is genuinely well-formed and correctly signed; the only thing varied for the refusal is the validity window (NotBefore now-2h, NotOnOrAfter now-1h, IssueInstant now-30s). The `cryptography` dependency IS present — the check's `_crypto_available()` guard did not trip; the failure occurs on the wire.

This is the same emulator defect as the one described for the other SAML checks in open issue 20261025T022928Z-sts-assumerolewithsaml-is-unroutable-unknownservice-assumerole-parses-the.md, but that issue's Issue-Trace-Ids and Issue-Key lines do not include this check id, so this check carries no replay key anywhere.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite).
2. Re-replay exactly the check id in trace_ids: `sts.assume-role-with-saml-expired-token-rejected`. It is self-contained and establishes its positive control (a valid, signed assertion must vend credentials) before asserting the refusal.
3. Minimal repro: with `cryptography`, mint an RSA keypair plus a self-signed X.509 cert; `iam.create_saml_provider(Name=..., SAMLMetadataDocument=<metadata embedding that cert, EntityDescriptor entityID=<entity_id>>)`; `iam.create_role` with a trust policy whose Principal is `{"Federated": <provider arn>}`, Action `sts:AssumeRoleWithSAML`, Condition `StringEquals {"SAML:aud": "https://signin.aws.amazon.com/saml"}`.
4. Build a SAMLResponse in Exclusive-C14N form, signed RSA-PKCS1v15/SHA-256, with NameID = the session name, the `https://aws.amazon.com/SAML/Attributes/Role` attribute = `<role arn>,<provider arn>`, Conditions/SubjectConfirmationData `NotBefore` = now-30s and `NotOnOrAfter` = now+10m. Call, with an UNSIGNED/anonymous client (the operation is modeled `auth: []`, `optionalAuth: true`), `sts.assume_role_with_saml(RoleArn=<role arn>, PrincipalArn=<provider arn>, SAMLAssertion=<base64 of that response>)`. AWS: HTTP 200 with Credentials and `Subject` = the session name. Emulator: JSON `{"__type":"UnknownService",...}` and botocore raises ResponseParserError.
5. Break exactly one thing: rebuild the same signed response with `NotBefore` = now-2h, `NotOnOrAfter` = now-1h and `IssueInstant` = now-30s (so the 5-minute redemption window is still satisfied and the expiry is what is refused), and call AssumeRoleWithSAML again. AWS: HTTP 400, error code `ExpiredTokenException`, message `Response has expired`. Emulator: the same JSON `UnknownService` body and ResponseParserError.
6. Repeat step 5 verbatim a second time: AWS returns the identical 400 `ExpiredTokenException` / `Response has expired`, and no credentials are ever vended for the expired assertion.
