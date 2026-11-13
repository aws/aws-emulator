Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.checkcapacity-wcu-cost-model-and-ceiling wafv2.create-webacl-duplicate-rule-name-within-webacl-rejected wafv2.webacl-duplicate-rule-priority-rejected wafv2.web-acl-custom-response-body-key-missing-rejected wafv2.webacl-description-non-ascii-rejected wafv2.webacl-unicode-description-round-trip wafv2.rule-action-override-unknown-rule-name-asymmetry wafv2.managed-rule-group-pinned-version-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.checkcapacity-wcu-cost-model-and-ceiling|_error_stable#22|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: wafv2.create-webacl-duplicate-rule-name-within-webacl-rejected|_error#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: wafv2.webacl-duplicate-rule-priority-rejected|_error_stable#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: wafv2.web-acl-custom-response-body-key-missing-rejected|_error_code#16|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Key: wafv2.webacl-description-non-ascii-rejected|_error_code#16|AssertionError|raise AssertionError(
Issue-Key: wafv2.webacl-unicode-description-round-trip|run#13|AssertionError|raise AssertionError(
Issue-Key: wafv2.rule-action-override-unknown-rule-name-asymmetry|_error_code#15|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: wafv2.managed-rule-group-pinned-version-nonexistent-rejected|_error_code#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: de414c514180eddfb96a534a3f6304a808cc952a

# wafv2 write-path validation is over-permissive: seven CreateWebACL/UpdateWebACL/CheckCapacity requests WAF refuses are answered 200

> _Note: this issue was identified against the code base at commit `de414c514180eddfb96a534a3f6304a808cc952a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
Seven distinct semantic preconditions on the wafv2 web-ACL write path are not checked: the emulator commits the write and answers HTTP 200 where AWS WAF refuses with a modeled 400. This is ONE issue with SEVEN causes; every cause below must be closed before the fix is submitted. (The open issue 20261029T192556Z-wafv2-request-validation-never-refuses-ten-operations-answer covers a DIFFERENT set of ten operations — ListWebACLs/ListAPIKeys scope, CreateAPIKey token-domain syntax, GetSampledRequests TimeWindow, ListSettlementRecords filters, PutLoggingConfiguration ARN, DeletePermissionPolicy, Associate/Get/DisassociateWebACL — and carries none of the replay keys below.)

Cause 1 of 7: CreateWebACL does not enforce the per-web-ACL WCU capacity ceiling.
AWS: Smithy: CreateWebACL.Rules targets `Rules`; the operation models WAFLimitsExceededException and WAFInvalidParameterException (both client, httpError 400). A rule set whose computed capacity exceeds the per-web-ACL WCU ceiling is refused with HTTP 400, error code `WAFInvalidParameterException`, and the message `Error reason: You exceeded the capacity limit for a rule group or web ACL., field: WEB_ACL, parameter: <total>` — fixed text is `Error reason: You exceeded the capacity limit for a rule group or web ACL., field: WEB_ACL, parameter: `, and the variable part is the computed WCU total of the submitted Rules (the same figure CheckCapacity returns for them). The web ACL must not appear in ListWebACLs afterwards.
Emulator: HTTP 200 with a normal CreateWebACL Summary (Name/Id/LockToken/ARN) for a rule set the check had just priced above 5000 WCU via CheckCapacity.
Why: WCU is a hard account limit. A client sizes and ships a web ACL the emulator accepted, and AWS rejects it at deploy time; capacity-guard code in the client is never exercised.
Fix: the capacity precondition in the CreateWebACL handler's validation, computing the total through the same WCU accounting CheckCapacity uses.
Traces: wafv2.checkcapacity-wcu-cost-model-and-ceiling.

Cause 2 of 7: CreateWebACL accepts two rules carrying the identical Rule.Name.
AWS: Smithy: CreateWebACL.Rules is a list of Rule, each with required Name (EntityName, @length 1..128, @pattern ^[\w\-]+$); errors include WAFInvalidParameterException (client, 400). Two rules with the same Name in one request are refused with HTTP 400, code `WAFInvalidParameterException`, message `Error reason: You have duplicated some of the information in the parameter., field: RULE, parameter: <rule name>` — fixed text through `field: RULE, parameter: `, variable part the duplicated Rule.Name from the request. Note this is WAFInvalidParameterException, not the resource-level WAFDuplicateItemException.
Emulator: HTTP 200 and a web ACL is created.
Why: rule names are the identity a client uses for overrides, metrics and GetSampledRequests attribution; silently de-duplicating or last-write-wins produces a web ACL whose rule set does not match what was sent, and the divergence surfaces only against real AWS.
Fix: a duplicate-Name check over Rules in the CreateWebACL handler's validation (and the shared validation UpdateWebACL uses).
Traces: wafv2.create-webacl-duplicate-rule-name-within-webacl-rejected.

Cause 3 of 7: CreateWebACL accepts two rules sharing the same Rule.Priority.
AWS: Smithy: Rule.Priority is a required integer; CreateWebACL models WAFInvalidParameterException (400). Duplicate priorities are refused with HTTP 400, code `WAFInvalidParameterException`, message `Error reason: You have a duplicate priority. Priorities must be unique., field: RULE, parameter: <priority>` — fixed text through `field: RULE, parameter: `, variable part the duplicated Priority value from the request. No web ACL is left behind.
Emulator: HTTP 200 with a Summary; the web ACL is created with two rules at the same priority.
Why: Priority is the only thing that orders rule evaluation. A web ACL with ambiguous ordering evaluates non-deterministically, so a client's block/count expectations are untestable, and AWS refuses the same configuration.
Fix: a duplicate-Priority check over Rules in the CreateWebACL handler's validation, shared with UpdateWebACL.
Traces: wafv2.webacl-duplicate-rule-priority-rejected.

Cause 4 of 7: UpdateWebACL accepts a CustomResponseBodyKey that names no entry in the web ACL's CustomResponseBodies map.
AWS: Smithy: UpdateWebACL.CustomResponseBodies is a map (@length min 1) and CustomResponse.CustomResponseBodyKey is documented as "one of the keys in the CustomResponseBodies map that you provide for this web ACL"; UpdateWebACL models WAFInvalidParameterException (400). A dangling key is refused with HTTP 400, code `WAFInvalidParameterException`, message `Error reason: AWS WAF couldn't find a resource that a parameter references., field: CUSTOM_RESPONSE, parameter: <key>` — fixed text through `field: CUSTOM_RESPONSE, parameter: `, variable part the unresolvable key from the request. This holds both when the key is renamed and when CustomResponseBodies is dropped out from under a live reference (UpdateWebACL is a full replace). The stored rules, bodies and LockToken must all be unchanged after the refusal.
Emulator: HTTP 200 with a fresh NextLockToken; the dangling reference is committed and the LockToken rotates.
Why: the intra-document reference is the only thing that binds a Block action to the body it serves. A committed dangling key means the edge has no body to serve for a blocked request, and the client gets no signal at configure time.
Fix: CustomResponseBodyKey resolution against the request's CustomResponseBodies in the UpdateWebACL (and CreateWebACL) handler's validation, before the web ACL is written to state.
Traces: wafv2.web-acl-custom-response-body-key-missing-rejected.

Cause 5 of 7: the Description pattern constraint is not enforced, so a non-ASCII Description is stored instead of refused.
AWS: Smithy: CreateWebACL.Description and UpdateWebACL.Description both target EntityDescription — @length 1..256 and @pattern ^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$ (ASCII \w plus a small punctuation/whitespace set), deliberately narrower than the Unicode-aware pattern on Tag Key/Value in the same model. A Description containing multi-byte characters violates that pattern and is refused as a request-shape error: HTTP 400, error code `ValidationException` (not the WAF-specific WAFInvalidParameterException), with the standard constraint-violation sentence naming the member path and the failed pattern. The prior state (the ASCII Description) and the LockToken must be unchanged.
Emulator: both paths answer HTTP 200 — CreateWebACL returns a Summary for a non-ASCII Description, and UpdateWebACL returns a fresh NextLockToken and commits it.
Why: a client that stores localized descriptions succeeds locally and fails at the first real AWS call; worse, the emulator's accepted value is unrepresentable in AWS so the configuration cannot be migrated.
Fix: pattern/length enforcement for EntityDescription in the shared request validation that CreateWebACL and UpdateWebACL (and every other EntityDescription-taking operation) run before the handler.
Traces: wafv2.webacl-description-non-ascii-rejected (UpdateWebACL leg), wafv2.webacl-unicode-description-round-trip (CreateWebACL leg).

Cause 6 of 7: UpdateWebACL accepts a RuleActionOverride naming a rule that the referenced CUSTOMER-owned rule group does not contain.
AWS: Smithy: RuleGroupReferenceStatement carries RuleActionOverrides; UpdateWebACL models WAFInvalidParameterException (400). For a customer-owned rule group WAF resolves each override's Name against the group's rules and refuses an unknown one with HTTP 400, code `WAFInvalidParameterException`, message `Error reason: AWS WAF couldn't find a resource that a parameter references., field: RULE_GROUP_REFERENCE_STATEMENT, parameter: <override rule name>` — fixed text through `field: RULE_GROUP_REFERENCE_STATEMENT, parameter: `, variable part the override's Name from the request. The asymmetry matters: the same unknown name inside a ManagedRuleGroupStatement's RuleActionOverrides IS accepted and ignored, so the validation must be scoped to customer-owned rule-group references only. Stored overrides and LockToken unchanged after the refusal.
Emulator: HTTP 200 with a fresh NextLockToken; the unresolvable override is committed.
Why: overrides are how every staged WAF rollout downgrades a Block to Count. A silently-accepted typo leaves a rule the operator believes is in Count still Blocking (or the reverse) — a security-relevant divergence with no configure-time signal.
Fix: resolve RuleActionOverrides[].Name against the referenced rule group's rules in the UpdateWebACL/CreateWebACL validation, for RuleGroupReferenceStatement only.
Traces: wafv2.rule-action-override-unknown-rule-name-asymmetry.

Cause 7 of 7: a ManagedRuleGroupStatement pinned to a Version that is not in the catalog is accepted by both CheckCapacity and CreateWebACL.
AWS: Smithy: ManagedRuleGroupStatement.Version targets VersionKeyString (@length 1..64, @pattern ^[\w#:\.\-/]+$); CheckCapacity and CreateWebACL both model WAFNonexistentItemException (client, 400). A syntactically valid but unpublished version is refused by BOTH operations with HTTP 400, code `WAFNonexistentItemException`, and WAF's fixed not-found sentence as the message: `AWS WAF couldn't perform the operation because your resource doesn't exist.` (with the typographic apostrophe, no variable parts). No web ACL may be created.
Emulator: CheckCapacity answers HTTP 200 with a Capacity figure, and CreateWebACL creates the web ACL.
Why: the customer believes the web ACL is pinned to a reviewed rule-set version when the reference resolves to nothing — silent over-permissiveness on the managed-rule surface, and the pin cannot be reproduced on AWS.
Fix: resolve ManagedRuleGroupStatement.Version against the managed-rule-group version catalog in the shared rule-statement validation used by the CheckCapacity and CreateWebACL/UpdateWebACL handlers.
Traces: wafv2.managed-rule-group-pinned-version-nonexistent-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator (137/194 passed). All eight checks below are grounded failures in that report; none is marked ALREADY FILED or OWNED.

Cause 1 — wafv2.checkcapacity-wcu-cost-model-and-ceiling: "AssertionError: expected the call to be rejected, but it succeeded: {'Summary': {'Name': 'sim2real-behav-waf-acl-...', 'Id': ..., 'LockToken': ..., 'ARN': 'arn:aws:wafv2:us-west-2:000000000000:regional/webacl/...'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}". The check first priced the same Rules via CheckCapacity and asserted the total exceeded 5000 before calling CreateWebACL. The names/Ids/ARN above are this run's generated values; the requirement is the 400 + code + message template.

Cause 2 — wafv2.create-webacl-duplicate-rule-name-within-webacl-rejected: "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'Summary': {...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; the check expected code WAFInvalidParameterException and message "Error reason: You have duplicated some of the information in the parameter., field: RULE, parameter: <dup rule name>".

Cause 3 — wafv2.webacl-duplicate-rule-priority-rejected: "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'Summary': {'Name': 'sim2real-behav-waf-acl-dup-...'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; expected WAFInvalidParameterException with "Error reason: You have a duplicate priority. Priorities must be unique., field: RULE, parameter: <priority>". The positive control in the same file (distinct priorities 7 and 8) passed, so the acceptance is attributable to the duplicate priority alone.

Cause 4 — wafv2.web-acl-custom-response-body-key-missing-rejected: "AssertionError: expected a rejection, but the call succeeded: {'NextLockToken': '61a4398f-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" on UpdateWebACL with CustomResponseBodyKey='undefined-body-key' while CustomResponseBodies held only 'blocked-html'. Expected 400 WAFInvalidParameterException with "Error reason: AWS WAF couldn't find a resource that a parameter references., field: CUSTOM_RESPONSE, parameter: undefined-body-key". The valid configuration round-tripped first (CustomResponseBodies, ResponseCode 403, ResponseHeaders, CustomRequestHandling InsertHeaders all correct), so only the dangling-reference check is missing.

Cause 5 — wafv2.webacl-description-non-ascii-rejected: "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'NextLockToken': 'c1a2b790-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" for UpdateWebACL with Description='网站防火墙 描述二 v2'; expected code ValidationException. wafv2.webacl-unicode-description-round-trip: "AssertionError: CreateWebACL accepted a Description outside the documented ASCII-only pattern instead of rejecting it with ValidationException". The ASCII positive control ('Regional WAF ACL, desc-01 v1.0 test_env') round-tripped byte-for-byte.

Cause 6 — wafv2.rule-action-override-unknown-rule-name-asymmetry: "AssertionError: expected the call to be refused, but it succeeded: {'NextLockToken': '8bda768d-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}". The check had already proven the VALID override round-trips through GetWebACL, then changed only the override's rule Name. Expected 400 WAFInvalidParameterException with "Error reason: AWS WAF couldn't find a resource that a parameter references., field: RULE_GROUP_REFERENCE_STATEMENT, parameter: <unknown rule name>".

Cause 7 — wafv2.managed-rule-group-pinned-version-nonexistent-rejected: "AssertionError: expected a rejection, but the call SUCCEEDED and returned {'Capacity': 700, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" for CheckCapacity with ManagedRuleGroupStatement{VendorName=AWS, Name=AWSManagedRulesCommonRuleSet, Version='Version_9.99'}. The check first confirmed via ListAvailableManagedRuleGroupVersions that this version is not published and that the same statement at CurrentDefaultVersion is accepted. Expected 400 WAFNonexistentItemException with the fixed not-found sentence, on CheckCapacity and on CreateWebACL.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite (run_behavioral_suite); the eight check ids in trace_ids reproduce these seven causes.
2. Cause 1: CheckCapacity a rule set of many OrStatement-of-SqliMatchStatement rules until the reported total exceeds 5000, then CreateWebACL with those Rules -> expect 400 WAFInvalidParameterException (field WEB_ACL, parameter = the total); emulator returns 200 and creates the web ACL.
3. Cause 2: CreateWebACL REGIONAL with two Rules sharing one Name at priorities 0 and 1 -> expect 400 WAFInvalidParameterException (field RULE, parameter = the name); emulator returns 200.
4. Cause 3: CreateWebACL REGIONAL with two Rules both at Priority 7 -> expect 400 WAFInvalidParameterException (field RULE, parameter = 7); emulator returns 200.
5. Cause 4: CreateWebACL with CustomResponseBodies {'blocked-html': ...} and a Block rule referencing that key; then UpdateWebACL with the rule referencing 'undefined-body-key' (and separately, with CustomResponseBodies omitted entirely) -> expect 400 WAFInvalidParameterException (field CUSTOM_RESPONSE) and an unchanged LockToken; emulator returns 200 and rotates the LockToken.
6. Cause 5: CreateWebACL with Description='网站防火墙 描述二 v2', and UpdateWebACL changing an ASCII Description to that value -> expect 400 ValidationException on both; emulator accepts both with 200.
7. Cause 6: CreateRuleGroup with one rule named R, CreateWebACL referencing it with RuleActionOverrides [{Name: R, ActionToUse: Count}], then UpdateWebACL changing only the override Name to one the group does not have -> expect 400 WAFInvalidParameterException (field RULE_GROUP_REFERENCE_STATEMENT); emulator returns 200. Then confirm the asymmetry still holds: the same unknown name inside a ManagedRuleGroupStatement's RuleActionOverrides must still be ACCEPTED.
8. Cause 7: CheckCapacity and CreateWebACL with ManagedRuleGroupStatement Version='Version_9.99' for AWS/AWSManagedRulesCommonRuleSet -> expect 400 WAFNonexistentItemException on both; emulator returns 200 (Capacity 700) and creates the web ACL.
