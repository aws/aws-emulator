Issue-Source: trace-sweep-divergence
Issue-Trace: rum-page-view-event-is-returned-by-get-app-monitor-data
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-page-view-event-is-returned-by-get-app-monitor-data
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|GetAppMonitorData|2xx->2xx|$.Events[*]
Issue-Commit: e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7

# RUM event document places client-sent metadata domain before the client's other members instead of with the service-set members

> _Note: this issue was identified against the code base at commit `e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAppMonitorData

## Description
ONE root cause. GetAppMonitorData answers 200 with the right event, but the enriched RUM event document it reports orders the `metadata` members — and therefore the parallel `metadata_keys` / `metadata_values` arrays — differently from AWS, because the emulator treats `domain` as a client-serialized member it must leave in place rather than as a member RUM sets at ingestion.

Cause 1 of 1: `domain` is reported at the client's position instead of with the service-set metadata members.

AWS: Smithy says only that `RumEvent$metadata` is a `@mediaType("application/json")` `JsonValue` — "Metadata about this event, which contains a JSON serialization of the identity of the user for this session. The user information comes from information such as the HTTP user-agent request header" — and that `metadata`, `metadata_keys` and `metadata_values` are first-level fields of the event document a custom metric may key on (`smithy_query(query="doc", shape="MetricDefinitionRequest")`). The ORDER is recorded behavior only: in the recorded GetAppMonitorData response the document carries the members the client serialized first, in the client's own order (`version`, `pageId`, `title`), and then the members RUM itself sets at ingestion LAST — `domain`, then `countryCode`, `subdivisionCode`, `localityName`. So in AWS's document `domain` sits with the service-set members, not where the sending client serialized it, even though the client's own metadata for this batch did carry a `domain`. The requirement is that placement rule; the member VALUES (and the geolocation values in particular) are not part of it.

Emulator: the ingestion enrichment adds only the three geolocation members — `caller_location.rs`'s `CallerLocation::members()` yields `countryCode`, `subdivisionCode`, `localityName` — and `rum_event.rs`'s `reported_metadata` merges them with `entry(..).or_insert_with(..)`, which appends only members the client did NOT send. A client-sent `domain` is therefore left exactly where the client serialized it. For this fixture's batch the emulator reports `metadata_keys` = ["version","domain","pageId","title","countryCode","subdivisionCode","localityName"] (and the same order inside `metadata` and `metadata_values`) where AWS reports ["version","pageId","title","domain","countryCode","subdivisionCode","localityName"]. HTTP status is 200 on both sides and no error is involved; only the `Events[0]` document bytes diverge.

Why: `metadata_keys` and `metadata_values` are two FLAT parallel arrays, so position is the only thing that pairs a key with its `<key>=<value>` entry; a consumer that reads them positionally — the documented way to recover the pairs — reads a different key at every index from 1 onward and mis-attributes the page identity to `domain` and the domain to `pageId`. The same reordering changes the member order of the `metadata` object a client deserializes or hashes, and the document is the one an extended/custom metric's `EventPattern` and a `GetAppMonitorData` `Country` filter are judged against.

Fix: the ingestion enrichment in the event-document builder — `services/service-com-amazonaws-rum-rum/src/state/rum_event.rs` (`reported_metadata`, whose output `metadata_projections` then flattens) together with the member list in `services/service-com-amazonaws-rum-rum/src/state/caller_location.rs`. `domain` must become a member RUM SETS at ingestion: removed from wherever the client serialized it and re-inserted after every client-serialized member, ahead of `countryCode`/`subdivisionCode`/`localityName`. `or_insert`-style merging cannot express that, since it only appends absent members.

NOT part of this cause (same diff, but values no replay can reproduce, deliberately excluded): `event_timestamp` and `log_stream` (clock instants — the replayed request carries a replay-time `RumEvent$timestamp`, which the emulator echoes correctly), `batch_id` (the per-run uuid the replayed `PutRumEvents` request supplies; the emulator does echo `PutRumEventsRequest$BatchId`), and `subdivisionCode`/`localityName` (AWS geolocates the caller's address, which for a loopback replay has no answer; the emulator's constant is documented as its own). A fixer should NOT change any of those.

Traces: rum-page-view-event-is-returned-by-get-app-monitor-data#task#3 (GetAppMonitorData, verdict mismatch on $.Events[0], expected 200 / actual 200).

## Evidence
Fixture rum-page-view-event-is-returned-by-get-app-monitor-data, step task#3, operation GetAppMonitorData, verdict mismatch, expected status 200, actual status 200, diverging key $.Events[0].

Expected (recorded) event document, metadata section:
  "metadata":{"version":"1.0.0","pageId":"/checkout","title":"Checkout","domain":"example.com","countryCode":"US","subdivisionCode":"OR","localityName":"Boardman"}
  "metadata_keys":["version","pageId","title","domain","countryCode","subdivisionCode","localityName"]
  "metadata_values":["version=1.0.0","pageId=/checkout","title=Checkout","domain=example.com","countryCode=US","subdivisionCode=OR","localityName=Boardman"]

Actual (emulator) event document, metadata section:
  "metadata":{"version":"1.0.0","domain":"example.com","pageId":"/checkout","title":"Checkout","countryCode":"US","subdivisionCode":"WA","localityName":"Seattle"}
  "metadata_keys":["version","domain","pageId","title","countryCode","subdivisionCode","localityName"]

The `domain` member moves from index 3 (AWS) to index 1 (emulator), shifting `pageId` and `title` by one position in both parallel arrays. Every other field of the document that a replay can reproduce agrees: userId, sessionId, application_id, application_version, application_name, event_id, event_type, event_version, account_id and the event_details / user_details sub-documents all match byte for byte.

Code confirming the cause: src/state/caller_location.rs CallerLocation::members() returns only (countryCode, subdivisionCode, localityName); src/state/rum_event.rs reported_metadata() merges them with members.entry(member).or_insert_with(..), so a client-supplied `domain` is never re-positioned.

Suite context: 283 steps replayed, 280 passed, 3 mismatched (score 98.9%), emulator up. This step was neither blocked by a stub nor owned by an open issue. The open issue 20261029T192752Z-rum-app-monitor-data-plane-resource-policy-tagging.md (behavioral corpus) covers RUM data-plane readback, resource-policy, tagging and CFN causes; its Cause 2 is "events accepted by PutRumEvents never become readable through GetAppMonitorData". That is a DIFFERENT cause and does not cover this one: here the event IS readable and the response is 200 — only the metadata member order inside the returned document diverges.

## Steps to Reproduce
1. Boot the emulator and replay the wire fixture corpus (simparity-wire-aws-v1); step task#3 of fixture rum-page-view-event-is-returned-by-get-app-monitor-data mismatches on $.Events[0].

Manual repro:
1. CreateAppMonitor(Name=m, DomainList=["example.com"]) and note the returned Id.
2. PutRumEvents(Id=<Id>, BatchId=<uuid>, AppMonitorDetails={name: m, version: "1.0.0"}, UserDetails={userId: <uuid>, sessionId: <uuid>}, RumEvents=[{id: <uuid>, timestamp: <now>, type: "com.amazon.rum.page_view_event", metadata: '{"version":"1.0.0","domain":"example.com","pageId":"/checkout","title":"Checkout"}', details: '{"version":"1.0.0","pageId":"/checkout","pageInteractionId":"/checkout-0","interaction":0}'}]) — note that the client's metadata serializes `domain` BEFORE `pageId`/`title`.
3. GetAppMonitorData(Name=m, TimeRange={After: now_ms-3600000, Before: now_ms+60000}, MaxResults=100) and parse Events[0] as JSON.
4. Observed: metadata_keys == ["version","domain","pageId","title","countryCode","subdivisionCode","localityName"] — the client's `domain` stays at index 1.
   Expected: metadata_keys == ["version","pageId","title","domain","countryCode","subdivisionCode","localityName"] — every client-serialized member first in the client's order, then the members RUM sets at ingestion (`domain`, then the geolocation members). The same order must hold inside the `metadata` object and in `metadata_values`.
5. Repeat with client metadata that carries NO `domain` ('{"version":"1.0.0","pageId":"/checkout"}'): `domain` must still be present and still sit after `pageId` and ahead of `countryCode`, so the fix must SET the member rather than only fill it in when absent.
