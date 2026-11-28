Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-agentic-retrieve-stream-guardrail-needs-custom-planner
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-agentic-retrieve-stream-guardrail-needs-custom-planner bedrock-agent-runtime-agentic-retrieve-stream-iteration-bounds bedrock-agent-runtime-agentic-retrieve-stream-query-size-limit bedrock-agent-runtime-agentic-retrieve-stream-retriever-list-refusals bedrock-agent-runtime-get-document-content-request-validation-errors bedrock-agent-runtime-optimize-prompt-text-refused-before-stream bedrock-agent-runtime-optimize-prompt-unserved-target-model-not-found bedrock-agent-runtime-put-invocation-step-validation-errors bedrock-agent-runtime-rerank-inline-source-member-mismatch-errors bedrock-agent-runtime-rerank-json-source-text-key-required bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|GetDocumentContent|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->2xx|$.message
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->2xx|$headers.content-type
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->2xx|status
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|PutInvocationStep|4xx->2xx|$.message
Issue-Key: bedrock-agent-runtime|PutInvocationStep|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|PutInvocationStep|4xx->2xx|status
Issue-Key: bedrock-agent-runtime|Rerank|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|$.message
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|status
Issue-Commit: 2bdcb6d32c1ef40e42e13d397b84071513486a08

# bedrock-agent-runtime error paths diverge: 9 net-new causes — missing request validation, resource-lookup-before-validation ordering, and non-AWS ValidationException messages

> _Note: this issue was identified against the code base at commit `2bdcb6d32c1ef40e42e13d397b84071513486a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AgenticRetrieveStream

## Description
The bedrock-agent-runtime emulator diverges from AWS on nine distinct request-rejection paths. Three shapes of defect recur: (a) a request AWS rejects with 400 ValidationException is accepted and served a success response, (b) a request AWS rejects with 400 ValidationException is answered with 404 ResourceNotFoundException because the handler resolves the referenced resource before it validates the request, and (c) the 400 is produced but its ValidationException `message` is the emulator's own prose instead of the sentence AWS emits for that error class. All nine are in this one service and must all be closed before this issue is submitted.

This issue is NOT covered by the open issues 20261127T134743Z-bedrock-agent-runtime-agentic-retrieval-is-unusable-4.md, ...-knowledge-base-data-plane-refuses.md or ...-flows-diverge-across-prepare-execution.md: none of them carries the replay keys listed below.

Cause 1 of 9: AgenticRetrieveStream rejects policyConfiguration with the wrong ValidationException message.
AWS: Smithy: AgenticRetrieveStream input member `policyConfiguration` targets AgenticRetrievePolicyConfiguration (bedrockGuardrailConfiguration); `agenticRetrieveConfiguration.foundationModelType` is an enum defaulting to MANAGED; modeled error ValidationException (httpError 400, member `message`). Recorded behavior: 400 with message exactly `Guardrail is only supported when foundationModelType is CUSTOM.` — this is fixed text with no request- or state-derived parts.
Emulator: 400, but message `policyConfiguration: it names the guardrail a retrieval is run under, and this service evaluates no guardrail policy, so neither an intervention nor its absence can be reported; omit policyConfiguration to retrieve ungoverned`.
Why: the error message is part of the contract a client matches on and surfaces to users; a client that string-matches AWS's sentence, or that reads it to learn it must switch foundationModelType to CUSTOM, gets emulator-internal prose about an unimplemented feature instead and cannot act on it.
Fix: AgenticRetrieveStream's request validation — replace the emulator's own refusal text for policyConfiguration with AWS's fixed sentence.
Traces: bedrock-agent-runtime-agentic-retrieve-stream-guardrail-needs-custom-planner#task#1 (and #task#2).

Cause 2 of 9: AgenticRetrieveStream resolves the knowledge base before validating the request, so request-level ValidationExceptions are replaced by 404 ResourceNotFoundException.
AWS: Smithy: AgenticRetrieveStream errors include ValidationException (400) and ResourceNotFoundException (404); `agenticRetrieveConfiguration.maxAgentIteration` is an Integer with @range min 0, default 5, and `generateResponse` is a Boolean with default true. AWS validates the request shape first: recorded behavior is 400 with message `generateResponse must be false when maxAgentIteration is 0, because generating a response requires at least one LLM call.` (fixed text; the member names in it are modeled names, not per-request values).
Emulator: 404 with `x-amzn-errortype: ResourceNotFoundException` and message `Knowledge Base with id <knowledgeBaseId> does not exist` — it looked up the retriever's knowledge base first and never reached the request-consistency check. (The recorded id KBABSENT42 is the request's value filling that template, not part of it.)
Why: the status class flips from 400 (client must fix the request) to 404 (resource missing), so a client retries against a different knowledge base instead of correcting maxAgentIteration/generateResponse, and SDK error-type dispatch selects the wrong exception class.
Fix: AgenticRetrieveStream's handler — order request validation ahead of knowledge-base/retriever resolution.
Traces: bedrock-agent-runtime-agentic-retrieve-stream-iteration-bounds#task#2; same cluster also hit by bedrock-agent-runtime-agentic-retrieve-stream-query-size-limit#task#2 and bedrock-agent-runtime-agentic-retrieve-stream-retriever-list-refusals#task#2.

Cause 3 of 9: GetDocumentContent's enum validation message lists the allowed values in the wrong order.
AWS: Smithy: GetDocumentContent input member `outputFormat` targets enum DocumentOutputFormat with values declared in model order [EXTRACTED, RAW]; ValidationException (400). AWS's standard constraint message is `1 validation error detected: Value '<supplied value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<allowed values in model declaration order>]` — the fixed text is the whole sentence; the supplied value and member path come from the request and the value set from the model.
Emulator: 400 with the same template but the set rendered `[RAW, EXTRACTED]`.
Why: the enum value set is rendered from an unordered or re-sorted collection, so any client or test asserting on AWS's message (a common pattern for validation fixtures) fails, and users reading the message see a value ordering AWS never emits.
Fix: GetDocumentContent's validation / the shared enum-constraint message formatter — emit the enum's declared model order.
Traces: bedrock-agent-runtime-get-document-content-request-validation-errors#task#1.

Cause 4 of 9: GetDocumentContent resolves the knowledge base before validating mediaId/outputFormat combinations.
AWS: Smithy: GetDocumentContent binds knowledgeBaseId/dataSourceId/documentId as @httpLabel and takes optional `mediaId` and `outputFormat`; errors ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: the incompatible combination is rejected first with 400 and message `OutputFormat EXTRACTED cannot be used when mediaId is supplied` — fixed text apart from the outputFormat value echoed from the request.
Emulator: 404 with `x-amzn-errortype: ResourceNotFoundException` and `Knowledge Base with id <knowledgeBaseId> does not exist`, i.e. the same lookup-before-validation ordering as Cause 2 in a second operation.
Why: callers see a missing-resource error for a malformed request, so the status class, the error type header and the remediation are all wrong.
Fix: GetDocumentContent's handler — validate the request (including member-combination rules) before resolving the knowledge base, data source or document.
Traces: bedrock-agent-runtime-get-document-content-request-validation-errors#task#4.

Cause 5 of 9: OptimizePrompt accepts an empty/malformed prompt and opens an event stream instead of returning 400.
AWS: Smithy: OptimizePrompt input `input` is a required InputPrompt union whose only member is `textPrompt`; output `optimizedPrompt` is an @httpPayload @streaming event stream; modeled error ValidationException (400). Recorded behavior: 400, `content-type: application/json`, an `x-amzn-errortype` header, and message `Input validation failed. Check your request parameters and retry the request. Input Prompt is malformed/empty` — fixed text (the leading sentence is ValidationException's documented text).
Emulator: 200 with `content-type: application/vnd.amazon.eventstream`, no `x-amzn-errortype` and no `message`; the stream is opened for an input AWS refuses before streaming begins.
Why: the request is validated before any stream is established on AWS, so a client gets a synchronous, catchable ValidationException. Against the emulator it instead receives a 200 event stream and must discover the failure (or an empty optimization) mid-stream, which no AWS-targeting error handler is written for.
Fix: OptimizePrompt's handler/validation — reject an empty or malformed InputPrompt before the event-stream response is started.
Traces: bedrock-agent-runtime-optimize-prompt-text-refused-before-stream#task#2 (and #task#3).

Cause 6 of 9: OptimizePrompt reports an unserved targetModelId as end-of-life instead of model-not-found.
AWS: Smithy: OptimizePrompt input `targetModelId` is required with @length 1..2048 and a pattern admitting bare model ids, foundation-model and custom/provisioned ARNs; ValidationException (400). Recorded behavior: 400 with message `Input validation failed. Check your request parameters and retry the request. Model not found: <targetModelId>` — the leading sentence is ValidationException's fixed text and the trailing clause names the model id taken from the request.
Emulator: 400 with `Input validation failed. Check your request parameters and retry the request. Invalid model ID: This model version has reached the end of its life. Please refer to the AWS documentation for more details.` — a different error class, and it never echoes the requested model id.
Why: a client is told the model it asked for is retired rather than unknown to the service, so it stops trying valid model ids instead of correcting the one it sent; the message also loses the identifying value AWS includes.
Fix: OptimizePrompt's validation — distinguish an unknown/unserved targetModelId from a deprecated one and emit AWS's `Model not found: <targetModelId>` wording with the request's value.
Traces: bedrock-agent-runtime-optimize-prompt-unserved-target-model-not-found#task#2 (and #task#3).

Cause 7 of 9: PutInvocationStep accepts a zero-length content-block text and returns 201.
AWS: Smithy: PutInvocationStep input `payload` is a required InvocationStepPayload union whose `contentBlocks` list is BedrockSessionContentBlocks with @length min 1 over BedrockSessionContentBlock (text member); success is HTTP 201; modeled error ValidationException (400). Recorded behavior: 400 with the standard constraint message `1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must have length greater than or equal to 1` — fixed text with the member path (here the indexed path into payload.contentBlocks ... .member.text) derived from the request.
Emulator: 201, no `message`, no `x-amzn-errortype`; the empty content block is persisted as a step.
Why: AWS never stores a step with an empty text block, so an emulator-backed session accumulates state AWS would have refused; a client's error path for malformed payloads is never exercised and later reads return content AWS would not have.
Fix: PutInvocationStep's validation — enforce the minimum length on each content block's text (and on contentBlocks itself) before writing session state.
Traces: bedrock-agent-runtime-put-invocation-step-validation-errors#task#5 (and #task#6).

Cause 8 of 9: Rerank's missing-jsonDocument refusal uses emulator prose instead of AWS's sentence.
AWS: Smithy: Rerank input `sources` is a RerankSourcesList of RerankSource{type: RerankSourceType(INLINE), inlineDocumentSource: RerankDocument}; RerankDocument has required `type` (TEXT|JSON) plus optional `textDocument`/`jsonDocument`, documented as "Choose the type to define and include the field that corresponds to the type"; ValidationException (400). Recorded behavior: 400 with message `The jsonDocument field is null for a provided JSON source.` — fixed text, no request-derived parts.
Emulator: 400 with `sources.0.inlineDocumentSource.jsonDocument is required when type is JSON` — a locator-style message AWS does not emit for this class.
Why: the status is right but the message is not the contract's; clients and fixtures matching AWS's sentence fail, and the emulator's index-prefixed form implies a per-member validation report AWS does not produce here.
Fix: Rerank's validation — emit AWS's fixed sentence for a JSON source with no jsonDocument.
Traces: bedrock-agent-runtime-rerank-inline-source-member-mismatch-errors#task#2 (and #task#3, #task#5).

Cause 9 of 9: Rerank accepts sources that are neither TEXT nor JSON and returns 200.
AWS: Smithy: RerankDocument.type is required and its enum DocumentType admits only [TEXT, JSON] (RerankSourceType admits only INLINE); ValidationException (400). Recorded behavior: 400 with message `All provided rerank sources must be of type TEXT or type JSON.` — fixed text.
Emulator: 200 with a normal RerankResponse, no `message` and no `x-amzn-errortype`; the unmodeled source type is silently reranked.
Why: an unsupported source type is a client bug AWS surfaces immediately; the emulator instead returns scores computed over input AWS would never have accepted, so the caller believes a malformed request succeeded and the divergence only shows up against the real service.
Fix: Rerank's validation — reject any source whose document type is outside the modeled enum before scoring.
Traces: bedrock-agent-runtime-rerank-inline-source-member-mismatch-errors#task#4; same cluster also hit by bedrock-agent-runtime-rerank-json-source-text-key-required#task#2 and bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (65 fixtures, 302 steps, 267 passed, score 88.4%).

Cause 1 — AgenticRetrieveStream, fixture bedrock-agent-runtime-agentic-retrieve-stream-guardrail-needs-custom-planner step task#1: expected 400 / $.message 'Guardrail is only supported when foundationModelType is CUSTOM.'; actual 400 / $.message 'policyConfiguration: it names the guardrail a retrieval is run under, and this service evaluates no guardrail policy, so neither an intervention nor its absence can be reported; omit policyConfiguration to retrieve ungoverned'.

Cause 2 — AgenticRetrieveStream, fixture bedrock-agent-runtime-agentic-retrieve-stream-iteration-bounds step task#2: expected status 400 / $.message 'generateResponse must be false when maxAgentIteration is 0, because generating a response requires at least one LLM call.'; actual status 404, $.message 'Knowledge Base with id KBABSENT42 does not exist', $headers.x-amzn-errortype 'ResourceNotFoundException' (expected shaped-not-asserted). Cluster also covers ...-query-size-limit#task#2 and ...-retriever-list-refusals#task#2.

Cause 3 — GetDocumentContent, fixture bedrock-agent-runtime-get-document-content-request-validation-errors step task#1: expected 400 / "1 validation error detected: Value 'MARKDOWN' at 'outputFormat' failed to satisfy constraint: Member must satisfy enum value set: [EXTRACTED, RAW]"; actual 400 / same sentence with '[RAW, EXTRACTED]'.

Cause 4 — GetDocumentContent, same fixture step task#4: expected 400 / 'OutputFormat EXTRACTED cannot be used when mediaId is supplied'; actual 404 / 'Knowledge Base with id KBABSENT01 does not exist' with x-amzn-errortype 'ResourceNotFoundException'.

Cause 5 — OptimizePrompt, fixture bedrock-agent-runtime-optimize-prompt-text-refused-before-stream step task#2: expected status 400, content-type 'application/json', $.message 'Input validation failed. Check your request parameters and retry the request. Input Prompt is malformed/empty', an x-amzn-errortype header; actual status 200, content-type 'application/vnd.amazon.eventstream', no message, no x-amzn-errortype. Also task#3.

Cause 6 — OptimizePrompt, fixture bedrock-agent-runtime-optimize-prompt-unserved-target-model-not-found step task#2: expected 400 / 'Input validation failed. Check your request parameters and retry the request. Model not found: amazon.titan-text-express-v1'; actual 400 / 'Input validation failed. Check your request parameters and retry the request. Invalid model ID: This model version has reached the end of its life. Please refer to the AWS documentation for more details.' Also task#3.

Cause 7 — PutInvocationStep, fixture bedrock-agent-runtime-put-invocation-step-validation-errors step task#5: expected status 400 / "1 validation error detected: Value at 'payload.contentBlocks.1.member.text' failed to satisfy constraint: Member must have length greater than or equal to 1" plus x-amzn-errortype; actual status 201 with neither. Also task#6.

Cause 8 — Rerank, fixture bedrock-agent-runtime-rerank-inline-source-member-mismatch-errors step task#2: expected 400 / 'The jsonDocument field is null for a provided JSON source.'; actual 400 / 'sources.0.inlineDocumentSource.jsonDocument is required when type is JSON'. Also task#3, task#5.

Cause 9 — Rerank, same fixture step task#4: expected status 400 / 'All provided rerank sources must be of type TEXT or type JSON.' plus x-amzn-errortype; actual status 200 with neither. Cluster also covers bedrock-agent-runtime-rerank-json-source-text-key-required#task#2 and bedrock-agent-runtime-rerank-number-of-results-bounded-by-source-count#task#2.

## Steps to Reproduce
1. Clone the repo and build/boot the bedrock-agent-runtime emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Inspect the representative step bedrock-agent-runtime-agentic-retrieve-stream-guardrail-needs-custom-planner#task#1 and each step listed in trace_ids:
   - AgenticRetrieveStream with policyConfiguration.bedrockGuardrailConfiguration and foundationModelType MANAGED -> expect 400 'Guardrail is only supported when foundationModelType is CUSTOM.'
   - AgenticRetrieveStream with maxAgentIteration 0 and generateResponse true against an absent knowledge base -> expect the 400 request-validation error, not 404 ResourceNotFoundException.
   - GetDocumentContent with outputFormat 'MARKDOWN' -> expect the enum set rendered '[EXTRACTED, RAW]'.
   - GetDocumentContent with outputFormat EXTRACTED plus mediaId against an absent knowledge base -> expect 400 'OutputFormat EXTRACTED cannot be used when mediaId is supplied'.
   - OptimizePrompt with an empty textPrompt -> expect 400 application/json ValidationException, not a 200 eventstream.
   - OptimizePrompt with an unserved targetModelId -> expect 'Model not found: <targetModelId>'.
   - PutInvocationStep with a contentBlocks entry whose text is empty -> expect 400, not 201.
   - Rerank with an INLINE source of type JSON and no jsonDocument -> expect 'The jsonDocument field is null for a provided JSON source.'
   - Rerank with a source document type outside [TEXT, JSON] -> expect 400 'All provided rerank sources must be of type TEXT or type JSON.'
4. All nine must pass before this issue is closed.
