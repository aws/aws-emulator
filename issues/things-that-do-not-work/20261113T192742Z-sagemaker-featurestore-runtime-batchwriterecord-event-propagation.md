Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 7461ab1bbe540afe8063ce03ee5477ebb649df48

# An entry BatchWriteRecord reports in UnprocessedEntries has already been replicated into the OfflineStore, where PutRecord's identical capacity refusal writes nothing

_Filed: 2026-11-13T19:27:42Z_
_Operation: BatchWriteRecord | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `7461ab1bbe540afe8063ce03ee5477ebb649df48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

BatchWriteRecord queues each entry's OfflineStore replication BEFORE it asks the online store whether it has room for the entry. When the store has no room — `group_or_create` answers `None` (the emulator already holds `MAX_FEATURE_GROUPS` groups) or `ingest` answers `AtCapacity` — the handler reports the entry in `UnprocessedEntries` ("A list of entries that were not processed. These entries can be retried.") and then, after releasing the store guard, performs the already-queued offline write anyway. So an entry the response declares NOT processed has in fact been processed into the group's offline store, and the retry the response invites appends a SECOND offline row for the same record.

PutRecord — the single-record writer whose behaviour this handler's own comments claim to reproduce exactly — does the opposite: its online block returns the modelled `ServiceUnavailable` for both capacity cases BEFORE reaching the offline replication, so a capacity-refused PutRecord writes nothing anywhere. The two whole-record writers therefore disagree about what the OfflineStore holds after a write the bounded store could not take, in the one case the crate's own comment says cannot differ.

## Evidence

The batch handler queues the offline write first — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/batch_write_record.rs:298-310:
```
            // The OfflineStore side of the entry, held for the awaited write below: the
            // same replication `PutRecord` makes, so a record written in a batch lands in
            // the offline store exactly as the same record written on its own — including a
            // historic one, which is written ONLY there.
            if crate::state::targets_offline_store(entry.target_stores.as_ref()) {
                if let Some(delivery) = definition.offline_store.as_ref() {
                    offline_writes.push((
```
…and only AFTERWARDS discovers the entry cannot be processed — batch_write_record.rs:322-334:
```
            // A write the emulator's bounded store has no room for is not the caller's
            // mistake, so it is not an entry error: it is an entry that was not
            // processed, which the model documents as one that can be retried.
            let Some(group) = store.group_or_create(&entry.feature_group_name) else {
                unprocessed_entries.push(entry.clone());
                continue;
            };
            match group.ingest(prepared) {
                ...
                crate::state::Ingested::AtCapacity => unprocessed_entries.push(entry.clone()),
            }
```
The queued write is then made unconditionally — batch_write_record.rs:338-353:
```
        // The replications, once this service's own store guard is released: each is a write
        // into another service, and the batch's answer does not depend on it ...
        for (group_name, delivery, record) in &offline_writes {
            crate::state::offline_store::replicate(
```

The sibling it claims parity with refuses BEFORE replicating — services/.../src/ops/put_record.rs:263-284:
```
            let Some(group) = store.group_or_create(&input.feature_group_name) else {
                return Err(at_capacity(format!(
                    "the emulator's online store already holds {} feature groups",
                    crate::state::MAX_FEATURE_GROUPS
                )));
            };
            ...
                crate::state::Ingested::AtCapacity => {
                    return Err(at_capacity(format!(
                        "the online store of feature group '{key}' already holds {} records",
```
with the offline replication only reached past that return — put_record.rs:292-304:
```
        if crate::state::targets_offline_store(input.target_stores.as_ref()) {
            if let Some(delivery) = definition.offline_store.as_ref() {
                crate::state::offline_store::replicate(
```

The parity the batch handler asserts for itself — batch_write_record.rs:24-26: "An entry written to a feature group that has an OfflineStore is replicated into it by the same write `PutRecord` makes ([`crate::state::offline_store`])." — and the retry contract it relies on, batch_write_record.rs:14-16: "An entry the emulator's bounded store has no room for is reported in `UnprocessedEntries`, which the model documents as the entries that \"can be retried\"." The model member itself: OPERATIONS.md:86-87 — "`UnprocessedEntries` (UnprocessedBatchWriteRecordEntries, len 0..) **required** — A list of entries that were not processed. These entries can be retried."

Test coverage: the capacity bound is pinned only for PutRecord, against groups created with NO offline store — tests/all/online_store_record_lifecycle.rs:1354-1406 (`the_online_store_refuses_a_write_past_its_capacity_bound`, asserting 503/ServiceUnavailable). The batch replication tests never reach capacity — tests/all/offline_store_replication.rs:755-759 (`a_batch_write_replicates_every_entry_including_a_historic_one`) — so no test pins an offline object for an entry reported unprocessed, and a fix that skips the queued write in that case leaves every existing assertion unchanged.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup for `MAX_FEATURE_GROUPS` (1000) distinct names with OnlineStoreConfig.EnableOnlineStore=true, and PutRecord one record into each (as tests/all/online_store_record_lifecycle.rs:1366-1383 does), so the emulator's online store holds its maximum number of groups.
2. CreateFeatureGroup one more group `latecomer`, this time WITH an OfflineStoreConfig whose S3Uri the group's RoleArn may write.
3. POST /BatchWriteRecord with Entries=[{FeatureGroupName:"latecomer", Record:[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T00:00:00Z"}]}].
4. The response is 200 with Errors=[] and UnprocessedEntries=[the entry] — `group_or_create` answered None (batch_write_record.rs:325-328), so nothing was stored online.
5. List the group's resolved offline output path in S3: one Parquet object for record C1 is nonetheless present, written by the replication queued at batch_write_record.rs:304-308 and performed at batch_write_record.rs:343-353.
6. Retry the entry exactly as the response invites: a SECOND offline object for the same record is appended, so the offline store now holds the record twice while the online store holds it zero times.
7. Contrast PUT /FeatureGroup/latecomer with the same Record: it answers 503 ServiceUnavailable from put_record.rs:264-268 and writes NO offline object, because the capacity refusal returns before put_record.rs:292.
