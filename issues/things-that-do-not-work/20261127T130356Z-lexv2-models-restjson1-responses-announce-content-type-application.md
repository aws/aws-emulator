Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-custom-vocabulary-absent-refusals
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-custom-vocabulary-absent-refusals lex-models-v2-test-execution-request-refusals lex-models-v2-slot-type-in-use-deletion-order
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|BatchCreateCustomVocabularyItem|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|ListTestExecutions|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|DescribeTestExecution|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|StartTestExecution|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|CreateSlotType|2xx->2xx|$.creationDateTime
Issue-Key: lexv2-models|CreateSlotType|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|CreateIntent|2xx->2xx|$.creationDateTime
Issue-Key: lexv2-models|CreateIntent|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|CreateSlot|2xx->2xx|$.creationDateTime
Issue-Key: lexv2-models|CreateSlot|2xx->2xx|$headers.content-type
Issue-Commit: 9c26f3bece60e98f7a80e23ef343cd438e1e94fa

# lexv2-models restJson1 responses announce Content-Type application/x-amz-json-1.1 instead of application/json

> _Note: this issue was identified against the code base at commit `9c26f3bece60e98f7a80e23ef343cd438e1e94fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateCustomVocabularyItem

## Description
Lex Models V2 is a restJson1 service, so every response it serves — success or error — must announce the REST-JSON media type `application/json`. The emulator instead announces `application/x-amz-json-1.1`, the media type of the awsJson1_1 RPC protocol, on two distinct response paths. Both must be closed before this issue is submitted.

Cause 1 of 2: every modelled 4xx error response announces the awsJson1_1 media type.
AWS: Smithy `com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2` declares protocols: [restJson1] (version 2020-08-07, endpointPrefix models-v2-lex). Under restJson1 an error response carries the error document as `Content-Type: application/json` together with the `x-amzn-errortype` header; `application/x-amz-json-1.1` is the awsJson1_1 content type and is never served by this service. The recorded responses assert `content-type: application/json` for the refusals of BatchCreateCustomVocabularyItem (ValidationException/ResourceNotFoundException class, HTTP 400), ListTestExecutions (ValidationException, HTTP 400 — modelled errors InternalServerException 500, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400), DescribeTestExecution (HTTP 400) and StartTestExecution (HTTP 400). The status and error code the emulator picks for these refusals already match the recording; only the announced media type diverges.
Emulator: the same refusals are served with `content-type: application/x-amz-json-1.1`.
Why: a client or proxy that dispatches on the response media type (content negotiation, a logging/parsing middlebox, a hand-rolled client, or any tooling that asserts the protocol of the endpoint it talks to) sees an awsJson1_1 response from a REST-JSON endpoint. It either refuses to parse the error document or mis-identifies the service's protocol, so a perfectly well-formed ValidationException is surfaced as a transport/protocol fault instead of as the modelled client error.
Fix: the restJson1 error serializer for this service (the shared response-writing layer the generated service declaration in services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/lib.rs mounts) — the error path must emit the restJson1 content type, not the awsJson1_1 one.
Traces: lex-models-v2-custom-vocabulary-absent-refusals#task#7 (BatchCreateCustomVocabularyItem), lex-models-v2-test-execution-request-refusals#task#2 (ListTestExecutions, cluster also covers task#3 and task#5), lex-models-v2-test-execution-request-refusals#task#7 (DescribeTestExecution), lex-models-v2-test-execution-request-refusals#task#9 (StartTestExecution, cluster also covers task#10).

Cause 2 of 2: body-carrying success responses of the create operations announce the awsJson1_1 media type.
AWS: Smithy CreateSlotType (PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes, success 200), CreateIntent (PUT .../intents, 200) and CreateSlot (PUT .../intents/{intentId}/slots, 200) all serialize their output document under restJson1, i.e. with `Content-Type: application/json`. The recordings assert exactly that header on all three 200 responses.
Emulator: all three 200 responses are served with `content-type: application/x-amz-json-1.1`. The same corpus shows a GET success response (DescribeSlotType, lex-models-v2-slot-type-in-use-deletion-order#task#10) announcing the header correctly, so the defect is confined to the paths above rather than to every response.
Why: the body is REST-JSON but is labelled as an RPC-protocol payload, so any consumer that trusts the media type of a successful create — an SDK response interceptor, a recording proxy, a schema-validating gateway — treats the create's output as a different protocol's document than the one it actually is.
Fix: the success-response serializer for this service's body-carrying restJson1 operations (same shared response-writing layer), which must announce application/json.
Traces: lex-models-v2-slot-type-in-use-deletion-order#task#6 (CreateSlotType), #task#7 (CreateIntent), #task#8 (CreateSlot).

## Evidence
Cause 1 — lex-models-v2-custom-vocabulary-absent-refusals#task#7, BatchCreateCustomVocabularyItem: status 400 expected and 400 served (status matches); `$headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1')`. Identical single-key divergence on lex-models-v2-test-execution-request-refusals#task#2 ListTestExecutions (cluster: task#2, task#3, task#5, all 400), #task#7 DescribeTestExecution (400), #task#9 StartTestExecution (cluster: task#9, task#10, both 400).

Cause 2 — lex-models-v2-slot-type-in-use-deletion-order#task#6 CreateSlotType, #task#7 CreateIntent, #task#8 CreateSlot: status 200 expected and served; `$headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1')` on each.

Contrast (not a divergence): lex-models-v2-slot-type-in-use-deletion-order#task#10 DescribeSlotType, a GET 200, reported no content-type divergence — it announces application/json already.

Model evidence: smithy_query(service_info) on services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json → {"protocols": ["restJson1"], "sdk_id": "Lex Models V2", "version": "2020-08-07"}. No awsJson protocol trait is attached to this service.

## Steps to Reproduce
1. Boot the emulator and address the lexv2-models endpoint.
2. Cause 1: call ListTestExecutions with an input that fails modelled validation (e.g. maxResults outside the modelled range 1..1000) and inspect the response headers — HTTP 400 is correct, but the response announces Content-Type: application/x-amz-json-1.1 instead of application/json. Repeat with DescribeTestExecution (GET /testexecutions/{testExecutionId}) for an id that does not resolve, StartTestExecution for an absent test set, and BatchCreateCustomVocabularyItem against a bot locale that holds no custom vocabulary.
3. Cause 2: CreateBot, CreateBotLocale (DRAFT, en_US), then CreateSlotType, CreateIntent and CreateSlot in that locale. Each answers HTTP 200 with a correct JSON body but announces Content-Type: application/x-amz-json-1.1.
4. For contrast, call DescribeSlotType on the slot type just created: that 200 announces application/json.
