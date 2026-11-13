Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# InvokeAssistant has no handler at all: the operation is unroutable

_Filed: 2026-10-24T10:08:02Z_
_Operation: InvokeAssistant | Class: missing_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

InvokeAssistant is modeled and bound to the service, but the scaffold generator declined to emit anything for it because its output carries a @streaming event-stream member. There is no src/ops/invoke_assistant.rs, and the operation is not in the service's operations(..) allow-list, so a request is answered by the framework's unroutable-operation path (an UnknownOperation-style refusal) rather than by the service.

Expected: POST /assistant/invocation accepts {message, conversationId, enableTrace} and answers an event stream of ResponseStream events (trace / output / accessDeniedException ...), or at minimum a modeled error from InvokeAssistant's own error set.

Actual: no handler file exists; the request never reaches this service's code.

## Evidence

src/ops/SKIPPED_STUBS.md line 21: 'SKIPPED: InvokeAssistant — unsupported shape construct: operation InvokeAssistant has a @streaming event-stream output member, which needs wire framing the generator does not emit'. glob of src/ops/ shows no invoke_assistant.rs, and grep for InvokeAssistant in src/ finds only that note — so it is neither a scaffold stub nor implemented. The operation IS bound to the AWSIoTSiteWise service in the Smithy model.

## Steps to Reproduce

1. Boot the emulator.
2. Call iotsitewise InvokeAssistant (POST /assistant/invocation) with a message body.
3. Observe the framework's unroutable-operation answer (no modeled InvokeAssistant response or error, no event-stream framing) instead of a ResponseStream.
