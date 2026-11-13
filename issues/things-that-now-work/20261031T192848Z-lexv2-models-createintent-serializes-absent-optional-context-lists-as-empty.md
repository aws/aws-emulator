Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-locale-slot-pipeline
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-locale-slot-pipeline lex-models-v2-bot-version-alias-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|CreateIntent|2xx->2xx|$.inputContexts
Issue-Key: lexv2-models|CreateIntent|2xx->2xx|$.outputContexts
Issue-Commit: 88218030aece2fbf10d6f0dd92e09bf5ad53454a

# CreateIntent serializes absent optional context lists as empty arrays instead of omitting them

> _Note: this issue was identified against the code base at commit `88218030aece2fbf10d6f0dd92e09bf5ad53454a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntent

## Description
Cause 1 of 1: CreateIntent's restJson1 response body materializes the optional list members `inputContexts` and `outputContexts` as empty JSON arrays when the request supplied no contexts, where AWS omits those members from the document entirely.

AWS: Smithy model services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json, operation CreateIntent (PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents, success code 200). On CreateIntentResponse, `inputContexts` targets InputContextsList (@length min 0 max 5) and `outputContexts` targets OutputContextsList (@length min 0 max 10); neither member is @required and neither carries a @default. Under restJson1 a structure member that has no value is not serialized at all, so for an intent created without contexts AWS returns a 200 body with no `inputContexts` and no `outputContexts` key. The recorded response confirms this: both keys are absent (expected value None). The rest of the response (intentId, intentName, botId, botVersion, localeId, creationDateTime) matches, and the status is 200 on both sides — this is purely the presence/absence of the two optional list members.

Emulator: answers 200 but emits `"inputContexts": []` and `"outputContexts": []`. Replay verdict `mismatch`, detail `$.inputContexts: type_mismatch (expected None, got [])` and `$.outputContexts: type_mismatch (expected None, got [])` (divergence key `service=lexv2-models|CreateIntent|status=200->200|keys=$.inputContexts,$.outputContexts`). It reproduces on every CreateIntent call in the corpus that omits contexts (2 recorded steps in the cluster), so it is the serializer defaulting an unset Option<Vec<_>> to an empty vector rather than anything fixture-specific.

Why: a client cannot distinguish "no contexts configured" from "contexts member not returned". In boto3 `'inputContexts' in response` is True against the emulator and False against AWS, so presence tests and `response.get('inputContexts')` guards take the wrong branch; in the typed SDKs the auto-construct-list marker is lost, so `hasInputContexts()`/`hasOutputContexts()` report true and a read-modify-write that feeds the CreateIntent output back into UpdateIntent now sends explicit empty lists where AWS sent nothing. Any response-equality or recorded-response assertion over the intent document also fails.

Fix: the CreateIntent response serializer / output shape construction in the lexmodelsv2 crate's CreateIntent handler (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_intent.rs and the CreateIntentResponse serialization it uses). Absent optional list members must stay None through to serialization and be skipped in the JSON document, not normalized to an empty collection; an empty list supplied explicitly by the request should still round-trip as `[]`. The same defaulting should be checked on the sibling intent-reading operations that echo these members.

Note: the open issue "Implement lex-models-v2 CreateIntent: unimplemented scaffold stub" (20261029T235013Z-lex-models-v2-createintent-stub.md) was filed against an older commit where CreateIntent answered 501; the operation now answers 200, that issue carries no wire replay key for this step, and the replay reports this cluster as unowned with no stub path, so this fidelity divergence is net-new and not covered by it.

Traces: lex-models-v2-bot-locale-slot-pipeline#task#7 (representative), lex-models-v2-bot-version-alias-lifecycle#task#8 (same cluster).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `lex-models-v2-bot-locale-slot-pipeline`, step `task#7` (operation CreateIntent, service lexv2-models):

  verdict: mismatch
  expected_status: 200   actual_status: 200
  detail:
    $.inputContexts: type_mismatch (expected None, got [])
    $.outputContexts: type_mismatch (expected None, got [])
  divergence_key: service=lexv2-models|CreateIntent|status=200->200|keys=$.inputContexts,$.outputContexts
  affected (2): lex-models-v2-bot-locale-slot-pipeline#task#7, lex-models-v2-bot-version-alias-lifecycle#task#8
  stub_path: (empty)   owned_by: (empty)

Expected (AWS, recorded): 200 with a JSON document that contains NO `inputContexts` key and NO `outputContexts` key.
Actual (emulator): 200 with `"inputContexts": []` and `"outputContexts": []` present in the document.

Model basis: CreateIntentResponse.inputContexts -> InputContextsList (@length min 0, max 5), CreateIntentResponse.outputContexts -> OutputContextsList (@length min 0, max 10); neither @required, neither with @default; protocol restJson1, so an unset member is not serialized.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm the lexv2-models (com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2) endpoint is serving.
2. CreateBot, then CreateBotLocale for a DRAFT version / locale, so a locale exists to hold the intent.
3. Call CreateIntent on that bot/DRAFT/locale with only `intentName` (plus optional sampleUtterances) and WITHOUT `inputContexts` or `outputContexts` — e.g. PUT /bots/{botId}/botversions/DRAFT/botlocales/{localeId}/intents with body {"intentName": "<name>"}.
4. Inspect the raw 200 response body. The emulator includes `"inputContexts": []` and `"outputContexts": []`; AWS omits both keys. In boto3, `'inputContexts' in response` is True here and False against AWS.
5. Equivalently, replay fixture `lex-models-v2-bot-locale-slot-pipeline` step `task#7` (or `lex-models-v2-bot-version-alias-lifecycle` step `task#8`) from trace set simparity-wire-aws-v1 and observe the two `type_mismatch (expected None, got [])` entries.
