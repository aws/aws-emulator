Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|DeleteCustomDetectionRuleAssociation|ResourceNotFoundException
Issue-Commit: c3867ff5ad180ddb764d6f7e7211ec419ed4ca10

# DeleteCustomDetectionRuleAssociation is not idempotent: deleting an already-deleted or never-existent association is refused with ResourceNotFoundException instead of succeeding

> _Note: this issue was identified against the code base at commit `c3867ff5ad180ddb764d6f7e7211ec419ed4ca10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomDetectionRuleAssociation

## Description
This issue carries 1 root cause in the guardduty crate.

Cause 1 of 1: DeleteCustomDetectionRuleAssociation refuses a delete of an association that is not present, instead of answering an empty success.

AWS: Smithy `DeleteCustomDetectionRuleAssociation` is marked `@idempotent`, is bound as `DELETE /custom-detection-rule/rule/{RuleId}/association/{AssociationId}` with success code 200, and its output shape `DeleteCustomDetectionRuleAssociationResponse` declares no members (empty body). Its own documentation states, verbatim: "Disables a custom detection rule by deleting its association. This operation is idempotent." Recorded behavior confirms what that idempotency means observably: a second Delete of an AssociationId that was already deleted returns HTTP 200 with an empty body and no error, and a Delete naming a syntactically valid but never-existent RuleId/AssociationId pair likewise returns HTTP 200 with an empty body — unlike Create/Get/Update at the same URI, which all validate existence first and answer the modeled `ResourceNotFoundException` (httpError 404). Both input labels carry their own `@length`/`@pattern` constraints (RuleId 1..100 `^[a-z0-9]+(-[a-z0-9]+)*$`, AssociationId 1..64 `^[a-zA-Z0-9_-]{1,64}$`), so a well-formed-but-absent pair is a successful no-op, not a refusal.

Emulator: the handler resolves the (region, RuleId, AssociationId) triple and, when nothing resolves, returns the modeled 404 `ResourceNotFoundException` with the message template `Association not found: <AssociationId>` (fixed text `Association not found: `, plus the AssociationId taken from the request URI). The replay shows this on the SECOND delete of an association the check had just deleted successfully: `ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteCustomDetectionRuleAssociation operation: Association not found: <associationId>`. The handler's own comments assert that `@idempotent` is satisfied by "a replay removes nothing more" — that is not the AWS contract: AWS's idempotent DELETE also does not change its RESPONSE on replay.

Why: every client that retries a DELETE after a lost response, and every teardown/cleanup path that deletes unconditionally, now sees a hard 404 client error on the retry. A caller cannot distinguish "the first delete actually succeeded" from "the rule or association never existed", so idempotent reconcilers and test/cleanup harnesses abort or report spurious failures where real GuardDuty returns 200. It also makes the whole association lifecycle unverifiable past the delete step, because the documented double-delete is an error in the emulator.

Fix: the existence-check branch of the DeleteCustomDetectionRuleAssociation handler in the guardduty crate — the unresolved-pair path must return the empty `DeleteCustomDetectionRuleAssociationOutput` with status 200 rather than constructing the 404. Note that `ResourceNotFoundException` must still be the answer for Get/Update/Create on the same absent pair (those parts of the check pass today and must keep passing). Note also the open issues 20261029T183757Z-guardduty-lifecycle-state-is-wrong-missing-dry-run.md (Cause 1, the missing DRY_RUN ExpiresAt on this same check) and the resolved 20261029T190006Z-guardduty-custom-detection-rule-associations-carry-a-malformed-arn.md (Create/Get/List ARN and expiresAt): neither covers Delete idempotency, which is the defect here.

Traces: guardduty.custom-detection-rule-association-lifecycle.

## Evidence
Failing check guardduty.custom-detection-rule-association-lifecycle, verdict [error]:

  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteCustomDetectionRuleAssociation operation: Association not found: b306a1d908dbe6da35f254fae8a00002

The check reached this after the full lifecycle had already succeeded (create DRY_RUN -> get -> update to LIVE -> list -> delete), and after the post-delete Get and Update had both correctly answered ResourceNotFoundException with the message `Association not found: <associationId>`. The failing call is the check's documented idempotent re-delete:

  # idempotent-delete semantics: deleting an already-deleted association succeeds silently
  gd.delete_custom_detection_rule_association(RuleId=rule_id, AssociationId=association_id)

Expected: HTTP 200, empty body, no exception. Actual: HTTP 404, error code `ResourceNotFoundException`, message `Association not found: <AssociationId>` — the id is a per-run value from the request, the fixed text is `Association not found: `.

The same divergence applies to the check's later never-existent-rule case, which also requires a silent success:

  # DeleteCustomDetectionRuleAssociation is idempotent-delete semantics: deleting an
  # association that never existed succeeds silently (no ResourceNotFoundException),
  # unlike Create/Get/Update which all validate RuleId existence first.
  gd.delete_custom_detection_rule_association(RuleId=missing_rule_id, AssociationId=missing_association_id)

Emulator source confirming the cause: services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/delete_custom_detection_rule_association.rs lines 161-176 return `not_found(&input.association_id)` whenever `detection_rule_association_by_id(...)` is `None`; `not_found` at lines 220-232 builds the modeled 404. Smithy grounding: `DeleteCustomDetectionRuleAssociation` has `"idempotent": true`, `http {method: DELETE, code: 200}`, an output shape with no members, and documents "This operation is idempotent."

## Steps to Reproduce
1. Run the behavioral check guardduty.custom-detection-rule-association-lifecycle.
2. Or by hand against the emulator: ensure a detector exists (CreateDetector(Enable=true) if ListDetectors is empty), then pick a RuleId from ListCustomDetectionRules.
3. CreateCustomDetectionRuleAssociation(RuleId=<that rule>, Mode='DRY_RUN') and keep the returned AssociationId.
4. DeleteCustomDetectionRuleAssociation(RuleId=<rule>, AssociationId=<id>) — succeeds (200, empty body).
5. DeleteCustomDetectionRuleAssociation(RuleId=<rule>, AssociationId=<id>) AGAIN with the same ids: the emulator answers HTTP 404 ResourceNotFoundException 'Association not found: <id>'. Required: HTTP 200 with an empty body and no error, because the operation is @idempotent.
6. Also call DeleteCustomDetectionRuleAssociation(RuleId=<a syntactically valid, never-created rule id>, AssociationId=<a random 32-hex id>): the emulator answers the same 404; required is HTTP 200 with an empty body. For contrast, Get/Update/Create on that same absent pair must keep answering ResourceNotFoundException.
