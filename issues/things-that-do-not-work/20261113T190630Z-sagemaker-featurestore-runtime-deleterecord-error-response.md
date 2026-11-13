Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 22f94653eba30c921a9f5299a2c0aa92b971d5f1

# An out-of-order HardDelete aimed only at the OfflineStore answers 200 and appends the delete marker, because the EventTime ordering check lives inside the OnlineStore branch

_Filed: 2026-11-13T19:06:30Z_
_Operation: DeleteRecord | Class: error-response_

> _Note: this issue was identified against the code base at commit `22f94653eba30c921a9f5299a2c0aa92b971d5f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteRecord decides the documented out-of-order refusal INSIDE the block it only enters when the request targets the OnlineStore and the group has one. `refused_out_of_order` is initialised to `false` and is only ever set from inside `if crate::state::targets_online_store(...) && definition.online_store_enabled`, so a `HardDelete` whose `EventTime` is strictly older than the stored record's — a record that exists in the OnlineStore and carries a readable event time — is never compared against it when the caller passes `TargetStores=["OfflineStore"]`. The handler then falls through to the offline append and answers 200 with the deleted-record marker written.

The model conditions the refusal on the event time of the record in the OnlineStore, not on which stores the request names, and the crate's own module doc states the rule unconditionally too. The sibling operation does the opposite of this handler: UpdateRecord deliberately evaluates its event-time ordering rule BEFORE the `TargetStores` shortcut and skips only the WRITE, with a comment stating exactly that reasoning. DeleteRecord skips the ordering DECISION itself, so the same request is judged by the request's own rules in one handler and not in the other, and a refused deletion leaves a marker the model says is not written.

## Evidence

Model, `smithy_query(query="doc", shape="DeleteRecord")`: "Note that the <code>EventTime</code> specified in <code>DeleteRecord</code> should be set later than the <code>EventTime</code> of the existing record in the <code>OnlineStore</code> for that <code>RecordIdentifer</code>. If it is not, the deletion does not occur: ... <code>HardDelete</code> returns <code>EventTime</code>: <code>400 ValidationException</code> to indicate that the delete operation failed. No delete record marker is written to the <code>OfflineStore</code>." (reproduced in the crate at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/delete_record.rs:68-74).

The handler's own module doc states the rule with no `TargetStores` condition — services/.../src/ops/delete_record.rs:14-19:
```
//! The deletion is also APPENDED to the offline store of a group the control plane gave an
//! `OfflineStoreConfig`, under that group's own `RoleArn`, as the deleted-record marker
//! ([`crate::state::offline_store::OfflineRecord::delete_marker`]): "In both cases, Feature
//! Store appends the deleted record marker to the OfflineStore", and a `SoftDelete` whose
//! `EventTime` is out of order appends it too, while the `HardDelete` the same `EventTime`
//! refuses appends nothing.
```

The code that makes the verdict depend on `TargetStores` — services/.../src/ops/delete_record.rs:289-331:
```
        let refused_out_of_order = {
            let mut refused = false;
            if crate::state::targets_online_store(input.target_stores.as_ref())
                && definition.online_store_enabled
            {
                let mut store = state.lock();
                if let Some(group) = store.group_mut(&input.feature_group_name) {
                    if let Some(stored_secs) =
                        group.record(&identifier).map(|stored| stored.event_time_secs())
                    {
                        ...
                        let out_of_order =
                            stored_secs.is_some_and(|existing| requested_secs < existing);
                        if out_of_order {
                            refused = hard_delete;
```
and what happens when that block is skipped — services/.../src/ops/delete_record.rs:335-362:
```
        if refused_out_of_order {
            return Err(validation(DELETE_EVENT_TIME_OUT_OF_ORDER));
        }
        ...
        if crate::state::targets_offline_store(input.target_stores.as_ref()) {
            if let Some(delivery) = definition.offline_store.as_ref() {
                crate::state::offline_store::replicate(
                    ...
                    &crate::state::offline_store::OfflineRecord::delete_marker(
```
so `refused_out_of_order` is `false`, the 400 is not answered and the marker IS appended.

The sibling handler that keeps the ordering rule independent of `TargetStores` — services/.../src/ops/update_record.rs:402-426:
```
        if let (Some(posted), Some(current)) = (posted_secs, updated.event_time_secs()) {
            if posted < current {
                return Err(UpdateRecordErrorModeled::ConflictException(ConflictException {
```
```
        // This service is the OnlineStore data plane, so a request naming
        // only the OfflineStore leaves the online record standing. It is answered HERE,
        // and not earlier: the request's own rules, the record's existence and the
        // ordering against its event time hold whichever store an update is aimed at, so
        // an offline-only request is validated and resolved exactly as any other and only
        // the WRITE is skipped.
        if !crate::state::targets_online_store(input.target_stores.as_ref()) {
            return Ok(UpdateRecordOutput);
        }
```
DeleteRecord itself already hoists its `EventTime` PARSE out of the store branch for the same reason (delete_record.rs:264-275: "a value this operation cannot read is a request defect, not something an OfflineStore-only deletion excuses"), but leaves the ORDERING behind it.

Test coverage: no test pins the combination. The `TargetStores` matrix in tests/all/offline_store_object_visibility.rs:684-693 has only `("a deletion aimed at the OfflineStore alone", event_time: "2026-06-02T08:00:00Z")` — AFTER the seeded record's time — while the out-of-order HardDelete row (:672-683, status 400, partition None) names no `TargetStores` at all (`target_stores: ""`); the suite's own stated rule at tests/all/offline_store_object_visibility.rs:595-597 is unconditional: "the one deletion it refuses (a `HardDelete` whose `EventTime` is older than the record's) appends nothing".

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=record_id, EventTimeFeatureName=event_time, OnlineStoreConfig.EnableOnlineStore=true and an OfflineStoreConfig whose role may write the bucket.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:record_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T07:00:00Z"}] — 200, the record is in the OnlineStore with that event time.
3. DELETE /FeatureGroup/customers?RecordIdentifierValueAsString=C1&EventTime=2026-01-01T00:00:00Z&DeletionMode=HardDelete — 400 ValidationError "Validation Error: Delete event time must be same or after latest record event time." and no object appended (the documented refusal).
4. DELETE /FeatureGroup/customers?RecordIdentifierValueAsString=C1&EventTime=2026-01-01T00:00:00Z&DeletionMode=HardDelete&TargetStores=OfflineStore — 200 instead, because delete_record.rs:291-293 never enters the comparison; list the offline prefix in S3 and a delete marker object now exists under year=2026/month=01/day=01/hour=00, which the model says is not written for a HardDelete whose EventTime is older than the record's.
