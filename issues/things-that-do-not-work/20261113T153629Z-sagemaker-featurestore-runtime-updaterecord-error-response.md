Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0e34df6a74924531fb9e5f6908b55d51a4f24bc0

# UpdateRecord answers 200 for a feature group with no OnlineStore, where no record can exist, instead of the ResourceNotFound its own quoted rule states

_Filed: 2026-11-13T15:36:29Z_
_Operation: UpdateRecord | Class: error-response_

> _Note: this issue was identified against the code base at commit `0e34df6a74924531fb9e5f6908b55d51a4f24bc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateRecord takes an unconditional success shortcut for any feature group whose definition has `online_store_enabled == false`: it returns `Ok(UpdateRecordOutput)` (HTTP 200) before it ever looks for the record. This service holds records only in the OnlineStore it serves, so for such a group there is NO record under the requested `RecordIdentifierValueAsString` — never was, and none is created or written by this call. The operation nevertheless reports success, which is the one answer the model's own documented rule for a missing record excludes.

The contradiction is visible inside the handler itself, nine lines apart: the comment immediately below the shortcut quotes the rule verbatim ("The record must already exist. If the record does not exist or has been soft-deleted, the operation returns a ResourceNotFound error. To create a record, use PutRecord.") and implements it twice — `store.group_mut(...)` returning `None` is a 404, and `group.live_record(...)` returning `None` is a 404. So one and the same absence of a record is answered 404 when the group has an online store that happens to hold no record, and 200 when the group has no online store at all, decided by a property of the GROUP that the quoted rule says nothing about. Unlike `PutRecord` (whose doc does describe a write that lands only in the OfflineStore), `UpdateRecord` is modelled as requiring a pre-existing record, and the handler's own module doc admits the offline half of an update is not performed here at all — so the 200 reports a mutation that happened nowhere.

## Evidence

Model sentence (the documented failure mode) — `smithy_query(query="doc", shape="UpdateRecord")`:
"The record must already exist. If the record does not exist or has been soft-deleted, the operation returns a <code>ResourceNotFound</code> error. To create a record, use <code>PutRecord</code>."

The shortcut that contradicts it — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/update_record.rs:369-377:
```
        // A feature group that HAS no `OnlineStore` holds no online record to update, and
        // the update AWS would replicate to the OfflineStore is not replicated here (see
        // the module docs), so
        // there is nothing for this handler to do once the request itself has been found
        // well-formed — the same answer a request naming only the OfflineStore gets
        // below, and not a refusal, which no recording shows.
        if !definition.online_store_enabled {
            return Ok(UpdateRecordOutput);
        }
```
The same handler stating and enforcing the rule it just skipped — services/.../src/ops/update_record.rs:385-395:
```
        // "The record must already exist. If the record does not exist or has been
        // soft-deleted, the operation returns a ResourceNotFound error. To create a
        // record, use PutRecord." `live_record` is exactly that: present and not
        // soft-deleted. A feature group that resolves but has taken no record yet holds
        // no online store here, which is the same "no such record" answer.
        let Some(group) = store.group_mut(&input.feature_group_name) else {
            return Err(no_such_record(&identifier));
        };
        let Some(existing) = group.live_record(&identifier) else {
            return Err(no_such_record(&identifier));
        };
```
That the update is written nowhere for such a group — services/.../src/ops/update_record.rs:36-38: "The OfflineStore side of an UPDATE — the partially updated record AWS replicates there — is left to the issue that owns this operation: the ingestion of a WHOLE record is what [`crate::state::offline_store`] replicates today." (i.e. the 200 persists nothing anywhere.)

Test coverage, and the reason it states: services/.../tests/all/feature_group_resolution.rs:352-362 pins the current 200 —
```
            // What each class of group answers. A name that resolves to NO feature group
            // is refused everywhere. A group that resolves but has no `OnlineStore` is
            // refused on the READS — there is no online store for this data plane to
            // serve — while a WRITE to it is accepted and leaves the online store alone,
            // because AWS replicates such a write to the OfflineStore and this emulator
            // does not model that store.
            let refused = match group {
                Group::WithOnlineStore => false,
                Group::Absent => true,
                Group::WithoutOnlineStore => call.reads(),
            };
```
with `reads()` excluding UpdateRecord (tests/all/feature_group_resolution.rs:73-78: `matches!(self, Call::GetRecord | Call::ListRecords | Call::BatchGetRecord)`), so the row asserts 200 at :390-397. The stated reason is a claim about writes in general ("AWS replicates such a write to the OfflineStore"); the model sentence quoted above is specific to UpdateRecord and says the absence of a record is a `ResourceNotFound`, which is what contradicts that reason. Note also that the reason's premise ("this emulator does not model that store") no longer holds: the crate does model the offline store (src/state/offline_store.rs), and update_record.rs:36-38 says UpdateRecord simply does not use it.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `offlineonly` with RecordIdentifierFeatureName=record_id, EventTimeFeatureName=event_time and NO OnlineStore (OnlineStoreConfig absent / EnableOnlineStore=false), so the shared directory records `online_store_enabled: false` (services/common/src/feature_store_groups.rs:61-65).
2. POST /FeatureGroup/offlineonly/Record with {"RecordIdentifierValueAsString":"C1","Features":[{"FeatureName":"city","ValueAsString":"Seattle"}]} — no record named C1 has ever been created in that group.
3. Observe HTTP 200 with an empty body: the handler returned at update_record.rs:375-377 before the existence check, so the documented ResourceNotFound (404) for a record that does not exist is never answered and nothing was written to any store.
4. For contrast, CreateFeatureGroup `customers` WITH an OnlineStore and send the identical UpdateRecord to it without creating the record first: the same absence is answered 404 ResourceNotFound (update_record.rs:390-395), so one condition gets two verdicts.
