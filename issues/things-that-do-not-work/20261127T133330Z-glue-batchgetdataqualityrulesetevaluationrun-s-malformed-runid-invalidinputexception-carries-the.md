Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.data-quality-ruleset-evaluation-run-lookup-rejections
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.data-quality-ruleset-evaluation-run-lookup-rejections|run#22|AssertionError|assert batch_malformed == ("InvalidInputException", ""), (
Issue-Commit: 32ed79bd1d8f0292c4a2e6bcda2466089447c235

# glue: BatchGetDataQualityRulesetEvaluationRun's malformed-RunId InvalidInputException carries the single-run read's message text; AWS sends none

> _Note: this issue was identified against the code base at commit `32ed79bd1d8f0292c4a2e6bcda2466089447c235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetDataQualityRulesetEvaluationRun

## Description
One net-new root cause on the glue emulator, surfaced by the only glue behavioral failure the replay reports as unowned (no OWNED-by marker, no unimplemented-stub marker).

Cause 1 of 1 (severity MEDIUM): BatchGetDataQualityRulesetEvaluationRun refuses a RunId outside the issued 'dqrun-' grammar with the correct class (InvalidInputException, HTTP 400) but attaches the SINGLE-run read's message text to it. AWS's batch refusal carries no message member at all, so the two refusals are not interchangeable.
Failing check: glue.data-quality-ruleset-evaluation-run-lookup-rejections.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): BatchGetDataQualityRulesetEvaluationRun input RunIds (DataQualityRulesetEvaluationRunIdList, @required, @length 1..100, member HashString @length 1..255 @pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$); modeled errors InvalidInputException (client, HTTP 400, "The input provided was not valid."), OperationTimeoutException (400), InternalServiceException (500). The model fixes the class, not the message. For the message this is recorded behavior only, and the recording is unambiguous and consistent across both corpora: this operation's malformed-RunId refusal is HTTP 400 with error code InvalidInputException and NO Message member (botocore therefore surfaces Error.Message as the empty string) — the check asserts exactly ('InvalidInputException', ''). The same recording distinguishes the batch read from its siblings: on the SINGLE-run paths GetDataQualityRulesetEvaluationRun and CancelDataQualityRulesetEvaluationRun the same malformed id is refused InvalidInputException with the all-fixed-text message "Invalid RunId: must start with 'dqrun-'" (no request- or state-derived part), and an in-shape-but-never-issued id ('dqrun-' + hex) is EntityNotFoundException "Cannot find Data Quality Run in account <accountId> with id <runId>" (fixed text "Cannot find Data Quality Run in account " / " with id "; variable parts: the caller's account id and the RunId from the request) — while the batch read reports that same in-shape absent id under RunsNotFound with no error. So three operations, three distinct refusal documents; only the batch one is message-less.

Emulator: BatchGetDataQualityRulesetEvaluationRun answers InvalidInputException with Message "Invalid RunId: must start with 'dqrun-'". Replay: expected ('InvalidInputException', ''), got ('InvalidInputException', "Invalid RunId: must start with 'dqrun-'"). Every earlier assertion of the check passed — the real run is readable through the single-run read, the batch read partitions a real run into Runs and an in-shape absent id into RunsNotFound, and both single-run paths produce AWS's exact format-refusal message — so the divergence is isolated to the message the BATCH refusal carries: the batch handler reuses the single-run read's shared message constant instead of refusing with no message.

Why: a client cannot tell the two refusals apart. Callers (and SDK retry/branching helpers, log scrapers, support tooling) that key off the refusal document see a batch rejection that claims to be a single-run RunId rejection; an assertion that the batch refusal carries no detail message fails against the emulator, and anything that parses the message to decide which id in a 100-element RunIds list was bad is encouraged to read a per-id diagnosis AWS never sends for this operation. Tests pinned to AWS's refusal body pass in production and fail here (and vice versa), which is exactly the signal this emulator exists to preserve.

Fix: the input validation of the BatchGetDataQualityRulesetEvaluationRun handler — it must raise the modeled InvalidInputException (400) for a RunId outside the minted-id grammar WITHOUT a message member, rather than sharing the single-run read's message text, and must leave the single-run Get/Cancel messages and the RunsNotFound partitioning of in-shape absent ids unchanged. Related but NOT covering this: the FIXED issue 20261123T140534Z-glue-connection-type-and-data-quality-batch-reads.md (its cause 2) is what introduced this refusal — it correctly replaced a 200 with InvalidInputException 400, and its own text noted that the recording asserts no Message for this refusal; the class is now right and the body is not, so this is the residue of that fix rather than the accepted-input defect it closed. The OPEN issue 20261123T192447Z-glue-2-net-new-causes-a-glue-etl.md (cause 2) covers only the single-run Get/Cancel format validation, whose assertions now pass, and explicitly scopes itself to leaving the batch read's behavior intact; it does not name the batch refusal's message. The replay reports this check unowned.

Traces: glue.data-quality-ruleset-evaluation-run-lookup-rejections (assertion at check_glue_data_quality_ruleset_evaluation_run_lookup_rejections.py:74). The emulator-side message is set in services/service-com-amazonaws-glue-awsglue/src/ops/batch_get_data_quality_ruleset_evaluation_run.rs:190-200, which passes crate::state::messages::invalid_data_quality_run_id() — the constant the single-run reads use.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 637/720 passed (88.5%), 61 mismatched, 22 errored. Service heading '### glue' shows 339/365 (93%), 26 failed; every one of those except the check below is marked ALREADY FILED (unimplemented scaffold stub) or OWNED by an open issue.

Check glue.data-quality-ruleset-evaluation-run-lookup-rejections, [mismatch]:
  AssertionError at check_glue_data_quality_ruleset_evaluation_run_lookup_rejections.py:74 —
  "unexpected BatchGetDataQualityRulesetEvaluationRun error for a malformed id: ('InvalidInputException', \"Invalid RunId: must start with 'dqrun-'\")"
  Call: glue.batch_get_data_quality_ruleset_evaluation_run(RunIds=[<a RunId not carrying the 'dqrun-' prefix>])
  Expected: HTTP 400, Error.Code 'InvalidInputException', Error.Message '' — i.e. the refusal body carries the error code and NO message member.
  Actual:   HTTP 400, Error.Code 'InvalidInputException', Error.Message "Invalid RunId: must start with 'dqrun-'" — the single-run read's fixed message text.

Assertions in the same check that PASSED, bounding the cause:
  - GetDataQualityRulesetEvaluationRun(RunId=<real run>) returns the run with its RulesetNames.
  - BatchGetDataQualityRulesetEvaluationRun(RunIds=[<real run>, 'dqrun-'+hex never issued]) returns Runs=[<real run>] and RunsNotFound=['dqrun-...'] — no error for an in-shape absent id.
  - GetDataQualityRulesetEvaluationRun and CancelDataQualityRulesetEvaluationRun with the same malformed id both return ('InvalidInputException', "Invalid RunId: must start with 'dqrun-'"), AWS's wording for the single-run paths.
So the only diverging element is the message on the BATCH refusal.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite against it (run_behavioral_suite).
3. Replay check glue.data-quality-ruleset-evaluation-run-lookup-rejections. It creates a role Glue can assume, an S3 bucket, a Glue database and table, and a data quality ruleset, then starts a data quality ruleset evaluation run and reads it back.
4. Call BatchGetDataQualityRulesetEvaluationRun with a RunIds list containing one id that does NOT start with 'dqrun-' (the check uses 'sim2real-' + a hex uuid). Observe the emulator answers HTTP 400 InvalidInputException with Message "Invalid RunId: must start with 'dqrun-'"; AWS answers HTTP 400 InvalidInputException with NO message member (botocore reports Error.Message as '').
5. Confirm nothing else regresses: the same malformed id on GetDataQualityRulesetEvaluationRun and CancelDataQualityRulesetEvaluationRun must still be InvalidInputException "Invalid RunId: must start with 'dqrun-'"; an in-shape but never-issued id ('dqrun-'+hex) must still be EntityNotFoundException "Cannot find Data Quality Run in account <accountId> with id <runId>" on both single-run paths and must still come back under RunsNotFound (no error) from the batch read.
6. The check must pass end to end before this issue is closed.
