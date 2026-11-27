Issue-Severity: LOW
Issue-Gap-Class: event-propagation
Issue-Commit: 4c602bb694b43bcf0075caf653c8c8008efcdb0d

# A failed offline-store replication is dropped without the trace its own Replicated::Failed documents, so a marker that never lands leaves no record anywhere

_Filed: 2026-11-27T03:07:27Z_
_Operation: DeleteRecord | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `4c602bb694b43bcf0075caf653c8c8008efcdb0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::offline_store::replicate` reports its outcome as `Replicated`, whose `Failed` variant documents itself as TRACED. Only ONE of the three places that produce `Failed` actually traces: the Amazon S3 refusal. The two that fail BEFORE the S3 call — an offline location that does not parse, and a row whose Parquet bytes cannot be laid out — just `return Replicated::Failed(..)` with no `tracing::` call at all. And every caller of `replicate` in this crate discards the returned value (`.await;`), while the same module marks its two constructors `#[must_use]`, so nothing holds the result either.

The consequence for DeleteRecord (and equally for PutRecord/BatchWriteRecord, which call the same function): the deleted-record marker the operation's own doc says it appends — "In both cases, Feature Store appends the deleted record marker to the OfflineStore" — silently never lands, the call answers 200, and NO log line names the feature group or the reason. The one failure mode the module itself says it guards for (a feature column colliding with one of the three columns Feature Store adds) produces its message into that dropped value, so the guard reports to nobody. The defect is not that the client is left uninformed (that is deliberate and documented); it is that the documented trace does not exist on two of the three failure paths, so an offline-store delivery can fail completely invisibly.

## Evidence

The documented rule, services/.../src/state/offline_store.rs:202-206:
```
    /// Nothing was written and the reason is not a permission: the location could not be
    /// read, the bytes could not be laid out, or Amazon S3 refused the write (no such
    /// bucket, say). Traced, never reported to the ingesting client, which AWS answers
    /// before the replication is made at all.
    Failed(String),
```

The two paths that break it — no `tracing::` anywhere in either, offline_store.rs:227-233:
```
    let Some(location) = OfflineStoreLocation::parse(&delivery.resolved_output_s3_uri) else {
        return Replicated::Failed(format!(
            "the offline store of feature group '{group_name}' is at \
             '{}', which names no Amazon S3 location",
            delivery.resolved_output_s3_uri
        ));
    };
```
and offline_store.rs:260-263:
```
    let body = match record_parquet_bytes(group_name, record, written_at) {
        Ok(body) => body,
        Err(error) => return Replicated::Failed(error),
    };
```

The path that does honour it, offline_store.rs:273-285: `let reported = ...; ::tracing::debug!(feature_group = group_name, bucket = location.bucket, key = key, error = reported, "the record could not be written to the offline store");` — and the withheld path too, offline_store.rs:252-258.

Every caller drops the value: delete_record.rs:349-362 `crate::state::offline_store::replicate(ctx.callback(), ..., &crate::state::offline_store::OfflineRecord::delete_marker(&definition, &identifier, &input.event_time), ::std::time::SystemTime::now(),) .await;` — likewise put_record.rs:289-297 and batch_write_record.rs:361-371. The same module marks `OfflineRecord::of` (offline_store.rs:99) and `OfflineRecord::delete_marker` (offline_store.rs:148) `#[must_use]`, but `pub async fn replicate` (offline_store.rs:219) carries no such attribute.

The guard whose message is lost, offline_store.rs:302-316: "// A feature column named after one of the three added ones would make the schema declare that column TWICE ... so this states the premise the layout rests on instead of relying on it." followed by `return Err(format!("the row of feature group '{group_name}' carries the feature '{name}', which is the name of a column Feature Store adds to every offline record"))`.

Test coverage of the surrounding behaviour: tests/all/offline_store_replication.rs:911 ("RULE: a deletion of this data plane is replicated into the feature group's offline store") and tests/all/offline_store_object_visibility.rs:595 ("RULE: a deletion of this data plane APPENDS the deleted record marker") assert the SUCCESS path only; no test exercises either silent `Failed` branch.

## Steps to Reproduce

1. Register a feature group in the shared FeatureGroupDirectory whose `offline_store` delivery carries a `resolved_output_s3_uri` that is not an `s3://` URI (the field `OfflineStoreLocation::parse` rejects at offline_store.rs:227).
2. PutRecord one record into that group, then call `DELETE /FeatureGroup/<name>?RecordIdentifierValueAsString=...&EventTime=...` with no `TargetStores` (so the offline store is targeted).
3. Observe HTTP 200 and no object written anywhere.
4. Search the emulator log at any level for the feature group name or the message text "which names no Amazon S3 location": nothing is emitted, because the branch at offline_store.rs:227-233 returns `Replicated::Failed` without tracing and delete_record.rs:349-362 discards the value.
5. Compare with the S3-refusal branch at offline_store.rs:273-285, which emits `::tracing::debug!` for the same `Replicated::Failed` outcome, and with the variant's own doc at offline_store.rs:202-206 that says `Failed` is "Traced".
