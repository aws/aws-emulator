Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-ai-prompt-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-ai-prompt-lifecycle
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetAIPrompt|2xx->2xx|$.aiPrompt.description
Issue-Key: wisdom|GetAIPrompt|2xx->2xx|$.aiPrompt.visibilityStatus
Issue-Key: wisdom|GetAIPrompt|4xx->2xx|$
Issue-Key: wisdom|GetAIPrompt|4xx->2xx|$.message
Issue-Key: wisdom|GetAIPrompt|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetAIPrompt|4xx->2xx|status
Issue-Commit: 07e1e88d592d3fe676e21622404cc67d61ac5470

# QConnect GetAIPrompt never reflects AI prompt mutations after update or delete

> _Note: this issue was identified against the code base at commit `07e1e88d592d3fe676e21622404cc67d61ac5470`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAIPrompt

## Description
GetAIPrompt (GET /assistants/{assistantId}/aiprompts/{aiPromptId}) only ever reports the record CreateAIPrompt stored: no later mutation of the AI prompt is visible through it. Two causes, both on the wisdom/QConnect AI-prompt resource, both observed in the qconnect-ai-prompt-lifecycle fixture.

Cause 1 of 2: a read after a successful UpdateAIPrompt still answers the pre-update prompt.
AWS: Smithy (com.amazonaws.qconnect#WisdomService) models UpdateAIPrompt POST /assistants/{assistantId}/aiprompts/{aiPromptId} with required input member visibilityStatus (VisibilityStatus, values SAVED|PUBLISHED) and optional description (Description, @length 1..255, pattern ^[a-zA-Z0-9\s_.,-]+), returning AIPromptData; GetAIPrompt is @readonly and returns the same AIPromptData. AIPromptData.visibilityStatus is @required and description is optional. AWS applies the update to the prompt's working copy, so the subsequent GetAIPrompt reports the updated description and the visibilityStatus the update requested (recorded response: description 'Updated probe answer generation prompt', visibilityStatus 'PUBLISHED'; the literal description text and prompt id are the fixture's values, the requirement is that whatever description/visibilityStatus the update carried is what the read reports).
Emulator: GetAIPrompt answers HTTP 200 (status matches) but with $.aiPrompt.description still the value given at create time and $.aiPrompt.visibilityStatus still 'SAVED' — i.e. the mutation is lost. Root cause: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_prompt.rs is still a scaffold stub (`//! SCAFFOLDED, NOT IMPLEMENTED:`, handler returns Scaffolded::not_implemented), so nothing is ever written to state; GetAIPrompt is implemented and faithfully reports the stale record. The open issue 20261113T130046Z-qconnect-updateaiprompt-stub.md asks for UpdateAIPrompt to be implemented but carries no wire replay key, so this read-path divergence is not covered by it and must be closed against this fixture step too.
Why: a client that updates a prompt and then reads it back (the normal publish flow — set visibilityStatus PUBLISHED, then verify) sees the prompt as unpublished with the old description forever, so publish/verify loops never converge and configuration changes appear silently discarded.
Fix: implement the UpdateAIPrompt handler so it mutates the stored AI-prompt record (description, visibilityStatus, templateConfiguration, modelId, inferenceConfiguration, modifiedTime) in the service state the AI-prompt store keeps, and make sure GetAIPrompt's projection of that record reads the mutated fields.
Traces: qconnect-ai-prompt-lifecycle#task#5 (GetAIPrompt, expected 200 with updated description/visibilityStatus).

Cause 2 of 2: a read after DeleteAIPrompt still answers HTTP 200 with the deleted prompt instead of 404 ResourceNotFoundException.
AWS: Smithy models DeleteAIPrompt DELETE /assistants/{assistantId}/aiprompts/{aiPromptId} (success 204, @idempotent) and GetAIPrompt with error ResourceNotFoundException (com.amazonaws.qconnect#ResourceNotFoundException, @error client, @httpError 404, members message and resourceName, doc 'The specified resource does not exist.'). After the prompt is deleted, GetAIPrompt fails with HTTP 404, x-amzn-errortype naming ResourceNotFoundException, and message 'AI Prompt does not exist.' — that message is entirely fixed text for this error class on the AI-prompt resource (no request- or state-derived substitution); the resourceName member carries the addressed prompt id from the request.
Emulator: GetAIPrompt keeps answering HTTP 200 with the full aiPrompt body (assistantId/aiPromptArn/name/templateConfiguration/visibilityStatus 'SAVED'/status 'ACTIVE'), no $.message and no x-amzn-errortype header; the fixture's poll for error code ResourceNotFoundException never observes one after 15 attempts. Root cause: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/delete_ai_prompt.rs is still a scaffold stub returning Scaffolded::not_implemented, so the prompt is never removed from state and the implemented GetAIPrompt still finds it.
Why: a client that deletes a prompt and polls for its disappearance (or relies on 404 to decide a teardown finished, or to re-create under the same name) loops forever and treats removed resources as live; nothing in the emulator ever reports an AI prompt as gone.
Fix: implement the DeleteAIPrompt handler so it removes the AI-prompt record (and its versions) from the service state under the addressed assistant and answers 204, so GetAIPrompt's existing not-found path — which already has the AI-prompt-not-found message constant and builds ResourceNotFoundException with resourceName — fires on the next read. Return the modeled ResourceNotFoundException from DeleteAIPrompt itself when the prompt is absent.
Traces: qconnect-ai-prompt-lifecycle#task#10 (GetAIPrompt, expected 404 ResourceNotFoundException 'AI Prompt does not exist.', got 200).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (score 63.2%, 38 mismatched steps):

Cause 1 — fixture qconnect-ai-prompt-lifecycle, step task#5, operation GetAIPrompt, verdict mismatch, expected status 200 / actual 200:
  $.aiPrompt.description: value_mismatch (expected 'Updated probe answer generation prompt', got 'Probe answer generation prompt')
  $.aiPrompt.visibilityStatus: value_mismatch (expected 'PUBLISHED', got 'SAVED')

Cause 2 — fixture qconnect-ai-prompt-lifecycle, step task#10, operation GetAIPrompt, verdict mismatch, expected status 404 / actual 200:
  status: expected 404, got 200
  $.message: missing (expected 'AI Prompt does not exist.', got None)
  $headers.x-amzn-errortype: missing (expected ResourceNotFoundException-shaped value, got None)
  poll: error code None not in ['ResourceNotFoundException'] after 15 attempt(s); the body returned was the live aiPrompt record (name sim2real-probe-qconnect-prompt-..., visibilityStatus 'SAVED', status 'ACTIVE').

Code evidence: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_prompt.rs:4 and services/service-com-amazonaws-qconnect-wisdomservice/src/ops/delete_ai_prompt.rs:4 both carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and their handlers return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (delete_ai_prompt.rs:162), while services/service-com-amazonaws-qconnect-wisdomservice/src/ops/get_ai_prompt.rs:179-199 reads the stored record and already has the 404 path (get_ai_prompt.rs:82-88, crate::state::MESSAGE_AI_PROMPT_NOT_FOUND).

Model evidence: smithy_query(operation="GetAIPrompt,UpdateAIPrompt,DeleteAIPrompt") — GetAIPrompt errors include ResourceNotFoundException (httpError 404); UpdateAIPrompt requires visibilityStatus and returns AIPromptData; DeleteAIPrompt success code 204. smithy_query(shape="AIPromptData,VisibilityStatus,ResourceNotFoundException") — VisibilityStatus values SAVED|PUBLISHED; AIPromptData.visibilityStatus required, description optional.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: CreateAssistant, then CreateAIPrompt (description 'Probe answer generation prompt', visibilityStatus SAVED), then UpdateAIPrompt on the same aiPromptId with a new description and visibilityStatus PUBLISHED, then GetAIPrompt — fixture qconnect-ai-prompt-lifecycle step task#5. Observe 200 with the pre-update description and visibilityStatus 'SAVED' instead of the updated description and 'PUBLISHED'.
3. Cause 2: with the same assistant/prompt, call DeleteAIPrompt, then poll GetAIPrompt — fixture qconnect-ai-prompt-lifecycle step task#10. Observe HTTP 200 with the full aiPrompt body on every attempt instead of HTTP 404 x-amzn-errortype ResourceNotFoundException with message 'AI Prompt does not exist.'.
