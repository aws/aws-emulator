Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-slot-type-in-use-deletion-order
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-slot-type-in-use-deletion-order
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|DescribeSlot|4xx->2xx|$.message
Issue-Key: lexv2-models|DescribeSlot|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lexv2-models|DescribeSlot|4xx->2xx|status
Issue-Key: lexv2-models|DescribeSlotType|4xx->2xx|$.message
Issue-Key: lexv2-models|DescribeSlotType|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lexv2-models|DescribeSlotType|4xx->2xx|status
Issue-Key: lexv2-models|DescribeIntent|4xx->2xx|$.message
Issue-Key: lexv2-models|DescribeIntent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lexv2-models|DescribeIntent|4xx->2xx|status
Issue-Commit: 9c26f3bece60e98f7a80e23ef343cd438e1e94fa

# lexv2-models DescribeSlot/DescribeSlotType/DescribeIntent still serve a deleted resource with HTTP 200

> _Note: this issue was identified against the code base at commit `9c26f3bece60e98f7a80e23ef343cd438e1e94fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSlot

## Description
Cause 1 of 1: after a slot, slot type and intent are deleted from a bot locale, reading any of them back still succeeds with HTTP 200 and the full resource document instead of being refused as absent. The delete leaves the record in the store, so the whole tear-down half of a locale's lifecycle reports the wrong state.

AWS: Smithy lexv2-models DescribeSlot (GET /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots/{slotId}), DescribeSlotType (GET .../slottypes/{slotTypeId}) and DescribeIntent (GET .../intents/{intentId}) each model `ResourceNotFoundException` with httpError 404, documented as "You asked to describe a resource that doesn't exist. Check the resource that you are requesting and try again." A read of a resource this service does not hold is answered HTTP 404, error code `ResourceNotFoundException` (announced in the `x-amzn-errortype` header), with the body message `Failed to retrieve resource since it does not exist` — fixed text in full, carrying no request-derived part, and the same sentence this service uses for any absent resource whatever identified it (recorded behavior; the emulator already holds it as state.rs's RESOURCE_NOT_FOUND_MESSAGE). DeleteSlot/DeleteSlotType/DeleteIntent answer 204 and the resource is gone from that bot/version/locale from then on.
Emulator: each read after the corresponding delete answers HTTP 200 with the resource still present — no status 404, no `message`, no `x-amzn-errortype` header. The read handlers themselves are correct: describe_slot.rs resolves the slot under its bot/version/locale/intent and returns ResourceNotFoundException with exactly that message when the lookup misses. What is wrong is that nothing ever removes the record — DeleteSlot, DeleteSlotType and DeleteIntent are still 501 `NotImplemented` scaffold stubs (each src/ops/delete_*.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and ends in `Scaffolded::not_implemented(..)`), so the delete steps of this fixture are refused and the subsequent reads keep serving the stale resource. Related open issue: "lexv2-models: 3 net-new causes — CreateUploadUrl, StartImport and DeleteSlotType are still 501 scaffold stubs" (20261127T103301Z-lexv2-models-3-net-new-causes-createuploadurl-startimport.md) covers DeleteSlotType's 501 from the behavioral suite; it does NOT carry these wire steps' replay keys, and does not cover DeleteSlot or DeleteIntent, so the read-after-delete divergence recorded here is unowned.
Why: a client cannot tear a bot locale down. It deletes a slot, its slot type and its intent, reads them back to confirm, and is told they all still exist — so dependency-ordered clean-up loops never terminate, re-creating a resource under the same name hits a conflict against a record the client believes it removed, and code that distinguishes "already gone" (404) from "still there" (200) takes the wrong branch every time.
Fix: the delete handlers for these three resources — DeleteSlot, DeleteSlotType and DeleteIntent must remove the addressed record (and, for a slot type, honour the in-use check) and answer their modelled success status, so the already-correct describe path then reports 404 ResourceNotFoundException with the fixed message above.
Traces: lex-models-v2-slot-type-in-use-deletion-order#task#12 (DescribeSlot after the slot is deleted), #task#14 (DescribeSlotType after the slot type is deleted), #task#16 (DescribeIntent after the intent is deleted). Handlers: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/delete_slot.rs, delete_slot_type.rs, delete_intent.rs; the correct read-side refusal is src/ops/describe_slot.rs:193-208 and the message constant is state.rs RESOURCE_NOT_FOUND_MESSAGE.

## Evidence
lex-models-v2-slot-type-in-use-deletion-order#task#12, DescribeSlot:
  status: expected 404, got 200
  $.message: missing (expected 'Failed to retrieve resource since it does not exist', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

lex-models-v2-slot-type-in-use-deletion-order#task#14, DescribeSlotType — same three keys, expected 404, got 200.
lex-models-v2-slot-type-in-use-deletion-order#task#16, DescribeIntent — same three keys, expected 404, got 200.

Expected per cause: HTTP 404, x-amzn-errortype naming ResourceNotFoundException, body message exactly `Failed to retrieve resource since it does not exist` (entirely fixed text — no value from the request or state appears in it). Actual: HTTP 200 carrying the deleted resource's full document, no error code and no error message.

Supporting code read: src/ops/delete_slot.rs:3-6 and src/ops/delete_intent.rs:3-6 both carry `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`; src/ops/describe_slot.rs:200-207 already returns ResourceNotFoundException with crate::state::RESOURCE_NOT_FOUND_MESSAGE on a miss, so the stale 200 comes from the record never being removed.

## Steps to Reproduce
1. Boot the emulator and address the lexv2-models endpoint.
2. CreateBot; CreateBotLocale for DRAFT/en_US; CreateSlotType with sample values; CreateIntent; CreateSlot whose slotTypeId is that slot type.
3. DeleteSlot for the slot, then DescribeSlot for the same slotId — the read answers HTTP 200 with the slot document instead of 404 ResourceNotFoundException 'Failed to retrieve resource since it does not exist'.
4. DeleteSlotType, then DescribeSlotType for the same slotTypeId — same stale 200.
5. DeleteIntent, then DescribeIntent for the same intentId — same stale 200.
6. Observe that each Delete* call is itself answered HTTP 501 NotImplemented, which is why the records survive.
