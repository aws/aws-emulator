Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.invoke-inline-agent-return-control-result-reenters-the-next-turn-prompt bedrock-agent-runtime.invoke-inline-agent-stream-final-response-flag-changes-chunk-framing
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|InvokeInlineAgent|NotImplemented
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# Implement bedrock-agent-runtime InvokeInlineAgent: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeInlineAgent

## Description
`InvokeInlineAgent` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/invoke_inline_agent.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `InvokeInlineAgent` call, so each one unblocks only when `InvokeInlineAgent` works: `bedrock-agent-runtime.invoke-inline-agent-return-control-result-reenters-the-next-turn-prompt`, `bedrock-agent-runtime.invoke-inline-agent-stream-final-response-flag-changes-chunk-framing`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/invoke_inline_agent.rs`. Replaying the corpus, the emulator refused the `InvokeInlineAgent` call: File "check_bedrock_agent_runtime_invoke_inline_agent_return_control_result_reenters_the_next_turn_prompt.py", line 179, in run
    _assert_entitlement_withdrawal(err, runtime, control_session)
  File "check_bedrock_agent_runtime_invoke_inline_agent_return_control_result_reenters_the_next_turn_prompt.py", line 409, in _assert_entitlement_withdrawal
    assert code == "AccessDeniedException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: InvokeInlineAgent failed with 'NotImplemented' against an account whose bedrock-agent-runtime session store is provably healthy. Only AccessDeniedException is the per-account model-entitlement refusal; 'NotImplemented' indicates a malformed request, a wrong foundationModel id, or a rejected actionGroups shape, not a withdrawal (H7/H8). Message: 'operation InvokeInlineAgent is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the bedrock-agent-runtime emulator
Call InvokeInlineAgent and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
