Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorization-testing-apis
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorization-testing-apis
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|TestAuthorization|2xx->2xx|$.authResults[*].denied
Issue-Key: iot|TestAuthorization|2xx->2xx|$.authResults[*].denied.explicitDeny
Issue-Commit: 87e7668771aa8c29b5e1dffb34be8842e4b92b4f

# iot TestAuthorization omits the AuthResult.denied projection

> _Note: this issue was identified against the code base at commit `87e7668771aa8c29b5e1dffb34be8842e4b92b4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestAuthorization

## Description
TestAuthorization answers HTTP 200 with a well-formed `authResults` list, but the emulator does not project the `denied` branch of each `AuthResult`: it is either absent entirely, or present with only one of its two sub-structures. Two distinct causes, both in the same response projection.

Cause 1 of 2: `AuthResult.denied` is omitted from every entry of `authResults`.
  AWS: Smithy: TestAuthorization output `authResults` -> list of `AuthResult`; `AuthResult.denied` targets structure `Denied` ("The policies and statements that denied the specified action" / "Contains information that denied the authorization"), whose members are `implicitDeny` -> `ImplicitDeny` and `explicitDeny` -> `ExplicitDeny`, each a structure with a `policies` list. Recorded behavior: for an authInfo that is evaluated and not matched by any deny statement, AWS still RETURNS the `denied` structure, populated with both branches and empty policy lists -- `denied: {explicitDeny: {policies: []}, implicitDeny: {policies: []}}`. The presence of `denied` (and of both of its branches) is not conditional on something having been denied; the empty `policies` lists are what communicate "nothing denied it". Status 200, no error.
  Emulator: the `denied` key is missing from the `authResults[0]` object altogether (replay: `$.authResults[0].denied: missing (expected {'explicitDeny': {'policies': []}, 'implicitDeny': {'policies': []}}, got None)`), status 200.
  Why: a client that reads `authResults[i].denied.explicitDeny.policies` to report WHY a request was refused -- or merely to distinguish an explicit deny from an implicit one -- dereferences a null/absent structure and either throws or silently treats every result as "no deny information available". Strongly-typed SDKs that model `denied` as always-present on an evaluated result (and policy-debug tooling built on `iot test-authorization`) break on the emulator but work against AWS.
  Fix: the TestAuthorization response serializer / projection in that operation's handler, `services/service-com-amazonaws-iot-awsiotservice/src/ops/test_authorization.rs` -- the per-authInfo result must always emit `denied` with both `implicitDeny` and `explicitDeny` present, each carrying a `policies` list that is empty when nothing contributed to a deny, rather than skipping the member when the deny set is empty.
  Traces: iot-authorization-testing-apis#task#4.

Cause 2 of 2: when `denied` IS emitted, its `explicitDeny` branch is dropped.
  AWS: Smithy: `Denied` has BOTH `implicitDeny` (`ImplicitDeny`, "Policies that don't contain a matching allow or deny statement...") and `explicitDeny` (`ExplicitDeny`, "Information that explicitly denies the authorization"). Recorded behavior: `denied.explicitDeny` is returned as `{policies: []}` when no explicit deny statement matched -- the branch is present with an empty list, not omitted. Status 200, no error.
  Emulator: `denied` is serialized without the `explicitDeny` member (replay: `$.authResults[0].denied.explicitDeny: missing (expected {'policies': []}, got None)`), status 200. So the two causes are not one bug seen twice: one path drops the whole `denied` structure, the other drops one of its two branches, and a fix that only stops skipping empty `denied` still leaves `explicitDeny` absent.
  Why: a client cannot tell an explicit deny (a policy that says Deny, which no additional grant can override) from an implicit one (no matching statement, which a new grant fixes) -- exactly the distinction this field exists to carry. Code that branches on `denied.explicitDeny.policies.isEmpty()` sees an absent member and mis-classifies every refusal as implicit.
  Fix: same projection in the same handler -- serialize `Denied` with both branches unconditionally, each with its (possibly empty) `policies` list.
  Traces: iot-authorization-testing-apis#task#5.

Note on existing issues: the open issue `20261108T182336Z-iot-testauthorization-stub.md` ("Implement iot TestAuthorization: unimplemented scaffold stub") describes TestAuthorization answering HTTP 501 as a scaffold. It does NOT cover these divergences and does not carry their replay keys: in this replay TestAuthorization is implemented and answers 200, and the fault is in the response projection it builds. The resolved issue `20261110T131727Z-iot-testauthorization-never-decides-...` concerned the authDecision/topicfilter namespace and likewise does not cover the `denied` projection.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 -- fixture `iot-authorization-testing-apis`, step `task#4`, operation TestAuthorization:
  expected status 200, actual status 200
  $.authResults[0].denied: missing (expected {'explicitDeny': {'policies': []}, 'implicitDeny': {'policies': []}}, got None)
  divergence_key: service=iot|TestAuthorization|status=200->200|keys=$.authResults[0].denied

Cause 2 -- fixture `iot-authorization-testing-apis`, step `task#5`, operation TestAuthorization:
  expected status 200, actual status 200
  $.authResults[0].denied.explicitDeny: missing (expected {'policies': []}, got None)
  divergence_key: service=iot|TestAuthorization|status=200->200|keys=$.authResults[0].denied.explicitDeny

No HTTP status, error code or error message diverges here -- both steps return 200 -- so the requirement at issue is the response shape: `AuthResult.denied` present on every evaluated result, with both `implicitDeny` and `explicitDeny` present and each carrying a `policies` list (empty when nothing denied). The empty lists are structural, not values copied from a recording; the policy names that would populate them come from the account's attached policies and the request's authInfos.

Smithy model consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json -- TestAuthorization (POST /test-authorization, output TestAuthorizationResponse.authResults: AuthResults), AuthResult.denied -> Denied, Denied.implicitDeny -> ImplicitDeny, Denied.explicitDeny -> ExplicitDeny, both with member `policies` -> Policies.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an AWS SDK/CLI iot client at it.
2. Create an IoT policy and attach it to a principal (or run fixture `iot-authorization-testing-apis` up to its TestAuthorization steps).
3. Call TestAuthorization with that principal and one or more authInfos that are NOT denied by any statement, e.g.:
   aws iot test-authorization --principal <certificate-or-cognito-principal> --auth-infos actionType=PUBLISH,resources=arn:aws:iot:us-west-2:000000000000:topic/<topic>
4. Inspect the response: AWS returns each authResults[i] with denied = {"implicitDeny": {"policies": []}, "explicitDeny": {"policies": []}}. The emulator returns the result with no `denied` key at all (cause 1).
5. Repeat for the request shape exercised by step task#5 of the same fixture (authInfos that produce an implicit-deny result): the emulator emits `denied` but with no `explicitDeny` member (cause 2).
6. Re-replay fixture iot-authorization-testing-apis steps task#4 and task#5 to confirm both causes are closed.
