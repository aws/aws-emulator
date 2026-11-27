Issue-Source: trace-sweep-divergence
Issue-Trace: search-content-refusals
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: search-content-refusals wisdom-content-refusals-by-knowledge-base-and-identifier wisdom-content-upload-and-identifier-validation wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateKnowledgeBase|2xx->2xx|$.knowledgeBase.lastModifiedRegion
Issue-Key: wisdom|CreateKnowledgeBase|2xx->2xx|$.knowledgeBase.lastModifiedTime
Issue-Key: wisdom|GetContent|4xx->4xx|$.message
Issue-Key: wisdom|GetContent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetContent|4xx->4xx|status
Issue-Key: wisdom|SearchContent|4xx->2xx|$.message
Issue-Key: wisdom|SearchContent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|SearchContent|4xx->2xx|status
Issue-Key: wisdom|StartContentUpload|4xx->4xx|$.message
Issue-Key: wisdom|StartContentUpload|4xx->2xx|$.message
Issue-Key: wisdom|StartContentUpload|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|StartContentUpload|4xx->2xx|status
Issue-Key: wisdom|DeleteContent|4xx->4xx|$.message
Issue-Key: wisdom|DeleteContent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|DeleteContent|4xx->4xx|status
Issue-Key: wisdom|StartImportJob|4xx->4xx|$.message
Issue-Key: wisdom|StartImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: b92cb7e4a0b1eae5b08ed7d2dc0bf009163c5925

# wisdom: seven wire divergences in the knowledge-base content plane - missing type gating, misclassified refusals, absent lastModified fields

> _Note: this issue was identified against the code base at commit `b92cb7e4a0b1eae5b08ed7d2dc0bf009163c5925`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
This issue bundles SEVEN distinct net-new root causes found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the `wisdom` service. All seven must be closed before submitting; each cause names its own fixture#step. (The open issue 20261127T201644Z-wisdom-eight-divergences-in-the-content-upload-plane.md is a BEHAVIORAL issue about uploadId lifecycle, upload-URL refusal shape, QueryAssistant paging and UpdateContent constraint wording; none of the causes below are covered by it, and no replayed step here was reported as owned by an open issue.)

=== Cause 1 of 7: CreateKnowledgeBase omits `lastModifiedRegion` and `lastModifiedTime` from the knowledgeBase it reports ===
AWS: recorded behavior only. Smithy `KnowledgeBaseData` (this on-disk model) declares knowledgeBaseId/knowledgeBaseArn/name/knowledgeBaseType/status/lastContentModificationTime/sourceConfiguration/renderingConfiguration/serverSideEncryptionConfiguration/description/tags and NO lastModified* member, so the model says nothing about these two; the recordings show the live CreateKnowledgeBase 200 response carrying BOTH `knowledgeBase.lastModifiedRegion` (the AWS region the knowledge base was last modified in - the region the request was served in) and `knowledgeBase.lastModifiedTime` (an epoch-seconds timestamp; for a freshly created knowledge base, the moment of the create). The recorded literals (`us-west-2`, 1795277275.99) are that run's region and clock and are NOT requirements - the requirement is that both members are PRESENT and carry the serving region and the record's own last-modification time.
Emulator: both members are absent from the 200 body (`$.knowledgeBase.lastModifiedRegion: missing`, `$.knowledgeBase.lastModifiedTime: missing`), on every CreateKnowledgeBase in the corpus (6 steps across 4 fixtures).
Why: a client that reads either member - any caller that renders when a knowledge base last changed, or that routes on the region a multi-region estate last wrote in - gets a null where AWS always sends a value, and code that treats the member as always-present breaks on the emulator only.
Fix: the CreateKnowledgeBase response projection and the state record it is built from (the knowledge-base record must carry a last-modified region + time, stamped at create and refreshed on mutation, and the output serializer must emit both).
Traces: search-content-refusals#setup#1 (also search-content-refusals#setup#2, wisdom-content-refusals-by-knowledge-base-and-identifier#setup#1 and #setup#2, wisdom-content-upload-and-identifier-validation#setup#1, wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type#setup#1).

=== Cause 2 of 7: GetContent against a QUICK_RESPONSES knowledge base answers 404 ResourceNotFoundException instead of the 400 type refusal ===
AWS: recorded behavior only (the model's own note on `KnowledgeBaseData$knowledgeBaseId` says it "should not be a QUICK_RESPONSES type knowledge base if you're storing Wisdom Content resource to it"). The recorded GetContent on an existing QUICK_RESPONSES knowledge base is refused HTTP 400 with message `Content APIs are not supported for QUICK_RESPONSES knowledge bases` - fixed text apart from the knowledge base's own type, rendered as stored (`QUICK_RESPONSES`), read from state; GetContent models ValidationException as its 400. The fixture asserts the status and the message, not the error code.
Emulator: answers HTTP 404 `ResourceNotFoundException` with message `Content does not exist.` - i.e. it only looks the content up and never consults the addressed knowledge base's type.
Why: the refusal is the wrong class entirely. A client gets "this content is missing" (retryable / create-it-and-retry) where AWS says "this API is not available for this kind of knowledge base" (a permanent 400 that must stop the caller), so a caller loops creating or re-reading content against a knowledge base that can never serve it.
Fix: the GetContent handler's knowledge-base resolution (gate on the resolved knowledge base's type BEFORE the content lookup and refuse with the modeled 400 and this message).
Traces: wisdom-content-refusals-by-knowledge-base-and-identifier#task#5.

=== Cause 3 of 7: SearchContent against a QUICK_RESPONSES knowledge base succeeds (200) instead of being refused 400 ===
AWS: recorded behavior only - the same gate as cause 2: SearchContent on a QUICK_RESPONSES knowledge base is refused HTTP 400 with the message `Content APIs are not supported for QUICK_RESPONSES knowledge bases` (fixed text plus the stored knowledge-base type).
Emulator: answers HTTP 200 with a normal search envelope (`status: expected 400, got 200`; `$.message` missing; no error type header).
Why: worse than a wrong error - the emulator reports SUCCESS for an API AWS refuses on that resource, so a client concludes a QUICK_RESPONSES knowledge base simply has no matching content (an empty result set) instead of learning the Content plane does not apply to it. Code validated against the emulator ships a search path that fails in production.
Fix: the SearchContent handler's knowledge-base resolution / pre-handler validation (same type gate as GetContent; the gate belongs with the shared knowledge-base resolution used by the Content APIs so every one of them refuses alike).
Traces: wisdom-content-refusals-by-knowledge-base-and-identifier#task#7.

=== Cause 4 of 7: the StartContentUpload contentType constraint message omits the `application/json+guide` alternative AWS's pattern carries ===
AWS: Smithy - `StartContentUpload$contentType` targets `ContentType` with pattern `^(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))$`; the operation models ValidationException (HTTP 400). The recording shows the LIVE service's pattern is wider than this on-disk model: the refusal of `image/png` is 400 with `1 validation error detected: Value 'image/png' at 'contentType' failed to satisfy constraint: Member must satisfy regular expression pattern: (application/json\+guide)|(text/(plain|html|csv))|(application/(pdf|vnd\.openxmlformats-officedocument\.wordprocessingml\.document))|(application/x\.wisdom-json;source=(salesforce|servicenow|zendesk))` - i.e. `(application/json\+guide)` is the FIRST alternative, and the pattern quoted back in the message is the full live pattern. Fixed text apart from the rejected value (from the request), the member path `contentType` and the pattern itself.
Emulator: same status and class, but the quoted pattern starts at `(text/(plain|html|csv))|...` - the `(application/json\+guide)` alternative is missing from the message, and by implication from the accepted set, so a request with contentType `application/json+guide` is refused where AWS admits it.
Why: two defects for one cause - a client reading the refusal is told a media type AWS accepts is unacceptable, and any caller that legitimately uploads `application/json+guide` assets cannot start an upload at all.
Fix: the contentType constraint used by StartContentUpload's validation (the service's pattern source: add the `application/json+guide` alternative in the leading position so both the accepted set and the quoted pattern in the ValidationException match the live service).
Traces: wisdom-content-upload-and-identifier-validation#task#4.

=== Cause 5 of 7: StartContentUpload accepts a contentType the addressed knowledge base's type does not support (200 instead of the 400 `Unsupported ContentType for KnowledgeBaseType` refusal) ===
AWS: recorded behavior only. A contentType that SATISFIES the member pattern but is not supported for the type of the addressed knowledge base is refused HTTP 400 with `Unsupported ContentType for KnowledgeBaseType: custom` - fixed text `Unsupported ContentType for KnowledgeBaseType: ` plus the addressed knowledge base's type taken from state and rendered in LOWER case (`custom` for a CUSTOM knowledge base), so the pattern check and the type/media-type compatibility check are two separate refusals.
Emulator: answers HTTP 200 and issues a presigned upload slot (`status: expected 400, got 200`, no message, no error type). The handler (`services/service-com-amazonaws-wisdom-wisdomservice/src/ops/start_content_upload.rs`, around the `knowledge_base.knowledge_base_type != crate::state::KNOWLEDGE_BASE_TYPE_CUSTOM` branch) applies `state::message_unsupported_content_type` ONLY when the knowledge base is not CUSTOM; for a CUSTOM knowledge base it never compares the requested media type against the type at all.
Why: the emulator hands out an upload URL for an asset the service will not accept, so a client PUTs bytes and only discovers the incompatibility at CreateContent time (or never) - and the refusal AWS gives up front, which tells the caller exactly which knowledge-base type rejected the media type, is never seen.
Fix: StartContentUpload's validation in that handler - extend the knowledge-base-type check to the CUSTOM case: decide the requested contentType against the resolved knowledge base's type and refuse with the modeled 400 and the lower-cased type in the message.
Traces: wisdom-content-upload-and-identifier-validation#task#5.

=== Cause 6 of 7: a content path the front door refuses as unauthorized is answered 400 ValidationException instead of 401 `Unauthorized` ===
AWS: recorded behavior only. For the Content paths `/knowledgeBases/{knowledgeBaseId}/contents/{contentId}` (GET GetContent, DELETE DeleteContent) carrying a `contentId` segment that the route cannot authorize, the recorded answer is HTTP 401 with body `{"message":"Unauthorized"}` - the message is entirely fixed text, carrying no value from the request - and the response does NOT carry an `x-amzn-errortype` of a modeled exception (the fixture does not assert a shaped error type there). The refusal happens at the REST front door, BEFORE any member-level constraint is evaluated. Hypothesis: AWS's REST front door authorizes the resolved route before the `UuidOrArn` httpLabel pattern is applied, so a path segment that does not resolve to an authorizable resource identifier is rejected by the authorizer rather than by input validation.
Emulator: validates the modeled `contentId` pattern first and answers HTTP 400 `ValidationException` (header `x-amzn-errortype: ValidationException`) with `1 validation error detected: Value '<contentId sent>' at 'contentId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-...` - the right shape for a constraint violation, but the wrong layer and the wrong status for this class.
Why: the status class differs (401 vs 400), so client retry/credential-refresh logic keyed on 401 never fires, and a caller is told its input is malformed where AWS tells it the request was not authorized. Error-handling code written against the emulator takes the wrong branch in production.
Fix: the ordering of the front door's authorization decision against member validation for the Content paths of this service (the shared request walk: the unauthorized refusal must precede httpLabel constraint validation, and must render the bare `{"message":"Unauthorized"}` 401 envelope without a modeled error type).
Traces: wisdom-content-refusals-by-knowledge-base-and-identifier#task#3 (GetContent; also wisdom-content-upload-and-identifier-validation#task#8) and wisdom-content-upload-and-identifier-validation#task#6 (DeleteContent; also wisdom-content-upload-and-identifier-validation#task#9).

=== Cause 7 of 7: StartImportJob refuses an out-of-set `importJobType` as BadRequestException `Invalid request body` instead of the ValidationException enum-constraint refusal ===
AWS: Smithy - `StartImportJob$importJobType` is required and targets the `ImportJobType` enum (this on-disk model lists only `QUICK_RESPONSES`); the operation models ValidationException (HTTP 400) and declares NO BadRequestException (no operation in this model does). The recording shows the live enum set is wider and the refusal is AWS's standard constraint sentence: 400 with `1 validation error detected: Value 'KNOWLEDGE_BASE_CONTENT' at 'importJobType' failed to satisfy constraint: Member must satisfy enum value set: [QUICK_RESPONSES, QUICK_REPLIES]` - fixed text apart from the rejected value (from the request), the member path `importJobType` and the admissible value set (`[QUICK_RESPONSES, QUICK_REPLIES]`, in that order).
Emulator: answers HTTP 400 but with `x-amzn-errortype: BadRequestException` and the deserialization-boundary message `Invalid request body`, naming neither the member, nor the rejected value, nor the value set.
Why: the error code is one no AWS model declares, so a client catching the modeled `ValidationException` (botocore's generated error class) does not catch this refusal at all; and the message tells neither an operator nor a caller WHICH member was wrong or what it may be.
Fix: the enum-constraint refusal for StartImportJob's `importJobType` - classify an out-of-set enum value as the modeled ValidationException through the shared constraint renderer (rather than letting it fall through to the front-door invalid-body refusal), and carry the live value set `[QUICK_RESPONSES, QUICK_REPLIES]` so the rendered clause matches AWS.
Traces: wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (18 fixtures, 128 steps): 104 passed, 24 mismatched, score 81.2%, emulator_up=true, bootstrap_signal=false. Nine divergence clusters, all service `wisdom`, none carrying stub_path or owned_by. Verbatim expected-vs-actual per cause:

Cause 1 - search-content-refusals#setup#1, CreateKnowledgeBase, status 200 -> 200:
  $.knowledgeBase.lastModifiedRegion: missing (expected 'us-west-2', got None)
  $.knowledgeBase.lastModifiedTime: missing (expected 1795277275.99, got None)
  affected (6): search-content-refusals#setup#1, search-content-refusals#setup#2, wisdom-content-refusals-by-knowledge-base-and-identifier#setup#1, wisdom-content-refusals-by-knowledge-base-and-identifier#setup#2, wisdom-content-upload-and-identifier-validation#setup#1, wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type#setup#1

Cause 2 - wisdom-content-refusals-by-knowledge-base-and-identifier#task#5, GetContent:
  status: expected 400, got 404
  $.message: value_mismatch (expected 'Content APIs are not supported for QUICK_RESPONSES knowledge bases', got 'Content does not exist.')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 3 - wisdom-content-refusals-by-knowledge-base-and-identifier#task#7, SearchContent:
  status: expected 400, got 200
  $.message: missing (expected 'Content APIs are not supported for QUICK_RESPONSES knowledge bases', got None)
  $headers.x-amzn-errortype: missing

Cause 4 - wisdom-content-upload-and-identifier-validation#task#4, StartContentUpload, status 400 -> 400:
  $.message expected: "1 validation error detected: Value 'image/png' at 'contentType' failed to satisfy constraint: Member must satisfy regular expression pattern: (application/json\\+guide)|(text/(plain|html|csv))|(application/(pdf|vnd\\.openxmlformats-officedocument\\.wordprocessingml\\.document))|(application/x\\.wisdom-json;source=(salesforce|servicenow|zendesk))"
  $.message actual:   "1 validation error detected: Value 'image/png' at 'contentType' failed to satisfy constraint: Member must satisfy regular expression pattern: (text/(plain|html|csv))|(application/(pdf|vnd\\.openxmlformats-officedocument\\.wordprocessingml\\.document))|(application/x\\.wisdom-json;source=(salesforce|servicenow|zendesk))"

Cause 5 - wisdom-content-upload-and-identifier-validation#task#5, StartContentUpload:
  status: expected 400, got 200
  $.message: missing (expected 'Unsupported ContentType for KnowledgeBaseType: custom', got None)
  $headers.x-amzn-errortype: missing

Cause 6 - wisdom-content-refusals-by-knowledge-base-and-identifier#task#3, GetContent (and wisdom-content-upload-and-identifier-validation#task#8):
  status: expected 401, got 400
  $.message: value_mismatch (expected 'Unauthorized', got "1 validation error detected: Value 'NOT-A-CONTENT-ID' at 'contentId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$|^arn:[a-z-]*?:wisdom:...")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  same cluster on DELETE: wisdom-content-upload-and-identifier-validation#task#6 (and #task#9), DeleteContent, expected 401 'Unauthorized', got 400 ValidationException on contentId 'not-a-content-id'

Cause 7 - wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type#task#3, StartImportJob, status 400 -> 400:
  $.message expected: "1 validation error detected: Value 'KNOWLEDGE_BASE_CONTENT' at 'importJobType' failed to satisfy constraint: Member must satisfy enum value set: [QUICK_RESPONSES, QUICK_REPLIES]"
  $.message actual:   'Invalid request body'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Model grounding (services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json): KnowledgeBaseData has no lastModifiedRegion/lastModifiedTime member; GetContent/SearchContent/StartContentUpload/DeleteContent each model ValidationException (400), ResourceNotFoundException (404), AccessDeniedException (403); ContentType pattern lacks the application/json+guide alternative; ImportJobType values = [QUICK_RESPONSES]; no operation declares BadRequestException.

## Steps to Reproduce
Boot the emulator from this checkout and replay the four wisdom fixtures of simparity-wire-aws-v1 (search-content-refusals, wisdom-content-refusals-by-knowledge-base-and-identifier, wisdom-content-upload-and-identifier-validation, wisdom-import-job-refusals-for-absent-knowledge-base-and-job-type). Per cause:

1) CreateKnowledgeBase(name=<any>, knowledgeBaseType=CUSTOM) -> read the 200 body: knowledgeBase carries no lastModifiedRegion and no lastModifiedTime; AWS returns both (the serving region and the record's last-modification epoch-seconds time).
2) CreateKnowledgeBase(knowledgeBaseType=QUICK_RESPONSES), then GetContent(knowledgeBaseId=<that id>, contentId=<a well-formed uuid>): observe 404 ResourceNotFoundException 'Content does not exist.'; AWS answers 400 'Content APIs are not supported for QUICK_RESPONSES knowledge bases'.
3) Same QUICK_RESPONSES knowledge base, SearchContent(knowledgeBaseId=<that id>, searchExpression=<any>): observe HTTP 200 with a search envelope; AWS answers 400 with the same message as (2).
4) StartContentUpload(knowledgeBaseId=<CUSTOM kb>, contentType='image/png'): observe the ValidationException message whose quoted pattern omits the leading '(application/json\+guide)|' alternative; also StartContentUpload(contentType='application/json+guide') is refused where AWS accepts it.
5) StartContentUpload(knowledgeBaseId=<CUSTOM kb>, contentType=<a media type the pattern admits but a CUSTOM knowledge base does not support, e.g. the application/x.wisdom-json;source=... external-source types>): observe HTTP 200 with a presigned slot; AWS answers 400 'Unsupported ContentType for KnowledgeBaseType: custom'.
6) GET /knowledgeBases/<kb>/contents/NOT-A-CONTENT-ID and DELETE /knowledgeBases/<kb>/contents/not-a-content-id: observe 400 ValidationException with the contentId pattern clause and x-amzn-errortype: ValidationException; AWS answers 401 with body {"message":"Unauthorized"} and no modeled error type.
7) StartImportJob(knowledgeBaseId=<kb>, importJobType='KNOWLEDGE_BASE_CONTENT', uploadId=<any>): observe 400 BadRequestException 'Invalid request body'; AWS answers 400 ValidationException "1 validation error detected: Value 'KNOWLEDGE_BASE_CONTENT' at 'importJobType' failed to satisfy constraint: Member must satisfy enum value set: [QUICK_RESPONSES, QUICK_REPLIES]".
