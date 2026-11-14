Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# CreateBillingView silently collapses a duplicated resourceTags key that TagResource refuses with ValidationException

_Filed: 2026-11-01T11:37:09Z_
_Operation: CreateBillingView | Class: validation_

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` refuses a request carrying the same tag key twice, with the stated reason that the stored value would otherwise be decided by arrival order. `CreateBillingView` takes the same `ResourceTagList` member (`ResourceTagList` carries no `@uniqueItems`, so duplicates reach the handler) and collects it straight into the view's `BTreeMap` tag set: a repeated key is accepted with 200, one of the two values is dropped, and which one survives depends on the order the entries happened to arrive in. The two writers of the one tag set therefore apply different rules to identical input, and a caller that creates with duplicate keys gets no signal that half of its request was discarded.

## Evidence

Sibling writer that refuses — services/service-com-amazonaws-billing-awsbilling/src/ops/tag_resource.rs:205-219:
```
// A tag set is keyed by key, so one request may not carry a key twice:
// the two entries would decide the stored value by arrival order.
let mut requested: std::collections::BTreeMap<String, String> = ...;
for tag in &input.resource_tags {
    if requested.insert(tag.key.clone(), tag.value.clone().unwrap_or_default()).is_some() {
        return Err(invalid_field(
            "resourceTags",
            &format!("the tag key {} is given more than once", tag.key),
        ));
    }
}
```
The handler under review — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:268-273, which performs the very collapse the sibling calls out, with no duplicate check:
```
tags: input
    .resource_tags
    .unwrap_or_default()
    .into_iter()
    .map(|tag| (tag.key, tag.value.unwrap_or_default()))
    .collect(),
```
The destination is a map, so the collapse is silent — services/service-com-amazonaws-billing-awsbilling/src/state.rs:337-338: "/// The view's resource tags, by key.\n    pub tags: BTreeMap<String, String>,". The modelled list has no uniqueness constraint to catch it at the boundary: `smithy_query(query="shape", shape="ResourceTagList")` -> `{"type": "list", "length": {"min": 0, "max": 200}, "member": "ResourceTag"}`.
Existing coverage pins only the TagResource half — services/service-com-amazonaws-billing-awsbilling/tests/all/source_views_tags_and_policy.rs:818-840: "// One request may not carry the same key twice: the stored value would depend\n    // on arrival order." asserting `status == 400` / `ValidationException`. No test sends duplicate `resourceTags` to CreateBillingView.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call TagResource(resourceArn=<a custom view>, resourceTags=[{key:"env",value:"dev"},{key:"env",value:"prod"}]) — observe 400 ValidationException (pinned by tests/all/source_views_tags_and_policy.rs:820-840).
3. Call CreateBillingView(name="dup-tags", sourceViews=[<primary arn>], resourceTags=[{key:"env",value:"dev"},{key:"env",value:"prod"}]) — observe 200.
4. Call ListTagsForResource on the minted ARN — observe a single `env` tag whose value is whichever entry arrived last, with no error reported for the discarded one.
