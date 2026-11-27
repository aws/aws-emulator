Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.mock-when-no-templates-rejects-an-unmapped-content-type apigateway.never-passthrough-unmapped-content-type-rejected-415 apigateway.iam-stage-rejects-a-signature-scoped-to-another-region apigateway.iam-stage-request-signed-for-another-service-rejected apigateway.none-auth-method-anonymous-call-refused-by-resource-policy apigateway.apigw-api-level-settings-take-effect-only-after-redeploy apigateway.apigw-request-content-encoding-that-cannot-be-decoded-rejected-with-415-and-an-empty-body apigateway.rest-api-tls13-security-policy-tls12-handshake-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.mock-when-no-templates-rejects-an-unmapped-content-type|run#18|AssertionError|assert got == (415, '{"message": "Unsupported Media Type"}'), (
Issue-Key: apigateway.never-passthrough-unmapped-content-type-rejected-415|run#11|AssertionError|assert (status, head.get("x-amzn-errortype"), text) == (415, "BadRequestException", _UNSUPPORTED), (
Issue-Key: apigateway.iam-stage-rejects-a-signature-scoped-to-another-region|run#20|AssertionError|assert (refused.status_code, json.loads(refused.text)) == (403, expected), (
Issue-Key: apigateway.iam-stage-request-signed-for-another-service-rejected|run#18|AssertionError|assert refusal == (
Issue-Key: apigateway.none-auth-method-anonymous-call-refused-by-resource-policy|_poll#14|AssertionError|raise AssertionError(f"{what} not observed within {cap}s; last {last!r}")
Issue-Key: apigateway.apigw-api-level-settings-take-effect-only-after-redeploy|run#10|AssertionError|assert status == 200 and "content-encoding" not in headers, (
Issue-Key: apigateway.apigw-request-content-encoding-that-cannot-be-decoded-rejected-with-415-and-an-empty-body|run#7|Timeout|time.sleep(2)
Issue-Key: apigateway.rest-api-tls13-security-policy-tls12-handshake-rejected|_refused#14|Timeout|time.sleep(3)
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway: 7 net-new execute-api causes — SigV4 credential scope is not checked, a resource-policy deny hits the admitted caller, the 415 gateway response is mis-framed, and stage settings/TLS policy are unenforced

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Seven independent causes on the REST execute-api data plane of a deployed stage. Two of them are authorization bypasses or inversions.

Cause 1 of 7: the 415 UNSUPPORTED_MEDIA_TYPE gateway response is mis-framed — the body is serialized compactly and x-amzn-ErrorType is absent.
AWS: recorded behavior only (BadRequestException is @httpError 400 in the model; this gateway response is the data-plane 415). A request whose Content-Type has no matching requestTemplate under passthroughBehavior WHEN_NO_TEMPLATES or NEVER is refused with HTTP 415, header `x-amzn-ErrorType: BadRequestException`, and the body `{"message": "Unsupported Media Type"}` — that exact text, including the space after the colon; the whole string is fixed, with no request or state values in it.
Emulator: HTTP 415 is right, but the body is `{"message":"Unsupported Media Type"}` (no space after the colon) and no `x-amzn-ErrorType` header is sent (observed as None).
Why: a client that byte-compares the documented body, or that classifies the error from x-amzn-ErrorType, cannot recognise the refusal.
Fix: the gateway-response serializer for UNSUPPORTED_MEDIA_TYPE — emit the x-amzn-ErrorType header and AWS's body text verbatim.
Traces: apigateway.mock-when-no-templates-rejects-an-unmapped-content-type, apigateway.never-passthrough-unmapped-content-type-rejected-415

Cause 2 of 7: a SigV4 signature whose credential scope names another region is served.
AWS: recorded behavior only. An AWS_IAM-authorized method refuses a request signed with a credential scope for a different region with HTTP 403, `x-amzn-ErrorType: InvalidSignatureException` and body `{"message": "Credential should be scoped to a valid region. "}` — fixed text, note the trailing space; nothing in it comes from the request. The subsequent correctly scoped request is served 200.
Emulator: the cross-region-signed GET is served 200 with the integration's body (`{"vault":"opened"}`) — the credential scope's region is not validated at all.
Why: a signature minted for another region is accepted, so a client (or a test) relying on API Gateway's regional scoping gets no refusal — an authorization bypass that only shows up against real AWS.
Fix: the execute-api SigV4 verifier — validate the credential scope's region and emit the modelled refusal.
Traces: apigateway.iam-stage-rejects-a-signature-scoped-to-another-region

Cause 3 of 7: the wrong-service credential-scope refusal message is not AWS's text.
AWS: recorded behavior only. A request signed for a service other than execute-api is refused 403, `x-amzn-ErrorType: InvalidSignatureException`, body `{"message": "Credential should be scoped to correct service: 'execute-api'. "}` — fixed text ending in a period and a single trailing space, with the service name `execute-api` quoted.
Emulator: status 403 and the error type are right, but the body is `{"message":"Credential should be scoped to correct service: 'execute-api'."}` — the trailing space after the period is missing (and the body is compactly serialized).
Fix: the execute-api SigV4 verifier's refusal body for a mis-scoped service.
Traces: apigateway.iam-stage-request-signed-for-another-service-rejected

Cause 4 of 7: a resource policy's explicit deny is applied to the signed caller the policy admits.
AWS: recorded behavior only. With a resource policy that denies anonymous access but admits the signed principal, the signed SigV4 request to the AWS_IAM method is served 200 by the integration; only the anonymous request to the NONE-auth method is refused with the explicit-deny AccessDeniedException.
Emulator: the signed AWS_IAM request is refused 403 AccessDeniedException `User: arn:aws:iam::<account>:user/emulator is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/GET/signed with an explicit deny in a resource-based policy` and never becomes served — the deny statement is matched against the admitted principal.
Why: the policy evaluation is inverted for the admitted caller, so every legitimate signed call to a policy-protected API is refused.
Fix: the execute-api resource-policy evaluation (principal matching for Deny statements).
Traces: apigateway.none-auth-method-anonymous-call-refused-by-resource-policy

Cause 5 of 7: API-level stage settings take effect on the live stage without a redeploy.
AWS: recorded behavior only. Changing an API-level setting (here response compression / content encoding) does not change what the already-deployed stage serves until a new deployment is created — the stage keeps answering uncompressed until the redeploy, then answers gzip.
Emulator: immediately after the setting change and before any redeploy the stage already answers `200` with `Content-Encoding: gzip`.
Why: deployment is the unit of change in API Gateway; a client that relies on a stage being frozen between deployments sees configuration leak into live traffic.
Fix: the stage/deployment snapshot used by the execute-api request path — resolve API-level settings from the stage's deployment snapshot, not from live API state.
Traces: apigateway.apigw-api-level-settings-take-effect-only-after-redeploy

Cause 6 of 7: a request body with an undecodable Content-Encoding is not refused 415 with an empty body.
AWS: recorded behavior only. A request declaring a Content-Encoding whose payload cannot be decoded is refused with HTTP 415 and an EMPTY response body.
Emulator: the check reaches no verdict inside its budget — the stage never produced the 415 (nor any graded answer) for the undecodable-encoding request.
Fix: the execute-api request-decoding path — refuse an undecodable Content-Encoding with a 415 and no body.
Traces: apigateway.apigw-request-content-encoding-that-cannot-be-decoded-rejected-with-415-and-an-empty-body

Cause 7 of 7: a domain name's TLS securityPolicy is not enforced at the handshake.
AWS: recorded behavior only (DomainName.securityPolicy is the SecurityPolicy enum in the model). A custom domain configured with the TLS 1.3 security policy refuses a TLS 1.2-only handshake; a conforming handshake succeeds.
Emulator: the check reaches no verdict inside its budget — neither a refused handshake nor a successful one is observed, so the securityPolicy has no effect on the listener. Related to the already-open issue 20261116T020413Z-acm-apigateway-the-regional-custom-domain-still-reaches (regional custom domains reach no TLS verdict), but that issue is filed against the acm__apigateway service and does not carry this check.
Fix: the custom-domain TLS listener — honour securityPolicy when negotiating.
Traces: apigateway.rest-api-tls13-security-policy-tls12-handshake-rejected

## Evidence
Cause 1 — expected (415, '{"message": "Unsupported Media Type"}') with x-amzn-ErrorType 'BadRequestException'; got (415, '{"message":"Unsupported Media Type"}') with x-amzn-ErrorType None.
Cause 2 — 'a signature scoped to us-east-1 gave 200 {"vault":"opened"}; expected 403 {"message": "Credential should be scoped to a valid region. "}'.
Cause 3 — expected (403, 'InvalidSignatureException', {'message': "Credential should be scoped to correct service: 'execute-api'. "}); got 403 'InvalidSignatureException' '{"message":"Credential should be scoped to correct service: \'execute-api\'."}'.
Cause 4 — 'the signed AWS_IAM method to be served not observed within 36.8s; last (403, AccessDeniedException, ...explicit deny in a resource-based policy)'.
Cause 5 — 'before a redeploy the stage must still answer uncompressed, got 200 \'gzip\''.
Cause 6 — check exhausted its 60 s budget with no 415 observed.
Cause 7 — check exhausted its 60 s budget with neither a handshake nor a refusal.

## Steps to Reproduce
1. CreateRestApi (REGIONAL) with a MOCK integration; CreateDeployment + stage.
2. Cause 1: set passthroughBehavior WHEN_NO_TEMPLATES (and NEVER) with only an application/json template, POST text/plain -> inspect the 415 body bytes and the x-amzn-ErrorType header.
3. Cause 2/3: PutMethod authorizationType=AWS_IAM; sign a GET with SigV4 using region us-east-1 (then service 'lambda') against the us-west-2 stage URL -> inspect status and body.
4. Cause 4: attach a resource policy denying anonymous access but admitting the signed principal; issue the signed GET to the AWS_IAM method.
5. Cause 5: change the API-level compression/content-encoding setting and immediately GET the stage without creating a deployment.
6. Cause 6: POST with Content-Encoding: gzip and a non-gzip body.
7. Cause 7: CreateDomainName with the TLS 1.3 security policy and attempt a TLS1.2-only handshake against it.
