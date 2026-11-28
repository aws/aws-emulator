Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 95874034a8b8f8d63ab74cd551a8e6e28bd2f3c9

# UpdateRecord reads the group's record-identifier feature raw instead of through the crate's one scalar read, so a value posted in ValueAsStringList is refused as a value MISMATCH rather than with the crate's identifier_not_scalar refusal

_Filed: 2026-11-24T18:04:52Z_
_Operation: UpdateRecord | Class: error-response_

> _Note: this issue was identified against the code base at commit `95874034a8b8f8d63ab74cd551a8e6e28bd2f3c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, once, how every write of the data plane refuses the group's record-identifier feature when a request carries its value somewhere other than `ValueAsString`: `crate::state::identifier_not_scalar`, answered for `ScalarFeature::Unreadable` (a value in `ValueAsStringList`, or in neither value member). Its sibling for the event-time feature, `crate::state::event_time_not_scalar`, carries the same promise and says explicitly that it is 'stated once here, so a whole-record write and a partial update refuse the same posted value the same way'.

`UpdateRecord` honours that for the EVENT-TIME feature: it reads it with `crate::state::held_scalar_feature` and answers `crate::state::event_time_not_scalar` for `ScalarFeature::Unreadable` (src/ops/update_record.rs:306-315). For the RECORD-IDENTIFIER feature, twenty lines further down, the same handler does NOT use that reading: it hand-rolls `find(...)` over the posted features and compares `posted.value_as_string.as_deref() != Some(identifier)` (src/ops/update_record.rs:366-378). A `FeatureValue` for the identifier feature whose value sits in `ValueAsStringList` (or in neither member) has `value_as_string == None`, so it falls into the mismatch branch and the client is told

  "Features: the record identifier feature 'customer_id' must carry the RecordIdentifierValueAsString of the record being updated ('C1')"

which is false about the request: the request DID carry that value, in the other value member. The crate's own single statement for that input exists and is skipped, so the one rule this module promises is worded two ways depending on which operation refuses it — the partial update being the one that disagrees. (Both answers are 400 `ValidationError`, so this is the message/classification of the refusal, not the status.)

## Evidence

The crate's ONE statement of the rule, and the promise it carries — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/state.rs:496-506:
```
/// The refusal every write of this data plane answers for the group's RECORD-IDENTIFIER
/// feature carrying no value this plane can read as one value
/// ([`ScalarFeature::Unreadable`]) — a value in `ValueAsStringList`, or in neither value
/// member, which is why the wording states the member the value has to be in rather than the
/// one a request happened to use.
pub fn identifier_not_scalar(identifier_feature_name: &str) -> String {
    format!(
        "the record identifier feature '{identifier_feature_name}' must carry its value in \
         ValueAsString"
    )
}
```
Its event-time sibling, naming the partial update explicitly — src/state.rs:508-517:
```
/// The refusal every write of this data plane answers for the group's EVENT-TIME feature
/// carrying no value this plane can read as one value ([`ScalarFeature::Unreadable`]):
/// stated once here, so a whole-record write and a partial update refuse the same posted
/// value the same way.
pub fn event_time_not_scalar(event_time_feature_name: &str) -> String {
```
UpdateRecord DOES use the shared reading for the event-time feature — src/ops/update_record.rs:306-315:
```
        let posted_event_time =
            match crate::state::held_scalar_feature(&posted_features, &event_time_feature_name) {
                crate::state::ScalarFeature::Absent => None,
                crate::state::ScalarFeature::Value(value) => Some(value.to_owned()),
                crate::state::ScalarFeature::Unreadable => {
                    return Err(validation(crate::state::event_time_not_scalar(
                        &event_time_feature_name,
                    )))
                }
            };
```
and does NOT for the identifier feature — src/ops/update_record.rs:365-378:
```
        let identifier_feature = definition.record_identifier_feature_name.clone();
        if let Some(posted) = posted_features
            .iter()
            .copied()
            .find(|feature| feature.feature_name == identifier_feature)
        {
            if posted.value_as_string.as_deref() != Some(identifier.as_str()) {
                return Err(validation(format!(
                    "Features: the record identifier feature '{identifier_feature}' must carry \
                     the RecordIdentifierValueAsString of the record being updated \
                     ('{identifier}')"
                )));
            }
        }
```
The input reaches the handler: `FeatureValue`'s two value members are both optional (`smithy_query(query="shape", shape="FeatureValue")`: `"ValueAsString": {"length": {"min": 0, "max": 358400}, "pattern": ".*"}`, `"ValueAsStringList": {"target": "ValueAsStringList", ...}`), so a feature carrying only `ValueAsStringList` passes the boundary validator.

Test coverage: the shared identifier refusal is pinned for the whole-record writers only — tests/all/rejection_statements.rs:523 `async fn a_key_feature_without_one_readable_value_is_refused_by_every_whole_record_write()`, whose rows (:526-529) pair `IDENTIFIER_FEATURE` with `identifier_refusal()` (:488-494, the `identifier_not_scalar` text) and exercise `emulator.put(...)` and `emulator.batch_write(...)` only. The event-time counterpart DOES cover the partial update — tests/all/rejection_statements.rs:645 `for writer in ["PutRecord", "BatchWriteRecord", "UpdateRecord"]` — so no existing test pins UpdateRecord's wording for the identifier feature, and routing it through `held_scalar_feature`/`identifier_not_scalar` breaks none of them.

## Steps to Reproduce

1. Through the SageMaker control plane, CreateFeatureGroup `customers` with RecordIdentifierFeatureName=customer_id, EventTimeFeatureName=event_time and OnlineStoreConfig.EnableOnlineStore=true, so the shared feature-group directory holds the definition this data plane resolves against.
2. PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsString:"C1"},{FeatureName:event_time,ValueAsString:"2035-01-01T00:00:00Z"},{FeatureName:city,ValueAsString:"Seattle"}] — the record is stored (200).
3. POST /FeatureGroup/customers/Record (UpdateRecord) with RecordIdentifierValueAsString="C1" and Features=[{FeatureName:customer_id,ValueAsStringList:["C1"]},{FeatureName:city,ValueAsString:"Tulsa"}].
4. Observe 400 ValidationError whose Message is "Features: the record identifier feature 'customer_id' must carry the RecordIdentifierValueAsString of the record being updated ('C1')" — a mismatch verdict about a request that carried exactly that value, in ValueAsStringList.
5. Send the same shape to the whole-record writers for comparison: PUT /FeatureGroup/customers with Record=[{FeatureName:customer_id,ValueAsStringList:["C1"]},{FeatureName:event_time,ValueAsString:"2035-01-01T00:00:00Z"}] is refused with the crate's one statement, "the record identifier feature 'customer_id' must carry its value in ValueAsString" (state.rs:501), and POST /BatchWriteRecord reports that same text as the entry's ErrorMessage — so the three readings of one rule disagree, with UpdateRecord the odd one out.
6. Repeat step 3 with Features=[{FeatureName:customer_id}] (neither value member): same misleading mismatch message, where `ScalarFeature::Unreadable` and `identifier_not_scalar` cover exactly that form.
