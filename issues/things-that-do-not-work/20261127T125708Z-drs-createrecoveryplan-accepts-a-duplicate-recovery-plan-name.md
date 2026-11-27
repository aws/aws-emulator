Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: drs.create-recovery-plan-duplicate-name-rejected drs.create-recovery-plan-client-token-replay-returns-same-plan drs.create-recovery-plan-reused-client-token-other-name-rejected
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs.create-recovery-plan-duplicate-name-rejected|_refusal#5|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: drs.create-recovery-plan-client-token-replay-returns-same-plan|run#10|AssertionError|assert refused == taken, (
Issue-Key: drs.create-recovery-plan-reused-client-token-other-name-rejected|_refusal#7|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result.get('recoveryPlan')!r}")
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# drs CreateRecoveryPlan accepts a duplicate recovery-plan name and replays a client token spent on different parameters, instead of refusing both with ConflictException

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRecoveryPlan

## Description
Two net-new root causes in the drs CreateRecoveryPlan handler, both making the operation overly permissive: it never refuses with the modeled ConflictException, so two uniqueness/idempotency invariants of the real service are absent. Both live in services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/create_recovery_plan.rs.

Cause 1 of 2: a recovery-plan name is not unique in the Region — a second CreateRecoveryPlan under a live plan's name is accepted.
Failing checks: drs.create-recovery-plan-duplicate-name-rejected, drs.create-recovery-plan-client-token-replay-returns-same-plan.
AWS: Smithy (com.amazonaws.drs#CreateRecoveryPlan) models ConflictException, errorKind client, httpError 409, documented "The request could not be completed due to a conflict with the current state of the target resource." Recorded behavior: a CreateRecoveryPlan whose `name` is already held by a live plan in the Region is refused 409 ConflictException with message "A recovery plan with name '<name>' already exists." — the fixed text is "A recovery plan with name '" ... "' already exists." and the only variable part is the `name` member copied from the request. The refusal creates nothing, so ListRecoveryPlans still holds exactly one plan under that name. Deleting that plan frees the name: a later create with the same name is accepted and yields a NEW plan ARN (never the deleted plan's) whose GetRecoveryPlan reads back name == the request's name and status == "ACTIVE".
Emulator: the handler mints an id and inserts unconditionally (create_recovery_plan.rs: after the clientToken lookup it goes straight to next_id/insert); there is no scan of store.recovery_plans for an existing plan of the same name. The duplicate create returns 201 with a second plan. The suite reports: "expected a refusal, but the call succeeded: 'sim2real-behav-drs-dup-c1689a514e8f'", and in the token-replay check "a taken name under another client token must be refused as a conflict, got ('accepted', '')".
Why: a client relying on the name being a Region-unique key (the pattern of creating-or-reusing a plan by name, or treating a 409 as "already provisioned") instead silently ends up with two plans sharing one name; subsequent lookups by name are ambiguous and the duplicate is never cleaned up, and error-handling paths that branch on ConflictException are never exercised.
Fix: CreateRecoveryPlan's handler — add the name-uniqueness precondition over the recovery-plan store before minting the id, returning the modeled ConflictException.
Traces: drs.create-recovery-plan-duplicate-name-rejected (also asserts the post-delete re-create path); drs.create-recovery-plan-client-token-replay-returns-same-plan (the same-token replay itself is correct; the check fails only on the taken-name conflict).

Cause 2 of 2: a clientToken already spent on a plan with DIFFERENT request parameters is replayed as that plan instead of being refused.
Failing check: drs.create-recovery-plan-reused-client-token-other-name-rejected.
AWS: `clientToken` is modeled ClientIdempotencyToken, @idempotencyToken, length 0..64. Recorded behavior: reusing a token with a different `name` is refused 409 ConflictException with message "Idempotency token was already used with different request parameters" — that message is entirely fixed text, with no value interpolated from the request. The refused call creates nothing (the Region still holds only the plan the token originally minted), and the second name is accepted once a fresh token is supplied.
Emulator: create_recovery_plan.rs treats the token as identifying the request by itself — it looks up any stored plan whose client_token matches and returns that plan verbatim, explicitly documented in the module header as "a repeat whose other members differ is still that same request retried". No comparison of the stored plan's name/description/tags against the incoming request is performed. The reuse therefore answered 201 with the FIRST plan: {'recoveryPlanArn': '...recovery-plan/plan-...', 'name': '<the original name>', 'status': 'ACTIVE', ...} — i.e. the response describes a plan whose `name` is not the one the caller asked for.
Why: a client that reuses a token by mistake (a retry loop reusing a constant token, or a token keyed to a workflow rather than a request) gets a 201 describing a plan it did not ask for, with a different name, and believes its requested plan exists. AWS's 409 is the signal that makes that programming error visible; without it the divergence surfaces much later as a missing plan.
Fix: CreateRecoveryPlan's handler — on a clientToken hit, compare the stored plan's request parameters with the incoming ones and return the modeled ConflictException on mismatch, replaying only on an identical request. The module doc-comment asserting no parameter-mismatch refusal exists should be corrected along with it.
Traces: drs.create-recovery-plan-reused-client-token-other-name-rejected.

Neither cause is covered by the open drs issues (20261117T165722Z-drs-createrecoveryplanstep-is-a-501-scaffold-stub-so.md concerns CreateRecoveryPlanStep; 20261124T211436Z-drs-createreplicationconfigurationtemplate-omits-a-tag-entry-from-the.md concerns a different operation's tags map).

## Evidence
Cause 1 — duplicate name:
  Expected: HTTP 409, Error.Code "ConflictException", Error.Message "A recovery plan with name '<name from the request>' already exists."; no plan created; ListRecoveryPlans holds only the first plan's ARN under that name.
  Actual:   HTTP 201 with a second recoveryPlan under the same name. Suite assertion: "expected a refusal, but the call succeeded: 'sim2real-behav-drs-dup-c1689a514e8f'"; and "a taken name under another client token must be refused as a conflict, got ('accepted', '')".

Cause 2 — reused clientToken, different parameters:
  Expected: HTTP 409, Error.Code "ConflictException", Error.Message "Idempotency token was already used with different request parameters" (all fixed text); nothing created; the Region still holds only the originally named plan.
  Actual:   HTTP 201 replaying the first plan. Suite assertion: "expected a refusal, but the call succeeded: {'recoveryPlanArn': 'arn:aws:drs:<region>:<account>:recovery-plan/plan-<id>', 'name': '<the ORIGINAL plan's name, not the requested one>', 'status': 'ACTIVE', 'createdAt': ..., 'updatedAt': ..., 'tags': {...}}" — the response's `name` is not the `name` the request carried.

Model grounding: smithy_query(query="operation", operation="CreateRecoveryPlan") on services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/smithy-model/com-amazonaws-drs-elasticdisasterrecoveryservice.json — errors include ConflictException (client, httpError 409); input member clientToken is ClientIdempotencyToken with idempotencyToken: true, length 0..64; name is RecoveryPlanName, required, length 1..256, pattern ^[a-zA-Z0-9][a-zA-Z0-9 _-]*$; success status 201.
Emulator grounding: services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/create_recovery_plan.rs lines 184-202 (token lookup returns the stored plan with no parameter comparison) and lines 225-256 (unconditional mint-and-insert, no name-uniqueness check); the module header at lines 10-17 states the intended-but-diverging rule.

## Steps to Reproduce
Cause 1 (duplicate name):
1. drs.create_recovery_plan(name="sim2real-dup-1") -> 201, note recoveryPlanArn A.
2. drs.create_recovery_plan(name="sim2real-dup-1") again (no clientToken, or a fresh one).
   AWS: 409 ConflictException "A recovery plan with name 'sim2real-dup-1' already exists."
   Emulator: 201 with a second plan.
3. list_recovery_plans -> AWS lists one plan named "sim2real-dup-1" (ARN A); emulator lists two.
4. delete_recovery_plan(recoveryPlanArn=A); create_recovery_plan(name="sim2real-dup-1") -> accepted with a new ARN != A, GetRecoveryPlan reads name/status == ("sim2real-dup-1", "ACTIVE").

Cause 2 (reused clientToken with other parameters):
1. drs.create_recovery_plan(name="sim2real-tok-a", clientToken="reuse-1") -> 201.
2. drs.create_recovery_plan(name="sim2real-tok-b", clientToken="reuse-1").
   AWS: 409 ConflictException "Idempotency token was already used with different request parameters".
   Emulator: 201 whose recoveryPlan.name is "sim2real-tok-a".
3. list_recovery_plans -> only "sim2real-tok-a" is present in both; then create_recovery_plan(name="sim2real-tok-b", clientToken="fresh-1") is accepted and reads back ("sim2real-tok-b", "ACTIVE").

Replay: run_behavioral_suite and re-run the checks in trace_ids.
