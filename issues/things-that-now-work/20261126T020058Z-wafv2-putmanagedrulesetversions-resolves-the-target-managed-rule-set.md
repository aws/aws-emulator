Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.put-managed-rule-set-versions-invalid-version-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.put-managed-rule-set-versions-invalid-version-rejected|run#13|AssertionError|assert invalid_version_code == "ValidationException", (
Issue-Commit: d13ac0f0199fff89cf39280dd342767ab3065596

# wafv2 PutManagedRuleSetVersions resolves the target managed rule set before validating the VersionsToPublish map keys, so a malformed version key is refused WAFNonexistentItemException instead of ValidationException

> _Note: this issue was identified against the code base at commit `d13ac0f0199fff89cf39280dd342767ab3065596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutManagedRuleSetVersions

## Description
REOPENS 20261114T181430Z-wafv2-putmanagedrulesetversions-a-well-formed-associatedrulegrouparn-naming-no.md: its fix left the call refused. That fix (and the related fixed issue 20261120T102658Z-wafv2-putmanagedrulesetversions-misclassifies-an-invalid-managed-rule-set.md) moved the resolution of the put's TARGET ahead of the body's judgements, and it was moved ahead of the request-level map-key validation too — so a call whose VersionsToPublish key breaks VersionKeyString's shape is now refused with the not-found class instead of the validator's. This is a precedence regression in the same handler, not a new surface: the open issue 20261026T120000Z-wafv2-two-refusals-carry-the-right-class-but-not-wafs-own-sentence.md records that this very check previously got the class right (ValidationException) and differed only in the clause's TEXT; that issue's replay key for this check is the message assertion, so it does not own this earlier class failure.

This is ONE issue with ONE cause.

Cause 1 of 1: a VersionsToPublish map KEY that violates VersionKeyString's constraints is a request-validation failure WAF answers before it resolves anything in the store; the emulator resolves the targeted managed rule set first and never reaches the key check.

AWS: Smithy com.amazonaws.wafv2#AWSWAF_20190729 PutManagedRuleSetVersions takes VersionsToPublish, a map whose KEY targets VersionKeyString (@length 1..64, @pattern ^[\w#:\.\-/]+$). A key outside that form is a request-validation failure reported in the validator's own envelope — HTTP 400, error code `ValidationException` — not one of the operation's modelled errors (WAFInvalidParameterException / WAFNonexistentItemException / WAFInvalidOperationException / WAFOptimisticLockException, all client 400, plus WAFInternalErrorException 500). It is answered whatever the target names, i.e. ahead of the (Scope, Name, Id) lookup, because request validation precedes any state read. The recorded message for this class is `1 validation error detected: Value '<the map as the validator renders it>' at 'versionsToPublish' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[\w#:\.\-/]+$]` — the fixed text is `1 validation error detected: Value '` … `' at 'versionsToPublish' failed to satisfy constraint: Map keys must satisfy constraint: [` + the constraint set of the key shape + `]`; the echoed value is taken from the request (the offending map/key), and the member path from the input member name. Nothing is published either way.

Emulator: HTTP 400 with error code `WAFNonexistentItemException` and message `AWS WAF couldn’t perform the operation because your resource doesn’t exist.` The handler (services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/put_managed_rule_set_versions.rs) resolves `state.managed_rule_set(&input.scope, &input.name, &input.id)` and returns `nonexistent_item_exception()` at lines 242-259, and only AFTER that calls `crate::state::map_key_constraint_envelope(..., VERSION_KEY_CONSTRAINTS, ...)` at lines 267-274 — so for a request whose target does not exist (and a managed rule set can never be created by a customer, so every such call's target is necessarily absent), the map-key clause is unreachable and no ValidationException can ever be served for this member. The module doc at lines 11-25 states that target-first order explicitly, so this is the documented order being wrong for request-level validation rather than an accident.

Why: the two classes mean opposite things to a client. `ValidationException` says the request itself is malformed — fix the version key, do not retry. `WAFNonexistentItemException` says the resource is missing or still propagating — create it, or wait and retry. A vendor whose version key is a typo is told its managed rule set is missing, so its tooling either aborts on the wrong object or retries a call that can never succeed, and the one machine-readable statement of what is wrong (the member path `versionsToPublish` and the key constraint set) is never served at all.

Fix: the judgement ORDER in this operation's handler (src/ops/put_managed_rule_set_versions.rs): the request-only judgements — the VersionsToPublish map-key constraint envelope, and the `unacceptable_rule_group_reference` check that needs no store read — must run BEFORE the target `(Scope, Name, Id)` resolution, leaving only the store-dependent judgements (target existence → WAFNonexistentItemException, then well-formed-but-unresolvable rule-group references, then the lock token) after it. Update the module doc's stated order to match, and keep the existing behavior for the two fixed issues above: a target that does not exist must still be WAFNonexistentItemException whenever the request itself validates.

Traces: wafv2.put-managed-rule-set-versions-invalid-version-rejected.

## Evidence
Check wafv2.put-managed-rule-set-versions-invalid-version-rejected, verdict [mismatch]:

  check_wafv2_put_managed_rule_set_versions_invalid_version_rejected.py line 56:
  assert invalid_version_code == "ValidationException", f"got {invalid_version_code!r} instead"
  AssertionError: got 'WAFNonexistentItemException' instead

The call under test is PutManagedRuleSetVersions with Scope=REGIONAL, a Name/Id/LockToken no managed rule set carries, and VersionsToPublish={"not a valid version!!": {"AssociatedRuleGroupArn": "arn:aws:wafv2:<region>:<account>:regional/rulegroup/sim2real-nonexistent/00000000-0000-0000-0000-000000000000"}} — the ARN is well formed, so the version KEY is the only malformed input.

Expected: HTTP 400, Error.Code 'ValidationException', Error.Message "1 validation error detected: Value '{not a valid version!!=VersionToPublish(associatedRuleGroupArn=<arn>, forecastedLifetime=null)}' at 'versionsToPublish' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[\w#:\.\-/]+$]".
Actual: HTTP 400, Error.Code 'WAFNonexistentItemException', so the message assertion is never reached.

The preceding leg of the same check passes: GetManagedRuleSet on an unknown Id is refused WAFNonexistentItemException with the fixed sentence "AWS WAF couldn’t perform the operation because your resource doesn’t exist." The names, id, region and account in the request are this run's generated values; the requirement is the error class and the clause's fixed text with the request's own value echoed.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call PutManagedRuleSetVersions with Scope='REGIONAL', Name=<any EntityName>, Id=<any UUID>, LockToken=<any UUID>, and VersionsToPublish={"not a valid version!!": {"AssociatedRuleGroupArn": "arn:aws:wafv2:<region>:<account>:regional/rulegroup/<name>/<uuid>"}} — a key that breaks VersionKeyString's @pattern ^[\w#:\.\-/]+$ with an otherwise well-formed body.
3. Expect HTTP 400 with Error.Code 'ValidationException' and the "1 validation error detected: ... at 'versionsToPublish' failed to satisfy constraint: Map keys must satisfy constraint: [...]" clause.
4. Observed: HTTP 400 with Error.Code 'WAFNonexistentItemException' and the generic not-found sentence; the key is never validated.
5. Control: ListManagedRuleSets(Scope='REGIONAL') must not list that Name/Id afterwards, and ListAvailableManagedRuleGroups(Scope='REGIONAL') must stay non-empty (both already hold).
6. Or replay the check id in trace_ids.
