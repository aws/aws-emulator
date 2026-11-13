Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4110983ca6b6466b7658f165e005d75330506d1

# PutRecord ignores the feature group's default TtlDuration, so a record ingested without a request-level TTL never expires and reports no ExpiresAt

_Filed: 2026-11-11T17:31:25Z_
_Operation: PutRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4110983ca6b6466b7658f165e005d75330506d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states that an ingestion with no request-level `TtlDuration` falls back to the feature group's own default: "If the input TtlDuration is null or unspecified, TtlDuration is set to the default feature group level TtlDuration. A record level TtlDuration supersedes the group level TtlDuration." The record preparation every whole-record write of this data plane shares (`crate::state::prepare_record`, called by PutRecord and by BatchWriteRecord) derives the TTL from the REQUEST only: when `ttl_duration` is `None` it sets `ttl_secs = None`, and the group definition it is handed (`FeatureGroupRecord`, which carries `default_ttl_seconds`) is read only for the two feature names. Consequence: a record put into a feature group created with `OnlineStoreConfig.TtlDuration` is stored with `expires_at = None` and `ttl_secs = None`, so (a) `FeatureGroupStore::purge_expired` never hard-deletes it, and (b) `GetRecord`/`BatchGetRecord` with `ExpirationTimeResponse=Enabled` report no `ExpiresAt` at all for a record that has one in AWS.

The crate contradicts itself about that same group-level default: UpdateRecord REFUSES an update with 400 ValidationError whenever the group has a default TTL and the request carries no EventTime, on the stated ground that the record's expiry is re-derived from its event time — a default TTL that no code in the crate ever applies to a record. `definition.default_ttl_seconds` is read in exactly one place in `src/` (update_record.rs:313), and nowhere is it ever turned into a stored expiry.

The fix belongs in the one shared preparation (`prepare_record`), so PutRecord and BatchWriteRecord cannot diverge.

## Evidence

Smithy, PutRecord documentation (smithy_query query="doc" shape="PutRecord"), also reproduced in the handler header at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/put_record.rs:67-69:
"A record level TtlDuration is set when specifying the TtlDuration parameter using the PutRecord API call. If the input TtlDuration is null or unspecified, TtlDuration is set to the default feature group level TtlDuration. A record level TtlDuration supersedes the group level TtlDuration."

The code that drops it — services/.../src/state.rs:435-447 (`prepare_record`):
```
    let ttl_secs = match ttl_duration {
        None => None,
        Some(ttl) => match ttl_seconds(ttl) {
```
and services/.../src/state.rs:449-458 stores exactly that: `ttl_secs,` then `stored.recompute_expiry();` — which, per services/.../src/state.rs:290-297, sets BOTH `expires_at` and `expires_at_secs` to `None` when `ttl_secs` is `None`. The `definition` parameter is used only for `record_identifier_feature_name` (state.rs:384) and `event_time_feature_name` (state.rs:410); `definition.default_ttl_seconds` is never read there.

PutRecord calls exactly that preparation — services/.../src/ops/put_record.rs:244-246:
```
        let prepared =
            crate::state::prepare_record(&definition, &input.record, input.ttl_duration.as_ref())
                .map_err(validation)?;
```

The same crate's other side of the rule — services/.../src/ops/update_record.rs:306-316:
```
        let ttl_in_force =
            input.ttl_duration.is_some() || definition.default_ttl_seconds.is_some();
        if ttl_in_force && posted_event_time.is_none() {
            return Err(validation(EVENT_TIME_REQUIRED_FOR_TTL));
        }
```
with the module doc stating the rule it enforces (services/.../src/ops/update_record.rs:19-22): "Because that expiry is derived from the event time, an update made under a TTL — the request's own `TtlDuration`, or the default one the feature group's `OnlineStoreConfig` configures — must carry an `EventTime`". So the crate refuses requests on account of a default TTL it never applies.

The field exists and is populated by the control plane in this repo: services/common/src/feature_store_groups.rs:74-81 — "The DEFAULT time-to-live the group's `OnlineStoreConfig.TtlDuration` states, in seconds … so a default TTL makes every record of the group carry an expiry derived from its own event time" / `pub default_ttl_seconds: Option<f64>,` (written at services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_feature_group.rs:317-320).

Test coverage: no test pins the default-TTL ingestion path. tests/all/online_store_record_lifecycle.rs:550-569 pins "A record written with no TtlDuration has no ExpiresAt at all", but its feature group is created with `default_ttl_seconds: None` (tests/all/online_store_record_lifecycle.rs:72), so it does not cover a group-level default. The only test that creates a group WITH a default TTL (tests/all/refusal_message_wording.rs:83-95, rows at :656-670) asserts the UpdateRecord refusal message only.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with `OnlineStoreConfig.EnableOnlineStore=true` and `OnlineStoreConfig.TtlDuration = {Unit: "Seconds", Value: 1}` (so the shared directory records `default_ttl_seconds = Some(1.0)`).
2. PUT /FeatureGroup/customers with a Record carrying the identifier feature and an event-time feature value of now, and NO `TtlDuration` member.
3. GET /FeatureGroup/customers?RecordIdentifierValueAsString=<id>&ExpirationTimeResponse=Enabled — the response carries no `ExpiresAt`, because prepare_record (state.rs:435-447) stored `ttl_secs = None`.
4. Wait past the group's default TTL and repeat the GetRecord — the record is still served: `FeatureGroupStore::purge_expired` (state.rs:128-135) has no `expires_at_secs` to act on.
5. Contrast: POST /FeatureGroup/customers/Record (UpdateRecord) with no EventTime feature is refused 400 with EVENT_TIME_REQUIRED_FOR_TTL (update_record.rs:313-316) precisely because the group has that default TTL.
