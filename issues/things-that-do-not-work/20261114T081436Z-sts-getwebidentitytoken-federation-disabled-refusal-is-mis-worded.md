Issue-Source: trace-sweep-divergence
Issue-Trace: sts-web-identity-token-basic
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-web-identity-token-basic sts-web-identity-token-federation-disabled sts-web-identity-token-outbound-federation sts-web-identity-token-with-tags-and-duration sts-web-identity-token-invalid-algorithm-error
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|GetWebIdentityToken|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sts|GetWebIdentityToken|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sts|GetWebIdentityToken|4xx->4xx|status
Issue-Commit: 5203ee4b51d686c4d938ce5913fc60c366a08b57

# sts GetWebIdentityToken federation-disabled refusal is mis-worded and preempts SigningAlgorithm validation

> _Note: this issue was identified against the code base at commit `5203ee4b51d686c4d938ce5913fc60c366a08b57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWebIdentityToken

## Description
Two net-new root causes in the sts GetWebIdentityToken path. Both concern how the operation refuses a request: the text of the federation-disabled refusal, and the fact that this refusal is emitted before the request's own input is validated. Cause count: 2.

Cause 1 of 2: the OutboundWebIdentityFederationDisabledException message is the model's documentation prose instead of the service's error message.
AWS: Smithy: GetWebIdentityToken error OutboundWebIdentityFederationDisabledException, errorKind client, @httpError 403 (awsQueryError code OutboundWebIdentityFederationDisabledException, httpResponseCode 403); the shape carries a single `message` member whose wire text the model does not state, so recorded behavior only — the recording shows AWS returning HTTP 403, Error.Code OutboundWebIdentityFederationDisabledException and the fixed message text 'OutboundWebIdentityFederation is disabled.' That message is entirely fixed text: it has no variable parts drawn from the request or from state, so it must be reproduced verbatim for every request refused for this reason.
Emulator: status 403 and Error.Code are correct, but Error.Message is 'The outbound web identity federation feature is not enabled for this account. To use this feature, you must first enable it through the Amazon Web Services Management Console or API.' — i.e. the @documentation string of the OutboundWebIdentityFederationDisabledException shape, rendered as the runtime message.
Why: a client relying on AWS's behavior that matches, logs or asserts on the refusal message (error-mapping tables, user-facing surfacing, test fixtures that pin the message) sees text AWS never emits. More broadly, this shows the error serializer defaulting any STS exception with no explicit message to its model documentation, so every such error on this path carries doc prose rather than the service's wording.
Fix: the GetWebIdentityToken handler / the sts error construction for OutboundWebIdentityFederationDisabledException — set the message explicitly to the service's fixed text rather than letting the serializer fall back to the shape's documentation.
Traces: sts-web-identity-token-basic#task#1 (representative); also sts-web-identity-token-federation-disabled#task#1, sts-web-identity-token-outbound-federation#task#1, sts-web-identity-token-with-tags-and-duration#task#1.

Cause 2 of 2: an unsupported SigningAlgorithm is not validated — the federation-disabled refusal is returned instead of the 400 ValidationError.
AWS: Smithy: GetWebIdentityToken.SigningAlgorithm is @required, target jwtAlgorithmType with @length min 5 / max 5; the model defines no enum of accepted algorithm values and no ValidationError shape on the operation, so recorded behavior only — the recording shows AWS validating the requested algorithm against its supported set and refusing an unsupported one with HTTP 400, Error.Code 'ValidationError' and message 'Requested SigningAlgorithm is not supported. See the GetWebIdentityToken documentation for supported algorithms'. That message is wholly fixed text for this error class (it names no request value), and the status/code pair 400 + ValidationError is the requirement. The recording also establishes ordering: request-shape validation of SigningAlgorithm happens before the account's outbound-web-identity-federation entitlement is consulted, because the same account that yields the 403 federation-disabled refusal for a well-formed request yields the 400 ValidationError when the algorithm is unsupported (see sts-web-identity-token-outbound-federation, whose task#1 is refused 403 and whose task#2 is refused 400).
Emulator: returns status 403 with Error.Code 'OutboundWebIdentityFederationDisabledException' and the doc-prose message from Cause 1 — the entitlement gate fires first and unconditionally, so SigningAlgorithm is never inspected and no ValidationError can ever be produced. Divergence: status expected 400, got 403; Error.Code expected 'ValidationError', got 'OutboundWebIdentityFederationDisabledException'.
Why: a client that sends a bad algorithm gets a retriable-looking account-entitlement error rather than the 400 client-validation error that tells it the request is malformed. Code that branches on ValidationError to correct the algorithm never takes that branch, and 4xx-class handling that treats 403 as 'ask an administrator to enable the feature' sends the operator down an unrelated path while the actual fault is in the request.
Fix: the GetWebIdentityToken handler's input validation — validate SigningAlgorithm (and the rest of the modeled input constraints) and emit the 400 ValidationError before consulting the account's outbound-web-identity-federation entitlement; the entitlement check must run only on an otherwise-valid request.
Traces: sts-web-identity-token-invalid-algorithm-error#task#1 (cause representative); also sts-web-identity-token-outbound-federation#task#2.

No open issue covers either cause: the replay reports both clusters with an empty owned_by. The nearest open issues are 20261112T235120Z-sts-iam-createopenidconnectprovider-accepts-the-account-s-own-in.md (IAM CreateOpenIDConnectProvider, a different operation) and 20261026T234020Z-cognito-identity-4-net-new-root-causes-web.md (cognito-identity web-identity sessions); neither touches sts GetWebIdentityToken.

## Evidence
Cause 1 — fixture sts-web-identity-token-basic, step task#1 (cluster of 4, affected_count 4):
  expected status 403, actual status 403
  $.ErrorResponse.Error.Message: value_mismatch
    expected: 'OutboundWebIdentityFederation is disabled.'
    actual:   'The outbound web identity federation feature is not enabled for this account. To use this feature, you must first enable it through the Amazon Web Services Management Console or API.'
  Error.Code matches (OutboundWebIdentityFederationDisabledException). The actual string is byte-for-byte the @documentation of com.amazonaws.sts#OutboundWebIdentityFederationDisabledException in the on-disk model.
  Other cluster members: sts-web-identity-token-federation-disabled#task#1, sts-web-identity-token-outbound-federation#task#1, sts-web-identity-token-with-tags-and-duration#task#1.

Cause 2 — fixture sts-web-identity-token-invalid-algorithm-error, step task#1 (cluster of 2, affected_count 2):
  status: expected 400, got 403
  $.ErrorResponse.Error.Code: expected 'ValidationError', got 'OutboundWebIdentityFederationDisabledException'
  $.ErrorResponse.Error.Message:
    expected: 'Requested SigningAlgorithm is not supported. See the GetWebIdentityToken documentation for supported algorithms'
    actual:   'The outbound web identity federation feature is not enabled for this account. To use this feature, you must first enable it through the Amazon Web Services Management Console or API.'
  Other cluster member: sts-web-identity-token-outbound-federation#task#2 — the same fixture whose task#1 is correctly refused 403, which is what pins the ordering: validation of SigningAlgorithm must precede the entitlement gate.

Model evidence (smithy_query, services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json):
  GetWebIdentityToken input: Audience (required, list, @length 1..10), DurationSeconds (@range 60..3600), SigningAlgorithm (required, jwtAlgorithmType @length 5..5), Tags (@length 0..50)
  errors: JWTPayloadSizeExceededException (400), OutboundWebIdentityFederationDisabledException (403), SessionDurationEscalationException (403) — no ValidationError shape and no algorithm enum are modeled, hence 'recorded behavior only' for cause 2's code and text.

Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, 41 steps, 27 passed, 14 mismatched, score 65.9%. These two clusters are the only ones with an empty owned_by and an empty stub_path.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture sts-web-identity-token-basic, step task#1 — a GetWebIdentityToken call on an account without outbound web identity federation enabled, with a well-formed request (valid Audience, valid 5-character SigningAlgorithm). Expect HTTP 403, Error.Code OutboundWebIdentityFederationDisabledException, Error.Message exactly 'OutboundWebIdentityFederation is disabled.'; observe the model's documentation prose as the message instead. Reproduces identically on sts-web-identity-token-federation-disabled#task#1, sts-web-identity-token-outbound-federation#task#1 and sts-web-identity-token-with-tags-and-duration#task#1.
3. Cause 2: replay fixture sts-web-identity-token-invalid-algorithm-error, step task#1 — a GetWebIdentityToken call whose SigningAlgorithm is not one of the supported JWT algorithms. Expect HTTP 400 with Error.Code 'ValidationError' and Error.Message 'Requested SigningAlgorithm is not supported. See the GetWebIdentityToken documentation for supported algorithms'; observe HTTP 403 with Error.Code OutboundWebIdentityFederationDisabledException. Reproduces on sts-web-identity-token-outbound-federation#task#2, whose task#1 in the same fixture is refused 403 correctly — confirming the entitlement gate must run after input validation, not before.
4. After fixing, re-replay both fixtures (and the other members listed in the clusters) and confirm every GetWebIdentityToken step passes.
