Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 323db8fd2b9402dc51e6cd8f50a72cdf880cd07a

# A SoftDelete clobbers the record-identifier feature's value with the delete EventTime when a group's identifier feature IS its event-time feature, while the delete-marker path in the same crate guards that exact collision

_Filed: 2026-11-25T18:29:48Z_
_Operation: DeleteRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `323db8fd2b9402dc51e6cd8f50a72cdf880cd07a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`FeatureGroupStore::soft_delete_record` builds the online tombstone in two steps: it retains the two features the group's definition fixes (its `RecordIdentifierFeatureName` and its `EventTimeFeatureName`), then walks the retained features and overwrites the value of the one whose name equals the event-time feature with the deletion's `EventTime`.

Those two names can be the SAME name — the crate's own offline-store writer says so in as many words and guards it. When they are, `retain` keeps ONE feature, the following loop matches it (it IS the event-time feature), and its `ValueAsString` is replaced by the delete input `EventTime`. The record's identifier feature therefore no longer carries the record's identifier: the stored record is left with `features[identifier_feature].value_as_string == <delete EventTime>` while the record is still keyed, and still reports itself, under `StoredRecord::identifier == <original identifier>`.

That breaks two things the crate states in writing:

1. The rule `soft_delete_record` quotes as its own spec — the tombstone is "a record with the same RecordIdentifer as the original". After this call the tombstone's record-identifier feature carries the deletion's event time instead of the original identifier.
2. The documented invariant of the field the whole store is keyed by — `StoredRecord::identifier` is "the value of the group's record-identifier feature in this record". After this call the two disagree, so the store holds a record whose key and whose identifier feature say different things. Every other write of this plane keeps them in step (`prepare_record` derives `identifier` FROM that feature, and `UpdateRecord` refuses a posted identifier feature that does not equal the record's identifier).

The sibling path in this crate that lays out the OFFLINE row for the same deletion — `OfflineRecord::delete_marker`, called by this very handler a few lines later — handles the collision explicitly and correctly: it emits the identifier column and only adds the event-time column when the two names differ. So one call writes a correct marker offline and a corrupted tombstone online.

The fix is local: the online tombstone should treat the collision the way the delete marker already does, i.e. not overwrite the retained feature when the identifier feature and the event-time feature are one and the same. No served response changes for any group whose two names differ, so every existing test is unaffected.

## Evidence

The rule the function quotes as its own spec, and the claim that it keeps BOTH of the group's two features — services/.../src/state.rs:244-252:
```
    /// Leave the `SoftDelete` tombstone the model describes: "a record with the same
    /// RecordIdentifer as the original, but with `is_deleted` value set to True,
    /// `EventTime` set to the delete input EventTime, and other feature values set to
    /// null."
    ///
    /// The two features the tombstone KEEPS are the group's own
    /// `RecordIdentifierFeatureName` and `EventTimeFeatureName`, which the caller reads
    /// from the group's definition ([`resolve_group`]) and passes in: this store holds no
    /// definition of its own, and a group that resolves always has both names.
```

The code under it, which overwrites whatever feature matches the event-time name — services/.../src/state.rs:273-282:
```
        record.features.retain(|feature| {
            identifier_feature == feature.feature_name
                || event_time_feature == feature.feature_name
        });
        for feature in &mut record.features {
            if event_time_feature == feature.feature_name {
                feature.value_as_string = Some(event_time.to_owned());
                feature.value_as_string_list = None;
            }
        }
```
Nothing here distinguishes `identifier_feature == event_time_feature`, so in that case the single retained feature — the RECORD-IDENTIFIER feature — has its value replaced by the deletion's `EventTime`.

The invariant that is then broken, on the field the store is keyed by — services/.../src/state.rs:301-304:
```
pub struct StoredRecord {
    /// `RecordIdentifierValueAsString` — the value of the group's record-identifier
    /// feature in this record.
    pub identifier: String,
```
(`identifier` is left untouched by `soft_delete_record`, which only assigns `soft_deleted`, `event_time` and the feature values.)

The sibling path in this crate that DOES guard the same collision, reached by the same `DeleteRecord` call — services/.../src/state/offline_store.rs:142-149:
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

The handler that calls both, within one request — services/.../src/ops/delete_record.rs:319-325 (online) and 354-358 (offline):
```
                            } else {
                                group.soft_delete_record(
                                    &identifier,
                                    &input.event_time,
                                    &definition.record_identifier_feature_name,
                                    &definition.event_time_feature_name,
                                );
```
```
                    &crate::state::offline_store::OfflineRecord::delete_marker(
                        &definition,
                        &identifier,
                        &input.event_time,
                    ),
```

Test coverage: no test asserts anything about the tombstone's FEATURE VALUES. `tests/all/feature_group_default_ttl.rs:1004-1017` reads a tombstone out of the store but only for its `ttl_secs`/`expires_at`, and `tests/all/online_store_record_lifecycle.rs:997` exercises a SoftDelete only through `ListRecords` (which reports `StoredRecord::identifier`, the untouched key). Every test feature group is registered with distinct names (`record_identifier_feature_name: "customer_id"`, `event_time_feature_name: "event_time"`, e.g. tests/all/online_store_record_lifecycle.rs:38-45), so the collision branch is never entered by the suite.

## Steps to Reproduce

1. Register a feature group in the shared directory with `record_identifier_feature_name` and `event_time_feature_name` set to the SAME feature name (e.g. both `"event_time"`), `online_store_enabled: true` — the configuration services/.../src/state/offline_store.rs:142-149 explicitly contemplates.
2. `PUT /FeatureGroup/<group>` with `Record: [{"FeatureName":"event_time","ValueAsString":"2026-01-01T00:00:00Z"}]`. `prepare_record` (src/state.rs:570-581) reads the identifier from that one feature, so the record is stored under `identifier == "2026-01-01T00:00:00Z"` and its single feature carries the same value.
3. `DELETE /FeatureGroup/<group>?RecordIdentifierValueAsString=2026-01-01T00:00:00Z&EventTime=2026-02-01T00:00:00Z` (no `DeletionMode`, so the default `SoftDelete` runs).
4. Read the stored record out of `crate::state` (as tests/all/feature_group_default_ttl.rs:1004 does): `features[0].value_as_string` is now `"2026-02-01T00:00:00Z"` while `identifier` — the key the record is held and listed under — is still `"2026-01-01T00:00:00Z"`.
5. Compare with the offline row the same call wrote through `OfflineRecord::delete_marker`: it carries the original identifier `"2026-01-01T00:00:00Z"` in the one column, because that path checks `event_time_feature_name != record_identifier_feature_name` and the online path does not.
