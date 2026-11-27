Issue-Source: trace-sweep-divergence
Issue-Trace: sts-http-origin-header-on-signed-query-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-http-origin-header-on-signed-query-requests
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|DecodeAuthorizationMessage|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sts|DecodeAuthorizationMessage|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sts|DecodeAuthorizationMessage|4xx->4xx|status
Issue-Key: sts|GetCallerIdentity|2xx->4xx|$.GetCallerIdentityResponse
Issue-Key: sts|GetCallerIdentity|2xx->4xx|status
Issue-Commit: e8f09314d35987b191cae688f7fba7f207639dbf

# sts: a correctly signed Query request that carries an HTTP Origin header is refused 403 SignatureDoesNotMatch

> _Note: this issue was identified against the code base at commit `e8f09314d35987b191cae688f7fba7f207639dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DecodeAuthorizationMessage

## Description
2 causes, both on the STS Query front door when an otherwise correctly SigV4-signed request also carries a browser `Origin` request header. In both cases the emulator never reaches the operation at all: it answers the signature gate's 403 instead of the operation's modelled response. The two causes share one fault — the presence of the `Origin` header changes the emulator's verdict on an unchanged, validly signed request — but each is a distinct operation/response contract and each must be closed separately.

Cause 1 of 2: DecodeAuthorizationMessage is refused 403 SignatureDoesNotMatch instead of 400 InvalidAuthorizationMessageException.
AWS: Smithy: DecodeAuthorizationMessage input EncodedMessage is required, target encodedMessageType with @length(min: 1, max: 10240); its only modelled error is InvalidAuthorizationMessageException (errorKind client, httpError 400, awsQuery error code `InvalidAuthorizationMessageException`), documented as "The error returned if the message passed to DecodeAuthorizationMessage was invalid. This can happen if the token contains invalid characters, such as line breaks, or if the message has expired." A request that is signed with valid credentials is authenticated regardless of any `Origin` header — `Origin` is not part of SigV4's canonical request unless the client itself signed it — so AWS proceeds to validate the message and refuses the *message*, not the signature. The recorded response is HTTP 400 with ErrorResponse.Error.Code `InvalidAuthorizationMessageException` and ErrorResponse.Error.Message of the form `Message is not valid: [<bytes>]`. In that message the fixed text is `Message is not valid: ` followed by a bracketed, comma-separated list; the list contents are a value derived from the request's own EncodedMessage (the signed 8-bit values of the bytes STS decoded from it), not a constant — the recorded run's particular array must not be reproduced literally.
Emulator: answers HTTP 403 with Error.Code `SignatureDoesNotMatch` and Error.Message "The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details." The request body/credentials are unchanged from the signed form the recording used; only the `Origin` header distinguishes it.
Why: a client that reaches STS from a browser-origin context (or through any proxy that adds `Origin`) can never decode an authorization message: it sees an authentication failure and will rotate or re-sign credentials forever instead of learning that its EncodedMessage was rejected, and it never receives the `InvalidAuthorizationMessageException` its error handling branches on.
Fix: the shared SigV4 request-authentication / cross-site front door in front of the sts Query dispatcher — the canonical request it recomputes must not depend on headers the client did not sign, and an `Origin`-bearing signed request must be authenticated and dispatched like any other. Once dispatched, DecodeAuthorizationMessage's own validation must raise InvalidAuthorizationMessageException (400) with AWS's `Message is not valid: [...]` wording.
Traces: sts-http-origin-header-on-signed-query-requests#task#1 (expected 400 InvalidAuthorizationMessageException, got 403 SignatureDoesNotMatch).

Cause 2 of 2: GetCallerIdentity is refused 403 SignatureDoesNotMatch instead of answering 200 with the caller's identity.
AWS: Smithy: GetCallerIdentity takes an empty input, returns UserId/Account/Arn (Arn @length 20..2048) and declares NO errors at all — a request signed with valid credentials always succeeds. An `Origin` header does not change that. The recorded response is HTTP 200 carrying GetCallerIdentityResponse.GetCallerIdentityResult with Arn, UserId and Account (the emulator's account, 000000000000), plus ResponseMetadata.RequestId; Arn, UserId and the request id are per-environment values and are not part of the requirement.
Emulator: answers HTTP 403 SignatureDoesNotMatch and serves no GetCallerIdentityResponse element at all (the whole response body is the error envelope).
Why: GetCallerIdentity is the canonical "who am I / is my configuration working" probe. Refusing it for an `Origin`-bearing but validly signed request makes every browser-origin or proxied SDK/CLI identity check report broken credentials, and because the operation models no errors, a client has no handling path for a 403 here — it reports an unauthenticated session for a session that is in fact authenticated.
Fix: same component — the SigV4 authentication/cross-site gate ahead of the sts Query dispatcher; GetCallerIdentity's handler is never entered, so the fix belongs in that gate, not in the handler or serializer.
Traces: sts-http-origin-header-on-signed-query-requests#task#2 (expected 200 GetCallerIdentityResponse, got 403 SignatureDoesNotMatch, body missing).

Related but NOT covering these: open issues `20261127T032705Z-dynamodb-listtables-refuses-a-well-formed-origin-bearing.md` and `20261117T221515Z-dynamodb-http-edge-omits-cors-response-headers-and.md` report the same shape of fault on the DynamoDB edge, and `20261124T063020Z-apigatewayv2-lambda-the-execute-api-front-door-refuses.md` on the execute-api front door; none of them carries an sts replay key, so these two sts causes are net-new. The six other sts divergences in this corpus (vended AccessKeyId / PackedPolicySize / SessionTokenUtilization) are already owned by `20261029T191504Z-sts-vended-credentials-and-policy-size-members-diverge.md` and are not re-filed here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (28 fixtures, 59 steps, 50 passed, 9 mismatched, score 84.7%).

Cause 1 — fixture sts-http-origin-header-on-signed-query-requests, step task#1, operation DecodeAuthorizationMessage:
  expected status 400, actual status 403
  $.ErrorResponse.Error.Code: expected 'InvalidAuthorizationMessageException', got 'SignatureDoesNotMatch'
  $.ErrorResponse.Error.Message: expected 'Message is not valid: [<signed byte values decoded from the request's EncodedMessage>]' (recorded run: '[-98, -117, 126, 106, 127, -98, -99, -54, 29, 121, -33, -90, 122, -53, 26, -127]'), got 'The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details.'

Cause 2 — fixture sts-http-origin-header-on-signed-query-requests, step task#2, operation GetCallerIdentity:
  expected status 200, actual status 403
  $.GetCallerIdentityResponse: missing (expected {'GetCallerIdentityResult': {'Arn': <shaped>, 'UserId': <shaped>, 'Account': '000000000000'}, 'ResponseMetadata': {'RequestId': <shaped>}}, got None)

Both steps are SigV4-signed Query requests that additionally carry an HTTP Origin header; neither cluster carries a stub_path or an owned_by owner.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm the sts Query endpoint answers.
2. Replay fixture `sts-http-origin-header-on-signed-query-requests` from trace set simparity-wire-aws-v1.
3. Step task#1 POSTs a SigV4-signed STS Query request for Action=DecodeAuthorizationMessage with an invalid EncodedMessage and an HTTP `Origin` header. Observe HTTP 403 ErrorResponse.Error.Code=SignatureDoesNotMatch instead of HTTP 400 Code=InvalidAuthorizationMessageException with a `Message is not valid: [...]` message.
4. Step task#2 POSTs a SigV4-signed STS Query request for Action=GetCallerIdentity with the same `Origin` header. Observe HTTP 403 SignatureDoesNotMatch instead of HTTP 200 with GetCallerIdentityResponse/GetCallerIdentityResult (Arn, UserId, Account).
5. Re-send either request with the `Origin` header removed and nothing else changed to see the request authenticate.
