Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.iot-deny-stringnotequals-sourcevpce-denies-implicitly-rejected iot.iot-sourceip-condition-denied-without-missing-context-rejected iot.iot-test-authorization-sourcevpce-missing-context-rejected iot.iot-connect-client-id-scoping-denies-other-client-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.iot-deny-stringnotequals-sourcevpce-denies-implicitly-rejected|run#7|AssertionError|assert result["authDecision"] == "IMPLICIT_DENY", (
Issue-Key: iot.iot-sourceip-condition-denied-without-missing-context-rejected|run#10|AssertionError|assert b["missingContextValues"] == [], (
Issue-Key: iot.iot-test-authorization-sourcevpce-missing-context-rejected|run#8|AssertionError|assert len(batched) == 1, (
Issue-Key: iot.iot-connect-client-id-scoping-denies-other-client-rejected|run#22|KeyError|assert denied["denied"]["explicitDeny"]["policies"] == [], (
Issue-Commit: dafb6a92f3304d60364f819845fa972c02211566

# iot TestAuthorization policy evaluation: 4 causes — condition keys mis-attributed, deny class inverted, batch not truncated, explicitDeny branch dropped

> _Note: this issue was identified against the code base at commit `dafb6a92f3304d60364f819845fa972c02211566`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestAuthorization

## Description
TestAuthorization answers HTTP 200 and builds a well-formed authResults list, but its policy-evaluation semantics and result projection diverge from AWS in four independent ways. All four are in the same operation's evaluation/serialization path but are separate defects: fixing the deny classification does not fix the batch truncation, and fixing either does not fix the missingContextValues attribution.

Cause 1 of 4: an unsatisfiable Deny condition is reported as an EXPLICIT_DENY instead of an IMPLICIT_DENY.
  AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json): TestAuthorization -> POST /test-authorization, output authResults: list<AuthResult>; AuthResult.authDecision -> AuthDecision enum (ALLOWED | EXPLICIT_DENY | IMPLICIT_DENY), AuthResult.missingContextValues -> MissingContextValues. Recorded behavior: for a policy that pairs an unconditional Allow with a Deny whose Condition is StringNotEquals on aws:SourceVpce, and a request context that carries NO aws:SourceVpce, AWS does not evaluate the negated operator as a match. The Deny statement is undecidable, so the result is authDecision=IMPLICIT_DENY, attributed to that policy under denied.implicitDeny, and the absent key is reported in missingContextValues. Status 200, no error.
  Emulator: authDecision='EXPLICIT_DENY', the policy is attributed under denied.explicitDeny, and missingContextValues=[] (replay: "Allow + Deny/StringNotEquals with no aws:SourceVpce in context was decided 'EXPLICIT_DENY', expected IMPLICIT_DENY ... 'missingContextValues': []"). The emulator treats a missing context value as satisfying StringNotEquals.
  Why: EXPLICIT_DENY and IMPLICIT_DENY mean opposite things to an operator: an explicit deny cannot be overridden by any further grant, an implicit deny is fixed by adding one. A client (or `aws iot test-authorization` used to debug a connection refusal) is told the policy actively forbids the action, when in fact the request context merely lacks the key the Deny is conditioned on. Tooling that refuses to proceed on an explicit deny blocks on the emulator and proceeds on AWS.
  Fix: the condition-evaluation step of the TestAuthorization handler — negated string operators (StringNotEquals/StringNotLike) over a context key that is absent must make the statement undecidable (recording the key in missingContextValues) rather than matching.
  Traces: iot.iot-deny-stringnotequals-sourcevpce-denies-implicitly-rejected.

Cause 2 of 4: a present-but-non-matching condition key is reported as a MISSING context value.
  AWS: recorded behavior only (the model constrains missingContextValues only as a list of strings). AWS's IoT authorizer always has aws:SourceIp in the request context, so an IpAddress condition that does not match is a non-match, not a missing value: the result is IMPLICIT_DENY with missingContextValues=[]. Only keys the authorizer genuinely cannot supply — aws:SourceVpce for a request that did not arrive over a VPC endpoint — appear in missingContextValues. Status 200.
  Emulator: for a policy whose only Condition is IpAddress on aws:SourceIp with a CIDR the caller is outside of, the refusal carries missingContextValues=['aws:SourceIp'] (replay: "the IpAddress-conditioned refusal reported missingContextValues=['aws:SourceIp'], expected an empty list").
  Why: missingContextValues is the single field that tells a caller whether to fix the policy (an unsatisfied condition) or to fix how the request is made (a key the caller never supplies). Reporting every unsatisfied condition key as missing makes the field useless and sends operators to debug a non-existent context problem.
  Fix: the same condition-evaluation step — populate missingContextValues only for keys the authorizer does not supply for the simulated request, and classify a supplied-but-non-matching key as a plain non-match.
  Traces: iot.iot-sourceip-condition-denied-without-missing-context-rejected.

Cause 3 of 4: the authResults batch is not truncated at the first undecidable authInfo.
  AWS: recorded behavior only (Smithy models authResults as a list with no length relation to the input authInfos). Recorded behavior: when an authInfo cannot be decided because the policy's condition key is absent from the context, AWS stops evaluating the batch and returns exactly ONE authResult — the first, undecidable one — regardless of how many authInfos were submitted. The recording submits four authInfos (CONNECT/PUBLISH/SUBSCRIBE/RECEIVE) against a policy pinned to aws:SourceVpce and receives 1 result.
  Emulator: one authResult per input authInfo — 4 results for 4 authInfos (replay: "TestAuthorization returned 4 results for 4 authInfos against a policy whose condition cannot be satisfied; real AWS truncates the batch at the first undecidable entry and returns exactly 1"). Status 200 in both cases.
  Why: a client that zips authResults positionally against the authInfos it sent, or that asserts a 1:1 correspondence, reads the emulator's extra entries as decided verdicts for actions AWS never evaluated — the opposite of the conservative "undecidable, stop here" signal AWS sends.
  Fix: the batch loop in the TestAuthorization handler — stop emitting results after the first authInfo whose evaluation reports a missing context value.
  Traces: iot.iot-test-authorization-sourcevpce-missing-context-rejected.

Cause 4 of 4: AuthResult.denied is serialized without its explicitDeny branch on an implicit-deny result.
  AWS: Smithy: AuthResult.denied -> Denied, with BOTH members implicitDeny -> ImplicitDeny and explicitDeny -> ExplicitDeny, each a structure carrying a policies list. Recorded behavior: on an evaluated result AWS always returns both branches, each with a (possibly empty) policies list — a resource mismatch comes back as denied={implicitDeny:{policies:[<policy>]}, explicitDeny:{policies:[]}}. The empty list is what says "nothing explicitly denied it". Status 200.
  Emulator: the denied map is emitted with implicitDeny only; reading denied.explicitDeny raises KeyError (replay: KeyError: 'explicitDeny' on a CONNECT whose client/<id> resource the policy does not name).
  Why: the caller cannot distinguish an explicit deny from an implicit one — exactly the distinction the two branches exist to carry — and strongly-typed SDK models that mark both branches present on an evaluated result fail to deserialize.
  Fix: the TestAuthorization response projection — serialize Denied with both implicitDeny and explicitDeny unconditionally, each with its possibly-empty policies list.
  Note on existing issues: open issue 20261113T164531Z-iot-testauthorization-omits-the-authresult-denied-projection.md describes this same projection defect from the wire corpus (its Cause 2) and may close with this cause; it does not carry this behavioral check's replay key, which is why the check is listed here. Open issue 20261108T182336Z-iot-testauthorization-stub.md (TestAuthorization answers 501) is stale against this replay — TestAuthorization answers 200 here.
  Traces: iot.iot-connect-client-id-scoping-denies-other-client-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service heading '### iot').

Cause 1 — check iot.iot-deny-stringnotequals-sourcevpce-denies-implicitly-rejected [mismatch]:
  AssertionError: Allow + Deny/StringNotEquals with no aws:SourceVpce in context was decided 'EXPLICIT_DENY', expected IMPLICIT_DENY: {'authInfo': {'actionType': 'PUBLISH', 'resources': ['arn:aws:iot:<region>:<account>:topic/sim2real/scn/t']}, 'allowed': {'policies': []}, 'denied': {'explicitDeny': {'policies': [{'policyName': '<policy>', 'policyArn': '<policy arn>'}]}}, 'authDecision': 'EXPLICIT_DENY', 'missingContextValues': []}
  Expected: authDecision 'IMPLICIT_DENY', the policy under denied.implicitDeny, missingContextValues ['aws:SourceVpce'].

Cause 2 — check iot.iot-sourceip-condition-denied-without-missing-context-rejected [mismatch]:
  AssertionError: the IpAddress-conditioned refusal reported missingContextValues=['aws:SourceIp'], expected an empty list — aws:SourceIp is a key the IoT authorizer always has, so an unsatisfied IpAddress condition is a non-match rather than a missing context value, and only the VPCE key is reported as missing.

Cause 3 — check iot.iot-test-authorization-sourcevpce-missing-context-rejected [mismatch]:
  AssertionError: TestAuthorization returned 4 results for 4 authInfos against a policy whose condition cannot be satisfied; real AWS truncates the batch at the first undecidable entry and returns exactly 1. Each emulator entry carried authDecision 'IMPLICIT_DENY' and missingContextValues ['aws:SourceVpce'].

Cause 4 — check iot.iot-connect-client-id-scoping-denies-other-client-rejected [error]:
  KeyError: 'explicitDeny' raised at `assert denied["denied"]["explicitDeny"]["policies"] == []` for a CONNECT whose client/<id> resource the attached policy does not name. The positive control (CONNECT as the named client id -> ALLOWED, attributed to the policy, missingContextValues []) passed, so the emulator does resolve the attachment; only the denied projection is short.

All four responses are HTTP 200, so the requirements at issue are the authDecision enum value, the contents of missingContextValues, the number of authResults entries, and the presence of both Denied branches. Policy names, certificate ARNs, account ids, regions and topic names in the output above are per-run values from the check's own setup, not part of any requirement.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — TestAuthorization (POST /test-authorization), AuthResult{authInfo, allowed, denied, authDecision, missingContextValues}, Denied{implicitDeny: ImplicitDeny, explicitDeny: ExplicitDeny}, AuthDecision enum ALLOWED|EXPLICIT_DENY|IMPLICIT_DENY.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. CreateKeysAndCertificate(setAsActive=true); CreatePolicy with an unconditional Allow on iot:Connect/Publish/Subscribe/Receive for Resource '*' plus a Deny on the same actions with Condition {"StringNotEquals": {"aws:SourceVpce": "vpce-00000000000000000"}}; AttachPolicy to the certificate ARN.
3. Call TestAuthorization(principal=<cert arn>, authInfos=[{actionType: PUBLISH, resources: [arn:aws:iot:<region>:<account>:topic/sim2real/scn/t]}]). Emulator answers authDecision EXPLICIT_DENY with missingContextValues []; AWS answers IMPLICIT_DENY with missingContextValues ['aws:SourceVpce'] (cause 1).
4. Replace the policy with a single Allow whose Condition is {"IpAddress": {"aws:SourceIp": "10.40.0.0/16"}} and repeat: the emulator reports missingContextValues ['aws:SourceIp'], AWS reports [] (cause 2).
5. With a policy pinned to {"StringEquals": {"aws:SourceVpce": "vpce-00000000000000000"}}, call TestAuthorization with four authInfos (CONNECT on client/<id>, PUBLISH on topic/..., SUBSCRIBE on topicfilter/..., RECEIVE on topic/...): the emulator returns 4 authResults, AWS returns 1 (cause 3).
6. With a policy allowing iot:Connect only on arn:aws:iot:<region>:<account>:client/<allowedId>, call TestAuthorization for a different client id and read authResults[0].denied: the emulator has no explicitDeny member, AWS returns {policies: []} (cause 4).
7. Re-replay the four checks in trace_ids to confirm all four causes are closed.
