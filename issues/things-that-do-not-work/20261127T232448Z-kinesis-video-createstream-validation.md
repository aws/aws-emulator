Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# CreateStream stores Tags without the TagKey/TagValue checks TagStream applies, and ignores the stream retention ceiling UpdateDataRetention enforces

_Filed: 2026-11-27T23:24:48Z_
_Operation: CreateStream | Class: validation_

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateStream persists whatever `Tags` map the request carries and whatever `DataRetentionInHours` it carries, skipping two rules the crate already implements elsewhere for the very same state fields.

1. Tag key/value bounds. `CreateStreamInput$Tags` targets the SAME `ResourceTags` map shape as `TagStreamInput$Tags`. The crate documents, in `state.rs`, that the framework's request walk does NOT check either side of a MAP, which is why the tagging handlers enforce `TagKey`/`TagValue` themselves. `TagStream` does exactly that; `CreateStream` collects the map straight into the stored record. So `CreateStream` with `Tags: {"": "v"}` (an empty key, below `TagKey`'s `@length(min: 1)`) or a 300-character tag value (past `TagValue`'s `@length(max: 256)`) is answered 200 and the illegal tag is persisted and later reported by `ListTagsForStream`, while the identical tag sent to `/tagStream` is refused 400 `InvalidArgumentException` — a refusal an existing test pins.

2. Retention ceiling. `state::MAX_DATA_RETENTION_IN_HOURS` is documented as "The most hours a stream may retain data for", quoted from the model, and `UpdateDataRetention` refuses a result past it. `CreateStream` applies no such bound, so a stream can be created with `DataRetentionInHours: 1000000`; `DescribeStream` then reports it, and the stream sits permanently outside the range `UpdateDataRetention` says it must stay inside (any further INCREASE is refused against a value the create itself accepted).

Both symptoms are the same class on the same operation: input the crate elsewhere refuses is accepted and written here.

## Evidence

Framework does not validate map key/value, so handlers must — `src/state.rs:404-415`:
```
/// Why `key` is not a `TagKey`, or `None` when it is one.
///
/// A MAP KEY's own `@length` is not among the constraints the request walk's validator
/// checks — an empty key reaches the handler — so the operations that accept tag keys
/// enforce it themselves.
```
and `src/state.rs:428-431`: "The companion of [`tag_key_violation`]: a map VALUE's `@length` is checked by the request walk for a list or structure member, but not for the value side of a map, so the tagging operations check it here."

Sibling that applies the rule — `src/ops/tag_stream.rs:165-179`:
```
// Every key must be a `TagKey` and every value a `TagValue`; the request
// walk checks neither side of a MAP, so the modelled bounds and character
// classes are enforced before anything is written (see
// `crate::state::tag_key_violation`).
if let Some(violation) = input.tags.iter().find_map(|(key, value)| {
    crate::state::tag_key_violation("tags", key)
        .or_else(|| crate::state::tag_value_violation("tags", value))
}) {
```

The path that skips it — `src/ops/create_stream.rs:255`:
```
tags: input.tags.unwrap_or_default().into_iter().collect(),
```
(no call to `tag_key_violation`/`tag_value_violation` anywhere in `create_stream.rs`).

Model: `smithy_query(query="shape", shape="CreateStreamInput")` gives `"Tags": {"target": "ResourceTags", ...}` and `ResourceTags` is `{"key": {"target": "TagKey", "length": {"min": 1, "max": 128}}, "value": {"target": "TagValue", "length": {"min": 0, "max": 256}}}` — the same shape `TagStreamInput$Tags` targets.

Existing test pinning the refusal for the sibling — `tests/all/stream_tags.rs:687-693`:
```
Refusal {
    name: "TagStream with an empty tag key, below TagKey's minimum length",
    path: "/tagStream",
    body: |_| serde_json::json!({ "StreamName": "refusals-stream", "Tags": { "": "1" } }),
    status: 400,
    code: "InvalidArgumentException",
},
```
(test fn `the_tagging_surface_refuses_bad_requests_with_its_modeled_errors`, `tests/all/stream_tags.rs:747`). No test exercises `CreateStream` with an invalid tag key or value.

Retention ceiling — `src/state.rs:56-60`:
```
/// The most hours a stream may retain data for: "The minimum value for data retention
/// is 0 and the maximum value is 87600 (ten years)"
/// (`UpdateDataRetentionInput$DataRetentionChangeInHours` docs), so a published bound
/// rather than one chosen here.
pub const MAX_DATA_RETENTION_IN_HOURS: i64 = 87_600;
```
enforced in `src/ops/update_data_retention.rs:256-267` (`if updated > crate::state::MAX_DATA_RETENTION_IN_HOURS { ... InvalidArgumentException ... }`), skipped in `src/ops/create_stream.rs:253-254`:
```
// "The default value is 0, indicating that the stream does not persist data."
data_retention_in_hours: input.data_retention_in_hours.unwrap_or(0),
```
Existing tests only create at or below the bound (`tests/all/stream_lifecycle.rs:375`, `tests/all/stream_record_state_model.rs:113`, `tests/all/stream_deletion_state.rs:94` all use `87_600`), so no test justifies accepting more.

## Steps to Reproduce

1. POST /createStream with {"StreamName": "tagged", "Tags": {"": "v"}} — observe HTTP 200 and a stored tag under an empty key
2. POST /listTagsForStream with {"StreamName": "tagged"} — the empty key is reported back
3. POST /tagStream with {"StreamName": "tagged", "Tags": {"": "1"}} — observe HTTP 400 InvalidArgumentException from the same crate for the same tag (tests/all/stream_tags.rs:687)
4. POST /createStream with {"StreamName": "forever", "DataRetentionInHours": 1000000} — observe HTTP 200, and DescribeStream reporting 1000000
5. POST /updateDataRetention on that stream with INCREASE_DATA_RETENTION / 1 — observe the 400 refusal naming the 87600 bound the create ignored
