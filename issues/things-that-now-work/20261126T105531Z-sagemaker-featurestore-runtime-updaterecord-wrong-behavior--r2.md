Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6c92ba71c5d15ae01ba85514d047ce7aed8fd467

# An UpdateRecord that states no TtlDuration stamps the feature group's CURRENT default onto a record stored under no TTL, which can hard-delete the record it just updated

_Filed: 2026-11-26T10:55:31Z_
_Operation: UpdateRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6c92ba71c5d15ae01ba85514d047ce7aed8fd467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateRecord` re-settles the stored record's TTL DURATION on every call, even when the request states no `TtlDuration` at all:

```rust
updated.ttl_secs =
    crate::state::effective_ttl_seconds(&definition, ttl_secs.or(updated.ttl_secs));
updated.recompute_expiry();
```

`effective_ttl_seconds` is `record_ttl_secs.or(definition.default_ttl_seconds)`, so when the request names no duration AND the stored record carries none (`ttl_secs == None`), the record's TTL setting is CHANGED from "none" to whatever default the feature group carries at the moment of the update. The model member doc says the opposite for exactly that request: "If you omit this parameter, the record's existing TTL setting remains unchanged." This crate's own state module states the same rule, and states that `ttl_secs` IS the record's TTL setting ("the duration the write that stored it was under"), so overwriting a stored `None` with the group's current default contradicts both.

The divergence is reachable because the group-level default is NOT fixed at creation: the SageMaker control plane's `UpdateFeatureGroup` re-registers `default_ttl_seconds` in the shared directory this data plane resolves through ("To remove the default TtlDuration from an existing feature group, use the UpdateFeatureGroup API"). So: create a group with no default TTL, `PutRecord` a record (stored with `ttl_secs = None`, no expiry), let the control plane add a default `TtlDuration`, then post a plain partial update that names only the feature it changes. The update now derives `ExpiresAt = <the record's OLD event time> + <the new group default>`. When that instant is already past — an ordinary case, since the record's event time can be arbitrarily old while the new default is short — the very next call that touches the group purges the record (`purge_expired` hard-deletes every record whose `expires_at_secs <= now`). A documented feature merge therefore destroys the record it claimed to update, and a subsequent `GetRecord` answers an empty result.

The symmetric direction is handled correctly (a record that carries its own duration keeps it when the group's default is later removed), which is what makes this one case the outlier.

Note this is NOT the already-fixed `UpdateRecord`/wrong-behavior gap (a record whose `EventTime + TtlDuration` cannot be formatted keeping its enforced expiry while losing the reported `ExpiresAt`): that one is about the SPELLING of a derived expiry, this one is about the stored TTL DURATION being replaced on a request that states none.

## Evidence

Model text, reproduced in the handler header at services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/update_record.rs:123-127:
```
//! * `TtlDuration` (TtlDuration) — The time-to-live (TTL) duration for the record. Amazon
//!   SageMaker Feature Store deletes the record when `EventTime` + `TtlDuration`
//!   elapses. If you omit this parameter, the record's existing TTL setting remains
//!   unchanged. ...
```

The handler doing the opposite — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/update_record.rs:482-492:
```
            // "If you omit this parameter, the record's existing TTL setting remains
            // unchanged" (`TtlDuration`), so a request that names one replaces the
            // record's duration and one that does not keeps it — and where neither the
            // request nor the record states a duration, the record is under the DEFAULT
            // its feature group configures, the same precedence an ingestion settles a
            // record's TTL by (`crate::state::effective_ttl_seconds`). ...
            updated.ttl_secs =
                crate::state::effective_ttl_seconds(&definition, ttl_secs.or(updated.ttl_secs));
            updated.recompute_expiry();
```

What that applies — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:847-852:
```
pub fn effective_ttl_seconds(
    definition: &::service_common::feature_store_groups::FeatureGroupRecord,
    record_ttl_secs: Option<f64>,
) -> Option<f64> {
    record_ttl_secs.or(definition.default_ttl_seconds)
}
```

The crate's own statement that `ttl_secs` is the record's TTL SETTING, quoting the same model sentence — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:320-327:
```
    /// The record's `TtlDuration` in SECONDS — the duration the write that stored it was
    /// under, which is its own when it named one and otherwise its feature group's
    /// default ([`effective_ttl_seconds`]) — kept so a later call that moves the record's
    /// `EventTime` can re-derive `ExpiresAt = EventTime + TtlDuration` from it.
    /// `UpdateRecord` documents exactly that: "If you omit this parameter, the record's
    /// existing TTL setting remains unchanged", and the setting is the DURATION, not the
    /// instant it happened to fall due at.
    pub ttl_secs: Option<f64>,
```

Why a record can hold `ttl_secs = None` while its group now has a default — the control plane re-registers the directory record this data plane resolves through, at services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_feature_group.rs:253-273:
```
        // The group's DEFAULT TtlDuration is one of the things the shared directory
        // records for the FeatureStore Runtime data plane, and this operation is what
        // changes it ("To remove the default TtlDuration from an existing feature group,
        // use the UpdateFeatureGroup API"), so the new duration is read out while the
        // group is still borrowed and written to the directory below.
        let default_ttl_seconds = group
            .online_store_config
            .as_ref()
            .and_then(crate::state::online_store_default_ttl_seconds);
        ...
                record.default_ttl_seconds = default_ttl_seconds;
                held.register(record);
```

The loss that follows — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:135-142:
```
    pub fn purge_expired(&mut self, now_secs: f64) {
        if self.next_expiry_secs.is_none_or(|due| due > now_secs) {
            return;
        }
        self.records
            .retain(|_, record| record.expires_at_secs.is_none_or(|at| at > now_secs));
```

Test coverage: every row of the TTL suite creates the feature group with its default already in place before the record is ingested, so the record always already carries the duration the update re-settles — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/tests/all/feature_group_default_ttl.rs:721-724 ("Each row needs its own feature group: the default TTL is the GROUP's." … `emulator.create_feature_group(&group, default_ttl);`) and the expected expiry at tests/all/feature_group_default_ttl.rs:812 (`record_ttl_secs.or(default_ttl)`). No test covers a default TTL added AFTER the record was written, which is the only shape that distinguishes "keep the record's setting" from "apply the group's current default", so those rows stay green under a fix.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=record_id, EventTimeFeatureName=event_time, OnlineStoreConfig.EnableOnlineStore=true and NO OnlineStoreConfig.TtlDuration — the shared directory holds `default_ttl_seconds = None`.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:record_id,ValueAsString:"rec-1"},{FeatureName:event_time,ValueAsString:"2026-01-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"}] — 200. The record is stored with `ttl_secs = None` and no `ExpiresAt` (state.rs:672, `effective_ttl_seconds(definition, None)` == None).
3. GET /FeatureGroup/customers?RecordIdentifierValueAsString=rec-1&ExpirationTimeResponse=Enabled — 200, Record present, no ExpiresAt.
4. Through the control plane, UpdateFeatureGroup `customers` with OnlineStoreConfig.TtlDuration={Unit:"Hours",Value:1}; the directory record is re-registered with `default_ttl_seconds = Some(3600.0)` (update_feature_group.rs:258-273).
5. POST /FeatureGroup/customers/Record with {"RecordIdentifierValueAsString":"rec-1","Features":[{"FeatureName":"city","ValueAsString":"Portland"}]} — no TtlDuration member, no event-time feature. Answers 200.
6. GET /FeatureGroup/customers?RecordIdentifierValueAsString=rec-1 — the record is GONE (empty result): update_record.rs:489-491 set `ttl_secs = Some(3600.0)` and re-derived `ExpiresAt = 2026-01-01T01:00:00Z`, which is in the past, so `purge_expired` (state.rs:135-142) hard-deleted it on this read. Expected per the quoted member doc: the record's existing TTL setting (none) remains unchanged, the merge applies and `city` reads back as "Portland".
