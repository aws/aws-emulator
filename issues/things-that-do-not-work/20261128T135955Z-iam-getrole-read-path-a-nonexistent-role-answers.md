Issue-Source: trace-sweep-divergence
Issue-Trace: iam-trust-policy-size-quota
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-trust-policy-size-quota iam-err-malformed-policy-trust-policy iam-malformed-principal-syntax-create-role
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRole|4xx->2xx|$.ErrorResponse
Issue-Key: iam|GetRole|4xx->2xx|status
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.Description
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.RoleId
Issue-Related: 20261128T135955Z-iam-role-trust-policy-documents-are-re-serialized.md
Issue-Commit: 045aa7b4fc15c1aef135f1e9d9cb0cfc8b5fe67c

# iam GetRole read path: a nonexistent role answers 200, and Role.Description is dropped

> _Note: this issue was identified against the code base at commit `045aa7b4fc15c1aef135f1e9d9cb0cfc8b5fe67c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Two defects in the GetRole read path: its not-found gate does not fire, and its Role projection omits a member the role carries. There are 2 causes.

Cause 1 of 2: GetRole on a role name that does not exist answers 200 instead of 404 NoSuchEntity.
AWS: Smithy iam (2010-05-08): GetRole(RoleName: roleNameType, required) declares error NoSuchEntityException, errorKind client, httpError 404, awsQueryError code NoSuchEntity / httpResponseCode 404. The recorded response is HTTP 404 with the awsQuery ErrorResponse envelope: Error.Type 'Sender', Error.Code 'NoSuchEntity', Error.Message 'The role with name <RoleName> cannot be found.' — the fixed text is 'The role with name ' + ' cannot be found.'; the only variable part is <RoleName>, taken verbatim from the request's RoleName member.
Emulator: answers HTTP 200 with no ErrorResponse at all for a role name that was never created (the replay reports expected 404, got 200, and $.ErrorResponse missing).
Why: every client idiom for 'does this role exist' is a GetRole whose NoSuchEntity is caught — IaC import/refresh, create-if-absent provisioning, and the AssumeRole pre-flight of agent frameworks. A 200 makes a ghost role look real, so a provisioning step skips the CreateRole it owed and the later AssumeRole/PassRole fails far from the cause, and a delete-verification loop never terminates.
Fix: the GetRole handler's not-found gate — look the role up in state and raise NoSuchEntityException (404, code NoSuchEntity, the message above) when the name resolves to nothing, before projecting a Role. Note the resolved issue 20261103T132715Z covered GetRole answering 200 for a role that had been DELETED; this is the never-created name and is not covered by it.
Traces: iam-trust-policy-size-quota#task#2.

Cause 2 of 2: GetRole omits Role.Description for a role that has one.
AWS: Smithy iam: Role.Description -> roleDescriptionType (@length min 0 max 1000, pattern ^[\u0009\u000A\u000D\u0020-\u007E\u00A1-\u00FF]*$), documented 'A description of the role that you provide.' It is optional on the wire, but AWS emits it whenever the role carries one (set at CreateRole or by UpdateRoleDescription) — the recording for this step returns GetRoleResult.Role.Description with the role's description text.
Emulator: the member is absent from the GetRole projection entirely (replay: Description missing, expected the role's description, got None), even though the role was given one earlier in the same fixture.
Why: a client that reads back the description it set — drift detection, a console-style inventory, or an UpdateRoleDescription read-after-write — concludes the role has no description and either reports drift forever or re-issues the update on every pass.
Fix: the GetRole Role projection/serializer — emit Description from role state when it is present (and only then).
Traces: iam-err-malformed-policy-trust-policy#task#8 (also iam-malformed-principal-syntax-create-role#task#5).

Excluded from this issue, deliberately: the same cluster's Role.RoleId difference (expected value is an unresolved fixture template, actual is an emulator-minted unique id — a per-run value, and the minted-id class is already owned by 20261126T141645Z), and the cluster's AssumeRolePolicyDocument difference, which is downstream of the open issue 20261029T184359Z (malformed trust policies accepted instead of refused with MalformedPolicyDocument, so the stored document is the one AWS rejected).

## Evidence
Cause 1 — GetRole, fixture iam-trust-policy-size-quota, step task#2:
  expected: HTTP 404, body $.ErrorResponse = {Error: {Type: 'Sender', Code: 'NoSuchEntity', Message: 'The role with name <RoleName> cannot be found.'}, RequestId: <per-run>}
  actual:   HTTP 200, $.ErrorResponse absent
  (the recording's RoleName is the fixture's ghost role name; the requirement is the template above with RoleName copied from the request, not that literal name)

Cause 2 — GetRole, fixture iam-err-malformed-policy-trust-policy, step task#8 (also iam-malformed-principal-syntax-create-role#task#5), 200 vs 200:
  expected $.GetRoleResponse.GetRoleResult.Role.Description = the description the role was given earlier in the fixture ('sim2real probe' in this recording)
  actual   member missing (None)

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator built from this checkout.
2. Cause 1: fixture iam-trust-policy-size-quota, step task#2 — call GetRole with a RoleName that was never created. Expect 404 / Sender / NoSuchEntity / 'The role with name <RoleName> cannot be found.'; the emulator answers 200 with no ErrorResponse.
3. Cause 2: fixture iam-err-malformed-policy-trust-policy, step task#8 — create a role, give it a description, then GetRole and look for GetRoleResult.Role.Description; it is absent.
