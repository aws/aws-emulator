Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-slot-type-in-use-deletion-order
Issue-Step: task#10
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-slot-type-in-use-deletion-order
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|DescribeSlotType|2xx->2xx|$.creationDateTime
Issue-Key: lexv2-models|DescribeSlotType|2xx->2xx|$.lastUpdatedDateTime
Issue-Related: 20261127T130356Z-lexv2-models-restjson1-responses-announce-content-type-application.md
Issue-Commit: 9c26f3bece60e98f7a80e23ef343cd438e1e94fa

# lexv2-models Timestamp members are serialized with sub-millisecond precision AWS never emits

> _Note: this issue was identified against the code base at commit `9c26f3bece60e98f7a80e23ef343cd438e1e94fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSlotType

## Description
Cause 1 of 1: every `Timestamp` member this service reports is serialized straight from the host clock's nanosecond reading, so the epoch-seconds number on the wire carries sub-millisecond fractional digits. AWS serializes these members with at most millisecond resolution, which is the shape the corpus asserts, and the assertion fails on every one of them.

AWS: Smithy lexv2-models models `creationDateTime` and `lastUpdatedDateTime` as `Timestamp` on DescribeSlotType, DescribeSlot, DescribeIntent, CreateSlotType, CreateIntent and CreateSlot (confirmed via smithy_query operation on all six). Under restJson1 a timestamp with no `@timestampFormat` override serializes as epoch-seconds, and the recorded AWS responses for this service carry at most three fractional digits — e.g. the recorded DescribeBotAlias history event in this corpus has `startDate: 1793167231.377`. The corpus therefore asserts the SHAPE of these members (`<shaped-not-asserted>`: the exact instant is environment-dependent and is not required), and the shape is "epoch seconds with millisecond resolution". Recorded behavior only for the resolution itself; the model states the member type and the protocol states the epoch-seconds encoding.
Emulator: the replayed responses carry values such as 1795784340.4771757 and 1795784340.4986298 — seven fractional digits, i.e. sub-microsecond resolution — and every asserted timestamp member fails the shape check. The source is state.rs's wire conversion, which builds the DateTime from `duration_since(UNIX_EPOCH)` keeping `subsec_nanos()` unrounded, so whatever precision the host clock has reaches the wire.
Why: a client that round-trips a Lex timestamp through a millisecond-resolution representation (Java `Instant`/`Date` via the v1 SDKs, JavaScript `Date`, any store with a millisecond-precision timestamp column) cannot reproduce the value the emulator served, so an equality check against a value it previously read back fails. Clients that key or de-duplicate resources on `creationDateTime`, or that compare a create response's timestamp with the one a later describe reports, see values AWS would never produce, and any fixture or snapshot recorded against the emulator is unmatchable against real AWS.
Fix: this service's timestamp-to-wire conversion, `to_date_time` in the service state module — it must truncate the instant to millisecond resolution before handing it to the serializer, so every operation that reports a stored instant (create responses and describe/list responses alike) emits the same millisecond-resolution epoch-seconds value AWS does.
Traces: lex-models-v2-slot-type-in-use-deletion-order#task#10 (DescribeSlotType — both creationDateTime and lastUpdatedDateTime fail), #task#6 (CreateSlotType creationDateTime), #task#7 (CreateIntent creationDateTime), #task#8 (CreateSlot creationDateTime). Conversion site: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/state.rs:1910-1918 (`to_date_time`, `DateTime::from_secs_and_nanos(.., since_epoch.subsec_nanos())`); a representative consumer is src/ops/describe_slot.rs:221-224.

## Evidence
lex-models-v2-slot-type-in-use-deletion-order#task#10 (DescribeSlotType, status 200 expected and served):
  $.creationDateTime: value_mismatch (expected '<shaped-not-asserted>', got 1795784340.4771757)
  $.lastUpdatedDateTime: value_mismatch (expected '<shaped-not-asserted>', got 1795784340.4771757)

lex-models-v2-slot-type-in-use-deletion-order#task#6 (CreateSlotType, 200): $.creationDateTime: value_mismatch (expected '<shaped-not-asserted>', got 1795784340.4771757)
lex-models-v2-slot-type-in-use-deletion-order#task#7 (CreateIntent, 200): $.creationDateTime: value_mismatch (expected '<shaped-not-asserted>', got 1795784340.4912102)
lex-models-v2-slot-type-in-use-deletion-order#task#8 (CreateSlot, 200): $.creationDateTime: value_mismatch (expected '<shaped-not-asserted>', got 1795784340.4986298)

Recorded AWS shape for the same service, same corpus: DescribeBotAlias botAliasHistoryEvents startDate = 1793167231.377 (three fractional digits).

Code: state.rs to_date_time keeps subsec_nanos() verbatim, so the fractional part is the host clock's full resolution rather than milliseconds. The specific instants above are per-run values and are not part of the requirement; the number of fractional digits is.

## Steps to Reproduce
1. Boot the emulator and address the lexv2-models endpoint.
2. CreateBot, then CreateBotLocale for DRAFT/en_US.
3. CreateSlotType in that locale, then CreateIntent, then CreateSlot referencing the slot type. Inspect the raw JSON of each 200 response: creationDateTime is an epoch-seconds number with more than three fractional digits.
4. Call DescribeSlotType for that slot type: creationDateTime and lastUpdatedDateTime likewise carry sub-millisecond fractional digits.
5. Compare with a recorded AWS response for this service, where the epoch-seconds value never carries more than millisecond resolution.
