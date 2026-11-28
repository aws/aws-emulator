Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6efdcc2c184ddba11bf2d42c3ae06c16d4d6253e

# TagResource/UntagResource mutate a stored rule without bumping LastModifiedTimestamp, unlike every other mutating handler

_Filed: 2026-11-25T16:42:47Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6efdcc2c184ddba11bf2d42c3ae06c16d4d6253e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` and `UntagResource` write to the stored rule (`rule.tags`), and that write is observable: `DescribeNotificationRule` reports the rule's tag map. Neither handler touches `rule.last_modified_timestamp`, so a tag added or removed leaves the rule reporting the instant of its last non-tag change — or of its creation.

Every other handler in this crate that mutates a stored rule does set it: `UpdateNotificationRule`, `Subscribe`, `Unsubscribe` and `DeleteTarget` all assign `st::now_timestamp()` to the same field on the same struct, with `DeleteTarget` going as far as taking ONE instant for every rule it touches. The two tagging paths are the only rule mutators that skip the step, and no comment or test in the crate states a reason for the difference, while the model documents the member as the time the rule "was most recently updated".

## Evidence

The four siblings that bump the field:
`src/ops/update_notification_rule.rs:277`: `rule.last_modified_timestamp = st::now_timestamp();`
`src/ops/subscribe.rs:243`: `rule.last_modified_timestamp = st::now_timestamp();`
`src/ops/unsubscribe.rs:183-185`:
```
if detached {
    rule.last_modified_timestamp = st::now_timestamp();
}
```
`src/ops/delete_target.rs:170-177`:
```
// ONE instant for every rule this call touches: they all stopped carrying the
// target at the same moment.
let modified_at = st::now_timestamp();
...
        rule.last_modified_timestamp = modified_at;
```
The two tagging mutators that do not, each writing the rule under the store lock:
`src/ops/tag_resource.rs:219-222`:
```
if merged.len() > st::MAX_TAGS_PER_RULE {
    return Err(over_tag_limit());
}
rule.tags = merged;
```
`src/ops/untag_resource.rs:196-198`:
```
for key in &input.tag_keys {
    rule.tags.remove(key);
}
```
The write is observable — `src/ops/describe_notification_rule.rs:228-231`:
```
last_modified_timestamp: Some(rule.last_modified_timestamp),
// `Tags` reports the rule's own tag map: ...
tags: Some(rule.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
```
and the member the model describes, `smithy_query(query="doc",shape="DescribeNotificationRuleResult",member="LastModifiedTimestamp")`: "The date and time the notification rule was most recently updated, in timestamp format."
Test coverage: only the update path is pinned — `tests/all/notification_rule_lifecycle.rs:1227-1232` asserts `LastModifiedTimestamp must be bumped` for `/updateNotificationRule`; a grep for `Timestamp` over `tests/` shows no assertion at all for `/tagResource` or `/untagResource`, so nothing pins the current behaviour or states a reason for it.

## Steps to Reproduce

1. POST /createNotificationRule with a well-formed body; note the `Arn`.
2. POST /describeNotificationRule and record `LastModifiedTimestamp`.
3. Sleep a few milliseconds (as tests/all/notification_rule_lifecycle.rs:1213 does), then POST /tagResource with `{"Arn": arn, "Tags": {"team": "dev"}}`.
4. POST /describeNotificationRule again: `Tags` now carries `team=dev` but `LastModifiedTimestamp` is unchanged.
5. Repeat with POST /untagResource/{Arn}?tagKeys=team: the tag is gone, `LastModifiedTimestamp` is still unchanged. Compare with the same sequence through /subscribe or /updateNotificationRule, which do bump it.
