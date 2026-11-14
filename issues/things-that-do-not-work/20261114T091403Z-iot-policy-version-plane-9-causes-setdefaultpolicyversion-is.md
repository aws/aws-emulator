Issue-Source: trace-sweep-divergence
Issue-Trace: iot-policy-versioning-limits
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-policy-versioning-limits iot-principal-policy-lifecycle iot-principal-policy-thing-attachment-lifecycle iot-vpce-traj-fix-endpoint-via-policy-version iot-vpce-traj-lenient-documents-accepted
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|GetPolicyVersion|2xx->2xx|$.isDefaultVersion
Issue-Key: iot|ListPolicyVersions|2xx->2xx|$.policyVersions[*]
Issue-Key: iot|DeletePolicy|4xx->4xx|$.message
Issue-Key: iot|ListPolicyVersions|2xx->2xx|$.policyVersions[*].isDefaultVersion
Issue-Key: iot|ListPolicyVersions|2xx->2xx|$.policyVersions[*].versionId
Issue-Key: iot|DeletePolicy|2xx->4xx|status
Issue-Key: iot|AttachPrincipalPolicy|4xx->4xx|$.message
Issue-Key: iot|CreatePolicyVersion|4xx->2xx|$.message
Issue-Key: iot|CreatePolicyVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreatePolicyVersion|4xx->2xx|status
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot policy-version plane: 9 causes — SetDefaultPolicyVersion is not reflected, version lists are mis-ordered, DeletePolicyVersion leaves versions behind and malformed documents are accepted

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicyVersion

## Description
Nine net-new wire divergences in the IoT policy/policy-version plane. The default-version pointer is not applied, ListPolicyVersions is ordered the wrong way, version deletion does not take effect (so DeletePolicy then deadlocks on its own conflict), a malformed policy document is accepted, and two refusal messages are emulator-authored. Nine causes must be closed.

Cause 1 of 9: GetPolicyVersion reports isDefaultVersion false for the version SetDefaultPolicyVersion just selected.
  AWS: Smithy: SetDefaultPolicyVersion -> PATCH /policies/{policyName}/version/{policyVersionId} (output empty); GetPolicyVersion -> GET /policies/{policyName}/version/{policyVersionId} with output member isDefaultVersion (IsDefaultVersion, @default false). Recorded response after setting a version as default: 200 with isDefaultVersion true for that version.
  Emulator: 200 with isDefaultVersion false — the pointer write is not visible to the read.
  Why: the default version is what IoT actually evaluates for a principal; a client that sets it and verifies, or that reads which version is live, is told the change did not happen and may re-apply it forever.
  Fix: the policy state's default-version pointer (written by SetDefaultPolicyVersion, read by GetPolicyVersion/ListPolicyVersions).
  Traces: iot-policy-versioning-limits#task#10 (representative).

Cause 2 of 9: ListPolicyVersions returns versions oldest-first and with the wrong isDefaultVersion flags.
  AWS: Smithy: ListPolicyVersions -> GET /policies/{policyName}/version, output policyVersions (list of PolicyVersion: versionId, isDefaultVersion, createDate). Recorded response: the list is newest-version-first (versionId '5' at index 0 for a policy with versions 1..5) and exactly one entry carries isDefaultVersion true — the version last set as default (version '3' in the recording's index 2 position).
  Emulator: the list is oldest-first (versionId '1' at index 0) and the default flag sits on the most recently created version instead of the one SetDefaultPolicyVersion named.
  Why: a client that takes policyVersions[0] as the newest version, or scans for the default, picks the wrong version — and reads a different policy document than IoT would evaluate.
  Fix: the ListPolicyVersions serializer's ordering plus the same default-version pointer as cause 1.
  Traces: iot-policy-versioning-limits#task#13.

Cause 3 of 9: DeletePolicy's multi-version conflict message is emulator-authored.
  AWS: Smithy: DeletePolicy -> DELETE /policies/{policyName}, errors include DeleteConflictException (httpError 409). Recorded response: 409 with message 'The policy cannot be deleted as multiple versions of the policy exist (name=' + <policyName> + '). Use DeletePolicyVersion action to delete all non-default versions of the policy and retry.' Fixed text: 'The policy cannot be deleted as multiple versions of the policy exist (name=' , '). Use DeletePolicyVersion action to delete all non-default versions of the policy and retry.'; variable part: the policy name from the request.
  Emulator: 409 DeleteConflictException (status and code correct) with 'The policy cannot be deleted as the policy has one or more non-default versions (name=<policyName>)' — reworded and missing the remediation sentence that tells the caller what to do.
  Fix: the DeletePolicy handler's conflict message.
  Traces: iot-policy-versioning-limits#task#14.

Cause 4 of 9: deleting the non-default policy versions has no effect — ListPolicyVersions still returns all of them.
  AWS: Smithy: DeletePolicyVersion -> DELETE /policies/{policyName}/version/{policyVersionId} (output empty; DeleteConflictException 409 only for the default version). Recorded response afterwards: ListPolicyVersions returns exactly one entry — the surviving default version (versionId '3', isDefaultVersion true).
  Emulator: ListPolicyVersions still returns versions 1,2,3,4,5 with the default flag on '5' and index 0 at '1'; none of the deletions is reflected.
  Why: the documented 'delete all non-default versions, then delete the policy' sequence can never complete; the policy is undeletable.
  Fix: the DeletePolicyVersion handler/state (open issue 20261114T082236Z-iot-deletepolicyversion-stub.md reports DeletePolicyVersion as an unimplemented scaffold stub; this cluster is the wire-visible consequence on ListPolicyVersions and is not carried by that stub issue's key, so verify the listing after the stub is implemented).
  Traces: iot-policy-versioning-limits#task#18.

Cause 5 of 9: DeletePolicy on a policy whose non-default versions were deleted still answers 409 instead of deleting it.
  AWS: recorded response: 200 — with only the default version left, DeletePolicy succeeds.
  Emulator: 409 DeleteConflictException, because the stale non-default versions from cause 4 are still present.
  Why: teardown cannot remove the policy at all, so every later run leaks it and name reuse then conflicts.
  Fix: same state as cause 4; verify the DeletePolicy conflict predicate against the post-delete version set.
  Traces: iot-policy-versioning-limits#teardown#19 (cluster of 4: also iot-principal-policy-lifecycle#teardown#10, iot-principal-policy-thing-attachment-lifecycle#teardown#22, iot-vpce-traj-fix-endpoint-via-policy-version#teardown#17).

Cause 6 of 9: AttachPrincipalPolicy's not-found message names the policy, which AWS does not.
  AWS: Smithy: AttachPrincipalPolicy -> PUT /principal-policies/{policyName} (deprecated but bound), errors include ResourceNotFoundException (httpError 404). Recorded response: 404 with message 'Policy not found' — entirely fixed text, with no interpolated value.
  Emulator: 404 ResourceNotFoundException (status and code correct) with 'Policy <policyName> does not exist'.
  Fix: the AttachPrincipalPolicy handler's not-found message.
  Traces: iot-principal-policy-lifecycle#task#7, iot-principal-policy-thing-attachment-lifecycle#task#19.

Cause 7 of 9: CreatePolicyVersion accepts a policy document with an unknown Condition operator.
  AWS: Smithy: CreatePolicyVersion -> POST /policies/{policyName}/version; modeled errors include MalformedPolicyException (httpError 400). Recorded response: 400 with message 'Policy document is Malformed: Invalid Condition type : ' + <the unrecognised condition operator from the document> — fixed text 'Policy document is Malformed: Invalid Condition type : ', variable part the offending operator name.
  Emulator: 200, creating a new version from the malformed document; no message and no x-amzn-errortype.
  Why: the emulator stores and evaluates a policy AWS would have rejected, so authorization results diverge from AWS for every principal the policy is attached to, and the typo is never reported.
  Fix: CreatePolicyVersion's policy-document validation (the Condition block's operator set), shared with CreatePolicy.
  Traces: iot-vpce-traj-fix-endpoint-via-policy-version#task#6.

Cause 8 of 9: the same ListPolicyVersions ordering/default defect on a three-version policy in the VPC-endpoint trajectory.
  AWS: recorded response: policyVersions[0] is versionId '2' with isDefaultVersion true (newest-first, the default being the version last set), and the list holds only the versions that still exist.
  Emulator: policyVersions[0] is versionId '1' with isDefaultVersion false, and versions '2' and '3' trail with the default flag on '3'.
  Why: as cause 2 — in this trajectory the caller fixes a broken policy by creating and defaulting a new version, then reads the listing back and is shown the old version as current.
  Fix: as causes 1 and 2; listed separately because it is a distinct replay step that must be re-run.
  Traces: iot-vpce-traj-fix-endpoint-via-policy-version#task#12.

Cause 9 of 9: DeletePolicy answers 404 for a policy the trajectory created.
  AWS: recorded response: 200 — the policy exists at teardown and is deleted.
  Emulator: 404 ResourceNotFoundException; the policy record is not present under the name the trajectory created it with.
  Why: teardown fails and the caller cannot tell whether the policy was ever created; combined with cause 5 the plane is left in a state no AWS sequence can reach.
  Fix: the policy state's keying/retention across the lenient-document create path, then the DeletePolicy lookup.
  Traces: iot-vpce-traj-lenient-documents-accepted#teardown#13.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. All nine clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-policy-versioning-limits#task#10 (GetPolicyVersion), status 200:
  $.isDefaultVersion: expected True, got False

Cause 2 — iot-policy-versioning-limits#task#13 (ListPolicyVersions), status 200:
  $.policyVersions[0]: expected {'versionId': '5', 'isDefaultVersion': False, ...}, got {'versionId': '1', 'isDefaultVersion': False, ...}
  $.policyVersions[2]: expected {'versionId': '3', 'isDefaultVersion': True, ...}, got {'versionId': '3', 'isDefaultVersion': False, ...}
  $.policyVersions[4]: extra (expected None, got {'versionId': '5', 'isDefaultVersion': True, ...})

Cause 3 — iot-policy-versioning-limits#task#14 (DeletePolicy), status 409 as expected:
  $.message: expected 'The policy cannot be deleted as multiple versions of the policy exist (name=<policyName>). Use DeletePolicyVersion action to delete all non-default versions of the policy and retry.', got 'The policy cannot be deleted as the policy has one or more non-default versions (name=<policyName>)'

Cause 4 — iot-policy-versioning-limits#task#18 (ListPolicyVersions), status 200:
  $.policyVersions[0].isDefaultVersion: expected True, got False
  $.policyVersions[0].versionId: expected '3', got '1'
  $.policyVersions[1..4]: extra (expected None, got versions '2','3','4','5')

Cause 5 — iot-policy-versioning-limits#teardown#19 (DeletePolicy):
  expected status 200, got 409
  also affected: iot-principal-policy-lifecycle#teardown#10, iot-principal-policy-thing-attachment-lifecycle#teardown#22, iot-vpce-traj-fix-endpoint-via-policy-version#teardown#17

Cause 6 — iot-principal-policy-lifecycle#task#7 (AttachPrincipalPolicy), status 404 as expected:
  $.message: expected 'Policy not found', got 'Policy <policyName> does not exist'
  also affected: iot-principal-policy-thing-attachment-lifecycle#task#19

Cause 7 — iot-vpce-traj-fix-endpoint-via-policy-version#task#6 (CreatePolicyVersion):
  expected status 400, got 200
  $.message: missing (expected 'Policy document is Malformed: Invalid Condition type : StringEqualsNope', got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type, got None)

Cause 8 — iot-vpce-traj-fix-endpoint-via-policy-version#task#12 (ListPolicyVersions), status 200:
  $.policyVersions[0].isDefaultVersion: expected True, got False
  $.policyVersions[0].versionId: expected '2', got '1'
  $.policyVersions[1], [2]: extra (expected None, got '2' and '3', the default flag on '3')

Cause 9 — iot-vpce-traj-lenient-documents-accepted#teardown#13 (DeletePolicy):
  expected status 200, got 404

Policy names, createDate timestamps and the condition operator 'StringEqualsNope' above are per-run probe values; the requirements are the statuses, error codes, the list ordering and default-flag placement, and each message's fixed text plus the request value it interpolates.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — GetPolicyVersion (GET /policies/{policyName}/version/{policyVersionId}; isDefaultVersion default false), SetDefaultPolicyVersion (PATCH same path), ListPolicyVersions (GET /policies/{policyName}/version; policyVersions list), DeletePolicy (DELETE /policies/{policyName}; DeleteConflictException 409, ResourceNotFoundException 404), DeletePolicyVersion (DELETE /policies/{policyName}/version/{policyVersionId}; DeleteConflictException 409), CreatePolicyVersion (POST /policies/{policyName}/version; MalformedPolicyException 400), AttachPrincipalPolicy (PUT /principal-policies/{policyName}; ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. CreatePolicy(policyName=P, policyDocument=<valid>), then CreatePolicyVersion(P, ...) four more times so P has versions 1..5.
3. Cause 1: SetDefaultPolicyVersion(P, '3'), then GetPolicyVersion(P, '3') — expect isDefaultVersion true.
4. Cause 2: ListPolicyVersions(P) — expect newest-first ordering (versionId '5' first) with isDefaultVersion true only on '3'.
5. Cause 3: DeletePolicy(P) — expect 409 with AWS's multi-version message including the 'Use DeletePolicyVersion action ...' sentence.
6. Cause 4: DeletePolicyVersion(P, v) for every non-default v, then ListPolicyVersions(P) — expect exactly the default version.
7. Cause 5: DeletePolicy(P) — expect 200.
8. Cause 6: AttachPrincipalPolicy(policyName=<a name never created>, principal=<a certificate ARN>) and read Error.Message — expect 'Policy not found'.
9. Cause 7: CreatePolicyVersion(P, policyDocument=<a document whose Condition block uses an unknown operator such as StringEqualsNope>) — expect 400 MalformedPolicyException 'Policy document is Malformed: Invalid Condition type : <operator>'.
10. Cause 8: on a policy with three versions where version 2 was set as default, ListPolicyVersions and check index 0.
11. Cause 9: create a policy through the lenient-document path, then DeletePolicy it — expect 200.
12. Re-replay the representative step iot-policy-versioning-limits#task#10 and every fixture#step in trace_ids.
