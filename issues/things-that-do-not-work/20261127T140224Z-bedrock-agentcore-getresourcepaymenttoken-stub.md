Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.resource-payment-token-unknown-provider-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.resource-payment-token-unknown-provider-rejected|run#15|AssertionError|assert got == ("ResourceNotFoundException", "Resource Credential Provider not found"), (
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# Implement bedrock-agentcore GetResourcePaymentToken: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePaymentToken

## Description
`GetResourcePaymentToken` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_resource_payment_token.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetResourcePaymentToken` call, so each one unblocks only when `GetResourcePaymentToken` works: `bedrock-agentcore.resource-payment-token-unknown-provider-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_resource_payment_token.rs`. Replaying the corpus, the emulator refused the `GetResourcePaymentToken` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_bedrock_agentcore_resource_payment_token_unknown_provider_rejected.py", line 81, in run
    assert got == ("ResourceNotFoundException", "Resource Credential Provider not found"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a payment token for a provider name nothing was created under answered ('NotImplemented', 'operation GetResourcePaymentToken is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock-agentcore emulator
Call GetResourcePaymentToken and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
