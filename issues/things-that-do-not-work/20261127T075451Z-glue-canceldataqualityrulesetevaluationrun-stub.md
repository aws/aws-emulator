Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.data-quality-ruleset-evaluation-run-lookup-rejections
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.data-quality-ruleset-evaluation-run-lookup-rejections|run#20|AssertionError|assert cancel_malformed == ("InvalidInputException", "Invalid RunId: must start with 'dqrun-'"), (
Issue-Commit: 929983b8776c4202c2a162ee50b02314bb9b729f

# Implement glue CancelDataQualityRulesetEvaluationRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `929983b8776c4202c2a162ee50b02314bb9b729f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelDataQualityRulesetEvaluationRun

## Description
`CancelDataQualityRulesetEvaluationRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/cancel_data_quality_ruleset_evaluation_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CancelDataQualityRulesetEvaluationRun` call, so each one unblocks only when `CancelDataQualityRulesetEvaluationRun` works: `glue.data-quality-ruleset-evaluation-run-lookup-rejections`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/cancel_data_quality_ruleset_evaluation_run.rs`. Replaying the corpus, the emulator refused the `CancelDataQualityRulesetEvaluationRun` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_glue_data_quality_ruleset_evaluation_run_lookup_rejections.py", line 68, in run
    assert cancel_malformed == ("InvalidInputException", "Invalid RunId: must start with 'dqrun-'"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected CancelDataQualityRulesetEvaluationRun message for a malformed id: ('NotImplemented', 'operation CancelDataQualityRulesetEvaluationRun is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the glue emulator
Call CancelDataQualityRulesetEvaluationRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
