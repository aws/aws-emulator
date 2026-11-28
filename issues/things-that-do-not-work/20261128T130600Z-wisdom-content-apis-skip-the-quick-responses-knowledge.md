Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-quick-responses-kb-type-gates-apis
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-quick-responses-kb-type-gates-apis
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetContentSummary|4xx->4xx|$.message
Issue-Key: wisdom|GetContentSummary|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetContentSummary|4xx->4xx|status
Issue-Key: wisdom|ListContents|4xx->2xx|$.message
Issue-Key: wisdom|ListContents|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|ListContents|4xx->2xx|status
Issue-Commit: 8c779968f63825f959c68d9ef908c64428218fcd

# wisdom Content APIs skip the QUICK_RESPONSES knowledge-base type gate on GetContentSummary and ListContents

> _Note: this issue was identified against the code base at commit `8c779968f63825f959c68d9ef908c64428218fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContentSummary, ListContents

## Description
This issue bundles TWO distinct net-new root causes found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the `wisdom` service (the qconnect/Wisdom restJson1 surface, crate services/service-com-amazonaws-qconnect-wisdomservice). BOTH must be closed before submitting; each cause names its own fixture#step.

Related open issue 20261127T234914Z-wisdom-seven-wire-divergences-in-the-knowledge-base.md carries the SAME gate for GetContent (its cause 2) and SearchContent (its cause 3); it does NOT name GetContentSummary or ListContents, and the replay reported neither cluster below as owned by an open issue, so neither is covered. A single shared gate in the Content APIs' knowledge-base resolution would close all four.

=== Cause 1 of 2: GetContentSummary against a QUICK_RESPONSES knowledge base answers 404 ResourceNotFoundException instead of the 400 type refusal ===
AWS: Smithy (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json) - GetContentSummary is GET /knowledgeBases/{knowledgeBaseId}/contents/{contentId}/summary and models ValidationException (httpError 400), ResourceNotFoundException (404), UnauthorizedException (401) and AccessDeniedException (403); `KnowledgeBaseType` values are [EXTERNAL, CUSTOM, QUICK_RESPONSES, MESSAGE_TEMPLATES, MANAGED], and the model's own note on the knowledge-base id says a Content resource should not be stored in a QUICK_RESPONSES knowledge base. The refusal itself is recorded behavior only: a Content API addressed at an EXISTING QUICK_RESPONSES knowledge base is refused HTTP 400 with body message `Content APIs are not supported for QUICK_RESPONSES knowledge bases` - fixed text `Content APIs are not supported for ` + the addressed knowledge base's own type, rendered as stored (`QUICK_RESPONSES`), read from state, + ` knowledge bases`. The fixture asserts the status and the message; the error type is shaped-not-asserted.
Emulator: answers HTTP 404 with x-amzn-errortype `ResourceNotFoundException` and message `Content does not exist.` - i.e. it resolves the content record only and never consults the type of the knowledge base the path addresses.
Why: the refusal is the wrong class. A client is told the content is missing - a condition it fixes by creating the content and retrying - where AWS says the Content plane does not apply to this kind of knowledge base at all, a permanent 400 that must stop the caller. A caller therefore loops creating/re-reading content against a knowledge base that can never serve it, and error handling keyed on the 404/400 split takes the wrong branch.
Fix: the GetContentSummary handler's knowledge-base resolution (services/service-com-amazonaws-qconnect-wisdomservice/src/ops/get_content_summary.rs) - evaluate the resolved knowledge base's type BEFORE the content lookup and refuse with the modeled 400 and this message; the gate belongs in the knowledge-base resolution shared by the Content APIs so every one of them refuses alike.
Traces: qconnect-quick-responses-kb-type-gates-apis#task#4.

=== Cause 2 of 2: ListContents against a QUICK_RESPONSES knowledge base succeeds (200) instead of being refused 400 ===
AWS: Smithy (same model) - ListContents is GET /knowledgeBases/{knowledgeBaseId}/contents, is @paginated, and models ValidationException (400), ResourceNotFoundException (404) and AccessDeniedException (403). Recorded behavior only for the refusal: the same gate as cause 1 - ListContents on a QUICK_RESPONSES knowledge base is refused HTTP 400 with the message `Content APIs are not supported for QUICK_RESPONSES knowledge bases` (fixed text plus the stored knowledge-base type).
Emulator: answers HTTP 200 with a normal list envelope (status expected 400, got 200; no `message` member, no error-type header).
Why: worse than a wrong error code - the emulator reports SUCCESS for an API AWS refuses on that resource, so a client concludes a QUICK_RESPONSES knowledge base simply holds no content (an empty page) instead of learning the Content plane does not apply to it. Code validated against the emulator ships an enumeration path that fails outright in production, and an inventory sweep silently under-reports.
Fix: the ListContents handler's knowledge-base resolution / pre-handler validation (services/service-com-amazonaws-qconnect-wisdomservice/src/ops/list_contents.rs) - same type gate as cause 1, applied before the page is assembled.
Traces: qconnect-quick-responses-kb-type-gates-apis#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1: 21 fixtures, 198 steps - 147 passed, 50 mismatched, 1 blocked, score 74.2%, emulator_up=true, bootstrap_signal=false. 10 divergence clusters, every one service `wisdom`; none of the two below carries stub_path, and neither carries owned_by.

Cause 1 - qconnect-quick-responses-kb-type-gates-apis#task#4, GetContentSummary (divergence_key wisdom|GetContentSummary|status=400->404|keys=$.message,$headers.x-amzn-errortype,status):
  status: expected 400, got 404
  $.message: value_mismatch (expected 'Content APIs are not supported for QUICK_RESPONSES knowledge bases', got 'Content does not exist.')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  affected_count 1.

Cause 2 - qconnect-quick-responses-kb-type-gates-apis#task#5, ListContents (divergence_key wisdom|ListContents|status=400->200|keys=$.message,$headers.x-amzn-errortype,status):
  status: expected 400, got 200
  $.message: missing (expected 'Content APIs are not supported for QUICK_RESPONSES knowledge bases', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  affected_count 1.

Model grounding (smithy_query against services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json): GetContentSummary errors = AccessDeniedException 403, ResourceNotFoundException 404, UnauthorizedException 401, ValidationException 400; ListContents errors = AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400; KnowledgeBaseType values = [EXTERNAL, CUSTOM, QUICK_RESPONSES, MESSAGE_TEMPLATES, MANAGED]. The 400 refusal message is not in the model - it is taken from the recorded responses.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture qconnect-quick-responses-kb-type-gates-apis of trace set simparity-wire-aws-v1 (steps task#4 and task#5). Minimal reproduction:

1) CreateKnowledgeBase(name=<any>, knowledgeBaseType='QUICK_RESPONSES') and wait for it to be ACTIVE.
2) GET /knowledgeBases/<that id>/contents/<a well-formed uuid>/summary (GetContentSummary): the emulator answers HTTP 404, x-amzn-errortype ResourceNotFoundException, message 'Content does not exist.'; AWS answers HTTP 400 with message 'Content APIs are not supported for QUICK_RESPONSES knowledge bases'.
3) GET /knowledgeBases/<that id>/contents (ListContents): the emulator answers HTTP 200 with a contentSummaries envelope; AWS answers HTTP 400 with the same message as (2).

Both must refuse before this is closed - re-replay task#4 AND task#5.
