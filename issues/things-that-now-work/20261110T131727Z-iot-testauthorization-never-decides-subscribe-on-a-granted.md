Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.test-authorization-subscribe-requires-topicfilter-resource
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.test-authorization-subscribe-requires-topicfilter-resource|_eventually#15|Timeout|time.sleep(interval_s)
Issue-Commit: b65ac132d2656b2cf62c7f436b7f396df22480d4

# iot TestAuthorization never decides: SUBSCRIBE on a granted topicfilter/ ARN is refused 501 instead of ALLOWED, and the topic/ vs topicfilter/ namespace split is never evaluated

> _Note: this issue was identified against the code base at commit `b65ac132d2656b2cf62c7f436b7f396df22480d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestAuthorization

## Description
Cause 1 of 1: `TestAuthorization` returns no authorization decision at all, so IoT policy evaluation — specifically the resource-namespace split between `topicfilter/<filter>` (SUBSCRIBE) and `topic/<topic>` (PUBLISH/RECEIVE) — is unobservable through the emulator.

AWS: Smithy `com.amazonaws.iot#TestAuthorization` — POST /test-authorization, success HTTP 200, input `authInfos` (required, @length min 1 max 10) of `AuthInfo{actionType: ActionType, resources: Resources (required)}`, `principal`, `clientId` (httpQuery), `cognitoIdentityPoolId`, `policyNamesToAdd`, `policyNamesToSkip`; output `authResults` of `AuthResult{authInfo, allowed: Allowed{policies}, denied, authDecision, missingContextValues}`. `ActionType` enum = PUBLISH | SUBSCRIBE | RECEIVE | CONNECT; `AuthDecision` enum = ALLOWED | EXPLICIT_DENY | IMPLICIT_DENY. Modeled errors are InternalFailureException(500), InvalidRequestException(400), LimitExceededException(410), ResourceNotFoundException(404), ServiceUnavailableException(503), ThrottlingException(400), UnauthorizedException(401) — HTTP 501 / `NotImplemented` is not among them. Model doc: "Tests if a specified principal is authorized to perform an IoT action on a specified resource." The behavior the check asserts, per AWS's documented IoT policy resource model: an `Allow` of `iot:Subscribe` on `arn:aws:iot:<region>:<account>:topicfilter/<path>` attached to a certificate principal must yield `authResults[0].authDecision == "ALLOWED"` for actionType SUBSCRIBE against that same topicfilter ARN; the SAME request against `arn:aws:iot:<region>:<account>:topic/<path>` must yield `IMPLICIT_DENY` with no policy reported under `allowed.policies` (Subscribe is evaluated against the `topicfilter/` namespace only); and actionType PUBLISH against the `topic/` ARN must also yield `IMPLICIT_DENY` with empty `allowed.policies`, because a Subscribe-only grant never confers Publish. The decision is stateless: repeating the granted SUBSCRIBE after the two denials must still be `ALLOWED`.

Emulator: every `TestAuthorization` call is refused with HTTP 501 and error code `NotImplemented`, message `operation TestAuthorization is scaffolded but not implemented in this emulator`. `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("TestAuthorization"))`. In this check the refusal is swallowed by the check's eventual-consistency retry loop (it tolerates transient exceptions while waiting for the attachment to propagate), so instead of a fast error the check exhausts its 60 s budget and the suite reports `Check timed out after 60.0s`. No `authResults` is ever produced, so none of the four namespace/action assertions is ever reached.

Related already-filed issue: `20261108T182336Z-iot-testauthorization-stub.md` names the same stub, but it carries only `iot.test-authorization-policy-evaluation` as its replay key and therefore does not cover this check; this check additionally pins the `topicfilter/` vs `topic/` namespace scoping and the Subscribe-does-not-confer-Publish rule, which must hold once the handler exists.

Why: a client debugging device permissions with `TestAuthorization` gets a 501 (or, behind any retry wrapper, an indefinite hang) instead of a decision. A device-policy test harness cannot distinguish "allowed" from "denied", and cannot catch the single most common IoT policy bug — granting `iot:Subscribe` on a `topic/` ARN instead of `topicfilter/`, or assuming Subscribe implies Publish — because the emulator never evaluates the resource namespace or the action at all.

Fix: implement the handler in `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs` — resolve the `principal` (certificate ARN) to its attached policies from the service's IoT policy/attachment state, evaluate each `authInfos` entry's `actionType` against each `resources` entry with the IoT resource-namespace rules (SUBSCRIBE against `topicfilter/`, PUBLISH and RECEIVE against `topic/`, CONNECT against `client/`), and serialize one `AuthResult` per `authInfo` with `authDecision` ALLOWED / EXPLICIT_DENY / IMPLICIT_DENY and `allowed.policies` populated only for an ALLOWED decision. Remove the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line only when the handler is real.

Traces: iot.test-authorization-subscribe-requires-topicfilter-resource (suite verdict `mismatch`, detail `Check timed out after 60.0s`).

## Evidence
Expected (AWS): HTTP 200 with `authResults[0]` present. With policy `{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:Subscribe","Resource":"arn:aws:iot:<region>:<account>:topicfilter/<path>"}]}` attached to the certificate principal:
  - TestAuthorization(principal=<certArn>, authInfos=[{actionType:"SUBSCRIBE", resources:["arn:aws:iot:<region>:<account>:topicfilter/<path>"]}]) -> authResults[0].authDecision == "ALLOWED"
  - TestAuthorization(... actionType="SUBSCRIBE", resources=["arn:aws:iot:<region>:<account>:topic/<path>"]) -> authResults[0].authDecision == "IMPLICIT_DENY", allowed.policies empty/absent
  - TestAuthorization(... actionType="PUBLISH", resources=["arn:aws:iot:<region>:<account>:topic/<path>"]) -> authResults[0].authDecision == "IMPLICIT_DENY", allowed.policies empty/absent
  - repeating the first call -> still "ALLOWED" (decision is stateless)
(The region, account, certificate ARN and topic path above are values taken from the request and the emulator's state; only the enum values ALLOWED / IMPLICIT_DENY and the empty `allowed.policies` are fixed requirements.)

Actual (emulator): every one of those calls is refused:
  HTTP 501, error code `NotImplemented`,
  message `operation TestAuthorization is scaffolded but not implemented in this emulator`
  (fixed text; the only variable part is the operation name `TestAuthorization`).
`NotImplemented`/501 is not a modeled outcome of TestAuthorization. Because the check retries through transient exceptions while waiting for attachment propagation, the observed suite verdict is `Check timed out after 60.0s` rather than a surfaced ClientError.

Handler state: `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:`; line 173 `Err(crate::scaffolded::Scaffolded::not_implemented("TestAuthorization"))`.

## Steps to Reproduce
1. Boot the iot emulator.
2. `create_keys_and_certificate(setAsActive=True)` and keep its certificateArn.
3. `create_policy` with an Allow of `iot:Subscribe` on `arn:aws:iot:<region>:<account>:topicfilter/<path>`, then `attach_policy(policyName=..., target=<certArn>)`.
4. Call `test_authorization(principal=<certArn>, authInfos=[{"actionType":"SUBSCRIBE","resources":["arn:aws:iot:<region>:<account>:topicfilter/<path>"]}])`. Observe HTTP 501 `NotImplemented` instead of HTTP 200 with `authResults[0].authDecision == "ALLOWED"`.
5. Repeat with `resources=["arn:aws:iot:<region>:<account>:topic/<path>"]` (SUBSCRIBE) and with `actionType="PUBLISH"` on the `topic/` ARN; both are refused 501 instead of returning `IMPLICIT_DENY` with no `allowed.policies`.
6. Replay `iot.test-authorization-subscribe-requires-topicfilter-resource`: it hangs in its eventual-consistency poll on the 501 and fails with `Check timed out after 60.0s`.
