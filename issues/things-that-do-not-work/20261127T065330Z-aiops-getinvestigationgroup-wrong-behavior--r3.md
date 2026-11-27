Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5616c0fe372bb644c36eb1b1314ae3023677a93c

# A collection member explicitly set to an empty value is reported absent, though the crate's own rule is that an empty collection is still reported

_Filed: 2026-11-27T06:53:30Z_
_Operation: GetInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5616c0fe372bb644c36eb1b1314ae3023677a93c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The group reader drops `tagKeyBoundaries`, `chatbotNotificationChannel` and `crossAccountConfigurations` from the response whenever the STORED collection is empty. The three helpers that build those members say they omit the member "when none were ever set", but the condition they actually test is emptiness, and the update path stores an EMPTY collection whenever the request carries one (an empty `crossAccountConfigurations` is a value the model explicitly admits, `@length` 0..25). So a caller that configures cross-account roles or tag key boundaries and later CLEARS them with `UpdateInvestigationGroup {"crossAccountConfigurations": [], "tagKeyBoundaries": []}` reads the group back as if those members had never been configured: the stored value (an empty collection) is not reported at all.

The crate decides the opposite question the opposite way elsewhere in the same crate: `ListTagsForResource` reports an empty `tags` map on purpose, with the rule spelled out in a comment — a resource with no tags still HAS a tag set, so the member is always reported and a caller never has to tell "no tags" apart from "the field was omitted" — and `ListInvestigationGroups` likewise always answers `investigationGroups`, empty page included. The group reader applies the inverse rule to three members whose model admits an empty value, so one crate answers both ways for the same question and the stored state of a cleared member is unobservable.

## Evidence

Omission keyed on emptiness, under a comment that claims a different condition — `services/service-com-amazonaws-aiops-aiops/src/state.rs:184`:
```
    /// `tagKeyBoundaries`, omitted when none were ever set.
    pub fn tag_key_boundaries_output(&self) -> Option<Vec<String>> {
        (!self.tag_key_boundaries.is_empty()).then(|| self.tag_key_boundaries.clone())
    }

    /// `crossAccountConfigurations`, omitted when none were ever set.
    pub fn cross_account_configurations_output(&self) -> Option<Vec<CrossAccountConfiguration>> {
        (!self.cross_account_configurations.is_empty())
            .then(|| self.cross_account_configurations.clone())
    }
```
(`chatbot_notification_channel_output` at `state.rs:175` is the same shape: `(!self.chatbot_notification_channel.is_empty()).then(...)`.)

The update path DOES store the empty collection the request carried — `services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:320`:
```
        if let Some(tag_key_boundaries) = input.tag_key_boundaries {
            group.tag_key_boundaries = tag_key_boundaries;
        }
...
        if let Some(cross_account_configurations) = input.cross_account_configurations {
            group.cross_account_configurations = cross_account_configurations;
        }
```
and the reader hands those helpers straight to the response — `services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group.rs:204`:
```
            chatbot_notification_channel: group.chatbot_notification_channel_output(),
            tag_key_boundaries: group.tag_key_boundaries_output(),
            ...
            cross_account_configurations: group.cross_account_configurations_output(),
```

The opposite rule, stated in this same crate — `services/service-com-amazonaws-aiops-aiops/src/ops/list_tags_for_resource.rs:175`:
```
        // A resource carrying no tags still HAS a tag set — an empty one — so the
        // member is always reported and a caller never has to tell "no tags" apart
        // from "the field was omitted".
        Ok(ListTagsForResourceOutput {
            tags: Some(group.tags.clone().into_iter().collect()),
        })
```
and `services/service-com-amazonaws-aiops-aiops/src/ops/list_investigation_groups.rs:267`: `investigation_groups: Some(investigation_groups)` — an empty page still carries the member.

The model admits the empty value that is then unreportable: `smithy_query(query="operation",operation="UpdateInvestigationGroup")` gives `"crossAccountConfigurations": {"target": "CrossAccountConfigurations", "length": {"min": 0, "max": 25}, "type": "list"}`.

Test coverage: `tests/all/investigation_groups.rs:464` pins only the NEVER-set case ("A member the request never carried and the service has no default for is reported by nobody"), which this issue does not touch. The explicit-empty case is encoded, without a stated reason, by one filter in `tests/all/investigation_group_singleton.rs:1021`: `let want = list.as_array().filter(|list| !list.is_empty());` — the row `("an empty list", entries(&[]), None)` is accepted and then expected to read back absent; the accepted empty-list UPDATE row asserts nothing about the read-back at all (`investigation_group_singleton.rs:1088`).

## Steps to Reproduce

1. CreateInvestigationGroup in us-west-2 with `{"name":"g","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","tagKeyBoundaries":["Service"],"crossAccountConfigurations":[{"sourceRoleArn":"arn:aws:iam::111111111111:role/Source"}]}`.
2. GetInvestigationGroup on the returned ARN: both `tagKeyBoundaries` and `crossAccountConfigurations` are reported.
3. UpdateInvestigationGroup (PATCH /investigationGroups/{arn}) with `{"tagKeyBoundaries":[],"crossAccountConfigurations":[]}` — accepted (200), and the empty collections are stored (update_investigation_group.rs:320/330).
4. GetInvestigationGroup again: both members are ABSENT from the response, so the stored empty value is not reported and a caller cannot tell the clear from a group that was never configured — while a group with no tags still reports `tags: {}` from ListTagsForResource in the same crate.
