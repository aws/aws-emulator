Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 6111c3fa081e7f8e952c1f0c4a75668e426695a2

# The offline-store Parquet row is built with one column per posted feature and no uniqueness guard, though the delete-marker path in the same module guards exactly that

_Filed: 2026-11-13T21:46:39Z_
_Operation: PutRecord | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `6111c3fa081e7f8e952c1f0c4a75668e426695a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`OfflineRecord::of` maps the posted `Record` one-to-one onto the columns of the Parquet object that is replicated into the feature group's OfflineStore, and `record_parquet_bytes` then declares one schema field per entry of that list. `Record` is modelled `list<FeatureValue>` with `length min 1` and no `uniqueItems`, so a request may legitimately carry the same `FeatureName` twice; such a record is laid out as an object whose schema declares the SAME column twice.

The very same module already states the rule that one column must not be written twice — `OfflineRecord::delete_marker` explicitly suppresses the second occurrence when a group's record-identifier feature IS its event-time feature — so the ingestion path skips a guard its sibling path applies. Whichever way the Parquet writer answers a duplicated field name, the outcome is wrong for the client: either the object that lands in Amazon S3 declares one column twice, or `record_parquet_bytes` returns `Err`, the replication becomes `Replicated::Failed` — which the module documents as "Traced, never reported to the ingesting client" — and `PutRecord` discards the result and answers 200 while nothing at all was appended to the OfflineStore. The same path is taken by `BatchWriteRecord` (which calls `OfflineRecord::of` per entry) and by the batch's replication loop.

## Evidence

The model allows the input: `smithy_query(query="shape", shape="BatchWriteRecordEntry")` -> `"Record": {"target": "Record", "required": true, "length": {"min": 1}, "type": "list", "doc": "<p>List of FeatureValues to be inserted. This will be a full over-write.</p>"}` — a list, `length min 1`, no `uniqueItems`, over `FeatureValue` whose `FeatureName` carries only `length 1..64` and a `@pattern`.

The unguarded ingestion path — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state/offline_store.rs:85-97:
```
        let features = record
            .iter()
            .map(|feature| {
                let value = match (&feature.value_as_string, &feature.value_as_string_list) {
...
                (feature.feature_name.clone(), value)
            })
            .collect();
```
and the schema built from it — src/state/offline_store.rs:275-278:
```
    let mut fields = Vec::with_capacity(record.features.len() + 3);
    for (name, _) in &record.features {
        fields.push(::std::sync::Arc::new(text_column(name)?));
    }
```

The other side of the inconsistency — the sibling path in the SAME module that does apply the rule, src/state/offline_store.rs:132-139:
```
        // A group whose identifier feature IS its event-time feature would otherwise be
        // written with one column twice; the two values are then the one column's.
        if definition.event_time_feature_name != definition.record_identifier_feature_name {
            features.push((
                definition.event_time_feature_name.clone(),
                event_time.to_owned(),
            ));
        }
```

Why a layout failure is invisible to the client — src/state/offline_store.rs:176-180:
```
    /// Nothing was written and the reason is not a permission: the location could not be
    /// read, the bytes could not be laid out, or Amazon S3 refused the write (no such
    /// bucket, say). Traced, never reported to the ingesting client, which AWS answers
    /// before the replication is made at all.
    Failed(String),
```
and src/state/offline_store.rs:234-237 (`record_parquet_bytes` error becomes `Failed`), with the caller discarding the verdict — src/ops/put_record.rs:292-306:
```
        if crate::state::targets_offline_store(input.target_stores.as_ref()) {
            if let Some(delivery) = definition.offline_store.as_ref() {
                crate::state::offline_store::replicate(
...
                .await;
            }
        }

        Ok(PutRecordOutput)
```

Test coverage: the offline-store suites only ever post records with distinct feature names — src/state/offline_store.rs:417-429 and tests/all/offline_store_replication.rs:450 ("RULE: a record ingested into a feature group with an `OfflineStoreConfig` is written as…"), tests/all/offline_store_object_visibility.rs:424 — so no test pins the duplicate-name case either way.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time, OnlineStoreConfig.EnableOnlineStore=true and an OfflineStoreConfig pointing at an S3 location the group's role may write (so the shared directory records an `offline_store` delivery).
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2026-06-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"},{FeatureName:city,ValueAsString:"Boston"}] — `city` appears twice, which the modelled `Record` list permits (no `uniqueItems`), so the boundary validator accepts it and the handler runs.
3. Observe the 200 answer, then list the group's offline S3 prefix: either the appended Parquet object declares the `city` column twice (offline_store.rs:275-278 pushed two fields of that name), or no object was appended at all because `record_parquet_bytes` refused the layout and the failure was only traced (offline_store.rs:176-180, 234-237) while put_record.rs:292-306 discarded it and still answered 200.
4. Contrast with a deletion of the same group whose identifier feature equals its event-time feature: `OfflineRecord::delete_marker` (offline_store.rs:132-139) refuses to emit that column twice, i.e. the crate already states the rule the ingestion path skips.
5. Repeat step 2 as one entry of POST /BatchWriteRecord — the same `OfflineRecord::of` is used (batch_write_record.rs:302-310), so the batch API behaves identically.
