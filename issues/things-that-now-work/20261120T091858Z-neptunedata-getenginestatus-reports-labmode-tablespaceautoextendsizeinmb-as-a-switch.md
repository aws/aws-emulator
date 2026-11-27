Issue-Source: trace-sweep-divergence
Issue-Trace: neptunedata-query-languages-live-cluster-round-trip
Issue-Step: task#18
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptunedata-query-languages-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata|GetEngineStatus|2xx->2xx|$.labMode.TableSpaceAutoExtendSizeInMB
Issue-Commit: c5b9149d70e2a9aad6a1bceb8aa9795c91a9cab7

# neptunedata GetEngineStatus reports labMode TableSpaceAutoExtendSizeInMB as a switch word instead of a size value

> _Note: this issue was identified against the code base at commit `c5b9149d70e2a9aad6a1bceb8aa9795c91a9cab7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEngineStatus

## Description
GetEngineStatus (GET /status) returns 200 with the right overall shape, but one entry of its `labMode` map carries a value of the wrong kind.

Cause 1 of 1: `$.labMode.TableSpaceAutoExtendSizeInMB` is reported as the word `disabled`.

AWS: Smithy: `GetEngineStatusOutput$labMode` targets `StringValuedMap` (map<String,String>) and is documented only as "Contains Lab Mode settings being used by the engine" — the model constrains neither the key set nor the value vocabulary, so this is recorded behavior only. The recording shows the Neptune /status answer carrying the `TableSpaceAutoExtendSizeInMB` lab-mode entry with an EMPTY value, not a status word: the parameter names a SIZE IN MB (its own name says so), so when no auto-extend size is in force the entry reports no size rather than the on/off vocabulary its sibling switch parameters (`ObjectIndex`, `ReadWriteConflictDetection`) use. The sibling switch entries in the same map matched, so the divergence is isolated to this one key's value kind. The status (200) and every other member of the answer are correct; nothing about an error code or message is involved here.

Emulator: the replayed step reports `$.labMode.TableSpaceAutoExtendSizeInMB: value_mismatch (expected '', got 'disabled')` — the emulator hard-codes this lab-mode entry to the shared off-switch word. In the checkout, `TABLE_SPACE_AUTO_EXTEND_SIZE_MB` is defined as an alias of `FEATURE_DISABLED` ("disabled") and is seeded into `EngineStatusRecord::default().lab_mode` alongside the two genuine switch parameters, so every /status read echoes it.

Why: a client reading `labMode` to discover the engine's auto-extend size parses this entry as a number of megabytes (or as "unset" when it is empty). Against AWS it gets an empty value and treats the size as not configured; against the emulator it gets the literal string `disabled`, which is neither a size nor a value the real engine ever reports for that key. Code that parses the entry as an integer fails outright, and code that switches on the on/off vocabulary concludes a size-valued parameter is a toggle — a conclusion that does not hold against real Neptune.

Fix: the GetEngineStatus state defaults, not the handler or the serializer. The lab-mode seed must distinguish a size-valued lab-mode parameter from a switch-valued one: `TableSpaceAutoExtendSizeInMB` should report the configured size, and the absence of a configured size must be reported as the empty value the recording shows, never borrowed from the feature-status vocabulary.

Traces: neptunedata-query-languages-live-cluster-round-trip#task#18 (GetEngineStatus, 200 -> 200, body value mismatch on `$.labMode.TableSpaceAutoExtendSizeInMB`).

Related but NOT the same defect: the open issue `20261113T062625Z-neptunedata-executegremlinexplainquery-explain-report-diverges-from-neptune-s.md` owns the ExecuteGremlinExplainQuery report divergence in the same fixture; it does not cover /status or `labMode`.

## Evidence
Fixture neptunedata-query-languages-live-cluster-round-trip, step task#18, operation GetEngineStatus.

Expected status 200, actual status 200 — the divergence is in the body:
  detail: $.labMode.TableSpaceAutoExtendSizeInMB: value_mismatch (expected '', got 'disabled')

Expected (recorded AWS): the `labMode` map contains key `TableSpaceAutoExtendSizeInMB` with an empty value — no auto-extend size in force.
Actual (emulator): the same key carries the string `disabled`.

Sibling lab-mode keys (`ObjectIndex`, `ReadWriteConflictDetection`) and all other GetEngineStatus members compared equal; this one map value is the whole divergence.

Source in the checkout:
  services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/state/engine_status.rs:111-123 — `TABLE_SPACE_AUTO_EXTEND_SETTING = "TableSpaceAutoExtendSizeInMB"`, `TABLE_SPACE_AUTO_EXTEND_SIZE_MB = FEATURE_DISABLED` (= "disabled").
  services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/state/engine_status.rs:245-255 — that constant is seeded into `lab_mode` next to the two switch parameters.
  services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/ops/get_engine_status.rs:227,245 — the handler emits the stored map verbatim as `labMode`.

Model reference: smithy_query(query="shape", shape="GetEngineStatusOutput") — `labMode` targets `StringValuedMap`, docs: "Contains Lab Mode settings being used by the engine." No value vocabulary is modelled, so the recording is the only authority for this entry's value kind.

## Steps to Reproduce
1. Boot the emulator from this checkout (it answers /__admin/health 200 and accepts on the data listener).
2. Replay fixture neptunedata-query-languages-live-cluster-round-trip, or issue the step directly: GET /status against the neptunedata data-plane listener (no request body; GetEngineStatus input is smithy.api#Unit).
3. Read the 200 JSON response body and inspect `$.labMode`.
4. Observe `"TableSpaceAutoExtendSizeInMB": "disabled"`. The recorded AWS answer for the same step carries that key with an empty value, so the replayed step task#18 reports value_mismatch (expected '', got 'disabled').
