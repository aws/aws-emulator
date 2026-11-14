Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3d6397f705e98d5dd244e0a56d66f506c6322d86

# A record posted with the same FeatureName twice is stored twice in the OnlineStore, so GetRecord serves two values for one feature and a later UpdateRecord of that feature replaces only the first, leaving the pre-update value observable

_Filed: 2026-11-14T03:41:20Z_
_Operation: PutRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3d6397f705e98d5dd244e0a56d66f506c6322d86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared whole-record preparation of this data plane (`crate::state::prepare_record`, called by PutRecord and by BatchWriteRecord) maps the posted `Record` one-to-one onto the stored feature values with no uniqueness guard: `features: record.iter().map(StoredFeatureValue::from).collect()`. The modelled `Record` is `list<FeatureValue>` with `length min 1` and NO `uniqueItems`, so a request may legitimately carry the same `FeatureName` twice and the boundary validator accepts it; the handler then stores BOTH entries under one record.

Two consequences follow, both observable through this service's own reads:

1. `StoredRecord::project` returns every stored entry, so `GetRecord` (and `BatchGetRecord`) answer a `Record` carrying the same `FeatureName` twice with two different values. A client that reads "the value of feature X" gets whichever of the two it happens to keep.

2. `UpdateRecord`'s merge replaces only the FIRST stored entry whose name matches (`find(|held| held.feature_name == value.feature_name)`), so after an update of that feature the record still holds the stale second copy and a subsequent `GetRecord` still serves the pre-update value beside the new one. That contradicts the operation's own modelled sentence, reproduced in the handler at update_record.rs:80-81: "Updates one or more feature values for an existing record in the specified feature group. Features that you do not include in the request remain unchanged" — here a feature the request DOES include remains observable with its old value.

The crate already states the rule that one feature name must not be written twice: the delete-marker path in `crate::state::offline_store` explicitly suppresses the second occurrence when a group's record-identifier feature IS its event-time feature. The ingestion path into the ONLINE store skips that guard. (This is the online-store/UpdateRecord half of the problem; the already-filed PutRecord event-propagation issue 20261113T214639Z covers only the Parquet object replicated into the OfflineStore, which is a different store, a different code path and a different symptom.)

## Evidence

The model admits the input — `smithy_query(query="shape", shape="Record,FeatureValue")`:
```
"Record": {"type": "list", "length": {"min": 1}, "member": "FeatureValue"}
"FeatureValue": {"members": {"FeatureName": {"required": true, "length": {"min": 1, "max": 64}, "pattern": "^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$"}, ...}}
```
No `uniqueItems`, so `[{city,"Seattle"},{city,"Boston"}]` passes the framework's constraint walk and reaches the handler.

The unguarded store — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:456-464 (`prepare_record`, reached from src/ops/put_record.rs:245-247):
```
    let mut stored = StoredRecord {
        identifier,
        features: record.iter().map(StoredFeatureValue::from).collect(),
```
and the field it fills — src/state.rs:256-258:
```
    /// Every feature value of the record, in the order it was written. A `PutRecord`
    /// is a full over-write, so this is replaced wholesale.
    pub features: Vec<StoredFeatureValue>,
```

What a read then serves — src/state.rs:312-320 (`StoredRecord::project`), which filters by name only and keeps every match:
```
        self.features
            .iter()
            .filter(|feature| {
                feature_names.is_none_or(|names| names.contains(&feature.feature_name))
            })
            .map(StoredFeatureValue::to_feature_value)
            .collect()
```
(called by src/ops/get_record.rs:235 and src/ops/batch_get_record.rs:244).

The merge that updates only the first copy — src/ops/update_record.rs:434-453:
```
        for posted in &input.features {
            let value = crate::state::StoredFeatureValue::from(posted);
            match updated
                .features
                .iter_mut()
                .find(|held| held.feature_name == value.feature_name)
            {
                Some(held) => *held = value,
```
against the rule the same file quotes — src/ops/update_record.rs:80-81:
```
//! Updates one or more feature values for an existing record in the specified feature
//! group. Features that you do not include in the request remain unchanged.
```

The guard the SAME crate applies on its other write path — src/state/offline_store.rs:132-139:
```
        // A group whose identifier feature IS its event-time feature would otherwise be
        // written with one column twice; the two values are then the one column's.
        if definition.event_time_feature_name != definition.record_identifier_feature_name {
```

Test coverage: no test in this crate posts a duplicated `FeatureName` — `grep -n 'duplicate\|twice\|same FeatureName'` over the crate matches only src/state/offline_store.rs:133 (the guard above), src/service.rs:182 and a comment in tests/all/batch_write_record_bulk_ingestion.rs:46 — so the duplicate-name case is unpinned and refusing or collapsing it breaks no existing assertion. The lifecycle suites that do pin the read/merge shape all post distinct names (tests/all/online_store_record_lifecycle.rs:207 `get_record_returns_exactly_the_feature_values_put_record_stored`, tests/all/update_record_partial_update.rs:231 `update_record_replaces_only_the_features_the_request_names`).

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true, so the shared feature-group directory holds the definition this data plane resolves against.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"},{FeatureName:city,ValueAsString:"Boston"}] — `city` appears twice, which the modelled `Record` list permits (no `uniqueItems`), so the request reaches the handler and answers 200.
3. GET /FeatureGroup/customers?RecordIdentifierValueAsString=C1 — the response `Record` carries TWO entries named `city` (Seattle and Boston): state.rs:456-464 stored both and state.rs:312-320 projects both.
4. POST /FeatureGroup/customers/Record (UpdateRecord) with RecordIdentifierValueAsString="C1" and Features=[{FeatureName:event_time,ValueAsString:"2026-06-02T00:00:00Z"},{FeatureName:city,ValueAsString:"Denver"}] — 200.
5. Repeat the GetRecord: the record now serves city=Denver AND city=Boston. The feature the update named still exposes its pre-update value, because update_record.rs:434-453 replaced only the first matching entry — contradicting the operation's quoted rule that only the features NOT included remain unchanged.
6. Repeat step 2 as one entry of POST /BatchWriteRecord: the same `prepare_record` is used (batch_write_record.rs:285-296), so the batch API stores the duplicate the same way.
