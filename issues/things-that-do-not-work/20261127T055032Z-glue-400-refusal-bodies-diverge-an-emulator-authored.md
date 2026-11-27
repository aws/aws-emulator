Issue-Source: trace-sweep-divergence
Issue-Trace: glue-data-quality-statistic-annotation-and-batch-run-reads
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-data-quality-statistic-annotation-and-batch-run-reads glue-tsd-foreign-and-malformed-catalog-id
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|BatchGetDataQualityRulesetEvaluationRun|4xx->4xx|$.Message
Issue-Key: glue|GetTables|4xx->4xx|$.Message
Issue-Key: glue|GetTables|4xx->4xx|$.__type
Issue-Commit: 489c1197152cf1b4a01399a5eda62086fe4d180a

# Glue 400 refusal bodies diverge: an emulator-authored Message and the wrong error class for a non-local catalog id

> _Note: this issue was identified against the code base at commit `489c1197152cf1b4a01399a5eda62086fe4d180a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetDataQualityRulesetEvaluationRun

## Description
This bundle carries 2 causes. Both are Glue 400 refusal BODIES that the emulator authors itself instead of reproducing AWS's: one invents a Message where AWS sends none, the other picks the wrong exception class and an invented message for an id AWS treats as simply absent. A fixer must close both before submitting.

Cause 1 of 2 — BatchGetDataQualityRulesetEvaluationRun's 400 carries an emulator-authored Message.
AWS: Smithy (service-com-amazonaws-glue-awsglue): BatchGetDataQualityRulesetEvaluationRun input RunIds (DataQualityRulesetEvaluationRunIdList, required, @length 1..100), errors InvalidInputException / OperationTimeoutException / InternalServiceException; the model states NO format rule for a run id, so there is no modelled "must start with dqrun-" constraint to echo. Recorded behavior: AWS refuses the malformed run id with HTTP 400 and the same error class the emulator chose (the __type matched on replay), but the body carries NO Message member at all.
Emulator: HTTP 400 with an extra body member $.Message whose text is emulator-authored and describes the emulator's own prefix rule: "Invalid RunId: must start with 'dqrun-'". None of that text is AWS wording — it is wholly invented, including the 'dqrun-' prefix as a stated requirement.
Why: a client surfacing or matching on the error message sees a message AWS never sends, and a client that distinguishes "no message" from "message present" (log scraping, error-mapping layers, SDK e.getMessage() being null vs a string) behaves differently against the emulator than against AWS. It also advertises a run-id format rule that is not AWS's contract, so a caller may "fix" valid input.
Fix: the BatchGetDataQualityRulesetEvaluationRun refusal path (its input validation / error serializer) must emit the recorded 400 with no Message member rather than attaching a locally generated explanation.
Traces: glue-data-quality-statistic-annotation-and-batch-run-reads#task#2.

Cause 2 of 2 — GetTables rejects a well-formed non-local CatalogId as malformed instead of reporting the catalog absent.
AWS: Smithy: GetTables input CatalogId (CatalogIdString, @length 1..255, pattern allows the general printable range — it does NOT restrict the value to lowercase letters/digits/underscores/slashes), errors include EntityNotFoundException and InvalidInputException. Recorded behavior: for a catalog id that is syntactically acceptable but names a catalog this account does not hold (a foreign account id), AWS answers HTTP 400 with __type EntityNotFoundException and the fixed message text "Catalog not found." — the message has no variable part; it is that fixed sentence.
Emulator: HTTP 400 with __type InvalidInputException and the message "Catalog id is invalid. The catalog id must be prepended with the account id prefix and can only contain lowercase letters, numbers, underscores, and slashes." — i.e. it applies a format rule the model does not state and classifies an absent catalog as a malformed one.
Why: EntityNotFoundException and InvalidInputException are handled differently by clients — not-found is a resolvable/ignorable condition (create it, fall through to the local catalog, treat the cross-account catalog as unavailable), while invalid-input is a permanent caller bug that retry/validation layers refuse to retry and surface as a programming error. Cross-account catalog access against the emulator therefore fails with the wrong class and an unrecognisable message.
Fix: the GetTables catalog-id resolution (its validation stage in the Glue catalog component) must only reject values that violate the modelled CatalogIdString constraints, and resolve an unknown-but-well-formed catalog to EntityNotFoundException with AWS's fixed "Catalog not found." text. Related but distinct open issues on this operation: 20261115T110711Z-glue-5-net-new-causes-gettables-pagination-tokens.md (pagination/absent-database) and 20261102T132146Z-glue-answers-200-for-absent-entities-and-an.md (absent entities answered 200) — neither covers the catalog-id error class.
Traces: glue-tsd-foreign-and-malformed-catalog-id#task#7.

## Evidence
Cause 1 (BatchGetDataQualityRulesetEvaluationRun, glue-data-quality-statistic-annotation-and-batch-run-reads#task#2): expected 400 with no $.Message; actual 400 with $.Message = "Invalid RunId: must start with 'dqrun-'" (unexpected_present). Fixed text invented in full; no part of it is request- or state-derived.
Cause 2 (GetTables, glue-tsd-foreign-and-malformed-catalog-id#task#7): expected 400 $.__type = "EntityNotFoundException", $.Message = "Catalog not found." (entirely fixed text, no variable part); actual 400 $.__type = "InvalidInputException", $.Message = "Catalog id is invalid. The catalog id must be prepended with the account id prefix and can only contain lowercase letters, numbers, underscores, and slashes."

## Steps to Reproduce
Cause 1: call BatchGetDataQualityRulesetEvaluationRun with a RunIds entry that is not a valid run id -> emulator returns 400 with an invented Message; AWS returns the 400 with no Message member. Replay glue-data-quality-statistic-annotation-and-batch-run-reads#task#2.
Cause 2: call GetTables with DatabaseName set and CatalogId set to a well-formed catalog id belonging to another account (not the caller's) -> emulator returns 400 InvalidInputException with the catalog-id-format message; AWS returns 400 EntityNotFoundException "Catalog not found.". Replay glue-tsd-foreign-and-malformed-catalog-id#task#7.
